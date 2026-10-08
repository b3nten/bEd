//! Real framed subprocess I/O around local editing, with delayed acknowledgements.
use bed_document_session::{ByteEdit, ClosePolicy, DocumentKind, EditorSession, SessionOptions};
use bed_editing::editor_commands::CursorReveal;
use bed_remote::{LocalBackend, RemoteClient, Request, SshTarget, serve_with};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::{Duration, Instant},
};

fn client(delay: bool) -> RemoteClient {
    configured_client(delay, false)
}
fn configured_client(delay_write: bool, delay_read: bool) -> RemoteClient {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command.arg("--stdio");
    if delay_write {
        command.env("BED_TEST_DELAY_WRITE", "1");
    }
    if delay_read {
        command.env("BED_TEST_DELAY_READ", "1");
    }
    RemoteClient::launch_command(command).unwrap()
}
fn session(root: &Path, client: RemoteClient, monitoring: bool) -> EditorSession {
    EditorSession::with_remote_options(
        SessionOptions {
            monitoring,
            ..SessionOptions::default()
        },
        SshTarget::new("fixture"),
        client,
        root.to_str().unwrap().into(),
    )
    .unwrap()
}
fn wait(session: &mut EditorSession, done: impl Fn(&EditorSession) -> bool) {
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        session.tick();
        if done(session) {
            return;
        }
        assert!(Instant::now() < until, "session timed out");
        thread::sleep(Duration::from_millis(2));
    }
}
fn open(session: &mut EditorSession, path: &Path) -> bed_document_session::DocumentId {
    assert!(session.request_open_file(path).unwrap().is_none());
    wait(session, |s| !s.open_pending(path));
    session.document_for_path(path).unwrap()
}
fn pump_for(session: &mut EditorSession, duration: Duration) {
    let until = Instant::now() + duration;
    while Instant::now() < until {
        session.tick();
        thread::sleep(Duration::from_millis(2));
    }
}
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("bed-remote-session-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        Self(fs::canonicalize(root).unwrap())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn main() {
    if !cfg!(unix) {
        println!("remote local-helper session fixtures require a Unix host");
        return;
    }
    if std::env::args().any(|arg| arg == "--stdio") {
        serve_with(
            std::io::stdin().lock(),
            std::io::stdout().lock(),
            |request| {
                if std::env::var_os("BED_TEST_DELAY_WRITE").is_some()
                    && matches!(request, Request::WriteFile { .. })
                {
                    thread::sleep(Duration::from_millis(150));
                }
                if std::env::var_os("BED_TEST_DELAY_READ").is_some()
                    && matches!(request, Request::ReadFile { .. })
                {
                    thread::sleep(Duration::from_millis(150));
                }
                LocalBackend.call(request)
            },
        )
        .unwrap();
        return;
    }
    let fixture = Fixture::new();
    let root = &fixture.0;
    let path = root.join("unicode.txt");
    fs::write(&path, "\u{feff}é🙂\r\ntail\r\n").unwrap();
    let transport = client(true);
    let mut session = session(root, transport.clone(), true);
    let doc = open(&mut session, &path);
    let view = session.create_view(doc).unwrap();
    session
        .with_commands(view, |c| {
            c.set_cursor(0, 0, false, CursorReveal::Ensure);
            c.type_text(b"first");
        })
        .unwrap();
    assert!(!session.save(doc).unwrap());
    assert!(session.save_pending(doc));
    assert!(!session.close_document(doc, ClosePolicy::Save).unwrap());
    session
        .with_commands(view, |c| c.type_text(b"second"))
        .unwrap();
    wait(&mut session, |s| !s.save_pending(doc));
    assert!(
        session.snapshot(doc).unwrap().dirty,
        "older save must not clear newer edits"
    );
    assert_eq!(
        fs::read(&path).unwrap(),
        "\u{feff}firsté🙂\r\ntail\r\n".as_bytes()
    );
    session.save(doc).unwrap();
    wait(&mut session, |s| !s.save_pending(doc));
    assert!(!session.snapshot(doc).unwrap().dirty);
    assert_eq!(
        fs::read(&path).unwrap(),
        "\u{feff}firstsecondé🙂\r\ntail\r\n".as_bytes()
    );

    // A target-side change refuses overwrite, preserving both external bytes and undo.
    fs::write(&path, b"external\n").unwrap();
    session
        .with_commands(view, |c| c.type_text(b"third"))
        .unwrap();
    session.save(doc).unwrap();
    wait(&mut session, |s| !s.save_pending(doc));
    assert!(session.snapshot(doc).unwrap().disk_conflict.is_some());
    assert_eq!(fs::read(&path).unwrap(), b"external\n");
    session.keep_buffer(doc).unwrap();
    wait(&mut session, |s| {
        s.snapshot(doc).unwrap().disk_conflict.is_none()
    });
    session.save(doc).unwrap();
    wait(&mut session, |s| !s.save_pending(doc));
    assert!(!session.snapshot(doc).unwrap().dirty);

    // A clean remote buffer reloads external bytes; local views survive.
    fs::write(&path, b"reloaded\r\n").unwrap();
    wait(&mut session, |s| {
        s.snapshot(doc).unwrap().bytes == b"reloaded\r\n"
    });
    assert!(session.view_snapshot(view).is_ok());

    // Save As commits binding after ack, while newer edits retain dirty state.
    let destination = root.join("new name.txt");
    assert!(!session.save_as(doc, &destination).unwrap());
    assert!(
        !session.close_document(doc, ClosePolicy::Save).unwrap(),
        "clean Save As must remain open until acknowledgement"
    );
    assert_eq!(
        session.shutdown(ClosePolicy::Save).unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    assert_eq!(session.snapshot(doc).unwrap().path, path.to_str().unwrap());
    session
        .with_commands(view, |c| c.type_text(b"newer"))
        .unwrap();
    wait(&mut session, |s| !s.save_pending(doc));
    assert_eq!(
        session.snapshot(doc).unwrap().path,
        destination.to_str().unwrap()
    );
    assert!(session.snapshot(doc).unwrap().dirty);
    assert_eq!(fs::read(&destination).unwrap(), b"reloaded\r\n");

    // Disconnect/reconnect preserves local bytes and rejects changed disk baseline.
    let before = session.snapshot(doc).unwrap().bytes;
    transport.disconnect();
    assert!(!session.remote_connected());
    fs::write(&destination, b"while disconnected").unwrap();
    let upgraded_target = SshTarget {
        host: "fixture".into(),
        agent: "/cache/bed/helpers/new-version/bed-headless".into(),
    };
    session
        .reconnect_remote_with_target(client(false), upgraded_target.clone())
        .unwrap();
    assert_eq!(session.ssh_target(), Some(&upgraded_target));
    wait(&mut session, |s| {
        s.snapshot(doc).unwrap().disk_conflict.is_some()
    });
    assert_eq!(session.snapshot(doc).unwrap().bytes, before);
    session.with_commands(view, |c| c.undo()).unwrap();
    assert_ne!(
        session.snapshot(doc).unwrap().bytes,
        before,
        "undo remains usable after reconnect"
    );
    let wrong_host = SshTarget::new("different-host");
    assert!(
        session
            .reconnect_remote_with_target(client(false), wrong_host)
            .is_err()
    );
    assert!(session.remote_connected());
    assert_eq!(session.ssh_target(), Some(&upgraded_target));
    session.shutdown(ClosePolicy::Discard).unwrap();

    // Canonical aliases resolve to the same document, including repeat opens.
    let mut aliases = self::session(root, client(false), false);
    let canonical = open(&mut aliases, &path);
    fs::create_dir(root.join("sub")).unwrap();
    let alias = root.join("sub/../unicode.txt");
    assert_eq!(open(&mut aliases, &alias), canonical);
    assert_eq!(aliases.document_ids(), vec![canonical]);
    aliases.shutdown(ClosePolicy::Discard).unwrap();

    // A failed read of a superseded path cannot mark the rebound document removed.
    let old = root.join("stale-read-old.txt");
    let rebound = root.join("stale-read-new.txt");
    fs::write(&old, b"original").unwrap();
    let mut stale_read = self::session(root, configured_client(false, true), false);
    let doc = open(&mut stale_read, &old);
    stale_read.reload_from_disk(doc).unwrap();
    fs::rename(&old, &rebound).unwrap();
    stale_read.rebind_path(doc, &rebound).unwrap();
    pump_for(&mut stale_read, Duration::from_millis(250));
    assert_eq!(
        stale_read.snapshot(doc).unwrap().path,
        rebound.to_str().unwrap()
    );
    assert!(
        stale_read.snapshot(doc).unwrap().disk_conflict.is_none(),
        "stale read error applied to the rebound document"
    );
    stale_read.replace_content(doc, b"rebound buffer").unwrap();
    stale_read.save(doc).unwrap();
    wait(&mut stale_read, |s| !s.save_pending(doc));
    assert_eq!(fs::read(&rebound).unwrap(), b"rebound buffer");
    stale_read.shutdown(ClosePolicy::Discard).unwrap();

    // Acknowledged bytes remain the disk baseline when the buffer was replaced meanwhile.
    let path = root.join("replacement-during-save.txt");
    fs::write(&path, b"initial").unwrap();
    let mut replacement = self::session(root, client(true), false);
    let doc = open(&mut replacement, &path);
    replacement.replace_content(doc, b"first snapshot").unwrap();
    replacement.save(doc).unwrap();
    replacement.replace_content(doc, b"new generation").unwrap();
    wait(&mut replacement, |s| !s.save_pending(doc));
    assert_eq!(fs::read(&path).unwrap(), b"first snapshot");
    assert!(replacement.snapshot(doc).unwrap().dirty);
    assert_eq!(replacement.snapshot(doc).unwrap().bytes, b"new generation");
    replacement.save(doc).unwrap();
    wait(&mut replacement, |s| !s.save_pending(doc));
    assert!(
        replacement.snapshot(doc).unwrap().disk_conflict.is_none(),
        "acknowledged baseline was lost across buffer replacement"
    );
    assert!(!replacement.snapshot(doc).unwrap().dirty);
    assert_eq!(fs::read(&path).unwrap(), b"new generation");
    replacement.shutdown(ClosePolicy::Discard).unwrap();

    // Changing only file metadata refreshes the baseline without conflicting with edits.
    let path = root.join("metadata-only.txt");
    fs::write(&path, b"unchanged disk bytes").unwrap();
    let mut metadata = self::session(root, client(false), true);
    let doc = open(&mut metadata, &path);
    metadata.replace_content(doc, b"local edits").unwrap();
    let file = fs::OpenOptions::new().write(true).open(&path).unwrap();
    let modified = file.metadata().unwrap().modified().unwrap();
    file.set_modified(modified + Duration::from_secs(2))
        .unwrap();
    drop(file);
    pump_for(&mut metadata, Duration::from_millis(650));
    assert!(
        metadata.snapshot(doc).unwrap().disk_conflict.is_none(),
        "metadata-only change created a conflict"
    );
    metadata.save(doc).unwrap();
    wait(&mut metadata, |s| !s.save_pending(doc));
    assert!(!metadata.snapshot(doc).unwrap().dirty);
    assert_eq!(fs::read(&path).unwrap(), b"local edits");
    metadata.shutdown(ClosePolicy::Discard).unwrap();

    // Explicit reload/keep decisions complete before another save may snapshot the buffer.
    let path = root.join("reload-save-order.txt");
    fs::write(&path, b"disk version").unwrap();
    let mut reload = self::session(root, configured_client(false, true), false);
    let doc = open(&mut reload, &path);
    reload.replace_content(doc, b"pre-reload buffer").unwrap();
    reload.reload_from_disk(doc).unwrap();
    assert_eq!(
        reload.save(doc).unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    wait(&mut reload, |s| {
        s.snapshot(doc).unwrap().bytes == b"disk version"
    });
    assert!(!reload.snapshot(doc).unwrap().dirty);
    assert_eq!(fs::read(&path).unwrap(), b"disk version");
    reload.replace_content(doc, b"buffer after keep").unwrap();
    fs::write(&path, b"external version").unwrap();
    reload.keep_buffer(doc).unwrap();
    assert_eq!(
        reload.save(doc).unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    pump_for(&mut reload, Duration::from_millis(250));
    reload.save(doc).unwrap();
    wait(&mut reload, |s| !s.save_pending(doc));
    assert_eq!(fs::read(&path).unwrap(), b"buffer after keep");
    assert!(!reload.snapshot(doc).unwrap().dirty);
    reload.shutdown(ClosePolicy::Discard).unwrap();
    // Auto resolution and exact-byte edits use the same queued remote lifecycle.
    let path = root.join("exact-bytes.bin");
    let raw = b"\xef\xbb\xbf\0\0\xffa\r\nb\rc\n";
    fs::write(&path, raw).unwrap();
    let mut binary = self::session(root, client(true), false);
    assert!(binary.request_open_file_auto(&path).unwrap().is_none());
    wait(&mut binary, |session| !session.open_pending(&path));
    let doc = binary.document_for_path(&path).unwrap();
    assert_eq!(binary.snapshot(doc).unwrap().kind, DocumentKind::Bytes);
    assert_eq!(binary.snapshot(doc).unwrap().bytes, raw);
    binary
        .apply_edits(
            doc,
            binary.document_revision(doc).unwrap(),
            &[ByteEdit {
                range: 4..5,
                bytes: vec![0x80, 0x81],
            }],
        )
        .unwrap();
    let first = binary.snapshot(doc).unwrap().bytes;
    binary.save(doc).unwrap();
    binary
        .apply_edits(
            doc,
            binary.document_revision(doc).unwrap(),
            &[ByteEdit {
                range: 0..0,
                bytes: vec![0x01],
            }],
        )
        .unwrap();
    let second = binary.snapshot(doc).unwrap().bytes;
    wait(&mut binary, |session| !session.save_pending(doc));
    assert_eq!(fs::read(&path).unwrap(), first);
    assert!(binary.snapshot(doc).unwrap().dirty);
    binary.save(doc).unwrap();
    wait(&mut binary, |session| !session.save_pending(doc));
    assert_eq!(fs::read(&path).unwrap(), second);
    binary.undo_document(doc).unwrap();
    assert_eq!(binary.snapshot(doc).unwrap().bytes, first);
    binary.undo_document(doc).unwrap();
    assert_eq!(binary.snapshot(doc).unwrap().bytes, raw);
    binary.redo_document(doc).unwrap();
    assert_eq!(binary.snapshot(doc).unwrap().bytes, first);
    binary.reload_from_disk(doc).unwrap();
    wait(&mut binary, |session| {
        session.snapshot(doc).unwrap().bytes == second
    });
    binary.undo_document(doc).unwrap();
    assert_eq!(binary.snapshot(doc).unwrap().bytes, second);
    binary.shutdown(ClosePolicy::Discard).unwrap();

    println!(
        "remote session: delayed saves/reads, generation races, metadata, conflicts, reload, Save As, reconnect, undo and aliases passed"
    );
}
