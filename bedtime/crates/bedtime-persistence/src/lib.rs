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
///
/// `directory` is where the app keeps what it saves: the
/// [`PersistenceDirectory`] resource, which plugins put their stores and
/// files under. Without one, nothing is saved.
#[derive(Default)]
pub struct PersistencePlugin {
    pub directory: Option<PathBuf>,
}

impl PersistencePlugin {
    /// Save under `directory`.
    pub fn directory(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: Some(directory.into()),
        }
    }

    /// Save in `app`'s configuration directory: `$<APP>_CONFIG_DIR` if it's
    /// set (`BED_CONFIG_DIR` for `bed`), else the usual place
    /// ([`config_directory`]).
    pub fn for_app(app: &str) -> Self {
        let variable = format!("{}_CONFIG_DIR", app.to_uppercase().replace('-', "_"));
        Self {
            directory: config_directory(app, Some(&variable)),
        }
    }
}

/// Where the app keeps what it saves, from [`PersistencePlugin`]. `None`
/// saves nothing.
#[derive(Resource, Clone, Debug, Default, PartialEq, Eq)]
pub struct PersistenceDirectory(pub Option<PathBuf>);

impl PersistenceDirectory {
    pub fn path(&self) -> Option<&std::path::Path> {
        self.0.as_deref()
    }

    /// `relative` under the directory, if there is one.
    pub fn join(&self, relative: impl AsRef<std::path::Path>) -> Option<PathBuf> {
        self.0.as_ref().map(|directory| directory.join(relative))
    }
}

impl Plugin for PersistencePlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(PersistenceDirectory(self.directory.clone()))
            .init_resource::<Stores>()
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
