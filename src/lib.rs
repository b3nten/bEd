//! Bed desktop application composition. Reusable APIs live in the bed-* crates.
pub mod bed;
pub mod files;
pub mod util;
pub mod workbench;
#[cfg(test)]
pub(crate) static IMGUI_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
