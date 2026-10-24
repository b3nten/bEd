//! Marker-selected, editable world captures. Hosts own storage and runtime activation.
mod layouts;
mod relationships;
use crate::layout::LayoutOrder;
use crate::*;
use bevy::{
    ecs::{
        entity::{EntityHashMap, EntityMapper},
        reflect::{ReflectComponent, ReflectResource},
    },
    prelude::*,
    reflect::{CreateTypeData, ReflectFromReflect, TypeData, TypeRegistry},
};
pub use bevy_world_serialization::{DynamicEntity, DynamicWorld};
use bevy_world_serialization::{DynamicWorldBuilder, WorldFilter, serde::WorldDeserializer};
pub use layouts::CapturedLayout;
pub use relationships::register_persisted_relationship;
use serde::{Deserialize, Serialize, de::DeserializeSeed};
use std::{
    any::TypeId,
    collections::{HashMap, HashSet},
    marker::PhantomData,
};

/// Reflection metadata selecting durable Workbench components and resources.
#[derive(Clone)]
pub struct ReflectWorkbenchPersist;
impl<T> CreateTypeData<T> for ReflectWorkbenchPersist {
    fn create_type_data(_: ()) -> Self {
        Self
    }
}

/// Encoded capture. Rust type paths and fields are part of this version's schema.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub version: u32,
    pub data: String,
}
const VERSION: u32 = 2;
const MAX_BYTES: usize = 4 * 1024 * 1024;
const MAX_ENTITIES: usize = 4096;

#[derive(Clone, Copy, PartialEq, Eq)]
enum CaptureScope {
    World,
    Layout,
}

/// A host closes the startup restore window once admission/loading completes.
/// This is deliberately independent of Bevy Startup, since hosts can load asynchronously.
#[derive(Resource)]
struct WorldRestoreFinished;

pub fn register(app: &mut App) {
    app.register_type::<Layout>()
        .register_type::<Area>()
        .register_type::<PanelInstance>()
        .register_type::<LayoutAreas>()
        .register_type::<AreaPanels>()
        .register_type::<ActiveLayout>()
        .register_type::<LayoutOrder>();
    register_persisted_relationship::<InLayout>(app);
    register_persisted_relationship::<InArea>(app);
    register_persisted_relationship::<InPanel>(app);
}

/// Detached data, not a live World: editing this cannot run ECS hooks or systems.
/// After edits, serialize/restore revalidate types, references and Workbench structure.
/// M is reflection metadata (e.g. ReflectWorkbenchPersist), not an entity marker.
pub struct CapturedWorld<M = ReflectWorkbenchPersist> {
    pub dynamic: DynamicWorld,
    marker: PhantomData<fn() -> M>,
}

impl Workbench<'_> {
    /// Capture every component and resource whose registered type carries M.
    /// Applications can use their own reflection metadata to select other partitions.
    /// Captured data can be incomplete while the host edits/projects it; serialize
    /// and restore enforce the final type allowlist and reference/structural invariants.
    pub fn capture_world<M: TypeData>(world: &mut World) -> Result<CapturedWorld<M>, String> {
        let registry = world.resource::<AppTypeRegistry>().clone();
        let registry = registry.read();
        // Reconciliation validates ordinary composition; it does not initialize panels.
        if registry
            .get(TypeId::of::<Layout>())
            .is_some_and(|r| r.data::<M>().is_some())
        {
            reconcile_workbench(world)?;
        }
        let filter = || {
            registry
                .iter()
                .filter(|r| r.data::<M>().is_some())
                .fold(WorldFilter::deny_all(), |f, r| f.allow_by_id(r.type_id()))
        };
        let entities: Vec<_> = world
            .archetypes()
            .iter()
            .flat_map(|a| a.entities())
            .map(|e| e.id())
            .filter(|e| world.get::<bevy::ecs::resource::IsResource>(*e).is_none())
            .collect();
        let mut dynamic = DynamicWorldBuilder::from_world(world, &registry)
            .with_component_filter(filter())
            .with_resource_filter(filter())
            .extract_entities(entities.into_iter())
            .extract_resources()
            .build();
        // InPanel is ownership, not durable content. Do not turn runtime-only
        // command/handler entities into empty restored panel parts.
        dynamic.entities.retain(|e| {
            !e.components.is_empty() && !(e.components.len() == 1 && value::<InPanel>(e).is_some())
        });
        Ok(CapturedWorld {
            dynamic,
            marker: PhantomData,
        })
    }

    /// Restore an admitted capture. Workbench graphs require an empty Workbench;
    /// resource replacement/admission and external runtime activation remain host policy.
    pub fn restore_world<M: TypeData>(
        world: &mut World,
        captured: CapturedWorld<M>,
    ) -> Result<Vec<Entity>, String> {
        if world.contains_resource::<WorldRestoreFinished>() {
            return Err("World restoration is only available during session startup".into());
        }
        if !captured.layouts().is_empty()
            && (!layouts(world).is_empty()
                || world
                    .query_filtered::<Entity, Or<(With<Area>, With<PanelInstance>)>>()
                    .iter(world)
                    .next()
                    .is_some())
        {
            return Err("Restore requires an empty admitted workbench".into());
        }
        insert_capture::<M>(world, captured.dynamic, CaptureScope::World)
    }

    /// Permanently close the world restore window after the host's startup loading.
    /// Capture/encoding and scoped layout replacement remain available.
    pub fn finish_world_restore(world: &mut World) {
        if !world.contains_resource::<WorldRestoreFinished>() {
            world.insert_resource(WorldRestoreFinished);
        }
    }
}

impl<M: TypeData> CapturedWorld<M> {
    /// Insert/replace a component in detached data; no required defaults or hooks run.
    pub fn insert_component<T: Component + Reflect>(&mut self, entity: Entity, component: T) {
        let index = self
            .dynamic
            .entities
            .iter()
            .position(|e| e.entity == entity);
        let entity = match index {
            Some(index) => &mut self.dynamic.entities[index],
            None => {
                self.dynamic.entities.push(DynamicEntity {
                    entity,
                    components: Vec::new(),
                });
                self.dynamic.entities.last_mut().unwrap()
            }
        };
        entity
            .components
            .retain(|v| type_id(v.as_ref()) != Some(TypeId::of::<T>()));
        entity.components.push(Box::new(component));
    }
    pub fn component<T: Component + FromReflect>(&self, entity: Entity) -> Option<T> {
        self.dynamic
            .entities
            .iter()
            .find(|e| e.entity == entity)
            .and_then(value::<T>)
    }
    pub fn has_entities(&self) -> bool {
        !self.dynamic.entities.is_empty()
    }
    pub fn layouts(&self) -> Vec<Entity> {
        self.dynamic
            .entities
            .iter()
            .filter(|e| value::<Layout>(e).is_some())
            .map(|e| e.entity)
            .collect()
    }
    pub fn components<T: Component + FromReflect>(&self) -> Vec<(Entity, T)> {
        self.dynamic
            .entities
            .iter()
            .filter_map(|e| value::<T>(e).map(|v| (e.entity, v)))
            .collect()
    }
    pub fn resource<T: Resource + FromReflect>(&self) -> Option<T> {
        resource::<T>(&self.dynamic)
    }
    pub fn replace_resource<T: Resource + Reflect>(&mut self, value: T) -> Result<(), String> {
        let slot = self
            .dynamic
            .resources
            .iter_mut()
            .find(|v| type_id(v.as_ref()) == Some(TypeId::of::<T>()))
            .ok_or("Resource is absent from capture")?;
        *slot = Box::new(value);
        Ok(())
    }
    /// Validate and encode a detached copy. Canonical IDs follow captured graph order;
    /// editing/encoding never changes entity IDs or values in the live world.
    pub fn serialize(&self, registry: &AppTypeRegistry) -> Result<Snapshot, String> {
        encode::<M>(&self.dynamic, registry, CaptureScope::World)
    }

    /// Decode and validate without inserting components or constructing required defaults.
    /// Custom Serde/FromReflect code is trusted and must be a pure value transformation.
    pub fn deserialize(snapshot: &Snapshot, registry: &AppTypeRegistry) -> Result<Self, String> {
        Ok(Self {
            dynamic: decode::<M>(snapshot, registry, CaptureScope::World)?,
            marker: PhantomData,
        })
    }
}

fn encode<M: TypeData>(
    dynamic: &DynamicWorld,
    registry: &AppTypeRegistry,
    scope: CaptureScope,
) -> Result<Snapshot, String> {
    let registry = registry.read();
    let relationship_plan = validate::<M>(dynamic, &registry, scope)?;
    let mut ordered = Vec::new();
    fn visit(
        dynamic: &DynamicWorld,
        registry: &TypeRegistry,
        e: Entity,
        ordered: &mut Vec<Entity>,
    ) {
        let mut pending = vec![e];
        while let Some(entity_id) = pending.pop() {
            if ordered.contains(&entity_id) {
                continue;
            }
            ordered.push(entity_id);
            let entity = dynamic
                .entities
                .iter()
                .find(|v| v.entity == entity_id)
                .unwrap();
            let mut targets: Vec<_> = entity
                .components
                .iter()
                .filter_map(|value| {
                    let adapter = relationships::adapter(registry, value.as_ref())?;
                    (type_id(value.as_ref()) == Some(adapter.target)).then_some(value)
                })
                .collect();
            targets.sort_by_key(|value| value.get_represented_type_info().unwrap().type_path());
            let mut children = Vec::new();
            for target in targets {
                let adapter = relationships::adapter(registry, target.as_ref()).unwrap();
                children.extend(
                    adapter
                        .children(target.as_ref())
                        .expect("Validated relationship target"),
                );
            }
            // Iterative depth-first traversal supports arbitrary application
            // hierarchies up to the entity bound without recursive stack growth.
            pending.extend(children.into_iter().rev());
        }
    }
    if let Some(layouts) = resource::<LayoutOrder>(dynamic) {
        for e in layouts.0 {
            visit(dynamic, &registry, e, &mut ordered);
        }
    }
    // Generic ownership roots and target collection order make a captured
    // hierarchy stable even if destination entities are allocated differently.
    for root in relationship_plan.roots {
        visit(dynamic, &registry, root, &mut ordered);
    }
    let mut remaining: Vec<_> = dynamic
        .entities
        .iter()
        .map(|e| e.entity)
        .filter(|e| !ordered.contains(e))
        .collect();
    remaining.sort();
    ordered.extend(remaining);
    let mapping: HashMap<_, _> = ordered
        .into_iter()
        .enumerate()
        .map(|(i, e)| (e, Entity::from_raw_u32(i as u32).unwrap()))
        .collect();
    let mut dynamic = clone_mapped(dynamic, &registry, &mapping)?;
    dynamic.entities.sort_by_key(|e| e.entity);
    // Bevy may remove empty reverse collections. Absence and an empty order
    // have the same meaning, so encode them identically across round trips.
    for entity in &mut dynamic.entities {
        let empty_areas = value::<LayoutAreas>(entity).is_some_and(|v| v.entities().is_empty());
        let empty_panels = value::<AreaPanels>(entity).is_some_and(|v| v.entities().is_empty());
        entity.components.retain(|v| {
            !(empty_areas && type_id(v.as_ref()) == Some(TypeId::of::<LayoutAreas>())
                || empty_panels && type_id(v.as_ref()) == Some(TypeId::of::<AreaPanels>()))
        });
    }
    for values in dynamic
        .entities
        .iter_mut()
        .map(|e| &mut e.components)
        .chain(std::iter::once(&mut dynamic.resources))
    {
        values.sort_by_key(|v| v.get_represented_type_info().unwrap().type_path());
    }
    let data = dynamic.serialize(&registry).map_err(|e| e.to_string())?;
    if data.len() > MAX_BYTES {
        return Err("Snapshot exceeds 4 MiB".into());
    }
    Ok(Snapshot {
        version: VERSION,
        data,
    })
}

fn decode<M: TypeData>(
    snapshot: &Snapshot,
    registry: &AppTypeRegistry,
    scope: CaptureScope,
) -> Result<DynamicWorld, String> {
    if snapshot.version != VERSION {
        return Err("Unsupported workbench snapshot version".into());
    }
    if snapshot.data.len() > MAX_BYTES {
        return Err("Snapshot exceeds 4 MiB".into());
    }
    let registry = registry.read();
    let mut assets = NoAssets { requested: false };
    let mut deserializer =
        ron::de::Deserializer::from_str(&snapshot.data).map_err(|e| e.to_string())?;
    let dynamic = WorldDeserializer {
        type_registry: &registry,
        load_from_path: &mut assets,
    }
    .deserialize(&mut deserializer)
    .map_err(|e| e.to_string())?;
    deserializer.end().map_err(|e| e.to_string())?;
    if assets.requested {
        return Err("Persist source descriptors instead of asset handles".into());
    }
    validate::<M>(&dynamic, &registry, scope)?;
    Ok(dynamic)
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
fn type_id(v: &dyn PartialReflect) -> Option<TypeId> {
    v.get_represented_type_info().map(|i| i.type_id())
}
fn value<T: FromReflect + 'static>(entity: &DynamicEntity) -> Option<T> {
    entity
        .components
        .iter()
        .filter(|v| type_id(v.as_ref()) == Some(TypeId::of::<T>()))
        .find_map(|v| T::from_reflect(v.as_partial_reflect()))
}
fn resource<T: FromReflect + 'static>(dynamic: &DynamicWorld) -> Option<T> {
    dynamic
        .resources
        .iter()
        .filter(|v| type_id(v.as_ref()) == Some(TypeId::of::<T>()))
        .find_map(|v| T::from_reflect(v.as_partial_reflect()))
}
fn structural(id: TypeId) -> bool {
    [
        TypeId::of::<Layout>(),
        TypeId::of::<Area>(),
        TypeId::of::<PanelInstance>(),
        TypeId::of::<ActiveLayout>(),
        TypeId::of::<LayoutOrder>(),
    ]
    .contains(&id)
}
fn clone_mapped(
    dynamic: &DynamicWorld,
    registry: &TypeRegistry,
    mapping: &HashMap<Entity, Entity>,
) -> Result<DynamicWorld, String> {
    struct Mapper<'a>(&'a HashMap<Entity, Entity>);
    impl EntityMapper for Mapper<'_> {
        fn get_mapped(&mut self, e: Entity) -> Entity {
            // Earlier schema-2 captures used from_bits(1), which equals Bevy's
            // PLACEHOLDER. A declared snapshot identity takes precedence over the
            // empty sentinel; new canonical IDs use from_raw_u32 and avoid it.
            self.0.get(&e).copied().unwrap_or_else(|| {
                assert_eq!(e, Entity::PLACEHOLDER, "Validated closed capture");
                e
            })
        }
        fn set_mapped(&mut self, _: Entity, _: Entity) {}
    }
    let mut mapper = Mapper(mapping);
    let mut values =
        |values: &[Box<dyn PartialReflect>]| -> Result<Vec<Box<dyn PartialReflect>>, String> {
            values
                .iter()
                .map(|v| {
                    let reg = registry
                        .get(type_id(v.as_ref()).ok_or("Missing reflected type")?)
                        .ok_or("Unregistered persisted type")?;
                    let mut concrete = reg
                        .data::<ReflectFromReflect>()
                        .ok_or("Expected FromReflect")?
                        .from_reflect(v.as_ref())
                        .ok_or("Cannot reconstruct persisted value")?;
                    reg.data::<ReflectComponent>()
                        .ok_or("Expected reflected component")?
                        .map_entities(concrete.as_mut(), &mut mapper);
                    Ok(concrete.into_partial_reflect())
                })
                .collect()
        };
    let entities = dynamic
        .entities
        .iter()
        .map(|e| {
            Ok(DynamicEntity {
                entity: mapping[&e.entity],
                components: values(&e.components)?,
            })
        })
        .collect::<Result<_, String>>()?;
    let resources = values(&dynamic.resources)?;
    Ok(DynamicWorld {
        entities,
        resources,
    })
}
fn entity_fields(
    value: &dyn PartialReflect,
    out: &mut HashSet<Entity>,
    depth: usize,
) -> Result<(), String> {
    use bevy::reflect::ReflectRef;
    if depth > 128 {
        return Err("Persisted value nesting limit exceeded".into());
    }
    if let Some(entity) = value.try_downcast_ref::<Entity>() {
        out.insert(*entity);
        return Ok(());
    }
    if value
        .reflect_type_path()
        .starts_with("bevy_asset::handle::Handle<")
    {
        return Err("Persist a source descriptor instead of an asset handle".into());
    }
    let mut visit = |v: &dyn PartialReflect| entity_fields(v, out, depth + 1);
    match value.reflect_ref() {
        ReflectRef::Struct(v) => {
            for (_, field) in v.iter_fields() {
                visit(field)?;
            }
        }
        ReflectRef::TupleStruct(v) => {
            for field in v.iter_fields() {
                visit(field)?;
            }
        }
        ReflectRef::Tuple(v) => {
            for field in v.iter_fields() {
                visit(field)?;
            }
        }
        ReflectRef::List(v) => {
            for field in v.iter() {
                visit(field)?;
            }
        }
        ReflectRef::Array(v) => {
            for field in v.iter() {
                visit(field)?;
            }
        }
        ReflectRef::Map(v) => {
            for (k, v) in v.iter() {
                visit(k)?;
                visit(v)?;
            }
        }
        ReflectRef::Set(v) => {
            for field in v.iter() {
                visit(field)?;
            }
        }
        ReflectRef::Enum(v) => {
            for field in v.iter_fields() {
                visit(field.value())?;
            }
        }
        _ => {}
    }
    Ok(())
}
fn validate<M: TypeData>(
    dynamic: &DynamicWorld,
    registry: &TypeRegistry,
    scope: CaptureScope,
) -> Result<relationships::RelationshipPlan, String> {
    if dynamic.entities.len() > MAX_ENTITIES || dynamic.resources.len() > 128 {
        return Err("Snapshot count limit exceeded".into());
    }
    let ids: HashSet<_> = dynamic.entities.iter().map(|e| e.entity).collect();
    if ids.len() != dynamic.entities.len() {
        return Err("Duplicate snapshot entity".into());
    }
    struct References<'a> {
        ids: &'a HashSet<Entity>,
        bad: bool,
        mapped: HashSet<Entity>,
    }
    impl EntityMapper for References<'_> {
        fn get_mapped(&mut self, source: Entity) -> Entity {
            self.bad |= source != Entity::PLACEHOLDER && !self.ids.contains(&source);
            self.mapped.insert(source);
            source
        }
        fn set_mapped(&mut self, _: Entity, _: Entity) {}
    }
    let mut refs = References {
        ids: &ids,
        bad: false,
        mapped: HashSet::new(),
    };
    let mut metadata = World::new();
    let panel_id = metadata.register_component::<PanelInstance>();
    let mut panel_capable = HashSet::new();
    for (entity, values, is_resource) in dynamic
        .entities
        .iter()
        .map(|e| (Some(e.entity), &e.components, false))
        .chain(std::iter::once((None, &dynamic.resources, true)))
    {
        if values.len() > 128 {
            return Err("Too many persisted component values".into());
        }
        let mut seen = HashSet::new();
        for v in values {
            let info = v
                .get_represented_type_info()
                .ok_or("Missing reflected type")?;
            let id = info.type_id();
            if !seen.insert(id) {
                return Err("Duplicate persisted type".into());
            }
            let reg = registry.get(id).ok_or("Unregistered persisted type")?;
            if reg.data::<M>().is_none() {
                return Err(format!(
                    "Type is not persistence-enabled: {}",
                    info.type_path()
                ));
            }
            if !is_resource && reg.data::<ReflectResource>().is_some() {
                return Err("Resource must be stored in the resource collection".into());
            }
            if is_resource && reg.data::<ReflectResource>().is_none() {
                return Err("Resource is not allowed in this partition".into());
            }
            let component = reg
                .data::<ReflectComponent>()
                .ok_or("Expected reflected component")?;
            let from = reg
                .data::<ReflectFromReflect>()
                .ok_or("Expected FromReflect")?;
            let mut concrete = from
                .from_reflect(v.as_partial_reflect())
                .ok_or("Cannot reconstruct persisted value")?;
            let mut fields = HashSet::new();
            entity_fields(concrete.as_partial_reflect(), &mut fields, 0)?;
            refs.mapped.clear();
            component.map_entities(concrete.as_mut(), &mut refs);
            if !fields.is_subset(&refs.mapped) {
                return Err("Entity fields must opt into Bevy entity mapping".into());
            }
            if let Some(e) = entity {
                let component_id = component.register_component(&mut metadata);
                if metadata
                    .components()
                    .get_info(component_id)
                    .unwrap()
                    .required_components()
                    .iter_ids()
                    .any(|id| id == panel_id)
                {
                    panel_capable.insert(e);
                }
                if matches!(
                    *metadata
                        .components()
                        .get_info(component_id)
                        .unwrap()
                        .clone_behavior(),
                    bevy::ecs::component::ComponentCloneBehavior::Ignore
                ) {
                    return Err("Persisted component opts out of cloning".into());
                }
            }
        }
    }
    if refs.bad {
        return Err("Entity reference leaves the persisted partition".into());
    }
    let relationship_plan = relationships::prepare::<M>(dynamic, registry)?;
    // Relationship targets carry both membership and authoritative order. Sources
    // are rebuilt from them; persisting both would duplicate ownership authority.
    let layouts: HashMap<_, _> = dynamic
        .entities
        .iter()
        .filter_map(|e| value::<Layout>(e).map(|v| (e.entity, v)))
        .collect();
    let areas: HashMap<_, _> = dynamic
        .entities
        .iter()
        .filter_map(|e| value::<Area>(e).map(|v| (e.entity, v)))
        .collect();
    let panels: HashSet<_> = dynamic
        .entities
        .iter()
        .filter(|e| value::<PanelInstance>(e).is_some())
        .map(|e| e.entity)
        .collect();
    if panel_capable.iter().any(|e| !panels.contains(e)) {
        return Err("Concrete panel has no durable panel structure".into());
    }
    let order = if scope == CaptureScope::Layout {
        if !dynamic.resources.is_empty()
            || layouts.len() != 1
            || dynamic
                .entities
                .iter()
                .any(|e| value::<ActiveLayout>(e).is_some())
        {
            return Err(
                "A layout capture needs exactly one layout, no resources or active-layout marker"
                    .into(),
            );
        }
        let root = *layouts.keys().next().unwrap();
        let owned = layouts::owned_entities(dynamic, root, registry)?;
        if owned.len() != dynamic.entities.len() {
            return Err("Layout capture contains entities outside its ownership tree".into());
        }
        vec![root]
    } else {
        resource::<LayoutOrder>(dynamic)
            .map(|v| v.0)
            .unwrap_or_default()
    };
    if order.len() != layouts.len()
        || order.iter().copied().collect::<HashSet<_>>().len() != order.len()
        || order.iter().any(|e| !layouts.contains_key(e))
    {
        return Err("Invalid layout order".into());
    }
    let mut owned = HashSet::new();
    let mut active = false;
    for e in &dynamic.entities {
        let layout = value::<Layout>(e);
        let area = value::<Area>(e);
        let panel = value::<PanelInstance>(e);
        if usize::from(layout.is_some())
            + usize::from(area.is_some())
            + usize::from(panel.is_some())
            > 1
        {
            return Err("Multiple structural roles".into());
        }
        if value::<LayoutAreas>(e).is_some() && layout.is_none()
            || value::<AreaPanels>(e).is_some() && area.is_none()
        {
            return Err("Invalid relationship target owner".into());
        }
        if value::<ActiveLayout>(e).is_some() {
            if layout.is_none() || active {
                return Err("Invalid active layout".into());
            }
            active = true;
        }
        if let Some(ref l) = layout {
            let children = value::<LayoutAreas>(e).unwrap_or_default();
            if l.focused_area
                .is_some_and(|a| !children.entities().contains(&a))
            {
                return Err("Invalid focused area".into());
            }
            let mut geometry = Vec::new();
            for (i, &child) in children.entities().iter().enumerate() {
                let a = areas.get(&child).ok_or("Missing area")?;
                if !owned.insert(child) {
                    return Err("Multiple structural owners".into());
                }
                geometry.push(crate::geometry::Area {
                    id: i as u32 + 1,
                    rect: a.rect,
                });
            }
            if !geometry.is_empty() {
                crate::geometry::Layout::from_areas(geometry)?;
            }
        }
        if let Some(ref a) = area {
            let children = value::<AreaPanels>(e).unwrap_or_default();
            if a.selected
                .is_some_and(|p| !children.entities().contains(&p))
            {
                return Err("Invalid selected panel".into());
            }
            for &child in children.entities() {
                if !panels.contains(&child)
                    || !panel_capable.contains(&child)
                    || !owned.insert(child)
                {
                    return Err("Missing/duplicate/unsupported concrete panel".into());
                }
            }
        }
        if let Some(part) = value::<InPanel>(e) {
            if !panels.contains(&part.0)
                || layout.is_some()
                || area.is_some()
                || panel.is_some()
                || !owned.insert(e.entity)
            {
                return Err("Invalid panel-owned part".into());
            }
        }
    }
    for e in areas.keys().chain(panels.iter()) {
        if !owned.contains(e) {
            return Err("Orphan structural entity".into());
        }
    }
    Ok(relationship_plan)
}

fn insert_capture<M: TypeData>(
    world: &mut World,
    dynamic: DynamicWorld,
    scope: CaptureScope,
) -> Result<Vec<Entity>, String> {
    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry_guard = registry.read();
    let relationship_plan = validate::<M>(&dynamic, &registry_guard, scope)?;
    let graph = dynamic
        .entities
        .iter()
        .any(|e| value::<Layout>(e).is_some());
    let mut map = EntityHashMap::default();
    for e in &dynamic.entities {
        map.insert(e.entity, world.spawn_empty().id());
    }
    let mapping = map.iter().map(|(&a, &b)| (a, b)).collect();
    let mut remapped = clone_mapped(&dynamic, &registry_guard, &mapping)?;
    drop(registry_guard);
    let ordered = if scope == CaptureScope::Layout {
        remapped
            .entities
            .iter()
            .filter(|e| value::<Layout>(e).is_some())
            .map(|e| e.entity)
            .collect()
    } else {
        resource::<LayoutOrder>(&remapped)
            .map(|v| v.0)
            .unwrap_or_default()
    };
    for &layout in &ordered {
        let e = remapped
            .entities
            .iter()
            .find(|e| e.entity == layout)
            .unwrap();
        let mut state = value::<Layout>(e).unwrap();
        state.focused_area = None;
        world.entity_mut(layout).insert(state);
    }
    for e in &remapped.entities {
        if let Some(mut area) = value::<Area>(e) {
            area.selected = None;
            world.entity_mut(e.entity).insert(area);
        }
    }
    // All ordinary entity data exists before any registered relationship hook runs.
    let registry_guard = registry.read();
    for e in &remapped.entities {
        for component in &e.components {
            let id = type_id(component.as_ref()).unwrap();
            if !structural(id)
                && relationships::adapter(&registry_guard, component.as_ref()).is_none()
            {
                registry_guard
                    .get(id)
                    .unwrap()
                    .data::<ReflectComponent>()
                    .unwrap()
                    .insert(
                        &mut world.entity_mut(e.entity),
                        component.as_ref(),
                        &registry_guard,
                    );
            }
        }
    }
    for resource in &remapped.resources {
        let id = type_id(resource.as_ref()).unwrap();
        if structural(id) {
            continue;
        }
        let component = registry_guard
            .get(id)
            .unwrap()
            .data::<ReflectComponent>()
            .unwrap();
        let component_id = component.register_component(world);
        let entity = world
            .resource_entities()
            .get(component_id)
            .unwrap_or_else(|| world.spawn_empty().id());
        component.insert(
            &mut world.entity_mut(entity),
            resource.as_ref(),
            &registry_guard,
        );
    }
    // Target payloads exist before source hooks inspect their parents. Clear only
    // the detached collection for this first insertion; no saved edges exist yet.
    for target in &relationship_plan.targets {
        let entity = mapping[&target.entity];
        let value = remapped
            .entities
            .iter()
            .find(|e| e.entity == entity)
            .unwrap()
            .components
            .iter()
            .find(|v| type_id(v.as_ref()) == Some(target.relationship.target))
            .unwrap();
        target
            .relationship
            .insert_data(world, entity, value.as_ref());
    }
    world.flush();
    for (source, id) in relationship_plan.sources {
        let entity = mapping[&source];
        let value = remapped
            .entities
            .iter()
            .find(|e| e.entity == entity)
            .unwrap()
            .components
            .iter()
            .find(|v| type_id(v.as_ref()) == Some(id))
            .unwrap();
        registry_guard
            .get(id)
            .unwrap()
            .data::<ReflectComponent>()
            .unwrap()
            .insert(
                &mut world.entity_mut(entity),
                value.as_ref(),
                &registry_guard,
            );
    }
    // Generic linked ownership order supplies parent-before-child attachment;
    // each pair supplies its saved source order and target component payload.
    for target in &relationship_plan.targets {
        let entity = mapping[&target.entity];
        let value = remapped
            .entities
            .iter()
            .find(|e| e.entity == entity)
            .unwrap()
            .components
            .iter()
            .find(|v| type_id(v.as_ref()) == Some(target.relationship.target))
            .unwrap();
        target.relationship.attach(world, entity, value.as_ref());
    }
    world.flush();
    for target in &relationship_plan.targets {
        let entity = mapping[&target.entity];
        let value = remapped
            .entities
            .iter()
            .find(|e| e.entity == entity)
            .unwrap()
            .components
            .iter()
            .find(|v| type_id(v.as_ref()) == Some(target.relationship.target))
            .unwrap();
        target.relationship.restore(world, entity, value.as_ref());
    }
    drop(registry_guard);
    for e in &remapped.entities {
        if let Some(panel) = value::<PanelInstance>(e) {
            let mut live = world.get_mut::<PanelInstance>(e.entity).unwrap();
            live.title = panel.title;
            live.preview = panel.preview;
        }
    }
    world.flush();
    for e in &mut remapped.entities {
        if let Some(area) = value::<Area>(e) {
            world.get_mut::<Area>(e.entity).unwrap().selected = area.selected;
        }
        if let Some(layout) = value::<Layout>(e) {
            world.get_mut::<Layout>(e.entity).unwrap().focused_area = layout.focused_area;
        }
        if value::<ActiveLayout>(e).is_some() {
            world.entity_mut(e.entity).insert(ActiveLayout);
        }
    }
    if graph {
        reconcile_workbench(world)?;
    }
    Ok(remapped.entities.iter().map(|e| e.entity).collect())
}

#[cfg(test)]
mod tests;
