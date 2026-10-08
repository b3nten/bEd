//! Bed terminal components. Attribution: workspace LICENSE and NOTICE.
#[cfg(feature = "ui")]
pub mod bed_terminal;
mod process_title;
pub mod terminal;
#[cfg(feature = "ui")]
pub mod terminal_font;
pub mod terminal_pty;
#[cfg(feature = "ui")]
pub mod terminal_view;

#[cfg(all(test, feature = "ui"))]
pub(crate) static IMGUI_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
