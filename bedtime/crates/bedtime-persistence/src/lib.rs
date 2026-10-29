//! Saving a Bevy app's state between runs.
//!
//! - **Stores** are places, each with a backend ([`DirectoryStore`] writes
//!   `<dir>/<entry>.ron`). `app.add_store::<S>(backend)` turns saving on for
//!   store `S`; a store never added isn't saved.
//! - **The world snapshot.** Components and resources marked
//!   `#[reflect(Persist)]`, entities included, are saved together in one store:
//!   `app.persist_world::<S>()` writes its `world` entry.
//! - **Persisted resources.** `app.persist::<T, S>()` saves one serde
//!   resource as its own entry in `S`, named by `#[persisted(key = "...")]`.
//!
//! Everything is read before `Startup` (in [`StoreSystems::Load`]) and saved
//! about once a second while it changes, and when the app exits. Saved data
//! that couldn't be read is never saved over. Failures are sent as
//! [`StoreError`] messages for the app to show.
// The derive names this crate; let it inside the crate too.
extern crate self as bedtime_persistence;

mod resources;
mod stores;
#[cfg(test)]
mod tests;
mod world;

pub use bedtime_persistence_macros::Persisted;
pub use resources::{Persisted, read_persisted, write_persisted};
pub use stores::*;
pub use world::{
    CaptureHook, Persistence, PersistenceRef, PersistenceWorldExt, ReflectPersist, RestoreCheck,
    RestoreHook, snapshot,
};

use bevy::prelude::*;
use std::path::PathBuf;

/// Loads stores before `Startup` and saves them in `Last`. Adding stores and
/// persisted types works before or after this plugin.
pub struct PersistencePlugin;

impl Plugin for PersistencePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Stores>()
            .init_resource::<world::Hooks>()
            .add_message::<StoreError>()
            .add_systems(PreStartup, stores::load.in_set(StoreSystems::Load))
            .add_systems(Last, stores::save.in_set(StoreSystems::Save));
    }
}

/// The app's configuration directory: `$<override_variable>` if it's set,
/// else `$XDG_CONFIG_HOME/<app>`, else `~/.config/<app>`.
pub fn config_directory(app: &str, override_variable: Option<&str>) -> Option<PathBuf> {
    if let Some(directory) = override_variable.and_then(std::env::var_os) {
        return Some(directory.into());
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(base.join(app))
}
