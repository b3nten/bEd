//! Project picker panels and their concrete workspace action contract.
pub mod welcome;

use bed_document_session::DocumentId;
use bed_workbench_api::workspace::WorkspaceSpec;
use bed_workbench_api::{
    CommandContext, HostContext, HostRequest, Module, ModulePanel, PanelPlacement, Registrar,
};
use dear_imgui_rs::Ui;
use serde_json::Value;
use std::{any::Any, cell::RefCell, path::PathBuf, rc::Rc};
use welcome::{Welcome, WelcomeAction};

pub const MODULE_ID: &str = "bed.projects";
pub const PANEL_ID: &str = "bed.projects.panel";
pub const NEW_COMMAND: &str = "bed.projects.new";

/// Requests are applied by the native workspace service after panel drawing.
/// They retain the workspace selected in the originating Projects panel.
#[derive(Debug)]
pub enum ProjectsAction {
    OpenFolder,
    OpenProject(PathBuf),
    OpenWorkspace(WorkspaceSpec),
    RemoveRecent(PathBuf),
    RemoveWorkspace(WorkspaceSpec),
    RenameWorkspace(WorkspaceSpec, String),
    Reconnect,
    Error(String),
}

#[derive(Default)]
pub struct ProjectsState {
    pub recent_workspaces: Vec<WorkspaceSpec>,
    pub connecting: bool,
    pub disconnected: bool,
    actions: Vec<ProjectsAction>,
}
impl ProjectsState {
    pub fn take_actions(&mut self) -> Vec<ProjectsAction> {
        std::mem::take(&mut self.actions)
    }

    fn accept(&mut self, action: WelcomeAction) {
        if action.open_folder {
            self.actions.push(ProjectsAction::OpenFolder);
        }
        if let Some(path) = action.project {
            self.actions.push(ProjectsAction::OpenProject(path));
        }
        if let Some(spec) = action.workspace.or(action.connect) {
            self.actions.push(ProjectsAction::OpenWorkspace(spec));
        }
        if let Some(path) = action.remove_recent {
            self.actions.push(ProjectsAction::RemoveRecent(path));
        }
        if let Some(spec) = action.remove_workspace {
            self.actions.push(ProjectsAction::RemoveWorkspace(spec));
        }
        if let Some((spec, name)) = action.rename_workspace {
            self.actions
                .push(ProjectsAction::RenameWorkspace(spec, name));
        }
        if let Some(error) = action.error {
            self.actions.push(ProjectsAction::Error(error));
        }
    }
}

pub struct ProjectsModule {
    state: Rc<RefCell<ProjectsState>>,
}
impl ProjectsModule {
    pub fn new(state: Rc<RefCell<ProjectsState>>) -> Self {
        Self { state }
    }
}
impl Module for ProjectsModule {
    fn id(&self) -> &'static str {
        MODULE_ID
    }
    fn register(&self, registrar: &mut Registrar<'_>) {
        registrar.panel_options(
            PANEL_ID,
            "Projects",
            false,
            PanelPlacement::Center,
            Some("projects"),
        );
        registrar.command(NEW_COMMAND, "New Projects Panel", Some("folder"));
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
        panel_type: &str,
        document: Option<DocumentId>,
        _: &Value,
    ) -> Result<Box<dyn ModulePanel>, String> {
        if panel_type != PANEL_ID || document.is_some() {
            return Err("Projects requires its own panel without a document".into());
        }
        Ok(Box::new(ProjectsPanel {
            state: self.state.clone(),
            welcome: Welcome::new(),
        }))
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

pub struct ProjectsPanel {
    state: Rc<RefCell<ProjectsState>>,
    welcome: Welcome,
}
impl ModulePanel for ProjectsPanel {
    fn title(&self, _: &HostContext<'_>) -> String {
        "Projects".into()
    }
    fn draw(&mut self, ui: &Ui, _: &HostContext<'_>, _: &mut Vec<HostRequest>) {
        let mut state = self.state.borrow_mut();
        if state.connecting {
            ui.text_disabled("Connecting over SSH…");
        }
        if state.disconnected {
            ui.text_disabled("SSH connection lost. Your buffers are retained.");
            if ui.button("Reconnect") {
                state.actions.push(ProjectsAction::Reconnect);
            }
        }
        self.welcome
            .set_recent_workspaces(state.recent_workspaces.clone());
        state.accept(self.welcome.draw_body(ui));
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

#[cfg(test)]
pub(crate) static IMGUI_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;
    use bed_workbench_api::Registry;

    #[test]
    fn projects_panels_share_workspace_state_and_keep_distinct_picker_views() {
        let state = Rc::new(RefCell::new(ProjectsState::default()));
        let mut module = ProjectsModule::new(state.clone());
        let mut registry = Registry::default();
        registry.register(&module).unwrap();
        let descriptor = registry.panel(PANEL_ID).unwrap();
        assert_eq!(descriptor.legacy_kind, Some("projects"));
        assert!(!descriptor.singleton);
        let first = module.create_panel(PANEL_ID, None, &Value::Null).unwrap();
        let second = module.create_panel(PANEL_ID, None, &Value::Null).unwrap();
        let first = first.as_any().downcast_ref::<ProjectsPanel>().unwrap();
        let second = second.as_any().downcast_ref::<ProjectsPanel>().unwrap();
        assert!(Rc::ptr_eq(&first.state, &second.state));
        assert!(
            module
                .create_panel(PANEL_ID, Some(DocumentId::next()), &Value::Null)
                .is_err()
        );
        let spec = WorkspaceSpec::local("/example/project");
        state.borrow_mut().accept(WelcomeAction {
            rename_workspace: Some((spec.clone(), "Renamed".into())),
            ..Default::default()
        });
        assert!(
            matches!(second.state.borrow_mut().take_actions().as_slice(),
            [ProjectsAction::RenameWorkspace(target, name)] if target == &spec && name == "Renamed")
        );
        assert!(first.state.borrow_mut().take_actions().is_empty());
    }
}
