use super::*;
use crate::*;

struct Copy;
impl CommandDefinition for Copy {
    const KEY: &'static str = "tests.copy";
    const LABEL: &'static str = "Copy";
}

struct Paste;
impl CommandDefinition for Paste {
    const KEY: &'static str = "tests.paste";
    const LABEL: &'static str = "Paste";
}

fn app() -> App {
    let mut app = App::new();
    app.add_plugins(WorkbenchCorePlugin);
    Workbench::new(app.world_mut())
        .register_command::<Copy, _>(|_: In<CommandInvocation>| -> CommandResult { Ok(()) })
        .unwrap();
    Workbench::new(app.world_mut())
        .register_command::<Paste, _>(|_: In<CommandInvocation>| -> CommandResult { Ok(()) })
        .unwrap();
    app
}

#[test]
fn menus_can_be_edited_reordered_and_reparented_without_native_services() {
    let mut app = app();
    let world = app.world_mut();
    let root = world.spawn(MenuBar).id();
    let file = world
        .spawn((
            Menu {
                label: "File".into(),
            },
            InMenu(root),
        ))
        .id();
    let tools = world
        .spawn((
            Menu {
                label: "Tools".into(),
            },
            InMenu(root),
        ))
        .id();
    let first = world
        .spawn((MenuItem::new("Copy"), InMenu(file), Invokes::of::<Copy>()))
        .id();
    let second = world
        .spawn((
            MenuItem::new("Copy again"),
            InMenu(file),
            Invokes::of::<Copy>(),
        ))
        .id();
    world
        .entity_mut(file)
        .insert_related::<InMenu>(0, &[second]);
    assert_eq!(
        world.get::<MenuEntries>(file).unwrap().entities(),
        &[second, first]
    );
    world.entity_mut(first).insert(InMenu(tools));
    world.get_mut::<MenuItem>(first).unwrap().label = "Renamed".into();
    let chord = KeyChord::new(Modifiers::PRIMARY, KeyCode::KeyC);
    world.spawn(KeyBinding::new::<Copy>(chord));
    let snapshot = menu_snapshot(world, root, CommandContext::Global).unwrap();
    assert!(snapshot.request(first, CommandType::of::<Copy>()).is_some());
    let MenuNode::Menu { entries, .. } = &snapshot.entries[1] else {
        panic!("Tools")
    };
    assert!(
        matches!(&entries[0],MenuNode::Item {label,shortcut:Some(c),..} if label=="Renamed" && c.key==chord.key && c.modifiers==chord.modifiers.resolved())
    );
}

#[test]
fn removing_menu_ownership_keeps_action_definitions_and_handlers() {
    let mut app = app();
    let world = app.world_mut();
    let root = world
        .spawn(Menu {
            label: "Context".into(),
        })
        .id();
    let item = world
        .spawn((MenuItem::new("Copy"), InMenu(root), Invokes::of::<Copy>()))
        .id();
    world.despawn(root);
    assert!(world.get_entity(item).is_err());
    assert!(Workbench::new(world).command::<Copy>().is_some());
    assert!(command_available(
        world,
        CommandType::of::<Copy>(),
        CommandContext::Global
    ));
}

#[test]
fn captured_bindings_expire_on_disable_detach_retarget_and_removal() {
    let mut app = app();
    let world = app.world_mut();
    let root = world
        .spawn(Menu {
            label: "Context".into(),
        })
        .id();
    let command = CommandType::of::<Copy>();
    let item = world
        .spawn((MenuItem::new("Copy"), InMenu(root), Invokes(command)))
        .id();
    assert!(
        menu_snapshot(world, root, CommandContext::Global)
            .unwrap()
            .request(item, command)
            .is_some()
    );
    world.get_mut::<MenuItem>(item).unwrap().enabled = false;
    assert!(
        menu_snapshot(world, root, CommandContext::Global)
            .unwrap()
            .request(item, command)
            .is_none()
    );
    world.get_mut::<MenuItem>(item).unwrap().enabled = true;
    world.entity_mut(item).remove::<InMenu>();
    assert!(
        menu_snapshot(world, root, CommandContext::Global)
            .unwrap()
            .request(item, command)
            .is_none()
    );
    world
        .entity_mut(item)
        .insert((InMenu(root), Invokes::of::<Paste>()));
    let snapshot = menu_snapshot(world, root, CommandContext::Global).unwrap();
    assert!(snapshot.request(item, command).is_none());
    assert!(snapshot.request(item, CommandType::of::<Paste>()).is_some());
    world.despawn(item);
    assert!(
        menu_snapshot(world, root, CommandContext::Global)
            .unwrap()
            .entries
            .is_empty()
    );
}

#[test]
fn malformed_trees_fail_at_the_snapshot_boundary() {
    let mut app = app();
    let world = app.world_mut();
    let root = world.spawn(MenuBar).id();
    let menu = world
        .spawn((
            Menu {
                label: String::new(),
            },
            InMenu(root),
        ))
        .id();
    assert!(menu_snapshot(world, root, CommandContext::Global).is_err());
    world.get_mut::<Menu>(menu).unwrap().label = "File".into();
    world.spawn((MenuItem::new("Not a group"), InMenu(root)));
    assert!(menu_snapshot(world, root, CommandContext::Global).is_err());
}
