//! GPU embedding adapter over portable GPU resources.
use bed_ui::{
    TextureHandle,
    gpu::{DeviceErrorHandlers, OUTPUT_FORMAT, RenderOutput},
};

pub struct GpuContext<'a> {
    /// Bed's main world. Bevy panels synchronize their entities here before
    /// Bevy advances simulation and rendering through its own schedules.
    /// Raw GPU embedding hosts can leave this absent.
    pub world: Option<&'a mut bevy::ecs::world::World>,
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
