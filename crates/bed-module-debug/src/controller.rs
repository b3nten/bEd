//! Debugger feature state. The workbench is accessed only through scoped services.
use bed_debug::{
    BuildEvent, BuildJob, BuildRequest, CargoDiscovery, CargoLaunch, CargoWorkspace, DebugEvent,
    DebugProfile, DebugSession, EvaluateContext, LaunchConfig, SessionState, SourceBreakpoint,
};
use bed_document_session::editor_session::EditorSession;
use bed_document_session::editor_session::{DocumentId, SessionEvent, ViewId};
use bed_editor_ui::{
    editor_view::{EditorView, SourceDebugPresentation},
    views::hover_tooltip::{HoverRect, hover_key_pressed, hover_rect_contains, render_hover_popup},
};
use bed_workbench_api::{HostRequest, ModuleServices, TerminalLaunch};
use dear_imgui_rs::{TreeNodeFlags, Ui};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io,
    path::{Path, PathBuf},
};

#[path = "panel.rs"]
mod panel;
#[path = "paths.rs"]
mod paths;
#[path = "source.rs"]
pub mod source;
use source::SourceDebugState;

const LOG_LIMIT: usize = 2 * 1024 * 1024;

#[derive(Default)]
pub struct Debugger {
    pub profiles: Vec<DebugProfile>,
    pub selected: usize,
    pub watches: Vec<String>,
    pub adapter_path: String,
    pub last_saved: Option<Value>,
    pub source: SourceDebugState,
    pub session: Option<DebugSession>,
    pub build: Option<BuildJob>,
    pub launch_profile: Option<DebugProfile>,
    pub terminal: Option<u64>,
    pub discovery: Option<CargoDiscovery>,
    pub cargo_workspace: Option<CargoWorkspace>,
    pub build_log: String,
    pub console_log: String,
    pub error: Option<String>,
    pub source_notice: Option<String>,
    pub source_paths: HashMap<i64, PathBuf>,
    pub configure: bool,
    pub console_input: String,
    pub console_commands: bool,
    pub watch_input: String,
    pub inspected_stop: Option<(u64, Option<i64>)>,
    pub first_stop: Option<u64>,
    evaluations: HashMap<u64, EvaluationPurpose>,
    pub watch_values: HashMap<String, String>,
    hover: Option<RuntimeHover>,
    pub breakpoint_requests: HashMap<u64, (String, Vec<u64>)>,
    pub adapter_breakpoints: HashMap<i64, (String, u64)>,
    pub navigation: Option<(String, i32, i32)>,
    /// Editor extension callbacks run without the host request queue. Coalesce
    /// their presentation changes until the next module tick.
    pub(super) pending_presentation_invalidation: bool,
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
pub enum Action {
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
    RetryVariables(i64),
    Evaluate(String, EvaluateContext),
    Watch(String),
    RemoveWatch(usize),
    BreakpointEnabled(String, u64, bool),
    RemoveBreakpoint(String, u64),
    ClearBreakpoints,
    Navigate(String, i32),
    ShowTerminal,
    BrowseManifest(usize),
    BrowseProgram(usize),
}

impl Debugger {
    pub fn owns_terminal(&self, id: u64) -> bool {
        self.terminal == Some(id)
    }
    pub fn active(&self) -> bool {
        self.build.is_some()
            || self.session.as_ref().is_some_and(|s| {
                !matches!(s.state, SessionState::Terminated | SessionState::Failed)
            })
    }
    pub fn settings(&self) -> Value {
        json!({"version":1,"profiles":self.profiles,"selected":self.selected,
            "watches":self.watches,"adapter_path":self.adapter_path})
    }
    pub fn load(&mut self, value: Option<&Value>) {
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

use crate::show_panel;

impl Debugger {
    pub fn debug_smoke_ready(&self) -> io::Result<bool> {
        if let Some(error) = &self.error {
            return Err(io::Error::other(format!("Debugger smoke failed: {error}")));
        }
        Ok(self.session.as_ref().is_some_and(|session| {
            let mut locals = session
                .scopes
                .iter()
                .filter(|scope| {
                    scope.is_locals() && !scope.expensive && scope.variables_reference > 0
                })
                .peekable();
            session.state == SessionState::Stopped
                && session.selected_frame.is_some()
                && locals.peek().is_some()
                && locals.all(|scope| session.variables.contains_key(&scope.variables_reference))
        }))
    }

    pub fn stop_debugger(
        &mut self,
        services: &mut ModuleServices<'_>,
        requests: &mut Vec<HostRequest>,
    ) {
        if let Some(job) = self.build.take() {
            job.cancel();
        }
        if let Some(mut session) = self.session.take() {
            let _ = session.disconnect();
        }
        if let Some(id) = self.terminal.take() {
            services.terminals.stop(id);
            services.terminals.release(id);
        }
        self.launch_profile = None;
        self.source.end_launch();
        self.evaluations.clear();
        self.breakpoint_requests.clear();
        self.adapter_breakpoints.clear();
        self.watch_values.clear();
        self.hover = None;
        self.inspected_stop = None;
        self.first_stop = None;
        self.source_notice = None;
        self.source_paths.clear();
        requests.push(HostRequest::Invalidate);
    }

    pub fn start_debugger(
        &mut self,
        services: &mut ModuleServices<'_>,
        requests: &mut Vec<HostRequest>,
    ) -> io::Result<()> {
        if !cfg!(any(target_os = "macos", target_os = "linux")) || services.documents.is_remote() {
            return Err(io::Error::other(
                "Debugging is available for local macOS and Linux projects",
            ));
        }
        if services.project_root.is_empty() {
            return Err(io::Error::other("Open a local project before debugging"));
        }
        if self.profiles.is_empty() {
            self.load(None);
        }
        let mut profile = self.profiles[self.selected].clone();
        if profile
            .cargo
            .as_ref()
            .is_some_and(|c| c.target.kind.is_test())
            && !profile.test_filter.trim().is_empty()
        {
            profile.args.insert(0, profile.test_filter.trim().into());
        }
        let request = BuildRequest::for_profile(&profile, Path::new(services.project_root))?;
        self.stop_debugger(services, requests);
        for document in services.documents.document_ids() {
            let snapshot = services.documents.snapshot(document)?;
            if snapshot.dirty && !snapshot.path.is_empty() {
                services.documents.save(document)?;
            }
        }
        self.source.begin_launch(services.documents)?;
        self.error = None;
        self.build_log.clear();
        self.console_log.clear();
        self.build = Some(BuildJob::start(request)?);
        self.launch_profile = Some(profile);
        self.configure = false;
        requests.push(HostRequest::Invalidate);
        Ok(())
    }

    pub fn draw_debugger(
        &mut self,
        ui: &Ui,
        services: &mut ModuleServices<'_>,
        requests: &mut Vec<HostRequest>,
    ) -> io::Result<()> {
        if services.documents.is_remote() || !cfg!(any(target_os = "macos", target_os = "linux")) {
            ui.text_wrapped("Debugging is available for local macOS and Linux projects.");
            return Ok(());
        }
        if services.project_root.is_empty() {
            ui.text_disabled("Open a local project to configure debugging");
            return Ok(());
        }
        if self.profiles.is_empty() {
            self.load(None);
        }
        let actions = self.draw(ui, Path::new(services.project_root));
        for action in actions {
            if let Err(error) = self.debug_action(action, services, requests) {
                self.error = Some(error.to_string());
            }
        }
        Ok(())
    }

    pub fn debug_action(
        &mut self,
        action: Action,
        services: &mut ModuleServices<'_>,
        requests: &mut Vec<HostRequest>,
    ) -> io::Result<()> {
        match action {
            Action::Start | Action::Restart => self.start_debugger(services, requests)?,
            Action::Stop => self.stop_debugger(services, requests),
            Action::Discover => {
                let profile = &self.profiles[self.selected];
                let cargo = profile
                    .cargo
                    .as_ref()
                    .ok_or_else(|| io::Error::other("Choose a Cargo profile"))?;
                self.discovery = Some(CargoDiscovery::start(&absolute(
                    services.project_root,
                    &cargo.manifest_path,
                ))?);
                self.error = None;
            }
            Action::Continue | Action::Pause | Action::Over | Action::Into | Action::Out => {
                if let Some(session) = &mut self.session {
                    match action {
                        Action::Continue => session.continue_execution()?,
                        Action::Pause => session.pause()?,
                        Action::Over => session.step_over()?,
                        Action::Into => session.step_in()?,
                        Action::Out => session.step_out()?,
                        _ => unreachable!(),
                    };
                    self.hover = None;
                    self.watch_values.clear();
                    self.evaluations.clear();
                }
            }
            Action::Frame(id) => {
                if let Some(session) = &mut self.session {
                    session.select_frame(id)?;
                }
                self.inspected_stop = None;
                self.hover = None;
                self.evaluations.clear();
            }
            Action::Thread(id) => {
                if let Some(session) = &mut self.session {
                    session.select_thread(id)?;
                }
                self.inspected_stop = None;
                self.hover = None;
                self.evaluations.clear();
            }
            Action::Variables(reference) => {
                if let Some(session) = &mut self.session {
                    session.load_variables(reference)?;
                }
            }
            Action::RetryVariables(reference) => {
                if let Some(session) = &mut self.session {
                    session.retry_variables(reference)?;
                }
            }
            Action::Evaluate(expression, context) => {
                let session = self
                    .session
                    .as_mut()
                    .ok_or_else(|| io::Error::other("Start a debugging session first"))?;
                let request = session.evaluate(expression, context, session.selected_frame)?;
                self.evaluations.insert(request, EvaluationPurpose::Console);
            }
            Action::Watch(expression) => {
                let expression = expression.trim().to_owned();
                if !expression.is_empty() && !self.watches.contains(&expression) {
                    self.watches.push(expression);
                    self.inspected_stop = None;
                }
            }
            Action::RemoveWatch(index) => {
                if index < self.watches.len() {
                    self.watches.remove(index);
                }
            }
            Action::BreakpointEnabled(path, id, enabled) => {
                self.source.set_enabled(&path, id, enabled);
                self.sync_debug_breakpoints(&path)?;
            }
            Action::RemoveBreakpoint(path, id) => {
                self.source.remove(&path, id);
                self.sync_debug_breakpoints(&path)?;
            }
            Action::ClearBreakpoints => {
                let paths: Vec<_> = self.source.breakpoints.keys().cloned().collect();
                self.source.clear();
                for path in paths {
                    self.sync_debug_breakpoints(&path)?;
                }
            }
            Action::Navigate(path, row) => requests.push(HostRequest::RevealSource {
                path,
                row,
                column: 0,
                center: false,
            }),
            Action::ShowTerminal => {
                if let Some(id) = self.terminal {
                    requests.push(HostRequest::ShowTerminal { id });
                }
            }
            Action::BrowseManifest(index) => {
                if let Some(path) = services
                    .dialogs
                    .pick_file(Path::new(services.project_root), &["toml"])
                    && let Some(cargo) = self
                        .profiles
                        .get_mut(index)
                        .and_then(|profile| profile.cargo.as_mut())
                {
                    cargo.manifest_path = path.to_string_lossy().into();
                    if index == self.selected {
                        self.cargo_workspace = None;
                        self.discovery = None;
                        self.debug_action(Action::Discover, services, requests)?;
                    }
                }
            }
            Action::BrowseProgram(index) => {
                if let Some(path) = services
                    .dialogs
                    .pick_file(Path::new(services.project_root), &[])
                    && let Some(profile) = self.profiles.get_mut(index)
                {
                    profile.program = path.to_string_lossy().into();
                }
            }
        }
        requests.push(HostRequest::Invalidate);
        Ok(())
    }

    pub fn tick_debugger(
        &mut self,
        services: &mut ModuleServices<'_>,
        requests: &mut Vec<HostRequest>,
    ) -> io::Result<()> {
        if std::mem::take(&mut self.pending_presentation_invalidation) {
            requests.push(HostRequest::Invalidate);
        }
        if let Some(discovery) = &mut self.discovery
            && discovery.poll()
            && let Some(result) = discovery.outcome().cloned()
        {
            self.discovery = None;
            match result {
                Ok(workspace) => {
                    if let Some(profile) = self.profiles.get_mut(self.selected)
                        && let Some(cargo) = &mut profile.cargo
                        && cargo.package.is_empty()
                        && let Some(target) = workspace.targets.first()
                    {
                        cargo.package = target.package.clone();
                        cargo.target = target.target.clone();
                    }
                    self.cargo_workspace = Some(workspace);
                }
                Err(error) => self.error = Some(error),
            }
            requests.push(HostRequest::Invalidate);
        }
        let events = self.build.as_mut().map(BuildJob::poll).unwrap_or_default();
        for event in events {
            match event {
                BuildEvent::Output(output) => append_log(&mut self.build_log, &output),
                BuildEvent::Finished(result) => {
                    self.build = None;
                    match result {
                        Ok(artifact) => {
                            if let Err(error) = self.launch_debug_artifact(artifact, services) {
                                self.error = Some(error.to_string());
                                self.source.end_launch();
                            }
                        }
                        Err(error) => {
                            append_log(&mut self.build_log, &format!("\n{error}\n"));
                            self.error = Some(error);
                            self.source.end_launch();
                        }
                    }
                }
            }
            requests.push(HostRequest::Invalidate);
        }
        let events = self
            .session
            .as_mut()
            .map(DebugSession::poll)
            .unwrap_or_default();
        let inspection = self.session.as_ref().and_then(|s| {
            (s.state == SessionState::Stopped).then_some((s.stop_generation, s.selected_frame))
        });
        if !events.is_empty() || inspection != self.inspected_stop {
            let workspace = Path::new(services.project_root);
            let profile = self.launch_profile.as_ref();
            let cwd = profile
                .filter(|profile| !profile.cwd.is_empty())
                .map(|profile| absolute(services.project_root, &profile.cwd))
                .unwrap_or_else(|| workspace.to_owned());
            let source_map = profile
                .map(|profile| profile.source_map.as_slice())
                .unwrap_or_default();
            // Keep the adapter's raw paths intact: applying mappings to an
            // already mapped frame on the next event can map it a second time.
            self.source_paths = self
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
            *self.first_stop.get_or_insert(generation) == generation
                && self
                    .launch_profile
                    .as_ref()
                    .is_some_and(|profile| profile.stop_on_entry)
        });
        if inspection != self.inspected_stop {
            self.inspected_stop = inspection;
            self.hover = None;
            self.evaluations.clear();
            self.watch_values.clear();
            self.source_notice = None;
            if let Some((_, Some(frame_id))) = inspection {
                if let Some(session) = &mut self.session {
                    if let Some(frame) = session.frames.iter().find(|f| f.id == frame_id) {
                        if let Some(path) = self.source_paths.get(&frame.id) {
                            self.navigation = Some((
                                path.to_string_lossy().into(),
                                frame.line.saturating_sub(1) as i32,
                                frame.column.saturating_sub(1) as i32,
                            ));
                        } else {
                            self.source_notice =
                                Some(frame_source_notice(frame.source.as_deref(), entry_stop));
                        }
                    }
                    for expression in &self.watches {
                        match session.evaluate(
                            expression.clone(),
                            EvaluateContext::Watch,
                            Some(frame_id),
                        ) {
                            Ok(id) => {
                                self.evaluations
                                    .insert(id, EvaluationPurpose::Watch(expression.clone()));
                            }
                            Err(error) => {
                                self.watch_values
                                    .insert(expression.clone(), error.to_string());
                            }
                        }
                    }
                }
                requests.push(show_panel());
                // Entry/assembly frames often have no local source. Reveal the
                // inspection panel while returning keyboard focus to the user's
                // source view, just as a successful source navigation would.
                if self.source_notice.is_some()
                    && let Some(view) = services.active_view
                {
                    requests.push(HostRequest::FocusView { view });
                }
            }
        }
        for event in events {
            match event {
                DebugEvent::Changed => {}
                DebugEvent::Output { category, output } => {
                    append_log(&mut self.console_log, &format!("[{category}] {output}"))
                }
                DebugEvent::Error(error) => {
                    append_log(&mut self.console_log, &format!("{error}\n"));
                    self.error = Some(error);
                }
                DebugEvent::RunInTerminal(request) => {
                    let args = std::env::current_exe()
                        .ok()
                        .and_then(|host| bed_debug::launcher::arguments(&request.args, &host))
                        .unwrap_or(request.args);
                    let title = program_terminal_title(self.launch_profile.as_ref());
                    let result = services.terminals.spawn(TerminalLaunch {
                        args,
                        cwd: (!request.cwd.is_empty()).then(|| PathBuf::from(request.cwd)),
                        env: request.env.into_iter().collect(),
                        title,
                    });
                    let reply = match result {
                        Ok((id, pid)) => {
                            self.terminal = Some(id);
                            requests.push(HostRequest::ShowTerminal { id });
                            Ok(pid)
                        }
                        Err(error) => Err(error.to_string()),
                    };
                    if let Some(session) = &mut self.session {
                        session.respond_run_in_terminal(request.request_seq, reply)?;
                    }
                }
                DebugEvent::Breakpoints {
                    path,
                    breakpoints,
                    request_id,
                } => {
                    let ids: Vec<_> = self
                        .breakpoint_requests
                        .remove(&request_id)
                        .map(|(_, ids)| ids)
                        .unwrap_or_else(|| {
                            self.source
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
                            self.adapter_breakpoints
                                .insert(adapter_id, (path.clone(), *id));
                        }
                    }
                    self.source.apply_result(&path, &ids, &results);
                }
                DebugEvent::BreakpointChanged { breakpoint, .. } => {
                    if let Some((path, id)) = breakpoint
                        .id
                        .and_then(|id| self.adapter_breakpoints.get(&id))
                        .cloned()
                    {
                        self.source.apply_result(
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
                    if let Some(purpose) = self.evaluations.remove(&request_id) {
                        let succeeded = result.is_ok();
                        let value = result
                            .map(|v| v.result)
                            .unwrap_or_else(|e| format!("Error: {e}"));
                        match purpose {
                            EvaluationPurpose::Watch(expression) => {
                                self.watch_values.insert(expression, value);
                            }
                            EvaluationPurpose::Console => {
                                append_log(&mut self.console_log, &format!("{value}\n"))
                            }
                            EvaluationPurpose::Hover(key) => {
                                if let Some(hover) = &mut self.hover
                                    && hover.key == key
                                    && services.documents.document_revision(key.document).ok()
                                        == Some(key.revision)
                                {
                                    hover.value = succeeded.then_some(value);
                                }
                            }
                        }
                    }
                }
            }
            requests.push(HostRequest::Invalidate);
        }
        if let Some((path, row, column)) = self.navigation.take() {
            if Path::new(&path).is_file() {
                requests.push(HostRequest::RevealSource {
                    path,
                    row,
                    column,
                    center: true,
                });
            } else {
                self.source_notice = Some(frame_source_notice(Some(&path), false));
            }
        }
        if self.session.as_ref().is_some_and(|s| s.state.is_finished()) {
            self.source.end_launch();
            self.hover = None;
            self.source_notice = None;
            self.source_paths.clear();
            if self
                .session
                .as_ref()
                .is_some_and(|s| s.state == SessionState::Failed)
                && let Some(id) = self.terminal
            {
                services.terminals.stop(id);
            }
        }
        Ok(())
    }

    fn launch_debug_artifact(
        &mut self,
        artifact: bed_debug::BuildArtifact,
        services: &mut ModuleServices<'_>,
    ) -> io::Result<()> {
        let profile = self
            .launch_profile
            .as_ref()
            .ok_or_else(|| io::Error::other("Launch was cancelled"))?;
        let adapter = bed_debug::discovery::discover_adapter(
            (!self.adapter_path.is_empty()).then(|| Path::new(&self.adapter_path)),
        )?;
        let mut breakpoints = std::collections::BTreeMap::new();
        for path in self.source.breakpoints.keys() {
            breakpoints.insert(
                path.clone(),
                self.source
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
                services.project_root.into()
            } else {
                absolute(services.project_root, &profile.cwd)
                    .to_string_lossy()
                    .into()
            },
            args: profile.args.clone(),
            env: artifact.environment,
            source_map: paths::absolute_source_map(
                Path::new(services.project_root),
                &profile.source_map,
            ),
            stop_on_entry: profile.stop_on_entry,
            breakpoints,
            init_commands: artifact.init_commands,
        };
        append_log(
            &mut self.console_log,
            &format!(
                "Adapter: {}\nProgram: {}\n",
                adapter.display(),
                config.program
            ),
        );
        self.session = Some(DebugSession::launch(&adapter, config)?);
        Ok(())
    }

    pub fn debug_document_events(
        &mut self,
        events: &[SessionEvent],
        documents: &EditorSession,
    ) -> io::Result<()> {
        if documents.is_remote()
            || !cfg!(any(target_os = "macos", target_os = "linux"))
            || (!self.active() && self.source.breakpoints.is_empty())
        {
            return Ok(());
        }
        let paths = self.source.on_events(documents, events)?;
        if !paths.is_empty() {
            self.hover = None;
        }
        for path in paths {
            self.sync_debug_breakpoints(&path)?;
        }
        Ok(())
    }

    pub fn toggle_debug_breakpoint(
        &mut self,
        documents: &EditorSession,
        document: DocumentId,
        row: i32,
    ) -> io::Result<()> {
        if documents.is_remote() || !cfg!(any(target_os = "macos", target_os = "linux")) {
            return Ok(());
        }
        let (path, column) = documents.with_document(document, |state| {
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
            self.error = Some("Save this file before adding breakpoints".into());
            return Ok(());
        }
        self.source.toggle(&path, row, column);
        self.sync_debug_breakpoints(&path)?;

        Ok(())
    }

    pub fn debug_source_presentation(
        &self,
        documents: &EditorSession,
        document: DocumentId,
    ) -> io::Result<Option<SourceDebugPresentation>> {
        if documents.is_remote() || !cfg!(any(target_os = "macos", target_os = "linux")) {
            return Ok(None);
        }
        let path = documents.with_document(document, |state| state.path.clone())?;
        if path.is_empty() {
            return Ok(None);
        }
        let row = self.session.as_ref().and_then(|s| {
            if s.state != SessionState::Stopped {
                return None;
            }
            s.frames
                .iter()
                .find(|f| Some(f.id) == s.selected_frame)
                .filter(|f| {
                    self.source_paths
                        .get(&f.id)
                        .is_some_and(|source| source == Path::new(&path))
                })
                .map(|f| f.line.saturating_sub(1) as i32)
        });
        Ok(Some(self.source.presentation(&path, row)))
    }

    pub fn draw_debug_hover(
        &mut self,
        ui: &Ui,
        documents: &EditorSession,
        view: &EditorView,
    ) -> io::Result<()> {
        let presentation = view.presentation();
        let info = presentation.hover_info;
        let mouse = ui.io().mouse_pos();
        let owns_hover = self.hover.as_ref().is_some_and(|h| h.key.view == view.id());
        let over_popup = self.hover.as_ref().is_some_and(|h| {
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
                self.hover = None;
            }
            return Ok(());
        }
        let path = documents.with_document(view.document_id(), |state| state.path.clone())?;
        let selected = self.session.as_ref().and_then(|s| {
            let frame = s.frames.iter().find(|f| Some(f.id) == s.selected_frame)?;
            let source = self.source_paths.get(&frame.id)?.to_str()?;
            (s.state == SessionState::Stopped && same_debug_source(source, &path))
                .then_some((frame.id, s.stop_generation))
        });
        if selected.is_none()
            || self.source.changed(&path)
            || hover_key_pressed(ui)
            || (presentation.hover_dismissed && !over_popup)
        {
            self.hover = None;
            return Ok(());
        }
        if !over_popup {
            if !info.active || info.zone != bed_editor_ui::views::hover_trigger::Zone::Text {
                if owns_hover {
                    self.hover = None;
                }
                return Ok(());
            }
            let expression = documents.with_document(view.document_id(), |state| {
                hover_expression(&state.line(info.row), info.column)
            })?;
            let Some(expression) = expression else {
                self.hover = None;
                return Ok(());
            };
            let (frame, stop) = selected.unwrap();
            let key = HoverKey {
                view: view.id(),
                document: view.document_id(),
                revision: documents.document_revision(view.document_id())?,
                frame,
                stop,
                expression,
            };
            if !self.hover.as_ref().is_some_and(|h| h.key == key)
                && let Some(session) = &mut self.session
            {
                let request = session.evaluate(
                    key.expression.clone(),
                    EvaluateContext::Hover,
                    Some(frame),
                )?;
                self.evaluations
                    .insert(request, EvaluationPurpose::Hover(key.clone()));
                self.hover = Some(RuntimeHover {
                    key,
                    value: None,
                    anchor: [mouse[0] + 12.0, mouse[1] + 16.0],
                    rect: None,
                    last_frame: ui.frame_count(),
                });
            }
        }
        if let Some(hover) = &mut self.hover
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

    fn sync_debug_breakpoints(&mut self, path: &str) -> io::Result<()> {
        let bindings = self.source.bindings(path);
        if let Some(session) = &mut self.session
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
                self.breakpoint_requests
                    .retain(|_, (pending_path, _)| pending_path != path);
                self.breakpoint_requests.insert(
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
}

pub fn program_terminal_title(profile: Option<&DebugProfile>) -> String {
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

pub fn same_debug_source(frame_source: &str, document_path: &str) -> bool {
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
