//! Project picker panels and their concrete workspace action contract.
mod feature;
pub mod welcome;
#[cfg(feature = "scenes")]
pub use feature::ProjectsPreview;
pub use feature::{ProjectsFrontend, ProjectsPanelState, ProjectsPlugin};

use bed_editor_api::workspace::WorkspaceSpec;
use welcome::WelcomeAction;

pub const MODULE_ID: &str = "bed.projects";
pub const PANEL_ID: &str = "bed.projects.panel";
pub const NEW_COMMAND: &str = "bed.projects.new";

/// Requests are applied by the native workspace service after panel drawing.
/// They retain the workspace selected in the originating Projects panel.
pub use bed_editor_api::ProjectRequest as ProjectsAction;

#[derive(Default, bevy::prelude::Resource)]
pub struct ProjectsState {
    pub recent_workspaces: Vec<WorkspaceSpec>,
    pub connecting: bool,
    pub disconnected: bool,
}

fn project_requests(action: WelcomeAction) -> Vec<ProjectsAction> {
    let mut requests = Vec::new();
    if action.open_folder {
        requests.push(ProjectsAction::OpenFolder);
    }
    if let Some(path) = action.project {
        requests.push(ProjectsAction::OpenProject(path));
    }
    if let Some(spec) = action.workspace.or(action.connect) {
        requests.push(ProjectsAction::OpenWorkspace(spec));
    }
    if let Some(spec) = action.workspace_window {
        requests.push(ProjectsAction::OpenWorkspaceWindow(spec));
    }
    if let Some(path) = action.remove_recent {
        requests.push(ProjectsAction::RemoveRecent(path));
    }
    if let Some(spec) = action.remove_workspace {
        requests.push(ProjectsAction::RemoveWorkspace(spec));
    }
    if let Some((spec, name)) = action.rename_workspace {
        requests.push(ProjectsAction::RenameWorkspace(spec, name));
    }
    if let Some(error) = action.error {
        requests.push(ProjectsAction::Error(error));
    }
    requests
}

#[cfg(test)]
static IMGUI_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
