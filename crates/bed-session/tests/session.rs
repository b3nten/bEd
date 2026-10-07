//! Shared ownership regressions around the translated document/command layers.
use bed_core::{editor_commands::CursorReveal, editor_view_state::Selection};
use bed_session::{ClosePolicy, EditorSession, SessionEvent, SessionOptions};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime},
};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "bed-session-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn file(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let p = self.0.join(name);
        fs::write(&p, bytes).unwrap();
        p
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn files_beyond_one_mib_open_edit_save_and_reload_without_synthetic_text() {
    let fixture = Fixture::new();
    let mut raw = b"\xef\xbb\xbfheader\r\n".to_vec();
    raw.resize(2 * 1024 * 1024, b'a');
    let path = fixture.file("large.txt", &raw);
    let mut session = EditorSession::new();
    let document = session.open_file(&path).unwrap();
    let view = session.create_view(document).unwrap();
    assert_eq!(session.snapshot(document).unwrap().bytes, &raw[3..]);
    session
        .with_commands(view, |commands| commands.type_text(b"edited "))
        .unwrap();
    assert!(session.save(document).unwrap());
    let saved = fs::read(&path).unwrap();
    assert!(saved.starts_with(b"\xef\xbb\xbfedited header\r\n"));
    assert_eq!(saved.len(), raw.len() + 7);
    session.reload_from_disk(document).unwrap();
    assert_eq!(session.snapshot(document).unwrap().bytes, &saved[3..]);
}

#[test]
fn oversized_open_and_reload_leave_the_session_and_existing_buffer_intact() {
    let fixture = Fixture::new();
    let oversized = fixture.file("oversized.txt", b"");
    fs::File::options()
        .write(true)
        .open(&oversized)
        .unwrap()
        .set_len(bed_files::files::MAX_FILE_SIZE as u64 + 1)
        .unwrap();
    let mut session = EditorSession::new();
    let path = fixture.file("preserved.txt", b"preserved");
    let document = session.open_file(&path).unwrap();
    let before = session.snapshot(document).unwrap();
    let error = session.open_file(&oversized).unwrap_err();
    assert!(error.to_string().contains("oversized.txt"));
    assert_eq!(session.document_ids(), vec![document]);
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(bed_files::files::MAX_FILE_SIZE as u64 + 1)
        .unwrap();
    assert!(session.reload_from_disk(document).is_err());
    let after = session.snapshot(document).unwrap();
    assert_eq!(after.bytes, before.bytes);
    assert_eq!(after.generation, before.generation);
    assert_eq!(after.dirty, before.dirty);
    assert_eq!(after.path, before.path);
    assert!(after.disk_conflict.is_some());
    let view = session.create_view(document).unwrap();
    session
        .with_commands(view, |commands| commands.type_text(b"local "))
        .unwrap();
    assert!(session.save(document).is_err());
    assert_eq!(
        fs::metadata(&path).unwrap().len(),
        bed_files::files::MAX_FILE_SIZE as u64 + 1
    );
}

#[test]
fn utf8_crlf_splices_transform_sibling_selection_and_undo_without_scroll_changes() {
    let mut session = EditorSession::new();
    let doc = session
        .create_document("A🙂é\r\nbeta\r\ngamma".as_bytes())
        .unwrap();
    let a = session.create_view(doc).unwrap();
    let b = session.create_view(doc).unwrap();
    session
        .with_commands(a, |c| c.set_cursor(0, 1, false, CursorReveal::Ensure))
        .unwrap();
    session
        .with_commands(b, |c| c.set_selection(0, 1, 1, 2, CursorReveal::Ensure))
        .unwrap();
    session.set_scroll(b, 17.0, 40.0).unwrap();
    let before = session.view_snapshot(b).unwrap();
    session
        .with_commands(a, |c| c.type_text("Ω\r\nx".as_bytes()))
        .unwrap();
    assert_eq!(
        session.snapshot(doc).unwrap().bytes,
        "AΩ\r\nx🙂é\r\nbeta\r\ngamma".as_bytes()
    );
    let after = session.view_snapshot(b).unwrap();
    assert_eq!(
        (after.primary().anchor_row, after.primary().anchor_column),
        (1, 1)
    );
    assert_eq!((after.row, after.column), (2, 2));
    assert_eq!(after.requested_scroll, before.requested_scroll);
    assert_eq!(after.cursor_blink_time, before.cursor_blink_time);
    session.with_commands(a, |c| c.undo()).unwrap();
    let restored = session.view_snapshot(b).unwrap();
    assert_eq!(restored.selections, before.selections);
    session.with_commands(a, |c| c.redo()).unwrap();
    session.with_commands(b, |c| c.undo()).unwrap();
    assert_eq!(
        session.snapshot(doc).unwrap().bytes,
        "A🙂é\r\nbeta\r\ngamma".as_bytes()
    );
    assert_eq!(
        (
            session.view_snapshot(b).unwrap().row,
            session.view_snapshot(b).unwrap().column
        ),
        (0, 1)
    );
}

#[test]
fn switching_editing_views_seals_typing_and_shared_undo_uses_callers_view() {
    let mut session = EditorSession::new();
    let doc = session.create_document(b"").unwrap();
    let a = session.create_view(doc).unwrap();
    let b = session.create_view(doc).unwrap();
    session.with_commands(a, |c| c.type_text(b"X")).unwrap();
    session.with_commands(b, |c| c.type_text(b"Y")).unwrap();
    assert_eq!(session.snapshot(doc).unwrap().bytes, b"XY");
    session.with_commands(a, |c| c.undo()).unwrap();
    assert_eq!(session.snapshot(doc).unwrap().bytes, b"X");
    assert_eq!(session.view_snapshot(a).unwrap().column, 1);
    session.with_commands(a, |c| c.undo()).unwrap();
    assert_eq!(session.snapshot(doc).unwrap().bytes, b"");
    session.with_commands(b, |c| c.redo()).unwrap();
    assert_eq!(session.snapshot(doc).unwrap().bytes, b"X");
    session.with_commands(a, |c| c.redo()).unwrap();
    assert_eq!(session.snapshot(doc).unwrap().bytes, b"XY");
}

#[test]
fn multiple_cursor_reverse_edits_collapse_sibling_endpoints_at_actual_deleted_bytes() {
    let mut session = EditorSession::new();
    let doc = session.create_document("a🙂bc\nxyz".as_bytes()).unwrap();
    let a = session.create_view(doc).unwrap();
    let b = session.create_view(doc).unwrap();
    session
        .with_commands(b, |c| c.set_selection(0, 3, 1, 2, CursorReveal::Ensure))
        .unwrap(); // byte 3 snaps to emoji start.
    session
        .with_commands(a, |c| {
            c.set_selections(
                vec![
                    Selection {
                        head_row: 0,
                        head_column: 1,
                        anchor_row: 0,
                        anchor_column: 5,
                        ..Selection::default()
                    },
                    Selection {
                        head_row: 1,
                        head_column: 0,
                        anchor_row: 1,
                        anchor_column: 2,
                        ..Selection::default()
                    },
                ],
                0,
                CursorReveal::Ensure,
            )
        })
        .unwrap();
    session.with_commands(a, |c| c.type_text(b"Q")).unwrap();
    assert_eq!(session.snapshot(doc).unwrap().bytes, b"aQbc\nQz");
    let sibling = session.view_snapshot(b).unwrap();
    assert_eq!(
        (
            sibling.primary().anchor_row,
            sibling.primary().anchor_column
        ),
        (0, 2)
    );
    assert_eq!((sibling.row, sibling.column), (1, 1));
    session.with_commands(a, |c| c.undo()).unwrap();
    assert_eq!(
        session.snapshot(doc).unwrap().bytes,
        "a🙂bc\nxyz".as_bytes()
    );
    assert_eq!(session.view_snapshot(a).unwrap().selections.len(), 2);
}

#[test]
fn canonical_open_returns_same_doc_and_views_have_distinct_ids() {
    let temp = Fixture::new();
    let path = temp.file("a.txt", b"alpha");
    let mut session = EditorSession::new();
    let doc = session.open_file(&path).unwrap();
    assert_eq!(session.open_file(&temp.0.join("./a.txt")).unwrap(), doc);
    let a = session.create_view(doc).unwrap();
    let b = session.create_view(doc).unwrap();
    assert_ne!(a, b);
    assert_eq!(session.document_ids(), [doc]);
    assert_eq!(session.view_count(doc), 2);
}

#[test]
fn one_autosave_for_two_views_keeps_shared_undo_after_save() {
    let temp = Fixture::new();
    let path = temp.file("a.txt", b"base");
    let mut session = EditorSession::with_options(SessionOptions {
        autosave: Some(Duration::ZERO),
        monitoring: true,
        ..SessionOptions::default()
    })
    .unwrap();
    let doc = session.open_file(&path).unwrap();
    let a = session.create_view(doc).unwrap();
    let b = session.create_view(doc).unwrap();
    session
        .with_commands(a, |c| {
            c.move_doc_end(false);
            c.type_text(b"X");
        })
        .unwrap();
    let report = session.tick();
    assert!(report.errors.is_empty());
    assert_eq!(
        report
            .events
            .iter()
            .filter(|e| matches!(e,SessionEvent::Saved{document,..}if *document==doc))
            .count(),
        1
    );
    assert_eq!(fs::read(&path).unwrap(), b"baseX");
    assert!(!session.snapshot(doc).unwrap().dirty);
    session.with_commands(b, |c| c.undo()).unwrap();
    assert_eq!(session.snapshot(doc).unwrap().bytes, b"base");
    assert!(session.snapshot(doc).unwrap().dirty);
    session.tick();
    assert_eq!(fs::read(&path).unwrap(), b"base");
}

#[test]
fn tick_error_does_not_starve_later_docs_or_recreate_removed_paths() {
    let temp = Fixture::new();
    let bad = temp.file("bad.txt", b"bad");
    let good = temp.file("good.txt", b"good");
    let mut session = EditorSession::with_options(SessionOptions {
        autosave: Some(Duration::ZERO),
        ..SessionOptions::default()
    })
    .unwrap();
    let d1 = session.open_file(&bad).unwrap();
    let d2 = session.open_file(&good).unwrap();
    let a = session.create_view(d1).unwrap();
    let b = session.create_view(d2).unwrap();
    session.with_commands(a, |c| c.type_text(b"X")).unwrap();
    session.with_commands(b, |c| c.type_text(b"Y")).unwrap();
    fs::remove_file(&bad).unwrap();
    let report = session.tick();
    assert!(!bad.exists());
    assert_eq!(fs::read(&good).unwrap(), b"Ygood");
    assert!(
        report
            .errors
            .iter()
            .any(|e| e.document == Some(d1) && e.service == "autosave")
    );
    assert!(session.snapshot(d1).unwrap().dirty);
    assert!(!session.snapshot(d2).unwrap().dirty);
}

#[test]
fn drop_never_saves_dirty_file_or_persistent_history() {
    let temp = Fixture::new();
    let path = temp.file("a.txt", b"original");
    {
        let mut session = EditorSession::with_options(SessionOptions {
            project_root: Some(temp.0.clone()),
            autosave: Some(Duration::ZERO),
            persistent_history: true,
            ..SessionOptions::default()
        })
        .unwrap();
        let doc = session.open_file(&path).unwrap();
        let view = session.create_view(doc).unwrap();
        session.with_commands(view, |c| c.type_text(b"X")).unwrap();
    }
    assert_eq!(fs::read(&path).unwrap(), b"original");
    assert!(!temp.0.join(".undo-redo-bed.json").exists());
}

#[test]
fn only_last_explicit_view_close_saves_and_failed_close_preserves_state() {
    let temp = Fixture::new();
    let path = temp.file("a.txt", b"a");
    let mut session = EditorSession::new();
    let doc = session.open_file(&path).unwrap();
    let a = session.create_view(doc).unwrap();
    let b = session.create_view(doc).unwrap();
    session.with_commands(a, |c| c.type_text(b"X")).unwrap();
    assert!(session.close_view(a, ClosePolicy::Save).unwrap());
    assert_eq!(fs::read(&path).unwrap(), b"a");
    assert_eq!(session.view_count(doc), 1);
    fs::remove_file(&path).unwrap();
    let before = session.view_snapshot(b).unwrap();
    assert!(session.close_view(b, ClosePolicy::Save).is_err());
    assert_eq!(session.view_snapshot(b).unwrap(), before);
    assert_eq!(session.snapshot(doc).unwrap().bytes, b"Xa");
    assert!(!path.exists());
    assert!(!session.close_view(b, ClosePolicy::Cancel).unwrap());
    assert!(session.close_view(b, ClosePolicy::Discard).unwrap());
    assert!(session.snapshot(doc).is_err());
}

#[test]
fn save_as_rebinds_once_without_changing_doc_views_or_undo() {
    let temp = Fixture::new();
    let old = temp.file("old.txt", b"base");
    let new = temp.0.join("new.txt");
    let mut session = EditorSession::new();
    let doc = session.open_file(&old).unwrap();
    let a = session.create_view(doc).unwrap();
    let b = session.create_view(doc).unwrap();
    session
        .with_commands(a, |c| {
            c.move_doc_end(false);
            c.type_text(b"X");
        })
        .unwrap();
    let va = session.view_snapshot(a).unwrap();
    let vb = session.view_snapshot(b).unwrap();
    session.save_as(doc, &new).unwrap();
    assert_eq!(session.document_for_path(&new), Some(doc));
    assert_eq!(session.view_snapshot(a).unwrap(), va);
    assert_eq!(session.view_snapshot(b).unwrap(), vb);
    assert_eq!(fs::read(&new).unwrap(), b"baseX");
    assert_eq!(fs::read(&old).unwrap(), b"base");
    session.with_commands(b, |c| c.undo()).unwrap();
    assert_eq!(session.snapshot(doc).unwrap().bytes, b"base");
    session.save(doc).unwrap();
    assert_eq!(fs::read(&new).unwrap(), b"base");
    assert_eq!(fs::read(&old).unwrap(), b"base");
}

#[test]
fn failed_save_as_and_live_path_collisions_preserve_identity() {
    let temp = Fixture::new();
    let a = temp.file("a.txt", b"a");
    let b = temp.file("b.txt", b"b");
    let mut session = EditorSession::new();
    let d1 = session.open_file(&a).unwrap();
    let d2 = session.open_file(&b).unwrap();
    let v = session.create_view(d1).unwrap();
    session.with_commands(v, |c| c.type_text(b"X")).unwrap();
    let before = session.snapshot(d1).unwrap();
    assert!(session.save_as(d1, &b).is_err());
    assert_eq!(session.snapshot(d1).unwrap(), before);
    assert_eq!(session.document_for_path(&b), Some(d2));
    assert!(
        session
            .save_as(d1, &temp.0.join("missing/new.txt"))
            .is_err()
    );
    assert_eq!(session.snapshot(d1).unwrap(), before);
    assert_eq!(fs::read(&a).unwrap(), b"a");
}

#[test]
fn failed_destination_write_restores_binding_views_history_and_autosave() {
    let temp = Fixture::new();
    let original = temp.file("original.txt", "base🙂".as_bytes());
    let directory = temp.0.join("directory");
    fs::create_dir(&directory).unwrap();
    let mut session = EditorSession::with_options(SessionOptions {
        autosave: Some(Duration::ZERO),
        monitoring: true,
        ..SessionOptions::default()
    })
    .unwrap();
    let doc = session.open_file(&original).unwrap();
    let a = session.create_view(doc).unwrap();
    let b = session.create_view(doc).unwrap();
    session
        .with_commands(a, |commands| {
            commands.move_doc_end(false);
            commands.type_text("雪".as_bytes());
        })
        .unwrap();
    session.set_scroll(b, 7.0, 33.0).unwrap();
    let document_before = session.snapshot(doc).unwrap();
    let a_before = session.view_snapshot(a).unwrap();
    let b_before = session.view_snapshot(b).unwrap();
    assert!(session.save_as(doc, &directory).is_err());
    assert_eq!(session.snapshot(doc).unwrap(), document_before);
    assert_eq!(session.view_snapshot(a).unwrap(), a_before);
    assert_eq!(session.view_snapshot(b).unwrap(), b_before);
    assert_eq!(session.document_for_path(&original), Some(doc));
    assert_eq!(session.document_for_path(&directory), None);
    assert_eq!(fs::read(&original).unwrap(), "base🙂".as_bytes());
    let report = session.tick();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    let saved: Vec<_> = report
        .events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::Saved { document, save } => Some((*document, save.path.as_str())),
            _ => None,
        })
        .collect();
    assert_eq!(saved, vec![(doc, document_before.path.as_str())]);
    assert_eq!(fs::read(&original).unwrap(), "base🙂雪".as_bytes());
    session
        .with_commands(b, |commands| commands.undo())
        .unwrap();
    assert_eq!(session.snapshot(doc).unwrap().bytes, "base🙂".as_bytes());
    assert!(directory.is_dir());
}

#[test]
fn clean_external_reload_clamps_each_view_and_preserves_each_scroll() {
    let temp = Fixture::new();
    let path = temp.file("a.txt", b"alpha\nbeta\ngamma");
    let mut session = EditorSession::with_options(SessionOptions {
        monitoring: true,
        ..SessionOptions::default()
    })
    .unwrap();
    let doc = session.open_file(&path).unwrap();
    let a = session.create_view(doc).unwrap();
    let b = session.create_view(doc).unwrap();
    session
        .with_commands(a, |c| c.set_cursor(2, 4, false, CursorReveal::Ensure))
        .unwrap();
    session
        .with_commands(b, |c| c.set_cursor(1, 3, false, CursorReveal::Ensure))
        .unwrap();
    session.set_scroll(a, 2.0, 80.0).unwrap();
    session.set_scroll(b, 9.0, 30.0).unwrap();
    fs::write(&path, b"z").unwrap();
    fs::File::open(&path)
        .unwrap()
        .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(7))
        .unwrap();
    let report = session.tick();
    assert!(report.errors.is_empty());
    assert_eq!(session.snapshot(doc).unwrap().bytes, b"z");
    assert!(
        report
            .events
            .iter()
            .any(|e| matches!(e,SessionEvent::Reloaded{document,..}if *document==doc))
    );
    assert_eq!(
        session.view_snapshot(a).unwrap().requested_scroll,
        Some([2.0, 80.0])
    );
    assert_eq!(
        session.view_snapshot(b).unwrap().requested_scroll,
        Some([9.0, 30.0])
    );
    assert_eq!(session.view_snapshot(a).unwrap().column, 1);
    assert_eq!(session.view_snapshot(b).unwrap().column, 1);
}

#[test]
fn enabling_monitoring_uses_the_shared_loaded_baseline_and_reloads_once() {
    let temp = Fixture::new();
    let path = temp.file("toggle.txt", "original🙂".as_bytes());
    let mut session = EditorSession::new();
    let doc = session.open_file(&path).unwrap();
    let a = session.create_view(doc).unwrap();
    let b = session.create_view(doc).unwrap();
    session
        .with_commands(a, |commands| commands.move_doc_end(false))
        .unwrap();
    session.set_scroll(b, 5.0, 13.0).unwrap();
    fs::write(&path, b"changed").unwrap();
    let disabled = session.tick();
    assert!(disabled.errors.is_empty());
    assert_eq!(
        session.snapshot(doc).unwrap().bytes,
        "original🙂".as_bytes()
    );
    let generation = session.snapshot(doc).unwrap().generation;
    let b_before = session.view_snapshot(b).unwrap();
    let mut options = session.options().clone();
    options.monitoring = true;
    session.configure(options).unwrap();
    let report = session.tick();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(
        report
            .events
            .iter()
            .filter(
                |event| matches!(event, SessionEvent::Reloaded { document, .. } if *document == doc)
            )
            .count(),
        1
    );
    let snapshot = session.snapshot(doc).unwrap();
    assert_eq!(snapshot.bytes, b"changed");
    assert_eq!(snapshot.generation, generation + 1);
    assert!(!snapshot.dirty);
    assert_eq!(session.document_ids(), vec![doc]);
    assert_eq!(session.view_count(doc), 2);
    assert_eq!(session.view_snapshot(a).unwrap().column, 7);
    assert_eq!(
        session.view_snapshot(b).unwrap().requested_scroll,
        b_before.requested_scroll
    );
}

#[test]
fn enabling_monitoring_blocks_external_overwrite_but_allows_normal_dirty_autosave() {
    let temp = Fixture::new();
    let changed = temp.file("changed.txt", b"original");
    let unchanged = temp.file("unchanged.txt", b"original");
    let mut session = EditorSession::new();
    let changed_doc = session.open_file(&changed).unwrap();
    let unchanged_doc = session.open_file(&unchanged).unwrap();
    for doc in [changed_doc, unchanged_doc] {
        let view = session.create_view(doc).unwrap();
        session
            .with_commands(view, |commands| commands.type_text(b"X"))
            .unwrap();
    }
    fs::write(&changed, b"external content").unwrap();
    let mut options = session.options().clone();
    options.monitoring = true;
    options.autosave = Some(Duration::ZERO);
    session.configure(options).unwrap();
    let report = session.tick();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert!(
        session
            .snapshot(changed_doc)
            .unwrap()
            .disk_conflict
            .is_some()
    );
    assert!(session.snapshot(changed_doc).unwrap().dirty);
    assert_eq!(session.snapshot(changed_doc).unwrap().bytes, b"Xoriginal");
    assert_eq!(fs::read(&changed).unwrap(), b"external content");
    assert!(
        session
            .snapshot(unchanged_doc)
            .unwrap()
            .disk_conflict
            .is_none()
    );
    assert!(!session.snapshot(unchanged_doc).unwrap().dirty);
    assert_eq!(fs::read(&unchanged).unwrap(), b"Xoriginal");
    let saved: Vec<_> = report
        .events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::Saved { document, .. } => Some(*document),
            _ => None,
        })
        .collect();
    assert_eq!(saved, vec![unchanged_doc]);
}

#[test]
fn dirty_external_conflict_blocks_autosave_until_explicit_resolution() {
    let temp = Fixture::new();
    let path = temp.file("a.txt", b"disk");
    let mut session = EditorSession::with_options(SessionOptions {
        monitoring: true,
        autosave: Some(Duration::ZERO),
        ..SessionOptions::default()
    })
    .unwrap();
    let doc = session.open_file(&path).unwrap();
    let view = session.create_view(doc).unwrap();
    session.with_commands(view, |c| c.type_text(b"X")).unwrap();
    fs::write(&path, b"external").unwrap();
    let report = session.tick();
    assert!(
        report
            .events
            .iter()
            .any(|e| matches!(e,SessionEvent::Conflict{document,..}if *document==doc))
    );
    assert_eq!(fs::read(&path).unwrap(), b"external");
    assert_eq!(session.snapshot(doc).unwrap().bytes, b"Xdisk");
    session.keep_buffer(doc).unwrap();
    session.tick();
    assert_eq!(fs::read(&path).unwrap(), b"Xdisk");
}

#[test]
fn rename_resumes_autosave_to_new_path_with_stable_document_and_views() {
    let temp = Fixture::new();
    let old = temp.file("old.txt", b"base");
    let new = temp.0.join("renamed.txt");
    let mut session = EditorSession::with_options(SessionOptions {
        autosave: Some(Duration::ZERO),
        monitoring: true,
        ..SessionOptions::default()
    })
    .unwrap();
    let doc = session.open_file(&old).unwrap();
    let a = session.create_view(doc).unwrap();
    let b = session.create_view(doc).unwrap();
    session.with_commands(a, |c| c.type_text(b"X")).unwrap();
    session.set_scroll(b, 1.0, 27.0).unwrap();
    fs::rename(&old, &new).unwrap();
    session.rebind_path(doc, &new).unwrap();
    let report = session.tick();
    assert!(report.errors.is_empty());
    assert!(!old.exists());
    assert_eq!(fs::read(&new).unwrap(), b"Xbase");
    assert_eq!(
        session.view_snapshot(b).unwrap().requested_scroll,
        Some([1.0, 27.0])
    );
    assert_eq!(session.document_for_view(a), Some(doc));
    assert_eq!(session.document_for_view(b), Some(doc));
    assert_eq!(session.document_for_path(&new), Some(doc));
    session.with_commands(b, |c| c.undo()).unwrap();
    assert_eq!(session.snapshot(doc).unwrap().bytes, b"base");
}

#[test]
fn paused_autosave_leaves_dirty_bytes_until_host_resumes() {
    let temp = Fixture::new();
    let path = temp.file("a.txt", b"base");
    let mut session = EditorSession::with_options(SessionOptions {
        autosave: Some(Duration::ZERO),
        ..SessionOptions::default()
    })
    .unwrap();
    let doc = session.open_file(&path).unwrap();
    let view = session.create_view(doc).unwrap();
    session.with_commands(view, |c| c.type_text(b"X")).unwrap();
    session.pause_autosave(doc, true).unwrap();
    session.tick();
    assert_eq!(fs::read(&path).unwrap(), b"base");
    assert!(session.snapshot(doc).unwrap().dirty);
    session.pause_autosave(doc, false).unwrap();
    session.tick();
    assert_eq!(fs::read(&path).unwrap(), b"Xbase");
}

#[test]
fn failed_history_flush_is_reported_and_retried_without_starving_save() {
    let temp = Fixture::new();
    let root = temp.0.join("project");
    fs::create_dir(&root).unwrap();
    let path = temp.file("outside.txt", b"base");
    let options = SessionOptions {
        project_root: Some(root.clone()),
        autosave: Some(Duration::ZERO),
        persistent_history: true,
        ..SessionOptions::default()
    };
    let mut session = EditorSession::with_options(options.clone()).unwrap();
    let doc = session.open_file(&path).unwrap();
    let view = session.create_view(doc).unwrap();
    session.with_commands(view, |c| c.type_text(b"X")).unwrap();
    fs::remove_dir(&root).unwrap();
    let report = session.tick();
    assert_eq!(fs::read(&path).unwrap(), b"Xbase");
    assert!(report.errors.iter().any(|error| error.service == "history"));
    fs::create_dir(&root).unwrap();
    assert!(session.tick().errors.is_empty());
    assert!(root.join(".undo-redo-bed.json").is_file());
    drop(session);
    let mut reopened = EditorSession::with_options(options).unwrap();
    let doc = reopened.open_file(&path).unwrap();
    let view = reopened.create_view(doc).unwrap();
    reopened.with_commands(view, |c| c.undo()).unwrap();
    assert_eq!(reopened.snapshot(doc).unwrap().bytes, b"base");
}

#[test]
fn failed_shutdown_save_retains_all_views_and_discard_shutdown_is_idempotent() {
    let temp = Fixture::new();
    let bad = temp.file("bad.txt", b"bad");
    let good = temp.file("good.txt", b"good");
    let mut session = EditorSession::new();
    let a = session.open_file(&bad).unwrap();
    let b = session.open_file(&good).unwrap();
    let va = session.create_view(a).unwrap();
    let vb = session.create_view(b).unwrap();
    session.with_commands(va, |c| c.type_text(b"X")).unwrap();
    session.with_commands(vb, |c| c.type_text(b"Y")).unwrap();
    fs::remove_file(&bad).unwrap();
    assert!(session.shutdown(ClosePolicy::Save).is_err());
    assert_eq!(session.document_for_view(va), Some(a));
    assert_eq!(session.document_for_view(vb), Some(b));
    assert_eq!(fs::read(&good).unwrap(), b"Ygood");
    assert!(!bad.exists());
    session.shutdown(ClosePolicy::Discard).unwrap();
    assert!(session.document_ids().is_empty());
    assert!(session.create_document(b"new").is_err());
    assert!(
        session
            .shutdown(ClosePolicy::Discard)
            .unwrap()
            .events
            .is_empty()
    );
    assert!(!bad.exists());
}

#[test]
fn unwinding_host_command_restores_view_borrows_and_updates_siblings() {
    let mut session = EditorSession::new();
    let doc = session.create_document(b"base").unwrap();
    let a = session.create_view(doc).unwrap();
    let b = session.create_view(doc).unwrap();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = session.with_commands(a, |c| {
            c.type_text(b"X");
            panic!("host callback failed");
        });
    }));
    assert!(result.is_err());
    assert_eq!(session.snapshot(doc).unwrap().bytes, b"Xbase");
    assert_eq!(session.view_snapshot(a).unwrap().column, 1);
    assert_eq!(session.view_snapshot(b).unwrap().column, 1);
    session.with_commands(b, |c| c.undo()).unwrap();
    assert_eq!(session.snapshot(doc).unwrap().bytes, b"base");
}

#[test]
fn scoped_view_unwind_restores_state_and_publishes_sibling_edits() {
    let mut session = EditorSession::new();
    let doc = session.create_document(b"base").unwrap();
    let a = session.create_view(doc).unwrap();
    let b = session.create_view(doc).unwrap();
    session.take_events();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = session.with_view(a, |view| {
            view.view_mut().set_scroll_position([12.0, 48.0]);
            view.commands().type_text(b"X");
            panic!("view callback failed");
        });
    }));
    assert!(result.is_err());
    assert_eq!(session.snapshot(doc).unwrap().bytes, b"Xbase");
    assert_eq!(
        session.view_snapshot(a).unwrap().scroll_position,
        [12.0, 48.0]
    );
    assert_eq!(session.view_snapshot(b).unwrap().column, 1);
    assert!(
        session
            .take_events()
            .iter()
            .any(|event| matches!(event,SessionEvent::Edited {view:Some(id),..} if *id==a))
    );
    session.with_view(b, |view| view.commands().undo()).unwrap();
    assert_eq!(session.snapshot(doc).unwrap().bytes, b"base");
}
