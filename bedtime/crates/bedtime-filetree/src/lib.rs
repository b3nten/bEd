//! A file tree panel: the folders under the workbench's [`Root`], read in the
//! background and kept up to date by watching them. A single click on a file
//! opens a preview of it with `open_path`, a double click or Enter opens it for
//! good, and its context menu lists `openers_for` as "Open With".
use bedtime_filesystem::{EntryKind, Filesystem, watch::DirectoryWatcher};
#[cfg(feature = "persistence")]
use bedtime_workbench::persistence::ReflectPersist;
use bedtime_workbench::{ManagedTask, ManagedWork, PanelInstance, Root};
use bevy::prelude::*;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

mod ops;
mod view;
pub use bedtime_filesystem::operations::Operation;
pub use ops::{FileOperations, copy_files, paste_files, run_file_operation};
pub use view::{ToggleHiddenFiles, ToggleIgnoredFiles, file_icon, open_files_factory};

#[cfg(test)]
mod tests;

pub struct FileTreePlugin;

impl Plugin for FileTreePlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<FileTree>()
            .init_resource::<FileTreeDefaults>()
            .init_resource::<Folders>()
            .init_resource::<view::Intents>()
            .init_resource::<FileOperations>()
            .add_systems(
                Update,
                (view::apply_intents, ops::finish_operations, update_folders)
                    .chain()
                    .in_set(FileTreeSystems),
            );
        view::install_actions(app);
    }
}

/// Doing what the trees were asked, then reading and watching folders, in `Update`.
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct FileTreeSystems;

/// What trees show unless one is told otherwise (`FileTree::show_hidden`,
/// `show_ignored`). Both are shown by default, dimmed.
#[derive(Resource, Clone, Copy, Debug, PartialEq)]
pub struct FileTreeDefaults {
    /// Hidden files: dot files, and those the system marks hidden.
    pub show_hidden: bool,
    /// Files Git ignores (`.gitignore`, `.git/info/exclude`, the global excludes).
    pub show_ignored: bool,
}

impl Default for FileTreeDefaults {
    fn default() -> Self {
        Self {
            show_hidden: true,
            show_ignored: true,
        }
    }
}

/// The file tree panel. Which folders are open, and whether it shows hidden
/// and ignored files, is saved with it.
#[derive(Component, Reflect, Clone, Debug)]
#[reflect(Component, Default)]
#[cfg_attr(feature = "persistence", reflect(Persist))]
#[require(PanelInstance = PanelInstance::of::<Self>(), TreeView)]
pub struct FileTree {
    /// Open folders, relative to the root; the empty path is the root itself.
    expanded: Vec<PathBuf>,
    /// Show hidden files; `None` follows `FileTreeDefaults`.
    #[reflect(default)]
    pub show_hidden: Option<bool>,
    /// Show files Git ignores; `None` follows `FileTreeDefaults`.
    #[reflect(default)]
    pub show_ignored: Option<bool>,
    /// Bumped when `expanded` changes, so the rows are worked out again.
    #[reflect(ignore)]
    generation: u64,
}

impl Default for FileTree {
    /// The root open.
    fn default() -> Self {
        Self {
            expanded: vec![PathBuf::new()],
            show_hidden: None,
            show_ignored: None,
            generation: 0,
        }
    }
}

impl FileTree {
    pub fn is_expanded(&self, relative: &Path) -> bool {
        self.expanded.iter().any(|open| open == relative)
    }

    pub fn set_expanded(&mut self, relative: &Path, expanded: bool) {
        if expanded == self.is_expanded(relative) {
            return;
        }
        if expanded {
            self.expanded.push(relative.to_owned());
        } else {
            self.expanded.retain(|open| open != relative);
        }
        self.generation += 1;
    }

    /// Whether hidden and ignored files show, given the defaults.
    pub fn filter(&self, defaults: &FileTreeDefaults) -> TreeFilter {
        TreeFilter {
            hidden: self.show_hidden.unwrap_or(defaults.show_hidden),
            ignored: self.show_ignored.unwrap_or(defaults.show_ignored),
        }
    }

    /// The open folders, as paths under `root`.
    pub fn expanded_folders<'a>(&'a self, root: &'a Path) -> impl Iterator<Item = PathBuf> + 'a {
        self.expanded.iter().map(|relative| {
            if relative.as_os_str().is_empty() {
                root.to_owned()
            } else {
                root.join(relative)
            }
        })
    }
}

/// Which kinds of files a tree shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TreeFilter {
    pub hidden: bool,
    pub ignored: bool,
}

impl TreeFilter {
    pub const ALL: Self = Self {
        hidden: true,
        ignored: true,
    };
}

/// What's drawn: the rows, the selection, scrolling. Not saved.
#[derive(Component, Default)]
pub struct TreeView {
    rows: Arc<[Row]>,
    /// What `rows` was worked out from: root, tree generation, folders
    /// revision, what shows.
    rows_from: Option<(PathBuf, u64, u64, TreeFilter)>,
    selected: Option<PathBuf>,
    /// Scroll so this row shows, on the next draw.
    reveal: Option<PathBuf>,
    /// The context menu's file and what can open it, found when it opened.
    menu: Option<(PathBuf, Vec<(bedtime_workbench::OpenerId, &'static str)>)>,
    typed: String,
    typed_at: f64,
    /// A name being typed (new file or folder, rename).
    naming: Option<view::Naming>,
}

impl TreeView {
    pub fn selected(&self) -> Option<&Path> {
        self.selected.as_deref()
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }
}

/// One line of the tree.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub path: PathBuf,
    pub name: String,
    pub depth: usize,
    pub kind: EntryKind,
    /// Hidden, or in a hidden folder.
    pub hidden: bool,
    /// Ignored by Git, or in an ignored folder.
    pub ignored: bool,
    /// A folder that's open.
    pub expanded: bool,
    /// For an open folder: still being read, or why it couldn't be.
    pub loading: bool,
    pub error: Option<String>,
}

/// A folder's entry.
#[derive(Clone, Debug)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub kind: EntryKind,
    pub hidden: bool,
    /// Git ignores it. Only worked out on the local filesystem.
    pub ignored: bool,
}

/// The folders open in any file tree, shared by them all: their last listing,
/// and a watch on each. A folder is read again when its watch reports a change;
/// without a working watcher, every two seconds.
#[derive(Resource)]
pub struct Folders {
    filesystem: Filesystem,
    /// `Root`, resolved: listings use resolved paths.
    root: Option<Result<PathBuf, String>>,
    root_from: Option<PathBuf>,
    folders: HashMap<PathBuf, Folder>,
    watcher: Option<Mutex<DirectoryWatcher>>,
    watch_error: Option<String>,
    polled: Instant,
    /// Bumped when any listing changes.
    revision: u64,
}

#[derive(Default)]
struct Folder {
    entries: Option<Arc<[Entry]>>,
    error: Option<String>,
    reading: Option<ManagedTask<Result<Vec<Entry>, String>>>,
    /// Read again once the current read finishes.
    stale: bool,
    /// Watching worked. It's tried again on each read until it does.
    watched: bool,
}

impl Default for Folders {
    fn default() -> Self {
        Self::new(Filesystem::local())
    }
}

/// More open folders than this aren't read.
pub const MAX_FOLDERS: usize = 512;
const POLL: Duration = Duration::from_secs(2);

impl Folders {
    pub fn new(filesystem: Filesystem) -> Self {
        let (watcher, watch_error) = match DirectoryWatcher::new() {
            Ok(watcher) => (Some(Mutex::new(watcher)), None),
            Err(error) => (None, Some(error.to_string())),
        };
        Self {
            filesystem,
            root: None,
            root_from: None,
            folders: HashMap::new(),
            watcher,
            watch_error,
            polled: Instant::now(),
            revision: 0,
        }
    }

    /// A folder's last listing, sorted folders first then by name.
    pub fn entries(&self, folder: &Path) -> Option<&[Entry]> {
        self.folders.get(folder)?.entries.as_deref()
    }

    pub fn error(&self, folder: &Path) -> Option<&str> {
        self.folders.get(folder)?.error.as_deref()
    }

    pub fn is_reading(&self, folder: &Path) -> bool {
        self.folders
            .get(folder)
            .is_some_and(|folder| folder.reading.is_some())
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The root the trees show (`Root`, resolved), or why it can't be shown.
    pub fn root(&self) -> Option<Result<&Path, &str>> {
        self.root
            .as_ref()
            .map(|root| root.as_deref().map_err(String::as_str))
    }

    /// Read every open folder again.
    pub fn refresh(&mut self) {
        self.root_from = None;
        for folder in self.folders.values_mut() {
            folder.stale = true;
        }
    }

    /// Why changes aren't being watched, if they aren't.
    pub fn watch_error(&self) -> Option<&str> {
        self.watch_error.as_deref()
    }
}

/// The root the trees show, if there's one that can be.
fn root(world: &World) -> Option<PathBuf> {
    Some(world.resource::<Folders>().root()?.ok()?.to_owned())
}

/// Keep the folders the trees have open read and watched; let go of the rest.
fn update_folders(world: &mut World) {
    let given = world.get_resource::<Root>().map(|root| root.0.clone());
    let mut folders = world.resource_mut::<Folders>();
    if folders.root_from != given {
        folders.root = given.as_ref().map(|root| {
            folders
                .filesystem
                .source(root)
                .canonicalize()
                .map(|source| source.path().to_owned())
                .map_err(|error| error.to_string())
        });
        folders.root_from = given;
    }
    let wanted: Vec<PathBuf> = match root(world) {
        Some(root) => {
            let mut wanted: Vec<PathBuf> = world
                .query::<&FileTree>()
                .iter(world)
                .flat_map(|tree| tree.expanded_folders(&root).collect::<Vec<_>>())
                .collect();
            wanted.sort();
            wanted.dedup();
            wanted.truncate(MAX_FOLDERS);
            wanted
        }
        None => Vec::new(),
    };
    world.resource_scope(|world, mut folders: Mut<Folders>| {
        let folders = &mut *folders;
        let mut changed = false;

        // Folders no longer open.
        let gone: Vec<PathBuf> = folders
            .folders
            .keys()
            .filter(|path| wanted.binary_search(path).is_err())
            .cloned()
            .collect();
        for path in gone {
            let folder = folders.folders.remove(&path).unwrap();
            if let Some(task) = folder.reading {
                task.cancel();
            }
            if folder.watched
                && let Some(watcher) = &folders.watcher
            {
                let _ = watcher.lock().unwrap().unwatch(&path);
            }
        }

        // What changed on disk.
        if let Some(watcher) = &folders.watcher {
            match watcher.lock().unwrap().poll() {
                Ok(batch) => {
                    if let Some(error) = batch.error {
                        folders.watch_error = Some(error);
                    }
                    // A changed .gitignore can change what's ignored below it.
                    let ignores_changed = batch
                        .events
                        .iter()
                        .any(|event| event.path.file_name().is_some_and(|n| n == ".gitignore"));
                    for event in batch.events {
                        // A changed folder, or a changed entry in one.
                        for path in [Some(event.path.as_path()), event.path.parent()]
                            .into_iter()
                            .flatten()
                        {
                            if let Some(folder) = folders.folders.get_mut(path) {
                                folder.stale = true;
                            }
                        }
                    }
                    if batch.rescan || ignores_changed {
                        folders.folders.values_mut().for_each(|f| f.stale = true);
                    }
                }
                Err(error) => folders.watch_error = Some(error.to_string()),
            }
        }
        if folders.watch_error.is_some() && folders.polled.elapsed() >= POLL {
            folders.polled = Instant::now();
            folders.folders.values_mut().for_each(|f| f.stale = true);
        }

        // Finished reads.
        for folder in folders.folders.values_mut() {
            let Some(task) = &folder.reading else {
                continue;
            };
            let result = match task.try_recv() {
                Ok(result) => result,
                Err(std::sync::mpsc::TryRecvError::Empty) => continue,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    Err("The folder couldn't be read".to_owned())
                }
            };
            folder.reading = None;
            match result {
                Ok(entries) => {
                    // A watch event for an unchanged listing redraws nothing.
                    if !folder
                        .entries
                        .as_deref()
                        .is_some_and(|old| same(old, &entries))
                    {
                        folder.entries = Some(entries.into());
                        changed = true;
                    }
                    changed |= folder.error.take().is_some();
                }
                Err(error) => {
                    // The last listing stays: a failed reread isn't a deletion.
                    changed |= folder.error.as_ref() != Some(&error);
                    folder.error = Some(error);
                }
            }
        }

        // New and stale folders. A folder is watched before it's read, so a
        // change in between isn't missed.
        let Some(mut work) = world.get_resource_mut::<ManagedWork>() else {
            return;
        };
        for path in &wanted {
            let folder = folders.folders.entry(path.clone()).or_insert_with(|| {
                changed = true;
                Folder {
                    stale: true,
                    ..default()
                }
            });
            if folder.stale && folder.reading.is_none() {
                folder.stale = false;
                if !folder.watched
                    && let Some(watcher) = &folders.watcher
                {
                    folder.watched = watcher.lock().unwrap().watch(path).is_ok();
                }
                let filesystem = folders.filesystem.clone();
                let target = path.clone();
                match work.spawn_task(None, format!("Reading {}", path.display()), move |cancel| {
                    read(&filesystem, &target, &|| cancel.is_cancelled())
                }) {
                    Ok(task) => folder.reading = Some(task),
                    Err(error) => folder.error = Some(error),
                }
            }
        }
        if changed {
            folders.revision += 1;
        }
    });
}

fn same(old: &[Entry], new: &[Entry]) -> bool {
    old.len() == new.len()
        && old
            .iter()
            .zip(new)
            .all(|(a, b)| a.path == b.path && a.kind == b.kind && a.ignored == b.ignored)
}

/// A folder's entries: folders first, then by name ignoring case.
fn read(
    filesystem: &Filesystem,
    folder: &Path,
    cancelled: &dyn Fn() -> bool,
) -> Result<Vec<Entry>, String> {
    let entries = filesystem
        .source(folder)
        .read_directory(cancelled)
        .map_err(|error| error.to_string())?;
    let mut entries: Vec<Entry> = entries
        .into_iter()
        .map(|entry| Entry {
            name: entry.name.to_string_lossy().into_owned(),
            path: entry.path,
            kind: entry.kind,
            hidden: entry.hidden,
            ignored: false,
        })
        .collect();
    if filesystem.is_local() {
        mark_ignored(folder, &mut entries);
    }
    entries.sort_by(|a, b| {
        (b.kind == EntryKind::Directory)
            .cmp(&(a.kind == EntryKind::Directory))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.path.cmp(&b.path))
    });
    Ok(entries)
}

/// Mark the entries Git ignores: those a walk of `folder` that applies Git's
/// ignore files (its own, its parents', `.git/info/exclude`, the global
/// excludes) leaves out. Outside a Git repository nothing is ignored.
fn mark_ignored(folder: &Path, entries: &mut [Entry]) {
    let kept: std::collections::HashSet<PathBuf> = ignore::WalkBuilder::new(folder)
        .max_depth(Some(1))
        .standard_filters(false)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(true)
        .parents(true)
        .require_git(true)
        .build()
        .filter_map(Result::ok)
        .map(ignore::DirEntry::into_path)
        .collect();
    for entry in entries {
        entry.ignored = !kept.contains(&entry.path);
    }
}

/// The rows a tree shows: the root, then each open folder's entries that
/// `filter` lets through.
pub fn rows(root: &Path, tree: &FileTree, folders: &Folders, filter: TreeFilter) -> Vec<Row> {
    let mut rows = Vec::new();
    let name = root.file_name().map_or_else(
        || root.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    push(
        &mut rows,
        root,
        root,
        name,
        0,
        EntryKind::Directory,
        (false, false),
        tree,
        folders,
        filter,
    );
    rows
}

#[allow(clippy::too_many_arguments)]
fn push(
    rows: &mut Vec<Row>,
    root: &Path,
    path: &Path,
    name: String,
    depth: usize,
    kind: EntryKind,
    (hidden, ignored): (bool, bool),
    tree: &FileTree,
    folders: &Folders,
    filter: TreeFilter,
) {
    let open = kind == EntryKind::Directory
        && path
            .strip_prefix(root)
            .is_ok_and(|relative| tree.is_expanded(relative));
    let entries = open.then(|| folders.entries(path)).flatten();
    rows.push(Row {
        path: path.to_owned(),
        name,
        depth,
        kind,
        hidden,
        ignored,
        expanded: open,
        loading: open && entries.is_none() && folders.error(path).is_none(),
        error: open
            .then(|| folders.error(path).map(str::to_owned))
            .flatten(),
    });
    for entry in entries.into_iter().flatten() {
        let hidden = hidden || entry.hidden;
        let ignored = ignored || entry.ignored;
        if hidden && !filter.hidden || ignored && !filter.ignored {
            continue;
        }
        push(
            rows,
            root,
            &entry.path,
            entry.name.clone(),
            depth + 1,
            entry.kind,
            (hidden, ignored),
            tree,
            folders,
            filter,
        );
    }
}

/// Bring `view.rows` up to date.
fn refresh_rows(
    root: &Path,
    tree: &FileTree,
    folders: &Folders,
    filter: TreeFilter,
    view: &mut TreeView,
) {
    let from = (root.to_owned(), tree.generation, folders.revision, filter);
    if view.rows_from.as_ref() != Some(&from) {
        view.rows = rows(root, tree, folders, filter).into();
        view.rows_from = Some(from);
    }
}
