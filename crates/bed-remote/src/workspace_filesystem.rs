//! Incremental workspace discovery shared by the finder, tree, and document host.
//! All filesystem I/O, Git classification, and watcher maintenance run off the UI.
use crate::{
    DirectoryEntry, DirectoryListing, FilesystemChange, RemoteClient, Request, Response,
    WorkspaceUpdate,
};
use notify_debouncer_full::{
    DebounceEventResult, Debouncer, NoCache, new_debouncer_opt,
    notify::{
        EventKind, RecommendedWatcher, RecursiveMode,
        event::{ModifyKind, RenameMode},
    },
};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque},
    fs, io,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, UNIX_EPOCH},
};

const FALLBACK_INTERVAL: Duration = Duration::from_secs(30);
const REMOTE_POLL_INTERVAL: Duration = Duration::from_millis(200);
fn private_temporary(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with(".bed-transfer-") || name.starts_with(".bed-save-"))
}
fn directory_within_workspace(root: &Path, path: &Path) -> bool {
    if !path.starts_with(root)
        || path
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return false;
    }
    // Missing children may be requested before a creation finishes. Validate
    // their nearest existing ancestor without following links out of the root.
    for ancestor in path.ancestors() {
        match fs::canonicalize(ancestor) {
            Ok(resolved) => return resolved.starts_with(root),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(_) => return false,
        }
    }
    false
}

enum Command {
    Subscribe(Sender<WorkspaceUpdate>),
    Directory(String),
    RefreshDirectory(String),
    Refresh,
    RefreshOptions(bool),
    IncludeIgnored(bool),
    Events(DebounceEventResult),
    Stop,
}

struct Inner {
    sender: Sender<Command>,
    stop: Arc<AtomicBool>,
    worker: Mutex<Option<JoinHandle<()>>>,
}
impl Drop for Inner {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = self.sender.send(Command::Stop);
        if let Some(worker) = self.worker.get_mut().unwrap().take() {
            let deadline = Instant::now() + Duration::from_millis(100);
            while !worker.is_finished() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(2));
            }
            if worker.is_finished() {
                let _ = worker.join();
            }
        }
    }
}

/// Cheap shared handle. Cloning it never creates another scanner or watcher.
#[derive(Clone)]
pub struct WorkspaceFilesystem(Arc<Inner>);
impl WorkspaceFilesystem {
    pub fn local(root: String, include_ignored: bool) -> Self {
        Self::start(root, include_ignored, None)
    }
    pub fn remote(root: String, include_ignored: bool, client: RemoteClient) -> Self {
        Self::start(root, include_ignored, Some(client))
    }
    fn start(root: String, include_ignored: bool, remote: Option<RemoteClient>) -> Self {
        let (sender, requests) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let events = sender.clone();
        let worker = thread::spawn(move || {
            if let Some(client) = remote {
                remote_worker(root, include_ignored, client, requests, worker_stop);
            } else {
                local_worker(root, include_ignored, events, requests, worker_stop);
            }
        });
        Self(Arc::new(Inner {
            sender,
            stop,
            worker: Mutex::new(Some(worker)),
        }))
    }
    pub fn subscribe(&self) -> Receiver<WorkspaceUpdate> {
        let (sender, receiver) = mpsc::channel();
        let _ = self.0.sender.send(Command::Subscribe(sender));
        receiver
    }
    pub fn request_directory(&self, path: impl Into<String>) {
        let _ = self.0.sender.send(Command::Directory(path.into()));
    }
    pub fn refresh_directory(&self, path: impl Into<String>) {
        let _ = self.0.sender.send(Command::RefreshDirectory(path.into()));
    }
    pub fn refresh(&self) {
        let _ = self.0.sender.send(Command::Refresh);
    }
    pub fn set_include_ignored(&self, include: bool) {
        let _ = self.0.sender.send(Command::IncludeIgnored(include));
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Stamp {
    len: u64,
    modified: Option<Duration>,
    identity: (u64, u64),
}
fn stamp(metadata: &fs::Metadata) -> Stamp {
    #[cfg(unix)]
    let identity = {
        use std::os::unix::fs::MetadataExt;
        (metadata.dev(), metadata.ino())
    };
    #[cfg(not(unix))]
    let identity = (0, 0);
    Stamp {
        len: metadata.len(),
        modified: metadata
            .modified()
            .ok()
            .and_then(|m| m.duration_since(UNIX_EPOCH).ok()),
        identity,
    }
}

struct NativeState {
    root: PathBuf,
    include_ignored: bool,
    repository: Option<git2::Repository>,
    workdir: Option<PathBuf>,
    tracked: BTreeSet<PathBuf>,
    ignored_paths: BTreeSet<PathBuf>,
    ignore_sources: BTreeSet<PathBuf>,
    files: BTreeMap<PathBuf, Stamp>,
    previous_files: BTreeMap<PathBuf, Option<Stamp>>,
    directory_identities: BTreeMap<PathBuf, Stamp>,
    previous_directories: BTreeMap<PathBuf, Option<Stamp>>,
    directory_files: BTreeMap<PathBuf, BTreeSet<PathBuf>>,
    directory_children: BTreeMap<PathBuf, BTreeSet<PathBuf>>,
    indexed_directories: BTreeSet<PathBuf>,
    requested: BTreeSet<PathBuf>,
    listings: BTreeMap<PathBuf, DirectoryListing>,
    watcher: Option<Debouncer<RecommendedWatcher, NoCache>>,
    #[cfg(target_os = "macos")]
    auxiliary_watchers: BTreeMap<PathBuf, Debouncer<RecommendedWatcher, NoCache>>,
    #[cfg(target_os = "macos")]
    event_sender: Sender<Command>,
    watched: BTreeSet<PathBuf>,
    degraded: Option<String>,
    generation: u64,
    stop: Arc<AtomicBool>,
}
impl NativeState {
    fn set_file(&mut self, path: PathBuf, current: Stamp) {
        if self.files.get(&path) == Some(&current) {
            return;
        }
        self.previous_files
            .entry(path.clone())
            .or_insert_with(|| self.files.get(&path).cloned());
        self.files.insert(path, current);
    }
    fn remove_file(&mut self, path: &Path) {
        if let Some(previous) = self.files.remove(path) {
            self.previous_files
                .entry(path.to_owned())
                .or_insert(Some(previous));
        }
    }
    fn changed_files(&self) -> (BTreeMap<PathBuf, Stamp>, BTreeMap<PathBuf, Stamp>) {
        let previous = self
            .previous_files
            .iter()
            .filter_map(|(path, previous)| previous.clone().map(|stamp| (path.clone(), stamp)))
            .collect();
        let current = self
            .previous_files
            .keys()
            .filter_map(|path| {
                self.files
                    .get(path)
                    .map(|stamp| (path.clone(), stamp.clone()))
            })
            .collect();
        (previous, current)
    }
    fn set_directory_identity(&mut self, path: &Path) {
        let Ok(metadata) = fs::symlink_metadata(path) else {
            return;
        };
        if !metadata.is_dir() {
            return;
        }
        let current = Stamp {
            len: 0,
            modified: None,
            identity: stamp(&metadata).identity,
        };
        if self.directory_identities.get(path) == Some(&current) {
            return;
        }
        self.previous_directories
            .entry(path.to_owned())
            .or_insert_with(|| self.directory_identities.get(path).cloned());
        self.directory_identities.insert(path.to_owned(), current);
    }
    fn directory_renames(&self) -> Vec<FilesystemChange> {
        let previous = self
            .previous_directories
            .iter()
            .filter_map(|(path, previous)| previous.clone().map(|stamp| (path.clone(), stamp)))
            .collect();
        let current = self
            .previous_directories
            .keys()
            .filter_map(|path| {
                self.directory_identities
                    .get(path)
                    .map(|stamp| (path.clone(), stamp.clone()))
            })
            .collect();
        let mut roots = BTreeSet::new();
        diff_changes(&previous, &current)
            .into_iter()
            .filter(|change| {
                if let FilesystemChange::Renamed { from, .. } = change {
                    let path = PathBuf::from(from);
                    if within_any(&path, &roots) {
                        false
                    } else {
                        roots.insert(path);
                        true
                    }
                } else {
                    false
                }
            })
            .collect()
    }
    fn update(&mut self) -> WorkspaceUpdate {
        self.generation = self.generation.wrapping_add(1);
        WorkspaceUpdate {
            root: self.root.to_string_lossy().into_owned(),
            generation: self.generation,
            degraded: self.degraded.clone(),
            ready: true,
            ..Default::default()
        }
    }
    fn snapshot(&self) -> WorkspaceUpdate {
        WorkspaceUpdate {
            root: self.root.to_string_lossy().into_owned(),
            generation: self.generation,
            directories: self.listings.values().cloned().collect(),
            indexed_files: Some(
                self.files
                    .keys()
                    .map(|p| p.to_string_lossy().into_owned())
                    .collect(),
            ),
            degraded: self.degraded.clone(),
            ready: true,
            ..Default::default()
        }
    }
    fn watch(&mut self, path: &Path) {
        if self.watched.contains(path) {
            return;
        }
        // FSEvents monitors a directory tree without scanning. Use one stable
        // stream: repeatedly registering individual children restarts the
        // native run loop, creates event gaps, and can stall under load.
        #[cfg(target_os = "macos")]
        let path = {
            if !self.watched.is_empty() {
                // Global Git excludes can live outside the project stream.
                // Register a separate stable stream for that small directory.
                if !self.watched.iter().any(|root| path.starts_with(root))
                    && !self.auxiliary_watchers.contains_key(path)
                {
                    let sender = self.event_sender.clone();
                    let auxiliary = new_debouncer_opt::<_, RecommendedWatcher, NoCache>(
                        Duration::from_millis(180),
                        Some(Duration::from_millis(40)),
                        move |events| {
                            let _ = sender.send(Command::Events(events));
                        },
                        NoCache::new(),
                        notify_debouncer_full::notify::Config::default(),
                    );
                    match auxiliary {
                        Ok(mut watcher) => match watcher.watch(path, RecursiveMode::NonRecursive) {
                            Ok(()) => {
                                self.auxiliary_watchers.insert(path.to_owned(), watcher);
                            }
                            Err(error) => {
                                self.degraded = Some(format!(
                                    "Filesystem watching is degraded: {error}. Reconciling every 30 seconds."
                                ))
                            }
                        },
                        Err(error) => {
                            self.degraded = Some(format!(
                                "Filesystem watching is degraded: {error}. Reconciling every 30 seconds."
                            ))
                        }
                    }
                }
                return;
            }
            let parent = self.root.parent().unwrap_or(&self.root);
            self.workdir
                .as_deref()
                .filter(|workdir| parent.starts_with(workdir))
                .map_or(parent, |workdir| workdir)
        };
        if let Some(watcher) = &mut self.watcher {
            let mode = if cfg!(target_os = "macos") {
                RecursiveMode::Recursive
            } else {
                RecursiveMode::NonRecursive
            };
            match watcher.watch(path, mode) {
                Ok(()) => {
                    self.watched.insert(path.to_owned());
                }
                Err(error) => {
                    self.degraded = Some(format!(
                        "Filesystem watching is degraded: {error}. Reconciling every 30 seconds."
                    ))
                }
            }
        }
    }
    fn git_metadata(&self, path: &Path) -> bool {
        path.strip_prefix(&self.root)
            .is_ok_and(|relative| relative.components().any(|p| p.as_os_str() == ".git"))
    }
    fn ignored(&self, path: &Path) -> bool {
        if self.git_metadata(path) {
            return true;
        }
        if self.include_ignored {
            return false;
        }
        self.gitignored(path)
    }
    fn gitignored(&self, path: &Path) -> bool {
        self.ignored_paths.contains(path)
    }
    fn interested_in(&self, path: &Path) -> bool {
        self.include_ignored
            || !path
                .ancestors()
                .any(|ancestor| self.ignored_paths.contains(ancestor))
            || self.requested.contains(path)
            || path
                .parent()
                .is_some_and(|parent| self.requested.contains(parent))
    }
    fn refresh_repository(&mut self) {
        self.repository = git2::Repository::discover(&self.root).ok();
        self.ignored_paths.clear();
        self.tracked.clear();
        self.ignore_sources.clear();
        self.workdir = self
            .repository
            .as_ref()
            .and_then(|repo| repo.workdir())
            .and_then(|path| fs::canonicalize(path).ok());
        let mut watch_paths = Vec::new();
        if let Some(repo) = &self.repository {
            let info = repo.path().join("info");
            if info.is_dir() {
                watch_paths.push(info.clone());
            }
            watch_paths.push(repo.path().to_owned());
            self.ignore_sources.insert(repo.path().join("index"));
            self.ignore_sources.insert(repo.path().join("config"));
            self.ignore_sources.insert(info.join("exclude"));
            if let (Some(workdir), Ok(index)) = (&self.workdir, repo.index()) {
                for entry in index.iter() {
                    if let Ok(path) = std::str::from_utf8(&entry.path) {
                        let path = workdir.join(path);
                        for ancestor in path
                            .ancestors()
                            .take_while(|path| path.starts_with(workdir))
                        {
                            self.tracked.insert(ancestor.to_owned());
                        }
                    }
                }
                for ancestor in self
                    .root
                    .ancestors()
                    .take_while(|path| path.starts_with(workdir))
                {
                    self.ignore_sources.insert(ancestor.join(".gitignore"));
                    watch_paths.push(ancestor.to_owned());
                }
            }
            if let Ok(config) = repo.config()
                && let Ok(path) = config.get_path("core.excludesfile")
            {
                if let Some(parent) = path.parent() {
                    watch_paths.push(parent.to_owned());
                }
                self.ignore_sources.insert(path);
            }
        }
        for path in watch_paths {
            if path.is_dir() {
                self.watch(&path);
            }
        }
    }
    fn listing(&mut self, path: &Path) -> io::Result<DirectoryListing> {
        if !directory_within_workspace(&self.root, path) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Directory escapes workspace root",
            ));
        }
        self.watch(path);
        self.set_directory_identity(path);
        let mut entries = Vec::new();
        for entry in fs::read_dir(path)? {
            if self.stop.load(Ordering::Acquire) {
                return Err(io::ErrorKind::Interrupted.into());
            }
            let entry = entry?;
            if private_temporary(&entry.path()) {
                continue;
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            entries.push(DirectoryEntry {
                path: entry.path().to_string_lossy().into_owned(),
                name,
                is_directory: kind.is_dir() || (kind.is_symlink() && entry.path().is_dir()),
                is_symlink: kind.is_symlink(),
                is_gitignored: false,
            });
        }
        let mut warning = None;
        if let Some(workdir) = &self.workdir {
            let names: Vec<_> = entries
                .iter()
                .map(|entry| {
                    Path::new(&entry.path)
                        .strip_prefix(workdir)
                        .unwrap_or(Path::new(&entry.path))
                        .to_string_lossy()
                        .into_owned()
                })
                .collect();
            match crate::git_ignored_paths(workdir, &names) {
                Ok(ignored) => {
                    for (entry, name) in entries.iter_mut().zip(names) {
                        let entry_path = PathBuf::from(&entry.path);
                        entry.is_gitignored =
                            ignored.contains(&name) && !self.tracked.contains(&entry_path);
                        if entry.is_gitignored {
                            self.ignored_paths.insert(entry_path);
                        } else {
                            self.ignored_paths.remove(&entry_path);
                        }
                    }
                }
                Err(error) => {
                    warning = Some(format!("Git ignore classification failed: {error}"));
                    for entry in &entries {
                        self.ignored_paths.remove(Path::new(&entry.path));
                    }
                }
            }
        }
        entries.sort_by(|a, b| {
            b.is_directory
                .cmp(&a.is_directory)
                .then_with(|| a.name.cmp(&b.name))
        });
        Ok(DirectoryListing {
            path: path.to_string_lossy().into_owned(),
            entries,
            warning,
        })
    }
    /// Read this directory once. Only newly discovered child directories recurse.
    fn reconcile_directory(&mut self, path: &Path, recursive: bool) -> io::Result<()> {
        if self.stop.load(Ordering::Acquire) {
            return Err(io::ErrorKind::Interrupted.into());
        }
        let listing = self.listing(path)?;
        let mut immediate_files = BTreeSet::new();
        let mut immediate_dirs = BTreeSet::new();
        for entry in &listing.entries {
            let child = PathBuf::from(&entry.path);
            if self.ignored(&child) {
                continue;
            }
            let Ok(metadata) = fs::symlink_metadata(&child) else {
                continue;
            };
            if metadata.is_dir() {
                immediate_dirs.insert(child.clone());
                let new = self.indexed_directories.insert(child.clone());
                if (new || recursive)
                    && let Err(error) = self.reconcile_directory(&child, recursive)
                {
                    if error.kind() == io::ErrorKind::Interrupted {
                        return Err(error);
                    }
                    if error.kind() != io::ErrorKind::NotFound {
                        self.degraded = Some(format!(
                            "Could not index {}: {error}. Reconciling every 30 seconds.",
                            child.display()
                        ));
                    }
                }
            } else if metadata.is_file() || (metadata.file_type().is_symlink() && child.is_file()) {
                immediate_files.insert(child.clone());
                self.set_file(child, stamp(&metadata));
            }
        }
        let previous = self
            .directory_files
            .insert(path.to_owned(), immediate_files.clone())
            .unwrap_or_default();
        for removed in previous.difference(&immediate_files) {
            self.remove_file(removed);
        }
        let previous = self
            .directory_children
            .insert(path.to_owned(), immediate_dirs.clone())
            .unwrap_or_default();
        let removed: Vec<_> = previous.difference(&immediate_dirs).cloned().collect();
        for directory in removed {
            self.remove_subtree(&directory);
        }
        if self.requested.contains(path) {
            self.listings.insert(path.to_owned(), listing);
        }
        Ok(())
    }
    fn remove_subtree(&mut self, path: &Path) {
        self.remove_file(path);
        let identities: Vec<_> = self
            .directory_identities
            .range(path.to_owned()..)
            .take_while(|(p, _)| p.starts_with(path))
            .map(|(p, _)| p.clone())
            .collect();
        for directory in identities {
            if let Some(previous) = self.directory_identities.remove(&directory) {
                self.previous_directories
                    .entry(directory)
                    .or_insert(Some(previous));
            }
        }
        let directories: Vec<_> = self
            .directory_files
            .range(path.to_owned()..)
            .take_while(|(p, _)| p.starts_with(path))
            .map(|(p, _)| p.clone())
            .collect();
        for directory in directories {
            if let Some(files) = self.directory_files.remove(&directory) {
                for file in files {
                    self.remove_file(&file);
                }
            }
            self.directory_children.remove(&directory);
            self.indexed_directories.remove(&directory);
            self.listings.remove(&directory);
        }
        let stale: Vec<_> = self
            .watched
            .range(path.to_owned()..)
            .take_while(|p| p.starts_with(path))
            .cloned()
            .collect();
        for directory in stale {
            if let Some(watcher) = &mut self.watcher {
                let _ = watcher.unwatch(&directory);
            }
            self.watched.remove(&directory);
        }
    }
    fn rebuild(&mut self) -> io::Result<WorkspaceUpdate> {
        self.previous_files.clear();
        self.previous_directories.clear();
        self.refresh_repository();
        self.reconcile_directory(&self.root.clone(), true)?;
        // Requested ignored directories remain visible independently of the index.
        for directory in self.requested.clone() {
            if let Ok(listing) = self.listing(&directory) {
                self.listings.insert(directory, listing);
            }
        }
        let mut update = self.update();
        let (previous, current) = self.changed_files();
        update.changes = self.directory_renames();
        let roots = rename_sources(&update.changes);
        update.changes.extend(
            diff_changes(&previous, &current)
                .into_iter()
                .filter(|change| match change {
                    FilesystemChange::Removed { path } | FilesystemChange::Modified { path } => {
                        !within_any(Path::new(path), &roots)
                    }
                    FilesystemChange::Renamed { from, .. } => !within_any(Path::new(from), &roots),
                    _ => true,
                }),
        );
        update.changes.retain(|change| !matches!(change, FilesystemChange::Removed { path } if Path::new(path).exists()));
        prioritize_changes(&mut update.changes);
        update.indexed_files = Some(
            self.files
                .keys()
                .map(|p| p.to_string_lossy().into_owned())
                .collect(),
        );
        update.directories = self.listings.values().cloned().collect();
        update.dirty_directories = update.directories.iter().map(|d| d.path.clone()).collect();
        Ok(update)
    }
    fn events(&mut self, events: DebounceEventResult) -> io::Result<WorkspaceUpdate> {
        let events = match events {
            Ok(events) => events,
            Err(errors) => {
                self.degraded = Some(format!(
                    "Filesystem watcher lost events: {}. Reconciling every 30 seconds.",
                    errors
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join("; ")
                ));
                return self.rebuild();
            }
        };
        if events.iter().any(|event| event.need_rescan()) {
            return self.rebuild();
        }
        self.previous_files.clear();
        self.previous_directories.clear();
        let mut changes = Vec::new();
        let mut dirty = BTreeSet::new();
        let mut ignore_roots = BTreeSet::new();
        for event in events {
            if matches!(event.kind, EventKind::Access(_)) {
                continue;
            }
            if event
                .paths
                .iter()
                .any(|path| self.ignore_sources.contains(path))
            {
                ignore_roots.insert(self.root.clone());
                dirty.insert(self.root.clone());
            }
            if matches!(
                event.kind,
                EventKind::Modify(ModifyKind::Name(RenameMode::Both))
            ) && event.paths.len() == 2
            {
                let from = &event.paths[0];
                let to = &event.paths[1];
                if from.starts_with(&self.root)
                    && to.starts_with(&self.root)
                    && !private_temporary(from)
                    && !private_temporary(to)
                    && !self.git_metadata(from)
                    && !self.git_metadata(to)
                    && !from.exists()
                    && to.exists()
                {
                    changes.push(FilesystemChange::Renamed {
                        from: from.to_string_lossy().into_owned(),
                        to: to.to_string_lossy().into_owned(),
                    });
                }
            }
            let workspace_root = self.root.clone();
            for path in event
                .paths
                .iter()
                .filter(|p| p.starts_with(&workspace_root))
            {
                if private_temporary(path) {
                    continue;
                }
                if self.git_metadata(path) {
                    // Administrative files stay out of document/index deltas,
                    // but an open .git folder still needs fresh tree listings.
                    if self.requested.contains(path) {
                        dirty.insert(path.clone());
                    }
                    if let Some(parent) = path.parent()
                        && self.requested.contains(parent)
                    {
                        dirty.insert(parent.to_owned());
                    }
                    continue;
                }
                if !self.interested_in(path) {
                    continue;
                }
                if path
                    .file_name()
                    .is_some_and(|name| name == ".gitignore" || name == ".gitmodules")
                {
                    ignore_roots.insert(path.parent().unwrap_or(&self.root).to_owned());
                }
                if path.file_name().is_some_and(|name| name == "exclude") {
                    ignore_roots.insert(self.root.clone());
                }
                if let Some(parent) = path.parent()
                    && parent.starts_with(&self.root)
                {
                    dirty.insert(parent.to_owned());
                }
                if path.is_dir() {
                    self.set_directory_identity(path);
                    dirty.insert(path.clone());
                }
                if fs::symlink_metadata(path)
                    .is_err_and(|error| error.kind() == io::ErrorKind::NotFound)
                {
                    self.remove_subtree(path);
                    changes.push(FilesystemChange::Removed {
                        path: path.to_string_lossy().into_owned(),
                    });
                } else if !matches!(event.kind, EventKind::Modify(ModifyKind::Name(_))) {
                    changes.push(if matches!(event.kind, EventKind::Create(_)) {
                        FilesystemChange::Created {
                            path: path.to_string_lossy().into_owned(),
                        }
                    } else {
                        FilesystemChange::Modified {
                            path: path.to_string_lossy().into_owned(),
                        }
                    });
                }
            }
        }
        if !ignore_roots.is_empty() {
            self.refresh_repository();
        }
        for path in &ignore_roots {
            if path.is_dir() {
                self.reconcile_directory(path, true)?;
            }
        }
        for directory in &dirty {
            if !directory.is_dir() {
                continue;
            }
            if !self.ignored(directory) {
                self.reconcile_directory(directory, false)?;
            } else if self.requested.contains(directory) {
                let listing = self.listing(directory)?;
                self.listings.insert(directory.clone(), listing);
            }
        }
        let (previous, current) = self.changed_files();
        let inferred = diff_changes(&previous, &current);
        changes.extend(self.directory_renames());
        let native_rename_roots: BTreeSet<PathBuf> = changes
            .iter()
            .filter_map(|change| match change {
                FilesystemChange::Renamed { from, to } => {
                    Some([PathBuf::from(from), PathBuf::from(to)])
                }
                _ => None,
            })
            .flatten()
            .collect();
        let mut known = HashSet::new();
        changes.retain(|change| known.insert(change.clone()));
        // Native paired renames win over their inferred leaf deletion/creation.
        for change in inferred {
            let path = match &change {
                FilesystemChange::Created { path }
                | FilesystemChange::Modified { path }
                | FilesystemChange::Removed { path } => Path::new(path),
                FilesystemChange::Renamed { from, .. } => Path::new(from),
            };
            if !within_any(path, &native_rename_roots) && known.insert(change.clone()) {
                changes.push(change);
            }
        }
        // A replace-on-save or rename is not a deletion of its surviving path.
        let rename_sources = rename_sources(&changes);
        changes.retain(|change| match change {
            FilesystemChange::Removed { path } => {
                !within_any(Path::new(path), &rename_sources)
                    && fs::symlink_metadata(path)
                        .is_err_and(|error| error.kind() == io::ErrorKind::NotFound)
            }
            _ => true,
        });
        prioritize_changes(&mut changes);
        let mut update = self.update();
        update.changes = changes;
        update.indexed_removed = previous
            .keys()
            .filter(|p| !current.contains_key(*p))
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        update.indexed_added = current
            .keys()
            .filter(|p| !previous.contains_key(*p))
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        update.directories = dirty
            .iter()
            .filter_map(|p| self.listings.get(p).cloned())
            .collect();
        // Rule changes can modify cached descendants even when only their parent fired.
        update.directories.extend(
            self.listings
                .values()
                .filter(|listing| {
                    ignore_roots
                        .iter()
                        .any(|root| Path::new(&listing.path).starts_with(root))
                        && !dirty.contains(Path::new(&listing.path))
                })
                .cloned(),
        );
        update.dirty_directories = dirty
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        Ok(update)
    }
    fn relevant_events(&self, events: &DebounceEventResult) -> bool {
        match events {
            Err(_) => true,
            Ok(events) => events.iter().any(|event| {
                event.need_rescan()
                    || (!matches!(event.kind, EventKind::Access(_))
                        && event.paths.iter().any(|path| {
                            self.ignore_sources.contains(path)
                                || (path.starts_with(&self.root)
                                    && !private_temporary(path)
                                    && self.interested_in(path)
                                    && (!self.git_metadata(path)
                                        || self.requested.contains(path)
                                        || path
                                            .parent()
                                            .is_some_and(|parent| self.requested.contains(parent))))
                        }))
            }),
        }
    }
}

fn rename_sources(changes: &[FilesystemChange]) -> BTreeSet<PathBuf> {
    changes
        .iter()
        .filter_map(|c| {
            if let FilesystemChange::Renamed { from, .. } = c {
                Some(PathBuf::from(from))
            } else {
                None
            }
        })
        .collect()
}
fn prioritize_changes(changes: &mut [FilesystemChange]) {
    // A source directory can disappear after its files were relocated into
    // an existing destination. Rebind surviving documents before processing
    // removal of that now-empty source directory.
    changes.sort_by_key(|change| match change {
        FilesystemChange::Renamed { .. } => 0,
        FilesystemChange::Removed { .. } => 1,
        FilesystemChange::Modified { .. } => 2,
        FilesystemChange::Created { .. } => 3,
    });
}
fn within_any(path: &Path, roots: &BTreeSet<PathBuf>) -> bool {
    path.ancestors().any(|ancestor| roots.contains(ancestor))
}

fn diff_changes(
    old: &BTreeMap<PathBuf, Stamp>,
    new: &BTreeMap<PathBuf, Stamp>,
) -> Vec<FilesystemChange> {
    let removed: Vec<_> = old.iter().filter(|(p, _)| !new.contains_key(*p)).collect();
    let added: Vec<_> = new.iter().filter(|(p, _)| !old.contains_key(*p)).collect();
    let mut added_by_identity = HashMap::new();
    let mut removed_counts = HashMap::new();
    for (_, previous) in &removed {
        *removed_counts.entry(previous.identity).or_insert(0_usize) += 1;
    }
    for (path, current) in &added {
        added_by_identity
            .entry(current.identity)
            .and_modify(|entry| *entry = None)
            .or_insert(Some(*path));
    }
    let mut paired = BTreeSet::new();
    let mut changes = Vec::new();
    for (path, previous) in removed {
        let target = (previous.identity != (0, 0)
            && removed_counts.get(&previous.identity) == Some(&1))
        .then(|| added_by_identity.get(&previous.identity).copied().flatten())
        .flatten();
        if let Some(to) = target {
            paired.insert(to.clone());
            changes.push(FilesystemChange::Renamed {
                from: path.to_string_lossy().into_owned(),
                to: to.to_string_lossy().into_owned(),
            });
        } else {
            changes.push(FilesystemChange::Removed {
                path: path.to_string_lossy().into_owned(),
            });
        }
    }
    for (path, _) in added {
        if !paired.contains(path) {
            changes.push(FilesystemChange::Created {
                path: path.to_string_lossy().into_owned(),
            });
        }
    }
    for (path, current) in new {
        if old.get(path).is_some_and(|previous| previous != current) {
            changes.push(FilesystemChange::Modified {
                path: path.to_string_lossy().into_owned(),
            });
        }
    }
    changes
}

fn publish(subscribers: &mut Vec<Sender<WorkspaceUpdate>>, update: WorkspaceUpdate) {
    subscribers.retain(|subscriber| subscriber.send(update.clone()).is_ok());
}
fn coalesce_events(
    mut events: DebounceEventResult,
    requests: &Receiver<Command>,
    deferred: &mut VecDeque<Command>,
    stop: &AtomicBool,
) -> DebounceEventResult {
    // The native debouncer drains paths on separate 40 ms ticks. Keep both
    // sides of a cross-directory rename together if they straddle a tick.
    let deadline = Instant::now() + Duration::from_millis(60);
    while Instant::now() < deadline && !stop.load(Ordering::Acquire) {
        match requests.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(Command::Events(next)) => match (&mut events, next) {
                (Ok(current), Ok(next)) => current.extend(next),
                (Err(current), Err(next)) => current.extend(next),
                (current @ Ok(_), Err(next)) => *current = Err(next),
                (Err(_), Ok(_)) => {}
            },
            Ok(Command::Stop) => {
                stop.store(true, Ordering::Release);
                break;
            }
            Ok(command) => deferred.push_back(command),
            Err(_) => break,
        }
    }
    events
}
fn local_worker(
    root: String,
    include_ignored: bool,
    sender: Sender<Command>,
    requests: Receiver<Command>,
    stop: Arc<AtomicBool>,
) {
    let root_path = match fs::canonicalize(&root) {
        Ok(path) => path,
        Err(error) => {
            let update = WorkspaceUpdate {
                root,
                degraded: Some(error.to_string()),
                ..Default::default()
            };
            while let Ok(Command::Subscribe(subscriber)) = requests.recv() {
                let _ = subscriber.send(update.clone());
            }
            return;
        }
    };
    #[cfg(target_os = "macos")]
    let event_sender = sender.clone();
    let watcher = new_debouncer_opt::<_, RecommendedWatcher, NoCache>(
        Duration::from_millis(180),
        Some(Duration::from_millis(40)),
        move |events| {
            let _ = sender.send(Command::Events(events));
        },
        NoCache::new(),
        notify_debouncer_full::notify::Config::default(),
    );
    let degraded = watcher.as_ref().err().map(|error| {
        format!("Filesystem watching is unavailable: {error}. Reconciling every 30 seconds.")
    });
    let mut state = NativeState {
        root: root_path.clone(),
        include_ignored,
        repository: None,
        workdir: None,
        tracked: BTreeSet::new(),
        ignored_paths: BTreeSet::new(),
        ignore_sources: BTreeSet::new(),
        files: BTreeMap::new(),
        previous_files: BTreeMap::new(),
        directory_identities: BTreeMap::new(),
        previous_directories: BTreeMap::new(),
        directory_files: BTreeMap::new(),
        directory_children: BTreeMap::new(),
        indexed_directories: BTreeSet::from([root_path.clone()]),
        requested: BTreeSet::from([root_path]),
        listings: BTreeMap::new(),
        watcher: watcher.ok(),
        #[cfg(target_os = "macos")]
        auxiliary_watchers: BTreeMap::new(),
        #[cfg(target_os = "macos")]
        event_sender,
        watched: BTreeSet::new(),
        degraded,
        generation: 0,
        stop: stop.clone(),
    };
    #[cfg(not(target_os = "macos"))]
    if let Some(parent) = state.root.parent().map(Path::to_path_buf) {
        state.watch(&parent);
    }
    // Register root before walking, and each child before reading it. Queued
    // events reconcile changes that raced with the initial background index.
    let initial = state.rebuild();
    let mut subscribers = Vec::new();
    let mut deferred = VecDeque::new();
    let mut last_reconcile = Instant::now();
    if let Err(error) = initial {
        state.degraded = Some(error.to_string());
    }
    while !stop.load(Ordering::Acquire) {
        let command = match deferred
            .pop_front()
            .map(Ok)
            .unwrap_or_else(|| requests.recv_timeout(Duration::from_millis(100)))
        {
            Ok(command) => Some(command),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(_) => break,
        };
        let update = match command {
            Some(Command::Stop) => break,
            _ if state.degraded.is_some() && last_reconcile.elapsed() >= FALLBACK_INTERVAL => {
                if let Some(command) = command {
                    deferred.push_front(command);
                }
                Some(state.rebuild())
            }
            Some(Command::Subscribe(subscriber)) => {
                let _ = subscriber.send(state.snapshot());
                subscribers.push(subscriber);
                None
            }
            Some(Command::Directory(path)) => {
                let path = PathBuf::from(path);
                if !directory_within_workspace(&state.root, &path)
                    || !state.requested.insert(path.clone())
                {
                    None
                } else if let Ok(listing) = state.listing(&path) {
                    state.listings.insert(path, listing.clone());
                    let mut update = state.update();
                    update.directories.push(listing);
                    Some(Ok(update))
                } else {
                    None
                }
            }
            Some(Command::Refresh) => Some(state.rebuild()),
            Some(Command::RefreshOptions(include)) => {
                state.include_ignored = include;
                Some(state.rebuild())
            }
            Some(Command::RefreshDirectory(path)) => {
                let path = PathBuf::from(path);
                if !directory_within_workspace(&state.root, &path) {
                    None
                } else {
                    state.requested.insert(path.clone());
                    let event = notify_debouncer_full::notify::Event::new(EventKind::Modify(
                        ModifyKind::Any,
                    ))
                    .add_path(path);
                    Some(
                        state.events(Ok(vec![notify_debouncer_full::DebouncedEvent::new(
                            event,
                            Instant::now(),
                        )])),
                    )
                }
            }
            Some(Command::IncludeIgnored(include)) if state.include_ignored != include => {
                state.include_ignored = include;
                Some(state.rebuild())
            }
            Some(Command::Events(events)) if state.relevant_events(&events) => {
                let events = coalesce_events(events, &requests, &mut deferred, &stop);
                if stop.load(Ordering::Acquire) {
                    None
                } else {
                    Some(state.events(events))
                }
            }
            _ => None,
        };
        if let Some(update) = update {
            match update {
                Ok(update) => {
                    if update.indexed_files.is_some() {
                        last_reconcile = Instant::now();
                    }
                    publish(&mut subscribers, update);
                }
                Err(error) if error.kind() != io::ErrorKind::Interrupted => {
                    last_reconcile = Instant::now();
                    state.degraded = Some(error.to_string());
                    let update = state.update();
                    publish(&mut subscribers, update);
                }
                _ => {}
            }
        }
    }
}

fn remote_worker(
    root: String,
    mut include_ignored: bool,
    client: RemoteClient,
    requests: Receiver<Command>,
    stop: Arc<AtomicBool>,
) {
    let mut subscribers = Vec::new();
    let mut latest = WorkspaceUpdate {
        root: root.clone(),
        ..Default::default()
    };
    let mut watch_id = None;
    let mut requested = BTreeSet::new();
    let mut indexed_files = BTreeSet::new();
    let mut next_poll = Instant::now();
    while !stop.load(Ordering::Acquire) {
        match requests.recv_timeout(Duration::from_millis(50)) {
            Ok(Command::Subscribe(subscriber)) => {
                if latest.ready {
                    let mut snapshot = latest.clone();
                    snapshot.indexed_files = Some(indexed_files.iter().cloned().collect());
                    let _ = subscriber.send(snapshot);
                }
                subscribers.push(subscriber);
            }
            Ok(Command::Directory(path)) => {
                if requested.insert(path.clone())
                    && let Some(watch_id) = watch_id
                {
                    let _ = client.call(Request::WatchDirectory { watch_id, path });
                }
            }
            Ok(Command::Refresh) => {
                if let Some(watch_id) = watch_id {
                    let _ = client.call(Request::RefreshWorkspace {
                        watch_id,
                        include_ignored,
                    });
                }
            }
            Ok(Command::IncludeIgnored(include)) => {
                include_ignored = include;
                if let Some(watch_id) = watch_id {
                    let _ = client.call(Request::RefreshWorkspace {
                        watch_id,
                        include_ignored,
                    });
                }
            }
            Ok(Command::RefreshDirectory(path)) => {
                requested.insert(path.clone());
                if let Some(watch_id) = watch_id {
                    let _ = client.call(Request::RefreshWorkspaceDirectory { watch_id, path });
                }
            }
            Ok(Command::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            _ => {}
        }
        if Instant::now() < next_poll {
            continue;
        }
        next_poll = Instant::now() + REMOTE_POLL_INTERVAL;
        if watch_id.is_none() {
            match client.call(Request::WatchWorkspace {
                root: root.clone(),
                include_ignored,
            }) {
                Ok(Response::WorkspaceWatch { watch_id: id }) => {
                    watch_id = Some(id);
                    for path in &requested {
                        let _ = client.call(Request::WatchDirectory {
                            watch_id: id,
                            path: path.clone(),
                        });
                    }
                }
                Err(error) => {
                    latest.degraded = Some(format!("Workspace discovery disconnected: {error}"));
                    publish(&mut subscribers, latest.clone());
                    next_poll = Instant::now() + Duration::from_secs(2);
                    continue;
                }
                _ => continue,
            }
        }
        match client.call(Request::PollWorkspace {
            watch_id: watch_id.unwrap(),
        }) {
            Ok(Response::WorkspaceUpdates { updates }) => {
                for update in updates {
                    // Keep a current snapshot for late subscribers without any disk walk.
                    if let Some(files) = &update.indexed_files {
                        indexed_files = files.iter().cloned().collect();
                    } else {
                        for path in &update.indexed_removed {
                            indexed_files.remove(path);
                        }
                        indexed_files.extend(update.indexed_added.iter().cloned());
                    }
                    for directory in &update.directories {
                        latest.directories.retain(|d| d.path != directory.path);
                        latest.directories.push(directory.clone());
                    }
                    latest.root = update.root.clone();
                    latest.generation = update.generation;
                    latest.ready = update.ready;
                    latest.degraded = update.degraded.clone();
                    publish(&mut subscribers, update);
                }
            }
            Err(error) => {
                watch_id = None;
                latest.degraded = Some(format!("Workspace discovery disconnected: {error}"));
                publish(
                    &mut subscribers,
                    WorkspaceUpdate {
                        root: root.clone(),
                        degraded: latest.degraded.clone(),
                        ..Default::default()
                    },
                );
                next_poll = Instant::now() + Duration::from_secs(2);
            }
            _ => {}
        }
    }
    if let Some(watch_id) = watch_id {
        let _ = client.call(Request::UnwatchWorkspace { watch_id });
    }
}

struct RemoteWatch {
    filesystem: WorkspaceFilesystem,
    updates: Receiver<WorkspaceUpdate>,
}
fn watches() -> &'static Mutex<HashMap<u64, RemoteWatch>> {
    static WATCHES: OnceLock<Mutex<HashMap<u64, RemoteWatch>>> = OnceLock::new();
    WATCHES.get_or_init(Mutex::default)
}

pub(crate) fn call(request: Request) -> Result<Response, crate::RemoteError> {
    use crate::{ErrorKind, RemoteError};
    match request {
        Request::WatchWorkspace {
            root,
            include_ignored,
        } => {
            let root = fs::canonicalize(root)?;
            if !root.is_dir() {
                return Err(RemoteError::new(
                    ErrorKind::InvalidInput,
                    "Workspace root is not a directory",
                ));
            }
            static NEXT: AtomicU64 = AtomicU64::new(1);
            let watch_id = NEXT.fetch_add(1, Ordering::Relaxed);
            let filesystem =
                WorkspaceFilesystem::local(root.to_string_lossy().into_owned(), include_ignored);
            let updates = filesystem.subscribe();
            watches().lock().unwrap().insert(
                watch_id,
                RemoteWatch {
                    filesystem,
                    updates,
                },
            );
            Ok(Response::WorkspaceWatch { watch_id })
        }
        Request::UnwatchWorkspace { watch_id } => {
            watches().lock().unwrap().remove(&watch_id);
            Ok(Response::Unit)
        }
        Request::PollWorkspace { watch_id } => {
            let registry = watches().lock().unwrap();
            let watch = registry
                .get(&watch_id)
                .ok_or_else(|| RemoteError::new(ErrorKind::NotFound, "Workspace watch expired"))?;
            Ok(Response::WorkspaceUpdates {
                updates: watch.updates.try_iter().take(64).collect(),
            })
        }
        Request::RefreshWorkspace {
            watch_id,
            include_ignored,
        } => {
            let registry = watches().lock().unwrap();
            let watch = registry
                .get(&watch_id)
                .ok_or_else(|| RemoteError::new(ErrorKind::NotFound, "Workspace watch expired"))?;
            let _ = watch
                .filesystem
                .0
                .sender
                .send(Command::RefreshOptions(include_ignored));
            Ok(Response::Unit)
        }
        Request::WatchDirectory { watch_id, path } => {
            let registry = watches().lock().unwrap();
            let watch = registry
                .get(&watch_id)
                .ok_or_else(|| RemoteError::new(ErrorKind::NotFound, "Workspace watch expired"))?;
            watch.filesystem.request_directory(path);
            Ok(Response::Unit)
        }
        Request::RefreshWorkspaceDirectory { watch_id, path } => {
            let registry = watches().lock().unwrap();
            let watch = registry
                .get(&watch_id)
                .ok_or_else(|| RemoteError::new(ErrorKind::NotFound, "Workspace watch expired"))?;
            watch.filesystem.refresh_directory(path);
            Ok(Response::Unit)
        }
        _ => unreachable!("Only workspace watch requests are dispatched here"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture(
        PathBuf,
        #[allow(dead_code)] std::sync::MutexGuard<'static, ()>,
    );
    impl Fixture {
        fn new() -> Self {
            // notify's FSEvents teardown purges device events. Keep native
            // stream lifecycle tests independent within this test process.
            static NATIVE_WATCHER_TESTS: Mutex<()> = Mutex::new(());
            let guard = NATIVE_WATCHER_TESTS
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "bed-watch-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&root).unwrap();
            Self(fs::canonicalize(root).unwrap(), guard)
        }
        fn write(&self, path: &str, bytes: &[u8]) {
            let path = self.0.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, bytes).unwrap();
        }
        fn start(
            &self,
        ) -> (
            WorkspaceFilesystem,
            Receiver<WorkspaceUpdate>,
            WorkspaceUpdate,
        ) {
            let filesystem =
                WorkspaceFilesystem::local(self.0.to_string_lossy().into_owned(), false);
            let updates = filesystem.subscribe();
            let initial = wait(&updates, |update| update.indexed_files.is_some());
            assert!(initial.degraded.is_none(), "{:?}", initial.degraded);
            (filesystem, updates, initial)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn wait(
        receiver: &Receiver<WorkspaceUpdate>,
        predicate: impl Fn(&WorkspaceUpdate) -> bool,
    ) -> WorkspaceUpdate {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let update = receiver
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("filesystem update timed out");
            if predicate(&update) {
                return update;
            }
        }
    }
    #[test]
    fn workspace_idle_has_no_recurring_full_scan_and_new_subtrees_are_indexed() {
        let fixture = Fixture::new();
        fixture.write("src/main.rs", b"initial");
        let (_filesystem, updates, initial) = fixture.start();
        assert_eq!(initial.indexed_files.as_ref().unwrap().len(), 1);
        let deadline = Instant::now() + Duration::from_millis(3250);
        while Instant::now() < deadline {
            if let Ok(update) = updates.recv_timeout(Duration::from_millis(50)) {
                assert!(
                    update.indexed_files.is_none(),
                    "idle watcher performed another full scan"
                );
            }
        }
        fixture.write("new/nested/file.rs", b"new");
        let update = wait(&updates, |update| {
            update
                .indexed_added
                .iter()
                .any(|path| path.ends_with("new/nested/file.rs"))
        });
        assert!(update.indexed_files.is_none());
        assert!(update.generation > initial.generation);
    }
    #[test]
    fn workspace_external_rename_and_removal_are_distinct_changes() {
        let fixture = Fixture::new();
        fixture.write("before.rs", b"text");
        let (_filesystem, updates, _) = fixture.start();
        fs::rename(fixture.0.join("before.rs"), fixture.0.join("after.rs")).unwrap();
        let renamed = wait(&updates, |update| {
            update.changes.iter().any(|change| matches!(change, FilesystemChange::Renamed { from, to } if from.ends_with("before.rs") && to.ends_with("after.rs")))
        });
        assert!(!renamed.changes.iter().any(|change| matches!(change, FilesystemChange::Removed { path } if path.ends_with("before.rs"))));
        fs::remove_file(fixture.0.join("after.rs")).unwrap();
        let removed = wait(&updates, |update| {
            update.changes.iter().any(|change| matches!(change, FilesystemChange::Removed { path } if path.ends_with("after.rs")))
        });
        assert!(
            removed
                .indexed_removed
                .iter()
                .any(|path| path.ends_with("after.rs"))
        );
    }
    #[test]
    fn workspace_external_folder_rename_preserves_the_parent_identity() {
        let fixture = Fixture::new();
        fixture.write("before/nested/file.rs", b"text");
        fixture.write("elsewhere/unrelated", b"text");
        let (_filesystem, updates, _) = fixture.start();
        fs::rename(fixture.0.join("before"), fixture.0.join("elsewhere/after")).unwrap();
        let renamed = wait(&updates, |update| {
            update.changes.iter().any(|change| matches!(change, FilesystemChange::Renamed { from,to } if from.ends_with("/before") && to.ends_with("/after")))
        });
        assert!(!renamed.changes.iter().any(|change| matches!(change, FilesystemChange::Removed { path } if path.ends_with("/before") || path.contains("/before/"))));
        assert!(
            renamed
                .indexed_added
                .iter()
                .any(|path| path.ends_with("after/nested/file.rs"))
        );
    }
    #[test]
    fn workspace_ignore_rules_update_the_index_without_removing_existing_documents() {
        let fixture = Fixture::new();
        git2::Repository::init(&fixture.0).unwrap();
        fixture.write(".gitignore", b"ignored/\n");
        fixture.write("ignored/artifact", b"data");
        fixture.write("src/main.rs", b"text");
        let (filesystem, updates, initial) = fixture.start();
        assert!(
            !initial
                .indexed_files
                .unwrap()
                .iter()
                .any(|path| path.ends_with("ignored/artifact"))
        );
        filesystem.set_include_ignored(true);
        let included = wait(&updates, |update| {
            update
                .indexed_files
                .as_ref()
                .is_some_and(|paths| paths.iter().any(|path| path.ends_with("ignored/artifact")))
        });
        assert!(
            included
                .indexed_files
                .unwrap()
                .iter()
                .all(|path| !path.contains("/.git/"))
        );
        filesystem.set_include_ignored(false);
        let excluded = wait(&updates, |update| update.indexed_files.is_some());
        assert!(!excluded.changes.iter().any(|change| matches!(change, FilesystemChange::Removed { path } if path.ends_with("ignored/artifact"))));
        fixture.write(".gitignore", b"");
        wait(&updates, |update| {
            update
                .indexed_added
                .iter()
                .chain(update.indexed_files.iter().flatten())
                .any(|path| path.ends_with("ignored/artifact"))
        });
    }
    #[test]
    fn workspace_atomic_replacement_does_not_emit_a_missing_target() {
        let fixture = Fixture::new();
        fixture.write("document", b"old");
        let (_filesystem, updates, _) = fixture.start();
        fixture.write("temporary", b"new");
        fs::rename(fixture.0.join("temporary"), fixture.0.join("document")).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut saw_target = false;
        while Instant::now() < deadline {
            if let Ok(update) = updates.recv_timeout(Duration::from_millis(50)) {
                assert!(!update.changes.iter().any(|change| matches!(change, FilesystemChange::Removed { path } if path.ends_with("/document"))));
                saw_target |= update.changes.iter().any(|change| matches!(change, FilesystemChange::Modified { path } if path.ends_with("/document")));
            }
        }
        assert!(saw_target);
    }
    #[test]
    fn workspace_requested_ignored_directory_uses_the_shared_background_cache() {
        let fixture = Fixture::new();
        git2::Repository::init(&fixture.0).unwrap();
        fixture.write(".gitignore", b"build/\n");
        fixture.write("build/artifact", b"bytes");
        let (filesystem, updates, _) = fixture.start();
        filesystem.request_directory(fixture.0.join("build").to_string_lossy().into_owned());
        let listing = wait(&updates, |update| {
            update
                .directories
                .iter()
                .any(|listing| listing.path.ends_with("/build"))
        });
        let entries = &listing
            .directories
            .iter()
            .find(|listing| listing.path.ends_with("/build"))
            .unwrap()
            .entries;
        assert_eq!(entries.len(), 1);
        assert!(entries[0].is_gitignored);
        fixture.write("build/new", b"bytes");
        wait(&updates, |update| {
            update.directories.iter().any(|listing| {
                listing.path.ends_with("/build")
                    && listing.entries.iter().any(|entry| entry.name == "new")
            })
        });
    }
    #[test]
    fn requested_git_directory_tracks_lock_files_without_indexing_them() {
        let fixture = Fixture::new();
        git2::Repository::init(&fixture.0).unwrap();
        fixture.write(".git/index.lock", b"lock");
        let (filesystem, updates, _) = fixture.start();
        let directory = fixture.0.join(".git").to_string_lossy().into_owned();
        filesystem.request_directory(directory.clone());
        wait(&updates, |update| {
            update.directories.iter().any(|listing| {
                listing.path == directory
                    && listing
                        .entries
                        .iter()
                        .any(|entry| entry.name == "index.lock")
            })
        });
        fs::remove_file(fixture.0.join(".git/index.lock")).unwrap();
        filesystem.refresh_directory(directory.clone());
        let refreshed = wait(&updates, |update| {
            update.directories.iter().any(|listing| {
                listing.path == directory
                    && !listing
                        .entries
                        .iter()
                        .any(|entry| entry.name == "index.lock")
            })
        });
        assert!(refreshed.indexed_files.is_none());
        assert!(refreshed.indexed_added.is_empty());
        assert!(refreshed.indexed_removed.is_empty());
        assert!(refreshed.changes.is_empty());

        // External changes also update the open folder through its watcher.
        fixture.write(".git/index.lock", b"another lock");
        let created = wait(&updates, |update| {
            update.directories.iter().any(|listing| {
                listing.path == directory
                    && listing
                        .entries
                        .iter()
                        .any(|entry| entry.name == "index.lock")
            })
        });
        assert!(created.indexed_added.is_empty());
        assert!(created.changes.is_empty());
    }
    #[test]
    fn workspace_explicit_directory_refresh_sends_a_delta_and_hides_incomplete_transfers() {
        let fixture = Fixture::new();
        fixture.write("src/original", b"bytes");
        fixture.write("other/unchanged", b"bytes");
        let (filesystem, updates, _) = fixture.start();
        fixture.write("src/added", b"bytes");
        fixture.write("src/.bed-transfer-private", b"partial");
        filesystem.refresh_directory(fixture.0.join("src").to_string_lossy().into_owned());
        let update = wait(&updates, |update| {
            update
                .indexed_added
                .iter()
                .any(|path| path.ends_with("src/added"))
        });
        assert!(update.indexed_files.is_none());
        assert!(
            update
                .indexed_added
                .iter()
                .all(|path| !path.contains(".bed-transfer-"))
        );
        assert!(
            !update
                .directories
                .iter()
                .any(|listing| listing.path.ends_with("/other"))
        );
        let src = update
            .directories
            .iter()
            .find(|listing| listing.path.ends_with("/src"))
            .unwrap();
        assert!(
            src.entries
                .iter()
                .all(|entry| !entry.name.starts_with(".bed-transfer-"))
        );
    }
    #[test]
    fn workspace_git_negations_tracked_ignored_files_and_index_changes_follow_git() {
        let fixture = Fixture::new();
        let repository = git2::Repository::init(&fixture.0).unwrap();
        fixture.write("ignored/tracked.log", b"tracked");
        fixture.write("src/new.log", b"new");
        let mut index = repository.index().unwrap();
        index.add_path(Path::new("ignored/tracked.log")).unwrap();
        index.write().unwrap();
        fixture.write(".gitignore", b"ignored/\n*.log\n");
        fixture.write("src/.gitignore", b"!keep.log\n");
        fixture.write("src/keep.log", b"keep");
        let (_filesystem, updates, initial) = fixture.start();
        let paths = initial.indexed_files.unwrap();
        assert!(
            paths
                .iter()
                .any(|path| path.ends_with("ignored/tracked.log"))
        );
        assert!(paths.iter().any(|path| path.ends_with("src/keep.log")));
        assert!(!paths.iter().any(|path| path.ends_with("src/new.log")));
        index.add_path(Path::new("src/new.log")).unwrap();
        index.write().unwrap();
        wait(&updates, |update| {
            update
                .indexed_added
                .iter()
                .chain(update.indexed_files.iter().flatten())
                .any(|path| path.ends_with("src/new.log"))
        });
    }
    #[test]
    fn workspace_shutdown_is_bounded_when_a_filesystem_call_is_blocked() {
        let (release, blocked) = mpsc::channel();
        let (sender, _) = mpsc::channel();
        let worker = thread::spawn(move || {
            let _ = blocked.recv();
        });
        let service = WorkspaceFilesystem(Arc::new(Inner {
            sender,
            stop: Arc::new(AtomicBool::new(false)),
            worker: Mutex::new(Some(worker)),
        }));
        let started = Instant::now();
        drop(service);
        let elapsed = started.elapsed();
        let _ = release.send(());
        assert!(
            elapsed < Duration::from_millis(500),
            "Shutdown waited {elapsed:?}"
        );
    }
    #[test]
    fn workspace_remote_poll_returns_cached_batches_without_rescanning() {
        let fixture = Fixture::new();
        fixture.write("file", b"data");
        let Response::WorkspaceWatch { watch_id } = call(Request::WatchWorkspace {
            root: fixture.0.to_string_lossy().into_owned(),
            include_ignored: false,
        })
        .unwrap() else {
            unreachable!()
        };
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            let Response::WorkspaceUpdates { updates } =
                call(Request::PollWorkspace { watch_id }).unwrap()
            else {
                unreachable!()
            };
            if updates.iter().any(|update| update.indexed_files.is_some()) {
                break;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        }
        let Response::WorkspaceUpdates { updates } =
            call(Request::PollWorkspace { watch_id }).unwrap()
        else {
            unreachable!()
        };
        assert!(updates.is_empty());
        call(Request::UnwatchWorkspace { watch_id }).unwrap();
        assert!(call(Request::PollWorkspace { watch_id }).is_err());
    }
}
