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
                        // The host samples plugin output as straight alpha.
                        blend: None,
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
        let mut pass = gpu.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Bed image output"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target.view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    // The host area owns its background. Image pixels are
                    // composited over it, including transparent image regions.
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
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

#[cfg(test)]
mod tests {
    use super::*;
    use bed_plugin::{TextureHandle, gpu::RenderOutput};
    use std::{
        future::Future,
        sync::Arc,
        task::{Context, Poll, Wake, Waker},
        time::Duration,
    };

    fn block_on<T>(future: impl Future<Output = T>) -> T {
        struct ThreadWake(std::thread::Thread);
        impl Wake for ThreadWake {
            fn wake(self: Arc<Self>) {
                self.0.unpark();
            }
        }
        let waker = Waker::from(Arc::new(ThreadWake(std::thread::current())));
        let mut context = Context::from_waker(&waker);
        let mut future = std::pin::pin!(future);
        loop {
            match future.as_mut().poll(&mut context) {
                Poll::Ready(value) => return value,
                Poll::Pending => std::thread::park(),
            }
        }
    }

    #[test]
    #[ignore = "requires a native GPU adapter"]
    fn image_output_keeps_empty_canvas_transparent_and_pixels_in_straight_alpha() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = block_on(instance.request_adapter(&Default::default())).unwrap();
        let (device, queue) = block_on(adapter.request_device(&Default::default())).unwrap();
        let decoded = DecodedImage {
            size: [2, 2],
            rgba: Arc::from([
                200, 20, 10, 255, 128, 64, 32, 128, 200, 20, 10, 255, 128, 64, 32, 128,
            ]),
        };
        let target = RenderTarget::new(
            &device,
            RenderOutput {
                handle: TextureHandle(1),
                size: [8, 8],
                depth: false,
                revision: 1,
            },
        )
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
        let renderer = ImageGpu::new(&gpu, &decoded).unwrap();
        renderer.render(
            &mut gpu,
            &target,
            CanvasState {
                size: [8.0, 8.0],
                pixels: [8, 8],
                scale: 2.0,
                pan: [0.0; 2],
            },
        );
        let stride = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Image alpha readback"),
            size: u64::from(stride) * 8,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            target.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(stride),
                    rows_per_image: Some(8),
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
                timeout: Some(Duration::from_secs(10)),
            })
            .unwrap();
        receiver
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .unwrap();
        let mapped = buffer.slice(..).get_mapped_range();
        let pixel = |x: usize, y: usize| {
            let offset = y * stride as usize + x * 4;
            <[u8; 4]>::try_from(&mapped[offset..offset + 4]).unwrap()
        };
        assert_eq!(
            pixel(0, 0),
            [0; 4],
            "canvas margins must show the host background"
        );
        assert_eq!(pixel(2, 2), [200, 20, 10, 255]);
        assert_eq!(
            pixel(5, 2),
            [128, 64, 32, 128],
            "ImGui applies this pixel's alpha once"
        );
        drop(mapped);
        buffer.unmap();
    }
}
