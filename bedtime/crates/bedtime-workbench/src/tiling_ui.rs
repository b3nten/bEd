//! Workspace-owned tabs and placement. Changes commit before the next frame's
//! content windows; native flat docks only present the selected content.
use super::{bed_imgui_dock_node_clear_background, bed_imgui_dock_node_select_window};
use crate::workspace::tiling::{Area, DockPlacement, DockPlan, EXTENT, Layout, Rect};
use crate::workspace::tiling_state::TilingState;
use bedtime_api::workbench::WorkbenchAction;
use bevy::prelude::Entity;
use dear_imgui_rs::{
    DrawCornerFlags, FocusedFlags, Key, MouseButton, MouseCursor, StyleColor, StyleVar, Ui,
    WindowFlags, sys,
};
use std::{collections::HashMap, ffi::CString};

const AREA_INSET: f32 = 2.5;
// Keep rectangular content inside the rounded surface, including opaque images.
const CONTENT_PADDING: f32 = 2.0;
const AREA_ROUNDING: f32 = 5.0;
const CORNER_SIZE: f32 = 20.0;
const HANDLE_SIZE: f32 = 8.0;

type Bounds = ([f32; 2], [f32; 2]);

#[derive(Clone, Copy)]
pub(super) enum TabPlacement {
    Insert { area: u32, index: usize },
    Split { area: u32, axis: usize, high: bool },
}

pub(super) enum TilingAction {
    Split {
        area: u32,
        axis: usize,
        at: i32,
    },
    Plan {
        area: u32,
        plan: DockPlan,
    },
    MoveTab {
        panel: u64,
        area: u32,
        index: usize,
    },
    SplitTab {
        panel: u64,
        area: u32,
        axis: usize,
        high: bool,
    },
    CloseTab(u64),
    CloseArea(u32),
    AddPanel {
        area: u32,
        kind: bedtime_api::PanelTypeId,
    },
    Drop {
        placement: TabPlacement,
        payload_type: String,
        data: Vec<u8>,
    },
}

enum Gesture {
    Corner {
        area: u32,
        start: [f32; 2],
        axis: Option<usize>,
    },
    Tab {
        panel: u64,
        start: [f32; 2],
        dragging: bool,
    },
    Border {
        original: Layout,
        axis: usize,
        at: i32,
        span: [i32; 2],
    },
}

#[derive(Default)]
struct TabScroll {
    offset: f32,
    selected: Option<u64>,
}

pub(super) struct TilingUi {
    pub docks: HashMap<u32, u32>,
    pub tab_rects: HashMap<u64, Bounds>,
    tab_positions: HashMap<u64, Bounds>,
    pub bar_rects: HashMap<u32, Bounds>,
    pub(super) area_bounds: HashMap<u32, Bounds>,
    expanding: bool,
    tab_scroll: HashMap<u32, TabScroll>,
    gesture: Option<Gesture>,
    pub(super) preview_bounds: Option<Bounds>,
    preview_target: Option<(Bounds, usize)>,
    dragging_panel: Option<u64>,
    payload_cancelled: bool,
    pub clear_roots: bool,
    pub origin: [f32; 2],
    pub size: [f32; 2],
    pub minimum: [i32; 2],
    main_viewport: u32,
    corner_cursor: bool,
    suppress_right_picker: bool,
    pub actions: Vec<TilingAction>,
}

impl Default for TilingUi {
    fn default() -> Self {
        Self {
            docks: HashMap::new(),
            tab_rects: HashMap::new(),
            tab_positions: HashMap::new(),
            bar_rects: HashMap::new(),
            area_bounds: HashMap::new(),
            expanding: false,
            tab_scroll: HashMap::new(),
            gesture: None,
            preview_bounds: None,
            preview_target: None,
            dragging_panel: None,
            payload_cancelled: false,
            clear_roots: true,
            origin: [0.0; 2],
            size: [1.0; 2],
            minimum: [1; 2],
            main_viewport: 0,
            corner_cursor: false,
            suppress_right_picker: false,
            actions: Vec::new(),
        }
    }
}

impl TilingUi {
    pub fn cancel_interaction(&mut self) {
        self.gesture = None;
        self.preview_bounds = None;
        self.preview_target = None;
        self.dragging_panel = None;
        self.corner_cursor = false;
        self.actions.clear();
    }
}

fn contains((min, max): Bounds, point: [f32; 2]) -> bool {
    (0..2).all(|axis| point[axis] >= min[axis] && point[axis] < max[axis])
}

fn area_surface(min: [f32; 2], max: [f32; 2], rect: Rect, area_count: usize) -> Bounds {
    if area_count == 1 {
        return (min, max);
    }
    (
        [0, 1].map(|axis| min[axis] + AREA_INSET * if rect.min[axis] == 0 { 2.0 } else { 1.0 }),
        [0, 1]
            .map(|axis| max[axis] - AREA_INSET * if rect.max[axis] == EXTENT { 2.0 } else { 1.0 }),
    )
}

// Explicit button ownership keeps workspace chrome usable over dock children.
// ImGui still blocks these items normally while another popup owns input.
fn hit_zone(ui: &Ui, name: &str, (min, max): Bounds) -> (bool, bool) {
    let id = ui.get_id(name).raw();
    let bounds = sys::ImRect_c {
        Min: min.into(),
        Max: max.into(),
    };
    let mut hovered = false;
    let mut held = false;
    let pressed = unsafe {
        sys::igItemAdd(bounds, id, std::ptr::null(), sys::ImGuiItemFlags_NoNav);
        sys::igButtonBehavior(
            bounds,
            id,
            &mut hovered,
            &mut held,
            sys::ImGuiButtonFlags_FlattenChildren
                | sys::ImGuiButtonFlags_PressedOnClick
                | sys::ImGuiButtonFlags_NoNavFocus
                | sys::ImGuiButtonFlags_NoFocus,
        )
    };
    (hovered, pressed)
}

#[derive(Clone)]
pub(crate) struct ChromeTab {
    pub entity: Entity,
    pub id: u64,
    pub title: String,
    pub icon: Option<dear_imgui_rs::TextureId>,
    pub dirty: bool,
}

#[derive(Clone)]
pub(crate) struct ChromePanel {
    pub id: bedtime_api::PanelTypeId,
    pub label: String,
    pub allow_empty: bool,
}

#[derive(Clone, Default)]
pub(crate) struct ChromeSettings {
    pub animations: bool,
    pub add_button: bool,
    pub tab_icons: bool,
    pub background: [f32; 4],
    pub opacity: f32,
}

impl ChromeSettings {
    fn bool(&self, key: &str, _default: bool) -> bool {
        match key {
            "ui_animations" => self.animations,
            "tab_bar_add_button" => self.add_button,
            _ => _default,
        }
    }

    fn window_background_color(&self) -> [f32; 4] {
        self.background
    }

    fn background_opacity(&self) -> f32 {
        self.opacity
    }
}

/// Only widget gestures and observations live here. `tiling` and `tabs` are
/// frame-entry read snapshots; the request reducer owns their world state.
#[derive(Default)]
pub(crate) struct TilingFrontend {
    pub preview_font: Option<dear_imgui_rs::FontId>,
    pub tiling: TilingState,
    pub tiling_ui: TilingUi,
    pub tabs: Vec<ChromeTab>,
    pub definitions: Vec<ChromePanel>,
    pub settings: ChromeSettings,
    pub focused: Option<u64>,
    pub focused_area: Option<u32>,
    pub requests: Vec<WorkbenchAction>,
    tab_context: Option<u64>,
}

impl TilingFrontend {
    pub fn refresh(&mut self, tiling: TilingState, tabs: Vec<ChromeTab>) {
        if matches!(&self.tiling_ui.gesture,Some(Gesture::Tab{panel,..}) if !tabs.iter().any(|tab|tab.id==*panel))
        {
            self.tiling_ui.cancel_interaction();
        }
        if matches!(&self.tiling_ui.gesture,Some(Gesture::Corner{area,..}) if !tiling.areas.iter().any(|group|group.area==*area))
        {
            self.tiling_ui.cancel_interaction();
        }
        self.tiling = tiling;
        self.tabs = tabs;
    }

    fn title(&self, tab: &ChromeTab) -> String {
        tab.title.clone()
    }

    fn focused_area(&self) -> Option<u32> {
        self.focused
            .and_then(|id| self.tiling.area_for(id))
            .or(self.focused_area)
    }

    fn current_tiling(&self) -> &TilingState {
        &self.tiling
    }

    pub(super) fn draw_tab_context_menu(&mut self, ui: &Ui) {
        if ui.is_mouse_released(MouseButton::Right)
            && !self.tiling_ui.suppress_right_picker
            && !ui.is_popup_open("Tab actions")
        {
            let mouse = ui.io().mouse_pos();
            self.tab_context = self
                .tiling_ui
                .tab_rects
                .iter()
                .find_map(|(id, bounds)| contains(*bounds, mouse).then_some(*id));
            if self.tab_context.is_some() {
                ui.open_popup("Tab actions");
            }
        }
        if ui.is_popup_open("Tab actions") {
            ui.set_next_window_viewport(ui.main_viewport().id());
        }
        let fs = ui.current_font_size();
        let _padding = ui.push_style_var(StyleVar::WindowPadding([fs * 0.85, fs * 0.7]));
        let _spacing = ui.push_style_var(StyleVar::ItemSpacing([fs * 0.6, fs * 0.4]));
        if let Some(_popup) = ui.begin_popup("Tab actions") {
            let Some((group, position)) = self.tiling.areas.iter().find_map(|group| {
                group
                    .tabs
                    .iter()
                    .position(|id| Some(*id) == self.tab_context)
                    .map(|position| (group, position))
            }) else {
                ui.close_current_popup();
                return;
            };
            for (label, range, except) in [
                ("Close", position..position + 1, None),
                ("Close All", 0..group.tabs.len(), None),
                ("Close Others", 0..group.tabs.len(), Some(position)),
                ("Close to the Right", position + 1..group.tabs.len(), None),
                ("Close to the Left", 0..position, None),
            ] {
                let tabs = range
                    .filter(|index| Some(*index) != except)
                    .map(|index| group.tabs[index])
                    .collect::<Vec<_>>();
                if ui.menu_item_enabled_selected_no_shortcut(label, false, !tabs.is_empty()) {
                    self.tiling_ui
                        .actions
                        .extend(tabs.into_iter().map(TilingAction::CloseTab));
                }
            }
        }
    }

    fn focus_empty_area(&mut self, area: u32) {
        self.focused = None;
        self.focused_area = Some(area);
        self.requests.push(WorkbenchAction::FocusArea(area));
    }

    fn switch_to_tab(&mut self, index: usize) {
        let tab = &self.tabs[index];
        self.focused = Some(tab.id);
        self.focused_area = self.tiling.area_for(tab.id);
        self.requests.push(WorkbenchAction::FocusPanel(tab.entity));
        if let Some(area) = self.focused_area {
            self.tiling
                .areas
                .iter_mut()
                .find(|group| group.area == area)
                .unwrap()
                .selected = Some(tab.id);
        }
    }
}

impl TilingFrontend {
    // Existing tabs and incoming payloads use the same insertion and split targets.
    fn tab_drop_target(
        &self,
        ui: &Ui,
        panel: Option<u64>,
    ) -> Option<(TabPlacement, Bounds, usize)> {
        let mouse = ui.io().mouse_pos();
        let screen = |point: [i32; 2]| {
            [0, 1].map(|axis| {
                self.tiling_ui.origin[axis]
                    + point[axis] as f32 * self.tiling_ui.size[axis] / EXTENT as f32
            })
        };
        if let Some((&area, &(min, max))) = self
            .tiling_ui
            .bar_rects
            .iter()
            .find(|(_, bounds)| contains(**bounds, mouse))
        {
            let group = self
                .tiling
                .areas
                .iter()
                .find(|group| group.area == area)
                .unwrap();
            let mut index = 0;
            let mut x = min[0] + ui.text_line_height() + 10.0;
            for id in group.tabs.iter().filter(|id| Some(**id) != panel) {
                if let Some((left, right)) = self.tiling_ui.tab_positions.get(id) {
                    if mouse[0] < (left[0] + right[0]) * 0.5 {
                        break;
                    }
                    x = right[0];
                }
                index += 1;
            }
            return Some((
                TabPlacement::Insert { area, index },
                ([x, min[1] + 3.0], [x, max[1] - 3.0]),
                self.tiling.layout.areas.len(),
            ));
        }
        let point = [0, 1].map(|axis| {
            ((mouse[axis] - self.tiling_ui.origin[axis]) * EXTENT as f32
                / self.tiling_ui.size[axis])
                .round() as i32
        });
        let target = self
            .tiling
            .layout
            .areas
            .iter()
            .find(|area| area.rect.contains(point))?;
        let relative = [0, 1].map(|axis| {
            (point[axis] - target.rect.min[axis]) as f32
                / (target.rect.max[axis] - target.rect.min[axis]) as f32
        });
        let edges = [
            relative[0],
            1.0 - relative[0],
            relative[1],
            1.0 - relative[1],
        ];
        let edge = (0..4)
            .min_by(|a, b| edges[*a].total_cmp(&edges[*b]))
            .unwrap();
        let mut next = self.tiling.clone();
        let (placement, destination) = if edges[edge] < 0.25 {
            let axis = edge / 2;
            let high = edge % 2 == 1;
            let destination = if let Some(panel) = panel {
                next.split_tab(panel, target.id, axis, high, self.tiling_ui.minimum)?
            } else {
                next.split_area(target.id, axis, high, self.tiling_ui.minimum)?
            };
            (
                TabPlacement::Split {
                    area: target.id,
                    axis,
                    high,
                },
                destination,
            )
        } else {
            let index = self
                .tiling
                .areas
                .iter()
                .find(|group| group.area == target.id)
                .unwrap()
                .tabs
                .iter()
                .position(|id| Some(*id) == panel)
                .unwrap_or(usize::MAX);
            if let Some(panel) = panel {
                next.move_tab(panel, target.id, index);
            }
            (
                TabPlacement::Insert {
                    area: target.id,
                    index,
                },
                target.id,
            )
        };
        let rect = next.layout.area(destination).rect;
        let count = next.layout.areas.len();
        Some((
            placement,
            area_surface(screen(rect.min), screen(rect.max), rect, count),
            count,
        ))
    }

    pub(super) fn accept_panel_drop(&mut self, ui: &Ui) {
        let active = unsafe { sys::igGetDragDropPayload() };
        if active.is_null() {
            self.tiling_ui.payload_cancelled = false;
            return;
        }
        if ui.is_key_pressed(Key::Escape) || ui.is_mouse_clicked(MouseButton::Right) {
            self.tiling_ui.payload_cancelled = true;
            self.tiling_ui.suppress_right_picker |= ui.is_mouse_clicked(MouseButton::Right);
        }
        if self.tiling_ui.payload_cancelled {
            return;
        }
        let Some((placement, preview, count)) = self.tab_drop_target(ui, None) else {
            return;
        };
        let area = match placement {
            TabPlacement::Insert { area, .. } | TabPlacement::Split { area, .. } => area,
        };
        let (min, max) = self.tiling_ui.area_bounds[&area];
        unsafe {
            // Viewport targets include the workspace tab strip and gaps. A smaller
            // content widget (such as a Files row) still owns its payload.
            let bounds = sys::ImRect_c {
                Min: min.into(),
                Max: max.into(),
            };
            if !sys::igBeginDragDropTargetViewport(sys::igGetMainViewport(), &bounds) {
                return;
            }
            let payload_type = std::ffi::CStr::from_ptr((*active).DataType.as_ptr());
            let accepted = sys::igAcceptDragDropPayload(
                payload_type.as_ptr(),
                sys::ImGuiDragDropFlags_AcceptBeforeDelivery
                    | sys::ImGuiDragDropFlags_AcceptNoDrawDefaultRect,
            );
            if !accepted.is_null() {
                if !(*accepted).Delivery {
                    self.tiling_ui.preview_target = Some((preview, count));
                } else {
                    let data = if (*accepted).DataSize > 0 {
                        std::slice::from_raw_parts(
                            (*accepted).Data.cast::<u8>(),
                            (*accepted).DataSize as usize,
                        )
                        .to_vec()
                    } else {
                        Vec::new()
                    };
                    self.tiling_ui.actions.push(TilingAction::Drop {
                        placement,
                        payload_type: payload_type.to_string_lossy().into_owned(),
                        data,
                    });
                }
            }
            sys::igEndDragDropTarget();
        }
    }

    pub(super) fn draw_tiling(&mut self, ui: &Ui, origin: [f32; 2], size: [f32; 2]) {
        self.tiling_ui.preview_target = None;
        self.tiling_ui.dragging_panel = None;
        let size = size.map(|value| value.max(1.0));
        let viewport = ui.main_viewport().id().raw();
        let view_changed = self.tiling_ui.origin != origin
            || self.tiling_ui.size != size
            || self.tiling_ui.main_viewport != viewport;
        self.tiling_ui.origin = origin;
        self.tiling_ui.size = size;
        self.tiling_ui.main_viewport = viewport;
        let minimum = [
            (160.0 * EXTENT as f32 / size[0]).ceil() as i32,
            (110.0 * EXTENT as f32 / size[1]).ceil() as i32,
        ]
        .map(|value| value.clamp(1, EXTENT));
        self.tiling_ui.minimum = minimum;
        let screen = |point: [i32; 2]| {
            [0, 1].map(|axis| origin[axis] + point[axis] as f32 * size[axis] / EXTENT as f32)
        };
        let coordinate = |point: [f32; 2]| {
            [0, 1].map(|axis| {
                ((point[axis] - origin[axis]) * EXTENT as f32 / size[axis]).round() as i32
            })
        };
        if self.tiling_ui.clear_roots {
            for dock in self.tiling_ui.docks.values() {
                unsafe { sys::igDockBuilderRemoveNode(*dock) };
            }
            self.tiling_ui.docks.clear();
            self.tiling_ui.gesture = None;
            self.tiling_ui.preview_bounds = None;
            self.tiling_ui.area_bounds.clear();
            self.tiling_ui.expanding = false;
            self.tiling_ui.clear_roots = false;
        }
        let mouse = ui.io().mouse_pos();
        self.tiling_ui.corner_cursor = false;
        let point = coordinate(mouse);
        let area_count = self.tiling.layout.areas.len();
        let content_padding = if area_count == 1 {
            0.0
        } else {
            CONTENT_PADDING
        };
        let rounding = if area_count == 1 { 0.0 } else { AREA_ROUNDING };
        let tab_height = ui.text_line_height() + 10.0;
        let interacting = self.tiling_ui.gesture.is_some() || ui.drag_drop_payload().is_some();
        if interacting && (self.tiling_ui.expanding || view_changed) {
            // The initiating press hit the displayed control. Finish expansion
            // before using final-model coordinates for its drag, and move the
            // previous frame's insertion targets to that same final geometry.
            for area in &self.tiling.layout.areas {
                let target = (screen(area.rect.min), screen(area.rect.max));
                self.tiling_ui.area_bounds.insert(area.id, target);
                let (surface_min, surface_max) =
                    area_surface(target.0, target.1, area.rect, area_count);
                let bar_min = surface_min.map(|value| value + content_padding);
                let bar_max = [surface_max[0] - content_padding, bar_min[1] + tab_height];
                if let Some((previous, _)) =
                    self.tiling_ui.bar_rects.insert(area.id, (bar_min, bar_max))
                {
                    let delta = [0, 1].map(|axis| bar_min[axis] - previous[axis]);
                    for panel in &self
                        .tiling
                        .areas
                        .iter()
                        .find(|group| group.area == area.id)
                        .expect("live area has a tab group")
                        .tabs
                    {
                        if let Some((min, max)) = self.tiling_ui.tab_positions.get_mut(panel) {
                            for axis in 0..2 {
                                min[axis] += delta[axis];
                                max[axis] += delta[axis];
                            }
                        }
                    }
                }
            }
            self.tiling_ui.expanding = false;
        }
        let mut preview: Option<Rect> = None;
        let mut preview_count = area_count;
        let mut preview_line = None;
        if let Some(gesture) = self.tiling_ui.gesture.take() {
            let cancel = ui.is_key_pressed(Key::Escape) || ui.is_mouse_clicked(MouseButton::Right);
            if ui.is_mouse_clicked(MouseButton::Right) {
                self.tiling_ui.suppress_right_picker = true;
            }
            match gesture {
                Gesture::Border {
                    original,
                    axis,
                    at,
                    span,
                } => {
                    self.tiling.layout = original.clone();
                    if !cancel {
                        self.tiling
                            .layout
                            .move_border(axis, at, span, point[axis], minimum[axis]);
                        ui.set_mouse_cursor(Some(if axis == 0 {
                            MouseCursor::ResizeEW
                        } else {
                            MouseCursor::ResizeNS
                        }));
                        if ui.is_mouse_down(MouseButton::Left) {
                            self.tiling_ui.gesture = Some(Gesture::Border {
                                original,
                                axis,
                                at,
                                span,
                            });
                        }
                    }
                }
                Gesture::Corner {
                    area,
                    start,
                    mut axis,
                } => {
                    if !cancel {
                        let rect = self.tiling.layout.area(area).rect;
                        let delta = [mouse[0] - start[0], mouse[1] - start[1]];
                        if axis.is_none() && delta[0].abs().max(delta[1].abs()) >= 7.0 {
                            axis = Some(usize::from(delta[1].abs() > delta[0].abs()));
                        }
                        let target = axis.and_then(|_| {
                            self.tiling
                                .layout
                                .areas
                                .iter()
                                .find(|candidate| {
                                    candidate.id != area && candidate.rect.contains(point)
                                })
                                .copied()
                        });
                        let action = if let Some(target) = target {
                            let joining = (0..2).any(|axis| {
                                let perpendicular = axis ^ 1;
                                let length = target.rect.max[axis] - target.rect.min[axis];
                                let margin = if length < minimum[axis] * 2 {
                                    length
                                } else {
                                    (length / 4).min(minimum[axis] * 5)
                                };
                                rect.min[perpendicular] < target.rect.max[perpendicular]
                                    && target.rect.min[perpendicular] < rect.max[perpendicular]
                                    && point[perpendicular] >= rect.min[perpendicular]
                                    && point[perpendicular] <= rect.max[perpendicular]
                                    && ((rect.max[axis] == target.rect.min[axis]
                                        && point[axis] <= target.rect.min[axis] + margin)
                                        || (rect.min[axis] == target.rect.max[axis]
                                            && point[axis] >= target.rect.max[axis] - margin))
                            });
                            let plan = if joining {
                                self.tiling.layout.plan_join(area, target.id, minimum)
                            } else {
                                let relative = [0, 1].map(|axis| {
                                    (point[axis] - target.rect.min[axis]) as f32
                                        / (target.rect.max[axis] - target.rect.min[axis]) as f32
                                });
                                let placement =
                                    if relative.iter().all(|value| (0.4..=0.6).contains(value)) {
                                        DockPlacement::Replace
                                    } else {
                                        let edges = [
                                            relative[0],
                                            1.0 - relative[0],
                                            relative[1],
                                            1.0 - relative[1],
                                        ];
                                        let edge = (0..4)
                                            .min_by(|a, b| edges[*a].total_cmp(&edges[*b]))
                                            .unwrap();
                                        let fraction = edges[edge] * 2.0;
                                        let fraction = if (fraction - 0.5).abs() <= 0.04 {
                                            0.5
                                        } else {
                                            fraction
                                        };
                                        DockPlacement::Split {
                                            axis: edge / 2,
                                            high: edge % 2 == 1,
                                            fraction: (fraction * EXTENT as f32).round() as i32,
                                        }
                                    };
                                self.tiling
                                    .layout
                                    .plan_dock(area, target.id, placement, minimum)
                            };
                            plan.map(|plan| {
                                preview = Some(plan.layout.area(area).rect);
                                preview_count = plan.layout.areas.len();
                                TilingAction::Plan { area, plan }
                            })
                        } else if let Some(axis) = axis.filter(|_| rect.contains(point)) {
                            if rect.max[axis] - rect.min[axis] >= minimum[axis] * 2 {
                                let at = point[axis].clamp(
                                    rect.min[axis] + minimum[axis],
                                    rect.max[axis] - minimum[axis],
                                );
                                let mut first = rect.min;
                                let mut second = rect.max;
                                first[axis] = at;
                                second[axis] = at;
                                preview_line = Some((first, second));
                                Some(TilingAction::Split { area, axis, at })
                            } else {
                                None
                            }
                        } else {
                            None
                        };
                        self.tiling_ui.corner_cursor = true;
                        if ui.is_mouse_released(MouseButton::Left) {
                            if let Some(action) = action {
                                self.tiling_ui.actions.push(action);
                            }
                        } else if ui.is_mouse_down(MouseButton::Left) {
                            self.tiling_ui.gesture = Some(Gesture::Corner { area, start, axis });
                        }
                    }
                }
                Gesture::Tab {
                    panel,
                    start,
                    mut dragging,
                } => {
                    if !cancel && self.tabs.iter().any(|tab| tab.id == panel) {
                        dragging |=
                            (mouse[0] - start[0]).abs().max((mouse[1] - start[1]).abs()) >= 7.0;
                        let mut action = None;
                        if dragging {
                            self.tiling_ui.dragging_panel = Some(panel);
                            if let Some((placement, bounds, count)) =
                                self.tab_drop_target(ui, Some(panel))
                            {
                                self.tiling_ui.preview_target = Some((bounds, count));
                                action = Some(match placement {
                                    TabPlacement::Insert { area, index } => {
                                        TilingAction::MoveTab { panel, area, index }
                                    }
                                    TabPlacement::Split { area, axis, high } => {
                                        TilingAction::SplitTab {
                                            panel,
                                            area,
                                            axis,
                                            high,
                                        }
                                    }
                                });
                            }
                        }
                        if ui.is_mouse_released(MouseButton::Left) {
                            if let Some(action) = action {
                                self.tiling_ui.actions.push(action);
                            }
                        } else if ui.is_mouse_down(MouseButton::Left) {
                            self.tiling_ui.gesture = Some(Gesture::Tab {
                                panel,
                                start,
                                dragging,
                            });
                        }
                    }
                }
            }
        }

        self.tiling_ui.tab_rects.clear();
        self.tiling_ui.tab_positions.clear();
        self.tiling_ui.bar_rects.clear();
        let areas = self.tiling.layout.areas.clone();
        let previous_count = self.tiling_ui.area_bounds.len();
        // Only grow into freed space. A topology rewrite can also shrink or
        // move survivors; interpolating those rectangles could overlap native
        // contents and composite their translucent backgrounds twice.
        let growing = areas.iter().all(|area| {
            let target = (screen(area.rect.min), screen(area.rect.max));
            self.tiling_ui
                .area_bounds
                .get(&area.id)
                .is_none_or(|(min, max)| {
                    (0..2).all(|axis| target.0[axis] <= min[axis] && target.1[axis] >= max[axis])
                })
        });
        let expanding = self.settings.bool("ui_animations", true)
            && !view_changed
            && !interacting
            && growing
            && areas.len() <= previous_count
            && (self.tiling_ui.expanding || areas.len() < previous_count);
        // Presses/releases refer to the last displayed controls. Hold that
        // sample for their hit tests; an established gesture finishes motion
        // on the next frame before its model-coordinate calculations.
        let pointer_action = [MouseButton::Left, MouseButton::Right, MouseButton::Middle]
            .into_iter()
            .any(|button| ui.is_mouse_clicked(button) || ui.is_mouse_released(button));
        let blend = if pointer_action {
            0.0
        } else {
            1.0 - (-ui.io().delta_time() / 0.04).exp()
        };
        let mut changed = false;
        let mut unfinished = false;
        for area in &areas {
            let target = (screen(area.rect.min), screen(area.rect.max));
            let previous = self.tiling_ui.area_bounds.get(&area.id).copied();
            let shown = if expanding {
                let current = previous.unwrap_or(target);
                let point = |current: [f32; 2], target: [f32; 2]| {
                    [0, 1].map(|axis| {
                        let value = current[axis] + (target[axis] - current[axis]) * blend;
                        if (target[axis] - value).abs() < 0.25 {
                            target[axis]
                        } else {
                            value
                        }
                    })
                };
                (point(current.0, target.0), point(current.1, target.1))
            } else {
                target
            };
            changed |= previous != Some(shown);
            unfinished |= shown != target;
            self.tiling_ui.area_bounds.insert(area.id, shown);
        }
        self.tiling_ui
            .area_bounds
            .retain(|id, _| areas.iter().any(|area| area.id == *id));
        self.tiling_ui.expanding = expanding && unfinished;
        let _ = changed;
        // Corners own junctions before any neighboring resize border can claim
        // the mouse, regardless of the order in which areas are laid out.
        for Area { id, rect } in &areas {
            let (min, max) = self.tiling_ui.area_bounds[id];
            let (surface_min, surface_max) = area_surface(min, max, *rect, area_count);
            let _id = ui.push_id(*id as i32);
            for (corner, high) in [[false, false], [true, false], [false, true], [true, true]]
                .into_iter()
                .enumerate()
            {
                let vertex = [0, 1].map(|axis| {
                    if high[axis] {
                        surface_max[axis]
                    } else {
                        surface_min[axis]
                    }
                });
                let bounds = (
                    [0, 1].map(|axis| {
                        if high[axis] {
                            vertex[axis] - CORNER_SIZE
                        } else {
                            min[axis]
                        }
                    }),
                    [0, 1].map(|axis| {
                        if high[axis] {
                            max[axis]
                        } else {
                            vertex[axis] + CORNER_SIZE
                        }
                    }),
                );
                let inward = [0, 1].map(|axis| {
                    (if high[axis] {
                        vertex[axis] - mouse[axis]
                    } else {
                        mouse[axis] - vertex[axis]
                    })
                    .max(0.0)
                });
                // Large corner triangles include the gaps while leaving the
                // tab picker and close controls outside their input region.
                if !contains(bounds, mouse) || inward[0] + inward[1] > CORNER_SIZE {
                    continue;
                }
                let (hovered, pressed) = hit_zone(ui, &format!("corner_{corner}"), bounds);
                if hovered && self.tiling_ui.gesture.is_none() {
                    self.tiling_ui.corner_cursor = true;
                }
                if pressed && self.tiling_ui.gesture.is_none() {
                    self.tiling_ui.gesture = Some(Gesture::Corner {
                        area: *id,
                        start: mouse,
                        axis: None,
                    });
                }
            }
        }
        let draw = ui.get_window_draw_list();
        let background = ui.style_color(StyleColor::WindowBg);
        let mut window_background = self.settings.window_background_color();
        window_background[3] = self.settings.background_opacity();
        // Partition the window and panel backgrounds instead of layering two
        // translucent fills. This also covers space vacated during expansion.
        let surfaces = areas
            .iter()
            .map(|Area { id, rect }| {
                let (min, max) = self.tiling_ui.area_bounds[id];
                area_surface(min, max, *rect, area_count)
            })
            .collect::<Vec<_>>();
        let end = [origin[0] + size[0], origin[1] + size[1]];
        let mut bands = vec![origin[1], end[1]];
        for (min, max) in &surfaces {
            bands.extend([min[1], max[1]]);
        }
        bands.sort_by(f32::total_cmp);
        bands.dedup();
        let native_draw = unsafe { sys::igGetWindowDrawList() };
        let fill_flags = unsafe { (*native_draw).Flags };
        unsafe {
            (*native_draw).Flags &= !sys::ImDrawListFlags_AntiAliasedFill;
        }
        for band in bands.windows(2) {
            let middle = (band[0] + band[1]) * 0.5;
            let mut spans = surfaces
                .iter()
                .filter(|(min, max)| middle >= min[1] && middle < max[1])
                .map(|(min, max)| (min[0], max[0]))
                .collect::<Vec<_>>();
            spans.sort_by(|left, right| left.0.total_cmp(&right.0));
            let mut x = origin[0];
            for (min, max) in spans {
                if min > x {
                    draw.add_rect([x, band[0]], [min, band[1]], window_background)
                        .filled(true)
                        .build();
                }
                x = x.max(max);
            }
            if x < end[0] {
                draw.add_rect([x, band[0]], [end[0], band[1]], window_background)
                    .filled(true)
                    .build();
            }
        }
        for (min, max) in &surfaces {
            draw.path_clear();
            draw.path_rect(*min, *max, rounding, DrawCornerFlags::ALL);
            // Reuse ImGui's arc vertices so the outside corner wedges share
            // the exact contour of the panel fill, without alpha fringes.
            let contour = unsafe {
                let path = &(*native_draw)._Path;
                std::slice::from_raw_parts(path.Data, path.Size as usize).to_vec()
            };
            draw.path_fill_convex(background);
            let corners = [*min, [max[0], min[1]], *max, [min[0], max[1]]];
            for (corner, arc) in corners
                .into_iter()
                .zip(contour.chunks_exact(contour.len() / 4))
            {
                for segment in arc.windows(2) {
                    draw.add_triangle(corner, segment[0], segment[1], window_background)
                        .filled(true)
                        .build();
                }
            }
        }
        unsafe {
            (*native_draw).Flags = fill_flags;
        }
        let border = ui.style_color(StyleColor::Border);
        let focused = self.focused_area();
        for Area { id, rect } in &areas {
            let (min, max) = self.tiling_ui.area_bounds[id];
            let (surface_min, surface_max) = area_surface(min, max, *rect, area_count);
            let surface_border = if focused == Some(*id) {
                [
                    border[0] + (1.0 - border[0]) * 0.2,
                    border[1] + (1.0 - border[1]) * 0.2,
                    border[2] + (1.0 - border[2]) * 0.2,
                    border[3],
                ]
            } else {
                border
            };
            if area_count > 1 {
                draw.add_rect(surface_min, surface_max, surface_border)
                    .rounding(rounding)
                    .thickness(1.0)
                    .build();
            }
            let _id = ui.push_id(*id as i32);
            for axis in 0..2 {
                let at = rect.max[axis];
                if at == EXTENT {
                    continue;
                }
                let position = if axis == 0 {
                    [max[0] - 4.0, min[1] + 8.0]
                } else {
                    [min[0] + 8.0, max[1] - 4.0]
                };
                let bounds = if axis == 0 {
                    (position, [position[0] + HANDLE_SIZE, max[1] - 8.0])
                } else {
                    (position, [max[0] - 8.0, position[1] + HANDLE_SIZE])
                };
                let (hovered, pressed) = hit_zone(ui, &format!("border_{axis}"), bounds);
                if hovered {
                    ui.set_mouse_cursor(Some(if axis == 0 {
                        MouseCursor::ResizeEW
                    } else {
                        MouseCursor::ResizeNS
                    }));
                }
                if pressed && self.tiling_ui.gesture.is_none() {
                    self.tiling_ui.gesture = Some(Gesture::Border {
                        original: self.tiling.layout.clone(),
                        axis,
                        at,
                        span: [rect.min[axis ^ 1], rect.max[axis ^ 1]],
                    });
                }
            }
            let group = self
                .tiling
                .areas
                .iter()
                .find(|group| group.area == *id)
                .unwrap()
                .clone();
            let mut dock_min = surface_min.map(|value| value + content_padding);
            {
                let bar_min = dock_min;
                let bar_max = [surface_max[0] - content_padding, bar_min[1] + tab_height];
                self.tiling_ui.bar_rects.insert(*id, (bar_min, bar_max));
                draw.add_line(
                    [surface_min[0] + 1.0, bar_max[1]],
                    [surface_max[0] - 1.0, bar_max[1]],
                    border,
                )
                .build();
                dock_min[1] = bar_max[1] + 1.0;
                let tab_center_y = bar_min[1] + tab_height * 0.5;
                let mut tabs_min = bar_min;
                if self.settings.bool("tab_bar_add_button", false) {
                    let add_bounds = (bar_min, [bar_min[0] + tab_height, bar_max[1]]);
                    let (hovered, pressed) = hit_zone(ui, "add_panel", add_bounds);
                    if hovered {
                        draw.add_rect(
                            [bar_min[0] + 2.0, bar_min[1] + 2.0],
                            [bar_min[0] + tab_height - 2.0, bar_max[1] - 2.0],
                            ui.style_color(StyleColor::ButtonHovered),
                        )
                        .rounding(3.0)
                        .filled(true)
                        .build();
                        bedtime_ui::components::tooltip_text(
                            ui,
                            &bedtime_ui::UiFrameContext::for_ui(ui),
                            "Add a window",
                        );
                    }
                    let center = [bar_min[0] + tab_height * 0.5, tab_center_y];
                    let icon_color = ui.style_color(if hovered {
                        StyleColor::Text
                    } else {
                        StyleColor::TextDisabled
                    });
                    draw.add_line(
                        [center[0] - 4.0, center[1]],
                        [center[0] + 4.0, center[1]],
                        icon_color,
                    )
                    .build();
                    draw.add_line(
                        [center[0], center[1] - 4.0],
                        [center[0], center[1] + 4.0],
                        icon_color,
                    )
                    .build();
                    self.draw_window_picker(ui, *id, pressed);
                    tabs_min[0] += tab_height;
                }
                let available = (bar_max[0] - tabs_min[0]).max(1.0);
                let labels = group
                    .tabs
                    .iter()
                    .filter_map(|id| {
                        self.tabs.iter().find(|tab| tab.id == *id).map(|tab| {
                            let title = self.title(tab);
                            let title = title.split("###").next().unwrap().to_owned();
                            let dirty = tab.dirty;
                            let icon = self.settings.tab_icons.then_some(tab.icon).flatten();
                            let _font = (group.preview == Some(*id))
                                .then_some(self.preview_font)
                                .flatten()
                                .map(|font| ui.push_font(font));
                            let width = (ui.calc_text_size(&title)[0]
                                + 37.0
                                + if icon.is_some() {
                                    ui.text_line_height() + 5.0
                                } else {
                                    0.0
                                }
                                + if dirty { 8.0 } else { 0.0 })
                            .clamp(70.0, 210.0);
                            (*id, title, dirty, icon, width)
                        })
                    })
                    .collect::<Vec<_>>();
                let total: f32 = labels.iter().map(|(_, _, _, _, width)| *width).sum();
                let scroll = self.tiling_ui.tab_scroll.entry(*id).or_default();
                if contains((tabs_min, bar_max), mouse) {
                    scroll.offset -= (ui.io().mouse_wheel() + ui.io().mouse_wheel_h()) * 40.0;
                    if self.tiling_ui.dragging_panel.is_some() || ui.drag_drop_payload().is_some() {
                        if mouse[0] < tabs_min[0] + 18.0 {
                            scroll.offset -= 5.0;
                        }
                        if mouse[0] > bar_max[0] - 18.0 {
                            scroll.offset += 5.0;
                        }
                    }
                }
                if scroll.selected != group.selected {
                    if let Some(selected) = group.selected {
                        let mut left = 0.0;
                        for (panel, _, _, _, width) in &labels {
                            if *panel == selected {
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
                    }
                    scroll.selected = group.selected;
                }
                scroll.offset = scroll.offset.clamp(0.0, (total - available).max(0.0));
                let mut x = tabs_min[0] - scroll.offset;
                let _clip = ui.push_clip_rect(tabs_min, bar_max, true);
                for (panel, title, dirty, icon, width) in labels {
                    let left = [x, bar_min[1] + 2.0];
                    let right = [x + width - 1.0, bar_max[1] - 1.0];
                    x += width;
                    self.tiling_ui.tab_positions.insert(panel, (left, right));
                    let clipped = (
                        [left[0].max(tabs_min[0]), left[1]],
                        [right[0].min(bar_max[0]), right[1]],
                    );
                    if clipped.1[0] <= clipped.0[0] {
                        continue;
                    }
                    self.tiling_ui.tab_rects.insert(panel, clipped);
                    let close_min = [right[0] - 21.0, left[1]];
                    let close_max = right;
                    let (close_hovered, close_pressed) = hit_zone(
                        ui,
                        &format!("close_{panel}"),
                        (
                            [close_min[0].max(tabs_min[0]), close_min[1]],
                            [close_max[0].min(bar_max[0]), close_max[1]],
                        ),
                    );
                    let tab_max = [close_min[0].min(clipped.1[0]), clipped.1[1]];
                    let (hovered, pressed) = if tab_max[0] > clipped.0[0] {
                        hit_zone(ui, &format!("tab_{panel}"), (clipped.0, tab_max))
                    } else {
                        (false, false)
                    };
                    let selected = group.selected == Some(panel);
                    draw.add_rect(
                        left,
                        right,
                        ui.style_color(if hovered || close_hovered {
                            StyleColor::TabHovered
                        } else if selected {
                            StyleColor::TabSelected
                        } else {
                            StyleColor::Tab
                        }),
                    )
                    .rounding(3.0)
                    .filled(true)
                    .build();
                    let text = ui.style_color(if selected || hovered {
                        StyleColor::Text
                    } else {
                        StyleColor::TextDisabled
                    });
                    let text_clip = draw.push_clip_rect(
                        [left[0] + 8.0, left[1]],
                        [close_min[0] - 3.0, right[1]],
                        true,
                    );
                    {
                        let _font = (group.preview == Some(panel))
                            .then_some(self.preview_font)
                            .flatten()
                            .map(|font| ui.push_font(font));
                        let mut text_x = left[0] + 8.0;
                        if let Some(icon) = icon {
                            let size = ui.text_line_height();
                            let top = tab_center_y - size * 0.5;
                            draw.add_image(
                                icon,
                                [text_x, top],
                                [text_x + size, top + size],
                                [0.0; 2],
                                [1.0; 2],
                                text,
                            );
                            text_x += size + 5.0;
                        }
                        draw.add_text([text_x, bar_min[1] + 5.0], text, &title);
                    }
                    drop(text_clip);
                    if dirty {
                        draw.add_circle(
                            [close_min[0] - 5.0, tab_center_y],
                            2.0,
                            ui.style_color(StyleColor::Text),
                        )
                        .filled(true)
                        .build();
                    }
                    if selected || hovered || close_hovered {
                        let center = [(close_min[0] + close_max[0]) * 0.5, tab_center_y];
                        if close_hovered {
                            draw.add_circle(center, 8.0, ui.style_color(StyleColor::ButtonHovered))
                                .filled(true)
                                .build();
                        }
                        draw.add_line(
                            [center[0] - 3.0, center[1] - 3.0],
                            [center[0] + 3.0, center[1] + 3.0],
                            text,
                        )
                        .build();
                        draw.add_line(
                            [center[0] + 3.0, center[1] - 3.0],
                            [center[0] - 3.0, center[1] + 3.0],
                            text,
                        )
                        .build();
                    }
                    if close_pressed {
                        self.tiling_ui.actions.push(TilingAction::CloseTab(panel));
                    }
                    if hovered {
                        bedtime_ui::components::tooltip_text(
                            ui,
                            &bedtime_ui::UiFrameContext::for_ui(ui),
                            &title,
                        );
                    }
                    if pressed && self.tiling_ui.gesture.is_none() {
                        if let Some(index) = self.tabs.iter().position(|tab| tab.id == panel) {
                            self.switch_to_tab(index);
                            if ui.is_mouse_double_clicked(MouseButton::Left) {
                                self.requests
                                    .push(WorkbenchAction::KeepPanel(self.tabs[index].entity));
                            }
                        }
                        self.tiling_ui.gesture = Some(Gesture::Tab {
                            panel,
                            start: mouse,
                            dragging: false,
                        });
                    }
                    if ui.is_mouse_released(MouseButton::Middle) && contains(clipped, mouse) {
                        self.tiling_ui.actions.push(TilingAction::CloseTab(panel));
                    }
                }
            }
            let dock = ui.get_id(&format!("bed_area_{id}")).raw();
            self.tiling_ui.docks.insert(*id, dock);
            let _rounding = ui.push_style_var(StyleVar::ChildRounding(0.0));
            let _empty_background = ui.push_style_color(StyleColor::DockingEmptyBg, [0.0; 4]);
            ui.set_cursor_screen_pos(dock_min);
            unsafe {
                // NoTabBar always presents Windows[0]. The model picks that member,
                // without giving native dragging/reordering ownership of our tabs.
                if let Some(selected) = self
                    .tiling
                    .areas
                    .iter()
                    .find(|group| group.area == *id)
                    .and_then(|group| group.selected)
                {
                    let name = CString::new(format!("###bed_tab_{selected}")).unwrap();
                    bed_imgui_dock_node_select_window(
                        sys::igDockBuilderGetNode(dock),
                        sys::igFindWindowByName(name.as_ptr()),
                    );
                }
                sys::igDockSpace(
                    dock,
                    [
                        (surface_max[0] - content_padding - dock_min[0]).max(1.0),
                        (surface_max[1] - content_padding - dock_min[1]).max(1.0),
                    ]
                    .into(),
                    sys::ImGuiDockNodeFlags_NoDocking | sys::ImGuiDockNodeFlags_NoTabBar,
                    std::ptr::null(),
                );
                bed_imgui_dock_node_clear_background(sys::igDockBuilderGetNode(dock));
            }
        }
        if self.tiling_ui.gesture.is_some() && self.tiling_ui.preview_target.is_none() {
            self.tiling_ui.preview_target = preview
                .map(|rect| area_surface(screen(rect.min), screen(rect.max), rect, preview_count))
                .or_else(|| preview_line.map(|(first, second)| (screen(first), screen(second))))
                .map(|bounds| (bounds, preview_count));
        }
        if self.tiling_ui.gesture.is_none() {
            self.tiling_ui.preview_target = None;
        }
    }

    fn draw_placement_preview(&mut self, ui: &Ui) {
        let border = ui.style_color(StyleColor::Border);
        let draw = ui.get_foreground_draw_list();
        let text = ui.style_color(StyleColor::Text);
        let preview_border =
            [0, 1, 2, 3].map(|axis| border[axis] + (text[axis] - border[axis]) * 0.75);
        let preview_fill = [
            preview_border[0],
            preview_border[1],
            preview_border[2],
            preview_border[3] * 0.14,
        ];
        let preview_rounding = if self
            .tiling_ui
            .preview_target
            .is_some_and(|(_, count)| count == 1)
        {
            0.0
        } else {
            AREA_ROUNDING
        };
        // The primary indicator can be a rectangle or a collapsed rectangle
        // representing a split/insertion line. Retarget its current visual
        // position so crossing between targets never restarts the animation.
        let target = self.tiling_ui.preview_target.map(|(bounds, _)| bounds);
        let blend = if self.settings.bool("ui_animations", true) {
            // An exponential step settles about 95% of the distance in 0.12s.
            1.0 - (-ui.io().delta_time() / 0.04).exp()
        } else {
            1.0
        };
        let interpolate = |current: Bounds, target: Bounds| {
            let point = |current: [f32; 2], target: [f32; 2]| {
                [0, 1].map(|axis| {
                    let value = current[axis] + (target[axis] - current[axis]) * blend;
                    if (target[axis] - value).abs() < 0.25 {
                        target[axis]
                    } else {
                        value
                    }
                })
            };
            (point(current.0, target.0), point(current.1, target.1))
        };
        self.tiling_ui.preview_bounds = target
            .map(|target| interpolate(self.tiling_ui.preview_bounds.unwrap_or(target), target));
        if let Some((min, max)) = self.tiling_ui.preview_bounds {
            if (max[0] - min[0]).abs() < 0.5 || (max[1] - min[1]).abs() < 0.5 {
                draw.add_line(min, max, preview_border)
                    .thickness(2.0)
                    .build();
            } else {
                draw.add_rect(min, max, preview_fill)
                    .rounding(preview_rounding)
                    .filled(true)
                    .build();
                draw.add_rect(min, max, preview_border)
                    .rounding(preview_rounding)
                    .thickness(2.0)
                    .build();
            }
        }
        drop(draw);
        if let Some(panel) = self.tiling_ui.dragging_panel {
            let title = self
                .tabs
                .iter()
                .find(|tab| tab.id == panel)
                .map(|tab| self.title(tab))
                .unwrap();
            let title = title.split("###").next().unwrap();
            bedtime_api::workbench::draw_panel_drag_label(ui, title);
        }
    }

    fn draw_window_picker(&mut self, ui: &Ui, area: u32, open: bool) {
        if open {
            ui.open_popup("Add window");
        }

        if let Some(_popup) = ui.begin_popup("Add window") {
            if ui.is_key_pressed(Key::Escape) {
                ui.close_current_popup();
            }
            for panel in &self.definitions {
                if !panel.allow_empty {
                    continue;
                }
                if ui.menu_item(&panel.label) {
                    self.tiling_ui.actions.push(TilingAction::AddPanel {
                        area,
                        kind: panel.id.clone(),
                    });
                }
            }
            if self
                .tiling
                .areas
                .iter()
                .find(|group| group.area == area)
                .expect("picker area exists")
                .tabs
                .is_empty()
            {
                ui.separator();
                if ui.menu_item_enabled_selected_no_shortcut(
                    "Close Panel",
                    false,
                    self.tiling.layout.areas.len() > 1,
                ) {
                    self.tiling_ui.actions.push(TilingAction::CloseArea(area));
                }
            }
        }
    }

    pub(super) fn draw_empty_areas(&mut self, ui: &Ui) {
        let empty = self
            .tiling
            .areas
            .iter()
            .filter(|group| group.tabs.is_empty())
            .map(|group| group.area)
            .collect::<Vec<_>>();
        for area in empty {
            ui.set_next_window_viewport(ui.main_viewport().id());
            unsafe {
                sys::igSetNextWindowDockID(self.tiling_ui.docks[&area], sys::ImGuiCond_Always);
            }
            ui.window(format!("###bed_empty_area_{area}"))
                .flags(
                    WindowFlags::NO_COLLAPSE
                        | WindowFlags::NO_MOVE
                        | WindowFlags::NO_BACKGROUND
                        | WindowFlags::NO_TITLE_BAR
                        | WindowFlags::NO_FOCUS_ON_APPEARING
                        | WindowFlags::NO_SAVED_SETTINGS,
                )
                .build(|| {
                    if ui.is_window_focused_with_flags(FocusedFlags::ROOT_AND_CHILD_WINDOWS)
                        && (self.focused.is_some() || self.focused_area() != Some(area))
                    {
                        self.focus_empty_area(area);
                    }
                    let picker = ui.is_mouse_released(MouseButton::Right)
                        && !self.tiling_ui.suppress_right_picker
                        && !ui.is_mouse_down(MouseButton::Left)
                        && !ui.is_mouse_released(MouseButton::Left)
                        && ui.is_window_hovered();
                    if picker {
                        self.focus_empty_area(area);
                    }
                    self.draw_window_picker(ui, area, picker);
                });
        }
        if ui.is_mouse_released(MouseButton::Right) {
            self.tiling_ui.suppress_right_picker = false;
        }
    }

    pub(super) fn finish_tiling(&mut self, ui: &Ui) {
        self.draw_placement_preview(ui);
        if self.tiling_ui.corner_cursor {
            ui.set_mouse_cursor(None);
            let [x, y] = ui.io().mouse_pos();
            let draw = ui.get_foreground_draw_list();
            for (color, thickness) in [
                (ui.style_color(StyleColor::WindowBg), 4.0),
                (ui.style_color(StyleColor::Text), 1.5),
            ] {
                draw.add_line([x - 10.0, y], [x + 10.0, y], color)
                    .thickness(thickness)
                    .build();
                draw.add_line([x, y - 10.0], [x, y + 10.0], color)
                    .thickness(thickness)
                    .build();
            }
        }
        let live = &self.tiling.layout.areas;
        self.tiling_ui.docks.retain(|id, dock| {
            let keep = live.iter().any(|area| area.id == *id);
            if !keep {
                unsafe {
                    sys::igDockBuilderRemoveNode(*dock);
                }
            }
            keep
        });
        self.tiling_ui
            .tab_scroll
            .retain(|id, _| live.iter().any(|area| area.id == *id));
    }

    pub(super) fn tiling_area_at(&self, position: [f32; 2], viewport: u32) -> Option<u32> {
        if self.tiling_ui.main_viewport == 0 || self.tiling_ui.main_viewport != viewport {
            return None;
        }
        if !self.tiling_ui.area_bounds.is_empty() {
            return self
                .current_tiling()
                .layout
                .areas
                .iter()
                .find(|area| {
                    self.tiling_ui
                        .area_bounds
                        .get(&area.id)
                        .is_some_and(|bounds| contains(*bounds, position))
                })
                .map(|area| area.id);
        }
        let point = [0, 1].map(|axis| {
            ((position[axis] - self.tiling_ui.origin[axis]) * EXTENT as f32
                / self.tiling_ui.size[axis])
                .floor() as i32
        });
        self.current_tiling()
            .layout
            .areas
            .iter()
            .find(|area| area.rect.contains(point))
            .map(|area| area.id)
    }
}
