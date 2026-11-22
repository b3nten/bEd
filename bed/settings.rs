//! bEd's settings: its own defaults for plugins' settings, and the order of
//! the Settings panel's sections. The settings themselves belong to the
//! crates whose behaviour they change (appearance and autosave to workbench,
//! the editor's to bedtime-code, the file tree's to bedtime-file-explorer,
//! the screen effects to `effects`), and show in the sections they declare.
use bedtime_workbench::{
    appearance::ThemeEditorPlugin,
    settings::{SettingsAppExt, SettingsPanelPlugin, ui::SettingsUiAppExt},
};
use bevy::prelude::*;

pub struct BedSettingsPlugin;

impl Plugin for BedSettingsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((SettingsPanelPlugin, ThemeEditorPlugin))
            .set_setting_default::<bedtime_workbench::Autosave>(true)
            // Sections not named here follow, as their plugins were added.
            .settings_sections([
                "Appearance",
                "Window",
                "Panels",
                "Editor",
                "Documents",
                "Files",
            ]);
    }
}
