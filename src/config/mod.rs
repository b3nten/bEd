//! bEd's settings draft, theme-file editing, and core preference projection.
pub(crate) mod integration;
pub mod keybinds;
pub use integration::{
    ensure_settings_view, flush_settings_outbox, register_app_settings, settings_owner, sync_view,
};
mod layout_slot;
pub use layout_slot::LayoutSlot;
mod persistence;
pub mod settings;
pub mod theme;
#[cfg(test)]
pub(crate) use crate::test_support;
#[cfg(test)]
pub(crate) use settings::read_json;
pub use settings::{EffectPreset, SETTINGS_FILE, Settings};
