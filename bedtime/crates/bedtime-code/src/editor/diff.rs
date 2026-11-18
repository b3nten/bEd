//! Git-independent, revision-bound inline comparisons. Historical rows are a
//! sparse projection over the real document, never document edits.
use super::{
    EditorError, EditorView, Navigation, Scroll, TextBuffer, TextSelection, TextSnapshot,
    background::Job,
    layout::{Metrics, VisibleLine},
    wrapping::WrapIndex,
};
use bedtime_ui::egui;
use std::{
    ops::Range,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangeRange {
    /// Zero-based logical lines, excluding the trailing empty editor line.
    pub old: Range<usize>,
    pub new: Range<usize>,
}
#[derive(Clone, Debug)]
pub struct DiffAction {
    pub id: u64,
    pub label: String,
    pub enabled: bool,
}
#[derive(Clone, Debug)]
pub struct DiffRequest {
    pub action: u64,
    pub comparison: u64,
    pub revision: u64,
    pub range: ChangeRange,
}
/// Immutable comparison input. Construction validates supplied authoritative
/// ranges against both snapshots before the renderer trusts their coordinates.
#[derive(Clone)]
pub struct Comparison {
    pub(crate) identity: u64,
    pub(crate) baseline: TextBuffer,
    saved: Option<(TextBuffer, Arc<[ChangeRange]>)>,
}
impl Comparison {
    pub fn new(identity: u64, baseline: TextBuffer) -> Self {
        Self {
            identity,
            baseline,
            saved: None,
        }
    }
    pub fn with_saved(
        mut self,
        current: TextBuffer,
        ranges: Vec<ChangeRange>,
    ) -> Result<Self, EditorError> {
        let old = lines(&self.baseline, &AtomicBool::new(false)).unwrap();
        let new = lines(&current, &AtomicBool::new(false)).unwrap();
        if !valid_ranges(&old, &new, &ranges) {
            return Err(EditorError("Invalid comparison line ranges".into()));
        }
        self.saved = Some((current, ranges.into()));
        Ok(self)
    }
}
fn lines(text: &TextBuffer, cancel: &AtomicBool) -> Option<Vec<String>> {
    let mut result = Vec::new();
    for row in 0..text.line_count() {
        if row % 256 == 0 && cancel.load(Ordering::Relaxed) {
            return None;
        }
        let start = text.line_start(row);
        let end = text.line_start(row + 1);
        if start < end {
            result.push(text.range_text(start..end));
        }
    }
    Some(result)
}
fn valid_ranges(old: &[String], new: &[String], ranges: &[ChangeRange]) -> bool {
    let (mut o, mut n) = (0, 0);
    for r in ranges {
        if r.old.start < o
            || r.new.start < n
            || r.old.end < r.old.start
            || r.new.end < r.new.start
            || r.old.end > old.len()
            || r.new.end > new.len()
            || (r.old.is_empty() && r.new.is_empty())
            || old[o..r.old.start] != new[n..r.new.start]
        {
            return false;
        }
        o = r.old.end;
        n = r.new.end;
    }
    old[o..] == new[n..]
}
/// Bounded line comparison for live buffers. Large unmatched regions remain a
/// single contiguous chunk; authoritative saved ranges retain Git's precision.
pub fn compare(
    baseline: &TextBuffer,
    current: &TextBuffer,
    cancel: &AtomicBool,
) -> Option<Vec<ChangeRange>> {
    let old = lines(baseline, cancel)?;
    let new = lines(current, cancel)?;
    let mut prefix = 0;
    while prefix < old.len().min(new.len()) && old[prefix] == new[prefix] {
        if prefix % 256 == 0 && cancel.load(Ordering::Relaxed) {
            return None;
        }
        prefix += 1;
    }
    let (mut oe, mut ne) = (old.len(), new.len());
    while oe > prefix && ne > prefix && old[oe - 1] == new[ne - 1] {
        oe -= 1;
        ne -= 1;
    }
    if oe == prefix && ne == prefix {
        return Some(Vec::new());
    }
    let (n, m) = (oe - prefix, ne - prefix);
    if n.saturating_mul(m) > 1_000_000 || n == 0 || m == 0 {
        return Some(vec![ChangeRange {
            old: prefix..oe,
            new: prefix..ne,
        }]);
    }
    let cols = m + 1;
    let mut dp = vec![0u32; (n + 1) * cols];
    for i in (0..n).rev() {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        for j in (0..m).rev() {
            dp[i * cols + j] = if old[prefix + i] == new[prefix + j] {
                1 + dp[(i + 1) * cols + j + 1]
            } else {
                dp[(i + 1) * cols + j].max(dp[i * cols + j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut ranges = Vec::new();
    let mut start = None;
    while i < n || j < m {
        if i < n && j < m && old[prefix + i] == new[prefix + j] {
            if let Some((o, a)) = start.take() {
                ranges.push(ChangeRange {
                    old: o..prefix + i,
                    new: a..prefix + j,
                });
            }
            i += 1;
            j += 1;
        } else {
            start.get_or_insert((prefix + i, prefix + j));
            if i < n && (j == m || dp[(i + 1) * cols + j] >= dp[i * cols + j + 1]) {
                i += 1;
            } else {
                j += 1;
            }
        }
    }
    if let Some((o, a)) = start {
        ranges.push(ChangeRange {
            old: o..oe,
            new: a..ne,
        });
    }
    Some(ranges)
}

struct Insertion {
    source: usize,
    visual: usize,
    count: usize,
    historical: Range<usize>,
}
pub(crate) enum Row {
    Document(usize),
    Historical(usize),
    Control(usize),
}
pub(crate) struct Projection {
    pub comparison: u64,
    pub revision: u64,
    pub width: Option<usize>,
    pub baseline: TextBuffer,
    pub ranges: Arc<[ChangeRange]>,
    historical_wrap: Option<WrapIndex>,
    insertions: Vec<Insertion>,
    extra: usize,
}
impl Projection {
    fn build(
        input: Comparison,
        snapshot: TextSnapshot,
        wrap: Option<WrapIndex>,
        cancel: &AtomicBool,
    ) -> Option<Self> {
        let ranges = if let Some((text, ranges)) =
            input.saved.as_ref().filter(|(t, _)| t == &snapshot.text)
        {
            let _ = text;
            ranges.clone()
        } else {
            compare(&input.baseline, &snapshot.text, cancel)?.into()
        };
        let historical_wrap = if let Some(w) = &wrap {
            Some(WrapIndex::build(
                TextSnapshot {
                    text: input.baseline.clone(),
                    revision: 0,
                },
                w.width,
                cancel,
            )?)
        } else {
            None
        };
        let mut extra = 0;
        let mut insertions = Vec::with_capacity(ranges.len());
        let row_at = |text: &TextBuffer, wrap: Option<&WrapIndex>, line: usize| {
            if line >= text.line_count() {
                wrap.map_or(text.line_count(), WrapIndex::rows)
            } else {
                wrap.map_or(line, |w| w.visual_row(text.line_start(line)))
            }
        };
        for r in ranges.iter() {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            let source = row_at(&snapshot.text, wrap.as_ref(), r.new.start);
            let historical = row_at(&input.baseline, historical_wrap.as_ref(), r.old.start)
                ..row_at(&input.baseline, historical_wrap.as_ref(), r.old.end);
            let count = historical.len() + 1;
            insertions.push(Insertion {
                source,
                visual: source + extra,
                count,
                historical,
            });
            extra += count;
        }
        Some(Self {
            comparison: input.identity,
            revision: snapshot.revision,
            width: wrap.as_ref().map(|w| w.width),
            baseline: input.baseline,
            ranges,
            historical_wrap,
            insertions,
            extra,
        })
    }
    pub fn rows(&self, source: usize) -> usize {
        source + self.extra
    }
    pub fn visual(&self, source: usize) -> usize {
        let i = self.insertions.partition_point(|r| r.source <= source);
        source
            + if i == 0 {
                0
            } else {
                let r = &self.insertions[i - 1];
                r.visual - r.source + r.count
            }
    }
    pub fn row(&self, visual: usize) -> Row {
        let i = self.insertions.partition_point(|r| r.visual <= visual);
        if i == 0 {
            return Row::Document(visual);
        }
        let r = &self.insertions[i - 1];
        let offset = visual - r.visual;
        if offset == 0 {
            Row::Control(i - 1)
        } else if offset < r.count {
            Row::Historical(r.historical.start + offset - 1)
        } else {
            Row::Document(visual - (r.visual - r.source + r.count))
        }
    }
    pub fn source(&self, visual: usize) -> usize {
        match self.row(visual) {
            Row::Document(row) => row,
            _ => {
                let i = self.insertions.partition_point(|r| r.visual <= visual);
                self.insertions[i - 1].source
            }
        }
    }
    pub fn historical_segment(&self, row: usize) -> super::wrapping::Segment {
        self.historical_wrap.as_ref().map_or_else(
            || super::wrapping::Segment {
                line: row,
                part: 0,
                bytes: self.baseline.line_range(row),
                columns: 0..self.baseline.column(self.baseline.line_range(row).end),
                indent: 0,
            },
            |w| w.segment(row),
        )
    }
    /// The change whose old lines visual `row` shows one of.
    pub fn chunk(&self, visual: usize) -> Option<usize> {
        let i = self.insertions.partition_point(|r| r.visual <= visual);
        matches!(self.row(visual), Row::Historical(_)).then(|| i - 1)
    }
    /// The text a change removed: the baseline's bytes, up to its last
    /// line's ending.
    fn removed(&self, chunk: usize) -> Range<usize> {
        let old = &self.ranges[chunk].old;
        if old.is_empty() {
            return 0..0;
        }
        self.baseline.line_start(old.start)..self.baseline.line_range(old.end - 1).end
    }
    /// An old row's text, laid out where it's drawn.
    pub fn historical_line(
        &self,
        ui: &egui::Ui,
        metrics: &Metrics,
        segment: &super::wrapping::Segment,
        cells: Range<usize>,
    ) -> VisibleLine {
        if self.width.is_some() {
            VisibleLine::fragment(
                ui,
                &self.baseline,
                metrics,
                segment.bytes.clone(),
                (segment.indent as f32 - segment.columns.start as f32) * metrics.cell,
            )
        } else {
            VisibleLine::new(ui, &self.baseline, metrics, segment.line, cells)
        }
    }
    /// The baseline byte at `position` (in the text's coordinates), kept
    /// within what `chunk` removed: above its old lines is their start, below
    /// them their end.
    fn historical_hit(
        &self,
        ui: &egui::Ui,
        metrics: &Metrics,
        chunk: usize,
        position: egui::Pos2,
    ) -> usize {
        let insertion = &self.insertions[chunk];
        let bytes = self.removed(chunk);
        let row = ((position.y - 6.0) / metrics.height).floor();
        if row <= insertion.visual as f32 {
            return bytes.start;
        }
        let offset = row as usize - insertion.visual - 1;
        if offset >= insertion.historical.len() {
            return bytes.end;
        }
        let column = ((position.x - 8.0) / metrics.cell).floor().max(0.0) as usize;
        let segment = self.historical_segment(insertion.historical.start + offset);
        self.historical_line(
            ui,
            metrics,
            &segment,
            column.saturating_sub(8)..column.saturating_add(16),
        )
        .hit(position.x - 8.0)
        .clamp(bytes.start, bytes.end)
    }
    /// What a press at `position` selects, if it's on a change's old lines:
    /// a caret there, a word on the second click, the line on the third.
    /// With `extend` (Shift, in the same change) from that selection's anchor.
    pub fn press(
        &self,
        ui: &egui::Ui,
        metrics: &Metrics,
        position: egui::Pos2,
        clicks: u8,
        extend: Option<&OldSelection>,
        document: TextSelection,
    ) -> Option<OldSelection> {
        let row = ((position.y - 6.0) / metrics.height).floor().max(0.0) as usize;
        let chunk = self.chunk(row)?;
        let hit = self.historical_hit(ui, metrics, chunk, position);
        let bytes = self.removed(chunk);
        let range = match clicks {
            3 => self.baseline.line_range(self.baseline.line_of(hit)),
            2 => super::word_range(&self.baseline, hit),
            _ => hit..hit,
        };
        let (mut anchor, head) = (range.start.max(bytes.start), range.end.min(bytes.end));
        if clicks == 1
            && let Some(old) = extend.filter(|old| {
                old.comparison == self.comparison
                    && old.revision == self.revision
                    && old.chunk == chunk
            })
        {
            anchor = old.anchor;
        }
        Some(OldSelection {
            comparison: self.comparison,
            revision: self.revision,
            chunk,
            anchor,
            head,
            document,
        })
    }
    /// Where dragging `selection` to `position` puts its head.
    pub fn drag(
        &self,
        ui: &egui::Ui,
        metrics: &Metrics,
        selection: &OldSelection,
        position: egui::Pos2,
    ) -> Option<usize> {
        (selection.comparison == self.comparison
            && selection.revision == self.revision
            && selection.chunk < self.insertions.len())
        .then(|| self.historical_hit(ui, metrics, selection.chunk, position))
    }
    pub fn old_line(&self, new_line: usize) -> Option<usize> {
        let i = self.ranges.partition_point(|r| r.new.start <= new_line);
        if i == 0 {
            return Some(new_line);
        }
        let r = &self.ranges[i - 1];
        if r.new.contains(&new_line) {
            None
        } else {
            Some(r.old.end + new_line - r.new.end)
        }
    }
}
/// Text selected in a comparison's old lines, to copy: bytes of the baseline,
/// within what one change removed. It's not part of the document, so it has
/// no caret and lasts only until the document's own selection changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct OldSelection {
    comparison: u64,
    revision: u64,
    chunk: usize,
    anchor: usize,
    pub head: usize,
    /// The document's selection when this was made.
    document: TextSelection,
}
/// What a projection is for: the comparison, the text's revision, the wrap width.
type ProjectionKey = (u64, u64, Option<usize>);

#[derive(Default)]
pub(crate) struct DiffState {
    input: Option<Comparison>,
    pub actions: Vec<DiffAction>,
    projection: Option<Projection>,
    job: Option<(ProjectionKey, Job<Projection>)>,
    /// Old lines' text selected, and whether a drag is selecting it.
    pub selection: Option<OldSelection>,
    pub selecting: bool,
}
impl DiffState {
    pub fn deselect(&mut self) {
        self.selection = None;
        self.selecting = false;
    }
    /// Drop the old lines' selection once the document's own has changed.
    pub fn keep_selection(&mut self, document: TextSelection, others: bool) {
        if self
            .selection
            .as_ref()
            .is_some_and(|old| others || old.document != document)
        {
            self.deselect();
        }
    }
    /// The change and the baseline bytes selected in its old lines, while
    /// the text is at the `revision` they were selected over.
    pub fn selected(&self, revision: u64) -> Option<(usize, Range<usize>)> {
        let old = self.selection.as_ref()?;
        (old.revision == revision
            && self
                .input
                .as_ref()
                .is_some_and(|input| input.identity == old.comparison))
        .then(|| {
            (
                old.chunk,
                old.anchor.min(old.head)..old.anchor.max(old.head),
            )
        })
    }
    /// The old lines' selected text, if any is.
    pub fn selected_text(&self, revision: u64) -> Option<String> {
        let (_, range) = self.selected(revision)?;
        let baseline = &self.input.as_ref()?.baseline;
        (!range.is_empty()).then(|| baseline.slice(range).to_string())
    }
    pub fn active(&self) -> bool {
        self.input.is_some()
    }
    pub fn current(&self, revision: u64, wrap: Option<&WrapIndex>) -> Option<&Projection> {
        self.projection.as_ref().filter(|p| {
            p.revision == revision
                && p.width == wrap.map(|w| w.width)
                && self
                    .input
                    .as_ref()
                    .is_some_and(|i| i.identity == p.comparison)
        })
    }
    pub fn prepare(
        &mut self,
        ctx: &egui::Context,
        snapshot: &TextSnapshot,
        wrap: Option<WrapIndex>,
    ) {
        let Some(input) = self.input.clone() else {
            return;
        };
        let key = (
            input.identity,
            snapshot.revision,
            wrap.as_ref().map(|w| w.width),
        );
        if let Some((job_key, job)) = &self.job {
            match job.poll() {
                Ok(Some(p)) => {
                    if *job_key == key {
                        self.projection = Some(p);
                    }
                    self.job = None;
                }
                Err(()) => {
                    self.job = None;
                }
                _ => {}
            }
        }
        if self.current(snapshot.revision, wrap.as_ref()).is_some() {
            return;
        }
        if self.job.as_ref().is_some_and(|(k, _)| *k == key) {
            return;
        }
        let snapshot = snapshot.clone();
        self.job = Some((
            key,
            Job::start(ctx, move |cancel| {
                Projection::build(input, snapshot, wrap, cancel)
            }),
        ));
    }
}
impl EditorView {
    pub fn set_comparison(&mut self, comparison: Option<Comparison>, actions: Vec<DiffAction>) {
        if self.diff.input.as_ref().map(|i| i.identity) != comparison.as_ref().map(|i| i.identity) {
            self.diff.projection = None;
            self.diff.job = None;
            self.diff.deselect();
        }
        self.diff.input = comparison;
        self.diff.actions = actions;
    }
    /// Whether text is selected in the comparison's old lines (Copy takes it).
    pub fn old_lines_selected(&self) -> bool {
        self.observed_revision
            .is_some_and(|revision| self.diff.selected_text(revision).is_some())
    }
    /// Copy with old lines' text selected takes it. Cut does nothing then:
    /// they aren't the document's to cut.
    pub(super) fn old_lines_event(
        &self,
        ui: &egui::Ui,
        event: &egui::Event,
        revision: u64,
    ) -> bool {
        let Some(text) = self.diff.selected_text(revision) else {
            return false;
        };
        match event {
            egui::Event::Copy => ui.ctx().copy_text(text),
            egui::Event::Cut => {}
            _ => return false,
        }
        true
    }
    /// Move to an adjacent chunk without moving the canonical caret.
    pub fn navigate_change(
        &mut self,
        snapshot: &TextSnapshot,
        forward: bool,
    ) -> Result<(), EditorError> {
        let Some(p) = self.diff.current(
            snapshot.revision,
            self.active_wrap(snapshot, self.settings.wrap.then_some(1)),
        ) else {
            return Ok(());
        };
        let line = snapshot.text.line_of(self.selection.head);
        let range = if forward {
            p.ranges
                .iter()
                .find(|r| r.new.start > line)
                .or(p.ranges.first())
        } else {
            p.ranges
                .iter()
                .rev()
                .find(|r| r.new.start < line)
                .or(p.ranges.last())
        };
        if let Some(r) = range {
            let byte = snapshot.text.line_start(r.new.start);
            self.request_navigation(
                snapshot,
                Navigation {
                    revision: snapshot.revision,
                    range: byte..byte,
                    selection: Some(super::TextSelection::caret(&snapshot.text, byte)?),
                    reveal: Scroll::Center,
                    focus: true,
                },
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::{
        EditorOutput, HistoryAction, MemoryTextSession, TextEdit, TextSession,
        folding::FoldMap,
        layout::{Geometry, Metrics},
        tests::frame,
    };
    fn projection(old: &str, new: &str, width: Option<usize>) -> Projection {
        let snapshot = TextSnapshot {
            text: TextBuffer::from(new),
            revision: 7,
        };
        let cancel = AtomicBool::new(false);
        let wrap = width.map(|w| WrapIndex::build(snapshot.clone(), w, &cancel).unwrap());
        Projection::build(
            Comparison::new(9, TextBuffer::from(old)),
            snapshot,
            wrap,
            &cancel,
        )
        .unwrap()
    }
    #[test]
    fn changes_cover_unicode_empty_eof_crlf_and_bom_without_changing_bytes() {
        for (old, new) in [
            ("", ""),
            ("", "é\n"),
            ("é\n", ""),
            ("a\n", "a"),
            ("a", "a\n"),
            ("\u{feff}é\r\nkeep\r\n", "\u{feff}🙂\r\nkeep\r\n"),
            ("one\ntwo\nthree\nfour\n", "ONE\ntwo\nthree\nFOUR\n"),
        ] {
            let old = TextBuffer::from(old);
            let new = TextBuffer::from(new);
            let ranges = compare(&old, &new, &AtomicBool::new(false)).unwrap();
            assert!(valid_ranges(
                &lines(&old, &AtomicBool::new(false)).unwrap(),
                &lines(&new, &AtomicBool::new(false)).unwrap(),
                &ranges
            ));
            assert_eq!(ranges.is_empty(), old == new);
            assert!(Comparison::new(1, old).with_saved(new, ranges).is_ok());
        }
    }
    #[test]
    fn invalid_authoritative_ranges_are_rejected_and_large_saved_chunks_stay_separate() {
        assert!(
            Comparison::new(1, TextBuffer::from("old\n"))
                .with_saved(
                    TextBuffer::from("new\n"),
                    vec![ChangeRange {
                        old: 3..4,
                        new: 0..1
                    }]
                )
                .is_err()
        );
        let old = (0..2000).map(|i| format!("line {i}\n")).collect::<String>();
        let new = old
            .replacen("line 0\n", "first\n", 1)
            .replace("line 1999\n", "last\n");
        let ranges = vec![
            ChangeRange {
                old: 0..1,
                new: 0..1,
            },
            ChangeRange {
                old: 1999..2000,
                new: 1999..2000,
            },
        ];
        let input = Comparison::new(1, TextBuffer::from(old))
            .with_saved(TextBuffer::from(new.clone()), ranges.clone())
            .unwrap();
        let p = Projection::build(
            input,
            TextSnapshot {
                text: TextBuffer::from(new),
                revision: 0,
            },
            None,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(&*p.ranges, ranges);
    }
    #[test]
    fn sparse_rows_keep_history_and_controls_outside_editable_coordinates() {
        let p = projection("before\ndeleted\nkeep\ntail\n", "after\nkeep\n", None);
        let current = TextBuffer::from("after\nkeep\n");
        let folds = FoldMap::default();
        let metrics = Metrics {
            font: egui::FontId::monospace(12.0),
            cell: 8.0,
            height: 16.0,
        };
        let g = Geometry {
            text: &current,
            metrics: &metrics,
            wrap: None,
            folds: &folds,
            diff: Some(&p),
        };
        assert_eq!(g.rows(), 8); // three document rows, three deleted rows, two controls
        for line in 0..current.line_count() {
            let byte = current.line_start(line);
            assert!(matches!(p.row(g.visual_row(byte)),Row::Document(row) if row==line));
            assert_eq!(g.segment(g.visual_row(byte)).bytes.start, byte);
        }
        let byte = current.line_start(1);
        assert_eq!(g.vertical(0, 0, 1, false).0, byte);
        assert_eq!(g.vertical(byte, 0, -1, false).0, 0);
        assert_eq!(g.caret(byte).top(), 6.0 + g.visual_row(byte) as f32 * 16.0);
    }
    #[test]
    fn wrapping_projects_both_sides_and_historical_clicks_resolve_to_working_text() {
        let old = "very long historical text that wraps\nkeep\n";
        let new = "very long replacement text that wraps\nkeep\n";
        let p = projection(old, new, Some(10));
        assert!(p.insertions[0].historical.len() > 1);
        let snapshot = TextSnapshot {
            text: TextBuffer::from(new),
            revision: 7,
        };
        let wrap = WrapIndex::build(snapshot.clone(), 10, &AtomicBool::new(false)).unwrap();
        let metrics = Metrics {
            font: egui::FontId::monospace(12.0),
            cell: 8.0,
            height: 16.0,
        };
        let folds = FoldMap::default();
        let g = Geometry {
            text: &snapshot.text,
            metrics: &metrics,
            wrap: Some(&wrap),
            folds: &folds,
            diff: Some(&p),
        };
        assert!(g.rows() > wrap.rows());
        for row in 0..g.rows() {
            let segment = g.segment(row);
            assert!(segment.bytes.end <= snapshot.text.len());
        }
        assert_eq!(g.segment(1).bytes.start, 0);
    }
    fn ready(
        ctx: &egui::Context,
        view: &mut EditorView,
        doc: &mut MemoryTextSession,
        id: egui::Id,
    ) -> EditorOutput {
        for _ in 0..200 {
            let output = frame(ctx, view, doc, id, vec![]);
            if !output.diff_controls.is_empty() {
                return output;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        panic!("Comparison did not finish");
    }
    #[test]
    fn inline_button_returns_observed_identity_and_editing_keeps_shared_undo_clean() {
        let ctx = egui::Context::default();
        let id = egui::Id::new("diff-input");
        let mut doc = MemoryTextSession::new("new\nkeep\n");
        let mut view = EditorView::default();
        view.set_comparison(
            Some(Comparison::new(42, TextBuffer::from("old\nkeep\n"))),
            vec![DiffAction {
                id: 3,
                label: "Stage".into(),
                enabled: true,
            }],
        );
        let output = ready(&ctx, &mut view, &mut doc, id);
        let pos = output.diff_controls[0].2.center();
        frame(
            &ctx,
            &mut view,
            &mut doc,
            id,
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        let clicked = frame(
            &ctx,
            &mut view,
            &mut doc,
            id,
            vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        let action = clicked.diff_request.expect("inline button click");
        assert_eq!(
            (action.action, action.comparison, action.revision),
            (3, 42, 0)
        );
        assert_eq!(
            action.range,
            ChangeRange {
                old: 0..1,
                new: 0..1
            }
        );
        assert_eq!(doc.text(), "new\nkeep\n");
        doc.edit(
            0,
            TextEdit {
                range: 0..3,
                replacement: "edited".into(),
            },
        )
        .unwrap();
        let next = frame(&ctx, &mut view, &mut doc, id, vec![]);
        assert!(next.diff_controls.is_empty());
        ready(&ctx, &mut view, &mut doc, id);
        doc.history(HistoryAction::Undo).unwrap();
        assert_eq!(doc.text(), "new\nkeep\n");
    }
    fn pointer(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }
    }
    #[test]
    fn old_lines_select_within_their_change_and_copy_without_touching_the_document() {
        let ctx = egui::Context::default();
        let id = egui::Id::new("diff-old-selection");
        let mut doc = MemoryTextSession::new("new\nkeep\n");
        let mut view = EditorView::default();
        view.set_comparison(
            Some(Comparison::new(
                5,
                TextBuffer::from("old words\nsecond gone\nkeep\n"),
            )),
            vec![DiffAction {
                id: 1,
                label: "Stage".into(),
                enabled: true,
            }],
        );
        let output = ready(&ctx, &mut view, &mut doc, id);
        // Rows: the buttons, two old lines, then the document (caret at 0).
        let caret = output.caret_rect;
        let (height, left) = (caret.height(), caret.left());
        let first = egui::pos2(left + 2.0, caret.top() - 1.5 * height);
        let below = egui::pos2(left + 400.0, caret.top() + 3.0 * height);
        let mut send = |view: &mut EditorView, doc: &mut MemoryTextSession, events| {
            frame(&ctx, view, doc, id, events)
        };
        // A press and a drag far past the old lines: all of them, no further.
        send(
            &mut view,
            &mut doc,
            vec![egui::Event::PointerMoved(first), pointer(first, true)],
        );
        send(&mut view, &mut doc, vec![egui::Event::PointerMoved(below)]);
        send(&mut view, &mut doc, vec![egui::Event::PointerMoved(below)]);
        assert_eq!(
            view.diff.selected_text(0).as_deref(),
            Some("old words\nsecond gone")
        );
        assert!(view.old_lines_selected());
        assert!(view.selection().range().is_empty());
        send(&mut view, &mut doc, vec![pointer(below, false)]);
        // Copy takes them (not the caret's line), Cut cuts nothing.
        send(
            &mut view,
            &mut doc,
            vec![egui::Event::Copy, egui::Event::Cut],
        );
        assert_eq!(doc.text(), "new\nkeep\n");
        assert!(view.old_lines_selected());
        // A double click selects a word.
        for _ in 0..2 {
            send(
                &mut view,
                &mut doc,
                vec![egui::Event::PointerMoved(first), pointer(first, true)],
            );
            send(&mut view, &mut doc, vec![pointer(first, false)]);
        }
        assert_eq!(view.diff.selected_text(0).as_deref(), Some("old"));
        // Moving the caret, or clicking in the document, drops it.
        send(
            &mut view,
            &mut doc,
            vec![crate::editor::tests::key(
                egui::Key::ArrowRight,
                egui::Modifiers::NONE,
            )],
        );
        assert!(!view.old_lines_selected());
        send(
            &mut view,
            &mut doc,
            vec![egui::Event::PointerMoved(first), pointer(first, true)],
        );
        send(&mut view, &mut doc, vec![pointer(first, false)]);
        assert!(view.diff.selection.is_some());
        let document = egui::pos2(left + 2.0, caret.top() + 0.5 * height);
        send(
            &mut view,
            &mut doc,
            vec![egui::Event::PointerMoved(document), pointer(document, true)],
        );
        assert!(view.diff.selection.is_none());
    }
    #[test]
    fn changing_comparison_rejects_old_worker_results() {
        let ctx = egui::Context::default();
        let id = egui::Id::new("diff-stale");
        let mut doc = MemoryTextSession::new("working\n");
        let mut view = EditorView::default();
        let actions = vec![DiffAction {
            id: 1,
            label: "Stage".into(),
            enabled: true,
        }];
        view.set_comparison(
            Some(Comparison::new(1, TextBuffer::from("old\n"))),
            actions.clone(),
        );
        frame(&ctx, &mut view, &mut doc, id, vec![]);
        view.set_comparison(
            Some(Comparison::new(2, TextBuffer::from("other\n"))),
            actions,
        );
        ready(&ctx, &mut view, &mut doc, id);
        assert_eq!(view.diff.projection.as_ref().unwrap().comparison, 2);
    }
}
