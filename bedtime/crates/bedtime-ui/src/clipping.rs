//! Rounded content clipping without an opaque cover over the panel corners.
use egui::{
    Context, LayerId, Pos2, Rect, Shape,
    epaint::{Mesh, Primitive, Vertex},
    layers::ShapeIdx,
};

pub(crate) fn finish(
    ctx: &Context,
    layer: LayerId,
    start: ShapeIdx,
    content_clip: Rect,
    outline: Rect,
    radius: egui::CornerRadius,
) {
    let mut boundary = Vec::new();
    egui::epaint::tessellator::path::rounded_rectangle(&mut boundary, outline, radius.into());
    let mut pending = Vec::new();
    ctx.graphics_mut(|graphics| {
        let list = graphics.entry(layer);
        for index in start.0..list.next_idx().0 {
            let index = ShapeIdx(index);
            list.mutate_shape(index, |shape| {
                shape.clip_rect = shape.clip_rect.intersect(content_clip);
                let bounds = shape
                    .shape
                    .visual_bounding_rect()
                    .intersect(shape.clip_rect);
                if !bounds.is_positive() {
                    shape.shape = Shape::Noop;
                } else if !bounds_inside(&boundary, bounds) {
                    pending.push((index, shape.clone()));
                }
            });
        }
    });
    // Only shapes touching a rounded corner need tessellation here. Interior
    // widgets keep egui's normal batching, glyph meshes and rectangular scissors.
    for (index, shape) in pending {
        let clip_rect = shape.clip_rect;
        let shapes = ctx
            .tessellate(vec![shape], ctx.pixels_per_point())
            .into_iter()
            .map(|primitive| match primitive.primitive {
                Primitive::Mesh(mesh) => Shape::mesh(clip_mesh(mesh, &boundary)),
                Primitive::Callback(_) => {
                    panic!("GPU callbacks at rounded clip boundaries must use UiViewport images")
                }
            })
            .collect();
        ctx.graphics_mut(|g| g.entry(layer).set(index, clip_rect, Shape::Vec(shapes)));
    }
}

fn distance(a: Pos2, b: Pos2, p: Pos2) -> f32 {
    let edge = b - a;
    let point = p - a;
    edge.x * point.y - edge.y * point.x
}
fn contains(boundary: &[Pos2], point: Pos2) -> bool {
    boundary
        .iter()
        .zip(boundary.iter().cycle().skip(1))
        .all(|(&a, &b)| distance(a, b, point) >= -0.001 * (b - a).length())
}
fn bounds_inside(boundary: &[Pos2], bounds: Rect) -> bool {
    [
        bounds.left_top(),
        bounds.right_top(),
        bounds.right_bottom(),
        bounds.left_bottom(),
    ]
    .into_iter()
    .all(|p| contains(boundary, p))
}

/// Intersect triangles with the convex rounded outline, preserving texture UVs
/// and premultiplied vertex colors. Interior triangles retain their indices.
fn clip_mesh(mut mesh: Mesh, boundary: &[Pos2]) -> Mesh {
    let mut indices = Vec::with_capacity(mesh.indices.len());
    let mut polygon = Vec::with_capacity(boundary.len() + 3);
    let mut next = Vec::with_capacity(boundary.len() + 3);
    for triangle in mesh.indices.chunks_exact(3) {
        polygon.clear();
        polygon.extend(triangle.iter().map(|&i| mesh.vertices[i as usize]));
        if polygon.iter().all(|v| contains(boundary, v.pos)) {
            indices.extend_from_slice(triangle);
            continue;
        }
        // Sutherland–Hodgman clipping: each outline edge keeps its interior half-plane.
        for (&a, &b) in boundary.iter().zip(boundary.iter().cycle().skip(1)) {
            if polygon.is_empty() {
                break;
            }
            next.clear();
            let mut previous = *polygon.last().unwrap();
            let mut previous_distance = distance(a, b, previous.pos);
            for &current in &polygon {
                let current_distance = distance(a, b, current.pos);
                if (previous_distance >= 0.0) != (current_distance >= 0.0) {
                    let t = previous_distance / (previous_distance - current_distance);
                    next.push(Vertex {
                        pos: previous.pos.lerp(current.pos, t),
                        uv: previous.uv.lerp(current.uv, t),
                        color: previous.color.lerp_to_gamma(current.color, t),
                    });
                }
                if current_distance >= 0.0 {
                    next.push(current);
                }
                previous = current;
                previous_distance = current_distance;
            }
            std::mem::swap(&mut polygon, &mut next);
        }
        if polygon.len() >= 3 {
            let base = mesh.vertices.len() as u32;
            mesh.vertices.extend_from_slice(&polygon);
            for i in 1..polygon.len() as u32 - 1 {
                indices.extend([base, base + i, base + i + 1]);
            }
        }
    }
    mesh.indices = indices;
    if mesh.indices.is_empty() {
        mesh.vertices.clear();
    }
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Theme, UiFrame, icon_cache};
    use bevy::prelude::{Entity, default};
    use std::sync::Mutex;

    #[test]
    fn rounded_clipping_contains_images_text_and_custom_paint_without_moving_their_uvs() {
        for dpi in [1.0, 1.5, 2.0] {
            for radius in [0.0, 7.0, 40.0, 200.0] {
                let ctx = Context::default();
                ctx.set_pixels_per_point(dpi);
                let theme = Theme::dark();
                let icons = Mutex::new(icon_cache::IconCache::default());
                let outline = Rect::from_min_max(egui::pos2(50.0, 60.0), egui::pos2(250.0, 180.0));
                let content = outline.with_min_y(90.0);
                let image = outline.expand(30.0);
                let texture = egui::TextureId::User(42);
                let tint = egui::Color32::from_rgba_premultiplied(90, 60, 30, 128);
                let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                    ui.set_clip_rect(content);
                    let mut frame = UiFrame {
                        view: Entity::PLACEHOLDER,
                        surface: Entity::PLACEHOLDER,
                        ui,
                        theme: &theme,
                        metrics: &crate::Metrics::default(),
                        motion: default(),
                        icons: &icons,
                    };
                    frame.clip_rounded(outline, radius, |frame| {
                        // Oversized application painting cannot widen the host's boundary.
                        frame.ui.set_clip_rect(Rect::EVERYTHING);
                        frame.ui.painter().image(
                            texture,
                            image,
                            Rect::from_min_max(Pos2::ZERO, egui::pos2(1.0, 1.0)),
                            tint,
                        );
                        frame
                            .ui
                            .painter()
                            .rect_filled(image, 0.0, egui::Color32::RED);
                        frame.ui.painter().text(
                            outline.left_bottom() - egui::vec2(5.0, 8.0),
                            egui::Align2::LEFT_BOTTOM,
                            "Clipped text",
                            egui::FontId::default(),
                            egui::Color32::WHITE,
                        );
                    });
                });
                output.textures_delta.clear();
                let meshes =
                    ctx.tessellate(std::mem::take(&mut output.shapes), output.pixels_per_point);
                let mut image_vertices = 0;
                for primitive in meshes {
                    assert_eq!(
                        primitive.clip_rect, content,
                        "The content scissor cannot expand into the tab bar"
                    );
                    let Primitive::Mesh(mesh) = primitive.primitive else {
                        panic!("Mesh output");
                    };
                    assert!(!mesh.indices.is_empty());
                    for &index in &mesh.indices {
                        let vertex = mesh.vertices[index as usize];
                        // Rectangular scissoring can leave vertices outside its bounds.
                        // Project those to the visible rectangle before checking rounding.
                        if radius > 0.0 {
                            let r = radius
                                .min(outline.width() * 0.5)
                                .min(outline.height() * 0.5);
                            let pos = primitive.clip_rect.clamp(vertex.pos);
                            let closest = outline.shrink(r).clamp(pos);
                            assert!(
                                (pos - closest).length() <= r + 0.002,
                                "Vertex outside rounded panel: {:?}",
                                vertex.pos
                            );
                        }
                        if mesh.texture_id == texture {
                            image_vertices += 1;
                            let expected = (vertex.pos - image.min) / image.size();
                            assert!((vertex.uv.to_vec2() - expected).length() < 0.0001);
                            assert_eq!(vertex.color, tint);
                        }
                    }
                }
                assert!(
                    image_vertices > 6 || radius == 0.0,
                    "The full rectangular image has its corners cut"
                );
            }
        }
    }

    #[test]
    fn clipping_is_local_to_content_and_keeps_overlays_and_following_paint() {
        let ctx = Context::default();
        let theme = Theme::dark();
        let icons = Mutex::new(icon_cache::IconCache::default());
        let outline = Rect::from_min_size(egui::pos2(40.0, 50.0), egui::vec2(100.0, 80.0));
        let outside = Rect::from_min_size(egui::pos2(220.0, 50.0), egui::vec2(60.0, 60.0));
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let mut frame = UiFrame {
                view: Entity::PLACEHOLDER,
                surface: Entity::PLACEHOLDER,
                ui,
                theme: &theme,
                metrics: &crate::Metrics::default(),
                motion: default(),
                icons: &icons,
            };
            frame.clip_rounded(outline, 20.0, |mut frame| {
                frame.scroll("nested", 7.0, |frame| {
                    frame
                        .ui
                        .painter()
                        .rect_filled(Rect::EVERYTHING, 0.0, egui::Color32::RED);
                });
                let popup = frame.ui.ctx().layer_painter(LayerId::new(
                    egui::Order::Foreground,
                    egui::Id::new("popup"),
                ));
                popup.rect_filled(outside, 0.0, egui::Color32::GREEN);
            });
            frame
                .ui
                .painter()
                .rect_filled(outside, 0.0, egui::Color32::BLUE);
        });
        output.textures_delta.clear();
        for color in [egui::Color32::GREEN, egui::Color32::BLUE] {
            assert!(output.shapes.iter().any(|shape| {
                matches!(&shape.shape, Shape::Rect(rect) if rect.rect == outside && rect.fill == color)
                    && shape.clip_rect.contains_rect(outside)
            }), "Overlays and subsequent shell painting keep their original bounds");
        }
    }
}
