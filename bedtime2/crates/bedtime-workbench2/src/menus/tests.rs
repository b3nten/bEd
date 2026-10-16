use super::*;
use crate::*;

struct Copy;
impl ContextCommand for Copy {
    const KEY: &'static str = "tests.copy";
    const LABEL: &'static str = "Copy";
}

struct Paste;
impl ContextCommand for Paste {
    const KEY: &'static str = "tests.paste";
    const LABEL: &'static str = "Paste";
}

fn app() -> App {
    let mut app = App::new();
    app.add_plugins(WorkbenchCorePlugin);
    Workbench::new(app.world_mut())
        .register_handler::<Copy, _>(|_: In<ContextCommandInvocation>| -> ContextCommandResult {
            Ok(())
        })
        .unwrap();
    Workbench::new(app.world_mut())
        .register_handler::<Paste, _>(|_: In<ContextCommandInvocation>| -> ContextCommandResult {
            Ok(())
        })
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
    world.spawn(Binding::new::<Copy>(chord));
    let snapshot = menu_snapshot(world, root, InvocationContext::Global).unwrap();
    assert!(
        snapshot
            .request(first, ContextCommandId::of::<Copy>())
            .is_some()
    );
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
        ContextCommandId::of::<Copy>(),
        InvocationContext::Global
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
    let command = ContextCommandId::of::<Copy>();
    let item = world
        .spawn((
            MenuItem::new("Copy"),
            InMenu(root),
            Invokes(command.clone()),
        ))
        .id();
    assert!(
        menu_snapshot(world, root, InvocationContext::Global)
            .unwrap()
            .request(item, command.clone())
            .is_some()
    );
    world.get_mut::<MenuItem>(item).unwrap().enabled = false;
    assert!(
        menu_snapshot(world, root, InvocationContext::Global)
            .unwrap()
            .request(item, command.clone())
            .is_none()
    );
    world.get_mut::<MenuItem>(item).unwrap().enabled = true;
    world.entity_mut(item).remove::<InMenu>();
    assert!(
        menu_snapshot(world, root, InvocationContext::Global)
            .unwrap()
            .request(item, command.clone())
            .is_none()
    );
    world
        .entity_mut(item)
        .insert((InMenu(root), Invokes::of::<Paste>()));
    let snapshot = menu_snapshot(world, root, InvocationContext::Global).unwrap();
    assert!(snapshot.request(item, command.clone()).is_none());
    assert!(
        snapshot
            .request(item, ContextCommandId::of::<Paste>())
            .is_some()
    );
    world.despawn(item);
    assert!(
        menu_snapshot(world, root, InvocationContext::Global)
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
    assert!(menu_snapshot(world, root, InvocationContext::Global).is_err());
    world.get_mut::<Menu>(menu).unwrap().label = "File".into();
    world.spawn((MenuItem::new("Not a group"), InMenu(root)));
    assert!(menu_snapshot(world, root, InvocationContext::Global).is_err());
}

#[test]
fn menu_hints_exclude_shadowed_shortcuts_and_panel_roots_retain_their_context() {
    let mut app = app();
    let world = app.world_mut();
    let mut w = Workbench::new(world);
    let panel = w.create_panel(w.active_layout(), "Target").unwrap();
    let chord = KeyChord::new(Modifiers::PRIMARY, KeyCode::KeyC);
    world.spawn(Binding::new::<Copy>(chord));
    world.spawn((Binding::new::<Paste>(chord), InPanel(panel)));
    let root = world
        .spawn((
            Menu {
                label: "Context".into(),
            },
            InPanel(panel),
        ))
        .id();
    world.spawn((MenuItem::new("Copy"), InMenu(root), Invokes::of::<Copy>()));
    world.spawn((MenuItem::new("Paste"), InMenu(root), Invokes::of::<Paste>()));
    let mut w = Workbench::new(world);
    let other = w.create_panel(w.active_layout(), "Other").unwrap();
    w.focus_panel(other).unwrap();
    let menu = menu_snapshot(world, root, InvocationContext::Focused).unwrap();
    assert!(
        matches!(&menu.entries[0],MenuNode::Item{shortcut:None,context:InvocationContext::Panel(p),..} if *p==panel)
    );
    assert!(
        matches!(&menu.entries[1],MenuNode::Item{shortcut:Some(_),context:InvocationContext::Panel(p),..} if *p==panel)
    );
    Workbench::new(world).close_panel(panel).unwrap();
    assert!(world.get_entity(root).is_err());
}

#[test]
fn panel_owned_bar_entries_follow_focus_and_expire_native_activations() {
    let mut app = app();
    let world = app.world_mut();
    let mut w = Workbench::new(world);
    let layout = w.active_layout();
    let notes = w.create_panel(layout, "Notes").unwrap();
    let other = w.create_panel(layout, "Other").unwrap();
    let root = world.spawn(MenuBar).id();
    let global = world
        .spawn((
            Menu {
                label: "Edit".into(),
            },
            InMenu(root),
        ))
        .id();
    let local = world
        .spawn((
            Menu {
                label: "Notes".into(),
            },
            InMenu(root),
            InPanel(notes),
        ))
        .id();
    let item = world
        .spawn((
            MenuItem::new("Copy note"),
            InMenu(local),
            Invokes::of::<Copy>(),
        ))
        .id();
    let local_item = world
        .spawn((
            MenuItem::new("Panel action"),
            InMenu(global),
            InPanel(notes),
            Invokes::of::<Copy>(),
        ))
        .id();
    Workbench::new(world).focus_panel(notes).unwrap();
    let shown = menu_snapshot(world, root, InvocationContext::Focused).unwrap();
    assert_eq!(shown.entries.len(), 2);
    assert_eq!(
        shown
            .request(item, ContextCommandId::of::<Copy>())
            .unwrap()
            .context,
        InvocationContext::Panel(notes)
    );
    assert!(
        shown
            .request(local_item, ContextCommandId::of::<Copy>())
            .is_some()
    );

    Workbench::new(world).focus_panel(other).unwrap();
    let hidden = menu_snapshot(world, root, InvocationContext::Focused).unwrap();
    assert_eq!(
        hidden.entries.len(),
        1,
        "Panel menu disappears on focus change"
    );
    let MenuNode::Menu { entries, .. } = &hidden.entries[0] else {
        panic!("Edit menu")
    };
    assert!(
        entries.is_empty(),
        "Panel items in global menus disappear too"
    );
    assert!(
        hidden
            .request(item, ContextCommandId::of::<Copy>())
            .is_none(),
        "A native click captured before the focus change must be rejected"
    );

    Workbench::new(world).focus_panel(notes).unwrap();
    assert!(
        menu_snapshot(world, root, InvocationContext::Focused)
            .unwrap()
            .request(item, ContextCommandId::of::<Copy>())
            .is_some()
    );
    Workbench::new(world).close_panel(notes).unwrap();
    assert!(world.get_entity(local).is_err());
    assert!(
        menu_snapshot(world, root, InvocationContext::Focused)
            .unwrap()
            .request(item, ContextCommandId::of::<Copy>())
            .is_none()
    );
}
