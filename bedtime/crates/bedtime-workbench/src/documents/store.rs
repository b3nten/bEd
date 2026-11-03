//! One document's state, typed by its model: the source, whether it has loaded,
//! the editing session, and the single source task (read or save) in flight.
use super::*;
use crate::ManagedTask;
use bedtime_document::History;
use std::{
    any::Any,
    sync::{Arc, mpsc::TryRecvError},
    time::{Duration, Instant},
};

const HISTORY_LIMIT: usize = 100;

/// The type-erased operations workbench needs without knowing the model.
pub(super) trait Store: Send + Sync {
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
    fn source_id(&self) -> SourceId;
    fn title(&self) -> String;
    fn info(&self) -> StoreInfo;
    fn reading(&self) -> bool;
    fn undo(&mut self, redo: bool) -> Result<u64, DocumentError>;
    fn begin_group(
        &mut self,
        id: DocumentId,
        viewer: Entity,
    ) -> Result<DocumentUndoGroup, DocumentError>;
    fn end_group(&mut self, group: DocumentUndoGroup) -> Result<(), DocumentError>;
    fn detach_viewer(&mut self, viewer: Entity);
    fn start_save(&mut self, work: &mut ManagedWork, owner: Entity) -> Result<(), DocumentError>;
    fn refresh(&mut self);
    /// Accept a finished task and start a read if one is due and allowed.
    /// Returns the outcome of a save that just finished.
    fn poll(
        &mut self,
        work: &mut ManagedWork,
        owner: Entity,
        may_read: bool,
    ) -> Option<Result<(), String>>;
}

pub(super) struct StoreInfo {
    pub status: DocumentStatus,
    pub revision: u64,
    pub dirty: bool,
    pub saving: bool,
    pub conflict: bool,
    pub missing: bool,
    pub writable: bool,
    pub error: Option<String>,
    pub undo_label: Option<String>,
    pub redo_label: Option<String>,
}

pub(super) struct Typed<M: DocumentModel> {
    source: Arc<dyn DocumentSource<M>>,
    state: State<M>,
    task: Option<Task<M>>,
    /// A full read was asked for (initially, by `refresh`, or after a failed save).
    refresh: bool,
    next_check: Instant,
    /// What the next save expects to find, after the source was replaced
    /// (`None`: nothing there).
    next_expected: Option<Option<SourceVersion>>,
}

enum State<M: DocumentModel> {
    Loading,
    Failed(String),
    Ready(Session<M>),
}

enum Task<M: DocumentModel> {
    /// `None` when a version check found nothing new.
    Read(ManagedTask<Result<Option<SourceLoad<M>>, SourceError>>),
    Save {
        snapshot: M,
        task: ManagedTask<Result<Option<SourceVersion>, SourceError>>,
    },
}

/// What a document's source held when last loaded or saved.
#[derive(Clone, PartialEq)]
struct Saved<M> {
    model: M,
    version: Option<SourceVersion>,
}

pub(super) struct Session<M: DocumentModel> {
    pub(super) history: History<M, DocumentUndoGroup>,
    /// `None` for contents never saved.
    saved: Option<Saved<M>>,
    /// An outside change that arrived while there were unsaved changes.
    incoming: Option<Saved<M>>,
    missing: bool,
    error: Option<String>,
    group: Option<DocumentUndoGroup>,
    next_group: u64,
}

impl<M: DocumentModel> Typed<M> {
    pub(super) fn new(source: impl DocumentSource<M>) -> Self {
        Self {
            source: Arc::new(source),
            state: State::Loading,
            task: None,
            refresh: true,
            next_check: Instant::now(),
            next_expected: None,
        }
    }

    /// Keep the contents somewhere else from now on (Save As): the next save
    /// writes there, expecting to find `expected`.
    pub(super) fn replace_source(
        &mut self,
        source: Arc<dyn DocumentSource<M>>,
        expected: Option<SourceVersion>,
    ) -> Result<(), DocumentError> {
        if self.saving() {
            return Err(DocumentError::Saving);
        }
        if let Some(Task::Read(read)) = self.task.take() {
            read.cancel();
        }
        let session = self.session_mut()?;
        session.incoming = None;
        session.missing = false;
        session.error = None;
        self.source = source;
        self.next_expected = Some(expected);
        Ok(())
    }

    pub(super) fn session(&self) -> Result<&Session<M>, DocumentError> {
        match &self.state {
            State::Loading => Err(DocumentError::Loading),
            State::Failed(error) => Err(DocumentError::LoadFailed(error.clone())),
            State::Ready(session) => Ok(session),
        }
    }

    pub(super) fn session_mut(&mut self) -> Result<&mut Session<M>, DocumentError> {
        match &mut self.state {
            State::Loading => Err(DocumentError::Loading),
            State::Failed(error) => Err(DocumentError::LoadFailed(error.clone())),
            State::Ready(session) => Ok(session),
        }
    }

    fn saving(&self) -> bool {
        matches!(self.task, Some(Task::Save { .. }))
    }

    fn read_due(&self) -> bool {
        match &self.state {
            State::Loading | State::Failed(_) => self.refresh,
            State::Ready(_) => {
                self.refresh
                    || (self.source.check_interval().is_some() && Instant::now() >= self.next_check)
            }
        }
    }

    fn start_read(&mut self, work: &mut ManagedWork, owner: Entity) {
        // A periodic check first asks for the version, and skips the load if unchanged.
        let check_only = !self.refresh && matches!(self.state, State::Ready(_));
        let known = match &self.state {
            State::Ready(session) => session.saved.as_ref().and_then(|s| s.version.clone()),
            _ => None,
        };
        self.refresh = false;
        self.next_check = Instant::now()
            + self
                .source
                .check_interval()
                .unwrap_or(Duration::from_secs(1));
        let source = self.source.clone();
        let task = work.spawn_task(Some(owner), "Read document", move |_| {
            if check_only
                && let Some(version) = source.version()?
                && Some(&version) == known.as_ref()
            {
                return Ok(None);
            }
            source.load().map(Some)
        });
        match task {
            Ok(task) => self.task = Some(Task::Read(task)),
            Err(error) => self.accept_read(Err(SourceError::Failed(error))),
        }
    }

    fn accept_read(&mut self, result: Result<Option<SourceLoad<M>>, SourceError>) {
        match (&mut self.state, result) {
            (State::Ready(session), Ok(Some(load))) => session.offer(load.model, load.version),
            (State::Ready(_), Ok(None)) => {}
            (State::Ready(session), Err(SourceError::Missing)) => session.missing = true,
            (State::Ready(session), Err(error)) => {
                session.error = Some(format!("Couldn't check for outside changes: {error}"))
            }
            (_, Ok(Some(load))) => self.state = State::Ready(Session::new(load)),
            (_, Ok(None)) => {}
            (_, Err(error)) => self.state = State::Failed(error.to_string()),
        }
    }

    fn accept_save(
        &mut self,
        snapshot: M,
        result: Result<Option<SourceVersion>, SourceError>,
    ) -> Result<(), String> {
        let State::Ready(session) = &mut self.state else {
            unreachable!("Only ready documents save");
        };
        match result {
            Ok(version) => {
                session.saved = Some(Saved {
                    model: snapshot,
                    version,
                });
                session.missing = false;
                session.error = None;
                Ok(())
            }
            Err(error) => {
                // Find out what the source holds now; if it changed, that's a conflict.
                self.refresh = true;
                let error = format!("Couldn't save: {error}");
                session.error = Some(error.clone());
                Err(error)
            }
        }
    }
}

impl<M: DocumentModel> Store for Typed<M> {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn source_id(&self) -> SourceId {
        self.source.id()
    }

    fn title(&self) -> String {
        self.source.title()
    }

    fn info(&self) -> StoreInfo {
        let writable = self.source.writable();
        let mut info = StoreInfo {
            status: DocumentStatus::Loading,
            revision: 0,
            dirty: false,
            saving: self.saving(),
            conflict: false,
            missing: false,
            writable,
            error: None,
            undo_label: None,
            redo_label: None,
        };
        match &self.state {
            State::Loading => {}
            State::Failed(error) => info.status = DocumentStatus::Failed(error.clone()),
            State::Ready(session) => {
                info.status = DocumentStatus::Ready;
                info.revision = session.history.revision();
                info.dirty = session.dirty();
                info.conflict = session.incoming.is_some();
                info.missing = session.missing;
                info.error = session.error.clone();
                info.undo_label = session.history.undo_label();
                info.redo_label = session.history.redo_label();
            }
        }
        info
    }

    fn reading(&self) -> bool {
        matches!(self.task, Some(Task::Read(_)))
    }

    fn undo(&mut self, redo: bool) -> Result<u64, DocumentError> {
        let session = self.session_mut()?;
        session.group = None;
        session
            .history
            .undo(redo)
            .map_err(DocumentError::InvalidChange)?
            .ok_or(if redo {
                DocumentError::NothingToRedo
            } else {
                DocumentError::NothingToUndo
            })
    }

    fn begin_group(
        &mut self,
        id: DocumentId,
        viewer: Entity,
    ) -> Result<DocumentUndoGroup, DocumentError> {
        let session = self.session_mut()?;
        session.next_group += 1;
        let group = DocumentUndoGroup {
            document: id,
            viewer,
            serial: session.next_group,
        };
        session.group = Some(group);
        Ok(group)
    }

    fn end_group(&mut self, group: DocumentUndoGroup) -> Result<(), DocumentError> {
        let session = self.session_mut()?;
        if session.group != Some(group) {
            return Err(DocumentError::StaleUndoGroup);
        }
        session.group = None;
        Ok(())
    }

    fn detach_viewer(&mut self, viewer: Entity) {
        if let State::Ready(session) = &mut self.state
            && session.group.is_some_and(|group| group.viewer == viewer)
        {
            session.group = None;
        }
    }

    fn start_save(&mut self, work: &mut ManagedWork, owner: Entity) -> Result<(), DocumentError> {
        if self.saving() {
            return Err(DocumentError::Saving);
        }
        let writable = self.source.writable();
        let session = self.session_mut()?;
        if !writable {
            return Err(DocumentError::ReadOnly);
        }
        if session.missing {
            return Err(DocumentError::Missing);
        }
        if session.incoming.is_some() {
            return Err(DocumentError::Conflict);
        }
        let snapshot = session.history.content().clone();
        let saved_version = session
            .saved
            .as_ref()
            .and_then(|saved| saved.version.clone());
        session.group = None;
        session.error = None;
        let expected = self.next_expected.take().unwrap_or(saved_version);
        let source = self.source.clone();
        let model = snapshot.clone();
        let task = work
            .spawn_task(Some(owner), "Save document", move |_| {
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    source.save(model, expected)
                }))
                .unwrap_or_else(|_| Err(SourceError::Failed("The save crashed".into())))
            })
            .map_err(DocumentError::Work)?;
        // Any read still running started before this save; its result is out of date.
        if let Some(Task::Read(read)) = self.task.take() {
            read.cancel();
        }
        self.task = Some(Task::Save { snapshot, task });
        Ok(())
    }

    fn refresh(&mut self) {
        self.refresh = true;
    }

    fn poll(
        &mut self,
        work: &mut ManagedWork,
        owner: Entity,
        may_read: bool,
    ) -> Option<Result<(), String>> {
        let mut finished = None;
        match self.task.take() {
            Some(Task::Read(task)) => match task.try_recv() {
                Err(TryRecvError::Empty) => self.task = Some(Task::Read(task)),
                Ok(result) => self.accept_read(result),
                Err(TryRecvError::Disconnected) => {
                    self.accept_read(Err(SourceError::Failed("The read stopped".into())))
                }
            },
            Some(Task::Save { snapshot, task }) => match task.try_recv() {
                Err(TryRecvError::Empty) => self.task = Some(Task::Save { snapshot, task }),
                Ok(result) => finished = Some(self.accept_save(snapshot, result)),
                Err(TryRecvError::Disconnected) => {
                    let result = Err(SourceError::Failed("The save was cancelled".into()));
                    finished = Some(self.accept_save(snapshot, result));
                }
            },
            None => {}
        }
        if may_read && self.task.is_none() && self.read_due() {
            self.start_read(work, owner);
        }
        finished
    }
}

impl<M: DocumentModel> Session<M> {
    fn new(load: SourceLoad<M>) -> Self {
        let saved = load.saved.then(|| Saved {
            model: load.model.clone(),
            version: load.version,
        });
        Self {
            history: History::new(load.model, HISTORY_LIMIT),
            saved,
            incoming: None,
            missing: false,
            error: None,
            group: None,
            next_group: 0,
        }
    }

    fn dirty(&self) -> bool {
        self.saved.as_ref().map(|saved| &saved.model) != Some(self.history.content())
    }

    pub(super) fn edit(
        &mut self,
        id: DocumentId,
        expected_revision: u64,
        changes: impl IntoIterator<Item = M::Change>,
        options: EditOptions,
    ) -> Result<u64, DocumentError> {
        let revision = self.history.revision();
        if expected_revision != revision {
            return Err(DocumentError::StaleRevision {
                expected: expected_revision,
                actual: revision,
            });
        }
        if let Some(group) = options.group
            && (self.group != Some(group)
                || group.document != id
                || options.viewer != Some(group.viewer))
        {
            return Err(DocumentError::StaleUndoGroup);
        }
        if options.group.is_none() {
            self.group = None;
        }
        self.history
            .edit(changes, options.label, options.group)
            .map_err(DocumentError::InvalidChange)
    }

    /// Take in what the source holds now.
    fn offer(&mut self, model: M, version: Option<SourceVersion>) {
        self.missing = false;
        self.error = None;
        let incoming = Saved { model, version };
        if let Some(saved) = &mut self.saved
            && saved.model == incoming.model
        {
            // Unchanged contents (perhaps a new version token), or the source
            // went back to what we last had: any pending conflict is over.
            saved.version = incoming.version;
            self.incoming = None;
            return;
        }
        if *self.history.content() == incoming.model {
            // The source caught up with our edits.
            self.saved = Some(incoming);
            self.incoming = None;
        } else if !self.dirty() {
            self.group = None;
            self.history
                .replace(incoming.model.clone(), "Outside change");
            self.saved = Some(incoming);
            self.incoming = None;
        } else {
            self.incoming = Some(incoming);
        }
    }

    pub(super) fn conflict(&self) -> Option<DocumentConflict<M>> {
        let incoming = self.incoming.as_ref()?;
        Some(DocumentConflict {
            base: self.saved.as_ref().map(|saved| saved.model.clone()),
            local: self.history.content().clone(),
            incoming: incoming.model.clone(),
        })
    }

    pub(super) fn resolve(
        &mut self,
        expected_revision: u64,
        resolution: DocumentConflictResolution<M>,
    ) -> Result<u64, DocumentError> {
        if expected_revision != self.history.revision() {
            return Err(DocumentError::StaleRevision {
                expected: expected_revision,
                actual: self.history.revision(),
            });
        }
        let incoming = self.incoming.take().ok_or(DocumentError::NoConflict)?;
        self.group = None;
        match resolution {
            DocumentConflictResolution::AcceptIncoming => self
                .history
                .replace(incoming.model.clone(), "Use outside change"),
            DocumentConflictResolution::KeepLocal => {}
            DocumentConflictResolution::Merged(model) => {
                self.history.replace(model, "Merge outside change")
            }
        }
        // The next save expects the source to hold what came in.
        self.saved = Some(incoming);
        Ok(self.history.revision())
    }
}
