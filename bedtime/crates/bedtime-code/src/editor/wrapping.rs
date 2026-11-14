//! Cell-based soft rows with hanging indentation, broken between words.
//! Metadata is built off-thread; small edits replace only affected blocks,
//! never a million-entry vector.
use crate::editor::{TextDelta, TextSnapshot, background::Job};
use bedtime_ui::egui;
use std::{
    ops::Range,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
const BLOCK: usize = 256;
/// Longer lines (minified files, data dumps) wrap at fixed cell counts, so
/// typing in them never rescans the whole line.
const LONG: usize = 16 * 1024;

#[derive(Clone, Copy)]
struct Line {
    cells: usize,
    /// Hanging indent of continuation rows, at most half the width.
    indent: usize,
    rows: usize,
    /// Offset of this line's row starts in its block's `breaks`; `None`
    /// wraps every `width - indent` cells.
    breaks: Option<usize>,
}
impl Line {
    fn uniform(cells: usize, indent: usize, width: usize) -> Self {
        let indent = indent.min(width / 2);
        Self {
            cells,
            indent,
            rows: 1 + cells.saturating_sub(width).div_ceil(width - indent),
            breaks: None,
        }
    }
    /// The column where row `part` (> 0) starts.
    fn row_start(self, block: &Block, part: usize, width: usize) -> usize {
        match self.breaks {
            Some(first) => block.breaks[first + part - 1],
            None => width + (part - 1) * (width - self.indent),
        }
    }
}

/// Chooses row starts while streaming one logical line. Rows break after
/// whitespace or beside wide characters; a single space may hang one cell
/// past the edge, so a row that fits exactly doesn't push the space onto the
/// next row. A word wider than a row breaks at the cell that overflows.
struct Breaker {
    width: usize,
    cells: usize,
    indent: usize,
    leading: bool,
    row: usize,
    opportunity: usize,
    after_space: bool,
    after_wide: bool,
}
impl Breaker {
    fn new(width: usize) -> Self {
        Self {
            width,
            cells: 0,
            indent: 0,
            leading: true,
            row: 0,
            opportunity: 0,
            after_space: false,
            after_wide: false,
        }
    }
    fn push(&mut self, ch: char, breaks: &mut Vec<usize>) {
        let column = self.cells;
        let cells = if ch == '\t' {
            4 - column % 4
        } else {
            unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0)
        };
        let space = matches!(ch, ' ' | '\t');
        let wide = cells > 1;
        if cells > 0
            && column > self.row
            && ((self.after_space && !space && !self.leading) || wide || self.after_wide)
        {
            self.opportunity = column;
        }
        self.cells += cells;
        self.leading &= space;
        if self.leading {
            self.indent = self.cells.min(self.width / 2);
        }
        if cells > 0 {
            self.after_space = space;
            self.after_wide = wide;
        }
        let hang = usize::from(ch == ' ');
        while column > self.row && self.cells > self.row + self.capacity() + hang {
            self.row = if !space && self.opportunity > self.row {
                self.opportunity
            } else {
                column
            };
            breaks.push(self.row);
        }
    }
    fn capacity(&self) -> usize {
        if self.row == 0 {
            self.width
        } else {
            self.width - self.indent
        }
    }
}

struct Block {
    lines: Vec<Line>,
    breaks: Vec<usize>,
    rows: usize,
}
/// Accumulates lines (and their row starts) into blocks.
#[derive(Default)]
struct Blocks {
    lines: Vec<Line>,
    breaks: Vec<usize>,
    blocks: Vec<Arc<Block>>,
}
impl Blocks {
    fn push(&mut self, line: Line) {
        self.lines.push(line);
        if self.lines.len() == BLOCK {
            self.flush();
        }
    }
    fn copy(&mut self, mut line: Line, from: &Block) {
        if let Some(first) = line.breaks {
            line.breaks = Some(self.breaks.len());
            self.breaks
                .extend_from_slice(&from.breaks[first..first + line.rows - 1]);
        }
        self.push(line);
    }
    fn scan(&mut self, text: &str, width: usize) {
        let first = self.breaks.len();
        let mut breaker = Breaker::new(width);
        for ch in text.chars() {
            breaker.push(ch, &mut self.breaks);
        }
        self.push(Line {
            cells: breaker.cells,
            indent: breaker.indent,
            rows: 1 + self.breaks.len() - first,
            breaks: Some(first),
        });
    }
    fn flush(&mut self) {
        if !self.lines.is_empty() {
            let rows = self.lines.iter().map(|l| l.rows).sum();
            self.blocks.push(Arc::new(Block {
                lines: std::mem::take(&mut self.lines),
                breaks: std::mem::take(&mut self.breaks),
                rows,
            }));
        }
    }
}
#[derive(Clone)]
pub(crate) struct WrapIndex {
    pub snapshot: TextSnapshot,
    pub width: usize,
    blocks: Vec<Arc<Block>>,
    // Exclusive block ends in logical and visual rows.
    ends: Vec<(usize, usize)>,
}
#[derive(Clone, Debug)]
pub(crate) struct Segment {
    pub line: usize,
    pub part: usize,
    pub bytes: Range<usize>,
    pub columns: Range<usize>,
    pub indent: usize,
}
impl WrapIndex {
    pub fn build(snapshot: TextSnapshot, width: usize, cancel: &AtomicBool) -> Option<Self> {
        let mut out = Blocks::default();
        let mut first = 0;
        let mut breaker = Breaker::new(width);
        let mut bytes = 0;
        let mut cr = false;
        let mut scratch = Vec::new();
        let finish = |out: &mut Blocks, breaker: &Breaker, first: usize, bytes: usize| {
            out.push(if bytes > LONG {
                Line::uniform(breaker.cells, breaker.indent, width)
            } else {
                Line {
                    cells: breaker.cells,
                    indent: breaker.indent,
                    rows: 1 + out.breaks.len() - first,
                    breaks: Some(first),
                }
            });
        };
        for chunk in snapshot.text.chunks() {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            for ch in chunk.chars() {
                if ch == '\n' && cr {
                    cr = false;
                    continue;
                }
                cr = ch == '\r';
                if matches!(ch, '\n' | '\r') {
                    finish(&mut out, &breaker, first, bytes);
                    first = out.breaks.len();
                    breaker = Breaker::new(width);
                    bytes = 0;
                } else {
                    bytes += ch.len_utf8();
                    if bytes > LONG {
                        // Keep counting cells and indentation only.
                        out.breaks.truncate(first);
                        scratch.clear();
                        breaker.push(ch, &mut scratch);
                    } else {
                        breaker.push(ch, &mut out.breaks);
                    }
                }
            }
        }
        finish(&mut out, &breaker, first, bytes);
        out.flush();
        let mut result = Self {
            snapshot,
            width,
            blocks: out.blocks,
            ends: Vec::new(),
        };
        result.reindex();
        Some(result)
    }
    fn reindex(&mut self) {
        self.ends.clear();
        let (mut lines, mut rows) = (0, 0);
        for block in &self.blocks {
            lines += block.lines.len();
            rows += block.rows;
            self.ends.push((lines, rows));
        }
    }
    pub fn rows(&self) -> usize {
        self.ends.last().unwrap().1
    }
    fn line(&self, row: usize) -> (Line, &Block, usize) {
        let b = self.ends.partition_point(|(end, _)| *end <= row);
        let (base, visual) = if b == 0 { (0, 0) } else { self.ends[b - 1] };
        let block = &self.blocks[b];
        (
            block.lines[row - base],
            block,
            visual
                + block.lines[..row - base]
                    .iter()
                    .map(|l| l.rows)
                    .sum::<usize>(),
        )
    }
    pub fn visual_row(&self, byte: usize) -> usize {
        let row = self.snapshot.text.line_of(byte);
        let (line, block, base) = self.line(row);
        let column = self.snapshot.text.column(byte);
        let part = match line.breaks {
            Some(first) => block.breaks[first..first + line.rows - 1]
                .partition_point(|start| *start <= column),
            None if column < self.width => 0,
            None => (1 + (column - self.width) / (self.width - line.indent)).min(line.rows - 1),
        };
        let mut visual = base + part;
        // A cell boundary can fall inside a grapheme. Its complete source
        // bytes belong to the following row; caret and hit testing agree.
        while visual + 1 < base + line.rows && byte >= self.segment(visual + 1).bytes.start {
            visual += 1;
        }
        while visual > base && byte < self.segment(visual).bytes.start {
            visual -= 1;
        }
        visual
    }
    pub fn segment(&self, visual: usize) -> Segment {
        let visual = visual.min(self.rows() - 1);
        let b = self.ends.partition_point(|(_, end)| *end <= visual);
        let (base, first) = if b == 0 { (0, 0) } else { self.ends[b - 1] };
        let block = &self.blocks[b];
        let mut remaining = visual - first;
        let mut index = 0;
        while remaining >= block.lines[index].rows {
            remaining -= block.lines[index].rows;
            index += 1;
        }
        let line = block.lines[index];
        let last = remaining + 1 == line.rows;
        let indent = if remaining == 0 { 0 } else { line.indent };
        let start = if remaining == 0 {
            0
        } else {
            line.row_start(block, remaining, self.width)
        };
        let end = if last {
            line.cells
        } else {
            line.row_start(block, remaining + 1, self.width)
        };
        let boundary = |column| {
            let at = self.snapshot.text.byte_at_column(base + index, column);
            if at == self.snapshot.text.line_range(base + index).end {
                at
            } else {
                self.snapshot
                    .text
                    .previous_grapheme(self.snapshot.text.next_grapheme(at))
            }
        };
        let bytes = boundary(start)..if last {
            self.snapshot.text.line_range(base + index).end
        } else {
            boundary(end)
        };
        Segment {
            line: base + index,
            part: remaining,
            bytes,
            columns: start..end,
            indent,
        }
    }
    /// Journal bounds refer to successive states. Everything before the minimum
    /// start and after the mapped maximum end is unchanged; rescan their adjacent
    /// logical lines to cover newline/CRLF joins. Only small changes run here.
    pub fn update(&mut self, after: TextSnapshot, changes: &[TextDelta]) -> bool {
        if changes.is_empty() {
            self.snapshot = after;
            return true;
        }
        let mut start = usize::MAX;
        let mut end = 0;
        for change in changes {
            start = start.min(change.range.start);
            end = change
                .map_position(end)
                .max(change.range.start + change.inserted_bytes);
        }
        let old_end =
            (end as i128 + self.snapshot.text.len() as i128 - after.text.len() as i128) as usize;
        let first = self.snapshot.text.line_of(start.saturating_sub(1));
        let old_last = (self
            .snapshot
            .text
            .line_of((old_end + 1).min(self.snapshot.text.len()))
            + 1)
        .min(self.snapshot.text.line_count());
        let new_last =
            (after.text.line_of((end + 1).min(after.text.len())) + 1).min(after.text.line_count());
        if new_last - first > 4096 {
            return false;
        }
        let left = self.ends.partition_point(|(end, _)| *end <= first);
        let right = self.ends.partition_point(|(end, _)| *end < old_last);
        let left_base = if left == 0 { 0 } else { self.ends[left - 1].0 };
        let right_base = if right == 0 {
            0
        } else {
            self.ends[right - 1].0
        };
        let mut out = Blocks::default();
        let before = &self.blocks[left];
        for line in &before.lines[..first - left_base] {
            out.copy(*line, before);
        }
        for row in first..new_last {
            let range = after.text.line_range(row);
            if range.len() > LONG {
                let cells = after.text.column(range.end);
                // Only indentation visible at this width matters. No scan of
                // a multi-megabyte whitespace-only prefix is needed after typing.
                let limit = after.text.byte_at_column(row, self.width / 2);
                let indent_end = crate::editor::whitespace::leading_end(&after.text, row, limit);
                out.push(Line::uniform(
                    cells,
                    after.text.column(indent_end),
                    self.width,
                ));
            } else {
                out.scan(&after.text.range_text(range), self.width);
            }
        }
        let behind = &self.blocks[right];
        for line in &behind.lines[old_last - right_base..] {
            out.copy(*line, behind);
        }
        out.flush();
        self.blocks.splice(left..=right, out.blocks);
        self.snapshot = after;
        self.reindex();
        true
    }
}

#[derive(Default)]
pub(crate) struct Wrapping {
    pub index: Option<WrapIndex>,
    job: Option<Job<WrapIndex>>,
    requested: Option<(u64, usize)>,
}
impl Wrapping {
    pub fn prepare(
        &mut self,
        ctx: &egui::Context,
        snapshot: &TextSnapshot,
        width: Option<usize>,
        changes: Option<&[TextDelta]>,
    ) -> bool {
        let Some(width) = width else {
            self.index = None;
            self.job = None;
            self.requested = None;
            return false;
        };
        let key = (snapshot.revision, width);
        if let Some(index) = &mut self.index
            && index.snapshot.revision != snapshot.revision
            && !changes.is_some_and(|changes| index.update(snapshot.clone(), changes))
        {
            self.index = None;
        }
        if let Some(job) = &self.job {
            match job.poll() {
                Ok(Some(index)) => {
                    if (index.snapshot.revision, index.width) == key {
                        self.index = Some(index);
                    }
                    self.job = None;
                }
                Err(()) => {
                    self.job = None;
                    self.requested = None;
                }
                _ => {}
            }
        }
        let ready = self
            .index
            .as_ref()
            .is_some_and(|index| (index.snapshot.revision, index.width) == key);
        if !ready && self.requested != Some(key) {
            let snapshot = snapshot.clone();
            self.job = Some(Job::start(ctx, move |cancel| {
                WrapIndex::build(snapshot, width, cancel)
            }));
            self.requested = Some(key);
        }
        ready
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(text: &str, width: usize) -> Vec<String> {
        let snapshot = TextSnapshot {
            revision: 0,
            text: text.into(),
        };
        let index = WrapIndex::build(snapshot.clone(), width, &AtomicBool::new(false)).unwrap();
        (0..index.rows())
            .map(|row| snapshot.text.range_text(index.segment(row).bytes))
            .collect()
    }

    #[test]
    fn breaks_between_words() {
        assert_eq!(
            rows("the quick brown fox jumps", 10),
            ["the quick ", "brown fox ", "jumps"]
        );
    }

    #[test]
    fn a_space_hangs_when_the_row_fits_exactly() {
        assert_eq!(rows("abc defg hij", 8), ["abc defg ", "hij"]);
    }

    #[test]
    fn words_wider_than_a_row_break_by_cells() {
        assert_eq!(rows("a abcdefghijkl", 5), ["a ", "abcde", "fghij", "kl"]);
    }

    #[test]
    fn continuation_rows_keep_the_hanging_indent() {
        let text = "    one two three four";
        let snapshot = TextSnapshot {
            revision: 0,
            text: text.into(),
        };
        let index = WrapIndex::build(snapshot, 12, &AtomicBool::new(false)).unwrap();
        assert_eq!(rows(text, 12), ["    one two ", "three ", "four"]);
        assert_eq!(index.segment(1).indent, 4);
        assert_eq!(index.visual_row(text.find("four").unwrap()), 2);
        assert_eq!(index.visual_row(text.find("three").unwrap()), 1);
    }

    #[test]
    fn wide_characters_break_between_themselves() {
        assert_eq!(rows("日本語のテキスト", 6), ["日本語", "のテキ", "スト"]);
    }

    #[test]
    fn very_long_lines_wrap_by_cells() {
        let line = "word ".repeat(LONG / 5 + 10);
        let wrapped = rows(&line, 7);
        assert_eq!(wrapped[0], "word wo");
        assert!(wrapped[..wrapped.len() - 1].iter().all(|row| row.len() == 7));
    }

    #[test]
    fn updates_match_a_rebuild() {
        let mut snapshot = TextSnapshot {
            revision: 0,
            text: "alpha beta gamma\n  delta epsilon zeta eta\n".repeat(300).into(),
        };
        let mut index = WrapIndex::build(snapshot.clone(), 11, &AtomicBool::new(false)).unwrap();
        for step in 0..60 {
            let at = snapshot.text.clamp_boundary((step * 389) % snapshot.text.len());
            let insert = if step % 4 == 0 { "\n" } else { " xyzzy" };
            let edit = bedtime_document::BufferEdit::new(at..at, insert);
            edit.apply(&mut snapshot.text).unwrap();
            snapshot.revision += 1;
            assert!(index.update(
                snapshot.clone(),
                &[TextDelta {
                    range: at..at,
                    inserted_bytes: insert.len(),
                }]
            ));
            let rebuilt =
                WrapIndex::build(snapshot.clone(), 11, &AtomicBool::new(false)).unwrap();
            assert_eq!(index.rows(), rebuilt.rows());
            for row in 0..index.rows() {
                assert_eq!(index.segment(row).bytes, rebuilt.segment(row).bytes);
            }
        }
    }
}
