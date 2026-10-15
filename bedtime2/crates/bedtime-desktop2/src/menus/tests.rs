use super::*;

fn tree() -> (World, Entity, Entity, Entity) {
    let mut world = World::new();
    let bar = world.spawn(MenuBar).id();
    let menu = world
        .spawn((
            Menu {
                label: "File".into(),
            },
            InMenu(bar),
        ))
        .id();
    let action = world.spawn(ActionEnabled(true)).id();
    (world, bar, menu, action)
}

#[test]
fn menus_can_be_reordered_reparented_and_edited() {
    let (mut world, bar, file, action) = tree();
    let tools = world
        .spawn((
            Menu {
                label: "Tools".into(),
            },
            InMenu(bar),
        ))
        .id();
    let first = world
        .spawn((MenuItem::new("First"), InMenu(file), Activates(action)))
        .id();
    let second = world
        .spawn((MenuItem::new("Second"), InMenu(file), Activates(action)))
        .id();
    let initial = snapshot(&mut world).unwrap();
    assert!(initial.allows(first, action));
    assert!(initial.allows(second, action));
    world
        .entity_mut(file)
        .insert_related::<InMenu>(0, &[second]);
    assert_eq!(
        world.get::<MenuEntries>(file).unwrap().entities(),
        &[second, first]
    );
    world.entity_mut(first).insert(InMenu(tools));
    assert_eq!(
        world.get::<MenuEntries>(file).unwrap().entities(),
        &[second]
    );
    assert_eq!(
        world.get::<MenuEntries>(tools).unwrap().entities(),
        &[first]
    );
    world.get_mut::<MenuItem>(first).unwrap().label = "Renamed".into();
    world.get_mut::<MenuItem>(first).unwrap().shortcut =
        Some(Accelerator::new(Modifiers::CONTROL, Code::KeyR));
    let updated = snapshot(&mut world).unwrap();
    assert_ne!(initial, updated);
    let MenuNode::Menu { entries, .. } = &updated.entries[1] else {
        panic!("Tools menu")
    };
    assert!(
        matches!(&entries[0], MenuNode::Item { label, shortcut: Some(_), .. } if label == "Renamed")
    );
}

#[test]
fn menu_ownership_does_not_own_shared_actions() {
    let (mut world, bar, file, action) = tree();
    let submenu = world
        .spawn((
            Menu {
                label: "More".into(),
            },
            InMenu(file),
        ))
        .id();
    let first = world
        .spawn((MenuItem::new("Save"), InMenu(submenu), Activates(action)))
        .id();
    let other = world
        .spawn((
            Menu {
                label: "Other".into(),
            },
            InMenu(bar),
        ))
        .id();
    let second = world
        .spawn((
            MenuItem::new("Save Again"),
            InMenu(other),
            Activates(action),
        ))
        .id();
    assert_eq!(
        world
            .get::<ActivationSources>(action)
            .unwrap()
            .entities()
            .len(),
        2
    );
    world.entity_mut(file).despawn();
    for entity in [file, submenu, first] {
        assert!(world.get_entity(entity).is_err());
    }
    assert!(world.get_entity(action).is_ok());
    assert!(snapshot(&mut world).unwrap().allows(second, action));
    assert_eq!(
        world.get::<ActivationSources>(action).unwrap().entities(),
        &[second]
    );
    world.entity_mut(action).despawn();
    assert!(world.get_entity(second).is_ok());
    assert!(world.get::<Activates>(second).is_none());
    assert!(!snapshot(&mut world).unwrap().allows(second, action));
}

#[test]
fn native_bindings_expire_on_disable_detach_retarget_and_removal() {
    let (mut world, _bar, menu, action) = tree();
    let item = world
        .spawn((MenuItem::new("Run"), InMenu(menu), Activates(action)))
        .id();
    assert!(snapshot(&mut world).unwrap().allows(item, action));
    world.get_mut::<ActionEnabled>(action).unwrap().0 = false;
    assert!(!snapshot(&mut world).unwrap().allows(item, action));
    world.get_mut::<ActionEnabled>(action).unwrap().0 = true;
    world.get_mut::<MenuItem>(item).unwrap().enabled = false;
    assert!(!snapshot(&mut world).unwrap().allows(item, action));
    world.get_mut::<MenuItem>(item).unwrap().enabled = true;
    world.entity_mut(item).remove::<InMenu>();
    assert!(!snapshot(&mut world).unwrap().allows(item, action));
    let replacement = world.spawn_empty().id();
    world
        .entity_mut(item)
        .insert((InMenu(menu), Activates(replacement)));
    let current = snapshot(&mut world).unwrap();
    assert!(!current.allows(item, action));
    assert!(current.allows(item, replacement));
    world.entity_mut(item).despawn();
    assert!(!snapshot(&mut world).unwrap().allows(item, replacement));
}

#[test]
fn malformed_menu_trees_are_rejected_before_native_construction() {
    let (mut world, bar, menu, _action) = tree();
    world.get_mut::<Menu>(menu).unwrap().label.clear();
    assert!(snapshot(&mut world).is_err());
    world.get_mut::<Menu>(menu).unwrap().label = "File".into();
    world.spawn((MenuItem::new("Not a group"), InMenu(bar)));
    assert!(snapshot(&mut world).is_err());
}
