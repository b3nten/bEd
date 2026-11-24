//! Plain list of a schedule's systems in executor order.

use bedtime_ui::egui;
use bedtime_ui::{Button, Row, Section, Text};

use super::{ViewState, details::access_line, named_sets, search_matches, style};
use crate::snapshot::{AppSnapshot, NodeRef, OrderKind, ScheduleSnapshot, SysIx, SystemSnapshot};

pub fn system_list(
    ui: &mut egui::Ui,
    snapshot: &AppSnapshot,
    schedule: &ScheduleSnapshot,
    view: &mut ViewState,
) {
    let sync_edges = schedule
        .flat_edges
        .iter()
        .filter(|e| e.kind == OrderKind::SyncPoint)
        .count();
    ui.add(Text::new(format!(
        "{} systems · {} sets · {} ordering edges ({} via sync points) · {}",
        schedule.systems.len(),
        schedule.sets.iter().filter(|s| s.is_named()).count(),
        schedule.flat_edges.len(),
        sync_edges,
        if schedule.conflicts_available {
            format!("{} ambiguities", schedule.conflicts.len())
        } else {
            "ambiguities unavailable".to_string()
        }
    )));
    let order: Vec<SysIx> = if schedule.exec_order.is_empty() {
        (0..schedule.systems.len() as SysIx).collect()
    } else {
        schedule.exec_order.clone()
    };
    let search = view.search.to_lowercase();
    let mut select = None;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for ix in order {
                let system = &schedule.systems[ix as usize];
                if (system.flags.internal && !view.show_internal)
                    || (system.flags.is_self && !view.show_self)
                    || (system.flags.apply_deferred && !view.show_sync)
                    || !search_matches(snapshot, system, &search)
                {
                    continue;
                }
                if system_entry(ui, snapshot, schedule, ix, system, view.selected) {
                    select = Some(NodeRef::System(ix));
                }
            }
        });
    if let Some(node) = select {
        view.select(Some(node));
    }
}

/// Returns `true` when the entry was clicked to select it.
fn system_entry(
    ui: &mut egui::Ui,
    snapshot: &AppSnapshot,
    schedule: &ScheduleSnapshot,
    ix: SysIx,
    system: &SystemSnapshot,
    selected: Option<NodeRef>,
) -> bool {
    let mut header = if system.flags.apply_deferred {
        "⏸ ApplyDeferred".to_string()
    } else {
        system.short_name.clone()
    };
    if system.flags.exclusive {
        header.push_str("  [exclusive]");
    }
    if system.flags.deferred {
        header.push_str("  [deferred]");
    }
    if !system.conditions.is_empty() {
        header.push_str("  [run_if]");
    }
    let mut clicked = false;
    let mut section = Section::new(header).id(("list", ix));
    if selected == Some(NodeRef::System(ix)) {
        section = section.open();
    }
    section.show(ui, |ui| {
        if ui.add(Button::new("details").small()).clicked() {
            clicked = true;
        }
        ui.add(Text::new(&system.name));
        let sets = named_sets(schedule, system);
        if !sets.is_empty() {
            ui.add(Text::new(format!("sets: {}", sets.join(", "))));
        }
        for condition in &system.conditions {
            Row::new().wrap().show(ui, |ui| {
                ui.add(Text::new("run_if"));
                ui.add(Text::new(&condition.short_name));
                access_line(
                    ui,
                    snapshot,
                    &condition.queries,
                    &condition.resources,
                    &mut None,
                );
            });
        }
        if !system.flags.access_available {
            ui.add(
                Text::new(if system.flags.exclusive {
                    "exclusive: accesses the whole World"
                } else {
                    "access not captured"
                })
                .color(style::filter()),
            );
        }
        for query in &system.queries {
            Row::new().wrap().show(ui, |ui| {
                ui.add(Text::new("query"));
                access_line(ui, snapshot, std::slice::from_ref(query), &[], &mut None);
            });
        }
        if !system.resources.is_empty() {
            Row::new().wrap().show(ui, |ui| {
                ui.add(Text::new("resources"));
                access_line(ui, snapshot, &[], &system.resources, &mut None);
            });
        }
    });
    clicked
}
