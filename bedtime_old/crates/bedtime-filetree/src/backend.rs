//! One world-owned filesystem worker. The world only exchanges bounded
//! intentions and Arc snapshots; filesystem access, sorting and projection live here.
use bedtime_filesystem::{FileSource as ByteSource, watch::DirectoryWatcher as NativeWatcher};
use bedtime_workbench::{
    Cancellation, ManagedTask, ManagedWork, OperationCompletion, OperationOutcome, WorkStatus,
};
use bevy::prelude::Entity;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, VecDeque},
    ffi::OsString,
    io,
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const MAX_VIEWS: usize = 64;
const MAX_DIRECTORIES: usize = 512;
// Includes queued/running operations AND undrained outcomes. Committed results
// therefore always have a reserved slot, even during session cancellation.
const MAX_OPERATIONS: usize = 16;
// Mailbox + panel ownership can retire the same version independently. Four
// slots per live view leave bounded headroom while a baseline/operation blocks.
const MAX_RETIRED_SNAPSHOTS: usize = MAX_VIEWS * 4;
const WORKER_TICK: Duration = Duration::from_millis(20);
const FALLBACK_INTERVAL: Duration = Duration::from_secs(2);
const POLL_BATCH: usize = 16;

pub use bedtime_filesystem::EntryKind as FileTreeEntryKind;
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileTreeDirectoryState {
    Unloaded,
    Loading,
    Ready,
    Failed(Arc<str>),
}

#[derive(Clone, Debug)]
pub struct FileTreeRow {
    pub path: PathBuf,
    pub name: OsString,
    pub label: Arc<str>,
    pub kind: FileTreeEntryKind,
    pub depth: usize,
    pub hidden: bool,
    pub directory_state: FileTreeDirectoryState,
}

#[derive(Clone, Debug, Default)]
pub struct FileTreeSnapshot {
    pub root: PathBuf,
    pub generation: u64,
    pub revision: u64,
    pub version: u64,
    pub rows: Arc<[FileTreeRow]>,
    pub loading: bool,
    pub error: Option<String>,
    /// Visible non-root cached directory failures, including off-viewport rows.
    /// Expansion-limit failures are summarized once in `error`, not repeated
    /// for every unadmitted path; this list stays bounded by the directory cache.
    pub directory_errors: Arc<[(PathBuf, Arc<str>)]>,
    pub watcher_error: Option<String>,
    pub cached_directories: usize,
    // Shared even if a caller clones the snapshot value instead of its Arc.
    // Constructed alongside the rows on the worker, never by a UI getter.
    row_indices: Arc<HashMap<PathBuf, (usize, usize)>>,
}

impl FileTreeSnapshot {
    pub fn row_index(&self, path: &Path) -> Option<usize> {
        self.row_indices.get(path).map(|&(index, _)| index)
    }

    /// Exclusive end of this visible row and its projected descendants. A
    /// collapsed directory or leaf spans one row; paths absent from the visible
    /// snapshot return None. Both endpoints are indexed on the worker, so a UI
    /// can address a whole folder branch without scanning following rows.
    pub fn subtree_end_index(&self, path: &Path) -> Option<usize> {
        self.row_indices.get(path).map(|&(_, end)| end)
    }
}

#[derive(Clone)]
pub(crate) struct ViewRequest {
    pub panel: Entity,
    pub generation: u64,
    pub revision: u64,
    pub root: PathBuf,
    pub root_expanded: bool,
    pub expanded: Arc<BTreeSet<PathBuf>>,
    pub visibility: crate::FileTreeVisibility,
    pub show_hidden: bool,
}

struct MailView {
    request: Arc<ViewRequest>,
    serial: u64,
    refresh: u64,
    snapshot: Option<Arc<FileTreeSnapshot>>,
}

struct PendingOperation {
    request: crate::FileTreeOperationRequest,
    completion: OperationCompletion<crate::FileTreeOperationResult>,
}

#[derive(Default)]
struct Mailbox {
    closed: bool,
    retirement_closed: bool,
    retired_snapshots: VecDeque<Arc<FileTreeSnapshot>>,
    views: BTreeMap<Entity, MailView>,
    next_serial: u64,
    operations: VecDeque<PendingOperation>,
    operation_cancellations: BTreeMap<u64, Cancellation>,
    progress: Option<(u64, crate::FileTreeOperationProgress)>,
}

impl Mailbox {
    fn retire_snapshot(
        &mut self,
        snapshot: Arc<FileTreeSnapshot>,
    ) -> Result<(), Arc<FileTreeSnapshot>> {
        if self.retirement_closed {
            return Err(snapshot);
        }
        // Another owner can transfer its Arc of the same immutable version.
        // Dropping that extra Arc is cheap: the queued owner keeps the rows alive.
        if self
            .retired_snapshots
            .iter()
            .any(|queued| Arc::ptr_eq(queued, &snapshot))
        {
            return Ok(());
        }
        if self.retired_snapshots.len() == MAX_RETIRED_SNAPSHOTS {
            return Err(snapshot);
        }
        self.retired_snapshots.push_back(snapshot);
        Ok(())
    }

    fn retire_view_snapshot(&mut self, view: &mut MailView) {
        if let Some(snapshot) = view.snapshot.take() {
            // The caller drops the returned view OUTSIDE the lock. Only queue
            // saturation or an already-finished worker takes this fallback path.
            view.snapshot = self.retire_snapshot(snapshot).err();
        }
    }
}

/// Main-world owners move replaced snapshots here AFTER mem::replace/take.
/// Accepted transfers are destroyed by the existing filesystem worker, without
/// holding its mailbox lock. Capacity is bounded; stopped/full returns ownership
/// so the host can deliberately choose a synchronous teardown fallback. External
/// Arc clones retain their ordinary reference-counted destruction semantics.
#[derive(Clone)]
pub(crate) struct SnapshotRetirementHandle {
    mailbox: Arc<Mutex<Mailbox>>,
}

impl SnapshotRetirementHandle {
    pub(crate) fn retire_snapshot(
        &self,
        snapshot: Arc<FileTreeSnapshot>,
    ) -> Result<(), Arc<FileTreeSnapshot>> {
        self.mailbox.lock().unwrap().retire_snapshot(snapshot)
    }
}

fn drain_retired_snapshots(mailbox: &Mutex<Mailbox>) {
    let retired = std::mem::take(&mut mailbox.lock().unwrap().retired_snapshots);
    drop(retired); // worker only; no row/path destruction while locked
}

// A worker panic must abandon queued completion tokens as well as its active
// operation. Drop outside the mailbox lock; managed work reports unknown effects.
struct CloseOperations(Arc<Mutex<Mailbox>>);
impl Drop for CloseOperations {
    fn drop(&mut self) {
        let pending = {
            let mut mail = self.0.lock().unwrap();
            mail.closed = true;
            std::mem::take(&mut mail.operations)
        };
        drop(pending);
    }
}

pub(crate) struct Backend {
    mailbox: Arc<Mutex<Mailbox>>,
    task: ManagedTask<()>,
}

impl Backend {
    pub(crate) fn spawn(scope: &mut ManagedWork) -> Result<Self, String> {
        let mailbox = Arc::new(Mutex::new(Mailbox::default()));
        let worker_mailbox = mailbox.clone();
        let task = scope.spawn_task(None, "File tree filesystem", move |cancel| {
            let _close = CloseOperations(worker_mailbox.clone());
            Worker::new(worker_mailbox).run(cancel);
        })?;
        Ok(Self { mailbox, task })
    }

    pub(crate) fn set_view(&mut self, request: ViewRequest) -> Result<(), String> {
        if self.status() != WorkStatus::Running {
            return Err("File tree worker is not running".into());
        }
        let request = Arc::new(request);
        let retired = {
            let mut mail = self.mailbox.lock().unwrap();
            if mail.closed {
                return Err("File tree worker is stopping".into());
            }
            if let Some(old) = mail.views.get(&request.panel) {
                if (request.generation, request.revision)
                    < (old.request.generation, old.request.revision)
                {
                    return Ok(());
                }
            } else if mail.views.len() == MAX_VIEWS {
                return Err(format!(
                    "At most {MAX_VIEWS} file tree views may share a session"
                ));
            }
            let refresh = mail
                .views
                .get(&request.panel)
                .map_or(0, |view| view.refresh);
            mail.next_serial = mail
                .next_serial
                .checked_add(1)
                .expect("View serial exhausted");
            let serial = mail.next_serial;
            // Replacing a view retires (possibly very large) snapshots outside
            // the lock. A stale worker projection cannot install over this serial.
            let mut retired = mail.views.insert(
                request.panel,
                MailView {
                    request,
                    serial,
                    refresh,
                    snapshot: None,
                },
            );
            if let Some(view) = retired.as_mut() {
                mail.retire_view_snapshot(view);
            }
            retired
        };
        drop(retired);
        Ok(())
    }

    pub(crate) fn remove_view(&mut self, panel: Entity) {
        let retired = {
            let mut mail = self.mailbox.lock().unwrap();
            let mut retired = mail.views.remove(&panel);
            if let Some(view) = retired.as_mut() {
                mail.retire_view_snapshot(view);
            }
            retired
        };
        drop(retired);
    }

    pub(crate) fn refresh(&mut self, panel: Entity) {
        let mut mail = self.mailbox.lock().unwrap();
        if let Some(view) = mail.views.get_mut(&panel) {
            view.refresh = view.refresh.wrapping_add(1);
        }
    }

    pub(crate) fn snapshot(&self, panel: Entity) -> Option<Arc<FileTreeSnapshot>> {
        self.mailbox
            .lock()
            .unwrap()
            .views
            .get(&panel)?
            .snapshot
            .clone()
    }

    pub(crate) fn submit_operation(
        &mut self,
        scope: &mut ManagedWork,
        request: crate::FileTreeOperationRequest,
        complete: impl FnOnce(
            &mut bevy::prelude::World,
            OperationOutcome<crate::FileTreeOperationResult>,
        ) + Send
        + 'static,
    ) -> Result<(), String> {
        if self.status() != WorkStatus::Running {
            return Err("File tree worker is not running".into());
        }
        let mut mail = self.mailbox.lock().unwrap();
        if mail.closed {
            return Err("File tree worker is stopping".into());
        }
        if mail.operation_cancellations.contains_key(&request.id) {
            return Err("File operation ID is already outstanding".into());
        }
        if mail.operation_cancellations.len() == MAX_OPERATIONS {
            return Err(format!(
                "File operation queue is full ({MAX_OPERATIONS} outstanding); drain results first"
            ));
        }
        let id = request.id;
        let mailbox = self.mailbox.clone();
        let completion =
            scope.admit_operation(None, "File tree mutation", move |world, outcome| {
                // Undrained outcomes retain their admission slots until this callback.
                mailbox.lock().unwrap().operation_cancellations.remove(&id);
                complete(world, outcome);
            })?;
        mail.operation_cancellations
            .insert(id, completion.cancellation());
        mail.operations.push_back(PendingOperation {
            request,
            completion,
        });
        Ok(())
    }

    pub(crate) fn cancel_operation(&mut self, id: u64) {
        if let Some(cancel) = self
            .mailbox
            .lock()
            .unwrap()
            .operation_cancellations
            .get(&id)
        {
            cancel.cancel();
        }
    }

    pub(crate) fn snapshot_retirement_handle(&self) -> SnapshotRetirementHandle {
        SnapshotRetirementHandle {
            mailbox: self.mailbox.clone(),
        }
    }

    #[cfg(test)]
    pub(crate) fn retire_snapshot(
        &self,
        snapshot: Arc<FileTreeSnapshot>,
    ) -> Result<(), Arc<FileTreeSnapshot>> {
        self.mailbox.lock().unwrap().retire_snapshot(snapshot)
    }

    pub(crate) fn progress(&self) -> Option<(u64, crate::FileTreeOperationProgress)> {
        self.mailbox.lock().unwrap().progress.clone()
    }

    pub(crate) fn status(&self) -> WorkStatus {
        self.task.status()
    }

    pub(crate) fn shutdown(&mut self) {
        let retired_views = {
            let mut mail = self.mailbox.lock().unwrap();
            mail.closed = true;
            for cancelled in mail.operation_cancellations.values() {
                cancelled.cancel();
            }
            let mut views = std::mem::take(&mut mail.views);
            for view in views.values_mut() {
                mail.retire_view_snapshot(view);
            }
            views
        };
        self.task.cancel();
        // Ordinarily snapshot slots moved into the worker lane above. Failed
        // transfers are a bounded synchronous teardown fallback, never locked.
        drop(retired_views);
    }
}

impl Drop for Backend {
    fn drop(&mut self) {
        self.shutdown();
    }
}

struct CachedEntry {
    path: PathBuf,
    name: OsString,
    label: Arc<str>,
    sort_label: String,
    kind: FileTreeEntryKind,
    hidden: bool,
}

struct CachedDirectory {
    state: FileTreeDirectoryState,
    entries: Vec<CachedEntry>,
    failure_kind: Option<io::ErrorKind>,
    used: u64,
}

impl CachedDirectory {
    fn unloaded(used: u64) -> Self {
        Self {
            state: FileTreeDirectoryState::Unloaded,
            entries: Vec::new(),
            failure_kind: None,
            used,
        }
    }

    fn invalidate(&mut self, retry_failed: bool) -> bool {
        if matches!(self.state, FileTreeDirectoryState::Failed(_)) && !retry_failed {
            return false;
        }
        self.state = FileTreeDirectoryState::Unloaded;
        self.failure_kind = None;
        // Invalidations are hints, not evidence that rows disappeared. Keep the
        // last baseline visible during reload; only a completed read replaces
        // it (or clears it on failure). This also preserves stable UI row keys.
        true
    }
}

struct WorkerView {
    request: Arc<ViewRequest>,
    serial: u64,
    refresh: u64,
    input_absolute: PathBuf,
    root: PathBuf,
    error: Option<String>,
    retry_missing: bool,
}

struct WatchState {
    watcher: Option<NativeWatcher>,
    watched: BTreeSet<PathBuf>,
    failures: BTreeMap<PathBuf, String>,
    backend_error: Option<String>,
    last_fallback: Instant,
    poll_cursor: usize,
}

impl WatchState {
    fn new() -> Self {
        let (watcher, backend_error) = match NativeWatcher::new() {
            Ok(watcher) => (Some(watcher), None),
            Err(error) => (
                None,
                Some(format!(
                    "Native file watching unavailable: {error}; polling loaded directories"
                )),
            ),
        };
        Self {
            watcher,
            watched: BTreeSet::new(),
            failures: BTreeMap::new(),
            backend_error,
            last_fallback: Instant::now(),
            poll_cursor: 0,
        }
    }

    fn ensure(&mut self, path: &Path) {
        if self.watched.contains(path) || self.failures.contains_key(path) {
            return;
        }
        let Some(watcher) = self.watcher.as_mut() else {
            return;
        };
        // Nonrecursive is deliberate. macOS uses our directory-only kqueue
        // registration, not notify's self-expanding kqueue/FSEvents adapters.
        // Only cache demand ever causes directory enumeration.
        let result = watcher.watch(path);
        match result {
            Ok(()) => {
                self.watched.insert(path.to_owned());
            }
            Err(error) => {
                self.failures.insert(
                    path.to_owned(),
                    format!(
                        "Cannot watch {}: {error}; polling loaded directories",
                        path.display()
                    ),
                );
            }
        }
    }

    fn reconcile(&mut self, wanted: &BTreeSet<PathBuf>) {
        let obsolete: Vec<_> = self.watched.difference(wanted).cloned().collect();
        for path in obsolete {
            self.forget(&path);
        }
        self.failures.retain(|path, _| wanted.contains(path));
        for path in wanted {
            self.ensure(path);
        }
    }

    fn forget(&mut self, path: &Path) {
        self.failures.remove(path);
        if !self.watched.remove(path) {
            return;
        }
        let error = self
            .watcher
            .as_mut()
            .and_then(|watcher| watcher.unwatch(path).err());
        let error = error
            .filter(|error| error.kind() != io::ErrorKind::NotFound)
            .map(|error| error.to_string());
        if let Some(error) = error {
            self.backend_error = Some(format!(
                "Cannot release native file watch: {error}; polling loaded directories"
            ));
            // Releasing the native owner bounds handles even if unwatch
            // failed. Missing/deleted registrations are already released.
            self.watcher = None;
            self.watched.clear();
        }
    }

    fn forget_subtree(&mut self, path: &Path) {
        if path.as_os_str().is_empty() {
            return;
        }
        let stale: Vec<_> = self
            .watched
            .range::<Path, _>((std::ops::Bound::Included(path), std::ops::Bound::Unbounded))
            .take_while(|watched| watched.starts_with(path))
            .cloned()
            .collect();
        for stale in stale {
            self.forget(&stale);
        }
    }

    fn error(&self) -> Option<String> {
        self.backend_error.clone().or_else(|| {
            self.failures.values().next().map(|message| {
                if self.failures.len() > 1 {
                    format!("{message} ({} watch failures)", self.failures.len())
                } else {
                    message.clone()
                }
            })
        })
    }
}

struct Worker {
    mailbox: Arc<Mutex<Mailbox>>,
    views: BTreeMap<Entity, WorkerView>,
    cache: BTreeMap<PathBuf, CachedDirectory>,
    watches: WatchState,
    version: u64,
}

impl Worker {
    fn new(mailbox: Arc<Mutex<Mailbox>>) -> Self {
        Self {
            mailbox,
            views: BTreeMap::new(),
            cache: BTreeMap::new(),
            watches: WatchState::new(),
            version: 0,
        }
    }

    fn run(mut self, cancel: Cancellation) {
        let mut changed = true;
        loop {
            drain_retired_snapshots(&self.mailbox);
            // There is one mutation execution point. Even when cancellation has
            // been requested, queued requests receive an actual cancelled result.
            // The operation executor finishes/report committed critical steps;
            // managed completion tokens retain outcomes BEFORE this task returns.
            let operation = self.mailbox.lock().unwrap().operations.pop_front();
            if let Some(operation) = operation {
                let id = operation.request.id;
                let operation_cancel = operation.completion.cancellation();
                let result = crate::operations::execute(
                    operation.request,
                    || cancel.is_cancelled() || operation_cancel.is_cancelled(),
                    |progress| {
                        let retired = self
                            .mailbox
                            .lock()
                            .unwrap()
                            .progress
                            .replace((id, progress));
                        drop(retired);
                    },
                );
                let affected = affected_cache_paths(&self.cache, &result.completed);
                let retired = {
                    let mut mail = self.mailbox.lock().unwrap();
                    mail.progress.take()
                };
                drop(retired);
                operation.completion.complete(result);
                for path in &affected {
                    self.cache.get_mut(path).unwrap().invalidate(true);
                }
                changed |= !affected.is_empty();
                drain_retired_snapshots(&self.mailbox);
                if cancel.is_cancelled() {
                    continue;
                }
            } else if cancel.is_cancelled() {
                // Seal admission and inspect pending requests under the SAME
                // lock used by submit_operation. A submit racing cancellation
                // must either be rejected or receive an actual cancelled result.
                let final_retirement = {
                    let mut mail = self.mailbox.lock().unwrap();
                    mail.closed = true;
                    if mail.operations.is_empty() {
                        // Seal transfers and take every mailbox snapshot in one
                        // transaction. Later panel drops get explicit ownership
                        // back; they cannot enqueue into a dead retirement lane.
                        mail.retirement_closed = true;
                        Some((
                            std::mem::take(&mut mail.retired_snapshots),
                            std::mem::take(&mut mail.views),
                        ))
                    } else {
                        None
                    }
                };
                if let Some((retired, views)) = final_retirement {
                    drop(retired);
                    drop(views);
                    return;
                }
                continue;
            }

            changed |= self.update_views();
            changed |= self.receive_notifications();
            changed |= self.poll_fallback();
            if changed {
                self.project_and_publish();
                changed = false;
            }

            // At most one baseline per turn: initial root rows are published
            // before loading roots, and before chasing expanded descendants.
            // Cancellation and newer view intentions are checked between reads.
            let next_root = self.views.values().find_map(|view| {
                self.cache
                    .get(&view.root)
                    .filter(|directory| matches!(directory.state, FileTreeDirectoryState::Loading))
                    .map(|_| view.root.clone())
            });
            let next = next_root.or_else(|| {
                self.cache.iter().find_map(|(path, directory)| {
                    matches!(directory.state, FileTreeDirectoryState::Loading).then(|| path.clone())
                })
            });
            if let Some(path) = next {
                self.watches.ensure(&path); // watch BEFORE baseline enumeration
                let result = read_directory(&path, || cancel.is_cancelled());
                if cancel.is_cancelled() {
                    continue;
                }
                let directory = self.cache.get_mut(&path).unwrap();
                match result {
                    Ok(entries) => {
                        directory.entries = entries;
                        directory.failure_kind = None;
                        directory.state = FileTreeDirectoryState::Ready;
                    }
                    Err(error) => {
                        directory.entries.clear();
                        directory.failure_kind = Some(error.kind());
                        directory.state = FileTreeDirectoryState::Failed(Arc::from(format!(
                            "Cannot read {}: {error}",
                            path.display()
                        )));
                    }
                }
                changed = true;
                continue;
            }
            cancel.wait(WORKER_TICK);
        }
    }

    fn update_views(&mut self) -> bool {
        // Arc-only copies, bounded by MAX_VIEWS. No path sets or rows cloned
        // while holding the mailbox lock.
        let requested: Vec<_> = self
            .mailbox
            .lock()
            .unwrap()
            .views
            .values()
            .map(|view| (view.request.clone(), view.serial, view.refresh))
            .collect();
        let panels: BTreeSet<_> = requested
            .iter()
            .map(|(request, _, _)| request.panel)
            .collect();
        let previous_len = self.views.len();
        self.views.retain(|panel, _| panels.contains(panel));
        let mut changed = previous_len != self.views.len();
        for (request, serial, refresh) in requested {
            let existing = self.views.get(&request.panel);
            if existing.is_some_and(|view| view.serial == serial && view.refresh == refresh) {
                continue;
            }
            let same_root = existing.is_some_and(|view| view.request.root == request.root);
            let refreshed = existing.is_some_and(|view| view.refresh != refresh);
            if same_root && !refreshed {
                let view = self.views.get_mut(&request.panel).unwrap();
                view.request = request;
                view.serial = serial;
                view.refresh = refresh;
            } else {
                let mut view = WorkerView {
                    input_absolute: PathBuf::new(),
                    root: PathBuf::new(),
                    error: None,
                    retry_missing: false,
                    request,
                    serial,
                    refresh,
                };
                resolve_root(&mut view, &mut self.watches);
                if refreshed {
                    for (path, directory) in &mut self.cache {
                        if path.starts_with(&view.root) {
                            directory.invalidate(true);
                        }
                    }
                    // Explicit refresh also retries previously failed watches.
                    self.watches.failures.clear();
                }
                self.views.insert(view.request.panel, view);
            }
            changed = true;
        }
        changed
    }

    fn receive_notifications(&mut self) -> bool {
        let mut changed = false;
        let mut ambiguous = false;
        let mut paths = BTreeSet::new();
        let mut replaced = BTreeSet::new();
        if let Some(watcher) = self.watches.watcher.as_mut() {
            // Native intake is bounded and nonblocking. macOS polls directory
            // registrations here; other targets drain the provider's bounded queue.
            match watcher.poll() {
                Ok(batch) => {
                    ambiguous |= batch.rescan;
                    if let Some(error) = batch.error {
                        self.watches.backend_error = Some(format!(
                            "Native file watch error: {error}; polling loaded directories"
                        ));
                        changed = true;
                    }
                    for event in batch.events {
                        let path = lexical_absolute(&event.path).unwrap_or(event.path);
                        if event.lost {
                            replaced.insert(path.clone());
                        }
                        paths.insert(path);
                    }
                }
                Err(error) => {
                    self.watches.backend_error = Some(format!(
                        "Native file watch error: {error}; polling loaded directories"
                    ));
                    // A broken kqueue must not turn each idle tick into a failed
                    // syscall + full rescan. Release it and use modest polling.
                    self.watches.watcher = None;
                    self.watches.watched.clear();
                    ambiguous = true;
                    changed = true;
                }
            }
        }
        if !ambiguous && paths.is_empty() {
            return changed;
        }
        if ambiguous {
            // Overflow may have hidden a remove/recreate at the SAME path.
            // Replacing baselines without replacing old inode registrations
            // would leave a superficially recovered but unwatched directory.
            let watched: Vec<_> = self.watches.watched.iter().cloned().collect();
            for path in watched {
                self.watches.forget(&path);
            }
        }
        // Native handles refer to directory identities, not reusable path
        // strings. A removed/moved directory (and its loaded descendants) must
        // be unregistered before a replacement baseline reinstalls watches.
        for path in replaced {
            self.watches.forget_subtree(&path);
        }
        for (path, directory) in &mut self.cache {
            if ambiguous || paths.iter().any(|event| affects_directory(event, path)) {
                // Failed permissions are stable until explicit refresh. A
                // disappearance may recover when the directory is recreated.
                let retry_failed = directory.failure_kind != Some(io::ErrorKind::PermissionDenied);
                changed |= directory.invalidate(retry_failed);
            }
        }
        for view in self.views.values_mut() {
            let affected = ambiguous
                || paths.iter().any(|path| {
                    path == &view.input_absolute
                        || path == &view.root
                        || view.input_absolute.starts_with(path)
                        || view.root.starts_with(path)
                });
            if affected && (view.error.is_none() || view.retry_missing) {
                resolve_root(view, &mut self.watches);
                changed = true;
            }
        }
        changed
    }

    fn poll_fallback(&mut self) -> bool {
        if self.watches.error().is_none()
            || self.watches.last_fallback.elapsed() < FALLBACK_INTERVAL
        {
            return false;
        }
        self.watches.last_fallback = Instant::now();
        // Retry failed installations modestly, not on every idle turn.
        self.watches.failures.clear();
        let paths: Vec<_> = self.cache.keys().cloned().collect();
        let count = paths.len().min(POLL_BATCH);
        for offset in 0..count {
            let index = (self.watches.poll_cursor + offset) % paths.len();
            let directory = self.cache.get_mut(&paths[index]).unwrap();
            let retry_missing = directory.failure_kind == Some(io::ErrorKind::NotFound);
            directory.invalidate(retry_missing);
        }
        if !paths.is_empty() {
            self.watches.poll_cursor = (self.watches.poll_cursor + count) % paths.len();
        }
        for view in self.views.values_mut().filter(|view| view.retry_missing) {
            resolve_root(view, &mut self.watches);
        }
        true
    }

    fn project_and_publish(&mut self) {
        self.version = self
            .version
            .checked_add(1)
            .expect("Snapshot versions exhausted");
        // Roots have priority across all panels, not just the first large view.
        let mut demanded: BTreeSet<PathBuf> = self
            .views
            .values()
            .filter(|view| view.error.is_none())
            .map(|view| view.root.clone())
            .collect();
        let mut per_view_demands = BTreeMap::new();
        for (&panel, view) in &self.views {
            let demand = collect_demands(view, &self.cache);
            for path in &demand {
                if demanded.len() < MAX_DIRECTORIES || demanded.contains(path) {
                    demanded.insert(path.clone());
                }
            }
            per_view_demands.insert(panel, demand);
        }

        // Eviction only removes nondemanded directories. Collapsed directories
        // otherwise stay cached, so toggling filters/expansion need not reread.
        for path in &demanded {
            if !self.cache.contains_key(path) {
                if self.cache.len() == MAX_DIRECTORIES {
                    let eviction = self
                        .cache
                        .iter()
                        .filter(|(cached, _)| !demanded.contains(*cached))
                        .min_by_key(|(_, directory)| directory.used)
                        .map(|(path, _)| path.clone());
                    if let Some(eviction) = eviction {
                        self.cache.remove(&eviction);
                    }
                }
                if self.cache.len() < MAX_DIRECTORIES {
                    self.cache
                        .insert(path.clone(), CachedDirectory::unloaded(self.version));
                }
            }
            if let Some(directory) = self.cache.get_mut(path) {
                directory.used = self.version;
                if matches!(directory.state, FileTreeDirectoryState::Unloaded) {
                    directory.state = FileTreeDirectoryState::Loading;
                }
            }
        }
        // Do not chase a collapsed/obsolete demand merely because it was queued
        // while a previous view was being projected.
        for (path, directory) in &mut self.cache {
            if !demanded.contains(path)
                && matches!(directory.state, FileTreeDirectoryState::Loading)
            {
                directory.state = FileTreeDirectoryState::Unloaded;
            }
        }

        let mut wanted: BTreeSet<_> = self
            .cache
            .iter()
            .filter(|(_, directory)| {
                matches!(
                    directory.state,
                    FileTreeDirectoryState::Ready | FileTreeDirectoryState::Loading
                )
            })
            .map(|(path, _)| path.clone())
            .collect();
        for view in self.views.values() {
            for path in [&view.input_absolute, &view.root] {
                if let Some(parent) = path.parent() {
                    wanted.insert(parent.to_owned());
                }
            }
        }
        self.watches.reconcile(&wanted);
        let watcher_error = self.watches.error();
        for (&panel, view) in &self.views {
            let limit_hit = per_view_demands[&panel]
                .iter()
                .any(|path| !demanded.contains(path));
            let snapshot = Arc::new(project(
                view,
                &self.cache,
                &demanded,
                self.version,
                watcher_error.clone(),
                limit_hit,
            ));
            let retired = {
                let mut mail = self.mailbox.lock().unwrap();
                match mail.views.get_mut(&panel) {
                    Some(target)
                        if target.serial == view.serial && target.refresh == view.refresh =>
                    {
                        target.snapshot.replace(snapshot)
                    }
                    _ => Some(snapshot), // obsolete generation/revision; retire off-lock
                }
            };
            drop(retired);
        }
    }
}

fn lexical_absolute(path: &Path) -> io::Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            component => normalized.push(component.as_os_str()),
        }
    }
    Ok(normalized)
}

fn resolve_root(view: &mut WorkerView, watches: &mut WatchState) {
    let previous_root = view.root.clone();
    let result = lexical_absolute(&view.request.root).and_then(|absolute| {
        view.input_absolute = absolute.clone();
        view.root = absolute.clone();
        if view.request.root.as_os_str().is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "No file tree root was supplied",
            ));
        }
        if let Some(parent) = absolute.parent() {
            watches.ensure(parent); // detect root disappearance before any baseline
        }
        let canonical = ByteSource::local(&absolute)
            .canonicalize()?
            .path()
            .to_owned();
        view.root = canonical.clone();
        if let Some(parent) = canonical.parent() {
            watches.ensure(parent);
        }
        if !ByteSource::local(&canonical).metadata()?.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                "Root is not a directory",
            ));
        }
        Ok(())
    });
    match result {
        Ok(()) => {
            view.error = None;
            view.retry_missing = false;
        }
        Err(error) => {
            view.retry_missing = matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            );
            if view.retry_missing {
                watches.forget_subtree(&previous_root);
                watches.forget_subtree(&view.root);
            }
            view.error = Some(format!(
                "Cannot open file tree root {}: {error}",
                view.root.display()
            ));
        }
    }
}

fn affects_directory(event: &Path, directory: &Path) -> bool {
    // A child changes its parent's listing; a moved/deleted ancestor invalidates
    // every cached descendant. Notifications never serve as tree contents.
    event == directory || event.parent() == Some(directory) || directory.starts_with(event)
}

fn affected_cache_paths(
    cache: &BTreeMap<PathBuf, CachedDirectory>,
    changes: &[crate::FileTreeChange],
) -> BTreeSet<PathBuf> {
    let mut affected = BTreeSet::new();
    for change in changes {
        let paths: [Option<&Path>; 2] = match change {
            crate::FileTreeChange::Created { path } | crate::FileTreeChange::Trashed { path } => {
                [Some(path), None]
            }
            crate::FileTreeChange::Copied { destination, .. } => [Some(destination), None],
            crate::FileTreeChange::Moved {
                source,
                destination,
            } => [Some(source), Some(destination)],
        };
        for path in paths.into_iter().flatten() {
            if let Some(parent) = path.parent() {
                if cache.contains_key(parent) {
                    affected.insert(parent.to_owned());
                }
            }
            // Prefix ranges keep large batch outcomes from turning into a full
            // cache scan for every item. Retained work is at most the cache cap.
            for (cached, _) in cache
                .range::<Path, _>((std::ops::Bound::Included(path), std::ops::Bound::Unbounded))
            {
                if !cached.starts_with(path) {
                    break;
                }
                affected.insert(cached.clone());
            }
        }
    }
    affected
}

fn read_directory(path: &Path, cancelled: impl Fn() -> bool) -> io::Result<Vec<CachedEntry>> {
    let mut entries: Vec<_> = ByteSource::local(path)
        .read_directory(&cancelled)?
        .into_iter()
        .map(|entry| {
            let label: Arc<str> = Arc::from(entry.name.to_string_lossy().as_ref());
            CachedEntry {
                path: entry.path,
                sort_label: label.to_lowercase(),
                name: entry.name,
                label,
                kind: entry.kind,
                hidden: entry.hidden,
            }
        })
        .collect();
    sort_entries(&mut entries);
    Ok(entries)
}

fn sort_entries(entries: &mut [CachedEntry]) {
    entries.sort_by(|left, right| {
        (left.kind != FileTreeEntryKind::Directory)
            .cmp(&(right.kind != FileTreeEntryKind::Directory))
            .then_with(|| left.sort_label.cmp(&right.sort_label))
            .then_with(|| left.name.cmp(&right.name))
    });
}

fn entry_hidden(view: &WorkerView, entry: &CachedEntry, parent_hidden: bool) -> bool {
    let relative = entry
        .path
        .strip_prefix(&view.root)
        .expect("Cache entries are under their root");
    parent_hidden
        || view.request.visibility.is_hidden(relative)
        || (view.request.visibility.hide_hidden && entry.hidden)
}

fn collect_demands(
    view: &WorkerView,
    cache: &BTreeMap<PathBuf, CachedDirectory>,
) -> BTreeSet<PathBuf> {
    let mut demanded = BTreeSet::new();
    if view.error.is_some() {
        return demanded;
    }
    // Root identity/baseline stay shared and watched even when this view is
    // collapsed. Descendant expansion is panel-local and dormant until reopen.
    demanded.insert(view.root.clone());
    if !view.request.root_expanded {
        return demanded;
    }
    let mut stack = Vec::new();
    if let Some(directory) = cache.get(&view.root) {
        if !matches!(directory.state, FileTreeDirectoryState::Failed(_)) {
            stack.push((directory.entries.iter(), false));
        }
    }
    while let Some((entries, parent_hidden)) = stack.last_mut() {
        let Some(entry) = entries.next() else {
            stack.pop();
            continue;
        };
        let hidden = entry_hidden(view, entry, *parent_hidden);
        if entry.kind != FileTreeEntryKind::Directory
            || !view.request.expanded.contains(&entry.path)
            || (hidden && !view.request.show_hidden)
        {
            continue;
        }
        demanded.insert(entry.path.clone());
        // One extra demand carries an explicit limit error. The DFS stack is
        // bounded by loaded depth, even if a root has a million expanded children.
        if demanded.len() > MAX_DIRECTORIES {
            break;
        }
        if let Some(directory) = cache.get(&entry.path) {
            if !matches!(directory.state, FileTreeDirectoryState::Failed(_)) {
                stack.push((directory.entries.iter(), hidden));
            }
        }
    }
    demanded
}

fn project(
    view: &WorkerView,
    cache: &BTreeMap<PathBuf, CachedDirectory>,
    demanded: &BTreeSet<PathBuf>,
    version: u64,
    watcher_error: Option<String>,
    limit_hit: bool,
) -> FileTreeSnapshot {
    let limit_error: Arc<str> = Arc::from(format!(
        "Expanded directory limit reached ({MAX_DIRECTORIES} shared directories); collapse another branch"
    ));
    let state_for = |path: &Path, expanded: bool| {
        if expanded && !demanded.contains(path) {
            FileTreeDirectoryState::Failed(limit_error.clone())
        } else {
            cache
                .get(path)
                .map(|directory| directory.state.clone())
                .unwrap_or(FileTreeDirectoryState::Unloaded)
        }
    };
    let name = view
        .root
        .file_name()
        .unwrap_or(view.root.as_os_str())
        .to_owned();
    let root_state = if let Some(error) = &view.error {
        FileTreeDirectoryState::Failed(Arc::from(error.as_str()))
    } else {
        state_for(&view.root, true)
    };
    let mut loading = matches!(root_state, FileTreeDirectoryState::Loading);
    let mut error = view.error.clone();
    if let FileTreeDirectoryState::Failed(message) = &root_state {
        error.get_or_insert_with(|| message.to_string());
    }
    if limit_hit {
        error.get_or_insert_with(|| limit_error.to_string());
    }
    let mut directory_errors = Vec::new();
    let mut rows = vec![FileTreeRow {
        path: view.root.clone(),
        label: Arc::from(name.to_string_lossy().as_ref()),
        name,
        kind: FileTreeEntryKind::Directory,
        depth: 0,
        hidden: false,
        directory_state: root_state,
    }];
    // Iterative DFS stores only directory slice iterators, not one path clone
    // per child. Work and row allocations happen solely on this worker.
    let mut stack = Vec::new();
    if view.error.is_none() && view.request.root_expanded {
        if let Some(directory) = cache.get(&view.root) {
            if !matches!(directory.state, FileTreeDirectoryState::Failed(_)) {
                stack.push((directory.entries.iter(), 1, false));
            }
        }
    }
    while let Some((entries, depth, parent_hidden)) = stack.last_mut() {
        let Some(entry) = entries.next() else {
            stack.pop();
            continue;
        };
        let depth = *depth;
        let hidden = entry_hidden(view, entry, *parent_hidden);
        if hidden && !view.request.show_hidden {
            continue;
        }
        let expanded = entry.kind == FileTreeEntryKind::Directory
            && view.request.expanded.contains(&entry.path);
        let state = if entry.kind == FileTreeEntryKind::Directory {
            state_for(&entry.path, expanded)
        } else {
            FileTreeDirectoryState::Ready
        };
        loading |= expanded && matches!(state, FileTreeDirectoryState::Loading);
        if entry.kind == FileTreeEntryKind::Directory {
            if let Some(directory) = cache.get(&entry.path) {
                if let FileTreeDirectoryState::Failed(message) = &directory.state {
                    directory_errors.push((entry.path.clone(), message.clone()));
                }
            }
        }
        rows.push(FileTreeRow {
            path: entry.path.clone(),
            name: entry.name.clone(),
            label: entry.label.clone(),
            kind: entry.kind,
            depth,
            hidden,
            directory_state: state,
        });
        if expanded && demanded.contains(&entry.path) {
            if let Some(directory) = cache.get(&entry.path) {
                if !matches!(directory.state, FileTreeDirectoryState::Failed(_)) {
                    stack.push((directory.entries.iter(), depth + 1, hidden));
                }
            }
        }
    }
    let row_indices = Arc::new(index_rows(&rows));
    FileTreeSnapshot {
        root: view.root.clone(),
        generation: view.request.generation,
        revision: view.request.revision,
        version,
        rows: rows.into(),
        loading,
        error,
        directory_errors: directory_errors.into(),
        watcher_error,
        cached_directories: cache.len(),
        row_indices,
    }
}

// One native-path index stores both endpoints. The depth stack closes a folder
// when its next sibling/ancestor appears; it never searches forward per row.
// Construction is linear in projected rows with only directory-depth scratch.
fn index_rows(rows: &[FileTreeRow]) -> HashMap<PathBuf, (usize, usize)> {
    let mut locations: HashMap<PathBuf, (usize, usize)> = HashMap::with_capacity(rows.len());
    let mut open_directories: Vec<usize> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        while let Some(&open) = open_directories.last() {
            if rows[open].depth < row.depth {
                break;
            }
            open_directories.pop();
            locations.get_mut(&rows[open].path).unwrap().1 = index;
        }
        locations.insert(row.path.clone(), (index, index + 1));
        if row.kind == FileTreeEntryKind::Directory {
            open_directories.push(index);
        }
    }
    for open in open_directories {
        locations.get_mut(&rows[open].path).unwrap().1 = rows.len();
    }
    locations
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::AtomicU64;
    use std::sync::atomic::Ordering;

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

    struct TempTree(PathBuf);
    impl TempTree {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "bedtime-filetree-backend-{}-{}",
                std::process::id(),
                NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(fs::canonicalize(path).unwrap())
        }
    }
    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn request(panel: Entity, root: &Path) -> ViewRequest {
        ViewRequest {
            panel,
            generation: 1,
            revision: 1,
            root: root.to_owned(),
            root_expanded: true,
            expanded: Arc::default(),
            visibility: crate::FileTreeVisibility::default(),
            show_hidden: false,
        }
    }

    fn wait_snapshot(
        backend: &Backend,
        panel: Entity,
        accept: impl Fn(&FileTreeSnapshot) -> bool,
    ) -> Arc<FileTreeSnapshot> {
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            if let Some(snapshot) = backend.snapshot(panel) {
                if accept(&snapshot) {
                    return snapshot;
                }
                assert!(
                    Instant::now() < deadline,
                    "Timed out waiting for snapshot: {snapshot:?}"
                );
            } else {
                assert!(
                    Instant::now() < deadline,
                    "No file tree snapshot; worker {:?}",
                    backend.status()
                );
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn ready(snapshot: &FileTreeSnapshot) -> bool {
        !snapshot.loading
            && snapshot
                .rows
                .first()
                .is_some_and(|row| row.directory_state == FileTreeDirectoryState::Ready)
    }

    #[test]
    fn lazy_shared_baseline_and_worker_built_row_index() {
        let tree = TempTree::new();
        fs::create_dir(tree.0.join("branch")).unwrap();
        fs::write(tree.0.join("branch/leaf"), b"leaf").unwrap();
        let mut scope = ManagedWork::default();
        let mut backend = Backend::spawn(&mut scope).unwrap();
        let panel = Entity::from_raw_u32(1).unwrap();
        backend.set_view(request(panel, &tree.0)).unwrap();
        let snapshot = wait_snapshot(&backend, panel, ready);
        assert_eq!(snapshot.cached_directories, 1);
        assert_eq!(snapshot.rows.len(), 2);
        assert_eq!(
            snapshot.rows[1].directory_state,
            FileTreeDirectoryState::Unloaded
        );
        assert_eq!(snapshot.row_index(&tree.0.join("branch")), Some(1));
        assert_eq!(scope.list().len(), 1);
        assert_eq!(scope.list()[0].owner, None);

        let mut expanded = request(panel, &tree.0);
        expanded.revision = 2;
        expanded.expanded = Arc::new(BTreeSet::from([tree.0.join("branch")]));
        backend.set_view(expanded).unwrap();
        let snapshot = wait_snapshot(&backend, panel, |snapshot| {
            snapshot.revision == 2 && ready(snapshot)
        });
        assert_eq!(snapshot.rows.len(), 3);
        assert_eq!(snapshot.rows[2].depth, 2);
        let other = Entity::from_raw_u32(2).unwrap();
        backend.set_view(request(other, &tree.0)).unwrap();
        let shared = wait_snapshot(&backend, other, ready);
        assert_eq!(shared.cached_directories, 2);
        assert_eq!(scope.list().len(), 1);
        backend.shutdown();
    }

    #[test]
    fn collapsed_root_does_not_load_descendants_and_other_panels_expand_independently() {
        let tree = TempTree::new();
        let branch = tree.0.join("visible");
        let hidden = tree.0.join(".hidden");
        let unopened = branch.join("unopened");
        for path in [&branch, &hidden, &unopened] {
            fs::create_dir(path).unwrap();
        }
        for path in [
            branch.join("leaf"),
            hidden.join("leaf"),
            unopened.join("deeper"),
        ] {
            fs::write(path, b"").unwrap();
        }
        let mut scope = ManagedWork::default();
        let mut backend = Backend::spawn(&mut scope).unwrap();
        let first = Entity::from_raw_u32(1).unwrap();
        let second = Entity::from_raw_u32(2).unwrap();
        let mut first_view = request(first, &tree.0);
        first_view.root_expanded = false;
        first_view.show_hidden = true;
        first_view.expanded = Arc::new(BTreeSet::from([branch.clone(), hidden.clone()]));
        backend.set_view(first_view.clone()).unwrap();
        let collapsed = wait_snapshot(&backend, first, ready);
        assert_eq!(collapsed.rows.len(), 1);
        assert_eq!(collapsed.rows[0].depth, 0);
        assert_eq!(
            collapsed.cached_directories, 1,
            "Even explicitly expanded/revealed children stay lazy under a collapsed root"
        );

        let mut second_view = first_view.clone();
        second_view.panel = second;
        second_view.root_expanded = true;
        backend.set_view(second_view.clone()).unwrap();
        let expanded = wait_snapshot(&backend, second, |snapshot| {
            ready(snapshot) && snapshot.rows.len() == 6
        });
        assert_eq!(expanded.cached_directories, 3);
        assert_eq!(
            expanded.rows[expanded.row_index(&unopened).unwrap()].directory_state,
            FileTreeDirectoryState::Unloaded
        );
        assert!(expanded.row_index(&unopened.join("deeper")).is_none());
        let still_collapsed = wait_snapshot(&backend, first, |snapshot| {
            ready(snapshot) && snapshot.cached_directories == 3
        });
        assert_eq!(still_collapsed.rows.len(), 1);
        assert_eq!(still_collapsed.generation, 1);

        second_view.root_expanded = false;
        second_view.revision = 2;
        backend.set_view(second_view).unwrap();
        wait_snapshot(&backend, second, |snapshot| {
            snapshot.revision == 2 && ready(snapshot)
        });
        first_view.root_expanded = true;
        first_view.revision = 2;
        backend.set_view(first_view).unwrap();
        let reopened = wait_snapshot(&backend, first, |snapshot| {
            snapshot.revision == 2 && ready(snapshot) && snapshot.rows.len() == 6
        });
        assert_eq!(reopened.generation, 1);
        assert_eq!(reopened.cached_directories, 3);
        assert!(reopened.row_index(&branch.join("leaf")).is_some());
        assert!(reopened.row_index(&hidden.join("leaf")).is_some());
        assert_eq!(wait_snapshot(&backend, second, ready).rows.len(), 1);
    }

    #[test]
    fn root_collapse_and_reopen_reuse_last_baseline_even_during_reload() {
        let tree = TempTree::new();
        let branch = tree.0.join("branch");
        let hidden = tree.0.join(".hidden");
        fs::create_dir(&branch).unwrap();
        fs::create_dir(&hidden).unwrap();
        let leaf = branch.join("leaf");
        fs::write(&leaf, b"").unwrap();
        let mut cache = BTreeMap::new();
        for path in [&tree.0, &branch] {
            cache.insert(
                path.clone(),
                CachedDirectory {
                    entries: read_directory(path, || false).unwrap(),
                    state: FileTreeDirectoryState::Ready,
                    failure_kind: None,
                    used: 1,
                },
            );
        }
        let mut requested = request(Entity::from_raw_u32(1).unwrap(), &tree.0);
        requested.expanded = Arc::new(BTreeSet::from([branch.clone(), hidden.clone()]));
        let mut view = WorkerView {
            request: Arc::new(requested.clone()),
            serial: 1,
            refresh: 0,
            input_absolute: tree.0.clone(),
            root: tree.0.clone(),
            error: None,
            retry_missing: false,
        };
        let expanded_demands = collect_demands(&view, &cache);
        assert_eq!(
            expanded_demands,
            BTreeSet::from([tree.0.clone(), branch.clone()])
        );
        let before = project(&view, &cache, &expanded_demands, 1, None, false);
        let leaf_label = before.rows[before.row_index(&leaf).unwrap()].label.clone();
        // Projection cannot accidentally reread after collapse/reopen: its
        // cached source is now gone, and a real successful baseline would differ.
        fs::remove_dir_all(&branch).unwrap();
        fs::remove_dir_all(&hidden).unwrap();
        requested.root_expanded = false;
        requested.revision = 2;
        view.request = Arc::new(requested.clone());
        assert_eq!(
            collect_demands(&view, &cache),
            BTreeSet::from([tree.0.clone()])
        );
        let collapsed = project(&view, &cache, &expanded_demands, 2, None, false);
        assert_eq!(collapsed.rows.len(), 1);
        assert_eq!(collapsed.row_index(&tree.0), Some(0));
        assert_eq!(cache.len(), 2);

        requested.root_expanded = true;
        requested.revision = 3;
        view.request = Arc::new(requested.clone());
        let reopened = project(
            &view,
            &cache,
            &collect_demands(&view, &cache),
            3,
            None,
            false,
        );
        assert_eq!(reopened.generation, before.generation);
        assert_eq!(reopened.rows.len(), before.rows.len());
        assert!(Arc::ptr_eq(
            &leaf_label,
            &reopened.rows[reopened.row_index(&leaf).unwrap()].label
        ));
        assert!(reopened.row_index(&hidden).is_none());

        for directory in cache.values_mut() {
            directory.invalidate(true);
            directory.state = FileTreeDirectoryState::Loading;
        }
        requested.root_expanded = false;
        view.request = Arc::new(requested.clone());
        let loading_closed = project(&view, &cache, &expanded_demands, 4, None, false);
        assert!(loading_closed.loading);
        assert_eq!(loading_closed.rows.len(), 1);
        assert_eq!(
            collect_demands(&view, &cache),
            BTreeSet::from([tree.0.clone()])
        );
        requested.root_expanded = true;
        view.request = Arc::new(requested);
        let loading_open = project(
            &view,
            &cache,
            &collect_demands(&view, &cache),
            5,
            None,
            false,
        );
        assert!(loading_open.loading);
        assert!(loading_open.row_index(&leaf).is_some());
        assert!(loading_open.row_index(&hidden).is_none());
    }

    #[test]
    fn subtree_extents_follow_nested_siblings_and_collapsed_branches() {
        let tree = TempTree::new();
        let a = tree.0.join("a");
        let nested = a.join("nested");
        let b = tree.0.join("b");
        fs::create_dir_all(&nested).unwrap();
        fs::create_dir(&b).unwrap();
        let nested_leaf = nested.join("leaf.txt");
        let a_file = a.join("file.txt");
        let b_file = b.join("file.txt");
        let top_file = tree.0.join("top.txt");
        for path in [&nested_leaf, &a_file, &b_file, &top_file] {
            fs::write(path, b"").unwrap();
        }
        let mut cache = BTreeMap::new();
        for path in [&tree.0, &a, &nested, &b] {
            cache.insert(
                path.clone(),
                CachedDirectory {
                    entries: read_directory(path, || false).unwrap(),
                    state: FileTreeDirectoryState::Ready,
                    failure_kind: None,
                    used: 1,
                },
            );
        }
        let mut requested = request(Entity::from_raw_u32(1).unwrap(), &tree.0);
        requested.expanded = Arc::new(BTreeSet::from([a.clone(), nested.clone(), b.clone()]));
        let mut view = WorkerView {
            request: Arc::new(requested.clone()),
            serial: 1,
            refresh: 0,
            input_absolute: tree.0.clone(),
            root: tree.0.clone(),
            error: None,
            retry_missing: false,
        };
        let bounds = |snapshot: &FileTreeSnapshot, path: &Path, index, end| {
            assert_eq!(snapshot.row_index(path), Some(index), "start for {path:?}");
            assert_eq!(
                snapshot.subtree_end_index(path),
                Some(end),
                "end for {path:?}"
            );
        };
        let expanded = project(
            &view,
            &cache,
            &collect_demands(&view, &cache),
            1,
            None,
            false,
        );
        assert_eq!(expanded.rows.len(), 8);
        bounds(&expanded, &tree.0, 0, 8);
        bounds(&expanded, &a, 1, 5);
        bounds(&expanded, &nested, 2, 4);
        bounds(&expanded, &nested_leaf, 3, 4);
        bounds(&expanded, &a_file, 4, 5);
        bounds(&expanded, &b, 5, 7);
        bounds(&expanded, &b_file, 6, 7);
        bounds(&expanded, &top_file, 7, 8);
        assert_eq!(expanded.subtree_end_index(&tree.0.join("absent")), None);

        Arc::make_mut(&mut requested.expanded).remove(&nested);
        requested.revision = 2;
        view.request = Arc::new(requested.clone());
        let nested_closed = project(
            &view,
            &cache,
            &collect_demands(&view, &cache),
            2,
            None,
            false,
        );
        assert_eq!(nested_closed.rows.len(), 7);
        bounds(&nested_closed, &nested, 2, 3);
        bounds(&nested_closed, &a, 1, 4);
        bounds(&nested_closed, &b, 4, 6);
        bounds(&nested_closed, &tree.0, 0, 7);
        assert_eq!(nested_closed.subtree_end_index(&nested_leaf), None);

        Arc::make_mut(&mut requested.expanded).remove(&a);
        requested.revision = 3;
        view.request = Arc::new(requested.clone());
        let a_closed = project(
            &view,
            &cache,
            &collect_demands(&view, &cache),
            3,
            None,
            false,
        );
        assert_eq!(a_closed.rows.len(), 5);
        bounds(&a_closed, &a, 1, 2);
        bounds(&a_closed, &b, 2, 4);
        bounds(&a_closed, &tree.0, 0, 5);
        assert_eq!(a_closed.subtree_end_index(&nested), None);

        requested.root_expanded = false;
        requested.revision = 4;
        view.request = Arc::new(requested);
        let root_closed = project(
            &view,
            &cache,
            &collect_demands(&view, &cache),
            4,
            None,
            false,
        );
        assert_eq!(root_closed.rows.len(), 1);
        bounds(&root_closed, &tree.0, 0, 1);
        assert_eq!(root_closed.subtree_end_index(&a), None);
        assert_eq!(root_closed.subtree_end_index(&b), None);
    }

    #[test]
    fn directories_first_case_insensitive_and_native_tiebreaker() {
        let tree = TempTree::new();
        for name in ["beta", "Alpha", "a-other"] {
            fs::write(tree.0.join(name), b"").unwrap();
        }
        for name in ["z-dir", "B-dir"] {
            fs::create_dir(tree.0.join(name)).unwrap();
        }
        let entries = read_directory(&tree.0, || false).unwrap();
        let names: Vec<_> = entries
            .iter()
            .map(|entry| entry.name.to_str().unwrap())
            .collect();
        assert_eq!(names, ["B-dir", "z-dir", "a-other", "Alpha", "beta"]);
        // Exercise native-name ties without depending on a case-sensitive host
        // filesystem (default macOS/Windows filesystems cannot create both).
        let mut ties: Vec<_> = ["a", "A"]
            .into_iter()
            .map(|name| CachedEntry {
                path: tree.0.join(name),
                name: OsString::from(name),
                label: Arc::from(name),
                sort_label: name.to_lowercase(),
                kind: FileTreeEntryKind::File,
                hidden: false,
            })
            .collect();
        sort_entries(&mut ties);
        assert_eq!(ties[0].name, "A");
        assert_eq!(ties[1].name, "a");
    }

    #[test]
    fn invalid_root_and_failed_directory_are_explicit_not_empty_success() {
        let tree = TempTree::new();
        let missing = tree.0.join("missing");
        let mut scope = ManagedWork::default();
        let mut backend = Backend::spawn(&mut scope).unwrap();
        let panel = Entity::from_raw_u32(1).unwrap();
        backend.set_view(request(panel, &missing)).unwrap();
        let snapshot = wait_snapshot(&backend, panel, |snapshot| snapshot.error.is_some());
        assert_eq!(snapshot.rows.len(), 1);
        assert!(
            snapshot.directory_errors.is_empty(),
            "Root failure is already in snapshot.error"
        );
        assert!(matches!(
            snapshot.rows[0].directory_state,
            FileTreeDirectoryState::Failed(_)
        ));
        assert!(!snapshot.loading);

        // Permission failures are represented identically regardless of whether
        // the test runner has privilege to bypass actual Unix mode bits.
        let mut directory = CachedDirectory::unloaded(0);
        directory.state = FileTreeDirectoryState::Failed(Arc::from("Permission denied"));
        directory.failure_kind = Some(io::ErrorKind::PermissionDenied);
        assert!(!directory.invalidate(false));
        assert!(matches!(directory.state, FileTreeDirectoryState::Failed(_)));
        assert!(directory.invalidate(true));
        assert_eq!(directory.state, FileTreeDirectoryState::Unloaded);
    }

    #[test]
    fn latest_value_views_are_bounded_and_stale_generations_cannot_install() {
        let tree = TempTree::new();
        let mut scope = ManagedWork::default();
        let mut backend = Backend::spawn(&mut scope).unwrap();
        let panel = Entity::from_raw_u32(1).unwrap();
        for revision in 1..=1000 {
            let mut view = request(panel, &tree.0);
            view.generation = 2;
            view.revision = revision;
            backend.set_view(view).unwrap();
        }
        backend
            .set_view(request(panel, &tree.0.join("obsolete")))
            .unwrap();
        let snapshot = wait_snapshot(&backend, panel, |snapshot| {
            snapshot.generation == 2 && snapshot.revision == 1000 && ready(snapshot)
        });
        assert_eq!(snapshot.root, tree.0);
        assert_eq!(backend.mailbox.lock().unwrap().views.len(), 1);
        for id in 2..=MAX_VIEWS as u32 {
            backend
                .set_view(request(Entity::from_raw_u32(id).unwrap(), &tree.0))
                .unwrap();
        }
        assert!(
            backend
                .set_view(request(Entity::from_raw_u32(1000).unwrap(), &tree.0))
                .is_err()
        );
        assert_eq!(backend.mailbox.lock().unwrap().views.len(), MAX_VIEWS);
        backend.remove_view(panel);
        assert!(backend.snapshot(panel).is_none());
    }

    #[test]
    fn native_watching_eventually_observes_external_create_and_rename() {
        let tree = TempTree::new();
        let mut scope = ManagedWork::default();
        let mut backend = Backend::spawn(&mut scope).unwrap();
        let panel = Entity::from_raw_u32(1).unwrap();
        backend.set_view(request(panel, &tree.0)).unwrap();
        let baseline = wait_snapshot(&backend, panel, ready);
        assert!(
            baseline.watcher_error.is_none(),
            "Native watch installation failed: {:?}",
            baseline.watcher_error
        );
        fs::write(tree.0.join("external"), b"hello").unwrap();
        wait_snapshot(&backend, panel, |snapshot| {
            ready(snapshot) && snapshot.row_index(&tree.0.join("external")).is_some()
        });
        fs::rename(tree.0.join("external"), tree.0.join("renamed")).unwrap();
        wait_snapshot(&backend, panel, |snapshot| {
            ready(snapshot)
                && snapshot.row_index(&tree.0.join("renamed")).is_some()
                && snapshot.row_index(&tree.0.join("external")).is_none()
        });
    }

    #[test]
    fn worker_resolves_relative_root_and_always_includes_root_row() {
        let mut scope = ManagedWork::default();
        let mut backend = Backend::spawn(&mut scope).unwrap();
        let panel = Entity::from_raw_u32(1).unwrap();
        backend.set_view(request(panel, Path::new("."))).unwrap();
        let snapshot = wait_snapshot(&backend, panel, ready);
        assert!(snapshot.root.is_absolute());
        assert_eq!(
            snapshot.root,
            fs::canonicalize(std::env::current_dir().unwrap()).unwrap()
        );
        assert_eq!(snapshot.rows[0].depth, 0);
        assert_eq!(snapshot.rows[0].path, snapshot.root);
        assert_eq!(snapshot.row_index(&snapshot.root), Some(0));
    }

    #[test]
    fn root_parent_watch_observes_disappearance_and_recreation() {
        let tree = TempTree::new();
        let root = tree.0.join("root");
        fs::create_dir(&root).unwrap();
        let mut scope = ManagedWork::default();
        let mut backend = Backend::spawn(&mut scope).unwrap();
        let panel = Entity::from_raw_u32(1).unwrap();
        backend.set_view(request(panel, &root)).unwrap();
        wait_snapshot(&backend, panel, ready);
        fs::remove_dir(&root).unwrap();
        let failed = wait_snapshot(&backend, panel, |snapshot| snapshot.error.is_some());
        assert_eq!(failed.rows.len(), 1);
        assert!(matches!(
            failed.rows[0].directory_state,
            FileTreeDirectoryState::Failed(_)
        ));
        fs::create_dir(&root).unwrap();
        fs::write(root.join("returned"), b"").unwrap();
        wait_snapshot(&backend, panel, |snapshot| {
            ready(snapshot) && snapshot.row_index(&root.join("returned")).is_some()
        });
        // Recovery must reinstall a watch on the NEW directory identity, not
        // merely notice its first baseline through the surviving parent watch.
        fs::write(root.join("after-recovery"), b"").unwrap();
        wait_snapshot(&backend, panel, |snapshot| {
            ready(snapshot) && snapshot.row_index(&root.join("after-recovery")).is_some()
        });
    }

    #[derive(bevy::prelude::Resource, Default)]
    struct Results(Vec<OperationOutcome<crate::FileTreeOperationResult>>);
    fn operation_app() -> bevy::prelude::App {
        let mut app = bevy::prelude::App::new();
        app.add_plugins(bedtime_workbench::WorkbenchCorePlugin)
            .init_resource::<Results>();
        app
    }
    fn submit(
        backend: &mut Backend,
        app: &mut bevy::prelude::App,
        request: crate::FileTreeOperationRequest,
    ) -> Result<(), String> {
        backend.submit_operation(
            &mut app.world_mut().resource_mut::<ManagedWork>(),
            request,
            |world, outcome| {
                world.resource_mut::<Results>().0.push(outcome);
            },
        )
    }
    #[test]
    fn operations_reserve_outcome_slots_and_survive_managed_task_cancellation() {
        let tree = TempTree::new();
        let mut app = operation_app();
        let mut backend =
            Backend::spawn(&mut app.world_mut().resource_mut::<ManagedWork>()).unwrap();
        let operation = |id| crate::FileTreeOperationRequest {
            id,
            root: tree.0.clone(),
            operation: crate::FileTreeOperation::CreateFile {
                directory: tree.0.clone(),
                name: OsString::from(format!("created-{id}")),
            },
        };
        for id in 1..=MAX_OPERATIONS as u64 {
            submit(&mut backend, &mut app, operation(id)).unwrap();
        }
        let deadline = Instant::now() + Duration::from_secs(8);
        while !(1..=MAX_OPERATIONS).all(|id| tree.0.join(format!("created-{id}")).exists()) {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(submit(&mut backend, &mut app, operation(100)).is_err());
        while app.world().resource::<Results>().0.len() != MAX_OPERATIONS {
            app.update();
        }
        let results = std::mem::take(&mut app.world_mut().resource_mut::<Results>().0);
        for outcome in results {
            let result = outcome.result.unwrap();
            assert_eq!(result.completed.len(), 1);
            assert!(result.errors.is_empty());
        }
        submit(&mut backend, &mut app, operation(100)).unwrap();
        app.world_mut().resource_mut::<ManagedWork>().shutdown();
        while !bedtime_workbench::work_finished(app.world()) {
            app.update();
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        let results = &app.world().resource::<Results>().0;
        assert_eq!(results.len(), 1);
        assert!(results[0].cancellation_requested);
        let result = results[0].result.as_ref().unwrap();
        assert_eq!(result.id, 100);
        assert_eq!(
            result.completed.len(),
            usize::from(tree.0.join("created-100").is_file())
        );
        assert!(result.cancelled || !result.completed.is_empty());
    }

    #[test]
    fn managed_completion_outlives_backend_until_orderly_world_drain() {
        let tree = TempTree::new();
        let mut app = operation_app();
        let mut backend =
            Backend::spawn(&mut app.world_mut().resource_mut::<ManagedWork>()).unwrap();
        submit(
            &mut backend,
            &mut app,
            crate::FileTreeOperationRequest {
                id: 1,
                root: tree.0.clone(),
                operation: crate::FileTreeOperation::CreateFile {
                    directory: tree.0.clone(),
                    name: OsString::from("committed-or-cancelled"),
                },
            },
        )
        .unwrap();
        drop(backend);
        app.world_mut().resource_mut::<ManagedWork>().shutdown();
        let deadline = Instant::now() + Duration::from_secs(8);
        while !bedtime_workbench::work_finished(app.world()) {
            app.update();
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        let results = &app.world().resource::<Results>().0;
        assert_eq!(results.len(), 1);
        let result = results[0].result.as_ref().unwrap();
        assert_eq!(result.id, 1);
        assert_eq!(
            result.completed.len(),
            usize::from(tree.0.join("committed-or-cancelled").exists())
        );
        assert!(result.cancelled || result.completed.len() == 1);
        app.update();
        assert_eq!(app.world().resource::<Results>().0.len(), 1);
    }

    #[test]
    fn worker_panic_delivers_committed_outcome_and_unknown_effects_for_abandoned_queue() {
        let mut app = operation_app();
        let mailbox = Arc::new(Mutex::new(Mailbox::default()));
        for id in 1..=3 {
            let completion = app
                .world_mut()
                .resource_mut::<ManagedWork>()
                .admit_operation(None, "Queued mutation", |world, outcome| {
                    world.resource_mut::<Results>().0.push(outcome);
                })
                .unwrap();
            mailbox
                .lock()
                .unwrap()
                .operations
                .push_back(PendingOperation {
                    request: crate::FileTreeOperationRequest {
                        id,
                        root: PathBuf::from("/unavailable"),
                        operation: crate::FileTreeOperation::CreateFile {
                            directory: PathBuf::from("/unavailable"),
                            name: "unused".into(),
                        },
                    },
                    completion,
                });
        }
        app.world_mut()
            .resource_mut::<ManagedWork>()
            .spawn_task(None, "Failing executor", move |_| {
                let _close = CloseOperations(mailbox.clone());
                let first = mailbox.lock().unwrap().operations.pop_front().unwrap();
                first.completion.complete(crate::FileTreeOperationResult {
                    id: first.request.id,
                    completed: Vec::new(),
                    cancelled: false,
                    errors: Vec::new(),
                });
                let _active = mailbox.lock().unwrap().operations.pop_front().unwrap();
                panic!("Injected executor failure after one reported outcome");
            })
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !bedtime_workbench::work_finished(app.world()) {
            app.update();
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        let outcomes = &app.world().resource::<Results>().0;
        assert_eq!(outcomes.len(), 3);
        assert_eq!(outcomes.iter().filter(|o| o.result.is_ok()).count(), 1);
        assert_eq!(
            outcomes
                .iter()
                .find_map(|o| o.result.as_ref().ok())
                .unwrap()
                .id,
            1
        );
        assert!(
            outcomes
                .iter()
                .filter_map(|o| o.result.as_ref().err())
                .all(|e| e.contains("effects may have occurred"))
        );
    }

    #[test]
    fn expanded_limit_is_explicit_without_silently_dropping_rows() {
        let tree = TempTree::new();
        let root_entries: Vec<_> = (0..MAX_DIRECTORIES + 4)
            .map(|index| {
                let name = format!("branch-{index:04}");
                CachedEntry {
                    path: tree.0.join(&name),
                    name: OsString::from(&name),
                    label: Arc::from(name.as_str()),
                    sort_label: name,
                    kind: FileTreeEntryKind::Directory,
                    hidden: false,
                }
            })
            .collect();
        let panel = Entity::from_raw_u32(1).unwrap();
        let mut requested = request(panel, &tree.0);
        requested.expanded = Arc::new(
            root_entries
                .iter()
                .map(|entry| entry.path.clone())
                .collect(),
        );
        let view = WorkerView {
            request: Arc::new(requested),
            serial: 1,
            refresh: 0,
            input_absolute: tree.0.clone(),
            root: tree.0.clone(),
            error: None,
            retry_missing: false,
        };
        let cache = BTreeMap::from([(
            tree.0.clone(),
            CachedDirectory {
                entries: root_entries,
                state: FileTreeDirectoryState::Ready,
                failure_kind: None,
                used: 0,
            },
        )]);
        let demands = collect_demands(&view, &cache);
        assert_eq!(demands.len(), MAX_DIRECTORIES + 1);
        let mut allowed = BTreeSet::from([tree.0.clone()]);
        for path in &demands {
            if allowed.len() < MAX_DIRECTORIES {
                allowed.insert(path.clone());
            }
        }
        let snapshot = project(&view, &cache, &allowed, 1, None, true);
        assert!(snapshot.error.as_deref().unwrap().contains("limit"));
        assert!(
            snapshot.directory_errors.is_empty(),
            "One global cache-limit error must not expand into unbounded per-path dialog entries"
        );
        assert!(
            snapshot
                .rows
                .iter()
                .any(|row| matches!(row.directory_state, FileTreeDirectoryState::Failed(_)))
        );
    }

    #[test]
    fn explicit_visibility_toggle_reprojects_cached_rows_without_filesystem_reads() {
        let tree = TempTree::new();
        fs::create_dir(tree.0.join("branch")).unwrap();
        fs::write(tree.0.join("branch/leaf"), b"").unwrap();
        let mut cache = BTreeMap::new();
        for path in [&tree.0, &tree.0.join("branch")] {
            cache.insert(
                path.clone(),
                CachedDirectory {
                    entries: read_directory(path, || false).unwrap(),
                    state: FileTreeDirectoryState::Ready,
                    failure_kind: None,
                    used: 1,
                },
            );
        }
        let mut requested = request(Entity::from_raw_u32(1).unwrap(), &tree.0);
        requested.expanded = Arc::new(BTreeSet::from([tree.0.join("branch")]));
        requested.visibility.hide(PathBuf::from("branch")).unwrap();
        let mut view = WorkerView {
            request: Arc::new(requested.clone()),
            serial: 1,
            refresh: 0,
            input_absolute: tree.0.clone(),
            root: tree.0.clone(),
            error: None,
            retry_missing: false,
        };
        // Removing the source makes any accidental projection-time I/O observable.
        fs::remove_dir_all(tree.0.join("branch")).unwrap();
        let demanded = BTreeSet::from([tree.0.clone(), tree.0.join("branch")]);
        assert_eq!(
            project(&view, &cache, &demanded, 1, None, false).rows.len(),
            1
        );
        requested.show_hidden = true;
        view.request = Arc::new(requested);
        let revealed = project(&view, &cache, &demanded, 2, None, false);
        assert_eq!(revealed.rows.len(), 3);
        assert!(revealed.rows[1].hidden && revealed.rows[2].hidden);
        assert_eq!(
            revealed.rows[2].directory_state,
            FileTreeDirectoryState::Ready
        );
    }

    #[test]
    fn directory_error_index_contains_visible_child_failures_but_not_root_failures() {
        let tree = TempTree::new();
        let failed = tree.0.join("failed");
        let hidden = tree.0.join(".hidden");
        fs::create_dir(&failed).unwrap();
        fs::create_dir(&hidden).unwrap();
        let message: Arc<str> = Arc::from("Cannot read folder: permission denied");
        let hidden_message: Arc<str> = Arc::from("Cannot read hidden folder: access denied");
        let mut cache = BTreeMap::from([(
            tree.0.clone(),
            CachedDirectory {
                entries: read_directory(&tree.0, || false).unwrap(),
                state: FileTreeDirectoryState::Ready,
                failure_kind: None,
                used: 1,
            },
        )]);
        for (path, message) in [(&failed, &message), (&hidden, &hidden_message)] {
            cache.insert(
                path.clone(),
                CachedDirectory {
                    entries: Vec::new(),
                    state: FileTreeDirectoryState::Failed(message.clone()),
                    failure_kind: Some(io::ErrorKind::PermissionDenied),
                    used: 1,
                },
            );
        }
        let mut requested = request(Entity::from_raw_u32(1).unwrap(), &tree.0);
        requested.expanded = Arc::new(BTreeSet::from([failed.clone(), hidden.clone()]));
        let mut view = WorkerView {
            request: Arc::new(requested.clone()),
            serial: 1,
            refresh: 0,
            input_absolute: tree.0.clone(),
            root: tree.0.clone(),
            error: None,
            retry_missing: false,
        };
        let visible = project(
            &view,
            &cache,
            &collect_demands(&view, &cache),
            1,
            None,
            false,
        );
        assert!(visible.error.is_none());
        assert_eq!(visible.directory_errors.len(), 1);
        assert_eq!(visible.directory_errors[0].0, failed);
        assert!(Arc::ptr_eq(&visible.directory_errors[0].1, &message));

        requested.show_hidden = true;
        view.request = Arc::new(requested.clone());
        let revealed = project(
            &view,
            &cache,
            &collect_demands(&view, &cache),
            2,
            None,
            false,
        );
        assert_eq!(revealed.directory_errors.len(), 2);
        assert!(
            revealed
                .directory_errors
                .iter()
                .any(|(path, error)| { path == &hidden && Arc::ptr_eq(error, &hidden_message) })
        );
        requested.root_expanded = false;
        view.request = Arc::new(requested.clone());
        let collapsed = project(
            &view,
            &cache,
            &collect_demands(&view, &cache),
            3,
            None,
            false,
        );
        assert!(collapsed.directory_errors.is_empty());

        requested.root_expanded = true;
        view.request = Arc::new(requested);
        let root_message: Arc<str> = Arc::from("Cannot read root folder: permission denied");
        cache.get_mut(&tree.0).unwrap().state =
            FileTreeDirectoryState::Failed(root_message.clone());
        let root_failed = project(
            &view,
            &cache,
            &collect_demands(&view, &cache),
            4,
            None,
            false,
        );
        assert_eq!(root_failed.error.as_deref(), Some(root_message.as_ref()));
        assert!(root_failed.directory_errors.is_empty());
        assert_eq!(root_failed.rows.len(), 1);
    }

    #[test]
    fn loading_keeps_last_baseline_until_a_completed_read_asserts_removal() {
        let tree = TempTree::new();
        let branch = tree.0.join("branch");
        let stable = tree.0.join("stable");
        let secret = tree.0.join(".secret");
        let leaf = branch.join("leaf");
        fs::create_dir(&branch).unwrap();
        for path in [&stable, &secret, &leaf] {
            fs::write(path, b"").unwrap();
        }
        let mut cache = BTreeMap::new();
        for path in [&tree.0, &branch] {
            let mut directory = CachedDirectory {
                entries: read_directory(path, || false).unwrap(),
                state: FileTreeDirectoryState::Ready,
                failure_kind: None,
                used: 1,
            };
            directory.invalidate(true);
            directory.state = FileTreeDirectoryState::Loading;
            cache.insert(path.clone(), directory);
        }
        let mut requested = request(Entity::from_raw_u32(1).unwrap(), &tree.0);
        requested.expanded = Arc::new(BTreeSet::from([branch.clone()]));
        let mut view = WorkerView {
            request: Arc::new(requested.clone()),
            serial: 1,
            refresh: 0,
            input_absolute: tree.0.clone(),
            root: tree.0.clone(),
            error: None,
            retry_missing: false,
        };
        let demand = collect_demands(&view, &cache);
        assert_eq!(demand, BTreeSet::from([tree.0.clone(), branch.clone()]));
        let loading = project(&view, &cache, &demand, 1, None, false);
        assert!(loading.loading);
        assert!(loading.row_index(&stable).is_some());
        assert!(loading.row_index(&leaf).is_some());
        assert!(loading.row_index(&secret).is_none());
        assert_eq!(
            loading.rows[0].directory_state,
            FileTreeDirectoryState::Loading
        );

        requested.expanded = Arc::default();
        view.request = Arc::new(requested.clone());
        let collapsed = project(
            &view,
            &cache,
            &collect_demands(&view, &cache),
            2,
            None,
            false,
        );
        assert!(collapsed.row_index(&branch).is_some());
        assert!(collapsed.row_index(&leaf).is_none());
        assert!(collapsed.row_index(&secret).is_none());
        requested.expanded = Arc::new(BTreeSet::from([branch.clone()]));
        requested.show_hidden = true;
        view.request = Arc::new(requested);
        let revealed = project(&view, &cache, &demand, 3, None, false);
        assert!(revealed.rows[revealed.row_index(&secret).unwrap()].hidden);

        fs::remove_file(&stable).unwrap();
        // A notification/invalidation alone must not claim disappearance.
        assert!(
            project(&view, &cache, &demand, 4, None, false)
                .row_index(&stable)
                .is_some()
        );
        let root = cache.get_mut(&tree.0).unwrap();
        root.entries = read_directory(&tree.0, || false).unwrap();
        root.state = FileTreeDirectoryState::Ready;
        let completed = project(&view, &cache, &demand, 5, None, false);
        assert!(completed.row_index(&stable).is_none());
        assert!(completed.row_index(&leaf).is_some());
    }

    #[test]
    fn retirement_handoff_is_bounded_and_destruction_runs_outside_main_and_lock() {
        let mail = Arc::new(Mutex::new(Mailbox::default()));
        let retirement = SnapshotRetirementHandle {
            mailbox: mail.clone(),
        };
        let snapshot = Arc::new(FileTreeSnapshot::default());
        let weak = Arc::downgrade(&snapshot);
        retirement.retire_snapshot(snapshot).unwrap();
        for version in 1..MAX_RETIRED_SNAPSHOTS {
            retirement
                .retire_snapshot(Arc::new(FileTreeSnapshot {
                    version: version as u64,
                    ..Default::default()
                }))
                .unwrap();
        }
        // Duplicate ownership transfer uses no additional slot and cannot free
        // the rows on main while the queued strong reference still exists.
        retirement.retire_snapshot(weak.upgrade().unwrap()).unwrap();
        let overflow = Arc::new(FileTreeSnapshot::default());
        let returned = retirement.retire_snapshot(overflow.clone()).unwrap_err();
        assert!(Arc::ptr_eq(&overflow, &returned));
        assert_eq!(
            mail.lock().unwrap().retired_snapshots.len(),
            MAX_RETIRED_SNAPSHOTS
        );

        let mut scope = ManagedWork::default();
        let worker_mail = mail.clone();
        let task = scope
            .spawn_task(None, "Test snapshot retirement", move |_| {
                drain_retired_snapshots(&worker_mail);
                std::thread::current().id()
            })
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(8);
        let worker = loop {
            if let Ok(worker) = task.try_recv() {
                break worker;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_ne!(worker, std::thread::current().id());
        assert!(weak.upgrade().is_none());
        assert!(mail.lock().unwrap().retired_snapshots.is_empty());
        mail.lock().unwrap().retirement_closed = true;
        assert!(
            retirement
                .retire_snapshot(Arc::new(FileTreeSnapshot::default()))
                .is_err()
        );
    }

    #[test]
    fn cancellation_finishes_retirement_without_waiting_for_surviving_handles() {
        let mut scope = ManagedWork::default();
        let mut backend = Backend::spawn(&mut scope).unwrap();
        let retirement = backend.snapshot_retirement_handle();
        let snapshot = Arc::new(FileTreeSnapshot::default());
        let weak = Arc::downgrade(&snapshot);
        backend.retire_snapshot(snapshot).unwrap();
        backend.shutdown();
        let deadline = Instant::now() + Duration::from_secs(8);
        while matches!(
            backend.status(),
            WorkStatus::Running | WorkStatus::Cancelling
        ) {
            assert!(
                Instant::now() < deadline,
                "Worker waited for a surviving retirement handle"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(backend.status(), WorkStatus::Cancelled);
        assert!(weak.upgrade().is_none());
        assert!(
            retirement
                .retire_snapshot(Arc::new(FileTreeSnapshot::default()))
                .is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn permission_denied_root_is_an_explicit_failed_snapshot() {
        use std::os::unix::fs::PermissionsExt;
        // A privileged test process bypasses these permissions. The portable
        // failure/retry-state test above still runs in that environment.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let tree = TempTree::new();
        let original = fs::metadata(&tree.0).unwrap().permissions();
        struct RestorePermissions(PathBuf, fs::Permissions);
        impl Drop for RestorePermissions {
            fn drop(&mut self) {
                let _ = fs::set_permissions(&self.0, self.1.clone());
            }
        }
        let _restore = RestorePermissions(tree.0.clone(), original);
        fs::set_permissions(&tree.0, fs::Permissions::from_mode(0o0)).unwrap();
        let mut scope = ManagedWork::default();
        let mut backend = Backend::spawn(&mut scope).unwrap();
        let panel = Entity::from_raw_u32(1).unwrap();
        backend.set_view(request(panel, &tree.0)).unwrap();
        let failed = wait_snapshot(&backend, panel, |snapshot| snapshot.error.is_some());
        assert!(matches!(
            failed.rows[0].directory_state,
            FileTreeDirectoryState::Failed(_)
        ));
        assert!(!failed.loading);
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_rows_not_traversal_and_native_non_utf8_names_survive() {
        use std::os::unix::{ffi::OsStringExt, fs::symlink};
        let tree = TempTree::new();
        fs::create_dir(tree.0.join("real")).unwrap();
        fs::write(tree.0.join("real/leaf"), b"").unwrap();
        symlink(&tree.0, tree.0.join("loop")).unwrap();
        let native_name = OsString::from_vec(vec![b'x', 0xff]);
        // APFS rejects invalid UTF-8 names (EILSEQ). Keep exercising symlinks,
        // canonical roots and filters there; verify byte-preserving rows on
        // filesystems which actually admit such a native name.
        let native_name_supported = match fs::write(tree.0.join(&native_name), b"") {
            Ok(()) => true,
            Err(error)
                if error.raw_os_error() == Some(libc::EILSEQ)
                    || error.kind() == io::ErrorKind::InvalidInput =>
            {
                false
            }
            Err(error) => panic!("Cannot create native filename fixture: {error}"),
        };
        fs::write(tree.0.join(".secret"), b"").unwrap();
        let mut scope = ManagedWork::default();
        let mut backend = Backend::spawn(&mut scope).unwrap();
        let panel = Entity::from_raw_u32(1).unwrap();
        // A supplied root alias is canonicalized on the worker, while symlink
        // entries under that canonical root remain links and cannot be expanded.
        let mut view = request(panel, &tree.0.join("loop"));
        view.expanded = Arc::new(BTreeSet::from([tree.0.join("loop")]));
        backend.set_view(view.clone()).unwrap();
        let snapshot = wait_snapshot(&backend, panel, ready);
        assert_eq!(snapshot.root, tree.0);
        let link = &snapshot.rows[snapshot.row_index(&tree.0.join("loop")).unwrap()];
        assert_eq!(link.kind, FileTreeEntryKind::Symlink);
        assert!(read_directory(&tree.0.join("loop"), || false).is_err());
        assert_eq!(snapshot.cached_directories, 1);
        assert!(snapshot.row_index(&tree.0.join(".secret")).is_none());
        if native_name_supported {
            let native = &snapshot.rows[snapshot.row_index(&tree.0.join(&native_name)).unwrap()];
            assert_eq!(native.name, native_name);
            assert!(native.label.contains('\u{fffd}'));
        }
        view.revision = 2;
        view.show_hidden = true;
        backend.set_view(view).unwrap();
        let revealed = wait_snapshot(&backend, panel, |snapshot| {
            snapshot.revision == 2 && ready(snapshot)
        });
        assert_eq!(revealed.cached_directories, 1);
        assert!(revealed.rows[revealed.row_index(&tree.0.join(".secret")).unwrap()].hidden);
    }
}
