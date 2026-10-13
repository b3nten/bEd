//! Settings controls hosted through the ordinary workbench module API.
mod feature;
mod panel;
mod ui;
#[cfg(test)]
pub(crate) use crate::test_support;
pub use feature::{SettingsFrontend, SettingsPanelState, SettingsPlugin};
pub use panel::{MODULE_ID, NEW_COMMAND, PANEL_ID};
pub use ui::{SettingsCategory, SettingsUi, SettingsWindowState};
