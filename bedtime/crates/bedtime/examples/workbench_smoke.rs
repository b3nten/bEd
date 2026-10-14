//! Minimal native application proof for plugin status contributions and shared UI configuration.
#[path = "../../../tests/support/native_smoke.rs"]
mod support;
use bedtime_api::{status::*, workbench::*};
use bedtime_core::bedtime_workbench::{self, WorkbenchLayout, WorkbenchViewport};
use bedtime_desktop::dear_imgui_rs::Ui;
use bedtime_desktop::{DesktopFrameInfo, DesktopInjectedEvent, DesktopInputStep};
use support::*;

#[derive(Resource, Default)]
struct Observations {
    commands: usize,
    panel_bottom: f32,
    focus: Option<Entity>,
    publishers: Vec<Entity>,
    font_generation: u64,
}
#[derive(Component)]
struct Widget {
    width: f32,
    control: Option<[f32; 2]>,
    last_inline: u64,
    last_popup: u64,
}
fn measure(world: &World, _: &Ui, context: StatusContext) -> Option<f32> {
    Some(world.get::<Widget>(context.publisher).unwrap().width)
}
fn draw(
    world: &mut World,
    ui: &Ui,
    context: StatusDrawContext,
    actions: &mut Vec<WorkbenchAction>,
) {
    let frame = world.resource::<DesktopFrameInfo>().frame.0;
    if ui.button("Increment") {
        actions.push(
            context
                .context
                .invoke("example.status.increment", Value::Null),
        );
    }
    let min = ui.item_rect_min();
    let max = ui.item_rect_max();
    let mut widget = world.get_mut::<Widget>(context.context.publisher).unwrap();
    widget.control = Some([(min[0] + max[0]) / 2., (min[1] + max[1]) / 2.]);
    match context.mode {
        StatusDrawMode::Inline => widget.last_inline = frame,
        StatusDrawMode::Popup => widget.last_popup = frame,
    }
}
fn tooltip(_: &World, ui: &Ui, _: StatusContext) {
    ui.text("A contribution from an independent plugin");
}
fn increment(_: In<CommandInvocation>, mut observations: ResMut<Observations>) -> CommandResult {
    observations.commands += 1;
    Ok(Vec::new())
}
struct Content;
impl Panel for Content {
    fn title(&self, _: &World) -> String {
        "Minimal application".into()
    }
    fn draw(
        &mut self,
        ui: &Ui,
        world: &mut World,
        _: &mut Vec<WorkbenchAction>,
        _: &ShortcutInput,
    ) {
        ui.text("No bEd settings, workspace or feature panels are installed.");
        world.resource_mut::<Observations>().panel_bottom =
            ui.window_pos()[1] + ui.window_size()[1];
    }
}
fn create(_: In<PanelCreation>) -> PanelResult {
    Ok(Box::new(Content))
}
fn main() -> io::Result<()> {
    let verified = Arc::<AtomicBool>::default();
    let launch = match bedtime::awaken("example.workbench.smoke", "Workbench smoke")?.dispatch()? {
        bedtime::LaunchOutcome::Handled(exit) => std::process::exit(exit),
        bedtime::LaunchOutcome::Desktop(launch) => *launch,
    };
    let options = bedtime::DesktopOptions {
        config_directory: Some(
            std::env::temp_dir().join(format!("bed-workbench-proof-{}", std::process::id())),
        ),
        desktop: bedtime_desktop::DesktopPlugins {
            window: Window {
                resolution: (1000, 650).into(),
                ..default()
            },
            ..default()
        },
        ..Default::default()
    };
    let exit = launch.run(options, |app| {
        assert!(
            !app.get_added_plugins::<bevy::window::WindowPlugin>()[0].close_when_requested,
            "a custom desktop window must retain the shared document close owner"
        );
        app.add_plugins(bedtime_plugin_automation::AutomationPlugin)
            .insert_resource(Smoke {
                verified: Arc::clone(&verified),
                ..Default::default()
            })
            .init_resource::<Observations>();
        register(app.world_mut(), "example.status", |registration| {
            registration.panel("example.status.panel", "Content", create);
            registration.command("example.status.increment", "Increment", increment);
        })
        .map_err(io::Error::other)?;
        let mut publishers = Vec::new();
        for (id, priority, width) in [
            ("example.status.primary", 100, 240.),
            ("example.status.secondary", 1, 260.),
        ] {
            publishers.push(
                app.world_mut()
                    .spawn((
                        StatusItems(vec![StatusItem {
                            id,
                            label: id,
                            priority,
                            icon: "default",
                            prefer_icon: false,
                            measure,
                            draw,
                            tooltip,
                        }]),
                        Widget {
                            width,
                            control: None,
                            last_inline: 0,
                            last_popup: 0,
                        },
                    ))
                    .id(),
            );
        }
        app.world_mut().resource_mut::<Observations>().publishers = publishers;
        bedtime_workbench::open_panel(
            app.world_mut(),
            json!({"panel_type":"example.status.panel"}),
            None,
            None,
        )
        .map_err(io::Error::other)?;
        app.add_systems(Last, advance.before(bevy::app::OnAppExitSystems));
        Ok(())
    })?;
    ensure(
        verified.load(Ordering::SeqCst),
        "native smoke exited before completing its assertions",
    )?;
    if exit.is_error() {
        return Err(io::Error::other("Workbench smoke failed"));
    }
    Ok(())
}
fn click(point: [f32; 2]) -> Operation {
    Operation::Input(vec![
        DesktopInputStep::from(vec![
            DesktopInjectedEvent::PointerMoved(point),
            DesktopInjectedEvent::PointerButton {
                button: winit::event::MouseButton::Left,
                pressed: true,
            },
        ]),
        DesktopInputStep::from(vec![DesktopInjectedEvent::PointerButton {
            button: winit::event::MouseButton::Left,
            pressed: false,
        }]),
    ])
}
fn advance(world: &mut World) {
    world.resource_scope(|world, mut smoke: Mut<Smoke>| {
        let result = (|| -> io::Result<bool> {
            ensure(
                smoke.started.elapsed() < Duration::from_secs(60),
                format!("Workbench smoke timed out in phase {}", smoke.phase),
            )?;
            if !world.contains_resource::<bedtime_desktop::DesktopGpu>() {
                return Ok(false);
            }
            if presented(world) < 3 {
                focus_window(world);
                return Ok(false);
            }
            let publishers = world.resource::<Observations>().publishers.clone();
            let frame = world.resource::<DesktopFrameInfo>().clone();
            let focused = world.resource::<WorkbenchLayout>().focused;
            match smoke.phase {
                0 => {
                    let height = world.resource::<WorkbenchViewport>().bottom_inset;
                    ensure(height > 0., "default host omitted plugin status items")?;
                    let observations = world.resource::<Observations>();
                    ensure(
                        observations.panel_bottom
                            <= frame.display_pos[1] + frame.display_size[1] - height + 1.,
                        "panel overlaps footer",
                    )?;
                    let point = world
                        .get::<Widget>(publishers[0])
                        .unwrap()
                        .control
                        .ok_or_else(|| io::Error::other("status button missing"))?;
                    world.resource_mut::<Observations>().focus = focused;
                    if smoke.operation(world, click(point))?.is_some() {
                        smoke.next(world);
                    }
                }
                1 if world.resource::<Observations>().commands == 1 => {
                    ensure(
                        focused == world.resource::<Observations>().focus,
                        "status command changed panel focus",
                    )?;
                    native_window(world, |window| {
                        let _ = window.request_inner_size(winit::dpi::LogicalSize::new(330., 650.));
                    })?;
                    smoke.next(world);
                }
                2 if frame.display_size[0] < 400. && smoke.elapsed_frames(world) > 3 => {
                    ensure(
                        world.get::<Widget>(publishers[0]).unwrap().last_inline < frame.frame.0 - 2,
                        "narrow footer did not compact",
                    )?;
                    let height = world.resource::<WorkbenchViewport>().bottom_inset;
                    let point = [
                        frame.display_pos[0] + 18.,
                        frame.display_pos[1] + frame.display_size[1] - height / 2.,
                    ];
                    if smoke.operation(world, click(point))?.is_some() {
                        smoke.next(world);
                    }
                }
                3 => {
                    let widget = world.get::<Widget>(publishers[0]).unwrap();
                    if widget.last_popup >= frame.frame.0.saturating_sub(1) {
                        let point = widget.control.unwrap();
                        if smoke.operation(world, click(point))?.is_some() {
                            smoke.next(world);
                        }
                    }
                }
                4 if world.resource::<Observations>().commands == 2 => {
                    ensure(
                        focused == world.resource::<Observations>().focus,
                        "compact popup changed panel focus",
                    )?;
                    world.entity_mut(publishers[0]).remove::<StatusItems>();
                    smoke.next(world);
                }
                5 if smoke.elapsed_frames(world) > 3 => {
                    ensure(
                        world.get::<Widget>(publishers[0]).unwrap().last_popup < frame.frame.0 - 2,
                        "withdrawn publisher kept drawing popup",
                    )?;
                    world.entity_mut(publishers[1]).remove::<StatusItems>();
                    smoke.next(world);
                }
                6 if smoke.elapsed_frames(world) > 2 => {
                    ensure(
                        world.resource::<WorkbenchViewport>().bottom_inset == 0.,
                        "empty default footer retained inset",
                    )?;
                    world.resource_mut::<Observations>().font_generation =
                        world.non_send::<bedtime_desktop::DesktopFonts>().generation;
                    world.write_message(bedtime_api::settings::SettingsRequest::new(
                        "bedtime.ui",
                        "fontSize",
                        24,
                    ));
                    smoke.next(world);
                }
                7 if world.non_send::<bedtime_desktop::DesktopFonts>().generation
                    > world.resource::<Observations>().font_generation =>
                {
                    if let Some(capture) = smoke.capture(world, vec![CaptureTarget::Window])? {
                        ensure(
                            visible_pixels(&capture.images[0].rgba.pixels) > 1000,
                            "minimal application capture is blank",
                        )?;
                        std::fs::create_dir_all("target")?;
                        capture.images[0]
                            .rgba
                            .write_png("target/workbench-smoke.png")?;
                        return Ok(true);
                    }
                }
                _ => {}
            }
            Ok(false)
        })();
        match result {
            Ok(true) => {
                smoke.verified.store(true, Ordering::SeqCst);
                eprintln!("Workbench smoke PASS");
                world.write_message(AppExit::Success);
            }
            Ok(false) => {}
            Err(error) => {
                eprintln!("Workbench smoke failed: {error}");
                world.write_message(AppExit::error());
            }
        }
    });
}
