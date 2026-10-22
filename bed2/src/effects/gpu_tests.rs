//! Opt-in native regression: actual Bevy pipelines, image readback and GPU timestamps.
use super::*;
use bevy::{
    app::PluginsState,
    render::{
        RenderPlugin,
        pipelined_rendering::PipelinedRenderingPlugin,
        renderer::RenderQueue,
        settings::{RenderCreation, WgpuSettings},
    },
    winit::WinitPlugin,
};
use std::time::{Duration, Instant};

fn host() -> (App, CachedRenderPipelineId) {
    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: None,
                exit_condition: bevy::window::ExitCondition::DontExit,
                ..default()
            })
            .set(RenderPlugin {
                render_creation: RenderCreation::Automatic(Box::new(WgpuSettings {
                    features: WgpuFeatures::TIMESTAMP_QUERY,
                    ..default()
                })),
                synchronous_pipeline_compilation: true,
                ..default()
            })
            .disable::<WinitPlugin>()
            .disable::<PipelinedRenderingPlugin>(),
    )
    .init_resource::<GlobalPreferences>()
    .add_plugins(EffectsPlugin);
    let deadline = Instant::now() + Duration::from_secs(30);
    while app.plugins_state() != PluginsState::Ready {
        assert!(
            Instant::now() < deadline,
            "Renderer initialization timed out"
        );
        std::thread::yield_now();
    }
    app.finish();
    app.cleanup();
    app.update();
    let reference_shader = app
        .world()
        .resource::<AssetServer>()
        .load("embedded://bed2/effects/fixtures/original_bloom.wesl");
    let world = app.sub_app(RenderApp).world();
    let pipeline = world.resource::<EffectPipeline>();
    let cache = world.resource::<PipelineCache>();
    let reference = cache.queue_render_pipeline(RenderPipelineDescriptor {
        label: Some("Original full-resolution bloom reference".into()),
        layout: vec![pipeline.layout.clone()],
        vertex: world.resource::<FullscreenShader>().to_vertex_state(),
        fragment: Some(FragmentState {
            shader: reference_shader,
            entry_point: Some("crt".into()),
            targets: vec![Some(ColorTargetState {
                format: TextureFormat::Rgba8UnormSrgb,
                blend: None,
                write_mask: ColorWrites::ALL,
            })],
            ..default()
        }),
        ..default()
    });
    loop {
        app.update();
        let world = app.sub_app(RenderApp).world();
        let pipeline = world.resource::<EffectPipeline>();
        let cache = world.resource::<PipelineCache>();
        if [
            reference,
            pipeline.crt,
            pipeline.bloom_downsample,
            pipeline.bloom_horizontal,
            pipeline.bloom_vertical,
        ]
        .into_iter()
        .all(|id| cache.get_render_pipeline(id).is_some())
        {
            break;
        }
        assert!(Instant::now() < deadline, "Shader compilation timed out");
    }
    (app, reference)
}

fn read_buffer(device: &RenderDevice, buffer: &wgpu::Buffer) -> Vec<u8> {
    let (send, receive) = std::sync::mpsc::channel();
    buffer.slice(..).map_async(MapMode::Read, move |result| {
        send.send(result).unwrap();
    });
    device
        .poll(PollType::Wait {
            submission_index: None,
            timeout: Some(Duration::from_secs(30)),
        })
        .unwrap();
    receive
        .recv_timeout(Duration::from_secs(30))
        .unwrap()
        .unwrap();
    let data = buffer.slice(..).get_mapped_range().unwrap().to_vec();
    buffer.unmap();
    data
}

/// Render the same image through the old CRT or the complete reduced bloom + CRT
/// path. Timing includes all bloom passes and final CRT, excluding upload/readback.
fn render_fixture(
    world: &World,
    reference: Option<CachedRenderPipelineId>,
    size: UVec2,
    input: &[u8],
    intensity: f32,
) -> (Vec<u8>, f64) {
    let device = world.resource::<RenderDevice>();
    let queue = world.resource::<RenderQueue>();
    let pipeline = world.resource::<EffectPipeline>();
    let cache = world.resource::<PipelineCache>();
    let full_size = Extent3d {
        width: size.x,
        height: size.y,
        depth_or_array_layers: 1,
    };
    let texture = |size, format, usage| {
        device.create_texture(&TextureDescriptor {
            label: Some("Bloom GPU fixture"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        })
    };
    let source = texture(
        full_size,
        TextureFormat::Rgba8UnormSrgb,
        TextureUsages::COPY_DST | TextureUsages::TEXTURE_BINDING,
    );
    queue.write_texture(
        source.as_image_copy(),
        input,
        TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(size.x * 4),
            rows_per_image: Some(size.y),
        },
        full_size,
    );
    let output = texture(
        full_size,
        TextureFormat::Rgba8UnormSrgb,
        TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC,
    );
    let reduced = std::array::from_fn::<_, 2, _>(|_| {
        texture(
            bloom_extent(full_size),
            BLOOM_FORMAT,
            TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING,
        )
    });
    let source = source.create_view(&default());
    let output_view = output.create_view(&default());
    let reduced_views = reduced.each_ref().map(|t| t.create_view(&default()));
    let settings = Settings {
        bloom_intensity: intensity,
        scanline_intensity: 0.0,
        vignette_intensity: 0.0,
        ..default()
    };
    let mut uniform = UniformBuffer::from(EffectUniform::new(&settings, &Time::<Real>::default()));
    uniform.write_buffer(device, queue);
    let group = |source: &TextureView, bloom: &TextureView| {
        device.create_bind_group(
            "Bloom GPU fixture",
            &cache.get_bind_group_layout(&pipeline.layout),
            &BindGroupEntries::sequential((
                source,
                &pipeline.sampler,
                uniform.binding().unwrap(),
                source,
                bloom,
            )),
        )
    };
    let downsample_group = group(&source, &source);
    let horizontal_group = group(&reduced_views[0], &source);
    let vertical_group = group(&reduced_views[1], &source);
    let final_group = group(
        &source,
        if reference.is_none() && intensity > 0.0 {
            &reduced_views[0]
        } else {
            &source
        },
    );
    let queries = device
        .wgpu_device()
        .create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("Bloom timings"),
            ty: wgpu::QueryType::Timestamp,
            count: 2,
        });
    let timestamps = device.create_buffer(&BufferDescriptor {
        label: Some("Bloom timestamps"),
        size: 256,
        usage: BufferUsages::QUERY_RESOLVE | BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let time_readback = device.create_buffer(&BufferDescriptor {
        label: None,
        size: 16,
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let stride = (size.x * 4).div_ceil(256) * 256;
    let image_readback = device.create_buffer(&BufferDescriptor {
        label: None,
        size: u64::from(stride) * u64::from(size.y),
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut times = Vec::new();
    // Warm the pipelines/caches before collecting measurements. Multiple
    // passes are encoded to one submission; timestamps are in render passes so
    // Metal needs only TIMESTAMP_QUERY, not encoder timestamp support.
    for iteration in 0..10 {
        let mut encoder = device.create_command_encoder(&default());
        let mut pass = |id, group: &BindGroup, output: &TextureView, first: bool, last: bool| {
            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("Bloom benchmark"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: output,
                    depth_slice: None,
                    resolve_target: None,
                    ops: Operations::default(),
                })],
                timestamp_writes: (first || last).then_some(wgpu::RenderPassTimestampWrites {
                    query_set: &queries,
                    beginning_of_pass_write_index: first.then_some(0),
                    end_of_pass_write_index: last.then_some(1),
                }),
                ..default()
            });
            pass.set_pipeline(cache.get_render_pipeline(id).unwrap());
            pass.set_bind_group(0, group, &[0]);
            pass.draw(0..3, 0..1);
        };
        if reference.is_none() && intensity > 0.0 {
            pass(
                pipeline.bloom_downsample,
                &downsample_group,
                &reduced_views[0],
                true,
                false,
            );
            pass(
                pipeline.bloom_horizontal,
                &horizontal_group,
                &reduced_views[1],
                false,
                false,
            );
            pass(
                pipeline.bloom_vertical,
                &vertical_group,
                &reduced_views[0],
                false,
                false,
            );
        }
        pass(
            reference.unwrap_or(pipeline.crt),
            &final_group,
            &output_view,
            reference.is_some() || intensity == 0.0,
            true,
        );
        // Resolve only after completion: on M4 Pro/Metal (2026-10-22), resolving
        // in the measured command buffer returned stale end timestamps.
        queue.submit([encoder.finish()]);
        device
            .poll(PollType::Wait {
                submission_index: None,
                timeout: Some(Duration::from_secs(30)),
            })
            .unwrap();
        let mut encoder = device.create_command_encoder(&default());
        encoder.resolve_query_set(&queries, 0..2, &timestamps, 0);
        encoder.copy_buffer_to_buffer(&timestamps, 0, &time_readback, 0, 16);
        if iteration == 9 {
            encoder.copy_texture_to_buffer(
                output.as_image_copy(),
                TexelCopyBufferInfo {
                    buffer: &image_readback,
                    layout: TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(stride),
                        rows_per_image: Some(size.y),
                    },
                },
                full_size,
            );
        }
        queue.submit([encoder.finish()]);
        let bytes = read_buffer(device, &time_readback);
        let start = u64::from_ne_bytes(bytes[..8].try_into().unwrap());
        let end = u64::from_ne_bytes(bytes[8..16].try_into().unwrap());
        if iteration >= 3 {
            let elapsed = end.checked_sub(start).unwrap_or_else(|| panic!("Invalid GPU timestamp pair: reference={}, size={size:?}, iteration={iteration}, start={start}, end={end}", reference.is_some()));
            times.push(elapsed as f64 * f64::from(queue.get_timestamp_period()) / 1_000_000.0);
        }
    }
    times.sort_by(f64::total_cmp);
    let image = read_buffer(device, &image_readback)
        .chunks_exact(stride as usize)
        .flat_map(|row| row[..size.x as usize * 4].iter().copied())
        .collect();
    (image, times[times.len() / 2])
}

#[test]
#[ignore = "Native GPU image and timing regression; requires a timestamp-capable adapter"]
fn native_bloom_quality_and_gpu_cost() {
    let (app, reference) = host();
    let world = app.sub_app(RenderApp).world();
    let device = world.resource::<RenderDevice>();
    println!(
        "GPU: {}",
        world
            .resource::<bevy::render::renderer::RenderAdapterInfo>()
            .name
    );
    assert!(device.features().contains(WgpuFeatures::TIMESTAMP_QUERY));
    // Flat color must retain DC gain; alpha must remain source-owned even where
    // blurred light crosses into a transparent region. Odd dimensions cover resize.
    for size in [UVec2::new(256, 128), UVec2::new(257, 129)] {
        let flat: Vec<u8> = (0..size.x * size.y)
            .flat_map(|_| [40, 60, 80, 128])
            .collect();
        let (old, _) = render_fixture(world, Some(reference), size, &flat, 0.5);
        let (new, _) = render_fixture(world, None, size, &flat, 0.5);
        assert!(
            old.iter().zip(&new).all(|(a, b)| a.abs_diff(*b) <= 1),
            "Flat-color bloom gain changed"
        );
        let mut impulse = vec![0; size.x as usize * size.y as usize * 4];
        for y in 0..size.y {
            for x in 0..size.x {
                let i = ((y * size.x + x) * 4) as usize;
                if x < size.x / 2 {
                    impulse[i..i + 4].copy_from_slice(&[40, 60, 80, 128]);
                }
                if (x % 32 < 2) && y % 16 < 10 && x < size.x / 2 {
                    impulse[i..i + 4].copy_from_slice(&[120, 120, 120, 128]);
                }
            }
        }
        let (old, _) = render_fixture(world, Some(reference), size, &impulse, 0.5);
        let (new, _) = render_fixture(world, None, size, &impulse, 0.5);
        let error = old
            .iter()
            .zip(&new)
            .map(|(a, b)| f64::from(a.abs_diff(*b)))
            .sum::<f64>()
            / old.len() as f64;
        assert!(
            error < 3.0,
            "Bloom appearance changed excessively: mean error {error}"
        );
        for (source, output) in impulse.chunks_exact(4).zip(new.chunks_exact(4)) {
            assert_eq!(source[3], output[3], "Bloom changed alpha");
            if source[3] == 0 {
                assert_eq!(output, &[0, 0, 0, 0]);
            }
        }
    }
    let size = UVec2::new(3840, 2160);
    let input: Vec<u8> = (0..size.y)
        .flat_map(|y| {
            (0..size.x).flat_map(move |x| {
                if x % 32 < 2 && y % 16 < 10 {
                    [220, 220, 220, 255]
                } else {
                    [40, 50, 60, 255]
                }
            })
        })
        .collect();
    let (_, old_ms) = render_fixture(world, Some(reference), size, &input, 0.3);
    let (_, new_ms) = render_fixture(world, None, size, &input, 0.3);
    println!(
        "4K bloom + CRT median GPU time: original={old_ms:.3} ms, reduced={new_ms:.3} ms, speedup={:.2}x",
        old_ms / new_ms
    );
    assert!(new_ms < old_ms, "Reduced bloom failed to improve GPU time");
}
