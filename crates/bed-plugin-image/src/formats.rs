use super::{DecodedImage, MAX_DECODED_BYTES};
use image::{ImageDecoder, ImageFormat, ImageReader};
use std::{
    io::Cursor,
    sync::{Arc, OnceLock},
};

pub(super) const SUPPORTED_EXTENSIONS: &[&str] = &[
    "png", "apng", "jpg", "jpeg", "jfif", "svg", "gif", "webp", "bmp", "ico", "tif", "tiff", "tga",
    "pnm", "pbm", "pgm", "ppm", "pam", "qoi", "dds", "hdr", "exr",
];

fn extension(path: &str) -> Option<&str> {
    path.rsplit(['/', '\\'])
        .next()?
        .rsplit_once('.')
        .map(|(_, ext)| ext)
}

pub(super) fn supported_path(path: &str) -> bool {
    extension(path).is_some_and(|ext| {
        SUPPORTED_EXTENSIONS
            .iter()
            .any(|candidate| ext.eq_ignore_ascii_case(candidate))
    })
}

fn validate_dimensions(width: u32, height: u32) -> Result<(), String> {
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_DECODED_BYTES / 4 {
        return Err("Image exceeds the 64 MiB decoded-pixel limit".into());
    }
    Ok(())
}

fn reader(bytes: &[u8], format: ImageFormat) -> ImageReader<Cursor<&[u8]>> {
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(MAX_DECODED_BYTES);
    limits.max_image_width = Some((MAX_DECODED_BYTES / 4) as u32);
    limits.max_image_height = Some((MAX_DECODED_BYTES / 4) as u32);
    reader.limits(limits);
    reader
}

pub(super) fn decode(bytes: &[u8], path: &str) -> Result<DecodedImage, String> {
    // TGA has no reliable signature, so use the document's extension as a fallback.
    // Reading the snapshot rather than the path also supports SSH and unsaved edits.
    let format = image::guess_format(bytes)
        .ok()
        .or_else(|| extension(path).and_then(ImageFormat::from_extension));
    let Some(format) = format else {
        return decode_svg(bytes);
    };
    let (width, height) = reader(bytes, format)
        .into_dimensions()
        .map_err(|error| error.to_string())?;
    validate_dimensions(width, height)?;
    let rgba = reader(bytes, format)
        .decode()
        .map_err(|error| error.to_string())?
        .into_rgba8();
    Ok(DecodedImage {
        size: [width, height],
        rgba: rgba.into_raw().into(),
    })
}

fn svg_fonts() -> Arc<resvg::usvg::fontdb::Database> {
    static FONTS: OnceLock<Arc<resvg::usvg::fontdb::Database>> = OnceLock::new();
    Arc::clone(FONTS.get_or_init(|| {
        let mut fonts = resvg::usvg::fontdb::Database::new();
        fonts.load_system_fonts();
        Arc::new(fonts)
    }))
}

fn decode_svg(bytes: &[u8]) -> Result<DecodedImage, String> {
    let resolve_data = resvg::usvg::ImageHrefResolver::default_data_resolver();
    let options = resvg::usvg::Options {
        fontdb: svg_fonts(),
        image_href_resolver: resvg::usvg::ImageHrefResolver {
            // Keep SVGs self-contained, including when the document is on SSH.
            resolve_string: Box::new(|_, _| None),
            resolve_data: Box::new(move |mime, data, options| {
                // Embedded rasters must obey the same limits before resvg decodes them.
                if let Ok(format) = image::guess_format(&data) {
                    let decoder = reader(&data, format).into_decoder().ok()?;
                    let (width, height) = decoder.dimensions();
                    validate_dimensions(width, height).ok()?;
                    if decoder.total_bytes() > MAX_DECODED_BYTES {
                        return None;
                    }
                }
                resolve_data(mime, data, options)
            }),
        },
        ..Default::default()
    };
    let tree = resvg::usvg::Tree::from_data(bytes, &options)
        .map_err(|error| format!("Invalid SVG: {error}"))?;
    rasterize_svg(&tree)
}

fn rasterize_svg(tree: &resvg::usvg::Tree) -> Result<DecodedImage, String> {
    let size = tree.size().to_int_size();
    validate_dimensions(size.width(), size.height())?;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(size.width(), size.height())
        .ok_or("Could not allocate SVG pixels")?;
    let transform = resvg::tiny_skia::Transform::from_scale(
        size.width() as f32 / tree.size().width(),
        size.height() as f32 / tree.size().height(),
    );
    resvg::render(tree, transform, &mut pixmap.as_mut());
    Ok(DecodedImage {
        size: [size.width(), size.height()],
        // The GPU uses straight alpha; tiny-skia renders premultiplied pixels.
        rgba: pixmap.take_demultiplied().into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encoded(format: ImageFormat) -> Vec<u8> {
        let image = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            3,
            2,
            image::Rgb([255, 64, 32]),
        ));
        let image = match format {
            ImageFormat::Ico => image::DynamicImage::ImageRgba8(image.into_rgba8()),
            ImageFormat::Hdr | ImageFormat::OpenExr => {
                image::DynamicImage::ImageRgb32F(image.into_rgb32f())
            }
            _ => image,
        };
        let mut bytes = Cursor::new(Vec::new());
        image.write_to(&mut bytes, format).unwrap();
        bytes.into_inner()
    }

    #[test]
    fn raster_formats_decode_with_exact_dimensions_and_rgba_size() {
        for format in [
            ImageFormat::Png,
            ImageFormat::Jpeg,
            ImageFormat::Gif,
            ImageFormat::WebP,
            ImageFormat::Bmp,
            ImageFormat::Ico,
            ImageFormat::Tiff,
            ImageFormat::Tga,
            ImageFormat::Pnm,
            ImageFormat::Qoi,
            ImageFormat::Hdr,
            ImageFormat::OpenExr,
        ] {
            let bytes = encoded(format);
            let path = format!("/remote/picture.{}", format.extensions_str()[0]);
            let decoded =
                decode(&bytes, &path).unwrap_or_else(|error| panic!("{format:?}: {error}"));
            assert_eq!(decoded.size, [3, 2], "{format:?}");
            assert_eq!(decoded.rgba.len(), 24, "{format:?}");
            assert!(
                decoded
                    .rgba
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .all(|pixel| pixel[3] == 255),
                "{format:?}"
            );
        }
    }

    #[test]
    fn signatures_override_extensions_and_tga_uses_the_snapshot_path() {
        let png = encoded(ImageFormat::Png);
        assert_eq!(decode(&png, "wrong.tga").unwrap().size, [3, 2]);
        assert_eq!(decode(&png, "no-extension").unwrap().size, [3, 2]);
        let tga = encoded(ImageFormat::Tga);
        assert!(image::guess_format(&tga).is_err());
        assert_eq!(
            decode(&tga, "C:\\remote\\picture.TGA").unwrap().size,
            [3, 2]
        );
    }

    #[test]
    fn animated_gif_displays_the_first_frame() {
        let mut bytes = Vec::new();
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(&mut bytes);
            for color in [[255, 0, 0, 255], [0, 255, 0, 255]] {
                encoder
                    .encode_frame(image::Frame::new(image::RgbaImage::from_pixel(
                        3,
                        2,
                        image::Rgba(color),
                    )))
                    .unwrap();
            }
        }
        let decoded = decode(&bytes, "animated.gif").unwrap();
        assert_eq!(decoded.size, [3, 2]);
        assert!(
            decoded
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .all(|pixel| pixel == &[255, 0, 0, 255])
        );
    }

    #[test]
    fn dds_dxt1_texture_decodes() {
        // One 4 × 4 DXT1 block with opaque red pixels.
        let mut bytes = vec![0u8; 136];
        bytes[..4].copy_from_slice(b"DDS ");
        for (offset, value) in [
            (4, 124u32),
            (8, 0x81007),
            (12, 4),
            (16, 4),
            (20, 8),
            (76, 32),
            (80, 4),
            (108, 0x1000),
        ] {
            bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        bytes[84..88].copy_from_slice(b"DXT1");
        bytes[128..130].copy_from_slice(&0xf800u16.to_le_bytes());
        let decoded = decode(&bytes, "texture.dds").unwrap();
        assert_eq!(decoded.size, [4, 4]);
        assert!(
            decoded
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .all(|pixel| pixel == &[255, 0, 0, 255])
        );
    }

    #[test]
    fn svg_uses_its_viewbox_and_preserves_straight_alpha() {
        let svg = br##"<?xml version="1.0"?>
            <!-- An SVG with no explicit width or height. -->
            <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 4 2">
                <rect width="2" height="2" fill="#ff0000" fill-opacity="0.5"/>
            </svg>"##;
        let decoded = decode(svg, "picture.SVG").unwrap();
        assert_eq!(decoded.size, [4, 2]);
        assert_eq!(decoded.rgba.len(), 32);
        assert_eq!(&decoded.rgba[..4], &[255, 0, 0, 128]);
        assert_eq!(&decoded.rgba[12..16], &[0, 0, 0, 0]);
        assert_eq!(decode(svg, "no-extension").unwrap().size, [4, 2]);
    }

    #[test]
    fn svg_fractional_dimensions_fill_the_rounded_pixel_bounds() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="2.5" height="1.5">
            <rect width="100%" height="100%" fill="red"/>
        </svg>"#;
        let decoded = decode(svg, "fractional.svg").unwrap();
        assert_eq!(decoded.size, [3, 2]);
        assert!(
            decoded
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .all(|pixel| pixel == &[255, 0, 0, 255])
        );
    }

    #[test]
    fn svg_text_renders_with_a_known_font() {
        let mut options = resvg::usvg::Options::default();
        options
            .fontdb_mut()
            .load_font_data(include_bytes!("../../../resources/fonts/DejaVuSans.ttf").to_vec());
        let tree = resvg::usvg::Tree::from_str(
            r#"
            <svg xmlns="http://www.w3.org/2000/svg" width="80" height="32">
                <text x="2" y="24" font-family="DejaVu Sans" font-size="24">SVG</text>
            </svg>"#,
            &options,
        )
        .unwrap();
        let decoded = rasterize_svg(&tree).unwrap();
        assert_eq!(decoded.size, [80, 32]);
        assert!(
            decoded
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .any(|pixel| pixel[3] > 0)
        );
    }

    fn png_data_url(bytes: &[u8]) -> String {
        let escaped: String = bytes.iter().map(|byte| format!("%{byte:02X}")).collect();
        format!("data:image/png,{escaped}")
    }

    #[test]
    fn svg_displays_embedded_images_and_ignores_external_files() {
        let embedded = png_data_url(&encoded(ImageFormat::Png));
        let external = concat!(env!("CARGO_MANIFEST_DIR"), "/../../resources/icons/bed.png");
        assert!(std::path::Path::new(external).exists());
        let svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="6" height="2">
            <image href="{embedded}" width="3" height="2"/>
            <image href="{external}" x="3" width="3" height="2"/>
        </svg>"#
        );
        let decoded = decode(svg.as_bytes(), "embedded.svg").unwrap();
        assert_eq!(&decoded.rgba[..4], &[255, 64, 32, 255]);
        assert_eq!(&decoded.rgba[12..16], &[0, 0, 0, 0]);
    }

    #[test]
    fn invalid_and_oversized_images_are_rejected() {
        for (bytes, path) in [
            (b"not an image".as_slice(), "image.png"),
            (b"<svg".as_slice(), "broken.svg"),
            (b"<html/>".as_slice(), "wrong.svg"),
        ] {
            assert!(decode(bytes, path).is_err(), "{path}");
        }
        assert!(validate_dimensions(4096, 4096).is_ok());
        assert!(validate_dimensions(4097, 4096).is_err());
        assert!(validate_dimensions(u32::MAX, u32::MAX).is_err());
        assert!(validate_dimensions(0, 1).is_err());

        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="4097" height="4096"/>"#;
        assert!(decode(svg, "huge.svg").err().unwrap().contains("64 MiB"));
        let mut bmp = encoded(ImageFormat::Bmp);
        bmp[18..22].copy_from_slice(&4097u32.to_le_bytes());
        bmp[22..26].copy_from_slice(&4096u32.to_le_bytes());
        assert!(decode(&bmp, "huge.bmp").err().unwrap().contains("64 MiB"));
    }
}
