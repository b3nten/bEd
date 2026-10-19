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

    pub fn caret(&self, text: &TextBuffer, byte: usize) -> Rect {
        Rect::from_min_size(
            egui::pos2(
                8.0 + text.column(byte) as f32 * self.cell,
                6.0 + text.line_of(byte) as f32 * self.height,
            ),
            egui::vec2(0.0, self.height),
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
}
impl Geometry<'_> {
    pub fn rows(&self) -> usize {
        self.wrap
            .map_or_else(|| self.text.line_count(), |w| w.rows())
    }
    pub fn size(&self, viewport: Vec2) -> Vec2 {
        if self.wrap.is_some() {
            egui::vec2(viewport.x, self.rows() as f32 * self.metrics.height + 12.0).max(viewport)
        } else {
            self.metrics.content_size(self.text).max(viewport)
        }
    }
    pub fn visual_row(&self, byte: usize) -> usize {
        self.wrap
            .map_or_else(|| self.text.line_of(byte), |w| w.visual_row(byte))
    }
    pub fn segment(&self, row: usize) -> crate::wrapping::Segment {
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
        if self.wrap.is_some() {
            let segment = self.segment(row);
            VisibleLine::fragment(
                ui,
                self.text,
                self.metrics,
                segment.bytes,
                (segment.indent as f32 - segment.columns.start as f32) * self.metrics.cell,
            )
        } else {
            VisibleLine::new(ui, self.text, self.metrics, row, cells)
        }
    }
    pub fn visual_row_for(&self, byte: usize, upstream: bool) -> usize {
        let row = self.visual_row(byte);
        if upstream && self.wrap.is_some() && row > 0 && self.segment(row).bytes.start == byte {
            row - 1
        } else {
            row
        }
    }
    pub fn caret(&self, byte: usize) -> Rect {
        self.caret_for(byte, false)
    }
    pub fn caret_for(&self, byte: usize, upstream: bool) -> Rect {
        if self.wrap.is_none() {
            return self.metrics.caret(self.text, byte);
        }
        let row = self.visual_row_for(byte, upstream);
        let segment = self.segment(row);
        Rect::from_min_size(
            egui::pos2(
                8.0 + (self.text.column(byte).saturating_sub(segment.columns.start)
                    + segment.indent) as f32
                    * self.metrics.cell,
                6.0 + row as f32 * self.metrics.height,
            ),
            egui::vec2(0.0, self.metrics.height),
        )
    }
    pub fn hit(&self, ui: &egui::Ui, position: egui::Pos2) -> usize {
        let row = (((position.y - 6.0) / self.metrics.height).floor().max(0.0) as usize)
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
        if let Some(wrap) = self.wrap {
            let row = self
                .visual_row_for(byte, upstream)
                .saturating_add_signed(step)
                .min(wrap.rows() - 1);
            let segment = wrap.segment(row);
            let at = self
                .text
                .byte_at_column(
                    segment.line,
                    segment.columns.start + column.saturating_sub(segment.indent),
                )
                .clamp(segment.bytes.start, segment.bytes.end);
            (at, at == segment.bytes.end)
        } else {
            let row = self
                .text
                .line_of(byte)
                .saturating_add_signed(step)
                .min(self.text.line_count() - 1);
            (self.text.byte_at_column(row, column), false)
        }
    }
    pub fn column_for(&self, byte: usize, upstream: bool) -> usize {
        if let Some(wrap) = self.wrap {
            let segment = wrap.segment(self.visual_row_for(byte, upstream));
            self.text.column(byte).saturating_sub(segment.columns.start) + segment.indent
        } else {
            self.text.column(byte)
        }
    }
}
