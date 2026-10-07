//! Document-level transactions are shared by native extensions and core editors.
use bed_core::editor_commands::CursorReveal;
use bed_session::{
    ByteEdit, ClosePolicy, DocumentKind, EditorSession, SessionEvent, SessionOptions,
};
use std::{fs, io, time::Duration};
#[path = "../../../tests/support/temp_dir.rs"]
mod temp_dir;
use temp_dir::TempDir;

#[test]
fn exact_byte_document_round_trips_and_groups_multiple_splices_in_memory() {
    let temp = TempDir::new();
    let original = b"\xef\xbb\xbf\0\0\xffa\r\nb\rc\n";
    let path = temp.write("bytes.bin", original);
    let mut session = EditorSession::with_options(SessionOptions {
        project_root: Some(temp.root().to_owned()),
        persistent_history: true,
        highlighting: true,
        git: true,
        ..Default::default()
    })
    .unwrap();
    let doc = session.open_file_auto(&path).unwrap();
    let snapshot = session.snapshot(doc).unwrap();
    assert_eq!(snapshot.kind, DocumentKind::Bytes);
    assert_eq!(snapshot.bytes, original);
    assert!(!snapshot.utf8_bom);
    assert!(snapshot.language_id.is_empty());
    let revision = session.document_revision(doc).unwrap();
    session
        .apply_edits(
            doc,
            revision,
            &[
                ByteEdit {
                    range: 4..7,
                    bytes: vec![0x00, 0x0d, 0xff, 0x0a],
                },
                ByteEdit {
                    range: 10..10,
                    bytes: vec![0x88, 0x99],
                },
            ],
        )
        .unwrap();
    let edited = session.snapshot(doc).unwrap().bytes;
    assert_ne!(edited, original);
    session.undo_document(doc).unwrap();
    assert_eq!(session.snapshot(doc).unwrap().bytes, original);
    session.redo_document(doc).unwrap();
    assert_eq!(session.snapshot(doc).unwrap().bytes, edited);
    session.save(doc).unwrap();
    assert_eq!(fs::read(&path).unwrap(), edited);
    session.tick();
    if let Ok(history) = fs::read(temp.path(".undo-redo-bed.json")) {
        let history: serde_json::Value = serde_json::from_slice(&history).unwrap();
        assert!(history["files"].get(path.to_str().unwrap()).is_none());
    }
    session.close_document(doc, ClosePolicy::Discard).unwrap();
    let reopened = session
        .open_file_with_kind(&path, DocumentKind::Bytes)
        .unwrap();
    session.undo_document(reopened).unwrap();
    assert_eq!(session.snapshot(reopened).unwrap().bytes, edited);
}

#[test]
fn text_transactions_update_sibling_views_and_form_one_separate_undo_unit() {
    let mut session = EditorSession::new();
    let doc = session.create_document(b"abc\ndef").unwrap();
    let first = session.create_view(doc).unwrap();
    let sibling = session.create_view(doc).unwrap();
    session
        .with_commands(first, |commands| {
            commands.set_selection(0, 1, 0, 3, CursorReveal::Ensure)
        })
        .unwrap();
    session
        .with_commands(sibling, |commands| {
            commands.set_cursor(1, 3, false, CursorReveal::Ensure)
        })
        .unwrap();
    session.take_events();
    session
        .apply_edits(
            doc,
            session.document_revision(doc).unwrap(),
            &[
                ByteEdit {
                    range: 0..1,
                    bytes: b"XYZ".to_vec(),
                },
                ByteEdit {
                    range: 5..6,
                    bytes: b"Q".to_vec(),
                },
            ],
        )
        .unwrap();
    assert_eq!(session.snapshot(doc).unwrap().bytes, b"XYZbc\ndQf");
    let owner_state = session.view_snapshot(first).unwrap();
    assert_eq!(
        owner_state.get_ordered(),
        (0, 3, 0, 5),
        "document edits must transform the command owner's selection without replacing it"
    );
    let sibling_state = session.view_snapshot(sibling).unwrap();
    assert_eq!((sibling_state.row, sibling_state.column), (1, 3));
    assert_eq!(
        session
            .take_events()
            .iter()
            .filter(|event| matches!(event, SessionEvent::Edited { .. }))
            .count(),
        1
    );
    session
        .with_commands(first, |commands| commands.undo())
        .unwrap();
    assert_eq!(session.snapshot(doc).unwrap().bytes, b"abc\ndef");
    session.redo_document(doc).unwrap();
    assert_eq!(session.snapshot(doc).unwrap().bytes, b"XYZbc\ndQf");
}

#[test]
fn stale_or_invalid_transactions_do_not_partially_edit_either_document_kind() {
    for kind in [DocumentKind::Text, DocumentKind::Bytes] {
        let mut session = EditorSession::new();
        let doc = session.create_document_with_kind(b"abcdef", kind).unwrap();
        let revision = session.document_revision(doc).unwrap();
        let invalid = [
            ByteEdit {
                range: 0..1,
                bytes: b"x".to_vec(),
            },
            ByteEdit {
                range: 5..99,
                bytes: b"y".to_vec(),
            },
        ];
        assert_eq!(
            session
                .apply_edits(doc, revision, &invalid)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(session.document_revision(doc).unwrap(), revision);
        assert!(!session.snapshot(doc).unwrap().dirty);
        let overlap = [
            ByteEdit {
                range: 1..4,
                bytes: Vec::new(),
            },
            ByteEdit {
                range: 3..5,
                bytes: Vec::new(),
            },
        ];
        assert!(session.apply_edits(doc, revision, &overlap).is_err());
        session
            .apply_edits(
                doc,
                revision,
                &[ByteEdit {
                    range: 0..1,
                    bytes: b"X".to_vec(),
                }],
            )
            .unwrap();
        assert_eq!(
            session
                .apply_edits(
                    doc,
                    revision,
                    &[ByteEdit {
                        range: 1..2,
                        bytes: b"Y".to_vec()
                    }]
                )
                .unwrap_err()
                .kind(),
            io::ErrorKind::WouldBlock
        );
        assert_eq!(session.snapshot(doc).unwrap().bytes, b"Xbcdef");
    }
}

#[test]
fn text_loading_and_line_ending_validation_stay_distinct_from_exact_bytes() {
    let temp = TempDir::new();
    let binary = temp.write("binary", &[0; 128]);
    let mut session = EditorSession::new();
    assert!(session.open_file(&binary).is_err());
    let bytes = session
        .open_file_with_kind(&binary, DocumentKind::Bytes)
        .unwrap();
    assert_eq!(
        session.open_file(&binary).unwrap_err().kind(),
        io::ErrorKind::AlreadyExists
    );
    let view = session.create_view(bytes).unwrap();
    assert!(
        session
            .with_commands(view, |commands| commands.type_text(b"x"))
            .is_err()
    );
    let doc = session.create_document(b"a\r\nb").unwrap();
    let revision = session.document_revision(doc).unwrap();
    assert!(
        session
            .apply_edits(
                doc,
                revision,
                &[ByteEdit {
                    range: 2..2,
                    bytes: b"x".to_vec()
                }]
            )
            .is_err()
    );
    assert_eq!(session.snapshot(doc).unwrap().bytes, b"a\r\nb");
    session
        .apply_edits(
            doc,
            revision,
            &[ByteEdit {
                range: 0..0,
                bytes: b"c\n".to_vec(),
            }],
        )
        .unwrap();
    assert_eq!(session.snapshot(doc).unwrap().bytes, b"c\r\na\r\nb");
    session.undo_document(doc).unwrap();
    assert_eq!(session.snapshot(doc).unwrap().bytes, b"a\r\nb");
}

#[test]
fn binary_autosave_save_as_reload_and_conflicts_use_the_shared_lifecycle() {
    let temp = TempDir::new();
    let original = b"\0\xff\r\n\xef\xbb\xbf\n\r";
    let path = temp.write("raw.bin", original);
    let mut session = EditorSession::with_options(SessionOptions {
        autosave: Some(Duration::ZERO),
        monitoring: true,
        ..Default::default()
    })
    .unwrap();
    let doc = session
        .open_file_with_kind(&path, DocumentKind::Bytes)
        .unwrap();
    session
        .apply_edits(
            doc,
            session.document_revision(doc).unwrap(),
            &[ByteEdit {
                range: 1..2,
                bytes: vec![0x88],
            }],
        )
        .unwrap();
    session.tick();
    let edited = session.snapshot(doc).unwrap().bytes;
    assert_eq!(fs::read(&path).unwrap(), edited);
    assert!(!session.snapshot(doc).unwrap().dirty);
    let destination = temp.path("copy.bin");
    session.save_as(doc, &destination).unwrap();
    assert_eq!(fs::read(&destination).unwrap(), edited);
    fs::write(&destination, original).unwrap();
    session.reload_from_disk(doc).unwrap();
    assert_eq!(session.snapshot(doc).unwrap().bytes, original);
    session.undo_document(doc).unwrap();
    assert_eq!(session.snapshot(doc).unwrap().bytes, original);
    session
        .apply_edits(
            doc,
            session.document_revision(doc).unwrap(),
            &[ByteEdit {
                range: 0..0,
                bytes: vec![1],
            }],
        )
        .unwrap();
    fs::write(&destination, b"\0external\xff").unwrap();
    std::thread::sleep(Duration::from_millis(510));
    session.tick();
    assert!(session.snapshot(doc).unwrap().disk_conflict.is_some());
    assert!(session.save(doc).is_err());
    session.keep_buffer(doc).unwrap();
    session.save(doc).unwrap();
    assert_eq!(
        fs::read(destination).unwrap(),
        session.snapshot(doc).unwrap().bytes
    );
}

#[test]
fn byte_positions_follow_shared_insert_delete_undo_redo_and_replacement() {
    let mut session = EditorSession::new();
    let document = session
        .create_document_with_kind(b"abcde", DocumentKind::Bytes)
        .unwrap();
    let initial = session.document_revision(document).unwrap();
    session
        .apply_edits(
            document,
            initial,
            &[ByteEdit {
                range: 1..1,
                bytes: b"XY".to_vec(),
            }],
        )
        .unwrap();
    let after_insert = session.document_revision(document).unwrap();
    let mut positions = [0, 1, 2, 5];
    assert!(
        session
            .transform_byte_offsets(document, initial, &mut positions)
            .unwrap()
    );
    assert_eq!(positions, [0, 3, 4, 7]);
    session
        .apply_edits(
            document,
            after_insert,
            &[ByteEdit {
                range: 0..2,
                bytes: Vec::new(),
            }],
        )
        .unwrap();
    let after_delete = session.document_revision(document).unwrap();
    session
        .transform_byte_offsets(document, after_insert, &mut positions)
        .unwrap();
    assert_eq!(positions, [0, 1, 2, 5]);
    session.undo_document(document).unwrap();
    session
        .transform_byte_offsets(document, after_delete, &mut positions)
        .unwrap();
    assert_eq!(positions, [2, 3, 4, 7]);
    let before_replace = session.document_revision(document).unwrap();
    session.replace_content(document, b"z").unwrap();
    assert!(
        !session
            .transform_byte_offsets(document, before_replace, &mut positions)
            .unwrap()
    );
    assert_eq!(positions, [1, 1, 1, 1]);
}

#[test]
fn byte_edits_invalidate_prior_text_history_in_memory_and_after_restart() {
    let temp = TempDir::new();
    let path = temp.write("mode-switch.txt", b"abc");
    let options = SessionOptions {
        project_root: Some(temp.root().to_owned()),
        persistent_history: true,
        ..Default::default()
    };
    let mut session = EditorSession::with_options(options.clone()).unwrap();
    let text = session.open_file(&path).unwrap();
    let view = session.create_view(text).unwrap();
    session
        .with_commands(view, |commands| commands.type_text(b"saved "))
        .unwrap();
    session.save(text).unwrap();
    session.close_document(text, ClosePolicy::Discard).unwrap();
    let bytes = session
        .open_file_with_kind(&path, DocumentKind::Bytes)
        .unwrap();
    session
        .apply_edits(
            bytes,
            session.document_revision(bytes).unwrap(),
            &[ByteEdit {
                range: 0..5,
                bytes: b"edited".to_vec(),
            }],
        )
        .unwrap();
    session.save(bytes).unwrap();
    session.close_document(bytes, ClosePolicy::Discard).unwrap();
    let text = session.open_file(&path).unwrap();
    session.undo_document(text).unwrap();
    assert_eq!(session.snapshot(text).unwrap().bytes, b"edited abc");
    assert!(!session.snapshot(text).unwrap().dirty);
    session.shutdown(ClosePolicy::Discard).unwrap();
    let mut restarted = EditorSession::with_options(options).unwrap();
    let text = restarted.open_file(&path).unwrap();
    restarted.undo_document(text).unwrap();
    restarted.redo_document(text).unwrap();
    assert_eq!(restarted.snapshot(text).unwrap().bytes, b"edited abc");
    assert!(!restarted.snapshot(text).unwrap().dirty);
}

#[test]
fn unchanged_byte_view_and_reload_preserve_existing_text_undo() {
    let temp = TempDir::new();
    let path = temp.write("read-only-view.txt", b"abc");
    let mut session = EditorSession::new();
    let text = session.open_file(&path).unwrap();
    let view = session.create_view(text).unwrap();
    session
        .with_commands(view, |commands| commands.type_text(b"saved "))
        .unwrap();
    session.save(text).unwrap();
    session.close_document(text, ClosePolicy::Discard).unwrap();
    let bytes = session
        .open_file_with_kind(&path, DocumentKind::Bytes)
        .unwrap();
    session.reload_from_disk(bytes).unwrap();
    session.close_document(bytes, ClosePolicy::Discard).unwrap();
    let text = session.open_file(&path).unwrap();
    session.undo_document(text).unwrap();
    assert_eq!(session.snapshot(text).unwrap().bytes, b"abc");
}

#[test]
fn byte_save_as_invalidates_destination_text_history_only_after_success() {
    for succeeds in [false, true] {
        let temp = TempDir::new();
        let target = temp.write("destination.txt", b"target");
        let mut session = EditorSession::with_options(SessionOptions {
            project_root: Some(temp.root().to_owned()),
            persistent_history: true,
            ..Default::default()
        })
        .unwrap();
        let text = session.open_file(&target).unwrap();
        let view = session.create_view(text).unwrap();
        session
            .with_commands(view, |commands| commands.type_text(b"old "))
            .unwrap();
        session.save(text).unwrap();
        session.close_document(text, ClosePolicy::Discard).unwrap();
        let content = if succeeds {
            b"replacement".to_vec()
        } else {
            vec![0; bed_files::files::MAX_FILE_SIZE + 1]
        };
        let bytes = session
            .create_document_with_kind(&content, DocumentKind::Bytes)
            .unwrap();
        assert_eq!(session.save_as(bytes, &target).is_ok(), succeeds);
        session.close_document(bytes, ClosePolicy::Discard).unwrap();
        let text = session.open_file(&target).unwrap();
        session.undo_document(text).unwrap();
        assert_eq!(
            session.snapshot(text).unwrap().bytes,
            if succeeds {
                b"replacement".as_slice()
            } else {
                b"target".as_slice()
            }
        );
    }
}
