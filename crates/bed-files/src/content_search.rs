//! Cancellable project content search with incremental result batches.
//! Non-overlapping byte ranges, ASCII case folding, and immutable buffer overlays.
//! Git-aware discovery is a Bed policy; matching and capped loading retain byte semantics.
use std::sync::mpsc;
use std::thread;
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc::{Receiver, Sender, SyncSender, TrySendError},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use crate::{file_finder::FileEntry, files::read_file_raw};
use bed_editing::{
    editor_state::EditorState,
    text_search::{CompiledSearch, SearchOptions},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContentMatch {
    pub file: FileEntry,
    /// Zero-based position in the loaded editor and original source file.
    pub row: i32,
    pub column: i32,
    /// Zero-based source-file line, used in the result label.
    pub source_row: i32,
    pub line: Arc<[u8]>,
    /// Zero-based, exclusive byte range within `line`.
    pub range: std::ops::Range<usize>,
}

#[derive(Clone, Debug)]
pub struct SearchBuffer {
    pub path: String,
    /// None asks discovery to report eligibility without reading disk contents.
    pub bytes: Option<Arc<[u8]>>,
}

enum WorkerMessage {
    Search {
        generation: u64,
        root: String,
        query: Vec<u8>,
        options: SearchOptions,
        buffers: Vec<SearchBuffer>,
        remote: Option<bed_remote::RemoteClient>,
    },
    Stop,
}
struct SearchResult {
    generation: u64,
    matches: Vec<ContentMatch>,
    skipped_files: usize,
    error: Option<String>,
    progress: SearchProgress,
    finished: bool,
    limit_reached: bool,
    eligible_buffer_paths: Vec<String>,
}

/// Stop collecting before a broad query consumes unbounded UI memory.
pub const MAX_SEARCH_RESULTS: usize = 100_000;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SearchProgress {
    pub discovered_files: usize,
    pub scanned_files: usize,
    /// Pruned paths (a skipped directory counts once, without walking it).
    pub ignored_paths: usize,
    pub discovering: bool,
}

/// Only the main thread mutates UI state. Bounded worker batches carry a
/// generation, so replacements/cancellation discard partial and final results.
pub struct ContentSearch {
    remote_client: Option<bed_remote::RemoteClient>,
    has_remote_requests: bool,
    pub active: bool,
    pub query: String,
    pub case_sensitive: bool,
    pub regex: bool,
    pub whole_words: bool,
    pub include: String,
    pub exclude: String,
    pub searching: bool,
    pub results: Vec<ContentMatch>,
    pub selected_index: usize,
    pub skipped_files: usize,
    pub error: Option<String>,
    pub include_ignored: bool,
    pub progress: SearchProgress,
    pub canceled: bool,
    pub limit_reached: bool,
    generation: Arc<AtomicU64>,
    requests: Sender<WorkerMessage>,
    completed: Receiver<SearchResult>,
    worker: Option<JoinHandle<()>>,
}

impl Default for ContentSearch {
    fn default() -> Self {
        let mut search = Self::new_lazy();
        search.start_worker();
        search
    }
}

impl ContentSearch {
    /// Construct inert UI state; embedded document widgets do not start project workers.
    pub fn new_lazy() -> Self {
        let (requests, _) = mpsc::channel();
        let (_, completed) = mpsc::channel();
        Self {
            remote_client: None,
            has_remote_requests: false,
            active: false,
            query: String::new(),
            case_sensitive: false,
            regex: false,
            whole_words: false,
            include: String::new(),
            exclude: String::new(),
            searching: false,
            results: Vec::new(),
            selected_index: 0,
            skipped_files: 0,
            error: None,
            include_ignored: false,
            progress: SearchProgress::default(),
            canceled: false,
            limit_reached: false,
            generation: Arc::new(AtomicU64::new(0)),
            requests,
            completed,
            worker: None,
        }
    }
    fn start_worker(&mut self) {
        if self.worker.is_some() {
            return;
        }
        let (requests, pending) = mpsc::channel();
        let (results, completed) = mpsc::sync_channel(8);
        let current_generation = Arc::clone(&self.generation);
        let worker = thread::spawn(move || {
            while let Ok(mut request) = pending.recv() {
                // A replaced query should not wait behind a queue of obsolete searches.
                for next in pending.try_iter() {
                    request = next;
                }
                let WorkerMessage::Search {
                    generation,
                    root,
                    query,
                    options,
                    buffers,
                    remote,
                } = request
                else {
                    break;
                };
                let result = if let Some(client) = &remote {
                    run_remote_search(
                        client,
                        &root,
                        &query,
                        &options,
                        &buffers,
                        generation,
                        &current_generation,
                    )
                } else {
                    run_search_project_with_options(
                        &root,
                        &query,
                        &options,
                        &buffers,
                        MAX_SEARCH_RESULTS,
                        generation,
                        &current_generation,
                        |update| send_update(&results, &current_generation, update),
                    )
                };
                if let Some(result) = result {
                    send_update(&results, &current_generation, result);
                }
            }
        });
        self.requests = requests;
        self.completed = completed;
        self.worker = Some(worker);
    }
    pub fn new() -> Self {
        Self::default()
    }
    pub fn set_remote_client(&mut self, client: Option<bed_remote::RemoteClient>) {
        self.cancel();
        self.results.clear();
        self.remote_client = client;
    }
    pub fn open(&mut self) {
        self.active = true;
    }
    pub fn dismiss(&mut self) {
        self.active = false;
        self.cancel();
    }
    pub fn cancel(&mut self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
        self.canceled = self.searching;
        self.searching = false;
        self.progress.discovering = false;
    }
    pub fn options(&self) -> SearchOptions {
        SearchOptions {
            case_sensitive: self.case_sensitive,
            regex: self.regex,
            whole_words: self.whole_words,
            include_ignored: self.include_ignored,
            include: self.include.clone(),
            exclude: self.exclude.clone(),
        }
    }
    pub fn start(&mut self, root: &str, query: &str, case_sensitive: bool) {
        self.case_sensitive = case_sensitive;
        self.start_with_buffers(root, query, Vec::new());
    }
    pub fn start_with_buffers(&mut self, root: &str, query: &str, buffers: Vec<SearchBuffer>) {
        if !query.is_empty() {
            self.start_worker();
        }
        let generation = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        self.query = query.to_owned();
        self.results.clear();
        self.selected_index = 0;
        self.skipped_files = 0;
        self.error = None;
        self.canceled = false;
        self.limit_reached = false;
        self.progress = SearchProgress {
            discovering: !query.is_empty(),
            ..SearchProgress::default()
        };
        self.searching = !query.is_empty();
        self.has_remote_requests |= self.searching && self.remote_client.is_some();
        if self.searching
            && self
                .requests
                .send(WorkerMessage::Search {
                    generation,
                    root: root.to_owned(),
                    query: query.as_bytes().to_vec(),
                    options: self.options(),
                    buffers,
                    remote: self.remote_client.clone(),
                })
                .is_err()
        {
            self.searching = false;
            self.error = Some("Content-search worker stopped".to_owned());
        }
    }
    /// Accept only the latest request. Canceled and previous-project results never reach the UI.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        for result in self.completed.try_iter() {
            if result.generation == self.generation.load(Ordering::Relaxed) {
                self.results.extend(result.matches);
                self.skipped_files = result.skipped_files;
                self.error = result.error;
                self.progress = result.progress;
                self.searching = !result.finished;
                if result.finished {
                    self.results.sort_by(|a, b| {
                        (&a.file.relative_path, a.row, a.range.start).cmp(&(
                            &b.file.relative_path,
                            b.row,
                            b.range.start,
                        ))
                    });
                }
                self.limit_reached = result.limit_reached;
                self.selected_index = self
                    .selected_index
                    .min(self.results.len().saturating_sub(1));
                changed = true;
            }
        }
        changed
    }
}

impl Drop for ContentSearch {
    fn drop(&mut self) {
        self.cancel();
        let _ = self.requests.send(WorkerMessage::Stop);
        if let Some(worker) = self.worker.take()
            && (!self.has_remote_requests || worker.is_finished())
        {
            let _ = worker.join();
        }
    }
}

fn run_remote_search(
    client: &bed_remote::RemoteClient,
    root: &str,
    query: &[u8],
    options: &SearchOptions,
    buffers: &[SearchBuffer],
    generation: u64,
    current_generation: &AtomicU64,
) -> Option<SearchResult> {
    if current_generation.load(Ordering::Relaxed) != generation {
        return None;
    }
    let response = client.call(bed_remote::Request::Search {
        root: root.into(),
        query: String::from_utf8_lossy(query).into_owned(),
        options: options.clone(),
        buffer_paths: buffers.iter().map(|buffer| buffer.path.clone()).collect(),
        max_results: MAX_SEARCH_RESULTS,
    });
    if current_generation.load(Ordering::Relaxed) != generation {
        return None;
    }
    let mut result = SearchResult {
        generation,
        matches: Vec::new(),
        skipped_files: 0,
        error: None,
        progress: SearchProgress::default(),
        finished: true,
        limit_reached: false,
        eligible_buffer_paths: Vec::new(),
    };
    match response {
        Ok(bed_remote::Response::Search {
            matches,
            truncated,
            scanned_files,
            discovered_files,
            ignored_paths,
            skipped_files,
            eligible_buffer_paths,
        }) => {
            result.limit_reached = truncated;
            result.skipped_files = skipped_files;
            result.progress = SearchProgress {
                scanned_files,
                discovered_files,
                ignored_paths,
                discovering: false,
            };
            for found in matches {
                if found.range.start > found.range.end
                    || found.range.end > found.line_bytes.len()
                    || found.column != found.range.start + 1
                {
                    result.error = Some("Invalid match range in remote search response".into());
                    break;
                }
                match FileEntry::from_remote_path(&found.path, root) {
                    Ok(file) => result.matches.push(ContentMatch {
                        file,
                        row: found.editor_row.saturating_sub(1).min(i32::MAX as usize) as i32,
                        source_row: found.line.saturating_sub(1).min(i32::MAX as usize) as i32,
                        column: found.column.saturating_sub(1).min(i32::MAX as usize) as i32,
                        range: found.range,
                        line: Arc::from(found.line_bytes),
                    }),
                    Err(error) => {
                        result.error = Some(error.to_string());
                        break;
                    }
                }
            }
            if let Ok(matcher) = CompiledSearch::new(query, options) {
                for buffer in buffers
                    .iter()
                    .filter(|buffer| eligible_buffer_paths.contains(&buffer.path))
                {
                    let Some(bytes) = &buffer.bytes else { continue };
                    let Ok(file) = FileEntry::from_remote_path(&buffer.path, root) else {
                        continue;
                    };
                    append_buffer_matches(&mut result.matches, &file, bytes, &matcher);
                }
                if result.matches.len() > MAX_SEARCH_RESULTS {
                    result.matches.truncate(MAX_SEARCH_RESULTS);
                    result.limit_reached = true;
                }
            }
        }
        Ok(_) => result.error = Some("Unexpected remote content-search response".into()),
        Err(error) => result.error = Some(error.to_string()),
    }
    Some(result)
}

/// Headless callers reuse the editor's search policy and exact byte positions.
pub struct SearchSnapshot {
    pub matches: Vec<ContentMatch>,
    pub skipped_files: usize,
    pub error: Option<String>,
    pub progress: SearchProgress,
    pub limit_reached: bool,
    pub eligible_buffer_paths: Vec<String>,
}
pub fn search_project_snapshot(
    root: &str,
    query: &[u8],
    case_sensitive: bool,
    include_ignored: bool,
    max_results: usize,
) -> SearchSnapshot {
    search_project_snapshot_with_options(
        root,
        query,
        &SearchOptions {
            case_sensitive,
            include_ignored,
            ..Default::default()
        },
        &[],
        max_results,
    )
}
pub fn search_project_snapshot_with_options(
    root: &str,
    query: &[u8],
    options: &SearchOptions,
    buffer_paths: &[String],
    max_results: usize,
) -> SearchSnapshot {
    let buffers: Vec<_> = buffer_paths
        .iter()
        .map(|path| SearchBuffer {
            path: path.clone(),
            bytes: None,
        })
        .collect();
    let mut matches = Vec::new();
    let mut eligible_buffer_paths = Vec::new();
    let result = run_search_project_with_options(
        root,
        query,
        options,
        &buffers,
        max_results.clamp(1, MAX_SEARCH_RESULTS),
        1,
        &AtomicU64::new(1),
        |batch| {
            matches.extend(batch.matches);
            eligible_buffer_paths.extend(batch.eligible_buffer_paths);
            true
        },
    )
    .expect("uncanceled synchronous search");
    matches.extend(result.matches);
    eligible_buffer_paths.extend(result.eligible_buffer_paths);
    let limit_reached = result.limit_reached || matches.len() > max_results;
    matches.truncate(max_results);
    SearchSnapshot {
        matches,
        skipped_files: result.skipped_files,
        error: result.error,
        progress: result.progress,
        limit_reached,
        eligible_buffer_paths,
    }
}

fn send_update(
    sender: &SyncSender<SearchResult>,
    generation: &AtomicU64,
    mut update: SearchResult,
) -> bool {
    loop {
        if generation.load(Ordering::Relaxed) != update.generation {
            return false;
        }
        match sender.try_send(update) {
            Ok(()) => return true,
            Err(TrySendError::Disconnected(_)) => return false,
            Err(TrySendError::Full(pending)) => update = pending,
        }
        // Backpressure remains cancellable even when a hidden consumer stops
        // polling; Drop increments the generation before joining the worker.
        thread::sleep(Duration::from_millis(1));
    }
}

#[allow(clippy::too_many_arguments)] // One immutable worker request plus its cancellation/output seams.
fn run_search_project_with_options(
    root: &str,
    query: &[u8],
    options: &SearchOptions,
    buffers: &[SearchBuffer],
    max_results: usize,
    generation: u64,
    current_generation: &AtomicU64,
    mut publish: impl FnMut(SearchResult) -> bool,
) -> Option<SearchResult> {
    let canceled = || current_generation.load(Ordering::Relaxed) != generation;
    if canceled() {
        return None;
    }
    let mut result = SearchResult {
        generation,
        matches: Vec::new(),
        skipped_files: 0,
        error: None,
        progress: SearchProgress {
            discovering: true,
            ..SearchProgress::default()
        },
        finished: true,
        limit_reached: false,
        eligible_buffer_paths: Vec::new(),
    };
    if query.is_empty() {
        result.progress.discovering = false;
        return Some(result);
    }
    let matcher = match CompiledSearch::new(query, options) {
        Ok(matcher) => matcher,
        Err(error) => {
            result.error = Some(error);
            result.progress.discovering = false;
            return Some(result);
        }
    };
    let buffers: std::collections::HashMap<_, _> = buffers
        .iter()
        .filter_map(|buffer| {
            std::fs::canonicalize(&buffer.path)
                .ok()
                .map(|path| (path, buffer))
        })
        .collect();
    let buffer_for = |file: &FileEntry| {
        let path = Path::new(&file.full_path);
        buffers.get(path).copied().or_else(|| {
            if buffers.is_empty() || !path.is_symlink() {
                return None;
            }
            std::fs::canonicalize(path)
                .ok()
                .and_then(|path| buffers.get(&path).copied())
        })
    };
    let mut last_update = Instant::now();
    let files = match super::search_files::discover(
        Path::new(root),
        options.include_ignored,
        canceled,
        |files, ignored| {
            result.progress.discovered_files = files;
            result.progress.ignored_paths = ignored;
            if last_update.elapsed() >= Duration::from_millis(40) {
                last_update = Instant::now();
                publish(batch(&mut result))
            } else {
                true
            }
        },
    ) {
        Ok(Some(files)) => files,
        Ok(None) => return None,
        Err(error) => {
            result.error = Some(error.to_string());
            result.progress.discovering = false;
            return (!canceled()).then_some(result);
        }
    };
    result.progress.discovering = false;
    if !publish(batch(&mut result)) {
        return None;
    }
    let files: Vec<_> = files
        .into_iter()
        .filter(|file| matcher.includes_path(&file.relative_path))
        .collect();
    result.progress.discovered_files = files.len();
    // Eligibility must be complete even when collecting results later reaches a limit.
    result.eligible_buffer_paths = files
        .iter()
        .filter(|file| buffer_for(file).is_some())
        .map(|file| file.full_path.clone())
        .collect();
    let mut total_matches = 0;
    for file in files {
        if canceled() {
            return None;
        }
        let overlay = buffer_for(&file);
        if overlay.is_some_and(|buffer| buffer.bytes.is_none()) {
            result.progress.scanned_files += 1;
            continue;
        }
        let raw = match overlay
            .and_then(|buffer| buffer.bytes.as_ref())
            .map(|bytes| crate::files::ReadFile {
                raw: bytes.to_vec(),
            })
            .map(Ok)
            .unwrap_or_else(|| read_file_raw(Path::new(&file.full_path)))
        {
            Ok(raw) => raw,
            Err(_) => {
                result.skipped_files += 1;
                result.progress.scanned_files += 1;
                if last_update.elapsed() >= Duration::from_millis(40) {
                    last_update = Instant::now();
                    if !publish(batch(&mut result)) {
                        return None;
                    }
                }
                continue;
            }
        };
        // Reuse the exact document splitter/BOM convention without building
        // and then walking an editable AVL rope for each read-only file.
        let content = raw.raw.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&raw.raw);
        let (lines, _) = EditorState::split_lines(content);
        for (row, line) in lines.into_iter().enumerate() {
            if canceled() {
                return None;
            }
            let mut shared_line = None;
            for range in matcher.matches(&line) {
                if canceled() {
                    return None;
                }
                let column = range.start;
                let shared = shared_line.get_or_insert_with(|| Arc::<[u8]>::from(line.as_slice()));
                result.matches.push(ContentMatch {
                    file: file.clone(),
                    row: row as i32,
                    column: column as i32,
                    source_row: row as i32,
                    line: Arc::clone(shared),
                    range,
                });
                total_matches += 1;
                if total_matches >= max_results {
                    result.limit_reached = true;
                    result.progress.scanned_files += 1;
                    return (!canceled()).then_some(result);
                }
                if result.matches.len() >= 1024 && !publish(batch(&mut result)) {
                    return None;
                }
            }
        }
        result.progress.scanned_files += 1;
        if last_update.elapsed() >= Duration::from_millis(40) {
            last_update = Instant::now();
            if !publish(batch(&mut result)) {
                return None;
            }
        }
    }
    (!canceled()).then_some(result)
}

fn batch(result: &mut SearchResult) -> SearchResult {
    SearchResult {
        generation: result.generation,
        matches: std::mem::take(&mut result.matches),
        skipped_files: result.skipped_files,
        error: result.error.clone(),
        progress: result.progress,
        finished: false,
        limit_reached: false,
        eligible_buffer_paths: std::mem::take(&mut result.eligible_buffer_paths),
    }
}

fn append_buffer_matches(
    matches: &mut Vec<ContentMatch>,
    file: &FileEntry,
    bytes: &[u8],
    matcher: &CompiledSearch,
) {
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    let (lines, _) = EditorState::split_lines(bytes);
    for (row, line) in lines.into_iter().enumerate() {
        let line: Arc<[u8]> = Arc::from(line);
        for range in matcher.matches(&line) {
            matches.push(ContentMatch {
                file: file.clone(),
                row: row as i32,
                source_row: row as i32,
                column: range.start as i32,
                range,
                line: Arc::clone(&line),
            });
            if matches.len() > MAX_SEARCH_RESULTS {
                return;
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn run_search_project(
    root: &str,
    query: &[u8],
    case_sensitive: bool,
    include_ignored: bool,
    max_results: usize,
    generation: u64,
    current_generation: &AtomicU64,
    publish: impl FnMut(SearchResult) -> bool,
) -> Option<SearchResult> {
    run_search_project_with_options(
        root,
        query,
        &SearchOptions {
            case_sensitive,
            include_ignored,
            ..Default::default()
        },
        &[],
        max_results,
        generation,
        current_generation,
        publish,
    )
}

#[cfg(test)]
fn search_project(
    root: &str,
    query: &[u8],
    case_sensitive: bool,
    generation: u64,
    current_generation: &AtomicU64,
) -> Option<SearchResult> {
    let mut matches = Vec::new();
    let mut result = run_search_project(
        root,
        query,
        case_sensitive,
        false,
        MAX_SEARCH_RESULTS,
        generation,
        current_generation,
        |update| {
            matches.extend(update.matches);
            true
        },
    )?;
    matches.extend(result.matches);
    result.matches = matches;
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{files::MAX_FILE_SIZE, test_support::TempDir};
    use std::time::{Duration, Instant};

    #[test]
    fn synchronous_headless_snapshot_reuses_byte_matching_and_result_limit() {
        let temp = TempDir::new();
        temp.write("file", b"\xff ababa");
        let result =
            search_project_snapshot(temp.root().to_str().unwrap(), b"aba", false, false, 1);
        assert!(result.error.is_none());
        assert!(result.limit_reached);
        assert_eq!(result.matches.len(), 1);
        assert_eq!(result.matches[0].column, 2);
        assert_eq!(&*result.matches[0].line, b"\xff ababa");
    }

    fn search(temp: &TempDir, query: &[u8], case_sensitive: bool) -> SearchResult {
        search_project(
            temp.root().to_str().unwrap(),
            query,
            case_sensitive,
            1,
            &AtomicU64::new(1),
        )
        .unwrap()
    }
    #[test]
    fn byte_columns_nonoverlap_ascii_case_and_file_order_are_stable() {
        let temp = TempDir::new();
        temp.write("longer.rs", b"ABA\r\nnext aba");
        temp.write("a", "éababa ABABA\rthird aba\n".as_bytes());
        let result = search(&temp, b"aba", false);
        let positions: Vec<_> = result
            .matches
            .iter()
            .map(|found| (found.file.relative_path.as_str(), found.row, found.column))
            .collect();
        assert_eq!(
            positions,
            vec![
                ("a", 0, 2),
                ("a", 0, 8),
                ("a", 1, 6),
                ("longer.rs", 0, 0),
                ("longer.rs", 1, 5)
            ]
        );
        assert_eq!(search(&temp, b"aba", true).matches.len(), 3);
        let unicode = search(&temp, "é".as_bytes(), false);
        assert_eq!((unicode.matches[0].row, unicode.matches[0].column), (0, 0));
        assert!(search(&temp, "É".as_bytes(), false).matches.is_empty());
        assert_eq!(&*result.matches[0].line, "éababa ABABA".as_bytes());
        assert!(Arc::ptr_eq(
            &result.matches[0].line,
            &result.matches[1].line
        ));
    }
    #[test]
    fn bom_binary_hidden_and_oversized_files_keep_source_positions() {
        let temp = TempDir::new();
        temp.write(".hidden", b"\xef\xbb\xbfneedle");
        temp.write("binary", b"\0needle");
        let large = temp.write("large", b"needle");
        std::fs::File::options()
            .write(true)
            .open(&large)
            .unwrap()
            .set_len(MAX_FILE_SIZE as u64 + 1)
            .unwrap();
        let result = search(&temp, b"needle", false);
        assert_eq!(result.skipped_files, 2);
        assert_eq!(result.matches.len(), 1);
        let hidden = result
            .matches
            .iter()
            .find(|found| found.file.relative_path == ".hidden")
            .unwrap();
        assert_eq!((hidden.row, hidden.column), (0, 0));
        assert!(search(&temp, b"File truncated", false).matches.is_empty());
    }
    #[test]
    fn canceled_generation_is_discarded_and_missing_project_is_reported() {
        let temp = TempDir::new();
        temp.write("file", b"needle");
        assert!(
            search_project(
                temp.root().to_str().unwrap(),
                b"needle",
                false,
                1,
                &AtomicU64::new(2)
            )
            .is_none()
        );
        let result = search_project(
            temp.path("missing").to_str().unwrap(),
            b"needle",
            false,
            1,
            &AtomicU64::new(1),
        )
        .unwrap();
        assert!(result.error.is_some());
        assert!(result.matches.is_empty());
    }
    #[test]
    fn replacement_and_cancel_do_not_publish_stale_results() {
        let temp = TempDir::new();
        temp.write("file", b"old needle\nnew target");
        let mut search = ContentSearch::new();
        search.start(temp.root().to_str().unwrap(), "needle", false);
        search.cancel();
        search.start(temp.root().to_str().unwrap(), "target", false);
        let deadline = Instant::now() + Duration::from_secs(5);
        while search.searching && Instant::now() < deadline {
            search.poll();
            thread::sleep(Duration::from_millis(1));
        }
        assert!(!search.searching);
        assert!(search.error.is_none());
        assert_eq!(search.results.len(), 1);
        assert_eq!((search.results[0].row, search.results[0].column), (1, 4));
        search.start(temp.root().to_str().unwrap(), "", false);
        assert!(!search.searching);
        assert!(search.results.is_empty());
    }
    #[test]
    fn literal_byte_matching_agrees_with_document_lines_and_ascii_windows() {
        let temp = TempDir::new();
        temp.write(
            "file",
            b"\xef\xbb\xbfAaAa .*[\xff\r\n\xe9\x9b\xaa aAaA\r\nend\r\xff\0 text\n",
        );
        let raw = read_file_raw(&temp.path("file")).unwrap();
        let mut state = EditorState::new();
        state.set_from_bytes(&raw.raw);
        for query in [
            b"aa".as_slice(),
            b"AaA",
            b".*[",
            b"\xff",
            b"\0",
            "雪".as_bytes(),
            "É".as_bytes(),
            b"\r",
            b"\n",
        ] {
            for case_sensitive in [false, true] {
                let actual = search(&temp, query, case_sensitive);
                let mut expected = Vec::new();
                for (row, line) in state.lines().iter().enumerate() {
                    let mut end = 0;
                    for (column, part) in line.windows(query.len()).enumerate() {
                        if column >= end
                            && ((case_sensitive && part == query)
                                || (!case_sensitive && part.eq_ignore_ascii_case(query)))
                        {
                            end = column + query.len();
                            expected.push((row as i32, column as i32));
                        }
                    }
                }
                assert_eq!(
                    actual
                        .matches
                        .iter()
                        .map(|found| (found.row, found.column))
                        .collect::<Vec<_>>(),
                    expected,
                    "query {query:?}, case_sensitive {case_sensitive}"
                );
            }
        }
    }
    #[test]
    fn files_beyond_the_old_limit_keep_bom_and_mixed_newline_source_positions() {
        let temp = TempDir::new();
        let mut raw = b"\xef\xbb\xbfneedle\r\nneedle\rneedle\n".to_vec();
        raw.resize(2 * 1024 * 1024, b'x');
        temp.write("large", &raw);
        let result = search(&temp, b"needle", false);
        assert_eq!(
            result
                .matches
                .iter()
                .map(|found| (found.source_row, found.row, found.column))
                .collect::<Vec<_>>(),
            vec![(0, 0, 0), (1, 1, 0), (2, 2, 0)]
        );
    }
    #[test]
    fn include_ignored_worker_option_reveals_files_but_never_git_metadata() {
        let temp = TempDir::new();
        git2::Repository::init(temp.root()).unwrap();
        temp.write(".gitignore", b"ignored/\n");
        temp.write(".git/needle", b"needle");
        temp.write("ignored/file", b"needle");
        temp.write("visible", b"needle");
        let mut search = ContentSearch::new();
        for include_ignored in [false, true] {
            search.include_ignored = include_ignored;
            search.start(temp.root().to_str().unwrap(), "needle", false);
            let deadline = Instant::now() + Duration::from_secs(5);
            while search.searching {
                search.poll();
                assert!(Instant::now() < deadline);
                thread::sleep(Duration::from_millis(1));
            }
            assert!(search.error.is_none());
            assert_eq!(search.results.len(), if include_ignored { 2 } else { 1 });
            assert!(
                search
                    .results
                    .iter()
                    .all(|found| !found.file.relative_path.starts_with(".git/"))
            );
            assert_eq!(
                search.progress.scanned_files,
                search.progress.discovered_files
            );
        }
    }
    #[test]
    fn broad_queries_publish_partial_batches_and_stop_at_visible_limit() {
        let temp = TempDir::new();
        temp.write("many", &vec![b'a'; MAX_SEARCH_RESULTS + 1]);
        let mut batches = Vec::new();
        let final_result = run_search_project(
            temp.root().to_str().unwrap(),
            b"a",
            true,
            false,
            MAX_SEARCH_RESULTS,
            1,
            &AtomicU64::new(1),
            |batch| {
                assert!(!batch.finished);
                batches.push(batch);
                true
            },
        )
        .unwrap();
        assert!(batches.iter().any(|batch| !batch.matches.is_empty()));
        assert!(final_result.finished && final_result.limit_reached);
        assert_eq!(final_result.progress.scanned_files, 1);
        let found: Vec<_> = batches
            .into_iter()
            .flat_map(|batch| batch.matches)
            .chain(final_result.matches)
            .collect();
        assert_eq!(found.len(), MAX_SEARCH_RESULTS);
        assert_eq!(found.first().unwrap().column, 0);
        assert_eq!(
            found.last().unwrap().column as usize,
            MAX_SEARCH_RESULTS - 1
        );
        assert!(Arc::ptr_eq(
            &found[0].line,
            &found[MAX_SEARCH_RESULTS - 1].line
        ));
    }
    #[test]
    fn a_full_output_channel_can_be_canceled_without_a_polling_consumer() {
        let (sender, _receiver) = mpsc::sync_channel(0);
        let generation = Arc::new(AtomicU64::new(1));
        let worker_generation = Arc::clone(&generation);
        let (entered, ready) = mpsc::channel();
        let (done, completed) = mpsc::channel();
        let worker = thread::spawn(move || {
            entered.send(()).unwrap();
            let sent = send_update(
                &sender,
                &worker_generation,
                SearchResult {
                    generation: 1,
                    matches: Vec::new(),
                    skipped_files: 0,
                    error: None,
                    progress: SearchProgress::default(),
                    finished: true,
                    limit_reached: false,
                    eligible_buffer_paths: Vec::new(),
                },
            );
            done.send(sent).unwrap();
        });
        ready.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(
            completed.recv_timeout(Duration::from_millis(10)),
            Err(mpsc::RecvTimeoutError::Timeout)
        );
        generation.store(2, Ordering::Relaxed);
        assert!(!completed.recv_timeout(Duration::from_secs(2)).unwrap());
        worker.join().unwrap();
    }
}

#[cfg(test)]
mod options_tests {
    use super::*;
    use crate::test_support::TempDir;
    #[test]
    fn filtered_buffer_overlays_replace_disk_results_and_obey_ignore_rules() {
        let temp = TempDir::new();
        git2::Repository::init(temp.root()).unwrap();
        temp.write(".gitignore", b"ignored.rs\n");
        let main = temp.write("src/main.rs", b"disk=1\n");
        let ignored = temp.write("ignored.rs", b"disk=1\n");
        temp.write("src/other.rs", b"disk=22\n");
        temp.write("src/excluded.rs", b"disk=33\n");
        let options = SearchOptions {
            regex: true,
            include: "*.rs".into(),
            exclude: "excluded.rs".into(),
            ..Default::default()
        };
        let buffers = vec![
            SearchBuffer {
                path: main.to_str().unwrap().into(),
                bytes: Some(Arc::from(b"buffer=42\n".as_slice())),
            },
            SearchBuffer {
                path: ignored.to_str().unwrap().into(),
                bytes: Some(Arc::from(b"buffer=99\n".as_slice())),
            },
        ];
        let mut found = Vec::new();
        let result = run_search_project_with_options(
            temp.root().to_str().unwrap(),
            b"\\w+=(\\d+)",
            &options,
            &buffers,
            MAX_SEARCH_RESULTS,
            1,
            &AtomicU64::new(1),
            |batch| {
                found.extend(batch.matches);
                true
            },
        )
        .unwrap();
        found.extend(result.matches);
        assert_eq!(found.len(), 2);
        let main = found
            .iter()
            .find(|found| found.file.relative_path == "src/main.rs")
            .unwrap();
        assert_eq!(&*main.line, b"buffer=42");
        assert_eq!(main.range, 0..9);
        assert!(
            found
                .iter()
                .all(|found| found.file.relative_path != "ignored.rs")
        );
    }
}
