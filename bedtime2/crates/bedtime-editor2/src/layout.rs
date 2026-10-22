//! One coordinate mapping shared by text, hit testing, selections and carets.
use crate::TextBuffer;
use bedtime_ui2::egui::{
    self, FontId, Rect, Vec2,
    text::{CCursor, LayoutJob},
};
use std::{ops::Range, sync::Arc};
use unicode_segmentation::UnicodeSegmentation;

const MAX_LINE_LAYOUT_BYTES: usize = 32 * 1024;

pub(crate) struct Metrics {
    pub font: FontId,
    pub cell: f32,
    pub height: f32,
}

impl Metrics {
    pub fn new(ui: &egui::Ui) -> Self {
        let font = egui::TextStyle::Monospace.resolve(ui.style());
        let (cell, height) =
            ui.fonts_mut(|fonts| (fonts.glyph_width(&font, ' '), fonts.row_height(&font)));
        Self {
            font,
            cell: cell.max(1.0),
            height: height.max(1.0),
        }
    }

    pub fn content_size(&self, text: &TextBuffer) -> Vec2 {
        egui::vec2(
            text.widest_line_cells() as f32 * self.cell + 16.0,
            text.line_count() as f32 * self.height + 12.0,
        )
    }
}

pub(crate) struct VisibleLine {
    pub galley: Arc<egui::Galley>,
    pub source_bytes: usize,
    left: f32,
    pub start: usize,
    pub end: usize,
    carets: Vec<(usize, usize)>,
}

impl VisibleLine {
    pub fn new(
        ui: &egui::Ui,
        text: &TextBuffer,
        metrics: &Metrics,
        row: usize,
        cells: Range<usize>,
    ) -> Self {
        let start = text.byte_at_column(row, cells.start.saturating_sub(2));
        let end = text.byte_at_column(row, cells.end);
        let end = text.clamp_boundary(end.min(start.saturating_add(MAX_LINE_LAYOUT_BYTES)));
        Self::fragment(ui, text, metrics, start..end, 0.0)
    }

    pub fn fragment(
        ui: &egui::Ui,
        text: &TextBuffer,
        metrics: &Metrics,
        range: Range<usize>,
        offset: f32,
    ) -> Self {
        let start = range.start;
        let end = text.clamp_boundary(range.end.min(start.saturating_add(MAX_LINE_LAYOUT_BYTES)));
        let source = text.range_text(start..end);
        let start_column = text.column(start);
        let mut display = String::new();
        let mut carets = vec![(start, 0)];
        let mut column = start_column;
        let mut char_index = 0;
        for (byte, grapheme) in source.grapheme_indices(true) {
            if grapheme == "\t" {
                let width = 4 - column % 4;
                display.extend(std::iter::repeat_n(' ', width));
                char_index += width;
                column += width;
            } else {
                display.push_str(grapheme);
                char_index += grapheme.chars().count();
                column += unicode_width::UnicodeWidthStr::width(grapheme);
            }
            carets.push((start + byte + grapheme.len(), char_index));
        }
        let galley = ui.fonts_mut(|fonts| {
            fonts.layout_job(LayoutJob::simple(
                display,
                metrics.font.clone(),
                ui.visuals().text_color(),
                f32::INFINITY,
            ))
        });
        Self {
            galley,
            source_bytes: source.len(),
            left: start_column as f32 * metrics.cell + offset,
            start,
            end,
            carets,
        }
    }

    pub fn left(&self) -> f32 {
        self.left
    }

    pub fn right(&self) -> f32 {
        self.left + self.galley.size().x
    }

    pub fn x_at(&self, byte: usize) -> f32 {
        if byte <= self.start {
            return self.left;
        }
        if byte >= self.end {
            return self.right();
        }
        let index = self
            .carets
            .partition_point(|(at, _)| *at < byte)
            .min(self.carets.len() - 1);
        self.left
            + self
                .galley
                .pos_from_cursor(CCursor::new(self.carets[index].1))
                .min
                .x
    }

    pub fn hit(&self, x: f32) -> usize {
        self.carets
            .iter()
            .min_by(|(_, a), (_, b)| {
                let ax =
                    (self.left + self.galley.pos_from_cursor(CCursor::new(*a)).min.x - x).abs();
                let bx =
                    (self.left + self.galley.pos_from_cursor(CCursor::new(*b)).min.x - x).abs();
                ax.total_cmp(&bx)
            })
            .unwrap()
            .0
    }
}

/// One mapping for scrolling, painting, pointer hits and keyboard movement.
/// A pending wrap build uses the ordinary unwrapped mapping consistently.
pub(crate) struct Geometry<'a> {
    pub text: &'a TextBuffer,
    pub metrics: &'a Metrics,
    pub wrap: Option<&'a crate::wrapping::WrapIndex>,
    pub folds: &'a crate::folding::FoldMap,
}
impl Geometry<'_> {
    pub fn rows(&self) -> usize {
        self.folds.rows(
            self.wrap
                .map_or_else(|| self.text.line_count(), |w| w.rows()),
        )
    }
    pub fn size(&self, viewport: Vec2) -> Vec2 {
        let width = if self.wrap.is_some() {
            viewport.x
        } else {
            self.metrics.content_size(self.text).x
        };
        egui::vec2(
            width,
            self.folds.height(
                self.wrap
                    .map_or_else(|| self.text.line_count(), |w| w.rows()),
            ) * self.metrics.height
                + 12.0,
        )
        .max(viewport)
    }
    pub fn visual_row(&self, byte: usize) -> usize {
        self.folds.visible_row(
            self.wrap
                .map_or_else(|| self.text.line_of(byte), |w| w.visual_row(byte)),
        )
    }
    pub fn segment(&self, row: usize) -> crate::wrapping::Segment {
        let row = self.folds.source_row(row);
        self.wrap.map_or_else(
            || crate::wrapping::Segment {
                line: row,
                part: 0,
                bytes: self.text.line_range(row),
                columns: 0..self.text.column(self.text.line_range(row).end),
                indent: 0,
            },
            |w| w.segment(row),
        )
    }
    pub fn line(&self, ui: &egui::Ui, row: usize, cells: Range<usize>) -> VisibleLine {
        let segment = self.segment(row);
        if self.wrap.is_some() {
            VisibleLine::fragment(
                ui,
                self.text,
                self.metrics,
                segment.bytes,
                (segment.indent as f32 - segment.columns.start as f32) * self.metrics.cell,
            )
        } else {
            VisibleLine::new(ui, self.text, self.metrics, segment.line, cells)
        }
    }
    pub fn visual_row_for(&self, byte: usize, upstream: bool) -> usize {
        let row = self.visual_row(byte);
        if upstream
            && self.wrap.is_some()
            && row > 0
            && self.segment(row).bytes.start == byte
            && self.segment(row).part > 0
        {
            row - 1
        } else {
            row
        }
    }
    pub fn caret(&self, byte: usize) -> Rect {
        self.caret_for(byte, false)
    }
    pub fn caret_for(&self, byte: usize, upstream: bool) -> Rect {
        let row = self.visual_row_for(byte, upstream);
        let segment = self.segment(row);
        Rect::from_min_size(
            egui::pos2(
                8.0 + (self.text.column(byte).saturating_sub(segment.columns.start)
                    + segment.indent) as f32
                    * self.metrics.cell,
                6.0 + self.folds.row_y(row) * self.metrics.height,
            ),
            egui::vec2(0.0, self.metrics.height),
        )
    }
    pub fn hit(&self, ui: &egui::Ui, position: egui::Pos2) -> usize {
        let row = self
            .folds
            .row_at((position.y - 6.0) / self.metrics.height)
            .min(self.rows() - 1);
        let column = ((position.x - 8.0) / self.metrics.cell).floor().max(0.0) as usize;
        self.line(ui, row, column.saturating_sub(8)..column.saturating_add(16))
            .hit(position.x - 8.0)
    }
    pub fn vertical(
        &self,
        byte: usize,
        column: usize,
        step: isize,
        upstream: bool,
    ) -> (usize, bool) {
        let row = self
            .visual_row_for(byte, upstream)
            .saturating_add_signed(step)
            .min(self.rows() - 1);
        let segment = self.segment(row);
        let at = self
            .text
            .byte_at_column(
                segment.line,
                segment.columns.start + column.saturating_sub(segment.indent),
            )
            .clamp(segment.bytes.start, segment.bytes.end);
        (at, self.wrap.is_some() && at == segment.bytes.end)
    }
    pub fn column_for(&self, byte: usize, upstream: bool) -> usize {
        let segment = self.segment(self.visual_row_for(byte, upstream));
        self.text.column(byte).saturating_sub(segment.columns.start) + segment.indent
    }
}

#[cfg(feature = "treesitter")]
impl VisibleLine {
    pub fn apply_syntax(
        &mut self,
        ui: &egui::Ui,
        metrics: &Metrics,
        observation: &bedtime_treesitter2::Observation,
        changes: &[crate::TextDelta],
        palette: &crate::syntax::SyntaxPalette,
    ) {
        let spans = crate::syntax::mapped_highlights(observation, changes, self.start..self.end);
        if spans.is_empty() {
            return;
        }
        let mut job = (*self.galley.job).clone();
        let offsets: Vec<usize> = job
            .text
            .char_indices()
            .map(|(i, _)| i)
            .chain(std::iter::once(job.text.len()))
            .collect();
        let display_byte = |source| {
            let index = self
                .carets
                .partition_point(|(at, _)| *at < source)
                .min(self.carets.len() - 1);
            offsets[self.carets[index].1]
        };
        job.sections.clear();
        let normal = ui.visuals().text_color();
        let mut start = 0;
        for span in &spans {
            let from = display_byte(span.range.start.max(self.start));
            let to = display_byte(span.range.end.min(self.end));
            if from > start {
                job.sections.push(egui::text::LayoutSection {
                    leading_space: 0.0,
                    byte_range: egui::text::ByteIndex(start)..egui::text::ByteIndex(from),
                    format: egui::TextFormat::simple(metrics.font.clone(), normal),
                });
            }
            if to > from {
                job.sections.push(egui::text::LayoutSection {
                    leading_space: 0.0,
                    byte_range: egui::text::ByteIndex(from)..egui::text::ByteIndex(to),
                    format: egui::TextFormat::simple(
                        metrics.font.clone(),
                        palette.color(&span.capture, normal),
                    ),
                });
            }
            start = to;
        }
        if start < job.text.len() {
            job.sections.push(egui::text::LayoutSection {
                leading_space: 0.0,
                byte_range: egui::text::ByteIndex(start)..egui::text::ByteIndex(job.text.len()),
                format: egui::TextFormat::simple(metrics.font.clone(), normal),
            });
        }
        self.galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
    }
}
