use crate::actions::*;
use crate::menus::*;

struct Copy;
impl Action for Copy {
    const KEY: &'static str = "tests.copy";
    const LABEL: &'static str = "Copy";
}

struct Paste;
impl Action for Paste {
    const KEY: &'static str = "tests.paste";
    const LABEL: &'static str = "Paste";
}

fn app() -> App {
    let mut app = App::new();
    app.add_plugins(ActionsPlugin);
    app.add_action_handler::<Copy>(Handler::new(|_, _| Ok(())))
        .add_action_handler::<Paste>(Handler::new(|_, _| Ok(())));
    app
}

fn bind<C: Action>(world: &mut World, chord: KeyChord) {
    world
        .resource_mut::<Keybindings>()
        .set(ActionId::of::<C>(), vec![chord]);
}

#[test]
fn world_menu_snapshots_remain_enabled_without_workspaces() {
    let mut app = App::new();
    app.add_plugins(ActionsPlugin)
        .add_action_handler::<Copy>(Handler::new(|_, _| Ok(())));
    let world = app.world_mut();
    let chord = KeyChord::new(Modifiers::CONTROL, KeyCode::KeyC);
    bind::<Copy>(world, chord);
    let root = world
        .spawn(Menu {
            label: "Edit".into(),
        })
        .id();
    let item = world
        .spawn((MenuItem::new("Copy"), InMenu(root), Invokes::of::<Copy>()))
        .id();
    let snapshot = world
        .menus()
        .snapshot(root, InvocationContext::Focused)
        .unwrap();
    assert!(
        matches!(&snapshot.entries[0], MenuNode::Item { enabled: true, shortcut: Some(c), context: InvocationContext::Global, .. } if *c == chord)
    );
    assert!(snapshot.request(item, ActionId::of::<Copy>()).is_some());
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
    bind::<Copy>(world, chord);
    let snapshot = world
        .menus()
        .snapshot(root, InvocationContext::Global)
        .unwrap();
    assert!(snapshot.request(first, ActionId::of::<Copy>()).is_some());
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
    assert!(
        world
            .actions()
            .available(&ActionId::of::<Copy>(), InvocationContext::Global)
    );
}

#[test]
fn captured_bindings_expire_when_unavailable_detached_retarget_and_removal() {
    let mut app = app();
    let world = app.world_mut();
    let root = world
        .spawn(Menu {
            label: "Context".into(),
        })
        .id();
    let command = ActionId::of::<Copy>();
    let item = world
        .spawn((
            MenuItem::new("Copy"),
            InMenu(root),
            Invokes(command.clone()),
        ))
        .id();
    assert!(
        world
            .menus()
            .snapshot(root, InvocationContext::Global)
            .unwrap()
            .request(item, command.clone())
            .is_some()
    );
    let unavailable = Handler::new(|_, _| Ok(())).available_when(|_, _| false);
    world
        .resource_mut::<ActionRegistry>()
        .set_global_handler::<Copy>(Some(unavailable));
    assert!(
        world
            .menus()
            .snapshot(root, InvocationContext::Global)
            .unwrap()
            .request(item, command.clone())
            .is_none()
    );
    world
        .resource_mut::<ActionRegistry>()
        .set_global_handler::<Copy>(Some(Handler::new(|_, _| Ok(()))));
    world.entity_mut(item).remove::<InMenu>();
    assert!(
        world
            .menus()
            .snapshot(root, InvocationContext::Global)
            .unwrap()
            .request(item, command.clone())
            .is_none()
    );
    world
        .entity_mut(item)
        .insert((InMenu(root), Invokes::of::<Paste>()));
    let snapshot = world
        .menus()
        .snapshot(root, InvocationContext::Global)
        .unwrap();
    assert!(snapshot.request(item, command.clone()).is_none());
    assert!(snapshot.request(item, ActionId::of::<Paste>()).is_some());
    world.despawn(item);
    assert!(
        world
            .menus()
            .snapshot(root, InvocationContext::Global)
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
    assert!(
        world
            .menus()
            .snapshot(root, InvocationContext::Global)
            .is_err()
    );
    world.get_mut::<Menu>(menu).unwrap().label = "File".into();
    world.spawn((MenuItem::new("Not a group"), InMenu(root)));
    assert!(
        world
            .menus()
            .snapshot(root, InvocationContext::Global)
            .is_err()
    );
}
