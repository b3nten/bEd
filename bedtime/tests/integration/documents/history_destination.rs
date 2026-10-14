use crate::temp_dir;
use bedtime_documents::{ClosePolicy, DocumentSession, SessionOptions};
use std::{fs, io};
use temp_dir::TempDir;

#[test]
fn exact_history_destination_is_independent_of_project_root_and_old_history() {
    let temp = TempDir::new();
    let path = temp.write("project/document.txt", b"original");
    let legacy = temp.write(
        "project/.undo-redo-bedtime.json",
        b"this is not valid history",
    );
    let history_path = temp.root().join("selected-state/history.json");
    let options = SessionOptions {
        history_path: Some(history_path.clone()),
        ..Default::default()
    };
    let mut session = DocumentSession::with_options(options.clone()).unwrap();
    let document = session.open_file(&path).unwrap();
    let view = session.create_view(document).unwrap();
    session
        .with_commands(view, |commands| commands.type_text(b"changed "))
        .unwrap();
    session.save(document).unwrap();
    session.shutdown(ClosePolicy::Discard).unwrap();
    assert!(history_path.is_file());
    assert_eq!(fs::read(legacy).unwrap(), b"this is not valid history");
    assert!(!temp.path("project/history.json").exists());
    let mut restored = DocumentSession::with_options(options).unwrap();
    let document = restored.open_file(&path).unwrap();
    restored.undo_document(document).unwrap();
    assert_eq!(restored.snapshot(document).unwrap().bytes, b"original");
}

#[test]
fn malformed_configured_history_is_reported_without_deleting_it() {
    let temp = TempDir::new();
    let path = temp.write("state/history.json", b"{invalid}");
    let result = DocumentSession::with_options(SessionOptions {
        history_path: Some(path.clone()),
        ..Default::default()
    });
    assert_eq!(result.err().unwrap().kind(), io::ErrorKind::InvalidData);
    assert_eq!(fs::read(path).unwrap(), b"{invalid}");
}
