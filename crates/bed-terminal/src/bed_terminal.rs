//! Shell session ownership translated from util/ned_terminal.{h,cpp}.
//! The workspace hosts each session in an independent docking window.
//! See NOTICE for source and terminal adapter attribution.
use crate::{
    terminal::{Terminal, TerminalEvent},
    terminal_font::TerminalFonts,
    terminal_pty::{PtyEvent, PtyOptions, TerminalPty, WindowSize},
    terminal_view::{TerminalIo, TerminalView},
};
use dear_imgui_rs::{Ui, sys};
use std::{
    io,
    path::{Path, PathBuf},
};

struct Session {
    terminal: Terminal,
    view: TerminalView,
    pty: Option<TerminalPty>,
    id: u64,
    ended: bool,
    working_directory: Option<PathBuf>,
    ssh_target: Option<bed_remote::SshTarget>,
}
impl Session {
    fn new(id: u64, ssh_target: Option<bed_remote::SshTarget>) -> Self {
        Self {
            terminal: Terminal::new(80, 24),
            view: transparent_view(),
            pty: None,
            id,
            ended: false,
            working_directory: None,
            ssh_target,
        }
    }
    fn alive(&self) -> bool {
        self.pty.is_some() && !self.ended
    }
    fn shutdown(&mut self) {
        if let Some(mut pty) = self.pty.take() {
            pty.shutdown();
        }
    }
}

pub struct BedTerminal {
    sessions: Vec<Session>,
    active: usize,
    next_id: u64,
    visible: bool,
    want_focus: bool,
    needs_font_resync: bool,
    font_px: f32,
    project_root: PathBuf,
    pty_options: PtyOptions,
    ssh_target: Option<bed_remote::SshTarget>,
}
impl Default for BedTerminal {
    fn default() -> Self {
        Self::new()
    }
}
impl BedTerminal {
    /// A panel host starts with no shell sessions; closing the last leaves it empty.
    pub fn new_empty() -> Self {
        let mut terminal = Self::with_pty_options(PtyOptions::default());
        terminal.sessions.clear();
        terminal.next_id = 1;
        terminal
    }
    /// Capture the working directory now; project switches do not move existing shells.
    pub fn new_session(&mut self) -> u64 {
        let directory = if !self.project_root.as_os_str().is_empty() {
            Some(self.project_root.clone())
        } else {
            self.pty_options
                .working_directory
                .clone()
                .or_else(|| std::env::current_dir().ok())
        };
        self.add_session();
        self.sessions[self.active].working_directory = directory;
        self.visible = true;
        self.want_focus = true;
        self.sessions[self.active].id
    }
    pub fn new_session_at(&mut self, root: impl AsRef<Path>) -> u64 {
        self.add_session();
        self.sessions[self.active].working_directory = Some(root.as_ref().to_owned());
        self.visible = true;
        self.want_focus = true;
        self.sessions[self.active].id
    }
    pub fn session_ids(&self) -> Vec<u64> {
        self.sessions.iter().map(|s| s.id).collect()
    }
    pub fn working_directory(&self, id: u64) -> Option<&Path> {
        self.sessions
            .iter()
            .find(|s| s.id == id)?
            .working_directory
            .as_deref()
    }
    pub fn focus_session(&mut self, id: u64) -> bool {
        let Some(index) = self.sessions.iter().position(|s| s.id == id) else {
            return false;
        };
        self.active = index;
        self.visible = true;
        self.want_focus = true;
        true
    }

    pub fn cancel_focus_request(&mut self) {
        self.want_focus = false;
    }
    pub fn close_session_id(&mut self, id: u64) -> bool {
        let Some(index) = self.sessions.iter().position(|s| s.id == id) else {
            return false;
        };
        self.sessions[index].shutdown();
        self.sessions.remove(index);
        if self.sessions.is_empty() {
            self.active = 0;
            self.want_focus = false;
        } else if self.active > index {
            self.active -= 1;
        } else if self.active >= self.sessions.len() {
            self.active = self.sessions.len() - 1;
        }
        true
    }
    /// Draw one ordinary docking-window body, with no nested terminal tab strip.
    pub fn render_session(&mut self, ui: &Ui, fonts: &TerminalFonts, id: u64) -> io::Result<bool> {
        let Some(index) = self.sessions.iter().position(|s| s.id == id) else {
            return Ok(false);
        };
        if self.sessions[index].pty.is_none() && !self.sessions[index].ended {
            self.ensure_shell(index)?;
        }
        if self.sessions[index].ended {
            ui.text_disabled("Session ended");
            ui.same_line();
            if ui.button(format!("Restart###restart_terminal_{id}")) {
                self.ensure_shell(index)?;
                self.active = index;
                self.want_focus = true;
            }
        }
        if self.want_focus && self.active == index {
            ui.set_keyboard_focus_here();
            self.want_focus = false;
        }
        let session = &mut self.sessions[index];
        let result = if session.ended {
            session
                .view
                .draw(ui, &mut session.terminal, fonts, &mut ClosedSessionIo)
        } else if let Some(pty) = session.pty.as_mut() {
            let mut pipe = SessionPipe {
                pty,
                ended: &mut session.ended,
            };
            session
                .view
                .draw(ui, &mut session.terminal, fonts, &mut pipe)
        } else {
            Ok(false)
        };
        if session.view.focused {
            self.active = index;
        }
        result
    }
    pub fn new() -> Self {
        Self::with_pty_options(PtyOptions::default())
    }
    pub fn with_pty_options(pty_options: PtyOptions) -> Self {
        let mut terminal = Self {
            sessions: Vec::new(),
            active: 0,
            next_id: 1,
            visible: true,
            want_focus: false,
            needs_font_resync: false,
            font_px: 0.0,
            project_root: PathBuf::new(),
            pty_options,
            ssh_target: None,
        };
        terminal.add_session();
        terminal
    }
    pub fn set_project_root(&mut self, root: &str) {
        self.project_root = PathBuf::from(root);
    }
    /// Configure future sessions. Existing sessions retain their original host.
    pub fn set_ssh_target(&mut self, target: Option<bed_remote::SshTarget>) {
        self.ssh_target = target;
    }
    pub fn visible(&self) -> bool {
        self.visible
    }
    pub fn is_visible(&self) -> bool {
        self.visible
    }
    pub fn is_focused(&self) -> bool {
        self.visible
            && self
                .sessions
                .get(self.active)
                .is_some_and(|session| session.view.focused)
    }
    pub fn is_started(&self) -> bool {
        self.sessions.iter().any(Session::alive)
    }
    pub fn active_terminal(&self) -> Option<&Terminal> {
        self.sessions.get(self.active).map(|s| &s.terminal)
    }
    pub fn active_terminal_mut(&mut self) -> Option<&mut Terminal> {
        self.sessions.get_mut(self.active).map(|s| &mut s.terminal)
    }
    pub fn write_active(&self, bytes: &[u8]) -> io::Result<()> {
        self.sessions
            .get(self.active)
            .and_then(|session| session.pty.as_ref())
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "terminal is not started"))?
            .write(bytes)
    }
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }
    pub fn active_session_id(&self) -> Option<u64> {
        self.sessions.get(self.active).map(|s| s.id)
    }
    pub fn hide(&mut self) {
        self.visible = false;
    }
    pub fn toggle(&mut self) -> io::Result<()> {
        self.set_visible(!self.visible, true)
    }
    pub fn set_visible(&mut self, on: bool, focus: bool) -> io::Result<()> {
        let was_visible = self.visible;
        self.visible = on;
        if !on {
            return Ok(());
        }
        if focus && !was_visible {
            self.want_focus = true;
        }
        if self.sessions.is_empty() {
            self.add_session();
        }
        self.ensure_shell(self.active)
    }
    pub fn shutdown(&mut self) {
        for session in &mut self.sessions {
            session.shutdown();
        }
        self.sessions.clear();
        self.active = 0;
        self.visible = false;
    }
    pub fn configured_font_px(&self) -> f32 {
        self.font_px
    }
    pub fn consume_needs_font_resync(&mut self) -> bool {
        std::mem::take(&mut self.needs_font_resync)
    }
    /// The host applies native font changes before NewFrame and retains atlas ownership.
    pub fn reload_terminal_fonts(&mut self, desired_px: f32) {
        self.font_px = if desired_px < 6.0 { 16.0 } else { desired_px };
    }
    fn add_session(&mut self) {
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .expect("terminal session ID overflow");
        self.sessions
            .push(Session::new(id, self.ssh_target.clone()));
        self.active = self.sessions.len() - 1;
    }
    fn close_session(&mut self, index: usize) {
        if index >= self.sessions.len() {
            return;
        }
        self.sessions[index].shutdown();
        self.sessions.remove(index);
        if self.sessions.is_empty() {
            self.add_session();
            return;
        }
        if self.active > index {
            self.active -= 1;
        } else if self.active >= self.sessions.len() {
            self.active = self.sessions.len() - 1;
        }
    }
    fn ensure_shell(&mut self, index: usize) -> io::Result<()> {
        if self.sessions[index].alive() {
            return Ok(());
        }
        self.sessions[index].shutdown();
        let mut options = self.pty_options.clone();
        if let Some(target) = &self.sessions[index].ssh_target {
            let root = self.sessions[index]
                .working_directory
                .as_deref()
                .unwrap_or(&self.project_root);
            let root = root.to_str().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Remote terminal root is not UTF-8",
                )
            })?;
            options = options.for_ssh(target, root)?;
        } else if let Some(directory) = &self.sessions[index].working_directory {
            options.working_directory = Some(directory.clone());
        } else if !self.project_root.as_os_str().is_empty() {
            if self.project_root.is_dir() {
                options.working_directory = Some(self.project_root.clone());
            } else {
                eprintln!(
                    "[terminal] chdir to project root failed: {}",
                    self.project_root.display()
                );
            }
        }
        let pty = TerminalPty::spawn(
            &options,
            WindowSize {
                num_cols: 80,
                num_lines: 24,
                cell_width: 8,
                cell_height: 16,
            },
        )?;
        self.sessions[index].terminal = Terminal::new(80, 24);
        self.sessions[index].view = transparent_view();
        self.sessions[index].pty = Some(pty);
        self.sessions[index].ended = false;
        self.needs_font_resync = true;
        Ok(())
    }
    /// Publish worker output on the main thread, including while the panel is hidden.
    pub fn poll(&mut self) -> io::Result<bool> {
        let mut changed = false;
        for session in &mut self.sessions {
            if let Some(pty) = session.pty.as_mut() {
                let mut pipe = SessionPipe {
                    pty,
                    ended: &mut session.ended,
                };
                changed |= pipe.pump(&mut session.terminal)?;
            }
        }
        Ok(changed)
    }
    pub fn render_panel(&mut self, ui: &Ui, fonts: &TerminalFonts) -> io::Result<bool> {
        if !self.visible {
            return Ok(false);
        }
        if self.sessions.is_empty() {
            self.add_session();
        }
        // Exact fixed non-reorderable tab flags; submit the trailing + first.
        if !unsafe {
            sys::igBeginTabBar(
                c"##bed_term_tabs".as_ptr(),
                sys::ImGuiTabBarFlags_FittingPolicyScroll
                    | sys::ImGuiTabBarFlags_DrawSelectedOverline,
            )
        } {
            return Ok(false);
        }
        let mut result = Ok(false);
        if unsafe {
            sys::igTabItemButton(
                c"+".as_ptr(),
                sys::ImGuiTabItemFlags_Trailing | sys::ImGuiTabItemFlags_NoTooltip,
            )
        } {
            self.add_session();
            if let Err(error) = self.ensure_shell(self.active) {
                result = Err(error);
            }
            self.want_focus = true;
            self.needs_font_resync = self.font_px < 6.0;
        }
        let mut i = 0;
        while i < self.sessions.len() {
            let label = std::ffi::CString::new(format!(
                "Terminal {}###bed_term_{}",
                self.sessions[i].id, self.sessions[i].id
            ))
            .unwrap();
            let mut open = true;
            let open_ptr = if self.sessions.len() > 1 {
                &mut open as *mut bool
            } else {
                std::ptr::null_mut()
            };
            if unsafe { sys::igBeginTabItem(label.as_ptr(), open_ptr, 0) } {
                self.active = i;
                for j in 0..self.sessions.len() {
                    if j == i || !self.sessions[j].alive() {
                        continue;
                    }
                    let session = &mut self.sessions[j];
                    let mut pipe = SessionPipe {
                        pty: session.pty.as_mut().unwrap(),
                        ended: &mut session.ended,
                    };
                    match session.view.tick(ui, &mut session.terminal, &mut pipe) {
                        Ok(changed) => {
                            if let Ok(value) = &mut result {
                                *value |= changed;
                            }
                        }
                        Err(error) => result = Err(error),
                    }
                }
                if let Err(error) = self.ensure_shell(i) {
                    ui.text("Terminal session ended. Click + or press Cmd/Ctrl+T.");
                    result = Err(error);
                } else {
                    if self.want_focus {
                        ui.set_keyboard_focus_here();
                        self.want_focus = false;
                    }
                    let session = &mut self.sessions[i];
                    let mut pipe = SessionPipe {
                        pty: session.pty.as_mut().unwrap(),
                        ended: &mut session.ended,
                    };
                    match session
                        .view
                        .draw(ui, &mut session.terminal, fonts, &mut pipe)
                    {
                        Ok(changed) => {
                            if let Ok(value) = &mut result {
                                *value |= changed;
                            }
                        }
                        Err(error) => result = Err(error),
                    }
                }
                unsafe {
                    sys::igEndTabItem();
                }
            }
            if !open {
                self.close_session(i);
                continue;
            }
            i += 1;
        }
        unsafe {
            sys::igEndTabBar();
        }
        result
    }
}
impl Drop for BedTerminal {
    fn drop(&mut self) {
        self.shutdown();
    }
}

struct ClosedSessionIo;
impl TerminalIo for ClosedSessionIo {
    fn pump(&mut self, _: &mut Terminal) -> io::Result<bool> {
        Ok(false)
    }
    fn write(&mut self, _: &[u8]) -> io::Result<()> {
        Ok(())
    }
    fn resize(&mut self, _: usize, _: usize, _: f32, _: f32) -> io::Result<()> {
        Ok(())
    }
}

struct SessionPipe<'a> {
    pty: &'a mut TerminalPty,
    ended: &'a mut bool,
}
impl TerminalIo for SessionPipe<'_> {
    fn pump(&mut self, terminal: &mut Terminal) -> io::Result<bool> {
        let mut changed = false;
        let mut error = None;
        for event in self.pty.poll() {
            match event {
                PtyEvent::Output(bytes) => {
                    changed = true;
                    for event in terminal.feed(&bytes) {
                        if let TerminalEvent::Write(bytes) = event
                            && self.pty.is_alive()
                            && let Err(reply_error) = self.pty.write(&bytes)
                        {
                            // Keep draining this batch, including its Exited event. A
                            // query in a child's final output can race its shutdown.
                            if self.pty.is_alive() {
                                error = Some(reply_error);
                            }
                        }
                    }
                }
                PtyEvent::Exited(_) => {
                    *self.ended = true;
                    changed = true;
                }
                PtyEvent::Error(message) => {
                    *self.ended = true;
                    // Preserve any following final output or exit notification.
                    error = Some(io::Error::other(message));
                }
            }
        }
        error.map_or(Ok(changed), Err)
    }
    fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.pty.write(bytes)
    }
    fn resize(
        &mut self,
        cols: usize,
        rows: usize,
        cell_width: f32,
        cell_height: f32,
    ) -> io::Result<()> {
        self.pty.resize(WindowSize {
            num_cols: cols.min(u16::MAX as usize) as u16,
            num_lines: rows.min(u16::MAX as usize) as u16,
            cell_width: cell_width.clamp(1.0, u16::MAX as f32) as u16,
            cell_height: cell_height.clamp(1.0, u16::MAX as f32) as u16,
        })
    }
}

fn transparent_view() -> TerminalView {
    let mut view = TerminalView::default();
    view.transparent_background = true;
    view
}

#[cfg(test)]
mod tests {
    #[test]
    fn sessions_capture_their_target_and_root_across_project_switches() {
        let mut terminal = BedTerminal::new_empty();
        terminal.set_project_root("/local/project");
        let local = terminal.new_session();
        let target = bed_remote::SshTarget::new("user@host");
        terminal.set_ssh_target(Some(target.clone()));
        terminal.set_project_root("/remote/project");
        let remote = terminal.new_session();
        terminal.set_ssh_target(None);
        terminal.set_project_root("/another/local");
        assert_eq!(
            terminal.working_directory(local),
            Some(Path::new("/local/project"))
        );
        assert_eq!(
            terminal.working_directory(remote),
            Some(Path::new("/remote/project"))
        );
        assert_eq!(terminal.sessions[0].ssh_target, None);
        assert_eq!(terminal.sessions[1].ssh_target, Some(target));
    }
    #[cfg(unix)]
    #[test]
    fn ended_panel_keeps_output_without_automatically_restarting_shell() {
        use crate::terminal_pty::TerminalShell;
        use dear_imgui_rs::{Condition, Context, FramePrepareOptions};
        use std::time::{Duration, Instant};
        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = Context::create();
        context.set_ini_filename(None::<PathBuf>).unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        let mut terminal = BedTerminal::new_empty();
        terminal.pty_options.shell = Some(TerminalShell::new(
            "/bin/sh",
            vec!["-c".into(), "printf ENDED".into()],
        ));
        let id = terminal.new_session();
        let render = |context: &mut Context, terminal: &mut BedTerminal| {
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
            ui.window("Terminal panel")
                .position([0.0; 2], Condition::Always)
                .size([640.0, 480.0], Condition::Always)
                .build(|| {
                    terminal.render_session(ui, &fonts, id).unwrap();
                });
            drop(context.render_legacy());
        };
        render(&mut context, &mut terminal);
        let pid = terminal.sessions[0].pty.as_ref().unwrap().process_id();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !terminal.sessions[0].ended && Instant::now() < deadline {
            terminal.poll().unwrap();
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(terminal.sessions[0].ended);
        let before = (0..5)
            .map(|column| terminal.sessions[0].terminal.cell(0, column).character)
            .collect::<String>();
        assert_eq!(before, "ENDED");
        for _ in 0..4 {
            render(&mut context, &mut terminal);
        }
        assert!(terminal.sessions[0].ended);
        assert_eq!(terminal.sessions[0].pty.as_ref().unwrap().process_id(), pid);
        assert_eq!(
            (0..5)
                .map(|column| terminal.sessions[0].terminal.cell(0, column).character)
                .collect::<String>(),
            before
        );
        assert!(terminal.close_session_id(id));
        assert!(terminal.session_ids().is_empty());
    }
    #[test]
    fn panel_sessions_keep_creation_directories_and_allow_zero_sessions() {
        let mut terminal = super::BedTerminal::new_empty();
        assert!(terminal.session_ids().is_empty());
        terminal.set_project_root("/first-project");
        let first = terminal.new_session();
        terminal.set_project_root("/second-project");
        let second = terminal.new_session();
        assert_eq!(
            terminal.working_directory(first),
            Some(std::path::Path::new("/first-project"))
        );
        assert_eq!(
            terminal.working_directory(second),
            Some(std::path::Path::new("/second-project"))
        );
        assert!(terminal.focus_session(first));
        assert!(terminal.want_focus);
        terminal.cancel_focus_request();
        assert!(!terminal.want_focus);
        assert!(terminal.focus_session(first));
        assert!(terminal.want_focus);
        assert!(terminal.close_session_id(second));
        assert_eq!(terminal.active_session_id(), Some(first));
        assert!(terminal.close_session_id(first));
        assert_eq!(terminal.session_count(), 0);
        assert_eq!(terminal.active_session_id(), None);
        assert!(!terminal.close_session_id(first));
        assert!(!terminal.focus_session(first));
        let third = terminal.new_session_at("/explicit-project");
        assert!(third > second);
        assert_eq!(
            terminal.working_directory(third),
            Some(std::path::Path::new("/explicit-project"))
        );
        assert!(!terminal.is_started());
    }
    use super::*;
    #[cfg(unix)]
    #[test]
    fn final_query_output_does_not_drop_exit_or_following_text() {
        use crate::terminal_pty::TerminalShell;
        use std::time::{Duration, Instant};
        let options = PtyOptions {
            shell: Some(TerminalShell::new(
                "/bin/sh",
                vec!["-c".into(), r"printf '\033[6nTAIL'".into()],
            )),
            ..PtyOptions::default()
        };
        let mut pty = TerminalPty::spawn(
            &options,
            WindowSize {
                num_cols: 80,
                num_lines: 24,
                cell_width: 8,
                cell_height: 16,
            },
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while pty.is_alive() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(!pty.is_alive());
        let mut terminal = Terminal::new(80, 24);
        let mut ended = false;
        while !ended && Instant::now() < deadline {
            SessionPipe {
                pty: &mut pty,
                ended: &mut ended,
            }
            .pump(&mut terminal)
            .unwrap();
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(ended);
        assert_eq!(
            (0..4)
                .map(|col| terminal.cell(0, col).character)
                .collect::<String>(),
            "TAIL"
        );
    }
    #[test]
    fn fixed_session_ids_and_active_index_survive_closing_neighbors() {
        let mut terminal = BedTerminal::new();
        assert!(terminal.visible());
        assert!(!terminal.want_focus);
        assert_eq!(terminal.active_session_id(), Some(1));
        terminal.add_session();
        terminal.add_session();
        assert_eq!(terminal.active_session_id(), Some(3));
        terminal.close_session(0);
        assert_eq!(terminal.session_count(), 2);
        assert_eq!(terminal.active_session_id(), Some(3));
        terminal.close_session(1);
        assert_eq!(terminal.active_session_id(), Some(2));
        terminal.close_session(0);
        assert_eq!(terminal.session_count(), 1);
        assert_eq!(terminal.active_session_id(), Some(4));
        terminal.shutdown();
        terminal.shutdown();
        assert!(!terminal.visible());
        assert_eq!(terminal.session_count(), 0);
        terminal.add_session();
        assert_eq!(terminal.active_session_id(), Some(5));
    }
    #[test]
    fn font_resync_is_consumed_once_and_hiding_keeps_sessions() {
        let mut terminal = BedTerminal::new();
        terminal.set_visible(false, true).unwrap();
        assert_eq!(terminal.session_count(), 1);
        assert!(!terminal.consume_needs_font_resync());
        terminal.needs_font_resync = true;
        assert!(terminal.consume_needs_font_resync());
        assert!(!terminal.consume_needs_font_resync());
        terminal.reload_terminal_fonts(5.0);
        assert_eq!(terminal.configured_font_px(), 16.0);
        terminal.reload_terminal_fonts(32.0);
        assert_eq!(terminal.configured_font_px(), 32.0);
        assert!(!terminal.is_started());
    }
}
