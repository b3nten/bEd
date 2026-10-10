//! Validate self-contained documents and adapt encodings Bevy cannot read.
//! Preserve source JSON; ordinary assets go straight to Bevy unchanged.
use crate::model::{MAX_INDICES, MAX_NODES, MAX_RESOURCE_BYTES, MAX_TEXTURE_BYTES, MAX_VERTICES};
use base64::{Engine, engine::general_purpose::STANDARD};
use gltf::json;
use serde_json::{Value, json as value};
use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    io::Cursor,
    sync::Arc,
};

pub(super) struct PreparedGltf {
    pub bytes: Arc<[u8]>,
    pub nodes: usize,
    pub animations: usize,
    pub draco_primitives: usize,
    pub meshopt_views: usize,
    pub meshes: Arc<crate::mesh_compat::Meshes>,
    pub warnings: Vec<String>,
}

const WEBP: &str = "EXT_texture_webp";
const QUANTIZATION: &str = "KHR_mesh_quantization";
const SUPPORTED_REQUIRED: &[&str] = &[
    "KHR_materials_unlit",
    "KHR_materials_emissive_strength",
    "KHR_texture_transform",
    "KHR_materials_ior",
    "KHR_materials_transmission",
    "KHR_materials_volume",
    crate::draco::EXTENSION,
    QUANTIZATION,
    WEBP,
    "EXT_meshopt_compression",
    "KHR_meshopt_compression",
];

pub(super) fn load(bytes: &[u8]) -> Result<PreparedGltf, String> {
    if bytes.len() > MAX_RESOURCE_BYTES * 2 {
        return Err("glTF document exceeds the 128 MiB input limit".into());
    }
    let (json_bytes, blob) = if bytes.starts_with(b"glTF") {
        let glb = gltf::binary::Glb::from_slice(bytes).map_err(|error| error.to_string())?;
        (glb.json, glb.bin)
    } else {
        (Cow::Borrowed(bytes), None)
    };
    let original: Value = serde_json::from_slice(&json_bytes).map_err(|error| error.to_string())?;
    let mut root = original.clone();
    if !root.is_object() {
        return Err("glTF root must be an object".into());
    }
    for extension in array(&root, "extensionsRequired")? {
        let name = extension
            .as_str()
            .ok_or("Invalid glTF required extension name")?;
        if !SUPPORTED_REQUIRED.contains(&name) {
            return Err(format!("Required glTF extension is not supported: {name}"));
        }
    }
    if array(&root, "nodes")?.len() > MAX_NODES {
        return Err("glTF exceeds the 100,000-node limit".into());
    }
    let quantized = ["extensionsUsed", "extensionsRequired"].iter().any(|key| {
        root[*key]
            .as_array()
            .is_some_and(|extensions| extensions.iter().any(|v| v.as_str() == Some(QUANTIZATION)))
    });
    if let Some(scenes) = root.get_mut("scenes").and_then(Value::as_array_mut) {
        for scene in scenes {
            if let Some(scene) = scene.as_object_mut() {
                scene.entry("nodes").or_insert_with(|| value!([]));
            }
        }
    }
    let mut buffers = Vec::new();
    let mut source_bytes = 0_usize;
    for (index, buffer) in array(&root, "buffers")?.iter().enumerate() {
        let declared = integer(buffer, "byteLength")?;
        let placeholder = crate::meshopt::is_placeholder(&root, index, blob.is_some())?;
        if declared == 0 && !placeholder {
            return Err("glTF buffer byteLength must be positive".into());
        }
        let data = if placeholder {
            Cow::Borrowed(&[][..])
        } else if let Some(uri) = buffer.get("uri") {
            Cow::Owned(data_uri(uri.as_str().ok_or("Invalid glTF buffer URI")?)?)
        } else if index == 0 {
            Cow::Borrowed(blob.as_deref().ok_or("GLB has no binary chunk")?)
        } else {
            return Err("Only buffer zero can refer to the GLB binary chunk".into());
        };
        if !placeholder && data.len() < declared {
            return Err("glTF buffer is shorter than its declared length".into());
        }
        source_bytes = source_bytes
            .checked_add(data.len())
            .ok_or("glTF buffer byte count overflow")?;
        if source_bytes > MAX_RESOURCE_BYTES {
            return Err("glTF buffers exceed the 64 MiB resource limit".into());
        }
        let data = if placeholder {
            data
        } else {
            match data {
                Cow::Borrowed(bytes) => Cow::Borrowed(&bytes[..declared]),
                Cow::Owned(mut bytes) => {
                    bytes.truncate(declared);
                    Cow::Owned(bytes)
                }
            }
        };
        buffers.push(data);
    }
    let mut expanded_bytes = 0;
    let meshopt_views = crate::meshopt::expand(&mut root, &mut buffers, &mut expanded_bytes)?;
    let mut warnings = Vec::new();
    normalize_images(&mut root, &buffers, &mut source_bytes)?;
    validate_materials(&root, &mut warnings)?;
    // Validate a copy. Never round-trip the source through gltf-json: that
    // changes valid fields (including empty scene node lists) and extension data.
    let parsed: json::Root = serde_json::from_value(root.clone()).map_err(|e| e.to_string())?;
    let compressed = crate::draco::validate_source(&parsed)?;
    let document = gltf::Document::from_json_without_validation(parsed);
    for view in document.views() {
        if buffers.get(view.buffer().index()).is_none_or(|b| {
            view.offset()
                .checked_add(view.length())
                .is_none_or(|end| end > b.len())
        }) {
            return Err("Buffer view is out of bounds".into());
        }
    }
    for accessor in document.accessors() {
        if compressed.contains(&accessor.index()) && accessor.view().is_none() {
            continue;
        }
        crate::model::validate_accessor(&accessor, &buffers)?;
        validate_floats(&accessor, &buffers)?;
    }
    let (meshes, draco_primitives) = crate::mesh_compat::prepare(
        &document,
        &buffers,
        quantized,
        MAX_RESOURCE_BYTES - expanded_bytes,
    )?;
    validate_geometry(&document, &buffers, &meshes, &root)?;
    let nodes = document
        .default_scene()
        .or_else(|| document.scenes().next())
        .ok_or("glTF has no scene")?;
    let nodes = crate::skin::scene_nodes(nodes, document.nodes().len())?.len();
    let animations = document.animations().len();
    // Keep ordinary assets byte-for-byte intact. Only unsupported buffer/image
    // encodings need a repacked GLB; mesh adapters are handed directly to Bevy.
    let bytes: Arc<[u8]> = if root == original {
        Arc::from(bytes)
    } else {
        pack_glb(root, &buffers)?.into()
    };
    Ok(PreparedGltf {
        bytes,
        nodes,
        animations,
        draco_primitives,
        meshopt_views,
        meshes: Arc::new(meshes),
        warnings,
    })
}

fn array<'a>(root: &'a Value, key: &str) -> Result<&'a [Value], String> {
    match root.get(key) {
        None => Ok(&[]),
        Some(value) => value
            .as_array()
            .map(Vec::as_slice)
            .ok_or_else(|| format!("Invalid glTF {key} array")),
    }
}
fn integer(value: &Value, key: &str) -> Result<usize, String> {
    value[key]
        .as_u64()
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| format!("Invalid glTF {key}"))
}
fn remove_declaration(root: &mut Value, extension: &str) {
    for key in ["extensionsUsed", "extensionsRequired"] {
        if let Some(list) = root.get_mut(key).and_then(Value::as_array_mut) {
            list.retain(|v| v.as_str() != Some(extension));
        }
    }
}
fn data_uri(uri: &str) -> Result<Vec<u8>, String> {
    let data = uri.strip_prefix("data:").ok_or_else(|| format!("External resource '{uri}' is not supported yet. Export a self-contained GLB or embed buffers and images in the glTF file."))?;
    let (header, encoded) = data.split_once(',').ok_or("Invalid glTF data URI")?;
    if !header.ends_with(";base64") {
        return Err("glTF data URIs must use base64 encoding".into());
    }
    if encoded.len() > MAX_RESOURCE_BYTES.div_ceil(3) * 4 {
        return Err("Embedded resource exceeds the 64 MiB limit".into());
    }
    let decoded = STANDARD
        .decode(encoded)
        .map_err(|error| format!("Invalid embedded base64: {error}"))?;
    if decoded.len() > MAX_RESOURCE_BYTES {
        return Err("Embedded resource exceeds the 64 MiB limit".into());
    }
    Ok(decoded)
}

fn validate_floats(accessor: &gltf::Accessor<'_>, buffers: &[Cow<'_, [u8]>]) -> Result<(), String> {
    if accessor.data_type() != gltf::accessor::DataType::F32 {
        return Ok(());
    }
    let check = |bytes: &[u8], count: usize, stride: usize, size: usize| {
        for element in bytes.chunks(stride).take(count) {
            if element[..size]
                .chunks_exact(4)
                .any(|value| !f32::from_le_bytes(value.try_into().unwrap()).is_finite())
            {
                return Err("Mesh contains non-finite vertex data".to_owned());
            }
        }
        Ok(())
    };
    if let Some(view) = accessor.view() {
        let bytes = &buffers[view.buffer().index()]
            [view.offset() + accessor.offset()..view.offset() + view.length()];
        check(
            bytes,
            accessor.count(),
            view.stride().unwrap_or(accessor.size()),
            accessor.size(),
        )?;
    }
    if let Some(sparse) = accessor.sparse() {
        let values = sparse.values();
        let view = values.view();
        let bytes = &buffers[view.buffer().index()]
            [view.offset() + values.offset()..view.offset() + view.length()];
        check(bytes, sparse.count(), accessor.size(), accessor.size())?;
    }
    Ok(())
}

/// Read one validated accessor into float components, respecting interleaving
/// and sparse overrides. Normalization uses glTF's signed endpoint equations.
pub(super) fn float_values(
    accessor: &gltf::Accessor<'_>,
    buffers: &[Cow<'_, [u8]>],
) -> Result<Vec<f32>, String> {
    crate::model::validate_accessor(accessor, buffers)?;
    let dimensions = accessor.dimensions().multiplicity();
    let component = accessor.data_type();
    let normalized = accessor.normalized();
    let decode = |bytes: &[u8]| component_value(bytes, component, normalized);
    let mut values = vec![0.0; accessor.count() * dimensions];
    if let Some(view) = accessor.view() {
        let bytes = &buffers[view.buffer().index()][view.offset() + accessor.offset()..];
        let stride = view.stride().unwrap_or(accessor.size());
        for (index, out) in values.chunks_exact_mut(dimensions).enumerate() {
            for (component_index, out) in out.iter_mut().enumerate() {
                *out = decode(&bytes[index * stride + component_index * component.size()..]);
            }
        }
    }
    if let Some(sparse) = accessor.sparse() {
        let indices = sparse.indices();
        let view = indices.view();
        let index_bytes = &buffers[view.buffer().index()][view.offset() + indices.offset()..];
        let source = sparse.values();
        let view = source.view();
        let data = &buffers[view.buffer().index()][view.offset() + source.offset()..];
        let index_size = indices.index_type().size();
        for sparse_index in 0..sparse.count() {
            let index = &index_bytes[sparse_index * index_size..][..index_size];
            let index = match index_size {
                1 => index[0] as usize,
                2 => u16::from_le_bytes(index.try_into().unwrap()) as usize,
                _ => u32::from_le_bytes(index.try_into().unwrap()) as usize,
            };
            for component_index in 0..dimensions {
                values[index * dimensions + component_index] = decode(
                    &data[(sparse_index * dimensions + component_index) * component.size()..],
                );
            }
        }
    }
    if values.iter().any(|v| !v.is_finite()) {
        return Err("Mesh contains non-finite vertex data".into());
    }
    Ok(values)
}

pub(super) fn component_value(
    bytes: &[u8],
    component: gltf::accessor::DataType,
    normalized: bool,
) -> f32 {
    use gltf::accessor::DataType;
    let value = match component {
        DataType::I8 => i8::from_le_bytes([bytes[0]]) as f32,
        DataType::U8 => bytes[0] as f32,
        DataType::I16 => i16::from_le_bytes(bytes[..2].try_into().unwrap()) as f32,
        DataType::U16 => u16::from_le_bytes(bytes[..2].try_into().unwrap()) as f32,
        DataType::U32 => u32::from_le_bytes(bytes[..4].try_into().unwrap()) as f32,
        DataType::F32 => f32::from_le_bytes(bytes[..4].try_into().unwrap()),
    };
    if !normalized {
        return value;
    }
    match component {
        DataType::I8 => (value / 127.0).max(-1.0),
        DataType::U8 => value / 255.0,
        DataType::I16 => (value / 32767.0).max(-1.0),
        DataType::U16 => value / 65535.0,
        _ => value,
    }
}

fn validate_geometry(
    document: &gltf::Document,
    buffers: &[Cow<'_, [u8]>],
    meshes: &crate::mesh_compat::Meshes,
    root: &Value,
) -> Result<(), String> {
    use gltf::accessor::{DataType, Dimensions};
    // Bevy checks all node cycles recursively, including unreachable nodes.
    // Establish a bounded forest here before invoking that recursive reader.
    let mut parents = vec![0_u8; document.nodes().len()];
    for node in document.nodes() {
        if !glam::Mat4::from_cols_array_2d(&node.transform().matrix()).is_finite() {
            return Err("glTF node has a non-finite transform".into());
        }
        for child in node.children() {
            parents[child.index()] += 1;
            if parents[child.index()] > 1 {
                return Err("glTF scene contains a cycle or a node with multiple parents".into());
            }
        }
    }
    let mut stack: Vec<_> = document
        .nodes()
        .filter(|n| parents[n.index()] == 0)
        .map(|node| (node, 1_usize))
        .collect();
    let mut visited = 0;
    while let Some((node, depth)) = stack.pop() {
        if depth > 256 {
            return Err("glTF hierarchy exceeds the 256-level depth limit".into());
        }
        visited += 1;
        stack.extend(node.children().map(|node| (node, depth + 1)));
    }
    if visited != document.nodes().len() {
        return Err("glTF scene contains a cycle or a node with multiple parents".into());
    }
    for skin in document.skins() {
        if let Some(accessor) = skin.inverse_bind_matrices() {
            if accessor.data_type() != DataType::F32
                || accessor.dimensions() != Dimensions::Mat4
                || accessor.normalized()
                || accessor.count() != skin.joints().len()
            {
                return Err("Skin inverse-bind matrices do not match the joints".into());
            }
        }
    }
    let materials = array(root, "materials")?;
    let empty_material = Value::Null;
    let mut total_vertices = 0_usize;
    let mut total_indices = 0_usize;
    for mesh in document.meshes() {
        for primitive in mesh.primitives() {
            let positions = primitive
                .get(&gltf::Semantic::Positions)
                .ok_or("Mesh has no readable POSITION attribute")?;
            let count = positions.count();
            for (_, accessor) in primitive.attributes() {
                if accessor.count() != count {
                    return Err("Mesh vertex attributes have mismatched lengths".into());
                }
            }
            let reader = primitive.reader(|buffer| buffers.get(buffer.index()).map(|b| b.as_ref()));
            let index_count = if let Some(mesh) = meshes.get(&(mesh.index(), primitive.index())) {
                mesh.indices().map_or(count, |i| i.len())
            } else if let Some(accessor) = primitive.indices() {
                if !matches!(
                    accessor.data_type(),
                    DataType::U8 | DataType::U16 | DataType::U32
                ) || accessor.dimensions() != Dimensions::Scalar
                    || accessor.normalized()
                {
                    return Err("Mesh indices must be unsigned scalar integers".into());
                }
                if reader
                    .read_indices()
                    .ok_or("Mesh contains unreadable indices")?
                    .into_u32()
                    .any(|index| index as usize >= count)
                {
                    return Err("Mesh index is outside the vertex buffer".into());
                }
                accessor.count()
            } else {
                count
            };
            if !index_count.is_multiple_of(3) {
                return Err("Triangle index count is not a multiple of three".into());
            }
            total_vertices += if primitive.get(&gltf::Semantic::Normals).is_none() {
                index_count
            } else {
                count
            };
            total_indices += index_count;
            if total_vertices > MAX_VERTICES || total_indices > MAX_INDICES {
                return Err("glTF scene exceeds the geometry budget (1,000,000 vertices / 3,000,000 indices)".into());
            }
            let material = primitive
                .material()
                .index()
                .map_or(&empty_material, |i| &materials[i]);
            for info in texture_slots(material) {
                let uv = effective_uv(info)?;
                if primitive.get(&gltf::Semantic::TexCoords(uv)).is_none() {
                    return Err(format!(
                        "Textured mesh is missing TEXCOORD_{uv} texture coordinates"
                    ));
                }
            }
        }
    }
    if total_indices == 0 {
        return Err("glTF scene contains no visible triangle meshes".into());
    }
    // Bound instancing without skinning or transforming every vertex twice.
    for scene in document.scenes() {
        let mut vertices = 0;
        let mut indices = 0;
        let nodes = crate::skin::scene_nodes(scene, document.nodes().len())?;
        let present: BTreeSet<_> = nodes.iter().map(|(node, _)| node.index()).collect();
        for (node, _) in nodes {
            if let Some(skin) = node.skin() {
                if skin.joints().any(|joint| !present.contains(&joint.index())) {
                    return Err("Skin joint is outside the selected scene".into());
                }
            }
            if let Some(mesh) = node.mesh() {
                for primitive in mesh.primitives() {
                    let count = primitive.get(&gltf::Semantic::Positions).unwrap().count();
                    let index_count = meshes.get(&(mesh.index(), primitive.index())).map_or_else(
                        || primitive.indices().map_or(count, |a| a.count()),
                        |m| m.indices().map_or(count, |i| i.len()),
                    );
                    vertices += if primitive.get(&gltf::Semantic::Normals).is_none() {
                        index_count
                    } else {
                        count
                    };
                    indices += index_count;
                    if vertices > MAX_VERTICES || indices > MAX_INDICES {
                        return Err("glTF scene exceeds the geometry budget (1,000,000 vertices / 3,000,000 indices)".into());
                    }
                }
            }
        }
    }
    Ok(())
}

fn texture_slots(material: &Value) -> Vec<&Value> {
    [
        material.pointer("/pbrMetallicRoughness/baseColorTexture"),
        material.pointer("/pbrMetallicRoughness/metallicRoughnessTexture"),
        material.get("normalTexture"),
        material.get("occlusionTexture"),
        material.get("emissiveTexture"),
    ]
    .into_iter()
    .flatten()
    .collect()
}
fn effective_uv(info: &Value) -> Result<u32, String> {
    let tex_coord = info
        .pointer("/extensions/KHR_texture_transform/texCoord")
        .or_else(|| info.get("texCoord"));
    let uv = match tex_coord {
        None => 0,
        Some(value) => value.as_u64().ok_or("Invalid texture coordinate set")?,
    };
    if uv > 1 {
        return Err(format!(
            "Texture uses TEXCOORD_{uv}; the glTF preview supports TEXCOORD_0 and TEXCOORD_1"
        ));
    }
    Ok(uv as u32)
}

fn validate_materials(root: &Value, warnings: &mut Vec<String>) -> Result<(), String> {
    let mapping = |transform: Option<&Value>| {
        let mut values = [0.0_f64, 0.0, 1.0, 1.0, 0.0];
        if let Some(transform) = transform {
            for (field, start) in [("offset", 0), ("scale", 2)] {
                if let Some(array) = transform[field].as_array() {
                    for (i, value) in array.iter().take(2).enumerate() {
                        values[start + i] = value.as_f64().unwrap_or(f64::NAN);
                    }
                }
            }
            if let Some(value) = transform.get("rotation") {
                values[4] = value.as_f64().unwrap_or(f64::NAN);
            }
        }
        values
    };
    for (index, material) in array(root, "materials")?.iter().enumerate() {
        for (extension, texture) in [
            ("KHR_materials_transmission", "transmissionTexture"),
            ("KHR_materials_volume", "thicknessTexture"),
        ] {
            if material["extensions"][extension].get(texture).is_some() {
                let required = array(root, "extensionsRequired")?
                    .iter()
                    .any(|name| name.as_str() == Some(extension));
                if required {
                    return Err(format!(
                        "Required {extension} texture support is not enabled in the preview"
                    ));
                }
                warnings.push(format!("Material {index}: Bevy's {extension} textures are not enabled; only the material factors are displayed."));
            }
        }
        // Validate numeric material data, including extension factors, at the
        // boundary. serde's f32 conversion can overflow finite JSON numbers.
        fn finite(value: &Value) -> bool {
            match value {
                Value::Number(n) => n.as_f64().is_some_and(|n| (n as f32).is_finite()),
                Value::Array(values) => values.iter().all(finite),
                Value::Object(values) => values
                    .iter()
                    .all(|(key, value)| key == "extras" || finite(value)),
                _ => true,
            }
        }
        if !finite(material) {
            return Err("Material contains non-finite values".into());
        }
        if material
            .pointer("/extensions/KHR_materials_emissive_strength/emissiveStrength")
            .is_some_and(|v| v.as_f64().is_none_or(|v| v < 0.0))
        {
            return Err("Invalid emissive strength".into());
        }
        for (slot, factor) in [("normalTexture", "scale"), ("occlusionTexture", "strength")] {
            if material[slot]
                .get(factor)
                .is_some_and(|v| v.as_f64() != Some(1.0))
            {
                warnings.push(format!("Material {index}: Bevy does not apply {slot} {factor}; the texture is displayed at its default strength."));
            }
        }
        let base_transform = material
            .pointer("/pbrMetallicRoughness/baseColorTexture/extensions/KHR_texture_transform");
        for info in texture_slots(material) {
            effective_uv(info)?;
            if let Some(transform) = info.pointer("/extensions/KHR_texture_transform") {
                if !transform.is_object() {
                    return Err("Invalid texture transform".into());
                }
                for field in ["offset", "scale"] {
                    if let Some(values) = transform.get(field) {
                        if !values.as_array().is_some_and(|v| {
                            v.len() == 2
                                && v.iter()
                                    .all(|n| n.as_f64().is_some_and(|v| (v as f32).is_finite()))
                        }) {
                            return Err(format!("Invalid texture transform {field}"));
                        }
                    }
                }
                if transform
                    .get("rotation")
                    .is_some_and(|v| v.as_f64().is_none_or(|v| !(v as f32).is_finite()))
                {
                    return Err("Invalid texture transform rotation".into());
                }
            }
            if mapping(info.pointer("/extensions/KHR_texture_transform")) != mapping(base_transform)
            {
                let warning = format!(
                    "Material {index}: Bevy applies the base-color texture transform to all texture slots; independent slot transforms are not supported."
                );
                if !warnings.contains(&warning) {
                    warnings.push(warning);
                }
            }
        }
    }
    Ok(())
}

fn normalize_images(
    root: &mut Value,
    buffers: &[Cow<'_, [u8]>],
    source_bytes: &mut usize,
) -> Result<(), String> {
    let mut images = array(root, "images")?.to_vec();
    let mut textures = array(root, "textures")?.to_vec();
    let mut decoded_sizes = BTreeMap::new();
    let mut formats = BTreeMap::new();
    let mut decoded_bytes = 0_usize;
    for texture in &mut textures {
        let webp = texture.pointer("/extensions/EXT_texture_webp");
        let image_index = if let Some(extension) = webp {
            integer(extension, "source")?
        } else {
            integer(texture, "source")?
        };
        if let std::collections::btree_map::Entry::Vacant(entry) = decoded_sizes.entry(image_index)
        {
            let mut image = images
                .get(image_index)
                .cloned()
                .ok_or("Texture references a missing image")?;
            let (bytes, embedded) = if let Some(uri) = image.get("uri") {
                (
                    Cow::Owned(data_uri(uri.as_str().ok_or("Invalid image URI")?)?),
                    true,
                )
            } else {
                let index = integer(&image, "bufferView")?;
                let view = root["bufferViews"]
                    .get(index)
                    .ok_or("Image buffer view is out of bounds")?;
                let buffer = buffers
                    .get(integer(view, "buffer")?)
                    .ok_or("Missing image buffer")?;
                let start = match view.get("byteOffset") {
                    Some(_) => integer(view, "byteOffset")?,
                    None => 0,
                };
                let end = start
                    .checked_add(integer(view, "byteLength")?)
                    .ok_or("Invalid image buffer range")?;
                (
                    Cow::Borrowed(
                        buffer
                            .get(start..end)
                            .ok_or("Image buffer view is out of bounds")?,
                    ),
                    false,
                )
            };
            let format = image::guess_format(&bytes).map_err(|error| error.to_string())?;
            if !matches!(
                format,
                image::ImageFormat::Png | image::ImageFormat::Jpeg | image::ImageFormat::WebP
            ) {
                return Err("glTF preview supports PNG, JPEG, and WebP textures".into());
            }
            if webp.is_some() && format != image::ImageFormat::WebP {
                return Err("EXT_texture_webp source is not a WebP image".into());
            }
            let pixel_bytes = image_size(&bytes)?;
            decoded_bytes += pixel_bytes;
            if decoded_bytes > MAX_TEXTURE_BYTES {
                return Err(texture_limit_error());
            }
            let mime = match format {
                image::ImageFormat::Png => "image/png",
                image::ImageFormat::Jpeg => "image/jpeg",
                _ => "image/webp",
            };
            if embedded {
                *source_bytes = source_bytes
                    .checked_add(bytes.len())
                    .filter(|n| *n <= MAX_RESOURCE_BYTES)
                    .ok_or("glTF resources exceed the 64 MiB resource limit")?;
            }
            if !embedded || image.get("mimeType").is_some() {
                image["mimeType"] = value!(mime);
            }
            images[image_index] = image;
            entry.insert(pixel_bytes);
            formats.insert(image_index, format);
        }
        // Even an image shared by a core source and a WebP source must satisfy
        // the extension's format requirement on the latter reference.
        if webp.is_some() && formats[&image_index] != image::ImageFormat::WebP {
            return Err("EXT_texture_webp source is not a WebP image".into());
        }
        texture["source"] = value!(image_index);
        if let Some(ext) = texture.get_mut("extensions").and_then(Value::as_object_mut) {
            ext.remove(WEBP);
        }
    }
    // Bevy chooses color space per texture index. Give color slots their own
    // texture when the same index is also used by a linear-data slot.
    let mut materials = array(root, "materials")?.to_vec();
    let mut linear = BTreeSet::new();
    for material in &materials {
        for info in [
            material.pointer("/pbrMetallicRoughness/metallicRoughnessTexture"),
            material.get("normalTexture"),
            material.get("occlusionTexture"),
        ]
        .into_iter()
        .flatten()
        {
            linear.insert(integer(info, "index")?);
        }
    }
    let mut colors = BTreeMap::new();
    for material in &mut materials {
        for path in [
            "/pbrMetallicRoughness/baseColorTexture",
            "/pbrMetallicRoughness/metallicRoughnessTexture",
            "/normalTexture",
            "/occlusionTexture",
            "/emissiveTexture",
        ] {
            if let Some(info) = material.pointer_mut(path) {
                let uv = effective_uv(info)?;
                if info.get("texCoord").and_then(Value::as_u64).unwrap_or(0) != u64::from(uv) {
                    info["texCoord"] = value!(uv);
                }
            }
        }
        for path in ["/pbrMetallicRoughness/baseColorTexture", "/emissiveTexture"] {
            if let Some(info) = material.pointer_mut(path) {
                let original = integer(info, "index")?;
                if linear.contains(&original) {
                    let replacement = *colors.entry(original).or_insert_with(|| {
                        let index = textures.len();
                        if let Some(texture) = textures.get(original).cloned() {
                            textures.push(texture);
                        }
                        index
                    });
                    if replacement >= textures.len() {
                        return Err("Material references a missing texture".into());
                    }
                    info["index"] = value!(replacement);
                }
            }
        }
    }
    let mut texture_bytes = 0_usize;
    for texture in &textures {
        let image = decoded_sizes
            .get(&integer(texture, "source")?)
            .ok_or("Texture references a missing image")?;
        texture_bytes = texture_bytes
            .checked_add(*image)
            .filter(|n| *n <= MAX_TEXTURE_BYTES)
            .ok_or_else(texture_limit_error)?;
    }
    for (key, values) in [
        ("images", images),
        ("textures", textures),
        ("materials", materials),
    ] {
        if root.get(key).is_some() || !values.is_empty() {
            root[key] = Value::Array(values);
        }
    }
    remove_declaration(root, WEBP);
    Ok(())
}

fn texture_limit_error() -> String {
    format!(
        "glTF textures exceed the {} MiB total decoded-pixel limit",
        MAX_TEXTURE_BYTES / (1024 * 1024)
    )
}

fn image_size(bytes: &[u8]) -> Result<usize, String> {
    let reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    let (width, height) = reader.into_dimensions().map_err(|e| e.to_string())?;
    if width == 0
        || height == 0
        || u64::from(width) * u64::from(height) > MAX_RESOURCE_BYTES as u64 / 4
    {
        return Err("glTF texture exceeds the decoded-pixel limit".into());
    }
    Ok(width as usize * height as usize * 4)
}

fn pack_glb(mut root: Value, buffers: &[Cow<'_, [u8]>]) -> Result<Vec<u8>, String> {
    let mut data = Vec::new();
    let mut offsets = Vec::new();
    for buffer in buffers {
        let offset = data.len().next_multiple_of(4);
        if offset
            .checked_add(buffer.len())
            .is_none_or(|n| n > MAX_RESOURCE_BYTES * 4)
        {
            return Err("Prepared glTF exceeds the 256 MiB output limit".into());
        }
        data.resize(offset, 0);
        offsets.push(offset);
        data.extend_from_slice(buffer);
    }
    if let Some(views) = root["bufferViews"].as_array_mut() {
        for view in views {
            let offset = view
                .get("byteOffset")
                .map_or(Ok(0), |_| integer(view, "byteOffset"))?;
            let buffer = integer(view, "buffer")?;
            let length = integer(view, "byteLength")?;
            if buffers
                .get(buffer)
                .is_none_or(|b| offset.checked_add(length).is_none_or(|end| end > b.len()))
            {
                return Err("Buffer view is out of bounds".into());
            }
            view["byteOffset"] = value!(
                offsets[buffer]
                    .checked_add(offset)
                    .ok_or("Buffer view offset overflow")?
            );
            view["buffer"] = value!(0);
        }
    }
    root["buffers"] = if data.is_empty() {
        value!([])
    } else {
        value!([{"byteLength":data.len()}])
    };
    let mut json = serde_json::to_vec(&root).map_err(|error| error.to_string())?;
    json.resize(json.len().next_multiple_of(4), b' ');
    data.resize(data.len().next_multiple_of(4), 0);
    let length = 12_usize
        .checked_add(8 + json.len())
        .and_then(|n| n.checked_add(if data.is_empty() { 0 } else { 8 + data.len() }))
        .and_then(|n| u32::try_from(n).ok())
        .ok_or("Prepared GLB byte count overflow")?;
    let mut output = Vec::with_capacity(length as usize);
    for word in [0x46546c67_u32, 2, length, json.len() as u32, 0x4e4f534a] {
        output.extend_from_slice(&word.to_le_bytes());
    }
    output.extend_from_slice(&json);
    if !data.is_empty() {
        output.extend_from_slice(&(data.len() as u32).to_le_bytes());
        output.extend_from_slice(&0x004e4942_u32.to_le_bytes());
        output.extend_from_slice(&data);
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn cube() -> Value {
        serde_json::from_slice(include_bytes!("../../../tests/fixtures/gltf/cube.gltf")).unwrap()
    }
    fn prepared(root: &Value) -> (PreparedGltf, gltf::Gltf) {
        let prepared = load(&serde_json::to_vec(root).unwrap()).unwrap();
        let document = gltf::Gltf::from_slice_without_validation(&prepared.bytes).unwrap();
        (prepared, document)
    }
    #[test]
    fn ordinary_assets_reach_bevy_byte_for_byte_without_mesh_preparation() {
        for bytes in [
            include_bytes!("../../../tests/fixtures/gltf/cube.gltf").as_slice(),
            include_bytes!("../../../tests/fixtures/gltf/cube.glb").as_slice(),
        ] {
            let prepared = load(bytes).unwrap();
            assert_eq!(&*prepared.bytes, bytes);
            assert!(prepared.meshes.is_empty());
            let scene = crate::bevy_scene::load_prepared_snapshot(&prepared).unwrap();
            assert_eq!(scene.triangles(), 12);
        }
    }
    #[test]
    fn empty_secondary_scene_survives_preparation_and_bevy_loading() {
        let mut source = cube();
        source["scenes"].as_array_mut().unwrap().push(value!({
            "name": "Empty scene", "nodes": []
        }));
        let (first_prepared, document) = prepared(&source);
        assert_eq!(document.scenes().len(), 2);
        assert_eq!(document.scenes().nth(1).unwrap().nodes().len(), 0);
        let scene = crate::bevy_scene::load_snapshot(&first_prepared.bytes).unwrap();
        assert_eq!(scene.triangles(), 12);
        source["scenes"][1].as_object_mut().unwrap().remove("nodes");
        let (prepared, document) = prepared(&source);
        assert_eq!(document.scenes().nth(1).unwrap().nodes().len(), 0);
        assert_eq!(
            crate::bevy_scene::load_prepared_snapshot(&prepared)
                .unwrap()
                .triangles(),
            12
        );
    }
    fn buffer(root: &mut Value, bytes: &[u8]) -> usize {
        let index = root["buffers"].as_array().unwrap().len();
        root["buffers"].as_array_mut().unwrap().push(value!({"byteLength":bytes.len(), "uri":format!("data:application/octet-stream;base64,{}", STANDARD.encode(bytes))}));
        index
    }
    fn triangle(bytes: &[u8]) -> Value {
        value!({"asset":{"version":"2.0"}, "scene":0, "scenes":[{"nodes":[0]}], "nodes":[{"mesh":0}], "meshes":[{"primitives":[{"attributes":{"POSITION":0}}]}], "accessors":[{"bufferView":0, "componentType":5126, "count":3, "type":"VEC3", "min":[0,0,0], "max":[1,1,0]}], "bufferViews":[{"buffer":0,"byteLength":bytes.len()}], "buffers":[{"byteLength":bytes.len(), "uri":format!("data:application/octet-stream;base64,{}", STANDARD.encode(bytes))}]})
    }
    #[test]
    fn meshopt_variants_replace_missing_fallback_views_with_identical_accessors() {
        let positions = [[0.0_f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        for name in crate::meshopt::EXTENSIONS {
            let mut encoded =
                vec![0; unsafe { meshopt::ffi::meshopt_encodeVertexBufferBound(3, 12) }];
            let length = unsafe {
                meshopt::ffi::meshopt_encodeVertexBufferLevel(
                    encoded.as_mut_ptr(),
                    encoded.len(),
                    positions.as_ptr().cast(),
                    3,
                    12,
                    2,
                    if name.starts_with("KHR") { 1 } else { 0 },
                )
            };
            encoded.truncate(length);
            let mut root = triangle(&encoded);
            root["extras"] = value!({"authorMetadata": {"emptyArray":[], "emptyObject":{}}});
            root["nodes"][0]["extras"] = value!({"selection":42});
            root["scenes"]
                .as_array_mut()
                .unwrap()
                .push(value!({"nodes":[],"extras":{"emptyScene":true}}));
            root["buffers"]
                .as_array_mut()
                .unwrap()
                .push(value!({"byteLength":1_000_000_000}));
            root["bufferViews"][0] = value!({"buffer":1,"byteLength":36,"byteStride":12,"extensions":{name:{"buffer":0,"byteLength":encoded.len(),"byteStride":12,"count":3,"mode":"ATTRIBUTES"}}});
            root["extensionsRequired"] = value!([name]);
            root["extensionsUsed"] = value!([name]);
            let (prepared, doc) = prepared(&root);
            assert_eq!(prepared.meshopt_views, 1);
            assert!(prepared.bytes.len() < 2000);
            assert!(!doc.extensions_required().any(|ext| ext == name));
            let packed = gltf::binary::Glb::from_slice(&prepared.bytes).unwrap();
            let json: Value = serde_json::from_slice(&packed.json).unwrap();
            assert_eq!(json["extras"], root["extras"]);
            assert_eq!(json["nodes"][0]["extras"], root["nodes"][0]["extras"]);
            assert_eq!(json["scenes"], root["scenes"]);
            let primitive = doc.meshes().next().unwrap().primitives().next().unwrap();
            let actual: Vec<_> = primitive
                .reader(|_| doc.blob.as_deref())
                .read_positions()
                .unwrap()
                .collect();
            assert_eq!(actual, positions);
        }
    }
    #[test]
    fn quantization_converts_sparse_interleaved_signed_values_without_baking_transforms() {
        let source: Vec<_> = [0_i16, 0, 0, 99, 100, 0, 0, 99, 0, 100, 0, 99]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let mut root = triangle(&source);
        root["nodes"][0]["scale"] = value!([-2, 2, 2]);
        root["extensionsRequired"] = value!([QUANTIZATION]);
        root["bufferViews"][0]["byteStride"] = value!(8);
        let indices = buffer(&mut root, &[1]);
        let sparse: Vec<_> = [-32768_i16, 32767, 0]
            .iter()
            .flat_map(|n| n.to_le_bytes())
            .collect();
        let values = buffer(&mut root, &sparse);
        root["bufferViews"].as_array_mut().unwrap().extend([
            value!({"buffer":indices,"byteLength":1}),
            value!({"buffer":values,"byteLength":6}),
        ]);
        root["accessors"][0] = value!({"bufferView":0,"componentType":5122,"normalized":true,"count":3,"type":"VEC3","min":[-32768,0,0],"max":[100,32767,0],"sparse":{"count":1,"indices":{"bufferView":1,"componentType":5121},"values":{"bufferView":2}}});
        let source_bytes = serde_json::to_vec(&root).unwrap();
        let prepared = load(&source_bytes).unwrap();
        assert_eq!(&*prepared.bytes, source_bytes);
        let doc = gltf::Gltf::from_slice_without_validation(&prepared.bytes).unwrap();
        let primitive = doc.meshes().next().unwrap().primitives().next().unwrap();
        let bevy::mesh::VertexAttributeValues::Float32x3(actual) = prepared.meshes[&(0, 0)]
            .attribute(bevy::mesh::Mesh::ATTRIBUTE_POSITION)
            .unwrap()
        else {
            panic!("expected float positions")
        };
        assert_eq!(
            actual.as_slice(),
            [
                [0.0, 0.0, 0.0],
                [-1.0, 1.0, 0.0],
                [0.0, 100.0 / 32767.0, 0.0]
            ]
        );
        assert_eq!(
            doc.nodes().next().unwrap().transform().decomposed().2,
            [-2.0, 2.0, 2.0]
        );
        let accessor = primitive.get(&gltf::Semantic::Positions).unwrap();
        assert_eq!(accessor.data_type(), gltf::accessor::DataType::I16);
        assert!(accessor.normalized());
        assert!(accessor.sparse().is_some());
        let scene = crate::bevy_scene::load_prepared_snapshot(&prepared).unwrap();
        assert_eq!(scene.triangles(), 1);
        assert_eq!(scene.maximum, glam::Vec3::new(2.0, 2.0, 0.0));
    }
    #[test]
    fn webp_without_core_source_selects_webp_and_leaves_fallback_unloaded() {
        use image::ImageEncoder;
        let mut webp = Vec::new();
        image::codecs::webp::WebPEncoder::new_lossless(&mut webp)
            .write_image(&[10, 20, 30, 255], 1, 1, image::ExtendedColorType::Rgba8)
            .unwrap();
        let mut root = cube();
        root["images"]
            .as_array_mut()
            .unwrap()
            .push(value!({"uri":format!("data:image/webp;base64,{}",STANDARD.encode(webp))}));
        root["images"][0]["uri"] = value!("missing-fallback.png");
        root["textures"][0] = value!({"extensions":{WEBP:{"source":1}}});
        root["extensionsRequired"] = value!([WEBP]);
        let (_, doc) = prepared(&root);
        assert_eq!(doc.images().len(), 2);
        assert_eq!(doc.textures().next().unwrap().source().index(), 1);
        assert!(matches!(
            doc.images().nth(1).unwrap().source(),
            gltf::image::Source::Uri { uri, .. } if uri.starts_with("data:image/webp;")
        ));
        assert!(!doc.extensions_required().any(|ext| ext == WEBP));
    }
    #[test]
    fn colorspace_is_preserved_and_unsupported_material_factors_warn_without_rebaking() {
        let mut root = cube();
        root["materials"][0]["normalTexture"] = value!({"index":0,"scale":0.0});
        root["materials"][0]["occlusionTexture"] = value!({"index":0,"strength":0.5});
        let (prepared, doc) = prepared(&root);
        let material = doc.materials().next().unwrap();
        let color = material
            .pbr_metallic_roughness()
            .base_color_texture()
            .unwrap()
            .texture()
            .index();
        let normal = material.normal_texture().unwrap();
        let ao = material.occlusion_texture().unwrap();
        assert_ne!(color, normal.texture().index());
        assert_ne!(color, ao.texture().index());
        assert_eq!(normal.texture().index(), ao.texture().index());
        assert_eq!(normal.scale(), 0.0);
        assert_eq!(ao.strength(), 0.5);
        assert_eq!(doc.images().len(), 1);
        assert_eq!(prepared.warnings.len(), 2);
        assert!(prepared.warnings[0].contains("normalTexture scale"));
        assert!(prepared.warnings[1].contains("occlusionTexture strength"));
    }
    #[test]
    fn texture_transform_uv_override_is_written_to_all_slots_and_warns_about_slot_mappings() {
        let mut root = cube();
        let position_count = root["accessors"]
            [root["meshes"][0]["primitives"][0]["attributes"]["POSITION"]
                .as_u64()
                .unwrap() as usize]["count"]
            .as_u64()
            .unwrap() as usize;
        let uv = root["meshes"][0]["primitives"][0]["attributes"]["TEXCOORD_0"].clone();
        root["meshes"][0]["primitives"][0]["attributes"]["TEXCOORD_1"] = uv;
        assert!(position_count > 0);
        root["materials"][0]["pbrMetallicRoughness"]["baseColorTexture"]["extensions"] =
            value!({"KHR_texture_transform":{"offset":[0.1,0.2],"texCoord":1}});
        root["materials"][0]["normalTexture"] =
            value!({"index":0,"extensions":{"KHR_texture_transform":{"texCoord":1}}});
        let (prepared, doc) = prepared(&root);
        let material = doc.materials().next().unwrap();
        assert_eq!(
            material
                .pbr_metallic_roughness()
                .base_color_texture()
                .unwrap()
                .tex_coord(),
            1
        );
        assert_eq!(material.normal_texture().unwrap().tex_coord(), 1);
        assert_eq!(prepared.warnings.len(), 1);
    }
    #[test]
    fn offset_only_texture_transform_preserves_the_default_core_uv_set() {
        let mut root = cube();
        root["materials"][0]["pbrMetallicRoughness"]["baseColorTexture"]["extensions"] =
            value!({"KHR_texture_transform":{"offset":[0.25,-0.125]}});
        let (_, doc) = prepared(&root);
        let info = doc
            .materials()
            .next()
            .unwrap()
            .pbr_metallic_roughness()
            .base_color_texture()
            .unwrap();
        assert_eq!(info.tex_coord(), 0);
        assert_eq!(info.texture_transform().unwrap().offset(), [0.25, -0.125]);

        root["materials"][0]["pbrMetallicRoughness"]["baseColorTexture"]["extensions"]["KHR_texture_transform"]
            ["texCoord"] = Value::Null;
        assert!(
            load(&serde_json::to_vec(&root).unwrap())
                .err()
                .unwrap()
                .contains("Invalid texture coordinate set")
        );
    }
    #[test]
    fn malformed_meshopt_and_unsupported_required_extensions_fail_at_the_boundary() {
        let mut root = cube();
        root["extensionsRequired"] = value!(["KHR_texture_basisu"]);
        assert!(
            load(&serde_json::to_vec(&root).unwrap())
                .err()
                .unwrap()
                .contains("Required glTF extension")
        );
        root["extensionsRequired"] = value!([]);
        root["bufferViews"][0]["extensions"] = value!({"EXT_meshopt_compression":{"buffer":0,"byteLength":usize::MAX,"byteStride":12,"count":3,"mode":"ATTRIBUTES"}});
        assert!(load(&serde_json::to_vec(&root).unwrap()).is_err());
    }
    #[test]
    fn excessive_hierarchy_depth_is_rejected_even_when_unreachable_from_the_scene() {
        let mut root = cube();
        let nodes = root["nodes"].as_array_mut().unwrap();
        for i in 1..=257 {
            nodes.push(if i == 257 {
                value!({})
            } else {
                value!({"children":[i + 1]})
            });
        }
        assert!(
            load(&serde_json::to_vec(&root).unwrap())
                .err()
                .unwrap()
                .contains("256-level depth limit")
        );
    }
}
