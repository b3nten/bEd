#![cfg(all(feature = "editor", feature = "workspaces"))]

#[path = "support/editor_host.rs"]
mod editor_host;
#[path = "support/temp_dir.rs"]
mod temp_dir;

use bed_api::{documents::PanelBinding, workbench::PanelTarget};
use bed_app::*;
use bed_app_config::{LayoutSlot, Settings};
use bed_documents::{ByteEdit, DocumentSession};
use bevy::prelude::*;
use serde_json::json;
use temp_dir::TempDir;

fn app(dir: &TempDir, options: AppOptions) -> App {
    let mut app = App::new();
    editor_host::install(
        &mut app,
        Settings::with_paths(
            dir.path("config"),
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")),
        )
        .unwrap(),
    )
    .unwrap();
    app.add_plugins((
        bed_runtime::BedRuntimePlugin,
        AppRuntimePlugin {
            options: AppOptions {
                working_directory: Some(dir.root().to_owned()),
                initial_layout: Some(LayoutSlot::Startup),
                ..options
            },
        },
        bed_plugin_editor::EditorPlugin,
        bed_plugin_workspaces::WorkspacesPlugin,
    ));
    app.update();
    assert!(app.world().resource::<WorkspaceState>().error.is_none());
    app
}

fn command(app: &mut App, command: WindowCommand) {
    let request = capture_app_request(
        app.world(),
        AppAction::Command(command),
        PanelTarget::LastFocused,
    );
    let id = submit_app_request(app.world_mut(), request);
    process_app_requests(app.world_mut()).unwrap();
    poll_app_request(app.world_mut(), id).unwrap().unwrap();
}

#[test]
fn startup_templates_include_workspaces_and_new_windows_ignore_previous_sessions() {
    let dir = TempDir::new();
    let mut original = app(&dir, Default::default());
    let picker = active_panel_entities(original.world_mut())[0];
    assert_eq!(
        original
            .world()
            .get::<PanelRecord>(picker)
            .unwrap()
            .panel_type,
        bed_plugin_workspaces::PANEL_ID
    );
    let surface = panel_presentation(original.world(), picker).unwrap();
    assert_eq!(
        bed_workbench::panel_title(original.world_mut(), surface).unwrap(),
        "Workspaces"
    );
    command(&mut original, WindowCommand::NewDocument);
    command(&mut original, WindowCommand::SplitDown);
    command(&mut original, WindowCommand::SetCurrentLayout);
    let template = original
        .world()
        .non_send::<Settings>()
        .layout_default(LayoutSlot::Startup)
        .unwrap()
        .clone();
    assert!(
        template["panels"]
            .as_array()
            .unwrap()
            .iter()
            .any(|panel| panel["panel_type"] == bed_plugin_workspaces::PANEL_ID)
    );
    assert!(
        original
            .world()
            .non_send::<Settings>()
            .layout_default(LayoutSlot::Layout1)
            .is_none()
    );
    command(
        &mut original,
        WindowCommand::SetLayoutDefault(LayoutSlot::Layout9),
    );
    assert_eq!(
        original
            .world()
            .non_send::<Settings>()
            .layout_default(LayoutSlot::Layout9),
        Some(&template)
    );
    command(&mut original, WindowCommand::UseLayout(LayoutSlot::Layout4));
    persist_workspace(original.world_mut()).unwrap();
    drop(original);

    let path = dir.write("note.txt", b"note").canonicalize().unwrap();
    let mut fresh = app(
        &dir,
        AppOptions {
            files: vec![OpenFileOptions {
                path: path.clone(),
                viewer: None,
                additional: false,
            }],
            ..Default::default()
        },
    );
    assert_eq!(
        fresh.world().resource::<LayoutSlots>().active(),
        LayoutSlot::Startup
    );
    let panels = active_panel_entities(fresh.world_mut());
    assert!(panels.iter().any(
        |panel| fresh.world().get::<PanelRecord>(*panel).unwrap().panel_type
            == bed_plugin_workspaces::PANEL_ID
    ));
    assert_eq!(
        fresh
            .world()
            .resource::<WorkbenchLayout>()
            .tiling
            .layout
            .areas
            .len(),
        2
    );
    let focused = fresh.world().resource::<WorkbenchLayout>().focused.unwrap();
    let document = fresh
        .world()
        .get::<PanelBinding>(focused)
        .unwrap()
        .document
        .unwrap();
    assert_eq!(
        fresh
            .world()
            .non_send::<DocumentSession>()
            .snapshot(document)
            .unwrap()
            .path,
        path.to_string_lossy()
    );
    command(
        &mut fresh,
        WindowCommand::RestoreBuiltinLayout(LayoutSlot::Startup),
    );
    assert!(
        fresh
            .world()
            .non_send::<Settings>()
            .layout_default(LayoutSlot::Startup)
            .is_none()
    );
    assert_eq!(active_panel_entities(fresh.world_mut()), panels);
    command(&mut fresh, WindowCommand::ResetLayout);
    assert_eq!(
        fresh
            .world()
            .resource::<WorkbenchLayout>()
            .tiling
            .layout
            .areas
            .len(),
        1
    );
    assert!(fresh.world().entities().contains(focused));
    let mut builtin = app(&dir, Default::default());
    assert_eq!(active_panel_entities(builtin.world_mut()).len(), 1);
    let panel = active_panel_entities(builtin.world_mut())[0];
    assert_eq!(
        builtin
            .world()
            .get::<PanelRecord>(panel)
            .unwrap()
            .panel_type,
        bed_plugin_workspaces::PANEL_ID
    );
}

#[test]
fn startup_switches_preserve_dirty_documents_and_attachment_adopts_workspace_selection() {
    let dir = TempDir::new();
    let mut app = app(&dir, Default::default());
    let picker = active_panel_entities(app.world_mut())[0];
    command(&mut app, WindowCommand::NewDocument);
    let panel = app.world().resource::<WorkbenchLayout>().focused.unwrap();
    let document = app
        .world()
        .get::<PanelBinding>(panel)
        .unwrap()
        .document
        .unwrap();
    let revision = app
        .world()
        .non_send::<DocumentSession>()
        .document_revision(document)
        .unwrap();
    app.world_mut()
        .non_send_mut::<DocumentSession>()
        .apply_edits(
            document,
            revision,
            &[ByteEdit {
                range: 0..0,
                bytes: b"retained".to_vec(),
            }],
        )
        .unwrap();
    command(&mut app, WindowCommand::UseLayout(LayoutSlot::Layout4));
    let numbered = app.world().resource::<WorkbenchLayout>().focused.unwrap();
    command(&mut app, WindowCommand::UseLayout(LayoutSlot::Startup));
    assert_eq!(
        app.world().resource::<WorkbenchLayout>().focused,
        Some(panel)
    );
    assert!(bed_workbench::api::panel_active(app.world(), picker));
    let root = dir.path("project");
    std::fs::create_dir(&root).unwrap();
    let root = root.canonicalize().unwrap();
    let spec = WorkspaceSpec::local(root.to_string_lossy());
    let empty = json!({"panels":[],"tiling":bed_workbench::workspace::tiling_state::TilingState::default().to_value([])});
    WorkspaceStore::load(&dir.path("config"))
        .unwrap()
        .save_session_layout(
            Some(&spec),
            json!({
                "active_layout_slot":4,"layouts":{"4":empty}
            }),
        )
        .unwrap();
    let session = app.world().non_send::<DocumentSession>().session_id();
    attach_workspace(app.world_mut(), &root).unwrap();
    assert_eq!(
        app.world().resource::<LayoutSlots>().active(),
        LayoutSlot::Layout4
    );
    assert_eq!(
        app.world().non_send::<DocumentSession>().session_id(),
        session
    );
    assert!(!app.world().entities().contains(picker));
    assert!(bed_workbench::api::panel_active(app.world(), panel));
    assert!(bed_workbench::api::panel_active(app.world(), numbered));
    assert_eq!(
        app.world()
            .non_send::<DocumentSession>()
            .snapshot(document)
            .unwrap()
            .bytes,
        b"retained"
    );
    assert!(
        app.world()
            .non_send::<DocumentSession>()
            .snapshot(document)
            .unwrap()
            .dirty
    );
    persist_workspace(app.world_mut()).unwrap();
    drop(app);

    let mut restored = self::app(
        &dir,
        AppOptions {
            workspace: Some(spec),
            ..Default::default()
        },
    );
    assert_eq!(
        restored.world().resource::<LayoutSlots>().active(),
        LayoutSlot::Layout4
    );
    assert!(
        active_panel_entities(restored.world_mut())
            .iter()
            .all(|panel| restored
                .world()
                .get::<PanelRecord>(*panel)
                .unwrap()
                .panel_type
                != bed_plugin_workspaces::PANEL_ID)
    );
}
