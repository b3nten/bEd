//! A layout is an owned tree, not a partition of session resources.
use super::*;

/// Editable persistent layout data. No resources or active-layout marker are allowed.
/// Required defaults and panel activation follow the same rules as world restoration.
pub struct CapturedLayout<M = ReflectWorkbenchPersist> {
    pub dynamic: DynamicWorld,
    marker: PhantomData<fn() -> M>,
}

impl Workbench<'_> {
    /// Capture one layout and its persistently declared linked ownership tree.
    /// References leaving that tree are rejected at encode/load/replace boundaries.
    pub fn capture_layout<M: TypeData>(
        world: &mut World,
        layout: Entity,
    ) -> Result<CapturedLayout<M>, String> {
        if world.get::<Layout>(layout).is_none() {
            return Err("Layout is unavailable".into());
        }
        Self::capture_world::<M>(world)?.into_layout(layout, world.resource::<AppTypeRegistry>())
    }

    /// Replace a layout after host admission. Validates before retiring anything;
    /// drafts refuse replacement, while canonical documents and session services remain.
    /// The new root has a fresh identity and retains the target's layout order/activation.
    /// Host-specific root metadata (e.g. numbered slots) belongs to the caller.
    pub fn replace_layout<M: TypeData>(
        world: &mut World,
        target: Entity,
        captured: CapturedLayout<M>,
    ) -> Result<Entity, String> {
        let registry = world.resource::<AppTypeRegistry>().clone();
        validate::<M>(&captured.dynamic, &registry.read(), CaptureScope::Layout)?;
        validate_workbench(world)?;
        let index = layouts(world)
            .iter()
            .position(|&e| e == target)
            .ok_or("Layout is unavailable")?;
        let selected = active_layout(world) == Some(target);
        let panels = Workbench::new(world).panels(target);
        crate::layout::check_panel_drafts(world, panels)?;
        if world
            .get_resource::<ManagedWork>()
            .is_some_and(ManagedWork::is_closing)
        {
            return Err("Session is shutting down".into());
        }
        #[cfg(feature = "ui")]
        if world
            .get_resource::<WorkbenchUi>()
            .is_some_and(|ui| ui.drawing)
        {
            return Err("Cannot replace a layout during workbench drawing".into());
        }
        // Build only after complete validation. Trusted hooks must be pure and
        // external work starts on subsequent schedules, after the old tree retires.
        let entities = insert_capture::<M>(world, captured.dynamic, CaptureScope::Layout)?;
        let root = entities
            .into_iter()
            .find(|&e| world.get::<Layout>(e).is_some())
            .unwrap();
        world.despawn(target);
        world.flush();
        let mut order = world.resource_mut::<LayoutOrder>();
        order.0.retain(|&e| e != root);
        order.0.insert(index, root);
        if selected {
            Workbench::new(world).activate_layout(root)?;
        }
        validate_workbench(world)?;
        Ok(root)
    }
}

impl<M: TypeData> CapturedWorld<M> {
    /// Select a layout after application projection/editing of a world capture.
    /// The result excludes all resources and session activation state.
    pub fn into_layout(
        mut self,
        layout: Entity,
        registry: &AppTypeRegistry,
    ) -> Result<CapturedLayout<M>, String> {
        if self.component::<Layout>(layout).is_none() {
            return Err("Layout is absent from capture".into());
        }
        let registry = registry.read();
        let owned = owned_entities(&self.dynamic, layout, &registry)?;
        self.dynamic.entities.retain(|e| owned.contains(&e.entity));
        for entity in &mut self.dynamic.entities {
            entity
                .components
                .retain(|v| type_id(v.as_ref()) != Some(TypeId::of::<ActiveLayout>()));
        }
        self.dynamic.resources.clear();
        Ok(CapturedLayout {
            dynamic: self.dynamic,
            marker: PhantomData,
        })
    }
}

impl<M: TypeData> CapturedLayout<M> {
    pub fn component<T: Component + FromReflect>(&self, entity: Entity) -> Option<T> {
        self.dynamic
            .entities
            .iter()
            .find(|e| e.entity == entity)
            .and_then(value::<T>)
    }
    pub fn components<T: Component + FromReflect>(&self) -> Vec<(Entity, T)> {
        self.dynamic
            .entities
            .iter()
            .filter_map(|e| value::<T>(e).map(|v| (e.entity, v)))
            .collect()
    }
    pub fn insert_component<T: Component + Reflect>(&mut self, entity: Entity, component: T) {
        let index = match self
            .dynamic
            .entities
            .iter()
            .position(|e| e.entity == entity)
        {
            Some(index) => index,
            None => {
                self.dynamic.entities.push(DynamicEntity {
                    entity,
                    components: Vec::new(),
                });
                self.dynamic.entities.len() - 1
            }
        };
        let entry = &mut self.dynamic.entities[index];
        entry
            .components
            .retain(|v| type_id(v.as_ref()) != Some(TypeId::of::<T>()));
        entry.components.push(Box::new(component));
    }
    /// Remove one panel and its declared persistent descendants from detached
    /// layout data, maintaining tab order and selection. Other references to the
    /// removed data remain errors at the final validation boundary.
    pub fn remove_panel(
        &mut self,
        panel: Entity,
        registry: &AppTypeRegistry,
    ) -> Result<(), String> {
        if self.component::<PanelInstance>(panel).is_none() {
            return Err("Panel is absent from capture".into());
        }
        let owned = owned_entities(&self.dynamic, panel, &registry.read())?;
        for entity in &mut self.dynamic.entities {
            if let Some(mut tabs) = value::<AreaPanels>(entity) {
                let original = tabs.entities().to_vec();
                let remaining: Vec<_> = original
                    .as_slice()
                    .iter()
                    .copied()
                    .filter(|e| !owned.contains(e))
                    .collect();
                use bevy::ecs::relationship::{RelationshipSourceCollection, RelationshipTarget};
                tabs.collection_mut_risky().clear();
                for &panel in &remaining {
                    tabs.collection_mut_risky().add(panel);
                }
                entity
                    .components
                    .retain(|v| type_id(v.as_ref()) != Some(TypeId::of::<AreaPanels>()));
                entity.components.push(Box::new(tabs));
                if let Some(mut area) = value::<Area>(entity) {
                    if area.selected.is_some_and(|e| owned.contains(&e)) {
                        let index = original
                            .as_slice()
                            .iter()
                            .position(|e| Some(*e) == area.selected)
                            .unwrap();
                        area.selected = remaining
                            .get(index.min(remaining.len().saturating_sub(1)))
                            .copied();
                        entity
                            .components
                            .retain(|v| type_id(v.as_ref()) != Some(TypeId::of::<Area>()));
                        entity.components.push(Box::new(area));
                    }
                }
            }
        }
        self.dynamic.entities.retain(|e| !owned.contains(&e.entity));
        Ok(())
    }
    pub fn serialize(&self, registry: &AppTypeRegistry) -> Result<Snapshot, String> {
        encode::<M>(&self.dynamic, registry, CaptureScope::Layout)
    }
    pub fn deserialize(snapshot: &Snapshot, registry: &AppTypeRegistry) -> Result<Self, String> {
        Ok(Self {
            dynamic: decode::<M>(snapshot, registry, CaptureScope::Layout)?,
            marker: PhantomData,
        })
    }
}

/// Follow only declared linked ownership, never shared references or non-owning
/// relationships. Either persisted side can describe an edge.
pub(super) fn owned_entities(
    dynamic: &DynamicWorld,
    root: Entity,
    registry: &TypeRegistry,
) -> Result<HashSet<Entity>, String> {
    let mut children: HashMap<Entity, Vec<Entity>> = HashMap::new();
    for entity in &dynamic.entities {
        for value in &entity.components {
            let Some(adapter) = relationships::adapter(registry, value.as_ref()) else {
                continue;
            };
            if !adapter.linked {
                continue;
            }
            if type_id(value.as_ref()) == Some(adapter.target) {
                children
                    .entry(entity.entity)
                    .or_default()
                    .extend(adapter.children(value.as_ref())?);
            } else {
                let parent = (adapter.source_entity)(value.as_ref())?;
                children.entry(parent).or_default().push(entity.entity);
            }
        }
    }
    let mut owned = HashSet::new();
    let mut pending = vec![root];
    while let Some(entity) = pending.pop() {
        if owned.insert(entity) {
            if owned.len() > MAX_ENTITIES {
                return Err("Snapshot count limit exceeded".into());
            }
            pending.extend(children.get(&entity).into_iter().flatten().copied());
        }
    }
    Ok(owned)
}
