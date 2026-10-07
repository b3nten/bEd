//! ImGui drawing and input adapter translated from pinned imgui-terminal.
//! See NOTICE and LICENSES/terminal-adapter-BSL-1.1.txt for attribution.
use crate::{terminal::*, terminal_font::TerminalFonts};
use dear_imgui_rs::{FontId, Key, MouseButton, Ui, sys};
use std::io;

const BORDER: f32 = 2.0;
const DEFAULT_FG: usize = 258;
const DEFAULT_BG: usize = 259;
const CURSOR_COLOR: usize = 256;
const REVERSE_CURSOR_COLOR: usize = 257;

#[derive(Clone, Debug, PartialEq)]
pub enum DrawOp {
    Rect {
        p0: [f32; 2],
        p1: [f32; 2],
        color: [u8; 3],
    },
    Text {
        pos: [f32; 2],
        color: [u8; 3],
        style: usize,
        character: char,
    },
    PushClip {
        p0: [f32; 2],
        p1: [f32; 2],
    },
    PopClip,
}
#[derive(Clone, Copy, Debug)]
pub struct PaintMetrics {
    pub cw: f32,
    pub ch: f32,
    pub ascent: f32,
    pub width: f32,
    pub height: f32,
}
#[derive(Default)]
pub struct TerminalPaint {
    pub rows: Vec<Vec<DrawOp>>,
    pub overlay: Vec<DrawOp>,
}

pub fn font_style(cell: TerminalCell) -> usize {
    if !cell.character.is_ascii() {
        return 0;
    }
    let bold = cell.mode & (ATTR_BOLD | ATTR_FAINT) == ATTR_BOLD;
    match (bold, cell.mode & ATTR_ITALIC != 0) {
        (true, true) => 3,
        (true, false) => 1,
        (false, true) => 2,
        _ => 0,
    }
}
fn resolve(color: TerminalColor, palette: &[[u8; 3]; 260]) -> [u8; 3] {
    match color {
        TerminalColor::Indexed(i) => palette.get(i).copied().unwrap_or(palette[DEFAULT_FG]),
        TerminalColor::Rgb(rgb) => rgb,
    }
}
/// Color operations retain their upstream ordering, including the bold/faint exception.
pub fn glyph_colors(
    cell: TerminalCell,
    palette: &[[u8; 3]; 260],
    reverse: bool,
    blink: bool,
    bad_font: bool,
) -> ([u8; 3], [u8; 3]) {
    glyph_colors_with_faint(cell, palette, reverse, blink, bad_font, None)
}

fn glyph_colors_with_faint(
    cell: TerminalCell,
    palette: &[[u8; 3]; 260],
    reverse: bool,
    blink: bool,
    bad_font: bool,
    faint: Option<[u8; 3]>,
) -> ([u8; 3], [u8; 3]) {
    let mut fg = resolve(cell.fg, palette);
    let mut bg = resolve(cell.bg, palette);
    if bad_font {
        fg = palette[11];
    }
    if cell.mode & (ATTR_BOLD | ATTR_FAINT) == ATTR_BOLD
        && let TerminalColor::Indexed(i @ 0..=7) = cell.fg
    {
        fg = palette[i + 8];
    }
    if reverse {
        fg = if fg == palette[DEFAULT_FG] {
            palette[DEFAULT_BG]
        } else {
            fg.map(|v| v ^ 255)
        };
        bg = if bg == palette[DEFAULT_BG] {
            palette[DEFAULT_FG]
        } else {
            bg.map(|v| v ^ 255)
        };
    }
    if cell.mode & (ATTR_BOLD | ATTR_FAINT) == ATTR_FAINT {
        fg = faint.unwrap_or_else(|| fg.map(|v| v / 2));
    }
    if cell.mode & ATTR_REVERSE != 0 {
        std::mem::swap(&mut fg, &mut bg);
    }
    if cell.mode & ATTR_BLINK != 0 && blink || cell.mode & ATTR_INVISIBLE != 0 {
        fg = bg;
    }
    (fg, bg)
}
fn rect(out: &mut Vec<DrawOp>, p0: [f32; 2], p1: [f32; 2], color: [u8; 3]) {
    out.push(DrawOp::Rect { p0, p1, color });
}
#[allow(clippy::too_many_arguments)]
fn glyph_run(
    out: &mut Vec<DrawOp>,
    cells: &[(usize, TerminalCell)],
    row: usize,
    term: &Terminal,
    m: PaintMetrics,
    blink: bool,
    bad_fonts: [bool; 4],
    emoji_position: &mut impl FnMut(usize, char, [f32; 2]) -> [f32; 2],
) {
    if cells.is_empty() {
        return;
    }
    let (col, base) = cells[0];
    let x = BORDER + col as f32 * m.cw;
    let y = BORDER + row as f32 * m.ch;
    let width = cells.len() as f32
        * if base.mode & ATTR_WIDE != 0 {
            2.0 * m.cw
        } else {
            m.cw
        };
    let bad_font = bad_fonts[font_style(base)];
    let faint =
        if !term.modes().reverse && !bad_font && base.bg == TerminalColor::Indexed(DEFAULT_BG) {
            term.faint_color(base.fg)
        } else {
            None
        };
    let (fg, bg) = glyph_colors_with_faint(
        base,
        term.palette(),
        term.modes().reverse,
        blink,
        bad_font,
        faint,
    );
    let tw = term.cols() as f32 * m.cw;
    let th = term.rows() as f32 * m.ch;
    if col == 0 {
        rect(
            out,
            [0.0, if row == 0 { 0.0 } else { y }],
            [BORDER, y + m.ch],
            bg,
        );
    }
    if x + width >= BORDER + tw {
        rect(
            out,
            [x + width, if row == 0 { 0.0 } else { y }],
            [(BORDER + tw + BORDER).min(m.width), y + m.ch],
            bg,
        );
    }
    if row == 0 {
        rect(out, [x, 0.0], [x + width, BORDER], bg);
    }
    if y + m.ch >= BORDER + th {
        rect(
            out,
            [x, y + m.ch],
            [x + width, (BORDER + th + BORDER).min(m.height)],
            bg,
        );
    }
    rect(out, [x, y], [x + width, y + m.ch], bg);
    out.push(DrawOp::PushClip {
        p0: [x, y],
        p1: [x + width, y + m.ch],
    });
    let mut xp = x;
    for &(_, cell) in cells {
        let style = font_style(cell);
        let mut pos = [xp, y];
        if base.mode & ATTR_WIDE != 0 && is_emoji_glyph(cell.character) {
            pos = emoji_position(style, cell.character, pos);
        }
        out.push(DrawOp::Text {
            pos,
            color: fg,
            style,
            character: cell.character,
        });
        xp += m.cw * if cell.mode & ATTR_WIDE != 0 { 2.0 } else { 1.0 };
    }
    if base.mode & ATTR_UNDERLINE != 0 {
        rect(
            out,
            [x, y + m.ascent + 1.0],
            [x + width, y + m.ascent + 2.0],
            fg,
        );
    }
    if base.mode & ATTR_STRUCK != 0 {
        let sy = y + 2.0 * m.ascent / 3.0;
        rect(out, [x, sy], [x + width, sy + 1.0], fg);
    }
    out.push(DrawOp::PopClip);
}
#[allow(clippy::too_many_arguments)]
pub fn render_ops(
    term: &Terminal,
    m: PaintMetrics,
    focused: bool,
    cursor_on: bool,
    blink: bool,
    bad_fonts: [bool; 4],
    mut emoji_position: impl FnMut(usize, char, [f32; 2]) -> [f32; 2],
) -> TerminalPaint {
    let mut paint = TerminalPaint::default();
    for row in 0..term.rows() {
        let mut ops = Vec::new();
        let mut run = Vec::new();
        let mut count = 0;
        for col in 0..term.cols() {
            let mut cell = term.cell(row, col);
            if cell.mode == ATTR_WDUMMY {
                continue;
            }
            if count == 1024 {
                break;
            }
            count += 1;
            if term.is_selected(col, row) {
                cell.mode ^= ATTR_REVERSE;
            }
            if run
                .first()
                .is_some_and(|&(_, base): &(usize, TerminalCell)| {
                    base.mode != cell.mode || base.fg != cell.fg || base.bg != cell.bg
                })
            {
                glyph_run(
                    &mut ops,
                    &run,
                    row,
                    term,
                    m,
                    blink,
                    bad_fonts,
                    &mut emoji_position,
                );
                run.clear();
            }
            run.push((col, cell));
        }
        glyph_run(
            &mut ops,
            &run,
            row,
            term,
            m,
            blink,
            bad_fonts,
            &mut emoji_position,
        );
        paint.rows.push(ops);
    }
    let cursor = term.cursor();
    if !cursor.visible || focused && cursor.blinking && !cursor_on {
        return paint;
    }
    let row = cursor.row.min(term.rows().saturating_sub(1));
    let mut col = cursor.col.min(term.cols().saturating_sub(1));
    if term.cell(row, col).mode & ATTR_WDUMMY != 0 {
        col = col.saturating_sub(1);
    }
    let mut cell = term.cell(row, col);
    cell.mode &= ATTR_BOLD | ATTR_ITALIC | ATTR_UNDERLINE | ATTR_STRUCK | ATTR_WIDE;
    let palette = term.palette();
    let selected = term.is_selected(col, row);
    let draw_color = if term.modes().reverse {
        cell.mode |= ATTR_REVERSE;
        cell.bg = TerminalColor::Indexed(DEFAULT_FG);
        cell.fg = TerminalColor::Indexed(if selected {
            REVERSE_CURSOR_COLOR
        } else {
            CURSOR_COLOR
        });
        palette[if selected {
            CURSOR_COLOR
        } else {
            REVERSE_CURSOR_COLOR
        }]
    } else {
        cell.fg = TerminalColor::Indexed(if selected { DEFAULT_FG } else { DEFAULT_BG });
        let bg = if selected {
            REVERSE_CURSOR_COLOR
        } else {
            CURSOR_COLOR
        };
        cell.bg = TerminalColor::Indexed(bg);
        palette[bg]
    };
    let x = BORDER + col as f32 * m.cw;
    let y = BORDER + row as f32 * m.ch;
    if focused {
        match cursor.shape {
            0 | 1 | 2 | 7 => {
                if cursor.shape == 7 {
                    cell.character = '☃';
                }
                glyph_run(
                    &mut paint.overlay,
                    &[(col, cell)],
                    row,
                    term,
                    m,
                    blink,
                    bad_fonts,
                    &mut emoji_position,
                );
            }
            3 | 4 => rect(
                &mut paint.overlay,
                [x, y + m.ch - 2.0],
                [x + m.cw, y + m.ch],
                draw_color,
            ),
            5 | 6 => rect(&mut paint.overlay, [x, y], [x + 2.0, y + m.ch], draw_color),
            _ => {}
        }
    } else {
        rect(
            &mut paint.overlay,
            [x, y],
            [x + m.cw - 1.0, y + 1.0],
            draw_color,
        );
        rect(
            &mut paint.overlay,
            [x, y],
            [x + 1.0, y + m.ch - 1.0],
            draw_color,
        );
        rect(
            &mut paint.overlay,
            [x + m.cw - 1.0, y],
            [x + m.cw, y + m.ch - 1.0],
            draw_color,
        );
        rect(
            &mut paint.overlay,
            [x, y + m.ch - 1.0],
            [x + m.cw, y + m.ch],
            draw_color,
        );
    }
    paint
}

// Unicode 17.0 Emoji_Presentation ranges retained from the pinned adapter.
fn is_emoji_glyph(c: char) -> bool {
    let c = c as u32;
    if (0x1f000..=0x1faff).contains(&c) {
        return true;
    }
    const RANGES: &[(u32, u32)] = &[
        (0x231a, 0x231b),
        (0x23e9, 0x23ec),
        (0x23f0, 0x23f0),
        (0x23f3, 0x23f3),
        (0x25fd, 0x25fe),
        (0x2614, 0x2615),
        (0x2648, 0x2653),
        (0x267f, 0x267f),
        (0x2693, 0x2693),
        (0x26a1, 0x26a1),
        (0x26aa, 0x26ab),
        (0x26bd, 0x26be),
        (0x26c4, 0x26c5),
        (0x26ce, 0x26ce),
        (0x26d4, 0x26d4),
        (0x26ea, 0x26ea),
        (0x26f2, 0x26f3),
        (0x26f5, 0x26f5),
        (0x26fa, 0x26fa),
        (0x26fd, 0x26fd),
        (0x2705, 0x2705),
        (0x270a, 0x270b),
        (0x2728, 0x2728),
        (0x274c, 0x274c),
        (0x274e, 0x274e),
        (0x2753, 0x2755),
        (0x2757, 0x2757),
        (0x2795, 0x2797),
        (0x27b0, 0x27b0),
        (0x27bf, 0x27bf),
        (0x2b1b, 0x2b1c),
        (0x2b50, 0x2b50),
        (0x2b55, 0x2b55),
    ];
    RANGES.iter().any(|&(a, b)| (a..=b).contains(&c))
}

/// The session owns IO; emulator and draw state stay on the UI thread.
pub trait TerminalIo {
    fn pump(&mut self, terminal: &mut Terminal) -> io::Result<bool>;
    fn write(&mut self, bytes: &[u8]) -> io::Result<()>;
    fn resize(
        &mut self,
        cols: usize,
        rows: usize,
        cell_width: f32,
        cell_height: f32,
    ) -> io::Result<()>;
}

pub struct TerminalView {
    pub transparent_background: bool,
    pub focused: bool,
    blink: bool,
    blink_timer: f64,
    cursor_on: bool,
    cursor_timer: f64,
    last_shape: u8,
    last_cursor_revision: u64,
    clock_initialized: bool,
    last_revision: u64,
    font_ids: [Option<FontId>; 4],
    font_size: f32,
    selecting: bool,
    rectangular: bool,
    last_motion: [i32; 2],
    previous_mouse_mode: u8,
    geometry: PaintMetrics,
    grid: [usize; 2],
    paint: TerminalPaint,
}
impl Default for TerminalView {
    fn default() -> Self {
        Self {
            transparent_background: false,
            focused: false,
            blink: false,
            blink_timer: 0.0,
            cursor_on: true,
            cursor_timer: 0.0,
            last_shape: 2,
            last_cursor_revision: 0,
            clock_initialized: false,
            last_revision: u64::MAX,
            font_ids: [None; 4],
            font_size: 0.0,
            selecting: false,
            rectangular: false,
            last_motion: [-1, -1],
            previous_mouse_mode: 0,
            geometry: PaintMetrics {
                cw: 8.0,
                ch: 16.0,
                ascent: 0.0,
                width: 640.0,
                height: 384.0,
            },
            grid: [0, 0],
            paint: TerminalPaint::default(),
        }
    }
}
impl TerminalView {
    pub fn tick(
        &mut self,
        ui: &Ui,
        terminal: &mut Terminal,
        io: &mut impl TerminalIo,
    ) -> io::Result<bool> {
        self.initialize_clock(ui.time());
        let output = io.pump(terminal)?;
        Ok(self.tick_blink(ui.time(), terminal) || output)
    }
    fn initialize_clock(&mut self, time: f64) {
        if !self.clock_initialized {
            self.cursor_timer = time * 1000.0;
            self.clock_initialized = true;
        }
    }
    fn tick_blink(&mut self, time: f64, terminal: &Terminal) -> bool {
        let ms = time * 1000.0;
        let mut changed = false;
        let has_blink = (0..terminal.rows())
            .any(|r| (0..terminal.cols()).any(|c| terminal.cell(r, c).mode & ATTR_BLINK != 0));
        if ms - self.blink_timer >= 800.0 {
            self.blink_timer = ms;
            self.blink = if has_blink { !self.blink } else { true };
            changed |= has_blink;
        }
        let cursor = terminal.cursor();
        if cursor.shape != self.last_shape
            || terminal.cursor_revision() != self.last_cursor_revision
        {
            self.last_cursor_revision = terminal.cursor_revision();
            self.last_shape = cursor.shape;
            self.cursor_on = true;
        }
        if cursor.blinking && ms - self.cursor_timer >= 750.0 {
            self.cursor_timer = ms;
            self.cursor_on = !self.cursor_on;
            changed = true;
        }
        changed
    }
    fn input_write(
        &mut self,
        ui: &Ui,
        terminal: &mut Terminal,
        io: &mut impl TerminalIo,
        bytes: &[u8],
        echo: bool,
    ) -> io::Result<()> {
        if echo && terminal.cursor().blinking {
            self.cursor_on = true;
            self.cursor_timer = ui.time() * 1000.0;
        }
        if echo && terminal.modes().local_echo {
            for event in terminal.feed_echo(bytes) {
                if let TerminalEvent::Write(data) = event {
                    io.write(&data)?;
                }
            }
        }
        if terminal.modes().newline_mode {
            let mut cooked = Vec::with_capacity(bytes.len());
            for &byte in bytes {
                cooked.push(byte);
                if byte == b'\r' {
                    cooked.push(b'\n');
                }
            }
            io.write(&cooked)
        } else {
            io.write(bytes)
        }
    }
    pub fn draw(
        &mut self,
        ui: &Ui,
        terminal: &mut Terminal,
        fonts: &TerminalFonts,
        io: &mut impl TerminalIo,
    ) -> io::Result<bool> {
        self.initialize_clock(ui.time());
        let avail = ui.content_region_avail();
        let [cw, ch] = fonts.metrics(ui);
        let size = fonts.size.max(1.0);
        let ascent = fonts
            .regular
            .and_then(|f| ui.baked_font(f, size))
            .map(|f| f.ascent().ceil())
            .unwrap_or(0.0);
        let cols = ((avail[0] as i32) / (cw as i32).max(1)).max(1) as usize;
        let rows = ((avail[1] as i32) / (ch as i32).max(1)).max(1) as usize;
        let mut changed = false;
        if self.grid != [cols, rows] || self.geometry.cw != cw || self.geometry.ch != ch {
            self.geometry = PaintMetrics {
                cw,
                ch,
                ascent,
                width: avail[0] as i32 as f32,
                height: avail[1] as i32 as f32,
            };
            self.grid = [cols, rows];
            terminal.resize(cols, rows);
            io.resize(cols, rows, cw, ch)?;
            changed = true;
        } else {
            self.geometry.ascent = ascent;
        }
        let origin = ui.cursor_screen_pos();
        // Public InvisibleButton binding omits EnableNav; retain the native source flags.
        unsafe {
            sys::igInvisibleButton(
                c"##term_canvas".as_ptr(),
                sys::ImVec2 {
                    x: avail[0],
                    y: avail[1],
                },
                sys::ImGuiButtonFlags_MouseButtonLeft
                    | sys::ImGuiButtonFlags_MouseButtonRight
                    | sys::ImGuiButtonFlags_MouseButtonMiddle
                    | sys::ImGuiButtonFlags_EnableNav,
            );
        }
        let focused = ui.is_item_focused();
        if focused {
            unsafe {
                sys::igSetNextFrameWantCaptureKeyboard(true);
            }
        }
        if terminal.modes().focus_reporting && focused != self.focused {
            self.input_write(
                ui,
                terminal,
                io,
                if focused { b"\x1b[I" } else { b"\x1b[O" },
                false,
            )?;
        }
        changed |= focused != self.focused;
        self.focused = focused;
        if !self.transparent_background {
            let rgb = terminal.palette()[if terminal.modes().reverse {
                DEFAULT_FG
            } else {
                DEFAULT_BG
            }];
            ui.get_window_draw_list()
                .add_rect(
                    origin,
                    [origin[0] + avail[0], origin[1] + avail[1]],
                    packed(rgb),
                )
                .filled(true)
                .build();
        }
        changed |= io.pump(terminal)?;
        changed |= self.tick_blink(ui.time(), terminal);
        if focused && !terminal.modes().keyboard_lock {
            changed |= self.dispatch_keyboard(ui, terminal, io)?;
        }
        changed |= self.dispatch_mouse(ui, terminal, io, origin)?;
        let bad = std::array::from_fn(|i| fonts.bad_weight[i] || fonts.bad_slant[i]);
        let ids = std::array::from_fn(|i| fonts.for_style(i));
        changed |= terminal.revision() != self.last_revision
            || ids != self.font_ids
            || size != self.font_size;
        if changed || self.paint.rows.is_empty() {
            self.last_revision = terminal.revision();
            self.font_ids = ids;
            self.font_size = size;
            self.paint = render_ops(
                terminal,
                self.geometry,
                focused,
                self.cursor_on,
                self.blink,
                bad,
                |style, character, mut pos| {
                    if let Some(mut baked) = fonts
                        .for_style(style)
                        .and_then(|font| ui.baked_font(font, size))
                        && let Some(glyph) = baked.glyph_or_fallback(character)
                    {
                        let (lo, hi) = glyph.position_and_size();
                        let bounds = [lo[0], lo[1], hi[0], hi[1]];
                        let scale = size / baked.size();
                        pos[1] += (ch - (bounds[3] - bounds[1]) * scale) * 0.5 - bounds[1] * scale;
                        #[cfg(windows)]
                        if character as u32 > 0xffff {
                            pos[0] += (cw * 2.0 - (bounds[2] - bounds[0]) * scale) * 0.5
                                - bounds[0] * scale;
                        }
                    }
                    pos
                },
            );
        }
        for ops in &self.paint.rows {
            replay(
                ui,
                ops,
                origin,
                fonts,
                size,
                self.transparent_background,
                terminal.palette()[DEFAULT_BG],
            );
        }
        replay(
            ui,
            &self.paint.overlay,
            origin,
            fonts,
            size,
            self.transparent_background,
            terminal.palette()[DEFAULT_BG],
        );
        Ok(changed)
    }
    fn paste(
        &mut self,
        ui: &Ui,
        terminal: &mut Terminal,
        io: &mut impl TerminalIo,
    ) -> io::Result<()> {
        let Some(text) = clipboard_text().filter(|text| !text.is_empty()) else {
            return Ok(());
        };
        if terminal.modes().bracket_paste {
            self.input_write(ui, terminal, io, b"\x1b[200~", false)?;
        }
        self.input_write(ui, terminal, io, text.as_bytes(), false)?;
        if terminal.modes().bracket_paste {
            self.input_write(ui, terminal, io, b"\x1b[201~", false)?;
        }
        Ok(())
    }
    fn dispatch_keyboard(
        &mut self,
        ui: &Ui,
        terminal: &mut Terminal,
        io: &mut impl TerminalIo,
    ) -> io::Result<bool> {
        // Shortcut() resolves focused routing before the repeat-enabled source key map.
        let shortcuts = [
            (sys::ImGuiKey_C, sys::ImGuiMod_Ctrl | sys::ImGuiMod_Shift, 0),
            (sys::ImGuiKey_V, sys::ImGuiMod_Ctrl | sys::ImGuiMod_Shift, 1),
            (sys::ImGuiKey_Y, sys::ImGuiMod_Ctrl | sys::ImGuiMod_Shift, 1),
            (sys::ImGuiKey_Insert, sys::ImGuiMod_Shift, 1),
            (
                sys::ImGuiKey_NumLock,
                sys::ImGuiMod_Ctrl | sys::ImGuiMod_Shift,
                2,
            ),
        ];
        for (key, mods, action) in shortcuts {
            if unsafe { sys::igShortcut_Nil(key | mods, sys::ImGuiInputFlags_RouteFocused) } {
                match action {
                    0 => {
                        if let Some(text) = terminal.selection_text() {
                            set_clipboard_text(&text);
                        }
                    }
                    1 => self.paste(ui, terminal, io)?,
                    _ => terminal.set_num_lock(!terminal.modes().num_lock),
                }
                return Ok(true);
            }
        }
        let input = ui.io();
        let pressed = |key| ui.is_key_pressed_with_repeat(key, true);
        if let Some(bytes) = key_bytes(
            pressed,
            input.key_shift() && !input.key_ctrl() && !input.key_alt() && !input.key_super(),
            terminal.modes().app_cursor,
        ) {
            self.input_write(ui, terminal, io, bytes, true)?;
            return Ok(true);
        }
        if input.key_ctrl() && !input.key_shift() && !input.key_alt() && !input.key_super() {
            const LETTERS: [Key; 26] = [
                Key::A,
                Key::B,
                Key::C,
                Key::D,
                Key::E,
                Key::F,
                Key::G,
                Key::H,
                Key::I,
                Key::J,
                Key::K,
                Key::L,
                Key::M,
                Key::N,
                Key::O,
                Key::P,
                Key::Q,
                Key::R,
                Key::S,
                Key::T,
                Key::U,
                Key::V,
                Key::W,
                Key::X,
                Key::Y,
                Key::Z,
            ];
            for (index, key) in LETTERS.into_iter().enumerate() {
                if pressed(key) {
                    self.input_write(ui, terminal, io, &[index as u8 + 1], true)?;
                    return Ok(true);
                }
            }
            for (key, byte) in [
                (Key::Space, 0),
                (Key::LeftBracket, 27),
                (Key::Backslash, 28),
                (Key::RightBracket, 29),
            ] {
                if pressed(key) {
                    self.input_write(ui, terminal, io, &[byte], true)?;
                    return Ok(true);
                }
            }
        }
        let mut changed = false;
        for character in input_characters() {
            let bytes = character_bytes(character, input.key_alt(), terminal.modes().eight_bit);
            self.input_write(ui, terminal, io, &bytes, true)?;
            changed = true;
        }
        Ok(changed)
    }
    fn dispatch_mouse(
        &mut self,
        ui: &Ui,
        terminal: &mut Terminal,
        io: &mut impl TerminalIo,
        origin: [f32; 2],
    ) -> io::Result<bool> {
        let modes = terminal.modes();
        let mouse_mode = mouse_mode_bits(&modes);
        if mouse_mode != self.previous_mouse_mode {
            self.selecting = false;
            self.last_motion = [-1, -1];
            self.previous_mouse_mode = mouse_mode;
        }
        let input = ui.io();
        let hovered = ui.is_item_hovered();
        let pos = input.mouse_pos();
        let [col, row] =
            pixel_to_cell(pos, origin, self.geometry, terminal.cols(), terminal.rows());
        let buttons = [MouseButton::Left, MouseButton::Right, MouseButton::Middle];
        let codes = [0, 2, 1];
        let mut changed = false;
        if mouse_mode != 0 && !input.key_shift() {
            let mods = if modes.mouse_x10 {
                0
            } else {
                u8::from(input.key_shift()) * 4
                    + u8::from(input.key_alt()) * 8
                    + u8::from(input.key_ctrl()) * 16
            };
            let mut report = |code, release| -> io::Result<()> {
                self.input_write(
                    ui,
                    terminal,
                    io,
                    &mouse_packet(code, col, row, mods, release, modes.mouse_sgr),
                    false,
                )?;
                self.last_motion = [col as i32, row as i32];
                changed = true;
                Ok(())
            };
            for i in 0..3 {
                if hovered && ui.is_mouse_clicked(buttons[i]) {
                    report(codes[i], false)?;
                }
            }
            for i in 0..3 {
                if ui.is_mouse_released(buttons[i]) && !modes.mouse_x10 {
                    report(codes[i], true)?;
                }
            }
            if hovered && input.mouse_wheel() != 0.0 {
                report(if input.mouse_wheel() > 0.0 { 64 } else { 65 }, false)?;
            }
            if modes.mouse_motion || modes.mouse_many {
                let held = [0, 2, 1]
                    .into_iter()
                    .find(|&i| ui.is_mouse_down(buttons[i]))
                    .map(|i| codes[i])
                    .or(if modes.mouse_many { Some(3) } else { None });
                if let Some(code) = held
                    && self.last_motion != [col as i32, row as i32]
                {
                    self.input_write(
                        ui,
                        terminal,
                        io,
                        &mouse_packet(code + 32, col, row, mods, false, modes.mouse_sgr),
                        false,
                    )?;
                    self.last_motion = [col as i32, row as i32];
                    changed = true;
                }
            }
        } else {
            if hovered && ui.is_mouse_clicked(MouseButton::Left) {
                self.rectangular = input.key_alt()
                    && !input.key_ctrl()
                    && !input.key_shift()
                    && !input.key_super();
                let n = unsafe { (*sys::igGetIO_Nil()).MouseClickedCount[0] };
                terminal.select_start(
                    col,
                    row,
                    if n >= 3 {
                        SelectionSnap::Line
                    } else if n == 2 {
                        SelectionSnap::Word
                    } else {
                        SelectionSnap::None
                    },
                );
                self.selecting = true;
                changed = true;
            }
            if self.selecting && ui.is_mouse_dragging(MouseButton::Left) {
                terminal.select_extend(col, row, self.rectangular, false);
                changed = true;
            }
            if self.selecting && ui.is_mouse_released(MouseButton::Left) {
                terminal.select_extend(col, row, self.rectangular, true);
                self.selecting = false;
                changed = true;
            }
            if ui.is_mouse_released(MouseButton::Middle) {
                self.paste(ui, terminal, io)?;
                changed = true;
            }
            if hovered && input.mouse_wheel() != 0.0 {
                let shift_only = input.key_shift()
                    && !input.key_ctrl()
                    && !input.key_alt()
                    && !input.key_super();
                let bytes = wheel_bytes(input.mouse_wheel() > 0.0, shift_only);
                self.input_write(ui, terminal, io, bytes, true)?;
                changed = true;
            }
        }
        Ok(changed
            || [
                MouseButton::Left,
                MouseButton::Right,
                MouseButton::Middle,
                MouseButton::Extra1,
                MouseButton::Extra2,
            ]
            .into_iter()
            .any(|b| ui.is_mouse_down(b)))
    }
}

pub fn pixel_to_cell(
    pos: [f32; 2],
    origin: [f32; 2],
    m: PaintMetrics,
    cols: usize,
    rows: usize,
) -> [usize; 2] {
    let x = ((pos[0] - origin[0]) as i32)
        .saturating_sub(2)
        .clamp(0, (cols as f32 * m.cw) as i32 - 1);
    let y = ((pos[1] - origin[1]) as i32)
        .saturating_sub(2)
        .clamp(0, (rows as f32 * m.ch) as i32 - 1);
    [(x as f32 / m.cw) as usize, (y as f32 / m.ch) as usize]
}
fn mouse_mode_bits(m: &TerminalModes) -> u8 {
    u8::from(m.mouse_x10)
        | u8::from(m.mouse_button) << 1
        | u8::from(m.mouse_motion) << 2
        | u8::from(m.mouse_many) << 3
}
pub fn mouse_packet(
    button: u8,
    col: usize,
    row: usize,
    mods: u8,
    release: bool,
    sgr: bool,
) -> Vec<u8> {
    if sgr {
        format!(
            "\x1b[<{};{};{}{}",
            button + mods,
            col + 1,
            row + 1,
            if release { 'm' } else { 'M' }
        )
        .into_bytes()
    } else {
        vec![
            27,
            b'[',
            b'M',
            32 + (if release { 3 } else { button }) + mods,
            32 + (col + 1).clamp(1, 223) as u8,
            32 + (row + 1).clamp(1, 223) as u8,
        ]
    }
}
pub fn wheel_bytes(up: bool, shift_only: bool) -> &'static [u8] {
    match (up, shift_only) {
        (true, true) => b"\x1b[5;2~",
        (false, true) => b"\x1b[6;2~",
        (true, false) => b"\x19",
        (false, false) => b"\x05",
    }
}
pub fn character_bytes(character: char, alt: bool, eight_bit: bool) -> Vec<u8> {
    let mut bytes = [0; 4];
    if alt && character.is_ascii() {
        if eight_bit {
            char::from_u32(character as u32 | 0x80)
                .unwrap()
                .encode_utf8(&mut bytes)
                .as_bytes()
                .to_vec()
        } else {
            vec![27, character as u8]
        }
    } else {
        character.encode_utf8(&mut bytes).as_bytes().to_vec()
    }
}
pub fn key_bytes(
    mut pressed: impl FnMut(Key) -> bool,
    shift: bool,
    app_cursor: bool,
) -> Option<&'static [u8]> {
    if pressed(Key::Tab) {
        return Some(if shift { b"\x1b[Z" } else { b"\t" });
    }
    for (key, bytes) in [
        (Key::Backspace, b"\x7f".as_slice()),
        (Key::Escape, b"\x1b"),
        (Key::Enter, b"\r"),
    ] {
        if pressed(key) {
            return Some(bytes);
        }
    }
    for (key, normal, app) in [
        (Key::UpArrow, b"\x1b[A".as_slice(), b"\x1bOA".as_slice()),
        (Key::DownArrow, b"\x1b[B", b"\x1bOB"),
        (Key::RightArrow, b"\x1b[C", b"\x1bOC"),
        (Key::LeftArrow, b"\x1b[D", b"\x1bOD"),
    ] {
        if pressed(key) {
            return Some(if app_cursor { app } else { normal });
        }
    }
    if !app_cursor {
        for (key, bytes) in [(Key::Home, b"\x1b[H".as_slice()), (Key::End, b"\x1b[F")] {
            if pressed(key) {
                return Some(bytes);
            }
        }
    }
    for (key, bytes) in [
        (Key::PageUp, b"\x1b[5~".as_slice()),
        (Key::PageDown, b"\x1b[6~"),
        (Key::Insert, b"\x1b[2~"),
        (Key::Delete, b"\x1b[3~"),
        (Key::F1, b"\x1bOP"),
        (Key::F2, b"\x1bOQ"),
        (Key::F3, b"\x1bOR"),
        (Key::F4, b"\x1bOS"),
        (Key::F5, b"\x1b[15~"),
        (Key::F6, b"\x1b[17~"),
        (Key::F7, b"\x1b[18~"),
        (Key::F8, b"\x1b[19~"),
        (Key::F9, b"\x1b[20~"),
        (Key::F10, b"\x1b[21~"),
        (Key::F11, b"\x1b[23~"),
        (Key::F12, b"\x1b[24~"),
    ] {
        if pressed(key) {
            return Some(bytes);
        }
    }
    None
}
fn packed(color: [u8; 3]) -> u32 {
    u32::from(color[0]) | u32::from(color[1]) << 8 | u32::from(color[2]) << 16 | 0xff000000
}
fn replay(
    ui: &Ui,
    ops: &[DrawOp],
    origin: [f32; 2],
    fonts: &TerminalFonts,
    size: f32,
    transparent: bool,
    default_bg: [u8; 3],
) {
    // Context/frame owns this draw list; every native pointer stays within this call.
    let dl = unsafe { sys::igGetWindowDrawList() };
    let point = |p: [f32; 2]| sys::ImVec2 {
        x: p[0] + origin[0],
        y: p[1] + origin[1],
    };
    for op in ops {
        match *op {
            DrawOp::Rect { p0, p1, color } => {
                if !transparent || color != default_bg {
                    unsafe {
                        sys::ImDrawList_AddRectFilled(
                            dl,
                            point(p0),
                            point(p1),
                            packed(color),
                            0.0,
                            0,
                        );
                    }
                }
            }
            DrawOp::Text {
                pos,
                color,
                style,
                character,
            } => {
                let token = fonts
                    .for_style(style)
                    .map(|f| ui.push_font_with_size(Some(f), size));
                let font = unsafe { sys::igGetFont() };
                let mut bytes = [0; 4];
                let text = character.encode_utf8(&mut bytes);
                unsafe {
                    sys::ImDrawList_AddText_FontPtr(
                        dl,
                        font,
                        size,
                        point(pos),
                        packed(color),
                        text.as_ptr().cast(),
                        text.as_ptr().add(text.len()).cast(),
                        0.0,
                        std::ptr::null(),
                    );
                }
                drop(token);
            }
            DrawOp::PushClip { p0, p1 } => unsafe {
                sys::ImDrawList_PushClipRect(dl, point(p0), point(p1), true);
            },
            DrawOp::PopClip => unsafe {
                sys::ImDrawList_PopClipRect(dl);
            },
        }
    }
}

fn clipboard_text() -> Option<String> {
    let ptr = unsafe { sys::igGetClipboardText() };
    if ptr.is_null() {
        None
    } else {
        Some(
            unsafe { std::ffi::CStr::from_ptr(ptr) }
                .to_string_lossy()
                .into_owned(),
        )
    }
}
fn set_clipboard_text(text: &str) {
    let end = text.find('\0').unwrap_or(text.len());
    if let Ok(text) = std::ffi::CString::new(&text[..end]) {
        unsafe {
            sys::igSetClipboardText(text.as_ptr());
        }
    }
}
fn input_characters() -> Vec<char> {
    let io = unsafe { &*sys::igGetIO_Nil() };
    let queue = &io.InputQueueCharacters;
    if queue.Size <= 0 || queue.Data.is_null() {
        return Vec::new();
    }
    unsafe { std::slice::from_raw_parts(queue.Data, queue.Size as usize) }
        .iter()
        .filter_map(|&value| char::from_u32(value))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use dear_imgui_rs::{ClipboardBackend, Condition, Context, FramePrepareOptions, WindowFlags};
    use std::{cell::RefCell, path::PathBuf, rc::Rc};
    #[derive(Default)]
    struct Pipe {
        writes: Vec<Vec<u8>>,
        resizes: Vec<[usize; 2]>,
    }
    impl TerminalIo for Pipe {
        fn pump(&mut self, _: &mut Terminal) -> io::Result<bool> {
            Ok(false)
        }
        fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
            self.writes.push(bytes.to_vec());
            Ok(())
        }
        fn resize(&mut self, cols: usize, rows: usize, _: f32, _: f32) -> io::Result<()> {
            self.resizes.push([cols, rows]);
            Ok(())
        }
    }
    struct Clipboard(Rc<RefCell<String>>);
    impl ClipboardBackend for Clipboard {
        fn get(&mut self) -> Option<String> {
            Some(self.0.borrow().clone())
        }
        fn set(&mut self, text: &str) {
            *self.0.borrow_mut() = text.into();
        }
    }
    fn setup() -> (Context, Rc<RefCell<String>>) {
        let mut context = Context::create();
        context.set_ini_filename(None::<PathBuf>).unwrap();
        let clipboard = Rc::new(RefCell::new(String::new()));
        context.set_clipboard_backend(Clipboard(clipboard.clone()));
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        (context, clipboard)
    }
    fn logical_modifier(context: &mut Context, key: Key, down: bool) {
        let swap = context.io().config_macosx_behaviors();
        let key = match (key, swap) {
            (Key::ModCtrl, true) => Key::ModSuper,
            (Key::ModSuper, true) => Key::ModCtrl,
            (key, _) => key,
        };
        let physical = match key {
            Key::ModCtrl => Key::LeftCtrl,
            Key::ModSuper => Key::LeftSuper,
            Key::ModShift => Key::LeftShift,
            Key::ModAlt => Key::LeftAlt,
            _ => key,
        };
        context.io_mut().add_key_event(physical, down);
        context.io_mut().add_key_event(key, down);
    }
    fn frame(
        context: &mut Context,
        view: &mut TerminalView,
        term: &mut Terminal,
        pipe: &mut Pipe,
        focus: bool,
    ) -> [f32; 2] {
        context.prepare_frame(FramePrepareOptions::new([640.0, 480.0], 1.0 / 60.0));
        let ui = context.frame();
        let font = ui.current_font();
        let fonts = TerminalFonts {
            regular: Some(font),
            bold: Some(font),
            italic: Some(font),
            bold_italic: Some(font),
            size: 13.0,
            ..TerminalFonts::default()
        };
        let mut origin = [0.0; 2];
        if !focus {
            ui.window("Other test pane")
                .position([530.0, 20.0], Condition::Always)
                .size([100.0, 200.0], Condition::Always)
                .build(|| ui.set_window_focus(None));
        }
        ui.window("Terminal native test")
            .position([20.0, 20.0], Condition::Always)
            .size([500.0, 300.0], Condition::Always)
            .flags(
                WindowFlags::NO_TITLE_BAR
                    | WindowFlags::NO_MOVE
                    | WindowFlags::NO_SCROLLBAR
                    | WindowFlags::NO_SCROLL_WITH_MOUSE,
            )
            .build(|| {
                if focus {
                    ui.set_window_focus(None);
                    ui.set_keyboard_focus_here();
                }
                origin = ui.cursor_screen_pos();
                view.draw(ui, term, &fonts, pipe).unwrap();
            });
        drop(context.render_legacy());
        origin
    }
    #[test]
    fn source_color_pipeline_preserves_order_bold_faint_and_reverse() {
        let term = Terminal::new(2, 1);
        let palette = term.palette();
        let mut cell = term.cell(0, 0);
        cell.character = 'x';
        cell.fg = TerminalColor::Indexed(1);
        cell.mode = ATTR_BOLD;
        assert_eq!(
            glyph_colors(cell, palette, false, false, false).0,
            palette[9]
        );
        cell.mode |= ATTR_FAINT;
        assert_eq!(
            glyph_colors(cell, palette, false, false, false).0,
            palette[1]
        );
        cell.mode = ATTR_FAINT;
        assert_eq!(
            glyph_colors(cell, palette, false, false, false).0,
            palette[1].map(|v| v / 2)
        );
        cell.mode = ATTR_FAINT | ATTR_REVERSE;
        let (fg, bg) = glyph_colors(cell, palette, true, false, false);
        assert_eq!(fg, palette[DEFAULT_FG]);
        assert_eq!(bg, palette[1].map(|v| (v ^ 255) / 2));
        cell.mode = ATTR_INVISIBLE;
        let (fg, bg) = glyph_colors(cell, palette, false, false, true);
        assert_eq!(fg, bg);
        cell.mode = 0;
        assert_eq!(
            glyph_colors(cell, palette, false, false, true).0,
            palette[11]
        );
    }
    #[test]
    fn latin_only_style_slots_and_wide_dummy_skip_match_source() {
        let mut term = Terminal::new(10, 2);
        term.feed("\x1b[1;3mA中B".as_bytes());
        assert_eq!(font_style(term.cell(0, 0)), 3);
        assert_eq!(font_style(term.cell(0, 1)), 0);
        assert_eq!(term.cell(0, 2).mode, ATTR_WDUMMY);
        let m = PaintMetrics {
            cw: 8.0,
            ch: 16.0,
            ascent: 12.0,
            width: 80.0,
            height: 32.0,
        };
        let paint = render_ops(&term, m, true, true, false, [false; 4], |_, _, p| p);
        let glyphs: Vec<_> = paint.rows[0]
            .iter()
            .filter_map(|op| {
                if let DrawOp::Text {
                    character,
                    pos,
                    style,
                    ..
                } = op
                {
                    Some((*character, *pos, *style))
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(
            &glyphs[..3],
            &[
                ('A', [2.0, 2.0], 3),
                ('中', [10.0, 2.0], 0),
                ('B', [26.0, 2.0], 3)
            ]
        );
    }
    #[test]
    fn key_mouse_and_wheel_protocols_keep_source_bytes() {
        assert_eq!(
            key_bytes(|key| key == Key::UpArrow, false, true),
            Some(b"\x1bOA".as_slice())
        );
        assert_eq!(key_bytes(|key| key == Key::Home, false, true), None);
        assert_eq!(
            key_bytes(|key| key == Key::Tab, true, false),
            Some(b"\x1b[Z".as_slice())
        );
        assert_eq!(character_bytes('a', true, false), b"\x1ba");
        assert_eq!(character_bytes('a', true, true), "á".as_bytes());
        assert_eq!(character_bytes('雪', true, true), "雪".as_bytes());
        assert_eq!(
            mouse_packet(2, 500, 600, 24, true, false),
            vec![27, 91, 77, 59, 255, 255]
        );
        assert_eq!(
            mouse_packet(2, 500, 600, 24, true, true),
            b"\x1b[<26;501;601m"
        );
        assert_eq!(wheel_bytes(true, false), b"\x19");
        assert_eq!(wheel_bytes(false, false), b"\x05");
        assert_eq!(wheel_bytes(true, true), b"\x1b[5;2~");
        let m = PaintMetrics {
            cw: 8.0,
            ch: 16.0,
            ascent: 12.0,
            width: 80.0,
            height: 32.0,
        };
        assert_eq!(pixel_to_cell([-5.0, -5.0], [10.0, 10.0], m, 10, 2), [0, 0]);
        assert_eq!(
            pixel_to_cell([1000.0, 1000.0], [10.0, 10.0], m, 10, 2),
            [9, 1]
        );
    }
    #[test]
    fn blink_intervals_and_hollow_unfocused_cursor_match_source() {
        let mut term = Terminal::new(2, 1);
        term.feed(b"\x1b[1 q\x1b[5mx");
        let mut view = TerminalView::default();
        assert!(!view.tick_blink(0.749, &term));
        assert!(view.tick_blink(0.750, &term));
        assert!(!view.cursor_on);
        assert!(!view.blink);
        assert!(view.tick_blink(0.800, &term));
        assert!(view.blink);
        let m = PaintMetrics {
            cw: 8.0,
            ch: 16.0,
            ascent: 12.0,
            width: 16.0,
            height: 16.0,
        };
        assert!(
            render_ops(&term, m, true, false, true, [false; 4], |_, _, p| p)
                .overlay
                .is_empty()
        );
        assert_eq!(
            render_ops(&term, m, false, false, true, [false; 4], |_, _, p| p)
                .overlay
                .len(),
            4
        );
    }
    #[test]
    fn native_canvas_focus_routes_keyboard_before_queued_characters() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let (mut context, _) = setup();
        let mut view = TerminalView::default();
        let mut term = Terminal::new(80, 24);
        let mut pipe = Pipe::default();
        frame(&mut context, &mut view, &mut term, &mut pipe, true);
        frame(&mut context, &mut view, &mut term, &mut pipe, true);
        assert!(view.focused);
        context.io_mut().add_input_characters_utf8("雪");
        context.io_mut().add_key_event(Key::LeftArrow, true);
        frame(&mut context, &mut view, &mut term, &mut pipe, true);
        assert_eq!(pipe.writes, vec![b"\x1b[D".to_vec()]);
        context.io_mut().add_key_event(Key::LeftArrow, false);
        context.io_mut().add_input_characters_utf8("é");
        frame(&mut context, &mut view, &mut term, &mut pipe, true);
        assert_eq!(pipe.writes.last().unwrap(), "é".as_bytes());
        logical_modifier(&mut context, Key::ModCtrl, true);
        context.io_mut().add_key_event(Key::C, true);
        frame(&mut context, &mut view, &mut term, &mut pipe, true);
        assert_eq!(pipe.writes.last().unwrap(), b"\x03");
        assert!(!pipe.resizes.is_empty());
    }
    #[test]
    fn native_bracketed_paste_copy_and_keyboard_lock_obey_shortcut_routing() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let (mut context, clipboard) = setup();
        let mut view = TerminalView::default();
        let mut term = Terminal::new(80, 24);
        let mut pipe = Pipe::default();
        frame(&mut context, &mut view, &mut term, &mut pipe, true);
        frame(&mut context, &mut view, &mut term, &mut pipe, true);
        term.feed(b"\x1b[?2004h");
        *clipboard.borrow_mut() = "raw\n\x1b[31m".into();
        logical_modifier(&mut context, Key::ModCtrl, true);
        logical_modifier(&mut context, Key::ModShift, true);
        context.io_mut().add_key_event(Key::V, true);
        frame(&mut context, &mut view, &mut term, &mut pipe, true);
        assert_eq!(pipe.writes.concat(), b"\x1b[200~raw\n\x1b[31m\x1b[201~");
        context.io_mut().add_key_event(Key::V, false);
        frame(&mut context, &mut view, &mut term, &mut pipe, true);
        term.feed(b"abc");
        term.select_start(0, 0, SelectionSnap::None);
        term.select_extend(2, 0, false, false);
        term.select_extend(2, 0, false, true);
        assert_eq!(term.selection_text().as_deref(), Some("abc"));
        context.io_mut().add_key_event(Key::C, true);
        frame(&mut context, &mut view, &mut term, &mut pipe, true);
        frame(&mut context, &mut view, &mut term, &mut pipe, true);
        assert_eq!(&*clipboard.borrow(), "abc");
        context.io_mut().add_key_event(Key::C, false);
        logical_modifier(&mut context, Key::ModCtrl, false);
        logical_modifier(&mut context, Key::ModShift, false);
        frame(&mut context, &mut view, &mut term, &mut pipe, true);
        term.feed(b"\x1b[2h");
        let before = pipe.writes.len();
        context.io_mut().add_input_characters_utf8("locked");
        frame(&mut context, &mut view, &mut term, &mut pipe, true);
        assert_eq!(pipe.writes.len(), before);
    }
    #[test]
    fn native_mouse_selection_and_reporting_are_clamped_and_source_wheel_is_not_scrollback() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let (mut context, _) = setup();
        let mut view = TerminalView::default();
        let mut term = Terminal::new(80, 24);
        let mut pipe = Pipe::default();
        let origin = frame(&mut context, &mut view, &mut term, &mut pipe, true);
        frame(&mut context, &mut view, &mut term, &mut pipe, true);
        term.feed(b"hello world");
        context
            .io_mut()
            .add_mouse_pos_event([origin[0] + 3.0, origin[1] + 3.0]);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, true);
        frame(&mut context, &mut view, &mut term, &mut pipe, true);
        context
            .io_mut()
            .add_mouse_pos_event([origin[0] + view.geometry.cw * 4.0 + 3.0, origin[1] + 3.0]);
        frame(&mut context, &mut view, &mut term, &mut pipe, true);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, false);
        frame(&mut context, &mut view, &mut term, &mut pipe, true);
        assert_eq!(term.selection_text().as_deref(), Some("hello"));
        context.io_mut().add_mouse_wheel_event([0.0, 1.0]);
        frame(&mut context, &mut view, &mut term, &mut pipe, true);
        assert_eq!(pipe.writes.last().unwrap(), b"\x19");
        term.feed(b"\x1b[?1000h\x1b[?1006h");
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Right, true);
        frame(&mut context, &mut view, &mut term, &mut pipe, true);
        assert_eq!(pipe.writes.last().unwrap(), b"\x1b[<2;5;1M");
        context.io_mut().add_mouse_pos_event([1000.0, 1000.0]);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Right, false);
        frame(&mut context, &mut view, &mut term, &mut pipe, true);
        let expected = mouse_packet(2, term.cols() - 1, term.rows() - 1, 0, true, true);
        assert_eq!(pipe.writes.last().unwrap(), &expected);
    }
    #[test]
    fn native_focus_reports_change_once_and_unfocused_text_is_not_sent() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let (mut context, _) = setup();
        let mut view = TerminalView::default();
        let mut term = Terminal::new(80, 24);
        let mut pipe = Pipe::default();
        frame(&mut context, &mut view, &mut term, &mut pipe, true);
        frame(&mut context, &mut view, &mut term, &mut pipe, true);
        term.feed(b"\x1b[?1004h");
        frame(&mut context, &mut view, &mut term, &mut pipe, false);
        assert!(!view.focused);
        assert_eq!(pipe.writes, vec![b"\x1b[O".to_vec()]);
        context.io_mut().add_input_characters_utf8("off");
        frame(&mut context, &mut view, &mut term, &mut pipe, false);
        assert_eq!(pipe.writes.len(), 1);
        frame(&mut context, &mut view, &mut term, &mut pipe, true);
        frame(&mut context, &mut view, &mut term, &mut pipe, true);
        assert_eq!(pipe.writes, vec![b"\x1b[O".to_vec(), b"\x1b[I".to_vec()]);
    }
}
