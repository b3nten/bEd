use super::*;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

const CUBE_GLB: &[u8] = include_bytes!("../../../tests/fixtures/gltf/cube.glb");
const CUBE_GLTF: &[u8] = include_bytes!("../../../tests/fixtures/gltf/cube.gltf");
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
    assert_eq!(material.normal_scale, 0.3);
    assert_eq!(material.occlusion_strength, 0.6);
    assert_eq!(material.emissive, [0.1, 0.2, 0.3]);
    assert_eq!(material.alpha, gltf::material::AlphaMode::Mask);
    assert_eq!(material.cutoff, 0.25);
    assert!(material.double_sided);
    for info in [material.texture, material.occlusion_texture]
        .into_iter()
        .flatten()
    {
        assert_eq!(info.image, 0);
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
        assert_eq!(info.image, 0);
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
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/LittlestTokyo.glb"),
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
    assert!(load(&value).err().unwrap().contains("self-contained GLB"));
    let mut value = asset();
    value["extensionsRequired"] = json!(["KHR_mesh_quantization"]);
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
fn worker_reloads_revisions_and_closing_releases_the_output() {
    let document = DocumentId(7);
    let mut documents = [bed_plugin::PluginDocument {
        id: document,
        path: "cube.glb".into(),
        kind: DocumentKind::Bytes,
        language_id: String::new(),
        revision: (1, 1),
        dirty: false,
        bytes: CUBE_GLB.into(),
        text: None,
    }];
    let mut panel = GltfPanel::new(document, &Value::Null);
    let textures = HashMap::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        panel.update(&HostContext {
            documents: &documents,
            active_document: Some(document),
            settings: &Value::Null,
            textures: &textures,
            animations: false,
            workspace: 1,
            diagnostics: &Value::Null,
        });
        if panel.scene.is_some() {
            break;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(5));
    }
    panel.canvas = Some(CanvasState {
        pixels: [320, 240],
        camera: panel.camera,
        background: [0.0; 4],
        settings: panel.settings,
    });
    assert!(panel.render_output().unwrap().depth);
    documents[0].revision = (1, 2);
    documents[0].bytes = b"invalid".as_slice().into();
    loop {
        panel.update(&HostContext {
            documents: &documents,
            active_document: Some(document),
            settings: &Value::Null,
            textures: &textures,
            animations: false,
            workspace: 1,
            diagnostics: &Value::Null,
        });
        if panel.error.is_some() {
            break;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(5));
    }
    assert!(panel.render_output().is_none());
    panel.close(&mut Vec::new());
    assert!(panel.loader.sender.is_none());
}

#[test]
fn worker_loads_stl_snapshots_and_restores_appearance() {
    let document = DocumentId(19);
    let documents = [bed_plugin::PluginDocument {
        id: document,
        path: "ssh://example/project/part.STL".into(),
        kind: DocumentKind::Bytes,
        language_id: String::new(),
        revision: (1, 1),
        dirty: false,
        bytes: b"solid part\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid part\n".as_slice().into(),
        text: None,
    }];
    let state = json!({"render": {"lighting": 3, "display": 2, "normals": true,
        "normal_length": 0.125, "skybox": true, "skybox_blur": 0.5, "horizon": -18.0,
        "shadows": false, "ao": false, "exposure": 1.0}});
    let mut panel = GltfPanel::new(document, &state);
    let textures = HashMap::new();
    let host = HostContext {
        documents: &documents,
        active_document: Some(document),
        settings: &Value::Null,
        textures: &textures,
        animations: false,
        workspace: 1,
        diagnostics: &Value::Null,
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        panel.update(&host);
        assert!(panel.error.is_none(), "{:?}", panel.error);
        if panel.scene.is_some() {
            break;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(panel.scene.as_ref().unwrap().triangles(), 1);
    assert_eq!(panel.save_state()["render"], state["render"]);
    panel.close(&mut Vec::new());
    assert!(panel.render_output().is_none());
}

#[test]
fn registration_routes_model_formats_and_commands_use_the_captured_path() {
    let mut registry = bed_plugin::Registry::default();
    registry.register(&GltfPlugin).unwrap();
    for path in ["model.GLB", "model.gltf", "part.STL"] {
        assert_eq!(registry.viewer_for_path(path).unwrap().id, VIEWER_ID);
    }
    let textures = HashMap::new();
    let host = HostContext {
        documents: &[],
        active_document: None,
        settings: &Value::Null,
        textures: &textures,
        animations: false,
        workspace: 1,
        diagnostics: &Value::Null,
    };
    let mut requests = Vec::new();
    GltfPlugin.command(
        OPEN_FILE_COMMAND,
        &CommandContext {
            path: Some("selected.glb".into()),
            ..Default::default()
        },
        &host,
        &mut requests,
    );
    assert!(
        matches!(requests.as_slice(), [HostRequest::OpenFile { path, viewer: Some(viewer) }] if path == "selected.glb" && viewer == VIEWER_ID)
    );
}

#[test]
fn draco_decodes_all_littlest_tokyo_primitives() {
    let bytes = tokyo_bytes();
    let gltf = gltf::Gltf::from_slice_without_validation(&bytes).unwrap();
    let mut buffers = vec![std::borrow::Cow::Borrowed(gltf.blob.as_deref().unwrap())];
    let started = Instant::now();
    let (document, count) = super::draco::expand(gltf.document, &mut buffers).unwrap();
    let points: usize = document
        .meshes()
        .flat_map(|m| m.primitives())
        .map(|p| p.get(&gltf::Semantic::Positions).unwrap().count())
        .sum();
    let indices: usize = document
        .meshes()
        .flat_map(|m| m.primitives())
        .map(|p| p.indices().unwrap().count())
        .sum();
    eprintln!(
        "LittlestTokyo: {count} Draco primitives, {points} vertices, {} triangles, {:.2?} to decode",
        indices / 3,
        started.elapsed()
    );
    assert_eq!(count, 71);
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
