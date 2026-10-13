//! Ordered context transitions retain policy in their requesting integration.
use crate::{
    DocumentSaveId, DocumentSaveTarget, begin_document_save, commit_document_drafts,
    document_dirty, document_revision, poll_document_save,
};
pub use bed_api::features::OperationBarrierToken;
use bed_documents::{ClosePolicy, DocumentId, DocumentSession, SessionId};
use bevy::prelude::*;
use std::collections::HashMap;

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
    prepared: Vec<(DocumentId, bed_api::documents::Revision)>,
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
        bed_api::features::HostAction::OperationBarrier { token },
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
            let targets = dirty
                .into_iter()
                .map(|document| DocumentSaveTarget {
                    document,
                    destination: None,
                })
                .collect::<Vec<_>>();
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
            Ok(_) => OperationBarrierStatus::Prepared,
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
            Option<&bed_api::documents::PanelBinding>,
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
    Ok(())
}
