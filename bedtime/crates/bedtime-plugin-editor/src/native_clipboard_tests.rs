use super::*;
use bedtime_api::{
    documents::PanelBinding,
    workbench::{ClipboardAction, ClipboardInvocation, PanelClipboard},
};
use bevy::prelude::*;

#[test]
fn native_clipboard_without_text_preserves_selected_document_contents() {
    let mut app = App::new();
    app.insert_non_send(DocumentSession::new());
    app.insert_non_send(EditorConfig::default());
    #[cfg(feature = "language-services")]
    app.insert_non_send(EditorRuntime::default());
    app.add_plugins((bedtime_workbench::WorkbenchPlugin, EditorPlugin));
    let panel = bedtime_workbench::open_panel(
        app.world_mut(),
        json!({"panel_type": TEXT_PANEL_TYPE,"state":null}),
        None,
        None,
    )
    .unwrap();
    let binding = app.world().get::<PanelBinding>(panel).unwrap().clone();
    let document = binding.document.unwrap();
    let view = binding.view.unwrap();
    {
        let mut session = app.world_mut().non_send_mut::<DocumentSession>();
        session
            .with_commands(view, |commands| {
                commands.type_text(b"selected text");
                commands.select_all();
            })
            .unwrap();
    }
    let clipboard = app.world().get::<PanelClipboard>(panel).unwrap().0;
    app.world_mut()
        .run_system_with(
            clipboard,
            ClipboardInvocation {
                entity: panel,
                action: ClipboardAction::PasteFromClipboard(None),
            },
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        app.world()
            .non_send::<DocumentSession>()
            .snapshot(document)
            .unwrap()
            .bytes,
        b"selected text"
    );
    app.world_mut()
        .run_system_with(
            clipboard,
            ClipboardInvocation {
                entity: panel,
                action: ClipboardAction::PasteFromClipboard(Some("replacement".into())),
            },
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        app.world()
            .non_send::<DocumentSession>()
            .snapshot(document)
            .unwrap()
            .bytes,
        b"replacement"
    );
}
