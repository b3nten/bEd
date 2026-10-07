//! Definition outlines from bundled grammars, independent of syntax highlighting.
//! One cancellable worker consumes only the latest immutable buffer snapshot.
use crate::{highlight_service::SKIP_TREE_SITTER_BYTES, tree_sitter::detect_language};
use bed_core::{buffer::text_buffer::Snapshot, identity::DocumentId};
use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
    ops::{ControlFlow, Range},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use tree_sitter::{
    Node, ParseOptions, Parser, Query, QueryCursor, QueryCursorOptions, StreamingIterator,
};

const EDIT_DEBOUNCE: Duration = Duration::from_millis(150);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutlineKey {
    pub document: DocumentId,
    pub generation: u64,
    pub revision: u64,
    pub path: String,
    pub language_id: String,
}
impl OutlineKey {
    fn same_source(&self, other: &Self) -> bool {
        self.document == other.document
            && self.path == other.path
            && self.language_id == other.language_id
    }
}

/// A flat, source-ordered forest. Parents precede their children; roots have no parent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutlineNode {
    /// Stable across offset changes, derived from declaration ancestry/name/kind/occurrence.
    pub id: u64,
    pub label: String,
    pub kind: String,
    pub range: Range<usize>,
    pub name_range: Range<usize>,
    pub parent: Option<usize>,
    pub depth: usize,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OutlineStatus {
    Ready,
    Unsupported,
    TooLarge,
    Failed(String),
}
#[derive(Clone, Debug)]
pub struct OutlineResult {
    pub key: OutlineKey,
    pub status: OutlineStatus,
    pub nodes: Vec<OutlineNode>,
}
struct Job {
    token: u64,
    key: OutlineKey,
    text: Snapshot,
    deadline: Instant,
}
#[derive(Default)]
struct Mailbox {
    pending: Option<Job>,
    stopped: bool,
}

pub struct OutlineService {
    mailbox: Arc<(Mutex<Mailbox>, Condvar)>,
    token: Arc<AtomicU64>,
    worker: Option<JoinHandle<()>>,
    receiver: mpsc::Receiver<OutlineResult>,
    requested: Option<OutlineKey>,
    result: Option<Arc<OutlineResult>>,
}
impl Default for OutlineService {
    fn default() -> Self {
        let mailbox = Arc::new((Mutex::new(Mailbox::default()), Condvar::new()));
        let token = Arc::new(AtomicU64::new(0));
        let (sender, receiver) = mpsc::channel();
        let worker_mailbox = Arc::clone(&mailbox);
        let worker_token = Arc::clone(&token);
        let worker = thread::spawn(move || {
            let (lock, wake) = &*worker_mailbox;
            let mut parser = Parser::new();
            let mut queries = HashMap::new();
            loop {
                let job = {
                    let mut slot = lock.lock().unwrap();
                    loop {
                        if slot.stopped {
                            return;
                        }
                        if let Some(job) = &slot.pending {
                            let now = Instant::now();
                            if now >= job.deadline {
                                break slot.pending.take().unwrap();
                            }
                            let delay = job.deadline - now;
                            slot = wake.wait_timeout(slot, delay).unwrap().0;
                        } else {
                            slot = wake.wait(slot).unwrap();
                        }
                    }
                };
                let canceled = || worker_token.load(Ordering::Relaxed) != job.token;
                if let Some(result) =
                    extract(&mut parser, &mut queries, job.key, &job.text, canceled)
                    && !canceled()
                    && sender.send(result).is_err()
                {
                    return;
                }
            }
        });
        Self {
            mailbox,
            token,
            worker: Some(worker),
            receiver,
            requested: None,
            result: None,
        }
    }
}
impl Drop for OutlineService {
    fn drop(&mut self) {
        self.token.fetch_add(1, Ordering::Relaxed);
        let (lock, wake) = &*self.mailbox;
        {
            let mut slot = lock.lock().unwrap();
            slot.stopped = true;
            slot.pending = None;
        }
        wake.notify_one();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
impl OutlineService {
    pub fn requested(&self) -> Option<&OutlineKey> {
        self.requested.as_ref()
    }
    pub fn result(&self) -> Option<&Arc<OutlineResult>> {
        self.result.as_ref()
    }
    pub fn updating(&self) -> bool {
        self.requested
            .as_ref()
            .is_some_and(|key| self.result.as_ref().is_none_or(|result| &result.key != key))
    }
    pub fn request(&mut self, key: OutlineKey, text: Snapshot) {
        if self.requested.as_ref() == Some(&key) {
            return;
        }
        let same_source = self
            .requested
            .as_ref()
            .is_some_and(|old| old.same_source(&key));
        if !same_source {
            self.result = None;
        }
        let token = self.token.fetch_add(1, Ordering::Relaxed) + 1;
        self.requested = Some(key.clone());
        let (lock, wake) = &*self.mailbox;
        lock.lock().unwrap().pending = Some(Job {
            token,
            key,
            text,
            deadline: Instant::now()
                + if same_source {
                    EDIT_DEBOUNCE
                } else {
                    Duration::ZERO
                },
        });
        wake.notify_one();
    }
    pub fn clear(&mut self) {
        if self.requested.take().is_none() {
            return;
        }
        self.token.fetch_add(1, Ordering::Relaxed);
        self.mailbox.0.lock().unwrap().pending = None;
        self.mailbox.1.notify_one();
        self.result = None;
        while self.receiver.try_recv().is_ok() {}
    }
    pub fn poll(&mut self) {
        while let Ok(result) = self.receiver.try_recv() {
            if self.requested.as_ref() == Some(&result.key) {
                self.result = Some(Arc::new(result));
            }
        }
    }
}

fn query_source(name: &str) -> &'static str {
    macro_rules! sources { ($($name:literal),* $(,)?) => { match name {
        $(concat!($name, ".scm") => include_str!(concat!("queries/", $name, ".scm")),)*
        _ => unreachable!("every bundled grammar has an outline query"),
    } }; }
    sources!(
        "c", "cpp", "csharp", "go", "java", "jsx", "tsx", "python", "rs", "rb", "kotlin", "sh",
        "json", "toml", "hcl", "html", "css"
    )
}
fn compact(text: &[u8]) -> String {
    // Bound labels without cutting UTF-8. Full source ranges remain available for navigation.
    let text = String::from_utf8_lossy(text);
    let mut label = String::new();
    let mut count = 0;
    for word in text.split_whitespace() {
        if !label.is_empty() {
            label.push(' ');
            count += 1;
        }
        for ch in word.chars() {
            if count >= 160 {
                label.push('…');
                return label;
            }
            label.push(ch);
            count += 1;
        }
    }
    label
}
fn declarator_name(mut node: Node<'_>) -> Node<'_> {
    // C/C++ declarators wrap identifiers in pointer/array/function declarators.
    while let Some(inner) = node.child_by_field_name("declarator") {
        node = inner;
    }
    node
}
fn header_range(node: Node<'_>, source: &[u8]) -> Range<usize> {
    let range = node.byte_range();
    let end = source[range.clone()]
        .iter()
        .position(|byte| matches!(byte, b'{' | b'\n' | b'\r'))
        .map_or(range.end, |offset| range.start + offset);
    range.start..end
}
fn local_value(node: Node<'_>) -> bool {
    let mut parent = node.parent();
    while let Some(node) = parent {
        if matches!(
            node.kind(),
            "function_item"
                | "function_definition"
                | "function_declaration"
                | "method_declaration"
                | "method_definition"
                | "function_expression"
                | "arrow_function"
                | "method"
                | "singleton_method"
                | "block"
                | "statement_block"
                | "lambda_literal"
        ) {
            return true;
        }
        parent = node.parent();
    }
    false
}

fn extract(
    parser: &mut Parser,
    queries: &mut HashMap<&'static str, Query>,
    key: OutlineKey,
    text: &Snapshot,
    canceled: impl Fn() -> bool,
) -> Option<OutlineResult> {
    if canceled() {
        return None;
    }
    let mut result = OutlineResult {
        key,
        status: OutlineStatus::Ready,
        nodes: Vec::new(),
    };
    if text.size() > SKIP_TREE_SITTER_BYTES {
        result.status = OutlineStatus::TooLarge;
        return Some(result);
    }
    let Some((language, query_name)) = detect_language(&result.key.language_id) else {
        result.status = OutlineStatus::Unsupported;
        return Some(result);
    };
    // The shared registry returns static, non-null bundled grammar pointers.
    let language = unsafe { tree_sitter::Language::from_raw(language) };
    if let Err(error) = parser.set_language(&language) {
        result.status = OutlineStatus::Failed(error.to_string());
        return Some(result);
    }
    if !queries.contains_key(query_name) {
        match Query::new(&language, query_source(query_name)) {
            Ok(query) => {
                queries.insert(query_name, query);
            }
            Err(error) => {
                result.status = OutlineStatus::Failed(error.to_string());
                return Some(result);
            }
        }
    }
    let mut source = text.bytes();
    // Some grammars treat bare CR as ordinary text (including inside comments).
    // Normalize it to LF without changing any byte offsets used for navigation.
    for index in 0..source.len() {
        if index % 65536 == 0 && canceled() {
            return None;
        }
        if source[index] == b'\r' && source.get(index + 1) != Some(&b'\n') {
            source[index] = b'\n';
        }
    }
    if canceled() {
        return None;
    }
    let mut progress = |_: &tree_sitter::ParseState| {
        if canceled() {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    };
    let tree = parser.parse_with_options(
        &mut |offset, _| &source[offset..],
        None,
        Some(ParseOptions::new().progress_callback(&mut progress)),
    );
    let Some(tree) = tree else {
        // A canceled parse must not resume with a different buffer on the next job.
        parser.reset();
        if canceled() {
            return None;
        }
        result.status = OutlineStatus::Failed("Could not parse this buffer".into());
        return Some(result);
    };
    let query = &queries[query_name];
    let mut cursor = QueryCursor::new();
    let mut progress = |_: &tree_sitter::QueryCursorState| {
        if canceled() {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    };
    let mut matches = cursor.matches_with_options(
        query,
        tree.root_node(),
        source.as_slice(),
        QueryCursorOptions::new().progress_callback(&mut progress),
    );
    let mut entry_indices = HashMap::new();
    while let Some(found) = matches.next() {
        if canceled() {
            return None;
        }
        let name = found
            .captures
            .iter()
            .find(|capture| query.capture_names()[capture.index as usize] == "name")
            .map(|capture| capture.node);
        for capture in found.captures {
            let Some(kind) = query.capture_names()[capture.index as usize].strip_prefix("outline.")
            else {
                continue;
            };
            let node = capture.node;
            if (kind == "constant" || (query_name == "kotlin.scm" && kind == "field"))
                && local_value(node)
            {
                continue;
            }
            if kind == "constant"
                && node.child_by_field_name("value").is_some_and(|value| {
                    matches!(
                        value.kind(),
                        "arrow_function" | "function_expression" | "generator_function"
                    )
                })
            {
                continue;
            }
            let name = name.map(declarator_name);
            let name_range =
                name.map_or_else(|| header_range(node, &source), |name| name.byte_range());
            if name_range.is_empty() {
                continue;
            }
            let label = if matches!(kind, "impl" | "block")
                || (query_name == "css.scm" && node.kind() != "rule_set")
            {
                compact(&source[header_range(node, &source)])
            } else if query_name == "json.scm" && kind == "key" {
                match serde_json::from_slice::<String>(&source[name_range.clone()]) {
                    Ok(name) => {
                        let label = compact(name.as_bytes());
                        if label.is_empty() {
                            compact(&source[name_range.clone()])
                        } else {
                            label
                        }
                    }
                    Err(_) => compact(&source[name_range.clone()]),
                }
            } else if kind == "entry" {
                if !entry_indices.contains_key(&node.id())
                    && let Some(parent) = node.parent()
                {
                    let mut walk = parent.walk();
                    for (index, child) in parent.named_children(&mut walk).enumerate() {
                        if canceled() {
                            return None;
                        }
                        entry_indices.insert(child.id(), index);
                    }
                }
                let index = entry_indices.get(&node.id()).copied().unwrap_or(0);
                format!("[{index}]")
            } else if query_name == "html.scm" {
                format!("<{}>", compact(&source[name_range.clone()]))
            } else {
                compact(&source[name_range.clone()])
            };
            if label.is_empty() {
                continue;
            }
            let mut range = node.byte_range();
            if node.kind() == "file_scoped_namespace_declaration" {
                range.end = tree.root_node().end_byte();
            }
            result.nodes.push(OutlineNode {
                id: 0,
                label,
                kind: kind.into(),
                range,
                name_range,
                parent: None,
                depth: 0,
            });
        }
    }
    drop(matches);
    if cursor.did_exceed_match_limit() {
        result.status = OutlineStatus::Failed("Outline query match limit reached".into());
        result.nodes.clear();
        return Some(result);
    }
    result.nodes.sort_by(|a, b| {
        a.range
            .start
            .cmp(&b.range.start)
            .then(b.range.end.cmp(&a.range.end))
            .then(a.name_range.start.cmp(&b.name_range.start))
    });
    result
        .nodes
        .dedup_by(|a, b| a.range == b.range && a.name_range == b.name_range);
    let mut stack: Vec<usize> = Vec::new();
    let mut occurrences = HashMap::new();
    for index in 0..result.nodes.len() {
        if canceled() {
            return None;
        }
        while let Some(&parent) = stack.last() {
            let a = &result.nodes[parent].range;
            let b = &result.nodes[index].range;
            if a.start <= b.start && b.end <= a.end && a != b {
                break;
            }
            stack.pop();
        }
        let parent = stack.last().copied();
        let parent_id = parent.map_or(0, |parent| result.nodes[parent].id);
        let node = &mut result.nodes[index];
        node.parent = parent;
        node.depth = stack.len();
        let occurrence = occurrences
            .entry((parent_id, node.kind.clone(), node.label.clone()))
            .or_insert(0usize);
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        (parent_id, &node.kind, &node.label, *occurrence).hash(&mut hasher);
        node.id = hasher.finish();
        *occurrence += 1;
        stack.push(index);
    }
    Some(result)
}

#[cfg(test)]
mod tests;
