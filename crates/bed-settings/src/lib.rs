//! Application settings, profile persistence, keybindings and appearance.
pub mod font;
pub mod keybinds;
mod persistence;
pub mod settings;
pub mod theme;
pub use settings::{EffectPreset, SETTINGS_FILE, Settings, read_json, write_json};
#[cfg(test)]
static IMGUI_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
#[cfg(test)]
#[allow(dead_code)]
#[path = "../tests/support/temp_dir.rs"]
mod test_support;
