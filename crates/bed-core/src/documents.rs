//! Document requests resolve between UI scopes, with retained operation receipts.
use crate::models::{self, ModelOperation, ModelRegistry};
pub use bed_api::features::HostRequest;
use bed_api::{
    documents::{PanelBinding, PanelInput, Revision},
    features::{HostAction, HostEffects, HostFrame},
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
pub enum HostRequestMode {
    #[default]
    Core,
    Application,
}
#[derive(Resource, Default)]
pub(crate) struct CoreExiting(pub bool);
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
    let layout = world.resource::<WorkbenchLayout>();
    let area = if matches!(
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
    };
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
#[derive(Resource, Default)]
pub struct HostRequests {
    next: u64,
    pending: VecDeque<Accepted>,
    results: HashMap<HostRequestId, Result<HostOutcome, String>>,
    cursor: MessageCursor<HostRequest>,
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
        app.init_non_send::<DocumentSession>()
            .init_resource::<ModelRegistry>()
            .init_resource::<HostFrame>()
            .init_resource::<HostEffects>()
            .init_resource::<bed_api::features::FeatureRegistry>()
            .init_resource::<bed_api::workbench::CommandPresentations>()
            .init_resource::<HostRequests>()
            .init_resource::<HostRequestMode>()
            .init_resource::<CoreExiting>()
            .init_resource::<LiveDocumentOrigin>()
            .init_resource::<DocumentClosePolicy>()
            .insert_resource(WorkbenchRequestMode::External)
            .add_message::<HostRequest>()
            .add_message::<CoreNotice>()
            .add_message::<bed_api::documents::DocumentReply>()
            .add_message::<bed_api::documents::DocumentEvent>()
            .add_message::<bed_api::features::FeatureStateChanged>()
            .add_systems(
                PreUpdate,
                (
                    poll_documents.in_set(CoreSystems::Services),
                    publish_documents
                        .in_set(CoreSystems::Frame)
                        .after(CoreSystems::Services)
                        .before(bed_api::features::FeatureSystems::Prepare),
                    resolve_documents
                        .in_set(CoreSystems::Requests)
                        .after(bed_api::features::FeatureSystems::Prepare),
                ),
            );
        if *app.world().resource::<HostRequestMode>() == HostRequestMode::Core
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
    }
}
fn core_mode(world: &World) -> bool {
    *world.resource::<HostRequestMode>() == HostRequestMode::Core
}
fn poll_documents(world: &mut World) {
    if !core_mode(world) {
        return;
    }
    let report = world.non_send_mut::<DocumentSession>().tick();
    for event in report.events {
        world.write_message(bed_api::documents::DocumentEvent(event));
    }
    for error in report.errors {
        notice(world, None, error.message);
    }
}
fn publish_documents(world: &mut World) {
    if !core_mode(world) {
        return;
    }
    let result = world.resource_scope(|world, mut frame: Mut<HostFrame>| {
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
    let mut frame = world.resource_mut::<HostFrame>();
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
    if !world.resource::<HostRequests>().pending.is_empty()
        || world
            .resource::<WorkbenchRequests>()
            .next_sequence()
            .is_some()
        || bed_workbench::waiting_for_layout(world)
    {
        return Err("Wait for pending document and layout operations before quitting".into());
    }
    let documents = world.non_send::<DocumentSession>().document_ids();
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
                for document in dirty {
                    save_document(world, document)?;
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
    if !core_mode(world) {
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
    for intent in world.resource_mut::<HostEffects>().take() {
        let request = capture_host_request(world, intent.source, intent.action);
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
pub fn resolve_documents(world: &mut World) {
    if !core_mode(world) {
        return;
    }
    assert!(
        !bed_workbench::frontend_active(world),
        "document requests run after drawing"
    );
    admit(world);
    bed_workbench::poll_request_results(world);
    while !bed_workbench::waiting_for_layout(world) && !world.resource::<CoreExiting>().0 {
        let document_sequence = world
            .resource::<HostRequests>()
            .pending
            .front()
            .map(|entry| entry.request.sequence());
        let panel_sequence = world.resource::<WorkbenchRequests>().next_sequence();
        if panel_sequence.is_some()
            && (document_sequence.is_none() || panel_sequence < document_sequence)
        {
            let queued = world
                .resource_mut::<WorkbenchRequests>()
                .take_next()
                .unwrap();
            bed_workbench::resolve_request(world, queued);
            // The outbox is empty at command entry. Its newly produced effects
            // finish before the next independent command.
            for intent in world.resource_mut::<HostEffects>().take() {
                let request = capture_host_request(world, intent.source, intent.action);
                let source = request.source();
                if let Err(error) = apply_request(world, request) {
                    notice(world, source, error);
                }
            }
        } else if let Some(accepted) = world.resource_mut::<HostRequests>().pending.pop_front() {
            let source = accepted.request.source();
            let result = apply_request(world, accepted.request);
            if let Some(id) = accepted.id {
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
                &world.resource::<HostFrame>().default_viewers,
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
        .query::<(Entity, &PanelBinding, &bed_api::workbench::PanelActions)>()
        .iter(world)
        .filter(|(_, binding, _)| {
            binding
                .document
                .is_some_and(|document| documents.contains(&document))
        })
        .map(|(entity, _, handler)| (entity, handler.0))
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
            {
                return Err("Draft commit may only produce its own document edits".into());
            }
            let request = capture_host_request(world, Some(entity), intent.action);
            apply_request(world, request)?;
        }
    }
    Ok(())
}

pub fn save_document(world: &mut World, document: DocumentId) -> Result<(), String> {
    assert!(
        !bed_workbench::frontend_active(world),
        "documents save after drawing"
    );
    if let Some(result) = models::run_model_operation(world, document, ModelOperation::Save) {
        return result;
    }
    commit_document_drafts(world, &[document])?;
    world
        .non_send_mut::<DocumentSession>()
        .save(document)
        .map_err(|error| error.to_string())?;
    let snapshot = world
        .non_send::<DocumentSession>()
        .snapshot(document)
        .map_err(|error| error.to_string())?;
    if snapshot.dirty {
        return Err("Document has not completed saving".into());
    }
    Ok(())
}
pub fn save_document_as(
    world: &mut World,
    document: DocumentId,
    path: &Path,
) -> Result<(), String> {
    assert!(
        !bed_workbench::frontend_active(world),
        "documents save after drawing"
    );
    if let Some(entity) = models::model_entity(world, document) {
        let previous = world
            .get::<models::ModelDocument>(entity)
            .unwrap()
            .path
            .clone();
        world.get_mut::<models::ModelDocument>(entity).unwrap().path = Some(path.to_owned());
        if let Err(error) = save_document(world, document) {
            world.get_mut::<models::ModelDocument>(entity).unwrap().path = previous;
            return Err(error);
        }
    } else {
        commit_document_drafts(world, &[document])?;
        world
            .non_send_mut::<DocumentSession>()
            .save_as(document, path)
            .map_err(|error| error.to_string())?;
        if document_dirty(world, document)? {
            return Err("Document has not completed saving".into());
        }
    }
    Ok(())
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
    let frontends = world
        .query::<(Entity, Option<&bed_api::workbench::PanelOwner>)>()
        .iter(world)
        .filter(|(entity, owner)| *entity == panel || owner.is_some_and(|owner| owner.0 == panel))
        .map(|(entity, _)| entity)
        .collect::<Vec<_>>();
    let mut documents = frontends
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
                ClosePolicy::Save => save_document(world, document)?,
                ClosePolicy::Discard => {}
            }
        }
    }
    Ok(true)
}

fn retire_panel(world: &mut World, panel: Entity) {
    if !core_mode(world) {
        return;
    }
    let frontends = world
        .query::<(Entity, Option<&bed_api::workbench::PanelOwner>)>()
        .iter(world)
        .filter(|(entity, owner)| *entity == panel || owner.is_some_and(|owner| owner.0 == panel))
        .map(|(entity, _)| entity)
        .collect::<Vec<_>>();
    for entity in frontends {
        if let Some(handler) = world.get::<bed_api::workbench::PanelClose>(entity).copied() {
            if let Err(error) = world
                .run_system_with(handler.0, entity)
                .map_err(|error| error.to_string())
                .and_then(|result| result)
            {
                notice(world, Some(entity), error);
            }
        }
    }
}

fn apply_request(world: &mut World, request: HostRequest) -> Result<HostOutcome, String> {
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
    match request.action().clone() {
        HostAction::ApplyEdits {
            document,
            revision,
            edits,
        } => world
            .non_send_mut::<DocumentSession>()
            .apply_edits(document, revision, &edits)
            .map_err(|error| error.to_string())?,
        HostAction::ApplyEditsWithResult {
            token,
            document,
            revision,
            edits,
        } => {
            let result = world
                .non_send_mut::<DocumentSession>()
                .apply_edits(document, revision, &edits)
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
            let result = world
                .non_send_mut::<DocumentSession>()
                .apply_edits(document, revision, &edits)
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
            save_document(world, document)?;
            return Ok(HostOutcome::Saved {
                documents: vec![document],
                revisions: vec![document_revision(world, document)?],
            });
        }
        HostAction::SaveAsDialog { document } => {
            if let Some(path) = rfd::FileDialog::new().save_file() {
                save_document_as(world, document, &path)?;
                return Ok(HostOutcome::Saved {
                    documents: vec![document],
                    revisions: vec![document_revision(world, document)?],
                });
            }
        }
        HostAction::SaveDocumentsWithResult {
            recipient,
            token,
            documents,
        } => {
            let result = (|| {
                commit_document_drafts(world, &documents)?;
                let mut saved = Vec::new();
                for document in documents {
                    save_document(world, document)?;
                    let revision = document_revision(world, document)?;
                    saved.push(bed_api::documents::SavedDocument { document, revision });
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
        HostAction::OpenFileDialog { viewer } => {
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
                    apply_request(
                        world,
                        capture_host_request(world, intent.source, intent.action),
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
                        .resource::<HostFrame>()
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
