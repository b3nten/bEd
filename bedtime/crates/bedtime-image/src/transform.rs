//! Where an image is in its canvas: zoom relative to fitting it, and which
//! point of it is at the center.
use bedtime_ui::egui::{Pos2, Rect, Vec2};

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
    /// Points per image pixel.
    pub fn scale(&self, viewport: Vec2, image: Vec2) -> f32 {
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
    /// Zoom so `scale` points show one image pixel, keeping the point under
    /// `anchor` where it is.
    pub fn zoom_to(&mut self, viewport: Rect, image: Vec2, anchor: Pos2, scale: f32) {
        let factor = scale / self.scale(viewport.size(), image);
        self.zoom_at(viewport, image, anchor, factor);
    }

    pub fn pan(&mut self, viewport: Rect, image: Vec2, delta: Vec2) {
        if !delta.is_finite() {
            return;
        }
        self.center -= delta / (image * self.scale(viewport.size(), image));
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
    fn zooming_to_a_scale() {
        let viewport = Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 400.0));
        let image = Vec2::new(200.0, 100.0);
        let mut transform = ImageTransform::default();
        transform.zoom_to(viewport, image, viewport.center(), 1.0);
        assert!((transform.image_rect(viewport, image).size() - image).length() < 0.01);
    }
}
