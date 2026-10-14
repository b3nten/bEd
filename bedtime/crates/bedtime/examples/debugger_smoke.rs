#[path = "../../../tests/support/native_smoke.rs"]
mod support;
use support::*;

fn main() -> io::Result<()> {
    let verified = Arc::<AtomicBool>::default();
    let launch = match bedtime::awaken("example.debugger", "debugger smoke")?
        .with_launchers(bedtime_plugin_debugger::launchers())
        .with_launchers(bedtime_plugin_terminal::launchers())
        .dispatch()?
    {
        bedtime::LaunchOutcome::Handled(exit) => std::process::exit(exit),
        bedtime::LaunchOutcome::Desktop(launch) => *launch,
    };
    let options = bedtime::DesktopOptions {
        config_directory: Some(
            std::env::temp_dir().join(format!("bed-debugger-config-{}", std::process::id())),
        ),
        desktop: bedtime_desktop::DesktopPlugins {
            window: Window {
                resolution: (1200, 800).into(),
                ..default()
            },
            ..default()
        },
        ..Default::default()
    };
    let exit = launch.run(options, |app| {
        app.add_plugins((
            bedtime_plugin_terminal::TerminalPlugin,
            bedtime_plugin_debugger::DebuggerPlugin,
            bedtime_plugin_editor::EditorPlugin,
            bedtime_plugin_automation::AutomationPlugin,
        ))
        .insert_resource(Smoke {
            verified: Arc::clone(&verified),
            ..Default::default()
        })
        .add_systems(Last, advance.before(bevy::app::OnAppExitSystems));
        Ok(())
    })?;
    ensure(
        verified.load(Ordering::SeqCst),
        "native smoke exited before completing its assertions",
    )?;
    if exit.is_error() {
        return Err(io::Error::other("debugger smoke failed"));
    }
    Ok(())
}
fn advance(world: &mut World) {
    world.resource_scope(|world, mut smoke: Mut<Smoke>| {
        let result = (|| -> io::Result<bool> {
            ensure(
                smoke.started.elapsed() < Duration::from_secs(90),
                format!("debugger smoke timed out in phase {}", smoke.phase),
            )?;
            if !world.contains_resource::<bedtime_desktop::DesktopGpu>() {
                return Ok(false);
            }
            smoke.poll(world)?;
            if smoke.snapshot.is_none() {
                return Ok(false);
            }
            if presented(world) < 2 {
                focus_window(world);
                return Ok(false);
            }
            debug(world, &mut smoke)
        })();
        match result {
            Ok(true) => {
                smoke.verified.store(true, Ordering::SeqCst);
                eprintln!("debugger smoke PASS");
                world.write_message(AppExit::Success);
            }
            Ok(false) => {}
            Err(error) => {
                eprintln!("debugger smoke failed: {error}");
                world.write_message(AppExit::error());
            }
        }
    });
}
fn debug(world: &mut World, smoke: &mut Smoke) -> io::Result<bool> {
    match smoke.phase {
        0 => {
            if fixture_directory(world, smoke, "debug")? {
                let directory = smoke
                    .fixture
                    .as_ref()
                    .unwrap()
                    .0
                    .to_string_lossy()
                    .into_owned();
                *world.resource_mut::<bedtime_plugin_debugger::DebugTarget>() =
                    bedtime_plugin_debugger::DebugTarget {
                        directory,
                        remote: false,
                    };
                smoke.fixture.as_ref().unwrap().write("main.rs",b"fn main() {\n    let number = 42;\n    println!(\"debug smoke {number}\");\n}\n")?;
                smoke.next(world);
            }
        }
        1 => {
            if smoke.open(
                world,
                smoke.fixture.as_ref().unwrap().0.join("main.rs"),
                None,
            )? {
                smoke.next(world);
            }
        }
        2 => {
            let root = &smoke.fixture.as_ref().unwrap().0;
            let executable = root.join("debug-fixture");
            let profile = bedtime_debugger::DebugProfile {
                name: "Native smoke".into(),
                program: executable.to_string_lossy().into_owned(),
                build_command: format!(
                    "rustc -g --edition=2024 main.rs -o '{}'",
                    executable.display()
                ),
                cwd: root.to_string_lossy().into_owned(),
                stop_on_entry: true,
                ..default()
            };
            {
                let mut debugger = world.non_send_mut::<bedtime_plugin_debugger::DebugState>();
                debugger.0.profiles = vec![profile];
                debugger.0.selected = 0;
            }
            smoke.next(world);
        }
        3 => {
            if smoke
                .operation(
                    world,
                    Operation::InvokeCommand {
                        command: bedtime_plugin_debugger::BREAKPOINT_COMMAND.into(),
                        source: smoke.panel,
                        arguments: Value::Null,
                    },
                )?
                .is_some()
            {
                smoke.next(world);
            }
        }
        4 => {
            if smoke
                .operation(
                    world,
                    Operation::InvokeCommand {
                        command: bedtime_plugin_debugger::START_COMMAND.into(),
                        source: smoke.panel,
                        arguments: Value::Null,
                    },
                )?
                .is_some()
            {
                smoke.next(world);
            }
        }
        5 => {
            if world
                .non_send::<bedtime_plugin_debugger::DebugState>()
                .0
                .debug_smoke_ready()?
            {
                focus_window(world);
                smoke.next(world);
            }
        }
        6 => {
            if let Some(frame) = smoke.capture(world, vec![CaptureTarget::Window])? {
                ensure(
                    visible_pixels(&frame.images[0].rgba.pixels) > 1000,
                    "debugger capture is blank",
                )?;
                std::fs::create_dir_all("target")?;
                frame.images[0]
                    .rgba
                    .write_png("target/debugger-smoke.png")?;
                smoke.next(world);
            }
        }
        7 => {
            if smoke
                .operation(
                    world,
                    Operation::InvokeCommand {
                        command: bedtime_plugin_debugger::STOP_COMMAND.into(),
                        source: smoke.panel,
                        arguments: Value::Null,
                    },
                )?
                .is_some()
            {
                return Ok(true);
            }
        }
        _ => unreachable!(),
    }
    Ok(false)
}
