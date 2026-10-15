//! External texture registrations stay alive until every referencing packet retires.
use crate::render::{Devices, Renderer};
use bevy::{asset::AssetId, prelude::*};
use dear_imgui_wgpu::ExternalTextureId;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

pub(crate) struct ViewportTexture {
    pub image: Handle<Image>,
    pub texture: ExternalTextureId,
    pub placeholder: wgpu::TextureView,
}
struct Entry {
    lease: Arc<ViewportTexture>,
    last_frame: u64,
}

#[derive(Default)]
pub(crate) struct ViewportTextures {
    entries: HashMap<AssetId<Image>, Entry>,
    devices: Option<Devices>,
    renderer: Option<Renderer>,
    placeholder: Option<wgpu::TextureView>,
    frame: u64,
}
impl ViewportTextures {
    pub fn begin_frame(&mut self, devices: &Devices, renderer: &Renderer) {
        if self.devices.as_ref().map(|device| device.generation) != Some(devices.generation) {
            // The old renderer owns its old registrations until its final packet
            // drops. Device/context retirement releases those GPU resources.
            self.entries.clear();
            self.placeholder = None;
        }
        self.frame += 1;
        self.devices = Some(devices.clone());
        self.renderer = Some(renderer.clone());
        let expired: Vec<_> = self
            .entries
            .iter()
            .filter(|(_, entry)| {
                entry.last_frame + 2 < self.frame && Arc::strong_count(&entry.lease) == 1
            })
            .map(|(&image, _)| image)
            .collect();
        if !expired.is_empty() {
            let mut renderer = renderer.lock().unwrap();
            for image in expired {
                let entry = self.entries.remove(&image).unwrap();
                renderer
                    .unregister_external_texture(entry.lease.texture)
                    .expect("Live viewport cache owns its external registration");
            }
        }
    }

    pub fn texture(&mut self, image: &Handle<Image>) -> dear_imgui_rs::TextureId {
        if !self.entries.contains_key(&image.id()) {
            let device = &self
                .devices
                .as_ref()
                .expect("Viewport textures prepared before drawing")
                .device;
            let placeholder = self
                .placeholder
                .get_or_insert_with(|| {
                    device
                        .create_texture(&wgpu::TextureDescriptor {
                            label: Some("Bedtime UI viewport loading placeholder"),
                            size: wgpu::Extent3d {
                                width: 1,
                                height: 1,
                                depth_or_array_layers: 1,
                            },
                            mip_level_count: 1,
                            sample_count: 1,
                            dimension: wgpu::TextureDimension::D2,
                            format: wgpu::TextureFormat::Rgba8Unorm,
                            usage: wgpu::TextureUsages::TEXTURE_BINDING,
                            view_formats: &[],
                        })
                        .create_view(&Default::default())
                })
                .clone();
            let texture = self
                .renderer
                .as_ref()
                .unwrap()
                .lock()
                .unwrap()
                .register_external_texture(&placeholder)
                .expect("Prepared UI renderer is active");
            self.entries.insert(
                image.id(),
                Entry {
                    lease: Arc::new(ViewportTexture {
                        image: image.clone(),
                        texture,
                        placeholder,
                    }),
                    last_frame: self.frame,
                },
            );
        }
        let entry = self.entries.get_mut(&image.id()).unwrap();
        entry.last_frame = self.frame;
        entry.lease.texture.texture_id()
    }

    pub fn frame_textures(&self) -> Vec<Arc<ViewportTexture>> {
        self.entries
            .values()
            .filter(|entry| entry.last_frame == self.frame)
            .map(|entry| entry.lease.clone())
            .collect()
    }
}

/// Current-frame GPU views are resolved in the render world, after scene cameras
/// have rendered. A resized image never samples a stale previous-frame texture.
#[derive(Resource, Default)]
pub(crate) struct PreparedViewports {
    views:
        HashMap<ExternalTextureId, (bevy::render::render_resource::TextureId, wgpu::TextureView)>,
}
impl PreparedViewports {
    pub fn bind(
        &mut self,
        renderer: &mut dear_imgui_wgpu::WgpuSnapshotRenderer,
        images: &bevy::render::render_asset::RenderAssets<bevy::render::texture::GpuImage>,
        textures: &[Arc<ViewportTexture>],
    ) -> Result<(), String> {
        let used: HashSet<_> = textures.iter().map(|texture| texture.texture).collect();
        self.views.retain(|id, _| used.contains(id));
        for texture in textures {
            let Some(image) = images.get(texture.image.id()) else {
                if self.views.remove(&texture.texture).is_some() {
                    renderer
                        .update_external_texture(texture.texture, &texture.placeholder)
                        .map_err(|e| e.to_string())?;
                }
                continue;
            };
            if self
                .views
                .get(&texture.texture)
                .is_some_and(|(id, _)| *id == image.texture.id())
            {
                continue;
            }
            let format = image.texture_descriptor.format;
            let raw_format = format.remove_srgb_suffix();
            if !matches!(
                raw_format,
                wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Bgra8Unorm
            ) || (raw_format != format
                && !image.texture_descriptor.view_formats.contains(&raw_format))
                || image.texture_descriptor.sample_count != 1
                || image.texture_descriptor.dimension != wgpu::TextureDimension::D2
                || image.texture_descriptor.size.depth_or_array_layers != 1
                || !image
                    .texture_descriptor
                    .usage
                    .contains(wgpu::TextureUsages::TEXTURE_BINDING)
            {
                return Err("UI viewports require a resolved RGBA/BGRA8 output with an encoded-color view; use viewport_image() after scene post-processing".into());
            }
            let view = image.texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("Bedtime UI encoded scene output"),
                format: Some(raw_format),
                ..Default::default()
            });
            renderer
                .update_external_texture(texture.texture, &view)
                .map_err(|e| e.to_string())?;
            self.views
                .insert(texture.texture, (image.texture.id(), (*view).clone()));
        }
        Ok(())
    }
}
