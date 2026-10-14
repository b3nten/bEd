//! Typed, asynchronous application control through the normal application transitions.
//! Install after the workbench plugins. External control is explicitly enabled.
use bedtime_api::workbench::{
    CommandInvocation, PanelPresentation, WorkbenchAction, WorkbenchRegistry,
};
use bedtime_desktop::{
    DesktopCaptureId, DesktopCaptureRequests, DesktopCaptureResults, DesktopCaptureTarget,
    DesktopCapturedFrame, DesktopFrameId, DesktopFrameInfo, DesktopInputId, DesktopInputRequests,
    DesktopInputResults, DesktopInputStep, DesktopSystems,
};
use bedtime_workbench::{
    WorkbenchLayout, WorkbenchOutcome, WorkbenchRequestId, WorkbenchRequests, WorkbenchSystems,
};
use bevy::prelude::*;
use serde_json::Value;
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[cfg(feature = "documents")]
mod documents;
mod extensions;
pub mod transport;
pub mod wire;
#[cfg(feature = "documents")]
pub use documents::{
    DocumentHandle, DocumentInfo, DocumentRead, document_handle, read_document, resolve_document,
};
pub use extensions::{ExtensionDefinition, ExtensionStart, register_extension};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct OperationId(pub u64);

/// A live identity, distinct from the panel number persisted in a workspace.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PanelHandle {
    app: u64,
    session_id: u64,
    entity: Entity,
}

/// Capture a currently live panel for an application-owned operation.
pub fn panel_handle(world: &World, entity: Entity) -> Result<PanelHandle, AutomationError> {
    if !world.contains_resource::<AutomationScope>() {
        return Err(AutomationError::new(
            ErrorKind::Unavailable,
            "automation is not installed",
        ));
    }
    let panel = handle(world, entity);
    resolve_panel(world, panel)?;
    Ok(panel)
}

impl PanelHandle {
    pub fn entity(self) -> Entity {
        self.entity
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureTarget {
    Window,
    Panel(PanelHandle),
}

#[derive(Debug)]
pub enum Operation {
    Describe,
    Extension {
        name: String,
        arguments: Value,
    },
    Inspect,
    OpenPanel {
        panel_type: String,
        state: Value,
        area: Option<u32>,
    },
    FocusPanel(PanelHandle),
    SplitPanel {
        panel: PanelHandle,
        axis: usize,
        high: bool,
    },
    ClosePanel(PanelHandle),
    InvokeCommand {
        command: String,
        source: Option<PanelHandle>,
        arguments: Value,
    },
    Input(Vec<DesktopInputStep>),
    Capture {
        targets: Vec<CaptureTarget>,
        after: Option<OperationId>,
        timeout: Duration,
    },
    #[cfg(feature = "documents")]
    ReadDocument(DocumentHandle),
}

#[derive(Clone, Debug)]
pub struct PanelInfo {
    #[cfg(feature = "documents")]
    pub document: Option<DocumentHandle>,
    pub handle: PanelHandle,
    pub persisted_id: u64,
    pub panel_type: String,
    pub title: String,
    pub area: Option<u32>,
    pub focused: bool,
    pub available: bool,
    pub readiness: Readiness,
    pub visible_content: Option<bedtime_desktop::DesktopRect>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Readiness {
    Pending,
    Ready,
    Failed(String),
}

#[derive(Clone, Debug)]
pub struct CommandInfo {
    pub id: String,
    pub label: String,
    pub enabled: bool,
    pub visible: bool,
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub frame: Option<DesktopFrameInfo>,
    pub panels: Vec<PanelInfo>,
    pub commands: Vec<CommandInfo>,
    pub layout: Value,
    #[cfg(feature = "documents")]
    pub documents: Vec<DocumentInfo>,
}

#[derive(Debug)]
pub enum OperationValue {
    Json(Value),
    Applied,
    Ignored,
    Opened(PanelHandle),
    Closed(Vec<PanelHandle>),
    Snapshot(Snapshot),
    Input(bedtime_desktop::DesktopInputReceipt),
    Captured(DesktopCapturedFrame),
    #[cfg(feature = "documents")]
    Document(DocumentRead),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperationStatus {
    Pending,
    Complete,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    StaleHandle,
    Unavailable,
    InvalidRequest,
    RevisionConflict,
    Failed,
    DeadlineExceeded,
    DependencyFailed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AutomationError {
    pub kind: ErrorKind,
    pub message: String,
}

impl std::fmt::Display for AutomationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.message.fmt(f)
    }
}

impl std::error::Error for AutomationError {}

impl AutomationError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

pub type OperationResult = Result<OperationValue, AutomationError>;

#[derive(Resource, Default)]
pub struct AutomationRequests {
    next: u64,
    pending: VecDeque<(OperationId, Operation)>,
    statuses: Arc<Mutex<HashMap<OperationId, OperationStatus>>>,
}

impl AutomationRequests {
    pub fn submit(&mut self, operation: Operation) -> OperationId {
        self.next = self
            .next
            .checked_add(1)
            .expect("automation operation ID overflow");
        let id = OperationId(self.next);
        self.statuses
            .lock()
            .expect("automation status mutex poisoned")
            .insert(id, OperationStatus::Pending);
        self.pending.push_back((id, operation));
        id
    }
}

struct Completion {
    frame: DesktopFrameId,
    result: Result<(), AutomationError>,
    payload: Option<OperationResult>,
}

#[derive(Resource, Default)]
pub struct AutomationResults {
    statuses: Arc<Mutex<HashMap<OperationId, OperationStatus>>>,
    complete: HashMap<OperationId, Completion>,
}

impl AutomationResults {
    pub fn status(&self, id: OperationId) -> Option<OperationStatus> {
        self.statuses
            .lock()
            .expect("automation status mutex poisoned")
            .get(&id)
            .copied()
    }
    /// Release the owned payload. Its small completion receipt remains usable by captures.
    pub fn take(&mut self, id: OperationId) -> Option<OperationResult> {
        self.complete.get_mut(&id)?.payload.take()
    }
    /// Explicitly retire a receipt once no future capture needs it.
    pub fn release(&mut self, id: OperationId) -> bool {
        if self.status(id) != Some(OperationStatus::Complete) {
            return false;
        }
        self.complete.remove(&id);
        self.statuses
            .lock()
            .expect("automation status mutex poisoned")
            .remove(&id);
        true
    }
    pub fn completion_frame(&self, id: OperationId) -> Option<DesktopFrameId> {
        self.complete.get(&id).map(|receipt| receipt.frame)
    }
    fn finish(&mut self, id: OperationId, result: OperationResult, frame: DesktopFrameId) {
        self.statuses
            .lock()
            .expect("automation status mutex poisoned")
            .insert(id, OperationStatus::Complete);
        self.complete.insert(
            id,
            Completion {
                frame,
                result: result.as_ref().map(|_| ()).map_err(Clone::clone),
                payload: Some(result),
            },
        );
    }
}

#[derive(Resource)]
struct AutomationScope(u64);

#[derive(Resource, Default)]
struct ActiveOperations(Vec<(OperationId, Pending)>);

enum Pending {
    Immediate(Option<OperationResult>),
    Extension(extensions::ExtensionPoll),
    Workbench(WorkbenchRequestId),
    Inspect,
    Input(DesktopInputId),
    Capture {
        targets: Vec<CaptureTarget>,
        after: Option<OperationId>,
        deadline: Instant,
        capture: Option<DesktopCaptureId>,
    },
    #[cfg(feature = "documents")]
    ReadDocument(DocumentHandle),
}

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AutomationSystems {
    Ingress,
    Complete,
}

pub struct AutomationPlugin;

impl Plugin for AutomationPlugin {
    fn is_unique(&self) -> bool {
        false
    }
    fn build(&self, app: &mut App) {
        if app.world().contains_resource::<AutomationScope>() {
            return;
        }
        assert!(
            app.world().contains_resource::<WorkbenchRequests>(),
            "install the workbench before AutomationPlugin"
        );
        let mut scope = [0u8; 8];
        std::io::Read::read_exact(
            &mut std::fs::File::open("/dev/urandom").expect("OS randomness"),
            &mut scope,
        )
        .expect("OS randomness");
        let statuses = Arc::default();
        app.insert_resource(AutomationRequests {
            statuses: Arc::clone(&statuses),
            ..default()
        })
        .insert_resource(AutomationResults {
            statuses,
            ..default()
        })
        .insert_resource(AutomationScope(u64::from_ne_bytes(scope)))
        .init_resource::<AutomationRequests>()
        .init_resource::<AutomationResults>()
        .init_resource::<ActiveOperations>()
        .init_resource::<extensions::Extensions>()
        .add_systems(
            PreUpdate,
            ingress
                .in_set(AutomationSystems::Ingress)
                .before(WorkbenchSystems::Requests)
                .before(DesktopSystems::InjectInput),
        )
        .add_systems(PostUpdate, complete.in_set(AutomationSystems::Complete))
        .add_systems(Last, shutdown.after(bevy::app::OnAppExitSystems));
    }
}

fn frame(world: &World) -> DesktopFrameId {
    world
        .get_resource::<DesktopFrameInfo>()
        .map_or(DesktopFrameId(0), |info| info.frame)
}

fn session_id(world: &World) -> u64 {
    #[cfg(feature = "documents")]
    if let Some(session) = world.get_non_send::<bedtime_documents::session::DocumentSession>() {
        return session.session_id().0;
    }
    let _ = world;
    0
}

fn handle(world: &World, entity: Entity) -> PanelHandle {
    PanelHandle {
        app: world.resource::<AutomationScope>().0,
        session_id: session_id(world),
        entity,
    }
}

pub fn resolve_panel(world: &World, panel: PanelHandle) -> Result<Entity, AutomationError> {
    let scope = world.get_resource::<AutomationScope>().ok_or_else(|| {
        AutomationError::new(ErrorKind::Unavailable, "automation is not installed")
    })?;
    if panel.app != scope.0
        || panel.session_id != session_id(world)
        || world.get::<PanelPresentation>(panel.entity).is_none()
    {
        return Err(AutomationError::new(
            ErrorKind::StaleHandle,
            "panel belongs to a retired application/session or has closed",
        ));
    }
    Ok(panel.entity)
}

fn ingress(world: &mut World) {
    let operations = std::mem::take(&mut world.resource_mut::<AutomationRequests>().pending);
    for (id, operation) in operations {
        match start(world, operation) {
            Ok(pending) => world
                .resource_mut::<ActiveOperations>()
                .0
                .push((id, pending)),
            Err(error) => {
                let frame = frame(world);
                world
                    .resource_mut::<AutomationResults>()
                    .finish(id, Err(error), frame);
            }
        }
    }
}

fn start(world: &mut World, operation: Operation) -> Result<Pending, AutomationError> {
    let action = match operation {
        Operation::Describe => {
            return Ok(Pending::Immediate(Some(Ok(OperationValue::Json(
                extensions::describe(world),
            )))));
        }
        Operation::Extension { name, arguments } => {
            return extensions::start(world, &name, arguments);
        }
        Operation::Inspect => return Ok(Pending::Inspect),
        Operation::OpenPanel {
            panel_type,
            state,
            area,
        } => {
            if world
                .resource::<WorkbenchRegistry>()
                .panel(&panel_type)
                .is_none()
            {
                return Err(AutomationError::new(
                    ErrorKind::Unavailable,
                    format!("panel feature is unavailable: {panel_type}"),
                ));
            }
            WorkbenchAction::OpenPanel {
                panel_type,
                state,
                area,
            }
        }
        Operation::FocusPanel(panel) => WorkbenchAction::FocusPanel(resolve_panel(world, panel)?),
        Operation::SplitPanel { panel, axis, high } => {
            if axis > 1 {
                return Err(AutomationError::new(
                    ErrorKind::InvalidRequest,
                    "split axis must be 0 or 1",
                ));
            }
            WorkbenchAction::SplitPanel {
                panel: resolve_panel(world, panel)?,
                axis,
                high,
            }
        }
        Operation::ClosePanel(panel) => WorkbenchAction::ClosePanel(resolve_panel(world, panel)?),
        Operation::InvokeCommand {
            command,
            source,
            arguments,
        } => {
            if world
                .resource::<WorkbenchRegistry>()
                .command(&command)
                .is_none()
            {
                return Err(AutomationError::new(
                    ErrorKind::Unavailable,
                    format!("command is unavailable: {command}"),
                ));
            }
            let source = source
                .map(|panel| resolve_panel(world, panel))
                .transpose()?
                .or_else(|| world.resource::<WorkbenchLayout>().focused);
            let area = source
                .and_then(|source| world.get::<PanelPresentation>(source))
                .and_then(|panel| {
                    world
                        .resource::<WorkbenchLayout>()
                        .tiling
                        .area_for(panel.id)
                })
                .unwrap_or_else(|| world.resource::<WorkbenchLayout>().tiling.layout.areas[0].id);
            let invocation = CommandInvocation {
                command,
                source,
                area,
                arguments,
            };
            if world
                .get_resource::<bedtime_api::workbench::CommandPresentations>()
                .and_then(|facts| facts.0.get(&invocation.command))
                .is_some_and(|fact| fact.blocks_invocation(world, &invocation))
            {
                return Err(AutomationError::new(
                    ErrorKind::Unavailable,
                    format!("command is currently unavailable: {}", invocation.command),
                ));
            }
            WorkbenchAction::Invoke(invocation)
        }
        Operation::Input(steps) => {
            if world
                .get_resource::<transport::InputBlocked>()
                .is_some_and(|blocked| blocked.0)
            {
                return Err(AutomationError::new(
                    ErrorKind::Unavailable,
                    "input is blocked during connection approval",
                ));
            }
            let mut requests = world
                .get_resource_mut::<DesktopInputRequests>()
                .ok_or_else(|| {
                    AutomationError::new(ErrorKind::Unavailable, "desktop input is not installed")
                })?;
            return Ok(Pending::Input(requests.submit(steps)));
        }
        Operation::Capture {
            targets,
            after,
            timeout,
        } => {
            if targets.is_empty() {
                return Err(AutomationError::new(
                    ErrorKind::InvalidRequest,
                    "capture requires at least one target",
                ));
            }
            for target in &targets {
                if let CaptureTarget::Panel(panel) = target {
                    resolve_panel(world, *panel)?;
                }
            }
            let deadline = Instant::now().checked_add(timeout).ok_or_else(|| {
                AutomationError::new(
                    ErrorKind::InvalidRequest,
                    "capture timeout exceeds clock range",
                )
            })?;
            return Ok(Pending::Capture {
                targets,
                after,
                deadline,
                capture: None,
            });
        }
        #[cfg(feature = "documents")]
        Operation::ReadDocument(document) => {
            documents::resolve_document(world, document)?;
            return Ok(Pending::ReadDocument(document));
        }
    };
    Ok(Pending::Workbench(
        world.resource_mut::<WorkbenchRequests>().submit(action),
    ))
}

fn complete(world: &mut World) {
    let operations = std::mem::take(&mut world.resource_mut::<ActiveOperations>().0);
    for (id, mut pending) in operations {
        let result = poll(world, &mut pending);
        if let Some(result) = result {
            let frame = frame(world);
            world
                .resource_mut::<AutomationResults>()
                .finish(id, result, frame);
        } else {
            world
                .resource_mut::<ActiveOperations>()
                .0
                .push((id, pending));
        }
    }
}

fn poll(world: &mut World, pending: &mut Pending) -> Option<OperationResult> {
    match pending {
        Pending::Immediate(result) => result.take(),
        Pending::Extension(poll) => poll(world).map(|result| result.map(OperationValue::Json)),
        Pending::Workbench(id) => world
            .resource_mut::<WorkbenchRequests>()
            .take_result(*id)
            .map(|result| {
                result
                    .map(|outcome| match outcome {
                        WorkbenchOutcome::Applied => OperationValue::Applied,
                        WorkbenchOutcome::Ignored => OperationValue::Ignored,
                        WorkbenchOutcome::Opened(panel) => {
                            OperationValue::Opened(handle(world, panel))
                        }
                        WorkbenchOutcome::Closed(panel) => {
                            OperationValue::Closed(vec![handle(world, panel)])
                        }
                    })
                    .map_err(|error| AutomationError::new(ErrorKind::Failed, error))
            }),
        Pending::Inspect => Some(inspect(world).map(OperationValue::Snapshot)),
        Pending::Input(id) => {
            let Some(mut results) = world.get_resource_mut::<DesktopInputResults>() else {
                return Some(Err(AutomationError::new(
                    ErrorKind::Unavailable,
                    "desktop input stopped",
                )));
            };
            results.take(*id).map(|result| {
                result
                    .map(OperationValue::Input)
                    .map_err(|error| AutomationError::new(ErrorKind::Failed, error.to_string()))
            })
        }
        Pending::Capture {
            targets,
            after,
            deadline,
            capture,
        } => {
            if Instant::now() >= *deadline {
                return Some(Err(AutomationError::new(
                    ErrorKind::DeadlineExceeded,
                    "capture deadline exceeded",
                )));
            }
            if let Some(id) = capture {
                let Some(mut results) = world.get_resource_mut::<DesktopCaptureResults>() else {
                    return Some(Err(AutomationError::new(
                        ErrorKind::Unavailable,
                        "desktop captures stopped",
                    )));
                };
                return results.take(*id).map(|result| {
                    result.map(OperationValue::Captured).map_err(|error| {
                        let kind = match &error {
                            bedtime_desktop::DesktopCaptureError::DeadlineExceeded => {
                                ErrorKind::DeadlineExceeded
                            }
                            bedtime_desktop::DesktopCaptureError::TargetNotVisible(_) => {
                                ErrorKind::Unavailable
                            }
                            _ => ErrorKind::Failed,
                        };
                        AutomationError::new(kind, error.to_string())
                    })
                });
            }
            let after_frame = if let Some(after) = after {
                let results = world.resource::<AutomationResults>();
                if let Some(receipt) = results.complete.get(after) {
                    if let Err(error) = &receipt.result {
                        return Some(Err(AutomationError::new(
                            ErrorKind::DependencyFailed,
                            error.to_string(),
                        )));
                    }
                    Some(receipt.frame)
                } else if results.status(*after) == Some(OperationStatus::Pending) {
                    return None;
                } else {
                    return Some(Err(AutomationError::new(
                        ErrorKind::InvalidRequest,
                        "preceding operation is unknown",
                    )));
                }
            } else {
                Some(frame(world))
            };
            let targets = match targets
                .iter()
                .map(|target| match target {
                    CaptureTarget::Window => Ok(DesktopCaptureTarget::Window),
                    CaptureTarget::Panel(panel) => {
                        resolve_panel(world, *panel).map(DesktopCaptureTarget::Panel)
                    }
                })
                .collect::<Result<Vec<_>, _>>()
            {
                Ok(targets) => targets,
                Err(error) => return Some(Err(error)),
            };
            let Some(mut requests) = world.get_resource_mut::<DesktopCaptureRequests>() else {
                return Some(Err(AutomationError::new(
                    ErrorKind::Unavailable,
                    "desktop captures are not installed",
                )));
            };
            *capture = Some(requests.submit(targets, *deadline, after_frame));
            None
        }
        #[cfg(feature = "documents")]
        Pending::ReadDocument(document) => {
            Some(read_document(world, *document).map(OperationValue::Document))
        }
    }
}

fn inspect(world: &mut World) -> Result<Snapshot, AutomationError> {
    let frame = world.get_resource::<DesktopFrameInfo>().cloned();
    let records = world
        .query_filtered::<(Entity, &PanelPresentation, &bedtime_workbench::PanelRecord), Without<bedtime_api::workbench::PanelSuspended>>()
        .iter(world)
        .map(|(entity, panel, record)| {
            (
                entity,
                panel.id,
                record.panel_type.clone(),
                record.unavailable.is_none(),
            )
        })
        .collect::<Vec<_>>();
    let mut panels = Vec::new();
    for (entity, persisted_id, panel_type, available) in records {
        let title = bedtime_workbench::panel_title(world, entity)
            .map_err(|error| AutomationError::new(ErrorKind::Failed, error))?;
        let layout = world.resource::<WorkbenchLayout>();
        let readiness = {
            let surface = world
                .get::<bedtime_api::workbench::PanelFrontend>(entity)
                .map_or(entity, |frontend| frontend.0);
            let readiness = world
                .get::<bedtime_api::workbench::PanelReadiness>(surface)
                .map(|readiness| match readiness {
                    bedtime_api::workbench::PanelReadiness::Pending => Readiness::Pending,
                    bedtime_api::workbench::PanelReadiness::Ready => Readiness::Ready,
                    bedtime_api::workbench::PanelReadiness::Failed(error) => {
                        Readiness::Failed(error.clone())
                    }
                })
                .unwrap_or(Readiness::Ready);
            if available {
                readiness
            } else {
                Readiness::Failed("panel feature is unavailable".into())
            }
        };
        panels.push(PanelInfo {
            #[cfg(feature = "documents")]
            document: {
                let surface = world
                    .get::<bedtime_api::workbench::PanelFrontend>(entity)
                    .map_or(entity, |frontend| frontend.0);
                world
                    .get::<bedtime_api::documents::PanelBinding>(surface)
                    .and_then(|binding| binding.document)
                    .map(|id| document_handle(world, id))
            },
            readiness,
            handle: handle(world, entity),
            persisted_id,
            panel_type,
            title,
            area: layout.tiling.area_for(persisted_id),
            focused: layout.focused == Some(entity),
            available,
            visible_content: frame.as_ref().and_then(|frame| {
                frame
                    .panels
                    .iter()
                    .find(|geometry| geometry.panel == entity && geometry.visible)
                    .map(|geometry| geometry.content)
            }),
        });
    }
    panels.sort_by_key(|panel| panel.persisted_id);
    let commands = world
        .resource::<WorkbenchRegistry>()
        .commands()
        .iter()
        .map(|command| {
            if let Some(fact) = world
                .get_resource::<bedtime_api::workbench::CommandPresentations>()
                .and_then(|facts| facts.0.get(command.id))
            {
                return CommandInfo {
                    id: command.id.into(),
                    label: fact.label.clone().unwrap_or_else(|| command.label.into()),
                    enabled: fact.enabled,
                    visible: fact.visible,
                };
            }
            CommandInfo {
                id: command.id.into(),
                label: command.label.into(),
                enabled: true,
                visible: true,
            }
        })
        .collect();
    let layout = world
        .resource::<WorkbenchLayout>()
        .tiling
        .to_value(panels.iter().map(|panel| panel.persisted_id));
    Ok(Snapshot {
        frame,
        panels,
        commands,
        layout,
        #[cfg(feature = "documents")]
        documents: documents::documents(world)?,
    })
}

fn shutdown(world: &mut World) {
    if !world
        .get_resource::<Messages<AppExit>>()
        .is_some_and(|events| !events.is_empty())
    {
        return;
    }
    let waiting = std::mem::take(&mut world.resource_mut::<AutomationRequests>().pending);
    let active = std::mem::take(&mut world.resource_mut::<ActiveOperations>().0);
    let frame = frame(world);
    for id in waiting
        .into_iter()
        .map(|(id, _)| id)
        .chain(active.into_iter().map(|(id, _)| id))
    {
        world.resource_mut::<AutomationResults>().finish(
            id,
            Err(AutomationError::new(
                ErrorKind::Failed,
                "application stopped before this operation completed",
            )),
            frame,
        );
    }
}

#[cfg(test)]
mod tests;
