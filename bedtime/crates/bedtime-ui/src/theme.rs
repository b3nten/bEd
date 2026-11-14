use bevy::prelude::*;
use egui::{Color32, Context};

mod folder;
mod hex;
mod syntax;
mod system;
pub use folder::ThemeFolder;
pub(crate) use folder::poll_theme_folder;
pub use syntax::{SyntaxPalette, SyntaxRole};
pub use system::SystemAppearance;
pub(crate) use system::install as install_system_appearance;

pub type Rgba = [f32; 4];

/// Line height over font size for the bundled Bedfont; installed monospace
/// fonts are shorter. Controls are sized for it so every font's text fits.
const LINE_HEIGHT: f32 = 1.33;

/// The font sizes a theme can use, in points.
pub const FONT_SIZES: std::ops::RangeInclusive<f32> = 8.0..=72.0;

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Palette {
    /// Window background, including titlebar, status bar, menu bar and panel gaps.
    #[serde(with = "hex")]
    pub background: Rgba,
    /// Panel bodies and tab bars.
    #[serde(with = "hex")]
    pub panel_background: Rgba,
    #[serde(with = "hex")]
    pub surface: Rgba,
    #[serde(with = "hex")]
    pub elevated: Rgba,
    #[serde(with = "hex")]
    pub border: Rgba,
    #[serde(with = "hex")]
    pub text: Rgba,
    #[serde(with = "hex")]
    pub muted: Rgba,
    #[serde(with = "hex")]
    pub accent: Rgba,
    #[serde(with = "hex")]
    pub selection: Rgba,
    #[serde(with = "hex")]
    pub success: Rgba,
    #[serde(with = "hex")]
    pub warning: Rgba,
    #[serde(with = "hex")]
    pub error: Rgba,
    /// ANSI colors: 0..=7 normal, 8..=15 bright, each ordered black, red, green,
    /// yellow, blue, magenta, cyan, white (named so in theme files). Host default
    /// foreground and background colors are independent of these entries.
    #[serde(with = "hex::terminal")]
    pub terminal: [Rgba; 16],
    /// Code colors, for editors and viewers.
    pub syntax: Syntax,
}

/// Colors for the roles of highlighted code.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Syntax {
    #[serde(with = "hex")]
    pub text: Rgba,
    #[serde(with = "hex")]
    pub comment: Rgba,
    #[serde(with = "hex")]
    pub keyword: Rgba,
    #[serde(with = "hex")]
    pub string: Rgba,
    #[serde(with = "hex")]
    pub number: Rgba,
    #[serde(with = "hex")]
    pub function: Rgba,
    #[serde(with = "hex")]
    pub r#type: Rgba,
    #[serde(with = "hex")]
    pub variable: Rgba,
    #[serde(with = "hex")]
    pub parameter: Rgba,
    #[serde(with = "hex")]
    pub property: Rgba,
    #[serde(with = "hex")]
    pub constant: Rgba,
    #[serde(with = "hex")]
    pub operator: Rgba,
    #[serde(with = "hex")]
    pub punctuation: Rgba,
    #[serde(with = "hex")]
    pub special: Rgba,
}

impl Syntax {
    fn colors(&self) -> [Rgba; 14] {
        [
            self.text,
            self.comment,
            self.keyword,
            self.string,
            self.number,
            self.function,
            self.r#type,
            self.variable,
            self.parameter,
            self.property,
            self.constant,
            self.operator,
            self.punctuation,
            self.special,
        ]
    }
}

/// A surface's sizes: font, padding, spacing and corners.
#[derive(Component, Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Metrics {
    pub font_size: f32,
    /// An installed family (see `fonts::InstalledFonts`); `None` is Bedfont.
    #[serde(default)]
    pub font_family: Option<String>,
    pub padding: [f32; 2],
    pub spacing: [f32; 2],
    /// Corner radius for ordinary widgets, independent of panels and popups.
    pub rounding: f32,
    pub tooltip_padding: [f32; 2],
    pub menu_padding: [f32; 2],
    pub menu_spacing: [f32; 2],
    pub popup_rounding: f32,
    pub popup_gap: f32,
}

impl Default for Metrics {
    fn default() -> Self {
        Self {
            font_size: 15.5,
            font_family: None,
            padding: [8.0, 3.0],
            spacing: [10.0, 8.0],
            rounding: 4.0,
            tooltip_padding: [12.0, 10.0],
            menu_padding: [8.0, 6.0],
            menu_spacing: [6.0, 4.0],
            popup_rounding: 7.0,
            popup_gap: 4.0,
        }
    }
}

/// A surface's colors. A theme file is this, in RON.
#[derive(Component, Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Theme {
    pub name: String,
    pub appearance: Appearance,
    pub palette: Palette,
}

/// Whether a theme is light or dark, for anything that adapts (the native
/// window appearance, say).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Appearance {
    Light,
    Dark,
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}

impl Theme {
    /// bedtime-ui's dark theme (Everforest Dark Medium).
    pub fn dark() -> Self {
        Self::from_ron(include_str!("../assets/themes/everforest-dark-medium.ron"))
            .expect("bundled theme")
    }

    /// bedtime-ui's light theme (Everforest Light Medium).
    pub fn light() -> Self {
        Self::from_ron(include_str!("../assets/themes/everforest-light-medium.ron"))
            .expect("bundled theme")
    }

    /// A theme file.
    pub fn from_ron(text: &str) -> Result<Self, String> {
        let theme: Self = ron::from_str(text).map_err(|error| error.to_string())?;
        theme.validate()?;
        Ok(theme)
    }

    /// As a theme file.
    pub fn to_ron(&self) -> String {
        let config = ron::ser::PrettyConfig::new().indentor("    ".to_owned());
        ron::ser::to_string_pretty(self, config).expect("themes serialize") + "\n"
    }

    pub fn validate(&self) -> Result<(), String> {
        let p = &self.palette;
        if [
            p.background,
            p.panel_background,
            p.surface,
            p.elevated,
            p.border,
            p.text,
            p.muted,
            p.accent,
            p.selection,
            p.success,
            p.warning,
            p.error,
        ]
        .into_iter()
        .chain(p.terminal)
        .chain(p.syntax.colors())
        .flatten()
        .any(|v| !v.is_finite() || !(0.0..=1.0).contains(&v))
        {
            return Err("Theme colors must be finite values in 0..=1".into());
        }
        Ok(())
    }
}

impl Metrics {
    /// These metrics `zoom` times bigger: text and the space around it.
    /// Rounding and popups (menus, tooltips) stay as they are.
    pub fn zoomed(&self, zoom: f32) -> Self {
        let scale = |[x, y]: [f32; 2]| [x * zoom, y * zoom];
        Self {
            font_size: self.font_size * zoom,
            padding: scale(self.padding),
            spacing: scale(self.spacing),
            ..self.clone()
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        let m = self;
        if !m.font_size.is_finite()
            || !FONT_SIZES.contains(&m.font_size)
            || m.padding
                .into_iter()
                .chain(m.spacing)
                .chain(m.tooltip_padding)
                .chain(m.menu_padding)
                .chain(m.menu_spacing)
                .chain([m.rounding, m.popup_rounding, m.popup_gap])
                .any(|v| !v.is_finite() || v < 0.0)
        {
            return Err(
                "Theme dimensions must be finite and nonnegative; font size must be 8..=72".into(),
            );
        }
        Ok(())
    }
}

impl Theme {
    pub(crate) fn apply(&self, metrics: &Metrics, context: &Context) {
        context.set_global_style(self.style(metrics, &context.global_style()));
    }

    /// `base` with this theme's colours and `metrics`' sizes.
    pub fn style(&self, metrics: &Metrics, base: &egui::Style) -> egui::Style {
        let p = &self.palette;
        let m = metrics;
        let mut style = base.clone();
        style.animation_time = 0.16;
        style.interaction.selectable_labels = false;
        style.spacing.item_spacing = egui::vec2(m.spacing[0], m.spacing[1]);
        style.spacing.button_padding = egui::vec2(m.padding[0], m.padding[1]);
        // egui starts each row at `interact_size.y` and centres widgets on it.
        // Make that exactly one control (a line of text plus button padding),
        // so widgets that nest a row, like combo boxes and sliders, stay
        // centred beside labels.
        style.spacing.interact_size.y = control_height(m);
        style.spacing.window_margin =
            egui::Margin::symmetric(m.menu_padding[0] as i8, m.menu_padding[1] as i8);
        style.spacing.menu_margin =
            egui::Margin::symmetric(m.menu_padding[0] as i8, m.menu_padding[1] as i8);
        style.spacing.menu_spacing = m.menu_spacing[0];
        for (kind, font) in &mut style.text_styles {
            font.size = m.font_size
                * match kind {
                    egui::TextStyle::Heading => 1.25,
                    egui::TextStyle::Small => 0.85,
                    _ => 1.0,
                };
        }
        style.visuals.override_text_color = Some(color(p.text));
        style.visuals.panel_fill = color(p.panel_background);
        style.visuals.window_fill = color(p.elevated);
        style.visuals.extreme_bg_color = color(p.panel_background);
        style.visuals.faint_bg_color = color(p.surface);
        style.visuals.selection.bg_fill = color(p.selection);
        style.visuals.selection.stroke = egui::Stroke::new(1.0, color(p.accent));
        style.visuals.hyperlink_color = color(p.accent);
        style.visuals.window_stroke = egui::Stroke::new(1.0, color(p.border));
        style.visuals.window_corner_radius = egui::CornerRadius::same(m.popup_rounding as u8);
        // Controls are drawn in `surface`. Some themes make it nearly the
        // panel background, which would hide borderless parts (a slider's
        // rail), so lift it toward the text color until it stands out.
        let surface = distinct(p.surface, p.panel_background, p.text);
        for (widget, fill) in [
            (&mut style.visuals.widgets.noninteractive, surface),
            (&mut style.visuals.widgets.inactive, surface),
            (
                &mut style.visuals.widgets.hovered,
                mix(surface, p.accent, 0.2),
            ),
            (
                &mut style.visuals.widgets.active,
                mix(surface, p.accent, 0.35),
            ),
            (&mut style.visuals.widgets.open, p.elevated),
        ] {
            widget.bg_fill = color(fill);
            widget.weak_bg_fill = color(fill);
            widget.bg_stroke = egui::Stroke::new(1.0, color(p.border));
            widget.fg_stroke = egui::Stroke::new(1.0, color(p.text));
            widget.corner_radius = egui::CornerRadius::same(m.rounding as u8);
        }
        style
    }
}

/// `color`, or if it's hard to tell from `background`, moved toward `toward`
/// just far enough to be.
fn distinct(color: Rgba, background: Rgba, toward: Rgba) -> Rgba {
    let difference = |a: Rgba, b: Rgba| (0..3).map(|i| (a[i] - b[i]).abs()).sum::<f32>();
    const ENOUGH: f32 = 0.09;
    if difference(color, background) >= ENOUGH {
        return color;
    }
    (1..=10)
        .map(|step| mix(color, toward, step as f32 * 0.02))
        .find(|lifted| difference(*lifted, background) >= ENOUGH)
        .unwrap_or_else(|| mix(color, toward, 0.2))
}

/// The height of a standard control: buttons, combo boxes, sliders, toggles.
pub fn control_height(metrics: &Metrics) -> f32 {
    (metrics.font_size * LINE_HEIGHT).ceil() + 2.0 * metrics.padding[1]
}

pub fn mix(a: Rgba, b: Rgba, amount: f32) -> Rgba {
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * amount)
}

pub fn color(rgba: Rgba) -> Color32 {
    Color32::from_rgba_unmultiplied(
        (rgba[0] * 255.0).round() as u8,
        (rgba[1] * 255.0).round() as u8,
        (rgba[2] * 255.0).round() as u8,
        (rgba[3] * 255.0).round() as u8,
    )
}
