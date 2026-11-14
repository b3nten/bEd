use crate::actions::*;
use bevy::prelude::*;

const CTRL_F: KeyChord = KeyChord::new(Modifiers::CONTROL, KeyCode::KeyF);

struct Find;
impl Action for Find {
    const KEY: &'static str = "edit.find";
    const LABEL: &'static str = "Find";
    const DEFAULT_KEYS: &'static [KeyChord] = &[CTRL_F];
}

struct ToggleBackfaces;
impl Action for ToggleBackfaces {
    const KEY: &'static str = "model.toggle_backfaces";
    const LABEL: &'static str = "Toggle Backfaces";
    const DEFAULT_KEYS: &'static [KeyChord] = &[CTRL_F];
}

struct Zoom;
impl Action for Zoom {
    const KEY: &'static str = "view.zoom";
    const LABEL: &'static str = "Zoom";
    const DEFAULT_KEYS: &'static [KeyChord] = &[KeyChord::primary(KeyCode::Equal)];
    const REPEATS: bool = true;
}

/// Who ran: the action, and whether it was the entity's handler.
#[derive(Resource, Default)]
struct Calls(Vec<(&'static str, &'static str, Option<Entity>)>);

fn record(world: &mut World, command: &'static str, scope: &'static str, panel: Option<Entity>) {
    world
        .resource_mut::<Calls>()
        .0
        .push((command, scope, panel));
}

fn find_global(world: &mut World, i: ActionInvocation) -> ActionResult {
    record(world, "find", "global", i.entity);
    Ok(())
}
fn find_in_panel(world: &mut World, i: ActionInvocation) -> ActionResult {
    record(world, "find", "panel", i.entity);
    Ok(())
}
fn toggle_in_panel(world: &mut World, i: ActionInvocation) -> ActionResult {
    record(world, "toggle", "panel", i.entity);
    Ok(())
}
fn zoom(world: &mut World, i: ActionInvocation) -> ActionResult {
    record(world, "zoom", "global", i.entity);
    Ok(())
}

/// The app's idea of focus: the last entity focused.
#[derive(Resource, Default)]
struct Focused(Option<Entity>);

fn app() -> App {
    let mut app = App::new();
    app.add_plugins(ActionsPlugin)
        .init_resource::<Calls>()
        .init_resource::<Focused>()
        .insert_resource(ActionFocus(|world| world.resource::<Focused>().0))
        .register_action::<Find>()
        .register_action::<ToggleBackfaces>()
        .add_action_handler::<Zoom>(Handler::new(zoom));
    app
}

/// A focusable thing, focused.
fn panel(app: &mut App) -> Entity {
    let entity = app.world_mut().spawn_empty().id();
    focus(app, entity);
    entity
}

fn focus(app: &mut App, entity: Entity) {
    app.world_mut().resource_mut::<Focused>().0 = Some(entity);
}

fn press(app: &mut App, chord: KeyChord, repeat: bool) {
    app.world_mut()
        .resource_mut::<BindingInput>()
        .0
        .push(BindingPress {
            chord,
            repeat,
            context: InvocationContext::Focused,
        });
    app.update();
}

fn calls(app: &mut App) -> Vec<(&'static str, &'static str, Option<Entity>)> {
    std::mem::take(&mut app.world_mut().resource_mut::<Calls>().0)
}

fn results(app: &mut App) -> Vec<ActionResult> {
    app.world_mut()
        .resource_mut::<Messages<ActionFinished>>()
        .drain()
        .map(|finished| finished.result)
        .collect()
}

#[test]
fn a_shared_chord_runs_the_command_handled_in_the_most_specific_scope() {
    let mut app = app();
    let editor = panel(&mut app);
    app.world_mut()
        .entity_mut(editor)
        .insert(ActionHandler::<Find>::new(Handler::new(find_in_panel)));
    let viewer = panel(&mut app);
    app.world_mut()
        .entity_mut(viewer)
        .insert(ActionHandler::<ToggleBackfaces>::new(Handler::new(
            toggle_in_panel,
        )));
    let neither = panel(&mut app);

    focus(&mut app, editor);
    press(&mut app, CTRL_F, false);
    assert_eq!(calls(&mut app), [("find", "panel", Some(editor))]);

    focus(&mut app, viewer);
    press(&mut app, CTRL_F, false);
    assert_eq!(calls(&mut app), [("toggle", "panel", Some(viewer))]);

    // Nothing handles either command here, and there's no global handler.
    focus(&mut app, neither);
    press(&mut app, CTRL_F, false);
    assert!(calls(&mut app).is_empty());

    // With a global Find, it runs where no panel claims the chord...
    app.world_mut()
        .resource_mut::<ActionRegistry>()
        .set_global_handler::<Find>(Some(Handler::new(find_global)));
    press(&mut app, CTRL_F, false);
    assert_eq!(calls(&mut app), [("find", "global", Some(neither))]);
    // ...but the viewer's own command still beats it.
    focus(&mut app, viewer);
    press(&mut app, CTRL_F, false);
    assert_eq!(calls(&mut app), [("toggle", "panel", Some(viewer))]);
}

#[test]
fn a_panel_handling_two_commands_on_one_chord_is_a_conflict() {
    let mut app = app();
    let panel = panel(&mut app);
    app.world_mut().entity_mut(panel).insert((
        ActionHandler::<Find>::new(Handler::new(find_in_panel)),
        ActionHandler::<ToggleBackfaces>::new(Handler::new(toggle_in_panel)),
    ));
    press(&mut app, CTRL_F, false);
    assert!(calls(&mut app).is_empty());
    let conflicts: Vec<_> = app
        .world_mut()
        .resource_mut::<Messages<BindingConflict>>()
        .drain()
        .collect();
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].actions.len(), 2);

    // Rebinding one of them resolves it.
    app.world_mut().resource_mut::<Keybindings>().set(
        ActionId::of::<ToggleBackfaces>(),
        vec![KeyChord::new(Modifiers::CONTROL, KeyCode::KeyB)],
    );
    press(&mut app, CTRL_F, false);
    assert_eq!(calls(&mut app), [("find", "panel", Some(panel))]);
}

#[test]
fn a_panel_handler_wins_and_a_disabled_one_blocks_the_global_handler() {
    let mut app = app();
    app.world_mut()
        .resource_mut::<ActionRegistry>()
        .set_global_handler::<Find>(Some(Handler::new(find_global)));
    let panel = panel(&mut app);
    app.world_mut()
        .entity_mut(panel)
        .insert(ActionHandler::<Find>::new(Handler::new(find_in_panel)));
    let invoke = |app: &mut App, context| {
        app.world_mut()
            .resource_mut::<ActionRequests>()
            .submit(ActionRequest {
                action: ActionId::of::<Find>(),
                context,
                source: None,
            });
        app.update();
    };
    invoke(&mut app, InvocationContext::Focused);
    invoke(&mut app, InvocationContext::Global);
    assert_eq!(
        calls(&mut app),
        [("find", "panel", Some(panel)), ("find", "global", None)]
    );

    app.world_mut()
        .get_mut::<ActionHandler<Find>>(panel)
        .unwrap()
        .handler = Handler::new(find_in_panel).available_when(|_, _| false);
    let world = app.world();
    let find = ActionId::of::<Find>();
    assert!(!world.actions().available(&find, InvocationContext::Focused));
    assert!(world.actions().available(&find, InvocationContext::Global));
    invoke(&mut app, InvocationContext::Focused);
    assert!(calls(&mut app).is_empty());
    assert!(results(&mut app).last().unwrap().is_err());

    app.world_mut()
        .entity_mut(panel)
        .remove::<ActionHandler<Find>>();
    invoke(&mut app, InvocationContext::Focused);
    assert_eq!(calls(&mut app), [("find", "global", Some(panel))]);
}

#[test]
fn requests_run_in_order_and_requests_from_handlers_run_next_frame() {
    fn again(world: &mut World, i: ActionInvocation) -> ActionResult {
        record(world, "find", "global", i.entity);
        world.resource_mut::<ActionRequests>().invoke::<Zoom>();
        Err("Not found".into())
    }
    let mut app = app();
    app.world_mut()
        .resource_mut::<ActionRegistry>()
        .set_global_handler::<Find>(Some(Handler::new(again)));
    let mut requests = app.world_mut().resource_mut::<ActionRequests>();
    requests.invoke::<Zoom>();
    requests.invoke::<Find>();
    app.update();
    let names: Vec<_> = calls(&mut app).into_iter().map(|call| call.0).collect();
    assert_eq!(names, ["zoom", "find"]);
    assert_eq!(results(&mut app), [Ok(()), Err("Not found".into())]);
    app.update();
    assert_eq!(calls(&mut app).len(), 1);
}

#[test]
fn held_keys_repeat_only_commands_that_repeat_and_modifiers_match_exactly() {
    let mut app = app();
    let zoom = KeyChord::primary(KeyCode::Equal);
    press(&mut app, zoom.resolved(), false);
    press(&mut app, zoom, true);
    assert_eq!(calls(&mut app).len(), 2);
    press(
        &mut app,
        KeyChord::new(Modifiers::PRIMARY | Modifiers::SHIFT, KeyCode::Equal),
        false,
    );
    assert!(calls(&mut app).is_empty());

    app.world_mut()
        .resource_mut::<ActionRegistry>()
        .set_global_handler::<Find>(Some(Handler::new(find_global)));
    press(&mut app, CTRL_F, true);
    assert!(calls(&mut app).is_empty());
}

#[test]
fn menu_hints_show_the_chord_that_would_run_the_command() {
    let mut app = app();
    let panel = panel(&mut app);
    app.world_mut()
        .entity_mut(panel)
        .insert(ActionHandler::<ToggleBackfaces>::new(Handler::new(
            toggle_in_panel,
        )));
    app.world_mut()
        .resource_mut::<ActionRegistry>()
        .set_global_handler::<Find>(Some(Handler::new(find_global)));
    let world = app.world();
    let find = ActionId::of::<Find>();
    // Ctrl+F toggles backfaces in this panel, so Find shows no shortcut here.
    assert_eq!(
        world
            .actions()
            .shortcut_for(&find, InvocationContext::Focused),
        None
    );
    assert_eq!(
        world
            .actions()
            .shortcut_for(&find, InvocationContext::Global),
        Some(CTRL_F)
    );
}

#[test]
#[should_panic(expected = "Two actions use the key")]
fn two_commands_cannot_share_a_key() {
    struct Impostor;
    impl Action for Impostor {
        const KEY: &'static str = Find::KEY;
        const LABEL: &'static str = "Impostor";
    }
    app().register_action::<Impostor>();
}

#[test]
fn chords_read_and_write_as_portable_text() {
    for text in [
        "Primary+S",
        "Primary+Shift+Z",
        "Ctrl+Alt+Enter",
        "F5",
        "Meta+1",
    ] {
        let chord: KeyChord = text.parse().unwrap();
        assert_eq!(chord.to_string(), text);
    }
    assert_eq!(
        "cmd+shift+z".parse::<KeyChord>().unwrap(),
        KeyChord::new(Modifiers::META | Modifiers::SHIFT, KeyCode::KeyZ)
    );
    assert!("Primary+".parse::<KeyChord>().is_err());
    assert!("Hyper+S".parse::<KeyChord>().is_err());
    assert!("Primary+Banana".parse::<KeyChord>().is_err());
    assert_eq!(
        ron::to_string(&KeyChord::primary(KeyCode::KeyS)).unwrap(),
        "\"Primary+S\""
    );
}

#[cfg(feature = "persistence")]
mod keybinding_settings {
    use super::*;
    use crate::settings::{SettingsAppExt, SettingsPlugin, SettingsWorldExt};
    use bedtime_persistence::{Store, StoreId};

    struct GlobalStore;
    impl Store for GlobalStore {}
    struct WorkbenchStore;
    impl Store for WorkbenchStore {}

    #[test]
    fn a_layer_replaces_a_commands_keys_and_an_empty_list_unbinds_it() {
        let mut app = App::new();
        app.add_plugins((SettingsPlugin, ActionsPlugin))
            .init_resource::<Calls>()
            .register_action::<Find>()
            .add_settings_layer::<GlobalStore>()
            .add_settings_layer::<WorkbenchStore>();
        app.update();
        let (global, workspace) = (
            StoreId::of::<GlobalStore>(),
            StoreId::of::<WorkbenchStore>(),
        );
        let find = ActionId::of::<Find>();
        let keys = |app: &App| app.world().resource::<Keybindings>().keys(&find).to_vec();
        let ctrl = |key| KeyChord::new(Modifiers::CONTROL, key);
        assert_eq!(keys(&app), [CTRL_F]);

        let world = app.world_mut();
        world
            .settings_mut()
            .set::<Keybinding<Find>>(global, vec![ctrl(KeyCode::KeyB)])
            .unwrap();
        world
            .settings_mut()
            .set::<Keybinding<Find>>(workspace, vec![ctrl(KeyCode::KeyD)])
            .unwrap();
        assert_eq!(keys(&app), [ctrl(KeyCode::KeyD)], "not both");

        let world = app.world_mut();
        world
            .settings_mut()
            .set::<Keybinding<Find>>(workspace, vec![])
            .unwrap();
        assert!(keys(&app).is_empty());
        app.world_mut()
            .settings_mut()
            .clear::<Keybinding<Find>>(workspace)
            .unwrap();
        assert_eq!(keys(&app), [ctrl(KeyCode::KeyB)]);

        // The same, for UIs that only have the command's id.
        assert_eq!(app.world().actions().keys_in(&find, workspace), None);
        app.world_mut()
            .actions_mut()
            .set_keys_in(&find, workspace, Some(vec![CTRL_F]))
            .unwrap();
        assert_eq!(
            app.world().actions().keys_in(&find, workspace),
            Some(vec![CTRL_F])
        );
        assert_eq!(keys(&app), [CTRL_F]);
        app.world_mut()
            .actions_mut()
            .set_keys_in(&find, global, None)
            .unwrap();
        assert_eq!(app.world().actions().keys_in(&find, global), None);
    }

    #[test]
    fn a_keymap_is_the_default_layers_change_and_clearing_returns_to() {
        const ALT_F: &[KeyChord] = &[KeyChord::new(Modifiers::ALT, KeyCode::KeyF)];
        let mut app = App::new();
        app.add_plugins((SettingsPlugin, ActionsPlugin))
            .init_resource::<Calls>()
            .add_keymap(&[("edit.find", ALT_F)])
            .register_action::<Find>()
            .add_settings_layer::<GlobalStore>();
        app.update();
        let find = ActionId::of::<Find>();
        let keys = |app: &App| app.world().resource::<Keybindings>().keys(&find).to_vec();
        assert_eq!(keys(&app), ALT_F, "not its own Ctrl+F");
        let global = StoreId::of::<GlobalStore>();
        app.world_mut()
            .settings_mut()
            .set::<Keybinding<Find>>(global, vec![CTRL_F])
            .unwrap();
        assert_eq!(keys(&app), [CTRL_F]);
        app.world_mut()
            .settings_mut()
            .clear::<Keybinding<Find>>(global)
            .unwrap();
        assert_eq!(keys(&app), ALT_F);
    }
}

#[cfg(feature = "ui")]
#[test]
fn requests_made_while_drawing_run_in_the_same_frame() {
    let mut app = App::new();
    app.add_plugins(ActionsPlugin)
        .init_resource::<Calls>()
        .add_action_handler::<Zoom>(Handler::new(zoom))
        .add_systems(
            PreUpdate,
            (|world: &mut World| {
                world.resource_mut::<ActionRequests>().invoke::<Zoom>();
            })
            .in_set(bedtime_ui::UiSystems::Draw),
        );
    app.update();
    assert_eq!(calls(&mut app).len(), 1);
}
