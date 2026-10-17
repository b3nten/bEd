use super::*;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Resource, Default)]
struct Counter(u32);
struct Count;
impl ContextCommand for Count {
    const KEY: &'static str = "workspace.count";
    const LABEL: &'static str = "Count";
}
fn counter(app: &mut App) {
    app.init_resource::<Counter>()
        .add_systems(Update, |mut n: ResMut<Counter>| n.0 += 1);
    Workbench::new(app.world_mut())
        .register_handler::<Count, _>(
            |_: In<ContextCommandInvocation>, mut n: ResMut<Counter>| -> ContextCommandResult {
                n.0 += 100;
                Ok(())
            },
        )
        .unwrap();
}
fn host() -> App {
    let mut app = App::new();
    app.add_plugins(WorkbenchCorePlugin);
    app
}
fn open(app: &mut App, name: &str) -> WorkspaceId {
    let mut runtime = WorkspaceRuntime::new(name);
    runtime.add_plugins(counter);
    Workspaces::new(app.world_mut()).open(runtime)
}
fn count(app: &mut App, id: WorkspaceId) -> u32 {
    Workbench::with_workspace(app.world_mut(), id)
        .unwrap()
        .world()
        .resource::<Counter>()
        .0
}
fn eventually(app: &mut App, mut condition: impl FnMut(&mut App) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition(app) {
        assert!(
            Instant::now() < deadline,
            "Workspace cleanup did not finish"
        );
        app.update();
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn resources_entities_and_queued_commands_stay_in_their_originating_world() {
    let mut app = host();
    let a = open(&mut app, "A");
    let b = open(&mut app, "B");
    Workspaces::new(app.world_mut()).activate(a).unwrap();
    let panel_a = {
        let mut w = Workbench::new(app.world_mut());
        let layout = w.active_layout();
        let panel = w.create_panel(layout, "A only").unwrap();
        w.world_mut()
            .resource_mut::<ContextCommandRequests>()
            .invoke::<Count>();
        panel
    };
    Workspaces::new(app.world_mut()).activate(b).unwrap();
    assert_eq!(Workbench::new(app.world_mut()).workspace_id(), b);
    app.update();
    assert_eq!(count(&mut app, a), 101);
    assert_eq!(count(&mut app, b), 1);
    assert!(app.world().get::<Panel>(panel_a).is_none());
    assert!(
        Workbench::with_workspace(app.world_mut(), b)
            .unwrap()
            .world()
            .get::<Panel>(panel_a)
            .is_none()
    );
    Workspaces::new(app.world_mut()).activate(a).unwrap();
    assert_eq!(
        Workbench::new(app.world_mut())
            .world()
            .get::<Panel>(panel_a)
            .unwrap()
            .title,
        "A only"
    );
}

#[test]
fn suspended_workspaces_keep_state_and_startup_runs_once() {
    let mut app = host();
    let a = open(&mut app, "A");
    let b = open(&mut app, "B");
    Workspaces::new(app.world_mut()).activate(a).unwrap();
    Workspaces::new(app.world_mut())
        .set_background_updates(b, false)
        .unwrap();
    app.update();
    app.update();
    assert_eq!(count(&mut app, a), 2);
    assert_eq!(count(&mut app, b), 0);
    Workspaces::new(app.world_mut()).activate(b).unwrap();
    app.update();
    assert_eq!(count(&mut app, b), 1);
}

#[test]
fn closing_requires_a_live_replacement_and_preserves_the_active_invariant() {
    let mut app = host();
    let a = Workspaces::new(app.world_mut()).active();
    assert!(Workspaces::new(app.world_mut()).close(a, None).is_err());
    assert!(Workspaces::new(app.world_mut()).close(a, Some(a)).is_err());
    let b = open(&mut app, "B");
    let c = open(&mut app, "C");
    Workspaces::new(app.world_mut()).close(c, None).unwrap();
    assert!(Workspaces::new(app.world_mut()).close(a, Some(c)).is_err());
    assert_eq!(Workspaces::new(app.world_mut()).active(), a);
    Workspaces::new(app.world_mut()).close(a, Some(b)).unwrap();
    assert_eq!(Workbench::new(app.world_mut()).workspace_id(), b);
    assert!(Workbench::with_workspace(app.world_mut(), a).is_err());
    app.update();
    assert!(app.world().get_entity(a.0).is_err());
    assert_eq!(count(&mut app, b), 1);
    let reopened = open(&mut app, "Reopened");
    assert_ne!(a, reopened);
    assert!(Workbench::with_workspace(app.world_mut(), a).is_err());
}

#[test]
fn closing_keeps_resources_alive_until_work_acknowledges_cancellation() {
    #[derive(Resource)]
    struct Probe(Arc<AtomicBool>);
    impl Drop for Probe {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }
    let mut app = host();
    let id = open(&mut app, "Closing");
    let dropped = Arc::new(AtomicBool::new(false));
    let (release, wait) = std::sync::mpsc::channel();
    {
        let mut w = Workbench::with_workspace(app.world_mut(), id).unwrap();
        w.world_mut().insert_resource(Probe(dropped.clone()));
        w.work()
            .spawn_task(None, "cleanup acknowledgement", move |cancel| {
                cancel.wait(Duration::from_secs(30));
                wait.recv().unwrap();
            })
            .unwrap();
    }
    Workspaces::new(app.world_mut()).close(id, None).unwrap();
    app.update();
    assert!(!dropped.load(Ordering::Acquire));
    assert!(app.world().get_entity(id.0).is_ok());
    assert!(
        Workspaces::new(app.world_mut())
            .list()
            .iter()
            .find(|w| w.id == id)
            .unwrap()
            .closing
    );
    release.send(()).unwrap();
    eventually(&mut app, |app| app.world().get_entity(id.0).is_err());
    assert!(dropped.load(Ordering::Acquire));
}

#[test]
fn work_cancels_when_its_entity_or_workspace_closes_without_blocking_the_host() {
    let mut app = host();
    let a = open(&mut app, "A");
    let b = open(&mut app, "B");
    let stopped = Arc::new(AtomicBool::new(false));
    let task = {
        let mut w = Workbench::with_workspace(app.world_mut(), a).unwrap();
        let owner = w.world_mut().spawn_empty().id();
        let seen = stopped.clone();
        let task = w
            .work()
            .spawn_task(Some(owner), "owned task", move |cancel| {
                cancel.wait(Duration::from_secs(30));
                seen.store(true, Ordering::Release);
            })
            .unwrap();
        w.world_mut().despawn(owner);
        task
    };
    eventually(&mut app, |_| stopped.load(Ordering::Acquire));
    assert!(matches!(
        task.status(),
        WorkStatus::Cancelled | WorkStatus::Cancelling
    ));
    let stopped = Arc::new(AtomicBool::new(false));
    let seen = stopped.clone();
    let _task = Workbench::with_workspace(app.world_mut(), a)
        .unwrap()
        .work()
        .spawn_task(None, "workspace task", move |cancel| {
            cancel.wait(Duration::from_secs(30));
            seen.store(true, Ordering::Release);
        })
        .unwrap();
    Workspaces::new(app.world_mut()).activate(a).unwrap();
    Workspaces::new(app.world_mut()).close(a, Some(b)).unwrap();
    eventually(&mut app, |app| app.world().get_entity(a.0).is_err());
    assert!(stopped.load(Ordering::Acquire));
    assert!(count(&mut app, b) > 0);
}

#[test]
fn completed_tasks_return_to_their_handle_and_closing_rejects_new_work() {
    let mut work = WorkspaceWork::default();
    let task = work.spawn_task(None, "compute", |_| 42).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(result) = task.try_recv() {
            assert_eq!(result, 42);
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    work.shutdown();
    assert!(work.spawn_task(None, "too late", |_| ()).is_err());
}

#[cfg(unix)]
#[test]
fn managed_process_drains_output_and_workspace_close_terminates_its_process_group() {
    let mut app = host();
    let a = open(&mut app, "A");
    let mut spec = ProcessSpec::new("/bin/sh");
    spec.arguments = ["-c", "printf ready; cat"].map(Into::into).to_vec();
    let process = Workbench::with_workspace(app.world_mut(), a)
        .unwrap()
        .work()
        .spawn_process(None, "interactive pipe", spec)
        .unwrap();
    let mut output = Vec::new();
    eventually(&mut app, |_| {
        for chunk in process.drain_output() {
            output.extend(chunk.bytes);
        }
        output.starts_with(b"ready")
    });
    process.write(b"hello\n").unwrap();
    eventually(&mut app, |_| {
        for chunk in process.drain_output() {
            output.extend(chunk.bytes);
        }
        output.ends_with(b"hello\n")
    });
    let pid = process.process_id;
    Workspaces::new(app.world_mut()).close(a, None).unwrap();
    eventually(&mut app, |app| app.world().get_entity(a.0).is_err());
    assert!(process.exit_status().is_some());
    assert_eq!(unsafe { libc::kill(pid as i32, 0) }, -1);
}

#[cfg(unix)]
#[test]
fn closing_process_stdin_delivers_eof_and_retains_output_and_exit_status() {
    let mut app = host();
    let id = open(&mut app, "Pipe");
    let process = Workbench::with_workspace(app.world_mut(), id)
        .unwrap()
        .work()
        .spawn_process(None, "cat", ProcessSpec::new("/bin/cat"))
        .unwrap();
    process.write(b"all input\n").unwrap();
    process.close_stdin();
    assert!(process.write(b"late").is_err());
    eventually(&mut app, |_| process.status() == WorkStatus::Finished);
    assert!(process.exit_status().unwrap().success());
    let output: Vec<_> = process
        .drain_output()
        .into_iter()
        .flat_map(|chunk| chunk.bytes)
        .collect();
    assert_eq!(output, b"all input\n");
}

#[test]
fn switching_in_dispatch_runs_each_workspaces_preupdate_once() {
    #[derive(Resource, Default)]
    struct Passes(u32);
    fn plugin(app: &mut App) {
        app.init_resource::<Passes>()
            .add_systems(PreUpdate, |mut passes: ResMut<Passes>| passes.0 += 1);
    }
    let mut app = host();
    let mut runtime = WorkspaceRuntime::new("A");
    runtime.add_plugins(plugin);
    let a = Workspaces::new(app.world_mut()).open(runtime);
    let mut runtime = WorkspaceRuntime::new("B");
    runtime.add_plugins(plugin);
    let b = Workspaces::new(app.world_mut()).open(runtime);
    Workspaces::new(app.world_mut()).activate(a).unwrap();
    app.world_mut()
        .resource_mut::<WorkspaceRequests>()
        .0
        .push_back(WorkspaceRequest::Activate(b));
    app.update();
    for id in [a, b] {
        assert_eq!(
            Workbench::with_workspace(app.world_mut(), id)
                .unwrap()
                .world()
                .resource::<Passes>()
                .0,
            1
        );
    }
}

#[test]
fn activating_a_suspended_workspace_prepares_its_frame_before_dispatch() {
    #[derive(Resource, Default)]
    struct Phases {
        first: u32,
        pre: u32,
    }
    fn plugin(app: &mut App) {
        app.init_resource::<Phases>()
            .add_systems(First, |mut phases: ResMut<Phases>| phases.first += 1)
            .add_systems(PreUpdate, |mut phases: ResMut<Phases>| {
                assert!(
                    phases.first > phases.pre,
                    "First must precede each PreUpdate"
                );
                phases.pre += 1;
            });
    }
    let mut app = host();
    let mut runtime = WorkspaceRuntime::new("Paused");
    runtime.background_updates = false;
    runtime.add_plugins(plugin);
    let id = Workspaces::new(app.world_mut()).open(runtime);
    app.update();
    app.world_mut()
        .resource_mut::<WorkspaceRequests>()
        .0
        .push_back(WorkspaceRequest::Activate(id));
    app.update();
    let mut w = Workbench::new(app.world_mut());
    let phases = w.world_mut().resource::<Phases>();
    assert_eq!((phases.first, phases.pre), (1, 1));
}

#[test]
fn asynchronous_plugin_readiness_precedes_activation_and_startup_runs_once() {
    #[derive(Resource, Default)]
    struct Starts(u32);
    struct Loading(Arc<AtomicBool>);
    impl Plugin for Loading {
        fn build(&self, app: &mut App) {
            app.init_resource::<Starts>()
                .add_systems(Startup, |mut starts: ResMut<Starts>| starts.0 += 1);
        }
        fn ready(&self, _: &App) -> bool {
            self.0.load(Ordering::Acquire)
        }
    }
    let mut app = host();
    let ready = Arc::new(AtomicBool::new(false));
    let mut runtime = WorkspaceRuntime::new("Loading");
    runtime.add_plugins(Loading(ready.clone()));
    let id = Workspaces::new(app.world_mut()).open(runtime);
    assert!(Workspaces::new(app.world_mut()).activate(id).is_err());
    ready.store(true, Ordering::Release);
    app.update();
    Workspaces::new(app.world_mut()).activate(id).unwrap();
    app.update();
    app.update();
    assert_eq!(
        Workbench::new(app.world_mut())
            .world()
            .resource::<Starts>()
            .0,
        1
    );
}

#[cfg(unix)]
#[test]
fn shutdown_escalates_for_processes_that_ignore_graceful_termination() {
    let mut app = host();
    let id = open(&mut app, "Ignore TERM");
    let mut spec = ProcessSpec::new("/bin/sh");
    spec.arguments = [
        "-c",
        "trap '' TERM; printf ready; while :; do sleep 10; done",
    ]
    .map(Into::into)
    .to_vec();
    let process = Workbench::with_workspace(app.world_mut(), id)
        .unwrap()
        .work()
        .spawn_process(None, "stubborn", spec)
        .unwrap();
    eventually(&mut app, |_| !process.drain_output().is_empty());
    Workspaces::new(app.world_mut()).close(id, None).unwrap();
    eventually(&mut app, |app| app.world().get_entity(id.0).is_err());
    assert_eq!(unsafe { libc::kill(process.process_id as i32, 0) }, -1);
}

#[cfg(feature = "ui")]
#[test]
fn drawing_queues_activation_until_input_ownership_can_be_released() {
    use bedtime_ui2::input::{InputRouter, RoutedInput};
    let mut app = host();
    app.world_mut().init_resource::<InputRouter>();
    app.world_mut().init_resource::<RoutedInput>();
    let a = Workspaces::new(app.world_mut()).active();
    let b = open(&mut app, "B");
    // UI2 lends the router to the drawing pass until that pass completes.
    let router = app.world_mut().remove_resource::<InputRouter>().unwrap();
    assert!(Workspaces::new(app.world_mut()).activate(b).is_err());
    assert_eq!(Workspaces::new(app.world_mut()).active(), a);
    app.world_mut()
        .resource_mut::<WorkspaceRequests>()
        .0
        .push_back(WorkspaceRequest::Activate(b));
    app.world_mut().insert_resource(router);
    app.update();
    assert_eq!(Workspaces::new(app.world_mut()).active(), b);
}

#[cfg(feature = "ui")]
#[test]
fn switching_releases_capture_and_held_keys_do_not_leak_between_equal_local_entity_ids() {
    use bedtime_ui2::input::{InputEvent, InputModifiers, InputRouter, RoutedInput, UiInputFrame};
    use bevy::input::{
        ButtonState,
        keyboard::{Key, KeyboardInput},
    };
    #[derive(Resource, Default)]
    struct Guest(RoutedInput);
    fn guest(mut input: PanelInput, mut guest: ResMut<Guest>) {
        guest.0 = input.input.clone();
        if input.content_focused {
            input.capture_keyboard();
        }
    }
    let mut app = host();
    app.world_mut().init_resource::<InputRouter>();
    app.world_mut().init_resource::<RoutedInput>();
    let mut make = |name| {
        let mut runtime = WorkspaceRuntime::new(name);
        let mut w = Workbench::new(runtime.world_mut());
        w.world_mut().init_resource::<Guest>();
        let panel = w.create_panel(w.active_layout(), "Game").unwrap();
        register_panel_input(w.world_mut(), panel, guest).unwrap();
        (Workspaces::new(app.world_mut()).open(runtime), panel)
    };
    let (a, panel_a) = make("A");
    let (b, panel_b) = make("B");
    assert_eq!(
        panel_a, panel_b,
        "Exercise colliding IDs in separate worlds"
    );
    Workspaces::new(app.world_mut()).activate(a).unwrap();
    let key = |id| InputEvent {
        id,
        modifiers: InputModifiers::default(),
        event: bevy::window::WindowEvent::KeyboardInput(KeyboardInput {
            key_code: KeyCode::KeyA,
            logical_key: Key::Character("a".into()),
            state: ButtonState::Pressed,
            text: None,
            repeat: id > 1,
            window: Entity::PLACEHOLDER,
        }),
    };
    let frame = |app: &mut App, id, panel, event| {
        let mut w = Workbench::with_workspace(app.world_mut(), id).unwrap();
        let mut p = w.world_mut().get_mut::<PanelPresentation>(panel).unwrap();
        p.visible = true;
        p.rect = Rect::from_corners(Vec2::ZERO, Vec2::splat(200.0));
        *app.world_mut()
            .resource_mut::<InputRouter>()
            .frame
            .get_mut()
            .unwrap() = UiInputFrame {
            focused_content: Some(panel),
            events: vec![event],
            ..default()
        };
        dispatch(app.world_mut());
        app.world_mut()
            .resource_scope(|world, mut router: Mut<InputRouter>| {
                router.publish(&mut world.resource_mut::<RoutedInput>())
            });
    };
    frame(&mut app, a, panel_a, key(1));
    assert!(
        Workbench::with_workspace(app.world_mut(), a)
            .unwrap()
            .world()
            .resource::<Guest>()
            .0
            .keys
            .pressed(KeyCode::KeyA)
    );
    Workspaces::new(app.world_mut()).activate(b).unwrap();
    assert!(
        Workbench::with_workspace(app.world_mut(), a)
            .unwrap()
            .world()
            .resource::<Guest>()
            .0
            .keys
            .just_released(KeyCode::KeyA)
    );
    frame(&mut app, b, panel_b, key(2));
    assert!(
        !Workbench::with_workspace(app.world_mut(), b)
            .unwrap()
            .world()
            .resource::<Guest>()
            .0
            .keys
            .pressed(KeyCode::KeyA)
    );
}
