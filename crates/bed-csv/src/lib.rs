//! CSV/TSV tables backed by the host's shared, revision-checked text document.
mod ecs;
mod model;
mod panel;
mod status;
mod worker;

pub use ecs::{CsvDocument, CsvPlugin, CsvStates, CsvSystems};

use bed_editor_api::{DocumentKind, Registrar};

pub use panel::CsvPanel;
pub const PLUGIN_ID: &str = "bed.csv";
pub const PANEL_ID: &str = "bed.csv.panel";
pub const VIEWER_ID: &str = "bed.csv.viewer";
pub const OPEN_COMMAND: &str = "bed.csv.open";
pub const OPEN_FILE_COMMAND: &str = "bed.csv.open-file";

fn register_metadata(registrar: &mut Registrar<'_>) {
    registrar.panel(PANEL_ID, "CSV Table");
    registrar.panel_requires_input(PANEL_ID);
    registrar.viewer(
        VIEWER_ID,
        "CSV Table",
        PANEL_ID,
        &["csv", "tsv"],
        DocumentKind::Text,
    );
    registrar.command(OPEN_COMMAND, "Open CSV/TSV…", None);
    registrar.command(OPEN_FILE_COMMAND, "Open in CSV Table", None);
}

fn supported_path(path: &str) -> bool {
    path.rsplit(['/', '\\'])
        .next()
        .and_then(|name| name.rsplit_once('.'))
        .is_some_and(|(_, ext)| ["csv", "tsv"].iter().any(|v| v.eq_ignore_ascii_case(ext)))
}

pub(crate) fn shortcut_actions() -> Vec<bed_keybindings::Action> {
    vec![
        bed_keybindings::Action::new(
            "bed.csv.clear",
            "Clear Cells",
            PLUGIN_ID,
            bed_keybindings::Scope::panel(PANEL_ID),
            &["Delete", "Backspace"],
        ),
        bed_keybindings::Action::new(
            "bed.csv.edit",
            "Edit Cell",
            PLUGIN_ID,
            bed_keybindings::Scope::panel(PANEL_ID),
            &["Enter", "F2"],
        ),
        bed_keybindings::Action::new(
            "bed.csv.next_cell",
            "Next Cell",
            PLUGIN_ID,
            bed_keybindings::Scope::panel(PANEL_ID),
            &["Tab"],
        ),
        bed_keybindings::Action::new(
            "bed.csv.previous_cell",
            "Previous Cell",
            PLUGIN_ID,
            bed_keybindings::Scope::panel(PANEL_ID),
            &["Shift+Tab"],
        ),
        bed_keybindings::Action::new(
            "bed.csv.move_up",
            "Move Up",
            PLUGIN_ID,
            bed_keybindings::Scope::panel(PANEL_ID),
            &["Up"],
        ),
        bed_keybindings::Action::new(
            "bed.csv.move_down",
            "Move Down",
            PLUGIN_ID,
            bed_keybindings::Scope::panel(PANEL_ID),
            &["Down"],
        ),
        bed_keybindings::Action::new(
            "bed.csv.move_left",
            "Move Left",
            PLUGIN_ID,
            bed_keybindings::Scope::panel(PANEL_ID),
            &["Left"],
        ),
        bed_keybindings::Action::new(
            "bed.csv.move_right",
            "Move Right",
            PLUGIN_ID,
            bed_keybindings::Scope::panel(PANEL_ID),
            &["Right"],
        ),
        bed_keybindings::Action::new(
            "bed.csv.select_up",
            "Extend Selection Up",
            PLUGIN_ID,
            bed_keybindings::Scope::panel(PANEL_ID),
            &["Shift+Up"],
        ),
        bed_keybindings::Action::new(
            "bed.csv.select_down",
            "Extend Selection Down",
            PLUGIN_ID,
            bed_keybindings::Scope::panel(PANEL_ID),
            &["Shift+Down"],
        ),
        bed_keybindings::Action::new(
            "bed.csv.select_left",
            "Extend Selection Left",
            PLUGIN_ID,
            bed_keybindings::Scope::panel(PANEL_ID),
            &["Shift+Left"],
        ),
        bed_keybindings::Action::new(
            "bed.csv.select_right",
            "Extend Selection Right",
            PLUGIN_ID,
            bed_keybindings::Scope::panel(PANEL_ID),
            &["Shift+Right"],
        ),
    ]
}
pub(crate) fn default_shortcuts() -> bed_keybindings::ShortcutInput {
    bed_keybindings::ShortcutInput::new(bed_keybindings::Bindings::new(
        bed_keybindings::common_actions()
            .into_iter()
            .chain(shortcut_actions())
            .collect(),
        &Default::default(),
        bed_keybindings::Platform::current(),
    ))
    .with_scope(bed_keybindings::Scope::panel(PANEL_ID))
}
