//! Settings: typed values resolved through layers, each layer kept in a store.
//!
//! Layers stack in the order the app adds them; later layers win. A layer
//! holds only what was set in it, so a missing value inherits from the layers
//! below, and finally from the setting's default. This module knows nothing
//! about what the layers mean; an app might stack a global store under a
//! workspace's so per-workspace values override global ones.
//!
//! A setting's `apply` runs whenever its resolved value changes: after a `set`
//! or `clear`, when layers load at startup, and when a layer's file changes on
//! disk. Consumers (resources, components) are updated there.
//!
//! The unit of override is the setting. Anything that should be overridable
//! per entry is split into one setting per entry with a derived key.
//!
//! With the `ui` feature, settings declared in a section describe how they're
//! edited, and [`ui`] draws them: layer tabs, sections and rows.
mod layers;
mod setting;
#[cfg(test)]
mod tests;
#[cfg(feature = "ui")]
pub mod ui;

/// The `settings!` macro, also at the crate root (where `#[macro_export]` puts it).
pub use crate::settings;
pub use layers::{SETTINGS_ENTRY, Settings, SettingsAppExt, SettingsMut, read_setting};
pub use setting::Setting;

use bedtime_persistence::{StoreError, StoreSystems};
use bevy::prelude::*;

#[doc(hidden)]
pub mod __private {
    pub use bedtime_persistence::StoreId;
    pub use bevy::prelude::World;
}

/// Loads the layers before the world snapshot (so restored entities see final
/// values), reloads layers changed on disk, and saves changed layers.
pub struct SettingsPlugin;

impl Plugin for SettingsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Settings>()
            .add_message::<SettingsError>()
            .add_message::<StoreError>()
            .configure_sets(PreStartup, SettingsSystems::Load.before(StoreSystems::Load))
            .add_systems(
                PreStartup,
                layers::load_layers.in_set(SettingsSystems::Load),
            )
            .add_systems(
                Last,
                (layers::reload_changed_layers, layers::save_layers).in_set(StoreSystems::Save),
            );
        #[cfg(feature = "ui")]
        app.init_resource::<ui::SettingsUi>();
    }
}

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SettingsSystems {
    /// Reading the layers, before `StoreSystems::Load`.
    Load,
}

/// Something about the settings the user should know: values that can't be
/// used, or a change that failed. Layers that can't be read or saved are
/// `StoreError`s.
#[derive(Message, Clone, Debug, PartialEq, Eq)]
pub struct SettingsError(pub String);

impl std::fmt::Display for SettingsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// `world.settings()` and `world.settings_mut()`.
pub trait SettingsWorldExt {
    fn settings(&self) -> &Settings;
    fn settings_mut(&mut self) -> &mut SettingsMut;
}

impl SettingsWorldExt for World {
    fn settings(&self) -> &Settings {
        self.resource::<Settings>()
    }
    fn settings_mut(&mut self) -> &mut SettingsMut {
        SettingsMut::of(self)
    }
}
