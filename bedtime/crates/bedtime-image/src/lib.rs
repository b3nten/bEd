//! An image viewer for workbench apps. The image fills its panel: scroll or
//! pinch zooms around the pointer, dragging pans, a double click switches
//! between fitting it and actual size. Zoomed in, pixels stay sharp.
//! Animated GIF, APNG and WebP play. A floating bar has zoom, fitting and
//! play; the right-click menu has the rest (backgrounds, smoothing).
use bedtime_ui::{BuiltinIcon, Floating, UiContext, egui, theme};
#[cfg(feature = "persistence")]
use bedtime_workbench::persistence::ReflectPersist;
use bedtime_workbench::{
    DocumentHandle, Fit, Opener, OpenersAppExt, Panel, PanelInstance, PanelPresentation, Viewer,
    viewer_document,
};
use bevy::prelude::*;
use std::sync::Arc;

mod picture;
mod transform;
pub use picture::{
    Decoded, Frame, MAX_DECODED_BYTES, Picture, SUPPORTED_EXTENSIONS, decode, supported,
};
pub use transform::ImageTransform;

pub struct ImagePlugin;

impl Plugin for ImagePlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<ImagePanel>().register_opener(
            Opener::<Picture>::new::<ImagePanel>("Image").files(|path, _| {
                if supported(path) {
                    Fit::Specific
                } else {
                    Fit::No
                }
            }),
        );
    }
}

/// What's behind the image (and shows through its transparency).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Reflect)]
pub enum Backdrop {
    #[default]
    Checkerboard,
    Dark,
    Light,
    /// The panel's own background.
    Panel,
}

#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
#[cfg_attr(feature = "persistence", reflect(Persist))]
#[require(PanelInstance = PanelInstance::of::<Self>())]
pub struct ImagePanel {
    pub backdrop: Backdrop,
    /// Smooth pixels when zoomed in, rather than showing them sharp.
    pub smooth: bool,
    #[reflect(ignore)]
    pub transform: ImageTransform,
    #[reflect(ignore)]
    shown: Option<Shown>,
    #[reflect(ignore)]
    pub paused: bool,
}

/// The picture on the GPU, and where its animation is.
struct Shown {
    picture: Arc<Decoded>,
    textures: Vec<Option<egui::TextureHandle>>,
    sharp: bool,
    frame: usize,
    /// When the frame changes, in egui time.
    next: f64,
}

impl ImagePanel {
    /// The frame showing, of how many.
    pub fn frame(&self) -> Option<(usize, usize)> {
        let shown = self.shown.as_ref()?;
        Some((shown.frame, shown.picture.frames.len()))
    }
}

impl Viewer<Picture> for ImagePanel {
    fn new(_: DocumentHandle<Picture>) -> Self {
        Self::default()
    }
    fn apply(_: &mut World, _: Entity, _: &()) {}
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
            draw(ui, context, panel, &mut image, picture, focused);
        }
    }
}

/// Zoom steps, as points per image pixel.
const STEPS: &[f32] = &[
    0.05, 0.1, 0.125, 0.25, 0.33, 0.5, 0.67, 1.0, 1.5, 2.0, 3.0, 4.0, 6.0, 8.0, 12.0, 16.0, 24.0,
    32.0, 48.0, 64.0,
];

fn step(scale: f32, up: bool) -> f32 {
    if up {
        STEPS
            .iter()
            .copied()
            .find(|&s| s > scale * 1.01)
            .unwrap_or(scale)
    } else {
        STEPS
            .iter()
            .rev()
            .copied()
            .find(|&s| s < scale * 0.99)
            .unwrap_or(scale)
    }
}

#[derive(Default)]
struct Asked {
    fit: bool,
    actual: bool,
    zoom: Option<bool>,
    toggle_play: bool,
}

fn draw(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    panel: Entity,
    image: &mut ImagePanel,
    picture: Arc<Decoded>,
    focused: bool,
) {
    let id = egui::Id::new(("bedtime.image", panel));
    let (rect, response) = ui.allocate_exact_size(
        ui.available_size().max(egui::Vec2::splat(1.0)),
        egui::Sense::click_and_drag(),
    );
    if response.clicked() || response.secondary_clicked() {
        response.request_focus();
    }
    // A new picture (or the file changed): new textures, from the first frame.
    if image
        .shown
        .as_ref()
        .is_none_or(|shown| !Arc::ptr_eq(&shown.picture, &picture))
    {
        image.shown = Some(Shown {
            textures: vec![None; picture.frames.len()],
            picture: picture.clone(),
            sharp: false,
            frame: 0,
            next: ui.input(|i| i.time),
        });
    }
    let size = egui::vec2(picture.size[0] as f32, picture.size[1] as f32);
    let animated = picture.frames.len() > 1;
    let mut asked = Asked::default();

    // Keys, while focused.
    if focused && !ui.ctx().egui_wants_keyboard_input() && !egui::Popup::is_any_open(ui.ctx()) {
        ui.input_mut(|i| {
            asked.zoom = if i.consume_key(egui::Modifiers::COMMAND, egui::Key::Plus)
                || i.consume_key(egui::Modifiers::COMMAND, egui::Key::Equals)
            {
                Some(true)
            } else if i.consume_key(egui::Modifiers::COMMAND, egui::Key::Minus) {
                Some(false)
            } else {
                None
            };
            asked.fit = i.consume_key(egui::Modifiers::COMMAND, egui::Key::Num0);
            asked.actual = i.consume_key(egui::Modifiers::COMMAND, egui::Key::Num1);
            asked.toggle_play = animated && i.consume_key(egui::Modifiers::NONE, egui::Key::Space);
        });
    }

    // The pointer: scroll and pinch zoom around it, dragging pans.
    let pointer = ui.input(|i| i.pointer.hover_pos());
    if response.hovered()
        && let Some(pointer) = pointer
    {
        let (scroll, pinch) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
        let factor = (scroll * 0.002).exp() * pinch;
        if factor != 1.0 {
            image.transform.zoom_at(rect, size, pointer, factor);
            ui.input_mut(|i| i.smooth_scroll_delta = egui::Vec2::ZERO);
        }
    }
    if response.dragged_by(egui::PointerButton::Primary) {
        image.transform.pan(rect, size, response.drag_delta());
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    } else if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
    }
    if response.double_clicked() {
        let fitted = (image.transform.zoom() - 1.0).abs() < 0.001;
        if fitted {
            let at = pointer.unwrap_or(rect.center());
            image.transform.zoom_to(rect, size, at, 1.0);
        } else {
            image.transform.reset();
        }
    }

    // The menu, and the floating bar.
    let mut frame = context.frame(ui);
    response.context_menu(|ui| menu(ui, image, animated, &mut asked));
    let scale = image.transform.scale(rect.size(), size);
    Floating::new(id.with("bar"), rect, egui::Align2::CENTER_BOTTOM).show(&mut frame, |f| {
        if animated {
            let (icon, tip) = if image.paused {
                (BuiltinIcon::Play, "Play (Space)")
            } else {
                (BuiltinIcon::Pause, "Pause (Space)")
            };
            if f.floating_button(icon, tip).clicked() {
                asked.toggle_play = true;
            }
        }
        if f.floating_button(BuiltinIcon::Minus, "Zoom Out").clicked() {
            asked.zoom = Some(false);
        }
        let label = f.floating_label(format!("{:.0}%", scale * 100.0));
        if label.interact(egui::Sense::click()).clicked() {
            asked.actual = true;
        }
        if f.floating_button(BuiltinIcon::Plus, "Zoom In").clicked() {
            asked.zoom = Some(true);
        }
        if f.floating_button(BuiltinIcon::Fit, "Fit (Double-Click)")
            .clicked()
        {
            asked.fit = true;
        }
    });
    let mut info = format!(
        "{} × {} · {}",
        picture.size[0], picture.size[1], picture.format
    );
    if animated {
        info = format!("{info} · {} frames", picture.frames.len());
    }
    Floating::new(id.with("info"), rect, egui::Align2::LEFT_BOTTOM).show(&mut frame, |f| {
        f.floating_label(info);
    });

    // What was asked for.
    if asked.fit {
        image.transform.reset();
    }
    if asked.actual {
        image.transform.zoom_to(rect, size, rect.center(), 1.0);
    }
    if let Some(up) = asked.zoom {
        image
            .transform
            .zoom_to(rect, size, rect.center(), step(scale, up));
    }
    if asked.toggle_play {
        image.paused = !image.paused;
    }

    // The backdrop and the image.
    let target = image.transform.image_rect(rect, size);
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    let p = &context.theme.palette;
    match image.backdrop {
        Backdrop::Checkerboard => checkerboard(&painter, target.intersect(rect), p),
        Backdrop::Dark => {
            painter.rect_filled(rect, 0.0, egui::Color32::from_gray(24));
        }
        Backdrop::Light => {
            painter.rect_filled(rect, 0.0, egui::Color32::from_gray(236));
        }
        Backdrop::Panel => {}
    }
    let now = ui.input(|i| i.time);
    let pixels_per_point = ui.ctx().pixels_per_point();
    let shown = image.shown.as_mut().unwrap();
    if animated && !image.paused {
        // Catch up on frames that should have shown, without spinning.
        let mut guard = 0;
        while now >= shown.next && guard < picture.frames.len() {
            shown.frame = (shown.frame + 1) % picture.frames.len();
            shown.next += picture.frames[shown.frame].delay.as_secs_f64();
            guard += 1;
        }
        if now >= shown.next {
            shown.next = now + picture.frames[shown.frame].delay.as_secs_f64();
        }
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_secs_f64(
                (shown.next - now).max(0.0),
            ));
    } else {
        shown.next = now;
    }
    // Sharp pixels once each covers a few screen pixels.
    let frame_pixels = &picture.frames[shown.frame];
    let shown_scale = target.width() / frame_pixels.size[0] as f32 * pixels_per_point;
    let sharp = !image.smooth && shown_scale >= 3.0;
    if sharp != shown.sharp {
        shown.sharp = sharp;
        shown
            .textures
            .iter_mut()
            .for_each(|texture| *texture = None);
    }
    let options = if sharp {
        egui::TextureOptions {
            magnification: egui::TextureFilter::Nearest,
            minification: egui::TextureFilter::Linear,
            ..egui::TextureOptions::LINEAR
        }
    } else {
        egui::TextureOptions::LINEAR
    };
    let texture = shown.textures[shown.frame].get_or_insert_with(|| {
        let pixels = egui::ColorImage::from_rgba_unmultiplied(
            [frame_pixels.size[0] as usize, frame_pixels.size[1] as usize],
            &frame_pixels.rgba,
        );
        ui.ctx()
            .load_texture(format!("{id:?}-{}", shown.frame), pixels, options)
    });
    painter.image(
        texture.id(),
        target,
        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
        egui::Color32::WHITE,
    );
}

fn checkerboard(painter: &egui::Painter, rect: egui::Rect, p: &theme::Palette) {
    if !rect.is_positive() {
        return;
    }
    let light = theme::color(theme::mix(p.panel_background, p.text, 0.10));
    let dark = theme::color(theme::mix(p.panel_background, p.text, 0.04));
    painter.rect_filled(rect, 0.0, dark);
    let side = 10.0;
    let mut mesh = egui::Mesh::default();
    let columns = (rect.width() / side).ceil() as i32;
    let rows = (rect.height() / side).ceil() as i32;
    // Bounded: a huge zoom only draws what's on screen.
    if (columns as i64) * (rows as i64) > 200_000 {
        return;
    }
    for row in 0..rows {
        for column in 0..columns {
            if (row + column) % 2 == 0 {
                continue;
            }
            let min = rect.min + egui::vec2(column as f32 * side, row as f32 * side);
            let square = egui::Rect::from_min_size(min, egui::Vec2::splat(side)).intersect(rect);
            mesh.add_colored_rect(square, light);
        }
    }
    painter.add(mesh);
}

fn menu(ui: &mut egui::Ui, image: &mut ImagePanel, animated: bool, asked: &mut Asked) {
    if ui.button("Fit").clicked() {
        asked.fit = true;
    }
    if ui.button("Actual Size").clicked() {
        asked.actual = true;
    }
    if ui.button("Zoom In").clicked() {
        asked.zoom = Some(true);
    }
    if ui.button("Zoom Out").clicked() {
        asked.zoom = Some(false);
    }
    if animated {
        ui.separator();
        let label = if image.paused { "Play" } else { "Pause" };
        if ui.button(label).clicked() {
            asked.toggle_play = true;
        }
    }
    ui.separator();
    ui.menu_button("Background", |ui| {
        for (backdrop, label) in [
            (Backdrop::Checkerboard, "Checkerboard"),
            (Backdrop::Dark, "Dark"),
            (Backdrop::Light, "Light"),
            (Backdrop::Panel, "None"),
        ] {
            if ui.radio(image.backdrop == backdrop, label).clicked() {
                image.backdrop = backdrop;
            }
        }
    });
    let mut smooth = image.smooth;
    if ui.checkbox(&mut smooth, "Smooth When Zoomed In").clicked() {
        image.smooth = smooth;
    }
}
