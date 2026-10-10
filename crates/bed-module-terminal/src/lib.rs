//! Terminal panels use the shared terminal service; panels do not own processes.
use bed_document_session::DocumentId;
use bed_workbench_api::{
    CommandContext, HostContext, HostRequest, Module, ModulePanel, ModuleServices, PanelPlacement,
    Registrar,
};
use dear_imgui_rs::Ui;
use serde_json::{Value, json};
use std::{
    any::Any,
    io,
    path::{Path, PathBuf},
};

pub const MODULE_ID: &str = "bed.terminal";
pub const PANEL_ID: &str = "bed.terminal.panel";
pub const NEW_COMMAND: &str = "bed.terminal.new";

fn shell_directory(saved: Option<&str>, base_directory: &str, remote: bool) -> Option<PathBuf> {
    saved
        .filter(|path| remote || Path::new(path).is_dir())
        .map(PathBuf::from)
        .or_else(|| (!base_directory.is_empty()).then(|| PathBuf::from(base_directory)))
}

#[derive(Default)]
pub struct TerminalModule;
impl Module for TerminalModule {
    fn id(&self) -> &'static str {
        MODULE_ID
    }
    fn register(&self, registrar: &mut Registrar<'_>) {
        registrar.panel_options(
            PANEL_ID,
            "Terminal",
            false,
            PanelPlacement::Bottom,
            Some("terminal"),
        );
        registrar.panel_open_placement(PANEL_ID, PanelPlacement::Center);
        registrar.command(NEW_COMMAND, "New Terminal", Some("terminal"));
    }
    fn command(
        &mut self,
        command: &str,
        _: &CommandContext,
        _: &HostContext<'_>,
        requests: &mut Vec<HostRequest>,
    ) {
        if command == NEW_COMMAND {
            requests.push(HostRequest::OpenPanel {
                panel_type: PANEL_ID.into(),
                document: None,
                state: Value::Null,
            });
        }
    }
    fn create_panel(
        &mut self,
        _: &str,
        _: Option<DocumentId>,
        _: &Value,
    ) -> Result<Box<dyn ModulePanel>, String> {
        Err("Terminal panels require terminal services".into())
    }
    fn create_panel_with_services(
        &mut self,
        panel_type: &str,
        input: bed_workbench_api::PanelInput,
        state: &Value,
        services: &mut ModuleServices<'_>,
    ) -> Result<Box<dyn ModulePanel>, String> {
        if input.local_file().is_some() {
            return Err("This module cannot open a local-file panel".into());
        }
        let document = input.document();
        if panel_type != PANEL_ID || document.is_some() {
            return Err("Terminal requires its own panel without a document".into());
        }
        // A session ID is only a live attachment request. Persisted state omits
        // it so reopening a workspace always creates a fresh shell.
        let session = if let Some(session) = state["session"].as_u64() {
            if services.terminals.title(session).is_none() {
                return Err("Terminal session is no longer available".into());
            }
            session
        } else {
            // Existing/restored shells keep their own cwd; fresh shells use
            // the window base and never inspect the last focused process.
            let cwd = shell_directory(
                state["cwd"].as_str(),
                services.working_directory,
                services.documents.is_remote(),
            );
            services
                .terminals
                .new_shell(cwd.as_deref())
                .map_err(|error| error.to_string())?
        };
        let title = services
            .terminals
            .title(session)
            .unwrap_or_else(|| format!("Terminal {session}"));
        let cwd = services.terminals.working_directory(session);
        let command = services.terminals.is_command(session);
        services.terminals.focus(session);
        Ok(Box::new(TerminalPanel {
            session,
            title,
            cwd,
            command,
        }))
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

pub struct TerminalPanel {
    session: u64,
    title: String,
    cwd: Option<PathBuf>,
    command: bool,
}
impl TerminalPanel {
    pub fn session_id(&self) -> u64 {
        self.session
    }
    pub fn is_command_session(&self) -> bool {
        self.command
    }
}
impl ModulePanel for TerminalPanel {
    fn title(&self, _: &HostContext<'_>) -> String {
        self.title.clone()
    }
    fn draw(&mut self, ui: &Ui, _: &HostContext<'_>, _: &mut Vec<HostRequest>) {
        ui.text_disabled("Terminal panel requires terminal services");
    }
    fn draw_with_services(
        &mut self,
        ui: &Ui,
        _: &HostContext<'_>,
        services: &mut ModuleServices<'_>,
        _: &mut Vec<HostRequest>,
    ) -> io::Result<()> {
        if services.project_root.is_empty()
            && let Some(cwd) = services
                .terminals
                .live_working_directory(self.session)
                .ok()
                .or_else(|| services.terminals.working_directory(self.session))
        {
            ui.text_disabled(cwd.to_string_lossy());
            if ui.is_item_hovered() {
                ui.tooltip_text(cwd.to_string_lossy());
            }
        }
        services.terminals.render(ui, self.session)?;
        if let Some(title) = services.terminals.title(self.session) {
            self.title = title;
        }
        self.cwd = services.terminals.working_directory(self.session);
        Ok(())
    }
    fn focus_with_services(&mut self, services: &mut ModuleServices<'_>) -> io::Result<()> {
        services.terminals.focus(self.session);
        Ok(())
    }
    fn persist(&self) -> bool {
        !self.command
    }
    fn save_state(&self) -> Value {
        json!({ "cwd": self.cwd, "command": self.command })
    }
    fn close_with_services(
        &mut self,
        services: &mut ModuleServices<'_>,
        _: &mut Vec<HostRequest>,
    ) -> io::Result<()> {
        // The service protects sessions retained by the debugger or another
        // feature while releasing ordinary shells with their last panel.
        services.terminals.close_panel(self.session);
        Ok(())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

#[cfg(test)]
#[allow(dead_code)]
#[path = "../../../tests/support/temp_dir.rs"]
pub(crate) mod test_support;

#[cfg(test)]
mod tests {
    use super::*;
    use bed_document_session::EditorSession;
    use bed_workbench_api::{FileDialogService, Registry, TerminalLaunch, TerminalService};
    use std::{collections::HashMap, path::Path};

    #[derive(Default)]
    struct Terminals {
        next: u64,
        shells: Vec<PathBuf>,
        sessions: HashMap<u64, (PathBuf, bool)>,
        closed_panels: Vec<u64>,
        focused: Vec<u64>,
        live_directories: HashMap<u64, PathBuf>,
    }
    impl TerminalService for Terminals {
        fn new_shell(&mut self, cwd: Option<&Path>) -> io::Result<u64> {
            self.next += 1;
            let cwd = cwd.unwrap_or(Path::new("/default")).to_owned();
            self.shells.push(cwd.clone());
            self.sessions.insert(self.next, (cwd, false));
            Ok(self.next)
        }
        fn title(&self, id: u64) -> Option<String> {
            self.sessions.get(&id).map(|_| format!("Terminal {id}"))
        }
        fn working_directory(&self, id: u64) -> Option<PathBuf> {
            self.sessions.get(&id).map(|(cwd, _)| cwd.clone())
        }
        fn active_session(&self) -> Option<u64> {
            self.focused.last().copied()
        }
        fn live_working_directory(&self, id: u64) -> io::Result<PathBuf> {
            self.live_directories
                .get(&id)
                .cloned()
                .ok_or_else(|| io::Error::other("No running local terminal"))
        }
        fn is_command(&self, id: u64) -> bool {
            self.sessions.get(&id).is_some_and(|(_, command)| *command)
        }
        fn focus(&mut self, id: u64) {
            self.focused.push(id);
        }
        fn close_panel(&mut self, id: u64) {
            self.closed_panels.push(id);
        }
        fn spawn(&mut self, _: TerminalLaunch) -> io::Result<(u64, u32)> {
            Err(io::Error::other("not used"))
        }
        fn stop(&mut self, _: u64) {}
        fn release(&mut self, _: u64) {
            panic!("panels must use close_panel policy");
        }
    }
    struct Dialogs;
    impl FileDialogService for Dialogs {
        fn pick_file(&mut self, _: &Path, _: &[&str]) -> Option<PathBuf> {
            None
        }
    }

    #[test]
    fn terminal_panels_restore_shell_directory_without_reusing_a_saved_process_id() {
        let directory = crate::test_support::TempDir::new();
        let original_root = directory.path("original");
        let another_root = directory.path("another");
        std::fs::create_dir_all(&original_root).unwrap();
        std::fs::create_dir_all(&another_root).unwrap();
        let mut module = TerminalModule::default();
        let mut registry = Registry::default();
        registry.register(&module).unwrap();
        let descriptor = registry.panel(PANEL_ID).unwrap();
        assert_eq!(descriptor.legacy_kind, Some("terminal"));
        assert_eq!(descriptor.placement, PanelPlacement::Bottom);
        assert!(!descriptor.singleton);
        let mut documents = EditorSession::new();
        let mut terminals = Terminals::default();
        let mut dialogs = Dialogs;
        let mut services = ModuleServices {
            documents: &mut documents,
            terminals: &mut terminals,
            dialogs: &mut dialogs,
            project_root: original_root.to_str().unwrap(),
            working_directory: original_root.to_str().unwrap(),
            active_view: None,
            resources: Default::default(),
            settings_ui: None,
        };
        let mut panel = module
            .create_panel_with_services(
                PANEL_ID,
                bed_workbench_api::PanelInput::None,
                &Value::Null,
                &mut services,
            )
            .unwrap();
        let original_session = panel
            .as_any()
            .downcast_ref::<TerminalPanel>()
            .unwrap()
            .session_id();
        assert!(panel.persist());
        let saved = panel.save_state();
        assert!(saved.get("session").is_none());
        assert_eq!(saved["cwd"], json!(original_root));
        panel
            .close_with_services(&mut services, &mut Vec::new())
            .unwrap();
        services.project_root = another_root.to_str().unwrap();
        let restored = module
            .create_panel_with_services(
                PANEL_ID,
                bed_workbench_api::PanelInput::None,
                &saved,
                &mut services,
            )
            .unwrap();
        assert_ne!(
            restored
                .as_any()
                .downcast_ref::<TerminalPanel>()
                .unwrap()
                .session_id(),
            original_session
        );
        assert_eq!(restored.save_state()["cwd"], json!(original_root));
        assert_eq!(terminals.closed_panels, vec![original_session]);
        assert_eq!(terminals.shells, vec![original_root; 2]);
    }

    #[test]
    fn fresh_panels_use_window_base_and_restored_directories_take_precedence() {
        let directory = crate::test_support::TempDir::new();
        let project = directory.path("project");
        let live = directory.path("after-cd");
        let saved = directory.path("saved");
        for path in [&project, &live, &saved] {
            std::fs::create_dir_all(path).unwrap();
        }
        let mut module = TerminalModule::default();
        let mut documents = EditorSession::new();
        let mut terminals = Terminals::default();
        terminals.live_directories.insert(41, live.clone());
        terminals.focused.push(41);
        let mut dialogs = Dialogs;
        let mut services = ModuleServices {
            documents: &mut documents,
            terminals: &mut terminals,
            dialogs: &mut dialogs,
            project_root: project.to_str().unwrap(),
            working_directory: project.to_str().unwrap(),
            active_view: None,
            resources: Default::default(),
            settings_ui: None,
        };
        let panel = module
            .create_panel_with_services(
                PANEL_ID,
                bed_workbench_api::PanelInput::None,
                &Value::Null,
                &mut services,
            )
            .unwrap();
        assert_eq!(panel.save_state()["cwd"], json!(project));
        let pending = module
            .create_panel_with_services(
                PANEL_ID,
                bed_workbench_api::PanelInput::None,
                &Value::Null,
                &mut services,
            )
            .unwrap();
        assert_eq!(pending.save_state()["cwd"], json!(project));
        services.terminals.focus(41);
        let restored = module
            .create_panel_with_services(
                PANEL_ID,
                bed_workbench_api::PanelInput::None,
                &json!({"cwd":saved}),
                &mut services,
            )
            .unwrap();
        assert_eq!(restored.save_state()["cwd"], json!(saved));
        // An unavailable terminal keeps the normal project default.
        services.terminals.focus(99);
        let fallback = module
            .create_panel_with_services(
                PANEL_ID,
                bed_workbench_api::PanelInput::None,
                &Value::Null,
                &mut services,
            )
            .unwrap();
        assert_eq!(fallback.save_state()["cwd"], json!(project));
    }

    #[test]
    fn invalid_saved_local_directories_fall_back_to_the_current_project() {
        let directory = crate::test_support::TempDir::new();
        let project_root = directory.path("project");
        std::fs::create_dir_all(&project_root).unwrap();
        let missing = directory.path("deleted-project");
        let regular_file = directory.write("not-a-directory", b"file");
        let mut module = TerminalModule::default();
        let mut documents = EditorSession::new();
        let mut terminals = Terminals::default();
        let mut dialogs = Dialogs;
        let mut services = ModuleServices {
            documents: &mut documents,
            terminals: &mut terminals,
            dialogs: &mut dialogs,
            project_root: project_root.to_str().unwrap(),
            working_directory: project_root.to_str().unwrap(),
            active_view: None,
            resources: Default::default(),
            settings_ui: None,
        };
        for state in [
            json!({"cwd":missing}),
            json!({"cwd":regular_file}),
            Value::Null,
        ] {
            let panel = module
                .create_panel_with_services(
                    PANEL_ID,
                    bed_workbench_api::PanelInput::None,
                    &state,
                    &mut services,
                )
                .unwrap();
            assert_eq!(panel.save_state()["cwd"], json!(project_root));
        }
        assert_eq!(terminals.shells, vec![project_root; 3]);
    }

    #[test]
    fn remote_directories_are_not_validated_against_the_local_filesystem() {
        let directory = crate::test_support::TempDir::new();
        let remote_only = directory.path("exists-only-on-ssh-host");
        assert!(!remote_only.exists());
        assert_eq!(
            shell_directory(remote_only.to_str(), "/remote/project", true),
            Some(remote_only.clone()),
        );
        assert_eq!(
            shell_directory(
                remote_only.to_str(),
                directory.root().to_str().unwrap(),
                false
            ),
            Some(directory.root().to_owned()),
        );
        assert_eq!(shell_directory(None, "", false), None);
    }

    #[test]
    fn debugger_terminal_attachment_keeps_service_owned_process_and_is_not_persisted() {
        let mut module = TerminalModule::default();
        let mut documents = EditorSession::new();
        let mut terminals = Terminals::default();
        terminals
            .sessions
            .insert(41, (PathBuf::from("/debug/project"), true));
        let mut dialogs = Dialogs;
        let mut services = ModuleServices {
            documents: &mut documents,
            terminals: &mut terminals,
            dialogs: &mut dialogs,
            project_root: "/project",
            working_directory: "/project",
            active_view: None,
            resources: Default::default(),
            settings_ui: None,
        };
        let mut panel = module
            .create_panel_with_services(
                PANEL_ID,
                bed_workbench_api::PanelInput::None,
                &json!({"session":41}),
                &mut services,
            )
            .unwrap();
        assert!(
            !panel.persist(),
            "command transcripts must not reopen as shells"
        );
        assert_eq!(
            panel
                .as_any()
                .downcast_ref::<TerminalPanel>()
                .unwrap()
                .session_id(),
            41
        );
        panel.focus_with_services(&mut services).unwrap();
        panel
            .close_with_services(&mut services, &mut Vec::new())
            .unwrap();
        assert!(
            module
                .create_panel_with_services(
                    PANEL_ID,
                    bed_workbench_api::PanelInput::None,
                    &json!({"session":99}),
                    &mut services
                )
                .is_err()
        );
        assert!(
            module
                .create_panel_with_services(
                    PANEL_ID,
                    Some(DocumentId::next()).into(),
                    &Value::Null,
                    &mut services
                )
                .is_err()
        );
        assert_eq!(terminals.closed_panels, vec![41]);
        assert!(terminals.sessions.contains_key(&41));
        assert!(terminals.shells.is_empty());
        assert_eq!(terminals.focused, vec![41, 41]);
    }
}
