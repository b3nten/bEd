//! Workspaces are workbench's (`bedtime_workbench::workspaces`); bed adds the
//! Workspaces panel.
pub use bedtime_workbench::workspaces::*;

mod panel;
pub use panel::{OpenWorkspaces, WorkspacesPanel, WorkspacesPanelPlugin};
