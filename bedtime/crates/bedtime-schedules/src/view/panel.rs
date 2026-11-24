//! Left-hand control panel: schedule picker, search, toggles, sets, hidden nodes, components.

use std::collections::{BTreeMap, HashSet};

use bedtime_ui::{BuiltinIcon, prelude::*};

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
    let mut mode = view.mode;
    let modes = Segmented::with(&mut mode, &Mode::ALL, |mode| mode.label().into());
    if ui.add(modes).changed() {
        view.set_mode(mode);
    }
    ui.add(
        Text::caption(format!(
            "snapshot #{} · {} schedules · {} components",
            snapshot.generation,
            snapshot.schedules.len(),
            snapshot.components.len()
        ))
        .color(style::filter()),
    );
    ui.add(Separator::new());

    // The schedules, a line between their groups (Main, startup, fixed, the rest).
    let names: Vec<String> = snapshot
        .schedules
        .iter()
        .map(|schedule| schedule.label.clone())
        .collect();
    let groups: Vec<u8> = names.iter().map(|name| group_of(snapshot, name)).collect();
    let dividers = (1..groups.len()).filter(|index| groups[*index] != groups[index - 1]);
    let mut chosen = view.schedule.clone().unwrap_or_default();
    let schedules = Select::with(&mut chosen, &names, |name| {
        let systems = snapshot.schedule(name).map_or(0, |s| s.systems.len());
        format!("{name}  ({systems})")
    })
    .placeholder("—")
    .dividers(dividers)
    .searchable()
    .fill_width();
    if ui.add(schedules).changed() {
        view.select_schedule(chosen);
    }
    ui.add(Separator::new());

    ui.add(
        TextField::new(&mut view.search)
            .hint("search systems or components")
            .icon(BuiltinIcon::Search)
            .clearable()
            .fill_width(),
    );
    ui.add(Checkbox::new(
        &mut view.hide_non_matching,
        "hide non-matching",
    ));
    Section::new("Show").show(ui, |ui| {
        ui.add(Checkbox::new(
            &mut view.show_internal,
            "bevy internal systems",
        ));
        ui.add(Checkbox::new(
            &mut view.show_self,
            "this panel's own systems",
        ));
        ui.add(Checkbox::new(
            &mut view.show_sync,
            "sync points (ApplyDeferred)",
        ));
    });
    if view.mode == Mode::Graph {
        Section::new("Edges").show(ui, |ui| {
            ui.add(Checkbox::new(&mut view.show_ordering, "ordering"));
            ui.add(Checkbox::new(
                &mut view.show_sync_edges,
                "sync-point ordering",
            ));
            ui.add(Checkbox::new(
                &mut view.show_flow,
                "data flow (hover / select / focus)",
            ));
            ui.add(Checkbox::new(&mut view.show_ambiguity, "ambiguities"));
            ui.add(Checkbox::new(
                &mut view.transitive_reduce,
                "hide transitive ordering edges",
            ));
        });
    }
    if view.mode.is_frame_wide() {
        Section::new("Frame").open().show(ui, |ui| {
            ui.add(Checkbox::new(
                &mut view.frame_opts.include_startup,
                "startup schedules",
            ));
            ui.add(Checkbox::new(
                &mut view.frame_opts.include_transitions,
                "state transition schedules",
            ));
            ui.add(Checkbox::new(
                &mut view.frame_opts.include_other,
                "schedules outside Main",
            ));
            if matches!(view.mode, Mode::Lanes | Mode::Arcs) {
                ui.add(Separator::new());
                ui.add(Checkbox::new(&mut view.show_legend, "legend"))
                    .on_hover_text("what the symbols and colours on the canvas mean");
                ui.add(Checkbox::new(
                    &mut view.frame_opts.show_components,
                    "component lanes",
                ));
                ui.add(Checkbox::new(
                    &mut view.frame_opts.show_resources,
                    "resource lanes",
                ));
                ui.add(Checkbox::new(
                    &mut view.frame_opts.writes_only,
                    "only lanes with a write",
                ));
                ui.add(Checkbox::new(
                    &mut view.frame_opts.only_matching,
                    "only lanes matching search",
                ));
                let touching = ui
                    .add(Checkbox::new(
                        &mut view.frame_opts.touching_only,
                        "only systems touching a shown lane",
                    ))
                    .on_hover_text(
                        "hides every system with no read or write on the lanes above, so a \
                         search narrows the columns as well as the rows",
                    );
                if touching.changed() {
                    // The frame keeps only a fraction of its columns: re-frame it.
                    view.frame_fit_requested = true;
                }
                Row::new().show(ui, |ui| {
                    ui.add(Text::new("sort"));
                    ui.add(
                        Segmented::with(
                            &mut view.frame_opts.lane_sort,
                            &[LaneSort::FirstAccess, LaneSort::Activity, LaneSort::Name],
                            |sort| {
                                match sort {
                                    LaneSort::FirstAccess => "first access",
                                    LaneSort::Activity => "activity",
                                    LaneSort::Name => "name",
                                }
                                .into()
                            },
                        )
                        .size(Size::Sm),
                    );
                });
            }
            if view.mode == Mode::Timeline {
                ui.add(Checkbox::new(
                    &mut view.show_critical_path,
                    "highlight critical path",
                ));
                #[cfg(feature = "profile")]
                {
                    let frames = view
                        .timings
                        .as_ref()
                        .and_then(|t| t.lock().ok().map(|s| s.frames.len()))
                        .unwrap_or(0);
                    ui.add_enabled(
                        frames > 0,
                        Checkbox::new(
                            &mut view.timeline_measured,
                            format!("measured timings ({frames} frames)"),
                        ),
                    )
                    .on_disabled_hover_text(
                        "no timings: install bedtime_schedules::profile::layer via LogPlugin::custom_layer",
                    );
                    if frames > 0 && view.timeline_measured {
                        Row::new().show(ui, |ui| {
                            ui.add(Text::new("time zoom"));
                            let old = view.time_zoom;
                            // bedtime-ui has no slider yet.
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
                            if ui.add(Button::new("1×").small()).clicked() {
                                view.time_zoom = 1.0;
                                view.frame_fit_requested = true;
                            }
                        });
                        ui.add(Checkbox::new(
                            &mut view.heat_colours,
                            "colour by duration",
                        ));
                        Row::new().show(ui, |ui| {
                            ui.add(Text::new("frame"));
                            ui.add(
                                Segmented::with(
                                    &mut view.use_slowest_frame,
                                    &[false, true],
                                    |slowest| if *slowest { "slowest" } else { "latest" }.into(),
                                )
                                .size(Size::Sm),
                            );
                        });
                    }
                }
            }
        });
    }

    #[cfg(feature = "profile")]
    if view.mode == Mode::Timeline && view.timeline_measured {
        Section::new("Slowest systems").open().show(ui, |ui| {
            super::profile_view::slowest_list(ui, snapshot, view)
        });
    }

    let Some(label) = view.schedule.clone() else {
        return;
    };
    let Some(schedule) = snapshot.schedule(&label) else {
        return;
    };

    Row::new().wrap().show(ui, |ui| {
        if ui.add(Button::new("Fit")).clicked() {
            view.fit_requested = true;
            view.frame_fit_requested = true;
        }
        if ui.add(Button::new("Expand all")).clicked() {
            view.schedule_state().collapsed_sets.clear();
        }
        if ui.add(Button::new("Collapse all")).clicked() {
            let names: Vec<String> = named_sets_with_members(schedule)
                .into_iter()
                .map(|(_, s)| s.name.clone())
                .collect();
            view.schedule_state().collapsed_sets.extend(names);
        }
        if ui.add(Button::new("Unhide all")).clicked() {
            let state = view.schedule_state();
            state.hidden_systems.clear();
            state.hidden_sets.clear();
        }
    });
    ui.add(Separator::new());
    ui.add(Text::new(format!(
        "{} systems · {} sets · {} ordering edges",
        schedule.systems.len(),
        named_sets_with_members(schedule).len(),
        schedule.flat_edges.len()
    )));
    if schedule.conflicts_available {
        ui.add(Text::new(format!(
            "{} ambiguities",
            schedule.conflicts.len()
        )));
    } else {
        ui.add(Text::new("ambiguities unavailable (schedule was running)").color(style::filter()));
    }
    if view.relayouts > 0 {
        ui.add(
            Text::caption(format!("layouts computed: {}", view.relayouts)).color(style::filter()),
        );
    }
    for warning in &schedule.warnings {
        ui.add(Text::caption(warning).color(style::warn()));
    }
    ui.add(Separator::new());

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
            Section::new("Sets").open().show(ui, |ui| {
                for (ix, set) in named_sets_with_members(schedule) {
                    Row::new().show(ui, |ui| {
                        let mut collapsed = state.collapsed_sets.contains(&set.name);
                        if ui
                            .add(Checkbox::new(&mut collapsed, ""))
                            .on_hover_text("collapse into one node")
                            .changed()
                        {
                            toggle_collapse.push(set.name.clone());
                        }
                        let row = Button::new(&set.name)
                            .ghost()
                            .fill_width()
                            .align(egui::Align::Min)
                            .strikethrough(state.hidden_sets.contains(&set.name))
                            .selected(view.selected == Some(NodeRef::Set(ix)));
                        if ui.add(row).clicked() {
                            select = Some(NodeRef::Set(ix));
                        }
                    });
                }
            });

            let hidden_count = state.hidden_systems.len() + state.hidden_sets.len();
            if hidden_count > 0 {
                Section::new(format!("Hidden ({hidden_count})"))
                    .id("schedules-hidden")
                    .show(ui, |ui| {
                        let mut names: Vec<&String> = state.hidden_systems.iter().collect();
                        names.sort();
                        for name in names {
                            Row::new().show(ui, |ui| {
                                if ui.add(Button::new("show").small()).clicked() {
                                    unhide_systems.push(name.clone());
                                }
                                let short = schedule
                                    .systems
                                    .iter()
                                    .find(|s| &s.name == name)
                                    .map_or(name.as_str(), |s| s.short_name.as_str());
                                ui.add(Text::new(short));
                            });
                        }
                        let mut names: Vec<&String> = state.hidden_sets.iter().collect();
                        names.sort();
                        for name in names {
                            Row::new().show(ui, |ui| {
                                if ui.add(Button::new("show").small()).clicked() {
                                    unhide_sets.push(name.clone());
                                }
                                ui.add(Text::new(format!("set {name}")));
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
            Section::new(format!("Components ({})", comps.len()))
                .id("schedules-components")
                .show(ui, |ui| {
                    for (name, (comp, writers, readers)) in comps {
                        if !search.is_empty() && !name.to_lowercase().contains(&search) {
                            continue;
                        }
                        let is_resource = snapshot.components[comp as usize].is_resource;
                        let row = Button::new(format!("{name}  {writers}w {readers}r"))
                            .ghost()
                            .fill_width()
                            .align(egui::Align::Min)
                            .italic(is_resource)
                            .selected(view.focus_component == Some(comp));
                        if ui
                            .add(row)
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
