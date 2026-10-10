//! Shared-project cases from upstream Workbench and undo_service_test.cpp.
//! See LICENSE and NOTICE for the original editor's attribution.

use bed_document_session::editor::{Editor, SharedProjectUndo};
use bed_document_session::{EditorSession, SessionOptions};
use bed_editing::project_undo::ProjectUndo;
use std::{
    cell::RefCell,
    fs,
    path::{Path, PathBuf},
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
struct TempDir(PathBuf);
impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "bed-shared-undo-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(fs::canonicalize(path).unwrap())
    }
    fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, bytes).unwrap();
        path
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn tab(shared: &SharedProjectUndo, path: &str, bytes: &[u8]) -> Editor {
    let mut editor = Editor::new();
    editor.bind_project_undo(Rc::clone(shared));
    editor.api().open_document(path, bytes);
    editor.api().restore_caret(0, bytes.len() as i32);
    editor
}

#[test]
fn two_editors_share_one_store_with_independent_files_and_caret_snapshots() {
    let shared = Rc::new(RefCell::new(ProjectUndo::default()));
    let mut first = tab(&shared, "file-a", b"A");
    let mut second = tab(&shared, "file-b", b"B");
    first.commands().type_text(b"1");
    first.commands().type_text(b"2");
    second.commands().type_text(b"3");
    second.commands().undo();
    assert_eq!(second.state.join(), b"B");
    assert_eq!(first.state.join(), b"A12");
    first.commands().undo();
    assert_eq!(first.state.join(), b"A");
    assert_eq!((first.view.row, first.view.column), (0, 1));
    first.commands().redo();
    second.commands().redo();
    assert_eq!(first.state.join(), b"A12");
    assert_eq!(second.state.join(), b"B3");
    assert_eq!(first.view.column, 3);
    assert_eq!(second.view.column, 2);
    // A bound tab's dormant standalone store never records a second copy.
    assert!(first.undo.undo("file-a").is_none());
    assert!(second.undo.undo("file-b").is_none());
}

#[test]
fn closing_and_reopening_a_tab_keeps_the_project_history() {
    let shared = Rc::new(RefCell::new(ProjectUndo::default()));
    {
        let mut first = tab(&shared, "file-a", b"A");
        first.commands().type_text(b"1");
    }
    let mut second = tab(&shared, "file-b", b"B");
    second.commands().type_text(b"2");
    let mut reopened = tab(&shared, "file-a", b"A1");
    reopened.commands().undo();
    assert_eq!(reopened.state.join(), b"A");
    assert_eq!(second.state.join(), b"B2");
    second.commands().undo();
    assert_eq!(second.state.join(), b"B");
}

#[test]
fn one_disk_store_keeps_both_tabs_and_project_open_loads_it_once() {
    let temp = TempDir::new();
    let first_path = temp.write("a.txt", b"A");
    let second_path = temp.write("b.txt", b"B");
    let shared = Rc::new(RefCell::new(ProjectUndo::new(
        temp.0.to_str().unwrap().to_owned(),
    )));
    let mut first = tab(&shared, first_path.to_str().unwrap(), b"A");
    let mut second = tab(&shared, second_path.to_str().unwrap(), b"B");
    first.commands().type_text(b"1");
    second.commands().type_text(b"2");
    first.save().unwrap();
    second.save().unwrap();
    first.with_project_undo(ProjectUndo::flush);
    let serialized: serde_json::Value =
        serde_json::from_slice(&fs::read(temp.0.join(".undo-redo-bed.json")).unwrap()).unwrap();
    assert_eq!(serialized["files"].as_object().unwrap().len(), 2);
    assert!(
        serialized["files"]
            .get(first_path.to_str().unwrap())
            .is_some()
    );
    assert!(
        serialized["files"]
            .get(second_path.to_str().unwrap())
            .is_some()
    );

    let options = SessionOptions {
        project_root: Some(temp.0.clone()),
        persistent_history: true,
        ..Default::default()
    };
    let mut session = EditorSession::with_options(options.clone()).unwrap();
    let first = session.open_file(&first_path).unwrap();
    let view = session.create_view(first).unwrap();
    session
        .with_commands(view, |commands| commands.undo())
        .unwrap();
    assert_eq!(session.snapshot(first).unwrap().bytes, b"A");
    session.configure(options).unwrap();
    session
        .with_commands(view, |commands| commands.undo())
        .unwrap();
    assert_eq!(session.snapshot(first).unwrap().bytes, b"A");
    let second = session.open_file(&second_path).unwrap();
    let view = session.create_view(second).unwrap();
    session
        .with_commands(view, |commands| commands.undo())
        .unwrap();
    assert_eq!(session.snapshot(second).unwrap().bytes, b"B");
}

#[test]
fn external_reload_invalidates_only_the_replaced_file_in_the_shared_store() {
    let temp = TempDir::new();
    let first_path = temp.write("a.txt", b"A");
    let second_path = temp.write("b.txt", b"B");
    let mut session = EditorSession::new();
    let first = session.open_file(&first_path).unwrap();
    let second = session.open_file(&second_path).unwrap();
    let a = session.create_view(first).unwrap();
    let b = session.create_view(second).unwrap();
    for view in [a, b] {
        session
            .with_commands(view, |commands| {
                commands.move_doc_end(false);
                commands.type_text(b"1");
            })
            .unwrap();
    }
    fs::write(first_path, b"external").unwrap();
    session.reload_from_disk(first).unwrap();
    session
        .with_commands(a, |commands| commands.undo())
        .unwrap();
    assert_eq!(session.snapshot(first).unwrap().bytes, b"external");
    session
        .with_commands(b, |commands| commands.undo())
        .unwrap();
    assert_eq!(session.snapshot(second).unwrap().bytes, b"B");
}

#[test]
fn unbound_editor_retains_the_public_standalone_history() {
    let mut editor = Editor::new();
    editor.api().open_document("standalone", b"");
    editor.commands().type_text(b"text");
    editor.commands().undo();
    assert!(editor.state.join().is_empty());
    assert!(editor.undo.redo("standalone").is_some());
    assert!(editor.project_undo().undo("standalone").is_some());
    assert_eq!(editor.state.path, Path::new("standalone").to_str().unwrap());
}
