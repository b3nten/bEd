//! Visual rows over a real document. Historical text never enters its buffer.
use crate::fold_animation::FoldVisual;
use crate::source_git::{SourceConflict, SourceGitPresentation};
use crate::views::view_layout::{column_at_x, glyph_advance_bytes, glyph_spans, line_column_x};
use bed_editing::{editor_events::DocumentChange, editor_state::EditorState, folding::FoldRange};
use dear_imgui_rs::Ui;
use std::{collections::HashMap, ops::Range, sync::Arc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffActionKind {
    Stage,
    Unstage,
    Discard,
}
impl DiffActionKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Stage => "Stage",
            Self::Unstage => "Unstage",
            Self::Discard => "Discard",
        }
    }
}
#[derive(Clone, Debug)]
pub struct DiffPresentation {
    pub baseline: Arc<[u8]>,
    /// Change this identity whenever the baseline or saved snapshot changes. The view
    /// caches its projection independently of the allocation backing the bytes.
    pub comparison: u64,
    pub actions: Vec<DiffActionKind>,
    /// Keep hunk controls visible while a pending operation prevents activation.
    /// This flag does not change the projection or its cached identity.
    pub actions_enabled: bool,
    /// Authoritative zero-context hunks for the saved comparison. Unsaved text
    /// is projected locally until the next saved snapshot becomes available.
    pub saved: Option<DiffSnapshot>,
}
#[derive(Clone, Debug)]
pub struct DiffSnapshot {
    pub current: Arc<[u8]>,
    pub hunks: Arc<[DiffHunkRange]>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffHunkRange {
    pub old_range: Range<usize>,
    pub new_range: Range<usize>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffAction {
    pub kind: DiffActionKind,
    pub comparison: u64,
    /// Zero-based line ranges, excluding the trailing empty editor row.
    pub old_range: Range<usize>,
    pub new_range: Range<usize>,
    pub revision: (u64, u64),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffLineKind {
    Context,
    Added,
}
#[derive(Clone, Debug)]
pub(crate) enum ProjectedRow {
    Document {
        row: i32,
        old_row: Option<usize>,
        kind: DiffLineKind,
    },
    Historical {
        old_row: usize,
        bytes: Arc<[u8]>,
    },
    Hunk {
        old_range: Range<usize>,
        new_range: Range<usize>,
    },
    Conflict(SourceConflict),
}
#[derive(Clone, Debug, Default)]
pub(crate) struct RowProjection {
    pub rows: Vec<ProjectedRow>,
    document_rows: Vec<usize>,
    identity_lines: usize,
    segments: Vec<Option<WrapSegment>>,
    wrap_width: Option<f32>,
    projected: bool,
    /// Hidden document rows resolve to their visible fold header for scrolling.
    fold_headers: Vec<Option<i32>>,
    folded_lines: Vec<usize>,
    fold_visuals: Vec<FoldVisual>,
    target_fold_headers: Vec<Option<i32>>,
    row_tops: Vec<f32>,
    row_heights: Vec<f32>,
}

#[derive(Clone, Debug, PartialEq)]
struct WrapSegment {
    range: Range<usize>,
    /// Virtual leading space on continuation rows; never enters the document.
    indent: f32,
}

/// Document-line wraps survive edits to other lines. Glyph metrics belong to
/// the current font and can be reused when only the available width changes.
#[derive(Default)]
pub(crate) struct WrapCache {
    document_ranges: Vec<Option<Vec<WrapSegment>>>,
    metrics: Option<(usize, u32)>,
    width: Option<u32>,
    advances: HashMap<Vec<u8>, f32>,
    #[cfg(test)]
    measured_lines: usize,
}
impl WrapCache {
    pub fn clear(&mut self) {
        self.document_ranges.clear();
    }
    /// Changes use successive pre-edit line coordinates, in operation order.
    pub fn invalidate(&mut self, changes: &[DocumentChange]) {
        if changes.is_empty() {
            self.clear();
            return;
        }
        for change in changes {
            if self.document_ranges.is_empty() {
                return;
            }
            if change.start_line < 0
                || change.end_line < change.start_line
                || change.end_line as usize >= self.document_ranges.len()
            {
                self.clear();
                return;
            }
            let inserted_rows = EditorState::split_lines(&change.text).0.len();
            self.document_ranges.splice(
                change.start_line as usize..=change.end_line as usize,
                std::iter::repeat_with(|| None).take(inserted_rows),
            );
        }
    }
    fn prepare(&mut self, ui: &Ui, state: &EditorState, width: f32) {
        let font = ui.with_bound_context(|| unsafe { dear_imgui_rs::sys::igGetFont() as usize });
        let metrics = (font, ui.current_font_size().to_bits());
        if self.metrics != Some(metrics) {
            self.advances.clear();
            self.metrics = Some(metrics);
            self.clear();
        }
        if self.width != Some(width.to_bits()) {
            self.width = Some(width.to_bits());
            self.clear();
        }
        let lines = state.line_count() as usize;
        if self.document_ranges.len() != lines {
            self.clear();
            self.document_ranges.resize_with(lines, || None);
        }
    }
    fn measure_line(&mut self, ui: &Ui, line: &[u8], width: f32, space: f32) -> Vec<WrapSegment> {
        #[cfg(test)]
        {
            self.measured_lines += 1;
        }
        wrap_ranges(line, width, space, |glyph| {
            if let Some(advance) = self.advances.get(glyph) {
                *advance
            } else {
                let advance = glyph_advance_bytes(ui, glyph);
                self.advances.insert(glyph.to_vec(), advance);
                advance
            }
        })
    }
}

#[derive(Clone, Copy)]
enum Edit {
    Equal(usize, usize),
    Delete(usize),
    Insert(usize),
}
fn lines(bytes: &[u8]) -> Vec<Vec<u8>> {
    if bytes.is_empty() {
        return Vec::new();
    }
    // Compare the same line representation as the text editor, including CR-only
    // files. Retain a newline sentinel while omitting the synthetic EOF row.
    let (parts, _) = EditorState::split_lines(bytes);
    let last = parts.len() - 1;
    parts
        .into_iter()
        .enumerate()
        .filter_map(|(index, mut line)| {
            if index == last && line.is_empty() {
                return None;
            }
            if index < last {
                line.push(b'\n');
            }
            Some(line)
        })
        .collect()
}
fn display_line(line: &[u8]) -> Vec<u8> {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    line.strip_suffix(b"\r").unwrap_or(line).to_vec()
}
fn authoritative_edits(
    old: &[Vec<u8>],
    new: &[Vec<u8>],
    hunks: &[DiffHunkRange],
) -> Option<Vec<Edit>> {
    let (mut old_at, mut new_at) = (0, 0);
    let mut result = Vec::new();
    for hunk in hunks {
        let (o, n) = (&hunk.old_range, &hunk.new_range);
        if o.start < old_at
            || n.start < new_at
            || o.start > o.end
            || n.start > n.end
            || o.end > old.len()
            || n.end > new.len()
            || o.start - old_at != n.start - new_at
        {
            return None;
        }
        for offset in 0..o.start - old_at {
            if old[old_at + offset] != new[new_at + offset] {
                return None;
            }
            result.push(Edit::Equal(old_at + offset, new_at + offset));
        }
        result.extend(o.clone().map(Edit::Delete));
        result.extend(n.clone().map(Edit::Insert));
        old_at = o.end;
        new_at = n.end;
    }
    if old.len() - old_at != new.len() - new_at {
        return None;
    }
    for offset in 0..old.len() - old_at {
        if old[old_at + offset] != new[new_at + offset] {
            return None;
        }
        result.push(Edit::Equal(old_at + offset, new_at + offset));
    }
    Some(result)
}
fn text_lines(bytes: &[u8]) -> Vec<Vec<u8>> {
    lines(bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes))
}
fn edits(old: &[Vec<u8>], new: &[Vec<u8>]) -> Vec<Edit> {
    let mut prefix = 0;
    while prefix < old.len().min(new.len()) && old[prefix] == new[prefix] {
        prefix += 1;
    }
    let (mut oe, mut ne) = (old.len(), new.len());
    while oe > prefix && ne > prefix && old[oe - 1] == new[ne - 1] {
        oe -= 1;
        ne -= 1;
    }
    let (n, m) = (oe - prefix, ne - prefix);
    let mut out: Vec<_> = (0..prefix).map(|i| Edit::Equal(i, i)).collect();
    if n.saturating_mul(m) > 1_000_000 || n == 0 || m == 0 {
        out.extend((prefix..oe).map(Edit::Delete));
        out.extend((prefix..ne).map(Edit::Insert));
    } else {
        let cols = m + 1;
        let mut dp = vec![0u32; (n + 1) * cols];
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                dp[i * cols + j] = if old[prefix + i] == new[prefix + j] {
                    1 + dp[(i + 1) * cols + j + 1]
                } else {
                    dp[(i + 1) * cols + j].max(dp[i * cols + j + 1])
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < n && j < m {
            if old[prefix + i] == new[prefix + j] {
                out.push(Edit::Equal(prefix + i, prefix + j));
                i += 1;
                j += 1;
            } else if dp[(i + 1) * cols + j] >= dp[i * cols + j + 1] {
                out.push(Edit::Delete(prefix + i));
                i += 1;
            } else {
                out.push(Edit::Insert(prefix + j));
                j += 1;
            }
        }
        out.extend((i..n).map(|i| Edit::Delete(prefix + i)));
        out.extend((j..m).map(|j| Edit::Insert(prefix + j)));
    }
    out.extend((0..old.len() - oe).map(|i| Edit::Equal(oe + i, ne + i)));
    out
}
impl RowProjection {
    pub fn build(
        state: &EditorState,
        diff: Option<&DiffPresentation>,
        git: Option<&SourceGitPresentation>,
    ) -> Self {
        if diff.is_none() && git.is_none_or(|git| git.conflicts.is_empty()) {
            return Self {
                identity_lines: state.line_count() as usize,
                ..Default::default()
            };
        }
        let mut rows = Vec::new();
        if let Some(diff) = diff {
            let old = text_lines(&diff.baseline);
            let new = lines(&state.join());
            let changes = diff
                .saved
                .as_ref()
                .filter(|snapshot| text_lines(&snapshot.current) == new)
                .and_then(|snapshot| authoritative_edits(&old, &new, &snapshot.hunks))
                .unwrap_or_else(|| edits(&old, &new));
            let (mut at, mut old_at, mut new_at) = (0, 0, 0);
            while at < changes.len() {
                if let Edit::Equal(o, n) = changes[at] {
                    rows.push(ProjectedRow::Document {
                        row: n as i32,
                        old_row: Some(o),
                        kind: DiffLineKind::Context,
                    });
                    old_at = o + 1;
                    new_at = n + 1;
                    at += 1;
                } else {
                    let start = at;
                    let (old_start, new_start) = (old_at, new_at);
                    while at < changes.len() && !matches!(changes[at], Edit::Equal(..)) {
                        match changes[at] {
                            Edit::Delete(o) => old_at = o + 1,
                            Edit::Insert(n) => new_at = n + 1,
                            _ => {}
                        }
                        at += 1;
                    }
                    rows.push(ProjectedRow::Hunk {
                        old_range: old_start..old_at,
                        new_range: new_start..new_at,
                    });
                    for change in &changes[start..at] {
                        match *change {
                            Edit::Delete(o) => rows.push(ProjectedRow::Historical {
                                old_row: o,
                                bytes: display_line(&old[o]).into(),
                            }),
                            Edit::Insert(n) => rows.push(ProjectedRow::Document {
                                row: n as i32,
                                old_row: None,
                                kind: DiffLineKind::Added,
                            }),
                            _ => {}
                        }
                    }
                }
            }
            // Editors always have an editable empty row, including after a final
            // newline. It is not a Git line and has no old-side line number.
            for row in new.len()..state.line_count() as usize {
                rows.push(ProjectedRow::Document {
                    row: row as i32,
                    old_row: None,
                    kind: DiffLineKind::Context,
                });
            }
        } else {
            rows.extend((0..state.line_count()).map(|row| ProjectedRow::Document {
                row,
                old_row: None,
                kind: DiffLineKind::Context,
            }));
        }
        if let Some(git) = git {
            for conflict in git.conflicts.iter().rev() {
                if let Some(index) = rows.iter().position(|entry| matches!(entry, ProjectedRow::Document { row, .. } if *row == conflict.row)) {
                    rows.insert(index, ProjectedRow::Conflict(conflict.clone()));
                }
            }
        }
        let mut document_rows = vec![0; state.line_count() as usize];
        for (visual, entry) in rows.iter().enumerate() {
            if let ProjectedRow::Document { row, .. } = entry {
                document_rows[*row as usize] = visual;
            }
        }
        Self {
            projected: rows.len() != document_rows.len(),
            rows,
            document_rows,
            identity_lines: 0,
            ..Default::default()
        }
    }
    /// Omit collapsed interiors from a fresh logical projection. Fold headers
    /// remain real document rows; document positions and selections do not move.
    pub fn fold(&mut self, collapsed: &[FoldRange]) {
        assert!(
            !self.is_wrapped(),
            "fold before wrapping a fresh projection"
        );
        assert!(!self.has_folds(), "fold a fresh logical projection");
        if collapsed.is_empty() {
            return;
        }
        let document_lines = if self.rows.is_empty() {
            self.identity_lines
        } else {
            self.document_rows.len()
        };
        self.fold_headers = vec![None; document_lines];
        self.folded_lines = vec![0; document_lines];
        let mut collapsed = collapsed.to_vec();
        // The outer range owns every hidden row, including nested headers.
        collapsed.sort_by_key(|range| (range.start_line, std::cmp::Reverse(range.end_line)));
        for range in collapsed {
            assert!(
                range.start_line >= 0
                    && range.end_line > range.start_line
                    && (range.end_line as usize) < document_lines,
                "fold range must be valid document coordinates"
            );
            let start = range.start_line as usize;
            let end = range.end_line as usize;
            if self.fold_headers[start].is_some() {
                continue;
            }
            self.folded_lines[start] = end - start;
            self.fold_headers[start + 1..=end].fill(Some(range.start_line));
        }
        if self.rows.is_empty() {
            self.rows
                .extend((0..document_lines).map(|row| ProjectedRow::Document {
                    row: row as i32,
                    old_row: None,
                    kind: DiffLineKind::Context,
                }));
        }
        self.rows.retain(|entry| match entry {
            ProjectedRow::Document { row, .. } => self.fold_headers[*row as usize].is_none(),
            _ => true,
        });
        self.document_rows = vec![0; document_lines];
        for (visual, entry) in self.rows.iter().enumerate() {
            if let ProjectedRow::Document { row, .. } = entry {
                self.document_rows[*row as usize] = visual;
            }
        }
        for (row, header) in self.fold_headers.iter().enumerate() {
            if let Some(header) = header {
                self.document_rows[row] = self.document_rows[*header as usize];
            }
        }
        self.identity_lines = 0;
        self.projected = true;
    }
    pub fn has_folds(&self) -> bool {
        !self.fold_headers.is_empty() || !self.fold_visuals.is_empty()
    }
    pub fn is_document_visible(&self, row: i32) -> bool {
        row >= 0
            && self
                .fold_headers
                .get(row as usize)
                .is_none_or(|header| header.is_none())
    }
    pub fn folded_lines(&self, row: i32) -> usize {
        if row < 0 {
            return 0;
        }
        self.folded_lines.get(row as usize).copied().unwrap_or(0)
    }
    pub fn fold_openness(&self, row: i32) -> f32 {
        self.fold_visuals
            .iter()
            .find(|fold| fold.range.start_line == row)
            .map_or(if self.folded_lines(row) > 0 { 0.0 } else { 1.0 }, |fold| {
                fold.openness
            })
    }
    /// Keep transitioning bodies available for wrapping. Target-collapsed
    /// bodies may still paint during closing, but cannot accept pointer input.
    pub fn fold_animated(&mut self, collapsed: &[FoldRange], visuals: &[FoldVisual]) {
        let closed: Vec<_> = visuals
            .iter()
            .filter(|fold| fold.openness == 0.0)
            .map(|fold| fold.range)
            .collect();
        self.fold(&closed);
        self.fold_visuals = visuals.to_vec();
        if visuals.is_empty() {
            return;
        }
        self.projected = true;
        let document_lines = if self.rows.is_empty() {
            self.identity_lines
        } else {
            self.document_rows.len()
        };
        self.target_fold_headers = vec![None; document_lines];
        self.folded_lines.resize(document_lines, 0);
        let mut collapsed = collapsed.to_vec();
        collapsed.sort_by_key(|range| (range.start_line, std::cmp::Reverse(range.end_line)));
        for range in collapsed {
            let start = range.start_line as usize;
            let end = range.end_line as usize;
            assert!(
                range.start_line >= 0 && end > start && end < document_lines,
                "fold target must be valid document coordinates"
            );
            if self.target_fold_headers[start].is_some() {
                continue;
            }
            self.target_fold_headers[start + 1..=end].fill(Some(range.start_line));
            if self.is_document_visible(range.start_line) {
                self.folded_lines[start] = end - start;
            }
        }
    }
    /// Crop each animated body from its bottom, after wrapping established its
    /// full visual height. Text keeps its normal size; only the final exposed
    /// row can have a fractional height, which renderers use as a clip boundary.
    pub fn crop_folds(&mut self, visuals: &[FoldVisual]) {
        assert!(self.row_tops.is_empty(), "crop a fresh projection once");
        let mut transitioning: Vec<_> = visuals
            .iter()
            .filter(|fold| fold.openness > 0.0 && fold.openness < 1.0)
            .copied()
            .collect();
        if transitioning.is_empty() {
            return;
        }
        let document_lines = if self.rows.is_empty() {
            self.identity_lines
        } else {
            self.document_rows.len()
        };
        if self.rows.is_empty() {
            self.rows
                .extend((0..document_lines).map(|row| ProjectedRow::Document {
                    row: row as i32,
                    old_row: None,
                    kind: DiffLineKind::Context,
                }));
        }
        if self.document_rows.is_empty() {
            self.document_rows = (0..document_lines).collect();
        }
        self.fold_headers.resize(document_lines, None);
        let mut heights = vec![1.0_f32; self.rows.len()];
        let mut crop_headers = vec![None; self.rows.len()];
        // Resolve nested geometry first so an outer reveal contains the child's
        // current height, including children that remain completely folded.
        transitioning.sort_by_key(|fold| fold.range.end_line - fold.range.start_line);
        for fold in transitioning {
            if !self.is_document_visible(fold.range.start_line) {
                continue;
            }
            let mut first = self.visual_row(fold.range.start_line);
            while self.document_row(first) == Some(fold.range.start_line) {
                first += 1;
            }
            let mut end = self.visual_row(fold.range.end_line);
            let last_document = self.document_row(end);
            while end < self.rows.len() && self.document_row(end) == last_document {
                end += 1;
            }
            let expanded: f32 = heights[first..end].iter().sum();
            let mut remaining = expanded * fold.openness;
            for visual in first..end {
                heights[visual] = heights[visual].min(remaining.max(0.0));
                remaining -= heights[visual];
                if heights[visual] == 0.0 {
                    crop_headers[visual] = Some(fold.range.start_line);
                }
            }
        }
        let logical = std::mem::take(&mut self.rows);
        let segments = std::mem::take(&mut self.segments);
        let mut visible = vec![false; document_lines];
        self.document_rows = vec![0; document_lines];
        let mut top = 0.0;
        for (visual, entry) in logical.into_iter().enumerate() {
            if heights[visual] == 0.0 {
                if let ProjectedRow::Document { row, .. } = entry {
                    self.fold_headers[row as usize] = crop_headers[visual];
                }
                continue;
            }
            if let ProjectedRow::Document { row, .. } = &entry {
                if !visible[*row as usize] {
                    self.document_rows[*row as usize] = self.rows.len();
                    visible[*row as usize] = true;
                }
            }
            self.rows.push(entry);
            if !segments.is_empty() {
                self.segments.push(segments[visual].clone());
            }
            self.row_tops.push(top);
            self.row_heights.push(heights[visual]);
            top += heights[visual];
        }
        // Some segments of a wrapped line can be cropped while its first
        // segment remains visible. Only entirely absent document lines hide.
        for (row, is_visible) in visible.into_iter().enumerate() {
            if is_visible {
                self.fold_headers[row] = None;
            } else if let Some(mut header) = self.fold_headers[row] {
                if let Some(outer) = self.fold_headers[header as usize] {
                    header = outer;
                }
                self.fold_headers[row] = Some(header);
                self.document_rows[row] = self.document_rows[header as usize];
            }
        }
        self.identity_lines = 0;
        self.projected = true;
    }
    pub fn row_top(&self, visual: usize) -> f32 {
        if visual >= self.len() {
            self.total_rows()
        } else {
            self.row_tops.get(visual).copied().unwrap_or(visual as f32)
        }
    }
    pub fn is_cropped(&self) -> bool {
        !self.row_tops.is_empty()
    }
    pub fn row_height(&self, visual: usize) -> f32 {
        if visual >= self.len() {
            0.0
        } else {
            self.row_heights.get(visual).copied().unwrap_or(1.0)
        }
    }
    pub fn total_rows(&self) -> f32 {
        self.row_tops
            .last()
            .zip(self.row_heights.last())
            .map_or(self.len() as f32, |(top, height)| top + height)
    }
    pub fn visual_at_y(&self, y: f32) -> usize {
        if self.row_tops.is_empty() {
            (y.max(0.0) as usize).min(self.len().saturating_sub(1))
        } else {
            self.row_tops
                .partition_point(|top| *top <= y.max(0.0))
                .saturating_sub(1)
                .min(self.len().saturating_sub(1))
        }
    }
    pub fn interactive_document_row(&self, visual: usize) -> Option<i32> {
        let row = self.document_row(visual)?;
        (self.row_height(visual) >= 1.0
            && self
                .target_fold_headers
                .get(row as usize)
                .is_none_or(|header| header.is_none()))
        .then_some(row)
    }
    /// Reflow a fresh logical projection. All consumers share these byte ranges;
    /// document storage and syntax-highlight coordinates remain unchanged.
    pub fn wrap(&mut self, ui: &Ui, state: &EditorState, width: f32) {
        self.wrap_cached(ui, state, width, &mut WrapCache::default());
    }
    pub fn wrap_cached(&mut self, ui: &Ui, state: &EditorState, width: f32, cache: &mut WrapCache) {
        assert!(!self.is_wrapped(), "wrap a fresh logical projection");
        let width = width.max(1.0);
        cache.prepare(ui, state, width);
        let mut logical = std::mem::take(&mut self.rows);
        if logical.is_empty() {
            logical.extend((0..self.identity_lines).map(|row| ProjectedRow::Document {
                row: row as i32,
                old_row: None,
                kind: DiffLineKind::Context,
            }));
        }
        self.document_rows = vec![0; state.line_count() as usize];
        let mut line = Vec::new();
        let space = glyph_advance_bytes(ui, b" ");
        for entry in logical {
            let historical_ranges;
            let ranges = match &entry {
                ProjectedRow::Document { row, .. } => {
                    self.document_rows[*row as usize] = self.rows.len();
                    if cache.document_ranges[*row as usize].is_none() {
                        state.line_into(*row, &mut line, usize::MAX);
                        let ranges = cache.measure_line(ui, &line, width, space);
                        cache.document_ranges[*row as usize] = Some(ranges);
                    }
                    cache.document_ranges[*row as usize].as_ref().unwrap()
                }
                ProjectedRow::Historical { bytes, .. } => {
                    historical_ranges = cache.measure_line(ui, bytes, width, space);
                    &historical_ranges
                }
                _ => {
                    self.rows.push(entry);
                    self.segments.push(None);
                    continue;
                }
            };
            for range in ranges {
                self.rows.push(entry.clone());
                self.segments.push(Some(range.clone()));
            }
        }
        // Wrapping moved visible headers. Hidden rows continue to point to the
        // header's first segment rather than an unrelated visual row.
        for (row, header) in self.fold_headers.iter().enumerate() {
            if let Some(header) = header {
                self.document_rows[row] = self.document_rows[*header as usize];
            }
        }
        self.wrap_width = Some(width);
        self.identity_lines = 0;
    }
    pub fn segment(&self, visual: usize) -> Option<Range<usize>> {
        self.segments
            .get(visual)
            .and_then(|s| s.as_ref())
            .map(|s| s.range.clone())
    }
    pub fn segment_indent(&self, visual: usize) -> f32 {
        self.segments
            .get(visual)
            .and_then(|s| s.as_ref())
            .map_or(0.0, |s| s.indent)
    }
    pub fn is_wrapped(&self) -> bool {
        self.wrap_width.is_some()
    }
    pub fn wrap_width(&self) -> Option<f32> {
        self.wrap_width
    }
    pub fn visual_position(&self, row: i32, column: i32) -> usize {
        let first = self.visual_row(row);
        if !self.is_wrapped() {
            return first;
        }
        let mut visual = first;
        while self.document_row(visual + 1) == Some(row)
            && self
                .segment(visual + 1)
                .is_some_and(|s| s.start <= column.max(0) as usize)
        {
            visual += 1;
        }
        visual
    }
    pub fn position_x(
        &self,
        ui: &Ui,
        state: &EditorState,
        row: i32,
        column: i32,
        origin: f32,
    ) -> f32 {
        let (row, column) = match self
            .fold_headers
            .get(row.max(0) as usize)
            .copied()
            .flatten()
        {
            Some(header) => (header, state.line_length(header)),
            None => (row, column),
        };
        let line = state.line(row);
        let visual = self.visual_position(row, column);
        let range = self.segment(visual).unwrap_or(0..line.len());
        line_column_x(
            ui,
            &line[range.clone()],
            column - range.start as i32,
            origin + self.segment_indent(visual),
        )
    }
    pub fn hit_column(&self, ui: &Ui, state: &EditorState, visual: usize, x: f32) -> i32 {
        let row = self
            .document_row(visual)
            .unwrap_or_else(|| self.nearest_document_row(visual));
        let line = state.line(row);
        let range = self.segment(visual).unwrap_or(0..line.len());
        bed_editing::util::utf8::snap_to_utf8_char_boundary(
            &line,
            range.start as i32 + column_at_x(ui, &line[range], x - self.segment_indent(visual)),
        )
    }
    pub fn len(&self) -> usize {
        if self.rows.is_empty() {
            self.identity_lines
        } else {
            self.rows.len()
        }
    }
    pub fn visual_row(&self, row: i32) -> usize {
        if self.rows.is_empty() {
            return row.max(0) as usize;
        }
        self.document_rows
            .get(row.max(0) as usize)
            .copied()
            .unwrap_or(0)
    }
    pub fn document_row(&self, visual: usize) -> Option<i32> {
        if self.rows.is_empty() {
            return (visual < self.identity_lines).then_some(visual as i32);
        }
        match self.rows.get(visual) {
            Some(ProjectedRow::Document { row, .. }) => Some(*row),
            _ => None,
        }
    }
    pub fn nearest_document_row(&self, visual: usize) -> i32 {
        if self.rows.is_empty() {
            return visual.min(self.identity_lines.saturating_sub(1)) as i32;
        }
        self.rows
            .iter()
            .skip(visual)
            .find_map(|row| match row {
                ProjectedRow::Document { row, .. } => Some(*row),
                _ => None,
            })
            .or_else(|| {
                self.rows
                    .iter()
                    .take(visual)
                    .rev()
                    .find_map(|row| match row {
                        ProjectedRow::Document { row, .. } => Some(*row),
                        _ => None,
                    })
            })
            .unwrap_or(0)
    }
    pub fn is_projected(&self) -> bool {
        self.projected
    }
}

/// Greedy whitespace wrapping with a glyph boundary fallback for long tokens.
/// Continuations inherit the line's leading indentation, capped at half the
/// width so deeply nested lines still have room for text. Tabs restart their
/// four-space stops relative to each segment's text origin. Every byte belongs
/// to exactly one segment, including trailing whitespace and empty lines.
fn wrap_ranges(
    line: &[u8],
    width: f32,
    space: f32,
    mut advance: impl FnMut(&[u8]) -> f32,
) -> Vec<WrapSegment> {
    let glyphs: Vec<_> = glyph_spans(line).collect();
    let leading_bytes = line
        .iter()
        .take_while(|b| **b == b' ' || **b == b'\t')
        .count();
    let mut leading_width = 0.0;
    for byte in &line[..leading_bytes] {
        leading_width += if *byte == b'\t' {
            let column = (leading_width / space) as i32;
            (((column / 4) + 1) * 4 - column) as f32 * space
        } else {
            space
        };
    }
    let continuation_indent = leading_width.min(width * 0.5);
    let mut ranges = Vec::new();
    let mut first = 0;
    while first < glyphs.len() {
        let indent = if first == 0 { 0.0 } else { continuation_indent };
        let available_width = width - indent;
        let mut x = 0.0;
        let mut end = first;
        let mut whitespace_end = None;
        while end < glyphs.len() {
            let (_, _, glyph) = glyphs[end];
            let w = if glyph == b"\t" {
                let column = (x / space) as i32;
                (((column / 4) + 1) * 4 - column) as f32 * space
            } else {
                advance(glyph)
            };
            if end > first && x + w > available_width {
                break;
            }
            x += w;
            end += 1;
            // Leading whitespace alone is not a useful word break: a long
            // identifier should start on the first row rather than leave it blank.
            if (glyph == b" " || glyph == b"\t") && glyphs[end - 1].1 > leading_bytes {
                whitespace_end = Some(end);
            }
        }
        if end < glyphs.len() {
            end = whitespace_end.unwrap_or(end);
        }
        ranges.push(WrapSegment {
            range: glyphs[first].0..glyphs[end - 1].1,
            indent,
        });
        first = end;
    }
    if ranges.is_empty() {
        ranges.push(WrapSegment {
            range: 0..0,
            indent: 0.0,
        });
    }
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn animated_body_reveals_normal_rows_and_crops_only_the_last_visible_row() {
        let mut state = EditorState::new();
        state.set_from_bytes(b"before\nheader\na\nb\nc\nd\nafter");
        let range = FoldRange {
            start_line: 1,
            end_line: 5,
        };
        let visual = [FoldVisual {
            range,
            openness: 0.375,
        }];
        for collapsed in [vec![range], vec![]] {
            let mut projection = RowProjection::build(&state, None, None);
            projection.fold_animated(&collapsed, &visual);
            projection.crop_folds(&visual);
            assert_eq!(projection.len(), 5);
            assert_eq!(projection.total_rows(), 4.5);
            assert_eq!(projection.document_row(2), Some(2));
            assert_eq!(projection.document_row(3), Some(3));
            assert_eq!(projection.document_row(4), Some(6));
            assert_eq!(projection.row_top(2), 2.0);
            assert_eq!(projection.row_height(2), 1.0);
            assert_eq!(projection.row_top(3), 3.0);
            assert_eq!(projection.row_height(3), 0.5);
            assert_eq!(projection.row_top(4), 3.5);
            assert_eq!(projection.visual_at_y(3.49), 3);
            assert_eq!(projection.visual_at_y(3.5), 4);
            assert!(projection.is_document_visible(3));
            assert!(!projection.is_document_visible(4));
            assert_eq!(projection.visual_row(4), 1);
            assert_eq!(projection.interactive_document_row(1), Some(1));
            assert_eq!(
                projection.interactive_document_row(2),
                collapsed.is_empty().then_some(2)
            );
            assert_eq!(
                projection.interactive_document_row(3),
                None,
                "partially cropped text is not clickable"
            );
            assert_eq!(projection.fold_openness(1), 0.375);
            assert!(projection.has_folds());
        }
    }

    #[test]
    fn cropping_parent_keeps_nested_fold_geometry_and_resolves_hidden_header() {
        let mut state = EditorState::new();
        state.set_from_bytes(b"before\nouter\na\ninner\nb\nc\nd\ne\nafter");
        let outer = FoldRange {
            start_line: 1,
            end_line: 7,
        };
        let inner = FoldRange {
            start_line: 3,
            end_line: 5,
        };
        let visual = [
            FoldVisual {
                range: outer,
                openness: 0.125,
            },
            FoldVisual {
                range: inner,
                openness: 0.0,
            },
        ];
        let mut projection = RowProjection::build(&state, None, None);
        projection.fold_animated(&[inner], &visual);
        projection.crop_folds(&visual);
        assert_eq!(projection.len(), 4);
        assert_eq!(projection.total_rows(), 3.5);
        assert_eq!(projection.document_row(2), Some(2));
        assert_eq!(projection.row_height(2), 0.5);
        assert_eq!(projection.document_row(3), Some(8));
        for row in 3..=7 {
            assert!(!projection.is_document_visible(row));
            assert_eq!(
                projection.visual_row(row),
                1,
                "nested hidden header resolves to visible outer header"
            );
        }
        assert_eq!(projection.interactive_document_row(3), Some(8));
    }

    #[test]
    fn animated_crop_measures_wrapped_body_height_without_cropping_header_segments() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = dear_imgui_rs::Context::create();
        context
            .set_ini_filename(None::<std::path::PathBuf>)
            .unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        context.prepare_frame(dear_imgui_rs::FramePrepareOptions::new(
            [640.0, 480.0],
            1.0 / 60.0,
        ));
        let ui = context.frame();
        ui.window("Animated wrapped document").build(|| {
            let mut state = EditorState::new();
            state.set_from_bytes(b"AAAAAA\nBBBBBB\nCCCCCC\nD");
            let range = FoldRange {
                start_line: 0,
                end_line: 2,
            };
            let visual = [FoldVisual {
                range,
                openness: 0.375,
            }];
            let mut projection = RowProjection::build(&state, None, None);
            projection.fold_animated(&[], &visual);
            projection.wrap(ui, &state, glyph_advance_bytes(ui, b"A") * 3.0 + 0.01);
            projection.crop_folds(&visual);
            assert_eq!(projection.len(), 5);
            assert_eq!(projection.total_rows(), 4.5);
            assert_eq!(projection.row_height(0), 1.0);
            assert_eq!(projection.row_height(1), 1.0);
            assert_eq!(projection.document_row(2), Some(1));
            assert_eq!(projection.segment(3), Some(3..6));
            assert_eq!(projection.row_height(3), 0.5);
            assert_eq!(projection.row_top(4), 3.5);
            assert_eq!(projection.document_row(4), Some(3));
            assert_eq!(projection.visual_position(1, 6), 3);
            assert_eq!(projection.visual_row(2), 0);
        });
        drop(context.render_legacy());
    }

    #[test]
    fn nested_folds_hide_interiors_without_changing_document_positions() {
        let mut state = EditorState::new();
        let source = b"before\nouter {\ninner {\nx\n}\n}\nafter";
        state.set_from_bytes(source);
        let mut projection = RowProjection::build(&state, None, None);
        projection.fold(&[
            FoldRange {
                start_line: 2,
                end_line: 4,
            },
            FoldRange {
                start_line: 1,
                end_line: 5,
            },
        ]);
        assert_eq!(projection.len(), 3);
        assert_eq!(projection.document_row(0), Some(0));
        assert_eq!(projection.document_row(1), Some(1));
        assert_eq!(projection.document_row(2), Some(6));
        assert_eq!(projection.nearest_document_row(2), 6);
        assert_eq!(projection.folded_lines(1), 4);
        assert_eq!(projection.folded_lines(2), 0, "nested header is hidden");
        for row in 2..=5 {
            assert!(!projection.is_document_visible(row));
            assert_eq!(projection.visual_row(row), 1);
            assert_eq!(projection.visual_position(row, 100), 1);
        }
        assert!(projection.is_document_visible(1));
        assert!(projection.is_document_visible(6));
        assert!(projection.has_folds() && projection.is_projected());
        assert_eq!(state.join(), source);

        // Unfolding the outer range leaves the nested range collapsed.
        let mut projection = RowProjection::build(&state, None, None);
        projection.fold(&[FoldRange {
            start_line: 2,
            end_line: 4,
        }]);
        assert_eq!(projection.len(), 5);
        assert_eq!(projection.document_row(2), Some(2));
        assert_eq!(projection.document_row(3), Some(5));
        assert_eq!(projection.folded_lines(2), 2);
    }

    #[test]
    fn no_collapsed_folds_preserves_identity_projection() {
        let mut state = EditorState::new();
        state.set_from_bytes(b"a\nb\nc");
        let mut projection = RowProjection::build(&state, None, None);
        projection.fold(&[]);
        assert_eq!(projection.len(), 3);
        assert!(!projection.has_folds() && !projection.is_projected());
        for row in 0..3 {
            assert!(projection.is_document_visible(row));
            assert_eq!(projection.visual_row(row), row as usize);
            assert_eq!(projection.document_row(row as usize), Some(row));
        }
    }

    #[test]
    fn folding_then_wrapping_maps_hidden_rows_to_visible_header() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = dear_imgui_rs::Context::create();
        context
            .set_ini_filename(None::<std::path::PathBuf>)
            .unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        context.prepare_frame(dear_imgui_rs::FramePrepareOptions::new(
            [640.0, 480.0],
            1.0 / 60.0,
        ));
        let ui = context.frame();
        ui.window("Folded wrapped document").build(|| {
            let mut state = EditorState::new();
            state.set_from_bytes(b"AAAAAA\nBBBBBB\nC\nD\nEE");
            let mut projection = RowProjection::build(&state, None, None);
            projection.fold(&[FoldRange {
                start_line: 1,
                end_line: 3,
            }]);
            let mut cache = WrapCache::default();
            projection.wrap_cached(
                ui,
                &state,
                glyph_advance_bytes(ui, b"A") * 3.0 + 0.01,
                &mut cache,
            );
            assert_eq!(projection.len(), 5);
            assert_eq!(projection.visual_row(1), 2);
            assert_eq!(projection.visual_position(1, 6), 3);
            assert_eq!(projection.visual_row(2), 2);
            assert_eq!(projection.visual_position(3, 100), 2);
            assert_eq!(projection.visual_row(4), 4);
            assert_eq!(projection.document_row(3), Some(1));
            assert_eq!(projection.document_row(4), Some(4));
            assert_eq!(projection.folded_lines(1), 2);
            assert_eq!(
                cache.measured_lines, 3,
                "hidden lines require no glyph layout"
            );
            assert_eq!(
                projection.position_x(ui, &state, 2, 100, 0.0),
                projection.position_x(ui, &state, 1, 6, 0.0),
                "hidden position uses the header rather than slicing hidden text"
            );
        });
        drop(context.render_legacy());
    }

    #[test]
    fn large_wrapped_document_reuses_unchanged_line_layouts_after_an_edit() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = dear_imgui_rs::Context::create();
        context
            .set_ini_filename(None::<std::path::PathBuf>)
            .unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        context.prepare_frame(dear_imgui_rs::FramePrepareOptions::new(
            [640.0, 480.0],
            1.0 / 60.0,
        ));
        let ui = context.frame();
        ui.window("Large wrapped document").build(|| {
            let bytes = vec!["x".repeat(160); 20_000].join("\n").into_bytes();
            let mut state = EditorState::new();
            state.set_from_bytes(&bytes);
            let mut cache = WrapCache::default();
            let width = glyph_advance_bytes(ui, b"x") * 40.0 + 0.1;
            let began = std::time::Instant::now();
            let mut projection = RowProjection::build(&state, None, None);
            projection.wrap_cached(ui, &state, width, &mut cache);
            let initial = began.elapsed();
            assert_eq!(projection.len(), 80_000);
            assert_eq!(cache.measured_lines, 20_000);
            let mut edited = bytes.clone();
            edited.insert(10_000 * 161 + 160, b'x');
            state.set_from_bytes(&edited);
            cache.invalidate(&[DocumentChange {
                start_line: 10_000,
                end_line: 10_000,
                start_character: 160,
                end_character: 160,
                text: vec![b'x'],
            }]);
            let began = std::time::Instant::now();
            let mut projection = RowProjection::build(&state, None, None);
            projection.wrap_cached(ui, &state, width, &mut cache);
            let edit = began.elapsed();
            assert_eq!(projection.len(), 80_001);
            assert_eq!(
                cache.measured_lines, 20_001,
                "only the edited line needs glyph layout"
            );
            assert_eq!(projection.visual_row(10_001), 40_005);
            assert_eq!(
                projection.segment(projection.visual_position(10_000, 161)),
                Some(160..161)
            );
            println!(
                "3.2 MB / 20,000 lines: initial wrap {initial:?}, cached edit reflow {edit:?}"
            );
        });
        drop(context.render_legacy());
    }
    #[test]
    fn wrapping_preserves_all_bytes_and_uses_whitespace_before_splitting_tokens() {
        let ranges = |bytes: &[u8], width| {
            wrap_ranges(bytes, width, 1.0, |_| 1.0)
                .into_iter()
                .map(|s| s.range)
                .collect::<Vec<_>>()
        };
        assert_eq!(ranges(b"one two three", 7.0), vec![0..4, 4..8, 8..13]);
        assert_eq!(ranges(b"abcdefgh", 3.0), vec![0..3, 3..6, 6..8]);
        assert_eq!(ranges(b"", 3.0), vec![0..0]);
        assert_eq!(ranges(b"a\tb", 4.0), vec![0..2, 2..3]);
        assert_eq!(ranges("🙂éZ".as_bytes(), 1.0), vec![0..4, 4..6, 6..7]);
        assert_eq!(ranges(b"x", 0.5), vec![0..1]);
        for bytes in [
            &b"one two three  "[..],
            &b"\t\tabc\tdef"[..],
            "🙂é Z".as_bytes(),
        ] {
            for width in 1..10 {
                let segments = ranges(bytes, width as f32);
                let joined: Vec<_> = segments
                    .iter()
                    .flat_map(|s| bytes[s.clone()].iter().copied())
                    .collect();
                assert_eq!(joined, bytes);
            }
        }
    }

    #[test]
    fn continuation_rows_preserve_space_and_tab_indent_with_room_for_text() {
        for (line, indent) in [
            (&b"    AAAA BBBB CCCC"[..], 4.0),
            (&b" \tAAAA BBBB CCCC"[..], 4.0),
            (&b"\t\tAAAA BBBB CCCC"[..], 6.0),
        ] {
            let segments = wrap_ranges(line, 12.0, 1.0, |_| 1.0);
            assert!(segments.len() > 1);
            assert_eq!(segments[0].indent, 0.0);
            assert!(segments[1..].iter().all(|s| s.indent == indent));
            let joined: Vec<_> = segments
                .iter()
                .flat_map(|s| line[s.range.clone()].iter().copied())
                .collect();
            assert_eq!(joined, line, "indentation is only visual");
        }
        let segments = wrap_ranges(b"    abcdefgh", 8.0, 1.0, |_| 1.0);
        assert_eq!(
            segments[0].range,
            0..8,
            "leading indent is not a word break"
        );
        assert_eq!(segments[1].range, 8..12);
        assert_eq!(segments[1].indent, 4.0);

        let segments = wrap_ranges(b"\t\tabcdefgh", 1.0, 1.0, |_| 1.0);
        assert_eq!(segments.len(), 10, "tiny panes still advance by a glyph");
        assert!(segments[1..].iter().all(|s| s.indent == 0.5));
        assert_eq!(segments.last().unwrap().range, 9..10);
        assert!(
            wrap_ranges(b"abcdefgh", 3.0, 1.0, |_| 1.0)
                .iter()
                .all(|s| s.indent == 0.0)
        );
    }

    #[test]
    fn wrapped_diff_keeps_history_controls_and_document_coordinates_separate() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = dear_imgui_rs::Context::create();
        context
            .set_ini_filename(None::<std::path::PathBuf>)
            .unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        context.prepare_frame(dear_imgui_rs::FramePrepareOptions::new(
            [640.0, 480.0],
            1.0 / 60.0,
        ));
        let ui = context.frame();
        ui.window("Wrapped projection").build(|| {
            let mut state = EditorState::new();
            state.set_from_bytes(b"abcdefgh\ntail");
            let mut p = projection(b"old old old\ntail", b"abcdefgh\ntail");
            let width = glyph_advance_bytes(ui, b"a") * 3.1;
            p.wrap(ui, &state, width);
            let first = p.visual_row(0);
            assert!(matches!(p.rows[0], ProjectedRow::Hunk { .. }));
            assert!(first > 1, "historical text also wraps");
            assert_eq!(p.segment(first), Some(0..3));
            assert_eq!(p.visual_position(0, 3), first + 1);
            assert_eq!(p.document_row(first + 1), Some(0));
            assert_eq!(p.hit_column(ui, &state, first + 1, 0.0), 3);
            assert_eq!(p.position_x(ui, &state, 0, 3, 10.0), 10.0);
            assert_eq!(p.visual_position(0, 8), first + 2);
            assert_eq!(state.join(), b"abcdefgh\ntail");
        });
        drop(context.render_legacy());
    }
    #[test]
    fn edit_reflow_reuses_unaffected_lines_and_tracks_inserted_and_deleted_rows() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = dear_imgui_rs::Context::create();
        context
            .set_ini_filename(None::<std::path::PathBuf>)
            .unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        context.prepare_frame(dear_imgui_rs::FramePrepareOptions::new(
            [640.0, 480.0],
            1.0 / 60.0,
        ));
        let ui = context.frame();
        ui.window("Cached wraps").build(|| {
            let mut state = EditorState::new();
            state.set_from_bytes(b"first words\nsecond words\nthird words");
            let mut cache = WrapCache::default();
            let width = glyph_advance_bytes(ui, b"a") * 6.1;
            let mut projection = RowProjection::build(&state, None, None);
            projection.wrap_cached(ui, &state, width, &mut cache);
            assert_eq!(cache.measured_lines, 3);
            let mut unchanged = RowProjection::build(&state, None, None);
            unchanged.wrap_cached(ui, &state, width, &mut cache);
            assert_eq!(
                cache.measured_lines, 3,
                "unchanged lines require no text measurement"
            );

            for (bytes, change, measured_lines) in [
                (
                    &b"first words\nnew\nsecond words\nthird words"[..],
                    DocumentChange {
                        start_line: 1,
                        end_line: 1,
                        text: b"new\n".to_vec(),
                        ..Default::default()
                    },
                    5,
                ),
                (
                    &b"first words\nsecond words\nthird words"[..],
                    DocumentChange {
                        start_line: 1,
                        end_line: 2,
                        ..Default::default()
                    },
                    6,
                ),
                (
                    &b"fresh words\nsecond words\nthird words"[..],
                    DocumentChange {
                        start_line: 0,
                        end_line: 0,
                        start_character: 0,
                        end_character: 5,
                        text: b"fresh".to_vec(),
                    },
                    7,
                ),
                (
                    &b"    fresh words\nsecond words\nthird words"[..],
                    DocumentChange {
                        start_line: 0,
                        end_line: 0,
                        text: b"    ".to_vec(),
                        ..Default::default()
                    },
                    8,
                ),
            ] {
                cache.invalidate(&[change]);
                state.set_from_bytes(bytes);
                let mut cached = RowProjection::build(&state, None, None);
                cached.wrap_cached(ui, &state, width, &mut cache);
                assert_eq!(
                    cache.measured_lines, measured_lines,
                    "only affected lines are measured"
                );
                let mut fresh = RowProjection::build(&state, None, None);
                fresh.wrap(ui, &state, width);
                assert_eq!(cached.segments, fresh.segments);
                for visual in 0..fresh.len() {
                    assert_eq!(cached.document_row(visual), fresh.document_row(visual));
                }
            }
            assert!(cache.document_ranges[0].as_ref().unwrap()[1].indent > 0.0);

            let glyphs = cache.advances.len();
            let measured_lines = cache.measured_lines;
            let mut resized = RowProjection::build(&state, None, None);
            resized.wrap_cached(ui, &state, width * 2.0, &mut cache);
            assert_eq!(cache.measured_lines, measured_lines + 3);
            assert_eq!(
                cache.advances.len(),
                glyphs,
                "width changes reuse font metrics"
            );
            let font_size = ui.current_font_size();
            let old_advance = cache.advances[&b"f"[..]];
            let _font = ui.push_font_with_size(None, font_size * 2.0);
            let mut zoomed = RowProjection::build(&state, None, None);
            zoomed.wrap_cached(ui, &state, width * 2.0, &mut cache);
            assert!(
                cache.advances[&b"f"[..]] > old_advance * 1.5,
                "zoom refreshes font metrics"
            );
        });
        drop(context.render_legacy());
    }
    fn projection(old: &[u8], new: &[u8]) -> RowProjection {
        let mut state = EditorState::new();
        state.set_from_bytes(new);
        RowProjection::build(
            &state,
            Some(&DiffPresentation {
                baseline: old.into(),
                comparison: 1,
                actions: vec![],
                actions_enabled: true,
                saved: None,
            }),
            None,
        )
    }
    #[test]
    fn replacement_rows_keep_real_document_coordinates() {
        let p = projection(b"head\nold\ntail\n", b"head\nnew\ntail\n");
        assert!(
            matches!(&p.rows[1], ProjectedRow::Hunk { old_range, new_range } if *old_range == (1..2) && *new_range == (1..2))
        );
        assert!(
            matches!(&p.rows[2], ProjectedRow::Historical { bytes, .. } if bytes.as_ref() == b"old")
        );
        assert_eq!(p.visual_row(1), 3);
        assert_eq!(p.document_row(3), Some(1));
        assert_eq!(p.document_row(2), None);
        assert_eq!(p.nearest_document_row(2), 1);
        assert_eq!(p.visual_row(3), 5);
    }
    #[test]
    fn empty_files_deletions_and_missing_final_newlines_are_distinct() {
        assert_eq!(projection(b"", b"").rows.len(), 1);
        let p = projection(b"old\n", b"");
        assert!(matches!(&p.rows[0], ProjectedRow::Hunk { new_range, .. } if new_range.is_empty()));
        assert_eq!(p.document_row(p.visual_row(0)), Some(0));
        assert!(
            projection(b"line\n", b"line")
                .rows
                .iter()
                .any(|row| matches!(row, ProjectedRow::Hunk { .. }))
        );
        let p = projection(b"", b"new\n");
        assert!(matches!(&p.rows[0], ProjectedRow::Hunk { old_range, .. } if old_range.is_empty()));
    }
    #[test]
    fn every_live_row_appears_once_even_with_conflict_controls() {
        let mut state = EditorState::new();
        state.set_from_bytes(b"one\ntwo\nthree");
        let p = RowProjection::build(
            &state,
            None,
            Some(&SourceGitPresentation {
                conflicts: vec![SourceConflict {
                    id: 1,
                    row: 1,
                    ..Default::default()
                }],
            }),
        );
        assert!(matches!(p.rows[1], ProjectedRow::Conflict(_)));
        for row in 0..state.line_count() {
            assert_eq!(p.document_row(p.visual_row(row)), Some(row));
        }
    }
    #[test]
    fn display_normalizes_bom_and_all_editor_line_endings() {
        for bytes in [
            &b"\xef\xbb\xbfhead\r\ntail\r\n"[..],
            &b"head\rtail\r"[..],
            &b"head\ntail\n"[..],
        ] {
            let p = projection(bytes, b"head\ntail\n");
            assert!(
                !p.rows
                    .iter()
                    .any(|row| matches!(row, ProjectedRow::Hunk { .. }))
            );
            for row in 0..3 {
                assert_eq!(p.document_row(p.visual_row(row)), Some(row));
            }
        }
    }
    #[test]
    fn saved_git_hunks_remain_separate_above_the_local_diff_work_limit() {
        let old: Vec<u8> = (0..1100)
            .flat_map(|row| format!("line {row}\n").into_bytes())
            .collect();
        let mut new = old.clone();
        new.splice(0..6, b"changed".iter().copied());
        let last = new.len() - "line 1099\n".len();
        new.splice(last..new.len(), b"last changed\n".iter().copied());
        let mut state = EditorState::new();
        state.set_from_bytes(&new);
        let presentation = DiffPresentation {
            baseline: old.into(),
            comparison: 4,
            actions: vec![DiffActionKind::Stage],
            actions_enabled: true,
            saved: Some(DiffSnapshot {
                current: new.into(),
                hunks: vec![
                    DiffHunkRange {
                        old_range: 0..1,
                        new_range: 0..1,
                    },
                    DiffHunkRange {
                        old_range: 1099..1100,
                        new_range: 1099..1100,
                    },
                ]
                .into(),
            }),
        };
        let p = RowProjection::build(&state, Some(&presentation), None);
        let hunks: Vec<_> = p
            .rows
            .iter()
            .filter_map(|row| match row {
                ProjectedRow::Hunk {
                    old_range,
                    new_range,
                } => Some((old_range.clone(), new_range.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(hunks, vec![(0..1, 0..1), (1099..1100, 1099..1100)]);
        // Additional unsaved edits cannot accidentally reuse the saved ranges.
        state.set_from_bytes(b"entirely unsaved\n");
        let p = RowProjection::build(&state, Some(&presentation), None);
        assert!(
            matches!(&p.rows[0], ProjectedRow::Hunk { old_range, new_range } if *old_range == (0..1100) && *new_range == (0..1))
        );
    }
}
