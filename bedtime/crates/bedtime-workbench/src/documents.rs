//! World-owned canonical documents, independent of their panel viewers.
//!
//! Providers supply canonical sources and own loading/persistence. Workbench
//! admits source I/O and accepts results against source generations. Only this
//! module mutates canonical content, history and acknowledged source checkpoints.
//! Text and custom models use the same session machinery. Source versions are
//! opaque provider tokens; sources enforce their actual overwrite expectations.
//!
//! ```
//! use bevy::prelude::*;
//! use bedtime_workbench::*;
//!
//! let mut app = App::new();
//! app.add_plugins(WorkbenchCorePlugin);
//! let document = open_document(
//!     app.world_mut(), MemorySource::new("Notes", String::from("hello")),
//! )?;
//! let panel = {
//!     let mut workbench = Workbench::new(app.world_mut());
//!     let layout = workbench.create_layout("Code")?;
//!     workbench.create_panel(layout, "Notes")?
//! };
//! while inspect_document(app.world(), document.id())?.load_state == DocumentLoadState::Loading { app.update(); std::thread::yield_now(); }
//! bind_document_view(app.world_mut(), panel, document)?;
//! edit_document(app.world_mut(), document, 0, TextChange {
//!     range: 5..5, replacement: " world".into(),
//! }, EditOptions::new("Append"))?;
//! assert_eq!(read_document(app.world(), document)?, "hello world");
//! assert!(inspect_document(app.world(), document.id())?.dirty);
//! undo_document(app.world_mut(), document.id())?;
//! assert!(!inspect_document(app.world(), document.id())?.dirty);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
use crate::{ManagedWork, PanelInstance, WorkId, Workbench};
use bevy::prelude::*;
use std::{
    any::{Any, TypeId, type_name},
    collections::HashMap,
    fmt,
    marker::PhantomData,
    sync::{Arc, mpsc::TryRecvError},
};

pub use bedtime_document::DocumentCommit;
use bedtime_document::History;
pub use bedtime_document::{
    BinaryDocument, DocumentModel, SourceVersion, TextChange, TextDocument,
};

/// Read transitions after `revision` without consuming another reader's updates.
/// `None` means the bounded journal no longer covers that revision. Refresh from
/// the current model in that case; never interpret a gap as no changes.
pub fn document_changes_since<M: DocumentModel>(
    world: &World,
    handle: DocumentHandle<M>,
    revision: u64,
) -> Result<Option<Vec<DocumentCommit<M::Change>>>, DocumentError> {
    let session = typed_session::<M>(world, handle.id)?;
    if revision > session.history.revision() {
        return Err(DocumentError::StaleRevision {
            expected: revision,
            actual: session.history.revision(),
        });
    }
    if revision == session.history.revision() {
        return Ok(Some(Vec::new()));
    }
    Ok(session.history.changes_since(revision))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DocumentId(pub Entity);
impl DocumentId {
    pub fn entity(self) -> Entity {
        self.0
    }
}

/// A checked typed reference, not an owner. Copies do not extend document life.
pub struct DocumentHandle<M: DocumentModel> {
    id: DocumentId,
    marker: PhantomData<fn() -> M>,
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

impl<M: DocumentModel> DocumentHandle<M> {
    pub fn id(self) -> DocumentId {
        self.id
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DocumentInitialState {
    /// The supplied model is the acknowledged source checkpoint, even if no
    /// source version or save binding is available.
    Loaded(Option<SourceVersion>),
    /// No contents have been acknowledged yet. An untouched new model is dirty.
    Unsaved,
}

#[derive(Clone, Debug)]
pub struct DocumentOptions {
    pub title: String,
    pub initial_state: DocumentInitialState,
    /// Maximum retained undo transactions. Zero disables history, not editing.
    pub history_limit: usize,
}

impl DocumentOptions {
    pub fn loaded(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            initial_state: DocumentInitialState::Loaded(None),
            history_limit: 100,
        }
    }
    pub fn unsaved(title: impl Into<String>) -> Self {
        Self {
            initial_state: DocumentInitialState::Unsaved,
            ..Self::loaded(title)
        }
    }
}

impl Default for DocumentOptions {
    fn default() -> Self {
        Self::unsaved("Untitled")
    }
}

/// Queryable metadata with privately owned, type-erased canonical session state.
#[derive(Component)]
pub struct Document {
    title: String,
    model_type: TypeId,
    model_name: &'static str,
    state: Box<dyn ErasedSession>,
    pub(crate) source: bedtime_document::SourceId,
    pub(crate) source_missing: bool,
    pub(crate) source_pending: bool,
    pub(crate) load_state: crate::DocumentLoadState,
}

impl Document {
    pub fn title(&self) -> &str {
        &self.title
    }
    pub fn model_type_name(&self) -> &'static str {
        self.model_name
    }
}

/// A non-owning reference: retiring a viewer never retires its document.
#[derive(Component, Clone, Copy, Debug)]
pub struct DocumentView {
    document: DocumentId,
    model_type: TypeId,
    model_name: &'static str,
}

impl DocumentView {
    pub fn document(&self) -> DocumentId {
        self.document
    }
}

/// Detach a viewer without retiring its document. Applications must resolve
/// viewer-local drafts first; local panel state is otherwise left untouched.
pub fn unbind_document_view(
    world: &mut World,
    panel: Entity,
) -> Result<Option<DocumentId>, DocumentError> {
    check_panel_document_draft(world, panel)?;
    let view = world.get::<DocumentView>(panel).copied();
    if let Some(view) = view {
        if let Some(mut document) = world.get_mut::<Document>(view.document.0) {
            document.state.detach_viewer(panel);
        }
        world.entity_mut(panel).remove::<DocumentView>();
        if let Some(mut metadata) = world.get_mut::<PanelInstance>(panel) {
            metadata.dirty = false;
        }
    }
    Ok(view.map(|view| view.document))
}

/// Application declaration that a bound panel has uncommitted input. Remove it
/// only after explicitly committing or abandoning that input. Save, undo, redo
/// and document/session retirement reject drafts rather than using old content.
#[derive(Component, Default, Debug)]
pub struct DocumentDraft;

#[derive(Clone, Debug)]
pub struct DocumentInfo {
    pub id: DocumentId,
    pub load_state: crate::DocumentLoadState,
    pub source_missing: bool,
    pub source_pending: bool,
    pub title: String,
    pub model_type_name: &'static str,
    pub revision: u64,
    /// Advances with accepted source knowledge, independently of content revision.
    pub source_generation: u64,
    /// Token of the acknowledged checkpoint. A pending conflict retains its own
    /// newer incoming token until explicit reconciliation.
    pub source_version: Option<SourceVersion>,
    pub dirty: bool,
    pub saving: bool,
    /// Includes a save CAS/source race requiring a fresh source observation,
    /// even when no incoming model is available to document_conflict yet.
    pub conflict: bool,
    pub requires_source_refresh: bool,
    pub has_save_binding: bool,
    pub has_drafts: bool,
    pub can_undo: bool,
    pub can_redo: bool,
    pub undo_label: Option<String>,
    pub redo_label: Option<String>,
    pub last_save_error: Option<DocumentSaveError>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DocumentError {
    Unavailable(DocumentId),
    Loading,
    LoadFailed(String),
    SessionUnavailable,
    SessionClosing,
    WrongModel {
        expected: &'static str,
        actual: &'static str,
    },
    InvalidPanel(Entity),
    StaleRevision {
        expected: u64,
        actual: u64,
    },
    StaleSourceGeneration {
        expected: u64,
        actual: u64,
    },
    InvalidChange(String),
    StaleUndoGroup,
    NothingToUndo,
    NothingToRedo,
    NoSaveBinding,
    Saving,
    DestinationBusy(String),
    Conflict,
    SourceRefreshRequired,
    NoConflict,
    Drafts,
    Dirty,
    Work(String),
    InvalidGraph(String),
}

impl fmt::Display for DocumentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Loading => f.write_str("Document is loading"),
            Self::LoadFailed(error) => write!(f, "Document load failed: {error}"),
            Self::Unavailable(id) => write!(f, "Document {id:?} is unavailable"),
            Self::SessionUnavailable => f.write_str("Document services are unavailable"),
            Self::SessionClosing => f.write_str("Session is closing"),
            Self::WrongModel { expected, actual } => {
                write!(f, "Expected document model {expected}, found {actual}")
            }
            Self::InvalidPanel(entity) => write!(f, "Panel {entity:?} is unavailable or unbound"),
            Self::StaleRevision { expected, actual } => write!(
                f,
                "Stale document revision {expected}; current revision is {actual}"
            ),
            Self::StaleSourceGeneration { expected, actual } => write!(
                f,
                "Stale source generation {expected}; current generation is {actual}"
            ),
            Self::InvalidChange(error) => write!(f, "Invalid document change: {error}"),
            Self::StaleUndoGroup => {
                f.write_str("Document undo group has ended or belongs to another viewer")
            }
            Self::NothingToUndo => f.write_str("Nothing to undo"),
            Self::NothingToRedo => f.write_str("Nothing to redo"),
            Self::NoSaveBinding => f.write_str("Document has no save binding"),
            Self::Saving => f.write_str("Document save is in progress"),
            Self::DestinationBusy(key) => write!(f, "Save destination {key:?} is busy"),
            Self::Conflict => f.write_str("Document has an unresolved source conflict"),
            Self::SourceRefreshRequired => {
                f.write_str("Refresh the document source before saving again")
            }
            Self::NoConflict => f.write_str("Document has no incoming conflict to resolve"),
            Self::Drafts => f.write_str("Document has unresolved viewer drafts"),
            Self::Dirty => f.write_str("Document has unsaved changes"),
            Self::Work(error) | Self::InvalidGraph(error) => f.write_str(error),
        }
    }
}

impl std::error::Error for DocumentError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DocumentUndoGroup {
    document: DocumentId,
    viewer: Entity,
    serial: u64,
}

#[derive(Clone, Debug)]
pub struct EditOptions {
    pub label: String,
    pub viewer: Option<Entity>,
    pub group: Option<DocumentUndoGroup>,
}

impl EditOptions {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            viewer: None,
            group: None,
        }
    }
    pub fn grouped(label: impl Into<String>, group: DocumentUndoGroup) -> Self {
        Self {
            label: label.into(),
            viewer: Some(group.viewer),
            group: Some(group),
        }
    }
}

impl Default for EditOptions {
    fn default() -> Self {
        Self::new("Edit")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DocumentSaveError {
    Failed(String),
    /// Backend compare-and-swap failed. Retrying is blocked until source refresh.
    Conflict(String),
    Panicked,
    Cancelled,
    /// The write may have succeeded, but newer source knowledge prevents its
    /// completion from acknowledging an obsolete snapshot. Refresh is required.
    SourceChanged,
}

impl fmt::Display for DocumentSaveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Failed(error) => f.write_str(error),
            Self::Conflict(error) => write!(f, "Source conflict: {error}"),
            Self::Panicked => f.write_str("Document save callback panicked"),
            Self::Cancelled => f.write_str("Document save was cancelled"),
            Self::SourceChanged => f.write_str("Source changed while saving; refresh is required"),
        }
    }
}

impl std::error::Error for DocumentSaveError {}

/// An owned immutable-in-time snapshot. The callback can manipulate its own copy
/// but cannot mutate the canonical session. Its expected token refers to the
/// acknowledged source checkpoint at save start, not the latest editor revision.
#[derive(Clone, Debug)]
pub struct DocumentSaveSnapshot<M: DocumentModel> {
    pub document: DocumentId,
    pub model: M,
    pub baseline: Option<M>,
    pub revision: u64,
    pub source_generation: u64,
    pub expected_source_version: Option<SourceVersion>,
}

type SaveResult = Result<Option<SourceVersion>, DocumentSaveError>;

type SaveCallback<M> = dyn Fn(DocumentSaveSnapshot<M>) -> SaveResult + Send + Sync;

pub(crate) struct SaveBinding<M: DocumentModel> {
    destination_key: Option<String>,
    callback: Arc<SaveCallback<M>>,
}

impl<M: DocumentModel> SaveBinding<M> {
    /// Equal destination keys serialize saves across documents in
    /// this World. Applications must use globally meaningful keys (e.g. canonical
    /// paths); absent keys only serialize saves within the individual document.
    pub fn new(
        destination_key: Option<String>,
        callback: impl Fn(DocumentSaveSnapshot<M>) -> SaveResult + Send + Sync + 'static,
    ) -> Self {
        Self {
            destination_key,
            callback: Arc::new(callback),
        }
    }
}

impl<M: DocumentModel> Clone for SaveBinding<M> {
    fn clone(&self) -> Self {
        Self {
            destination_key: self.destination_key.clone(),
            callback: self.callback.clone(),
        }
    }
}

#[derive(Message, Clone, Debug)]
pub struct DocumentSaveFinished {
    pub document: DocumentId,
    pub work: WorkId,
    pub captured_revision: u64,
    pub result: Result<(), DocumentSaveError>,
}

#[derive(Clone, Debug)]
pub struct DocumentConflict<M: DocumentModel> {
    pub base: Option<M>,
    /// Current local content, including edits accepted after conflict detection.
    pub local: M,
    pub incoming: M,
    pub incoming_version: Option<SourceVersion>,
    pub revision: u64,
    pub source_generation: u64,
}

#[derive(Clone, Debug)]
pub enum DocumentConflictResolution<M: DocumentModel> {
    AcceptIncoming,
    KeepLocal,
    Merged(M),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocumentExternalUpdate {
    Applied,
    Acknowledged,
    Duplicate,
    Conflict,
}

#[cfg(any(test, feature = "test-support"))]
pub(crate) fn create_memory_snapshot<M: DocumentModel>(
    world: &mut World,
    model: M,
    options: DocumentOptions,
) -> Result<DocumentHandle<M>, DocumentError> {
    validate_session(world)?;
    let memory = MemorySource::new(options.title.clone(), model.clone());
    let source_id = <MemorySource<M> as DocumentSource<M>>::id(&memory);
    let state = Session::new(model, &options);
    let entity = world
        .spawn((Document {
            title: options.title,
            model_type: TypeId::of::<M>(),
            model_name: type_name::<M>(),
            state: Box::new(state),
            source: source_id,
            source_missing: false,
            source_pending: false,
            load_state: crate::DocumentLoadState::Ready,
        },))
        .id();
    sources::attach_memory(world, entity, memory);
    Ok(DocumentHandle {
        id: DocumentId(entity),
        marker: PhantomData,
    })
}

pub fn cast_document<M: DocumentModel>(
    world: &World,
    id: DocumentId,
) -> Result<DocumentHandle<M>, DocumentError> {
    let document = get_document(world, id)?;
    if document.model_type != TypeId::of::<M>() {
        return Err(DocumentError::WrongModel {
            expected: type_name::<M>(),
            actual: document.model_name,
        });
    }
    Ok(DocumentHandle {
        id,
        marker: PhantomData,
    })
}

pub fn read_document<M: DocumentModel>(
    world: &World,
    handle: DocumentHandle<M>,
) -> Result<&M, DocumentError> {
    Ok(typed_session::<M>(world, handle.id)?.history.content())
}

/// A cheap immutable capture of acknowledged contents for source adapters.
pub fn document_checkpoint<M: DocumentModel>(
    world: &World,
    handle: DocumentHandle<M>,
) -> Result<Option<M>, DocumentError> {
    Ok(typed_session::<M>(world, handle.id)?.baseline.clone())
}

pub fn document_ids(world: &World) -> Vec<DocumentId> {
    let Some(mut query) = world.try_query::<(Entity, &Document)>() else {
        return Vec::new();
    };
    let mut ids: Vec<_> = query
        .iter(world)
        .map(|(entity, _)| DocumentId(entity))
        .collect();
    ids.sort();
    ids
}

pub fn inspect_document(world: &World, id: DocumentId) -> Result<DocumentInfo, DocumentError> {
    let document = get_document(world, id)?;
    let state = document.state.info();
    let has_drafts = document_has_drafts(world, id);
    Ok(DocumentInfo {
        id,
        load_state: document.load_state.clone(),
        source_missing: document.source_missing,
        source_pending: document.source_pending,
        title: document.title.clone(),
        model_type_name: document.model_name,
        revision: state.revision,
        source_generation: state.source_generation,
        source_version: state.source_version,
        dirty: state.dirty,
        saving: state.saving,
        conflict: state.conflict,
        requires_source_refresh: state.requires_source_refresh,
        has_save_binding: state.has_save_binding,
        has_drafts,
        can_undo: state.undo_label.is_some() && !has_drafts,
        can_redo: state.redo_label.is_some() && !has_drafts,
        undo_label: state.undo_label,
        redo_label: state.redo_label,
        last_save_error: state.last_save_error,
    })
}

pub fn edit_document<M: DocumentModel>(
    world: &mut World,
    handle: DocumentHandle<M>,
    expected_revision: u64,
    change: M::Change,
    options: EditOptions,
) -> Result<u64, DocumentError> {
    edit_document_transaction(world, handle, expected_revision, [change], options)
}

/// All changes use successive states of one isolated clone and commit as one
/// revision/undo transaction. No-op transactions do not advance the revision.
pub fn edit_document_transaction<M: DocumentModel>(
    world: &mut World,
    handle: DocumentHandle<M>,
    expected_revision: u64,
    changes: impl IntoIterator<Item = M::Change>,
    options: EditOptions,
) -> Result<u64, DocumentError> {
    validate_mutation(world, handle.id)?;
    if let Some(viewer) = options.viewer {
        validate_viewer(world, viewer, handle.id)?;
    }
    let mut document = world
        .get_mut::<Document>(handle.id.0)
        .ok_or(DocumentError::Unavailable(handle.id))?;
    let session = session_mut::<M>(&mut document)?;
    session.check_revision(expected_revision)?;
    session.check_group(handle.id, &options)?;
    if options.group.is_none() {
        session.group = None;
    }
    session
        .history
        .edit(changes, options.label, options.group)
        .map_err(DocumentError::InvalidChange)
}

pub fn begin_document_undo_group(
    world: &mut World,
    id: DocumentId,
    viewer: Entity,
) -> Result<DocumentUndoGroup, DocumentError> {
    validate_mutation(world, id)?;
    validate_viewer(world, viewer, id)?;
    Ok(world
        .get_mut::<Document>(id.0)
        .unwrap()
        .state
        .begin_group(id, viewer))
}

pub fn end_document_undo_group(
    world: &mut World,
    id: DocumentId,
    group: DocumentUndoGroup,
) -> Result<(), DocumentError> {
    validate_mutation(world, id)?;
    world
        .get_mut::<Document>(id.0)
        .unwrap()
        .state
        .end_group(group)
}

pub fn undo_document(world: &mut World, id: DocumentId) -> Result<u64, DocumentError> {
    validate_mutation(world, id)?;
    reject_drafts(world, id)?;
    world.get_mut::<Document>(id.0).unwrap().state.undo(false)
}

pub fn redo_document(world: &mut World, id: DocumentId) -> Result<u64, DocumentError> {
    validate_mutation(world, id)?;
    reject_drafts(world, id)?;
    world.get_mut::<Document>(id.0).unwrap().state.undo(true)
}

pub fn bind_document_view<M: DocumentModel>(
    world: &mut World,
    panel: Entity,
    handle: DocumentHandle<M>,
) -> Result<(), DocumentError> {
    validate_mutation(world, handle.id)?;
    let document = get_document(world, handle.id)?;
    if document.model_type != TypeId::of::<M>() {
        return Err(DocumentError::WrongModel {
            expected: type_name::<M>(),
            actual: document.model_name,
        });
    }
    if world.get::<PanelInstance>(panel).is_none() {
        return Err(DocumentError::InvalidPanel(panel));
    }
    if let Some(view) = world.get::<DocumentView>(panel) {
        if view.model_type != TypeId::of::<M>() {
            return Err(DocumentError::WrongModel {
                expected: view.model_name,
                actual: type_name::<M>(),
            });
        }
        if view.document == handle.id {
            return Ok(());
        }
    }
    check_panel_document_draft(world, panel)?;
    let dirty = document.state.info().dirty;
    world.entity_mut(panel).insert(DocumentView {
        document: handle.id,
        model_type: TypeId::of::<M>(),
        model_name: type_name::<M>(),
    });
    world.get_mut::<PanelInstance>(panel).unwrap().dirty = dirty;
    Ok(())
}

pub fn panel_has_draft(world: &World, panel: Entity) -> bool {
    world.get::<DocumentDraft>(panel).is_some()
}

pub fn check_panel_document_draft(world: &World, panel: Entity) -> Result<(), DocumentError> {
    if panel_has_draft(world, panel) {
        Err(DocumentError::Drafts)
    } else {
        Ok(())
    }
}

pub fn document_has_drafts(world: &World, id: DocumentId) -> bool {
    let Some(mut query) = world.try_query_filtered::<&DocumentView, With<DocumentDraft>>() else {
        return false;
    };
    query.iter(world).any(|view| view.document == id)
}

pub(crate) fn install_save_binding<M: DocumentModel>(
    world: &mut World,
    handle: DocumentHandle<M>,
    binding: SaveBinding<M>,
) -> Result<(), DocumentError> {
    validate_mutation(world, handle.id)?;
    let mut document = world.get_mut::<Document>(handle.id.0).unwrap();
    let session = session_mut::<M>(&mut document)?;
    if session.saving.is_some() {
        return Err(DocumentError::Saving);
    }
    session.binding_generation = next_counter(session.binding_generation);
    session.binding = Some(binding);
    session.group = None;
    Ok(())
}

pub(crate) fn remove_save_binding(world: &mut World, id: DocumentId) -> Result<(), DocumentError> {
    validate_mutation(world, id)?;
    world.get_mut::<Document>(id.0).unwrap().state.unbind_save()
}

#[derive(Resource, Default)]
struct SaveDestinations(HashMap<String, DocumentId>);

/// Start one asynchronous typed snapshot save. This untyped entry point is also
/// suitable for contextual commands.
pub fn save_document(world: &mut World, id: DocumentId) -> Result<WorkId, DocumentError> {
    start_document_save(world, id)
}

pub fn start_document_save(world: &mut World, id: DocumentId) -> Result<WorkId, DocumentError> {
    validate_mutation(world, id)?;
    reject_drafts(world, id)?;
    let document = get_document(world, id)?;

    let (destination, job) = document.state.prepare_save(id)?;
    world.init_resource::<SaveDestinations>();
    if let Some(key) = &destination {
        if world.resource::<SaveDestinations>().0.contains_key(key) {
            return Err(DocumentError::DestinationBusy(key.clone()));
        }
    }
    let task = world
        .get_resource_mut::<ManagedWork>()
        .ok_or(DocumentError::SessionUnavailable)?
        .spawn_task(Some(id.0), "Save document", move |_| job())
        .map_err(DocumentError::Work)?;
    let work = task.id;
    world
        .get_mut::<Document>(id.0)
        .unwrap()
        .state
        .save_started(task);
    if let Some(key) = destination {
        world.resource_mut::<SaveDestinations>().0.insert(key, id);
    }
    Ok(work)
}

/// Poll without blocking. Each completion is returned once and, when installed,
/// also emitted as a DocumentSaveFinished message. No task mutates the World.
pub fn poll_document_saves(world: &mut World) -> Vec<DocumentSaveFinished> {
    let ids: Vec<_> = world
        .query::<(Entity, &Document)>()
        .iter(world)
        .filter_map(|(entity, document)| document.state.info().saving.then_some(DocumentId(entity)))
        .collect();
    let mut finished = Vec::new();
    for id in ids {
        let completion = world.get_mut::<Document>(id.0).unwrap().state.poll_save(id);
        if let Some((destination, completion)) = completion {
            if let Some(key) = destination {
                if let Some(mut destinations) = world.get_resource_mut::<SaveDestinations>() {
                    if destinations.0.get(&key) == Some(&id) {
                        destinations.0.remove(&key);
                    }
                }
            }
            if let Some(mut messages) = world.get_resource_mut::<Messages<DocumentSaveFinished>>() {
                messages.write(completion.clone());
            }
            finished.push(completion);
        }
    }
    finished
}

pub fn offer_document_external_update<M: DocumentModel>(
    world: &mut World,
    handle: DocumentHandle<M>,
    expected_source_generation: u64,
    incoming: M,
    version: Option<SourceVersion>,
) -> Result<DocumentExternalUpdate, DocumentError> {
    // Already-admitted source effects must be reconciled during orderly drain.
    // Closing prevents new user work; it cannot discard a completed source write.
    get_document(world, handle.id)?;
    let mut document = world.get_mut::<Document>(handle.id.0).unwrap();
    let session = session_mut::<M>(&mut document)?;
    session.check_source_generation(expected_source_generation)?;
    let same_pending = session
        .conflict
        .as_ref()
        .is_some_and(|pending| pending.incoming == incoming && pending.version == version);
    let same_base = session.baseline.as_ref() == Some(&incoming);
    let duplicate = !session.refresh_required
        && ((same_pending && (*session.history.content() != incoming || session.saving.is_some()))
            || (session.conflict.is_none() && same_base && session.source_version == version));
    session.group = None;
    if duplicate {
        return Ok(DocumentExternalUpdate::Duplicate);
    }
    session.source_generation = next_counter(session.source_generation);
    session.refresh_required = false;
    session.last_save_error = None;
    // A watcher can observe our write before its receipt reaches the main world.
    // Do not let that echo replace content the user has since undone, or mark a
    // failed write saved prematurely. Its snapshot/token can be acknowledged by
    // the matching successful receipt; until then retain it without changing content.
    if session
        .saving
        .as_ref()
        .is_some_and(|active| active.snapshot == incoming)
    {
        session.conflict = Some(Incoming {
            base: session.baseline.clone(),
            incoming,
            version,
        });
        return Ok(DocumentExternalUpdate::Conflict);
    }
    if *session.history.content() == incoming {
        session.baseline = Some(incoming);
        session.source_version = version;
        session.conflict = None;
        return Ok(DocumentExternalUpdate::Acknowledged);
    }
    if same_base {
        session.source_version = version;
        session.conflict = None;
        return Ok(DocumentExternalUpdate::Duplicate);
    }
    if !session.dirty() && session.conflict.is_none() {
        session.replace_content(incoming.clone(), "External update");
        session.baseline = Some(incoming);
        session.source_version = version;
        session.conflict = None;
        return Ok(DocumentExternalUpdate::Applied);
    }
    session.conflict = Some(Incoming {
        base: session.baseline.clone(),
        incoming,
        version,
    });
    Ok(DocumentExternalUpdate::Conflict)
}

pub fn document_conflict<M: DocumentModel>(
    world: &World,
    handle: DocumentHandle<M>,
) -> Result<Option<DocumentConflict<M>>, DocumentError> {
    let session = typed_session::<M>(world, handle.id)?;
    Ok(session.conflict.as_ref().map(|conflict| DocumentConflict {
        base: conflict.base.clone(),
        local: session.history.content().clone(),
        incoming: conflict.incoming.clone(),
        incoming_version: conflict.version.clone(),
        revision: session.history.revision(),
        source_generation: session.source_generation,
    }))
}

pub fn resolve_document_conflict<M: DocumentModel>(
    world: &mut World,
    handle: DocumentHandle<M>,
    expected_revision: u64,
    expected_source_generation: u64,
    resolution: DocumentConflictResolution<M>,
) -> Result<u64, DocumentError> {
    validate_mutation(world, handle.id)?;
    let mut document = world.get_mut::<Document>(handle.id.0).unwrap();
    let session = session_mut::<M>(&mut document)?;
    session.check_revision(expected_revision)?;
    session.check_source_generation(expected_source_generation)?;
    // A receipt can establish that the retained incoming value predates a write
    // or failed CAS. Resolving that old comparison is not a fresh source read.
    if session.refresh_required {
        return Err(DocumentError::SourceRefreshRequired);
    }
    let incoming = session
        .conflict
        .as_ref()
        .ok_or(DocumentError::NoConflict)?
        .incoming
        .clone();
    let version = session.conflict.as_ref().unwrap().version.clone();
    match resolution {
        DocumentConflictResolution::AcceptIncoming => {
            session.replace_content(incoming.clone(), "Accept incoming")
        }
        DocumentConflictResolution::KeepLocal => {}
        DocumentConflictResolution::Merged(model) => {
            session.replace_content(model, "Resolve conflict")
        }
    }
    session.baseline = Some(incoming);
    session.source_version = version;
    session.source_generation = next_counter(session.source_generation);
    session.conflict = None;
    session.refresh_required = false;
    session.last_save_error = None;
    session.group = None;
    Ok(session.history.revision())
}

pub fn check_document_close(
    world: &World,
    id: DocumentId,
    discard: bool,
) -> Result<(), DocumentError> {
    let info = inspect_document(world, id)?;
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
    Ok(())
}

pub fn check_session_documents(world: &World) -> Result<(), DocumentError> {
    validate_session(world)?;
    for id in document_ids(world) {
        check_document_close(world, id, false)?;
    }
    Ok(())
}

pub fn close_document(world: &mut World, id: DocumentId) -> Result<(), DocumentError> {
    retire_document(world, id, false)
}

/// Explicitly abandon unsaved content and conflicts. In-flight writes and drafts
/// still require completion or an explicit application draft decision first.
pub fn discard_document(world: &mut World, id: DocumentId) -> Result<(), DocumentError> {
    retire_document(world, id, true)
}

fn retire_document(world: &mut World, id: DocumentId, discard: bool) -> Result<(), DocumentError> {
    validate_mutation(world, id)?;
    check_document_close(world, id, discard)?;
    #[cfg(feature = "ui")]
    if world
        .get_resource::<crate::WorkbenchUi>()
        .is_some_and(|ui| ui.drawing)
    {
        return Err(DocumentError::InvalidGraph(
            "Cannot retire documents during workbench drawing".into(),
        ));
    }
    crate::reconcile_workbench(world).map_err(DocumentError::InvalidGraph)?;
    check_document_close(world, id, discard)?;
    let panels: Vec<_> = world
        .query::<(Entity, &DocumentView)>()
        .iter(world)
        .filter_map(|(entity, view)| (view.document == id).then_some(entity))
        .collect();
    let mut workbench = Workbench::new(world);
    // Validate every viewer before retiring any of them.
    for &panel in &panels {
        workbench
            .panel_layout(panel)
            .map_err(DocumentError::InvalidGraph)?;
        check_panel_document_draft(workbench.world(), panel)?;
    }
    for panel in panels {
        workbench
            .close_panel(panel)
            .map_err(DocumentError::InvalidGraph)?;
    }
    world.despawn(id.0);
    Ok(())
}

/// Validate documents and their viewers at the workbench composition boundary.
pub fn reconcile_documents(world: &mut World) -> Result<(), String> {
    reconcile_document_views(world)
}

/// Validate document references and synchronize derived panel dirty flags. This
/// does not retire unviewed sessions. Parent composition calls it after membership
/// reconciliation and panel initialization. Validation precedes all flag writes.
pub fn reconcile_document_views(world: &mut World) -> Result<(), String> {
    let mut sources = HashMap::new();
    for (entity, document) in world.query::<(Entity, &Document)>().iter(world) {
        if sources.insert(document.source, entity).is_some() {
            return Err("Duplicate canonical source document".into());
        }
    }
    for entity in world
        .query_filtered::<Entity, With<DocumentDraft>>()
        .iter(world)
    {
        if world.get::<DocumentView>(entity).is_none() {
            return Err("DocumentDraft requires a bound DocumentView".into());
        }
    }
    let mut dirty = Vec::new();
    for (panel, view) in world.query::<(Entity, &DocumentView)>().iter(world) {
        if world.get::<PanelInstance>(panel).is_none() {
            return Err("DocumentView requires PanelInstance".into());
        }
        let document = get_document(world, view.document).map_err(|error| error.to_string())?;
        if document.model_type != view.model_type {
            return Err("DocumentView model does not match its document".into());
        }
        dirty.push((
            panel,
            document.state.info().dirty || panel_has_draft(world, panel),
        ));
    }
    for (panel, dirty) in dirty {
        if world.get::<PanelInstance>(panel).unwrap().dirty != dirty {
            world.get_mut::<PanelInstance>(panel).unwrap().dirty = dirty;
        }
    }
    Ok(())
}

pub(crate) fn install(app: &mut App) {
    app.init_resource::<SaveDestinations>()
        .add_message::<DocumentSaveFinished>()
        .add_systems(
            PreUpdate,
            (sources::poll_sources, poll_saves_system)
                .chain()
                .before(crate::SessionSystems::Prepare),
        );
}

fn poll_saves_system(world: &mut World) {
    poll_document_saves(world);
}

fn get_document(world: &World, id: DocumentId) -> Result<&Document, DocumentError> {
    world
        .get::<Document>(id.0)
        .ok_or(DocumentError::Unavailable(id))
}

fn typed_session<M: DocumentModel>(
    world: &World,
    id: DocumentId,
) -> Result<&Session<M>, DocumentError> {
    let document = get_document(world, id)?;
    match &document.load_state {
        crate::DocumentLoadState::Loading => return Err(DocumentError::Loading),
        crate::DocumentLoadState::Failed(e) => return Err(DocumentError::LoadFailed(e.clone())),
        _ => {}
    }
    document
        .state
        .as_any()
        .downcast_ref::<Session<M>>()
        .ok_or(DocumentError::WrongModel {
            expected: type_name::<M>(),
            actual: document.model_name,
        })
}

fn session_mut<M: DocumentModel>(
    document: &mut Document,
) -> Result<&mut Session<M>, DocumentError> {
    match &document.load_state {
        crate::DocumentLoadState::Loading => return Err(DocumentError::Loading),
        crate::DocumentLoadState::Failed(e) => return Err(DocumentError::LoadFailed(e.clone())),
        _ => {}
    }
    document
        .state
        .as_any_mut()
        .downcast_mut::<Session<M>>()
        .ok_or(DocumentError::WrongModel {
            expected: type_name::<M>(),
            actual: document.model_name,
        })
}

fn validate_session(world: &World) -> Result<(), DocumentError> {
    let work = world
        .get_resource::<ManagedWork>()
        .ok_or(DocumentError::SessionUnavailable)?;
    if work.is_closing() {
        return Err(DocumentError::SessionClosing);
    }
    Ok(())
}

fn validate_mutation(world: &World, id: DocumentId) -> Result<(), DocumentError> {
    get_document(world, id)?;
    validate_session(world)
}

fn validate_viewer(world: &World, viewer: Entity, id: DocumentId) -> Result<(), DocumentError> {
    let view = world
        .get::<DocumentView>(viewer)
        .ok_or(DocumentError::InvalidPanel(viewer))?;
    if view.document != id || world.get::<PanelInstance>(viewer).is_none() {
        return Err(DocumentError::InvalidPanel(viewer));
    }
    Ok(())
}

fn reject_drafts(world: &World, id: DocumentId) -> Result<(), DocumentError> {
    if document_has_drafts(world, id) {
        Err(DocumentError::Drafts)
    } else {
        Ok(())
    }
}

fn next_counter(value: u64) -> u64 {
    value.checked_add(1).expect("Document counter exhausted")
}

struct SessionInfo {
    revision: u64,
    source_generation: u64,
    source_version: Option<SourceVersion>,
    dirty: bool,
    saving: bool,
    conflict: bool,
    requires_source_refresh: bool,
    has_save_binding: bool,
    undo_label: Option<String>,
    redo_label: Option<String>,
    last_save_error: Option<DocumentSaveError>,
}

type SaveJob = Box<dyn FnOnce() -> SaveResult + Send>;

trait ErasedSession: Send + Sync + 'static {
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
    fn info(&self) -> SessionInfo;
    fn begin_group(&mut self, id: DocumentId, viewer: Entity) -> DocumentUndoGroup;
    fn end_group(&mut self, group: DocumentUndoGroup) -> Result<(), DocumentError>;
    fn detach_viewer(&mut self, viewer: Entity);
    fn undo(&mut self, redo: bool) -> Result<u64, DocumentError>;
    fn unbind_save(&mut self) -> Result<(), DocumentError>;
    fn prepare_save(&self, id: DocumentId) -> Result<(Option<String>, SaveJob), DocumentError>;
    fn save_started(&mut self, task: crate::ManagedTask<SaveResult>);
    fn poll_save(&mut self, id: DocumentId) -> Option<(Option<String>, DocumentSaveFinished)>;
}

struct Session<M: DocumentModel> {
    history: History<M, DocumentUndoGroup>,
    baseline: Option<M>,
    source_generation: u64,
    source_version: Option<SourceVersion>,
    next_group: u64,
    group: Option<DocumentUndoGroup>,
    binding: Option<SaveBinding<M>>,
    binding_generation: u64,
    saving: Option<ActiveSave<M>>,
    last_saved: Option<(WorkId, M)>,
    conflict: Option<Incoming<M>>,
    refresh_required: bool,
    last_save_error: Option<DocumentSaveError>,
}

struct Incoming<M> {
    base: Option<M>,
    incoming: M,
    version: Option<SourceVersion>,
}

struct ActiveSave<M: DocumentModel> {
    snapshot: M,
    revision: u64,
    source_generation: u64,
    binding_generation: u64,
    destination: Option<String>,
    task: crate::ManagedTask<SaveResult>,
}

impl<M: DocumentModel> Session<M> {
    fn new(content: M, options: &DocumentOptions) -> Self {
        let (baseline, source_version) = match &options.initial_state {
            DocumentInitialState::Loaded(version) => (Some(content.clone()), version.clone()),
            DocumentInitialState::Unsaved => (None, None),
        };
        Self {
            history: History::new(content, options.history_limit),
            baseline,
            source_generation: 0,
            source_version,
            next_group: 0,
            group: None,
            binding: None,
            binding_generation: 0,
            saving: None,
            last_saved: None,
            conflict: None,
            refresh_required: false,
            last_save_error: None,
        }
    }
    fn dirty(&self) -> bool {
        self.baseline.as_ref() != Some(self.history.content())
    }
    fn check_revision(&self, expected: u64) -> Result<(), DocumentError> {
        if expected != self.history.revision() {
            return Err(DocumentError::StaleRevision {
                expected,
                actual: self.history.revision(),
            });
        }
        Ok(())
    }
    fn check_source_generation(&self, expected: u64) -> Result<(), DocumentError> {
        if expected != self.source_generation {
            return Err(DocumentError::StaleSourceGeneration {
                expected,
                actual: self.source_generation,
            });
        }
        Ok(())
    }
    fn check_group(&self, id: DocumentId, options: &EditOptions) -> Result<(), DocumentError> {
        if let Some(group) = options.group {
            if self.group != Some(group)
                || group.document != id
                || options.viewer != Some(group.viewer)
            {
                return Err(DocumentError::StaleUndoGroup);
            }
        }
        Ok(())
    }
    fn replace_content(&mut self, model: M, label: &str) {
        self.group = None;
        self.history.replace(model, label);
    }
}

impl<M: DocumentModel> ErasedSession for Session<M> {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    fn info(&self) -> SessionInfo {
        SessionInfo {
            revision: self.history.revision(),
            source_generation: self.source_generation,
            source_version: self.source_version.clone(),
            dirty: self.dirty(),
            saving: self.saving.is_some(),
            conflict: self.conflict.is_some() || self.refresh_required,
            requires_source_refresh: self.refresh_required,
            has_save_binding: self.binding.is_some(),
            undo_label: self.history.undo_label(),
            redo_label: self.history.redo_label(),
            last_save_error: self.last_save_error.clone(),
        }
    }
    fn begin_group(&mut self, id: DocumentId, viewer: Entity) -> DocumentUndoGroup {
        self.next_group = next_counter(self.next_group);
        let group = DocumentUndoGroup {
            document: id,
            viewer,
            serial: self.next_group,
        };
        self.group = Some(group);
        group
    }
    fn end_group(&mut self, group: DocumentUndoGroup) -> Result<(), DocumentError> {
        if self.group != Some(group) {
            return Err(DocumentError::StaleUndoGroup);
        }
        self.group = None;
        Ok(())
    }
    fn detach_viewer(&mut self, viewer: Entity) {
        if self.group.is_some_and(|group| group.viewer == viewer) {
            self.group = None;
        }
    }
    fn undo(&mut self, redo: bool) -> Result<u64, DocumentError> {
        self.group = None;
        self.history
            .undo(redo)
            .map_err(DocumentError::InvalidChange)?
            .ok_or(if redo {
                DocumentError::NothingToRedo
            } else {
                DocumentError::NothingToUndo
            })
    }

    fn unbind_save(&mut self) -> Result<(), DocumentError> {
        if self.saving.is_some() {
            return Err(DocumentError::Saving);
        }
        self.binding_generation = next_counter(self.binding_generation);
        self.binding = None;
        self.group = None;
        Ok(())
    }
    fn prepare_save(&self, id: DocumentId) -> Result<(Option<String>, SaveJob), DocumentError> {
        if self.saving.is_some() {
            return Err(DocumentError::Saving);
        }
        if self.conflict.is_some() {
            return Err(DocumentError::Conflict);
        }
        if self.refresh_required {
            return Err(DocumentError::SourceRefreshRequired);
        }
        let binding = self.binding.as_ref().ok_or(DocumentError::NoSaveBinding)?;
        let callback = binding.callback.clone();
        let snapshot = DocumentSaveSnapshot {
            document: id,
            model: self.history.content().clone(),
            baseline: self.baseline.clone(),
            revision: self.history.revision(),
            source_generation: self.source_generation,
            expected_source_version: self.source_version.clone(),
        };
        Ok((
            binding.destination_key.clone(),
            Box::new(move || {
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| callback(snapshot)))
                    .unwrap_or(Err(DocumentSaveError::Panicked))
            }),
        ))
    }
    fn save_started(&mut self, task: crate::ManagedTask<SaveResult>) {
        self.group = None;
        self.last_save_error = None;
        self.saving = Some(ActiveSave {
            snapshot: self.history.content().clone(),
            revision: self.history.revision(),
            source_generation: self.source_generation,
            binding_generation: self.binding_generation,
            destination: self.binding.as_ref().unwrap().destination_key.clone(),
            task,
        });
    }
    fn poll_save(&mut self, id: DocumentId) -> Option<(Option<String>, DocumentSaveFinished)> {
        let active = self.saving.as_ref()?;
        let result = match active.task.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => Err(match active.task.status() {
                crate::WorkStatus::Cancelled | crate::WorkStatus::Cancelling => {
                    DocumentSaveError::Cancelled
                }
                crate::WorkStatus::Failed(error) => DocumentSaveError::Failed(error),
                _ => DocumentSaveError::Failed("Save worker did not return a result".into()),
            }),
        };
        let active = self.saving.take().unwrap();
        self.group = None;
        let result = match result {
            Ok(version) => {
                let observed_capture = self.conflict.as_ref().is_some_and(|incoming| {
                    incoming.incoming == active.snapshot && incoming.version == version
                }) || (self.conflict.is_none()
                    && self.baseline.as_ref() == Some(&active.snapshot)
                    && self.source_version == version);
                if active.binding_generation != self.binding_generation
                    || (active.source_generation != self.source_generation && !observed_capture)
                {
                    self.refresh_required = true;
                    // Invalidate source reads admitted before this uncertain
                    // receipt; only a read started afterward can resolve it.
                    self.source_generation = next_counter(self.source_generation);
                    Err(DocumentSaveError::SourceChanged)
                } else {
                    // Matching source observations are self-write echoes, not new
                    // edits. Only acknowledge the captured value; local undo/edits
                    // and their redo history remain exactly where the user left them.
                    self.last_saved = Some((active.task.id, active.snapshot.clone()));
                    self.baseline = Some(active.snapshot);
                    self.source_version = version;
                    self.source_generation = next_counter(self.source_generation);
                    self.conflict = None;
                    self.refresh_required = false;
                    Ok(())
                }
            }
            Err(error) => {
                if matches!(
                    error,
                    DocumentSaveError::Conflict(_)
                        | DocumentSaveError::Cancelled
                        | DocumentSaveError::Panicked
                        | DocumentSaveError::SourceChanged
                ) {
                    self.refresh_required = true;
                    self.source_generation = next_counter(self.source_generation);
                }
                Err(error)
            }
        };
        self.last_save_error = result.as_ref().err().cloned();
        Some((
            active.destination,
            DocumentSaveFinished {
                document: id,
                work: active.task.id,
                captured_revision: active.revision,
                result,
            },
        ))
    }
}

#[path = "documents/policy.rs"]
mod policy;
pub use policy::*;
#[path = "documents/bytes.rs"]
mod bytes;
pub use bytes::*;
#[path = "documents/sources.rs"]
mod sources;
pub use sources::*;
#[cfg(test)]
mod tests;

/// Contents actually acknowledged by this completion, independent of newer edits.
pub fn saved_document_capture<M: DocumentModel>(
    world: &World,
    completion: &DocumentSaveFinished,
) -> Option<M> {
    if completion.result.is_err() {
        return None;
    }
    let session = typed_session::<M>(world, completion.document).ok()?;
    session
        .last_saved
        .as_ref()
        .filter(|(work, _)| *work == completion.work)
        .map(|(_, model)| model.clone())
}

#[cfg(test)]
#[path = "documents/source_tests.rs"]
mod source_tests;
