//! Theme and spacing shared by Bed's controls and popups.
use bed_editing::util::color::{blend, ensure_contrast};
use dear_imgui_rs::{ColorStackToken, StyleColor, StyleStackToken, StyleVar, Ui};

pub const POPUP_ROUNDING: f32 = 7.0;
pub const POPUP_BORDER_SIZE: f32 = 1.0;
const DIALOG_ROUNDING: f32 = 8.0;

/// Derive every control state from the active theme's surface, ink and accent.
pub fn control_colors(
    background: [f32; 4],
    text: [f32; 4],
    accent: [f32; 4],
) -> impl Iterator<Item = (StyleColor, [f32; 4])> {
    let background = [background[0], background[1], background[2], 1.0];
    let text = ensure_contrast(text, background, 4.5);
    let surface = |amount| blend(text, background, amount);
    let selected = blend(accent, background, 0.16);
    let text = ensure_contrast(ensure_contrast(text, surface(0.16), 4.5), selected, 4.5);
    let accent = ensure_contrast(accent, surface(0.10), 3.0);
    let accent_surface = |amount| blend(accent, background, amount);
    [
        (StyleColor::Text, text),
        (StyleColor::InputTextCursor, text),
        (
            StyleColor::TextDisabled,
            ensure_contrast(blend(text, background, 0.60), background, 3.0),
        ),
        (StyleColor::PopupBg, surface(0.025)),
        (StyleColor::Border, surface(0.30)),
        (StyleColor::BorderShadow, [0.0; 4]),
        (StyleColor::FrameBg, surface(0.045)),
        (StyleColor::FrameBgHovered, surface(0.08)),
        (StyleColor::FrameBgActive, surface(0.12)),
        (StyleColor::Button, surface(0.075)),
        (StyleColor::ButtonHovered, surface(0.12)),
        (StyleColor::ButtonActive, surface(0.16)),
        (
            StyleColor::CheckMark,
            ensure_contrast(accent, selected, 3.0),
        ),
        (StyleColor::CheckboxSelectedBg, selected),
        (StyleColor::SliderGrab, accent),
        (
            StyleColor::SliderGrabActive,
            ensure_contrast(accent, surface(0.12), 4.5),
        ),
        (StyleColor::Header, selected),
        (StyleColor::HeaderHovered, surface(0.10)),
        (StyleColor::HeaderActive, surface(0.16)),
        (StyleColor::Separator, surface(0.18)),
        (StyleColor::SeparatorHovered, surface(0.35)),
        (StyleColor::SeparatorActive, accent),
        (StyleColor::ResizeGrip, surface(0.20)),
        (StyleColor::ResizeGripHovered, surface(0.40)),
        (StyleColor::ResizeGripActive, accent),
        (StyleColor::ScrollbarBg, [0.0; 4]),
        // Native hover/active states reveal the scrollbar without changing layout.
        (StyleColor::ScrollbarGrab, [0.0; 4]),
        (StyleColor::ScrollbarGrabHovered, surface(0.50)),
        (StyleColor::ScrollbarGrabActive, surface(0.65)),
        (StyleColor::TableHeaderBg, surface(0.06)),
        (StyleColor::TableBorderStrong, surface(0.25)),
        (StyleColor::TableBorderLight, surface(0.12)),
        (StyleColor::TableRowBg, [0.0; 4]),
        (StyleColor::TableRowBgAlt, surface(0.025)),
        (
            StyleColor::TextLink,
            ensure_contrast(accent, background, 4.5),
        ),
        (StyleColor::TextSelectedBg, accent_surface(0.20)),
        (StyleColor::NavCursor, accent),
        (StyleColor::DockingPreview, accent_surface(0.25)),
        (StyleColor::ModalWindowDimBg, [0.0, 0.0, 0.0, 0.35]),
    ]
    .into_iter()
}

pub struct ControlsStyle<'ui> {
    _colors: Vec<ColorStackToken<'ui>>,
    _vars: Vec<StyleStackToken<'ui>>,
}
impl Drop for ControlsStyle<'_> {
    fn drop(&mut self) {
        while let Some(token) = self._vars.pop() {
            drop(token);
        }
        while let Some(token) = self._colors.pop() {
            drop(token);
        }
    }
}

/// Scope this to controls, preserving an embedding host's style on return.
pub fn controls_style(ui: &Ui) -> ControlsStyle<'_> {
    let fs = ui.current_font_size();
    ControlsStyle {
        _colors: control_colors(
            ui.style_color(StyleColor::WindowBg),
            ui.style_color(StyleColor::Text),
            ui.style_color(StyleColor::CheckMark),
        )
        .map(|(slot, color)| ui.push_style_color(slot, color))
        .collect(),
        _vars: [
            StyleVar::FrameRounding(fs * 0.25),
            StyleVar::FrameBorderSize(1.0),
            StyleVar::FramePadding([fs * 0.55, fs * 0.30]),
            StyleVar::ItemSpacing([fs * 0.6, fs * 0.4]),
            StyleVar::GrabRounding(fs * 0.2),
            StyleVar::GrabMinSize(fs * 0.5),
            StyleVar::PopupRounding(POPUP_ROUNDING),
            StyleVar::PopupBorderSize(POPUP_BORDER_SIZE),
            StyleVar::ScrollbarRounding(fs * 0.3),
            StyleVar::ScrollbarSize(fs * 0.55),
            StyleVar::DisabledAlpha(0.75),
        ]
        .into_iter()
        .map(|var| ui.push_style_var(var))
        .collect(),
    }
}

pub struct DialogStyle<'ui> {
    _vars: Vec<StyleStackToken<'ui>>,
    _background: ColorStackToken<'ui>,
    _controls: ControlsStyle<'ui>,
}
impl Drop for DialogStyle<'_> {
    fn drop(&mut self) {
        while let Some(token) = self._vars.pop() {
            drop(token);
        }
    }
}

/// Push before BeginPopupModal and retain until after the modal ends.
pub fn dialog_style(ui: &Ui) -> DialogStyle<'_> {
    let controls = controls_style(ui);
    let fs = ui.current_font_size();
    DialogStyle {
        _vars: [
            StyleVar::WindowPadding([fs * 0.85, fs * 0.7]),
            StyleVar::WindowRounding(DIALOG_ROUNDING),
            StyleVar::WindowBorderSize(1.0),
        ]
        .into_iter()
        .map(|var| ui.push_style_var(var))
        .collect(),
        _background: ui.push_style_color(StyleColor::WindowBg, ui.style_color(StyleColor::PopupBg)),
        _controls: controls,
    }
}

/// Restores the caller's style when dropped after the popup has ended.
pub struct ContextMenuStyle<'ui> {
    _vars: Vec<StyleStackToken<'ui>>,
    _viewport: PopupViewport<'ui>,
    _controls: ControlsStyle<'ui>,
}
impl Drop for ContextMenuStyle<'_> {
    fn drop(&mut self) {
        while let Some(token) = self._vars.pop() {
            drop(token);
        }
    }
}

struct PopupViewport<'ui> {
    ui: &'ui Ui,
    id: u32,
    armed: bool,
}
impl<'ui> PopupViewport<'ui> {
    fn new(ui: &'ui Ui) -> Self {
        let has_viewport = ui.with_bound_context(|| unsafe {
            (*dear_imgui_rs::sys::igGetCurrentContext())
                .NextWindowData
                .HasFlags
                & dear_imgui_rs::sys::ImGuiNextWindowDataFlags_HasViewport
                != 0
        });
        let fallback = ui.with_bound_context(|| unsafe {
            (*dear_imgui_rs::sys::igGetCurrentWindowRead()).IsFallbackWindow
        });
        let viewport = if fallback {
            // The implicit Debug window can own a dummy viewport that is
            // destroyed at frame end. Popups submitted outside a window need
            // a real destination; an explicit caller viewport still wins.
            ui.main_viewport().id()
        } else {
            ui.window_viewport().id()
        };
        if !has_viewport {
            // An escaping popup can acquire its own platform viewport, whose
            // native window deliberately removes ImGui's rounded corners.
            // Lock to this window's viewport so both placement and appearance
            // remain consistent, including menus in detached editor windows.
            ui.set_next_window_viewport(viewport);
        }
        Self {
            ui,
            id: viewport.raw(),
            armed: !has_viewport,
        }
    }
}
impl Drop for PopupViewport<'_> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        self.ui.with_bound_context(|| unsafe {
            let next = &mut (*dear_imgui_rs::sys::igGetCurrentContext()).NextWindowData;
            // Begin consumes NextWindowData. If no popup was begun (including
            // a skipped native context item), leave the caller's next window alone.
            if next.HasFlags & dear_imgui_rs::sys::ImGuiNextWindowDataFlags_HasViewport != 0
                && next.ViewportId == self.id
            {
                next.HasFlags &= !dear_imgui_rs::sys::ImGuiNextWindowDataFlags_HasViewport;
            }
        });
    }
}

/// Push before beginning a context popup and retain through its EndPopup.
pub fn context_menu_style(ui: &Ui) -> ContextMenuStyle<'_> {
    let controls = controls_style(ui);
    ContextMenuStyle {
        _vars: [
            StyleVar::WindowPadding([8.0, 6.0]),
            StyleVar::ItemSpacing([6.0, 4.0]),
            // Native submenus are child windows and capture the Child* slots.
            StyleVar::ChildRounding(POPUP_ROUNDING),
            StyleVar::ChildBorderSize(POPUP_BORDER_SIZE),
        ]
        .into_iter()
        .map(|var| ui.push_style_var(var))
        .collect(),
        _viewport: PopupViewport::new(ui),
        _controls: controls,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bed_editing::util::color::contrast_ratio;
    #[test]
    fn light_and_dark_controls_keep_readable_text_and_disabled_labels() {
        for (background, text, accent) in [
            (
                [0.99, 0.96, 0.89, 1.0],
                [0.39, 0.48, 0.51, 1.0],
                [0.15, 0.55, 0.82, 1.0],
            ),
            (
                [0.10, 0.10, 0.15, 1.0],
                [0.77, 0.79, 0.96, 1.0],
                [0.48, 0.64, 0.97, 1.0],
            ),
            ([0.95; 4], [1.0; 4], [1.0, 0.1, 0.7, 1.0]),
        ] {
            let colors = control_colors(background, text, accent)
                .collect::<std::collections::HashMap<_, _>>();
            for surface in [
                StyleColor::PopupBg,
                StyleColor::FrameBg,
                StyleColor::FrameBgHovered,
                StyleColor::FrameBgActive,
                StyleColor::Button,
                StyleColor::ButtonHovered,
                StyleColor::ButtonActive,
                StyleColor::Header,
                StyleColor::HeaderHovered,
                StyleColor::HeaderActive,
            ] {
                assert!(
                    contrast_ratio(colors[&StyleColor::Text], colors[&surface]) >= 4.49,
                    "{surface:?}: {:?}",
                    colors[&surface]
                );
            }
            assert!(contrast_ratio(colors[&StyleColor::TextDisabled], background) >= 2.99);
            assert_eq!(
                colors[&StyleColor::InputTextCursor],
                colors[&StyleColor::Text]
            );
            assert!(
                contrast_ratio(
                    colors[&StyleColor::CheckMark],
                    colors[&StyleColor::CheckboxSelectedBg]
                ) >= 2.99
            );
        }
    }
}
