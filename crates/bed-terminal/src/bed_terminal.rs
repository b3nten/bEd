//! Shell session ownership translated from util/ned_terminal.{h,cpp}.
//! The workspace hosts each session in an independent docking window.
//! See NOTICE for host integration attribution.
use crate::{
    terminal::{Terminal, TerminalSnapshot, TerminalTheme},
    terminal_font::TerminalFonts,
    terminal_pty::{PtyEvent, PtyOptions, TerminalCommand, TerminalPty, WindowSize},
    terminal_view::{TerminalIo, TerminalView},
};
use dear_imgui_rs::{Ui, sys};
use std::{
    collections::HashMap,
    io,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

static NEXT_PANE: AtomicU64 = AtomicU64::new(0);

struct Session {
    terminal: Arc<TerminalSnapshot>,
    view: TerminalView,
    pty: Option<TerminalPty>,
    id: u64,
    pane: String,
    ended: bool,
    working_directory: Option<PathBuf>,
    ssh_target: Option<bed_remote::SshTarget>,
    command_title: Option<String>,
    last_reported_title: String,
    pending_theme: Option<TerminalTheme>,
    rendered: bool,
}
impl Session {
    fn new(id: u64, ssh_target: Option<bed_remote::SshTarget>) -> Self {
        Self {
            terminal: Terminal::new(80, 24).snapshot(),
            view: transparent_view(),
            pty: None,
            id,
            pane: format!(
                "{:x}-{:x}",
                std::process::id(),
                NEXT_PANE.fetch_add(1, Ordering::Relaxed)
            ),
            ended: false,
            working_directory: None,
            ssh_target,
            command_title: None,
            last_reported_title: "Terminal".into(),
            pending_theme: None,
            rendered: false,
        }
    }
    fn alive(&self) -> bool {
        self.pty.is_some() && !self.ended
    }
    fn title(&self) -> String {
        if let Some(title) = &self.command_title {
            return tab_title(title);
        }
        let foreground = self.pty.as_ref().and_then(|pty| {
            self.ssh_target
                .is_none()
                .then(|| pty.foreground_process_name())
                .flatten()
        });
        let title = foreground
            .as_deref()
            .or_else(|| (self.terminal.title() != "Terminal").then(|| self.terminal.title()))
            .or_else(|| self.pty.as_ref().map(TerminalPty::shell_name))
            .unwrap_or("Terminal");
        tab_title(title)
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
    window_focused: bool,
    focus_request: Option<u64>,
    needs_font_resync: bool,
    font_px: f32,
    project_root: PathBuf,
    pty_options: PtyOptions,
    ssh_target: Option<bed_remote::SshTarget>,
    theme: Option<TerminalTheme>,
    pending_links: Vec<(u64, String)>,
    retired_views: Vec<TerminalView>,
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
        self.focus_request = Some(self.sessions[self.active].id);
        self.sessions[self.active].id
    }
    pub fn new_session_at(&mut self, root: impl AsRef<Path>) -> u64 {
        self.add_session();
        self.sessions[self.active].working_directory = Some(root.as_ref().to_owned());
        self.visible = true;
        self.focus_request = Some(self.sessions[self.active].id);
        self.sessions[self.active].id
    }
    /// Launch an owned local command immediately, without a shell wrapper.
    /// Arguments and environment are passed literally through `PtyOptions`.
    /// The returned child PID can answer a DAP `runInTerminal` request. These
    /// sessions never restart automatically or through the terminal Restart UI.
    pub fn new_command_session(
        &mut self,
        options: PtyOptions,
        title: impl Into<String>,
    ) -> io::Result<(u64, u32)> {
        if options.shell.is_none() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Command terminal requires an executable",
            ));
        }
        let mut model = Terminal::new(80, 24);
        if let Some(theme) = &self.theme {
            model.set_theme(theme);
        }
        let pty = TerminalPty::spawn_terminal(&options, default_window_size(), model)?;
        let process_id = pty.process_id();
        self.add_session();
        let session = &mut self.sessions[self.active];
        session.working_directory = options.working_directory;
        session.ssh_target = None;
        session.command_title = Some(title.into());
        session.pty = Some(pty);
        self.visible = true;
        self.focus_request = Some(session.id);
        self.needs_font_resync = true;
        Ok((session.id, process_id))
    }
    /// Command transcripts should not be restored as ordinary shell sessions.
    pub fn is_command_session(&self, id: u64) -> bool {
        self.sessions
            .iter()
            .find(|session| session.id == id)
            .is_some_and(|session| session.command_title.is_some())
    }
    /// Terminate the child while retaining its terminal screen and scrollback.
    pub fn stop_session_id(&mut self, id: u64) -> bool {
        let Some(session) = self.sessions.iter_mut().find(|session| session.id == id) else {
            return false;
        };
        if let Some(pty) = session.pty.as_ref() {
            if pty.command(TerminalCommand::Stop).is_err() {
                return false;
            }
            session.terminal = pty.snapshot();
        }
        session.ended = true;
        true
    }
    pub fn session_ids(&self) -> Vec<u64> {
        self.sessions.iter().map(|s| s.id).collect()
    }
    pub fn process_id(&self, id: u64) -> Option<u32> {
        self.sessions
            .iter()
            .find(|session| session.id == id && session.alive())
            .and_then(|session| session.pty.as_ref().map(TerminalPty::process_id))
    }
    pub fn live_working_directory(&self, id: u64) -> io::Result<PathBuf> {
        let session = self
            .sessions
            .iter()
            .find(|session| session.id == id)
            .ok_or_else(|| io::Error::other("The selected terminal is no longer available"))?;
        if session.ssh_target.is_some() {
            return Err(io::Error::other(
                "Select a running local terminal to open its workspace",
            ));
        }
        let pid = self
            .process_id(id)
            .ok_or_else(|| io::Error::other("The selected terminal has no running process"))?;
        crate::process_cwd::for_process(pid)
    }
    pub fn configure_shell_environment(&mut self, env: HashMap<String, String>) {
        self.pty_options.env.extend(env);
    }
    pub fn configure_shell(&mut self, shell: crate::terminal_pty::TerminalShell) {
        self.pty_options.shell = Some(shell);
    }
    pub fn session_for_pane(&self, pane: &str) -> Option<u64> {
        self.sessions
            .iter()
            .find(|session| session.pane == pane)
            .map(|session| session.id)
    }
    /// A short display label; the host should retain the session ID as its tab ID.
    pub fn session_title(&self, id: u64) -> Option<String> {
        self.sessions
            .iter()
            .find(|session| session.id == id)
            .map(Session::title)
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
        self.focus_request = Some(id);
        true
    }

    pub fn cancel_focus_request(&mut self) {
        self.focus_request = None;
    }
    pub fn close_session_id(&mut self, id: u64) -> bool {
        let Some(index) = self.sessions.iter().position(|s| s.id == id) else {
            return false;
        };
        self.sessions[index].shutdown();
        let session = self.sessions.remove(index);
        if self.focus_request == Some(session.id) {
            self.focus_request = None;
        }
        self.retired_views.push(session.view);
        if self.sessions.is_empty() {
            self.active = 0;
            self.focus_request = None;
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
            if self.sessions[index].command_title.is_none() {
                ui.same_line();
                if ui.button(format!("Restart###restart_terminal_{id}")) {
                    self.ensure_shell(index)?;
                    self.active = index;
                    self.focus_request = Some(id);
                }
            }
        }
        // A focus request belongs to a session, independently of the order
        // visible panes are drawn. Keep it until the target canvas acquires
        // focus; Dear ImGui may apply the request on the following frame.
        if self.focus_request == Some(id) {
            ui.set_keyboard_focus_here();
        }
        let session = &mut self.sessions[index];
        session.rendered = true;
        session.view.set_window_focused(
            self.window_focused,
            &mut SessionPipe {
                pty: session.pty.as_ref(),
            },
        )?;
        let result = session.view.draw(
            ui,
            &session.terminal,
            fonts,
            &mut SessionPipe {
                pty: session.pty.as_ref(),
            },
        );
        if let Some(target) = session.view.take_open_link() {
            self.pending_links.push((id, target));
        }
        let focused = session.view.focused;
        if focused && self.focus_request.is_none_or(|requested| requested == id) {
            self.active = index;
            self.focus_request = None;
            self.blur_other_sessions(index)?;
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
            window_focused: true,
            focus_request: None,
            needs_font_resync: false,
            font_px: 0.0,
            project_root: PathBuf::new(),
            pty_options,
            ssh_target: None,
            theme: None,
            pending_links: Vec::new(),
            retired_views: Vec::new(),
        };
        terminal.add_session();
        terminal
    }
    pub fn set_project_root(&mut self, root: &str) {
        self.project_root = PathBuf::from(root);
    }
    /// Update current sessions and the defaults used by future/restarted shells.
    pub fn set_theme(
        &mut self,
        background: [f32; 4],
        foreground: [f32; 4],
        ansi: [[f32; 4]; 16],
    ) -> bool {
        let theme = TerminalTheme::new(background, foreground, ansi);
        if self.theme.as_ref() == Some(&theme) {
            return false;
        }
        for session in &mut self.sessions {
            if let Some(pty) = &session.pty {
                session.pending_theme =
                    if pty.command(TerminalCommand::Theme(theme.clone())).is_ok() {
                        None
                    } else {
                        Some(theme.clone())
                    };
            } else {
                let mut model = Terminal::new(80, 24);
                model.set_theme(&theme);
                session.terminal = model.snapshot();
            }
        }
        self.theme = Some(theme);
        true
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
            && self.window_focused
            && self
                .sessions
                .get(self.active)
                .is_some_and(|session| session.view.focused)
    }
    pub fn is_started(&self) -> bool {
        self.sessions.iter().any(Session::alive)
    }
    pub fn active_terminal(&self) -> Option<&TerminalSnapshot> {
        self.sessions.get(self.active).map(|s| s.terminal.as_ref())
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
    pub fn hide(&mut self) -> io::Result<()> {
        self.set_visible(false, false)
    }

    /// Native window focus is independent of ImGui's retained canvas focus.
    pub fn set_window_focused(&mut self, focused: bool) -> io::Result<()> {
        self.window_focused = focused;
        for session in &mut self.sessions {
            session.view.set_window_focused(
                focused && self.visible,
                &mut SessionPipe {
                    pty: session.pty.as_ref(),
                },
            )?;
        }
        Ok(())
    }

    fn blur_other_sessions(&mut self, focused: usize) -> io::Result<()> {
        for (index, session) in self.sessions.iter_mut().enumerate() {
            if index != focused {
                session.view.blur(&mut SessionPipe {
                    pty: session.pty.as_ref(),
                })?;
            }
        }
        Ok(())
    }
    pub fn toggle(&mut self) -> io::Result<()> {
        self.set_visible(!self.visible, true)
    }
    pub fn set_visible(&mut self, on: bool, focus: bool) -> io::Result<()> {
        let was_visible = self.visible;
        self.visible = on;
        if !on {
            self.focus_request = None;
            self.blur_other_sessions(usize::MAX)?;
            return Ok(());
        }
        if self.sessions.is_empty() {
            self.add_session();
        }
        if focus && !was_visible {
            self.focus_request = Some(self.sessions[self.active].id);
        }
        self.ensure_shell(self.active)
    }
    pub fn shutdown(&mut self) {
        for session in &mut self.sessions {
            session.shutdown();
        }
        self.retired_views
            .extend(self.sessions.drain(..).map(|session| session.view));
        self.active = 0;
        self.visible = false;
        self.focus_request = None;
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
        let mut session = Session::new(id, self.ssh_target.clone());
        if let Some(theme) = &self.theme {
            let mut model = Terminal::new(80, 24);
            model.set_theme(theme);
            session.terminal = model.snapshot();
        }
        self.sessions.push(session);
        self.active = self.sessions.len() - 1;
    }
    fn close_session(&mut self, index: usize) {
        if index >= self.sessions.len() {
            return;
        }
        self.sessions[index].shutdown();
        let session = self.sessions.remove(index);
        if self.focus_request == Some(session.id) {
            self.focus_request = None;
        }
        self.retired_views.push(session.view);
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
        if self.sessions[index].alive() || self.sessions[index].command_title.is_some() {
            return Ok(());
        }
        self.sessions[index].shutdown();
        let mut options = self.pty_options.clone();
        if options.env.contains_key(crate::shell_bridge::SOCKET_ENV) {
            options.env.insert(
                crate::shell_bridge::PANE_ENV.into(),
                self.sessions[index].pane.clone(),
            );
        }
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
        let mut model = Terminal::new(80, 24);
        if let Some(theme) = &self.theme {
            model.set_theme(theme);
        }
        let pty = TerminalPty::spawn_terminal(&options, default_window_size(), model)?;
        self.sessions[index].terminal = pty.snapshot();
        self.retired_views.push(std::mem::replace(
            &mut self.sessions[index].view,
            transparent_view(),
        ));
        self.sessions[index].pty = Some(pty);
        self.sessions[index].ended = false;
        self.needs_font_resync = true;
        Ok(())
    }
    /// Receive immutable state; all parsing and mutations remain on the worker.
    pub fn poll(&mut self) -> io::Result<bool> {
        let mut changed = false;
        let mut error = None;
        for session in &mut self.sessions {
            if let Some(pty) = session.pty.as_mut() {
                if let Some(theme) = &session.pending_theme {
                    match pty.command(TerminalCommand::Theme(theme.clone())) {
                        Ok(()) => session.pending_theme = None,
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                        Err(theme_error) => error = Some(theme_error),
                    }
                }
                let snapshot = pty.snapshot();
                changed |= snapshot.revision != session.terminal.revision
                    || snapshot.cursor_revision != session.terminal.cursor_revision;
                session.terminal = snapshot;
                pty.request_snapshot();
                for event in pty.poll() {
                    match event {
                        PtyEvent::Exited(_) => {
                            session.ended = true;
                            changed = true;
                        }
                        PtyEvent::Error(message) => {
                            session.ended = true;
                            error = Some(io::Error::other(message));
                        }
                        PtyEvent::Output(_) => {
                            unreachable!("Production terminal only publishes snapshots")
                        }
                    }
                }
            }
            let title = session.title();
            if title != session.last_reported_title {
                session.last_reported_title = title;
                changed = true;
            }
        }
        error.map_or(Ok(changed), Err)
    }

    pub fn prepare_frame(
        &mut self,
        context: &mut dear_imgui_rs::Context,
        fonts: &TerminalFonts,
        scale: f32,
    ) -> io::Result<()> {
        for mut view in self.retired_views.drain(..) {
            view.clear(context);
        }
        for session in &mut self.sessions {
            // Preparing textures is permitted only between frames. A pane's
            // first appearance schedules its first preparation next frame.
            if std::mem::take(&mut session.rendered) && self.visible {
                session
                    .view
                    .prepare_frame(context, &session.terminal, fonts, scale)?;
            } else {
                session.view.blur(&mut SessionPipe {
                    pty: session.pty.as_ref(),
                })?;
            }
        }
        Ok(())
    }
    pub fn invalidate_textures(&mut self) {
        for session in &mut self.sessions {
            session.view.invalidate_textures();
        }
    }
    pub fn has_pending_uploads(&self) -> bool {
        self.sessions
            .iter()
            .any(|session| session.rendered && session.view.has_pending_uploads())
    }
    pub fn take_open_links(&mut self) -> Vec<(u64, String)> {
        std::mem::take(&mut self.pending_links)
    }
    pub fn ssh_target(&self, id: u64) -> Option<&bed_remote::SshTarget> {
        self.sessions
            .iter()
            .find(|s| s.id == id)?
            .ssh_target
            .as_ref()
    }
    fn command_active(&self, command: TerminalCommand) -> io::Result<()> {
        self.sessions
            .get(self.active)
            .and_then(|s| s.pty.as_ref())
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "Terminal is not started"))?
            .command(command)
    }
    pub fn queue_key(&self, event: crate::terminal_input::TerminalKeyEvent) -> io::Result<()> {
        self.command_active(TerminalCommand::Key(event))
    }
    pub fn queue_ime_preedit(&mut self, text: String) {
        if let Some(session) = self.sessions.get_mut(self.active) {
            session.view.set_preedit(text);
        }
    }
    pub fn queue_ime_commit(&mut self, text: String) -> io::Result<()> {
        self.queue_ime_preedit(String::new());
        self.command_active(TerminalCommand::Text(text))
    }
    pub fn native_copy(&self) -> Option<String> {
        self.active_terminal()?.selection_text()
    }
    pub fn native_paste(&self, text: String) -> io::Result<()> {
        self.command_active(TerminalCommand::Paste(text))
    }
    pub fn native_select_all(&self) -> io::Result<()> {
        self.command_active(TerminalCommand::SelectAll)
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
            self.focus_request = Some(self.sessions[self.active].id);
            self.needs_font_resync = self.font_px < 6.0;
        }
        let mut i = 0;
        while i < self.sessions.len() {
            let label = std::ffi::CString::new(format!(
                "{}###bed_term_{}",
                self.sessions[i].title(),
                self.sessions[i].id
            ))
            .unwrap();
            let mut open = true;
            let open_ptr = if self.sessions.len() > 1 {
                &mut open as *mut bool
            } else {
                std::ptr::null_mut()
            };
            let flags = if self.focus_request == Some(self.sessions[i].id) {
                sys::ImGuiTabItemFlags_SetSelected
            } else {
                0
            };
            if unsafe { sys::igBeginTabItem(label.as_ptr(), open_ptr, flags) } {
                if self.active != i
                    && let Err(error) = self.blur_other_sessions(i)
                {
                    result = Err(error);
                }
                if self
                    .focus_request
                    .is_none_or(|requested| requested == self.sessions[i].id)
                {
                    self.active = i;
                }
                let id = self.sessions[i].id;
                match self.render_session(ui, fonts, id) {
                    Ok(changed) => {
                        if let Ok(value) = &mut result {
                            *value |= changed;
                        }
                    }
                    Err(error) => result = Err(error),
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

fn default_window_size() -> WindowSize {
    WindowSize {
        num_cols: 80,
        num_lines: 24,
        cell_width: 8,
        cell_height: 16,
    }
}

fn tab_title(title: &str) -> String {
    let mut label = String::new();
    for character in title
        .chars()
        .filter(|character| !character.is_control())
        .take(64)
    {
        // ImGui treats adjacent hashes as a hidden/stable label ID separator.
        if character == '#' && label.ends_with('#') {
            label.push(' ');
        }
        label.push(character);
    }
    if label.trim().is_empty() {
        "Terminal".into()
    } else {
        label
    }
}

#[cfg(test)]
mod title_tests {
    use super::*;
    #[test]
    fn tab_titles_are_bounded_and_safe_for_imgui_labels() {
        assert_eq!(tab_title("codex\0\n###title"), "codex# # #title");
        assert_eq!(tab_title(""), "Terminal");
        assert_eq!(tab_title(&"😀".repeat(100)).chars().count(), 64);
    }
}
impl Drop for BedTerminal {
    fn drop(&mut self) {
        self.shutdown();
    }
}

struct SessionPipe<'a> {
    pty: Option<&'a TerminalPty>,
}
impl TerminalIo for SessionPipe<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.pty.map_or(Ok(()), |pty| pty.write(bytes))
    }
    fn command(&mut self, command: TerminalCommand) -> io::Result<()> {
        self.pty.map_or(Ok(()), |pty| pty.command(command))
    }
    fn resize(&mut self, cols: usize, rows: usize, cw: f32, ch: f32) -> io::Result<()> {
        self.pty.map_or(Ok(()), |pty| {
            pty.resize(WindowSize {
                num_cols: cols.clamp(2, i16::MAX as usize) as u16,
                num_lines: rows.clamp(1, i16::MAX as usize) as u16,
                cell_width: cw.clamp(1.0, u16::MAX as f32) as u16,
                cell_height: ch.clamp(1.0, u16::MAX as f32) as u16,
            })
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
    fn requested_canvas_focus_survives_two_visible_panes_in_either_draw_order() {
        use dear_imgui_rs::{Condition, Context, FramePrepareOptions};
        let _guard = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = Context::create();
        context.set_ini_filename(None::<PathBuf>).unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        let mut terminal = BedTerminal::new_empty();
        let first = terminal.new_session();
        let second = terminal.new_session();
        for session in &mut terminal.sessions {
            session.ended = true;
            session.command_title = Some("Focus fixture".into());
        }
        let fonts = TerminalFonts::default();
        let frame = |context: &mut Context, terminal: &mut BedTerminal, order: [u64; 2]| {
            terminal.prepare_frame(context, &fonts, 1.0).unwrap();
            context.prepare_frame(FramePrepareOptions::new([800.0, 480.0], 1.0 / 60.0));
            let ui = context.frame();
            for id in order {
                ui.window(format!("Focus pane {id}"))
                    .position(
                        [if id == first { 0.0 } else { 400.0 }, 0.0],
                        Condition::Always,
                    )
                    .size([400.0, 480.0], Condition::Always)
                    .build(|| {
                        terminal.render_session(ui, &fonts, id).unwrap();
                    });
            }
            drop(context.render_legacy());
        };
        frame(&mut context, &mut terminal, [first, second]);
        for (requested, order) in [
            (first, [first, second]),
            (second, [first, second]),
            (first, [second, first]),
        ] {
            assert!(terminal.focus_session(requested));
            for _ in 0..8 {
                frame(&mut context, &mut terminal, order);
                assert_eq!(
                    terminal.active_session_id(),
                    Some(requested),
                    "an old focused canvas cannot take ownership from the requested pane"
                );
                if terminal.focus_request.is_none() {
                    break;
                }
            }
            assert!(
                terminal.focus_request.is_none(),
                "the requested canvas must acquire focus"
            );
            assert!(terminal.is_focused());
            assert_eq!(
                terminal
                    .sessions
                    .iter()
                    .filter(|session| session.view.focused)
                    .count(),
                1
            );
            frame(&mut context, &mut terminal, order);
            assert_eq!(terminal.active_session_id(), Some(requested));
        }
    }

    #[cfg(unix)]
    #[test]
    fn command_sessions_start_eagerly_keep_transcript_and_cannot_restart_as_shell() {
        use crate::terminal_pty::TerminalShell;
        use std::time::{Duration, Instant};
        let mut terminal = BedTerminal::new_empty();
        terminal.set_ssh_target(Some(bed_remote::SshTarget::new("unused-host")));
        let (id, pid) = terminal
            .new_command_session(
                PtyOptions {
                    shell: Some(TerminalShell::new(
                        "/bin/sh",
                        vec!["-c".into(), "printf 'DEBUG_OUTPUT'; exec sleep 10".into()],
                    )),
                    working_directory: Some(PathBuf::from("/")),
                    ..PtyOptions::default()
                },
                "Debug program",
            )
            .unwrap();
        assert!(pid > 0);
        assert!(terminal.is_started());
        assert!(terminal.is_command_session(id));
        assert!(!terminal.is_command_session(id + 1));
        assert_eq!(terminal.sessions[0].pty.as_ref().unwrap().process_id(), pid);
        assert!(terminal.sessions[0].ssh_target.is_none());
        assert_eq!(terminal.session_title(id).as_deref(), Some("Debug program"));
        let transcript = |terminal: &BedTerminal| {
            (0..12)
                .map(|col| terminal.sessions[0].terminal.cell(0, col).character)
                .collect::<String>()
        };
        let deadline = Instant::now() + Duration::from_secs(3);
        while transcript(&terminal) != "DEBUG_OUTPUT" && Instant::now() < deadline {
            terminal.poll().unwrap();
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(transcript(&terminal), "DEBUG_OUTPUT");
        assert!(terminal.stop_session_id(id));
        assert!(!terminal.is_started());
        assert!(terminal.sessions[0].pty.is_some());
        assert!(terminal.sessions[0].ended);
        terminal.ensure_shell(0).unwrap();
        terminal.hide().unwrap();
        terminal.set_visible(true, false).unwrap();
        assert!(terminal.sessions[0].pty.is_some());
        assert_eq!(transcript(&terminal), "DEBUG_OUTPUT");
        assert!(terminal.stop_session_id(id));
        assert!(!terminal.stop_session_id(id + 1));
        assert!(terminal.close_session_id(id));
        assert!(terminal.session_ids().is_empty());
    }

    #[test]
    fn command_spawn_failure_does_not_allocate_a_session_or_fallback_to_shell() {
        use crate::terminal_pty::TerminalShell;
        let mut terminal = BedTerminal::new_empty();
        assert!(
            terminal
                .new_command_session(PtyOptions::default(), "Debug")
                .is_err()
        );
        assert!(
            terminal
                .new_command_session(
                    PtyOptions {
                        shell: Some(TerminalShell::new(
                            "/missing-bed-debug-command/program",
                            Vec::new(),
                        )),
                        ..PtyOptions::default()
                    },
                    "Debug",
                )
                .is_err()
        );
        assert!(terminal.session_ids().is_empty());
        assert_eq!(terminal.new_session(), 1);
    }

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
            let mut fonts = TerminalFonts::default();
            fonts.regular = Some(font);
            fonts.bold = Some(font);
            fonts.italic = Some(font);
            fonts.bold_italic = Some(font);
            fonts.size = 13.0;
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
    fn shell_tokens_do_not_route_to_a_replacement_session() {
        let mut terminal = BedTerminal::new_empty();
        let first = terminal.new_session();
        let token = terminal.sessions[0].pane.clone();
        assert_eq!(terminal.session_for_pane(&token), Some(first));
        assert!(terminal.close_session_id(first));
        terminal.new_session();
        assert_eq!(terminal.session_for_pane(&token), None);
        let mut other_window = BedTerminal::new_empty();
        other_window.new_session();
        assert_ne!(terminal.sessions[0].pane, other_window.sessions[0].pane);
    }

    #[test]
    fn promotion_requires_a_running_local_shell() {
        let mut terminal = BedTerminal::new_empty();
        let local = terminal.new_session();
        assert!(terminal.live_working_directory(local).is_err());
        terminal.set_ssh_target(Some(bed_remote::SshTarget::new("example.invalid")));
        let remote = terminal.new_session();
        assert!(terminal.live_working_directory(remote).is_err());
        assert!(terminal.live_working_directory(u64::MAX).is_err());
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
        assert_eq!(terminal.focus_request, Some(first));
        terminal.cancel_focus_request();
        assert!(terminal.focus_request.is_none());
        assert!(terminal.focus_session(first));
        assert_eq!(terminal.focus_request, Some(first));
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
        let mut pty =
            TerminalPty::spawn_terminal(&options, default_window_size(), Terminal::new(80, 24))
                .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut ended = false;
        while !ended && Instant::now() < deadline {
            for event in pty.poll() {
                match event {
                    PtyEvent::Exited(_) => ended = true,
                    PtyEvent::Error(error) => panic!("terminal exit failed: {error}"),
                    PtyEvent::Output(_) => unreachable!("production sessions publish snapshots"),
                }
            }
            pty.request_snapshot();
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(ended);
        assert_eq!(
            (0..4)
                .map(|col| pty.snapshot().cell(0, col).character)
                .collect::<String>(),
            "TAIL"
        );
    }
    #[test]
    fn fixed_session_ids_and_active_index_survive_closing_neighbors() {
        let mut terminal = BedTerminal::new();
        assert!(terminal.visible());
        assert!(terminal.focus_request.is_none());
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
