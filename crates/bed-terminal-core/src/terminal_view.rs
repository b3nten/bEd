//! ImGui terminal canvas. Terminal mutations are ordered worker commands;
//! drawing consumes immutable, prepared viewport rows.
use crate::{
    terminal::{SelectionSnap, TerminalModes, TerminalSnapshot},
    terminal_font::TerminalFonts,
    terminal_input::{TerminalKey, TerminalKeyEvent},
    terminal_links::{LinkCell, LinkRow, TerminalLink, collect_links},
    terminal_pty::TerminalCommand,
    terminal_renderer::TerminalRenderer,
};
use dear_imgui_rs::{Context, MouseButton, Ui, sys};
use std::io;

const BORDER: f32 = 2.0;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum MouseGesture {
    #[default]
    None,
    Local,
    Application {
        sgr: bool,
        x10: bool,
    },
}
#[derive(Clone, Copy, Debug)]
pub struct PaintMetrics {
    pub cw: f32,
    pub ch: f32,
    pub ascent: f32,
    pub width: f32,
    pub height: f32,
}

pub trait TerminalIo {
    fn write(&mut self, bytes: &[u8]) -> io::Result<()>;
    fn command(&mut self, command: TerminalCommand) -> io::Result<()>;
    fn resize(&mut self, cols: usize, rows: usize, cw: f32, ch: f32) -> io::Result<()>;
}

pub struct TerminalView {
    pub transparent_background: bool,
    pub focused: bool,
    renderer: TerminalRenderer,
    geometry: PaintMetrics,
    grid: [usize; 2],
    density: f32,
    viewport_scale: Option<f32>,
    prepared: bool,
    canvas_focused: bool,
    window_focused: bool,
    reported_focus: bool,
    gestures: [MouseGesture; 3],
    rectangular: bool,
    last_motion: [usize; 2],
    scroll_remainder: f32,
    mouse_wheel_remainder: f32,
    last_cursor_revision: u64,
    cursor_started: f64,
    links_revision: u64,
    links: Vec<TerminalLink>,
    open_link: Option<String>,
    context_link: Option<String>,
    preedit: String,
}
impl Default for TerminalView {
    fn default() -> Self {
        Self {
            transparent_background: false,
            focused: false,
            renderer: TerminalRenderer::default(),
            geometry: PaintMetrics {
                cw: 8.0,
                ch: 16.0,
                ascent: 12.0,
                width: 640.0,
                height: 384.0,
            },
            grid: [0, 0],
            density: 1.0,
            viewport_scale: None,
            prepared: false,
            canvas_focused: false,
            window_focused: true,
            reported_focus: false,
            gestures: [MouseGesture::None; 3],
            rectangular: false,
            last_motion: [usize::MAX; 2],
            scroll_remainder: 0.0,
            mouse_wheel_remainder: 0.0,
            last_cursor_revision: 0,
            cursor_started: 0.0,
            links_revision: 0,
            links: Vec::new(),
            open_link: None,
            context_link: None,
            preedit: String::new(),
        }
    }
}
impl TerminalView {
    pub fn prepare_frame(
        &mut self,
        context: &mut Context,
        snapshot: &TerminalSnapshot,
        fonts: &TerminalFonts,
        scale: f32,
    ) -> io::Result<()> {
        self.renderer.prepare(
            context,
            snapshot,
            fonts,
            self.viewport_scale.unwrap_or(scale),
        )?;
        self.prepared = true;
        if self.links_revision != snapshot.revision {
            self.links = collect_links(
                &snapshot
                    .lines
                    .iter()
                    .map(|row| LinkRow {
                        cells: row
                            .cells
                            .iter()
                            .map(|cell| LinkCell {
                                text: &cell.text,
                                width: cell.width,
                                explicit: cell.hyperlink.as_deref(),
                            })
                            .collect(),
                        wrapped: row.wrapped,
                    })
                    .collect::<Vec<_>>(),
            );
            self.links_revision = snapshot.revision;
        }
        Ok(())
    }
    pub fn invalidate_textures(&mut self) {
        self.renderer.invalidate_textures();
        self.prepared = false;
    }
    pub fn clear(&mut self, context: &mut Context) {
        self.renderer.clear(context);
    }
    pub fn set_preedit(&mut self, text: String) {
        self.preedit = text;
    }
    pub fn take_open_link(&mut self) -> Option<String> {
        self.open_link.take()
    }
    pub fn has_pending_uploads(&self) -> bool {
        !self.prepared
            || self
                .viewport_scale
                .is_some_and(|scale| scale != self.renderer.density())
            || self.renderer.has_pending_uploads()
    }

    fn sync_focus(&mut self, io: &mut impl TerminalIo) -> io::Result<bool> {
        let focused = self.canvas_focused && self.window_focused;
        let changed = focused != self.focused;
        self.focused = focused;
        if focused != self.reported_focus {
            io.command(TerminalCommand::Focus(focused))?;
            self.reported_focus = focused;
        }
        if changed {
            self.preedit.clear();
        }
        Ok(changed)
    }

    pub(crate) fn set_window_focused(
        &mut self,
        focused: bool,
        io: &mut impl TerminalIo,
    ) -> io::Result<()> {
        self.window_focused = focused;
        if !focused {
            self.end_gestures(io)?;
        }
        self.sync_focus(io)?;
        Ok(())
    }

    pub(crate) fn blur(&mut self, io: &mut impl TerminalIo) -> io::Result<()> {
        self.canvas_focused = false;
        self.end_gestures(io)?;
        self.sync_focus(io)?;
        Ok(())
    }

    fn end_gestures(&mut self, io: &mut impl TerminalIo) -> io::Result<()> {
        let [col, row] = self.last_motion;
        for index in 0..self.gestures.len() {
            match self.gestures[index] {
                MouseGesture::Application { sgr, x10: false } => {
                    io.write(&mouse_packet(index as u8, col, row, 0, true, sgr))?;
                }
                MouseGesture::Local if index == 0 => {
                    io.command(TerminalCommand::SelectionExtend {
                        col,
                        row,
                        rectangular: self.rectangular,
                        done: true,
                    })?;
                }
                _ => {}
            }
            self.gestures[index] = MouseGesture::None;
        }
        Ok(())
    }

    pub fn draw(
        &mut self,
        ui: &Ui,
        terminal: &TerminalSnapshot,
        _fonts: &TerminalFonts,
        io: &mut impl TerminalIo,
    ) -> io::Result<bool> {
        let avail = ui.content_region_avail();
        let [cw, ch] = self.renderer.metrics();
        let density = self.renderer.density();
        let cols = ((avail[0] - 2.0 * BORDER) / cw).floor().max(2.0) as usize;
        let rows = ((avail[1] - 2.0 * BORDER) / ch).floor().max(1.0) as usize;
        // Monitor/UI DPI and framebuffer density differ on platforms whose
        // desktop coordinates are already physical pixels. Rasterize for the
        // actual framebuffer, as the ImGui font loader does.
        let viewport_scale = ui.window_viewport().framebuffer_scale()[0];
        if viewport_scale.is_finite() && viewport_scale > 0.0 {
            self.viewport_scale = Some(viewport_scale);
        }
        let mut changed = self.has_pending_uploads();
        if self.grid != [cols, rows]
            || self.geometry.cw != cw
            || self.geometry.ch != ch
            || self.density != density
        {
            io.resize(cols, rows, (cw * density).round(), (ch * density).round())?;
            self.density = density;
            self.grid = [cols, rows];
            changed = true;
        }
        self.geometry = PaintMetrics {
            cw,
            ch,
            ascent: ch * 0.8,
            width: avail[0],
            height: avail[1],
        };
        let origin = ui.cursor_screen_pos();
        unsafe {
            sys::igInvisibleButton(
                c"##term_canvas".as_ptr(),
                sys::ImVec2 {
                    x: avail[0].max(1.0),
                    y: avail[1].max(1.0),
                },
                sys::ImGuiButtonFlags_MouseButtonLeft
                    | sys::ImGuiButtonFlags_MouseButtonRight
                    | sys::ImGuiButtonFlags_MouseButtonMiddle
                    | sys::ImGuiButtonFlags_EnableNav,
            );
        }
        self.canvas_focused = ui.is_item_focused();
        changed |= self.sync_focus(io)?;
        let focused = self.focused;
        if focused {
            unsafe {
                sys::igSetNextFrameWantCaptureKeyboard(true);
            }
        }
        if self.last_cursor_revision != terminal.cursor_revision {
            self.last_cursor_revision = terminal.cursor_revision;
            self.cursor_started = ui.time();
        }
        let cursor_on = (((ui.time() - self.cursor_started) / 0.75) as u64).is_multiple_of(2);
        let blink = !((ui.time() / 0.8) as u64).is_multiple_of(2);
        changed |= self.dispatch_mouse(ui, terminal, io, origin)?;
        let content = [origin[0] + BORDER, origin[1] + BORDER];
        let clipmax = [origin[0] + avail[0], origin[1] + avail[1]];
        self.renderer.draw(
            ui,
            terminal,
            content,
            clipmax,
            focused,
            cursor_on,
            blink,
            self.transparent_background,
        );
        self.draw_hover(ui, terminal, origin);
        if let Some(warning) = terminal
            .warnings
            .last()
            .map(|warning| warning.as_ref())
            .or_else(|| self.renderer.warning())
        {
            ui.get_window_draw_list().add_text(
                [content[0], (clipmax[1] - ch).max(content[1])],
                0xff70b7ff,
                warning,
            );
        }
        if focused && !self.preedit.is_empty() {
            let p = [
                content[0] + terminal.cursor.col as f32 * cw,
                content[1] + terminal.cursor.row as f32 * ch,
            ];
            let draw = ui.get_window_draw_list();
            draw.add_rect(
                p,
                [p[0] + ui.calc_text_size(&self.preedit)[0], p[1] + ch],
                0xe6222222,
            )
            .filled(true)
            .build();
            draw.add_text(p, 0xffffffff, &self.preedit);
            draw.add_line(
                [p[0], p[1] + ch - 1.0],
                [p[0] + ui.calc_text_size(&self.preedit)[0], p[1] + ch - 1.0],
                0xffffffff,
            )
            .build();
        }
        Ok(changed)
    }

    fn cell_at(&self, ui: &Ui, terminal: &TerminalSnapshot, origin: [f32; 2]) -> [usize; 2] {
        let [mut col, row] = pixel_to_cell(
            ui.io().mouse_pos(),
            origin,
            self.geometry,
            terminal.cols,
            terminal.rows,
        );
        while col > 0 && terminal.lines[row].cells[col].width == 0 {
            col -= 1;
        }
        [col, row]
    }
    fn hovered_link(&self, row: usize, col: usize) -> Option<&TerminalLink> {
        self.links.iter().find(|link| link.contains(row, col))
    }
    fn draw_hover(&self, ui: &Ui, terminal: &TerminalSnapshot, origin: [f32; 2]) {
        if !ui.is_item_hovered() {
            return;
        }
        let [col, row] = self.cell_at(ui, terminal, origin);
        if let Some(link) = self.hovered_link(row, col) {
            ui.set_mouse_cursor(Some(dear_imgui_rs::MouseCursor::Hand));
            ui.tooltip_text(&link.target);
            let draw = ui.get_window_draw_list();
            for span in &link.spans {
                let y = origin[1] + BORDER + (span.row + 1) as f32 * self.geometry.ch - 1.0;
                draw.add_line(
                    [origin[0] + BORDER + span.col as f32 * self.geometry.cw, y],
                    [
                        origin[0] + BORDER + span.end_col as f32 * self.geometry.cw,
                        y,
                    ],
                    0xffffffff,
                )
                .build();
            }
        }
    }
    fn paste(&self, io: &mut impl TerminalIo) -> io::Result<()> {
        if let Some(text) = clipboard_text() {
            io.command(TerminalCommand::Paste(text))?;
        }
        Ok(())
    }
    fn dispatch_mouse(
        &mut self,
        ui: &Ui,
        terminal: &TerminalSnapshot,
        io: &mut impl TerminalIo,
        origin: [f32; 2],
    ) -> io::Result<bool> {
        let modes = terminal.modes;
        let input = ui.io();
        let hovered = ui.is_item_hovered();
        let [col, row] = self.cell_at(ui, terminal, origin);
        let mouse_mode = mouse_mode_bits(&modes);
        let local = mouse_mode == 0 || input.key_shift();
        let link_modifier = input.key_ctrl(); // macOS Command is ImGui's logical Ctrl.
        if hovered
            && link_modifier
            && ui.is_mouse_clicked(MouseButton::Left)
            && let Some(link) = self.hovered_link(row, col)
        {
            self.open_link = Some(link.target.clone());
            return Ok(true);
        }
        let mut changed = false;
        let buttons = [MouseButton::Left, MouseButton::Middle, MouseButton::Right];
        let mods = u8::from(input.key_shift()) * 4
            + u8::from(input.key_alt()) * 8
            + u8::from(input.key_super() || input.key_ctrl()) * 16;

        // Choose ownership once, at the press. Shift changes during a drag and
        // clicks in another pane cannot turn into terminal motion or releases.
        for (code, button) in buttons.iter().enumerate() {
            if hovered && ui.is_mouse_clicked(*button) {
                if !local {
                    io.write(&mouse_packet(
                        code as u8,
                        col,
                        row,
                        if modes.mouse_x10 { 0 } else { mods },
                        false,
                        modes.mouse_sgr,
                    ))?;
                    self.gestures[code] = MouseGesture::Application {
                        sgr: modes.mouse_sgr,
                        x10: modes.mouse_x10,
                    };
                } else {
                    match button {
                        MouseButton::Left => {
                            let count = unsafe { (*sys::igGetIO_Nil()).MouseClickedCount[0] };
                            self.rectangular = input.key_alt();
                            io.command(TerminalCommand::SelectionStart {
                                col,
                                row,
                                snap: if count >= 3 {
                                    SelectionSnap::Line
                                } else if count == 2 {
                                    SelectionSnap::Word
                                } else {
                                    SelectionSnap::None
                                },
                            })?;
                        }
                        MouseButton::Right => {
                            self.context_link =
                                self.hovered_link(row, col).map(|link| link.target.clone());
                            ui.open_popup("terminal_context");
                        }
                        _ => {}
                    }
                    self.gestures[code] = MouseGesture::Local;
                }
                self.last_motion = [col, row];
                changed = true;
            }
            if ui.is_mouse_released(*button) {
                match self.gestures[code] {
                    MouseGesture::Application { sgr, x10: false } => {
                        io.write(&mouse_packet(code as u8, col, row, mods, true, sgr))?;
                        changed = true;
                    }
                    MouseGesture::Local if code == 0 => {
                        io.command(TerminalCommand::SelectionExtend {
                            col,
                            row,
                            rectangular: self.rectangular,
                            done: true,
                        })?;
                        changed = true;
                    }
                    MouseGesture::Local if code == 1 && hovered => {
                        self.paste(io)?;
                        changed = true;
                    }
                    _ => {}
                }
                self.gestures[code] = MouseGesture::None;
            }
        }
        if self.gestures[0] == MouseGesture::Local && ui.is_mouse_dragging(MouseButton::Left) {
            io.command(TerminalCommand::SelectionExtend {
                col,
                row,
                rectangular: self.rectangular,
                done: false,
            })?;
            changed = true;
        }
        if hovered
            && self.last_motion != [col, row]
            && (modes.mouse_motion || modes.mouse_many)
            && (!local
                || self
                    .gestures
                    .iter()
                    .any(|owner| matches!(owner, MouseGesture::Application { .. })))
        {
            let held = self
                .gestures
                .iter()
                .position(|owner| matches!(owner, MouseGesture::Application { .. }))
                .map(|index| index as u8)
                .or_else(|| {
                    (modes.mouse_many && self.gestures == [MouseGesture::None; 3]).then_some(3)
                });
            if let Some(code) = held {
                io.write(&mouse_packet(
                    code + 32,
                    col,
                    row,
                    mods,
                    false,
                    modes.mouse_sgr,
                ))?;
                changed = true;
            }
        }
        self.last_motion = [col, row];
        if hovered {
            if local {
                let lines = wheel_steps(&mut self.scroll_remainder, input.mouse_wheel() * 3.0);
                if modes.alt_screen {
                    let key = if lines > 0 {
                        TerminalKey::Up
                    } else {
                        TerminalKey::Down
                    };
                    for _ in 0..lines.unsigned_abs() {
                        io.command(TerminalCommand::Key(TerminalKeyEvent {
                            key: key.clone(),
                            ..TerminalKeyEvent::default()
                        }))?;
                    }
                } else if lines != 0 {
                    io.command(TerminalCommand::Scroll(lines))?;
                }
                changed |= lines != 0;
            } else {
                let steps = wheel_steps(&mut self.mouse_wheel_remainder, input.mouse_wheel());
                let code = if steps > 0 { 64 } else { 65 };
                for _ in 0..steps.unsigned_abs() {
                    io.write(&mouse_packet(
                        code,
                        col,
                        row,
                        if modes.mouse_x10 { 0 } else { mods },
                        false,
                        modes.mouse_sgr,
                    ))?;
                }
                changed |= steps != 0;
            }
        }
        if let Some(_popup) = ui.begin_popup("terminal_context") {
            if ui.menu_item("Copy")
                && let Some(text) = terminal.selection_text()
            {
                set_clipboard_text(&text);
            }
            if ui.menu_item("Paste") {
                self.paste(io)?;
            }
            if ui.menu_item("Select All") {
                io.command(TerminalCommand::SelectAll)?;
            }
            if let Some(target) = &self.context_link {
                if ui.menu_item("Open Link") {
                    self.open_link = Some(target.clone());
                }
                if ui.menu_item("Copy Link") {
                    set_clipboard_text(target);
                }
            }
        }
        Ok(changed)
    }
}

fn wheel_steps(remainder: &mut f32, delta: f32) -> i32 {
    *remainder += delta;
    let steps = remainder.trunc().clamp(-32.0, 32.0) as i32;
    *remainder -= steps as f32;
    steps
}

pub fn pixel_to_cell(
    pos: [f32; 2],
    origin: [f32; 2],
    m: PaintMetrics,
    cols: usize,
    rows: usize,
) -> [usize; 2] {
    let pitch = [m.cw.max(1.0), m.ch.max(1.0)];
    let limits = [cols.saturating_sub(1), rows.saturating_sub(1)];
    std::array::from_fn(|axis| {
        let relative = pos[axis] - (origin[axis] + BORDER);
        let cell = (relative / pitch[axis]).floor().max(0.0) as usize;
        cell.min(limits[axis])
    })
}
fn mouse_mode_bits(m: &TerminalModes) -> u8 {
    [m.mouse_x10, m.mouse_button, m.mouse_motion, m.mouse_many]
        .into_iter()
        .enumerate()
        .fold(0, |mask, (bit, enabled)| mask | (u8::from(enabled) << bit))
}
/// Xterm mouse reports: SGR keeps button identity on release; the legacy
/// byte protocol uses release button 3 and clamps coordinates to 223.
pub fn mouse_packet(
    button: u8,
    col: usize,
    row: usize,
    mods: u8,
    release: bool,
    sgr: bool,
) -> Vec<u8> {
    if sgr {
        let code = button + mods;
        let x = col.saturating_add(1);
        let y = row.saturating_add(1);
        let terminator = if release { "m" } else { "M" };
        return format!("\x1b[<{code};{x};{y}{terminator}").into_bytes();
    }
    let code = if release { 3 } else { button };
    let mut report = b"\x1b[M".to_vec();
    report.push(32 + code + mods);
    report.extend([col, row].map(|value| 32 + value.saturating_add(1).min(223) as u8));
    report
}
pub(crate) fn clipboard_text() -> Option<String> {
    let pointer = unsafe { sys::igGetClipboardText() };
    if pointer.is_null() {
        return None;
    }
    let text = unsafe { std::ffi::CStr::from_ptr(pointer) };
    Some(text.to_string_lossy().into_owned())
}
pub(crate) fn set_clipboard_text(text: &str) {
    let prefix = text.split('\0').next().expect("split has a first part");
    let value = std::ffi::CString::new(prefix).expect("prefix contains no NUL");
    unsafe {
        sys::igSetClipboardText(value.as_ptr());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct RecordedIo {
        writes: Vec<Vec<u8>>,
        commands: Vec<TerminalCommand>,
    }
    impl TerminalIo for RecordedIo {
        fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
            self.writes.push(bytes.to_vec());
            Ok(())
        }
        fn command(&mut self, command: TerminalCommand) -> io::Result<()> {
            self.commands.push(command);
            Ok(())
        }
        fn resize(&mut self, _: usize, _: usize, _: f32, _: f32) -> io::Result<()> {
            Ok(())
        }
    }
    #[test]
    fn font_density_follows_the_framebuffer_instead_of_monitor_dpi() {
        use dear_imgui_rs::{Condition, FramePrepareOptions};
        let _guard = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = Context::create();
        context
            .set_ini_filename(None::<std::path::PathBuf>)
            .unwrap();
        let mut fonts = TerminalFonts::default();
        let root = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."));
        fonts.reload(&mut context, root, 20.0).unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        let snapshot = crate::terminal::Terminal::new(80, 24).snapshot();
        let mut view = TerminalView::default();
        let mut io = RecordedIo::default();
        view.prepare_frame(&mut context, &snapshot, &fonts, 2.0)
            .unwrap();
        context.prepare_frame(FramePrepareOptions::new([640.0, 480.0], 1.0 / 60.0));
        // Windows/Linux physical desktop coordinates can have a 2x monitor
        // scale but a 1x framebuffer. The backing density is authoritative.
        context.main_viewport().set_dpi_scale(2.0);
        context.main_viewport().set_framebuffer_scale([1.0, 1.0]);
        let ui = context.frame();
        ui.window("Density terminal")
            .position([0.0, 0.0], Condition::Always)
            .size([320.0, 240.0], Condition::Always)
            .build(|| view.draw(ui, &snapshot, &fonts, &mut io).unwrap());
        drop(context.render_legacy());
        view.prepare_frame(&mut context, &snapshot, &fonts, 2.0)
            .unwrap();
        assert_eq!(view.renderer.density(), 1.0);
        view.clear(&mut context);
    }
    #[test]
    fn native_focus_loss_preserves_canvas_focus_but_hiding_does_not() {
        let mut view = TerminalView::default();
        let mut io = RecordedIo::default();
        view.canvas_focused = true;
        view.sync_focus(&mut io).unwrap();
        view.set_window_focused(false, &mut io).unwrap();
        assert!(!view.focused);
        view.set_window_focused(true, &mut io).unwrap();
        assert!(view.focused);
        view.blur(&mut io).unwrap();
        view.set_window_focused(true, &mut io).unwrap();
        assert!(!view.focused);
        assert_eq!(
            io.commands
                .iter()
                .filter_map(|command| match command {
                    TerminalCommand::Focus(focused) => Some(*focused),
                    _ => None,
                })
                .collect::<Vec<_>>(),
            [true, false, true, false]
        );
    }
    #[test]
    fn wheel_accumulates_fractional_motion_and_retains_burst_remainder() {
        let mut remainder = 0.0;
        for _ in 0..3 {
            assert_eq!(wheel_steps(&mut remainder, 0.25), 0);
        }
        assert_eq!(wheel_steps(&mut remainder, 0.25), 1);
        assert_eq!(wheel_steps(&mut remainder, -0.5), 0);
        assert_eq!(wheel_steps(&mut remainder, -0.5), -1);
        assert_eq!(wheel_steps(&mut remainder, 40.0), 32);
        assert_eq!(wheel_steps(&mut remainder, 0.0), 8);
        assert_eq!(remainder, 0.0);
    }

    #[test]
    fn terminal_mouse_gestures_keep_their_press_owner_across_shift_changes() {
        use dear_imgui_rs::{Condition, FramePrepareOptions, Key};
        let _guard = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = Context::create();
        context
            .set_ini_filename(None::<std::path::PathBuf>)
            .unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        let mut terminal = crate::terminal::Terminal::new(80, 24);
        terminal.feed(b"\x1b[?1002h\x1b[?1006h");
        let snapshot = terminal.snapshot();
        let mut view = TerminalView::default();
        let mut io = RecordedIo::default();
        let frame = |context: &mut Context, view: &mut TerminalView, io: &mut RecordedIo| {
            context.prepare_frame(FramePrepareOptions::new([640.0, 480.0], 1.0 / 60.0));
            let ui = context.frame();
            let mut target = [0.0; 2];
            ui.window("Mouse terminal")
                .position([0.0, 0.0], Condition::Always)
                .size([320.0, 240.0], Condition::Always)
                .build(|| {
                    let origin = ui.cursor_screen_pos();
                    target = [origin[0] + 20.0, origin[1] + 20.0];
                    view.draw(ui, &snapshot, &TerminalFonts::default(), io)
                        .unwrap();
                });
            drop(context.render_legacy());
            target
        };
        let target = frame(&mut context, &mut view, &mut io);
        frame(&mut context, &mut view, &mut io);
        context.io_mut().add_mouse_pos_event([600.0, 400.0]);
        frame(&mut context, &mut view, &mut io);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, true);
        frame(&mut context, &mut view, &mut io);
        context.io_mut().add_mouse_pos_event(target);
        frame(&mut context, &mut view, &mut io);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, false);
        frame(&mut context, &mut view, &mut io);
        assert!(
            io.writes.is_empty(),
            "a press in another pane has no terminal release"
        );

        context.io_mut().add_key_event(Key::ModShift, true);
        frame(&mut context, &mut view, &mut io);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, true);
        frame(&mut context, &mut view, &mut io);
        context.io_mut().add_key_event(Key::ModShift, false);
        context
            .io_mut()
            .add_mouse_pos_event([target[0] + 40.0, target[1]]);
        frame(&mut context, &mut view, &mut io);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, false);
        frame(&mut context, &mut view, &mut io);
        assert!(
            io.writes.is_empty(),
            "releasing Shift must not change a local drag to app motion"
        );
        assert!(
            io.commands
                .iter()
                .any(|command| matches!(command, TerminalCommand::SelectionStart { .. }))
        );
        assert!(
            io.commands.iter().any(|command| matches!(
                command,
                TerminalCommand::SelectionExtend { done: true, .. }
            ))
        );
        io.commands.clear();

        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, true);
        frame(&mut context, &mut view, &mut io);
        context.io_mut().add_key_event(Key::ModShift, true);
        frame(&mut context, &mut view, &mut io);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, false);
        frame(&mut context, &mut view, &mut io);
        assert_eq!(io.writes.len(), 2);
        assert_eq!(io.writes[0].last(), Some(&b'M'));
        assert_eq!(io.writes[1].last(), Some(&b'm'));
        assert!(
            !io.commands
                .iter()
                .any(|command| matches!(command, TerminalCommand::SelectionStart { .. }))
        );
    }
    #[test]
    fn cell_hit_testing_clips_negative_and_outside_coordinates() {
        let m = PaintMetrics {
            cw: 8.0,
            ch: 16.0,
            ascent: 12.0,
            width: 800.0,
            height: 400.0,
        };
        assert_eq!(
            pixel_to_cell([-100.0, -100.0], [0.0, 0.0], m, 80, 24),
            [0, 0]
        );
        assert_eq!(
            pixel_to_cell([1000.0, 1000.0], [0.0, 0.0], m, 80, 24),
            [79, 23]
        );
    }
    #[test]
    fn sgr_mouse_keeps_large_coordinates_and_release_identity() {
        assert_eq!(
            mouse_packet(0, 300, 255, 16, true, true),
            b"\x1b[<16;301;256m"
        );
        assert_eq!(
            mouse_packet(2, 2, 3, 0, false, false),
            vec![27, b'[', b'M', 34, 35, 36]
        );
    }
}
