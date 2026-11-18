//! A diagram canvas for workbench apps: `.diagram` files (RON) of shapes,
//! lines, arrows, text and freehand strokes, in theme colors, on a canvas
//! that pans and zooms. Boxes, ellipses, diamonds, lines and arrows are drawn
//! with the tools, and selected, moved, resized, styled and deleted; each is a
//! step of the document's undo.
//!
//! `model` is the document (elements, changes, the file format), `edit` the
//! arithmetic of editing one, `canvas` draws one through a camera and turns
//! gestures into edits, and the panel here ties them to the workbench: the
//! file format, New Diagram, Zoom In/Out/Reset Zoom, making the canvas's
//! edits, and keeping where it's looked at from with the layout. `export`
//! writes a diagram as an SVG; the panel's Export and Copy actions save
//! that, or the PNG it renders to, or put the PNG on the clipboard.
use bedtime_ui::{UiContext, egui};
#[cfg(feature = "persistence")]
use bedtime_workbench::persistence::ReflectPersist;
use bedtime_workbench::{
    Action, ActionAppExt, ActionHandler, ActionInvocation, ActionResult, DocumentReference,
    EditOptions, Format, FormatId, FormatsAppExt, Handler, InArea, OpenOptions, Panel,
    PanelInstance, PanelPresentation, Placement, ResetZoom, Target, ViewersAppExt,
    WorkbenchWorldExt, ZoomIn, ZoomOut, viewer_document,
};
use bevy::prelude::*;

pub mod canvas;
pub mod edit;
pub mod export;
pub mod model;
#[cfg(test)]
mod tests;

pub use canvas::Camera;
pub use edit::{Restyle, Style, Tool};
pub use model::{
    Change, Color, Dash, Diagram, Element, ElementId, Head, Point, Rect, Shape, Side,
};

pub struct DiagramPlugin;

/// Diagrams: `.diagram` files. They're text (RON), so the editor opens them too.
pub const DIAGRAM: FormatId = FormatId("diagram");
/// What a diagram file's name ends with.
pub const EXTENSION: &str = "diagram";

impl Plugin for DiagramPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<DiagramPanel>()
            .register_format(
                Format::new(DIAGRAM)
                    .extensions([EXTENSION])
                    .conforms_to([bedtime_workbench::TEXT]),
            )
            .register_viewer::<Diagram, DiagramPanel>("Diagram")
            .formats([DIAGRAM]);
        app.add_action_handler::<NewDiagram>(Handler::new(new_diagram));
        app.register_action::<ExportSvg>()
            .register_action::<ExportPng>()
            .register_action::<CopyPng>()
            .init_non_send::<Saving>()
            .add_systems(Update, poll_saving);
    }
}

/// Save the focused diagram (its selection, if there is one) as an SVG,
/// where the system's picker says.
pub struct ExportSvg;
impl Action for ExportSvg {
    const KEY: &'static str = "diagram.export_svg";
    const LABEL: &'static str = "Export Diagram as SVG…";
}

/// Save the focused diagram (or its selection) as a PNG: at least twice its
/// size, and 2048 pixels on its longer side.
pub struct ExportPng;
impl Action for ExportPng {
    const KEY: &'static str = "diagram.export_png";
    const LABEL: &'static str = "Export Diagram as PNG…";
}

/// Put the focused diagram (or its selection) on the clipboard, as an image.
pub struct CopyPng;
impl Action for CopyPng {
    const KEY: &'static str = "diagram.copy_png";
    const LABEL: &'static str = "Copy Diagram as Image";
}

/// A picture asked for: made by the next draw, which has the theme.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Picture {
    Svg,
    Png,
    Clipboard,
}

/// How many times its size on the canvas a PNG is, at least (the renderer
/// makes a small picture larger: see `rendered`).
pub const PNG_SCALE: f32 = 2.0;

/// A diagram's elements (or just `only`, if any) as an SVG in the theme's
/// colors; `None` if there's nothing to draw.
pub fn svg(
    diagram: &Diagram,
    only: &[ElementId],
    theme: &bedtime_ui::theme::Theme,
    scale: f32,
) -> Option<String> {
    let rgb = |color: egui::Color32| [color.r(), color.g(), color.b()];
    let inks = export::Inks {
        background: rgb(bedtime_ui::theme::color(theme.palette.panel_background)),
        named: Color::ALL.map(|named| rgb(canvas::resolve(theme, named))),
    };
    let elements: Vec<&Element> = diagram
        .elements()
        .iter()
        .filter(|element| only.is_empty() || only.contains(&element.id))
        .collect();
    export::svg(&elements, &inks, scale)
}

/// An SVG rendered by `bedtime_ui::image`, which draws it at least 2048
/// pixels on its longer side: its size in pixels, and its pixels (RGBA).
pub fn rendered(svg: &str) -> Result<([u32; 2], Vec<u8>), String> {
    let decoded = bedtime_ui::image::decode(svg.as_bytes())?;
    let frame = decoded.frames.first().ok_or("Nothing was drawn")?;
    Ok((frame.size, frame.rgba.to_vec()))
}

/// An SVG rendered and written as a PNG file's bytes.
pub fn png(svg: &str) -> Result<Vec<u8>, String> {
    let ([width, height], rgba) = rendered(svg)?;
    let pixels = image::RgbaImage::from_raw(width, height, rgba).ok_or("Nothing was drawn")?;
    let mut bytes = std::io::Cursor::new(Vec::new());
    pixels
        .write_to(&mut bytes, image::ImageFormat::Png)
        .map_err(|error| error.to_string())?;
    Ok(bytes.into_inner())
}

/// The save picker, while it's open, and the file's bytes it's for. AppKit
/// owns the dialog, so its future is polled on the main thread.
#[derive(Default)]
struct Saving(Option<(Picking, Vec<u8>)>);

type Picking = std::pin::Pin<Box<dyn std::future::Future<Output = Option<rfd::FileHandle>>>>;

fn poll_saving(world: &mut World) {
    let mut saving = world.non_send_mut::<Saving>();
    let Some((picking, _)) = saving.0.as_mut() else {
        return;
    };
    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
    let std::task::Poll::Ready(chosen) = picking.as_mut().poll(&mut context) else {
        return;
    };
    let Some((_, bytes)) = saving.0.take() else {
        return;
    };
    if let Some(file) = chosen {
        let written = std::fs::write(file.path(), bytes);
        world.workbench_mut().notify_failure("Couldn't export", written);
    }
}

/// Make the picture asked of `panel`'s diagram, and save or copy it.
fn make_picture(
    world: &mut World,
    picture: Picture,
    diagram: &Diagram,
    only: &[ElementId],
    theme: &bedtime_ui::theme::Theme,
    title: &str,
) -> Result<(), String> {
    let scale = if picture == Picture::Svg { 1.0 } else { PNG_SCALE };
    let svg = svg(diagram, only, theme, scale).ok_or("There's nothing to export")?;
    if picture == Picture::Clipboard {
        let ([width, height], rgba) = rendered(&svg)?;
        return world
            .resource_mut::<bedtime_clipboard::Clipboard>()
            .write_image(&bedtime_clipboard::ClipboardImage {
                width: width as usize,
                height: height as usize,
                rgba,
            })
            .map_err(|error| error.to_string());
    }
    let (extension, kind, bytes) = match picture {
        Picture::Svg => ("svg", "SVG", svg.into_bytes()),
        _ => ("png", "PNG", png(&svg)?),
    };
    // Named after the diagram: "flow.diagram" as "flow.svg".
    let name = title.strip_suffix(&format!(".{EXTENSION}")).unwrap_or(title);
    let dialog = rfd::AsyncFileDialog::new()
        .set_title("Export")
        .add_filter(kind, &[extension])
        .set_file_name(format!("{name}.{extension}"));
    // One picker at a time: a new one replaces the last.
    world.non_send_mut::<Saving>().0 = Some((Box::pin(dialog.save_file()), bytes));
    Ok(())
}

/// A new untitled diagram, beside the focused document.
pub struct NewDiagram;
impl Action for NewDiagram {
    const KEY: &'static str = "diagram.new";
    const LABEL: &'static str = "New Diagram";
}

fn new_diagram(world: &mut World, invocation: ActionInvocation) -> ActionResult {
    let area = invocation
        .entity
        .filter(|&panel| world.get::<DocumentReference>(panel).is_some())
        .and_then(|panel| world.get::<InArea>(panel).map(|area| area.0));
    world
        .workbench_mut()
        .new_untitled::<Diagram>(OpenOptions {
            target: Target::New,
            place: area.map(Placement::In),
            ..default()
        })
        .map(|_| ())
}

#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
#[cfg_attr(feature = "persistence", reflect(Persist))]
#[require(
    PanelInstance = PanelInstance::of::<Self>(),
    // Zooming zooms the canvas, rather than the panel's zoom.
    ActionHandler<ZoomIn> = ActionHandler::new(Handler::new(|w, i| ask(w, i, ZoomAsk::In))),
    ActionHandler<ZoomOut> = ActionHandler::new(Handler::new(|w, i| ask(w, i, ZoomAsk::Out))),
    ActionHandler<ResetZoom> = ActionHandler::new(Handler::new(|w, i| ask(w, i, ZoomAsk::Fit))),
    ActionHandler<ExportSvg> = ActionHandler::new(Handler::new(|w, i| picture(w, i, Picture::Svg))),
    ActionHandler<ExportPng> = ActionHandler::new(Handler::new(|w, i| picture(w, i, Picture::Png))),
    ActionHandler<CopyPng> = ActionHandler::new(Handler::new(|w, i| picture(w, i, Picture::Clipboard))),
)]
pub struct DiagramPanel {
    /// The canvas point in the middle of the panel.
    pub center: Vec2,
    /// Screen points per canvas unit; 0 until the diagram is first drawn,
    /// which fits it to the panel.
    pub zoom: f32,
    /// The tool in hand, the selection and the drag under way.
    #[reflect(ignore)]
    pub canvas: canvas::State,
    /// A picture to make of it, when it's next drawn.
    #[reflect(ignore)]
    pub picture: Option<Picture>,
}

impl DiagramPanel {
    pub fn camera(&self) -> Camera {
        Camera {
            center: Point::new(self.center.x, self.center.y),
            zoom: self.zoom,
        }
    }

    fn set_camera(&mut self, camera: Camera) {
        self.center = Vec2::new(camera.center.x, camera.center.y);
        self.zoom = camera.zoom;
    }
}

fn picture(world: &mut World, invocation: ActionInvocation, picture: Picture) -> ActionResult {
    let mut diagram = invocation
        .entity
        .and_then(|panel| world.get_mut::<DiagramPanel>(panel))
        .ok_or("No diagram is focused")?;
    diagram.picture = Some(picture);
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum ZoomAsk {
    In,
    Out,
    Fit,
}

fn ask(world: &mut World, invocation: ActionInvocation, zoom: ZoomAsk) -> ActionResult {
    let mut diagram = invocation
        .entity
        .and_then(|panel| world.get_mut::<DiagramPanel>(panel))
        .ok_or("No diagram is focused")?;
    let mut camera = diagram.camera();
    match zoom {
        ZoomAsk::In => camera.zoom_in(),
        ZoomAsk::Out => camera.zoom_out(),
        // Fitted again by the next draw, which knows the panel's size.
        ZoomAsk::Fit => camera.zoom = 0.0,
    }
    diagram.set_camera(camera);
    Ok(())
}

impl Panel for DiagramPanel {
    const LABEL: &'static str = "Diagram";

    fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, panel: Entity) {
        let Some(document) = viewer_document::<Diagram>(ui, world, panel) else {
            return;
        };
        let focused = world
            .get::<PanelPresentation>(panel)
            .is_some_and(|presentation| presentation.focused);
        let Some(mut diagram) = world.get_mut::<DiagramPanel>(panel) else {
            return;
        };
        let mut camera = diagram.camera();
        let state = &mut diagram.bypass_change_detection().canvas;
        let edit = ui
            .push_id(panel, |ui| {
                canvas::show(ui, context, &document.model, &mut camera, state, focused)
            })
            .inner;
        // Only when it moved: the layout is saved when a panel changes.
        if camera != diagram.camera() {
            diagram.set_camera(camera);
        }
        let asked = diagram.bypass_change_detection().picture.take();
        let selection = diagram.canvas.selection.clone();
        if let Some(asked) = asked {
            let title = world
                .workbench()
                .inspect_document(document.handle.id())
                .map(|info| info.title)
                .unwrap_or_default();
            let made = make_picture(world, asked, &document.model, &selection, context.theme, &title);
            world.workbench_mut().notify_failure("Couldn't export", made);
        }
        if let Some(edit) = edit {
            let made = world.workbench_mut().edit_document_transaction(
                document.handle,
                document.revision,
                edit.changes,
                EditOptions::new(edit.label.clone()),
            );
            world.workbench_mut().notify_failure(&edit.label, made);
        }
    }
}
