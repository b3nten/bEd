//! Crisp text inside an [`egui::Scene`].
//!
//! A `Scene` pans and zooms by transforming its whole paint layer, and that transform scales the
//! rasterised glyphs along with everything else: at any zoom but 1× the text is a magnified (or
//! shrunk) bitmap, which looks blurry (egui#4813).
//!
//! [`SceneText`] sidesteps it. It lays each string out at the size the string ends up on screen
//! and paints it, already in screen coordinates, on an untransformed sublayer directly above the
//! scene. Glyphs are therefore rasterised at their final size, so text stays sharp however far the
//! view is zoomed. Call sites keep working in scene coordinates; only the text goes through here.

use std::sync::Arc;

use bedtime_ui::egui::{
    Align2, Color32, FontId, Galley, LayerId, Painter, Pos2, Rect, Ui, Vec2, emath::TSTransform,
    epaint::TextShape,
};

pub(super) struct SceneText {
    painter: Painter,
    /// Scene coordinates → screen coordinates.
    to_global: TSTransform,
}

impl SceneText {
    /// Creates the text layer for the scene `ui` is drawing into.
    pub fn new(ui: &Ui) -> Self {
        let ctx = ui.ctx();
        let scene = ui.layer_id();
        let to_global = ctx.layer_transform_to_global(scene).unwrap_or_default();
        // A sublayer of the scene paints directly above it and keeps its own (identity) transform.
        let layer = LayerId::new(scene.order, scene.id.with("schedules_text"));
        ctx.set_sublayer(scene, layer);
        let painter = Painter::new(ctx.clone(), layer, to_global * ui.clip_rect());
        Self { painter, to_global }
    }

    /// The scene's current zoom: one scene unit is this many screen points.
    pub fn scale(&self) -> f32 {
        self.to_global.scaling
    }

    /// Lays out one line at its on-screen size. Measure it with [`Self::size`], which converts
    /// back to scene units.
    pub fn layout(&self, text: impl ToString, font: &FontId, color: Color32) -> Arc<Galley> {
        let font = FontId::new(font.size * self.scale(), font.family.clone());
        self.painter.layout_no_wrap(text.to_string(), font, color)
    }

    /// The size of a laid-out galley in scene units.
    pub fn size(&self, galley: &Galley) -> Vec2 {
        galley.size() / self.scale()
    }

    /// Paints a galley from [`Self::layout`] with its top-left at `pos` in scene coordinates.
    pub fn galley(&self, pos: Pos2, galley: Arc<Galley>, color: Color32) {
        self.painter.galley(self.to_global * pos, galley, color);
    }

    /// Paints a galley rotated by `angle` radians about `pos` (scene coordinates).
    pub fn rotated(&self, pos: Pos2, galley: Arc<Galley>, color: Color32, angle: f32) {
        self.painter
            .add(TextShape::new(self.to_global * pos, galley, color).with_angle(angle));
    }

    /// Paints `text` anchored at `pos` (scene coordinates), like [`Painter::text`].
    pub fn text(
        &self,
        pos: Pos2,
        anchor: Align2,
        text: impl ToString,
        font: &FontId,
        color: Color32,
    ) {
        let galley = self.layout(text, font, color);
        let rect = anchor.anchor_size(self.to_global * pos, galley.size());
        self.painter.galley(rect.min, galley, color);
    }

    /// A copy that only paints inside `rect` (scene coordinates), like [`Painter::with_clip_rect`].
    pub fn clipped(&self, rect: Rect) -> Self {
        Self {
            painter: self
                .painter
                .with_clip_rect((self.to_global * rect).intersect(self.painter.clip_rect())),
            to_global: self.to_global,
        }
    }
}
