//! Expand compressed primitives into bounded, ordinary glTF accessors. All
//! downstream loading, transforms, materials and GPU drawing share one path.
use crate::model::{MAX_INDICES, MAX_RESOURCE_BYTES, MAX_VERTICES};
use draco_core::{DataType, DecodeLimits, DecoderBuffer, Mesh, MeshDecoder, PointIndex};
use gltf::json::{self, Index, validation::Checked};
use std::borrow::Cow;

pub(super) const EXTENSION: &str = "KHR_draco_mesh_compression";

pub(super) fn decode(bytes: &[u8], limits: DecodeLimits) -> Result<Mesh, String> {
    let mut input = DecoderBuffer::new(bytes).with_limits(limits);
    let mut mesh = Mesh::new();
    MeshDecoder::new()
        .decode(&mut input, &mut mesh)
        .map_err(|error| error.to_string())?;
    Ok(mesh)
}

pub(super) fn expand(
    document: gltf::Document,
    buffers: &mut Vec<Cow<'_, [u8]>>,
) -> Result<(gltf::Document, usize), String> {
    let mut root = document.into_json();
    validate_source(&root)?;
    let buffer_index = root.buffers.len();
    let mut expanded = Vec::new();
    let mut primitive_count = 0;
    let mut points = 0;
    let mut indices = 0;
    for mesh_index in 0..root.meshes.len() {
        for primitive_index in 0..root.meshes[mesh_index].primitives.len() {
            let mut primitive = root.meshes[mesh_index].primitives[primitive_index].clone();
            let Some(extension) = primitive
                .extensions
                .as_ref()
                .and_then(|extensions| extensions.others.get(EXTENSION))
            else {
                continue;
            };
            if !matches!(
                primitive.mode,
                Checked::Valid(json::mesh::Mode::Triangles | json::mesh::Mode::TriangleStrip)
            ) {
                return Err("Draco primitives must be triangles or triangle strips".into());
            }
            let view_index = extension["bufferView"]
                .as_u64()
                .and_then(|v| usize::try_from(v).ok())
                .ok_or("Invalid Draco bufferView")?;
            let attributes = extension["attributes"]
                .as_object()
                .ok_or("Missing Draco attribute map")?;
            let view = root
                .buffer_views
                .get(view_index)
                .ok_or("Draco bufferView is out of bounds")?;
            let buffer = buffers
                .get(view.buffer.value())
                .ok_or("Missing Draco buffer")?;
            let start = usize::try_from(view.byte_offset.unwrap_or_default().0)
                .map_err(|_| "Draco buffer offset overflow")?;
            let end = usize::try_from(view.byte_length.0)
                .ok()
                .and_then(|length| start.checked_add(length))
                .ok_or("Draco buffer range overflow")?;
            let bytes = buffer
                .get(start..end)
                .ok_or("Draco bufferView is out of bounds")?;
            let limits = DecodeLimits::default()
                .with_max_points((MAX_VERTICES - points) as u64)
                .with_max_faces(((MAX_INDICES - indices) / 3) as u64)
                .with_max_decoded_bytes((MAX_RESOURCE_BYTES - expanded.len()) as u64);
            let mesh = decode(bytes, limits).map_err(|error| {
                format!("Could not decode Draco mesh {mesh_index}/{primitive_index}: {error}")
            })?;
            points += mesh.num_points();
            indices += mesh.num_faces() * 3;
            if mesh.num_points() == 0
                || mesh.num_faces() == 0
                || points > MAX_VERTICES
                || indices > MAX_INDICES
            {
                return Err("Draco geometry is empty or exceeds the vertex/index budget".into());
            }
            for (semantic, id) in attributes {
                let id = id
                    .as_u64()
                    .and_then(|v| u32::try_from(v).ok())
                    .ok_or("Invalid Draco attribute ID")?;
                let accessor_index = primitive
                    .attributes
                    .iter()
                    .find_map(|(key, index)| (key.to_string() == *semantic).then_some(*index))
                    .ok_or("Draco attribute is missing from the primitive's accessor map")?;
                let mut accessor = root.accessors[accessor_index.value()].clone();
                let attribute = mesh
                    .attribute_by_unique_id(id)
                    .ok_or("Draco attribute ID is missing from the decoded mesh")?;
                let Checked::Valid(component) = accessor.component_type else {
                    return Err("Invalid Draco accessor component type".into());
                };
                let Checked::Valid(shape) = accessor.type_ else {
                    return Err("Invalid Draco accessor dimensions".into());
                };
                let data_type = match component.0 {
                    json::accessor::ComponentType::I8 => DataType::Int8,
                    json::accessor::ComponentType::U8 => DataType::Uint8,
                    json::accessor::ComponentType::I16 => DataType::Int16,
                    json::accessor::ComponentType::U16 => DataType::Uint16,
                    json::accessor::ComponentType::U32 => DataType::Uint32,
                    json::accessor::ComponentType::F32 => DataType::Float32,
                };
                if attribute.data_type() != data_type
                    || usize::from(attribute.num_components()) != shape.multiplicity()
                    || !matches!(
                        shape,
                        json::accessor::Type::Scalar
                            | json::accessor::Type::Vec2
                            | json::accessor::Type::Vec3
                            | json::accessor::Type::Vec4
                    )
                    || accessor.sparse.is_some()
                    || accessor.count.0 != mesh.num_points() as u64
                {
                    return Err(format!(
                        "Decoded Draco {semantic} does not match its accessor"
                    ));
                }
                let width = component.0.size() * shape.multiplicity();
                let stride = usize::try_from(attribute.byte_stride())
                    .map_err(|_| "Invalid Draco attribute stride")?;
                if stride < width {
                    return Err("Invalid Draco attribute stride".into());
                }
                let offset = aligned_offset(&mut expanded, mesh.num_points() * width)?;
                for point in 0..mesh.num_points() {
                    let mapped = attribute.mapped_index(PointIndex(point as u32)).0 as usize;
                    if mapped >= attribute.size() {
                        return Err("Invalid Draco point-to-attribute mapping".into());
                    }
                    let start = mapped
                        .checked_mul(stride)
                        .ok_or("Draco attribute offset overflow")?;
                    let end = start
                        .checked_add(width)
                        .ok_or("Draco attribute range overflow")?;
                    let value = attribute
                        .buffer()
                        .data()
                        .get(start..end)
                        .ok_or("Draco attribute data is truncated")?;
                    expanded.extend_from_slice(value);
                }
                accessor.buffer_view = Some(append_view(
                    &mut root,
                    buffer_index,
                    offset,
                    mesh.num_points() * width,
                ));
                accessor.byte_offset = None;
                let index = append_accessor(&mut root, accessor_index, accessor);
                *primitive
                    .attributes
                    .iter_mut()
                    .find(|(key, _)| key.to_string() == *semantic)
                    .unwrap()
                    .1 = index;
            }
            // Draco's decoded topology is a triangle list, including streams
            // whose source primitive declared TRIANGLE_STRIP.
            let offset = aligned_offset(&mut expanded, mesh.num_faces() * 12)?;
            for face in mesh.faces() {
                for point in face {
                    if point.0 as usize >= mesh.num_points() {
                        return Err("Draco triangle references a missing point".into());
                    }
                    expanded.extend_from_slice(&point.0.to_le_bytes());
                }
            }
            let mut accessor: json::accessor::Accessor =
                serde_json::from_value(serde_json::json!({
                    "componentType": 5125, "type": "SCALAR", "count": mesh.num_faces() * 3
                }))
                .map_err(|error| error.to_string())?;
            accessor.buffer_view = Some(append_view(
                &mut root,
                buffer_index,
                offset,
                mesh.num_faces() * 12,
            ));
            if let Some(original) = primitive.indices {
                if matches!(primitive.mode, Checked::Valid(json::mesh::Mode::Triangles))
                    && root.accessors[original.value()].count.0 != accessor.count.0
                {
                    return Err("Decoded Draco indices do not match their accessor count".into());
                }
                primitive.indices = Some(append_accessor(&mut root, original, accessor));
            } else {
                let index = Index::new(root.accessors.len() as u32);
                root.accessors.push(accessor);
                primitive.indices = Some(index);
            }
            primitive.mode = Checked::Valid(json::mesh::Mode::Triangles);
            primitive
                .extensions
                .as_mut()
                .unwrap()
                .others
                .remove(EXTENSION);
            root.meshes[mesh_index].primitives[primitive_index] = primitive;
            primitive_count += 1;
        }
    }
    if primitive_count != 0 {
        root.buffers.push(json::buffer::Buffer {
            byte_length: expanded.len().into(),
            uri: None,
            name: None,
            extensions: None,
            extras: Default::default(),
        });
        buffers.push(Cow::Owned(expanded));
    }
    root.extensions_required.retain(|name| name != EXTENSION);
    root.extensions_used.retain(|name| name != EXTENSION);
    let document = gltf::Document::from_json(root).map_err(|error| error.to_string())?;
    Ok((document, primitive_count))
}

fn validate_source(root: &json::Root) -> Result<(), String> {
    // Keep all original core validation. The only exception is bufferView on
    // accessors supplied by Draco: validate those declarations against an
    // existing view here, then replace them with decoded data below. This copy
    // is used for JSON validation only; compressed bytes never reach a reader.
    let mut validation = root.clone();
    validation
        .extensions_required
        .retain(|name| name != EXTENSION);
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
                    let accessor = &mut validation.accessors[index.value()];
                    if accessor.buffer_view.is_none() {
                        accessor.buffer_view = Some(Index::new(view as u32));
                    }
                }
            }
            if let Some(index) = primitive.indices {
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
    Ok(())
}

fn aligned_offset(bytes: &mut Vec<u8>, length: usize) -> Result<usize, String> {
    let start = bytes.len().next_multiple_of(4);
    if start
        .checked_add(length)
        .is_none_or(|end| end > MAX_RESOURCE_BYTES)
    {
        return Err("Expanded Draco geometry exceeds the 64 MiB limit".into());
    }
    bytes.resize(start, 0);
    Ok(start)
}

fn append_view(
    root: &mut json::Root,
    buffer: usize,
    offset: usize,
    length: usize,
) -> Index<json::buffer::View> {
    let index = Index::new(root.buffer_views.len() as u32);
    root.buffer_views.push(json::buffer::View {
        buffer: Index::new(buffer as u32),
        byte_length: length.into(),
        byte_offset: Some(offset.into()),
        byte_stride: None,
        name: None,
        target: None,
        extensions: None,
        extras: Default::default(),
    });
    index
}

fn append_accessor(
    root: &mut json::Root,
    original: Index<json::accessor::Accessor>,
    accessor: json::accessor::Accessor,
) -> Index<json::accessor::Accessor> {
    // Fill compressed-only declarations so core validation can still validate
    // the entire document. Preserve any existing uncompressed fallback data.
    if root.accessors[original.value()].buffer_view.is_none() {
        root.accessors[original.value()] = accessor.clone();
    }
    let index = Index::new(root.accessors.len() as u32);
    root.accessors.push(accessor);
    index
}
