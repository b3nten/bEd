//! Settings panel contribution identifiers.
use bed_api::{features::FeatureRegistrar, workbench::PanelPlacement};
pub const MODULE_ID: &str = "bed.settings";
pub const PANEL_ID: &str = "bed.settings.panel";
pub const NEW_COMMAND: &str = "bed.settings.new";

pub(crate) fn register_metadata(registrar: &mut FeatureRegistrar<'_>) {
    registrar.panel_options(PANEL_ID, false, PanelPlacement::Center, Some("settings"));
    registrar.command_ref(NEW_COMMAND);
    registrar.toolbar(NEW_COMMAND);
}
