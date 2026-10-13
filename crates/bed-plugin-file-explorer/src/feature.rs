//! Files widgets keep pointer bounds; the shared finder and tree run in Prepare.
use super::*;
use bed_api::documents::DocumentFrame;
use bed_api::features::FrontendSnapshot;
use bed_api::storage::{FeatureStateContribution, FeatureStateRestore};
use bed_api::workbench::{
    CommandInvocation, CommandResult, Panel, PanelCreation, PanelPadding, PanelResult,
    WorkbenchAction, register,
};
use bed_api::{
    documents::DocumentReply,
    features::{FeatureSystems, HostEffects},
    workbench::{
        CommandPresentations, FileClipboardService, PanelExternalFiles,
        PanelExternalFilesInvocation,
    },
};
use bed_documents::DocumentSession;
use bed_files::{FileContext, FileServices};
use bed_ui::icons::Icons;
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
struct Transport(FileServices);

#[derive(Resource, Default)]
struct ReplyCursor(bevy::ecs::message::MessageCursor<DocumentReply>);

#[derive(Resource, Default)]
struct DropCursor(bevy::ecs::message::MessageCursor<bed_api::workbench::PanelDrop>);

#[derive(Resource)]
struct Drag(bed_api::workbench::PanelExternalFilesSystem);

/// Filesystem location selected by the host, independent of its workspace policy.
#[derive(Resource, Clone, Default)]
pub struct ExplorerTarget {
    pub root: String,
    pub services: FileServices,
}

impl ExplorerTarget {
    fn resolve(&self, context: &FileContext, services: &FileServices) -> Self {
        if self.root.is_empty() {
            Self {
                root: context.directory_string(),
                services: services.for_context(context),
            }
        } else {
            self.clone()
        }
    }
}

/// Inherited visibility choices. Saved feature state retains only explicit overrides.
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExplorerDefaults {
    pub hide_hidden: bool,
    pub hide_gitignored: bool,
}

impl Default for ExplorerDefaults {
    fn default() -> Self {
        Self {
            hide_hidden: true,
            hide_gitignored: true,
        }
    }
}

pub struct FileExplorerPlugin;

impl Plugin for FileExplorerPlugin {
    fn build(&self, app: &mut App) {
        bed_api::settings::register(
            app.world_mut(),
            MODULE_ID,
            vec![
                bed_api::settings::SettingDefinition::new("hide_hidden", "Hide hidden files", true),
                bed_api::settings::SettingDefinition::new(
                    "hide_gitignored",
                    "Hide gitignored files",
                    true,
                ),
            ],
        )
        .expect("Explorer preferences install once");
        app.init_non_send::<bed_ui::icons::Icons>();
        let directory = app
            .world()
            .get_non_send::<bed_ui::icons::IconDirectories>()
            .and_then(|sources| sources.get(MODULE_ID))
            .map(std::path::Path::to_path_buf);
        let definitions = [
            ("files", "files.svg", "files"),
            ("folder", "folder.svg", "folder"),
            ("folder-open", "folder-open.svg", "folder-open"),
            ("chevron-right", "chevron-right.svg", "chevron-right"),
            ("chevron-down", "chevron-down.svg", "chevron-down"),
        ]
        .map(|(name, filename, bundled)| bed_ui::icons::IconDefinition {
            name,
            filename,
            embedded_svg: bed_ui::icons::Icons::bundled_svg(bundled),
        });
        app.world_mut()
            .non_send_mut::<bed_ui::icons::Icons>()
            .register(MODULE_ID, &definitions, directory.as_deref())
            .expect("plugin icon contract and configured sources are valid");
        app.world_mut().init_non_send::<ExplorerState>();
        app.world_mut().init_non_send::<DocumentSession>();
        app.world_mut().init_non_send::<ExplorerPanels>();
        app.world_mut()
            .init_non_send::<operations::FileOperations>();
        app.world_mut().init_non_send::<Transport>();
        app.init_resource::<ExplorerPresentation>()
            .init_resource::<DocumentFrame>()
            .init_resource::<ExplorerTarget>()
            .init_resource::<FileContext>()
            .init_resource::<FileServices>()
            .init_resource::<ExplorerDefaults>()
            .init_resource::<HostEffects>()
            .init_resource::<bed_api::features::FeatureRegistry>()
            .init_resource::<CommandPresentations>()
            .init_resource::<ReplyCursor>()
            .init_resource::<DropCursor>()
            .add_message::<bed_api::workbench::PanelDrop>()
            .add_message::<DocumentReply>()
            .add_message::<bed_api::storage::FeatureStateChanged>();
        app.world_mut()
            .resource_mut::<bed_api::features::FeatureRegistry>()
            .register_with(MODULE_ID, |registrar| {
                registrar.command_ref(SHOW_COMMAND);
                registrar.command_ref(NEW_COMMAND);
                registrar.toolbar(NEW_COMMAND);
                registrar.command_ref(FIND_COMMAND);
                registrar.panel_options(PANEL_ID, false, PanelPlacement::Sidebar);
                registrar.panel_open_placement(PANEL_ID, PanelPlacement::Center);
                registrar.settings_ui("bed.explorer.settings", "Files", files_settings);
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
            registrar.panel_icon(PANEL_ID, "bed.explorer.files");
            registrar.command_icon(NEW_COMMAND, "bed.explorer.files");
            registrar.toolbar(NEW_COMMAND);
        })
        .expect("FileExplorerPlugin is installed once");
        app.world_mut()
            .spawn(bed_api::workbench::WorkbenchOverlay(draw_popups));
        let drag = app.world_mut().register_system(external_files);
        app.insert_resource(Drag(drag));
        let save = app.world_mut().register_system(save_workspace);
        let restore = app.world_mut().register_system(restore_workspace);
        app.world_mut().spawn(FeatureStateContribution {
            owner: MODULE_ID,
            save,
            restore,
        });
        app.add_systems(PreUpdate, prepare.in_set(FeatureSystems::Prepare))
            .add_systems(Last, shutdown.after(bevy::app::OnAppExitSystems));
    }
}

fn files_settings(ui: &Ui, settings: &mut Value) -> bool {
    let mut changed = false;
    for (key, label) in [
        ("hide_hidden", "Hide hidden files by default"),
        ("hide_gitignored", "Hide gitignored files by default"),
    ] {
        let mut value = settings[key].as_bool().unwrap_or(true);
        if ui.checkbox(label, &mut value) {
            settings[key] = Value::Bool(value);
            changed = true;
        }
    }
    changed
}

fn shutdown(
    mut exit: MessageReader<bevy::app::AppExit>,
    mut handle: NonSendMut<ExplorerState>,
    mut operations: NonSendMut<operations::FileOperations>,
    mut session: NonSendMut<DocumentSession>,
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

fn prepare(world: &mut World) {
    if let Some(settings) = world.get_resource::<bed_api::settings::SettingsSnapshot>() {
        let current = *world.resource::<ExplorerDefaults>();
        let defaults = ExplorerDefaults {
            hide_hidden: settings
                .get(MODULE_ID, "hide_hidden")
                .and_then(Value::as_bool)
                .unwrap_or(current.hide_hidden),
            hide_gitignored: settings
                .get(MODULE_ID, "hide_gitignored")
                .and_then(Value::as_bool)
                .unwrap_or(current.hide_gitignored),
        };
        *world.resource_mut::<ExplorerDefaults>() = defaults;
    }
    let alive: std::collections::HashSet<_> = world.query::<Entity>().iter(world).collect();
    world
        .non_send_mut::<ExplorerPanels>()
        .0
        .retain(|entity, _| alive.contains(entity));
    let mut requests = Vec::new();
    let frame = FrontendSnapshot::capture(world);
    let mut state = SystemState::<(
        NonSendMut<ExplorerState>,
        NonSendMut<DocumentSession>,
        NonSendMut<operations::FileOperations>,
        NonSendMut<Transport>,
        Res<bed_api::features::FeatureRegistry>,
        Res<bed_api::workbench::WorkbenchRegistry>,
        Res<CommandPresentations>,
        Res<ExplorerPresentation>,
        (Res<ExplorerTarget>, Res<FileContext>, Res<FileServices>),
        Res<ExplorerDefaults>,
        Option<NonSendMut<Box<dyn FileClipboardService>>>,
        Res<Messages<DocumentReply>>,
        ResMut<ReplyCursor>,
        MessageWriter<bed_api::storage::FeatureStateChanged>,
    )>::new(world);
    let (
        mut explorer,
        mut documents,
        mut operations,
        mut transport,
        registry,
        definitions,
        commands,
        presentation,
        (target, file_context, file_services),
        defaults,
        mut clipboard,
        replies,
        mut cursor,
        mut feature_changes,
    ) = state.get_mut(world).expect("Files resources are installed");
    let target = target.resolve(&file_context, &file_services);
    operations.bind_target(&target, &mut documents);
    let services = target.services.clone();
    let changed = !transport.0.same_provider(&services);
    if changed || explorer.file_explorer.project_root != target.root {
        let preferences = explorer.file_explorer.file_tree.preferences.clone();
        configure_target(&mut explorer, &target, preferences);
        transport.0 = services;
    }
    explorer.configure_presentation(
        presentation.tree,
        presentation.finder,
        documents.session_id().0.to_string(),
    );
    if explorer
        .file_explorer
        .file_tree
        .preferences
        .apply_defaults(defaults.hide_hidden, defaults.hide_gitignored)
    {
        let directories = explorer.file_explorer.file_tree.open_directories();
        explorer
            .file_explorer
            .file_finder
            .refresh_directories(directories);
        requests.push(HostAction::Invalidate);
    }
    let mut context = frame.context();
    context.remote = target.services.is_remote();
    if !operations::provider_matches(&target.services, &documents) {
        context.documents = &[];
        context.active_document = None;
    }
    explorer.update_menu_model(
        &registry,
        &definitions,
        Some(&commands),
        &context,
        |_, _| true,
    );
    for reply in cursor.0.read(&replies) {
        operations.reply(reply, &mut documents, &explorer);
    }
    explorer.tick(&target, &mut requests);
    operations::reconcile(&mut documents, &mut explorer, &target, &mut requests);
    operations.poll(
        &mut documents,
        &mut explorer,
        clipboard
            .as_mut()
            .map(|clipboard| &mut ***clipboard as &mut dyn FileClipboardService),
        &mut requests,
    );
    for action in explorer.take_actions() {
        if matches!(&action, ExplorerAction::PreferencesChanged(_)) {
            feature_changes.write(bed_api::storage::FeatureStateChanged { owner: MODULE_ID });
        }
        if let Err(error) = operations.action(
            action,
            &mut documents,
            &mut explorer,
            &target.root,
            clipboard
                .as_mut()
                .map(|clipboard| &mut ***clipboard as &mut dyn FileClipboardService),
            &mut requests,
        ) {
            requests.push(HostAction::Notify {
                message: error.to_string(),
            });
        }
    }
    for request in requests {
        world.resource_mut::<HostEffects>().push(None, request);
    }
}

fn show(In(invocation): In<CommandInvocation>, mut intents: ResMut<HostEffects>) -> CommandResult {
    intents.push_at(
        invocation.source,
        invocation.area,
        HostAction::ShowPanel {
            panel_type: PANEL_ID.into(),
            document: None,
            state: Value::Null,
            action: None,
        },
    );
    Ok(Vec::new())
}

fn open(In(invocation): In<CommandInvocation>) -> CommandResult {
    Ok(vec![WorkbenchAction::OpenPanel {
        panel_type: PANEL_ID.into(),
        state: Value::Null,
        area: Some(invocation.area),
    }])
}

fn find(
    In(invocation): In<CommandInvocation>,
    mut handle: NonSendMut<ExplorerState>,
    mut intents: ResMut<HostEffects>,
) -> CommandResult {
    handle.toggle_finder();
    intents.push(invocation.source, HostAction::Invalidate);
    Ok(Vec::new())
}

fn create_panel(In(creation): In<PanelCreation>, world: &mut World) -> PanelResult {
    world.entity_mut(creation.entity).insert(bed_api::workbench::PanelDocumentation::new(&[
        ("Files", "Expand folders and open files from the tree. Use Shift to extend a selection, or Cmd/Ctrl to toggle individual items."),
        ("Operations", "Right-click files or folders for rename, duplicate, copy, paste, and delete. Drag files onto a tab area to open them there."),
        ("Visibility", "Choose whether hidden and gitignored files are hidden by default in Settings > Extensions > Files. Right-click the tree to change workspace visibility or temporarily show all files."),
    ]).shortcut_actions(&["bed.navigation.move_left", "bed.navigation.move_right", "bed.navigation.move_up", "bed.navigation.move_down", "bed.navigation.select_up", "bed.navigation.select_down", "bed.edit.copy", "bed.edit.cut", "bed.edit.paste", "bed.edit.select_all"]));
    world
        .non_send_mut::<ExplorerPanels>()
        .0
        .insert(creation.entity, ExplorerPanel::new());
    let drag = world.resource::<Drag>().0;
    world
        .entity_mut(creation.entity)
        .insert((PanelPadding([2.0; 2]), PanelExternalFiles(drag)));
    Ok(Box::new(Frontend(creation.entity)))
}

fn external_files(
    In(invocation): In<PanelExternalFilesInvocation>,
    world: &mut World,
) -> Result<bed_api::workbench::ExternalFileDropResponse, String> {
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
            .resource_mut::<HostEffects>()
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
        _: &mut Vec<WorkbenchAction>,
        _shortcuts: &bed_api::workbench::ShortcutInput,
    ) {
        let mut requests = Vec::new();
        let frame = FrontendSnapshot::capture(world);
        let mut state = SystemState::<(
            NonSendMut<ExplorerPanels>,
            NonSendMut<ExplorerState>,
            NonSendMut<DocumentSession>,
            Res<ExplorerTarget>,
            Res<FileContext>,
            Res<FileServices>,
            Option<NonSend<Icons>>,
        )>::new(world);
        let (mut panels, mut controller, mut documents, target, file_context, file_services, icons) =
            state
                .get_mut(world)
                .expect("Files frontend resources are installed");
        let target = target.resolve(&file_context, &file_services);
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
                &target,
                &mut documents,
                icons.as_deref().map(|icons| icons as &dyn FileIcons),
                &mut requests,
            )
        {
            requests.push(HostAction::Notify {
                message: error.to_string(),
            });
        }
        for request in requests {
            world
                .resource_mut::<HostEffects>()
                .push(Some(self.0), request);
        }
    }
}

/// Draw the finder even when no Files panel is open; the application calls this
/// once during its frontend scope, after feature preparation.
pub fn draw_popups(world: &mut World, ui: &Ui) {
    open_dropped_files(world);
    let mut requests = Vec::new();
    let mut state = SystemState::<(
        NonSendMut<ExplorerState>,
        NonSendMut<operations::FileOperations>,
        NonSend<DocumentSession>,
        Res<ExplorerTarget>,
        Res<FileContext>,
        Res<FileServices>,
        Option<NonSend<Icons>>,
    )>::new(world);
    let (mut explorer, mut operations, session, target, file_context, file_services, icons) = state
        .get_mut(world)
        .expect("Files popup resources are installed");
    let target = target.resolve(&file_context, &file_services);
    explorer.config.tree_style.rainbow_time = ui.time() as f32;
    explorer.draw_popups(
        ui,
        icons.as_deref().map(|icons| icons as &dyn FileIcons),
        &mut requests,
    );
    operations.draw(ui, &session, &target.root, &mut requests);
    operations::guard_document_requests(&target, &session, &mut requests);
    for request in requests {
        world.resource_mut::<HostEffects>().push(None, request);
    }
}

fn open_dropped_files(world: &mut World) {
    let mut state = SystemState::<(
        Res<Messages<bed_api::workbench::PanelDrop>>,
        ResMut<DropCursor>,
        NonSend<ExplorerState>,
        Res<ExplorerTarget>,
        Res<FileContext>,
        Res<FileServices>,
        NonSend<DocumentSession>,
        ResMut<HostEffects>,
    )>::new(world);
    let (drops, mut cursor, explorer, target, file_context, file_services, documents, mut intents) =
        state.get_mut(world).unwrap();
    let tree = &explorer.file_explorer.file_tree;
    let target = target.resolve(&file_context, &file_services);
    for drop in cursor.0.read(&drops) {
        if drop.payload_type != "BED_FILES" {
            continue;
        }
        if !operations::provider_matches(&target.services, &documents) {
            intents.push(
                drop.panel,
                HostAction::Notify {
                    message: operations::DOCUMENT_PROVIDER_UNAVAILABLE.into(),
                },
            );
            continue;
        }
        if let Some(paths) = tree.dragged_paths() {
            for (offset, path) in paths
                .iter()
                .filter(|path| tree.is_directory(path) != Some(true))
                .enumerate()
            {
                intents.push_effect(bed_api::features::HostEffect {
                    source: drop.panel,
                    area: Some(drop.area),
                    tab_index: drop.tab_index.map(|index| index + offset),
                    action: HostAction::DropFile { path: path.clone() },
                });
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
    In(restore): In<FeatureStateRestore>,
    mut handle: NonSendMut<ExplorerState>,
    mut operations: NonSendMut<operations::FileOperations>,
    mut documents: NonSendMut<DocumentSession>,
    mut transport: NonSendMut<Transport>,
    (target, file_context, file_services): (
        Res<ExplorerTarget>,
        Res<FileContext>,
        Res<FileServices>,
    ),
    defaults: Res<ExplorerDefaults>,
) -> Result<(), String> {
    let target = target.resolve(&file_context, &file_services);
    let mut preferences = restore
        .state
        .as_ref()
        .map(FileTreePreferences::from_value)
        .unwrap_or_default();
    preferences.apply_defaults(defaults.hide_hidden, defaults.hide_gitignored);
    if !operations.bind_target(&target, &mut documents) {
        operations.cancel(&mut documents);
    }
    transport.0 = target.services.clone();
    configure_target(&mut handle, &target, preferences);
    Ok(())
}

/// Changing the filesystem target and restoring preferences share one binding
/// transition, so both discard stale navigation and transport work.
fn configure_target(
    handle: &mut ExplorerState,
    target: &ExplorerTarget,
    preferences: FileTreePreferences,
) {
    handle.file_content.reset();
    handle.actions.clear();
    let state = &mut handle.file_explorer;
    state.project_root = target.root.clone();
    state.file_finder.cancel_and_close();
    state.file_finder.set_project_dir("");
    state.file_finder.set_file_services(target.services.clone());
    state.file_finder.set_directory_label(None);
    state.file_tree = file_tree::FileTree {
        root_node: FileNode {
            name: std::path::Path::new(&state.project_root)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| state.project_root.clone()),
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use bed_api::documents::PanelBinding;
    use bed_workbench::{WorkbenchPlugin, WorkbenchRequests, commit_close};

    #[test]
    fn a_local_file_panel_does_not_block_moving_files_or_database_sidecars() {
        use std::time::{Duration, Instant};
        let directory = crate::test_support::TempDir::new();
        let database = directory.write("database.sqlite", b"database");
        let wal = directory.write("database.sqlite-wal", b"journal");
        let shm = directory.write("database.sqlite-shm", b"shared memory");
        let destination = directory.path("moved");
        std::fs::create_dir(&destination).unwrap();
        let mut app = App::new();
        app.insert_resource(FileContext::local(directory.root()).unwrap())
            .add_plugins(FileExplorerPlugin);
        // A direct file viewer keeps a path, without owning a document buffer.
        let panel = app
            .world_mut()
            .spawn(PanelBinding {
                input: bed_api::documents::PanelInput::LocalFile(database.clone()),
                ..Default::default()
            })
            .id();
        prepare(app.world_mut());
        app.world_mut()
            .non_send_mut::<ExplorerState>()
            .actions
            .push(ExplorerAction::Tree(FileTreeAction::Move {
                paths: [&database, &wal, &shm]
                    .map(|path| path.to_string_lossy().into_owned())
                    .into(),
                destination: destination.to_string_lossy().into_owned(),
            }));
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            prepare(app.world_mut());
            if !app.world().non_send::<operations::FileOperations>().busy()
                && [&database, &wal, &shm].iter().all(|path| !path.exists())
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "local file panels must not reserve paths against normal Files operations: {:?}",
                app.world_mut().resource_mut::<HostEffects>().take()
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        for (name, content) in [
            ("database.sqlite", b"database".as_slice()),
            ("database.sqlite-wal", b"journal".as_slice()),
            ("database.sqlite-shm", b"shared memory".as_slice()),
        ] {
            assert_eq!(std::fs::read(destination.join(name)).unwrap(), content);
        }
        assert_eq!(
            app.world().get::<PanelBinding>(panel).unwrap().input,
            bed_api::documents::PanelInput::LocalFile(database)
        );
    }

    #[test]
    fn a_temporary_directory_is_browsable_without_a_saved_workspace_or_host_module() {
        use std::time::{Duration, Instant};
        let directory = crate::test_support::TempDir::new();
        let path = std::fs::canonicalize(directory.write("standalone.txt", b"content")).unwrap();
        let context = FileContext::local(directory.root()).unwrap();
        let root = context.directory_string();
        let mut app = App::new();
        app.insert_resource(context)
            .add_plugins((WorkbenchPlugin, FileExplorerPlugin));
        app.update();
        assert_eq!(
            app.world()
                .non_send::<ExplorerState>()
                .file_explorer
                .project_root,
            root
        );
        // Native watcher startup can take several seconds under concurrent tests.
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            app.update();
            let explorer = app.world().non_send::<ExplorerState>();
            if explorer
                .file_explorer
                .file_tree
                .root_node
                .children
                .iter()
                .any(|node| node.full_path == path.to_string_lossy())
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "temporary directory discovery did not finish: tree={:?}, status={:?}",
                explorer.file_explorer.file_tree.root_node,
                explorer.file_explorer.file_finder.discovery_status,
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(app.world().resource::<FileServices>().connected());
    }

    #[cfg(unix)]
    #[test]
    fn a_remote_target_with_local_documents_never_inspects_or_opens_local_paths() {
        let temp = crate::test_support::TempDir::new();
        let file = temp.write("coincident.txt", b"local content");
        let client = crate::test_remote_client();
        client.disconnect();
        let mut app = App::new();
        app.add_plugins(FileExplorerPlugin)
            .insert_resource(ExplorerTarget {
                root: temp.root().to_string_lossy().into_owned(),
                services: FileServices::remote("explorer-fixture", client),
            });
        prepare(app.world_mut());
        assert!(!app.world().non_send::<DocumentSession>().is_remote());
        assert!(matches!(
            app.world_mut()
                .non_send_mut::<ExplorerState>()
                .file_content
                .inspect(&file.to_string_lossy()),
            crate::file_content::ContentStatus::Unavailable(_)
        ));
        for action in [
            FileTreeAction::Open(file.to_string_lossy().into_owned()),
            FileTreeAction::Preview(file.to_string_lossy().into_owned()),
        ] {
            app.world_mut()
                .non_send_mut::<ExplorerState>()
                .actions
                .push(ExplorerAction::Tree(action));
        }
        prepare(app.world_mut());
        let intents = app.world_mut().resource_mut::<HostEffects>().take();
        assert!(intents.iter().all(|intent| !matches!(
            intent.action,
            HostAction::OpenFileIn { .. } | HostAction::PreviewFileIn { .. }
        )));
        assert!(intents.iter().any(|intent| matches!(&intent.action, HostAction::Notify { message } if message.contains("document access"))));
        assert!(
            app.world()
                .non_send::<DocumentSession>()
                .document_ids()
                .is_empty()
        );
    }

    #[test]
    fn panel_and_finder_draw_current_icons_without_a_duplicate_root_label() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let temp = crate::test_support::TempDir::new();
        let file = temp.write("example.txt", b"example");
        let root = temp.root().to_string_lossy().into_owned();
        let old = dear_imgui_rs::TextureId::from(23usize);
        let current = dear_imgui_rs::TextureId::from(148usize);
        let tree_icons = [
            (
                "bed.explorer.folder-open",
                dear_imgui_rs::TextureId::from(200usize),
            ),
            (
                "bed.explorer.folder",
                dear_imgui_rs::TextureId::from(201usize),
            ),
            (
                "bed.explorer.chevron-down",
                dear_imgui_rs::TextureId::from(202usize),
            ),
            (
                "bed.explorer.chevron-right",
                dear_imgui_rs::TextureId::from(203usize),
            ),
        ];
        let mut app = App::new();
        let mut icons = Icons::default();
        icons.set_texture("bed.core.default", old);
        app.world_mut().insert_non_send(icons);
        app.add_plugins((WorkbenchPlugin, FileExplorerPlugin));
        app.world_mut()
            .resource_mut::<WorkbenchRequests>()
            .push(WorkbenchAction::OpenPanel {
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
        app.world_mut().resource_mut::<ExplorerTarget>().root = root.clone();
        prepare(app.world_mut());
        {
            let mut explorer = app.world_mut().non_send_mut::<ExplorerState>();
            explorer.file_explorer.project_root = root.clone();
            explorer.file_explorer.file_tree.root_node = FileNode {
                name: "project".into(),
                full_path: root.clone(),
                is_directory: true,
                is_open: true,
                children: vec![
                    FileNode {
                        name: "src".into(),
                        full_path: format!("{root}/src"),
                        is_directory: true,
                        ..Default::default()
                    },
                    FileNode {
                        name: "example.txt".into(),
                        full_path: file.to_string_lossy().into_owned(),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            };
        }
        // The canvas stage may replace all GPU registrations after feature
        // preparation and before UI. The frontend must read this new mapping.
        {
            let mut icons = app.world_mut().non_send_mut::<Icons>();
            icons.set_texture("bed.core.default", current);
            for (key, texture) in tree_icons {
                // Match desktop upload: only registered source artwork acquires
                // a texture. This also exercises the plugin's icon contract.
                if icons.images.contains_key(key) {
                    icons.set_texture(key, texture);
                }
            }
        }
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
                let mut content_origin = [0.0; 2];
                let mut line_height = 0.0;
                if finder {
                    draw_popups(app.world_mut(), ui);
                } else {
                    ui.window("Files")
                        .position([0.0; 2], dear_imgui_rs::Condition::Always)
                        .size([600.0, 500.0], dear_imgui_rs::Condition::Always)
                        .build(|| {
                            content_origin = ui.cursor_screen_pos();
                            line_height = ui.text_line_height_with_spacing();
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
                if !finder {
                    for (key, texture) in tree_icons {
                        assert!(textures.contains(&texture), "tree artwork absent: {key}");
                    }
                    let root_top = draw
                        .draw_lists()
                        .flat_map(|list| {
                            list.commands().filter_map(move |command| match command {
                                dear_imgui_rs::DrawCmd::Elements { count, cmd_params }
                                    if cmd_params.texture_id == tree_icons[0].1 =>
                                {
                                    Some(
                                        list.idx_buffer()
                                            [cmd_params.idx_offset..cmd_params.idx_offset + count]
                                            .iter()
                                            .map(|index| {
                                                list.vtx_buffer()
                                                    [cmd_params.vtx_offset + usize::from(*index)]
                                                .pos[1]
                                            })
                                            .fold(f32::INFINITY, f32::min),
                                    )
                                }
                                _ => None,
                            })
                        })
                        .fold(f32::INFINITY, f32::min);
                    assert!(
                        root_top < content_origin[1] + line_height,
                        "the root row must start at the panel's first line; extra path label moved it to {root_top} from {}",
                        content_origin[1]
                    );
                }
            }
        }
    }

    #[test]
    fn registered_files_panels_and_prepare_share_the_supplied_controller() {
        let mut app = App::new();
        app.world_mut().insert_non_send(DocumentSession::new());
        app.add_plugins((WorkbenchPlugin, FileExplorerPlugin));
        for _ in 0..2 {
            app.world_mut()
                .resource_mut::<WorkbenchRequests>()
                .push(WorkbenchAction::Invoke(CommandInvocation {
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
        assert!(app.world_mut().resource_mut::<HostEffects>().take().iter().any(|intent|matches!(&intent.action,HostAction::Notify{message} if message=="filesystem watch failed")));
        assert!(commit_close(app.world_mut(), panels[0]));
        app.update();
        assert_eq!(app.world().non_send::<ExplorerPanels>().0.len(), 1);
        assert!(
            app.world()
                .get::<bed_api::workbench::PanelPresentation>(panels[1])
                .is_some()
        );
    }

    #[test]
    fn registered_workspace_restore_resets_navigation_and_round_trips_preferences() {
        let mut app = App::new();
        app.add_plugins(FileExplorerPlugin);
        let contribution = *app
            .world_mut()
            .query::<&FeatureStateContribution>()
            .single(app.world())
            .unwrap();
        app.world_mut().resource_mut::<ExplorerTarget>().root = "/new-project".into();
        // New targets and snapshots predating these filters use the defaults.
        for state in [
            None,
            Some(serde_json::json!({"include_ignored":true})),
            Some(
                serde_json::json!({"include_ignored":true,"hide_hidden":true,"hide_gitignored":true}),
            ),
        ] {
            app.world_mut()
                .run_system_with(contribution.restore, FeatureStateRestore { state })
                .unwrap()
                .unwrap();
            let saved = app.world_mut().run_system(contribution.save).unwrap();
            let tree = &app.world().non_send::<ExplorerState>().explorer().file_tree;
            assert!(tree.preferences.hide_hidden);
            assert!(tree.preferences.hide_gitignored);
            assert!(saved.get("hide_hidden").is_none());
            assert!(saved.get("hide_gitignored").is_none());
        }
        app.world_mut()
            .non_send_mut::<ExplorerState>()
            .submit(
                vec![
                    FileTreeAction::SetHideHidden(false),
                    FileTreeAction::SetHideGitignored(false),
                    FileTreeAction::SetShowHidden(true),
                ],
                false,
                &mut Vec::new(),
            )
            .unwrap();
        app.update();
        let changes = app
            .world()
            .resource::<Messages<bed_api::storage::FeatureStateChanged>>();
        assert!(
            changes
                .get_cursor()
                .read(changes)
                .any(|change| change.owner == MODULE_ID)
        );
        let saved = app.world_mut().run_system(contribution.save).unwrap();
        assert_eq!(saved["hide_hidden"], false);
        assert_eq!(saved["hide_gitignored"], false);
        assert_eq!(saved["include_ignored"], true);
        assert!(saved.get("show_hidden").is_none());
        // Switching workspaces resets the temporary reveal and restores defaults.
        for (root, state) in [
            ("/other-project", None),
            (
                "/legacy-project",
                Some(serde_json::json!({"hide_hidden":false,"hide_gitignored":false})),
            ),
            ("/new-project", Some(saved)),
        ] {
            app.world_mut().resource_mut::<ExplorerTarget>().root = root.into();
            let expected_hide = state.is_none();
            app.world_mut()
                .run_system_with(contribution.restore, FeatureStateRestore { state })
                .unwrap()
                .unwrap();
            let explorer = app.world().non_send::<ExplorerState>().explorer();
            assert_eq!(explorer.file_tree.preferences.hide_hidden, expected_hide);
            assert_eq!(
                explorer.file_tree.preferences.hide_gitignored,
                expected_hide
            );
            assert!(!explorer.file_tree.show_hidden);
        }
        assert_eq!(
            app.world()
                .non_send::<ExplorerState>()
                .explorer()
                .project_root,
            "/new-project"
        );
    }

    #[test]
    fn registered_file_settings_update_defaults_without_overwriting_workspace_choices() {
        use dear_imgui_rs::{Condition, Context, FramePrepareOptions, MouseButton};

        fn frame(
            context: &mut Context,
            draw: fn(&Ui, &mut Value) -> bool,
            values: &mut Value,
        ) -> ([[f32; 2]; 2], bool) {
            context.prepare_frame(FramePrepareOptions::new([400.0, 300.0], 1.0 / 60.0));
            let ui = context.frame();
            let mut points = [[0.0; 2]; 2];
            let mut changed = false;
            ui.window("Files settings")
                .position([0.0; 2], Condition::Always)
                .size([400.0, 300.0], Condition::Always)
                .build(|| {
                    let origin = ui.cursor_screen_pos();
                    let height = ui.frame_height();
                    let spacing = ui.clone_style().item_spacing()[1];
                    points = std::array::from_fn(|row| {
                        [
                            origin[0] + 5.0,
                            origin[1] + row as f32 * (height + spacing) + height * 0.5,
                        ]
                    });
                    changed = draw(ui, values);
                });
            drop(context.render_legacy());
            (points, changed)
        }

        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut app = App::new();
        app.add_plugins(FileExplorerPlugin);
        let section = app
            .world()
            .resource::<bed_api::features::FeatureRegistry>()
            .settings()[0]
            .clone();
        assert_eq!(section.plugin, MODULE_ID);
        let draw = section.draw.expect("Files settings have visible controls");
        let mut context = Context::create();
        context
            .set_ini_filename(None::<std::path::PathBuf>)
            .unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        let mut values = serde_json::json!({});
        frame(&mut context, draw, &mut values);
        let (points, changed) = frame(&mut context, draw, &mut values);
        assert!(!changed);
        for (row, key) in ["hide_hidden", "hide_gitignored"].into_iter().enumerate() {
            context.io_mut().add_mouse_pos_event(points[row]);
            frame(&mut context, draw, &mut values);
            context
                .io_mut()
                .add_mouse_button_event(MouseButton::Left, true);
            assert!(!frame(&mut context, draw, &mut values).1);
            context
                .io_mut()
                .add_mouse_button_event(MouseButton::Left, false);
            assert!(frame(&mut context, draw, &mut values).1);
            assert_eq!(values[key], false, "both settings start enabled");
        }
        let contribution = *app
            .world_mut()
            .query::<&FeatureStateContribution>()
            .single(app.world())
            .unwrap();
        app.world_mut().insert_resource(ExplorerDefaults {
            hide_hidden: values["hide_hidden"].as_bool().unwrap(),
            hide_gitignored: values["hide_gitignored"].as_bool().unwrap(),
        });
        app.world_mut()
            .run_system_with(
                contribution.restore,
                FeatureStateRestore {
                    state: Some(
                        serde_json::json!({"include_ignored": true, "hidden_paths": ["private"], "hide_hidden": true, "hide_gitignored": true}),
                    ),
                },
            )
            .unwrap()
            .unwrap();
        {
            let tree = &app.world().non_send::<ExplorerState>().explorer().file_tree;
            assert!(!tree.preferences.hide_hidden);
            assert!(!tree.preferences.hide_gitignored);
        }
        let inherited = app.world_mut().run_system(contribution.save).unwrap();
        assert!(inherited.get("hide_hidden").is_none());
        assert!(inherited.get("hide_gitignored").is_none());
        assert_eq!(inherited["hidden_paths"], serde_json::json!(["private"]));
        // A live settings change updates inherited filters, including after restore.
        app.world_mut().insert_resource(ExplorerDefaults::default());
        prepare(app.world_mut());
        {
            let tree = &app.world().non_send::<ExplorerState>().explorer().file_tree;
            assert!(tree.preferences.hide_hidden);
            assert!(tree.preferences.hide_gitignored);
        }
        // An explicit choice overrides its default; the other filter still inherits.
        app.world_mut().insert_resource(ExplorerDefaults {
            hide_hidden: false,
            hide_gitignored: false,
        });
        prepare(app.world_mut());
        app.world_mut()
            .non_send_mut::<ExplorerState>()
            .submit(
                vec![FileTreeAction::SetHideHidden(true)],
                false,
                &mut Vec::new(),
            )
            .unwrap();
        let explicit = app.world_mut().run_system(contribution.save).unwrap();
        assert_eq!(explicit["hide_hidden"], true);
        assert!(explicit.get("hide_gitignored").is_none());
        app.world_mut()
            .run_system_with(
                contribution.restore,
                FeatureStateRestore {
                    state: Some(explicit),
                },
            )
            .unwrap()
            .unwrap();
        prepare(app.world_mut());
        let tree = &app.world().non_send::<ExplorerState>().explorer().file_tree;
        assert!(tree.preferences.hide_hidden);
        assert!(!tree.preferences.hide_gitignored);
    }

    #[test]
    fn registered_native_drop_uses_this_presentation_bounds_and_rejects_stale_workspace() {
        let mut app = App::new();
        app.add_plugins((WorkbenchPlugin, FileExplorerPlugin));
        app.world_mut().resource_mut::<ExplorerTarget>().root = "/project".into();
        app.world_mut()
            .resource_mut::<WorkbenchRequests>()
            .push(WorkbenchAction::OpenPanel {
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
        let handler = app.world().get::<PanelExternalFiles>(entity).unwrap().0;
        let event = bed_api::workbench::ExternalFileDrag {
            paths: vec!["/incoming.txt".into()],
            position: [50.0, 50.0],
            viewport: 1,
            phase: bed_api::workbench::ExternalFileDragPhase::Drop,
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
            bed_api::workbench::ExternalFileDropResponse::Accepted
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
            bed_api::workbench::ExternalFileDropResponse::Ignored
        );
        assert!(
            app.world_mut()
                .non_send_mut::<ExplorerState>()
                .take_actions()
                .is_empty()
        );
    }
}
