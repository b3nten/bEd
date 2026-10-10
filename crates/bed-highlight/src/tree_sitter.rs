//! Arborium grammar spans projected onto Bed's byte-based document lines.
use crate::{
    capture_map::{THEME_KEYS, ThemeSlot, capture_priority, theme_slot_for_capture},
    grammars::{grammar_store, language_name},
};
use arborium::{Highlighter, advanced::Span};
use bed_editing::{buffer::text_buffer::Snapshot, editor_state::EditorState};
use std::{
    cmp::Reverse,
    collections::{BTreeMap, BTreeSet, HashSet},
    sync::{Mutex, Once, OnceLock},
};

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
    pub path: String,
    pub language_id: String,
}
impl ParseSnapshot {
    pub fn from_document(state: &EditorState) -> Self {
        Self {
            text: state.snapshot(),
            path: state.path.clone(),
            language_id: state.language_id.clone(),
        }
    }
}

fn snapshot_language(snap: &ParseSnapshot) -> Option<&'static str> {
    // A manually selected language wins. For automatic detection, the filename
    // can identify formats whose extension alone is ambiguous (CMakeLists.txt).
    if snap.language_id == EditorState::language_id_from_path(&snap.path) {
        language_name(&snap.path).or_else(|| language_name(&snap.language_id))
    } else {
        language_name(&snap.language_id)
    }
}

fn warmed_languages() -> &'static Mutex<HashSet<&'static str>> {
    static WARMED: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    WARMED.get_or_init(Mutex::default)
}

/// Arborium accepts UTF-8 strings. Keep document byte offsets stable when files
/// contain invalid UTF-8, and make bare CR line endings visible to its parsers.
pub(crate) fn parser_text(mut bytes: Vec<u8>) -> String {
    for i in 0..bytes.len() {
        if bytes[i] == b'\r' && bytes.get(i + 1) != Some(&b'\n') {
            bytes[i] = b'\n';
        }
    }
    let mut offset = 0;
    while let Err(error) = std::str::from_utf8(&bytes[offset..]) {
        offset += error.valid_up_to();
        let end = offset + error.error_len().unwrap_or(bytes.len() - offset);
        bytes[offset..end].fill(b' ');
        offset = end;
    }
    String::from_utf8(bytes).expect("invalid UTF-8 replaced without changing byte offsets")
}

fn source_line_starts(text: &[u8]) -> Vec<u32> {
    let mut starts = vec![0];
    let mut position = 0;
    while position < text.len() {
        let byte = text[position];
        position += 1;
        if byte == b'\r' && text.get(position) == Some(&b'\n') {
            position += 1;
        }
        if matches!(byte, b'\r' | b'\n') {
            starts.push(position as u32);
        }
    }
    starts
}

/// Resolve enclosing captures before their children. Specific roles win equal
/// ranges; later captures break ties, including captures from injected languages.
fn line_colors(
    spans: Vec<Span>,
    text: &[u8],
    starts: &[u32],
    canceled: impl Fn() -> bool,
) -> Option<ColorRangeMap> {
    let mut colors = vec![Vec::<ColorSpan>::new(); starts.len()];
    let mut events = Vec::with_capacity(spans.len() * 2);
    for (index, span) in spans.iter().enumerate() {
        if span.start < span.end && span.end as usize <= text.len() {
            events.push((span.start, true, index));
            events.push((span.end, false, index));
        }
    }
    if canceled() {
        return None;
    }
    events.sort_unstable();
    let mut active = BTreeSet::new();
    let mut event = 0;
    while event < events.len() {
        if canceled() {
            return None;
        }
        let position = events[event].0;
        while event < events.len() && events[event].0 == position {
            let (_, entering, index) = events[event];
            let span = &spans[index];
            let rank = (
                Reverse(span.end - span.start),
                capture_priority(&span.capture),
                index,
            );
            if entering {
                active.insert(rank);
            } else {
                active.remove(&rank);
            }
            event += 1;
        }
        let Some(&(_, _, index)) = active.last() else {
            continue;
        };
        let Some(&(end, _, _)) = events.get(event) else {
            break;
        };
        let slot = theme_slot_for_capture(&spans[index].capture);
        if slot == ThemeSlot::Text {
            continue;
        }
        let mut row = starts
            .partition_point(|&start| start <= position)
            .saturating_sub(1);
        let mut start = position as usize;
        while start < end as usize {
            let line_start = starts[row] as usize;
            let next = starts
                .get(row + 1)
                .map_or(text.len(), |&start| start as usize);
            let mut line_end = next;
            while line_end > line_start && matches!(text[line_end - 1], b'\r' | b'\n') {
                line_end -= 1;
            }
            let stop = (end as usize).min(line_end);
            if start < stop {
                let a = (start - line_start) as i32;
                let b = (stop - line_start) as i32;
                let line = &mut colors[row];
                if let Some(last) = line.last_mut()
                    && last.end == a
                    && last.slot == slot
                {
                    last.end = b;
                } else {
                    line.push(ColorSpan {
                        start: a,
                        end: b,
                        slot,
                    });
                }
            }
            if end as usize <= next {
                break;
            }
            start = next;
            row += 1;
            if row % 4096 == 0 && canceled() {
                return None;
            }
        }
    }
    Some(colors)
}

pub struct TreeSitter {
    highlighter: Highlighter,
}
impl Default for TreeSitter {
    fn default() -> Self {
        Self {
            highlighter: Highlighter::with_store(grammar_store().clone()),
        }
    }
}
impl TreeSitter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn query_ready(&self, id: &str) -> bool {
        Self::is_query_ready(id)
    }

    /// Does not compile a query or wait for a parsing worker.
    pub fn is_query_ready(id: &str) -> bool {
        language_name(id).is_none_or(|name| warmed_languages().lock().unwrap().contains(name))
    }

    pub(crate) fn snapshot_query_ready(snap: &ParseSnapshot) -> bool {
        snapshot_language(snap).is_none_or(|name| warmed_languages().lock().unwrap().contains(name))
    }

    pub fn prewarm(id: &str) -> Result<(), String> {
        let name = language_name(id).ok_or_else(|| format!("Unsupported grammar: {id}"))?;
        grammar_store()
            .get(name)
            .ok_or_else(|| format!("Unable to compile {name} queries"))?;
        warmed_languages().lock().unwrap().insert(name);
        Ok(())
    }

    pub fn start_background_prewarm() {
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            std::thread::spawn(|| {
                for language in ["rust", "c", "cpp", "python", "json", "markdown"] {
                    if let Err(error) = Self::prewarm(language) {
                        eprintln!("{error}");
                    }
                }
            });
        });
    }

    pub fn color_document(
        &mut self,
        snap: &ParseSnapshot,
        chunk_lines: i32,
        canceled: impl Fn() -> bool,
        mut emit: impl FnMut(ParseResult),
    ) {
        if canceled() {
            return;
        }
        let mut starts = Vec::new();
        snap.text.line_starts(&mut starts);
        if starts.is_empty() {
            starts.push(0);
        }
        let line_count = starts.len();
        let Some(language) = snapshot_language(snap) else {
            emit(ParseResult {
                kind: ParseKind::Full,
                line_count,
                full_colors: vec![Vec::new(); line_count],
                ..ParseResult::default()
            });
            return;
        };
        let source = parser_text(snap.text.bytes());
        if canceled() {
            return;
        }
        let spans = match self.highlighter.highlight_spans(language, &source) {
            Ok(spans) => spans,
            Err(error) => {
                eprintln!("Highlighting {language}: {error}");
                emit(ParseResult {
                    kind: ParseKind::Full,
                    line_count,
                    full_colors: vec![Vec::new(); line_count],
                    ..ParseResult::default()
                });
                return;
            }
        };
        if canceled() {
            return;
        }
        warmed_languages().lock().unwrap().insert(language);
        let Some(colors) = line_colors(spans, source.as_bytes(), &starts, &canceled) else {
            return;
        };
        // Keep publication bounded and cancellable even though Arborium parses
        // a complete snapshot. The service rejects results from superseded jobs.
        for (chunk, lines) in colors.chunks(chunk_lines.max(1) as usize).enumerate() {
            if canceled() {
                return;
            }
            let start = chunk * chunk_lines.max(1) as usize;
            emit(ParseResult {
                kind: ParseKind::Partial,
                line_count,
                dirty_rows: (start..start + lines.len()).map(|row| row as i32).collect(),
                dirty_spans: lines.to_vec(),
                ..ParseResult::default()
            });
        }
    }

    /// Give the initial viewport colors from a bounded prefix before the worker
    /// processes the complete snapshot (which resolves multi-line constructs).
    pub fn query_prefix(src: &ParseSnapshot, max_lines: i32) -> ParseResult {
        let line_count = src.text.line_count().max(1) as usize;
        let mut result = ParseResult {
            kind: ParseKind::Partial,
            line_count,
            ..ParseResult::default()
        };
        if max_lines <= 0 {
            return result;
        }
        let cap = src.text.size().min(64 * 1024);
        let mut bytes = vec![0; cap];
        src.text.copy_bytes(0, cap, &mut bytes);
        let mut starts = source_line_starts(&bytes);
        let end = starts
            .get(max_lines as usize)
            .map_or(cap, |&start| start as usize);
        starts.truncate(max_lines as usize);
        let Some(language) = snapshot_language(src) else {
            return result;
        };
        bytes.truncate(end);
        let source = parser_text(bytes);
        let mut highlighter = Highlighter::with_store(grammar_store().clone());
        if let Ok(spans) = highlighter.highlight_spans(language, &source) {
            warmed_languages().lock().unwrap().insert(language);
            let colors = line_colors(spans, source.as_bytes(), &starts, || false).unwrap();
            result.dirty_rows = (0..colors.len()).map(|row| row as i32).collect();
            result.dirty_spans = colors;
        }
        result
    }

    pub fn highlight_snippet(language_id: &str, text: &[u8]) -> ColorRangeMap {
        type SnippetCache = BTreeMap<(String, Vec<u8>), ColorRangeMap>;
        static CACHE: OnceLock<Mutex<SnippetCache>> = OnceLock::new();
        if text.is_empty() {
            return Vec::new();
        }
        let cache = CACHE.get_or_init(Mutex::default);
        let key = (language_id.to_owned(), text.to_vec());
        if let Some(out) = cache.lock().unwrap().get(&key) {
            return out.clone();
        }
        let source = parser_text(text.to_vec());
        let starts = source_line_starts(source.as_bytes());
        let mut highlighter = Highlighter::with_store(grammar_store().clone());
        let mut out = vec![Vec::new(); starts.len()];
        if let Some(language) = language_name(language_id) {
            match highlighter.highlight_spans(language, &source) {
                Ok(spans) => {
                    warmed_languages().lock().unwrap().insert(language);
                    out = line_colors(spans, source.as_bytes(), &starts, || false).unwrap();
                }
                Err(error) => eprintln!("Highlighting {language}: {error}"),
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
