//! Right-hand details panel for the selected system or set.

use std::collections::BTreeMap;

use bedtime_ui::egui;
use bedtime_ui::{Button, Row, Separator, Text};

use super::{ViewState, comp_name, named_sets, style};
use crate::{
    flow::FlowKind,
    snapshot::{
        AppSnapshot, CompIx, NodeRef, QueryAccess, ResourceAccess, ScheduleSnapshot, SysIx,
    },
};

/// A clickable coloured label; returns `true` when clicked.
fn chip(ui: &mut egui::Ui, text: impl Into<String>, color: egui::Color32) -> bool {
    ui.add(Text::new(text).color(color).clickable()).clicked()
}

/// Renders the reads/writes/filters of some queries and resources. Clicking a component name
/// stores it in `focus`.
pub fn access_line(
    ui: &mut egui::Ui,
    snapshot: &AppSnapshot,
    queries: &[QueryAccess],
    resources: &[ResourceAccess],
    focus: &mut Option<CompIx>,
) {
    for query in queries {
        if query.write_all {
            ui.add(Text::new("&mut *").color(style::warn()));
        } else if query.read_all {
            ui.add(Text::new("&*").color(style::warn()));
        }
        for c in &query.writes {
            if chip(
                ui,
                format!("&mut {}", comp_name(snapshot, *c)),
                style::write(),
            ) {
                *focus = Some(*c);
            }
        }
        for c in &query.reads {
            if chip(ui, format!("&{}", comp_name(snapshot, *c)), style::read()) {
                *focus = Some(*c);
            }
        }
        for c in &query.archetypal {
            if chip(
                ui,
                format!("Has<{}>", comp_name(snapshot, *c)),
                style::filter(),
            ) {
                *focus = Some(*c);
            }
        }
        for (i, filter) in query.filters.iter().enumerate() {
            if i > 0 {
                ui.add(Text::new("or").color(style::filter()));
            }
            if !filter.with.is_empty() {
                let names: Vec<&str> = filter
                    .with
                    .iter()
                    .map(|c| comp_name(snapshot, *c))
                    .collect();
                ui.add(Text::new(format!("With<{}>", names.join(", "))).color(style::filter()));
            }
            if !filter.without.is_empty() {
                let names: Vec<&str> = filter
                    .without
                    .iter()
                    .map(|c| comp_name(snapshot, *c))
                    .collect();
                ui.add(Text::new(format!("Without<{}>", names.join(", "))).color(style::filter()));
            }
        }
    }
    for resource in resources {
        let (text, color) = if resource.write {
            (
                format!("ResMut<{}>", comp_name(snapshot, resource.comp)),
                style::write(),
            )
        } else {
            (
                format!("Res<{}>", comp_name(snapshot, resource.comp)),
                style::read(),
            )
        };
        if chip(ui, text, color) {
            *focus = Some(resource.comp);
        }
    }
}

pub fn details_panel(
    ui: &mut egui::Ui,
    snapshot: &AppSnapshot,
    schedule: &ScheduleSnapshot,
    view: &mut ViewState,
) {
    if let Some(comp) = view.focus_component {
        Row::new().show(ui, |ui| {
            if ui.add(Button::new("×")).clicked() {
                view.focus_component = None;
            }
            ui.add(Text::heading(
                snapshot.components[comp as usize].short_name.clone(),
            ));
        });
        if view.focus_component.is_some() {
            let height = if view.selected.is_some() {
                ui.available_height() * 0.45
            } else {
                ui.available_height()
            };
            egui::ScrollArea::vertical()
                .id_salt("component_explorer")
                .max_height(height)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    super::frame_view::component_explorer(ui, snapshot, view, comp)
                });
            ui.add(Separator::new());
        }
    }
    let Some(selected) = view.selected else {
        return;
    };
    let mut select: Option<Option<NodeRef>> = None;
    let mut focus: Option<CompIx> = None;
    let mut hide: Option<NodeRef> = None;
    let mut toggle_collapse: Option<String> = None;
    Row::new().show(ui, |ui| {
        if ui.add(Button::new("×")).clicked() {
            select = Some(None);
        }
        ui.add(Text::heading(match selected {
            NodeRef::System(s) => schedule.systems[s as usize].short_name.clone(),
            NodeRef::Set(s) => schedule.sets[s as usize].name.clone(),
        }));
    });
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| match selected {
            NodeRef::System(ix) => system_details(
                ui,
                snapshot,
                schedule,
                view,
                ix,
                &mut select,
                &mut focus,
                &mut hide,
            ),
            NodeRef::Set(ix) => set_details(
                ui,
                schedule,
                view,
                ix,
                &mut select,
                &mut hide,
                &mut toggle_collapse,
            ),
        });
    if let Some(node) = select {
        view.select(node);
    }
    if let Some(comp) = focus {
        view.focus_component = if view.focus_component == Some(comp) {
            None
        } else {
            Some(comp)
        };
    }
    if let Some(node) = hide {
        let name = match node {
            NodeRef::System(s) => schedule.systems[s as usize].name.clone(),
            NodeRef::Set(s) => schedule.sets[s as usize].name.clone(),
        };
        let state = view.schedule_state();
        match node {
            NodeRef::System(_) => state.hidden_systems.insert(name),
            NodeRef::Set(_) => state.hidden_sets.insert(name),
        };
        view.selected = None;
    }
    if let Some(name) = toggle_collapse {
        let state = view.schedule_state();
        if !state.collapsed_sets.remove(&name) {
            state.collapsed_sets.insert(name);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn system_details(
    ui: &mut egui::Ui,
    snapshot: &AppSnapshot,
    schedule: &ScheduleSnapshot,
    view: &ViewState,
    ix: SysIx,
    select: &mut Option<Option<NodeRef>>,
    focus: &mut Option<CompIx>,
    hide: &mut Option<NodeRef>,
) {
    let system = &schedule.systems[ix as usize];
    ui.add(Text::caption(&system.name).wrap());
    Row::new().wrap().show(ui, |ui| {
        let f = &system.flags;
        if f.apply_deferred {
            ui.add(Text::new("sync point").color(style::edge_sync()));
        }
        if f.exclusive {
            ui.add(Text::new("exclusive").color(style::exclusive()));
        }
        if f.deferred {
            ui.add(Text::new("deferred").color(style::filter()));
        }
        if f.non_send {
            ui.add(Text::new("non-send").color(style::filter()));
        }
        if f.write_all {
            ui.add(Text::new("writes all").color(style::warn()));
        } else if f.read_all {
            ui.add(Text::new("reads all").color(style::warn()));
        }
        if !f.access_available && !f.exclusive && !f.apply_deferred {
            ui.add(Text::new("access not captured").color(style::warn()));
        }
        if ui.add(Button::new("hide").small()).clicked() {
            *hide = Some(NodeRef::System(ix));
        }
    });
    let sets = named_sets(schedule, system);
    if !sets.is_empty() {
        ui.add(Separator::new());
        ui.add(Text::new("Sets"));
        Row::new().wrap().show(ui, |ui| {
            for (set_ix, set) in schedule.sets.iter().enumerate() {
                if set.is_named()
                    && sets.contains(&set.name.as_str())
                    && chip(ui, &set.name, style::node_text())
                {
                    *select = Some(Some(NodeRef::Set(set_ix as u32)));
                }
            }
        });
    }
    if !system.conditions.is_empty() {
        ui.add(Separator::new());
        ui.add(Text::new("Run conditions"));
        for condition in &system.conditions {
            Row::new().wrap().show(ui, |ui| {
                ui.add(Text::new(&condition.short_name));
                access_line(
                    ui,
                    snapshot,
                    &condition.queries,
                    &condition.resources,
                    focus,
                );
            });
        }
    }
    ui.add(Separator::new());
    ui.add(Text::new("Access"));
    if system.queries.is_empty() && system.resources.is_empty() {
        ui.add(Text::new("no component or resource access").color(style::filter()));
    }
    for query in &system.queries {
        Row::new().wrap().show(ui, |ui| {
            ui.add(Text::new("query").color(style::filter()));
            access_line(ui, snapshot, std::slice::from_ref(query), &[], focus);
        });
    }
    if !system.resources.is_empty() {
        Row::new().wrap().show(ui, |ui| {
            ui.add(Text::new("resources").color(style::filter()));
            access_line(ui, snapshot, &[], &system.resources, focus);
        });
    }

    ui.add(Separator::new());
    ui.add(Text::new("Ordering"));
    let mut after: Vec<SysIx> = schedule
        .flat_edges
        .iter()
        .filter(|e| e.to == ix)
        .map(|e| e.from)
        .collect();
    let mut before: Vec<SysIx> = schedule
        .flat_edges
        .iter()
        .filter(|e| e.from == ix)
        .map(|e| e.to)
        .collect();
    after.sort_unstable();
    after.dedup();
    before.sort_unstable();
    before.dedup();
    system_links(ui, schedule, "after", &after, select);
    system_links(ui, schedule, "before", &before, select);

    if let Some(flow) = view.flow_cache.as_ref() {
        let mut consumes: BTreeMap<CompIx, Vec<SysIx>> = BTreeMap::new();
        let mut produces: BTreeMap<CompIx, Vec<SysIx>> = BTreeMap::new();
        for e in &flow.edges {
            if e.to == ix && e.kind == FlowKind::WriteRead {
                consumes.entry(e.comp).or_default().push(e.from);
            }
            if e.from == ix {
                produces.entry(e.comp).or_default().push(e.to);
            }
        }
        if !consumes.is_empty() || !produces.is_empty() {
            ui.add(Separator::new());
            ui.add(Text::new("Data flow"));
        }
        for (title, map) in [("consumes from", consumes), ("produces for", produces)] {
            for (comp, systems) in map {
                Row::new().wrap().show(ui, |ui| {
                    ui.add(Text::new(title).color(style::filter()));
                    if chip(
                        ui,
                        comp_name(snapshot, comp),
                        style::hue_color(&snapshot.components[comp as usize].name, 0.6, 0.95, 255),
                    ) {
                        *focus = Some(comp);
                    }
                    ui.add(Text::new("◀").color(style::filter()));
                    let mut systems = systems;
                    systems.sort_unstable();
                    systems.dedup();
                    for s in systems {
                        if chip(
                            ui,
                            &schedule.systems[s as usize].short_name,
                            style::node_text(),
                        ) {
                            *select = Some(Some(NodeRef::System(s)));
                        }
                    }
                });
            }
        }
    }

    let conflicts: Vec<_> = schedule
        .conflicts
        .iter()
        .filter(|c| c.a == ix || c.b == ix)
        .collect();
    if !conflicts.is_empty() {
        ui.add(Separator::new());
        ui.add(Text::new(format!("{} ambiguities", conflicts.len())).color(style::ambiguity()));
        for c in conflicts {
            let other = if c.a == ix { c.b } else { c.a };
            Row::new().wrap().show(ui, |ui| {
                if chip(
                    ui,
                    &schedule.systems[other as usize].short_name,
                    style::node_text(),
                ) {
                    *select = Some(Some(NodeRef::System(other)));
                }
                ui.add(Text::new("on").color(style::filter()));
                if c.components.is_empty() {
                    ui.add(Text::new("World").color(style::warn()));
                }
                for comp in &c.components {
                    if chip(ui, comp_name(snapshot, *comp), style::ambiguity()) {
                        *focus = Some(*comp);
                    }
                }
            });
        }
    }
}

fn system_links(
    ui: &mut egui::Ui,
    schedule: &ScheduleSnapshot,
    title: &str,
    systems: &[SysIx],
    select: &mut Option<Option<NodeRef>>,
) {
    if systems.is_empty() {
        return;
    }
    Row::new().wrap().show(ui, |ui| {
        ui.add(Text::new(title).color(style::filter()));
        for s in systems {
            let system = &schedule.systems[*s as usize];
            let label = if system.flags.apply_deferred {
                "⏸ sync".to_string()
            } else {
                system.short_name.clone()
            };
            if chip(ui, label, style::node_text()) {
                *select = Some(Some(NodeRef::System(*s)));
            }
        }
    });
}

fn set_details(
    ui: &mut egui::Ui,
    schedule: &ScheduleSnapshot,
    view: &ViewState,
    ix: u32,
    select: &mut Option<Option<NodeRef>>,
    hide: &mut Option<NodeRef>,
    toggle_collapse: &mut Option<String>,
) {
    let set = &schedule.sets[ix as usize];
    let collapsed = view
        .per_schedule
        .get(&schedule.label)
        .is_some_and(|s| s.collapsed_sets.contains(&set.name));
    Row::new().show(ui, |ui| {
        if ui
            .add(Button::new(if collapsed { "expand" } else { "collapse" }).small())
            .clicked()
        {
            *toggle_collapse = Some(set.name.clone());
        }
        if ui.add(Button::new("hide").small()).clicked() {
            *hide = Some(NodeRef::Set(ix));
        }
    });
    if !set.conditions.is_empty() {
        ui.add(Separator::new());
        ui.add(Text::new("Run conditions"));
        for condition in &set.conditions {
            ui.add(Text::new(&condition.short_name));
        }
    }
    ui.add(Separator::new());
    let members: Vec<SysIx> = schedule
        .systems
        .iter()
        .enumerate()
        .filter(|(_, s)| s.sets.contains(&ix))
        .map(|(i, _)| i as SysIx)
        .collect();
    ui.add(Text::new(format!("{} direct members", members.len())));
    for s in members {
        if chip(
            ui,
            &schedule.systems[s as usize].short_name,
            style::node_text(),
        ) {
            *select = Some(Some(NodeRef::System(s)));
        }
    }
    let children: Vec<&str> = schedule
        .hierarchy
        .iter()
        .filter_map(|(p, c)| match (p, c) {
            (NodeRef::Set(p), NodeRef::Set(c)) if *p == ix => {
                Some(schedule.sets[*c as usize].name.as_str())
            }
            _ => None,
        })
        .collect();
    if !children.is_empty() {
        ui.add(Separator::new());
        ui.add(Text::new(format!("child sets: {}", children.join(", "))));
    }
    let deps: Vec<String> = schedule
        .dependency
        .iter()
        .filter_map(|(a, b)| {
            let name = |n: &NodeRef| match n {
                NodeRef::System(s) => schedule.systems[*s as usize].short_name.clone(),
                NodeRef::Set(s) => schedule.sets[*s as usize].name.clone(),
            };
            if *a == NodeRef::Set(ix) {
                Some(format!("before {}", name(b)))
            } else if *b == NodeRef::Set(ix) {
                Some(format!("after {}", name(a)))
            } else {
                None
            }
        })
        .collect();
    if !deps.is_empty() {
        ui.add(Separator::new());
        ui.add(Text::new("Configured ordering"));
        for d in deps {
            ui.add(Text::new(d));
        }
    }
}
