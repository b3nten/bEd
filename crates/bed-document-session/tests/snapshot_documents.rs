use bed_document_session::{EditorSession, editor_session::ByteEdit};

#[test]
fn comparisons_support_navigation_but_reject_commands_transactions_and_saves() {
    let mut session = EditorSession::new();
    let document = session
        .create_snapshot_document(b"first\nsecond", "rust")
        .unwrap();
    let view = session.create_view(document).unwrap();
    let revision = session.document_revision(document).unwrap();
    session
        .with_commands(view, |commands| {
            commands.select_all();
            assert_eq!(commands.copy(), b"first\nsecond");
            commands.type_text(b"replacement");
            commands.cut();
            commands.paste(b"paste");
            commands.delete_left(false);
            commands.delete_right(false);
            commands.insert_newline();
            commands.indent();
            commands.outdent();
            commands.undo();
            commands.redo();
        })
        .unwrap();
    assert_eq!(session.snapshot(document).unwrap().bytes, b"first\nsecond");
    assert_eq!(session.document_revision(document).unwrap(), revision);
    assert!(
        session
            .apply_edits(
                document,
                revision,
                &[ByteEdit {
                    range: 0..1,
                    bytes: b"x".to_vec()
                }]
            )
            .is_err()
    );
    assert!(session.save(document).is_err());
    assert!(
        session
            .save_as(
                document,
                std::path::Path::new("comparison-must-not-be-written.rs")
            )
            .is_err()
    );
    assert!(session.replace_content(document, b"changed").is_err());
    let mut immutable = session.with_document(document, Clone::clone).unwrap();
    let mut operations = bed_editing::editor_operations::EditorOperations::new();
    assert!(
        operations
            .splice_bytes(&mut immutable, 0..1, b"x")
            .is_none()
    );
    assert_eq!(immutable.join(), b"first\nsecond");
    assert!(!immutable.dirty);
    assert!(!session.snapshot(document).unwrap().dirty);
    assert_eq!(session.snapshot(document).unwrap().language_id, "rust");
}

#[test]
fn replacing_comparison_updates_every_view_without_changing_live_document_history() {
    let mut session = EditorSession::new();
    let live = session.create_document(b"live").unwrap();
    let live_view = session.create_view(live).unwrap();
    session
        .with_commands(live_view, |commands| commands.type_text(b"edited "))
        .unwrap();
    let comparison = session.create_snapshot_document(b"old", "text").unwrap();
    let left = session.create_view(comparison).unwrap();
    let right = session.create_view(comparison).unwrap();
    session
        .replace_snapshot_document(comparison, b"new\ncontent")
        .unwrap();
    assert!(session.tick().events.iter().any(|event| matches!(event,
        bed_document_session::SessionEvent::Reloaded { document, .. } if *document == comparison)));
    assert_eq!(session.document_for_view(left), Some(comparison));
    assert_eq!(session.document_for_view(right), Some(comparison));
    assert_eq!(session.snapshot(comparison).unwrap().bytes, b"new\ncontent");
    assert!(session.is_snapshot_document(comparison));
    session.undo_document(live).unwrap();
    assert_eq!(session.snapshot(live).unwrap().bytes, b"live");
    assert_eq!(session.snapshot(comparison).unwrap().bytes, b"new\ncontent");
}
