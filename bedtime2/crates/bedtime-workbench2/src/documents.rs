//! Workspace-owned document sessions, independent of their panel viewers.
//!
//! The application creates/loads models and supplies source observations and save
//! I/O. Only this module mutates canonical content, history and acknowledged source
//! checkpoints. Text and other models use exactly the same session machinery.
//! A source version is an opaque application token, not a filesystem timestamp.
//! Save callbacks must enforce their backend's compare-and-swap contract if lost
//! updates must be prevented: an absent token (or an unconditional writer) cannot
//! provide overwrite protection. Observations must be supplied in source order;
//! generation checks reject reads started against older document source knowledge.
//!
//! ```
//! use bevy::prelude::*;
//! use bedtime_workbench2::*;
//!
//! let mut app = App::new();
//! app.add_plugins(WorkbenchCorePlugin);
//! let workspace = Workspaces::new(app.world_mut()).open("Project");
//! let document = open_document(
//!     app.world_mut(), workspace.entity(), String::from("hello"),
//!     DocumentOptions::loaded("Notes"),
//! )?;
//! let panel = {
//!     let mut workbench = Workbench::with_workspace(app.world_mut(), workspace)?;
//!     let layout = workbench.active_layout();
//!     workbench.create_panel(layout, "Notes")?
//! };
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
use crate::{InWorkspace, PanelInstance, WorkId, Workbench, Workspace, WorkspaceId, WorkspaceWork};
use bevy::prelude::*;
use std::{
    any::{Any, TypeId, type_name},
    collections::{HashMap, VecDeque},
    fmt,
    marker::PhantomData,
    ops::Range,
    sync::{Arc, mpsc::TryRecvError},
};

/// A model's change must return an inverse that restores the previous model.
/// Changes are evaluated on an isolated clone; returning an error never changes
/// the canonical model, revision or history. Implementations must not perform I/O.
pub trait DocumentModel: Clone + PartialEq + Send + Sync + 'static {
    type Change: Clone + Send + Sync + 'static;
    fn apply(&mut self, change: &Self::Change) -> Result<Self::Change, String>;
}

/// Replace a UTF-8 byte range. Both ends must be character boundaries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextChange {
    pub range: Range<usize>,
    pub replacement: String,
}

/// One accepted content transition. Changes use successive pre-change states;
/// `None` means a whole-model replacement and requires refreshing derived data.
/// This journal is independent of undo history and includes undo/redo.
#[derive(Clone, Debug)]
pub struct DocumentCommit<C> {
    pub revision: u64,
    pub changes: Option<Vec<C>>,
}

const CHANGE_JOURNAL_LIMIT: usize = 128;

/// Read transitions after `revision` without consuming another reader's updates.
/// `None` means the bounded journal no longer covers that revision. Refresh from
/// the current model in that case; never interpret a gap as no changes.
pub fn document_changes_since<M: DocumentModel>(
    world: &World,
    handle: DocumentHandle<M>,
    revision: u64,
) -> Result<Option<Vec<DocumentCommit<M::Change>>>, DocumentError> {
    let session = typed_session::<M>(world, handle.id)?;
    if revision > session.revision {
        return Err(DocumentError::StaleRevision {
            expected: revision,
            actual: session.revision,
        });
    }
    if revision == session.revision {
        return Ok(Some(Vec::new()));
    }
    if session
        .commits
        .front()
        .is_none_or(|first| first.revision > revision + 1)
    {
        return Ok(None);
    }
    Ok(Some(
        session
            .commits
            .iter()
            .filter(|commit| commit.revision > revision)
            .cloned()
            .collect(),
    ))
}

impl DocumentModel for String {
    type Change = TextChange;
    fn apply(&mut self, change: &TextChange) -> Result<TextChange, String> {
        let range = change.range.clone();
        if range.start > range.end
            || range.end > self.len()
            || !self.is_char_boundary(range.start)
            || !self.is_char_boundary(range.end)
        {
            return Err("Text change is not a valid UTF-8 byte range".into());
        }
        let inverse = TextChange {
            range: range.start..range.start + change.replacement.len(),
            replacement: self[range.clone()].to_owned(),
        };
        self.replace_range(range, &change.replacement);
        Ok(inverse)
    }
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

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SourceVersion(pub String);

impl From<&str> for SourceVersion {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl From<String> for SourceVersion {
    fn from(value: String) -> Self {
        Self(value)
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
    /// Application identity, deduplicated only within this workspace. Reopening
    /// an existing key returns its session and does not replace its contents.
    pub source_key: Option<String>,
    pub initial_state: DocumentInitialState,
    /// Maximum retained undo transactions. Zero disables history, not editing.
    pub history_limit: usize,
}

impl DocumentOptions {
    pub fn loaded(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            source_key: None,
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
/// Moving/removing its InWorkspace relationship directly is not supported.
#[derive(Component)]
pub struct Document {
    workspace: Entity,
    title: String,
    source_key: Option<String>,
    model_type: TypeId,
    model_name: &'static str,
    state: Box<dyn ErasedSession>,
}

impl Document {
    pub fn workspace(&self) -> Entity {
        self.workspace
    }
    pub fn title(&self) -> &str {
        &self.title
    }
    pub fn source_key(&self) -> Option<&str> {
        self.source_key.as_deref()
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

/// Application declaration that a bound panel has uncommitted input. Remove it
/// only after explicitly committing or abandoning that input. Save, undo, redo
/// and document/workspace retirement reject drafts rather than using old content.
#[derive(Component, Default, Debug)]
pub struct DocumentDraft;

#[derive(Clone, Debug)]
pub struct DocumentInfo {
    pub id: DocumentId,
    pub workspace: Entity,
    pub title: String,
    pub source_key: Option<String>,
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
    WorkspaceUnavailable(Entity),
    WorkspaceClosing(Entity),
    WrongWorkspace,
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
            Self::Unavailable(id) => write!(f, "Document {id:?} is unavailable"),
            Self::WorkspaceUnavailable(entity) => write!(f, "Workspace {entity:?} is unavailable"),
            Self::WorkspaceClosing(entity) => write!(f, "Workspace {entity:?} is closing"),
            Self::WrongWorkspace => {
                f.write_str("Document and viewer belong to different workspaces")
            }
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
    pub revision: u64,
    pub source_generation: u64,
    pub expected_source_version: Option<SourceVersion>,
}

type SaveResult = Result<Option<SourceVersion>, DocumentSaveError>;

type SaveCallback<M> = dyn Fn(DocumentSaveSnapshot<M>) -> SaveResult + Send + Sync;

pub struct DocumentSaveBinding<M: DocumentModel> {
    destination_key: Option<String>,
    callback: Arc<SaveCallback<M>>,
}

impl<M: DocumentModel> DocumentSaveBinding<M> {
    /// Equal destination keys serialize saves across documents and workspaces in
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

impl<M: DocumentModel> Clone for DocumentSaveBinding<M> {
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

pub fn open_document<M: DocumentModel>(
    world: &mut World,
    workspace: Entity,
    model: M,
    options: DocumentOptions,
) -> Result<DocumentHandle<M>, DocumentError> {
    validate_workspace(world, workspace)?;
    if let Some(key) = &options.source_key {
        for id in document_ids(world, workspace) {
            if world.get::<Document>(id.0).unwrap().source_key.as_ref() == Some(key) {
                return cast_document(world, id);
            }
        }
    }
    let state = Session::new(model, &options);
    let entity = world
        .spawn((
            Document {
                workspace,
                title: options.title,
                source_key: options.source_key,
                model_type: TypeId::of::<M>(),
                model_name: type_name::<M>(),
                state: Box::new(state),
            },
            InWorkspace(workspace),
        ))
        .id();
    Ok(DocumentHandle {
        id: DocumentId(entity),
        marker: PhantomData,
    })
}

pub fn cast_document<M: DocumentModel>(
    world: &World,
    id: DocumentId,
) -> Result<DocumentHandle<M>, DocumentError> {
    typed_session::<M>(world, id)?;
    Ok(DocumentHandle {
        id,
        marker: PhantomData,
    })
}

pub fn read_document<M: DocumentModel>(
    world: &World,
    handle: DocumentHandle<M>,
) -> Result<&M, DocumentError> {
    Ok(&typed_session::<M>(world, handle.id)?.content)
}

pub fn document_ids(world: &World, workspace: Entity) -> Vec<DocumentId> {
    let Some(mut query) = world.try_query::<(Entity, &Document)>() else {
        return Vec::new();
    };
    let mut ids: Vec<_> = query
        .iter(world)
        .filter_map(|(entity, document)| {
            (document.workspace == workspace).then_some(DocumentId(entity))
        })
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
        workspace: document.workspace,
        title: document.title.clone(),
        source_key: document.source_key.clone(),
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
    let mut candidate = session.content.clone();
    let mut inverses = Vec::new();
    let mut committed = Vec::new();
    for change in changes {
        inverses.push(
            candidate
                .apply(&change)
                .map_err(DocumentError::InvalidChange)?,
        );
        committed.push(change);
    }
    inverses.reverse();
    if options.group.is_none() {
        session.group = None;
    }
    if candidate == session.content {
        return Ok(session.revision);
    }
    let revision = next_counter(session.revision);
    session.redo.clear();
    if options.group.is_some()
        && session
            .undo
            .back()
            .is_some_and(|entry| entry.group == options.group)
    {
        let last = session.undo.back_mut().unwrap();
        match &mut last.record {
            Record::Changes(previous) => {
                inverses.append(previous);
                *previous = inverses;
            }
            Record::Replace(_) => unreachable!("Only ordinary edits belong to explicit groups"),
        }
    } else {
        session.push_undo(HistoryEntry {
            record: Record::Changes(inverses),
            label: options.label,
            group: options.group,
        });
    }
    session.content = candidate;
    session.revision = revision;
    session.record_commit(Some(committed));
    Ok(revision)
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
    typed_session::<M>(world, handle.id)?;
    if world.get::<PanelInstance>(panel).is_none() {
        return Err(DocumentError::InvalidPanel(panel));
    }
    if world.get::<InWorkspace>(panel).map(|owner| owner.0) != Some(document.workspace) {
        return Err(DocumentError::WrongWorkspace);
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

pub fn bind_document_save<M: DocumentModel>(
    world: &mut World,
    handle: DocumentHandle<M>,
    binding: DocumentSaveBinding<M>,
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

pub fn unbind_document_save(world: &mut World, id: DocumentId) -> Result<(), DocumentError> {
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
    let workspace = document.workspace;
    let (destination, job) = document.state.prepare_save(id)?;
    world.init_resource::<SaveDestinations>();
    if let Some(key) = &destination {
        if world.resource::<SaveDestinations>().0.contains_key(key) {
            return Err(DocumentError::DestinationBusy(key.clone()));
        }
    }
    let task = world
        .get_mut::<WorkspaceWork>(workspace)
        .ok_or(DocumentError::WorkspaceUnavailable(workspace))?
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
    validate_mutation(world, handle.id)?;
    let mut document = world.get_mut::<Document>(handle.id.0).unwrap();
    let session = session_mut::<M>(&mut document)?;
    session.check_source_generation(expected_source_generation)?;
    let same_pending = session
        .conflict
        .as_ref()
        .is_some_and(|pending| pending.incoming == incoming && pending.version == version);
    let same_base = session.baseline.as_ref() == Some(&incoming);
    let duplicate = !session.refresh_required
        && ((same_pending && (session.content != incoming || session.saving.is_some()))
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
    if session.content == incoming {
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
        local: session.content.clone(),
        incoming: conflict.incoming.clone(),
        incoming_version: conflict.version.clone(),
        revision: session.revision,
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
    Ok(session.revision)
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

pub fn check_workspace_documents(world: &World, workspace: Entity) -> Result<(), DocumentError> {
    validate_workspace(world, workspace)?;
    for id in document_ids(world, workspace) {
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
    let workspace = get_document(world, id)?.workspace;
    #[cfg(feature = "ui")]
    if world
        .get::<crate::WorkspaceUi>(workspace)
        .is_some_and(|ui| ui.drawing)
    {
        return Err(DocumentError::InvalidGraph(
            "Cannot retire documents during workspace drawing".into(),
        ));
    }
    crate::reconcile_workbench(world).map_err(DocumentError::InvalidGraph)?;
    // Initializing newly composed viewers runs application code and can admit
    // edits, drafts or a save. The final gate must observe those effects too.
    check_document_close(world, id, discard)?;
    let panels: Vec<_> = world
        .query::<(Entity, &DocumentView)>()
        .iter(world)
        .filter_map(|(entity, view)| (view.document == id).then_some(entity))
        .collect();
    let mut workbench = Workbench::with_workspace(world, WorkspaceId(workspace))
        .map_err(DocumentError::InvalidGraph)?;
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
        if world.get::<Workspace>(document.workspace).is_none()
            || world.get::<InWorkspace>(entity).map(|owner| owner.0) != Some(document.workspace)
        {
            return Err("Document has invalid workspace ownership".into());
        }
        if let Some(key) = &document.source_key {
            if sources
                .insert((document.workspace, key.clone()), entity)
                .is_some()
            {
                return Err("Duplicate document source key within a workspace".into());
            }
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
        if world.get::<InWorkspace>(panel).map(|owner| owner.0) != Some(document.workspace) {
            return Err(DocumentError::WrongWorkspace.to_string());
        }
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
            poll_saves_system.before(crate::WorkspaceSystems::Prepare),
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
    document
        .state
        .as_any_mut()
        .downcast_mut::<Session<M>>()
        .ok_or(DocumentError::WrongModel {
            expected: type_name::<M>(),
            actual: document.model_name,
        })
}

fn validate_workspace(world: &World, workspace: Entity) -> Result<(), DocumentError> {
    if world.get::<Workspace>(workspace).is_none() {
        return Err(DocumentError::WorkspaceUnavailable(workspace));
    }
    if world
        .get::<crate::layout::WorkspaceRuntime>(workspace)
        .is_none_or(|runtime| runtime.closing)
        || world
            .get::<WorkspaceWork>(workspace)
            .is_none_or(WorkspaceWork::is_closing)
    {
        return Err(DocumentError::WorkspaceClosing(workspace));
    }
    Ok(())
}

fn validate_mutation(world: &World, id: DocumentId) -> Result<(), DocumentError> {
    let document = get_document(world, id)?;
    validate_workspace(world, document.workspace)
}

fn validate_viewer(world: &World, viewer: Entity, id: DocumentId) -> Result<(), DocumentError> {
    let view = world
        .get::<DocumentView>(viewer)
        .ok_or(DocumentError::InvalidPanel(viewer))?;
    if view.document != id || world.get::<PanelInstance>(viewer).is_none() {
        return Err(DocumentError::InvalidPanel(viewer));
    }
    if world.get::<InWorkspace>(viewer).map(|owner| owner.0)
        != Some(get_document(world, id)?.workspace)
    {
        return Err(DocumentError::WrongWorkspace);
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
    fn undo(&mut self, redo: bool) -> Result<u64, DocumentError>;
    fn unbind_save(&mut self) -> Result<(), DocumentError>;
    fn prepare_save(&self, id: DocumentId) -> Result<(Option<String>, SaveJob), DocumentError>;
    fn save_started(&mut self, task: crate::WorkspaceTask<SaveResult>);
    fn poll_save(&mut self, id: DocumentId) -> Option<(Option<String>, DocumentSaveFinished)>;
}

struct Session<M: DocumentModel> {
    content: M,
    /// Acknowledged source knowledge is deliberately outside undo history.
    baseline: Option<M>,
    revision: u64,
    commits: VecDeque<DocumentCommit<M::Change>>,
    source_generation: u64,
    source_version: Option<SourceVersion>,
    undo: VecDeque<HistoryEntry<M>>,
    redo: Vec<HistoryEntry<M>>,
    history_limit: usize,
    next_group: u64,
    group: Option<DocumentUndoGroup>,
    binding: Option<DocumentSaveBinding<M>>,
    binding_generation: u64,
    saving: Option<ActiveSave<M>>,
    conflict: Option<Incoming<M>>,
    refresh_required: bool,
    last_save_error: Option<DocumentSaveError>,
}

struct Incoming<M> {
    base: Option<M>,
    incoming: M,
    version: Option<SourceVersion>,
}

enum Record<M: DocumentModel> {
    Changes(Vec<M::Change>),
    Replace(M),
}

struct HistoryEntry<M: DocumentModel> {
    record: Record<M>,
    label: String,
    group: Option<DocumentUndoGroup>,
}

struct ActiveSave<M: DocumentModel> {
    snapshot: M,
    revision: u64,
    source_generation: u64,
    binding_generation: u64,
    destination: Option<String>,
    task: crate::WorkspaceTask<SaveResult>,
}

impl<M: DocumentModel> Session<M> {
    fn new(content: M, options: &DocumentOptions) -> Self {
        let (baseline, source_version) = match &options.initial_state {
            DocumentInitialState::Loaded(version) => (Some(content.clone()), version.clone()),
            DocumentInitialState::Unsaved => (None, None),
        };
        Self {
            content,
            baseline,
            revision: 0,
            commits: VecDeque::new(),
            source_generation: 0,
            source_version,
            undo: VecDeque::new(),
            redo: Vec::new(),
            history_limit: options.history_limit,
            next_group: 0,
            group: None,
            binding: None,
            binding_generation: 0,
            saving: None,
            conflict: None,
            refresh_required: false,
            last_save_error: None,
        }
    }
    fn dirty(&self) -> bool {
        self.baseline.as_ref() != Some(&self.content)
    }
    fn record_commit(&mut self, changes: Option<Vec<M::Change>>) {
        self.commits.push_back(DocumentCommit {
            revision: self.revision,
            changes,
        });
        while self.commits.len() > CHANGE_JOURNAL_LIMIT {
            self.commits.pop_front();
        }
    }
    fn check_revision(&self, expected: u64) -> Result<(), DocumentError> {
        if expected != self.revision {
            return Err(DocumentError::StaleRevision {
                expected,
                actual: self.revision,
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
    fn push_undo(&mut self, entry: HistoryEntry<M>) {
        if self.history_limit == 0 {
            return;
        }
        self.undo.push_back(entry);
        while self.undo.len() > self.history_limit {
            self.undo.pop_front();
        }
    }
    fn replace_content(&mut self, model: M, label: &str) {
        self.group = None;
        if model == self.content {
            return;
        }
        let previous = self.content.clone();
        self.revision = next_counter(self.revision);
        self.content = model;
        self.record_commit(None);
        self.redo.clear();
        self.push_undo(HistoryEntry {
            record: Record::Replace(previous),
            label: label.into(),
            group: None,
        });
    }
    fn apply_record(&self, record: &Record<M>) -> Result<(M, Record<M>), DocumentError> {
        match record {
            Record::Replace(previous) => {
                Ok((previous.clone(), Record::Replace(self.content.clone())))
            }
            Record::Changes(changes) => {
                let mut candidate = self.content.clone();
                let mut inverse = Vec::new();
                for change in changes {
                    inverse.push(
                        candidate
                            .apply(change)
                            .map_err(DocumentError::InvalidChange)?,
                    );
                }
                inverse.reverse();
                Ok((candidate, Record::Changes(inverse)))
            }
        }
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
            revision: self.revision,
            source_generation: self.source_generation,
            source_version: self.source_version.clone(),
            dirty: self.dirty(),
            saving: self.saving.is_some(),
            conflict: self.conflict.is_some() || self.refresh_required,
            requires_source_refresh: self.refresh_required,
            has_save_binding: self.binding.is_some(),
            undo_label: self.undo.back().map(|entry| entry.label.clone()),
            redo_label: self.redo.last().map(|entry| entry.label.clone()),
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
    fn undo(&mut self, redo: bool) -> Result<u64, DocumentError> {
        let entry = if redo {
            self.redo.last()
        } else {
            self.undo.back()
        }
        .ok_or(if redo {
            DocumentError::NothingToRedo
        } else {
            DocumentError::NothingToUndo
        })?;
        let (content, record) = self.apply_record(&entry.record)?;
        let changes = match &entry.record {
            Record::Changes(changes) => Some(changes.clone()),
            Record::Replace(_) => None,
        };
        let label = entry.label.clone();
        let revision = next_counter(self.revision);
        if redo {
            self.redo.pop();
            self.push_undo(HistoryEntry {
                record,
                label,
                group: None,
            });
        } else {
            self.undo.pop_back();
            self.redo.push(HistoryEntry {
                record,
                label,
                group: None,
            });
        }
        self.group = None;
        self.content = content;
        self.revision = revision;
        self.record_commit(changes);
        Ok(revision)
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
            model: self.content.clone(),
            revision: self.revision,
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
    fn save_started(&mut self, task: crate::WorkspaceTask<SaveResult>) {
        self.group = None;
        self.last_save_error = None;
        self.saving = Some(ActiveSave {
            snapshot: self.content.clone(),
            revision: self.revision,
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

#[cfg(test)]
mod tests;
