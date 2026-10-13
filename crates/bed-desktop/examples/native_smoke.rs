//! Native proof: current-frame canvas sampling, presentation/screenshot color,
//! skipped-camera snapshot reconciliation, resize and same-App device recovery.
use bed_desktop::{bevy::prelude::*, *};
use dear_imgui_rs::{Color, Ui};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

#[derive(Resource)]
struct Proof {
    phase: u8,
    entered: u64,
    scale: [f32; 2],
    generation: u64,
    pending_loss: bool,
    image: Option<(u64, wgpu::Texture, dear_imgui_wgpu::ExternalTextureId)>,
    directory: PathBuf,
    deadline: Instant,
}

fn main() -> std::io::Result<()> {
    let directory = std::env::temp_dir().join(format!("bed-desktop-proof-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let launch = match bed_app::Application::new("bed.proof.native-smoke", "Desktop proof")?
        .dispatch(std::env::args_os())?
    {
        bed_app::LaunchOutcome::Desktop(launch) => *launch,
        bed_app::LaunchOutcome::Handled(exit) => std::process::exit(exit),
    };
    let mut app = launch
        .options(DesktopPlugins {
            window: Window {
                title: "Bed desktop GPU proof".into(),
                resolution: (180, 100).into(),
                ..default()
            },
            ..default()
        })
        .presentation_mode(bed_app::bed_core::CorePresentationMode::Application)
        .config_directory(
            std::env::temp_dir().join(format!("bed.proof.native-smoke-{}", std::process::id())),
        )
        .build(|app| {
            app.insert_resource(Proof {
                phase: 0,
                entered: 0,
                scale: [1.; 2],
                generation: 0,
                pending_loss: false,
                image: None,
                directory,
                deadline: Instant::now() + Duration::from_secs(45),
            })
            .insert_resource(DesktopUi(draw))
            .add_systems(First, inject_loss_before_ui)
            .add_systems(PreUpdate, advance.in_set(DesktopSystems::AfterUi));
            Ok(())
        })?;
    if app.run().is_error() {
        return Err(std::io::Error::other("Desktop proof exited with an error"));
    }
    Ok(())
}

fn draw(world: &mut World, ui: &Ui) {
    let gpu = world.resource::<DesktopGpu>().clone();
    let mut proof = world.remove_resource::<Proof>().unwrap();
    proof.scale = ui.io().display_framebuffer_scale();
    let draw = ui.get_background_draw_list();
    draw.add_rect(
        [0., 0.],
        ui.io().display_size(),
        Color::from_rgba_bytes(64, 96, 128, 255),
    )
    .filled(true)
    .build();
    if !proof
        .image
        .as_ref()
        .is_some_and(|image| image.0 == gpu.generation)
    {
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("desktop proof canvas"),
            size: wgpu::Extent3d {
                width: 4,
                height: 4,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let id = gpu
            .register_external_texture(&texture.create_view(&Default::default()))
            .unwrap();
        proof.image = Some((gpu.generation, texture, id));
    }
    let (_, texture, id) = proof.image.as_ref().unwrap();
    let view = texture.create_view(&Default::default());
    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("desktop proof canvas producer"),
        });
    {
        let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 160. / 255.,
                        g: 32. / 255.,
                        b: 80. / 255.,
                        a: 1.,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
    }
    world
        .resource_mut::<DesktopFrame>()
        .canvases
        .push(encoder.finish());
    draw.add_image(
        id.texture_id(),
        [80., 20.],
        [140., 60.],
        [0., 0.],
        [1., 1.],
        Color::WHITE,
    );
    world.insert_resource(proof);
}

fn advance(world: &mut World) {
    let status = world.resource::<DesktopRenderStatus>().read();
    let mut proof = world.remove_resource::<Proof>().unwrap();
    if Instant::now() > proof.deadline {
        panic!(
            "desktop proof timed out in phase {}: {:?}",
            proof.phase, status.error
        );
    }
    let completed = world
        .resource_mut::<DesktopScreenshotResult>()
        .completed
        .take();
    if let Some(error) = world.resource_mut::<DesktopScreenshotResult>().error.take() {
        panic!("screenshot failed: {error}");
    }
    match proof.phase {
        0 if status.completed_frames >= 5 => {
            world.resource_mut::<DesktopScreenshotRequest>().path =
                Some(proof.directory.join("initial.ppm"));
            proof.phase = 1;
        }
        1 if completed.is_some() => {
            assert_pixels(completed.as_ref().unwrap(), proof.scale);
            proof.entered = status.completed_frames;
            let mut cameras = world.query::<&mut Camera>();
            for mut camera in cameras.iter_mut(world) {
                camera.is_active = false;
            }
            proof.phase = 2;
            eprintln!("desktop proof: canvas/presentation/gamma pixels passed; skipping frames");
        }
        2 => {
            assert_eq!(
                status.completed_frames, proof.entered,
                "inactive camera presented a frame"
            );
            proof.phase = 3;
        }
        3 => {
            assert_eq!(status.completed_frames, proof.entered);
            let mut cameras = world.query::<&mut Camera>();
            for mut camera in cameras.iter_mut(world) {
                camera.is_active = true;
            }
            let mut windows = world.query::<&mut Window>();
            for mut window in windows.iter_mut(world) {
                window.resolution.set(196., 120.);
            }
            proof.phase = 4;
        }
        4 if status.completed_frames >= proof.entered + 4 => {
            world.resource_mut::<DesktopScreenshotRequest>().path =
                Some(proof.directory.join("resized.ppm"));
            proof.phase = 5;
        }
        5 if completed.is_some() => {
            assert_pixels(completed.as_ref().unwrap(), proof.scale);
            let gpu = world.resource::<DesktopGpu>().clone();
            proof.generation = gpu.generation;
            // No main-thread GPU allocations occur between destroy and Bevy's
            // pre_extract loss handling; this frame's snapshot is discarded.
            gpu.device.destroy();
            let _ = gpu.device.poll(wgpu::PollType::Poll);
            proof.phase = 6;
            eprintln!("desktop proof: skipped frames and resize passed; replacing lost device");
        }
        6 if world
            .get_resource::<DesktopGpu>()
            .is_some_and(|gpu| gpu.generation > proof.generation) =>
        {
            proof.entered = status.completed_frames;
            proof.phase = 7;
        }
        7 if status.completed_frames >= proof.entered + 5 => {
            world.resource_mut::<DesktopScreenshotRequest>().path =
                Some(proof.directory.join("recovered.ppm"));
            proof.phase = 8;
        }
        8 if completed.is_some() => {
            assert_pixels(completed.as_ref().unwrap(), proof.scale);
            assert_eq!(status.recoveries, 1);
            eprintln!(
                "desktop proof: same-world recovery and reuploaded fonts/external textures passed"
            );
            proof.pending_loss = true;
            proof.phase = 9;
        }
        9 if world
            .get_resource::<DesktopGpu>()
            .is_some_and(|gpu| gpu.generation > proof.generation) =>
        {
            proof.entered = status.completed_frames;
            proof.phase = 10;
        }
        10 if status.completed_frames >= proof.entered + 5 => {
            world.resource_mut::<DesktopScreenshotRequest>().path =
                Some(proof.directory.join("loss-before-ui.ppm"));
            proof.phase = 11;
        }
        11 if completed.is_some() => {
            assert_pixels(completed.as_ref().unwrap(), proof.scale);
            assert_eq!(status.recoveries, 2);
            assert!(
                status.error.is_none(),
                "recovery retained an error: {:?}",
                status.error
            );
            eprintln!("desktop proof: loss before UI recovered without a host skip flag");
            world.write_message(bevy::app::AppExit::Success);
        }
        _ => {}
    }
    world.insert_resource(proof);
}

fn inject_loss_before_ui(mut proof: ResMut<Proof>, gpu: Option<Res<DesktopGpu>>) {
    if !proof.pending_loss {
        return;
    }
    let gpu = gpu.expect("previous frame presented on an active GPU");
    proof.pending_loss = false;
    proof.generation = gpu.generation;
    gpu.device.destroy();
    let _ = gpu.device.poll(wgpu::PollType::Poll);
}

fn assert_pixels(path: &std::path::Path, scale: [f32; 2]) {
    let bytes = std::fs::read(path).unwrap();
    let mut lines = bytes.splitn(4, |byte| *byte == b'\n');
    assert_eq!(lines.next().unwrap(), b"P6");
    let size = std::str::from_utf8(lines.next().unwrap())
        .unwrap()
        .split_whitespace()
        .map(|part| part.parse::<usize>().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(lines.next().unwrap(), b"255");
    let pixels = lines.next().unwrap();
    for (point, expected) in [
        ([25., 25.], [64u8, 96, 128]),
        ([110., 40.], [160u8, 32, 80]),
    ] {
        let [x, y] =
            [point[0] * scale[0], point[1] * scale[1]].map(|coordinate| coordinate as usize);
        let offset = (y * size[0] + x) * 3;
        let actual = &pixels[offset..offset + 3];
        for (actual, expected) in actual.iter().zip(expected) {
            assert!(
                actual.abs_diff(expected) <= 2,
                "{} pixel {point:?}: {actual} expected {expected}",
                path.display()
            );
        }
    }
}
