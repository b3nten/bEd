//! End-to-end disk monitoring checks through the shared session boundary.
use bed_core::editor_commands::{CursorReveal, EditorCommands};
use bed_session::{
    DocumentId, DocumentSnapshot, EditorSession, SessionEvent, SessionOptions, TickReport, ViewId,
};
use std::{
    fs,
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};
#[allow(dead_code)]
#[path = "../../../tests/support/temp_dir.rs"]
mod temp_dir;
struct Fixture {
    session: EditorSession,
    doc: DocumentId,
    view: ViewId,
    path: PathBuf,
    temp: temp_dir::TempDir,
    polled: Option<Instant>,
}
impl Fixture {
    fn new(raw: &[u8]) -> Self {
        let temp = temp_dir::TempDir::new();
        let path = temp.write("document.txt", raw);
        let mut session = EditorSession::with_options(SessionOptions {
            project_root: Some(temp.root().to_owned()),
            monitoring: true,
            persistent_history: true,
            ..Default::default()
        })
        .unwrap();
        let doc = session.open_file(&path).unwrap();
        let view = session.create_view(doc).unwrap();
        session.take_events();
        Self {
            session,
            doc,
            view,
            path,
            temp,
            polled: None,
        }
    }
    fn snapshot(&self) -> DocumentSnapshot {
        self.session.snapshot(self.doc).unwrap()
    }
    fn edit<R>(&mut self, f: impl FnOnce(&mut EditorCommands<'_>) -> R) -> R {
        self.session.with_commands(self.view, f).unwrap()
    }
    fn external(&self, raw: &[u8]) {
        fs::write(&self.path, raw).unwrap();
    }
    fn poll(&mut self) -> TickReport {
        if let Some(last) = self.polled {
            thread::sleep(Duration::from_millis(510).saturating_sub(last.elapsed()));
        }
        self.polled = Some(Instant::now());
        self.session.tick()
    }
    fn reloaded(&mut self) -> bool {
        let report = self.poll();
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        report.events.iter().any(
            |event| matches!(event,SessionEvent::Reloaded { document,.. } if *document==self.doc),
        )
    }
    fn save(&mut self) -> std::io::Result<bool> {
        self.session.save(self.doc)
    }
    fn autosave(&mut self) {
        let mut options = self.session.options().clone();
        options.autosave = Some(Duration::ZERO);
        self.session.configure(options).unwrap();
    }
}
#[test]
fn clean_external_edit_reloads_once_and_emits_session_event() {
    let mut fx = Fixture::new(b"before");
    let generation = fx.snapshot().generation;
    fx.external(b"after!");
    assert!(fx.reloaded());
    let doc = fx.snapshot();
    assert_eq!(doc.bytes, b"after!");
    assert_eq!(doc.generation, generation + 1);
    assert!(!doc.dirty);
    assert!(doc.disk_conflict.is_none());
    assert!(!fx.reloaded());
}
#[test]
fn clean_reload_clamps_caret_and_preserves_scroll() {
    let mut fx = Fixture::new(b"first\nsecond\nlong final line");
    fx.edit(|c| c.set_cursor(2, 12, false, CursorReveal::Ensure));
    fx.session.set_scroll(fx.view, 12.0, 48.0).unwrap();
    let scroll = fx.session.view_snapshot(fx.view).unwrap();
    fx.external(b"x");
    assert!(fx.reloaded());
    let view = fx.session.view_snapshot(fx.view).unwrap();
    assert_eq!((view.row, view.column), (0, 1));
    assert_eq!(view.scroll_position, scroll.scroll_position);
    assert_eq!(view.requested_scroll, scroll.requested_scroll);
    fx.external("éxy".as_bytes());
    assert!(fx.reloaded());
    let view = fx.session.view_snapshot(fx.view).unwrap();
    assert_eq!((view.row, view.column), (0, 0));
    assert_eq!(view.selection_count(), 1);
}
#[test]
fn external_reload_preserves_bom_and_crlf_and_save_round_trips() {
    let mut fx = Fixture::new(b"original\n");
    let external = b"\xef\xbb\xbfhello\r\nworld\r\n";
    fx.external(external);
    assert!(fx.reloaded());
    let doc = fx.snapshot();
    assert!(doc.utf8_bom);
    assert_eq!(doc.line_ending, b"\r\n");
    assert_eq!(doc.bytes, &external[3..]);
    fx.edit(|c| c.type_text(b"!"));
    assert!(fx.save().unwrap());
    assert_eq!(
        fs::read(&fx.path).unwrap(),
        b"\xef\xbb\xbf!hello\r\nworld\r\n"
    );
    assert!(!fx.reloaded());
}
#[test]
fn dirty_external_edit_retains_buffer_and_blocks_explicit_and_idle_saves() {
    let mut fx = Fixture::new(b"original");
    fx.autosave();
    fx.edit(|c| c.type_text(b"local "));
    let local = fx.snapshot().bytes;
    let before = fx.session.view_snapshot(fx.view).unwrap();
    fx.external(b"remote");
    assert!(!fx.reloaded());
    assert_eq!(fx.snapshot().bytes, local);
    assert_eq!(
        fx.session.view_snapshot(fx.view).unwrap().selections,
        before.selections
    );
    assert!(fx.snapshot().dirty);
    assert!(fx.snapshot().disk_conflict.is_some());
    assert!(fx.save().is_err());
    fx.edit(|c| c.type_text(b"still local "));
    fx.poll();
    assert_eq!(fs::read(&fx.path).unwrap(), b"remote");
}
#[test]
fn keep_buffer_accepts_disk_baseline_and_allows_explicit_save() {
    let mut fx = Fixture::new(b"original");
    fx.edit(|c| c.type_text(b"local "));
    let local = fx.snapshot().bytes;
    fx.external(b"remote");
    assert!(!fx.reloaded());
    fx.session.keep_buffer(fx.doc).unwrap();
    assert!(fx.snapshot().disk_conflict.is_none());
    assert!(fx.snapshot().dirty);
    assert!(!fx.reloaded());
    assert!(fx.save().unwrap());
    assert_eq!(fs::read(&fx.path).unwrap(), local);
    assert!(!fx.reloaded());
    assert!(!fx.snapshot().dirty);
}
#[test]
fn removed_file_requires_keep_then_explicit_save_to_recreate() {
    let mut fx = Fixture::new(b"preserved");
    fs::remove_file(&fx.path).unwrap();
    assert!(!fx.reloaded());
    assert!(fx.snapshot().disk_conflict.is_some());
    assert_eq!(fx.snapshot().bytes, b"preserved");
    fx.edit(|c| c.type_text(b"local "));
    assert!(fx.save().is_err());
    assert!(!fx.path.exists());
    fx.session.keep_buffer(fx.doc).unwrap();
    assert!(!fx.path.exists());
    assert!(fx.save().unwrap());
    assert_eq!(fs::read(&fx.path).unwrap(), b"local preserved");
    assert!(!fx.reloaded());
}
#[test]
fn self_save_does_not_reload_or_reset_caret_or_undo() {
    let mut fx = Fixture::new(b"\xef\xbb\xbfhello\r\nworld");
    fx.edit(|c| {
        c.set_cursor(1, 5, false, CursorReveal::Ensure);
        c.type_text(b"!");
    });
    let before = fx.snapshot();
    assert!(fx.save().unwrap());
    assert!(!fx.reloaded());
    assert_eq!(fx.snapshot().generation, before.generation);
    assert_eq!(fx.snapshot().version, before.version);
    let view = fx.session.view_snapshot(fx.view).unwrap();
    assert_eq!((view.row, view.column), (1, 6));
    fx.edit(|c| c.undo());
    assert_eq!(fx.snapshot().bytes, b"hello\r\nworld");
}
#[test]
fn confirmed_reload_discards_conflicting_edits_and_cancels_autosave() {
    let mut fx = Fixture::new(b"original");
    fx.edit(|c| c.type_text(b"local "));
    fx.external(b"remote");
    assert!(!fx.reloaded());
    fx.session.reload_from_disk(fx.doc).unwrap();
    assert_eq!(fx.snapshot().bytes, b"remote");
    assert!(!fx.snapshot().dirty);
    assert!(fx.snapshot().disk_conflict.is_none());
    fx.autosave();
    fx.poll();
    assert_eq!(fs::read(&fx.path).unwrap(), b"remote");
}
fn invalid_external(raw: &[u8]) {
    let mut fx = Fixture::new(b"preserved");
    let generation = fx.snapshot().generation;
    fx.external(raw);
    let report = fx.poll();
    assert!(report.errors.iter().any(|error| error.service == "monitor"));
    assert_eq!(fx.snapshot().bytes, b"preserved");
    assert_eq!(fx.snapshot().generation, generation);
    assert!(!fx.snapshot().dirty);
    assert!(fx.snapshot().disk_conflict.is_some());
    fx.autosave();
    fx.edit(|c| c.type_text(b"local "));
    fx.poll();
    assert!(fx.save().is_err());
    assert!(fx.snapshot().dirty);
    assert_eq!(fs::read(&fx.path).unwrap(), raw);
}
#[test]
fn binary_external_reload_failure_preserves_buffer_and_blocks_future_saves() {
    invalid_external(&[0; 100]);
}
#[test]
fn oversized_external_change_preserves_buffer_and_blocks_future_saves() {
    invalid_external(&vec![b'a'; bed_files::files::MAX_FILE_SIZE + 1]);
}
#[test]
fn unchanged_oversized_monitor_errors_are_reported_once_and_recovery_is_retried() {
    let mut fx = Fixture::new(b"preserved");
    let generation = fx.snapshot().generation;
    let grow = |path: &std::path::Path, extra: u64| {
        fs::OpenOptions::new()
            .write(true)
            .open(path)
            .unwrap()
            .set_len(bed_files::files::MAX_FILE_SIZE as u64 + extra)
            .unwrap();
    };
    grow(&fx.path, 1);
    let first = fx.poll();
    assert_eq!(
        first
            .errors
            .iter()
            .filter(|error| error.service == "monitor")
            .count(),
        1
    );
    let conflict = fx.snapshot().disk_conflict;
    assert!(conflict.is_some());
    let repeated = fx.poll();
    assert!(
        repeated.errors.is_empty(),
        "unchanged failure must stay dismissed"
    );
    assert_eq!(fx.snapshot().disk_conflict, conflict);
    assert_eq!(fx.snapshot().bytes, b"preserved");
    assert_eq!(fx.snapshot().generation, generation);

    grow(&fx.path, 2);
    let changed = fx.poll();
    assert_eq!(
        changed
            .errors
            .iter()
            .filter(|error| error.service == "monitor")
            .count(),
        1
    );
    assert_ne!(fx.snapshot().disk_conflict, conflict);

    fx.external(b"recovered");
    assert!(
        fx.reloaded(),
        "monitoring must continue retrying after an error"
    );
    assert_eq!(fx.snapshot().bytes, b"recovered");
    assert!(fx.snapshot().disk_conflict.is_none());

    grow(&fx.path, 1);
    assert_eq!(
        fx.poll()
            .errors
            .iter()
            .filter(|error| error.service == "monitor")
            .count(),
        1
    );
}
#[test]
fn external_reload_resets_old_undo_and_persists_empty_history() {
    let mut fx = Fixture::new(b"original");
    fx.edit(|c| c.type_text(b"local "));
    fx.save().unwrap();
    fx.poll();
    let root = fx.temp.root().to_str().unwrap().to_owned();
    let mut prior = bed_core::project_undo::ProjectUndo::new(root.clone());
    prior.load_project(&root);
    assert!(prior.undo(&fx.snapshot().path).is_some());
    fx.external(b"external");
    assert!(fx.reloaded());
    fx.edit(|c| {
        c.undo();
        c.redo();
    });
    assert_eq!(fx.snapshot().bytes, b"external");
    fx.session
        .shutdown(bed_session::ClosePolicy::Discard)
        .unwrap();
    let mut restored = EditorSession::with_options(SessionOptions {
        project_root: Some(fx.temp.root().to_owned()),
        persistent_history: true,
        ..Default::default()
    })
    .unwrap();
    let doc = restored.open_file(&fx.path).unwrap();
    let view = restored.create_view(doc).unwrap();
    restored
        .with_commands(view, |c| {
            c.undo();
            c.redo();
        })
        .unwrap();
    assert_eq!(restored.snapshot(doc).unwrap().bytes, b"external");
    assert!(!restored.snapshot(doc).unwrap().dirty);
    restored
        .with_commands(view, |c| {
            c.set_cursor(0, 0, false, CursorReveal::Ensure);
            c.type_text(b"new ");
        })
        .unwrap();
    assert_eq!(restored.snapshot(doc).unwrap().bytes, b"new external");
    restored.with_commands(view, |c| c.undo()).unwrap();
    assert_eq!(restored.snapshot(doc).unwrap().bytes, b"external");
}
