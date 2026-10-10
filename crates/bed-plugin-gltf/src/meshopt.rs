//! Decode buffer views before glTF's ordinary accessor readers see their bytes.
use crate::model::MAX_RESOURCE_BYTES;
use serde_json::{Value, json};
use std::borrow::Cow;

pub(super) const EXTENSIONS: [&str; 2] = ["EXT_meshopt_compression", "KHR_meshopt_compression"];

pub(super) fn extension(view: &Value) -> Result<Option<(&str, &Value)>, String> {
    let ext = &view["extensions"];
    match (ext.get(EXTENSIONS[0]), ext.get(EXTENSIONS[1])) {
        (Some(_), Some(_)) => Err("A buffer/view cannot use both meshopt extensions".into()),
        (Some(v), None) => Ok(Some((EXTENSIONS[0], v))),
        (None, Some(v)) => Ok(Some((EXTENSIONS[1], v))),
        _ => Ok(None),
    }
}

/// Missing fallback buffers describe the decoded layout; they are never allocated.
pub(super) fn is_placeholder(root: &Value, index: usize, has_blob: bool) -> Result<bool, String> {
    let buffer = &root["buffers"][index];
    let declaration = extension(buffer)?;
    let tagged = match declaration {
        Some((name, ext)) => {
            if !ext.is_object() {
                return Err("Invalid meshopt fallback buffer declaration".into());
            }
            match ext.get("fallback") {
                Some(Value::Bool(true)) => Some((name, ext)),
                None | Some(Value::Bool(false)) => None,
                _ => return Err("Invalid meshopt fallback buffer declaration".into()),
            }
        }
        None => None,
    };
    let missing = buffer.get("uri").is_none() && !(index == 0 && has_blob);
    if !missing && tagged.is_none() {
        return Ok(false);
    }
    let views = root["bufferViews"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let declared = integer(buffer, "byteLength")?;
    let mut referenced = false;
    for view in views {
        if let Some((name, ext)) = extension(view)? {
            if integer(ext, "buffer")? == index {
                return Err("A meshopt fallback buffer cannot contain compressed data".into());
            }
            if integer(view, "buffer")? == index {
                referenced = true;
                if tagged.is_some_and(|(tag, _)| tag != name) {
                    return Err(
                        "Meshopt fallback buffer uses a different extension than its views".into(),
                    );
                }
                if missing
                    && !root["extensionsRequired"]
                        .as_array()
                        .is_some_and(|list| list.iter().any(|v| v.as_str() == Some(name)))
                {
                    return Err(
                        "Missing meshopt fallback buffer requires the meshopt extension".into(),
                    );
                }
                let offset = optional_integer(view, "byteOffset", 0)?;
                if offset
                    .checked_add(integer(view, "byteLength")?)
                    .is_none_or(|end| end > declared)
                {
                    return Err("Meshopt fallback view exceeds its declared buffer".into());
                }
            }
        } else if view["buffer"].as_u64() == Some(index as u64) {
            return Err(
                "A missing/fallback buffer is referenced by an ordinary buffer view".into(),
            );
        }
    }
    Ok(tagged.is_some() || referenced)
}

pub(super) fn expand(
    root: &mut Value,
    buffers: &mut Vec<Cow<'_, [u8]>>,
    expanded_bytes: &mut usize,
) -> Result<usize, String> {
    let Some(views) = root.get_mut("bufferViews").and_then(Value::as_array_mut) else {
        return Ok(0);
    };
    let mut count = 0;
    for (index, view) in views.iter_mut().enumerate() {
        let Some((name, ext)) = extension(view)? else {
            continue;
        };
        let name = name.to_owned();
        let source_index = integer(ext, "buffer")?;
        let start = optional_integer(ext, "byteOffset", 0)?;
        let length = integer(ext, "byteLength")?;
        let end = start
            .checked_add(length)
            .ok_or("Meshopt compressed byte range overflow")?;
        let source = buffers
            .get(source_index)
            .and_then(|buffer| buffer.get(start..end))
            .ok_or("Meshopt compressed buffer view is out of bounds")?;
        let stride = integer(ext, "byteStride")?;
        let elements = integer(ext, "count")?;
        let mode = ext["mode"].as_str().ok_or("Invalid meshopt mode")?;
        let filter = match ext.get("filter") {
            None => "NONE",
            Some(value) => value.as_str().ok_or("Invalid meshopt filter")?,
        };
        let decoded_size = stride
            .checked_mul(elements)
            .ok_or("Meshopt decoded byte count overflow")?;
        if decoded_size != integer(view, "byteLength")?
            || name == EXTENSIONS[0]
                && view
                    .get("byteStride")
                    .is_some_and(|v| v.as_u64() != Some(stride as u64))
        {
            return Err(format!(
                "Meshopt view {index} decoded layout does not match its buffer view"
            ));
        }
        if expanded_bytes
            .checked_add(decoded_size)
            .is_none_or(|end| end > MAX_RESOURCE_BYTES)
        {
            return Err("Expanded glTF geometry exceeds the 64 MiB limit".into());
        }
        let decoded = decode(
            source,
            elements,
            stride,
            mode,
            filter,
            name == EXTENSIONS[1],
        )
        .map_err(|error| format!("Could not decode meshopt view {index}: {error}"))?;
        *expanded_bytes += decoded.len();
        view["buffer"] = json!(buffers.len());
        view["byteOffset"] = json!(0);
        view["extensions"].as_object_mut().unwrap().remove(&name);
        buffers.push(Cow::Owned(decoded));
        count += 1;
    }
    if count == 0 && !buffers.iter().any(|buffer| buffer.is_empty()) {
        return Ok(0);
    }
    let declarations: Vec<_> = buffers
        .iter()
        .map(|bytes| json!({"byteLength": bytes.len().max(1)}))
        .collect();
    root["buffers"] = Value::Array(declarations);
    for key in ["extensionsUsed", "extensionsRequired"] {
        if let Some(list) = root.get_mut(key).and_then(Value::as_array_mut) {
            list.retain(|value| !EXTENSIONS.contains(&value.as_str().unwrap_or("")));
        }
    }
    Ok(count)
}

fn decode(
    source: &[u8],
    count: usize,
    stride: usize,
    mode: &str,
    filter: &str,
    khr: bool,
) -> Result<Vec<u8>, String> {
    let valid_mode = match mode {
        "ATTRIBUTES" => stride >= 4 && stride <= 256 && stride.is_multiple_of(4),
        "TRIANGLES" => matches!(stride, 2 | 4) && count.is_multiple_of(3) && filter == "NONE",
        "INDICES" => matches!(stride, 2 | 4) && filter == "NONE",
        _ => false,
    };
    let valid_filter = match filter {
        "NONE" => true,
        "OCTAHEDRAL" => matches!(stride, 4 | 8),
        "QUATERNION" => stride == 8,
        "EXPONENTIAL" => stride.is_multiple_of(4),
        "COLOR" => khr && matches!(stride, 4 | 8),
        _ => false,
    };
    if count == 0 || source.is_empty() || !valid_mode || !valid_filter {
        return Err("Invalid meshopt count, stride, mode, or filter".into());
    }
    if mode == "ATTRIBUTES" && !khr && source.first() != Some(&0xa0) {
        return Err("EXT_meshopt_compression requires attribute bitstream version 0".into());
    }
    if mode == "TRIANGLES" && source.first() != Some(&0xe1)
        || mode == "INDICES" && source.first() != Some(&0xd1)
    {
        return Err("Unsupported meshopt index bitstream version".into());
    }
    let length = count
        .checked_mul(stride)
        .filter(|n| *n <= MAX_RESOURCE_BYTES)
        .ok_or("Meshopt decoded data exceeds the 64 MiB limit")?;
    // Filters access 16-/32-bit components, so allocate explicitly aligned storage.
    let mut destination = vec![0_u32; length.div_ceil(4)];
    // SAFETY: layout restrictions above are meshoptimizer's preconditions. The
    // destination is aligned, fully initialized, and sized count * stride; input
    // decoders accept untrusted bytes and return an error for malformed streams.
    let result = unsafe {
        let output = destination.as_mut_ptr().cast();
        match mode {
            "ATTRIBUTES" => meshopt::ffi::meshopt_decodeVertexBuffer(
                output,
                count,
                stride,
                source.as_ptr(),
                source.len(),
            ),
            "TRIANGLES" => meshopt::ffi::meshopt_decodeIndexBuffer(
                output,
                count,
                stride,
                source.as_ptr(),
                source.len(),
            ),
            _ => meshopt::ffi::meshopt_decodeIndexSequence(
                output,
                count,
                stride,
                source.as_ptr(),
                source.len(),
            ),
        }
    };
    if result != 0 {
        return Err(format!(
            "Invalid meshopt bitstream (decoder error {result})"
        ));
    }
    // SAFETY: the decoder succeeded and the filter/stride pair has been checked.
    unsafe {
        let output = destination.as_mut_ptr().cast();
        match filter {
            "OCTAHEDRAL" => meshopt::ffi::meshopt_decodeFilterOct(output, count, stride),
            "QUATERNION" => meshopt::ffi::meshopt_decodeFilterQuat(output, count, stride),
            "EXPONENTIAL" => meshopt::ffi::meshopt_decodeFilterExp(output, count, stride),
            "COLOR" => meshopt::ffi::meshopt_decodeFilterColor(output, count, stride),
            _ => (),
        }
    }
    Ok(bytemuck::cast_slice(&destination)[..length].to_vec())
}

fn integer(value: &Value, key: &str) -> Result<usize, String> {
    value[key]
        .as_u64()
        .and_then(|n| usize::try_from(n).ok())
        .ok_or_else(|| format!("Invalid meshopt {key}"))
}
fn optional_integer(value: &Value, key: &str, default: usize) -> Result<usize, String> {
    if value.get(key).is_none() {
        Ok(default)
    } else {
        integer(value, key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrips_attributes_and_triangle_indices() {
        let vertices = [[1.0_f32, 2.0, 3.0], [2.0, 3.0, 4.0], [4.0, 3.0, 2.0]];
        let encoded = meshopt::encode_vertex_buffer(&vertices).unwrap();
        let decoded = decode(&encoded, 3, 12, "ATTRIBUTES", "NONE", true).unwrap();
        assert_eq!(decoded, bytemuck::cast_slice::<_, u8>(&vertices));
        let encoded = meshopt::encode_index_buffer(&[0, 1, 2], 3).unwrap();
        assert_eq!(
            decode(&encoded, 3, 2, "TRIANGLES", "NONE", false).unwrap(),
            bytemuck::cast_slice::<_, u8>(&[0_u16, 1, 2])
        );
    }
    #[test]
    fn roundtrips_index_sequences() {
        let indices = [2_u32, 7, 4, 9, 0];
        let mut encoded =
            vec![0; unsafe { meshopt::ffi::meshopt_encodeIndexSequenceBound(indices.len(), 10) }];
        let length = unsafe {
            meshopt::ffi::meshopt_encodeIndexSequence(
                encoded.as_mut_ptr(),
                encoded.len(),
                indices.as_ptr(),
                indices.len(),
            )
        };
        encoded.truncate(length);
        assert_eq!(
            decode(&encoded, indices.len(), 4, "INDICES", "NONE", false).unwrap(),
            bytemuck::cast_slice::<_, u8>(&indices)
        );
    }
    #[test]
    fn decodes_every_attribute_filter_and_both_bitstream_versions() {
        for version in [0, 1] {
            for (filter, stride, input, expected) in [
                (
                    "OCTAHEDRAL",
                    4_usize,
                    [0.0_f32, 0.0, 1.0, 1.0],
                    vec![0_u8, 0, 127, 127],
                ),
                (
                    "QUATERNION",
                    8,
                    [0.0, 0.0, 0.0, 1.0],
                    bytemuck::cast_slice(&[0_i16, 0, 0, 32767]).to_vec(),
                ),
                (
                    "EXPONENTIAL",
                    16,
                    [1.0, 2.0, 4.0, -8.0],
                    bytemuck::cast_slice(&[1.0_f32, 2.0, 4.0, -8.0]).to_vec(),
                ),
                ("COLOR", 4, [1.0, 0.0, 0.0, 1.0], vec![254, 1, 0, 255]),
            ] {
                let mut filtered = vec![0_u32; stride / 4];
                let mut encoded =
                    vec![0; unsafe { meshopt::ffi::meshopt_encodeVertexBufferBound(1, stride) }];
                unsafe {
                    let output = filtered.as_mut_ptr().cast();
                    match filter {
                        "OCTAHEDRAL" => meshopt::ffi::meshopt_encodeFilterOct(
                            output,
                            1,
                            stride,
                            8,
                            input.as_ptr(),
                        ),
                        "QUATERNION" => meshopt::ffi::meshopt_encodeFilterQuat(
                            output,
                            1,
                            stride,
                            16,
                            input.as_ptr(),
                        ),
                        "EXPONENTIAL" => meshopt::ffi::meshopt_encodeFilterExp(
                            output,
                            1,
                            stride,
                            24,
                            input.as_ptr(),
                            meshopt::ffi::meshopt_EncodeExpMode_meshopt_EncodeExpSeparate,
                        ),
                        _ => meshopt::ffi::meshopt_encodeFilterColor(
                            output,
                            1,
                            stride,
                            8,
                            input.as_ptr(),
                        ),
                    }
                    let length = meshopt::ffi::meshopt_encodeVertexBufferLevel(
                        encoded.as_mut_ptr(),
                        encoded.len(),
                        filtered.as_ptr().cast(),
                        1,
                        stride,
                        2,
                        version,
                    );
                    encoded.truncate(length);
                }
                assert_eq!(
                    decode(&encoded, 1, stride, "ATTRIBUTES", filter, true).unwrap(),
                    expected,
                    "{filter} version {version}"
                );
                if filter != "COLOR" && version == 0 {
                    assert_eq!(
                        decode(&encoded, 1, stride, "ATTRIBUTES", filter, false).unwrap(),
                        expected
                    );
                }
            }
        }
    }
    #[test]
    fn invalid_layouts_never_reach_the_native_decoder() {
        for (count, stride, mode, filter) in [
            (1, 3, "ATTRIBUTES", "NONE"),
            (2, 2, "TRIANGLES", "NONE"),
            (3, 4, "INDICES", "EXPONENTIAL"),
            (1, 12, "ATTRIBUTES", "QUATERNION"),
            (1, 4, "ATTRIBUTES", "COLOR"),
        ] {
            assert!(decode(&[0], count, stride, mode, filter, false).is_err());
        }
        assert!(decode(&[0xa0], usize::MAX, 256, "ATTRIBUTES", "NONE", true).is_err());
    }
    #[test]
    fn recognizes_missing_required_fallback_without_allocating_it() {
        let root = json!({"extensionsRequired": [EXTENSIONS[0]], "buffers": [{"byteLength": 40, "uri":"data:a"}, {"byteLength": 1_000_000_000}], "bufferViews": [{"buffer":1, "byteLength":12, "extensions": {EXTENSIONS[0]: {"buffer":0}}}]});
        assert!(is_placeholder(&root, 1, false).unwrap());
        let mut invalid = root;
        invalid["extensionsRequired"] = json!([]);
        assert!(is_placeholder(&invalid, 1, false).is_err());
    }
}
