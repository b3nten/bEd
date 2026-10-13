//! Editor transactions on permanent Bevy resources and generic workbench panels.
extern crate self as bed_editor_workbench;
pub mod commands;
pub mod editor_runtime;
mod editor_ui;
mod shortcuts;
mod status_bar;
pub mod workspace;
pub use bed_workbench::{PanelRecord, WorkbenchLayout};
pub use bed_workbench_api::{PanelGeometry, PanelPresentation as EditorPanel};
pub use editor_runtime::*;
pub use editor_ui::{draw_ui, set_root_top_inset};
pub use workspace::store::{WorkspaceSpec, WorkspaceStore, WorkspaceTarget};

/// Editor and generic workbench read the same canonical layout resource.
pub type EditorLayout = WorkbenchLayout;
#[cfg(test)]
pub(crate) static IMGUI_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
#[cfg(test)]
#[allow(dead_code)]
#[path = "../../../tests/support/temp_dir.rs"]
pub(crate) mod test_support;
