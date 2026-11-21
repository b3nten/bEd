//! The workbench as bEd sets it up: bedtime's UI and workbench under bEd's
//! name and config folder, the folder it opens at startup, its themes and
//! Settings layout, its menus and the command palette (and the Language
//! Servers panel, which must come before the editor).
use crate::{menus, settings, themes};
use bedtime_workbench::{WorkbenchPlugins, workspaces::WorkspacesPlugin};
use bevy::prelude::*;

pub struct BedWorkbenchPlugin;

impl Plugin for BedWorkbenchPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            bedtime_ui::UiPlugin,
            WorkbenchPlugins
                .set(bedtime_workbench::StandardMenusPlugin { app_name: "bEd" })
                .set(bedtime_persistence::PersistencePlugin::for_app("bed")),
            // Right after persistence: it reads the recent folders and
            // adds the settings layers the rest use.
            WorkspacesPlugin::default(),
            themes::plugin(),
            // Before bedtime-code's `CodePlugin` (in `plugins`): added after
            // it, files no longer open (cause not yet found).
            bedtime_code::lsp::LanguageServersPanelPlugin,
            settings::BedSettingsPlugin,
            bedtime_workbench::CommandPalettePlugin,
            menus::MenusPlugin,
        ));
    }
}
