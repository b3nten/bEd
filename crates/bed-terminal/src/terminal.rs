//! Terminal state adapter for the pinned ned/imgui-terminal emulator.
//!
//! Alacritty owns the grid and ANSI parser. Source-specific glyph attributes,
//! palette, screen switching, resize, selection and protocol behavior are
//! translated from terminal.{h,cpp}. Attribution: resources/terminal/LICENSE,
//! LICENSE and NOTICE. The adapter is independent of the GUI and PTY transport.
use alacritty_terminal::vte::ansi;
use alacritty_terminal::{
    event::{Event, EventListener},
    grid::{Cursor, Dimensions, Grid, Scroll},
    index::{Column, Line},
    term::{
        Config, Osc52, Term, TermMode,
        cell::{Cell, Flags},
    },
    vte::ansi::{
        Attr, CharsetIndex, ClearMode, Color, CursorShape, CursorStyle, Handler, LineClearMode,
        Mode, NamedColor, PrivateMode, Processor, Rgb, StandardCharset, TabulationClearMode,
    },
};
use bed_core::util::color::{blend, ensure_contrast};
use std::{cell::RefCell, collections::BTreeMap, rc::Rc, sync::OnceLock};
use unicode_width::UnicodeWidthChar;

pub const ATTR_BOLD: u16 = 1 << 0;
pub const ATTR_FAINT: u16 = 1 << 1;
pub const ATTR_ITALIC: u16 = 1 << 2;
pub const ATTR_UNDERLINE: u16 = 1 << 3;
pub const ATTR_BLINK: u16 = 1 << 4;
pub const ATTR_REVERSE: u16 = 1 << 5;
pub const ATTR_INVISIBLE: u16 = 1 << 6;
pub const ATTR_STRUCK: u16 = 1 << 7;
pub const ATTR_WRAP: u16 = 1 << 8;
pub const ATTR_WIDE: u16 = 1 << 9;
pub const ATTR_WDUMMY: u16 = 1 << 10;
const BLINK_FLAG: Flags = Flags::from_bits_retain(1 << 15);
const DEFAULT_FOREGROUND: usize = 258;
const DEFAULT_BACKGROUND: usize = 259;

/// Host colors for the terminal's theme-managed defaults and ANSI palette.
#[derive(Clone, Debug, PartialEq)]
pub struct TerminalTheme {
    background: [f32; 4],
    foreground: [f32; 4],
    ansi: [[f32; 4]; 16],
}

impl TerminalTheme {
    pub fn new(background: [f32; 4], foreground: [f32; 4], ansi: [[f32; 4]; 16]) -> Self {
        Self {
            background,
            foreground,
            ansi,
        }
    }

    fn palette(&self) -> [[u8; 3]; 260] {
        let mut palette = default_palette();
        let background = [
            self.background[0],
            self.background[1],
            self.background[2],
            1.0,
        ];
        // Leave a small margin for quantization to the terminal's RGB8 palette.
        let readable = |color| rgb8(ensure_contrast(color, background, 4.6), background);
        for (index, color) in self.ansi.into_iter().enumerate() {
            palette[index] = readable(color);
        }
        palette[DEFAULT_FOREGROUND] = readable(self.foreground);
        palette[DEFAULT_BACKGROUND] = rgb8(background, background);
        palette[256] = palette[DEFAULT_FOREGROUND];
        palette[257] = palette[DEFAULT_BACKGROUND];
        palette
    }
}

fn rgba(rgb: [u8; 3]) -> [f32; 4] {
    [
        rgb[0] as f32 / 255.0,
        rgb[1] as f32 / 255.0,
        rgb[2] as f32 / 255.0,
        1.0,
    ]
}

fn rgb8(color: [f32; 4], background: [f32; 4]) -> [u8; 3] {
    let rendered = blend(color, background, color[3]);
    std::array::from_fn(|index| (rendered[index].clamp(0.0, 1.0) * 255.0).round() as u8)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalColor {
    Indexed(usize),
    Rgb([u8; 3]),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalCell {
    pub character: char,
    pub mode: u16,
    pub fg: TerminalColor,
    pub bg: TerminalColor,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalCursor {
    pub row: usize,
    pub col: usize,
    /// Original cursor style: 0/1 blinking block, 2 steady block,
    /// 3/4 underline, 5/6 bar, 7 snowman.
    pub shape: u8,
    pub blinking: bool,
    pub visible: bool,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TerminalModes {
    pub app_cursor: bool,
    pub app_keypad: bool,
    pub mouse_x10: bool,
    pub mouse_button: bool,
    pub mouse_motion: bool,
    pub mouse_many: bool,
    pub mouse_sgr: bool,
    pub focus_reporting: bool,
    pub eight_bit: bool,
    pub reverse: bool,
    pub keyboard_lock: bool,
    pub bracket_paste: bool,
    pub num_lock: bool,
    pub alt_screen: bool,
    pub local_echo: bool,
    pub newline_mode: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalEvent {
    Write(Vec<u8>),
    Title(String),
    Bell,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SelectionSnap {
    #[default]
    None,
    Word,
    Line,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Coordinate {
    col: usize,
    row: usize,
}
#[derive(Clone, Debug)]
struct TerminalSelection {
    original_begin: Coordinate,
    original_end: Coordinate,
    begin: Coordinate,
    end: Coordinate,
    snap: SelectionSnap,
    rectangular: bool,
    empty: bool,
    alt: bool,
}
#[derive(Clone, Default)]
struct EventQueue(Rc<RefCell<Vec<Event>>>);
impl EventListener for EventQueue {
    fn send_event(&self, event: Event) {
        self.0.borrow_mut().push(event);
    }
}
struct Size {
    cols: usize,
    rows: usize,
}
impl Dimensions for Size {
    fn columns(&self) -> usize {
        self.cols
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn total_lines(&self) -> usize {
        self.rows
    }
}

pub struct Terminal {
    term: Term<EventQueue>,
    parser: Processor,
    events: EventQueue,
    outgoing: Vec<TerminalEvent>,
    inactive_grid: Grid<Cell>,
    saved_cursor: [Cursor<Cell>; 2],
    modes: TerminalModes,
    palette: [[u8; 3]; 260],
    theme_palette: [[u8; 3]; 260],
    palette_overrides: [bool; 260],
    faint_palette: [[u8; 3]; 260],
    theme: Option<TerminalTheme>,
    title: String,
    cursor_shape: u8,
    cursor_blinking: bool,
    selection: Option<TerminalSelection>,
    sequence: Vec<u8>,
    active_charset: CharsetIndex,
    scroll_top: usize,
    scroll_bottom: usize,
    revision: u64,
    cursor_revision: u64,
    last_char: Option<char>,
}
impl Default for Terminal {
    fn default() -> Self {
        Self::new(80, 24)
    }
}
impl Terminal {
    pub fn new(cols: usize, rows: usize) -> Self {
        let size = Size {
            cols: cols.max(1),
            rows: rows.max(1),
        };
        let events = EventQueue::default();
        let config = Config {
            scrolling_history: 0,
            semantic_escape_chars: " ".into(),
            default_cursor_style: CursorStyle {
                shape: CursorShape::Block,
                blinking: false,
            },
            kitty_keyboard: false,
            osc52: Osc52::Disabled,
            ..Default::default()
        };
        let term = Term::new(config, &size, events.clone());
        Self {
            inactive_grid: Grid::new(size.rows, size.cols, 0),
            saved_cursor: [term.grid().cursor.clone(), term.grid().cursor.clone()],
            term,
            parser: Processor::new(),
            events,
            outgoing: Vec::new(),
            modes: TerminalModes {
                num_lock: true,
                ..Default::default()
            },
            palette: default_palette(),
            theme_palette: default_palette(),
            palette_overrides: [false; 260],
            faint_palette: [[0; 3]; 260],
            theme: None,
            title: "Terminal".into(),
            cursor_shape: 2,
            cursor_blinking: false,
            selection: None,
            sequence: Vec::new(),
            active_charset: CharsetIndex::G0,
            scroll_top: 0,
            scroll_bottom: size.rows - 1,
            revision: 0,
            cursor_revision: 0,
            last_char: None,
        }
    }
    pub fn cols(&self) -> usize {
        self.term.columns()
    }
    pub fn rows(&self) -> usize {
        self.term.screen_lines()
    }
    pub fn palette(&self) -> &[[u8; 3]; 260] {
        &self.palette
    }
    /// Change host defaults without discarding colors explicitly set by OSC.
    pub fn set_theme(&mut self, theme: &TerminalTheme) -> bool {
        if self.theme.as_ref() == Some(theme) {
            return false;
        }
        self.theme_palette = theme.palette();
        self.theme = Some(theme.clone());
        for (index, color) in self.palette.iter_mut().enumerate() {
            if !self.palette_overrides[index] {
                *color = self.theme_palette[index];
            }
        }
        self.refresh_faint_palette();
        self.revision = self.revision.wrapping_add(1);
        true
    }

    #[cfg(feature = "ui")]
    pub(crate) fn faint_color(&self, color: TerminalColor) -> Option<[u8; 3]> {
        if self.theme.is_some()
            && let TerminalColor::Indexed(index) = color
            && (index < 16 || index == DEFAULT_FOREGROUND)
            && !self.palette_overrides[index]
        {
            Some(self.faint_palette[index])
        } else {
            None
        }
    }

    fn refresh_faint_palette(&mut self) {
        if self.theme.is_none() {
            return;
        }
        let background = rgba(self.palette[DEFAULT_BACKGROUND]);
        for index in (0..16).chain([DEFAULT_FOREGROUND]) {
            let dimmed = blend(rgba(self.palette[index]), background, 0.65);
            self.faint_palette[index] = rgb8(ensure_contrast(dimmed, background, 4.6), background);
        }
    }

    fn set_palette_color(&mut self, index: usize, color: [u8; 3]) {
        if index < self.palette.len() {
            self.palette[index] = color;
            self.palette_overrides[index] = true;
            self.refresh_faint_palette();
        }
    }

    fn reset_palette_color(&mut self, index: usize) {
        if index < self.palette.len() {
            self.palette[index] = self.theme_palette[index];
            self.palette_overrides[index] = false;
            self.refresh_faint_palette();
        }
    }

    fn reset_palette(&mut self) {
        self.palette = self.theme_palette;
        self.palette_overrides.fill(false);
        self.refresh_faint_palette();
    }
    pub fn title(&self) -> &str {
        &self.title
    }
    pub fn modes(&self) -> TerminalModes {
        self.modes
    }
    pub fn set_num_lock(&mut self, value: bool) {
        self.modes.num_lock = value;
        self.revision = self.revision.wrapping_add(1);
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn cursor_revision(&self) -> u64 {
        self.cursor_revision
    }
    pub fn wrap_pending(&self) -> bool {
        self.term.grid().cursor.input_needs_wrap
    }
    pub fn cursor(&self) -> TerminalCursor {
        let point = self.term.grid().cursor.point;
        TerminalCursor {
            row: point.line.0 as usize,
            col: point.column.0,
            shape: self.cursor_shape,
            blinking: self.cursor_blinking,
            visible: self.term.mode().contains(TermMode::SHOW_CURSOR),
        }
    }
    pub fn cell(&self, row: usize, col: usize) -> TerminalCell {
        let cell = &self.term.grid()[Line(row as i32)][Column(col)];
        let mut mode = 0;
        for (native, original) in [
            (Flags::BOLD, ATTR_BOLD),
            (Flags::DIM, ATTR_FAINT),
            (Flags::ITALIC, ATTR_ITALIC),
            (Flags::UNDERLINE, ATTR_UNDERLINE),
            (BLINK_FLAG, ATTR_BLINK),
            (Flags::INVERSE, ATTR_REVERSE),
            (Flags::HIDDEN, ATTR_INVISIBLE),
            (Flags::STRIKEOUT, ATTR_STRUCK),
            (Flags::WRAPLINE, ATTR_WRAP),
            (Flags::WIDE_CHAR, ATTR_WIDE),
            (Flags::WIDE_CHAR_SPACER, ATTR_WDUMMY),
        ] {
            if cell.flags.contains(native) {
                mode |= original;
            }
        }
        TerminalCell {
            character: cell.c,
            mode,
            fg: color(cell.fg),
            bg: color(cell.bg),
        }
    }
    pub fn scroll_display(&mut self, lines: i32) {
        self.term.scroll_display(Scroll::Delta(lines));
    }

    /// Parse on the main thread; all generated PTY replies are returned to the
    /// caller, which owns the transport. Partial escapes/UTF-8 survive calls.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<TerminalEvent> {
        if !bytes.is_empty() {
            self.revision = self.revision.wrapping_add(1);
        }
        let mut offset = 0;
        while offset < bytes.len() {
            if self.sequence.is_empty() {
                let remaining = &bytes[offset..];
                let plain = remaining
                    .iter()
                    .position(|&byte| byte == 0x1b)
                    .unwrap_or(remaining.len());
                if plain > 0 {
                    self.advance_plain(&remaining[..plain]);
                    offset += plain;
                }
                if offset == bytes.len() {
                    break;
                }
                self.sequence.push(0x1b);
                offset += 1;
                continue;
            }
            let byte = bytes[offset];
            offset += 1;
            self.sequence.push(byte);
            let done = match self.sequence.get(1) {
                Some(b'[') => self.sequence.len() > 2 && (0x40..=0x7e).contains(&byte),
                Some(b']' | b'P' | b'^' | b'_' | b'k') => {
                    byte == 7 || self.sequence.ends_with(b"\x1b\\")
                }
                Some(b'(' | b')' | b'*' | b'+' | b'%' | b'#') => self.sequence.len() >= 3,
                Some(_) => true,
                None => false,
            };
            if done {
                let sequence = std::mem::take(&mut self.sequence);
                if !self.compat_sequence(&sequence) {
                    self.advance(&sequence);
                }
            }
        }
        let pending = std::mem::take(&mut *self.events.0.borrow_mut());
        for event in pending {
            match event {
                Event::PtyWrite(text) => {
                    self.outgoing.push(TerminalEvent::Write(text.into_bytes()))
                }
                Event::Title(title) => {
                    self.title = title.clone();
                    self.outgoing.push(TerminalEvent::Title(title));
                }
                Event::ResetTitle => {
                    self.title = "Terminal".into();
                    self.outgoing.push(TerminalEvent::Title(self.title.clone()));
                }
                Event::Bell => self.outgoing.push(TerminalEvent::Bell),
                _ => {}
            }
        }
        std::mem::take(&mut self.outgoing)
    }
    /// Source ttywrite's local-echo path displays C0 controls as caret text;
    /// newline, carriage return and tab retain their terminal behavior.
    pub fn feed_echo(&mut self, bytes: &[u8]) -> Vec<TerminalEvent> {
        let mut visible = String::new();
        for mut character in String::from_utf8_lossy(bytes).chars() {
            let code = character as u32;
            if code <= 0x1f || (0x7f..=0x9f).contains(&code) {
                if code & 0x80 != 0 {
                    visible.push_str("^[");
                    character = char::from_u32(code & 0x7f).unwrap();
                } else if !matches!(character, '\n' | '\r' | '\t') {
                    visible.push('^');
                    character = char::from_u32(code ^ 0x40).unwrap();
                }
            }
            visible.push(character);
        }
        self.feed(visible.as_bytes())
    }
    fn advance(&mut self, bytes: &[u8]) {
        let mut parser = std::mem::take(&mut self.parser);
        parser.advance(self, bytes);
        self.parser = parser;
    }
    fn advance_plain(&mut self, mut bytes: &[u8]) {
        while !bytes.is_empty() {
            let plain = bytes
                .iter()
                .position(|&byte| byte < 0x20 || byte == 0x7f)
                .unwrap_or(bytes.len());
            if plain > 0 {
                self.advance(&bytes[..plain]);
            }
            bytes = &bytes[plain..];
            if let Some((&control, rest)) = bytes.split_first() {
                self.last_char = None;
                self.advance(&[control]);
                bytes = rest;
            }
        }
    }
    fn compat_sequence(&mut self, sequence: &[u8]) -> bool {
        if sequence.starts_with(b"\x1b]") || sequence.starts_with(b"\x1bk") {
            return self.osc(sequence);
        }
        if sequence.starts_with(b"\x1b[") {
            let body = &sequence[2..sequence.len() - 1];
            let final_byte = sequence[sequence.len() - 1];
            if final_byte == b'b' {
                let count = std::str::from_utf8(body)
                    .unwrap_or("")
                    .parse::<usize>()
                    .unwrap_or(1)
                    .max(1);
                if let Some(character) = self.last_char {
                    for _ in 0..count {
                        self.input(character);
                    }
                }
                return true;
            }
            if body.ends_with(b" ") && final_byte == b'q' {
                let argument = std::str::from_utf8(&body[..body.len() - 1]).unwrap_or("");
                let shape = if argument.is_empty() {
                    Ok(0)
                } else {
                    argument.parse::<u8>()
                };
                if let Ok(shape) = shape
                    && shape <= 7
                {
                    self.cursor_shape = shape;
                    self.cursor_blinking = matches!(shape, 0 | 1 | 3 | 5);
                    self.cursor_revision = self.cursor_revision.wrapping_add(1);
                }
                return true;
            }
            if matches!(final_byte, b'h' | b'l') {
                let private = body.first() == Some(&b'?');
                let parameters = if private { &body[1..] } else { body };
                for number in std::str::from_utf8(parameters).unwrap_or("").split(';') {
                    let number = number.parse::<u16>().unwrap_or(0);
                    self.source_mode(private, final_byte == b'h', number);
                }
                return true;
            }
        }
        false
    }
    fn source_mode(&mut self, private: bool, set: bool, number: u16) {
        if private {
            match number {
                1 => self.modes.app_cursor = set,
                5 => self.modes.reverse = set,
                9 | 1000 | 1002 | 1003 => {
                    self.modes.mouse_x10 = set && number == 9;
                    self.modes.mouse_button = set && number == 1000;
                    self.modes.mouse_motion = set && number == 1002;
                    self.modes.mouse_many = set && number == 1003;
                }
                12 => {
                    self.cursor_blinking = set;
                    self.cursor_revision = self.cursor_revision.wrapping_add(1);
                }
                1004 => self.modes.focus_reporting = set,
                1006 => self.modes.mouse_sgr = set,
                1034 => self.modes.eight_bit = set,
                47 | 1047 | 1049 => {
                    if number == 1049 {
                        if set {
                            self.save_cursor_position();
                        } else {
                            self.restore_cursor_position();
                        }
                    }
                    if self.modes.alt_screen {
                        self.clear_screen(ClearMode::All);
                    }
                    if set != self.modes.alt_screen {
                        self.swap_screens();
                    }
                    if number == 1049 {
                        if set {
                            self.save_cursor_position();
                        } else {
                            self.restore_cursor_position();
                        }
                    }
                    return;
                }
                1048 => {
                    if set {
                        self.save_cursor_position();
                    } else {
                        self.restore_cursor_position();
                    }
                    return;
                }
                2004 => self.modes.bracket_paste = set,
                6 | 7 | 25 => {}
                _ => return, // Pinned source deliberately ignores other private modes.
            }
            let native = PrivateMode::Named(match number {
                1 => ansi::NamedPrivateMode::CursorKeys,
                6 => ansi::NamedPrivateMode::Origin,
                7 => ansi::NamedPrivateMode::LineWrap,
                12 => ansi::NamedPrivateMode::BlinkingCursor,
                25 => ansi::NamedPrivateMode::ShowCursor,
                1000 => ansi::NamedPrivateMode::ReportMouseClicks,
                1002 => ansi::NamedPrivateMode::ReportCellMouseMotion,
                1003 => ansi::NamedPrivateMode::ReportAllMouseMotion,
                1004 => ansi::NamedPrivateMode::ReportFocusInOut,
                1006 => ansi::NamedPrivateMode::SgrMouse,
                2004 => ansi::NamedPrivateMode::BracketedPaste,
                _ => return,
            });
            if set {
                self.term.set_private_mode(native);
            } else {
                self.term.unset_private_mode(native);
            }
        } else {
            match number {
                2 => self.modes.keyboard_lock = set,
                12 => self.modes.local_echo = !set,
                20 => self.modes.newline_mode = set,
                4 => {}
                _ => return,
            }
            let native = Mode::Named(match number {
                4 => ansi::NamedMode::Insert,
                20 => ansi::NamedMode::LineFeedNewLine,
                _ => return,
            });
            if set {
                self.term.set_mode(native);
            } else {
                self.term.unset_mode(native);
            }
        }
    }
    fn swap_screens(&mut self) {
        let cursor = self.term.grid().cursor.clone();
        let current = self.term.grid().clone();
        self.term.swap_alt();
        *self.term.grid_mut() = std::mem::replace(&mut self.inactive_grid, current);
        self.term.grid_mut().cursor = cursor;
        self.modes.alt_screen = !self.modes.alt_screen;
    }
    /// Width changes truncate cells rather than reflowing the source screen.
    /// Both grids slide by the active cursor's overflow on a height shrink.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        if cols == 0 || rows == 0 || (cols == self.cols() && rows == self.rows()) {
            return;
        }
        let offset = (self.term.grid().cursor.point.line.0 as usize + 1).saturating_sub(rows);
        self.revision = self.revision.wrapping_add(1);
        let active = resized_grid(self.term.grid(), cols, rows, offset);
        let inactive = resized_grid(&self.inactive_grid, cols, rows, offset);
        self.term.resize(Size { cols, rows });
        *self.term.grid_mut() = active;
        self.inactive_grid = inactive;
        self.scroll_top = 0;
        self.scroll_bottom = rows - 1;
        self.clear_selection();
    }
    fn clear_region(&mut self, left: usize, top: usize, right: usize, bottom: usize) {
        let cursor = self.term.grid().cursor.template.clone();
        let blank = Cell {
            c: ' ',
            fg: cursor.fg,
            bg: cursor.bg,
            flags: Flags::empty(),
            extra: None,
        };
        for row in top..=bottom.min(self.rows() - 1) {
            for col in left..=right.min(self.cols() - 1) {
                if self.is_selected(col, row) {
                    self.clear_selection();
                }
                self.term.grid_mut()[Line(row as i32)][Column(col)] = blank.clone();
            }
        }
    }
    fn osc(&mut self, sequence: &[u8]) -> bool {
        let end = sequence.len() - if sequence.ends_with(b"\x1b\\") { 2 } else { 1 };
        let content = String::from_utf8_lossy(&sequence[2..end]);
        let mut fields = content.split(';');
        if sequence[1] == b'k' {
            self.set_title(Some(fields.next().unwrap_or("").into()));
            return true;
        }
        let code = fields.next().unwrap_or("").parse::<i32>().unwrap_or(0);
        match code {
            0 | 2 => {
                if let Some(title) = fields.next() {
                    self.set_title(Some(title.into()));
                }
            }
            4 => {
                let index = fields.next().unwrap_or("").parse::<usize>().unwrap_or(0);
                if let Some(spec) = fields.next() {
                    self.osc_color(4, index, spec, true);
                }
            }
            10..=12 => {
                let index = [258, 259, 256][(code - 10) as usize];
                if let Some(spec) = fields.next() {
                    self.osc_color(code, index, spec, false);
                }
            }
            104 => {
                if let Some(index) = fields.next() {
                    if let Ok(index) = index.parse::<usize>()
                        && index < 260
                    {
                        self.reset_palette_color(index);
                    }
                } else {
                    self.reset_palette();
                }
            }
            110..=112 => {
                let index = [258, 259, 256][(code - 110) as usize];
                self.reset_palette_color(index);
            }
            _ => {} // Includes source's disabled OSC52 and ignored DCS/APC/PM.
        }
        true
    }
    fn osc_color(&mut self, code: i32, index: usize, spec: &str, indexed: bool) {
        if index >= self.palette.len() {
            return;
        }
        if spec == "?" {
            let [r, g, b] = self.palette[index];
            let response = if indexed {
                format!(
                    "\x1b]4;{index};rgb:{:04x}/{:04x}/{:04x}\x07",
                    u16::from(r) * 257,
                    u16::from(g) * 257,
                    u16::from(b) * 257
                )
            } else {
                format!(
                    "\x1b]{code};rgb:{:04x}/{:04x}/{:04x}\x07",
                    u16::from(r) * 257,
                    u16::from(g) * 257,
                    u16::from(b) * 257
                )
            };
            self.outgoing
                .push(TerminalEvent::Write(response.into_bytes()));
        } else if let Some(rgb) = parse_color(spec) {
            self.set_palette_color(index, rgb);
        }
    }
    pub fn clear_selection(&mut self) {
        self.selection = None;
        self.revision = self.revision.wrapping_add(1);
    }
    pub fn select_start(&mut self, col: usize, row: usize, snap: SelectionSnap) {
        self.revision = self.revision.wrapping_add(1);
        let at = Coordinate {
            col: col.min(self.cols() - 1),
            row: row.min(self.rows() - 1),
        };
        self.selection = Some(TerminalSelection {
            original_begin: at,
            original_end: at,
            begin: at,
            end: at,
            snap,
            rectangular: false,
            empty: snap == SelectionSnap::None,
            alt: self.modes.alt_screen,
        });
        self.normalize_selection();
    }
    pub fn select_extend(&mut self, col: usize, row: usize, rectangular: bool, done: bool) {
        self.revision = self.revision.wrapping_add(1);
        let Some(selection) = &mut self.selection else {
            return;
        };
        if done && selection.empty {
            self.clear_selection();
            return;
        }
        selection.original_end = Coordinate {
            col: col.min(self.term.columns() - 1),
            row: row.min(self.term.screen_lines() - 1),
        };
        self.normalize_selection();
        if let Some(selection) = &mut self.selection {
            // The source normalizes before changing the selection type.
            selection.rectangular = rectangular;
            selection.empty = false;
        }
    }
    fn line_length(&self, row: usize) -> usize {
        if self.cell(row, self.cols() - 1).mode & ATTR_WRAP != 0 {
            return self.cols();
        }
        let mut length = self.cols();
        while length > 0 && self.cell(row, length - 1).character == ' ' {
            length -= 1;
        }
        length
    }
    fn normalize_selection(&mut self) {
        let Some(mut selection) = self.selection.take() else {
            return;
        };
        let a = selection.original_begin;
        let b = selection.original_end;
        if !selection.rectangular && a.row != b.row {
            selection.begin.col = if a.row < b.row { a.col } else { b.col };
            selection.end.col = if a.row < b.row { b.col } else { a.col };
        } else {
            selection.begin.col = a.col.min(b.col);
            selection.end.col = a.col.max(b.col);
        }
        selection.begin.row = a.row.min(b.row);
        selection.end.row = a.row.max(b.row);
        self.snap_selection(&mut selection.begin, selection.snap, -1);
        self.snap_selection(&mut selection.end, selection.snap, 1);
        if !selection.rectangular {
            let length = self.line_length(selection.begin.row);
            if length < selection.begin.col {
                selection.begin.col = length;
            }
            if self.line_length(selection.end.row) <= selection.end.col {
                selection.end.col = self.cols() - 1;
            }
        }
        self.selection = Some(selection);
    }
    fn scroll_selection(&mut self, origin: usize, delta: i32) {
        let Some(selection) = &self.selection else {
            return;
        };
        if selection.alt != self.modes.alt_screen {
            return;
        }
        let begin_inside = (origin..=self.scroll_bottom).contains(&selection.begin.row);
        let end_inside = (origin..=self.scroll_bottom).contains(&selection.end.row);
        if begin_inside != end_inside {
            self.clear_selection();
        } else if begin_inside {
            let begin = selection.original_begin.row as i32 + delta;
            let end = selection.original_end.row as i32 + delta;
            if begin < self.scroll_top as i32
                || begin > self.scroll_bottom as i32
                || end < self.scroll_top as i32
                || end > self.scroll_bottom as i32
            {
                self.clear_selection();
            } else if let Some(selection) = &mut self.selection {
                selection.original_begin.row = begin as usize;
                selection.original_end.row = end as usize;
                self.normalize_selection();
            }
        }
    }
    fn snap_selection(&self, at: &mut Coordinate, snap: SelectionSnap, direction: i32) {
        match snap {
            SelectionSnap::None => {}
            SelectionSnap::Line => {
                at.col = if direction < 0 { 0 } else { self.cols() - 1 };
                if direction < 0 {
                    while at.row > 0 && self.cell(at.row - 1, self.cols() - 1).mode & ATTR_WRAP != 0
                    {
                        at.row -= 1;
                    }
                } else {
                    while at.row + 1 < self.rows()
                        && self.cell(at.row, self.cols() - 1).mode & ATTR_WRAP != 0
                    {
                        at.row += 1;
                    }
                }
            }
            SelectionSnap::Word => {
                let mut previous = self.cell(at.row, at.col);
                let mut delimiter = previous.character == ' ';
                loop {
                    let mut x = at.col as i32 + direction;
                    let mut y = at.row as i32;
                    if x < 0 || x >= self.cols() as i32 {
                        y += direction;
                        x = (x + self.cols() as i32) % self.cols() as i32;
                        if y < 0 || y >= self.rows() as i32 {
                            break;
                        }
                        let (wrap_y, wrap_x) = if direction > 0 {
                            (at.row, at.col)
                        } else {
                            (y as usize, x as usize)
                        };
                        if self.cell(wrap_y, wrap_x).mode & ATTR_WRAP == 0 {
                            break;
                        }
                    }
                    if x as usize >= self.line_length(y as usize) {
                        break;
                    }
                    let cell = self.cell(y as usize, x as usize);
                    let next_delimiter = cell.character == ' ';
                    if cell.mode & ATTR_WDUMMY == 0
                        && (delimiter != next_delimiter
                            || (next_delimiter && cell.character != previous.character))
                    {
                        break;
                    }
                    at.col = x as usize;
                    at.row = y as usize;
                    previous = cell;
                    delimiter = next_delimiter;
                }
            }
        }
    }
    pub fn is_selected(&self, col: usize, row: usize) -> bool {
        let Some(selection) = &self.selection else {
            return false;
        };
        if selection.empty || selection.alt != self.modes.alt_screen {
            return false;
        }
        row >= selection.begin.row
            && row <= selection.end.row
            && if selection.rectangular {
                col >= selection.begin.col && col <= selection.end.col
            } else {
                (row != selection.begin.row || col >= selection.begin.col)
                    && (row != selection.end.row || col <= selection.end.col)
            }
    }
    pub fn selection_text(&self) -> Option<String> {
        let selection = self.selection.as_ref()?;
        let mut result = String::new();
        for row in selection.begin.row..=selection.end.row {
            let length = self.line_length(row);
            if length == 0 {
                result.push('\n');
                continue;
            }
            let first = if selection.rectangular || row == selection.begin.row {
                selection.begin.col
            } else {
                0
            };
            let last_x = if selection.rectangular || row == selection.end.row {
                selection.end.col
            } else {
                self.cols() - 1
            };
            let mut end = (last_x + 1).min(length);
            while end > first && self.cell(row, end - 1).character == ' ' {
                end -= 1;
            }
            for col in first..end {
                let cell = self.cell(row, col);
                if cell.mode & ATTR_WDUMMY == 0 {
                    result.push(cell.character);
                }
            }
            // Source uses the last retained glyph's wrap flag (including when
            // trailing spaces were trimmed), then emits LF for a real break.
            let wrapped = end
                .checked_sub(1)
                .is_some_and(|last| self.cell(row, last).mode & ATTR_WRAP != 0);
            if (row < selection.end.row || last_x >= length) && (!wrapped || selection.rectangular)
            {
                result.push('\n');
            }
        }
        Some(result)
    }
}

fn color(color: Color) -> TerminalColor {
    match color {
        Color::Indexed(index) => TerminalColor::Indexed(index.into()),
        Color::Spec(Rgb { r, g, b }) => TerminalColor::Rgb([r, g, b]),
        Color::Named(named) => TerminalColor::Indexed(match named {
            NamedColor::Foreground | NamedColor::BrightForeground | NamedColor::DimForeground => {
                258
            }
            NamedColor::Background => 259,
            NamedColor::Cursor => 256,
            named => named as usize,
        }),
    }
}
fn resized_grid(old: &Grid<Cell>, cols: usize, rows: usize, offset: usize) -> Grid<Cell> {
    let mut result = Grid::new(rows, cols, 0);
    let blank = Cell {
        c: ' ',
        fg: old.cursor.template.fg,
        bg: old.cursor.template.bg,
        flags: Flags::empty(),
        extra: None,
    };
    for row in 0..rows {
        for col in 0..cols {
            result[Line(row as i32)][Column(col)] =
                if row + offset < old.screen_lines() && col < old.columns() {
                    old[Line((row + offset) as i32)][Column(col)].clone()
                } else {
                    blank.clone()
                };
        }
    }
    result.cursor = old.cursor.clone();
    result.cursor.point.line = Line(result.cursor.point.line.0.min(rows as i32 - 1));
    result.cursor.point.column = Column(result.cursor.point.column.0.min(cols - 1));
    result.cursor.input_needs_wrap = false;
    result.saved_cursor = old.saved_cursor.clone();
    result
}
fn default_palette() -> [[u8; 3]; 260] {
    let mut palette = [[0; 3]; 260];
    let names = [
        "black", "red3", "green3", "yellow3", "blue2", "magenta3", "cyan3", "gray90", "gray50",
        "red", "green", "yellow", "#5c5cff", "magenta", "cyan", "white",
    ];
    for (index, name) in names.into_iter().enumerate() {
        palette[index] = parse_color(name).unwrap();
    }
    for (index, rgb) in palette.iter_mut().enumerate().take(232).skip(16) {
        let channel = |value| {
            if value == 0 {
                0
            } else {
                ((0x3737 + 0x2828 * value) >> 8) as u8
            }
        };
        *rgb = [
            channel(((index - 16) / 36) % 6),
            channel(((index - 16) / 6) % 6),
            channel((index - 16) % 6),
        ];
    }
    for (index, rgb) in palette.iter_mut().enumerate().take(256).skip(232) {
        *rgb = [((0x0808 + 0x0a0a * (index - 232)) >> 8) as u8; 3];
    }
    for (index, name) in ["#cccccc", "#555555", "gray90", "black"]
        .into_iter()
        .enumerate()
    {
        palette[index + 256] = parse_color(name).unwrap();
    }
    palette
}
/// Exact upstream X11 name normalization and 16-bit hex-component scaling.
pub fn parse_color(spec: &str) -> Option<[u8; 3]> {
    fn component(value: &str) -> Option<u8> {
        if value.is_empty() || value.len() > 4 {
            return None;
        }
        let number = u32::from_str_radix(value, 16).ok()?;
        let max = (1_u32 << (value.len() * 4)) - 1;
        Some(((number * 0xffff / max) >> 8) as u8)
    }
    if let Some(hex) = spec.strip_prefix('#') {
        if !matches!(hex.len(), 3 | 6 | 12) {
            return None;
        }
        let digits = hex.len() / 3;
        return Some([
            component(hex.get(..digits)?)?,
            component(hex.get(digits..digits * 2)?)?,
            component(hex.get(digits * 2..)?)?,
        ]);
    }
    if let Some(rgb) = spec.strip_prefix("rgb:") {
        let mut parts = rgb.split('/');
        let result = [
            component(parts.next()?)?,
            component(parts.next()?)?,
            component(parts.next()?)?,
        ];
        return parts.next().is_none().then_some(result);
    }
    static COLORS: OnceLock<BTreeMap<String, [u8; 3]>> = OnceLock::new();
    let colors = COLORS.get_or_init(|| {
        let mut colors = BTreeMap::new();
        for line in include_str!("../../../resources/terminal/rgb.txt").lines() {
            let mut fields = line.split_whitespace();
            let Some(r) = fields.next().and_then(|v| v.parse::<u8>().ok()) else {
                continue;
            };
            let Some(g) = fields.next().and_then(|v| v.parse::<u8>().ok()) else {
                continue;
            };
            let Some(b) = fields.next().and_then(|v| v.parse::<u8>().ok()) else {
                continue;
            };
            colors
                .entry(fields.collect::<String>().to_ascii_lowercase())
                .or_insert([r, g, b]);
        }
        colors
    });
    let canonical = spec
        .chars()
        .filter(|c| !c.is_ascii_whitespace())
        .collect::<String>()
        .to_ascii_lowercase();
    colors.get(&canonical).copied()
}

impl Handler for Terminal {
    fn input(&mut self, mut character: char) {
        if ('\u{80}'..='\u{9f}').contains(&character) {
            return;
        }
        let point = self.term.grid().cursor.point;
        if self.is_selected(point.column.0, point.line.0 as usize) {
            self.clear_selection();
        }
        if matches!(character, '\u{fe0e}' | '\u{fe0f}') {
            if character == '\u{fe0f}' {
                let mut col =
                    point.column.0 as i32 - i32::from(!self.term.grid().cursor.input_needs_wrap);
                if col >= 0
                    && self.term.grid()[point.line][Column(col as usize)]
                        .flags
                        .contains(Flags::WIDE_CHAR_SPACER)
                {
                    col -= 1;
                }
                if col >= 0
                    && col as usize + 1 < self.cols()
                    && !self.term.grid()[point.line][Column(col as usize)]
                        .flags
                        .contains(Flags::WIDE_CHAR)
                {
                    self.term.grid_mut()[point.line][Column(col as usize)]
                        .flags
                        .insert(Flags::WIDE_CHAR);
                    let dummy = &mut self.term.grid_mut()[point.line][Column(col as usize + 1)];
                    dummy.c = '\0';
                    dummy.flags = Flags::WIDE_CHAR_SPACER;
                    dummy.extra = None;
                    if !self.term.grid().cursor.input_needs_wrap {
                        if col as usize + 2 < self.cols() {
                            self.term.grid_mut().cursor.point.column = Column(col as usize + 2);
                        } else {
                            self.term.grid_mut().cursor.input_needs_wrap = true;
                        }
                    }
                }
            }
            return;
        }
        self.last_char = Some(character);
        let width = if is_emoji_presentation(character) {
            2
        } else {
            character.width().unwrap_or(1)
        };
        if self.term.mode().contains(TermMode::LINE_WRAP)
            && self.term.grid().cursor.input_needs_wrap
        {
            self.term.grid_mut()[point].flags.insert(Flags::WRAPLINE);
            self.linefeed();
            self.term.carriage_return();
        }
        if self.term.mode().contains(TermMode::INSERT)
            && self.term.grid().cursor.point.column.0 + width < self.cols()
        {
            self.term.insert_blank(width);
            let point = self.term.grid().cursor.point;
            self.term.grid_mut()[point].flags.remove(Flags::WIDE_CHAR);
        }
        if self.term.grid().cursor.point.column.0 + width > self.cols() {
            if self.term.mode().contains(TermMode::LINE_WRAP) {
                self.linefeed();
                self.term.carriage_return();
            } else {
                self.term.grid_mut().cursor.point.column =
                    Column(self.cols().saturating_sub(width));
                self.term.grid_mut().cursor.input_needs_wrap = false;
            }
        }
        let point = self.term.grid().cursor.point;
        let flags = self.term.grid()[point].flags;
        if flags.contains(Flags::WIDE_CHAR) && point.column.0 + 1 < self.cols() {
            let following = &mut self.term.grid_mut()[point.line][point.column + 1];
            following.c = ' ';
            following.flags.remove(Flags::WIDE_CHAR_SPACER);
        } else if flags.contains(Flags::WIDE_CHAR_SPACER) && point.column.0 > 0 {
            let previous = &mut self.term.grid_mut()[point.line][point.column - 1];
            previous.c = ' ';
            previous.flags.remove(Flags::WIDE_CHAR);
        }
        if self.term.grid().cursor.charsets[self.active_charset]
            == StandardCharset::SpecialCharacterAndLineDrawing
        {
            character = match character {
                'A' => '↑',
                'B' => '↓',
                'C' => '→',
                'D' => '←',
                'E' => '█',
                'F' => '▚',
                'G' => '☃',
                c => StandardCharset::SpecialCharacterAndLineDrawing.map(c),
            };
        }
        let mut cell = self.term.grid().cursor.template.clone();
        cell.c = character;
        cell.extra = None;
        if width == 2 {
            cell.flags.insert(Flags::WIDE_CHAR);
        }
        self.term.grid_mut()[point] = cell;
        if width == 2 && point.column.0 + 1 < self.cols() {
            if self.term.grid()[point.line][point.column + 1]
                .flags
                .contains(Flags::WIDE_CHAR)
                && point.column.0 + 2 < self.cols()
            {
                let following = &mut self.term.grid_mut()[point.line][point.column + 2];
                following.c = ' ';
                following.flags.remove(Flags::WIDE_CHAR_SPACER);
            }
            let dummy = &mut self.term.grid_mut()[point.line][point.column + 1];
            dummy.c = '\0';
            dummy.flags = Flags::WIDE_CHAR_SPACER;
            dummy.extra = None;
        }
        if point.column.0 + width < self.cols() {
            self.term.grid_mut().cursor.point.column = Column(point.column.0 + width);
            self.term.grid_mut().cursor.input_needs_wrap = false;
        } else {
            self.term.grid_mut().cursor.input_needs_wrap = true;
        }
    }
    fn set_title(&mut self, title: Option<String>) {
        let title = title.unwrap_or_else(|| "Terminal".into());
        self.title =
            String::from_utf8_lossy(&title.as_bytes()[..title.len().min(255)]).into_owned();
        self.outgoing.push(TerminalEvent::Title(self.title.clone()));
    }
    fn set_cursor_style(&mut self, style: Option<CursorStyle>) {
        let style = style.unwrap_or(CursorStyle {
            shape: CursorShape::Block,
            blinking: false,
        });
        self.cursor_shape = match style.shape {
            CursorShape::Block | CursorShape::HollowBlock => 2,
            CursorShape::Underline => 4,
            CursorShape::Beam => 6,
            CursorShape::Hidden => 2,
        } - u8::from(style.blinking);
        self.cursor_blinking = style.blinking;
    }
    fn set_cursor_shape(&mut self, shape: CursorShape) {
        self.set_cursor_style(Some(CursorStyle {
            shape,
            blinking: self.cursor_blinking,
        }));
    }
    fn terminal_attribute(&mut self, attr: Attr) {
        match attr {
            Attr::BlinkSlow | Attr::BlinkFast => self
                .term
                .grid_mut()
                .cursor
                .template
                .flags
                .insert(BLINK_FLAG),
            Attr::CancelBlink => self
                .term
                .grid_mut()
                .cursor
                .template
                .flags
                .remove(BLINK_FLAG),
            Attr::Reset => self.term.grid_mut().cursor.template = Cell::default(),
            Attr::DoubleUnderline
            | Attr::Undercurl
            | Attr::DottedUnderline
            | Attr::DashedUnderline
            | Attr::UnderlineColor(_)
            | Attr::CancelBold => {}
            attr => self.term.terminal_attribute(attr),
        }
    }
    fn set_private_mode(&mut self, mode: PrivateMode) {
        self.source_mode(true, true, mode.raw());
    }
    fn unset_private_mode(&mut self, mode: PrivateMode) {
        self.source_mode(true, false, mode.raw());
    }
    fn set_mode(&mut self, mode: Mode) {
        self.source_mode(false, true, mode.raw());
    }
    fn unset_mode(&mut self, mode: Mode) {
        self.source_mode(false, false, mode.raw());
    }
    fn set_color(&mut self, index: usize, rgb: Rgb) {
        self.set_palette_color(index, [rgb.r, rgb.g, rgb.b]);
    }
    fn reset_color(&mut self, index: usize) {
        self.reset_palette_color(index);
    }
    fn dynamic_color_sequence(&mut self, prefix: String, index: usize, _: &str) {
        let native = if index == NamedColor::Foreground as usize {
            258
        } else if index == NamedColor::Background as usize {
            259
        } else if index == NamedColor::Cursor as usize {
            256
        } else {
            index
        };
        let code = prefix.parse::<i32>().unwrap_or(4);
        self.osc_color(code, native, "?", code == 4);
    }
    fn save_cursor_position(&mut self) {
        self.saved_cursor[usize::from(self.modes.alt_screen)] = self.term.grid().cursor.clone();
    }
    fn restore_cursor_position(&mut self) {
        let charsets = self.term.grid().cursor.charsets;
        let mut cursor = self.saved_cursor[usize::from(self.modes.alt_screen)].clone();
        cursor.charsets = charsets;
        cursor.point.column = Column(cursor.point.column.0.min(self.cols() - 1));
        cursor.point.line = Line(cursor.point.line.0.max(0).min(self.rows() as i32 - 1));
        cursor.input_needs_wrap = false;
        self.term.grid_mut().cursor = cursor;
    }
    fn reset_state(&mut self) {
        // Source resets terminal/core state, colors, title, hide and paste;
        // its other window modes remain set across RIS.
        let modes = self.modes;
        let cols = self.cols();
        let rows = self.rows();
        self.term.reset_state();
        self.inactive_grid = Grid::new(rows, cols, 0);
        self.saved_cursor = [Cursor::default(), Cursor::default()];
        self.modes = TerminalModes {
            alt_screen: false,
            bracket_paste: false,
            newline_mode: false,
            local_echo: false,
            ..modes
        };
        self.active_charset = CharsetIndex::G0;
        self.reset_palette();
        self.set_title(None);
        self.clear_selection();
        self.scroll_top = 0;
        self.scroll_bottom = rows - 1;
    }
    fn clear_line(&mut self, mode: LineClearMode) {
        let cursor = self.term.grid().cursor.point;
        let (left, right) = match mode {
            LineClearMode::Right => (cursor.column.0, self.cols() - 1),
            LineClearMode::Left => (0, cursor.column.0),
            LineClearMode::All => (0, self.cols() - 1),
        };
        self.clear_region(left, cursor.line.0 as usize, right, cursor.line.0 as usize);
    }
    fn clear_screen(&mut self, mode: ClearMode) {
        let cursor = self.term.grid().cursor.point;
        match mode {
            ClearMode::All => self.clear_region(0, 0, self.cols() - 1, self.rows() - 1),
            ClearMode::Below => {
                self.clear_region(
                    cursor.column.0,
                    cursor.line.0 as usize,
                    self.cols() - 1,
                    cursor.line.0 as usize,
                );
                if cursor.line.0 as usize + 1 < self.rows() {
                    self.clear_region(
                        0,
                        cursor.line.0 as usize + 1,
                        self.cols() - 1,
                        self.rows() - 1,
                    );
                }
            }
            ClearMode::Above => {
                self.clear_region(
                    0,
                    cursor.line.0 as usize,
                    cursor.column.0,
                    cursor.line.0 as usize,
                );
                if cursor.line.0 > 0 {
                    self.clear_region(0, 0, self.cols() - 1, cursor.line.0 as usize - 1);
                }
            }
            ClearMode::Saved => {}
        }
    }
    fn erase_chars(&mut self, count: usize) {
        let cursor = self.term.grid().cursor.point;
        if count > 0 {
            self.clear_region(
                cursor.column.0,
                cursor.line.0 as usize,
                (cursor.column.0 + count - 1).min(self.cols() - 1),
                cursor.line.0 as usize,
            );
        }
    }
    fn configure_charset(&mut self, index: CharsetIndex, charset: StandardCharset) {
        self.term.configure_charset(index, charset);
    }
    fn set_active_charset(&mut self, index: CharsetIndex) {
        self.active_charset = index;
        self.term.set_active_charset(index);
    }
    fn set_scrolling_region(&mut self, top: usize, bottom: Option<usize>) {
        let top = top.saturating_sub(1).min(self.rows() - 1);
        let bottom = bottom
            .unwrap_or(self.rows())
            .saturating_sub(1)
            .min(self.rows() - 1);
        self.scroll_top = top.min(bottom);
        self.scroll_bottom = top.max(bottom);
        self.term
            .set_scrolling_region(self.scroll_top + 1, Some(self.scroll_bottom + 1));
    }
    fn set_keypad_application_mode(&mut self) {
        self.modes.app_keypad = true;
        self.term.set_keypad_application_mode();
    }
    fn unset_keypad_application_mode(&mut self) {
        self.modes.app_keypad = false;
        self.term.unset_keypad_application_mode();
    }
    fn linefeed(&mut self) {
        let scroll = self.term.grid().cursor.point.line.0 as usize == self.scroll_bottom;
        if scroll {
            self.scroll_up(1);
        } else {
            self.term.linefeed();
        }
        self.term.grid_mut().cursor.input_needs_wrap = false;
        if self.modes.newline_mode {
            self.term.carriage_return();
        }
    }
    fn newline(&mut self) {
        self.linefeed();
        self.term.carriage_return();
    }
    fn substitute(&mut self) {
        let at = self.term.grid().cursor.point;
        let mut cell = self.term.grid().cursor.template.clone();
        cell.c = '?';
        self.term.grid_mut()[at] = cell;
    }
    fn bell(&mut self) {} // Source consumes BEL without emitting a host bell.
    fn identify_terminal(&mut self, _: Option<char>) {
        self.outgoing
            .push(TerminalEvent::Write(b"\x1b[?6c".to_vec()));
    }
    fn device_status(&mut self, status: usize) {
        if status == 6 {
            let cursor = self.cursor();
            self.outgoing.push(TerminalEvent::Write(
                format!("\x1b[{};{}R", cursor.row + 1, cursor.col + 1).into_bytes(),
            ));
        }
    }
    fn goto(&mut self, arg0: i32, arg1: usize) {
        self.term.goto(arg0, arg1);
    }
    fn goto_line(&mut self, arg0: i32) {
        self.term.goto_line(arg0);
    }
    fn goto_col(&mut self, arg0: usize) {
        self.term.goto_col(arg0);
    }
    fn insert_blank(&mut self, arg0: usize) {
        self.term.insert_blank(arg0);
    }
    fn move_up(&mut self, arg0: usize) {
        self.term.move_up(arg0);
    }
    fn move_down(&mut self, arg0: usize) {
        self.term.move_down(arg0);
    }
    fn move_forward(&mut self, arg0: usize) {
        self.term.move_forward(arg0);
    }
    fn move_backward(&mut self, arg0: usize) {
        self.term.move_backward(arg0);
    }
    fn move_down_and_cr(&mut self, arg0: usize) {
        self.term.move_down_and_cr(arg0);
    }
    fn move_up_and_cr(&mut self, arg0: usize) {
        self.term.move_up_and_cr(arg0);
    }
    fn put_tab(&mut self, arg0: u16) {
        for _ in 0..arg0 {
            let point = self.term.grid().cursor.point;
            let cell = self.term.grid()[point].clone();
            // ned moves through tabs without storing a tab glyph or changing
            // pending wrap. Disable Alacritty's tab-triggered wrap during the
            // movement, then retain the original pending state.
            let pending = self.term.grid().cursor.input_needs_wrap;
            self.term.grid_mut().cursor.input_needs_wrap = false;
            self.term.put_tab(1);
            self.term.grid_mut()[point] = cell;
            self.term.grid_mut().cursor.input_needs_wrap = pending;
        }
    }
    fn backspace(&mut self) {
        self.term.backspace();
    }
    fn carriage_return(&mut self) {
        self.term.carriage_return();
    }
    fn set_horizontal_tabstop(&mut self) {
        self.term.set_horizontal_tabstop();
    }
    fn scroll_up(&mut self, arg0: usize) {
        let count = arg0.min(self.scroll_bottom - self.scroll_top + 1);
        if count == 0 {
            return;
        }
        self.clear_region(
            0,
            self.scroll_top,
            self.cols() - 1,
            self.scroll_top + count - 1,
        );
        self.term.scroll_up(count);
        self.scroll_selection(self.scroll_top, -(count as i32));
        self.clear_region(
            0,
            self.scroll_bottom + 1 - count,
            self.cols() - 1,
            self.scroll_bottom,
        );
    }
    fn scroll_down(&mut self, arg0: usize) {
        let count = arg0.min(self.scroll_bottom - self.scroll_top + 1);
        if count == 0 {
            return;
        }
        self.clear_region(
            0,
            self.scroll_bottom + 1 - count,
            self.cols() - 1,
            self.scroll_bottom,
        );
        self.term.scroll_down(count);
        self.scroll_selection(self.scroll_top, count as i32);
        self.clear_region(
            0,
            self.scroll_top,
            self.cols() - 1,
            self.scroll_top + count - 1,
        );
    }
    fn insert_blank_lines(&mut self, arg0: usize) {
        self.term.insert_blank_lines(arg0);
    }
    fn delete_lines(&mut self, arg0: usize) {
        self.term.delete_lines(arg0);
    }
    fn delete_chars(&mut self, arg0: usize) {
        self.term.delete_chars(arg0);
    }
    fn move_backward_tabs(&mut self, arg0: u16) {
        self.term.move_backward_tabs(arg0);
    }
    fn move_forward_tabs(&mut self, arg0: u16) {
        self.put_tab(arg0);
    }
    fn clear_tabs(&mut self, arg0: TabulationClearMode) {
        self.term.clear_tabs(arg0);
    }
    fn set_tabs(&mut self, arg0: u16) {
        self.term.set_tabs(arg0);
    }
    fn reverse_index(&mut self) {
        self.term.reverse_index();
    }
    fn decaln(&mut self) {
        self.term.decaln();
    }
}

/// Pinned upstream Unicode 17.0 Emoji_Presentation ranges.
pub fn is_emoji_presentation(character: char) -> bool {
    let code = character as u32;
    const RANGES: &[(u32, u32)] = &[
        (0x231A, 0x231B),
        (0x23E9, 0x23EC),
        (0x23F0, 0x23F0),
        (0x23F3, 0x23F3),
        (0x25FD, 0x25FE),
        (0x2614, 0x2615),
        (0x2648, 0x2653),
        (0x267F, 0x267F),
        (0x2693, 0x2693),
        (0x26A1, 0x26A1),
        (0x26AA, 0x26AB),
        (0x26BD, 0x26BE),
        (0x26C4, 0x26C5),
        (0x26CE, 0x26CE),
        (0x26D4, 0x26D4),
        (0x26EA, 0x26EA),
        (0x26F2, 0x26F3),
        (0x26F5, 0x26F5),
        (0x26FA, 0x26FA),
        (0x26FD, 0x26FD),
        (0x2705, 0x2705),
        (0x270A, 0x270B),
        (0x2728, 0x2728),
        (0x274C, 0x274C),
        (0x274E, 0x274E),
        (0x2753, 0x2755),
        (0x2757, 0x2757),
        (0x2795, 0x2797),
        (0x27B0, 0x27B0),
        (0x27BF, 0x27BF),
        (0x2B1B, 0x2B1C),
        (0x2B50, 0x2B50),
        (0x2B55, 0x2B55),
        (0x1F004, 0x1F004),
        (0x1F0CF, 0x1F0CF),
        (0x1F18E, 0x1F18E),
        (0x1F191, 0x1F19A),
        (0x1F1E6, 0x1F1FF),
        (0x1F201, 0x1F201),
        (0x1F21A, 0x1F21A),
        (0x1F22F, 0x1F22F),
        (0x1F232, 0x1F236),
        (0x1F238, 0x1F23A),
        (0x1F250, 0x1F251),
        (0x1F300, 0x1F320),
        (0x1F32D, 0x1F335),
        (0x1F337, 0x1F37C),
        (0x1F37E, 0x1F393),
        (0x1F3A0, 0x1F3CA),
        (0x1F3CF, 0x1F3D3),
        (0x1F3E0, 0x1F3F0),
        (0x1F3F4, 0x1F3F4),
        (0x1F3F8, 0x1F43E),
        (0x1F440, 0x1F440),
        (0x1F442, 0x1F4FC),
        (0x1F4FF, 0x1F53D),
        (0x1F54B, 0x1F54E),
        (0x1F550, 0x1F567),
        (0x1F57A, 0x1F57A),
        (0x1F595, 0x1F596),
        (0x1F5A4, 0x1F5A4),
        (0x1F5FB, 0x1F64F),
        (0x1F680, 0x1F6C5),
        (0x1F6CC, 0x1F6CC),
        (0x1F6D0, 0x1F6D2),
        (0x1F6D5, 0x1F6D8),
        (0x1F6DC, 0x1F6DF),
        (0x1F6EB, 0x1F6EC),
        (0x1F6F4, 0x1F6FC),
        (0x1F7E0, 0x1F7EB),
        (0x1F7F0, 0x1F7F0),
        (0x1F90C, 0x1F93A),
        (0x1F93C, 0x1F945),
        (0x1F947, 0x1F9FF),
        (0x1FA70, 0x1FA7C),
        (0x1FA80, 0x1FA8A),
        (0x1FA8E, 0x1FAC6),
        (0x1FAC8, 0x1FAC8),
        (0x1FACD, 0x1FADC),
        (0x1FADF, 0x1FAEA),
        (0x1FAEF, 0x1FAF8),
    ];
    RANGES
        .iter()
        .any(|&(first, last)| code >= first && code <= last)
}
