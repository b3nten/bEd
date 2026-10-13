//! Document requests resolve between UI scopes, with retained operation receipts.
use crate::models::{self, ModelRegistry};
pub use bed_api::features::HostRequest;
use bed_api::{
    documents::DocumentFrame,
    documents::{PanelBinding, PanelInput, Revision},
    features::{HostAction, HostEffects},
};
use bed_documents::{ClosePolicy, DocumentId, DocumentSession};
use bed_workbench::{WorkbenchLayout, WorkbenchRequestMode, WorkbenchRequests};
use bevy::{ecs::message::MessageCursor, prelude::*};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, VecDeque},
    path::Path,
};

#[derive(Resource, Default, Clone, Copy, PartialEq, Eq)]
pub enum CorePresentationMode {
    #[default]
    Standard,
    Application,
}
#[derive(Resource, Default)]
pub(crate) struct CoreExiting(pub bool);
#[derive(Resource, Default)]
struct LifecycleSaves {
    close: Option<crate::DocumentSaveId>,
    quit: Option<crate::DocumentSaveId>,
}
pub(crate) fn quit_save_pending(world: &World) -> bool {
    world.resource::<LifecycleSaves>().quit.is_some()
}
#[derive(Resource)]
struct LiveDocumentOrigin(bevy::asset::uuid::Uuid);
impl Default for LiveDocumentOrigin {
    fn default() -> Self {
        Self(bevy::asset::uuid::Uuid::new_v4())
    }
}
#[derive(Resource, Clone, Copy)]
pub struct DocumentClosePolicy(pub ClosePolicy);
impl Default for DocumentClosePolicy {
    fn default() -> Self {
        Self(ClosePolicy::Cancel)
    }
}

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CoreSystems {
    Services,
    Frame,
    Requests,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct HostRequestId(u64);
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostOutcome {
    Applied,
    Opened(Entity),
    Pending(crate::DocumentSaveId),
    Transition(crate::OperationBarrierToken),
    Saved {
        documents: Vec<DocumentId>,
        revisions: Vec<Revision>,
    },
}

pub fn capture_host_request(
    world: &World,
    source: Option<Entity>,
    request: HostAction,
) -> HostRequest {
    capture_host_request_area(world, source, request, None)
}
fn capture_host_request_area(
    world: &World,
    source: Option<Entity>,
    request: HostAction,
    area: Option<u32>,
) -> HostRequest {
    let layout = world.resource::<WorkbenchLayout>();
    let area = area.unwrap_or_else(|| {
        if matches!(
            &request,
            HostAction::OpenFileIn {
                target: bed_api::workbench::PanelTarget::LastFocused,
                ..
            } | HostAction::PreviewFileIn {
                target: bed_api::workbench::PanelTarget::LastFocused,
                ..
            } | HostAction::ShowPanelIn {
                target: bed_api::workbench::PanelTarget::LastFocused,
                ..
            } | HostAction::DropFile { .. }
        ) {
            source
                .and_then(|entity| bed_api::workbench::panel_owner(world, entity))
                .and_then(|owner| world.get::<bed_api::workbench::PanelPresentation>(owner))
                .and_then(|panel| layout.tiling.area_for(panel.id))
                .or(layout.focused_area)
                .unwrap_or_else(|| layout.tiling.layout.largest_area())
        } else {
            layout.tiling.layout.largest_area()
        }
    });
    HostRequest::capture(
        world.non_send::<DocumentSession>().session_id(),
        source,
        request,
        area,
        world.resource::<bed_workbench::WorkbenchLayoutEpoch>().0,
    )
}
#[derive(Message, Clone, Debug)]
pub struct CoreNotice {
    pub source: Option<Entity>,
    pub message: String,
}

struct Accepted {
    id: Option<HostRequestId>,
    request: HostRequest,
}
struct PendingCommand {
    id: Option<bed_workbench::WorkbenchRequestId>,
    outcome: Option<Result<bed_workbench::WorkbenchOutcome, String>>,
    effects: VecDeque<HostRequest>,
    saves: Vec<crate::DocumentSaveId>,
    barrier: Option<crate::OperationBarrierToken>,
    error: Option<String>,
}
#[derive(Resource, Default)]
pub struct HostRequests {
    next: u64,
    pending: VecDeque<Accepted>,
    results: HashMap<HostRequestId, Result<HostOutcome, String>>,
    cursor: MessageCursor<HostRequest>,
    settings_cursor: MessageCursor<bed_api::settings::SettingsRequest>,
    settings: VecDeque<bed_api::settings::SettingsRequest>,
    saves: Vec<(Option<HostRequestId>, crate::DocumentSaveId)>,
    commands: Vec<PendingCommand>,
    transitions: Vec<(Option<HostRequestId>, crate::OperationBarrierToken)>,
}
pub fn submit_host_request(
    world: &mut World,
    source: Option<Entity>,
    request: HostAction,
) -> HostRequestId {
    let request = capture_host_request(world, source, request);
    let mut queue = world.resource_mut::<HostRequests>();
    queue.next = queue
        .next
        .checked_add(1)
        .expect("document request ID overflow");
    let id = HostRequestId(queue.next);
    queue.pending.push_back(Accepted {
        id: Some(id),
        request,
    });
    id
}
pub fn take_host_result(
    world: &mut World,
    id: HostRequestId,
) -> Option<Result<HostOutcome, String>> {
    world.resource_mut::<HostRequests>().results.remove(&id)
}

pub struct DocumentHostPlugin;
impl Plugin for DocumentHostPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<bed_workbench::WorkbenchPlugin>() {
            app.add_plugins(bed_workbench::WorkbenchPlugin);
        }
        crate::settings::install_settings(app.world_mut());
        app.init_non_send::<DocumentSession>()
            .init_resource::<bed_files::FileContext>()
            .init_resource::<bed_files::FileServices>()
            .init_resource::<bed_storage::Storage>()
            .init_resource::<crate::saves::DocumentSaves>()
            .init_resource::<crate::transitions::OperationBarriers>()
            .init_non_send::<DocumentServiceReport>()
            .init_resource::<crate::files::FileOperations>()
            .init_resource::<ModelRegistry>()
            .init_resource::<DocumentFrame>()
            .init_resource::<HostEffects>()
            .init_resource::<bed_api::features::FeatureRegistry>()
            .init_resource::<bed_api::workbench::CommandPresentations>()
            .init_resource::<HostRequests>()
            .init_resource::<CorePresentationMode>()
            .init_resource::<DocumentRequestScheduling>()
            .init_resource::<bed_api::settings::SettingsSnapshot>()
            .init_resource::<bed_api::presentation::PresentationFrame>()
            .add_message::<bed_api::settings::SettingsRequest>()
            .init_resource::<CoreExiting>()
            .init_resource::<LifecycleSaves>()
            .init_resource::<LiveDocumentOrigin>()
            .init_resource::<DocumentClosePolicy>()
            .insert_resource(WorkbenchRequestMode::External)
            .add_message::<HostRequest>()
            .add_message::<CoreNotice>()
            .add_message::<bed_api::documents::DocumentReply>()
            .add_message::<bed_api::documents::DocumentEvent>()
            .add_message::<bed_api::storage::FeatureStateChanged>()
            .add_systems(
                PreUpdate,
                (
                    poll_documents.in_set(CoreSystems::Services),
                    publish_documents
                        .in_set(CoreSystems::Frame)
                        .after(CoreSystems::Services)
                        .before(bed_api::features::FeatureSystems::Prepare),
                    resolve_scheduled_requests
                        .in_set(CoreSystems::Requests)
                        .after(bed_api::features::FeatureSystems::Prepare),
                ),
            );
        if *app.world().resource::<CorePresentationMode>() == CorePresentationMode::Standard
            && !app
                .world()
                .contains_resource::<bed_workbench::WorkbenchClosePolicy>()
        {
            app.insert_resource(bed_workbench::WorkbenchClosePolicy(close_panel));
        }
        if !app
            .world()
            .contains_resource::<bed_workbench::WorkbenchPanelRetirement>()
        {
            app.insert_resource(bed_workbench::WorkbenchPanelRetirement(retire_panel));
        }
        if !app
            .world()
            .contains_resource::<bed_workbench::PanelPreparation>()
        {
            app.insert_resource(bed_workbench::PanelPreparation(prepare_panel));
        }
        if !app.is_plugin_added::<crate::commands::DocumentCommandsPlugin>() {
            app.add_plugins(crate::commands::DocumentCommandsPlugin);
        }
        #[cfg(feature = "language-services")]
        app.init_resource::<crate::format_saves::ExplicitDocumentSaves>();
    }
}
#[derive(Resource, Default, Clone, Copy, PartialEq, Eq)]
pub enum DocumentRequestScheduling {
    #[default]
    Automatic,
    External,
}
fn resolve_scheduled_requests(world: &mut World) {
    if *world.resource::<DocumentRequestScheduling>() == DocumentRequestScheduling::Automatic {
        resolve_documents(world);
    }
}
#[derive(Default)]
struct DocumentServiceReport(bed_documents::session::TickReport);

pub fn take_document_service_report(world: &mut World) -> bed_documents::session::TickReport {
    std::mem::take(&mut world.non_send_mut::<DocumentServiceReport>().0)
}
pub fn poll_document_services(world: &mut World) {
    crate::settings::refresh_settings(world);
    #[cfg(feature = "language-services")]
    crate::configuration::configure_syntax(world);
    let report = world.non_send_mut::<DocumentSession>().tick();
    crate::saves::poll_document_saves(world, &report);
    #[cfg(feature = "language-services")]
    crate::format_saves::poll_explicit_document_saves(world);
    crate::transitions::poll_barrier_admission(world);
    crate::files::poll_file_operations(world);
    for event in &report.events {
        world.write_message(bed_api::documents::DocumentEvent(event.clone()));
    }
    for error in &report.errors {
        notice(world, None, error.message.clone());
    }
    world.non_send_mut::<DocumentServiceReport>().0 = report;
}
fn poll_documents(world: &mut World) {
    poll_document_services(world);
}
pub fn publish_documents(world: &mut World) {
    let result = world.resource_scope(|world, mut frame: Mut<DocumentFrame>| {
        frame.session_id = world.non_send::<DocumentSession>().session_id().0;
        frame.refresh_documents(world.non_send::<DocumentSession>())
    });
    if let Err(error) = result {
        notice(world, None, error.to_string());
    }
    let records = world
        .query::<(Entity, &PanelBinding)>()
        .iter(world)
        .filter_map(|(entity, binding)| binding.document.map(|document| (entity, document)))
        .collect::<Vec<_>>();
    for (entity, document) in records {
        if let Ok(snapshot) = world.non_send::<DocumentSession>().snapshot(document) {
            let owner = bed_api::workbench::panel_owner(world, entity).unwrap_or(entity);
            if !snapshot.path.is_empty()
                && let Some(mut record) = world.get_mut::<bed_workbench::PanelRecord>(owner)
            {
                record.serialized["path"] = json!(snapshot.path);
                record.serialized["document_kind"] =
                    json!(if snapshot.kind == bed_documents::DocumentKind::Text {
                        "text"
                    } else {
                        "bytes"
                    });
            }
            if world
                .get::<bed_api::workbench::PanelPresentation>(owner)
                .is_some()
            {
                world
                    .entity_mut(owner)
                    .insert(bed_workbench::PanelDirty(snapshot.dirty));
            }
        }
    }
    let models = world
        .query::<(Entity, &models::DocumentBinding)>()
        .iter(world)
        .map(|(entity, binding)| (entity, binding.0))
        .collect::<Vec<_>>();
    for (entity, document) in models {
        if let Some(dirty) =
            models::model_document(world, document).map(|document| document.dirty())
            && let Some(owner) = bed_api::workbench::panel_owner(world, entity)
        {
            world
                .entity_mut(owner)
                .insert(bed_workbench::PanelDirty(dirty));
        }
    }
    let focused = world.resource::<WorkbenchLayout>().focused;
    let frontend = focused.and_then(|entity| {
        world
            .get::<bed_api::workbench::PanelFrontend>(entity)
            .map_or(Some(entity), |frontend| Some(frontend.0))
    });
    let active = frontend.and_then(|entity| {
        world
            .get::<PanelBinding>(entity)
            .and_then(|binding| binding.document.map(|document| (document, binding.view)))
            .or_else(|| {
                world
                    .get::<models::DocumentBinding>(entity)
                    .map(|binding| (binding.0, None))
            })
    });
    let mut frame = world.resource_mut::<DocumentFrame>();
    frame.active_document = active.map(|(document, _)| document);
    frame.active_view = active.and_then(|(_, view)| view);
}

/// Validate every document before quitting. Application-specific prompts and
/// persistence can choose a close policy; a failed save leaves all panels live.
pub fn quit(world: &mut World) -> Result<(), String> {
    assert!(
        !bed_workbench::frontend_active(world),
        "quit resolves after drawing"
    );
    admit(world);
    if crate::operation_barrier_active(world)
        || !world.resource::<HostRequests>().saves.is_empty()
        || !world.resource::<HostRequests>().commands.is_empty()
        || !world.resource::<HostRequests>().settings.is_empty()
        || !world.resource::<HostRequests>().pending.is_empty()
        || world
            .resource::<WorkbenchRequests>()
            .next_sequence()
            .is_some()
        || bed_workbench::waiting_for_layout(world)
    {
        return Err("Wait for pending document and layout operations before quitting".into());
    }
    if let Some(save) = world.resource::<LifecycleSaves>().quit {
        let Some(result) = crate::poll_document_save(world, save) else {
            return Err("Waiting for documents to finish saving".into());
        };
        world.resource_mut::<LifecycleSaves>().quit = None;
        result?;
    }
    let documents = crate::transitions::transition_documents(world);
    commit_document_drafts(world, &documents)?;
    let mut dirty = documents
        .into_iter()
        .filter(|document| {
            world
                .non_send::<DocumentSession>()
                .snapshot(*document)
                .is_ok_and(|snapshot| snapshot.dirty)
        })
        .collect::<Vec<_>>();
    dirty.extend(
        world
            .query::<&models::ModelDocument>()
            .iter(world)
            .filter(|document| document.dirty())
            .map(models::ModelDocument::id),
    );
    let policy = world.resource::<DocumentClosePolicy>().0;
    if !dirty.is_empty() {
        match policy {
            ClosePolicy::Cancel => {
                return Err("Save or discard changed documents before quitting".into());
            }
            ClosePolicy::Save => {
                let targets = dirty
                    .into_iter()
                    .map(|document| crate::DocumentSaveTarget {
                        document,
                        destination: None,
                    })
                    .collect::<Vec<_>>();
                let save = crate::begin_document_save(world, &targets)?;
                if let Some(result) = crate::poll_document_save(world, save) {
                    result?;
                } else {
                    world.resource_mut::<LifecycleSaves>().quit = Some(save);
                    return Err("Waiting for documents to finish saving".into());
                }
            }
            ClosePolicy::Discard => {}
        }
    }
    world
        .non_send_mut::<DocumentSession>()
        .shutdown(ClosePolicy::Discard)
        .map_err(|error| error.to_string())?;
    world.resource_mut::<CoreExiting>().0 = true;
    Ok(())
}

fn prepare_panel(world: &mut World, entity: Entity) -> Result<(), String> {
    if *world.resource::<CorePresentationMode>() == CorePresentationMode::Application {
        return Ok(());
    }
    let record = world
        .get::<bed_workbench::PanelRecord>(entity)
        .ok_or("Panel record is unavailable")?
        .serialized
        .clone();
    let Some(path) = record.get("path").and_then(Value::as_str) else {
        if let Some(id) = record["document"].as_u64() {
            let session = world.non_send::<DocumentSession>().session_id().0;
            if record["document_session"].as_u64() != Some(session)
                || record["document_origin"].as_str()
                    != Some(
                        world
                            .resource::<LiveDocumentOrigin>()
                            .0
                            .to_string()
                            .as_str(),
                    )
            {
                return Err(
                    "Untitled documents require application restoration after a session change"
                        .into(),
                );
            }
            let document = DocumentId(id);
            world
                .non_send::<DocumentSession>()
                .snapshot(document)
                .map_err(|error| error.to_string())?;
            world.entity_mut(entity).insert(PanelBinding {
                input: PanelInput::Document(document),
                document: Some(document),
                session_id: session,
                ..Default::default()
            });
        }
        return Ok(());
    };
    let document = match record.get("backing").and_then(Value::as_str) {
        Some("local_file") => None,
        _ => {
            let kind = if record.get("document_kind").and_then(Value::as_str) == Some("bytes") {
                bed_documents::DocumentKind::Bytes
            } else {
                bed_documents::DocumentKind::Text
            };
            Some(
                world
                    .non_send_mut::<DocumentSession>()
                    .open_file_with_kind(Path::new(path), kind)
                    .map_err(|error| error.to_string())?,
            )
        }
    };
    let session = world.non_send::<DocumentSession>().session_id().0;
    world.entity_mut(entity).insert(PanelBinding {
        input: document.map_or_else(|| PanelInput::LocalFile(path.into()), PanelInput::Document),
        document,
        session_id: session,
        ..Default::default()
    });
    Ok(())
}
fn notice(world: &mut World, source: Option<Entity>, message: impl Into<String>) {
    world.write_message(CoreNotice {
        source,
        message: message.into(),
    });
}
fn admit(world: &mut World) {
    let requests = world.resource_scope(|world, mut queue: Mut<HostRequests>| {
        queue
            .cursor
            .read(world.resource::<Messages<HostRequest>>())
            .cloned()
            .collect::<Vec<_>>()
    });
    world.resource_mut::<HostRequests>().pending.extend(
        requests
            .into_iter()
            .map(|request| Accepted { id: None, request }),
    );
    let settings = world.resource_scope(|world, mut queue: Mut<HostRequests>| {
        queue
            .settings_cursor
            .read(world.resource::<Messages<bed_api::settings::SettingsRequest>>())
            .cloned()
            .collect::<Vec<_>>()
    });
    world
        .resource_mut::<HostRequests>()
        .settings
        .extend(settings);
    world
        .resource_mut::<HostRequests>()
        .settings
        .make_contiguous()
        .sort_by_key(|setting| setting.sequence);
    for intent in world.resource_mut::<HostEffects>().take() {
        let request = capture_host_request_area(world, intent.source, intent.action, intent.area);
        world
            .resource_mut::<HostRequests>()
            .pending
            .push_back(Accepted { id: None, request });
    }
    world
        .resource_mut::<HostRequests>()
        .pending
        .make_contiguous()
        .sort_by_key(|accepted| accepted.request.sequence());
}
fn continue_command(world: &mut World, mut command: PendingCommand) {
    if command.outcome.is_none()
        && let Some(id) = command.id
        && let Some(result) = world.resource_mut::<WorkbenchRequests>().take_result(id)
    {
        command.outcome = Some(result);
    }
    command
        .saves
        .retain(|save| match crate::poll_document_save(world, *save) {
            Some(Ok(_)) => false,
            Some(Err(error)) => {
                command.error.get_or_insert(error);
                false
            }
            None => true,
        });
    if let Some(token) = command.barrier {
        match crate::barrier_status(world, token).expect("Admitted command barrier exists") {
            crate::OperationBarrierStatus::Complete => {
                crate::files::take_opened(world, token);
                command.barrier = None;
            }
            crate::OperationBarrierStatus::Failed(error) => {
                command.error.get_or_insert(error);
                command.barrier = None;
            }
            _ => {}
        }
    }
    if command.error.is_some() || command.outcome.as_ref().is_some_and(Result::is_err) {
        command.effects.clear();
    }
    if command.barrier.is_none() && command.saves.is_empty() {
        while let Some(request) = command.effects.pop_front() {
            match apply_document_request(world, request) {
                Ok(HostOutcome::Pending(save)) => {
                    command.saves.push(save);
                    break;
                }
                Ok(HostOutcome::Transition(token)) => {
                    command.barrier = Some(token);
                    break;
                }
                Err(error) => {
                    command.error.get_or_insert(error);
                    command.effects.clear();
                    break;
                }
                _ => {}
            }
            if let Some(token) = crate::active_operation_barrier(world) {
                command.barrier = Some(token);
                break;
            }
        }
    }
    if command.effects.is_empty()
        && command.saves.is_empty()
        && command.barrier.is_none()
        && (command.id.is_none() || command.outcome.is_some())
    {
        if let Some(id) = command.id {
            let outcome = command.error.map_or_else(
                || command.outcome.expect("Completed command has a result"),
                Err,
            );
            world
                .resource_mut::<WorkbenchRequests>()
                .complete(id, outcome);
        } else if let Some(error) = command.error {
            notice(world, None, error);
        }
    } else {
        world.resource_mut::<HostRequests>().commands.push(command);
    }
}

pub fn resolve_documents(world: &mut World) {
    assert!(
        !bed_workbench::frontend_active(world),
        "document requests run after drawing"
    );
    admit(world);
    crate::transitions::poll_barrier_admission(world);
    let transitions = std::mem::take(&mut world.resource_mut::<HostRequests>().transitions);
    for (receipt, token) in transitions {
        let result = match crate::barrier_status(world, token) {
            Some(crate::OperationBarrierStatus::Complete) => {
                Some(Ok(crate::files::take_opened(world, token)
                    .map_or(HostOutcome::Applied, HostOutcome::Opened)))
            }
            Some(crate::OperationBarrierStatus::Failed(error)) => Some(Err(error)),
            _ => None,
        };
        if let Some(result) = result {
            if let Some(receipt) = receipt {
                world
                    .resource_mut::<HostRequests>()
                    .results
                    .insert(receipt, result);
            } else if let Err(error) = result {
                notice(world, None, error);
            }
        } else {
            world
                .resource_mut::<HostRequests>()
                .transitions
                .push((receipt, token));
        }
    }
    bed_workbench::poll_request_results(world);
    let commands = std::mem::take(&mut world.resource_mut::<HostRequests>().commands);
    for command in commands {
        continue_command(world, command);
    }
    let pending = std::mem::take(&mut world.resource_mut::<HostRequests>().saves);
    for (receipt, save) in pending {
        if let Some(result) = crate::poll_document_save(world, save) {
            let result = result.map(|saved| HostOutcome::Saved {
                documents: saved.iter().map(|saved| saved.document).collect(),
                revisions: saved.iter().map(|saved| saved.revision).collect(),
            });
            if let Some(receipt) = receipt {
                world
                    .resource_mut::<HostRequests>()
                    .results
                    .insert(receipt, result);
            } else if let Err(error) = result {
                notice(world, None, error);
            }
        } else {
            world
                .resource_mut::<HostRequests>()
                .saves
                .push((receipt, save));
        }
    }
    while !bed_workbench::waiting_for_layout(world)
        && !world.resource::<CoreExiting>().0
        && !crate::operation_barrier_active(world)
        && !world
            .resource::<HostRequests>()
            .commands
            .iter()
            .any(|command| !command.effects.is_empty())
    {
        let document_sequence = world
            .resource::<HostRequests>()
            .pending
            .front()
            .map(|entry| entry.request.sequence());
        let panel_sequence = world.resource::<WorkbenchRequests>().next_sequence();
        let setting_sequence = world
            .resource::<HostRequests>()
            .settings
            .front()
            .map(|setting| setting.sequence);
        if setting_sequence.is_some_and(|sequence| {
            document_sequence.is_none_or(|next| sequence < next)
                && panel_sequence.is_none_or(|next| sequence < next)
        }) {
            let setting = world
                .resource_mut::<HostRequests>()
                .settings
                .pop_front()
                .unwrap();
            if let Err(error) = crate::settings::apply_settings_request(world, &setting) {
                notice(world, None, error);
            }
            continue;
        }
        if panel_sequence.is_some()
            && (document_sequence.is_none() || panel_sequence < document_sequence)
        {
            let queued = world
                .resource_mut::<WorkbenchRequests>()
                .take_next()
                .unwrap();
            let receipt = queued.id;
            bed_workbench::resolve_request(world, queued);
            // The outbox is empty at command entry. Its newly produced effects
            // finish before the next independent command.
            let effects = world
                .resource_mut::<HostEffects>()
                .take()
                .into_iter()
                .map(|intent| {
                    capture_host_request_area(world, intent.source, intent.action, intent.area)
                })
                .collect();
            let outcome =
                receipt.and_then(|id| world.resource_mut::<WorkbenchRequests>().take_result(id));
            continue_command(
                world,
                PendingCommand {
                    id: receipt,
                    outcome,
                    effects,
                    saves: Vec::new(),
                    barrier: None,
                    error: None,
                },
            );
        } else if let Some(accepted) = world.resource_mut::<HostRequests>().pending.pop_front() {
            let source = accepted.request.source();
            let result = apply_document_request(world, accepted.request);
            if let Ok(HostOutcome::Transition(token)) = &result {
                world
                    .resource_mut::<HostRequests>()
                    .transitions
                    .push((accepted.id, *token));
            } else if let Ok(HostOutcome::Pending(save)) = &result {
                world
                    .resource_mut::<HostRequests>()
                    .saves
                    .push((accepted.id, *save));
            } else if let Some(id) = accepted.id {
                world
                    .resource_mut::<HostRequests>()
                    .results
                    .insert(id, result);
            } else if let Err(error) = result {
                notice(world, source, error);
            }
        } else {
            break;
        }
    }
    publish_documents(world);
}

pub fn open_document_panel(
    world: &mut World,
    panel_type: &str,
    document: DocumentId,
    state: Value,
    area: Option<u32>,
) -> Result<Entity, String> {
    assert!(
        !bed_workbench::frontend_active(world),
        "document panels open after drawing"
    );
    let snapshot = world
        .non_send::<DocumentSession>()
        .snapshot(document)
        .map_err(|error| error.to_string())?;
    publish_documents(world);
    let factory = world
        .resource::<bed_api::workbench::WorkbenchRegistry>()
        .panel(panel_type)
        .ok_or_else(|| format!("Panel unavailable: {panel_type}"))?
        .factory;
    let mut serialized = json!({"panel_type":panel_type,"state":state,"document":document.0,"backing":"document","document_kind":if snapshot.kind == bed_documents::DocumentKind::Text { "text" } else { "bytes" }});
    if !snapshot.path.is_empty() {
        serialized["path"] = json!(snapshot.path);
    }
    let session = world.non_send::<DocumentSession>().session_id().0;
    serialized["document_session"] = json!(session);
    serialized["document_origin"] = json!(world.resource::<LiveDocumentOrigin>().0.to_string());
    let entity = bed_workbench::spawn_panel(world, serialized, area, None)?;
    world.entity_mut(entity).insert(PanelBinding {
        input: PanelInput::Document(document),
        document: Some(document),
        session_id: session,
        ..Default::default()
    });
    let result = world
        .run_system_with(factory, bed_api::workbench::PanelCreation { entity, state })
        .map_err(|error| error.to_string())
        .and_then(|result| result);
    match result {
        Ok(frontend) => {
            bed_workbench::install_panel_frontend(world, entity, frontend)?;
            bed_workbench::focus_panel(world, entity);
            Ok(entity)
        }
        Err(error) => {
            bed_workbench::commit_close(world, entity);
            Err(error)
        }
    }
}
pub fn open_file(
    world: &mut World,
    path: &Path,
    viewer: Option<&str>,
    area: Option<u32>,
) -> Result<Entity, String> {
    assert!(
        !bed_workbench::frontend_active(world),
        "files open after drawing"
    );
    let explicit = viewer
        .map(|id| {
            world
                .resource::<bed_api::features::FeatureRegistry>()
                .viewer(id)
                .cloned()
                .ok_or_else(|| format!("File viewer unavailable: {id}"))
        })
        .transpose()?;
    if let Some(descriptor) = explicit
        .as_ref()
        .filter(|descriptor| descriptor.is_local_file())
    {
        return open_local_file_panel(world, path, descriptor, area);
    }
    let document = world
        .non_send_mut::<DocumentSession>()
        .open_file_auto(path)
        .map_err(|error| error.to_string())?;
    let kind = world
        .non_send::<DocumentSession>()
        .document_kind(document)
        .map_err(|error| error.to_string())?;
    let registry = world.resource::<bed_api::features::FeatureRegistry>();
    let descriptor = explicit.or_else(|| {
        registry
            .preferred_viewer_for_file(
                &path.to_string_lossy(),
                kind,
                &world
                    .resource::<bed_api::settings::SettingsSnapshot>()
                    .default_viewers,
            )
            .cloned()
    });
    let descriptor = descriptor.ok_or_else(|| format!("No file viewer for {}", path.display()))?;
    if descriptor.is_local_file() {
        return open_local_file_panel(world, path, &descriptor, area);
    }
    if !descriptor.supports_document_kind(kind) {
        return Err("This viewer does not support the document's content kind".into());
    }
    if let Some(existing) = world
        .query::<(Entity, &PanelBinding, &bed_workbench::PanelRecord)>()
        .iter(world)
        .find_map(|(entity, binding, record)| {
            (binding.document == Some(document)
                && record.panel_type == descriptor.panel_type
                && bed_api::workbench::panel_active(world, entity))
            .then_some(entity)
        })
    {
        bed_workbench::focus_panel(world, existing);
        return Ok(existing);
    }
    let entity = open_document_panel(world, descriptor.panel_type, document, Value::Null, area)?;
    world.get_mut::<PanelBinding>(entity).unwrap().viewer = Some(descriptor.id.into());
    world
        .get_mut::<bed_workbench::PanelRecord>(entity)
        .unwrap()
        .serialized["viewer"] = json!(descriptor.id);
    Ok(entity)
}
fn open_local_file_panel(
    world: &mut World,
    path: &Path,
    viewer: &bed_api::documents::FileViewer,
    area: Option<u32>,
) -> Result<Entity, String> {
    if world.resource::<bed_files::FileServices>().is_remote() {
        return Err("This file viewer requires a local filesystem".into());
    }
    let path = std::fs::canonicalize(path).map_err(|error| error.to_string())?;
    if !path.is_file() {
        return Err("File viewer requires a regular file".into());
    }
    let entity = bed_workbench::open_panel(
        world,
        json!({"panel_type":viewer.panel_type,"state":null,"path":path.to_string_lossy(),"backing":"local_file","viewer":viewer.id}),
        area,
        None,
    )?;
    if let Some(error) = world
        .get::<bed_workbench::PanelRecord>(entity)
        .and_then(|record| record.unavailable.clone())
    {
        return Err(error);
    }
    if let Some(mut binding) = world.get_mut::<PanelBinding>(entity) {
        binding.viewer = Some(viewer.id.into());
    }
    Ok(entity)
}

fn preview_file(
    world: &mut World,
    path: &Path,
    viewer: Option<&str>,
    area: u32,
) -> Result<Entity, String> {
    let previous = world
        .resource::<WorkbenchLayout>()
        .tiling
        .areas
        .iter()
        .find(|group| group.area == area)
        .and_then(|group| group.preview);
    let existing = world
        .query::<(Entity, &bed_api::workbench::PanelPresentation)>()
        .iter(world)
        .map(|(entity, _)| entity)
        .collect::<Vec<_>>();
    let panel = open_file(world, path, viewer, Some(area))?;
    if existing.contains(&panel) {
        return Ok(panel);
    }
    if let Some(previous) = previous.and_then(|id| {
        world
            .query::<(Entity, &bed_api::workbench::PanelPresentation)>()
            .iter(world)
            .find_map(|(entity, panel)| (panel.id == id).then_some(entity))
    }) {
        let document = world
            .get::<PanelBinding>(previous)
            .and_then(|binding| binding.document);
        let dirty = document
            .map(|document| document_dirty(world, document))
            .transpose()?
            .unwrap_or_else(|| {
                world
                    .get::<bed_workbench::PanelDirty>(previous)
                    .is_some_and(|dirty| dirty.0)
            });
        if dirty {
            bed_workbench::keep_panel(world, previous);
        } else {
            bed_workbench::request_close(world, previous)?;
        }
    }
    bed_workbench::mark_panel_preview(world, panel)?;
    Ok(panel)
}

pub fn commit_document_drafts(world: &mut World, documents: &[DocumentId]) -> Result<(), String> {
    assert!(
        !bed_workbench::frontend_active(world),
        "drafts commit after drawing"
    );
    let targets = world
        .query::<(
            Entity,
            Option<&PanelBinding>,
            Option<&models::DocumentBinding>,
            &bed_api::workbench::PanelActions,
        )>()
        .iter(world)
        .filter(|(_, text, model, _)| {
            text.and_then(|binding| binding.document)
                .or(model.map(|binding| binding.0))
                .is_some_and(|document| documents.contains(&document))
        })
        .map(|(entity, _, _, handler)| (entity, handler.0))
        .collect::<Vec<_>>();
    for (entity, handler) in targets {
        let previous = world.resource_mut::<HostEffects>().take();
        let result = world
            .run_system_with(
                handler,
                bed_api::workbench::PanelActionInvocation {
                    entity,
                    action: bed_api::workbench::PanelAction::CommitEdit,
                },
            )
            .map_err(|error| error.to_string())
            .and_then(|result| result.map(|_| ()));
        let effects = world.resource_mut::<HostEffects>().take();
        for intent in previous {
            world.resource_mut::<HostEffects>().push_effect(intent);
        }
        result?;
        for intent in effects {
            let target = match &intent.action {
                HostAction::ApplyEdits { document, .. }
                | HostAction::ApplyEditsWithResult { document, .. } => Some(*document),
                _ => None,
            };
            if intent.source != Some(entity)
                || target
                    != world
                        .get::<PanelBinding>(entity)
                        .and_then(|binding| binding.document)
                        .or_else(|| {
                            world
                                .get::<models::DocumentBinding>(entity)
                                .map(|binding| binding.0)
                        })
            {
                return Err("Draft commit may only produce its own document edits".into());
            }
            let request = capture_host_request(world, Some(entity), intent.action);
            apply_document_request(world, request)?;
        }
    }
    Ok(())
}

/// Begin an unformatted lifecycle save. Poll the returned receipt for the
/// revision that reached storage; a later edit may keep the document dirty.
pub fn save_document(
    world: &mut World,
    document: DocumentId,
) -> Result<crate::DocumentSaveId, String> {
    crate::begin_document_save(
        world,
        &[crate::DocumentSaveTarget {
            document,
            destination: None,
        }],
    )
}
pub fn save_document_as(
    world: &mut World,
    document: DocumentId,
    path: &Path,
) -> Result<crate::DocumentSaveId, String> {
    crate::begin_document_save(
        world,
        &[crate::DocumentSaveTarget {
            document,
            destination: Some(path.to_owned()),
        }],
    )
}

/// One revision representation for default documents and typed model documents.
pub fn document_revision(world: &World, document: DocumentId) -> Result<Revision, String> {
    if let Some(model) = models::model_document(world, document) {
        if model.session_id() != world.non_send::<DocumentSession>().session_id() {
            return Err("Document session was replaced".into());
        }
        return Ok((0, model.revision()));
    }
    world
        .non_send::<DocumentSession>()
        .document_revision(document)
        .map_err(|error| error.to_string())
}
pub fn document_dirty(world: &World, document: DocumentId) -> Result<bool, String> {
    if let Some(model) = models::model_document(world, document) {
        if model.session_id() != world.non_send::<DocumentSession>().session_id() {
            return Err("Document session was replaced".into());
        }
        return Ok(model.dirty());
    }
    world
        .non_send::<DocumentSession>()
        .snapshot(document)
        .map(|document| document.dirty)
        .map_err(|error| error.to_string())
}
pub fn document_path(
    world: &World,
    document: DocumentId,
) -> Result<Option<std::path::PathBuf>, String> {
    if let Some(model) = models::model_document(world, document) {
        if model.session_id() != world.non_send::<DocumentSession>().session_id() {
            return Err("Document session was replaced".into());
        }
        return Ok(model.path.clone());
    }
    let snapshot = world
        .non_send::<DocumentSession>()
        .snapshot(document)
        .map_err(|error| error.to_string())?;
    Ok((!snapshot.path.is_empty()).then(|| snapshot.path.into()))
}

/// Retire contents only after the final panel binding has been released.
/// Save/discard approval belongs to the operation that retired those panels.
pub fn retire_document(world: &mut World, document: DocumentId) -> Result<bool, String> {
    assert!(
        !bed_workbench::frontend_active(world),
        "documents retire after drawing"
    );
    let bound = world
        .query::<(Option<&PanelBinding>, Option<&models::DocumentBinding>)>()
        .iter(world)
        .any(|(binding, model)| {
            binding.is_some_and(|binding| binding.document == Some(document))
                || model.is_some_and(|binding| binding.0 == document)
        });
    if bound {
        return Ok(false);
    }
    if models::model_entity(world, document).is_some() {
        return Ok(models::retire_model_document(world, document));
    }
    world
        .non_send_mut::<DocumentSession>()
        .close_document(document, ClosePolicy::Discard)
        .map_err(|error| error.to_string())
}

fn close_panel(world: &mut World, panel: Entity) -> Result<bool, String> {
    let closing =
        bed_workbench::closing_panels(world).map_or_else(|| vec![panel], <[Entity]>::to_vec);
    let closing_frontends = world
        .query::<(Entity, Option<&bed_api::workbench::PanelOwner>)>()
        .iter(world)
        .filter(|(entity, owner)| {
            closing.contains(entity) || owner.is_some_and(|owner| closing.contains(&owner.0))
        })
        .map(|(entity, _)| entity)
        .collect::<Vec<_>>();
    let mut documents = closing_frontends
        .iter()
        .filter_map(|entity| {
            world
                .get::<PanelBinding>(*entity)
                .and_then(|binding| binding.document)
                .or_else(|| {
                    world
                        .get::<models::DocumentBinding>(*entity)
                        .map(|binding| binding.0)
                })
        })
        .collect::<Vec<_>>();
    documents.sort_by_key(|document| document.0);
    documents.dedup();
    commit_document_drafts(world, &documents)?;
    let policy = world.resource::<DocumentClosePolicy>().0;
    if let Some(save) = world.resource::<LifecycleSaves>().close {
        let Some(result) = crate::poll_document_save(world, save) else {
            return Ok(false);
        };
        world.resource_mut::<LifecycleSaves>().close = None;
        result?;
    }
    let mut dirty_documents = Vec::new();
    for document in documents {
        let remaining = world
            .query::<(
                Entity,
                Option<&PanelBinding>,
                Option<&models::DocumentBinding>,
            )>()
            .iter(world)
            .any(|(entity, binding, model)| {
                !closing_frontends.contains(&entity)
                    && (binding.is_some_and(|binding| binding.document == Some(document))
                        || model.is_some_and(|binding| binding.0 == document))
            });
        if remaining {
            continue;
        }
        let dirty = document_dirty(world, document)?;
        if dirty {
            match policy {
                ClosePolicy::Cancel => {
                    return Err("Save or discard the document before closing".into());
                }
                ClosePolicy::Save => dirty_documents.push(document),
                ClosePolicy::Discard => {}
            }
        }
    }
    if !dirty_documents.is_empty() {
        let targets = dirty_documents
            .into_iter()
            .map(|document| crate::DocumentSaveTarget {
                document,
                destination: None,
            })
            .collect::<Vec<_>>();
        let save = crate::begin_document_save(world, &targets)?;
        if let Some(result) = crate::poll_document_save(world, save) {
            result?;
        } else {
            world.resource_mut::<LifecycleSaves>().close = Some(save);
            return Ok(false);
        }
    }
    Ok(true)
}

fn retire_panel(world: &mut World, panel: Entity) {
    let frontends = world
        .query::<(Entity, Option<&bed_api::workbench::PanelOwner>)>()
        .iter(world)
        .filter(|(entity, owner)| *entity == panel || owner.is_some_and(|owner| owner.0 == panel))
        .map(|(entity, _)| entity)
        .collect::<Vec<_>>();
    for entity in frontends {
        if let Some(handler) = world.get::<bed_api::workbench::PanelClose>(entity).copied()
            && let Err(error) = world
                .run_system_with(handler.0, entity)
                .map_err(|error| error.to_string())
                .and_then(|result| result)
        {
            notice(world, Some(entity), error);
        }
        if let Some(binding) = world.entity_mut(entity).take::<PanelBinding>()
            && let Some(view) = binding.view
            && world
                .non_send::<DocumentSession>()
                .document_for_view(view)
                .is_some()
        {
            world.non_send_mut::<DocumentSession>().detach_view(view);
        }
        world.entity_mut(entity).remove::<models::DocumentBinding>();
    }
}

/// Apply one revision-checked text or byte edit batch. The shared document
/// owner preserves I/O error kinds for integrations with typed failure policy.
pub fn apply_edits(
    world: &mut World,
    document: DocumentId,
    revision: Revision,
    edits: &[bed_documents::ByteEdit],
) -> std::io::Result<()> {
    assert!(
        !bed_workbench::frontend_active(world),
        "document edits run after drawing"
    );
    world
        .non_send_mut::<DocumentSession>()
        .apply_edits(document, revision, edits)
}

pub fn apply_document_request(
    world: &mut World,
    request: HostRequest,
) -> Result<HostOutcome, String> {
    if request.session_id() != world.non_send::<DocumentSession>().session_id() {
        return Err("Document session was replaced".into());
    }
    if let Some(source) = request.source() {
        let owner = bed_api::workbench::panel_owner(world, source)
            .ok_or("Originating panel is unavailable")?;
        if [source, owner].into_iter().any(|entity| {
            world
                .get::<bed_workbench::PanelRecord>(entity)
                .is_some_and(|record| record.unavailable.is_some())
        }) {
            return Err("Originating panel failed".into());
        }
    }
    if request.is_layout_scoped()
        && request
            .source()
            .is_some_and(|source| !bed_api::workbench::panel_active(world, source))
    {
        return Err("Originating panel's layout is inactive".into());
    }
    if request.is_layout_scoped()
        && request.layout_epoch() != world.resource::<bed_workbench::WorkbenchLayoutEpoch>().0
    {
        return Err("Originating layout is no longer active".into());
    }
    #[cfg(feature = "language-services")]
    if let HostAction::NavigateLocation {
        origin: Some(origin),
        ..
    } = request.action()
        && !world
            .non_send::<DocumentSession>()
            .accepts_lsp_origin(origin)
    {
        return Ok(HostOutcome::Applied);
    }
    if let Some(token) = crate::files::defer_remote_open(world, &request)? {
        return Ok(HostOutcome::Transition(token));
    }
    match request.action().clone() {
        HostAction::OperationBarrier { token } => {
            crate::transitions::activate_operation_barrier(world, token)?
        }
        HostAction::OpenDirectory { location } => {
            return crate::files::open_directory(world, location).map(HostOutcome::Transition);
        }
        HostAction::ChooseDirectory => {
            if world.resource::<bed_files::FileServices>().is_remote() {
                return Err("Choose a remote directory through its filesystem browser".into());
            }
            if let Some(path) = rfd::FileDialog::new()
                .set_directory(&world.resource::<bed_files::FileContext>().directory)
                .pick_folder()
            {
                return crate::files::open_directory(
                    world,
                    bed_files::FileLocation {
                        provider: bed_files::FileProviderId::Local,
                        path,
                    },
                )
                .map(HostOutcome::Transition);
            }
        }
        HostAction::ApplyEdits {
            document,
            revision,
            edits,
        } => apply_edits(world, document, revision, &edits).map_err(|error| error.to_string())?,
        HostAction::ApplyEditsWithResult {
            token,
            document,
            revision,
            edits,
        } => {
            let result = apply_edits(world, document, revision, &edits)
                .map_err(|error| error.to_string())
                .and_then(|_| {
                    world
                        .non_send::<DocumentSession>()
                        .document_revision(document)
                        .map_err(|error| error.to_string())
                });
            if let Some(source) = request.source().and_then(|source| {
                world
                    .get::<bed_api::documents::PanelEdits>(source)
                    .map(|handler| (source, handler.0))
            }) {
                world
                    .run_system_with(
                        source.1,
                        bed_api::documents::PanelEditResult {
                            entity: source.0,
                            token,
                            result: result.clone(),
                        },
                    )
                    .map_err(|error| error.to_string())?;
            }
            result?;
        }
        HostAction::ApplyModuleEditsWithResult {
            recipient,
            token,
            document,
            revision,
            edits,
        } => {
            let result = apply_edits(world, document, revision, &edits)
                .map_err(|error| error.to_string())
                .and_then(|_| {
                    world
                        .non_send::<DocumentSession>()
                        .document_revision(document)
                        .map_err(|error| error.to_string())
                });
            world.write_message(bed_api::documents::DocumentReply::Edit {
                recipient,
                token,
                result: result.clone(),
            });
            result?;
        }
        HostAction::Undo { document } => models::undo_document(world, document)?,
        HostAction::Redo { document } => models::redo_document(world, document)?,
        HostAction::Save { document } => {
            #[cfg(feature = "language-services")]
            let save = crate::begin_explicit_document_save(
                world,
                document,
                world
                    .resource::<bed_api::settings::SettingsSnapshot>()
                    .settings["bed.editor"]["lsp_format_on_save"]
                    .as_bool()
                    .unwrap_or(true),
            )?;
            #[cfg(not(feature = "language-services"))]
            let save = crate::begin_document_save(
                world,
                &[crate::DocumentSaveTarget {
                    document,
                    destination: None,
                }],
            )?;
            return document_save_outcome(world, save);
        }
        HostAction::SaveAsDialog { document } => {
            if world.resource::<bed_files::FileServices>().is_remote() {
                return Err("Choose an SSH save destination through its filesystem browser".into());
            }
            if let Some(path) = rfd::FileDialog::new().save_file() {
                let save = crate::begin_document_save(
                    world,
                    &[crate::DocumentSaveTarget {
                        document,
                        destination: Some(path),
                    }],
                )?;
                return document_save_outcome(world, save);
            }
        }
        HostAction::SaveDocumentsWithResult {
            recipient,
            token,
            documents,
        } => {
            let targets = documents
                .into_iter()
                .map(|document| crate::DocumentSaveTarget {
                    document,
                    destination: None,
                })
                .collect::<Vec<_>>();
            match crate::begin_document_save(world, &targets) {
                Ok(save) => {
                    crate::saves::attach_document_save_reply(world, save, recipient, token);
                    return document_save_outcome(world, save);
                }
                Err(error) => {
                    world.write_message(bed_api::documents::DocumentReply::Save {
                        recipient,
                        token,
                        result: Err(error.clone()),
                    });
                    return Err(error);
                }
            }
        }
        HostAction::CommitDocumentsWithResult {
            recipient,
            token,
            documents,
        } => {
            let result = (|| {
                commit_document_drafts(world, &documents)?;
                let mut saved = Vec::new();
                for document in documents {
                    if world.non_send::<DocumentSession>().save_pending(document) {
                        return Err("Document has a pending write".into());
                    }
                    saved.push(bed_api::documents::SavedDocument {
                        document,
                        revision: document_revision(world, document)?,
                    });
                }
                Ok::<_, String>(saved)
            })();
            world.write_message(bed_api::documents::DocumentReply::Save {
                recipient,
                token,
                result: result.clone(),
            });
            result?;
        }
        HostAction::OpenFile { path, viewer } | HostAction::OpenFileIn { path, viewer, .. } => {
            return open_file(
                world,
                Path::new(&path),
                viewer.as_deref(),
                Some(request.area()),
            )
            .map(HostOutcome::Opened);
        }
        HostAction::PreviewFileIn { path, viewer, .. } => {
            return preview_file(world, Path::new(&path), viewer.as_deref(), request.area())
                .map(HostOutcome::Opened);
        }
        HostAction::OpenTerminalFiles { paths } => {
            for path in paths {
                open_file(world, &path, None, Some(request.area()))?;
            }
        }
        HostAction::OpenTerminalLink { path, position } => {
            if let Some((row, column)) = position {
                return reveal_source(
                    world,
                    &path,
                    (row, column),
                    true,
                    false,
                    request.area(),
                    false,
                )
                .map(HostOutcome::Opened);
            }
            return open_file(world, &path, None, Some(request.area())).map(HostOutcome::Opened);
        }
        HostAction::RevealSource {
            path,
            row,
            column,
            center,
        } => {
            return reveal_source(
                world,
                Path::new(&path),
                (row, column),
                center,
                false,
                request.area(),
                false,
            )
            .map(HostOutcome::Opened);
        }
        HostAction::PreviewSource {
            path,
            row,
            column,
            center,
        } => {
            return reveal_source(
                world,
                Path::new(&path),
                (row, column),
                center,
                true,
                request.area(),
                false,
            )
            .map(HostOutcome::Opened);
        }
        #[cfg(feature = "language-services")]
        HostAction::NavigateLocation {
            path,
            row,
            utf16_column,
            ..
        } => {
            return reveal_source(
                world,
                Path::new(&path),
                (row, utf16_column),
                true,
                true,
                request.area(),
                true,
            )
            .map(HostOutcome::Opened);
        }
        HostAction::OpenFileDialog { viewer } => {
            if world.resource::<bed_files::FileServices>().is_remote() {
                return Err("Open SSH files through their filesystem browser".into());
            }
            if let Some(path) = rfd::FileDialog::new().pick_file() {
                return open_file(world, &path, viewer.as_deref(), Some(request.area()))
                    .map(HostOutcome::Opened);
            }
        }
        HostAction::DropFile { path } => {
            let panel = open_file(world, Path::new(&path), None, Some(request.area()))?;
            bed_workbench::move_panel_to_area(world, panel, request.area())?;
            return Ok(HostOutcome::Opened(panel));
        }
        HostAction::OpenPanel {
            panel_type,
            document,
            state,
        } => {
            let entity = match document {
                Some(document) => {
                    open_document_panel(world, &panel_type, document, state, Some(request.area()))?
                }
                None => bed_workbench::open_panel(
                    world,
                    json!({"panel_type":panel_type,"state":state}),
                    Some(request.area()),
                    None,
                )?,
            };
            return Ok(HostOutcome::Opened(entity));
        }
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
            let existing = world
                .query::<(Entity, &bed_workbench::PanelRecord, Option<&PanelBinding>)>()
                .iter(world)
                .find_map(|(entity, record, binding)| {
                    (record.panel_type == panel_type
                        && document == binding.and_then(|binding| binding.document)
                        && bed_api::workbench::panel_active(world, entity))
                    .then_some(entity)
                });
            let entity = if let Some(entity) = existing {
                entity
            } else {
                match document {
                    Some(document) => open_document_panel(
                        world,
                        &panel_type,
                        document,
                        state,
                        Some(request.area()),
                    )?,
                    None => bed_workbench::open_panel(
                        world,
                        json!({"panel_type":panel_type,"state":state}),
                        Some(request.area()),
                        None,
                    )?,
                }
            };
            bed_workbench::focus_panel(world, entity);
            if let Some(action) = action {
                let handler = world
                    .get::<bed_api::workbench::PanelActions>(entity)
                    .ok_or("Panel does not support this action")?
                    .0;
                let previous = world.resource_mut::<HostEffects>().take();
                let result = world
                    .run_system_with(
                        handler,
                        bed_api::workbench::PanelActionInvocation { entity, action },
                    )
                    .map_err(|error| error.to_string())
                    .and_then(|result| result);
                let effects = world.resource_mut::<HostEffects>().take();
                for intent in previous {
                    world.resource_mut::<HostEffects>().push_effect(intent);
                }
                result?;
                for intent in effects {
                    if intent.source != Some(entity) {
                        return Err("Panel actions may only produce their own effects".into());
                    }
                    apply_document_request(
                        world,
                        capture_host_request_area(world, intent.source, intent.action, intent.area),
                    )?;
                }
            }
            return Ok(HostOutcome::Opened(entity));
        }
        HostAction::ClosePanels { panel_type } => {
            let panels = world
                .query::<(
                    Entity,
                    &bed_workbench::PanelRecord,
                    &bed_api::workbench::PanelPresentation,
                )>()
                .iter(world)
                .filter_map(|(entity, record, _)| {
                    (record.panel_type == panel_type
                        && bed_api::workbench::panel_active(world, entity))
                    .then_some(entity)
                })
                .collect::<Vec<_>>();
            if !bed_workbench::request_close_many(world, &panels)? {
                return Err("Panel close is awaiting application approval".into());
            }
        }
        HostAction::FocusView { view } => {
            let entity = world
                .query::<(Entity, &PanelBinding)>()
                .iter(world)
                .find_map(|(entity, binding)| (binding.view == Some(view)).then_some(entity))
                .ok_or("Editor view is unavailable")?;
            bed_workbench::focus_panel(world, entity);
        }
        HostAction::Navigate {
            document,
            byte_offset,
            revision,
        } => {
            let session = world.non_send::<DocumentSession>();
            if session
                .document_revision(document)
                .map_err(|error| error.to_string())?
                != revision
            {
                return Err("Document changed before navigation".into());
            }
            let view = request
                .source()
                .and_then(|source| world.get::<PanelBinding>(source))
                .filter(|binding| binding.document == Some(document))
                .and_then(|binding| binding.view)
                .or_else(|| {
                    world
                        .resource::<DocumentFrame>()
                        .active_view
                        .filter(|view| session.document_for_view(*view) == Some(document))
                })
                .or_else(|| session.view_ids(document).into_iter().next())
                .ok_or("Document has no text view")?;
            let (row, col) = session
                .with_document(document, |state| state.row_col_from_offset(byte_offset))
                .map_err(|error| error.to_string())?;
            world
                .non_send_mut::<DocumentSession>()
                .with_commands(view, |commands| {
                    commands.set_cursor(
                        row,
                        col,
                        false,
                        bed_editing::text_commands::CursorReveal::Center,
                    )
                })
                .map_err(|error| error.to_string())?;
        }
        HostAction::Notify { message } => notice(world, request.source(), message),
        HostAction::Invalidate => {}
        _ => return Err("This operation requires an application integration".into()),
    }
    Ok(HostOutcome::Applied)
}

fn reveal_source(
    world: &mut World,
    path: &Path,
    position: (i32, i32),
    center: bool,
    preview: bool,
    area: u32,
    utf16: bool,
) -> Result<Entity, String> {
    let (row, column) = position;
    let viewer = world
        .resource::<bed_api::features::FeatureRegistry>()
        .default_viewer(bed_documents::DocumentKind::Text)
        .ok_or("No text viewer is installed")?
        .id;
    let panel = if preview {
        preview_file(world, path, Some(viewer), area)?
    } else {
        open_file(world, path, Some(viewer), Some(area))?
    };
    let binding = world
        .get::<PanelBinding>(panel)
        .ok_or("Source panel has no document binding")?;
    let view = binding.view.ok_or("Source panel has no text view")?;
    let column = if utf16 {
        world
            .non_send::<DocumentSession>()
            .with_document(
                binding.document.ok_or("Source panel has no document")?,
                |state| {
                    bed_editing::util::utf8::utf16_to_utf8_byte_offset(&state.line(row), column)
                },
            )
            .map_err(|error| error.to_string())?
    } else {
        column
    };
    world
        .non_send_mut::<DocumentSession>()
        .with_commands(view, |commands| {
            commands.set_cursor(
                row,
                column,
                false,
                if center {
                    bed_editing::text_commands::CursorReveal::Center
                } else {
                    bed_editing::text_commands::CursorReveal::Ensure
                },
            )
        })
        .map_err(|error| error.to_string())?;
    bed_workbench::focus_panel(world, panel);
    Ok(panel)
}

fn document_save_outcome(
    world: &mut World,
    save: crate::DocumentSaveId,
) -> Result<HostOutcome, String> {
    match crate::poll_document_save(world, save) {
        Some(result) => result.map(|saved| HostOutcome::Saved {
            documents: saved.iter().map(|saved| saved.document).collect(),
            revisions: saved.iter().map(|saved| saved.revision).collect(),
        }),
        None => Ok(HostOutcome::Pending(save)),
    }
}
