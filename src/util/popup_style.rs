//! Popup-only spacing shared by Bed's context menus.
use dear_imgui_rs::{StyleStackToken, StyleVar, Ui};

/// Restores the caller's style when dropped after the popup has ended.
pub struct ContextMenuStyle<'ui> {
    // Fields drop in declaration order, so the last push must come first.
    _spacing: StyleStackToken<'ui>,
    _padding: StyleStackToken<'ui>,
}

/// Push before beginning a context popup and retain through its EndPopup.
pub fn context_menu_style(ui: &Ui) -> ContextMenuStyle<'_> {
    let padding = ui.push_style_var(StyleVar::WindowPadding([8.0, 6.0]));
    let spacing = ui.push_style_var(StyleVar::ItemSpacing([6.0, 4.0]));
    ContextMenuStyle {
        _spacing: spacing,
        _padding: padding,
    }
}
