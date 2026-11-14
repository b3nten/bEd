//! The world snapshot: everything marked `Persist`, saved together.
//!
//! Both sides of every relationship are saved and written back exactly, the way
//! Bevy scenes work, so relationship order survives a restore. Everything else
//! an entity needs at runtime comes back through required components and hooks.
//!
//! Plugins whose data needs more than that add hooks: a capture hook edits what
//! is about to be saved, a restore check refuses a snapshot before anything is
//! written, and a restore hook fixes things up afterwards (or fails, and every
//! entity the restore wrote is despawned again).
use bevy::{
    ecs::entity::EntityHashMap,
    prelude::*,
    reflect::{CreateTypeData, TypeRegistry},
};
use bevy_world_serialization::{DynamicWorld, DynamicWorldBuilder};

/// Marks a component or resource as saved: `#[reflect(Component, Persist)]`.
#[derive(Clone)]
pub struct ReflectPersist;

impl<T> CreateTypeData<T> for ReflectPersist {
    fn create_type_data(_: ()) -> Self {
        Self
    }
}

/// Edits a snapshot about to be saved (leaving out entities that couldn't be
/// rebuilt, say).
pub type CaptureHook = fn(&World, &mut DynamicWorld, &TypeRegistry);
/// Refuses a snapshot before anything from it is written.
pub type RestoreCheck = fn(&World, &DynamicWorld) -> Result<(), String>;
/// Runs after a snapshot is written, with the saved-to-live entity map. An
/// error undoes the restore's entities.
pub type RestoreHook = fn(&mut World, &DynamicWorld, &EntityHashMap<Entity>) -> Result<(), String>;

#[derive(Resource, Default)]
pub(crate) struct Hooks {
    pub(crate) capture: Vec<CaptureHook>,
    pub(crate) check: Vec<RestoreCheck>,
    pub(crate) restore: Vec<RestoreHook>,
}

/// Capturing the world snapshot: `world.persistence()`.
#[repr(transparent)]
pub struct PersistenceRef {
    world: World,
}

/// Restoring the world snapshot: `world.persistence_mut()`. It can capture too.
#[repr(transparent)]
pub struct Persistence {
    world: World,
}

impl std::ops::Deref for Persistence {
    type Target = PersistenceRef;
    fn deref(&self) -> &PersistenceRef {
        view(&self.world)
    }
}

fn view(world: &World) -> &PersistenceRef {
    // SAFETY: PersistenceRef is a repr(transparent) wrapper of World.
    unsafe { &*(world as *const World).cast::<PersistenceRef>() }
}

/// `world.persistence()` and `world.persistence_mut()`.
pub trait PersistenceWorldExt {
    fn persistence(&self) -> &PersistenceRef;
    fn persistence_mut(&mut self) -> &mut Persistence;
}

impl PersistenceWorldExt for World {
    fn persistence(&self) -> &PersistenceRef {
        view(self)
    }
    fn persistence_mut(&mut self) -> &mut Persistence {
        // SAFETY: Persistence is a repr(transparent) wrapper of World.
        unsafe { &mut *(self as *mut World).cast::<Persistence>() }
    }
}

impl PersistenceRef {
    /// Everything marked `Persist`, as RON text.
    pub fn capture(&self) -> Result<String, String> {
        let world = &self.world;
        let registry = world.resource::<AppTypeRegistry>().clone();
        let registry = registry.read();
        let entities: Vec<_> = world
            .archetypes()
            .iter()
            .flat_map(|archetype| archetype.entities())
            .map(|entity| entity.id())
            .filter(|&entity| {
                world
                    .get::<bevy::ecs::resource::IsResource>(entity)
                    .is_none()
            })
            .collect();
        let mut dynamic = DynamicWorldBuilder::from_world(world, &registry)
            .with_component_filter(snapshot::persisted_components(&registry))
            .with_resource_filter(snapshot::persisted_resources(&registry))
            .extract_entities(entities.into_iter())
            .extract_resources()
            .build();
        dynamic
            .entities
            .retain(|entity| !entity.components.is_empty());
        if let Some(hooks) = world.get_resource::<Hooks>() {
            for hook in &hooks.capture {
                hook(world, &mut dynamic, &registry);
            }
        }
        snapshot::check_references(&dynamic, &registry)?;
        snapshot::serialize(dynamic, &registry)
    }
}

impl Persistence {
    /// Write a captured snapshot back into the world. On failure nothing
    /// restored from entities is kept.
    pub fn restore(&mut self, text: &str) -> Result<(), String> {
        let world = &mut self.world;
        let registry = world.resource::<AppTypeRegistry>().clone();
        let registry = registry.read();
        let dynamic = snapshot::parse(text, &registry)?;
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
        }
        if let Some(hooks) = world.get_resource::<Hooks>() {
            for check in &hooks.check {
                check(world, &dynamic)?;
            }
        }
        snapshot::write(world, &dynamic, &registry).map(|_| ())
    }
}

/// Pieces for saving and restoring parts of the world (one layout as a
/// template, say) the way the world snapshot does.
pub mod snapshot {
    use super::{Hooks, ReflectPersist};
    use bevy::{
        ecs::{
            entity::{EntityHashMap, EntityMapper},
            reflect::{ReflectComponent, ReflectResource},
        },
        prelude::*,
        reflect::{ReflectFromReflect, TypeRegistry},
    };
    use bevy_world_serialization::{
        DynamicEntity, DynamicWorld, WorldFilter, serde::WorldDeserializer,
    };
    use serde::de::DeserializeSeed;
    use std::{any::TypeId, collections::HashSet};

    /// Every persisted component type.
    pub fn persisted_components(registry: &TypeRegistry) -> WorldFilter {
        registry
            .iter()
            .filter(|r| {
                r.data::<ReflectPersist>().is_some() && r.data::<ReflectResource>().is_none()
            })
            .fold(WorldFilter::deny_all(), |filter, r| {
                filter.allow_by_id(r.type_id())
            })
    }

    /// Every persisted resource type.
    pub fn persisted_resources(registry: &TypeRegistry) -> WorldFilter {
        registry
            .iter()
            .filter(|r| {
                r.data::<ReflectPersist>().is_some() && r.data::<ReflectResource>().is_some()
            })
            .fold(WorldFilter::deny_all(), |filter, r| {
                filter.allow_by_id(r.type_id())
            })
    }

    /// RON text with components in a stable order, so saves diff cleanly.
    pub fn serialize(mut dynamic: DynamicWorld, registry: &TypeRegistry) -> Result<String, String> {
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
    pub fn parse(text: &str, registry: &TypeRegistry) -> Result<DynamicWorld, String> {
        let mut assets = NoAssets { requested: false };
        let mut deserializer = ron::de::Deserializer::from_str(text).map_err(|e| e.to_string())?;
        let dynamic = WorldDeserializer {
            type_registry: registry,
            load_from_path: &mut assets,
        }
        .deserialize(&mut deserializer)
        .map_err(|e| e.to_string())?;
        if assets.requested {
            return Err(
                "Asset handles can't be saved; save a path or other descriptor instead".into(),
            );
        }
        Ok(dynamic)
    }

    /// Write saved entities into the world, then run the restore hooks. If one
    /// fails, every entity written is despawned again. Returns the
    /// saved-to-live entity map.
    pub fn write(
        world: &mut World,
        dynamic: &DynamicWorld,
        registry: &TypeRegistry,
    ) -> Result<EntityHashMap<Entity>, String> {
        let mut map = EntityHashMap::default();
        dynamic
            .write_to_world_with(world, &mut map, registry)
            .map_err(|e| e.to_string())?;
        world.flush();
        let hooks = world
            .get_resource::<Hooks>()
            .map(|hooks| hooks.restore.clone())
            .unwrap_or_default();
        for hook in hooks {
            if let Err(error) = hook(world, dynamic, &map) {
                for &live in map.values() {
                    if let Ok(entity) = world.get_entity_mut(live) {
                        entity.despawn();
                    }
                }
                return Err(error);
            }
        }
        Ok(map)
    }

    /// Every entity a saved value refers to must be saved too.
    pub fn check_references(dynamic: &DynamicWorld, registry: &TypeRegistry) -> Result<(), String> {
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
    pub fn refers_outside(
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

    /// A saved entity's `T`, if it has one.
    pub fn value<T: Component + FromReflect>(entity: &DynamicEntity) -> Option<T> {
        entity
            .components
            .iter()
            .filter(|v| {
                v.get_represented_type_info()
                    .is_some_and(|i| i.type_id() == TypeId::of::<T>())
            })
            .find_map(|v| T::from_reflect(v.as_partial_reflect()))
    }

    /// Replace a saved entity's `T`.
    pub fn replace<T: Component + Reflect>(entity: &mut DynamicEntity, component: T) {
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
}
