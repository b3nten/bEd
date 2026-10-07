//! Render-target and quad types translated from ned shaders/shader_types.{h,cpp}.

pub const OFFSCREEN_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

pub struct FramebufferState {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub last_display_w: u32,
    pub last_display_h: u32,
}

impl FramebufferState {
    pub fn new(device: &wgpu::Device, label: &str, width: u32, height: u32) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: OFFSCREEN_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            texture,
            view,
            last_display_w: width,
            last_display_h: height,
        }
    }
}

pub struct AccumulationBuffers {
    pub accum: [FramebufferState; 2],
    pub swap: bool,
}

impl AccumulationBuffers {
    pub fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        Self {
            accum: [
                FramebufferState::new(device, "Bed burn-in 0", width, height),
                FramebufferState::new(device, "Bed burn-in 1", width, height),
            ],
            swap: false,
        }
    }
}

pub struct ShaderQuad;
impl ShaderQuad {
    pub fn draw(pass: &mut wgpu::RenderPass<'_>) {
        pass.draw(0..6, 0..1);
    }
}
