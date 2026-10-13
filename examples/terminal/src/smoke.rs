#[path = "../../../tests/support/native_smoke.rs"]
mod support;
use bed_desktop::{DesktopInjectedEvent, DesktopInputStep};
use std::collections::HashSet;
use support::*;

#[derive(Resource, Default)]
struct LoggedPhase(Option<u32>);

fn main() -> io::Result<()> {
    let verified = Arc::<AtomicBool>::default();
    let launch = match bed_launch::builder("example.terminal.smoke", "Terminal smoke")?
        .with_launchers(bed_plugin_terminal::launchers())
        .dispatch()?
    {
        bed_launch::LaunchOutcome::Handled(exit) => std::process::exit(exit),
        bed_launch::LaunchOutcome::Desktop(launch) => *launch,
    };
    let options = bed_launch::DesktopOptions {
        config_directory: Some(
            std::env::temp_dir().join(format!("bed-terminal-native-config-{}", std::process::id())),
        ),
        desktop: bed_desktop::DesktopPlugins {
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
            bed_plugin_terminal::TerminalPlugin,
            bed_plugin_terminal::TerminalAppIntegrationPlugin,
            bed_plugin_csv_table::CsvTablePlugin,
            bed_plugin_image_viewer::ImageViewerPlugin,
            bed_plugin_automation::AutomationPlugin,
        ))
        .insert_resource(Smoke {
            verified: Arc::clone(&verified),
            ..Default::default()
        })
        .init_resource::<LoggedPhase>()
        .add_systems(Last, advance.before(bevy::app::OnAppExitSystems));
        Ok(())
    })?;
    ensure(
        verified.load(Ordering::SeqCst),
        "native smoke exited before completing its assertions",
    )?;
    if exit.is_error() {
        return Err(io::Error::other("Terminal smoke failed"));
    }
    Ok(())
}
fn advance(world: &mut World) {
    world.resource_scope(|world, mut smoke: Mut<Smoke>| {
        let result = (|| -> io::Result<bool> {
            ensure(
                smoke.started.elapsed() < Duration::from_secs(90),
                format!(
                    "Terminal smoke timed out in phase {}: {}",
                    smoke.phase,
                    terminal_diagnostics(world, &smoke)
                ),
            )?;
            if !world.contains_resource::<bed_desktop::DesktopGpu>() {
                return Ok(false);
            }
            smoke.poll(world)?;
            let events = &world
                .resource::<bed_desktop::DesktopInjectedInputEvents>()
                .0;
            ensure(
                events
                    .iter()
                    .filter(|event| {
                        matches!(
                            &event.event,
                            DesktopInjectedEvent::Text(_) | DesktopInjectedEvent::Key { .. }
                        )
                    })
                    .all(|event| event.consumed),
                "terminal did not consume its injected text/key batch",
            )?;
            if smoke.snapshot.is_none() {
                return Ok(false);
            }
            if presented(world) < 2 {
                focus_window(world);
                return Ok(false);
            }
            if world.resource::<LoggedPhase>().0 != Some(smoke.phase) {
                world.resource_mut::<LoggedPhase>().0 = Some(smoke.phase);
                eprintln!(
                    "Terminal smoke phase {}: {}",
                    smoke.phase,
                    terminal_diagnostics(world, &smoke)
                );
            }
            if !native_window(world, |window| window.has_focus())? {
                focus_window(world);
            }
            terminal(world, &mut smoke)
        })();
        match result {
            Ok(true) => {
                smoke.verified.store(true, Ordering::SeqCst);
                eprintln!("Terminal smoke PASS");
                world.write_message(AppExit::Success);
            }
            Ok(false) => {}
            Err(error) => {
                eprintln!("Terminal smoke failed: {error}");
                world.write_message(AppExit::error());
            }
        }
    });
}
fn terminal(world: &mut World, smoke: &mut Smoke) -> io::Result<bool> {
    use bed_terminal::bed_terminal::BedTerminal;
    match smoke.phase {
        0 => {
            if smoke.fixture.is_none() {
                smoke.fixture = Some(Fixture::new("terminal")?);
                let mut terminal = world.non_send_mut::<BedTerminal>();
                terminal.configure_shell(bed_terminal::terminal_pty::TerminalShell::new(
                    "/bin/sh",
                    vec!["-i".into()],
                ));
                terminal.configure_shell_environment(std::collections::HashMap::from([
                    ("ENV".into(), String::new()),
                    ("BASH_ENV".into(), String::new()),
                    ("PS1".into(), "bed smoke> ".into()),
                    ("PS2".into(), "> ".into()),
                ]));
            }
            let panels = smoke.snapshot.as_ref().unwrap().panels.clone();
            if let Some(panel) = panels.first() {
                if smoke
                    .operation(world, Operation::ClosePanel(panel.handle))?
                    .is_some()
                {
                    smoke.snapshot = None;
                }
            } else {
                smoke.next(world);
            }
        }
        1 => {
            let cwd = smoke
                .fixture
                .as_ref()
                .unwrap()
                .0
                .to_string_lossy()
                .into_owned();
            if let Some(OperationValue::Opened(panel)) = smoke.operation(
                world,
                Operation::OpenPanel {
                    panel_type: "bed.terminal.panel".into(),
                    state: json!({"cwd":cwd}),
                    area: None,
                },
            )? {
                smoke.panel = Some(panel);
                focus_window(world);
                smoke.next(world);
            }
        }
        2 => {
            if smoke.elapsed_frames(world) >= 4
                && world.non_send::<BedTerminal>().is_focused()
                && native_window(world, |window| window.has_focus())?
            {
                let geometry = smoke
                    .snapshot
                    .as_ref()
                    .unwrap()
                    .panels
                    .iter()
                    .find(|p| Some(p.handle) == smoke.panel)
                    .and_then(|p| p.visible_content)
                    .ok_or_else(|| io::Error::other("initial terminal geometry missing"))?;
                ensure(
                    geometry.max[0] - geometry.min[0] > 900.
                        && geometry.max[1] - geometry.min[1] > 500.,
                    "initial terminal did not fill native window",
                )?;
                if (smoke.operation.is_some()
                    || terminal_panel_focused(world, smoke.panel.unwrap())?)
                    && smoke
                        .operation(
                            world,
                            Operation::Input(terminal_input(terminal_fixture_command())),
                        )?
                        .is_some()
                {
                    smoke.next(world);
                }
            }
        }
        3 => {
            if (smoke.operation.is_some() || terminal_fixture_ready(world)?)
                && let Some(OperationValue::Opened(panel)) = smoke.operation(
                    world,
                    Operation::OpenPanel {
                        panel_type: "bed.terminal.panel".into(),
                        state: json!({"cwd":smoke.fixture.as_ref().unwrap().0}),
                        area: None,
                    },
                )?
            {
                smoke.panel = Some(panel);
                smoke.next(world);
            }
        }
        4 => {
            // Shared splitting moves a tab. Opening the shell explicitly keeps
            // this reusable fixture independent of bEd's duplicate-panel policy.
            if smoke
                .operation(
                    world,
                    Operation::SplitPanel {
                        panel: smoke.panel.unwrap(),
                        axis: 0,
                        high: true,
                    },
                )?
                .is_some()
            {
                smoke.next(world);
            }
        }
        5 => {
            if smoke.panels("bed.terminal.panel").len() == 2
                && (smoke.operation.is_some()
                    || terminal_panel_focused(world, smoke.panel.unwrap())?)
                && smoke
                    .operation(
                        world,
                        Operation::Input(terminal_input(terminal_fixture_command())),
                    )?
                    .is_some()
            {
                smoke.next(world);
            }
        }
        6 => {
            if (smoke.operation.is_some() || terminal_fixture_ready(world)?)
                && let Some(OperationValue::Opened(panel)) = smoke.operation(
                    world,
                    Operation::OpenPanel {
                        panel_type: "bed.terminal.panel".into(),
                        state: json!({"cwd":smoke.fixture.as_ref().unwrap().0}),
                        area: None,
                    },
                )?
            {
                smoke.panel = Some(panel);
                smoke.next(world);
            }
        }
        7 => {
            if smoke
                .operation(
                    world,
                    Operation::SplitPanel {
                        panel: smoke.panel.unwrap(),
                        axis: 1,
                        high: true,
                    },
                )?
                .is_some()
            {
                smoke.next(world);
            }
        }
        8 => {
            if smoke.panels("bed.terminal.panel").len() == 3
                && (smoke.operation.is_some()
                    || terminal_panel_focused(world, smoke.panel.unwrap())?)
                && smoke
                    .operation(
                        world,
                        Operation::Input(terminal_input(terminal_fixture_command())),
                    )?
                    .is_some()
            {
                smoke.next(world);
            }
        }
        9 => {
            if terminal_fixture_ready(world)? {
                verify_terminal_splits(world, smoke)?;
                smoke.other = smoke.panel;
                let f = smoke.fixture.as_ref().unwrap();
                f.write("results.csv", b"name,value\nred,1\nblue,2\n")?;
                f.write("chart.svg",br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><rect width="100" height="100" fill="#ff8000"/></svg>"##)?;
                smoke.next(world);
            }
        }
        10 => {
            if smoke.open(
                world,
                smoke.fixture.as_ref().unwrap().0.join("results.csv"),
                None,
            )? {
                smoke.next(world);
            }
        }
        11 => {
            if smoke.open(
                world,
                smoke.fixture.as_ref().unwrap().0.join("chart.svg"),
                None,
            )? {
                smoke.next(world);
            }
        }
        12 => {
            if smoke.ready(smoke.panel.unwrap())? {
                ensure(
                    smoke.panels("bed.csv.panel").len() == 1
                        && smoke.panels("bed.image.panel").len() == 1,
                    "terminal companions missing",
                )?;
                if smoke
                    .operation(world, Operation::FocusPanel(smoke.other.unwrap()))?
                    .is_some()
                {
                    smoke.panel = smoke.other;
                    smoke.next(world);
                }
            }
        }
        13 => {
            if smoke.elapsed_frames(world) >= 2
                && terminal_fixture_ready(world)?
                && let Some(frame) = smoke.capture(
                    world,
                    vec![
                        CaptureTarget::Window,
                        CaptureTarget::Panel(smoke.panel.unwrap()),
                    ],
                )?
            {
                verify_terminal_capture(&frame, smoke.panel.unwrap())?;
                std::fs::create_dir_all("target")?;
                frame.images[0]
                    .rgba
                    .write_ppm("target/native-terminal.ppm")?;
                return Ok(true);
            }
        }
        _ => unreachable!(),
    }
    Ok(false)
}
fn verify_terminal_capture(
    frame: &bed_desktop::DesktopCapturedFrame,
    panel: PanelHandle,
) -> io::Result<()> {
    let image = frame
        .images
        .iter()
        .find(|image| image.target == bed_desktop::DesktopCaptureTarget::Panel(panel.entity()))
        .ok_or_else(|| io::Error::other("terminal panel capture is missing"))?;
    let width = image.rgba.size[0] as usize;
    // Exclude the cwd/header region. Only terminal glyphs and the fixture's
    // red Kitty image in the body can satisfy this rendered-output check.
    let first_row = (48.0 * frame.info.framebuffer_scale[1]).ceil() as usize;
    let mut text = 0;
    let mut image_pixels = 0;
    for row in image.rgba.pixels.chunks_exact(width * 4).skip(first_row) {
        for pixel in row.as_chunks::<4>().0 {
            let [r, g, b, _] = *pixel;
            if r.min(g).min(b) > 110 && r.max(g).max(b) - r.min(g).min(b) < 70 {
                text += 1;
            }
            if r > 160 && g < 90 && b < 90 {
                image_pixels += 1;
            }
        }
    }
    ensure(
        text > 400 && image_pixels > 20,
        format!(
            "terminal body did not render fixture glyphs/image: {text} text pixels, {image_pixels} red image pixels"
        ),
    )
}

fn terminal_input(text: &str) -> Vec<DesktopInputStep> {
    use winit::keyboard::{Key, KeyCode, NamedKey};
    vec![
        DesktopInputStep::from(vec![DesktopInjectedEvent::Text(
            text.trim_end_matches('\n').into(),
        )]),
        DesktopInputStep::from(vec![DesktopInjectedEvent::Key {
            physical: KeyCode::Enter,
            logical: Key::Named(NamedKey::Enter),
            pressed: true,
        }]),
        DesktopInputStep::from(vec![DesktopInjectedEvent::Key {
            physical: KeyCode::Enter,
            logical: Key::Named(NamedKey::Enter),
            pressed: false,
        }]),
    ]
}

fn terminal_panel_focused(world: &mut World, panel: PanelHandle) -> io::Result<bool> {
    let session = binding(world, panel)?.terminal;
    let focused = {
        let terminal = world.non_send::<bed_terminal::bed_terminal::BedTerminal>();
        terminal.is_focused() && terminal.active_session_id() == session
    };
    Ok(focused && native_window(world, |window| window.has_focus())?)
}

fn terminal_diagnostics(world: &mut World, smoke: &Smoke) -> String {
    let terminal = world
        .get_non_send::<bed_terminal::bed_terminal::BedTerminal>()
        .map(|terminal| {
            (
                terminal.is_focused(),
                terminal.active_session_id(),
                terminal.session_ids(),
            )
        });
    let panels = smoke
        .panels("bed.terminal.panel")
        .into_iter()
        .map(|panel| (panel, binding(world, panel).map(|binding| binding.terminal)))
        .collect::<Vec<_>>();
    let native_focus = native_window(world, |window| window.has_focus()).ok();
    format!(
        "native_focus={native_focus:?}, terminal=(focused,active,sessions){terminal:?}, requested={:?}, panel_bindings={panels:?}, operation={:?}",
        smoke.panel, smoke.operation,
    )
}

fn terminal_fixture_command() -> &'static str {
    "printf '\\033[2J\\033[H\\r\\nGraphemes: é 日本語 👩‍💻\\r\\n\\033[4:3mcurly\\033[4:2m double\\033[0m\\r\\nAGENTS.md\\tNOTICE\\t\\033[34mvendor\\033[0m\\r\\n\\033]8;;https://example.com\\033\\\\OSC 8 link\\033]8;;\\033\\\\\\r\\n\\033_Ga=T,f=32,s=1,v=1,c=6,r=3,i=1,q=2;/wAA/w==\\033\\\\\\r\\nTERMINAL_GPU_READY\\r\\n'\n"
}

fn terminal_fixture_text_ready(text: &str) -> bool {
    ["é", "日本語", "👩‍💻", "TERMINAL_GPU_READY"]
        .iter()
        .all(|value| text.contains(value))
}

fn terminal_fixture_ready(world: &World) -> io::Result<bool> {
    let terminal = world.non_send::<bed_terminal::bed_terminal::BedTerminal>();
    let Some(snapshot) = terminal.active_terminal() else {
        return Ok(false);
    };
    let text = snapshot.selection_text().unwrap_or_else(|| {
        snapshot
            .lines
            .iter()
            .flat_map(|r| r.cells.iter())
            .filter(|c| c.width != 0)
            .map(|c| c.text.as_ref())
            .collect()
    });
    Ok(terminal_fixture_text_ready(&text)
        && !snapshot.images.is_empty()
        && snapshot
            .lines
            .iter()
            .flat_map(|r| r.cells.iter())
            .any(|c| c.hyperlink.as_deref() == Some("https://example.com")))
}

fn verify_terminal_splits(world: &World, smoke: &Smoke) -> io::Result<()> {
    let terminal = world.non_send::<bed_terminal::bed_terminal::BedTerminal>();
    let processes = terminal
        .session_ids()
        .into_iter()
        .filter_map(|id| terminal.process_id(id))
        .collect::<HashSet<_>>();
    let areas = smoke
        .snapshot
        .as_ref()
        .unwrap()
        .panels
        .iter()
        .filter(|p| p.panel_type == "bed.terminal.panel")
        .filter_map(|p| p.area)
        .collect::<HashSet<_>>();
    let docks = smoke
        .snapshot
        .as_ref()
        .unwrap()
        .panels
        .iter()
        .filter(|panel| panel.panel_type == "bed.terminal.panel")
        .filter_map(|panel| {
            world.get::<bed_api::workbench::PanelPresentation>(panel.handle.entity())
        })
        .map(|panel| panel.dock_id)
        .collect::<HashSet<_>>();
    ensure(
        terminal.session_count() == 3
            && processes.len() == 3
            && areas.len() == 3
            && docks.len() == 3
            && !docks.contains(&0),
        "terminal splits did not create three independent docked shells/leaves",
    )
}
