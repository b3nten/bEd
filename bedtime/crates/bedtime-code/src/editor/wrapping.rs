//! Cell-based soft rows with hanging indentation. Metadata is built off-thread;
//! small edits replace only affected blocks, never a million-entry vector.
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

#[derive(Clone, Copy)]
struct Line {
    cells: usize,
    indent: usize,
}
impl Line {
    fn indent(self, width: usize) -> usize {
        self.indent.min(width / 2)
    }
    fn rows(self, width: usize) -> usize {
        1 + self
            .cells
            .saturating_sub(width)
            .div_ceil(width - self.indent(width))
    }
}
struct Block {
    lines: Vec<Line>,
    rows: usize,
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
        let mut blocks = Vec::new();
        let mut lines = Vec::with_capacity(BLOCK);
        let mut line = Line {
            cells: 0,
            indent: 0,
        };
        let mut leading = true;
        let mut cr = false;
        let push = |line, lines: &mut Vec<Line>, blocks: &mut Vec<Arc<Block>>| {
            lines.push(line);
            if lines.len() == BLOCK {
                let rows = lines.iter().map(|l| l.rows(width)).sum();
                blocks.push(Arc::new(Block {
                    lines: std::mem::take(lines),
                    rows,
                }));
            }
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
                    push(line, &mut lines, &mut blocks);
                    line = Line {
                        cells: 0,
                        indent: 0,
                    };
                    leading = true;
                } else {
                    line.cells += if ch == '\t' {
                        4 - line.cells % 4
                    } else {
                        unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0)
                    };
                    leading &= matches!(ch, ' ' | '\t');
                    if leading {
                        line.indent = line.cells;
                    }
                }
            }
        }
        push(line, &mut lines, &mut blocks);
        if !lines.is_empty() {
            let rows = lines.iter().map(|l| l.rows(width)).sum();
            blocks.push(Arc::new(Block { lines, rows }));
        }
        let mut result = Self {
            snapshot,
            width,
            blocks,
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
    fn line(&self, row: usize) -> (Line, usize) {
        let b = self.ends.partition_point(|(end, _)| *end <= row);
        let (base, visual) = if b == 0 { (0, 0) } else { self.ends[b - 1] };
        let block = &self.blocks[b];
        (
            block.lines[row - base],
            visual
                + block.lines[..row - base]
                    .iter()
                    .map(|l| l.rows(self.width))
                    .sum::<usize>(),
        )
    }
    pub fn visual_row(&self, byte: usize) -> usize {
        let row = self.snapshot.text.line_of(byte);
        let (line, base) = self.line(row);
        let column = self.snapshot.text.column(byte);
        let part = if column < self.width {
            0
        } else {
            (1 + (column - self.width) / (self.width - line.indent(self.width)))
                .min(line.rows(self.width) - 1)
        };
        let mut visual = base + part;
        // A cell boundary can fall inside a grapheme. Its complete source
        // bytes belong to the following row; caret and hit testing agree.
        while visual + 1 < base + line.rows(self.width)
            && byte >= self.segment(visual + 1).bytes.start
        {
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
        let mut remaining = visual - first;
        let mut index = 0;
        while remaining >= self.blocks[b].lines[index].rows(self.width) {
            remaining -= self.blocks[b].lines[index].rows(self.width);
            index += 1;
        }
        let line = self.blocks[b].lines[index];
        let indent = if remaining == 0 {
            0
        } else {
            line.indent(self.width)
        };
        let start = if remaining == 0 {
            0
        } else {
            self.width + (remaining - 1) * (self.width - line.indent(self.width))
        };
        let end = if remaining + 1 == line.rows(self.width) {
            line.cells
        } else {
            start + self.width - indent
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
        let bytes = boundary(start)..if remaining + 1 == line.rows(self.width) {
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
        let mut replacement = Vec::new();
        for row in first..new_last {
            let range = after.text.line_range(row);
            let cells = after.text.column(range.end);
            // Only indentation visible at this width matters. No scan of a
            // multi-megabyte whitespace-only prefix is needed after typing.
            let limit = after.text.byte_at_column(row, self.width / 2);
            let indent_end = crate::editor::whitespace::leading_end(&after.text, row, limit);
            replacement.push(Line {
                cells,
                indent: after.text.column(indent_end).min(self.width / 2),
            });
        }
        let left = self.ends.partition_point(|(end, _)| *end <= first);
        let right = self.ends.partition_point(|(end, _)| *end < old_last);
        let left_base = if left == 0 { 0 } else { self.ends[left - 1].0 };
        let right_base = if right == 0 {
            0
        } else {
            self.ends[right - 1].0
        };
        let mut lines = self.blocks[left].lines[..first - left_base].to_vec();
        lines.extend(replacement);
        lines.extend_from_slice(&self.blocks[right].lines[old_last - right_base..]);
        let blocks = lines
            .chunks(BLOCK)
            .map(|chunk| {
                Arc::new(Block {
                    lines: chunk.to_vec(),
                    rows: chunk.iter().map(|l| l.rows(self.width)).sum(),
                })
            })
            .collect::<Vec<_>>();
        self.blocks.splice(left..=right, blocks);
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
