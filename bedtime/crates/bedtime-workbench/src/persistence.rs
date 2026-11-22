//! Saving the workbench, on top of `bedtime-persistence`.
//!
//! Workbench saves the world snapshot in `WorkbenchStore`: layouts, areas,
//! panels (bedtime-ui's `persistence` feature keeps their graph valid) and
//! anything else marked `Persist`. Saving a panel's concrete component is
//! enough to restore the panel.
#[cfg(test)]
pub(crate) mod tests;

pub use bedtime_persistence::*;
pub use bedtime_ui::layouts::LayoutTemplate;

use crate::*;
use crate::{BedtimeWorldExt, Notification};
use bevy::{ecs::message::MessageCursor, prelude::*};

/// The session's store: the world snapshot (layouts, areas, panels and
/// whatever else is marked `Persist`), and what plugins keep with it. It's
/// `<config>/session`; with workspaces it's the open workspace's, a store
/// for each folder.
pub struct WorkbenchStore;
impl Store for WorkbenchStore {}

/// The app's own store, the same whatever is open: its settings (the Global
/// layer) and what plugins keep for the whole app. It's `<config>/global`.
pub struct GlobalStore;
impl Store for GlobalStore {}

/// Every app has both stores and a settings layer: with a
/// `PersistenceDirectory` they're saved, without one they're in memory.
pub(crate) fn register(app: &mut App) {
    let global = app
        .world()
        .get_resource::<PersistenceDirectory>()
        .and_then(|directory| directory.join("global"));
    if let Some(global) = global {
        app.add_store::<GlobalStore>(DirectoryStore::new(global));
    }
    crate::settings::SettingsAppExt::add_settings_layer::<GlobalStore>(app);
    app.persist_world::<WorkbenchStore>()
        .configure_sets(Last, StoreSystems::Save.before(SessionSystems::Close))
        .add_systems(Update, report_store_errors);
}

/// The session's store is `<config>/session`, unless something gave it a
/// place of its own while the plugins were built (workspaces does).
pub(crate) fn finish(app: &mut App) {
    let given = app
        .world()
        .resource::<Stores>()
        .backend(StoreId::of::<WorkbenchStore>())
        .is_some();
    let session = app
        .world()
        .get_resource::<PersistenceDirectory>()
        .and_then(|directory| directory.join("session"));
    if let (false, Some(session)) = (given, session) {
        app.add_store::<WorkbenchStore>(DirectoryStore::new(session));
    }
}

/// Show store failures as notifications.
fn report_store_errors(world: &mut World, mut cursor: Local<MessageCursor<StoreError>>) {
    let errors: Vec<_> = cursor
        .read(world.resource::<Messages<StoreError>>())
        .map(ToString::to_string)
        .collect();
    for error in errors {
        world.bedtime_mut().notify(Notification::error(error));
    }
}
