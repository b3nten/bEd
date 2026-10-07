//! Translated from ned files/file_finder.{h,cpp}; see LICENSE and NOTICE.
//! The worker sends complete snapshots; only the UI thread mutates displayed results.
use std::sync::mpsc;
use std::thread;
use std::{
    fs, io,
    path::{Component, Path, PathBuf},
    sync::mpsc::{Receiver, Sender},
    thread::JoinHandle,
    time::{Duration, Instant},
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
    // std::filesystem::relative uses weakly_canonical; both entries exist here.
    let path = fs::canonicalize(path)?;
    let root = fs::canonicalize(root)?;
    let a: Vec<_> = path.components().collect();
    let b: Vec<_> = root.components().collect();
    if a.first() != b.first() {
        return Ok(PathBuf::new());
    }
    let common = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let mut out = PathBuf::new();
    for part in &b[common..] {
        if matches!(part, Component::Normal(_)) {
            out.push("..");
        }
    }
    for part in &a[common..] {
        out.push(part.as_os_str());
    }
    Ok(out)
}
/// Include every regular file, including .git and dot directories. Upstream applies no ignore files.
pub fn scan_file_list(project_dir: &Path) -> io::Result<Vec<FileEntry>> {
    fn scan(dir: &Path, root: &Path, out: &mut Vec<FileEntry>) -> io::Result<()> {
        for item in fs::read_dir(dir)? {
            let entry = item?;
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let Ok(metadata) = fs::metadata(&path) else {
                continue;
            };
            if metadata.is_file()
                && let Ok(file) = FileEntry::from_path(&path, root)
            {
                out.push(file);
            }
            // Recursive iterator defaults do not follow directory symlinks.
            if kind.is_dir() {
                scan(&path, root, out)?;
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    scan(project_dir, project_dir, &mut files)?;
    Ok(files)
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
enum WorkerMessage {
    Project(String, u64),
    Stop,
}
struct ScanResult {
    root: String,
    generation: u64,
    files: Vec<FileEntry>,
}
#[derive(Default)]
pub struct FileFinder {
    remote_client: Option<bed_remote::RemoteClient>,
    pub show_ff_window: bool,
    pub search_buffer: String,
    pub selected_index: usize,
    pub filtered_list: Vec<FileEntry>,
    previous_search: String,
    file_list: Vec<FileEntry>,
    current_project_dir: String,
    generation: u64,
    worker_sender: Option<Sender<WorkerMessage>>,
    worker_receiver: Option<Receiver<ScanResult>>,
    worker: Option<JoinHandle<()>>,
}
impl FileFinder {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn set_remote_client(&mut self, client: Option<bed_remote::RemoteClient>) {
        let was_started = self.worker.is_some();
        self.stop_worker();
        self.remote_client = client;
        self.file_list.clear();
        self.filtered_list.clear();
        self.generation = self.generation.wrapping_add(1);
        if was_started {
            self.start_background_thread();
        }
    }
    fn stop_worker(&mut self) {
        if let Some(sender) = self.worker_sender.take() {
            let _ = sender.send(WorkerMessage::Stop);
        }
        self.worker_receiver.take();
        if let Some(worker) = self.worker.take()
            && (self.remote_client.is_none() || worker.is_finished())
        {
            let _ = worker.join();
        }
    }
    pub fn start_background_thread(&mut self) {
        if self.worker.is_some() {
            return;
        }
        let (sender, requests) = mpsc::channel();
        let (results, receiver) = mpsc::channel();
        self.worker_sender = Some(sender);
        self.worker_receiver = Some(receiver);
        let remote = self.remote_client.clone();
        self.worker = Some(thread::spawn(move || {
            let mut project = String::new();
            let mut scanned = String::new();
            let mut generation = 0;
            let mut force_scan = false;
            let mut last_scan = Instant::now();
            loop {
                match requests.recv_timeout(Duration::from_millis(100)) {
                    Ok(WorkerMessage::Project(root, next)) => {
                        project = root;
                        generation = next;
                        force_scan = true;
                    }
                    Ok(WorkerMessage::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
                if !project.is_empty()
                    && (force_scan
                        || project != scanned
                        || last_scan.elapsed() >= Duration::from_secs(3))
                {
                    scanned = project.clone();
                    force_scan = false;
                    last_scan = Instant::now();
                    let scan = if let Some(client) = &remote {
                        client
                            .call(bed_remote::Request::ListFiles {
                                root: project.clone(),
                            })
                            .and_then(|response| match response {
                                bed_remote::Response::Files { paths } => paths
                                    .into_iter()
                                    .map(|path| FileEntry::from_remote_path(&path, &project))
                                    .collect(),
                                _ => Err(io::Error::new(
                                    io::ErrorKind::InvalidData,
                                    "Unexpected remote file-list response",
                                )),
                            })
                    } else {
                        scan_file_list(Path::new(&project))
                    };
                    if let Ok(files) = scan
                        && results
                            .send(ScanResult {
                                root: project.clone(),
                                generation,
                                files,
                            })
                            .is_err()
                    {
                        break;
                    }
                }
            }
        }));
        if !self.current_project_dir.is_empty() {
            let _ = self
                .worker_sender
                .as_ref()
                .unwrap()
                .send(WorkerMessage::Project(
                    self.current_project_dir.clone(),
                    self.generation,
                ));
        }
    }
    pub fn set_project_dir(&mut self, root: &str) {
        if self.current_project_dir != root {
            self.file_list.clear();
            self.filtered_list.clear();
            self.search_buffer.clear();
            self.previous_search.clear();
            self.selected_index = 0;
        }
        self.current_project_dir = root.to_owned();
        self.generation = self.generation.wrapping_add(1);
        if let Some(sender) = &self.worker_sender {
            let _ = sender.send(WorkerMessage::Project(root.to_owned(), self.generation));
        }
    }
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        if let Some(receiver) = &self.worker_receiver {
            for result in receiver.try_iter() {
                if result.root == self.current_project_dir && result.generation == self.generation {
                    self.file_list = result.files;
                    changed = true;
                }
            }
        }
        if changed {
            self.update_filtered_list();
        }
        changed
    }
    pub fn update_filtered_list(&mut self) {
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
        if self.search_buffer.to_ascii_lowercase() != self.previous_search {
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
    fn scanner_includes_git_and_does_not_apply_gitignore() {
        let temp = TempDir::new();
        temp.write(".git/config", b"");
        temp.write(".gitignore", b"ignored\n");
        temp.write("ignored/file.txt", b"");
        temp.write("src/main.rs", b"");
        let files = scan_file_list(temp.root()).unwrap();
        assert_eq!(files.len(), 4);
        assert!(files.iter().any(|f| f.relative_path == "ignored/file.txt"));
        assert!(files.iter().any(|f| f.relative_path == ".git/config"));
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
    fn scanner_delivers_results_to_main_thread_and_shutdown_joins() {
        let temp = TempDir::new();
        temp.write("main.rs", b"");
        let mut finder = FileFinder::new();
        finder.start_background_thread();
        finder.set_project_dir(temp.root().to_str().unwrap());
        let deadline = Instant::now() + Duration::from_secs(5);
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
    fn recursive_scan_does_not_follow_directory_symlinks_and_relative_paths_resolve_file_links() {
        use std::os::unix::fs::symlink;
        let temp = TempDir::new();
        let outside = TempDir::new();
        outside.write("external.txt", b"");
        temp.write("plain.txt", b"");
        symlink(outside.root(), temp.path("external-directory")).unwrap();
        symlink(temp.path("plain.txt"), temp.path(".alias")).unwrap();
        let files = scan_file_list(temp.root()).unwrap();
        assert_eq!(files.len(), 2);
        assert!(files.iter().all(|f| f.relative_path == "plain.txt"));
        assert_eq!(filter_file_list(&files, "").len(), 2);
    }
}

#[cfg(test)]
mod generation_tests {
    use super::*;
    #[test]
    fn a_stale_same_root_scan_cannot_restore_renamed_entries() {
        let mut finder = FileFinder::new();
        finder.set_project_dir("/project");
        let old = finder.generation;
        finder.set_project_dir("/project");
        let (tx, rx) = mpsc::channel();
        finder.worker_receiver = Some(rx);
        tx.send(ScanResult {
            root: "/project".into(),
            generation: old,
            files: vec![FileEntry {
                relative_path: "old".into(),
                full_path: "/project/old".into(),
                ..Default::default()
            }],
        })
        .unwrap();
        assert!(!finder.poll());
        assert!(finder.filtered_list.is_empty());
        tx.send(ScanResult {
            root: "/project".into(),
            generation: finder.generation,
            files: vec![FileEntry {
                relative_path: "new".into(),
                full_path: "/project/new".into(),
                ..Default::default()
            }],
        })
        .unwrap();
        assert!(finder.poll());
        assert_eq!(finder.filtered_list[0].relative_path, "new");
        finder.set_project_dir("/different");
        assert!(finder.filtered_list.is_empty());
    }
    #[test]
    fn same_root_action_requests_an_immediate_scan() {
        let dir = crate::test_support::TempDir::new();
        dir.write("original", b"");
        let mut finder = FileFinder::new();
        finder.set_project_dir(dir.root().to_str().unwrap());
        finder.start_background_thread();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !finder.poll() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        dir.write("created", b"");
        finder.set_project_dir(dir.root().to_str().unwrap());
        let deadline = Instant::now() + Duration::from_secs(2);
        while !finder
            .filtered_list
            .iter()
            .any(|entry| entry.relative_path == "created")
        {
            finder.poll();
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
