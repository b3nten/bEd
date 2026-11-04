//! Appearance (`ui` and `persistence` features): the font, and the window's
//! and panels' look, as settings, applied to every `LayoutSurface` with its
//! theme. `AppearancePlugin` (in `WorkbenchPlugins`) registers them; disable
//! it to style surfaces yourself.
//!
//! The theme is `ColorTheme`, a theme id looked up in bedtime-ui's
//! `ThemeFolder` (bedtime's own dark theme without one). Which themes exist
//! is the app's: `ThemesPlugin` installs its shipped themes into its themes
//! folder and sets the default. `ThemeEditorPlugin` (opt-in) adds the Theme
//! Editor panel.
use crate::settings::{
    Setting, SettingsAppExt, SettingsWorldExt, settings,
    ui::{CONTROL_WIDTH, Marks, columns, combo, row},
};
use crate::*;
use bedtime_persistence::StoreId;
use bedtime_ui::{
    BuiltinIcon, Metrics, UiContext, egui,
    fonts::DEFAULT_FAMILY,
    theme::{FONT_SIZES, ThemeFolder},
};
use bevy::prelude::*;
use std::{ops::RangeInclusive, path::PathBuf};

mod theme_editor;
pub use theme_editor::{
    OpenThemeEditor, ThemeEditorPanel, ThemeEditorPlugin, open as open_theme_editor,
};

const OPACITY: RangeInclusive<f32> = 0.0..=1.0;
const PANEL_RADIUS: RangeInclusive<f32> = 0.0..=32.0;
const PANEL_GAP: RangeInclusive<f32> = 0.0..=32.0;

settings! {
    "Appearance" {
        /// A theme's id: the name of a file in the themes folder (`tokyo`).
        pub ColorTheme: String {
            default: String::new(),
            key: "appearance.theme",
            label: "Theme",
            ui: custom(theme_row),
            apply: |world, theme| world.resource_mut::<AppearanceSettings>().theme = theme.clone(),
        }
        /// An installed font family; `None` is the bundled Bedfont.
        pub FontFamily: Option<String> {
            default: None,
            key: "appearance.font_family",
            label: "Font",
            ui: custom(font_family_row),
            apply: |world, family| world.resource_mut::<AppearanceSettings>().font_family = family.clone(),
        }
        pub FontSize: f32 {
            default: 15.5,
            key: "appearance.font_size",
            ui: slider(FONT_SIZES).suffix(" pt").decimals(1),
            apply: |world, size| world.resource_mut::<AppearanceSettings>().font_size = crate::settings::limit(*size, FONT_SIZES, 15.5),
        }
    }
    "Window" {
        pub WindowOpacity: f32 {
            default: 1.0,
            key: "appearance.window_opacity",
            label: "Opacity",
            ui: slider(OPACITY),
            apply: |world, opacity| world.resource_mut::<AppearanceSettings>().window_opacity = crate::settings::limit(*opacity, OPACITY, 1.0),
        }
        pub WindowBlurEnabled: bool {
            default: false,
            key: "appearance.window_blur",
            label: "Blur",
            // Only macOS blurs behind windows.
            visible: |_, _| cfg!(target_os = "macos"),
            apply: |world, blur| world.resource_mut::<AppearanceSettings>().window_blur = *blur,
        }
    }
    "Panels" {
        pub PanelOpacity: f32 {
            default: 1.0,
            key: "appearance.panel_opacity",
            label: "Opacity",
            ui: slider(OPACITY),
            apply: |world, opacity| world.resource_mut::<AppearanceSettings>().panel_opacity = crate::settings::limit(*opacity, OPACITY, 1.0),
        }
        pub PanelRadius: f32 {
            default: 7.0,
            key: "appearance.panel_radius",
            label: "Corner radius",
            ui: slider(PANEL_RADIUS).decimals(1),
            apply: |world, radius| world.resource_mut::<AppearanceSettings>().panel_radius = crate::settings::limit(*radius, PANEL_RADIUS, 7.0),
        }
        pub PanelGap: f32 {
            default: 5.0,
            key: "appearance.panel_gap",
            label: "Gap",
            ui: slider(PANEL_GAP).decimals(1),
            apply: |world, gap| world.resource_mut::<AppearanceSettings>().panel_gap = crate::settings::limit(*gap, PANEL_GAP, 5.0),
        }
    }
}

/// How the window and workbench look, fed by the appearance settings.
#[derive(Resource, Clone, PartialEq)]
pub struct AppearanceSettings {
    /// A theme id in the `ThemeFolder`; the app's to set.
    pub theme: String,
    pub font_size: f32,
    pub font_family: Option<String>,
    pub window_opacity: f32,
    pub window_blur: bool,
    pub panel_opacity: f32,
    pub panel_radius: f32,
    pub panel_gap: f32,
}

impl Default for AppearanceSettings {
    fn default() -> Self {
        Self {
            theme: String::new(),
            font_size: FontSize::default(),
            font_family: FontFamily::default(),
            window_opacity: WindowOpacity::default(),
            window_blur: WindowBlurEnabled::default(),
            panel_opacity: PanelOpacity::default(),
            panel_radius: PanelRadius::default(),
            panel_gap: PanelGap::default(),
        }
    }
}

/// The appearance settings, applied to every layout surface.
pub struct AppearancePlugin;

impl Plugin for AppearancePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AppearanceSettings>()
            .register_setting::<ColorTheme>()
            .register_setting::<FontFamily>()
            .register_setting::<FontSize>()
            .register_setting::<WindowOpacity>()
            .register_setting::<WindowBlurEnabled>()
            .register_setting::<PanelOpacity>()
            .register_setting::<PanelRadius>()
            .register_setting::<PanelGap>()
            .add_systems(Update, apply_appearance);
    }
}

/// An app's themes: `folder` (none: only bedtime's own dark theme) gets the
/// `shipped` themes it's missing (edited ones are left alone), and `default`
/// is the theme until one's chosen.
pub struct ThemesPlugin {
    pub folder: Option<PathBuf>,
    /// Each shipped theme's id and file (RON).
    pub shipped: &'static [(&'static str, &'static str)],
    pub default: &'static str,
}

impl Plugin for ThemesPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(ShippedThemes(self.shipped))
            .set_setting_default::<ColorTheme>(self.default.into());
        let Some(folder) = self.folder.clone() else {
            return;
        };
        let result = install(&folder, self.shipped);
        app.world_mut().workbench_mut().notify_failure(
            &format!("Couldn't write themes to {}", folder.display()),
            result,
        );
        app.insert_resource(ThemeFolder::new(folder));
    }
}

/// The app's shipped themes: what the Theme Editor can reset one to.
#[derive(Resource, Clone, Copy)]
pub struct ShippedThemes(pub &'static [(&'static str, &'static str)]);

impl ShippedThemes {
    /// A shipped theme's file, if `id` is one.
    pub fn built_in(&self, id: &str) -> Option<&'static str> {
        self.0
            .iter()
            .find(|(shipped, _)| *shipped == id)
            .map(|(_, text)| *text)
    }
}

/// Write the shipped themes whose files are missing.
fn install(folder: &std::path::Path, shipped: &[(&str, &str)]) -> std::io::Result<()> {
    std::fs::create_dir_all(folder)?;
    for (id, text) in shipped {
        let path = folder.join(format!("{id}.ron"));
        if !path.exists() {
            std::fs::write(path, text)?;
        }
    }
    Ok(())
}

/// The folder's themes, by name.
fn theme_choices(folder: Option<&ThemeFolder>) -> Vec<(String, String)> {
    let mut choices: Vec<_> = folder
        .into_iter()
        .flat_map(ThemeFolder::themes)
        .map(|(id, theme)| (id.to_owned(), theme.name.clone()))
        .collect();
    choices.sort_by(|a, b| a.1.cmp(&b.1));
    choices
}

/// The Theme row of the settings panel: the themes, a button to edit one in
/// the Theme Editor (when it's installed), and one to open the folder.
fn theme_row(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, layer: StoreId) {
    let settings = world.settings();
    let chosen = settings.get_at::<ColorTheme>(layer);
    let set_here = settings.get_in::<ColorTheme>(layer).is_some();
    let marks = Marks::new(
        settings,
        layer,
        settings.overridden_above::<ColorTheme>(layer),
    );
    let themes = world.get_resource::<ThemeFolder>();
    let choices = theme_choices(themes);
    let found = themes.and_then(|themes| themes.get(&chosen));
    let shown = found.map_or_else(|| chosen.clone(), |theme| theme.name.clone());
    let broken: Vec<String> = themes
        .into_iter()
        .flat_map(ThemeFolder::errors)
        .map(|(path, error)| format!("{}: {error}", path.display()))
        .collect();
    let folder = themes.map(|themes| themes.path().to_path_buf());
    let editor = world.contains_resource::<theme_editor::Installed>();
    let found = found.is_some();
    let mut value = chosen.clone();
    let mut reset = false;
    let mut open_folder = false;
    let mut edit = false;
    columns(
        ui,
        "Theme",
        None,
        |ui| marks.overridden(ui, context),
        |ui| {
            egui::ComboBox::from_id_salt("theme")
                .width(CONTROL_WIDTH)
                .selected_text(shown)
                .show_ui(ui, |ui| {
                    for (id, name) in &choices {
                        ui.selectable_value(&mut value, id.clone(), name);
                    }
                });
        },
        |ui| {
            reset = set_here && marks.reset(ui, context);
            if editor {
                let mut frame = context.frame(ui);
                let button = frame.icon_button_sized(BuiltinIcon::Sliders, 14.0);
                button.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Edit theme")
                });
                frame.tooltip(
                    &button,
                    "Edit this theme's colors, or make your own from it",
                );
                edit = button.clicked();
            }
            if folder.is_some() {
                let mut frame = context.frame(ui);
                let button = frame.icon_button_sized(BuiltinIcon::FolderOpen, 14.0);
                frame.tooltip(&button, "Open the themes folder to edit or add themes");
                open_folder = button.clicked();
            }
            if !found {
                ui.weak("Not found");
            }
            if !broken.is_empty() {
                let warning = bedtime_ui::theme::color(context.theme.palette.warning);
                let mut frame = context.frame(ui);
                let side = context.metrics.font_size;
                let icon = frame.icon(BuiltinIcon::CircleAlert, side, warning);
                let label = format!("{} theme file(s) have errors", broken.len());
                icon.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::Label, true, &label)
                });
                frame.tooltip(&icon, format!("{label}:\n{}", broken.join("\n")));
            }
        },
    );
    if value != chosen {
        let result = world.settings_mut().set::<ColorTheme>(layer, value);
        world
            .workbench_mut()
            .notify_failure("Couldn't change the theme", result);
    }
    if reset {
        let result = world.settings_mut().clear::<ColorTheme>(layer);
        world
            .workbench_mut()
            .notify_failure("Couldn't reset the theme", result);
    }
    if edit {
        let result = theme_editor::open(world, Some(chosen.clone()));
        world
            .workbench_mut()
            .notify_failure("Couldn't open the Theme Editor", result);
    }
    if open_folder && let Some(folder) = folder {
        let opener = if cfg!(target_os = "macos") {
            "open"
        } else if cfg!(windows) {
            "explorer"
        } else {
            "xdg-open"
        };
        let result = std::fs::create_dir_all(&folder)
            .and_then(|()| std::process::Command::new(opener).arg(&folder).spawn());
        world
            .workbench_mut()
            .notify_failure(&format!("Couldn't open {}", folder.display()), result);
    }
}

/// Puts the chosen theme and sizes on each surface: when the appearance or
/// the themes folder changes (a theme file was edited), and on a new surface.
fn apply_appearance(
    appearance: Res<AppearanceSettings>,
    folder: Option<Res<ThemeFolder>>,
    mut surfaces: Query<(Entity, &mut LayoutSurface)>,
    mut commands: Commands,
) {
    let folder_changed = folder.as_ref().is_some_and(|folder| folder.is_changed());
    for (surface, mut style) in &mut surfaces {
        if !appearance.is_changed() && !folder_changed && !style.is_added() {
            continue;
        }
        let mut theme = folder
            .as_ref()
            .and_then(|folder| folder.get(&appearance.theme))
            .cloned()
            .unwrap_or_else(bedtime_ui::Theme::dark);
        theme.palette.background[3] = appearance.window_opacity;
        theme.palette.panel_background[3] = appearance.panel_opacity;
        let metrics = Metrics {
            font_size: appearance.font_size,
            font_family: appearance.font_family.clone(),
            ..default()
        };
        commands
            .entity(surface)
            .insert((theme, metrics, WindowBlur(appearance.window_blur)));
        style.rounding = appearance.panel_radius;
        style.panel_gap = appearance.panel_gap;
    }
}

/// The monospaced font families to choose from (the editor and terminal need
/// fixed-width text), and a note on them: still loading, an error, the chosen
/// one isn't installed.
#[cfg(feature = "system-fonts")]
fn font_choices(world: &World, chosen: &Option<String>) -> (Vec<String>, Option<String>) {
    use bedtime_ui::fonts::InstalledFonts;
    let installed = world.get_resource::<InstalledFonts>();
    let families: Vec<String> = installed
        .map(|fonts| fonts.families())
        .unwrap_or_default()
        .iter()
        .filter(|family| family.monospaced)
        .map(|family| family.name.clone())
        .collect();
    let note = match (installed, chosen) {
        (Some(fonts), _) if fonts.loading() => Some("Loading installed fonts…".to_owned()),
        (Some(fonts), _) if fonts.error().is_some() => fonts.error().map(str::to_owned),
        (_, Some(family)) if !families.contains(family) => {
            Some(format!("Not installed; using {DEFAULT_FAMILY}"))
        }
        _ => None,
    };
    (families, note)
}

/// Without installed fonts (no `system-fonts` feature): just the one chosen.
#[cfg(not(feature = "system-fonts"))]
fn font_choices(_: &World, chosen: &Option<String>) -> (Vec<String>, Option<String>) {
    (chosen.iter().cloned().collect(), None)
}

/// Bedfont and the installed monospaced families.
fn font_family_row(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, layer: StoreId) {
    let chosen = world.settings().get_at::<FontFamily>(layer);
    let (families, note) = font_choices(world, &chosen);
    row::<FontFamily>(ui, world, context, layer, note, |ui, family| {
        let before = family.clone();
        let shown = family.clone().unwrap_or(DEFAULT_FAMILY.into());
        combo(ui, "font", shown, |ui| {
            ui.selectable_value(family, None, DEFAULT_FAMILY);
            for name in families {
                let label = name.clone();
                ui.selectable_value(family, Some(name), label);
            }
        });
        before != *family
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::SettingsAppExt;
    use bedtime_persistence::Store;

    struct Global;
    impl Store for Global {}
    struct Workspace;
    impl Store for Workspace {}

    #[test]
    fn appearance_follows_the_settings_and_stays_in_range() {
        let mut app = App::new();
        app.add_plugins((WorkbenchCorePlugins, AppearancePlugin))
            .add_settings_layer::<Global>()
            .add_settings_layer::<Workspace>();
        app.update();
        let global = StoreId::of::<Global>();
        let world = app.world_mut();
        world.settings_mut().set::<FontSize>(global, 20.0).unwrap();
        assert_eq!(world.resource::<AppearanceSettings>().font_size, 20.0);
        // As if hand-edited: the value is kept, but what's applied is in range.
        world.settings_mut().set::<FontSize>(global, 500.0).unwrap();
        assert_eq!(world.resource::<AppearanceSettings>().font_size, 72.0);
        world.settings_mut().clear::<FontSize>(global).unwrap();
        assert_eq!(world.resource::<AppearanceSettings>().font_size, 15.5);
        // A workspace can choose its own font.
        let workspace = StoreId::of::<Workspace>();
        let settings = world.settings_mut();
        settings
            .set::<FontFamily>(global, Some("Menlo".into()))
            .unwrap();
        settings
            .set::<FontFamily>(workspace, Some("PT Mono".into()))
            .unwrap();
        let family = |world: &World| world.resource::<AppearanceSettings>().font_family.clone();
        assert_eq!(family(world).as_deref(), Some("PT Mono"));
        world.settings_mut().clear::<FontFamily>(workspace).unwrap();
        assert_eq!(family(world).as_deref(), Some("Menlo"));
    }

    #[test]
    fn an_app_sets_its_own_defaults_for_a_plugins_settings() {
        let mut app = App::new();
        app.add_plugins((WorkbenchCorePlugins, AppearancePlugin))
            .add_settings_layer::<Global>()
            .set_setting_default::<FontSize>(13.0);
        app.update();
        let world = app.world_mut();
        assert_eq!(world.resource::<AppearanceSettings>().font_size, 13.0);
        let global = StoreId::of::<Global>();
        world.settings_mut().set::<FontSize>(global, 18.0).unwrap();
        world.settings_mut().clear::<FontSize>(global).unwrap();
        assert_eq!(world.resource::<AppearanceSettings>().font_size, 13.0);
    }
}
