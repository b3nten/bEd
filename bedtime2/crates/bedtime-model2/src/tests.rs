use super::*;
use crate::model::Scene;
use serde_json::{Value, json};

const CUBE_GLB: &[u8] = include_bytes!("../../../../bedtime/tests/fixtures/gltf/cube.glb");
const CUBE_GLTF: &[u8] = include_bytes!("../../../../bedtime/tests/fixtures/gltf/cube.gltf");

fn asset() -> Value {
    serde_json::from_slice(CUBE_GLTF).unwrap()
}

fn load(value: &Value) -> Result<Scene, String> {
    model::load(&serde_json::to_vec(value).unwrap())
}

fn add_float_attribute(value: &mut Value, semantic: &str, dimensions: &str, data: &[f32]) {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let bytes: Vec<_> = data
        .iter()
        .flat_map(|component| component.to_le_bytes())
        .collect();
    let buffer = value["buffers"].as_array().unwrap().len();
    let view = value["bufferViews"].as_array().unwrap().len();
    let accessor = value["accessors"].as_array().unwrap().len();
    value["buffers"].as_array_mut().unwrap().push(json!({
        "byteLength":bytes.len(),
        "uri":format!("data:application/octet-stream;base64,{}", STANDARD.encode(bytes))
    }));
    value["bufferViews"].as_array_mut().unwrap().push(json!({
        "buffer":buffer,"byteLength":data.len() * 4
    }));
    value["accessors"].as_array_mut().unwrap().push(json!({
        "bufferView":view,"componentType":5126,"count":24,"type":dimensions
    }));
    value["meshes"][0]["primitives"][0]["attributes"][semantic] = json!(accessor);
}

#[test]
fn pbr_material_maps_keep_independent_uv_sets_and_samplers() {
    use gltf::texture::{MagFilter, MinFilter, WrappingMode};
    let mut value = asset();
    add_float_attribute(&mut value, "TEXCOORD_1", "VEC2", &[0.25, 0.75].repeat(24));
    value["samplers"] = json!([
        {"wrapS":33071,"wrapT":33648,"magFilter":9728,"minFilter":9984},
        {"wrapS":10497,"wrapT":33071,"magFilter":9729,"minFilter":9987}
    ]);
    value["textures"] = json!([{"source":0,"sampler":0},{"source":0,"sampler":1}]);
    value["materials"][0] = json!({
        "pbrMetallicRoughness": {
            "baseColorFactor":[0.2,0.3,0.4,0.8],
            "baseColorTexture":{"index":0,"texCoord":0},
            "metallicFactor":0.45,"roughnessFactor":0.75,
            "metallicRoughnessTexture":{"index":1,"texCoord":1}
        },
        "normalTexture":{"index":1,"texCoord":1,"scale":0.3},
        "occlusionTexture":{"index":0,"texCoord":0,"strength":0.6},
        "emissiveFactor":[0.1,0.2,0.3],
        "emissiveTexture":{"index":1,"texCoord":1},
        "alphaMode":"MASK","alphaCutoff":0.25,"doubleSided":true
    });
    let scene = load(&value).unwrap();
    let primitive = &scene.primitives[0];
    let material = &primitive.material;
    assert_eq!(material.color, [0.2, 0.3, 0.4, 0.8]);
    assert_eq!(material.metallic, 0.45);
    assert_eq!(material.roughness, 0.75);
    // The compatibility pass bakes these factors into linear image variants;
    // the resulting stock Bevy material uses the default factors.
    assert_eq!(material.normal_scale, 1.0);
    assert_eq!(material.occlusion_strength, 1.0);
    assert_eq!(material.emissive, [0.1, 0.2, 0.3]);
    assert_eq!(material.alpha, gltf::material::AlphaMode::Mask);
    assert_eq!(material.cutoff, 0.25);
    assert!(material.double_sided);
    for info in [material.texture, material.occlusion_texture]
        .into_iter()
        .flatten()
    {
        assert_eq!(scene.images[info.image].size, [2, 2]);
        assert_eq!(info.tex_coord, 0);
        assert_eq!(
            info.sampler.wrap,
            [WrappingMode::ClampToEdge, WrappingMode::MirroredRepeat]
        );
        assert_eq!(info.sampler.mag_filter, Some(MagFilter::Nearest));
        assert_eq!(
            info.sampler.min_filter,
            Some(MinFilter::NearestMipmapNearest)
        );
    }
    for info in [
        material.metallic_roughness_texture,
        material.normal_texture,
        material.emissive_texture,
    ]
    .into_iter()
    .flatten()
    {
        assert_eq!(scene.images[info.image].size, [2, 2]);
        assert_eq!(info.tex_coord, 1);
        assert_eq!(
            info.sampler.wrap,
            [WrappingMode::Repeat, WrappingMode::ClampToEdge]
        );
        assert_eq!(info.sampler.mag_filter, Some(MagFilter::Linear));
        assert_eq!(info.sampler.min_filter, Some(MinFilter::LinearMipmapLinear));
    }
    assert_ne!(primitive.vertices[0].uv0, primitive.vertices[0].uv1);
    assert_eq!(primitive.vertices[0].uv1, [0.25, 0.75]);
}

#[test]
fn implicit_material_uses_gltf_pbr_defaults() {
    let mut value = asset();
    value["meshes"][0]["primitives"][0]
        .as_object_mut()
        .unwrap()
        .remove("material");
    let scene = load(&value).unwrap();
    let material = &scene.primitives[0].material;
    assert_eq!(material.color, [1.0; 4]);
    assert_eq!([material.metallic, material.roughness], [1.0; 2]);
    assert_eq!(
        [material.normal_scale, material.occlusion_strength],
        [1.0; 2]
    );
    assert_eq!(material.emissive, [0.0; 3]);
    assert!(material.texture.is_none());
    assert!(material.metallic_roughness_texture.is_none());
    assert!(material.normal_texture.is_none());
    assert!(material.occlusion_texture.is_none());
    assert!(material.emissive_texture.is_none());
    assert!(!scene.primitives[0].has_tangents);
}

#[test]
fn emissive_strength_is_imported_by_bevy() {
    let mut value = asset();
    value["extensionsUsed"] = json!(["KHR_materials_emissive_strength"]);
    value["extensionsRequired"] = value["extensionsUsed"].clone();
    value["materials"][0]["emissiveFactor"] = json!([0.25, 0.5, 1.0]);
    value["materials"][0]["extensions"] =
        json!({"KHR_materials_emissive_strength": {"emissiveStrength": 4.0}});
    let scene = load(&value).unwrap();
    assert_eq!(scene.primitives[0].material.emissive, [1.0, 2.0, 4.0]);
}

#[test]
fn texture_transform_uv_override_reaches_bevy_material() {
    let mut value = asset();
    add_float_attribute(&mut value, "TEXCOORD_1", "VEC2", &[0.25, 0.75].repeat(24));
    value["extensionsUsed"] = json!(["KHR_texture_transform"]);
    value["extensionsRequired"] = value["extensionsUsed"].clone();
    value["materials"][0]["pbrMetallicRoughness"]["baseColorTexture"]["texCoord"] = json!(0);
    value["materials"][0]["pbrMetallicRoughness"]["baseColorTexture"]["extensions"] =
        json!({"KHR_texture_transform": {"texCoord": 1}});
    let scene = load(&value).unwrap();
    assert_eq!(
        scene.primitives[0].material.texture.unwrap().tex_coord,
        1,
        "the extension UV override must select the effective Bevy material channel"
    );
}

#[test]
fn unused_extra_uv_sets_do_not_prevent_loading() {
    let mut value = asset();
    add_float_attribute(&mut value, "TEXCOORD_2", "VEC2", &[0.25, 0.75].repeat(24));
    let scene = load(&value).unwrap();
    assert_eq!(scene.primitives[0].material.texture.unwrap().tex_coord, 0);
    assert_eq!(scene.primitives[0].vertices.len(), 24);

    value["materials"][0]["pbrMetallicRoughness"]["baseColorTexture"]["texCoord"] = json!(2);
    let error = load(&value).err().unwrap();
    assert!(error.contains("TEXCOORD_2"), "{error}");
}

#[test]
fn unused_extra_color_sets_do_not_prevent_loading() {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let mut value = asset();
    add_float_attribute(&mut value, "COLOR_0", "VEC3", &[0.25, 0.5, 0.75].repeat(24));
    add_float_attribute(
        &mut value,
        "COLOR_1",
        "VEC4",
        &[1.0, 0.0, 1.0, 1.0].repeat(24),
    );

    let buffer = value["buffers"].as_array().unwrap().len();
    let view = value["bufferViews"].as_array().unwrap().len();
    let accessor = value["accessors"].as_array().unwrap().len();
    let bytes = [255_u8, 0, 0, 255].repeat(24);
    value["buffers"].as_array_mut().unwrap().push(json!({
        "byteLength":bytes.len(),
        "uri":format!("data:application/octet-stream;base64,{}", STANDARD.encode(&bytes))
    }));
    value["bufferViews"].as_array_mut().unwrap().push(json!({
        "buffer":buffer,"byteLength":bytes.len()
    }));
    value["accessors"].as_array_mut().unwrap().push(json!({
        "bufferView":view,"componentType":5121,"normalized":true,"count":24,"type":"VEC4"
    }));
    value["meshes"][0]["primitives"][0]["attributes"]["COLOR_2"] = json!(accessor);

    let scene = load(&value).unwrap();
    assert_eq!(scene.primitives[0].vertices.len(), 24);
    assert!(
        scene.primitives[0]
            .vertices
            .iter()
            .all(|vertex| { vertex.color == [0.25, 0.5, 0.75, 1.0] })
    );

    value["accessors"][accessor]["type"] = json!("VEC2");
    let error = load(&value).err().unwrap();
    assert!(
        error.contains("unsupported component type or dimensions"),
        "{error}"
    );
}

#[test]
fn unused_zero_byte_meshopt_fallback_does_not_prevent_loading() {
    for extension in meshopt::EXTENSIONS {
        let mut value = asset();
        value["extensionsUsed"] = json!([extension]);
        value["extensionsRequired"] = json!([extension]);
        value["buffers"].as_array_mut().unwrap().push(json!({
            "byteLength":0,"extensions":{extension:{"fallback":true}}
        }));
        let scene = load(&value).unwrap();
        assert_eq!(scene.primitives[0].vertices.len(), 24);
        assert_eq!(scene.meshopt_views, 0);
    }

    let mut value = asset();
    value["buffers"][0]["byteLength"] = json!(0);
    let error = load(&value).err().unwrap();
    assert!(error.contains("byteLength must be positive"), "{error}");
}

#[test]
fn all_material_maps_require_supported_present_uv_sets() {
    for path in [
        "/pbrMetallicRoughness/baseColorTexture",
        "/pbrMetallicRoughness/metallicRoughnessTexture",
        "/normalTexture",
        "/occlusionTexture",
        "/emissiveTexture",
    ] {
        for tex_coord in [0, 1, 2] {
            let mut value = asset();
            if tex_coord == 0 {
                value["meshes"][0]["primitives"][0]["attributes"]
                    .as_object_mut()
                    .unwrap()
                    .remove("TEXCOORD_0");
            }
            let material = &mut value["materials"][0];
            let binding = json!({"index":0,"texCoord":tex_coord});
            if path.starts_with("/pbrMetallicRoughness/") {
                material["pbrMetallicRoughness"][path.rsplit('/').next().unwrap()] = binding;
            } else {
                material[path.trim_start_matches('/')] = binding;
            }
            let error = load(&value).err().unwrap();
            assert!(
                error.contains(&format!("TEXCOORD_{tex_coord}")),
                "{path}: {error}"
            );
        }
    }
}

#[test]
fn authored_tangents_follow_node_reflection_and_are_validated() {
    let mut value = asset();
    add_float_attribute(
        &mut value,
        "TANGENT",
        "VEC4",
        &[1.0, 0.0, 0.0, 1.0].repeat(24),
    );
    value["nodes"][0]["scale"] = json!([-2.0, 1.0, 1.0]);
    let scene = load(&value).unwrap();
    assert!(scene.primitives[0].has_tangents);
    assert_eq!(
        scene.primitives[0].vertices[0].tangent,
        [-1.0, 0.0, 0.0, -1.0]
    );
    let accessor = value["accessors"]
        .as_array_mut()
        .unwrap()
        .last_mut()
        .unwrap();
    accessor["type"] = json!("VEC3");
    assert!(load(&value).is_err());
}

#[test]
fn non_finite_normals_and_invalid_tangent_handedness_are_rejected() {
    for (semantic, dimensions, data) in [
        ("NORMAL", "VEC3", vec![f32::NAN, 0.0, 1.0]),
        ("TANGENT", "VEC4", vec![1.0, 0.0, 0.0, 0.0]),
        ("TANGENT", "VEC4", vec![f32::INFINITY, 0.0, 0.0, 1.0]),
    ] {
        let mut value = asset();
        add_float_attribute(&mut value, semantic, dimensions, &data.repeat(24));
        assert!(load(&value).is_err(), "{semantic}: {data:?}");
    }
}

#[test]
fn degenerate_authored_tangents_request_regeneration() {
    let mut value = asset();
    add_float_attribute(
        &mut value,
        "TANGENT",
        "VEC4",
        &[0.0, 0.0, 0.0, 1.0].repeat(24),
    );
    assert!(!load(&value).unwrap().primitives[0].has_tangents);
    add_float_attribute(
        &mut value,
        "TANGENT",
        "VEC4",
        &[1.0, 0.0, 0.0, 1.0].repeat(24),
    );
    value["meshes"][0]["primitives"][0]["attributes"]
        .as_object_mut()
        .unwrap()
        .remove("NORMAL");
    // Flat normals change the per-corner basis, so authored tangents must be
    // regenerated against the resulting geometry and normal-map UV channel.
    assert!(!load(&value).unwrap().primitives[0].has_tangents);
}

fn tokyo_bytes() -> Vec<u8> {
    std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../bedtime/tests/fixtures/gltf/LittlestTokyo.glb"),
    )
    .unwrap()
}

// Isolate a real exporter-produced stream, keeping malformed-input checks
// small and independent of the rest of LittlestTokyo's scene and materials.
fn draco_asset() -> Value {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let bytes = tokyo_bytes();
    let glb = gltf::binary::Glb::from_slice(&bytes).unwrap();
    let source: Value = serde_json::from_slice(&glb.json).unwrap();
    let mut primitive = source["meshes"]
        .as_array()
        .unwrap()
        .iter()
        .skip(8)
        .map(|m| &m["primitives"][0])
        .min_by_key(|p| {
            source["accessors"][p["attributes"]["POSITION"].as_u64().unwrap() as usize]["count"]
                .as_u64()
                .unwrap()
        })
        .unwrap()
        .clone();
    primitive.as_object_mut().unwrap().remove("material");
    let view = &source["bufferViews"][primitive["extensions"][draco::EXTENSION]["bufferView"]
        .as_u64()
        .unwrap() as usize];
    let offset = view["byteOffset"].as_u64().unwrap() as usize;
    let length = view["byteLength"].as_u64().unwrap() as usize;
    let compressed = &glb.bin.as_ref().unwrap()[offset..offset + length];
    let mut accessors = Vec::new();
    for index in primitive["attributes"]
        .as_object_mut()
        .unwrap()
        .values_mut()
    {
        accessors.push(source["accessors"][index.as_u64().unwrap() as usize].clone());
        *index = json!(accessors.len() - 1);
    }
    accessors.push(source["accessors"][primitive["indices"].as_u64().unwrap() as usize].clone());
    primitive["indices"] = json!(accessors.len() - 1);
    primitive["extensions"][draco::EXTENSION]["bufferView"] = json!(0);
    json!({"asset":{"version":"2.0"}, "scene":0, "scenes":[{"nodes":[0]}],
        "nodes":[{"mesh":0}], "meshes":[{"primitives":[primitive]}], "accessors":accessors,
        "extensionsRequired":[draco::EXTENSION], "extensionsUsed":[draco::EXTENSION],
        "bufferViews":[{"buffer":0,"byteLength":length}],
        "buffers":[{"byteLength":length,"uri":format!("data:application/octet-stream;base64,{}",STANDARD.encode(compressed))}]})
}

#[test]
fn draco_and_uncompressed_primitives_share_the_loader() {
    let mut value = draco_asset();
    let compressed = load(&value).unwrap();
    assert_eq!(compressed.draco_primitives, 1);
    let mut cube = asset();
    let accessor_offset = value["accessors"].as_array().unwrap().len();
    let view_offset = value["bufferViews"].as_array().unwrap().len();
    for view in cube["bufferViews"].as_array_mut().unwrap() {
        view["buffer"] = json!(1);
    }
    for accessor in cube["accessors"].as_array_mut().unwrap() {
        accessor["bufferView"] =
            json!(accessor["bufferView"].as_u64().unwrap() as usize + view_offset);
    }
    let primitive = &mut cube["meshes"][0]["primitives"][0];
    primitive.as_object_mut().unwrap().remove("material");
    for index in primitive["attributes"]
        .as_object_mut()
        .unwrap()
        .values_mut()
    {
        *index = json!(index.as_u64().unwrap() as usize + accessor_offset);
    }
    primitive["indices"] = json!(primitive["indices"].as_u64().unwrap() as usize + accessor_offset);
    for key in ["buffers", "bufferViews", "accessors", "meshes"] {
        value[key]
            .as_array_mut()
            .unwrap()
            .extend(cube[key].as_array().unwrap().iter().cloned());
    }
    value["nodes"]
        .as_array_mut()
        .unwrap()
        .push(json!({"mesh":1}));
    value["scenes"][0]["nodes"]
        .as_array_mut()
        .unwrap()
        .push(json!(1));
    let scene = load(&value).unwrap();
    assert_eq!(scene.draco_primitives, 1);
    assert_eq!(scene.primitives.len(), 2);
    assert_eq!(scene.triangles(), compressed.triangles() + 12);
    // Draco strips are materialized as triangle lists for the shared renderer.
    value["meshes"][0]["primitives"][0]["mode"] = json!(5);
    assert_eq!(load(&value).unwrap().triangles(), scene.triangles());
}

#[test]
fn malformed_draco_maps_buffers_and_declarations_fail_cleanly() {
    let source = draco_asset();
    for (field, invalid) in [
        ("bufferView", json!(9999)),
        ("attributes", json!({"POSITION":9999})),
    ] {
        let mut value = source.clone();
        value["meshes"][0]["primitives"][0]["extensions"][draco::EXTENSION][field] = invalid;
        assert!(load(&value).is_err());
    }
    let mut value = source.clone();
    value["bufferViews"][0]["byteLength"] = json!(2);
    assert!(load(&value).is_err());
    let position = source["meshes"][0]["primitives"][0]["attributes"]["POSITION"]
        .as_u64()
        .unwrap() as usize;
    for (field, invalid) in [
        ("count", json!(1_000_001)),
        ("componentType", json!(5123)),
        ("type", json!("VEC4")),
    ] {
        let mut value = source.clone();
        value["accessors"][position][field] = invalid;
        assert!(load(&value).is_err());
    }
    let mut value = source;
    value["meshes"][0]["primitives"][0]["attributes"]["POSITION"] = json!(9999);
    assert!(load(&value).is_err());
}

#[test]
fn draco_decoder_enforces_allocation_and_topology_limits() {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use draco_core::DecodeLimits;
    let value = draco_asset();
    let uri = value["buffers"][0]["uri"].as_str().unwrap();
    let bytes = STANDARD.decode(uri.split_once(',').unwrap().1).unwrap();
    assert!(draco::decode(&bytes, DecodeLimits::default()).is_ok());
    for limits in [
        DecodeLimits::default().with_max_faces(1),
        DecodeLimits::default().with_max_decoded_bytes(1),
    ] {
        assert!(draco::decode(&bytes, limits).is_err());
    }
}

#[test]
fn glb_and_embedded_gltf_load_the_same_textured_scene() {
    for bytes in [CUBE_GLB, CUBE_GLTF] {
        let scene = model::load(bytes).unwrap();
        assert_eq!(scene.triangles(), 12);
        assert_eq!(scene.primitives[0].vertices.len(), 24);
        assert_eq!(scene.images[0].size, [2, 2]);
        assert_eq!(scene.minimum, glam::Vec3::splat(-1.0));
        assert_eq!(scene.maximum, glam::Vec3::splat(1.0));
    }
}

#[test]
fn static_skin_uses_joint_hierarchy_and_inverse_bind_without_mesh_transform() {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use glam::{Mat4, Vec3};
    let mut value = asset();
    let mut bytes = vec![0; 24 * 4]; // Every vertex uses joint 0.
    for _ in 0..24 {
        for weight in [1.0_f32, 0.0, 0.0, 0.0] {
            bytes.extend(weight.to_le_bytes());
        }
    }
    for component in Mat4::from_translation(-Vec3::X).to_cols_array() {
        bytes.extend(component.to_le_bytes());
    }
    value["buffers"].as_array_mut().unwrap().push(json!({
        "byteLength":bytes.len(),
        "uri":format!("data:application/octet-stream;base64,{}", STANDARD.encode(bytes))
    }));
    value["bufferViews"].as_array_mut().unwrap().extend([
        json!({"buffer":1,"byteOffset":0,"byteLength":96}),
        json!({"buffer":1,"byteOffset":96,"byteLength":384}),
        json!({"buffer":1,"byteOffset":480,"byteLength":64}),
    ]);
    value["accessors"].as_array_mut().unwrap().extend([
        json!({"bufferView":5,"componentType":5121,"count":24,"type":"VEC4"}),
        json!({"bufferView":6,"componentType":5126,"count":24,"type":"VEC4"}),
        json!({"bufferView":7,"componentType":5126,"count":1,"type":"MAT4"}),
    ]);
    value["meshes"][0]["primitives"][0]["attributes"]["JOINTS_0"] = json!(4);
    value["meshes"][0]["primitives"][0]["attributes"]["WEIGHTS_0"] = json!(5);
    value["nodes"] = json!([
        {"mesh":0,"skin":0,"translation":[1000.0,0.0,0.0]},
        {"translation":[4.0,0.0,0.0],"children":[2]},
        {"translation":[0.0,2.0,0.0]}
    ]);
    value["scenes"][0]["nodes"] = json!([0, 1]);
    value["skins"] = json!([{"joints":[2],"inverseBindMatrices":6}]);
    let scene = load(&value).unwrap();
    assert_eq!(scene.posed_meshes, 1);
    assert_eq!(scene.minimum, Vec3::new(2.0, 1.0, -1.0));
    assert_eq!(scene.maximum, Vec3::new(4.0, 3.0, 1.0));
    value["scenes"][0]["nodes"] = json!([0]);
    assert!(matches!(load(&value), Err(error) if error.contains("outside the selected scene")));
}

#[test]
fn node_transforms_and_reflections_preserve_mesh_orientation() {
    let mut value = asset();
    value["nodes"][0]["translation"] = json!([5, 2, 3]);
    value["nodes"][0]["scale"] = json!([-2, 1, 1]);
    let scene = load(&value).unwrap();
    assert_eq!(scene.minimum, glam::Vec3::new(3.0, 1.0, 2.0));
    assert_eq!(scene.maximum, glam::Vec3::new(7.0, 3.0, 4.0));
    let p = &scene.primitives[0];
    let triangle = &p.indices[..3];
    let [a, b, c] = [triangle[0], triangle[1], triangle[2]]
        .map(|i| glam::Vec3::from_array(p.vertices[i as usize].position));
    assert!(
        (b - a).cross(c - a).dot(glam::Vec3::from_array(
            p.vertices[triangle[0] as usize].normal
        )) > 0.0
    );
    // Models exported in large units often use a small scene-root scale.
    let mut value = asset();
    value["nodes"][0]["scale"] = json!([0.0001, 0.0001, 0.0001]);
    let scene = load(&value).unwrap();
    assert_eq!(scene.triangles(), 12);
    assert!(scene.maximum.x > 0.0 && scene.maximum.x < 0.001);
}

#[test]
fn missing_normals_generate_flat_normals() {
    let mut value = asset();
    value["meshes"][0]["primitives"][0]["attributes"]
        .as_object_mut()
        .unwrap()
        .remove("NORMAL");
    let scene = load(&value).unwrap();
    assert_eq!(scene.primitives[0].vertices.len(), 36);
    for vertex in &scene.primitives[0].vertices {
        assert!((glam::Vec3::from_array(vertex.normal).length() - 1.0).abs() < 1e-6);
    }
}

#[test]
fn external_resources_and_unsupported_required_extensions_report_errors() {
    let mut value = asset();
    value["buffers"][0]["uri"] = json!("geometry.bin");
    assert!(
        load(&value)
            .err()
            .unwrap()
            .contains("resolver is unavailable")
    );
    let mut value = asset();
    value["extensionsRequired"] = json!(["EXT_bed_unknown_required"]);
    value["extensionsUsed"] = value["extensionsRequired"].clone();
    assert!(load(&value).is_err());
    assert!(model::load(b"invalid gltf").is_err());
    assert!(model::load(&CUBE_GLB[..CUBE_GLB.len() - 4]).is_err());
}

#[test]
fn invalid_binary_spans_attribute_types_and_node_cycles_are_rejected() {
    let mut value = asset();
    value["bufferViews"][0]["byteOffset"] = json!(1_000_000);
    assert!(load(&value).is_err());
    let mut value = asset();
    value["accessors"][0]["count"] = json!(0);
    assert!(load(&value).is_err());
    let mut value = asset();
    value["accessors"][0]["componentType"] = json!(5123);
    assert!(load(&value).is_err());
    let mut value = asset();
    value["accessors"][3]["componentType"] = json!(5126);
    assert!(load(&value).is_err());
    let mut value = asset();
    value["nodes"][0]["children"] = json!([0]);
    assert!(load(&value).is_err());
}

#[test]
fn draco_decodes_all_littlest_tokyo_primitives() {
    let bytes = tokyo_bytes();
    let prepared = super::prepare::load(&bytes).unwrap();
    assert_eq!(prepared.draco_primitives, 71);
    let points: usize = prepared.meshes.values().map(|m| m.count_vertices()).sum();
    let indices: usize = prepared
        .meshes
        .values()
        .map(|m| m.indices().unwrap().len())
        .sum();
    assert_eq!(points, 193_125);
    assert_eq!(indices, 425_406);
    let scene = model::load(&bytes).unwrap();
    assert_eq!(scene.draco_primitives, 71);
    assert_eq!(scene.posed_meshes, 8);
    assert_eq!(scene.triangles(), 141_802);
    assert_eq!(scene.animations, 1);
    assert_eq!(scene.images.len(), 4);
    assert!(scene.radius().is_finite());
}

#[test]
fn local_sidecars_resolve_once_and_preserve_the_scene() {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let mut source = asset();
    let uri = source["buffers"][0]["uri"].as_str().unwrap();
    let bytes = STANDARD.decode(uri.split_once(',').unwrap().1).unwrap();
    source["buffers"][0]["uri"] = json!("../geometry%20data.bin");
    let mut requested = Vec::new();
    let prepared = super::prepare::load_with(&serde_json::to_vec(&source).unwrap(), &mut |path| {
        requested.push(path.to_path_buf());
        Ok(bytes.clone())
    })
    .unwrap();
    assert_eq!(
        requested,
        vec![std::path::PathBuf::from("../geometry data.bin")]
    );
    assert_eq!(
        super::bevy_scene::load_prepared_snapshot(&prepared)
            .unwrap()
            .triangles(),
        12
    );
    let error = super::prepare::load_with(&serde_json::to_vec(&source).unwrap(), &mut |_| {
        Err("Missing geometry".into())
    })
    .err()
    .unwrap();
    assert_eq!(error, "Missing geometry");
}
#[test]
fn sidecar_boundary_rejects_urls_absolute_paths_and_invalid_escapes() {
    for uri in [
        "https://example.com/mesh.bin",
        "file:///tmp/mesh.bin",
        "/tmp/mesh.bin",
        "%2Ftmp/x",
        "C:/mesh.bin",
        "//server/x",
        "../x%00.bin",
        "bad%",
        "bad%gg",
        "a\\b",
        "a?query",
        "a#fragment",
    ] {
        assert!(relative_resource(uri).is_err(), "{uri}");
    }
    assert_eq!(
        relative_resource("../textures/a%20b.png").unwrap(),
        std::path::PathBuf::from("../textures/a b.png")
    );
}
#[test]
fn saved_view_state_round_trips_and_rejects_nonfinite_or_unbounded_values() {
    let mut view = ViewState::default();
    view.camera = Some(OrbitCamera::default());
    let serialized = serde_json::to_vec(&view).unwrap();
    let restored: ViewState = serde_json::from_slice(&serialized).unwrap();
    assert_eq!(view, restored);
    restored.validate().unwrap();
    view.appearance.exposure = f32::NAN;
    assert!(view.validate().is_err());
    view.appearance = Appearance::default();
    view.camera.as_mut().unwrap().distance = -1.0;
    assert!(view.validate().is_err());
}

#[test]
fn views_isolate_layers_camera_appearance_and_linked_cleanup() {
    use bevy::{camera::visibility::RenderLayers, prelude::*};
    let (mut app, _) = crate::bevy_scene::headless_app();
    let unrelated = app.world_mut().spawn(RenderLayers::layer(1)).id();
    let content = Arc::new(
        PreparedModel::prepare(CUBE_GLB, Path::new("cube.glb"), |_| unreachable!()).unwrap(),
    );
    let mut entities = Vec::new();
    for _ in 0..2 {
        let mut scene = ModelScene::default();
        scene.set_model(content.clone());
        entities.push(app.world_mut().spawn(scene).id());
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !entities
        .iter()
        .all(|&entity| app.world().get::<ModelScene>(entity).unwrap().is_ready())
    {
        app.update();
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    let first = app
        .world()
        .get::<ModelScene>(entities[0])
        .unwrap()
        .active
        .as_ref()
        .unwrap();
    let second = app
        .world()
        .get::<ModelScene>(entities[1])
        .unwrap()
        .active
        .as_ref()
        .unwrap();
    let (first_root, first_camera, second_camera) = (first.root, first.camera, second.camera);
    let a = app.world().get::<RenderLayers>(first_camera).unwrap();
    let b = app.world().get::<RenderLayers>(second_camera).unwrap();
    assert!(!a.intersects(b));
    assert!(!a.intersects(&RenderLayers::layer(0)));
    assert!(!a.intersects(&RenderLayers::layer(1)));
    assert!(!app.world().get::<Camera>(first_camera).unwrap().is_active);
    for &entity in &entities {
        let mut viewport = app.world_mut().get_mut::<UiViewport>(entity).unwrap();
        viewport.visible = true;
        viewport.physical_size = UVec2::new(140, 700);
    }
    app.world_mut()
        .get_mut::<ModelScene>(entities[0])
        .unwrap()
        .view
        .appearance
        .display = DisplayMode::Wireframe;
    app.update();
    assert!(app.world().get::<Camera>(first_camera).unwrap().is_active);
    assert_eq!(
        app.world()
            .get::<ModelScene>(entities[1])
            .unwrap()
            .view
            .appearance
            .display,
        DisplayMode::Shaded
    );
    let root_transform = app.world().get::<GlobalTransform>(first_camera).unwrap();
    assert!(root_transform.translation().is_finite());
    app.world_mut()
        .get_mut::<UiViewport>(entities[0])
        .unwrap()
        .visible = false;
    app.update();
    assert!(!app.world().get::<Camera>(first_camera).unwrap().is_active);
    app.world_mut()
        .entity_mut(entities[0])
        .remove::<ModelScene>();
    app.update();
    assert!(app.world().get_entity(first_root).is_err());
    assert!(app.world().get_entity(first_camera).is_err());
    assert!(app.world().get::<UiViewport>(entities[0]).is_none());
    assert!(app.world().get_entity(second_camera).is_ok());
    assert!(app.world().get_entity(unrelated).is_ok());
    app.world_mut().despawn(entities[1]);
    app.update();
    assert!(app.world().get_entity(second_camera).is_err());
}

#[test]
fn layered_imports_preserve_authored_parents_and_normalized_render_bounds() {
    use bevy::prelude::*;
    let mut source = asset();
    source["nodes"] = json!([
        {"translation":[4.0,0.0,0.0],"children":[1]},
        {"translation":[0.0,2.0,0.0],"mesh":0}
    ]);
    let prepared = Arc::new(
        PreparedModel::prepare(
            &serde_json::to_vec(&source).unwrap(),
            Path::new("nested.gltf"),
            |_| unreachable!(),
        )
        .unwrap(),
    );
    let (mut app, _) = crate::bevy_scene::headless_app();
    let mut scene = ModelScene::default();
    scene.set_model(prepared);
    let view = app.world_mut().spawn(scene).id();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !app.world().get::<ModelScene>(view).unwrap().is_ready() {
        app.update();
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    app.update();
    let info = app.world().get::<ModelScene>(view).unwrap().info().unwrap();
    assert_eq!(info.minimum, [3.0, 1.0, -1.0]);
    assert_eq!(info.maximum, [5.0, 3.0, 1.0]);
    let surfaces: Vec<_> = app
        .world_mut()
        .query_filtered::<&GlobalTransform, (With<Mesh3d>, Without<bevy::light::NotShadowCaster>)>()
        .iter(app.world())
        .copied()
        .collect();
    assert!(!surfaces.is_empty());
    for transform in surfaces {
        assert!(
            transform.translation().length() < 0.001,
            "Authored translations are canceled by the instance normalization"
        );
        assert!(
            (transform.to_scale_rotation_translation().0.x - 1.0 / 3.0f32.sqrt()).abs() < 0.001
        );
    }
}

#[test]
fn external_texture_pixels_reach_bevy_without_companion_file_loading() {
    use image::ImageEncoder;
    let mut source = asset();
    source["images"] = json!([{"uri":"textures/albedo%20color.png"}]);
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(&[20, 210, 55, 255], 1, 1, image::ExtendedColorType::Rgba8)
        .unwrap();
    let prepared = prepare::load_with(&serde_json::to_vec(&source).unwrap(), &mut |path| {
        assert_eq!(path, Path::new("textures/albedo color.png"));
        Ok(png.clone())
    })
    .unwrap();
    let scene = bevy_scene::load_prepared_snapshot(&prepared).unwrap();
    assert_eq!(scene.images.len(), 1);
    assert_eq!(scene.images[0].rgba.as_ref(), [20, 210, 55, 255]);
}
