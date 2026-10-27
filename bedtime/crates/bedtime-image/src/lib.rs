//! Static image decoding and an embeddable, toolbar-free UI canvas.
//! Decoding/rasterization is explicit and must run on a host-owned worker.
use bedtime_ui::egui::{self, Pos2, Rect, Vec2};
use bevy::prelude::Component;
use std::sync::Arc;
mod decode;
pub use decode::{SUPPORTED_EXTENSIONS, decode, supported_path};

pub const MAX_DECODED_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct DecodedImage {
    pub size: [u32; 2],
    /// Straight-alpha sRGBA.
    pub rgba: Arc<[u8]>,
}

/// Immutable content; views share this while retaining independent transforms.
pub struct ImageContent(ImageKind);

enum ImageKind {
    Raster(DecodedImage),
    Svg(resvg::usvg::Tree),
}
impl std::fmt::Debug for ImageContent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImageContent")
            .field("dimensions", &self.dimensions())
            .finish()
    }
}
impl ImageContent {
    pub fn prepare(bytes: &[u8], path: &str) -> Result<Self, String> {
        if path
            .rsplit('.')
            .next()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("svg"))
        {
            Ok(Self(ImageKind::Svg(decode::svg_tree(bytes)?)))
        } else {
            Ok(Self(ImageKind::Raster(decode(bytes, path)?)))
        }
    }
    pub fn dimensions(&self) -> Vec2 {
        match &self.0 {
            ImageKind::Raster(image) => Vec2::new(image.size[0] as f32, image.size[1] as f32),
            ImageKind::Svg(tree) => Vec2::new(tree.size().width(), tree.size().height()),
        }
    }
    /// Requested physical dimensions are reduced proportionally to the pixel budget.
    pub fn render(&self, requested: [u32; 2]) -> Result<DecodedImage, String> {
        match &self.0 {
            ImageKind::Raster(image) => Ok(image.clone()),
            ImageKind::Svg(tree) => {
                let size = bounded_size(requested);
                decode::rasterize_svg(tree, (size[0], size[1]))
            }
        }
    }
    pub fn is_svg(&self) -> bool {
        matches!(&self.0, ImageKind::Svg(_))
    }
}

pub fn bounded_size(size: [u32; 2]) -> [u32; 2] {
    let width = size[0].max(1) as f64;
    let height = size[1].max(1) as f64;
    let factor = ((MAX_DECODED_BYTES / 4) as f64 / (width * height))
        .sqrt()
        .min(1.0);
    [
        (width * factor).floor().max(1.0) as u32,
        (height * factor).floor().max(1.0) as u32,
    ]
}

/// The image coordinate at the canvas center, and zoom relative to containment.
#[derive(Clone, Debug)]
pub struct ImageTransform {
    zoom: f32,
    center: Vec2,
}
impl Default for ImageTransform {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            center: Vec2::splat(0.5),
        }
    }
}
impl ImageTransform {
    pub fn zoom(&self) -> f32 {
        self.zoom
    }
    pub fn center(&self) -> Vec2 {
        self.center
    }
    pub fn reset(&mut self) {
        *self = Self::default();
    }
    fn scale(&self, viewport: Vec2, image: Vec2) -> f32 {
        (viewport.x / image.x)
            .min(viewport.y / image.y)
            .max(f32::MIN_POSITIVE)
            * self.zoom
    }
    pub fn image_rect(&self, viewport: Rect, image: Vec2) -> Rect {
        let size = image * self.scale(viewport.size(), image);
        Rect::from_min_size(viewport.center() - self.center * size, size)
    }
    pub fn zoom_at(&mut self, viewport: Rect, image: Vec2, anchor: Pos2, factor: f32) {
        if !factor.is_finite() || factor <= 0.0 {
            return;
        }
        let old = self.image_rect(viewport, image);
        let coordinate = (anchor - old.min) / old.size();
        self.zoom = (self.zoom * factor).clamp(0.01, 100.0);
        let size = image * self.scale(viewport.size(), image);
        self.center = coordinate - (anchor - viewport.center()) / size;
    }
    pub fn pan(&mut self, viewport: Rect, image: Vec2, delta: Vec2) {
        if !delta.is_finite() {
            return;
        }
        self.center -= delta / (image * self.scale(viewport.size(), image));
    }
}

/// ECS-owned presentation. Texture handles retire with the view; no global cache.
#[derive(Component, Default)]
pub struct ImageView {
    pub transform: ImageTransform,
    texture: Option<egui::TextureHandle>,
    pending: Option<DecodedImage>,
    dimensions: Option<Vec2>,
    rect: Option<Rect>,
    requested: [u32; 2],
}
impl ImageView {
    pub fn set_image(&mut self, pixels: DecodedImage, dimensions: Vec2) {
        self.dimensions = Some(dimensions);
        self.pending = Some(pixels);
    }
    pub fn has_image(&self) -> bool {
        self.dimensions.is_some()
    }
    pub fn rect(&self) -> Option<Rect> {
        self.rect
    }
    pub fn requested_size(&self) -> [u32; 2] {
        self.requested
    }
    pub fn dimensions(&self) -> Option<Vec2> {
        self.dimensions
    }
    /// Draws the complete available canvas and returns its response for host menus.
    pub fn draw(&mut self, ui: &mut egui::Ui, id: impl egui::AsId) -> egui::Response {
        if let Some(pixels) = self.pending.take() {
            let color = egui::ColorImage::from_rgba_unmultiplied(
                [pixels.size[0] as usize, pixels.size[1] as usize],
                &pixels.rgba,
            );
            if let Some(texture) = &mut self.texture {
                texture.set(color, egui::TextureOptions::LINEAR);
            } else {
                self.texture = Some(ui.ctx().load_texture(
                    format!("image:{:?}", egui::Id::new(id)),
                    color,
                    egui::TextureOptions::LINEAR,
                ));
            }
        }
        let (rect, response) = ui.allocate_exact_size(
            ui.available_size().max(Vec2::splat(1.0)),
            egui::Sense::click_and_drag(),
        );
        self.rect = Some(rect);
        let Some(image) = self.dimensions else {
            return response;
        };
        let eligible = ui.is_enabled()
            && ui.input(|i| i.focused)
            && !ui.ctx().any_popup_open()
            && ui.ctx().memory(|m| m.top_modal_layer().is_none());
        if eligible {
            if response.double_clicked() {
                self.transform.reset();
            }
            if response.hovered() {
                let (scroll, pinch, pointer) = ui.input(|i| {
                    (
                        i.smooth_scroll_delta.y,
                        i.zoom_delta(),
                        i.pointer.hover_pos(),
                    )
                });
                if let Some(pointer) = pointer {
                    self.transform
                        .zoom_at(rect, image, pointer, (scroll * 0.002).exp() * pinch);
                    ui.input_mut(|i| {
                        i.smooth_scroll_delta = Vec2::ZERO;
                    });
                }
            }
            if response.dragged_by(egui::PointerButton::Primary) {
                self.transform
                    .pan(rect, image, ui.input(|i| i.pointer.delta()));
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            } else if response.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
            }
        }
        let target = self.transform.image_rect(rect, image);
        let physical = target.size() * ui.ctx().pixels_per_point();
        self.requested = bounded_size([
            physical.x.ceil().max(1.0) as u32,
            physical.y.ceil().max(1.0) as u32,
        ]);
        if let Some(texture) = &self.texture {
            ui.painter()
                .with_clip_rect(rect.intersect(ui.clip_rect()))
                .image(
                    texture.id(),
                    target,
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
        }
        response
    }
}

#[cfg(feature = "workbench")]
pub mod workbench {
    use super::*;
    use bedtime_workbench::{Panel, PanelInstance};
    use bevy::prelude::*;
    #[derive(Component, Default)]
    #[require(PanelInstance = PanelInstance::of::<Self>(), ImageView)]
    pub struct ImagePanel;
    impl Panel for ImagePanel {
        const KEY: &'static str = "bedtime.image";
        const LABEL: &'static str = "Image";
        fn draw(
            ui: &mut egui::Ui,
            world: &mut World,
            _: &bedtime_ui::UiContext<'_>,
            panel: Entity,
        ) {
            world.get_mut::<ImageView>(panel).unwrap().draw(ui, panel);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cursor_anchor_survives_zoom_and_clamping() {
        let viewport = Rect::from_min_size(Pos2::new(20.0, 30.0), Vec2::new(800.0, 600.0));
        let image = Vec2::new(1600.0, 900.0);
        let pointer = Pos2::new(250.0, 170.0);
        let mut transform = ImageTransform::default();
        transform.pan(viewport, image, Vec2::new(17.0, -23.0));
        for factor in [2.0, 0.7, 10000.0, 0.000001] {
            let before = transform.image_rect(viewport, image);
            let coordinate = (pointer - before.min) / before.size();
            transform.zoom_at(viewport, image, pointer, factor);
            let after = transform.image_rect(viewport, image);
            assert!(((pointer - after.min) / after.size() - coordinate).length() < 0.001);
        }
    }
    #[test]
    fn contain_resize_pan_and_reset() {
        let image = Vec2::new(200.0, 100.0);
        let viewport = Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 400.0));
        let mut transform = ImageTransform::default();
        let fitted = transform.image_rect(viewport, image);
        assert_eq!(fitted.size(), Vec2::new(400.0, 200.0));
        assert_eq!(fitted.center(), viewport.center());
        transform.pan(viewport, image, Vec2::new(50.0, 20.0));
        assert_eq!(
            transform.image_rect(viewport, image).min,
            fitted.min + Vec2::new(50.0, 20.0)
        );
        let center = transform.center();
        let resized = Rect::from_min_size(Pos2::ZERO, Vec2::new(600.0, 300.0));
        let rendered = transform.image_rect(resized, image);
        assert!(((resized.center() - rendered.min) / rendered.size() - center).length() < 0.0001);
        transform.reset();
        assert_eq!(
            transform.image_rect(resized, image).center(),
            resized.center()
        );
    }
    #[test]
    fn vector_resolution_is_bounded() {
        let size = bounded_size([100000, 200000]);
        assert!(u64::from(size[0]) * u64::from(size[1]) * 4 <= MAX_DECODED_BYTES);
        let source = ImageContent::prepare(br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 5"><rect width="10" height="5" fill="red"/></svg>"#, "test.svg").unwrap();
        assert_eq!(source.render([200, 100]).unwrap().size, [200, 100]);
    }
}

#[cfg(feature = "workbench")]
pub mod document;
