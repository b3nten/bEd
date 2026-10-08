//! Debugger module: owns sessions and panels and contributes to the text editor.
//!
//! The GUI-free DAP implementation remains in `bed-debug`. This crate integrates
//! it through workbench services and the editor's concrete extension contract.
mod controller;

use bed_debug::{CargoLaunch, SessionState};
use bed_document_session::editor_session::{DocumentId, EditorSession, SessionEvent};
use bed_editor_ui::{
    EditorView, SourceDebugAction, SourceDebugPresentation,
    extensions::{EditorExtensions, SourceDebugExtension},
};
use bed_workbench_api::{
    CommandContext, HostContext, HostRequest, MenuSlot, Module, ModulePanel, ModuleServices,
    PanelPlacement, Registrar,
};
use controller::{Action, Debugger};
use dear_imgui_rs::{Key, Ui};
use serde_json::Value;
use std::{any::Any, cell::RefCell, io, path::Path, rc::Rc};

pub const MODULE_ID: &str = "bed.debug";
pub const PANEL_ID: &str = "bed.debug.panel";
pub const SHOW_COMMAND: &str = "bed.debug.show";
pub const START_COMMAND: &str = "bed.debug.start";
pub const STOP_COMMAND: &str = "bed.debug.stop";
pub const RESTART_COMMAND: &str = "bed.debug.restart";
pub const CONTINUE_COMMAND: &str = "bed.debug.continue";
pub const PAUSE_COMMAND: &str = "bed.debug.pause";
pub const OVER_COMMAND: &str = "bed.debug.step_over";
pub const INTO_COMMAND: &str = "bed.debug.step_into";
pub const OUT_COMMAND: &str = "bed.debug.step_out";
pub const BREAKPOINT_COMMAND: &str = "bed.debug.toggle_breakpoint";

pub(crate) fn show_panel() -> HostRequest {
    HostRequest::OpenPanel {
        panel_type: PANEL_ID.into(),
        document: None,
        state: Value::Null,
    }
}

pub struct DebugModule {
    state: Rc<RefCell<Debugger>>,
}

impl DebugModule {
    /// Register once per workspace; contributions serve all text-editor views.
    pub fn new(extensions: &EditorExtensions) -> Self {
        let state = Rc::new(RefCell::new(Debugger::default()));
        let provider: Rc<RefCell<dyn SourceDebugExtension>> = state.clone();
        extensions.register_source_debug(&provider);
        Self { state }
    }

    /// Native state injection for host regression fixtures.
    #[cfg(any(test, feature = "test-support"))]
    pub fn state(&self) -> Rc<RefCell<Debugger>> {
        self.state.clone()
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn borrow_state(&self) -> std::cell::RefMut<'_, Debugger> {
        self.state.borrow_mut()
    }

    /// Launch the host's debugger smoke scenario through normal feature services.
    pub fn smoke_setup(
        &mut self,
        services: &mut ModuleServices<'_>,
        requests: &mut Vec<HostRequest>,
    ) -> io::Result<()> {
        if let Some(view) = services.active_view
            && let Some(document) = services.documents.document_for_view(view)
        {
            let snapshot = services.documents.snapshot(document)?;
            let row = String::from_utf8_lossy(&snapshot.bytes)
                .lines()
                .position(|line| line.contains("DEBUG_BREAKPOINT"))
                .unwrap_or(0) as i32;
            self.state
                .borrow_mut()
                .toggle_debug_breakpoint(services.documents, document, row)?;
        }
        self.show(services, requests);
        self.state.borrow_mut().start_debugger(services, requests)
    }

    pub fn smoke_ready(&self) -> io::Result<bool> {
        self.state.borrow().debug_smoke_ready()
    }

    fn show(&self, services: &mut ModuleServices<'_>, requests: &mut Vec<HostRequest>) {
        requests.push(show_panel());
        let mut debugger = self.state.borrow_mut();
        if debugger.profiles.is_empty() {
            debugger.load(None);
        }
        if debugger.cargo_workspace.is_none()
            && debugger.discovery.is_none()
            && debugger.profiles[debugger.selected].cargo.is_some()
            && let Err(error) = debugger.debug_action(Action::Discover, services, requests)
        {
            debugger.error = Some(error.to_string());
        }
    }

    fn run_action(
        &self,
        action: Action,
        services: &mut ModuleServices<'_>,
        requests: &mut Vec<HostRequest>,
    ) {
        let mut debugger = self.state.borrow_mut();
        if let Err(error) = debugger.debug_action(action, services, requests) {
            debugger.error = Some(error.to_string());
        }
    }
}

impl SourceDebugExtension for Debugger {
    fn presentation(
        &self,
        session: &EditorSession,
        document: DocumentId,
    ) -> io::Result<Option<SourceDebugPresentation>> {
        self.debug_source_presentation(session, document)
    }

    fn action(
        &mut self,
        session: &EditorSession,
        document: DocumentId,
        action: SourceDebugAction,
    ) -> io::Result<()> {
        match action {
            SourceDebugAction::ToggleBreakpoint { row } => {
                self.toggle_debug_breakpoint(session, document, row)?;
                self.pending_presentation_invalidation = true;
                Ok(())
            }
        }
    }

    fn draw_hover(
        &mut self,
        ui: &Ui,
        session: &EditorSession,
        view: &EditorView,
    ) -> io::Result<()> {
        self.draw_debug_hover(ui, session, view)
    }
}

impl Module for DebugModule {
    fn id(&self) -> &'static str {
        MODULE_ID
    }

    fn register(&self, registrar: &mut Registrar<'_>) {
        for (id, label) in [
            (SHOW_COMMAND, "Debugger"),
            (START_COMMAND, "Start Debugging"),
            (STOP_COMMAND, "Stop Debugging"),
            (RESTART_COMMAND, "Restart Debugging"),
            (CONTINUE_COMMAND, "Continue Debugging"),
            (PAUSE_COMMAND, "Pause Debugging"),
            (OVER_COMMAND, "Step Over"),
            (INTO_COMMAND, "Step Into"),
            (OUT_COMMAND, "Step Out"),
            (BREAKPOINT_COMMAND, "Toggle Breakpoint"),
        ] {
            registrar.command(id, label, None);
        }
        registrar.menu(MenuSlot::Application, SHOW_COMMAND);
        registrar.panel_options(
            PANEL_ID,
            "Debug",
            true,
            PanelPlacement::Bottom,
            Some("debug"),
        );
    }

    fn restore_workspace(&mut self, state: Option<&Value>, project_root: &str) {
        // Replace the value inside the allocation so registered editor providers
        // and any restored panels continue to refer to the same feature state.
        let mut debugger = self.state.borrow_mut();
        *debugger = Debugger::default();
        debugger.load(state);
        if state.is_none() && Path::new(project_root).join("Cargo.toml").is_file() {
            debugger.profiles[0].cargo = Some(CargoLaunch::default());
        }
    }

    fn save_workspace(&self) -> Value {
        self.state.borrow().settings()
    }

    fn tick_with_services(
        &mut self,
        _host: &HostContext<'_>,
        services: &mut ModuleServices<'_>,
        requests: &mut Vec<HostRequest>,
    ) -> io::Result<()> {
        self.state.borrow_mut().tick_debugger(services, requests)
    }

    fn document_events(
        &mut self,
        session: &EditorSession,
        events: &[SessionEvent],
    ) -> io::Result<()> {
        self.state
            .borrow_mut()
            .debug_document_events(events, session)
    }

    fn shutdown(&mut self, services: &mut ModuleServices<'_>) {
        self.state
            .borrow_mut()
            .stop_debugger(services, &mut Vec::new());
    }

    fn retains_terminal(&self, id: u64) -> bool {
        self.state.borrow().owns_terminal(id)
    }

    fn command_enabled(
        &self,
        command: &str,
        context: &CommandContext,
        _host: &HostContext<'_>,
    ) -> bool {
        let debugger = self.state.borrow();
        let state = debugger.session.as_ref().map(|session| session.state);
        match command {
            START_COMMAND => !debugger.active(),
            STOP_COMMAND | RESTART_COMMAND => debugger.active(),
            CONTINUE_COMMAND | OVER_COMMAND | INTO_COMMAND | OUT_COMMAND => {
                state == Some(SessionState::Stopped)
            }
            PAUSE_COMMAND => state == Some(SessionState::Running),
            BREAKPOINT_COMMAND => context.document.is_some(),
            _ => true,
        }
    }

    fn command(
        &mut self,
        command: &str,
        _context: &CommandContext,
        _host: &HostContext<'_>,
        requests: &mut Vec<HostRequest>,
    ) {
        if command == SHOW_COMMAND {
            requests.push(show_panel());
        }
    }

    fn command_with_services(
        &mut self,
        command: &str,
        context: &CommandContext,
        _host: &HostContext<'_>,
        services: &mut ModuleServices<'_>,
        requests: &mut Vec<HostRequest>,
    ) -> io::Result<()> {
        if command == SHOW_COMMAND {
            self.show(services, requests);
            return Ok(());
        }
        if command == BREAKPOINT_COMMAND {
            if let Some(view) = services.active_view {
                let document = services
                    .documents
                    .document_for_view(view)
                    .ok_or_else(|| io::Error::other("Focused editor view is no longer open"))?;
                // Context originates at the command surface; do not reinterpret
                // a selection command against an unrelated focused document.
                if context.document.is_none_or(|target| target == document) {
                    let row = services.documents.view_snapshot(view)?.primary().head_row;
                    self.state.borrow_mut().toggle_debug_breakpoint(
                        services.documents,
                        document,
                        row,
                    )?;
                    requests.push(HostRequest::Invalidate);
                }
            }
            return Ok(());
        }
        let action = match command {
            START_COMMAND => Action::Start,
            STOP_COMMAND => Action::Stop,
            RESTART_COMMAND => Action::Restart,
            CONTINUE_COMMAND => Action::Continue,
            PAUSE_COMMAND => Action::Pause,
            OVER_COMMAND => Action::Over,
            INTO_COMMAND => Action::Into,
            OUT_COMMAND => Action::Out,
            _ => return Ok(()),
        };
        if matches!(action, Action::Start) {
            self.show(services, requests);
        }
        self.run_action(action, services, requests);
        Ok(())
    }

    fn shortcuts(
        &mut self,
        ui: &Ui,
        _host: &HostContext<'_>,
        services: &mut ModuleServices<'_>,
        requests: &mut Vec<HostRequest>,
    ) -> io::Result<()> {
        if ui.io().want_text_input()
            || ui.io().key_ctrl()
            || ui.io().key_super()
            || ui.io().key_alt()
        {
            return Ok(());
        }
        let pressed = |key| ui.is_key_pressed_with_repeat(key, false);
        let action = if pressed(Key::F9) {
            if let Some(view) = services.active_view {
                let document = services
                    .documents
                    .document_for_view(view)
                    .ok_or_else(|| io::Error::other("Focused editor view is no longer open"))?;
                let row = services.documents.view_snapshot(view)?.primary().head_row;
                self.state.borrow_mut().toggle_debug_breakpoint(
                    services.documents,
                    document,
                    row,
                )?;
                requests.push(HostRequest::Invalidate);
            }
            None
        } else if pressed(Key::F5) {
            let debugger = self.state.borrow();
            Some(if ui.io().key_shift() {
                Action::Stop
            } else if debugger
                .session
                .as_ref()
                .is_some_and(|session| session.state == SessionState::Stopped)
            {
                Action::Continue
            } else if debugger.active() {
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
                self.show(services, requests);
            }
            self.run_action(action, services, requests);
        }
        Ok(())
    }

    fn create_panel(
        &mut self,
        panel_type: &str,
        _document: Option<DocumentId>,
        _state: &Value,
    ) -> Result<Box<dyn ModulePanel>, String> {
        if panel_type != PANEL_ID {
            return Err(format!("Unknown debugger panel: {panel_type}"));
        }
        Ok(Box::new(DebugPanel {
            state: self.state.clone(),
        }))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

struct DebugPanel {
    state: Rc<RefCell<Debugger>>,
}

impl ModulePanel for DebugPanel {
    fn title(&self, _host: &HostContext<'_>) -> String {
        "Debug".into()
    }

    fn draw(&mut self, _ui: &Ui, _host: &HostContext<'_>, _requests: &mut Vec<HostRequest>) {}

    fn draw_with_services(
        &mut self,
        ui: &Ui,
        _host: &HostContext<'_>,
        services: &mut ModuleServices<'_>,
        requests: &mut Vec<HostRequest>,
    ) -> io::Result<()> {
        self.state
            .borrow_mut()
            .draw_debugger(ui, services, requests)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// Native state injection for host regression tests; production integrations use
/// commands, scoped services and the editor extension contract.
#[cfg(feature = "test-support")]
pub mod fixtures {
    pub use crate::controller::source::{Breakpoint, SourceDebugState};
    pub use crate::controller::{Action, Debugger, program_terminal_title, same_debug_source};
}

#[cfg(test)]
static IMGUI_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod test_support {
    use std::{
        fs,
        path::{Path, PathBuf},
        sync::atomic::{AtomicUsize, Ordering},
    };

    pub struct TempDir(PathBuf);

    impl TempDir {
        pub fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let root = std::env::temp_dir().join(format!(
                "bed-module-debug-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&root).unwrap();
            Self(root)
        }

        pub fn root(&self) -> &Path {
            &self.0
        }

        pub fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
            let path = self.0.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, bytes).unwrap();
            path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bed_workbench_api::{FileDialogService, Registry, TerminalLaunch, TerminalService};

    struct Dialogs;

    impl FileDialogService for Dialogs {
        fn pick_file(
            &mut self,
            _directory: &Path,
            _extensions: &[&str],
        ) -> Option<std::path::PathBuf> {
            panic!("This test must not open a file dialog")
        }
    }

    #[derive(Default)]
    struct Terminals {
        stopped: Vec<u64>,
        released: Vec<u64>,
    }

    impl TerminalService for Terminals {
        fn spawn(&mut self, _launch: TerminalLaunch) -> io::Result<(u64, u32)> {
            panic!("This test must not start a process")
        }
        fn stop(&mut self, id: u64) {
            self.stopped.push(id);
        }
        fn release(&mut self, id: u64) {
            self.released.push(id);
        }
    }

    #[test]
    fn closing_inspection_panel_preserves_session_resources_until_module_shutdown() {
        let extensions = EditorExtensions::default();
        let mut module = DebugModule::new(&extensions);
        module.restore_workspace(None, "");
        let mut registry = Registry::default();
        registry.register(&module).unwrap();
        let descriptor = registry.panel(PANEL_ID).unwrap();
        assert!(descriptor.singleton);
        assert_eq!(descriptor.placement, PanelPlacement::Bottom);
        assert_eq!(descriptor.legacy_kind, Some("debug"));
        assert!(registry.command(SHOW_COMMAND).is_some());
        let mut panel = module.create_panel(PANEL_ID, None, &Value::Null).unwrap();
        let state = module.state();
        state.borrow_mut().terminal = Some(42);
        let mut documents = EditorSession::new();
        let mut terminals = Terminals::default();
        let mut dialogs = Dialogs;
        let mut services = ModuleServices {
            resources: Default::default(),
            settings_ui: None,
            documents: &mut documents,
            terminals: &mut terminals,
            dialogs: &mut dialogs,
            active_view: None,
            project_root: "",
        };
        panel
            .close_with_services(&mut services, &mut Vec::new())
            .unwrap();
        drop(panel);
        assert!(module.retains_terminal(42));
        module.shutdown(&mut services);
        assert!(!module.retains_terminal(42));
        assert_eq!(terminals.stopped, [42]);
        assert_eq!(terminals.released, [42]);
    }

    #[test]
    fn workspace_restore_keeps_panel_and_editor_provider_allocation_and_resets_session_markers() {
        let extensions = EditorExtensions::default();
        let mut module = DebugModule::new(&extensions);
        module.restore_workspace(None, "");
        let state = module.state();
        let panel = module.create_panel(PANEL_ID, None, &Value::Null).unwrap();
        {
            let mut debugger = state.borrow_mut();
            debugger.watches.push("object.field".into());
            debugger.source.toggle("main.rs", 3, 0);
        }
        let saved = module.save_workspace();
        module.restore_workspace(Some(&saved), "");
        assert!(Rc::ptr_eq(&state, &module.state()));
        let panel = panel.as_any().downcast_ref::<DebugPanel>().unwrap();
        assert!(Rc::ptr_eq(&panel.state, &state));
        assert_eq!(state.borrow().watches, ["object.field"]);
        assert!(state.borrow().source.breakpoints.is_empty());
    }

    #[test]
    fn executable_browse_uses_host_picker_and_cancellation_preserves_profile() {
        struct Picker {
            selected: Option<std::path::PathBuf>,
            calls: Vec<(std::path::PathBuf, Vec<String>)>,
        }
        impl FileDialogService for Picker {
            fn pick_file(
                &mut self,
                directory: &Path,
                extensions: &[&str],
            ) -> Option<std::path::PathBuf> {
                self.calls.push((
                    directory.to_owned(),
                    extensions
                        .iter()
                        .map(|extension| (*extension).into())
                        .collect(),
                ));
                self.selected.take()
            }
        }
        let extensions = EditorExtensions::default();
        let mut module = DebugModule::new(&extensions);
        module.restore_workspace(None, "");
        let mut documents = EditorSession::new();
        let mut terminals = Terminals::default();
        let mut picker = Picker {
            selected: Some("chosen-program".into()),
            calls: Vec::new(),
        };
        let mut services = ModuleServices {
            resources: Default::default(),
            settings_ui: None,
            documents: &mut documents,
            terminals: &mut terminals,
            dialogs: &mut picker,
            active_view: None,
            project_root: "project",
        };
        for _ in 0..2 {
            module
                .borrow_state()
                .debug_action(Action::BrowseProgram(0), &mut services, &mut Vec::new())
                .unwrap();
            assert_eq!(module.borrow_state().profiles[0].program, "chosen-program");
        }
        assert_eq!(
            picker.calls,
            vec![("project".into(), Vec::<String>::new()); 2]
        );
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn loader_stop_reveals_inspection_then_restores_source_focus_without_navigation() {
        let extensions = EditorExtensions::default();
        let mut module = DebugModule::new(&extensions);
        module.restore_workspace(None, "");
        let mut documents = EditorSession::new();
        let document = documents.create_document(b"fn main() {}\n").unwrap();
        let view = documents.create_view(document).unwrap();
        let root = std::env::temp_dir();
        let mut session = bed_debug::DebugSession::launch_with_args(
            Path::new("/bin/sleep"),
            &["30".into()],
            bed_debug::LaunchConfig {
                program: "/unused/program".into(),
                cwd: root.to_string_lossy().into(),
                ..Default::default()
            },
        )
        .unwrap();
        session.state = SessionState::Stopped;
        session.stop_generation = 7;
        session.selected_thread = Some(1);
        session.frames = vec![bed_debug::StackFrame {
            id: 10,
            name: "_dyld_start".into(),
            source: Some("/usr/lib/dyld`_dyld_start".into()),
            line: 1,
            column: 1,
        }];
        session.select_frame(10).unwrap();
        {
            let mut debugger = module.borrow_state();
            debugger.session = Some(session);
            debugger.launch_profile = Some(bed_debug::DebugProfile {
                stop_on_entry: true,
                ..Default::default()
            });
        }
        let mut terminals = Terminals::default();
        let mut dialogs = Dialogs;
        let mut services = ModuleServices {
            resources: Default::default(),
            settings_ui: None,
            documents: &mut documents,
            terminals: &mut terminals,
            dialogs: &mut dialogs,
            active_view: Some(view),
            project_root: root.to_str().unwrap(),
        };
        let mut requests = Vec::new();
        module
            .borrow_state()
            .tick_debugger(&mut services, &mut requests)
            .unwrap();
        let show = requests.iter().position(|request| matches!(request, HostRequest::OpenPanel { panel_type, .. } if panel_type == PANEL_ID)).unwrap();
        let focus = requests.iter().position(|request| matches!(request, HostRequest::FocusView { view: target } if *target == view)).unwrap();
        assert!(show < focus);
        assert!(
            !requests
                .iter()
                .any(|request| matches!(request, HostRequest::RevealSource { .. }))
        );
        assert!(
            module
                .borrow_state()
                .source_notice
                .as_deref()
                .unwrap()
                .contains("Continue (F5)")
        );
        module.shutdown(&mut services);
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn editor_breakpoint_actions_invalidate_host_presentation_once_on_next_tick() {
        let directory = test_support::TempDir::new();
        let path = directory.write("main.rs", b"fn main() {\n}\n");
        let extensions = EditorExtensions::default();
        let mut module = DebugModule::new(&extensions);
        module.restore_workspace(None, directory.root().to_str().unwrap());
        let mut documents = EditorSession::new();
        let document = documents.open_file(&path).unwrap();
        for row in [0, 1] {
            SourceDebugExtension::action(
                &mut *module.borrow_state(),
                &documents,
                document,
                SourceDebugAction::ToggleBreakpoint { row },
            )
            .unwrap();
        }
        let mut terminals = Terminals::default();
        let mut dialogs = Dialogs;
        let mut services = ModuleServices {
            resources: Default::default(),
            settings_ui: None,
            documents: &mut documents,
            terminals: &mut terminals,
            dialogs: &mut dialogs,
            active_view: None,
            project_root: directory.root().to_str().unwrap(),
        };
        let mut requests = Vec::new();
        module
            .borrow_state()
            .tick_debugger(&mut services, &mut requests)
            .unwrap();
        assert!(matches!(requests.as_slice(), [HostRequest::Invalidate]));
        requests.clear();
        module
            .borrow_state()
            .tick_debugger(&mut services, &mut requests)
            .unwrap();
        assert!(requests.is_empty());
    }
}
