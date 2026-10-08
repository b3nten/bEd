//! Workspace-owned debugging. Editor widgets receive presentation data only.
use super::{Panel, Tool, Workbench};
use bed_core::editor_events::Overlay;
use bed_debug::{
    BuildEvent, BuildJob, BuildRequest, CargoDiscovery, CargoLaunch, CargoWorkspace, DebugEvent,
    DebugProfile, DebugSession, EvaluateContext, LaunchConfig, SessionState, SourceBreakpoint,
};
use bed_session::editor_session::{DocumentId, SessionEvent, ViewId};
use bed_terminal::terminal_pty::{PtyOptions, TerminalShell};
use bed_ui::{
    editor_view::{EditorView, SourceDebugPresentation},
    views::hover_tooltip::{HoverRect, hover_key_pressed, hover_rect_contains, render_hover_popup},
};
use dear_imgui_rs::{Key, TreeNodeFlags, Ui, sys};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io,
    path::{Path, PathBuf},
};

#[path = "debugger_panel.rs"]
mod panel;
#[path = "debugger_paths.rs"]
mod paths;
#[path = "debugger_source.rs"]
mod source;
use source::SourceDebugState;

#[cfg(test)]
#[path = "debugger_tests.rs"]
mod workbench_tests;

const LOG_LIMIT: usize = 2 * 1024 * 1024;

#[derive(Default)]
pub(super) struct Debugger {
    profiles: Vec<DebugProfile>,
    selected: usize,
    watches: Vec<String>,
    adapter_path: String,
    last_saved: Option<Value>,
    source: SourceDebugState,
    session: Option<DebugSession>,
    build: Option<BuildJob>,
    launch_profile: Option<DebugProfile>,
    terminal: Option<u64>,
    discovery: Option<CargoDiscovery>,
    cargo_workspace: Option<CargoWorkspace>,
    build_log: String,
    console_log: String,
    error: Option<String>,
    source_notice: Option<String>,
    source_paths: HashMap<i64, PathBuf>,
    configure: bool,
    console_input: String,
    console_commands: bool,
    watch_input: String,
    inspected_stop: Option<(u64, Option<i64>)>,
    first_stop: Option<u64>,
    evaluations: HashMap<u64, EvaluationPurpose>,
    watch_values: HashMap<String, String>,
    hover: Option<RuntimeHover>,
    breakpoint_requests: HashMap<u64, (String, Vec<u64>)>,
    adapter_breakpoints: HashMap<i64, (String, u64)>,
    navigation: Option<(String, i32, i32)>,
}

enum EvaluationPurpose {
    Watch(String),
    Console,
    Hover(HoverKey),
}

#[derive(Clone, PartialEq, Eq)]
struct HoverKey {
    view: ViewId,
    document: DocumentId,
    revision: (u64, u64),
    frame: i64,
    stop: u64,
    expression: String,
}
struct RuntimeHover {
    key: HoverKey,
    value: Option<String>,
    anchor: [f32; 2],
    rect: Option<HoverRect>,
    last_frame: usize,
}

#[derive(Clone)]
enum Action {
    Start,
    Stop,
    Restart,
    Continue,
    Pause,
    Over,
    Into,
    Out,
    Discover,
    Frame(i64),
    Thread(i64),
    Variables(i64),
    Evaluate(String, EvaluateContext),
    Watch(String),
    RemoveWatch(usize),
    BreakpointEnabled(String, u64, bool),
    RemoveBreakpoint(String, u64),
    ClearBreakpoints,
    Navigate(String, i32),
    ShowTerminal,
}

impl Debugger {
    pub(super) fn owns_terminal(&self, id: u64) -> bool {
        self.terminal == Some(id)
    }
    pub(super) fn active(&self) -> bool {
        self.build.is_some()
            || self.session.as_ref().is_some_and(|s| {
                !matches!(s.state, SessionState::Terminated | SessionState::Failed)
            })
    }
    fn settings(&self) -> Value {
        json!({"version":1,"profiles":self.profiles,"selected":self.selected,
            "watches":self.watches,"adapter_path":self.adapter_path})
    }
    fn load(&mut self, value: Option<&Value>) {
        if let Some(value) = value {
            self.profiles = serde_json::from_value(value["profiles"].clone()).unwrap_or_default();
            self.selected = value["selected"].as_u64().unwrap_or(0) as usize;
            self.watches = serde_json::from_value(value["watches"].clone()).unwrap_or_default();
            self.adapter_path = value["adapter_path"].as_str().unwrap_or_default().into();
        }
        if self.profiles.is_empty() {
            self.profiles.push(DebugProfile::default());
            self.configure = true;
        }
        self.selected = self.selected.min(self.profiles.len() - 1);
        self.last_saved = Some(self.settings());
    }
}

impl Workbench {
    /// Native render fixture: use an isolated configured project and place a
    /// breakpoint at the fixture's DEBUG_BREAKPOINT comment before launching.
    pub fn debug_smoke_setup(&mut self) -> io::Result<()> {
        if let Some(snapshot) = self.active_snapshot() {
            let row = String::from_utf8_lossy(&snapshot.bytes)
                .lines()
                .position(|line| line.contains("DEBUG_BREAKPOINT"))
                .unwrap_or(0) as i32;
            self.toggle_debug_breakpoint(snapshot.id, row)?;
        }
        self.show_debugger();
        self.start_debugger()
    }

    pub fn debug_smoke_ready(&self) -> io::Result<bool> {
        if let Some(error) = &self.debugger.error {
            let terminal = self
                .terminal
                .active_terminal()
                .map(|terminal| {
                    (0..terminal.rows().min(40))
                        .map(|row| {
                            (0..terminal.cols())
                                .map(|col| terminal.cell(row, col).character)
                                .collect::<String>()
                                .trim_end()
                                .to_owned()
                        })
                        .filter(|line| !line.is_empty())
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default();
            return Err(io::Error::other(format!(
                "Debugger smoke failed: {error}\n{terminal}"
            )));
        }
        Ok(self.debugger.session.as_ref().is_some_and(|session| {
            session.state == SessionState::Stopped
                && session.selected_frame.is_some()
                && !session.scopes.is_empty()
                && session
                    .scopes
                    .iter()
                    .filter(|scope| !scope.expensive && scope.variables_reference > 0)
                    .all(|scope| session.variables.contains_key(&scope.variables_reference))
        }))
    }

    pub(super) fn show_debugger(&mut self) {
        let existed = self.panel_visible("debug");
        self.show_tool(Tool::Debug);
        if !existed
            && self.dock_built
            && self.center_dock != 0
            && let Some(binding) = &self.context_binding
        {
            let mut bottom = 0;
            let mut center = 0;
            // New debugger panels get a useful source/inspection split. Existing
            // saved docking is left to the ordinary panel/layout machinery.
            binding.with_bound_context(|| unsafe {
                sys::igDockBuilderSplitNode(
                    self.center_dock,
                    sys::ImGuiDir_Down,
                    0.4,
                    &mut bottom,
                    &mut center,
                );
                sys::igDockBuilderFinish(self.dock_root);
            });
            self.center_dock = center;
            if let Some(tab) = self
                .tabs
                .iter_mut()
                .find(|t| matches!(t.panel, Panel::Tool(Tool::Debug)))
            {
                tab.dock = Some(bottom);
            }
        }
        if self.dock_built
            && let Some(binding) = &self.context_binding
            && let Some(tab) = self
                .tabs
                .iter()
                .find(|t| matches!(t.panel, Panel::Tool(Tool::Debug)))
        {
            // Source navigation takes keyboard focus after a stop. Select the
            // Debug dock tab separately so that navigation cannot hide it.
            let name = std::ffi::CString::new(format!("###bed_tab_{}", tab.id)).unwrap();
            binding.with_bound_context(|| unsafe {
                let window = sys::igFindWindowByName(name.as_ptr());
                if !window.is_null() {
                    let bar = super::bed_imgui_dock_node_tab_bar((*window).DockNode);
                    if !bar.is_null() {
                        (*bar).SelectedTabId = (*window).TabId;
                        (*bar).NextSelectedTabId = (*window).TabId;
                    }
                }
            });
        }
        if self.debugger.profiles.is_empty() {
            self.debugger.load(None);
        }
        if self.debugger.cargo_workspace.is_none()
            && self.debugger.discovery.is_none()
            && self.debugger.profiles[self.debugger.selected]
                .cargo
                .is_some()
            && let Err(error) = self.debug_action(Action::Discover)
        {
            self.debugger.error = Some(error.to_string());
        }
    }

    pub(super) fn restore_debugger_settings(&mut self) {
        self.debugger = Debugger::default();
        let value = self
            .workspace_spec
            .as_ref()
            .and_then(|spec| self.store.as_ref()?.debug_settings(spec));
        self.debugger.load(value);
        if value.is_none() && Path::new(&self.project_root).join("Cargo.toml").is_file() {
            self.debugger.profiles[0].cargo = Some(CargoLaunch::default());
        }
    }

    pub(super) fn persist_debugger_settings(&mut self) -> io::Result<()> {
        if self.session.is_remote() || self.debugger.profiles.is_empty() {
            return Ok(());
        }
        let value = self.debugger.settings();
        if self.debugger.last_saved.as_ref() != Some(&value)
            && let (Some(store), Some(spec)) = (&mut self.store, &self.workspace_spec)
        {
            store.save_debug_settings(spec, value.clone())?;
            self.debugger.last_saved = Some(value);
        }
        Ok(())
    }

    pub(super) fn stop_debugger(&mut self) {
        if let Some(job) = self.debugger.build.take() {
            job.cancel();
        }
        if let Some(mut session) = self.debugger.session.take() {
            let _ = session.disconnect();
        }
        if let Some(id) = self.debugger.terminal.take() {
            self.terminal.stop_session_id(id);
            if !self
                .tabs
                .iter()
                .any(|tab| matches!(tab.panel, Panel::Terminal(terminal) if terminal == id))
            {
                self.terminal.close_session_id(id);
            }
        }
        self.debugger.launch_profile = None;
        self.debugger.source.end_launch();
        self.debugger.evaluations.clear();
        self.debugger.breakpoint_requests.clear();
        self.debugger.adapter_breakpoints.clear();
        self.debugger.watch_values.clear();
        self.debugger.hover = None;
        self.debugger.inspected_stop = None;
        self.debugger.first_stop = None;
        self.debugger.source_notice = None;
        self.debugger.source_paths.clear();
        self.scene += 1;
    }

    fn start_debugger(&mut self) -> io::Result<()> {
        if !cfg!(any(target_os = "macos", target_os = "linux")) || self.session.is_remote() {
            return Err(io::Error::other(
                "Debugging is available for local macOS and Linux projects",
            ));
        }
        if self.project_root.is_empty() {
            return Err(io::Error::other("Open a local project before debugging"));
        }
        if self.debugger.profiles.is_empty() {
            self.debugger.load(None);
        }
        let mut profile = self.debugger.profiles[self.debugger.selected].clone();
        if profile
            .cargo
            .as_ref()
            .is_some_and(|c| c.target.kind.is_test())
            && !profile.test_filter.trim().is_empty()
        {
            profile.args.insert(0, profile.test_filter.trim().into());
        }
        let request = BuildRequest::for_profile(&profile, Path::new(&self.project_root))?;
        self.stop_debugger();
        for document in self.session.document_ids() {
            let snapshot = self.session.snapshot(document)?;
            if snapshot.dirty && !snapshot.path.is_empty() {
                self.session.save(document)?;
            }
        }
        self.debugger.source.begin_launch(&self.session)?;
        self.debugger.error = None;
        self.debugger.build_log.clear();
        self.debugger.console_log.clear();
        self.debugger.build = Some(BuildJob::start(request)?);
        self.debugger.launch_profile = Some(profile);
        self.debugger.configure = false;
        self.scene += 1;
        Ok(())
    }

    pub(super) fn debug_document_events(&mut self, events: &[SessionEvent]) -> io::Result<()> {
        if self.session.is_remote()
            || !cfg!(any(target_os = "macos", target_os = "linux"))
            || (!self.debugger.active() && self.debugger.source.breakpoints.is_empty())
        {
            return Ok(());
        }
        let paths = self.debugger.source.on_events(&self.session, events)?;
        if !paths.is_empty() {
            self.debugger.hover = None;
        }
        for path in paths {
            self.sync_debug_breakpoints(&path)?;
        }
        Ok(())
    }

    pub(super) fn toggle_debug_breakpoint(
        &mut self,
        document: DocumentId,
        row: i32,
    ) -> io::Result<()> {
        if self.session.is_remote() || !cfg!(any(target_os = "macos", target_os = "linux")) {
            return Ok(());
        }
        let (path, column) = self.session.with_document(document, |state| {
            let line = state.line(row);
            let prefix = line
                .iter()
                .position(|b| !b.is_ascii_whitespace())
                .unwrap_or(0);
            (
                state.path.clone(),
                String::from_utf8_lossy(&line[..prefix])
                    .encode_utf16()
                    .count() as i32,
            )
        })?;
        if path.is_empty() {
            self.debugger.error = Some("Save this file before adding breakpoints".into());
            return Ok(());
        }
        self.debugger.source.toggle(&path, row, column);
        self.sync_debug_breakpoints(&path)?;
        self.scene += 1;
        Ok(())
    }

    pub(super) fn debug_source_presentation(
        &self,
        document: DocumentId,
    ) -> io::Result<Option<SourceDebugPresentation>> {
        if self.session.is_remote() || !cfg!(any(target_os = "macos", target_os = "linux")) {
            return Ok(None);
        }
        let path = self
            .session
            .with_document(document, |state| state.path.clone())?;
        if path.is_empty() {
            return Ok(None);
        }
        let row = self.debugger.session.as_ref().and_then(|s| {
            if s.state != SessionState::Stopped {
                return None;
            }
            s.frames
                .iter()
                .find(|f| Some(f.id) == s.selected_frame)
                .filter(|f| {
                    self.debugger
                        .source_paths
                        .get(&f.id)
                        .is_some_and(|source| source == Path::new(&path))
                })
                .map(|f| f.line.saturating_sub(1) as i32)
        });
        Ok(Some(self.debugger.source.presentation(&path, row)))
    }

    pub(super) fn debug_shortcuts(&mut self, ui: &Ui) -> io::Result<()> {
        if self.focused_terminal()
            || self.active_overlay() != Overlay::None
            || self.file_dialog.is_some()
            || self.reload_confirmation.is_some()
            || self.file_explorer.file_finder.show_ff_window
            || ui.io().want_text_input()
            || ui.io().key_ctrl()
            || ui.io().key_super()
            || ui.io().key_alt()
        {
            return Ok(());
        }
        let pressed = |key| ui.is_key_pressed_with_repeat(key, false);
        let action = if pressed(Key::F9) {
            if let Some(view) = self.active_view() {
                let document = self.session.document_for_view(view).unwrap();
                let row = self.session.view_snapshot(view)?.primary().head_row;
                self.toggle_debug_breakpoint(document, row)?;
            }
            None
        } else if pressed(Key::F5) {
            Some(if ui.io().key_shift() {
                Action::Stop
            } else if self
                .debugger
                .session
                .as_ref()
                .is_some_and(|s| s.state == SessionState::Stopped)
            {
                Action::Continue
            } else if self.debugger.active() {
                return Ok(());
            } else {
                Action::Start
            })
        } else if pressed(Key::F10) {
            Some(Action::Over)
        } else if pressed(Key::F11) {
            Some(if ui.io().key_shift() {
                Action::Out
            } else {
                Action::Into
            })
        } else {
            None
        };
        if let Some(action) = action {
            if matches!(action, Action::Start) {
                self.show_debugger();
            }
            if let Err(error) = self.debug_action(action) {
                self.debugger.error = Some(error.to_string());
            }
        }
        Ok(())
    }

    pub(super) fn draw_debugger(&mut self, ui: &Ui) -> io::Result<()> {
        if self.session.is_remote() || !cfg!(any(target_os = "macos", target_os = "linux")) {
            ui.text_wrapped("Debugging is available for local macOS and Linux projects.");
            return Ok(());
        }
        if self.project_root.is_empty() {
            ui.text_disabled("Open a local project to configure debugging");
            return Ok(());
        }
        if self.debugger.profiles.is_empty() {
            self.debugger.load(None);
        }
        let actions = self.debugger.draw(ui, Path::new(&self.project_root));
        for action in actions {
            if let Err(error) = self.debug_action(action) {
                self.debugger.error = Some(error.to_string());
            }
        }
        Ok(())
    }

    fn sync_debug_breakpoints(&mut self, path: &str) -> io::Result<()> {
        let bindings = self.debugger.source.bindings(path);
        if let Some(session) = &mut self.debugger.session
            && !session.state.is_finished()
        {
            let points = bindings
                .iter()
                .map(|(_, row)| SourceBreakpoint {
                    line: (*row).max(0) as usize + 1,
                    ..Default::default()
                })
                .collect();
            let request = session.set_breakpoints(path, points)?;
            if request != 0 {
                self.debugger
                    .breakpoint_requests
                    .retain(|_, (pending_path, _)| pending_path != path);
                self.debugger.breakpoint_requests.insert(
                    request,
                    (
                        path.to_owned(),
                        bindings.into_iter().map(|(id, _)| id).collect(),
                    ),
                );
            }
        }
        Ok(())
    }

    fn debug_action(&mut self, action: Action) -> io::Result<()> {
        match action {
            Action::Start | Action::Restart => self.start_debugger()?,
            Action::Stop => self.stop_debugger(),
            Action::Discover => {
                let profile = &self.debugger.profiles[self.debugger.selected];
                let cargo = profile
                    .cargo
                    .as_ref()
                    .ok_or_else(|| io::Error::other("Choose a Cargo profile"))?;
                self.debugger.discovery = Some(CargoDiscovery::start(&absolute(
                    &self.project_root,
                    &cargo.manifest_path,
                ))?);
                self.debugger.error = None;
            }
            Action::Continue | Action::Pause | Action::Over | Action::Into | Action::Out => {
                if let Some(session) = &mut self.debugger.session {
                    match action {
                        Action::Continue => session.continue_execution()?,
                        Action::Pause => session.pause()?,
                        Action::Over => session.step_over()?,
                        Action::Into => session.step_in()?,
                        Action::Out => session.step_out()?,
                        _ => unreachable!(),
                    };
                    self.debugger.hover = None;
                    self.debugger.watch_values.clear();
                    self.debugger.evaluations.clear();
                }
            }
            Action::Frame(id) => {
                if let Some(session) = &mut self.debugger.session {
                    session.select_frame(id)?;
                }
                self.debugger.inspected_stop = None;
                self.debugger.hover = None;
                self.debugger.evaluations.clear();
            }
            Action::Thread(id) => {
                if let Some(session) = &mut self.debugger.session {
                    session.select_thread(id)?;
                }
                self.debugger.inspected_stop = None;
                self.debugger.hover = None;
                self.debugger.evaluations.clear();
            }
            Action::Variables(reference) => {
                if let Some(session) = &mut self.debugger.session {
                    session.load_variables(reference)?;
                }
            }
            Action::Evaluate(expression, context) => {
                let session = self
                    .debugger
                    .session
                    .as_mut()
                    .ok_or_else(|| io::Error::other("Start a debugging session first"))?;
                let request = session.evaluate(expression, context, session.selected_frame)?;
                self.debugger
                    .evaluations
                    .insert(request, EvaluationPurpose::Console);
            }
            Action::Watch(expression) => {
                let expression = expression.trim().to_owned();
                if !expression.is_empty() && !self.debugger.watches.contains(&expression) {
                    self.debugger.watches.push(expression);
                    self.debugger.inspected_stop = None;
                }
            }
            Action::RemoveWatch(index) => {
                if index < self.debugger.watches.len() {
                    self.debugger.watches.remove(index);
                }
            }
            Action::BreakpointEnabled(path, id, enabled) => {
                self.debugger.source.set_enabled(&path, id, enabled);
                self.sync_debug_breakpoints(&path)?;
            }
            Action::RemoveBreakpoint(path, id) => {
                self.debugger.source.remove(&path, id);
                self.sync_debug_breakpoints(&path)?;
            }
            Action::ClearBreakpoints => {
                let paths: Vec<_> = self.debugger.source.breakpoints.keys().cloned().collect();
                self.debugger.source.clear();
                for path in paths {
                    self.sync_debug_breakpoints(&path)?;
                }
            }
            Action::Navigate(path, row) => self.navigate_file(&path, row, 0, false)?,
            Action::ShowTerminal => {
                if let Some(id) = self.debugger.terminal {
                    if let Some(index) = self
                        .tabs
                        .iter()
                        .position(|t| matches!(t.panel,Panel::Terminal(tid) if tid==id))
                    {
                        self.switch_to_tab(index);
                    } else if self.terminal.session_ids().contains(&id) {
                        self.push_panel(Panel::Terminal(id));
                    }
                }
            }
        }
        self.scene += 1;
        Ok(())
    }

    pub(super) fn tick_debugger(&mut self) -> io::Result<()> {
        if let Some(discovery) = &mut self.debugger.discovery
            && discovery.poll()
            && let Some(result) = discovery.outcome().cloned()
        {
            self.debugger.discovery = None;
            match result {
                Ok(workspace) => {
                    if let Some(profile) = self.debugger.profiles.get_mut(self.debugger.selected)
                        && let Some(cargo) = &mut profile.cargo
                        && cargo.package.is_empty()
                        && let Some(target) = workspace.targets.first()
                    {
                        cargo.package = target.package.clone();
                        cargo.target = target.target.clone();
                    }
                    self.debugger.cargo_workspace = Some(workspace);
                }
                Err(error) => self.debugger.error = Some(error),
            }
            self.scene += 1;
        }
        let events = self
            .debugger
            .build
            .as_mut()
            .map(BuildJob::poll)
            .unwrap_or_default();
        for event in events {
            match event {
                BuildEvent::Output(output) => append_log(&mut self.debugger.build_log, &output),
                BuildEvent::Finished(result) => {
                    self.debugger.build = None;
                    match result {
                        Ok(artifact) => {
                            if let Err(error) = self.launch_debug_artifact(artifact) {
                                self.debugger.error = Some(error.to_string());
                                self.debugger.source.end_launch();
                            }
                        }
                        Err(error) => {
                            append_log(&mut self.debugger.build_log, &format!("\n{error}\n"));
                            self.debugger.error = Some(error);
                            self.debugger.source.end_launch();
                        }
                    }
                }
            }
            self.scene += 1;
        }
        let events = self
            .debugger
            .session
            .as_mut()
            .map(DebugSession::poll)
            .unwrap_or_default();
        let inspection = self.debugger.session.as_ref().and_then(|s| {
            (s.state == SessionState::Stopped).then_some((s.stop_generation, s.selected_frame))
        });
        if !events.is_empty() || inspection != self.debugger.inspected_stop {
            let workspace = Path::new(&self.project_root);
            let profile = self.debugger.launch_profile.as_ref();
            let cwd = profile
                .filter(|profile| !profile.cwd.is_empty())
                .map(|profile| absolute(&self.project_root, &profile.cwd))
                .unwrap_or_else(|| workspace.to_owned());
            let source_map = profile
                .map(|profile| profile.source_map.as_slice())
                .unwrap_or_default();
            // Keep the adapter's raw paths intact: applying mappings to an
            // already mapped frame on the next event can map it a second time.
            self.debugger.source_paths = self
                .debugger
                .session
                .as_ref()
                .into_iter()
                .flat_map(|session| &session.frames)
                .filter_map(|frame| {
                    Some((
                        frame.id,
                        paths::resolve_source_path(
                            frame.source.as_deref()?,
                            workspace,
                            &cwd,
                            source_map,
                        )?,
                    ))
                })
                .collect();
        }
        let entry_stop = inspection.is_some_and(|(generation, _)| {
            *self.debugger.first_stop.get_or_insert(generation) == generation
                && self
                    .debugger
                    .launch_profile
                    .as_ref()
                    .is_some_and(|profile| profile.stop_on_entry)
        });
        if inspection != self.debugger.inspected_stop {
            self.debugger.inspected_stop = inspection;
            self.debugger.hover = None;
            self.debugger.evaluations.clear();
            self.debugger.watch_values.clear();
            self.debugger.source_notice = None;
            if let Some((_, Some(frame_id))) = inspection {
                if let Some(session) = &mut self.debugger.session {
                    if let Some(frame) = session.frames.iter().find(|f| f.id == frame_id) {
                        if let Some(path) = self.debugger.source_paths.get(&frame.id) {
                            self.debugger.navigation = Some((
                                path.to_string_lossy().into(),
                                frame.line.saturating_sub(1) as i32,
                                frame.column.saturating_sub(1) as i32,
                            ));
                        } else {
                            self.debugger.source_notice =
                                Some(frame_source_notice(frame.source.as_deref(), entry_stop));
                        }
                    }
                    for expression in &self.debugger.watches {
                        match session.evaluate(
                            expression.clone(),
                            EvaluateContext::Watch,
                            Some(frame_id),
                        ) {
                            Ok(id) => {
                                self.debugger
                                    .evaluations
                                    .insert(id, EvaluationPurpose::Watch(expression.clone()));
                            }
                            Err(error) => {
                                self.debugger
                                    .watch_values
                                    .insert(expression.clone(), error.to_string());
                            }
                        }
                    }
                }
                self.show_debugger();
                if self.debugger.source_notice.is_some()
                    && let Some(index) = self.active_tab_index()
                {
                    self.switch_to_tab(index);
                }
            }
        }
        for event in events {
            match event {
                DebugEvent::Changed => {}
                DebugEvent::Output { category, output } => append_log(
                    &mut self.debugger.console_log,
                    &format!("[{category}] {output}"),
                ),
                DebugEvent::Error(error) => {
                    append_log(&mut self.debugger.console_log, &format!("{error}\n"));
                    self.debugger.error = Some(error);
                }
                DebugEvent::RunInTerminal(request) => {
                    let args = std::env::current_exe()
                        .ok()
                        .and_then(|host| bed_debug::launcher::arguments(&request.args, &host))
                        .unwrap_or(request.args);
                    let mut options = PtyOptions {
                        working_directory: (!request.cwd.is_empty())
                            .then(|| PathBuf::from(&request.cwd)),
                        shell: Some(TerminalShell::new(&args[0], args[1..].to_vec())),
                        ..Default::default()
                    };
                    for (name, value) in request.env {
                        if let Some(value) = value {
                            options.env.insert(name, value);
                        } else {
                            options.env_remove.push(name);
                        }
                    }
                    let title = program_terminal_title(self.debugger.launch_profile.as_ref());
                    let result = self.terminal.new_command_session(options, title);
                    let reply = match result {
                        Ok((id, pid)) => {
                            self.debugger.terminal = Some(id);
                            self.push_panel(Panel::Terminal(id));
                            Ok(pid)
                        }
                        Err(error) => Err(error.to_string()),
                    };
                    if let Some(session) = &mut self.debugger.session {
                        session.respond_run_in_terminal(request.request_seq, reply)?;
                    }
                }
                DebugEvent::Breakpoints {
                    path,
                    breakpoints,
                    request_id,
                } => {
                    let ids: Vec<_> = self
                        .debugger
                        .breakpoint_requests
                        .remove(&request_id)
                        .map(|(_, ids)| ids)
                        .unwrap_or_else(|| {
                            self.debugger
                                .source
                                .bindings(&path)
                                .into_iter()
                                .map(|(id, _)| id)
                                .collect()
                        });
                    let results: Vec<_> = breakpoints
                        .iter()
                        .map(|b| {
                            (
                                b.verified,
                                b.line.map(|r| r.saturating_sub(1) as i32),
                                b.message.clone().unwrap_or_default(),
                            )
                        })
                        .collect();
                    for (id, result) in ids.iter().zip(&breakpoints) {
                        if let Some(adapter_id) = result.id {
                            self.debugger
                                .adapter_breakpoints
                                .insert(adapter_id, (path.clone(), *id));
                        }
                    }
                    self.debugger.source.apply_result(&path, &ids, &results);
                }
                DebugEvent::BreakpointChanged { breakpoint, .. } => {
                    if let Some((path, id)) = breakpoint
                        .id
                        .and_then(|id| self.debugger.adapter_breakpoints.get(&id))
                        .cloned()
                    {
                        self.debugger.source.apply_result(
                            &path,
                            &[id],
                            &[(
                                breakpoint.verified,
                                breakpoint.line.map(|r| r.saturating_sub(1) as i32),
                                breakpoint.message.unwrap_or_default(),
                            )],
                        );
                    }
                }
                DebugEvent::Evaluated { request_id, result } => {
                    if let Some(purpose) = self.debugger.evaluations.remove(&request_id) {
                        let succeeded = result.is_ok();
                        let value = result
                            .map(|v| v.result)
                            .unwrap_or_else(|e| format!("Error: {e}"));
                        match purpose {
                            EvaluationPurpose::Watch(expression) => {
                                self.debugger.watch_values.insert(expression, value);
                            }
                            EvaluationPurpose::Console => {
                                append_log(&mut self.debugger.console_log, &format!("{value}\n"))
                            }
                            EvaluationPurpose::Hover(key) => {
                                if let Some(hover) = &mut self.debugger.hover
                                    && hover.key == key
                                    && self.session.document_revision(key.document).ok()
                                        == Some(key.revision)
                                {
                                    hover.value = succeeded.then_some(value);
                                }
                            }
                        }
                    }
                }
            }
            self.scene += 1;
        }
        if let Some((path, row, column)) = self.debugger.navigation.take() {
            if Path::new(&path).is_file() {
                if let Err(error) = self.navigate_file(&path, row, column, true) {
                    self.debugger.source_notice =
                        Some(format!("Unable to open source: {path}. {error}"));
                }
            } else {
                self.debugger.source_notice = Some(frame_source_notice(Some(&path), false));
            }
        }
        if self
            .debugger
            .session
            .as_ref()
            .is_some_and(|s| s.state.is_finished())
        {
            self.debugger.source.end_launch();
            self.debugger.hover = None;
            self.debugger.source_notice = None;
            self.debugger.source_paths.clear();
            if self
                .debugger
                .session
                .as_ref()
                .is_some_and(|s| s.state == SessionState::Failed)
                && let Some(id) = self.debugger.terminal
            {
                self.terminal.stop_session_id(id);
            }
        }
        Ok(())
    }

    fn launch_debug_artifact(&mut self, artifact: bed_debug::BuildArtifact) -> io::Result<()> {
        let profile = self
            .debugger
            .launch_profile
            .as_ref()
            .ok_or_else(|| io::Error::other("Launch was cancelled"))?;
        let adapter = bed_debug::discovery::discover_adapter(
            (!self.debugger.adapter_path.is_empty())
                .then(|| Path::new(&self.debugger.adapter_path)),
        )?;
        let mut breakpoints = std::collections::BTreeMap::new();
        for path in self.debugger.source.breakpoints.keys() {
            breakpoints.insert(
                path.clone(),
                self.debugger
                    .source
                    .bindings(path)
                    .into_iter()
                    .map(|(_, row)| SourceBreakpoint {
                        line: row.max(0) as usize + 1,
                        ..Default::default()
                    })
                    .collect(),
            );
        }
        let config = LaunchConfig {
            program: artifact.program.to_string_lossy().into(),
            cwd: if profile.cwd.is_empty() {
                self.project_root.clone()
            } else {
                absolute(&self.project_root, &profile.cwd)
                    .to_string_lossy()
                    .into()
            },
            args: profile.args.clone(),
            env: artifact.environment,
            source_map: paths::absolute_source_map(
                Path::new(&self.project_root),
                &profile.source_map,
            ),
            stop_on_entry: profile.stop_on_entry,
            breakpoints,
            init_commands: Vec::new(),
        };
        append_log(
            &mut self.debugger.console_log,
            &format!(
                "Adapter: {}\nProgram: {}\n",
                adapter.display(),
                config.program
            ),
        );
        self.debugger.session = Some(DebugSession::launch(&adapter, config)?);
        Ok(())
    }

    pub(super) fn draw_debug_hover(&mut self, ui: &Ui, view: &EditorView) -> io::Result<()> {
        let presentation = view.presentation();
        let info = presentation.hover_info;
        let mouse = ui.io().mouse_pos();
        let owns_hover = self
            .debugger
            .hover
            .as_ref()
            .is_some_and(|h| h.key.view == view.id());
        let over_popup = self.debugger.hover.as_ref().is_some_and(|h| {
            h.key.view == view.id() && h.rect.is_some_and(|r| hover_rect_contains(r, mouse))
        });
        let layout = presentation.layout;
        let in_pane = mouse[0] >= layout.text_pos[0]
            && mouse[0] < layout.pane_pos[0] + layout.pane_size[0]
            && mouse[1] >= layout.pane_pos[1]
            && mouse[1] < layout.pane_pos[1] + layout.pane_size[1];
        // Sibling editor views must not dismiss or render another view's popup.
        if !in_pane && !over_popup {
            if owns_hover {
                self.debugger.hover = None;
            }
            return Ok(());
        }
        let path = self
            .session
            .with_document(view.document_id(), |state| state.path.clone())?;
        let selected = self.debugger.session.as_ref().and_then(|s| {
            let frame = s.frames.iter().find(|f| Some(f.id) == s.selected_frame)?;
            let source = self.debugger.source_paths.get(&frame.id)?.to_str()?;
            (s.state == SessionState::Stopped && same_debug_source(source, &path))
                .then_some((frame.id, s.stop_generation))
        });
        if selected.is_none()
            || self.debugger.source.changed(&path)
            || hover_key_pressed(ui)
            || (presentation.hover_dismissed && !over_popup)
        {
            self.debugger.hover = None;
            return Ok(());
        }
        if !over_popup {
            if !info.active || info.zone != bed_ui::views::hover_trigger::Zone::Text {
                if owns_hover {
                    self.debugger.hover = None;
                }
                return Ok(());
            }
            let expression = self.session.with_document(view.document_id(), |state| {
                hover_expression(&state.line(info.row), info.column)
            })?;
            let Some(expression) = expression else {
                self.debugger.hover = None;
                return Ok(());
            };
            let (frame, stop) = selected.unwrap();
            let key = HoverKey {
                view: view.id(),
                document: view.document_id(),
                revision: self.session.document_revision(view.document_id())?,
                frame,
                stop,
                expression,
            };
            if !self.debugger.hover.as_ref().is_some_and(|h| h.key == key)
                && let Some(session) = &mut self.debugger.session
            {
                let request = session.evaluate(
                    key.expression.clone(),
                    EvaluateContext::Hover,
                    Some(frame),
                )?;
                self.debugger
                    .evaluations
                    .insert(request, EvaluationPurpose::Hover(key.clone()));
                self.debugger.hover = Some(RuntimeHover {
                    key,
                    value: None,
                    anchor: [mouse[0] + 12.0, mouse[1] + 16.0],
                    rect: None,
                    last_frame: ui.frame_count(),
                });
            }
        }
        if let Some(hover) = &mut self.debugger.hover
            && hover.key.view == view.id()
            && (hover.last_frame + 1 >= ui.frame_count() || info.active)
            && let Some(value) = &hover.value
            && presentation.tooltip_arbiter.claim(ui)
        {
            hover.rect = render_hover_popup(
                ui,
                &format!("##debug_hover_{}", view.id().0),
                hover.anchor,
                hover.rect,
                || {
                    ui.text(&hover.key.expression);
                    ui.separator();
                    ui.text(value);
                },
            );
            hover.last_frame = ui.frame_count();
        }
        Ok(())
    }
}

fn program_terminal_title(profile: Option<&DebugProfile>) -> String {
    let Some(profile) = profile else {
        return "Program Terminal".into();
    };
    let executable = match &profile.cargo {
        Some(cargo) => Some(cargo.target.name.as_str()),
        None => Path::new(&profile.program)
            .file_name()
            .and_then(|name| name.to_str()),
    };
    executable
        .filter(|name| !name.trim().is_empty())
        .or_else(|| (!profile.name.trim().is_empty()).then_some(profile.name.as_str()))
        .map(|name| format!("Program: {name}"))
        .unwrap_or_else(|| "Program Terminal".into())
}

fn append_log(log: &mut String, text: &str) {
    log.push_str(text);
    if log.len() > LOG_LIMIT {
        let mut cut = log.len() - LOG_LIMIT;
        while !log.is_char_boundary(cut) {
            cut += 1;
        }
        log.drain(..cut);
    }
}

fn absolute(root: &str, path: &str) -> PathBuf {
    let path = Path::new(path);
    if path.is_absolute() {
        path.to_owned()
    } else {
        Path::new(root).join(path)
    }
}

fn same_debug_source(frame_source: &str, document_path: &str) -> bool {
    !document_path.is_empty()
        && Path::new(frame_source).is_absolute()
        && (frame_source == document_path
            || Path::new(frame_source)
                .canonicalize()
                .ok()
                .zip(Path::new(document_path).canonicalize().ok())
                .is_some_and(|(frame, document)| frame == document))
}

fn frame_source_notice(source: Option<&str>, entry_stop: bool) -> String {
    if entry_stop {
        "Paused at program entry, before application code. Continue (F5) to reach a breakpoint."
            .into()
    } else if let Some(path) = source {
        format!(
            "Source is not available locally: {path}. Select another frame, or configure a source mapping for files built elsewhere."
        )
    } else {
        "No source for this runtime or assembly frame. Continue (F5) to reach a breakpoint, or select a frame with source.".into()
    }
}

/// Hover evaluates names/member access only; calls and arbitrary expressions
/// require an explicit user action in Watches or the console.
fn hover_expression(line: &[u8], column: i32) -> Option<String> {
    let text = std::str::from_utf8(line).ok()?;
    let position = (column.max(0) as usize).min(text.len());
    let allowed = |c: char| c.is_alphanumeric() || matches!(c, '_' | '.' | ':' | '-' | '>');
    let start = text
        .char_indices()
        .take_while(|(i, _)| *i < position)
        .filter(|(_, c)| !allowed(*c))
        .last()
        .map_or(0, |(i, c)| i + c.len_utf8());
    let end = text
        .char_indices()
        .filter(|(i, _)| *i >= position)
        .find(|(_, c)| !allowed(*c))
        .map_or(text.len(), |(i, _)| i);
    let expression = text.get(start..end)?.trim_matches('.');
    if expression.is_empty() || expression.len() > 256 {
        return None;
    }
    let parts = expression.replace("->", ".").replace("::", ".");
    if parts.split('.').all(|part| {
        let mut chars = part.chars();
        chars.next().is_some_and(|c| c.is_alphabetic() || c == '_')
            && chars.all(|c| c.is_alphanumeric() || c == '_')
    }) {
        Some(expression.to_owned())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hover_extracts_names_without_running_arbitrary_expressions() {
        assert_eq!(
            hover_expression(b"let x = foo.bar;", 12).as_deref(),
            Some("foo.bar")
        );
        assert_eq!(
            hover_expression(b"ptr->field", 6).as_deref(),
            Some("ptr->field")
        );
        assert_eq!(
            hover_expression("alpha.βeta".as_bytes(), 8).as_deref(),
            Some("alpha.βeta")
        );
        assert_eq!(hover_expression(b"23", 0), None);
        assert_eq!(hover_expression(b"foo--bar", 2), None);
    }
    #[test]
    fn logs_are_bounded_at_unicode_boundaries() {
        let mut log = "x".repeat(LOG_LIMIT - 1);
        append_log(&mut log, "🌞🌞");
        assert!(log.len() <= LOG_LIMIT);
        assert!(log.ends_with("🌞🌞"));
    }
}
