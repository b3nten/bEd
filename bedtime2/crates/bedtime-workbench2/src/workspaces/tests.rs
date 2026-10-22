use super::*;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Component, Default)]
struct Counter(u32);
fn host() -> App {
    let mut app = App::new();
    app.add_plugins(WorkbenchCorePlugin).add_systems(
        Update,
        |mut counters: Query<&mut Counter, With<WorkspaceRunning>>| {
            for mut count in &mut counters {
                count.0 += 1;
            }
        },
    );
    app
}
fn open(app: &mut App, name: &str) -> WorkspaceId {
    let id = Workspaces::new(app.world_mut()).open(name);
    app.world_mut()
        .entity_mut(id.entity())
        .insert(Counter::default());
    id
}
fn count(app: &mut App, id: WorkspaceId) -> u32 {
    app.world().get::<Counter>(id.entity()).unwrap().0
}
fn eventually(app: &mut App, mut condition: impl FnMut(&mut App) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition(app) {
        assert!(Instant::now() < deadline, "Workspace operation timed out");
        app.update();
        std::thread::yield_now();
    }
}

#[test]
fn application_startup_opens_the_first_workspace_and_later_opens_keep_it_active() {
    let mut app = host();
    assert!(Workspaces::new(app.world_mut()).list().is_empty());
    app.add_systems(Startup, |world: &mut World| {
        Workspaces::new(world).open("Project");
        Workspaces::new(world).open("Scratch");
    });
    app.update();
    let entries = Workspaces::new(app.world_mut()).list();
    assert_eq!(entries.len(), 2);
    let selected = entries
        .iter()
        .filter(|workspace| workspace.active)
        .collect::<Vec<_>>();
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].name, "Project");
    let active = Workspaces::new(app.world_mut()).active();
    assert_eq!(active, selected[0].id);
    assert!(
        app.world()
            .get::<ActiveWorkspace>(active.entity())
            .is_some()
    );
    let mut workbench = Workbench::new(app.world_mut());
    let layout = workbench.active_layout();
    workbench.create_panel(layout, "Inspector").unwrap();
}

#[test]
#[should_panic(expected = "No active workspace")]
fn workspace_operations_require_application_initialization() {
    let mut app = host();
    Workbench::new(app.world_mut());
}

#[test]
fn workspace_members_and_application_entities_share_one_world() {
    let mut app = host();
    let a = open(&mut app, "A");
    let b = open(&mut app, "Other");
    let game = app.world_mut().spawn(Counter(42)).id();
    let mut w = Workbench::with_workspace(app.world_mut(), b).unwrap();
    assert_eq!(w.world().get::<Counter>(game).unwrap().0, 42);
    let layout = w.active_layout();
    let panel = w.create_panel(layout, "Inspector").unwrap();
    assert_eq!(w.world().get::<InWorkspace>(panel).unwrap().0, b.entity());
    assert_ne!(
        Workbench::with_workspace(app.world_mut(), a)
            .unwrap()
            .active_layout(),
        layout
    );
    app.update();
    assert_eq!(app.world().get::<Counter>(game).unwrap().0, 42);
    assert!(app.world().get::<InWorkspace>(game).is_none());
}

#[test]
fn markers_follow_spawn_transfer_removal_and_activation() {
    let mut app = host();
    let a = open(&mut app, "A");
    let b = open(&mut app, "B");
    let member = app.world_mut().spawn(InWorkspace(a.entity())).id();
    app.update();
    assert!(app.world().get::<InActiveWorkspace>(member).is_some());
    app.world_mut()
        .entity_mut(member)
        .insert(InWorkspace(b.entity()));
    app.update();
    assert!(app.world().get::<InActiveWorkspace>(member).is_none());
    assert!(
        !app.world()
            .get::<WorkspaceMembers>(a.entity())
            .unwrap()
            .entities()
            .contains(&member)
    );
    assert!(
        app.world()
            .get::<WorkspaceMembers>(b.entity())
            .unwrap()
            .entities()
            .contains(&member)
    );
    Workspaces::new(app.world_mut()).activate(b).unwrap();
    assert!(app.world().get::<InActiveWorkspace>(member).is_some());
    app.world_mut().entity_mut(member).remove::<InWorkspace>();
    app.update();
    assert!(app.world().get::<InActiveWorkspace>(member).is_none());
    assert!(app.world().get::<InRunningWorkspace>(member).is_none());
}

#[test]
fn deferred_membership_changes_use_the_final_state() {
    let mut app = host();
    let a = open(&mut app, "A");
    let b = open(&mut app, "B");
    let member = app.world_mut().spawn(InWorkspace(a.entity())).id();
    app.add_systems(Startup, move |mut commands: Commands| {
        commands.entity(member).insert(InWorkspace(b.entity()));
        commands.entity(member).remove::<InWorkspace>();
        commands.entity(member).insert(InWorkspace(a.entity()));
    });
    app.update();
    assert!(app.world().get::<InActiveWorkspace>(member).is_some());
    Workspaces::new(app.world_mut()).activate(b).unwrap();
    Workspaces::new(app.world_mut()).activate(a).unwrap();
    Workspaces::new(app.world_mut()).activate(b).unwrap();
    assert!(app.world().get::<InActiveWorkspace>(member).is_none());
    app.world_mut().despawn(member);
    app.update();
}

#[test]
fn normal_systems_scope_active_and_background_workspaces() {
    let mut app = host();
    let a = open(&mut app, "A");
    let b = open(&mut app, "B");
    let a_member = app
        .world_mut()
        .spawn((InWorkspace(a.entity()), Counter::default()))
        .id();
    let b_member = app
        .world_mut()
        .spawn((InWorkspace(b.entity()), Counter::default()))
        .id();
    app.add_systems(
        Update,
        |mut members: Query<&mut Counter, With<InActiveWorkspace>>| {
            for mut count in &mut members {
                count.0 += 1;
            }
        },
    );
    Workspaces::new(app.world_mut()).activate(a).unwrap();
    Workspaces::new(app.world_mut())
        .set_background_updates(b, false)
        .unwrap();
    app.update();
    assert_eq!(count(&mut app, a), 1);
    assert_eq!(count(&mut app, b), 0);
    assert_eq!(app.world().get::<Counter>(a_member).unwrap().0, 1);
    assert_eq!(app.world().get::<Counter>(b_member).unwrap().0, 0);
    Workspaces::new(app.world_mut()).activate(b).unwrap();
    assert!(app.world().get::<WorkspaceRunning>(b.entity()).is_some());
    assert!(app.world().get::<InRunningWorkspace>(b_member).is_some());
    app.update();
    assert_eq!(count(&mut app, a), 2);
    assert_eq!(count(&mut app, b), 1);
    assert_eq!(app.world().get::<Counter>(a_member).unwrap().0, 1);
    assert_eq!(app.world().get::<Counter>(b_member).unwrap().0, 1);
}

#[test]
fn layouts_and_panels_reject_foreign_workspace_targets() {
    let mut app = host();
    let a = open(&mut app, "A");
    let b = open(&mut app, "B");
    let b_layout = Workbench::with_workspace(app.world_mut(), b)
        .unwrap()
        .active_layout();
    let b_area = Workbench::with_workspace(app.world_mut(), b)
        .unwrap()
        .destination_area(b_layout)
        .unwrap();
    let mut w = Workbench::with_workspace(app.world_mut(), a).unwrap();
    assert!(w.activate_layout(b_layout).is_err());
    assert!(w.delete_layout(b_layout).is_err());
    assert!(w.create_panel_in_area(b_area, "Wrong owner").is_err());
    assert!(w.focus_area(b_area).is_err());
    assert!(w.destination_area(b_layout).is_err());
    assert_ne!(w.active_layout(), b_layout);
}

#[test]
fn closing_cleans_members_and_keeps_unrelated_game_entities() {
    let mut app = host();
    let a = open(&mut app, "A");
    let b = open(&mut app, "B");
    let game = app.world_mut().spawn_empty().id();
    let member = app.world_mut().spawn(InWorkspace(a.entity())).id();
    let mut w = Workbench::with_workspace(app.world_mut(), a).unwrap();
    let layout = w.active_layout();
    let panel = w.create_panel(layout, "Owned").unwrap();
    Workspaces::new(app.world_mut()).activate(a).unwrap();
    assert!(Workspaces::new(app.world_mut()).close(a, Some(a)).is_err());
    Workspaces::new(app.world_mut()).close(a, Some(b)).unwrap();
    assert!(Workbench::with_workspace(app.world_mut(), a).is_err());
    assert!(app.world().get::<InActiveWorkspace>(member).is_none());
    assert!(app.world().get::<InRunningWorkspace>(member).is_none());
    app.update();
    for entity in [a.entity(), member, layout, panel] {
        assert!(app.world().get_entity(entity).is_err());
    }
    assert!(app.world().get_entity(game).is_ok());
    assert_eq!(Workspaces::new(app.world_mut()).active(), b);
    Workspaces::new(app.world_mut()).close(b, None).unwrap();
    app.update();
    assert!(Workspaces::new(app.world_mut()).selected().is_none());
}

struct Increment;
impl ContextCommand for Increment {
    const KEY: &'static str = "test.increment";
    const LABEL: &'static str = "Increment";
}
#[test]
fn queued_commands_retain_their_workspace_when_activation_changes() {
    let mut app = host();
    let a = open(&mut app, "A");
    let b = open(&mut app, "B");
    install_global_handler::<Increment, _>(
        app.world_mut(),
        |In(invocation): In<ContextCommandInvocation>,
         mut counters: Query<&mut Counter>|
         -> ContextCommandResult {
            counters
                .get_mut(invocation.workspace.unwrap().entity())
                .unwrap()
                .0 += 100;
            Ok(())
        },
    )
    .unwrap();
    Workbench::with_workspace(app.world_mut(), a)
        .unwrap()
        .commands()
        .invoke::<Increment>();
    Workbench::with_workspace(app.world_mut(), b)
        .unwrap()
        .commands()
        .invoke::<Increment>();
    Workspaces::new(app.world_mut()).activate(b).unwrap();
    app.update();
    assert_eq!(count(&mut app, a), 101);
    assert_eq!(count(&mut app, b), 101);
}

#[test]
fn queued_layout_edits_retain_their_workspace() {
    let mut app = host();
    let a = open(&mut app, "A");
    let b = open(&mut app, "B");
    let mut w = Workbench::with_workspace(app.world_mut(), a).unwrap();
    let other = w.create_layout("Other").unwrap();
    w.requests().submit(WorkbenchRequest::ActivateLayout(other));
    Workspaces::new(app.world_mut()).activate(b).unwrap();
    app.update();
    assert_eq!(
        Workbench::with_workspace(app.world_mut(), a)
            .unwrap()
            .active_layout(),
        other
    );
    assert_ne!(Workbench::new(app.world_mut()).active_layout(), other);
}

#[cfg(feature = "ui")]
#[test]
fn titlebar_state_stays_with_the_surface_across_workspace_switches_and_closure() {
    #[derive(Component)]
    struct Title(u32);
    let mut app = host();
    let a = open(&mut app, "A");
    let b = open(&mut app, "B");
    let surface = app
        .world_mut()
        .spawn((
            Window::default(),
            crate::Titlebar {
                height: 20.0,
                draw: |_, _, _| {},
            },
            Title(42),
        ))
        .id();
    Workspaces::new(app.world_mut()).activate(b).unwrap();
    assert_eq!(app.world().get::<Title>(surface).unwrap().0, 42);
    Workspaces::new(app.world_mut()).close(a, None).unwrap();
    app.update();
    Workspaces::new(app.world_mut()).close(b, None).unwrap();
    app.update();
    assert!(Workspaces::new(app.world_mut()).selected().is_none());
    assert_eq!(
        app.world().get::<crate::Titlebar>(surface).unwrap().height,
        20.0
    );
    assert_eq!(app.world().get::<Title>(surface).unwrap().0, 42);
}

#[test]
fn closing_waits_for_cooperative_work_without_blocking_updates() {
    let mut app = host();
    let a = open(&mut app, "A");
    let b = open(&mut app, "B");
    let (release, wait) = std::sync::mpsc::channel();
    Workbench::with_workspace(app.world_mut(), a)
        .unwrap()
        .work()
        .spawn_task(None, "Wait", move |_| {
            wait.recv().unwrap();
        })
        .unwrap();
    Workspaces::new(app.world_mut()).close(a, Some(b)).unwrap();
    app.update();
    assert!(app.world().get_entity(a.entity()).is_ok());
    assert_eq!(count(&mut app, b), 1);
    release.send(()).unwrap();
    eventually(&mut app, |app| app.world().get_entity(a.entity()).is_err());
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
    let b = open(&mut app, "B");
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
    Workspaces::new(app.world_mut()).close(a, Some(b)).unwrap();
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
fn inactive_menu_snapshots_capture_their_own_workspaces_focused_panel() {
    let mut app = host();
    let a = open(&mut app, "A");
    let b = open(&mut app, "B");
    let mut w = Workbench::with_workspace(app.world_mut(), b).unwrap();
    let panel = w.create_panel(w.active_layout(), "B panel").unwrap();
    w.register_handler::<Increment, _>(|_: In<ContextCommandInvocation>| -> ContextCommandResult {
        Ok(())
    })
    .unwrap();
    let root = app
        .world_mut()
        .spawn((MenuBar, InWorkspace(b.entity())))
        .id();
    let menu = app
        .world_mut()
        .spawn((
            Menu {
                label: "File".into(),
            },
            InMenu(root),
        ))
        .id();
    let item = app
        .world_mut()
        .spawn((
            MenuItem::new("Increment"),
            Invokes::of::<Increment>(),
            InMenu(menu),
        ))
        .id();
    let popup = app
        .world_mut()
        .spawn((
            Menu {
                label: "Panel popup".into(),
            },
            InPanel(panel),
        ))
        .id();
    let popup_item = app
        .world_mut()
        .spawn((
            MenuItem::new("Increment"),
            Invokes::of::<Increment>(),
            InMenu(popup),
        ))
        .id();
    let popup_snapshot = menu_snapshot(app.world_mut(), popup, InvocationContext::Focused).unwrap();
    assert_eq!(
        popup_snapshot
            .request(popup_item, ContextCommandId::of::<Increment>())
            .unwrap()
            .context,
        InvocationContext::Panel(panel)
    );
    let snapshot = menu_snapshot(app.world_mut(), root, InvocationContext::Focused).unwrap();
    let request = snapshot
        .request(item, ContextCommandId::of::<Increment>())
        .unwrap();
    assert_eq!(request.context, InvocationContext::Panel(panel));
    Workbench::with_workspace(app.world_mut(), b)
        .unwrap()
        .commands()
        .submit(request);
    app.update();
    let result = app
        .world_mut()
        .resource_mut::<Messages<ContextCommandFinished>>()
        .drain()
        .next()
        .unwrap();
    assert!(result.result.is_ok());
    assert_eq!(result.invocation.workspace, Some(b));
    assert_eq!(result.invocation.panel, Some(panel));
    assert_eq!(Workspaces::new(app.world_mut()).active(), a);
}
