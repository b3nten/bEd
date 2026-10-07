//! Independent CRT state for each backend-owned native viewport.
use crate::{
    shader_manager::{ShaderManager, ShaderSettings},
    shader_types::OFFSCREEN_FORMAT,
};
use dear_imgui_wgpu::viewport_postprocess::{ViewportPostprocessor, ViewportPostprocessorFactory};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};

#[derive(Clone, Copy, Debug)]
pub struct EffectFrame {
    pub settings: ShaderSettings,
    pub time: f32,
    pub scene_generation: u64,
}

/// Shared frame parameters contain values only. Every viewport owns its buffers.
#[derive(Clone, Debug)]
pub struct ViewportEffectsFactory {
    frame: Rc<Cell<EffectFrame>>,
    captures: Rc<RefCell<CaptureState>>,
}
impl Default for ViewportEffectsFactory {
    fn default() -> Self {
        Self {
            captures: Rc::new(RefCell::new(CaptureState::default())),
            frame: Rc::new(Cell::new(EffectFrame {
                settings: ShaderSettings::subtle(),
                time: 0.0,
                scene_generation: 0,
            })),
        }
    }
}
impl ViewportEffectsFactory {
    /// Capture one final GPU output per native viewport for acceptance fixtures.
    pub fn enable_capture(&self) {
        self.captures.borrow_mut().enabled = true;
    }
    /// Call after the viewport route has submitted its secondary command buffers.
    pub fn finish_captures(&self) -> Result<Vec<CapturedFrame>, String> {
        let pending = std::mem::take(&mut self.captures.borrow_mut().pending);
        let mut frames = Vec::new();
        for capture in pending {
            let (sender, receiver) = std::sync::mpsc::channel();
            capture
                .buffer
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |result| {
                    let _ = sender.send(result);
                });
            capture
                .device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: Some(Duration::from_secs(10)),
                })
                .map_err(|error| error.to_string())?;
            receiver
                .recv_timeout(Duration::from_secs(10))
                .map_err(|error| error.to_string())?
                .map_err(|error| error.to_string())?;
            let data = capture
                .buffer
                .slice(..)
                .get_mapped_range()
                .map_err(|error| error.to_string())?;
            let pixels = data.to_vec();
            drop(data);
            capture.buffer.unmap();
            frames.push(CapturedFrame {
                id: capture.id,
                width: capture.width,
                height: capture.height,
                stride: capture.stride,
                format: capture.format,
                pixels,
            });
        }
        Ok(frames)
    }
    pub fn update(&self, settings: ShaderSettings, time: f32, scene_generation: u64) {
        self.frame.set(EffectFrame {
            settings,
            time,
            scene_generation,
        });
    }
}
impl ViewportPostprocessorFactory for ViewportEffectsFactory {
    fn create(
        &self,
        device: &wgpu::Device,
        input_format: wgpu::TextureFormat,
        output_format: wgpu::TextureFormat,
        extent: [u32; 2],
    ) -> Result<Box<dyn ViewportPostprocessor>, String> {
        if input_format != OFFSCREEN_FORMAT {
            return Err("Bed effects require an Rgba8Unorm input target".into());
        }
        let id = {
            let mut captures = self.captures.borrow_mut();
            captures.next_id += 1;
            captures.next_id
        };
        Ok(Box::new(ViewportEffects {
            manager: ShaderManager::new(device, extent[0], extent[1], output_format),
            device: device.clone(),
            captures: Rc::clone(&self.captures),
            id,
            rendered: 0,
            captured: false,
            frame: Rc::clone(&self.frame),
            scene_generation: self.frame.get().scene_generation,
        }))
    }
}
#[derive(Debug, Default)]
struct CaptureState {
    enabled: bool,
    next_id: u64,
    pending: Vec<PendingCapture>,
}
#[derive(Debug)]
struct PendingCapture {
    device: wgpu::Device,
    buffer: wgpu::Buffer,
    id: u64,
    width: u32,
    height: u32,
    stride: u32,
    format: wgpu::TextureFormat,
}
pub struct CapturedFrame {
    pub id: u64,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub format: wgpu::TextureFormat,
    pub pixels: Vec<u8>,
}
struct ViewportEffects {
    manager: ShaderManager,
    device: wgpu::Device,
    captures: Rc<RefCell<CaptureState>>,
    id: u64,
    rendered: u64,
    captured: bool,
    frame: Rc<Cell<EffectFrame>>,
    scene_generation: u64,
}
impl ViewportPostprocessor for ViewportEffects {
    fn input_view(
        &mut self,
        device: &wgpu::Device,
        extent: [u32; 2],
    ) -> Result<wgpu::TextureView, String> {
        self.manager
            .initialize_framebuffers(device, extent[0], extent[1]);
        Ok(self.manager.fb.view.clone())
    }
    fn encode(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        output: &wgpu::TextureView,
    ) -> Result<(), String> {
        let frame = self.frame.get();
        if self.scene_generation != frame.scene_generation {
            self.manager.invalidate_history();
            self.scene_generation = frame.scene_generation;
        }
        self.manager
            .render_with_effects(encoder, queue, output, frame.time, &frame.settings);
        self.rendered += 1;
        if self.rendered >= 2 && !self.captured && self.captures.borrow().enabled {
            let texture = output.texture();
            if !texture.usage().contains(wgpu::TextureUsages::COPY_SRC) {
                return Err("native viewport output does not support GPU capture".into());
            }
            let extent = texture.size();
            let stride = (extent.width * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
                * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
            let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Bed native viewport capture"),
                size: u64::from(stride) * u64::from(extent.height),
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture,
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
            self.captures.borrow_mut().pending.push(PendingCapture {
                device: self.device.clone(),
                buffer,
                id: self.id,
                width: extent.width,
                height: extent.height,
                stride,
                format: texture.format(),
            });
            self.captured = true;
        }
        Ok(())
    }
}
