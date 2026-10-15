use super::*;
use bedtime_desktop2::{Activates, InMenu, Menu, MenuBar, MenuItem};

struct First;
impl CommandDefinition for First {
    const KEY: &'static str = "tests.first";
    const LABEL: &'static str = "First";
}
struct Second;
impl CommandDefinition for Second {
    const KEY: &'static str = "tests.second";
    const LABEL: &'static str = "Second";
}
struct Collision;
impl CommandDefinition for Collision {
    const KEY: &'static str = First::KEY;
    const LABEL: &'static str = "Collision";
}
#[derive(Resource, Default)]
struct Calls(Vec<CommandInvocation>);

fn app() -> App {
    let mut app = App::new();
    app.add_plugins(WorkbenchPlugin).init_resource::<Calls>();
    app
}
fn record(In(invocation): In<CommandInvocation>, mut calls: ResMut<Calls>) -> CommandResult {
    calls.0.push(invocation);
    Ok(())
}
fn results(app: &mut App) -> Vec<CommandFinished> {
    app.world_mut()
        .resource_mut::<Messages<CommandFinished>>()
        .drain()
        .collect()
}

#[test]
fn typed_commands_run_in_submission_order_and_report_handler_errors() {
    let mut app = app();
    let (first, second) = {
        let mut workbench = Workbench::new(app.world_mut());
        let first = workbench.register_command::<First, _>(record).unwrap();
        let second = workbench
            .register_command::<Second, _>(|In(_): In<CommandInvocation>| -> CommandResult {
                Err("expected failure".into())
            })
            .unwrap();
        assert_eq!(workbench.command::<First>(), Some(first));
        (first, second)
    };
    let source = app.world_mut().spawn_empty().id();
    app.world_mut()
        .resource_mut::<CommandRequests>()
        .submit(CommandInvocation {
            command: first,
            source: Some(source),
        });
    app.world_mut()
        .resource_mut::<CommandRequests>()
        .invoke(second);
    app.world_mut()
        .resource_mut::<CommandRequests>()
        .invoke(first);
    app.update();
    assert_eq!(
        app.world().resource::<Calls>().0,
        [
            CommandInvocation {
                command: first,
                source: Some(source)
            },
            CommandInvocation {
                command: first,
                source: None
            }
        ]
    );
    let completed = results(&mut app);
    assert_eq!(
        completed
            .iter()
            .map(|result| result.invocation.command)
            .collect::<Vec<_>>(),
        [first, second, first]
    );
    assert!(completed[0].result.is_ok());
    assert_eq!(completed[1].result, Err("expected failure".into()));
    assert!(completed[2].result.is_ok());
}

#[test]
fn stable_keys_round_trip_and_resolve_a_new_registration() {
    let mut old = app();
    let command = Workbench::new(old.world_mut())
        .register_command::<First, _>(record)
        .unwrap();
    let key = old.world().get::<CommandInfo>(command).unwrap().key();
    let saved = serde_json::to_string(key).unwrap();
    assert_eq!(saved, "\"tests.first\"");
    let loaded: CommandKey = serde_json::from_str(&saved).unwrap();
    let mut new = app();
    new.world_mut().spawn_empty();
    let registered = Workbench::new(new.world_mut())
        .register_command::<First, _>(record)
        .unwrap();
    assert_eq!(
        Workbench::new(new.world_mut()).command_by_key(loaded.as_str()),
        Some(registered)
    );
    assert_eq!(
        Workbench::new(new.world_mut()).command_by_key("missing.command"),
        None
    );
}

#[test]
fn key_collisions_do_not_replace_the_registered_handler() {
    let mut app = app();
    let mut workbench = Workbench::new(app.world_mut());
    let first = workbench.register_command::<First, _>(record).unwrap();
    assert!(workbench.register_command::<First, _>(record).is_err());
    assert!(workbench.register_command::<Collision, _>(record).is_err());
    assert_eq!(workbench.command_by_key(First::KEY), Some(first));
    app.world_mut()
        .resource_mut::<CommandRequests>()
        .invoke(first);
    app.update();
    assert_eq!(app.world().resource::<Calls>().0.len(), 1);
}

#[test]
fn disabled_and_removed_commands_never_run_queued_handlers() {
    let mut app = app();
    let (first, second) = {
        let mut workbench = Workbench::new(app.world_mut());
        (
            workbench.register_command::<First, _>(record).unwrap(),
            workbench.register_command::<Second, _>(record).unwrap(),
        )
    };
    app.world_mut()
        .resource_mut::<CommandRequests>()
        .invoke(first);
    app.world_mut()
        .resource_mut::<CommandRequests>()
        .invoke(second);
    app.world_mut().get_mut::<ActionEnabled>(first).unwrap().0 = false;
    app.world_mut().entity_mut(second).despawn();
    app.update();
    assert!(app.world().resource::<Calls>().0.is_empty());
    assert!(
        results(&mut app)
            .iter()
            .all(|result| result.result.is_err())
    );
    app.world_mut().get_mut::<ActionEnabled>(first).unwrap().0 = true;
    app.world_mut()
        .resource_mut::<CommandRequests>()
        .invoke(first);
    app.update();
    assert_eq!(app.world().resource::<Calls>().0.len(), 1);
}

#[test]
fn menu_activations_use_the_same_dispatcher_and_capture_their_source() {
    let mut app = app();
    let command = Workbench::new(app.world_mut())
        .register_command::<First, _>(record)
        .unwrap();
    let bar = app.world_mut().spawn(MenuBar).id();
    let menu = app
        .world_mut()
        .spawn((
            Menu {
                label: "File".into(),
            },
            InMenu(bar),
        ))
        .id();
    let item = app
        .world_mut()
        .spawn((MenuItem::new("First"), InMenu(menu), Activates(command)))
        .id();
    app.world_mut().write_message(DesktopAction::Activated {
        source: item,
        target: command,
    });
    app.update();
    assert_eq!(
        app.world().resource::<Calls>().0,
        [CommandInvocation {
            command,
            source: Some(item)
        }]
    );
    app.world_mut().entity_mut(menu).despawn();
    assert!(app.world().get::<CommandInfo>(command).is_some());
    app.world_mut()
        .resource_mut::<CommandRequests>()
        .invoke(command);
    app.update();
    assert_eq!(app.world().resource::<Calls>().0.len(), 2);
}

#[test]
fn handlers_enqueue_followup_commands_without_reentrant_execution() {
    let mut app = app();
    let followup = Workbench::new(app.world_mut())
        .register_command::<Second, _>(record)
        .unwrap();
    let first = Workbench::new(app.world_mut())
        .register_command::<First, _>(
            move |In(_): In<CommandInvocation>,
                  mut requests: ResMut<CommandRequests>|
                  -> CommandResult {
                requests.invoke(followup);
                Ok(())
            },
        )
        .unwrap();
    app.world_mut()
        .resource_mut::<CommandRequests>()
        .invoke(first);
    app.update();
    assert!(app.world().resource::<Calls>().0.is_empty());
    assert_eq!(results(&mut app).len(), 1);
    app.update();
    assert_eq!(app.world().resource::<Calls>().0.len(), 1);
    assert_eq!(results(&mut app)[0].invocation.command, followup);
}

#[test]
fn application_owned_native_actions_are_not_treated_as_commands() {
    let mut app = app();
    let source = app.world_mut().spawn_empty().id();
    let target = app.world_mut().spawn_empty().id();
    app.world_mut()
        .write_message(DesktopAction::Activated { source, target });
    app.update();
    assert!(results(&mut app).is_empty());
}

#[test]
fn actions_produced_before_post_update_ingress_execute_in_the_same_frame() {
    let mut app = app();
    let command = Workbench::new(app.world_mut())
        .register_command::<First, _>(record)
        .unwrap();
    let source = app.world_mut().spawn_empty().id();
    app.add_systems(
        PostUpdate,
        (move |mut actions: MessageWriter<DesktopAction>| {
            actions.write(DesktopAction::Activated {
                source,
                target: command,
            });
        })
        .before(CommandSystems::Ingress),
    );
    app.update();
    assert_eq!(
        app.world().resource::<Calls>().0,
        [CommandInvocation {
            command,
            source: Some(source)
        }]
    );
}
