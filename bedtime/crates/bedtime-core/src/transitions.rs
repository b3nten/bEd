//! Ordered context transitions retain policy in their requesting integration.
use crate::{
    DocumentSaveId, begin_document_save, commit_document_drafts, document_dirty, document_revision,
    poll_document_save,
};
pub use bedtime_api::features::OperationBarrierToken;
use bedtime_documents::{ClosePolicy, DocumentId, DocumentSession, SessionId};
use bevy::prelude::*;
use std::collections::HashMap;

/// Retaining a local directory keeps documents and processes; replacement owns
/// the complete save/decision/retirement transition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextChangeMode {
    RetainDocuments,
    /// Retain one non-document panel while replacing the surrounding workbench.
    RetainPanel(Entity),
    ReplaceDocuments,
    Reconnect,
}

pub struct ContextChange {
    pub context: bedtime_files::FileContext,
    pub services: bedtime_files::FileServices,
    pub options: bedtime_documents::SessionOptions,
    pub mode: ContextChangeMode,
}

#[derive(Clone, Debug)]
pub enum LifecycleReason {
    ContextChange {
        mode: ContextChangeMode,
        from: bedtime_files::FileContext,
        to: bedtime_files::FileContext,
    },
    Quit,
}

/// Optional persistence runs on the admitted operation before its live panels
/// and session are retired. A failed commit leaves the operation's data alive.
#[derive(Resource, Default)]
pub struct LifecyclePreCommit(
    pub Vec<bevy::ecs::system::SystemId<In<LifecycleReason>, Result<(), String>>>,
);

pub(crate) fn run_lifecycle_pre_commit(
    world: &mut World,
    reason: LifecycleReason,
) -> Result<(), String> {
    let systems = world
        .get_resource::<LifecyclePreCommit>()
        .map(|hooks| hooks.0.clone())
        .unwrap_or_default();
    for system in systems {
        world
            .run_system_with(system, reason.clone())
            .map_err(|error| error.to_string())??;
    }
    Ok(())
}

fn validate_retained_panel(world: &mut World, panel: Entity) -> Result<(), String> {
    if bedtime_api::workbench::panel_owner(world, panel) != Some(panel)
        || !bedtime_workbench::owns_panel(world, panel)
        || !bedtime_api::workbench::panel_active(world, panel)
    {
        return Err("Retained panel is unavailable".into());
    }
    if world
        .get::<bedtime_workbench::PanelRecord>(panel)
        .is_some_and(|record| record.unavailable.is_some())
    {
        return Err("Retained panel failed".into());
    }
    let edits = world
        .query::<(
            Entity,
            Option<&bedtime_api::documents::PanelBinding>,
            Option<&crate::DocumentBinding>,
        )>()
        .iter(world)
        .any(|(entity, text, model)| {
            bedtime_api::workbench::panel_owner(world, entity) == Some(panel)
                && (text.is_some_and(|binding| binding.document.is_some()) || model.is_some())
        });
    if edits {
        return Err("Retaining one panel requires a non-document panel".into());
    }
    Ok(())
}

struct PreparedContext {
    change: ContextChange,
    session: Option<DocumentSession>,
}

#[derive(Default)]
struct PreparedContexts(HashMap<OperationBarrierToken, PreparedContext>);

/// Continue a context operation at its existing barrier, never behind it.
pub fn prepare_context_change(
    world: &mut World,
    token: OperationBarrierToken,
    mut change: ContextChange,
) -> Result<OperationBarrierStatus, String> {
    if active_operation_barrier(world) != Some(token) {
        return Err("Context operation has not reached admission".into());
    }
    if change.context.provider != *change.services.provider_id() {
        return Err("Context and services use different providers".into());
    }
    if change.mode == ContextChangeMode::RetainDocuments
        && (world.non_send::<DocumentSession>().is_remote() || change.services.is_remote())
    {
        return Err("Retaining documents requires a local directory".into());
    }
    if let ContextChangeMode::RetainPanel(panel) = change.mode {
        validate_retained_panel(world, panel)?;
        if world.non_send::<DocumentSession>().is_remote() != change.services.is_remote() {
            return Err("Retaining a panel requires the existing document provider".into());
        }
        if change.services.is_remote() {
            let current = world.resource::<bedtime_files::FileContext>();
            if current.provider != change.context.provider
                || current.directory != change.context.directory
            {
                return Err(
                    "Retaining a remote panel requires the same provider and directory".into(),
                );
            }
        }
    }
    if change.mode == ContextChangeMode::Reconnect {
        let current = world.resource::<bedtime_files::FileContext>();
        if current.provider != change.context.provider
            || current.directory != change.context.directory
        {
            return Err("Reconnection must retain the same provider and directory".into());
        }
    }
    change.options.project_root = Some(change.context.directory.clone());
    let session = if change.mode == ContextChangeMode::ReplaceDocuments {
        Some(
            DocumentSession::with_file_services(
                change.options.clone(),
                &change.context,
                &change.services,
            )
            .map_err(|error| error.to_string())?,
        )
    } else {
        None
    };
    world.init_non_send::<PreparedContexts>();
    world
        .non_send_mut::<PreparedContexts>()
        .0
        .insert(token, PreparedContext { change, session });
    poll_context_change(world, token)
}

pub fn poll_context_change(
    world: &mut World,
    token: OperationBarrierToken,
) -> Result<OperationBarrierStatus, String> {
    let mode = world
        .non_send::<PreparedContexts>()
        .0
        .get(&token)
        .ok_or("Context operation is unavailable")?
        .change
        .mode;
    poll_barrier_admission(world);
    if barrier_status(world, token) == Some(OperationBarrierStatus::Waiting) {
        return Ok(OperationBarrierStatus::Waiting);
    }
    if matches!(
        mode,
        ContextChangeMode::ReplaceDocuments | ContextChangeMode::RetainPanel(_)
    ) {
        crate::documents::prepare_lifecycle_documents(world, token)
    } else {
        prepare_document_transition(world, token, ClosePolicy::Discard)
    }
}

pub fn commit_prepared_context_change(
    world: &mut World,
    token: OperationBarrierToken,
) -> Result<(), String> {
    validate_document_transition(world, token)?;
    let context = world
        .non_send::<PreparedContexts>()
        .0
        .get(&token)
        .ok_or("Context operation is unavailable")?;
    let retained = if let ContextChangeMode::RetainPanel(panel) = context.change.mode {
        Some(panel)
    } else {
        None
    };
    let reason = LifecycleReason::ContextChange {
        mode: context.change.mode,
        from: world.resource::<bedtime_files::FileContext>().clone(),
        to: context.change.context.clone(),
    };
    if let Some(panel) = retained {
        validate_retained_panel(world, panel)?;
    }
    run_lifecycle_pre_commit(world, reason)?;
    if let Some(panel) = retained {
        validate_retained_panel(world, panel)?;
    }
    validate_document_transition(world, token)?;
    let prepared = world
        .non_send_mut::<PreparedContexts>()
        .0
        .remove(&token)
        .unwrap();
    if let Some(session) = prepared.session {
        let panels = world
            .query_filtered::<Entity, With<bedtime_workbench::PanelPresentation>>()
            .iter(world)
            .filter(|panel| bedtime_workbench::owns_panel(world, *panel))
            .collect::<Vec<_>>();
        for panel in panels {
            bedtime_workbench::commit_close(world, panel);
        }
        commit_context_change(world, token, session)?;
        *world.resource_mut::<bedtime_workbench::WorkbenchLayout>() = Default::default();
        world.remove_resource::<crate::DocumentFrameSelection>();
        *world.resource_mut::<crate::DocumentNavigation>() = Default::default();
        bedtime_workbench::advance_layout_epoch(world);
    } else {
        // Validate and configure the retained provider before retiring peers.
        // A vanished destination or unavailable transport keeps all views live.
        world
            .non_send_mut::<DocumentSession>()
            .configure_file_services(
                prepared.change.options,
                &prepared.change.context,
                &prepared.change.services,
            )
            .map_err(|error| error.to_string())?;
        if let ContextChangeMode::RetainPanel(panel) = prepared.change.mode {
            let documents = transition_documents(world);
            let peers = crate::panel_entities(world)
                .into_iter()
                .filter(|peer| *peer != panel)
                .collect::<Vec<_>>();
            crate::retire_panels(world, &peers)?;
            for document in documents {
                crate::retire_document(world, document)?;
            }
            let id = world
                .get::<bedtime_workbench::PanelPresentation>(panel)
                .expect("validated retained panel")
                .id;
            let mut layout = bedtime_workbench::WorkbenchLayout::default();
            layout.tiling.assign(id, 1);
            layout.focused = Some(panel);
            layout.focused_area = Some(1);
            *world.resource_mut::<bedtime_workbench::WorkbenchLayout>() = layout;
            world
                .get_mut::<bedtime_workbench::PanelPresentation>(panel)
                .unwrap()
                .dock_next = true;
            world.remove_resource::<crate::DocumentFrameSelection>();
            *world.resource_mut::<crate::DocumentNavigation>() = Default::default();
            bedtime_workbench::advance_layout_epoch(world);
        }
    }
    world
        .resource_mut::<bedtime_files::FileContext>()
        .replace(prepared.change.context);
    world.insert_resource(prepared.change.services);
    crate::publish_documents(world);
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OperationBarrierStatus {
    Waiting,
    Ready,
    Pending,
    NeedsDecision(Vec<DocumentId>),
    Prepared,
    Failed(String),
    Complete,
}
struct Barrier {
    session: SessionId,
    status: OperationBarrierStatus,
    prepared: Vec<(DocumentId, bedtime_api::documents::Revision)>,
    save: Option<DocumentSaveId>,
}
#[derive(Resource, Default)]
pub(crate) struct OperationBarriers {
    next: u64,
    active: Option<OperationBarrierToken>,
    operations: HashMap<OperationBarrierToken, Barrier>,
}

pub fn create_operation_barrier(world: &mut World) -> OperationBarrierToken {
    let session = world.non_send::<DocumentSession>().session_id();
    let mut barriers = world.resource_mut::<OperationBarriers>();
    barriers.next = barriers.next.checked_add(1).expect("operation id overflow");
    let token = OperationBarrierToken(barriers.next);
    barriers.operations.insert(
        token,
        Barrier {
            session,
            status: OperationBarrierStatus::Waiting,
            prepared: Vec::new(),
            save: None,
        },
    );
    token
}
pub fn capture_operation_barrier(
    world: &mut World,
    source: Option<Entity>,
) -> OperationBarrierToken {
    let token = create_operation_barrier(world);
    crate::submit_host_request(
        world,
        source,
        bedtime_api::features::HostAction::OperationBarrier { token },
    );
    token
}
pub fn barrier_status(
    world: &World,
    token: OperationBarrierToken,
) -> Option<OperationBarrierStatus> {
    world
        .resource::<OperationBarriers>()
        .operations
        .get(&token)
        .map(|operation| operation.status.clone())
}

pub(crate) fn reject_operation_barrier(
    world: &mut World,
    token: OperationBarrierToken,
    error: String,
) {
    let mut barriers = world.resource_mut::<OperationBarriers>();
    if let Some(operation) = barriers.operations.get_mut(&token) {
        operation.status = OperationBarrierStatus::Failed(error);
    }
    if barriers.active == Some(token) {
        barriers.active = None;
    }
}
fn earlier_writes_pending(world: &World) -> bool {
    if crate::saves::saves_pending(world) {
        return true;
    }
    #[cfg(feature = "language-services")]
    if crate::format_saves::formatting_pending(world) {
        return true;
    }
    let session = world.non_send::<DocumentSession>();
    session
        .document_ids()
        .into_iter()
        .any(|document| session.save_pending(document))
}
/// Admission holds subsequent intentions while earlier writes finish. A save
/// result need not have been consumed before its confirmed write can settle.
pub(crate) fn poll_barrier_admission(world: &mut World) {
    let Some(token) = active_operation_barrier(world) else {
        return;
    };
    if barrier_status(world, token) != Some(OperationBarrierStatus::Waiting)
        || earlier_writes_pending(world)
    {
        return;
    }
    let session = world.non_send::<DocumentSession>().session_id();
    if world.resource::<OperationBarriers>().operations[&token].session != session {
        finish_operation_barrier(world, token, Err("Document session was replaced".into()))
            .expect("active admission");
    } else {
        world
            .resource_mut::<OperationBarriers>()
            .operations
            .get_mut(&token)
            .unwrap()
            .status = OperationBarrierStatus::Ready;
    }
}

pub(crate) fn activate_operation_barrier(
    world: &mut World,
    token: OperationBarrierToken,
) -> Result<(), String> {
    let session = world.non_send::<DocumentSession>().session_id();
    let pending = earlier_writes_pending(world);
    let mut barriers = world.resource_mut::<OperationBarriers>();
    if barriers.active.is_some() {
        return Err("Another transition is active".into());
    }
    let operation = barriers
        .operations
        .get_mut(&token)
        .ok_or("Unknown transition")?;
    if operation.session != session {
        return Err("Document session was replaced".into());
    }
    if operation.status != OperationBarrierStatus::Waiting {
        return Err("Transition was already admitted".into());
    }
    if !pending {
        operation.status = OperationBarrierStatus::Ready;
    }
    barriers.active = Some(token);
    Ok(())
}
pub fn operation_barrier_active(world: &World) -> bool {
    world.resource::<OperationBarriers>().active.is_some()
}
pub fn active_operation_barrier(world: &World) -> Option<OperationBarrierToken> {
    world.resource::<OperationBarriers>().active
}
pub(crate) fn transition_documents(world: &mut World) -> Vec<DocumentId> {
    let mut documents = world.non_send::<DocumentSession>().document_ids();
    let session = world.non_send::<DocumentSession>().session_id();
    documents.extend(
        world
            .query::<&crate::models::ModelDocument>()
            .iter(world)
            .filter(|model| model.session_id() == session)
            .map(crate::models::ModelDocument::id),
    );
    documents.sort();
    documents.dedup();
    documents
}

/// Run preflight as a continuation of the held operation. It is never submitted
/// behind the barrier that it must complete.
pub fn prepare_document_transition(
    world: &mut World,
    token: OperationBarrierToken,
    policy: ClosePolicy,
) -> Result<OperationBarrierStatus, String> {
    if world.resource::<OperationBarriers>().active != Some(token) {
        return Err("Transition has not reached admission".into());
    }
    poll_barrier_admission(world);
    if barrier_status(world, token) == Some(OperationBarrierStatus::Waiting) {
        return Ok(OperationBarrierStatus::Waiting);
    }
    let documents = transition_documents(world);
    commit_document_drafts(world, &documents)?;
    let dirty = documents
        .iter()
        .copied()
        .filter(|document| document_dirty(world, *document).unwrap_or(true))
        .collect::<Vec<_>>();
    let prepared = documents
        .iter()
        .map(|&document| document_revision(world, document).map(|revision| (document, revision)))
        .collect::<Result<Vec<_>, _>>()?;
    let (status, save) = match policy {
        ClosePolicy::Cancel if !dirty.is_empty() => {
            (OperationBarrierStatus::NeedsDecision(dirty), None)
        }
        ClosePolicy::Save if !dirty.is_empty() => {
            let targets = crate::documents::lifecycle_save_targets(world, &dirty)?;
            (
                OperationBarrierStatus::Pending,
                Some(begin_document_save(world, &targets)?),
            )
        }
        _ => (OperationBarrierStatus::Prepared, None),
    };
    let mut barriers = world.resource_mut::<OperationBarriers>();
    let operation = barriers
        .operations
        .get_mut(&token)
        .expect("active transition");
    operation.prepared = prepared;
    operation.save = save;
    operation.status = status.clone();
    Ok(status)
}
pub fn poll_document_transition(
    world: &mut World,
    token: OperationBarrierToken,
) -> Result<OperationBarrierStatus, String> {
    let save = world
        .resource::<OperationBarriers>()
        .operations
        .get(&token)
        .ok_or("Unknown transition")?
        .save;
    if let Some(save) = save
        && let Some(result) = poll_document_save(world, save)
    {
        let status = match result {
            Ok(saved) => {
                if saved.iter().any(|saved| {
                    document_revision(world, saved.document).ok() != Some(saved.revision)
                }) {
                    OperationBarrierStatus::Failed(
                        "Document changed while saving; prepare the transition again".into(),
                    )
                } else {
                    let current = transition_documents(world)
                        .into_iter()
                        .map(|document| {
                            document_revision(world, document).map(|revision| (document, revision))
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    world
                        .resource_mut::<OperationBarriers>()
                        .operations
                        .get_mut(&token)
                        .unwrap()
                        .prepared = current;
                    OperationBarrierStatus::Prepared
                }
            }
            Err(error) => OperationBarrierStatus::Failed(error),
        };
        let mut barriers = world.resource_mut::<OperationBarriers>();
        let operation = barriers.operations.get_mut(&token).unwrap();
        operation.save = None;
        operation.status = status;
    }
    barrier_status(world, token).ok_or_else(|| "Unknown transition".into())
}
pub fn commit_context_change(
    world: &mut World,
    token: OperationBarrierToken,
    session: DocumentSession,
) -> Result<(), String> {
    if poll_document_transition(world, token)? != OperationBarrierStatus::Prepared {
        return Err("Transition preflight has not completed".into());
    }
    let documents = transition_documents(world);
    commit_document_drafts(world, &documents)?;
    let prepared = world
        .resource::<OperationBarriers>()
        .operations
        .get(&token)
        .unwrap()
        .prepared
        .clone();
    let current = documents
        .into_iter()
        .map(|document| document_revision(world, document).map(|revision| (document, revision)))
        .collect::<Result<Vec<_>, _>>()?;
    if current.iter().any(|entry| !prepared.contains(entry)) {
        return Err("Documents changed after transition preflight; prepare again".into());
    }
    let bound = world
        .query::<(
            Option<&bedtime_api::documents::PanelBinding>,
            Option<&crate::DocumentBinding>,
        )>()
        .iter(world)
        .any(|(text, model)| {
            text.and_then(|binding| binding.document)
                .or(model.map(|binding| binding.0))
                .is_some_and(|document| prepared.iter().any(|(id, _)| *id == document))
        });
    if bound {
        return Err("Retire document presentations before replacing their session".into());
    }
    let old_session = world.non_send::<DocumentSession>().session_id();
    let operation = world
        .resource::<OperationBarriers>()
        .operations
        .get(&token)
        .unwrap();
    if old_session != operation.session {
        return Err("Document session was replaced".into());
    }
    let models = world
        .query::<&crate::ModelDocument>()
        .iter(world)
        .filter(|model| model.session_id() == old_session)
        .map(crate::ModelDocument::id)
        .collect::<Vec<_>>();
    for document in models {
        crate::models::retire_model_document(world, document);
    }
    world
        .non_send_mut::<DocumentSession>()
        .shutdown(ClosePolicy::Discard)
        .map_err(|error| error.to_string())?;
    world.insert_non_send(session);
    Ok(())
}
/// Validate immediately before retiring the prepared presentations/documents.
pub fn validate_document_transition(
    world: &mut World,
    token: OperationBarrierToken,
) -> Result<(), String> {
    if poll_document_transition(world, token)? != OperationBarrierStatus::Prepared {
        return Err("Transition preflight has not completed".into());
    }
    let documents = transition_documents(world);
    commit_document_drafts(world, &documents)?;
    let current = documents
        .into_iter()
        .map(|document| document_revision(world, document).map(|revision| (document, revision)))
        .collect::<Result<Vec<_>, _>>()?;
    let barriers = world.resource::<OperationBarriers>();
    let operation = barriers
        .operations
        .get(&token)
        .ok_or("Unknown transition")?;
    if world.non_send::<DocumentSession>().session_id() != operation.session
        || current != operation.prepared
    {
        return Err("Documents changed after transition preflight; prepare again".into());
    }
    Ok(())
}
pub fn finish_operation_barrier(
    world: &mut World,
    token: OperationBarrierToken,
    result: Result<(), String>,
) -> Result<(), String> {
    let mut barriers = world.resource_mut::<OperationBarriers>();
    if barriers.active != Some(token) {
        return Err("Transition is not active".into());
    }
    let operation = barriers
        .operations
        .get_mut(&token)
        .ok_or("Unknown transition")?;
    if result.is_ok()
        && !matches!(
            operation.status,
            OperationBarrierStatus::Ready | OperationBarrierStatus::Prepared
        )
    {
        return Err("Transition has not completed admission and preflight".into());
    }
    operation.status = match result {
        Ok(()) => OperationBarrierStatus::Complete,
        Err(error) => OperationBarrierStatus::Failed(error),
    };
    barriers.active = None;
    if let Some(mut contexts) = world.get_non_send_mut::<PreparedContexts>() {
        contexts.0.remove(&token);
    }
    Ok(())
}
