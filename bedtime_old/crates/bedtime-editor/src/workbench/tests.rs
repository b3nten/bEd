use super::*;

fn host() -> (App, DocumentHandle<TextBuffer>, Entity, Entity) {
    let mut app = App::new();
    app.add_plugins(WorkbenchCorePlugin);
    let _workspace = crate::test_layout(app.world_mut());
    let document = open_memory_document(
        app.world_mut(),
        TextBuffer::from("original"),
        DocumentOptions::loaded("Text"),
    )
    .unwrap();
    let mut workbench = Workbench::new(app.world_mut());
    let layout = workbench.active_layout();
    let first = workbench.create_panel(layout, "First").unwrap();
    let second = workbench.create_panel(layout, "Second").unwrap();
    for panel in [first, second] {
        workbench.world_mut().entity_mut(panel).insert((
            WorkbenchEditor::default(),
            DocumentReference::new(document.id()),
        ));
    }
    (app, document, first, second)
}

#[test]
fn adapters_share_canonical_history_and_keep_navigation_per_view() {
    let (mut app, document, first, second) = host();
    let mut group = None;
    let mut session = WorkbenchTextSession {
        world: app.world_mut(),
        handle: document,
        panel: first,
        group: &mut group,
    };
    session
        .edit(
            0,
            TextEdit {
                range: 0..0,
                replacement: "é".into(),
            },
        )
        .unwrap();
    assert_eq!(session.snapshot().unwrap().text, "éoriginal");
    let mut other_group = None;
    let mut other = WorkbenchTextSession {
        world: app.world_mut(),
        handle: document,
        panel: second,
        group: &mut other_group,
    };
    other.history(HistoryAction::Undo).unwrap();
    assert_eq!(other.snapshot().unwrap().text, "original");
    other.history(HistoryAction::Redo).unwrap();
    assert_eq!(other.snapshot().unwrap().text, "éoriginal");
    let revision = other.snapshot().unwrap().revision;
    navigate_editor(
        app.world_mut(),
        second,
        Navigation {
            revision,
            range: 2..2,
            selection: Some(TextSelection::caret(&TextBuffer::from("éoriginal"), 2).unwrap()),
            reveal: Reveal::Center,
            focus: true,
        },
    )
    .unwrap();
    assert!(
        app.world()
            .get::<WorkbenchEditor>(first)
            .unwrap()
            .view
            .navigation
            .is_none()
    );
    assert!(
        app.world()
            .get::<WorkbenchEditor>(second)
            .unwrap()
            .view
            .navigation
            .is_some()
    );
    assert!(
        navigate_editor(
            app.world_mut(),
            first,
            Navigation {
                revision: 0,
                range: 0..0,
                selection: None,
                reveal: Reveal::Minimal,
                focus: false
            }
        )
        .is_err()
    );
}

#[test]
fn composition_blocks_save_history_rebind_and_retirement_until_cleared() {
    let (mut app, document, first, second) = host();
    let mut group = None;
    let mut session = WorkbenchTextSession {
        world: app.world_mut(),
        handle: document,
        panel: first,
        group: &mut group,
    };
    session.set_composing(egui::Id::new("first"), true);
    assert!(
        inspect_document(app.world(), document.id())
            .unwrap()
            .has_drafts
    );
    assert_eq!(
        undo_document(app.world_mut(), document.id()),
        Err(DocumentError::Drafts)
    );
    assert_eq!(
        save_document(app.world_mut(), document.id()),
        Err(DocumentError::Drafts)
    );
    assert!(preflight_session_close(app.world(), SessionCloseMode::Discard).is_err());
    let mut other_group = None;
    let mut other = WorkbenchTextSession {
        world: app.world_mut(),
        handle: document,
        panel: second,
        group: &mut other_group,
    };
    other.set_composing(egui::Id::new("second"), false);
    assert!(
        other
            .edit(
                0,
                TextEdit {
                    range: 0..0,
                    replacement: "no".into()
                }
            )
            .is_err()
    );
    assert_eq!(read_document(app.world(), document).unwrap(), "original");
    app.world_mut().entity_mut(first).remove::<DocumentDraft>();
    app.world_mut()
        .entity_mut(first)
        .remove::<(WorkbenchEditor, DocumentReference)>();
    assert_eq!(
        app.world()
            .get::<DocumentReference>(second)
            .unwrap()
            .document(),
        document.id()
    );
    assert_eq!(read_document(app.world(), document).unwrap(), "original");
}
