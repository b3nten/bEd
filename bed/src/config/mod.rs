//! bEd paths, defaults, and application preference registration.
pub(crate) mod integration;
pub use integration::{ensure_settings_view, register_app_settings};
mod layout_slot;
pub use layout_slot::LayoutSlot;
mod defaults;
pub use defaults::{AppConfig, SETTINGS_FILE};
