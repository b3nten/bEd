//! Graph validation and layout bookkeeping; composition is complete before this runs.
use crate::*;
use bevy::prelude::*;
use std::collections::HashMap;

/// Optional, runtime-only tab label appearance on a panel item.
///
/// Without this component, normal tabs inherit the theme unchanged and preview
/// tabs use italic text at 90% opacity. An explicit component replaces both
/// defaults without changing [`PanelInstance::preview`] or keep/move behavior.
/// Font size, color, selection and hover emphasis remain theme-owned. Reflected snapshots do not capture this component; applications reapply it.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct PanelTabStyle {
    pub italic: bool,
    /// Multiplier on the themed label's opacity, not its icon or controls.
    /// Rendering clamps finite values to `0.0..=1.0`; nonfinite values use `1.0`.
    pub text_opacity: f32,
}

impl Default for PanelTabStyle {
    fn default() -> Self {
        Self {
            italic: false,
            text_opacity: 1.0,
        }
    }
}

#[derive(Resource, Default)]
pub(crate) struct ReconciledState {
    topology: HashMap<Entity, Vec<(Entity, AreaRect, Vec<Entity>)>>,
}

fn layout_topology(world: &World, layout: Entity) -> Vec<(Entity, AreaRect, Vec<Entity>)> {
    world
        .get::<LayoutAreas>(layout)
        .map_or(&[][..], |areas| areas.entities())
        .iter()
        .filter_map(|&area| {
            world
                .get::<Area>(area)
                .map(|state| (area, state.rect, panels_in_area(world, area).to_vec()))
        })
        .collect()
}

// Attach/detach hooks already advance revisions. Their queued maintenance can
// precede Bevy's target-list update, so observing an added/removed member must
// not count the same transition twice. Detect only edits outside those hooks:
// changed geometry or reordered members retained in both observations.
fn untracked_topology_change(
    old: &[(Entity, AreaRect, Vec<Entity>)],
    new: &[(Entity, AreaRect, Vec<Entity>)],
) -> bool {
    let old_areas: Vec<_> = old
        .iter()
        .map(|row| row.0)
        .filter(|area| new.iter().any(|row| row.0 == *area))
        .collect();
    let new_areas: Vec<_> = new
        .iter()
        .map(|row| row.0)
        .filter(|area| old.iter().any(|row| row.0 == *area))
        .collect();
    if old_areas != new_areas {
        return true;
    }
    new.iter().any(|(area, rect, tabs)| {
        old.iter()
            .find(|row| row.0 == *area)
            .is_some_and(|(_, old_rect, old_tabs)| {
                if rect != old_rect {
                    return true;
                }
                let before: Vec<_> = old_tabs.iter().filter(|tab| tabs.contains(tab)).collect();
                let after: Vec<_> = tabs.iter().filter(|tab| old_tabs.contains(tab)).collect();
                before != after
            })
    })
}

/// Known structural transitions acknowledge their final shape when they advance
/// the revision. Scheduled validation must not invalidate the same transition a
/// second time after callers have already captured a fresh stamp.
pub(crate) fn observe_topology(world: &mut World, layout: Entity) {
    if !world.contains_resource::<ReconciledState>() {
        return;
    }
    let shape = layout_topology(world, layout);
    world
        .resource_mut::<ReconciledState>()
        .topology
        .insert(layout, shape);
}

/// Resolve a composed graph immediately, or let SessionSystems::Prepare do it.
/// Required components supply local defaults; this phase validates references
/// and updates layout revisions. Lifecycle hooks
/// maintain attachment order and selection markers as commands are applied.
/// Zero layouts, areas or panels is valid. Conflicting live references
/// and duplicate active selections return an error rather than choosing a winner.
pub fn reconcile_workbench(world: &mut World) -> Result<(), String> {
    world.flush();
    if world
        .query_filtered::<Entity, (With<InLayout>, Without<Area>)>()
        .iter(world)
        .next()
        .is_some()
    {
        return Err("InLayout requires Area".into());
    }
    if world
        .query_filtered::<Entity, (With<InArea>, Without<PanelInstance>)>()
        .iter(world)
        .next()
        .is_some()
    {
        return Err("InArea requires PanelInstance".into());
    }
    let layouts: Vec<_> = world
        .query_filtered::<Entity, With<Layout>>()
        .iter(world)
        .collect();
    let selected = layouts
        .iter()
        .filter(|&&e| world.get::<ActiveLayout>(e).is_some())
        .count();
    if selected > 1 {
        return Err("Multiple ActiveLayout entities".into());
    }
    for entity in world
        .query_filtered::<Entity, With<ActiveLayout>>()
        .iter(world)
    {
        if world.get::<Layout>(entity).is_none() {
            return Err("ActiveLayout requires Layout".into());
        }
    }
    let areas: Vec<_> = world
        .query_filtered::<Entity, With<Area>>()
        .iter(world)
        .collect();
    for &area in &areas {
        let layout = world
            .get::<InLayout>(area)
            .ok_or("Area requires InLayout")?
            .0;
        if world.get::<Layout>(layout).is_none() {
            return Err("Area references a missing Layout".into());
        }
    }
    for &layout in &layouts {
        let areas = world
            .get::<LayoutAreas>(layout)
            .map_or(&[][..], |a| a.entities());
        if !areas.is_empty() {
            crate::geometry::Layout::from_areas(
                areas
                    .iter()
                    .enumerate()
                    .map(|(i, &e)| crate::geometry::Area {
                        id: i as u32 + 1,
                        rect: world.get::<Area>(e).expect("InLayout requires Area").rect,
                    })
                    .collect(),
            )?;
        }
        let focused = world.get::<Layout>(layout).unwrap().focused_area;
        if focused.is_some_and(|entity| !areas.contains(&entity)) {
            return Err("Focused area belongs to another layout or no longer exists".into());
        }
    }
    let panels: Vec<_> = world
        .query_filtered::<Entity, With<PanelInstance>>()
        .iter(world)
        .collect();
    for panel in panels {
        let area = world
            .get::<InArea>(panel)
            .ok_or("PanelInstance requires InArea")?
            .0;
        if world.get::<Area>(area).is_none() {
            return Err("Panel references a missing Area".into());
        }
    }
    for area in areas {
        let selected = world.get::<Area>(area).unwrap().selected;
        let tabs = panels_in_area(world, area);
        if selected.is_some_and(|entity| !tabs.contains(&entity)) {
            return Err("Selected panel belongs to another area or no longer exists".into());
        }
    }
    // Direct composition must invalidate admitted layout gestures just as the
    // existing editing operations do. Compare topology, excluding focus/hover.
    let topology: HashMap<_, _> = layouts
        .iter()
        .map(|&layout| (layout, layout_topology(world, layout)))
        .collect();
    let changed: Vec<_> = topology
        .iter()
        .filter_map(|(&layout, shape)| {
            world
                .resource::<ReconciledState>()
                .topology
                .get(&layout)
                .is_some_and(|old| untracked_topology_change(old, shape))
                .then_some(layout)
        })
        .collect();
    for layout in changed {
        world
            .get_mut::<crate::layout::LayoutRevision>(layout)
            .unwrap()
            .0 += 1;
    }
    world.resource_mut::<ReconciledState>().topology = topology;
    crate::documents::reconcile_documents(world)?;
    Ok(())
}
