//! Read-only SQLite browsing over local files, independent of editor documents.
use bedtime_sqlite as backend;
mod ecs;
mod panel;
mod status;
use bedtime_sqlite as worker;

pub use ecs::{SqliteBrowserPlugin, SqliteFile, SqliteStates, SqliteSystems};

use bedtime_api::features::FeatureRegistrar;

pub use panel::SqlitePanel;
pub const PLUGIN_ID: &str = "bedtime.sqlite";
pub const PANEL_ID: &str = "bedtime.sqlite.panel";
pub const VIEWER_ID: &str = "bedtime.sqlite.viewer";
pub const OPEN_COMMAND: &str = "bedtime.sqlite.open";
pub const OPEN_FILE_COMMAND: &str = "bedtime.sqlite.open-file";

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

pub(crate) fn shortcut_actions() -> Vec<bedtime_keybindings::ShortcutAction> {
    vec![
        bedtime_keybindings::ShortcutAction::new(
            "bedtime.sqlite.run_query",
            "Run Query",
            PLUGIN_ID,
            bedtime_keybindings::Scope::panel(PANEL_ID),
            &["Primary+Enter"],
        ),
        bedtime_keybindings::ShortcutAction::new(
            "bedtime.sqlite.cancel_query",
            "Cancel Query",
            PLUGIN_ID,
            bedtime_keybindings::Scope::panel(PANEL_ID),
            &["Escape"],
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_sqlite_requests_fail_before_opening_a_connection() {
        let mut app = bevy::app::App::new();
        app.add_plugins(SqliteBrowserPlugin);
        app.world_mut()
            .resource_mut::<bedtime_api::files::FileContext>()
            .provider = bedtime_api::files::FileProviderId::Ssh {
            host: "fixture".into(),
        };
        for id in [OPEN_COMMAND, OPEN_FILE_COMMAND] {
            let command = app
                .world()
                .resource::<bedtime_api::workbench::WorkbenchRegistry>()
                .command(id)
                .unwrap()
                .handler;
            assert!(
                app.world_mut()
                    .run_system_with(
                        command,
                        bedtime_api::workbench::CommandInvocation {
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
                .resource_mut::<bedtime_api::features::HostEffects>()
                .take()
                .is_empty()
        );
    }
}
