use crate::Workbench;
use super::*;
use bevy::ecs::{lifecycle::HookContext, world::DeferredWorld};

#[derive(Clone)]
struct ReflectProjectData;
impl<T> CreateTypeData<T> for ReflectProjectData {
    fn create_type_data(_: ()) -> Self {
        Self
    }
}
#[derive(Component, Reflect, Clone)]
#[reflect(Component, ProjectData)]
struct Label(String);

#[derive(Component, Reflect, Clone)]
#[reflect(Component, ProjectData)]
#[component(on_insert = member_inserted)]
#[relationship(relationship_target = Members)]
struct MemberOf {
    #[relationship]
    #[entities]
    group: Entity,
    weight: u32,
}
#[derive(Component, Reflect, Clone, Default)]
#[reflect(Component, ProjectData)]
#[relationship_target(relationship = MemberOf, linked_spawn)]
struct Members {
    #[relationship]
    #[entities]
    members: Vec<Entity>,
    caption: String,
    #[entities]
    selected: Option<Entity>,
    #[reflect(ignore)]
    observed_members: usize,
}
fn member_inserted(mut world: DeferredWorld, context: HookContext) {
    let group = world.get::<MemberOf>(context.entity).unwrap().group;
    assert_eq!(world.get::<Label>(group).unwrap().0, "group");
    assert_eq!(
        world.get::<Members>(group).unwrap().caption,
        "saved caption"
    );
    world.get_mut::<Members>(group).unwrap().observed_members += 1;
    let member = context.entity;
    world.commands().queue(move |world: &mut World| {
        world.get_mut::<Members>(group).unwrap().selected = Some(member);
    });
}
fn app() -> App {
    let mut app = App::new();
    app.add_plugins(WorkbenchCorePlugin)
        .register_type::<Label>();
    register_persisted_relationship::<MemberOf>(&mut app);
    app
}
fn fixture(world: &mut World) -> (Entity, Entity, Entity) {
    let group = world
        .spawn((
            Label("group".into()),
            Members {
                caption: "saved caption".into(),
                ..default()
            },
        ))
        .id();
    let first = world
        .spawn((Label("first".into()), MemberOf { group, weight: 10 }))
        .id();
    let second = world
        .spawn((Label("second".into()), MemberOf { group, weight: 20 }))
        .id();
    world.flush();
    world
        .entity_mut(group)
        .replace_related::<MemberOf>(&[second, first]);
    world.get_mut::<Members>(group).unwrap().selected = Some(first);
    (group, first, second)
}
fn capture(world: &mut World) -> CapturedWorld<ReflectProjectData> {
    Workbench::capture_world::<ReflectProjectData>(world).unwrap()
}
fn saved(world: &mut World) -> Snapshot {
    capture(world)
        .serialize(world.resource::<AppTypeRegistry>())
        .unwrap()
}
fn decoded(
    snapshot: &Snapshot,
    world: &World,
) -> Result<CapturedWorld<ReflectProjectData>, String> {
    CapturedWorld::deserialize(snapshot, world.resource::<AppTypeRegistry>())
}
fn assert_rejected_without_mutation(captured: CapturedWorld<ReflectProjectData>, mut target: App) {
    let sentinel = target.world_mut().spawn(Label("existing".into())).id();
    let before = target.world().entities().len();
    assert!(
        captured
            .serialize(target.world().resource::<AppTypeRegistry>())
            .is_err()
    );
    // The decoder independently validates data from storage too.
    let invalid = Snapshot {
        version: VERSION,
        data: captured
            .dynamic
            .serialize(&target.world().resource::<AppTypeRegistry>().read())
            .unwrap(),
    };
    assert!(decoded(&invalid, target.world()).is_err());
    assert!(Workbench::restore_world(target.world_mut(), captured).is_err());
    assert_eq!(target.world().entities().len(), before);
    assert_eq!(target.world().get::<Label>(sentinel).unwrap().0, "existing");
}

#[test]
fn custom_relationships_preserve_order_source_and_target_data_and_remap_references() {
    let mut source = app();
    fixture(source.world_mut());
    let snapshot = saved(source.world_mut());
    let mut captured = decoded(&snapshot, source.world()).unwrap();
    // Neither serialized component order nor entity allocation order is authority.
    captured.dynamic.entities.reverse();
    for entity in &mut captured.dynamic.entities {
        entity.components.reverse();
    }
    let mut target = app();
    for _ in 0..30 {
        target.world_mut().spawn_empty();
    }
    Workbench::restore_world(target.world_mut(), captured).unwrap();
    let (group, members) = target
        .world_mut()
        .query::<(Entity, &Members)>()
        .single(target.world())
        .unwrap();
    assert_eq!(members.caption, "saved caption");
    assert_eq!(
        members.observed_members, 2,
        "Runtime fields survive final durable-state application"
    );
    assert_eq!(
        members
            .members
            .iter()
            .map(|e| target.world().get::<Label>(e).unwrap().0.as_str())
            .collect::<Vec<_>>(),
        ["second", "first"]
    );
    assert_eq!(members.selected, Some(members.members[1]));
    assert_eq!(
        target
            .world()
            .get::<MemberOf>(members.members[0])
            .unwrap()
            .weight,
        20
    );
    assert_eq!(
        target
            .world()
            .get::<MemberOf>(members.members[1])
            .unwrap()
            .weight,
        10
    );
    assert_eq!(
        target
            .world()
            .get::<MemberOf>(members.members[0])
            .unwrap()
            .group,
        group
    );
    assert_eq!(saved(target.world_mut()), snapshot);
}

#[test]
fn inconsistent_forward_and_reverse_membership_is_rejected() {
    let mut source = app();
    let (group, first, second) = fixture(source.world_mut());
    let mut captured = capture(source.world_mut());
    captured.insert_component(
        group,
        Members {
            members: vec![second],
            caption: "saved caption".into(),
            selected: None,
            observed_members: 0,
        },
    );
    assert_rejected_without_mutation(captured, app());
    let mut captured = capture(source.world_mut());
    captured.insert_component(
        first,
        MemberOf {
            group: second,
            weight: 10,
        },
    );
    assert_rejected_without_mutation(captured, app());
}

#[test]
fn duplicate_reverse_members_and_disallowed_self_links_are_rejected() {
    let mut source = app();
    let (group, first, second) = fixture(source.world_mut());
    let mut captured = capture(source.world_mut());
    captured.insert_component(
        group,
        Members {
            members: vec![first, first],
            caption: "saved caption".into(),
            selected: None,
            observed_members: 0,
        },
    );
    assert_rejected_without_mutation(captured, app());
    let mut captured = capture(source.world_mut());
    captured.insert_component(group, MemberOf { group, weight: 0 });
    captured.insert_component(
        group,
        Members {
            members: vec![group, first, second],
            caption: "saved caption".into(),
            selected: None,
            observed_members: 0,
        },
    );
    assert_rejected_without_mutation(captured, app());
}

#[test]
fn linked_ownership_cycles_are_rejected() {
    let mut source = app();
    let (group, first, second) = fixture(source.world_mut());
    let mut captured = capture(source.world_mut());
    captured.insert_component(
        group,
        MemberOf {
            group: first,
            weight: 5,
        },
    );
    captured.insert_component(
        first,
        Members {
            members: vec![group],
            caption: "saved caption".into(),
            selected: None,
            observed_members: 0,
        },
    );
    captured.insert_component(
        group,
        Members {
            members: vec![first, second],
            caption: "saved caption".into(),
            selected: None,
            observed_members: 0,
        },
    );
    assert_rejected_without_mutation(captured, app());
}

#[test]
fn nonowning_cycles_and_explicitly_allowed_self_links_roundtrip() {
    #[derive(Component, Reflect, Clone)]
    #[reflect(Component, ProjectData)]
    #[relationship(relationship_target = Links, allow_self_referential)]
    struct LinksTo(#[entities] Entity);
    #[derive(Component, Reflect, Clone, Default)]
    #[reflect(Component, ProjectData)]
    #[relationship_target(relationship = LinksTo)]
    struct Links(#[entities] Vec<Entity>);
    let mut source = app();
    register_persisted_relationship::<LinksTo>(&mut source);
    let a = source.world_mut().spawn(Label("a".into())).id();
    let b = source.world_mut().spawn(Label("b".into())).id();
    let c = source.world_mut().spawn(Label("self".into())).id();
    source.world_mut().entity_mut(a).insert(LinksTo(b));
    source.world_mut().entity_mut(b).insert(LinksTo(a));
    source.world_mut().entity_mut(c).insert(LinksTo(c));
    let snapshot = saved(source.world_mut());
    let mut target = app();
    register_persisted_relationship::<LinksTo>(&mut target);
    let captured = decoded(&snapshot, target.world()).unwrap();
    Workbench::restore_world(target.world_mut(), captured).unwrap();
    assert_eq!(
        target
            .world_mut()
            .query::<&LinksTo>()
            .iter(target.world())
            .count(),
        3
    );
    assert_eq!(saved(target.world_mut()), snapshot);
}

#[test]
fn one_to_one_relationship_conflicts_are_rejected_before_insertion() {
    // Reflected type paths omit the enclosing function; fixture names must be unique.
    #[derive(Component, Reflect, Clone)]
    #[reflect(Component, ProjectData)]
    #[relationship(relationship_target = ConflictingSlot)]
    struct ConflictingOccupant(#[entities] Entity);
    #[derive(Component, Reflect, Clone)]
    #[reflect(Component)]
    #[relationship_target(relationship = ConflictingOccupant)]
    struct ConflictingSlot(#[entities] Entity);
    let mut source = app();
    register_persisted_relationship::<ConflictingOccupant>(&mut source);
    let root = source.world_mut().spawn(Label("root".into())).id();
    let first = source
        .world_mut()
        .spawn((Label("first".into()), ConflictingOccupant(root)))
        .id();
    let second = source.world_mut().spawn(Label("second".into())).id();
    let mut captured = capture(source.world_mut());
    captured.insert_component(second, ConflictingOccupant(root));
    assert!(
        captured
            .serialize(source.world().resource::<AppTypeRegistry>())
            .is_err()
    );
    let before = source.world().entities().len();
    assert!(Workbench::restore_world(source.world_mut(), captured).is_err());
    assert_eq!(source.world().entities().len(), before);
    assert_eq!(
        source.world().get::<ConflictingSlot>(root).unwrap().0,
        first
    );
}

#[test]
fn old_schema_two_entity_ids_still_remap_when_the_first_id_equals_placeholder() {
    let mut source = app();
    fixture(source.world_mut());
    let snapshot = saved(source.world_mut());
    let captured = decoded(&snapshot, source.world()).unwrap();
    let registry = source.world().resource::<AppTypeRegistry>().clone();
    let mapping = captured
        .dynamic
        .entities
        .iter()
        .enumerate()
        .map(|(i, e)| (e.entity, Entity::from_bits(i as u64 + 1)))
        .collect();
    let legacy = clone_mapped(&captured.dynamic, &registry.read(), &mapping).unwrap();
    assert_eq!(legacy.entities[0].entity, Entity::PLACEHOLDER);
    let legacy = Snapshot {
        version: VERSION,
        data: legacy.serialize(&registry.read()).unwrap(),
    };
    let mut target = app();
    let captured = decoded(&legacy, target.world()).unwrap();
    Workbench::restore_world(target.world_mut(), captured).unwrap();
    assert_eq!(saved(target.world_mut()), snapshot);
}

#[test]
fn empty_and_populated_one_to_one_targets_roundtrip_without_mapping_empty_sentinels() {
    #[derive(Component, Reflect, Clone)]
    #[reflect(Component, ProjectData)]
    #[relationship(relationship_target = Slot)]
    struct UsesSlot(#[entities] Entity);
    #[derive(Component, Reflect, Clone)]
    #[reflect(Component, ProjectData)]
    #[relationship_target(relationship = UsesSlot)]
    struct Slot(#[entities] Entity);
    let mut source = app();
    register_persisted_relationship::<UsesSlot>(&mut source);
    let empty = source
        .world_mut()
        .spawn((Label("empty".into()), Slot(Entity::PLACEHOLDER)))
        .id();
    let populated = source.world_mut().spawn(Label("populated".into())).id();
    source
        .world_mut()
        .spawn((Label("member".into()), UsesSlot(populated)));
    let snapshot = saved(source.world_mut());
    let mut target = app();
    register_persisted_relationship::<UsesSlot>(&mut target);
    let captured = decoded(&snapshot, target.world()).unwrap();
    Workbench::restore_world(target.world_mut(), captured).unwrap();
    assert_eq!(
        target
            .world_mut()
            .query::<&Slot>()
            .iter(target.world())
            .count(),
        2
    );
    assert_eq!(saved(target.world_mut()), snapshot);
    // A sentinel is valid as an empty collection, not as a relationship endpoint.
    let mut invalid = capture(source.world_mut());
    invalid.insert_component(empty, UsesSlot(Entity::PLACEHOLDER));
    assert!(
        invalid
            .serialize(source.world().resource::<AppTypeRegistry>())
            .is_err()
    );
}

#[test]
fn target_only_relationships_restore_in_ownership_order_with_default_source_data() {
    #[derive(Component, Reflect, Clone)]
    #[reflect(Component)]
    #[component(on_insert = assigned)]
    #[relationship(relationship_target = Roster)]
    struct AssignedTo {
        #[relationship]
        #[entities]
        parent: Entity,
        observations: u32,
    }
    #[derive(Component, Reflect, Clone, Default)]
    #[reflect(Component, ProjectData)]
    #[relationship_target(relationship = AssignedTo, linked_spawn)]
    struct Roster {
        #[relationship]
        #[entities]
        members: Vec<Entity>,
        note: String,
    }
    fn assigned(world: DeferredWorld, context: HookContext) {
        let parent = world.get::<AssignedTo>(context.entity).unwrap().parent;
        let label = &world.get::<Label>(parent).unwrap().0;
        if label == "nested" {
            assert!(
                world.get::<AssignedTo>(parent).is_some(),
                "Parent must be attached first"
            );
        }
        assert_eq!(world.get::<Roster>(parent).unwrap().note, "saved");
    }
    let mut source = app();
    register_persisted_relationship::<AssignedTo>(&mut source);
    let root = source
        .world_mut()
        .spawn((
            Label("root".into()),
            Roster {
                members: vec![],
                note: "saved".into(),
            },
        ))
        .id();
    let nested = source
        .world_mut()
        .spawn((
            Label("nested".into()),
            Roster {
                members: vec![],
                note: "saved".into(),
            },
            AssignedTo {
                parent: root,
                observations: 50,
            },
        ))
        .id();
    let first = source
        .world_mut()
        .spawn((
            Label("first".into()),
            AssignedTo {
                parent: nested,
                observations: 60,
            },
        ))
        .id();
    let second = source
        .world_mut()
        .spawn((
            Label("second".into()),
            AssignedTo {
                parent: nested,
                observations: 70,
            },
        ))
        .id();
    source
        .world_mut()
        .entity_mut(nested)
        .replace_related::<AssignedTo>(&[second, first]);
    let snapshot = saved(source.world_mut());
    let mut captured = decoded(&snapshot, source.world()).unwrap();
    captured.dynamic.entities.reverse();
    let mut target = app();
    register_persisted_relationship::<AssignedTo>(&mut target);
    Workbench::restore_world(target.world_mut(), captured).unwrap();
    assert!(
        target
            .world_mut()
            .query::<&AssignedTo>()
            .iter(target.world())
            .all(|r| r.observations == 0)
    );
    assert_eq!(saved(target.world_mut()), snapshot);
    // Target-only collections must still agree about each source's single owner.
    let mut invalid = capture(source.world_mut());
    invalid.insert_component(
        root,
        Roster {
            members: vec![nested, first],
            note: "saved".into(),
        },
    );
    let mut destination = app();
    register_persisted_relationship::<AssignedTo>(&mut destination);
    assert_rejected_without_mutation(invalid, destination);
}

#[test]
fn a_hierarchy_at_the_entity_limit_roundtrips_without_recursive_restore_or_capture() {
    #[derive(Component, Reflect, Clone)]
    #[reflect(Component, ProjectData)]
    #[relationship(relationship_target = ChainChildren)]
    struct ChainParent(#[entities] Entity);
    #[derive(Component, Reflect, Clone, Default)]
    #[reflect(Component, ProjectData)]
    #[relationship_target(relationship = ChainParent, linked_spawn)]
    struct ChainChildren(#[entities] Vec<Entity>);
    let mut source = app();
    register_persisted_relationship::<ChainParent>(&mut source);
    let mut parent = source.world_mut().spawn(Label("root".into())).id();
    for index in 1..MAX_ENTITIES {
        parent = source
            .world_mut()
            .spawn((Label(index.to_string()), ChainParent(parent)))
            .id();
    }
    let snapshot = saved(source.world_mut());
    let mut target = app();
    register_persisted_relationship::<ChainParent>(&mut target);
    let captured = decoded(&snapshot, target.world()).unwrap();
    Workbench::restore_world(target.world_mut(), captured).unwrap();
    assert_eq!(
        target
            .world_mut()
            .query::<&Label>()
            .iter(target.world())
            .count(),
        MAX_ENTITIES
    );
    assert_eq!(saved(target.world_mut()), snapshot);
}
