//! Drawing bytes: offsets, hex and text columns, a selection, keyboard and
//! mouse navigation. Only the rows on screen are drawn, and positions stay
//! integers, so offsets past 4 GiB are exact.
use bedtime_ui::egui::{self, Align2, Color32, FontId, Key, Rect, Sense, Vec2};
use std::ops::Range;

pub const BYTES_PER_ROW: u64 = 16;

/// The colors bytes are drawn in, by kind.
#[derive(Clone, Copy)]
pub struct HexColors {
    pub text: Color32,
    pub muted: Color32,
    /// Zero bytes.
    pub zero: Color32,
    /// Bytes above 0x7F.
    pub high: Color32,
    /// Control characters.
    pub control: Color32,
    pub selection: Color32,
    pub match_fill: Color32,
}

#[derive(Default)]
pub struct HexView {
    pub top_row: u64,
    cursor: u64,
    anchor: u64,
    horizontal: f32,
    pub rendered_rows: usize,
    /// A match of the search, highlighted.
    pub found: Option<Range<u64>>,
}

pub struct HexOutput {
    pub response: egui::Response,
    pub copy: bool,
}

impl HexView {
    pub fn cursor(&self) -> u64 {
        self.cursor
    }

    pub fn selection(&self, len: u64) -> Range<u64> {
        if len == 0 {
            return 0..0;
        }
        self.anchor.min(self.cursor).min(len)
            ..self.anchor.max(self.cursor).saturating_add(1).min(len)
    }

    pub fn clamp(&mut self, len: u64) {
        self.cursor = self.cursor.min(len.saturating_sub(1));
        self.anchor = self.anchor.min(len.saturating_sub(1));
        self.top_row = self.top_row.min(len.saturating_sub(1) / BYTES_PER_ROW);
    }

    /// Select `range` and scroll it to the top (unless it's on screen).
    pub fn select(&mut self, range: Range<u64>, len: u64, rows: u64) {
        self.anchor = range.start;
        self.cursor = range.end.saturating_sub(1).max(range.start);
        self.clamp(len);
        let row = self.anchor / BYTES_PER_ROW;
        if row < self.top_row || row >= self.top_row + rows.max(1) {
            self.top_row = row.saturating_sub(rows / 3);
        }
    }

    pub fn select_all(&mut self, len: u64) {
        self.anchor = 0;
        self.cursor = len.saturating_sub(1);
    }

    pub fn go_to(&mut self, offset: u64, len: u64, rows: u64) {
        self.select(offset..offset.saturating_add(1), len, rows);
    }

    /// Rows that fit in `height`.
    pub fn rows_in(height: f32, row_height: f32) -> u64 {
        ((height - row_height).max(row_height) / row_height).floor() as u64
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        id: egui::Id,
        bytes: &[u8],
        font: FontId,
        colors: HexColors,
    ) -> HexOutput {
        let len = bytes.len() as u64;
        self.clamp(len);
        let char_width = ui.fonts_mut(|f| f.glyph_width(&font, '0'));
        let row_height = (font.size * 1.45).round();
        let (_, rect) = ui.allocate_space(ui.available_size().max(Vec2::splat(1.0)));
        let response = ui.interact(rect, id, Sense::click_and_drag());
        if response.clicked() || response.drag_started() || response.secondary_clicked() {
            response.request_focus();
        }
        let rows = Self::rows_in(rect.height(), row_height);
        let total_rows = len.div_ceil(BYTES_PER_ROW);
        let last_top = total_rows.saturating_sub(rows);
        self.top_row = self.top_row.min(last_top);
        if response.hovered() {
            let delta = ui.input_mut(|i| {
                let d = i.smooth_scroll_delta;
                i.smooth_scroll_delta = Vec2::ZERO;
                d
            });
            let step = (delta.y.abs() / row_height).ceil() as u64;
            if delta.y < 0.0 {
                self.top_row = self.top_row.saturating_add(step).min(last_top);
            }
            if delta.y > 0.0 {
                self.top_row = self.top_row.saturating_sub(step);
            }
            self.horizontal = (self.horizontal - delta.x).max(0.0);
        }
        let mut copy = false;
        if response.has_focus() {
            ui.input(|i| {
                copy = i.events.iter().any(|e| matches!(e, egui::Event::Copy));
                if i.modifiers.command && i.key_pressed(Key::A) {
                    self.select_all(len);
                }
                for key in [
                    Key::ArrowLeft,
                    Key::ArrowRight,
                    Key::ArrowUp,
                    Key::ArrowDown,
                    Key::PageUp,
                    Key::PageDown,
                    Key::Home,
                    Key::End,
                ] {
                    if !i.key_pressed(key) {
                        continue;
                    }
                    let next = match key {
                        Key::ArrowLeft => self.cursor.saturating_sub(1),
                        Key::ArrowRight => self.cursor.saturating_add(1),
                        Key::ArrowUp => self.cursor.saturating_sub(16),
                        Key::ArrowDown => self.cursor.saturating_add(16),
                        Key::PageUp => self.cursor.saturating_sub(rows * 16),
                        Key::PageDown => self.cursor.saturating_add(rows * 16),
                        Key::Home if i.modifiers.command || i.modifiers.ctrl => 0,
                        Key::End if i.modifiers.command || i.modifiers.ctrl => {
                            len.saturating_sub(1)
                        }
                        Key::Home => self.cursor / 16 * 16,
                        Key::End => self.cursor / 16 * 16 + 15,
                        _ => unreachable!(),
                    }
                    .min(len.saturating_sub(1));
                    self.cursor = next;
                    if !i.modifiers.shift {
                        self.anchor = next;
                    }
                    let row = next / 16;
                    if row < self.top_row {
                        self.top_row = row;
                    }
                    if row >= self.top_row + rows {
                        self.top_row = row.saturating_sub(rows - 1).min(last_top);
                    }
                }
            });
        }
        // Only the scrollbar maps to a fraction; rows, hit testing and
        // navigation stay integers.
        let bar = Rect::from_min_max(egui::pos2(rect.right() - 10.0, rect.top()), rect.max);
        let bar_response = ui.interact(bar, id.with("scroll"), Sense::click_and_drag());
        if (bar_response.dragged() || bar_response.clicked())
            && let Some(p) = bar_response.interact_pointer_pos()
        {
            let fraction = ((p.y - bar.top()) / bar.height()).clamp(0.0, 1.0) as f64;
            self.top_row = (fraction * last_top as f64).round() as u64;
        }
        let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
        if last_top > 0 {
            let thumb_height = (rows as f64 / total_rows as f64 * bar.height() as f64) as f32;
            let thumb_height = thumb_height.clamp(24.0_f32.min(bar.height()), bar.height());
            let fraction = self.top_row as f64 / last_top as f64;
            let thumb = Rect::from_min_size(
                egui::pos2(
                    bar.left() + 3.0,
                    bar.top() + fraction as f32 * (bar.height() - thumb_height),
                ),
                Vec2::new(4.0, thumb_height),
            );
            let hover = bar_response.hovered() || bar_response.dragged();
            painter.rect_filled(
                thumb,
                2.0,
                colors.muted.gamma_multiply(if hover { 0.8 } else { 0.45 }),
            );
        }
        let digits = format!("{:X}", len.saturating_sub(1)).len().max(8);
        let margin = 12.0;
        let hex_x = margin + (digits as f32 + 2.0) * char_width;
        let ascii_x = hex_x + 50.0 * char_width;
        let width = ascii_x + 16.0 * char_width + margin;
        self.horizontal = self.horizontal.min((width - rect.width() + 20.0).max(0.0));
        let left = rect.left() - self.horizontal;
        let top = rect.top() + 6.0;
        let hit = |p: egui::Pos2, top_row: u64| -> Option<u64> {
            if p.x >= bar.left() || p.y < top + row_height {
                return None;
            }
            let row = ((p.y - top - row_height) / row_height).floor() as u64;
            let x = (p.x - left) / char_width;
            let col = if x >= ascii_x / char_width {
                (x - ascii_x / char_width).floor() as u64
            } else if x >= hex_x / char_width {
                let x = x - hex_x / char_width;
                if x < 24.0 {
                    (x / 3.0).floor() as u64
                } else {
                    ((x - 1.0) / 3.0).floor() as u64
                }
            } else {
                return None;
            };
            if col >= 16 || row >= rows {
                return None;
            }
            let offset = top_row
                .saturating_add(row)
                .saturating_mul(16)
                .saturating_add(col);
            (offset < len).then_some(offset)
        };
        let shift = ui.input(|i| i.modifiers.shift);
        if (response.clicked() || response.drag_started() || response.dragged())
            && let Some(p) = response.interact_pointer_pos()
            && let Some(offset) = hit(p, self.top_row)
        {
            if (response.clicked() || response.drag_started()) && !shift {
                self.anchor = offset;
            }
            self.cursor = offset;
        }
        // A right click outside the selection selects the byte under it.
        if response.secondary_clicked()
            && let Some(p) = response.interact_pointer_pos()
            && let Some(offset) = hit(p, self.top_row)
            && !self.selection(len).contains(&offset)
        {
            self.anchor = offset;
            self.cursor = offset;
        }
        for (x, text) in [
            (margin, "Offset"),
            (hex_x, "00 01 02 03 04 05 06 07  08 09 0A 0B 0C 0D 0E 0F"),
        ] {
            painter.text(
                egui::pos2(left + x, top),
                Align2::LEFT_TOP,
                text,
                font.clone(),
                colors.muted.gamma_multiply(0.7),
            );
        }
        self.rendered_rows = 0;
        let selection = self.selection(len);
        let found = self.found.clone().unwrap_or(0..0);
        for row in 0..rows.min(total_rows.saturating_sub(self.top_row)) {
            let offset = (self.top_row + row) * 16;
            let y = top + (row + 1) as f32 * row_height;
            painter.text(
                egui::pos2(left + margin, y),
                Align2::LEFT_TOP,
                format!("{offset:0digits$X}"),
                font.clone(),
                colors.muted,
            );
            for column in 0..16_u64.min(len - offset) {
                let at = offset + column;
                let x = left
                    + hex_x
                    + (column as f32 * 3.0 + if column >= 8 { 1.0 } else { 0.0 }) * char_width;
                let ax = left + ascii_x + column as f32 * char_width;
                let fill = if selection.contains(&at) {
                    Some(colors.selection)
                } else if found.contains(&at) {
                    Some(colors.match_fill)
                } else {
                    None
                };
                if let Some(fill) = fill {
                    // Joined with the next selected byte in the row.
                    let joined = column < 15
                        && column != 7
                        && (selection.contains(&(at + 1)) || found.contains(&(at + 1)));
                    let hex_width = if joined { 3.0 } else { 2.0 } * char_width;
                    for (x, w) in [(x, hex_width), (ax, char_width)] {
                        painter.rect_filled(
                            Rect::from_min_size(
                                egui::pos2(x, y - 1.0),
                                Vec2::new(w, row_height - 2.0),
                            ),
                            0.0,
                            fill,
                        );
                    }
                }
                let byte = bytes[at as usize];
                let ink = match byte {
                    0 => colors.zero,
                    b if b.is_ascii_graphic() || b == b' ' => colors.text,
                    b if b < 0x80 => colors.control,
                    _ => colors.high,
                };
                let ascii = if byte.is_ascii_graphic() || byte == b' ' {
                    byte as char
                } else {
                    '·'
                };
                painter.text(
                    egui::pos2(x, y),
                    Align2::LEFT_TOP,
                    format!("{byte:02X}"),
                    font.clone(),
                    ink,
                );
                painter.text(
                    egui::pos2(ax, y),
                    Align2::LEFT_TOP,
                    ascii,
                    font.clone(),
                    if byte.is_ascii_graphic() {
                        colors.text
                    } else {
                        colors.zero
                    },
                );
            }
            self.rendered_rows += 1;
        }
        if len == 0 {
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                "Empty file",
                font,
                colors.muted,
            );
        }
        HexOutput { response, copy }
    }
}

/// An offset typed by the user: decimal, or hex with `0x`.
pub fn parse_offset(text: &str) -> Result<u64, String> {
    let text = text.trim();
    let error = || "Enter an offset: 1024, or 0x400".to_owned();
    if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16).map_err(|_| error())
    } else {
        text.parse().map_err(|_| error())
    }
}

/// What to find: hex bytes ("89 50 4E 47"), else the text's UTF-8 bytes.
pub fn search_bytes(text: &str) -> Vec<u8> {
    parse_hex(text).unwrap_or_else(|_| text.as_bytes().to_vec())
}

pub fn parse_hex(text: &str) -> Result<Vec<u8>, String> {
    let digits: Vec<_> = text.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if digits.is_empty() || !digits.len().is_multiple_of(2) {
        return Err("Enter two hex digits per byte, for example 89 50 4E 47".into());
    }
    digits
        .as_chunks::<2>().0.iter()
        .map(|pair| {
            let digit = |b: u8| {
                (b as char)
                    .to_digit(16)
                    .map(|n| n as u8)
                    .ok_or_else(|| "Invalid hex digit".to_owned())
            };
            Ok(digit(pair[0])? * 16 + digit(pair[1])?)
        })
        .collect()
}

/// The next match of `needle` after `from` (or before it, backwards),
/// wrapping around.
pub fn find(bytes: &[u8], needle: &[u8], from: u64, forward: bool) -> Option<u64> {
    if needle.is_empty() || needle.len() > bytes.len() {
        return None;
    }
    let last = bytes.len() - needle.len();
    let at = |i: usize| bytes[i..i + needle.len()] == *needle;
    let from = (from as usize).min(last + 1);
    if forward {
        (from..=last)
            .chain(0..from.min(last + 1))
            .find(|&i| at(i))
            .map(|i| i as u64)
    } else {
        (0..from.min(last + 1))
            .rev()
            .chain((from..=last).rev())
            .find(|&i| at(i))
            .map(|i| i as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn large_offsets_and_selection_survive_clamping() {
        let mut view = HexView::default();
        let offset = (1_u64 << 55) + 37;
        view.go_to(offset, offset + 100, 30);
        assert_eq!(view.cursor(), offset);
        assert_eq!(view.selection(offset + 100), offset..offset + 1);
        view.clamp(3);
        assert_eq!(view.selection(3), 2..3);
        view.clamp(0);
        assert_eq!(view.selection(0), 0..0);
    }

    #[test]
    fn offsets_and_searches_are_parsed() {
        assert_eq!(parse_offset("0x100000001").unwrap(), 4_294_967_297);
        assert_eq!(parse_offset("42").unwrap(), 42);
        assert!(parse_offset("18446744073709551616").is_err());
        assert_eq!(parse_hex("89 50\n4e47").unwrap(), [0x89, 0x50, 0x4e, 0x47]);
        assert!(parse_hex("F").is_err());
        assert_eq!(search_bytes("PNG"), b"PNG");
        assert_eq!(search_bytes("89 50"), [0x89, 0x50]);
    }

    #[test]
    fn find_wraps_both_ways() {
        let bytes = b"abcabcab";
        assert_eq!(find(bytes, b"abc", 0, true), Some(0));
        assert_eq!(find(bytes, b"abc", 1, true), Some(3));
        assert_eq!(find(bytes, b"abc", 4, true), Some(0));
        assert_eq!(find(bytes, b"abc", 3, false), Some(0));
        assert_eq!(find(bytes, b"abc", 0, false), Some(3));
        assert_eq!(find(bytes, b"zz", 0, true), None);
    }
}
