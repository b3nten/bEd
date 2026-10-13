//! Remote document access works without language servers, highlighting or Git.
use bed_documents::{ClosePolicy, DocumentSession, SessionOptions};
use bed_remote::{RemoteClient, SshTarget};
use std::{
    io,
    path::Path,
    time::{Duration, Instant},
};
#[path = "../../../tests/support/temp_dir.rs"]
mod test_support;

fn main() {
    if std::env::args().any(|argument| argument == "--stdio") {
        let mut mixed_document_opened = false;
        bed_remote::serve_with(io::stdin().lock(), io::stdout().lock(), |request| {
            if matches!(&request,bed_remote::Request::ReadFile{path,..} if path.ends_with("mixed.txt")) {
                if mixed_document_opened {
                    return Err(bed_remote::RemoteError::new(bed_remote::ErrorKind::PermissionDenied, "Fixture monitor read failed"));
                }
                mixed_document_opened = true;
            }
            if matches!(&request,bed_remote::Request::WriteFile{path,..} if path.ends_with("save-denied.txt")) {
                Err(bed_remote::RemoteError::new(bed_remote::ErrorKind::PermissionDenied, "Fixture save was denied"))
            } else if matches!(&request,bed_remote::Request::WriteFile{path,..} if path.ends_with("save-disconnected.txt")) {
                std::process::exit(0);
            } else if matches!(&request,bed_remote::Request::WriteFile{path,..} if path.ends_with("save-confirmed-disconnect.txt")) {
                let result = bed_remote::LocalBackend.call(request);
                std::thread::spawn(|| {
                    std::thread::sleep(Duration::from_millis(25));
                    std::process::exit(0);
                });
                result
            } else if matches!(&request,bed_remote::Request::ReadFile{path,..} if path.ends_with("wrong-response"))
                || matches!(&request,bed_remote::Request::WriteFile{path,..} if path.ends_with("save-wrong-response.txt")) {
                Ok(bed_remote::Response::Unit)
            } else {bed_remote::LocalBackend.call(request)}
        }).unwrap();
        return;
    }
    let directory = test_support::TempDir::new();
    let path = directory.write("document.txt", b"initial");
    let root = std::fs::canonicalize(directory.root()).unwrap();
    let client = RemoteClient::launch_local(std::env::current_exe().unwrap()).unwrap();
    let context = bed_files::FileContext::remote("fixture", root.clone()).unwrap();
    let services =
        bed_files::FileServices::remote_target(SshTarget::new("fixture"), client.clone());
    let history_path = directory.path("must-stay-in-memory/history.json");
    let mut session = DocumentSession::with_file_services(
        SessionOptions {
            history_path: Some(history_path.clone()),
            ..Default::default()
        },
        &context,
        &services,
    )
    .unwrap();
    assert!(session.options().history_path.is_none());
    assert_eq!(session.request_open_file_auto(&path).unwrap(), None);
    let deadline = Instant::now() + Duration::from_secs(5);
    let document = loop {
        session.tick();
        if let Some(document) = session.document_for_path(&path) {
            break document;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    };
    assert!(session.is_remote());
    assert!(session.file_provider_matches(&services));
    assert!(!session.file_provider_matches(&bed_files::FileServices::local()));
    let view = session.create_view(document).unwrap();
    session
        .with_commands(view, |commands| commands.type_text(b"changed "))
        .unwrap();
    assert!(!session.save(document).unwrap());
    while session.save_pending(document) {
        session.tick();
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(std::fs::read(&path).unwrap(), b"changed initial");
    session.undo_document(document).unwrap();
    assert_eq!(session.snapshot(document).unwrap().bytes, b"initial");
    for name in ["missing", "wrong-response"] {
        let failed = root.join(name);
        assert_eq!(session.request_open_file_auto(&failed).unwrap(), None);
        let deadline = Instant::now() + Duration::from_secs(5);
        while session.open_pending(&failed) {
            session.tick();
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(session.take_open_file_error(&path).is_none());
        let error = session
            .take_open_file_error(&failed)
            .expect("failed open retains its path result");
        assert_eq!(
            error.kind(),
            if name == "missing" {
                io::ErrorKind::NotFound
            } else {
                io::ErrorKind::InvalidData
            }
        );
        assert!(session.take_open_file_error(&failed).is_none());
    }
    let retried = root.join("missing");
    std::fs::write(&retried, b"retry").unwrap();
    assert_eq!(session.request_open_file_auto(&retried).unwrap(), None);
    let deadline = Instant::now() + Duration::from_secs(5);
    while session.open_pending(&retried) {
        session.tick();
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(session.take_open_file_error(&retried).is_none());
    assert_eq!(
        session
            .snapshot(session.document_for_path(&retried).unwrap())
            .unwrap()
            .bytes,
        b"retry"
    );

    std::fs::create_dir(root.join("nested")).unwrap();
    for filename in ["copy.txt", "copy.txt"] {
        let view = session.view_ids(document)[0];
        session
            .with_commands(view, |commands| commands.type_text(b"alias "))
            .unwrap();
        let before = session.document_revision(document).unwrap();
        let original = session.snapshot(document).unwrap().path;
        session
            .save_as(document, &root.join("nested/..").join(filename))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut events = Vec::new();
        while session.save_pending(document) {
            let report = session.tick();
            assert!(report.errors.is_empty(), "{:?}", report.errors);
            events.extend(report.events);
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        let canonical = root.join(filename).to_string_lossy().into_owned();
        assert_eq!(session.snapshot(document).unwrap().path, canonical);
        let rebound = canonical != original;
        assert_eq!(events.iter().any(|event| matches!(event,bed_documents::SessionEvent::PathChanged{document:id,..} if *id == document)), rebound);
        assert!(events.iter().any(|event| matches!(event,bed_documents::SessionEvent::Saved{document:id,save} if *id == document && save.path == canonical)));
        assert_eq!(
            session.document_revision(document).unwrap(),
            if rebound {
                (before.0 + 1, before.1 + 1)
            } else {
                before
            }
        );
        assert!(!session.snapshot(document).unwrap().dirty);
    }

    let mixed_path = directory.write("mixed.txt", b"before");
    session.request_open_file_auto(&mixed_path).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while session.open_pending(&mixed_path) {
        session.tick();
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    let mixed = session.document_for_path(&mixed_path).unwrap();
    let view = session.create_view(mixed).unwrap();
    session
        .with_commands(view, |commands| commands.type_text(b"after "))
        .unwrap();
    session.notify_disk_change(mixed).unwrap();
    session.save(mixed).unwrap();
    let mut errors = Vec::new();
    let mut saved = false;
    while session.save_pending(mixed) {
        let report = session.tick();
        errors.extend(report.errors);
        saved |= report.events.iter().any(|event| matches!(event, bed_documents::SessionEvent::Saved {document,..} if *document == mixed));
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(
        errors
            .iter()
            .any(|error| error.document == Some(mixed) && error.service == "remote")
    );
    assert!(
        !errors
            .iter()
            .any(|error| error.document == Some(mixed) && error.service == "save")
    );
    assert!(saved);
    assert_eq!(std::fs::read(mixed_path).unwrap(), b"after before");
    assert!(!session.snapshot(mixed).unwrap().dirty);

    for name in [
        "save-denied.txt",
        "save-wrong-response.txt",
        "save-disconnected.txt",
    ] {
        let failed_path = directory.write(name, b"original");
        session.request_open_file_auto(&failed_path).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while session.open_pending(&failed_path) {
            session.tick();
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        let failed = session.document_for_path(&failed_path).unwrap();
        let view = session.create_view(failed).unwrap();
        session
            .with_commands(view, |commands| commands.type_text(b"changed "))
            .unwrap();
        session.save(failed).unwrap();
        let mut errors = Vec::new();
        while session.save_pending(failed) {
            errors.extend(session.tick().errors);
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(
            errors
                .iter()
                .any(|error| error.document == Some(failed) && error.service == "save"),
            "{name}: {errors:?}"
        );
        assert!(
            !errors
                .iter()
                .any(|error| error.document == Some(failed) && error.service == "remote")
        );
        assert!(session.snapshot(failed).unwrap().dirty);
        assert_eq!(std::fs::read(failed_path).unwrap(), b"original");
    }
    assert!(
        session
            .close_document(document, ClosePolicy::Discard)
            .unwrap()
    );
    assert!(session.document_for_path(Path::new(&path)).is_none());
    session.shutdown(ClosePolicy::Discard).unwrap();
    assert!(!history_path.exists());

    let confirmed_path = directory.write("save-confirmed-disconnect.txt", b"original");
    let client = RemoteClient::launch_local(std::env::current_exe().unwrap()).unwrap();
    let services =
        bed_files::FileServices::remote_target(SshTarget::new("fixture"), client.clone());
    let mut session =
        DocumentSession::with_file_services(SessionOptions::default(), &context, &services)
            .unwrap();
    session.request_open_file_auto(&confirmed_path).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while session.open_pending(&confirmed_path) {
        session.tick();
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    let confirmed = session.document_for_path(&confirmed_path).unwrap();
    let view = session.create_view(confirmed).unwrap();
    session
        .with_commands(view, |commands| commands.type_text(b"confirmed "))
        .unwrap();
    session.save(confirmed).unwrap();
    // Consume its completion only after the transport has closed: a confirmed
    // write remains a successful save independently of connection availability.
    let probe = client.clone();
    let connection_probe = std::thread::spawn(move || {
        while probe.is_connected() {
            let _ = probe.call(bed_remote::Request::Hello {
                version: bed_remote::PROTOCOL_VERSION,
            });
            std::thread::sleep(Duration::from_millis(2));
        }
    });
    while client.is_connected() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    connection_probe.join().unwrap();
    let mut saved = false;
    while session.save_pending(confirmed) {
        let report = session.tick();
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        saved |= report.events.iter().any(|event| matches!(event,bed_documents::SessionEvent::Saved{document,..} if *document == confirmed));
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(saved);
    assert!(!session.snapshot(confirmed).unwrap().dirty);
    assert_eq!(
        std::fs::read(confirmed_path).unwrap(),
        b"confirmed original"
    );
    session.shutdown(ClosePolicy::Discard).unwrap();
    println!("Remote documents, atomic save and in-memory undo work without IDE services");
}
