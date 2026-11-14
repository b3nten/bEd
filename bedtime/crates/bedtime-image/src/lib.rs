//! An image viewer for workbench apps: an image file in a panel, drawn by
//! `bedtime_ui::image::ImageView` (zoom around the pointer, pan, fit or actual
//! size, sharp pixels when zoomed in, animations playing). The panel adds the
//! file formats, Zoom In/Out/Reset Zoom acting on the picture, and keeping the
//! backdrop and smoothing with the layout.
use bedtime_ui::{UiContext, egui, image::ImageView};
#[cfg(feature = "persistence")]
use bedtime_workbench::persistence::ReflectPersist;
use bedtime_workbench::{
    ActionHandler, ActionInvocation, ActionResult, Handler, Panel, PanelInstance,
    PanelPresentation, ResetZoom, ViewersAppExt, ZoomIn, ZoomOut, viewer_document,
};
use bevy::prelude::*;

mod picture;
pub use bedtime_ui::image::{
    Backdrop, Decoded, Frame, ImageTransform, MAX_DECODED_BYTES, SUPPORTED_EXTENSIONS, decode,
    supported,
};
pub use picture::Picture;

pub struct ImagePlugin;

/// Images: the extensions in `SUPPORTED_EXTENSIONS` but SVG.
pub const IMAGE: bedtime_workbench::FormatId = bedtime_workbench::FormatId("image");
/// SVG: an image, and XML (so the editor opens it too).
pub const SVG: bedtime_workbench::FormatId = bedtime_workbench::FormatId("svg");

impl Plugin for ImagePlugin {
    fn build(&self, app: &mut App) {
        use bedtime_workbench::{Format, FormatId, FormatsAppExt};
        let raster = SUPPORTED_EXTENSIONS
            .iter()
            .copied()
            .filter(|&ext| ext != "svg");
        app.register_type::<ImagePanel>()
            .register_format(Format::new(IMAGE).extensions(raster))
            .register_format(
                Format::new(SVG)
                    .extensions(["svg"])
                    .conforms_to([IMAGE, FormatId("xml")]),
            )
            .register_viewer::<Picture, ImagePanel>("Image")
            .formats([IMAGE]);
    }
}

#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
#[cfg_attr(feature = "persistence", reflect(Persist))]
#[require(
    PanelInstance = PanelInstance::of::<Self>(),
    // Zooming zooms the picture, rather than the panel's zoom.
    ActionHandler<ZoomIn> = ActionHandler::new(Handler::new(|w, i| ask(w, i, ZoomAsk::In))),
    ActionHandler<ZoomOut> = ActionHandler::new(Handler::new(|w, i| ask(w, i, ZoomAsk::Out))),
    ActionHandler<ResetZoom> = ActionHandler::new(Handler::new(|w, i| ask(w, i, ZoomAsk::Fit))),
)]
pub struct ImagePanel {
    pub backdrop: Backdrop,
    /// Smooth pixels when zoomed in, rather than showing them sharp.
    pub smooth: bool,
    /// Where it's zoomed to, playing, and its textures.
    #[reflect(ignore)]
    pub view: ImageView,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum ZoomAsk {
    In,
    Out,
    Fit,
}

fn ask(world: &mut World, invocation: ActionInvocation, zoom: ZoomAsk) -> ActionResult {
    let mut image = invocation
        .entity
        .and_then(|panel| world.get_mut::<ImagePanel>(panel))
        .ok_or("No image is focused")?;
    match zoom {
        ZoomAsk::In => image.view.zoom_in(),
        ZoomAsk::Out => image.view.zoom_out(),
        ZoomAsk::Fit => image.view.fit(),
    }
    Ok(())
}

impl ImagePanel {
    /// The frame showing, of how many.
    pub fn frame(&self) -> Option<(usize, usize)> {
        self.view.frame()
    }
}

impl Panel for ImagePanel {
    const LABEL: &'static str = "Image";

    fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, panel: Entity) {
        let Some(document) = viewer_document::<Picture>(ui, world, panel) else {
            return;
        };
        let Some(picture) = document.model.0 else {
            return;
        };
        let focused = world
            .get::<PanelPresentation>(panel)
            .is_some_and(|presentation| presentation.focused);
        if let Some(mut image) = world.get_mut::<ImagePanel>(panel) {
            let image = &mut *image;
            image.view.backdrop = image.backdrop;
            image.view.smooth = image.smooth;
            image.view.show(ui, context, &picture, focused);
            // The menu may have changed them.
            image.backdrop = image.view.backdrop;
            image.smooth = image.view.smooth;
        }
    }
}
