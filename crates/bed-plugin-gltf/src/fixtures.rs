//! Encode the hand-built GPU fixtures as real assets so native tests exercise
//! the same glTF loader and material construction as the viewer.
use crate::model::{Scene, TextureInfo};
use image::ImageEncoder;
use serde_json::{Value, json};

/// Mutable JSON form for fixtures that add authored hierarchy or extensions.
pub(super) fn embedded_scene(scene: &Scene) -> Value {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let bytes = scene_bytes(scene);
    let gltf = gltf::Gltf::from_slice(&bytes).unwrap();
    let mut document = serde_json::to_value(gltf.document.into_json()).unwrap();
    document["buffers"][0]["uri"] = json!(format!(
        "data:application/octet-stream;base64,{}",
        STANDARD.encode(gltf.blob.unwrap())
    ));
    document
}

pub(super) fn scene_bytes(scene: &Scene) -> Vec<u8> {
    let mut binary = Vec::new();
    let mut views = Vec::new();
    let mut accessors = Vec::new();
    let mut meshes = Vec::new();
    let mut nodes = Vec::new();
    let mut materials = Vec::new();
    let mut textures = Vec::new();
    let mut samplers = Vec::new();
    let mut extensions = Vec::new();
    let mut images = Vec::new();

    for image in &scene.images {
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(
                &image.rgba,
                image.size[0],
                image.size[1],
                image::ExtendedColorType::Rgba8,
            )
            .unwrap();
        let view = buffer_view(&mut binary, &mut views, &png, None);
        images.push(json!({"bufferView": view, "mimeType": "image/png"}));
    }

    for primitive in &scene.primitives {
        let count = primitive.vertices.len();
        let mut attributes = serde_json::Map::new();
        // Separate tightly packed accessors avoid imposing any layout on the
        // runtime vertex representation.
        for (name, dimensions, width) in [
            ("POSITION", "VEC3", 3),
            ("NORMAL", "VEC3", 3),
            ("TEXCOORD_0", "VEC2", 2),
            ("TEXCOORD_1", "VEC2", 2),
            ("COLOR_0", "VEC4", 4),
            ("TANGENT", "VEC4", 4),
        ] {
            if name == "TANGENT" && !primitive.has_tangents {
                continue;
            }
            let mut bytes = Vec::with_capacity(count * width * 4);
            let mut minimum = vec![f32::INFINITY; width];
            let mut maximum = vec![f32::NEG_INFINITY; width];
            for vertex in &primitive.vertices {
                let components: &[f32] = match name {
                    "POSITION" => &vertex.position,
                    "NORMAL" => &vertex.normal,
                    "TEXCOORD_0" => &vertex.uv0,
                    "TEXCOORD_1" => &vertex.uv1,
                    "COLOR_0" => &vertex.color,
                    "TANGENT" => &vertex.tangent,
                    _ => unreachable!(),
                };
                for (index, &component) in components.iter().enumerate() {
                    bytes.extend_from_slice(&component.to_le_bytes());
                    minimum[index] = minimum[index].min(component);
                    maximum[index] = maximum[index].max(component);
                }
            }
            let view = buffer_view(&mut binary, &mut views, &bytes, Some(34962));
            let mut accessor = json!({
                "bufferView": view, "componentType": 5126,
                "count": count, "type": dimensions,
            });
            if name == "POSITION" {
                accessor["min"] = json!(minimum);
                accessor["max"] = json!(maximum);
            }
            attributes.insert(name.into(), json!(accessors.len()));
            accessors.push(accessor);
        }
        let indices: Vec<_> = primitive
            .indices
            .iter()
            .flat_map(|index| index.to_le_bytes())
            .collect();
        let view = buffer_view(&mut binary, &mut views, &indices, Some(34963));
        let indices = accessors.len();
        accessors.push(json!({
            "bufferView": view, "componentType": 5125,
            "count": primitive.indices.len(), "type": "SCALAR",
        }));

        let source = &primitive.material;
        let mut pbr = json!({
            "baseColorFactor": source.color, "metallicFactor": source.metallic,
            "roughnessFactor": source.roughness,
        });
        if let Some(info) = source.texture {
            pbr["baseColorTexture"] = texture(info, &mut textures, &mut samplers);
        }
        if let Some(info) = source.metallic_roughness_texture {
            pbr["metallicRoughnessTexture"] = texture(info, &mut textures, &mut samplers);
        }
        // glTF's core emissive factor is bounded by one; preserve higher values
        // through the standard emissive-strength extension.
        let strength = source.emissive.into_iter().fold(1.0_f32, f32::max);
        let mut material = json!({
            "pbrMetallicRoughness": pbr,
            "emissiveFactor": source.emissive.map(|component| component / strength),
            "alphaMode": match source.alpha {
                gltf::material::AlphaMode::Opaque => "OPAQUE",
                gltf::material::AlphaMode::Mask => "MASK",
                gltf::material::AlphaMode::Blend => "BLEND",
            },
            "alphaCutoff": source.cutoff, "doubleSided": source.double_sided,
        });
        if let Some(info) = source.normal_texture {
            let mut info = texture(info, &mut textures, &mut samplers);
            info["scale"] = json!(source.normal_scale);
            material["normalTexture"] = info;
        }
        if let Some(info) = source.occlusion_texture {
            let mut info = texture(info, &mut textures, &mut samplers);
            info["strength"] = json!(source.occlusion_strength);
            material["occlusionTexture"] = info;
        }
        if let Some(info) = source.emissive_texture {
            material["emissiveTexture"] = texture(info, &mut textures, &mut samplers);
        }
        let mut material_extensions = serde_json::Map::new();
        if source.unlit {
            material_extensions.insert("KHR_materials_unlit".into(), json!({}));
            if !extensions.contains(&"KHR_materials_unlit") {
                extensions.push("KHR_materials_unlit");
            }
        }
        if strength != 1.0 {
            material_extensions.insert(
                "KHR_materials_emissive_strength".into(),
                json!({"emissiveStrength": strength}),
            );
            if !extensions.contains(&"KHR_materials_emissive_strength") {
                extensions.push("KHR_materials_emissive_strength");
            }
        }
        if !material_extensions.is_empty() {
            material["extensions"] = Value::Object(material_extensions);
        }
        meshes.push(json!({"primitives": [{
            "attributes": attributes, "indices": indices,
            "material": materials.len(), "mode": 4,
        }]}));
        nodes.push(json!({"mesh": meshes.len() - 1}));
        materials.push(material);
    }

    let mut document = json!({
        "asset": {"version": "2.0"}, "scene": 0,
        "scenes": [{"nodes": (0..nodes.len()).collect::<Vec<_>>()}],
        "nodes": nodes, "meshes": meshes, "materials": materials,
        "accessors": accessors, "bufferViews": views,
        "buffers": [{"byteLength": binary.len()}],
    });
    if !images.is_empty() {
        document["images"] = json!(images);
    }
    if !textures.is_empty() {
        document["textures"] = json!(textures);
        document["samplers"] = json!(samplers);
    }
    if !extensions.is_empty() {
        document["extensionsUsed"] = json!(extensions);
    }
    let mut json = serde_json::to_vec(&document).unwrap();
    while json.len() % 4 != 0 {
        json.push(b' ');
    }
    while binary.len() % 4 != 0 {
        binary.push(0);
    }
    let length = 12 + 8 + json.len() + 8 + binary.len();
    let mut result = Vec::with_capacity(length);
    result.extend_from_slice(b"glTF");
    result.extend_from_slice(&2_u32.to_le_bytes());
    result.extend_from_slice(&(length as u32).to_le_bytes());
    result.extend_from_slice(&(json.len() as u32).to_le_bytes());
    result.extend_from_slice(b"JSON");
    result.extend_from_slice(&json);
    result.extend_from_slice(&(binary.len() as u32).to_le_bytes());
    result.extend_from_slice(b"BIN\0");
    result.extend_from_slice(&binary);
    result
}

fn buffer_view(
    binary: &mut Vec<u8>,
    views: &mut Vec<Value>,
    bytes: &[u8],
    target: Option<u32>,
) -> usize {
    while binary.len() % 4 != 0 {
        binary.push(0);
    }
    let mut view = json!({
        "buffer": 0, "byteOffset": binary.len(), "byteLength": bytes.len(),
    });
    if let Some(target) = target {
        view["target"] = json!(target);
    }
    let index = views.len();
    views.push(view);
    binary.extend_from_slice(bytes);
    index
}

fn texture(info: TextureInfo, textures: &mut Vec<Value>, samplers: &mut Vec<Value>) -> Value {
    let mut sampler = json!({
        "wrapS": info.sampler.wrap[0].as_gl_enum(),
        "wrapT": info.sampler.wrap[1].as_gl_enum(),
    });
    if let Some(filter) = info.sampler.mag_filter {
        sampler["magFilter"] = json!(filter.as_gl_enum());
    }
    if let Some(filter) = info.sampler.min_filter {
        sampler["minFilter"] = json!(filter.as_gl_enum());
    }
    let index = textures.len();
    textures.push(json!({"source": info.image, "sampler": samplers.len()}));
    samplers.push(sampler);
    json!({"index": index, "texCoord": info.tex_coord})
}
