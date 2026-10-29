//! bEd's color themes. bEd ships twenty (`bed/themes/`); each launch writes
//! any whose file is missing into `<config>/themes`, which is where themes
//! are read from. Users edit them there, or add their own: the folder is
//! watched while bed runs. Without a config directory, bed uses bedtime-ui's
//! dark theme.
//! See `bed/themes/README.md` for the format and the themes' sources.
use bedtime_ui::{BuiltinIcon, UiContext, egui, theme::ThemeFolder};
use bedtime_workbench::{
    persistence::StoreId,
    settings::{
        SettingsWorldExt,
        ui::{CONTROL_WIDTH, Marks, columns},
    },
};
use bevy::prelude::*;
use std::path::{Path, PathBuf};

const BUILT_IN: &[(&str, &str)] = &[
    ("carbon", include_str!("themes/carbon.ron")),
    (
        "catppuccin-frappe",
        include_str!("themes/catppuccin-frappe.ron"),
    ),
    (
        "catppuccin-latte",
        include_str!("themes/catppuccin-latte.ron"),
    ),
    (
        "catppuccin-macchiato",
        include_str!("themes/catppuccin-macchiato.ron"),
    ),
    (
        "catppuccin-mocha",
        include_str!("themes/catppuccin-mocha.ron"),
    ),
    (
        "everforest-dark-hard",
        include_str!("themes/everforest-dark-hard.ron"),
    ),
    (
        "everforest-dark-medium",
        include_str!("themes/everforest-dark-medium.ron"),
    ),
    (
        "everforest-dark-soft",
        include_str!("themes/everforest-dark-soft.ron"),
    ),
    (
        "everforest-light-hard",
        include_str!("themes/everforest-light-hard.ron"),
    ),
    (
        "everforest-light-medium",
        include_str!("themes/everforest-light-medium.ron"),
    ),
    (
        "everforest-light-soft",
        include_str!("themes/everforest-light-soft.ron"),
    ),
    ("oxocarbon-dark", include_str!("themes/oxocarbon-dark.ron")),
    (
        "oxocarbon-light",
        include_str!("themes/oxocarbon-light.ron"),
    ),
    ("rose-pine", include_str!("themes/rose-pine.ron")),
    ("rose-pine-dawn", include_str!("themes/rose-pine-dawn.ron")),
    ("rose-pine-moon", include_str!("themes/rose-pine-moon.ron")),
    ("slash", include_str!("themes/slash.ron")),
    (
        "solarized-light",
        include_str!("themes/solarized-light.ron"),
    ),
    ("synthwave-84", include_str!("themes/synthwave-84.ron")),
    ("tokyo", include_str!("themes/tokyo.ron")),
];

/// The default theme's id.
pub const DEFAULT: &str = "everforest-dark-medium";

/// Where themes are read from.
pub fn folder() -> Option<PathBuf> {
    crate::config_directory().map(|config| config.join("themes"))
}

/// Write the shipped themes whose files are missing. Existing files, edited
/// or not, are left alone.
fn install(folder: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(folder)?;
    for (id, text) in BUILT_IN {
        let path = folder.join(format!("{id}.ron"));
        if !path.exists() {
            std::fs::write(path, text)?;
        }
    }
    Ok(())
}

pub struct ThemesPlugin;

impl Plugin for ThemesPlugin {
    fn build(&self, app: &mut App) {
        let Some(folder) = folder() else {
            return;
        };
        let result = install(&folder);
        crate::status::notify_failure(
            app.world_mut(),
            &format!("Couldn't write themes to {}", folder.display()),
            result,
        );
        app.insert_resource(ThemeFolder::new(folder));
    }
}

/// The folder's themes, by name.
fn choices(folder: Option<&ThemeFolder>) -> Vec<(String, String)> {
    let mut choices: Vec<_> = folder
        .into_iter()
        .flat_map(ThemeFolder::themes)
        .map(|(id, theme)| (id.to_owned(), theme.name.clone()))
        .collect();
    choices.sort_by(|a, b| a.1.cmp(&b.1));
    choices
}

/// The Theme row of the Settings panel.
pub fn row(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, layer: StoreId) {
    let settings = world.settings();
    let chosen = settings.get_at::<crate::settings::ColorTheme>(layer);
    let set_here = settings
        .get_in::<crate::settings::ColorTheme>(layer)
        .is_some();
    let marks = Marks::new(
        settings,
        layer,
        settings.overridden_above::<crate::settings::ColorTheme>(layer),
    );
    let themes = world.get_resource::<ThemeFolder>();
    let choices = choices(themes);
    let found = themes.and_then(|themes| themes.get(&chosen));
    let shown = found.map_or_else(|| chosen.clone(), |theme| theme.name.clone());
    let broken: Vec<String> = themes
        .into_iter()
        .flat_map(ThemeFolder::errors)
        .map(|(path, error)| format!("{}: {error}", path.display()))
        .collect();
    let found = found.is_some();
    let mut value = chosen.clone();
    let mut reset = false;
    let mut open_folder = false;
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
            let mut frame = context.frame(ui);
            let button = frame.icon_button_sized(BuiltinIcon::FolderOpen, 14.0);
            frame.tooltip(&button, "Open the themes folder to edit or add themes");
            open_folder = button.clicked();
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
        let result = world
            .settings_mut()
            .set::<crate::settings::ColorTheme>(layer, value);
        crate::status::notify_failure(world, "Couldn't change the theme", result);
    }
    if reset {
        let result = world
            .settings_mut()
            .clear::<crate::settings::ColorTheme>(layer);
        crate::status::notify_failure(world, "Couldn't reset the theme", result);
    }
    if open_folder && let Some(folder) = folder() {
        let opener = if cfg!(target_os = "macos") {
            "open"
        } else if cfg!(windows) {
            "explorer"
        } else {
            "xdg-open"
        };
        let result = std::fs::create_dir_all(&folder)
            .and_then(|()| std::process::Command::new(opener).arg(&folder).spawn());
        crate::status::notify_failure(
            world,
            &format!("Couldn't open {}", folder.display()),
            result,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bedtime_ui::Theme;

    #[test]
    fn bed_ships_twenty_valid_themes_and_installs_only_missing_ones() {
        assert_eq!(BUILT_IN.len(), 20);
        for (id, text) in BUILT_IN {
            Theme::from_ron(text).unwrap_or_else(|error| panic!("{id}: {error}"));
        }
        let folder = std::env::temp_dir().join(format!("bed-install-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        // An edited theme is kept.
        std::fs::write(folder.join("tokyo.ron"), "edited").unwrap();
        install(&folder).unwrap();
        assert_eq!(
            std::fs::read_to_string(folder.join("tokyo.ron")).unwrap(),
            "edited"
        );
        let themes = ThemeFolder::new(&folder);
        assert_eq!(themes.themes().count(), 19, "tokyo.ron no longer parses");
        assert!(themes.get(DEFAULT).is_some());
        let names: Vec<_> = choices(Some(&themes))
            .into_iter()
            .map(|(_, name)| name)
            .collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted, "listed by name");
        std::fs::remove_dir_all(folder).unwrap();
    }
}
