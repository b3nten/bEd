//! Blender-style chrome is a projection of ECS areas. Pointer gestures and
//! interpolation are local to this drawing system; only completed edits commit.
use super::{Layouts, LayoutsRef, LayoutsWorldExt};
use super::{
    geometry, *, {GeometrySnapshot, plan_tab_geometry},
};
use crate::{
    UiContext, UiDraw, UiFrame,
    egui::{self, CursorIcon, Key, PointerButton},
};
use bevy::prelude::*;
use std::{collections::HashMap, sync::Arc};

const PANEL_BORDER_WIDTH: f32 = 1.0;

fn allows_panel_edit(settings: LayoutSurface, edit: &LayoutEdit) -> bool {
    match edit {
        LayoutEdit::FocusArea(_) | LayoutEdit::FocusPanel(_) | LayoutEdit::KeepPanel(_) => true,
        LayoutEdit::ClosePanels(_) | LayoutEdit::CloseArea(_) => settings.panel_closure,
        LayoutEdit::DropPanel { .. } => settings.panel_creation && settings.layout_editing,
        _ => settings.layout_editing,
    }
}

/// Who renders and handles input in the window's titlebar. Native geometry is
/// still reserved in either mode, so the workbench stays below native chrome.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub enum TitlebarOwnership {
    /// The surface draws the titlebar band, its `Titlebar` content, and moves the
    /// window when the band is dragged.
    #[default]
    Surface,
    /// Leave titlebar drawing, native appearance and gestures to the application.
    /// The Titlebar component is suspended until ownership returns to the surface.
    Application,
}

/// The workbench shell required by LayoutSurface on a window. This entity
/// lives in the application world and remains stable across layout switches.
#[derive(Component, Clone)]
#[require(SurfacePresentation)]
pub struct SurfaceShell {
    pub titlebar_ownership: TitlebarOwnership,
    /// Hide the tiled content (for a scene underneath), keeping the chrome.
    pub panels_visible: bool,
    /// Outer shell bounds in egui points. Defaults to the parent's available UI.
    pub bounds: Option<Rect>,
    pub minimum_area_size: Vec2,
}

/// Observed shell allocations, in egui points. The host alone writes these.
#[derive(Component, Default, Clone, Copy, Debug)]
pub struct SurfacePresentation {
    pub bounds: Rect,
    pub titlebar: Rect,
    /// Usable titlebar content, after excluding native controls and edge padding.
    pub titlebar_content: Rect,
    /// Where the panels were drawn: the body, unless the app's body put them
    /// elsewhere.
    pub panels: Rect,
}

/// Optional titlebar content on the same window as LayoutSurface.
/// The renderer receives that window as UiContext::view and UiContext::surface.
#[derive(Component, Clone, Copy)]
#[require(LayoutSurface)]
pub struct Titlebar {
    /// Requested content height in egui points. The host adds four points above
    /// the content; native chrome may require a taller band. The header fallback
    /// uses the same spacing.
    pub height: f32,
    pub draw: UiDraw,
}

/// The layout UI's state between frames: gestures, animations.
#[derive(Resource, Default)]
pub struct LayoutsUi {
    frontend: Frontend,
    pub(crate) drawing: bool,
}

pub(crate) fn layouts_ui_mut(world: &mut World) -> Mut<'_, LayoutsUi> {
    world.resource_mut()
}

impl LayoutsRef {
    /// Whether the layout UI is being drawn right now (panels' drawing code
    /// runs inside it, so structural changes then are refused).
    pub fn drawing(&self) -> bool {
        self.world
            .get_resource::<LayoutsUi>()
            .is_some_and(|ui| ui.drawing)
    }

    pub fn has_focus(&self, panel: Entity) -> bool {
        self.world
            .get::<PanelPresentation>(panel)
            .is_some_and(|p| p.visible && p.focused)
    }
}

impl Layouts {
    /// Apply a user-originated edit with the surface's configured permissions.
    /// An edit that closes panels goes through the close checks and hooks.
    /// Direct application operations deliberately bypass this user policy.
    pub fn user_edit(&mut self, layout: Entity, edit: &LayoutEdit) -> Result<(), String> {
        let settings = self
            .world
            .query::<&LayoutSurface>()
            .iter(&self.world)
            .next()
            .copied()
            .unwrap_or_default();
        if !allows_panel_edit(settings, edit) {
            return Err("User panel editing is disabled by the layout surface".into());
        }
        if matches!(
            edit,
            LayoutEdit::ClosePanels(_)
                | LayoutEdit::CloseArea(_)
                | LayoutEdit::JoinAreas { .. }
                | LayoutEdit::DockArea { .. }
        ) {
            let panels = self.close_targets(layout, edit)?;
            super::check_panel_close(&self.world, panels.iter().copied())?;
            let hooks = self.world.resource::<super::CloseHooks>().hooks.clone();
            for hook in hooks {
                if hook(&mut self.world, layout, edit, &panels)? {
                    return Ok(());
                }
            }
        }
        self.apply_edit(layout, edit)
    }
}

pub(crate) fn reset(world: &mut World) {
    for mut panel in world.query::<&mut PanelPresentation>().iter_mut(world) {
        *panel = default();
    }
    for mut area in world.query::<&mut AreaPresentation>().iter_mut(world) {
        area.visible = false;
    }
    for mut viewport in world.query::<&mut crate::UiViewport>().iter_mut(world) {
        viewport.visible = false;
        viewport.hovered = false;
    }
}

pub(crate) fn draw_active(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>) {
    if world.get::<LayoutSurface>(context.surface).is_none() {
        return;
    }
    reset(world);
    let context = context.for_view(context.surface);
    let frame = context.frame(ui);
    let descriptor = world.get::<SurfaceShell>(frame.view).unwrap().clone();
    let host_entity = frame.view;
    #[cfg(feature = "native")]
    let (native_height, controls) = world
        .get::<WindowTitlebarGeometry>(frame.surface)
        .map_or((0.0, None), |geometry| (geometry.height, geometry.controls));
    #[cfg(not(feature = "native"))]
    let (native_height, controls) = (0.0, None);
    #[cfg(feature = "native")]
    {
        let background = WindowBackground::new(frame.theme.palette.background);
        if world.get::<WindowBackground>(frame.surface) != Some(&background) {
            world.entity_mut(frame.surface).insert(background);
        }
    }
    let outer = descriptor
        .bounds
        .map(er)
        .unwrap_or_else(|| frame.ui.available_rect_before_wrap())
        .intersect(frame.ui.clip_rect());
    if !outer.is_positive() {
        *world.get_mut::<SurfacePresentation>(host_entity).unwrap() = default();
        return;
    }
    let owns_titlebar = descriptor.titlebar_ownership == TitlebarOwnership::Surface;
    let background = {
        let titlebar = world
            .get::<Titlebar>(frame.surface)
            .copied()
            .filter(|_| owns_titlebar);
        let (band, content) = titlebar_rects(
            outer,
            native_height,
            controls,
            titlebar.map_or(0.0, |titlebar| titlebar.height),
            frame.ui.ctx().zoom_factor(),
        );
        let mut root = frame.ui.new_child(
            egui::UiBuilder::new()
                .id_salt("workbench-world")
                .max_rect(outer),
        );
        frame.ui.advance_cursor_after_rect(outer);
        root.set_clip_rect(outer);
        let background = owns_titlebar.then(|| {
            root.painter().rect_filled(
                band,
                0.0,
                crate::theme::color(frame.theme.palette.background),
            );
            // Register the background first: child widgets own presses that begin on
            // them, including drag sources, even after the pointer leaves their bounds.
            root.interact(
                band,
                root.id().with("titlebar-drag"),
                // Claim only clicks. A drag sense would also claim the pending drag
                // behind a click-only child button, before egui decides click vs drag.
                egui::Sense::click(),
            )
        });
        if let Some(titlebar) = titlebar {
            if content.is_positive() {
                let mut ui = root.new_child(
                    egui::UiBuilder::new()
                        .id_salt(("titlebar-content", frame.surface))
                        .max_rect(content)
                        .layout(egui::Layout::left_to_right(egui::Align::Center)),
                );
                ui.set_clip_rect(content);
                frame
                    .for_view(frame.surface, &mut ui)
                    .clip_rounded(content, 0, |frame| {
                        (titlebar.draw)(frame.ui, world, &context);
                    });
            }
        }
        if world.get::<LayoutSurface>(frame.surface).is_none() {
            return;
        }
        let body = egui::Rect::from_min_max(egui::pos2(outer.left(), band.bottom()), outer.max);
        let mut ui = root.new_child(egui::UiBuilder::new().id_salt("session").max_rect(body));
        let body_view = context.for_view(host_entity);
        match world.get::<super::SurfaceBody>(frame.surface).copied() {
            Some(super::SurfaceBody(draw)) => draw(&mut ui, world, &body_view),
            None => show(&mut ui, world, &body_view),
        }
        let mut presentation = *world.get::<SurfacePresentation>(host_entity).unwrap();
        if world.get::<LayoutSurface>(frame.surface).is_none() {
            return;
        }
        presentation.bounds = observed(outer);
        presentation.titlebar = observed(band);
        presentation.titlebar_content = observed(content);
        *world.get_mut::<SurfacePresentation>(host_entity).unwrap() = presentation;
        background
    };
    if let Some(background) = background {
        if background.double_clicked() {
            #[cfg(all(feature = "native", target_os = "macos"))]
            if let Some(handle) = world.get::<bevy::window::RawHandleWrapper>(frame.surface) {
                crate::layouts::chrome::titlebar_double_click(handle);
            }
        } else if background.is_pointer_button_down_on()
            && frame
                .ui
                .input(|i| i.pointer.button_pressed(PointerButton::Primary))
            && let Some(mut window) = world.get_mut::<bevy::window::Window>(frame.surface)
            && window.mode == bevy::window::WindowMode::Windowed
        {
            window.start_drag_move();
            #[cfg(all(feature = "native", target_os = "macos"))]
            world
                .entity_mut(frame.surface)
                .insert(crate::layouts::chrome::WindowDrag);
        }
    }
}

/// Areas, their rectangles and tabs: what a gesture in progress depends on.
fn layout_topology(world: &World, layout: Entity) -> Vec<(Entity, AreaRect, Vec<Entity>)> {
    world
        .get::<LayoutAreas>(layout)
        .map_or(&[][..], |areas| areas.entities())
        .iter()
        .filter_map(|&area| {
            let state = world.get::<Area>(area)?;
            Some((
                area,
                state.rect,
                world.layouts().panels_in_area(area).to_vec(),
            ))
        })
        .collect()
}

fn observed(rect: egui::Rect) -> Rect {
    Rect::from_corners(
        Vec2::new(rect.min.x, rect.min.y),
        Vec2::new(rect.max.x, rect.max.y),
    )
}

/// Native coordinates are window-relative. Intersect before reserving space so
/// an embedded workbench below the titlebar never acquires a phantom native gap.
fn titlebar_rects(
    outer: egui::Rect,
    native_height: f32,
    controls: Option<Rect>,
    requested: f32,
    zoom: f32,
) -> (egui::Rect, egui::Rect) {
    let top_padding = if requested > 0.0 { 4.0 } else { 0.0 };
    let height = (requested + top_padding)
        .max((native_height / zoom - outer.top()).max(0.0))
        .min(outer.height());
    let band = egui::Rect::from_min_size(outer.min, egui::vec2(outer.width(), height));
    let padding = 6.0_f32.min(band.width() * 0.5);
    let mut content = band.shrink2(egui::vec2(padding, 0.0));
    content.min.y += top_padding.min(height);
    if let Some(controls) = controls {
        let controls = egui::Rect::from_min_max(
            egui::pos2(controls.min.x / zoom, controls.min.y / zoom),
            egui::pos2(controls.max.x / zoom, controls.max.y / zoom),
        );
        if controls.intersects(band) {
            let control_padding = 24.0;
            if controls.center().x <= band.center().x {
                content.min.x =
                    (controls.right() + control_padding).clamp(content.left(), content.right());
            } else {
                content.max.x =
                    (controls.left() - control_padding).clamp(content.left(), content.right());
            }
        }
    }
    (band, content)
}

#[derive(Default)]
struct Frontend {
    /// The layout and topology last drawn; a change cancels any gesture.
    layout: Option<Entity>,
    topology: Vec<(Entity, AreaRect, Vec<Entity>)>,
    bounds: Rect,
    gesture: Option<Gesture>,
    area_bounds: HashMap<Entity, Rect>,
    content_fades: HashMap<Entity, ContentFade>,
    tab_bounds: HashMap<Entity, Rect>,
    tab_bars: HashMap<Entity, Rect>,
    tab_scroll: HashMap<Entity, TabScroll>,
    preview: Option<Rect>,
    suppress_context: bool,
    payload_cancelled: bool,
}

#[derive(Default)]
struct TabScroll {
    offset: f32,
    selected: Option<Entity>,
}

struct ContentFade {
    area: Entity,
    placement: geometry::Rect,
    started_at: f64,
}

impl ContentFade {
    fn opacity(
        &mut self,
        area: Entity,
        placement: geometry::Rect,
        now: f64,
        motion: crate::MotionPolicy,
    ) -> f32 {
        if self.area != area || self.placement != placement {
            self.area = area;
            self.placement = placement;
            self.started_at = now;
        }
        if !motion.enabled {
            self.started_at = now - f64::from(motion.seconds);
            return 1.0;
        }
        let progress = ((now - self.started_at) / f64::from(motion.seconds)).clamp(0.0, 1.0);
        (1.0 - (1.0 - progress).powi(3)) as f32
    }
}

enum Gesture {
    Corner {
        area: Entity,
        start: Vec2,
        axis: Option<SplitAxis>,
    },
    Tab {
        panel: Entity,
        start: Vec2,
        dragging: bool,
    },
    Border {
        axis: SplitAxis,
        at: i32,
        span: [i32; 2],
        pointer_offset: i32,
    },
}

#[derive(Clone)]
struct Group {
    entity: Entity,
    rect: geometry::Rect,
    tabs: Vec<Tab>,
    selected: Option<Entity>,
}

#[derive(Clone)]
struct Tab {
    entity: Entity,
    title: String,
    dirty: bool,
    style: PanelTabStyle,
    icon: Option<crate::IconSource>,
}

impl Tab {
    fn from_world(world: &World, entity: Entity) -> Self {
        let panel = world.get::<PanelInstance>(entity).expect("Owned tab");
        Self {
            entity,
            title: panel.title.clone(),
            dirty: world
                .get_resource::<super::PanelModified>()
                .is_some_and(|modified| (modified.0)(world, entity)),
            style: world
                .get::<PanelTabStyle>(entity)
                .copied()
                .unwrap_or(if panel.preview {
                    PanelTabStyle {
                        italic: true,
                        text_opacity: 0.9,
                    }
                } else {
                    PanelTabStyle::default()
                }),
            icon: world.get::<crate::IconSource>(entity).cloned(),
        }
    }

    fn layout_label(&self, painter: &egui::Painter, font: egui::FontId) -> Arc<egui::Galley> {
        painter.layout_job(egui::text::LayoutJob::single_section(
            self.title.clone(),
            egui::TextFormat {
                font_id: font,
                italics: self.style.italic,
                // Resolve the animated theme color when painting, without
                // reshaping or changing the metrics used to measure this tab.
                color: egui::Color32::PLACEHOLDER,
                ..default()
            },
        ))
    }

    fn label_color(&self, themed: egui::Color32) -> egui::Color32 {
        let opacity = if self.style.text_opacity.is_finite() {
            self.style.text_opacity.clamp(0.0, 1.0)
        } else {
            1.0
        };
        themed.gamma_multiply(opacity)
    }
}

fn screen(r: geometry::Rect, bounds: Rect) -> Rect {
    let point = |p: [i32; 2]| {
        bounds.min + Vec2::new(p[0] as f32, p[1] as f32) * bounds.size() / LAYOUT_EXTENT as f32
    };
    Rect::from_corners(point(r.min), point(r.max))
}

fn logical(p: Vec2, bounds: Rect) -> [i32; 2] {
    let p = (p - bounds.min) * LAYOUT_EXTENT as f32 / bounds.size();
    [p.x.round() as i32, p.y.round() as i32]
}

fn surface(r: Rect, model: geometry::Rect, panel_gap: f32) -> Rect {
    let lo = Vec2::new(
        if model.min[0] == 0 {
            panel_gap
        } else {
            panel_gap * 0.5
        },
        if model.min[1] == 0 {
            panel_gap
        } else {
            panel_gap * 0.5
        },
    );
    let hi = Vec2::new(
        if model.max[0] == LAYOUT_EXTENT {
            panel_gap
        } else {
            panel_gap * 0.5
        },
        if model.max[1] == LAYOUT_EXTENT {
            panel_gap
        } else {
            panel_gap * 0.5
        },
    );
    // Large gaps can consume a small area; collapse it instead of inverting it.
    let max = (r.max - hi).max(r.min);
    Rect::from_corners((r.min + lo).min(max), max)
}

fn corner_hit_rect(surface: Rect, high: [bool; 2], size: f32) -> Rect {
    let size = size.min(surface.width() * 0.5).min(surface.height() * 0.5);
    let anchor = Vec2::new(
        if high[0] {
            surface.max.x
        } else {
            surface.min.x
        },
        if high[1] {
            surface.max.y
        } else {
            surface.min.y
        },
    );
    let inward = Vec2::new(
        if high[0] { -size } else { size },
        if high[1] { -size } else { size },
    );
    Rect::from_corners(anchor, anchor + inward)
}

fn axis(index: usize) -> SplitAxis {
    if index == 0 {
        SplitAxis::Horizontal
    } else {
        SplitAxis::Vertical
    }
}

fn blend_rect(from: Rect, to: Rect, blend: f32) -> Rect {
    let settle = |p: Vec2, t: Vec2| {
        let v = p.lerp(t, blend);
        Vec2::new(
            if (v.x - t.x).abs() < 0.25 { t.x } else { v.x },
            if (v.y - t.y).abs() < 0.25 { t.y } else { v.y },
        )
    };
    Rect::from_corners(settle(from.min, to.min), settle(from.max, to.max))
}

fn expand_rect(from: Rect, to: Rect, blend: f32) -> Rect {
    if from.size() == to.size() {
        return to;
    }
    // Give up reassigned space immediately. Only edges growing into available
    // space interpolate, including when a split also expands the other axis.
    let retained = Rect::from_corners(
        from.min.clamp(to.min, to.max),
        from.max.clamp(to.min, to.max),
    );
    blend_rect(retained, to, blend)
}

fn er(r: Rect) -> egui::Rect {
    egui::Rect::from_min_max(egui::pos2(r.min.x, r.min.y), egui::pos2(r.max.x, r.max.y))
}

/// `label` names the target for accessibility (and scripts), if it has one.
fn hit(ui: &egui::Ui, name: &str, rect: Rect, label: Option<&str>) -> (bool, bool) {
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return (false, false);
    }
    let response = ui.interact(er(rect), ui.id().with(name), egui::Sense::click_and_drag());
    if let Some(label) = label {
        let enabled = ui.is_enabled();
        response
            .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
    }
    (
        response.hovered(),
        response.is_pointer_button_down_on()
            && ui.input(|i| i.pointer.button_pressed(PointerButton::Primary)),
    )
}

#[derive(Clone)]
pub struct PanelFactoryPayload(
    pub PanelFactory,
    Option<std::sync::Arc<dyn std::any::Any + Send + Sync>>,
);

impl PanelFactoryPayload {
    pub fn new(factory: PanelFactory) -> Self {
        Self(factory, None)
    }

    /// Preserve a widget's own drop semantics while also offering area creation.
    /// Content targets run first and can consume this entire payload.
    pub fn with_content<T: Send + Sync + 'static>(factory: PanelFactory, content: T) -> Self {
        Self(factory, Some(std::sync::Arc::new(content)))
    }

    pub fn content<T: Send + Sync + 'static>(&self) -> Option<std::sync::Arc<T>> {
        self.1.as_ref()?.clone().downcast().ok()
    }

    /// Reserve this pass for a content target's feedback and release handling.
    pub fn reserve_for_content(ctx: &egui::Context) {
        let pass = ctx.cumulative_pass_nr();
        ctx.data_mut(|data| data.insert_temp(egui::Id::new("workbench-content-drop-pass"), pass));
    }

    fn content_reserved(ctx: &egui::Context) -> bool {
        ctx.data(|data| data.get_temp::<u64>(egui::Id::new("workbench-content-drop-pass")))
            == Some(ctx.cumulative_pass_nr())
    }
}

/// Draw the active layout in the space `ui` has left: areas, tab bars, panels,
/// and the gestures that rearrange them. A `SurfaceBody` calls this where it
/// wants the panels.
pub fn show(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>) {
    let Some(mut shell) = world.get::<SurfaceShell>(context.surface).cloned() else {
        return;
    };
    shell.bounds = None;
    let focused = world
        .get::<bevy::window::Window>(context.surface)
        .is_none_or(|w| w.focused);
    let context = context.for_view(context.surface);
    world.resource_mut::<LayoutsUi>().drawing = true;
    let mut state = std::mem::take(&mut layouts_ui_mut(world).frontend);
    draw(context.frame(ui), world, &mut state, &shell, focused);
    layouts_ui_mut(world).frontend = state;
    layouts_ui_mut(world).drawing = false;
}

fn draw(
    frame: UiFrame,
    world: &mut World,
    state: &mut Frontend,
    host: &SurfaceShell,
    window_focused: bool,
) {
    let (panels_visible, minimum_area_size) = (host.panels_visible, host.minimum_area_size);
    let mut settings = *world.get::<LayoutSurface>(frame.surface).unwrap();
    let outer = host
        .bounds
        .map(er)
        .unwrap_or_else(|| frame.ui.available_rect_before_wrap())
        .intersect(frame.ui.clip_rect());
    if !outer.is_positive() {
        *world.get_mut::<SurfacePresentation>(frame.view).unwrap() = default();
        return;
    }
    let mut root = frame.ui.new_child(
        egui::UiBuilder::new()
            .id_salt("workbench-shell")
            .max_rect(outer)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    frame.ui.advance_cursor_after_rect(outer);
    root.set_clip_rect(outer);
    let background = crate::theme::color(frame.theme.palette.background);
    if panels_visible {
        root.painter().rect_filled(outer, 0.0, background);
    }
    let content = root.available_rect_before_wrap();
    let observed = |r: egui::Rect| {
        Rect::from_corners(Vec2::new(r.min.x, r.min.y), Vec2::new(r.max.x, r.max.y))
    };
    world
        .get_mut::<SurfacePresentation>(frame.view)
        .unwrap()
        .panels = observed(content);
    if !panels_visible || !content.is_positive() {
        state.gesture = None;
        state.preview = None;
        state.layout = None;
        return;
    }
    let bounds = observed(content);
    let mut frame = frame.for_view(frame.view, &mut root);
    let minimum = [
        (minimum_area_size.x * LAYOUT_EXTENT as f32 / bounds.width())
            .ceil()
            .clamp(1.0, LAYOUT_EXTENT as f32) as i32,
        (minimum_area_size.y * LAYOUT_EXTENT as f32 / bounds.height())
            .ceil()
            .clamp(1.0, LAYOUT_EXTENT as f32) as i32,
    ];
    if world.layouts().active().is_none_or(|layout| {
        world
            .get::<LayoutAreas>(layout)
            .is_none_or(|a| a.entities().is_empty())
    }) {
        state.gesture = None;
        state.preview = None;
        state.layout = None;
        return;
    }
    let layout = world.layouts().active().unwrap();
    let topology = layout_topology(world, layout);
    let snapshot = world.layouts_mut().snapshot(layout).unwrap();
    let flush = settings.single_area_edge_to_edge && snapshot.geometry.areas.len() == 1;
    if flush {
        settings.panel_gap = 0.0;
        settings.rounding = 0.0;
    }
    if !settings.layout_editing
        || !settings.drag_corners && matches!(state.gesture, Some(Gesture::Corner { .. }))
        || settings.tab_bar == TabBarVisibility::Hidden
            && matches!(state.gesture, Some(Gesture::Tab { .. }))
    {
        state.gesture = None;
        state.preview = None;
    }
    if state.layout != Some(layout) || state.topology != topology || state.bounds != bounds {
        state.gesture = None;
        state.preview = None;
        if state.layout != Some(layout) {
            state.area_bounds.clear();
            state.content_fades.clear();
        }
    }
    state.layout = Some(layout);
    state.topology = topology;
    state.bounds = bounds;
    let focused = world.get::<Layout>(layout).unwrap().focused_area();
    let groups: Vec<_> = snapshot
        .geometry
        .areas
        .iter()
        .map(|a| {
            let e = snapshot.entity(a.id).unwrap();
            let info = world.get::<Area>(e).unwrap();
            Group {
                entity: e,
                rect: a.rect,
                selected: info.selected_panel(),
                tabs: world
                    .layouts()
                    .panels_in_area(e)
                    .iter()
                    .map(|&entity| Tab::from_world(world, entity))
                    .collect(),
            }
        })
        .collect();
    let ui = &*frame.ui;
    let mouse = ui.input(|i| {
        i.pointer
            .latest_pos()
            .map(|p| Vec2::new(p.x, p.y))
            .unwrap_or(Vec2::splat(-10000.0))
    });
    let point = logical(mouse, bounds);
    let dt = world
        .get_resource::<Time<Real>>()
        .map_or(ui.input(|i| i.stable_dt), |t| t.delta_secs())
        .clamp(0.0, 0.1);
    let blend = if frame.motion.enabled {
        1.0 - (-dt / (frame.motion.seconds * 0.25)).exp()
    } else {
        1.0
    };
    let mut edits = Vec::new();
    let mut preview = None;
    let mut preview_clip = None;
    let mut preview_blend = blend;
    let mut corner_targets = Vec::new();
    let mut geometry = snapshot.geometry.clone();
    let was_interacting = state.gesture.is_some();
    let cancel = ui.input(|i| i.key_pressed(Key::Escape))
        || ui.input(|i| i.pointer.button_pressed(PointerButton::Secondary))
        || ui.input(|i| i.pointer.latest_pos().is_none())
        || !window_focused
        || !ui.is_enabled();
    if cancel && state.gesture.is_some() {
        state.gesture = None;
        state.preview = None;
        state.suppress_context = true;
    }
    if let Some(gesture) = state.gesture.take() {
        let held = ui.input(|i| i.pointer.button_down(PointerButton::Primary));
        let released = ui.input(|i| i.pointer.button_released(PointerButton::Primary));
        match gesture {
            Gesture::Border {
                axis,
                at,
                span,
                pointer_offset,
            } => {
                let to = point[axis.index()] + pointer_offset;
                geometry.move_border(axis.index(), at, span, to, minimum[axis.index()]);
                ui.ctx().set_cursor_icon(if axis == SplitAxis::Horizontal {
                    CursorIcon::ResizeHorizontal
                } else {
                    CursorIcon::ResizeVertical
                });
                if released {
                    edits.push(LayoutEdit::ResizeBorder {
                        axis,
                        at,
                        span,
                        to,
                        minimum: minimum[axis.index()],
                    });
                } else if held {
                    state.gesture = Some(Gesture::Border {
                        axis,
                        at,
                        span,
                        pointer_offset,
                    });
                }
            }
            Gesture::Corner {
                area,
                start,
                axis: mut direction,
            } => {
                let delta = mouse - start;
                if direction.is_none() && delta.abs().max_element() >= 7.0 {
                    direction = Some(axis(usize::from(delta.y.abs() > delta.x.abs())));
                }
                let proposed = corner_target(&snapshot, area, point, direction, minimum);
                if let Some((edit, _, rect)) = proposed {
                    let mut r = screen(rect, bounds);
                    preview = Some(if r.width() == 0.0 || r.height() == 0.0 {
                        // A split marker follows the pointer without easing;
                        // only the committed panels obey the minimum size.
                        preview_blend = 1.0;
                        let source = snapshot.geometry.area(snapshot.id(area).unwrap()).rect;
                        let s = surface(screen(source, bounds), source, settings.panel_gap);
                        let along = if r.width() == 0.0 { 1 } else { 0 };
                        // A two-point stroke stays inside the visible surface,
                        // including its gap and window-edge insets.
                        r.min[along] = s.min[along] + 1.0;
                        r.max[along] = s.max[along] - 1.0;
                        preview_clip = Some(er(s).shrink(1.0));
                        r
                    } else {
                        surface(r, rect, settings.panel_gap)
                    });
                    if released {
                        edits.push(edit);
                    }
                }
                if held {
                    state.gesture = Some(Gesture::Corner {
                        area,
                        start,
                        axis: direction,
                    });
                }
                ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
            }
            Gesture::Tab {
                panel,
                start,
                mut dragging,
            } => {
                dragging |= (mouse - start).abs().max_element() >= 7.0;
                if dragging {
                    if let Some((edit, rect, _count)) = tab_target(
                        &groups,
                        &snapshot,
                        state,
                        Some(panel),
                        mouse,
                        bounds,
                        minimum,
                        settings.panel_gap,
                    ) {
                        preview = Some(rect);
                        if released {
                            edits.push(edit);
                        }
                    }
                }
                if held {
                    state.gesture = Some(Gesture::Tab {
                        panel,
                        start,
                        dragging,
                    });
                }
            }
        }
    }
    let mut moving = false;
    let visual: Vec<_> = geometry
        .areas
        .iter()
        .map(|a| {
            let entity = snapshot.entity(a.id).expect("Border preview retains areas");
            let target = screen(a.rect, bounds);
            let rect = if !was_interacting && frame.motion.enabled {
                // Only an existing area has a previous rectangle. Newly created
                // splits appear at their final size, without entrance motion.
                let from = state.area_bounds.get(&entity).copied().unwrap_or(target);
                expand_rect(from, target, blend)
            } else {
                target
            };
            moving |= rect != target;
            (
                entity,
                a.rect,
                rect,
                surface(rect, a.rect, settings.panel_gap),
            )
        })
        .collect();
    state.area_bounds = visual.iter().map(|(e, _, r, _)| (*e, *r)).collect();
    state.tab_bounds.clear();
    let p = &frame.theme.palette;
    let painter = frame.ui.painter().clone();
    let overlay = frame.ui.ctx().layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        frame.ui.id().with("preview"),
    ));
    for &(entity, _, _, s) in &visual {
        if !settings.layout_editing || !settings.drag_corners {
            continue;
        }
        // Corner ownership precedes border ownership at junctions.
        for (corner, high) in [[false, false], [true, false], [false, true], [true, true]]
            .into_iter()
            .enumerate()
        {
            let zone = corner_hit_rect(s, high, settings.hit_size);
            if zone.width() <= 0.0 || zone.height() <= 0.0 {
                continue;
            }
            let id = frame.ui.id().with(("corner", entity, corner));
            corner_targets.push((id, zone));
            // Interaction uses the previous pass's widgets. Register these
            // handles after panel content below, so they have priority over it.
            let response = frame.ui.ctx().read_response(id);
            if settings.debug {
                let debug = overlay.with_clip_rect(er(bounds));
                debug.rect_filled(
                    er(zone),
                    0.0,
                    egui::Color32::from_rgba_unmultiplied(255, 0, 255, 40),
                );
                debug.rect_stroke(
                    er(zone),
                    0.0,
                    egui::Stroke::new(1.0, egui::Color32::from_rgb(255, 0, 255)),
                    egui::StrokeKind::Inside,
                );
            }
            if !was_interacting && !cancel && state.gesture.is_none() {
                if response.is_some_and(|response| response.is_pointer_button_down_on())
                    && frame
                        .ui
                        .input(|i| i.pointer.button_pressed(PointerButton::Primary))
                {
                    // egui owns the hit, including its interaction radius in
                    // the gutter. The pointer can already have left the handle
                    // if press and motion events arrived in the same frame.
                    let start = frame
                        .ui
                        .input(|i| i.pointer.press_origin())
                        .map_or(mouse, |p| Vec2::new(p.x, p.y));
                    state.gesture = Some(Gesture::Corner {
                        area: entity,
                        start,
                        axis: None,
                    });
                    edits.push(LayoutEdit::FocusArea(entity));
                }
            }
        }
    }
    for &(entity, model, r, _) in &visual {
        if !settings.layout_editing {
            continue;
        }
        for index in 0..2 {
            let at = model.max[index];
            if at == LAYOUT_EXTENT {
                continue;
            }
            let zone = if index == 0 {
                Rect::from_corners(
                    Vec2::new(r.max.x - 4.0, r.min.y + 8.0),
                    Vec2::new(r.max.x + 4.0, r.max.y - 8.0),
                )
            } else {
                Rect::from_corners(
                    Vec2::new(r.min.x + 8.0, r.max.y - 4.0),
                    Vec2::new(r.max.x - 8.0, r.max.y + 4.0),
                )
            };
            let (hovered, pressed) = hit(
                frame.ui,
                &format!("border-{}-{index}", entity.to_bits()),
                zone,
                None,
            );
            if hovered && state.gesture.is_none() {
                frame.ui.ctx().set_cursor_icon(if index == 0 {
                    CursorIcon::ResizeHorizontal
                } else {
                    CursorIcon::ResizeVertical
                });
            }
            if pressed && state.gesture.is_none() && !was_interacting && !cancel {
                state.gesture = Some(Gesture::Border {
                    axis: axis(index),
                    at,
                    span: [model.min[index ^ 1], model.max[index ^ 1]],
                    pointer_offset: at - point[index],
                });
            }
        }
    }
    let background = paints_background(settings.area_background, groups.len());
    for group in &groups {
        let (_, _, r, s) = *visual
            .iter()
            .find(|(e, _, _, _)| *e == group.entity)
            .unwrap();
        let show_tabs = shows_tab_bar(settings.tab_bar, group.tabs.len(), groups.len());
        let bar_height = if show_tabs {
            frame.metrics.font_size + 15.0
        } else {
            0.0
        };
        let bar = Rect::from_corners(s.min, Vec2::new(s.max.x, s.min.y + bar_height));
        let content = Rect::from_corners(
            Vec2::new(s.min.x, bar.max.y + if show_tabs { 1.0 } else { 0.0 }),
            s.max,
        );
        state.tab_bars.insert(group.entity, bar);
        *world.get_mut::<AreaPresentation>(group.entity).unwrap() = AreaPresentation {
            visible: true,
            rect: r,
            surface: s,
            tab_bar: bar,
            content,
        };
        if background {
            painter.rect_filled(
                er(s),
                settings.rounding,
                crate::theme::color(p.panel_background),
            );
        }
        if group.tabs.is_empty() {
            let response = frame.ui.interact(
                er(content),
                frame.ui.id().with(("empty", group.entity)),
                egui::Sense::click(),
            );
            if response.clicked() {
                edits.push(LayoutEdit::FocusArea(group.entity));
            }
            if !state.suppress_context && settings.panel_closure {
                response.context_menu(|ui| {
                    if ui.button("Close area").clicked() {
                        edits.push(LayoutEdit::CloseArea(group.entity));
                        ui.close();
                    }
                });
            }
        }
        if !show_tabs {
            continue;
        }
        let painter = frame.ui.painter().clone();
        painter.line_segment(
            [
                egui::pos2(s.min.x + 1.0, bar.max.y),
                egui::pos2(s.max.x - 1.0, bar.max.y),
            ],
            egui::Stroke::new(1.0, crate::theme::color(p.border)),
        );
        let tabs_min = bar.min;
        let font = egui::FontId::proportional(frame.metrics.font_size);
        let labels: Vec<_> = group
            .tabs
            .iter()
            .map(|t| {
                // Measure and paint the same shaped label, including italics.
                let galley = t.layout_label(&painter, font.clone());
                let width = (galley.size().x
                    + 37.0
                    + if t.dirty { 8.0 } else { 0.0 }
                    + if t.icon.is_some() {
                        frame.metrics.font_size + 5.0
                    } else {
                        0.0
                    })
                .clamp(70.0, 210.0);
                (t, galley, width)
            })
            .collect();
        let total: f32 = labels.iter().map(|(_, _, w)| *w).sum();
        let available = (bar.max.x - tabs_min.x).max(1.0);
        let scroll = state.tab_scroll.entry(group.entity).or_default();
        if Rect::from_corners(tabs_min, bar.max).contains(mouse) {
            let delta = frame.ui.input(|i| i.smooth_scroll_delta);
            scroll.offset -= delta.x + delta.y;
            if matches!(state.gesture, Some(Gesture::Tab { dragging: true, .. }))
                || egui::DragAndDrop::has_any_payload(frame.ui.ctx())
            {
                if mouse.x < tabs_min.x + 18.0 {
                    scroll.offset -= 5.0;
                }
                if mouse.x > bar.max.x - 18.0 {
                    scroll.offset += 5.0;
                }
            }
        }
        if scroll.selected != group.selected {
            let mut left = 0.0;
            for (tab, _, width) in &labels {
                if Some(tab.entity) == group.selected {
                    if left < scroll.offset {
                        scroll.offset = left;
                    }
                    if left + width > scroll.offset + available {
                        scroll.offset = left + width - available;
                    }
                    break;
                }
                left += width;
            }
            scroll.selected = group.selected;
        }
        scroll.offset = scroll.offset.clamp(0.0, (total - available).max(0.0));
        let mut x = tabs_min.x - scroll.offset;
        let tabs_painter = painter.with_clip_rect(er(Rect::from_corners(
            tabs_min,
            bar.max + Vec2::Y / frame.ui.ctx().pixels_per_point(),
        )));
        for (tab, galley, width) in labels {
            let target = Rect::from_corners(
                Vec2::new(x, bar.min.y + PANEL_BORDER_WIDTH),
                Vec2::new(
                    x + width - 1.0,
                    bar.max.y + 1.0 / frame.ui.ctx().pixels_per_point(),
                ),
            );
            x += width;
            let position = target;
            let clipped = position.intersect(bar);
            if clipped.width() <= 0.0 {
                continue;
            }
            state.tab_bounds.insert(tab.entity, clipped);
            let close = Rect::from_corners(
                Vec2::new((position.max.x - 21.0).max(tabs_min.x), position.min.y),
                position.max.min(bar.max),
            );
            let (close_hover, close_pressed) = if settings.panel_closure {
                hit(
                    frame.ui,
                    &format!("close-{}", tab.entity.to_bits()),
                    close,
                    Some(&format!("Close {}", tab.title)),
                )
            } else {
                (false, false)
            };
            let zone = Rect::from_corners(
                clipped.min,
                Vec2::new(close.min.x.min(clipped.max.x), clipped.max.y),
            );
            let response = frame.ui.interact(
                er(zone),
                frame.ui.id().with(("tab", tab.entity)),
                egui::Sense::click_and_drag(),
            );
            let enabled = frame.ui.is_enabled();
            let is_selected = group.selected == Some(tab.entity);
            response.widget_info(|| {
                egui::WidgetInfo::selected(
                    egui::WidgetType::SelectableLabel,
                    enabled,
                    is_selected,
                    &tab.title,
                )
            });
            let hovered = response.hovered();
            let pressed = response.is_pointer_button_down_on()
                && frame
                    .ui
                    .input(|i| i.pointer.button_pressed(PointerButton::Primary));
            let selected = group.selected == Some(tab.entity);
            let hover_seconds = frame.motion.seconds * 0.5;
            let hover = frame.animate_bool_with_duration(
                response.id.with("hover-background"),
                hovered || close_hover,
                hover_seconds,
            );
            if hover > 0.0 {
                let mut tint = p.text;
                tint[3] = 0.12;
                let mut background = position;
                background.min.x = clipped.min.x;
                background.max.x = clipped.max.x;
                background.min.y = background.max.y - background.height() * hover;
                tabs_painter.rect_filled(
                    er(background),
                    egui::CornerRadius {
                        sw: 0,
                        se: 0,
                        ..egui::CornerRadius::from(settings.rounding)
                    },
                    crate::theme::color(tint),
                );
            }
            let emphasis = frame.animate_bool_with_duration(
                response.id.with("text-color"),
                selected || hovered || close_hover,
                hover_seconds,
            );
            let text = crate::theme::color(crate::theme::mix(p.muted, p.text, emphasis));
            let text_x = position.min.x
                + 8.0
                + if tab.icon.is_some() {
                    frame.metrics.font_size + 5.0
                } else {
                    0.0
                };
            tabs_painter
                .with_clip_rect(er(Rect::from_corners(
                    Vec2::new(position.min.x + 8.0, bar.min.y),
                    Vec2::new(close.min.x - 3.0, bar.max.y),
                )))
                .galley(
                    egui::pos2(text_x, bar.center().y - galley.size().y * 0.5),
                    galley,
                    tab.label_color(text),
                );
            if let Some(icon) = &tab.icon {
                // icon_at paints through the Ui, so use the tab painter's clip.
                let clip = frame.ui.clip_rect();
                frame.ui.set_clip_rect(tabs_painter.clip_rect());
                frame.icon_at(
                    icon,
                    [
                        position.min.x + 8.0,
                        bar.center().y - frame.metrics.font_size * 0.5,
                    ],
                    frame.metrics.font_size,
                );
                frame.ui.set_clip_rect(clip);
            }
            if tab.dirty {
                tabs_painter.circle_filled(
                    egui::pos2(close.min.x - 5.0, bar.center().y),
                    2.0,
                    text,
                );
            }
            let close_tint = frame.animate_bool_with_duration(
                response.id.with("close-color"),
                close_hover,
                hover_seconds,
            );
            if emphasis > 0.0 && settings.panel_closure {
                let c = egui::pos2(close.center().x, bar.center().y);
                let mut fill = p.elevated;
                fill[3] *= close_tint;
                tabs_painter.circle_filled(c, 8.0, crate::theme::color(fill));
                let text = text.gamma_multiply(emphasis);
                tabs_painter.line_segment(
                    [c - egui::vec2(3.0, 3.0), c + egui::vec2(3.0, 3.0)],
                    egui::Stroke::new(1.0, text),
                );
                tabs_painter.line_segment(
                    [c + egui::vec2(3.0, -3.0), c + egui::vec2(-3.0, 3.0)],
                    egui::Stroke::new(1.0, text),
                );
            }
            {
                let mut observation = world.get_mut::<PanelPresentation>(tab.entity).unwrap();
                observation.tab_rect = zone;
                observation.close_rect = close;
            }
            frame.tooltip(&response, &tab.title);
            if settings.panel_closure
                && (close_pressed
                    || (clipped.contains(mouse)
                        && frame
                            .ui
                            .input(|i| i.pointer.button_released(PointerButton::Middle))))
            {
                edits.push(LayoutEdit::ClosePanels(vec![tab.entity]));
            }
            if pressed && state.gesture.is_none() && !was_interacting {
                edits.push(LayoutEdit::FocusPanel(tab.entity));
                if response.double_clicked() {
                    edits.push(LayoutEdit::KeepPanel(tab.entity));
                }
                if settings.layout_editing {
                    state.gesture = Some(Gesture::Tab {
                        panel: tab.entity,
                        start: mouse,
                        dragging: false,
                    });
                }
            }
            if !state.suppress_context && settings.panel_closure {
                response.context_menu(|ui| {
                    let at = group
                        .tabs
                        .iter()
                        .position(|t| t.entity == tab.entity)
                        .unwrap();
                    for (label, range, except) in [
                        ("Close", at..at + 1, None),
                        ("Close All", 0..group.tabs.len(), None),
                        ("Close Others", 0..group.tabs.len(), Some(at)),
                        ("Close to the Right", at + 1..group.tabs.len(), None),
                        ("Close to the Left", 0..at, None),
                    ] {
                        let tabs: Vec<_> = range
                            .filter(|i| Some(*i) != except)
                            .map(|i| group.tabs[i].entity)
                            .collect();
                        if ui
                            .add_enabled(!tabs.is_empty(), egui::Button::new(label))
                            .clicked()
                        {
                            edits.push(LayoutEdit::ClosePanels(tabs));
                            ui.close();
                        }
                    }
                });
            }
        }
    }
    for group in &groups {
        if let Some(panel) = group.selected {
            let presentation = *world.get::<AreaPresentation>(group.entity).unwrap();
            let content = presentation.content;
            if content.width() < 1.0 || content.height() < 1.0 {
                continue;
            }
            let now = frame.ui.input(|i| i.time);
            let fade = state.content_fades.entry(panel).or_insert(ContentFade {
                area: group.entity,
                placement: group.rect,
                started_at: now,
            });
            // Start at first display, excluding the preceding idle/frame time.
            // UI time is unchanged across egui's repeated layout passes.
            // Compare committed placement, not the interpolated screen bounds.
            let opacity = fade.opacity(group.entity, group.rect, now, frame.motion);
            moving |= opacity < 1.0;
            let focus = focused == Some(group.entity);
            *world.get_mut::<PanelPresentation>(panel).unwrap() = PanelPresentation {
                visible: true,
                focused: focus,
                rect: content,
                tab_rect: world.get::<PanelPresentation>(panel).unwrap().tab_rect,
                close_rect: world.get::<PanelPresentation>(panel).unwrap().close_rect,
            };
            if frame.ui.is_enabled()
                && content.contains(mouse)
                && frame.ui.input(|i| i.pointer.any_pressed())
                && !frame.ui.ctx().any_popup_open()
            {
                edits.push(LayoutEdit::FocusArea(group.entity));
            }
            frame.scope(
                egui::UiBuilder::new()
                    .id_salt(panel.to_bits())
                    .max_rect(er(content))
                    .layout(egui::Layout::top_down(egui::Align::Min)),
                |mut content_frame| {
                    content_frame.ui.set_clip_rect(er(content));
                    content_frame.ui.multiply_opacity(opacity);
                    content_frame.view = panel;
                    content_frame.clip_rounded(
                        er(presentation.surface),
                        settings.rounding,
                        |content_frame| {
                            if let Some(draw) =
                                world.get::<PanelInstance>(panel).and_then(|p| p.draw)
                            {
                                let context = content_frame.context();
                                draw(content_frame.ui, world, &context, panel);
                            } else {
                                content_frame.ui.weak("This panel has no content renderer.");
                            }
                        },
                    );
                },
            );
        }
    }
    state.content_fades.retain(|panel, _| {
        world
            .get::<PanelPresentation>(*panel)
            .is_some_and(|presentation| presentation.visible)
    });
    // Keep shell handles above content widgets on the same layer. Popups and
    // modals use higher layers and retain their normal egui input priority.
    for (id, zone) in corner_targets {
        frame
            .ui
            .interact(er(zone), id, egui::Sense::click_and_drag())
            .on_hover_and_drag_cursor(CursorIcon::Crosshair);
    }
    // Draw borders after content so edge-to-edge surfaces retain their outline.
    for &(entity, _, _, s) in &visual {
        if flush {
            continue;
        }
        let focus = frame.animate_bool(
            frame.ui.id().with(("area-focus", entity.to_bits())),
            Some(entity) == focused,
        );
        let mut border = p.border;
        for v in &mut border[..3] {
            *v += (1.0 - *v) * 0.2 * focus;
        }
        painter.rect_stroke(
            er(s),
            settings.rounding,
            egui::Stroke::new(PANEL_BORDER_WIDTH, crate::theme::color(border)),
            egui::StrokeKind::Inside,
        );
    }
    // Content has first opportunity to take a typed egui payload. The shell
    // receives only the remaining payload, preserving widget target priority.
    if let Some(payload) =
        egui::DragAndDrop::payload::<PanelFactoryPayload>(frame.ui.ctx()).filter(|_| {
            frame.ui.is_enabled()
                && settings.layout_editing
                && settings.panel_creation
                && !PanelFactoryPayload::content_reserved(frame.ui.ctx())
        })
    {
        if cancel {
            state.payload_cancelled = true;
            state.suppress_context = true;
        }
        if !state.payload_cancelled {
            if let Some((mut edit, rect, _count)) = tab_target(
                &groups,
                &snapshot,
                state,
                None,
                mouse,
                bounds,
                minimum,
                settings.panel_gap,
            ) {
                preview = Some(rect);
                if frame
                    .ui
                    .input(|i| i.pointer.button_released(PointerButton::Primary))
                {
                    if let LayoutEdit::DropPanel { factory, .. } = &mut edit {
                        *factory = payload.0.clone();
                    }
                    egui::DragAndDrop::take_payload::<PanelFactoryPayload>(frame.ui.ctx());
                    edits.push(edit);
                }
            }
        }
    } else if let Some(FileDrops(drops)) = world.get_resource::<FileDrops>().copied()
        && frame.ui.is_enabled()
        && settings.layout_editing
        && settings.panel_creation
        && !PanelFactoryPayload::content_reserved(frame.ui.ctx())
    {
        // Files dragged in from outside the app: shown where they'd go, and
        // opened there when dropped (the app says how).
        state.payload_cancelled = false;
        let (hovering, dropped) = frame.ui.input(|i| {
            let dropped: Vec<std::path::PathBuf> = i
                .raw
                .dropped_files
                .iter()
                .map(|file| file.path().to_owned())
                .collect();
            (!i.raw.hovered_files.is_empty(), dropped)
        });
        if (hovering || !dropped.is_empty())
            && let Some((mut edit, rect, _count)) = tab_target(
                &groups,
                &snapshot,
                state,
                None,
                mouse,
                bounds,
                minimum,
                settings.panel_gap,
            )
        {
            preview = Some(rect);
            if !dropped.is_empty() {
                match drops(world, dropped) {
                    Some(factory) => {
                        if let LayoutEdit::DropPanel { factory: slot, .. } = &mut edit {
                            *slot = factory;
                        }
                        edits.push(edit);
                    }
                    None => preview = None,
                }
            }
        }
    } else {
        state.payload_cancelled = false;
    }
    state
        .tab_scroll
        .retain(|e, _| world.get::<Area>(*e).is_some());
    let painter = overlay;
    if let Some(target) = preview {
        let painter =
            preview_clip.map_or_else(|| painter.clone(), |clip| painter.with_clip_rect(clip));
        let current = blend_rect(state.preview.unwrap_or(target), target, preview_blend);
        moving |= current != target;
        state.preview = Some(current);
        crate::paint_drop_overlay(&painter, p, er(current), settings.rounding);
    } else {
        state.preview = None;
    }
    if (was_interacting || state.gesture.is_some()) && cancel {
        frame.ui.input_mut(|input| {
            input.consume_key(egui::Modifiers::NONE, Key::Escape);
        });
    }
    if moving || state.gesture.is_some() {
        frame.ui.ctx().request_repaint();
        world.write_message(bevy::window::RequestRedraw);
    }
    if state.suppress_context
        && !frame.ui.input(|i| {
            i.pointer.button_down(PointerButton::Secondary)
                || i.pointer.button_released(PointerButton::Secondary)
        })
    {
        state.suppress_context = false;
    }
    for edit in edits {
        if !allows_panel_edit(settings, &edit) {
            continue;
        }
        world
            .resource_mut::<crate::layouts::PendingEdits>()
            .0
            .push((layout, edit));
    }
}

/// What files dragged in from outside the app become, where they're dropped
/// on a layout: a panel factory (given the paths), or `None` when the app
/// dealt with them itself. Content (a file tree) can take the drop first,
/// with `PanelFactoryPayload::reserve_for_content`.
#[derive(Resource, Clone, Copy)]
pub struct FileDrops(pub fn(&mut World, Vec<std::path::PathBuf>) -> Option<PanelFactory>);

/// Offer an application-provided constructor, not a registered panel type.
/// The caller can attach ordinary egui tooltip content to the response.
pub fn panel_drag_source(response: &egui::Response, factory: PanelFactory) {
    response.dnd_set_drag_payload(PanelFactoryPayload::new(factory));
}

// Placeholder used only while computing a drop preview; replaced by the payload
// before any edit is submitted.
fn preview_factory() -> PanelFactory {
    PanelFactory::new("Preview", |_, _| {
        panic!("Unresolved panel creation preview")
    })
}

fn tab_target(
    groups: &[Group],
    snapshot: &GeometrySnapshot,
    state: &Frontend,
    panel: Option<Entity>,
    mouse: Vec2,
    bounds: Rect,
    minimum: [i32; 2],
    panel_gap: f32,
) -> Option<(LayoutEdit, Rect, usize)> {
    for group in groups {
        let r = state.area_bounds.get(&group.entity).copied()?;
        let s = surface(r, group.rect, panel_gap);
        let bar = state.tab_bars.get(&group.entity).copied()?;
        let bar_bottom = bar.max.y;
        if bar.contains(mouse) {
            let mut index = 0;
            let mut x = bar.min.x;
            for tab in group.tabs.iter().filter(|t| Some(t.entity) != panel) {
                if let Some(r) = state.tab_bounds.get(&tab.entity) {
                    if mouse.x < r.center().x {
                        break;
                    }
                    x = r.max.x;
                }
                index += 1;
            }
            let edit = if let Some(panel) = panel {
                LayoutEdit::MoveTab {
                    panel,
                    area: group.entity,
                    index,
                }
            } else {
                LayoutEdit::DropPanel {
                    area: group.entity,
                    factory: preview_factory(),
                    index,
                    split: None,
                    minimum,
                }
            };
            return Some((
                edit,
                Rect::from_corners(Vec2::new(x, s.min.y + 3.0), Vec2::new(x, bar_bottom - 3.0)),
                groups.len(),
            ));
        }
    }
    let point = logical(mouse, bounds);
    let target = snapshot
        .geometry
        .areas
        .iter()
        .find(|a| a.rect.contains(point))?;
    let entity = snapshot.entity(target.id)?;
    let (source, last_tab) = if let Some(panel) = panel {
        let source = groups
            .iter()
            .find(|g| g.tabs.iter().any(|t| t.entity == panel))?;
        (snapshot.id(source.entity).ok()?, source.tabs.len() == 1)
    } else {
        // A factory creates a new instance in the target; no source disappears.
        (target.id, false)
    };
    let relative = Vec2::new(
        (point[0] - target.rect.min[0]) as f32 / (target.rect.max[0] - target.rect.min[0]) as f32,
        (point[1] - target.rect.min[1]) as f32 / (target.rect.max[1] - target.rect.min[1]) as f32,
    );
    let edges = [relative.x, 1.0 - relative.x, relative.y, 1.0 - relative.y];
    let edge = (0..4).min_by(|a, b| edges[*a].total_cmp(&edges[*b]))?;
    if edges[edge] < 0.25 {
        let index = edge / 2;
        let high = edge % 2 == 1;
        let (next, id) = plan_tab_geometry(
            &snapshot.geometry,
            source,
            target.id,
            last_tab,
            Some((axis(index), high)),
            minimum,
        )
        .ok()?;
        let destination = next.area(id).rect;
        let edit = if let Some(panel) = panel {
            LayoutEdit::SplitTab {
                panel,
                area: entity,
                axis: axis(index),
                high,
                minimum,
            }
        } else {
            LayoutEdit::DropPanel {
                area: entity,
                factory: preview_factory(),
                index: usize::MAX,
                split: Some((axis(index), high)),
                minimum,
            }
        };
        Some((
            edit,
            surface(screen(destination, bounds), destination, panel_gap),
            next.areas.len(),
        ))
    } else {
        let (next, id) = plan_tab_geometry(
            &snapshot.geometry,
            source,
            target.id,
            last_tab,
            None,
            minimum,
        )
        .ok()?;
        let destination = next.area(id).rect;
        let index = groups
            .iter()
            .find(|g| g.entity == entity)?
            .tabs
            .iter()
            .position(|t| Some(t.entity) == panel)
            .unwrap_or(usize::MAX);
        let edit = if let Some(panel) = panel {
            LayoutEdit::MoveTab {
                panel,
                area: entity,
                index,
            }
        } else {
            LayoutEdit::DropPanel {
                area: entity,
                factory: preview_factory(),
                index,
                split: None,
                minimum,
            }
        };
        Some((
            edit,
            surface(screen(destination, bounds), destination, panel_gap),
            next.areas.len(),
        ))
    }
}

fn corner_target(
    snapshot: &GeometrySnapshot,
    source: Entity,
    point: [i32; 2],
    direction: Option<SplitAxis>,
    minimum: [i32; 2],
) -> Option<(LayoutEdit, geometry::Layout, geometry::Rect)> {
    let index = direction?.index();
    let source_id = snapshot.id(source).ok()?;
    let rect = snapshot.geometry.area(source_id).rect;
    if let Some(target) = snapshot
        .geometry
        .areas
        .iter()
        .find(|a| a.id != source_id && a.rect.contains(point))
    {
        let target_entity = snapshot.entity(target.id)?;
        let joining = (0..2).any(|a| {
            let p = a ^ 1;
            let length = target.rect.max[a] - target.rect.min[a];
            let margin = if length < minimum[a] * 2 {
                length
            } else {
                (length / 4).min(minimum[a] * 5)
            };
            rect.min[p] < target.rect.max[p]
                && target.rect.min[p] < rect.max[p]
                && point[p] >= rect.min[p]
                && point[p] <= rect.max[p]
                && ((rect.max[a] == target.rect.min[a] && point[a] <= target.rect.min[a] + margin)
                    || (rect.min[a] == target.rect.max[a]
                        && point[a] >= target.rect.max[a] - margin))
        });
        let (edit, plan) = if joining {
            (
                LayoutEdit::JoinAreas {
                    source,
                    target: target_entity,
                    minimum,
                },
                snapshot.geometry.plan_join(source_id, target.id, minimum)?,
            )
        } else {
            let relative = [0, 1].map(|a| {
                (point[a] - target.rect.min[a]) as f32
                    / (target.rect.max[a] - target.rect.min[a]) as f32
            });
            let placement = if relative.iter().all(|v| (0.4..=0.6).contains(v)) {
                AreaPlacement::Replace
            } else {
                let edges = [
                    relative[0],
                    1.0 - relative[0],
                    relative[1],
                    1.0 - relative[1],
                ];
                let edge = (0..4).min_by(|a, b| edges[*a].total_cmp(&edges[*b]))?;
                let f = edges[edge] * 2.0;
                let fraction = if (f - 0.5).abs() <= 0.04 { 0.5 } else { f };
                AreaPlacement::Split {
                    axis: axis(edge / 2),
                    high: edge % 2 == 1,
                    fraction,
                }
            };
            let p = match placement {
                AreaPlacement::Replace => geometry::DockPlacement::Replace,
                AreaPlacement::Split {
                    axis,
                    high,
                    fraction,
                } => geometry::DockPlacement::Split {
                    axis: axis.index(),
                    high,
                    fraction: (fraction * LAYOUT_EXTENT as f32).round() as i32,
                },
            };
            (
                LayoutEdit::DockArea {
                    source,
                    target: target_entity,
                    placement,
                    minimum,
                },
                snapshot
                    .geometry
                    .plan_dock(source_id, target.id, p, minimum)?,
            )
        };
        let preview = plan.layout.area(source_id).rect;
        Some((edit, plan.layout, preview))
    } else if rect.contains(point) {
        if rect.max[index] - rect.min[index] < minimum[index] * 2 {
            return None;
        }
        let mut next = snapshot.geometry.clone();
        let at = point[index].clamp(
            rect.min[index] + minimum[index],
            rect.max[index] - minimum[index],
        );
        next.split(source_id, index, at, minimum[index])?;
        let mut line = rect;
        line.min[index] = point[index];
        line.max[index] = point[index];
        Some((
            LayoutEdit::SplitArea {
                area: source,
                axis: direction.unwrap(),
                at,
                minimum,
            },
            next,
            line,
        ))
    } else {
        None
    }
}

/// Whether an area with `tabs` tabs shows its tab bar, in a layout of `areas`.
fn shows_tab_bar(visibility: TabBarVisibility, tabs: usize, areas: usize) -> bool {
    match visibility {
        TabBarVisibility::Always => true,
        TabBarVisibility::MultipleTabs => tabs > 1,
        TabBarVisibility::MultipleAreas => areas > 1,
        TabBarVisibility::MultipleTabsOrAreas => tabs > 1 || areas > 1,
        TabBarVisibility::Hidden => false,
    }
}

/// Whether areas paint their background, in a layout of `areas`.
fn paints_background(background: AreaBackground, areas: usize) -> bool {
    match background {
        AreaBackground::Always => true,
        AreaBackground::MultipleAreas => areas > 1,
        AreaBackground::Never => false,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_lone_panel_can_drop_its_tab_bar_and_background() {
        use super::{AreaBackground as B, TabBarVisibility as T, *};
        // (tabs in the area, areas in the layout)
        let cases = [(1, 1), (2, 1), (1, 2), (2, 2)];
        let shown = |visibility| cases.map(|(tabs, areas)| shows_tab_bar(visibility, tabs, areas));
        assert_eq!(shown(T::Always), [true; 4]);
        assert_eq!(shown(T::MultipleTabs), [false, true, false, true]);
        assert_eq!(shown(T::MultipleAreas), [false, false, true, true]);
        // Only one area with one tab is just its panel.
        assert_eq!(shown(T::MultipleTabsOrAreas), [false, true, true, true]);
        assert_eq!(shown(T::Hidden), [false; 4]);

        assert!(paints_background(B::Always, 1));
        assert!(!paints_background(B::MultipleAreas, 1));
        assert!(paints_background(B::MultipleAreas, 2));
        assert!(!paints_background(B::Never, 2));
    }

    use super::*;
    use crate::UiView;
    use crate::layouts::tests::TestPanels;

    #[test]
    fn panel_fade_starts_at_display_and_advances_only_with_elapsed_ui_time() {
        let area = Entity::PLACEHOLDER;
        let placement = geometry::Rect {
            min: [0, 0],
            max: [LAYOUT_EXTENT, LAYOUT_EXTENT],
        };
        let motion = crate::MotionPolicy::default();
        // First display follows a long idle. A second layout pass at the same
        // timestamp must paint the same opacity, even partway through the fade.
        let now = 300.0;
        let mut fade = ContentFade {
            area,
            placement,
            started_at: now,
        };
        assert_eq!(fade.opacity(area, placement, now, motion), 0.0);
        assert_eq!(fade.opacity(area, placement, now, motion), 0.0);
        let halfway = now + f64::from(motion.seconds) * 0.5;
        assert_eq!(fade.opacity(area, placement, halfway, motion), 0.875);
        assert_eq!(fade.opacity(area, placement, halfway, motion), 0.875);
        assert_eq!(fade.opacity(area, placement, now + 1.0, motion), 1.0);

        let moved = geometry::Rect {
            max: [LAYOUT_EXTENT / 2, LAYOUT_EXTENT],
            ..placement
        };
        assert_eq!(fade.opacity(area, moved, now + 20.0, motion), 0.0);
        assert_eq!(fade.opacity(area, moved, now + 21.0, motion), 1.0);
        let other = World::new().spawn_empty().id();
        assert_eq!(fade.opacity(other, moved, now + 30.0, motion), 0.0);
    }

    #[test]
    fn panel_fade_settles_immediately_with_reduced_motion() {
        let area = Entity::PLACEHOLDER;
        let placement = geometry::Rect {
            min: [0, 0],
            max: [LAYOUT_EXTENT, LAYOUT_EXTENT],
        };
        let mut motion = crate::MotionPolicy::default();
        let mut fade = ContentFade {
            area,
            placement,
            started_at: 10.0,
        };
        motion.enabled = false;
        assert_eq!(fade.opacity(area, placement, 10.0, motion), 1.0);
        motion.enabled = true;
        assert_eq!(fade.opacity(area, placement, 10.1, motion), 1.0);
    }

    #[test]
    fn tab_style_overrides_are_per_item_and_follow_live_component_edits() {
        let mut world = World::new();
        let normal = world.spawn(PanelInstance::default()).id();
        let preview = world
            .spawn(PanelInstance {
                preview: true,
                ..default()
            })
            .id();
        let preview_style = PanelTabStyle {
            italic: true,
            text_opacity: 0.9,
        };
        let custom = PanelTabStyle {
            italic: true,
            text_opacity: 0.5,
        };
        assert_eq!(
            Tab::from_world(&world, normal).style,
            PanelTabStyle::default()
        );
        assert_eq!(Tab::from_world(&world, preview).style, preview_style);

        world.entity_mut(normal).insert(custom);
        world.entity_mut(preview).insert(PanelTabStyle::default());
        assert_eq!(Tab::from_world(&world, normal).style, custom);
        assert_eq!(
            Tab::from_world(&world, preview).style,
            PanelTabStyle::default()
        );
        assert!(!world.get::<PanelInstance>(normal).unwrap().preview);
        assert!(world.get::<PanelInstance>(preview).unwrap().preview);

        world
            .get_mut::<PanelTabStyle>(preview)
            .unwrap()
            .text_opacity = 0.7;
        assert_eq!(Tab::from_world(&world, preview).style.text_opacity, 0.7);
        world.entity_mut(preview).remove::<PanelTabStyle>();
        assert_eq!(Tab::from_world(&world, preview).style, preview_style);
        world.get_mut::<PanelInstance>(preview).unwrap().preview = false;
        assert_eq!(
            Tab::from_world(&world, preview).style,
            PanelTabStyle::default()
        );
        assert_eq!(Tab::from_world(&world, normal).style, custom);
    }

    #[test]
    fn tab_labels_shape_italics_with_the_theme_font_and_preserve_themed_emphasis() {
        let mut world = World::new();
        let normal = world
            .spawn(PanelInstance {
                title: "Normal label".into(),
                ..default()
            })
            .id();
        let preview = world
            .spawn(PanelInstance {
                title: "Preview label".into(),
                preview: true,
                ..default()
            })
            .id();
        let custom = world
            .spawn((
                PanelInstance {
                    title: "Custom label".into(),
                    ..default()
                },
                PanelTabStyle {
                    italic: true,
                    text_opacity: 0.5,
                },
            ))
            .id();
        for density in [1.0, 1.25, 2.0] {
            for mut theme in [crate::Theme::dark(), crate::Theme::light()] {
                // The opacity multiplier must preserve an already-translucent theme.
                theme.palette.text[3] = 0.8;
                theme.palette.muted[3] = 0.6;
                let ctx = egui::Context::default();
                ctx.set_pixels_per_point(density);
                ctx.set_fonts(crate::fonts::definitions());
                ctx.options_mut(|o| o.max_passes = std::num::NonZeroUsize::new(1).unwrap());
                let mut measured = Vec::new();
                let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                    for size in [12.0, 15.5, 24.0] {
                        let font = egui::FontId::proportional(size);
                        for (panel, italic, opacity) in [
                            (normal, false, 1.0),
                            (preview, true, 0.9),
                            (custom, true, 0.5),
                        ] {
                            let tab = Tab::from_world(&world, panel);
                            let galley = tab.layout_label(ui.painter(), font.clone());
                            let format = &galley.job.sections[0].format;
                            assert_eq!(format.font_id, font);
                            assert_eq!(format.italics, italic);
                            assert_eq!(format.underline, egui::Stroke::NONE);
                            assert_eq!(format.color, egui::Color32::PLACEHOLDER);
                            if panel == normal {
                                let old = ui.painter().layout_no_wrap(
                                    tab.title.clone(),
                                    font.clone(),
                                    egui::Color32::WHITE,
                                );
                                assert_eq!(
                                    galley.size(),
                                    old.size(),
                                    "Normal tab metrics stay unchanged"
                                );
                            } else {
                                let regular = ui.painter().layout_no_wrap(
                                    tab.title.clone(),
                                    font.clone(),
                                    egui::Color32::PLACEHOLDER,
                                );
                                assert_ne!(
                                    galley.rows[0].visuals.mesh, regular.rows[0].visuals.mesh,
                                    "Italic text must change the actual glyph rendering"
                                );
                            }
                            for emphasis in [0.0, 0.5, 1.0] {
                                let themed = crate::theme::color(crate::theme::mix(
                                    theme.palette.muted,
                                    theme.palette.text,
                                    emphasis,
                                ));
                                let color = tab.label_color(themed);
                                assert_eq!(color, themed.gamma_multiply(opacity));
                                ui.painter()
                                    .galley(egui::pos2(8.0, 8.0), galley.clone(), color);
                                measured.push((galley.clone(), color));
                            }
                        }
                    }
                });
                let painted: Vec<_> = output
                    .shapes
                    .iter()
                    .filter_map(|shape| match &shape.shape {
                        egui::Shape::Text(text) => Some(text),
                        _ => None,
                    })
                    .collect();
                assert_eq!(painted.len(), measured.len());
                for (text, (galley, color)) in painted.iter().zip(&measured) {
                    assert_eq!(text.galley.job, galley.job);
                    assert_eq!(text.galley.size(), galley.size());
                    assert_eq!(text.fallback_color, *color);
                    assert_eq!(text.underline, egui::Stroke::NONE);
                }
                output.textures_delta.clear();
            }
        }
    }

    #[test]
    fn tab_label_opacity_has_defined_rendering_at_its_boundaries() {
        let mut world = World::new();
        let panel = world.spawn(PanelInstance::default()).id();
        let themed = egui::Color32::from_rgba_unmultiplied(80, 140, 200, 180);
        for (opacity, expected) in [
            (-1.0, 0.0),
            (0.0, 0.0),
            (0.4, 0.4),
            (1.0, 1.0),
            (2.0, 1.0),
            (f32::NAN, 1.0),
            (f32::INFINITY, 1.0),
            (f32::NEG_INFINITY, 1.0),
        ] {
            world.entity_mut(panel).insert(PanelTabStyle {
                italic: false,
                text_opacity: opacity,
            });
            assert_eq!(
                Tab::from_world(&world, panel).label_color(themed),
                themed.gamma_multiply(expected)
            );
        }
    }

    #[test]
    fn free_panel_creation_uses_the_supplied_inactive_layout() {
        let mut app = App::new();
        app.add_plugins(crate::layouts::LayoutsPlugin);
        let active = crate::layouts::tests::layout(app.world_mut());
        let other = crate::layouts::tests::layout(app.world_mut());
        let area = app
            .world_mut()
            .layouts_mut()
            .destination_area(other)
            .unwrap();
        let panel = app
            .world_mut()
            .spawn((
                PanelInstance {
                    title: "Inactive".into(),
                    draw: Some(|_, _, _, _| {}),
                    ..default()
                },
                InArea(area),
            ))
            .id();
        assert_eq!(
            app.world_mut().layouts_mut().panel_layout(panel).unwrap(),
            other
        );
        assert_eq!(app.world().layouts().active(), Some(active));
    }

    #[test]
    fn user_permissions_do_not_restrict_application_layout_operations() {
        let mut app = App::new();
        app.add_plugins(crate::layouts::LayoutsPlugin);
        crate::layouts::tests::layout(app.world_mut());
        app.world_mut()
            .spawn((Window::default(), LayoutSurface::single_panel()));
        let w = app.world_mut().layouts_mut();
        let layout = w.active().unwrap();
        let area = w.destination_area(layout).unwrap();
        let panel = w.create_panel(layout, "Application content").unwrap();
        assert!(
            w.user_edit(w.active().unwrap(), &LayoutEdit::ClosePanels(vec![panel]))
                .is_err()
        );
        assert!(
            w.user_edit(
                w.active().unwrap(),
                &LayoutEdit::DropPanel {
                    area,
                    factory: preview_factory(),
                    index: 0,
                    split: None,
                    minimum: [100, 100],
                }
            )
            .is_err()
        );
        assert!(
            w.user_edit(
                w.active().unwrap(),
                &LayoutEdit::SplitArea {
                    area,
                    axis: SplitAxis::Horizontal,
                    at: 5000,
                    minimum: [100, 100],
                }
            )
            .is_err()
        );
        let split = w
            .split_area(area, SplitAxis::Horizontal, 5000, [100, 100])
            .unwrap();
        assert_ne!(split, area);
        w.close_panel(panel).unwrap();
        assert!(w.panels(layout).is_empty());
        assert!(w.create_panel(layout, "Replacement content").is_ok());
    }

    #[test]
    fn titlebar_excludes_native_controls_at_either_edge_and_scales_with_zoom() {
        let outer = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        for zoom in [0.75, 1.0, 2.0] {
            for right in [false, true] {
                let x = if right {
                    outer.width() * zoom - 80.0
                } else {
                    8.0
                };
                let controls = Rect::from_corners(Vec2::new(x, 6.0), Vec2::new(x + 72.0, 22.0));
                let (band, content) = titlebar_rects(outer, 28.0, Some(controls), 32.0, zoom);
                assert_eq!(band.height(), 36.0_f32.max(28.0 / zoom));
                let controls = egui::Rect::from_min_max(
                    egui::pos2(controls.min.x / zoom, controls.min.y / zoom),
                    egui::pos2(controls.max.x / zoom, controls.max.y / zoom),
                );
                if controls.intersects(band) {
                    assert!(!content.intersects(controls));
                    let gap = if right {
                        controls.left() - content.right()
                    } else {
                        content.left() - controls.right()
                    };
                    assert!(gap >= 12.0 - 0.001);
                }
                assert!(band.contains_rect(content));
            }
        }
    }

    #[test]
    fn titlebar_fallback_embedded_bounds_and_hidden_controls_have_no_native_gap() {
        let outer = egui::Rect::from_min_size(egui::pos2(100.0, 80.0), egui::vec2(400.0, 250.0));
        let controls = Rect::from_corners(Vec2::new(8.0, 6.0), Vec2::new(80.0, 22.0));
        let fallback = titlebar_rects(outer, 0.0, None, 32.0, 1.0);
        assert_eq!(
            fallback,
            titlebar_rects(outer, 28.0, Some(controls), 32.0, 1.0)
        );
        assert_eq!(fallback.0.height(), 36.0);
        assert_eq!(fallback.1.top(), outer.top() + 4.0);
        assert_eq!(fallback.1.height(), 32.0);
        assert_eq!(
            titlebar_rects(outer, 28.0, Some(controls), 0.0, 1.0)
                .0
                .height(),
            0.0
        );
        let tiny = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(20.0, 12.0));
        let (band, content) = titlebar_rects(tiny, 28.0, Some(controls), 32.0, 1.0);
        assert_eq!(band.height(), 12.0);
        assert!(band.contains_rect(content));
    }

    #[test]
    fn replacing_and_removing_the_titlebar_keeps_the_surface_and_root() {
        let mut app = App::new();
        app.add_plugins(super::LayoutsPlugin);
        let surface = app
            .world_mut()
            .spawn((
                Window::default(),
                Titlebar {
                    height: 32.0,
                    draw: |_, _, _| {},
                },
            ))
            .id();
        assert!(app.world().get::<LayoutSurface>(surface).is_some());
        app.world_mut().spawn((Camera2d, LayoutCamera));
        app.update();
        let root = app
            .world_mut()
            .query_filtered::<Entity, With<UiView>>()
            .single(app.world())
            .unwrap();
        app.world_mut().entity_mut(surface).insert(Titlebar {
            height: 40.0,
            draw: |_, _, _| {},
        });
        app.update();
        assert_eq!(app.world().get::<Titlebar>(surface).unwrap().height, 40.0);
        assert!(app.world().get::<UiView>(root).is_some());
        app.world_mut().entity_mut(surface).remove::<Titlebar>();
        app.update();
        assert!(app.world().get::<Titlebar>(surface).is_none());
        assert!(app.world().get::<LayoutSurface>(surface).is_some());
        assert!(app.world().get::<UiView>(root).is_some());
    }

    #[test]
    fn tab_splits_release_space_immediately_and_animate_only_the_expansion() {
        for original_axis in [SplitAxis::Horizontal, SplitAxis::Vertical] {
            for target_high in [false, true] {
                for split_high in [false, true] {
                    for last_tab in [false, true] {
                        let mut app = App::new();
                        app.add_plugins(crate::layouts::LayoutsPlugin);
                        crate::layouts::tests::layout(app.world_mut());
                        let w = app.world_mut().layouts_mut();
                        let layout = w.active().unwrap();
                        let low = w.destination_area(layout).unwrap();
                        let high = w.split_area(low, original_axis, 5000, [100, 100]).unwrap();
                        let (target, source) = if target_high {
                            (high, low)
                        } else {
                            (low, high)
                        };
                        w.create_panel_in_area(target, "Staying").unwrap();
                        let panel = w.create_panel_in_area(source, "Moving").unwrap();
                        if !last_tab {
                            w.create_panel_in_area(source, "Also staying").unwrap();
                        }
                        let bounds =
                            Rect::from_corners(Vec2::new(50.0, 75.0), Vec2::new(1250.0, 875.0));
                        let before = screen(w.world.get::<Area>(target).unwrap().rect(), bounds);
                        let split_axis = axis(original_axis.index() ^ 1);
                        let destination = w
                            .split_tab(panel, target, split_axis, split_high, [100, 100])
                            .unwrap();
                        let to = screen(w.world.get::<Area>(target).unwrap().rect(), bounds);
                        let incoming =
                            screen(w.world.get::<Area>(destination).unwrap().rect(), bounds);
                        let mut current = expand_rect(before, to, 0.25);
                        let split = split_axis.index();
                        assert_eq!(current.min[split], to.min[split]);
                        assert_eq!(current.max[split], to.max[split]);
                        if last_tab {
                            assert_ne!(
                                current, to,
                                "The surviving area expands into the closed source"
                            );
                        } else {
                            assert_eq!(current, to, "A split without expansion snaps immediately");
                        }
                        for _ in 0..128 {
                            assert!(to.contains(current.min) && to.contains(current.max));
                            assert!(
                                current.max[split] <= incoming.min[split]
                                    || incoming.max[split] <= current.min[split],
                                "The surviving area must never overlap the dropped panel"
                            );
                            let next = expand_rect(current, to, 0.25);
                            assert!(next.contains(current.min) && next.contains(current.max));
                            current = next;
                        }
                        assert_eq!(current, to);
                    }
                }
            }
        }
    }

    #[test]
    fn retargeting_an_expansion_immediately_releases_reclaimed_space() {
        let bounds = Rect::from_corners(Vec2::ZERO, Vec2::splat(1000.0));
        let left = Rect::from_corners(Vec2::ZERO, Vec2::new(500.0, 1000.0));
        let right = Rect::from_corners(Vec2::new(500.0, 0.0), bounds.max);
        assert_eq!(expand_rect(bounds, left, 0.25), left);
        let expanding = expand_rect(left, bounds, 0.25);
        assert_eq!(expanding.max.x, 625.0);
        let reclaimed = expand_rect(expanding, right, 0.25);
        assert_eq!(reclaimed.min.x, right.min.x);
        assert!(reclaimed.max.x > expanding.max.x && reclaimed.max.x < right.max.x);
        assert_eq!(expand_rect(expanding, right, 1.0), right);
        assert_eq!(
            expand_rect(left, right, 0.25),
            right,
            "Pure relocation still snaps"
        );
    }

    #[test]
    fn corner_split_line_follows_the_pointer_but_commits_the_minimum_panel_size() {
        let minimum = [1000, 800];
        for direction in [SplitAxis::Horizontal, SplitAxis::Vertical] {
            for offset in [1, 500, 2500, 4500, 4999] {
                let mut app = App::new();
                app.add_plugins(crate::layouts::LayoutsPlugin);
                crate::layouts::tests::layout(app.world_mut());
                let w = app.world_mut().layouts_mut();
                let layout = w.active().unwrap();
                let root = w.destination_area(layout).unwrap();
                let right = w
                    .split_area(root, SplitAxis::Horizontal, 5000, minimum)
                    .unwrap();
                let source = w
                    .split_area(right, SplitAxis::Vertical, 5000, minimum)
                    .unwrap();
                let snapshot = w.snapshot(layout).unwrap();
                let id = snapshot.id(source).unwrap();
                let rect = snapshot.geometry.area(id).rect;
                let index = direction.index();
                let mut point = [7500, 7500];
                point[index] = rect.min[index] + offset;
                let (edit, planned, line) =
                    corner_target(&snapshot, source, point, Some(direction), minimum).unwrap();
                assert_eq!(line.min[index], point[index]);
                assert_eq!(line.max[index], point[index]);
                assert_eq!(line.min[index ^ 1], rect.min[index ^ 1]);
                assert_eq!(line.max[index ^ 1], rect.max[index ^ 1]);
                let expected = point[index].clamp(
                    rect.min[index] + minimum[index],
                    rect.max[index] - minimum[index],
                );
                assert_eq!(planned.area(id).rect.max[index], expected);
                w.apply_edit(w.active().unwrap(), &edit).unwrap();
                assert_eq!(
                    w.world.get::<Area>(source).unwrap().rect(),
                    planned.area(id).rect
                );
                let actual = w.snapshot(layout).unwrap();
                assert_eq!(actual.geometry.areas.len(), planned.areas.len());
                for area in &actual.geometry.areas {
                    for axis in 0..2 {
                        assert!(area.rect.max[axis] - area.rect.min[axis] >= minimum[axis]);
                    }
                }
            }
        }
    }

    #[test]
    fn corner_hit_targets_grow_inward_from_each_visible_panel_corner() {
        let area = Rect::from_corners(Vec2::new(50.0, 75.0), Vec2::new(450.0, 375.0));
        for (high, anchor, direction) in [
            ([false, false], area.min, Vec2::new(1.0, 1.0)),
            ([true, false], Vec2::new(450.0, 75.0), Vec2::new(-1.0, 1.0)),
            ([false, true], Vec2::new(50.0, 375.0), Vec2::new(1.0, -1.0)),
            ([true, true], area.max, Vec2::new(-1.0, -1.0)),
        ] {
            let small = corner_hit_rect(area, high, 8.0);
            let large = corner_hit_rect(area, high, 32.0);
            assert_eq!(small, Rect::from_corners(anchor, anchor + direction * 8.0));
            assert_eq!(large, Rect::from_corners(anchor, anchor + direction * 32.0));
        }
    }

    #[test]
    fn corner_hit_targets_can_be_disabled_and_cannot_overlap_in_small_areas() {
        let area = Rect::from_corners(Vec2::new(50.0, 75.0), Vec2::new(150.0, 155.0));
        let disabled = corner_hit_rect(area, [false, false], 0.0);
        assert_eq!(disabled.size(), Vec2::ZERO);
        let large = corner_hit_rect(area, [true, true], 200.0);
        assert_eq!(large.max, area.max);
        assert_eq!(large.size(), Vec2::splat(40.0));
    }

    fn projection(
        w: &Layouts,
        layout: Entity,
        bounds: Rect,
        panel_gap: f32,
    ) -> (Vec<Group>, Frontend) {
        let mut state = Frontend {
            bounds,
            ..Default::default()
        };
        let areas = w.world.get::<LayoutAreas>(layout).unwrap().entities();
        let groups = areas
            .iter()
            .map(|&entity| {
                let area = w.world.get::<Area>(entity).unwrap();
                let rect = area.rect();
                let r = screen(rect, bounds);
                let s = surface(r, rect, panel_gap);
                state.area_bounds.insert(entity, r);
                state.tab_bars.insert(
                    entity,
                    Rect::from_corners(s.min, Vec2::new(s.max.x, s.min.y + 30.0)),
                );
                Group {
                    entity,
                    rect,
                    tabs: w
                        .panels_in_area(entity)
                        .iter()
                        .map(|&entity| Tab {
                            entity,
                            title: String::new(),
                            dirty: false,
                            style: PanelTabStyle::default(),
                            icon: None,
                        })
                        .collect(),
                    selected: area.selected_panel(),
                }
            })
            .collect();
        (groups, state)
    }

    #[test]
    fn last_tab_preview_includes_source_gap_and_single_area_border_policy() {
        let mut app = App::new();
        app.add_plugins(crate::layouts::LayoutsPlugin);
        crate::layouts::tests::layout(app.world_mut());
        let w = app.world_mut().layouts_mut();
        let layout = w.active().unwrap();
        let source = w.destination_area(layout).unwrap();
        let target = w
            .split_area(source, SplitAxis::Horizontal, 5000, [100, 100])
            .unwrap();
        let panel = w.create_panel_in_area(source, "Moving").unwrap();
        let bounds = Rect::from_corners(Vec2::new(50.0, 75.0), Vec2::new(1250.0, 875.0));
        let snapshot = w.snapshot(layout).unwrap();
        let panel_gap = 7.2;
        let (groups, state) = projection(w, layout, bounds, panel_gap);
        let target_rect = screen(w.world.get::<Area>(target).unwrap().rect(), bounds);

        let (_, preview, count) = tab_target(
            &groups,
            &snapshot,
            &state,
            Some(panel),
            target_rect.center(),
            bounds,
            [100, 100],
            panel_gap,
        )
        .unwrap();
        assert_eq!(count, 1);
        assert_eq!(
            preview,
            Rect::from_corners(
                bounds.min + Vec2::splat(panel_gap),
                bounds.max - Vec2::splat(panel_gap)
            ),
            "The last source closes; the only area retains its outer inset"
        );

        let mouse = Vec2::new(
            target_rect.min.x + target_rect.width() * 0.1,
            target_rect.center().y,
        );
        let (_, preview, count) = tab_target(
            &groups,
            &snapshot,
            &state,
            Some(panel),
            mouse,
            bounds,
            [100, 100],
            panel_gap,
        )
        .unwrap();
        assert_eq!(count, 2);
        assert_eq!(
            preview,
            Rect::from_corners(Vec2::new(57.2, 82.2), Vec2::new(946.4, 867.8)),
            "The left split expands into the source and uses outer/shared border insets"
        );
    }

    #[test]
    fn tab_outlines_match_committed_surfaces_on_all_edges_center_and_t_junctions() {
        for t_junction in [false, true] {
            for last_tab in [false, true] {
                for relative in [
                    Vec2::new(0.1, 0.5),
                    Vec2::new(0.9, 0.5),
                    Vec2::new(0.5, 0.1),
                    Vec2::new(0.5, 0.9),
                    Vec2::splat(0.5),
                ] {
                    let mut app = App::new();
                    app.add_plugins(crate::layouts::LayoutsPlugin);
                    crate::layouts::tests::layout(app.world_mut());
                    let w = app.world_mut().layouts_mut();
                    let layout = w.active().unwrap();
                    let left = w.destination_area(layout).unwrap();
                    let target = w
                        .split_area(left, SplitAxis::Horizontal, 5000, [100, 100])
                        .unwrap();
                    let source = if t_junction {
                        w.split_area(left, SplitAxis::Vertical, 5000, [100, 100])
                            .unwrap()
                    } else {
                        left
                    };
                    let panel = w.create_panel_in_area(source, "Moving").unwrap();
                    if !last_tab {
                        w.create_panel_in_area(source, "Staying").unwrap();
                    }
                    let target_panel = w.create_panel_in_area(target, "Target").unwrap();
                    let bounds =
                        Rect::from_corners(Vec2::new(50.0, 75.0), Vec2::new(1250.0, 875.0));
                    let snapshot = w.snapshot(layout).unwrap();
                    let panel_gap = 16.0;
                    let (groups, state) = projection(w, layout, bounds, panel_gap);
                    let r = screen(w.world.get::<Area>(target).unwrap().rect(), bounds);
                    let mouse = r.min + relative * r.size();
                    let (edit, preview, count) = tab_target(
                        &groups,
                        &snapshot,
                        &state,
                        Some(panel),
                        mouse,
                        bounds,
                        [100, 100],
                        panel_gap,
                    )
                    .unwrap();
                    w.apply_edit(w.active().unwrap(), &edit).unwrap();
                    let destination = w.world.get::<InArea>(panel).unwrap().0;
                    let rect = w.world.get::<Area>(destination).unwrap().rect();
                    let actual_count = w.world.get::<LayoutAreas>(layout).unwrap().entities().len();
                    assert_eq!(count, actual_count);
                    assert_eq!(
                        preview,
                        surface(screen(rect, bounds), rect, panel_gap),
                        "T={t_junction}, last={last_tab}, mouse={relative:?}"
                    );
                    assert!(w.world.get::<PanelInstance>(target_panel).is_some());
                }
            }
        }
    }
    #[test]
    fn configured_gaps_can_inset_a_single_area() {
        let old = Rect::from_corners(Vec2::ZERO, Vec2::splat(1000.0));
        assert_eq!(
            surface(
                old,
                geometry::Rect {
                    min: [0, 0],
                    max: [LAYOUT_EXTENT; 2]
                },
                7.2
            ),
            Rect::from_corners(Vec2::splat(7.2), Vec2::splat(992.8))
        );
    }

    #[test]
    fn panel_gap_is_equal_between_panels_and_at_shell_edges() {
        let bounds = Rect::from_corners(Vec2::new(50.0, 75.0), Vec2::new(1250.0, 875.0));
        let left = geometry::Rect {
            min: [0, 0],
            max: [LAYOUT_EXTENT / 2, LAYOUT_EXTENT],
        };
        let right = geometry::Rect {
            min: [LAYOUT_EXTENT / 2, 0],
            max: [LAYOUT_EXTENT; 2],
        };
        for gap in [0.0, 7.2, 16.0] {
            let a = surface(screen(left, bounds), left, gap);
            let b = surface(screen(right, bounds), right, gap);
            for actual in [
                a.min.x - bounds.min.x,
                bounds.max.x - b.max.x,
                a.min.y - bounds.min.y,
                bounds.max.y - a.max.y,
                b.min.x - a.max.x,
            ] {
                assert!((actual - gap).abs() < 0.001);
            }
        }
        let collapsed = surface(screen(left, bounds), left, 1000.0);
        assert_eq!(collapsed.size(), Vec2::ZERO);
        assert!(bounds.contains(collapsed.min));
    }
}
