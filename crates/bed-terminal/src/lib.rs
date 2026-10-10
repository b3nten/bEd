//! Bed terminal components. Attribution: workspace LICENSE and NOTICE.
#[cfg(feature = "ui")]
pub mod bed_terminal;
pub mod process_cwd;
mod process_title;
pub mod shell_bridge;
pub mod terminal;
#[cfg(feature = "ui")]
pub mod terminal_font;
pub mod terminal_input;
pub mod terminal_links;
pub mod terminal_pty;
#[cfg(feature = "ui")]
pub mod terminal_renderer;
#[cfg(feature = "ui")]
pub mod terminal_view;

#[cfg(all(test, feature = "ui"))]
pub(crate) static IMGUI_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
