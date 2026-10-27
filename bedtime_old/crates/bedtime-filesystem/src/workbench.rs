//! Standard file opening and source mutation coordination for Workbench.
use crate::{Filesystem, FsDocumentSource};
use bedtime_document::{BinaryDocument, DocumentSource, SourceError, SourceLoad, TextDocument};
use bedtime_workbench::*;
use bevy::prelude::*;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FilesystemDocumentSystems {
    Update,
}
pub struct FilesystemDocumentsPlugin;
impl Plugin for FilesystemDocumentsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FileWrites>().add_systems(
            Update,
            (
                poll_opens,
                poll_ready,
                poll_refreshes,
                crate::source_watch::update,
                project_panels,
            )
                .chain()
                .in_set(FilesystemDocumentSystems::Update),
        );
    }
}
pub enum FileDocumentLocation {
    Available(PathBuf),
    Missing { retained: bool },
}
#[derive(Component)]
pub struct FileDocumentPanel {
    callback: fn(&mut World, Entity, FileDocumentLocation),
    last_path: Option<PathBuf>,
    missing: bool,
}
impl FileDocumentPanel {
    pub fn new(callback: fn(&mut World, Entity, FileDocumentLocation)) -> Self {
        Self {
            callback,
            last_path: None,
            missing: false,
        }
    }
}
fn project_panels(world: &mut World) {
    let panels: Vec<_> = world
        .query_filtered::<(Entity, &DocumentReference), With<FileDocumentPanel>>()
        .iter(world)
        .map(|(p, v)| (p, v.document()))
        .collect();
    for (panel, id) in panels {
        let Ok(info) = inspect_document(world, id) else {
            continue;
        };
        let path = document_source_path(world, id);
        let mut state = world.get_mut::<FileDocumentPanel>(panel).unwrap();
        let callback = state.callback;
        if info.source_missing && !state.missing {
            state.missing = true;
            state.last_path = None;
            let retained = info.dirty || info.saving || info.conflict || info.has_drafts;
            if let Some(mut metadata) = world.get_mut::<PanelInstance>(panel) {
                metadata.preview = false;
            }
            if !retained {
                world.entity_mut(panel).remove::<DocumentReference>();
            }
            callback(world, panel, FileDocumentLocation::Missing { retained });
            if !retained
                && world
                    .get::<bedtime_workbench::InArea>(panel)
                    .is_some_and(|a| bedtime_workbench::panels_in_area(world, a.0).len() > 1)
            {
                let _ = Workbench::new(world).close_panel(panel);
            }
        } else if !info.source_missing && (state.missing || state.last_path != path) {
            state.missing = false;
            state.last_path = path.clone();
            if let Some(path) = path {
                if let Some(mut metadata) = world.get_mut::<PanelInstance>(panel) {
                    metadata.title = path
                        .file_name()
                        .unwrap_or(path.as_os_str())
                        .to_string_lossy()
                        .into_owned();
                }
                callback(world, panel, FileDocumentLocation::Available(path));
            }
        }
    }
}
#[derive(Resource, Clone)]
pub struct FileWrites(pub crate::WriteAccess);
impl Default for FileWrites {
    fn default() -> Self {
        Self(crate::WriteAccess::local())
    }
}
impl std::ops::Deref for FileWrites {
    type Target = crate::WriteAccess;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
pub use crate::WriteLease;

enum Prepared {
    Text(FsDocumentSource, SourceLoad<TextDocument>),
    Binary(FsDocumentSource, SourceLoad<BinaryDocument>),
}
type OpenCallback = fn(&mut World, Entity, Result<DocumentId, String>);
#[derive(Component)]
struct FileOpen {
    task: ManagedTask<Result<Prepared, String>>,
    callback: OpenCallback,
}
impl Drop for FileOpen {
    fn drop(&mut self) {
        self.task.cancel();
    }
}

/// Retire viewer-owned opening work before replacing the viewer's descriptor.
pub fn cancel_file_open(world: &mut World, owner: Entity) {
    world
        .entity_mut(owner)
        .remove::<(FileOpen, FileRefresh, FileReady)>();
}

pub fn file_open_pending(world: &World, owner: Entity) -> bool {
    world.get::<FileOpen>(owner).is_some()
        || world.get::<FileReady>(owner).is_some()
        || world.get::<FileRefresh>(owner).is_some()
}

/// Main-world admission. Canonicalization, classification and loading run on work.
pub fn open_file_document(
    world: &mut World,
    owner: Entity,
    path: PathBuf,
    callback: OpenCallback,
) -> Result<(), String> {
    cancel_file_open(world, owner);
    let existing: Vec<_> = document_ids(world)
        .into_iter()
        .filter_map(|id| Some((document_source_path(world, id)?, id)))
        .collect();
    // Existing paths can be reused without touching disk; aliases resolve on work.
    if let Some((_, id)) = existing.iter().find(|(p, _)| p == &path) {
        complete_open(world, owner, *id, callback);
        return Ok(());
    }
    let task = world.resource_mut::<ManagedWork>().spawn_task(
        Some(owner),
        "Open filesystem document",
        move |cancel| {
            if cancel.is_cancelled() {
                return Err("File opening cancelled".into());
            }
            let source = Filesystem::local()
                .document_source(&path)
                .map_err(|e| e.to_string())?;
            prepare_file(source)
        },
    )?;
    world.entity_mut(owner).insert(FileOpen { task, callback });
    Ok(())
}
fn prepare_file(source: FsDocumentSource) -> Result<Prepared, String> {
    let bytes = source.bytes();
    let metadata = bytes.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file() {
        return Err("Source is not a regular file".into());
    }
    if metadata.len <= 256 * 1024 * 1024 {
        let probe = bytes.read_prefix(8192).map_err(|e| e.to_string())?.bytes;
        if !probe.contains(&0)
            && !std::str::from_utf8(&probe).is_err_and(|e| e.error_len().is_some())
        {
            match <FsDocumentSource as DocumentSource<TextDocument>>::load(&source) {
                Ok(loaded) => return Ok(Prepared::Text(source, loaded)),
                Err(SourceError::Unsupported(_)) => {}
                Err(error) => return Err(error.to_string()),
            }
        }
    }
    let loaded = <FsDocumentSource as DocumentSource<BinaryDocument>>::load(&source)
        .map_err(|e| e.to_string())?;
    Ok(Prepared::Binary(source, loaded))
}
/// Preserve a source already resolved by a producer such as Filetree.
pub fn open_resolved_document(
    world: &mut World,
    owner: Entity,
    source: FsDocumentSource,
    callback: OpenCallback,
) -> Result<(), String> {
    cancel_file_open(world, owner);
    if let Some(id) = document_for_source(world, source.id()) {
        complete_open(world, owner, id, callback);
        return Ok(());
    }
    let task = world.resource_mut::<ManagedWork>().spawn_task(
        Some(owner),
        "Load resolved file source",
        move |_| prepare_file(source),
    )?;
    world.entity_mut(owner).insert(FileOpen { task, callback });
    Ok(())
}

fn poll_opens(world: &mut World) {
    let owners: Vec<_> = world
        .query_filtered::<Entity, With<FileOpen>>()
        .iter(world)
        .collect();
    for owner in owners {
        let open = world.get::<FileOpen>(owner).unwrap();
        let result = match open.task.try_recv() {
            Ok(r) => Some(r),
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(_) => Some(Err("File opening worker stopped".into())),
        };
        if let Some(result) = result {
            let open = world.entity_mut(owner).take::<FileOpen>().unwrap();
            let result = result.and_then(|prepared| match prepared {
                Prepared::Text(source, loaded) => {
                    if source.is_deleted() {
                        Err("File no longer exists".into())
                    } else if let Some(id) = document_for_source(world, source.id()) {
                        Ok(id)
                    } else {
                        open_prepared_document(world, source, loaded)
                            .map(|h| h.id())
                            .map_err(|e| e.to_string())
                    }
                }
                Prepared::Binary(source, loaded) => {
                    if source.is_deleted() {
                        Err("File no longer exists".into())
                    } else if let Some(id) = document_for_source(world, source.id()) {
                        Ok(id)
                    } else {
                        open_prepared_document(world, source, loaded)
                            .map(|h| h.id())
                            .map_err(|e| e.to_string())
                    }
                }
            });
            match result {
                Ok(id) => complete_open(world, owner, id, open.callback),
                Err(e) => (open.callback)(world, owner, Err(e)),
            }
        }
    }
}

pub fn source_path(world: &World, id: DocumentId) -> Option<PathBuf> {
    document_source_path(world, id)
}
pub struct SourceCapture {
    known: Vec<TextDocument>,
    id: DocumentId,
    path: PathBuf,
    generation: u64,
    source: Arc<dyn DocumentSource<TextDocument>>,
}
pub struct SourceObservation {
    capture: SourceCapture,
    incoming: Result<SourceLoad<TextDocument>, SourceError>,
}
pub fn capture_sources(world: &mut World, root: &Path) -> Vec<SourceCapture> {
    document_ids(world)
        .into_iter()
        .filter_map(|id| {
            let path = source_path(world, id)?;
            if !path.starts_with(root) {
                return None;
            }
            let handle = cast_document::<TextDocument>(world, id).ok()?;
            let known = document_checkpoint(world, handle)
                .ok()?
                .into_iter()
                .chain(std::iter::once(read_document(world, handle).ok()?.clone()))
                .collect();
            Some(SourceCapture {
                known,
                id,
                path,
                generation: inspect_document(world, id).ok()?.source_generation,
                source: source_document(world, id)?,
            })
        })
        .collect()
}
pub fn read_source_observation(capture: SourceCapture) -> SourceObservation {
    let incoming = capture.source.load().map(|mut loaded| {
        if let Some(known) = capture.known.iter().find(|known| **known == loaded.model) {
            loaded.model = known.clone();
        }
        loaded
    });
    SourceObservation { capture, incoming }
}
pub fn apply_source_observation(
    world: &mut World,
    observation: SourceObservation,
) -> Result<(), String> {
    let SourceObservation { capture, incoming } = observation;
    if source_path(world, capture.id).as_ref() != Some(&capture.path)
        || !inspect_document(world, capture.id)
            .is_ok_and(|i| i.source_generation == capture.generation)
    {
        return Ok(());
    }
    match incoming {
        Ok(loaded) => {
            let h = cast_document::<TextDocument>(world, capture.id).map_err(|e| e.to_string())?;
            offer_document_external_update(
                world,
                h,
                capture.generation,
                loaded.model,
                loaded.version,
            )
            .map_err(|e| e.to_string())?;
            Ok(())
        }
        Err(SourceError::Missing) => refresh_document(world, capture.id).map_err(|e| e.to_string()),
        Err(e) => Err(e.to_string()),
    }
}

/// Captures source knowledge and write admission before an external operation.
pub struct SourceChange {
    captures: Vec<SourceCapture>,
    lease: Option<WriteLease>,
}
pub struct SourceChangeResult<T> {
    output: T,
    observations: Vec<SourceObservation>,
    _lease: Option<WriteLease>,
}
/// Receives presentation output after source reconciliation. Dropping this handle
/// does not cancel or discard the admitted operation's source effects.
pub struct SourceChangeTask<T> {
    receiver: std::sync::Mutex<std::sync::mpsc::Receiver<Result<(T, Vec<String>), String>>>,
}
impl<T> SourceChangeTask<T> {
    pub fn try_recv(
        &self,
    ) -> Result<Result<(T, Vec<String>), String>, std::sync::mpsc::TryRecvError> {
        self.receiver.lock().unwrap().try_recv()
    }
}
pub fn prepare_source_change(
    world: &mut World,
    write_scope: Option<&Path>,
    observe_scope: Option<&Path>,
) -> Result<SourceChange, String> {
    let lease = write_scope
        .map(|root| crate::WriteAccess::local().repository(root))
        .transpose()?;
    let captures = observe_scope
        .map(|root| capture_sources(world, root))
        .unwrap_or_default();
    Ok(SourceChange { captures, lease })
}
impl SourceChange {
    /// Admit an operation whose source effects must survive cancellation and
    /// observer closure. Reconciliation runs before presentation output is sent.
    pub fn spawn<T: Send + 'static>(
        self,
        world: &mut World,
        label: impl Into<String>,
        operation: impl FnOnce(Cancellation) -> T + Send + 'static,
    ) -> Result<SourceChangeTask<T>, String> {
        let uncertain: Vec<_> = self.captures.iter().map(|capture| capture.id).collect();
        let (sender, receiver) = std::sync::mpsc::channel();
        let label = label.into();
        let completion = world.resource_mut::<ManagedWork>().admit_operation(
            None,
            label.clone(),
            move |world, outcome: OperationOutcome<SourceChangeResult<T>>| {
                let result = match outcome.result {
                    Ok(result) => Ok(result.reconcile(world)),
                    Err(error) => {
                        for id in uncertain {
                            invalidate_document_source(world, id);
                        }
                        Err(error)
                    }
                };
                let _ = sender.send(result);
            },
        )?;
        let cancel = completion.cancellation();
        world
            .resource_mut::<ManagedWork>()
            .spawn_task(None, label, move |_| {
                completion.complete(self.execute(|| operation(cancel)));
            })?;
        Ok(SourceChangeTask {
            receiver: std::sync::Mutex::new(receiver),
        })
    }

    /// Worker computation for read-only comparisons. For mutations use `spawn`
    /// so cancellation cannot suppress source reconciliation.
    pub fn execute<T>(self, operation: impl FnOnce() -> T) -> SourceChangeResult<T> {
        let output = operation();
        let observations = self
            .captures
            .into_iter()
            .map(read_source_observation)
            .collect();
        SourceChangeResult {
            output,
            observations,
            _lease: self.lease,
        }
    }
}
impl<T> SourceChangeResult<T> {
    /// Keep write admission until all captured source outcomes have been accepted.
    pub fn reconcile(self, world: &mut World) -> (T, Vec<String>) {
        let mut errors = Vec::new();
        for observation in self.observations {
            if let Err(e) = apply_source_observation(world, observation) {
                errors.push(e);
            }
        }
        (self.output, errors)
    }
}

#[derive(Component)]
struct FileRefresh {
    document: DocumentId,
    callback: OpenCallback,
}
pub fn refresh_file_document(
    world: &mut World,
    owner: Entity,
    document: DocumentId,
    callback: OpenCallback,
) -> Result<(), String> {
    refresh_document(world, document).map_err(|e| e.to_string())?;
    world
        .entity_mut(owner)
        .insert(FileRefresh { document, callback });
    Ok(())
}
fn poll_refreshes(world: &mut World) {
    let owners: Vec<_> = world
        .query_filtered::<Entity, With<FileRefresh>>()
        .iter(world)
        .collect();
    for owner in owners {
        let r = world.get::<FileRefresh>(owner).unwrap();
        let info = inspect_document(world, r.document);
        if info.as_ref().is_ok_and(|i| i.source_pending) {
            continue;
        }
        let r = world.entity_mut(owner).take::<FileRefresh>().unwrap();
        let result = info.map(|_| r.document).map_err(|e| e.to_string());
        (r.callback)(world, owner, result);
    }
}

#[derive(Component)]
struct FileReady {
    document: DocumentId,
    callback: OpenCallback,
}
fn complete_open(world: &mut World, owner: Entity, document: DocumentId, callback: OpenCallback) {
    match inspect_document(world, document).map(|i| i.load_state) {
        Ok(DocumentLoadState::Loading) => {
            world
                .entity_mut(owner)
                .insert(FileReady { document, callback });
        }
        Ok(DocumentLoadState::Ready) => callback(world, owner, Ok(document)),
        Ok(DocumentLoadState::Failed(error)) => callback(world, owner, Err(error)),
        Err(e) => callback(world, owner, Err(e.to_string())),
    }
}
fn poll_ready(world: &mut World) {
    let owners: Vec<_> = world
        .query_filtered::<Entity, With<FileReady>>()
        .iter(world)
        .collect();
    for owner in owners {
        let r = world.get::<FileReady>(owner).unwrap();
        if inspect_document(world, r.document)
            .is_ok_and(|i| i.load_state == DocumentLoadState::Loading)
        {
            continue;
        }
        let r = world.entity_mut(owner).take::<FileReady>().unwrap();
        complete_open(world, owner, r.document, r.callback);
    }
}

pub fn source_watch_error(world: &World) -> Option<String> {
    crate::source_watch::error(world)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::mpsc,
        time::{Duration, Instant},
    };

    #[test]
    fn source_mutation_reconciles_after_shutdown_before_releasing_write_access() {
        let root = std::env::temp_dir().join(format!("source-mutation-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let root = std::fs::canonicalize(root).unwrap();
        let path = root.join("document.txt");
        std::fs::write(&path, "before").unwrap();
        let mut app = App::new();
        app.add_plugins(WorkbenchCorePlugin);
        let source = Filesystem::local().document_source(&path).unwrap();
        let handle = open_document::<TextDocument>(app.world_mut(), source).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while inspect_document(app.world(), handle.id())
            .unwrap()
            .load_state
            != DocumentLoadState::Ready
        {
            assert!(Instant::now() < deadline);
            app.update();
            std::thread::yield_now();
        }
        let change = prepare_source_change(app.world_mut(), Some(&root), Some(&root)).unwrap();
        let (committed, effect) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        let task = change
            .spawn(app.world_mut(), "Mutation", move |_| {
                std::fs::write(path, "after").unwrap();
                committed.send(()).unwrap();
                wait.recv().unwrap();
                42
            })
            .unwrap();
        effect.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(crate::WriteAccess::local().repository(&root).is_err());
        app.world_mut().resource_mut::<ManagedWork>().shutdown();
        release.send(()).unwrap();
        while !work_finished(app.world()) {
            assert!(Instant::now() < deadline);
            app.update();
            std::thread::yield_now();
        }
        let (output, errors) = task.try_recv().unwrap().unwrap();
        assert_eq!(output, 42);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(
            read_document(app.world(), handle).unwrap(),
            &TextDocument::from("after")
        );
        assert!(!inspect_document(app.world(), handle.id()).unwrap().dirty);
        assert!(crate::WriteAccess::local().repository(&root).is_ok());
        std::fs::remove_dir_all(root).unwrap();
    }
}
