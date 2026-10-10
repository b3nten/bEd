//! Mesh adapters only for encodings Bevy cannot load directly. The document,
//! hierarchy, materials, normal/tangent generation and skinning stay with Bevy.
use crate::model::{MAX_INDICES, MAX_VERTICES};
use bevy::{
    asset::RenderAssetUsages,
    mesh::{Indices, Mesh, PrimitiveTopology, VertexAttributeValues},
};
use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
};

pub(super) type Meshes = BTreeMap<(usize, usize), Mesh>;

pub(super) fn validate_attributes(
    document: &gltf::Document,
    quantized: bool,
) -> Result<(), String> {
    use gltf::accessor::{DataType, Dimensions};
    for mesh in document.meshes() {
        for primitive in mesh.primitives() {
            if primitive.morph_targets().next().is_some() {
                return Err("Morph targets are not supported by the static glTF preview".into());
            }
            if let Some(a) = primitive.indices() {
                if !matches!(a.data_type(), DataType::U8 | DataType::U16 | DataType::U32)
                    || a.dimensions() != Dimensions::Scalar
                    || a.normalized()
                {
                    return Err("Mesh indices must be unsigned scalar integers".into());
                }
                if a.count() > MAX_INDICES {
                    return Err("glTF exceeds the 3,000,000-index limit".into());
                }
            }
            for (semantic, accessor) in primitive.attributes() {
                let component = accessor.data_type();
                let dimensions = accessor.dimensions();
                let norm = accessor.normalized();
                let small_integer = matches!(
                    component,
                    DataType::I8 | DataType::U8 | DataType::I16 | DataType::U16
                );
                let valid = match semantic {
                    gltf::Semantic::Extras(_) => true,
                    gltf::Semantic::Positions => {
                        dimensions == Dimensions::Vec3
                            && (component == DataType::F32 && !norm || quantized && small_integer)
                    }
                    gltf::Semantic::Normals => {
                        dimensions == Dimensions::Vec3
                            && (component == DataType::F32 && !norm
                                || quantized
                                    && norm
                                    && matches!(component, DataType::I8 | DataType::I16))
                    }
                    gltf::Semantic::Tangents => {
                        dimensions == Dimensions::Vec4
                            && (component == DataType::F32 && !norm
                                || quantized
                                    && norm
                                    && matches!(component, DataType::I8 | DataType::I16))
                    }
                    gltf::Semantic::TexCoords(_) => {
                        // Bevy ignores unused UV channels beyond UV1. Material
                        // references are checked separately at the boundary.
                        dimensions == Dimensions::Vec2
                            && (component == DataType::F32 && !norm
                                || norm && matches!(component, DataType::U8 | DataType::U16)
                                || quantized && small_integer)
                    }
                    gltf::Semantic::Colors(_) => {
                        // Bevy consumes COLOR_0 and ignores other color sets.
                        matches!(dimensions, Dimensions::Vec3 | Dimensions::Vec4)
                            && (component == DataType::F32 && !norm
                                || norm && matches!(component, DataType::U8 | DataType::U16))
                    }
                    gltf::Semantic::Joints(set) => {
                        set == 0
                            && dimensions == Dimensions::Vec4
                            && !norm
                            && matches!(component, DataType::U8 | DataType::U16)
                    }
                    gltf::Semantic::Weights(set) => {
                        set == 0
                            && dimensions == Dimensions::Vec4
                            && (component == DataType::F32 && !norm
                                || norm && matches!(component, DataType::U8 | DataType::U16))
                    }
                };
                if !valid {
                    return Err(
                        "Mesh attribute has an unsupported component type or dimensions".into(),
                    );
                }
                if accessor.count() > MAX_VERTICES {
                    return Err("glTF vertex attributes exceed the 1,000,000-vertex limit".into());
                }
            }
        }
    }
    Ok(())
}

pub(super) fn prepare(
    document: &gltf::Document,
    buffers: &[Cow<'_, [u8]>],
    quantized: bool,
    budget: usize,
) -> Result<(Meshes, usize), String> {
    validate_attributes(document, quantized)?;
    let skinned: BTreeSet<_> = document
        .nodes()
        .filter(|n| n.skin().is_some())
        .filter_map(|n| n.mesh().map(|m| m.index()))
        .collect();
    let (mut meshes, draco_count) = crate::draco::meshes(document, buffers, budget)?;
    let mut adapted_bytes: usize = meshes
        .values()
        .map(|mesh| mesh.get_vertex_buffer_size() + mesh.indices().map_or(0, |i| i.len() * 4))
        .sum();
    for source in document.meshes() {
        for primitive in source.primitives() {
            let key = (source.index(), primitive.index());
            let skin = skinned.contains(&source.index());
            if let Some(mesh) = meshes.get_mut(&key) {
                finish(mesh, skin)?;
                continue;
            }
            let normalize_weights = if skin {
                primitive
                    .get(&gltf::Semantic::Weights(0))
                    .map(|a| crate::prepare::float_values(&a, buffers))
                    .transpose()?
                    .is_some_and(|values| {
                        values.as_chunks::<4>().0.iter().any(|v| {
                            let sum: f32 = v.iter().sum();
                            !sum.is_finite()
                                || (sum - 1.0).abs() > 1e-6
                                || v.iter().any(|w| *w < 0.0)
                        })
                    })
            } else {
                false
            };
            let needs_adapter = primitive.mode() != gltf::mesh::Mode::Triangles
                || normalize_weights
                || primitive.attributes().any(|(semantic, a)| {
                    a.data_type() != gltf::accessor::DataType::F32
                        && matches!(
                            semantic,
                            gltf::Semantic::Positions
                                | gltf::Semantic::Normals
                                | gltf::Semantic::Tangents
                        )
                        || matches!(semantic, gltf::Semantic::TexCoords(0 | 1))
                            && (matches!(
                                a.data_type(),
                                gltf::accessor::DataType::I8 | gltf::accessor::DataType::I16
                            ) || a.data_type() != gltf::accessor::DataType::F32
                                && !a.normalized())
                });
            let tangents = primitive
                .get(&gltf::Semantic::Tangents)
                .map(|a| crate::prepare::float_values(&a, buffers))
                .transpose()?;
            if let Some(values) = &tangents
                && values
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .any(|v| ![-1.0, 1.0].contains(&v[3]))
            {
                return Err("Mesh contains invalid tangent data".into());
            }
            let bad_tangents = tangents.as_ref().is_some_and(|values| {
                values
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .any(|v| glam::Vec3::from_slice(&v[..3]).length_squared() == 0.0)
            }) || tangents.is_some()
                && match primitive.get(&gltf::Semantic::Normals) {
                    Some(a) => crate::prepare::float_values(&a, buffers)?
                        .as_chunks::<3>()
                        .0
                        .iter()
                        .any(|v| glam::Vec3::from_slice(v).length_squared() == 0.0),
                    None => true,
                };
            if !needs_adapter && !bad_tangents {
                continue;
            }
            let count = primitive
                .get(&gltf::Semantic::Positions)
                .ok_or("Mesh has no readable POSITION attribute")?
                .count();
            let source_indices = primitive.indices().map_or(count, |a| a.count());
            let index_count = if primitive.mode() == gltf::mesh::Mode::Triangles {
                source_indices
            } else {
                source_indices.saturating_sub(2) * 3
            };
            let vertex_bytes: usize = primitive
                .attributes()
                .filter(|(s, _)| supported(s))
                .map(|(s, a)| {
                    a.count()
                        * if s == gltf::Semantic::Colors(0) {
                            4
                        } else {
                            a.dimensions().multiplicity()
                        }
                        * 4
                })
                .sum();
            adapted_bytes = adapted_bytes
                .checked_add(vertex_bytes)
                .and_then(|b| b.checked_add(index_count * 4))
                .filter(|b| *b <= budget)
                .ok_or("Expanded glTF geometry exceeds the 64 MiB limit")?;
            let mut mesh = Mesh::new(
                PrimitiveTopology::TriangleList,
                RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
            );
            for (semantic, accessor) in primitive.attributes() {
                if supported(&semantic) {
                    insert_attribute(
                        &mut mesh,
                        semantic,
                        accessor.dimensions().multiplicity(),
                        crate::prepare::float_values(&accessor, buffers)?,
                    )?;
                }
            }
            let reader = primitive.reader(|b| buffers.get(b.index()).map(|b| b.as_ref()));
            let indices: Vec<u32> = reader
                .read_indices()
                .map(|i| i.into_u32().collect())
                .unwrap_or_else(|| (0..count as u32).collect());
            if indices.iter().any(|i| *i as usize >= count) {
                return Err("Mesh index is outside the vertex buffer".into());
            }
            mesh.insert_indices(Indices::U32(crate::model::triangles(
                indices,
                primitive.mode(),
            )?));
            finish(&mut mesh, skin)?;
            meshes.insert(key, mesh);
        }
    }
    Ok((meshes, draco_count))
}

pub(super) fn supported(semantic: &gltf::Semantic) -> bool {
    matches!(
        semantic,
        gltf::Semantic::Positions
            | gltf::Semantic::Normals
            | gltf::Semantic::Tangents
            | gltf::Semantic::TexCoords(0 | 1)
            | gltf::Semantic::Colors(0)
            | gltf::Semantic::Joints(0)
            | gltf::Semantic::Weights(0)
    )
}

pub(super) fn insert_attribute(
    mesh: &mut Mesh,
    semantic: gltf::Semantic,
    dimensions: usize,
    values: Vec<f32>,
) -> Result<(), String> {
    if values.iter().any(|v| !v.is_finite()) {
        return Err("Mesh contains non-finite vertex data".into());
    }
    let vec2 = || values.as_chunks::<2>().0.to_vec();
    let vec3 = || values.as_chunks::<3>().0.to_vec();
    let vec4 = || values.as_chunks::<4>().0.to_vec();
    match semantic {
        gltf::Semantic::Positions => mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, vec3()),
        gltf::Semantic::Normals => mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec3()),
        gltf::Semantic::Tangents => mesh.insert_attribute(Mesh::ATTRIBUTE_TANGENT, vec4()),
        gltf::Semantic::TexCoords(0) => mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, vec2()),
        gltf::Semantic::TexCoords(1) => mesh.insert_attribute(Mesh::ATTRIBUTE_UV_1, vec2()),
        gltf::Semantic::Colors(0) => mesh.insert_attribute(
            Mesh::ATTRIBUTE_COLOR,
            values
                .chunks_exact(dimensions)
                .map(|v| [v[0], v[1], v[2], if dimensions == 4 { v[3] } else { 1.0 }])
                .collect::<Vec<_>>(),
        ),
        gltf::Semantic::Joints(0) => mesh.insert_attribute(
            Mesh::ATTRIBUTE_JOINT_INDEX,
            VertexAttributeValues::Uint16x4(
                values
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|v| [v[0] as u16, v[1] as u16, v[2] as u16, v[3] as u16])
                    .collect(),
            ),
        ),
        gltf::Semantic::Weights(0) => mesh.insert_attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT, vec4()),
        _ => {}
    }
    Ok(())
}

fn finish(mesh: &mut Mesh, skinned: bool) -> Result<(), String> {
    if !skinned {
        mesh.remove_attribute(Mesh::ATTRIBUTE_JOINT_INDEX);
        mesh.remove_attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT);
    } else if let Some(VertexAttributeValues::Float32x4(values)) =
        mesh.attribute_mut(Mesh::ATTRIBUTE_JOINT_WEIGHT)
    {
        for weights in values {
            let sum: f32 = weights.iter().sum();
            if !sum.is_finite() || sum <= 0.0 || weights.iter().any(|v| *v < 0.0) {
                return Err(
                    "Skin weights must be finite, nonnegative and have a positive sum".into(),
                );
            }
            for weight in weights {
                *weight /= sum;
            }
        }
    }
    if let Some(VertexAttributeValues::Float32x4(values)) = mesh.attribute(Mesh::ATTRIBUTE_TANGENT)
    {
        if values.iter().any(|v| ![-1.0, 1.0].contains(&v[3])) {
            return Err("Mesh contains invalid tangent data".into());
        }
        let valid = values
            .iter()
            .all(|v| glam::Vec3::from_slice(&v[..3]).length_squared() > 0.0)
            && match mesh.attribute(Mesh::ATTRIBUTE_NORMAL) {
                Some(VertexAttributeValues::Float32x3(values)) => values
                    .iter()
                    .all(|v| glam::Vec3::from_slice(v).length_squared() > 0.0),
                _ => false,
            };
        if !valid {
            mesh.remove_attribute(Mesh::ATTRIBUTE_TANGENT);
        }
    }
    Ok(())
}
