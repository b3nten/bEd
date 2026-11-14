//! The graph canvas: a pan/zoom scene with cluster boxes, nodes and edges.

use std::{
    collections::{HashMap, HashSet},
    hash::{Hash, Hasher},
};

use bedtime_ui::egui::{
    self, Align2, Color32, CornerRadius, FontId, Painter, Pos2, Rect, Sense, Shape, Stroke,
    StrokeKind, TextStyle, epaint::CubicBezierShape, pos2, vec2,
};

use super::{
    FlowCache, LayoutCache, ViewState, comp_name, named_sets, scene_text::SceneText,
    search_matches, style,
};
use crate::{
    flow,
    layout::{
        self,
        visible::{self, VisibleSpec},
    },
    snapshot::{AppSnapshot, CompIx, NodeRef, OrderKind, ScheduleSnapshot, SetIx, SysIx},
};

enum Action {
    Select(Option<NodeRef>),
    Hover(Option<NodeRef>),
    ToggleCollapse(String),
    Hide(NodeRef),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Dash {
    Solid,
    Dashed,
    Dotted,
}

fn to_egui(rect: bevy::math::Rect) -> Rect {
    Rect::from_min_max(pos2(rect.min.x, rect.min.y), pos2(rect.max.x, rect.max.y))
}

fn to_pos(v: bevy::math::Vec2) -> Pos2 {
    pos2(v.x, v.y)
}

pub fn show(
    ui: &mut egui::Ui,
    snapshot: &AppSnapshot,
    schedule: &ScheduleSnapshot,
    view: &mut ViewState,
) {
    let font = TextStyle::Body.resolve(ui.style());
    ensure_layout(ui, snapshot, schedule, view, &font);
    ensure_flow(snapshot, schedule, view);
    let Some(cache) = view.layout_cache.take() else {
        return;
    };
    let flow = view.flow_cache.take();
    if view.fit_requested {
        let bounds = to_egui(cache.layout.bounds);
        view.scene_rect = if bounds.width() > 1.0 && bounds.height() > 1.0 {
            bounds
        } else {
            Rect::from_min_size(Pos2::ZERO, vec2(400.0, 300.0))
        };
        view.fit_requested = false;
    }
    let mut scene_rect = view.scene_rect;
    let mut actions = Vec::new();
    egui::Scene::new()
        .zoom_range(0.01..=4.0)
        .show(ui, &mut scene_rect, |ui| {
            draw(
                ui,
                snapshot,
                schedule,
                view,
                &cache,
                flow.as_ref(),
                &font,
                &mut actions,
            );
        });
    view.scene_rect = scene_rect;
    view.layout_cache = Some(cache);
    view.flow_cache = flow;

    for action in actions {
        match action {
            Action::Select(node) => view.select(node),
            Action::Hover(node) => view.hovered = node,
            Action::ToggleCollapse(name) => {
                let state = view.schedule_state();
                if !state.collapsed_sets.remove(&name) {
                    state.collapsed_sets.insert(name);
                }
            }
            Action::Hide(node) => {
                let name = match node {
                    NodeRef::System(s) => schedule.systems[s as usize].name.clone(),
                    NodeRef::Set(s) => schedule.sets[s as usize].name.clone(),
                };
                let state = view.schedule_state();
                match node {
                    NodeRef::System(_) => state.hidden_systems.insert(name),
                    NodeRef::Set(_) => state.hidden_sets.insert(name),
                };
                if view.selected == Some(node) {
                    view.selected = None;
                }
            }
        }
    }
}

fn ensure_flow(snapshot: &AppSnapshot, schedule: &ScheduleSnapshot, view: &mut ViewState) {
    let key = (snapshot.generation, schedule.label.clone());
    if view.flow_cache.as_ref().is_some_and(|c| c.key == key) {
        return;
    }
    view.flow_cache = Some(FlowCache {
        key,
        edges: flow::flow_edges(schedule),
    });
}

fn ensure_layout(
    ui: &egui::Ui,
    snapshot: &AppSnapshot,
    schedule: &ScheduleSnapshot,
    view: &mut ViewState,
    font: &FontId,
) {
    let state = view
        .per_schedule
        .get(&schedule.label)
        .cloned()
        .unwrap_or_default();
    let mut collapsed: Vec<SetIx> = schedule
        .sets
        .iter()
        .enumerate()
        .filter(|(_, s)| state.collapsed_sets.contains(&s.name))
        .map(|(i, _)| i as SetIx)
        .collect();
    collapsed.sort_unstable();
    let mut hidden: Vec<NodeRef> = schedule
        .systems
        .iter()
        .enumerate()
        .filter(|(_, s)| state.hidden_systems.contains(&s.name))
        .map(|(i, _)| NodeRef::System(i as SysIx))
        .chain(
            schedule
                .sets
                .iter()
                .enumerate()
                .filter(|(_, s)| state.hidden_sets.contains(&s.name))
                .map(|(i, _)| NodeRef::Set(i as SetIx)),
        )
        .collect();
    hidden.sort_unstable();
    let search = view.search.to_lowercase();
    let only: Option<Vec<SysIx>> = (view.hide_non_matching && !search.is_empty()).then(|| {
        schedule
            .systems
            .iter()
            .enumerate()
            .filter(|(_, s)| search_matches(snapshot, s, &search))
            .map(|(i, _)| i as SysIx)
            .collect()
    });

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    schedule.label.hash(&mut hasher);
    snapshot.generation.hash(&mut hasher);
    collapsed.hash(&mut hasher);
    hidden.hash(&mut hasher);
    only.hash(&mut hasher);
    (
        view.show_internal,
        view.show_self,
        view.show_sync,
        view.transitive_reduce,
    )
        .hash(&mut hasher);
    for f in [
        view.params.node_height,
        view.params.rank_gap,
        view.params.node_gap,
        view.params.cluster_pad,
    ] {
        f.to_bits().hash(&mut hasher);
    }
    let key = hasher.finish();
    if view.layout_cache.as_ref().is_some_and(|c| c.key == key) {
        return;
    }

    let collapsed: HashSet<SetIx> = collapsed.into_iter().collect();
    let hidden: HashSet<NodeRef> = hidden.into_iter().collect();
    let only: Option<HashSet<SysIx>> = only.map(|v| v.into_iter().collect());
    let spec = VisibleSpec {
        collapsed: &collapsed,
        hidden: &hidden,
        show_internal: view.show_internal,
        show_self: view.show_self,
        show_sync: view.show_sync,
        only: only.as_ref(),
        transitive_reduce: view.transitive_reduce,
    };
    let painter = ui.painter().clone();
    let width_cache = &mut view.width_cache;
    let measure = |text: &str| -> f32 {
        *width_cache.entry(text.to_string()).or_insert_with(|| {
            painter
                .layout_no_wrap(text.to_string(), font.clone(), Color32::WHITE)
                .size()
                .x
        })
    };
    let graph = visible::build_visible(schedule, &spec, measure);
    let layout = layout::compute_layout(&graph.input, &view.params);
    view.relayouts += 1;
    bevy::log::debug!(
        "schedules: layout of {} recomputed ({} nodes, {} edges, {} clusters, {} crossings, {:.0}x{:.0})",
        schedule.label,
        graph.input.nodes.len(),
        graph.input.edges.len(),
        graph.input.clusters.len(),
        layout.crossings,
        layout.bounds.width(),
        layout.bounds.height()
    );
    if let Ok(path) = std::env::var("BEVYGRAPH_DUMP_LAYOUT") {
        dump_layout(&path, schedule, &graph, &layout);
    }
    view.layout_cache = Some(LayoutCache { key, graph, layout });
}

/// Debug aid: writes the visible graph and its layout as plain text.
fn dump_layout(
    path: &str,
    schedule: &ScheduleSnapshot,
    graph: &visible::VisibleGraph,
    layout: &layout::LayoutResult,
) {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(out, "schedule {}", schedule.label);
    for (i, node) in graph.input.nodes.iter().enumerate() {
        let r = layout.node_rects[i];
        let _ = writeln!(
            out,
            "node {i} rank={} x0={:.0} x1={:.0} y0={:.0} cluster={:?} label={}",
            layout.ranks[i], r.min.x, r.max.x, r.min.y, node.cluster, graph.labels[i]
        );
    }
    for (c, cluster) in graph.input.clusters.iter().enumerate() {
        let rect = layout
            .cluster_rects
            .iter()
            .find(|(k, _)| *k == c)
            .map(|(_, r)| *r);
        let _ = writeln!(
            out,
            "cluster {c} parent={:?} name={} rect={:?}",
            cluster.parent, cluster.name, rect
        );
    }
    for e in &graph.input.edges {
        let _ = writeln!(
            out,
            "edge {} -> {} {:?} via_hidden={}",
            e.from, e.to, e.kind, e.via_hidden
        );
    }
    if let Err(err) = std::fs::write(path, out) {
        bevy::log::warn!("schedules: could not write layout dump to {path}: {err}");
    }
}

fn draw_bezier(
    painter: &Painter,
    points: [Pos2; 4],
    color: Color32,
    width: f32,
    dash: Dash,
    arrow: bool,
) {
    let stroke = Stroke::new(width, color);
    let shape = CubicBezierShape::from_points_stroke(points, false, Color32::TRANSPARENT, stroke);
    match dash {
        Dash::Solid => {
            painter.add(shape);
        }
        Dash::Dashed => {
            let flat = shape.flatten(Some(0.5));
            painter.extend(Shape::dashed_line(&flat, stroke, 7.0, 4.0));
        }
        Dash::Dotted => {
            let flat = shape.flatten(Some(0.5));
            painter.extend(Shape::dashed_line(&flat, stroke, 2.0, 3.0));
        }
    }
    if arrow {
        draw_arrowhead(painter, points[2], points[3], color, width);
    }
}

fn draw_arrowhead(painter: &Painter, from: Pos2, tip: Pos2, color: Color32, width: f32) {
    let dir = (tip - from).normalized();
    if !dir.x.is_finite() || !dir.y.is_finite() {
        return;
    }
    let size = 5.0 + width * 1.5;
    let normal = vec2(-dir.y, dir.x);
    let base = tip - dir * size;
    painter.add(Shape::convex_polygon(
        vec![tip, base + normal * size * 0.5, base - normal * size * 0.5],
        color,
        Stroke::NONE,
    ));
}

fn bezier_point(points: [Pos2; 4], t: f32) -> Pos2 {
    let u = 1.0 - t;
    let p = points[0].to_vec2() * (u * u * u)
        + points[1].to_vec2() * (3.0 * u * u * t)
        + points[2].to_vec2() * (3.0 * u * t * t)
        + points[3].to_vec2() * (t * t * t);
    p.to_pos2()
}

#[allow(clippy::too_many_arguments)]
fn draw(
    ui: &mut egui::Ui,
    snapshot: &AppSnapshot,
    schedule: &ScheduleSnapshot,
    view: &ViewState,
    cache: &LayoutCache,
    flow: Option<&FlowCache>,
    font: &FontId,
    actions: &mut Vec<Action>,
) {
    let graph = &cache.graph;
    let layout = &cache.layout;
    let clip = ui.clip_rect();
    let painter = ui.painter().clone();
    let text = SceneText::new(ui);
    let node_count = graph.input.nodes.len();
    let search = view.search.to_lowercase();

    // Node representing each system (after collapsing).
    let mut node_of_sys: HashMap<SysIx, usize> = HashMap::new();
    for (i, members) in graph.members.iter().enumerate() {
        for s in members {
            node_of_sys.insert(*s, i);
        }
    }

    // --- Interaction pass -------------------------------------------------------------------
    let mut hovered: Option<NodeRef> = None;
    let mut responses: Vec<Option<egui::Response>> = Vec::with_capacity(node_count);
    for (i, node) in graph.input.nodes.iter().enumerate() {
        let rect = to_egui(layout.node_rects[i]);
        if !rect.intersects(clip) {
            responses.push(None);
            continue;
        }
        let resp = ui.interact(
            rect,
            ui.id().with(("schedules_node", node.id)),
            Sense::click(),
        );
        if resp.hovered() {
            hovered = Some(node.id);
        }
        if resp.clicked() {
            actions.push(Action::Select(Some(node.id)));
        }
        if resp.double_clicked()
            && let NodeRef::Set(set) = node.id
        {
            actions.push(Action::ToggleCollapse(
                schedule.sets[set as usize].name.clone(),
            ));
        }
        if resp.secondary_clicked() {
            actions.push(Action::Hide(node.id));
        }
        responses.push(Some(resp));
    }
    let header_h = view.params.cluster_header;
    let mut hovered_cluster: Option<usize> = None;
    for (c, rect) in &layout.cluster_rects {
        let rect = to_egui(*rect);
        let header = Rect::from_min_size(rect.min, vec2(rect.width(), header_h));
        if !header.intersects(clip) {
            continue;
        }
        let cluster = &graph.input.clusters[*c];
        let resp = ui.interact(
            header,
            ui.id().with(("schedules_cluster", cluster.set)),
            Sense::click(),
        );
        if resp.hovered() {
            hovered_cluster = Some(*c);
        }
        if resp.clicked() {
            actions.push(Action::ToggleCollapse(cluster.name.clone()));
        }
        if resp.secondary_clicked() {
            actions.push(Action::Hide(NodeRef::Set(cluster.set)));
        }
        resp.on_hover_text("click to collapse · right-click to hide");
    }
    actions.push(Action::Hover(hovered));

    let focus_node: Option<NodeRef> = hovered.or(view.selected);
    let focus_idx: Option<usize> = focus_node.and_then(|id| graph.node_index.get(&id).copied());
    let focus_members: HashSet<SysIx> = focus_idx
        .map(|i| graph.members[i].iter().copied().collect())
        .unwrap_or_default();

    // --- Clusters ---------------------------------------------------------------------------
    for (c, rect) in &layout.cluster_rects {
        let rect = to_egui(*rect);
        if !rect.intersects(clip) {
            continue;
        }
        let cluster = &graph.input.clusters[*c];
        let hovered = hovered_cluster == Some(*c);
        let fill = style::hue_color(&cluster.name, 0.35, 0.5, if hovered { 60 } else { 34 });
        let stroke = style::hue_color(&cluster.name, 0.45, 0.8, 140);
        painter.rect_filled(rect, CornerRadius::same(6), fill);
        painter.rect_stroke(
            rect,
            CornerRadius::same(6),
            Stroke::new(1.0, stroke),
            StrokeKind::Inside,
        );
        text.text(
            rect.min + vec2(8.0, header_h * 0.5),
            Align2::LEFT_CENTER,
            format!("▪ {}", cluster.name),
            font,
            style::hue_color(&cluster.name, 0.3, 1.0, 230),
        );
    }

    // --- Base ordering edges ----------------------------------------------------------------
    let mut incident: Vec<(usize, bool)> = Vec::new(); // (edge index, incoming)
    for (path, edge) in layout.edge_paths.iter().zip(&graph.input.edges) {
        let is_sync = edge.kind == OrderKind::SyncPoint;
        if (is_sync && !view.show_sync_edges) || (!is_sync && !view.show_ordering) {
            continue;
        }
        if let Some(f) = focus_idx
            && (edge.from == f || edge.to == f)
        {
            incident.push((path.edge, edge.to == f));
            continue;
        }
        let pts = path.points.map(to_pos);
        let bbox = Rect::from_points(&pts);
        if !bbox.intersects(clip) {
            continue;
        }
        let color = if focus_idx.is_some() {
            style::edge_dim()
        } else if is_sync {
            style::edge_sync()
        } else {
            style::edge()
        };
        let dash = if edge.via_hidden {
            Dash::Dotted
        } else if is_sync {
            Dash::Dashed
        } else {
            Dash::Solid
        };
        draw_bezier(&painter, pts, color, 1.2, dash, false);
    }

    // --- Ambiguity edges --------------------------------------------------------------------
    if view.show_ambiguity && schedule.conflicts_available {
        let draw_all = focus_idx.is_none() && schedule.conflicts.len() <= 80;
        for conflict in &schedule.conflicts {
            let (Some(&a), Some(&b)) = (node_of_sys.get(&conflict.a), node_of_sys.get(&conflict.b))
            else {
                continue;
            };
            if a == b {
                continue;
            }
            let is_incident = focus_idx.is_some_and(|f| f == a || f == b);
            // Whole-World conflicts (exclusive systems) touch everything: only when focused.
            if !is_incident && (!draw_all || conflict.components.is_empty()) {
                continue;
            }
            let ra = to_egui(layout.node_rects[a]);
            let rb = to_egui(layout.node_rects[b]);
            let color = if is_incident {
                style::ambiguity()
            } else {
                style::ambiguity().gamma_multiply(0.35)
            };
            painter.extend(Shape::dashed_line(
                &[ra.center(), rb.center()],
                Stroke::new(if is_incident { 1.6 } else { 1.0 }, color),
                4.0,
                4.0,
            ));
        }
    }

    // --- Data-flow edges (focused only) -----------------------------------------------------
    if view.show_flow
        && let Some(flow) = flow
    {
        let selected: Vec<&flow::FlowEdge> = flow
            .edges
            .iter()
            .filter(|e| match view.focus_component {
                Some(comp) => e.comp == comp,
                None => {
                    (focus_members.contains(&e.from) || focus_members.contains(&e.to))
                        && !(focus_members.contains(&e.from) && focus_members.contains(&e.to))
                }
            })
            .collect();
        let label_edges = selected.len() <= 16;
        let mut drawn: HashSet<(usize, usize, CompIx)> = HashSet::new();
        for e in selected {
            let (Some(&a), Some(&b)) = (node_of_sys.get(&e.from), node_of_sys.get(&e.to)) else {
                continue;
            };
            if a == b || !drawn.insert((a, b, e.comp)) {
                continue;
            }
            let ra = to_egui(layout.node_rects[a]);
            let rb = to_egui(layout.node_rects[b]);
            let name = &snapshot.components[e.comp as usize].name;
            let color = style::hue_color(name, 0.7, 1.0, 235);
            // Offset the curve sideways a little so it does not sit on the ordering edge.
            let offset = ((e.comp as f32 * 7.0) % 20.0) - 10.0;
            let p0 = pos2(ra.center().x + offset, ra.max.y);
            let p3 = pos2(rb.center().x + offset, rb.min.y);
            let dy = (p3.y - p0.y).abs().max(30.0) * 0.6;
            let pts = [p0, p0 + vec2(0.0, dy), p3 - vec2(0.0, dy), p3];
            draw_bezier(&painter, pts, color, 1.6, Dash::Dashed, true);
            if label_edges {
                let mid = bezier_point(pts, 0.5);
                text.text(
                    mid + vec2(4.0, 0.0),
                    Align2::LEFT_CENTER,
                    comp_name(snapshot, e.comp),
                    &FontId::proportional(font.size * 0.85),
                    color,
                );
            }
        }
    }

    // --- Highlighted ordering edges ---------------------------------------------------------
    for (edge_ix, incoming) in incident {
        let pts = layout.edge_paths[edge_ix].points.map(to_pos);
        let edge = &graph.input.edges[edge_ix];
        let color = if incoming {
            style::edge_in()
        } else {
            style::edge_out()
        };
        let dash = if edge.via_hidden {
            Dash::Dotted
        } else if edge.kind == OrderKind::SyncPoint {
            Dash::Dashed
        } else {
            Dash::Solid
        };
        draw_bezier(&painter, pts, color, 2.2, dash, true);
    }

    // --- Nodes ------------------------------------------------------------------------------
    let conflicted: HashSet<SysIx> = schedule.conflicts.iter().flat_map(|c| [c.a, c.b]).collect();
    for (i, node) in graph.input.nodes.iter().enumerate() {
        let rect = to_egui(layout.node_rects[i]);
        if !rect.intersects(clip) {
            continue;
        }
        let system = match node.id {
            NodeRef::System(s) => Some(&schedule.systems[s as usize]),
            NodeRef::Set(_) => None,
        };
        let is_hovered = hovered == Some(node.id);
        let is_selected = view.selected == Some(node.id);
        let matches = search.is_empty()
            || graph.members[i]
                .iter()
                .any(|s| search_matches(snapshot, &schedule.systems[*s as usize], &search));
        let sync = system.is_some_and(|s| s.flags.apply_deferred);
        let mut fill = match node.id {
            NodeRef::Set(_) => style::node_fill_set(),
            NodeRef::System(_) if sync => style::node_fill_sync(),
            NodeRef::System(_) => style::node_fill(),
        };
        if is_hovered {
            fill = style::node_fill_hover();
        }
        let text_color = if matches {
            style::node_text()
        } else {
            style::node_text_dim()
        };
        if !matches {
            fill = fill.gamma_multiply(0.6);
        }
        let stroke = if is_selected {
            Stroke::new(2.0, style::selected())
        } else if system.is_some_and(|s| s.flags.exclusive) {
            Stroke::new(1.5, style::exclusive())
        } else if focus_idx == Some(i) {
            Stroke::new(1.5, style::node_text())
        } else {
            Stroke::new(1.0, style::node_stroke())
        };
        let radius = if sync { 12 } else { 5 };
        painter.rect_filled(rect, CornerRadius::same(radius), fill);
        painter.rect_stroke(rect, CornerRadius::same(radius), stroke, StrokeKind::Inside);
        text.text(
            rect.center(),
            Align2::CENTER_CENTER,
            &graph.labels[i],
            font,
            text_color,
        );
        if let Some(system) = system {
            let mut badge_x = rect.max.x - 4.0;
            if !system.conditions.is_empty() {
                text.text(
                    pos2(badge_x, rect.min.y + 1.0),
                    Align2::RIGHT_TOP,
                    "?",
                    &FontId::proportional(font.size * 0.7),
                    style::warn(),
                );
                badge_x -= 8.0;
            }
            if let NodeRef::System(s) = node.id
                && conflicted.contains(&s)
            {
                text.text(
                    pos2(badge_x, rect.min.y + 1.0),
                    Align2::RIGHT_TOP,
                    "!",
                    &FontId::proportional(font.size * 0.7),
                    style::ambiguity(),
                );
            }
        }
    }

    // --- Tooltip ----------------------------------------------------------------------------
    if let Some(id) = hovered
        && let Some(i) = graph.node_index.get(&id).copied()
        && let Some(resp) = responses[i].take()
    {
        resp.on_hover_ui(|ui| tooltip(ui, snapshot, schedule, graph, i));
    }
}

fn tooltip(
    ui: &mut egui::Ui,
    snapshot: &AppSnapshot,
    schedule: &ScheduleSnapshot,
    graph: &visible::VisibleGraph,
    node: usize,
) {
    ui.set_max_width(420.0);
    match graph.input.nodes[node].id {
        NodeRef::System(s) => {
            let system = &schedule.systems[s as usize];
            ui.monospace(&system.name);
            let sets = named_sets(schedule, system);
            if !sets.is_empty() {
                ui.colored_label(style::filter(), format!("sets: {}", sets.join(", ")));
            }
            let mut writes = Vec::new();
            let mut reads = Vec::new();
            for q in &system.queries {
                writes.extend(q.writes.iter().map(|c| comp_name(snapshot, *c)));
                reads.extend(q.reads.iter().map(|c| comp_name(snapshot, *c)));
            }
            for r in &system.resources {
                if r.write {
                    writes.push(comp_name(snapshot, r.comp));
                } else {
                    reads.push(comp_name(snapshot, r.comp));
                }
            }
            if !writes.is_empty() {
                ui.colored_label(style::write(), format!("writes: {}", writes.join(", ")));
            }
            if !reads.is_empty() {
                ui.colored_label(style::read(), format!("reads: {}", reads.join(", ")));
            }
            if system.flags.exclusive {
                ui.colored_label(style::exclusive(), "exclusive (&mut World)");
            }
            if !system.conditions.is_empty() {
                let names: Vec<&str> = system
                    .conditions
                    .iter()
                    .map(|c| c.short_name.as_str())
                    .collect();
                ui.colored_label(style::warn(), format!("run_if: {}", names.join(", ")));
            }
            ui.colored_label(style::filter(), "click: details · right-click: hide");
        }
        NodeRef::Set(set) => {
            ui.label(format!("set {}", schedule.sets[set as usize].name));
            let members = &graph.members[node];
            ui.colored_label(style::filter(), format!("{} systems", members.len()));
            for s in members.iter().take(12) {
                ui.label(&schedule.systems[*s as usize].short_name);
            }
            if members.len() > 12 {
                ui.label("…");
            }
            ui.colored_label(style::filter(), "double-click: expand · right-click: hide");
        }
    }
}
