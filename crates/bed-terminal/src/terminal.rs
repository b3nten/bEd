//! Rio owns the terminal state machine. The PTY worker is its only writer;
//! the frontend receives immutable viewport snapshots.
use bed_editing::util::color::{blend, ensure_contrast};
use rio_graphics::{ColorType, GraphicData, atlas_image_key, kitty_image_key};
use rio_vt::{
    ansi::{
        CursorShape,
        graphics::{OverlayViewport, atlas_overlay_geometry, kitty_overlay_geometry},
    },
    config::colors::{AnsiColor, ColorRgb},
    crosswords::{
        Crosswords, CrosswordsSize, Mode,
        grid::Scroll,
        pos::{Column, Line, Pos, Side},
        square::Wide,
        style::{StyleFlags, UnderlineKind},
    },
    event::{EventListener, RioEvent, WindowId},
    performer::handler::Processor,
    selection::{Selection, SelectionType},
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock, Weak},
    time::Instant,
};

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
const SCROLLBACK_LINES: usize = 10_000;
pub const MAX_IMAGE_BYTES: usize = 64 * 1024 * 1024;
pub const IMAGE_BUDGET_BYTES: usize = 128 * 1024 * 1024;
// Rio retains protocol pixels; the frontend retains immutable RGBA buffers.
// Splitting the budget bounds their combined retained pixel storage.
const IMAGE_STORE_BUDGET_BYTES: usize = IMAGE_BUDGET_BYTES / 2;

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
        let readable = |color| rgb8(ensure_contrast(color, background, 4.6), background);
        for (index, color) in self.ansi.into_iter().enumerate() {
            palette[index] = readable(color);
        }
        palette[258] = readable(self.foreground);
        palette[259] = rgb8(background, background);
        palette[256] = palette[258];
        palette[257] = palette[259];
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TerminalColor {
    Indexed(usize),
    Rgb([u8; 3]),
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TerminalUnderline {
    #[default]
    None,
    Single,
    Double,
    Curly,
    Dotted,
    Dashed,
}
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TerminalCell {
    /// First scalar, retained for text-only consumers. Render `text` as one cluster.
    pub character: char,
    pub text: Arc<str>,
    /// 0 for a continuation cell, otherwise 1 or 2 terminal columns.
    pub width: u8,
    pub mode: u16,
    pub fg: TerminalColor,
    pub bg: TerminalColor,
    pub underline: TerminalUnderline,
    pub underline_color: Option<TerminalColor>,
    pub hyperlink: Option<Arc<str>>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalCursor {
    pub row: usize,
    pub col: usize,
    /// DECSCUSR styles: 1/2 block, 3/4 underline, 5/6 beam.
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
    pub kitty_keyboard: u8,
    pub modify_other_keys: u8,
    pub grapheme_clustering: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalEvent {
    Write(Vec<u8>),
    Title(String),
    Bell,
    Error(String),
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SelectionSnap {
    #[default]
    None,
    Word,
    Line,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalRow {
    pub revision: u64,
    pub cells: Vec<TerminalCell>,
    pub wrapped: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalSelectionSpan {
    pub row: usize,
    pub start: usize,
    pub end: usize,
}
#[derive(Clone, Debug)]
pub struct TerminalImage {
    pub id: u64,
    pub revision: u64,
    pub width: usize,
    pub height: usize,
    pub rgba: Arc<[u8]>,
}
#[derive(Clone, Debug)]
pub struct TerminalImagePlacement {
    pub image: Arc<TerminalImage>,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub source_rect: [f32; 4],
    pub z_index: i32,
}
#[derive(Clone, Debug)]
pub struct TerminalSnapshot {
    pub revision: u64,
    pub cursor_revision: u64,
    pub cols: usize,
    pub rows: usize,
    pub lines: Vec<Arc<TerminalRow>>,
    pub cursor: TerminalCursor,
    pub modes: TerminalModes,
    pub palette: Arc<[[u8; 3]; 260]>,
    pub faint_palette: Option<Arc<[[u8; 3]; 260]>>,
    pub selection: Vec<TerminalSelectionSpan>,
    pub selection_text: Option<Arc<str>>,
    pub title: Arc<str>,
    pub current_directory: Option<std::path::PathBuf>,
    pub display_offset: usize,
    pub history_size: usize,
    pub images: Vec<TerminalImagePlacement>,
    pub warnings: Vec<Arc<str>>,
}
impl TerminalSnapshot {
    pub fn cols(&self) -> usize {
        self.cols
    }
    pub fn rows(&self) -> usize {
        self.rows
    }
    pub fn display_cell(&self, row: usize, col: usize) -> TerminalCell {
        self.lines[row].cells[col].clone()
    }
    pub fn cell(&self, row: usize, col: usize) -> TerminalCell {
        self.display_cell(row, col)
    }
    pub fn palette(&self) -> &[[u8; 3]; 260] {
        &self.palette
    }
    pub fn title(&self) -> &str {
        &self.title
    }
    pub fn modes(&self) -> TerminalModes {
        self.modes
    }
    pub fn cursor(&self) -> TerminalCursor {
        self.cursor
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn cursor_revision(&self) -> u64 {
        self.cursor_revision
    }
    pub fn display_offset(&self) -> usize {
        self.display_offset
    }
    pub fn history_size(&self) -> usize {
        self.history_size
    }
    pub fn selection_text(&self) -> Option<String> {
        self.selection_text.as_ref().map(ToString::to_string)
    }
    pub fn is_selected(&self, col: usize, row: usize) -> bool {
        self.selection
            .iter()
            .any(|span| span.row == row && col >= span.start && col < span.end)
    }
    #[cfg(feature = "ui")]
    pub(crate) fn faint_color(&self, color: TerminalColor) -> Option<[u8; 3]> {
        let TerminalColor::Indexed(index) = color else {
            return None;
        };
        self.faint_palette
            .as_ref()
            .filter(|_| index < 16 || index == 258)
            .map(|palette| palette[index])
    }
}

#[derive(Clone, Default)]
struct EventQueue(Arc<Mutex<Vec<RioEvent>>>);
impl EventListener for EventQueue {
    fn send_event(&self, event: RioEvent, _: WindowId) {
        self.0.lock().unwrap().push(event);
    }
}
pub struct Terminal {
    term: Crosswords<EventQueue>,
    parser: Processor,
    events: EventQueue,
    theme: Option<TerminalTheme>,
    theme_palette: [[u8; 3]; 260],
    palette: Arc<[[u8; 3]; 260]>,
    faint_palette: Option<Arc<[[u8; 3]; 260]>>,
    revision: u64,
    cursor_revision: u64,
    row_revision: u64,
    image_revision: u64,
    rows: Vec<Arc<TerminalRow>>,
    images: HashMap<u64, Arc<TerminalImage>>,
    image_buffers: Vec<Weak<TerminalImage>>,
    ascii: [Arc<str>; 128],
    num_lock: bool,
    selection_anchor: Option<Pos>,
    warnings: Vec<Arc<str>>,
    last_snapshot: Option<Arc<TerminalSnapshot>>,
}
impl Default for Terminal {
    fn default() -> Self {
        Self::new(80, 24)
    }
}
impl Terminal {
    pub fn new(cols: usize, rows: usize) -> Self {
        let events = EventQueue::default();
        let mut term = Crosswords::new(
            CrosswordsSize::new_with_dimensions(
                cols.max(2),
                rows.max(1),
                cols as u32 * 8,
                rows as u32 * 16,
                8,
                16,
            ),
            CursorShape::Block,
            events.clone(),
            WindowId::from(0),
            0,
            SCROLLBACK_LINES,
        );
        term.title = "Terminal".into();
        term.graphics.total_limit = IMAGE_STORE_BUDGET_BYTES;
        let mut terminal = Self {
            term,
            parser: Processor::default(),
            events,
            theme: None,
            theme_palette: default_palette(),
            palette: Arc::new(default_palette()),
            faint_palette: None,
            revision: 1,
            cursor_revision: 1,
            row_revision: 0,
            image_revision: 0,
            rows: Vec::new(),
            images: HashMap::new(),
            image_buffers: Vec::new(),
            ascii: std::array::from_fn(|c| {
                Arc::from(char::from_u32(c as u32).unwrap().to_string())
            }),
            num_lock: true,
            selection_anchor: None,
            warnings: Vec::new(),
            last_snapshot: None,
        };
        terminal.snapshot();
        terminal
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
    pub fn title(&self) -> &str {
        &self.term.title
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn cursor_revision(&self) -> u64 {
        self.cursor_revision
    }
    pub fn wrap_pending(&self) -> bool {
        self.term.grid.cursor.should_wrap
    }
    pub fn set_num_lock(&mut self, value: bool) {
        if self.num_lock != value {
            self.num_lock = value;
            self.changed();
        }
    }
    pub fn modes(&self) -> TerminalModes {
        let mode = self.term.mode();
        TerminalModes {
            app_cursor: mode.contains(Mode::APP_CURSOR),
            app_keypad: mode.contains(Mode::APP_KEYPAD),
            mouse_x10: mode.contains(Mode::MOUSE_REPORT_X10),
            mouse_button: mode.contains(Mode::MOUSE_REPORT_CLICK),
            mouse_motion: mode.contains(Mode::MOUSE_DRAG),
            mouse_many: mode.contains(Mode::MOUSE_MOTION),
            mouse_sgr: mode.contains(Mode::SGR_MOUSE),
            focus_reporting: mode.contains(Mode::FOCUS_IN_OUT),
            bracket_paste: mode.contains(Mode::BRACKETED_PASTE),
            alt_screen: mode.contains(Mode::ALT_SCREEN),
            newline_mode: mode.contains(Mode::LINE_FEED_NEW_LINE),
            num_lock: self.num_lock,
            kitty_keyboard: self.term.keyboard_mode().bits(),
            modify_other_keys: self.term.modify_other_keys().unwrap_or(0),
            grapheme_clustering: mode.contains(Mode::GRAPHEME_CLUSTER),
            ..Default::default()
        }
    }
    pub fn cursor(&self) -> TerminalCursor {
        let cursor = self.term.cursor();
        let blinking = self.term.blinking_cursor;
        let shape = match cursor.content {
            CursorShape::Underline => 4,
            CursorShape::Beam => 6,
            _ => 2,
        } - u8::from(blinking);
        TerminalCursor {
            row: cursor.pos.row.0.max(0) as usize,
            col: cursor.pos.col.0,
            shape,
            blinking,
            visible: cursor.content != CursorShape::Hidden,
        }
    }
    pub fn cell(&self, row: usize, col: usize) -> TerminalCell {
        self.grid_cell(Line(row as i32), Column(col))
    }
    pub fn display_cell(&self, row: usize, col: usize) -> TerminalCell {
        self.grid_cell(Line(row as i32 - self.display_offset() as i32), Column(col))
    }
    fn grid_cell(&self, row: Line, col: Column) -> TerminalCell {
        let square = self.term.grid[row][col];
        let style = self.term.grid.style_of(&square);
        let extra = square
            .extras_id_checked()
            .and_then(|id| self.term.grid.extras_table.get(id));
        // Rio's compact blank/background cells use scalar zero internally.
        let character = match square.c() {
            '\0' => ' ',
            character => character,
        };
        let text = if let Some(extra) = extra.filter(|extra| !extra.zerowidth.is_empty()) {
            Arc::from(
                std::iter::once(character)
                    .chain(extra.zerowidth.iter().copied())
                    .collect::<String>(),
            )
        } else if character.is_ascii() {
            self.ascii[character as usize].clone()
        } else {
            Arc::from(character.to_string())
        };
        let mut mode = 0;
        for (flag, attr) in [
            (StyleFlags::BOLD, ATTR_BOLD),
            (StyleFlags::DIM, ATTR_FAINT),
            (StyleFlags::ITALIC, ATTR_ITALIC),
            (StyleFlags::ALL_UNDERLINES, ATTR_UNDERLINE),
            (StyleFlags::ALL_BLINK, ATTR_BLINK),
            (StyleFlags::INVERSE, ATTR_REVERSE),
            (StyleFlags::HIDDEN, ATTR_INVISIBLE),
            (StyleFlags::STRIKEOUT, ATTR_STRUCK),
        ] {
            if style.flags.intersects(flag) {
                mode |= attr;
            }
        }
        if square.wrapline() {
            mode |= ATTR_WRAP;
        }
        if character == rio_vt::ansi::kitty_virtual::PLACEHOLDER {
            mode |= ATTR_INVISIBLE;
        }
        let width = match square.wide() {
            Wide::Wide => {
                mode |= ATTR_WIDE;
                2
            }
            Wide::Spacer | Wide::LeadingSpacer => {
                mode |= ATTR_WDUMMY;
                0
            }
            _ => 1,
        };
        let underline = match style.flags.underline_kind() {
            None => TerminalUnderline::None,
            Some(UnderlineKind::Single) => TerminalUnderline::Single,
            Some(UnderlineKind::Double) => TerminalUnderline::Double,
            Some(UnderlineKind::Curly) => TerminalUnderline::Curly,
            Some(UnderlineKind::Dotted) => TerminalUnderline::Dotted,
            Some(UnderlineKind::Dashed) => TerminalUnderline::Dashed,
        };
        TerminalCell {
            character,
            text,
            width,
            mode,
            fg: color(style.fg),
            bg: color(style.bg),
            underline,
            underline_color: style.underline_color.map(color),
            hyperlink: extra
                .and_then(|extra| extra.hyperlink.as_ref())
                .map(|link| Arc::from(link.uri())),
        }
    }
    pub fn display_offset(&self) -> usize {
        self.term.display_offset()
    }
    pub fn history_size(&self) -> usize {
        self.term.history_size()
    }
    pub fn scroll_display(&mut self, lines: i32) {
        let before = self.display_offset();
        self.term.scroll_display(Scroll::Delta(lines));
        if self.display_offset() != before {
            self.changed();
        }
    }
    pub fn scroll_to_bottom(&mut self) {
        let before = self.display_offset();
        self.term.scroll_display(Scroll::Bottom);
        if self.display_offset() != before {
            self.changed();
        }
    }
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<TerminalEvent> {
        let cursor = self.cursor();
        self.parser.advance(&mut self.term, bytes);
        if !bytes.is_empty() {
            self.changed();
        }
        if cursor != self.cursor() {
            self.cursor_revision += 1;
        }
        self.refresh_palette();
        let mut events = self.drain_events();
        events.extend(self.process_graphics());
        events
    }
    pub fn feed_echo(&mut self, bytes: &[u8]) -> Vec<TerminalEvent> {
        self.feed(bytes)
    }
    pub fn sync_deadline(&self) -> Option<Instant> {
        self.parser.sync_timeout().sync_timeout()
    }
    pub fn flush_sync(&mut self) -> Vec<TerminalEvent> {
        if !self
            .sync_deadline()
            .is_some_and(|deadline| deadline <= Instant::now())
        {
            return Vec::new();
        }
        self.finish_output()
    }
    pub fn finish_output(&mut self) -> Vec<TerminalEvent> {
        if self.sync_deadline().is_none() {
            return Vec::new();
        }
        self.parser.stop_sync(&mut self.term);
        self.changed();
        self.refresh_palette();
        let mut events = self.drain_events();
        events.extend(self.process_graphics());
        events
    }
    fn drain_events(&mut self) -> Vec<TerminalEvent> {
        let native = std::mem::take(&mut *self.events.0.lock().unwrap());
        let mut outgoing = Vec::new();
        for event in native {
            match event {
                RioEvent::PtyWrite(_, bytes) => {
                    outgoing.push(TerminalEvent::Write(bytes.into_bytes()))
                }
                RioEvent::Title(_, title) => outgoing.push(TerminalEvent::Title(title)),
                RioEvent::Bell(_) => outgoing.push(TerminalEvent::Bell),
                RioEvent::UpdateGraphics { queues, .. } => {
                    outgoing.extend(self.consume_graphics_queues(queues))
                }
                RioEvent::ColorRequest(_, index, format) => {
                    let rgb = self.palette[map_color_index(index).min(259)];
                    outgoing.push(TerminalEvent::Write(
                        format(ColorRgb {
                            r: rgb[0],
                            g: rgb[1],
                            b: rgb[2],
                        })
                        .into_bytes(),
                    ));
                }
                RioEvent::TextAreaSizeRequest(_, format) => outgoing.push(TerminalEvent::Write(
                    format(rio_vt::event::WindowSize {
                        cols: self.cols().min(u16::MAX as usize) as u16,
                        rows: self.rows().min(u16::MAX as usize) as u16,
                        width: (self.cols() as f32 * self.term.graphics.cell_width)
                            .min(u16::MAX as f32) as u16,
                        height: (self.rows() as f32 * self.term.graphics.cell_height)
                            .min(u16::MAX as f32) as u16,
                    })
                    .into_bytes(),
                )),
                // Clipboard reads need explicit frontend approval; don't expose host clipboard to applications.
                RioEvent::ClipboardLoad(_, _, format) => {
                    outgoing.push(TerminalEvent::Write(format("").into_bytes()))
                }
                _ => {}
            }
        }
        outgoing
    }
    fn changed(&mut self) {
        self.revision += 1;
    }
    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.resize_with_pixels(
            cols,
            rows,
            self.term.graphics.cell_width as u32,
            self.term.graphics.cell_height as u32,
        );
    }
    pub fn resize_with_pixels(
        &mut self,
        cols: usize,
        rows: usize,
        cell_width: u32,
        cell_height: u32,
    ) {
        let cols = cols.clamp(2, u16::MAX as usize);
        let rows = rows.clamp(1, u16::MAX as usize);
        if cols == self.cols()
            && rows == self.rows()
            && cell_width == self.term.graphics.cell_width as u32
            && cell_height == self.term.graphics.cell_height as u32
        {
            return;
        }
        self.term.resize(CrosswordsSize::new_with_dimensions(
            cols,
            rows,
            cols as u32 * cell_width,
            rows as u32 * cell_height,
            cell_width.max(1),
            cell_height.max(1),
        ));
        self.changed();
        self.cursor_revision += 1;
    }
    pub fn set_theme(&mut self, theme: &TerminalTheme) -> bool {
        if self.theme.as_ref() == Some(theme) {
            return false;
        }
        self.theme_palette = theme.palette();
        self.theme = Some(theme.clone());
        self.refresh_palette();
        self.changed();
        true
    }
    fn refresh_palette(&mut self) {
        let mut palette = self.theme_palette;
        for (index, rgb) in palette.iter_mut().enumerate() {
            if index == 257 {
                continue;
            }
            if let Some(override_rgb) = self.term.colors()[native_color_index(index)] {
                *rgb = std::array::from_fn(|channel| {
                    (override_rgb[channel] * 255.0).round().clamp(0.0, 255.0) as u8
                });
            }
        }
        palette[257] = palette[259];
        if self.palette.as_ref() != &palette {
            self.palette = Arc::new(palette);
        }
        if self.theme.is_some() {
            let mut faint = palette;
            let background = rgba(palette[259]);
            for index in (0..16).chain([258]) {
                let dimmed = blend(rgba(palette[index]), background, 0.65);
                faint[index] = if self.term.colors()[native_color_index(index)].is_none() {
                    rgb8(ensure_contrast(dimmed, background, 4.6), background)
                } else {
                    rgb8(dimmed, background)
                };
            }
            if self.faint_palette.as_deref() != Some(&faint) {
                self.faint_palette = Some(Arc::new(faint));
            }
        }
    }
    fn viewport_pos(&self, col: usize, row: usize) -> Pos {
        Pos::new(
            Line(row.min(self.rows() - 1) as i32 - self.display_offset() as i32),
            Column(col.min(self.cols() - 1)),
        )
    }
    pub fn clear_selection(&mut self) {
        if self.term.selection.take().is_some() {
            self.changed();
        }
        self.selection_anchor = None;
    }
    pub fn select_start(&mut self, col: usize, row: usize, snap: SelectionSnap) {
        let pos = self.viewport_pos(col, row);
        self.selection_anchor = Some(pos);
        self.term.selection = Some(Selection::new(
            match snap {
                SelectionSnap::None => SelectionType::Simple,
                SelectionSnap::Word => SelectionType::Semantic,
                SelectionSnap::Line => SelectionType::Lines,
            },
            pos,
            Side::Left,
        ));
        self.changed();
    }
    pub fn select_extend(&mut self, col: usize, row: usize, rectangular: bool, _done: bool) {
        let pos = self.viewport_pos(col, row);
        if let Some(selection) = self.term.selection.as_mut() {
            if rectangular {
                selection.ty = SelectionType::Block;
            }
            let side = if self.selection_anchor.is_some_and(|anchor| pos < anchor) {
                Side::Left
            } else {
                Side::Right
            };
            selection.update(pos, side);
            self.changed();
        }
    }
    pub fn select_all(&mut self) {
        let mut selection = Selection::new(
            SelectionType::Lines,
            Pos::new(Line(-(self.history_size() as i32)), Column(0)),
            Side::Left,
        );
        selection.update(
            Pos::new(Line(self.rows() as i32 - 1), Column(self.cols() - 1)),
            Side::Right,
        );
        self.term.selection = Some(selection);
        self.selection_anchor = None;
        self.changed();
    }
    pub fn is_selected(&self, col: usize, row: usize) -> bool {
        self.term
            .selection
            .as_ref()
            .and_then(|selection| selection.to_range(&self.term))
            .is_some_and(|range| range.contains(self.viewport_pos(col, row)))
    }
    pub fn selection_text(&self) -> Option<String> {
        self.term.selection_to_string()
    }
    pub fn snapshot(&mut self) -> Arc<TerminalSnapshot> {
        // A begin marker and its following bytes can arrive in the same read.
        // Freeze presentation until ESU or timeout, regardless of read boundaries.
        if let Some(snapshot) = &self.last_snapshot
            && (self.sync_deadline().is_some() || snapshot.revision == self.revision)
        {
            return snapshot.clone();
        }
        let offset = self.display_offset() as i32;
        let old_rows = std::mem::take(&mut self.rows);
        let full = self.term.is_fully_damaged() || old_rows.len() != self.rows();
        let mut rows = Vec::with_capacity(self.rows());
        for y in 0..self.rows() {
            let line = Line(y as i32 - offset);
            if !full && !self.term.grid[line].dirty {
                rows.push(old_rows[y].clone());
                continue;
            }
            let cells: Vec<_> = (0..self.cols())
                .map(|x| self.grid_cell(line, Column(x)))
                .collect();
            let wrapped = cells.last().is_some_and(|cell| cell.mode & ATTR_WRAP != 0);
            if let Some(previous) = old_rows
                .iter()
                .find(|previous| previous.wrapped == wrapped && previous.cells == cells)
            {
                rows.push(previous.clone());
            } else {
                self.row_revision += 1;
                rows.push(Arc::new(TerminalRow {
                    revision: self.row_revision,
                    cells,
                    wrapped,
                }));
            }
        }
        for y in 0..self.rows() {
            self.term.grid[Line(y as i32 - offset)].dirty = false;
        }
        self.term.reset_damage();
        let mut selection = Vec::new();
        if let Some(range) = self
            .term
            .selection
            .as_ref()
            .and_then(|selection| selection.to_range(&self.term))
        {
            for row in 0..self.rows() {
                let mut start = None;
                for col in 0..self.cols() {
                    if range.contains(self.viewport_pos(col, row)) {
                        start.get_or_insert(col);
                    } else if let Some(first) = start.take() {
                        selection.push(TerminalSelectionSpan {
                            row,
                            start: first,
                            end: col,
                        });
                    }
                }
                if let Some(first) = start {
                    selection.push(TerminalSelectionSpan {
                        row,
                        start: first,
                        end: self.cols(),
                    });
                }
            }
        }
        self.rows = rows;
        let images = self.snapshot_images();
        let snapshot = Arc::new(TerminalSnapshot {
            revision: self.revision,
            cursor_revision: self.cursor_revision,
            cols: self.cols(),
            rows: self.rows(),
            lines: self.rows.clone(),
            cursor: self.cursor(),
            modes: self.modes(),
            palette: self.palette.clone(),
            faint_palette: self.faint_palette.clone(),
            selection,
            selection_text: self.selection_text().map(Arc::from),
            title: Arc::from(self.title()),
            current_directory: self.term.current_directory.clone(),
            display_offset: self.display_offset(),
            history_size: self.history_size(),
            images,
            warnings: self.warnings.clone(),
        });
        self.last_snapshot = Some(snapshot.clone());
        snapshot
    }
    fn snapshot_images(&mut self) -> Vec<TerminalImagePlacement> {
        self.process_graphics();
        // Images may have been evicted from the frontend's bounded store. A
        // retained kitty placement can be restored from Rio's image cache.
        let requested: std::collections::HashSet<_> = self
            .term
            .graphics
            .kitty_placements
            .values()
            .map(|placement| placement.image_id)
            .chain(
                self.term
                    .graphics
                    .kitty_virtual_placements
                    .values()
                    .map(|placement| placement.image_id),
            )
            .collect();
        let missing: Vec<_> = requested
            .into_iter()
            .filter_map(|id| {
                let key = kitty_image_key(id);
                if self.images.contains_key(&key) {
                    return None;
                }
                self.term
                    .graphics
                    .kitty_images
                    .get(&id)
                    .map(|image| (key, image.data.clone()))
            })
            .collect();
        for (key, image) in missing {
            self.store_image(key, image);
        }
        let viewport = OverlayViewport {
            cell_width: self.term.graphics.cell_width,
            cell_height: self.term.graphics.cell_height,
            origin_x: 0.0,
            origin_y: 0.0,
            history_size: self.history_size() as i64 + self.term.lines_evicted() as i64,
            display_offset: self.display_offset() as i64,
            screen_lines: self.rows() as i64,
        };
        let mut placements = Vec::new();
        for placement in self.term.graphics.kitty_placements.values() {
            let Some(image) = self.images.get(&kitty_image_key(placement.image_id)) else {
                continue;
            };
            if let Some(geometry) =
                kitty_overlay_geometry(placement, image.width, image.height, &viewport)
            {
                placements.push(TerminalImagePlacement {
                    image: image.clone(),
                    x: geometry.x,
                    y: geometry.y,
                    width: geometry.width,
                    height: geometry.height,
                    source_rect: geometry.source_rect,
                    z_index: placement.z_index,
                });
            }
        }
        for placement in &self.term.graphics.atlas_placements {
            let Some(image) = self.images.get(&placement.image_key) else {
                continue;
            };
            if let Some(geometry) = atlas_overlay_geometry(placement, &viewport) {
                placements.push(TerminalImagePlacement {
                    image: image.clone(),
                    x: geometry.x,
                    y: geometry.y,
                    width: geometry.width,
                    height: geometry.height,
                    source_rect: geometry.source_rect,
                    z_index: -1,
                });
            }
        }
        if !self.term.graphics.kitty_virtual_placements.is_empty() {
            use rio_vt::ansi::kitty_virtual::{
                IncompletePlacement, PLACEHOLDER, compute_run_geometry, resolve_virtual_placement,
            };
            for row in 0..self.rows() {
                let line = Line(row as i32 - self.display_offset() as i32);
                let mut col = 0;
                while col < self.cols() {
                    let square = self.term.grid[line][Column(col)];
                    if square.c() != PLACEHOLDER {
                        col += 1;
                        continue;
                    }
                    let decode = |square| {
                        let style = self.term.grid.style_of(&square);
                        let combining = square
                            .extras_id_checked()
                            .and_then(|id| self.term.grid.extras_table.get(id))
                            .map_or(&[][..], |extras| extras.zerowidth.as_slice());
                        IncompletePlacement::from_cell(style.fg, style.underline_color, combining)
                    };
                    let start = col;
                    let mut run = decode(square);
                    col += 1;
                    while col < self.cols() {
                        let next = self.term.grid[line][Column(col)];
                        if next.c() != PLACEHOLDER || !run.can_append(&decode(next)) {
                            break;
                        }
                        run.append();
                        col += 1;
                    }
                    let run = run.complete();
                    let Some(image) = self.images.get(&kitty_image_key(run.image_id)) else {
                        continue;
                    };
                    let Some(placement) = resolve_virtual_placement(
                        &self.term.graphics.kitty_virtual_placements,
                        run.image_id,
                        run.placement_id,
                    ) else {
                        continue;
                    };
                    if let Some(geometry) = compute_run_geometry(
                        &run,
                        placement.columns,
                        placement.rows,
                        image.width as u32,
                        image.height as u32,
                        (placement.x, placement.y, placement.width, placement.height),
                        viewport.cell_width,
                        viewport.cell_height,
                        0.0,
                        0.0,
                        row,
                        start,
                    ) {
                        placements.push(TerminalImagePlacement {
                            image: image.clone(),
                            x: geometry.x,
                            y: geometry.y,
                            width: geometry.width,
                            height: geometry.height,
                            source_rect: geometry.source_rect,
                            z_index: 0,
                        });
                    }
                }
            }
        }
        placements.sort_by_key(|placement| placement.z_index);
        placements
    }
    fn process_graphics(&mut self) -> Vec<TerminalEvent> {
        let mut events = Vec::new();
        if std::mem::take(&mut self.term.graphics.budget_rejected)
            && let Some(error) = self.image_warning(
                "Terminal image resource limit reached (128 MiB, 1024 images or 4096 placements); this image was skipped",
            )
        {
            events.push(TerminalEvent::Error(error));
        }
        if std::mem::take(&mut self.term.graphics.budget_evicted_displayed)
            && let Some(error) = self.image_warning(
                "Terminal image resource limit reached (128 MiB or 4096 placements); oldest displayed images were released",
            )
        {
            events.push(TerminalEvent::Error(error));
        }
        if self.term.graphics.total_bytes > IMAGE_STORE_BUDGET_BYTES {
            let used = self.term.graphics.collect_active_graphic_ids();
            self.term.graphics.evict_images(0, &used);
            if let Some(error) = self.image_warning(
                "Terminal image budget reached (128 MiB); oldest images were released",
            ) {
                events.push(TerminalEvent::Error(error));
            }
        }
        if let Some(queues) = self.term.graphics_take_queues() {
            events.extend(self.consume_graphics_queues(queues));
        }
        events
    }
    fn consume_graphics_queues(
        &mut self,
        queues: rio_vt::ansi::graphics::UpdateQueues,
    ) -> Vec<TerminalEvent> {
        let mut events = Vec::new();
        for key in queues.remove_queue {
            self.images.remove(&key);
        }
        for image in queues.pending {
            if let Some(error) = self.store_image(atlas_image_key(image.id.get()), image) {
                events.push(TerminalEvent::Error(error));
            }
        }
        for (id, image) in queues.pending_images {
            if let Some(error) = self.store_image(kitty_image_key(id), image) {
                events.push(TerminalEvent::Error(error));
            }
        }
        events
    }
    fn store_image(&mut self, key: u64, image: GraphicData) -> Option<String> {
        let bytes = image
            .width
            .checked_mul(image.height)
            .and_then(|pixels| pixels.checked_mul(4))
            .unwrap_or(usize::MAX);
        if bytes > MAX_IMAGE_BYTES {
            return self.image_warning("Terminal image exceeds the 64 MiB upload limit");
        }
        // Replacing an image releases the cache's old generation. A snapshot
        // may still pin its pixels, so the residency ledger counts both.
        self.images.remove(&key);
        if self.sync_deadline().is_none() {
            self.last_snapshot = None;
        }
        // Remove unused textures first. Rio retains kitty pixels for a later
        // placement; that path can request their upload again.
        let mut active = std::collections::HashSet::new();
        for placement in self.term.graphics.kitty_placements.values() {
            active.insert(kitty_image_key(placement.image_id));
        }
        for placement in self.term.graphics.kitty_virtual_placements.values() {
            active.insert(kitty_image_key(placement.image_id));
        }
        for placement in &self.term.graphics.atlas_placements {
            active.insert(placement.image_key);
        }
        for placement in self
            .term
            .graphics
            .kitty_inactive_screen
            .kitty_placements
            .values()
        {
            active.insert(kitty_image_key(placement.image_id));
        }
        for placement in &self.term.graphics.kitty_inactive_screen.atlas_placements {
            active.insert(placement.image_key);
        }
        let resident = self.image_resident_bytes();
        if resident.saturating_add(bytes) > IMAGE_STORE_BUDGET_BYTES {
            self.images.retain(|id, _| active.contains(id));
        }
        let mut resident = self.image_resident_bytes();
        if resident.saturating_add(bytes) > IMAGE_STORE_BUDGET_BYTES {
            let mut oldest: Vec<_> = self
                .images
                .iter()
                .filter(|(id, _)| **id != key)
                .map(|(id, image)| (*id, image.revision))
                .collect();
            oldest.sort_by_key(|(_, revision)| *revision);
            for (id, _) in oldest {
                if resident.saturating_add(bytes) <= IMAGE_STORE_BUDGET_BYTES {
                    break;
                }
                self.images.remove(&id);
                if id <= u32::MAX as u64 {
                    self.term
                        .graphics
                        .delete_kitty_images(|image_id, _| u64::from(*image_id) == id);
                } else {
                    self.term
                        .graphics
                        .atlas_placements
                        .retain(|placement| placement.image_key != id);
                    self.term
                        .graphics
                        .kitty_inactive_screen
                        .atlas_placements
                        .retain(|placement| placement.image_key != id);
                    self.term.graphics.recount_atlas_keys();
                    self.term.graphics.recount_inactive_atlas_keys();
                }
                resident = self.image_resident_bytes();
            }
            self.image_warning(
                "Terminal image budget reached (128 MiB); oldest displayed images were released",
            );
        }
        if self.image_resident_bytes().saturating_add(bytes) > IMAGE_STORE_BUDGET_BYTES {
            // Old immutable frames remain valid until consumed. Never admit
            // a replacement that would count their live pixels twice.
            if key <= u32::MAX as u64 {
                self.term
                    .graphics
                    .delete_kitty_images(|id, _| u64::from(*id) == key);
            } else {
                self.term
                    .graphics
                    .atlas_placements
                    .retain(|placement| placement.image_key != key);
                self.term.graphics.recount_atlas_keys();
            }
            self.changed();
            return self
                .image_warning("Terminal image budget is full (128 MiB); this image was skipped");
        }
        let rgba = match image.color_type {
            ColorType::Rgba => image.pixels,
            ColorType::Rgb => image
                .pixels
                .as_chunks::<3>()
                .0
                .iter()
                .flat_map(|rgb| [rgb[0], rgb[1], rgb[2], 255])
                .collect(),
        };
        if rgba.len() != bytes {
            return self.image_warning("Invalid terminal image pixel data");
        }
        self.image_revision += 1;
        let image = Arc::new(TerminalImage {
            id: key,
            revision: self.image_revision,
            width: image.width,
            height: image.height,
            rgba: Arc::from(rgba),
        });
        self.image_buffers.push(Arc::downgrade(&image));
        self.images.insert(key, image);
        None
    }
    fn image_resident_bytes(&mut self) -> usize {
        // Each admission adds exactly one weak reference, keyed by its unique
        // image revision rather than the protocol ID which clients can reuse.
        self.image_buffers.retain(|image| image.strong_count() != 0);
        self.image_buffers
            .iter()
            .filter_map(Weak::upgrade)
            .map(|image| image.rgba.len())
            .sum()
    }
    fn image_warning(&mut self, text: &str) -> Option<String> {
        if self
            .warnings
            .last()
            .is_none_or(|warning| warning.as_ref() != text)
        {
            if self.warnings.len() == 8 {
                self.warnings.remove(0);
            }
            self.warnings.push(Arc::from(text));
        }
        Some(text.into())
    }
}
fn native_color_index(index: usize) -> usize {
    match index {
        256 => 258,
        258 => 256,
        259 => 257,
        _ => index,
    }
}
fn map_color_index(index: usize) -> usize {
    match index {
        256 => 258,
        257 => 259,
        258 => 256,
        259..=266 => index - 259,
        267..=268 => 258,
        _ => index,
    }
}
fn color(value: AnsiColor) -> TerminalColor {
    match value {
        AnsiColor::Indexed(index) => TerminalColor::Indexed(index as usize),
        AnsiColor::Named(index) => TerminalColor::Indexed(map_color_index(index as usize)),
        AnsiColor::Spec(rgb) => TerminalColor::Rgb([rgb.r, rgb.g, rgb.b]),
    }
}

fn default_palette() -> [[u8; 3]; 260] {
    // Preserve Bed's default ANSI colors. The remaining indexed colors use
    // xterm's documented 6×6×6 cube and 24-step grayscale ramp.
    const ANSI: [[u8; 3]; 16] = [
        [0, 0, 0],
        [205, 0, 0],
        [0, 205, 0],
        [205, 205, 0],
        [0, 0, 238],
        [205, 0, 205],
        [0, 205, 205],
        [229, 229, 229],
        [127, 127, 127],
        [255, 0, 0],
        [0, 255, 0],
        [255, 255, 0],
        [92, 92, 255],
        [255, 0, 255],
        [0, 255, 255],
        [255, 255, 255],
    ];
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    std::array::from_fn(|slot| match slot {
        0..16 => ANSI[slot],
        16..232 => {
            let cube = slot - 16;
            [LEVELS[cube / 36], LEVELS[cube / 6 % 6], LEVELS[cube % 6]]
        }
        232..256 => [8 + 10 * (slot - 232) as u8; 3],
        256 => [204; 3],
        257 => [85; 3],
        258 => ANSI[7],
        _ => ANSI[0],
    })
}
/// XParseColor-style names, #RGB/#RRGGBB/#RRRRGGGGBBBB and rgb:R/G/B.
/// Names come directly from X.Org's separately licensed color database.
pub fn parse_color(spec: &str) -> Option<[u8; 3]> {
    let channels: Vec<&str> = match spec.as_bytes().first() {
        Some(b'#') if matches!(spec.len(), 4 | 7 | 13) && spec.is_ascii() => {
            let width = (spec.len() - 1) / 3;
            (0..3)
                .map(|i| &spec[1 + i * width..1 + (i + 1) * width])
                .collect()
        }
        Some(b'#') => return None,
        _ if spec.starts_with("rgb:") => spec[4..].split('/').collect(),
        _ => {
            static NAMES: OnceLock<HashMap<String, [u8; 3]>> = OnceLock::new();
            let names = NAMES.get_or_init(|| {
                include_str!("../../../resources/terminal/rgb.txt")
                    .lines()
                    .filter(|line| !line.starts_with('!') && !line.trim().is_empty())
                    .map(|line| {
                        let mut words = line.split_ascii_whitespace();
                        let rgb = std::array::from_fn(|_| {
                            words
                                .next()
                                .expect("X.Org color channel")
                                .parse()
                                .expect("X.Org RGB byte")
                        });
                        (words.collect::<String>().to_ascii_lowercase(), rgb)
                    })
                    .collect()
            });
            let key: String = spec
                .bytes()
                .filter(|byte| !byte.is_ascii_whitespace())
                .map(|byte| byte.to_ascii_lowercase() as char)
                .collect();
            return names.get(&key).copied();
        }
    };
    if channels.len() != 3 {
        return None;
    }
    let mut rgb = [0; 3];
    for (out, digits) in rgb.iter_mut().zip(channels) {
        if !(1..=4).contains(&digits.len()) || !digits.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return None;
        }
        let value = u32::from_str_radix(digits, 16).ok()?;
        let range = (1u32 << (4 * digits.len())) - 1;
        *out = (value * 65535 / range / 256) as u8;
    }
    Some(rgb)
}

#[cfg(test)]
mod image_budget_tests {
    use super::*;
    use rio_graphics::GraphicId;
    use rio_vt::ansi::graphics::KittyPlacement;
    use std::fmt::Write as _;

    fn pixels(id: u64, width: usize, height: usize) -> GraphicData {
        GraphicData {
            id: GraphicId::new(id),
            width,
            height,
            color_type: ColorType::Rgba,
            pixels: vec![255; width * height * 4],
            is_opaque: true,
            resize: None,
            display_width: None,
            display_height: None,
            transmit_time: Instant::now(),
        }
    }

    #[test]
    fn a_pinned_snapshot_counts_against_replacement_image_admission() {
        let mut terminal = Terminal::new(20, 3);
        assert!(terminal.store_image(7, pixels(7, 4096, 4096)).is_none());
        terminal.term.graphics.kitty_placements.insert(
            (7, 1),
            KittyPlacement {
                image_id: 7,
                placement_id: 1,
                source_x: 0,
                source_y: 0,
                source_width: 0,
                source_height: 0,
                dest_col: 0,
                dest_row: 0,
                columns: 1,
                rows: 1,
                requested_columns: 1,
                requested_rows: 1,
                pixel_width: 8,
                pixel_height: 16,
                cell_x_offset: 0,
                cell_y_offset: 0,
                z_index: 0,
                transmit_time: Instant::now(),
            },
        );
        terminal.changed();
        let held = terminal.snapshot();
        assert_eq!(held.images[0].image.rgba.len(), IMAGE_STORE_BUDGET_BYTES);

        // Reusing the same protocol ID does not conceal the old generation's
        // pixels while a consumer still holds its immutable snapshot.
        assert!(terminal.store_image(7, pixels(7, 1, 1)).is_some());
        assert!(!terminal.images.contains_key(&7));
        assert_eq!(terminal.image_resident_bytes(), IMAGE_STORE_BUDGET_BYTES);
        let denied = terminal.snapshot();
        assert!(denied.images.is_empty());
        assert!(!denied.warnings.is_empty());
        drop(held);
        assert!(terminal.store_image(7, pixels(7, 1, 1)).is_none());
        assert_eq!(terminal.image_resident_bytes(), 4);
    }

    #[test]
    fn native_admission_evicts_oldest_atlas_images_without_exceeding_the_budget() {
        let mut terminal = Terminal::new(20, 3);
        terminal.term.graphics.total_limit = 4;
        let image = concat!(
            "\x1b]1337;File=inline=1:",
            "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg==",
            "\x07"
        );
        terminal.feed(format!("{image}{image}").as_bytes());
        assert_eq!(terminal.term.graphics.total_bytes, 4);
        let snapshot = terminal.snapshot();
        assert_eq!(snapshot.images.len(), 1);
        assert!(
            snapshot
                .warnings
                .iter()
                .any(|warning| warning.contains("released"))
        );

        // Atlas budget entries remain evictable after their pixels leave
        // Rio's pending queue, including when a Kitty upload needs the room.
        terminal.feed(b"\x1b_Ga=T,f=32,s=1,v=1,i=7;/////w==\x1b\\");
        assert!(terminal.term.graphics.kitty_images.contains_key(&7));
        assert_eq!(terminal.term.graphics.total_bytes, 4);
        assert_eq!(terminal.snapshot().images[0].image.id, 7);
    }

    #[test]
    fn native_admission_rejects_an_image_larger_than_its_storage_budget() {
        let mut terminal = Terminal::new(20, 3);
        terminal.term.graphics.total_limit = 3;
        terminal.feed(b"\x1b_Ga=T,f=32,s=1,v=1,i=7;/////w==\x1b\\");
        assert!(!terminal.term.graphics.kitty_images.contains_key(&7));
        assert_eq!(terminal.term.graphics.total_bytes, 0);
        let snapshot = terminal.snapshot();
        assert!(snapshot.images.is_empty());
        assert!(
            snapshot
                .warnings
                .iter()
                .any(|warning| warning.contains("skipped"))
        );
    }

    #[test]
    fn native_replacement_uses_the_existing_images_storage() {
        let mut terminal = Terminal::new(20, 3);
        terminal.term.graphics.total_limit = 4;
        assert!(
            terminal
                .term
                .graphics
                .store_kitty_image(7, None, pixels(7, 1, 1))
        );
        assert!(
            terminal
                .term
                .graphics
                .store_kitty_image(7, None, pixels(7, 1, 1))
        );
        assert_eq!(terminal.term.graphics.total_bytes, 4);
        assert!(!terminal.term.graphics.budget_rejected);
    }

    #[test]
    fn tiny_uploads_cannot_exceed_the_native_image_count_across_screens() {
        use rio_vt::ansi::graphics::MAX_STORED_IMAGES;
        let mut terminal = Terminal::new(20, 3);
        let mut upload = String::new();
        for id in 1..=MAX_STORED_IMAGES {
            write!(upload, "\x1b_Ga=t,f=32,s=1,v=1,i={id},q=2;/////w==\x1b\\").unwrap();
        }
        terminal.feed(upload.as_bytes());
        assert_eq!(
            terminal.term.graphics.stored_image_count(),
            MAX_STORED_IMAGES
        );
        assert_eq!(terminal.term.graphics.total_bytes, MAX_STORED_IMAGES * 4);
        // Replacing an existing ID stays possible at capacity.
        terminal.feed(b"\x1b_Ga=t,f=32,s=1,v=1,i=1,q=2;/wAA/w==\x1b\\");
        assert_eq!(
            &terminal.term.graphics.kitty_images[&1].data.pixels,
            &[255, 0, 0, 255]
        );
        terminal.feed(b"\x1b[?1049h\x1b_Ga=t,f=32,s=1,v=1,i=1025,q=2;/////w==\x1b\\");
        assert_eq!(terminal.term.graphics.kitty_images.len(), 1);
        assert_eq!(
            terminal.term.graphics.stored_image_count(),
            MAX_STORED_IMAGES
        );
        assert_eq!(terminal.term.graphics.total_bytes, MAX_STORED_IMAGES * 4);
        assert!(
            terminal
                .term
                .graphics
                .kitty_inactive_screen
                .kitty_images
                .contains_key(&1)
        );
        assert!(
            !terminal
                .term
                .graphics
                .kitty_inactive_screen
                .kitty_images
                .contains_key(&2)
        );
    }

    #[test]
    fn direct_and_virtual_placements_share_a_count_limit_across_screens() {
        use rio_vt::ansi::graphics::MAX_PLACEMENTS;
        let mut terminal = Terminal::new(20, 3);
        terminal.feed(b"\x1b_Ga=t,f=32,s=1,v=1,i=7,q=2;/////w==\x1b\\");
        let mut placements = String::new();
        for id in 1..=MAX_PLACEMENTS / 2 {
            write!(placements, "\x1b_Ga=p,i=7,p={id},C=1,q=2\x1b\\").unwrap();
            write!(placements, "\x1b_Ga=p,i=7,p={id},U=1,c=1,r=1,q=2\x1b\\").unwrap();
        }
        terminal.feed(placements.as_bytes());
        assert_eq!(terminal.term.graphics.placement_count(), MAX_PLACEMENTS);
        terminal.feed(b"\x1b_Ga=p,i=7,p=1,C=1,q=2\x1b\\");
        assert_eq!(terminal.term.graphics.placement_count(), MAX_PLACEMENTS);
        terminal.feed(b"\x1b_Ga=p,i=7,p=5000,C=1,q=2\x1b\\");
        assert!(
            !terminal
                .term
                .graphics
                .kitty_placements
                .contains_key(&(7, 5000))
        );
        terminal.feed(b"\x1b[?1049h\x1b_Ga=T,f=32,s=1,v=1,i=8,q=2;/////w==\x1b\\");
        assert!(terminal.term.graphics.kitty_placements.is_empty());
        assert_eq!(terminal.term.graphics.placement_count(), MAX_PLACEMENTS);
        assert!(
            terminal
                .snapshot()
                .warnings
                .iter()
                .any(|warning| warning.contains("4096 placements"))
        );
    }

    #[test]
    fn atlas_clipping_cannot_grow_placement_fragments_beyond_the_limit() {
        use rio_vt::ansi::graphics::{AtlasPlacement, MAX_PLACEMENTS};
        let mut terminal = Terminal::new(20, 3);
        let key = atlas_image_key(1);
        terminal.term.graphics.track_graphic(GraphicId::new(1), 4);
        terminal.term.graphics.atlas_placements = vec![
            AtlasPlacement {
                image_key: key,
                abs_row: 0,
                col: 0,
                columns: 4,
                rows: 1,
                src_x: 0,
                src_y: 0,
                src_width: 32,
                src_height: 16,
                total_width: 32,
                total_height: 16,
                insert_cell_w: 8,
                insert_cell_h: 16,
            };
            MAX_PLACEMENTS
        ];
        terminal.term.graphics.recount_atlas_keys();
        terminal.term.clip_atlas_placements(0, 1, 1, 2);
        assert_eq!(terminal.term.graphics.placement_count(), MAX_PLACEMENTS);
        assert!(terminal.term.graphics.budget_evicted_displayed);
        assert_eq!(
            terminal.term.graphics.atlas_key_refs[&key],
            MAX_PLACEMENTS as u32
        );
    }
}
