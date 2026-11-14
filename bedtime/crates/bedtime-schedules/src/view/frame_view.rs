//! Frame-wide views: the *lanes* timeline (components as rows, systems as columns in execution
//! order) and the *arc diagram* (systems on one axis, ordering arcs above, data hand-offs below).

use std::{
    collections::{HashMap, HashSet},
    hash::{Hash, Hasher},
    sync::Arc,
};

use bedtime_ui::egui::{
    self, Align2, Color32, CornerRadius, FontId, Galley, Painter, Pos2, Rect, Sense, Shape, Stroke,
    StrokeKind, TextStyle, epaint::QuadraticBezierShape, pos2, vec2,
};

use super::{
    FrameCache, Mode, ViewState, comp_name, legend, scene_text::SceneText, search_matches, style,
};
use crate::{
    frame::{self, BandKind, FrameOptions, FrameSequence, MarkKind},
    snapshot::{AppSnapshot, CompIx, NodeRef, SysIx},
};

const COL_W: f32 = 22.0;
const LANE_H: f32 = 18.0;
const LABEL_W: f32 = 240.0;
const BAND_H: f32 = 18.0;
const NAME_H: f32 = 170.0;
const NAME_CHARS: usize = 28;
const MARK: f32 = 5.0;

/// Options of the frame views that live in [`ViewState`].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FrameViewOptions {
    pub include_startup: bool,
    pub include_transitions: bool,
    pub include_other: bool,
    pub writes_only: bool,
    pub show_resources: bool,
    pub show_components: bool,
    pub only_matching: bool,
    /// Hide the systems that touch none of the shown lanes (Lanes and Arcs only).
    pub touching_only: bool,
    pub lane_sort: LaneSort,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LaneSort {
    FirstAccess,
    Activity,
    Name,
}

impl Default for FrameViewOptions {
    fn default() -> Self {
        Self {
            include_startup: false,
            include_transitions: true,
            include_other: false,
            writes_only: false,
            show_resources: true,
            show_components: true,
            // A search narrows the frame by default: rows to the lanes it matches, columns to
            // the systems that touch them. With no search text both are no-ops.
            only_matching: true,
            touching_only: true,
            lane_sort: LaneSort::FirstAccess,
        }
    }
}

enum Action {
    SelectColumn(usize),
    FocusLane(CompIx),
    Hover(Option<NodeRef>),
}

pub(super) fn ensure_frame(snapshot: &AppSnapshot, view: &mut ViewState) {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    // `touching_only` drops columns, which would silently thin out the timeline and the flame
    // chart too, so it only applies to the views whose lanes it follows from.
    let lane_view = matches!(view.mode, Mode::Lanes | Mode::Arcs);
    snapshot.generation.hash(&mut hasher);
    view.frame_opts.hash(&mut hasher);
    lane_view.hash(&mut hasher);
    (view.show_internal, view.show_self, view.show_sync).hash(&mut hasher);
    if view.frame_opts.only_matching {
        view.search.to_lowercase().hash(&mut hasher);
    }
    let key = hasher.finish();
    if view.frame_cache.as_ref().is_some_and(|c| c.key == key) {
        return;
    }
    let (show_internal, show_self, show_sync) =
        (view.show_internal, view.show_self, view.show_sync);
    let include = move |s: &crate::snapshot::SystemSnapshot| {
        (show_internal || !s.flags.internal)
            && (show_self || !s.flags.is_self)
            && (show_sync || !s.flags.apply_deferred)
    };
    let options = FrameOptions {
        include_startup: view.frame_opts.include_startup,
        include_transitions: view.frame_opts.include_transitions,
        include_other: view.frame_opts.include_other,
        include_system: &include,
        require_write: view.frame_opts.writes_only,
    };
    let mut seq = frame::frame_sequence(snapshot, &options);

    // Lane filtering and ordering.
    let search = view.search.to_lowercase();
    let mut activity: HashMap<CompIx, usize> = HashMap::new();
    for m in &seq.marks {
        *activity.entry(m.comp).or_default() += 1;
    }
    seq.lanes.retain(|c| {
        let comp = &snapshot.components[*c as usize];
        (comp.is_resource && view.frame_opts.show_resources
            || !comp.is_resource && view.frame_opts.show_components)
            && (!view.frame_opts.only_matching
                || search.is_empty()
                || comp.short_name.to_lowercase().contains(&search))
    });
    match view.frame_opts.lane_sort {
        LaneSort::FirstAccess => {}
        LaneSort::Activity => seq.lanes.sort_by_key(|c| std::cmp::Reverse(activity[c])),
        LaneSort::Name => seq
            .lanes
            .sort_by_key(|c| snapshot.components[*c as usize].short_name.clone()),
    }
    if lane_view && view.frame_opts.touching_only {
        seq.retain_columns_touching_lanes();
    }
    view.frame_cache = Some(FrameCache { key, seq });
}

struct Columns {
    x0: f32,
    count: usize,
}

impl Columns {
    fn left(&self, column: usize) -> f32 {
        self.x0 + column as f32 * COL_W
    }
    fn center(&self, column: usize) -> f32 {
        self.left(column) + COL_W * 0.5
    }
    fn width(&self) -> f32 {
        self.count as f32 * COL_W
    }
}

/// How far `Fit` may magnify a frame that is smaller than the canvas.
const MAX_FIT_ZOOM: f32 = 1.8;

/// The scene rect to fit `bounds` into `canvas` with.
///
/// [`egui::Scene`] scales whatever rect it is given up to the canvas, so a frame narrowed down to
/// a handful of columns would be blown up to the 4× zoom limit. Padding the bounds out keeps the
/// fit at [`MAX_FIT_ZOOM`] or below, with the content centred.
fn fit_bounds(bounds: Rect, canvas: Rect) -> Rect {
    Rect::from_center_size(
        bounds.center(),
        bounds.size().max(canvas.size() / MAX_FIT_ZOOM),
    )
}

/// Shortens a label to `max` characters with an ellipsis.
pub(super) fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        let mut out: String = text.chars().take(max - 1).collect();
        out.push('…');
        out
    }
}

pub(super) fn band_color(kind: BandKind, label: &str, alpha: u8) -> Color32 {
    let (s, v) = match kind {
        BandKind::Main => (0.45, 0.6),
        BandKind::Fixed => (0.55, 0.75),
        BandKind::Transition => (0.35, 0.8),
        BandKind::Startup => (0.2, 0.6),
        BandKind::Other => (0.1, 0.5),
    };
    style::hue_color(label, s, v, alpha)
}

/// Draws the schedule bands and rotated system names; returns the y where content starts.
#[allow(clippy::too_many_arguments)]
fn draw_header(
    painter: &Painter,
    text: &SceneText,
    snapshot: &AppSnapshot,
    seq: &FrameSequence,
    cols: &Columns,
    top: f32,
    font: &FontId,
    clip: Rect,
    hovered_col: Option<usize>,
    selected_col: Option<usize>,
) -> f32 {
    let bands_top = top;
    for band in &seq.bands {
        if band.first == band.last {
            continue;
        }
        let x0 = cols.left(band.first);
        let x1 = cols.left(band.last);
        let y0 = bands_top + band.depth as f32 * BAND_H;
        let rect = Rect::from_min_max(pos2(x0, y0), pos2(x1, y0 + BAND_H - 2.0));
        if !rect.intersects(clip) {
            continue;
        }
        painter.rect_filled(
            rect,
            CornerRadius::same(3),
            band_color(band.kind, &band.label, 70),
        );
        let label = match band.kind {
            BandKind::Fixed => format!("{} (fixed)", band.label),
            BandKind::Transition => format!("{} (on transition)", band.label),
            _ => band.label.clone(),
        };
        let galley = text.layout(
            label,
            &FontId::proportional(font.size * 0.85),
            style::node_text(),
        );
        if text.size(&galley).x < rect.width() - 6.0 {
            text.galley(pos2(x0 + 4.0, y0 + 2.0), galley, style::node_text());
        }
    }
    let names_top = bands_top + 2.0 * BAND_H;
    let names_bottom = names_top + NAME_H;
    let angle = -std::f32::consts::FRAC_PI_3;
    for fs in &seq.systems {
        let x = cols.left(fs.column);
        if x + COL_W < clip.min.x - NAME_H || x > clip.max.x + 10.0 {
            continue;
        }
        let system = &snapshot.schedules[fs.schedule].systems[fs.system as usize];
        let highlighted = hovered_col == Some(fs.column) || selected_col == Some(fs.column);
        let color = if highlighted {
            style::selected()
        } else if fs.wave % 2 == 0 {
            style::node_text()
        } else {
            style::node_text_alt()
        };
        let label = if system.flags.apply_deferred {
            "⏸".to_string()
        } else {
            truncate(&system.short_name, NAME_CHARS)
        };
        let galley: Arc<Galley> = text.layout(label, font, color);
        // Anchor the rotated baseline at the bottom-left of the column; text rises to the right.
        let pos = pos2(x + COL_W * 0.5 - 4.0, names_bottom - 4.0);
        text.rotated(pos, galley, color, angle);
    }
    // Wave separators: a faint tick where the topological level changes inside a schedule.
    for w in seq.systems.windows(2) {
        if w[0].schedule == w[1].schedule && w[0].wave != w[1].wave {
            let x = cols.left(w[1].column);
            painter.line_segment(
                [pos2(x, names_bottom - 12.0), pos2(x, names_bottom)],
                Stroke::new(1.0, style::rule()),
            );
        }
    }
    names_bottom
}

pub(super) fn mark_shape(
    painter: &Painter,
    center: Pos2,
    kind: MarkKind,
    color: Color32,
    strong: bool,
) {
    let s = if strong { MARK + 1.5 } else { MARK };
    match kind {
        MarkKind::Write | MarkKind::ConditionWrite => {
            painter.rect_filled(
                Rect::from_center_size(center, vec2(2.0 * s, 2.0 * s)),
                CornerRadius::same(2),
                color,
            );
            if kind == MarkKind::ConditionWrite {
                painter.rect_stroke(
                    Rect::from_center_size(center, vec2(2.0 * s + 3.0, 2.0 * s + 3.0)),
                    CornerRadius::same(2),
                    Stroke::new(1.0, style::warn()),
                    StrokeKind::Middle,
                );
            }
        }
        MarkKind::Read => {
            painter.circle_stroke(center, s, Stroke::new(1.6, color));
        }
        MarkKind::ConditionRead => {
            painter.circle_stroke(center, s, Stroke::new(1.6, color));
            painter.circle_stroke(center, s + 2.5, Stroke::new(1.0, style::warn()));
        }
        MarkKind::Filter => {
            painter.add(Shape::convex_polygon(
                vec![
                    center + vec2(0.0, -s),
                    center + vec2(s, s * 0.8),
                    center + vec2(-s, s * 0.8),
                ],
                Color32::TRANSPARENT,
                Stroke::new(1.2, style::filter()),
            ));
        }
        MarkKind::Archetypal => {
            painter.rect_stroke(
                Rect::from_center_size(center, vec2(2.0 * s, 2.0 * s)),
                CornerRadius::ZERO,
                Stroke::new(1.2, style::filter()),
                StrokeKind::Middle,
            );
        }
    }
}

fn mark_color(kind: MarkKind, lane_color: Color32) -> Color32 {
    match kind {
        MarkKind::Write | MarkKind::ConditionWrite => lane_color,
        MarkKind::Read | MarkKind::ConditionRead => style::read(),
        _ => style::filter(),
    }
}

fn selected_column(seq: &FrameSequence, snapshot: &AppSnapshot, view: &ViewState) -> Option<usize> {
    let NodeRef::System(sys) = view.selected? else {
        return None;
    };
    let label = view.schedule.as_deref()?;
    seq.systems
        .iter()
        .find(|fs| fs.system == sys && snapshot.schedules[fs.schedule].label == label)
        .map(|fs| fs.column)
}

fn apply_actions(actions: Vec<Action>, snapshot: &AppSnapshot, view: &mut ViewState) {
    for action in actions {
        match action {
            Action::SelectColumn(col) => {
                let (label, sys) = {
                    let seq = &view.frame_cache.as_ref().unwrap().seq;
                    let fs = seq.systems[col];
                    (snapshot.schedules[fs.schedule].label.clone(), fs.system)
                };
                view.select_in_schedule(label, NodeRef::System(sys));
            }
            Action::FocusLane(comp) => {
                view.focus_component = if view.focus_component == Some(comp) {
                    None
                } else {
                    Some(comp)
                };
            }
            Action::Hover(node) => view.hovered = node,
        }
    }
}

// ------------------------------------------------------------------------------------------------
// Lanes
// ------------------------------------------------------------------------------------------------

pub fn show_lanes(ui: &mut egui::Ui, snapshot: &AppSnapshot, view: &mut ViewState) {
    ensure_frame(snapshot, view);
    let font = TextStyle::Body.resolve(ui.style());
    let Some(cache) = view.frame_cache.take() else {
        return;
    };
    let seq = &cache.seq;
    let cols = Columns {
        x0: 0.0,
        count: seq.systems.len(),
    };
    let content_h = 2.0 * BAND_H + NAME_H + seq.lanes.len() as f32 * LANE_H + 20.0;
    let bounds = Rect::from_min_max(pos2(-LABEL_W, -10.0), pos2(cols.width() + 40.0, content_h));
    let canvas = ui.max_rect();
    if view.frame_fit_requested {
        view.frame_scene_rect = fit_bounds(bounds, canvas);
        view.frame_fit_requested = false;
    }
    let mut scene_rect = view.frame_scene_rect;
    let mut actions = Vec::new();
    egui::Scene::new()
        .zoom_range(0.05..=4.0)
        .show(ui, &mut scene_rect, |ui| {
            draw_lanes(ui, snapshot, view, seq, &cols, &font, &mut actions);
        });
    view.frame_scene_rect = scene_rect;
    view.frame_cache = Some(cache);
    apply_actions(actions, snapshot, view);
    legend::overlay(ui, Mode::Lanes, canvas, &mut view.show_legend);
}

fn draw_lanes(
    ui: &mut egui::Ui,
    snapshot: &AppSnapshot,
    view: &ViewState,
    seq: &FrameSequence,
    cols: &Columns,
    font: &FontId,
    actions: &mut Vec<Action>,
) {
    let clip = ui.clip_rect();
    let painter = ui.painter().clone();
    let names = SceneText::new(ui);
    let lanes_top = 2.0 * BAND_H + NAME_H;
    let lanes_bottom = lanes_top + seq.lanes.len() as f32 * LANE_H;
    let lane_index: HashMap<CompIx, usize> =
        seq.lanes.iter().enumerate().map(|(i, c)| (*c, i)).collect();

    // --- Interaction pass ---
    let mut hovered_col: Option<usize> = None;
    let mut hovered_lane: Option<usize> = None;
    for fs in &seq.systems {
        let rect = Rect::from_min_max(
            pos2(cols.left(fs.column), 2.0 * BAND_H),
            pos2(cols.left(fs.column) + COL_W, lanes_bottom),
        );
        if !rect.intersects(clip) {
            continue;
        }
        let resp = ui.interact(rect, ui.id().with(("lane_col", fs.column)), Sense::click());
        if resp.hovered() {
            hovered_col = Some(fs.column);
        }
        if resp.clicked() {
            actions.push(Action::SelectColumn(fs.column));
        }
    }
    for (i, comp) in seq.lanes.iter().enumerate() {
        let y = lanes_top + i as f32 * LANE_H;
        let rect = Rect::from_min_max(pos2(-LABEL_W, y), pos2(0.0, y + LANE_H));
        if !rect.intersects(clip) {
            continue;
        }
        let resp = ui.interact(rect, ui.id().with(("lane_row", *comp)), Sense::click());
        if resp.hovered() {
            hovered_lane = Some(i);
        }
        if resp.clicked() {
            actions.push(Action::FocusLane(*comp));
        }
    }
    let selected_col = selected_column(seq, snapshot, view);
    let focus_col = hovered_col.or(selected_col);
    let focus_lane = hovered_lane.or_else(|| {
        view.focus_component
            .and_then(|c| lane_index.get(&c).copied())
    });
    let hovered_node = hovered_col.map(|c| NodeRef::System(seq.systems[c].system));
    actions.push(Action::Hover(hovered_node));

    // --- Background: lanes and focus bands ---
    for (i, comp) in seq.lanes.iter().enumerate() {
        let y = lanes_top + i as f32 * LANE_H;
        let rect = Rect::from_min_max(pos2(-LABEL_W, y), pos2(cols.width(), y + LANE_H));
        if !rect.intersects(clip) {
            continue;
        }
        let is_focus = focus_lane == Some(i);
        let bg = if is_focus {
            style::selected().gamma_multiply(0.12)
        } else if i % 2 == 0 {
            style::node_text().gamma_multiply(0.03)
        } else {
            Color32::TRANSPARENT
        };
        if bg != Color32::TRANSPARENT {
            painter.rect_filled(rect, CornerRadius::ZERO, bg);
        }
        let c = &snapshot.components[*comp as usize];
        let name_color = if c.is_resource {
            style::read()
        } else {
            style::node_text()
        };
        let label = if c.is_resource {
            format!("{} ⚙", c.short_name)
        } else {
            c.short_name.clone()
        };
        let galley = names.layout(label, font, name_color);
        let size = names.size(&galley);
        let w = size.x.min(LABEL_W - 12.0);
        names.galley(
            pos2(-w - 8.0, y + (LANE_H - size.y) / 2.0),
            galley,
            name_color,
        );
    }
    // Schedule band separators through the lanes.
    for band in &seq.bands {
        if band.first == band.last || band.depth > 0 {
            continue;
        }
        let x = cols.left(band.first);
        painter.line_segment(
            [pos2(x, lanes_top), pos2(x, lanes_bottom)],
            Stroke::new(1.0, style::rule()),
        );
    }
    if let Some(col) = focus_col {
        let rect = Rect::from_min_max(
            pos2(cols.left(col), 2.0 * BAND_H),
            pos2(cols.left(col) + COL_W, lanes_bottom),
        );
        painter.rect_filled(
            rect,
            CornerRadius::ZERO,
            style::selected().gamma_multiply(0.12),
        );
    }

    // --- Hand-offs ---
    for h in &seq.handoffs {
        let Some(&lane) = lane_index.get(&h.comp) else {
            continue;
        };
        let y = lanes_top + lane as f32 * LANE_H + LANE_H / 2.0;
        let strong =
            focus_lane == Some(lane) || focus_col.is_some_and(|c| c == h.from || h.to.contains(&c));
        let dim = focus_col.is_some() || focus_lane.is_some();
        let name = &snapshot.components[h.comp as usize].name;
        let color = if strong {
            style::hue_color(name, 0.7, 1.0, 255)
        } else {
            style::hue_color(name, 0.5, 0.9, if dim { 50 } else { 120 })
        };
        let x0 = cols.center(h.from);
        let x1 = cols.center(*h.to.iter().max().unwrap_or(&h.from));
        if x1 < clip.min.x || x0 > clip.max.x || y < clip.min.y || y > clip.max.y {
            continue;
        }
        painter.line_segment(
            [pos2(x0, y), pos2(x1, y)],
            Stroke::new(if strong { 2.2 } else { 1.2 }, color),
        );
        for t in &h.to {
            let x = cols.center(*t);
            painter.add(Shape::convex_polygon(
                vec![
                    pos2(x - MARK - 2.0, y - 4.0),
                    pos2(x - MARK - 2.0, y + 4.0),
                    pos2(x - MARK + 2.0, y),
                ],
                color,
                Stroke::NONE,
            ));
        }
    }

    // --- Marks ---
    for m in &seq.marks {
        let Some(&lane) = lane_index.get(&m.comp) else {
            continue;
        };
        let center = pos2(
            cols.center(m.column),
            lanes_top + lane as f32 * LANE_H + LANE_H / 2.0,
        );
        if !clip.expand(10.0).contains(center) {
            continue;
        }
        let name = &snapshot.components[m.comp as usize].name;
        let strong = focus_col == Some(m.column) || focus_lane == Some(lane);
        let lane_color = style::hue_color(name, 0.7, 1.0, 255);
        mark_shape(
            &painter,
            center,
            m.kind,
            mark_color(m.kind, lane_color),
            strong,
        );
    }

    draw_header(
        &painter,
        &names,
        snapshot,
        seq,
        cols,
        0.0,
        font,
        clip,
        hovered_col,
        selected_col,
    );

    // --- Tooltip ---
    if let Some(col) = hovered_col {
        let rect = Rect::from_min_max(
            pos2(cols.left(col), 2.0 * BAND_H),
            pos2(cols.left(col) + COL_W, lanes_bottom),
        );
        let resp = ui.interact(rect, ui.id().with(("lane_col_tip", col)), Sense::hover());
        resp.on_hover_ui_at_pointer(|ui| column_tooltip(ui, snapshot, seq, col));
    }
    if let Some(lane) = hovered_lane {
        let comp = seq.lanes[lane];
        let y = lanes_top + lane as f32 * LANE_H;
        let rect = Rect::from_min_max(pos2(-LABEL_W, y), pos2(0.0, y + LANE_H));
        let resp = ui.interact(rect, ui.id().with(("lane_row_tip", comp)), Sense::hover());
        resp.on_hover_ui_at_pointer(|ui| lane_tooltip(ui, snapshot, seq, comp));
    }
}

fn column_tooltip(ui: &mut egui::Ui, snapshot: &AppSnapshot, seq: &FrameSequence, col: usize) {
    ui.set_max_width(420.0);
    let fs = seq.systems[col];
    let schedule = &snapshot.schedules[fs.schedule];
    let system = &schedule.systems[fs.system as usize];
    ui.monospace(&system.name);
    ui.colored_label(
        style::filter(),
        format!("{} · wave {}", schedule.label, fs.wave),
    );
    let mut writes = Vec::new();
    let mut reads = Vec::new();
    for m in seq.marks.iter().filter(|m| m.column == col) {
        let name = comp_name(snapshot, m.comp);
        match m.kind {
            MarkKind::Write | MarkKind::ConditionWrite => writes.push(name),
            MarkKind::Read | MarkKind::ConditionRead => reads.push(name),
            _ => {}
        }
    }
    if !writes.is_empty() {
        ui.colored_label(style::write(), format!("writes: {}", writes.join(", ")));
    }
    if !reads.is_empty() {
        ui.colored_label(style::read(), format!("reads: {}", reads.join(", ")));
    }
    ui.colored_label(style::filter(), "click: details");
}

fn lane_tooltip(ui: &mut egui::Ui, snapshot: &AppSnapshot, seq: &FrameSequence, comp: CompIx) {
    ui.set_max_width(420.0);
    let c = &snapshot.components[comp as usize];
    ui.monospace(&c.name);
    let writers = seq
        .marks
        .iter()
        .filter(|m| m.comp == comp && m.kind.is_write())
        .count();
    let readers = seq
        .marks
        .iter()
        .filter(|m| m.comp == comp && m.kind.is_read())
        .count();
    ui.colored_label(
        style::filter(),
        format!(
            "{} · {writers} writers · {readers} readers",
            if c.is_resource {
                "resource"
            } else {
                "component"
            }
        ),
    );
    ui.colored_label(style::filter(), "click: focus this lane");
}

// ------------------------------------------------------------------------------------------------
// Arc diagram
// ------------------------------------------------------------------------------------------------

pub fn show_arcs(ui: &mut egui::Ui, snapshot: &AppSnapshot, view: &mut ViewState) {
    ensure_frame(snapshot, view);
    let font = TextStyle::Body.resolve(ui.style());
    let Some(cache) = view.frame_cache.take() else {
        return;
    };
    let seq = &cache.seq;
    let cols = Columns {
        x0: 0.0,
        count: seq.systems.len(),
    };
    let max_span = seq.systems.len() as f32 * COL_W;
    let arc_h = (max_span * 0.18).clamp(80.0, 400.0);
    let axis_y = 2.0 * BAND_H + NAME_H + arc_h + 10.0;
    let bounds = Rect::from_min_max(
        pos2(-20.0, -10.0),
        pos2(cols.width() + 40.0, axis_y + arc_h + 40.0),
    );
    let canvas = ui.max_rect();
    if view.frame_fit_requested {
        view.frame_scene_rect = fit_bounds(bounds, canvas);
        view.frame_fit_requested = false;
    }
    let mut scene_rect = view.frame_scene_rect;
    let mut actions = Vec::new();
    egui::Scene::new()
        .zoom_range(0.05..=4.0)
        .show(ui, &mut scene_rect, |ui| {
            draw_arcs(
                ui,
                snapshot,
                view,
                seq,
                &cols,
                axis_y,
                arc_h,
                &font,
                &mut actions,
            );
        });
    view.frame_scene_rect = scene_rect;
    view.frame_cache = Some(cache);
    apply_actions(actions, snapshot, view);
    legend::overlay(ui, Mode::Arcs, canvas, &mut view.show_legend);
}

#[allow(clippy::too_many_arguments)]
fn arc(
    painter: &Painter,
    x0: f32,
    x1: f32,
    y: f32,
    up: bool,
    height_scale: f32,
    color: Color32,
    width: f32,
) {
    let span = (x1 - x0).abs();
    let h = (span * 0.35 * height_scale + 14.0).min(420.0);
    let ctrl = pos2((x0 + x1) / 2.0, if up { y - h } else { y + h });
    painter.add(QuadraticBezierShape::from_points_stroke(
        [pos2(x0, y), ctrl, pos2(x1, y)],
        false,
        Color32::TRANSPARENT,
        Stroke::new(width, color),
    ));
    // Arrow head at the destination.
    let dir = (pos2(x1, y) - ctrl).normalized();
    let normal = vec2(-dir.y, dir.x);
    let tip = pos2(x1, y);
    let base = tip - dir * (5.0 + width);
    painter.add(Shape::convex_polygon(
        vec![tip, base + normal * 3.5, base - normal * 3.5],
        color,
        Stroke::NONE,
    ));
}

#[allow(clippy::too_many_arguments)]
fn draw_arcs(
    ui: &mut egui::Ui,
    snapshot: &AppSnapshot,
    view: &ViewState,
    seq: &FrameSequence,
    cols: &Columns,
    axis_y: f32,
    arc_h: f32,
    font: &FontId,
    actions: &mut Vec<Action>,
) {
    let clip = ui.clip_rect();
    let painter = ui.painter().clone();
    let text = SceneText::new(ui);
    let top = 2.0 * BAND_H;
    let bottom = axis_y + arc_h + 30.0;

    let mut hovered_col: Option<usize> = None;
    for fs in &seq.systems {
        let rect = Rect::from_min_max(
            pos2(cols.left(fs.column), top),
            pos2(cols.left(fs.column) + COL_W, bottom),
        );
        if !rect.intersects(clip) {
            continue;
        }
        let resp = ui.interact(rect, ui.id().with(("arc_col", fs.column)), Sense::click());
        if resp.hovered() {
            hovered_col = Some(fs.column);
        }
        if resp.clicked() {
            actions.push(Action::SelectColumn(fs.column));
        }
    }
    let selected_col = selected_column(seq, snapshot, view);
    let focus_col = hovered_col.or(selected_col);
    actions.push(Action::Hover(
        hovered_col.map(|c| NodeRef::System(seq.systems[c].system)),
    ));
    let height_scale = arc_h / 400.0;

    // Column index by (schedule, system) for ordering edges.
    let mut col_of: HashMap<(usize, SysIx), usize> = HashMap::new();
    for fs in &seq.systems {
        col_of.insert((fs.schedule, fs.system), fs.column);
    }

    // Axis and schedule bands.
    painter.line_segment(
        [pos2(0.0, axis_y), pos2(cols.width(), axis_y)],
        Stroke::new(1.0, style::rule()),
    );
    for band in &seq.bands {
        if band.first == band.last || band.depth > 0 {
            continue;
        }
        let x = cols.left(band.first);
        painter.line_segment(
            [pos2(x, top), pos2(x, bottom)],
            Stroke::new(1.0, style::rule()),
        );
    }
    if let Some(col) = focus_col {
        let rect = Rect::from_min_max(
            pos2(cols.left(col), top),
            pos2(cols.left(col) + COL_W, bottom),
        );
        painter.rect_filled(
            rect,
            CornerRadius::ZERO,
            style::selected().gamma_multiply(0.12),
        );
    }

    // Ordering arcs (above).
    let mut highlighted: Vec<(f32, f32, bool)> = Vec::new();
    for (si, schedule) in snapshot.schedules.iter().enumerate() {
        for e in &schedule.flat_edges {
            let (Some(&a), Some(&b)) = (col_of.get(&(si, e.from)), col_of.get(&(si, e.to))) else {
                continue;
            };
            let (x0, x1) = (cols.center(a), cols.center(b));
            if x1.max(x0) < clip.min.x || x0.min(x1) > clip.max.x {
                continue;
            }
            if focus_col.is_some_and(|c| c == a || c == b) {
                highlighted.push((x0, x1, focus_col == Some(b)));
                continue;
            }
            let color = if focus_col.is_some() {
                style::edge_dim()
            } else {
                style::filter().gamma_multiply(0.45)
            };
            arc(
                &painter,
                x0,
                x1,
                axis_y - 4.0,
                true,
                height_scale,
                color,
                1.0,
            );
        }
    }
    for (x0, x1, incoming) in highlighted {
        arc(
            &painter,
            x0,
            x1,
            axis_y - 4.0,
            true,
            height_scale,
            if incoming {
                style::edge_in()
            } else {
                style::edge_out()
            },
            2.0,
        );
    }

    // Data hand-off arcs (below).
    let focus_comp = view.focus_component;
    let total: usize = seq.handoffs.iter().map(|h| h.to.len()).sum();
    let draw_all = focus_comp.is_none() && focus_col.is_none() && total <= 600;
    let mut strong_arcs: Vec<(f32, f32, Color32)> = Vec::new();
    for h in &seq.handoffs {
        let name = &snapshot.components[h.comp as usize].name;
        for t in &h.to {
            let strong =
                focus_comp == Some(h.comp) || focus_col.is_some_and(|c| c == h.from || c == *t);
            if !(strong || draw_all) {
                continue;
            }
            let (x0, x1) = (cols.center(h.from), cols.center(*t));
            if x1.max(x0) < clip.min.x || x0.min(x1) > clip.max.x {
                continue;
            }
            if strong {
                strong_arcs.push((x0, x1, style::hue_color(name, 0.75, 1.0, 255)));
            } else {
                arc(
                    &painter,
                    x0,
                    x1,
                    axis_y + 4.0,
                    false,
                    height_scale,
                    style::hue_color(name, 0.5, 0.9, 70),
                    1.0,
                );
            }
        }
    }
    for (x0, x1, color) in strong_arcs {
        arc(
            &painter,
            x0,
            x1,
            axis_y + 4.0,
            false,
            height_scale,
            color,
            2.0,
        );
    }

    // System markers on the axis.
    let search = view.search.to_lowercase();
    for fs in &seq.systems {
        let x = cols.center(fs.column);
        if x < clip.min.x - 10.0 || x > clip.max.x + 10.0 {
            continue;
        }
        let system = &snapshot.schedules[fs.schedule].systems[fs.system as usize];
        let matches = search.is_empty() || search_matches(snapshot, system, &search);
        let writes = seq
            .marks
            .iter()
            .any(|m| m.column == fs.column && m.kind.is_write());
        let mut color = if writes {
            style::write()
        } else {
            style::read()
        };
        if !matches {
            color = color.gamma_multiply(0.4);
        }
        let r = if focus_col == Some(fs.column) {
            6.0
        } else {
            4.0
        };
        painter.circle_filled(pos2(x, axis_y), r, color);
        if system.flags.exclusive {
            painter.circle_stroke(
                pos2(x, axis_y),
                r + 2.0,
                Stroke::new(1.0, style::exclusive()),
            );
        }
    }

    draw_header(
        &painter,
        &text,
        snapshot,
        seq,
        cols,
        0.0,
        font,
        clip,
        hovered_col,
        selected_col,
    );
    let caption = FontId::proportional(font.size * 0.8);
    text.text(
        pos2(4.0, axis_y - arc_h - 2.0),
        Align2::LEFT_BOTTOM,
        "ordering ⬆",
        &caption,
        style::filter(),
    );
    text.text(
        pos2(4.0, axis_y + 14.0),
        Align2::LEFT_TOP,
        "data flow ⬇",
        &caption,
        style::filter(),
    );

    if let Some(col) = hovered_col {
        let rect = Rect::from_min_max(
            pos2(cols.left(col), top),
            pos2(cols.left(col) + COL_W, bottom),
        );
        let resp = ui.interact(rect, ui.id().with(("arc_col_tip", col)), Sense::hover());
        resp.on_hover_ui_at_pointer(|ui| column_tooltip(ui, snapshot, seq, col));
    }
}

// ------------------------------------------------------------------------------------------------
// Component explorer (used by the details panel)
// ------------------------------------------------------------------------------------------------

/// Lifecycle of one component through the frame: every access in execution order.
pub fn component_explorer(
    ui: &mut egui::Ui,
    snapshot: &AppSnapshot,
    view: &mut ViewState,
    comp: CompIx,
) {
    ensure_frame(snapshot, view);
    let Some(cache) = view.frame_cache.take() else {
        return;
    };
    let seq = &cache.seq;
    let c = &snapshot.components[comp as usize];
    ui.add(egui::Label::new(egui::RichText::new(&c.name).monospace().small()).wrap());
    ui.colored_label(
        style::filter(),
        if c.is_resource {
            "resource"
        } else {
            "component"
        },
    );
    let marks: Vec<&frame::Mark> = seq.marks.iter().filter(|m| m.comp == comp).collect();
    let writers = marks.iter().filter(|m| m.kind.is_write()).count();
    let readers = marks.iter().filter(|m| m.kind.is_read()).count();
    ui.label(format!(
        "{writers} writes · {readers} reads · {} hand-offs",
        seq.handoffs.iter().filter(|h| h.comp == comp).count()
    ));
    ui.separator();
    let mut select: Option<(String, SysIx)> = None;
    let mut last_band: Option<usize> = None;
    let band_of = |col: usize| {
        seq.bands
            .iter()
            .position(|b| b.first <= col && col < b.last)
    };
    for m in marks {
        let band = band_of(m.column);
        if band != last_band {
            if let Some(b) = band {
                ui.add_space(4.0);
                let band = &seq.bands[b];
                ui.colored_label(band_color(band.kind, &band.label, 255), &band.label);
            }
            last_band = band;
        }
        let fs = seq.systems[m.column];
        let schedule = &snapshot.schedules[fs.schedule];
        let system = &schedule.systems[fs.system as usize];
        ui.horizontal_wrapped(|ui| {
            let (chip, color) = match m.kind {
                MarkKind::Write => ("W", style::write()),
                MarkKind::ConditionWrite => ("W?", style::write()),
                MarkKind::Read => ("R", style::read()),
                MarkKind::ConditionRead => ("R?", style::read()),
                MarkKind::Filter => ("filter", style::filter()),
                MarkKind::Archetypal => ("Has", style::filter()),
            };
            ui.colored_label(color, chip);
            if ui
                .add(
                    egui::Label::new(
                        egui::RichText::new(&system.short_name).color(style::node_text()),
                    )
                    .sense(Sense::click()),
                )
                .clicked()
            {
                select = Some((schedule.label.clone(), fs.system));
            }
            // Query context: the filters of the query touching this component.
            for q in &system.queries {
                if !(q.reads.contains(&comp) || q.writes.contains(&comp)) {
                    continue;
                }
                let mut parts = Vec::new();
                for f in &q.filters {
                    if !f.with.is_empty() {
                        parts.push(format!(
                            "With<{}>",
                            f.with
                                .iter()
                                .map(|c| comp_name(snapshot, *c))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ));
                    }
                    if !f.without.is_empty() {
                        parts.push(format!(
                            "Without<{}>",
                            f.without
                                .iter()
                                .map(|c| comp_name(snapshot, *c))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ));
                    }
                }
                if !parts.is_empty() {
                    ui.colored_label(style::filter(), parts.join(" "));
                }
            }
        });
    }
    view.frame_cache = Some(cache);
    if let Some((label, sys)) = select {
        view.select_in_schedule(label, NodeRef::System(sys));
    }
}

/// Systems of the frame that touch `comp`, for highlighting elsewhere.
pub fn systems_touching(seq: &FrameSequence, comp: CompIx) -> HashSet<usize> {
    seq.marks
        .iter()
        .filter(|m| m.comp == comp)
        .map(|m| m.column)
        .collect()
}
