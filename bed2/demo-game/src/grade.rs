//! A game-owned fullscreen shader, independent of the editor's UI compositor.
use bevy::{
    core_pipeline::{Core3d, Core3dSystems, tonemapping::tonemapping, upscaling::upscaling},
    prelude::*,
    render::{
        RenderApp, RenderStartup,
        extract_component::{ExtractComponent, ExtractComponentPlugin},
        renderer::{RenderContext, RenderDevice, ViewQuery},
        view::ViewTarget,
    },
};

#[derive(Component, Clone, ExtractComponent)]
#[extract_app(RenderApp)]
pub(crate) struct DemoGrade;

pub(super) fn install(app: &mut App) {
    app.add_plugins(ExtractComponentPlugin::<DemoGrade>::default());
    app.sub_app_mut(RenderApp)
        .add_systems(RenderStartup, prepare)
        .add_systems(
            Core3d,
            grade
                .in_set(Core3dSystems::PostProcess)
                .after(tonemapping)
                .before(upscaling),
        );
}

#[derive(Resource)]
struct GradePipeline {
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
}
fn prepare(mut commands: Commands, device: Res<RenderDevice>) {
    let device = device.wgpu_device();
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("Cube game grade input"),
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
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("Cube game post-processing"),
        source: wgpu::ShaderSource::Wgsl(include_str!("grade.wgsl").into()),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("Cube game grade"),
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("Cube game grade"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vertex"),
            compilation_options: default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fragment"),
            compilation_options: default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::Rgba16Float,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: default(),
        depth_stencil: None,
        multisample: default(),
        multiview_mask: None,
        cache: None,
    });
    commands.insert_resource(GradePipeline { layout, pipeline });
}

fn grade(
    view: ViewQuery<(&ViewTarget, &DemoGrade)>,
    pipeline: Res<GradePipeline>,
    mut ctx: RenderContext,
) {
    let (target, _) = view.into_inner();
    let post = target.post_process_write();
    let binding = ctx
        .render_device()
        .wgpu_device()
        .create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Cube game grade"),
            layout: &pipeline.layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(post.source),
            }],
        });
    let mut pass = ctx
        .command_encoder()
        .begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Cube game post-process"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: post.destination,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..default()
        });
    pass.set_pipeline(&pipeline.pipeline);
    pass.set_bind_group(0, &binding, &[]);
    pass.draw(0..3, 0..1);
}
