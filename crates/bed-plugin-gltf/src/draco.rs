//! Bounded Draco decoding directly into Bevy meshes, without rebuilding glTF.
use crate::mesh_compat::Meshes;
use crate::model::{MAX_INDICES, MAX_VERTICES};
use bevy::{
    asset::RenderAssetUsages,
    mesh::{Indices, Mesh as BevyMesh, PrimitiveTopology},
};
use draco_core::{DataType, DecodeLimits, DecoderBuffer, Mesh, MeshDecoder, PointIndex};
use gltf::json::{self, Index, validation::Checked};
use std::{borrow::Cow, collections::BTreeSet};

pub(super) const EXTENSION: &str = "KHR_draco_mesh_compression";

pub(super) fn decode(bytes: &[u8], limits: DecodeLimits) -> Result<Mesh, String> {
    let mut input = DecoderBuffer::new(bytes).with_limits(limits);
    let mut mesh = Mesh::new();
    MeshDecoder::new()
        .decode(&mut input, &mut mesh)
        .map_err(|e| e.to_string())?;
    Ok(mesh)
}

pub(super) fn meshes(
    document: &gltf::Document,
    buffers: &[Cow<'_, [u8]>],
    budget: usize,
) -> Result<(Meshes, usize), String> {
    let mut meshes = Meshes::new();
    let mut points = 0;
    let mut indices = 0;
    let mut decoded_bytes = 0;
    for source in document.meshes() {
        for primitive in source.primitives() {
            let Some(extension) = primitive.extension_value(EXTENSION) else {
                continue;
            };
            if !matches!(
                primitive.mode(),
                gltf::mesh::Mode::Triangles | gltf::mesh::Mode::TriangleStrip
            ) {
                return Err("Draco primitives must be triangles or triangle strips".into());
            }
            let view = extension["bufferView"]
                .as_u64()
                .and_then(|v| usize::try_from(v).ok())
                .and_then(|v| document.views().nth(v))
                .ok_or("Invalid Draco bufferView")?;
            let bytes = buffers
                .get(view.buffer().index())
                .and_then(|b| {
                    view.offset()
                        .checked_add(view.length())
                        .and_then(|end| b.get(view.offset()..end))
                })
                .ok_or("Draco bufferView is out of bounds")?;
            let attributes = extension["attributes"]
                .as_object()
                .ok_or("Missing Draco attribute map")?;
            let limits = DecodeLimits::default()
                .with_max_points((MAX_VERTICES - points) as u64)
                .with_max_faces(((MAX_INDICES - indices) / 3) as u64)
                .with_max_decoded_bytes(budget.saturating_sub(decoded_bytes) as u64);
            let decoded = decode(bytes, limits).map_err(|e| {
                format!(
                    "Could not decode Draco mesh {}/{}: {e}",
                    source.index(),
                    primitive.index()
                )
            })?;
            points += decoded.num_points();
            indices += decoded.num_faces() * 3;
            if decoded.num_points() == 0
                || decoded.num_faces() == 0
                || points > MAX_VERTICES
                || indices > MAX_INDICES
            {
                return Err("Draco geometry is empty or exceeds the vertex/index budget".into());
            }
            for semantic in attributes.keys() {
                if !primitive
                    .attributes()
                    .any(|(s, _)| s.to_string() == *semantic)
                {
                    return Err(
                        "Draco attribute is missing from the primitive's accessor map".into(),
                    );
                }
            }
            let mut mesh = BevyMesh::new(
                PrimitiveTopology::TriangleList,
                RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
            );
            for (semantic, accessor) in primitive.attributes() {
                let dimensions = accessor.dimensions().multiplicity();
                let values = if let Some(id) = attributes.get(&semantic.to_string()) {
                    let id = id
                        .as_u64()
                        .and_then(|v| u32::try_from(v).ok())
                        .ok_or("Invalid Draco attribute ID")?;
                    let attribute = decoded
                        .attribute_by_unique_id(id)
                        .ok_or("Draco attribute ID is missing from the decoded mesh")?;
                    let component = accessor.data_type();
                    let data_type = match component {
                        gltf::accessor::DataType::I8 => DataType::Int8,
                        gltf::accessor::DataType::U8 => DataType::Uint8,
                        gltf::accessor::DataType::I16 => DataType::Int16,
                        gltf::accessor::DataType::U16 => DataType::Uint16,
                        gltf::accessor::DataType::U32 => DataType::Uint32,
                        gltf::accessor::DataType::F32 => DataType::Float32,
                    };
                    if attribute.data_type() != data_type
                        || usize::from(attribute.num_components()) != dimensions
                        || dimensions > 4
                        || accessor.sparse().is_some()
                        || accessor.count() != decoded.num_points()
                    {
                        return Err(format!(
                            "Decoded Draco {semantic:?} does not match its accessor"
                        ));
                    }
                    let width = component.size() * dimensions;
                    let stride = usize::try_from(attribute.byte_stride())
                        .map_err(|_| "Invalid Draco attribute stride")?;
                    if stride < width {
                        return Err("Invalid Draco attribute stride".into());
                    }
                    // Rendering ignores additional UV/color sets, but declarations
                    // still have to match the compressed attribute.
                    if !crate::mesh_compat::supported(&semantic) {
                        continue;
                    }
                    decoded_bytes += decoded.num_points()
                        * if semantic == gltf::Semantic::Colors(0) {
                            4
                        } else {
                            dimensions
                        }
                        * 4;
                    if decoded_bytes > budget {
                        return Err("Expanded Draco geometry exceeds the 64 MiB limit".into());
                    }
                    let mut values = Vec::with_capacity(decoded.num_points() * dimensions);
                    for point in 0..decoded.num_points() {
                        let mapped = attribute.mapped_index(PointIndex(point as u32)).0 as usize;
                        if mapped >= attribute.size() {
                            return Err("Invalid Draco point-to-attribute mapping".into());
                        }
                        let start = mapped
                            .checked_mul(stride)
                            .ok_or("Draco attribute offset overflow")?;
                        let bytes = start
                            .checked_add(width)
                            .and_then(|end| attribute.buffer().data().get(start..end))
                            .ok_or("Draco attribute data is truncated")?;
                        values.extend(bytes.chunks_exact(component.size()).map(|v| {
                            crate::prepare::component_value(v, component, accessor.normalized())
                        }));
                    }
                    values
                } else {
                    if accessor.count() != decoded.num_points() {
                        return Err("Mesh vertex attributes have mismatched lengths".into());
                    }
                    if !crate::mesh_compat::supported(&semantic) {
                        continue;
                    }
                    decoded_bytes += decoded.num_points()
                        * if semantic == gltf::Semantic::Colors(0) {
                            4
                        } else {
                            dimensions
                        }
                        * 4;
                    if decoded_bytes > budget {
                        return Err("Expanded Draco geometry exceeds the 64 MiB limit".into());
                    }
                    crate::prepare::float_values(&accessor, buffers)?
                };
                crate::mesh_compat::insert_attribute(&mut mesh, semantic, dimensions, values)?;
            }
            if !mesh.contains_attribute(BevyMesh::ATTRIBUTE_POSITION) {
                return Err("Mesh has no readable POSITION attribute".into());
            }
            if let Some(accessor) = primitive.indices()
                && primitive.mode() == gltf::mesh::Mode::Triangles
                && accessor.count() != decoded.num_faces() * 3
            {
                return Err("Decoded Draco indices do not match their accessor count".into());
            }
            decoded_bytes += decoded.num_faces() * 12;
            if decoded_bytes > budget {
                return Err("Expanded Draco geometry exceeds the 64 MiB limit".into());
            }
            let mut faces = Vec::with_capacity(decoded.num_faces() * 3);
            for face in decoded.faces() {
                for point in face {
                    if point.0 as usize >= decoded.num_points() {
                        return Err("Draco triangle references a missing point".into());
                    }
                    faces.push(point.0);
                }
            }
            mesh.insert_indices(Indices::U32(faces));
            meshes.insert((source.index(), primitive.index()), mesh);
        }
    }
    let count = meshes.len();
    Ok((meshes, count))
}

pub(super) fn validate_source(root: &json::Root) -> Result<BTreeSet<usize>, String> {
    let mut compressed = BTreeSet::new();
    // Core validation requires bufferView on compressed-only declarations.
    // Give the validation copy a view; keep the source document intact. The
    // bounded decoder validates these declarations against the actual stream.
    let mut validation = root.clone();
    validation
        .extensions_required
        .retain(|name| json::extensions::ENABLED_EXTENSIONS.contains(&name.as_str()));
    for mesh in &root.meshes {
        for primitive in &mesh.primitives {
            for index in primitive
                .attributes
                .values()
                .chain(primitive.indices.iter())
            {
                if index.value() >= root.accessors.len() {
                    return Err("Mesh references a missing accessor".into());
                }
            }
            let Some(extension) = primitive
                .extensions
                .as_ref()
                .and_then(|e| e.others.get(EXTENSION))
            else {
                continue;
            };
            let view = extension["bufferView"]
                .as_u64()
                .and_then(|v| usize::try_from(v).ok())
                .filter(|v| *v < root.buffer_views.len())
                .ok_or("Invalid Draco bufferView")?;
            let attributes = extension["attributes"]
                .as_object()
                .ok_or("Missing Draco attribute map")?;
            for (semantic, index) in &primitive.attributes {
                if attributes.contains_key(&semantic.to_string()) {
                    compressed.insert(index.value());
                    let accessor = &mut validation.accessors[index.value()];
                    if accessor.buffer_view.is_none() {
                        accessor.buffer_view = Some(Index::new(view as u32));
                    }
                }
            }
            if let Some(index) = primitive.indices {
                compressed.insert(index.value());
                let accessor = &mut validation.accessors[index.value()];
                if !matches!(accessor.type_, Checked::Valid(json::accessor::Type::Scalar))
                    || !matches!(
                        accessor.component_type,
                        Checked::Valid(json::accessor::GenericComponentType(
                            json::accessor::ComponentType::U8
                                | json::accessor::ComponentType::U16
                                | json::accessor::ComponentType::U32
                        ))
                    )
                    || accessor.normalized
                    || accessor.sparse.is_some()
                {
                    return Err("Draco indices must declare unsigned scalar integers".into());
                }
                if accessor.buffer_view.is_none() {
                    accessor.buffer_view = Some(Index::new(view as u32));
                }
            }
        }
    }
    gltf::Document::from_json(validation).map_err(|error| error.to_string())?;
    Ok(compressed)
}
