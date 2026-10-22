use super::*;
use bedtime_workbench2::{
    InArea, Workbench, WorkbenchCorePlugin, WorkspaceId, WorkspaceWork, Workspaces,
};
use std::time::{Duration, Instant};

fn host() -> (App, WorkspaceId) {
    let mut app = App::new();
    // Capability installation does not require a workspace or a UI/GPU context.
    app.add_plugins((TerminalPlugin, WorkbenchCorePlugin));
    let workspace = Workspaces::new(app.world_mut()).open("Terminals");
    (app, workspace)
}

fn panel(app: &mut App, workspace: WorkspaceId, options: TerminalOptions) -> Entity {
    let workbench = Workbench::with_workspace(app.world_mut(), workspace).unwrap();
    let area = workbench
        .destination_area(workbench.active_layout())
        .unwrap();
    app.world_mut()
        .spawn((TerminalPanel::new(options), InArea(area)))
        .id()
}

fn observe(app: &mut App, panel: Entity) {
    let mut terminal = app.world_mut().get_mut::<TerminalPanel>(panel).unwrap();
    terminal.size = Some(WindowSize {
        num_cols: 80,
        num_lines: 24,
        cell_width: 8,
        cell_height: 16,
    });
    terminal.drawn = true;
}

fn text(app: &App, panel: Entity) -> String {
    app.world()
        .get::<TerminalPanel>(panel)
        .unwrap()
        .snapshot()
        .lines
        .iter()
        .map(|line| {
            line.cells
                .iter()
                .filter(|cell| cell.width != 0)
                .map(|cell| cell.text.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn eventually(app: &mut App, mut condition: impl FnMut(&mut App) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(8);
    while !condition(app) {
        if Instant::now() >= deadline {
            let statuses: Vec<_> = app
                .world_mut()
                .query::<&TerminalPanel>()
                .iter(app.world())
                .map(|terminal| terminal.status().clone())
                .collect();
            panic!("Terminal operation timed out: {statuses:?}");
        }
        app.update();
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn finish(app: &mut App) {
    Workspaces::new(app.world_mut()).shutdown_work().unwrap();
    eventually(app, |app| Workspaces::new(app.world_mut()).work_finished());
    app.update();
}

#[test]
fn concrete_composition_preserves_options_and_does_not_start_unseen_shells() {
    let (mut app, workspace) = host();
    let mut options = TerminalOptions::default();
    options
        .env
        .insert("TERMINAL_TEST".into(), "preserved".into());
    let panel = panel(&mut app, workspace, options);
    app.update();
    let terminal = app.world().get::<TerminalPanel>(panel).unwrap();
    assert_eq!(terminal.options.env["TERMINAL_TEST"], "preserved");
    assert_eq!(terminal.status(), &TerminalStatus::Pending);
    assert!(terminal.process_id().is_none());
    assert!(
        app.world()
            .get::<WorkspaceWork>(workspace.entity())
            .unwrap()
            .list()
            .is_empty()
    );

    let area = app.world().get::<InArea>(panel).unwrap().0;
    let fresh = app
        .world_mut()
        .spawn((TerminalPanel::default(), InArea(area)))
        .id();
    let terminal = app.world().get::<TerminalPanel>(fresh).unwrap();
    assert_eq!(terminal.status(), &TerminalStatus::Pending);
    assert!(terminal.process_id().is_none());
    assert!(terminal.options.env.is_empty());
    finish(&mut app);
}

#[cfg(unix)]
fn script(source: &str) -> TerminalOptions {
    TerminalOptions {
        shell: Some(TerminalShell::new(
            "/bin/sh",
            vec!["-c".into(), source.into()],
        )),
        ..Default::default()
    }
}

#[cfg(unix)]
fn input(app: &mut App, panel: Entity, input: TerminalCommand) {
    app.world_mut()
        .get_mut::<TerminalPanel>(panel)
        .unwrap()
        .runtime
        .as_mut()
        .unwrap()
        .pty
        .command(input)
        .unwrap();
}

#[cfg(unix)]
fn alive(pid: u32) -> bool {
    // A reaped child, not just a signalled process, must disappear.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn tab_title_follows_foreground_jobs_and_returns_to_the_shell_even_while_hidden() {
    let (mut app, workspace) = host();
    let panel = panel(
        &mut app,
        workspace,
        TerminalOptions {
            shell: Some(TerminalShell::new(
                "/bin/bash",
                vec!["--noprofile".into(), "--norc".into(), "-i".into()],
            )),
            ..Default::default()
        },
    );
    observe(&mut app, panel);
    eventually(&mut app, |app| {
        observe(app, panel);
        app.world().get::<PanelInstance>(panel).unwrap().title == "bash"
            && app.world().get::<TerminalPanel>(panel).unwrap().status() == &TerminalStatus::Running
    });
    let pid = app
        .world()
        .get::<TerminalPanel>(panel)
        .unwrap()
        .process_id();
    input(
        &mut app,
        panel,
        TerminalCommand::Text("printf '\\033]2;not-a-process\\007'\r".into()),
    );
    eventually(&mut app, |app| {
        observe(app, panel);
        app.world()
            .get::<TerminalPanel>(panel)
            .unwrap()
            .snapshot()
            .title
            .as_ref()
            == "not-a-process"
    });
    assert_eq!(
        app.world().get::<PanelInstance>(panel).unwrap().title,
        "bash",
        "Shell OSC titles must not replace the process name"
    );

    input(
        &mut app,
        panel,
        TerminalCommand::Text("/bin/sleep 2\r".into()),
    );
    // Do not observe/draw the panel again: process-title observation is not
    // coupled to viewport snapshots, process output, or visible workspaces.
    eventually(&mut app, |app| {
        app.world().get::<PanelInstance>(panel).unwrap().title == "sleep"
    });
    assert_eq!(
        app.world()
            .get::<TerminalPanel>(panel)
            .unwrap()
            .process_id(),
        pid
    );
    eventually(&mut app, |app| {
        app.world().get::<PanelInstance>(panel).unwrap().title == "bash"
    });

    // A newly composed session has its own local state and default label.
    app.world_mut()
        .get_mut::<PanelInstance>(panel)
        .unwrap()
        .title = "stale job".into();
    let area = app.world().get::<InArea>(panel).unwrap().0;
    let fresh = app
        .world_mut()
        .spawn((TerminalPanel::default(), InArea(area)))
        .id();
    assert_eq!(
        app.world().get::<PanelInstance>(fresh).unwrap().title,
        TerminalPanel::LABEL
    );
    assert_eq!(
        app.world().get::<TerminalPanel>(fresh).unwrap().status(),
        &TerminalStatus::Pending
    );
    finish(&mut app);
}

#[cfg(unix)]
#[test]
fn live_theme_updates_recolor_the_existing_session_and_preserve_shell_overrides() {
    let (mut app, workspace) = host();
    let panel = panel(
        &mut app,
        workspace,
        script(
            "printf '\\033[34mBLUE\\033[0m\\n'; while IFS= read -r line; do \
         case \"$line\" in \
         override) printf '\\033]4;4;#123456\\007OVERRIDE\\n';; \
         reset) printf '\\033]104;4\\007RESET\\n';; esac; done",
        ),
    );
    let mut theme = bedtime_ui2::themes::dark();
    let expected_palette = |theme: &Theme| {
        let mut terminal = Terminal::new_text_only(80, 24);
        terminal.set_theme(&TerminalTheme::new(
            theme.palette.panel_background,
            theme.palette.text,
            theme.palette.terminal,
        ));
        terminal.snapshot().palette.clone()
    };
    app.world_mut()
        .get_mut::<TerminalPanel>(panel)
        .unwrap()
        .theme = Some(terminal_theme(&theme));
    observe(&mut app, panel);
    eventually(&mut app, |app| {
        observe(app, panel);
        let state = app.world().get::<TerminalPanel>(panel).unwrap();
        state.snapshot().palette == expected_palette(&theme) && text(app, panel).contains("BLUE")
    });
    let pid = app
        .world()
        .get::<TerminalPanel>(panel)
        .unwrap()
        .process_id();

    theme.palette.terminal[4] = [1.0, 0.6, 1.0, 1.0];
    app.world_mut()
        .get_mut::<TerminalPanel>(panel)
        .unwrap()
        .theme = Some(terminal_theme(&theme));
    eventually(&mut app, |app| {
        observe(app, panel);
        app.world()
            .get::<TerminalPanel>(panel)
            .unwrap()
            .snapshot()
            .palette
            == expected_palette(&theme)
    });
    assert_eq!(
        app.world()
            .get::<TerminalPanel>(panel)
            .unwrap()
            .snapshot()
            .palette[4],
        [255, 153, 255]
    );

    input(&mut app, panel, TerminalCommand::Text("override\r".into()));
    eventually(&mut app, |app| {
        observe(app, panel);
        app.world()
            .get::<TerminalPanel>(panel)
            .unwrap()
            .snapshot()
            .palette[4]
            == [0x12, 0x34, 0x56]
    });
    theme = bedtime_ui2::themes::light();
    let mut overridden = *expected_palette(&theme);
    overridden[4] = [0x12, 0x34, 0x56];
    app.world_mut()
        .get_mut::<TerminalPanel>(panel)
        .unwrap()
        .theme = Some(terminal_theme(&theme));
    eventually(&mut app, |app| {
        observe(app, panel);
        app.world()
            .get::<TerminalPanel>(panel)
            .unwrap()
            .snapshot()
            .palette
            .as_ref()
            == &overridden
    });
    input(&mut app, panel, TerminalCommand::Text("reset\r".into()));
    eventually(&mut app, |app| {
        observe(app, panel);
        app.world()
            .get::<TerminalPanel>(panel)
            .unwrap()
            .snapshot()
            .palette
            == expected_palette(&theme)
    });
    let state = app.world().get::<TerminalPanel>(panel).unwrap();
    assert_eq!(
        state.process_id(),
        pid,
        "A theme edit must not restart the shell"
    );
    assert!(
        text(&app, panel).contains("BLUE"),
        "Theme changes retain transcript cells"
    );
    finish(&mut app);
}

#[cfg(unix)]
#[test]
fn real_pty_input_resize_exit_selection_and_restart() {
    let (mut app, workspace) = host();
    let panel = panel(
        &mut app,
        workspace,
        script("printf 'READY\\n'; IFS= read -r line; printf 'ANSWER:%s\\n' \"$line\"; exit 7"),
    );
    observe(&mut app, panel);
    eventually(&mut app, |app| {
        observe(app, panel);
        text(app, panel).contains("READY")
            && app.world().get::<TerminalPanel>(panel).unwrap().status() == &TerminalStatus::Running
    });
    let first_pid = app
        .world()
        .get::<TerminalPanel>(panel)
        .unwrap()
        .process_id()
        .unwrap();
    assert!(alive(first_pid));
    let work = app
        .world()
        .get::<WorkspaceWork>(workspace.entity())
        .unwrap()
        .list();
    assert_eq!(
        work.len(),
        1,
        "One scoped runtime, not a supervisor plus a PTY worker"
    );
    assert_eq!(work[0].owner, Some(panel));

    let size = WindowSize {
        num_cols: 60,
        num_lines: 18,
        cell_width: 9,
        cell_height: 17,
    };
    {
        let mut terminal = app.world_mut().get_mut::<TerminalPanel>(panel).unwrap();
        terminal.size = Some(size);
        terminal.drawn = true;
    }
    app.update();
    eventually(&mut app, |app| {
        let mut terminal = app.world_mut().get_mut::<TerminalPanel>(panel).unwrap();
        terminal.drawn = true;
        terminal.snapshot().cols == 60 && terminal.snapshot().rows == 18
    });
    input(
        &mut app,
        panel,
        TerminalCommand::Text("hello terminal\r".into()),
    );
    eventually(&mut app, |app| {
        matches!(
            app.world().get::<TerminalPanel>(panel).unwrap().status(),
            TerminalStatus::Exited(_)
        )
    });
    let terminal = app.world().get::<TerminalPanel>(panel).unwrap();
    assert!(
        matches!(terminal.status(), TerminalStatus::Exited(Some(exit)) if exit.code() == Some(7))
    );
    assert!(text(&app, panel).contains("ANSWER:hello terminal"));
    assert!(!alive(first_pid));
    assert_eq!(
        app.world()
            .get::<WorkspaceWork>(workspace.entity())
            .unwrap()
            .list()
            .len(),
        1,
        "Exited emulator remains selectable"
    );

    input(&mut app, panel, TerminalCommand::SelectAll);
    eventually(&mut app, |app| {
        app.world_mut()
            .get_mut::<TerminalPanel>(panel)
            .unwrap()
            .drawn = true;
        app.world()
            .get::<TerminalPanel>(panel)
            .unwrap()
            .snapshot()
            .selection_text
            .as_deref()
            .is_some_and(|s| s.contains("ANSWER:hello terminal"))
    });
    app.world_mut()
        .get_mut::<TerminalPanel>(panel)
        .unwrap()
        .restart();
    eventually(&mut app, |app| {
        observe(app, panel);
        let terminal = app.world().get::<TerminalPanel>(panel).unwrap();
        terminal.status() == &TerminalStatus::Running
            && terminal.process_id().is_some_and(|pid| pid != first_pid)
    });
    assert!(
        !text(&app, panel).contains("ANSWER:hello terminal"),
        "Restart must not reuse the old transcript"
    );
    finish(&mut app);
}

#[cfg(unix)]
#[test]
fn hidden_inactive_terminals_keep_parsing_and_raw_despawn_reaps_the_child() {
    let (mut app, workspace) = host();
    let panel = panel(
        &mut app,
        workspace,
        script("printf 'READY\\n'; IFS= read -r line; printf 'HIDDEN:%s\\n' \"$line\"; sleep 30"),
    );
    observe(&mut app, panel);
    eventually(&mut app, |app| {
        observe(app, panel);
        app.world().get::<TerminalPanel>(panel).unwrap().status() == &TerminalStatus::Running
    });
    let pid = app
        .world()
        .get::<TerminalPanel>(panel)
        .unwrap()
        .process_id()
        .unwrap();
    Workspaces::new(app.world_mut())
        .set_background_updates(workspace, false)
        .unwrap();
    let other = Workspaces::new(app.world_mut()).open("Other");
    Workspaces::new(app.world_mut()).activate(other).unwrap();
    input(
        &mut app,
        panel,
        TerminalCommand::Text("kept running\r".into()),
    );
    // No visibility-dependent system is needed to drain or parse terminal bytes.
    for _ in 0..20 {
        app.update();
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(alive(pid));
    Workspaces::new(app.world_mut())
        .activate(workspace)
        .unwrap();
    eventually(&mut app, |app| {
        observe(app, panel);
        text(app, panel).contains("HIDDEN:kept running")
    });
    app.world_mut().despawn(panel);
    eventually(&mut app, |app| {
        app.world()
            .get::<WorkspaceWork>(workspace.entity())
            .unwrap()
            .list()
            .is_empty()
    });
    assert!(!alive(pid), "Panel despawn must complete process cleanup");
    finish(&mut app);
}

#[cfg(unix)]
#[test]
fn workspace_close_finishes_terminal_cleanup_before_retiring_its_entities() {
    let (mut app, workspace) = host();
    let panel = panel(&mut app, workspace, script("printf 'READY\\n'; sleep 30"));
    observe(&mut app, panel);
    eventually(&mut app, |app| {
        app.world().get::<TerminalPanel>(panel).unwrap().status() == &TerminalStatus::Running
    });
    let pid = app
        .world()
        .get::<TerminalPanel>(panel)
        .unwrap()
        .process_id()
        .unwrap();
    Workspaces::new(app.world_mut())
        .close(workspace, None)
        .unwrap();
    eventually(&mut app, |app| {
        app.world().get_entity(workspace.entity()).is_err()
    });
    assert!(app.world().get_entity(panel).is_err());
    assert!(!alive(pid));
}

#[cfg(unix)]
#[test]
fn failed_start_is_observable_and_can_restart_with_new_options() {
    let (mut app, workspace) = host();
    let panel = panel(
        &mut app,
        workspace,
        TerminalOptions {
            shell: Some(TerminalShell::new(
                "/no/such/bedtime-terminal-test-shell",
                Vec::new(),
            )),
            ..Default::default()
        },
    );
    observe(&mut app, panel);
    eventually(&mut app, |app| {
        matches!(
            app.world().get::<TerminalPanel>(panel).unwrap().status(),
            TerminalStatus::Failed(_)
        )
    });
    assert!(
        app.world()
            .get::<TerminalPanel>(panel)
            .unwrap()
            .error()
            .is_some()
    );
    {
        let mut terminal = app.world_mut().get_mut::<TerminalPanel>(panel).unwrap();
        terminal.options = script("printf 'RECOVERED\\n'; sleep 30");
        terminal.restart();
    }
    eventually(&mut app, |app| {
        observe(app, panel);
        text(app, panel).contains("RECOVERED")
    });
    assert!(
        app.world()
            .get::<TerminalPanel>(panel)
            .unwrap()
            .error()
            .is_none()
    );
    finish(&mut app);
}

#[cfg(unix)]
#[test]
fn shutdown_refuses_new_terminals_without_starting_a_child() {
    let (mut app, workspace) = host();
    let panel = panel(&mut app, workspace, script("sleep 30"));
    observe(&mut app, panel);
    Workspaces::new(app.world_mut()).shutdown_work().unwrap();
    app.update();
    let terminal = app.world().get::<TerminalPanel>(panel).unwrap();
    assert!(
        matches!(terminal.status(), TerminalStatus::Failed(error) if error.contains("closing"))
    );
    assert!(terminal.process_id().is_none());
    assert!(Workspaces::new(app.world_mut()).work_finished());
}

#[cfg(unix)]
#[test]
fn stopping_an_exited_terminal_is_idempotent_and_keeps_restart_available() {
    let (mut app, workspace) = host();
    let panel = panel(&mut app, workspace, script("printf FINISHED; exit 7"));
    observe(&mut app, panel);
    eventually(&mut app, |app| {
        matches!(
            app.world().get::<TerminalPanel>(panel).unwrap().status(),
            TerminalStatus::Exited(_)
        )
    });
    for _ in 0..2 {
        app.world_mut()
            .get_mut::<TerminalPanel>(panel)
            .unwrap()
            .stop();
        app.update();
        assert!(
            matches!(app.world().get::<TerminalPanel>(panel).unwrap().status(), TerminalStatus::Exited(Some(exit)) if exit.code() == Some(7))
        );
    }
    finish(&mut app);
}

#[cfg(unix)]
#[test]
fn explicit_environment_removal_wins_over_v2_defaults_but_not_explicit_values() {
    let (mut app, workspace) = host();
    let mut options = script("printf 'PROGRAM:%s\\n' \"${TERM_PROGRAM-UNSET}\"");
    options.env_remove.push("TERM_PROGRAM".into());
    let removed = panel(&mut app, workspace, options.clone());
    options
        .env
        .insert("TERM_PROGRAM".into(), "caller-override".into());
    let overridden = panel(&mut app, workspace, options);
    observe(&mut app, removed);
    observe(&mut app, overridden);
    eventually(&mut app, |app| {
        observe(app, removed);
        observe(app, overridden);
        text(app, removed).contains("PROGRAM:UNSET")
            && text(app, overridden).contains("PROGRAM:caller-override")
    });
    finish(&mut app);
}

#[cfg(unix)]
#[test]
fn queue_backpressure_retains_ordered_key_release_and_focus_cleanup_until_delivery() {
    use bedtime_terminal::terminal_input::{KeyState, TerminalKey, TerminalKeyEvent};
    use std::sync::atomic::{AtomicBool, Ordering};
    let (mut app, workspace) = host();
    let panel = panel(&mut app, workspace, script("printf READY; sleep 30"));
    let size = WindowSize {
        num_cols: 80,
        num_lines: 24,
        cell_width: 8,
        cell_height: 16,
    };
    let mut model = Terminal::new_text_only(80, 24);
    model.feed(b"\x1b[>3u\x1b[?1004h");
    let options = app
        .world()
        .get::<TerminalPanel>(panel)
        .unwrap()
        .options
        .clone();
    let (pty, worker) = TerminalPty::prepare_terminal(options, size, model).unwrap();
    let mut full = false;
    for _ in 0..1024 {
        if let Err(error) = pty.command(TerminalCommand::Scroll(0)) {
            assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
            full = true;
            break;
        }
    }
    assert!(full);
    let start = Arc::new(AtomicBool::new(false));
    let start_worker = start.clone();
    let task = app
        .world_mut()
        .get_mut::<WorkspaceWork>(workspace.entity())
        .unwrap()
        .spawn_task(Some(panel), "Delayed terminal", move |cancel| {
            while !start_worker.load(Ordering::Acquire) && !cancel.wait(Duration::from_millis(1)) {}
            worker
                .run(|| cancel.is_cancelled())
                .map_err(|error| error.to_string())
        })
        .unwrap();
    {
        let mut terminal = app.world_mut().get_mut::<TerminalPanel>(panel).unwrap();
        terminal.runtime = Some(Runtime {
            shell_token: None,
            pty,
            task,
            pending_inputs: VecDeque::new(),
            size,
            theme: None,
        });
        terminal.status = TerminalStatus::Starting;
        send_inputs(
            &mut terminal,
            vec![
                TerminalInput::Command(TerminalCommand::Key(TerminalKeyEvent {
                    key: TerminalKey::Character("a".into()),
                    state: KeyState::Release,
                    ..Default::default()
                })),
                TerminalInput::Command(TerminalCommand::Focus(false)),
            ],
        );
        let pending = &terminal.runtime.as_ref().unwrap().pending_inputs;
        assert!(
            matches!(pending.front(), Some(TerminalInput::Command(TerminalCommand::Key(event))) if event.state == KeyState::Release)
        );
        assert!(matches!(
            pending.back(),
            Some(TerminalInput::Command(TerminalCommand::Focus(false)))
        ));
        assert!(
            terminal.error().is_none(),
            "Backpressure is retryable, not input loss"
        );
    }
    start.store(true, Ordering::Release);
    eventually(&mut app, |app| {
        observe(app, panel);
        let terminal = app.world().get::<TerminalPanel>(panel).unwrap();
        terminal.runtime.as_ref().unwrap().pending_inputs.is_empty()
            && terminal.status() == &TerminalStatus::Running
    });
    finish(&mut app);
}

#[cfg(unix)]
#[test]
fn restart_requires_confirmed_cleanup_even_when_cancelled_task_results_are_discarded() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let (mut app, workspace) = host();
    let panel = panel(&mut app, workspace, script("sleep 30"));
    let size = WindowSize {
        num_cols: 80,
        num_lines: 24,
        cell_width: 8,
        cell_height: 16,
    };
    let options = app
        .world()
        .get::<TerminalPanel>(panel)
        .unwrap()
        .options
        .clone();
    let (pty, worker) =
        TerminalPty::prepare_terminal(options, size, Terminal::new_text_only(80, 24)).unwrap();
    let task = app
        .world_mut()
        .get_mut::<WorkspaceWork>(workspace.entity())
        .unwrap()
        .spawn_task(Some(panel), "Panicking terminal owner", move |_| {
            let checks = AtomicUsize::new(0);
            worker
                .run(|| {
                    assert_eq!(
                        checks.fetch_add(1, Ordering::Relaxed),
                        0,
                        "Simulated owner failure after child startup"
                    );
                    false
                })
                .map_err(|error| error.to_string())
        })
        .unwrap();
    {
        let mut terminal = app.world_mut().get_mut::<TerminalPanel>(panel).unwrap();
        terminal.runtime = Some(Runtime {
            shell_token: None,
            pty,
            task,
            pending_inputs: VecDeque::new(),
            size,
            theme: None,
        });
        terminal.status = TerminalStatus::Starting;
    }
    eventually(&mut app, |app| {
        matches!(
            app.world().get::<TerminalPanel>(panel).unwrap().status(),
            TerminalStatus::Failed(_)
        )
    });
    let pid = app
        .world()
        .get::<TerminalPanel>(panel)
        .unwrap()
        .process_id();
    app.world_mut()
        .get_mut::<TerminalPanel>(panel)
        .unwrap()
        .restart();
    app.update();
    let terminal = app.world().get::<TerminalPanel>(panel).unwrap();
    assert!(
        matches!(terminal.status(), TerminalStatus::Failed(error) if error.contains("restart refused"))
    );
    assert_eq!(
        terminal.process_id(),
        pid,
        "Unconfirmed owner cleanup cannot admit another child"
    );
    finish(&mut app);
}

#[cfg(unix)]
#[test]
fn stop_bypasses_an_input_byte_budget_blocked_write_without_waiting_for_the_child() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let (mut app, workspace) = host();
    let panel = panel(&mut app, workspace, script("sleep 30"));
    let size = WindowSize {
        num_cols: 80,
        num_lines: 24,
        cell_width: 8,
        cell_height: 16,
    };
    let options = app
        .world()
        .get::<TerminalPanel>(panel)
        .unwrap()
        .options
        .clone();
    let (pty, worker) =
        TerminalPty::prepare_terminal(options, size, Terminal::new_text_only(80, 24)).unwrap();
    let chunk = vec![b'x'; bedtime_terminal::terminal_pty::MAX_WRITE_BYTES];
    for _ in 0..4 {
        pty.write(&chunk).unwrap();
    }
    assert_eq!(
        pty.write(b"blocked").unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
    let start = Arc::new(AtomicBool::new(false));
    let start_worker = start.clone();
    let task = app
        .world_mut()
        .get_mut::<WorkspaceWork>(workspace.entity())
        .unwrap()
        .spawn_task(Some(panel), "Delayed terminal", move |cancel| {
            while !start_worker.load(Ordering::Acquire) && !cancel.wait(Duration::from_millis(1)) {}
            worker
                .run(|| cancel.is_cancelled())
                .map_err(|error| error.to_string())
        })
        .unwrap();
    {
        let mut terminal = app.world_mut().get_mut::<TerminalPanel>(panel).unwrap();
        terminal.runtime = Some(Runtime {
            shell_token: None,
            pty,
            task,
            pending_inputs: VecDeque::new(),
            size,
            theme: None,
        });
        terminal.status = TerminalStatus::Starting;
        send_inputs(
            &mut terminal,
            vec![TerminalInput::Command(TerminalCommand::Text(
                "blocked".into(),
            ))],
        );
        assert_eq!(terminal.runtime.as_ref().unwrap().pending_inputs.len(), 1);
        terminal.stop();
    }
    app.update();
    let terminal = app.world().get::<TerminalPanel>(panel).unwrap();
    assert_eq!(terminal.status(), &TerminalStatus::Stopping);
    assert!(
        terminal.runtime.as_ref().unwrap().pending_inputs.is_empty(),
        "Stop must be admitted despite exhausted user-input byte reservations"
    );
    start.store(true, Ordering::Release);
    eventually(&mut app, |app| {
        matches!(
            app.world().get::<TerminalPanel>(panel).unwrap().status(),
            TerminalStatus::Exited(_)
        )
    });
    finish(&mut app);
}

#[test]
fn shell_registration_is_atomic_and_ordinary_launches_are_not_helpers() {
    fn unused(_: &mut World, _: ShellInvocation) {}
    let (mut app, _) = host();
    assert!(
        app.add_shell_command(["+custom", "+../invalid"], unused)
            .is_err()
    );
    app.add_shell_command(["+custom", "+alias"], unused)
        .unwrap();
    assert!(app.add_shell_command(["+new", "+custom"], unused).is_err());
    app.add_shell_command(["+new"], unused).unwrap();
    assert!(run_shell_command(["bed2".into(), "folder".into()]).is_none());
    assert!(run_shell_command(["bed2".into(), "+open".into()]).is_none());
    use std::os::unix::ffi::OsStringExt;
    let error = run_shell_command([
        "/tmp/helpers/+custom".into(),
        std::ffi::OsString::from_vec(b"bad\0argument".to_vec()),
    ])
    .unwrap()
    .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    app.update();
    assert!(app.add_shell_command(["+late"], unused).is_err());
    finish(&mut app);
}

#[derive(Resource, Default)]
struct ShellObservations(Vec<ShellInvocation>);

fn capture_shell(world: &mut World, invocation: ShellInvocation) {
    world.resource_mut::<ShellObservations>().0.push(invocation);
}

#[cfg(unix)]
fn shell_client(app: &App, alias: &str, token: &str) -> std::os::unix::net::UnixStream {
    use std::io::Write;
    let mut stream =
        std::os::unix::net::UnixStream::connect(shell_socket_path(app.world()).unwrap()).unwrap();
    // Native byte arrays exercise non-UTF-8 cwd/arguments without environment
    // mutation or invoking the test executable as a desktop application.
    writeln!(stream, r#"{{"command":"invoke","alias":"{alias}","arguments":[[97,255],[]],"directory":[47,116,109,112,255],"pane":"{token}"}}"#).unwrap();
    stream.set_nonblocking(true).unwrap();
    stream
}

#[cfg(unix)]
fn shell_reply(app: &mut App, stream: &mut std::os::unix::net::UnixStream) -> String {
    use std::io::Read;
    let mut bytes = Vec::new();
    eventually(app, |_| {
        let mut chunk = [0; 4096];
        match stream.read(&mut chunk) {
            Ok(0) => panic!("Helper closed without a reply"),
            Ok(n) => bytes.extend_from_slice(&chunk[..n]),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) => panic!("Helper reply failed: {error}"),
        }
        bytes.last() == Some(&b'\n')
    });
    String::from_utf8(bytes).unwrap()
}

#[cfg(unix)]
#[test]
fn shell_commands_preserve_bytes_origin_and_defer_acknowledgement() {
    use std::{io::Read, os::unix::ffi::OsStrExt};
    let (mut app, workspace) = host();
    app.init_resource::<ShellObservations>();
    app.add_shell_command(["+custom", "+alias"], capture_shell)
        .unwrap();
    let panel = panel(
        &mut app,
        workspace,
        script("printf '%s\\n' \"$BEDTERM_SOCKET\" \"$BEDTERM_PANE\"; while read line; do :; done"),
    );
    observe(&mut app, panel);
    eventually(&mut app, |app| {
        app.world().get::<TerminalPanel>(panel).unwrap().status() == &TerminalStatus::Running
    });
    let token = app
        .world()
        .get::<TerminalPanel>(panel)
        .unwrap()
        .runtime
        .as_ref()
        .unwrap()
        .shell_token
        .clone()
        .unwrap();
    let socket = shell_socket_path(app.world()).unwrap().to_owned();
    eventually(&mut app, |app| {
        observe(app, panel);
        let output = text(app, panel).replace('\n', "");
        output.contains(socket.to_str().unwrap()) && output.contains(&token)
    });
    assert!(
        app.world()
            .get::<TerminalPanel>(panel)
            .unwrap()
            .options
            .env
            .is_empty(),
        "Bridge environment is applied to the child launch, not saved options"
    );
    let other = Workspaces::new(app.world_mut()).open("Other");
    Workspaces::new(app.world_mut()).activate(other).unwrap();
    let mut client = shell_client(&app, "+alias", &token);
    eventually(&mut app, |app| {
        !app.world().resource::<ShellObservations>().0.is_empty()
    });
    let invocation = app
        .world_mut()
        .resource_mut::<ShellObservations>()
        .0
        .pop()
        .unwrap();
    assert_eq!(invocation.panel, panel);
    assert_eq!(invocation.workspace, workspace.entity());
    assert_eq!(invocation.alias, "+alias");
    assert_eq!(invocation.directory.as_os_str().as_bytes(), b"/tmp\xff");
    assert_eq!(invocation.arguments[0].as_os_str().as_bytes(), b"a\xff");
    assert!(invocation.arguments[1].is_empty());
    let mut byte = [0];
    assert_eq!(
        client.read(&mut byte).unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
    invocation
        .reply
        .complete(Err("Application open failed".into()));
    assert!(shell_reply(&mut app, &mut client).contains("Application open failed"));
    finish(&mut app);
    drop(app);
    assert!(!socket.exists());
}

#[cfg(unix)]
#[test]
fn restarted_or_closed_origins_and_abandoned_commands_fail_explicitly() {
    let (mut app, workspace) = host();
    app.init_resource::<ShellObservations>();
    app.add_shell_command(["+custom"], capture_shell).unwrap();
    let panel = panel(&mut app, workspace, script("while read line; do :; done"));
    observe(&mut app, panel);
    eventually(&mut app, |app| {
        app.world().get::<TerminalPanel>(panel).unwrap().status() == &TerminalStatus::Running
    });
    let old = app
        .world()
        .get::<TerminalPanel>(panel)
        .unwrap()
        .runtime
        .as_ref()
        .unwrap()
        .shell_token
        .clone()
        .unwrap();
    app.world_mut()
        .get_mut::<TerminalPanel>(panel)
        .unwrap()
        .restart();
    eventually(&mut app, |app| {
        observe(app, panel);
        app.world()
            .get::<TerminalPanel>(panel)
            .unwrap()
            .runtime
            .as_ref()
            .is_some_and(|r| r.shell_token.as_deref().is_some_and(|token| token != old))
            && app.world().get::<TerminalPanel>(panel).unwrap().status() == &TerminalStatus::Running
    });
    let mut stale = shell_client(&app, "+custom", &old);
    assert!(shell_reply(&mut app, &mut stale).contains("closed or restarted"));
    assert!(app.world().resource::<ShellObservations>().0.is_empty());
    let token = app
        .world()
        .get::<TerminalPanel>(panel)
        .unwrap()
        .runtime
        .as_ref()
        .unwrap()
        .shell_token
        .clone()
        .unwrap();
    let mut client = shell_client(&app, "+custom", &token);
    eventually(&mut app, |app| {
        !app.world().resource::<ShellObservations>().0.is_empty()
    });
    app.world_mut()
        .resource_mut::<ShellObservations>()
        .0
        .clear();
    assert!(shell_reply(&mut app, &mut client).contains("abandoned"));
    let mut unknown = shell_client(&app, "+unknown", &token);
    assert!(shell_reply(&mut app, &mut unknown).contains("Unknown shell command"));
    app.world_mut().despawn(panel);
    let mut closed = shell_client(&app, "+custom", &token);
    assert!(shell_reply(&mut app, &mut closed).contains("closed or restarted"));
    finish(&mut app);
}
