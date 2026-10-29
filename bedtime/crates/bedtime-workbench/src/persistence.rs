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
use crate::{Notification, WorkbenchWorldExt};
use bevy::{ecs::message::MessageCursor, prelude::*};

/// The workbench's store: the world snapshot (layouts, areas, panels and
/// whatever else is marked `Persist`). With workspaces it's the workspace's.
pub struct WorkbenchStore;
impl Store for WorkbenchStore {}

pub(crate) fn register(app: &mut App) {
    if !app.is_plugin_added::<PersistencePlugin>() {
        app.add_plugins(PersistencePlugin);
    }
    app.persist_world::<WorkbenchStore>()
        .configure_sets(Last, StoreSystems::Save.before(SessionSystems::Close))
        .add_systems(Update, report_store_errors);
}

/// Show store failures as notifications.
fn report_store_errors(world: &mut World, mut cursor: Local<MessageCursor<StoreError>>) {
    let errors: Vec<_> = cursor
        .read(world.resource::<Messages<StoreError>>())
        .map(ToString::to_string)
        .collect();
    for error in errors {
        world.workbench_mut().notify(Notification::error(error));
    }
}
