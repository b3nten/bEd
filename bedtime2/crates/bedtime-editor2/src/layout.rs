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
    start: usize,
    end: usize,
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
            left: start_column as f32 * metrics.cell,
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

pub(crate) fn hit(
    ui: &egui::Ui,
    text: &TextBuffer,
    metrics: &Metrics,
    position: egui::Pos2,
) -> usize {
    let row = (((position.y - 6.0) / metrics.height).floor().max(0.0) as usize)
        .min(text.line_count() - 1);
    let column = ((position.x - 8.0) / metrics.cell).floor().max(0.0) as usize;
    VisibleLine::new(
        ui,
        text,
        metrics,
        row,
        column.saturating_sub(8)..column.saturating_add(16),
    )
    .hit(position.x - 8.0)
}
