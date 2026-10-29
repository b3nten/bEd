//! Documents: one per source, owned by an entity, kept alive by `DocumentReference`s.
//!
//! A document loads from its source on a worker, then holds the content, the
//! undo history and what was last saved. Each document runs at most one source
//! task at a time, a read or a save, so saving and outside changes never race:
//!
//! - A save writes a snapshot. Edits made meanwhile stay unsaved.
//! - Reads happen on `refresh_document`, or periodically when the source can
//!   cheaply tell it changed. A read never starts during a save, and a read
//!   still running when a save starts is dropped.
//! - An outside change to a document without unsaved changes is applied as an
//!   undoable edit. With unsaved changes it becomes a conflict, which blocks
//!   saving until resolved.
//! - A failed save keeps everything and schedules a read, so a save refused
//!   because the source changed turns into a conflict.
//!
//! ```
//! use bevy::prelude::*;
//! use bedtime_workbench::*;
//!
//! let mut app = App::new();
//! app.add_plugins(WorkbenchCorePlugins);
//! let document = app.world_mut().workbench_mut().open_document(MemorySource::new("Notes", TextBuffer::from("hello")))?;
//! while app.world().workbench().inspect_document(document.id())?.status == DocumentStatus::Loading {
//!     app.update();
//! }
//! app.world_mut().workbench_mut().edit_document(document, 0, BufferEdit::new(5..5, " world"), EditOptions::new("Append"))?;
//! assert_eq!(app.world().workbench().read_document(document)?.to_string(), "hello world");
//! assert!(app.world().workbench().inspect_document(document.id())?.dirty);
//! app.world_mut().workbench_mut().undo_document(document.id())?;
//! assert!(!app.world().workbench().inspect_document(document.id())?.dirty);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
use crate::LayoutsWorldExt;
use crate::{ManagedWork, PanelInstance, Workbench};
use crate::{WorkbenchRef, WorkbenchWorldExt};
use bevy::{
    ecs::lifecycle::{Discard, Insert},
    prelude::*,
};
use std::{
    any::{TypeId, type_name},
    collections::HashSet,
    fmt,
    marker::PhantomData,
};

pub use bedtime_document::{
    BufferEdit, DocumentCommit, DocumentModel, DocumentSource, MemorySource, SourceError, SourceId,
    SourceLoad, SourceVersion, TextBuffer,
};

#[path = "documents/autosave.rs"]
mod autosave;
#[path = "documents/store.rs"]
mod store;
#[cfg(test)]
#[path = "documents/tests.rs"]
pub(crate) mod tests;

pub(crate) use autosave::autosave;
pub use autosave::{DocumentAutosave, DocumentAutosavePolicy};
use store::{Session, Store, Typed};

/// At most this many documents read their sources at once.
const MAX_READS: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DocumentId(pub Entity);

impl DocumentId {
    pub fn entity(self) -> Entity {
        self.0
    }
}

/// A typed document id. Copies don't keep the document alive.
pub struct DocumentHandle<M: DocumentModel> {
    id: DocumentId,
    marker: PhantomData<fn() -> M>,
}

impl<M: DocumentModel> DocumentHandle<M> {
    fn new(id: DocumentId) -> Self {
        Self {
            id,
            marker: PhantomData,
        }
    }
    pub fn id(self) -> DocumentId {
        self.id
    }
}

impl<M: DocumentModel> Copy for DocumentHandle<M> {}
impl<M: DocumentModel> Clone for DocumentHandle<M> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<M: DocumentModel> PartialEq for DocumentHandle<M> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}
impl<M: DocumentModel> Eq for DocumentHandle<M> {}
impl<M: DocumentModel> fmt::Debug for DocumentHandle<M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("DocumentHandle").field(&self.id).finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DocumentStatus {
    Loading,
    Ready,
    /// The first load failed. `refresh_document` tries again.
    Failed(String),
}

/// The document itself. Its contents and history are private; use the
/// functions in this module to read and change them.
#[derive(Component)]
pub struct Document {
    model_type: TypeId,
    model_name: &'static str,
    store: Box<dyn Store>,
}

impl Document {
    pub fn title(&self) -> String {
        self.store.title()
    }
    pub fn model_type_name(&self) -> &'static str {
        self.model_name
    }
}

/// A reference to a document. On a panel it is the document the panel shows; on
/// any other entity (a child of a panel showing several documents, or a plugin's
/// own holder) it simply keeps the document open. A document that has been
/// referenced is released once its last reference goes (see `closing`).
///
/// Insert it directly: `(MyViewer, DocumentReference::new(id), InArea(area))`.
/// Remove or replace it to stop showing the document. Commit or cancel any
/// `DocumentDraft` on the holder first.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
#[relationship(relationship_target = DocumentReferences)]
pub struct DocumentReference(Entity);

impl DocumentReference {
    pub fn new(document: DocumentId) -> Self {
        Self(document.0)
    }
    pub fn document(&self) -> DocumentId {
        DocumentId(self.0)
    }
}

/// The entities referencing a document, maintained by Bevy.
#[derive(Component, Debug, Default)]
#[relationship_target(relationship = DocumentReference)]
pub struct DocumentReferences(Vec<Entity>);

impl DocumentReferences {
    pub fn holders(&self) -> &[Entity] {
        &self.0
    }
}

/// A document that has been referenced at least once. Documents never
/// referenced are left to whoever opened them.
#[derive(Component)]
pub(crate) struct Referenced;

/// Marks a holder with uncommitted input (e.g. an IME composition). Saving,
/// undo, outside changes and closing wait until it's removed.
#[derive(Component, Default, Debug)]
pub struct DocumentDraft;

#[derive(Clone, Debug)]
pub struct DocumentInfo {
    pub id: DocumentId,
    pub title: String,
    pub model_type_name: &'static str,
    pub status: DocumentStatus,
    /// Increases with every change to the content.
    pub revision: u64,
    /// The content differs from what was last loaded or saved.
    pub dirty: bool,
    pub saving: bool,
    /// The source changed while there were unsaved changes. Saving is blocked
    /// until `resolve_document_conflict`.
    pub conflict: bool,
    /// The source no longer exists. Saving is refused; the content is kept.
    pub missing: bool,
    pub writable: bool,
    /// The last failed save or read, cleared by the next success.
    pub error: Option<String>,
    pub has_drafts: bool,
    pub undo_label: Option<String>,
    pub redo_label: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DocumentError {
    Unavailable(DocumentId),
    Loading,
    LoadFailed(String),
    SessionClosing,
    WrongModel {
        expected: &'static str,
        actual: &'static str,
    },
    StaleRevision {
        expected: u64,
        actual: u64,
    },
    InvalidChange(String),
    StaleUndoGroup,
    NotReferenced(Entity),
    NothingToUndo,
    NothingToRedo,
    ReadOnly,
    Missing,
    Saving,
    Conflict,
    NoConflict,
    Drafts,
    Dirty,
    Work(String),
    InvalidGraph(String),
}

impl fmt::Display for DocumentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(id) => write!(f, "Document {id:?} is unavailable"),
            Self::Loading => f.write_str("The document is still loading"),
            Self::LoadFailed(error) => write!(f, "The document couldn't be loaded: {error}"),
            Self::SessionClosing => f.write_str("The application is closing"),
            Self::WrongModel { expected, actual } => {
                write!(f, "Expected a {expected} document, found {actual}")
            }
            Self::StaleRevision { expected, actual } => {
                write!(
                    f,
                    "Expected revision {expected}, but the document is at {actual}"
                )
            }
            Self::InvalidChange(error) => write!(f, "Invalid change: {error}"),
            Self::StaleUndoGroup => f.write_str("The undo group has ended"),
            Self::NotReferenced(entity) => {
                write!(f, "{entity:?} doesn't reference this document")
            }
            Self::NothingToUndo => f.write_str("Nothing to undo"),
            Self::NothingToRedo => f.write_str("Nothing to redo"),
            Self::ReadOnly => f.write_str("The document is read-only"),
            Self::Missing => f.write_str("The document's source no longer exists"),
            Self::Saving => f.write_str("The document is being saved"),
            Self::Conflict => f.write_str("The document was changed elsewhere; resolve that first"),
            Self::NoConflict => f.write_str("The document has no conflict to resolve"),
            Self::Drafts => f.write_str("Finish typing before doing that"),
            Self::Dirty => f.write_str("The document has unsaved changes"),
            Self::Work(error) | Self::InvalidGraph(error) => f.write_str(error),
        }
    }
}

impl std::error::Error for DocumentError {}

/// Consecutive edits in one group undo together.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DocumentUndoGroup {
    document: DocumentId,
    viewer: Entity,
    serial: u64,
}

#[derive(Clone, Debug)]
pub struct EditOptions {
    pub label: String,
    pub group: Option<DocumentUndoGroup>,
    /// The holder making the edit, required with `group`.
    pub viewer: Option<Entity>,
}

impl EditOptions {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            group: None,
            viewer: None,
        }
    }
    pub fn grouped(label: impl Into<String>, group: DocumentUndoGroup) -> Self {
        Self {
            label: label.into(),
            group: Some(group),
            viewer: Some(group.viewer),
        }
    }
}

impl Default for EditOptions {
    fn default() -> Self {
        Self::new("Edit")
    }
}

#[derive(Message, Clone, Debug)]
pub struct DocumentSaveFinished {
    pub document: DocumentId,
    pub result: Result<(), String>,
}

/// The two sides of a conflict, plus what they started from.
#[derive(Clone, Debug)]
pub struct DocumentConflict<M: DocumentModel> {
    /// What was last loaded or saved, before either side changed it.
    pub base: Option<M>,
    pub local: M,
    pub incoming: M,
}

#[derive(Clone, Debug)]
pub enum DocumentConflictResolution<M: DocumentModel> {
    AcceptIncoming,
    KeepLocal,
    Merged(M),
}

impl WorkbenchRef {
    pub fn cast_document<M: DocumentModel>(
        &self,
        id: DocumentId,
    ) -> Result<DocumentHandle<M>, DocumentError> {
        let world = &self.world;
        let document = get_document(world, id)?;
        if document.model_type != TypeId::of::<M>() {
            return Err(DocumentError::WrongModel {
                expected: type_name::<M>(),
                actual: document.model_name,
            });
        }
        Ok(DocumentHandle::new(id))
    }

    pub fn read_document<M: DocumentModel>(
        &self,
        handle: DocumentHandle<M>,
    ) -> Result<&M, DocumentError> {
        let world = &self.world;
        Ok(session::<M>(world, handle.id)?.history.content())
    }

    pub fn document_ids(&self) -> Vec<DocumentId> {
        let world = &self.world;
        let Some(mut query) = world.try_query_filtered::<Entity, With<Document>>() else {
            return Vec::new();
        };
        let mut ids: Vec<_> = query.iter(world).map(DocumentId).collect();
        ids.sort();
        ids
    }

    pub fn inspect_document(&self, id: DocumentId) -> Result<DocumentInfo, DocumentError> {
        let world = &self.world;
        let document = get_document(world, id)?;
        let info = document.store.info();
        Ok(DocumentInfo {
            id,
            title: document.title(),
            model_type_name: document.model_name,
            status: info.status,
            revision: info.revision,
            dirty: info.dirty,
            saving: info.saving,
            conflict: info.conflict,
            missing: info.missing,
            writable: info.writable,
            error: info.error,
            has_drafts: world.workbench().document_has_drafts(id),
            undo_label: info.undo_label,
            redo_label: info.redo_label,
        })
    }

    /// The changes committed after `revision`. `None` means they're no longer
    /// retained, so derived state should be rebuilt from the current content.
    pub fn document_changes_since<M: DocumentModel>(
        &self,
        handle: DocumentHandle<M>,
        revision: u64,
    ) -> Result<Option<Vec<DocumentCommit<M::Change>>>, DocumentError> {
        let world = &self.world;
        let history = &session::<M>(world, handle.id)?.history;
        if revision > history.revision() {
            return Err(DocumentError::StaleRevision {
                expected: revision,
                actual: history.revision(),
            });
        }
        Ok(history.changes_since(revision))
    }

    pub fn document_conflict<M: DocumentModel>(
        &self,
        handle: DocumentHandle<M>,
    ) -> Result<Option<DocumentConflict<M>>, DocumentError> {
        let world = &self.world;
        Ok(session::<M>(world, handle.id)?.conflict())
    }

    pub fn document_has_drafts(&self, id: DocumentId) -> bool {
        let world = &self.world;
        let Some(mut query) = world.try_query_filtered::<&DocumentReference, With<DocumentDraft>>()
        else {
            return false;
        };
        query
            .iter(world)
            .any(|reference| reference.document() == id)
    }

    /// Whether a panel (or other holder) shows unsaved work: its document is
    /// unsaved, or it has uncommitted input.
    pub fn holder_unsaved(&self, holder: Entity) -> bool {
        let world = &self.world;
        world.get::<DocumentDraft>(holder).is_some()
            || world
                .get::<DocumentReference>(holder)
                .and_then(|reference| world.get::<Document>(reference.0))
                .is_some_and(|document| document.store.info().dirty)
    }
}

impl Workbench {
    /// Open the document for `source`, or return the one already open for it.
    /// Loading happens on a worker; `inspect_document` reports when it's ready.
    pub fn open_document<M: DocumentModel>(
        &mut self,
        source: impl DocumentSource<M>,
    ) -> Result<DocumentHandle<M>, DocumentError> {
        let world = &mut self.world;
        validate_session(world)?;
        let source_id = source.id();
        let existing = world
            .query::<(Entity, &Document)>()
            .iter(world)
            .find(|(_, document)| document.store.source_id() == source_id)
            .map(|(entity, _)| DocumentId(entity));
        if let Some(id) = existing {
            return world.workbench().cast_document(id);
        }
        let entity = world
            .spawn(Document {
                model_type: TypeId::of::<M>(),
                model_name: type_name::<M>(),
                store: Box::new(Typed::new(source)),
            })
            .id();
        Ok(DocumentHandle::new(DocumentId(entity)))
    }

    pub fn edit_document<M: DocumentModel>(
        &mut self,
        handle: DocumentHandle<M>,
        expected_revision: u64,
        change: M::Change,
        options: EditOptions,
    ) -> Result<u64, DocumentError> {
        let world = &mut self.world;
        world.workbench_mut().edit_document_transaction(
            handle,
            expected_revision,
            [change],
            options,
        )
    }

    /// Apply the changes as one undoable step, or none of them.
    pub fn edit_document_transaction<M: DocumentModel>(
        &mut self,
        handle: DocumentHandle<M>,
        expected_revision: u64,
        changes: impl IntoIterator<Item = M::Change>,
        options: EditOptions,
    ) -> Result<u64, DocumentError> {
        let world = &mut self.world;
        validate_session(world)?;
        if let Some(viewer) = options.viewer {
            validate_holder(world, viewer, handle.id)?;
        }
        session_mut::<M>(world, handle.id)?.edit(handle.id, expected_revision, changes, options)
    }

    pub fn begin_document_undo_group(
        &mut self,
        id: DocumentId,
        viewer: Entity,
    ) -> Result<DocumentUndoGroup, DocumentError> {
        let world = &mut self.world;
        validate_session(world)?;
        validate_holder(world, viewer, id)?;
        store_mut(world, id)?.begin_group(id, viewer)
    }

    pub fn end_document_undo_group(
        &mut self,
        id: DocumentId,
        group: DocumentUndoGroup,
    ) -> Result<(), DocumentError> {
        let world = &mut self.world;
        store_mut(world, id)?.end_group(group)
    }

    pub fn undo_document(&mut self, id: DocumentId) -> Result<u64, DocumentError> {
        let world = &mut self.world;
        validate_session(world)?;
        reject_drafts(world, id)?;
        store_mut(world, id)?.undo(false)
    }

    pub fn redo_document(&mut self, id: DocumentId) -> Result<u64, DocumentError> {
        let world = &mut self.world;
        validate_session(world)?;
        reject_drafts(world, id)?;
        store_mut(world, id)?.undo(true)
    }

    /// Start saving the current content. `DocumentSaveFinished` reports the outcome.
    pub fn save_document(&mut self, id: DocumentId) -> Result<(), DocumentError> {
        let world = &mut self.world;
        validate_session(world)?;
        reject_drafts(world, id)?;
        world.resource_scope(|world, mut work: Mut<ManagedWork>| {
            store_mut(world, id)?.start_save(&mut work, id.0)
        })
    }

    /// Read the source again soon, picking up outside changes. Retries a failed load.
    pub fn refresh_document(&mut self, id: DocumentId) -> Result<(), DocumentError> {
        let world = &mut self.world;
        store_mut(world, id)?.refresh();
        Ok(())
    }

    /// Settle a conflict. `expected_revision` guards against resolving against
    /// local content the user hasn't seen.
    pub fn resolve_document_conflict<M: DocumentModel>(
        &mut self,
        handle: DocumentHandle<M>,
        expected_revision: u64,
        resolution: DocumentConflictResolution<M>,
    ) -> Result<u64, DocumentError> {
        let world = &mut self.world;
        validate_session(world)?;
        session_mut::<M>(world, handle.id)?.resolve(expected_revision, resolution)
    }

    /// Close a document and the panels showing it. Refused with unsaved changes.
    pub fn close_document(&mut self, id: DocumentId) -> Result<(), DocumentError> {
        let world = &mut self.world;
        retire_document(world, id, false)
    }

    /// Close a document and the panels showing it, dropping unsaved changes.
    pub fn discard_document(&mut self, id: DocumentId) -> Result<(), DocumentError> {
        let world = &mut self.world;
        retire_document(world, id, true)
    }
}

fn retire_document(world: &mut World, id: DocumentId, discard: bool) -> Result<(), DocumentError> {
    validate_session(world)?;
    let info = world.workbench().inspect_document(id)?;
    if info.has_drafts {
        return Err(DocumentError::Drafts);
    }
    if info.saving {
        return Err(DocumentError::Saving);
    }
    if !discard && info.conflict {
        return Err(DocumentError::Conflict);
    }
    if !discard && info.dirty {
        return Err(DocumentError::Dirty);
    }
    if world.layouts().drawing() {
        return Err(DocumentError::InvalidGraph(
            "Documents can't be closed while the workbench is drawing".into(),
        ));
    }
    // Close the panels showing it; despawning the document drops other references.
    let panels: Vec<_> = world
        .get::<DocumentReferences>(id.0)
        .map_or(&[][..], |references| references.holders())
        .iter()
        .copied()
        .filter(|&holder| world.get::<PanelInstance>(holder).is_some())
        .collect();
    let layouts = world.layouts_mut();
    for panel in panels {
        layouts
            .close_panel(panel)
            .map_err(DocumentError::InvalidGraph)?;
    }
    world.despawn(id.0);
    Ok(())
}

/// Every draft has a reference, and every reference points at a document.
pub(crate) fn validate_documents(world: &mut World) -> Result<(), String> {
    for entity in world
        .query_filtered::<Entity, With<DocumentDraft>>()
        .iter(world)
    {
        if world.get::<DocumentReference>(entity).is_none() {
            return Err("DocumentDraft requires a DocumentReference".into());
        }
    }
    for reference in world.query::<&DocumentReference>().iter(world) {
        get_document(world, reference.document()).map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub(crate) fn install(app: &mut App) {
    app.add_message::<DocumentSaveFinished>()
        .add_observer(mark_referenced)
        .add_observer(release_reference)
        .add_systems(
            PreUpdate,
            poll_documents.before(crate::SessionSystems::Prepare),
        );
}

fn mark_referenced(
    insert: On<Insert<DocumentReference>>,
    references: Query<&DocumentReference>,
    mut commands: Commands,
) {
    if let Ok(reference) = references.get(insert.entity) {
        commands.entity(reference.0).try_insert(Referenced);
    }
}

/// A holder letting go ends its undo group.
fn release_reference(
    discard: On<Discard<DocumentReference>>,
    references: Query<&DocumentReference>,
    mut documents: Query<&mut Document>,
) {
    let holder = discard.entity;
    if let Ok(reference) = references.get(holder)
        && let Ok(mut document) = documents.get_mut(reference.0)
    {
        document.store.detach_viewer(holder);
    }
}

/// Accept finished reads and saves, and start reads that are due.
fn poll_documents(world: &mut World) {
    let drafted: HashSet<_> = world
        .query_filtered::<&DocumentReference, With<DocumentDraft>>()
        .iter(world)
        .map(DocumentReference::document)
        .collect();
    let mut finished = Vec::new();
    world.resource_scope(|world, mut work: Mut<ManagedWork>| {
        let mut documents = world.query::<(Entity, &mut Document)>();
        let mut reads = documents
            .iter(world)
            .filter(|(_, document)| document.store.reading())
            .count();
        let closing = work.is_closing();
        for (entity, mut document) in documents.iter_mut(world) {
            let id = DocumentId(entity);
            let may_read = !closing && reads < MAX_READS && !drafted.contains(&id);
            let was_reading = document.store.reading();
            if let Some(result) = document.store.poll(&mut work, entity, may_read) {
                finished.push(DocumentSaveFinished {
                    document: id,
                    result,
                });
            }
            if !was_reading && document.store.reading() {
                reads += 1;
            }
        }
    });
    world.write_message_batch(finished);
}

fn get_document(world: &World, id: DocumentId) -> Result<&Document, DocumentError> {
    world
        .get::<Document>(id.0)
        .ok_or(DocumentError::Unavailable(id))
}

fn store_mut(world: &mut World, id: DocumentId) -> Result<&mut dyn Store, DocumentError> {
    Ok(world
        .get_mut::<Document>(id.0)
        .ok_or(DocumentError::Unavailable(id))?
        .into_inner()
        .store
        .as_mut())
}

fn session<M: DocumentModel>(world: &World, id: DocumentId) -> Result<&Session<M>, DocumentError> {
    let document = get_document(world, id)?;
    document
        .store
        .as_any()
        .downcast_ref::<Typed<M>>()
        .ok_or(DocumentError::WrongModel {
            expected: type_name::<M>(),
            actual: document.model_name,
        })?
        .session()
}

fn session_mut<M: DocumentModel>(
    world: &mut World,
    id: DocumentId,
) -> Result<&mut Session<M>, DocumentError> {
    let document = world
        .get_mut::<Document>(id.0)
        .ok_or(DocumentError::Unavailable(id))?
        .into_inner();
    let actual = document.model_name;
    document
        .store
        .as_any_mut()
        .downcast_mut::<Typed<M>>()
        .ok_or(DocumentError::WrongModel {
            expected: type_name::<M>(),
            actual,
        })?
        .session_mut()
}

fn validate_session(world: &World) -> Result<(), DocumentError> {
    if world.resource::<ManagedWork>().is_closing() {
        return Err(DocumentError::SessionClosing);
    }
    Ok(())
}

fn validate_holder(world: &World, holder: Entity, id: DocumentId) -> Result<(), DocumentError> {
    match world.get::<DocumentReference>(holder) {
        Some(reference) if reference.document() == id => Ok(()),
        _ => Err(DocumentError::NotReferenced(holder)),
    }
}

fn reject_drafts(world: &World, id: DocumentId) -> Result<(), DocumentError> {
    if world.workbench().document_has_drafts(id) {
        Err(DocumentError::Drafts)
    } else {
        Ok(())
    }
}
