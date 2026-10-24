//! Experimental scoped DynamicWorld persistence. Not a production session format.
use bedtime_workbench2::*;
use bevy::{
    ecs::entity::EntityHashMap,
    prelude::*,
    reflect::{CreateTypeData, TypeRegistry},
};
use bevy_world_serialization::{
    DynamicWorld, DynamicWorldBuilder, WorldFilter, serde::WorldDeserializer,
};
use serde::de::DeserializeSeed;
use std::{any::TypeId, collections::HashSet};

/// Opt-in reflection type data, attached with #[reflect(WorkbenchPersist)].
#[derive(Clone)]
pub struct ReflectWorkbenchPersist;
impl<T> CreateTypeData<T> for ReflectWorkbenchPersist {
    fn create_type_data(_: ()) -> Self {
        Self
    }
}

// Core workbench types deliberately remain unchanged. These temporary components
// exist only in the snapshot / staging world, not as a second live layout model.
#[derive(Component, Reflect, Clone)]
#[reflect(Component)]
struct WorkspaceState {
    name: String,
    background_updates: bool,
}
#[derive(Component, Reflect, Clone)]
#[reflect(Component)]
struct LayoutState {
    name: String,
    #[entities]
    workspace: Entity,
    #[entities]
    areas: Vec<Entity>,
    #[entities]
    focused: Option<Entity>,
    active: bool,
}
#[derive(Component, Reflect, Clone)]
#[reflect(Component)]
struct AreaState {
    min: [i32; 2],
    max: [i32; 2],
    #[entities]
    panels: Vec<Entity>,
    #[entities]
    selected: Option<Entity>,
}
#[derive(Component, Reflect, Clone)]
#[reflect(Component)]
struct PanelState {
    title: String,
    preview: bool,
}
#[derive(Component, Reflect, Clone)]
#[reflect(Component)]
enum Attachment {
    Workspace(#[entities] Entity),
    Panel(#[entities] Entity),
}

pub fn register(app: &mut App) {
    app.register_type::<WorkspaceState>()
        .register_type::<LayoutState>()
        .register_type::<AreaState>()
        .register_type::<PanelState>()
        .register_type::<Attachment>();
}

fn filter(registry: &TypeRegistry) -> WorldFilter {
    registry
        .iter()
        .filter(|r| r.data::<ReflectWorkbenchPersist>().is_some())
        .fold(WorldFilter::deny_all(), |f, r| f.allow_by_id(r.type_id()))
}

/// None captures global resources and unscoped entities; Some captures one
/// workspace, including its structural entities and other marked owned entities.
pub fn capture(world: &mut World, workspace: Option<Entity>) -> Result<String, String> {
    reconcile_workbench(world).map_err(|e| format!("{e:?}"))?;
    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry = registry.read();
    let entities: Vec<_> = world
        .query::<EntityRef>()
        .iter(world)
        .filter(|e| !e.contains::<bevy::ecs::resource::IsResource>())
        .filter(|e| {
            let owner = if e.contains::<Workspace>() {
                Some(e.id())
            } else {
                e.get::<InWorkspace>().map(|r| r.0)
            };
            owner == workspace
        })
        .map(|e| e.id())
        .collect();
    let builder = DynamicWorldBuilder::from_world(world, &registry)
        .with_component_filter(filter(&registry))
        .with_resource_filter(if workspace.is_none() {
            filter(&registry)
        } else {
            WorldFilter::deny_all()
        })
        .extract_entities(entities.into_iter())
        .extract_resources();
    // Keep empty structural entities; omit unrelated global runtime entities.
    let mut dynamic = builder.build();
    dynamic.entities.retain(|e| {
        !e.components.is_empty()
            || workspace.is_some_and(|w| {
                e.entity == w
                    || world.get::<Layout>(e.entity).is_some()
                    || world.get::<Area>(e.entity).is_some()
                    || world.get::<PanelInstance>(e.entity).is_some()
            })
    });
    for entity in &mut dynamic.entities {
        let e = entity.entity;
        if let Some(w) = world.get::<Workspace>(e) {
            entity.components.push(Box::new(WorkspaceState {
                name: w.name.clone(),
                background_updates: w.background_updates,
            }));
        } else if let Some(l) = world.get::<Layout>(e) {
            entity.components.push(Box::new(LayoutState {
                name: l.name.clone(),
                workspace: world.get::<InWorkspace>(e).unwrap().0,
                areas: world
                    .get::<LayoutAreas>(e)
                    .map_or(Vec::new(), |a| a.entities().to_vec()),
                focused: l.focused_area,
                active: world.get::<ActiveLayout>(e).is_some(),
            }));
        } else if let Some(a) = world.get::<Area>(e) {
            entity.components.push(Box::new(AreaState {
                min: a.rect.min,
                max: a.rect.max,
                panels: panels_in_area(world, e).to_vec(),
                selected: a.selected,
            }));
        } else if let Some(p) = world.get::<PanelInstance>(e) {
            entity.components.push(Box::new(PanelState {
                title: p.title.clone(),
                preview: p.preview,
            }));
        } else if let Some(p) = world.get::<InPanel>(e) {
            entity.components.push(Box::new(Attachment::Panel(p.0)));
        } else if let Some(w) = world.get::<InWorkspace>(e) {
            entity.components.push(Box::new(Attachment::Workspace(w.0)));
        }
    }
    validate_payload(&dynamic, &registry)?;
    dynamic.serialize(&registry).map_err(|e| e.to_string())
}

/// Restrict writes to opted-in types (plus our structural bridge), and reject
/// references outside the partition. Entity fields must use #[entities].
fn validate_payload(dynamic: &DynamicWorld, registry: &TypeRegistry) -> Result<(), String> {
    let ids: HashSet<_> = dynamic.entities.iter().map(|e| e.entity).collect();
    if ids.len() != dynamic.entities.len() {
        return Err("Duplicate entity".into());
    }
    let bridges = [
        TypeId::of::<WorkspaceState>(),
        TypeId::of::<LayoutState>(),
        TypeId::of::<AreaState>(),
        TypeId::of::<PanelState>(),
        TypeId::of::<Attachment>(),
    ];
    struct CheckRefs<'a> {
        ids: &'a HashSet<Entity>,
        bad: bool,
    }
    impl bevy::ecs::entity::EntityMapper for CheckRefs<'_> {
        fn get_mapped(&mut self, source: Entity) -> Entity {
            self.bad |= !self.ids.contains(&source);
            source
        }
        fn set_mapped(&mut self, _: Entity, _: Entity) {}
    }
    let mut refs = CheckRefs {
        ids: &ids,
        bad: false,
    };
    for (values, resource) in dynamic
        .entities
        .iter()
        .map(|e| (&e.components, false))
        .chain(std::iter::once((&dynamic.resources, true)))
    {
        let mut seen = HashSet::new();
        for value in values {
            let info = value
                .get_represented_type_info()
                .ok_or("Missing reflected type")?;
            if !seen.insert(info.type_id()) {
                return Err("Duplicate component/resource type".into());
            }
            let reg = registry.get(info.type_id()).ok_or("Unregistered type")?;
            if reg.data::<ReflectWorkbenchPersist>().is_none()
                && !(bridges.contains(&info.type_id()) && !resource)
            {
                return Err(format!(
                    "Type is not persistence-enabled: {}",
                    info.type_path()
                ));
            }
            if resource && reg.data::<bevy::ecs::reflect::ReflectResource>().is_none() {
                return Err("Expected resource".into());
            }
            let component = reg
                .data::<bevy::ecs::reflect::ReflectComponent>()
                .ok_or("Expected reflected component")?;
            let from = reg
                .data::<bevy::reflect::ReflectFromReflect>()
                .ok_or("Expected FromReflect")?;
            let mut concrete = from
                .from_reflect(value.as_partial_reflect())
                .ok_or("Cannot reconstruct component")?;
            component.map_entities(concrete.as_mut(), &mut refs);
        }
    }
    if refs.bad {
        return Err("Entity reference leaves the persisted partition".into());
    }
    Ok(())
}

// This experiment intentionally uses value-only types, never asset handles.
struct NoAssets;
impl bevy::asset::LoadFromPath for NoAssets {
    fn load_from_path_erased(
        &mut self,
        _: TypeId,
        _: bevy::asset::AssetPath<'static>,
    ) -> bevy::asset::UntypedHandle {
        panic!("Asset handles are outside this prototype; persist a source descriptor instead")
    }
}

pub struct Prepared {
    dynamic: DynamicWorld,
}
/// Parse and validate in a disposable world before retiring any live workspace.
pub fn prepare(
    text: &str,
    registry: &AppTypeRegistry,
    workspace: bool,
) -> Result<Prepared, String> {
    if text.len() > 4 * 1024 * 1024 {
        return Err("Partition exceeds 4 MiB".into());
    }
    let mut deserializer = ron::de::Deserializer::from_str(text).map_err(|e| e.to_string())?;
    let dynamic = WorldDeserializer {
        type_registry: &registry.read(),
        load_from_path: &mut NoAssets,
    }
    .deserialize(&mut deserializer)
    .map_err(|e| e.to_string())?;
    deserializer.end().map_err(|e| e.to_string())?;
    if workspace && !dynamic.resources.is_empty() {
        return Err("Resources belong in the global partition".into());
    }
    validate_payload(&dynamic, &registry.read())?;
    let mut staging = World::new();
    staging.insert_resource(registry.clone());
    dynamic
        .write_to_world(&mut staging, &mut EntityHashMap::default())
        .map_err(|e| e.to_string())?;
    validate_structure(&mut staging, workspace)?;
    Ok(Prepared { dynamic })
}

fn validate_structure(world: &mut World, workspace: bool) -> Result<(), String> {
    let roots: Vec<_> = world
        .query_filtered::<Entity, With<WorkspaceState>>()
        .iter(world)
        .collect();
    if roots.len() != usize::from(workspace) {
        return Err("Wrong partition root count".into());
    }
    if !workspace {
        if world
            .query::<Option<&LayoutState>>()
            .iter(world)
            .any(|v| v.is_some())
            || world.query::<&AreaState>().iter(world).next().is_some()
            || world.query::<&PanelState>().iter(world).next().is_some()
            || world.query::<&Attachment>().iter(world).next().is_some()
        {
            return Err("Scoped structure in global partition".into());
        }
        return Ok(());
    }
    let mut attached = HashSet::from([roots[0]]);
    let mut active = false;
    for (e, layout) in world.query::<(Entity, &LayoutState)>().iter(world) {
        if layout.workspace != roots[0] || layout.active && active {
            return Err("Invalid layout owner/selection".into());
        }
        active |= layout.active;
        attached.insert(e);
        if layout.focused.is_some_and(|e| !layout.areas.contains(&e)) {
            return Err("Invalid focused area".into());
        }
        let mut rects = Vec::new();
        let mut coverage = 0_i64;
        for &area in &layout.areas {
            if !attached.insert(area) {
                return Err("Duplicate area ownership".into());
            }
            let a = world.get::<AreaState>(area).ok_or("Missing area")?;
            if (0..2).any(|i| a.min[i] < 0 || a.max[i] > LAYOUT_EXTENT || a.min[i] >= a.max[i]) {
                return Err("Invalid rectangle".into());
            }
            if rects.iter().any(|(min, max): &([i32; 2], [i32; 2])| {
                (0..2).all(|i| a.min[i] < max[i] && min[i] < a.max[i])
            }) {
                return Err("Overlapping areas".into());
            }
            coverage += i64::from(a.max[0] - a.min[0]) * i64::from(a.max[1] - a.min[1]);
            rects.push((a.min, a.max));
            if a.selected.is_some_and(|e| !a.panels.contains(&e)) {
                return Err("Invalid selected panel".into());
            }
            for &panel in &a.panels {
                if !attached.insert(panel)
                    || world.get::<PanelState>(panel).is_none()
                    || world.get::<PanelInstance>(panel).is_none()
                {
                    return Err("Missing/duplicate concrete panel".into());
                }
            }
        }
        if !rects.is_empty() && coverage != i64::from(LAYOUT_EXTENT).pow(2) {
            return Err("Incomplete tiling".into());
        }
    }
    for (e, attachment) in world.query::<(Entity, &Attachment)>().iter(world) {
        match attachment {
            Attachment::Workspace(w) if *w == roots[0] => {}
            Attachment::Panel(p) if world.get::<PanelState>(*p).is_some() => {}
            _ => return Err("Invalid attachment".into()),
        }
        if !attached.insert(e) {
            return Err("Multiple structural roles".into());
        }
    }
    // Every entity in a workspace partition must have a structural owner.
    if world
        .query::<Entity>()
        .iter(world)
        .filter(|e| {
            !world
                .entity(*e)
                .contains::<bevy::ecs::resource::IsResource>()
        })
        .any(|e| !attached.contains(&e))
    {
        return Err("Orphan entity in workspace partition".into());
    }
    Ok(())
}

/// Caller owns replacement/admission. This appends one already validated partition.
pub fn restore(world: &mut World, prepared: Prepared) -> Result<Vec<Entity>, String> {
    let mut map = EntityHashMap::default();
    prepared
        .dynamic
        .write_to_world(world, &mut map)
        .map_err(|e| e.to_string())?;
    let ids: Vec<_> = prepared
        .dynamic
        .entities
        .iter()
        .map(|e| map[&e.entity])
        .collect();
    // The stock writer skips relationship hooks. Apply actual workbench structure
    // explicitly, parent-first, with hooks enabled and ordering under our control.
    for &e in &ids {
        if let Some(s) = world.get::<WorkspaceState>(e).cloned() {
            world.entity_mut(e).insert(Workspace {
                name: s.name,
                background_updates: s.background_updates,
            });
        }
    }
    for &e in &ids {
        if let Some(s) = world.get::<LayoutState>(e).cloned() {
            world.entity_mut(e).insert((
                Layout {
                    name: s.name,
                    focused_area: None,
                },
                InWorkspace(s.workspace),
            ));
            for area in s.areas {
                world
                    .entity_mut(area)
                    .insert((Area::default(), InLayout(e)));
            }
        }
    }
    for &e in &ids {
        if let Some(s) = world.get::<AreaState>(e).cloned() {
            world.get_mut::<Area>(e).unwrap().rect = AreaRect {
                min: s.min,
                max: s.max,
            };
            for panel in s.panels {
                world.entity_mut(panel).insert(InArea(e));
            }
        }
    }
    for &e in &ids {
        if let Some(s) = world.get::<Attachment>(e).cloned() {
            match s {
                Attachment::Workspace(w) => {
                    world.entity_mut(e).insert(InWorkspace(w));
                }
                Attachment::Panel(p) => {
                    world.entity_mut(e).insert(InPanel(p));
                }
            }
        }
        if let Some(s) = world.get::<PanelState>(e).cloned() {
            let mut p = world.get_mut::<PanelInstance>(e).unwrap();
            p.title = s.title;
            p.preview = s.preview;
        }
    }
    world.flush();
    // Restore remembered selection only after all attachment hooks finish.
    for &e in &ids {
        if let Some(s) = world.get::<AreaState>(e).cloned() {
            world.get_mut::<Area>(e).unwrap().selected = s.selected;
        }
        if let Some(s) = world.get::<LayoutState>(e).cloned() {
            world.get_mut::<Layout>(e).unwrap().focused_area = s.focused;
            if s.active {
                world.entity_mut(e).insert(ActiveLayout);
            }
        }
        world.entity_mut(e).remove::<(
            WorkspaceState,
            LayoutState,
            AreaState,
            PanelState,
            Attachment,
        )>();
    }
    reconcile_workbench(world).map_err(|e| format!("{e:?}"))?;
    Ok(ids)
}
