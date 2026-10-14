#[path = "../../../tests/support/native_smoke.rs"]
mod support;
use support::*;

fn main() -> io::Result<()> {
    let verified = Arc::<AtomicBool>::default();
    let launch = match bedtime::awaken("example.git", "git smoke")?.dispatch()? {
        bedtime::LaunchOutcome::Handled(exit) => std::process::exit(exit),
        bedtime::LaunchOutcome::Desktop(launch) => *launch,
    };
    let options = bedtime::DesktopOptions {
        config_directory: Some(
            std::env::temp_dir().join(format!("bed-git-config-{}", std::process::id())),
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
            bedtime_plugin_git::GitPlugin,
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
        return Err(io::Error::other("git smoke failed"));
    }
    Ok(())
}
fn advance(world: &mut World) {
    world.resource_scope(|world, mut smoke: Mut<Smoke>| {
        let result = (|| -> io::Result<bool> {
            ensure(
                smoke.started.elapsed() < Duration::from_secs(90),
                format!("git smoke timed out in phase {}", smoke.phase),
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
            git(world, &mut smoke)
        })();
        match result {
            Ok(true) => {
                smoke.verified.store(true, Ordering::SeqCst);
                eprintln!("git smoke PASS");
                world.write_message(AppExit::Success);
            }
            Ok(false) => {}
            Err(error) => {
                eprintln!("git smoke failed: {error}");
                world.write_message(AppExit::error());
            }
        }
    });
}
fn git(world: &mut World, smoke: &mut Smoke) -> io::Result<bool> {
    match smoke.phase {
        0 => {
            if smoke.fixture.is_none() {
                let fixture = Fixture::new("git")?;
                fixture.write("main.rs",b"fn greeting(name: &str) {\n    println!(\"Hello, {name}!\");\n}\n\nfn main() {\n    greeting(\"world\");\n}\n")?;
                for args in [
                    vec!["init", "-q"],
                    vec!["config", "user.name", "bEd Smoke"],
                    vec!["config", "user.email", "smoke@example.invalid"],
                    vec!["config", "core.hooksPath", ".git/hooks"],
                    vec!["add", "main.rs"],
                    vec!["-c", "commit.gpgsign=false", "commit", "-qm", "Initial"],
                ] {
                    let output = std::process::Command::new("git")
                        .current_dir(&fixture.0)
                        .args(args)
                        .output()?;
                    ensure(
                        output.status.success(),
                        String::from_utf8_lossy(&output.stderr),
                    )?;
                }
                fixture.write("main.rs",b"fn greeting(name: &str) {\n    println!(\"Welcome, {name}!\");\n}\n\nfn main() {\n    greeting(\"world\");\n}\n")?;
                ensure(
                    std::process::Command::new("git")
                        .current_dir(&fixture.0)
                        .args(["add", "main.rs"])
                        .status()?
                        .success(),
                    "git stage fixture failed",
                )?;
                fixture.write("main.rs",b"fn greeting(name: &str) {\n    println!(\"Welcome, {name}!\");\n}\n\nfn main() {\n    let name = \"bEd\";\n    greeting(name);\n}\n")?;
                fixture.write("notes.txt", b"A new file, ready to stage.\n")?;
                smoke.fixture = Some(fixture);
            }
            if fixture_directory(world, smoke, "git")? {
                world
                    .resource_mut::<bedtime_plugin_git::GitTarget>()
                    .session_id = Some(
                    world
                        .non_send::<bedtime_documents::DocumentSession>()
                        .session_id(),
                );
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
            if smoke
                .operation(
                    world,
                    Operation::InvokeCommand {
                        command: bedtime_plugin_git::SHOW_COMMAND.into(),
                        source: smoke.panel,
                        arguments: Value::Null,
                    },
                )?
                .is_some()
            {
                smoke.next(world);
            }
        }
        3 | 4 => {
            let side = if smoke.phase == 3 {
                "staged"
            } else {
                "unstaged"
            };
            if let Some(OperationValue::Opened(panel)) = smoke.operation(
                world,
                Operation::OpenPanel {
                    panel_type: bedtime_plugin_git::DIFF_PANEL_ID.into(),
                    state: json!({"path":"main.rs","side":side}),
                    area: None,
                },
            )? {
                if smoke.phase == 3 {
                    smoke.other = Some(panel);
                } else {
                    smoke.panel = Some(panel);
                }
                smoke.next(world);
            }
        }
        5 => {
            if smoke.ready(smoke.panel.unwrap())? && smoke.ready(smoke.other.unwrap())? {
                ensure(
                    smoke.panels(bedtime_plugin_git::DIFF_PANEL_ID).len() == 2,
                    "Git comparisons did not load both sides",
                )?;
                if smoke
                    .operation(world, Operation::FocusPanel(smoke.panel.unwrap()))?
                    .is_some()
                {
                    smoke.next(world);
                }
            }
        }
        6 => {
            let staged = binding(world, smoke.other.unwrap())?
                .document
                .ok_or_else(|| io::Error::other("staged comparison has no document"))?;
            let working = binding(world, smoke.panel.unwrap())?
                .document
                .ok_or_else(|| io::Error::other("working comparison has no document"))?;
            {
                let session = world.non_send::<bedtime_documents::DocumentSession>();
                ensure(
                    session.is_snapshot_document(staged) && !session.is_snapshot_document(working),
                    format!(
                        "Git comparisons did not retain read-only index and editable working documents: staged {:?}, working {:?}",
                        session.snapshot(staged)?.path,
                        session.snapshot(working)?.path
                    ),
                )?;
                let staged_bytes = session.snapshot(staged)?.bytes;
                let working_bytes = session.snapshot(working)?.bytes;
                ensure(
                    staged_bytes
                        .windows(b"Welcome".len())
                        .any(|bytes| bytes == b"Welcome")
                        && !staged_bytes
                            .windows(b"let name".len())
                            .any(|bytes| bytes == b"let name")
                        && working_bytes
                            .windows(b"let name = \"bEd\"".len())
                            .any(|bytes| bytes == b"let name = \"bEd\""),
                    "Git comparisons displayed the wrong revisions",
                )?;
            }
            if let Some(frame) = smoke.capture(
                world,
                vec![
                    CaptureTarget::Window,
                    CaptureTarget::Panel(smoke.panel.unwrap()),
                ],
            )? {
                ensure(
                    visible_pixels(&frame.images[1].rgba.pixels) > 1000,
                    "Git comparison render is blank",
                )?;
                std::fs::create_dir_all("target")?;
                frame.images[0].rgba.write_png("target/git-smoke.png")?;
                return Ok(true);
            }
        }
        _ => unreachable!(),
    }
    Ok(false)
}
