//! Source-backed execution; workers never mutate canonical state.
use super::*;
pub use bedtime_document2::{
    DocumentSource, MemorySource, SourceError, SourceId, SourceLoad, SourceSave,
};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DocumentLoadState {
    Loading,
    Ready,
    Failed(String),
}

struct Unloaded;
impl ErasedSession for Unloaded {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    fn info(&self) -> SessionInfo {
        SessionInfo {
            revision: 0,
            source_generation: 0,
            source_version: None,
            dirty: false,
            saving: false,
            conflict: false,
            requires_source_refresh: false,
            has_save_binding: false,
            undo_label: None,
            redo_label: None,
            last_save_error: None,
        }
    }
    fn begin_group(&mut self, _: DocumentId, _: Entity) -> DocumentUndoGroup {
        unreachable!("Loading documents cannot edit")
    }
    fn end_group(&mut self, _: DocumentUndoGroup) -> Result<(), DocumentError> {
        Err(DocumentError::Loading)
    }
    fn detach_viewer(&mut self, _: Entity) {}
    fn undo(&mut self, _: bool) -> Result<u64, DocumentError> {
        Err(DocumentError::Loading)
    }
    fn unbind_save(&mut self) -> Result<(), DocumentError> {
        Err(DocumentError::Loading)
    }
    fn prepare_save(&self, _: DocumentId) -> Result<(Option<String>, SaveJob), DocumentError> {
        Err(DocumentError::Loading)
    }
    fn save_started(&mut self, _: crate::ManagedTask<SaveResult>) {
        unreachable!()
    }
    fn poll_save(&mut self, _: DocumentId) -> Option<(Option<String>, DocumentSaveFinished)> {
        None
    }
}

#[derive(Component)]
pub(crate) struct DocumentSourceRuntime {
    runtime: Box<dyn SourceRuntime>,
}
trait SourceRuntime: Send + Sync {
    fn poll(&mut self, world: &mut World, id: DocumentId, admit: bool);
    fn pending(&self) -> bool;
    fn path(&self) -> Option<std::path::PathBuf>;
    fn location(&self) -> Option<std::path::PathBuf>;
    fn invalidate(&mut self);
    fn refresh(&mut self);
    fn as_any(&self) -> &dyn Any;
    fn set_missing(&mut self);
    fn write_blocked(&self) -> bool;
}
struct Runtime<M: DocumentModel> {
    source: Arc<dyn DocumentSource<M>>,
    task: Option<crate::ManagedTask<Result<Option<SourceLoad<M>>, SourceError>>>,
    generation: u64,
    location_generation: u64,
    next_check: Instant,
    refresh: bool,
    missing: bool,
}
impl<M: DocumentModel> Drop for Runtime<M> {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.cancel();
        }
    }
}

/// Reserve the representation before loading. Equal source IDs share one document.
pub fn open_document<M: DocumentModel>(
    world: &mut World,
    source: impl DocumentSource<M>,
) -> Result<DocumentHandle<M>, DocumentError> {
    open_shared_document(world, Arc::new(source))
}
pub fn open_shared_document<M: DocumentModel>(
    world: &mut World,
    source: Arc<dyn DocumentSource<M>>,
) -> Result<DocumentHandle<M>, DocumentError> {
    validate_session(world)?;
    let source_id = source.id();
    for id in document_ids(world) {
        let document = get_document(world, id)?;
        if document.source == source_id {
            if document.model_type != TypeId::of::<M>() {
                return Err(DocumentError::WrongModel {
                    expected: type_name::<M>(),
                    actual: document.model_name,
                });
            }
            return Ok(DocumentHandle {
                id,
                marker: PhantomData,
            });
        }
    }
    let title = source.title();
    let entity = world
        .spawn(Document {
            title,
            model_type: TypeId::of::<M>(),
            model_name: type_name::<M>(),
            state: Box::new(Unloaded),
            source: source_id,
            source_missing: false,
            source_pending: false,
            load_state: DocumentLoadState::Loading,
        })
        .id();
    let id = DocumentId(entity);
    let mut runtime = Runtime {
        source,
        task: None,
        generation: 0,
        location_generation: 0,
        next_check: Instant::now(),
        refresh: true,
        missing: false,
    };
    runtime.poll(world, id, true);
    world.entity_mut(entity).insert(DocumentSourceRuntime {
        runtime: Box::new(runtime),
    });
    Ok(DocumentHandle {
        id,
        marker: PhantomData,
    })
}
/// Accept a provider's prepared load. Concurrent opens still resolve by identity.
pub fn open_prepared_document<M: DocumentModel>(
    world: &mut World,
    source: impl DocumentSource<M>,
    loaded: SourceLoad<M>,
) -> Result<DocumentHandle<M>, DocumentError> {
    validate_session(world)?;
    if let Some(id) = document_for_source(world, source.id()) {
        return cast_document(world, id);
    }
    let source: Arc<dyn DocumentSource<M>> = Arc::new(source);
    let options = DocumentOptions {
        title: source.title(),
        initial_state: if loaded.acknowledged {
            DocumentInitialState::Loaded(loaded.version)
        } else {
            DocumentInitialState::Unsaved
        },
        history_limit: if TypeId::of::<M>() == TypeId::of::<BinaryDocument>() {
            0
        } else {
            100
        },
    };
    let entity = world
        .spawn(Document {
            title: options.title.clone(),
            model_type: TypeId::of::<M>(),
            model_name: type_name::<M>(),
            state: Box::new(Session::new(loaded.model, &options)),
            source: source.id(),
            source_missing: false,
            source_pending: false,
            load_state: DocumentLoadState::Ready,
        })
        .id();
    let id = DocumentId(entity);
    let handle = DocumentHandle {
        id,
        marker: PhantomData,
    };
    install_source_save(world, handle, source.clone());
    world.entity_mut(entity).insert(DocumentSourceRuntime {
        runtime: Box::new(Runtime {
            source,
            task: None,
            generation: 0,
            location_generation: 0,
            next_check: Instant::now() + Duration::from_millis(500),
            refresh: false,
            missing: false,
        }),
    });
    Ok(handle)
}
fn install_source_save<M: DocumentModel>(
    world: &mut World,
    handle: DocumentHandle<M>,
    source: Arc<dyn DocumentSource<M>>,
) {
    if source.writable() {
        let key = format!("source:{:?}", source.id());
        let _ = install_save_binding(
            world,
            handle,
            SaveBinding::new(Some(key), move |snapshot: DocumentSaveSnapshot<M>| {
                source
                    .save(SourceSave {
                        model: snapshot.model,
                        baseline: snapshot.baseline,
                        expected_version: snapshot.expected_source_version,
                    })
                    .map_err(save_error)
            }),
        );
    }
}
impl Workbench<'_> {
    pub fn open_document<M: DocumentModel>(
        &mut self,
        source: impl DocumentSource<M>,
    ) -> Result<DocumentHandle<M>, DocumentError> {
        open_document(self.world, source)
    }
}

pub fn document_source_id(world: &World, id: DocumentId) -> Option<SourceId> {
    Some(world.get::<Document>(id.0)?.source)
}
pub fn document_source_path(world: &World, id: DocumentId) -> Option<std::path::PathBuf> {
    world.get::<DocumentSourceRuntime>(id.0)?.runtime.path()
}
pub fn refresh_document(world: &mut World, id: DocumentId) -> Result<(), DocumentError> {
    validate_mutation(world, id)?;
    world
        .get_mut::<DocumentSourceRuntime>(id.0)
        .ok_or(DocumentError::NoSaveBinding)?
        .runtime
        .refresh();
    world.get_mut::<Document>(id.0).unwrap().source_pending = true;
    Ok(())
}
pub fn mark_document_source_missing(
    world: &mut World,
    id: DocumentId,
) -> Result<(), DocumentError> {
    if let Some(mut runtime) = world.get_mut::<DocumentSourceRuntime>(id.0) {
        runtime.runtime.set_missing();
    }
    if let Some(mut document) = world.get_mut::<Document>(id.0) {
        document.source_missing = true;
    }
    remove_save_binding(world, id)
}
pub fn document_for_source(world: &World, source: SourceId) -> Option<DocumentId> {
    document_ids(world).into_iter().find(|id| {
        world
            .get::<Document>(id.0)
            .is_some_and(|d| d.source == source)
    })
}
pub fn source_document<M: DocumentModel>(
    world: &World,
    id: DocumentId,
) -> Option<Arc<dyn DocumentSource<M>>> {
    Some(
        world
            .get::<DocumentSourceRuntime>(id.0)?
            .runtime
            .as_any()
            .downcast_ref::<Runtime<M>>()?
            .source
            .clone(),
    )
}

#[derive(Resource, Default)]
struct SourceSchedule {
    cursor: Option<Entity>,
}
pub(crate) fn poll_sources(world: &mut World) {
    world.init_resource::<SourceSchedule>();
    let mut ids: Vec<_> = world
        .query_filtered::<Entity, With<DocumentSourceRuntime>>()
        .iter(world)
        .collect();
    ids.sort();
    // Accept every completed outcome before admitting the next bounded batch.
    for &entity in &ids {
        let Some(mut runtime) = world.entity_mut(entity).take::<DocumentSourceRuntime>() else {
            continue;
        };
        runtime.runtime.poll(world, DocumentId(entity), false);
        world.entity_mut(entity).insert(runtime);
    }
    let running = ids
        .iter()
        .filter(|id| {
            world
                .get::<DocumentSourceRuntime>(**id)
                .is_some_and(|r| r.runtime.pending())
        })
        .count();
    let mut budget = 64usize.saturating_sub(running);
    let cursor = world.resource::<SourceSchedule>().cursor;
    let ordered: Vec<_> = ids
        .iter()
        .copied()
        .filter(|e| cursor.is_none_or(|c| *e > c))
        .chain(
            ids.iter()
                .copied()
                .filter(|e| cursor.is_some_and(|c| *e <= c)),
        )
        .collect();
    for entity in ordered {
        if budget == 0 {
            break;
        }
        let Some(mut runtime) = world.entity_mut(entity).take::<DocumentSourceRuntime>() else {
            continue;
        };
        let was_pending = runtime.runtime.pending();
        runtime.runtime.poll(world, DocumentId(entity), true);
        let started = !was_pending && runtime.runtime.pending();
        world.entity_mut(entity).insert(runtime);
        if started {
            budget -= 1;
            world.resource_mut::<SourceSchedule>().cursor = Some(entity);
        }
    }
}

fn save_error(error: SourceError) -> DocumentSaveError {
    match error {
        SourceError::Changed(s) => DocumentSaveError::Conflict(s),
        SourceError::Uncertain => DocumentSaveError::SourceChanged,
        error => DocumentSaveError::Failed(error.to_string()),
    }
}
impl<M: DocumentModel> SourceRuntime for Runtime<M> {
    fn path(&self) -> Option<std::path::PathBuf> {
        if self.missing {
            None
        } else {
            self.source.path()
        }
    }
    fn location(&self) -> Option<std::path::PathBuf> {
        self.source.path()
    }
    fn invalidate(&mut self) {
        self.next_check = Instant::now();
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn refresh(&mut self) {
        self.refresh = true;
    }
    fn set_missing(&mut self) {
        self.missing = true;
        self.refresh = true;
    }
    fn write_blocked(&self) -> bool {
        self.source.write_blocked()
    }
    fn pending(&self) -> bool {
        self.task.is_some()
    }
    fn poll(&mut self, world: &mut World, id: DocumentId, admit: bool) {
        world.get_mut::<Document>(id.0).unwrap().title = self.source.title();
        if let Some(task) = &self.task {
            let result = match task.try_recv() {
                Ok(result) => Some(result),
                Err(TryRecvError::Empty) => None,
                Err(_) => Some(Err(SourceError::Failed("Source worker stopped".into()))),
            };
            if let Some(result) = result {
                self.task = None;
                world.get_mut::<Document>(id.0).unwrap().source_pending = false;
                if self.location_generation != self.source.location_generation() {
                    self.refresh = true;
                    return;
                }

                let initial = world
                    .get::<Document>(id.0)
                    .is_some_and(|d| d.load_state != DocumentLoadState::Ready);
                match result {
                    Ok(Some(loaded)) if initial => {
                        let options = DocumentOptions {
                            title: self.source.title(),
                            initial_state: if loaded.acknowledged {
                                DocumentInitialState::Loaded(loaded.version)
                            } else {
                                DocumentInitialState::Unsaved
                            },
                            history_limit: if TypeId::of::<M>() == TypeId::of::<BinaryDocument>() {
                                0
                            } else {
                                100
                            },
                        };
                        let mut document = world.get_mut::<Document>(id.0).unwrap();
                        document.state = Box::new(Session::new(loaded.model, &options));
                        document.load_state = DocumentLoadState::Ready;
                        install_source_save(
                            world,
                            DocumentHandle {
                                id,
                                marker: PhantomData,
                            },
                            self.source.clone(),
                        );
                        world.get_mut::<Document>(id.0).unwrap().source_missing = false;
                        self.missing = false;
                    }
                    Ok(Some(loaded)) => {
                        let handle = DocumentHandle {
                            id,
                            marker: PhantomData,
                        };
                        let applied = offer_document_external_update(
                            world,
                            handle,
                            self.generation,
                            loaded.model,
                            loaded.version,
                        );
                        if applied.is_ok() && self.missing {
                            install_source_save(world, handle, self.source.clone());
                        }
                        world.get_mut::<Document>(id.0).unwrap().source_missing = false;
                        self.missing = false;
                    }
                    Ok(None) => {}
                    Err(SourceError::Missing) if !initial => {
                        self.missing = true;
                        world.get_mut::<Document>(id.0).unwrap().source_missing = true;
                        let _ = remove_save_binding(world, id);
                    }
                    Err(error) if initial => {
                        world.get_mut::<Document>(id.0).unwrap().load_state =
                            DocumentLoadState::Failed(error.to_string());
                    }
                    Err(error) => {
                        if let Some(mut document) = world.get_mut::<Document>(id.0) {
                            if let Ok(session) = session_mut::<M>(&mut document) {
                                session.refresh_required = true;
                                session.last_save_error =
                                    Some(DocumentSaveError::Failed(error.to_string()));
                            }
                        }
                    }
                }
            }
        }
        if !admit || self.task.is_some() || world.resource::<ManagedWork>().is_closing() {
            return;
        }
        let now = Instant::now();
        if !self.refresh && self.source.observation_interval().is_none() {
            return;
        }
        if !self.refresh && now < self.next_check {
            return;
        }
        if let Ok(info) = inspect_document(world, id) {
            if info.saving || info.has_drafts {
                return;
            }
            self.generation = info.source_generation;
        }
        // An explicit refresh retries a failed load. Periodic checks retain its error.
        if world
            .get::<Document>(id.0)
            .is_some_and(|d| matches!(d.load_state, DocumentLoadState::Failed(_)))
            && !self.refresh
        {
            return;
        }
        let location_changed = self.location_generation != self.source.location_generation();
        self.location_generation = self.source.location_generation();
        let force = self.refresh
            || location_changed
            || self.missing
            || inspect_document(world, id).is_ok_and(|i| i.requires_source_refresh);
        self.refresh = false;
        self.next_check = now
            + self
                .source
                .observation_interval()
                .unwrap_or(Duration::from_millis(500));
        let source = self.source.clone();
        let initial = world.get::<Document>(id.0).unwrap().load_state != DocumentLoadState::Ready;
        let expected = inspect_document(world, id)
            .ok()
            .and_then(|info| info.source_version);
        let known: Vec<M> = typed_session::<M>(world, id)
            .ok()
            .map(|s| {
                s.baseline
                    .iter()
                    .chain(std::iter::once(s.history.content()))
                    .chain(s.conflict.iter().map(|c| &c.incoming))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        match world.resource_mut::<ManagedWork>().spawn_task(
            Some(id.0),
            "Read document source",
            move |_| {
                if !initial && !force {
                    match source.observed_version()? {
                        None => {}
                        Some(version) if Some(&version) == expected.as_ref() => return Ok(None),
                        _ => {}
                    }
                }
                source.load().map(|mut loaded| {
                    if let Some(equal) = known.iter().find(|known| **known == loaded.model) {
                        loaded.model = equal.clone();
                    }
                    Some(loaded)
                })
            },
        ) {
            Ok(task) => {
                self.task = Some(task);
                world.get_mut::<Document>(id.0).unwrap().source_pending = true;
            }
            Err(error) => {
                if let Some(mut document) = world.get_mut::<Document>(id.0) {
                    if document.load_state == DocumentLoadState::Loading {
                        document.load_state = DocumentLoadState::Failed(error);
                    }
                }
            }
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
pub(super) fn attach_memory<M: DocumentModel>(
    world: &mut World,
    entity: Entity,
    source: MemorySource<M>,
) {
    world.entity_mut(entity).insert(DocumentSourceRuntime {
        runtime: Box::new(Runtime {
            source: Arc::new(source),
            task: None,
            generation: 0,
            location_generation: 0,
            next_check: Instant::now() + Duration::from_secs(86400),
            refresh: false,
            missing: false,
        }),
    });
}

pub fn document_write_blocked(world: &World, id: DocumentId) -> bool {
    world
        .get::<DocumentSourceRuntime>(id.0)
        .is_some_and(|r| r.runtime.write_blocked())
}

pub fn document_source_location(world: &World, id: DocumentId) -> Option<std::path::PathBuf> {
    world.get::<DocumentSourceRuntime>(id.0)?.runtime.location()
}
pub fn invalidate_document_source(world: &mut World, id: DocumentId) {
    if let Some(mut runtime) = world.get_mut::<DocumentSourceRuntime>(id.0) {
        runtime.runtime.invalidate();
    }
}
