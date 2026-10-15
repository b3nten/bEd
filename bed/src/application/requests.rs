//! Request adapters used by bEd regression fixtures and automation.
//! All editing, saves, closes and context transitions execute in bedtime-core.
#![allow(dead_code, unused_imports)]
use bed::application::runtime::{
    LayoutCommand, LayoutRequestId, submit_layout_command, take_layout_result,
};
use bed::application::{
    AppOptions, AppRuntimePlugin, ApplicationState, LayoutSlot, LayoutSlots, WorkspacePersistence,
    WorkspaceSpec, WorkspaceStore, WorkspaceTarget, publish_app_state,
};
use bedtime_api::{
    documents::{CommandContext, PanelBinding, PanelInput, Revision, SavedDocument},
    features::HostAction,
    workbench::{CommandInvocation, PanelAction, PanelTarget, WorkbenchAction},
};
use bedtime_core::DocumentNavigation;
use bedtime_documents::{ByteEdit, DocumentId, DocumentSession};
use bedtime_plugin_workspaces::{WorkspaceAction, WorkspaceRepositoryResource};
use bedtime_workbench::{
    PanelPresentation, WorkbenchLayout, WorkbenchOutcome, WorkbenchRequestId, WorkbenchRequests,
};
use bevy::prelude::*;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    io,
    path::{Path, PathBuf},
};

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

pub fn panel_presentation(world: &World, panel: Entity) -> Option<Entity> {
    bedtime_core::panel_presentation(world, panel)
}

#[derive(Debug)]
pub enum AppAction {
    Workspace(WorkspaceAction),
    WorkspaceWithBarrier {
        action: WorkspaceAction,
        token: bedtime_api::features::OperationBarrierToken,
    },
    Settings(bedtime_api::settings::SettingsRequest),
    Command(WindowCommand),
    PluginCommand(bedtime_api::CommandId),
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
        event: bedtime_api::workbench::ExternalFileDrag,
    },
    FocusPanel(Entity),
    ObserveFocus(Entity),
    FocusArea(u32),
    ClosePanel(Entity),
    ClosePanels(Vec<Entity>),
    OpenFile(PathBuf),
    OpenFileWithViewer {
        path: PathBuf,
        viewer: Option<bedtime_api::ViewerId>,
        additional: bool,
        move_existing: bool,
    },
    OpenViewer {
        panel: Entity,
        viewer: bedtime_api::ViewerId,
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
    pub layout_epoch: u64,
    pub source: Option<Entity>,
    pub panel: Option<Entity>,
    pub area: u32,
    pub explicit_area: bool,
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
    let panel = source.and_then(|source| bedtime_api::workbench::panel_owner(world, source));
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
        && let Ok(captured) = bedtime_api::documents::selection_context(session, view)
    {
        context = captured;
    }
    if context.document.is_none() {
        let model = source
            .and_then(|entity| world.get::<bedtime_core::DocumentBinding>(entity))
            .or_else(|| {
                presentation.and_then(|entity| world.get::<bedtime_core::DocumentBinding>(entity))
            })
            .or_else(|| {
                panel.and_then(|entity| world.get::<bedtime_core::DocumentBinding>(entity))
            });
        if let Some(model) = model {
            context.document = Some(model.0);
            context.revision = bedtime_core::document_revision(world, model.0).ok();
            context.path = bedtime_core::document_path(world, model.0)
                .ok()
                .flatten()
                .map(|path| path.to_string_lossy().into_owned());
        }
    }
    let remembers_document = match &action {
        AppAction::Command(WindowCommand::Save | WindowCommand::SaveAs) => true,
        AppAction::PluginCommand(command) => matches!(
            command.as_str(),
            "bedtime.documents.save" | "bedtime.documents.save_as"
        ),
        AppAction::WorkbenchAction(WorkbenchAction::Invoke(invocation)) => matches!(
            invocation.command.as_str(),
            "bedtime.documents.save" | "bedtime.documents.save_as"
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
        layout_epoch: world
            .resource::<bedtime_workbench::WorkbenchLayoutEpoch>()
            .0,
        source,
        panel,
        area,
        explicit_area: target == PanelTarget::LastFocused,
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

/// Compatibility receipts refer to the owning crate's operation; this resource
/// does not queue or execute editing/workbench requests.
#[derive(Resource, Default)]
pub struct AppRequests {
    next_id: u64,
    receipts: HashMap<AppRequestId, Receipt>,
    results: HashMap<AppRequestId, Result<AppRequestOutcome, AppRequestError>>,
}
enum Receipt {
    Host(bedtime_core::HostRequestId),
    Workbench(WorkbenchRequestId, Option<Vec<Entity>>),
    Workspace(bedtime_plugin_workspaces::WorkspaceRequestId),
    Settings(bedtime_api::settings::SettingsRequestId),
    Layout(LayoutRequestId),
}
impl AppRequests {
    pub fn is_empty(&self) -> bool {
        self.receipts.is_empty()
    }
    pub fn take_result(&mut self, id: AppRequestId) -> Option<Result<AppRequestOutcome, String>> {
        self.results
            .remove(&id)
            .map(|result| result.map_err(|error| error.message))
    }
}

pub fn process_app_requests(world: &mut World) -> io::Result<()> {
    bedtime_core::resolve_documents(world);
    bedtime_plugin_workspaces::resolve_workspace_requests(world);
    bedtime_core::resolve_documents(world);
    bedtime_plugin_workspaces::resolve_workspace_requests(world);
    bedtime_core::resolve_documents(world);
    bedtime_plugin_layouts::poll::<LayoutSlot, bedtime_core::layouts::DocumentLayouts>(world);
    bed::application::runtime::update_application_policy(world);
    Ok(())
}
pub fn tick_app_services(world: &mut World) -> io::Result<()> {
    bedtime_core::poll_document_services(world);
    process_app_requests(world)
}
pub fn wait_app_request(world: &mut World, id: AppRequestId) -> io::Result<AppRequestOutcome> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        tick_app_services(world)?;
        if let Some(result) = poll_app_request_result(world, id) {
            return result.map_err(|error| io::Error::new(error.kind, error.message));
        }
        if std::time::Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Application request did not complete",
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}
pub fn configure_feature_targets(world: &mut World) {
    bedtime_core::publish_documents(world);
    publish_app_state(world);
}
pub fn drain_feature_intents(world: &mut World) {
    for intent in world
        .resource_mut::<bedtime_api::features::HostEffects>()
        .take()
    {
        let request = bedtime_core::capture_host_request_area(
            world,
            intent.source,
            intent.action,
            intent.area,
        )
        .with_tab_index(intent.tab_index);
        bedtime_core::submit_captured_host_request(world, request);
    }
}
pub fn draw_ui(world: &mut World, ui: &dear_imgui_rs::Ui) -> io::Result<()> {
    process_app_requests(world)?;
    bedtime_core::publish_documents(world);
    publish_app_state(world);
    bedtime_core::draw_workbench(world, ui);
    Ok(())
}

fn captured_host(
    _world: &World,
    request: &AppRequest,
    action: HostAction,
) -> bedtime_core::HostRequest {
    bedtime_core::HostRequest::capture(
        bedtime_documents::SessionId(request.session_id),
        request.source,
        action,
        request.area,
        request.layout_epoch,
    )
    .with_explicit_area(request.explicit_area)
    .with_tab_index(request.tab_index)
}

pub fn submit_app_request(world: &mut World, mut request: AppRequest) -> AppRequestId {
    world.init_resource::<AppRequests>();
    let id = {
        let mut requests = world.resource_mut::<AppRequests>();
        requests.next_id += 1;
        AppRequestId(requests.next_id)
    };
    let result = (|| -> io::Result<Option<Receipt>> {
        if request.session_id != world.non_send::<DocumentSession>().session_id().0 {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "The request belongs to a replaced document session",
            ));
        }
        let layout_scoped = match &request.action {
            AppAction::HostAction(action) => action.is_layout_scoped(),
            AppAction::Command(
                WindowCommand::UseLayout(_)
                | WindowCommand::SetLayoutDefault(_)
                | WindowCommand::RestoreBuiltinLayout(_)
                | WindowCommand::Save
                | WindowCommand::SaveAs
                | WindowCommand::Quit,
            ) => false,
            AppAction::ApplyEdits { .. }
            | AppAction::SaveDocuments(_)
            | AppAction::SaveAs { .. }
            | AppAction::Settings(_)
            | AppAction::Workspace(_)
            | AppAction::WorkspaceWithBarrier { .. }
            | AppAction::SetWorkspace(_)
            | AppAction::AttachWorkspace(_) => false,
            _ => true,
        };
        if layout_scoped
            && (request.layout_epoch
                != world
                    .resource::<bedtime_workbench::WorkbenchLayoutEpoch>()
                    .0
                || request
                    .source
                    .is_some_and(|source| !bedtime_api::workbench::panel_active(world, source)))
        {
            complete(world, Some(id), Ok(AppRequestOutcome::Ignored));
            return Ok(None);
        }
        let action = std::mem::replace(
            &mut request.action,
            AppAction::HostActions {
                actions: Vec::new(),
            },
        );
        let host = match action {
            AppAction::HostAction(action) => Some(action),
            AppAction::HostActions { actions } => {
                let effects = actions
                    .into_iter()
                    .map(|action| captured_host(world, &request, action))
                    .collect();
                return Ok(Some(Receipt::Host(bedtime_core::submit_host_batch(
                    world, effects,
                ))));
            }
            AppAction::ApplyEdits {
                document,
                revision,
                edits,
            } => Some(HostAction::ApplyEdits {
                document,
                revision,
                edits,
            }),
            AppAction::SaveDocuments(documents) => Some(HostAction::SaveDocuments { documents }),
            AppAction::SaveAs { document, path } => Some(HostAction::SaveAs { document, path }),
            AppAction::PanelAction { panel, action } => {
                Some(HostAction::PanelAction { panel, action })
            }
            AppAction::ExternalFiles { panel, event } => {
                Some(HostAction::ExternalFiles { panel, event })
            }
            AppAction::OpenFile(path) => Some(HostAction::OpenFile {
                path: path.to_string_lossy().into_owned(),
                viewer: None,
            }),
            AppAction::OpenFileWithViewer {
                path,
                viewer,
                additional,
                move_existing,
            } => Some(if move_existing {
                HostAction::DropFile {
                    path: path.to_string_lossy().into_owned(),
                }
            } else {
                HostAction::OpenFileInstance {
                    path: path.to_string_lossy().into_owned(),
                    viewer,
                    additional,
                }
            }),
            AppAction::OpenViewer { panel, viewer } => {
                let document = world
                    .get::<PanelBinding>(panel)
                    .and_then(|binding| binding.document)
                    .ok_or_else(|| io::Error::other("Panel has no document"))?;
                let tab = world
                    .get::<bedtime_workbench::PanelPresentation>(panel)
                    .ok_or_else(|| io::Error::other("Panel unavailable"))?
                    .id;
                Some(HostAction::OpenDocumentViewer {
                    tab,
                    document,
                    viewer,
                })
            }
            AppAction::PluginCommand(command) => {
                return Ok(Some(invoke(
                    world,
                    &request,
                    command.into(),
                    request.arguments.clone(),
                )));
            }
            AppAction::WorkbenchAction(mut action) => {
                if let WorkbenchAction::Invoke(invocation) = &mut action
                    && invocation.arguments.is_null()
                {
                    invocation.arguments = bedtime_core::command_arguments(&request.context);
                }
                return Ok(Some(submit_workbench(world, action, request.tab_index)));
            }
            AppAction::FocusPanel(panel) => {
                return Ok(Some(submit_workbench(
                    world,
                    WorkbenchAction::FocusPanel(panel),
                    None,
                )));
            }
            AppAction::ObserveFocus(panel) => {
                return Ok(Some(submit_workbench(
                    world,
                    WorkbenchAction::ObserveFocus(panel),
                    None,
                )));
            }
            AppAction::FocusArea(area) => {
                return Ok(Some(submit_workbench(
                    world,
                    WorkbenchAction::FocusArea(area),
                    None,
                )));
            }
            AppAction::ClosePanel(panel) => {
                return Ok(Some(submit_workbench(
                    world,
                    WorkbenchAction::ClosePanel(panel),
                    None,
                )));
            }
            AppAction::ClosePanels(panels) => {
                let mut tiling = world.resource::<WorkbenchLayout>().tiling.clone();
                let ids = panel_entities(world)
                    .into_iter()
                    .filter(|panel| !panels.contains(panel))
                    .map(|panel| {
                        world
                            .get::<bedtime_workbench::PanelPresentation>(panel)
                            .unwrap()
                            .id
                    })
                    .collect::<Vec<_>>();
                tiling.retain_panels(ids.clone());
                let value = tiling.to_value(ids);
                let receipt =
                    world
                        .resource_mut::<WorkbenchRequests>()
                        .submit(WorkbenchAction::SetLayout {
                            tiling: value,
                            close: panels.clone(),
                        });
                return Ok(Some(Receipt::Workbench(receipt, Some(panels))));
            }
            AppAction::Workspace(action) | AppAction::WorkspaceWithBarrier { action, .. } => {
                return workspace_receipt(world, &request, action).map(Some);
            }
            AppAction::AttachWorkspace(path) => {
                return workspace_receipt(
                    world,
                    &request,
                    WorkspaceAction::OpenLocalWorkspace(path),
                )
                .map(Some);
            }
            AppAction::SetWorkspace(spec) => {
                if matches!(spec.target, WorkspaceTarget::Local)
                    && world
                        .resource::<WorkspaceRepositoryResource>()
                        .0
                        .find(&spec)?
                        .is_none()
                {
                    Some(HostAction::OpenDirectory {
                        location: bedtime_files::FileLocation {
                            provider: bedtime_files::FileProviderId::Local,
                            path: PathBuf::from(spec.root),
                        },
                    })
                } else {
                    return workspace_receipt(
                        world,
                        &request,
                        WorkspaceAction::OpenWorkspace(spec),
                    )
                    .map(Some);
                }
            }
            AppAction::Settings(mut settings) => {
                let receipt = settings
                    .receipt
                    .unwrap_or(bedtime_api::settings::SettingsRequestId(settings.sequence));
                settings.receipt = Some(receipt);
                world.write_message(settings);
                return Ok(Some(Receipt::Settings(receipt)));
            }
            AppAction::Command(command) => {
                return window_request(world, &request, command, Some(id));
            }
        };
        Ok(host.map(|action| {
            let captured = captured_host(world, &request, action);
            Receipt::Host(bedtime_core::submit_captured_host_request(world, captured))
        }))
    })();
    match result {
        Ok(Some(receipt)) => {
            world
                .resource_mut::<AppRequests>()
                .receipts
                .insert(id, receipt);
        }
        Ok(None) => {}
        Err(error) => complete(world, Some(id), Err(error.into())),
    }
    id
}

fn submit_workbench(
    world: &mut World,
    action: WorkbenchAction,
    tab_index: Option<usize>,
) -> Receipt {
    let closed = match &action {
        WorkbenchAction::ClosePanel(panel) => Some(vec![*panel]),
        _ => None,
    };
    let receipt = world
        .resource_mut::<WorkbenchRequests>()
        .submit_with_tab_index(action, tab_index);
    Receipt::Workbench(receipt, closed)
}
fn invoke(
    world: &mut World,
    request: &AppRequest,
    command: bedtime_api::CommandId,
    mut arguments: Value,
) -> Receipt {
    if arguments.is_null() {
        arguments = bedtime_core::command_arguments(&request.context);
    }
    submit_workbench(
        world,
        WorkbenchAction::Invoke(CommandInvocation {
            command,
            source: request.source,
            area: request.area,
            arguments,
        }),
        request.tab_index,
    )
}
fn workspace_receipt(
    world: &mut World,
    request: &AppRequest,
    action: WorkspaceAction,
) -> io::Result<Receipt> {
    Ok(Receipt::Workspace(
        bedtime_plugin_workspaces::submit_workspace_request(
            world,
            action,
            request.source,
            request.area,
        ),
    ))
}

fn window_request(
    world: &mut World,
    request: &AppRequest,
    command: WindowCommand,
    id: Option<AppRequestId>,
) -> io::Result<Option<Receipt>> {
    match command {
        command @ (WindowCommand::UseLayout(_)
        | WindowCommand::SetCurrentLayout
        | WindowCommand::SetLayoutDefault(_)
        | WindowCommand::RestoreBuiltinLayout(_)
        | WindowCommand::SaveDefaultLayout
        | WindowCommand::ResetDefaultLayout
        | WindowCommand::ResetLayout) => {
            let command = match command {
                WindowCommand::UseLayout(slot) => LayoutCommand::UseLayout(slot),
                WindowCommand::SetCurrentLayout => LayoutCommand::SetCurrentLayout,
                WindowCommand::SetLayoutDefault(slot) => LayoutCommand::SetLayoutDefault(slot),
                WindowCommand::RestoreBuiltinLayout(slot) => {
                    LayoutCommand::RestoreBuiltinLayout(slot)
                }
                WindowCommand::SaveDefaultLayout => LayoutCommand::SaveDefaultLayout,
                WindowCommand::ResetDefaultLayout => LayoutCommand::ResetDefaultLayout,
                WindowCommand::ResetLayout => LayoutCommand::ResetLayout,
                _ => unreachable!(),
            };
            Ok(Some(Receipt::Layout(submit_layout_command(world, command))))
        }
        WindowCommand::OpenFolder => {
            workspace_receipt(world, request, WorkspaceAction::OpenFolder).map(Some)
        }
        WindowCommand::Quit => Ok(Some(Receipt::Host(bedtime_core::submit_quit(world)))),
        WindowCommand::Save | WindowCommand::SaveAs => {
            let Some(document) = request.context.document else {
                complete(world, id, Ok(AppRequestOutcome::Ignored));
                return Ok(None);
            };
            let action = if command == WindowCommand::SaveAs {
                HostAction::SaveAsDialog { document }
            } else {
                HostAction::Save { document }
            };
            let captured = captured_host(world, request, action);
            Ok(Some(Receipt::Host(
                bedtime_core::submit_captured_host_request(world, captured),
            )))
        }
        command => {
            let canonical = match command {
                WindowCommand::NewDocument => "bedtime.documents.new",
                WindowCommand::OpenFile => "bedtime.documents.open_file",
                WindowCommand::Save => "bedtime.documents.save",
                WindowCommand::SaveAs => "bedtime.documents.save_as",
                WindowCommand::Close => "bedtime.core.close",
                WindowCommand::SplitRight => "bedtime.workbench.split_right",
                WindowCommand::SplitDown => "bedtime.workbench.split_down",
                WindowCommand::DuplicateView => "bedtime.workbench.duplicate_view",
                WindowCommand::Find => "bedtime.edit.find",
                WindowCommand::GoToLine => "bedtime.edit.go_to_line",
                WindowCommand::Debug => "bedtime.debug.show",
                WindowCommand::FindFile => "bedtime.explorer.find_file",
                WindowCommand::FindWorkspace => "bedtime.search.show",
                WindowCommand::NewTerminal | WindowCommand::Terminal => "bedtime.terminal.new",
                WindowCommand::NewExplorer | WindowCommand::Explorer => "bedtime.explorer.new",
                WindowCommand::NewSettings | WindowCommand::Settings => "bedtime.settings.new",
                WindowCommand::NewWorkspaces | WindowCommand::Workspaces => {
                    "bedtime.workspaces.new"
                }
                WindowCommand::NewDiagnostics | WindowCommand::Diagnostics => {
                    "bedtime.editor.new_diagnostics"
                }
                WindowCommand::NewStructure | WindowCommand::Structure => {
                    "bedtime.editor.new_structure"
                }
                WindowCommand::NewLspDashboard | WindowCommand::LspDashboard => {
                    "bedtime.editor.new_language_servers"
                }
                WindowCommand::NewReferences => "bedtime.editor.new_references",
                WindowCommand::NewContentSearch => "bedtime.search.new",
                _ => unreachable!(),
            };
            if world
                .resource::<bedtime_api::workbench::WorkbenchRegistry>()
                .command(&bedtime_api::CommandId::from(canonical))
                .is_none()
            {
                complete(world, id, Ok(AppRequestOutcome::Ignored));
                return Ok(None);
            }
            let arguments = if matches!(command, WindowCommand::Save | WindowCommand::SaveAs) {
                json!({"document":request.context.document.map(|document|document.0)})
            } else {
                request.arguments.clone()
            };
            Ok(Some(invoke(world, request, canonical.into(), arguments)))
        }
    }
}

fn opened(world: &World, panel: Entity) -> AppRequestOutcome {
    AppRequestOutcome::Opened {
        panel,
        document: world
            .get::<PanelBinding>(panel)
            .and_then(|binding| binding.document)
            .or_else(|| {
                world
                    .get::<bedtime_core::DocumentBinding>(panel)
                    .map(|binding| binding.0)
            }),
    }
}
fn complete(
    world: &mut World,
    id: Option<AppRequestId>,
    result: Result<AppRequestOutcome, AppRequestError>,
) {
    if let Err(error) = &result {
        world.resource_mut::<ApplicationState>().error = Some(error.message.clone());
    }
    if let Some(id) = id {
        world
            .resource_mut::<AppRequests>()
            .results
            .insert(id, result);
    }
}
pub fn poll_app_request_result(
    world: &mut World,
    id: AppRequestId,
) -> Option<Result<AppRequestOutcome, AppRequestError>> {
    if let Some(result) = world.resource_mut::<AppRequests>().results.remove(&id) {
        return Some(result);
    }
    let receipt = world.resource_mut::<AppRequests>().receipts.remove(&id)?;
    let result = match &receipt {
        Receipt::Host(id) => bedtime_core::take_host_result(world, *id).map(|result| {
            result.map(|outcome| match outcome {
                bedtime_core::HostOutcome::Ignored => AppRequestOutcome::Ignored,
                bedtime_core::HostOutcome::Opened(panel) => opened(world, panel),
                bedtime_core::HostOutcome::Saved {
                    documents,
                    revisions,
                } => AppRequestOutcome::Saved {
                    documents: documents
                        .into_iter()
                        .zip(revisions)
                        .map(|(document, revision)| SavedDocument { document, revision })
                        .collect(),
                },
                _ => AppRequestOutcome::Applied,
            })
        }),
        Receipt::Workbench(id, closed) => world
            .resource_mut::<WorkbenchRequests>()
            .take_result(*id)
            .map(|result| {
                result.map(|outcome| match outcome {
                    WorkbenchOutcome::Ignored => AppRequestOutcome::Ignored,
                    WorkbenchOutcome::Opened(panel) => opened(world, panel),
                    WorkbenchOutcome::Closed(panel) => AppRequestOutcome::Closed {
                        panels: vec![panel],
                    },
                    _ => closed
                        .as_ref()
                        .map_or(AppRequestOutcome::Applied, |panels| {
                            AppRequestOutcome::Closed {
                                panels: panels.clone(),
                            }
                        }),
                })
            }),
        Receipt::Workspace(id) => bedtime_plugin_workspaces::take_workspace_result(world, *id)
            .map(|result| result.map(|_| AppRequestOutcome::Applied)),
        Receipt::Layout(id) => {
            take_layout_result(world, *id).map(|result| result.map(|_| AppRequestOutcome::Applied))
        }
        Receipt::Settings(id) => world
            .resource_mut::<bedtime_api::settings::SettingsResults>()
            .0
            .remove(id)
            .map(|result| result.map(|_| AppRequestOutcome::Applied)),
    };
    match result {
        None => {
            world
                .resource_mut::<AppRequests>()
                .receipts
                .insert(id, receipt);
            None
        }
        Some(result) => Some(result.map_err(|error| AppRequestError {
            kind: io::ErrorKind::Other,
            message: error,
        })),
    }
}
pub fn poll_app_request(
    world: &mut World,
    id: AppRequestId,
) -> Option<Result<AppRequestOutcome, String>> {
    poll_app_request_result(world, id).map(|result| result.map_err(|error| error.message))
}
pub fn dispatch_app_command(
    world: &mut World,
    command: WindowCommand,
    target: PanelTarget,
) -> io::Result<bool> {
    dispatch_request(
        world,
        capture_app_request(world, AppAction::Command(command), target),
    )
}
pub fn dispatch_app_plugin_command(
    world: &mut World,
    command: &bedtime_api::CommandId,
    target: PanelTarget,
) -> io::Result<bool> {
    dispatch_request(
        world,
        capture_app_request(world, AppAction::PluginCommand(command.into()), target),
    )
}
fn dispatch_request(world: &mut World, request: AppRequest) -> io::Result<bool> {
    let id = submit_app_request(world, request);
    process_app_requests(world)?;
    match poll_app_request(world, id) {
        Some(Ok(AppRequestOutcome::Ignored)) => Ok(false),
        Some(Err(error)) => Err(io::Error::other(error)),
        _ => Ok(true),
    }
}

pub fn panel_entities(world: &mut World) -> Vec<Entity> {
    bedtime_core::panel_entities(world)
}
pub fn active_panel_entities(world: &mut World) -> Vec<Entity> {
    bedtime_core::active_panel_entities(world)
}
pub fn open_panel(
    world: &mut World,
    kind: &bedtime_api::PanelTypeId,
    input: PanelInput,
    state: Value,
    viewer: Option<bedtime_api::ViewerId>,
    area: Option<u32>,
    show_existing: bool,
) -> io::Result<Entity> {
    bedtime_core::open_panel(world, kind, input, state, viewer, area, show_existing)
        .map_err(io::Error::other)
}
pub fn focus_panel(world: &mut World, panel: Entity, request_native: bool) -> io::Result<()> {
    bedtime_core::focus_panel(world, panel, request_native).map_err(io::Error::other)
}
pub fn open_file(
    world: &mut World,
    path: &Path,
    viewer: Option<&bedtime_api::ViewerId>,
    additional: bool,
    area: Option<u32>,
) -> io::Result<Entity> {
    bedtime_core::open_file_instance(world, path, viewer, additional, area)
        .map_err(io::Error::other)
}
pub fn open_viewer(
    world: &mut World,
    panel: Entity,
    viewer: &bedtime_api::ViewerId,
) -> io::Result<Entity> {
    let document = world
        .get::<PanelBinding>(panel)
        .and_then(|binding| binding.document)
        .ok_or_else(|| io::Error::other("Panel has no document"))?;
    let area = world
        .get::<bedtime_workbench::PanelPresentation>(panel)
        .and_then(|panel| {
            world
                .resource::<WorkbenchLayout>()
                .tiling
                .area_for(panel.id)
        });
    bedtime_core::open_document_viewer(world, document, viewer, area).map_err(io::Error::other)
}
pub fn split_panel(
    world: &mut World,
    panel: Entity,
    axis: usize,
    high: bool,
) -> io::Result<Entity> {
    bedtime_core::split_panel(world, panel, axis, high).map_err(io::Error::other)
}
pub fn retire_panels(world: &mut World, panels: &[Entity]) -> io::Result<()> {
    bedtime_core::retire_panels(world, panels).map_err(io::Error::other)
}
pub fn commit_documents(world: &mut World, documents: &HashSet<DocumentId>) -> io::Result<()> {
    bedtime_core::commit_document_drafts(world, &documents.iter().copied().collect::<Vec<_>>())
        .map_err(io::Error::other)
}
pub fn dispatch_panel_action(
    world: &mut World,
    panel: Entity,
    action: PanelAction,
) -> io::Result<bool> {
    bedtime_core::dispatch_panel_action(world, panel, action)
        .map(|(accepted, effects)| {
            for effect in effects {
                world
                    .resource_mut::<bedtime_api::features::HostEffects>()
                    .push_effect(effect);
            }
            accepted
        })
        .map_err(io::Error::other)
}
pub fn restore_layout(world: &mut World, value: &Value, attach: bool) -> io::Result<()> {
    let result = bedtime_core::restore_document_layout(world, value, attach, None);
    bedtime_plugin_layouts::poll::<LayoutSlot, bedtime_core::layouts::DocumentLayouts>(world);
    bed::application::runtime::update_application_policy(world);
    result.map_err(io::Error::other)
}
pub fn save_default_layout(world: &mut World) -> io::Result<()> {
    bed::application::runtime::apply_layout_command(world, LayoutCommand::SaveDefaultLayout)
}
pub fn reset_layout(world: &mut World) -> io::Result<()> {
    bed::application::runtime::apply_layout_command(world, LayoutCommand::ResetLayout)
}
pub fn attach_workspace(world: &mut World, path: &Path) -> io::Result<()> {
    let request = capture_app_request(
        world,
        AppAction::AttachWorkspace(path.to_owned()),
        PanelTarget::LargestArea,
    );
    let id = submit_app_request(world, request);
    wait_app_request(world, id).map(|_| ())
}
pub fn replace_workspace(world: &mut World, spec: WorkspaceSpec) -> io::Result<()> {
    let request = capture_app_request(
        world,
        AppAction::Workspace(WorkspaceAction::OpenWorkspace(spec)),
        PanelTarget::LargestArea,
    );
    let id = submit_app_request(world, request);
    wait_app_request(world, id).map(|_| ())
}

pub fn application_commands(world: &World) -> Vec<CommandItem> {
    let editor = world.resource::<bedtime_api::features::FeatureRegistry>();
    let registry = world.resource::<bedtime_api::workbench::WorkbenchRegistry>();
    let facts = world.get_resource::<bedtime_api::workbench::CommandPresentations>();
    let mut seen = HashSet::new();
    editor
        .menus()
        .iter()
        .filter(|menu| menu.slot == bedtime_api::features::MenuSlot::Application)
        .map(|menu| &menu.command)
        .chain(registry.menus().iter())
        .filter(|id| seen.insert((*id).clone()))
        .filter_map(|id| registry.command(id))
        .map(|command| command.presentation(facts))
        .filter(|command| command.visible)
        .map(|command| CommandItem {
            id: command.id.into(),
            label: command.label.into(),
            icon: command.icon.map(str::to_owned),
            enabled: command.enabled,
        })
        .collect()
}

pub fn toolbar_commands(world: &World) -> Vec<CommandItem> {
    let editor = world.resource::<bedtime_api::features::FeatureRegistry>();
    let registry = world.resource::<bedtime_api::workbench::WorkbenchRegistry>();
    let facts = world.get_resource::<bedtime_api::workbench::CommandPresentations>();
    let mut seen = HashSet::new();
    editor
        .toolbar()
        .iter()
        .map(|item| &item.command)
        .chain(registry.toolbar().iter())
        .filter(|id| seen.insert((*id).clone()))
        .filter_map(|id| registry.command(id))
        .map(|command| command.presentation(facts))
        .filter(|command| command.visible)
        .map(|command| CommandItem {
            id: command.id.into(),
            label: command.label.into(),
            icon: command.icon.map(str::to_owned),
            enabled: command.enabled,
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandItem {
    pub id: bedtime_api::CommandId,
    pub label: String,
    pub icon: Option<String>,
    pub enabled: bool,
}
