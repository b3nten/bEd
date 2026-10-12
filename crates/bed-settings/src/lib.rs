//! Settings controls hosted through the ordinary workbench module API.
mod feature;
mod panel;
mod theme_editor;
mod ui;
pub use feature::{SettingsFrontend, SettingsPanelState, SettingsPlugin};
pub use panel::{MODULE_ID, NEW_COMMAND, PANEL_ID};
pub use ui::{SettingsCategory, SettingsUi, SettingsWindowState};
#[cfg(test)]
static IMGUI_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
#[cfg(test)]
#[allow(dead_code)]
#[path = "../tests/support/temp_dir.rs"]
mod test_support;
