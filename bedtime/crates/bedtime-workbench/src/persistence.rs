//! Saving the workbench, on top of `bedtime-persistence`.
//!
//! Workbench saves the world snapshot in `WorkbenchStore`: layouts, areas,
//! panels and anything else marked `Persist`. It adds hooks so a snapshot
//! keeps the workbench valid: panels with nothing saved are left out, layouts
//! are only restored into a workbench that has none, and which tab was
//! selected and which area focused are put back after restoring. Saving a
//! panel's concrete component is enough to restore the panel.
mod templates;
#[cfg(test)]
pub(crate) mod tests;

pub use bedtime_persistence::*;
pub use templates::*;

use crate::*;
use crate::{Notification, WorkbenchWorldExt};
use bedtime_persistence::snapshot::{replace, value};
use bevy::{
    ecs::{entity::EntityHashMap, message::MessageCursor},
    prelude::*,
};
use bevy_world_serialization::DynamicWorld;
use std::{any::TypeId, collections::HashSet};

/// The workbench's store: the world snapshot (layouts, areas, panels and
/// whatever else is marked `Persist`). With workspaces it's the workspace's.
pub struct WorkbenchStore;
impl Store for WorkbenchStore {}

pub(crate) fn register(app: &mut App) {
    if !app.is_plugin_added::<PersistencePlugin>() {
        app.add_plugins(PersistencePlugin);
    }
    app.register_type::<Layout>()
        .register_type::<ActiveLayout>()
        .register_type::<Area>()
        .register_type::<InLayout>()
        .register_type::<LayoutAreas>()
        .register_type::<InArea>()
        .register_type::<AreaPanels>()
        .register_type::<crate::layout::LayoutOrder>()
        .register_type::<LayoutTemplate>()
        .persist_world::<WorkbenchStore>()
        .on_capture(|_, dynamic, _| leave_out_unsaved_panels(dynamic))
        .check_restore(only_into_an_empty_workbench)
        .on_restore(put_back_selection_and_focus)
        .configure_sets(Last, StoreSystems::Save.before(SessionSystems::Close))
        .add_systems(Update, report_store_errors);
}

/// Layouts can only be restored into a workbench that has none yet.
fn only_into_an_empty_workbench(world: &World, dynamic: &DynamicWorld) -> Result<(), String> {
    if dynamic
        .entities
        .iter()
        .any(|e| value::<Layout>(e).is_some())
        && !world.workbench().layouts().is_empty()
    {
        return Err("Layouts can only be restored into an empty workbench".into());
    }
    Ok(())
}

/// Hooks select newly attached tabs and areas as entities arrive, so put back
/// which tabs were selected and which areas focused. A workbench that isn't
/// valid afterwards undoes the restore.
fn put_back_selection_and_focus(
    world: &mut World,
    dynamic: &DynamicWorld,
    map: &EntityHashMap<Entity>,
) -> Result<(), String> {
    for entity in &dynamic.entities {
        let live = map[&entity.entity];
        if let Some(area) = value::<Area>(entity) {
            world.get_mut::<Area>(live).unwrap().selected = area.selected.map(|e| map[&e]);
        }
        if let Some(layout) = value::<Layout>(entity) {
            world.get_mut::<Layout>(live).unwrap().focused_area =
                layout.focused_area.map(|e| map[&e]);
        }
    }
    validate_workbench(world)
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

/// A panel entity saved with nothing but its place in an area couldn't be
/// rebuilt, so it's dropped from the save, along with references to it.
fn leave_out_unsaved_panels(dynamic: &mut DynamicWorld) {
    let unsaved: HashSet<Entity> = dynamic
        .entities
        .iter()
        .filter(|e| e.components.len() == 1 && value::<InArea>(e).is_some())
        .map(|e| e.entity)
        .collect();
    leave_out_panels(dynamic, &unsaved);
}

/// Drop `unsaved` panels from a capture, along with references to them.
fn leave_out_panels(dynamic: &mut DynamicWorld, unsaved: &HashSet<Entity>) {
    if unsaved.is_empty() {
        return;
    }
    dynamic.entities.retain(|e| !unsaved.contains(&e.entity));
    for entity in &mut dynamic.entities {
        // The tab list's field is private (Bevy requires it), so edit the
        // captured value through reflection.
        for component in &mut entity.components {
            let is_tabs = component
                .get_represented_type_info()
                .is_some_and(|i| i.type_id() == TypeId::of::<AreaPanels>());
            if let (true, bevy::reflect::ReflectMut::TupleStruct(tabs)) =
                (is_tabs, component.reflect_mut())
                && let Some(bevy::reflect::ReflectMut::List(list)) =
                    tabs.field_mut(0).map(|field| field.reflect_mut())
            {
                for index in (0..list.len()).rev() {
                    let panel = list.get(index).and_then(|v| v.try_downcast_ref::<Entity>());
                    if panel.is_some_and(|panel| unsaved.contains(panel)) {
                        list.remove(index);
                    }
                }
            }
        }
        if let Some(mut area) = value::<Area>(entity)
            && area.selected.is_some_and(|panel| unsaved.contains(&panel))
        {
            area.selected = None;
            replace(entity, area);
        }
    }
}
