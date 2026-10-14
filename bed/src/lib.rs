extern crate self as bed;
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
compile_error!("Bed supports macOS and Linux only.");

pub mod app;
pub mod application;
pub(crate) mod automation;
pub mod builtins;
pub mod config;
pub mod native_commands;
mod remote_helpers;
pub mod settings_view;
mod smoke;
mod startup;
#[cfg(all(test, unix))]
mod terminal_tests;
pub mod themes;
#[cfg(test)]
pub(crate) static IMGUI_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
#[cfg(test)]
#[path = "../../bedtime/tests/support/temp_dir.rs"]
pub(crate) mod test_support;
