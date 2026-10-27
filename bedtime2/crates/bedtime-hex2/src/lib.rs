//! Read-only byte presentation. The host supplies cached bytes and performs I/O.
use bedtime_ui2::egui::{self, Align2, Color32, FontId, Key, Rect, Sense, Vec2};
use std::ops::Range;

pub const BYTES_PER_ROW: u64 = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CopyFormat {
    Hex,
    Text,
    Rows,
    Offset,
}

#[derive(Default)]
pub struct HexView {
    pub top_row: u64,
    cursor: u64,
    anchor: u64,
    horizontal: f32,
    pub rendered_rows: usize,
    pub rect: Option<Rect>,
}

pub struct HexOutput {
    pub response: egui::Response,
    pub needed: Range<u64>,
    pub copy: Option<CopyFormat>,
    pub find: bool,
    pub goto: bool,
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
    
    pub fn select(&mut self, range: Range<u64>, len: u64) {
        self.anchor = range.start;
        self.cursor = range.end.saturating_sub(1).max(range.start);
        self.clamp(len);
        self.top_row = self.anchor / BYTES_PER_ROW;
    }
    
    pub fn go_to(&mut self, offset: u64, len: u64) {
        self.select(offset..offset.saturating_add(1), len);
    }
    
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        id: impl egui::AsIdSalt,
        len: u64,
        byte: impl Fn(u64) -> Option<u8>,
    ) -> HexOutput {
        self.clamp(len);
        let font = FontId::monospace(egui::TextStyle::Body.resolve(ui.style()).size);
        let char_width = ui.fonts_mut(|f| f.glyph_width(&font, '0'));
        let row_height = font.size + 5.0;
        let (_, rect) = ui.allocate_space(ui.available_size().max(Vec2::splat(1.0)));
        let response = ui.interact(rect, ui.id().with(id), Sense::click_and_drag());
        if response.clicked() || response.drag_started() {
            response.request_focus();
        }
        self.rect = Some(rect);
        let rows = ((rect.height() - row_height).max(row_height) / row_height).floor() as u64;
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
        let mut copy = None;
        let mut find = false;
        let mut goto = false;
        if response.has_focus() {
            ui.input(|i| {
                find = i.modifiers.command && i.key_pressed(Key::F);
                goto = i.modifiers.command && i.key_pressed(Key::G);
                if i.events.iter().any(|e| matches!(e, egui::Event::Copy)) {
                    copy = Some(CopyFormat::Hex);
                }
                if i.modifiers.command && i.key_pressed(Key::A) {
                    self.anchor = 0;
                    self.cursor = len.saturating_sub(1);
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
        // Only the scrollbar maps to a fraction. Stored rows, hit testing and
        // navigation stay integer-based; no huge floating-point scroll canvas.
        let bar = Rect::from_min_max(egui::pos2(rect.right() - 12.0, rect.top()), rect.max);
        let bar_response = ui.interact(bar, response.id.with("scroll"), Sense::click_and_drag());
        if (bar_response.dragged() || bar_response.clicked())
            && let Some(p) = bar_response.interact_pointer_pos()
        {
            let fraction = ((p.y - bar.top()) / bar.height()).clamp(0.0, 1.0) as f64;
            self.top_row = (fraction * last_top as f64).round() as u64;
        }
        let painter = ui
            .painter()
            .with_clip_rect(rect.shrink2(Vec2::new(0.0, 0.0)));
        painter.rect_filled(bar, 3.0, ui.visuals().faint_bg_color);
        let thumb_height = if total_rows == 0 {
            bar.height()
        } else {
            (rows as f64 / total_rows as f64 * bar.height() as f64) as f32
        }
        .clamp(16.0_f32.min(bar.height()), bar.height());
        let fraction = if last_top == 0 {
            0.0
        } else {
            self.top_row as f64 / last_top as f64
        };
        let thumb = Rect::from_min_size(
            egui::pos2(
                bar.left(),
                bar.top() + fraction as f32 * (bar.height() - thumb_height),
            ),
            Vec2::new(bar.width(), thumb_height),
        );
        painter.rect_filled(thumb, 3.0, ui.visuals().widgets.inactive.bg_fill);
        let digits = format!("{:X}", len.saturating_sub(1)).len().max(8);
        let hex_x = (digits as f32 + 2.0) * char_width;
        let ascii_x = hex_x + 50.0 * char_width;
        let width = ascii_x + 16.0 * char_width;
        self.horizontal = self.horizontal.min((width - rect.width() + 20.0).max(0.0));
        let left = rect.left() - self.horizontal;
        let hit = |p: egui::Pos2, top: u64| -> Option<u64> {
            if p.x >= bar.left() || p.y < rect.top() + row_height {
                return None;
            }
            let row = ((p.y - rect.top() - row_height) / row_height).floor() as u64;
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
            let offset = top
                .saturating_add(row)
                .saturating_mul(16)
                .saturating_add(col);
            (offset < len).then_some(offset)
        };
        if (response.clicked() || response.drag_started() || response.dragged())
            && let Some(p) = response.interact_pointer_pos()
            && let Some(offset) = hit(p, self.top_row)
        {
            if !response.dragged() && !ui.input(|i| i.modifiers.shift) {
                self.anchor = offset;
            }
            if response.drag_started() && !ui.input(|i| i.modifiers.shift) {
                self.anchor = offset;
            }
            self.cursor = offset;
        }
        let ink = ui.visuals().text_color();
        let muted = ui.visuals().weak_text_color();
        painter.text(
            egui::pos2(left, rect.top()),
            Align2::LEFT_TOP,
            "Offset",
            font.clone(),
            muted,
        );
        painter.text(
            egui::pos2(left + hex_x, rect.top()),
            Align2::LEFT_TOP,
            "Hexadecimal bytes",
            font.clone(),
            muted,
        );
        painter.text(
            egui::pos2(left + ascii_x, rect.top()),
            Align2::LEFT_TOP,
            "ASCII",
            font.clone(),
            muted,
        );
        self.rendered_rows = 0;
        let selection = self.selection(len);
        for row in 0..rows.min(total_rows.saturating_sub(self.top_row)) {
            let offset = (self.top_row + row) * 16;
            let y = rect.top() + (row + 1) as f32 * row_height;
            painter.text(
                egui::pos2(left, y),
                Align2::LEFT_TOP,
                format!("{offset:0digits$X}"),
                font.clone(),
                muted,
            );
            for column in 0..16_u64.min(len - offset) {
                let pos = offset + column;
                let x = left
                    + hex_x
                    + (column as f32 * 3.0 + if column >= 8 { 1.0 } else { 0.0 }) * char_width;
                let ax = left + ascii_x + column as f32 * char_width;
                let selected = selection.contains(&pos);
                if selected {
                    for (x, w) in [(x, 2.0 * char_width), (ax, char_width)] {
                        painter.rect_filled(
                            Rect::from_min_size(egui::pos2(x, y), Vec2::new(w, row_height)),
                            1.0,
                            ui.visuals().selection.bg_fill,
                        );
                    }
                }
                let color = if selected {
                    ui.visuals().selection.stroke.color
                } else {
                    ink
                };
                let (hex, ascii, ascii_color) = match byte(pos) {
                    Some(b) => (
                        format!("{b:02X}"),
                        if b.is_ascii_graphic() || b == b' ' {
                            (b as char).to_string()
                        } else {
                            ".".into()
                        },
                        if b.is_ascii_graphic() { color } else { muted },
                    ),
                    None => ("··".into(), "·".into(), muted),
                };
                painter.text(egui::pos2(x, y), Align2::LEFT_TOP, hex, font.clone(), color);
                painter.text(
                    egui::pos2(ax, y),
                    Align2::LEFT_TOP,
                    ascii,
                    font.clone(),
                    ascii_color,
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
                Color32::GRAY,
            );
        }
        let start = self.top_row * 16;
        let end = start.saturating_add(rows.saturating_mul(16)).min(len);
        HexOutput {
            response,
            needed: start..end,
            copy,
            find,
            goto,
        }
    }
}

pub fn parse_offset(text: &str) -> Result<u64, String> {
    let text = text.trim();
    if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16)
            .map_err(|_| "Enter a decimal offset or hexadecimal offset starting with 0x".into())
    } else {
        text.parse()
            .map_err(|_| "Enter a decimal offset or hexadecimal offset starting with 0x".into())
    }
}

pub fn parse_hex(text: &str) -> Result<Vec<u8>, String> {
    let digits: Vec<_> = text.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if digits.is_empty() || digits.len() % 2 != 0 {
        return Err("Enter two hex digits per byte, for example 89 50 4E 47".into());
    }
    digits
        .chunks_exact(2)
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

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn exact_large_offsets_and_selection_survive_truncation() {
        let mut view = HexView::default();
        let offset = (1_u64 << 55) + 37;
        view.go_to(offset, offset + 100);
        assert_eq!(view.cursor(), offset);
        assert_eq!(view.top_row, offset / 16);
        assert_eq!(view.selection(offset + 100), offset..offset + 1);
        view.clamp(3);
        assert_eq!(view.selection(3), 2..3);
        view.clamp(0);
        assert_eq!(view.selection(0), 0..0);
    }
    
    #[test]
    fn viewport_is_bounded_at_large_offsets_and_uses_application_text_size() {
        let ctx = egui::Context::default();
        ctx.global_style_mut(|style| {
            style
                .text_styles
                .insert(egui::TextStyle::Body, FontId::proportional(23.0));
            style
                .text_styles
                .insert(egui::TextStyle::Monospace, FontId::monospace(9.0));
        });
        let mut view = HexView::default();
        let offset = (1_u64 << 40) + 32;
        view.go_to(offset, offset + 4096);
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::new(900.0, 500.0),
                )),
                ..Default::default()
            },
            |ui| {
                let output = view.show(ui, "hex", offset + 4096, |_| Some(0x41));
                assert_eq!(output.needed.start, offset);
                assert!(output.needed.end - output.needed.start < 1024);
            },
        );
        output.textures_delta.clear();
        assert!(view.rendered_rows > 0 && view.rendered_rows < 30);
        let texts: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|shape| {
                if let egui::Shape::Text(text) = &shape.shape {
                    Some(text)
                } else {
                    None
                }
            })
            .collect();
        assert!(!texts.is_empty());
        for text in texts {
            for section in &text.galley.job.sections {
                assert_eq!(section.format.font_id.size, 23.0);
            }
        }
    }

    #[test]
    fn user_offset_and_byte_inputs_are_checked() {
        assert_eq!(parse_offset("0x100000001").unwrap(), 4_294_967_297);
        assert_eq!(parse_offset("42").unwrap(), 42);
        assert!(parse_offset("18446744073709551616").is_err());
        assert_eq!(parse_hex("89 50\n4e47").unwrap(), [0x89, 0x50, 0x4e, 0x47]);
        assert!(parse_hex("F").is_err());
        assert!(parse_hex("xx").is_err());
    }
}

#[cfg(feature = "workbench")]
pub mod workbench;
