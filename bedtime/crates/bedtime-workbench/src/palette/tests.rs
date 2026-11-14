//! Below drawing: what the palette lists, and when what's chosen runs.
use super::*;

/// Like an editor command: handled by its panel, available only while the
/// panel has the keyboard.
struct MoveLine;
impl Action for MoveLine {
    const KEY: &'static str = "test.move_line";
    const LABEL: &'static str = "Move Line Down";
}

struct OpenThing;
impl Action for OpenThing {
    const KEY: &'static str = "test.open_thing";
    const LABEL: &'static str = "Open Thing";
}

struct Never;
impl Action for Never {
    const KEY: &'static str = "test.never";
    const LABEL: &'static str = "Never Available";
}

#[derive(Component)]
struct HasKeyboard;

#[derive(Resource, Default)]
struct Focused(Option<Entity>);

#[derive(Resource, Default)]
struct Ran(Vec<(&'static str, Option<Entity>)>);

fn app() -> (App, Entity) {
    let mut app = App::new();
    app.add_plugins((ActionsPlugin, CommandPalettePlugin))
        .init_resource::<Focused>()
        .init_resource::<Ran>()
        .insert_resource(ActionFocus(|world| world.resource::<Focused>().0))
        .register_action::<MoveLine>()
        .add_action_handler::<OpenThing>(Handler::new(|world, invocation| {
            world
                .resource_mut::<Ran>()
                .0
                .push(("open", invocation.entity));
            Ok(())
        }))
        .add_action_handler::<Never>(Handler::new(|_, _| Ok(())).available_when(|_, _| false));
    let panel = app
        .world_mut()
        .spawn((
            HasKeyboard,
            ActionHandler::<MoveLine>::new(
                Handler::new(|world, invocation| {
                    world
                        .resource_mut::<Ran>()
                        .0
                        .push(("move", invocation.entity));
                    Ok(())
                })
                .available_when(|world, panel| {
                    panel.is_some_and(|panel| world.get::<HasKeyboard>(panel).is_some())
                }),
            ),
        ))
        .id();
    app.world_mut().resource_mut::<Focused>().0 = Some(panel);
    (app, panel)
}

fn show_palette(app: &mut App) {
    app.world_mut()
        .resource_mut::<ActionRequests>()
        .invoke::<ShowCommandPalette>();
    app.update();
}

fn listed(app: &mut App) -> Vec<String> {
    app.world_mut()
        .resource_mut::<CommandPalette>()
        .listed()
        .into_iter()
        .map(|command| command.label.clone())
        .collect()
}

fn ran(app: &App) -> Vec<(&'static str, Option<Entity>)> {
    app.world().resource::<Ran>().0.clone()
}

#[test]
fn lists_what_can_run_when_it_opens() {
    let (mut app, panel) = app();
    show_palette(&mut app);
    assert!(app.world().resource::<CommandPalette>().is_open());
    assert_eq!(listed(&mut app), ["Move Line Down", "Open Thing"]);
    // The palette's field takes the keyboard: the list stays as it was.
    app.world_mut().entity_mut(panel).remove::<HasKeyboard>();
    app.update();
    assert_eq!(listed(&mut app), ["Move Line Down", "Open Thing"]);
}

#[test]
fn typing_narrows_the_list() {
    let (mut app, _) = app();
    show_palette(&mut app);
    app.world_mut()
        .resource_mut::<CommandPalette>()
        .set_query("open");
    assert_eq!(listed(&mut app), ["Open Thing"]);
    app.world_mut()
        .resource_mut::<CommandPalette>()
        .set_query("zzz");
    assert!(listed(&mut app).is_empty());
}

#[test]
fn a_chosen_command_runs_for_the_target_once_available() {
    let (mut app, panel) = app();
    show_palette(&mut app);
    app.world_mut().entity_mut(panel).remove::<HasKeyboard>();
    app.world_mut().resource_mut::<CommandPalette>().choose(0);
    assert!(!app.world().resource::<CommandPalette>().is_open());
    app.update();
    app.update();
    assert!(
        ran(&app).is_empty(),
        "waits for the panel to have the keyboard"
    );
    // Focus moves elsewhere meanwhile; it still runs for the panel it was
    // chosen for.
    app.world_mut().resource_mut::<Focused>().0 = None;
    app.world_mut().entity_mut(panel).insert(HasKeyboard);
    app.update();
    assert_eq!(ran(&app), [("move", Some(panel))]);
    app.update();
    assert_eq!(ran(&app).len(), 1, "once");
}

#[test]
fn a_command_that_never_becomes_available_gives_up() {
    let (mut app, panel) = app();
    show_palette(&mut app);
    app.world_mut().entity_mut(panel).remove::<HasKeyboard>();
    app.world_mut().resource_mut::<CommandPalette>().choose(0);
    for _ in 0..=PATIENCE {
        app.update();
    }
    assert!(app.world().resource::<CommandPalette>().closing.is_none());
    let finished: Vec<_> = app
        .world_mut()
        .resource_mut::<Messages<ActionFinished>>()
        .drain()
        .filter(|finished| finished.invocation.action == ActionId::of::<MoveLine>())
        .collect();
    assert_eq!(finished.len(), 1);
    assert!(!finished[0].ran);
    // The panel getting the keyboard later doesn't run it.
    app.world_mut().entity_mut(panel).insert(HasKeyboard);
    app.update();
    assert!(ran(&app).is_empty());
}

#[test]
fn a_global_command_runs_for_the_target_too() {
    let (mut app, panel) = app();
    show_palette(&mut app);
    app.world_mut().resource_mut::<CommandPalette>().choose(1);
    app.update();
    assert_eq!(ran(&app), [("open", Some(panel))]);
}

#[test]
fn dismissing_runs_nothing() {
    let (mut app, _) = app();
    show_palette(&mut app);
    app.world_mut().resource_mut::<CommandPalette>().dismiss();
    for _ in 0..=PATIENCE {
        app.update();
    }
    assert!(ran(&app).is_empty());
    assert!(!app.world().resource::<CommandPalette>().is_open());
}

/// A menu bar with `menu` holding an item per `(label, action)`.
fn menu_bar(app: &mut App, menu: &str, items: &[(&str, ActionId)]) {
    let world = app.world_mut();
    let bar = world.spawn(MenuBar).id();
    let menu = world.spawn((Menu { label: menu.into() }, InMenu(bar))).id();
    for (label, action) in items {
        world.spawn((MenuItem::new(*label), Invokes(action.clone()), InMenu(menu)));
    }
}

#[test]
fn actions_in_menus_are_named_by_them_and_come_first() {
    let (mut app, _) = app();
    menu_bar(&mut app, "View", &[("Thing", ActionId::of::<OpenThing>())]);
    show_palette(&mut app);
    assert_eq!(listed(&mut app), ["View: Thing", "Move Line Down"]);
    // The menu's name matches too, best.
    app.world_mut()
        .resource_mut::<CommandPalette>()
        .set_query("view");
    assert_eq!(listed(&mut app)[0], "View: Thing");
}

#[test]
fn an_action_several_items_run_keeps_its_own_label() {
    let (mut app, _) = app();
    let open = ActionId::of::<OpenThing>();
    menu_bar(
        &mut app,
        "File",
        &[("a.txt", open.clone()), ("b.txt", open)],
    );
    show_palette(&mut app);
    assert_eq!(listed(&mut app), ["File: Open Thing", "Move Line Down"]);
}

#[test]
fn what_was_run_lately_comes_first() {
    let (mut app, _) = app();
    menu_bar(&mut app, "View", &[("Thing", ActionId::of::<OpenThing>())]);
    show_palette(&mut app);
    assert_eq!(listed(&mut app)[1], "Move Line Down");
    app.world_mut().resource_mut::<CommandPalette>().choose(1);
    app.update();
    app.update();
    show_palette(&mut app);
    assert_eq!(listed(&mut app), ["Move Line Down", "View: Thing"]);
}
