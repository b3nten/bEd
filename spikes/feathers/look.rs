//! The spike's settings, and bed's themes and font fed to feathers.
//!
//! Feathers widgets read colors from `UiTheme` tokens, so a bed `Theme`
//! becomes one: its palette fills feathers' semantic slots. Feathers loads
//! Fira Sans from fixed embedded paths; registering Bedfont's bytes under
//! those paths makes every widget use it without touching feathers.
use bedtime_ui::Theme;
use bevy::asset::io::embedded::EmbeddedAssetRegistry;
use bevy::feathers::dark_theme::create_dark_theme;
use bevy::feathers::theme::{InheritableThemeTextColor, UiTheme};
use bevy::feathers::tokens::semantic;
use bevy::prelude::*;
use std::path::{Path, PathBuf};

pub const THEMES: &[(&str, &str)] = &[
    (
        "everforest-dark-medium",
        include_str!("../../bed/themes/everforest-dark-medium.ron"),
    ),
    (
        "everforest-light-medium",
        include_str!("../../bed/themes/everforest-light-medium.ron"),
    ),
    (
        "catppuccin-mocha",
        include_str!("../../bed/themes/catppuccin-mocha.ron"),
    ),
    ("tokyo", include_str!("../../bed/themes/tokyo.ron")),
    ("rose-pine", include_str!("../../bed/themes/rose-pine.ron")),
    (
        "solarized-light",
        include_str!("../../bed/themes/solarized-light.ron"),
    ),
    (
        "synthwave-84",
        include_str!("../../bed/themes/synthwave-84.ron"),
    ),
];

const BEDFONT: &[u8] =
    include_bytes!("../../bedtime/crates/bedtime-ui/assets/fonts/NordwandMono-Regular.ttf");
const BEDFONT_BOLD: &[u8] =
    include_bytes!("../../bedtime/crates/bedtime-ui/assets/fonts/NordwandMono-Bold.ttf");
/// Any of feathers' text fonts; after `plugin` it holds Bedfont.
pub const FONT: &str = bevy::feathers::constants::fonts::REGULAR;

/// What the Settings panel edits. The shell and panels read it every frame.
#[derive(Resource, Clone, PartialEq)]
pub struct Look {
    pub theme_index: usize,
    pub theme: Theme,
    pub font_size: f32,
    pub opacity: f32,
    pub blur: bool,
    pub rounding: f32,
    pub gap: f32,
    pub tab_bar: bool,
}

impl Default for Look {
    fn default() -> Self {
        Self {
            theme_index: 0,
            theme: theme(0),
            font_size: 15.5,
            opacity: 1.0,
            blur: false,
            rounding: 7.0,
            gap: 5.0,
            tab_bar: true,
        }
    }
}

impl Look {
    fn from_env() -> Self {
        let mut look = Self::default();
        let var = |name| std::env::var(name).ok();
        if let Some(index) =
            var("SPIKE_THEME").and_then(|t| THEMES.iter().position(|(n, _)| *n == t))
        {
            look.theme_index = index;
            look.theme = theme(index);
        }
        if let Some(gap) = var("SPIKE_GAP").and_then(|v| v.parse().ok()) {
            look.gap = gap;
        }
        if let Some(rounding) = var("SPIKE_ROUNDING").and_then(|v| v.parse().ok()) {
            look.rounding = rounding;
        }
        look
    }

    pub fn color(&self, rgba: bedtime_ui::theme::Rgba) -> Color {
        Color::srgba(rgba[0], rgba[1], rgba[2], rgba[3])
    }

    pub fn mix(&self, a: bedtime_ui::theme::Rgba, b: bedtime_ui::theme::Rgba, t: f32) -> Color {
        self.color(bedtime_ui::theme::mix(a, b, t))
    }
}

pub fn theme(index: usize) -> Theme {
    Theme::from_ron(THEMES[index].1).expect("bundled theme")
}

pub fn plugin(app: &mut App) {
    let registry = app.world_mut().resource_mut::<EmbeddedAssetRegistry>();
    for (path, bytes) in [
        ("bevy_feathers/assets/fonts/FiraSans-Regular.ttf", BEDFONT),
        ("bevy_feathers/assets/fonts/FiraSans-Italic.ttf", BEDFONT),
        ("bevy_feathers/assets/fonts/FiraSans-Bold.ttf", BEDFONT_BOLD),
        (
            "bevy_feathers/assets/fonts/FiraSans-BoldItalic.ttf",
            BEDFONT_BOLD,
        ),
        ("bevy_feathers/assets/fonts/FiraMono-Medium.ttf", BEDFONT),
    ] {
        registry.insert_asset(PathBuf::new(), Path::new(path), bytes);
    }
    app.insert_resource(Look::from_env())
        .add_systems(PreStartup, apply_theme)
        .add_systems(
            PreUpdate,
            (apply_theme, refresh_inherited_text)
                .chain()
                .run_if(resource_changed::<Look>),
        )
        .add_systems(Update, switch_theme_for_screenshot);
}

fn apply_theme(look: Res<Look>, mut ui_theme: ResMut<UiTheme>, mut clear: ResMut<ClearColor>) {
    let p = &look.theme.palette;
    let c = |rgba| look.color(rgba);
    let lift = |rgba, t| look.mix(rgba, p.text, t);
    clear.0 = c(p.background);
    let mut props = create_dark_theme();
    // bed has one surface level per area, so feathers' contextual overrides
    // (tuned for its own grays) would only fight the palette.
    props.semantic_overrides.clear();
    props.semantic_base.extend([
        (semantic::SURFACE_WINDOW, c(p.background)),
        (semantic::SURFACE_PANE_BODY, c(p.panel_background)),
        (semantic::SURFACE_PANE_HEADER, c(p.panel_background)),
        (semantic::SURFACE_SUBPANE_BODY, c(p.panel_background)),
        (semantic::SURFACE_SUBPANE_HEADER, c(p.surface)),
        (semantic::SURFACE_GROUP, c(p.surface)),
        (semantic::SURFACE_DIALOG, c(p.elevated)),
        (semantic::SURFACE_MENU, c(p.elevated)),
        (semantic::BORDER_DEFAULT, c(p.border)),
        (semantic::TEXT_DEFAULT, c(p.text)),
        (semantic::TEXT_DIM, c(p.muted)),
        (semantic::TEXT_DISABLED, c(p.muted).with_alpha(0.5)),
        // bed's controls keep ordinary text on their fills.
        (semantic::TEXT_ON_ACCENT, c(p.text)),
        (
            semantic::FILL_ACCENT_DEFAULT,
            look.mix(p.surface, p.accent, 0.55),
        ),
        (
            semantic::FILL_ACCENT_HOVER,
            look.mix(p.surface, p.accent, 0.65),
        ),
        (
            semantic::FILL_ACCENT_PRESSED,
            look.mix(p.surface, p.accent, 0.75),
        ),
        (semantic::FILL_ACCENT_DISABLED, c(p.surface).with_alpha(0.5)),
        (semantic::FILL_SOLID_DEFAULT, c(p.surface)),
        (semantic::FILL_SOLID_HOVER, lift(p.surface, 0.08)),
        (semantic::FILL_SOLID_PRESSED, lift(p.surface, 0.14)),
        (semantic::FILL_SOLID_DISABLED, c(p.surface).with_alpha(0.5)),
        (semantic::FILL_ITEM_HOVER, lift(p.elevated, 0.08)),
        (semantic::FILL_ITEM_PRESSED, c(p.selection)),
        (semantic::FILL_FIELD_DEFAULT, c(p.surface)),
        (semantic::FILL_FIELD_HOVER, lift(p.surface, 0.05)),
        (semantic::FILL_FIELD_PRESSED, lift(p.surface, 0.1)),
        (semantic::FOCUS_RING, c(p.accent).with_alpha(0.5)),
    ]);
    ui_theme.0 = props;
}

/// `SPIKE_SWITCH_THEME=<name>` switches theme mid-run, to check that live
/// theme changes recolor everything.
fn switch_theme_for_screenshot(mut look: ResMut<Look>, mut frames: Local<u32>) {
    *frames += 1;
    if *frames != 20 {
        return;
    }
    let Some(index) = std::env::var("SPIKE_SWITCH_THEME")
        .ok()
        .and_then(|t| THEMES.iter().position(|(n, _)| *n == t))
    else {
        return;
    };
    look.theme_index = index;
    look.theme = theme(index);
}

/// Feathers resolves inherited text colors only when the component is
/// inserted, not when `UiTheme` changes; re-inserting resolves them again.
fn refresh_inherited_text(
    query: Query<(Entity, &InheritableThemeTextColor)>,
    mut commands: Commands,
) {
    for (entity, color) in &query {
        commands.entity(entity).insert(color.clone());
    }
}
