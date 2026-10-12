//! Application settings, profile persistence, keybindings and appearance.
pub mod keybinds;
mod layout_slot;
pub use layout_slot::LayoutSlot;
mod persistence;
pub mod settings;
pub mod theme;
pub use settings::{EffectPreset, SETTINGS_FILE, Settings, read_json, write_json};
#[cfg(test)]
#[allow(dead_code)]
#[path = "../tests/support/temp_dir.rs"]
mod test_support;
