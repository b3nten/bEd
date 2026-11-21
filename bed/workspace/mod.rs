//! bEd's workspaces: a window shows one folder's workspace (or the default
//! one), with its numbered layouts, and the Workspaces panel to switch
//! between them. The workspace itself is workbench's; this is bEd's policy
//! around it.
pub mod layouts;
mod panel;

pub use bedtime_workbench::workspaces::*;
use bedtime_workbench::{ErrorDialog, ErrorDialogWorldExt};
use bevy::prelude::*;
pub use panel::{WorkspacesPanel, WorkspacesPanelPlugin};
use std::path::PathBuf;

/// bEd's workspaces on top of workbench's (`WorkspacesPlugin`, part of
/// `BedWorkbenchPlugin`): the numbered layouts and the Workspaces panel.
pub struct BedWorkspacesPlugin;

impl Plugin for BedWorkspacesPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((layouts::LayoutSlotsPlugin, WorkspacesPanelPlugin));
    }
}

/// Switch this window to `folder`'s workspace (`None`: the default one),
/// or explain why not.
pub fn switch(world: &mut World, folder: Option<PathBuf>) {
    if let Err(error) = open_workspace(world, folder) {
        world.request_error_dialog(ErrorDialog {
            title: "Open Folder".into(),
            message: error,
            details: None,
        });
    }
}

#[cfg(test)]
mod tests {
    use bevy::reflect::TypePath;

    /// Saved layouts name these types by path: moving their modules must
    /// not rename them, or what was saved no longer opens.
    #[test]
    fn saved_types_keep_the_names_they_were_saved_under() {
        assert_eq!(
            super::WorkspacesPanel::type_path(),
            "bed::workspace::WorkspacesPanel"
        );
        assert_eq!(
            super::layouts::LayoutSlot::type_path(),
            "bed::layouts::LayoutSlot"
        );
    }
}
