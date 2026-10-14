//! Runtime appearance values supplied by an application, independent of storage.
#[derive(Clone, Debug, PartialEq)]
pub struct UiFontOptions {
    pub family: String,
    pub size: f32,
}

impl Default for UiFontOptions {
    fn default() -> Self {
        Self {
            family: "Paper Mono".into(),
            size: 20.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct UiPalette {
    pub background: [f32; 4],
    pub window_background: [f32; 4],
    pub tab_bar: [f32; 4],
    pub surface: [f32; 4],
    pub text: [f32; 4],
    pub accent: [f32; 4],
    pub selection: [f32; 4],
}

impl Default for UiPalette {
    fn default() -> Self {
        let rgb = |r: u8, g: u8, b: u8| {
            [
                f32::from(r) / 255.0,
                f32::from(g) / 255.0,
                f32::from(b) / 255.0,
                1.0,
            ]
        };
        let background = rgb(0x1au8, 0x1b, 0x26);
        Self {
            background,
            window_background: background,
            tab_bar: background,
            surface: rgb(0x16, 0x16, 0x1e),
            text: rgb(0xc0, 0xca, 0xf5),
            accent: rgb(0x7a, 0xa2, 0xf7),
            selection: rgb(0x28, 0x34, 0x57),
        }
    }
}

/// Shared dimensions. Control values scale with the current font size; popup
/// dimensions are logical pixels. Compact controls are for dense lists/toolbars.
#[derive(Clone, Debug, PartialEq)]
pub struct UiMetrics {
    pub control_padding: [f32; 2],
    pub control_spacing: [f32; 2],
    pub compact_padding: [f32; 2],
    pub compact_spacing: [f32; 2],
    pub control_rounding: f32,
    pub border: f32,
    pub grab_rounding: f32,
    pub grab_min_size: f32,
    pub window_rounding: f32,
    pub scrollbar_rounding: f32,
    pub scrollbar_size: f32,
    pub dialog_padding: [f32; 2],
    pub popup_rounding: f32,
    pub dialog_rounding: f32,
    pub tooltip_padding: [f32; 2],
    pub tooltip_wrap_em: f32,
    pub menu_padding: [f32; 2],
    pub menu_spacing: [f32; 2],
    pub disabled_alpha: f32,
}
impl Default for UiMetrics {
    fn default() -> Self {
        Self {
            control_padding: [0.55, 0.30],
            control_spacing: [0.6, 0.4],
            compact_padding: [0.45, 0.20],
            compact_spacing: [0.5, 0.3],
            control_rounding: 0.25,
            border: 1.0,
            grab_rounding: 0.2,
            grab_min_size: 0.5,
            window_rounding: 0.4,
            scrollbar_rounding: 0.3,
            scrollbar_size: 0.55,
            dialog_padding: [0.85, 0.7],
            popup_rounding: 7.0,
            dialog_rounding: 8.0,
            tooltip_padding: [12.0, 10.0],
            tooltip_wrap_em: 36.0,
            menu_padding: [8.0, 6.0],
            menu_spacing: [6.0, 4.0],
            disabled_alpha: 0.75,
        }
    }
}

/// Motion is a presentation policy shared by controls and panel content.
#[derive(Clone, Debug, PartialEq)]
pub struct UiMotion {
    pub enabled: bool,
    pub panel_seconds: f64,
    pub popup_seconds: f64,
    pub transition_seconds: f64,
    pub tree_seconds: f64,
    pub tree_stagger_seconds: f64,
    pub popup_initial_scale: f32,
}
impl Default for UiMotion {
    fn default() -> Self {
        Self {
            enabled: true,
            panel_seconds: 0.300,
            popup_seconds: 0.150,
            transition_seconds: 0.120,
            tree_seconds: 0.168,
            tree_stagger_seconds: 0.012,
            popup_initial_scale: 0.96,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct UiTheme {
    pub fonts: UiFontOptions,
    pub metrics: UiMetrics,
    pub motion: UiMotion,
    pub palette: UiPalette,
    pub opacity: f32,
}

impl Default for UiTheme {
    fn default() -> Self {
        Self {
            fonts: UiFontOptions::default(),
            metrics: UiMetrics::default(),
            motion: UiMotion::default(),
            palette: UiPalette::default(),
            opacity: 1.0,
        }
    }
}

/// Apply runtime styling before beginning a frame. Embedded users preserve host
/// background and sizing; their main font is selected explicitly while drawing.
#[cfg(feature = "imgui")]
pub fn apply_style(context: &mut dear_imgui_rs::Context, appearance: &UiTheme, embedded: bool) {
    use crate::color::{blend, ensure_contrast};
    use dear_imgui_rs::StyleColor;
    let style = context.style_mut();
    let palette = &appearance.palette;
    let text = palette.text;
    style.set_color(StyleColor::Text, text);
    style.set_color(
        StyleColor::TextDisabled,
        ensure_contrast(
            blend(text, palette.background, 0.60),
            palette.background,
            3.0,
        ),
    );
    style.set_color(StyleColor::TextSelectedBg, palette.selection);
    style.set_color(StyleColor::ScrollbarBg, [0.0; 4]);
    if embedded {
        return;
    }
    let bg = palette.background;
    let panel = [bg[0], bg[1], bg[2], appearance.opacity.clamp(0.0, 1.0)];
    let tab = palette.tab_bar;
    let chrome = [tab[0], tab[1], tab[2], panel[3]];
    style.set_color(StyleColor::WindowBg, panel);
    style.set_color(StyleColor::ChildBg, [0.0; 4]);
    for slot in [
        StyleColor::TitleBg,
        StyleColor::TitleBgActive,
        StyleColor::TitleBgCollapsed,
        StyleColor::MenuBarBg,
    ] {
        style.set_color(slot, chrome);
    }
    style.set_color(StyleColor::DockingEmptyBg, panel);
    for slot in [
        StyleColor::Tab,
        StyleColor::TabSelected,
        StyleColor::TabDimmed,
        StyleColor::TabDimmedSelected,
        StyleColor::TabSelectedOverline,
        StyleColor::TabDimmedSelectedOverline,
        StyleColor::ScrollbarGrab,
        StyleColor::ScrollbarGrabHovered,
        StyleColor::ScrollbarGrabActive,
    ] {
        style.set_color(slot, [0.0; 4]);
    }
    style.set_color(StyleColor::TabHovered, [text[0], text[1], text[2], 0.12]);
    for (slot, color) in crate::components::control_colors(palette.surface, text, palette.accent) {
        style.set_color(slot, color);
    }
    style.set_color(StyleColor::TextSelectedBg, palette.selection);
    let fs = appearance.fonts.size;
    style.set_frame_rounding(fs * appearance.metrics.control_rounding);
    style.set_frame_border_size(appearance.metrics.border);
    style.set_frame_padding(appearance.metrics.control_padding.map(|value| value * fs));
    style.set_item_spacing(appearance.metrics.control_spacing.map(|value| value * fs));
    style.set_grab_rounding(fs * appearance.metrics.grab_rounding);
    style.set_grab_min_size(fs * appearance.metrics.grab_min_size);
    style.set_popup_rounding(appearance.metrics.popup_rounding);
    style.set_popup_border_size(appearance.metrics.border);
    style.set_window_rounding(fs * appearance.metrics.window_rounding);
    style.set_scrollbar_rounding(fs * appearance.metrics.scrollbar_rounding);
    style.set_scrollbar_size(fs * appearance.metrics.scrollbar_size);
    style.set_tab_bar_border_size(0.0);
    style.set_disabled_alpha(appearance.metrics.disabled_alpha);
}

#[cfg(all(test, feature = "imgui"))]
mod tests {
    use super::*;
    use crate::font::UiFonts;
    use dear_imgui_rs::{Context, FontSource, StyleColor};
    use std::path::PathBuf;

    #[test]
    fn palette_and_opacity_changes_preserve_font_atlas_and_opaque_ink() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = Context::create();
        context.set_ini_filename(None::<PathBuf>).unwrap();
        let mut fonts = UiFonts::default();
        let mut appearance = UiTheme::default();
        assert!(fonts.apply(&mut context, &appearance.fonts, false).unwrap());
        let main = fonts.main;
        let generation = fonts.generation;
        let mut previous_text = None;
        for opacity in [0.0, 0.5, 1.0] {
            appearance.opacity = opacity;
            apply_style(&mut context, &appearance, false);
            assert!(!fonts.apply(&mut context, &appearance.fonts, false).unwrap());
            assert_eq!(fonts.main, main);
            assert_eq!(fonts.generation, generation);
            let style = context.style();
            assert_eq!(style.color(StyleColor::WindowBg)[3], opacity);
            assert_eq!(style.color(StyleColor::TitleBg)[3], opacity);
            assert_eq!(style.color(StyleColor::ChildBg)[3], 0.0);
            for slot in [
                StyleColor::Text,
                StyleColor::PopupBg,
                StyleColor::FrameBg,
                StyleColor::Button,
                StyleColor::CheckMark,
            ] {
                assert_eq!(style.color(slot)[3], 1.0, "{slot:?}");
            }
            let text = style.color(StyleColor::Text);
            if let Some(previous) = previous_text {
                assert_eq!(text, previous);
            }
            previous_text = Some(text);
        }
        appearance.palette.accent = [0.9, 0.2, 0.3, 1.0];
        apply_style(&mut context, &appearance, false);
        assert!(!fonts.apply(&mut context, &appearance.fonts, false).unwrap());
        assert_eq!(fonts.generation, generation);
    }

    #[test]
    fn embedded_appearance_preserves_host_size_and_backgrounds() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = Context::create();
        context.set_ini_filename(None::<PathBuf>).unwrap();
        let host = context
            .font_atlas()
            .add_font(&[FontSource::default_font_with_size(13.0)]);
        context.style_mut().set_font_size_base(27.0);
        let background = [0.11, 0.22, 0.33, 0.88];
        let child = [0.44, 0.55, 0.66, 0.77];
        context
            .style_mut()
            .set_color(StyleColor::WindowBg, background);
        context.style_mut().set_color(StyleColor::ChildBg, child);
        let mut fonts = UiFonts::default();
        let appearance = UiTheme::default();
        fonts.apply(&mut context, &appearance.fonts, true).unwrap();
        apply_style(&mut context, &appearance, true);
        assert_eq!(host.reference_size(), Some(13.0));
        assert_eq!(context.style().font_size_base(), 27.0);
        assert_eq!(context.style().color(StyleColor::WindowBg), background);
        assert_eq!(context.style().color(StyleColor::ChildBg), child);
    }
}
