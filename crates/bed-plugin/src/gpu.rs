//! Optional GPU canvas contract. Plugins own pipelines and source resources;
//! the host owns output targets, submission, and native renderer registration.
pub use wgpu;

use crate::{HostContext, TextureHandle};
use dear_imgui_rs::{InvisibleButtonMouseButtons, InvisibleButtonOptions, Ui};

pub const OUTPUT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
pub const MAX_OUTPUT_BYTES: u64 = 64 * 1024 * 1024;

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
            view_formats: &[],
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
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub encoder: &'a mut wgpu::CommandEncoder,
    /// Changes with the host device. Rebuild all plugin GPU resources on change.
    pub generation: u64,
}
impl GpuContext<'_> {
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
}
