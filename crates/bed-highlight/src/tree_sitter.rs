// Translated from nealmick/ned editor/services/highlight/tree_sitter.{h,cpp}.
// Source revision and attribution in NOTICE; upstream MIT/X Consortium license in NOTICE (ned section).
//! Snapshot parsing, incremental tree edits, and prioritized query captures.
use crate::capture_map::{
    THEME_KEYS, ThemeSlot, capture_priority, is_none_capture, is_string_capture, subtract_ranges,
    theme_slot_for_capture,
};
use bed_editing::{
    buffer::text_buffer::Snapshot,
    editor_operations::{OpKind, PendingEdit},
    editor_state::EditorState,
};
use std::{
    collections::{BTreeMap, HashMap},
    ffi::{c_char, c_void},
    path::{Path, PathBuf},
    ptr,
    sync::{Arc, Condvar, Mutex, Once, OnceLock},
};
use tree_sitter::ffi;

#[derive(Clone, Debug, PartialEq)]
pub struct ThemeColors {
    pub slots: [[f32; 4]; 14],
}
impl Default for ThemeColors {
    fn default() -> Self {
        let mut slots = [[0.85, 0.85, 0.85, 1.0]; 14];
        slots[ThemeSlot::Comment as usize] = [0.5, 0.5, 0.5, 1.0];
        Self { slots }
    }
}
impl ThemeColors {
    pub fn from_settings(settings: &serde_json::Value) -> Self {
        let mut colors = Self::default();
        let name = settings
            .get("theme")
            .and_then(|v| v.as_str())
            .unwrap_or("default");
        let Some(theme) = settings
            .get("themes")
            .and_then(|v| v.get(name))
            .filter(|v| v.is_object())
        else {
            return colors;
        };
        let load = |key: &str, fallback: [f32; 4]| -> [f32; 4] {
            let Some(a) = theme
                .get(key)
                .and_then(|v| v.as_array())
                .filter(|a| a.len() >= 4)
            else {
                return fallback;
            };
            let mut out = [0.; 4];
            for (i, c) in out.iter_mut().enumerate() {
                let Some(n) = a[i].as_f64() else {
                    return fallback;
                };
                *c = n as f32;
            }
            out
        };
        colors.slots[0] = load("text", colors.slots[0]);
        colors.slots[1] = load("comment", colors.slots[1]);
        let text = colors.slots[0];
        for slot in [
            ThemeSlot::Keyword,
            ThemeSlot::String,
            ThemeSlot::Number,
            ThemeSlot::Function,
            ThemeSlot::Type,
            ThemeSlot::Variable,
            ThemeSlot::Operator,
            ThemeSlot::Punctuation,
        ] {
            colors.slots[slot as usize] = load(THEME_KEYS[slot as usize], text);
        }
        for (slot, fallback) in [
            (ThemeSlot::Parameter, ThemeSlot::Variable),
            (ThemeSlot::Property, ThemeSlot::Variable),
            (ThemeSlot::Constant, ThemeSlot::Number),
            (ThemeSlot::Special, ThemeSlot::Keyword),
        ] {
            colors.slots[slot as usize] =
                load(THEME_KEYS[slot as usize], colors.slots[fallback as usize]);
        }
        colors
    }
    pub fn color(&self, slot: ThemeSlot) -> [f32; 4] {
        self.slots[slot as usize]
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ColorSpan {
    pub start: i32,
    pub end: i32,
    pub slot: ThemeSlot,
}
pub type LineColorSpans = Vec<ColorSpan>;
pub type ColorRangeMap = Vec<LineColorSpans>;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ParseKind {
    #[default]
    Failed,
    Full,
    Partial,
    TreeOnly,
}
#[derive(Debug, Default)]
pub struct ParseResult {
    pub kind: ParseKind,
    pub line_count: usize,
    pub full_colors: ColorRangeMap,
    pub dirty_rows: Vec<i32>,
    pub dirty_spans: Vec<LineColorSpans>,
}
#[derive(Clone, Debug, Default)]
pub struct ParseSnapshot {
    pub text: Snapshot,
    pub line_ending: Vec<u8>,
    pub line_starts: Vec<u32>,
    pub path: String,
    pub language_id: String,
    pub pending_edits: Vec<PendingEdit>,
    pub byte_limit: u32,
}
impl ParseSnapshot {
    pub fn from_document(state: &EditorState, pending_edits: Vec<PendingEdit>) -> Self {
        Self {
            text: state.snapshot(),
            line_ending: state.line_ending.clone(),
            path: state.path.clone(),
            language_id: state.language_id.clone(),
            pending_edits,
            ..Self::default()
        }
    }
    fn size(&self) -> usize {
        if self.byte_limit != 0 {
            self.byte_limit as usize
        } else {
            self.text.size()
        }
    }
    fn line_length(&self, row: usize) -> i32 {
        let Some(&start) = self.line_starts.get(row) else {
            return 0;
        };
        if let Some(&next) = self.line_starts.get(row + 1) {
            (next as usize - start as usize - self.line_ending.len()) as i32
        } else {
            (self.size() - start as usize) as i32
        }
    }
    fn row_col(&self, offset: usize) -> (i32, i32) {
        if self.line_starts.is_empty() {
            return (0, 0);
        }
        let off = offset.min(self.size());
        let (mut lo, mut hi) = (0, self.line_starts.len() - 1);
        while lo < hi {
            let mid = lo + (hi - lo).div_ceil(2);
            if self.line_starts[mid] as usize <= off {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        let begin = self.line_starts[lo] as usize;
        let end = self
            .line_starts
            .get(lo + 1)
            .map_or(self.size(), |&s| s as usize - self.line_ending.len());
        if off <= end {
            (lo as i32, (off - begin) as i32)
        } else if lo + 1 < self.line_starts.len() {
            (lo as i32 + 1, 0)
        } else {
            (lo as i32, self.line_length(lo))
        }
    }
    fn node_text(&self, node: ffi::TSNode) -> Vec<u8> {
        // Nodes belong to the tree kept alive under the engine lock throughout query.
        let (a, b) = unsafe { (ffi::ts_node_start_byte(node), ffi::ts_node_end_byte(node)) };
        if b <= a || a as usize >= self.text.size() {
            return Vec::new();
        }
        let len = (b - a) as usize;
        let len = len.min(self.text.size() - a as usize);
        let mut out = vec![0; len];
        self.text.copy_bytes(a as usize, len, &mut out);
        out
    }
}

unsafe extern "C" {
    fn free(ptr: *mut c_void);
}
type LanguageFn = unsafe extern "C" fn() -> *const ();
extern "C" fn kotlin_language() -> *const () {
    // This published binding exposes Language rather than a LanguageFn constant.
    tree_sitter_kotlin_codanna::language().into_raw().cast()
}
const LANGS: &[(&[&str], LanguageFn, &str)] = &[
    (&["c"], tree_sitter_c::LANGUAGE.into_raw(), "c.scm"),
    (
        &["cpp", "c++", "h", "hpp", "mm", "cc", "cxx"],
        tree_sitter_cpp::LANGUAGE.into_raw(),
        "cpp.scm",
    ),
    (
        &["js", "jsx", "javascript"],
        tree_sitter_javascript::LANGUAGE.into_raw(),
        "jsx.scm",
    ),
    (
        &["py", "python"],
        tree_sitter_python::LANGUAGE.into_raw(),
        "python.scm",
    ),
    (
        &["cs", "csharp"],
        tree_sitter_c_sharp::LANGUAGE.into_raw(),
        "csharp.scm",
    ),
    (
        &["html", "cshtml"],
        tree_sitter_html::LANGUAGE.into_raw(),
        "html.scm",
    ),
    (
        &["tsx", "ts", "typescript"],
        tree_sitter_typescript::LANGUAGE_TSX.into_raw(),
        "tsx.scm",
    ),
    (&["css"], tree_sitter_css::LANGUAGE.into_raw(), "css.scm"),
    (&["java"], tree_sitter_java::LANGUAGE.into_raw(), "java.scm"),
    (
        &["go", "golang"],
        tree_sitter_go::LANGUAGE.into_raw(),
        "go.scm",
    ),
    (
        &["tf", "hcl"],
        tree_sitter_hcl::LANGUAGE.into_raw(),
        "hcl.scm",
    ),
    (&["json"], tree_sitter_json::LANGUAGE.into_raw(), "json.scm"),
    (
        &["sh", "bash"],
        tree_sitter_bash::LANGUAGE.into_raw(),
        "sh.scm",
    ),
    (&["kt", "kts"], kotlin_language, "kotlin.scm"),
    (
        &["rs", "rust"],
        tree_sitter_rust::LANGUAGE.into_raw(),
        "rs.scm",
    ),
    (
        &["toml"],
        tree_sitter_toml_ng::LANGUAGE.into_raw(),
        "toml.scm",
    ),
    (&["rb"], tree_sitter_ruby::LANGUAGE.into_raw(), "rb.scm"),
];
pub(crate) fn detect_language(id: &str) -> Option<(*const ffi::TSLanguage, &'static str)> {
    let id = id.strip_prefix('.').unwrap_or(id);
    LANGS
        .iter()
        .find(|(ids, _, _)| ids.contains(&id))
        .map(|(_, lang, query)| (unsafe { lang() }.cast(), *query))
}
fn bundled_query(name: &str) -> &'static str {
    macro_rules! source {
        ($f:literal) => {
            include_str!(concat!(
                concat!(env!("CARGO_MANIFEST_DIR"), "/../.."),
                "/resources/queries/",
                $f
            ))
        };
    }
    match name {
        "c.scm" => source!("c.scm"),
        "cpp.scm" => source!("cpp.scm"),
        "jsx.scm" => source!("jsx.scm"),
        "python.scm" => source!("python.scm"),
        "csharp.scm" => source!("csharp.scm"),
        "html.scm" => source!("html.scm"),
        "tsx.scm" => source!("tsx.scm"),
        "css.scm" => source!("css.scm"),
        "java.scm" => source!("java.scm"),
        "go.scm" => source!("go.scm"),
        "hcl.scm" => source!("hcl.scm"),
        "json.scm" => source!("json.scm"),
        "sh.scm" => source!("sh.scm"),
        "kotlin.scm" => source!("kotlin.scm"),
        "rs.scm" => source!("rs.scm"),
        "toml.scm" => source!("toml.scm"),
        "rb.scm" => source!("rb.scm"),
        _ => "",
    }
}
fn query_file(name: &str) -> PathBuf {
    query_file_for_executable(name, std::env::current_exe().ok().as_deref())
}
fn query_file_for_executable(name: &str, executable: Option<&Path>) -> PathBuf {
    let mut candidates = Vec::new();
    if let Some(exe) = executable
        && let Some(parent) = exe.parent()
    {
        candidates.push(parent.join("queries").join(name));
        // Packaged queries share the resources tree used by fonts and config.
        // Keep executable-local overrides ahead of the installed resources.
        candidates.push(parent.join("../share/Bed/resources/queries").join(name));
        if let Some(contents) = parent.parent() {
            candidates.push(contents.join("Resources/resources/queries").join(name));
        }
    }
    candidates.extend([
        PathBuf::from("queries").join(name),
        PathBuf::from("resources/queries").join(name),
    ]);
    if cfg!(target_os = "linux") {
        candidates.push(PathBuf::from("/usr/share/Bed/resources/queries").join(name));
    }
    candidates
        .into_iter()
        .find(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from("bundled").join(name))
}
struct QueryHandle(*mut ffi::TSQuery);
// TSQuery is immutable after construction. Every execution owns its TSQueryCursor.
unsafe impl Send for QueryHandle {}
unsafe impl Sync for QueryHandle {}
impl Drop for QueryHandle {
    fn drop(&mut self) {
        unsafe {
            ffi::ts_query_delete(self.0);
        }
    }
}
type QueryWaiter = Arc<(Mutex<Option<Option<Arc<QueryHandle>>>>, Condvar)>;
#[derive(Default)]
struct QueryCache {
    compiled: HashMap<PathBuf, Arc<QueryHandle>>,
    inflight: HashMap<PathBuf, QueryWaiter>,
}
fn query_cache() -> &'static Mutex<QueryCache> {
    static CACHE: OnceLock<Mutex<QueryCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(QueryCache::default()))
}
fn load_query(lang: *const ffi::TSLanguage, name: &str) -> Option<Arc<QueryHandle>> {
    load_query_source(lang, name, false)
}
fn load_query_source(
    lang: *const ffi::TSLanguage,
    name: &str,
    bundled: bool,
) -> Option<Arc<QueryHandle>> {
    // Builtins have a separate cache identity, without executable/cwd probes.
    let path = if bundled {
        PathBuf::from("bed-builtin-query").join(name)
    } else {
        query_file(name)
    };
    let (waiter, owner) = {
        let mut cache = query_cache().lock().unwrap();
        if let Some(q) = cache.compiled.get(&path) {
            return Some(Arc::clone(q));
        }
        if let Some(w) = cache.inflight.get(&path) {
            (Arc::clone(w), false)
        } else {
            let w = Arc::new((Mutex::new(None), Condvar::new()));
            cache.inflight.insert(path.clone(), Arc::clone(&w));
            (w, true)
        }
    };
    if !owner {
        let (lock, cv) = &*waiter;
        let mut value = lock.lock().unwrap();
        while value.is_none() {
            value = cv.wait(value).unwrap();
        }
        return value.as_ref().unwrap().clone();
    }
    let source = if bundled {
        bundled_query(name).to_owned()
    } else {
        std::fs::read_to_string(&path).unwrap_or_else(|_| bundled_query(name).to_owned())
    };
    let (mut offset, mut error) = (0, 0);
    let raw = unsafe {
        ffi::ts_query_new(
            lang,
            source.as_ptr().cast(),
            source.len() as u32,
            &mut offset,
            &mut error,
        )
    };
    let q = if raw.is_null() {
        eprintln!("Query error ({error}) at offset {offset}: {name}");
        None
    } else {
        Some(Arc::new(QueryHandle(raw)))
    };
    {
        let mut cache = query_cache().lock().unwrap();
        if let Some(q) = &q {
            cache.compiled.insert(path.clone(), Arc::clone(q));
        }
        cache.inflight.remove(&path);
    }
    let (lock, cv) = &*waiter;
    *lock.lock().unwrap() = Some(q.clone());
    cv.notify_all();
    q
}

fn regex_match(text: &[u8], pattern: &str) -> bool {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<regex::bytes::Regex>>>> = OnceLock::new();
    let mut cache = CACHE
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap();
    let re = cache.entry(pattern.to_owned()).or_insert_with(|| {
        // The 22 shipped patterns use the common ECMAScript/Rust regex subset. Dot
        // excludes CR as well as LF in upstream's ECMAScript byte expressions.
        let pattern = pattern
            .replace(".*", "[^\\r\\n]*")
            .replace(".+", "[^\\r\\n]+");
        regex::bytes::RegexBuilder::new(&pattern)
            .unicode(false)
            .build()
            .ok()
    });
    re.as_ref().is_some_and(|re| re.is_match(text))
}
fn captures(m: &ffi::TSQueryMatch) -> &[ffi::TSQueryCapture] {
    if m.capture_count == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(m.captures, m.capture_count as usize) }
    }
}
fn query_string(query: *const ffi::TSQuery, id: u32) -> Vec<u8> {
    let mut n = 0;
    let p = unsafe { ffi::ts_query_string_value_for_id(query, id, &mut n) };
    if n == 0 {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(p.cast::<u8>(), n as usize) }.to_vec()
    }
}
fn predicates_pass(
    query: *const ffi::TSQuery,
    m: &ffi::TSQueryMatch,
    snap: &ParseSnapshot,
) -> bool {
    let mut count = 0;
    let p =
        unsafe { ffi::ts_query_predicates_for_pattern(query, m.pattern_index as u32, &mut count) };
    if p.is_null() || count == 0 {
        return true;
    }
    let steps = unsafe { std::slice::from_raw_parts(p, count as usize) };
    let mut i = 0;
    while i < steps.len() {
        if steps[i].type_ == ffi::TSQueryPredicateStepTypeDone {
            i += 1;
            continue;
        }
        if steps[i].type_ != ffi::TSQueryPredicateStepTypeString {
            while i < steps.len() && steps[i].type_ != ffi::TSQueryPredicateStepTypeDone {
                i += 1;
            }
            if i < steps.len() {
                i += 1;
            }
            continue;
        }
        let op = query_string(query, steps[i].value_id);
        i += 1;
        let mut args = Vec::new();
        while i < steps.len() && steps[i].type_ != ffi::TSQueryPredicateStepTypeDone {
            args.push((
                steps[i].type_ == ffi::TSQueryPredicateStepTypeCapture,
                steps[i].value_id,
            ));
            i += 1;
        }
        if i < steps.len() {
            i += 1;
        }
        let arg_text = |(is_capture, id): (bool, u32)| -> Vec<u8> {
            if is_capture {
                captures(m)
                    .iter()
                    .find(|c| c.index == id)
                    .map_or_else(Vec::new, |c| snap.node_text(c.node))
            } else {
                query_string(query, id)
            }
        };
        match op.as_slice() {
            b"match?" | b"not-match?" => {
                if args.len() < 2 || !args[0].0 || args[1].0 {
                    continue;
                }
                let text = arg_text(args[0]);
                let pat = arg_text(args[1]);
                let matched = std::str::from_utf8(&pat).is_ok_and(|p| regex_match(&text, p));
                if (op == b"match?" && !matched) || (op == b"not-match?" && matched) {
                    return false;
                }
            }
            b"eq?" | b"not-eq?" => {
                if args.len() < 2 {
                    continue;
                }
                let equal = arg_text(args[0]) == arg_text(args[1]);
                if (op == b"eq?" && !equal) || (op == b"not-eq?" && equal) {
                    return false;
                }
            }
            b"any-of?" => {
                if args.len() < 2 || !args[0].0 {
                    continue;
                }
                let text = arg_text(args[0]);
                if !args[1..].iter().any(|&a| arg_text(a) == text) {
                    return false;
                }
            }
            _ => {} // Upstream deliberately ignores unsupported host-specific predicates.
        }
    }
    true
}

pub struct TreeSitter {
    parser: *mut ffi::TSParser,
    tree: *mut ffi::TSTree,
    tree_file_path: String,
    tree_language_id: String,
    tree_doc_bytes: usize,
    last_committed_gen: u64,
    input_scratch: Vec<u8>,
    bundled_queries: bool,
}
// The parser/tree are owned exclusively; callers serialize jobs with one Mutex.
unsafe impl Send for TreeSitter {}
impl Default for TreeSitter {
    fn default() -> Self {
        let parser = unsafe { ffi::ts_parser_new() };
        assert!(!parser.is_null());
        Self {
            parser,
            tree: ptr::null_mut(),
            tree_file_path: String::new(),
            tree_language_id: String::new(),
            tree_doc_bytes: 0,
            last_committed_gen: 0,
            input_scratch: Vec::new(),
            bundled_queries: false,
        }
    }
}
impl Drop for TreeSitter {
    fn drop(&mut self) {
        unsafe {
            ffi::ts_tree_delete(self.tree);
            ffi::ts_parser_delete(self.parser);
        }
    }
}
struct InputState<'a> {
    text: &'a Snapshot,
    scratch: &'a mut Vec<u8>,
}
unsafe extern "C" fn read_snapshot(
    payload: *mut c_void,
    byte_index: u32,
    _point: ffi::TSPoint,
    bytes_read: *mut u32,
) -> *const c_char {
    // The synchronous C parse callback never escapes its input-state stack frame.
    let state = unsafe { &mut *payload.cast::<InputState<'_>>() };
    let off = byte_index as usize;
    if off >= state.text.size() {
        unsafe {
            *bytes_read = 0;
        }
        return c"".as_ptr();
    }
    let want = 4096.min(state.text.size() - off);
    state.scratch.resize(want, 0);
    state.text.copy_bytes(off, want, state.scratch);
    unsafe {
        *bytes_read = want as u32;
    }
    state.scratch.as_ptr().cast()
}
impl TreeSitter {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn use_bundled_queries(&mut self) {
        self.bundled_queries = true;
    }
    pub fn query_ready(&self, id: &str) -> bool {
        Self::is_query_ready_with_source(id, self.bundled_queries)
    }
    /// Checks the shared query cache without locking a parser or waiting for a worker.
    pub fn is_query_ready(id: &str) -> bool {
        Self::is_query_ready_with_source(id, false)
    }
    pub(crate) fn is_query_ready_with_source(id: &str, bundled: bool) -> bool {
        let Some((_, name)) = detect_language(id) else {
            return true;
        };
        query_cache()
            .lock()
            .unwrap()
            .compiled
            .contains_key(&if bundled {
                PathBuf::from("bed-builtin-query").join(name)
            } else {
                query_file(name)
            })
    }
    pub fn start_background_prewarm() {
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            std::thread::spawn(|| {
                for (_, language, name) in LANGS {
                    load_query(unsafe { language() }.cast(), name);
                }
            });
        });
    }
    pub fn compile_all_queries() -> Result<(), String> {
        for (_, language, name) in LANGS {
            let lang = unsafe { language() }.cast();
            let version = unsafe { ffi::ts_language_abi_version(lang) };
            if !(ffi::TREE_SITTER_MIN_COMPATIBLE_LANGUAGE_VERSION
                ..=ffi::TREE_SITTER_LANGUAGE_VERSION)
                .contains(&version)
            {
                return Err(format!("{name} grammar ABI {version} is incompatible"));
            }
            if load_query(lang, name).is_none() {
                return Err(format!("Unable to compile {name}"));
            }
        }
        Ok(())
    }
    fn merge_adjacent(spans: &mut LineColorSpans) {
        if spans.len() < 2 {
            return;
        }
        let mut out: LineColorSpans = Vec::with_capacity(spans.len());
        out.push(spans[0]);
        for &s in &spans[1..] {
            let prev = out.last_mut().unwrap();
            if prev.end == s.start && prev.slot == s.slot {
                prev.end = s.end;
            } else {
                out.push(s);
            }
        }
        *spans = out;
    }
    fn set_range(spans: &mut LineColorSpans, start: i32, end: i32, slot: ThemeSlot) {
        if start >= end {
            return;
        }
        let mut out = Vec::with_capacity(spans.len() + 2);
        let mut inserted = false;
        for &s in spans.iter() {
            if s.end <= start {
                out.push(s);
                continue;
            }
            if s.start >= end {
                if !inserted {
                    out.push(ColorSpan { start, end, slot });
                    inserted = true;
                }
                out.push(s);
                continue;
            }
            if s.start < start {
                out.push(ColorSpan {
                    start: s.start,
                    end: start,
                    slot: s.slot,
                });
            }
            if !inserted {
                out.push(ColorSpan { start, end, slot });
                inserted = true;
            }
            if s.end > end {
                out.push(ColorSpan {
                    start: end,
                    end: s.end,
                    slot: s.slot,
                });
            }
        }
        if !inserted {
            out.push(ColorSpan { start, end, slot });
        }
        Self::merge_adjacent(&mut out);
        *spans = out;
    }
    fn fill_range(
        snap: &ParseSnapshot,
        colors: &mut ColorRangeMap,
        row_base: i32,
        start: u32,
        end: u32,
        slot: ThemeSlot,
    ) {
        if start >= end {
            return;
        }
        let (sr, sc) = snap.row_col(start as usize);
        let (er, ec) = snap.row_col(end as usize);
        for row in sr..=er {
            let idx = row - row_base;
            if idx < 0 || idx as usize >= colors.len() {
                continue;
            }
            let len = snap.line_length(row as usize);
            let a = if row == sr { sc } else { 0 }.clamp(0, len);
            let b = if row == er { ec } else { len }.clamp(0, len);
            if a < b {
                Self::set_range(&mut colors[idx as usize], a, b, slot);
            }
        }
    }
    fn run_query(
        query: &QueryHandle,
        tree: *mut ffi::TSTree,
        snap: &ParseSnapshot,
        byte_start: u32,
        byte_end: u32,
        colors: &mut ColorRangeMap,
        row_base: i32,
    ) {
        #[derive(Clone)]
        struct Hit {
            start: u32,
            end: u32,
            priority: i32,
            capture: String,
        }
        let (mut hits, mut holes) = (Vec::with_capacity(256), Vec::new());
        let cursor = unsafe { ffi::ts_query_cursor_new() };
        unsafe {
            if byte_end > byte_start {
                ffi::ts_query_cursor_set_byte_range(cursor, byte_start, byte_end);
            }
            ffi::ts_query_cursor_exec(cursor, query.0, ffi::ts_tree_root_node(tree));
        }
        let mut m = ffi::TSQueryMatch {
            id: 0,
            pattern_index: 0,
            capture_count: 0,
            captures: ptr::null(),
        };
        while unsafe { ffi::ts_query_cursor_next_match(cursor, &mut m) } {
            if !predicates_pass(query.0, &m, snap) {
                continue;
            }
            for capture in captures(&m) {
                let mut len = 0;
                let name =
                    unsafe { ffi::ts_query_capture_name_for_id(query.0, capture.index, &mut len) };
                let name = unsafe { std::slice::from_raw_parts(name.cast::<u8>(), len as usize) };
                let name = std::str::from_utf8(name).unwrap_or("");
                if name.starts_with('_') {
                    continue;
                }
                let (a, b) = unsafe {
                    (
                        ffi::ts_node_start_byte(capture.node),
                        ffi::ts_node_end_byte(capture.node),
                    )
                };
                if is_none_capture(name) {
                    if a < b {
                        holes.push((a, b));
                    }
                    continue;
                }
                let priority = capture_priority(name);
                if priority <= 10 {
                    continue;
                }
                hits.push(Hit {
                    start: a,
                    end: b,
                    priority,
                    capture: name.to_owned(),
                });
            }
        }
        unsafe {
            ffi::ts_query_cursor_delete(cursor);
        }
        holes.sort_unstable();
        let mut merged: Vec<(u32, u32)> = Vec::new();
        for hole in holes {
            if let Some(last) = merged.last_mut()
                && hole.0 <= last.1
            {
                last.1 = last.1.max(hole.1);
            } else {
                merged.push(hole);
            }
        }
        let mut expanded = Vec::with_capacity(hits.len() + merged.len());
        for h in hits {
            if is_string_capture(&h.capture) && !merged.is_empty() {
                for (a, b) in subtract_ranges(h.start, h.end, &merged) {
                    expanded.push(Hit {
                        start: a,
                        end: b,
                        ..h.clone()
                    });
                }
            } else {
                expanded.push(h);
            }
        }
        expanded.sort_by_key(|h| (h.priority, h.start));
        for h in expanded {
            Self::fill_range(
                snap,
                colors,
                row_base,
                h.start,
                h.end,
                theme_slot_for_capture(&h.capture),
            );
        }
    }
    fn query_window_into(
        query: &QueryHandle,
        tree: *mut ffi::TSTree,
        snap: &ParseSnapshot,
        line_lo: i32,
        line_hi: i32,
        out_rows: &mut Vec<i32>,
        out_spans: &mut Vec<LineColorSpans>,
    ) {
        if tree.is_null() || line_lo >= line_hi || snap.line_starts.is_empty() {
            return;
        }
        let n = snap.line_starts.len() as i32;
        let lo = line_lo.clamp(0, n);
        let hi = line_hi.clamp(lo, n);
        if lo >= hi {
            return;
        }
        let mut scratch = vec![Vec::new(); (hi - lo) as usize];
        let start = snap.line_starts[lo as usize];
        let end = if hi < n {
            snap.line_starts[hi as usize]
        } else {
            snap.size() as u32
        };
        Self::run_query(query, tree, snap, start, end, &mut scratch, lo);
        for (row, spans) in (lo..hi).zip(scratch) {
            out_rows.push(row);
            out_spans.push(spans);
        }
    }
    fn mark_dirty(snap: &ParseSnapshot, start: u32, end: u32, dirty: &mut [u8]) {
        if start >= end || dirty.is_empty() {
            return;
        }
        let (sr, _) = snap.row_col(start as usize);
        let (er, _) = snap.row_col(end.saturating_sub(1) as usize);
        let lo = sr.min(er).clamp(0, dirty.len() as i32 - 1);
        let hi = sr.max(er).clamp(0, dirty.len() as i32 - 1);
        dirty[lo as usize..=hi as usize].fill(1);
    }
    fn advance_point(mut point: ffi::TSPoint, s: &[u8]) -> ffi::TSPoint {
        for &b in s {
            if b == b'\n' {
                point.row += 1;
                point.column = 0;
            } else {
                point.column += 1;
            }
        }
        point
    }
    fn apply_pending_edits(&mut self, edits: &[PendingEdit]) -> bool {
        if self.tree.is_null() || edits.is_empty() {
            return false;
        }
        for pe in edits {
            if pe.old_end_byte < pe.start_byte || pe.new_end_byte < pe.start_byte {
                return false;
            }
            let start = ffi::TSPoint {
                row: pe.op.row.max(0) as u32,
                column: pe.op.column.max(0) as u32,
            };
            let (mut old_end, mut new_end) = (start, start);
            if pe.op.kind == OpKind::Insert {
                new_end = Self::advance_point(start, &pe.op.text);
            } else if !pe.removed_bytes.is_empty() {
                old_end = Self::advance_point(start, &pe.removed_bytes);
            } else if pe.old_end_byte > pe.start_byte {
                return false;
            }
            let edit = ffi::TSInputEdit {
                start_byte: pe.start_byte,
                old_end_byte: pe.old_end_byte,
                new_end_byte: pe.new_end_byte,
                start_point: start,
                old_end_point: old_end,
                new_end_point: new_end,
            };
            unsafe {
                ffi::ts_tree_edit(self.tree, &edit);
            }
        }
        true
    }
    fn parse(&mut self, snap: &mut ParseSnapshot, generation: u64) -> ParseResult {
        if snap.line_starts.is_empty() {
            snap.text.line_starts(&mut snap.line_starts);
            if snap.line_starts.is_empty() {
                snap.line_starts.push(0);
            }
        }
        let mut result = ParseResult {
            line_count: snap.line_starts.len(),
            ..ParseResult::default()
        };
        if generation < self.last_committed_gen {
            return result;
        }
        let Some((language, query_name)) = detect_language(&snap.language_id) else {
            result.kind = ParseKind::Full;
            result.full_colors = vec![Vec::new(); result.line_count];
            return result;
        };
        if !unsafe { ffi::ts_parser_set_language(self.parser, language) } {
            return result;
        }
        let mut pending = std::mem::take(&mut snap.pending_edits);
        let had_pending = !pending.is_empty();
        if self.tree_file_path != snap.path || self.tree_language_id != snap.language_id {
            unsafe {
                ffi::ts_tree_delete(self.tree);
            }
            self.tree = ptr::null_mut();
            self.tree_doc_bytes = 0;
            pending.clear();
        }
        if !self.tree.is_null() {
            let ok = if !pending.is_empty() {
                self.apply_pending_edits(&pending)
            } else {
                self.tree_doc_bytes == snap.text.size()
            };
            if !ok {
                unsafe {
                    ffi::ts_tree_delete(self.tree);
                }
                self.tree = ptr::null_mut();
                self.tree_doc_bytes = 0;
            }
        }
        let old_tree = self.tree;
        let mut input_state = InputState {
            text: &snap.text,
            scratch: &mut self.input_scratch,
        };
        let input = ffi::TSInput {
            payload: (&mut input_state as *mut InputState<'_>).cast(),
            read: Some(read_snapshot),
            encoding: ffi::TSInputEncodingUTF8,
            decode: None,
        };
        let new_tree = unsafe { ffi::ts_parser_parse(self.parser, old_tree, input) };
        if new_tree.is_null() {
            return result;
        }
        let mut dirty = Vec::new();
        let mut try_partial = false;
        if !old_tree.is_null() && had_pending {
            let mut count = 0;
            let ranges = unsafe { ffi::ts_tree_get_changed_ranges(old_tree, new_tree, &mut count) };
            dirty = vec![0; result.line_count];
            if !ranges.is_null() {
                for range in unsafe { std::slice::from_raw_parts(ranges, count as usize) } {
                    Self::mark_dirty(snap, range.start_byte, range.end_byte, &mut dirty);
                }
                unsafe {
                    free(ranges.cast());
                }
            }
            for pe in &pending {
                let a = pe.start_byte;
                let b = pe.old_end_byte.max(pe.new_end_byte);
                Self::mark_dirty(snap, a, if b > a { b } else { a + 1 }, &mut dirty);
            }
            let count = dirty.iter().filter(|&&v| v != 0).count();
            try_partial = count > 0 && count <= (0.45 * result.line_count as f32 + 1.) as usize;
        }
        unsafe {
            ffi::ts_tree_delete(old_tree);
        }
        self.tree = new_tree;
        self.tree_file_path = snap.path.clone();
        self.tree_language_id = snap.language_id.clone();
        self.tree_doc_bytes = snap.text.size();
        self.last_committed_gen = generation;
        if try_partial {
            let Some(query) = load_query_source(language, query_name, self.bundled_queries) else {
                result.kind = ParseKind::Full;
                result.full_colors = vec![Vec::new(); result.line_count];
                return result;
            };
            let mut i = 0;
            while i < dirty.len() {
                if dirty[i] == 0 {
                    i += 1;
                    continue;
                }
                let lo = i;
                while i < dirty.len() && dirty[i] != 0 {
                    i += 1;
                }
                Self::query_window_into(
                    &query,
                    self.tree,
                    snap,
                    lo as i32,
                    i as i32,
                    &mut result.dirty_rows,
                    &mut result.dirty_spans,
                );
            }
            result.kind = ParseKind::Partial;
            return result;
        }
        result.kind = ParseKind::TreeOnly;
        result
    }
    fn query_window(&self, snap: &ParseSnapshot, lo: i32, hi: i32) -> ParseResult {
        let mut result = ParseResult {
            line_count: snap.line_starts.len(),
            ..ParseResult::default()
        };
        if self.tree.is_null() || snap.line_starts.is_empty() {
            return result;
        }
        let query = detect_language(&snap.language_id)
            .and_then(|(l, n)| load_query_source(l, n, self.bundled_queries));
        let Some(query) = query else {
            result.kind = ParseKind::Full;
            result.full_colors = vec![Vec::new(); result.line_count];
            return result;
        };
        Self::query_window_into(
            &query,
            self.tree,
            snap,
            lo,
            hi,
            &mut result.dirty_rows,
            &mut result.dirty_spans,
        );
        result.kind = ParseKind::Partial;
        result
    }
    pub fn color_document(
        &mut self,
        snap: &mut ParseSnapshot,
        generation: u64,
        chunk_lines: i32,
        canceled: impl Fn() -> bool,
        mut emit: impl FnMut(ParseResult),
    ) {
        if canceled() {
            return;
        }
        let result = self.parse(snap, generation);
        if canceled() || result.kind == ParseKind::Failed {
            return;
        }
        if result.kind != ParseKind::TreeOnly {
            emit(result);
            return;
        }
        let n = snap.line_starts.len() as i32;
        let chunk = chunk_lines.max(1);
        let mut lo = 0;
        while lo < n {
            if canceled() {
                return;
            }
            let hi = n.min(lo + chunk);
            emit(self.query_window(snap, lo, hi));
            lo = hi;
        }
    }
    pub fn query_prefix(src: &ParseSnapshot, max_lines: i32) -> ParseResult {
        Self::query_prefix_with_source(src, max_lines, false)
    }
    pub(crate) fn query_prefix_with_source(
        src: &ParseSnapshot,
        max_lines: i32,
        bundled: bool,
    ) -> ParseResult {
        let mut result = ParseResult {
            line_count: src.text.line_count().max(1) as usize,
            ..ParseResult::default()
        };
        if max_lines <= 0 || src.text.size() == 0 {
            result.kind = ParseKind::Partial;
            return result;
        }
        let Some((language, name)) = detect_language(&src.language_id) else {
            result.kind = ParseKind::Full;
            result.full_colors = vec![Vec::new(); result.line_count];
            return result;
        };
        let cap = src.text.size().min(64 * 1024);
        let mut buf = vec![0; cap];
        src.text.copy_bytes(0, cap, &mut buf);
        let ending = if src.line_ending.is_empty() {
            b"\n".to_vec()
        } else {
            src.line_ending.clone()
        };
        let mut snap = ParseSnapshot {
            text: src.text.clone(),
            line_ending: ending.clone(),
            language_id: src.language_id.clone(),
            line_starts: vec![0],
            ..ParseSnapshot::default()
        };
        let mut pos = 0;
        while snap.line_starts.len() < max_lines as usize && pos < cap {
            let Some(at) = buf[pos..].windows(ending.len()).position(|w| w == ending) else {
                break;
            };
            pos += at + ending.len();
            snap.line_starts.push(pos as u32);
        }
        let prefix_end = if snap.line_starts.len() >= max_lines as usize {
            let last = snap.line_starts[max_lines as usize - 1] as usize;
            buf[last..]
                .windows(ending.len())
                .position(|w| w == ending)
                .map_or(cap, |at| last + at + ending.len())
        } else {
            cap
        };
        snap.byte_limit = prefix_end as u32;
        let Some(query) = load_query_source(language, name, bundled) else {
            result.kind = ParseKind::Partial;
            return result;
        };
        let mut engine = Self::new();
        engine.bundled_queries = bundled;
        unsafe {
            ffi::ts_parser_set_language(engine.parser, language);
        }
        engine.tree = unsafe {
            ffi::ts_parser_parse_string(
                engine.parser,
                ptr::null(),
                buf.as_ptr().cast(),
                prefix_end as u32,
            )
        };
        if !engine.tree.is_null() {
            Self::query_window_into(
                &query,
                engine.tree,
                &snap,
                0,
                snap.line_starts.len() as i32,
                &mut result.dirty_rows,
                &mut result.dirty_spans,
            );
        }
        result.kind = ParseKind::Partial;
        result
    }
    pub fn highlight_snippet(language_id: &str, text: &[u8]) -> ColorRangeMap {
        type SnippetCache = BTreeMap<(String, Vec<u8>), ColorRangeMap>;
        static CACHE: OnceLock<Mutex<SnippetCache>> = OnceLock::new();
        let cache = CACHE.get_or_init(|| Mutex::new(BTreeMap::new()));
        if text.is_empty() {
            return Vec::new();
        }
        let key = (language_id.to_owned(), text.to_vec());
        if let Some(out) = cache.lock().unwrap().get(&key) {
            return out.clone();
        }
        let mut state = EditorState::new();
        state.set_from_bytes(text);
        state.line_ending = b"\n".to_vec();
        let mut snap = ParseSnapshot::from_document(&state, Vec::new());
        state.line_starts(&mut snap.line_starts);
        snap.language_id = language_id.to_owned();
        let lines = state.line_count().max(1) as usize;
        let result = Self::query_prefix(&snap, lines as i32);
        let mut out = vec![Vec::new(); lines];
        if result.kind == ParseKind::Full && result.full_colors.len() == out.len() {
            out = result.full_colors;
        } else {
            for (row, spans) in result.dirty_rows.into_iter().zip(result.dirty_spans) {
                if row >= 0 && (row as usize) < lines {
                    out[row as usize] = spans;
                }
            }
        }
        let mut cache = cache.lock().unwrap();
        if cache.len() >= 32 {
            cache.pop_first();
        }
        cache.insert(key, out.clone());
        out
    }
}

#[cfg(test)]
mod resource_tests {
    use super::*;
    use crate::test_support::TempDir;

    #[test]
    fn installed_queries_resolve_from_binary_and_keep_local_override_priority() {
        for (binary_directory, resource_directory) in [
            ("usr/bin", "usr/share/Bed/resources"),
            (
                "Bed.app/Contents/MacOS",
                "Bed.app/Contents/Resources/resources",
            ),
        ] {
            let temp = TempDir::new();
            let executable = temp.write(&format!("{binary_directory}/bed"), b"path fixture");
            let name = "bed-packaged-query-resolution.scm";
            let installed = temp.write(
                &format!("{resource_directory}/queries/{name}"),
                b"(identifier) @variable",
            );
            let found = query_file_for_executable(name, Some(&executable));
            assert_eq!(
                std::fs::canonicalize(&found).unwrap(),
                std::fs::canonicalize(&installed).unwrap()
            );
            assert_eq!(std::fs::read(found).unwrap(), b"(identifier) @variable");
            let local = temp.write(
                &format!("{binary_directory}/queries/{name}"),
                b"(identifier) @function",
            );
            assert_eq!(query_file_for_executable(name, Some(&executable)), local);
            std::fs::remove_file(local).unwrap();
            assert_eq!(
                std::fs::canonicalize(query_file_for_executable(name, Some(&executable))).unwrap(),
                std::fs::canonicalize(&installed).unwrap()
            );
            std::fs::remove_file(installed).unwrap();
            assert_eq!(
                query_file_for_executable(name, Some(&executable)),
                PathBuf::from("bundled").join(name)
            );
        }
    }
}
