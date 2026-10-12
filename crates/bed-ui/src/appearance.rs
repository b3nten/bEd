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
        Self {
            background: rgb(0x1au8, 0x1b, 0x26),
            tab_bar: rgb(0x1a, 0x1b, 0x26),
            surface: rgb(0x16, 0x16, 0x1e),
            text: rgb(0xc0, 0xca, 0xf5),
            accent: rgb(0x7a, 0xa2, 0xf7),
            selection: rgb(0x28, 0x34, 0x57),
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct UiAppearance {
    pub fonts: UiFontOptions,
    pub palette: UiPalette,
    pub opacity: f32,
}
impl Default for UiAppearance {
    fn default() -> Self {
        Self {
            fonts: UiFontOptions::default(),
            palette: UiPalette::default(),
            opacity: 1.0,
        }
    }
}

/// Apply runtime styling before beginning a frame. Embedded users preserve host
/// background and sizing; their main font is selected explicitly while drawing.
#[cfg(feature = "imgui")]
pub fn apply_style(
    context: &mut dear_imgui_rs::Context,
    appearance: &UiAppearance,
    embedded: bool,
) {
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
    for (slot, color) in
        crate::util::popup_style::control_colors(palette.surface, text, palette.accent)
    {
        style.set_color(slot, color);
    }
    style.set_color(StyleColor::TextSelectedBg, palette.selection);
    let fs = appearance.fonts.size;
    style.set_frame_rounding(fs * 0.25);
    style.set_frame_border_size(1.0);
    style.set_frame_padding([fs * 0.55, fs * 0.30]);
    style.set_grab_rounding(fs * 0.2);
    style.set_grab_min_size(fs * 0.5);
    style.set_popup_rounding(crate::util::popup_style::POPUP_ROUNDING);
    style.set_popup_border_size(crate::util::popup_style::POPUP_BORDER_SIZE);
    style.set_window_rounding(fs * 0.4);
    style.set_scrollbar_rounding(fs * 0.3);
    style.set_scrollbar_size(fs * 0.55);
    style.set_tab_bar_border_size(0.0);
    style.set_disabled_alpha(0.75);
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
        let mut appearance = UiAppearance::default();
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
        let appearance = UiAppearance::default();
        fonts.apply(&mut context, &appearance.fonts, true).unwrap();
        apply_style(&mut context, &appearance, true);
        assert_eq!(host.reference_size(), Some(13.0));
        assert_eq!(context.style().font_size_base(), 27.0);
        assert_eq!(context.style().color(StyleColor::WindowBg), background);
        assert_eq!(context.style().color(StyleColor::ChildBg), child);
    }
}
