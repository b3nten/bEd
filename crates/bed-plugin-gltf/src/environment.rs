//! Offline Poly Haven HDRIs, converted once to Bevy's floating point cubemaps.
use bevy::{
    asset::RenderAssetUsages,
    image::ImageSampler,
    prelude::{Image, Vec3},
    render::render_resource::{
        Extent3d, TextureDimension, TextureFormat, TextureViewDescriptor, TextureViewDimension,
    },
};
use std::sync::OnceLock;

const SIDE: u32 = 256;
const SOURCES: [&[u8]; 2] = [
    include_bytes!("../assets/environments/studio_small_08_1k.hdr"),
    include_bytes!("../assets/environments/kiara_1_dawn_1k.hdr"),
];
static CUBEMAPS: [OnceLock<Result<Image, String>>; 2] = [const { OnceLock::new() }; 2];

/// Studio Small 08 (0) and Kiara 1 Dawn (1), including the original HDR range.
pub(super) fn bundled(index: usize) -> Result<Image, String> {
    let source = SOURCES
        .get(index)
        .ok_or_else(|| "Unknown bundled HDRI".to_string())?;
    CUBEMAPS[index].get_or_init(|| decode(source)).clone()
}

fn decode(source: &[u8]) -> Result<Image, String> {
    let panorama = image::load_from_memory_with_format(source, image::ImageFormat::Hdr)
        .map_err(|error| format!("Could not decode bundled HDRI: {error}"))?
        .into_rgb32f();
    let mut rgba = Vec::with_capacity((SIDE * SIDE * 6 * 8) as usize);
    for face in 0..6 {
        for y in 0..SIDE {
            for x in 0..SIDE {
                let u = 2.0 * (x as f32 + 0.5) / SIDE as f32 - 1.0;
                let v = 2.0 * (y as f32 + 0.5) / SIDE as f32 - 1.0;
                let color = sample(&panorama, direction(face, u, v));
                for value in color.into_iter().chain([1.0]) {
                    // Rgba16Float is filterable on all of Bevy's supported HDR
                    // adapters. Preserve radiance instead of tonemapping to 8-bit.
                    rgba.extend_from_slice(
                        &half::f16::from_f32(value.clamp(0.0, 65504.0)).to_le_bytes(),
                    );
                }
            }
        }
    }
    let mut image = Image::new(
        Extent3d {
            width: SIDE,
            height: SIDE,
            depth_or_array_layers: 6,
        },
        TextureDimension::D2,
        rgba,
        TextureFormat::Rgba16Float,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.texture_view_descriptor = Some(TextureViewDescriptor {
        dimension: Some(TextureViewDimension::Cube),
        ..Default::default()
    });
    image.sampler = ImageSampler::linear();
    Ok(image)
}

/// Cube layer order and orientation: +X, -X, +Y, -Y, +Z, -Z.
fn direction(face: usize, u: f32, v: f32) -> Vec3 {
    match face {
        0 => Vec3::new(1.0, -v, -u),
        1 => Vec3::new(-1.0, -v, u),
        2 => Vec3::new(u, 1.0, v),
        3 => Vec3::new(u, -1.0, -v),
        4 => Vec3::new(u, -v, 1.0),
        _ => Vec3::new(-u, -v, -1.0),
    }
    .normalize()
}

/// Bilinear panorama lookup wraps horizontally and clamps at the poles.
fn sample(image: &image::Rgb32FImage, direction: Vec3) -> [f32; 3] {
    let (width, height) = image.dimensions();
    let u = direction.z.atan2(direction.x) / std::f32::consts::TAU + 0.5;
    let v = direction.y.clamp(-1.0, 1.0).acos() / std::f32::consts::PI;
    let x = u * width as f32 - 0.5;
    let y = v * height as f32 - 0.5;
    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let pixel = |x: i32, y: i32| {
        image
            .get_pixel(
                x.rem_euclid(width as i32) as u32,
                y.clamp(0, height as i32 - 1) as u32,
            )
            .0
    };
    let top_left = pixel(x0, y0);
    let top_right = pixel(x0 + 1, y0);
    let bottom_left = pixel(x0, y0 + 1);
    let bottom_right = pixel(x0 + 1, y0 + 1);
    let tx = x - x.floor();
    let ty = y - y.floor();
    std::array::from_fn(|channel| {
        let top = top_left[channel] * (1.0 - tx) + top_right[channel] * tx;
        let bottom = bottom_left[channel] * (1.0 - tx) + bottom_right[channel] * tx;
        top * (1.0 - ty) + bottom * ty
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_environments_preserve_hdr_and_cube_layout() {
        for index in 0..SOURCES.len() {
            let image = bundled(index).unwrap();
            assert_eq!(image.texture_descriptor.format, TextureFormat::Rgba16Float);
            assert_eq!(image.texture_descriptor.size.depth_or_array_layers, 6);
            assert_eq!(image.texture_descriptor.size.width, SIDE);
            let pixels = image.data.unwrap();
            assert_eq!(pixels.len(), (SIDE * SIDE * 6 * 8) as usize);
            let values = pixels
                .as_chunks::<2>()
                .0
                .iter()
                .map(|bytes| half::f16::from_le_bytes(*bytes).to_f32());
            let mut maximum = 0.0f32;
            for value in values {
                assert!(value.is_finite() && value >= 0.0);
                maximum = maximum.max(value);
            }
            assert!(maximum > 1.0, "HDR range was lost: {maximum}");
        }
    }

    #[test]
    fn cube_faces_meet_and_panorama_wraps_without_a_seam() {
        assert_eq!(direction(0, 0.0, 0.0), Vec3::X);
        assert_eq!(direction(1, 0.0, 0.0), Vec3::NEG_X);
        assert_eq!(direction(2, 0.0, 0.0), Vec3::Y);
        assert_eq!(direction(3, 0.0, 0.0), Vec3::NEG_Y);
        assert_eq!(direction(4, 0.0, 0.0), Vec3::Z);
        assert_eq!(direction(5, 0.0, 0.0), Vec3::NEG_Z);
        assert_eq!(direction(0, -1.0, 0.0), direction(4, 1.0, 0.0));
        assert_eq!(direction(0, 1.0, 0.0), direction(5, -1.0, 0.0));
        let image = image::Rgb32FImage::from_fn(4, 2, |x, _| image::Rgb([x as f32, 2.0, 3.0]));
        let left = sample(&image, Vec3::new(-1.0, 0.0, -1e-7).normalize());
        let right = sample(&image, Vec3::new(-1.0, 0.0, 1e-7).normalize());
        assert!((left[0] - right[0]).abs() < 1e-5);
    }
}
