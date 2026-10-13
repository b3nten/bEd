//! Ported cases from ned tests/editor/{save_service,undo_service}_test.cpp.

use std::{
    cell::RefCell,
    fs,
    path::{Path, PathBuf},
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
};

use bed_documents::document_runtime::DocumentRuntime;
use bed_editing::text_view_state::Selection;

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

fn document(raw: &[u8], path: &Path) -> DocumentRuntime {
    let mut runtime = DocumentRuntime::new();
    runtime.set_content(raw);
    runtime.state.path = path.to_str().unwrap().to_owned();
    runtime.state.set_line_ending(b"\n");
    runtime.undo.ensure_file(&runtime.state.path);
    runtime
}

#[test]
fn dirty_path_writes_clears_dirty_and_emits_version() {
    let temp = TempDir::new();
    let file = temp.file("note.txt");
    fs::write(&file, b"old").unwrap();
    let mut runtime = document(b"new content", &file);
    runtime.state.dirty = true;
    runtime.state.version = 3;
    let saved = Rc::new(RefCell::new(Vec::new()));
    let observed = Rc::clone(&saved);
    runtime
        .events
        .subscribe_did_save(move |event| observed.borrow_mut().push(event.clone()));
    assert!(runtime.save().unwrap());
    assert!(!runtime.state.dirty);
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
    let mut runtime = DocumentRuntime::new();
    runtime.open(&file).unwrap();
    assert!(runtime.state.utf8_bom);
    assert_eq!(runtime.state.join(), &bytes[3..]);
    runtime.state.dirty = true;
    runtime.save().unwrap();
    assert_eq!(fs::read(&file).unwrap(), bytes);
    runtime.set_content(b"plain\0\xff");
    runtime.state.dirty = true;
    runtime.save().unwrap();
    assert_eq!(fs::read(&file).unwrap(), b"plain\0\xff");
    runtime.set_content(b"would overwrite");
    assert!(!runtime.save().unwrap());
    assert_eq!(fs::read(&file).unwrap(), b"plain\0\xff");
}

#[test]
fn empty_path_and_oversized_save_keep_dirty_and_disk_unchanged() {
    let mut runtime = DocumentRuntime::new();
    runtime.set_content(b"x");
    runtime.state.dirty = true;
    assert!(!runtime.save().unwrap());
    assert!(runtime.state.dirty);
    let temp = TempDir::new();
    let file = temp.file("big.txt");
    fs::write(&file, b"ORIGINAL_ON_DISK").unwrap();
    let mut runtime = document(&vec![b'a'; bed_files::files::MAX_FILE_SIZE + 1], &file);
    runtime.state.dirty = true;
    let error = runtime.save().unwrap_err();
    assert!(error.to_string().contains("128 MiB"));
    assert!(bed_documents::save_service::DocumentSave::bytes_for_save(&runtime.state).is_err());
    assert!(runtime.state.dirty);
    assert_eq!(fs::read(&file).unwrap(), b"ORIGINAL_ON_DISK");
}

#[test]
fn legitimate_text_resembling_the_old_truncation_notice_can_be_saved() {
    let temp = TempDir::new();
    let file = temp.file("notice.txt");
    let bytes = b"\n\n[File truncated - No Edits - showing first 1MB of 5MB]\nactual source text";
    let mut runtime = document(bytes, &file);
    runtime.state.dirty = true;
    assert!(runtime.save().unwrap());
    assert_eq!(fs::read(&file).unwrap(), bytes);
}

#[test]
fn save_limit_includes_the_utf8_bom_before_touching_the_destination() {
    let temp = TempDir::new();
    let file = temp.file("bom-limit.txt");
    fs::write(&file, b"original").unwrap();
    let mut runtime = document(&vec![b'a'; bed_files::files::MAX_FILE_SIZE - 2], &file);
    runtime.state.utf8_bom = true;
    runtime.state.dirty = true;
    assert!(runtime.save().is_err());
    assert!(runtime.state.dirty);
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
    let mut runtime = DocumentRuntime::new();
    runtime.set_content(b"preserved");
    runtime.view.column = 4;
    assert!(runtime.open(&file).is_err());
    assert_eq!(runtime.state.join(), b"preserved");
    assert_eq!(runtime.view.column, 4);
    assert!(runtime.state.path.is_empty());
}

#[test]
fn autosave_is_idle_debounced_and_explicit_save_cancels_schedule() {
    let temp = TempDir::new();
    let file = temp.file("note.txt");
    fs::write(&file, b"old").unwrap();
    let mut runtime = document(b"new", &file);
    runtime.state.dirty = true;
    runtime.save_service.on_did_edit(&runtime.state);
    assert!(!runtime.poll_save().unwrap());
    assert_eq!(fs::read(&file).unwrap(), b"old");
    runtime.save_service.set_autosave_idle_ms(0);
    assert!(runtime.poll_save().unwrap());
    assert_eq!(fs::read(&file).unwrap(), b"new");
    runtime.commands().type_text(b"again");
    runtime.save().unwrap();
    assert!(!runtime.poll_save().unwrap());
    runtime.state.path.clear();
    runtime.state.dirty = true;
    runtime.save_service.on_did_edit(&runtime.state);
    assert!(!runtime.poll_save().unwrap());
    assert!(runtime.state.dirty);
}

#[test]
fn failed_write_does_not_mark_saved_or_emit_success() {
    let temp = TempDir::new();
    let mut runtime = document(b"text", &temp.0);
    runtime.state.dirty = true;
    let saves = Rc::new(RefCell::new(0));
    let count = Rc::clone(&saves);
    runtime
        .events
        .subscribe_did_save(move |_| *count.borrow_mut() += 1);
    assert!(runtime.save().is_err());
    assert!(runtime.state.dirty);
    assert_eq!(*saves.borrow(), 0);
}

#[test]
fn real_commands_schedule_autosave_and_group_adjacent_inserts() {
    let temp = TempDir::new();
    let file = temp.file("doc.txt");
    let mut runtime = document(b"", &file);
    runtime.commands().type_text(b"a");
    runtime.commands().type_text(b"b");
    runtime.commands().type_text(b"c");
    assert_eq!(runtime.state.join(), b"abc");
    runtime.commands().undo();
    assert_eq!(runtime.state.join(), b"");
    assert_eq!(runtime.view.column, 0);
    runtime.commands().redo();
    assert_eq!(runtime.state.join(), b"abc");
    runtime.save_service.set_autosave_idle_ms(0);
    assert!(runtime.poll_save().unwrap());
    assert_eq!(fs::read(file).unwrap(), b"abc");
}

#[test]
fn autosave_retains_named_document_undo_and_redo_stacks() {
    for shared_history in [false, true] {
        let temp = TempDir::new();
        let file = temp.file("doc.txt");
        fs::write(&file, b"start").unwrap();
        let mut runtime = DocumentRuntime::new();
        if shared_history {
            runtime.bind_project_undo(Rc::new(RefCell::new(
                bed_editing::project_undo::ProjectUndo::default(),
            )));
        }
        runtime.open(&file).unwrap();
        runtime.api().restore_caret(0, 5);
        runtime.save_service.set_autosave_idle_ms(0);
        runtime.commands().type_text(b"-edit");
        assert!(runtime.poll_save().unwrap());
        assert_eq!(fs::read(&file).unwrap(), b"start-edit");
        assert!(!runtime.state.dirty);
        runtime.commands().undo();
        assert_eq!(runtime.state.join(), b"start");
        assert_eq!(runtime.view.column, 5);
        assert!(runtime.state.dirty);
        assert!(runtime.poll_save().unwrap());
        assert_eq!(fs::read(&file).unwrap(), b"start");
        runtime.commands().redo();
        assert_eq!(runtime.state.join(), b"start-edit");
        assert_eq!(runtime.view.column, 10);
        assert!(runtime.poll_save().unwrap());
        assert_eq!(fs::read(file).unwrap(), b"start-edit");
    }
}

#[test]
fn selection_replacement_is_one_history_group_and_restores_selection() {
    let mut runtime = document(b"abcdef", Path::new("test://replace"));
    runtime
        .commands()
        .set_selection(0, 1, 0, 4, bed_editing::text_commands::CursorReveal::Ensure);
    runtime.commands().type_text(b"Z");
    assert_eq!(runtime.state.join(), b"aZef");
    runtime.commands().undo();
    assert_eq!(runtime.state.join(), b"abcdef");
    assert_eq!(runtime.view.get_ordered(), (0, 1, 0, 4));
}

#[test]
fn per_file_stacks_remain_independent_and_disk_history_reloads() {
    let temp = TempDir::new();
    let file_a = temp.file("a.txt");
    let file_b = temp.file("b.txt");
    let mut runtime = document(b"A", &file_a);
    runtime.undo.project_root = temp.0.to_str().unwrap().to_owned();
    runtime.commands().set_cursor(
        0,
        1,
        false,
        bed_editing::text_commands::CursorReveal::Ensure,
    );
    runtime.commands().type_text(b"1");
    runtime.set_content(b"B");
    runtime.state.path = file_b.to_str().unwrap().to_owned();
    runtime.view.set_both(0, 1);
    runtime.commands().type_text(b"2");
    runtime.commands().undo();
    assert_eq!(runtime.state.join(), b"B");
    runtime.undo.flush();
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
    let mut runtime = document(b"a\nb", Path::new("test://multi"));
    let mut first = Selection::default();
    first.set_both(0, 1);
    let mut second = Selection::default();
    second.set_both(1, 1);
    runtime
        .view
        .set_selections(&runtime.state, vec![first, second], 1);
    runtime.commands().type_text(b"!");
    assert_eq!(runtime.state.join(), b"a!\nb!");
    runtime.commands().undo();
    assert_eq!(runtime.state.join(), b"a\nb");
    assert_eq!(runtime.view.primary_index, 1);
    assert_eq!(runtime.view.selection_count(), 2);
    runtime.commands().redo();
    assert_eq!(runtime.state.join(), b"a!\nb!");
}

#[test]
fn failed_open_preserves_current_document_and_pending_save() {
    let temp = TempDir::new();
    let current = temp.file("current.txt");
    let mut runtime = document(b"abc", &current);
    runtime.commands().type_text(b"x");
    assert!(runtime.open(&temp.file("missing.txt")).is_err());
    assert_eq!(runtime.state.join(), b"xabc");
    assert_eq!(runtime.state.path, current.to_str().unwrap());
    assert!(runtime.state.dirty);
}

#[test]
fn reopening_dirty_current_file_loads_the_just_saved_bytes() {
    let temp = TempDir::new();
    let file = temp.file("current.txt");
    fs::write(&file, b"abc").unwrap();
    let mut runtime = DocumentRuntime::new();
    runtime.open(&file).unwrap();
    runtime.commands().type_text(b"x");
    runtime.open(&file).unwrap();
    assert_eq!(runtime.state.join(), b"xabc");
    assert_eq!(fs::read(&file).unwrap(), b"xabc");
    assert!(!runtime.state.dirty);
}
