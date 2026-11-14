//! Left-hand control panel: schedule picker, search, toggles, sets, hidden nodes, components.

use std::collections::{BTreeMap, HashSet};

use bedtime_ui::egui::{self, RichText};

use super::{LaneSort, Mode, ViewState, comp_name, style};
use crate::{
    flow,
    snapshot::{AppSnapshot, CompIx, NodeRef, ScheduleSnapshot},
};

fn group_of(snapshot: &AppSnapshot, label: &str) -> u8 {
    if snapshot.main_order.iter().any(|l| l == label) {
        0
    } else if snapshot.startup_order.iter().any(|l| l == label) {
        1
    } else if snapshot.fixed_main_order.iter().any(|l| l == label) {
        2
    } else {
        3
    }
}

pub fn side_panel(ui: &mut egui::Ui, snapshot: &AppSnapshot, view: &mut ViewState) {
    ui.horizontal_wrapped(|ui| {
        for mode in Mode::ALL {
            if ui
                .selectable_label(view.mode == mode, mode.label())
                .clicked()
            {
                view.set_mode(mode);
            }
        }
    });
    ui.label(
        RichText::new(format!(
            "snapshot #{} · {} schedules · {} components",
            snapshot.generation,
            snapshot.schedules.len(),
            snapshot.components.len()
        ))
        .small()
        .color(style::filter()),
    );
    ui.separator();

    let current = view.schedule.clone().unwrap_or_else(|| "—".into());
    let mut pick: Option<String> = None;
    egui::ComboBox::from_id_salt("schedules_schedule")
        .selected_text(current.as_str())
        .width(ui.available_width())
        .show_ui(ui, |ui| {
            let mut last_group = None;
            for s in &snapshot.schedules {
                let group = group_of(snapshot, &s.label);
                if last_group.is_some_and(|g| g != group) {
                    ui.separator();
                }
                last_group = Some(group);
                let label = format!("{}  ({})", s.label, s.systems.len());
                if ui
                    .selectable_label(view.schedule.as_deref() == Some(s.label.as_str()), label)
                    .clicked()
                {
                    pick = Some(s.label.clone());
                }
            }
        });
    if let Some(label) = pick {
        view.select_schedule(label);
    }
    ui.separator();

    ui.add(
        egui::TextEdit::singleline(&mut view.search)
            .hint_text("search systems or components")
            .desired_width(f32::INFINITY),
    );
    ui.checkbox(&mut view.hide_non_matching, "hide non-matching");
    ui.collapsing("Show", |ui| {
        ui.checkbox(&mut view.show_internal, "bevy internal systems");
        ui.checkbox(&mut view.show_self, "this panel's own systems");
        ui.checkbox(&mut view.show_sync, "sync points (ApplyDeferred)");
    });
    if view.mode == Mode::Graph {
        ui.collapsing("Edges", |ui| {
            ui.checkbox(&mut view.show_ordering, "ordering");
            ui.checkbox(&mut view.show_sync_edges, "sync-point ordering");
            ui.checkbox(&mut view.show_flow, "data flow (hover / select / focus)");
            ui.checkbox(&mut view.show_ambiguity, "ambiguities");
            ui.checkbox(
                &mut view.transitive_reduce,
                "hide transitive ordering edges",
            );
        });
    }
    if view.mode.is_frame_wide() {
        egui::CollapsingHeader::new("Frame")
            .default_open(true)
            .show(ui, |ui| {
                ui.checkbox(&mut view.frame_opts.include_startup, "startup schedules");
                ui.checkbox(
                    &mut view.frame_opts.include_transitions,
                    "state transition schedules",
                );
                ui.checkbox(&mut view.frame_opts.include_other, "schedules outside Main");
                if matches!(view.mode, Mode::Lanes | Mode::Arcs) {
                    ui.separator();
                    ui.checkbox(&mut view.show_legend, "legend")
                        .on_hover_text("what the symbols and colours on the canvas mean");
                    ui.checkbox(&mut view.frame_opts.show_components, "component lanes");
                    ui.checkbox(&mut view.frame_opts.show_resources, "resource lanes");
                    ui.checkbox(&mut view.frame_opts.writes_only, "only lanes with a write");
                    ui.checkbox(
                        &mut view.frame_opts.only_matching,
                        "only lanes matching search",
                    );
                    let touching = ui
                        .checkbox(
                            &mut view.frame_opts.touching_only,
                            "only systems touching a shown lane",
                        )
                        .on_hover_text(
                            "hides every system with no read or write on the lanes above, so a \
                             search narrows the columns as well as the rows",
                        );
                    if touching.changed() {
                        // The frame keeps only a fraction of its columns: re-frame it.
                        view.frame_fit_requested = true;
                    }
                    ui.horizontal(|ui| {
                        ui.label("sort");
                        ui.selectable_value(
                            &mut view.frame_opts.lane_sort,
                            LaneSort::FirstAccess,
                            "first access",
                        );
                        ui.selectable_value(
                            &mut view.frame_opts.lane_sort,
                            LaneSort::Activity,
                            "activity",
                        );
                        ui.selectable_value(&mut view.frame_opts.lane_sort, LaneSort::Name, "name");
                    });
                }
                if view.mode == Mode::Timeline {
                    ui.checkbox(&mut view.show_critical_path, "highlight critical path");
                    #[cfg(feature = "profile")]
                    {
                        let frames = view
                            .timings
                            .as_ref()
                            .and_then(|t| t.lock().ok().map(|s| s.frames.len()))
                            .unwrap_or(0);
                        ui.add_enabled(
                            frames > 0,
                            egui::Checkbox::new(
                                &mut view.timeline_measured,
                                format!("measured timings ({frames} frames)"),
                            ),
                        )
                        .on_disabled_hover_text(
                            "no timings: install bedtime_schedules::profile::layer via LogPlugin::custom_layer",
                        );
                        if frames > 0 && view.timeline_measured {
                            ui.horizontal(|ui| {
                                ui.label("time zoom");
                                let old = view.time_zoom;
                                if ui
                                    .add(
                                        egui::Slider::new(&mut view.time_zoom, 0.25..=64.0)
                                            .logarithmic(true)
                                            .show_value(false),
                                    )
                                    .changed()
                                {
                                    // Keep the visible time window anchored at the label column.
                                    let k = view.time_zoom / old.max(1e-3);
                                    let x0 = super::profile_view::TRACK_LABEL_W;
                                    let r = view.frame_scene_rect;
                                    view.frame_scene_rect = egui::Rect::from_min_max(
                                        egui::pos2(x0 + (r.min.x - x0) * k, r.min.y),
                                        egui::pos2(x0 + (r.max.x - x0) * k, r.max.y),
                                    );
                                }
                                if ui.small_button("1×").clicked() {
                                    view.time_zoom = 1.0;
                                    view.frame_fit_requested = true;
                                }
                            });
                            ui.checkbox(&mut view.heat_colours, "colour by duration");
                            ui.horizontal(|ui| {
                                ui.label("frame");
                                ui.selectable_value(&mut view.use_slowest_frame, false, "latest");
                                ui.selectable_value(&mut view.use_slowest_frame, true, "slowest");
                            });
                        }
                    }
                }
            });
    }

    #[cfg(feature = "profile")]
    if view.mode == Mode::Timeline && view.timeline_measured {
        egui::CollapsingHeader::new("Slowest systems")
            .default_open(true)
            .show(ui, |ui| {
                super::profile_view::slowest_list(ui, snapshot, view)
            });
    }

    let Some(label) = view.schedule.clone() else {
        return;
    };
    let Some(schedule) = snapshot.schedule(&label) else {
        return;
    };

    ui.horizontal_wrapped(|ui| {
        if ui.button("Fit").clicked() {
            view.fit_requested = true;
            view.frame_fit_requested = true;
            view.frame_fit_requested = true;
        }
        if ui.button("Expand all").clicked() {
            view.schedule_state().collapsed_sets.clear();
        }
        if ui.button("Collapse all").clicked() {
            let names: Vec<String> = named_sets_with_members(schedule)
                .into_iter()
                .map(|(_, s)| s.name.clone())
                .collect();
            view.schedule_state().collapsed_sets.extend(names);
        }
        if ui.button("Unhide all").clicked() {
            let state = view.schedule_state();
            state.hidden_systems.clear();
            state.hidden_sets.clear();
        }
    });
    ui.separator();
    ui.label(format!(
        "{} systems · {} sets · {} ordering edges",
        schedule.systems.len(),
        named_sets_with_members(schedule).len(),
        schedule.flat_edges.len()
    ));
    if schedule.conflicts_available {
        ui.label(format!("{} ambiguities", schedule.conflicts.len()));
    } else {
        ui.colored_label(
            style::filter(),
            "ambiguities unavailable (schedule was running)",
        );
    }
    if view.relayouts > 0 {
        ui.label(
            RichText::new(format!("layouts computed: {}", view.relayouts))
                .small()
                .color(style::filter()),
        );
    }
    for warning in &schedule.warnings {
        ui.colored_label(style::warn(), RichText::new(warning).small());
    }
    ui.separator();

    let state = view.per_schedule.get(&label).cloned().unwrap_or_default();
    let mut toggle_collapse: Vec<String> = Vec::new();
    let mut unhide_systems: Vec<String> = Vec::new();
    let mut unhide_sets: Vec<String> = Vec::new();
    let mut select: Option<NodeRef> = None;
    let mut focus: Option<CompIx> = None;
    let search = view.search.to_lowercase();

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::CollapsingHeader::new("Sets")
                .default_open(true)
                .show(ui, |ui| {
                    for (ix, set) in named_sets_with_members(schedule) {
                        ui.horizontal(|ui| {
                            let mut collapsed = state.collapsed_sets.contains(&set.name);
                            if ui
                                .checkbox(&mut collapsed, "")
                                .on_hover_text("collapse into one node")
                                .changed()
                            {
                                toggle_collapse.push(set.name.clone());
                            }
                            let hidden = state.hidden_sets.contains(&set.name);
                            let text = if hidden {
                                RichText::new(&set.name).strikethrough()
                            } else {
                                RichText::new(&set.name)
                            };
                            if ui
                                .selectable_label(view.selected == Some(NodeRef::Set(ix)), text)
                                .clicked()
                            {
                                select = Some(NodeRef::Set(ix));
                            }
                        });
                    }
                });

            let hidden_count = state.hidden_systems.len() + state.hidden_sets.len();
            if hidden_count > 0 {
                egui::CollapsingHeader::new(format!("Hidden ({hidden_count})"))
                    .default_open(false)
                    .show(ui, |ui| {
                        let mut names: Vec<&String> = state.hidden_systems.iter().collect();
                        names.sort();
                        for name in names {
                            ui.horizontal(|ui| {
                                if ui.small_button("show").clicked() {
                                    unhide_systems.push(name.clone());
                                }
                                let short = schedule
                                    .systems
                                    .iter()
                                    .find(|s| &s.name == name)
                                    .map_or(name.as_str(), |s| s.short_name.as_str());
                                ui.label(short);
                            });
                        }
                        let mut names: Vec<&String> = state.hidden_sets.iter().collect();
                        names.sort();
                        for name in names {
                            ui.horizontal(|ui| {
                                if ui.small_button("show").clicked() {
                                    unhide_sets.push(name.clone());
                                }
                                ui.label(format!("set {name}"));
                            });
                        }
                    });
            }

            let index = flow::access_index(schedule);
            let mut comps: BTreeMap<&str, (CompIx, usize, usize)> = BTreeMap::new();
            for (comp, writers) in &index.writers {
                comps
                    .entry(comp_name(snapshot, *comp))
                    .or_insert((*comp, 0, 0))
                    .1 = writers.len();
            }
            for (comp, readers) in &index.readers {
                comps
                    .entry(comp_name(snapshot, *comp))
                    .or_insert((*comp, 0, 0))
                    .2 = readers.len();
            }
            egui::CollapsingHeader::new(format!("Components ({})", comps.len()))
                .default_open(false)
                .show(ui, |ui| {
                    for (name, (comp, writers, readers)) in comps {
                        if !search.is_empty() && !name.to_lowercase().contains(&search) {
                            continue;
                        }
                        let is_resource = snapshot.components[comp as usize].is_resource;
                        let mut text = RichText::new(format!("{name}  {writers}w {readers}r"));
                        if is_resource {
                            text = text.italics();
                        }
                        if ui
                            .selectable_label(view.focus_component == Some(comp), text)
                            .on_hover_text(if is_resource { "resource" } else { "component" })
                            .clicked()
                        {
                            focus = Some(comp);
                        }
                    }
                });
        });

    if !toggle_collapse.is_empty() || !unhide_systems.is_empty() || !unhide_sets.is_empty() {
        let state = view.schedule_state();
        for name in toggle_collapse {
            if !state.collapsed_sets.remove(&name) {
                state.collapsed_sets.insert(name);
            }
        }
        for name in unhide_systems {
            state.hidden_systems.remove(&name);
        }
        for name in unhide_sets {
            state.hidden_sets.remove(&name);
        }
    }
    if let Some(node) = select {
        view.select(Some(node));
    }
    if let Some(comp) = focus {
        view.focus_component = if view.focus_component == Some(comp) {
            None
        } else {
            Some(comp)
        };
    }
}

/// Named sets that contain at least one system (directly or through child sets).
pub fn named_sets_with_members(
    schedule: &ScheduleSnapshot,
) -> Vec<(u32, &crate::snapshot::SetSnapshot)> {
    let mut used: HashSet<u32> = HashSet::new();
    let mut parents: Vec<Vec<u32>> = vec![Vec::new(); schedule.sets.len()];
    for (p, c) in &schedule.hierarchy {
        if let (NodeRef::Set(p), NodeRef::Set(c)) = (p, c) {
            parents[*c as usize].push(*p);
        }
    }
    for system in &schedule.systems {
        let mut stack: Vec<u32> = system.sets.clone();
        while let Some(s) = stack.pop() {
            if used.insert(s) {
                stack.extend(parents[s as usize].iter().copied());
            }
        }
    }
    schedule
        .sets
        .iter()
        .enumerate()
        .filter(|(i, s)| s.is_named() && used.contains(&(*i as u32)))
        .map(|(i, s)| (i as u32, s))
        .collect()
}
