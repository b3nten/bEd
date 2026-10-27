//! Saving the session world, and the stores it is saved to.
//!
//! Components and resources opt in with `#[reflect(Persist)]`. Entities with
//! persisted components go to `WorkbenchStore`; a persisted resource goes there
//! too, unless the app redirects it to another store with `persist_in`.
//!
//! Both sides of every relationship are saved and written back exactly, the way
//! Bevy scenes work, so tab and area order survive a restore. Everything else a
//! panel needs at runtime comes back through required components and hooks:
//! saving a panel's concrete component is enough to restore the panel.
//! A panel with nothing persisted is left out of the save.
mod stores;
#[cfg(test)]
pub(crate) mod tests;

pub use stores::*;

use crate::*;
use bevy::{
    ecs::{
        entity::{EntityHashMap, EntityMapper},
        reflect::{ReflectComponent, ReflectResource},
    },
    prelude::*,
    reflect::{CreateTypeData, ReflectFromReflect, TypeRegistry},
};
use bevy_world_serialization::{
    DynamicEntity, DynamicWorld, DynamicWorldBuilder, WorldFilter, serde::WorldDeserializer,
};
use serde::de::DeserializeSeed;
use std::{
    any::{TypeId, type_name},
    collections::HashSet,
};

/// Marks a component or resource as saved: `#[reflect(Component, Persist)]`.
#[derive(Clone)]
pub struct ReflectPersist;

impl<T> CreateTypeData<T> for ReflectPersist {
    fn create_type_data(_: ()) -> Self {
        Self
    }
}

pub(crate) fn register(app: &mut App) {
    app.register_type::<Layout>()
        .register_type::<ActiveLayout>()
        .register_type::<Area>()
        .register_type::<InLayout>()
        .register_type::<LayoutAreas>()
        .register_type::<InArea>()
        .register_type::<AreaPanels>()
        .register_type::<crate::layout::LayoutOrder>();
    stores::install(app);
}

/// Everything persisted that belongs in store `S`, as RON text.
pub fn capture<S: Store>(world: &mut World) -> Result<String, String> {
    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry = registry.read();
    let routing = world.resource::<PersistRouting>();
    let with_entities = TypeId::of::<S>() == TypeId::of::<WorkbenchStore>();
    let mut components = WorldFilter::deny_all();
    let mut resources = WorldFilter::deny_all();
    for registration in registry.iter() {
        if registration.data::<ReflectPersist>().is_none() {
            continue;
        }
        let id = registration.type_id();
        if registration.data::<ReflectResource>().is_some() {
            if routing.store_of(id) == TypeId::of::<S>() {
                resources = resources.allow_by_id(id);
            }
        } else if with_entities {
            components = components.allow_by_id(id);
        }
    }
    let entities: Vec<_> = if with_entities {
        world
            .archetypes()
            .iter()
            .flat_map(|archetype| archetype.entities())
            .map(|entity| entity.id())
            .filter(|&entity| {
                world
                    .get::<bevy::ecs::resource::IsResource>(entity)
                    .is_none()
            })
            .collect()
    } else {
        Vec::new()
    };
    let mut dynamic = DynamicWorldBuilder::from_world(world, &registry)
        .with_component_filter(components)
        .with_resource_filter(resources)
        .extract_entities(entities.into_iter())
        .extract_resources()
        .build();
    dynamic
        .entities
        .retain(|entity| !entity.components.is_empty());
    leave_out_unsaved_panels(&mut dynamic);
    check_references(&dynamic, &registry)?;
    for components in dynamic
        .entities
        .iter_mut()
        .map(|entity| &mut entity.components)
        .chain(std::iter::once(&mut dynamic.resources))
    {
        components.sort_by_key(|value| value.reflect_type_path().to_owned());
    }
    dynamic.serialize(&registry).map_err(|e| e.to_string())
}

/// Write a capture of store `S` back into the world. Layouts can only be
/// restored into a workbench that has none yet. On failure nothing restored
/// from entities is kept.
pub fn restore<S: Store>(world: &mut World, text: &str) -> Result<(), String> {
    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry = registry.read();
    let mut assets = NoAssets { requested: false };
    let mut deserializer = ron::de::Deserializer::from_str(text).map_err(|e| e.to_string())?;
    let dynamic = WorldDeserializer {
        type_registry: &registry,
        load_from_path: &mut assets,
    }
    .deserialize(&mut deserializer)
    .map_err(|e| e.to_string())?;
    if assets.requested {
        return Err("Asset handles can't be saved; save a path or other descriptor instead".into());
    }
    let with_entities = TypeId::of::<S>() == TypeId::of::<WorkbenchStore>();
    if !with_entities && !dynamic.entities.is_empty() {
        return Err(format!(
            "Only WorkbenchStore holds entities, not {}",
            type_name::<S>()
        ));
    }
    let routing = world.resource::<PersistRouting>();
    for value in dynamic
        .entities
        .iter()
        .flat_map(|entity| &entity.components)
        .chain(&dynamic.resources)
    {
        let registration = value
            .get_represented_type_info()
            .and_then(|info| registry.get(info.type_id()))
            .ok_or_else(|| format!("{} isn't registered", value.reflect_type_path()))?;
        if registration.data::<ReflectPersist>().is_none() {
            return Err(format!(
                "{} isn't marked Persist",
                value.reflect_type_path()
            ));
        }
        if registration.data::<ReflectResource>().is_some()
            && routing.store_of(registration.type_id()) != TypeId::of::<S>()
        {
            return Err(format!(
                "{} belongs in another store",
                value.reflect_type_path()
            ));
        }
    }
    if dynamic
        .entities
        .iter()
        .any(|e| value::<Layout>(e).is_some())
        && !layouts(world).is_empty()
    {
        return Err("Layouts can only be restored into an empty workbench".into());
    }

    let mut map = EntityHashMap::default();
    dynamic
        .write_to_world_with(world, &mut map, &registry)
        .map_err(|e| e.to_string())?;
    world.flush();
    // Hooks select newly attached tabs and areas as entities arrive; put back
    // what was actually selected and focused.
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
    if let Err(error) = validate_workbench(world) {
        for &live in map.values() {
            if let Ok(entity) = world.get_entity_mut(live) {
                entity.despawn();
            }
        }
        return Err(error);
    }
    Ok(())
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

/// Every entity a saved value refers to must be saved too.
fn check_references(dynamic: &DynamicWorld, registry: &TypeRegistry) -> Result<(), String> {
    struct Check<'a> {
        saved: &'a HashSet<Entity>,
        outside: bool,
    }
    impl EntityMapper for Check<'_> {
        fn get_mapped(&mut self, entity: Entity) -> Entity {
            self.outside |= entity != Entity::PLACEHOLDER && !self.saved.contains(&entity);
            entity
        }
        fn set_mapped(&mut self, _: Entity, _: Entity) {}
    }
    let saved: HashSet<_> = dynamic.entities.iter().map(|e| e.entity).collect();
    for value in dynamic
        .entities
        .iter()
        .flat_map(|entity| &entity.components)
        .chain(&dynamic.resources)
    {
        let registration = registry
            .get(value.get_represented_type_info().unwrap().type_id())
            .unwrap();
        let (Some(from), Some(component)) = (
            registration.data::<ReflectFromReflect>(),
            registration.data::<ReflectComponent>(),
        ) else {
            continue;
        };
        let Some(mut concrete) = from.from_reflect(value.as_partial_reflect()) else {
            continue;
        };
        let mut check = Check {
            saved: &saved,
            outside: false,
        };
        component.map_entities(concrete.as_mut(), &mut check);
        if check.outside {
            return Err(format!(
                "{} refers to an entity that isn't saved",
                value.reflect_type_path()
            ));
        }
    }
    Ok(())
}

fn value<T: Component + FromReflect>(entity: &DynamicEntity) -> Option<T> {
    entity
        .components
        .iter()
        .filter(|v| {
            v.get_represented_type_info()
                .is_some_and(|i| i.type_id() == TypeId::of::<T>())
        })
        .find_map(|v| T::from_reflect(v.as_partial_reflect()))
}

fn replace<T: Component + Reflect>(entity: &mut DynamicEntity, component: T) {
    entity.components.retain(|v| {
        v.get_represented_type_info()
            .is_none_or(|i| i.type_id() != TypeId::of::<T>())
    });
    entity.components.push(Box::new(component));
}

struct NoAssets {
    requested: bool,
}

impl bevy::asset::LoadFromPath for NoAssets {
    fn load_from_path_erased(
        &mut self,
        id: TypeId,
        _: bevy::asset::AssetPath<'static>,
    ) -> bevy::asset::UntypedHandle {
        self.requested = true;
        bevy::asset::UntypedHandle::default_for_type(id)
    }
}
