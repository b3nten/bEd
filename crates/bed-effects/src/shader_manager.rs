//! Physical-resolution ImGui rendering with optional, per-viewport CRT processing.
//! Formula origins are nealmick/ned; the sharp preset and temporal ordering are Bed behavior.
use crate::{
    shader::Shader,
    shader_types::{AccumulationBuffers, FramebufferState, OFFSCREEN_FORMAT, ShaderQuad},
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShaderSettings {
    pub enabled: bool,
    pub scanline_intensity: f32,
    pub vignet_intensity: f32,
    pub bloom_intensity: f32,
    pub static_intensity: f32,
    pub colorshift_intensity: f32,
    pub jitter_intensity: f32,
    pub curvature_intensity: f32,
    pub pixelation_intensity: f32,
    pub pixel_width: f32,
    pub burnin_intensity: f32,
    pub pulse_intensity: f32,
}
impl Default for ShaderSettings {
    fn default() -> Self {
        Self::subtle()
    }
}
impl ShaderSettings {
    pub fn subtle() -> Self {
        Self {
            enabled: true,
            scanline_intensity: 0.06,
            vignet_intensity: 0.04,
            bloom_intensity: 0.025,
            static_intensity: 0.0,
            colorshift_intensity: 0.0,
            jitter_intensity: 0.0,
            curvature_intensity: 0.0,
            pixelation_intensity: 0.0,
            pixel_width: 5000.0,
            burnin_intensity: 0.0,
            pulse_intensity: 0.0,
        }
    }
    pub fn legacy() -> Self {
        Self {
            scanline_intensity: 0.41,
            vignet_intensity: 0.3,
            bloom_intensity: 0.77,
            static_intensity: 0.5,
            jitter_intensity: 2.72,
            curvature_intensity: 0.19,
            pixelation_intensity: -0.312,
            burnin_intensity: 0.9,
            pulse_intensity: 0.05,
            ..Self::subtle()
        }
    }
    pub fn from_json(settings: &serde_json::Value) -> Self {
        let base = Self::subtle();
        let number = |key: &str, default| {
            settings
                .get(key)
                .and_then(serde_json::Value::as_f64)
                .map(|n| n as f32)
                .filter(|n| n.is_finite())
                .unwrap_or(default)
        };
        Self {
            enabled: settings
                .get("shader_toggle")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(base.enabled),
            scanline_intensity: number("scanline_intensity", base.scanline_intensity),
            vignet_intensity: number("vignet_intensity", base.vignet_intensity),
            bloom_intensity: number("bloom_intensity", base.bloom_intensity),
            static_intensity: number("static_intensity", base.static_intensity),
            colorshift_intensity: number("colorshift_intensity", base.colorshift_intensity),
            jitter_intensity: number("jitter_intensity", base.jitter_intensity),
            curvature_intensity: number("curvature_intensity", base.curvature_intensity),
            pixelation_intensity: number("pixelation_intensity", base.pixelation_intensity),
            pixel_width: number("pixel_width", base.pixel_width),
            burnin_intensity: number("burnin_intensity", base.burnin_intensity).clamp(0.0, 1.0),
            pulse_intensity: number("pulse_intensity", base.pulse_intensity),
        }
    }
    pub fn uniform_bytes(&self, width: u32, height: u32, time: f32, output_srgb: bool) -> Vec<u8> {
        [
            width as f32,
            height as f32,
            time,
            f32::from(self.enabled),
            self.scanline_intensity,
            self.vignet_intensity,
            self.bloom_intensity,
            self.static_intensity,
            self.colorshift_intensity,
            self.jitter_intensity,
            self.curvature_intensity,
            self.pixelation_intensity,
            self.pixel_width,
            self.burnin_intensity,
            f32::from(output_srgb),
            self.pulse_intensity,
        ]
        .into_iter()
        .flat_map(f32::to_ne_bytes)
        .collect()
    }
}
struct TemporalBuffers {
    accum: AccumulationBuffers,
    burn_inputs: [wgpu::BindGroup; 2],
    crt_inputs: [wgpu::BindGroup; 2],
}
pub struct ShaderManager {
    pub fb: FramebufferState,
    device: wgpu::Device,
    temporal: Option<TemporalBuffers>,
    uniforms: wgpu::Buffer,
    burn_in: Shader,
    crt: Shader,
    crt_input: wgpu::BindGroup,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    output_srgb: bool,
    last_settings: Option<ShaderSettings>,
    frame_timestamps: Option<(wgpu::QuerySet, u32)>,
}
impl ShaderManager {
    pub fn new(
        device: &wgpu::Device,
        width: u32,
        height: u32,
        output_format: wgpu::TextureFormat,
    ) -> Self {
        let texture = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Bed effect bindings"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: std::num::NonZeroU64::new(64),
                    },
                    count: None,
                },
                texture(1),
                texture(2),
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Bed pixel-center linear clamp"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Bed effect uniforms"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let fb = FramebufferState::new(device, "Bed ImGui offscreen", width.max(1), height.max(1));
        let crt_input = Self::bind_group(device, &layout, &uniforms, &sampler, &fb.view, &fb.view);
        Self {
            device: device.clone(),
            fb,
            temporal: None,
            burn_in: Shader::new(
                device,
                "Bed burn-in",
                include_str!("burn_in.wgsl"),
                &layout,
                OFFSCREEN_FORMAT,
            ),
            crt: Shader::new(
                device,
                "Bed CRT",
                include_str!("fragment.wgsl"),
                &layout,
                output_format,
            ),
            uniforms,
            crt_input,
            layout,
            sampler,
            output_srgb: output_format.is_srgb(),
            last_settings: None,
            frame_timestamps: None,
        }
    }
    fn bind_group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        uniforms: &wgpu::Buffer,
        sampler: &wgpu::Sampler,
        current: &wgpu::TextureView,
        previous: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Bed effect textures"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniforms.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(current),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(previous),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        })
    }
    /// Discard persistence after a scene change; unrelated native viewports retain their own state.
    pub fn invalidate_history(&mut self) {
        self.temporal = None;
    }
    pub fn initialize_framebuffers(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        if width == 0
            || height == 0
            || (width == self.fb.last_display_w && height == self.fb.last_display_h)
        {
            return;
        }
        self.fb = FramebufferState::new(device, "Bed ImGui offscreen", width, height);
        self.crt_input = Self::bind_group(
            device,
            &self.layout,
            &self.uniforms,
            &self.sampler,
            &self.fb.view,
            &self.fb.view,
        );
        self.invalidate_history();
    }
    fn ensure_temporal(&mut self) {
        if self.temporal.is_some() {
            return;
        }
        let accum =
            AccumulationBuffers::new(&self.device, self.fb.last_display_w, self.fb.last_display_h);
        let bind = |current: &wgpu::TextureView, previous: &wgpu::TextureView| {
            Self::bind_group(
                &self.device,
                &self.layout,
                &self.uniforms,
                &self.sampler,
                current,
                previous,
            )
        };
        let burn_inputs = [
            bind(&self.fb.view, &accum.accum[0].view),
            bind(&self.fb.view, &accum.accum[1].view),
        ];
        let crt_inputs = [
            bind(&accum.accum[0].view, &self.fb.view),
            bind(&accum.accum[1].view, &self.fb.view),
        ];
        self.temporal = Some(TemporalBuffers {
            accum,
            burn_inputs,
            crt_inputs,
        });
    }
    /// Time the next postprocessing frame with two consecutive timestamp queries.
    /// The device must have `TIMESTAMP_QUERY` enabled. Both the optional burn-in
    /// pass and CRT presentation are included; subsequent frames are untimed.
    pub fn timestamp_next_frame(&mut self, queries: &wgpu::QuerySet, first_query: u32) {
        self.frame_timestamps = Some((queries.clone(), first_query));
    }
    pub fn render_with_effects(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        queue: &wgpu::Queue,
        output: &wgpu::TextureView,
        time: f32,
        settings: &ShaderSettings,
    ) {
        let timestamps = self.frame_timestamps.take();
        let writes = |begin: bool, end: bool| {
            timestamps
                .as_ref()
                .map(|(query_set, first)| wgpu::RenderPassTimestampWrites {
                    query_set,
                    beginning_of_pass_write_index: begin.then_some(*first),
                    end_of_pass_write_index: end.then_some(*first + 1),
                })
        };
        if self.last_settings != Some(*settings) {
            self.invalidate_history();
            self.last_settings = Some(*settings);
        }
        queue.write_buffer(
            &self.uniforms,
            0,
            &settings.uniform_bytes(
                self.fb.last_display_w,
                self.fb.last_display_h,
                time,
                self.output_srgb,
            ),
        );
        if settings.enabled && settings.burnin_intensity > 0.0 {
            self.ensure_temporal();
            let temporal = self.temporal.as_mut().expect("temporal state initialized");
            let previous = usize::from(temporal.accum.swap);
            let current = 1 - previous;
            Self::render_pass(
                encoder,
                &temporal.accum.accum[current].view,
                &self.burn_in,
                &temporal.burn_inputs[previous],
                "Bed burn-in pass",
                writes(true, false),
            );
            temporal.accum.swap = !temporal.accum.swap;
            Self::render_pass(
                encoder,
                output,
                &self.crt,
                &temporal.crt_inputs[current],
                "Bed CRT pass",
                writes(false, true),
            );
        } else {
            Self::render_pass(
                encoder,
                output,
                &self.crt,
                &self.crt_input,
                "Bed sharp presentation",
                writes(true, true),
            );
        }
    }
    fn render_pass(
        encoder: &mut wgpu::CommandEncoder,
        output: &wgpu::TextureView,
        shader: &Shader,
        input: &wgpu::BindGroup,
        label: &str,
        timestamp_writes: Option<wgpu::RenderPassTimestampWrites<'_>>,
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(label),
            timestamp_writes,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: output,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&shader.pipeline);
        pass.set_bind_group(0, input, &[]);
        ShaderQuad::draw(&mut pass);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn block_on<T>(future: impl std::future::Future<Output = T>) -> T {
        struct ThreadWake(std::thread::Thread);
        impl std::task::Wake for ThreadWake {
            fn wake(self: std::sync::Arc<Self>) {
                self.0.unpark();
            }
        }
        let waker = std::task::Waker::from(std::sync::Arc::new(ThreadWake(std::thread::current())));
        let mut context = std::task::Context::from_waker(&waker);
        let mut future = std::pin::pin!(future);
        loop {
            match future.as_mut().poll(&mut context) {
                std::task::Poll::Ready(value) => return value,
                std::task::Poll::Pending => std::thread::park(),
            }
        }
    }
    use std::time::Duration;

    #[test]
    fn sharp_preset_has_no_distortion_noise_or_temporal_ghosts() {
        let settings = ShaderSettings::from_json(&serde_json::json!({}));
        assert_eq!(settings, ShaderSettings::subtle());
        assert!(settings.enabled);
        assert_eq!(settings.bloom_intensity, 0.025);
        assert_eq!(settings.scanline_intensity, 0.06);
        assert_eq!(settings.vignet_intensity, 0.04);
        assert_eq!(
            [
                settings.colorshift_intensity,
                settings.jitter_intensity,
                settings.curvature_intensity,
                settings.static_intensity,
                settings.burnin_intensity,
                settings.pulse_intensity
            ],
            [0.0; 6]
        );
        let custom = ShaderSettings::from_json(&serde_json::json!({
            "bloom_intensity": 0.2, "pulse_intensity": 0.01,
            "burnin_intensity": 5.0
        }));
        assert_eq!(custom.bloom_intensity, 0.2);
        assert_eq!(custom.pulse_intensity, 0.01);
        assert_eq!(custom.burnin_intensity, 1.0);
    }

    fn neutral(enabled: bool) -> ShaderSettings {
        ShaderSettings {
            enabled,
            scanline_intensity: 0.0,
            vignet_intensity: 0.0,
            bloom_intensity: 0.0,
            static_intensity: 0.0,
            colorshift_intensity: 0.0,
            jitter_intensity: 0.0,
            curvature_intensity: 0.0,
            pixelation_intensity: 0.0,
            pixel_width: 1.0,
            burnin_intensity: 0.0,
            pulse_intensity: 0.0,
        }
    }

    #[test]
    fn settings_uniforms_keep_original_names_and_wgsl_layout() {
        let settings = ShaderSettings::from_json(&serde_json::json!({
            "shader_toggle": false, "scanline_intensity": 0.2, "vignet_intensity": 0.4,
            "bloom_intensity": 0.6, "static_intensity": 0.8, "colorshift_intensity": 1.2,
            "jitter_intensity": 1.4, "curvature_intensity": 1.6, "pixelation_intensity": -0.3,
            "pixel_width": 250.0, "burnin_intensity": 0.97
        }));
        let bytes = settings.uniform_bytes(840, 600, 2.5, true);
        let floats: Vec<f32> = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|bytes| f32::from_ne_bytes(*bytes))
            .collect();
        assert_eq!(
            floats,
            [
                840.0, 600.0, 2.5, 0.0, 0.2, 0.4, 0.6, 0.8, 1.2, 1.4, 1.6, -0.3, 250.0, 0.97, 1.0,
                0.0
            ]
        );
    }

    fn upload(queue: &wgpu::Queue, texture: &wgpu::Texture, pixels: &[u8]) {
        let extent = texture.size();
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(extent.width * 4),
                rows_per_image: Some(extent.height),
            },
            extent,
        );
    }

    fn render_readback(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        manager: &mut ShaderManager,
        output: &wgpu::Texture,
        settings: &ShaderSettings,
        time: f32,
    ) -> Vec<u8> {
        let extent = output.size();
        let stride = (extent.width * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
            * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Shader fixture readback"),
            size: u64::from(stride) * u64::from(extent.height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        manager.render_with_effects(
            &mut encoder,
            queue,
            &output.create_view(&Default::default()),
            time,
            settings,
        );
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: output,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(stride),
                    rows_per_image: Some(extent.height),
                },
            },
            extent,
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
        let data = buffer.slice(..).get_mapped_range();
        let pixels = data
            .chunks_exact(stride as usize)
            .flat_map(|row| row[..extent.width as usize * 4].iter().copied())
            .collect();
        drop(data);
        buffer.unmap();
        pixels
    }

    fn assert_pixels_close(actual: &[u8], expected: &[u8], tolerance: i16) {
        assert_eq!(actual.len(), expected.len());
        for (index, (&actual, &expected)) in actual.iter().zip(expected).enumerate() {
            assert!(
                (i16::from(actual) - i16::from(expected)).abs() <= tolerance,
                "channel {index}: got {actual}, expected {expected} ± {tolerance}"
            );
        }
    }

    #[test]
    #[ignore = "requires a native GPU adapter; run with --ignored on desktop CI"]
    fn native_shader_optimized_matches_original() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter =
            block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).unwrap();
        let (device, queue) =
            block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap();
        let reference_source = format!(
            "{}\n{}",
            include_str!("fragment.wgsl")
                .replace("fn sample_bloom(", "fn unused_optimized_bloom(")
                .replace("fn apply_curvature(", "fn unused_optimized_curvature("),
            include_str!("fixtures/original_bloom_curvature.wgsl"),
        );
        for (width, height) in [(63, 31), (128, 64)] {
            for format in [OFFSCREEN_FORMAT, OFFSCREEN_FORMAT.add_srgb_suffix()] {
                let mut optimized = ShaderManager::new(&device, width, height, format);
                let mut reference = ShaderManager::new(&device, width, height, format);
                reference.crt = Shader::new(
                    &device,
                    "Original CRT reference",
                    &reference_source,
                    &reference.layout,
                    format,
                );
                let output = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("Optimized shader comparison"),
                    size: optimized.fb.texture.size(),
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                    view_formats: &[],
                });
                // Irregular, high-contrast input catches kernel weights, sampling
                // offsets, clamped edges and fractional curvature coordinates.
                let pattern: Vec<u8> = (0..height)
                    .flat_map(|y| {
                        (0..width).flat_map(move |x| {
                            [
                                ((x * 71 + y * 37) % 256) as u8,
                                ((x * 3 + y * 131) % 256) as u8,
                                if (x + y) % 3 == 0 { 255 } else { 0 },
                                255,
                            ]
                        })
                    })
                    .collect();
                upload(&queue, &optimized.fb.texture, &pattern);
                upload(&queue, &reference.fb.texture, &pattern);
                let sharp = ShaderSettings::subtle();
                for settings in [
                    ShaderSettings {
                        enabled: false,
                        ..sharp
                    },
                    sharp,
                    ShaderSettings::legacy(),
                    ShaderSettings {
                        bloom_intensity: 1.0,
                        ..neutral(true)
                    },
                    ShaderSettings {
                        curvature_intensity: 0.5,
                        ..neutral(true)
                    },
                    ShaderSettings {
                        curvature_intensity: -0.5,
                        ..sharp
                    },
                ] {
                    for time in [0.0, 2.0, 4.95] {
                        let actual = render_readback(
                            &device,
                            &queue,
                            &mut optimized,
                            &output,
                            &settings,
                            time,
                        );
                        let expected = render_readback(
                            &device,
                            &queue,
                            &mut reference,
                            &output,
                            &settings,
                            time,
                        );
                        assert_pixels_close(&actual, &expected, 2);
                    }
                }
            }
        }
    }

    /// Run explicitly on a native graphics host. This validates compiled WGSL,
    /// attachment orientation, transfer conversion and temporal texture ordering.
    #[test]
    #[ignore = "requires a native GPU adapter; run with --ignored on desktop CI"]
    fn native_shader_fixtures() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter =
            block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).unwrap();
        let (device, queue) =
            block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap();
        eprintln!(
            "Shader fixtures running on {:?}",
            adapter.get_info().backend
        );
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut manager = ShaderManager::new(&device, 64, 32, OFFSCREEN_FORMAT);
        assert!(block_on(scope.pop()).is_none());
        let output = FramebufferState::new(&device, "Shader fixture output", 64, 32);
        let pattern: Vec<u8> = (0..32)
            .flat_map(|y| {
                (0..64).flat_map(move |x| {
                    [
                        ((x * 3 + y) % 256) as u8,
                        ((y * 5 + x) % 256) as u8,
                        (x + y * 2) as u8,
                        ((x + 2 * y) % 256) as u8,
                    ]
                })
            })
            .collect();
        upload(&queue, &manager.fb.texture, &pattern);
        let pixels = render_readback(
            &device,
            &queue,
            &mut manager,
            &output.texture,
            &neutral(false),
            0.0,
        );
        assert_pixels_close(&pixels, &pattern, 1);
        assert!(
            manager.temporal.as_ref().is_none_or(|t| !t.accum.swap),
            "disabled effects must not advance temporal history"
        );

        // With no distortion, sampling preserves every physical texel.
        // Enabled output forces alpha as in the original fragment shader.
        let mut opaque = pattern.clone();
        for pixel in opaque.as_chunks_mut::<4>().0 {
            pixel[3] = 255;
        }
        let pixels = render_readback(
            &device,
            &queue,
            &mut manager,
            &output.texture,
            &neutral(true),
            0.0,
        );
        assert_pixels_close(&pixels, &opaque, 1);
        assert!(manager.temporal.is_none());
        let pixels = render_readback(
            &device,
            &queue,
            &mut manager,
            &output.texture,
            &neutral(true),
            0.0,
        );
        assert_pixels_close(&pixels, &opaque, 1);
        assert!(manager.temporal.as_ref().is_none_or(|t| !t.accum.swap));

        // The two accumulation textures alternate and use pow(decay,4), max
        // against the new frame, and normalized 8-bit storage before CRT.
        let bright: Vec<u8> = [200, 100, 50, 255].repeat(64 * 32);
        upload(&queue, &manager.fb.texture, &bright);
        let mut settings = neutral(true);
        settings.burnin_intensity = 0.8;
        let pixels = render_readback(
            &device,
            &queue,
            &mut manager,
            &output.texture,
            &settings,
            0.0,
        );
        assert_pixels_close(&pixels, &bright, 1);
        assert!(manager.temporal.as_ref().is_some_and(|t| t.accum.swap));
        let black = [0, 0, 0, 255].repeat(64 * 32);
        upload(&queue, &manager.fb.texture, &black);
        let pixels = render_readback(
            &device,
            &queue,
            &mut manager,
            &output.texture,
            &settings,
            0.0,
        );
        assert_pixels_close(&pixels, &[82, 41, 20, 255].repeat(64 * 32), 1);
        assert!(manager.temporal.as_ref().is_none_or(|t| !t.accum.swap));
        let pixels = render_readback(
            &device,
            &queue,
            &mut manager,
            &output.texture,
            &settings,
            0.0,
        );
        assert_pixels_close(&pixels, &[34, 17, 8, 255].repeat(64 * 32), 1);
        assert!(manager.temporal.as_ref().is_some_and(|t| t.accum.swap));
        let pixels = render_readback(
            &device,
            &queue,
            &mut manager,
            &output.texture,
            &settings,
            0.0,
        );
        assert_pixels_close(&pixels, &[14, 7, 3, 255].repeat(64 * 32), 1);
        assert!(manager.temporal.as_ref().is_none_or(|t| !t.accum.swap));

        // Persistence changes apply immediately without reviving old glyphs.
        settings.burnin_intensity = 0.0;
        let pixels = render_readback(
            &device,
            &queue,
            &mut manager,
            &output.texture,
            &settings,
            0.0,
        );
        assert_pixels_close(&pixels, &black, 0);
        assert!(manager.temporal.is_none());
        settings.burnin_intensity = 0.8;
        let pixels = render_readback(
            &device,
            &queue,
            &mut manager,
            &output.texture,
            &settings,
            0.0,
        );
        assert_pixels_close(&pixels, &black, 0);

        // Native viewports each own persistence; input and history cannot leak.
        let mut other = ShaderManager::new(&device, 64, 32, OFFSCREEN_FORMAT);
        upload(&queue, &other.fb.texture, &bright);
        let pixels = render_readback(&device, &queue, &mut other, &output.texture, &settings, 0.0);
        assert_pixels_close(&pixels, &bright, 1);
        let pixels = render_readback(
            &device,
            &queue,
            &mut manager,
            &output.texture,
            &settings,
            0.0,
        );
        assert_pixels_close(&pixels, &black, 0);

        // Resolution changes discard temporal history; zero-sized resize is safe.
        manager.initialize_framebuffers(&device, 0, 0);
        assert_eq!(
            (manager.fb.last_display_w, manager.fb.last_display_h),
            (64, 32)
        );
        manager.initialize_framebuffers(&device, 32, 16);
        assert!(manager.temporal.as_ref().is_none_or(|t| !t.accum.swap));
        let small = FramebufferState::new(&device, "Resized shader fixture", 32, 16);
        upload(&queue, &manager.fb.texture, &[0, 0, 0, 255].repeat(32 * 16));
        let pixels = render_readback(
            &device,
            &queue,
            &mut manager,
            &small.texture,
            &settings,
            0.0,
        );
        assert_pixels_close(&pixels, &[0, 0, 0, 255].repeat(32 * 16), 0);

        // A uniform input isolates bloom → vignette → scanline → pulse → grid
        // ordering from random/static sampling. Compare the original formulas.
        manager.initialize_framebuffers(&device, 64, 32);
        upload(
            &queue,
            &manager.fb.texture,
            &[50, 75, 100, 255].repeat(64 * 32),
        );
        let _ = render_readback(
            &device,
            &queue,
            &mut manager,
            &output.texture,
            &neutral(true),
            0.0,
        );
        settings = neutral(true);
        settings.bloom_intensity = 0.3;
        settings.vignet_intensity = 0.25;
        settings.scanline_intensity = 0.41;
        settings.pixelation_intensity = -0.2;
        settings.pulse_intensity = 0.05;
        let time = 2.0;
        let pixels = render_readback(
            &device,
            &queue,
            &mut manager,
            &output.texture,
            &settings,
            time,
        );
        let smoothstep = |low: f32, high: f32, value: f32| {
            let t = ((value - low) / (high - low)).clamp(0.0, 1.0);
            t * t * (3.0 - 2.0 * t)
        };
        for (x, y) in [(10, 10), (31, 16), (52, 29)] {
            let uv = [(x as f32 + 0.5) / 64.0, 1.0 - (y as f32 + 0.5) / 32.0];
            let vignette = (uv[0] * (1.0 - uv[1]) * uv[1] * (1.0 - uv[0]) * 15.0).powf(0.25);
            let distance = (uv[1] - 0.6) * 32.0;
            let scanline = if (0.0..130.0).contains(&distance) {
                let fade_out = smoothstep(0.2, 1.0, 1.0 - distance / 130.0).powf(1.2);
                let fade_in = smoothstep(0.0, 1.0, (distance / 7.0).min(1.0));
                1.0 + 0.7 * fade_in * fade_out * 0.41
            } else {
                1.0
            };
            let factor = 1.3 * vignette * scanline * (time * 0.5).sin().mul_add(0.05, 1.0) * 1.2;
            let expected = [
                (50.0 * factor).round() as u8,
                (75.0 * factor).round() as u8,
                (100.0 * factor).round() as u8,
                255,
            ];
            let offset = (y * 64 + x) * 4;
            assert_pixels_close(&pixels[offset..offset + 4], &expected, 2);
        }

        // Preserve display-referred GL colors on an sRGB presentation surface.
        let mut srgb_manager =
            ShaderManager::new(&device, 64, 32, wgpu::TextureFormat::Rgba8UnormSrgb);
        let srgb_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("sRGB fixture output"),
            size: wgpu::Extent3d {
                width: 64,
                height: 32,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        upload(&queue, &srgb_manager.fb.texture, &pattern);
        let pixels = render_readback(
            &device,
            &queue,
            &mut srgb_manager,
            &srgb_texture,
            &neutral(false),
            0.0,
        );
        assert_pixels_close(&pixels, &pattern, 1);
    }
}
