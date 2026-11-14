//! Bounded STL import from document snapshots, using stl_io's parser.
//!
//! Triangle expansion is adapted from bevy_stl 0.18.0 (MIT); see
//! licenses/bevy_stl/LICENSE for its original notice.
#[cfg(test)]
use crate::model::Material;
use crate::model::{MAX_INDICES, MAX_RESOURCE_BYTES, MAX_VERTICES, Primitive, Scene, Vertex};
use bevy::{
    asset::RenderAssetUsages,
    mesh::{Indices, Mesh},
    render::render_resource::PrimitiveTopology,
};
use glam::Vec3;
use std::{borrow::Cow, io::Cursor};

const MAX_FACETS: usize = MAX_VERTICES / 3;

pub(super) struct PreparedStl {
    pub mesh: Mesh,
    pub inspection: Scene,
}

#[cfg(test)]
pub(super) fn load(bytes: &[u8]) -> Result<Scene, String> {
    Ok(prepare(bytes)?.inspection)
}

pub(super) fn prepare(bytes: &[u8]) -> Result<PreparedStl, String> {
    // Expand each facet to three vertices. Bound that allocation
    // before parsing, including binary files whose header starts with "solid".
    let facets = validate_envelope(bytes)?;
    // The binary envelope takes precedence over its arbitrary header. stl_io
    // probes the first line for "solid ", which can also occur in binary files.
    let binary = bytes.get(80..84).is_some_and(|count| {
        let count = u32::from_le_bytes(count.try_into().unwrap()) as usize;
        count.checked_mul(50).and_then(|size| size.checked_add(84)) == Some(bytes.len())
    });
    let bytes = if binary && bytes.starts_with(b"solid ") {
        let mut masked = bytes.to_vec();
        masked[0] = b'_';
        Cow::Owned(masked)
    } else {
        Cow::Borrowed(bytes)
    };
    let stl = stl_io::read_stl(&mut Cursor::new(bytes))
        .map_err(|error| format!("Could not load STL: {error}"))?;
    if stl.faces.len() != facets {
        return Err("STL contains incomplete or inconsistent triangle geometry".into());
    }
    let indices: Vec<u32> = (0..facets as u32 * 3).collect();

    let mut minimum = Vec3::splat(f32::INFINITY);
    let mut maximum = Vec3::splat(f32::NEG_INFINITY);
    let mut vertices = Vec::with_capacity(facets * 3);
    for face in &stl.faces {
        let normal = Vec3::from_array(face.normal.0);
        for &index in &face.vertices {
            let position = Vec3::from_array(stl.vertices[index].0);
            if !position.is_finite() || !normal.is_finite() {
                return Err("STL contains non-finite vertex or normal data".into());
            }
            minimum = minimum.min(position);
            maximum = maximum.max(position);
            vertices.push(Vertex {
                position: position.to_array(),
                normal: normal.as_dvec3().normalize_or_zero().as_vec3().to_array(),
                #[cfg(test)]
                uv0: [0.0; 2],
                #[cfg(test)]
                uv1: [0.0; 2],
                #[cfg(test)]
                tangent: [0.0; 4],
                #[cfg(test)]
                color: [1.0; 4],
            });
        }
    }
    for triangle in indices.as_chunks::<3>().0 {
        // CAD exporters commonly write zero facet normals. Recompute those
        // from winding while retaining separate vertices at every hard edge.
        let [a, b, c] =
            triangle.map(|index| Vec3::from_array(vertices[index as usize].position).as_dvec3());
        let geometric = (b - a).cross(c - a).normalize_or_zero().as_vec3();
        let fallback = if geometric == Vec3::ZERO {
            Vec3::Y
        } else {
            geometric
        };
        for &index in triangle {
            let vertex = &mut vertices[index as usize];
            if vertex.normal == [0.0; 3] {
                vertex.normal = fallback.to_array();
            }
        }
    }
    let extent = maximum - minimum;
    if !extent.is_finite() || extent.length() > 1e15 || !(minimum + maximum).is_finite() {
        return Err("STL bounds are too large for a stable preview".into());
    }
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    );
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_POSITION,
        vertices.iter().map(|v| v.position).collect::<Vec<_>>(),
    );
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_NORMAL,
        vertices.iter().map(|v| v.normal).collect::<Vec<_>>(),
    );
    mesh.insert_indices(Indices::U32(indices.clone()));
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, vec![[0.0_f32; 2]; vertices.len()]);
    Ok(PreparedStl {
        mesh,
        inspection: Scene {
            primitives: vec![Primitive {
                vertices,
                indices,
                #[cfg(test)]
                material: Material {
                    color: [0.65, 0.65, 0.65, 1.0],
                    texture: None,
                    metallic: 0.0,
                    roughness: 0.65,
                    metallic_roughness_texture: None,
                    normal_texture: None,
                    normal_scale: 1.0,
                    occlusion_texture: None,
                    occlusion_strength: 1.0,
                    emissive: [0.0; 3],
                    emissive_texture: None,
                    alpha: gltf::material::AlphaMode::Opaque,
                    cutoff: 0.5,
                    double_sided: false,
                    unlit: false,
                },
                #[cfg(test)]
                has_tangents: false,
            }],
            #[cfg(test)]
            images: vec![],
            textures: 0,
            minimum,
            maximum,
            nodes: 1,
            animations: 0,
            draco_primitives: 0,
            meshopt_views: 0,
            #[cfg(test)]
            source_bytes: None,
            posed_meshes: 0,
        },
    })
}

fn geometry_limit() -> String {
    "STL exceeds the geometry budget (1,000,000 vertices / 3,000,000 indices)".into()
}

fn validate_facet_count(facets: usize) -> Result<usize, String> {
    if facets == 0 {
        return Err("STL contains no triangle facets".into());
    }
    if facets > MAX_FACETS || facets > MAX_INDICES / 3 {
        return Err(geometry_limit());
    }
    Ok(facets)
}

fn validate_envelope(bytes: &[u8]) -> Result<usize, String> {
    if bytes.len() > MAX_RESOURCE_BYTES {
        return Err("STL exceeds the 64 MiB resource limit".into());
    }
    let binary_count = bytes
        .get(80..84)
        .map(|count| u32::from_le_bytes(count.try_into().expect("four-byte facet count")) as usize);
    if let Some(facets) = binary_count {
        let expected = facets.checked_mul(50).and_then(|size| size.checked_add(84));
        if expected == Some(bytes.len()) {
            return validate_facet_count(facets);
        }
    }
    // This only validates the allocation envelope and document boundaries;
    // stl_io parses all facet syntax and numeric data below.
    let ascii = std::str::from_utf8(bytes)
        .ok()
        .filter(|text| !text.bytes().any(|byte| byte == 0) && text.starts_with("solid "));
    if let Some(text) = ascii {
        let mut lines = text
            .lines()
            .filter_map(|line| line.split_whitespace().next());
        if lines.next() != Some("solid") {
            return Err("Invalid ASCII STL header".into());
        }
        let mut facets = 0;
        let mut closed = false;
        for token in lines {
            if closed {
                return Err("ASCII STL contains content after its closing endsolid".into());
            }
            if token == "endsolid" {
                closed = true;
            }
            if token == "facet" {
                facets += 1;
                validate_facet_count(facets)?;
            }
        }
        if !closed {
            return Err("ASCII STL is missing its closing endsolid".into());
        }
        return validate_facet_count(facets);
    }
    if let Some(facets) = binary_count {
        validate_facet_count(facets)?;
        return Err("Binary STL length does not match its declared facet count".into());
    }
    Err("File is not a complete ASCII or binary STL".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ASCII: &str = "solid triangle\n\
facet normal 0 0 1\n\
outer loop\n\
vertex 0 0 0\n\
vertex 1 0 0\n\
vertex 0 1 0\n\
endloop\n\
endfacet\n\
endsolid triangle\n";

    fn binary(header: &[u8], normal: [f32; 3], positions: [[f32; 3]; 3]) -> Vec<u8> {
        let mut bytes = vec![0; 80];
        bytes[..header.len()].copy_from_slice(header);
        bytes.extend_from_slice(&1_u32.to_le_bytes());
        for value in normal.into_iter().chain(positions.into_iter().flatten()) {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        bytes
    }

    fn triangle_binary() -> Vec<u8> {
        binary(
            b"solid binary triangle",
            [0.0, 0.0, 1.0],
            [[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        )
    }

    fn error(bytes: &[u8]) -> String {
        load(bytes).err().expect("invalid STL should fail")
    }

    #[test]
    fn ascii_and_binary_solid_header_load_neutral_pbr_geometry() {
        let binary = triangle_binary();
        let binary_newline = binary_with_solid_newline();
        for bytes in [
            ASCII.as_bytes(),
            binary.as_slice(),
            binary_newline.as_slice(),
        ] {
            let prepared = prepare(bytes).unwrap();
            assert_eq!(
                prepared.mesh.primitive_topology(),
                PrimitiveTopology::TriangleList
            );
            assert_eq!(
                prepared.mesh.indices().unwrap().iter().collect::<Vec<_>>(),
                [0, 1, 2]
            );
            assert!(matches!(
                prepared.mesh.attribute(Mesh::ATTRIBUTE_POSITION),
                Some(bevy::mesh::VertexAttributeValues::Float32x3(positions))
                    if positions == &[[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]
            ));
            let scene = prepared.inspection;
            assert_eq!(scene.triangles(), 1);
            assert_eq!(scene.minimum, Vec3::ZERO);
            assert_eq!(scene.maximum, Vec3::new(1.0, 1.0, 0.0));
            let primitive = &scene.primitives[0];
            assert_eq!(primitive.indices, [0, 1, 2]);
            assert_eq!(primitive.vertices[1].position, [1.0, 0.0, 0.0]);
            assert!(
                primitive
                    .vertices
                    .iter()
                    .all(|vertex| vertex.normal == [0.0, 0.0, 1.0])
            );
            assert_eq!(primitive.material.color, [0.65, 0.65, 0.65, 1.0]);
            assert_eq!(primitive.material.metallic, 0.0);
            assert_eq!(primitive.material.roughness, 0.65);
            assert!(!primitive.material.unlit);
            assert_eq!(scene.animations, 0);
            assert!(scene.images.is_empty());
        }
    }

    fn binary_with_solid_newline() -> Vec<u8> {
        binary(
            b"solid binary triangle\n",
            [0.0, 0.0, 1.0],
            [[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        )
    }

    #[test]
    fn missing_and_non_unit_facet_normals_are_repaired() {
        for normal in [[0.0; 3], [0.0, 0.0, 100.0], [0.0, 0.0, f32::MAX]] {
            let bytes = binary(
                b"normals",
                normal,
                [[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            );
            let prepared = prepare(&bytes).unwrap();
            assert!(matches!(
                prepared.mesh.attribute(Mesh::ATTRIBUTE_NORMAL),
                Some(bevy::mesh::VertexAttributeValues::Float32x3(normals))
                    if normals == &[[0.0, 0.0, 1.0]; 3]
            ));
            let scene = prepared.inspection;
            assert!(
                scene.primitives[0]
                    .vertices
                    .iter()
                    .all(|vertex| vertex.normal == [0.0, 0.0, 1.0])
            );
        }
        let ascii = ASCII.replace("normal 0 0 1", "normal 0 0 0");
        assert_eq!(
            load(ascii.as_bytes()).unwrap().primitives[0].vertices[0].normal,
            [0.0, 0.0, 1.0]
        );
    }

    #[test]
    fn empty_malformed_and_truncated_stl_is_rejected() {
        for bytes in [
            b"".as_slice(),
            b"not an stl",
            b"solid empty\nendsolid empty\n",
            b"solid broken\nfacet normal 0 0 1\nendsolid broken\n",
        ] {
            assert!(!error(bytes).is_empty());
        }
        let mut binary = triangle_binary();
        binary.pop();
        assert!(error(&binary).contains("length"));
        assert!(error(ASCII.replace("endsolid triangle", "").as_bytes()).contains("endsolid"));
        assert!(error(format!("{ASCII}garbage\nendsolid triangle\n").as_bytes()).contains("after"));
        assert!(
            error(
                ASCII
                    .replace("vertex 1 0 0", "vertex broken 0 0")
                    .as_bytes()
            )
            .contains("load STL")
        );
    }

    #[test]
    fn nonfinite_vertex_and_normal_data_is_rejected() {
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut bytes = triangle_binary();
            bytes[96..100].copy_from_slice(&value.to_le_bytes());
            assert!(load(&bytes).is_err());
            let mut bytes = triangle_binary();
            bytes[84..88].copy_from_slice(&value.to_le_bytes());
            assert!(load(&bytes).is_err());
        }
        assert!(load(ASCII.replace("vertex 1 0 0", "vertex NaN 0 0").as_bytes()).is_err());
    }

    #[test]
    fn declared_binary_and_ascii_facet_budgets_are_checked_before_loading() {
        let mut bytes = vec![0; 80];
        bytes.extend_from_slice(&((MAX_FACETS + 1) as u32).to_le_bytes());
        assert!(error(&bytes).contains("geometry budget"));
        for line in ["facet\n", "\u{2003}facet\n"] {
            let ascii = format!(
                "solid large\n{}endsolid large\n",
                line.repeat(MAX_FACETS + 1)
            );
            assert!(error(ascii.as_bytes()).contains("geometry budget"));
        }
        assert!(error(&vec![0; MAX_RESOURCE_BYTES + 1]).contains("64 MiB"));
    }

    #[test]
    fn binary_empty_count_trailing_bytes_and_unstable_bounds_are_rejected() {
        assert!(error(&[0; 84]).contains("no triangle"));
        let mut bytes = triangle_binary();
        bytes.push(0);
        assert!(error(&bytes).contains("length"));
        let bytes = binary(
            b"huge",
            [0.0, 0.0, 1.0],
            [[0.0; 3], [1e20, 0.0, 0.0], [0.0, 1.0, 0.0]],
        );
        assert!(error(&bytes).contains("bounds"));
    }
}
