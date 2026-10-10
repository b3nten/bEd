//! Geometry snapshots for inspection; shaded glTF assets belong to Bevy.
use glam::Vec3;
use std::borrow::Cow;
#[cfg(test)]
use std::sync::Arc;

pub(super) const MAX_RESOURCE_BYTES: usize = 64 * 1024 * 1024;
pub(super) const MAX_VERTICES: usize = 1_000_000;
pub(super) const MAX_INDICES: usize = 3_000_000;
pub(super) const MAX_TEXTURE_BYTES: usize = 512 * 1024 * 1024;
pub(super) const MAX_NODES: usize = 100_000;

pub(super) enum PreparedModel {
    Gltf(crate::prepare::PreparedGltf),
    Stl(crate::stl::PreparedStl),
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    #[cfg(test)]
    pub uv0: [f32; 2],
    #[cfg(test)]
    pub uv1: [f32; 2],
    #[cfg(test)]
    pub tangent: [f32; 4],
    #[cfg(test)]
    pub color: [f32; 4],
}
#[cfg(test)]
#[derive(Clone)]
pub(super) struct Image {
    pub size: [u32; 2],
    pub rgba: Arc<[u8]>,
}
#[cfg(test)]
#[derive(Clone, Copy, Debug)]
pub(super) struct TextureSampler {
    pub wrap: [gltf::texture::WrappingMode; 2],
    pub mag_filter: Option<gltf::texture::MagFilter>,
    pub min_filter: Option<gltf::texture::MinFilter>,
}
#[cfg(test)]
#[derive(Clone, Copy, Debug)]
pub(super) struct TextureInfo {
    pub image: usize,
    pub tex_coord: u32,
    pub sampler: TextureSampler,
}
#[cfg(test)]
#[derive(Clone)]
pub(super) struct Material {
    pub color: [f32; 4],
    pub texture: Option<TextureInfo>,
    pub metallic: f32,
    pub roughness: f32,
    pub metallic_roughness_texture: Option<TextureInfo>,
    pub normal_texture: Option<TextureInfo>,
    pub normal_scale: f32,
    pub occlusion_texture: Option<TextureInfo>,
    pub occlusion_strength: f32,
    pub emissive: [f32; 3],
    pub emissive_texture: Option<TextureInfo>,
    pub alpha: gltf::material::AlphaMode,
    pub cutoff: f32,
    pub double_sided: bool,
    pub unlit: bool,
}
#[derive(Clone)]
pub(super) struct Primitive {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    #[cfg(test)]
    pub material: Material,
    #[cfg(test)]
    pub has_tangents: bool,
}
#[derive(Clone)]
pub(super) struct Scene {
    pub primitives: Vec<Primitive>,
    #[cfg(test)]
    pub images: Vec<Image>,
    pub textures: usize,
    pub minimum: Vec3,
    pub maximum: Vec3,
    pub nodes: usize,
    pub animations: usize,
    pub draco_primitives: usize,
    pub meshopt_views: usize,
    #[cfg(test)]
    pub source_bytes: Option<Arc<[u8]>>,
    pub posed_meshes: usize,
}
impl Scene {
    pub fn center(&self) -> Vec3 {
        (self.minimum + self.maximum) * 0.5
    }
    pub fn radius(&self) -> f32 {
        ((self.maximum - self.minimum).length() * 0.5).max(0.001)
    }
    pub fn triangles(&self) -> usize {
        self.primitives.iter().map(|p| p.indices.len() / 3).sum()
    }
}

#[cfg(test)]
pub(super) fn load(bytes: &[u8]) -> Result<Scene, String> {
    let prepared = crate::prepare::load(bytes)?;
    let mut scene = crate::bevy_scene::load_prepared_snapshot(&prepared)?;
    scene.nodes = prepared.nodes;
    scene.draco_primitives = prepared.draco_primitives;
    scene.meshopt_views = prepared.meshopt_views;
    scene.source_bytes = Some(prepared.bytes);
    Ok(scene)
}

/// gltf's JSON validation does not validate binary view/accessor spans. Check
/// these before constructing its typed iterators (which assume valid spans).
pub(super) fn validate_accessor(
    accessor: &gltf::Accessor<'_>,
    buffers: &[Cow<'_, [u8]>],
) -> Result<(), String> {
    if accessor.count() == 0 {
        return Err("Mesh accessors must not be empty".into());
    }
    let check_view = |view: gltf::buffer::View<'_>,
                      offset: usize,
                      count: usize,
                      size: usize|
     -> Result<(), String> {
        let data = buffers
            .get(view.buffer().index())
            .ok_or("Missing accessor buffer")?;
        let stride = view.stride().unwrap_or(size);
        let span = count
            .saturating_sub(1)
            .checked_mul(stride)
            .and_then(|v| v.checked_add(if count == 0 { 0 } else { size }))
            .and_then(|v| v.checked_add(offset))
            .ok_or("Accessor byte range overflow")?;
        if stride < size
            || span > view.length()
            || view
                .offset()
                .checked_add(view.length())
                .is_none_or(|end| end > data.len())
        {
            return Err("Accessor extends beyond its buffer view".into());
        }
        Ok(())
    };
    if let Some(view) = accessor.view() {
        check_view(view, accessor.offset(), accessor.count(), accessor.size())?;
    }
    if let Some(sparse) = accessor.sparse() {
        let indices = sparse.indices();
        let values = sparse.values();
        if sparse.count() == 0
            || sparse.count() > accessor.count()
            || indices.view().stride().is_some()
            || values.view().stride().is_some()
        {
            return Err("Invalid sparse accessor count or stride".into());
        }
        check_view(
            indices.view(),
            indices.offset(),
            sparse.count(),
            indices.index_type().size(),
        )?;
        check_view(
            values.view(),
            values.offset(),
            sparse.count(),
            accessor.size(),
        )?;
        // Validate sparse indices themselves before gltf's iterator applies them.
        let view = indices.view();
        let data = &buffers[view.buffer().index()][view.offset() + indices.offset()..];
        let mut previous = None;
        for bytes in data
            .chunks_exact(indices.index_type().size())
            .take(sparse.count())
        {
            let value = match bytes.len() {
                1 => usize::from(bytes[0]),
                2 => usize::from(u16::from_le_bytes(bytes.try_into().unwrap())),
                _ => u32::from_le_bytes(bytes.try_into().unwrap()) as usize,
            };
            if value >= accessor.count() || previous.is_some_and(|prev| value <= prev) {
                return Err(
                    "Sparse accessor indices must be ordered and inside the accessor".into(),
                );
            }
            previous = Some(value);
        }
    }
    Ok(())
}

pub(super) fn triangles(indices: Vec<u32>, mode: gltf::mesh::Mode) -> Result<Vec<u32>, String> {
    use gltf::mesh::Mode;
    match mode {
        Mode::Triangles if indices.len().is_multiple_of(3) => Ok(indices),
        Mode::TriangleStrip | Mode::TriangleFan => {
            if indices.len().saturating_sub(2) * 3 > MAX_INDICES {
                return Err("Triangulated mesh exceeds the index limit".into());
            }
            let mut result = Vec::new();
            for i in 2..indices.len() {
                let triangle = if mode == Mode::TriangleFan {
                    [indices[0], indices[i - 1], indices[i]]
                } else if i % 2 == 0 {
                    [indices[i - 2], indices[i - 1], indices[i]]
                } else {
                    [indices[i - 1], indices[i - 2], indices[i]]
                };
                if triangle[0] != triangle[1]
                    && triangle[1] != triangle[2]
                    && triangle[0] != triangle[2]
                {
                    result.extend(triangle);
                }
            }
            Ok(result)
        }
        Mode::Triangles => Err("Triangle index count is not a multiple of three".into()),
        _ => Err("glTF preview currently supports triangle meshes, strips, and fans".into()),
    }
}
