//! Semantic presentation data. Identity, selection and persistence belong to the host.
use bevy::prelude::*;
use dear_imgui_rs::{Context, StyleColor as C};

pub type Rgba = [f32; 4];

#[derive(Clone, Debug, PartialEq)]
pub struct Palette {
    pub background: Rgba,
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
}

#[derive(Clone, Debug, PartialEq)]
pub struct Metrics {
    pub font_size: f32,
    pub padding: [f32; 2],
    pub spacing: [f32; 2],
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
            font_size: 17.0,
            padding: [10.0, 6.0],
            spacing: [10.0, 8.0],
            rounding: 5.0,
            tooltip_padding: [12.0, 10.0],
            menu_padding: [8.0, 6.0],
            menu_spacing: [6.0, 4.0],
            popup_rounding: 7.0,
            popup_gap: 4.0,
        }
    }
}

#[derive(Component, Clone, Debug, PartialEq)]
pub struct Theme {
    pub palette: Palette,
    pub metrics: Metrics,
}

impl Theme {
    pub fn validate(&self) -> Result<(), String> {
        let p = &self.palette;
        if [
            p.background,
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
        .flatten()
        .any(|v| !v.is_finite() || !(0.0..=1.0).contains(&v))
        {
            return Err("Theme colors must be finite values in 0..=1".into());
        }
        let m = &self.metrics;
        if !m.font_size.is_finite()
            || !(8.0..=72.0).contains(&m.font_size)
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

    pub(crate) fn apply(&self, context: &mut Context) {
        let p = &self.palette;
        let style = context.style_mut();
        // Install the complete shared style once, before NewFrame. Controls inherit it.
        for (slot, color) in [
            (C::Text, p.text),
            (C::InputTextCursor, p.text),
            (C::TextDisabled, p.muted),
            (C::WindowBg, p.background),
            (C::ChildBg, [0.0; 4]),
            (C::PopupBg, p.elevated),
            (C::Border, p.border),
            (C::BorderShadow, [0.0; 4]),
            (C::FrameBg, p.surface),
            (C::FrameBgHovered, mix(p.surface, p.accent, 0.15)),
            (C::FrameBgActive, mix(p.surface, p.accent, 0.25)),
            (C::Button, p.surface),
            (C::ButtonHovered, mix(p.surface, p.accent, 0.2)),
            (C::ButtonActive, mix(p.surface, p.accent, 0.35)),
            (C::Header, p.selection),
            (C::HeaderHovered, mix(p.surface, p.accent, 0.2)),
            (C::HeaderActive, mix(p.surface, p.accent, 0.35)),
            (C::CheckMark, p.accent),
            (C::CheckboxSelectedBg, p.selection),
            (C::SliderGrab, p.accent),
            (C::SliderGrabActive, p.text),
            (C::TitleBg, p.surface),
            (C::TitleBgActive, p.elevated),
            (C::TitleBgCollapsed, p.surface),
            (C::MenuBarBg, p.surface),
            (C::Separator, p.border),
            (C::SeparatorHovered, p.accent),
            (C::SeparatorActive, p.accent),
            (C::ScrollbarBg, p.background),
            (C::ScrollbarGrab, p.border),
            (C::ScrollbarGrabHovered, p.muted),
            (C::ScrollbarGrabActive, p.accent),
            (C::ResizeGrip, p.border),
            (C::ResizeGripHovered, p.accent),
            (C::ResizeGripActive, p.accent),
            (C::Tab, p.surface),
            (C::TabHovered, p.selection),
            (C::TabSelected, p.elevated),
            (C::TabDimmed, p.surface),
            (C::TabDimmedSelected, p.elevated),
            (C::TableHeaderBg, p.elevated),
            (C::TableBorderStrong, p.border),
            (C::TableBorderLight, p.border),
            (C::TableRowBg, [0.0; 4]),
            (C::TableRowBgAlt, mix(p.background, p.surface, 0.5)),
            (C::TextSelectedBg, p.selection),
            (C::TextLink, p.accent),
            (C::NavCursor, p.accent),
            (C::DragDropTarget, p.accent),
            (C::ModalWindowDimBg, [0.0, 0.0, 0.0, 0.4]),
        ] {
            style.set_color(slot, color);
        }
        let m = &self.metrics;
        style.set_window_padding([18.0, 18.0]);
        style.set_frame_padding(m.padding);
        style.set_item_spacing(m.spacing);
        style.set_frame_rounding(m.rounding);
        style.set_window_rounding(m.rounding);
        style.set_popup_rounding(m.popup_rounding);
        style.set_grab_rounding(m.rounding);
        style.set_scrollbar_rounding(m.rounding);
        style.set_frame_border_size(1.0);
        style.set_popup_border_size(1.0);
        style.set_disabled_alpha(0.5);
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
                background: rgb(0x2d353b),
                surface: rgb(0x343f44),
                elevated: rgb(0x3d484d),
                border: rgb(0x56635f),
                text: rgb(0xd3c6aa),
                muted: rgb(0x9da9a0),
                accent: rgb(0xa7c080),
                selection: rgb(0x425047),
                success: rgb(0xa7c080),
                warning: rgb(0xdbbc7f),
                error: rgb(0xe67e80),
            },
            metrics: Metrics::default(),
        }
    }

    pub fn light() -> Theme {
        Theme {
            palette: Palette {
                background: rgb(0xfdf6e3),
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
            },
            metrics: Metrics::default(),
        }
    }
}
