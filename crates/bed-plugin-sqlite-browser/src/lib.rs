//! Read-only SQLite browsing over local files, independent of editor documents.
mod backend;
mod ecs;
mod panel;
mod status;
mod worker;

pub use ecs::{SqliteBrowserPlugin, SqliteFile, SqliteStates, SqliteSystems};

use bed_api::features::FeatureRegistrar;

pub use panel::SqlitePanel;
pub const PLUGIN_ID: &str = "bed.sqlite";
pub const PANEL_ID: &str = "bed.sqlite.panel";
pub const VIEWER_ID: &str = "bed.sqlite.viewer";
pub const OPEN_COMMAND: &str = "bed.sqlite.open";
pub const OPEN_FILE_COMMAND: &str = "bed.sqlite.open-file";

fn register_metadata(registrar: &mut FeatureRegistrar<'_>) {
    registrar.panel_ref(PANEL_ID);
    registrar.panel_requires_input(PANEL_ID);
    registrar.local_file_viewer(
        VIEWER_ID,
        "SQLite Viewer",
        PANEL_ID,
        &["db", "sqlite", "sqlite3"],
    );
    registrar.command_ref(OPEN_COMMAND);
    registrar.command_ref(OPEN_FILE_COMMAND);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_sqlite_requests_fail_before_opening_a_connection() {
        let mut app = bevy::app::App::new();
        app.add_plugins(SqliteBrowserPlugin);
        app.world_mut()
            .resource_mut::<bed_api::features::HostFrame>()
            .remote = true;
        for id in [OPEN_COMMAND, OPEN_FILE_COMMAND] {
            let command = app
                .world()
                .resource::<bed_api::workbench::WorkbenchRegistry>()
                .command(id)
                .unwrap()
                .handler;
            assert!(
                app.world_mut()
                    .run_system_with(
                        command,
                        bed_api::workbench::CommandInvocation {
                            command: id.into(),
                            source: None,
                            area: 0,
                            arguments: serde_json::json!({"path":"/remote/data.sqlite"})
                        }
                    )
                    .unwrap()
                    .is_err()
            );
        }
        assert!(
            app.world_mut()
                .resource_mut::<bed_api::features::HostEffects>()
                .take()
                .is_empty()
        );
    }
}

pub(crate) fn shortcut_actions() -> Vec<bed_keybindings::ShortcutAction> {
    vec![
        bed_keybindings::ShortcutAction::new(
            "bed.sqlite.run_query",
            "Run Query",
            PLUGIN_ID,
            bed_keybindings::Scope::panel(PANEL_ID),
            &["Primary+Enter"],
        ),
        bed_keybindings::ShortcutAction::new(
            "bed.sqlite.cancel_query",
            "Cancel Query",
            PLUGIN_ID,
            bed_keybindings::Scope::panel(PANEL_ID),
            &["Escape"],
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
