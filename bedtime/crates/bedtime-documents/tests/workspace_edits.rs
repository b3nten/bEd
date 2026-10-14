#![cfg(feature = "ide-services")]
use bedtime_documents::{ByteEdit, ClosePolicy, DocumentSession, SessionOptions};
use bedtime_lsp::lsp_uri::LspUri;
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "bed-workspace-edits-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(fs::canonicalize(path).unwrap())
    }

    fn file(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, bytes).unwrap();
        path
    }

    fn session(&self, history: bool) -> DocumentSession {
        DocumentSession::with_options(SessionOptions {
            project_root: Some(self.0.clone()),
            history_path: history.then(|| self.0.join("history.json")),
            ..Default::default()
        })
        .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn edit(path: &Path, end: usize, replacement: &str) -> Value {
    json!({"textDocument":{"uri":LspUri::file_uri_from_path(path.to_str().unwrap()).unwrap().to_string(),"version":null},"edits":[{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":end}},"newText":replacement}]})
}

#[test]
fn multi_file_edits_save_unopened_targets_and_undo_as_one_group() {
    let fixture = Fixture::new();
    let a = fixture.file("a.rs", b"old");
    let b = fixture.file("b.rs", b"old");
    let mut session = fixture.session(false);
    let first = session.open_file(&a).unwrap();
    session
        .apply_lsp_workspace_edit(
            &json!({"documentChanges":[edit(&a, 3, "new"),edit(&b, 3, "new")]}),
            None,
        )
        .unwrap();
    let second = session.document_for_path(&b).unwrap();
    assert_eq!(session.snapshot(first).unwrap().bytes, b"new");
    assert!(session.snapshot(first).unwrap().dirty);
    assert_eq!(fs::read(&a).unwrap(), b"old");
    assert_eq!(fs::read(&b).unwrap(), b"new");
    assert!(!session.snapshot(second).unwrap().dirty);
    session.undo_document(second).unwrap();
    assert_eq!(session.snapshot(first).unwrap().bytes, b"old");
    assert_eq!(fs::read(&b).unwrap(), b"old");
    session.redo_document(first).unwrap();
    assert_eq!(session.snapshot(second).unwrap().bytes, b"new");
    assert_eq!(fs::read(&b).unwrap(), b"new");
}

#[test]
fn a_newer_participant_edit_blocks_grouped_undo_without_consuming_history() {
    let fixture = Fixture::new();
    let a = fixture.file("a", b"old");
    let b = fixture.file("b", b"old");
    let mut session = fixture.session(false);
    let first = session.open_file(&a).unwrap();
    let second = session.open_file(&b).unwrap();
    session
        .apply_lsp_workspace_edit(
            &json!({"documentChanges":[edit(&a,3,"new"),edit(&b,3,"new")]}),
            None,
        )
        .unwrap();
    session
        .apply_edits(
            second,
            session.document_revision(second).unwrap(),
            &[ByteEdit {
                range: 3..3,
                bytes: b"!".to_vec(),
            }],
        )
        .unwrap();
    let error = session.undo_document(first).unwrap_err();
    assert!(error.to_string().contains(b.to_str().unwrap()));
    assert_eq!(session.snapshot(first).unwrap().bytes, b"new");
    session.undo_document(second).unwrap();
    session.undo_document(first).unwrap();
    assert_eq!(session.snapshot(second).unwrap().bytes, b"old");
    session.redo_document(first).unwrap();
    assert_eq!(session.snapshot(second).unwrap().bytes, b"new");
}

#[test]
fn all_ranges_and_versions_are_validated_before_any_document_mutation() {
    let fixture = Fixture::new();
    let a = fixture.file("a", b"old");
    let b = fixture.file("b", "🙂".as_bytes());
    let mut session = fixture.session(false);
    let first = session.open_file(&a).unwrap();
    let payload = json!({"documentChanges":[edit(&a,3,"new"),edit(&b,1,"split-surrogate")]});
    assert!(session.apply_lsp_workspace_edit(&payload, None).is_err());
    assert_eq!(session.snapshot(first).unwrap().bytes, b"old");
    assert_eq!(fs::read(&b).unwrap(), "🙂".as_bytes());
    let mut wrong_version = edit(&a, 3, "new");
    wrong_version["textDocument"]["version"] = json!(999);
    assert!(
        session
            .apply_lsp_workspace_edit(&json!({"documentChanges":[wrong_version]}), None)
            .is_err()
    );
    session.undo_document(first).unwrap();
    assert_eq!(session.snapshot(first).unwrap().bytes, b"old");
}

#[test]
fn grouped_history_survives_close_and_restart_and_checks_disk_baselines() {
    let fixture = Fixture::new();
    let a = fixture.file("a", b"old");
    let b = fixture.file("b", b"old");
    let mut session = fixture.session(true);
    let first = session.open_file(&a).unwrap();
    session
        .apply_lsp_workspace_edit(
            &json!({"documentChanges":[edit(&a,3,"new"),edit(&b,3,"new")]}),
            None,
        )
        .unwrap();
    session.save(first).unwrap();
    session.shutdown(ClosePolicy::Discard).unwrap();
    drop(session);
    let mut session = fixture.session(true);
    let first = session.open_file(&a).unwrap();
    session.undo_document(first).unwrap();
    assert_eq!(fs::read(&b).unwrap(), b"old");
    session.redo_document(first).unwrap();
    assert_eq!(fs::read(&b).unwrap(), b"new");
    fs::write(&b, b"outside").unwrap();
    assert!(session.undo_document(first).is_err());
    assert_eq!(session.snapshot(first).unwrap().bytes, b"new");
}

#[test]
fn closing_a_dirty_participant_preserves_the_canonical_buffer_for_group_undo() {
    let fixture = Fixture::new();
    let a = fixture.file("a", b"old");
    let b = fixture.file("b", b"old");
    let mut session = fixture.session(false);
    let first = session.open_file(&a).unwrap();
    let second = session.open_file(&b).unwrap();
    session.create_view(second).unwrap();
    session
        .apply_lsp_workspace_edit(
            &json!({"documentChanges":[edit(&a,3,"new"),edit(&b,3,"new")]}),
            None,
        )
        .unwrap();
    session
        .close_document(second, ClosePolicy::Discard)
        .unwrap();
    assert_eq!(session.view_count(second), 0);
    session.undo_document(first).unwrap();
    assert_eq!(session.snapshot(second).unwrap().bytes, b"old");
    assert_eq!(session.open_file(&b).unwrap(), second);
}
