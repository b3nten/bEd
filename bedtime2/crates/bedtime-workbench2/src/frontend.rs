//! Blender-style chrome is a projection of ECS areas. Pointer gestures and
//! interpolation are local to this drawing system; only completed edits commit.
use crate::{
    geometry,
    layout::{LayoutSnapshot, plan_tab_geometry},
    *,
};
use bedtime_ui2::{
    UiFrame, UiSurface, UiSystems,
    egui::{self, CursorIcon, Key, PointerButton},
    register_view,
};
use bevy::{ecs::system::SystemId, prelude::*};
use std::collections::HashMap;

const PANEL_BORDER_WIDTH: f32 = 1.0;

#[derive(Component)]
#[require(WorkbenchPresentation)]
pub struct WorkbenchHost {
    /// Hide the tiled content for a scene underneath, retaining menu and status.
    pub panels_visible: bool,
    /// Outer shell bounds in egui points. Defaults to the parent's available UI.
    pub bounds: Option<Rect>,
    pub minimum_area_size: Vec2,
    /// Explicit menu root; native presentation automatically omits its egui row.
    pub menu: Option<Entity>,
    status_bar: Entity,
}

impl WorkbenchHost {
    /// Register plugin status contributions on this host-owned bar.
    pub fn status_bar(&self) -> Entity {
        self.status_bar
    }
}

/// Observed shell allocations, in egui points. The host alone writes these.
#[derive(Component, Default, Clone, Copy, Debug)]
pub struct WorkbenchPresentation {
    pub bounds: Rect,
    pub titlebar: Rect,
    pub menu: Rect,
    pub panels: Rect,
    pub status_bar: Rect,
}

#[derive(Component)]
struct PanelRenderer;

/// Draw the workbench on this window. The host owns native titlebar reservation,
/// menu, panel layout, background and status bar; applications supply contributions.
pub fn attach_surface(world: &mut World, surface: Entity) -> Entity {
    assert!(world.get::<UiSurface>(surface).is_some());
    assert!(
        world.get::<bevy::window::Window>(surface).is_some(),
        "Workbench surfaces must be windows"
    );
    assert!(
        world.query::<&WorkbenchHost>().iter(world).next().is_none(),
        "Only one workbench host is supported"
    );
    let host = register_view(world, surface, draw);
    let status_bar = bedtime_ui2::create_status_bar(world, host);
    world
        .entity_mut(surface)
        .insert(bedtime_ui2::UiBackground::Transparent);
    world.entity_mut(host).insert(WorkbenchHost {
        panels_visible: true,
        bounds: None,
        minimum_area_size: Vec2::new(100.0, 80.0),
        menu: None,
        status_bar,
    });
    host
}

pub fn register_panel<M>(
    world: &mut World,
    layout: Entity,
    title: impl Into<String>,
    draw: impl IntoSystem<UiFrame<'static>, (), M> + 'static,
) -> Result<Entity, String> {
    let area = Workbench::new(world).destination_area(layout)?;
    register_panel_in_area(world, area, title, draw)
}

pub fn register_panel_in_area<M>(
    world: &mut World,
    area: Entity,
    title: impl Into<String>,
    draw: impl IntoSystem<UiFrame<'static>, (), M> + 'static,
) -> Result<Entity, String> {
    world.get::<Area>(area).ok_or("Area is unavailable")?;
    let system = world.register_system(draw);
    let panel = system.entity();
    Workbench::new(world).attach_panel(panel, area, title.into());
    world.entity_mut(panel).insert(PanelRenderer);
    Ok(panel)
}

pub(crate) fn configure(app: &mut App) {
    app.add_systems(PreUpdate, reset.before(UiSystems::Draw));
}

fn reset(mut panels: Query<&mut PanelPresentation>, mut areas: Query<&mut AreaPresentation>) {
    for mut p in &mut panels {
        p.visible = false;
        p.focused = false;
        p.tab_rect = Rect::default();
        p.close_rect = Rect::default();
    }
    for mut a in &mut areas {
        a.visible = false;
    }
}

#[derive(Default)]
struct Frontend {
    stamp: Option<LayoutStamp>,
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
    progress: f32,
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
    preview: bool,
    icon: Option<bedtime_ui2::IconSource>,
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

fn corner_hit_rect(area: Rect, high: [bool; 2], size: f32) -> Rect {
    let size = size.min(area.width() * 0.5).min(area.height() * 0.5);
    let anchor = Vec2::new(
        if high[0] { area.max.x } else { area.min.x },
        if high[1] { area.max.y } else { area.min.y },
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

fn er(r: Rect) -> egui::Rect {
    egui::Rect::from_min_max(egui::pos2(r.min.x, r.min.y), egui::pos2(r.max.x, r.max.y))
}
fn hit(ui: &egui::Ui, name: &str, rect: Rect) -> (bool, bool) {
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return (false, false);
    }
    let response = ui.interact(er(rect), ui.id().with(name), egui::Sense::click_and_drag());
    (
        response.hovered(),
        response.is_pointer_button_down_on()
            && ui.input(|i| i.pointer.button_pressed(PointerButton::Primary)),
    )
}
#[derive(Clone)]
pub struct PanelTypePayload(pub Entity);
fn draw(frame: UiFrame, world: &mut World, mut local: Local<Frontend>) {
    let state = &mut *local;
    let host = world
        .get::<WorkbenchHost>(frame.view)
        .expect("Workbench host");
    let (panels_visible, menu, status_bar, minimum_area_size) = (
        host.panels_visible,
        host.menu,
        host.status_bar,
        host.minimum_area_size,
    );
    let panel_settings = frame.panel_settings;
    let outer = host
        .bounds
        .map(er)
        .unwrap_or_else(|| frame.ui.available_rect_before_wrap())
        .intersect(frame.ui.clip_rect());
    if !outer.is_positive() {
        *world.get_mut::<WorkbenchPresentation>(frame.view).unwrap() = default();
        return;
    }
    #[cfg(feature = "native")]
    {
        let background = WindowBackground::new(frame.theme.palette.background);
        if world.get::<WindowBackground>(frame.surface) != Some(&background) {
            world.entity_mut(frame.surface).insert(background);
        }
    }
    // One root owns all shell geometry. The native measurement is in logical
    // window coordinates; every allocation below uses egui points.
    #[cfg(feature = "native")]
    let native_top = world
        .get::<WindowTitlebarInset>(frame.surface)
        .map_or(0.0, |inset| inset.0 / frame.ui.ctx().zoom_factor());
    #[cfg(not(feature = "native"))]
    let native_top = 0.0_f32;
    let inset = (native_top - outer.top()).clamp(0.0, outer.height());
    let mut root = frame.ui.new_child(
        egui::UiBuilder::new()
            .id_salt("workbench-shell")
            .max_rect(outer)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    frame.ui.advance_cursor_after_rect(outer);
    root.set_clip_rect(outer);
    let background = bedtime_ui2::theme::color(frame.theme.palette.background);
    if panels_visible {
        root.painter().rect_filled(outer, 0.0, background);
    }
    // Full-window scene mode paints only the shell bands over the scene.
    let band = egui::Frame::NONE.fill(if panels_visible {
        egui::Color32::TRANSPARENT
    } else {
        background
    });
    let titlebar = egui::Panel::top(root.id().with("native-titlebar"))
        .exact_size(inset)
        .show_separator_line(false)
        .frame(band)
        .show(&mut root, |_| {})
        .response
        .rect;
    let menu_rect = if let Some(menu) = menu {
        egui::Panel::top(root.id().with("menu"))
            .min_size(0.0)
            .show_separator_line(false)
            .frame(band)
            .show(&mut root, |ui| {
                if let Err(error) =
                    crate::ui::draw_menu_bar(&mut frame.for_view(frame.view, ui), world, menu)
                {
                    world.write_message(bedtime_ui2::UiError(error));
                }
            })
            .response
            .rect
    } else {
        egui::Rect::from_min_size(root.available_rect_before_wrap().min, egui::Vec2::ZERO)
    };
    let status_height = bedtime_ui2::status_bar_height(&root).min(root.available_height().max(0.0));
    let status_rect = egui::Panel::bottom(root.id().with("status"))
        .exact_size(status_height)
        .show_separator_line(false)
        .frame(band)
        .show(&mut root, |ui| {
            bedtime_ui2::draw_status_bar(&mut frame.for_view(status_bar, ui), world, status_bar);
        })
        .response
        .rect;
    let content = root.available_rect_before_wrap();
    let observed = |r: egui::Rect| {
        Rect::from_corners(Vec2::new(r.min.x, r.min.y), Vec2::new(r.max.x, r.max.y))
    };
    *world.get_mut::<WorkbenchPresentation>(frame.view).unwrap() = WorkbenchPresentation {
        bounds: observed(outer),
        titlebar: observed(titlebar),
        menu: observed(menu_rect),
        panels: observed(content),
        status_bar: observed(status_rect),
    };
    if !panels_visible || !content.is_positive() {
        state.gesture = None;
        state.preview = None;
        state.stamp = None;
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
    let stamp = Workbench::new(world).stamp();
    let layout = stamp.layout;
    let snapshot = Workbench::new(world).snapshot(layout).unwrap();
    if state.stamp != Some(stamp) || state.bounds != bounds {
        state.gesture = None;
        state.preview = None;
        if state
            .stamp
            .is_none_or(|old| old.layout != layout || old.activation != stamp.activation)
        {
            state.area_bounds.clear();
            state.content_fades.clear();
        }
    }
    state.stamp = Some(stamp);
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
                tabs: panels_in_area(world, e)
                    .iter()
                    .map(|&entity| {
                        let p = world.get::<Panel>(entity).expect("Owned tab");
                        Tab {
                            entity,
                            title: p.title.clone(),
                            dirty: p.dirty,
                            preview: p.preview,
                            icon: world.get::<bedtime_ui2::IconSource>(entity).cloned(),
                        }
                    })
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
    let mut corner_targets = Vec::new();
    let mut geometry = snapshot.geometry.clone();
    let was_interacting = state.gesture.is_some();
    let cancel = ui.input(|i| i.key_pressed(Key::Escape))
        || ui.input(|i| i.pointer.button_pressed(PointerButton::Secondary))
        || ui.input(|i| i.pointer.latest_pos().is_none())
        || world
            .get::<bevy::window::Window>(frame.surface)
            .is_some_and(|w| !w.focused);
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
                        let source = snapshot.geometry.area(snapshot.id(area).unwrap()).rect;
                        let s = surface(screen(source, bounds), source, panel_settings.panel_gap);
                        let along = if r.width() == 0.0 { 1 } else { 0 };
                        // A two-point stroke stays inside the visible surface,
                        // including its gap and window-edge insets.
                        r.min[along] = s.min[along] + 1.0;
                        r.max[along] = s.max[along] - 1.0;
                        preview_clip = Some(er(s).shrink(1.0));
                        r
                    } else {
                        surface(r, rect, panel_settings.panel_gap)
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
                        panel_settings.panel_gap,
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
                if from.size() == target.size() {
                    target
                } else {
                    blend_rect(from, target, blend)
                }
            } else {
                target
            };
            moving |= rect != target;
            (
                entity,
                a.rect,
                rect,
                surface(rect, a.rect, panel_settings.panel_gap),
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
    for &(entity, _, r, _) in &visual {
        // Corner ownership precedes border ownership at junctions.
        for (corner, high) in [[false, false], [true, false], [false, true], [true, true]]
            .into_iter()
            .enumerate()
        {
            let zone = corner_hit_rect(r, high, panel_settings.hit_size);
            if zone.width() <= 0.0 || zone.height() <= 0.0 {
                continue;
            }
            let id = frame.ui.id().with(("corner", entity, corner));
            corner_targets.push((id, zone));
            // Interaction uses the previous pass's widgets. Register these
            // handles after panel content below, so they have priority over it.
            let response = frame.ui.ctx().read_response(id);
            if panel_settings.debug {
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
            if !was_interacting && !cancel && state.gesture.is_none() && zone.contains(mouse) {
                if response.is_some_and(|response| response.is_pointer_button_down_on())
                    && frame
                        .ui
                        .input(|i| i.pointer.button_pressed(PointerButton::Primary))
                {
                    state.gesture = Some(Gesture::Corner {
                        area: entity,
                        start: mouse,
                        axis: None,
                    });
                    edits.push(LayoutEdit::FocusArea(entity));
                }
            }
        }
    }
    for &(entity, model, r, _) in &visual {
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
    let types: Vec<_> = world
        .query::<(Entity, &PanelType)>()
        .iter(world)
        .map(|(e, t)| {
            (
                e,
                t.label.clone(),
                world.get::<ActionEnabled>(e).is_none_or(|a| a.0),
            )
        })
        .collect();
    for group in &groups {
        let (_, _, r, s) = *visual
            .iter()
            .find(|(e, _, _, _)| *e == group.entity)
            .unwrap();
        let bar_height = frame.theme.metrics.font_size + 15.0;
        let bar = Rect::from_corners(s.min, Vec2::new(s.max.x, s.min.y + bar_height));
        let content = Rect::from_corners(Vec2::new(s.min.x, bar.max.y + 1.0), s.max);
        state.tab_bars.insert(group.entity, bar);
        *world.get_mut::<AreaPresentation>(group.entity).unwrap() = AreaPresentation {
            visible: true,
            rect: r,
            surface: s,
            tab_bar: bar,
            content,
        };
        let painter = frame.ui.painter().clone();
        painter.line_segment(
            [
                egui::pos2(s.min.x + 1.0, bar.max.y),
                egui::pos2(s.max.x - 1.0, bar.max.y),
            ],
            egui::Stroke::new(1.0, bedtime_ui2::theme::color(p.border)),
        );
        let tabs_min = bar.min;
        let font = egui::FontId::proportional(frame.theme.metrics.font_size);
        let labels: Vec<_> = group
            .tabs
            .iter()
            .map(|t| {
                let width = painter
                    .layout_no_wrap(t.title.clone(), font.clone(), egui::Color32::WHITE)
                    .size()
                    .x;
                (
                    t,
                    (width
                        + 37.0
                        + if t.dirty { 8.0 } else { 0.0 }
                        + if t.icon.is_some() {
                            frame.theme.metrics.font_size + 5.0
                        } else {
                            0.0
                        })
                    .clamp(70.0, 210.0),
                )
            })
            .collect();
        let total: f32 = labels.iter().map(|(_, w)| *w).sum();
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
            for (tab, width) in &labels {
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
        for (tab, width) in labels {
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
            let (close_hover, close_pressed) =
                hit(frame.ui, &format!("close-{}", tab.entity.to_bits()), close);
            let zone = Rect::from_corners(
                clipped.min,
                Vec2::new(close.min.x.min(clipped.max.x), clipped.max.y),
            );
            let response = frame.ui.interact(
                er(zone),
                frame.ui.id().with(("tab", tab.entity)),
                egui::Sense::click_and_drag(),
            );
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
                        ..egui::CornerRadius::from(panel_settings.rounding)
                    },
                    bedtime_ui2::theme::color(tint),
                );
            }
            let emphasis = frame.animate_bool_with_duration(
                response.id.with("text-color"),
                selected || hovered || close_hover,
                hover_seconds,
            );
            let text =
                bedtime_ui2::theme::color(bedtime_ui2::theme::mix(p.muted, p.text, emphasis));
            let text_x = position.min.x
                + 8.0
                + if tab.icon.is_some() {
                    frame.theme.metrics.font_size + 5.0
                } else {
                    0.0
                };
            tabs_painter
                .with_clip_rect(er(Rect::from_corners(
                    Vec2::new(position.min.x + 8.0, bar.min.y),
                    Vec2::new(close.min.x - 3.0, bar.max.y),
                )))
                .text(
                    egui::pos2(text_x, bar.center().y),
                    egui::Align2::LEFT_CENTER,
                    &tab.title,
                    font.clone(),
                    text,
                );
            if let Some(icon) = &tab.icon {
                // icon_at paints through the Ui, so use the tab painter's clip.
                let clip = frame.ui.clip_rect();
                frame.ui.set_clip_rect(tabs_painter.clip_rect());
                frame.icon_at(
                    icon,
                    [
                        position.min.x + 8.0,
                        bar.center().y - frame.theme.metrics.font_size * 0.5,
                    ],
                    frame.theme.metrics.font_size,
                );
                frame.ui.set_clip_rect(clip);
            }
            if tab.preview {
                tabs_painter.line_segment(
                    [
                        egui::pos2(position.min.x + 8.0, bar.max.y - 4.0),
                        egui::pos2(close.min.x - 3.0, bar.max.y - 4.0),
                    ],
                    egui::Stroke::new(1.0, text),
                );
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
            if emphasis > 0.0 {
                let c = egui::pos2(close.center().x, bar.center().y);
                let mut fill = p.elevated;
                fill[3] *= close_tint;
                tabs_painter.circle_filled(c, 8.0, bedtime_ui2::theme::color(fill));
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
            if close_pressed
                || (clipped.contains(mouse)
                    && frame
                        .ui
                        .input(|i| i.pointer.button_released(PointerButton::Middle)))
            {
                edits.push(LayoutEdit::ClosePanels(vec![tab.entity]));
            }
            if pressed && state.gesture.is_none() && !was_interacting {
                edits.push(LayoutEdit::FocusPanel(tab.entity));
                if response.double_clicked() {
                    edits.push(LayoutEdit::KeepPanel(tab.entity));
                }
                state.gesture = Some(Gesture::Tab {
                    panel: tab.entity,
                    start: mouse,
                    dragging: false,
                });
            }
            if !state.suppress_context {
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
        if group.tabs.is_empty() {
            let response = frame.ui.interact(
                er(content),
                frame.ui.id().with(("empty", group.entity)),
                egui::Sense::click(),
            );
            if response.clicked() {
                edits.push(LayoutEdit::FocusArea(group.entity));
            }
            if !state.suppress_context {
                response.context_menu(|ui| picker(ui, &types, group.entity, &mut edits));
            }
            painter.text(
                egui::pos2(content.min.x + 14.0, content.min.y + 14.0),
                egui::Align2::LEFT_TOP,
                "Right-click to open a panel",
                font,
                bedtime_ui2::theme::color(p.muted),
            );
        }
    }
    for group in &groups {
        if let Some(panel) = group.selected {
            let content = world.get::<AreaPresentation>(group.entity).unwrap().content;
            if content.width() < 1.0 || content.height() < 1.0 {
                continue;
            }
            let fade = state.content_fades.entry(panel).or_insert(ContentFade {
                area: group.entity,
                placement: group.rect,
                progress: 0.0,
            });
            // Compare committed placement, not the interpolated screen bounds.
            if fade.area != group.entity || fade.placement != group.rect {
                fade.progress = 0.0;
                fade.area = group.entity;
                fade.placement = group.rect;
            }
            fade.progress = if frame.motion.enabled {
                (fade.progress + dt / frame.motion.seconds).min(1.0)
            } else {
                1.0
            };
            let opacity = 1.0 - (1.0 - fade.progress).powi(3);
            moving |= fade.progress < 1.0;
            let focus = focused == group.entity;
            *world.get_mut::<PanelPresentation>(panel).unwrap() = PanelPresentation {
                visible: true,
                focused: focus,
                rect: content,
                tab_rect: world.get::<PanelPresentation>(panel).unwrap().tab_rect,
                close_rect: world.get::<PanelPresentation>(panel).unwrap().close_rect,
            };
            if content.contains(mouse)
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
                    if world.get::<PanelRenderer>(panel).is_some() {
                        if let Err(error) = world.run_system_with(
                            SystemId::<UiFrame<'static>>::from_entity(panel),
                            content_frame,
                        ) {
                            world.write_message(bedtime_ui2::UiError(error.to_string()));
                        }
                    } else {
                        content_frame.ui.weak("This panel has no content renderer.");
                    }
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
        let focus = frame.animate_bool(
            frame.ui.id().with(("area-focus", entity.to_bits())),
            entity == focused,
        );
        let mut border = p.border;
        for v in &mut border[..3] {
            *v += (1.0 - *v) * 0.2 * focus;
        }
        painter.rect_stroke(
            er(s),
            panel_settings.rounding,
            egui::Stroke::new(PANEL_BORDER_WIDTH, bedtime_ui2::theme::color(border)),
            egui::StrokeKind::Inside,
        );
    }
    // Content has first opportunity to take a typed egui payload. The shell
    // receives only the remaining payload, preserving widget target priority.
    if let Some(payload) = egui::DragAndDrop::payload::<PanelTypePayload>(frame.ui.ctx()) {
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
                panel_settings.panel_gap,
            ) {
                preview = Some(rect);
                if frame
                    .ui
                    .input(|i| i.pointer.button_released(PointerButton::Primary))
                {
                    if let LayoutEdit::DropPanel { kind, .. } = &mut edit {
                        *kind = payload.0;
                    }
                    egui::DragAndDrop::take_payload::<PanelTypePayload>(frame.ui.ctx());
                    edits.push(edit);
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
        let current = blend_rect(state.preview.unwrap_or(target), target, blend);
        moving |= current != target;
        state.preview = Some(current);
        let border = std::array::from_fn(|i| p.border[i] + (p.text[i] - p.border[i]) * 0.75);
        let fill = [border[0], border[1], border[2], border[3] * 0.14];
        if current.width().abs() < 0.5 || current.height().abs() < 0.5 {
            painter.line_segment(
                [er(current).min, er(current).max],
                egui::Stroke::new(2.0, bedtime_ui2::theme::color(border)),
            );
        } else {
            let rounding = panel_settings.rounding;
            painter.rect_filled(er(current), rounding, bedtime_ui2::theme::color(fill));
            painter.rect_stroke(
                er(current),
                rounding,
                egui::Stroke::new(2.0, bedtime_ui2::theme::color(border)),
                egui::StrokeKind::Inside,
            );
        }
    } else {
        state.preview = None;
    }
    if was_interacting || state.gesture.is_some() {
        frame.claim_pointer();
        if cancel {
            frame.claim_key(KeyCode::Escape);
        }
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
        if let LayoutEdit::SplitArea { area, .. } = &edit {
            // A corner split reveals both halves in place. Forget the source's
            // old bounds so it snaps alongside the newly created area.
            state.area_bounds.remove(area);
        }
        world
            .resource_mut::<WorkbenchRequests>()
            .submit(WorkbenchRequest::Edit { stamp, edit });
    }
}
fn picker(
    ui: &mut egui::Ui,
    types: &[(Entity, String, bool)],
    area: Entity,
    edits: &mut Vec<LayoutEdit>,
) {
    for (kind, label, enabled) in types {
        if ui.add_enabled(*enabled, egui::Button::new(label)).clicked() {
            edits.push(LayoutEdit::OpenPanel { area, kind: *kind });
            ui.close();
        }
    }
    if types.is_empty() {
        ui.weak("No panel types registered");
    }
    ui.separator();
    if ui.button("Close area").clicked() {
        edits.push(LayoutEdit::CloseArea(area));
        ui.close();
    }
}
/// Call with a drag-sensitive toolbar response. Payload identity stays typed.
pub fn panel_drag_source(response: &egui::Response, kind: Entity, _label: &str) {
    response.dnd_set_drag_payload(PanelTypePayload(kind));
}
fn tab_target(
    groups: &[Group],
    snapshot: &LayoutSnapshot,
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
                    kind: Entity::PLACEHOLDER,
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
                kind: Entity::PLACEHOLDER,
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
                kind: Entity::PLACEHOLDER,
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
    snapshot: &LayoutSnapshot,
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
        line.min[index] = at;
        line.max[index] = at;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corner_hit_targets_grow_inward_from_each_outside_corner() {
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

    #[test]
    fn shell_owns_its_status_contributions_and_preserves_the_application_window() {
        let mut app = App::new();
        app.add_plugins(WorkbenchCorePlugin);
        let world = app.world_mut();
        let window = world
            .spawn((bevy::window::Window::default(), UiSurface))
            .id();
        let host = attach_surface(world, window);
        let bar = world.get::<WorkbenchHost>(host).unwrap().status_bar();
        let item = bedtime_ui2::register_status_item(
            world,
            bar,
            bedtime_ui2::StatusItem::new(
                bedtime_ui2::StatusSide::Left,
                bedtime_ui2::BuiltinIcon::Folder,
                "Owned",
                60.0,
            ),
            |_: UiFrame| {},
        );
        let tooltip = bedtime_ui2::register_status_tooltip(world, item, |_: UiFrame| {});
        assert_eq!(
            world.get::<bedtime_ui2::UiBackground>(window),
            Some(&bedtime_ui2::UiBackground::Transparent)
        );
        world.despawn(host);
        for entity in [bar, item, tooltip] {
            assert!(world.get_entity(entity).is_err());
        }
        assert!(world.get::<bevy::window::Window>(window).is_some());
    }

    fn projection(
        w: &Workbench,
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
                    tabs: panels_in_area(w.world, entity)
                        .iter()
                        .map(|&entity| Tab {
                            entity,
                            title: String::new(),
                            dirty: false,
                            preview: false,
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
        app.add_plugins(WorkbenchCorePlugin);
        let mut w = Workbench::new(app.world_mut());
        let layout = w.active_layout();
        let source = w.destination_area(layout).unwrap();
        let target = w
            .split_area(source, SplitAxis::Horizontal, 5000, [100, 100])
            .unwrap();
        let panel = w.create_panel_in_area(source, "Moving").unwrap();
        let bounds = Rect::from_corners(Vec2::new(50.0, 75.0), Vec2::new(1250.0, 875.0));
        let snapshot = w.snapshot(layout).unwrap();
        let panel_gap = 7.2;
        let (groups, state) = projection(&w, layout, bounds, panel_gap);
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
                    app.add_plugins(WorkbenchCorePlugin);
                    let mut w = Workbench::new(app.world_mut());
                    let layout = w.active_layout();
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
                    let (groups, state) = projection(&w, layout, bounds, panel_gap);
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
                    w.edit(w.stamp(), &edit).unwrap();
                    let destination = w.world.get::<InArea>(panel).unwrap().0;
                    let rect = w.world.get::<Area>(destination).unwrap().rect();
                    let actual_count = w.world.get::<LayoutAreas>(layout).unwrap().entities().len();
                    assert_eq!(count, actual_count);
                    assert_eq!(
                        preview,
                        surface(screen(rect, bounds), rect, panel_gap),
                        "T={t_junction}, last={last_tab}, mouse={relative:?}"
                    );
                    assert!(w.world.get::<Panel>(target_panel).is_some());
                }
            }
        }
    }
    #[test]
    fn a_single_panel_keeps_the_shell_inset() {
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
