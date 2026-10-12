//! Read-only JSON and JSONC structure over the shared editor document.
mod ecs;
mod model;
mod panel;
mod worker;

pub use ecs::{JsonDocument, JsonPlugin, JsonStates, JsonSystems};

#[cfg(test)]
pub(crate) static IMGUI_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

use bed_editor_api::{DocumentKind, MenuSlot, Registrar};

pub use panel::JsonPanel;
pub const PLUGIN_ID: &str = "bed.json";
pub const PANEL_ID: &str = "bed.json.panel";
pub const VIEWER_ID: &str = "bed.json.viewer";
pub const OPEN_COMMAND: &str = "bed.json.open";
pub const OPEN_FILE_COMMAND: &str = "bed.json.open-file";

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

fn register_metadata(registrar: &mut Registrar<'_>) {
    registrar.panel(PANEL_ID, "JSON Tree");
    registrar.panel_requires_input(PANEL_ID);
    registrar.viewer(
        VIEWER_ID,
        "JSON Tree",
        PANEL_ID,
        &["json", "jsonc"],
        DocumentKind::Text,
    );
    registrar.command(OPEN_COMMAND, "Open JSON/JSONC…", None);
    registrar.command(OPEN_FILE_COMMAND, "Open in JSON Tree", None);
    registrar.menu(MenuSlot::Application, OPEN_COMMAND);
}
