// Translated from nealmick/ned editor/services/highlight/highlight_service.{h,cpp}.
// Source revision and attribution in NOTICE; upstream MIT/X Consortium license in NOTICE (ned section).
use crate::{
    capture_map::ThemeSlot,
    span_map::SpanMap,
    tree_sitter::{ColorSpan, ParseKind, ParseResult, ParseSnapshot, ThemeColors, TreeSitter},
};
use bed_editing::{
    editor_operations::{EditorOperations, PendingEdit},
    editor_state::EditorState,
};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::JoinHandle,
};

pub const SYNC_HIGHLIGHT_BYTES: usize = 16 * 1024;
pub const SKIP_TREE_SITTER_BYTES: usize = 100 * 1024 * 1024;
pub const PRIME_QUERY_LINES: i32 = 128;
pub const QUERY_CHUNK_LINES: i32 = 384;

pub struct EditorHighlight {
    pub enabled: bool,
    tree_sitter: Arc<Mutex<TreeSitter>>,
    cached_colors: ThemeColors,
    spans: SpanMap,
    visual_gen: u64,
    cancel_flag: Option<Arc<AtomicBool>>,
    tasks: Vec<JoinHandle<()>>,
    job_gen: u64,
    sender: mpsc::Sender<(u64, String, ParseResult)>,
    receiver: mpsc::Receiver<(u64, String, ParseResult)>,
    pending_result: Option<(u64, String, ParseResult)>,
}
impl Default for EditorHighlight {
    fn default() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            enabled: true,
            tree_sitter: Arc::new(Mutex::new(TreeSitter::new())),
            cached_colors: ThemeColors::default(),
            spans: SpanMap::default(),
            visual_gen: 1,
            cancel_flag: None,
            tasks: Vec::new(),
            job_gen: 0,
            sender,
            receiver,
            pending_result: None,
        }
    }
}
impl Drop for EditorHighlight {
    fn drop(&mut self) {
        self.cancel_highlighting();
        for task in self.tasks.drain(..) {
            let _ = task.join();
        }
    }
}
impl EditorHighlight {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn set_enabled(&mut self, enabled: bool) {
        if self.enabled != enabled {
            self.enabled = enabled;
            self.clear();
        }
    }
    pub fn start_background_prewarm() {
        TreeSitter::start_background_prewarm();
    }
    pub fn cancel_highlighting(&mut self) {
        if let Some(flag) = &self.cancel_flag {
            flag.store(true, Ordering::Relaxed);
        }
        self.reap_tasks();
    }
    fn reap_tasks(&mut self) {
        let mut i = 0;
        while i < self.tasks.len() {
            if self.tasks[i].is_finished() {
                let task = self.tasks.swap_remove(i);
                let _ = task.join();
            } else {
                i += 1;
            }
        }
    }
    pub fn set_theme_colors(&mut self, colors: ThemeColors) {
        self.cached_colors = colors;
        self.visual_gen += 1;
    }
    pub fn force_color_update(
        &mut self,
        colors: ThemeColors,
        state: &EditorState,
        operations: &mut EditorOperations,
    ) {
        self.set_theme_colors(colors);
        if state.path.is_empty() || !self.enabled {
            return;
        }
        if self.spans.lines.iter().any(|line| !line.is_empty()) {
            return;
        }
        self.highlight_content(state, operations);
    }
    pub fn default_text_color(&self) -> [f32; 4] {
        self.cached_colors.color(ThemeSlot::Text)
    }
    pub fn color_for_slot(&self, slot: ThemeSlot) -> [f32; 4] {
        self.cached_colors.color(slot)
    }
    pub fn visual_generation(&self) -> u64 {
        self.visual_gen
    }
    pub fn spans_for_line(&self, row: i32) -> &[ColorSpan] {
        self.spans.at(row)
    }
    fn sync_lens_from_content(&mut self, state: &EditorState) {
        self.spans.lens = (0..state.line_count())
            .map(|row| state.line_length(row))
            .collect();
    }
    pub fn clear(&mut self) {
        self.cancel_highlighting();
        self.job_gen += 1;
        self.visual_gen += 1;
        self.spans.clear();
        self.pending_result = None;
        while self.receiver.try_recv().is_ok() {}
    }
    pub fn reset_for_document(&mut self, state: &EditorState, line_count: usize) {
        self.cancel_highlighting();
        self.job_gen += 1;
        self.visual_gen += 1;
        self.spans.assign_empty(line_count);
        self.sync_lens_from_content(state);
        if self.spans.lens.len() != line_count {
            self.spans.lens = vec![0; line_count];
        }
        self.pending_result = None;
        while self.receiver.try_recv().is_ok() {}
    }
    fn morph_spans(&mut self, state: &EditorState, edits: &[PendingEdit]) {
        if self.spans.apply_edits(edits, &state.line_ending) {
            self.visual_gen += 1;
            return;
        }
        self.spans.lines = vec![Vec::new(); state.line_count() as usize];
        self.sync_lens_from_content(state);
        self.visual_gen += 1;
    }
    fn post_result(&mut self, generation: u64, path: String, result: ParseResult) {
        if result.kind == ParseKind::Failed {
            return;
        }
        if let Some((pending_gen, _, pending)) = &mut self.pending_result {
            if *pending_gen > generation {
                return;
            }
            if *pending_gen == generation
                && pending.kind == ParseKind::Partial
                && result.kind == ParseKind::Partial
            {
                pending.dirty_rows.extend(result.dirty_rows);
                pending.dirty_spans.extend(result.dirty_spans);
                return;
            }
        }
        self.pending_result = Some((generation, path, result));
    }
    fn collect_results(&mut self) {
        while let Ok((generation, path, result)) = self.receiver.try_recv() {
            self.post_result(generation, path, result);
        }
    }
    fn apply_parse_result(&mut self, state: &EditorState, result: ParseResult) {
        if result.kind == ParseKind::Full {
            if result.full_colors.len() != result.line_count {
                return;
            }
            self.spans.lines = result.full_colors;
            self.sync_lens_from_content(state);
            self.visual_gen += 1;
            return;
        }
        if result.kind != ParseKind::Partial {
            return;
        }
        if self.spans.lines.len() != result.line_count {
            self.spans.lines = vec![Vec::new(); result.line_count];
        }
        let n = result.dirty_rows.len().min(result.dirty_spans.len());
        for (row, spans) in result.dirty_rows.into_iter().zip(result.dirty_spans) {
            if row >= 0 && (row as usize) < self.spans.lines.len() {
                self.spans.lines[row as usize] = spans;
            }
        }
        if n > 0 {
            self.visual_gen += 1;
        }
    }
    fn publish_pending(&mut self, state: &EditorState, operations: &EditorOperations) {
        self.collect_results();
        let Some((generation, path, result)) = self.pending_result.take() else {
            return;
        };
        if generation < self.job_gen || operations.generation() != generation || state.path != path
        {
            return;
        }
        self.apply_parse_result(state, result);
    }
    fn run_sync(
        &mut self,
        snap: ParseSnapshot,
        generation: u64,
        state: &EditorState,
        operations: &EditorOperations,
    ) {
        let line_count = snap.text.line_count().max(1) as usize;
        let path = snap.path.clone();
        let mut results = Vec::new();
        self.tree_sitter.lock().unwrap().color_document(
            &snap,
            QUERY_CHUNK_LINES,
            || false,
            |result| {
                if result.kind != ParseKind::Failed && result.line_count == line_count {
                    results.push(result);
                }
            },
        );
        for result in results {
            self.post_result(generation, path.clone(), result);
        }
        self.publish_pending(state, operations);
    }
    fn launch_job(&mut self, snap: ParseSnapshot, generation: u64) {
        let canceled = Arc::new(AtomicBool::new(false));
        self.cancel_flag = Some(Arc::clone(&canceled));
        let tree_sitter = Arc::clone(&self.tree_sitter);
        let sender = self.sender.clone();
        self.tasks.push(std::thread::spawn(move || {
            if canceled.load(Ordering::Relaxed) {
                return;
            }
            let line_count = snap.text.line_count().max(1) as usize;
            let path = snap.path.clone();
            tree_sitter.lock().unwrap().color_document(
                &snap,
                QUERY_CHUNK_LINES,
                || canceled.load(Ordering::Relaxed),
                |result| {
                    if result.kind != ParseKind::Failed && result.line_count == line_count {
                        let _ = sender.send((generation, path.clone(), result));
                    }
                },
            );
        }));
    }
    fn recolor(&mut self, state: &EditorState, operations: &EditorOperations, edited: bool) {
        let bytes = state.byte_size();
        if !self.enabled || bytes > SKIP_TREE_SITTER_BYTES {
            return;
        }
        self.cancel_highlighting();
        let generation = operations.generation();
        self.job_gen = generation;
        let snap = ParseSnapshot::from_document(state);
        let tiny = bytes <= SYNC_HIGHLIGHT_BYTES;
        let ready = TreeSitter::snapshot_query_ready(&snap);
        if tiny && ready && self.tasks.is_empty() {
            self.run_sync(snap, generation, state, operations);
            return;
        }
        if !edited && ready {
            let result = TreeSitter::query_prefix(&snap, PRIME_QUERY_LINES);
            self.apply_parse_result(state, result);
        }
        self.launch_job(snap, generation);
    }
    pub fn poll(&mut self, state: &EditorState, operations: &EditorOperations) {
        self.reap_tasks();
        self.publish_pending(state, operations);
    }
    pub fn highlight_content(&mut self, state: &EditorState, operations: &mut EditorOperations) {
        let pending = operations.take_pending();
        let edited = !pending.is_empty();
        if edited {
            self.morph_spans(state, &pending);
        } else if self.spans.lines.len() != state.line_count() as usize {
            self.spans.lines = vec![Vec::new(); state.line_count() as usize];
            self.sync_lens_from_content(state);
        }
        self.recolor(state, operations, edited);
    }
}
