//! Application transactions on permanent Bevy resources and generic workbench panels.
extern crate self as bed_app;
pub mod commands;
pub mod runtime;
mod shortcuts;
mod status_bar;
mod ui;
pub mod workspace;
pub use bed_api::workbench::{PanelGeometry, PanelPresentation};
pub use bed_workbench::{PanelRecord, WorkbenchLayout};
pub use runtime::*;
pub use ui::{draw_ui, set_root_top_inset};
pub use workspace::store::{WorkspaceSpec, WorkspaceStore, WorkspaceTarget};

#[cfg(test)]
pub(crate) static IMGUI_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
#[cfg(test)]
#[allow(dead_code)]
#[path = "../../../tests/support/temp_dir.rs"]
pub(crate) mod test_support;
