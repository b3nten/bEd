//! CSV/TSV tables backed by the host's shared, revision-checked text document.
mod ecs;
mod model;
mod panel;
mod status;
mod worker;

pub use ecs::{CsvDocument, CsvStates, CsvSystems, CsvTablePlugin};

use bedtime_api::{documents::DocumentKind, features::FeatureRegistrar};

pub use panel::CsvPanel;
pub const PLUGIN_ID: &str = "bedtime.csv";
pub const PANEL_ID: &str = "bedtime.csv.panel";
pub const VIEWER_ID: &str = "bedtime.csv.viewer";
pub const OPEN_COMMAND: &str = "bedtime.csv.open";
pub const OPEN_FILE_COMMAND: &str = "bedtime.csv.open-file";

fn register_metadata(registrar: &mut FeatureRegistrar<'_>) {
    registrar.panel_ref(PANEL_ID);
    registrar.panel_requires_input(PANEL_ID);
    registrar.viewer(
        VIEWER_ID,
        "CSV Table",
        PANEL_ID,
        &["csv", "tsv"],
        DocumentKind::Text,
    );
    registrar.command_ref(OPEN_COMMAND);
    registrar.command_ref(OPEN_FILE_COMMAND);
}

fn supported_path(path: &str) -> bool {
    path.rsplit(['/', '\\'])
        .next()
        .and_then(|name| name.rsplit_once('.'))
        .is_some_and(|(_, ext)| ["csv", "tsv"].iter().any(|v| v.eq_ignore_ascii_case(ext)))
}

pub(crate) fn shortcut_actions() -> Vec<bedtime_keybindings::ShortcutAction> {
    vec![
        bedtime_keybindings::ShortcutAction::new(
            "bedtime.csv.clear",
            "Clear Cells",
            PLUGIN_ID,
            bedtime_keybindings::Scope::panel(PANEL_ID),
            &["Delete", "Backspace"],
        ),
        bedtime_keybindings::ShortcutAction::new(
            "bedtime.csv.edit",
            "Edit Cell",
            PLUGIN_ID,
            bedtime_keybindings::Scope::panel(PANEL_ID),
            &["Enter", "F2"],
        ),
        bedtime_keybindings::ShortcutAction::new(
            "bedtime.csv.next_cell",
            "Next Cell",
            PLUGIN_ID,
            bedtime_keybindings::Scope::panel(PANEL_ID),
            &["Tab"],
        ),
        bedtime_keybindings::ShortcutAction::new(
            "bedtime.csv.previous_cell",
            "Previous Cell",
            PLUGIN_ID,
            bedtime_keybindings::Scope::panel(PANEL_ID),
            &["Shift+Tab"],
        ),
    ]
}

pub(crate) fn default_shortcuts() -> bedtime_keybindings::ShortcutInput {
    bedtime_keybindings::ShortcutInput::new(bedtime_keybindings::Bindings::new(
        bedtime_keybindings::common_actions()
            .into_iter()
            .chain(shortcut_actions())
            .collect(),
        &Default::default(),
        bedtime_keybindings::Platform::current(),
    ))
    .with_scope(bedtime_keybindings::Scope::panel(PANEL_ID))
}
