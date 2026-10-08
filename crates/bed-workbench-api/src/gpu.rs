//! Optional GPU canvas contract. Plugins own pipelines and source resources;
//! the host owns output targets and native renderer registration. Plugins normally
//! record into the supplied encoder. Embedded renderers may submit to the shared
//! queue during rendering, before the host submits its canvas and UI commands.
//! Such submissions must not depend on commands pending in the host encoder.
pub use wgpu;

use crate::{HostContext, TextureHandle};
use dear_imgui_rs::{InvisibleButtonMouseButtons, InvisibleButtonOptions, Ui};
use std::sync::{Arc, Mutex};

pub const OUTPUT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
pub const MAX_OUTPUT_BYTES: u64 = 64 * 1024 * 1024;

/// Device capabilities shared by Bed and embedded renderers. Adapter limits
/// enable optional rendering effects, including SSAO on supported hardware.
pub fn renderer_device_descriptor(adapter: &wgpu::Adapter) -> wgpu::DeviceDescriptor<'static> {
    // Match Bevy's functionality defaults without enabling experimental features.
    let mut features = adapter.features() - wgpu::Features::all_experimental_mask();
    if adapter.get_info().device_type == wgpu::DeviceType::DiscreteGpu {
        features.remove(wgpu::Features::MAPPABLE_PRIMARY_BUFFERS);
    }
    wgpu::DeviceDescriptor {
        label: Some("Bed GPU"),
        required_features: features,
        required_limits: adapter.limits(),
        ..Default::default()
    }
}

/// Host-owned device callbacks. Embedded renderers can temporarily replace wgpu
/// callbacks during initialization, then restore these and forward captured loss.
#[derive(Clone, Default)]
pub struct DeviceErrorHandlers {
    lost: Arc<Mutex<Option<String>>>,
    uncaptured: Arc<Mutex<Option<String>>>,
}
impl DeviceErrorHandlers {
    pub fn install(&self, device: &wgpu::Device) {
        let handlers = self.clone();
        device.set_device_lost_callback(move |reason, message| {
            if reason != wgpu::DeviceLostReason::Destroyed {
                handlers.report_device_lost(message);
            }
        });
        let errors = Arc::clone(&self.uncaptured);
        device.on_uncaptured_error(Arc::new(move |error: wgpu::Error| {
            eprintln!("bEd: uncaptured GPU error: {error}");
            if let Ok(mut slot) = errors.lock() {
                slot.get_or_insert_with(|| error.to_string());
            }
        }));
    }

    pub fn report_device_lost(&self, message: impl Into<String>) {
        if let Ok(mut slot) = self.lost.lock() {
            slot.get_or_insert_with(|| message.into());
        }
    }

    pub fn take_device_lost(&self) -> Option<String> {
        self.lost.lock().ok()?.take()
    }

    pub fn take_error(&self) -> Option<String> {
        self.uncaptured.lock().ok()?.take()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RenderOutput {
    pub handle: TextureHandle,
    /// Physical pixels, independent of the logical ImGui canvas size.
    pub size: [u32; 2],
    pub depth: bool,
    /// Increment whenever the output's contents need to change.
    pub revision: u64,
}
impl RenderOutput {
    pub fn validate(&self, maximum: u32) -> Result<(), String> {
        let [width, height] = self.size;
        if width == 0
            || height == 0
            || width > maximum
            || height > maximum
            || u64::from(width) * u64::from(height) > MAX_OUTPUT_BYTES / 4
        {
            return Err(
                "Plugin output exceeds the GPU dimensions or 64 MiB color-target limit".into(),
            );
        }
        Ok(())
    }
}

pub struct RenderTarget {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub depth: Option<wgpu::TextureView>,
    pub size: [u32; 2],
}
impl RenderTarget {
    pub fn new(device: &wgpu::Device, output: RenderOutput) -> Result<Self, String> {
        output.validate(device.limits().max_texture_dimension_2d)?;
        let extent = wgpu::Extent3d {
            width: output.size[0],
            height: output.size[1],
            depth_or_array_layers: 1,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Bed plugin output"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: OUTPUT_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            // Renderers can write through an sRGB view while ImGui samples the
            // existing linear-format view, preserving Bed's output contract.
            view_formats: &[OUTPUT_FORMAT.add_srgb_suffix()],
        });
        let view = texture.create_view(&Default::default());
        let depth = output.depth.then(|| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("Bed plugin depth"),
                    size: extent,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: DEPTH_FORMAT,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        });
        Ok(Self {
            texture,
            view,
            depth,
            size: output.size,
        })
    }
}

pub struct GpuContext<'a> {
    /// Shared backend handles for embedded renderers using the host's device.
    pub instance: &'a wgpu::Instance,
    pub adapter: &'a wgpu::Adapter,
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub encoder: &'a mut wgpu::CommandEncoder,
    /// Restore after an embedded renderer installs callbacks on the shared device.
    pub error_handlers: Option<&'a DeviceErrorHandlers>,
    /// Changes with the host device. Rebuild all plugin GPU resources on change.
    pub generation: u64,
}
impl GpuContext<'_> {
    pub fn restore_host_handlers(&self) {
        if let Some(handlers) = self.error_handlers {
            handlers.install(self.device);
        }
    }

    pub fn report_device_lost(&self, message: impl Into<String>) {
        if let Some(handlers) = self.error_handlers {
            handlers.report_device_lost(message);
        }
    }

    pub fn upload_rgba(
        &self,
        size: [u32; 2],
        rgba: &[u8],
        srgb: bool,
    ) -> Result<wgpu::Texture, String> {
        RenderOutput {
            handle: TextureHandle(0),
            size,
            depth: false,
            revision: 0,
        }
        .validate(self.device.limits().max_texture_dimension_2d)?;
        if u64::from(size[0]) * u64::from(size[1]) * 4 != rgba.len() as u64 {
            return Err("Texture dimensions do not match its RGBA pixels".into());
        }
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Bed plugin source image"),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: if srgb {
                wgpu::TextureFormat::Rgba8UnormSrgb
            } else {
                OUTPUT_FORMAT
            },
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            texture.as_image_copy(),
            rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size[0] * 4),
                rows_per_image: Some(size[1]),
            },
            texture.size(),
        );
        Ok(texture)
    }
}

/// Shared presentation/input area. Native windows and ImGui remain host-owned.
pub struct Canvas {
    pub size: [f32; 2],
    pub pixels: [u32; 2],
    pub hovered: bool,
    pub active: bool,
}
impl Canvas {
    pub fn show(ui: &Ui, host: &HostContext<'_>, handle: TextureHandle, id: &str) -> Self {
        let size = ui.content_region_avail().map(|v| v.max(1.0));
        let origin = ui.cursor_screen_pos();
        ui.invisible_button_options(
            id,
            size,
            InvisibleButtonOptions::new().mouse_buttons(
                InvisibleButtonMouseButtons::LEFT
                    | InvisibleButtonMouseButtons::RIGHT
                    | InvisibleButtonMouseButtons::MIDDLE,
            ),
        );
        let hovered = ui.is_item_hovered();
        let active = ui.is_item_active();
        if let Some(texture) = host.texture(handle) {
            let end = [origin[0] + size[0], origin[1] + size[1]];
            let _clip = ui.push_clip_rect(origin, end, true);
            ui.get_window_draw_list()
                .add_image(texture, origin, end, [0.0; 2], [1.0; 2], [1.0; 4]);
        }
        let scale = ui.window_viewport().framebuffer_scale();
        let pixels = canvas_pixels(size, scale);
        Self {
            size,
            pixels,
            hovered,
            active,
        }
    }
}

fn canvas_pixels(size: [f32; 2], scale: [f32; 2]) -> [u32; 2] {
    let mut pixels = [0; 2];
    for axis in 0..2 {
        let scale = if scale[axis].is_finite() && scale[axis] > 0.0 {
            scale[axis]
        } else {
            1.0
        };
        pixels[axis] = (size[axis] * scale).ceil().clamp(1.0, 8192.0) as u32;
    }
    // Keep large/high-DPI canvases within the output budget, preserving aspect.
    let area = u64::from(pixels[0]) * u64::from(pixels[1]);
    if area > MAX_OUTPUT_BYTES / 4 {
        let factor = ((MAX_OUTPUT_BYTES / 4) as f64 / area as f64).sqrt();
        pixels = pixels.map(|v| (f64::from(v) * factor).floor().max(1.0) as u32);
    }
    pixels
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn output_limits_reject_zero_oversized_and_overflowing_dimensions() {
        let output = |size| RenderOutput {
            handle: TextureHandle(1),
            size,
            depth: true,
            revision: 0,
        };
        assert!(output([4096, 4096]).validate(8192).is_ok());
        for size in [[0, 1], [1, 0], [8193, 1], [4097, 4096], [u32::MAX; 2]] {
            assert!(output(size).validate(8192).is_err());
        }
    }
    #[test]
    fn canvas_uses_physical_pixels_and_bounds_large_targets() {
        assert_eq!(canvas_pixels([300.0, 200.0], [2.0; 2]), [600, 400]);
        assert_eq!(canvas_pixels([300.0, 200.0], [f32::NAN, 0.0]), [300, 200]);
        let size = canvas_pixels([8000.0; 2], [2.0; 2]);
        assert_eq!(size, [4096; 2]);
    }

    #[test]
    fn device_loss_reports_are_shared_and_keep_the_first_failure() {
        let host = DeviceErrorHandlers::default();
        let renderer = host.clone();
        renderer.report_device_lost("renderer initialization lost the device");
        host.report_device_lost("later failure");
        assert_eq!(
            host.take_device_lost().as_deref(),
            Some("renderer initialization lost the device")
        );
        assert_eq!(renderer.take_device_lost(), None);
        assert_eq!(host.take_error(), None);
    }

    #[test]
    #[ignore = "requires a native GPU adapter; run with --ignored on desktop CI"]
    fn native_shared_target_supports_srgb_rendering() {
        use std::{future::Future, sync::Arc, task::Wake};

        fn block_on<T>(future: impl Future<Output = T>) -> T {
            struct ThreadWake(std::thread::Thread);
            impl Wake for ThreadWake {
                fn wake(self: Arc<Self>) {
                    self.0.unpark();
                }
            }
            let waker = std::task::Waker::from(Arc::new(ThreadWake(std::thread::current())));
            let mut context = std::task::Context::from_waker(&waker);
            let mut future = std::pin::pin!(future);
            loop {
                match future.as_mut().poll(&mut context) {
                    std::task::Poll::Ready(value) => return value,
                    std::task::Poll::Pending => std::thread::park(),
                }
            }
        }

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter =
            block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).unwrap();
        let descriptor = renderer_device_descriptor(&adapter);
        assert!(adapter.features().contains(descriptor.required_features));
        assert!(
            !descriptor
                .required_features
                .intersects(wgpu::Features::all_experimental_mask())
        );
        let (device, queue) = block_on(adapter.request_device(&descriptor)).unwrap();
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let target = RenderTarget::new(
            &device,
            RenderOutput {
                handle: TextureHandle(1),
                size: [1, 1],
                depth: false,
                revision: 0,
            },
        )
        .unwrap();
        assert_eq!(target.texture.format(), OUTPUT_FORMAT);
        let srgb = target.texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(OUTPUT_FORMAT.add_srgb_suffix()),
            ..Default::default()
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &srgb,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.5,
                            g: 0.5,
                            b: 0.5,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
        }
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Shared renderer target readback"),
            size: 256,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            target.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(256),
                    rows_per_image: Some(1),
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
                timeout: Some(std::time::Duration::from_secs(10)),
            })
            .unwrap();
        receiver
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap()
            .unwrap();
        let pixels = buffer.slice(..).get_mapped_range();
        // A half-intensity linear clear must be sRGB encoded in the shared bytes.
        assert_eq!(&pixels[..4], &[188, 188, 188, 255]);
        drop(pixels);
        buffer.unmap();
        assert!(block_on(validation.pop()).is_none());

        let previous_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let previous = Arc::clone(&previous_calls);
        device.on_uncaptured_error(Arc::new(move |_| {
            previous.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }));
        let handlers = DeviceErrorHandlers::default();
        handlers.install(&device);
        // A host restoration must replace any callbacks installed by a renderer.
        let _invalid = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Host callback restoration fixture"),
            size: 4,
            usage: wgpu::BufferUsages::empty(),
            mapped_at_creation: false,
        });
        assert!(handlers.take_error().is_some());
        assert_eq!(previous_calls.load(std::sync::atomic::Ordering::Relaxed), 0);
        assert!(handlers.take_error().is_none());
    }
}
