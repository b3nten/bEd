use crate::syntax::*;
use std::{
    collections::{BinaryHeap, HashMap, HashSet},
    ops::ControlFlow,
    time::Instant,
};
use tree_sitter::{
    InputEdit, Language, ParseOptions, Parser, Point, Query, QueryCursor, QueryCursorOptions,
    StreamingIterator, Tree,
};

struct Queries {
    language: Language,
    highlights: Query,
    locals: Option<Query>,
    injections: Option<Query>,
    outline: Option<Query>,
    captures: Vec<Arc<str>>,
}
impl Queries {
    fn new(name: &str) -> Result<Self, AnalysisStatus> {
        let source = languages::grammar(name).ok_or(AnalysisStatus::Unsupported)?;
        let compile = |s: &str| {
            Query::new(&source.language, s)
                .map_err(|e| AnalysisStatus::Failed(format!("{name}: {e}")))
        };
        let optional = |s: &str| {
            if s.is_empty() {
                Ok(None)
            } else {
                compile(s).map(Some)
            }
        };
        let highlights = compile(&source.highlights)?;
        let captures = highlights
            .capture_names()
            .iter()
            .map(|s| Arc::from(*s))
            .collect();
        Ok(Self {
            captures,
            highlights,
            locals: optional(source.locals)?,
            injections: optional(&source.injections)?,
            outline: optional(outline::query_source(name).unwrap_or(""))?,
            language: source.language,
        })
    }
}
struct Previous {
    key: ObservationKey,
    tree: Tree,
    bytes: Vec<u8>,
}
#[derive(Default)]
pub(crate) struct Backend {
    queries: HashMap<&'static str, Arc<Queries>>,
    previous: Option<Previous>,
}

struct Budget<'a> {
    start: Instant,
    canceled: &'a dyn Fn() -> bool,
    results: usize,
    injections: usize,
    injected_bytes: usize,
    line_starts: Vec<usize>,
}
impl Budget<'_> {
    fn point(&self, byte: usize) -> Point {
        let row = self
            .line_starts
            .partition_point(|start| *start <= byte)
            .saturating_sub(1);
        Point::new(row, byte - self.line_starts[row])
    }
    fn check(&self) -> Result<(), AnalysisStatus> {
        if (self.canceled)() {
            return Err(AnalysisStatus::Pending);
        }
        if self.start.elapsed() > MAX_ANALYSIS_TIME {
            return Err(AnalysisStatus::Limited("Analysis exceeded five seconds"));
        }
        Ok(())
    }
    fn record(&mut self) -> Result<(), AnalysisStatus> {
        self.results += 1;
        if self.results > MAX_RESULTS {
            return Err(AnalysisStatus::Limited("Analysis exceeded 250,000 results"));
        }
        self.check()
    }
}
fn point(source: &[u8], at: usize) -> Point {
    let prefix = &source[..at];
    Point::new(
        prefix.iter().filter(|&&b| b == b'\n').count(),
        prefix
            .iter()
            .rposition(|&b| b == b'\n')
            .map_or(at, |p| at - p - 1),
    )
}
/// Conservative replacement between retained worker snapshots. Points count LF
/// and byte columns, never editor display columns. No document diff runs on UI.
fn replacement(before: &[u8], after: &[u8]) -> InputEdit {
    let mut start = before.iter().zip(after).take_while(|(a, b)| a == b).count();
    while start > 0
        && ((before.get(start).is_some_and(|b| b & 0xc0 == 0x80))
            || (after.get(start).is_some_and(|b| b & 0xc0 == 0x80)))
    {
        start -= 1;
    }
    let mut suffix = before[start..]
        .iter()
        .rev()
        .zip(after[start..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    while suffix > 0
        && (before
            .get(before.len() - suffix)
            .is_some_and(|b| b & 0xc0 == 0x80)
            || after
                .get(after.len() - suffix)
                .is_some_and(|b| b & 0xc0 == 0x80))
    {
        suffix -= 1;
    }
    InputEdit {
        start_byte: start,
        old_end_byte: before.len() - suffix,
        new_end_byte: after.len() - suffix,
        start_position: point(before, start),
        old_end_position: point(before, before.len() - suffix),
        new_end_position: point(after, after.len() - suffix),
    }
}
fn parse(
    parser: &mut Parser,
    bytes: &[u8],
    old: Option<&Tree>,
    budget: &Budget<'_>,
) -> Result<Tree, AnalysisStatus> {
    budget.check()?;
    let mut progress = |_: &tree_sitter::ParseState| {
        if budget.check().is_ok() {
            ControlFlow::Continue(())
        } else {
            ControlFlow::Break(())
        }
    };
    let tree = parser.parse_with_options(
        &mut |byte, _| &bytes[byte..],
        old,
        Some(ParseOptions::new().progress_callback(&mut progress)),
    );
    budget.check()?;
    tree.ok_or_else(|| AnalysisStatus::Failed("Parser did not produce a tree".into()))
}
impl Backend {
    fn queries(&mut self, name: &'static str) -> Result<Arc<Queries>, AnalysisStatus> {
        if let Some(queries) = self.queries.get(name) {
            return Ok(queries.clone());
        }
        let queries = Arc::new(Queries::new(name)?);
        self.queries.insert(name, queries.clone());
        Ok(queries)
    }
    pub(crate) fn analyze(
        &mut self,
        key: ObservationKey,
        language: &'static str,
        source: Arc<dyn Source>,
        base_revision: Option<u64>,
        canceled: &dyn Fn() -> bool,
    ) -> Result<Observation, AnalysisStatus> {
        let mut budget = Budget {
            start: Instant::now(),
            canceled,
            results: 0,
            injections: 0,
            injected_bytes: 0,
            line_starts: Vec::new(),
        };
        if source.len() > MAX_SOURCE_BYTES {
            return Err(AnalysisStatus::Limited("Source exceeds 8 MiB"));
        }
        let mut bytes = Vec::with_capacity(source.len());
        while bytes.len() < source.len() {
            budget.check()?;
            let chunk = source.chunk(bytes.len());
            if chunk.is_empty() || chunk.len() > source.len() - bytes.len() {
                return Err(AnalysisStatus::Failed("Invalid source chunk".into()));
            }
            bytes.extend_from_slice(chunk);
        }
        // Preserve offsets while allowing bare CR logical lines in grammars
        // which otherwise treat CR as ordinary characters inside comments.
        for i in 0..bytes.len() {
            if bytes[i] == b'\r' && bytes.get(i + 1) != Some(&b'\n') {
                bytes[i] = b'\n';
            }
        }
        let text =
            std::str::from_utf8(&bytes).map_err(|e| AnalysisStatus::Failed(e.to_string()))?;
        budget.line_starts = std::iter::once(0)
            .chain(
                bytes
                    .iter()
                    .enumerate()
                    .filter_map(|(i, b)| (*b == b'\n').then_some(i + 1)),
            )
            .collect();
        let queries = self.queries(language)?;
        let mut parser = Parser::new();
        parser
            .set_language(&queries.language)
            .map_err(|e| AnalysisStatus::Failed(e.to_string()))?;
        let mut old = self.previous.take().filter(|p| {
            p.key.source == key.source
                && p.key.language_generation == key.language_generation
                && Some(p.key.revision) == base_revision
        });
        if let Some(old) = &mut old {
            old.tree.edit(&replacement(&old.bytes, &bytes));
        }
        let tree = parse(&mut parser, &bytes, old.as_ref().map(|p| &p.tree), &budget)?;
        let incremental = old.is_some();
        let mut raw = Vec::new();
        self.highlight(language, &tree, &bytes, 0, &mut raw, &mut budget)?;
        let outline_result =
            outline::extract(&tree, queries.outline.as_ref(), language, text, || {
                budget.check().is_err()
            });
        budget.check()?;
        let outline = outline_result?;
        if outline.len() > MAX_RESULTS {
            return Err(AnalysisStatus::Limited("Too many outline entries"));
        }
        let mut folds = Vec::new();
        let kinds = folds::fold_kinds(language).unwrap_or(&[]);
        let mut cursor = tree.walk();
        loop {
            budget.check()?;
            let node = cursor.node();
            if kinds.contains(&node.kind()) {
                let start_line = budget.point(node.start_byte()).row;
                let end_line = budget.point(node.end_byte().saturating_sub(1)).row;
                if end_line > start_line {
                    budget.record()?;
                    folds.push(FoldRegion {
                        range: node.byte_range(),
                        start_line,
                        end_line,
                    });
                }
            }
            if cursor.goto_first_child() {
                continue;
            }
            loop {
                if cursor.goto_next_sibling() {
                    break;
                }
                if !cursor.goto_parent() {
                    break;
                }
            }
            if cursor.node() == tree.root_node() {
                break;
            }
        }
        drop(cursor);
        folds.sort_by_key(|f| (f.start_line, std::cmp::Reverse(f.end_line)));
        folds.dedup_by_key(|f| f.start_line);
        // Validate once at publication. Source errors remain recoverable trees.
        let valid = |r: &Range<usize>| {
            r.start <= r.end && text.is_char_boundary(r.start) && text.is_char_boundary(r.end)
        };
        if raw.iter().any(|s| !valid(&s.range))
            || folds.iter().any(|f| !valid(&f.range))
            || outline.iter().enumerate().any(|(index, n)| {
                !valid(&n.range) || !valid(&n.name_range) || n.parent.is_some_and(|p| p >= index)
            })
        {
            return Err(AnalysisStatus::Failed(
                "Analysis produced an invalid source range or outline parent".into(),
            ));
        }
        raw.retain(|s| !s.range.is_empty());
        let highlights = resolve_spans(raw, &budget)?;
        budget.check()?;
        self.previous = Some(Previous {
            key,
            tree: tree.clone(),
            bytes,
        });
        Ok(Observation {
            key,
            language,
            source,
            tree,
            highlights,
            folds,
            outline,
            incremental,
            elapsed: budget.start.elapsed(),
        })
    }
    fn highlight(
        &mut self,
        language: &'static str,
        tree: &Tree,
        bytes: &[u8],
        depth: usize,
        spans: &mut Vec<RawSpan>,
        budget: &mut Budget<'_>,
    ) -> Result<(), AnalysisStatus> {
        let queries = self.queries(language)?;
        let locals = local_ranges(queries.locals.as_ref(), tree, bytes, budget)?;
        let mut cursor = QueryCursor::new();
        cursor.set_match_limit(8192);
        let mut progress = |_: &tree_sitter::QueryCursorState| {
            if budget.check().is_ok() {
                ControlFlow::Continue(())
            } else {
                ControlFlow::Break(())
            }
        };
        // QueryCursor evaluates the bundled text predicates against this snapshot.
        let mut matches = cursor.matches_with_options(
            &queries.highlights,
            tree.root_node(),
            bytes,
            QueryCursorOptions::new().progress_callback(&mut progress),
        );
        let mut count = 0;
        while let Some(found) = matches.next() {
            budget.check()?;
            if queries
                .highlights
                .property_predicates(found.pattern_index)
                .iter()
                .any(|(property, positive)| {
                    if property.key.as_ref() != "local" {
                        return false;
                    }
                    let local = found.captures.iter().any(|c| {
                        property.capture_id.is_none_or(|id| c.index as usize == id)
                            && locals.contains(&c.node.byte_range())
                    });
                    local != *positive
                })
            {
                continue;
            }
            for capture in found.captures {
                count += 1;
                if budget.results + count > MAX_RESULTS {
                    return Err(AnalysisStatus::Limited("Too many highlights"));
                }
                let name = &queries.captures[capture.index as usize];
                if !name.starts_with('_') && !name.starts_with("injection.") {
                    spans.push(RawSpan {
                        range: capture.node.byte_range(),
                        capture: name.clone(),
                        priority: (depth, found.pattern_index),
                    });
                }
            }
        }
        drop(matches);
        if cursor.did_exceed_match_limit() {
            return Err(AnalysisStatus::Limited("Highlight query match limit"));
        }
        budget.results += count;
        if budget.results > MAX_RESULTS {
            return Err(AnalysisStatus::Limited("Too many analysis results"));
        }
        budget.check()?;
        if depth >= 3 {
            return Ok(());
        }
        if let Some(query) = &queries.injections {
            let mut cursor = QueryCursor::new();
            cursor.set_match_limit(8192);
            let mut progress = |_: &tree_sitter::QueryCursorState| {
                if budget.check().is_ok() {
                    ControlFlow::Continue(())
                } else {
                    ControlFlow::Break(())
                }
            };
            let mut matches = cursor.matches_with_options(
                query,
                tree.root_node(),
                bytes,
                QueryCursorOptions::new().progress_callback(&mut progress),
            );
            let mut injections = Vec::new();
            let mut combined: HashMap<(&'static str, usize), Vec<tree_sitter::Range>> =
                HashMap::new();
            let mut range_count = 0;
            while let Some(found) = matches.next() {
                let mut name = None;
                let mut include_children = false;
                let mut combine = false;
                for property in query.property_settings(found.pattern_index) {
                    if property.key.as_ref() == "injection.combined" {
                        combine = true;
                    }
                    if property.key.as_ref() == "injection.language" {
                        name = property.value.as_deref();
                    }
                    if property.key.as_ref() == "injection.include-children" {
                        include_children = true;
                    }
                }
                let mut content = Vec::new();
                for capture in found.captures {
                    match query.capture_names()[capture.index as usize] {
                        "injection.language" => {
                            name = std::str::from_utf8(&bytes[capture.node.byte_range()]).ok()
                        }
                        "injection.content" => content.push(capture.node),
                        _ => {}
                    }
                }
                let Some(name) = name.and_then(|s| {
                    if s == "markdown_inline" {
                        Some("markdown_inline")
                    } else {
                        language_name(s)
                    }
                }) else {
                    continue;
                };
                for node in content {
                    let mut ranges = Vec::new();
                    let mut start = node.start_byte();
                    if !include_children {
                        let mut walk = node.walk();
                        for child in node.named_children(&mut walk) {
                            if start < child.start_byte() {
                                ranges.push(ts_range(budget, start..child.start_byte()));
                            }
                            start = child.end_byte();
                        }
                    }
                    if start < node.end_byte() {
                        ranges.push(ts_range(budget, start..node.end_byte()));
                    }
                    if !ranges.is_empty() {
                        range_count += ranges.len();
                        if range_count > 8192 {
                            return Err(AnalysisStatus::Limited("Too many injection ranges"));
                        }
                        if combine {
                            combined
                                .entry((name, found.pattern_index))
                                .or_default()
                                .extend(ranges);
                        } else {
                            injections.push((name, ranges));
                        }
                    }
                    if injections.len() > 4096 {
                        return Err(AnalysisStatus::Limited("Too many language injections"));
                    }
                }
            }
            drop(matches);
            if cursor.did_exceed_match_limit() {
                return Err(AnalysisStatus::Limited("Injection query match limit"));
            }
            for ((name, _), mut ranges) in combined {
                ranges.sort_by_key(|r| r.start_byte);
                ranges.dedup_by_key(|r| (r.start_byte, r.end_byte));
                injections.push((name, ranges));
            }
            for (name, ranges) in injections {
                budget.injections += 1;
                budget.injected_bytes += ranges
                    .iter()
                    .map(|r| r.end_byte - r.start_byte)
                    .sum::<usize>();
                if budget.injections > 4096 || budget.injected_bytes > MAX_SOURCE_BYTES * 3 {
                    return Err(AnalysisStatus::Limited(
                        "Language injection budget exceeded",
                    ));
                }
                let grammar = self.queries(name)?;
                let mut parser = Parser::new();
                parser
                    .set_language(&grammar.language)
                    .map_err(|e| AnalysisStatus::Failed(e.to_string()))?;
                parser
                    .set_included_ranges(&ranges)
                    .map_err(|e| AnalysisStatus::Failed(e.to_string()))?;
                let tree = parse(&mut parser, bytes, None, budget)?;
                self.highlight(name, &tree, bytes, depth + 1, spans, budget)?;
            }
        }
        Ok(())
    }
}
fn ts_range(budget: &Budget<'_>, r: Range<usize>) -> tree_sitter::Range {
    tree_sitter::Range {
        start_byte: r.start,
        end_byte: r.end,
        start_point: budget.point(r.start),
        end_point: budget.point(r.end),
    }
}
fn local_ranges(
    query: Option<&Query>,
    tree: &Tree,
    bytes: &[u8],
    budget: &Budget<'_>,
) -> Result<HashSet<Range<usize>>, AnalysisStatus> {
    let Some(query) = query else {
        return Ok(HashSet::new());
    };
    let mut cursor = QueryCursor::new();
    cursor.set_match_limit(8192);
    let mut progress = |_: &tree_sitter::QueryCursorState| {
        if budget.check().is_ok() {
            ControlFlow::Continue(())
        } else {
            ControlFlow::Break(())
        }
    };
    let mut matches = cursor.matches_with_options(
        query,
        tree.root_node(),
        bytes,
        QueryCursorOptions::new().progress_callback(&mut progress),
    );
    let mut scopes = vec![tree.root_node().byte_range()];
    let mut definitions = Vec::new();
    let mut references = Vec::new();
    while let Some(found) = matches.next() {
        if scopes.len() + definitions.len() + references.len() > MAX_RESULTS {
            return Err(AnalysisStatus::Limited("Too many locals"));
        }
        for capture in found.captures {
            match query.capture_names()[capture.index as usize] {
                "local.scope" => scopes.push(capture.node.byte_range()),
                "local.definition" => definitions.push(capture.node.byte_range()),
                "local.reference" => references.push(capture.node.byte_range()),
                _ => {}
            }
        }
    }
    drop(matches);
    if cursor.did_exceed_match_limit() {
        return Err(AnalysisStatus::Limited("Locals query match limit"));
    }
    let mut bindings: HashMap<&[u8], Vec<Range<usize>>> = HashMap::new();
    let mut local = definitions.clone();
    for definition in definitions {
        budget.check()?;
        let scope = scopes
            .iter()
            .filter(|s| s.start <= definition.start && s.end >= definition.end)
            .min_by_key(|s| s.len())
            .unwrap();
        bindings
            .entry(&bytes[definition.clone()])
            .or_default()
            .push(definition.start..scope.end);
    }
    for reference in references {
        budget.check()?;
        if bindings
            .get(&bytes[reference.clone()])
            .is_some_and(|definitions| definitions.iter().any(|d| d.contains(&reference.start)))
        {
            local.push(reference);
        }
    }
    Ok(local.into_iter().collect())
}
struct RawSpan {
    range: Range<usize>,
    capture: Arc<str>,
    priority: (usize, usize),
}
fn resolve_spans(raw: Vec<RawSpan>, budget: &Budget<'_>) -> Result<Vec<Highlight>, AnalysisStatus> {
    let mut events = Vec::with_capacity(raw.len() * 2);
    for (i, s) in raw.iter().enumerate() {
        events.push((s.range.start, true, i));
        events.push((s.range.end, false, i));
    }
    events.sort_unstable();
    let mut active = vec![false; raw.len()];
    let mut heap: BinaryHeap<(usize, usize, usize, usize)> = BinaryHeap::new();
    let mut result: Vec<Highlight> = Vec::new();
    let mut previous = 0;
    for (at, start, i) in events {
        budget.check()?;
        while heap.peek().is_some_and(|&(_, _, _, i)| !active[i]) {
            heap.pop();
        }
        if at > previous
            && let Some(&(_, _, _, i)) = heap.peek()
        {
            let span: &RawSpan = &raw[i];
            if let Some(last) = result.last_mut()
                && last.range.end == previous
                && last.capture == span.capture
            {
                last.range.end = at;
            } else {
                result.push(Highlight {
                    range: previous..at,
                    capture: span.capture.clone(),
                });
            }
            if result.len() > MAX_RESULTS {
                return Err(AnalysisStatus::Limited("Too many resolved highlights"));
            }
        }
        active[i] = start;
        if start {
            heap.push((
                raw[i].priority.0,
                raw[i].priority.1,
                usize::MAX - raw[i].range.len(),
                i,
            ));
        }
        previous = at;
    }
    Ok(result)
}
