//! The workbench as bEd sets it up: bedtime's UI and workbench under bEd's
//! name and config folder, documents, the folder it opens at startup, its
//! themes and its settings (and the Language Servers panel, which must come
//! before the editor).
use crate::{settings, themes};
use bedtime_workbench::{AppPlugins, workspaces::WorkspacesPlugin};
use bevy::prelude::*;

pub struct BedWorkbenchPlugin;

impl Plugin for BedWorkbenchPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            // Bevy's plugins are the app's (`bed::default_plugins`).
            AppPlugins::new("bEd").without_bevy_plugins(),
            // bEd's panels show documents.
            bedtime_workbench::DocumentsPlugins,
            // Right after persistence: it reads the recent folders and
            // adds the settings layers the rest use.
            WorkspacesPlugin::default(),
            themes::plugin(),
            // Before bedtime-code's `CodePlugin` (in `plugins`): added after
            // it, files no longer open (cause not yet found).
            bedtime_code::lsp::LanguageServersPanelPlugin,
            settings::BedSettingsPlugin,
        ));
    }
}
