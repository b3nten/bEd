//! Independent desktop consumer: no editor, configuration, workspace or resource lookup.
use bed_desktop::{
    bevy::prelude::*,
    dear_imgui_rs::{Key, Ui},
    *,
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Resource, Default)]
struct Counter {
    value: u32,
    button: [f32; 2],
}
#[derive(Resource)]
struct Smoke {
    phase: u8,
    deadline: Instant,
    input: Option<DesktopInputId>,
    capture: Option<DesktopCaptureId>,
    font_generation: u64,
    verified: Arc<AtomicBool>,
}
fn command(id: &str, label: &str, key: Option<Key>) -> DesktopMenuCommand {
    DesktopMenuCommand {
        id: id.into(),
        label: label.into(),
        enabled: true,
        shortcut: key.map(|key| DesktopShortcut {
            key,
            modifiers: DesktopModifiers {
                super_key: true,
                ..default()
            },
        }),
    }
}
fn main() {
    let smoke = std::env::args().any(|arg| arg == "--smoke");
    let verified = Arc::new(AtomicBool::new(false));
    let mut app = App::new();
    app.insert_resource(DesktopNativePresentation {
        menus: vec![DesktopMenu {
            label: "Desktop consumer".into(),
            windows_menu: false,
            items: vec![
                DesktopMenuItem::Command(command("consumer.increment", "Increment", Some(Key::I))),
                DesktopMenuItem::Separator,
                DesktopMenuItem::Quit(command("consumer.quit", "Quit", Some(Key::Q))),
            ],
        }],
        toolbar: vec![DesktopToolbarItem {
            id: "consumer.increment".into(),
            label: "+".into(),
            icon: None,
            enabled: true,
        }],
        dock: vec![command("consumer.increment", "Increment", None)],
        ..default()
    })
    .add_plugins(DesktopPlugins {
        window: Window {
            title: "Desktop consumer".into(),
            resolution: (640, 420).into(),
            ..default()
        },
        ..default()
    })
    .init_resource::<Counter>()
    .insert_resource(DesktopUi(draw))
    .add_systems(
        PreUpdate,
        native_commands.in_set(DesktopSystems::InterceptInput),
    );
    #[cfg(feature = "effects")]
    app.add_plugins(bed_effects::EffectsPlugin);
    #[cfg(feature = "layout")]
    install_layout(&mut app);
    if smoke {
        app.insert_resource(Smoke {
            phase: 0,
            deadline: Instant::now() + Duration::from_secs(45),
            input: None,
            capture: None,
            font_generation: 0,
            verified: Arc::clone(&verified),
        })
        .add_systems(
            PreUpdate,
            advance
                .after(DesktopSystems::AfterUi)
                .before(DesktopSystems::RouteInput),
        );
    }
    let exit = app.run();
    if smoke {
        assert!(
            verified.load(Ordering::SeqCst),
            "desktop consumer stopped before completing smoke"
        );
        assert_eq!(exit, AppExit::Success);
        println!("desktop consumer smoke PASS");
    }
}
fn native_commands(mut native: ResMut<DesktopNativeActions>, mut counter: ResMut<Counter>) {
    for action in native.0.drain(..) {
        if action.command == "consumer.increment" {
            counter.value += 1;
        }
    }
}
fn counter_ui(ui: &Ui, world: &mut World) {
    let mut counter = world.resource_mut::<Counter>();
    ui.text(format!("Counter: {}", counter.value));
    if ui.button("Increment") {
        counter.value += 1;
    }
    let min = ui.item_rect_min();
    let max = ui.item_rect_max();
    counter.button = [(min[0] + max[0]) * 0.5, (min[1] + max[1]) * 0.5];
    ui.text("ImGui, fonts, native commands and GPU presentation supplied by the base.");
}
fn draw(world: &mut World, ui: &Ui) {
    #[cfg(not(feature = "layout"))]
    ui.window("Desktop application")
        .position(
            [12.0, world.resource::<DesktopContentInsets>().top + 12.0],
            dear_imgui_rs::Condition::Always,
        )
        .size([610.0, 340.0], dear_imgui_rs::Condition::Always)
        .build(|| counter_ui(ui, world));
    #[cfg(feature = "layout")]
    {
        bed_workbench::render_ui(world, ui);
        world.resource_mut::<DesktopFrame>().panels = world
            .query::<(Entity, &bed_workbench::api::PanelGeometry)>()
            .iter(world)
            .map(|(panel, geometry)| DesktopPanelGeometry {
                panel,
                content: DesktopRect {
                    min: geometry.content_min,
                    max: geometry.content_max,
                },
                clip: DesktopRect {
                    min: geometry.clip_min,
                    max: geometry.clip_max,
                },
                visible: geometry.visible,
            })
            .collect();
    }
}
fn advance(world: &mut World) {
    let mut smoke = world.remove_resource::<Smoke>().expect("smoke installed");
    assert!(
        Instant::now() < smoke.deadline,
        "consumer timed out at phase {}",
        smoke.phase
    );
    assert!(
        world.resource::<DesktopNativeErrors>().0.is_empty(),
        "native errors: {:?}",
        world.resource::<DesktopNativeErrors>().0
    );
    assert!(
        world.resource::<DesktopAppearanceErrors>().0.is_empty(),
        "appearance errors: {:?}",
        world.resource::<DesktopAppearanceErrors>().0
    );
    let info = world.resource::<DesktopFrameInfo>().clone();
    match smoke.phase {
        0 if info.frame.0 >= 8 => {
            let fonts = world.non_send::<DesktopFonts>();
            assert!(fonts.main.is_some() && fonts.large.is_some());
            assert!(
                fonts.warning.is_none(),
                "default font must require no resource discovery"
            );
            assert_eq!(fonts.resolved.as_ref().unwrap().family, "Paper Mono");
            smoke.font_generation = fonts.generation;
            let desktop = world.non_send::<DesktopContext>();
            if world.resource::<DesktopNativeCapabilities>().clipboard {
                let old = desktop.context.clipboard_text();
                desktop
                    .context
                    .set_clipboard_text("desktop-consumer clipboard proof");
                assert_eq!(
                    desktop.context.clipboard_text().as_deref(),
                    Some("desktop-consumer clipboard proof")
                );
                if let Some(old) = old {
                    desktop.context.set_clipboard_text(old);
                }
            }
            #[cfg(target_os = "macos")]
            world
                .non_send::<DesktopNativePlatform>()
                .menu
                .as_ref()
                .unwrap()
                .perform_for_smoke("consumer.increment")
                .unwrap();
            #[cfg(not(target_os = "macos"))]
            {
                world.resource_mut::<Counter>().value += 1;
            }
            smoke.phase = 1;
        }
        1 if world.resource::<Counter>().value == 1 => {
            let position = world.resource::<Counter>().button;
            smoke.input = Some(world.resource_mut::<DesktopInputRequests>().submit(vec![
                DesktopInputStep::from(vec![
                    DesktopInjectedEvent::PointerMoved(position),
                    DesktopInjectedEvent::PointerButton {
                        button: bed_desktop::winit::event::MouseButton::Left,
                        pressed: true,
                    },
                ]),
                DesktopInputStep::from(vec![DesktopInjectedEvent::PointerButton {
                    button: bed_desktop::winit::event::MouseButton::Left,
                    pressed: false,
                }]),
            ]));
            smoke.phase = 2;
        }
        2 => {
            if let Some(receipt) = world
                .resource_mut::<DesktopInputResults>()
                .take(smoke.input.unwrap())
            {
                assert_eq!(receipt.unwrap().steps, 2);
                assert_eq!(world.resource::<Counter>().value, 2);
                let targets = {
                    #[cfg(feature = "layout")]
                    {
                        vec![
                            DesktopCaptureTarget::Window,
                            DesktopCaptureTarget::Panel(
                                world
                                    .resource::<bed_workbench::WorkbenchLayout>()
                                    .focused
                                    .unwrap(),
                            ),
                        ]
                    }
                    #[cfg(not(feature = "layout"))]
                    {
                        vec![DesktopCaptureTarget::Window]
                    }
                };
                smoke.capture = Some(world.resource_mut::<DesktopCaptureRequests>().submit(
                    targets,
                    smoke.deadline,
                    Some(info.frame),
                ));
                smoke.phase = 3;
            }
        }
        3 => {
            if let Some(capture) = world
                .resource_mut::<DesktopCaptureResults>()
                .take(smoke.capture.unwrap())
            {
                let capture = capture.unwrap();
                for image in capture.images {
                    assert!(image.rgba.size.iter().all(|size| *size > 0));
                    assert_eq!(
                        image.rgba.pixels.len(),
                        image.rgba.size[0] as usize * image.rgba.size[1] as usize * 4
                    );
                    assert!(
                        image
                            .rgba
                            .pixels
                            .as_chunks::<4>()
                            .0
                            .iter()
                            .any(|pixel| pixel[0] != pixel[1])
                    );
                }
                world.resource_mut::<DesktopAppearance>().0.palette.accent = [0.8, 0.2, 0.4, 1.0];
                smoke.phase = 4;
            }
        }
        4 => {
            assert_eq!(
                world.non_send::<DesktopFonts>().generation,
                smoke.font_generation,
                "palette-only update rebuilt atlas"
            );
            world.resource_mut::<DesktopAppearance>().0.fonts.size += 2.0;
            smoke.phase = 5;
        }
        5 if world.non_send::<DesktopFonts>().generation > smoke.font_generation => {
            #[cfg(target_os = "macos")]
            {
                let native = world.non_send::<DesktopNativePlatform>();
                assert!(
                    native
                        .window
                        .as_ref()
                        .unwrap()
                        .preserves_winit_content_view()
                );
                assert!(native.menu.as_ref().unwrap().is_installed());
                native
                    .menu
                    .as_ref()
                    .unwrap()
                    .perform_for_smoke("consumer.quit")
                    .unwrap();
            }
            #[cfg(not(target_os = "macos"))]
            {
                world.write_message(AppExit::Success);
            }
            smoke.verified.store(true, Ordering::SeqCst);
            smoke.phase = 6;
        }
        _ => {}
    }
    world.insert_resource(smoke);
}
#[cfg(feature = "layout")]
struct CounterPanel;
#[cfg(feature = "layout")]
impl bed_workbench::Panel for CounterPanel {
    fn title(&self, _: &World) -> String {
        "Counter panel".into()
    }
    fn draw(
        &mut self,
        ui: &Ui,
        world: &mut World,
        _: &mut Vec<bed_workbench::Action>,
        _shortcuts: &bed_workbench_api::ShortcutInput,
    ) {
        counter_ui(ui, world);
    }
    fn save(&self, world: &World) -> serde_json::Value {
        serde_json::json!({"counter":world.resource::<Counter>().value})
    }
}
#[cfg(feature = "layout")]
fn install_layout(app: &mut App) {
    app.add_plugins(bed_workbench::WorkbenchPlugin)
        .configure_sets(
            PreUpdate,
            bed_workbench::WorkbenchSystems::Requests.in_set(DesktopSystems::AfterUi),
        );
    bed_workbench::register(app.world_mut(), "consumer", |registry| {
        registry.panel(
            "consumer.counter",
            "Counter",
            |_: In<bed_workbench::api::PanelCreation>| -> bed_workbench::api::PanelResult {
                Ok(Box::new(CounterPanel))
            },
        );
    })
    .unwrap();
    app.add_systems(Startup, |world: &mut World| {
        use bed_workbench::{Action, WorkbenchRequests};
        world
            .resource_mut::<WorkbenchRequests>()
            .push(Action::OpenPanel {
                panel_type: "consumer.counter".into(),
                state: serde_json::Value::Null,
                area: None,
            });
    });
    app.add_systems(
        PreUpdate,
        layout_presentation
            .in_set(DesktopSystems::Prepare)
            .after(DesktopNativeSystems::Prepare),
    );
}
#[cfg(feature = "layout")]
fn layout_presentation(
    insets: Res<DesktopContentInsets>,
    appearance: Res<DesktopAppearance>,
    mut viewport: ResMut<bed_workbench::WorkbenchViewport>,
    mut layout: ResMut<bed_workbench::WorkbenchAppearance>,
) {
    viewport.top_inset = insets.top;
    layout.background = appearance.0.palette.background;
}
