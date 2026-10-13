//! Typed document contents share lifecycle and history without becoming text buffers.
use bed_documents::{DocumentId, DocumentSession, SessionId};
use bevy::{ecs::system::SystemId, prelude::*};
use std::{any::TypeId, collections::HashMap, path::PathBuf};

/// Applying a change returns its inverse. The core applies it to a clone first,
/// so a rejected change cannot partially mutate the live document.
/// Clones must isolate editable state; applying or encoding a model must not
/// mutate external state.
pub trait DocumentModel: Clone + Send + Sync + 'static {
    type Change: Clone + Send + Sync + 'static;
    fn apply(&mut self, change: &Self::Change) -> Result<Self::Change, String>;
    fn encode(&self) -> Result<Vec<u8>, String>;
}

#[derive(Component)]
pub struct ModelContent<M: DocumentModel>(M);
impl<M: DocumentModel> ModelContent<M> {
    pub fn read(&self) -> &M {
        &self.0
    }
}

/// A panel can associate with a document without depending on a text editor.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct DocumentBinding(pub DocumentId);

#[derive(Component, Clone, Debug)]
pub struct ModelDocument {
    id: DocumentId,
    session_id: SessionId,
    revision: u64,
    state: u64,
    saved_state: u64,
    next_state: u64,
    pub label: String,
    pub path: Option<PathBuf>,
}
impl ModelDocument {
    pub fn id(&self) -> DocumentId {
        self.id
    }
    pub fn session_id(&self) -> SessionId {
        self.session_id
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn dirty(&self) -> bool {
        self.state != self.saved_state
    }
}

struct Transaction<C> {
    undo: C,
    redo: C,
    before: u64,
    after: u64,
    label: String,
}
#[derive(Component)]
struct History<M: DocumentModel> {
    undo: Vec<Transaction<M::Change>>,
    redo: Vec<Transaction<M::Change>>,
}

#[derive(Clone, Copy)]
pub(crate) enum ModelOperation {
    Undo,
    Redo,
}
pub(crate) struct ModelInvocation {
    pub entity: Entity,
    pub operation: ModelOperation,
}
type ModelSystem = SystemId<In<ModelInvocation>, Result<(), String>>;
fn ensure_model_session(world: &World, session: SessionId) -> Result<(), String> {
    let documents = world.non_send::<DocumentSession>();
    if !documents.is_running() {
        return Err("Document session is shut down".into());
    }
    if session != documents.session_id() {
        return Err("Document session was replaced".into());
    }
    Ok(())
}
type ModelEncoder = fn(&World, Entity) -> Result<Vec<u8>, String>;
#[derive(Resource, Default)]
pub(crate) struct ModelRegistry {
    documents: HashMap<DocumentId, (Entity, ModelSystem)>,
    handlers: HashMap<TypeId, ModelSystem>,
    encoders: HashMap<DocumentId, ModelEncoder>,
}

pub(crate) struct ModelSaveCapture {
    pub document: DocumentId,
    pub session: SessionId,
    pub revision: u64,
    state: u64,
    pub path: PathBuf,
    pub bytes: Vec<u8>,
}
fn encode_model<M: DocumentModel>(world: &World, entity: Entity) -> Result<Vec<u8>, String> {
    world
        .get::<ModelContent<M>>(entity)
        .ok_or("Document has a different model")?
        .read()
        .encode()
}
pub(crate) fn capture_model_save(
    world: &World,
    document: DocumentId,
    destination: Option<&std::path::Path>,
) -> Result<ModelSaveCapture, String> {
    let entity = model_entity(world, document).ok_or("Document is unavailable")?;
    let info = world
        .get::<ModelDocument>(entity)
        .ok_or("Document metadata is unavailable")?;
    ensure_model_session(world, info.session_id)?;
    let path = destination
        .map(std::path::Path::to_path_buf)
        .or_else(|| info.path.clone())
        .ok_or("Save As requires a document destination")?;
    let encoder = world
        .resource::<ModelRegistry>()
        .encoders
        .get(&document)
        .ok_or("Document encoder is unavailable")?;
    Ok(ModelSaveCapture {
        document,
        session: info.session_id,
        revision: info.revision,
        state: info.state,
        path,
        bytes: encoder(world, entity)?,
    })
}
pub(crate) fn complete_model_save(
    world: &mut World,
    saved: &ModelSaveCapture,
) -> Result<(), String> {
    ensure_model_session(world, saved.session)?;
    let entity = model_entity(world, saved.document).ok_or("Document is unavailable")?;
    let mut info = world
        .get_mut::<ModelDocument>(entity)
        .ok_or("Document metadata is unavailable")?;
    if info.session_id != saved.session {
        return Err("Document session was replaced".into());
    }
    info.saved_state = saved.state;
    info.path = Some(saved.path.clone());
    Ok(())
}

pub fn create_model_document<M: DocumentModel>(
    world: &mut World,
    model: M,
    label: impl Into<String>,
    path: Option<PathBuf>,
) -> DocumentId {
    assert!(
        world.non_send::<DocumentSession>().is_running(),
        "models belong to a running document session"
    );
    let id = DocumentId::next();
    let session = world.non_send::<DocumentSession>().session_id();
    let handler = match world
        .resource::<ModelRegistry>()
        .handlers
        .get(&TypeId::of::<M>())
        .copied()
    {
        Some(handler) => handler,
        None => {
            let handler = world.register_system(model_operation::<M>);
            world
                .resource_mut::<ModelRegistry>()
                .handlers
                .insert(TypeId::of::<M>(), handler);
            handler
        }
    };
    let entity = world
        .spawn((
            ModelContent(model),
            ModelDocument {
                id,
                session_id: session,
                revision: 0,
                state: 0,
                saved_state: 0,
                next_state: 1,
                label: label.into(),
                path,
            },
            History::<M> {
                undo: Vec::new(),
                redo: Vec::new(),
            },
        ))
        .id();
    world
        .resource_mut::<ModelRegistry>()
        .documents
        .insert(id, (entity, handler));
    world
        .resource_mut::<ModelRegistry>()
        .encoders
        .insert(id, encode_model::<M>);
    id
}

pub fn model_entity(world: &World, document: DocumentId) -> Option<Entity> {
    world
        .get_resource::<ModelRegistry>()?
        .documents
        .get(&document)
        .map(|entry| entry.0)
}
pub fn model<M: DocumentModel>(world: &World, document: DocumentId) -> Option<&M> {
    world
        .get::<ModelContent<M>>(model_entity(world, document)?)
        .map(ModelContent::read)
}
pub fn model_document(world: &World, document: DocumentId) -> Option<&ModelDocument> {
    world.get::<ModelDocument>(model_entity(world, document)?)
}

pub(crate) fn retire_model_document(world: &mut World, document: DocumentId) -> bool {
    let entry = world
        .resource_mut::<ModelRegistry>()
        .documents
        .remove(&document);
    world
        .resource_mut::<ModelRegistry>()
        .encoders
        .remove(&document);
    if let Some((entity, _)) = entry {
        world.despawn(entity);
        true
    } else {
        false
    }
}

/// Commit one semantic edit between frontend scopes. History and revisions
/// advance only after the model accepts the whole change.
pub fn edit_model<M: DocumentModel>(
    world: &mut World,
    document: DocumentId,
    expected_revision: u64,
    change: M::Change,
    label: impl Into<String>,
) -> Result<u64, String> {
    assert!(
        !bed_workbench::frontend_active(world),
        "document edits resolve after drawing"
    );
    let entity = model_entity(world, document).ok_or("Document is unavailable")?;
    let info = world
        .get::<ModelDocument>(entity)
        .ok_or("Document metadata is unavailable")?;
    ensure_model_session(world, info.session_id)?;
    if info.revision != expected_revision {
        return Err("Document changed; refresh before editing".into());
    }
    let mut draft = world
        .get::<ModelContent<M>>(entity)
        .ok_or("Document has a different model")?
        .0
        .clone();
    let inverse = draft.apply(&change)?;
    let before = info.state;
    let after = info.next_state;
    let next_state = after.checked_add(1).ok_or("Document state overflow")?;
    let revision = info
        .revision
        .checked_add(1)
        .ok_or("Document revision overflow")?;
    world.get_mut::<ModelContent<M>>(entity).unwrap().0 = draft;
    let mut history = world.get_mut::<History<M>>(entity).unwrap();
    history.redo.clear();
    history.undo.push(Transaction {
        undo: inverse,
        redo: change,
        before,
        after,
        label: label.into(),
    });
    let mut info = world.get_mut::<ModelDocument>(entity).unwrap();
    info.state = after;
    info.next_state = next_state;
    info.revision = revision;
    Ok(revision)
}

pub fn undo_label<M: DocumentModel>(world: &World, document: DocumentId) -> Option<&str> {
    world
        .get::<History<M>>(model_entity(world, document)?)
        .and_then(|history| history.undo.last())
        .map(|entry| entry.label.as_str())
}

/// An application with its own persistence can acknowledge an exact saved
/// revision. A stale acknowledgement never marks newer edits clean.
pub fn mark_model_saved(
    world: &mut World,
    document: DocumentId,
    expected_revision: u64,
) -> Result<(), String> {
    let entity = model_entity(world, document).ok_or("Document is unavailable")?;
    let session = world.non_send::<DocumentSession>().session_id();
    ensure_model_session(world, session)?;
    let mut info = world
        .get_mut::<ModelDocument>(entity)
        .ok_or("Document metadata is unavailable")?;
    if info.session_id != session || info.revision != expected_revision {
        return Err("Document changed while saving".into());
    }
    info.saved_state = info.state;
    Ok(())
}

pub(crate) fn run_model_operation(
    world: &mut World,
    document: DocumentId,
    operation: ModelOperation,
) -> Option<Result<(), String>> {
    let (entity, handler) = *world
        .get_resource::<ModelRegistry>()?
        .documents
        .get(&document)?;
    Some(
        world
            .run_system_with(handler, ModelInvocation { entity, operation })
            .map_err(|error| error.to_string())
            .and_then(|result| result),
    )
}

fn model_operation<M: DocumentModel>(
    In(input): In<ModelInvocation>,
    world: &mut World,
) -> Result<(), String> {
    let info = world
        .get::<ModelDocument>(input.entity)
        .ok_or("Document is unavailable")?
        .clone();
    ensure_model_session(world, info.session_id)?;
    let history = world.get::<History<M>>(input.entity).unwrap();
    let undo = matches!(input.operation, ModelOperation::Undo);
    let entry = if undo {
        history.undo.last()
    } else {
        history.redo.last()
    };
    let Some(entry) = entry else {
        return Ok(());
    };
    let mut draft = world
        .get::<ModelContent<M>>(input.entity)
        .unwrap()
        .0
        .clone();
    draft.apply(if undo { &entry.undo } else { &entry.redo })?;
    let state = if undo { entry.before } else { entry.after };
    let revision = info
        .revision
        .checked_add(1)
        .ok_or("Document revision overflow")?;
    world.get_mut::<ModelContent<M>>(input.entity).unwrap().0 = draft;
    let mut history = world.get_mut::<History<M>>(input.entity).unwrap();
    if undo {
        let entry = history.undo.pop().unwrap();
        history.redo.push(entry);
    } else {
        let entry = history.redo.pop().unwrap();
        history.undo.push(entry);
    }
    let mut info = world.get_mut::<ModelDocument>(input.entity).unwrap();
    info.state = state;
    info.revision = revision;
    Ok(())
}

pub fn undo_document(world: &mut World, document: DocumentId) -> Result<(), String> {
    assert!(
        !bed_workbench::frontend_active(world),
        "history resolves after drawing"
    );
    if let Some(result) = run_model_operation(world, document, ModelOperation::Undo) {
        return result;
    }
    crate::commit_document_drafts(world, &[document])?;
    world
        .non_send_mut::<DocumentSession>()
        .undo_document(document)
        .map_err(|error| error.to_string())
}
pub fn redo_document(world: &mut World, document: DocumentId) -> Result<(), String> {
    assert!(
        !bed_workbench::frontend_active(world),
        "history resolves after drawing"
    );
    if let Some(result) = run_model_operation(world, document, ModelOperation::Redo) {
        return result;
    }
    crate::commit_document_drafts(world, &[document])?;
    world
        .non_send_mut::<DocumentSession>()
        .redo_document(document)
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Clone)]
    struct Point(i32);
    impl DocumentModel for Point {
        type Change = i32;
        fn apply(&mut self, value: &i32) -> Result<i32, String> {
            let before = self.0;
            self.0 = *value;
            if *value < 0 {
                return Err("negative point".into());
            }
            Ok(before)
        }
        fn encode(&self) -> Result<Vec<u8>, String> {
            Ok(self.0.to_string().into_bytes())
        }
    }
    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(crate::DocumentHostPlugin);
        app
    }
    #[test]
    fn typed_history_rejects_partial_edits_and_preserves_saved_checkpoints() {
        let mut app = app();
        let world = app.world_mut();
        let document = create_model_document(world, Point(1), "Point", None);
        assert!(edit_model::<Point>(world, document, 0, -1, "Reject").is_err());
        assert_eq!(model::<Point>(world, document).unwrap().0, 1);
        assert_eq!(model_document(world, document).unwrap().revision(), 0);
        edit_model::<Point>(world, document, 0, 2, "Move").unwrap();
        mark_model_saved(world, document, 1).unwrap();
        edit_model::<Point>(world, document, 1, 3, "Move").unwrap();
        assert!(mark_model_saved(world, document, 1).is_err());
        undo_document(world, document).unwrap();
        assert!(!model_document(world, document).unwrap().dirty());
        assert_eq!(model_document(world, document).unwrap().revision(), 3);
        redo_document(world, document).unwrap();
        assert!(model_document(world, document).unwrap().dirty());
        assert_eq!(model::<Point>(world, document).unwrap().0, 3);
        assert!(edit_model::<Point>(world, document, 1, 4, "Stale").is_err());
    }
    #[test]
    fn a_replaced_document_session_rejects_model_work() {
        let mut app = app();
        let document = create_model_document(app.world_mut(), Point(1), "Point", None);
        app.world_mut().insert_non_send(DocumentSession::new());
        assert!(edit_model::<Point>(app.world_mut(), document, 0, 2, "Move").is_err());
        assert!(undo_document(app.world_mut(), document).is_err());
    }
}
