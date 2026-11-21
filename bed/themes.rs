//! bEd's themes: the ones it ships, which `ThemesPlugin` (workbench) writes
//! into the config folder's `themes` when missing, and its default.
use bedtime_workbench::appearance::ThemesPlugin;

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

/// The themes folder, with bEd's shipped themes and default.
pub fn plugin() -> ThemesPlugin {
    ThemesPlugin {
        // Under the persistence directory.
        folder: Some("themes".into()),
        shipped: BUILT_IN,
        default: DEFAULT,
    }
}
