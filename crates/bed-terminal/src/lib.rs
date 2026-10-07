//! Bed terminal components. Attribution: workspace LICENSE, NOTICE and UPSTREAM_REVISION.
#[cfg(feature = "ui")]
pub mod bed_terminal;
pub mod terminal;
#[cfg(feature = "ui")]
pub mod terminal_font;
pub mod terminal_pty;
#[cfg(feature = "ui")]
pub mod terminal_view;

#[cfg(all(test, feature = "ui"))]
pub(crate) static IMGUI_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
