//! bEd's namespaced settings draft and shared preference transactions.
pub(crate) mod integration;
pub use integration::{
    ensure_settings_view, flush_settings_outbox, register_app_settings, sync_view,
};
mod layout_slot;
pub use layout_slot::LayoutSlot;
pub mod settings;
#[cfg(test)]
pub(crate) use crate::test_support;
#[cfg(test)]
pub(crate) use settings::read_json;
pub use settings::{EffectPreset, SETTINGS_FILE, Settings};
