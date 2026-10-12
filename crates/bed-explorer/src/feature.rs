//! Files widgets keep pointer bounds; the shared finder and tree run in Prepare.
use super::*;
use bed_document_session::EditorSession;
use bed_editor_api::{
    CommandPresentations, EditorFeatureReply, EditorFeatureSystems, EditorFrame, EditorIntents,
    EditorPanelBinding, EditorPanelExternalFiles, EditorWorkspaceContribution,
    EditorWorkspaceRestore, FileClipboardService, PanelExternalFilesInvocation,
};
use bed_ui::icons::Icons;
use bed_workbench_api::{
    Action, CommandInvocation, CommandResult, Panel, PanelCreation, PanelPadding, PanelResult,
    register,
};
use bevy::{ecs::system::SystemState, prelude::*};

#[derive(Resource, Clone, Copy, Default)]
pub struct ExplorerPresentation {
    pub tree: FileTreeStyle,
    pub finder: FileFinderStyle,
}

#[derive(Default)]
pub struct ExplorerPanels(pub HashMap<Entity, ExplorerPanel>);
impl ExplorerPanels {
    pub fn get(&self, entity: Entity) -> Option<&ExplorerPanel> {
        self.0.get(&entity)
    }
    pub fn get_mut(&mut self, entity: Entity) -> Option<&mut ExplorerPanel> {
        self.0.get_mut(&entity)
    }
}

#[derive(Resource, Default)]
struct Transport(Option<RemoteClient>);
#[derive(Resource, Default)]
struct ReplyCursor(bevy::ecs::message::MessageCursor<EditorFeatureReply>);

#[derive(Resource, Default)]
struct DropCursor(bevy::ecs::message::MessageCursor<bed_workbench_api::PanelDrop>);

#[derive(Resource)]
struct Drag(bed_editor_api::PanelExternalFilesSystem);

pub struct ExplorerPlugin;
impl Plugin for ExplorerPlugin {
    fn build(&self, app: &mut App) {
        app.world_mut().init_non_send::<ExplorerState>();
        app.world_mut().init_non_send::<EditorSession>();
        app.world_mut().init_non_send::<ExplorerPanels>();
        app.world_mut()
            .init_non_send::<operations::FileOperations>();
        app.world_mut().init_non_send::<Transport>();
        app.init_resource::<ExplorerPresentation>()
            .init_resource::<EditorFrame>()
            .init_resource::<EditorIntents>()
            .init_resource::<bed_editor_api::Registry>()
            .init_resource::<CommandPresentations>()
            .init_resource::<ReplyCursor>()
            .init_resource::<DropCursor>()
            .add_message::<bed_workbench_api::PanelDrop>()
            .add_message::<EditorFeatureReply>();
        app.world_mut()
            .resource_mut::<bed_editor_api::Registry>()
            .register_with(MODULE_ID, |registrar| {
                registrar.command(SHOW_COMMAND, "Files", None);
                registrar.command(NEW_COMMAND, "New Files Panel", Some("files"));
                registrar.toolbar(NEW_COMMAND);
                registrar.command(FIND_COMMAND, "Find File", None);
                registrar.panel_options(
                    PANEL_ID,
                    "Files",
                    false,
                    PanelPlacement::Sidebar,
                    Some("explorer"),
                );
                registrar.panel_open_placement(PANEL_ID, PanelPlacement::Center);
            })
            .expect("Explorer metadata is registered once");
        register(app.world_mut(), MODULE_ID, |registrar| {
            registrar.panel(PANEL_ID, "Files", create_panel);
            for action in crate::shortcut_actions() {
                registrar.shortcut_action(action);
            }
            registrar.command(SHOW_COMMAND, "Files", show);
            registrar.command(NEW_COMMAND, "New Files Panel", open);
            registrar.command(FIND_COMMAND, "Find File", find);
            registrar.global_shortcut(FIND_COMMAND, &["Primary+P"]);
            registrar.global_shortcut(SHOW_COMMAND, &["Primary+Shift+E"]);
        })
        .expect("ExplorerPlugin is installed once");
        app.world_mut()
            .spawn(bed_workbench_api::WorkbenchOverlay(draw_popups));
        let drag = app.world_mut().register_system(external_files);
        app.insert_resource(Drag(drag));
        let save = app.world_mut().register_system(save_workspace);
        let restore = app.world_mut().register_system(restore_workspace);
        app.world_mut().spawn(EditorWorkspaceContribution {
            owner: MODULE_ID,
            save,
            restore,
        });
        app.add_systems(PreUpdate, prepare.in_set(EditorFeatureSystems::Prepare))
            .add_systems(Last, shutdown.after(bevy::app::OnAppExitSystems));
    }
}
fn shutdown(
    mut exit: MessageReader<bevy::app::AppExit>,
    mut handle: NonSendMut<ExplorerState>,
    mut operations: NonSendMut<operations::FileOperations>,
    mut session: NonSendMut<EditorSession>,
) {
    if exit.read().next().is_some() {
        operations.cancel(&mut session);
        let state = &mut handle.file_explorer;
        state.file_finder = file_finder::FileFinder::new();
        state.file_tree.file_info = Default::default();
        handle.file_content.reset();
        handle.actions.clear();
    }
}
fn protected_paths(world: &mut World) -> Vec<std::path::PathBuf> {
    world
        .query::<&EditorPanelBinding>()
        .iter(world)
        .filter_map(|binding| binding.input.local_file())
        .flat_map(|file| {
            [
                file.to_path_buf(),
                std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf()),
            ]
        })
        .flat_map(|file| {
            ["", "-wal", "-shm"].map(|suffix| {
                let mut path = file.as_os_str().to_os_string();
                path.push(suffix);
                std::path::PathBuf::from(path)
            })
        })
        .collect()
}
fn prepare(world: &mut World) {
    let alive: std::collections::HashSet<_> = world.query::<Entity>().iter(world).collect();
    world
        .non_send_mut::<ExplorerPanels>()
        .0
        .retain(|entity, _| alive.contains(entity));
    let protected = protected_paths(world);
    let mut requests = Vec::new();
    let mut state = SystemState::<(
        NonSendMut<ExplorerState>,
        NonSendMut<EditorSession>,
        NonSendMut<operations::FileOperations>,
        NonSendMut<Transport>,
        Res<EditorFrame>,
        Res<bed_editor_api::Registry>,
        Res<CommandPresentations>,
        Res<ExplorerPresentation>,
        Option<NonSendMut<Box<dyn FileClipboardService>>>,
        Res<Messages<EditorFeatureReply>>,
        ResMut<ReplyCursor>,
    )>::new(world);
    let (
        mut explorer,
        mut documents,
        mut operations,
        mut transport,
        frame,
        registry,
        commands,
        presentation,
        mut clipboard,
        replies,
        mut cursor,
    ) = state.get_mut(world).expect("Files resources are installed");
    let remote = documents.remote_client();
    let changed = match (&transport.0, &remote) {
        (Some(old), Some(new)) => !old.same_connection(new),
        (None, None) => false,
        _ => true,
    };
    if changed {
        explorer.set_remote_client(remote.clone());
        transport.0 = remote;
    }
    explorer.configure_presentation(
        presentation.tree,
        presentation.finder,
        documents.workspace_id().0.to_string(),
    );
    explorer.update_menu_model(&registry, &frame.context(), |command, _| {
        commands
            .0
            .get(command)
            .is_none_or(|state| state.visible && state.enabled)
    });
    for reply in cursor.0.read(&replies) {
        operations.reply(reply, &mut documents, &explorer);
    }
    explorer.tick(&documents, &mut requests);
    operations::reconcile(&mut documents, &mut explorer, &mut requests);
    operations.poll(
        &mut documents,
        &mut explorer,
        clipboard
            .as_mut()
            .map(|clipboard| &mut ***clipboard as &mut dyn FileClipboardService),
        &protected,
        &mut requests,
    );
    for action in explorer.take_actions() {
        if let Err(error) = operations.action(
            action,
            &mut documents,
            &mut explorer,
            &frame,
            clipboard
                .as_mut()
                .map(|clipboard| &mut ***clipboard as &mut dyn FileClipboardService),
            &protected,
            &mut requests,
        ) {
            requests.push(HostRequest::Notify {
                message: error.to_string(),
            });
        }
    }
    for request in requests {
        world.resource_mut::<EditorIntents>().push(None, request);
    }
}

fn show(
    In(invocation): In<CommandInvocation>,
    mut intents: ResMut<EditorIntents>,
) -> CommandResult {
    intents.push_at(
        invocation.source,
        invocation.area,
        HostRequest::ShowPanel {
            panel_type: PANEL_ID.into(),
            document: None,
            state: Value::Null,
            action: None,
        },
    );
    Ok(Vec::new())
}
fn open(In(invocation): In<CommandInvocation>) -> CommandResult {
    Ok(vec![Action::OpenPanel {
        panel_type: PANEL_ID.into(),
        state: Value::Null,
        area: Some(invocation.area),
    }])
}
fn find(
    In(invocation): In<CommandInvocation>,
    mut handle: NonSendMut<ExplorerState>,
    mut intents: ResMut<EditorIntents>,
) -> CommandResult {
    handle.toggle_finder();
    intents.push(invocation.source, HostRequest::Invalidate);
    Ok(Vec::new())
}
fn create_panel(In(creation): In<PanelCreation>, world: &mut World) -> PanelResult {
    world
        .non_send_mut::<ExplorerPanels>()
        .0
        .insert(creation.entity, ExplorerPanel::new());
    let drag = world.resource::<Drag>().0;
    world
        .entity_mut(creation.entity)
        .insert((PanelPadding([2.0; 2]), EditorPanelExternalFiles(drag)));
    Ok(Box::new(Frontend(creation.entity)))
}
fn external_files(
    In(invocation): In<PanelExternalFilesInvocation>,
    world: &mut World,
) -> Result<bed_editor_api::ExternalFileDropResponse, String> {
    let mut requests = Vec::new();
    let mut state =
        SystemState::<(NonSendMut<ExplorerPanels>, NonSendMut<ExplorerState>)>::new(world);
    let (mut panels, mut controller) = state.get_mut(world).expect("Files resources are installed");
    let response = panels
        .0
        .get_mut(&invocation.entity)
        .ok_or("Files panel is closed")?
        .external_files(&mut controller, &invocation.event, &mut requests)
        .map_err(|error| error.to_string())?;
    for request in requests {
        world
            .resource_mut::<EditorIntents>()
            .push(Some(invocation.entity), request);
    }
    Ok(response)
}

struct Frontend(Entity);
impl Panel for Frontend {
    fn title(&self, _: &World) -> String {
        "Files".into()
    }
    fn draw(
        &mut self,
        ui: &Ui,
        world: &mut World,
        _: &mut Vec<Action>,
        _shortcuts: &bed_workbench_api::ShortcutInput,
    ) {
        let mut requests = Vec::new();
        let mut state = SystemState::<(
            NonSendMut<ExplorerPanels>,
            NonSendMut<ExplorerState>,
            NonSendMut<EditorSession>,
            Res<EditorFrame>,
            Option<NonSend<Icons>>,
        )>::new(world);
        let (mut panels, mut controller, mut documents, frame, icons) = state
            .get_mut(world)
            .expect("Files frontend resources are installed");
        controller.file_explorer.file_tree.shortcuts = _shortcuts.clone();
        controller.config.tree_style.rainbow_time = ui.time() as f32;
        if let Err(error) = panels
            .0
            .get_mut(&self.0)
            .expect("Files frontend has widget state")
            .draw_body(
                &mut controller,
                ui,
                &frame,
                &mut documents,
                icons.as_deref().map(|icons| icons as &dyn FileIcons),
                &mut requests,
            )
        {
            requests.push(HostRequest::Notify {
                message: error.to_string(),
            });
        }
        for request in requests {
            world
                .resource_mut::<EditorIntents>()
                .push(Some(self.0), request);
        }
    }
}
/// Draw the finder even when no Files panel is open; the application calls this
/// once during its frontend scope, after feature preparation.
pub fn draw_popups(world: &mut World, ui: &Ui) {
    open_dropped_files(world);
    let mut requests = Vec::new();
    let protected = protected_paths(world);
    let mut state = SystemState::<(
        NonSendMut<ExplorerState>,
        NonSendMut<operations::FileOperations>,
        NonSend<EditorSession>,
        Res<EditorFrame>,
        Option<NonSend<Icons>>,
    )>::new(world);
    let (mut explorer, mut operations, session, frame, icons) = state
        .get_mut(world)
        .expect("Files popup resources are installed");
    explorer.config.tree_style.rainbow_time = ui.time() as f32;
    explorer.draw_popups(
        ui,
        icons.as_deref().map(|icons| icons as &dyn FileIcons),
        &mut requests,
    );
    operations.draw(ui, &session, &frame, &protected, &mut requests);
    for request in requests {
        world.resource_mut::<EditorIntents>().push(None, request);
    }
}

fn open_dropped_files(world: &mut World) {
    let mut state = SystemState::<(
        Res<Messages<bed_workbench_api::PanelDrop>>,
        ResMut<DropCursor>,
        NonSend<ExplorerState>,
        ResMut<EditorIntents>,
    )>::new(world);
    let (drops, mut cursor, explorer, mut intents) = state.get_mut(world).unwrap();
    let tree = &explorer.file_explorer.file_tree;
    for drop in cursor.0.read(&drops) {
        if drop.payload_type != "BED_FILES" {
            continue;
        }
        if let Some(paths) = tree.dragged_paths() {
            for path in paths
                .iter()
                .filter(|path| tree.is_directory(path) != Some(true))
            {
                intents.push_at(
                    drop.panel,
                    drop.area,
                    HostRequest::DropFile { path: path.clone() },
                );
            }
        }
    }
}
fn save_workspace(handle: NonSendMut<ExplorerState>) -> Value {
    let state = &handle.file_explorer;
    let mut value = state.file_tree.preferences.to_value();
    value["include_ignored"] = Value::Bool(state.file_finder.include_ignored());
    value
}
fn restore_workspace(
    In(restore): In<EditorWorkspaceRestore>,
    mut handle: NonSendMut<ExplorerState>,
    documents: NonSend<EditorSession>,
) -> Result<(), String> {
    handle.file_content.reset();
    handle.actions.clear();
    let state = &mut handle.file_explorer;
    state.project_root = restore.root;
    state.file_finder.cancel_and_close();
    state.file_finder.set_project_dir("");
    state
        .file_finder
        .set_remote_client(documents.remote_client());
    state.file_finder.set_directory_label(None);
    let preferences = restore
        .state
        .as_ref()
        .map(FileTreePreferences::from_value)
        .unwrap_or_default();
    state.file_tree = file_tree::FileTree {
        root_node: FileNode {
            name: bed_editor_api::workspace::default_name(&state.project_root),
            full_path: state.project_root.clone(),
            is_directory: true,
            is_open: true,
            ..Default::default()
        },
        preferences,
        ..Default::default()
    };
    let include_ignored = state.file_tree.preferences.include_ignored;
    state.file_finder.set_include_ignored(include_ignored);
    state.file_finder.set_project_dir(&state.project_root);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bed_workbench::{WorkbenchPlugin, WorkbenchRequests, commit_close};

    #[test]
    fn panel_and_finder_draw_current_icons_after_feature_preparation() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let temp = crate::test_support::TempDir::new();
        let file = temp.write("example.txt", b"example");
        let root = temp.root().to_string_lossy().into_owned();
        let old = dear_imgui_rs::TextureId::from(23usize);
        let current = dear_imgui_rs::TextureId::from(148usize);
        let mut app = App::new();
        let mut icons = Icons::default();
        icons.set_texture("default", old);
        app.world_mut().insert_non_send(icons);
        app.add_plugins((WorkbenchPlugin, ExplorerPlugin));
        app.world_mut()
            .resource_mut::<WorkbenchRequests>()
            .push(Action::OpenPanel {
                panel_type: PANEL_ID.into(),
                state: Value::Null,
                area: None,
            });
        app.update();
        let entity = *app
            .world()
            .non_send::<ExplorerPanels>()
            .0
            .keys()
            .next()
            .unwrap();
        app.world_mut().resource_mut::<EditorFrame>().project_root = root.clone();
        {
            let mut explorer = app.world_mut().non_send_mut::<ExplorerState>();
            explorer.file_explorer.project_root = root.clone();
            explorer.file_explorer.file_tree.root_node = FileNode {
                name: "project".into(),
                full_path: root.clone(),
                is_directory: true,
                is_open: true,
                children: vec![FileNode {
                    name: "example.txt".into(),
                    full_path: file.to_string_lossy().into_owned(),
                    ..Default::default()
                }],
                ..Default::default()
            };
        }
        // The canvas stage may replace all GPU registrations after feature
        // preparation and before UI. The frontend must read this new mapping.
        prepare(app.world_mut());
        app.world_mut()
            .non_send_mut::<Icons>()
            .set_texture("default", current);
        let mut context = dear_imgui_rs::Context::create();
        context
            .set_ini_filename(None::<std::path::PathBuf>)
            .unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        for finder in [false, true] {
            if finder {
                let mut explorer = app.world_mut().non_send_mut::<ExplorerState>();
                explorer.file_explorer.file_finder.show_ff_window = true;
                explorer.file_explorer.file_finder.filtered_list =
                    vec![bed_files::file_finder::FileEntry::from_path(&file, temp.root()).unwrap()];
            }
            for _ in 0..2 {
                context.prepare_frame(dear_imgui_rs::FramePrepareOptions::new(
                    [800.0, 600.0],
                    1.0 / 60.0,
                ));
                let ui = context.frame();
                if finder {
                    draw_popups(app.world_mut(), ui);
                } else {
                    ui.window("Files")
                        .position([0.0; 2], dear_imgui_rs::Condition::Always)
                        .size([600.0, 500.0], dear_imgui_rs::Condition::Always)
                        .build(|| {
                            bed_workbench::render_panel(app.world_mut(), entity, ui).unwrap();
                        });
                }
                let draw = context.render_legacy();
                let textures: Vec<_> = draw
                    .draw_lists()
                    .flat_map(|list| list.commands())
                    .filter_map(|command| match command {
                        dear_imgui_rs::DrawCmd::Elements { cmd_params, .. } => {
                            Some(cmd_params.texture_id)
                        }
                        _ => None,
                    })
                    .collect();
                assert!(
                    !textures.contains(&old),
                    "retired icon used by finder={finder}"
                );
                assert!(
                    textures.contains(&current),
                    "current icon absent for finder={finder}"
                );
            }
        }
    }

    #[test]
    fn live_local_file_panels_protect_database_and_sidecars_before_job_creation() {
        let temp = crate::test_support::TempDir::new();
        let database = temp.write("database.sqlite", b"database");
        let sidecar = temp.write("database.sqlite-wal", b"pending transaction");
        std::fs::create_dir(temp.path("destination")).unwrap();
        let mut app = App::new();
        app.add_plugins(ExplorerPlugin);
        let workspace = app.world().non_send::<EditorSession>().workspace_id().0;
        app.world_mut().resource_mut::<EditorFrame>().project_root =
            temp.root().to_string_lossy().into_owned();
        app.world_mut().resource_mut::<EditorFrame>().workspace = workspace;
        app.world_mut().spawn(EditorPanelBinding {
            input: bed_editor_api::PanelInput::LocalFile(database),
            workspace,
            ..Default::default()
        });
        app.world_mut()
            .non_send_mut::<ExplorerState>()
            .actions
            .push(ExplorerAction::Tree(FileTreeAction::Move {
                paths: vec![sidecar.to_string_lossy().into_owned()],
                destination: temp.path("destination").to_string_lossy().into_owned(),
            }));
        app.update();
        assert!(!app.world().non_send::<operations::FileOperations>().busy());
        assert_eq!(std::fs::read(&sidecar).unwrap(), b"pending transaction");
        assert!(!temp.path("destination/database.sqlite-wal").exists());
        let intents = app.world_mut().resource_mut::<EditorIntents>().take();
        assert!(intents.iter().any(|intent|matches!(&intent.request,HostRequest::Notify{message} if message.contains("Close the file viewer"))));
        assert!(intents.iter().all(|intent| !matches!(
            &intent.request,
            HostRequest::CommitDocumentsWithResult { .. }
        )));
    }
    #[test]
    fn registered_files_panels_and_prepare_share_the_supplied_controller() {
        let mut app = App::new();
        app.world_mut().insert_non_send(EditorSession::new());
        app.add_plugins((WorkbenchPlugin, ExplorerPlugin));
        for _ in 0..2 {
            app.world_mut()
                .resource_mut::<WorkbenchRequests>()
                .push(Action::Invoke(CommandInvocation {
                    command: NEW_COMMAND.into(),
                    source: None,
                    area: 0,
                    arguments: Value::Null,
                }));
            app.update();
        }
        let panels: Vec<_> = app
            .world()
            .non_send::<ExplorerPanels>()
            .0
            .keys()
            .copied()
            .collect();
        assert_eq!(panels.len(), 2);
        app.world_mut()
            .non_send_mut::<ExplorerState>()
            .file_explorer
            .file_tree
            .error = Some("filesystem watch failed".into());
        app.update();
        assert!(app.world_mut().resource_mut::<EditorIntents>().take().iter().any(|intent|matches!(&intent.request,HostRequest::Notify{message} if message=="filesystem watch failed")));
        assert!(commit_close(app.world_mut(), panels[0]));
        app.update();
        assert_eq!(app.world().non_send::<ExplorerPanels>().0.len(), 1);
        assert!(
            app.world()
                .get::<bed_workbench_api::PanelPresentation>(panels[1])
                .is_some()
        );
    }
    #[test]
    fn registered_workspace_restore_resets_navigation_and_round_trips_preferences() {
        let mut app = App::new();
        app.add_plugins(ExplorerPlugin);
        let contribution = *app
            .world_mut()
            .query::<&EditorWorkspaceContribution>()
            .single(app.world())
            .unwrap();
        app.world_mut()
            .run_system_with(
                contribution.restore,
                EditorWorkspaceRestore {
                    workspace: 1,
                    root: "/new-project".into(),
                    state: Some(serde_json::json!({"hide_hidden":true,"include_ignored":true})),
                },
            )
            .unwrap()
            .unwrap();
        let saved = app.world_mut().run_system(contribution.save).unwrap();
        assert_eq!(saved["hide_hidden"], true);
        assert_eq!(saved["include_ignored"], true);
        assert_eq!(
            app.world()
                .non_send::<ExplorerState>()
                .explorer()
                .project_root,
            "/new-project"
        );
    }
    #[test]
    fn registered_native_drop_uses_this_presentation_bounds_and_rejects_stale_workspace() {
        let mut app = App::new();
        app.add_plugins((WorkbenchPlugin, ExplorerPlugin));
        app.world_mut()
            .non_send_mut::<ExplorerState>()
            .file_explorer
            .project_root = "/project".into();
        app.world_mut()
            .resource_mut::<WorkbenchRequests>()
            .push(Action::OpenPanel {
                panel_type: PANEL_ID.into(),
                state: Value::Null,
                area: None,
            });
        app.update();
        let entity = *app
            .world()
            .non_send::<ExplorerPanels>()
            .0
            .keys()
            .next()
            .unwrap();
        {
            let mut panels = app.world_mut().non_send_mut::<ExplorerPanels>();
            let panel = panels.get_mut(entity).unwrap();
            panel.drop_root = "/project".into();
            panel.panel_bounds = Some(([10.0, 10.0], [200.0, 200.0]));
        }
        let handler = app
            .world()
            .get::<EditorPanelExternalFiles>(entity)
            .unwrap()
            .0;
        let event = bed_editor_api::ExternalFileDrag {
            paths: vec!["/incoming.txt".into()],
            position: [50.0, 50.0],
            viewport: 1,
            phase: bed_editor_api::ExternalFileDragPhase::Drop,
            modifiers: Default::default(),
        };
        assert_eq!(
            app.world_mut()
                .run_system_with(
                    handler,
                    PanelExternalFilesInvocation {
                        entity,
                        event: event.clone()
                    }
                )
                .unwrap()
                .unwrap(),
            bed_editor_api::ExternalFileDropResponse::Accepted
        );
        let actions = app
            .world_mut()
            .non_send_mut::<ExplorerState>()
            .take_actions();
        assert!(
            matches!(&actions[0],ExplorerAction::Import{destination,paths} if destination=="/project" && paths==&event.paths)
        );
        app.world_mut()
            .non_send_mut::<ExplorerState>()
            .file_explorer
            .project_root = "/different".into();
        assert_eq!(
            app.world_mut()
                .run_system_with(handler, PanelExternalFilesInvocation { entity, event })
                .unwrap()
                .unwrap(),
            bed_editor_api::ExternalFileDropResponse::Ignored
        );
        assert!(
            app.world_mut()
                .non_send_mut::<ExplorerState>()
                .take_actions()
                .is_empty()
        );
    }
}
