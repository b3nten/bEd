//! Ported cases from ned tests/editor/{save_service,undo_service}_test.cpp.

use std::{
    cell::RefCell,
    fs,
    path::{Path, PathBuf},
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
};

use bed_core::editor_view_state::Selection;
use bed_session::editor::Editor;

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);
impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "bed-save-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn document(raw: &[u8], path: &Path) -> Editor {
    let mut editor = Editor::new();
    editor.set_content(raw);
    editor.state.path = path.to_str().unwrap().to_owned();
    editor.state.set_line_ending(b"\n");
    editor.undo.ensure_file(&editor.state.path);
    editor
}

#[test]
fn dirty_path_writes_clears_dirty_and_emits_version() {
    let temp = TempDir::new();
    let file = temp.file("note.txt");
    fs::write(&file, b"old").unwrap();
    let mut editor = document(b"new content", &file);
    editor.state.dirty = true;
    editor.state.version = 3;
    let saved = Rc::new(RefCell::new(Vec::new()));
    let observed = Rc::clone(&saved);
    editor
        .events
        .subscribe_did_save(move |event| observed.borrow_mut().push(event.clone()));
    assert!(editor.save().unwrap());
    assert!(!editor.state.dirty);
    assert_eq!(fs::read(&file).unwrap(), b"new content");
    assert_eq!(saved.borrow()[0].version, 3);
    assert_eq!(saved.borrow()[0].path, file.to_str().unwrap());
}

#[test]
fn bom_and_byte_content_round_trip_and_clean_save_is_noop() {
    let temp = TempDir::new();
    let file = temp.file("bom.txt");
    let bytes = b"\xef\xbb\xbfhello\r\nworld\r\n";
    fs::write(&file, bytes).unwrap();
    let mut editor = Editor::new();
    editor.open(&file).unwrap();
    assert!(editor.state.utf8_bom);
    assert_eq!(editor.state.join(), &bytes[3..]);
    editor.state.dirty = true;
    editor.save().unwrap();
    assert_eq!(fs::read(&file).unwrap(), bytes);
    editor.set_content(b"plain\0\xff");
    editor.state.dirty = true;
    editor.save().unwrap();
    assert_eq!(fs::read(&file).unwrap(), b"plain\0\xff");
    editor.set_content(b"would overwrite");
    assert!(!editor.save().unwrap());
    assert_eq!(fs::read(&file).unwrap(), b"plain\0\xff");
}

#[test]
fn empty_path_and_oversized_save_keep_dirty_and_disk_unchanged() {
    let mut editor = Editor::new();
    editor.set_content(b"x");
    editor.state.dirty = true;
    assert!(!editor.save().unwrap());
    assert!(editor.state.dirty);
    let temp = TempDir::new();
    let file = temp.file("big.txt");
    fs::write(&file, b"ORIGINAL_ON_DISK").unwrap();
    let mut editor = document(&vec![b'a'; bed_files::files::MAX_FILE_SIZE + 1], &file);
    editor.state.dirty = true;
    let error = editor.save().unwrap_err();
    assert!(error.to_string().contains("128 MiB"));
    assert!(bed_session::save_service::EditorSave::bytes_for_save(&editor.state).is_err());
    assert!(editor.state.dirty);
    assert_eq!(fs::read(&file).unwrap(), b"ORIGINAL_ON_DISK");
}

#[test]
fn legitimate_text_resembling_the_old_truncation_notice_can_be_saved() {
    let temp = TempDir::new();
    let file = temp.file("notice.txt");
    let bytes = b"\n\n[File truncated - No Edits - showing first 1MB of 5MB]\nactual source text";
    let mut editor = document(bytes, &file);
    editor.state.dirty = true;
    assert!(editor.save().unwrap());
    assert_eq!(fs::read(&file).unwrap(), bytes);
}

#[test]
fn save_limit_includes_the_utf8_bom_before_touching_the_destination() {
    let temp = TempDir::new();
    let file = temp.file("bom-limit.txt");
    fs::write(&file, b"original").unwrap();
    let mut editor = document(&vec![b'a'; bed_files::files::MAX_FILE_SIZE - 2], &file);
    editor.state.utf8_bom = true;
    editor.state.dirty = true;
    assert!(editor.save().is_err());
    assert!(editor.state.dirty);
    assert_eq!(fs::read(&file).unwrap(), b"original");
}

#[test]
fn oversized_open_preserves_the_existing_document_and_caret() {
    let temp = TempDir::new();
    let file = temp.file("oversized.txt");
    fs::File::create(&file)
        .unwrap()
        .set_len(bed_files::files::MAX_FILE_SIZE as u64 + 1)
        .unwrap();
    let mut editor = Editor::new();
    editor.set_content(b"preserved");
    editor.view.column = 4;
    assert!(editor.open(&file).is_err());
    assert_eq!(editor.state.join(), b"preserved");
    assert_eq!(editor.view.column, 4);
    assert!(editor.state.path.is_empty());
}

#[test]
fn autosave_is_idle_debounced_and_explicit_save_cancels_schedule() {
    let temp = TempDir::new();
    let file = temp.file("note.txt");
    fs::write(&file, b"old").unwrap();
    let mut editor = document(b"new", &file);
    editor.state.dirty = true;
    editor.save_service.on_did_edit(&editor.state);
    assert!(!editor.poll_save().unwrap());
    assert_eq!(fs::read(&file).unwrap(), b"old");
    editor.save_service.set_autosave_idle_ms(0);
    assert!(editor.poll_save().unwrap());
    assert_eq!(fs::read(&file).unwrap(), b"new");
    editor.commands().type_text(b"again");
    editor.save().unwrap();
    assert!(!editor.poll_save().unwrap());
    editor.state.path.clear();
    editor.state.dirty = true;
    editor.save_service.on_did_edit(&editor.state);
    assert!(!editor.poll_save().unwrap());
    assert!(editor.state.dirty);
}

#[test]
fn failed_write_does_not_mark_saved_or_emit_success() {
    let temp = TempDir::new();
    let mut editor = document(b"text", &temp.0);
    editor.state.dirty = true;
    let saves = Rc::new(RefCell::new(0));
    let count = Rc::clone(&saves);
    editor
        .events
        .subscribe_did_save(move |_| *count.borrow_mut() += 1);
    assert!(editor.save().is_err());
    assert!(editor.state.dirty);
    assert_eq!(*saves.borrow(), 0);
}

#[test]
fn real_commands_schedule_autosave_and_group_adjacent_inserts() {
    let temp = TempDir::new();
    let file = temp.file("doc.txt");
    let mut editor = document(b"", &file);
    editor.commands().type_text(b"a");
    editor.commands().type_text(b"b");
    editor.commands().type_text(b"c");
    assert_eq!(editor.state.join(), b"abc");
    editor.commands().undo();
    assert_eq!(editor.state.join(), b"");
    assert_eq!(editor.view.column, 0);
    editor.commands().redo();
    assert_eq!(editor.state.join(), b"abc");
    editor.save_service.set_autosave_idle_ms(0);
    assert!(editor.poll_save().unwrap());
    assert_eq!(fs::read(file).unwrap(), b"abc");
}

#[test]
fn autosave_retains_named_document_undo_and_redo_stacks() {
    for shared_history in [false, true] {
        let temp = TempDir::new();
        let file = temp.file("doc.txt");
        fs::write(&file, b"start").unwrap();
        let mut editor = Editor::new();
        if shared_history {
            editor.bind_project_undo(Rc::new(RefCell::new(
                bed_core::project_undo::ProjectUndo::default(),
            )));
        }
        editor.open(&file).unwrap();
        editor.api().restore_caret(0, 5);
        editor.save_service.set_autosave_idle_ms(0);
        editor.commands().type_text(b"-edit");
        assert!(editor.poll_save().unwrap());
        assert_eq!(fs::read(&file).unwrap(), b"start-edit");
        assert!(!editor.state.dirty);
        editor.commands().undo();
        assert_eq!(editor.state.join(), b"start");
        assert_eq!(editor.view.column, 5);
        assert!(editor.state.dirty);
        assert!(editor.poll_save().unwrap());
        assert_eq!(fs::read(&file).unwrap(), b"start");
        editor.commands().redo();
        assert_eq!(editor.state.join(), b"start-edit");
        assert_eq!(editor.view.column, 10);
        assert!(editor.poll_save().unwrap());
        assert_eq!(fs::read(file).unwrap(), b"start-edit");
    }
}

#[test]
fn selection_replacement_is_one_history_group_and_restores_selection() {
    let mut editor = document(b"abcdef", Path::new("test://replace"));
    editor
        .commands()
        .set_selection(0, 1, 0, 4, bed_core::editor_commands::CursorReveal::Ensure);
    editor.commands().type_text(b"Z");
    assert_eq!(editor.state.join(), b"aZef");
    editor.commands().undo();
    assert_eq!(editor.state.join(), b"abcdef");
    assert_eq!(editor.view.get_ordered(), (0, 1, 0, 4));
}

#[test]
fn per_file_stacks_remain_independent_and_disk_history_reloads() {
    let temp = TempDir::new();
    let file_a = temp.file("a.txt");
    let file_b = temp.file("b.txt");
    let mut editor = document(b"A", &file_a);
    editor.undo.project_root = temp.0.to_str().unwrap().to_owned();
    editor
        .commands()
        .set_cursor(0, 1, false, bed_core::editor_commands::CursorReveal::Ensure);
    editor.commands().type_text(b"1");
    editor.set_content(b"B");
    editor.state.path = file_b.to_str().unwrap().to_owned();
    editor.view.set_both(0, 1);
    editor.commands().type_text(b"2");
    editor.commands().undo();
    assert_eq!(editor.state.join(), b"B");
    editor.undo.flush();
    let mut restored = document(b"A1", &file_a);
    restored.undo.project_root = temp.0.to_str().unwrap().to_owned();
    restored
        .undo
        .load_project(&restored.undo.project_root.clone());
    restored.commands().undo();
    assert_eq!(restored.state.join(), b"A");
    assert_eq!(restored.view.column, 1);
}

#[test]
fn multi_cursor_edit_undo_restores_primary_and_all_carets() {
    let mut editor = document(b"a\nb", Path::new("test://multi"));
    let mut first = Selection::default();
    first.set_both(0, 1);
    let mut second = Selection::default();
    second.set_both(1, 1);
    editor
        .view
        .set_selections(&editor.state, vec![first, second], 1);
    editor.commands().type_text(b"!");
    assert_eq!(editor.state.join(), b"a!\nb!");
    editor.commands().undo();
    assert_eq!(editor.state.join(), b"a\nb");
    assert_eq!(editor.view.primary_index, 1);
    assert_eq!(editor.view.selection_count(), 2);
    editor.commands().redo();
    assert_eq!(editor.state.join(), b"a!\nb!");
}

#[test]
fn failed_open_preserves_current_document_and_pending_save() {
    let temp = TempDir::new();
    let current = temp.file("current.txt");
    let mut editor = document(b"abc", &current);
    editor.commands().type_text(b"x");
    assert!(editor.open(&temp.file("missing.txt")).is_err());
    assert_eq!(editor.state.join(), b"xabc");
    assert_eq!(editor.state.path, current.to_str().unwrap());
    assert!(editor.state.dirty);
}

#[test]
fn reopening_dirty_current_file_loads_the_just_saved_bytes() {
    let temp = TempDir::new();
    let file = temp.file("current.txt");
    fs::write(&file, b"abc").unwrap();
    let mut editor = Editor::new();
    editor.open(&file).unwrap();
    editor.commands().type_text(b"x");
    editor.open(&file).unwrap();
    assert_eq!(editor.state.join(), b"xabc");
    assert_eq!(fs::read(&file).unwrap(), b"xabc");
    assert!(!editor.state.dirty);
}
