//! Scheduled graph validation, panel initialization and layout bookkeeping.
use crate::layout::WorkspaceRuntime;
use crate::*;
use bevy::prelude::*;
use std::collections::{HashMap, HashSet};

/// Optional, runtime-only tab label appearance on a panel item.
///
/// Without this component, normal tabs inherit the theme unchanged and preview
/// tabs use italic text at 90% opacity. An explicit component replaces both
/// defaults without changing [`PanelInstance::preview`] or keep/move behavior.
/// Font size, color, selection and hover emphasis remain theme-owned. Workspace
/// and layout snapshots do not capture this component; applications reapply it.
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
    selections: HashMap<Entity, (bool, Option<Entity>)>,
    topology: HashMap<Entity, Vec<(Entity, AreaRect, Vec<Entity>)>>,
}

/// Resolve a composed graph immediately, or let WorkspaceSystems::Prepare do it.
/// Required components supply local defaults; this phase only resolves references,
/// initializes registered panels, and updates layout revisions. Lifecycle hooks
/// maintain workspace membership and selection markers as commands are applied.
/// Zero workspaces, layouts, areas or panels is valid. Conflicting live references
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
    let roots: Vec<_> = world
        .query_filtered::<Entity, With<Workspace>>()
        .iter(world)
        .collect();
    let active: Vec<_> = world
        .query_filtered::<Entity, With<ActiveWorkspace>>()
        .iter(world)
        .collect();
    if active.len() > 1 {
        return Err("Multiple ActiveWorkspace entities".into());
    }
    if active.iter().any(|e| world.get::<Workspace>(*e).is_none()) {
        return Err("ActiveWorkspace requires Workspace".into());
    }
    let layouts: Vec<_> = world
        .query_filtered::<Entity, With<Layout>>()
        .iter(world)
        .collect();
    let mut selected = HashMap::new();
    for &layout in &layouts {
        let workspace = world
            .get::<InWorkspace>(layout)
            .ok_or("Layout requires InWorkspace")?
            .0;
        if world.get::<Workspace>(workspace).is_none() {
            return Err("Layout references a missing Workspace".into());
        }
        if world.get::<ActiveLayout>(layout).is_some()
            && selected.insert(workspace, layout).is_some()
        {
            return Err("Multiple ActiveLayout entities in one workspace".into());
        }
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
        let workspace = world.get::<InWorkspace>(layout).unwrap().0;
        if world.get::<InWorkspace>(area).map(|w| w.0) != Some(workspace) {
            return Err("Area workspace membership disagrees with its layout".into());
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
        if focused.is_some_and(|e| world.get_entity(e).is_ok() && !areas.contains(&e)) {
            let was_member = world
                .resource::<ReconciledState>()
                .topology
                .get(&layout)
                .is_some_and(|old| old.iter().any(|(area, _, _)| Some(*area) == focused));
            if !was_member {
                return Err("Focused area belongs to another layout".into());
            }
        }
        if focused.is_none_or(|e| !areas.contains(&e)) {
            let next = areas.first().copied();
            if focused != next {
                world.get_mut::<Layout>(layout).unwrap().focused_area = next;
            }
        }
    }
    let kinds: HashMap<_, _> = world
        .query::<(Entity, &PanelType)>()
        .iter(world)
        .map(|(e, p)| (p.key.to_owned(), e))
        .collect();
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
        let workspace = world.get::<InWorkspace>(area).unwrap().0;
        if world.get::<InWorkspace>(panel).map(|w| w.0) != Some(workspace) {
            return Err("Panel workspace membership disagrees with its area".into());
        }
        let instance = world.get::<PanelInstance>(panel).unwrap();
        if let Some(initialized) = world.get::<crate::layout::InitializedPanel>(panel) {
            if world.get::<PanelType>(initialized.0).map(|p| p.key) != Some(instance.key.as_str()) {
                return Err("Changing the type of an initialized panel is unsupported".into());
            }
        } else if !instance.key.is_empty() {
            let kind = *kinds
                .get(&instance.key)
                .ok_or_else(|| format!("Unregistered panel type: {}", instance.key))?;
            crate::layout::initialize_panel(world, panel, kind);
        }
    }
    for area in areas {
        let selected = world.get::<Area>(area).unwrap().selected;
        let tabs = panels_in_area(world, area);
        if selected.is_some_and(|e| world.get_entity(e).is_ok() && !tabs.contains(&e)) {
            let was_member = world
                .resource::<ReconciledState>()
                .topology
                .values()
                .flatten()
                .find(|(old_area, _, _)| *old_area == area)
                .is_some_and(|(_, _, old_tabs)| selected.is_some_and(|e| old_tabs.contains(&e)));
            if !was_member {
                return Err("Selected panel belongs to another area".into());
            }
        }
        if selected.is_none_or(|e| !tabs.contains(&e)) {
            let next = tabs.first().copied();
            if selected != next {
                world.get_mut::<Area>(area).unwrap().selected = next;
            }
        }
    }
    // Direct composition must invalidate admitted layout gestures just as the
    // existing editing operations do. Compare topology, excluding focus/hover.
    let topology: HashMap<_, _> = layouts
        .iter()
        .map(|&layout| {
            let areas = world
                .get::<LayoutAreas>(layout)
                .map_or(&[][..], |a| a.entities());
            (
                layout,
                areas
                    .iter()
                    .map(|&area| {
                        (
                            area,
                            world.get::<Area>(area).unwrap().rect,
                            panels_in_area(world, area).to_vec(),
                        )
                    })
                    .collect::<Vec<_>>(),
            )
        })
        .collect();
    let changed: Vec<_> = topology
        .iter()
        .filter_map(|(&layout, shape)| {
            world
                .resource::<ReconciledState>()
                .topology
                .get(&layout)
                .is_some_and(|old| old != shape)
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
    let mut current = HashMap::new();
    for workspace in roots {
        let closing = world.get::<WorkspaceRuntime>(workspace).unwrap().closing;
        let is_active = active.first() == Some(&workspace) && !closing;
        current.insert(workspace, (is_active, selected.get(&workspace).copied()));
        crate::membership::refresh_running(world, workspace);
    }
    let members: Vec<_> = world
        .query_filtered::<Entity, Or<(
            With<InWorkspace>,
            With<InActiveWorkspace>,
            With<InRunningWorkspace>,
        )>>()
        .iter(world)
        .collect();
    for member in members {
        let owner = world.get::<InWorkspace>(member).map(|w| w.0);
        if owner.is_some_and(|e| !current.contains_key(&e)) {
            return Err("InWorkspace references a missing Workspace".into());
        }
        let is_active = owner.is_some_and(|e| current[&e].0);
        let running = owner.is_some_and(|e| world.get::<WorkspaceRunning>(e).is_some());
        if is_active != world.get::<InActiveWorkspace>(member).is_some() {
            return Err("InActiveWorkspace disagrees with workspace selection".into());
        }
        if running != world.get::<InRunningWorkspace>(member).is_some() {
            return Err("InRunningWorkspace disagrees with workspace running state".into());
        }
    }
    let previous = &world.resource::<ReconciledState>().selections;
    let changed: HashSet<_> = current
        .iter()
        .filter_map(|(&e, state)| (previous.get(&e).is_some_and(|old| old != state)).then_some(e))
        .collect();
    for entity in changed {
        world
            .get_mut::<WorkspaceRuntime>(entity)
            .unwrap()
            .activation += 1;
    }
    world.resource_mut::<ReconciledState>().selections = current;
    crate::documents::reconcile_documents(world)?;
    Ok(())
}
