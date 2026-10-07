//! Shared control and popup presentation used by the host and plugins.
pub use bed_ui::util::popup_style::{
    ContextMenuStyle, ControlsStyle, DialogStyle, context_menu_style, controls_style, dialog_style,
};
pub(crate) use bed_ui::util::popup_style::{POPUP_BORDER_SIZE, POPUP_ROUNDING, control_colors};

#[cfg(test)]
#[path = "popup_style_tests.rs"]
pub(crate) mod native_tests;
