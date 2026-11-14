//! Drawing bytes: offsets, hex and text columns, a selection, keyboard and
//! mouse navigation. Only the rows on screen are drawn, and positions stay
//! integers, so offsets past 4 GiB are exact.
use egui::{self, Align2, Color32, FontId, Key, Pos2, Rect, Sense, Vec2};
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
    /// The first row shown.
    pub top_row: u64,
    /// How far the first row is scrolled off the top, in points.
    top_offset: f32,
    cursor: u64,
    anchor: u64,
    horizontal: f32,
    pub rendered_rows: usize,
    /// A match of the search, highlighted.
    pub found: Option<Range<u64>>,
    /// Room to scroll past the last row, for what floats over the bottom.
    pub end_padding: f32,
    /// A drag selecting bytes: whether it began in the text column.
    drag: Option<bool>,
    /// Where each scrollbar's thumb is held (horizontal, vertical).
    grab: [Option<f32>; 2],
}

pub struct HexOutput {
    pub response: egui::Response,
    pub copy: bool,
}

/// A scroll position: whole rows, and points into the next one. Rows stay
/// integers, so only the part of a row is ever a fraction.
#[derive(Clone, Copy, Default, PartialEq, PartialOrd, Debug)]
struct Scroll {
    row: u64,
    offset: f32,
}

impl Scroll {
    /// Where the last of `rows` rows ends at the bottom of `height`.
    fn bottom(rows: u64, height: f32, row_height: f32) -> Self {
        let whole = (height / row_height).floor().max(0.0);
        let rest = height - whole * row_height;
        let whole = whole as u64;
        if rows <= whole {
            Self::default()
        } else if rest > 0.01 {
            Self {
                row: rows - whole - 1,
                offset: row_height - rest,
            }
        } else {
            Self {
                row: rows - whole,
                offset: 0.0,
            }
        }
    }

    /// As rows, for the scrollbar alone.
    fn rows(self, row_height: f32) -> f64 {
        self.row as f64 + (self.offset / row_height) as f64
    }
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
            self.top_offset = 0.0;
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

    fn scroll(&self) -> Scroll {
        Scroll {
            row: self.top_row,
            offset: self.top_offset,
        }
    }

    fn set_scroll(&mut self, scroll: Scroll, max: Scroll) {
        let scroll = if scroll > max { max } else { scroll };
        self.top_row = scroll.row;
        self.top_offset = scroll.offset;
    }

    /// Scroll down by `points` (up when negative), no further than `max`.
    /// Whether that moved anything.
    fn scroll_by(&mut self, points: f32, row_height: f32, max: Scroll) -> bool {
        let before = self.scroll();
        let offset = self.top_offset + points;
        let rows = (offset / row_height).floor();
        let offset = (offset - rows * row_height).clamp(0.0, row_height);
        let scroll = if rows >= 0.0 {
            Scroll {
                row: self.top_row.saturating_add(rows as u64),
                offset,
            }
        } else {
            match self.top_row.checked_sub(-rows as u64) {
                Some(row) => Scroll { row, offset },
                None => Scroll::default(),
            }
        };
        self.set_scroll(scroll, max);
        self.scroll() != before
    }

    /// Scroll just enough to show all of `row` in a body `height` tall.
    fn reveal(&mut self, row: u64, height: f32, row_height: f32, max: Scroll) {
        let top = Scroll { row, offset: 0.0 };
        let bottom = Scroll::bottom(row.saturating_add(1), height, row_height);
        let now = self.scroll();
        if now > top {
            self.set_scroll(top, max);
        } else if now < bottom {
            self.set_scroll(bottom, max);
        } else {
            self.set_scroll(now, max);
        }
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
        // The column names stay at the top; rows scroll in the body under them.
        let top = rect.top() + 6.0;
        let body = Rect::from_min_max(
            egui::pos2(rect.left(), (top + row_height - 1.0).min(rect.bottom())),
            rect.max,
        );
        let rows = Self::rows_in(rect.height(), row_height);
        let total_rows = len.div_ceil(BYTES_PER_ROW);
        let digits = format!("{:X}", len.saturating_sub(1)).len().max(8);
        let margin = 12.0;
        let hex_x = margin + (digits as f32 + 2.0) * char_width;
        let ascii_x = hex_x + 50.0 * char_width;
        let bar_width = 12.0;
        // What scrolls sideways ends clear of the vertical scrollbar, and
        // rows end clear of the horizontal one.
        let width = ascii_x + 16.0 * char_width + margin.max(bar_width);
        let max_x = (width - rect.width()).max(0.0);
        let height = body.height() - if max_x > 0.0 { bar_width } else { 4.0 };
        let max = Scroll::bottom(
            total_rows,
            (height - self.end_padding).max(row_height),
            row_height,
        );
        self.set_scroll(self.scroll(), max);
        self.horizontal = self.horizontal.clamp(0.0, max_x);
        // The wheel scrolls by what it says, a part of a row included; what
        // isn't used (at either end) is left for whatever is around the view.
        if response.hovered() {
            let delta = ui.input(|i| i.smooth_scroll_delta);
            let before = self.horizontal;
            self.horizontal = (before - delta.x).clamp(0.0, max_x);
            let moved = self.scroll_by(-delta.y, row_height, max);
            let sideways = self.horizontal != before;
            ui.input_mut(|i| {
                if moved {
                    i.smooth_scroll_delta.y = 0.0;
                }
                if sideways {
                    i.smooth_scroll_delta.x = 0.0;
                }
            });
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
                    // A page moves the view with the cursor, which keeps its
                    // place on screen.
                    let page = Scroll {
                        row: match key {
                            Key::PageUp => self.top_row.saturating_sub(rows),
                            Key::PageDown => self.top_row.saturating_add(rows),
                            _ => self.top_row,
                        },
                        offset: self.top_offset,
                    };
                    self.set_scroll(page, max);
                    self.reveal(next / 16, height, row_height, max);
                    if next + 1 == len && key == Key::End {
                        // The end of the file, clear of what floats over it.
                        self.set_scroll(max, max);
                    }
                }
            });
        }
        // Only the scrollbars map to a fraction; rows, hit testing and
        // navigation stay integers.
        let mut bars = Vec::new();
        let bar_left = if max > Scroll::default() {
            let track = Rect::from_min_max(
                egui::pos2(rect.right() - bar_width, body.top()),
                egui::pos2(
                    rect.right(),
                    rect.bottom() - if max_x > 0.0 { bar_width } else { 0.0 },
                ),
            );
            let span = max.rows(row_height);
            let (moved, thumb) = scrollbar(
                ui,
                id.with("scroll"),
                track,
                true,
                (height as f64 / (total_rows as f64 * row_height as f64)) as f32,
                (self.scroll().rows(row_height) / span) as f32,
                &mut self.grab[1],
                colors.muted,
            );
            if let Some(at) = moved {
                let at = at as f64 * span;
                let scroll = Scroll {
                    row: at.floor() as u64,
                    offset: (at.fract() as f32 * row_height).floor(),
                };
                self.set_scroll(scroll, max);
            }
            bars.push(thumb);
            track.left()
        } else {
            rect.right()
        };
        if max_x > 0.0 {
            let track = Rect::from_min_max(
                egui::pos2(rect.left(), rect.bottom() - bar_width),
                egui::pos2(bar_left, rect.bottom()),
            );
            let (moved, thumb) = scrollbar(
                ui,
                id.with("scroll-x"),
                track,
                false,
                rect.width() / width,
                self.horizontal / max_x,
                &mut self.grab[0],
                colors.muted,
            );
            if let Some(at) = moved {
                self.horizontal = at * max_x;
            }
            bars.push(thumb);
        }
        let pixels = ui.ctx().pixels_per_point();
        let left = (rect.left() - self.horizontal).round();
        let hex_columns = hex_x / char_width;
        let text_columns = ascii_x / char_width;
        // The byte at a point, and whether that's in the text column. With
        // `held` (a drag that began in the hex or the text column), the
        // nearest byte of that column, so a drag may leave the bytes.
        let hit = |p: Pos2, scroll: Scroll, held: Option<bool>| -> Option<(u64, bool)> {
            if len == 0 {
                return None;
            }
            let x = (p.x - left) / char_width;
            if held.is_none() && (p.x >= bar_left || p.y < body.top() || x < hex_columns) {
                return None;
            }
            let text = held.unwrap_or(x >= text_columns);
            let column = if text {
                (x - text_columns).floor()
            } else {
                // A byte takes half of the gap on each side of it.
                let x = x - hex_columns + 0.5;
                (if x < 24.5 { x } else { x - 1.0 } / 3.0).floor()
            };
            let y = p.y.clamp(body.top(), body.bottom().max(body.top())) - body.top() - 1.0;
            let row = ((y + scroll.offset) / row_height).floor().max(0.0) as u64;
            if held.is_none() && column >= 16.0 {
                return None;
            }
            let offset = scroll
                .row
                .saturating_add(row)
                .saturating_mul(16)
                .saturating_add(column.clamp(0.0, 15.0) as u64);
            if held.is_some() {
                return Some((offset.min(len - 1), text));
            }
            (offset < len).then_some((offset, text))
        };
        // The row whose offset is at a point.
        let offset_hit = |p: Pos2, scroll: Scroll| -> Option<u64> {
            let x = (p.x - left) / char_width;
            if p.y < body.top() || x >= hex_columns - 1.0 || x < 0.0 {
                return None;
            }
            let row = ((p.y - body.top() - 1.0 + scroll.offset) / row_height)
                .floor()
                .max(0.0) as u64;
            let offset = scroll.row.saturating_add(row).saturating_mul(16);
            (offset < len).then_some(offset)
        };
        let shift = ui.input(|i| i.modifiers.shift);
        if response.clicked() || response.drag_started() {
            // A drag starts where the button went down, not where the
            // pointer was once it counted as a drag.
            let pressed = if response.drag_started() {
                ui.input(|i| i.pointer.press_origin())
            } else {
                response.interact_pointer_pos()
            };
            let pressed = pressed.filter(|p| rect.contains(*p));
            self.drag = None;
            if let Some(p) = pressed {
                if let Some((offset, text)) = hit(p, self.scroll(), None) {
                    if !shift {
                        self.anchor = offset;
                    }
                    self.cursor = offset;
                    if response.drag_started() {
                        self.drag = Some(text);
                    }
                } else if let Some(offset) = offset_hit(p, self.scroll()) {
                    // An offset selects its row.
                    if !shift {
                        self.anchor = offset;
                    }
                    self.cursor = (offset + 15).min(len - 1);
                }
            }
        }
        if !response.dragged() {
            self.drag = None;
        }
        if let Some(held) = self.drag
            && let Some(p) = response.interact_pointer_pos()
        {
            // Past the top or the bottom, the view follows: faster the
            // further out.
            let past = if p.y < body.top() {
                p.y - body.top()
            } else {
                (p.y - body.bottom()).max(0.0)
            };
            if past != 0.0 {
                let seconds = ui.input(|i| i.stable_dt).min(0.1);
                let reach = row_height * 4.0;
                self.scroll_by(past.clamp(-reach, reach) * 20.0 * seconds, row_height, max);
                ui.ctx().request_repaint();
            }
            if let Some((offset, _)) = hit(p, self.scroll(), Some(held)) {
                self.cursor = offset;
            }
        }
        // A right click outside the selection selects the byte under it.
        if response.secondary_clicked()
            && let Some(p) = response.interact_pointer_pos()
            && let Some((offset, _)) = hit(p, self.scroll(), None)
            && !self.selection(len).contains(&offset)
        {
            self.anchor = offset;
            self.cursor = offset;
        }
        let hovered = response
            .hover_pos()
            .filter(|_| self.drag.is_none())
            .and_then(|p| hit(p, self.scroll(), None))
            .map(|(offset, _)| offset);
        let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
        // The names of the columns; the cursor's is lit, like its row's offset.
        let quiet = colors.muted.gamma_multiply(0.7);
        painter.text(
            egui::pos2(left + margin, top),
            Align2::LEFT_TOP,
            "Offset",
            font.clone(),
            quiet,
        );
        let column_x = |column: u64| {
            left + hex_x + (column as f32 * 3.0 + if column >= 8 { 1.0 } else { 0.0 }) * char_width
        };
        for column in 0..16_u64 {
            painter.text(
                egui::pos2(column_x(column), top),
                Align2::LEFT_TOP,
                format!("{column:02X}"),
                font.clone(),
                if len > 0 && self.cursor % 16 == column {
                    colors.text
                } else {
                    quiet
                },
            );
        }
        let scroll = self.scroll();
        if scroll > Scroll::default() {
            painter.hline(
                rect.x_range(),
                body.top(),
                egui::Stroke::new(1.0, colors.muted.gamma_multiply(0.25)),
            );
        }
        self.rendered_rows = 0;
        let selection = self.selection(len);
        let found = self.found.clone().unwrap_or(0..0);
        let painter = ui.painter().with_clip_rect(body.intersect(ui.clip_rect()));
        // Whole pixels, so text stays sharp between rows.
        let shift = (scroll.offset * pixels).round() / pixels;
        let visible = ((body.height() + shift) / row_height).ceil().max(0.0) as u64;
        for row in 0..visible.min(total_rows.saturating_sub(scroll.row)) {
            let offset = (scroll.row + row) * 16;
            let y = body.top() + 1.0 - shift + row as f32 * row_height;
            painter.text(
                egui::pos2(left + margin, y),
                Align2::LEFT_TOP,
                format!("{offset:0digits$X}"),
                font.clone(),
                if self.cursor / 16 * 16 == offset {
                    colors.text
                } else {
                    colors.muted
                },
            );
            for column in 0..16_u64.min(len - offset) {
                let at = offset + column;
                let x = column_x(column);
                let ax = left + ascii_x + column as f32 * char_width;
                let fill = if selection.contains(&at) {
                    Some(colors.selection)
                } else if found.contains(&at) {
                    Some(colors.match_fill)
                } else if hovered == Some(at) {
                    Some(colors.muted.gamma_multiply(0.3))
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
        let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
        painter.extend(bars);
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

/// A scrollbar in `track`: a thumb `shown` of the track long (0 to 1), `at`
/// a place along it (0 to 1). Pressing the thumb holds it where it was
/// pressed; pressing the track brings the thumb's middle there. Returns
/// where the thumb was taken, and the thumb to paint over the content.
#[allow(clippy::too_many_arguments)]
fn scrollbar(
    ui: &egui::Ui,
    id: egui::Id,
    track: Rect,
    vertical: bool,
    shown: f32,
    at: f32,
    grab: &mut Option<f32>,
    color: Color32,
) -> (Option<f32>, egui::Shape) {
    let axis = usize::from(vertical);
    let length = track.size()[axis];
    let thumb = (shown * length).clamp(24.0_f32.min(length), length);
    let travel = (length - thumb).max(1.0);
    let mut at = at.clamp(0.0, 1.0);
    let response = ui.interact(track, id, Sense::click_and_drag());
    let mut moved = None;
    match response.interact_pointer_pos() {
        Some(p) if response.is_pointer_button_down_on() => {
            let p = p[axis] - track.min[axis];
            let start = at * travel;
            let held = *grab.get_or_insert(if (start..=start + thumb).contains(&p) {
                p - start
            } else {
                thumb / 2.0
            });
            let next = ((p - held) / travel).clamp(0.0, 1.0);
            if next != at {
                at = next;
                moved = Some(next);
            }
        }
        _ => *grab = None,
    }
    // Thin until the pointer is on it.
    let active = response.hovered() || grab.is_some();
    let wide = ui.ctx().animate_bool_responsive(id, active);
    let thickness = egui::lerp(4.0..=8.0, wide);
    let start = track.min[axis] + at * travel;
    let rect = if vertical {
        Rect::from_min_size(
            egui::pos2(track.right() - 2.0 - thickness, start),
            Vec2::new(thickness, thumb),
        )
    } else {
        Rect::from_min_size(
            egui::pos2(start, track.bottom() - 2.0 - thickness),
            Vec2::new(thumb, thickness),
        )
    };
    let shape = egui::Shape::rect_filled(
        rect,
        thickness / 2.0,
        color.gamma_multiply(egui::lerp(0.45..=0.8, wide)),
    );
    (moved, shape)
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
        .as_chunks::<2>()
        .0
        .iter()
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
    fn scrolling_keeps_parts_of_rows_and_stops_at_the_ends() {
        // 100 rows of 20 points in a body 90 tall: four and a half rows fit.
        let max = Scroll::bottom(100, 90.0, 20.0);
        assert_eq!(
            max,
            Scroll {
                row: 95,
                offset: 10.0
            }
        );
        assert_eq!(Scroll::bottom(4, 90.0, 20.0), Scroll::default());
        assert_eq!(
            Scroll::bottom(100, 80.0, 20.0),
            Scroll {
                row: 96,
                offset: 0.0
            }
        );
        let mut view = HexView::default();
        assert!(view.scroll_by(7.0, 20.0, max));
        assert_eq!(
            view.scroll(),
            Scroll {
                row: 0,
                offset: 7.0
            }
        );
        assert!(view.scroll_by(35.0, 20.0, max));
        assert_eq!(
            view.scroll(),
            Scroll {
                row: 2,
                offset: 2.0
            }
        );
        assert!(view.scroll_by(-5.0, 20.0, max));
        assert_eq!(
            view.scroll(),
            Scroll {
                row: 1,
                offset: 17.0
            }
        );
        assert!(view.scroll_by(-1000.0, 20.0, max));
        assert_eq!(view.scroll(), Scroll::default());
        assert!(!view.scroll_by(-3.0, 20.0, max));
        assert!(view.scroll_by(1e9, 20.0, max));
        assert_eq!(view.scroll(), max);
        assert!(!view.scroll_by(3.0, 20.0, max));
        // Far into a huge file, rows stay exact.
        let far = 1_u64 << 58;
        let max = Scroll::bottom(far + 100, 90.0, 20.0);
        view.set_scroll(
            Scroll {
                row: far,
                offset: 0.0,
            },
            max,
        );
        assert!(view.scroll_by(21.0, 20.0, max));
        assert_eq!(
            view.scroll(),
            Scroll {
                row: far + 1,
                offset: 1.0
            }
        );
    }

    #[test]
    fn revealing_a_row_scrolls_no_more_than_needed() {
        let max = Scroll::bottom(100, 90.0, 20.0);
        let mut view = HexView::default();
        view.set_scroll(
            Scroll {
                row: 10,
                offset: 5.0,
            },
            max,
        );
        // On screen already.
        view.reveal(12, 90.0, 20.0, max);
        assert_eq!(
            view.scroll(),
            Scroll {
                row: 10,
                offset: 5.0
            }
        );
        // Cut off at the top, then above.
        view.reveal(10, 90.0, 20.0, max);
        assert_eq!(
            view.scroll(),
            Scroll {
                row: 10,
                offset: 0.0
            }
        );
        view.reveal(3, 90.0, 20.0, max);
        assert_eq!(
            view.scroll(),
            Scroll {
                row: 3,
                offset: 0.0
            }
        );
        // Below: its bottom meets the body's.
        view.reveal(20, 90.0, 20.0, max);
        assert_eq!(
            view.scroll(),
            Scroll {
                row: 16,
                offset: 10.0
            }
        );
        view.reveal(99, 90.0, 20.0, max);
        assert_eq!(view.scroll(), max);
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
