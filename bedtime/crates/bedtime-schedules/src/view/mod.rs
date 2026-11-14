//! The egui viewer.

mod canvas;
mod details;
pub mod frame_view;
mod legend;
mod list;
mod panel;
pub mod profile_view;
mod scene_text;
pub mod style;

use std::collections::{HashMap, HashSet};

use bedtime_ui::egui;

pub use frame_view::{FrameViewOptions, LaneSort};

use crate::{
    flow::FlowEdge,
    frame::FrameSequence,
    layout::{LayoutParams, LayoutResult, visible::VisibleGraph},
    snapshot::{AppSnapshot, CompIx, NodeRef, ScheduleSnapshot, SystemSnapshot},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Per-schedule dependency graph.
    Graph,
    /// Frame-wide timeline: components as lanes, systems as columns.
    Lanes,
    /// Frame-wide arc diagram: ordering above the axis, data flow below.
    Arcs,
    /// Profiler-style timeline: waves as time slots, parallel systems stacked.
    Timeline,
    /// Flame chart of frame → schedule → sets → systems.
    Flame,
    /// Plain list in executor order.
    List,
}

impl Mode {
    pub const ALL: [Mode; 6] = [
        Mode::Graph,
        Mode::Lanes,
        Mode::Arcs,
        Mode::Timeline,
        Mode::Flame,
        Mode::List,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Mode::Graph => "Graph",
            Mode::Lanes => "Lanes",
            Mode::Arcs => "Arcs",
            Mode::Timeline => "Timeline",
            Mode::Flame => "Flame",
            Mode::List => "List",
        }
    }

    /// Whether the mode shows the whole frame rather than one schedule.
    pub fn is_frame_wide(self) -> bool {
        !matches!(self, Mode::Graph | Mode::List)
    }
}

/// Per-schedule collapse/hide state, stored by name so it survives snapshot rebuilds.
#[derive(Default, Clone, Debug)]
pub struct ScheduleViewState {
    pub collapsed_sets: HashSet<String>,
    pub hidden_systems: HashSet<String>,
    pub hidden_sets: HashSet<String>,
}

pub struct LayoutCache {
    pub key: u64,
    pub graph: VisibleGraph,
    pub layout: LayoutResult,
}

pub struct FlowCache {
    pub key: (u64, String),
    pub edges: Vec<FlowEdge>,
}

pub struct FrameCache {
    pub key: u64,
    pub seq: FrameSequence,
}

/// Interaction state of the viewer.
pub struct ViewState {
    pub mode: Mode,
    /// Label of the schedule being shown.
    pub schedule: Option<String>,
    pub search: String,
    pub hide_non_matching: bool,
    pub show_internal: bool,
    pub show_self: bool,
    pub show_sync: bool,
    pub show_ordering: bool,
    pub show_sync_edges: bool,
    pub show_flow: bool,
    pub show_ambiguity: bool,
    pub transitive_reduce: bool,
    pub selected: Option<NodeRef>,
    pub hovered: Option<NodeRef>,
    pub focus_component: Option<CompIx>,
    pub per_schedule: HashMap<String, ScheduleViewState>,
    pub scene_rect: egui::Rect,
    pub fit_requested: bool,
    pub params: LayoutParams,
    pub width_cache: HashMap<String, f32>,
    pub layout_cache: Option<LayoutCache>,
    pub flow_cache: Option<FlowCache>,
    pub generation_seen: u64,
    /// Number of layouts computed so far (debug aid: must not grow every frame).
    pub relayouts: u64,
    pub frame_opts: FrameViewOptions,
    pub frame_cache: Option<FrameCache>,
    pub frame_scene_rect: egui::Rect,
    pub frame_fit_requested: bool,
    pub show_critical_path: bool,
    /// Show the symbol/colour legend over the Lanes and Arcs canvas.
    pub show_legend: bool,
    pub flame_root: Option<usize>,
    pub flame_show_sets: bool,
    /// Use measured per-system timings for the timeline when available.
    pub timeline_measured: bool,
    /// Horizontal zoom of the measured timeline (1 = the frame spans ~2200 scene px).
    pub time_zoom: f32,
    /// Colour measured bars by duration.
    pub heat_colours: bool,
    /// Show the slowest kept frame instead of the latest one.
    pub use_slowest_frame: bool,
    /// Whether the last timeline drawn used measured timings (re-fit when this flips).
    pub timeline_was_measured: bool,
    #[cfg(feature = "profile")]
    pub timings: Option<std::sync::Arc<std::sync::Mutex<crate::profile::TimingStore>>>,
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            mode: Mode::Lanes,
            schedule: None,
            search: String::new(),
            hide_non_matching: false,
            show_internal: false,
            show_self: false,
            show_sync: true,
            show_ordering: true,
            show_sync_edges: true,
            show_flow: true,
            show_ambiguity: true,
            transitive_reduce: true,
            selected: None,
            hovered: None,
            focus_component: None,
            per_schedule: HashMap::new(),
            scene_rect: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0)),
            fit_requested: true,
            params: LayoutParams::default(),
            width_cache: HashMap::new(),
            layout_cache: None,
            flow_cache: None,
            generation_seen: 0,
            relayouts: 0,
            frame_opts: FrameViewOptions::default(),
            frame_cache: None,
            frame_scene_rect: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0)),
            frame_fit_requested: true,
            show_critical_path: true,
            show_legend: false,
            flame_root: None,
            flame_show_sets: true,
            timeline_measured: true,
            time_zoom: 1.0,
            heat_colours: true,
            use_slowest_frame: false,
            timeline_was_measured: false,
            #[cfg(feature = "profile")]
            timings: Some(crate::profile::store()),
        }
    }
}

impl ViewState {
    /// Selects a node in a schedule, switching to that schedule without resetting the view.
    pub fn select_in_schedule(&mut self, label: String, node: NodeRef) {
        if self.schedule.as_deref() != Some(label.as_str()) {
            self.schedule = Some(label);
            self.layout_cache = None;
            self.fit_requested = true;
        }
        self.selected = Some(node);
    }

    pub fn set_mode(&mut self, mode: Mode) {
        if self.mode != mode {
            self.mode = mode;
            if mode.is_frame_wide() {
                self.frame_fit_requested = true;
            } else {
                self.fit_requested = true;
            }
        }
    }

    /// Mutable per-schedule state of the schedule being shown.
    pub fn schedule_state(&mut self) -> &mut ScheduleViewState {
        let label = self.schedule.clone().unwrap_or_default();
        self.per_schedule.entry(label).or_default()
    }

    pub fn select_schedule(&mut self, label: String) {
        if self.schedule.as_deref() != Some(label.as_str()) {
            self.schedule = Some(label);
            self.selected = None;
            self.hovered = None;
            self.focus_component = None;
            self.fit_requested = true;
            self.layout_cache = None;
        }
    }

    pub fn select(&mut self, node: Option<NodeRef>) {
        self.selected = node;
    }
}

/// Draws the viewer into `ui`, in `theme`'s colours: the controls, the view chosen, and the
/// details of what's selected.
pub fn frame(
    ui: &mut egui::Ui,
    theme: &bedtime_ui::Theme,
    snapshot: &AppSnapshot,
    view: &mut ViewState,
) {
    style::set(theme);
    if view.schedule.is_none() {
        view.schedule = snapshot
            .schedule("Update")
            .or(snapshot.schedules.first())
            .map(|s| s.label.clone());
    }
    if view.generation_seen != snapshot.generation {
        view.generation_seen = snapshot.generation;
        view.layout_cache = None;
        view.flow_cache = None;
        view.frame_cache = None;
    }
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        view.selected = None;
        view.focus_component = None;
    }
    egui::Panel::left(egui::Id::new("schedules_side"))
        .resizable(true)
        .default_size(270.0)
        .size_range(200.0..=420.0)
        .show(ui, |ui| panel::side_panel(ui, snapshot, view));
    let Some(schedule) = view
        .schedule
        .as_deref()
        .and_then(|label| snapshot.schedule(label))
    else {
        egui::CentralPanel::default().show(ui, |ui| {
            ui.label("No schedule selected");
        });
        return;
    };
    if view.selected.is_some() || view.focus_component.is_some() {
        egui::Panel::right(egui::Id::new("schedules_details"))
            .resizable(true)
            .default_size(360.0)
            .size_range(240.0..=600.0)
            .show(ui, |ui| {
                details::details_panel(ui, snapshot, schedule, view)
            });
    }
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.fill(style::canvas_bg()))
        .show(ui, |ui| match view.mode {
            Mode::Graph => canvas::show(ui, snapshot, schedule, view),
            Mode::Lanes => frame_view::show_lanes(ui, snapshot, view),
            Mode::Arcs => frame_view::show_arcs(ui, snapshot, view),
            Mode::Timeline => profile_view::show_timeline(ui, snapshot, view),
            Mode::Flame => profile_view::show_flame(ui, snapshot, view),
            Mode::List => list::system_list(ui, snapshot, schedule, view),
        });
}

/// Whether a system matches the (lower-cased) search text by name or by accessed component.
pub(crate) fn search_matches(
    snapshot: &AppSnapshot,
    system: &SystemSnapshot,
    search: &str,
) -> bool {
    if search.is_empty() || system.name.to_lowercase().contains(search) {
        return true;
    }
    let comp_matches = |ix: &CompIx| {
        snapshot.components[*ix as usize]
            .short_name
            .to_lowercase()
            .contains(search)
    };
    system.queries.iter().any(|q| {
        q.reads.iter().any(comp_matches)
            || q.writes.iter().any(comp_matches)
            || q.filters
                .iter()
                .any(|f| f.with.iter().any(comp_matches) || f.without.iter().any(comp_matches))
    }) || system.resources.iter().any(|r| comp_matches(&r.comp))
}

/// Short display name of a component.
pub(crate) fn comp_name(snapshot: &AppSnapshot, ix: CompIx) -> &str {
    &snapshot.components[ix as usize].short_name
}

/// Named sets a system belongs to.
pub(crate) fn named_sets<'a>(
    schedule: &'a ScheduleSnapshot,
    system: &SystemSnapshot,
) -> Vec<&'a str> {
    system
        .sets
        .iter()
        .map(|s| &schedule.sets[*s as usize])
        .filter(|s| s.is_named())
        .map(|s| s.name.as_str())
        .collect()
}
