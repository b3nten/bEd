//! Only owned draw snapshots and GPU handles cross into Bevy's render world.
use bevy::{
    prelude::*,
    render::{
        ExtractSchedule, Render, RenderApp, RenderStartup, RenderSystems,
        renderer::{CurrentView, PendingCommandBuffers, RenderDevice, RenderQueue},
        view::ViewTarget,
    },
};
use dear_imgui_rs::render::FrameSnapshot;
use dear_imgui_wgpu::{FramebufferExtent, WgpuSnapshotRenderer};
use std::sync::{Arc, Mutex};

#[derive(bevy::ecs::schedule::ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct UiGraph;

/// Editor effects can run between UI drawing and final window composition.
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum UiRenderSystems {
    Draw,
    Composite,
}

#[derive(Clone)]
pub(crate) struct Devices {
    pub generation: u64,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

#[derive(Resource, Clone, Default)]
pub(crate) struct DeviceBridge(pub Arc<Mutex<Option<Devices>>>);

pub(crate) type Renderer = Arc<Mutex<WgpuSnapshotRenderer>>;

pub(crate) struct Packet {
    pub snapshot: FrameSnapshot,
    pub renderer: Renderer,
    pub window: Entity,
    pub size: [u32; 2],
    pub generation: u64,
    pub images: Vec<Arc<crate::viewport_textures::ViewportTexture>>,
}

#[derive(Resource, Default)]
pub(crate) struct FrameSlot(pub Option<Packet>);

pub(crate) fn install(app: &mut App) {
    let bridge = DeviceBridge::default();
    app.insert_resource(bridge.clone())
        .init_resource::<FrameSlot>();
    app.sub_app_mut(RenderApp)
        .insert_resource(bridge)
        .init_resource::<FrameSlot>()
        .init_resource::<crate::viewport_textures::PreparedViewports>()
        .init_schedule(UiGraph)
        .configure_sets(
            UiGraph,
            (UiRenderSystems::Draw, UiRenderSystems::Composite).chain(),
        )
        .add_systems(UiGraph, draw_ui.in_set(UiRenderSystems::Draw))
        .add_systems(UiGraph, composite_ui.in_set(UiRenderSystems::Composite))
        .add_systems(RenderStartup, publish_device)
        .add_systems(ExtractSchedule, extract)
        .add_systems(Render, reconcile_unused.in_set(RenderSystems::Cleanup));
}

fn publish_device(world: &mut World) {
    let bridge = world.resource::<DeviceBridge>().clone();
    let mut slot = bridge.0.lock().expect("UI device bridge poisoned");
    let generation = slot.as_ref().map_or(1, |old| old.generation + 1);
    *slot = Some(Devices {
        generation,
        device: world.resource::<RenderDevice>().wgpu_device().clone(),
        queue: world.resource::<RenderQueue>().clone().into_inner(),
    });
    world.remove_resource::<UiTarget>();
    world.insert_resource(crate::viewport_textures::PreparedViewports::default());
}

fn extract(mut commands: Commands, mut main: ResMut<bevy::render::MainWorld>) {
    commands.insert_resource(FrameSlot(main.resource_mut::<FrameSlot>().0.take()));
}

fn reconcile_unused(world: &mut World) {
    if let Some(packet) = world.resource_mut::<FrameSlot>().0.take() {
        let mut renderer = packet.renderer.lock().unwrap();
        let current = world
            .resource::<DeviceBridge>()
            .0
            .lock()
            .unwrap()
            .as_ref()
            .map(|d| d.generation);
        if current == Some(packet.generation) {
            if let Err(error) = renderer.reconcile_snapshot(packet.snapshot) {
                bevy::log::error!("UI skipped-frame reconciliation: {error}");
            }
        }
    }
}

fn draw_ui(world: &mut World) {
    if let Some(mut layer) = world.get_resource_mut::<UiTarget>() {
        layer.ready = false;
    }
    let view = world.resource::<CurrentView>().0;
    let Some(packet) = world.resource::<FrameSlot>().0.as_ref() else {
        return;
    };
    let camera = world
        .get::<bevy::render::camera::ExtractedCamera>(view)
        .expect("UI graph runs for a camera");
    if !matches!(&camera.target, Some(bevy::camera::NormalizedRenderTarget::Window(window)) if window.entity() == packet.window)
    {
        return;
    }
    let Some(target) = world.get::<ViewTarget>(view) else {
        return;
    };
    let Some(format) = target.out_texture_view_format() else {
        return;
    };
    let size = [
        target.main_texture().width(),
        target.main_texture().height(),
    ];
    let packet = world.resource_mut::<FrameSlot>().0.take().unwrap();
    let mut renderer = packet.renderer.lock().unwrap();
    let current = world
        .resource::<DeviceBridge>()
        .0
        .lock()
        .unwrap()
        .as_ref()
        .map(|d| d.generation);
    if current != Some(packet.generation) {
        return;
    }
    if packet.size != size {
        if let Err(error) = renderer.reconcile_snapshot(packet.snapshot) {
            bevy::log::error!("UI resize reconciliation: {error}");
        }
        return;
    }
    let result: Result<(), String> = world.resource_scope(|world, mut prepared: Mut<crate::viewport_textures::PreparedViewports>| {
        prepared.bind(&mut renderer,
            world.resource::<bevy::render::render_asset::RenderAssets<bevy::render::texture::GpuImage>>(),
            &packet.images)?;
        let device = world.resource::<RenderDevice>().wgpu_device().clone();
        if !world.get_resource::<UiTarget>().is_some_and(|layer| layer.size == size && layer.output_format == format) {
            world.insert_resource(UiTarget::new(&device, size, format));
        }
        let layer = world.resource::<UiTarget>();
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Bedtime UI layer"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Bedtime UI ImGui"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &layer.view, depth_slice: None, resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })], ..Default::default()
            });
            renderer.render_snapshot(packet.snapshot, &mut pass, FramebufferExtent::from_texture(&layer.offscreen))
                .map_err(|error| error.to_string())?;
        }
        world.resource_mut::<PendingCommandBuffers>().push([encoder.finish()]);
        world.resource_mut::<UiTarget>().ready = true;
        Ok(())
    });
    if let Err(error) = result {
        bevy::log::error!("UI drawing: {error}");
    }
}

#[derive(Resource)]
struct UiTarget {
    offscreen: wgpu::Texture,
    view: wgpu::TextureView,
    blit: wgpu::RenderPipeline,
    binding: wgpu::BindGroup,
    size: [u32; 2],
    output_format: wgpu::TextureFormat,
    ready: bool,
}

fn composite_ui(world: &mut World) {
    let Some(mut layer) = world.get_resource_mut::<UiTarget>() else {
        return;
    };
    if !std::mem::take(&mut layer.ready) {
        return;
    }
    let view = world.resource::<CurrentView>().0;
    let camera = world
        .get::<bevy::render::camera::ExtractedCamera>(view)
        .unwrap();
    let target = world.get::<ViewTarget>(view).unwrap();
    let clear = match camera.clear_color {
        ClearColorConfig::None => None,
        ClearColorConfig::Custom(color) => Some(color.to_linear()),
        ClearColorConfig::Default => Some(world.resource::<ClearColor>().0.to_linear()),
    };
    let Some(attachment) = target.out_texture_color_attachment(clear) else {
        return;
    };
    let device = world.resource::<RenderDevice>().wgpu_device();
    let layer = world.resource::<UiTarget>();
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("Bedtime UI composition"),
    });
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Bedtime UI output"),
            color_attachments: &[Some(attachment)],
            ..Default::default()
        });
        pass.set_pipeline(&layer.blit);
        pass.set_bind_group(0, &layer.binding, &[]);
        pass.draw(0..3, 0..1);
    }
    world
        .resource_mut::<PendingCommandBuffers>()
        .push([encoder.finish()]);
}

impl UiTarget {
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
            label: Some("Bedtime UI 2 UI"),
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
            label: Some("Bedtime UI 2 blit"),
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
            label: Some("Bedtime UI 2 blit"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            }],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Bedtime UI 2 blit"),
            source: wgpu::ShaderSource::Wgsl(
                format!("{BLIT}\nconst OUTPUT_SRGB: bool = {decode};").into(),
            ),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Bedtime UI 2 blit"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let blit = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Bedtime UI 2 blit"),
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
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
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
            ready: false,
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
    // ImGui's transparent intermediate stores premultiplied display colors.
    // Decode straight RGB, then premultiply in the output's blending space.
    let rgb = color.rgb / max(color.a, 0.000001);
    if OUTPUT_SRGB {
        let low = rgb / 12.92;
        let high = pow((rgb + vec3<f32>(0.055)) / 1.055,vec3<f32>(2.4));
        return vec4<f32>(select(high,low,rgb <= vec3<f32>(0.04045)) * color.a,color.a);
    }
    return color;
}";
