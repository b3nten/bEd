//! Typed relationship mechanics. The capture marker still decides which values
//! are durable; registration neither captures extra entities nor grants permission.
use super::*;
use bevy::ecs::relationship::{Relationship, RelationshipSourceCollection, RelationshipTarget};
use bevy::reflect::GetTypeRegistration;
use std::collections::BTreeSet;

/// Register a Bevy relationship pair for validated restoration of membership,
/// order and component payloads. Registering the pair does not mark either type.
/// Mark the source to preserve its data; mark the target to preserve collection
/// order/target data. An unmarked source is rebuilt with Relationship::from.
/// Both marked sides must agree about membership. Linked-spawn ownership must be
/// acyclic; non-owning cycles and explicitly allowed self-links are supported.
/// Runtime fields ignored by reflection survive the final saved-state application.
/// Custom hooks must preserve restored membership and defer external activation.
pub fn register_persisted_relationship<R>(app: &mut App)
where
    R: Relationship + FromReflect + GetTypeRegistration,
    R::RelationshipTarget: FromReflect + GetTypeRegistration,
{
    app.register_type::<R>()
        .register_type::<R::RelationshipTarget>();
    let adapter = PersistedRelationship {
        source: TypeId::of::<R>(),
        target: TypeId::of::<R::RelationshipTarget>(),
        linked: R::RelationshipTarget::LINKED_SPAWN,
        allow_self: R::ALLOW_SELF_REFERENTIAL,
        source_entity: |value| {
            R::from_reflect(value)
                .map(|r| r.get())
                .ok_or("Invalid relationship source".into())
        },
        target_entities: |value| {
            R::RelationshipTarget::from_reflect(value)
                .map(|r| r.iter().collect())
                .ok_or("Invalid relationship target".into())
        },
        check_capacity: |sources| {
            let mut collection = <R::RelationshipTarget as RelationshipTarget>::Collection::new();
            for &source in sources {
                if collection.source_to_remove_before_add().is_some() {
                    return Err("Persisted relationships exceed target cardinality".into());
                }
                if !collection.add(source) {
                    return Err("Duplicate persisted relationship source".into());
                }
            }
            if collection.len() != sources.len() {
                return Err("Persisted relationships exceed target cardinality".into());
            }
            Ok(())
        },
        insert_target_data: |world, entity, value| {
            let mut target =
                R::RelationshipTarget::from_reflect(value).expect("Validated relationship target");
            // Only detached data is edited directly. Establish target payloads
            // before source hooks; live membership is built through normal APIs.
            target.collection_mut_risky().clear();
            world.entity_mut(entity).insert(target);
        },
        attach: |world, entity, value| {
            let target =
                R::RelationshipTarget::from_reflect(value).expect("Validated relationship target");
            let sources: Vec<_> = target.iter().collect();
            // Create missing sources while the target's full payload is visible.
            // replace_related temporarily installs an empty default target, so
            // defer reordering until it no longer needs to insert any R values.
            for &source in &sources {
                if world.get::<R>(source).is_none_or(|r| r.get() != entity) {
                    world.entity_mut(source).insert(R::from(entity));
                }
            }
            if !sources.is_empty() {
                world.entity_mut(entity).replace_related::<R>(&sources);
            }
        },
        restore_target: |world, entity, value| {
            let saved =
                R::RelationshipTarget::from_reflect(value).expect("Validated relationship target");
            let mut live = world
                .get_mut::<R::RelationshipTarget>(entity)
                .expect("Restored relationship target");
            let expected: HashSet<_> = saved.iter().collect();
            let actual: HashSet<_> = live.iter().collect();
            assert_eq!(
                actual, expected,
                "Trusted relationship hooks must preserve restored membership"
            );
            // Membership is already established. Applying reflected fields restores
            // durable order/data after attachment maintenance, while preserving
            // ignored runtime fields initialized by ordinary hooks.
            live.apply(value);
        },
    };
    let registry = app.world().resource::<AppTypeRegistry>().clone();
    let mut registry = registry.write();
    registry
        .get_mut(adapter.source)
        .unwrap()
        .insert(adapter.clone());
    registry.get_mut(adapter.target).unwrap().insert(adapter);
}

#[derive(Clone)]
pub(super) struct PersistedRelationship {
    pub source: TypeId,
    pub target: TypeId,
    pub(super) linked: bool,
    allow_self: bool,
    pub(super) source_entity: fn(&dyn PartialReflect) -> Result<Entity, String>,
    target_entities: fn(&dyn PartialReflect) -> Result<Vec<Entity>, String>,
    check_capacity: fn(&[Entity]) -> Result<(), String>,
    insert_target_data: fn(&mut World, Entity, &dyn PartialReflect),
    attach: fn(&mut World, Entity, &dyn PartialReflect),
    restore_target: fn(&mut World, Entity, &dyn PartialReflect),
}
impl PersistedRelationship {
    pub fn children(&self, value: &dyn PartialReflect) -> Result<Vec<Entity>, String> {
        (self.target_entities)(value)
    }
    pub fn insert_data(&self, world: &mut World, entity: Entity, value: &dyn PartialReflect) {
        (self.insert_target_data)(world, entity, value);
    }
    pub fn attach(&self, world: &mut World, entity: Entity, value: &dyn PartialReflect) {
        (self.attach)(world, entity, value);
    }
    pub fn restore(&self, world: &mut World, entity: Entity, value: &dyn PartialReflect) {
        (self.restore_target)(world, entity, value);
    }
}

pub(super) fn adapter<'a>(
    registry: &'a TypeRegistry,
    value: &dyn PartialReflect,
) -> Option<&'a PersistedRelationship> {
    registry
        .get(type_id(value)?)?
        .data::<PersistedRelationship>()
}

/// Ordered target operations, computed and validated before live mutation.
/// Entity references still use the capture's IDs; restore maps them at commit.
pub(super) struct RelationshipPlan {
    pub roots: Vec<Entity>,
    pub sources: Vec<(Entity, TypeId)>,
    pub targets: Vec<TargetPlan>,
}
pub(super) struct TargetPlan {
    pub entity: Entity,
    pub relationship: PersistedRelationship,
}

pub(super) fn prepare<M: TypeData>(
    dynamic: &DynamicWorld,
    registry: &TypeRegistry,
) -> Result<RelationshipPlan, String> {
    let ids: HashSet<_> = dynamic.entities.iter().map(|e| e.entity).collect();
    let mut pairs = HashMap::new();
    for value in dynamic.entities.iter().flat_map(|e| &e.components) {
        if let Some(adapter) = adapter(registry, value.as_ref()) {
            pairs
                .entry(adapter.source)
                .or_insert_with(|| adapter.clone());
        }
    }
    let mut source_components = Vec::new();
    let mut targets = Vec::new();
    let mut ownership = HashSet::new();
    for relationship in pairs.values() {
        let source_marked = registry
            .get(relationship.source)
            .unwrap()
            .data::<M>()
            .is_some();
        let target_marked = registry
            .get(relationship.target)
            .unwrap()
            .data::<M>()
            .is_some();
        let mut sources = HashMap::new();
        let mut orders = HashMap::new();
        for entity in &dynamic.entities {
            for value in &entity.components {
                match type_id(value.as_ref()) {
                    Some(id) if id == relationship.source => {
                        source_components.push((entity.entity, relationship.source));
                        sources
                            .insert(entity.entity, (relationship.source_entity)(value.as_ref())?);
                    }
                    Some(id) if id == relationship.target => {
                        orders.insert(entity.entity, relationship.children(value.as_ref())?);
                        targets.push(TargetPlan {
                            entity: entity.entity,
                            relationship: relationship.clone(),
                        });
                    }
                    _ => {}
                }
            }
        }
        // Each source belongs to one target for this relationship type.
        let mut owners = sources.clone();
        for (&target, children) in &orders {
            let mut seen = HashSet::new();
            for &source in children {
                if !seen.insert(source) {
                    return Err("Duplicate persisted relationship source".into());
                }
                if source_marked && sources.get(&source) != Some(&target) {
                    return Err("Persisted relationship source/target membership disagrees".into());
                }
                if owners
                    .insert(source, target)
                    .is_some_and(|owner| owner != target)
                {
                    return Err("Persisted relationship source has multiple targets".into());
                }
            }
        }
        if target_marked {
            for (&source, &target) in &sources {
                if !orders
                    .get(&target)
                    .is_some_and(|children| children.contains(&source))
                {
                    return Err(
                        "Persisted relationship source is absent from its target collection".into(),
                    );
                }
            }
        }
        let mut grouped: HashMap<Entity, Vec<Entity>> = HashMap::new();
        for (&source, &target) in &owners {
            if !ids.contains(&source) || !ids.contains(&target) {
                return Err("Relationship leaves captured graph".into());
            }
            if source == target && !relationship.allow_self {
                return Err("Self-reference is not allowed for this relationship".into());
            }
            grouped.entry(target).or_default().push(source);
            if relationship.linked {
                ownership.insert((target, source));
            }
        }
        for sources in grouped.values() {
            (relationship.check_capacity)(sources)?;
        }
    }
    // Linked-spawn edges are ownership: restoring a cycle would make recursive
    // cleanup unsafe. Ordinary non-owning relationship cycles remain supported.
    let mut incoming: HashMap<_, usize> = dynamic.entities.iter().map(|e| (e.entity, 0)).collect();
    let mut children: HashMap<Entity, Vec<Entity>> = HashMap::new();
    for (parent, child) in ownership {
        *incoming
            .get_mut(&child)
            .ok_or("Relationship leaves captured graph")? += 1;
        if !incoming.contains_key(&parent) {
            return Err("Relationship leaves captured graph".into());
        }
        children.entry(parent).or_default().push(child);
    }
    let mut ready: BTreeSet<_> = incoming
        .iter()
        .filter_map(|(&e, &count)| (count == 0).then_some(e))
        .collect();
    let roots = ready.iter().copied().collect();
    let mut rank = HashMap::new();
    while let Some(entity) = ready.pop_first() {
        rank.insert(entity, rank.len());
        for child in children.get(&entity).into_iter().flatten() {
            let count = incoming.get_mut(child).unwrap();
            *count -= 1;
            if *count == 0 {
                ready.insert(*child);
            }
        }
    }
    if rank.len() != incoming.len() {
        return Err("Cycle in persisted linked ownership relationships".into());
    }
    source_components.sort_by_key(|(entity, id)| {
        (
            rank[entity],
            registry.get(*id).unwrap().type_info().type_path(),
        )
    });
    targets.sort_by_key(|target| {
        (
            rank[&target.entity],
            registry
                .get(target.relationship.target)
                .unwrap()
                .type_info()
                .type_path(),
        )
    });
    Ok(RelationshipPlan {
        roots,
        sources: source_components,
        targets,
    })
}

#[cfg(test)]
mod tests;
