use super::*;
use bevy::{
    input::{InputPlugin, keyboard::KeyboardInput, mouse::MouseWheel},
    window::{CursorMoved, WindowEvent, WindowFocused},
};

fn app() -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, InputPlugin, AutomationPlugin::manual()))
        .add_message::<WindowEvent>()
        .add_message::<CursorMoved>()
        .add_message::<WindowFocused>();
    app.world_mut().spawn(Window::default());
    app.update();
    let ready = app.world().resource::<Client>().try_recv().unwrap();
    assert_eq!(ready["event"], "ready");
    app
}
fn call(app: &mut App, value: Value) -> Value {
    app.world().resource::<Client>().submit(value).unwrap();
    for _ in 0..100 {
        app.update();
        if let Some(response) = app.world().resource::<Client>().try_recv() {
            return response;
        }
    }
    panic!("Operation did not complete");
}
#[test]
fn ordered_receipts_wait_for_real_frames_and_bad_requests_do_not_break_the_connection() {
    let mut app = app();
    let client = app.world().resource::<Client>().clone();
    client
        .submit(json!({"id":1,"op":"wait_frames","frames":3}))
        .unwrap();
    client.submit(json!({"id":2,"op":"inspect"})).unwrap();
    app.update();
    app.update();
    app.update();
    assert!(client.try_recv().is_none());
    app.update();
    let first = client.try_recv().unwrap();
    assert_eq!(first["id"], 1);
    assert_eq!(first["result"]["waited"], 3);
    app.update();
    app.update();
    assert_eq!(client.try_recv().unwrap()["id"], 2);
    let invalid = call(
        &mut app,
        json!({"id":3,"op":"input","steps":[{"type":"move","position":["bad",0]}]}),
    );
    assert!(invalid.get("error").is_some());
    assert!(
        call(&mut app, json!({"id":4,"op":"describe"}))["result"]["operations"]
            .as_array()
            .unwrap()
            .contains(&json!("input"))
    );
    assert!(
        call(&mut app, json!({"id":5,"op":"capture","path":"x.png"}))
            .get("error")
            .is_some()
    );
    assert!(
        call(&mut app, json!({"id":6,"op":"describe"}))
            .get("result")
            .is_some()
    );
}
#[test]
fn raw_input_reaches_bevy_and_is_released_at_sequence_completion() {
    #[derive(Resource, Default)]
    struct Observed {
        pressed: bool,
        text: String,
        scrolled: bool,
    }
    let mut app = app();
    app.init_resource::<Observed>().add_systems(
        Update,
        |keys: Res<ButtonInput<KeyCode>>,
         buttons: Res<ButtonInput<MouseButton>>,
         mut keyboard: MessageReader<KeyboardInput>,
         mut wheel: MessageReader<MouseWheel>,
         mut seen: ResMut<Observed>| {
            seen.pressed |= keys.pressed(KeyCode::ShiftLeft) && buttons.pressed(MouseButton::Left);
            for e in keyboard.read() {
                if let Some(text) = &e.text {
                    seen.text.push_str(text);
                }
            }
            seen.scrolled |= wheel.read().next().is_some();
        },
    );
    let result = call(
        &mut app,
        json!({"id":1,"op":"input","steps":[
            {"type":"key","code":"ShiftLeft","pressed":true},
            {"type":"button","button":"Left","pressed":true},
            {"type":"wait","frames":2},
            {"type":"text","text":"hello"},
            {"type":"scroll","delta":[0,-2]}
        ]}),
    );
    assert!(result.get("result").is_some(), "{result}");
    let seen = app.world().resource::<Observed>();
    assert!(seen.pressed);
    assert_eq!(seen.text, "hello");
    assert!(seen.scrolled);
    assert!(
        !app.world()
            .resource::<ButtonInput<KeyCode>>()
            .pressed(KeyCode::ShiftLeft)
    );
    assert!(
        !app.world()
            .resource::<ButtonInput<MouseButton>>()
            .pressed(MouseButton::Left)
    );
}
#[test]
fn input_isolation_preserves_lifecycle_message_ids_and_normal_expiration() {
    #[derive(Resource, Default)]
    struct Resizes(u32);
    let mut app = app();
    app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
        Duration::from_secs_f64(1.0 / 30.0),
    ));
    app.init_resource::<Resizes>().add_systems(
        Update,
        |mut events: MessageReader<WindowEvent>, mut count: ResMut<Resizes>| {
            count.0 += events
                .read()
                .filter(|e| matches!(e, WindowEvent::WindowResized(_)))
                .count() as u32;
        },
    );
    let window = app
        .world_mut()
        .query_filtered::<Entity, With<Window>>()
        .single(app.world())
        .unwrap();
    app.world_mut()
        .write_message(WindowEvent::WindowResized(bevy::window::WindowResized {
            window,
            width: 700.,
            height: 400.,
        }));
    for _ in 0..4 {
        app.update();
    }
    assert_eq!(app.world().resource::<Resizes>().0, 1);
    assert!(app.world().resource::<Messages<WindowEvent>>().is_empty());
}

#[cfg(feature = "ui")]
#[test]
fn injected_modifiers_reach_the_normal_egui_modifier_conversion() {
    use bevy_egui::input::{ModifierKeysState, write_modifiers_keys_state_system};
    #[derive(Resource, Default)]
    struct Seen([bool; 4]);
    let mut app = app();
    app.init_resource::<ModifierKeysState>()
        .init_resource::<Seen>()
        .add_systems(
            PreUpdate,
            write_modifiers_keys_state_system.after(InputSystems),
        )
        .add_systems(
            Update,
            |state: Res<ModifierKeysState>, mut seen: ResMut<Seen>| {
                let modifiers = state.to_egui_modifiers();
                for (seen, pressed) in seen.0.iter_mut().zip([
                    modifiers.shift,
                    modifiers.ctrl,
                    modifiers.alt,
                    modifiers.mac_cmd,
                ]) {
                    *seen |= pressed;
                }
            },
        );
    let result = call(
        &mut app,
        json!({"id":1,"op":"input","steps":[
            {"type":"key","code":"ShiftLeft","pressed":true},
            {"type":"key","code":"ControlRight","pressed":true},
            {"type":"key","code":"AltLeft","pressed":true},
            {"type":"key","code":"SuperRight","pressed":true}
        ]}),
    );
    assert!(result.get("result").is_some(), "{result}");
    let seen = app.world().resource::<Seen>();
    assert!(seen.0[..3].iter().all(|pressed| *pressed));
    if cfg!(target_os = "macos") {
        assert!(seen.0[3]);
    }
    let modifiers = app
        .world()
        .resource::<ModifierKeysState>()
        .to_egui_modifiers();
    assert_eq!(modifiers, Default::default());
}

#[test]
fn timeout_releases_held_input_and_exit_waits_for_its_receipt_to_be_consumed() {
    let mut app = app();
    let client = app.world().resource::<Client>().clone();
    client.submit(json!({"id":1,"op":"input","steps":[{"type":"key","code":"KeyA","pressed":true},{"type":"wait","frames":20}]})).unwrap();
    app.update();
    assert!(
        app.world()
            .resource::<ButtonInput<KeyCode>>()
            .pressed(KeyCode::KeyA)
    );
    app.world_mut()
        .resource_mut::<State>()
        .current
        .as_mut()
        .unwrap()
        .deadline = Instant::now();
    app.update();
    assert!(client.try_recv().unwrap().get("error").is_some());
    assert!(
        !app.world()
            .resource::<ButtonInput<KeyCode>>()
            .pressed(KeyCode::KeyA)
    );
    client.submit(json!({"id":2,"op":"exit"})).unwrap();
    app.update();
    app.update();
    assert!(app.world().resource::<Messages<AppExit>>().is_empty());
    assert_eq!(client.try_recv().unwrap()["result"]["exiting"], true);
    app.update();
    assert!(!app.world().resource::<Messages<AppExit>>().is_empty());
}
#[test]
fn custom_operations_share_the_same_completion_boundary() {
    #[derive(Resource, Default)]
    struct Count(u64);
    let mut app = app();
    app.init_resource::<Count>()
        .register_automation_command("app.increment", |world, args| {
            let count: u64 = required(&args, "amount")?;
            world.resource_mut::<Count>().0 += count;
            Ok(json!(world.resource::<Count>().0))
        });
    assert_eq!(
        call(&mut app, json!({"id":1,"op":"app.increment","amount":7}))["result"],
        7
    );
}

#[cfg(feature = "workbench")]
mod workbench {
    use super::*;
    use bedtime_workbench2::*;
    struct TestPanel;
    impl Panel for TestPanel {
        const KEY: &'static str = "automation.test";
        const LABEL: &'static str = "Test";
        fn draw(
            _: &mut bedtime_ui2::egui::Ui,
            _: &mut World,
            _: &bedtime_ui2::UiContext<'_>,
            _: Entity,
        ) {
        }
    }
    fn app() -> App {
        let mut app = super::app();
        app.add_plugins(WorkbenchCorePlugin)
            .register_panel_type::<TestPanel>();
        app
    }
    #[test]
    fn moving_a_document_view_to_another_workspace_is_rejected_before_reparenting() {
        let mut app = app();
        let first = call(
            &mut app,
            json!({"id":1,"op":"workspace.create","name":"One","activate":true}),
        )["result"]
            .clone();
        let second = call(
            &mut app,
            json!({"id":2,"op":"workspace.create","name":"Two","with_layout":false}),
        )["result"]
            .clone();
        let opened = call(
            &mut app,
            json!({"id":3,"op":"panel.open","key":TestPanel::KEY}),
        )["result"]
            .clone();
        let workspace = entity(app.world(), &first, "workspace").unwrap();
        let layout = entity(app.world(), &first, "layout").unwrap();
        let panel = entity(app.world(), &opened, "panel").unwrap();
        let document = open_document(
            app.world_mut(),
            workspace,
            String::from("Shared"),
            DocumentOptions::loaded("Notes"),
        )
        .unwrap();
        bind_document_view(app.world_mut(), panel, document).unwrap();
        let result = call(
            &mut app,
            json!({"id":4,"op":"layout.move","layout":first["layout"],"workspace":second["workspace"]}),
        );
        assert!(result.get("error").is_some(), "{result}");
        assert_eq!(app.world().get::<InWorkspace>(layout).unwrap().0, workspace);
        assert_eq!(app.world().get::<InWorkspace>(panel).unwrap().0, workspace);
        assert_eq!(read_document(app.world(), document).unwrap(), "Shared");
        assert!(
            call(&mut app, json!({"id":5,"op":"inspect"}))
                .get("result")
                .is_some()
        );
    }

    #[test]
    fn workspace_layout_roundtrips_moves_and_teardown_use_live_application_entities() {
        let mut app = app();
        let first = call(
            &mut app,
            json!({"id":1,"op":"workspace.create","name":"One","activate":true}),
        )["result"]
            .clone();
        let second = call(
            &mut app,
            json!({"id":2,"op":"workspace.create","name":"Two","with_layout":false}),
        )["result"]
            .clone();
        let panel = call(
            &mut app,
            json!({"id":3,"op":"panel.open","key":TestPanel::KEY}),
        )["result"]["panel"]
            .clone();
        assert!(panel.is_string());
        let snapshot = call(&mut app, json!({"id":4,"op":"workspace.capture"}))["result"].clone();
        let restored = call(
            &mut app,
            json!({"id":5,"op":"workspace.restore","snapshot":snapshot,"activate":true}),
        )["result"]["workspace"]
            .clone();
        assert!(restored.is_string());
        assert_eq!(
            call(&mut app, json!({"id":6,"op":"workspace.capture"}))["result"],
            snapshot
        );
        let moved = call(
            &mut app,
            json!({"id":7,"op":"layout.move","layout":first["layout"],"workspace":second["workspace"]}),
        );
        assert!(moved.get("result").is_some(), "{moved}");
        call(
            &mut app,
            json!({"id":8,"op":"workspace.close","workspace":first["workspace"]}),
        );
        let e = entity(app.world(), &json!({"panel":panel}), "panel").unwrap();
        let owner = entity(
            app.world(),
            &json!({"workspace":second["workspace"]}),
            "workspace",
        )
        .unwrap();
        assert_eq!(app.world().get::<InWorkspace>(e).unwrap().0, owner);
        assert!(
            call(
                &mut app,
                json!({"id":9,"op":"panel.focus","panel":"18446744073709551615"})
            )
            .get("error")
            .is_some()
        );
    }
    #[test]
    fn invoke_reports_the_actual_handler_failure() {
        struct Failing;
        impl ContextCommand for Failing {
            const KEY: &'static str = "automation.fail";
            const LABEL: &'static str = "Fail";
        }
        let mut app = app();
        call(
            &mut app,
            json!({"id":1,"op":"workspace.create","name":"Commands","activate":true}),
        );
        Workbench::new(app.world_mut())
            .register_handler::<Failing, _>(
                |_: In<ContextCommandInvocation>| -> ContextCommandResult {
                    Err("Expected handler failure".into())
                },
            )
            .unwrap();
        let response = call(
            &mut app,
            json!({"id":2,"op":"invoke","command":Failing::KEY}),
        );
        assert_eq!(response["error"]["message"], "Expected handler failure");
    }
    #[test]
    fn invalid_restoration_is_rejected_before_creating_entities() {
        let mut app = app();
        let count = app.world().entities().len();
        let response = call(
            &mut app,
            json!({"id":1,"op":"workspace.restore","snapshot":{"version":99,"name":"Bad","background_updates":true,"layouts":[],"active_layout":null}}),
        );
        assert!(response.get("error").is_some());
        assert_eq!(app.world().entities().len(), count);
    }
    #[test]
    fn invalid_configuration_patches_preserve_the_current_components() {
        let mut app = app();
        app.add_plugins(WorkbenchUiPlugin);
        let window = app
            .world_mut()
            .query_filtered::<Entity, With<Window>>()
            .single(app.world())
            .unwrap();
        app.world_mut()
            .entity_mut(window)
            .insert(WorkbenchSurface::default());
        app.world_mut().spawn((Camera2d, WorkbenchCamera));
        app.update();
        let before = *app.world().get::<WorkbenchSurface>(window).unwrap();
        let response = call(
            &mut app,
            json!({"id":1,"op":"surface.configure","settings":{"rounding":12,"typo":true}}),
        );
        assert!(response.get("error").is_some());
        assert_eq!(app.world().get::<WorkbenchSurface>(window), Some(&before));
        let theme = app.world().get::<bedtime_ui2::UsesTheme>(window).unwrap().0;
        let before = app
            .world()
            .get::<bedtime_ui2::Theme>(theme)
            .unwrap()
            .clone();
        let response = call(
            &mut app,
            json!({"id":2,"op":"theme.set","theme":{"palette":{"accent":[2,0,0,1]}}}),
        );
        assert!(response.get("error").is_some());
        assert_eq!(app.world().get::<bedtime_ui2::Theme>(theme), Some(&before));
    }
}
