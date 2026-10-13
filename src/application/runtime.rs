//! Canonical application resources and one ordered application transition boundary.
mod layouts;
pub use layouts::LayoutSlots;

use crate::application::workspace::store::{WorkspaceSpec, WorkspaceStore, WorkspaceTarget};
pub use crate::config::LayoutSlot;
use crate::config::Settings;
#[cfg(all(test, feature = "workspaces"))]
use bed_api::storage::{FeatureStateContribution, FeatureStateRestore};
use bed_api::workbench::{
    CommandInvocation, PanelCreation, PanelOwner, PanelPresentation, WorkbenchAction,
};
use bed_api::{
    documents::{
        CommandContext, LspRequestOrigin, PanelBinding, PanelInput, Revision, SavedDocument,
    },
    features::{HostAction, HostEffects},
    workbench::{
        PanelAction, PanelActionInvocation, PanelActions, PanelClose, PanelFocus, PanelTarget,
    },
};
use bed_documents::session::{
    ByteEdit, ClosePolicy, DocumentId, DocumentSession, SessionOptions, ViewId,
};
use bed_plugin_workspaces::{
    WorkspaceAction, WorkspaceAttachment, WorkspaceRepositoryResource, WorkspaceRequest,
};
use bed_workbench::{
    PanelRecord, WorkbenchLayout, WorkbenchOutcome, WorkbenchRequestId, WorkbenchRequests,
};
use bevy::{ecs::schedule::ScheduleLabel, prelude::*};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    io,
    path::{Path, PathBuf},
    time::Instant,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WorkbenchHostMode {
    #[default]
    Fullscreen,
    Floating,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowCommand {
    Debug,
    NewDocument,
    NewTerminal,
    NewExplorer,
    NewSettings,
    NewWorkspaces,
    NewDiagnostics,
    NewStructure,
    NewLspDashboard,
    NewContentSearch,
    NewReferences,
    DuplicateView,
    SplitRight,
    SplitDown,
    SetCurrentLayout,
    ResetLayout,
    UseLayout(LayoutSlot),
    SetLayoutDefault(LayoutSlot),
    RestoreBuiltinLayout(LayoutSlot),
    SaveDefaultLayout,
    ResetDefaultLayout,
    Workspaces,
    Diagnostics,
    Structure,
    OpenFolder,
    OpenFile,
    Save,
    SaveAs,
    Close,
    Quit,
    Find,
    GoToLine,
    Explorer,
    Terminal,
    Settings,
    LspDashboard,
    FindFile,
    FindWorkspace,
}

#[derive(Clone, Debug)]
pub struct OpenFileOptions {
    pub path: PathBuf,
    pub viewer: Option<String>,
    pub additional: bool,
}

#[derive(Clone, Debug, Default)]
pub struct AppOptions {
    pub working_directory: Option<PathBuf>,
    pub workspace: Option<WorkspaceSpec>,
    pub files: Vec<OpenFileOptions>,
    /// Restore the last standalone session only when explicitly requested.
    /// Initial arrangement for a fresh standalone host.
    pub initial_layout: Option<LayoutSlot>,
    /// Initial tool, opened before file arguments so those files retain focus.
    pub startup_panel: Option<String>,
    pub host_mode: WorkbenchHostMode,
}

#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActiveSessionId(pub u64);

#[derive(Resource, Clone, Debug)]
pub struct WorkspaceState {
    pub spec: Option<WorkspaceSpec>,
    pub project_root: String,
    pub working_directory: PathBuf,
    pub error: Option<String>,
    pub close_requested: bool,
    pub windows: Vec<WorkspaceSpec>,
}

#[derive(Resource, Clone, Default, Debug)]
pub struct DocumentNavigation {
    pub active_view: Option<ViewId>,
    pub active_document: Option<DocumentId>,
    pub last_document_panel: Option<Entity>,
}

pub struct WorkspacePersistence {
    pub(crate) store: WorkspaceStore,
    last_saved: Instant,
    last_value: Option<Value>,
}

impl WorkspacePersistence {
    pub fn new(store: WorkspaceStore) -> Self {
        Self {
            store,
            last_saved: Instant::now(),
            last_value: None,
        }
    }

    /// Select persistence before panel plugins can install their in-memory defaults.
    pub(crate) fn install(app: &mut App, store: WorkspaceStore) {
        #[cfg(feature = "workspaces")]
        app.insert_resource(WorkspaceRepositoryResource(Box::new(store.repository())));
        app.insert_non_send(Self::new(store));
    }
}

/// The application locates packaged helpers before installing editor integration.
#[derive(Resource, Clone)]
pub struct RemoteHelperDirectory(pub PathBuf);

pub fn panel_presentation(world: &World, panel: Entity) -> Option<Entity> {
    if let Some(frontend) = world.get::<bed_api::workbench::PanelFrontend>(panel) {
        Some(frontend.0)
    } else if bed_api::workbench::panel_owner(world, panel).is_some() {
        Some(panel)
    } else {
        None
    }
}

#[derive(Debug)]
pub enum AppAction {
    Workspace(WorkspaceAction),
    WorkspaceWithBarrier {
        action: WorkspaceAction,
        token: bed_api::features::OperationBarrierToken,
    },
    Settings(bed_api::settings::SettingsRequest),
    Command(WindowCommand),
    PluginCommand(String),
    WorkbenchAction(WorkbenchAction),
    HostAction(HostAction),
    HostActions {
        actions: Vec<HostAction>,
    },
    PanelAction {
        panel: Entity,
        action: PanelAction,
    },
    ExternalFiles {
        panel: Option<Entity>,
        event: bed_api::workbench::ExternalFileDrag,
    },
    FocusPanel(Entity),
    ObserveFocus(Entity),
    FocusArea(u32),
    ClosePanel(Entity),
    ClosePanels(Vec<Entity>),
    OpenFile(PathBuf),
    OpenFileWithViewer {
        path: PathBuf,
        viewer: Option<String>,
        additional: bool,
        move_existing: bool,
    },
    OpenViewer {
        panel: Entity,
        viewer: String,
    },
    AttachWorkspace(PathBuf),
    SetWorkspace(WorkspaceSpec),
    ApplyEdits {
        document: DocumentId,
        revision: Revision,
        edits: Vec<ByteEdit>,
    },
    SaveDocuments(Vec<DocumentId>),
    SaveAs {
        document: DocumentId,
        path: PathBuf,
    },
}

#[derive(Debug)]
pub struct AppRequest {
    pub action: AppAction,
    pub session_id: u64,
    pub layout_slot: LayoutSlot,
    pub source: Option<Entity>,
    pub panel: Option<Entity>,
    pub area: u32,
    pub tab_index: Option<usize>,
    pub context: CommandContext,
    pub arguments: Value,
}

/// Capture is a read operation; requests never borrow or change later focus.
pub fn capture_app_request(world: &World, action: AppAction, target: PanelTarget) -> AppRequest {
    capture_app_request_from(
        world,
        action,
        target,
        world.resource::<WorkbenchLayout>().focused,
    )
}

pub fn capture_app_request_from(
    world: &World,
    action: AppAction,
    target: PanelTarget,
    source: Option<Entity>,
) -> AppRequest {
    let target = explicit_panel_target(&action).unwrap_or(target);
    let panel = source.and_then(|source| bed_api::workbench::panel_owner(world, source));
    let presentation = panel.and_then(|panel| panel_presentation(world, panel));
    let binding = source
        .and_then(|entity| world.get::<PanelBinding>(entity))
        .or_else(|| presentation.and_then(|entity| world.get::<PanelBinding>(entity)))
        .or_else(|| panel.and_then(|entity| world.get::<PanelBinding>(entity)));
    let session = world.non_send::<DocumentSession>();
    let mut context = binding
        .map(|binding| CommandContext {
            path: binding
                .input
                .local_file()
                .map(|path| path.to_string_lossy().into_owned())
                .or_else(|| {
                    binding.document.and_then(|document| {
                        session
                            .with_document(document, |state| state.path.clone())
                            .ok()
                    })
                }),
            document: binding.document,
            revision: binding
                .document
                .and_then(|document| session.document_revision(document).ok()),
            selection: None,
        })
        .unwrap_or_default();
    if let Some(view) = binding.and_then(|binding| binding.view)
        && let Ok(captured) = bed_api::documents::selection_context(session, view)
    {
        context = captured;
    }
    if context.document.is_none() {
        let model = source
            .and_then(|entity| world.get::<bed_core::DocumentBinding>(entity))
            .or_else(|| {
                presentation.and_then(|entity| world.get::<bed_core::DocumentBinding>(entity))
            })
            .or_else(|| panel.and_then(|entity| world.get::<bed_core::DocumentBinding>(entity)));
        if let Some(model) = model {
            context.document = Some(model.0);
            context.revision = bed_core::document_revision(world, model.0).ok();
            context.path = bed_core::document_path(world, model.0)
                .ok()
                .flatten()
                .map(|path| path.to_string_lossy().into_owned());
        }
    }
    let remembers_document = match &action {
        AppAction::Command(WindowCommand::Save | WindowCommand::SaveAs) => true,
        AppAction::PluginCommand(command) => matches!(
            command.as_str(),
            "bed.documents.save" | "bed.documents.save_as"
        ),
        AppAction::WorkbenchAction(WorkbenchAction::Invoke(invocation)) => matches!(
            invocation.command.as_str(),
            "bed.documents.save" | "bed.documents.save_as"
        ),
        _ => false,
    };
    if context.document.is_none()
        && remembers_document
        && binding.is_none_or(|binding| binding.input.local_file().is_none())
    {
        let navigation = world.resource::<DocumentNavigation>();
        context.document = navigation.active_document;
        context.revision = context
            .document
            .and_then(|document| session.document_revision(document).ok());
        context.path = context
            .document
            .and_then(|document| session.snapshot(document).ok())
            .map(|snapshot| snapshot.path);
    }
    let layout = world.resource::<WorkbenchLayout>();
    let target_panel = match target {
        PanelTarget::LastFocused => panel.or(layout.focused),
        PanelTarget::LargestArea => None,
    };
    let area = target_panel
        .and_then(|entity| world.get::<PanelPresentation>(entity))
        .and_then(|presentation| layout.tiling.area_for(presentation.id))
        .or_else(|| {
            (target == PanelTarget::LastFocused)
                .then_some(layout.focused_area)
                .flatten()
        })
        .unwrap_or_else(|| layout.tiling.layout.largest_area());
    AppRequest {
        action,
        session_id: session.session_id().0,
        layout_slot: world.resource::<LayoutSlots>().active(),
        source,
        panel,
        area,
        tab_index: None,
        context,
        arguments: Value::Null,
    }
}

fn explicit_panel_target(action: &AppAction) -> Option<PanelTarget> {
    match action {
        AppAction::HostAction(
            HostAction::ShowPanelIn { target, .. }
            | HostAction::OpenFileIn { target, .. }
            | HostAction::PreviewFileIn { target, .. },
        ) => Some(*target),
        AppAction::HostAction(HostAction::NavigateLocation { .. }) => {
            Some(PanelTarget::LastFocused)
        }
        AppAction::HostAction(
            HostAction::OpenTerminalFiles { .. }
            | HostAction::OpenTerminalLink { .. }
            | HostAction::PreviewSource { .. },
        ) => Some(PanelTarget::LargestArea),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AppRequestId(pub u64);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppRequestError {
    pub kind: io::ErrorKind,
    pub message: String,
}

impl std::fmt::Display for AppRequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for AppRequestError {}

impl From<io::Error> for AppRequestError {
    fn from(error: io::Error) -> Self {
        Self {
            kind: error.kind(),
            message: error.to_string(),
        }
    }
}

impl From<String> for AppRequestError {
    fn from(message: String) -> Self {
        Self {
            kind: io::ErrorKind::Other,
            message,
        }
    }
}

impl From<&str> for AppRequestError {
    fn from(message: &str) -> Self {
        message.to_owned().into()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppRequestOutcome {
    Applied,
    Ignored,
    Opened {
        panel: Entity,
        document: Option<DocumentId>,
    },
    Saved {
        documents: Vec<SavedDocument>,
    },
    Closed {
        panels: Vec<Entity>,
    },
}

struct QueuedRequest {
    sequence: u64,
    layout_epoch: Option<u64>,
    id: Option<AppRequestId>,
    request: AppRequest,
}

#[derive(Resource, Default)]
pub struct AppRequests {
    next_id: u64,
    pending: VecDeque<QueuedRequest>,
    results: HashMap<AppRequestId, Result<AppRequestOutcome, AppRequestError>>,
    generic: HashMap<AppRequestId, WorkbenchRequestId>,
    documents: bevy::ecs::message::MessageCursor<bed_api::features::HostRequest>,
    workspaces: bevy::ecs::message::MessageCursor<WorkspaceRequest>,
    settings: bevy::ecs::message::MessageCursor<bed_api::settings::SettingsRequest>,
}

impl AppRequests {
    pub fn enqueue(&mut self, request: AppRequest) {
        self.pending.push_back(QueuedRequest {
            sequence: match &request.action {
                AppAction::Settings(settings) => settings.sequence,
                _ => bed_workbench::next_request_sequence(),
            },
            layout_epoch: None,
            id: None,
            request,
        });
    }

    pub fn submit(&mut self, request: AppRequest) -> AppRequestId {
        self.next_id = self
            .next_id
            .checked_add(1)
            .expect("app request ID overflow");
        let id = AppRequestId(self.next_id);
        self.pending.push_back(QueuedRequest {
            sequence: match &request.action {
                AppAction::Settings(settings) => settings.sequence,
                _ => bed_workbench::next_request_sequence(),
            },
            layout_epoch: None,
            id: Some(id),
            request,
        });
        id
    }

    pub fn take_result(&mut self, id: AppRequestId) -> Option<Result<AppRequestOutcome, String>> {
        self.results
            .remove(&id)
            .map(|result| result.map_err(|error| error.message))
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}

pub fn submit_app_request(world: &mut World, request: AppRequest) -> AppRequestId {
    world.resource_mut::<AppRequests>().submit(request)
}

pub fn poll_app_request_result(
    world: &mut World,
    id: AppRequestId,
) -> Option<Result<AppRequestOutcome, AppRequestError>> {
    world.resource_mut::<AppRequests>().results.remove(&id)
}

pub fn poll_app_request(
    world: &mut World,
    id: AppRequestId,
) -> Option<Result<AppRequestOutcome, String>> {
    world.resource_mut::<AppRequests>().take_result(id)
}

#[derive(ScheduleLabel, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AppRequestsSchedule;

#[derive(ScheduleLabel, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AppServicesSchedule;

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AppSystems {
    Services,
    Requests,
    Frame,
}

#[derive(Default)]
pub struct AppRuntimePlugin {
    pub options: AppOptions,
}

impl Plugin for AppRuntimePlugin {
    fn build(&self, app: &mut App) {
        #[cfg(feature = "workspaces")]
        let supplied_repository = app
            .world()
            .contains_resource::<WorkspaceRepositoryResource>();
        app.insert_resource(bed_core::CorePresentationMode::Application)
            .insert_resource(bed_core::DocumentRequestScheduling::External);
        if !app.is_plugin_added::<bed_core::DocumentHostPlugin>() {
            app.add_plugins(bed_core::DocumentHostPlugin);
        }
        #[cfg(feature = "workspaces")]
        {
            app.insert_resource(bed_plugin_workspaces::WorkspaceLifecycleMode::Application);
            if !app.is_plugin_added::<bed_plugin_workspaces::WorkspacesIntegrationPlugin>() {
                app.add_plugins(bed_plugin_workspaces::WorkspacesIntegrationPlugin);
            }
        }
        app.add_message::<WorkspaceRequest>()
            .insert_resource(bed_core::DocumentFrameSelection::default())
            .init_resource::<AppRequests>()
            .init_resource::<DocumentNavigation>()
            .init_resource::<LayoutSlots>()
            .init_resource::<PendingOperations>()
            .init_resource::<AppInitialized>()
            .init_resource::<FeatureStateCursor>()
            .init_schedule(AppRequestsSchedule)
            .init_schedule(AppServicesSchedule)
            .add_systems(
                AppRequestsSchedule,
                apply_app_requests.in_set(AppSystems::Requests),
            )
            .add_systems(
                AppServicesSchedule,
                (bed_core::poll_document_services, update_app_services)
                    .chain()
                    .in_set(AppSystems::Services),
            )
            .add_systems(
                PreUpdate,
                update_app_services
                    .in_set(AppSystems::Services)
                    .after(bed_core::CoreSystems::Services)
                    .before(AppSystems::Frame),
            )
            .add_systems(
                PreUpdate,
                prepare_app_state
                    .in_set(AppSystems::Frame)
                    .before(bed_core::CoreSystems::Frame)
                    .before(bed_api::features::FeatureSystems::Prepare),
            )
            .add_systems(PostUpdate, apply_app_requests.in_set(AppSystems::Requests));
        let world = app.world_mut();
        crate::application::command_registration::register(world);
        assert!(
            world.contains_non_send::<Settings>(),
            "application settings are installed before app integration"
        );
        assert!(
            world.contains_non_send::<bed_ui::icons::Icons>(),
            "application icons are installed before app integration"
        );
        assert!(
            world.contains_non_send::<WorkspacePersistence>(),
            "loaded workspace persistence is installed before app integration"
        );
        assert!(
            world.contains_resource::<RemoteHelperDirectory>(),
            "application remote helper paths are installed before app integration"
        );
        world.init_non_send::<RemoteOperations>();
        let working_directory = self
            .options
            .working_directory
            .clone()
            .expect("application startup supplies the working directory");
        world.insert_resource(ActiveSessionId(
            world.non_send::<DocumentSession>().session_id().0,
        ));
        let (context, services) = if let (Some(target), Some(client)) = (
            world.non_send::<DocumentSession>().ssh_target().cloned(),
            world.non_send::<DocumentSession>().remote_client(),
        ) {
            let root = world
                .non_send::<DocumentSession>()
                .options()
                .project_root
                .clone()
                .unwrap_or_else(|| working_directory.clone());
            (
                bed_files::FileContext::remote(target.host.clone(), root)
                    .expect("injected remote context is canonical"),
                bed_files::FileServices::remote_target(target, client),
            )
        } else {
            (
                bed_files::FileContext::local(&working_directory)
                    .expect("startup directory is valid"),
                bed_files::FileServices::local(),
            )
        };
        world.insert_resource(WorkspaceState {
            spec: None,
            project_root: context.directory_string(),
            working_directory: context.directory.clone(),
            error: None,
            close_requested: false,
            windows: Vec::new(),
        });
        world
            .resource_mut::<bed_files::FileContext>()
            .replace(context);
        world.insert_resource(services);
        #[cfg(feature = "workspaces")]
        {
            let repository = world.non_send::<WorkspacePersistence>().store.repository();
            let state_store = world.non_send::<WorkspacePersistence>().store.state_store();
            if !supplied_repository {
                world.insert_resource(WorkspaceRepositoryResource(Box::new(repository)));
            }
            world.insert_resource(bed_storage::Storage(Box::new(state_store)));
        }
        world.insert_non_send(crate::application::ui::AppUi::new(self.options.host_mode));
        world.insert_resource(InitialAppOptions(Some(self.options.clone())));
        app.add_systems(Startup, initialize_app);
    }
}

#[derive(Resource, Default)]
struct AppInitialized(bool);

#[derive(Resource, Default)]
struct FeatureStateCursor(bevy::ecs::message::MessageCursor<bed_api::storage::FeatureStateChanged>);

/// bEd translates its selected project into each installed feature's actual input.
/// Reusable features can receive the same inputs from any other application.
pub fn configure_feature_targets(world: &mut World) {
    publish_file_context(world);
    #[cfg(feature = "git")]
    {
        let session = world.non_send::<DocumentSession>().session_id();
        if let Some(mut target) = world.get_resource_mut::<bed_plugin_git::GitTarget>() {
            target.session_id = Some(session);
        }
    }
    #[cfg(feature = "debugger")]
    {
        let context = world.resource::<bed_files::FileContext>();
        let directory = context.directory_string();
        let remote = matches!(context.provider, bed_files::FileProviderId::Ssh { .. });
        if let Some(mut target) = world.get_resource_mut::<bed_plugin_debugger::DebugTarget>() {
            target.directory = directory;
            target.remote = remote;
        }
    }
    #[cfg(feature = "http-client")]
    {
        let location = world
            .resource::<WorkspaceState>()
            .spec
            .as_ref()
            .map(|_| "in this workspace".to_owned());
        if let Some(mut persistence) =
            world.get_resource_mut::<bed_plugin_http_client::HttpPersistence>()
        {
            persistence.location = location;
        }
    }
}

fn publish_file_context(world: &mut World) {
    let workspace = world.resource::<WorkspaceState>();
    let session = world.non_send::<DocumentSession>();
    let (mut context, services) = if let Some(client) = session.remote_client() {
        let target = session
            .ssh_target()
            .expect("remote sessions retain their authenticated SSH target")
            .clone();
        let mut context = bed_files::FileContext::remote(
            target.host.clone(),
            PathBuf::from(&workspace.project_root),
        )
        .expect("connected remote directory is canonical");
        if !client.is_connected() {
            context.availability = bed_files::FileAvailability::Disconnected;
        }
        (
            context,
            bed_files::FileServices::remote_target(target, client),
        )
    } else {
        (
            bed_files::FileContext {
                provider: bed_files::FileProviderId::Local,
                directory: workspace.working_directory.clone(),
                availability: bed_files::FileAvailability::Available,
                generation: 1,
            },
            bed_files::FileServices::local(),
        )
    };
    if let Some(mut current) = world.get_resource_mut::<bed_files::FileContext>() {
        context.generation = current.generation;
        current.replace(context);
    } else {
        world.insert_resource(context);
    }
    if world
        .get_resource::<bed_files::FileServices>()
        .is_none_or(|current| {
            !current.same_provider(&services) || current.ssh_target() != services.ssh_target()
        })
    {
        world.insert_resource(services);
    }
    let connecting = world.non_send::<RemoteOperations>().connection.is_some();
    if let Some(mut observations) =
        world.get_resource_mut::<bed_plugin_workspaces::WorkspacesState>()
    {
        observations.connecting = connecting;
    }
}

#[derive(Resource)]
struct InitialAppOptions(Option<AppOptions>);

fn initialize_app(world: &mut World) {
    let Some(options) = world.resource_mut::<InitialAppOptions>().0.take() else {
        return;
    };
    let result = (|| -> io::Result<()> {
        crate::config::ensure_settings_view(world)?;
        bed_core::settings::configure_document_settings(world)?;
        let directory = world.resource::<WorkspaceState>().working_directory.clone();
        let mut session_options = session_options(world, Some(directory));
        session_options.history_path = None;
        world
            .non_send_mut::<DocumentSession>()
            .configure(session_options)?;
        // Initial factories need the configured working directory before the
        // first PreUpdate frame publisher has run.
        publish_app_state(world);
        if let Some(spec) = options.workspace {
            if !cfg!(feature = "workspaces") {
                return Err(io::Error::other("Workspace support is disabled"));
            }
            let existing_remote = matches!(&spec.target,WorkspaceTarget::Ssh {host}
                if world.non_send::<DocumentSession>().ssh_target().is_some_and(|target| &target.host==host)
                && world.non_send::<DocumentSession>().options().project_root.as_deref()==Some(Path::new(&spec.root)));
            if existing_remote {
                let saved = world
                    .resource::<WorkspaceRepositoryResource>()
                    .0
                    .find(&spec)?
                    .ok_or_else(|| {
                        io::Error::new(
                            io::ErrorKind::NotFound,
                            "Injected remote workspace has not been saved",
                        )
                    })?;
                world.resource_mut::<WorkspaceState>().spec = Some(saved.clone());
                world.insert_resource(WorkspaceAttachment::Saved(saved));
                restore_workspace(world)?;
            } else {
                replace_workspace(world, spec)?;
            }
        } else if let Some(slot) = options.initial_layout {
            layouts::initialize(world, slot)?;
        }
        if let Some(startup) = options.startup_panel.as_deref()
            && panel_entities(world).is_empty()
            && world
                .resource::<bed_api::workbench::WorkbenchRegistry>()
                .panel(startup)
                .is_some()
        {
            open_panel(
                world,
                startup,
                PanelInput::None,
                Value::Null,
                None,
                None,
                false,
            )?;
        }
        for file in options.files {
            begin_open_file(
                world,
                &file.path,
                file.viewer.as_deref(),
                file.additional,
                false,
                None,
                None,
                None,
                FileOpenMode::Keep,
            )?;
        }
        publish_app_state(world);
        Ok(())
    })();
    if let Err(error) = result {
        world.resource_mut::<WorkspaceState>().error = Some(error.to_string());
    }
    world.resource_mut::<AppInitialized>().0 = true;
}

pub fn process_app_requests(world: &mut World) -> io::Result<()> {
    world.run_schedule(AppRequestsSchedule);
    Ok(())
}

pub fn tick_app_services(world: &mut World) -> io::Result<()> {
    world.run_schedule(AppServicesSchedule);
    process_app_requests(world)
}

pub fn dispatch_app_command(
    world: &mut World,
    command: WindowCommand,
    target: PanelTarget,
) -> io::Result<bool> {
    let request = capture_app_request(world, AppAction::Command(command), target);
    dispatch_request(world, request)
}

pub fn dispatch_app_plugin_command(
    world: &mut World,
    command: impl Into<String>,
    target: PanelTarget,
) -> io::Result<bool> {
    let request = capture_app_request(world, AppAction::PluginCommand(command.into()), target);
    dispatch_request(world, request)
}

fn dispatch_request(world: &mut World, request: AppRequest) -> io::Result<bool> {
    let id = submit_app_request(world, request);
    process_app_requests(world)?;
    match poll_app_request(world, id) {
        Some(Ok(AppRequestOutcome::Ignored)) => Ok(false),
        Some(Ok(_)) => Ok(true),
        Some(Err(error)) => Err(io::Error::other(error)),
        None => Ok(false),
    }
}

enum Continuation {
    Close(Vec<Entity>),
    Quit(Vec<Entity>),
    Promotion {
        root: PathBuf,
        caller: Entity,
        terminal: u64,
        panels: Vec<Entity>,
    },
    Workspace(WorkspaceSpec),
    CreateWorkspace(PathBuf),
    Directory(PathBuf),
    Connected(Box<ConnectedWorkspace>),
    Layout {
        tiling: Value,
        panels: Vec<Entity>,
    },
}

struct PendingSave {
    id: Option<AppRequestId>,
    documents: Vec<SavedDocument>,
    save: bed_core::DocumentSaveId,
    continuation: Option<Continuation>,
}

#[derive(Resource, Default)]
struct PendingOperations {
    saves: Vec<PendingSave>,
    batches: Vec<PendingBatch>,
    barrier: Option<bed_api::features::OperationBarrierToken>,
    admission: Option<PendingAdmission>,
}

enum PendingAdmission {
    Request {
        request: Box<AppRequest>,
        id: Option<AppRequestId>,
    },
    Close {
        panels: Vec<Entity>,
        continuation: Continuation,
        id: Option<AppRequestId>,
    },
}

struct PendingBatch {
    parent: Option<AppRequestId>,
    child: AppRequestId,
    remaining: VecDeque<AppRequest>,
}

fn internal_request_id(world: &mut World) -> AppRequestId {
    let mut requests = world.resource_mut::<AppRequests>();
    requests.next_id = requests
        .next_id
        .checked_add(1)
        .expect("app request ID overflow");
    AppRequestId(requests.next_id)
}

fn apply_batch(
    world: &mut World,
    mut requests: VecDeque<AppRequest>,
    parent: Option<AppRequestId>,
    mut outcome: AppRequestOutcome,
) -> io::Result<Option<AppRequestOutcome>> {
    while let Some(request) = requests.pop_front() {
        let child = internal_request_id(world);
        match apply_request(world, request, Some(child))? {
            Some(result) => outcome = result,
            None => {
                world
                    .resource_mut::<PendingOperations>()
                    .batches
                    .push(PendingBatch {
                        parent,
                        child,
                        remaining: requests,
                    });
                return Ok(None);
            }
        }
    }
    Ok(Some(outcome))
}

fn poll_batches(world: &mut World) {
    let batches = std::mem::take(&mut world.resource_mut::<PendingOperations>().batches);
    for batch in batches {
        match poll_app_request_result(world, batch.child) {
            None => world
                .resource_mut::<PendingOperations>()
                .batches
                .push(batch),
            Some(Err(error)) => complete(world, batch.parent, Err(error)),
            Some(Ok(outcome)) => match apply_batch(world, batch.remaining, batch.parent, outcome) {
                Ok(Some(outcome)) => complete(world, batch.parent, Ok(outcome)),
                Ok(None) => {}
                Err(error) => complete(world, batch.parent, Err(error.into())),
            },
        }
    }
}

fn captured_effect(
    world: &World,
    template: &AppRequest,
    action: AppAction,
    source: Option<Entity>,
    area: Option<u32>,
) -> AppRequest {
    let explicit_target = explicit_panel_target(&action).is_some();
    let mut request = capture_app_request_from(world, action, PanelTarget::LargestArea, source);
    request.session_id = template.session_id;
    request.layout_slot = template.layout_slot;
    request.context = template.context.clone();
    if let Some(area) = area {
        request.area = area;
    } else if !explicit_target {
        request.area = template.area;
    }
    if request.area == template.area {
        request.tab_index = template.tab_index;
    }
    request
}

fn complete(
    world: &mut World,
    id: Option<AppRequestId>,
    result: Result<AppRequestOutcome, AppRequestError>,
) {
    if let Err(error) = &result {
        world.resource_mut::<WorkspaceState>().error = Some(error.message.clone());
    }
    if let Some(id) = id {
        let generic = world.resource_mut::<AppRequests>().generic.remove(&id);
        if let Some(generic) = generic {
            let result =
                result
                    .clone()
                    .map_err(|error| error.message)
                    .map(|outcome| match outcome {
                        AppRequestOutcome::Ignored => WorkbenchOutcome::Ignored,
                        AppRequestOutcome::Opened { panel, .. } => WorkbenchOutcome::Opened(panel),
                        AppRequestOutcome::Closed { panels } if panels.len() == 1 => {
                            WorkbenchOutcome::Closed(panels[0])
                        }
                        _ => WorkbenchOutcome::Applied,
                    });
            world
                .resource_mut::<WorkbenchRequests>()
                .complete(generic, result);
        }
        if generic.is_none() {
            world
                .resource_mut::<AppRequests>()
                .results
                .insert(id, result);
        }
    }
}

fn apply_app_requests(world: &mut World) {
    assert!(
        !bed_workbench::frontend_active(world),
        "editor transitions run between frontend scopes"
    );
    // UI save intentions captured their ordering while drawing. Admit them
    // with application and plugin commands after every frontend scope has ended.
    crate::config::flush_settings_outbox(world);
    drain_feature_intents(world);
    loop {
        if layouts::waiting(world) || bed_core::operation_barrier_active(world) {
            break;
        }
        let generic_sequence = world.resource::<WorkbenchRequests>().next_sequence();
        let app_sequence = world
            .resource::<AppRequests>()
            .pending
            .front()
            .map(|request| request.sequence);
        if generic_sequence
            .is_some_and(|sequence| app_sequence.is_none_or(|editor| sequence < editor))
        {
            let queued = world
                .resource_mut::<WorkbenchRequests>()
                .take_next()
                .unwrap();
            if queued.layout_epoch.is_some_and(|epoch| {
                epoch != *world.resource::<bed_workbench::WorkbenchLayoutEpoch>()
            }) {
                if let Some(id) = queued.id {
                    world
                        .resource_mut::<WorkbenchRequests>()
                        .complete(id, Ok(bed_workbench::WorkbenchOutcome::Ignored));
                }
                continue;
            }
            let source = match &queued.action {
                WorkbenchAction::Invoke(invocation) => invocation.source,
                WorkbenchAction::ClosePanel(entity)
                | WorkbenchAction::KeepPanel(entity)
                | WorkbenchAction::FocusPanel(entity)
                | WorkbenchAction::ObserveFocus(entity)
                | WorkbenchAction::SplitPanel { panel: entity, .. } => Some(*entity),
                WorkbenchAction::OpenPanel { .. } | WorkbenchAction::ShowPanel { .. } => {
                    world.resource::<WorkbenchLayout>().focused
                }
                WorkbenchAction::FocusArea(_) | WorkbenchAction::SetLayout { .. } => None,
            };
            let request = capture_app_request_from(
                world,
                AppAction::WorkbenchAction(queued.action),
                PanelTarget::LargestArea,
                source,
            );
            let id = queued.id.map(|generic| {
                let mut requests = world.resource_mut::<AppRequests>();
                requests.next_id = requests
                    .next_id
                    .checked_add(1)
                    .expect("app request ID overflow");
                let id = AppRequestId(requests.next_id);
                requests.generic.insert(id, generic);
                id
            });
            let result = apply_request(world, request, id);
            match result {
                Ok(Some(outcome)) => complete(world, id, Ok(outcome)),
                Ok(None) => {}
                Err(error) => complete(world, id, Err(error.into())),
            }
        } else if let Some(queued) = world.resource_mut::<AppRequests>().pending.pop_front() {
            let result = if queued.layout_epoch.is_some_and(|epoch| {
                epoch != world.resource::<bed_workbench::WorkbenchLayoutEpoch>().0
            }) {
                Ok(Some(AppRequestOutcome::Ignored))
            } else {
                apply_request(world, queued.request, queued.id)
            };
            match result {
                Ok(Some(outcome)) => complete(world, queued.id, Ok(outcome)),
                Ok(None) => {}
                Err(error) => complete(world, queued.id, Err(error.into())),
            }
        } else {
            break;
        }
        drain_feature_intents(world);
    }
    publish_app_state(world);
}

fn drain_feature_intents(world: &mut World) {
    let documents = world.resource_scope(|world, mut queue: Mut<AppRequests>| {
        queue
            .documents
            .read(world.resource::<Messages<bed_api::features::HostRequest>>())
            .cloned()
            .collect::<Vec<_>>()
    });
    for envelope in documents {
        let mut request = capture_app_request_from(
            world,
            AppAction::HostAction(envelope.action().clone()),
            PanelTarget::LargestArea,
            envelope.source(),
        );
        request.session_id = envelope.session_id().0;
        request.source = envelope.source();
        request.panel = envelope
            .source()
            .and_then(|source| bed_api::workbench::panel_owner(world, source));
        request.area = envelope.area();
        world
            .resource_mut::<AppRequests>()
            .pending
            .push_back(QueuedRequest {
                sequence: envelope.sequence(),
                layout_epoch: envelope
                    .is_layout_scoped()
                    .then_some(envelope.layout_epoch()),
                id: None,
                request,
            });
    }
    let workspaces = world.resource_scope(|world, mut queue: Mut<AppRequests>| {
        queue
            .workspaces
            .read(world.resource::<Messages<WorkspaceRequest>>())
            .cloned()
            .collect::<Vec<_>>()
    });
    for envelope in workspaces {
        let mut request = capture_app_request_from(
            world,
            match envelope.barrier {
                Some(token) => AppAction::WorkspaceWithBarrier {
                    action: envelope.action,
                    token,
                },
                None => AppAction::Workspace(envelope.action),
            },
            PanelTarget::LargestArea,
            envelope.source,
        );
        request.session_id = envelope.session_id;
        request.area = envelope.area;
        world
            .resource_mut::<AppRequests>()
            .pending
            .push_back(QueuedRequest {
                sequence: envelope.sequence,
                layout_epoch: None,
                id: None,
                request,
            });
    }
    let settings = world.resource_scope(|world, mut queue: Mut<AppRequests>| {
        queue
            .settings
            .read(world.resource::<Messages<bed_api::settings::SettingsRequest>>())
            .cloned()
            .collect::<Vec<_>>()
    });
    for setting in settings {
        let sequence = setting.sequence;
        let request = capture_app_request_from(
            world,
            AppAction::Settings(setting),
            PanelTarget::LargestArea,
            None,
        );
        world
            .resource_mut::<AppRequests>()
            .pending
            .push_back(QueuedRequest {
                sequence,
                layout_epoch: None,
                id: None,
                request,
            });
    }
    let intents = world.resource_mut::<HostEffects>().take();
    for intent in intents {
        if intent
            .source
            .is_some_and(|source| bed_api::workbench::panel_owner(world, source).is_none())
        {
            continue;
        }
        let mut request = capture_app_request_from(
            world,
            AppAction::HostAction(intent.action),
            PanelTarget::LargestArea,
            intent.source,
        );
        if let Some(area) = intent.area {
            request.area = area;
        }
        request.tab_index = intent.tab_index;
        world.resource_mut::<AppRequests>().enqueue(request);
    }
    world
        .resource_mut::<AppRequests>()
        .pending
        .make_contiguous()
        .sort_by_key(|request| request.sequence);
}

fn apply_request(
    world: &mut World,
    mut request: AppRequest,
    id: Option<AppRequestId>,
) -> io::Result<Option<AppRequestOutcome>> {
    if !matches!(&request.action, AppAction::Settings(_))
        && (request.session_id != world.non_send::<DocumentSession>().session_id().0
            || request
                .source
                .is_some_and(|source| bed_api::workbench::panel_owner(world, source).is_none()))
    {
        if id.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "The request's originating workspace or panel is no longer available",
            ));
        }
        return Ok(Some(AppRequestOutcome::Ignored));
    }
    let layout_independent = matches!(
        &request.action,
        AppAction::Command(
            WindowCommand::UseLayout(_)
                | WindowCommand::SetLayoutDefault(_)
                | WindowCommand::RestoreBuiltinLayout(_)
                | WindowCommand::Quit
        ) | AppAction::ApplyEdits { .. }
            | AppAction::SaveDocuments(_)
            | AppAction::SaveAs { .. }
            | AppAction::Settings(_)
            | AppAction::WorkspaceWithBarrier { .. }
            | AppAction::Workspace(_)
            | AppAction::SetWorkspace(_)
            | AppAction::AttachWorkspace(_)
    ) || matches!(&request.action, AppAction::HostAction(host) if !host.is_layout_scoped());
    let switch_command = matches!(&request.action,
        AppAction::WorkbenchAction(WorkbenchAction::Invoke(invocation)) if invocation.command.starts_with("bed.app.layout_"));
    if !layout_independent
        && !switch_command
        && (request.layout_slot != world.resource::<LayoutSlots>().active()
            || request
                .source
                .is_some_and(|source| !bed_api::workbench::panel_active(world, source)))
    {
        return Ok(Some(AppRequestOutcome::Ignored));
    }
    let action = std::mem::replace(
        &mut request.action,
        AppAction::Command(WindowCommand::NewDocument),
    );
    let outcome = match action {
        AppAction::Settings(settings) => {
            bed_core::settings::apply_settings_request(world, &settings)
                .map_err(io::Error::other)?;
            crate::config::sync_view(world)?;
            bed_core::settings::configure_document_settings(world)?;
            AppRequestOutcome::Applied
        }
        AppAction::WorkspaceWithBarrier { action, token } => {
            if bed_core::active_operation_barrier(world) != Some(token) {
                return Err(io::Error::other(
                    "The workspace operation has not reached admission",
                ));
            }
            world.resource_mut::<PendingOperations>().barrier = Some(token);
            if bed_core::barrier_status(world, token)
                == Some(bed_core::OperationBarrierStatus::Waiting)
            {
                request.action = AppAction::WorkspaceWithBarrier { action, token };
                let mut pending = world.resource_mut::<PendingOperations>();
                assert!(pending.admission.is_none(), "one operation owns admission");
                pending.admission = Some(PendingAdmission::Request {
                    request: Box::new(request),
                    id,
                });
                return Ok(None);
            }
            let result = apply_workspace_action(world, action, &request, id);
            if !matches!(result, Ok(None)) {
                release_transition_barrier(
                    world,
                    result.as_ref().map(|_| ()).map_err(ToString::to_string),
                )?;
            }
            return result;
        }
        AppAction::Workspace(action) => {
            if matches!(action, WorkspaceAction::SaveCurrent { .. }) {
                let token = bed_core::create_operation_barrier(world);
                let barrier = bed_core::capture_host_request(
                    world,
                    None,
                    HostAction::OperationBarrier { token },
                );
                bed_core::apply_document_request(world, barrier).map_err(io::Error::other)?;
                request.action = AppAction::WorkspaceWithBarrier { action, token };
                return apply_request(world, request, id);
            }
            return apply_workspace_action(world, action, &request, id);
        }
        AppAction::PluginCommand(command) => {
            let invocation = CommandInvocation {
                command,
                source: request.source,
                area: request.area,
                arguments: if request.arguments.is_null() {
                    command_arguments(&request.context)
                } else {
                    request.arguments.clone()
                },
            };
            if world
                .get_resource::<bed_api::workbench::CommandPresentations>()
                .and_then(|facts| facts.0.get(&invocation.command))
                .is_some_and(|facts| facts.blocks_invocation(world, &invocation))
            {
                return Ok(Some(AppRequestOutcome::Ignored));
            }
            let handler = world
                .resource::<bed_api::workbench::WorkbenchRegistry>()
                .command(&invocation.command)
                .map(|definition| definition.handler)
                .ok_or_else(|| {
                    io::Error::other(format!("Command unavailable: {}", invocation.command))
                })?;
            let previous = world.resource_mut::<HostEffects>().take();
            let result = world
                .run_system_with(handler, invocation)
                .map_err(io::Error::other)
                .and_then(|result| result.map_err(io::Error::other));
            let created = world.resource_mut::<HostEffects>().take();
            for intent in previous {
                world.resource_mut::<HostEffects>().push_effect(intent);
            }
            let effects = result?;
            let mut requests = effects
                .into_iter()
                .map(|action| {
                    captured_effect(
                        world,
                        &request,
                        AppAction::WorkbenchAction(action),
                        request.source,
                        None,
                    )
                })
                .collect::<VecDeque<_>>();
            requests.extend(created.into_iter().map(|intent| {
                captured_effect(
                    world,
                    &request,
                    AppAction::HostAction(intent.action),
                    intent.source,
                    intent.area,
                )
            }));
            let workspace_requests = world.resource_scope(|world, mut queue: Mut<AppRequests>| {
                queue
                    .workspaces
                    .read(world.resource::<Messages<WorkspaceRequest>>())
                    .cloned()
                    .collect::<Vec<_>>()
            });
            for envelope in workspace_requests {
                let action = match envelope.barrier {
                    Some(token) => AppAction::WorkspaceWithBarrier {
                        action: envelope.action,
                        token,
                    },
                    None => AppAction::Workspace(envelope.action),
                };
                let mut captured = captured_effect(
                    world,
                    &request,
                    action,
                    envelope.source,
                    Some(envelope.area),
                );
                captured.session_id = envelope.session_id;
                requests.push_back(captured);
            }
            return apply_batch(world, requests, id, AppRequestOutcome::Applied);
        }
        AppAction::WorkbenchAction(action) => match action {
            WorkbenchAction::ShowPanel {
                panel_type,
                state,
                area,
            } => {
                request.action = AppAction::WorkbenchAction(
                    if let Some(panel) = bed_workbench::recent_panel(world, &panel_type) {
                        WorkbenchAction::FocusPanel(panel)
                    } else {
                        WorkbenchAction::OpenPanel {
                            panel_type,
                            state,
                            area,
                        }
                    },
                );
                return apply_request(world, request, id);
            }
            WorkbenchAction::Invoke(invocation) => {
                request.action = AppAction::PluginCommand(invocation.command);
                request.source = invocation.source;
                request.area = invocation.area;
                request.arguments = invocation.arguments;
                return apply_request(world, request, id);
            }
            WorkbenchAction::OpenPanel {
                panel_type,
                state,
                area,
            } => {
                let document = state
                    .get("document")
                    .and_then(Value::as_u64)
                    .map(DocumentId);
                let panel = open_panel(
                    world,
                    &panel_type,
                    document.map(PanelInput::Document).unwrap_or_default(),
                    state,
                    None,
                    area.or(Some(request.area)),
                    false,
                )?;
                AppRequestOutcome::Opened {
                    panel,
                    document: world
                        .get::<PanelBinding>(panel)
                        .and_then(|binding| binding.document),
                }
            }
            WorkbenchAction::ClosePanel(panel) => {
                return begin_close(world, vec![panel], false, id);
            }
            WorkbenchAction::KeepPanel(panel) => {
                if bed_workbench::keep_panel(world, panel) {
                    AppRequestOutcome::Applied
                } else {
                    AppRequestOutcome::Ignored
                }
            }
            WorkbenchAction::ObserveFocus(panel) => {
                if bed_workbench::accepts_focus_observation(world, panel) {
                    focus_panel(world, panel, false)?;
                    AppRequestOutcome::Applied
                } else {
                    AppRequestOutcome::Ignored
                }
            }
            WorkbenchAction::FocusArea(area) => {
                bed_workbench::focus_area(world, area);
                AppRequestOutcome::Applied
            }
            WorkbenchAction::SetLayout { tiling, close } => {
                let closing = close.iter().copied().collect::<HashSet<_>>();
                let ids = active_panel_entities(world)
                    .into_iter()
                    .filter(|entity| !closing.contains(entity))
                    .map(|entity| world.get::<PanelPresentation>(entity).unwrap().id)
                    .collect::<Vec<_>>();
                bed_workbench::workspace::tiling_state::TilingState::from_value(&tiling, ids)
                    .map_err(io::Error::other)?;
                return begin_close_transition(
                    world,
                    close,
                    Continuation::Layout {
                        tiling,
                        panels: Vec::new(),
                    },
                    id,
                );
            }
            WorkbenchAction::FocusPanel(panel) => {
                focus_panel(world, panel, true)?;
                AppRequestOutcome::Applied
            }
            WorkbenchAction::SplitPanel { panel, axis, high } => {
                split_panel(world, panel, axis, high)?;
                AppRequestOutcome::Applied
            }
        },
        AppAction::PanelAction { panel, action } => {
            let entity = if world.get::<PanelOwner>(panel).is_some() {
                panel
            } else {
                panel_presentation(world, panel)
                    .ok_or_else(|| io::Error::other("Panel is unavailable"))?
            };
            if matches!(
                action,
                PanelAction::Undo | PanelAction::Redo | PanelAction::CommitEdit
            ) && let Some(document) = world
                .get::<PanelBinding>(entity)
                .and_then(|binding| binding.document)
            {
                commit_documents(world, &HashSet::from([document]))?;
            }
            let (handled, intents) =
                bed_core::dispatch_panel_action(world, entity, action).map_err(io::Error::other)?;
            let effects = intents
                .into_iter()
                .map(|intent| {
                    captured_effect(
                        world,
                        &request,
                        AppAction::HostAction(intent.action),
                        intent.source,
                        intent.area,
                    )
                })
                .collect();
            return apply_batch(
                world,
                effects,
                id,
                if handled {
                    AppRequestOutcome::Applied
                } else {
                    AppRequestOutcome::Ignored
                },
            );
        }
        AppAction::ExternalFiles { panel, event } => {
            let (accepted, intents) =
                bed_core::dispatch_external_files(world, panel, event.clone())
                    .map_err(io::Error::other)?;
            let mut effects = intents
                .into_iter()
                .map(|intent| {
                    captured_effect(
                        world,
                        &request,
                        AppAction::HostAction(intent.action),
                        intent.source,
                        intent.area,
                    )
                })
                .collect::<VecDeque<_>>();
            if accepted == bed_api::workbench::ExternalFileDropResponse::Accepted {
                return apply_batch(world, effects, id, AppRequestOutcome::Applied);
            }
            if event.phase != bed_api::workbench::ExternalFileDragPhase::Drop {
                return apply_batch(world, effects, id, AppRequestOutcome::Ignored);
            }
            let area = bed_workbench::area_at(world, event.position, event.viewport)
                .unwrap_or(request.area);
            effects.extend(event.paths.into_iter().map(|path| {
                let action = if path.is_dir() {
                    AppAction::HostAction(HostAction::OpenDirectory {
                        location: bed_files::FileLocation {
                            provider: bed_files::FileProviderId::Local,
                            path,
                        },
                    })
                } else {
                    AppAction::OpenFileWithViewer {
                        path,
                        viewer: None,
                        additional: false,
                        move_existing: true,
                    }
                };
                captured_effect(world, &request, action, request.source, Some(area))
            }));
            return apply_batch(world, effects, id, AppRequestOutcome::Applied);
        }
        AppAction::FocusPanel(panel) => {
            focus_panel(world, panel, true)?;
            AppRequestOutcome::Applied
        }
        AppAction::ObserveFocus(panel) => {
            focus_panel(world, panel, false)?;
            AppRequestOutcome::Applied
        }
        AppAction::FocusArea(area) => {
            bed_workbench::focus_area(world, area);
            AppRequestOutcome::Applied
        }
        AppAction::ClosePanel(panel) => return begin_close(world, vec![panel], false, id),
        AppAction::ClosePanels(panels) => return begin_close(world, panels, false, id),
        AppAction::OpenFile(path) => {
            return begin_open_file(
                world,
                &path,
                None,
                false,
                false,
                Some(request.area),
                request.tab_index,
                id,
                FileOpenMode::Keep,
            );
        }
        AppAction::OpenFileWithViewer {
            path,
            viewer,
            additional,
            move_existing,
        } => {
            return begin_open_file(
                world,
                &path,
                viewer.as_deref(),
                additional,
                move_existing,
                Some(request.area),
                request.tab_index,
                id,
                FileOpenMode::Keep,
            );
        }
        AppAction::OpenViewer { panel, viewer } => {
            let panel = open_viewer(world, panel, &viewer)?;
            AppRequestOutcome::Opened {
                panel,
                document: world
                    .get::<PanelBinding>(panel)
                    .and_then(|binding| binding.document),
            }
        }
        AppAction::AttachWorkspace(path) => {
            attach_workspace(world, &path)?;
            AppRequestOutcome::Applied
        }
        AppAction::SetWorkspace(spec) => return begin_workspace(world, spec, id),
        AppAction::ApplyEdits {
            document,
            revision,
            edits,
        } => {
            commit_documents(world, &HashSet::from([document]))?;
            bed_core::apply_edits(world, document, revision, &edits)?;
            AppRequestOutcome::Applied
        }
        AppAction::SaveDocuments(documents) => {
            return begin_save(world, documents, None, id);
        }
        AppAction::SaveAs { document, path } => {
            return begin_save_as(world, document, &path, id);
        }
        AppAction::Command(command) => return window_command(world, command, &request, id),
        AppAction::HostAction(host) => return apply_host_action(world, host, &request, id),
        AppAction::HostActions { actions } => {
            let requests = actions
                .into_iter()
                .map(|host| {
                    captured_effect(
                        world,
                        &request,
                        AppAction::HostAction(host),
                        request.source,
                        None,
                    )
                })
                .collect();
            return apply_batch(world, requests, id, AppRequestOutcome::Applied);
        }
    };
    if let AppRequestOutcome::Opened { panel, .. } = &outcome {
        place_opened_panel(world, *panel, Some(request.area), false, request.tab_index)?;
    }
    Ok(Some(outcome))
}

fn command_arguments(context: &CommandContext) -> Value {
    json!({"path":context.path,"document":context.document.map(|id|id.0),"revision":context.revision,
        "selection":context.selection.as_ref().map(|selection| json!({"document":selection.document.0,"revision":selection.revision,"ranges":selection.ranges.iter().map(|range|[range.start,range.end]).collect::<Vec<_>>(),"text":selection.text}))})
}

pub fn panel_entities(world: &mut World) -> Vec<Entity> {
    let mut panels = world
        .query::<(Entity, &PanelPresentation, &PanelRecord)>()
        .iter(world)
        .map(|(entity, panel, _)| (panel.id, entity))
        .collect::<Vec<_>>();
    panels.sort_by_key(|(id, _)| *id);
    panels.into_iter().map(|(_, entity)| entity).collect()
}

pub fn active_panel_entities(world: &mut World) -> Vec<Entity> {
    panel_entities(world)
        .into_iter()
        .filter(|panel| bed_api::workbench::panel_active(world, *panel))
        .collect()
}

pub fn open_panel(
    world: &mut World,
    kind: &str,
    input: PanelInput,
    state: Value,
    viewer: Option<String>,
    area: Option<u32>,
    show_existing: bool,
) -> io::Result<Entity> {
    if area.is_some_and(|area| {
        !world
            .resource::<WorkbenchLayout>()
            .tiling
            .areas
            .iter()
            .any(|group| group.area == area)
    }) {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "This area is no longer available",
        ));
    }
    if world
        .resource::<bed_api::workbench::WorkbenchRegistry>()
        .panel(kind)
        .is_none()
    {
        return Err(io::Error::other(format!(
            "Panel feature unavailable: {kind}"
        )));
    }
    if show_existing {
        let existing = bed_workbench::recent_panel(world, kind)
            .filter(|entity| {
                world
                    .get::<PanelBinding>(*entity)
                    .is_some_and(|binding| binding.input == input)
            })
            .or_else(|| {
                world
                    .query::<(Entity, &PanelRecord, &PanelBinding)>()
                    .iter(world)
                    .find_map(|(entity, record, binding)| {
                        (bed_api::workbench::panel_active(world, entity)
                            && record.panel_type == kind
                            && binding.input == input
                            && world.get::<PanelPresentation>(entity).is_some())
                        .then_some(entity)
                    })
            });
        if let Some(existing) = existing {
            focus_panel(world, existing, true)?;
            return Ok(existing);
        }
    }
    let record = json!({"panel_type":kind,"viewer":viewer,"state":state,"document":input.document().map(|id|id.0),"path":input.local_file().map(|path|path.to_string_lossy().into_owned())});
    let panel = bed_workbench::spawn_panel(world, record, area, None).map_err(io::Error::other)?;
    let binding = PanelBinding {
        input,
        document: None,
        viewer,
        session_id: world.non_send::<DocumentSession>().session_id().0,
        ..Default::default()
    };
    world.entity_mut(panel).insert(binding.clone());
    mount_presentation(world, panel, kind, binding, state)?;
    focus_panel(world, panel, true)?;
    Ok(panel)
}

fn mount_presentation(
    world: &mut World,
    panel: Entity,
    kind: &str,
    binding: PanelBinding,
    state: Value,
) -> io::Result<Entity> {
    let binding = PanelBinding {
        document: binding.input.document().or(binding.document),
        ..binding
    };
    let factory = world
        .resource::<bed_api::workbench::WorkbenchRegistry>()
        .panel(kind)
        .map(|definition| definition.factory);
    let entity = world
        .spawn((
            PanelOwner(panel),
            binding.clone(),
            PanelRecord {
                panel_type: kind.into(),
                serialized: json!({"panel_type":kind,"state":state}),
                unavailable: None,
            },
        ))
        .id();
    let unavailable = world
        .get::<PanelRecord>(panel)
        .and_then(|record| record.unavailable.clone())
        .or_else(|| {
            binding
                .viewer
                .as_ref()
                .filter(|viewer| {
                    world
                        .resource::<bed_api::features::FeatureRegistry>()
                        .viewer(viewer)
                        .is_none()
                })
                .map(|viewer| format!("Viewer feature unavailable: {viewer}"))
        });
    let result = match (unavailable, factory) {
        (Some(error), _) => Err(error),
        (None, Some(factory)) => world
            .run_system_with(factory, PanelCreation { entity, state })
            .map_err(|error| error.to_string())
            .and_then(|result| result),
        (None, None) => Err(format!("Panel feature unavailable: {kind}")),
    };
    match result {
        Ok(frontend) => {
            bed_workbench::install_panel_frontend(world, entity, frontend)
                .map_err(io::Error::other)?;
        }
        Err(error) => {
            world.get_mut::<PanelRecord>(entity).unwrap().unavailable = Some(error.clone());
            world.get_mut::<PanelRecord>(panel).unwrap().unavailable = Some(error);
        }
    }
    let binding = world.get::<PanelBinding>(entity).unwrap().clone();
    world.entity_mut(panel).insert(binding.clone());
    world
        .entity_mut(panel)
        .insert(bed_api::workbench::PanelFrontend(entity));
    Ok(entity)
}

pub fn focus_panel(world: &mut World, source: Entity, request_native: bool) -> io::Result<()> {
    let Some(panel) = bed_api::workbench::panel_owner(world, source) else {
        return Ok(());
    };
    if !bed_workbench::owns_panel(world, panel) || !bed_api::workbench::panel_active(world, panel) {
        return Ok(());
    }
    let changed = world.resource::<WorkbenchLayout>().focused != Some(panel);
    if request_native {
        bed_workbench::focus_panel(world, panel);
    } else {
        bed_workbench::observe_focus(world, panel);
    }
    let binding = world
        .get::<PanelBinding>(panel)
        .cloned()
        .unwrap_or_default();
    let model = panel_presentation(world, panel)
        .and_then(|entity| world.get::<bed_core::DocumentBinding>(entity))
        .or_else(|| world.get::<bed_core::DocumentBinding>(panel))
        .map(|binding| binding.0);
    let last_model = world
        .resource::<DocumentNavigation>()
        .last_document_panel
        .and_then(|panel| {
            panel_presentation(world, panel)
                .and_then(|entity| world.get::<bed_core::DocumentBinding>(entity))
                .or_else(|| world.get::<bed_core::DocumentBinding>(panel))
        })
        .map(|binding| binding.0);
    let last_binding = world
        .resource::<DocumentNavigation>()
        .last_document_panel
        .and_then(|panel| world.get::<PanelBinding>(panel))
        .cloned();
    {
        let mut navigation = world.resource_mut::<DocumentNavigation>();
        if let Some(document) = model {
            navigation.active_document = Some(document);
            navigation.active_view = None;
            navigation.last_document_panel = Some(panel);
        } else if binding.document.is_some() {
            navigation.active_view = binding.view;
            navigation.active_document = binding.document;
            navigation.last_document_panel = Some(panel);
        } else if binding.input.local_file().is_some() {
            navigation.active_view = None;
            navigation.active_document = None;
        } else if let Some(document) = last_model {
            navigation.active_document = Some(document);
            navigation.active_view = None;
        } else if let Some(last) = last_binding {
            navigation.active_view = last.view;
            navigation.active_document = last.document;
        }
    }
    if (changed || request_native)
        && let Some(entity) = panel_presentation(world, panel)
        && let Some(handler) = world.get::<PanelFocus>(entity).copied()
    {
        world
            .run_system_with(handler.0, entity)
            .map_err(io::Error::other)?
            .map_err(io::Error::other)?;
    }
    Ok(())
}

pub fn open_file(
    world: &mut World,
    path: &Path,
    explicit: Option<&str>,
    additional: bool,
    area: Option<u32>,
) -> io::Result<Entity> {
    open_file_with_origin(
        world,
        path,
        explicit,
        additional,
        area,
        None,
        FileOpenMode::Keep,
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FileOpenMode {
    Keep,
    Preview,
}

fn open_file_with_origin(
    world: &mut World,
    path: &Path,
    explicit: Option<&str>,
    additional: bool,
    area: Option<u32>,
    origin: Option<LspRequestOrigin>,
    mode: FileOpenMode,
) -> io::Result<Entity> {
    // A newer explorer selection supersedes a pending preview in this area.
    // Explicitly opening the same file also supersedes its pending preview.
    let session = world.non_send::<DocumentSession>().session_id().0;
    let slot = world.resource::<LayoutSlots>().active();
    let mut canceled = Vec::new();
    {
        let mut operations = world.non_send_mut::<RemoteOperations>();
        let opens = std::mem::take(&mut operations.opens);
        for pending in opens {
            if pending.session_id == session
                && pending.mode == FileOpenMode::Preview
                && ((mode == FileOpenMode::Preview
                    && pending.layout_slot == slot
                    && pending.area == area)
                    || (mode == FileOpenMode::Keep && pending.path == path))
            {
                operations.abandoned_paths.insert(pending.path.clone());
                canceled.push(pending.id);
            } else {
                operations.opens.push(pending);
            }
        }
    }
    for id in canceled {
        complete(world, id, Ok(AppRequestOutcome::Ignored));
    }
    if area.is_some_and(|area| {
        !world
            .resource::<WorkbenchLayout>()
            .tiling
            .areas
            .iter()
            .any(|group| group.area == area)
    }) {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "This area is no longer available",
        ));
    }
    let remote = world.non_send::<DocumentSession>().is_remote();
    let path = if remote {
        path.to_path_buf()
    } else {
        std::fs::canonicalize(path)?
    };
    let path_string = path
        .to_str()
        .ok_or_else(|| io::Error::other("Choose a UTF-8 file path"))?;
    let kind = if remote {
        if explicit.is_none() {
            if world
                .resource::<bed_api::features::FeatureRegistry>()
                .viewer_for_path(path_string)
                .is_some_and(|viewer| viewer.is_local_file())
            {
                return Err(io::Error::other("This viewer opens local files only"));
            }
            let document = world
                .non_send_mut::<DocumentSession>()
                .open_file_auto(&path)?;
            Some(
                world
                    .non_send::<DocumentSession>()
                    .document_kind(document)?,
            )
        } else {
            None
        }
    } else {
        Some(bed_files::files::classify_bytes(
            &bed_files::files::read_file_bytes(&path)?.raw,
        ))
    };
    let viewer = {
        let registry = world.resource::<bed_api::features::FeatureRegistry>();
        match explicit {
            Some(id) => registry.viewer(id),
            None => kind
                .and_then(|kind| {
                    registry.preferred_viewer_for_file(
                        path_string,
                        kind,
                        &world
                            .resource::<bed_api::settings::SettingsSnapshot>()
                            .default_viewers,
                    )
                })
                .or_else(|| registry.viewer_for_path(path_string)),
        }
        .cloned()
        .ok_or_else(|| io::Error::other("File viewer is unavailable"))?
    };
    if viewer.is_local_file() && remote {
        return Err(io::Error::other("This viewer opens local files only"));
    }
    let input = if viewer.is_local_file() {
        PanelInput::LocalFile(path.clone())
    } else if let Some(document) = world.non_send::<DocumentSession>().document_for_path(&path) {
        if !viewer.supports_document_kind(
            world
                .non_send::<DocumentSession>()
                .document_kind(document)?,
        ) {
            return Err(io::Error::other("Viewer cannot display this document kind"));
        }
        PanelInput::Document(document)
    } else {
        PanelInput::Document(match origin {
            Some(origin) => world
                .non_send_mut::<DocumentSession>()
                .open_file_from_lsp(&path, origin)?,
            None => world
                .non_send_mut::<DocumentSession>()
                .open_file_with_kind(
                    &path,
                    viewer
                        .document_kind()
                        .unwrap_or(bed_api::documents::DocumentKind::Text),
                )?,
        })
    };
    if !additional {
        let recent = if viewer.is_local_file() {
            bed_workbench::recent_panel(world, viewer.panel_type).filter(|panel| {
                world.get::<PanelBinding>(*panel).is_some_and(|binding| {
                    binding.input == input && binding.viewer.as_deref() == Some(viewer.id)
                })
            })
        } else {
            None
        };
        let existing = recent.or_else(|| {
            world
                .query::<(Entity, &PanelPresentation, &PanelBinding, &PanelRecord)>()
                .iter(world)
                .find_map(|(entity, _, binding, record)| {
                    (bed_api::workbench::panel_active(world, entity)
                        && binding.input == input
                        && record.panel_type == viewer.panel_type
                        && binding.viewer.as_deref() == Some(viewer.id))
                    .then_some(entity)
                })
        });
        if let Some(panel) = existing {
            if mode == FileOpenMode::Keep {
                bed_workbench::keep_panel(world, panel);
            }
            focus_panel(world, panel, true)?;
            return Ok(panel);
        }
    }
    let mut target = area.unwrap_or_else(|| {
        world
            .resource::<WorkbenchLayout>()
            .tiling
            .layout
            .largest_area()
    });
    let previous = if mode == FileOpenMode::Preview {
        let id = world
            .resource::<WorkbenchLayout>()
            .tiling
            .areas
            .iter()
            .find(|group| group.area == target)
            .and_then(|group| group.preview);
        id.map(|id| {
            let panel = panel_entities(world)
                .into_iter()
                .find(|panel| world.get::<PanelPresentation>(*panel).unwrap().id == id)
                .expect("preview belongs to a live panel");
            let index = world
                .resource::<WorkbenchLayout>()
                .tiling
                .areas
                .iter()
                .find(|group| group.area == target)
                .unwrap()
                .tabs
                .iter()
                .position(|tab| *tab == id)
                .unwrap();
            (panel, index)
        })
    } else {
        None
    };
    // Finish local drafts before deciding whether the previous preview is safe
    // to retire. A draft failure or unsaved work retains the old panel.
    let mut previous = previous;
    if let Some((panel, _)) = previous
        && let Some(document) = world
            .get::<PanelBinding>(panel)
            .and_then(|binding| binding.document)
    {
        if let Err(error) = commit_documents(world, &HashSet::from([document])) {
            bed_workbench::keep_panel(world, panel);
            return Err(error);
        }
        let snapshot = world.non_send::<DocumentSession>().snapshot(document)?;
        if snapshot.dirty || snapshot.disk_conflict.is_some() {
            bed_workbench::keep_panel(world, panel);
            previous = None;
        }
    }
    let reusable = if !additional && input.document().is_some() {
        reusable_panel(world, area)?
    } else {
        None
    };
    // Keep a sole terminal visible when its first file opens. Do the split in
    // the standard opening path so shell commands and links share this rule.
    // Commit the new area only after the file's panel has opened successfully.
    let active = active_panel_entities(world);
    let mut terminal_split = None;
    if active.len() == 1
        && world
            .get::<PanelBinding>(active[0])
            .is_some_and(|binding| binding.terminal.is_some())
        && world.resource::<WorkbenchLayout>().tiling.areas.len() == 1
    {
        let mut tiling = world.resource::<WorkbenchLayout>().tiling.clone();
        let rect = tiling.layout.area(target).rect;
        let destination = tiling
            .layout
            .split(target, 0, (rect.min[0] + rect.max[0]) / 2, 100)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "This area is too small to split",
                )
            })?;
        tiling.add_area(destination);
        terminal_split = Some((tiling, destination));
    }
    let panel = if let Some(panel) = reusable {
        replace_pristine_panel(
            world,
            panel,
            viewer.panel_type,
            input.document().unwrap(),
            viewer.id,
        )?
    } else {
        open_panel(
            world,
            viewer.panel_type,
            input,
            Value::Null,
            Some(viewer.id.into()),
            area,
            false,
        )?
    };
    if let Some((mut tiling, destination)) = terminal_split {
        tiling.assign(
            world.get::<PanelPresentation>(panel).unwrap().id,
            destination,
        );
        world.resource_mut::<WorkbenchLayout>().tiling = tiling;
        target = destination;
        focus_panel(world, panel, true)?;
    }
    if mode == FileOpenMode::Preview
        && world
            .get::<PanelRecord>(panel)
            .unwrap()
            .unavailable
            .is_none()
    {
        if let Some((old, index)) = previous {
            let id = world.get::<PanelPresentation>(panel).unwrap().id;
            world
                .resource_mut::<WorkbenchLayout>()
                .tiling
                .move_tab(id, target, index);
            retire_panels(world, &[old])?;
        }
        let id = world.get::<PanelPresentation>(panel).unwrap().id;
        world
            .resource_mut::<WorkbenchLayout>()
            .tiling
            .areas
            .iter_mut()
            .find(|group| group.area == target)
            .unwrap()
            .preview = Some(id);
    }
    Ok(panel)
}

/// Open another tab for a compatible viewer of the same canonical document.
pub fn open_viewer(world: &mut World, panel: Entity, viewer: &str) -> io::Result<Entity> {
    let panel = bed_api::workbench::panel_owner(world, panel)
        .ok_or_else(|| io::Error::other("Panel is unavailable"))?;
    let document = world
        .get::<PanelBinding>(panel)
        .and_then(|binding| binding.document)
        .ok_or_else(|| io::Error::other("Panel has no document"))?;
    let area = world
        .get::<PanelPresentation>(panel)
        .and_then(|presentation| {
            world
                .resource::<WorkbenchLayout>()
                .tiling
                .area_for(presentation.id)
        });
    // Opening a second viewer keeps the originating preview as a normal tab.
    bed_workbench::keep_panel(world, panel);
    let opened =
        bed_core::open_document_viewer(world, document, viewer, area).map_err(io::Error::other)?;
    focus_panel(world, opened, true)?;
    Ok(opened)
}

pub fn split_panel(
    world: &mut World,
    source: Entity,
    axis: usize,
    high: bool,
) -> io::Result<Entity> {
    if axis > 1 {
        return Err(io::Error::other(
            "Split axis must be horizontal or vertical",
        ));
    }
    let panel = bed_api::workbench::panel_owner(world, source)
        .ok_or_else(|| io::Error::other("Panel is unavailable"))?;
    let presentation = panel_presentation(world, panel)
        .ok_or_else(|| io::Error::other("Panel has no presentation"))?;
    let mut state = bed_workbench::panel_state(world, presentation).map_err(io::Error::other)?;
    let binding = world.get::<PanelBinding>(panel).cloned().unwrap();
    let kind = world.get::<PanelRecord>(panel).unwrap().panel_type.clone();
    let id = world.get::<PanelPresentation>(panel).unwrap().id;
    let area = world
        .resource::<WorkbenchLayout>()
        .tiling
        .area_for(id)
        .unwrap();
    let mut tiling = world.resource::<WorkbenchLayout>().tiling.clone();
    let rect = tiling.layout.area(area).rect;
    let new_area = tiling
        .layout
        .split(area, axis, (rect.min[axis] + rect.max[axis]) / 2, 100)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "This area is too small to split",
            )
        })?;
    if !high {
        let low = tiling.layout.area(area).rect;
        let upper = tiling.layout.area(new_area).rect;
        for region in &mut tiling.layout.areas {
            if region.id == area {
                region.rect = upper;
            } else if region.id == new_area {
                region.rect = low;
            }
        }
    }
    tiling.add_area(new_area);
    if binding.terminal.is_some()
        && let Some(state) = state.as_object_mut()
    {
        state.remove("session");
    }
    let copied = open_panel(
        world,
        &kind,
        binding.input,
        state,
        binding.viewer,
        Some(area),
        false,
    )?;
    let copied_id = world.get::<PanelPresentation>(copied).unwrap().id;
    tiling.assign(copied_id, new_area);
    world.resource_mut::<WorkbenchLayout>().tiling = tiling;
    focus_panel(world, copied, true)?;
    Ok(copied)
}

pub fn commit_documents(world: &mut World, documents: &HashSet<DocumentId>) -> io::Result<()> {
    bed_core::commit_document_drafts(world, &documents.iter().copied().collect::<Vec<_>>())
        .map_err(io::Error::other)
}

struct SaveTarget {
    document: DocumentId,
    destination: Option<PathBuf>,
}

fn begin_save(
    world: &mut World,
    documents: Vec<DocumentId>,
    continuation: Option<Continuation>,
    id: Option<AppRequestId>,
) -> io::Result<Option<AppRequestOutcome>> {
    begin_save_targets(
        world,
        documents
            .into_iter()
            .map(|document| SaveTarget {
                document,
                destination: None,
            })
            .collect(),
        continuation,
        id,
    )
}

fn begin_save_as(
    world: &mut World,
    document: DocumentId,
    path: &Path,
    id: Option<AppRequestId>,
) -> io::Result<Option<AppRequestOutcome>> {
    begin_save_targets(
        world,
        vec![SaveTarget {
            document,
            destination: Some(path.to_path_buf()),
        }],
        None,
        id,
    )
}

// Capture every write before starting any I/O. A draft or path failure keeps
// all affected panels live and acknowledges the exact failed operation.
fn begin_save_targets(
    world: &mut World,
    targets: Vec<SaveTarget>,
    continuation: Option<Continuation>,
    id: Option<AppRequestId>,
) -> io::Result<Option<AppRequestOutcome>> {
    let targets = targets
        .into_iter()
        .map(|target| bed_core::DocumentSaveTarget {
            document: target.document,
            destination: target.destination,
        })
        .collect::<Vec<_>>();
    let save = bed_core::begin_document_save(world, &targets).map_err(io::Error::other)?;
    track_document_save(world, save, continuation, id)
}

// Core owns the write or format operation. bEd tracks only its request and the
// application transition that must wait for the committed revision.
fn track_document_save(
    world: &mut World,
    save: bed_core::DocumentSaveId,
    continuation: Option<Continuation>,
    id: Option<AppRequestId>,
) -> io::Result<Option<AppRequestOutcome>> {
    let mut pending = PendingSave {
        id,
        documents: Vec::new(),
        save,
        continuation,
    };
    let result = bed_core::poll_document_save(world, save);
    if let Some(warning) = bed_core::take_document_save_warning(world, save) {
        world.resource_mut::<WorkspaceState>().error = Some(warning);
    }
    match result {
        None => {
            world
                .resource_mut::<PendingOperations>()
                .saves
                .push(pending);
            Ok(None)
        }
        Some(Ok(documents)) => {
            pending.documents = documents;
            finish_save(world, pending).map(Some)
        }
        Some(Err(error)) => Err(io::Error::other(error)),
    }
}

fn finish_save(world: &mut World, pending: PendingSave) -> io::Result<AppRequestOutcome> {
    let transition = pending.continuation.is_some();
    let result = (|| {
        if transition {
            for saved in &pending.documents {
                if bed_core::document_revision(world, saved.document).map_err(io::Error::other)?
                    != saved.revision
                {
                    return Err(io::Error::other(
                        "Document changed while saving; prepare the transition again",
                    ));
                }
            }
            if let Some(token) = world.resource::<PendingOperations>().barrier {
                bed_core::prepare_document_transition(world, token, ClosePolicy::Discard)
                    .map_err(io::Error::other)?;
            }
        }
        finish_save_inner(world, pending)
    })();
    if transition {
        release_transition_barrier(
            world,
            result.as_ref().map(|_| ()).map_err(ToString::to_string),
        )?;
    }
    result
}

fn finish_save_inner(world: &mut World, pending: PendingSave) -> io::Result<AppRequestOutcome> {
    match pending.continuation {
        None => Ok(AppRequestOutcome::Saved {
            documents: pending.documents,
        }),
        Some(Continuation::Close(panels)) => {
            retire_panels(world, &panels)?;
            Ok(AppRequestOutcome::Closed { panels })
        }
        Some(Continuation::Quit(panels)) => {
            persist_workspace(world)?;
            retire_panels(world, &panels)?;
            world
                .non_send_mut::<DocumentSession>()
                .shutdown(ClosePolicy::Discard)?;
            world.resource_mut::<WorkspaceState>().close_requested = true;
            Ok(AppRequestOutcome::Closed { panels })
        }
        Some(Continuation::Directory(root)) => {
            replace_directory(world, root)?;
            Ok(AppRequestOutcome::Applied)
        }
        Some(Continuation::Workspace(spec)) => {
            replace_workspace(world, spec)?;
            Ok(AppRequestOutcome::Applied)
        }
        Some(Continuation::CreateWorkspace(root)) => {
            let spec = WorkspaceSpec::local(root.to_string_lossy());
            if world
                .resource::<WorkspaceRepositoryResource>()
                .0
                .find(&spec)?
                .is_none()
            {
                world
                    .resource_mut::<WorkspaceRepositoryResource>()
                    .0
                    .commit(
                        spec.clone(),
                        bed_plugin_workspaces::WorkspaceSnapshot {
                        layouts: json!({"active_layout_slot":LayoutSlot::Layout1.to_value(),"layouts":{}}),
                            plugins: json!({}),
                        },
                    )?;
            }
            replace_workspace(world, spec)?;
            persist_workspace(world)?;
            Ok(AppRequestOutcome::Applied)
        }
        Some(Continuation::Promotion {
            root,
            caller,
            terminal,
            panels,
        }) => {
            validate_terminal_promotion(world, caller, terminal)?;
            persist_workspace(world)?;
            retire_panels(world, &panels)?;
            attach_workspace_with_terminal(world, &root, Some(caller))?;
            focus_panel(world, caller, true)?;
            persist_workspace(world)?;
            Ok(AppRequestOutcome::Applied)
        }
        Some(Continuation::Connected(connected)) => {
            replace_connected_workspace(world, *connected)?;
            Ok(AppRequestOutcome::Applied)
        }
        Some(Continuation::Layout { tiling, panels }) => {
            retire_panels(world, &panels)?;
            bed_workbench::apply_layout(world, &tiling).map_err(io::Error::other)?;
            Ok(AppRequestOutcome::Applied)
        }
    }
}

fn begin_close(
    world: &mut World,
    panels: Vec<Entity>,
    quit: bool,
    id: Option<AppRequestId>,
) -> io::Result<Option<AppRequestOutcome>> {
    begin_close_transition(
        world,
        panels,
        if quit {
            Continuation::Quit(Vec::new())
        } else {
            Continuation::Close(Vec::new())
        },
        id,
    )
}

fn begin_close_transition(
    world: &mut World,
    panels: Vec<Entity>,
    continuation: Continuation,
    id: Option<AppRequestId>,
) -> io::Result<Option<AppRequestOutcome>> {
    if world.resource::<PendingOperations>().barrier.is_none() {
        let token = bed_core::create_operation_barrier(world);
        let request =
            bed_core::capture_host_request(world, None, HostAction::OperationBarrier { token });
        bed_core::apply_document_request(world, request).map_err(io::Error::other)?;
        world.resource_mut::<PendingOperations>().barrier = Some(token);
    }
    let token = world.resource::<PendingOperations>().barrier.unwrap();
    if bed_core::barrier_status(world, token) == Some(bed_core::OperationBarrierStatus::Waiting) {
        let mut pending = world.resource_mut::<PendingOperations>();
        assert!(pending.admission.is_none(), "one operation owns admission");
        pending.admission = Some(PendingAdmission::Close {
            panels,
            continuation,
            id,
        });
        return Ok(None);
    }
    let result = begin_close_transition_inner(world, panels, continuation, id);
    if !matches!(result, Ok(None)) {
        let outcome = result.as_ref().map(|_| ()).map_err(ToString::to_string);
        release_transition_barrier(world, outcome)?;
    }
    result
}

fn release_transition_barrier(world: &mut World, result: Result<(), String>) -> io::Result<()> {
    if let Some(token) = world.resource_mut::<PendingOperations>().barrier.take() {
        bed_core::finish_operation_barrier(world, token, result).map_err(io::Error::other)?;
    }
    Ok(())
}

fn begin_close_transition_inner(
    world: &mut World,
    panels: Vec<Entity>,
    continuation: Continuation,
    id: Option<AppRequestId>,
) -> io::Result<Option<AppRequestOutcome>> {
    let replacing = matches!(
        continuation,
        Continuation::Directory(_)
            | Continuation::Workspace(_)
            | Continuation::CreateWorkspace(_)
            | Continuation::Connected(_)
            | Continuation::Quit(_)
    );
    let quit = matches!(continuation, Continuation::Quit(_));
    let panels = panels
        .into_iter()
        .filter_map(|source| bed_api::workbench::panel_owner(world, source))
        .filter(|panel| bed_workbench::owns_panel(world, *panel))
        .collect::<HashSet<_>>();
    if panels.is_empty() && !replacing {
        if quit {
            persist_workspace(world)?;
            world
                .non_send_mut::<DocumentSession>()
                .shutdown(ClosePolicy::Discard)?;
            world.resource_mut::<WorkspaceState>().close_requested = true;
            return Ok(Some(AppRequestOutcome::Applied));
        }
        if let Continuation::Layout { tiling, .. } = continuation {
            bed_workbench::apply_layout(world, &tiling).map_err(io::Error::other)?;
            return Ok(Some(AppRequestOutcome::Applied));
        }
        if matches!(
            continuation,
            Continuation::Promotion { .. }
                | Continuation::Directory(_)
                | Continuation::Workspace(_)
                | Continuation::CreateWorkspace(_)
                | Continuation::Connected(_)
        ) {
            return begin_save_targets(world, Vec::new(), Some(continuation), id);
        }
        return Ok(Some(AppRequestOutcome::Ignored));
    }
    let mut documents = world
        .query::<(
            Entity,
            Option<&PanelBinding>,
            Option<&bed_core::DocumentBinding>,
        )>()
        .iter(world)
        .filter_map(|(entity, binding, model)| {
            let owner = bed_api::workbench::panel_owner(world, entity).unwrap_or(entity);
            panels
                .contains(&owner)
                .then(|| {
                    binding
                        .and_then(|binding| binding.document)
                        .or_else(|| model.map(|binding| binding.0))
                })
                .flatten()
        })
        .collect::<HashSet<_>>();
    if replacing {
        documents.extend(world.non_send::<DocumentSession>().document_ids());
        let session = world.non_send::<DocumentSession>().session_id();
        documents.extend(
            world
                .query::<&bed_core::ModelDocument>()
                .iter(world)
                .filter(|model| model.session_id() == session)
                .map(bed_core::ModelDocument::id),
        );
    }
    commit_documents(world, &documents)?;
    let remaining = if replacing {
        HashSet::new()
    } else {
        world
            .query::<(
                Entity,
                Option<&PanelBinding>,
                Option<&bed_core::DocumentBinding>,
            )>()
            .iter(world)
            .filter_map(|(entity, binding, model)| {
                let owner = bed_api::workbench::panel_owner(world, entity).unwrap_or(entity);
                (!panels.contains(&owner))
                    .then(|| {
                        binding
                            .and_then(|binding| binding.document)
                            .or_else(|| model.map(|binding| binding.0))
                    })
                    .flatten()
            })
            .collect::<HashSet<_>>()
    };
    let mut targets = Vec::new();
    for document in documents.difference(&remaining).copied() {
        let model = world
            .query::<&bed_core::ModelDocument>()
            .iter(world)
            .find(|model| model.id() == document)
            .cloned();
        let (path, dirty) = if let Some(model) = model {
            (
                model
                    .path
                    .as_ref()
                    .map(|path| path.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                model.dirty(),
            )
        } else {
            let session = world.non_send::<DocumentSession>();
            if session.is_snapshot_document(document) {
                continue;
            }
            let snapshot = session.snapshot(document)?;
            (snapshot.path, snapshot.dirty)
        };
        let destination = if dirty && path.is_empty() {
            let Some(path) = rfd::FileDialog::new()
                .set_directory(&world.resource::<WorkspaceState>().working_directory)
                .save_file()
            else {
                return Ok(Some(AppRequestOutcome::Ignored));
            };
            Some(path)
        } else {
            None
        };
        if destination.is_some() || !path.is_empty() {
            targets.push(SaveTarget {
                document,
                destination,
            });
        }
    }
    let panels = panels.into_iter().collect::<Vec<_>>();
    let continuation = match continuation {
        Continuation::Close(_) => Continuation::Close(panels),
        Continuation::Quit(_) => Continuation::Quit(panels),
        Continuation::Layout { tiling, .. } => Continuation::Layout { tiling, panels },
        Continuation::Promotion {
            root,
            caller,
            terminal,
            ..
        } => Continuation::Promotion {
            root,
            caller,
            terminal,
            panels,
        },
        other @ (Continuation::Directory(_)
        | Continuation::Workspace(_)
        | Continuation::CreateWorkspace(_)
        | Continuation::Connected(_)) => other,
    };
    begin_save_targets(world, targets, Some(continuation), id)
}

fn retire_presentations(world: &mut World, panel: Entity) -> io::Result<()> {
    let presentations = world
        .query::<(Entity, &PanelOwner)>()
        .iter(world)
        .filter_map(|(entity, owner)| (owner.0 == panel).then_some(entity))
        .collect::<Vec<_>>();
    for entity in presentations {
        if let Some(close) = world.get::<PanelClose>(entity).copied() {
            world
                .run_system_with(close.0, entity)
                .map_err(io::Error::other)?
                .map_err(io::Error::other)?;
        }
        drop(bed_workbench::take_panel_frontend(world, entity));
        world.despawn(entity);
    }
    world
        .entity_mut(panel)
        .remove::<bed_api::workbench::PanelFrontend>();
    Ok(())
}

fn pristine_panel(world: &World, panel: Entity) -> bool {
    let Some(surface) = panel_presentation(world, panel) else {
        return false;
    };
    // Reusing the shell requires a child frontend that can be fully retired.
    // A standalone core tab owns its feature components on the shell itself.
    if surface == panel {
        return false;
    }
    let Some(stamp) = world.get::<bed_api::documents::PanelDefaultInput>(surface) else {
        return false;
    };
    let session = world.non_send::<DocumentSession>();
    session.document_revision(stamp.document).ok() == Some(stamp.revision)
        && session.snapshot(stamp.document).is_ok_and(|snapshot| {
            snapshot.path.is_empty() && snapshot.bytes.is_empty() && !snapshot.dirty
        })
}

fn reusable_panel(world: &mut World, area: Option<u32>) -> io::Result<Option<Entity>> {
    let layout = world.resource::<WorkbenchLayout>();
    let focused = layout.focused.filter(|panel| {
        area.is_none_or(|area| {
            world
                .get::<PanelPresentation>(*panel)
                .is_some_and(|panel| layout.tiling.area_for(panel.id) == Some(area))
        })
    });
    let area = area.unwrap_or_else(|| {
        layout
            .tiling
            .layout
            .areas
            .iter()
            .max_by_key(|area| {
                (area.rect.max[0] - area.rect.min[0]) * (area.rect.max[1] - area.rect.min[1])
            })
            .unwrap()
            .id
    });
    let Some(ids) = layout
        .tiling
        .areas
        .iter()
        .find(|group| group.area == area)
        .map(|group| group.tabs.clone())
    else {
        return Ok(None);
    };
    let panels = panel_entities(world);
    let candidates = focused
        .into_iter()
        .chain(ids.into_iter().filter_map(|id| {
            panels
                .iter()
                .copied()
                .find(|panel| world.get::<PanelPresentation>(*panel).unwrap().id == id)
        }))
        .collect::<Vec<_>>();
    for panel in candidates {
        if !pristine_panel(world, panel) {
            continue;
        }
        let document = world
            .get::<PanelBinding>(panel)
            .and_then(|binding| binding.document)
            .expect("pristine input has a document");
        commit_documents(world, &HashSet::from([document]))?;
        if pristine_panel(world, panel) {
            return Ok(Some(panel));
        }
    }
    Ok(None)
}

fn replace_pristine_panel(
    world: &mut World,
    panel: Entity,
    kind: &str,
    document: DocumentId,
    viewer: &str,
) -> io::Result<Entity> {
    let old = world
        .get::<PanelBinding>(panel)
        .and_then(|binding| binding.document);
    retire_presentations(world, panel)?;
    let binding = PanelBinding {
        input: PanelInput::Document(document),
        document: Some(document),
        viewer: Some(viewer.into()),
        session_id: world.non_send::<DocumentSession>().session_id().0,
        ..Default::default()
    };
    let id = world.get::<PanelPresentation>(panel).unwrap().id;
    world.entity_mut(panel).insert(PanelRecord {
        panel_type: kind.into(),
        serialized: json!({"id":id,"panel_type":kind,"viewer":viewer,"state":null}),
        unavailable: None,
    });
    mount_presentation(world, panel, kind, binding, Value::Null)?;
    if let Some(old) = old.filter(|old| *old != document) {
        let retained = world
            .query::<(&PanelPresentation, &PanelBinding)>()
            .iter(world)
            .any(|(_, binding)| binding.document == Some(old));
        if !retained {
            bed_core::retire_document(world, old).map_err(io::Error::other)?;
        }
    }
    focus_panel(world, panel, true)?;
    Ok(panel)
}

pub fn retire_panels(world: &mut World, panels: &[Entity]) -> io::Result<()> {
    let documents = world
        .query::<(
            Entity,
            Option<&PanelBinding>,
            Option<&bed_core::DocumentBinding>,
        )>()
        .iter(world)
        .filter_map(|(entity, binding, model)| {
            let owner = bed_api::workbench::panel_owner(world, entity).unwrap_or(entity);
            panels
                .contains(&owner)
                .then(|| {
                    binding
                        .and_then(|binding| binding.document)
                        .or_else(|| model.map(|binding| binding.0))
                })
                .flatten()
        })
        .collect::<HashSet<_>>();
    for &panel in panels {
        retire_presentations(world, panel)?;
        bed_workbench::commit_close(world, panel);
    }
    for document in documents {
        bed_core::retire_document(world, document).map_err(io::Error::other)?;
    }
    if world
        .resource::<DocumentNavigation>()
        .last_document_panel
        .is_some_and(|panel| !world.entities().contains(panel))
    {
        let last = active_panel_entities(world).into_iter().find(|panel| {
            world
                .get::<PanelBinding>(*panel)
                .is_some_and(|binding| binding.document.is_some())
        });
        world
            .resource_mut::<DocumentNavigation>()
            .last_document_panel = last;
        if last.is_none() {
            let mut navigation = world.resource_mut::<DocumentNavigation>();
            navigation.active_document = None;
            navigation.active_view = None;
        }
    }
    if let Some(panel) = world.resource::<WorkbenchLayout>().focused {
        focus_panel(world, panel, true)?;
    } else {
        let mut navigation = world.resource_mut::<DocumentNavigation>();
        navigation.active_view = None;
        navigation.active_document = None;
        navigation.last_document_panel = None;
    }
    Ok(())
}

fn resume_admitted_operation(world: &mut World) {
    let Some(operation) = world.resource_mut::<PendingOperations>().admission.take() else {
        return;
    };
    let token = world
        .resource::<PendingOperations>()
        .barrier
        .expect("admission belongs to its operation barrier");
    let status = bed_core::barrier_status(world, token);
    if status == Some(bed_core::OperationBarrierStatus::Waiting) {
        world.resource_mut::<PendingOperations>().admission = Some(operation);
        return;
    }
    let id = match &operation {
        PendingAdmission::Request { id, .. } | PendingAdmission::Close { id, .. } => *id,
    };
    let result = match status {
        Some(bed_core::OperationBarrierStatus::Ready) => match operation {
            PendingAdmission::Request { request, id } => apply_request(world, *request, id),
            PendingAdmission::Close {
                panels,
                continuation,
                id,
            } => begin_close_transition(world, panels, continuation, id),
        },
        Some(bed_core::OperationBarrierStatus::Failed(message)) => {
            world.resource_mut::<PendingOperations>().barrier = None;
            Err(io::Error::other(message))
        }
        _ => Err(io::Error::other(
            "The admitted operation is no longer available",
        )),
    };
    if !matches!(result, Ok(None)) && world.resource::<PendingOperations>().barrier.is_some() {
        let outcome = result.as_ref().map(|_| ()).map_err(ToString::to_string);
        if let Err(error) = release_transition_barrier(world, outcome) {
            complete(world, id, Err(error.into()));
            return;
        }
    }
    match result {
        Ok(Some(outcome)) => complete(world, id, Ok(outcome)),
        Ok(None) => {}
        Err(error) => complete(world, id, Err(error.into())),
    }
}

fn update_app_services(world: &mut World) {
    bed_core::settings::refresh_settings(world);
    if let Err(error) = crate::config::sync_view(world)
        .and_then(|()| bed_core::settings::configure_document_settings(world))
    {
        world.resource_mut::<WorkspaceState>().error = Some(error.to_string());
    }
    let report = bed_core::take_document_service_report(world);
    let pending = std::mem::take(&mut world.resource_mut::<PendingOperations>().saves);
    for save in pending {
        let result = bed_core::poll_document_save(world, save.save);
        if let Some(warning) = bed_core::take_document_save_warning(world, save.save) {
            world.resource_mut::<WorkspaceState>().error = Some(warning);
        }
        match result {
            None => world.resource_mut::<PendingOperations>().saves.push(save),
            Some(Ok(documents)) => {
                let id = save.id;
                let mut save = save;
                save.documents = documents;
                let result = finish_save(world, save).map_err(AppRequestError::from);
                complete(world, id, result);
            }
            Some(Err(error)) => {
                let message = error.to_string();
                if save.continuation.is_some() {
                    let _ = release_transition_barrier(world, Err(message.clone()));
                }
                complete(world, save.id, Err(io::Error::other(message).into()));
            }
        }
    }
    if let Err(error) = poll_remote_operations(world, &report.errors) {
        world.resource_mut::<WorkspaceState>().error = Some(error.to_string());
    }
    resume_admitted_operation(world);
    poll_batches(world);
    layouts::poll(world);
    for event in &report.events {
        if let bed_documents::session::SessionEvent::Removed { document, .. } = event {
            let panels = panel_entities(world)
                .into_iter()
                .filter(|panel| {
                    world
                        .get::<PanelBinding>(*panel)
                        .is_some_and(|binding| binding.document == Some(*document))
                })
                .collect::<Vec<_>>();
            let result = commit_documents(world, &HashSet::from([*document]));
            let dirty = world
                .non_send::<DocumentSession>()
                .snapshot(*document)
                .is_ok_and(|snapshot| snapshot.dirty);
            if result.is_err() || dirty {
                if let Err(error) = world
                    .non_send_mut::<DocumentSession>()
                    .detach_removed_document(*document)
                {
                    world.resource_mut::<WorkspaceState>().error = Some(error.to_string());
                }
                if let Err(error) = result {
                    world.resource_mut::<WorkspaceState>().error =
                        Some(format!("Deleted file buffer preserved: {error}"));
                }
            } else if let Err(error) = retire_panels(world, &panels) {
                world.resource_mut::<WorkspaceState>().error = Some(error.to_string());
                let _ = world
                    .non_send_mut::<DocumentSession>()
                    .detach_removed_document(*document);
            }
        }
    }
    for error in report.errors {
        world.resource_mut::<WorkspaceState>().error = Some(error.message);
    }
    let state_changed = world.resource_scope(|world, mut cursor: Mut<FeatureStateCursor>| {
        cursor
            .0
            .read(world.resource::<Messages<bed_api::storage::FeatureStateChanged>>())
            .count()
            > 0
    });
    if world.resource::<AppInitialized>().0
        && !world.resource::<WorkspaceState>().close_requested
        && (state_changed
            || world
                .non_send::<WorkspacePersistence>()
                .last_saved
                .elapsed()
                >= std::time::Duration::from_secs(2))
        && let Err(error) = persist_workspace(world)
    {
        world.resource_mut::<WorkspaceState>().error = Some(error.to_string());
    }
    configure_feature_targets(world);
}

pub fn publish_app_state(world: &mut World) {
    prepare_app_state(world);
    bed_core::publish_documents(world);
}

fn prepare_app_state(world: &mut World) {
    // Document changes are already owned by the editor host. Retain edited
    // previews once, even if autosave subsequently makes them clean again.
    let preview_ids = world
        .resource::<WorkbenchLayout>()
        .tiling
        .areas
        .iter()
        .filter_map(|group| group.preview)
        .collect::<HashSet<_>>();
    let edited = world
        .query::<(&PanelPresentation, &PanelBinding)>()
        .iter(world)
        .filter_map(|(panel, binding)| {
            (preview_ids.contains(&panel.id)
                && binding.document.is_some_and(|document| {
                    world
                        .non_send::<DocumentSession>()
                        .with_document(document, |state| state.dirty)
                        .unwrap_or(false)
                }))
            .then_some(panel.id)
        })
        .collect::<Vec<_>>();
    for id in edited {
        world.resource_mut::<WorkbenchLayout>().tiling.keep_tab(id);
    }
    publish_file_context(world);
    let navigation = world.resource::<DocumentNavigation>().clone();
    world.insert_resource(bed_core::DocumentFrameSelection {
        view: navigation.active_view,
        document: navigation.active_document,
    });
    if let Err(error) = crate::config::sync_view(world) {
        world.resource_mut::<WorkspaceState>().error = Some(error.to_string());
    }
}

fn window_command(
    world: &mut World,
    command: WindowCommand,
    request: &AppRequest,
    id: Option<AppRequestId>,
) -> io::Result<Option<AppRequestOutcome>> {
    let plugin = match command {
        WindowCommand::Debug => Some("bed.debug.show"),
        WindowCommand::FindFile => Some("bed.explorer.find_file"),
        WindowCommand::FindWorkspace => Some("bed.search.show"),
        _ => None,
    };
    if let Some(command) = plugin {
        if world
            .resource::<bed_api::workbench::WorkbenchRegistry>()
            .command(command)
            .is_none()
        {
            return Ok(Some(AppRequestOutcome::Ignored));
        }
        let mut captured = capture_app_request_from(
            world,
            AppAction::PluginCommand(command.into()),
            PanelTarget::LargestArea,
            request.source,
        );
        captured.context = request.context.clone();
        captured.area = request.area;
        captured.tab_index = request.tab_index;
        return apply_request(world, captured, id);
    }
    let panel_kind = match command {
        WindowCommand::NewTerminal | WindowCommand::Terminal => {
            Some(("bed.terminal.panel", command == WindowCommand::Terminal))
        }
        WindowCommand::NewExplorer | WindowCommand::Explorer => {
            Some(("bed.explorer.panel", command == WindowCommand::Explorer))
        }
        WindowCommand::NewSettings | WindowCommand::Settings => {
            Some(("bed.settings.panel", command == WindowCommand::Settings))
        }
        WindowCommand::NewWorkspaces | WindowCommand::Workspaces => {
            Some(("bed.workspaces.panel", command == WindowCommand::Workspaces))
        }
        WindowCommand::NewDiagnostics | WindowCommand::Diagnostics => Some((
            "bed.editor.diagnostics-panel",
            command == WindowCommand::Diagnostics,
        )),
        WindowCommand::NewStructure | WindowCommand::Structure => {
            Some(("bed.structure.panel", command == WindowCommand::Structure))
        }
        WindowCommand::NewLspDashboard | WindowCommand::LspDashboard => Some((
            "bed.editor.language-servers-panel",
            command == WindowCommand::LspDashboard,
        )),
        WindowCommand::NewContentSearch => Some(("bed.search.panel", false)),
        WindowCommand::NewReferences => Some(("bed.editor.references-panel", false)),
        _ => None,
    };
    if let Some((kind, existing)) = panel_kind {
        if world
            .resource::<bed_api::workbench::WorkbenchRegistry>()
            .panel(kind)
            .is_none()
        {
            return Ok(Some(AppRequestOutcome::Ignored));
        }
        let panel = open_panel(
            world,
            kind,
            PanelInput::None,
            Value::Null,
            None,
            Some(request.area),
            existing,
        )?;
        return Ok(Some(AppRequestOutcome::Opened {
            panel,
            document: world
                .get::<PanelBinding>(panel)
                .and_then(|binding| binding.document),
        }));
    }
    let outcome = match command {
        WindowCommand::NewDocument => {
            let panel = open_panel(
                world,
                "bed.editor.text-panel",
                PanelInput::None,
                Value::Null,
                Some("bed.editor.text".into()),
                Some(request.area),
                false,
            )?;
            AppRequestOutcome::Opened {
                panel,
                document: world
                    .get::<PanelBinding>(panel)
                    .and_then(|binding| binding.document),
            }
        }
        WindowCommand::Save => {
            let Some(document) = request.context.document else {
                return Ok(Some(AppRequestOutcome::Ignored));
            };
            if bed_core::document_path(world, document)
                .map_err(io::Error::other)?
                .is_none_or(|path| path.as_os_str().is_empty())
            {
                return window_command(world, WindowCommand::SaveAs, request, id);
            }
            let save = bed_core::begin_explicit_document_save(world, document)
                .map_err(io::Error::other)?;
            return track_document_save(world, save, None, id);
        }
        WindowCommand::SaveAs => {
            let Some(document) = request.context.document else {
                return Ok(Some(AppRequestOutcome::Ignored));
            };
            let path = bed_core::document_path(world, document)
                .map_err(io::Error::other)?
                .unwrap_or_default();
            let dialog = rfd::FileDialog::new()
                .set_directory(&world.resource::<WorkspaceState>().working_directory);
            let dialog = if path.as_os_str().is_empty() {
                dialog
            } else {
                dialog.set_file_name(path.file_name().unwrap_or_default().to_string_lossy())
            };
            let Some(path) = dialog.save_file() else {
                return Ok(Some(AppRequestOutcome::Ignored));
            };
            return begin_save_as(world, document, &path, id);
        }
        WindowCommand::Close => {
            return begin_close(world, request.panel.into_iter().collect(), false, id);
        }
        WindowCommand::Quit => {
            let panels = panel_entities(world);
            return begin_close(world, panels, true, id);
        }
        WindowCommand::Find | WindowCommand::GoToLine => {
            let Some(source) = request
                .panel
                .and_then(|panel| panel_presentation(world, panel))
            else {
                return Ok(Some(AppRequestOutcome::Ignored));
            };
            let Some(handler) = world.get::<PanelActions>(source).copied() else {
                return Ok(Some(AppRequestOutcome::Ignored));
            };
            let accepted = world
                .run_system_with(
                    handler.0,
                    PanelActionInvocation {
                        entity: source,
                        action: if command == WindowCommand::Find {
                            PanelAction::Find
                        } else {
                            PanelAction::GoToLine
                        },
                    },
                )
                .map_err(io::Error::other)?
                .map_err(io::Error::other)?;
            if accepted {
                focus_panel(world, source, true)?;
                AppRequestOutcome::Applied
            } else {
                AppRequestOutcome::Ignored
            }
        }
        WindowCommand::SplitRight | WindowCommand::SplitDown | WindowCommand::DuplicateView => {
            let Some(panel) = request.panel else {
                return Ok(Some(AppRequestOutcome::Ignored));
            };
            if command == WindowCommand::DuplicateView {
                let presentation = panel_presentation(world, panel)
                    .ok_or_else(|| io::Error::other("Panel is unavailable"))?;
                let state =
                    bed_workbench::panel_state(world, presentation).map_err(io::Error::other)?;
                let binding = world.get::<PanelBinding>(panel).cloned().unwrap();
                let kind = world.get::<PanelRecord>(panel).unwrap().panel_type.clone();
                let panel = open_panel(
                    world,
                    &kind,
                    binding.input,
                    state,
                    binding.viewer,
                    Some(request.area),
                    false,
                )?;
                AppRequestOutcome::Opened {
                    panel,
                    document: world
                        .get::<PanelBinding>(panel)
                        .and_then(|binding| binding.document),
                }
            } else {
                let panel = split_panel(
                    world,
                    panel,
                    usize::from(command == WindowCommand::SplitDown),
                    true,
                )?;
                AppRequestOutcome::Opened {
                    panel,
                    document: world
                        .get::<PanelBinding>(panel)
                        .and_then(|binding| binding.document),
                }
            }
        }
        WindowCommand::OpenFile => {
            return apply_host_action(
                world,
                HostAction::OpenFileDialog { viewer: None },
                request,
                id,
            );
        }
        WindowCommand::OpenFolder => {
            return apply_workspace_action(world, WorkspaceAction::OpenFolder, request, id);
        }
        WindowCommand::UseLayout(slot) => return layouts::request_switch(world, slot, id),
        WindowCommand::SetCurrentLayout => {
            layouts::save_default(world, world.resource::<LayoutSlots>().active())?;
            AppRequestOutcome::Applied
        }
        WindowCommand::SetLayoutDefault(slot) => {
            layouts::save_default(world, slot)?;
            AppRequestOutcome::Applied
        }
        WindowCommand::RestoreBuiltinLayout(slot) => {
            world
                .non_send_mut::<WorkspacePersistence>()
                .store
                .set_layout_default(slot, None)?;
            AppRequestOutcome::Applied
        }
        WindowCommand::SaveDefaultLayout => {
            save_default_layout(world)?;
            AppRequestOutcome::Applied
        }
        WindowCommand::ResetDefaultLayout => {
            world
                .non_send_mut::<WorkspacePersistence>()
                .store
                .set_layout_default(LayoutSlot::Layout1, None)?;
            AppRequestOutcome::Applied
        }
        WindowCommand::ResetLayout => {
            reset_layout(world)?;
            AppRequestOutcome::Applied
        }
        _ => AppRequestOutcome::Ignored,
    };
    if let AppRequestOutcome::Opened { panel, .. } = &outcome {
        place_opened_panel(world, *panel, Some(request.area), false, request.tab_index)?;
    }
    Ok(Some(outcome))
}

fn apply_workspace_action(
    world: &mut World,
    action: WorkspaceAction,
    request: &AppRequest,
    id: Option<AppRequestId>,
) -> io::Result<Option<AppRequestOutcome>> {
    if !cfg!(feature = "workspaces")
        && !matches!(
            action,
            WorkspaceAction::OpenFolder | WorkspaceAction::OpenLocalWorkspace(_)
        )
    {
        return Err(io::Error::other("Workspace support is disabled"));
    }
    let outcome = match action {
        WorkspaceAction::OpenFolder => {
            let Some(path) = rfd::FileDialog::new()
                .set_directory(&world.resource::<WorkspaceState>().working_directory)
                .pick_folder()
            else {
                return Ok(Some(AppRequestOutcome::Ignored));
            };
            return begin_saved_directory(world, &path, id);
        }
        WorkspaceAction::OpenLocalWorkspace(path) => {
            return begin_saved_directory(world, &path, id);
        }
        WorkspaceAction::OpenWorkspace(spec) => return begin_workspace(world, spec, id),
        WorkspaceAction::OpenWorkspaceWindow(spec) => {
            world.resource_mut::<WorkspaceState>().windows.push(spec);
            AppRequestOutcome::Applied
        }
        WorkspaceAction::RemoveRecent(path) => {
            world
                .resource_mut::<WorkspaceRepositoryResource>()
                .0
                .forget(&WorkspaceSpec::local(path.to_string_lossy()))?;
            world
                .non_send_mut::<WorkspacePersistence>()
                .store
                .reload()?;
            AppRequestOutcome::Applied
        }
        WorkspaceAction::RemoveWorkspace(spec) => {
            world
                .resource_mut::<WorkspaceRepositoryResource>()
                .0
                .forget(&spec)?;
            world
                .non_send_mut::<WorkspacePersistence>()
                .store
                .reload()?;
            AppRequestOutcome::Applied
        }
        WorkspaceAction::RenameWorkspace(spec, name) => {
            let renamed = bed_plugin_workspaces::rename_workspace(world, &spec, &name)?;
            world
                .non_send_mut::<WorkspacePersistence>()
                .store
                .reload()?;
            if world
                .resource::<WorkspaceState>()
                .spec
                .as_ref()
                .is_some_and(|current| current.identity() == spec.identity())
            {
                world.resource_mut::<WorkspaceState>().spec = Some(renamed);
            }
            AppRequestOutcome::Applied
        }
        WorkspaceAction::SaveCurrent { name } => {
            save_current_workspace(world, name)?;
            AppRequestOutcome::Applied
        }
        WorkspaceAction::Promote(root) => {
            return begin_terminal_promotion(world, &root, request, id);
        }
        WorkspaceAction::Persist => {
            persist_workspace(world)?;
            AppRequestOutcome::Applied
        }
        WorkspaceAction::Reconnect => {
            reconnect_workspace(world, id)?;
            return Ok(None);
        }
        WorkspaceAction::Error(message) => return Err(io::Error::other(message)),
    };
    if cfg!(feature = "workspaces") {
        bed_plugin_workspaces::refresh_workspace_observations(world)?;
    }
    Ok(Some(outcome))
}

fn apply_host_action(
    world: &mut World,
    host: HostAction,
    request: &AppRequest,
    id: Option<AppRequestId>,
) -> io::Result<Option<AppRequestOutcome>> {
    let relocate_panel = matches!(&host, HostAction::ShowPanelIn { .. });
    let outcome = match host {
        HostAction::ExternalFiles { panel, event } => {
            let mut captured = captured_effect(
                world,
                request,
                AppAction::ExternalFiles { panel, event },
                request.source,
                Some(request.area),
            );
            captured.arguments = request.arguments.clone();
            return apply_request(world, captured, id);
        }
        HostAction::PanelAction { panel, action } => {
            let captured = captured_effect(
                world,
                request,
                AppAction::PanelAction { panel, action },
                request.source,
                Some(request.area),
            );
            return apply_request(world, captured, id);
        }
        HostAction::Clipboard { panel, action } => {
            let effects = bed_core::dispatch_panel_clipboard(world, panel, action)
                .map_err(io::Error::other)?;
            let effects = effects
                .into_iter()
                .map(|effect| {
                    captured_effect(
                        world,
                        request,
                        AppAction::HostAction(effect.action),
                        effect.source,
                        effect.area,
                    )
                })
                .collect();
            return apply_batch(world, effects, id, AppRequestOutcome::Applied);
        }

        HostAction::ChooseDirectory => {
            return apply_workspace_action(world, WorkspaceAction::OpenFolder, request, id);
        }
        HostAction::OpenDirectory { location } => match location.provider {
            bed_files::FileProviderId::Local => return begin_directory(world, &location.path, id),
            bed_files::FileProviderId::Ssh { host } => {
                let spec = WorkspaceSpec {
                    name: bed_plugin_workspaces::default_name(&location.path.to_string_lossy()),
                    target: WorkspaceTarget::Ssh { host },
                    root: location.path.to_string_lossy().into_owned(),
                };
                start_connection(world, spec, false, id)?;
                return Ok(None);
            }
        },
        HostAction::OpenTerminalFiles { paths } => {
            if world.non_send::<DocumentSession>().is_remote() {
                return Err(io::Error::other(
                    "Local shell file requests require a local workspace",
                ));
            }
            let mut area = request.area;
            let mut outcome = AppRequestOutcome::Ignored;
            for path in paths {
                let area_count = world.resource::<WorkbenchLayout>().tiling.areas.len();
                let panel = open_file(world, &path, None, false, Some(area))?;
                if world.resource::<WorkbenchLayout>().tiling.areas.len() != area_count {
                    // The first file split a sole terminal; put the rest in the
                    // newly created editor area as well.
                    area = world
                        .resource::<WorkbenchLayout>()
                        .tiling
                        .area_for(world.get::<PanelPresentation>(panel).unwrap().id)
                        .unwrap();
                }
                outcome = AppRequestOutcome::Opened {
                    panel,
                    document: world
                        .get::<PanelBinding>(panel)
                        .and_then(|binding| binding.document),
                };
            }
            return Ok(Some(outcome));
        }
        HostAction::OpenTerminalLink { path, position } => {
            let host = if let Some((row, column)) = position {
                HostAction::RevealSource {
                    path: path.to_string_lossy().into_owned(),
                    row,
                    column,
                    center: true,
                }
            } else {
                HostAction::OpenFile {
                    path: path.to_string_lossy().into_owned(),
                    viewer: None,
                }
            };
            return apply_host_action(world, host, request, id);
        }
        HostAction::Command { command, context } => {
            let mut captured = capture_app_request_from(
                world,
                AppAction::PluginCommand(command),
                PanelTarget::LargestArea,
                request.source,
            );
            captured.context = context;
            captured.area = request.area;
            captured.tab_index = request.tab_index;
            return apply_request(world, captured, id);
        }
        HostAction::ClosePanels { panel_type } => {
            let panels = active_panel_entities(world)
                .into_iter()
                .filter(|entity| {
                    world
                        .get::<PanelRecord>(*entity)
                        .is_some_and(|record| record.panel_type == panel_type)
                })
                .collect();
            return begin_close(world, panels, false, id);
        }
        HostAction::FocusView { view } => {
            let panel = world
                .query::<(Entity, &PanelPresentation, &PanelBinding)>()
                .iter(world)
                .find_map(|(entity, _, binding)| {
                    (bed_api::workbench::panel_active(world, entity) && binding.view == Some(view))
                        .then_some(entity)
                });
            if let Some(panel) = panel {
                focus_panel(world, panel, true)?;
                AppRequestOutcome::Applied
            } else {
                AppRequestOutcome::Ignored
            }
        }
        HostAction::ShowTerminal { id: terminal } => {
            let panel = world
                .query::<(Entity, &PanelPresentation, &PanelBinding)>()
                .iter(world)
                .find_map(|(entity, _, binding)| {
                    (bed_api::workbench::panel_active(world, entity)
                        && binding.terminal == Some(terminal))
                    .then_some(entity)
                });
            let panel = match panel {
                Some(panel) => {
                    focus_panel(world, panel, true)?;
                    panel
                }
                None => open_panel(
                    world,
                    "bed.terminal.panel",
                    PanelInput::None,
                    json!({"session":terminal}),
                    None,
                    Some(request.area),
                    false,
                )?,
            };
            AppRequestOutcome::Opened {
                panel,
                document: None,
            }
        }
        HostAction::Invalidate => AppRequestOutcome::Applied,
        HostAction::ShowPanel {
            panel_type,
            document,
            state,
            action,
        }
        | HostAction::ShowPanelIn {
            panel_type,
            document,
            state,
            action,
            ..
        } => {
            let panel = open_panel(
                world,
                &panel_type,
                document.into(),
                state,
                None,
                Some(request.area),
                true,
            )?;
            if relocate_panel {
                let panel_id = world.get::<PanelPresentation>(panel).unwrap().id;
                world
                    .resource_mut::<WorkbenchLayout>()
                    .tiling
                    .assign(panel_id, request.area);
                bed_workbench::focus_panel(world, panel);
            }
            if let Some(action) = action {
                dispatch_panel_action(world, panel, action)?;
            }
            AppRequestOutcome::Opened { panel, document }
        }
        HostAction::OpenPanel {
            panel_type,
            document,
            state,
        } => {
            let panel = open_panel(
                world,
                &panel_type,
                document.into(),
                state,
                None,
                Some(request.area),
                false,
            )?;
            AppRequestOutcome::Opened { panel, document }
        }
        HostAction::OpenFile { path, viewer } | HostAction::OpenFileIn { path, viewer, .. } => {
            return begin_open_file(
                world,
                Path::new(&path),
                viewer.as_deref(),
                false,
                false,
                Some(request.area),
                request.tab_index,
                id,
                FileOpenMode::Keep,
            );
        }
        HostAction::PreviewFileIn { path, viewer, .. } => {
            return begin_open_file(
                world,
                Path::new(&path),
                viewer.as_deref(),
                false,
                false,
                Some(request.area),
                request.tab_index,
                id,
                FileOpenMode::Preview,
            );
        }
        HostAction::DropFile { path } => {
            return begin_open_file(
                world,
                Path::new(&path),
                None,
                false,
                true,
                Some(request.area),
                request.tab_index,
                id,
                FileOpenMode::Keep,
            );
        }
        HostAction::OpenFileDialog { viewer } => {
            let extensions = viewer
                .as_deref()
                .and_then(|viewer| {
                    world
                        .resource::<bed_api::features::FeatureRegistry>()
                        .viewer(viewer)
                })
                .map(|viewer| viewer.extensions)
                .unwrap_or(&[]);
            let directory = world.resource::<WorkspaceState>().working_directory.clone();
            let path = if let Some(mut dialogs) =
                world.get_non_send_mut::<Box<dyn bed_api::workbench::FileDialogService>>()
            {
                dialogs.pick_file(&directory, extensions)
            } else {
                rfd::FileDialog::new().set_directory(&directory).pick_file()
            };
            let Some(path) = path else {
                return Ok(Some(AppRequestOutcome::Ignored));
            };
            let panel = open_file(world, &path, viewer.as_deref(), false, Some(request.area))?;
            AppRequestOutcome::Opened {
                panel,
                document: world
                    .get::<PanelBinding>(panel)
                    .and_then(|binding| binding.document),
            }
        }
        HostAction::OpenDocumentViewer {
            tab,
            document,
            viewer,
        } => {
            let panel = request
                .panel
                .filter(|entity| {
                    world
                        .get::<PanelBinding>(*entity)
                        .is_some_and(|binding| binding.document == Some(document))
                })
                .or_else(|| {
                    active_panel_entities(world).into_iter().find(|entity| {
                        world
                            .get::<PanelPresentation>(*entity)
                            .is_some_and(|panel| panel.id == tab)
                            && world
                                .get::<PanelBinding>(*entity)
                                .is_some_and(|binding| binding.document == Some(document))
                    })
                });
            if let Some(panel) = panel {
                let panel = open_viewer(world, panel, &viewer)?;
                AppRequestOutcome::Opened {
                    panel,
                    document: Some(document),
                }
            } else {
                AppRequestOutcome::Ignored
            }
        }
        HostAction::Navigate {
            document,
            byte_offset,
            revision,
        } => {
            if world
                .non_send::<DocumentSession>()
                .document_revision(document)?
                != revision
            {
                return Ok(Some(AppRequestOutcome::Ignored));
            }
            let panel = request
                .panel
                .filter(|entity| {
                    world.get::<PanelBinding>(*entity).is_some_and(|binding| {
                        binding.document == Some(document) && binding.view.is_some()
                    })
                })
                .or_else(|| {
                    active_panel_entities(world).into_iter().find(|entity| {
                        world.get::<PanelBinding>(*entity).is_some_and(|binding| {
                            binding.document == Some(document) && binding.view.is_some()
                        })
                    })
                });
            let Some(panel) = panel else {
                return Ok(Some(AppRequestOutcome::Ignored));
            };
            let view = world.get::<PanelBinding>(panel).unwrap().view.unwrap();
            let (row, column) = world
                .non_send::<DocumentSession>()
                .with_document(document, |state| state.row_col_from_offset(byte_offset))?;
            world
                .non_send_mut::<DocumentSession>()
                .with_view(view, |view| {
                    view.commands().set_cursor(
                        row,
                        column,
                        false,
                        bed_editing::text_commands::CursorReveal::Center,
                    )
                })?;
            focus_panel(world, panel, true)?;
            AppRequestOutcome::Applied
        }
        HostAction::NavigateLocation {
            path,
            row,
            utf16_column,
            origin,
        } => {
            if origin.is_some_and(|origin| {
                !world
                    .non_send::<DocumentSession>()
                    .accepts_lsp_origin(&origin)
            }) {
                return Ok(Some(AppRequestOutcome::Ignored));
            }
            let path = if world.non_send::<DocumentSession>().is_remote() {
                PathBuf::from(path)
            } else {
                std::fs::canonicalize(path)?
            };
            let document = world.non_send::<DocumentSession>().document_for_path(&path);
            let requested_view = origin.map(|origin| origin.view_id).or_else(|| {
                request
                    .source
                    .and_then(|source| world.get::<PanelBinding>(source))
                    .and_then(|binding| binding.view)
            });
            let source = requested_view.and_then(|view| {
                world
                    .query::<(Entity, &PanelBinding)>()
                    .iter(world)
                    .find_map(|(entity, binding)| {
                        (binding.view == Some(view)
                            && binding.document == document
                            && document.is_some())
                        .then_some(entity)
                    })
            });
            let panel = if let Some(source) = source {
                bed_api::workbench::panel_owner(world, source)
                    .expect("live editor presentation belongs to a layout panel")
            } else {
                match open_file_with_origin(
                    world,
                    &path,
                    Some("bed.editor.text"),
                    false,
                    Some(request.area),
                    origin,
                    FileOpenMode::Preview,
                ) {
                    Ok(panel) => panel,
                    Err(error)
                        if error.kind() == io::ErrorKind::WouldBlock
                            && world.non_send::<DocumentSession>().is_remote() =>
                    {
                        let follow_up = captured_effect(
                            world,
                            request,
                            AppAction::HostAction(HostAction::NavigateLocation {
                                path: path.to_string_lossy().into_owned(),
                                row,
                                utf16_column,
                                origin,
                            }),
                            request.source,
                            None,
                        );
                        world
                            .non_send_mut::<RemoteOperations>()
                            .opens
                            .push(RemoteOpen {
                                session_id: request.session_id,
                                id,
                                path,
                                viewer: Some("bed.editor.text".into()),
                                additional: false,
                                mode: FileOpenMode::Preview,
                                layout_slot: request.layout_slot,
                                move_existing: false,
                                area: Some(request.area),
                                tab_index: request.tab_index,
                                restore: None,
                                follow_up: Some(follow_up),
                            });
                        return Ok(None);
                    }
                    Err(error) => return Err(error),
                }
            };
            let binding = world
                .get::<PanelBinding>(panel)
                .expect("text panel has a document binding");
            let document = binding.document.expect("text panel has a document");
            let view = binding.view.expect("text panel has a managed view");
            let column = world
                .non_send::<DocumentSession>()
                .with_document(document, |state| {
                    bed_editing::util::utf8::utf16_to_utf8_byte_offset(
                        &state.line(row),
                        utf16_column,
                    )
                })?;
            world
                .non_send_mut::<DocumentSession>()
                .with_commands(view, |commands| {
                    commands.set_cursor(
                        row,
                        column,
                        false,
                        bed_editing::text_commands::CursorReveal::Center,
                    )
                })?;
            focus_panel(world, panel, true)?;
            AppRequestOutcome::Applied
        }
        HostAction::SaveAsDialog { document } => {
            let mut captured = captured_effect(
                world,
                request,
                AppAction::Command(WindowCommand::SaveAs),
                request.source,
                None,
            );
            captured.context.document = Some(document);
            return window_command(world, WindowCommand::SaveAs, &captured, id);
        }
        ref navigation @ HostAction::RevealSource {
            ref path,
            row,
            column,
            center,
        }
        | ref navigation @ HostAction::PreviewSource {
            ref path,
            row,
            column,
            center,
        } => {
            let source_mode = if matches!(navigation, HostAction::PreviewSource { .. }) {
                FileOpenMode::Preview
            } else {
                FileOpenMode::Keep
            };
            let panel = match open_file_with_origin(
                world,
                Path::new(path),
                Some("bed.editor.text"),
                false,
                Some(request.area),
                None,
                source_mode,
            ) {
                Ok(panel) => panel,
                Err(error)
                    if error.kind() == io::ErrorKind::WouldBlock
                        && world.non_send::<DocumentSession>().is_remote() =>
                {
                    let navigation = if source_mode == FileOpenMode::Preview {
                        HostAction::PreviewSource {
                            path: path.clone(),
                            row,
                            column,
                            center,
                        }
                    } else {
                        HostAction::RevealSource {
                            path: path.clone(),
                            row,
                            column,
                            center,
                        }
                    };
                    let follow_up = captured_effect(
                        world,
                        request,
                        AppAction::HostAction(navigation),
                        request.source,
                        None,
                    );
                    world
                        .non_send_mut::<RemoteOperations>()
                        .opens
                        .push(RemoteOpen {
                            session_id: request.session_id,
                            id,
                            path: PathBuf::from(path),
                            viewer: Some("bed.editor.text".into()),
                            additional: false,
                            mode: source_mode,
                            layout_slot: request.layout_slot,
                            move_existing: false,
                            area: Some(request.area),
                            tab_index: request.tab_index,
                            restore: None,
                            follow_up: Some(follow_up),
                        });
                    return Ok(None);
                }
                Err(error) => return Err(error),
            };
            if let Some(view) = world
                .get::<PanelBinding>(panel)
                .and_then(|binding| binding.view)
            {
                world
                    .non_send_mut::<DocumentSession>()
                    .with_view(view, |view| {
                        view.commands().set_cursor(
                            row,
                            column,
                            false,
                            if center {
                                bed_editing::text_commands::CursorReveal::Center
                            } else {
                                bed_editing::text_commands::CursorReveal::Ensure
                            },
                        )
                    })?;
            }
            AppRequestOutcome::Opened {
                panel,
                document: world
                    .get::<PanelBinding>(panel)
                    .and_then(|binding| binding.document),
            }
        }
        HostAction::ApplyEdits {
            document,
            revision,
            edits,
        } => {
            commit_documents(world, &HashSet::from([document]))?;
            // The direct core operation preserves typed I/O failures used by
            // application batches; the message dispatcher exposes string errors.
            bed_core::apply_edits(world, document, revision, &edits)?;
            AppRequestOutcome::Applied
        }
        action @ (HostAction::ApplyEditsWithResult { .. }
        | HostAction::ApplyModuleEditsWithResult { .. }
        | HostAction::CommitDocumentsWithResult { .. }
        | HostAction::SaveDocumentsWithResult { .. }
        | HostAction::Undo { .. }
        | HostAction::Redo { .. }) => {
            return apply_core_document_action(world, action, request, id);
        }
        HostAction::Save { document } => {
            let mut captured = captured_effect(
                world,
                request,
                AppAction::Command(WindowCommand::Save),
                request.source,
                None,
            );
            captured.context.document = Some(document);
            return window_command(world, WindowCommand::Save, &captured, id);
        }
        action @ HostAction::OperationBarrier { .. } => {
            let captured = bed_core::capture_host_request(world, request.source, action);
            bed_core::apply_document_request(world, captured).map_err(io::Error::other)?;
            AppRequestOutcome::Applied
        }
        HostAction::Notify { message } => {
            world.resource_mut::<WorkspaceState>().error = Some(message);
            AppRequestOutcome::Applied
        }
    };
    if let AppRequestOutcome::Opened { panel, .. } = &outcome {
        place_opened_panel(world, *panel, Some(request.area), false, request.tab_index)?;
    }
    Ok(Some(outcome))
}

// Execute shared document behavior at bEd's admitted request point. Only the
// application receipt and any later compound transition stay application-owned.
fn apply_core_document_action(
    world: &mut World,
    action: HostAction,
    request: &AppRequest,
    id: Option<AppRequestId>,
) -> io::Result<Option<AppRequestOutcome>> {
    let captured = bed_api::features::HostRequest::capture(
        bed_documents::SessionId(request.session_id),
        request.source,
        action,
        request.area,
        world.resource::<bed_workbench::WorkbenchLayoutEpoch>().0,
    );
    let outcome = bed_core::apply_document_request(world, captured).map_err(io::Error::other)?;
    match outcome {
        bed_core::HostOutcome::Applied => Ok(Some(AppRequestOutcome::Applied)),
        bed_core::HostOutcome::Pending(save) => track_document_save(world, save, None, id),
        bed_core::HostOutcome::Saved {
            documents,
            revisions,
        } => Ok(Some(AppRequestOutcome::Saved {
            documents: documents
                .into_iter()
                .zip(revisions)
                .map(|(document, revision)| SavedDocument { document, revision })
                .collect(),
        })),
        _ => unreachable!("ordinary document actions do not open panels or replace contexts"),
    }
}

pub fn dispatch_panel_action(
    world: &mut World,
    panel: Entity,
    action: PanelAction,
) -> io::Result<bool> {
    let Some(entity) = (if world.get::<PanelOwner>(panel).is_some() {
        Some(panel)
    } else {
        panel_presentation(world, panel)
    }) else {
        return Ok(false);
    };
    let Some(handler) = world.get::<PanelActions>(entity).copied() else {
        return Ok(false);
    };
    world
        .run_system_with(handler.0, PanelActionInvocation { entity, action })
        .map_err(io::Error::other)?
        .map_err(io::Error::other)
}

fn session_options(world: &World, root: Option<PathBuf>) -> SessionOptions {
    let settings = world.non_send::<Settings>();
    let project = root.is_some();
    SessionOptions {
        project_root: root,
        autosave: world.non_send::<DocumentSession>().options().autosave,
        monitoring: true,
        git: world.non_send::<DocumentSession>().options().git,
        highlighting: world.non_send::<DocumentSession>().options().highlighting,
        history_path: world
            .resource::<WorkspaceState>()
            .spec
            .as_ref()
            .and_then(|spec| {
                world
                    .non_send::<WorkspacePersistence>()
                    .store
                    .history_path(spec)
            }),
        lsp_config: project.then(|| settings.config_dir.join("lsp.json")),
        lsp_config_mode: bed_documents::LspConfigMode::Layered,
        lsp_file_observations: project,
    }
}

fn snapshot_layout(
    world: &mut World,
    panels: Vec<Entity>,
    layout: &WorkbenchLayout,
    navigation: &DocumentNavigation,
) -> io::Result<Value> {
    let mut records = Vec::new();
    for entity in panels {
        let presentation = world.get::<PanelPresentation>(entity).unwrap().clone();
        let binding = world
            .get::<PanelBinding>(entity)
            .cloned()
            .unwrap_or_default();
        let record = world.get::<PanelRecord>(entity).unwrap().clone();
        let active = panel_presentation(world, entity);
        if active
            .and_then(|entity| world.get::<bed_api::workbench::PanelPersistence>(entity))
            .is_some_and(|policy| !policy.0)
        {
            continue;
        }
        let mut value = record.serialized.clone();
        value["id"] = json!(presentation.id);
        value["panel_type"] = json!(record.panel_type);
        value["viewer"] = json!(binding.viewer);
        if record.unavailable.is_none() {
            let state = active
                .and_then(|entity| bed_workbench::panel_state(world, entity).ok())
                .unwrap_or(value["state"].clone());
            value["state"] = state;
            if let Some(path) = binding.input.local_file() {
                value["path"] = json!(path);
                value["backing"] = json!("local_file");
            } else if let Some(document) = binding.document {
                let snapshot = world.non_send::<DocumentSession>().snapshot(document)?;
                if world
                    .non_send::<DocumentSession>()
                    .is_snapshot_document(document)
                {
                    continue;
                }
                if snapshot.path.is_empty() {
                    if !snapshot.bytes.is_empty() || snapshot.dirty {
                        continue;
                    }
                    value["default_input"] = json!(true);
                } else {
                    value["path"] = json!(snapshot.path);
                    value["document_kind"] =
                        json!(if snapshot.kind == bed_documents::DocumentKind::Bytes {
                            "bytes"
                        } else {
                            "text"
                        });
                }
            } else if binding.terminal.is_some() {
                value["default_input"] = json!(true);
                if let Some(state) = value["state"].as_object_mut() {
                    state.remove("session");
                }
            }
        }
        records.push(value);
    }
    let active_document_panel = navigation
        .last_document_panel
        .and_then(|panel| world.get::<PanelPresentation>(panel))
        .map(|panel| panel.id)
        .filter(|id| {
            records
                .iter()
                .any(|record| record["id"].as_u64() == Some(*id))
        });
    let value = json!({"version":2,"panels":records,"focused_area":layout.focused_area,"tiling":layout.tiling.to_value(records.iter().filter_map(|record|record["id"].as_u64())),
        "ini":"","focused":layout.focused.and_then(|entity|world.get::<PanelPresentation>(entity)).map(|panel|panel.id),
        "active_document_panel":active_document_panel});
    Ok(value)
}

pub fn persist_workspace(world: &mut World) -> io::Result<()> {
    let Some(spec) = world.resource::<WorkspaceState>().spec.clone() else {
        return Ok(());
    };
    if !cfg!(feature = "workspaces") {
        return Ok(());
    }
    let layouts = layouts::snapshot(world)?;
    let previous = world
        .resource::<WorkspaceRepositoryResource>()
        .0
        .load(&spec)?;
    let plugins =
        bed_storage::collect_snapshot(world, previous.as_ref().map(|snapshot| &snapshot.plugins))?;
    world
        .resource_mut::<WorkspaceRepositoryResource>()
        .0
        .commit(
            spec,
            bed_plugin_workspaces::WorkspaceSnapshot {
                layouts: layouts.clone(),
                plugins,
            },
        )?;
    refresh_workspace_persistence(world);
    let mut persistence = world.non_send_mut::<WorkspacePersistence>();
    persistence.last_value = Some(layouts);
    persistence.last_saved = Instant::now();
    Ok(())
}

fn refresh_workspace_persistence(world: &mut World) {
    if let Some(error) = world
        .resource_mut::<WorkspaceRepositoryResource>()
        .0
        .take_index_error()
    {
        world.resource_mut::<WorkspaceState>().error = Some(error);
    }
    if let Err(error) = world.non_send_mut::<WorkspacePersistence>().store.reload() {
        world.resource_mut::<WorkspaceState>().error = Some(format!(
            "Workspace saved, but the client persistence index could not be refreshed: {error}"
        ));
    }
    if let Err(error) = bed_plugin_workspaces::refresh_workspace_observations(world) {
        world.resource_mut::<WorkspaceState>().error = Some(format!(
            "Workspace saved, but recent workspaces could not be refreshed: {error}"
        ));
    }
}

/// Saving attaches persistence to the live session without replacing or restoring it.
pub fn save_current_workspace(
    world: &mut World,
    name: Option<String>,
) -> io::Result<WorkspaceSpec> {
    if !cfg!(feature = "workspaces") {
        return Err(io::Error::other("Workspace support is disabled"));
    }
    let snapshot = layouts::snapshot(world)?;
    world
        .resource_mut::<bed_plugin_workspaces::WorkspacesState>()
        .error = None;
    let spec = bed_plugin_workspaces::save_current_workspace(world, name, snapshot.clone())?;
    if let Some(error) = world
        .resource::<bed_plugin_workspaces::WorkspacesState>()
        .error
        .clone()
    {
        world.resource_mut::<WorkspaceState>().error = Some(error);
    }
    world.resource_mut::<WorkspaceState>().spec = Some(spec.clone());
    refresh_workspace_persistence(world);
    let mut options = world.non_send::<DocumentSession>().options().clone();
    options.history_path = world
        .non_send::<WorkspacePersistence>()
        .store
        .history_path(&spec);
    if let Err(error) = world.non_send_mut::<DocumentSession>().configure(options) {
        world.resource_mut::<WorkspaceState>().error = Some(format!(
            "Workspace saved, but undo history could not be attached: {error}"
        ));
    }
    let mut persistence = world.non_send_mut::<WorkspacePersistence>();
    persistence.last_value = Some(snapshot);
    persistence.last_saved = Instant::now();
    Ok(spec)
}

fn restore_contributions(world: &mut World) -> io::Result<()> {
    publish_app_state(world);
    configure_feature_targets(world);
    let spec = world.resource::<WorkspaceState>().spec.clone();
    let state = match &spec {
        Some(spec) => world
            .resource::<WorkspaceRepositoryResource>()
            .0
            .load(spec)?
            .map(|snapshot| snapshot.plugins),
        None => None,
    };
    bed_storage::restore_snapshot(world, state.as_ref())
}

pub fn restore_workspace(world: &mut World) -> io::Result<()> {
    restore_contributions(world)?;
    let value = layouts::load(world)?;
    match value {
        Some(value) => restore_layout(world, &value, false),
        None => compose_default_layout(world),
    }
}

pub fn restore_layout(world: &mut World, value: &Value, attach: bool) -> io::Result<()> {
    restore_layout_with_terminal(world, value, attach, None)
}

fn restore_layout_with_terminal(
    world: &mut World,
    value: &Value,
    attach: bool,
    terminal: Option<Entity>,
) -> io::Result<()> {
    use bed_workbench::workspace::tiling_state::TilingState;
    if value["version"] != 2 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Saved layout version must be 2",
        ));
    }
    let mut records = value["panels"]
        .as_array()
        .ok_or_else(|| io::Error::other("Saved panels must be an array"))?
        .clone();
    let carried = if attach {
        active_panel_entities(world)
    } else {
        Vec::new()
    };
    let all_panels = panel_entities(world);
    let mut used = all_panels
        .iter()
        .map(|entity| world.get::<PanelPresentation>(*entity).unwrap().id)
        .collect::<HashSet<_>>();
    if records.iter().any(|record| !record.is_object()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Saved panels must be objects",
        ));
    }
    let terminal_slot = terminal.and_then(|_| {
        records
            .iter()
            .position(|record| record["panel_type"] == "bed.terminal.panel")
    });
    let mut seen = HashSet::new();
    let recovered_ids = records.iter().any(|record| {
        record["id"]
            .as_u64()
            .filter(|id| *id > 0 && *id <= u32::MAX as u64)
            .is_none_or(|id| !seen.insert(id))
    });
    if recovered_ids {
        world.resource_mut::<WorkspaceState>().error = Some(
            "Saved panel identities were invalid; restored the contents in a fresh layout".into(),
        );
    }
    let mut remap = HashMap::new();
    let mut next = records
        .iter()
        .filter_map(|record| record["id"].as_u64().filter(|id| *id <= u32::MAX as u64))
        .chain(used.iter().copied())
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    let mut recovered_next = 1;
    for (index, record) in records.iter_mut().enumerate() {
        let saved = record["id"].as_u64().unwrap_or(0);
        let id = if Some(index) == terminal_slot {
            world
                .get::<PanelPresentation>(terminal.unwrap())
                .unwrap()
                .id
        } else if recovered_ids {
            while used.contains(&recovered_next) {
                recovered_next += 1;
            }
            let id = recovered_next;
            recovered_next += 1;
            id
        } else if used.contains(&saved) {
            let id = next;
            next = next
                .checked_add(1)
                .ok_or_else(|| io::Error::other("Panel IDs exhausted"))?;
            id
        } else {
            saved
        };
        used.insert(id);
        remap.entry(saved).or_insert(id);
        record["id"] = json!(id);
    }
    let saved_ids = remap.keys().copied().collect::<Vec<_>>();
    let parsed = if recovered_ids {
        None
    } else if value["tiling"].is_object() {
        match TilingState::from_value(&value["tiling"], saved_ids.iter().copied()) {
            Ok(tiling) => Some(tiling),
            Err(error) => {
                world.resource_mut::<WorkspaceState>().error = Some(format!(
                    "Saved layout was invalid; restored the contents in a fresh layout: {error}"
                ));
                None
            }
        }
    } else {
        None
    };
    let mut tiling = match parsed {
        Some(mut tiling) => {
            tiling.remap_panel_ids(&remap);
            tiling
        }
        None => {
            let mut tiling = TilingState::default();
            for record in &records {
                tiling.assign(record["id"].as_u64().unwrap(), 1);
            }
            tiling
        }
    };
    if !attach && !active_panel_entities(world).is_empty() {
        return Err(io::Error::other("Saved layout requires an empty editor"));
    }
    let mut restored = HashMap::new();
    for (index, mut record) in records.into_iter().enumerate() {
        let id = record["id"].as_u64().unwrap();
        if Some(index) == terminal_slot {
            restored.insert(id, terminal.unwrap());
            continue;
        }
        let kind = record["panel_type"]
            .as_str()
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Saved panel must name its panel_type",
                )
            })?
            .to_owned();
        record["panel_type"] = json!(kind);
        let viewer = record["viewer"].as_str().map(str::to_owned);
        let descriptor = viewer
            .as_deref()
            .and_then(|viewer| {
                world
                    .resource::<bed_api::features::FeatureRegistry>()
                    .viewer(viewer)
            })
            .cloned();
        let mut unavailable = None;
        let input = if let Some(path) = record["path"].as_str().filter(|path| !path.is_empty()) {
            if descriptor
                .as_ref()
                .is_some_and(|viewer| viewer.is_local_file())
                || record["backing"] == "local_file"
            {
                PanelInput::LocalFile(PathBuf::from(path))
            } else {
                let kind = if record["document_kind"] == "bytes" {
                    bed_documents::DocumentKind::Bytes
                } else {
                    descriptor
                        .as_ref()
                        .and_then(|viewer| viewer.document_kind())
                        .unwrap_or(bed_documents::DocumentKind::Text)
                };
                match world
                    .non_send_mut::<DocumentSession>()
                    .open_file_with_kind(Path::new(path), kind)
                {
                    Ok(document) => PanelInput::Document(document),
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        let panel = queue_remote_restore(world, record, tiling.area_for(id))?;
                        restored.insert(id, panel);
                        continue;
                    }
                    Err(error) => {
                        unavailable = Some(error.to_string());
                        world.resource_mut::<WorkspaceState>().error = unavailable.clone();
                        PanelInput::None
                    }
                }
            }
        } else {
            PanelInput::None
        };
        let panel =
            bed_workbench::spawn_panel(world, record.clone(), tiling.area_for(id), Some(id))
                .map_err(io::Error::other)?;
        world.get_mut::<PanelRecord>(panel).unwrap().unavailable = unavailable;
        let binding = PanelBinding {
            input,
            viewer,
            session_id: world.non_send::<DocumentSession>().session_id().0,
            ..Default::default()
        };
        world.entity_mut(panel).insert(binding.clone());
        let state = record
            .get("state")
            .cloned()
            .unwrap_or_else(|| record.clone());
        mount_presentation(world, panel, &kind, binding, state)?;
        // Preserve the original record for unavailable features.
        world.get_mut::<PanelRecord>(panel).unwrap().serialized = record;
        restored.insert(id, panel);
    }
    let destination = tiling
        .layout
        .areas
        .iter()
        .max_by_key(|area| {
            (area.rect.max[0] - area.rect.min[0]) * (area.rect.max[1] - area.rect.min[1])
        })
        .unwrap()
        .id;
    for entity in carried {
        if restored.values().any(|restored| *restored == entity) {
            continue;
        }
        let id = world.get::<PanelPresentation>(entity).unwrap().id;
        tiling.assign(id, destination);
    }
    let live_ids = active_panel_entities(world)
        .iter()
        .map(|entity| world.get::<PanelPresentation>(*entity).unwrap().id)
        .collect::<Vec<_>>();
    tiling.retain_panels(live_ids);
    let focused_area = value["focused_area"]
        .as_u64()
        .and_then(|area| u32::try_from(area).ok())
        .filter(|area| tiling.areas.iter().any(|group| group.area == *area))
        .or_else(|| tiling.areas.first().map(|group| group.area));
    world.resource_mut::<WorkbenchLayout>().tiling = tiling;
    world.resource_mut::<WorkbenchLayout>().focused_area = focused_area;
    let active = value["active_document_panel"]
        .as_u64()
        .and_then(|id| remap.get(&id))
        .and_then(|id| restored.get(id))
        .copied();
    if (!attach
        || world
            .resource::<DocumentNavigation>()
            .last_document_panel
            .is_none())
        && active.is_some()
    {
        world
            .resource_mut::<DocumentNavigation>()
            .last_document_panel = active;
    }
    let focus = value["focused"]
        .as_u64()
        .and_then(|id| remap.get(&id))
        .and_then(|id| restored.get(id))
        .copied();
    let carried_focus = world
        .resource::<WorkbenchLayout>()
        .focused
        .filter(|entity| world.entities().contains(*entity));
    world.resource_mut::<WorkbenchLayout>().focused = None;
    if let Some(panel) = if attach {
        carried_focus.or(focus)
    } else {
        focus
    } {
        focus_panel(world, panel, true)?;
    }
    Ok(())
}

fn compose_default_layout(world: &mut World) -> io::Result<()> {
    compose_default_layout_with_terminal(world, None)
}

fn compose_default_layout_with_terminal(
    world: &mut World,
    terminal: Option<Entity>,
) -> io::Result<()> {
    if world.resource::<WorkspaceState>().spec.is_none() {
        return Ok(());
    }
    let slot = world.resource::<LayoutSlots>().active();
    let template = layouts::template(world, slot);
    restore_layout_with_terminal(world, &template, true, terminal)
}

/// Compatibility helper: save the current arrangement into slot 1.
pub fn save_default_layout(world: &mut World) -> io::Result<()> {
    layouts::save_default(world, LayoutSlot::Layout1)
}

pub fn reset_layout(world: &mut World) -> io::Result<()> {
    layouts::reset(world)
}

fn begin_directory(
    world: &mut World,
    path: &Path,
    id: Option<AppRequestId>,
) -> io::Result<Option<AppRequestOutcome>> {
    let root = std::fs::canonicalize(path)?;
    if !root.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Directory must be a folder",
        ));
    }
    if cfg!(feature = "workspaces") {
        let probe = WorkspaceSpec::local(root.to_string_lossy());
        if let Some(spec) = world
            .resource::<WorkspaceRepositoryResource>()
            .0
            .find(&probe)?
        {
            return begin_workspace(world, spec, id);
        }
    }
    if !world.non_send::<DocumentSession>().is_remote()
        && world.resource::<WorkspaceState>().working_directory == root
    {
        let startup = world.resource::<LayoutSlots>().active() == LayoutSlot::Startup;
        layouts::enter_directory(world)?;
        return Ok(Some(if startup {
            AppRequestOutcome::Applied
        } else {
            AppRequestOutcome::Ignored
        }));
    }
    if !world.non_send::<DocumentSession>().is_remote() {
        adopt_directory(world, root)?;
        return Ok(Some(AppRequestOutcome::Applied));
    }
    let panels = panel_entities(world);
    begin_close_transition(world, panels, Continuation::Directory(root), id)
}

/// Picker and File-menu opens explicitly opt into workspace persistence.
fn begin_saved_directory(
    world: &mut World,
    path: &Path,
    id: Option<AppRequestId>,
) -> io::Result<Option<AppRequestOutcome>> {
    let root = std::fs::canonicalize(path)?;
    if !root.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Workspace must be a directory",
        ));
    }
    if !cfg!(feature = "workspaces") {
        return begin_directory(world, &root, id);
    }
    let spec = WorkspaceSpec::local(root.to_string_lossy());
    if let Some(saved) = world
        .resource::<WorkspaceRepositoryResource>()
        .0
        .find(&spec)?
    {
        return begin_workspace(world, saved, id);
    }
    if world.resource::<WorkspaceState>().spec.is_none()
        && !world.non_send::<DocumentSession>().is_remote()
    {
        // Attaching preserves already-open buffers, drafts and terminal processes.
        attach_workspace(world, &root)?;
        persist_workspace(world)?;
        return Ok(Some(AppRequestOutcome::Applied));
    }
    let panels = panel_entities(world);
    begin_close_transition(world, panels, Continuation::CreateWorkspace(root), id)
}

fn adopt_directory(world: &mut World, root: PathBuf) -> io::Result<()> {
    let session = world.non_send::<DocumentSession>().session_id();
    let mut documents = world.non_send::<DocumentSession>().document_ids();
    documents.extend(
        world
            .query::<&bed_core::ModelDocument>()
            .iter(world)
            .filter(|model| model.session_id() == session)
            .map(bed_core::ModelDocument::id),
    );
    commit_documents(world, &documents.into_iter().collect())?;
    persist_workspace(world)?;
    let context = bed_files::FileContext::local(&root)?;
    let services = bed_files::FileServices::local();
    let mut options = session_options(world, Some(root.clone()));
    options.history_path = None;
    world
        .non_send_mut::<DocumentSession>()
        .configure_file_services(options, &context, &services)?;
    world
        .resource_mut::<bed_files::FileContext>()
        .replace(context);
    world.insert_resource(services);
    world.insert_resource(WorkspaceAttachment::Temporary);
    let mut state = world.resource_mut::<WorkspaceState>();
    state.spec = None;
    state.project_root = root.to_string_lossy().into_owned();
    state.working_directory = root;
    layouts::enter_directory(world)?;
    publish_app_state(world);
    configure_feature_targets(world);
    Ok(())
}

fn install_context_session(world: &mut World, session: DocumentSession) -> io::Result<()> {
    if let Some(token) = world.resource::<PendingOperations>().barrier {
        bed_core::commit_context_change(world, token, session).map_err(io::Error::other)
    } else {
        // Startup has no prior user operation to hold.
        world
            .non_send_mut::<DocumentSession>()
            .shutdown(ClosePolicy::Discard)?;
        world.insert_non_send(session);
        Ok(())
    }
}

fn replace_directory(world: &mut World, root: PathBuf) -> io::Result<()> {
    let mut options = session_options(world, Some(root.clone()));
    options.history_path = None;
    let context = bed_files::FileContext::local(&root)?;
    let services = bed_files::FileServices::local();
    let session = DocumentSession::with_file_services(options, &context, &services)?;
    persist_workspace(world)?;
    if let Some(token) = world.resource::<PendingOperations>().barrier {
        bed_core::validate_document_transition(world, token).map_err(io::Error::other)?;
    }
    let panels = panel_entities(world);
    retire_panels(world, &panels)?;
    install_context_session(world, session)?;
    world.insert_resource(ActiveSessionId(
        world.non_send::<DocumentSession>().session_id().0,
    ));
    let context = bed_files::FileContext::local(&root)?;
    world
        .resource_mut::<bed_files::FileContext>()
        .replace(context);
    world.insert_resource(bed_files::FileServices::local());
    world.insert_resource(WorkspaceAttachment::Temporary);
    let mut state = world.resource_mut::<WorkspaceState>();
    state.spec = None;
    state.project_root = root.to_string_lossy().into_owned();
    state.working_directory = root;
    *world.resource_mut::<DocumentNavigation>() = DocumentNavigation::default();
    *world.resource_mut::<WorkbenchLayout>() = WorkbenchLayout::default();
    layouts::clear(world);
    cancel_foreign_pending(world);
    // Empty restoration explicitly clears installed plugins, with no disk lookup.
    bed_storage::restore_snapshot(world, None)?;
    layouts::initialize(world, LayoutSlot::Layout1)?;
    publish_app_state(world);
    configure_feature_targets(world);
    Ok(())
}

fn begin_workspace(
    world: &mut World,
    mut spec: WorkspaceSpec,
    id: Option<AppRequestId>,
) -> io::Result<Option<AppRequestOutcome>> {
    if matches!(spec.target, WorkspaceTarget::Local) {
        spec.root = std::fs::canonicalize(&spec.root)?
            .to_str()
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "Workspace path is not UTF-8")
            })?
            .to_owned();
    }
    if !cfg!(feature = "workspaces") {
        return Err(io::Error::other("Workspace support is disabled"));
    }
    if matches!(spec.target, WorkspaceTarget::Local)
        && world
            .resource::<WorkspaceRepositoryResource>()
            .0
            .find(&spec)?
            .is_none()
    {
        return begin_directory(world, Path::new(&spec.root), id);
    }
    if world
        .resource::<WorkspaceState>()
        .spec
        .as_ref()
        .is_some_and(|current| current.identity() == spec.identity())
    {
        return Ok(Some(AppRequestOutcome::Ignored));
    }
    if matches!(spec.target, WorkspaceTarget::Ssh { .. }) {
        start_connection(world, spec, false, id)?;
        return Ok(None);
    }
    let panels = panel_entities(world);
    begin_close_transition(world, panels, Continuation::Workspace(spec), id)
}

pub fn attach_workspace(world: &mut World, path: &Path) -> io::Result<()> {
    if !cfg!(feature = "workspaces") {
        return Err(io::Error::other("Workspace support is disabled"));
    }
    attach_workspace_with_terminal(world, path, None)
}

fn validate_terminal_promotion(world: &World, caller: Entity, terminal: u64) -> io::Result<()> {
    if world.resource::<WorkspaceState>().spec.is_some()
        || world.non_send::<DocumentSession>().is_remote()
    {
        return Err(io::Error::other(
            "Promotion requires a standalone local session",
        ));
    }
    if !bed_workbench::owns_panel(world, caller)
        || world
            .get::<PanelBinding>(caller)
            .and_then(|binding| binding.terminal)
            != Some(terminal)
    {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "The calling terminal is no longer open",
        ));
    }
    Ok(())
}

fn begin_terminal_promotion(
    world: &mut World,
    root: &Path,
    request: &AppRequest,
    id: Option<AppRequestId>,
) -> io::Result<Option<AppRequestOutcome>> {
    let caller = request.panel.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "The calling terminal is unavailable",
        )
    })?;
    let terminal = world
        .get::<PanelBinding>(caller)
        .and_then(|binding| binding.terminal)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "The calling terminal is unavailable",
            )
        })?;
    #[cfg(feature = "terminal")]
    if world
        .get_non_send::<bed_terminal::bed_terminal::BedTerminal>()
        .is_some_and(|backend| backend.is_remote_session(terminal))
    {
        return Err(io::Error::other(
            "Workspace promotion requires a live local terminal",
        ));
    }
    let root = std::fs::canonicalize(root)?;
    if !root.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Workspace must be a directory",
        ));
    }
    if !bed_workbench::owns_panel(world, caller) || world.non_send::<DocumentSession>().is_remote()
    {
        return Err(io::Error::other(
            "Workspace promotion requires a live local terminal",
        ));
    }
    if let Some(current) = &world.resource::<WorkspaceState>().spec {
        if Path::new(&current.root) == root {
            return Ok(Some(AppRequestOutcome::Ignored));
        }
        world
            .resource_mut::<WorkspaceState>()
            .windows
            .push(WorkspaceSpec::local(root.to_string_lossy()));
        return Ok(Some(AppRequestOutcome::Applied));
    }
    validate_terminal_promotion(world, caller, terminal)?;
    let panels = active_panel_entities(world)
        .into_iter()
        .filter(|panel| *panel != caller)
        .collect();
    begin_close_transition(
        world,
        panels,
        Continuation::Promotion {
            root,
            caller,
            terminal,
            panels: Vec::new(),
        },
        id,
    )
}

fn attach_workspace_with_terminal(
    world: &mut World,
    path: &Path,
    terminal: Option<Entity>,
) -> io::Result<()> {
    let root = std::fs::canonicalize(path)?;
    if !root.is_dir() {
        return Err(io::Error::other("Workspace must be a directory"));
    }
    if world.resource::<WorkspaceState>().spec.is_some()
        || world.non_send::<DocumentSession>().is_remote()
    {
        return Err(io::Error::other(
            "Attachment requires a standalone local session",
        ));
    }
    let documents = world
        .non_send::<DocumentSession>()
        .document_ids()
        .into_iter()
        .collect();
    commit_documents(world, &documents)?;
    persist_workspace(world)?;
    let probe = WorkspaceSpec::local(root.to_string_lossy());
    let spec = world
        .resource::<WorkspaceRepositoryResource>()
        .0
        .find(&probe)?
        .unwrap_or(probe);
    let snapshot = world
        .resource::<WorkspaceRepositoryResource>()
        .0
        .load(&spec)?
        .unwrap_or_else(|| bed_plugin_workspaces::WorkspaceSnapshot {
            layouts: Value::Null,
            plugins: json!({}),
        });
    let spec = world
        .resource_mut::<WorkspaceRepositoryResource>()
        .0
        .commit(spec, snapshot)?;
    refresh_workspace_persistence(world);
    let mut options = session_options(world, Some(root.clone()));
    options.history_path = world
        .non_send::<WorkspacePersistence>()
        .store
        .history_path(&spec);
    world.non_send_mut::<DocumentSession>().configure(options)?;
    {
        let mut workspace = world.resource_mut::<WorkspaceState>();
        workspace.spec = Some(spec.clone());
        workspace.project_root = root.to_string_lossy().into_owned();
        workspace.working_directory = root;
    }
    let spec = world
        .resource::<WorkspaceState>()
        .spec
        .clone()
        .expect("workspace attached");
    world.insert_resource(WorkspaceAttachment::Saved(spec));
    restore_contributions(world)?;
    let launchers = panel_entities(world)
        .into_iter()
        .filter(|entity| {
            world
                .get::<PanelRecord>(*entity)
                .is_some_and(|record| record.panel_type == "bed.workspaces.panel")
        })
        .collect::<Vec<_>>();
    retire_panels(world, &launchers)?;
    layouts::attach(world, terminal)?;
    publish_app_state(world);
    Ok(())
}

fn validate_workspace_snapshot(
    snapshot: &bed_plugin_workspaces::WorkspaceSnapshot,
) -> io::Result<()> {
    bed_storage::validate_snapshot(&snapshot.plugins)?;
    let layout = &snapshot.layouts;
    if layout.is_null() {
        return Ok(());
    }
    let active = layout
        .get("active_layout_slot")
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Missing active layout slot"))?;
    if LayoutSlot::from_value(active).is_none()
        || !layout["layouts"].as_object().is_some_and(|slots| {
            slots.iter().all(|(slot, value)| {
                LayoutSlot::from_key(slot).is_some()
                    && value["version"] == 2
                    && value["panels"].as_array().is_some_and(|panels| {
                        panels.iter().all(|panel| panel["panel_type"].is_string())
                    })
            })
        })
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Invalid saved workspace layouts",
        ));
    }
    Ok(())
}

pub fn replace_workspace(world: &mut World, mut spec: WorkspaceSpec) -> io::Result<()> {
    if !cfg!(feature = "workspaces") {
        return Err(io::Error::other("Workspace support is disabled"));
    }
    if matches!(spec.target, WorkspaceTarget::Ssh { .. }) {
        start_connection(world, spec, false, None)?;
        return Ok(());
    }
    let root = std::fs::canonicalize(&spec.root)?;
    spec.root = root
        .to_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Workspace path is not UTF-8"))?
        .to_owned();
    let Some(stored) = world
        .resource::<WorkspaceRepositoryResource>()
        .0
        .find(&spec)?
    else {
        return replace_directory(world, root);
    };
    spec = stored;
    let mut options = session_options(world, Some(root.clone()));
    options.history_path = world
        .non_send::<WorkspacePersistence>()
        .store
        .history_path(&spec);
    let context = bed_files::FileContext::local(&root)?;
    let services = bed_files::FileServices::local();
    let session = DocumentSession::with_file_services(options, &context, &services)?;
    if world.resource::<AppInitialized>().0 {
        persist_workspace(world)?;
    }
    let snapshot = world
        .resource::<WorkspaceRepositoryResource>()
        .0
        .load(&spec)?
        .unwrap_or_else(|| bed_plugin_workspaces::WorkspaceSnapshot {
            layouts: Value::Null,
            plugins: json!({}),
        });
    validate_workspace_snapshot(&snapshot)?;
    let spec = world
        .resource_mut::<WorkspaceRepositoryResource>()
        .0
        .commit(spec, snapshot)?;
    refresh_workspace_persistence(world);
    if let Some(token) = world.resource::<PendingOperations>().barrier {
        bed_core::validate_document_transition(world, token).map_err(io::Error::other)?;
    }
    let panels = panel_entities(world);
    retire_panels(world, &panels)?;
    install_context_session(world, session)?;
    world.insert_resource(ActiveSessionId(
        world.non_send::<DocumentSession>().session_id().0,
    ));
    *world.resource_mut::<DocumentNavigation>() = DocumentNavigation::default();
    *world.resource_mut::<WorkbenchLayout>() = WorkbenchLayout::default();
    layouts::clear(world);
    {
        let mut workspace = world.resource_mut::<WorkspaceState>();
        workspace.project_root = spec.root.clone();
        workspace.working_directory = root;
        workspace.spec = Some(spec.clone());
        world.insert_resource(WorkspaceAttachment::Saved(spec));
    }
    cancel_foreign_pending(world);
    restore_workspace(world)
}

fn cancel_foreign_pending(world: &mut World) {
    bed_core::cancel_explicit_document_saves(world);
    let pending = std::mem::take(&mut world.resource_mut::<PendingOperations>().saves);
    for save in pending {
        let error = "The workspace changed before this operation completed".to_owned();
        complete(world, save.id, Err(error.into()));
    }
}

pub fn application_commands(world: &World) -> Vec<crate::application::commands::CommandItem> {
    let editor = world.resource::<bed_api::features::FeatureRegistry>();
    let registry = world.resource::<bed_api::workbench::WorkbenchRegistry>();
    let facts = world.get_resource::<bed_api::workbench::CommandPresentations>();
    let mut seen = HashSet::new();
    editor
        .menus()
        .iter()
        .filter(|menu| menu.slot == bed_api::features::MenuSlot::Application)
        .map(|menu| menu.command)
        .chain(registry.menus().iter().copied())
        .filter(|id| seen.insert(*id))
        .filter_map(|id| registry.command(id))
        .map(|command| command.presentation(facts))
        .filter(|command| command.visible)
        .map(|command| crate::application::commands::CommandItem {
            id: command.id.into(),
            label: command.label.into(),
            icon: command.icon.map(str::to_owned),
            enabled: command.enabled,
        })
        .collect()
}

pub fn toolbar_commands(world: &World) -> Vec<crate::application::commands::CommandItem> {
    let editor = world.resource::<bed_api::features::FeatureRegistry>();
    let registry = world.resource::<bed_api::workbench::WorkbenchRegistry>();
    let facts = world.get_resource::<bed_api::workbench::CommandPresentations>();
    let mut seen = HashSet::new();
    editor
        .toolbar()
        .iter()
        .map(|item| item.command)
        .chain(registry.toolbar().iter().copied())
        .filter(|id| seen.insert(*id))
        .filter_map(|id| registry.command(id))
        .map(|command| command.presentation(facts))
        .filter(|command| command.visible)
        .map(|command| crate::application::commands::CommandItem {
            id: command.id.into(),
            label: command.label.into(),
            icon: command.icon.map(str::to_owned),
            enabled: command.enabled,
        })
        .collect()
}

struct ConnectedWorkspace {
    spec: WorkspaceSpec,
    target: bed_remote::SshTarget,
    client: bed_remote::RemoteClient,
}

struct Connection {
    session_id: u64,
    id: Option<AppRequestId>,
    reconnect: bool,
    receiver: std::sync::mpsc::Receiver<io::Result<ConnectedWorkspace>>,
}

struct RemoteOpen {
    session_id: u64,
    id: Option<AppRequestId>,
    path: PathBuf,
    viewer: Option<String>,
    additional: bool,
    mode: FileOpenMode,
    layout_slot: LayoutSlot,
    move_existing: bool,
    area: Option<u32>,
    tab_index: Option<usize>,
    restore: Option<Entity>,
    follow_up: Option<AppRequest>,
}

#[derive(Default)]
struct RemoteOperations {
    connection: Option<Connection>,
    opens: Vec<RemoteOpen>,
    abandoned_paths: HashSet<PathBuf>,
}

fn start_connection(
    world: &mut World,
    spec: WorkspaceSpec,
    reconnect: bool,
    id: Option<AppRequestId>,
) -> io::Result<()> {
    let WorkspaceTarget::Ssh { host } = &spec.target else {
        return Err(io::Error::other("Workspace is not remote"));
    };
    if host.trim().is_empty()
        || !(spec.root.starts_with('/') || spec.root == "~" || spec.root.starts_with("~/"))
        || spec.root.contains('\0')
    {
        return Err(io::Error::other(
            "SSH workspace requires a host and absolute project path",
        ));
    }
    let target = bed_remote::SshTarget::new(host.clone());
    let helpers = world.resource::<RemoteHelperDirectory>().0.clone();
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("bed-ssh-connect".into())
        .spawn(move || {
            let result = (|| {
                let root = bed_remote::expand_ssh_path(&target, &spec.root)?;
                let target = bed_remote::prepare_ssh_target(&target, &helpers)?;
                let client = bed_remote::RemoteClient::launch_ssh(&target)?;
                let bed_remote::Response::Path { path } =
                    client.call(bed_remote::Request::Canonicalize {
                        root,
                        path: ".".into(),
                        allow_missing: false,
                    })?
                else {
                    return Err(io::Error::other("Unexpected remote validation response"));
                };
                if !matches!(
                    client.call(bed_remote::Request::ReadDirectory {
                        root: path.clone(),
                        path: path.clone(),
                        classify_gitignored: false
                    })?,
                    bed_remote::Response::Directory { .. }
                ) {
                    return Err(io::Error::other("Remote workspace is not a directory"));
                }
                let mut spec = spec;
                spec.root = path;
                Ok(ConnectedWorkspace {
                    spec,
                    target,
                    client,
                })
            })();
            let _ = sender.send(result);
        })?;
    let old = world.non_send_mut::<RemoteOperations>().connection.take();
    if let Some(old) = old {
        complete(
            world,
            old.id,
            Err("Superseded by a newer connection request".into()),
        );
    }
    world.non_send_mut::<RemoteOperations>().connection = Some(Connection {
        session_id: world.non_send::<DocumentSession>().session_id().0,
        id,
        reconnect,
        receiver,
    });
    Ok(())
}

fn reconnect_workspace(world: &mut World, id: Option<AppRequestId>) -> io::Result<()> {
    let context = world.resource::<bed_files::FileContext>();
    let bed_files::FileProviderId::Ssh { host } = &context.provider else {
        return Err(io::Error::other("No remote directory is open"));
    };
    let spec = world
        .resource::<WorkspaceState>()
        .spec
        .clone()
        .unwrap_or_else(|| WorkspaceSpec {
            name: bed_plugin_workspaces::default_name(&context.directory_string()),
            target: WorkspaceTarget::Ssh { host: host.clone() },
            root: context.directory_string(),
        });
    start_connection(world, spec, true, id)
}

fn replace_connected_workspace(world: &mut World, connected: ConnectedWorkspace) -> io::Result<()> {
    let root = connected.spec.root.clone();
    let WorkspaceTarget::Ssh { host } = &connected.spec.target else {
        return Err(io::Error::other("Remote context has no host"));
    };
    let host = host.clone();
    let mut saved = if cfg!(feature = "workspaces") {
        world
            .resource::<WorkspaceRepositoryResource>()
            .0
            .find(&connected.spec)?
    } else {
        None
    };
    let mut options = session_options(world, Some(PathBuf::from(&root)));
    options.history_path = None;
    let context = bed_files::FileContext::remote(host.clone(), PathBuf::from(&root))?;
    let services = bed_files::FileServices::remote_target(connected.target, connected.client);
    let session = DocumentSession::with_file_services(options, &context, &services)?;
    if world.resource::<AppInitialized>().0 {
        persist_workspace(world)?;
    }
    if let Some(spec) = saved.take() {
        let snapshot = world
            .resource::<WorkspaceRepositoryResource>()
            .0
            .load(&spec)?
            .unwrap_or_else(|| bed_plugin_workspaces::WorkspaceSnapshot {
                layouts: Value::Null,
                plugins: json!({}),
            });
        validate_workspace_snapshot(&snapshot)?;
        saved = Some(
            world
                .resource_mut::<WorkspaceRepositoryResource>()
                .0
                .commit(spec, snapshot)?,
        );
        refresh_workspace_persistence(world);
    }
    if let Some(token) = world.resource::<PendingOperations>().barrier {
        bed_core::validate_document_transition(world, token).map_err(io::Error::other)?;
    }
    let panels = panel_entities(world);
    retire_panels(world, &panels)?;
    install_context_session(world, session)?;
    let context = bed_files::FileContext::remote(host, PathBuf::from(&root))?;
    world
        .resource_mut::<bed_files::FileContext>()
        .replace(context);
    world.insert_resource(services);
    {
        let mut workspace = world.resource_mut::<WorkspaceState>();
        workspace.project_root = root.clone();
        workspace.working_directory = PathBuf::from(&root);
        workspace.spec = saved.clone();
    }
    world.insert_resource(saved.map_or(WorkspaceAttachment::Temporary, WorkspaceAttachment::Saved));
    world.insert_resource(ActiveSessionId(
        world.non_send::<DocumentSession>().session_id().0,
    ));
    *world.resource_mut::<DocumentNavigation>() = DocumentNavigation::default();
    *world.resource_mut::<WorkbenchLayout>() = WorkbenchLayout::default();
    layouts::clear(world);
    cancel_foreign_pending(world);
    if world.resource::<WorkspaceState>().spec.is_some() {
        restore_workspace(world)
    } else {
        bed_storage::restore_snapshot(world, None)?;
        layouts::initialize(world, LayoutSlot::Layout1)?;
        publish_app_state(world);
        Ok(())
    }
}

fn place_opened_panel(
    world: &mut World,
    panel: Entity,
    area: Option<u32>,
    move_existing: bool,
    tab_index: Option<usize>,
) -> io::Result<()> {
    if (move_existing || tab_index.is_some())
        && let Some(area) = area
    {
        let id = world
            .get::<PanelPresentation>(panel)
            .expect("opened content belongs to a panel")
            .id;
        world
            .resource_mut::<WorkbenchLayout>()
            .tiling
            .assign(id, area);
        if let Some(index) = tab_index {
            world
                .resource_mut::<WorkbenchLayout>()
                .tiling
                .move_tab(id, area, index);
        }
        world.get_mut::<PanelPresentation>(panel).unwrap().dock_next = true;
        focus_panel(world, panel, true)?;
    }
    Ok(())
}

// Captured document input and destination stay together through asynchronous opening.
#[allow(clippy::too_many_arguments)]
fn begin_open_file(
    world: &mut World,
    path: &Path,
    viewer: Option<&str>,
    additional: bool,
    move_existing: bool,
    area: Option<u32>,
    tab_index: Option<usize>,
    id: Option<AppRequestId>,
    mode: FileOpenMode,
) -> io::Result<Option<AppRequestOutcome>> {
    match open_file_with_origin(world, path, viewer, additional, area, None, mode) {
        Ok(panel) => {
            place_opened_panel(world, panel, area, move_existing, tab_index)?;
            Ok(Some(AppRequestOutcome::Opened {
                panel,
                document: world
                    .get::<PanelBinding>(panel)
                    .and_then(|binding| binding.document),
            }))
        }
        Err(error)
            if error.kind() == io::ErrorKind::WouldBlock
                && world.non_send::<DocumentSession>().is_remote() =>
        {
            let session = world.non_send::<DocumentSession>().session_id().0;
            let layout_slot = world.resource::<LayoutSlots>().active();
            world
                .non_send_mut::<RemoteOperations>()
                .opens
                .push(RemoteOpen {
                    session_id: session,
                    id,
                    path: path.to_path_buf(),
                    viewer: viewer.map(str::to_owned),
                    additional,
                    mode,
                    layout_slot,
                    move_existing,
                    area,
                    tab_index,
                    restore: None,
                    follow_up: None,
                });
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

fn queue_remote_restore(world: &mut World, record: Value, area: Option<u32>) -> io::Result<Entity> {
    let panel = bed_workbench::spawn_panel(world, record.clone(), area, record["id"].as_u64())
        .map_err(io::Error::other)?;
    let session = world.non_send::<DocumentSession>().session_id().0;
    let viewer = record["viewer"].as_str().map(str::to_owned);
    world.entity_mut(panel).insert((PanelBinding {
        session_id: session,
        viewer: viewer.clone(),
        ..Default::default()
    },));
    world.get_mut::<PanelRecord>(panel).unwrap().unavailable =
        Some("Opening remote document…".into());
    let layout_slot = world.resource::<LayoutSlots>().active();
    world
        .non_send_mut::<RemoteOperations>()
        .opens
        .push(RemoteOpen {
            session_id: session,
            id: None,
            path: PathBuf::from(record["path"].as_str().unwrap()),
            viewer,
            additional: true,
            mode: FileOpenMode::Keep,
            layout_slot,
            move_existing: false,
            area,
            tab_index: None,
            restore: Some(panel),
            follow_up: None,
        });
    Ok(panel)
}

fn poll_remote_operations(
    world: &mut World,
    errors: &[bed_documents::ServiceError],
) -> io::Result<()> {
    if let Some(connection) = world.non_send_mut::<RemoteOperations>().connection.take() {
        let result = connection.receiver.try_recv();
        match result {
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                world.non_send_mut::<RemoteOperations>().connection = Some(connection)
            }
            result => {
                let result = result
                    .map_err(|_| io::Error::other("SSH connection worker stopped"))
                    .and_then(|result| result);
                let result = (|| {
                    if connection.session_id != world.non_send::<DocumentSession>().session_id().0 {
                        return Ok(Some(AppRequestOutcome::Ignored));
                    }
                    let connected = result?;
                    if connection.reconnect {
                        world
                            .non_send_mut::<DocumentSession>()
                            .reconnect_remote_with_target(connected.client, connected.target)?;
                        return Ok(Some(AppRequestOutcome::Applied));
                    }
                    let panels = panel_entities(world);
                    begin_close_transition(
                        world,
                        panels,
                        Continuation::Connected(Box::new(connected)),
                        connection.id,
                    )
                })();
                match result {
                    Ok(Some(outcome)) => {
                        release_transition_barrier(world, Ok(()))?;
                        complete(world, connection.id, Ok(outcome));
                    }
                    Ok(None) => {}
                    Err(error) => {
                        release_transition_barrier(world, Err(error.to_string()))?;
                        complete(world, connection.id, Err(error.into()));
                    }
                }
            }
        }
    }
    let opens = std::mem::take(&mut world.non_send_mut::<RemoteOperations>().opens);
    for mut pending in opens {
        if pending.session_id != world.non_send::<DocumentSession>().session_id().0 {
            complete(
                world,
                pending.id,
                Err("Workspace changed while opening a document".into()),
            );
            continue;
        }
        if pending.mode == FileOpenMode::Preview
            && pending.layout_slot != world.resource::<LayoutSlots>().active()
        {
            world
                .non_send_mut::<RemoteOperations>()
                .abandoned_paths
                .insert(pending.path.clone());
            complete(world, pending.id, Ok(AppRequestOutcome::Ignored));
            continue;
        }
        if pending.area.is_some_and(|area| {
            !world
                .resource::<WorkbenchLayout>()
                .tiling
                .areas
                .iter()
                .any(|group| group.area == area)
        }) {
            world
                .non_send_mut::<RemoteOperations>()
                .abandoned_paths
                .insert(pending.path.clone());
            complete(
                world,
                pending.id,
                Err(
                    io::Error::new(io::ErrorKind::NotFound, "This area is no longer available")
                        .into(),
                ),
            );
            continue;
        }
        if pending.follow_up.as_ref().is_some_and(|request|matches!(&request.action,AppAction::HostAction(HostAction::NavigateLocation{origin:Some(origin),..}) if !world.non_send::<DocumentSession>().accepts_lsp_origin(origin))) {world.non_send_mut::<RemoteOperations>().abandoned_paths.insert(pending.path.clone());complete(world,pending.id,Ok(AppRequestOutcome::Ignored));continue;}
        if let Some(error) = errors
            .iter()
            .find(|error| error.service == "remote" && error.document.is_none())
        {
            complete(world, pending.id, Err(error.message.clone().into()));
            continue;
        }
        if let Some(follow_up) = pending.follow_up.take() {
            // Resume navigation before opening its destination: replacing a
            // preview can retire the source that validates the request.
            match apply_request(world, follow_up, pending.id) {
                Ok(Some(outcome)) => complete(world, pending.id, Ok(outcome)),
                Ok(None) => {}
                Err(error) => complete(world, pending.id, Err(error.into())),
            }
            continue;
        }
        let result = if let Some(panel) = pending.restore {
            if !world.entities().contains(panel) {
                world
                    .non_send_mut::<RemoteOperations>()
                    .abandoned_paths
                    .insert(pending.path.clone());
                continue;
            }
            let record = world.get::<PanelRecord>(panel).unwrap().clone();
            let kind = if record.serialized["document_kind"] == "bytes" {
                bed_documents::DocumentKind::Bytes
            } else {
                pending
                    .viewer
                    .as_deref()
                    .and_then(|viewer| {
                        world
                            .resource::<bed_api::features::FeatureRegistry>()
                            .viewer(viewer)
                    })
                    .and_then(|viewer| viewer.document_kind())
                    .unwrap_or(bed_documents::DocumentKind::Text)
            };
            let document = world
                .non_send_mut::<DocumentSession>()
                .open_file_with_kind(&pending.path, kind);
            document.and_then(|document| {
                world.get_mut::<PanelRecord>(panel).unwrap().unavailable = None;
                let binding = PanelBinding {
                    input: PanelInput::Document(document),
                    document: Some(document),
                    viewer: pending.viewer.clone(),
                    session_id: pending.session_id,
                    ..Default::default()
                };
                mount_presentation(
                    world,
                    panel,
                    &record.panel_type,
                    binding,
                    record
                        .serialized
                        .get("state")
                        .cloned()
                        .unwrap_or(record.serialized),
                )?;
                world.get_mut::<PanelRecord>(panel).unwrap().unavailable = world
                    .get::<PanelRecord>(panel_presentation(world, panel).unwrap())
                    .unwrap()
                    .unavailable
                    .clone();
                let focused = world.resource::<WorkbenchLayout>().focused;
                if let Some(focused) = focused
                    && (focused == panel
                        || world.resource::<DocumentNavigation>().last_document_panel
                            == Some(panel))
                {
                    focus_panel(world, focused, focused == panel)?;
                }
                Ok(panel)
            })
        } else {
            open_file_with_origin(
                world,
                &pending.path,
                pending.viewer.as_deref(),
                pending.additional,
                pending.area,
                None,
                pending.mode,
            )
        };
        match result {
            Ok(panel) => {
                place_opened_panel(
                    world,
                    panel,
                    pending.area,
                    pending.move_existing,
                    pending.tab_index,
                )?;
                complete(
                    world,
                    pending.id,
                    Ok(AppRequestOutcome::Opened {
                        panel,
                        document: world
                            .get::<PanelBinding>(panel)
                            .and_then(|binding| binding.document),
                    }),
                )
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                world.non_send_mut::<RemoteOperations>().opens.push(pending)
            }
            Err(error) => {
                if let Some(panel) = pending.restore
                    && let Some(mut record) = world.get_mut::<PanelRecord>(panel)
                {
                    record.unavailable = Some(error.to_string());
                }
                complete(world, pending.id, Err(error.into()));
            }
        }
    }
    let abandoned = std::mem::take(&mut world.non_send_mut::<RemoteOperations>().abandoned_paths);
    for path in abandoned {
        if world.non_send::<DocumentSession>().open_pending(&path) {
            world
                .non_send_mut::<RemoteOperations>()
                .abandoned_paths
                .insert(path);
            continue;
        }
        if world
            .non_send::<RemoteOperations>()
            .opens
            .iter()
            .any(|open| open.path == path)
        {
            continue;
        }
        if let Some(document) = world.non_send::<DocumentSession>().document_for_path(&path)
            && world.non_send::<DocumentSession>().view_count(document) == 0
            && world
                .non_send::<DocumentSession>()
                .snapshot(document)
                .is_ok_and(|snapshot| !snapshot.dirty)
            && !world
                .query::<&PanelBinding>()
                .iter(world)
                .any(|binding| binding.document == Some(document))
        {
            bed_core::retire_document(world, document).map_err(io::Error::other)?;
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn test_application_paths(config: &Path) -> bed_api::launch::ApplicationPaths {
    bed_api::launch::ApplicationPaths::new("bed", Some(config.to_owned()), None).unwrap()
}

#[cfg(test)]
fn install_test_infrastructure(app: &mut App, config: &Path) {
    let settings = Settings::with_paths(
        config.to_owned(),
        Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf(),
    )
    .unwrap();
    let paths = test_application_paths(config);
    app.insert_resource(paths.clone())
        .insert_non_send(bed_ui::icons::Icons::default())
        .insert_non_send(settings)
        .insert_resource(RemoteHelperDirectory(PathBuf::new()));
    WorkspacePersistence::install(app, WorkspaceStore::load(&paths).unwrap());
}

#[cfg(test)]
#[path = "terminal_tests.rs"]
mod terminal_tests;
#[cfg(test)]
#[path = "runtime_tests.rs"]
mod tests;
#[cfg(test)]
#[path = "transaction_tests.rs"]
mod transaction_tests;
