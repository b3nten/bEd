//! Shared control and popup presentation used by the host and plugins.
pub(crate) use bed_ui::util::popup_style::{POPUP_BORDER_SIZE, POPUP_ROUNDING};
pub use bed_ui::util::popup_style::{context_menu_style, dialog_style};

#[cfg(test)]
#[path = "../../../../tests/unit/workbench/presentation/popup_style_tests.rs"]
pub(crate) mod native_tests;
