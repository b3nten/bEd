use bevy::prelude::*;
use egui::{Color32, Context};

pub type Rgba = [f32; 4];

/// The font sizes a theme can use, in points.
pub const FONT_SIZES: std::ops::RangeInclusive<f32> = 8.0..=72.0;

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Palette {
    /// Window background, including titlebar, status bar, menu bar and panel gaps.
    pub background: Rgba,
    /// Panel bodies and tab bars.
    pub panel_background: Rgba,
    pub surface: Rgba,
    pub elevated: Rgba,
    pub border: Rgba,
    pub text: Rgba,
    pub muted: Rgba,
    pub accent: Rgba,
    pub selection: Rgba,
    pub success: Rgba,
    pub warning: Rgba,
    pub error: Rgba,
    /// ANSI colors: 0..=7 normal, 8..=15 bright, each ordered black, red, green,
    /// yellow, blue, magenta, cyan, white. Host default foreground and background
    /// colors are independent of these entries.
    pub terminal: [Rgba; 16],
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
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
            padding: [10.0, 6.0],
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

#[derive(Component, Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Theme {
    pub palette: Palette,
    pub metrics: Metrics,
}

impl Theme {
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
        .flatten()
        .any(|v| !v.is_finite() || !(0.0..=1.0).contains(&v))
        {
            return Err("Theme colors must be finite values in 0..=1".into());
        }
        let m = &self.metrics;
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

    pub(crate) fn apply(&self, context: &Context) {
        let p = &self.palette;
        let m = &self.metrics;
        let mut style = (*context.global_style()).clone();
        style.animation_time = 0.16;
        style.interaction.selectable_labels = false;
        style.spacing.item_spacing = egui::vec2(m.spacing[0], m.spacing[1]);
        style.spacing.button_padding = egui::vec2(m.padding[0], m.padding[1]);
        style.spacing.window_margin =
            egui::Margin::symmetric(m.padding[0] as i8, m.padding[1] as i8);
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
        for (widget, fill) in [
            (&mut style.visuals.widgets.noninteractive, p.surface),
            (&mut style.visuals.widgets.inactive, p.surface),
            (
                &mut style.visuals.widgets.hovered,
                mix(p.surface, p.accent, 0.2),
            ),
            (
                &mut style.visuals.widgets.active,
                mix(p.surface, p.accent, 0.35),
            ),
            (&mut style.visuals.widgets.open, p.elevated),
        ] {
            widget.bg_fill = color(fill);
            widget.weak_bg_fill = color(fill);
            widget.bg_stroke = egui::Stroke::new(1.0, color(p.border));
            widget.fg_stroke = egui::Stroke::new(1.0, color(p.text));
            widget.corner_radius = egui::CornerRadius::same(m.rounding as u8);
        }
        context.set_global_style(style);
    }
}

pub fn mix(a: Rgba, b: Rgba, amount: f32) -> Rgba {
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * amount)
}

fn rgb(hex: u32) -> Rgba {
    [
        ((hex >> 16) & 255) as f32 / 255.0,
        ((hex >> 8) & 255) as f32 / 255.0,
        (hex & 255) as f32 / 255.0,
        1.0,
    ]
}

/// Everforest Medium palettes. See assets/everforest-LICENSE.txt for attribution.
pub mod themes {
    use super::*;

    pub fn dark() -> Theme {
        Theme {
            palette: Palette {
                background: rgb(0x343f44),
                panel_background: rgb(0x2d353b),
                surface: rgb(0x343f44),
                elevated: rgb(0x3d484d),
                border: rgb(0x56635f),
                text: rgb(0xd6ccb9),
                muted: rgb(0x9da9a0),
                accent: rgb(0xa7c080),
                selection: rgb(0x425047),
                success: rgb(0xa7c080),
                warning: rgb(0xdbbc7f),
                error: rgb(0xe67e80),
                terminal: [
                    0x475258, 0xe67e80, 0xa7c080, 0xdbbc7f, 0x7fbbb3, 0xd699b6, 0x83c092, 0xd3c6aa,
                    0x475258, 0xe67e80, 0xa7c080, 0xdbbc7f, 0x7fbbb3, 0xd699b6, 0x83c092, 0xd3c6aa,
                ]
                .map(rgb),
            },
            metrics: Metrics::default(),
        }
    }

    pub fn light() -> Theme {
        Theme {
            palette: Palette {
                background: rgb(0xf4f0d9),
                panel_background: rgb(0xfdf6e3),
                surface: rgb(0xf4f0d9),
                elevated: rgb(0xefebd4),
                border: rgb(0xbdc3af),
                text: rgb(0x5c6a72),
                muted: rgb(0x829181),
                accent: rgb(0x8da101),
                selection: rgb(0xeaedc8),
                success: rgb(0x8da101),
                warning: rgb(0xdfa000),
                error: rgb(0xf85552),
                terminal: [
                    0x5c6a72, 0xf85552, 0x8da101, 0xdfa000, 0x3a94c5, 0xdf69ba, 0x35a77c, 0xe6e2cc,
                    0x5c6a72, 0xf85552, 0x8da101, 0xdfa000, 0x3a94c5, 0xdf69ba, 0x35a77c, 0xe6e2cc,
                ]
                .map(rgb),
            },
            metrics: Metrics::default(),
        }
    }
}

pub fn color(rgba: Rgba) -> Color32 {
    Color32::from_rgba_unmultiplied(
        (rgba[0] * 255.0).round() as u8,
        (rgba[1] * 255.0).round() as u8,
        (rgba[2] * 255.0).round() as u8,
        (rgba[3] * 255.0).round() as u8,
    )
}
