//! bEd adds its theme catalog/editor to the reusable preferences panel.
mod feature;
pub use bedtime_plugin_settings::{MODULE_ID, NEW_COMMAND, PANEL_ID, SettingsPanelState};
pub use feature::BedSettingsPagesPlugin;
