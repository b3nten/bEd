//! Blender-style chrome is a projection of ECS areas. Pointer gestures and
//! interpolation are local to this drawing system; only completed edits commit.
use crate::{
    geometry,
    layout::{LayoutSnapshot, plan_tab_geometry},
    *,
};
use bedtime_ui2::{
    UiFrame, UiSurface, UiSystems,
    imgui::{self, *},
    register_view,
};
use bevy::{ecs::system::SystemId, prelude::*};
use std::collections::HashMap;

const INSET: f32 = 2.5;
const CORNER: f32 = 20.0;
const CONTENT_INSET: f32 = 5.0;
pub const PANEL_TYPE_PAYLOAD: &str = "BEDTIME_PANEL_TYPE";

#[derive(Component)]
pub struct WorkbenchHost {
    pub visible: bool,
    pub top_inset: f32,
    pub bottom_inset: f32,
    pub bounds: Option<Rect>,
    pub minimum_area_size: Vec2,
    pub show_add_button: bool,
}
#[derive(Component)]
struct PanelRenderer;

pub fn attach_surface(world: &mut World, surface: Entity) -> Entity {
    assert!(world.get::<UiSurface>(surface).is_some());
    let host = register_view(world, surface, draw);
    world.entity_mut(host).insert(WorkbenchHost {
        visible: true,
        top_inset: 0.0,
        bottom_inset: 0.0,
        bounds: None,
        minimum_area_size: Vec2::new(100.0, 80.0),
        show_add_button: true,
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
    app.configure_sets(PostUpdate, CommandSystems::Ingress.after(UiSystems::Draw))
        .add_systems(PostUpdate, reset.before(UiSystems::Draw));
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
    tab_bounds: HashMap<Entity, Rect>,
    tab_bars: HashMap<Entity, Rect>,
    tab_positions: HashMap<Entity, Rect>,
    tab_scroll: HashMap<Entity, TabScroll>,
    appearances: HashMap<Entity, (usize, f32)>,
    preview: Option<Rect>,
    suppress_context: bool,
    payload_cancelled: bool,
}

#[derive(Default)]
struct TabScroll {
    offset: f32,
    selected: Option<Entity>,
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

fn surface(r: Rect, model: geometry::Rect, count: usize) -> Rect {
    if count == 1 {
        return r;
    }
    let lo = Vec2::new(
        if model.min[0] == 0 { 5.0 } else { INSET },
        if model.min[1] == 0 { 5.0 } else { INSET },
    );
    let hi = Vec2::new(
        if model.max[0] == LAYOUT_EXTENT {
            5.0
        } else {
            INSET
        },
        if model.max[1] == LAYOUT_EXTENT {
            5.0
        } else {
            INSET
        },
    );
    Rect::from_corners(r.min + lo, r.max - hi)
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

/// Native button ownership flattens content children, while retaining ImGui's
/// popup blocking and pointer ownership. All raw pointers stay in this scope.
fn hit(ui: &Ui, name: &str, rect: Rect) -> (bool, bool) {
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return (false, false);
    }
    ui.with_bound_context(|| unsafe {
        let bounds = imgui::sys::ImRect_c {
            Min: rect.min.to_array().into(),
            Max: rect.max.to_array().into(),
        };
        let id = ui.get_id(name).raw();
        let mut hovered = false;
        let mut held = false;
        imgui::sys::igItemAdd(
            bounds,
            id,
            std::ptr::null(),
            imgui::sys::ImGuiItemFlags_NoNav,
        );
        let pressed = imgui::sys::igButtonBehavior(
            bounds,
            id,
            &mut hovered,
            &mut held,
            imgui::sys::ImGuiButtonFlags_FlattenChildren
                | imgui::sys::ImGuiButtonFlags_PressedOnClick
                | imgui::sys::ImGuiButtonFlags_NoNavFocus
                | imgui::sys::ImGuiButtonFlags_NoFocus,
        );
        (hovered, pressed)
    })
}

fn draw(frame: UiFrame, world: &mut World, mut local: Local<Frontend>) {
    let state = &mut *local;
    let host = world
        .get::<WorkbenchHost>(frame.view)
        .expect("Workbench host");
    if !host.visible {
        state.gesture = None;
        state.preview = None;
        state.stamp = None;
        return;
    }
    let display = Vec2::from_array(frame.ui.io().display_size());
    let bounds = host.bounds.unwrap_or(Rect::from_corners(
        Vec2::new(0.0, host.top_inset),
        Vec2::new(display.x, display.y - host.bottom_inset),
    ));
    if bounds.width() <= 0.0 || bounds.height() <= 0.0 {
        return;
    }
    let minimum = [
        (host.minimum_area_size.x * LAYOUT_EXTENT as f32 / bounds.width())
            .ceil()
            .clamp(1.0, LAYOUT_EXTENT as f32) as i32,
        (host.minimum_area_size.y * LAYOUT_EXTENT as f32 / bounds.height())
            .ceil()
            .clamp(1.0, LAYOUT_EXTENT as f32) as i32,
    ];
    let show_add = host.show_add_button;
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
            state.tab_positions.clear();
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
    let ui = frame.ui;
    let mouse = Vec2::from_array(ui.io().mouse_pos());
    let point = logical(mouse, bounds);
    let dt = world
        .get_resource::<Time<Real>>()
        .map_or(ui.io().delta_time(), |t| t.delta_secs())
        .clamp(0.0, 0.1);
    let blend = if frame.motion.enabled {
        1.0 - (-dt / 0.04).exp()
    } else {
        1.0
    };
    let mut edits = Vec::new();
    let mut preview = None;
    let mut preview_count = groups.len();
    let mut geometry = snapshot.geometry.clone();
    let was_interacting = state.gesture.is_some();
    let cancel = ui.is_key_pressed(Key::Escape)
        || ui.is_mouse_clicked(imgui::MouseButton::Right)
        || !ui.is_mouse_pos_valid_at(mouse.to_array())
        || world
            .get::<bevy::window::Window>(frame.surface)
            .is_some_and(|w| !w.focused);
    if cancel && state.gesture.is_some() {
        state.gesture = None;
        state.preview = None;
        state.suppress_context = true;
    }
    if let Some(gesture) = state.gesture.take() {
        let held = ui.is_mouse_down(imgui::MouseButton::Left);
        let released = ui.is_mouse_released(imgui::MouseButton::Left);
        match gesture {
            Gesture::Border {
                axis,
                at,
                span,
                pointer_offset,
            } => {
                let to = point[axis.index()] + pointer_offset;
                geometry.move_border(axis.index(), at, span, to, minimum[axis.index()]);
                ui.set_mouse_cursor(Some(if axis == SplitAxis::Horizontal {
                    MouseCursor::ResizeEW
                } else {
                    MouseCursor::ResizeNS
                }));
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
                if let Some((edit, next, rect)) = proposed {
                    preview_count = next.areas.len();
                    let r = screen(rect, bounds);
                    preview = Some(if r.width() == 0.0 || r.height() == 0.0 {
                        r
                    } else {
                        surface(r, rect, preview_count)
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
                ui.set_mouse_cursor(Some(MouseCursor::ResizeAll));
            }
            Gesture::Tab {
                panel,
                start,
                mut dragging,
            } => {
                dragging |= (mouse - start).abs().max_element() >= 7.0;
                if dragging {
                    if let Some((edit, rect, count)) = tab_target(
                        &groups,
                        &snapshot,
                        state,
                        Some(panel),
                        mouse,
                        bounds,
                        minimum,
                    ) {
                        preview = Some(rect);
                        preview_count = count;
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
    let visual_count = geometry.areas.len();
    let growing = groups.len() <= state.area_bounds.len()
        && groups.iter().all(|g| {
            state.area_bounds.get(&g.entity).is_none_or(|old| {
                let target = screen(g.rect, bounds);
                target.min.x <= old.min.x
                    && target.min.y <= old.min.y
                    && target.max.x >= old.max.x
                    && target.max.y >= old.max.y
            })
        });
    let mut moving = false;
    let visual: Vec<_> = geometry
        .areas
        .iter()
        .map(|a| {
            let entity = snapshot.entity(a.id).expect("Border preview retains areas");
            let target = screen(a.rect, bounds);
            let rect = if growing && !was_interacting && frame.motion.enabled {
                blend_rect(
                    state.area_bounds.get(&entity).copied().unwrap_or(target),
                    target,
                    blend,
                )
            } else {
                target
            };
            moving |= rect != target;
            (entity, a.rect, rect, surface(rect, a.rect, visual_count))
        })
        .collect();
    state.area_bounds = visual.iter().map(|(e, _, r, _)| (*e, *r)).collect();
    state.tab_bounds.clear();
    let mut next_positions = HashMap::new();
    let host_flags = WindowFlags::NO_DECORATION
        | WindowFlags::NO_MOVE
        | WindowFlags::NO_BACKGROUND
        | WindowFlags::NO_SAVED_SETTINGS
        | WindowFlags::NO_SCROLLBAR
        | WindowFlags::NO_SCROLL_WITH_MOUSE
        | WindowFlags::NO_BRING_TO_FRONT_ON_FOCUS;
    ui.window(format!("##workbench-host-{}", frame.view.to_bits()))
        .position(bounds.min.to_array(), Condition::Always)
        .size(bounds.size().to_array(), Condition::Always)
        .flags(host_flags)
        .build(|| {
            paint_surfaces(&frame, &visual, focused);
            // Corners take precedence over border handles at junctions.
            if !was_interacting && !cancel {
                'corners: for &(entity, _, r, s) in &visual {
                    for (corner, high) in
                        [[false, false], [true, false], [false, true], [true, true]]
                            .into_iter()
                            .enumerate()
                    {
                        let vertex = Vec2::new(
                            if high[0] { s.max.x } else { s.min.x },
                            if high[1] { s.max.y } else { s.min.y },
                        );
                        let inward = Vec2::new(
                            if high[0] {
                                vertex.x - mouse.x
                            } else {
                                mouse.x - vertex.x
                            },
                            if high[1] {
                                vertex.y - mouse.y
                            } else {
                                mouse.y - vertex.y
                            },
                        )
                        .max(Vec2::ZERO);
                        let lo = Vec2::new(
                            if high[0] { vertex.x - CORNER } else { r.min.x },
                            if high[1] { vertex.y - CORNER } else { r.min.y },
                        );
                        let hi = Vec2::new(
                            if high[0] { r.max.x } else { vertex.x + CORNER },
                            if high[1] { r.max.y } else { vertex.y + CORNER },
                        );
                        let zone = Rect::from_corners(lo, hi);
                        if zone.contains(mouse) && inward.element_sum() <= CORNER {
                            let (hovered, pressed) =
                                hit(ui, &format!("corner-{}-{corner}", entity.to_bits()), zone);
                            if hovered {
                                ui.set_mouse_cursor(Some(MouseCursor::ResizeAll));
                            }
                            if pressed && state.gesture.is_none() {
                                state.gesture = Some(Gesture::Corner {
                                    area: entity,
                                    start: mouse,
                                    axis: None,
                                });
                                edits.push(LayoutEdit::FocusArea(entity));
                                break 'corners;
                            }
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
                    let (hovered, pressed) =
                        hit(ui, &format!("border-{}-{index}", entity.to_bits()), zone);
                    if hovered && state.gesture.is_none() {
                        ui.set_mouse_cursor(Some(if index == 0 {
                            MouseCursor::ResizeEW
                        } else {
                            MouseCursor::ResizeNS
                        }));
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
                .map(|(e, p)| {
                    (
                        e,
                        p.label.clone(),
                        world
                            .get::<bedtime_desktop2::ActionEnabled>(e)
                            .is_none_or(|a| a.0),
                    )
                })
                .collect();
            for group in &groups {
                let (_, _, r, s) = visual
                    .iter()
                    .find(|(e, _, _, _)| *e == group.entity)
                    .unwrap();
                let tab_height = ui.text_line_height() + 10.0;
                let bar = Rect::from_corners(
                    s.min + Vec2::splat(CONTENT_INSET),
                    Vec2::new(
                        s.max.x - CONTENT_INSET,
                        s.min.y + CONTENT_INSET + tab_height,
                    ),
                );
                let content = Rect::from_corners(
                    Vec2::new(s.min.x + CONTENT_INSET, bar.max.y + 1.0),
                    s.max - Vec2::splat(CONTENT_INSET),
                );
                state.tab_bars.insert(group.entity, bar);
                *world.get_mut::<AreaPresentation>(group.entity).unwrap() = AreaPresentation {
                    visible: true,
                    rect: *r,
                    surface: *s,
                    tab_bar: bar,
                    content,
                };
                ui.get_window_draw_list()
                    .add_line(
                        [s.min.x + 1.0, bar.max.y],
                        [s.max.x - 1.0, bar.max.y],
                        ui.get_color_u32_from_rgba(frame.theme.palette.border),
                    )
                    .build();
                let _area_id = ui.push_id(group.entity.to_bits() as usize);
                let mut tabs_min = bar.min;
                if show_add {
                    let add =
                        Rect::from_corners(bar.min, Vec2::new(bar.min.x + tab_height, bar.max.y));
                    let (hovered, pressed) = hit(ui, "add", add);
                    let draw = ui.get_window_draw_list();
                    let center = add.center();
                    let color = ui.get_color_u32_from_rgba(if hovered {
                        frame.theme.palette.text
                    } else {
                        frame.theme.palette.muted
                    });
                    draw.add_line(
                        (center - Vec2::new(4.0, 0.0)).to_array(),
                        (center + Vec2::new(4.0, 0.0)).to_array(),
                        color,
                    )
                    .build();
                    draw.add_line(
                        (center - Vec2::new(0.0, 4.0)).to_array(),
                        (center + Vec2::new(0.0, 4.0)).to_array(),
                        color,
                    )
                    .build();
                    drop(draw);
                    if hovered {
                        frame.controls().tooltip("Add a panel");
                    }
                    if pressed {
                        ui.open_popup("picker");
                    }
                    tabs_min.x += tab_height;
                }
                let labels: Vec<_> = group
                    .tabs
                    .iter()
                    .map(|t| {
                        (
                            t,
                            (ui.calc_text_size(&t.title)[0]
                                + 37.0
                                + if t.dirty { 8.0 } else { 0.0 }
                                + if t.icon.is_some() {
                                    ui.text_line_height() + 5.0
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
                    scroll.offset -= (ui.io().mouse_wheel() + ui.io().mouse_wheel_h()) * 40.0;
                    if matches!(state.gesture, Some(Gesture::Tab { dragging: true, .. }))
                        || ui.drag_drop_payload().is_some()
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
                let _clip = ui.push_clip_rect(tabs_min.to_array(), bar.max.to_array(), true);
                for (tab, width) in labels {
                    let target = Rect::from_corners(
                        Vec2::new(x, bar.min.y + 2.0),
                        Vec2::new(x + width - 1.0, bar.max.y - 1.0),
                    );
                    x += width;
                    let position = if !was_interacting && frame.motion.enabled {
                        blend_rect(
                            state
                                .tab_positions
                                .get(&tab.entity)
                                .copied()
                                .unwrap_or(target),
                            target,
                            blend,
                        )
                    } else {
                        target
                    };
                    moving |= position != target;
                    next_positions.insert(tab.entity, position);
                    let clipped =
                        Rect::from_corners(position.min.max(tabs_min), position.max.min(bar.max));
                    if clipped.width() <= 0.0 {
                        continue;
                    }
                    state.tab_bounds.insert(tab.entity, clipped);
                    let close = Rect::from_corners(
                        Vec2::new((position.max.x - 21.0).max(tabs_min.x), position.min.y),
                        position.max.min(bar.max),
                    );
                    let (close_hover, close_pressed) =
                        hit(ui, &format!("close-{}", tab.entity.to_bits()), close);
                    let zone = Rect::from_corners(
                        clipped.min,
                        Vec2::new(close.min.x.min(clipped.max.x), clipped.max.y),
                    );
                    let (hovered, pressed) =
                        hit(ui, &format!("tab-{}", tab.entity.to_bits()), zone);
                    let selected = group.selected == Some(tab.entity);
                    let p = &frame.theme.palette;
                    let color = ui.get_color_u32_from_rgba(if hovered || close_hover {
                        p.elevated
                    } else if selected {
                        p.selection
                    } else {
                        p.background
                    });
                    let draw = ui.get_window_draw_list();
                    draw.add_rect(position.min.to_array(), position.max.to_array(), color)
                        .rounding(3.0)
                        .filled(true)
                        .build();
                    let text = ui.get_color_u32_from_rgba(if selected || hovered {
                        p.text
                    } else {
                        p.muted
                    });
                    {
                        let _text_clip = draw.push_clip_rect(
                            (position.min + Vec2::new(8.0, 0.0)).to_array(),
                            Vec2::new(close.min.x - 3.0, position.max.y).to_array(),
                            true,
                        );
                        let mut text_x = position.min.x + 8.0;
                        if tab.icon.is_some() {
                            text_x += ui.text_line_height() + 5.0;
                        }
                        draw.add_text([text_x, bar.min.y + 5.0], text, &tab.title);
                        if tab.preview {
                            draw.add_line(
                                [position.min.x + 8.0, bar.max.y - 4.0],
                                [close.min.x - 3.0, bar.max.y - 4.0],
                                text,
                            )
                            .build();
                        }
                    }
                    if tab.dirty {
                        draw.add_circle([close.min.x - 5.0, bar.center().y], 2.0, text)
                            .filled(true)
                            .build();
                    }
                    if selected || hovered || close_hover {
                        let c = close.center();
                        if close_hover {
                            draw.add_circle(
                                c.to_array(),
                                8.0,
                                ui.get_color_u32_from_rgba(p.elevated),
                            )
                            .filled(true)
                            .build();
                        }
                        draw.add_line(
                            (c - Vec2::splat(3.0)).to_array(),
                            (c + Vec2::splat(3.0)).to_array(),
                            text,
                        )
                        .build();
                        draw.add_line(
                            (c + Vec2::new(3.0, -3.0)).to_array(),
                            (c + Vec2::new(-3.0, 3.0)).to_array(),
                            text,
                        )
                        .build();
                    }
                    drop(draw);
                    if let Some(icon) = &tab.icon {
                        frame.controls().icon_at(
                            icon,
                            [
                                position.min.x + 8.0,
                                bar.center().y - ui.text_line_height() * 0.5,
                            ],
                            ui.text_line_height(),
                        );
                    }
                    {
                        let mut observation =
                            world.get_mut::<PanelPresentation>(tab.entity).unwrap();
                        observation.tab_rect = zone;
                        observation.close_rect = close;
                    }
                    if hovered {
                        frame.controls().tooltip(&tab.title);
                    }
                    if close_pressed
                        || (clipped.contains(mouse)
                            && ui.is_mouse_released(imgui::MouseButton::Middle))
                    {
                        edits.push(LayoutEdit::ClosePanels(vec![tab.entity]));
                    }
                    if pressed && state.gesture.is_none() && !was_interacting {
                        edits.push(LayoutEdit::FocusPanel(tab.entity));
                        if ui.is_mouse_double_clicked(imgui::MouseButton::Left) {
                            edits.push(LayoutEdit::KeepPanel(tab.entity));
                        }
                        state.gesture = Some(Gesture::Tab {
                            panel: tab.entity,
                            start: mouse,
                            dragging: false,
                        });
                    }
                    if !state.suppress_context {
                        let _tab_id = ui.push_id(tab.entity.to_bits() as usize);
                        frame.controls().context_menu("tab-actions", || {
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
                                if ui.menu_item_enabled_selected_no_shortcut(
                                    label,
                                    false,
                                    !tabs.is_empty(),
                                ) {
                                    edits.push(LayoutEdit::ClosePanels(tabs));
                                }
                            }
                        });
                    }
                }
                drop(_clip);
                if group.tabs.is_empty() && content.width() > 0.0 && content.height() > 0.0 {
                    let (hovered, pressed) = hit(ui, "empty-area", content);
                    if pressed {
                        edits.push(LayoutEdit::FocusArea(group.entity));
                    }
                    if hovered
                        && ui.is_mouse_released(imgui::MouseButton::Right)
                        && !state.suppress_context
                    {
                        ui.open_popup("picker");
                    }
                    ui.get_window_draw_list().add_text(
                        (content.min + Vec2::splat(14.0)).to_array(),
                        ui.get_color_u32_from_rgba(frame.theme.palette.muted),
                        "Right-click or use + to open a panel",
                    );
                }
                picker(&frame, &types, group.entity, &mut edits);
            }
            state.tab_positions = next_positions;
            // Content windows use panel identity, never area identity. Reparenting
            // a tab retains its native scroll state and its ECS/system-local state.
            for group in &groups {
                if let Some(panel) = group.selected {
                    let content = world.get::<AreaPresentation>(group.entity).unwrap().content;
                    if content.width() < 1.0 || content.height() < 1.0 {
                        continue;
                    }
                    ui.set_cursor_screen_pos(content.min.to_array());
                    ui.child_window(format!("panel-{}", panel.to_bits()))
                        .size(content.size().to_array())
                        .build(ui, || {
                            let min = Vec2::from_array(ui.cursor_screen_pos());
                            let rect = Rect::from_corners(
                                min,
                                min + Vec2::from_array(ui.content_region_avail()),
                            );
                            let focus = ui.is_window_focused();
                            {
                                let mut p = world.get_mut::<PanelPresentation>(panel).unwrap();
                                p.visible = true;
                                p.focused = focus;
                                p.rect = rect;
                            }
                            if focus && focused != group.entity {
                                edits.push(LayoutEdit::FocusArea(group.entity));
                            }
                            let motion = state.appearances.entry(panel).or_insert((0, 0.0));
                            if motion.0 + 1 < ui.frame_count() {
                                motion.1 = 0.0;
                            }
                            motion.0 = ui.frame_count();
                            motion.1 = if frame.motion.enabled {
                                (motion.1 + dt / frame.motion.seconds).min(1.0)
                            } else {
                                1.0
                            };
                            moving |= motion.1 < 1.0;
                            let opacity = 1.0 - (1.0 - motion.1).powi(3);
                            let _alpha = ui.push_style_var(StyleVar::Alpha(
                                ui.clone_style().alpha() * opacity,
                            ));
                            if world.get::<PanelRenderer>(panel).is_some() {
                                if let Err(error) = world.run_system_with(
                                    SystemId::<UiFrame<'static>>::from_entity(panel),
                                    frame.for_view(panel),
                                ) {
                                    world.write_message(bedtime_ui2::UiError(error.to_string()));
                                    ui.text_colored(frame.theme.palette.error, error.to_string());
                                }
                            } else {
                                ui.text_disabled("This panel has no content renderer.");
                            }
                        });
                }
            }
            // Viewport-level targets run after content targets. ImGui's smallest
            // accepted rectangle wins, so a widget can consume its own payload.
            if let Some((edit, rect, count)) =
                external_drop(&frame, &groups, &snapshot, state, mouse, bounds, minimum)
            {
                preview = Some(rect);
                preview_count = count;
                if let Some(edit) = edit {
                    edits.push(edit);
                }
            }
        });
    state
        .appearances
        .retain(|e, _| world.get::<Panel>(*e).is_some());
    state
        .tab_scroll
        .retain(|e, _| world.get::<Area>(*e).is_some());
    if let Some(target) = preview {
        let current = blend_rect(state.preview.unwrap_or(target), target, blend);
        moving |= current != target;
        state.preview = Some(current);
        let p = &frame.theme.palette;
        let border = std::array::from_fn(|i| p.border[i] + (p.text[i] - p.border[i]) * 0.75);
        let fill = [border[0], border[1], border[2], border[3] * 0.14];
        let draw = ui.get_foreground_draw_list();
        if current.width().abs() < 0.5 || current.height().abs() < 0.5 {
            draw.add_line(
                current.min.to_array(),
                current.max.to_array(),
                ui.get_color_u32_from_rgba(border),
            )
            .thickness(2.0)
            .build();
        } else {
            let rounding = if preview_count == 1 { 0.0 } else { 5.0 };
            draw.add_rect(
                current.min.to_array(),
                current.max.to_array(),
                ui.get_color_u32_from_rgba(fill),
            )
            .rounding(rounding)
            .filled(true)
            .build();
            draw.add_rect(
                current.min.to_array(),
                current.max.to_array(),
                ui.get_color_u32_from_rgba(border),
            )
            .rounding(rounding)
            .thickness(2.0)
            .build();
        }
    } else {
        state.preview = None;
    }
    if let Some(Gesture::Tab {
        panel,
        dragging: true,
        ..
    }) = &state.gesture
    {
        if let Some(tab) = groups
            .iter()
            .flat_map(|g| &g.tabs)
            .find(|t| t.entity == *panel)
        {
            let draw = ui.get_foreground_draw_list();
            let min = mouse + Vec2::splat(14.0);
            let max = min + Vec2::from_array(ui.calc_text_size(&tab.title)) + Vec2::new(16.0, 12.0);
            draw.add_rect(
                min.to_array(),
                max.to_array(),
                ui.get_color_u32_from_rgba(frame.theme.palette.elevated),
            )
            .rounding(5.0)
            .filled(true)
            .build();
            draw.add_text(
                (min + Vec2::new(8.0, 6.0)).to_array(),
                ui.get_color_u32_from_rgba(frame.theme.palette.text),
                &tab.title,
            );
        }
    }
    if moving || state.gesture.is_some() {
        world.write_message(bevy::window::RequestRedraw);
    }
    if state.suppress_context
        && !ui.is_mouse_down(imgui::MouseButton::Right)
        && !ui.is_mouse_released(imgui::MouseButton::Right)
    {
        state.suppress_context = false;
    }
    for edit in edits {
        world
            .resource_mut::<WorkbenchRequests>()
            .submit(WorkbenchRequest::Edit { stamp, edit });
    }
}

fn picker(
    frame: &UiFrame,
    types: &[(Entity, String, bool)],
    area: Entity,
    edits: &mut Vec<LayoutEdit>,
) {
    frame.controls().popup_menu("picker", || {
        for (kind, label, enabled) in types {
            if frame
                .ui
                .menu_item_enabled_selected_no_shortcut(label, false, *enabled)
            {
                edits.push(LayoutEdit::OpenPanel { area, kind: *kind });
            }
        }
        if types.is_empty() {
            frame.ui.text_disabled("No panel types registered");
        }
        frame.ui.separator();
        if frame.ui.menu_item("Close area") {
            edits.push(LayoutEdit::CloseArea(area));
        }
    });
}

fn tab_target(
    groups: &[Group],
    snapshot: &LayoutSnapshot,
    state: &Frontend,
    panel: Option<Entity>,
    mouse: Vec2,
    bounds: Rect,
    minimum: [i32; 2],
) -> Option<(LayoutEdit, Rect, usize)> {
    for group in groups {
        let r = state.area_bounds.get(&group.entity).copied()?;
        let s = surface(r, group.rect, groups.len());
        let bar = state.tab_bars.get(&group.entity).copied()?;
        let bar_bottom = bar.max.y;
        if bar.contains(mouse) {
            let mut index = 0;
            let mut x = bar.min.x + bar.height();
            for tab in group.tabs.iter().filter(|t| Some(t.entity) != panel) {
                if let Some(r) = state.tab_positions.get(&tab.entity) {
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
            surface(screen(destination, bounds), destination, next.areas.len()),
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
            surface(screen(destination, bounds), destination, next.areas.len()),
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

fn paint_surfaces(
    frame: &UiFrame,
    areas: &[(Entity, geometry::Rect, Rect, Rect)],
    focused: Entity,
) {
    let ui = frame.ui;
    let p = &frame.theme.palette;
    let draw = ui.get_window_draw_list();
    // Themes currently use opaque panel backgrounds. Paint gaps first and leave
    // ordinary popup/floating-window style untouched.
    let bounds = areas
        .iter()
        .map(|(_, _, r, _)| *r)
        .reduce(|a, b| Rect::from_corners(a.min.min(b.min), a.max.max(b.max)))
        .unwrap();
    draw.add_rect(
        bounds.min.to_array(),
        bounds.max.to_array(),
        ui.get_color_u32_from_rgba(p.background),
    )
    .filled(true)
    .build();
    for &(entity, _, _, s) in areas {
        let rounding = if areas.len() == 1 { 0.0 } else { 5.0 };
        draw.add_rect(
            s.min.to_array(),
            s.max.to_array(),
            ui.get_color_u32_from_rgba(p.background),
        )
        .rounding(rounding)
        .filled(true)
        .build();
        if areas.len() > 1 {
            let mut border = p.border;
            if entity == focused {
                for v in &mut border[..3] {
                    *v += (1.0 - *v) * 0.2;
                }
            }
            draw.add_rect(
                s.min.to_array(),
                s.max.to_array(),
                ui.get_color_u32_from_rgba(border),
            )
            .rounding(rounding)
            .thickness(1.0)
            .build();
        }
    }
}

fn external_drop(
    frame: &UiFrame,
    groups: &[Group],
    snapshot: &LayoutSnapshot,
    state: &mut Frontend,
    mouse: Vec2,
    bounds: Rect,
    minimum: [i32; 2],
) -> Option<(Option<LayoutEdit>, Rect, usize)> {
    let ui = frame.ui;
    if ui.drag_drop_payload().is_none() {
        state.payload_cancelled = false;
        return None;
    }
    if ui.is_key_pressed(Key::Escape) || ui.is_mouse_clicked(imgui::MouseButton::Right) {
        state.payload_cancelled = true;
        state.suppress_context = true;
    }
    if state.payload_cancelled {
        return None;
    }
    let (edit, preview, count) = tab_target(groups, snapshot, state, None, mouse, bounds, minimum)?;
    let area = match &edit {
        LayoutEdit::DropPanel { area, .. } => *area,
        _ => unreachable!(),
    };
    let rect = state.area_bounds[&area];
    let (delivery, kind) = ui.with_bound_context(|| unsafe {
        // The viewport fallback is larger than content widgets. Native target
        // arbitration preserves the smaller widget's priority. Our own payload
        // wire format is exactly eight little-endian entity bits, copied by ImGui.
        let r = imgui::sys::ImRect_c {
            Min: rect.min.to_array().into(),
            Max: rect.max.to_array().into(),
        };
        if !imgui::sys::igBeginDragDropTargetViewport(imgui::sys::igGetMainViewport(), &r) {
            return None;
        }
        let name = std::ffi::CString::new(PANEL_TYPE_PAYLOAD).unwrap();
        let p = imgui::sys::igAcceptDragDropPayload(
            name.as_ptr(),
            imgui::sys::ImGuiDragDropFlags_AcceptBeforeDelivery
                | imgui::sys::ImGuiDragDropFlags_AcceptNoDrawDefaultRect,
        );
        let result = if p.is_null() || (*p).Data.is_null() || (*p).DataSize != 8 {
            None
        } else {
            let bytes = std::ptr::read_unaligned((*p).Data.cast::<[u8; 8]>());
            Entity::try_from_bits(u64::from_le_bytes(bytes)).map(|kind| ((*p).Delivery, kind))
        };
        imgui::sys::igEndDragDropTarget();
        result
    })?;
    let mut edit = edit;
    if let LayoutEdit::DropPanel { kind: target, .. } = &mut edit {
        *target = kind;
    }
    Some((delivery.then_some(edit), preview, count))
}

/// Call immediately after an application toolbar/picker item. The payload
/// references a registered panel type; workbench owns opening the dropped panel.
pub fn panel_drag_source(ui: &Ui, kind: Entity, label: &str) {
    let bytes = kind.to_bits().to_le_bytes();
    // SAFETY: begin copies all eight initialized bytes before this scope ends.
    if let Some(_source) = unsafe {
        ui.drag_drop_source_config(PANEL_TYPE_PAYLOAD)
            .begin_payload_unchecked(bytes.as_ptr().cast(), bytes.len())
    } {
        ui.text(label);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn projection(w: &Workbench, layout: Entity, bounds: Rect) -> (Vec<Group>, Frontend) {
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
                let s = surface(r, rect, areas.len());
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
        app.add_plugins(WorkbenchPlugin);
        let mut w = Workbench::new(app.world_mut());
        let layout = w.active_layout();
        let source = w.destination_area(layout).unwrap();
        let target = w
            .split_area(source, SplitAxis::Horizontal, 5000, [100, 100])
            .unwrap();
        let panel = w.create_panel_in_area(source, "Moving").unwrap();
        let bounds = Rect::from_corners(Vec2::new(50.0, 75.0), Vec2::new(1250.0, 875.0));
        let snapshot = w.snapshot(layout).unwrap();
        let (groups, state) = projection(&w, layout, bounds);
        let target_rect = screen(w.world.get::<Area>(target).unwrap().rect(), bounds);

        let (_, preview, count) = tab_target(
            &groups,
            &snapshot,
            &state,
            Some(panel),
            target_rect.center(),
            bounds,
            [100, 100],
        )
        .unwrap();
        assert_eq!(count, 1);
        assert_eq!(
            preview, bounds,
            "The last source closes; the only area has no inset"
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
        )
        .unwrap();
        assert_eq!(count, 2);
        assert_eq!(
            preview,
            Rect::from_corners(Vec2::new(55.0, 80.0), Vec2::new(947.5, 870.0)),
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
                    app.add_plugins(WorkbenchPlugin);
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
                    let (groups, state) = projection(&w, layout, bounds);
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
                    )
                    .unwrap();
                    w.edit(w.stamp(), &edit).unwrap();
                    let destination = w.world.get::<InArea>(panel).unwrap().0;
                    let rect = w.world.get::<Area>(destination).unwrap().rect();
                    let actual_count = w.world.get::<LayoutAreas>(layout).unwrap().entities().len();
                    assert_eq!(count, actual_count);
                    assert_eq!(
                        preview,
                        surface(screen(rect, bounds), rect, actual_count),
                        "T={t_junction}, last={last_tab}, mouse={relative:?}"
                    );
                    assert!(w.world.get::<Panel>(target_panel).is_some());
                }
            }
        }
    }
}
