#![cfg(all(feature = "editor", feature = "file-explorer", feature = "workspaces"))]

#[path = "support/editor_host.rs"]
mod editor_host;

use bed::application::{
    AppOptions, AppRuntimePlugin, RemoteHelperDirectory, WorkspacePersistence, WorkspaceSpec,
    WorkspaceState, WorkspaceStore,
};
use bed::config::Settings;
use bed_api::features::{HostAction, HostEffects};
use bed_plugin_workspaces::WorkspaceAction;
use bevy::prelude::*;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, Instant},
};

static IMGUI_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "bed-bevy-regressions-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path.canonicalize().unwrap())
    }

    fn app(&self) -> App {
        let config = self.0.join("config");
        let settings =
            Settings::with_paths(config.clone(), PathBuf::from(env!("CARGO_MANIFEST_DIR")))
                .unwrap();
        let mut app = App::new();
        app.insert_resource(
            bed_api::launch::ApplicationPaths::new("bed", Some(config.clone()), None).unwrap(),
        )
        .insert_non_send(settings)
        .insert_non_send(bed_ui::icons::Icons::default())
        .insert_non_send(WorkspacePersistence::new(
            WorkspaceStore::load(
                &bed_api::launch::ApplicationPaths::new("bed", Some(config.clone()), None).unwrap(),
            )
            .unwrap(),
        ))
        .insert_resource(RemoteHelperDirectory(PathBuf::new()))
        .add_plugins((
            (bevy::MinimalPlugins, bevy::input::InputPlugin),
            AppRuntimePlugin {
                options: AppOptions {
                    working_directory: Some(self.0.clone()),
                    startup_panel: Some(bed_plugin_workspaces::PANEL_ID.into()),
                    ..Default::default()
                },
            },
            bed_plugin_editor::EditorPlugin,
            bed_plugin_file_explorer::FileExplorerPlugin,
            bed_plugin_workspaces::WorkspacesPlugin,
        ));
        app.update();
        app
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn tree_files_drop_into_populated_and_empty_areas_and_move_existing_tabs() {
    use bed_api::documents::PanelBinding;
    use bed_documents::DocumentSession;
    use dear_imgui_rs::{Context, MouseButton};
    use {
        bed_workbench::PanelPresentation, bed_workbench::WorkbenchLayout,
        bed_workbench::workspace::tiling::Layout,
        bed_workbench::workspace::tiling_state::TilingState,
    };

    let _lock = IMGUI_TEST_LOCK.lock().unwrap();
    let fixture = Fixture::new();
    let root = fixture.0.join("project");
    std::fs::create_dir(&root).unwrap();
    let source = root.join("source.txt");
    let target = root.join("target.txt");
    std::fs::write(&source, "dragged file\n").unwrap();
    std::fs::write(&target, "target panel\n").unwrap();
    let mut app = fixture.app();
    bed_plugin_workspaces::request_workspace(
        app.world_mut(),
        WorkspaceAction::OpenLocalWorkspace(root.clone()),
        None,
        0,
    );
    app.update();
    app.world_mut().resource_mut::<HostEffects>().push(
        None,
        HostAction::OpenPanel {
            panel_type: bed_plugin_file_explorer::PANEL_ID.into(),
            document: None,
            state: serde_json::Value::Null,
        },
    );
    app.update();
    let files_front = *app
        .world()
        .non_send::<bed_plugin_file_explorer::ExplorerPanels>()
        .0
        .keys()
        .last()
        .unwrap();
    let files_panel = bed_workbench::api::panel_owner(app.world(), files_front).unwrap();
    let target_panel =
        bed::application::open_file(app.world_mut(), &target, None, false, None).unwrap();
    let mut layout = Layout::default();
    let right = layout.split(1, 0, 4000, 1).unwrap();
    let empty = layout.split(right, 1, 5000, 1).unwrap();
    let mut tiling = TilingState::new(layout);
    let panels = app
        .world_mut()
        .query::<(Entity, &PanelPresentation)>()
        .iter(app.world())
        .map(|(entity, presentation)| (entity, presentation.id))
        .collect::<Vec<_>>();
    for (panel, id) in panels {
        tiling.assign(id, if panel == target_panel { right } else { 1 });
        app.world_mut()
            .get_mut::<PanelPresentation>(panel)
            .unwrap()
            .dock_next = true;
    }
    tiling
        .areas
        .iter_mut()
        .find(|area| area.area == 1)
        .unwrap()
        .selected = Some(
        app.world()
            .get::<PanelPresentation>(files_panel)
            .unwrap()
            .id,
    );
    app.world_mut().resource_mut::<WorkbenchLayout>().tiling = tiling;
    bed_workbench::focus_panel(app.world_mut(), files_panel);
    app.world_mut()
        .write_message(bed_api::settings::SettingsRequest::new(
            "bed.ui",
            "ui_animations",
            false,
        ));
    app.update();
    app.world_mut()
        .resource_mut::<bed_plugin_file_explorer::ExplorerPresentation>()
        .tree
        .animations = false;
    app.world_mut()
        .non_send_mut::<bed_plugin_file_explorer::ExplorerState>()
        .file_explorer
        .file_tree
        .preferences
        .hide_hidden = true;
    let mut context = Context::create();
    editor_host::prepare_ui(app.world_mut(), &mut context).unwrap();
    context.set_ini_filename(None::<PathBuf>).unwrap();
    context
        .io_mut()
        .set_config_flags(dear_imgui_rs::ConfigFlags::DOCKING_ENABLE);
    context.io_mut().set_display_size([1200., 800.]);
    context.io_mut().set_delta_time(1. / 60.);
    context
        .font_atlas()
        .try_claim_legacy_renderer()
        .unwrap()
        .build();
    let files_id = app
        .world()
        .get::<PanelPresentation>(files_panel)
        .unwrap()
        .id;
    let frame = |app: &mut App, context: &mut Context| {
        app.update();
        let ui = context.frame();
        editor_host::draw_ui(app.world_mut(), ui).unwrap();
        let mut rows = Vec::new();
        ui.window(format!("Files###bed_tab_{files_id}")).build(|| {
            rows = app
                .world()
                .non_send::<bed_plugin_file_explorer::ExplorerState>()
                .explorer()
                .file_tree
                .drop_targets(ui);
        });
        drop(context.render_legacy());
        bed::application::process_app_requests(app.world_mut()).unwrap();
        rows
    };
    let deadline = Instant::now() + Duration::from_secs(20);
    let rows = loop {
        let rows = frame(&mut app, &mut context);
        if rows.len() == 3 {
            break rows;
        }
        assert!(
            Instant::now() < deadline,
            "tree rows never appeared: {rows:?}"
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    for _ in 0..6 {
        frame(&mut app, &mut context);
    }
    let row = &rows[1]; // Root, source.txt, target.txt (hidden history directory omitted).
    let point = [row.min[0] + 30., (row.min[1] + row.max[1]) * 0.5];
    for area in [right, empty] {
        bed_workbench::focus_panel(app.world_mut(), files_panel);
        context.io_mut().add_mouse_pos_event(point);
        frame(&mut app, &mut context);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, true);
        frame(&mut app, &mut context);
        let (min, max) = bed_workbench::area_bounds(app.world(), area).unwrap();
        context
            .io_mut()
            .add_mouse_pos_event([(min[0] + max[0]) * 0.5, (min[1] + max[1]) * 0.5]);
        for _ in 0..3 {
            frame(&mut app, &mut context);
        }
        assert_eq!(
            app.world()
                .non_send::<bed_plugin_file_explorer::ExplorerState>()
                .explorer()
                .file_tree
                .dragged_paths()
                .unwrap(),
            &[source.to_string_lossy().into_owned()]
        );
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, false);
        frame(&mut app, &mut context);
        for _ in 0..6 {
            frame(&mut app, &mut context);
        }
        let document = app
            .world()
            .non_send::<DocumentSession>()
            .document_for_path(&source)
            .expect("drop never opened the file");
        let matching = app
            .world_mut()
            .query::<(Entity, &PanelPresentation, &PanelBinding)>()
            .iter(app.world())
            .filter(|(_, _, binding)| binding.document == Some(document))
            .map(|(entity, presentation, _)| (entity, presentation.id))
            .collect::<Vec<_>>();
        assert_eq!(
            matching.len(),
            1,
            "dropping an open file duplicated its tab"
        );
        assert_eq!(
            app.world()
                .resource::<WorkbenchLayout>()
                .tiling
                .area_for(matching[0].1),
            Some(area)
        );
        assert_eq!(
            app.world().resource::<WorkbenchLayout>().focused,
            Some(matching[0].0)
        );
        assert_eq!(std::fs::read_to_string(&source).unwrap(), "dragged file\n");
        assert!(app.world().resource::<WorkspaceState>().error.is_none());
    }

    // File clicks target the largest area, including a second click after
    // the first open has focused that new editor.
    for name in ["clicked.txt", "clicked-again.txt"] {
        let clicked = root.join(name);
        std::fs::write(&clicked, "clicked file\n").unwrap();
        app.world_mut()
            .non_send_mut::<bed_plugin_file_explorer::ExplorerState>()
            .explorer_mut()
            .file_tree
            .refresh_file_tree(root.to_str().unwrap())
            .unwrap();
        bed::application::focus_panel(app.world_mut(), target_panel, true).unwrap();
        for _ in 0..3 {
            frame(&mut app, &mut context);
        }
        bed::application::focus_panel(app.world_mut(), files_panel, true).unwrap();
        let mut rows = Vec::new();
        for _ in 0..6 {
            rows = frame(&mut app, &mut context);
        }
        let row = &rows[1]; // The newly created file sorts before the other files.
        context
            .io_mut()
            .add_mouse_pos_event([row.min[0] + 30., (row.min[1] + row.max[1]) * 0.5]);
        frame(&mut app, &mut context);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, true);
        frame(&mut app, &mut context);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, false);
        for _ in 0..6 {
            frame(&mut app, &mut context);
        }
        let panel = app.world().resource::<WorkbenchLayout>().focused.unwrap();
        assert_eq!(
            app.world()
                .non_send::<DocumentSession>()
                .snapshot(
                    app.world()
                        .get::<PanelBinding>(panel)
                        .unwrap()
                        .document
                        .unwrap()
                )
                .unwrap()
                .path,
            clicked.to_string_lossy(),
        );
        assert_eq!(
            app.world()
                .resource::<WorkbenchLayout>()
                .tiling
                .area_for(app.world().get::<PanelPresentation>(panel).unwrap().id),
            Some(
                app.world()
                    .resource::<WorkbenchLayout>()
                    .tiling
                    .layout
                    .largest_area()
            )
        );
    }

    // A folder row must retain its narrower filesystem move target rather
    // than letting the panel's viewer target consume the same drop.
    let destination = root.join("destination");
    std::fs::create_dir(&destination).unwrap();
    app.world_mut()
        .non_send_mut::<bed_plugin_file_explorer::ExplorerState>()
        .explorer_mut()
        .file_tree
        .refresh_file_tree(root.to_str().unwrap())
        .unwrap();
    bed_workbench::focus_panel(app.world_mut(), files_panel);
    let mut rows = Vec::new();
    for _ in 0..6 {
        rows = frame(&mut app, &mut context);
    }
    let file_row = rows
        .iter()
        .filter(|row| row.destination == root.to_string_lossy())
        .nth(3)
        .unwrap();
    context.io_mut().add_mouse_pos_event([
        file_row.min[0] + 30.,
        (file_row.min[1] + file_row.max[1]) * 0.5,
    ]);
    frame(&mut app, &mut context);
    context
        .io_mut()
        .add_mouse_button_event(MouseButton::Left, true);
    frame(&mut app, &mut context);
    let folder_row = rows
        .iter()
        .find(|row| row.destination == destination.to_string_lossy())
        .unwrap();
    context.io_mut().add_mouse_pos_event([
        folder_row.min[0] + 30.,
        (folder_row.min[1] + folder_row.max[1]) * 0.5,
    ]);
    for _ in 0..3 {
        frame(&mut app, &mut context);
    }
    context
        .io_mut()
        .add_mouse_button_event(MouseButton::Left, false);
    frame(&mut app, &mut context);
    let moved = destination.join("source.txt");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !moved.is_file() {
        frame(&mut app, &mut context);
        assert!(
            Instant::now() < deadline,
            "folder move was intercepted by the viewer drop target"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!source.exists());
    assert_eq!(std::fs::read_to_string(&moved).unwrap(), "dragged file\n");
    assert!(app.world().resource::<WorkspaceState>().error.is_none());
}

#[test]
fn directory_picker_saves_workspace_and_restores_recent_projects_and_editor_tabs() {
    let fixture = Fixture::new();
    let root = fixture.0.join("project");
    std::fs::create_dir(&root).unwrap();
    let file = root.join("hello.txt");
    std::fs::write(&file, "hello\n").unwrap();
    let mut app = fixture.app();
    assert!(
        app.world()
            .resource::<bed_plugin_workspaces::WorkspacesState>()
            .recent_workspaces
            .is_empty()
    );
    bed_plugin_workspaces::request_workspace(
        app.world_mut(),
        WorkspaceAction::OpenLocalWorkspace(root.clone()),
        None,
        0,
    );
    app.update();
    // Context admission runs after feature preparation; its consumers observe it next frame.
    app.update();
    let spec = WorkspaceSpec::local(root.to_string_lossy());
    assert_eq!(
        app.world().resource::<WorkspaceState>().spec.as_ref(),
        Some(&spec)
    );
    assert_eq!(
        app.world()
            .resource::<bed::application::LayoutSlots>()
            .active(),
        bed::config::LayoutSlot::Layout1
    );
    assert!(root.join(".bed/workspace.json").is_file());
    {
        let explorer = app
            .world()
            .non_send::<bed_plugin_file_explorer::ExplorerState>()
            .explorer();
        assert_eq!(explorer.file_tree.root_node.full_path, spec.root);
        assert_eq!(explorer.file_tree.root_node.name, "project");
        assert!(explorer.file_tree.root_node.is_directory);
        assert!(explorer.file_tree.root_node.is_open);
    }
    app.world_mut()
        .non_send_mut::<bed_plugin_file_explorer::ExplorerState>()
        .toggle_finder();
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        app.update();
        let explorer = app
            .world()
            .non_send::<bed_plugin_file_explorer::ExplorerState>()
            .explorer();
        if explorer
            .file_tree
            .root_node
            .children
            .iter()
            .any(|node| node.full_path == file.to_string_lossy())
            && explorer
                .file_finder
                .filtered_list
                .iter()
                .any(|entry| entry.full_path == file.to_string_lossy())
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "project discovery never populated the tree: {:?}, file context {:?}, explorer root {:?}, watching {}, finder visible {}, finder status {:?}, indexed {:?}, children {:?}",
            app.world().resource::<WorkspaceState>().error,
            app.world().resource::<bed_files::FileContext>(),
            explorer.project_root,
            explorer.file_finder.workspace_filesystem().is_some(),
            explorer.file_finder.show_ff_window,
            explorer.file_finder.discovery_status,
            explorer.file_finder.filtered_list,
            explorer.file_tree.root_node.children
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        app.world()
            .resource::<bed_plugin_workspaces::WorkspacesState>()
            .recent_workspaces,
        vec![spec.clone()]
    );
    assert!(app.world().resource::<WorkspaceState>().error.is_none());
    app.world_mut().resource_mut::<HostEffects>().push(
        None,
        HostAction::OpenFile {
            path: file.to_string_lossy().into_owned(),
            viewer: None,
        },
    );
    app.update();
    assert!(
        app.world()
            .resource::<bed_api::documents::DocumentFrame>()
            .documents
            .iter()
            .any(|document| document.path == file.to_string_lossy())
    );
    assert!(app.world().resource::<WorkspaceState>().error.is_none());
    bed::application::dispatch_app_command(
        app.world_mut(),
        bed::application::WindowCommand::Quit,
        bed_api::workbench::PanelTarget::LargestArea,
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !app.world().resource::<WorkspaceState>().close_requested {
        app.update();
        assert!(
            Instant::now() < deadline,
            "clean workspace quit did not complete"
        );
    }
    assert!(app.world().resource::<WorkspaceState>().error.is_none());
    assert_eq!(
        WorkspaceStore::load(
            &bed_api::launch::ApplicationPaths::new("bed", Some(fixture.0.join("config")), None)
                .unwrap()
        )
        .unwrap()
        .recent_workspaces(),
        vec![spec.clone()]
    );
    drop(app);

    // A fresh standalone launcher receives saved history before its first draw.
    let mut app = fixture.app();
    assert_eq!(
        app.world()
            .resource::<bed_plugin_workspaces::WorkspacesState>()
            .recent_workspaces,
        vec![spec.clone()]
    );
    bed_plugin_workspaces::request_workspace(
        app.world_mut(),
        WorkspaceAction::OpenWorkspace(spec.clone()),
        None,
        0,
    );
    app.update();
    app.update();
    assert_eq!(
        app.world().resource::<WorkspaceState>().spec.as_ref(),
        Some(&spec)
    );
    assert_eq!(
        app.world()
            .resource::<bed::application::LayoutSlots>()
            .active(),
        bed::config::LayoutSlot::Layout1
    );
    assert!(
        app.world()
            .resource::<bed_api::documents::DocumentFrame>()
            .documents
            .iter()
            .any(|document| document.path == file.to_string_lossy())
    );
    assert!(app.world().resource::<WorkspaceState>().error.is_none());
    bed_plugin_workspaces::request_workspace(
        app.world_mut(),
        WorkspaceAction::RenameWorkspace(spec.clone(), "Renamed".into()),
        None,
        0,
    );
    app.update();
    app.update();
    assert_eq!(
        app.world()
            .resource::<bed_plugin_workspaces::WorkspacesState>()
            .recent_workspaces[0]
            .name,
        "Renamed"
    );
    bed_plugin_workspaces::request_workspace(
        app.world_mut(),
        WorkspaceAction::RemoveWorkspace(spec),
        None,
        0,
    );
    app.update();
    app.update();
    assert!(
        app.world()
            .resource::<bed_plugin_workspaces::WorkspacesState>()
            .recent_workspaces
            .is_empty()
    );
}
