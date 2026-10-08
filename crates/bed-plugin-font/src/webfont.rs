//! Normalize webfont containers on the worker, without changing document bytes.
use flate2::{Decompress, FlushDecompress, Status};
use std::{error::Error, io, sync::Arc};

pub(crate) const MAX_FONT_BYTES: usize = 64 * 1024 * 1024;

pub(crate) fn decode(bytes: Arc<[u8]>) -> Result<Arc<[u8]>, String> {
    if bytes.is_empty() || bytes.len() > MAX_FONT_BYTES {
        return Err("Font must contain between 1 byte and 64 MiB".into());
    }
    let (format, header_size) = match bytes.get(..4) {
        Some(b"wOFF") => ("WOFF", 44),
        Some(b"wOF2") => ("WOFF2", 48),
        _ => return Ok(bytes),
    };
    let invalid = || format!("Invalid or truncated {format} font");
    if bytes.len() < header_size || u32_at(&bytes, 8) != bytes.len() {
        return Err(invalid());
    }
    let tables = u16::from_be_bytes([bytes[12], bytes[13]]) as usize;
    let decoded_size = u32_at(&bytes, 16);
    if tables == 0 || decoded_size == 0 || decoded_size > MAX_FONT_BYTES {
        return Err(format!("Decoded {format} font must fit within 64 MiB"));
    }
    let decoded = if header_size == 44 {
        // Validate every table's allocation before invoking the decoder. The
        // header's totalSfntSize alone cannot bound malicious table lengths.
        let directory_end = header_size + tables * 20;
        if directory_end > bytes.len() || bytes[14..16] != [0, 0] {
            return Err(invalid());
        }
        let mut total = 12 + tables * 16;
        for entry in bytes[header_size..directory_end].as_chunks::<20>().0 {
            let offset = u32_at(entry, 4);
            let compressed = u32_at(entry, 8);
            let original = u32_at(entry, 12);
            if compressed > original
                || original > MAX_FONT_BYTES
                || offset < directory_end
                || !offset.is_multiple_of(4)
                || offset > bytes.len()
                || compressed > bytes.len() - offset
            {
                return Err(invalid());
            }
            total += (original + 3) & !3;
            if total > MAX_FONT_BYTES {
                return Err("Decoded WOFF font exceeds 64 MiB".into());
            }
        }
        if total != decoded_size {
            return Err(invalid());
        }
        wuff::decompress_woff1_with_custom_z(&bytes, &mut decompress_zlib)
    } else {
        // Wuff independently bounds its Brotli buffer using the directory's
        // transformed lengths, and limits reconstruction to 128 MiB. Check
        // our smaller document limit again on the reconstructed SFNT below.
        wuff::decompress_woff2(&bytes)
    }
    .map_err(|_| format!("Could not decode {format} font: invalid compressed data"))?;
    if decoded.is_empty() || decoded.len() > MAX_FONT_BYTES {
        return Err(format!("Decoded {format} font must fit within 64 MiB"));
    }
    Ok(decoded.into())
}

fn u32_at(bytes: &[u8], offset: usize) -> usize {
    u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize
}

fn decompress_zlib(input: &[u8], length: usize) -> Result<Vec<u8>, Box<dyn Error>> {
    if length > MAX_FONT_BYTES {
        return Err(io::Error::other("WOFF table exceeds 64 MiB").into());
    }
    // A fixed output slice prevents decompression from growing past origLength.
    let mut output = vec![0; length];
    let mut decoder = Decompress::new(true);
    let status = decoder.decompress(input, &mut output, FlushDecompress::Finish)?;
    if status != Status::StreamEnd
        || decoder.total_in() != input.len() as u64
        || decoder.total_out() != length as u64
    {
        return Err(io::Error::other("WOFF table length does not match its zlib stream").into());
    }
    Ok(output)
}
