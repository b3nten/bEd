//! Saving layouts (`persistence` feature): their types are `Persist`, and
//! hooks keep a restored layout graph valid. Which store they're saved in is
//! the app's choice (`persist_world`).
use super::*;
use bedtime_persistence::{
    PersistenceAppExt,
    snapshot::{replace, value},
};
use bevy::ecs::entity::EntityHashMap;
use bevy_world_serialization::DynamicWorld;
use std::{any::TypeId, collections::HashSet};

pub(crate) fn install(app: &mut App) {
    app.register_type::<Layout>()
        .register_type::<ActiveLayout>()
        .register_type::<Area>()
        .register_type::<InLayout>()
        .register_type::<LayoutAreas>()
        .register_type::<InArea>()
        .register_type::<AreaPanels>()
        .register_type::<LayoutOrder>()
        .register_type::<LayoutTemplate>()
        .on_capture(|_, dynamic, _| leave_out_unsaved_panels(dynamic))
        .check_restore(only_into_an_empty_workbench)
        .on_restore(put_back_selection_and_focus);
}

/// Layouts can only be restored into a world that has none yet.
fn only_into_an_empty_workbench(world: &World, dynamic: &DynamicWorld) -> Result<(), String> {
    if dynamic
        .entities
        .iter()
        .any(|e| value::<Layout>(e).is_some())
        && !world.layouts().all().is_empty()
    {
        return Err("Layouts can only be restored into a world without layouts".into());
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
    validate_layouts(world)
}

/// A panel entity saved with nothing but its place in an area couldn't be
/// rebuilt, so it's dropped from the save, along with references to it.
pub(super) fn leave_out_unsaved_panels(dynamic: &mut DynamicWorld) {
    let unsaved: HashSet<Entity> = dynamic
        .entities
        .iter()
        .filter(|e| e.components.len() == 1 && value::<InArea>(e).is_some())
        .map(|e| e.entity)
        .collect();
    leave_out_panels(dynamic, &unsaved);
}

/// Drop `unsaved` panels from a capture, along with references to them.
pub(super) fn leave_out_panels(dynamic: &mut DynamicWorld, unsaved: &HashSet<Entity>) {
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
