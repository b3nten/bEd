#[cfg(not(any(target_os = "macos", target_os = "linux")))]
compile_error!("Bed supports macOS and Linux only.");

pub mod app;
pub mod application;
#[cfg(feature = "automation")]
pub(crate) mod automation;
pub mod builtins;
pub mod config;
#[cfg(target_os = "macos")]
pub mod native_commands;
mod remote_helpers;
#[cfg(all(test, feature = "effects"))]
mod rendering;
#[cfg(feature = "settings")]
pub mod settings_view;
#[cfg(feature = "automation")]
mod smoke;
mod startup;
#[cfg(all(test, feature = "terminal", feature = "editor", unix))]
mod terminal_tests;
pub mod themes;
#[cfg(test)]
pub(crate) static IMGUI_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
#[cfg(test)]
#[path = "../tests/support/temp_dir.rs"]
pub(crate) mod test_support;
