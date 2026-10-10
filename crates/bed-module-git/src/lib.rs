//! Git workspace workflow. Repository workers outlive the panels that display them.
mod conflicts;
mod controller;
mod jobs;
mod panels;

use bed_document_session::{DocumentId, EditorSession, SessionEvent};
use bed_editor_ui::extensions::SourceGitExtension;
use bed_module_editor::{EditorConfig, EditorRuntime};
use bed_workbench_api::{
    CommandContext, HostContext, HostRequest, MenuSlot, Module, ModulePanel, ModuleServices,
    PanelPlacement, Registrar, SaveToken, SavedDocument,
};
use controller::GitController;
use serde_json::{Value, json};
use std::{any::Any, cell::RefCell, io, rc::Rc};

pub const MODULE_ID: &str = "bed.git";
pub const PANEL_ID: &str = "bed.git.panel";
pub const DIFF_PANEL_ID: &str = "bed.git.diff";
pub const SHOW_COMMAND: &str = "bed.git.show";
pub const REFRESH_COMMAND: &str = "bed.git.refresh";
pub const DIFF_COMMAND: &str = "bed.git.diff_file";

pub struct GitModule {
    state: Rc<RefCell<GitController>>,
    config: Rc<RefCell<EditorConfig>>,
    runtime: Rc<RefCell<EditorRuntime>>,
}

impl GitModule {
    pub fn new(config: Rc<RefCell<EditorConfig>>, runtime: Rc<RefCell<EditorRuntime>>) -> Self {
        let state = Rc::new(RefCell::new(GitController::default()));
        let provider: Rc<RefCell<dyn SourceGitExtension>> = state.clone();
        config
            .borrow()
            .options
            .extensions
            .register_source_git(&provider);
        Self {
            state,
            config,
            runtime,
        }
    }
    /// Exercise the normal sidebar and both shared-document diff surfaces.
    pub fn smoke_setup(
        &mut self,
        path: &str,
        services: &mut ModuleServices<'_>,
        requests: &mut Vec<HostRequest>,
    ) -> io::Result<()> {
        let mut state = self.state.borrow_mut();
        state.root = services.project_root.into();
        state.refresh = true;
        requests.push(HostRequest::ShowPanel {
            panel_type: PANEL_ID.into(),
            document: None,
            state: Value::Null,
            action: None,
        });
        state.open_diff(path.into(), bed_git::DiffSide::Staged, requests);
        state.open_diff(path.into(), bed_git::DiffSide::Unstaged, requests);
        Ok(())
    }
    pub fn smoke_ready(&self) -> io::Result<bool> {
        let state = self.state.borrow();
        if let Some(error) = &state.error {
            return Err(io::Error::other(error.clone()));
        }
        Ok(state.status.is_some() && state.diffs.len() >= 2 && !state.busy())
    }
}

impl Module for GitModule {
    fn id(&self) -> &'static str {
        MODULE_ID
    }
    fn register(&self, registrar: &mut Registrar<'_>) {
        registrar.command(SHOW_COMMAND, "Git Changes", Some("git"));
        registrar.command(REFRESH_COMMAND, "Refresh Git Status", None);
        registrar.command(DIFF_COMMAND, "Show File Diff", None);
        registrar.menu(MenuSlot::Application, SHOW_COMMAND);
        registrar.menu(MenuSlot::File, DIFF_COMMAND);
        registrar.menu(MenuSlot::TextSelection, DIFF_COMMAND);
        registrar.toolbar(SHOW_COMMAND);
        registrar.panel_options(PANEL_ID, "Git", true, PanelPlacement::Sidebar, None);
        registrar.panel(DIFF_PANEL_ID, "Git diff");
    }
    fn restore_workspace(&mut self, state: Option<&Value>, root: &str) {
        let mut controller = self.state.borrow_mut();
        controller.cancel();
        *controller = GitController::default();
        controller.root = root.into();
        controller.draft = state
            .and_then(|s| s["commit_message"].as_str())
            .unwrap_or_default()
            .into();
        controller.refresh = true;
    }
    fn save_workspace(&self) -> Value {
        json!({"commit_message": self.state.borrow().draft})
    }
    fn tick_with_services(
        &mut self,
        _: &HostContext<'_>,
        services: &mut ModuleServices<'_>,
        requests: &mut Vec<HostRequest>,
    ) -> io::Result<()> {
        self.state.borrow_mut().tick(services, requests)
    }
    fn document_events(&mut self, _: &EditorSession, events: &[SessionEvent]) -> io::Result<()> {
        if events.iter().any(|event| {
            matches!(
                event,
                SessionEvent::Saved { .. }
                    | SessionEvent::Reloaded { .. }
                    | SessionEvent::PathChanged { .. }
            )
        }) {
            self.state.borrow_mut().refresh = true;
        }
        Ok(())
    }
    fn save_result(&mut self, token: SaveToken, result: Result<Vec<SavedDocument>, String>) {
        self.state.borrow_mut().save_result(token, result);
    }
    fn shutdown(&mut self, _: &mut ModuleServices<'_>) {
        self.state.borrow_mut().cancel();
    }
    fn command_enabled(
        &self,
        command: &str,
        context: &CommandContext,
        _: &HostContext<'_>,
    ) -> bool {
        command != DIFF_COMMAND || context.path.as_ref().is_some_and(|path| !path.is_empty())
    }
    fn command(
        &mut self,
        command: &str,
        _: &CommandContext,
        _: &HostContext<'_>,
        requests: &mut Vec<HostRequest>,
    ) {
        if command == SHOW_COMMAND {
            requests.push(HostRequest::ShowPanel {
                panel_type: PANEL_ID.into(),
                document: None,
                state: Value::Null,
                action: None,
            });
        }
        if command == REFRESH_COMMAND {
            self.state.borrow_mut().refresh = true;
        }
    }
    fn command_with_services(
        &mut self,
        command: &str,
        context: &CommandContext,
        host: &HostContext<'_>,
        services: &mut ModuleServices<'_>,
        requests: &mut Vec<HostRequest>,
    ) -> io::Result<()> {
        if matches!(command, SHOW_COMMAND | DIFF_COMMAND | REFRESH_COMMAND) {
            self.state.borrow_mut().capture_directory(services);
        }
        self.command(command, context, host, requests);
        if command == DIFF_COMMAND
            && let Some(path) = &context.path
        {
            self.state
                .borrow_mut()
                .open_absolute_diff(path, services, requests);
        }
        Ok(())
    }
    fn create_panel(
        &mut self,
        kind: &str,
        document: Option<DocumentId>,
        state: &Value,
    ) -> Result<Box<dyn ModulePanel>, String> {
        if kind == DIFF_PANEL_ID && document.is_none() && state.is_null() {
            return Ok(Box::new(panels::EmptyDiffPanel));
        }
        if kind != PANEL_ID {
            return Err("Git diff panels require document services".into());
        }
        Ok(Box::new(panels::GitPanel::new(self.state.clone())))
    }
    fn create_panel_with_services(
        &mut self,
        kind: &str,
        input: bed_workbench_api::PanelInput,
        state: &Value,
        services: &mut ModuleServices<'_>,
    ) -> Result<Box<dyn ModulePanel>, String> {
        if input.local_file().is_some() {
            return Err("This module cannot open a local-file panel".into());
        }
        let document = input.document();
        if kind == PANEL_ID {
            self.state.borrow_mut().capture_directory(services);
            return self.create_panel(kind, document, state);
        }
        if kind != DIFF_PANEL_ID {
            return Err(format!("Unknown Git panel: {kind}"));
        }
        if document.is_none() && state.is_null() {
            return self.create_panel(kind, document, state);
        }
        panels::DiffPanel::new(
            self.state.clone(),
            self.config.clone(),
            self.runtime.clone(),
            document,
            state,
            services,
        )
        .map(|panel| Box::new(panel) as Box<dyn ModulePanel>)
        .map_err(|e| e.to_string())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn default_diff_is_empty_and_does_not_request_repository_work() {
        let config = Rc::new(RefCell::new(EditorConfig::default()));
        let runtime = Rc::new(RefCell::new(EditorRuntime::default()));
        let mut module = GitModule::new(config, runtime);
        let panel = module
            .create_panel(DIFF_PANEL_ID, None, &Value::Null)
            .unwrap();
        let host = HostContext {
            remote: false,
            documents: &[],
            active_document: None,
            settings: &Value::Null,
            textures: &Default::default(),
            animations: false,
            workspace: 0,
            diagnostics: &Value::Null,
            viewer_menu: None,
            default_viewers: &Value::Null,
        };
        assert_eq!(panel.title(&host), "Git diff");
        assert!(panel.is_input_empty(&host));
        assert_eq!(panel.attached_document(), None);
        assert_eq!(panel.save_state(), Value::Null);
        let state = module.state.borrow();
        assert!(state.root.is_empty());
        assert_eq!(state.visible, 0);
        assert!(state.diff_users.is_empty());
        assert!(!state.busy());
    }

    #[test]
    fn closing_sidebar_preserves_commit_draft_and_registered_editor_capability() {
        let config = Rc::new(RefCell::new(EditorConfig::default()));
        let runtime = Rc::new(RefCell::new(EditorRuntime::default()));
        let mut module = GitModule::new(config, runtime);
        module.restore_workspace(
            Some(&json!({"commit_message":"Work in progress\n\nDetails"})),
            "/tmp",
        );
        let panel = module.create_panel(PANEL_ID, None, &Value::Null).unwrap();
        assert_eq!(module.state.borrow().visible, 1);
        drop(panel);
        assert_eq!(module.state.borrow().visible, 0);
        assert_eq!(
            module.save_workspace()["commit_message"],
            "Work in progress\n\nDetails"
        );
        let panel = module.create_panel(PANEL_ID, None, &Value::Null).unwrap();
        assert_eq!(module.state.borrow().draft, "Work in progress\n\nDetails");
        drop(panel);
        let mut registry = bed_workbench_api::Registry::default();
        registry.register(&module).unwrap();
        assert!(
            registry
                .panels
                .iter()
                .any(|p| p.id == PANEL_ID && p.singleton)
        );
    }
}
