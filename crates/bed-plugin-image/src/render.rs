use super::{CanvasState, DecodedImage};
use bed_plugin::gpu::{GpuContext, OUTPUT_FORMAT, RenderTarget, wgpu};

pub(super) struct ImageGpu {
    pub generation: u64,
    pipeline: wgpu::RenderPipeline,
    bindings: wgpu::BindGroup,
    uniforms: wgpu::Buffer,
    size: [u32; 2],
}
impl ImageGpu {
    pub fn new(gpu: &GpuContext<'_>, decoded: &DecodedImage) -> Result<Self, String> {
        let texture = gpu.upload_rgba(decoded.size, &decoded.rgba, false)?;
        let uniforms = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Image canvas rectangle"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let shader = gpu
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Bed image canvas"),
                source: wgpu::ShaderSource::Wgsl(include_str!("image.wgsl").into()),
            });
        let pipeline = gpu
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Bed image canvas"),
                layout: None,
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: OUTPUT_FORMAT,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            });
        let sampler = gpu.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Bed image sampling"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let bindings = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Bed image bindings"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniforms.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(
                        &texture.create_view(&Default::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
        Ok(Self {
            generation: gpu.generation,
            pipeline,
            bindings,
            uniforms,
            size: decoded.size,
        })
    }
    pub fn render(&self, gpu: &mut GpuContext<'_>, target: &RenderTarget, canvas: CanvasState) {
        let size = [
            self.size[0] as f32 * canvas.scale / canvas.size[0],
            self.size[1] as f32 * canvas.scale / canvas.size[1],
        ];
        let rect = [
            (1.0 - size[0]) * 0.5 + canvas.pan[0] / canvas.size[0],
            (1.0 - size[1]) * 0.5 + canvas.pan[1] / canvas.size[1],
            size[0],
            size[1],
        ];
        let bytes: Vec<_> = rect.into_iter().flat_map(f32::to_ne_bytes).collect();
        gpu.queue.write_buffer(&self.uniforms, 0, &bytes);
        let background = canvas.background.map(f64::from);
        let mut pass = gpu.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Bed image output"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target.view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: background[0],
                        g: background[1],
                        b: background[2],
                        a: background[3],
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bindings, &[]);
        pass.draw(0..6, 0..1);
    }
}
