//! Translated from ned files/file_finder.{h,cpp}; see LICENSE and NOTICE.
//! Shared filesystem discovery sends an initial index and incremental deltas.
//! Only the UI thread mutates displayed finder results.
use bed_remote::{DirectoryListing, WorkspaceFilesystem, WorkspaceUpdate};
use std::{
    io,
    path::{Path, PathBuf},
    sync::mpsc::Receiver,
};
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileEntry {
    pub full_path: String,
    pub relative_path: String,
    pub relative_path_lower: String,
    pub filename_lower: String,
}
impl FileEntry {
    /// Build a target-native entry without probing the local filesystem.
    pub fn from_remote_path(path: &str, root: &str) -> io::Result<Self> {
        let prefix = format!("{}/", root.trim_end_matches('/'));
        let relative = path.strip_prefix(&prefix).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "Remote file is outside its workspace",
            )
        })?;
        Ok(Self {
            full_path: path.to_owned(),
            relative_path: relative.to_owned(),
            relative_path_lower: relative.to_ascii_lowercase(),
            filename_lower: relative
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .to_ascii_lowercase(),
        })
    }
    pub fn from_path(path: &Path, root: &Path) -> io::Result<Self> {
        let full_path = path
            .to_str()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "File path is not UTF-8"))?
            .to_owned();
        let relative = relative_path(path, root)?;
        let relative_path = relative
            .to_str()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "File path is not UTF-8"))?
            .to_owned();
        let filename_lower = relative
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        Ok(Self {
            relative_path_lower: relative_path.to_ascii_lowercase(),
            full_path,
            relative_path,
            filename_lower,
        })
    }
}
fn relative_path(path: &Path, root: &Path) -> io::Result<PathBuf> {
    // Discovery already canonicalized the workspace root. Keep entry paths
    // lexical: per-file canonicalization is expensive and destroys link names.
    path.strip_prefix(root)
        .map(Path::to_path_buf)
        .map_err(io::Error::other)
}
/// One-shot discovery for callers that explicitly need a fresh snapshot.
/// Finder background updates use WorkspaceFilesystem instead of this walk.
pub fn scan_file_list(project_dir: &Path) -> io::Result<Vec<FileEntry>> {
    scan_file_list_cancellable(project_dir, || false).map(Option::unwrap_or_default)
}
fn scan_file_list_cancellable(
    project_dir: &Path,
    canceled: impl FnMut() -> bool,
) -> io::Result<Option<Vec<FileEntry>>> {
    crate::search_files::discover(project_dir, false, canceled, |_, _| true)
}
/// Substring filter and path-length ranking, preserving the upstream dotfile exception.
pub fn filter_file_list(files: &[FileEntry], query: &str) -> Vec<FileEntry> {
    let query = query.to_ascii_lowercase();
    let mut out: Vec<_> = files
        .iter()
        .filter(|file| {
            file.relative_path_lower.contains(&query)
                && (query.contains('.') || !file.filename_lower.starts_with('.'))
        })
        .cloned()
        .collect();
    out.sort_by_key(|file| file.relative_path.len());
    out
}
fn merge_entries(files: &mut Vec<FileEntry>, mut additions: Vec<FileEntry>) {
    additions.sort_by(|a, b| a.full_path.cmp(&b.full_path));
    additions.dedup_by(|a, b| a.full_path == b.full_path);
    let mut merged = Vec::with_capacity(files.len() + additions.len());
    let mut old = std::mem::take(files).into_iter().peekable();
    let mut added = additions.into_iter().peekable();
    while let (Some(previous), Some(next)) = (old.peek(), added.peek()) {
        match previous.full_path.cmp(&next.full_path) {
            std::cmp::Ordering::Less => merged.push(old.next().unwrap()),
            std::cmp::Ordering::Greater => merged.push(added.next().unwrap()),
            std::cmp::Ordering::Equal => {
                old.next();
                merged.push(added.next().unwrap());
            }
        }
    }
    merged.extend(old);
    merged.extend(added);
    *files = merged;
}
#[derive(Default)]
pub struct FileFinder {
    remote_client: Option<bed_remote::RemoteClient>,
    pub show_ff_window: bool,
    pub search_buffer: String,
    pub selected_index: usize,
    pub filtered_list: Vec<FileEntry>,
    previous_search: String,
    filter_dirty: bool,
    file_list: Vec<FileEntry>,
    current_project_dir: String,
    filesystem: Option<WorkspaceFilesystem>,
    updates: Option<Receiver<WorkspaceUpdate>>,
    workspace_updates: Vec<WorkspaceUpdate>,
    directory_updates: Vec<DirectoryListing>,
    requested_directories: std::collections::BTreeSet<String>,
    started: bool,
    include_ignored: bool,
    last_generation: u64,
    pub discovery_status: Option<String>,
}
impl FileFinder {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn set_remote_client(&mut self, client: Option<bed_remote::RemoteClient>) {
        self.stop_worker();
        self.remote_client = client;
        self.file_list.clear();
        self.filtered_list.clear();
        self.start_service();
    }
    fn stop_worker(&mut self) {
        self.updates.take();
        self.filesystem.take();
        self.workspace_updates.clear();
        self.directory_updates.clear();
        self.requested_directories.clear();
        self.last_generation = 0;
    }
    pub fn start_background_thread(&mut self) {
        self.started = true;
        self.start_service();
    }
    fn start_service(&mut self) {
        if !self.started || self.filesystem.is_some() || self.current_project_dir.is_empty() {
            return;
        }
        let filesystem = if let Some(client) = &self.remote_client {
            WorkspaceFilesystem::remote(
                self.current_project_dir.clone(),
                self.include_ignored,
                client.clone(),
            )
        } else {
            WorkspaceFilesystem::local(self.current_project_dir.clone(), self.include_ignored)
        };
        self.updates = Some(filesystem.subscribe());
        self.filesystem = Some(filesystem);
    }
    pub fn set_project_dir(&mut self, root: &str) {
        if self.current_project_dir == root {
            self.request_refresh();
            return;
        }
        self.stop_worker();
        self.file_list.clear();
        self.filtered_list.clear();
        self.search_buffer.clear();
        self.previous_search.clear();
        self.selected_index = 0;
        self.discovery_status = None;
        self.current_project_dir = root.to_owned();
        self.start_service();
    }
    pub fn workspace_filesystem(&self) -> Option<WorkspaceFilesystem> {
        self.filesystem.clone()
    }
    pub fn request_refresh(&self) {
        if let Some(filesystem) = &self.filesystem {
            filesystem.refresh();
        }
    }
    pub fn refresh_directories(&self, directories: impl IntoIterator<Item = String>) {
        if let Some(filesystem) = &self.filesystem {
            for directory in directories {
                filesystem.refresh_directory(directory);
            }
        }
    }
    pub fn request_directories(&mut self, directories: impl IntoIterator<Item = String>) {
        if let Some(filesystem) = &self.filesystem {
            for path in directories {
                if self.requested_directories.insert(path.clone()) {
                    filesystem.request_directory(path);
                }
            }
        }
    }
    pub fn include_ignored(&self) -> bool {
        self.include_ignored
    }
    pub fn set_include_ignored(&mut self, include: bool) {
        if self.include_ignored == include {
            return;
        }
        self.include_ignored = include;
        if let Some(filesystem) = &self.filesystem {
            filesystem.set_include_ignored(include);
        }
    }
    /// Host document handling receives the same generations as finder/tree.
    pub fn take_workspace_updates(&mut self) -> Vec<WorkspaceUpdate> {
        std::mem::take(&mut self.workspace_updates)
    }
    pub fn take_directory_updates(&mut self) -> Vec<DirectoryListing> {
        std::mem::take(&mut self.directory_updates)
    }
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        if let Some(receiver) = &self.updates {
            for mut update in receiver.try_iter() {
                if update.generation != 0 && update.generation < self.last_generation {
                    continue;
                }
                self.last_generation = update.generation;
                changed |= self.discovery_status != update.degraded;
                self.discovery_status = update.degraded.clone();
                if let Some(paths) = update.indexed_files.take() {
                    self.file_list = paths
                        .iter()
                        .filter_map(|path| FileEntry::from_remote_path(path, &update.root).ok())
                        .collect();
                    self.file_list.sort_by(|a, b| a.full_path.cmp(&b.full_path));
                    changed = true;
                }
                if !update.indexed_removed.is_empty() {
                    let removed: std::collections::HashSet<_> =
                        update.indexed_removed.iter().collect();
                    self.file_list
                        .retain(|entry| !removed.contains(&entry.full_path));
                    changed = true;
                }
                if !update.indexed_added.is_empty() {
                    let additions = update
                        .indexed_added
                        .iter()
                        .filter_map(|path| FileEntry::from_remote_path(path, &update.root).ok())
                        .collect();
                    merge_entries(&mut self.file_list, additions);
                    changed = true;
                }
                self.directory_updates.append(&mut update.directories);
                self.workspace_updates.push(update);
            }
        }
        self.filter_dirty |= changed;
        if changed && self.show_ff_window {
            self.update_filtered_list();
        }
        changed
    }
    pub fn update_filtered_list(&mut self) {
        self.filter_dirty = false;
        let query = self.search_buffer.to_ascii_lowercase();
        if query != self.previous_search {
            self.selected_index = 0;
            self.previous_search = query;
        }
        self.filtered_list = filter_file_list(&self.file_list, &self.search_buffer);
    }
    pub fn toggle_window(&mut self) {
        self.show_ff_window = !self.show_ff_window;
        if self.show_ff_window {
            self.search_buffer.clear();
            self.previous_search.clear();
            self.selected_index = 0;
            self.update_filtered_list();
        }
    }
    pub fn cancel_and_close(&mut self) {
        self.show_ff_window = false;
    }
    pub fn commit_selection(&mut self) -> Option<String> {
        self.show_ff_window = false;
        self.filtered_list
            .get(self.selected_index)
            .map(|f| f.full_path.clone())
    }
    pub fn move_selection(&mut self, delta: i32) {
        if delta < 0 {
            self.selected_index = self
                .selected_index
                .saturating_sub(delta.unsigned_abs() as usize);
        } else if !self.filtered_list.is_empty() {
            self.selected_index = self
                .selected_index
                .saturating_add(delta as usize)
                .min(self.filtered_list.len() - 1);
        }
    }
    pub fn set_query(&mut self, query: &str) {
        let mut end = query.len().min(255);
        while !query.is_char_boundary(end) {
            end -= 1;
        }
        self.search_buffer = query[..end].to_owned();
        if self.filter_dirty || self.search_buffer.to_ascii_lowercase() != self.previous_search {
            self.update_filtered_list();
        }
    }
}
impl Drop for FileFinder {
    fn drop(&mut self) {
        self.stop_worker();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use std::{
        thread,
        time::{Duration, Instant},
    };
    #[test]
    fn remote_entries_use_target_slashes_without_local_canonicalization() {
        let file =
            FileEntry::from_remote_path("/remote/project/src/A.RS", "/remote/project/").unwrap();
        assert_eq!(file.relative_path, "src/A.RS");
        assert_eq!(file.filename_lower, "a.rs");
        assert_eq!(file.relative_path_lower, "src/a.rs");
        assert_eq!(
            FileEntry::from_remote_path("/file.rs", "/")
                .unwrap()
                .relative_path,
            "file.rs"
        );
        assert!(FileEntry::from_remote_path("/remote/project-two/a", "/remote/project").is_err());
    }
    fn entry(path: &str) -> FileEntry {
        FileEntry {
            full_path: path.to_owned(),
            relative_path: path.to_owned(),
            relative_path_lower: path.to_ascii_lowercase(),
            filename_lower: Path::new(path)
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .to_ascii_lowercase(),
        }
    }
    #[test]
    fn substring_case_insensitive_ranking_is_path_byte_length() {
        let files = vec![
            entry("deep/long_name.rs"),
            entry("b.rs"),
            entry("a.rs"),
            entry("sources/test.RS"),
            entry("not.txt"),
        ];
        let filtered = filter_file_list(&files, ".rs");
        assert_eq!(filtered.len(), 4);
        assert_eq!(
            filtered
                .iter()
                .map(|f| f.relative_path.len())
                .collect::<Vec<_>>(),
            vec![4, 4, 15, 17]
        );
        assert!(filter_file_list(&files, "dlrs").is_empty());
    }
    #[test]
    fn hides_dotfiles_only_when_query_has_no_dot_and_includes_hidden_directories() {
        let files = vec![entry(".env"), entry(".git/config"), entry("src/main.rs")];
        let all = filter_file_list(&files, "");
        assert_eq!(all.len(), 2);
        assert!(all.iter().any(|f| f.relative_path == ".git/config"));
        assert_eq!(filter_file_list(&files, ".").len(), 3);
        assert!(filter_file_list(&files, "env").is_empty());
        assert_eq!(filter_file_list(&files, ".env")[0].relative_path, ".env");
    }
    #[test]
    fn scanner_excludes_git_and_gitignored_entries() {
        let temp = TempDir::new();
        git2::Repository::init(temp.root()).unwrap();
        temp.write(".gitignore", b"ignored\n");
        temp.write("ignored/file.txt", b"");
        temp.write("src/main.rs", b"");
        let files = scan_file_list(temp.root()).unwrap();
        assert_eq!(files.len(), 2);
        assert!(!files.iter().any(|f| f.relative_path == "ignored/file.txt"));
        assert!(!files.iter().any(|f| f.relative_path.starts_with(".git/")));
    }
    #[test]
    fn canceled_recursive_scan_discards_partial_results() {
        let temp = TempDir::new();
        temp.write("nested/a.rs", b"");
        temp.write("nested/b.rs", b"");
        let mut checks = 0;
        let files = scan_file_list_cancellable(temp.root(), || {
            checks += 1;
            checks == 3
        })
        .unwrap();
        assert!(files.is_none());
        assert_eq!(checks, 3);
    }
    #[test]
    fn query_changes_reset_selection_and_commit_is_only_open_signal() {
        let mut finder = FileFinder::new();
        finder.file_list = vec![entry("b.rs"), entry("deep/a.rs")];
        finder.toggle_window();
        finder.move_selection(1);
        assert_eq!(finder.selected_index, 1);
        assert!(finder.show_ff_window);
        finder.set_query("a.rs");
        assert_eq!(finder.selected_index, 0);
        assert_eq!(finder.commit_selection(), Some("deep/a.rs".to_owned()));
        assert!(!finder.show_ff_window);
        finder.toggle_window();
        finder.move_selection(1);
        finder.cancel_and_close();
        assert!(!finder.show_ff_window);
    }
    #[test]
    fn closed_finder_defers_filtering_and_merges_incremental_results_when_opened() {
        let mut finder = FileFinder::new();
        let (sender, receiver) = std::sync::mpsc::channel();
        finder.updates = Some(receiver);
        sender
            .send(WorkspaceUpdate {
                root: "/project".into(),
                generation: 1,
                indexed_files: Some(vec!["/project/z".into(), "/project/b".into()]),
                ready: true,
                ..Default::default()
            })
            .unwrap();
        assert!(finder.poll());
        assert!(finder.filtered_list.is_empty());
        sender
            .send(WorkspaceUpdate {
                root: "/project".into(),
                generation: 2,
                indexed_added: vec![
                    "/project/c".into(),
                    "/project/a".into(),
                    "/project/c".into(),
                ],
                indexed_removed: vec!["/project/b".into()],
                ready: true,
                ..Default::default()
            })
            .unwrap();
        assert!(finder.poll());
        assert!(finder.filtered_list.is_empty());
        finder.toggle_window();
        assert_eq!(
            finder
                .filtered_list
                .iter()
                .map(|file| file.relative_path.as_str())
                .collect::<Vec<_>>(),
            ["a", "c", "z"]
        );
    }
    #[test]
    fn scanner_delivers_results_to_main_thread_and_shutdown_joins() {
        let _watcher_guard = native_watcher_test_guard();
        let temp = TempDir::new();
        temp.write("main.rs", b"");
        let mut finder = FileFinder::new();
        finder.start_background_thread();
        finder.set_project_dir(temp.root().to_str().unwrap());
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if finder.poll() {
                break;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        }
        finder.toggle_window();
        assert_eq!(finder.filtered_list.len(), 1);
        assert_eq!(finder.filtered_list[0].relative_path, "main.rs");
        finder.start_background_thread();
    }
    #[cfg(unix)]
    #[test]
    fn recursive_scan_does_not_follow_directory_symlinks_and_preserves_file_link_names() {
        use std::os::unix::fs::symlink;
        let temp = TempDir::new();
        let outside = TempDir::new();
        outside.write("external.txt", b"");
        temp.write("plain.txt", b"");
        symlink(outside.root(), temp.path("external-directory")).unwrap();
        symlink(temp.path("plain.txt"), temp.path(".alias")).unwrap();
        let files = scan_file_list(temp.root()).unwrap();
        assert_eq!(files.len(), 2);
        assert!(files.iter().any(|f| f.relative_path == "plain.txt"));
        assert!(files.iter().any(|f| f.relative_path == ".alias"));
        assert_eq!(filter_file_list(&files, "").len(), 1);
    }
}

#[cfg(test)]
fn native_watcher_test_guard() -> std::sync::MutexGuard<'static, ()> {
    // FSEvents stream teardown can purge device events from another test's
    // stream. Keep tests that start native workspace services independent.
    static NATIVE_WATCHER_TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());
    NATIVE_WATCHER_TESTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod generation_tests {
    use super::*;
    use std::{
        thread,
        time::{Duration, Instant},
    };
    #[test]
    fn a_previous_workspace_cannot_restore_old_entries() {
        let _watcher_guard = native_watcher_test_guard();
        let old = crate::test_support::TempDir::new();
        let new = crate::test_support::TempDir::new();
        old.write("old", b"");
        new.write("new", b"");
        let mut finder = FileFinder::new();
        finder.start_background_thread();
        finder.set_project_dir(old.root().to_str().unwrap());
        finder.set_project_dir(new.root().to_str().unwrap());
        let deadline = Instant::now() + Duration::from_secs(20);
        while !finder.poll() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        }
        finder.toggle_window();
        assert_eq!(
            finder
                .filtered_list
                .iter()
                .map(|entry| entry.relative_path.as_str())
                .collect::<Vec<_>>(),
            ["new"]
        );
    }
    #[test]
    fn same_root_action_requests_an_immediate_scan() {
        let _watcher_guard = native_watcher_test_guard();
        let dir = crate::test_support::TempDir::new();
        dir.write("original", b"");
        let mut finder = FileFinder::new();
        finder.set_project_dir(dir.root().to_str().unwrap());
        finder.start_background_thread();
        finder.toggle_window();
        let deadline = Instant::now() + Duration::from_secs(20);
        while !finder.poll() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        }
        dir.write("created", b"");
        finder.set_project_dir(dir.root().to_str().unwrap());
        let deadline = Instant::now() + Duration::from_secs(20);
        while !finder
            .filtered_list
            .iter()
            .any(|entry| entry.relative_path == "created")
        {
            finder.poll();
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        }
    }
}
