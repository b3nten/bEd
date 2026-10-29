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
mod templates;
#[cfg(test)]
pub(crate) mod tests;

pub use stores::*;
pub use templates::*;

use crate::*;
use crate::{Workbench, WorkbenchWorldExt};
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
        .register_type::<crate::layout::LayoutOrder>()
        .register_type::<LayoutTemplate>();
    stores::install(app);
}

impl Workbench {
    /// Everything persisted that belongs in store `S`, as RON text.
    pub fn capture<S: Store>(&mut self) -> Result<String, String> {
        let world = &mut self.world;
        let registry = world.resource::<AppTypeRegistry>().clone();
        let registry = registry.read();
        let routing = world.resource::<PersistRouting>();
        let with_entities = TypeId::of::<S>() == TypeId::of::<WorkbenchStore>();
        let components = if with_entities {
            persisted_components(&registry)
        } else {
            WorldFilter::deny_all()
        };
        let mut resources = WorldFilter::deny_all();
        for registration in registry.iter() {
            let id = registration.type_id();
            if registration.data::<ReflectPersist>().is_some()
                && registration.data::<ReflectResource>().is_some()
                && routing.store_of(id) == TypeId::of::<S>()
            {
                resources = resources.allow_by_id(id);
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
        serialize(dynamic, &registry)
    }

    /// Write a capture of store `S` back into the world. Layouts can only be
    /// restored into a workbench that has none yet. On failure nothing restored
    /// from entities is kept.
    pub fn restore<S: Store>(&mut self, text: &str) -> Result<(), String> {
        let world = &mut self.world;
        let registry = world.resource::<AppTypeRegistry>().clone();
        let registry = registry.read();
        let dynamic = parse(text, &registry)?;
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
            && !world.workbench().layouts().is_empty()
        {
            return Err("Layouts can only be restored into an empty workbench".into());
        }

        write_entities(world, &dynamic, &registry).map(|_| ())
    }
}

/// Every persisted component type.
fn persisted_components(registry: &TypeRegistry) -> WorldFilter {
    registry
        .iter()
        .filter(|r| r.data::<ReflectPersist>().is_some() && r.data::<ReflectResource>().is_none())
        .fold(WorldFilter::deny_all(), |filter, r| {
            filter.allow_by_id(r.type_id())
        })
}

/// RON text with components in a stable order, so saves diff cleanly.
fn serialize(mut dynamic: DynamicWorld, registry: &TypeRegistry) -> Result<String, String> {
    for components in dynamic
        .entities
        .iter_mut()
        .map(|entity| &mut entity.components)
        .chain(std::iter::once(&mut dynamic.resources))
    {
        components.sort_by_key(|value| value.reflect_type_path().to_owned());
    }
    dynamic.serialize(registry).map_err(|e| e.to_string())
}

/// Parse saved RON text. Asset handles can't be saved, so none may appear.
fn parse(text: &str, registry: &TypeRegistry) -> Result<DynamicWorld, String> {
    let mut assets = NoAssets { requested: false };
    let mut deserializer = ron::de::Deserializer::from_str(text).map_err(|e| e.to_string())?;
    let dynamic = WorldDeserializer {
        type_registry: registry,
        load_from_path: &mut assets,
    }
    .deserialize(&mut deserializer)
    .map_err(|e| e.to_string())?;
    if assets.requested {
        return Err("Asset handles can't be saved; save a path or other descriptor instead".into());
    }
    Ok(dynamic)
}

/// Write saved entities into the world, then put back which tabs were selected
/// and which areas focused: hooks select newly attached tabs and areas as
/// entities arrive. If the workbench isn't valid afterwards, every entity
/// written is despawned again. Returns the saved-to-live entity map.
fn write_entities(
    world: &mut World,
    dynamic: &DynamicWorld,
    registry: &TypeRegistry,
) -> Result<EntityHashMap<Entity>, String> {
    let mut map = EntityHashMap::default();
    dynamic
        .write_to_world_with(world, &mut map, registry)
        .map_err(|e| e.to_string())?;
    world.flush();
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
    Ok(map)
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

/// Every entity a saved value refers to must be saved too.
fn check_references(dynamic: &DynamicWorld, registry: &TypeRegistry) -> Result<(), String> {
    let saved: HashSet<_> = dynamic.entities.iter().map(|e| e.entity).collect();
    for value in dynamic
        .entities
        .iter()
        .flat_map(|entity| &entity.components)
        .chain(&dynamic.resources)
    {
        if refers_outside(value.as_ref(), registry, &saved) {
            return Err(format!(
                "{} refers to an entity that isn't saved",
                value.reflect_type_path()
            ));
        }
    }
    Ok(())
}

/// Whether a captured component refers to an entity not in `saved`.
fn refers_outside(
    value: &dyn bevy::reflect::PartialReflect,
    registry: &TypeRegistry,
    saved: &HashSet<Entity>,
) -> bool {
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
    let Some(registration) = value
        .get_represented_type_info()
        .and_then(|info| registry.get(info.type_id()))
    else {
        return false;
    };
    let (Some(from), Some(component)) = (
        registration.data::<ReflectFromReflect>(),
        registration.data::<ReflectComponent>(),
    ) else {
        return false;
    };
    let Some(mut concrete) = from.from_reflect(value) else {
        return false;
    };
    let mut check = Check {
        saved,
        outside: false,
    };
    component.map_entities(concrete.as_mut(), &mut check);
    check.outside
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
