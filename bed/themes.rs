//! bEd's themes: the ones it ships, which `ThemesPlugin` (workbench) writes
//! into the config folder's `themes` when missing, and its default.
use bedtime_workbench::appearance::ThemesPlugin;
use std::path::PathBuf;

/// The shipped themes: each id and file.
pub const BUILT_IN: &[(&str, &str)] = &[
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

/// The themes folder, with bEd's shipped themes and default.
pub fn plugin() -> ThemesPlugin {
    ThemesPlugin {
        folder: folder(),
        shipped: BUILT_IN,
        default: DEFAULT,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bedtime_ui::{Theme, theme::ThemeFolder};
    use bedtime_workbench::WorkbenchCorePlugins;
    use bevy::prelude::*;

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
        let mut app = App::new();
        app.add_plugins((
            WorkbenchCorePlugins,
            ThemesPlugin {
                folder: Some(folder.clone()),
                shipped: BUILT_IN,
                default: DEFAULT,
            },
        ));
        assert_eq!(
            std::fs::read_to_string(folder.join("tokyo.ron")).unwrap(),
            "edited"
        );
        let themes = ThemeFolder::new(&folder);
        assert_eq!(themes.themes().count(), 19, "tokyo.ron no longer parses");
        assert!(themes.get(DEFAULT).is_some());
        std::fs::remove_dir_all(folder).unwrap();
    }
}
