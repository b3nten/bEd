//! Canonical editor resources and one ordered application transition boundary.
#[path = "lsp_format_save.rs"]
mod format_save;
mod layouts;
pub use layouts::LayoutSlots;

use crate::workspace::store::{WorkspaceSpec, WorkspaceStore, WorkspaceTarget};
pub use bed_config::LayoutSlot;
use bed_config::Settings;
use bed_document_session::editor_session::{
    ByteEdit, ClosePolicy, DocumentId, EditorSession, SessionOptions, ViewId,
};
use bed_editor_api::{
    CommandContext, EditorFeatureReply, EditorFrame, EditorIntents, EditorPanelActions,
    EditorPanelBinding, EditorPanelClose, EditorPanelFocus, EditorWorkspaceContribution,
    EditorWorkspaceRestore, HostRequest, LspRequestOrigin, PanelAction, PanelActionInvocation,
    PanelInput, PanelTarget, ProjectRequest, Revision, SaveToken, SavedDocument,
};
use bed_workbench::{
    PanelRecord, WorkbenchLayout, WorkbenchOutcome, WorkbenchRequestId, WorkbenchRequests,
};
use bed_workbench_api::{Action, CommandInvocation, PanelCreation, PanelOwner, PanelPresentation};
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
    NewProjects,
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
    Projects,
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
    FindProject,
}

#[derive(Clone, Debug)]
pub struct OpenFileOptions {
    pub path: PathBuf,
    pub viewer: Option<String>,
    pub additional: bool,
}

#[derive(Clone, Debug, Default)]
pub struct EditorWorkbenchOptions {
    pub working_directory: Option<PathBuf>,
    pub workspace: Option<WorkspaceSpec>,
    pub files: Vec<OpenFileOptions>,
    /// Restore the last standalone session only when explicitly requested.
    pub restore_standalone: bool,
    /// Initial arrangement for a fresh standalone host.
    pub initial_layout: Option<LayoutSlot>,
    /// Initial tool, opened before file arguments so those files retain focus.
    pub startup_panel: Option<String>,
    pub host_mode: WorkbenchHostMode,
}

#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq)]
pub struct EditorSessionId(pub u64);

#[derive(Resource, Clone, Debug)]
pub struct EditorWorkspace {
    pub spec: Option<WorkspaceSpec>,
    pub project_root: String,
    pub working_directory: PathBuf,
    pub error: Option<String>,
    pub close_requested: bool,
    pub windows: Vec<WorkspaceSpec>,
}

#[derive(Resource, Clone, Default, Debug)]
pub struct EditorNavigation {
    pub active_view: Option<ViewId>,
    pub active_document: Option<DocumentId>,
    pub last_document_panel: Option<Entity>,
}

pub struct WorkspacePersistence {
    store: WorkspaceStore,
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
}
/// The application locates packaged helpers before installing editor integration.
#[derive(Resource, Clone)]
pub struct RemoteHelperDirectory(pub PathBuf);

/// Layout panels own retained presentation entities, never feature widgets.
#[derive(Component, Default)]
pub struct EditorPresentations {
    pub retained: HashMap<String, Entity>,
}

pub fn panel_presentation(world: &World, panel: Entity) -> Option<Entity> {
    if let Some(frontend) = world.get::<bed_workbench_api::PanelFrontend>(panel) {
        Some(frontend.0)
    } else if bed_workbench_api::panel_owner(world, panel).is_some() {
        Some(panel)
    } else {
        None
    }
}

#[derive(Debug)]
pub enum EditorAction {
    Command(WindowCommand),
    PluginCommand(String),
    WorkbenchAction(Action),
    HostRequest(HostRequest),
    HostRequests {
        requests: Vec<HostRequest>,
    },
    PanelAction {
        panel: Entity,
        action: PanelAction,
    },
    ExternalFiles {
        panel: Option<Entity>,
        event: bed_editor_api::ExternalFileDrag,
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
    SwitchViewer {
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
pub struct EditorRequest {
    pub action: EditorAction,
    pub session: u64,
    pub layout_slot: LayoutSlot,
    pub source: Option<Entity>,
    pub panel: Option<Entity>,
    pub area: u32,
    pub tab_index: Option<usize>,
    pub context: CommandContext,
    pub arguments: Value,
}

/// Capture is a read operation; requests never borrow or change later focus.
pub fn capture_editor_request(
    world: &World,
    action: EditorAction,
    target: PanelTarget,
) -> EditorRequest {
    capture_editor_request_from(
        world,
        action,
        target,
        world.resource::<WorkbenchLayout>().focused,
    )
}

pub fn capture_editor_request_from(
    world: &World,
    action: EditorAction,
    target: PanelTarget,
    source: Option<Entity>,
) -> EditorRequest {
    let target = explicit_panel_target(&action).unwrap_or(target);
    let panel = source.and_then(|source| bed_workbench_api::panel_owner(world, source));
    let presentation = panel.and_then(|panel| panel_presentation(world, panel));
    let binding = source
        .and_then(|entity| world.get::<EditorPanelBinding>(entity))
        .or_else(|| presentation.and_then(|entity| world.get::<EditorPanelBinding>(entity)))
        .or_else(|| panel.and_then(|entity| world.get::<EditorPanelBinding>(entity)));
    let session = world.non_send::<EditorSession>();
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
        && let Ok(captured) = bed_editor_api::selection_context(session, view)
    {
        context = captured;
    }
    if context.document.is_none()
        && matches!(
            action,
            EditorAction::Command(WindowCommand::Save | WindowCommand::SaveAs)
        )
        && binding.is_none_or(|binding| binding.input.local_file().is_none())
    {
        let navigation = world.resource::<EditorNavigation>();
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
    EditorRequest {
        action,
        session: session.workspace_id().0,
        layout_slot: world.resource::<LayoutSlots>().active(),
        source,
        panel,
        area,
        tab_index: None,
        context,
        arguments: Value::Null,
    }
}

fn explicit_panel_target(action: &EditorAction) -> Option<PanelTarget> {
    match action {
        EditorAction::HostRequest(
            HostRequest::ShowPanelIn { target, .. }
            | HostRequest::OpenFileIn { target, .. }
            | HostRequest::PreviewFileIn { target, .. },
        ) => Some(*target),
        EditorAction::HostRequest(HostRequest::NavigateLocation { .. }) => {
            Some(PanelTarget::LastFocused)
        }
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EditorRequestId(pub u64);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditorRequestError {
    pub kind: io::ErrorKind,
    pub message: String,
}
impl std::fmt::Display for EditorRequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for EditorRequestError {}
impl From<io::Error> for EditorRequestError {
    fn from(error: io::Error) -> Self {
        Self {
            kind: error.kind(),
            message: error.to_string(),
        }
    }
}
impl From<String> for EditorRequestError {
    fn from(message: String) -> Self {
        Self {
            kind: io::ErrorKind::Other,
            message,
        }
    }
}
impl From<&str> for EditorRequestError {
    fn from(message: &str) -> Self {
        message.to_owned().into()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditorRequestOutcome {
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
    id: Option<EditorRequestId>,
    request: EditorRequest,
}
#[derive(Resource, Default)]
pub struct EditorRequests {
    next_id: u64,
    pending: VecDeque<QueuedRequest>,
    results: HashMap<EditorRequestId, Result<EditorRequestOutcome, EditorRequestError>>,
    generic: HashMap<EditorRequestId, WorkbenchRequestId>,
}
impl EditorRequests {
    pub fn enqueue(&mut self, request: EditorRequest) {
        self.pending.push_back(QueuedRequest {
            sequence: bed_workbench::next_request_sequence(),
            id: None,
            request,
        });
    }
    pub fn submit(&mut self, request: EditorRequest) -> EditorRequestId {
        self.next_id = self
            .next_id
            .checked_add(1)
            .expect("editor request ID overflow");
        let id = EditorRequestId(self.next_id);
        self.pending.push_back(QueuedRequest {
            sequence: bed_workbench::next_request_sequence(),
            id: Some(id),
            request,
        });
        id
    }
    pub fn take_result(
        &mut self,
        id: EditorRequestId,
    ) -> Option<Result<EditorRequestOutcome, String>> {
        self.results
            .remove(&id)
            .map(|result| result.map_err(|error| error.message))
    }
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}
pub fn submit_editor_request(world: &mut World, request: EditorRequest) -> EditorRequestId {
    world.resource_mut::<EditorRequests>().submit(request)
}
pub fn poll_editor_request_result(
    world: &mut World,
    id: EditorRequestId,
) -> Option<Result<EditorRequestOutcome, EditorRequestError>> {
    world.resource_mut::<EditorRequests>().results.remove(&id)
}
pub fn poll_editor_request(
    world: &mut World,
    id: EditorRequestId,
) -> Option<Result<EditorRequestOutcome, String>> {
    world.resource_mut::<EditorRequests>().take_result(id)
}

#[derive(ScheduleLabel, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EditorRequestsSchedule;
#[derive(ScheduleLabel, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EditorServicesSchedule;
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EditorSystemSet {
    Services,
    Requests,
    Frame,
}

#[derive(Default)]
pub struct EditorWorkbenchPlugin {
    pub options: EditorWorkbenchOptions,
}
impl Plugin for EditorWorkbenchPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<bed_workbench::WorkbenchPlugin>() {
            app.add_plugins(bed_workbench::WorkbenchPlugin);
        }
        app.insert_resource(bed_workbench::WorkbenchRequestMode::External)
            .init_resource::<bed_editor_api::Registry>()
            .init_resource::<EditorFrame>()
            .init_resource::<EditorIntents>()
            .init_resource::<EditorRequests>()
            .init_resource::<EditorNavigation>()
            .init_resource::<LayoutSlots>()
            .init_resource::<PendingOperations>()
            .init_resource::<TerminalCompanions>()
            .init_resource::<EditorInitialized>()
            .add_message::<EditorFeatureReply>()
            .add_message::<bed_editor_api::EditorDocumentEvent>()
            .init_schedule(EditorRequestsSchedule)
            .init_schedule(EditorServicesSchedule)
            .add_systems(
                EditorRequestsSchedule,
                apply_editor_requests.in_set(EditorSystemSet::Requests),
            )
            .add_systems(
                EditorServicesSchedule,
                update_editor_services.in_set(EditorSystemSet::Services),
            )
            .add_systems(
                PreUpdate,
                update_editor_services
                    .in_set(EditorSystemSet::Services)
                    .before(EditorSystemSet::Frame),
            )
            .add_systems(
                PreUpdate,
                publish_editor_frame
                    .in_set(EditorSystemSet::Frame)
                    .before(bed_editor_api::EditorFeatureSystems::Prepare),
            )
            .add_systems(
                PostUpdate,
                apply_editor_requests.in_set(EditorSystemSet::Requests),
            );
        let world = app.world_mut();
        crate::shortcuts::register(world);
        if !world.contains_non_send::<EditorSession>() {
            world.insert_non_send(EditorSession::new());
        }
        assert!(
            world.contains_non_send::<Settings>(),
            "application settings are installed before editor integration"
        );
        assert!(
            world.contains_non_send::<bed_ui::icons::Icons>(),
            "application icons are installed before editor integration"
        );
        assert!(
            world.contains_non_send::<WorkspacePersistence>(),
            "loaded workspace persistence is installed before editor integration"
        );
        assert!(
            world.contains_resource::<RemoteHelperDirectory>(),
            "application remote helper paths are installed before editor integration"
        );
        world.init_non_send::<RemoteOperations>();
        let working_directory = self
            .options
            .working_directory
            .clone()
            .expect("application startup supplies the working directory");
        world.insert_resource(EditorSessionId(
            world.non_send::<EditorSession>().workspace_id().0,
        ));
        world.insert_resource(EditorWorkspace {
            spec: None,
            project_root: String::new(),
            working_directory,
            error: None,
            close_requested: false,
            windows: Vec::new(),
        });
        world.insert_non_send(crate::editor_ui::EditorUi::new(self.options.host_mode));
        world.insert_resource(InitialEditorOptions(Some(self.options.clone())));
        app.add_systems(Startup, initialize_editor);
    }
}

#[derive(Resource, Default)]
struct EditorInitialized(bool);
#[derive(Resource)]
struct InitialEditorOptions(Option<EditorWorkbenchOptions>);

fn initialize_editor(world: &mut World) {
    let Some(options) = world.resource_mut::<InitialEditorOptions>().0.take() else {
        return;
    };
    let result = (|| -> io::Result<()> {
        layouts::migrate_default(world)?;
        // Initial factories need the configured working directory before the
        // first PreUpdate frame publisher has run.
        publish_editor_frame(world);
        if let Some(spec) = options.workspace {
            replace_workspace(world, spec)?;
        } else if options.restore_standalone {
            restore_workspace(world)?;
        } else if let Some(slot) = options.initial_layout {
            layouts::initialize(world, slot)?;
        }
        if let Some(startup) = options.startup_panel.as_deref()
            && panel_entities(world).is_empty()
            && world
                .resource::<bed_workbench_api::Registry>()
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
        publish_editor_frame(world);
        Ok(())
    })();
    if let Err(error) = result {
        world.resource_mut::<EditorWorkspace>().error = Some(error.to_string());
    }
    world.resource_mut::<EditorInitialized>().0 = true;
}

pub fn process_editor_requests(world: &mut World) -> io::Result<()> {
    world.run_schedule(EditorRequestsSchedule);
    Ok(())
}
pub fn tick_editor_services(world: &mut World) -> io::Result<()> {
    world.run_schedule(EditorServicesSchedule);
    process_editor_requests(world)
}

pub fn dispatch_editor_command(
    world: &mut World,
    command: WindowCommand,
    target: PanelTarget,
) -> io::Result<bool> {
    let request = capture_editor_request(world, EditorAction::Command(command), target);
    dispatch_request(world, request)
}
pub fn dispatch_editor_plugin_command(
    world: &mut World,
    command: impl Into<String>,
    target: PanelTarget,
) -> io::Result<bool> {
    let request =
        capture_editor_request(world, EditorAction::PluginCommand(command.into()), target);
    dispatch_request(world, request)
}
fn dispatch_request(world: &mut World, request: EditorRequest) -> io::Result<bool> {
    let id = submit_editor_request(world, request);
    process_editor_requests(world)?;
    match poll_editor_request(world, id) {
        Some(Ok(EditorRequestOutcome::Ignored)) => Ok(false),
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
    Connected(Box<ConnectedWorkspace>),
    Layout {
        tiling: Value,
        panels: Vec<Entity>,
    },
}
struct PendingSave {
    id: Option<EditorRequestId>,
    session: u64,
    documents: Vec<SavedDocument>,
    paths: Vec<String>,
    reply: Option<(String, SaveToken)>,
    continuation: Option<Continuation>,
}
#[derive(Resource, Default)]
struct PendingOperations {
    saves: Vec<PendingSave>,
    formats: Vec<format_save::PendingFormatSave>,
    batches: Vec<PendingBatch>,
}

#[derive(Resource, Default)]
struct TerminalCompanions {
    session: u64,
    views: HashMap<(Entity, PathBuf), Entity>,
}

struct PendingBatch {
    parent: Option<EditorRequestId>,
    child: EditorRequestId,
    remaining: VecDeque<EditorRequest>,
}
fn internal_request_id(world: &mut World) -> EditorRequestId {
    let mut requests = world.resource_mut::<EditorRequests>();
    requests.next_id = requests
        .next_id
        .checked_add(1)
        .expect("editor request ID overflow");
    EditorRequestId(requests.next_id)
}
fn apply_batch(
    world: &mut World,
    mut requests: VecDeque<EditorRequest>,
    parent: Option<EditorRequestId>,
    mut outcome: EditorRequestOutcome,
) -> io::Result<Option<EditorRequestOutcome>> {
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
        match poll_editor_request_result(world, batch.child) {
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
    template: &EditorRequest,
    action: EditorAction,
    source: Option<Entity>,
    area: Option<u32>,
) -> EditorRequest {
    let explicit_target = explicit_panel_target(&action).is_some();
    let mut request = capture_editor_request_from(world, action, PanelTarget::LargestArea, source);
    request.session = template.session;
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
    id: Option<EditorRequestId>,
    result: Result<EditorRequestOutcome, EditorRequestError>,
) {
    if let Err(error) = &result {
        world.resource_mut::<EditorWorkspace>().error = Some(error.message.clone());
    }
    if let Some(id) = id {
        let generic = world.resource_mut::<EditorRequests>().generic.remove(&id);
        if let Some(generic) = generic {
            let result =
                result
                    .clone()
                    .map_err(|error| error.message)
                    .map(|outcome| match outcome {
                        EditorRequestOutcome::Ignored => WorkbenchOutcome::Ignored,
                        EditorRequestOutcome::Opened { panel, .. } => {
                            WorkbenchOutcome::Opened(panel)
                        }
                        EditorRequestOutcome::Closed { panels } if panels.len() == 1 => {
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
                .resource_mut::<EditorRequests>()
                .results
                .insert(id, result);
        }
    }
}

fn apply_editor_requests(world: &mut World) {
    assert!(
        !bed_workbench::frontend_active(world),
        "editor transitions run between frontend scopes"
    );
    // Both public queues are consumed in submission order by this owner.
    drain_feature_intents(world);
    loop {
        if layouts::waiting(world) {
            break;
        }
        let generic_sequence = world.resource::<WorkbenchRequests>().next_sequence();
        let editor_sequence = world
            .resource::<EditorRequests>()
            .pending
            .front()
            .map(|request| request.sequence);
        if generic_sequence
            .is_some_and(|sequence| editor_sequence.is_none_or(|editor| sequence < editor))
        {
            let queued = world
                .resource_mut::<WorkbenchRequests>()
                .take_next()
                .unwrap();
            let source = match &queued.action {
                Action::Invoke(invocation) => invocation.source,
                Action::ClosePanel(entity)
                | Action::KeepPanel(entity)
                | Action::FocusPanel(entity)
                | Action::ObserveFocus(entity)
                | Action::SplitPanel { panel: entity, .. } => Some(*entity),
                Action::OpenPanel { .. } | Action::ShowPanel { .. } => {
                    world.resource::<WorkbenchLayout>().focused
                }
                Action::FocusArea(_) | Action::SetLayout { .. } => None,
            };
            let request = capture_editor_request_from(
                world,
                EditorAction::WorkbenchAction(queued.action),
                PanelTarget::LargestArea,
                source,
            );
            let id = queued.id.map(|generic| {
                let mut requests = world.resource_mut::<EditorRequests>();
                requests.next_id = requests
                    .next_id
                    .checked_add(1)
                    .expect("editor request ID overflow");
                let id = EditorRequestId(requests.next_id);
                requests.generic.insert(id, generic);
                id
            });
            let result = apply_request(world, request, id);
            match result {
                Ok(Some(outcome)) => complete(world, id, Ok(outcome)),
                Ok(None) => {}
                Err(error) => complete(world, id, Err(error.into())),
            }
        } else if let Some(queued) = world.resource_mut::<EditorRequests>().pending.pop_front() {
            let result = apply_request(world, queued.request, queued.id);
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
    publish_editor_frame(world);
}

fn drain_feature_intents(world: &mut World) {
    let intents = world.resource_mut::<EditorIntents>().take();
    for intent in intents {
        if intent
            .source
            .is_some_and(|source| bed_workbench_api::panel_owner(world, source).is_none())
        {
            continue;
        }
        let mut request = capture_editor_request_from(
            world,
            EditorAction::HostRequest(intent.request),
            PanelTarget::LargestArea,
            intent.source,
        );
        if let Some(area) = intent.area {
            request.area = area;
        }
        request.tab_index = intent.tab_index;
        world.resource_mut::<EditorRequests>().enqueue(request);
    }
}

fn apply_request(
    world: &mut World,
    mut request: EditorRequest,
    id: Option<EditorRequestId>,
) -> io::Result<Option<EditorRequestOutcome>> {
    if request.session != world.non_send::<EditorSession>().workspace_id().0
        || request
            .source
            .is_some_and(|source| bed_workbench_api::panel_owner(world, source).is_none())
    {
        if id.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "The request's originating workspace or panel is no longer available",
            ));
        }
        return Ok(Some(EditorRequestOutcome::Ignored));
    }
    let layout_independent = matches!(
        &request.action,
        EditorAction::Command(
            WindowCommand::UseLayout(_)
                | WindowCommand::SetLayoutDefault(_)
                | WindowCommand::RestoreBuiltinLayout(_)
                | WindowCommand::Quit
        ) | EditorAction::ApplyEdits { .. }
            | EditorAction::SaveDocuments(_)
            | EditorAction::SaveAs { .. }
            | EditorAction::SetWorkspace(_)
            | EditorAction::AttachWorkspace(_)
    );
    let switch_command = matches!(&request.action,
        EditorAction::WorkbenchAction(Action::Invoke(invocation)) if invocation.command.starts_with("bed.app.layout_"));
    if !layout_independent
        && !switch_command
        && (request.layout_slot != world.resource::<LayoutSlots>().active()
            || request
                .source
                .is_some_and(|source| !bed_workbench_api::panel_active(world, source)))
    {
        return Ok(Some(EditorRequestOutcome::Ignored));
    }
    let action = std::mem::replace(
        &mut request.action,
        EditorAction::Command(WindowCommand::NewDocument),
    );
    let outcome = match action {
        EditorAction::PluginCommand(command) => {
            if world
                .get_resource::<bed_editor_api::CommandPresentations>()
                .and_then(|facts| facts.0.get(&command))
                .is_some_and(|facts| !facts.enabled || !facts.visible)
            {
                return Ok(Some(EditorRequestOutcome::Ignored));
            }
            let handler = world
                .resource::<bed_workbench_api::Registry>()
                .command(&command)
                .map(|definition| definition.handler)
                .ok_or_else(|| io::Error::other(format!("Command unavailable: {command}")))?;
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
            let previous = world.resource_mut::<EditorIntents>().take();
            let result = world
                .run_system_with(handler, invocation)
                .map_err(io::Error::other)
                .and_then(|result| result.map_err(io::Error::other));
            let created = world.resource_mut::<EditorIntents>().take();
            for intent in previous {
                world.resource_mut::<EditorIntents>().push_intent(intent);
            }
            let effects = result?;
            let mut requests = effects
                .into_iter()
                .map(|action| {
                    captured_effect(
                        world,
                        &request,
                        EditorAction::WorkbenchAction(action),
                        request.source,
                        None,
                    )
                })
                .collect::<VecDeque<_>>();
            requests.extend(created.into_iter().map(|intent| {
                captured_effect(
                    world,
                    &request,
                    EditorAction::HostRequest(intent.request),
                    intent.source,
                    intent.area,
                )
            }));
            return apply_batch(world, requests, id, EditorRequestOutcome::Applied);
        }
        EditorAction::WorkbenchAction(action) => match action {
            Action::ShowPanel {
                panel_type,
                state,
                area,
            } => {
                request.action = EditorAction::WorkbenchAction(
                    if let Some(panel) = bed_workbench::recent_panel(world, &panel_type) {
                        Action::FocusPanel(panel)
                    } else {
                        Action::OpenPanel {
                            panel_type,
                            state,
                            area,
                        }
                    },
                );
                return apply_request(world, request, id);
            }
            Action::Invoke(invocation) => {
                request.action = EditorAction::PluginCommand(invocation.command);
                request.source = invocation.source;
                request.area = invocation.area;
                request.arguments = invocation.arguments;
                return apply_request(world, request, id);
            }
            Action::OpenPanel {
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
                EditorRequestOutcome::Opened {
                    panel,
                    document: world
                        .get::<EditorPanelBinding>(panel)
                        .and_then(|binding| binding.document),
                }
            }
            Action::ClosePanel(panel) => return begin_close(world, vec![panel], false, id),
            Action::KeepPanel(panel) => {
                if bed_workbench::keep_panel(world, panel) {
                    EditorRequestOutcome::Applied
                } else {
                    EditorRequestOutcome::Ignored
                }
            }
            Action::ObserveFocus(panel) => {
                focus_panel(world, panel, false)?;
                EditorRequestOutcome::Applied
            }
            Action::FocusArea(area) => {
                bed_workbench::focus_area(world, area);
                EditorRequestOutcome::Applied
            }
            Action::SetLayout { tiling, close } => {
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
            Action::FocusPanel(panel) => {
                focus_panel(world, panel, true)?;
                EditorRequestOutcome::Applied
            }
            Action::SplitPanel { panel, axis, high } => {
                split_panel(world, panel, axis, high)?;
                EditorRequestOutcome::Applied
            }
        },
        EditorAction::PanelAction { panel, action } => {
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
                .get::<EditorPanelBinding>(entity)
                .and_then(|binding| binding.document)
            {
                commit_documents(world, &HashSet::from([document]))?;
            }
            let handled = dispatch_panel_action(world, entity, action)?;
            let intents = world.resource_mut::<EditorIntents>().take();
            let effects = intents
                .into_iter()
                .map(|intent| {
                    captured_effect(
                        world,
                        &request,
                        EditorAction::HostRequest(intent.request),
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
                    EditorRequestOutcome::Applied
                } else {
                    EditorRequestOutcome::Ignored
                },
            );
        }
        EditorAction::ExternalFiles { panel, event } => {
            let surface = panel.and_then(|panel| {
                if world.get::<PanelOwner>(panel).is_some() {
                    Some(panel)
                } else {
                    panel_presentation(world, panel)
                }
            });
            let mut effects = VecDeque::new();
            if let Some(entity) = surface
                && let Some(handler) = world
                    .get::<bed_editor_api::EditorPanelExternalFiles>(entity)
                    .copied()
            {
                let accepted = world
                    .run_system_with(
                        handler.0,
                        bed_editor_api::PanelExternalFilesInvocation {
                            entity,
                            event: event.clone(),
                        },
                    )
                    .map_err(io::Error::other)?
                    .map_err(io::Error::other)?;
                let intents = world.resource_mut::<EditorIntents>().take();
                effects.extend(intents.into_iter().map(|intent| {
                    captured_effect(
                        world,
                        &request,
                        EditorAction::HostRequest(intent.request),
                        intent.source,
                        intent.area,
                    )
                }));
                if accepted == bed_editor_api::ExternalFileDropResponse::Accepted {
                    return apply_batch(world, effects, id, EditorRequestOutcome::Applied);
                }
            }
            if event.phase != bed_editor_api::ExternalFileDragPhase::Drop {
                return apply_batch(world, effects, id, EditorRequestOutcome::Ignored);
            }
            let area = bed_workbench::area_at(world, event.position, event.viewport)
                .unwrap_or(request.area);
            effects.extend(event.paths.into_iter().map(|path| {
                let action = if path.is_dir() {
                    EditorAction::SetWorkspace(WorkspaceSpec::local(path.to_string_lossy()))
                } else {
                    EditorAction::OpenFileWithViewer {
                        path,
                        viewer: None,
                        additional: false,
                        move_existing: true,
                    }
                };
                captured_effect(world, &request, action, request.source, Some(area))
            }));
            return apply_batch(world, effects, id, EditorRequestOutcome::Applied);
        }
        EditorAction::FocusPanel(panel) => {
            focus_panel(world, panel, true)?;
            EditorRequestOutcome::Applied
        }
        EditorAction::ObserveFocus(panel) => {
            focus_panel(world, panel, false)?;
            EditorRequestOutcome::Applied
        }
        EditorAction::FocusArea(area) => {
            bed_workbench::focus_area(world, area);
            EditorRequestOutcome::Applied
        }
        EditorAction::ClosePanel(panel) => return begin_close(world, vec![panel], false, id),
        EditorAction::ClosePanels(panels) => return begin_close(world, panels, false, id),
        EditorAction::OpenFile(path) => {
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
        EditorAction::OpenFileWithViewer {
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
        EditorAction::SwitchViewer { panel, viewer } => {
            switch_viewer(world, panel, &viewer)?;
            EditorRequestOutcome::Applied
        }
        EditorAction::AttachWorkspace(path) => {
            attach_workspace(world, &path)?;
            EditorRequestOutcome::Applied
        }
        EditorAction::SetWorkspace(spec) => return begin_workspace(world, spec, id),
        EditorAction::ApplyEdits {
            document,
            revision,
            edits,
        } => {
            commit_documents(world, &HashSet::from([document]))?;
            world
                .non_send_mut::<EditorSession>()
                .apply_edits(document, revision, &edits)?;
            EditorRequestOutcome::Applied
        }
        EditorAction::SaveDocuments(documents) => {
            return begin_save(world, documents, None, None, id);
        }
        EditorAction::SaveAs { document, path } => {
            return begin_save_as(world, document, &path, id);
        }
        EditorAction::Command(command) => return window_command(world, command, &request, id),
        EditorAction::HostRequest(host) => return apply_host_request(world, host, &request, id),
        EditorAction::HostRequests { requests } => {
            let requests = requests
                .into_iter()
                .map(|host| {
                    captured_effect(
                        world,
                        &request,
                        EditorAction::HostRequest(host),
                        request.source,
                        None,
                    )
                })
                .collect();
            return apply_batch(world, requests, id, EditorRequestOutcome::Applied);
        }
    };
    if let EditorRequestOutcome::Opened { panel, .. } = &outcome {
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
        .filter(|panel| bed_workbench_api::panel_active(world, *panel))
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
        .resource::<bed_workbench_api::Registry>()
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
                    .get::<EditorPanelBinding>(*entity)
                    .is_some_and(|binding| binding.input == input)
            })
            .or_else(|| {
                world
                    .query::<(Entity, &PanelRecord, &EditorPanelBinding)>()
                    .iter(world)
                    .find_map(|(entity, record, binding)| {
                        (bed_workbench_api::panel_active(world, entity)
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
    let binding = EditorPanelBinding {
        input,
        document: None,
        viewer,
        workspace: world.non_send::<EditorSession>().workspace_id().0,
        ..Default::default()
    };
    world
        .entity_mut(panel)
        .insert((binding.clone(), EditorPresentations::default()));
    mount_presentation(world, panel, kind, binding, state)?;
    focus_panel(world, panel, true)?;
    Ok(panel)
}

fn mount_presentation(
    world: &mut World,
    panel: Entity,
    kind: &str,
    binding: EditorPanelBinding,
    state: Value,
) -> io::Result<Entity> {
    let binding = EditorPanelBinding {
        document: binding.input.document().or(binding.document),
        ..binding
    };
    let factory = world
        .resource::<bed_workbench_api::Registry>()
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
                        .resource::<bed_editor_api::Registry>()
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
    let binding = world.get::<EditorPanelBinding>(entity).unwrap().clone();
    world.entity_mut(panel).insert(binding.clone());
    let key = binding.viewer.clone().unwrap_or_else(|| kind.into());
    world
        .entity_mut(panel)
        .insert(bed_workbench_api::PanelFrontend(entity));
    world
        .get_mut::<EditorPresentations>(panel)
        .unwrap()
        .retained
        .insert(key, entity);
    sync_local_file_protections(world);
    Ok(entity)
}

pub fn focus_panel(world: &mut World, source: Entity, request_native: bool) -> io::Result<()> {
    let Some(panel) = bed_workbench_api::panel_owner(world, source) else {
        return Ok(());
    };
    if !bed_workbench::owns_panel(world, panel) || !bed_workbench_api::panel_active(world, panel) {
        return Ok(());
    }
    let changed = world.resource::<WorkbenchLayout>().focused != Some(panel);
    if request_native {
        bed_workbench::focus_panel(world, panel);
    } else {
        bed_workbench::observe_focus(world, panel);
    }
    let binding = world
        .get::<EditorPanelBinding>(panel)
        .cloned()
        .unwrap_or_default();
    let last_binding = world
        .resource::<EditorNavigation>()
        .last_document_panel
        .and_then(|panel| world.get::<EditorPanelBinding>(panel))
        .cloned();
    {
        let mut navigation = world.resource_mut::<EditorNavigation>();
        if binding.document.is_some() {
            navigation.active_view = binding.view;
            navigation.active_document = binding.document;
            navigation.last_document_panel = Some(panel);
        } else if binding.input.local_file().is_some() {
            navigation.active_view = None;
            navigation.active_document = None;
        } else if let Some(last) = last_binding {
            navigation.active_view = last.view;
            navigation.active_document = last.document;
        }
    }
    if (changed || request_native)
        && let Some(entity) = panel_presentation(world, panel)
        && let Some(handler) = world.get::<EditorPanelFocus>(entity).copied()
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
    let session = world.non_send::<EditorSession>().workspace_id().0;
    let slot = world.resource::<LayoutSlots>().active();
    let mut canceled = Vec::new();
    {
        let mut operations = world.non_send_mut::<RemoteOperations>();
        let opens = std::mem::take(&mut operations.opens);
        for pending in opens {
            if pending.session == session
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
        complete(world, id, Ok(EditorRequestOutcome::Ignored));
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
    let remote = world.non_send::<EditorSession>().is_remote();
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
                .resource::<bed_editor_api::Registry>()
                .viewer_for_path(path_string)
                .is_some_and(|viewer| viewer.is_local_file())
            {
                return Err(io::Error::other("This viewer opens local files only"));
            }
            let document = world
                .non_send_mut::<EditorSession>()
                .open_file_auto(&path)?;
            Some(world.non_send::<EditorSession>().document_kind(document)?)
        } else {
            None
        }
    } else {
        Some(bed_files::files::classify_bytes(
            &bed_files::files::read_file_bytes(&path)?.raw,
        ))
    };
    let viewer = {
        let registry = world.resource::<bed_editor_api::Registry>();
        match explicit {
            Some(id) => registry.viewer(id),
            None => kind
                .and_then(|kind| {
                    registry.preferred_viewer_for_file(
                        path_string,
                        kind,
                        &world.non_send::<Settings>().settings["default_viewers"],
                    )
                })
                .or_else(|| registry.viewer_for_path(path_string)),
        }
        .cloned()
        .ok_or_else(|| io::Error::other("File viewer is unavailable"))?
    };
    if viewer.is_local_file() {
        if remote {
            return Err(io::Error::other("This viewer opens local files only"));
        }
        if world
            .non_send::<EditorSession>()
            .document_ids()
            .into_iter()
            .any(|id| {
                world
                    .non_send::<EditorSession>()
                    .with_document(id, |state| Path::new(&state.path) == path)
                    .unwrap_or(false)
            })
        {
            return Err(io::Error::other(
                "This file is already an editable document",
            ));
        }
    }
    let input = if viewer.is_local_file() {
        PanelInput::LocalFile(path.clone())
    } else {
        PanelInput::Document(match origin {
            Some(origin) => world
                .non_send_mut::<EditorSession>()
                .open_file_from_lsp(&path, origin)?,
            None => world.non_send_mut::<EditorSession>().open_file_with_kind(
                &path,
                viewer
                    .document_kind()
                    .unwrap_or(bed_editor_api::DocumentKind::Text),
            )?,
        })
    };
    if !additional {
        let recent = if viewer.is_local_file() {
            bed_workbench::recent_panel(world, viewer.panel_type).filter(|panel| {
                world
                    .get::<EditorPanelBinding>(*panel)
                    .is_some_and(|binding| binding.input == input)
            })
        } else {
            None
        };
        let existing = recent.or_else(|| {
            world
                .query::<(
                    Entity,
                    &PanelPresentation,
                    &EditorPanelBinding,
                    &PanelRecord,
                )>()
                .iter(world)
                .find_map(|(entity, _, binding, record)| {
                    (bed_workbench_api::panel_active(world, entity)
                        && binding.input == input
                        && (!viewer.is_local_file() || record.panel_type == viewer.panel_type))
                        .then_some(entity)
                })
        });
        if let Some(panel) = existing {
            if explicit.is_some() && !viewer.is_local_file() {
                switch_viewer(world, panel, viewer.id)?;
            }
            if mode == FileOpenMode::Keep {
                bed_workbench::keep_panel(world, panel);
            }
            focus_panel(world, panel, true)?;
            return Ok(panel);
        }
    }
    let target = area.unwrap_or_else(|| {
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
            .get::<EditorPanelBinding>(panel)
            .and_then(|binding| binding.document)
    {
        if let Err(error) = commit_documents(world, &HashSet::from([document])) {
            bed_workbench::keep_panel(world, panel);
            return Err(error);
        }
        let snapshot = world.non_send::<EditorSession>().snapshot(document)?;
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

pub fn switch_viewer(world: &mut World, panel: Entity, viewer: &str) -> io::Result<()> {
    let panel = bed_workbench_api::panel_owner(world, panel)
        .ok_or_else(|| io::Error::other("Panel is unavailable"))?;
    let descriptor = world
        .resource::<bed_editor_api::Registry>()
        .viewer(viewer)
        .cloned()
        .ok_or_else(|| io::Error::other("Viewer is unavailable"))?;
    let mut binding = world
        .get::<EditorPanelBinding>(panel)
        .cloned()
        .ok_or_else(|| io::Error::other("Panel has no document"))?;
    let document = binding
        .document
        .ok_or_else(|| io::Error::other("Panel has no document"))?;
    commit_documents(world, &HashSet::from([document]))?;
    if !descriptor
        .supports_document_kind(world.non_send::<EditorSession>().document_kind(document)?)
    {
        return Err(io::Error::other("Viewer cannot display this document kind"));
    }
    let retained = world
        .get::<EditorPresentations>(panel)
        .and_then(|presentations| presentations.retained.get(descriptor.id))
        .copied();
    let presentation = match retained {
        Some(entity) => entity,
        None => {
            let previous_binding = binding.clone();
            let previous_record = world.get::<PanelRecord>(panel).unwrap().clone();
            let previous_frontend = world
                .get::<bed_workbench_api::PanelFrontend>(panel)
                .copied();
            let previous_retained = world
                .get::<EditorPresentations>(panel)
                .unwrap()
                .retained
                .clone();
            binding.viewer = Some(descriptor.id.into());
            binding.view = None;
            let created = mount_presentation(
                world,
                panel,
                descriptor.panel_type,
                binding,
                world
                    .get::<PanelRecord>(panel)
                    .and_then(|record| record.serialized.get("presentations"))
                    .and_then(|states| states.get(viewer))
                    .cloned()
                    .unwrap_or(Value::Null),
            )?;
            if let Some(error) = world
                .get::<PanelRecord>(created)
                .and_then(|record| record.unavailable.clone())
            {
                world
                    .entity_mut(panel)
                    .insert((previous_binding, previous_record));
                world
                    .get_mut::<EditorPresentations>(panel)
                    .unwrap()
                    .retained = previous_retained;
                if let Some(frontend) = previous_frontend {
                    world.entity_mut(panel).insert(frontend);
                } else {
                    world
                        .entity_mut(panel)
                        .remove::<bed_workbench_api::PanelFrontend>();
                }
                let close = world.get::<EditorPanelClose>(created).copied();
                let cleanup = match close {
                    Some(close) => world
                        .run_system_with(close.0, created)
                        .map_err(|error| error.to_string())
                        .and_then(|result| result),
                    None => Ok(()),
                };
                drop(bed_workbench::take_panel_frontend(world, created));
                world.despawn(created);
                if let Err(cleanup) = cleanup {
                    return Err(io::Error::other(format!(
                        "{error}; viewer cleanup failed: {cleanup}"
                    )));
                }
                return Err(io::Error::other(error));
            }
            created
        }
    };
    world
        .entity_mut(panel)
        .insert(bed_workbench_api::PanelFrontend(presentation));
    let binding = world
        .get::<EditorPanelBinding>(presentation)
        .unwrap()
        .clone();
    world.entity_mut(panel).insert(binding);
    world.get_mut::<PanelRecord>(panel).unwrap().panel_type = descriptor.panel_type.into();
    if let Some(handler) = world.get::<EditorPanelFocus>(presentation).copied() {
        world
            .run_system_with(handler.0, presentation)
            .map_err(io::Error::other)?
            .map_err(io::Error::other)?;
    }
    focus_panel(world, panel, true)?;
    Ok(())
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
    let panel = bed_workbench_api::panel_owner(world, source)
        .ok_or_else(|| io::Error::other("Panel is unavailable"))?;
    let presentation = panel_presentation(world, panel)
        .ok_or_else(|| io::Error::other("Panel has no presentation"))?;
    let mut state = bed_workbench::panel_state(world, presentation).map_err(io::Error::other)?;
    let binding = world.get::<EditorPanelBinding>(panel).cloned().unwrap();
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

fn sync_local_file_protections(world: &mut World) {
    let paths = world
        .query::<&EditorPanelBinding>()
        .iter(world)
        .filter_map(|binding| binding.input.local_file())
        .flat_map(|path| {
            ["", "-wal", "-shm"].map(|suffix| {
                let mut value = path.as_os_str().to_os_string();
                value.push(suffix);
                PathBuf::from(value)
            })
        })
        .collect();
    world
        .non_send_mut::<EditorSession>()
        .set_read_only_local_paths(paths);
}

pub fn commit_documents(world: &mut World, documents: &HashSet<DocumentId>) -> io::Result<()> {
    let targets = world
        .query::<(Entity, &EditorPanelBinding, &EditorPanelActions)>()
        .iter(world)
        .filter(|(_, binding, _)| {
            binding
                .document
                .is_some_and(|document| documents.contains(&document))
        })
        .map(|(entity, _, handler)| (entity, handler.0))
        .collect::<Vec<_>>();
    for (entity, handler) in targets {
        let previous = world.resource_mut::<EditorIntents>().take();
        let result = world
            .run_system_with(
                handler,
                PanelActionInvocation {
                    entity,
                    action: PanelAction::CommitEdit,
                },
            )
            .map_err(io::Error::other)
            .and_then(|result| result.map_err(io::Error::other));
        let edits = world.resource_mut::<EditorIntents>().take();
        for intent in previous {
            world.resource_mut::<EditorIntents>().push_intent(intent);
        }
        result?;
        if edits.iter().any(|edit| {
            edit.source != Some(entity)
                || !matches!(
                    edit.request,
                    HostRequest::ApplyEdits { .. } | HostRequest::ApplyEditsWithResult { .. }
                )
        }) {
            return Err(io::Error::other(
                "CommitEdit may only queue its own document edits",
            ));
        }
        for edit in edits {
            apply_panel_edit(world, entity, edit.request)?;
        }
    }
    Ok(())
}

fn apply_panel_edit(world: &mut World, source: Entity, request: HostRequest) -> io::Result<()> {
    let (document, revision, edits, token) = match request {
        HostRequest::ApplyEdits {
            document,
            revision,
            edits,
        } => (document, revision, edits, None),
        HostRequest::ApplyEditsWithResult {
            token,
            document,
            revision,
            edits,
        } => (document, revision, edits, Some(token)),
        _ => unreachable!("validated panel edit"),
    };
    let result = world
        .non_send_mut::<EditorSession>()
        .apply_edits(document, revision, &edits)
        .and_then(|()| {
            world
                .non_send::<EditorSession>()
                .document_revision(document)
        })
        .map_err(|error| error.to_string());
    publish_editor_frame(world);
    if let Some(token) = token
        && let Some(handler) = world
            .get::<bed_editor_api::EditorPanelEdits>(source)
            .copied()
    {
        world
            .run_system_with(
                handler.0,
                bed_editor_api::PanelEditResult {
                    entity: source,
                    token,
                    result: result.clone(),
                },
            )
            .map_err(io::Error::other)?;
    }
    result.map(|_| ()).map_err(io::Error::other)
}

struct SaveTarget {
    document: DocumentId,
    destination: Option<PathBuf>,
}
fn begin_save(
    world: &mut World,
    documents: Vec<DocumentId>,
    reply: Option<(String, SaveToken)>,
    continuation: Option<Continuation>,
    id: Option<EditorRequestId>,
) -> io::Result<Option<EditorRequestOutcome>> {
    begin_save_targets(
        world,
        documents
            .into_iter()
            .map(|document| SaveTarget {
                document,
                destination: None,
            })
            .collect(),
        reply,
        continuation,
        id,
    )
}
fn begin_save_as(
    world: &mut World,
    document: DocumentId,
    path: &Path,
    id: Option<EditorRequestId>,
) -> io::Result<Option<EditorRequestOutcome>> {
    begin_save_targets(
        world,
        vec![SaveTarget {
            document,
            destination: Some(path.to_path_buf()),
        }],
        None,
        None,
        id,
    )
}
// Capture every write before starting any I/O. A draft or path failure keeps
// all affected panels live and acknowledges the exact failed operation.
fn begin_save_targets(
    world: &mut World,
    mut targets: Vec<SaveTarget>,
    reply: Option<(String, SaveToken)>,
    continuation: Option<Continuation>,
    id: Option<EditorRequestId>,
) -> io::Result<Option<EditorRequestOutcome>> {
    targets.sort_by_key(|target| target.document);
    targets.dedup_by_key(|target| target.document);
    let result = (|| {
        commit_documents(
            world,
            &targets.iter().map(|target| target.document).collect(),
        )?;
        publish_editor_frame(world);
        let session = world.non_send::<EditorSession>();
        let mut documents = Vec::new();
        let mut paths = Vec::new();
        for target in &mut targets {
            let snapshot = session.snapshot(target.document)?;
            if session.is_snapshot_document(target.document) {
                return Err(io::Error::other("Comparison documents cannot be saved"));
            }
            if target.destination.is_none() && snapshot.disk_conflict.is_some() {
                return Err(io::Error::other("Resolve the disk conflict before saving"));
            }
            let path = match &target.destination {
                Some(path) if session.is_remote() => path.to_string_lossy().into_owned(),
                Some(path) => {
                    let path = session.check_rebind_path(target.document, path)?;
                    target.destination = Some(path.clone());
                    path.to_string_lossy().into_owned()
                }
                None => snapshot.path.clone(),
            };
            if path.is_empty() {
                return Err(io::Error::other(
                    "Save As is required before saving this document",
                ));
            }
            let mut revision = session.document_revision(target.document)?;
            if path != snapshot.path {
                revision = (revision.0.wrapping_add(1), revision.1.wrapping_add(1));
            }
            documents.push(SavedDocument {
                document: target.document,
                revision,
            });
            paths.push(path);
        }
        let pending = PendingSave {
            id,
            session: session.workspace_id().0,
            documents,
            paths,
            reply: reply.clone(),
            continuation,
        };
        for target in targets {
            match target.destination {
                Some(path) => {
                    world
                        .non_send_mut::<EditorSession>()
                        .save_as(target.document, &path)?;
                }
                None => {
                    world
                        .non_send_mut::<EditorSession>()
                        .save(target.document)?;
                }
            }
        }
        Ok::<_, io::Error>(pending)
    })();
    let pending = match result {
        Ok(pending) => pending,
        Err(error) => {
            if let Some((recipient, token)) = reply {
                world.write_message(EditorFeatureReply::Save {
                    recipient,
                    token,
                    result: Err(error.to_string()),
                });
            }
            return Err(error);
        }
    };
    match saved_result(world, &pending) {
        Ok(false) => {
            world
                .resource_mut::<PendingOperations>()
                .saves
                .push(pending);
            Ok(None)
        }
        Ok(true) => finish_save(world, pending).map(Some),
        Err(error) => {
            if let Some((recipient, token)) = pending.reply {
                world.write_message(EditorFeatureReply::Save {
                    recipient,
                    token,
                    result: Err(error.to_string()),
                });
            }
            Err(error)
        }
    }
}

fn saved_result(world: &World, pending: &PendingSave) -> io::Result<bool> {
    let session = world.non_send::<EditorSession>();
    if session.workspace_id().0 != pending.session {
        return Err(io::Error::other("Workspace changed while saving"));
    }
    if session.is_remote() && !session.remote_connected() {
        return Err(io::Error::other(
            "SSH disconnected before the save completed",
        ));
    }
    let mut complete = true;
    for (saved, path) in pending.documents.iter().zip(&pending.paths) {
        if session.save_pending(saved.document) {
            complete = false;
            continue;
        }
        if session.document_revision(saved.document)? != saved.revision {
            return Err(io::Error::other(
                "Document changed while saving; save and try again",
            ));
        }
        let snapshot = session.snapshot(saved.document)?;
        if snapshot.path != *path {
            return Err(io::Error::other(
                "Document moved while saving; save and try again",
            ));
        }
        if snapshot.dirty || snapshot.disk_conflict.is_some() {
            return Err(io::Error::other(
                "Document was not saved; resolve the file error and try again",
            ));
        }
    }
    Ok(complete)
}

fn finish_save(world: &mut World, pending: PendingSave) -> io::Result<EditorRequestOutcome> {
    if let Some((recipient, token)) = pending.reply {
        world.write_message(EditorFeatureReply::Save {
            recipient,
            token,
            result: Ok(pending.documents.clone()),
        });
    }
    match pending.continuation {
        None => Ok(EditorRequestOutcome::Saved {
            documents: pending.documents,
        }),
        Some(Continuation::Close(panels)) => {
            retire_panels(world, &panels)?;
            Ok(EditorRequestOutcome::Closed { panels })
        }
        Some(Continuation::Quit(panels)) => {
            persist_workspace(world)?;
            retire_panels(world, &panels)?;
            world
                .non_send_mut::<EditorSession>()
                .shutdown(ClosePolicy::Discard)?;
            world.resource_mut::<EditorWorkspace>().close_requested = true;
            Ok(EditorRequestOutcome::Closed { panels })
        }
        Some(Continuation::Workspace(spec)) => {
            replace_workspace(world, spec)?;
            Ok(EditorRequestOutcome::Applied)
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
            Ok(EditorRequestOutcome::Applied)
        }
        Some(Continuation::Connected(connected)) => {
            replace_connected_workspace(world, *connected)?;
            Ok(EditorRequestOutcome::Applied)
        }
        Some(Continuation::Layout { tiling, panels }) => {
            retire_panels(world, &panels)?;
            bed_workbench::apply_layout(world, &tiling).map_err(io::Error::other)?;
            Ok(EditorRequestOutcome::Applied)
        }
    }
}

fn begin_close(
    world: &mut World,
    panels: Vec<Entity>,
    quit: bool,
    id: Option<EditorRequestId>,
) -> io::Result<Option<EditorRequestOutcome>> {
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
    id: Option<EditorRequestId>,
) -> io::Result<Option<EditorRequestOutcome>> {
    let quit = matches!(continuation, Continuation::Quit(_));
    let panels = panels
        .into_iter()
        .filter_map(|source| bed_workbench_api::panel_owner(world, source))
        .filter(|panel| bed_workbench::owns_panel(world, *panel))
        .collect::<HashSet<_>>();
    if panels.is_empty() {
        if quit {
            persist_workspace(world)?;
            world
                .non_send_mut::<EditorSession>()
                .shutdown(ClosePolicy::Discard)?;
            world.resource_mut::<EditorWorkspace>().close_requested = true;
            return Ok(Some(EditorRequestOutcome::Applied));
        }
        if let Continuation::Layout { tiling, .. } = continuation {
            bed_workbench::apply_layout(world, &tiling).map_err(io::Error::other)?;
            return Ok(Some(EditorRequestOutcome::Applied));
        }
        if matches!(continuation, Continuation::Promotion { .. }) {
            return begin_save_targets(world, Vec::new(), None, Some(continuation), id);
        }
        return Ok(Some(EditorRequestOutcome::Ignored));
    }
    let documents = panels
        .iter()
        .filter_map(|panel| {
            world
                .get::<EditorPanelBinding>(*panel)
                .and_then(|binding| binding.document)
        })
        .collect::<HashSet<_>>();
    commit_documents(world, &documents)?;
    let remaining = world
        .query::<(Entity, &PanelPresentation, &EditorPanelBinding)>()
        .iter(world)
        .filter_map(|(entity, _, binding)| {
            (!panels.contains(&entity))
                .then_some(binding.document)
                .flatten()
        })
        .collect::<HashSet<_>>();
    let mut targets = Vec::new();
    for document in documents.difference(&remaining).copied() {
        let session = world.non_send::<EditorSession>();
        let snapshot = session.snapshot(document)?;
        if session.is_snapshot_document(document) {
            continue;
        }
        let destination = if snapshot.dirty && snapshot.path.is_empty() {
            let Some(path) = rfd::FileDialog::new()
                .set_directory(&world.resource::<EditorWorkspace>().working_directory)
                .save_file()
            else {
                return Ok(Some(EditorRequestOutcome::Ignored));
            };
            Some(path)
        } else {
            None
        };
        if destination.is_some() || !snapshot.path.is_empty() {
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
        _ => unreachable!("close transition"),
    };
    begin_save_targets(world, targets, None, Some(continuation), id)
}

fn retire_presentations(world: &mut World, panel: Entity) -> io::Result<()> {
    let presentations = world
        .query::<(Entity, &PanelOwner)>()
        .iter(world)
        .filter_map(|(entity, owner)| (owner.0 == panel).then_some(entity))
        .collect::<Vec<_>>();
    for entity in presentations {
        if let Some(close) = world.get::<EditorPanelClose>(entity).copied() {
            world
                .run_system_with(close.0, entity)
                .map_err(io::Error::other)?
                .map_err(io::Error::other)?;
        }
        drop(bed_workbench::take_panel_frontend(world, entity));
        world.despawn(entity);
    }
    if let Some(mut presentations) = world.get_mut::<EditorPresentations>(panel) {
        presentations.retained.clear();
    }
    world
        .entity_mut(panel)
        .remove::<bed_workbench_api::PanelFrontend>();
    Ok(())
}
fn pristine_panel(world: &World, panel: Entity) -> bool {
    let Some(surface) = panel_presentation(world, panel) else {
        return false;
    };
    let Some(stamp) = world.get::<bed_editor_api::EditorPanelDefaultInput>(surface) else {
        return false;
    };
    let session = world.non_send::<EditorSession>();
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
            .get::<EditorPanelBinding>(panel)
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
        .get::<EditorPanelBinding>(panel)
        .and_then(|binding| binding.document);
    retire_presentations(world, panel)?;
    let binding = EditorPanelBinding {
        input: PanelInput::Document(document),
        document: Some(document),
        viewer: Some(viewer.into()),
        workspace: world.non_send::<EditorSession>().workspace_id().0,
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
            .query::<(&PanelPresentation, &EditorPanelBinding)>()
            .iter(world)
            .any(|(_, binding)| binding.document == Some(old));
        if !retained {
            world
                .non_send_mut::<EditorSession>()
                .close_document(old, ClosePolicy::Discard)?;
        }
    }
    focus_panel(world, panel, true)?;
    Ok(panel)
}

pub fn retire_panels(world: &mut World, panels: &[Entity]) -> io::Result<()> {
    let documents = panels
        .iter()
        .filter_map(|panel| {
            world
                .get::<EditorPanelBinding>(*panel)
                .and_then(|binding| binding.document)
        })
        .collect::<HashSet<_>>();
    for &panel in panels {
        retire_presentations(world, panel)?;
        bed_workbench::commit_close(world, panel);
    }
    let retained = world
        .query::<(&PanelPresentation, &EditorPanelBinding)>()
        .iter(world)
        .filter_map(|(_, binding)| binding.document)
        .collect::<HashSet<_>>();
    for document in documents.difference(&retained).copied() {
        world
            .non_send_mut::<EditorSession>()
            .close_document(document, ClosePolicy::Discard)?;
    }
    sync_local_file_protections(world);
    if world
        .resource::<EditorNavigation>()
        .last_document_panel
        .is_some_and(|panel| !world.entities().contains(panel))
    {
        let last = active_panel_entities(world).into_iter().find(|panel| {
            world
                .get::<EditorPanelBinding>(*panel)
                .is_some_and(|binding| binding.document.is_some())
        });
        world.resource_mut::<EditorNavigation>().last_document_panel = last;
        if last.is_none() {
            let mut navigation = world.resource_mut::<EditorNavigation>();
            navigation.active_document = None;
            navigation.active_view = None;
        }
    }
    if let Some(panel) = world.resource::<WorkbenchLayout>().focused {
        focus_panel(world, panel, true)?;
    } else {
        let mut navigation = world.resource_mut::<EditorNavigation>();
        navigation.active_view = None;
        navigation.active_document = None;
        navigation.last_document_panel = None;
    }
    Ok(())
}

fn update_editor_services(world: &mut World) {
    let mut options = world.non_send::<EditorSession>().options().clone();
    let settings = world.non_send::<Settings>();
    options.autosave = settings
        .autosave_enabled()
        .then(|| settings.autosave_delay());
    options.git = settings.bool("git_changed_lines", true);
    options.highlighting = settings.bool("treesitter", true);
    options.monitoring = true;
    let highlight_theme = settings.highlight_colors();
    world
        .non_send_mut::<EditorSession>()
        .set_highlight_theme(highlight_theme);
    if world.non_send::<EditorSession>().options() != &options
        && let Err(error) = world.non_send_mut::<EditorSession>().configure(options)
    {
        world.resource_mut::<EditorWorkspace>().error = Some(error.to_string());
    }

    let report = world.non_send_mut::<EditorSession>().tick();
    format_save::poll(world);
    let pending = std::mem::take(&mut world.resource_mut::<PendingOperations>().saves);
    for save in pending {
        let failure = report.errors.iter().find(|error| {
            (error.document.is_none()
                || save
                    .documents
                    .iter()
                    .any(|saved| Some(saved.document) == error.document))
                && matches!(error.service, "remote" | "save" | "autosave")
        });
        let result = match failure {
            Some(error) => Err(io::Error::other(error.message.clone())),
            None => saved_result(world, &save),
        };
        match result {
            Ok(false) => world.resource_mut::<PendingOperations>().saves.push(save),
            Ok(true) => {
                let id = save.id;
                let result = finish_save(world, save).map_err(EditorRequestError::from);
                complete(world, id, result);
            }
            Err(error) => {
                if let Some((recipient, token)) = save.reply {
                    world.write_message(EditorFeatureReply::Save {
                        recipient,
                        token,
                        result: Err(error.to_string()),
                    });
                }
                complete(world, save.id, Err(error.into()));
            }
        }
    }
    if let Err(error) = poll_remote_operations(world, &report.errors) {
        world.resource_mut::<EditorWorkspace>().error = Some(error.to_string());
    }
    poll_batches(world);
    layouts::poll(world);
    for event in &report.events {
        if let bed_document_session::editor_session::SessionEvent::Removed { document, .. } = event
        {
            let panels = panel_entities(world)
                .into_iter()
                .filter(|panel| {
                    world
                        .get::<EditorPanelBinding>(*panel)
                        .is_some_and(|binding| binding.document == Some(*document))
                })
                .collect::<Vec<_>>();
            let result = commit_documents(world, &HashSet::from([*document]));
            let dirty = world
                .non_send::<EditorSession>()
                .snapshot(*document)
                .is_ok_and(|snapshot| snapshot.dirty);
            if result.is_err() || dirty {
                if let Err(error) = world
                    .non_send_mut::<EditorSession>()
                    .detach_removed_document(*document)
                {
                    world.resource_mut::<EditorWorkspace>().error = Some(error.to_string());
                }
                if let Err(error) = result {
                    world.resource_mut::<EditorWorkspace>().error =
                        Some(format!("Deleted file buffer preserved: {error}"));
                }
            } else if let Err(error) = retire_panels(world, &panels) {
                world.resource_mut::<EditorWorkspace>().error = Some(error.to_string());
                let _ = world
                    .non_send_mut::<EditorSession>()
                    .detach_removed_document(*document);
            }
        }
    }
    for event in report.events {
        world.write_message(bed_editor_api::EditorDocumentEvent(event));
    }
    for error in report.errors {
        world.resource_mut::<EditorWorkspace>().error = Some(error.message);
    }
    if world.resource::<EditorInitialized>().0
        && !world.resource::<EditorWorkspace>().close_requested
        && world
            .non_send::<WorkspacePersistence>()
            .last_saved
            .elapsed()
            >= std::time::Duration::from_secs(2)
        && let Err(error) = persist_workspace(world)
    {
        world.resource_mut::<EditorWorkspace>().error = Some(error.to_string());
    }
    publish_editor_frame(world);
}

pub fn publish_editor_frame(world: &mut World) {
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
        .query::<(&PanelPresentation, &EditorPanelBinding)>()
        .iter(world)
        .filter_map(|(panel, binding)| {
            (preview_ids.contains(&panel.id)
                && binding.document.is_some_and(|document| {
                    world
                        .non_send::<EditorSession>()
                        .with_document(document, |state| state.dirty)
                        .unwrap_or(false)
                }))
            .then_some(panel.id)
        })
        .collect::<Vec<_>>();
    for id in edited {
        world.resource_mut::<WorkbenchLayout>().tiling.keep_tab(id);
    }
    world.resource_scope(|world, mut frame: Mut<EditorFrame>| {
        let session = world.non_send::<EditorSession>();
        frame.workspace = session.workspace_id().0;
        frame.remote = session.is_remote();
        frame.remote_disconnected = session
            .remote_client()
            .is_some_and(|client| !client.is_connected());
        frame.remote_connecting = world.non_send::<RemoteOperations>().connection.is_some();
        frame.recent_workspaces = world
            .non_send::<WorkspacePersistence>()
            .store
            .recent_workspaces();
        if let Err(error) = frame.refresh_documents(session) {
            world.resource_mut::<EditorWorkspace>().error = Some(error.to_string());
        }
        let workspace = world.resource::<EditorWorkspace>();
        frame.project_root.clone_from(&workspace.project_root);
        frame.working_directory = workspace.working_directory.to_string_lossy().into_owned();
        let navigation = world.resource::<EditorNavigation>();
        frame.active_view = navigation.active_view;
        frame.active_document = navigation.active_document;
        let settings = world.non_send::<Settings>();
        frame.settings = settings.settings.clone();
        frame.default_viewers = settings.settings["default_viewers"].clone();
        frame.animations = settings.bool("ui_animations", true);
    });
}

fn window_command(
    world: &mut World,
    command: WindowCommand,
    request: &EditorRequest,
    id: Option<EditorRequestId>,
) -> io::Result<Option<EditorRequestOutcome>> {
    let plugin = match command {
        WindowCommand::Debug => Some("bed.debug.show"),
        WindowCommand::FindFile => Some("bed.explorer.find_file"),
        WindowCommand::FindProject => Some("bed.search.show"),
        _ => None,
    };
    if let Some(command) = plugin {
        if world
            .resource::<bed_workbench_api::Registry>()
            .command(command)
            .is_none()
        {
            return Ok(Some(EditorRequestOutcome::Ignored));
        }
        if command == "bed.debug.show" && world.resource::<EditorWorkspace>().spec.is_none() {
            return Ok(Some(EditorRequestOutcome::Ignored));
        }
        let mut captured = capture_editor_request_from(
            world,
            EditorAction::PluginCommand(command.into()),
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
        WindowCommand::NewProjects | WindowCommand::Projects => {
            Some(("bed.projects.panel", command == WindowCommand::Projects))
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
            .resource::<bed_workbench_api::Registry>()
            .panel(kind)
            .is_none()
        {
            return Ok(Some(EditorRequestOutcome::Ignored));
        }
        if kind == "bed.editor.diagnostics-panel"
            && world.resource::<EditorWorkspace>().spec.is_none()
        {
            return Ok(Some(EditorRequestOutcome::Ignored));
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
        return Ok(Some(EditorRequestOutcome::Opened {
            panel,
            document: world
                .get::<EditorPanelBinding>(panel)
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
            EditorRequestOutcome::Opened {
                panel,
                document: world
                    .get::<EditorPanelBinding>(panel)
                    .and_then(|binding| binding.document),
            }
        }
        WindowCommand::Save => {
            let Some(document) = request.context.document else {
                return Ok(Some(EditorRequestOutcome::Ignored));
            };
            if world
                .non_send::<EditorSession>()
                .with_document(document, |state| state.path.is_empty())?
            {
                return window_command(world, WindowCommand::SaveAs, request, id);
            }
            return format_save::begin(world, document, id);
        }
        WindowCommand::SaveAs => {
            let Some(document) = request.context.document else {
                return Ok(Some(EditorRequestOutcome::Ignored));
            };
            let snapshot = world.non_send::<EditorSession>().snapshot(document)?;
            let dialog = rfd::FileDialog::new()
                .set_directory(&world.resource::<EditorWorkspace>().working_directory);
            let dialog = if snapshot.path.is_empty() {
                dialog
            } else {
                dialog.set_file_name(
                    Path::new(&snapshot.path)
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy(),
                )
            };
            let Some(path) = dialog.save_file() else {
                return Ok(Some(EditorRequestOutcome::Ignored));
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
                return Ok(Some(EditorRequestOutcome::Ignored));
            };
            let Some(handler) = world.get::<EditorPanelActions>(source).copied() else {
                return Ok(Some(EditorRequestOutcome::Ignored));
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
                EditorRequestOutcome::Applied
            } else {
                EditorRequestOutcome::Ignored
            }
        }
        WindowCommand::SplitRight | WindowCommand::SplitDown | WindowCommand::DuplicateView => {
            let Some(panel) = request.panel else {
                return Ok(Some(EditorRequestOutcome::Ignored));
            };
            if command == WindowCommand::DuplicateView {
                let presentation = panel_presentation(world, panel)
                    .ok_or_else(|| io::Error::other("Panel is unavailable"))?;
                let state =
                    bed_workbench::panel_state(world, presentation).map_err(io::Error::other)?;
                let binding = world.get::<EditorPanelBinding>(panel).cloned().unwrap();
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
                EditorRequestOutcome::Opened {
                    panel,
                    document: world
                        .get::<EditorPanelBinding>(panel)
                        .and_then(|binding| binding.document),
                }
            } else {
                let panel = split_panel(
                    world,
                    panel,
                    usize::from(command == WindowCommand::SplitDown),
                    true,
                )?;
                EditorRequestOutcome::Opened {
                    panel,
                    document: world
                        .get::<EditorPanelBinding>(panel)
                        .and_then(|binding| binding.document),
                }
            }
        }
        WindowCommand::OpenFile => {
            return apply_host_request(
                world,
                HostRequest::OpenFileDialog { viewer: None },
                request,
                id,
            );
        }
        WindowCommand::OpenFolder => {
            return apply_host_request(
                world,
                HostRequest::Project(ProjectRequest::OpenFolder),
                request,
                id,
            );
        }
        WindowCommand::UseLayout(slot) => return layouts::request_switch(world, slot, id),
        WindowCommand::SetCurrentLayout => {
            layouts::save_default(world, world.resource::<LayoutSlots>().active())?;
            EditorRequestOutcome::Applied
        }
        WindowCommand::SetLayoutDefault(slot) => {
            layouts::save_default(world, slot)?;
            EditorRequestOutcome::Applied
        }
        WindowCommand::RestoreBuiltinLayout(slot) => {
            world
                .non_send_mut::<Settings>()
                .set_layout_default(slot, None)?;
            EditorRequestOutcome::Applied
        }
        WindowCommand::SaveDefaultLayout => {
            save_default_layout(world)?;
            EditorRequestOutcome::Applied
        }
        WindowCommand::ResetDefaultLayout => {
            world
                .non_send_mut::<Settings>()
                .set_layout_default(LayoutSlot::Layout1, None)?;
            EditorRequestOutcome::Applied
        }
        WindowCommand::ResetLayout => {
            reset_layout(world)?;
            EditorRequestOutcome::Applied
        }
        _ => EditorRequestOutcome::Ignored,
    };
    if let EditorRequestOutcome::Opened { panel, .. } = &outcome {
        place_opened_panel(world, *panel, Some(request.area), false, request.tab_index)?;
    }
    Ok(Some(outcome))
}

fn apply_host_request(
    world: &mut World,
    host: HostRequest,
    request: &EditorRequest,
    id: Option<EditorRequestId>,
) -> io::Result<Option<EditorRequestOutcome>> {
    let relocate_panel = matches!(&host, HostRequest::ShowPanelIn { .. });
    let outcome = match host {
        HostRequest::OpenTerminalCompanions { paths } => {
            return open_terminal_companions(world, paths, request).map(Some);
        }
        HostRequest::PromoteWorkspace { root } => {
            return begin_terminal_promotion(world, &root, request, id);
        }
        HostRequest::Command { command, context } => {
            let mut captured = capture_editor_request_from(
                world,
                EditorAction::PluginCommand(command),
                PanelTarget::LargestArea,
                request.source,
            );
            captured.context = context;
            captured.area = request.area;
            captured.tab_index = request.tab_index;
            return apply_request(world, captured, id);
        }
        HostRequest::Project(action) => match action {
            ProjectRequest::OpenFolder => {
                let Some(path) = rfd::FileDialog::new()
                    .set_directory(&world.resource::<EditorWorkspace>().working_directory)
                    .pick_folder()
                else {
                    return Ok(Some(EditorRequestOutcome::Ignored));
                };
                attach_workspace(world, &path)?;
                EditorRequestOutcome::Applied
            }
            ProjectRequest::OpenProject(path) => {
                attach_workspace(world, &path)?;
                EditorRequestOutcome::Applied
            }
            ProjectRequest::OpenWorkspace(spec) => return begin_workspace(world, spec, id),
            ProjectRequest::OpenWorkspaceWindow(spec) => {
                world.resource_mut::<EditorWorkspace>().windows.push(spec);
                EditorRequestOutcome::Applied
            }
            ProjectRequest::RemoveRecent(path) => {
                world
                    .non_send_mut::<WorkspacePersistence>()
                    .store
                    .forget_workspace(&WorkspaceSpec::local(path.to_string_lossy()))?;
                EditorRequestOutcome::Applied
            }
            ProjectRequest::RemoveWorkspace(spec) => {
                world
                    .non_send_mut::<WorkspacePersistence>()
                    .store
                    .forget_workspace(&spec)?;
                EditorRequestOutcome::Applied
            }
            ProjectRequest::RenameWorkspace(spec, name) => {
                let renamed = world
                    .non_send_mut::<WorkspacePersistence>()
                    .store
                    .rename_workspace(&spec, &name)?;
                if world
                    .resource::<EditorWorkspace>()
                    .spec
                    .as_ref()
                    .is_some_and(|current| current.identity() == spec.identity())
                {
                    world.resource_mut::<EditorWorkspace>().spec = Some(renamed);
                }
                EditorRequestOutcome::Applied
            }
            ProjectRequest::Reconnect => {
                reconnect_workspace(world, id)?;
                return Ok(None);
            }
            ProjectRequest::Error(message) => return Err(io::Error::other(message)),
        },
        HostRequest::ClosePanels { panel_type } => {
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
        HostRequest::FocusView { view } => {
            let panel = world
                .query::<(Entity, &PanelPresentation, &EditorPanelBinding)>()
                .iter(world)
                .find_map(|(entity, _, binding)| {
                    (bed_workbench_api::panel_active(world, entity) && binding.view == Some(view))
                        .then_some(entity)
                });
            if let Some(panel) = panel {
                focus_panel(world, panel, true)?;
                EditorRequestOutcome::Applied
            } else {
                EditorRequestOutcome::Ignored
            }
        }
        HostRequest::ShowTerminal { id: terminal } => {
            let panel = world
                .query::<(Entity, &PanelPresentation, &EditorPanelBinding)>()
                .iter(world)
                .find_map(|(entity, _, binding)| {
                    (bed_workbench_api::panel_active(world, entity)
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
            EditorRequestOutcome::Opened {
                panel,
                document: None,
            }
        }
        HostRequest::Invalidate => EditorRequestOutcome::Applied,
        HostRequest::PersistWorkspace => {
            persist_workspace(world)?;
            EditorRequestOutcome::Applied
        }
        HostRequest::ShowPanel {
            panel_type,
            document,
            state,
            action,
        }
        | HostRequest::ShowPanelIn {
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
            EditorRequestOutcome::Opened { panel, document }
        }
        HostRequest::OpenPanel {
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
            EditorRequestOutcome::Opened { panel, document }
        }
        HostRequest::OpenFile { path, viewer } | HostRequest::OpenFileIn { path, viewer, .. } => {
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
        HostRequest::PreviewFileIn { path, viewer, .. } => {
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
        HostRequest::DropFile { path } => {
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
        HostRequest::OpenFileDialog { viewer } => {
            let extensions = viewer
                .as_deref()
                .and_then(|viewer| world.resource::<bed_editor_api::Registry>().viewer(viewer))
                .map(|viewer| viewer.extensions)
                .unwrap_or(&[]);
            let directory = world
                .resource::<EditorWorkspace>()
                .working_directory
                .clone();
            let path = if let Some(mut dialogs) =
                world.get_non_send_mut::<Box<dyn bed_editor_api::FileDialogService>>()
            {
                dialogs.pick_file(&directory, extensions)
            } else {
                rfd::FileDialog::new().set_directory(&directory).pick_file()
            };
            let Some(path) = path else {
                return Ok(Some(EditorRequestOutcome::Ignored));
            };
            let panel = open_file(world, &path, viewer.as_deref(), false, Some(request.area))?;
            EditorRequestOutcome::Opened {
                panel,
                document: world
                    .get::<EditorPanelBinding>(panel)
                    .and_then(|binding| binding.document),
            }
        }
        HostRequest::SwitchViewer {
            tab,
            document,
            viewer,
        } => {
            let panel = request
                .panel
                .filter(|entity| {
                    world
                        .get::<EditorPanelBinding>(*entity)
                        .is_some_and(|binding| binding.document == Some(document))
                })
                .or_else(|| {
                    active_panel_entities(world).into_iter().find(|entity| {
                        world
                            .get::<PanelPresentation>(*entity)
                            .is_some_and(|panel| panel.id == tab)
                            && world
                                .get::<EditorPanelBinding>(*entity)
                                .is_some_and(|binding| binding.document == Some(document))
                    })
                });
            if let Some(panel) = panel {
                switch_viewer(world, panel, &viewer)?;
                EditorRequestOutcome::Applied
            } else {
                EditorRequestOutcome::Ignored
            }
        }
        HostRequest::Navigate {
            document,
            byte_offset,
            revision,
        } => {
            if world
                .non_send::<EditorSession>()
                .document_revision(document)?
                != revision
            {
                return Ok(Some(EditorRequestOutcome::Ignored));
            }
            let panel = request
                .panel
                .filter(|entity| {
                    world
                        .get::<EditorPanelBinding>(*entity)
                        .is_some_and(|binding| {
                            binding.document == Some(document) && binding.view.is_some()
                        })
                })
                .or_else(|| {
                    active_panel_entities(world).into_iter().find(|entity| {
                        world
                            .get::<EditorPanelBinding>(*entity)
                            .is_some_and(|binding| {
                                binding.document == Some(document) && binding.view.is_some()
                            })
                    })
                });
            let Some(panel) = panel else {
                return Ok(Some(EditorRequestOutcome::Ignored));
            };
            let view = world
                .get::<EditorPanelBinding>(panel)
                .unwrap()
                .view
                .unwrap();
            let (row, column) = world
                .non_send::<EditorSession>()
                .with_document(document, |state| state.row_col_from_offset(byte_offset))?;
            world
                .non_send_mut::<EditorSession>()
                .with_view(view, |view| {
                    view.commands().set_cursor(
                        row,
                        column,
                        false,
                        bed_editing::editor_commands::CursorReveal::Center,
                    )
                })?;
            focus_panel(world, panel, true)?;
            EditorRequestOutcome::Applied
        }
        HostRequest::NavigateLocation {
            path,
            row,
            utf16_column,
            origin,
        } => {
            if origin.is_some_and(|origin| {
                !world
                    .non_send::<EditorSession>()
                    .accepts_lsp_origin(&origin)
            }) {
                return Ok(Some(EditorRequestOutcome::Ignored));
            }
            let path = if world.non_send::<EditorSession>().is_remote() {
                PathBuf::from(path)
            } else {
                std::fs::canonicalize(path)?
            };
            let document = world.non_send::<EditorSession>().document_for_path(&path);
            let requested_view = origin.map(|origin| origin.view_id).or_else(|| {
                request
                    .source
                    .and_then(|source| world.get::<EditorPanelBinding>(source))
                    .and_then(|binding| binding.view)
            });
            let source = requested_view.and_then(|view| {
                world
                    .query::<(Entity, &EditorPanelBinding)>()
                    .iter(world)
                    .find_map(|(entity, binding)| {
                        (binding.view == Some(view)
                            && binding.document == document
                            && document.is_some())
                        .then_some(entity)
                    })
            });
            let panel = if let Some(source) = source {
                let panel = bed_workbench_api::panel_owner(world, source)
                    .expect("live editor presentation belongs to a layout panel");
                if panel_presentation(world, panel) != Some(source) {
                    let viewer = world
                        .get::<EditorPanelBinding>(source)
                        .and_then(|binding| binding.viewer.clone())
                        .unwrap_or_else(|| "bed.editor.text".into());
                    switch_viewer(world, panel, &viewer)?;
                }
                panel
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
                            && world.non_send::<EditorSession>().is_remote() =>
                    {
                        let follow_up = captured_effect(
                            world,
                            request,
                            EditorAction::HostRequest(HostRequest::NavigateLocation {
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
                                session: request.session,
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
                .get::<EditorPanelBinding>(panel)
                .expect("text panel has a document binding");
            let document = binding.document.expect("text panel has a document");
            let view = binding.view.expect("text panel has a managed view");
            let column = world
                .non_send::<EditorSession>()
                .with_document(document, |state| {
                    bed_editing::util::utf8::utf16_to_utf8_byte_offset(
                        &state.line(row),
                        utf16_column,
                    )
                })?;
            world
                .non_send_mut::<EditorSession>()
                .with_commands(view, |commands| {
                    commands.set_cursor(
                        row,
                        column,
                        false,
                        bed_editing::editor_commands::CursorReveal::Center,
                    )
                })?;
            focus_panel(world, panel, true)?;
            EditorRequestOutcome::Applied
        }
        HostRequest::SaveAsDialog { document } => {
            let mut captured = captured_effect(
                world,
                request,
                EditorAction::Command(WindowCommand::SaveAs),
                request.source,
                None,
            );
            captured.context.document = Some(document);
            return window_command(world, WindowCommand::SaveAs, &captured, id);
        }
        HostRequest::RevealSource {
            path,
            row,
            column,
            center,
        } => {
            let panel = open_file(
                world,
                Path::new(&path),
                Some("bed.editor.text"),
                false,
                Some(request.area),
            )?;
            if let Some(view) = world
                .get::<EditorPanelBinding>(panel)
                .and_then(|binding| binding.view)
            {
                world
                    .non_send_mut::<EditorSession>()
                    .with_view(view, |view| {
                        view.commands().set_cursor(
                            row,
                            column,
                            false,
                            if center {
                                bed_editing::editor_commands::CursorReveal::Center
                            } else {
                                bed_editing::editor_commands::CursorReveal::Ensure
                            },
                        )
                    })?;
            }
            EditorRequestOutcome::Opened {
                panel,
                document: world
                    .get::<EditorPanelBinding>(panel)
                    .and_then(|binding| binding.document),
            }
        }
        HostRequest::ApplyEdits {
            document,
            revision,
            edits,
        } => {
            commit_documents(world, &HashSet::from([document]))?;
            world
                .non_send_mut::<EditorSession>()
                .apply_edits(document, revision, &edits)?;
            EditorRequestOutcome::Applied
        }
        HostRequest::ApplyEditsWithResult {
            token,
            document,
            revision,
            edits,
        } => {
            let source = request.source.ok_or_else(|| {
                io::Error::other("An edit acknowledgement needs its source presentation")
            })?;
            apply_panel_edit(
                world,
                source,
                HostRequest::ApplyEditsWithResult {
                    token,
                    document,
                    revision,
                    edits,
                },
            )?;
            EditorRequestOutcome::Applied
        }
        HostRequest::ApplyModuleEditsWithResult {
            recipient,
            token,
            document,
            revision,
            edits,
        } => {
            let result = commit_documents(world, &HashSet::from([document])).and_then(|_| {
                world
                    .non_send_mut::<EditorSession>()
                    .apply_edits(document, revision, &edits)?;
                world
                    .non_send::<EditorSession>()
                    .document_revision(document)
            });
            world.write_message(EditorFeatureReply::Edit {
                recipient,
                token,
                result: result.as_ref().copied().map_err(ToString::to_string),
            });
            result?;
            EditorRequestOutcome::Applied
        }
        HostRequest::Save { document } => {
            let mut captured = captured_effect(
                world,
                request,
                EditorAction::Command(WindowCommand::Save),
                request.source,
                None,
            );
            captured.context.document = Some(document);
            return window_command(world, WindowCommand::Save, &captured, id);
        }
        HostRequest::CommitDocumentsWithResult {
            recipient,
            token,
            documents,
        } => {
            let result =
                commit_documents(world, &documents.iter().copied().collect()).and_then(|_| {
                    let session = world.non_send::<EditorSession>();
                    documents
                        .iter()
                        .map(|document| {
                            if session.save_pending(*document) {
                                return Err(io::Error::other("A document save is still pending"));
                            }
                            Ok(SavedDocument {
                                document: *document,
                                revision: session.document_revision(*document)?,
                            })
                        })
                        .collect::<io::Result<Vec<_>>>()
                });
            world.write_message(EditorFeatureReply::Save {
                recipient,
                token,
                result: result.as_ref().cloned().map_err(ToString::to_string),
            });
            EditorRequestOutcome::Saved { documents: result? }
        }
        HostRequest::SaveDocumentsWithResult {
            recipient,
            token,
            documents,
        } => return begin_save(world, documents, Some((recipient, token)), None, id),
        HostRequest::Undo { document } | HostRequest::Redo { document } => {
            commit_documents(world, &HashSet::from([document]))?;
            if matches!(host, HostRequest::Undo { .. }) {
                world
                    .non_send_mut::<EditorSession>()
                    .undo_document(document)?;
            } else {
                world
                    .non_send_mut::<EditorSession>()
                    .redo_document(document)?;
            }
            EditorRequestOutcome::Applied
        }
        HostRequest::SetSetting {
            plugin: _,
            key,
            value,
        } => {
            world.non_send_mut::<Settings>().settings[key] = value;
            world.non_send_mut::<Settings>().save_settings()?;
            world.non_send_mut::<Settings>().request_apply();
            EditorRequestOutcome::Applied
        }
        HostRequest::Notify { message } => {
            world.resource_mut::<EditorWorkspace>().error = Some(message);
            EditorRequestOutcome::Applied
        }
    };
    if let EditorRequestOutcome::Opened { panel, .. } = &outcome {
        place_opened_panel(world, *panel, Some(request.area), false, request.tab_index)?;
    }
    Ok(Some(outcome))
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
    let Some(handler) = world.get::<EditorPanelActions>(entity).copied() else {
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
        autosave: settings
            .autosave_enabled()
            .then(|| settings.autosave_delay()),
        monitoring: true,
        git: settings.bool("git_changed_lines", true),
        highlighting: settings.bool("treesitter", true),
        persistent_history: project,
        lsp_config: project.then(|| settings.config_dir.join("lsp.json")),
        lsp_config_mode: bed_document_session::LspConfigMode::Layered,
        lsp_file_observations: project,
    }
}

fn snapshot_layout(
    world: &mut World,
    panels: Vec<Entity>,
    layout: &WorkbenchLayout,
    navigation: &EditorNavigation,
) -> io::Result<Value> {
    let mut records = Vec::new();
    for entity in panels {
        let presentation = world.get::<PanelPresentation>(entity).unwrap().clone();
        let binding = world
            .get::<EditorPanelBinding>(entity)
            .cloned()
            .unwrap_or_default();
        let record = world.get::<PanelRecord>(entity).unwrap().clone();
        let active = panel_presentation(world, entity);
        if active
            .and_then(|entity| world.get::<bed_workbench_api::PanelPersistence>(entity))
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
            let retained = world
                .get::<EditorPresentations>(entity)
                .map(|presentations| presentations.retained.clone())
                .unwrap_or_default();
            let mut states = serde_json::Map::new();
            for (viewer, child) in retained {
                if let Ok(state) = bed_workbench::panel_state(world, child) {
                    states.insert(viewer, state);
                }
            }
            value["presentations"] = Value::Object(states);
            if let Some(path) = binding.input.local_file() {
                value["path"] = json!(path);
                value["backing"] = json!("local_file");
            } else if let Some(document) = binding.document {
                let snapshot = world.non_send::<EditorSession>().snapshot(document)?;
                if world
                    .non_send::<EditorSession>()
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
                    value["document_kind"] = json!(if snapshot.kind
                        == bed_document_session::DocumentKind::Bytes
                    {
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
    let value = layouts::snapshot(world)?;
    let spec = world.resource::<EditorWorkspace>().spec.clone();
    let hooks = world
        .query::<&EditorWorkspaceContribution>()
        .iter(world)
        .copied()
        .collect::<Vec<_>>();
    let mut state = {
        let persistence = world.non_send::<WorkspacePersistence>();
        let existing = if let Some(spec) = &spec {
            persistence.store.all_module_settings(spec)
        } else {
            persistence.store.all_standalone_module_settings()
        };
        existing
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default()
    };
    for hook in hooks {
        let value = world.run_system(hook.save).map_err(io::Error::other)?;
        state.insert(hook.owner.into(), value);
    }
    let mut persistence = world.non_send_mut::<WorkspacePersistence>();
    if persistence.last_value.as_ref() != Some(&value) {
        persistence
            .store
            .save_session_layout(spec.as_ref(), value.clone())?;
        persistence.last_value = Some(value);
    }
    if let Some(spec) = &spec {
        persistence
            .store
            .save_module_settings(spec, Value::Object(state))?;
    } else {
        persistence
            .store
            .save_standalone_module_settings(Value::Object(state))?;
    }
    persistence.last_saved = Instant::now();
    Ok(())
}

fn restore_contributions(world: &mut World) -> io::Result<()> {
    publish_editor_frame(world);
    let spec = world.resource::<EditorWorkspace>().spec.clone();
    let hooks = world
        .query::<&EditorWorkspaceContribution>()
        .iter(world)
        .copied()
        .collect::<Vec<_>>();
    for hook in hooks {
        let state = {
            let persistence = world.non_send::<WorkspacePersistence>();
            spec.as_ref()
                .and_then(|spec| persistence.store.module_settings(spec, hook.owner))
                .or_else(|| {
                    if spec.is_none() {
                        persistence.store.standalone_module_settings(hook.owner)
                    } else {
                        None
                    }
                })
                .cloned()
        };
        let workspace = world.resource::<EditorWorkspace>();
        let invocation = EditorWorkspaceRestore {
            workspace: world.non_send::<EditorSession>().workspace_id().0,
            root: workspace.project_root.clone(),
            state,
        };
        world
            .run_system_with(hook.restore, invocation)
            .map_err(io::Error::other)?
            .map_err(io::Error::other)?;
    }
    Ok(())
}

pub fn restore_workspace(world: &mut World) -> io::Result<()> {
    restore_contributions(world)?;
    let value = layouts::load(world);
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
        records.iter().position(|record| {
            record["panel_type"] == "bed.terminal.panel" || record["kind"] == "terminal"
        })
    });
    let mut seen = HashSet::new();
    let recovered_ids = records.iter().any(|record| {
        record["id"]
            .as_u64()
            .filter(|id| *id > 0 && *id <= u32::MAX as u64)
            .is_none_or(|id| !seen.insert(id))
    });
    if recovered_ids {
        world.resource_mut::<EditorWorkspace>().error = Some(
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
                world.resource_mut::<EditorWorkspace>().error = Some(format!(
                    "Saved layout was invalid; restored the contents in a fresh layout: {error}"
                ));
                None
            }
        }
    } else {
        TilingState::from_legacy_ini(
            value["ini"].as_str().unwrap_or(""),
            saved_ids.iter().copied(),
        )
        .ok()
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
            .map(str::to_owned)
            .or_else(|| {
                let legacy = record["kind"].as_str()?;
                world
                    .resource::<bed_editor_api::Registry>()
                    .panels()
                    .iter()
                    .find(|panel| panel.legacy_kind == Some(legacy))
                    .map(|panel| panel.id.into())
            })
            .unwrap_or_else(|| record["kind"].as_str().unwrap_or("unavailable").into());
        record["panel_type"] = json!(kind);
        let viewer = record["viewer"].as_str().map(str::to_owned);
        let descriptor = viewer
            .as_deref()
            .and_then(|viewer| world.resource::<bed_editor_api::Registry>().viewer(viewer))
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
                    bed_document_session::DocumentKind::Bytes
                } else {
                    descriptor
                        .as_ref()
                        .and_then(|viewer| viewer.document_kind())
                        .unwrap_or(bed_document_session::DocumentKind::Text)
                };
                match world
                    .non_send_mut::<EditorSession>()
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
                        world.resource_mut::<EditorWorkspace>().error = unavailable.clone();
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
        let binding = EditorPanelBinding {
            input,
            viewer,
            workspace: world.non_send::<EditorSession>().workspace_id().0,
            ..Default::default()
        };
        world
            .entity_mut(panel)
            .insert((binding.clone(), EditorPresentations::default()));
        let state = record
            .get("state")
            .cloned()
            .unwrap_or_else(|| record.clone());
        mount_presentation(world, panel, &kind, binding, state)?;
        // Preserve the original record for missing features and lazy inactive viewers.
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
            .resource::<EditorNavigation>()
            .last_document_panel
            .is_none())
        && active.is_some()
    {
        world.resource_mut::<EditorNavigation>().last_document_panel = active;
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
    if world.resource::<EditorWorkspace>().spec.is_none() {
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

fn begin_workspace(
    world: &mut World,
    spec: WorkspaceSpec,
    id: Option<EditorRequestId>,
) -> io::Result<Option<EditorRequestOutcome>> {
    if world
        .resource::<EditorWorkspace>()
        .spec
        .as_ref()
        .is_some_and(|current| current.identity() == spec.identity())
    {
        return Ok(Some(EditorRequestOutcome::Ignored));
    }
    if matches!(spec.target, WorkspaceTarget::Ssh { .. }) {
        start_connection(world, spec, false, id)?;
        return Ok(None);
    }
    if world.resource::<EditorWorkspace>().spec.is_none()
        && !world.non_send::<EditorSession>().is_remote()
    {
        attach_workspace(world, Path::new(&spec.root))?;
        return Ok(Some(EditorRequestOutcome::Applied));
    }
    let documents = world.non_send::<EditorSession>().document_ids();
    commit_documents(world, &documents.iter().copied().collect())?;
    let documents = documents
        .into_iter()
        .filter(|document| {
            world
                .non_send::<EditorSession>()
                .snapshot(*document)
                .is_ok_and(|snapshot| snapshot.dirty)
                && !world
                    .non_send::<EditorSession>()
                    .is_snapshot_document(*document)
        })
        .collect();
    begin_save(
        world,
        documents,
        None,
        Some(Continuation::Workspace(spec)),
        id,
    )
}

pub fn attach_workspace(world: &mut World, path: &Path) -> io::Result<()> {
    attach_workspace_with_terminal(world, path, None)
}

fn validate_terminal_promotion(world: &World, caller: Entity, terminal: u64) -> io::Result<()> {
    if world.resource::<EditorWorkspace>().spec.is_some()
        || world.non_send::<EditorSession>().is_remote()
    {
        return Err(io::Error::other(
            "Promotion requires a standalone local session",
        ));
    }
    if !bed_workbench::owns_panel(world, caller)
        || world
            .get::<EditorPanelBinding>(caller)
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
    request: &EditorRequest,
    id: Option<EditorRequestId>,
) -> io::Result<Option<EditorRequestOutcome>> {
    let caller = request.panel.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "The calling terminal is unavailable",
        )
    })?;
    let terminal = world
        .get::<EditorPanelBinding>(caller)
        .and_then(|binding| binding.terminal)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "The calling terminal is unavailable",
            )
        })?;
    let root = std::fs::canonicalize(root)?;
    if !root.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Workspace must be a directory",
        ));
    }
    if !bed_workbench::owns_panel(world, caller) || world.non_send::<EditorSession>().is_remote() {
        return Err(io::Error::other(
            "Workspace promotion requires a live local terminal",
        ));
    }
    if let Some(current) = &world.resource::<EditorWorkspace>().spec {
        if Path::new(&current.root) == root {
            return Ok(Some(EditorRequestOutcome::Ignored));
        }
        world
            .resource_mut::<EditorWorkspace>()
            .windows
            .push(WorkspaceSpec::local(root.to_string_lossy()));
        return Ok(Some(EditorRequestOutcome::Applied));
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

fn open_terminal_companions(
    world: &mut World,
    paths: Vec<PathBuf>,
    request: &EditorRequest,
) -> io::Result<EditorRequestOutcome> {
    let caller = request
        .panel
        .filter(|caller| {
            bed_workbench::owns_panel(world, *caller)
                && world
                    .get::<EditorPanelBinding>(*caller)
                    .is_some_and(|binding| binding.terminal.is_some())
        })
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "The calling terminal is unavailable",
            )
        })?;
    if world.non_send::<EditorSession>().is_remote() {
        return Err(io::Error::other(
            "Local shell companions require a local workspace",
        ));
    }
    let session = world.non_send::<EditorSession>().workspace_id().0;
    if world.resource::<TerminalCompanions>().session != session {
        *world.resource_mut::<TerminalCompanions>() = TerminalCompanions {
            session,
            ..Default::default()
        };
    }
    let mut outcome = EditorRequestOutcome::Ignored;
    for path in paths {
        let path = std::fs::canonicalize(path)?;
        if !path.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Shell companions require a file",
            ));
        }
        let existing = world
            .resource::<TerminalCompanions>()
            .views
            .get(&(caller, path.clone()))
            .copied()
            .filter(|panel| {
                bed_workbench::owns_panel(world, *panel)
                    && bed_workbench_api::panel_active(world, *panel)
            });
        if let Some(panel) = existing {
            focus_panel(world, panel, true)?;
            outcome = EditorRequestOutcome::Opened {
                panel,
                document: world
                    .get::<EditorPanelBinding>(panel)
                    .and_then(|binding| binding.document),
            };
            continue;
        }
        let anchor = world
            .resource::<TerminalCompanions>()
            .views
            .iter()
            .filter_map(|((shell, _), panel)| {
                (*shell == caller
                    && bed_workbench::owns_panel(world, *panel)
                    && bed_workbench_api::panel_active(world, *panel))
                .then(|| {
                    world
                        .get::<PanelPresentation>(*panel)
                        .map(|presentation| (presentation.id, *panel))
                })
                .flatten()
            })
            .min_by_key(|(id, _)| *id)
            .map(|(_, panel)| panel);
        let anchor = anchor.unwrap_or(caller);
        let anchor_id = world.get::<PanelPresentation>(anchor).unwrap().id;
        let mut tiling = world.resource::<WorkbenchLayout>().tiling.clone();
        let area = tiling
            .area_for(anchor_id)
            .expect("a live shell companion belongs to an area");
        let panel = open_file(world, &path, None, true, Some(area))?;
        if anchor == caller {
            let rect = tiling.layout.area(area).rect;
            if let Some(destination) =
                tiling
                    .layout
                    .split(area, 0, (rect.min[0] + rect.max[0]) / 2, 100)
            {
                tiling.add_area(destination);
                tiling.assign(
                    world.get::<PanelPresentation>(panel).unwrap().id,
                    destination,
                );
                world.resource_mut::<WorkbenchLayout>().tiling = tiling;
            }
        }
        world
            .resource_mut::<TerminalCompanions>()
            .views
            .insert((caller, path), panel);
        focus_panel(world, panel, true)?;
        outcome = EditorRequestOutcome::Opened {
            panel,
            document: world
                .get::<EditorPanelBinding>(panel)
                .and_then(|binding| binding.document),
        };
    }
    Ok(outcome)
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
    if world.resource::<EditorWorkspace>().spec.is_some()
        || world.non_send::<EditorSession>().is_remote()
    {
        return Err(io::Error::other(
            "Attachment requires a standalone local session",
        ));
    }
    let documents = world
        .non_send::<EditorSession>()
        .document_ids()
        .into_iter()
        .collect();
    commit_documents(world, &documents)?;
    persist_workspace(world)?;
    let spec = world
        .non_send_mut::<WorkspacePersistence>()
        .store
        .record_workspace(WorkspaceSpec::local(root.to_string_lossy()))?;
    let options = session_options(world, Some(root.clone()));
    world.non_send_mut::<EditorSession>().configure(options)?;
    {
        let mut workspace = world.resource_mut::<EditorWorkspace>();
        workspace.spec = Some(spec);
        workspace.project_root = root.to_string_lossy().into_owned();
        workspace.working_directory = root;
    }
    restore_contributions(world)?;
    let launchers = panel_entities(world)
        .into_iter()
        .filter(|entity| {
            world
                .get::<PanelRecord>(*entity)
                .is_some_and(|record| record.panel_type == "bed.projects.panel")
        })
        .collect::<Vec<_>>();
    retire_panels(world, &launchers)?;
    layouts::attach(world, terminal)?;
    publish_editor_frame(world);
    Ok(())
}

pub fn replace_workspace(world: &mut World, spec: WorkspaceSpec) -> io::Result<()> {
    if matches!(spec.target, WorkspaceTarget::Ssh { .. }) {
        start_connection(world, spec, false, None)?;
        return Ok(());
    }
    let root = std::fs::canonicalize(&spec.root)?;
    let options = session_options(world, Some(root.clone()));
    let session = EditorSession::with_options(options)?;
    if world.resource::<EditorInitialized>().0 {
        persist_workspace(world)?;
    }
    let panels = panel_entities(world);
    retire_panels(world, &panels)?;
    world
        .non_send_mut::<EditorSession>()
        .shutdown(ClosePolicy::Discard)?;
    world.insert_non_send(session);
    let spec = world
        .non_send_mut::<WorkspacePersistence>()
        .store
        .record_workspace(spec)?;
    world.insert_resource(EditorSessionId(
        world.non_send::<EditorSession>().workspace_id().0,
    ));
    *world.resource_mut::<EditorNavigation>() = EditorNavigation::default();
    *world.resource_mut::<WorkbenchLayout>() = WorkbenchLayout::default();
    layouts::clear(world);
    {
        let mut workspace = world.resource_mut::<EditorWorkspace>();
        workspace.project_root = spec.root.clone();
        workspace.working_directory = root;
        workspace.spec = Some(spec);
    }
    cancel_foreign_pending(world);
    restore_workspace(world)
}

fn cancel_foreign_pending(world: &mut World) {
    format_save::cancel(world);
    let pending = std::mem::take(&mut world.resource_mut::<PendingOperations>().saves);
    for save in pending {
        let error = "The workspace changed before this operation completed".to_owned();
        if let Some((recipient, token)) = save.reply {
            world.write_message(EditorFeatureReply::Save {
                recipient,
                token,
                result: Err(error.clone()),
            });
        }
        complete(world, save.id, Err(error.into()));
    }
}

pub fn application_commands(world: &World) -> Vec<crate::commands::CommandItem> {
    let registry = world.resource::<bed_editor_api::Registry>();
    let facts = world.get_resource::<bed_editor_api::CommandPresentations>();
    registry
        .menus()
        .iter()
        .filter(|menu| menu.slot == bed_editor_api::MenuSlot::Application)
        .filter_map(|menu| registry.command(menu.command))
        .filter_map(|command| {
            let fact = facts.and_then(|facts| facts.0.get(command.id));
            if fact.is_some_and(|fact| !fact.visible) {
                return None;
            }
            Some(crate::commands::CommandItem {
                id: command.id.into(),
                label: fact
                    .and_then(|fact| fact.label.clone())
                    .unwrap_or_else(|| command.label.into()),
                icon: command.icon.map(str::to_owned),
                enabled: fact.is_none_or(|fact| fact.enabled),
            })
        })
        .collect()
}
pub fn toolbar_commands(world: &World) -> Vec<crate::commands::CommandItem> {
    let registry = world.resource::<bed_editor_api::Registry>();
    let facts = world.get_resource::<bed_editor_api::CommandPresentations>();
    registry
        .toolbar()
        .iter()
        .filter_map(|item| registry.command(item.command))
        .filter_map(|command| {
            let fact = facts.and_then(|facts| facts.0.get(command.id));
            if fact.is_some_and(|fact| !fact.visible) {
                return None;
            }
            Some(crate::commands::CommandItem {
                id: command.id.into(),
                label: fact
                    .and_then(|fact| fact.label.clone())
                    .unwrap_or_else(|| command.label.into()),
                icon: command.icon.map(str::to_owned),
                enabled: fact.is_none_or(|fact| fact.enabled),
            })
        })
        .collect()
}

struct ConnectedWorkspace {
    spec: WorkspaceSpec,
    target: bed_remote::SshTarget,
    client: bed_remote::RemoteClient,
}
struct Connection {
    session: u64,
    id: Option<EditorRequestId>,
    reconnect: bool,
    receiver: std::sync::mpsc::Receiver<io::Result<ConnectedWorkspace>>,
}
struct RemoteOpen {
    session: u64,
    id: Option<EditorRequestId>,
    path: PathBuf,
    viewer: Option<String>,
    additional: bool,
    mode: FileOpenMode,
    layout_slot: LayoutSlot,
    move_existing: bool,
    area: Option<u32>,
    tab_index: Option<usize>,
    restore: Option<Entity>,
    follow_up: Option<EditorRequest>,
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
    id: Option<EditorRequestId>,
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
        session: world.non_send::<EditorSession>().workspace_id().0,
        id,
        reconnect,
        receiver,
    });
    Ok(())
}
fn reconnect_workspace(world: &mut World, id: Option<EditorRequestId>) -> io::Result<()> {
    let spec = world
        .resource::<EditorWorkspace>()
        .spec
        .clone()
        .ok_or_else(|| io::Error::other("No remote workspace is open"))?;
    start_connection(world, spec, true, id)
}
fn replace_connected_workspace(world: &mut World, connected: ConnectedWorkspace) -> io::Result<()> {
    let options = session_options(world, Some(PathBuf::from(&connected.spec.root)));
    let session = EditorSession::with_remote_options(
        options,
        connected.target,
        connected.client,
        connected.spec.root.clone(),
    )?;
    if world.resource::<EditorInitialized>().0 {
        persist_workspace(world)?;
    }
    let panels = panel_entities(world);
    retire_panels(world, &panels)?;
    world
        .non_send_mut::<EditorSession>()
        .shutdown(ClosePolicy::Discard)?;
    world.insert_non_send(session);
    let spec = world
        .non_send_mut::<WorkspacePersistence>()
        .store
        .record_workspace(connected.spec)?;
    {
        let mut workspace = world.resource_mut::<EditorWorkspace>();
        workspace.project_root = spec.root.clone();
        workspace.spec = Some(spec);
    }
    world.insert_resource(EditorSessionId(
        world.non_send::<EditorSession>().workspace_id().0,
    ));
    *world.resource_mut::<EditorNavigation>() = EditorNavigation::default();
    *world.resource_mut::<WorkbenchLayout>() = WorkbenchLayout::default();
    layouts::clear(world);
    cancel_foreign_pending(world);
    restore_workspace(world)
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
fn begin_open_file(
    world: &mut World,
    path: &Path,
    viewer: Option<&str>,
    additional: bool,
    move_existing: bool,
    area: Option<u32>,
    tab_index: Option<usize>,
    id: Option<EditorRequestId>,
    mode: FileOpenMode,
) -> io::Result<Option<EditorRequestOutcome>> {
    match open_file_with_origin(world, path, viewer, additional, area, None, mode) {
        Ok(panel) => {
            place_opened_panel(world, panel, area, move_existing, tab_index)?;
            Ok(Some(EditorRequestOutcome::Opened {
                panel,
                document: world
                    .get::<EditorPanelBinding>(panel)
                    .and_then(|binding| binding.document),
            }))
        }
        Err(error)
            if error.kind() == io::ErrorKind::WouldBlock
                && world.non_send::<EditorSession>().is_remote() =>
        {
            let session = world.non_send::<EditorSession>().workspace_id().0;
            let layout_slot = world.resource::<LayoutSlots>().active();
            world
                .non_send_mut::<RemoteOperations>()
                .opens
                .push(RemoteOpen {
                    session,
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
    let session = world.non_send::<EditorSession>().workspace_id().0;
    let viewer = record["viewer"].as_str().map(str::to_owned);
    world.entity_mut(panel).insert((
        EditorPanelBinding {
            workspace: session,
            viewer: viewer.clone(),
            ..Default::default()
        },
        EditorPresentations::default(),
    ));
    world.get_mut::<PanelRecord>(panel).unwrap().unavailable =
        Some("Opening remote document…".into());
    let layout_slot = world.resource::<LayoutSlots>().active();
    world
        .non_send_mut::<RemoteOperations>()
        .opens
        .push(RemoteOpen {
            session,
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
    errors: &[bed_document_session::ServiceError],
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
                    if connection.session != world.non_send::<EditorSession>().workspace_id().0 {
                        return Ok(Some(EditorRequestOutcome::Ignored));
                    }
                    let connected = result?;
                    if connection.reconnect {
                        world
                            .non_send_mut::<EditorSession>()
                            .reconnect_remote_with_target(connected.client, connected.target)?;
                        return Ok(Some(EditorRequestOutcome::Applied));
                    }
                    let documents = world.non_send::<EditorSession>().document_ids();
                    commit_documents(world, &documents.iter().copied().collect())?;
                    let documents = documents
                        .into_iter()
                        .filter(|document| {
                            world
                                .non_send::<EditorSession>()
                                .snapshot(*document)
                                .is_ok_and(|snapshot| snapshot.dirty)
                                && !world
                                    .non_send::<EditorSession>()
                                    .is_snapshot_document(*document)
                        })
                        .collect();
                    begin_save(
                        world,
                        documents,
                        None,
                        Some(Continuation::Connected(Box::new(connected))),
                        connection.id,
                    )
                })();
                match result {
                    Ok(Some(outcome)) => complete(world, connection.id, Ok(outcome)),
                    Ok(None) => {}
                    Err(error) => complete(world, connection.id, Err(error.into())),
                }
            }
        }
    }
    let opens = std::mem::take(&mut world.non_send_mut::<RemoteOperations>().opens);
    for mut pending in opens {
        if pending.session != world.non_send::<EditorSession>().workspace_id().0 {
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
            complete(world, pending.id, Ok(EditorRequestOutcome::Ignored));
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
        if pending.follow_up.as_ref().is_some_and(|request|matches!(&request.action,EditorAction::HostRequest(HostRequest::NavigateLocation{origin:Some(origin),..}) if !world.non_send::<EditorSession>().accepts_lsp_origin(origin))) {world.non_send_mut::<RemoteOperations>().abandoned_paths.insert(pending.path.clone());complete(world,pending.id,Ok(EditorRequestOutcome::Ignored));continue;}
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
                bed_document_session::DocumentKind::Bytes
            } else {
                pending
                    .viewer
                    .as_deref()
                    .and_then(|viewer| world.resource::<bed_editor_api::Registry>().viewer(viewer))
                    .and_then(|viewer| viewer.document_kind())
                    .unwrap_or(bed_document_session::DocumentKind::Text)
            };
            let document = world
                .non_send_mut::<EditorSession>()
                .open_file_with_kind(&pending.path, kind);
            document.and_then(|document| {
                world.get_mut::<PanelRecord>(panel).unwrap().unavailable = None;
                let binding = EditorPanelBinding {
                    input: PanelInput::Document(document),
                    document: Some(document),
                    viewer: pending.viewer.clone(),
                    workspace: pending.session,
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
                        || world.resource::<EditorNavigation>().last_document_panel == Some(panel))
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
                    Ok(EditorRequestOutcome::Opened {
                        panel,
                        document: world
                            .get::<EditorPanelBinding>(panel)
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
        if world.non_send::<EditorSession>().open_pending(&path) {
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
        if let Some(document) = world.non_send::<EditorSession>().document_for_path(&path)
            && world.non_send::<EditorSession>().view_count(document) == 0
            && world
                .non_send::<EditorSession>()
                .snapshot(document)
                .is_ok_and(|snapshot| !snapshot.dirty)
            && !world
                .query::<&EditorPanelBinding>()
                .iter(world)
                .any(|binding| binding.document == Some(document))
        {
            world
                .non_send_mut::<EditorSession>()
                .close_document(document, ClosePolicy::Discard)?;
        }
    }
    Ok(())
}

#[cfg(test)]
fn install_test_infrastructure(app: &mut App, config: &Path) {
    let settings = Settings::with_paths(
        config.to_owned(),
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."),
    )
    .unwrap();
    app.insert_non_send(bed_ui::icons::Icons::default())
        .insert_non_send(WorkspacePersistence::new(
            WorkspaceStore::load(config).unwrap(),
        ))
        .insert_non_send(settings)
        .insert_resource(RemoteHelperDirectory(PathBuf::new()));
}

#[cfg(test)]
#[path = "editor_terminal_tests.rs"]
mod terminal_tests;
#[cfg(test)]
#[path = "editor_runtime_tests.rs"]
mod tests;
#[cfg(test)]
#[path = "editor_transaction_tests.rs"]
mod transaction_tests;
