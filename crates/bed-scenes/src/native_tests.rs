//! Opt-in validation of every decorative scene on Bed's shared GPU contract.
use super::*;
use bed_workbench_api::gpu::{renderer_device_descriptor, wgpu};

#[test]
#[ignore = "requires a native GPU adapter; run with --ignored on desktop CI"]
fn native_scenes_render_geometry_and_react_to_mascot_input() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = bevy::tasks::block_on(instance.request_adapter(&Default::default())).unwrap();
    let (device, queue) =
        bevy::tasks::block_on(adapter.request_device(&renderer_device_descriptor(&adapter)))
            .unwrap();
    let mut encoder = device.create_command_encoder(&Default::default());
    let mut gpu = GpuContext {
        instance: &instance,
        adapter: &adapter,
        device: &device,
        queue: &queue,
        encoder: &mut encoder,
        generation: 1,
        error_handlers: None,
    };
    let capture = std::env::var_os("BED_SCENE_CAPTURE_DIR").map(std::path::PathBuf::from);
    let size = if capture.is_some() {
        [512, 384]
    } else {
        [64; 2]
    };
    for kind in [
        SceneKind::Bed,
        SceneKind::WelcomeBed,
        SceneKind::Duck,
        SceneKind::Bedtime,
    ] {
        let size = if capture.is_some() && kind == SceneKind::Bedtime {
            [1280, 960]
        } else {
            size
        };
        let mut scene = renderer::SceneRenderer::new(&gpu, kind).unwrap();
        let target = RenderTarget::new(
            &device,
            RenderOutput {
                handle: TextureHandle::next(),
                size,
                depth: false,
                revision: 1,
            },
        )
        .unwrap();
        let frame = SceneFrame::default();
        for _ in 0..12 {
            scene.render(&mut gpu, &target, frame, 0.0, 0.0).unwrap();
        }
        let before = pixels(&device, &queue, &target);
        if let Some(directory) = &capture {
            image::save_buffer(
                directory.join(format!(
                    "bed-creative-tools-scene-{}.png",
                    format!("{kind:?}").to_lowercase()
                )),
                &before,
                size[0],
                size[1],
                image::ColorType::Rgba8,
            )
            .unwrap();
        }
        let background = &before[..4];
        let geometry = before
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|pixel| {
                pixel
                    .iter()
                    .zip(background)
                    .any(|(a, b)| a.abs_diff(*b) > 12)
            })
            .count();
        assert!(
            geometry > 100,
            "{kind:?} rendered only {geometry} geometry pixels"
        );
        if kind == SceneKind::Bedtime {
            assert_eq!(before[3], 255, "{kind:?} lost its opaque background");
        } else {
            for (x, y) in [(0, 0), (size[0] - 1, 0)] {
                assert_eq!(
                    before[((y * size[0] + x) * 4 + 3) as usize],
                    0,
                    "{kind:?} background should be transparent"
                );
            }
            assert!(
                before
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .any(|pixel| pixel[3] == 255),
                "{kind:?} did not render opaque geometry"
            );
            assert!(
                before
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .any(|pixel| pixel[3] > 0 && pixel[3] < 255),
                "{kind:?} silhouette was not antialiased"
            );
        }
        if matches!(kind, SceneKind::Bed | SceneKind::WelcomeBed) {
            let tinted = SceneFrame {
                accent: [0.9, 0.2, 0.35, 1.0],
                animations: false,
                ..frame
            };
            for _ in 0..3 {
                scene.render(&mut gpu, &target, tinted, 0.0, 0.0).unwrap();
            }
            let after = pixels(&device, &queue, &target);
            if let Some(directory) = &capture {
                image::save_buffer(
                    directory.join(format!("bed-{kind:?}-tinted.png")),
                    &after,
                    size[0],
                    size[1],
                    image::ColorType::Rgba8,
                )
                .unwrap();
            }
            let changed = before
                .as_chunks::<4>()
                .0
                .iter()
                .zip(after.as_chunks::<4>().0)
                .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(a, b)| a.abs_diff(*b) > 30))
                .count();
            assert!(
                changed > geometry / 4,
                "{kind:?} covers did not follow the theme accent"
            );
            assert!(
                before
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .zip(after.as_chunks::<4>().0)
                    .all(|(a, b)| a[3] == b[3]),
                "{kind:?} theme change altered the silhouette"
            );
            let sheets: Vec<_> = before
                .as_chunks::<4>()
                .0
                .iter()
                .zip(after.as_chunks::<4>().0)
                .filter(|(pixel, _)| {
                    pixel[3] == 255
                        && pixel[..3].iter().min().unwrap() > &80
                        && pixel[..3].iter().max().unwrap() - pixel[..3].iter().min().unwrap() < 3
                })
                .collect();
            assert!(sheets.len() > 5, "{kind:?} sheets did not stay white");
            // MSAA blends a few sheet-edge pixels with the adjacent blanket.
            let unchanged_sheets = sheets.iter().filter(|(a, b)| a == b).count();
            assert!(
                unchanged_sheets * 100 > sheets.len() * 95,
                "{kind:?} theme accent changed the white sheets: only {unchanged_sheets}/{} pixels stayed white",
                sheets.len()
            );
            for _ in 0..3 {
                scene.render(&mut gpu, &target, frame, 0.0, 0.0).unwrap();
            }
            assert_eq!(before, pixels(&device, &queue, &target));
        }
        if kind == SceneKind::Bed {
            for (mouse, impulse, message) in [
                ([0.8, -0.8], 0.0, "Bed did not rotate with the pointer"),
                ([0.0; 2], 1.0, "Bed did not squash when clicked"),
            ] {
                for _ in 0..3 {
                    scene
                        .render(
                            &mut gpu,
                            &target,
                            SceneFrame { mouse, ..frame },
                            0.0,
                            impulse,
                        )
                        .unwrap();
                }
                assert_ne!(before, pixels(&device, &queue, &target), "{message}");
            }
        }
        if kind == SceneKind::WelcomeBed {
            for time in [10.0, 20.0, 30.0, 39.0] {
                for _ in 0..3 {
                    scene.render(&mut gpu, &target, frame, time, 0.0).unwrap();
                }
                let image = pixels(&device, &queue, &target);
                assert_ne!(before, image, "Welcome bed did not rotate at {time}s");
                for (index, pixel) in image.as_chunks::<4>().0.iter().enumerate() {
                    let x = index as u32 % size[0];
                    let y = index as u32 / size[0];
                    if x == 0 || y == 0 || x == size[0] - 1 || y == size[1] - 1 {
                        assert_eq!(pixel[3], 0, "Welcome bed was cropped at {time}s");
                    }
                }
            }
        }
        if kind == SceneKind::Bed {
            for size in [[256, 512], [512, 256]] {
                let target = RenderTarget::new(
                    &device,
                    RenderOutput {
                        handle: TextureHandle::next(),
                        size,
                        depth: false,
                        revision: 1,
                    },
                )
                .unwrap();
                for (mouse, impulse) in [([0.0; 2], 0.0), ([1.0, -1.0], 1.0), ([-1.0, 1.0], 1.0)] {
                    for _ in 0..3 {
                        scene
                            .render(
                                &mut gpu,
                                &target,
                                SceneFrame { mouse, ..frame },
                                0.0,
                                impulse,
                            )
                            .unwrap();
                    }
                    let image = pixels(&device, &queue, &target);
                    for (index, pixel) in image.as_chunks::<4>().0.iter().enumerate() {
                        let x = index as u32 % size[0];
                        let y = index as u32 / size[0];
                        if x == 0 || x == size[0] - 1 || y == 0 || y == size[1] - 1 {
                            assert_eq!(pixel[3], 0, "{kind:?} was cropped in {size:?}");
                        }
                    }
                    assert!(image.as_chunks::<4>().0.iter().any(|pixel| pixel[3] == 255));
                    if mouse == [0.0; 2]
                        && let Some(directory) = &capture
                    {
                        image::save_buffer(
                            directory.join(format!("bed-{kind:?}-{}x{}.png", size[0], size[1])),
                            &image,
                            size[0],
                            size[1],
                            image::ColorType::Rgba8,
                        )
                        .unwrap();
                    }
                }
            }
        }
        if kind == SceneKind::Duck {
            for (x, y) in [(0, size[1] - 1), (size[0] - 1, size[1] - 1)] {
                assert_eq!(before[((y * size[0] + x) * 4 + 3) as usize], 0);
            }
            // Only the neck pitch follows vertical cursor movement; the camera
            // and body keep their pose. Exercise it without bob or click squash.
            let looking_up = SceneFrame {
                mouse: [0.0, -0.8],
                ..frame
            };
            for _ in 0..3 {
                scene
                    .render(&mut gpu, &target, looking_up, 0.0, 0.0)
                    .unwrap();
            }
            assert_ne!(
                before,
                pixels(&device, &queue, &target),
                "Duck neck did not follow the vertical cursor position"
            );
            for size in [[256, 512], [512, 256]] {
                let target = RenderTarget::new(
                    &device,
                    RenderOutput {
                        handle: TextureHandle::next(),
                        size,
                        depth: false,
                        revision: 1,
                    },
                )
                .unwrap();
                for (mouse, impulse) in [
                    ([0.0; 2], 0.0),
                    ([0.9, -0.9], 0.0),
                    ([-0.9, 0.9], 0.0),
                    ([0.9, -0.9], 1.0),
                    ([-0.9, 0.9], 1.0),
                ] {
                    for _ in 0..3 {
                        scene
                            .render(
                                &mut gpu,
                                &target,
                                SceneFrame { mouse, ..frame },
                                0.0,
                                impulse,
                            )
                            .unwrap();
                    }
                    let image = pixels(&device, &queue, &target);
                    let mut min = size;
                    let mut max = [0; 2];
                    for (index, pixel) in image.as_chunks::<4>().0.iter().enumerate() {
                        if pixel[3] == 0 {
                            continue;
                        }
                        let position = [index as u32 % size[0], index as u32 / size[0]];
                        for axis in 0..2 {
                            min[axis] = min[axis].min(position[axis]);
                            max[axis] = max[axis].max(position[axis]);
                        }
                    }
                    assert!(
                        min[0] > 0 && min[1] > 0 && max[0] < size[0] - 1 && max[1] < size[1] - 1,
                        "Duck is cropped in {size:?}, mouse {mouse:?}, impulse {impulse}: {min:?}..{max:?}"
                    );
                    let occupancy = ((max[0] - min[0] + 1) as f32 / size[0] as f32)
                        .max((max[1] - min[1] + 1) as f32 / size[1] as f32);
                    assert!(
                        occupancy > 0.65,
                        "Duck only filled {:.0}% of its limiting dimension in {size:?}, mouse {mouse:?}, impulse {impulse}",
                        occupancy * 100.0
                    );
                    if mouse == [0.0; 2]
                        && let Some(directory) = &capture
                    {
                        image::save_buffer(
                            directory.join(format!("bed-duck-{}x{}.png", size[0], size[1])),
                            &image,
                            size[0],
                            size[1],
                            image::ColorType::Rgba8,
                        )
                        .unwrap();
                    }
                }
            }
        }
        if kind != SceneKind::Bedtime {
            let changed = SceneFrame {
                mouse: [0.8, 0.0],
                ..frame
            };
            for _ in 0..3 {
                scene.render(&mut gpu, &target, changed, 1.0, 1.0).unwrap();
            }
            assert_ne!(
                before,
                pixels(&device, &queue, &target),
                "{kind:?} did not react to changed input"
            );
        } else {
            for _ in 0..3 {
                scene.render(&mut gpu, &target, frame, 25.0, 0.0).unwrap();
            }
            let moved = pixels(&device, &queue, &target);
            assert_ne!(before, moved, "Bedtime scene did not animate");
            if let Some(directory) = &capture {
                image::save_buffer(
                    directory.join("bedtime-camera-25s.png"),
                    &moved,
                    size[0],
                    size[1],
                    image::ColorType::Rgba8,
                )
                .unwrap();
            }
            let still = SceneFrame {
                animations: false,
                ..frame
            };
            for _ in 0..3 {
                scene.render(&mut gpu, &target, still, 0.0, 0.0).unwrap();
            }
            let frozen = pixels(&device, &queue, &target);
            for _ in 0..3 {
                scene.render(&mut gpu, &target, still, 25.0, 0.0).unwrap();
            }
            assert_eq!(
                frozen,
                pixels(&device, &queue, &target),
                "Bedtime moved with animations disabled"
            );
            let themed = SceneFrame {
                accent: [0.85, 0.22, 0.50, 1.0],
                ..still
            };
            for _ in 0..3 {
                scene.render(&mut gpu, &target, themed, 0.0, 0.0).unwrap();
            }
            let tinted = pixels(&device, &queue, &target);
            let changed = frozen
                .as_chunks::<4>()
                .0
                .iter()
                .zip(tinted.as_chunks::<4>().0)
                .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(a, b)| a.abs_diff(*b) > 12))
                .count();
            assert!(
                changed > geometry / 8,
                "Bedtime furnishings and lighting did not follow the theme accent"
            );
            if let Some(directory) = &capture {
                image::save_buffer(
                    directory.join("bedtime-rose-theme.png"),
                    &tinted,
                    size[0],
                    size[1],
                    image::ColorType::Rgba8,
                )
                .unwrap();
                let target = RenderTarget::new(
                    &device,
                    RenderOutput {
                        handle: TextureHandle::next(),
                        size: [512, 384],
                        depth: false,
                        revision: 1,
                    },
                )
                .unwrap();
                let mut animation = image::codecs::gif::GifEncoder::new(
                    std::fs::File::create(directory.join("bedtime-motion.gif")).unwrap(),
                );
                animation
                    .set_repeat(image::codecs::gif::Repeat::Infinite)
                    .unwrap();
                for step in 0..32 {
                    for _ in 0..2 {
                        scene
                            .render(&mut gpu, &target, frame, step as f32 * 0.5, 0.0)
                            .unwrap();
                    }
                    let rgba =
                        image::RgbaImage::from_raw(512, 384, pixels(&device, &queue, &target))
                            .unwrap();
                    animation
                        .encode_frame(image::Frame::from_parts(
                            rgba,
                            0,
                            0,
                            image::Delay::from_numer_denom_ms(500, 1),
                        ))
                        .unwrap();
                }
            }
            for size in [[256, 512], [512, 256]] {
                let target = RenderTarget::new(
                    &device,
                    RenderOutput {
                        handle: TextureHandle::next(),
                        size,
                        depth: false,
                        revision: 1,
                    },
                )
                .unwrap();
                for _ in 0..3 {
                    scene.render(&mut gpu, &target, frame, 25.0, 0.0).unwrap();
                }
                let image = pixels(&device, &queue, &target);
                let background = &image[..4];
                let visible = image
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .filter(|pixel| {
                        pixel[..3]
                            .iter()
                            .zip(background)
                            .any(|(a, b)| a.abs_diff(*b) > 12)
                    })
                    .count();
                assert!(visible > 100, "Bedtime room disappeared at {size:?}");
                if let Some(directory) = &capture {
                    image::save_buffer(
                        directory.join(format!("bedtime-{}x{}.png", size[0], size[1])),
                        &image,
                        size[0],
                        size[1],
                        image::ColorType::Rgba8,
                    )
                    .unwrap();
                }
            }
        }
    }
}

fn pixels(device: &wgpu::Device, queue: &wgpu::Queue, target: &RenderTarget) -> Vec<u8> {
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Decorative scene pixels"),
        size: u64::from(target.size[0]) * 4 * u64::from(target.size[1]),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        target.texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(target.size[0] * 4),
                rows_per_image: Some(target.size[1]),
            },
        },
        target.texture.size(),
    );
    queue.submit([encoder.finish()]);
    let (sender, receiver) = std::sync::mpsc::channel();
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            sender.send(result).unwrap();
        });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(Duration::from_secs(30)),
        })
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(30))
        .unwrap()
        .unwrap();
    let result = buffer.slice(..).get_mapped_range().to_vec();
    buffer.unmap();
    result
}
