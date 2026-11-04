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

#[bedtime_ui::panel]
#[derive(Default)]
struct Notes;
impl crate::Panel for Notes {
    const LABEL: &'static str = "Notes";
    fn draw(_: &mut bedtime_ui::egui::Ui, _: &mut World, _: &bedtime_ui::UiContext<'_>, _: Entity) {
    }
}

#[bedtime_ui::panel]
#[derive(Default)]
struct Other;
impl crate::Panel for Other {
    const LABEL: &'static str = "Other";
    fn draw(_: &mut bedtime_ui::egui::Ui, _: &mut World, _: &bedtime_ui::UiContext<'_>, _: Entity) {
    }
}

#[test]
fn entries_of_a_panel_show_while_it_is_open_or_focused() {
    use crate::{LayoutsWorldExt, PanelsWorldExt};
    let mut app = app();
    app.add_plugins(bedtime_ui::layouts::LayoutsPlugin);
    let world = app.world_mut();
    let menu = world
        .spawn(Menu {
            label: "Edit".into(),
        })
        .id();
    world.spawn((MenuItem::action::<Copy>(), InMenu(menu)));
    world.spawn((
        MenuItem::action::<Paste>(),
        InMenu(menu),
        ForPanel::of::<Notes>(),
    ));
    let labels = |world: &World| -> Vec<String> {
        world
            .menus()
            .snapshot(menu, InvocationContext::Global)
            .unwrap()
            .entries
            .iter()
            .filter_map(|entry| match entry {
                MenuNode::Item { label, .. } => Some(label.clone()),
                _ => None,
            })
            .collect()
    };
    assert_eq!(labels(world), ["Copy"], "no Notes panel is open");
    let notes = world.open_panel(Notes);
    assert_eq!(labels(world), ["Copy", "Paste"]);

    // One panel's own entry: while it's focused, and gone with it.
    let own = world
        .spawn((
            MenuItem::new("Notes Only"),
            InMenu(menu),
            ForPanel::instance(notes),
        ))
        .id();
    assert_eq!(labels(world), ["Copy", "Paste", "Notes Only"]);
    world.open_panel(Other);
    assert_eq!(labels(world), ["Copy", "Paste"]);
    world.layouts_mut().close_panel(notes).unwrap();
    assert!(world.get_entity(own).is_err());
    assert_eq!(labels(world), ["Copy"]);
}

#[test]
fn the_standard_bar_puts_items_in_their_groups_and_leaves_out_empty_menus() {
    struct Save;
    impl Action for Save {
        const KEY: &'static str = "tests.save";
        const LABEL: &'static str = "Save";
    }
    struct Tidy;
    impl Action for Tidy {
        const KEY: &'static str = "tests.tidy";
        const LABEL: &'static str = "Tidy";
    }
    let mut app = App::new();
    app.add_plugins(ActionsPlugin);
    // Added in plugin order, out of group order.
    app.add_menu_item::<Save>(StandardMenu::File, "save")
        .add_menu_item::<Copy>(StandardMenu::File, "new")
        .add_menu_item::<Paste>(StandardMenu::File, "new")
        .add_menu_item::<Tidy>(StandardMenu::Custom("Tools"), "tidy")
        .add_menu_item::<Copy>(StandardMenu::Edit, "clipboard")
        .hide_menu_item::<Paste>()
        .add_plugins(StandardMenusPlugin { app_name: "Test" });
    app.update();
    let world = app.world_mut();
    let bar = world.resource::<StandardMenus>().bar;
    let snapshot = world
        .menus()
        .snapshot(bar, InvocationContext::Global)
        .unwrap();
    let menus: Vec<&str> = snapshot
        .entries
        .iter()
        .filter_map(|entry| match entry {
            MenuNode::Menu { label, .. } => Some(label.as_str()),
            _ => None,
        })
        .collect();
    // App, Window and Help have nothing: left out. The app's Tools after
    // View (which has workbench's zoom).
    assert_eq!(menus, ["File", "Edit", "View", "Tools"]);
    let MenuNode::Menu { entries: file, .. } = &snapshot.entries[0] else {
        panic!("File is a menu");
    };
    let file: Vec<String> = file
        .iter()
        .map(|entry| match entry {
            MenuNode::Item { label, .. } => label.clone(),
            MenuNode::Separator(_) => "-".into(),
            MenuNode::Menu { label, .. } => label.clone(),
        })
        .collect();
    // Groups in File's order (new, save, close) with lines between, each in
    // the order added (workbench's own after the test's); Paste is hidden.
    assert_eq!(
        file,
        ["Copy", "New", "-", "Save", "Save", "-", "Close Panel"]
    );
}
