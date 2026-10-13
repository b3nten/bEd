//! Workspace picker panels and their concrete workspace action contract.
mod contracts;
mod feature;
mod lifecycle;
pub use contracts::*;
pub use lifecycle::*;
pub mod welcome;
#[cfg(feature = "mascots")]
pub use feature::WorkspacesPreview;
pub use feature::{WorkspacesFrontend, WorkspacesPanelState, WorkspacesPlugin};

use welcome::WelcomeAction;

pub const MODULE_ID: &str = "bed.workspaces";
pub const PANEL_ID: &str = "bed.workspaces.panel";
pub const NEW_COMMAND: &str = "bed.workspaces.new";

/// Requests are applied by the native workspace service after panel drawing.
/// They retain the workspace selected in the originating Workspaces panel.

#[derive(Default, bevy::prelude::Resource)]
pub struct WorkspacesState {
    pub recent_workspaces: Vec<WorkspaceSpec>,
    pub connecting: bool,
    pub disconnected: bool,
    pub error: Option<String>,
}

fn workspace_actions(action: WelcomeAction) -> Vec<WorkspaceAction> {
    let mut requests = Vec::new();
    if action.open_folder {
        requests.push(WorkspaceAction::OpenFolder);
    }
    if let Some(path) = action.project {
        requests.push(WorkspaceAction::OpenLocalWorkspace(path));
    }
    if let Some(spec) = action.workspace.or(action.connect) {
        requests.push(WorkspaceAction::OpenWorkspace(spec));
    }
    if let Some(spec) = action.workspace_window {
        requests.push(WorkspaceAction::OpenWorkspaceWindow(spec));
    }
    if let Some(path) = action.remove_recent {
        requests.push(WorkspaceAction::RemoveRecent(path));
    }
    if let Some(spec) = action.remove_workspace {
        requests.push(WorkspaceAction::RemoveWorkspace(spec));
    }
    if let Some((spec, name)) = action.rename_workspace {
        requests.push(WorkspaceAction::RenameWorkspace(spec, name));
    }
    if let Some(error) = action.error {
        requests.push(WorkspaceAction::Error(error));
    }
    requests
}

#[cfg(test)]
static IMGUI_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
