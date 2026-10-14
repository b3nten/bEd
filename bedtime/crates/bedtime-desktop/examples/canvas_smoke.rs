//! Native proof for the optional registered canvas/icon presentation plugin.
use bedtime::ui::{
    TextureHandle,
    gpu::{
        CanvasOutput, CanvasRenderInvocation, CanvasRenderResult, CanvasRenderer, CanvasTextures,
        CanvasVisible, RenderOutput,
    },
};
use bedtime::{bevy::prelude::*, desktop::*};
use dear_imgui_rs::{Color, Ui};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Component)]
struct Source {
    color: wgpu::Color,
    panic: bool,
    calls: u32,
}

#[derive(Resource)]
struct Proof {
    verified: Arc<AtomicBool>,
    first: Entity,
    second: Entity,
    phase: u8,
    entered: DesktopFrameId,
    capture: Option<DesktopCaptureId>,
    external: Option<dear_imgui_rs::TextureId>,
    generation: u64,
    deadline: Instant,
}

fn main() -> std::io::Result<()> {
    let verified = Arc::<AtomicBool>::default();
    let launch = match bedtime::awaken("bedtime.proof.canvas-smoke", "Desktop proof")?.dispatch()? {
        bedtime::LaunchOutcome::Desktop(launch) => *launch,
        bedtime::LaunchOutcome::Handled(exit) => std::process::exit(exit),
    };
    let mut app = launch.build(
        bedtime::DesktopOptions {
            desktop: DesktopPlugins {
                window: Window {
                    title: "Desktop canvas proof".into(),
                    resolution: (140, 80).into(),
                    ..default()
                },
                ..default()
            },
            config_directory: Some(
                std::env::temp_dir()
                    .join(format!("bedtime.proof.canvas-smoke-{}", std::process::id())),
            ),
            ..Default::default()
        },
        |app| {
            let mut icons = bedtime::ui::icons::Icons::default();
            icons.images.insert(
                "proof".into(),
                bedtime::ui::icons::RgbaImage {
                    width: 1,
                    height: 1,
                    pixels: vec![255, 240, 80, 255],
                },
            );
            app.insert_non_send(icons);
            let mut canvas = |handle, color| {
                let entity = app
                    .world_mut()
                    .spawn((
                        CanvasOutput(RenderOutput {
                            handle: TextureHandle(handle),
                            size: [16, 16],
                            depth: false,
                            revision: 0,
                        }),
                        CanvasVisible(true),
                        Source {
                            color,
                            panic: false,
                            calls: 0,
                        },
                    ))
                    .id();
                let system = app.world_mut().register_system(clear);
                app.world_mut()
                    .entity_mut(system.entity())
                    .insert(ChildOf(entity));
                app.world_mut()
                    .entity_mut(entity)
                    .insert(CanvasRenderer(system));
                entity
            };
            let first = canvas(1, wgpu::Color::RED);
            let second = canvas(2, wgpu::Color::GREEN);
            app.insert_resource(Proof {
                verified: Arc::clone(&verified),
                first,
                second,
                phase: 0,
                entered: DesktopFrameId(0),
                capture: None,
                external: None,
                generation: 0,
                deadline: Instant::now() + Duration::from_secs(45),
            })
            .insert_resource(DesktopUi(draw))
            .add_systems(
                PreUpdate,
                advance
                    .after(DesktopCanvasSystems::Render)
                    .before(DesktopSystems::RouteInput),
            );
            Ok(())
        },
    )?;
    if app.run().is_error() {
        return Err(std::io::Error::other("Desktop proof exited with an error"));
    }
    assert!(
        verified.load(Ordering::SeqCst),
        "desktop smoke exited before completing its assertions"
    );
    Ok(())
}

fn clear(
    In(input): In<CanvasRenderInvocation>,
    mut sources: Query<&mut Source>,
) -> CanvasRenderResult {
    let mut source = sources.get_mut(input.entity).unwrap();
    source.calls += 1;
    assert!(!source.panic, "intentional per-panel canvas failure");
    let mut encoder = input
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("canvas proof source"),
        });
    {
        let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &input.target.view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(source.color),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
    }
    Ok(vec![encoder.finish()])
}

fn draw(world: &mut World, ui: &Ui) {
    let draw = ui.get_background_draw_list();
    draw.add_rect([0., 0.], ui.io().display_size(), Color::BLACK)
        .filled(true)
        .build();
    for (handle, min, max) in [
        (TextureHandle(1), [8., 8.], [24., 24.]),
        (TextureHandle(2), [40., 8.], [56., 24.]),
    ] {
        if let Some(texture) = world.resource::<CanvasTextures>().0.get(&handle) {
            draw.add_image(*texture, min, max, [0.; 2], [1.; 2], Color::WHITE);
        }
    }
    if let Some(texture) = world
        .get_non_send::<bedtime::ui::icons::Icons>()
        .unwrap()
        .get("proof")
    {
        draw.add_image(
            texture,
            [80., 8.],
            [96., 24.],
            [0.; 2],
            [1.; 2],
            Color::WHITE,
        );
    }
}

fn capture(world: &mut World, proof: &mut Proof) {
    proof.capture = Some(world.resource_mut::<DesktopCaptureRequests>().submit(
        vec![DesktopCaptureTarget::Window],
        proof.deadline,
        None,
    ));
}

fn advance(world: &mut World) {
    let mut proof = world.remove_resource::<Proof>().unwrap();
    assert!(
        Instant::now() < proof.deadline,
        "canvas proof timed out phase {}",
        proof.phase
    );
    let info = world.resource::<DesktopFrameInfo>().clone();
    match proof.phase {
        0 if info.frame.0 >= 5 => {
            proof.external = world
                .resource::<CanvasTextures>()
                .0
                .get(&TextureHandle(1))
                .copied();
            capture(world, &mut proof);
            proof.phase = 1;
        }
        1 => {
            if let Some(result) = world
                .resource_mut::<DesktopCaptureResults>()
                .take(proof.capture.unwrap())
            {
                let image = result.unwrap();
                pixel(&image, 16., [255, 0, 0]);
                pixel(&image, 48., [0, 255, 0]);
                pixel(&image, 88., [255, 240, 80]);
                world.get_mut::<CanvasOutput>(proof.first).unwrap().0.size = [32, 24];
                proof.entered = info.frame;
                proof.phase = 2;
            }
        }
        2 if info.frame.0 >= proof.entered.0 + 2 => {
            assert_eq!(
                world
                    .resource::<CanvasTextures>()
                    .0
                    .get(&TextureHandle(1))
                    .copied(),
                proof.external,
                "resize must retain external texture ID"
            );
            world.get_mut::<Source>(proof.first).unwrap().panic = true;
            world.get_mut::<Source>(proof.second).unwrap().color = wgpu::Color::BLUE;
            proof.entered = info.frame;
            proof.phase = 3;
        }
        3 if info.frame.0 >= proof.entered.0 + 3 => {
            assert!(
                world
                    .get::<CanvasFailure>(proof.first)
                    .unwrap()
                    .0
                    .contains("intentional per-panel")
            );
            assert!(world.get::<Source>(proof.second).unwrap().calls > 5);
            capture(world, &mut proof);
            proof.phase = 4;
        }
        4 => {
            if let Some(result) = world
                .resource_mut::<DesktopCaptureResults>()
                .take(proof.capture.unwrap())
            {
                let image = result.unwrap();
                pixel(&image, 48., [0, 0, 255]);
                pixel(&image, 88., [255, 240, 80]);
                world.entity_mut(proof.first).remove::<CanvasOutput>();
                proof.entered = info.frame;
                proof.phase = 5;
                eprintln!("canvas proof: ordering/icons/resize and isolated renderer panic passed");
            }
        }
        5 if info.frame.0 > proof.entered.0 => {
            assert!(
                !world
                    .resource::<CanvasTextures>()
                    .0
                    .contains_key(&TextureHandle(1))
            );
            let gpu = world.resource::<DesktopGpu>().clone();
            proof.generation = gpu.generation;
            gpu.device.destroy();
            let _ = gpu.device.poll(wgpu::PollType::Poll);
            proof.phase = 6;
        }
        6 if world
            .get_resource::<DesktopGpu>()
            .is_some_and(|gpu| gpu.generation > proof.generation) =>
        {
            proof.entered = info.frame;
            proof.phase = 7;
        }
        7 if info.frame.0 >= proof.entered.0 + 4 => {
            assert!(
                world
                    .get::<CanvasFailure>(proof.first)
                    .unwrap()
                    .0
                    .contains("intentional per-panel")
            );
            capture(world, &mut proof);
            proof.phase = 8;
        }
        8 => {
            if let Some(result) = world
                .resource_mut::<DesktopCaptureResults>()
                .take(proof.capture.unwrap())
            {
                let image = result.unwrap();
                pixel(&image, 48., [0, 0, 255]);
                pixel(&image, 88., [255, 240, 80]);
                assert!(image.info.generation > proof.generation);
                assert!(world.get::<Source>(proof.second).is_some());
                eprintln!(
                    "canvas proof: safe retirement, persistent quarantine and generation-aware canvas/icon replacement passed"
                );
                proof.verified.store(true, Ordering::SeqCst);
                world.write_message(bevy::app::AppExit::Success);
                proof.phase = 9;
            }
        }
        _ => {}
    }
    world.insert_resource(proof);
}

fn pixel(image: &DesktopCapturedFrame, x: f32, expected: [u8; 3]) {
    let rgba = &image.images[0].rgba;
    let scale = image.info.framebuffer_scale;
    let index = (((16. * scale[1]) as u32 * rgba.size[0] + (x * scale[0]) as u32) * 4) as usize;
    let actual = &rgba.pixels[index..index + 3];
    assert!(
        actual.iter().zip(expected).all(|(a, b)| a.abs_diff(b) <= 1),
        "pixel mismatch at {x}: {actual:?} versus {expected:?}"
    );
}
