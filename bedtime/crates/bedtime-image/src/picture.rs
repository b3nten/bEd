//! Decoding a file into pixels, on the document's load worker: raster
//! formats (by their signature; TGA has none, so it's tried last), animated
//! GIF, APNG and WebP frames, and SVG rendered at a sharp size.
use bedtime_document::DocumentModel;
use bedtime_filesystem::FileModel;
use image::{AnimationDecoder, ImageDecoder, ImageFormat, ImageReader};
use std::{
    io::Cursor,
    sync::{Arc, OnceLock},
    time::Duration,
};

/// Decoded pixels kept for one image (all its frames): 256 MiB.
pub const MAX_DECODED_BYTES: u64 = 256 * 1024 * 1024;
/// The longest side shown; larger images are scaled down to it.
pub const MAX_SIDE: u32 = 8192;
/// SVGs are rendered with their longest side about this long.
const SVG_SIDE: f32 = 2048.0;

pub const SUPPORTED_EXTENSIONS: &[&str] = &[
    "png", "apng", "jpg", "jpeg", "jfif", "svg", "gif", "webp", "bmp", "ico", "tif", "tiff", "tga",
    "pnm", "pbm", "pgm", "ppm", "pam", "qoi", "dds", "hdr", "exr",
];

pub fn supported(path: &std::path::Path) -> bool {
    path.extension().is_some_and(|ext| {
        SUPPORTED_EXTENSIONS
            .iter()
            .any(|candidate| ext.eq_ignore_ascii_case(candidate))
    })
}

/// One frame: straight-alpha sRGBA pixels.
#[derive(Clone, Debug)]
pub struct Frame {
    pub size: [u32; 2],
    pub rgba: Arc<[u8]>,
    /// How long it shows, in an animation.
    pub delay: Duration,
}

/// An image file's pixels.
#[derive(Clone, Debug, Default)]
pub struct Picture(pub Option<Arc<Decoded>>);

#[derive(Debug)]
pub struct Decoded {
    /// Its own size, in pixels (or the SVG's size).
    pub size: [u32; 2],
    /// "PNG", "SVG", ...
    pub format: &'static str,
    /// One, or an animation's frames.
    pub frames: Vec<Frame>,
}

impl PartialEq for Picture {
    fn eq(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        }
    }
}

impl DocumentModel for Picture {
    type Change = std::convert::Infallible;
    type OpenState = ();
    fn apply(&mut self, change: &Self::Change) -> Result<Self::Change, String> {
        match *change {}
    }
}

impl FileModel for Picture {
    fn decode(bytes: &[u8]) -> Result<Self, String> {
        decode(bytes).map(|decoded| Self(Some(Arc::new(decoded))))
    }
    fn encode(&self, _: &mut dyn std::io::Write) -> std::io::Result<()> {
        Err(std::io::Error::other("Images aren't saved"))
    }
}

pub fn decode(bytes: &[u8]) -> Result<Decoded, String> {
    match image::guess_format(bytes) {
        Ok(format) => decode_raster(bytes, format),
        Err(_) if looks_like_svg(bytes) => decode_svg(bytes),
        // TGA has no signature.
        Err(_) => decode_raster(bytes, ImageFormat::Tga)
            .map_err(|_| "It isn't an image this can show".to_owned()),
    }
}

fn looks_like_svg(bytes: &[u8]) -> bool {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]);
    head.contains("<svg")
}

fn name(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Png => "PNG",
        ImageFormat::Jpeg => "JPEG",
        ImageFormat::Gif => "GIF",
        ImageFormat::WebP => "WebP",
        ImageFormat::Pnm => "PNM",
        ImageFormat::Tiff => "TIFF",
        ImageFormat::Tga => "TGA",
        ImageFormat::Dds => "DDS",
        ImageFormat::Bmp => "BMP",
        ImageFormat::Ico => "ICO",
        ImageFormat::Hdr => "HDR",
        ImageFormat::OpenExr => "OpenEXR",
        ImageFormat::Qoi => "QOI",
        _ => "Image",
    }
}

fn validate_dimensions(width: u32, height: u32) -> Result<(), String> {
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_DECODED_BYTES / 4 {
        return Err("It's too large to show (over 256 MiB of pixels)".into());
    }
    Ok(())
}

fn reader(bytes: &[u8], format: ImageFormat) -> ImageReader<Cursor<&[u8]>> {
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(MAX_DECODED_BYTES);
    reader.limits(limits);
    reader
}

fn decode_raster(bytes: &[u8], format: ImageFormat) -> Result<Decoded, String> {
    let (width, height) = reader(bytes, format)
        .into_dimensions()
        .map_err(|error| error.to_string())?;
    validate_dimensions(width, height)?;
    if let Some(frames) = animation(bytes, format)? {
        return Ok(Decoded {
            size: [width, height],
            format: name(format),
            frames,
        });
    }
    let image = reader(bytes, format)
        .decode()
        .map_err(|error| error.to_string())?;
    Ok(Decoded {
        size: [width, height],
        format: name(format),
        frames: vec![frame(image.into_rgba8(), Duration::ZERO)],
    })
}

/// An animation's frames, if it has more than one.
fn animation(bytes: &[u8], format: ImageFormat) -> Result<Option<Vec<Frame>>, String> {
    let frames = match format {
        ImageFormat::Gif => image::codecs::gif::GifDecoder::new(Cursor::new(bytes))
            .map(|decoder| decoder.into_frames()),
        ImageFormat::Png => {
            let decoder = image::codecs::png::PngDecoder::new(Cursor::new(bytes))
                .map_err(|error| error.to_string())?;
            if !decoder.is_apng().unwrap_or(false) {
                return Ok(None);
            }
            decoder.apng().map(|decoder| decoder.into_frames())
        }
        ImageFormat::WebP => {
            let decoder = image::codecs::webp::WebPDecoder::new(Cursor::new(bytes))
                .map_err(|error| error.to_string())?;
            if !decoder.has_animation() {
                return Ok(None);
            }
            Ok(decoder.into_frames())
        }
        _ => return Ok(None),
    }
    .map_err(|error| error.to_string())?;
    let mut decoded = Vec::new();
    let mut total = 0u64;
    for frame_result in frames {
        let image_frame = frame_result.map_err(|error| error.to_string())?;
        let (numerator, denominator) = image_frame.delay().numer_denom_ms();
        let millis = u64::from(numerator) / u64::from(denominator.max(1));
        // Browsers show 0 and 10 ms frames for 100 ms.
        let delay = Duration::from_millis(if millis <= 10 { 100 } else { millis });
        let buffer = image_frame.into_buffer();
        total += buffer.len() as u64;
        if total > MAX_DECODED_BYTES {
            break;
        }
        decoded.push(frame(buffer, delay));
    }
    Ok((decoded.len() > 1).then_some(decoded))
}

/// Pixels as a frame, scaled down to `MAX_SIDE`.
fn frame(image: image::RgbaImage, delay: Duration) -> Frame {
    let (width, height) = image.dimensions();
    let image = if width.max(height) > MAX_SIDE {
        let scale = MAX_SIDE as f32 / width.max(height) as f32;
        image::imageops::resize(
            &image,
            ((width as f32 * scale).round() as u32).max(1),
            ((height as f32 * scale).round() as u32).max(1),
            image::imageops::FilterType::Triangle,
        )
    } else {
        image
    };
    Frame {
        size: [image.width(), image.height()],
        rgba: image.into_raw().into(),
        delay,
    }
}

fn svg_fonts() -> Arc<resvg::usvg::fontdb::Database> {
    static FONTS: OnceLock<Arc<resvg::usvg::fontdb::Database>> = OnceLock::new();
    Arc::clone(FONTS.get_or_init(|| {
        let mut fonts = resvg::usvg::fontdb::Database::new();
        fonts.load_system_fonts();
        Arc::new(fonts)
    }))
}

fn svg_tree(bytes: &[u8]) -> Result<resvg::usvg::Tree, String> {
    let resolve_data = resvg::usvg::ImageHrefResolver::default_data_resolver();
    let options = resvg::usvg::Options {
        fontdb: svg_fonts(),
        image_href_resolver: resvg::usvg::ImageHrefResolver {
            // Only embedded images: an SVG never reads other files.
            resolve_string: Box::new(|_, _| None),
            resolve_data: Box::new(move |mime, data, options| {
                // Embedded rasters obey the same limits.
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
    resvg::usvg::Tree::from_data(bytes, &options)
        .map_err(|error| format!("It isn't a valid SVG: {error}"))
}

fn decode_svg(bytes: &[u8]) -> Result<Decoded, String> {
    let tree = svg_tree(bytes)?;
    let size = tree.size();
    let own = [size.width().ceil() as u32, size.height().ceil() as u32];
    // Sharp when zoomed in: rendered larger than its own size.
    let scale = (SVG_SIDE / size.width().max(size.height())).max(1.0);
    let pixels = [
        ((size.width() * scale).ceil() as u32).clamp(1, MAX_SIDE),
        ((size.height() * scale).ceil() as u32).clamp(1, MAX_SIDE),
    ];
    validate_dimensions(pixels[0], pixels[1])?;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(pixels[0], pixels[1])
        .ok_or("Couldn't make room for the SVG's pixels")?;
    let transform = resvg::tiny_skia::Transform::from_scale(
        pixels[0] as f32 / size.width(),
        pixels[1] as f32 / size.height(),
    );
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    Ok(Decoded {
        size: own,
        format: "SVG",
        frames: vec![Frame {
            size: pixels,
            // tiny-skia renders premultiplied pixels; egui wants straight.
            rgba: pixmap.take_demultiplied().into(),
            delay: Duration::ZERO,
        }],
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
    fn raster_formats_decode_by_signature() {
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
            let decoded =
                decode(&encoded(format)).unwrap_or_else(|error| panic!("{format:?}: {error}"));
            assert_eq!(decoded.size, [3, 2], "{format:?}");
            assert_eq!(decoded.frames.len(), 1, "{format:?}");
            assert_eq!(decoded.frames[0].rgba.len(), 24, "{format:?}");
        }
    }

    #[test]
    fn animated_gifs_keep_their_frames() {
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
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded.frames.len(), 2);
        assert_eq!(&decoded.frames[1].rgba[..4], &[0, 255, 0, 255]);
        assert!(decoded.frames[0].delay >= Duration::from_millis(100));
    }

    #[test]
    fn svgs_render_sharp_and_keep_their_size() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 4 2">
                <rect width="2" height="2" fill="#ff0000" fill-opacity="0.5"/>
            </svg>"##;
        let decoded = decode(svg).unwrap();
        assert_eq!(decoded.size, [4, 2]);
        assert_eq!(decoded.format, "SVG");
        assert_eq!(decoded.frames[0].size, [2048, 1024]);
        assert_eq!(&decoded.frames[0].rgba[..4], &[255, 0, 0, 128]);
    }

    #[test]
    fn huge_images_are_scaled_down_and_garbage_is_refused() {
        let wide = image::RgbaImage::from_pixel(MAX_SIDE * 2, 4, image::Rgba([1, 2, 3, 255]));
        let shown = frame(wide, Duration::ZERO);
        assert_eq!(shown.size, [MAX_SIDE, 2]);
        assert!(decode(b"not an image").is_err());
        assert!(decode(b"<svg").is_err());
        assert!(validate_dimensions(u32::MAX, u32::MAX).is_err());
        assert!(validate_dimensions(0, 1).is_err());
    }
}
