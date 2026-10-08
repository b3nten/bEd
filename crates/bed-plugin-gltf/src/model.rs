use base64::{Engine, engine::general_purpose::STANDARD};
use glam::{Mat3, Mat4, Vec3};
use std::{borrow::Cow, io::Cursor, sync::Arc};

pub(super) const MAX_RESOURCE_BYTES: usize = 64 * 1024 * 1024;
pub(super) const MAX_VERTICES: usize = 1_000_000;
pub(super) const MAX_INDICES: usize = 3_000_000;
const MAX_TEXTURE_BYTES: usize = 128 * 1024 * 1024;
const MAX_NODES: usize = 100_000;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    pub color: [f32; 4],
}
pub(super) struct Image {
    pub size: [u32; 2],
    pub rgba: Arc<[u8]>,
}
pub(super) struct Material {
    pub color: [f32; 4],
    pub texture: Option<usize>,
    pub wrap: [gltf::texture::WrappingMode; 2],
    pub nearest: bool,
    pub alpha: gltf::material::AlphaMode,
    pub cutoff: f32,
    pub double_sided: bool,
    pub unlit: bool,
}
pub(super) struct Primitive {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    pub material: Material,
    pub center: Vec3,
}
pub(super) struct Scene {
    pub primitives: Vec<Primitive>,
    pub images: Vec<Image>,
    pub minimum: Vec3,
    pub maximum: Vec3,
    pub nodes: usize,
    pub animations: usize,
    pub draco_primitives: usize,
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

pub(super) fn load(bytes: &[u8]) -> Result<Scene, String> {
    // Draco accessors can omit their buffer views. Normalize those into ordinary
    // accessors before applying gltf's core validation and typed readers.
    let mut gltf =
        gltf::Gltf::from_slice_without_validation(bytes).map_err(|error| error.to_string())?;
    for extension in gltf.extensions_required() {
        if !["KHR_materials_unlit", crate::draco::EXTENSION].contains(&extension) {
            return Err(format!(
                "Required glTF extension is not supported: {extension}"
            ));
        }
    }
    if gltf.nodes().len() > MAX_NODES {
        return Err("glTF exceeds the 100,000-node limit".into());
    }
    let mut total = 0;
    let mut buffers = Vec::new();
    for buffer in gltf.buffers() {
        let data = match buffer.source() {
            gltf::buffer::Source::Bin => {
                Cow::Borrowed(gltf.blob.as_deref().ok_or("GLB has no binary chunk")?)
            }
            gltf::buffer::Source::Uri(uri) => Cow::Owned(data_uri(uri)?),
        };
        total += data.len();
        if total > MAX_RESOURCE_BYTES {
            return Err("glTF buffers exceed the 64 MiB resource limit".into());
        }
        if data.len() < buffer.length() {
            return Err("glTF buffer is shorter than its declared length".into());
        }
        buffers.push(data);
    }
    let (document, draco_primitives) = crate::draco::expand(gltf.document, &mut buffers)?;
    gltf.document = document;
    let mut images = Vec::new();
    let mut image_bytes = 0;
    for image in gltf.images() {
        let bytes = match image.source() {
            gltf::image::Source::View { view, .. } => {
                let buffer = &buffers[view.buffer().index()];
                let end = view
                    .offset()
                    .checked_add(view.length())
                    .ok_or("Invalid image buffer range")?;
                Cow::Borrowed(
                    buffer
                        .get(view.offset()..end)
                        .ok_or("Image buffer view is out of bounds")?,
                )
            }
            gltf::image::Source::Uri { uri, .. } => Cow::Owned(data_uri(uri)?),
        };
        let image = decode_image(&bytes)?;
        image_bytes += image.rgba.len();
        if image_bytes > MAX_TEXTURE_BYTES {
            return Err("glTF textures exceed the 128 MiB total decoded-pixel limit".into());
        }
        images.push(image);
    }
    let scene = gltf
        .default_scene()
        .or_else(|| gltf.scenes().next())
        .ok_or("glTF has no scene")?;
    let nodes = crate::skin::scene_nodes(scene, gltf.nodes().len())?;
    let mut world = vec![None; gltf.nodes().len()];
    for (node, transform) in &nodes {
        world[node.index()] = Some(*transform);
    }
    let mut primitives = Vec::new();
    let mut vertex_count = 0;
    let mut index_count = 0;
    let mut minimum = Vec3::splat(f32::INFINITY);
    let mut maximum = Vec3::splat(f32::NEG_INFINITY);
    let mut posed_meshes = 0;
    for (node, node_transform) in &nodes {
        let Some(mesh) = node.mesh() else {
            continue;
        };
        let palette = node
            .skin()
            .map(|skin| crate::skin::palette(skin, &world, &buffers))
            .transpose()?;
        if palette.is_some() {
            posed_meshes += 1;
        }
        // Skin palettes already produce world-space geometry; the skinned
        // mesh node's own transform must not be applied a second time.
        let transform = if palette.is_some() {
            Mat4::IDENTITY
        } else {
            *node_transform
        };
        let determinant = if let Some(palette) = &palette {
            // A reflected skeleton reverses the baked winding, just like a
            // reflected ordinary node. A zero-scale individual joint does not
            // imply that the entire skinned mesh is invisible.
            if palette.iter().all(|matrix| matrix.determinant() < 0.0) {
                -1.0
            } else {
                1.0
            }
        } else {
            transform.determinant()
        };
        if !determinant.is_finite() {
            return Err("glTF node transform exceeds the preview's numeric range".into());
        }
        if determinant == 0.0 {
            continue;
        } // Zero-scale nodes are invisible; small nonzero scales remain visible.
        let normals = Mat3::from_mat4(transform).inverse().transpose();
        for primitive in mesh.primitives() {
            if primitive.morph_targets().next().is_some() {
                return Err("Morph targets are not supported by the static glTF preview".into());
            }
            for (semantic, accessor) in primitive.attributes() {
                if accessor.count() > MAX_VERTICES {
                    return Err("glTF vertex attributes exceed the 1,000,000-vertex limit".into());
                }
                use gltf::accessor::{DataType, Dimensions};
                let valid = match semantic {
                    gltf::Semantic::Positions | gltf::Semantic::Normals => {
                        accessor.data_type() == DataType::F32
                            && accessor.dimensions() == Dimensions::Vec3
                    }
                    gltf::Semantic::TexCoords(_) => {
                        matches!(
                            accessor.data_type(),
                            DataType::U8 | DataType::U16 | DataType::F32
                        ) && accessor.dimensions() == Dimensions::Vec2
                    }
                    gltf::Semantic::Colors(_) => {
                        matches!(
                            accessor.data_type(),
                            DataType::U8 | DataType::U16 | DataType::F32
                        ) && matches!(accessor.dimensions(), Dimensions::Vec3 | Dimensions::Vec4)
                    }
                    gltf::Semantic::Joints(set) => {
                        if palette.is_some() && set != 0 {
                            return Err("Static skin preview currently supports four joint influences per vertex".into());
                        }
                        matches!(accessor.data_type(), DataType::U8 | DataType::U16)
                            && accessor.dimensions() == Dimensions::Vec4
                    }
                    gltf::Semantic::Weights(set) => {
                        if palette.is_some() && set != 0 {
                            return Err("Static skin preview currently supports four joint influences per vertex".into());
                        }
                        matches!(
                            accessor.data_type(),
                            DataType::U8 | DataType::U16 | DataType::F32
                        ) && accessor.dimensions() == Dimensions::Vec4
                            && (accessor.data_type() == DataType::F32 || accessor.normalized())
                    }
                    _ => true,
                };
                if !valid {
                    return Err(
                        "Mesh attribute has an unsupported component type or dimensions".into(),
                    );
                }
                validate_accessor(&accessor, &buffers)?;
            }
            if let Some(indices) = primitive.indices() {
                if indices.count() > MAX_INDICES {
                    return Err("glTF exceeds the 3,000,000-index limit".into());
                }
                if !matches!(
                    indices.data_type(),
                    gltf::accessor::DataType::U8
                        | gltf::accessor::DataType::U16
                        | gltf::accessor::DataType::U32
                ) || indices.dimensions() != gltf::accessor::Dimensions::Scalar
                {
                    return Err("Mesh indices must be unsigned scalar integers".into());
                }
                validate_accessor(&indices, &buffers)?;
            }
            let material = primitive.material();
            let pbr = material.pbr_metallic_roughness();
            let color_texture = pbr.base_color_texture();
            let reader = primitive.reader(|buffer| buffers.get(buffer.index()).map(|b| b.as_ref()));
            let mut positions: Vec<_> = reader
                .read_positions()
                .ok_or("Mesh has no readable POSITION attribute")?
                .collect();
            if positions.is_empty() {
                continue;
            }
            let mut normal_values: Option<Vec<_>> = reader.read_normals().map(Iterator::collect);
            let uv: Option<Vec<_>> = reader
                .read_tex_coords(color_texture.as_ref().map_or(0, |t| t.tex_coord()))
                .map(|v| v.into_f32().collect());
            let colors: Option<Vec<_>> = reader.read_colors(0).map(|v| v.into_rgba_f32().collect());
            for count in [
                normal_values.as_ref().map(Vec::len),
                uv.as_ref().map(Vec::len),
                colors.as_ref().map(Vec::len),
            ]
            .into_iter()
            .flatten()
            {
                if count != positions.len() {
                    return Err("Mesh vertex attributes have mismatched lengths".into());
                }
            }
            if color_texture.is_some() && uv.is_none() {
                return Err("Textured mesh is missing its texture coordinates".into());
            }
            if let Some(palette) = &palette {
                let joints: Vec<_> = reader
                    .read_joints(0)
                    .ok_or("Skinned mesh is missing JOINTS_0")?
                    .into_u16()
                    .collect();
                let weights: Vec<_> = reader
                    .read_weights(0)
                    .ok_or("Skinned mesh is missing WEIGHTS_0")?
                    .into_f32()
                    .collect();
                crate::skin::bake(
                    &mut positions,
                    normal_values.as_deref_mut(),
                    &joints,
                    &weights,
                    palette,
                )?;
            }
            let raw_indices = reader
                .read_indices()
                .map(|v| v.into_u32().collect())
                .unwrap_or_else(|| (0..positions.len() as u32).collect());
            let mut indices = triangles(raw_indices, primitive.mode())?;
            if indices
                .iter()
                .any(|&index| index as usize >= positions.len())
            {
                return Err("Mesh index is outside the vertex buffer".into());
            }
            if determinant < 0.0 {
                for triangle in indices.as_chunks_mut::<3>().0 {
                    triangle.swap(1, 2);
                }
            }
            let mut vertices = Vec::with_capacity(positions.len());
            for (index, position) in positions.into_iter().enumerate() {
                let position = transform.transform_point3(Vec3::from_array(position));
                let normal = normal_values.as_ref().map_or(Vec3::Y, |values| {
                    (normals * Vec3::from_array(values[index])).normalize_or_zero()
                });
                let uv = uv.as_ref().map_or([0.0; 2], |values| values[index]);
                let color = colors.as_ref().map_or([1.0; 4], |values| values[index]);
                if !position.is_finite()
                    || !normal.is_finite()
                    || !uv.into_iter().all(f32::is_finite)
                    || !color.into_iter().all(f32::is_finite)
                {
                    return Err("Mesh contains non-finite vertex data".into());
                }
                minimum = minimum.min(position);
                maximum = maximum.max(position);
                vertices.push(Vertex {
                    position: position.to_array(),
                    normal: normal.to_array(),
                    uv,
                    color,
                });
            }
            if normal_values.is_none() {
                if indices.len() > MAX_VERTICES {
                    return Err("Generating flat normals exceeds the vertex limit".into());
                }
                let mut flat = Vec::with_capacity(indices.len());
                for triangle in indices.as_chunks::<3>().0 {
                    let [a, b, c] = [triangle[0], triangle[1], triangle[2]]
                        .map(|i| Vec3::from_array(vertices[i as usize].position));
                    let normal = (b - a).cross(c - a).normalize_or_zero().to_array();
                    flat.extend(triangle.iter().map(|&i| Vertex {
                        normal,
                        ..vertices[i as usize]
                    }));
                }
                vertices = flat;
                indices = (0..vertices.len() as u32).collect();
            }
            vertex_count += vertices.len();
            index_count += indices.len();
            if vertex_count > MAX_VERTICES || index_count > MAX_INDICES {
                return Err("glTF scene exceeds the geometry budget (1,000,000 vertices / 3,000,000 indices)".into());
            }
            if indices.is_empty() {
                continue;
            }
            let texture = color_texture.as_ref().map(|info| info.texture());
            let sampler = texture.as_ref().map(|texture| texture.sampler());
            let center = vertices
                .iter()
                .map(|v| Vec3::from_array(v.position))
                .sum::<Vec3>()
                / vertices.len() as f32;
            primitives.push(Primitive {
                vertices,
                indices,
                center,
                material: Material {
                    color: pbr.base_color_factor(),
                    texture: texture.as_ref().map(|texture| texture.source().index()),
                    wrap: sampler
                        .as_ref()
                        .map_or([gltf::texture::WrappingMode::Repeat; 2], |s| {
                            [s.wrap_s(), s.wrap_t()]
                        }),
                    nearest: sampler
                        .as_ref()
                        .is_some_and(|s| s.mag_filter() == Some(gltf::texture::MagFilter::Nearest)),
                    alpha: material.alpha_mode(),
                    cutoff: material.alpha_cutoff().unwrap_or(0.5),
                    double_sided: material.double_sided(),
                    unlit: material.unlit(),
                },
            });
        }
    }
    if primitives.is_empty() {
        return Err("glTF scene contains no visible triangle meshes".into());
    }
    if !(maximum - minimum).is_finite() || (maximum - minimum).length() > 1e15 {
        return Err("glTF scene bounds are too large for a stable preview".into());
    }
    Ok(Scene {
        primitives,
        images,
        minimum,
        maximum,
        nodes: nodes.len(),
        animations: gltf.animations().len(),
        draco_primitives,
        posed_meshes,
    })
}

fn data_uri(uri: &str) -> Result<Vec<u8>, String> {
    let Some(data) = uri.strip_prefix("data:") else {
        return Err(format!(
            "External resource '{uri}' is not supported yet. Export a self-contained GLB or embed buffers and images in the glTF file."
        ));
    };
    let (header, encoded) = data.split_once(',').ok_or("Invalid glTF data URI")?;
    if !header.ends_with(";base64") {
        return Err("glTF data URIs must use base64 encoding".into());
    }
    if encoded.len() > MAX_RESOURCE_BYTES.div_ceil(3) * 4 {
        return Err("Embedded resource exceeds the 64 MiB limit".into());
    }
    let decoded = STANDARD
        .decode(encoded)
        .map_err(|e| format!("Invalid embedded base64: {e}"))?;
    if decoded.len() > MAX_RESOURCE_BYTES {
        return Err("Embedded resource exceeds the 64 MiB limit".into());
    }
    Ok(decoded)
}

fn decode_image(bytes: &[u8]) -> Result<Image, String> {
    let make_reader = || -> Result<_, String> {
        let mut reader = image::ImageReader::new(Cursor::new(bytes))
            .with_guessed_format()
            .map_err(|e| e.to_string())?;
        if !matches!(
            reader.format(),
            Some(image::ImageFormat::Png | image::ImageFormat::Jpeg)
        ) {
            return Err("glTF preview supports PNG/JPEG textures; compressed textures are not supported yet".into());
        }
        let mut limits = image::Limits::default();
        limits.max_alloc = Some(MAX_RESOURCE_BYTES as u64);
        reader.limits(limits);
        Ok(reader)
    };
    let (width, height) = make_reader()?
        .into_dimensions()
        .map_err(|e| e.to_string())?;
    if width == 0
        || height == 0
        || u64::from(width) * u64::from(height) > MAX_RESOURCE_BYTES as u64 / 4
    {
        return Err("glTF texture exceeds the decoded-pixel limit".into());
    }
    let rgba = make_reader()?
        .decode()
        .map_err(|e| e.to_string())?
        .into_rgba8();
    Ok(Image {
        size: [width, height],
        rgba: rgba.into_raw().into(),
    })
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

fn triangles(indices: Vec<u32>, mode: gltf::mesh::Mode) -> Result<Vec<u32>, String> {
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
