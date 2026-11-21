//! bEd's settings: its own defaults for plugins' settings, its themes, and
//! how the Settings panel lays them out. The settings
//! themselves belong to the crates whose behaviour they change (appearance
//! and autosave to workbench, the editor's to bedtime-code, the file tree's
//! to bedtime-file-explorer, the screen effects to `effects`).
use crate::visuals::effects::*;
use bedtime_code::{LineNumbers, WrapLines};
use bedtime_file_explorer::{ShowHiddenFiles, ShowIgnoredFiles};
use bedtime_workbench::{Autosave, workspaces::ReopenLastWorkspace};
use bedtime_workbench::{
    appearance::{
        ColorTheme, FontFamily, FontSize, PanelGap, PanelOpacity, PanelRadius, RainbowInk,
        ThemeEditorPlugin, WindowBlurEnabled, WindowOpacity,
    },
    settings::{SettingsAppExt, SettingsPanelPlugin, ui::SettingsUiAppExt},
    settings_panel,
};
use bevy::prelude::*;

pub struct BedSettingsPlugin;

impl Plugin for BedSettingsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((SettingsPanelPlugin, ThemeEditorPlugin))
            // For the Font setting's list.
            .init_resource::<bedtime_ui::fonts::InstalledFonts>()
            .set_setting_default::<bedtime_workbench::Autosave>(true)
            .set_settings_layout(settings_panel! {
                "Appearance" {
                    ColorTheme, FontFamily, FontSize, RainbowInk,
                    EffectsEnabled, Scanlines, Vignette, Bloom,
                    Noise, Curvature, PixelGrid, BurnIn, Pulse,
                }
                "Window" { crate::visuals::backdrop::Backdrop, WindowOpacity, WindowBlurEnabled, crate::perf::ShowFrameRate }
                "Panels" { PanelOpacity, PanelRadius, PanelGap }
                "Editor" { WrapLines, Autosave, LineNumbers }
                "Files" { ShowHiddenFiles, ShowIgnoredFiles }
                "Web" { bedtime_web::DrawWebPages }
                "Startup" { ReopenLastWorkspace }
            });
    }
}
