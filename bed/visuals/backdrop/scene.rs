//! What bEd's 3D scenes share: each draws into an image of its own, on a
//! render layer no other camera sees, and the chosen one's image is the
//! window's backdrop. The cat uses the same render layers.
use super::{FRAME_RATE, flyover, internals};
use bedtime_ui::{MotionPolicy, Theme, layouts::LayoutSurface, theme::Rgba};
use bedtime_workbench::LayoutCamera;
use bevy::{camera::visibility::RenderLayers, prelude::*};
use std::time::Duration;

/// bEd's scenes: each one's `Backdrop` setting and name in Settings.
pub const SCENES: &[(&str, &str)] = &[
    (flyover::NAME, flyover::LABEL),
    (internals::NAME, internals::LABEL),
];

/// What the chosen scene's camera draws into: the window's backdrop.
#[derive(Resource)]
pub struct SceneImage(pub Handle<Image>);

/// A scene that stopped takes its image back, unless another scene's is
/// already there in its place.
pub fn retire(world: &mut World, image: &Handle<Image>) {
    if world
        .get_resource::<SceneImage>()
        .is_some_and(|shown| shown.0 == *image)
    {
        world.remove_resource::<SceneImage>();
    }
}

/// A render layer nothing uses yet: a scene's camera sees only its scene, and
/// no other camera sees it.
pub fn free_layer(world: &mut World) -> RenderLayers {
    let used: Vec<_> = world
        .query::<&RenderLayers>()
        .iter(world)
        .cloned()
        .collect();
    let layer = (1..)
        .find(|&layer| {
            used.iter()
                .all(|layers| !layers.intersects(&RenderLayers::layer(layer)))
        })
        .unwrap();
    RenderLayers::layer(layer)
}

/// Ask for the window's next frame, for a scene that moves.
pub fn request_frame(world: &mut World) {
    let frame = Duration::from_secs_f32(1.0 / FRAME_RATE);
    let mut contexts = world.query_filtered::<&mut bevy_egui::EguiContext, With<LayoutCamera>>();
    for mut context in contexts.iter_mut(world) {
        context.get_mut().request_repaint_after(frame);
    }
}

/// The first layout surface's theme, and whether animations are on.
pub fn surface_style(world: &mut World) -> Option<(Theme, bool)> {
    world
        .query_filtered::<(&Theme, Option<&MotionPolicy>), With<LayoutSurface>>()
        .iter(world)
        .next()
        .map(|(theme, motion)| (theme.clone(), motion.is_none_or(|motion| motion.enabled)))
}

pub fn linear(color: Rgba) -> LinearRgba {
    let [r, g, b, _] = color;
    Color::srgb(r, g, b).to_linear()
}

/// A value from 0 to 1 for a cell, the same each time.
pub fn hash(x: i64, z: i64) -> f32 {
    let mut n = (x.wrapping_mul(374_761_393) ^ z.wrapping_mul(668_265_263)) as u64;
    n = (n ^ (n >> 13)).wrapping_mul(1_274_126_177);
    ((n ^ (n >> 16)) & 0xffff) as f32 / 65535.0
}
