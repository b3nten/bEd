//! Settings panel contribution identifiers.
use bed_editor_api::{PanelPlacement, Registrar};
pub const MODULE_ID: &str = "bed.settings";
pub const PANEL_ID: &str = "bed.settings.panel";
pub const NEW_COMMAND: &str = "bed.settings.new";
pub(crate) fn register_metadata(registrar: &mut Registrar<'_>) {
    registrar.panel_options(
        PANEL_ID,
        "Settings",
        false,
        PanelPlacement::Center,
        Some("settings"),
    );
    registrar.command(NEW_COMMAND, "New Settings", Some("settings"));
    registrar.toolbar(NEW_COMMAND);
}
