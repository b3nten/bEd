#![cfg(all(feature = "editor", feature = "file-explorer", feature = "workspaces"))]

use bed_api::features::{HostAction, HostEffects};
use bed_app::{
    AppOptions, AppRuntimePlugin, RemoteHelperDirectory, WorkspacePersistence, WorkspaceSpec,
    WorkspaceState, WorkspaceStore,
};
use bed_app_config::Settings;
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
        app.insert_non_send(settings)
            .insert_non_send(bed_ui::icons::Icons::default())
            .insert_non_send(WorkspacePersistence::new(
                WorkspaceStore::load(&config).unwrap(),
            ))
            .insert_resource(RemoteHelperDirectory(PathBuf::new()))
            .add_plugins((
                bed_runtime::BedRuntimePlugin,
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
#[ignore = "requires installed rust-analyzer and rust-src; renders real dependency definition jumps"]
fn rust_dependency_navigation_reuses_one_server_and_keeps_definitions_visible() {
    use bed_api::documents::PanelBinding;
    use bed_documents::{ClosePolicy, DocumentSession};
    use bed_plugin_editor::lsp::lsp_ui::ContextLspAction;
    use bed_workbench::WorkbenchLayout;
    use bevy::ecs::system::SystemState;
    use dear_imgui_rs::Context;

    fn frame(app: &mut App, context: &mut Context) {
        app.update();
        bed_app::draw_ui(app.world_mut(), context.frame()).unwrap();
        drop(context.render_legacy());
        bed_app::process_app_requests(app.world_mut()).unwrap();
        std::thread::sleep(Duration::from_millis(10));
    }

    fn caret_visible(app: &App, panel: Entity) -> bool {
        let Some(surface) = bed_app::panel_presentation(app.world(), panel) else {
            return false;
        };
        let Some(geometry) = app.world().get::<bed_workbench::api::PanelGeometry>(panel) else {
            return false;
        };
        app.world()
            .non_send::<bed_plugin_editor::EditorViews>()
            .text(surface)
            .is_some_and(|view| {
                let caret = view.presentation().caret_position;
                caret[1] > geometry.content_min[1] && caret[1] < geometry.content_max[1]
            })
    }

    let _lock = IMGUI_TEST_LOCK.lock().unwrap();
    let fixture = Fixture::new();
    let root = fixture.0.join("rust-project");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("Cargo.toml"),
        "[package]\nname = \"bed_definition_fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[workspace]\n").unwrap();
    let source = root.join("src/main.rs");
    std::fs::write(
        &source,
        "fn main() { let _value: String = String::new(); }\n",
    )
    .unwrap();
    let mut app = fixture.app();
    std::fs::write(
        fixture.0.join("config/lsp.json"),
        serde_json::to_vec(&serde_json::json!({
            "language_servers":{"rust-analyzer":{"command":"rust-analyzer"}}
        }))
        .unwrap(),
    )
    .unwrap();
    bed_plugin_workspaces::request_workspace(
        app.world_mut(),
        WorkspaceAction::OpenLocalWorkspace(root),
        None,
        0,
    );
    app.update();
    let panel = bed_app::open_file(app.world_mut(), &source, None, false, None).unwrap();
    let source_binding = app.world().get::<PanelBinding>(panel).unwrap().clone();
    let document = source_binding.document.unwrap();
    let view = source_binding.view.unwrap();
    let mut context = Context::create();
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
    // Prepare the fixture's initial workspace analysis. Each navigation below is
    // a single editor action; a null/error response fails that action rather
    // than making the test silently issue another definition request.
    let startup = Instant::now();
    let deadline = startup + Duration::from_secs(90);
    let mut settled_since = None;
    let mut progress = std::collections::BTreeSet::new();
    let client = loop {
        frame(&mut app, &mut context);
        let client = app
            .world()
            .non_send::<DocumentSession>()
            .lsp()
            .unwrap()
            .client_for_document(document);
        if let Some(client) = &client {
            let client = client.borrow();
            let jobs = client.progress();
            for job in &jobs {
                progress.insert(job.title.clone());
            }
            let ready = client.is_initialized()
                && !progress.is_empty()
                && jobs.iter().all(|job| job.finished);
            if ready {
                let since = settled_since.get_or_insert_with(Instant::now);
                if since.elapsed() >= Duration::from_millis(750) {
                    drop(client);
                    break app
                        .world()
                        .non_send::<DocumentSession>()
                        .lsp()
                        .unwrap()
                        .client_for_document(document)
                        .unwrap();
                }
            } else {
                settled_since = None;
            }
        }
        assert!(
            Instant::now() < deadline,
            "initial workspace analysis did not settle: {progress:?}"
        );
    };
    let process = client.borrow().process_id().unwrap();
    eprintln!(
        "Initial workspace analysis: {:?}; progress: {progress:?}",
        startup.elapsed()
    );
    let mut string_binding = None;
    let mut string_source = String::new();
    for step in 0..3 {
        let (view, position, suffix, declaration) = if step == 1 {
            let binding: &PanelBinding = string_binding.as_ref().unwrap();
            let (row, line) = string_source
                .lines()
                .enumerate()
                .find(|(_, line)| line.contains("vec: Vec<u8>"))
                .unwrap();
            (
                binding.view.unwrap(),
                (row as i32, (line.find("Vec").unwrap() + 1) as i32),
                "alloc/src/vec/mod.rs",
                "pub struct Vec<",
            )
        } else {
            (view, (0, 25), "alloc/src/string.rs", "pub struct String")
        };
        let requested = Instant::now();
        {
            let mut state = SystemState::<(
                NonSendMut<bed_plugin_editor::EditorRuntime>,
                NonSendMut<DocumentSession>,
            )>::new(app.world_mut());
            let (mut runtime, mut session) = state.get_mut(app.world_mut()).unwrap();
            runtime
                .request_lsp(
                    &mut session,
                    view,
                    ContextLspAction::Definition,
                    Some(position),
                )
                .unwrap();
        }
        let definition = loop {
            frame(&mut app, &mut context);
            let runtime = app.world().non_send::<bed_plugin_editor::EditorRuntime>();
            assert!(
                runtime.navigation_outcome().is_none(),
                "single navigation action failed: {:?}",
                runtime.navigation_outcome()
            );
            let focused = app.world().resource::<WorkbenchLayout>().focused.unwrap();
            let binding = app.world().get::<PanelBinding>(focused).unwrap();
            if binding.document.is_some_and(|document| {
                app.world()
                    .non_send::<DocumentSession>()
                    .snapshot(document)
                    .unwrap()
                    .path
                    .ends_with(suffix)
            }) {
                break focused;
            }
            assert!(
                requested.elapsed() < Duration::from_secs(20),
                "single navigation timed out: {:?}",
                runtime.navigation_activity()
            );
        };
        // Let asynchronous syntax folds and the centering animation finish.
        let mut first_visible = caret_visible(&app, definition).then(|| requested.elapsed());
        for _ in 0..60 {
            frame(&mut app, &mut context);
            if first_visible.is_none() && caret_visible(&app, definition) {
                first_visible = Some(requested.elapsed());
            }
        }
        let binding = app.world().get::<PanelBinding>(definition).unwrap();
        let session = app.world().non_send::<DocumentSession>();
        let definition_path = session.snapshot(binding.document.unwrap()).unwrap().path;
        let contents = std::fs::read_to_string(&definition_path).unwrap();
        let expected_row = contents
            .lines()
            .position(|line| line.contains(declaration))
            .unwrap() as i32;
        let snapshot = session.view_snapshot(binding.view.unwrap()).unwrap();
        assert_eq!(snapshot.row, expected_row);
        assert!(snapshot.scroll_position[1] > 0.0);
        assert!(
            !snapshot.folds.ranges().is_empty(),
            "real syntax folds never arrived"
        );
        let destination_client = session
            .lsp()
            .unwrap()
            .client_for_document(binding.document.unwrap())
            .unwrap();
        assert!(
            std::rc::Rc::ptr_eq(&client, &destination_client),
            "dependency navigation forked a server"
        );
        let statuses = session.lsp().unwrap().server_statuses();
        let running: Vec<_> = statuses
            .iter()
            .filter(|status| status.process_id.is_some())
            .collect();
        assert_eq!(running.len(), 1, "unexpected extra servers: {running:?}");
        assert_eq!(running[0].process_id, Some(process));
        let surface = bed_app::panel_presentation(app.world(), definition).unwrap();
        let views = app.world().non_send::<bed_plugin_editor::EditorViews>();
        let caret = views.text(surface).unwrap().presentation().caret_position;
        let geometry = app
            .world()
            .get::<bed_workbench::api::PanelGeometry>(definition)
            .unwrap();
        assert!(
            caret[1] > geometry.content_min[1] && caret[1] < geometry.content_max[1],
            "definition is outside the actual rendered viewport: {snapshot:?}"
        );
        let timing = app
            .world()
            .non_send::<bed_plugin_editor::EditorRuntime>()
            .navigation_timing()
            .unwrap();
        let sent = timing.first_sent.unwrap().duration_since(timing.started);
        let response = timing
            .response_received
            .unwrap()
            .duration_since(timing.started);
        eprintln!(
            "Single action {step}: {suffix} row {expected_row}; action→send {sent:?}, action→response {response:?}, action→visible {:?}; caret {caret:?}; server PID {process}",
            first_visible.unwrap()
        );
        assert!(app.world().resource::<WorkspaceState>().error.is_none());
        if step == 0 {
            string_binding = Some(binding.clone());
            string_source = contents;
        }
    }
    app.world_mut()
        .non_send_mut::<DocumentSession>()
        .shutdown(ClosePolicy::Discard)
        .unwrap();
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
    let target_panel = bed_app::open_file(app.world_mut(), &target, None, false, None).unwrap();
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
        .resource_mut::<bed_workbench::WorkbenchAppearance>()
        .animations = false;
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
        bed_app::draw_ui(app.world_mut(), ui).unwrap();
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
        bed_app::process_app_requests(app.world_mut()).unwrap();
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
        bed_app::focus_panel(app.world_mut(), target_panel, true).unwrap();
        for _ in 0..3 {
            frame(&mut app, &mut context);
        }
        bed_app::focus_panel(app.world_mut(), files_panel, true).unwrap();
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
fn directory_picker_stays_temporary_until_saved_then_restores_recent_workspaces() {
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
    assert_eq!(app.world().resource::<WorkspaceState>().spec.as_ref(), None);
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
            "project discovery never populated the tree: {:?}, finder status {:?}, indexed {:?}, children {:?}",
            app.world().resource::<WorkspaceState>().error,
            explorer.file_finder.discovery_status,
            explorer.file_finder.filtered_list,
            explorer.file_tree.root_node.children
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        app.world()
            .resource::<bed_plugin_workspaces::WorkspacesState>()
            .recent_workspaces
            .is_empty()
    );
    bed_plugin_workspaces::request_workspace(
        app.world_mut(),
        WorkspaceAction::SaveCurrent { name: None },
        None,
        0,
    );
    app.update();
    assert_eq!(
        app.world().resource::<WorkspaceState>().spec.as_ref(),
        Some(&spec)
    );
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
    assert_eq!(
        WorkspaceStore::load(&fixture.0.join("config"))
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
