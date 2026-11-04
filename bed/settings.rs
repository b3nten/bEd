//! bEd's settings: its own defaults for plugins' settings, its themes, and
//! how the Settings panel lays them out. The settings
//! themselves belong to the crates whose behaviour they change (appearance
//! and autosave to workbench, the editor's to bedtime-code, the file tree's
//! to bedtime-filetree, the screen effects to `effects`).
use bedtime_workbench::{
    appearance::{
        ColorTheme, FontFamily, FontSize, PanelGap, PanelOpacity, PanelRadius, ThemeEditorPlugin,
        WindowBlurEnabled, WindowOpacity,
    },
    settings::{
        SettingsAppExt, SettingsPanelPlugin,
        ui::{SettingsLayout, SettingsUiAppExt},
    },
    settings_panel,
};
use bevy::prelude::*;

pub struct BedSettingsPlugin;

impl Plugin for BedSettingsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((SettingsPanelPlugin, ThemeEditorPlugin))
            // bEd saves as you go.
            .set_setting_default::<bedtime_workbench::Autosave>(true)
            .set_settings_layout(layout());
    }
}

/// The Settings panel: every setting bEd shows, in its order.
fn layout() -> SettingsLayout {
    use crate::effects::*;
    use bedtime_code::{LineNumbers, WrapLines};
    use bedtime_filetree::{ShowHiddenFiles, ShowIgnoredFiles};
    use bedtime_workbench::{Autosave, workspaces::ReopenLastWorkspace};
    settings_panel! {
        "Appearance" {
            ColorTheme, FontFamily, FontSize,
            EffectsEnabled, Scanlines, Vignette, Bloom, Noise, Curvature, PixelGrid, BurnIn, Pulse,
        }
        "Window" { WindowOpacity, WindowBlurEnabled }
        "Panels" { PanelOpacity, PanelRadius, PanelGap }
        "Editor" { WrapLines, Autosave, LineNumbers }
        "Files" { ShowHiddenFiles, ShowIgnoredFiles }
        "Startup" { ReopenLastWorkspace }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bedtime_workbench::appearance::AppearanceSettings;
    use bedtime_workbench::{
        KeyChord, WorkbenchCorePlugins,
        settings::{SettingsPanel, ui::SettingsUi},
    };

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins((
            WorkbenchCorePlugins,
            bedtime_workbench::appearance::AppearancePlugin,
            bedtime_workbench::DocumentSettingsPlugin,
            BedSettingsPlugin,
            crate::layouts::LayoutSlotsPlugin,
            // bEd's themes, without writing a folder.
            bedtime_workbench::appearance::ThemesPlugin {
                folder: None,
                ..crate::themes::plugin()
            },
        ));
        app
    }

    #[test]
    fn the_theme_and_autosave_have_beds_defaults() {
        let mut app = app();
        app.update();
        let world = app.world();
        assert_eq!(
            world.resource::<AppearanceSettings>().theme,
            crate::themes::DEFAULT
        );
        assert!(
            world
                .resource::<bedtime_workbench::DocumentAutosave>()
                .enabled()
        );
    }

    #[test]
    fn the_panel_shows_beds_sections_in_order() {
        let mut app = app();
        app.update();
        let sections: Vec<_> = app
            .world()
            .resource::<SettingsUi>()
            .sections()
            .into_iter()
            .map(|(section, _)| section)
            .collect();
        // Editor, Files and Startup come from plugins this test doesn't add.
        assert_eq!(sections, ["Appearance", "Window", "Panels", "Editor"]);
    }

    #[test]
    fn the_open_settings_keys_open_one_settings_panel() {
        use bedtime_workbench::{BindingInput, BindingPress, InvocationContext};
        let mut app = app();
        app.update();
        let chord = KeyChord::primary(KeyCode::Comma);
        for _ in 0..2 {
            app.world_mut()
                .resource_mut::<BindingInput>()
                .0
                .push(BindingPress {
                    chord,
                    repeat: false,
                    context: InvocationContext::Focused,
                });
            app.update();
        }
        let world = app.world_mut();
        let panels = world.query::<&SettingsPanel>().iter(world).count();
        assert_eq!(panels, 1);
    }
}
