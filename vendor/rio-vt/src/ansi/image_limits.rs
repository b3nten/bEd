//! Bed's image boundary: reject oversized dimensions before decoding pixels.
#[cfg(feature = "graphics")]
pub(super) fn decode(
    bytes: &[u8],
    format: Option<image_rs::ImageFormat>,
) -> image_rs::ImageResult<image_rs::DynamicImage> {
    use image_rs::{DynamicImage, ImageDecoder, ImageReader, Limits};
    use std::io::Cursor;
    const MAX_BYTES: u64 = 64 * 1024 * 1024;
    let mut reader = ImageReader::new(Cursor::new(bytes));
    if let Some(format) = format {
        reader.set_format(format);
    } else {
        reader = reader.with_guessed_format()?;
    }
    let mut limits = Limits::default();
    limits.max_image_width = Some(10_000);
    limits.max_image_height = Some(10_000);
    limits.max_alloc = Some(MAX_BYTES);
    reader.limits(limits);
    let decoder = reader.into_decoder()?;
    let (width, height) = decoder.dimensions();
    if u64::from(width) * u64::from(height) * 4 > MAX_BYTES {
        return Err(image_rs::ImageError::Limits(
            image_rs::error::LimitError::from_kind(
                image_rs::error::LimitErrorKind::InsufficientMemory,
            ),
        ));
    }
    DynamicImage::from_decoder(decoder)
}
