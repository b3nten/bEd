//! Native proof of typed input, final-frame window/panel captures and recovery.
use bed_desktop::{bevy::prelude::*, *};
use dear_imgui_rs::{Color, Condition, Ui};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use winit::{
    event::MouseButton,
    keyboard::{Key, KeyCode},
};

#[derive(Resource)]
struct Proof {
    panel: Entity,
    phase: u8,
    clicks: u32,
    text: String,
    button: [f32; 2],
    field: [f32; 2],
    held_b: bool,
    ctrl: bool,
    input: Option<DesktopInputId>,
    capture: Option<DesktopCaptureId>,
    generation: u64,
    presented_frames: u64,
    capture_after: DesktopFrameId,
    deadline: Instant,
}

/// Exercise the actual camera graph without an acquired native output. Keeping
/// the configured format matches Bevy's real `CurrentSurfaceTexture::Occluded`.
#[derive(Resource, Clone, Default)]
struct AbsentOutput {
    enabled: Arc<AtomicBool>,
    frames: Arc<AtomicU64>,
}

fn withhold_output(
    absent: Res<AbsentOutput>,
    mut windows: Query<&mut bevy::render::view::ExtractedWindow>,
) {
    if absent.enabled.load(Ordering::Relaxed) {
        for mut window in &mut windows {
            window.swap_chain_texture_view = None;
            window.swap_chain_texture = None;
        }
        absent.frames.fetch_add(1, Ordering::Relaxed);
    }
}

fn main() {
    let mut app = App::new();
    app.add_plugins(DesktopPlugins {
        window: Window {
            title: "Desktop automation proof".into(),
            resolution: (320, 180).into(),
            ..default()
        },
        ..default()
    });
    let absent = AbsentOutput::default();
    app.insert_resource(absent.clone());
    app.sub_app_mut(bevy::render::RenderApp)
        .insert_resource(absent)
        .add_systems(
            bevy::render::Render,
            withhold_output
                .in_set(bevy::render::RenderSystems::PrepareViews)
                .after(bevy::render::view::prepare_windows)
                .before(bevy::render::view::prepare_view_attachments)
                .before(bevy::render::view::prepare_view_targets),
        );
    let panel = app.world_mut().spawn_empty().id();
    app.insert_resource(Proof {
        panel,
        phase: 0,
        clicks: 0,
        text: String::new(),
        button: [0.; 2],
        field: [0.; 2],
        held_b: false,
        ctrl: false,
        input: None,
        capture: None,
        generation: 0,
        presented_frames: 0,
        capture_after: DesktopFrameId(0),
        deadline: Instant::now() + Duration::from_secs(45),
    })
    .insert_resource(DesktopUi(draw))
    .add_systems(
        PreUpdate,
        advance
            .after(DesktopSystems::AfterUi)
            .before(DesktopSystems::RouteInput),
    );
    app.run();
}

fn draw(world: &mut World, ui: &Ui) {
    let mut proof = world.remove_resource::<Proof>().unwrap();
    let background = ui.get_background_draw_list();
    let rgb = if proof.phase >= 4 {
        [128, 48, 96]
    } else {
        [64, 96, 128]
    };
    background
        .add_rect(
            [0., 0.],
            ui.io().display_size(),
            Color::from_rgba_bytes(rgb[0], rgb[1], rgb[2], 255),
        )
        .filled(true)
        .build();
    ui.window("Proof")
        .position([10., 10.], Condition::Always)
        .size([290., 150.], Condition::Always)
        .build(|| {
            if ui.button("Click once") {
                proof.clicks += 1;
            }
            let min = ui.item_rect_min();
            let max = ui.item_rect_max();
            proof.button = [(min[0] + max[0]) * 0.5, (min[1] + max[1]) * 0.5];
            ui.input_text("Text", &mut proof.text).build();
            let min = ui.item_rect_min();
            let max = ui.item_rect_max();
            proof.field = [min[0] + 12., (min[1] + max[1]) * 0.5];
            ui.text(format!("Clicks: {}", proof.clicks));
        });
    // A host supplies real content/clip facts; this fixture deliberately gives a
    // partially clipped body to verify that the returned crop names its extent.
    world
        .resource_mut::<DesktopFrame>()
        .publish_panel(DesktopPanelGeometry {
            panel: proof.panel,
            content: DesktopRect {
                min: [0., 0.],
                max: [320., 180.],
            },
            clip: DesktopRect {
                min: [2., 2.],
                max: [8., 8.],
            },
            visible: true,
        });
    proof.held_b = ui.is_key_down(dear_imgui_rs::Key::B);
    proof.ctrl = ui.io().key_ctrl();
    world.insert_resource(proof);
}

fn advance(world: &mut World) {
    let mut proof = world.remove_resource::<Proof>().unwrap();
    assert!(
        Instant::now() < proof.deadline,
        "automation proof timed out at phase {}",
        proof.phase
    );
    let info = world.resource::<DesktopFrameInfo>().clone();
    match proof.phase {
        0 if info.frame.0 >= 8 => {
            let click = |position| {
                vec![
                    DesktopInputStep::from(vec![
                        DesktopInjectedEvent::PointerMoved(position),
                        DesktopInjectedEvent::PointerButton {
                            button: MouseButton::Left,
                            pressed: true,
                        },
                    ]),
                    DesktopInputStep::from(vec![DesktopInjectedEvent::PointerButton {
                        button: MouseButton::Left,
                        pressed: false,
                    }]),
                ]
            };
            let mut steps = click(proof.button);
            steps.extend(click(proof.field));
            steps.push(DesktopInputStep::from(vec![
                DesktopInjectedEvent::Key {
                    physical: KeyCode::KeyA,
                    logical: Key::Character("a".into()),
                    pressed: true,
                },
                DesktopInjectedEvent::Text("ok".into()),
            ]));
            steps.push(DesktopInputStep::from(vec![DesktopInjectedEvent::Key {
                physical: KeyCode::KeyA,
                logical: Key::Character("a".into()),
                pressed: false,
            }]));
            steps.push(DesktopInputStep::from(vec![
                DesktopInjectedEvent::Modifiers(DesktopModifiers {
                    shift: true,
                    ..default()
                }),
            ]));
            steps.push(DesktopInputStep::from(vec![
                DesktopInjectedEvent::Modifiers(DesktopModifiers::default()),
            ]));
            steps.push(DesktopInputStep::from(vec![
                DesktopInjectedEvent::Key {
                    physical: KeyCode::KeyB,
                    logical: Key::Character("b".into()),
                    pressed: true,
                },
                DesktopInjectedEvent::Modifiers(DesktopModifiers {
                    ctrl: true,
                    ..default()
                }),
            ]));
            proof.input = Some(world.resource_mut::<DesktopInputRequests>().submit(steps));
            proof.phase = 1;
        }
        1 => {
            if let Some(result) = world
                .resource_mut::<DesktopInputResults>()
                .take(proof.input.unwrap())
            {
                let receipt = result.unwrap();
                assert!(receipt.last_ui_frame.0 > 8);
                assert_eq!(receipt.steps, 9);
                assert!(
                    !proof.held_b && !proof.ctrl,
                    "sequence cleanup must release held UI keys/modifiers before its receipt"
                );
                assert_eq!(proof.clicks, 1);
                assert_eq!(
                    proof.text, "ok",
                    "keys must not manufacture a duplicate text character"
                );
                proof.capture = Some(world.resource_mut::<DesktopCaptureRequests>().submit(
                    vec![
                        DesktopCaptureTarget::Window,
                        DesktopCaptureTarget::Panel(proof.panel),
                    ],
                    proof.deadline,
                    Some(receipt.last_ui_frame),
                ));
                proof.phase = 2;
                eprintln!(
                    "automation proof: pointer/key/text/modifier steps and UI receipt passed"
                );
            }
        }
        2 => {
            if let Some(result) = world
                .resource_mut::<DesktopCaptureResults>()
                .take(proof.capture.unwrap())
            {
                let captured = result.unwrap();
                assert_batch(&captured, proof.panel, [64, 96, 128]);
                let path = std::env::temp_dir()
                    .join(format!("bed-automation-proof-{}.png", std::process::id()));
                captured.images[0].rgba.write_png(&path).unwrap();
                assert!(std::fs::metadata(&path).unwrap().len() > 100);
                std::fs::remove_file(path).unwrap();
                let gpu = world.resource::<DesktopGpu>().clone();
                proof.generation = gpu.generation;
                // The request waits across actual loss, and uses replacement geometry.
                proof.capture = Some(world.resource_mut::<DesktopCaptureRequests>().submit(
                    vec![
                        DesktopCaptureTarget::Window,
                        DesktopCaptureTarget::Panel(proof.panel),
                    ],
                    proof.deadline,
                    Some(DesktopFrameId(info.frame.0 + 2)),
                ));
                gpu.device.destroy();
                let _ = gpu.device.poll(wgpu::PollType::Poll);
                proof.phase = 3;
                eprintln!(
                    "automation proof: same-frame final pixels, clipped scaled crop and PNG passed; replacing device"
                );
            }
        }
        3 => {
            if let Some(result) = world
                .resource_mut::<DesktopCaptureResults>()
                .take(proof.capture.unwrap())
            {
                let captured = result.unwrap();
                assert!(captured.info.generation > proof.generation);
                assert_batch(&captured, proof.panel, [64, 96, 128]);
                assert_eq!(proof.text, "ok");
                assert_eq!(proof.clicks, 1);
                let status = world.resource::<DesktopRenderStatus>().read();
                assert_eq!(status.recoveries, 1);
                assert!(!status.recovering);
                assert!(
                    status.error.is_none(),
                    "recovered device loss is not a terminal rendering error"
                );
                eprintln!(
                    "automation proof: capture deadline/generation survived native device replacement"
                );
                proof.presented_frames = status.completed_frames;
                proof.capture_after = info.frame;
                world
                    .resource::<AbsentOutput>()
                    .enabled
                    .store(true, Ordering::Relaxed);
                proof.capture = Some(world.resource_mut::<DesktopCaptureRequests>().submit(
                    vec![
                        DesktopCaptureTarget::Window,
                        DesktopCaptureTarget::Panel(proof.panel),
                    ],
                    proof.deadline,
                    Some(info.frame),
                ));
                proof.phase = 4;
            }
        }
        4 => {
            if let Some(result) = world
                .resource_mut::<DesktopCaptureResults>()
                .take(proof.capture.unwrap())
            {
                let captured = result.unwrap();
                assert!(captured.info.frame > proof.capture_after);
                // This new color was first drawn after native output disappeared.
                // Reading an earlier final target would fail these pixel checks.
                assert_batch(&captured, proof.panel, [128, 48, 96]);
                let status = world.resource::<DesktopRenderStatus>().read();
                assert_eq!(status.completed_frames, proof.presented_frames);
                assert!(status.error.is_none());
                assert!(
                    world
                        .resource::<AbsentOutput>()
                        .frames
                        .load(Ordering::Relaxed)
                        > 0
                );
                eprintln!(
                    "automation proof: fresh window/panel pixels without a native surface; presented counter stayed unchanged"
                );
                world.write_message(bevy::app::AppExit::Success);
                proof.phase = 5;
            }
        }
        _ => {}
    }
    world.insert_resource(proof);
}

fn assert_batch(captured: &DesktopCapturedFrame, panel: Entity, rgb: [u8; 3]) {
    assert_eq!(captured.images.len(), 2);
    let window = &captured.images[0];
    let crop = &captured.images[1];
    assert_eq!(crop.target, DesktopCaptureTarget::Panel(panel));
    let [x, y, width, height] = crop.physical_rect;
    assert!(width > 0 && height > 0);
    assert_eq!(crop.rgba.size, [width, height]);
    for row in 0..height {
        let start = (((row + y) * window.rgba.size[0] + x) * 4) as usize;
        let crop_start = (row * width * 4) as usize;
        assert_eq!(
            &crop.rgba.pixels[crop_start..crop_start + width as usize * 4],
            &window.rgba.pixels[start..start + width as usize * 4]
        );
    }
    for pixel in crop.rgba.pixels.as_chunks::<4>().0 {
        for (actual, expected) in pixel[..3].iter().zip(rgb) {
            assert!(
                actual.abs_diff(expected) <= 1,
                "final color mismatch: {pixel:?}"
            );
        }
    }
}
