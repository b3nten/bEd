//! Literal search over rope chunks, including matches spanning leaves. Workers
//! return revision-bound observations; only the editor may commit replacements.
use crate::{EditorError, TextBuffer, TextSnapshot, background::Job, text::BufferBuilder};
use bedtime_ui2::egui;
use std::{
    collections::VecDeque,
    ops::Range,
    sync::atomic::{AtomicBool, Ordering},
};
const MATCH_CACHE: usize = 10_000;
const MAX_REPLACEMENT_BYTES: usize = 256 * 1024 * 1024;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchQuery {
    pub text: String,
    pub match_case: bool,
    pub whole_word: bool,
}
struct Pattern {
    chars: Vec<char>,
    failure: Vec<usize>,
}
impl Pattern {
    fn new(query: &SearchQuery) -> Self {
        let chars: Vec<_> = if query.match_case {
            query.text.chars().collect()
        } else {
            query.text.chars().flat_map(char::to_lowercase).collect()
        };
        let mut failure = vec![0; chars.len()];
        for i in 1..chars.len() {
            let mut j = failure[i - 1];
            while j > 0 && chars[i] != chars[j] {
                j = failure[j - 1];
            }
            if chars[i] == chars[j] {
                j += 1;
            }
            failure[i] = j;
        }
        Self { chars, failure }
    }
}
fn word(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_' || unicode_width::UnicodeWidthChar::width(ch) == Some(0)
}
fn boundary(text: &TextBuffer, at: usize, before: bool) -> bool {
    if (before && at == 0) || (!before && at == text.len()) {
        return true;
    }
    let (chunk, base) = text.chunk_at(if before { at - 1 } else { at });
    let ch = if before {
        chunk[..at - base].chars().next_back().unwrap()
    } else {
        chunk[at - base..].chars().next().unwrap()
    };
    !word(ch)
}
/// Non-overlapping Unicode literal matches. Lowercase expansion retains source
/// offsets and cannot match half of one expanded source character.
pub(crate) fn scan(
    text: &TextBuffer,
    query: &SearchQuery,
    cancel: &AtomicBool,
    mut found: impl FnMut(Range<usize>) -> Result<(), EditorError>,
) -> Result<bool, EditorError> {
    scan_range(text, query, 0..text.len(), cancel, &mut found)
}
fn scan_bytes(
    text: &TextBuffer,
    query: &SearchQuery,
    range: Range<usize>,
    cancel: &AtomicBool,
    mut found: impl FnMut(Range<usize>) -> Result<(), EditorError>,
) -> Result<bool, EditorError> {
    let pattern = if query.match_case {
        query.text.as_bytes().to_vec()
    } else {
        query.text.to_ascii_lowercase().into_bytes()
    };
    if pattern.is_empty() {
        return Ok(true);
    }
    let mut failure = vec![0; pattern.len()];
    for i in 1..pattern.len() {
        let mut j = failure[i - 1];
        while j > 0 && pattern[i] != pattern[j] {
            j = failure[j - 1];
        }
        if pattern[i] == pattern[j] {
            j += 1;
        }
        failure[i] = j;
    }
    let mut matched = 0;
    let mut at = range.start;
    while at < range.end {
        if cancel.load(Ordering::Relaxed) {
            return Ok(false);
        }
        let (chunk, base) = text.chunk_at(at);
        let end = (base + chunk.len()).min(range.end);
        for (offset, byte) in chunk.as_bytes()[at - base..end - base].iter().enumerate() {
            let byte = if query.match_case {
                *byte
            } else {
                byte.to_ascii_lowercase()
            };
            while matched > 0 && byte != pattern[matched] {
                matched = failure[matched - 1];
            }
            if byte == pattern[matched] {
                matched += 1;
            }
            if matched == pattern.len() {
                let end = at + offset + 1;
                let range = end - pattern.len()..end;
                if !query.whole_word
                    || (boundary(text, range.start, true) && boundary(text, range.end, false))
                {
                    found(range)?;
                    matched = 0;
                } else {
                    matched = failure[matched - 1];
                }
            }
        }
        at = end;
    }
    Ok(true)
}
fn scan_range(
    text: &TextBuffer,
    query: &SearchQuery,
    range: Range<usize>,
    cancel: &AtomicBool,
    mut found: impl FnMut(Range<usize>) -> Result<(), EditorError>,
) -> Result<bool, EditorError> {
    if query.match_case
        || (query.text.is_ascii()
            && (text.is_ascii() || !query.text.chars().any(char::is_alphabetic)))
    {
        return scan_bytes(text, query, range, cancel, found);
    }
    let pattern = Pattern::new(query);
    if pattern.chars.is_empty() {
        return Ok(true);
    }
    let mut matched = 0;
    let mut recent = VecDeque::with_capacity(pattern.chars.len());
    let mut at = range.start;
    while at < range.end {
        let (source, base) = text.chunk_at(at);
        let chunk = &source[at - base..(range.end - base).min(source.len())];
        let base = at;
        if cancel.load(Ordering::Relaxed) {
            return Ok(false);
        }
        for (offset, ch) in chunk.char_indices() {
            let start = base + offset;
            let end = start + ch.len_utf8();
            let mut lowered = std::iter::once(ch)
                .take(usize::from(query.match_case))
                .chain(
                    ch.to_lowercase()
                        .take(if query.match_case { 0 } else { usize::MAX }),
                )
                .peekable();
            let mut first_token = true;
            while let Some(character) = lowered.next() {
                let last_token = lowered.peek().is_none();
                recent.push_back((start, end, first_token, last_token));
                first_token = false;
                if recent.len() > pattern.chars.len() {
                    recent.pop_front();
                }
                while matched > 0 && character != pattern.chars[matched] {
                    matched = pattern.failure[matched - 1];
                }
                if character == pattern.chars[matched] {
                    matched += 1;
                }
                if matched == pattern.chars.len() {
                    let first = recent.front().unwrap();
                    let range = first.0..end;
                    let accepted = first.2
                        && last_token
                        && (!query.whole_word
                            || (boundary(text, range.start, true)
                                && boundary(text, range.end, false)));
                    if accepted {
                        found(range)?;
                        matched = 0;
                    } else {
                        matched = pattern.failure[matched - 1];
                    }
                }
            }
        }
        at += chunk.len();
    }
    Ok(true)
}
/// Search groups retain only the first/last match per 4 KiB source region.
/// Starting at an accepted match preserves global non-overlap alignment; the
/// visible scan stays bounded even for millions of repeated matches.
pub(crate) fn visible_matches(
    text: &TextBuffer,
    results: &Results,
    visible: Range<usize>,
) -> Vec<Range<usize>> {
    let first = results
        .groups
        .partition_point(|group| group.end <= visible.start);
    let groups = results.groups[first..]
        .iter()
        .take_while(|group| group.start < visible.end)
        .collect::<Vec<_>>();
    let (Some(first), Some(last)) = (groups.first(), groups.last()) else {
        return Vec::new();
    };
    let mut ranges = Vec::new();
    let _ = scan_range(
        text,
        &results.query,
        first.start..last.end,
        &AtomicBool::new(false),
        |range| {
            if range.end > visible.start && range.start < visible.end {
                ranges.push(range);
            }
            Ok(())
        },
    );
    ranges
}
#[derive(Debug)]
pub(crate) struct Results {
    pub revision: u64,
    pub query: SearchQuery,
    pub ranges: Vec<Range<usize>>,
    groups: Vec<Range<usize>>,
    pub count: usize,
    pub next: Option<Range<usize>>,
    pub previous: Option<Range<usize>>,
    pub anchor: usize,
}
impl Results {
    pub fn find(
        snapshot: &TextSnapshot,
        query: SearchQuery,
        anchor: usize,
        cancel: &AtomicBool,
    ) -> Option<Self> {
        let mut result = Self {
            revision: snapshot.revision,
            query: query.clone(),
            ranges: Vec::new(),
            groups: Vec::new(),
            count: 0,
            next: None,
            previous: None,
            anchor,
        };
        let mut first = None;
        let mut last = None;
        if !scan(&snapshot.text, &query, cancel, |range| {
            if first.is_none() {
                first = Some(range.clone());
            }
            if range.start >= anchor && result.next.is_none() {
                result.next = Some(range.clone());
            }
            if range.start < anchor {
                result.previous = Some(range.clone());
            }
            last = Some(range.clone());
            result.count += 1;
            if let Some(group) = result
                .groups
                .last_mut()
                .filter(|group| group.start / 4096 == range.start / 4096)
            {
                group.end = range.end;
            } else {
                result.groups.push(range.clone());
            }
            if result.ranges.len() < MATCH_CACHE {
                result.ranges.push(range);
            }
            Ok(())
        })
        .ok()?
        {
            return None;
        }
        result.next = result.next.or(first);
        result.previous = result.previous.or(last);
        Some(result)
    }
    pub fn at(&self, anchor: usize, forward: bool) -> Option<Range<usize>> {
        if self.count == 0 {
            return None;
        }
        if forward {
            self.ranges
                .iter()
                .find(|r| r.start >= anchor)
                .cloned()
                .or_else(|| {
                    if self.count == self.ranges.len() {
                        self.ranges.first().cloned()
                    } else if anchor == self.anchor {
                        self.next.clone()
                    } else {
                        None
                    }
                })
        } else {
            if self.count > self.ranges.len() && anchor > self.ranges.last().unwrap().end {
                return if anchor == self.anchor {
                    self.previous.clone()
                } else {
                    None
                };
            }
            self.ranges
                .iter()
                .rev()
                .find(|r| r.start < anchor)
                .cloned()
                .or_else(|| {
                    if self.count == self.ranges.len() {
                        self.ranges.last().cloned()
                    } else if anchor == self.anchor {
                        self.previous.clone()
                    } else {
                        None
                    }
                })
        }
    }
}

pub(crate) fn replace_all(
    snapshot: &TextSnapshot,
    query: &SearchQuery,
    replacement: &str,
    cancel: &AtomicBool,
) -> Option<Result<(TextBuffer, usize), EditorError>> {
    let mut builder = BufferBuilder::default();
    let mut at = 0;
    let mut count = 0;
    let mut bytes = 0usize;
    let result = scan(&snapshot.text, query, cancel, |range| {
        bytes = bytes
            .checked_add(range.start - at)
            .and_then(|n| n.checked_add(replacement.len()))
            .ok_or_else(|| EditorError("Replacement is too large".into()))?;
        if bytes > MAX_REPLACEMENT_BYTES {
            return Err(EditorError(
                "Replace all exceeds the 256 MiB result limit".into(),
            ));
        }
        if !builder.push_range(&snapshot.text, at..range.start, cancel) {
            return Err(EditorError("Replacement cancelled".into()));
        }
        builder.push(replacement);
        at = range.end;
        count += 1;
        Ok(())
    });
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    match result {
        Ok(false) => None,
        Err(error) => Some(Err(error)),
        Ok(true) => {
            if count == 0 {
                return Some(Ok((snapshot.text.clone(), 0)));
            }
            if bytes.saturating_add(snapshot.text.len() - at) > MAX_REPLACEMENT_BYTES {
                return Some(Err(EditorError(
                    "Replace all exceeds the 256 MiB result limit".into(),
                )));
            }
            if !builder.push_range(&snapshot.text, at..snapshot.text.len(), cancel) {
                return None;
            }
            let result = builder.finish();
            let result = if result == snapshot.text {
                snapshot.text.clone()
            } else {
                result
            };
            Some(Ok((result, count)))
        }
    }
}

#[derive(Default)]
pub(crate) struct Search {
    pub open: bool,
    pub replace: bool,
    pub query: SearchQuery,
    pub replacement: String,
    pub results: Option<Results>,
    pub job: Option<Job<Results>>,
    pub requested: Option<(u64, SearchQuery, usize)>,
    pub navigation: Option<bool>,
    pub focus: bool,
    pub replacement_job: Option<(u64, Job<Result<(TextBuffer, usize), EditorError>>)>,
    pub message: Option<String>,
}
impl Search {
    pub fn refresh(&mut self, ctx: &egui::Context, snapshot: &TextSnapshot, anchor: usize) {
        if !self.open || self.query.text.is_empty() {
            self.job = None;
            self.results = None;
            self.requested = None;
            return;
        }
        if let Some(job) = &self.job {
            match job.poll() {
                Ok(Some(result)) => {
                    if result.revision == snapshot.revision && result.query == self.query {
                        self.results = Some(result);
                    }
                    self.job = None;
                }
                Err(()) => {
                    self.job = None;
                    self.requested = None;
                    self.message = Some("Search worker stopped".into());
                }
                _ => {}
            }
        }
        let ready = self
            .results
            .as_ref()
            .is_some_and(|r| r.revision == snapshot.revision && r.query == self.query);
        if !ready {
            self.results = None;
        }
        let needs_anchor = self.navigation.is_some_and(|direction| {
            self.results
                .as_ref()
                .is_some_and(|r| r.count > 0 && r.at(anchor, direction).is_none())
        });
        let key = (snapshot.revision, self.query.clone(), anchor);
        if (!ready || needs_anchor) && self.requested.as_ref() != Some(&key) {
            let snapshot = snapshot.clone();
            let query = self.query.clone();
            self.job = Some(Job::start(ctx, move |cancel| {
                Results::find(&snapshot, query, anchor, cancel)
            }));
            self.requested = Some(key);
        }
    }
}
