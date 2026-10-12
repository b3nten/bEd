use crate::{
    DesktopFrameInfo, DesktopGpu, DesktopRenderStatus,
    capture::{CaptureBridge, DesktopCaptureError, DesktopCaptureId, unpack_pixels},
};
use bevy::{
    prelude::*,
    render::{
        Render, RenderApp, RenderSystems,
        renderer::{
            CurrentView, PendingCommandBuffers, RenderDevice, RenderGraph, RenderGraphSystems,
        },
        view::ViewTarget,
    },
};
use dear_imgui_rs::render::FrameSnapshot;
use dear_imgui_wgpu::{FramebufferExtent, WgpuSnapshotRenderer};

#[derive(bevy::ecs::schedule::ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct DesktopGraph;

/// Override final composition, for example to apply editor-specific effects.
/// Render the snapshot into the supplied final texture and append commands to
/// PendingCommandBuffers. Presentation and capture consume that same texture.
pub type DesktopCompositionFn = fn(
    &mut World,
    &mut WgpuSnapshotRenderer,
    FrameSnapshot,
    &wgpu::TextureView,
    wgpu::TextureFormat,
    [u32; 2],
) -> Result<(), String>;

#[derive(Resource, Clone, Copy)]
pub struct DesktopComposition(pub DesktopCompositionFn);

#[derive(Resource, Default)]
pub(crate) struct RenderFrame {
    pub snapshot: Option<FrameSnapshot>,
    pub canvases: Vec<wgpu::CommandBuffer>,
    pub size: [u32; 2],
    pub info: DesktopFrameInfo,
}

#[derive(Resource)]
pub(crate) struct DesktopRenderer {
    offscreen: wgpu::Texture,
    view: wgpu::TextureView,
    blit: wgpu::RenderPipeline,
    binding: wgpu::BindGroup,
    size: [u32; 2],
    output_format: wgpu::TextureFormat,
}

pub(crate) fn install(app: &mut App) {
    app.sub_app_mut(RenderApp)
        .init_schedule(DesktopGraph)
        .add_systems(DesktopGraph, render_desktop)
        .add_systems(
            RenderGraph,
            enqueue_canvases
                .after(RenderGraphSystems::Begin)
                .before(RenderGraphSystems::Render),
        )
        .init_resource::<Readbacks>()
        .add_systems(
            Render,
            (reconcile_unrendered, start_readbacks).in_set(RenderSystems::Cleanup),
        );
}

fn enqueue_canvases(
    mut frame: ResMut<RenderFrame>,
    mut pending: ResMut<PendingCommandBuffers>,
    devices: Res<crate::DeviceBridge>,
) {
    // Extraction commands may remain deferred while Bevy recovers. Discard
    // their old-device snapshot and buffers before any camera submits them.
    let generation = devices.0.lock().expect("device bridge poisoned").0;
    if frame.info.generation != generation {
        frame.snapshot.take();
        frame.canvases.clear();
        return;
    }
    pending.push(std::mem::take(&mut frame.canvases));
}

fn reconcile_unrendered(world: &mut World) {
    let snapshot = world.resource_mut::<RenderFrame>().snapshot.take();
    let Some(snapshot) = snapshot else {
        return;
    };
    let Some(gpu) = world.get_resource::<DesktopGpu>().cloned() else {
        drop(snapshot);
        return;
    };
    let result = gpu.with_renderer(|renderer| renderer.reconcile_snapshot(snapshot));
    if let Err(error) = result {
        world
            .resource::<DesktopRenderStatus>()
            .0
            .lock()
            .unwrap()
            .error = Some(error);
    }
}

fn render_desktop(world: &mut World) {
    let Some(snapshot) = world.resource_mut::<RenderFrame>().snapshot.take() else {
        return;
    };
    let Some(gpu) = world.get_resource::<DesktopGpu>().cloned() else {
        drop(snapshot);
        return;
    };
    let view = world.resource::<CurrentView>().0;
    let output = world.get::<ViewTarget>(view).and_then(|target| {
        let attachment = target.out_texture_color_attachment(None)?;
        let format = target.out_texture_view_format()?;
        Some((attachment.view.clone(), format))
    });
    let mut renderer = gpu
        .renderer
        .lock()
        .expect("desktop renderer mutex poisoned");
    let Some(renderer) = renderer.as_mut() else {
        drop(snapshot);
        return;
    };
    let info = &world.resource::<RenderFrame>().info;
    let requested = world.resource::<CaptureBridge>().wants_frame(info);
    let format = output.as_ref().map(|(_, format)| *format).or_else(|| {
        // Bevy keeps the configured surface format when Metal returns Occluded.
        // This is real surface metadata; the final composition never guesses a format.
        let target = world
            .get::<bevy::render::camera::ExtractedCamera>(view)?
            .target
            .as_ref()?;
        let bevy::camera::NormalizedRenderTarget::Window(window) = target else {
            return None;
        };
        let entity = window.entity();
        world
            .query::<(
                &bevy::render::sync_world::MainEntity,
                &bevy::render::view::ExtractedWindow,
            )>()
            .iter(world)
            .find(|(main, _)| main.id() == entity)
            .and_then(|(_, window)| {
                window.swap_chain_texture_view_format.or_else(|| {
                    window
                        .swap_chain_texture_format
                        .map(|format| format.add_srgb_suffix())
                })
            })
    });
    let result = if let Some(format) = format.filter(|_| output.is_some() || requested) {
        let size = world
            .get::<bevy::render::camera::ExtractedCamera>(view)
            .and_then(|camera| camera.physical_target_size)
            .map(|size| [size.x, size.y])
            .unwrap_or_else(|| {
                world
                    .resource::<RenderFrame>()
                    .size
                    .map(|value| value.max(1))
            });
        let compose = world
            .get_resource::<DesktopComposition>()
            .map(|compose| compose.0)
            .unwrap_or(compose_default);
        compose_final(
            world,
            renderer,
            snapshot,
            output.as_ref().map(|(output, _)| output),
            format,
            size,
            compose,
        )
    } else {
        renderer
            .reconcile_snapshot(snapshot)
            .map_err(|error| error.to_string())
    };
    let mut status = world.resource::<DesktopRenderStatus>().0.lock().unwrap();
    match result {
        Ok(()) if output.is_some() => status.completed_frames += 1,
        Ok(()) => {}
        Err(error) => {
            let info = &world.resource::<RenderFrame>().info;
            status.error = Some(format!(
                "{error} (desktop frame {}, captured GPU {}, renderer GPU {})",
                info.frame.0, info.generation, gpu.generation
            ));
        }
    }
}

/// The final texture is the authoritative pixel image for presentation and capture.
#[derive(Resource)]
struct FinalTarget(DesktopRenderer);
struct Readback {
    buffer: wgpu::Buffer,
    ids: Vec<DesktopCaptureId>,
    info: DesktopFrameInfo,
    stride: usize,
    format: wgpu::TextureFormat,
}
#[derive(Resource, Default)]
struct Readbacks(Vec<Readback>);

pub(crate) fn reset_capture_targets(world: &mut World) {
    world.remove_resource::<FinalTarget>();
    if let Some(mut pending) = world.get_resource_mut::<Readbacks>() {
        pending.0.clear();
    }
}

#[allow(clippy::too_many_arguments)]
fn compose_final(
    world: &mut World,
    imgui: &mut WgpuSnapshotRenderer,
    snapshot: FrameSnapshot,
    output: Option<&wgpu::TextureView>,
    format: wgpu::TextureFormat,
    size: [u32; 2],
    compose: DesktopCompositionFn,
) -> Result<(), String> {
    let device = world.resource::<RenderDevice>().wgpu_device().clone();
    if !world
        .get_resource::<FinalTarget>()
        .is_some_and(|target| target.0.size == size && target.0.output_format == format)
    {
        world.insert_resource(FinalTarget(DesktopRenderer::with_source(
            &device, size, format, format, false,
        )));
    }
    let final_view = world.resource::<FinalTarget>().0.view.clone();
    compose(world, imgui, snapshot, &final_view, format, size)?;
    let info = world.resource::<RenderFrame>().info.clone();
    // A resize between UI construction and surface acquisition must not give
    // old geometry new pixel dimensions. Wait for a matching frontend frame.
    let ids = if info.physical_size == size {
        world.resource::<CaptureBridge>().begin(&info)
    } else {
        Vec::new()
    };
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("Desktop final presentation/capture"),
    });
    let target = world.resource::<FinalTarget>();
    let supported = matches!(
        format,
        wgpu::TextureFormat::Rgba8Unorm
            | wgpu::TextureFormat::Rgba8UnormSrgb
            | wgpu::TextureFormat::Bgra8Unorm
            | wgpu::TextureFormat::Bgra8UnormSrgb
    );
    if !ids.is_empty() && !supported {
        world.resource::<CaptureBridge>().finish(
            &ids,
            &info,
            Err(DesktopCaptureError::UnsupportedFormat),
        );
    }
    let readback = if ids.is_empty() || !supported {
        None
    } else {
        let stride = (size[0] * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
            * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Desktop capture pixels"),
            size: u64::from(stride) * u64::from(size[1]),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            target.0.offscreen.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(stride),
                    rows_per_image: Some(size[1]),
                },
            },
            target.0.offscreen.size(),
        );
        Some(Readback {
            buffer,
            ids,
            info,
            stride: stride as usize,
            format,
        })
    };
    if let Some(output) = output {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Desktop final presentation"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: output,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&target.0.blit);
        pass.set_bind_group(0, &target.0.binding, &[]);
        pass.draw(0..3, 0..1);
    }
    if let Some(readback) = readback {
        world.resource_mut::<Readbacks>().0.push(readback);
    }
    world
        .resource_mut::<PendingCommandBuffers>()
        .push([encoder.finish()]);
    Ok(())
}

// Cleanup runs after Bevy's render_system submitted the copy commands. Starting
// map_async earlier could map the staging buffer before that submission.
fn start_readbacks(world: &mut World) {
    let bridge = world.resource::<CaptureBridge>().clone();
    for readback in std::mem::take(&mut world.resource_mut::<Readbacks>().0) {
        let bridge = bridge.clone();
        let buffer = readback.buffer.clone();
        readback
            .buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let pixels = result
                    .map_err(|error| DesktopCaptureError::Gpu(error.to_string()))
                    .and_then(|()| {
                        let mapped = buffer
                            .slice(..)
                            .get_mapped_range()
                            .map_err(|error| DesktopCaptureError::Gpu(error.to_string()))?;
                        unpack_pixels(
                            &mapped,
                            readback.info.physical_size,
                            readback.stride,
                            readback.format,
                        )
                    });
                buffer.unmap();
                bridge.finish(&readback.ids, &readback.info, pixels);
            });
    }
}

fn compose_default(
    world: &mut World,
    imgui: &mut WgpuSnapshotRenderer,
    snapshot: FrameSnapshot,
    output: &wgpu::TextureView,
    format: wgpu::TextureFormat,
    size: [u32; 2],
) -> Result<(), String> {
    let device = world.resource::<RenderDevice>().wgpu_device().clone();
    if !world
        .get_resource::<DesktopRenderer>()
        .is_some_and(|renderer| renderer.size == size && renderer.output_format == format)
    {
        world.insert_resource(DesktopRenderer::new(&device, size, format));
    }
    let renderer = world.resource::<DesktopRenderer>();
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("Bed desktop composition"),
    });
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Bed desktop ImGui"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &renderer.view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        imgui
            .render_snapshot(
                snapshot,
                &mut pass,
                FramebufferExtent::from_texture(&renderer.offscreen),
            )
            .map_err(|error| error.to_string())?;
    }
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Bed desktop output"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: output,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&renderer.blit);
        pass.set_bind_group(0, &renderer.binding, &[]);
        pass.draw(0..3, 0..1);
    }
    world
        .resource_mut::<PendingCommandBuffers>()
        .push([encoder.finish()]);
    Ok(())
}

impl DesktopRenderer {
    fn new(device: &wgpu::Device, size: [u32; 2], format: wgpu::TextureFormat) -> Self {
        Self::with_source(
            device,
            size,
            format,
            wgpu::TextureFormat::Rgba8Unorm,
            format.is_srgb(),
        )
    }
    fn with_source(
        device: &wgpu::Device,
        size: [u32; 2],
        format: wgpu::TextureFormat,
        source: wgpu::TextureFormat,
        decode: bool,
    ) -> Self {
        let offscreen = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Bed desktop UI"),
            size: wgpu::Extent3d {
                width: size[0].max(1),
                height: size[1].max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: source,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = offscreen.create_view(&Default::default());
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Bed desktop blit"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let binding = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Bed desktop blit"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            }],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Bed desktop blit"),
            source: wgpu::ShaderSource::Wgsl(
                format!("{BLIT}\nconst OUTPUT_SRGB: bool = {decode};").into(),
            ),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Bed desktop blit"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let blit = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Bed desktop blit"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vertex"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fragment"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        Self {
            offscreen,
            view,
            blit,
            binding,
            size,
            output_format: format,
        }
    }
}

const BLIT: &str = "
@group(0) @binding(0) var image: texture_2d<f32>;
@vertex fn vertex(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let points = array<vec2<f32>,3>(vec2<f32>(-1.,-1.),vec2<f32>(3.,-1.),vec2<f32>(-1.,3.));
    return vec4<f32>(points[index],0.,1.);
}
@fragment fn fragment(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let color = textureLoad(image,vec2<i32>(position.xy),0);
    if OUTPUT_SRGB {
        let low = color.rgb / 12.92;
        let high = pow((color.rgb + vec3<f32>(0.055)) / 1.055,vec3<f32>(2.4));
        return vec4<f32>(select(high,low,color.rgb <= vec3<f32>(0.04045)),color.a);
    }
    return color;
}";

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    use dear_imgui_rs::{Context, FramePrepareOptions};

    #[test]
    fn deferred_old_device_frame_is_discarded_before_camera_submission() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = Context::create();
        context
            .set_ini_filename(None::<std::path::PathBuf>)
            .unwrap();
        context.prepare_frame(
            FramePrepareOptions::new([320.0, 240.0], 1.0 / 60.0).renderer_has_textures(),
        );
        let consumer = context.create_detached_renderer_consumer().unwrap();
        let token = context.begin_frame();
        token.ui().text("old renderer frame");
        let snapshot = token.render_snapshot(&consumer).unwrap();
        assert!(context.prepare_renderer_texture_reset(&consumer).is_err());

        let mut world = World::new();
        let bridge = crate::DeviceBridge::default();
        bridge.0.lock().unwrap().0 = 2;
        world.insert_resource(bridge);
        world.init_resource::<PendingCommandBuffers>();
        world.insert_resource(RenderFrame {
            snapshot: Some(snapshot),
            info: DesktopFrameInfo {
                generation: 1,
                ..default()
            },
            ..default()
        });
        world.run_system_once(enqueue_canvases).unwrap();
        assert!(world.resource::<RenderFrame>().snapshot.is_none());
        context.poll_snapshot_completions().unwrap();
        context
            .prepare_renderer_texture_reset(&consumer)
            .unwrap()
            .commit();

        // A matching replacement frame remains available to the camera. Its
        // font upload is still requested after the discarded frame's feedback.
        context.prepare_frame(
            FramePrepareOptions::new([320.0, 240.0], 1.0 / 60.0).renderer_has_textures(),
        );
        let token = context.begin_frame();
        token.ui().text("replacement renderer frame");
        let snapshot = token.render_snapshot(&consumer).unwrap();
        assert!(!snapshot.texture_requests().is_empty());
        {
            let mut frame = world.resource_mut::<RenderFrame>();
            frame.info.generation = 2;
            frame.snapshot = Some(snapshot);
        }
        world.run_system_once(enqueue_canvases).unwrap();
        assert!(world.resource::<RenderFrame>().snapshot.is_some());
        world.resource_mut::<RenderFrame>().snapshot.take();
        context.poll_snapshot_completions().unwrap();
        context
            .prepare_renderer_texture_reset(&consumer)
            .unwrap()
            .commit();
    }
}
