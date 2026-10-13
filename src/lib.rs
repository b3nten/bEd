#[cfg(not(any(target_os = "macos", target_os = "linux")))]
compile_error!("Bed supports macOS and Linux only.");

pub mod app;
pub mod builtins;
#[cfg(target_os = "macos")]
pub mod native_commands;
mod remote_helpers;
#[cfg(all(test, feature = "effects"))]
mod rendering;
#[cfg(feature = "automation")]
mod smoke;
mod startup;
pub use bed_app as app_runtime;
#[cfg(test)]
pub(crate) static IMGUI_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
