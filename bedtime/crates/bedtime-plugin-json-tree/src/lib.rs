//! Read-only JSON and JSONC structure over the shared editor document.
mod ecs;
mod model;
mod panel;
mod worker;

pub use ecs::{JsonDocument, JsonStates, JsonSystems, JsonTreePlugin};

#[cfg(test)]
pub(crate) static IMGUI_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

use bedtime_api::{documents::DocumentKind, features::FeatureRegistrar};

pub use panel::JsonPanel;
pub const PLUGIN_ID: &str = "bedtime.json";
pub const PANEL_ID: &str = "bedtime.json.panel";
pub const VIEWER_ID: &str = "bedtime.json.viewer";
pub const OPEN_COMMAND: &str = "bedtime.json.open";
pub const OPEN_FILE_COMMAND: &str = "bedtime.json.open-file";

fn supported_path(path: &str) -> bool {
    path.rsplit(['/', '\\'])
        .next()
        .and_then(|name| name.rsplit_once('.'))
        .is_some_and(|(_, extension)| {
            ["json", "jsonc"]
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(extension))
        })
}

fn register_metadata(registrar: &mut FeatureRegistrar<'_>) {
    registrar.panel_ref(PANEL_ID);
    registrar.panel_requires_input(PANEL_ID);
    registrar.viewer(
        VIEWER_ID,
        "JSON Tree",
        PANEL_ID,
        &["json", "jsonc"],
        DocumentKind::Text,
    );
    registrar.command_ref(OPEN_COMMAND);
    registrar.command_ref(OPEN_FILE_COMMAND);
}
