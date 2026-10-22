//! Lifecycle maintenance for workspace ownership and its query markers.
use crate::*;
use bevy::{
    ecs::{lifecycle::HookContext, world::DeferredWorld},
    prelude::*,
};
use std::collections::HashSet;

pub(crate) fn workspace_selection_changed(mut world: DeferredWorld, context: HookContext) {
    if let Some(mut runtime) = world.get_mut::<crate::layout::WorkspaceRuntime>(context.entity) {
        runtime.activation = runtime
            .activation
            .checked_add(1)
            .expect("Workspace activation exhausted");
    }
    workspace_changed(world, context);
}

pub(crate) fn layout_selection_changed(mut world: DeferredWorld, context: HookContext) {
    if let Some(owner) = world
        .get::<InWorkspace>(context.entity)
        .map(|owner| owner.0)
    {
        if let Some(mut runtime) = world.get_mut::<crate::layout::WorkspaceRuntime>(owner) {
            runtime.activation = runtime
                .activation
                .checked_add(1)
                .expect("Workspace activation exhausted");
        }
    }
}

pub(crate) fn workspace_changed(mut world: DeferredWorld, context: HookContext) {
    world.commands().queue(move |world: &mut World| {
        refresh_workspace(world, context.entity);
    });
}

pub(crate) fn member_changed(mut world: DeferredWorld, context: HookContext) {
    crate::commands::handler_scope_changed(&mut world, context.entity);
    if world.get::<ActiveLayout>(context.entity).is_some() {
        if let Some(owner) = world
            .get::<InWorkspace>(context.entity)
            .map(|owner| owner.0)
        {
            if let Some(mut runtime) = world.get_mut::<crate::layout::WorkspaceRuntime>(owner) {
                runtime.activation = runtime
                    .activation
                    .checked_add(1)
                    .expect("Workspace activation exhausted");
            }
        }
    }
    world.commands().queue(move |world: &mut World| {
        refresh_members(world, context.entity);
    });
}

/// Relationship hooks queue one coherent maintenance operation. They do not
/// execute application initialization, I/O, or close negotiation.
pub(crate) fn panel_inserted(mut world: DeferredWorld, context: HookContext) {
    let entity = context.entity;
    let area = world.get::<InArea>(entity).unwrap().0;
    let changed = world
        .get::<crate::layout::PanelLocation>(entity)
        .is_none_or(|old| old.0 != Some(area));
    if let Some(mut location) = world.get_mut::<crate::layout::PanelLocation>(entity) {
        location.0 = Some(area);
    }
    world.commands().queue(move |world: &mut World| {
        refresh_members(world, entity);
        if changed && world.get::<InArea>(entity).map(|r| r.0) == Some(area) {
            if let Some(mut state) = world.get_mut::<Area>(area) {
                state.selected = Some(entity);
            }
            bump_area(world, area);
        }
    });
}

pub(crate) fn panel_replaced(mut world: DeferredWorld, context: HookContext) {
    let entity = context.entity;
    let area = world.get::<InArea>(entity).unwrap().0;
    let index = panels_in_area(&world, area)
        .iter()
        .position(|&p| p == entity)
        .unwrap_or(0);
    world.commands().queue(move |world: &mut World| {
        if world.get::<InArea>(entity).map(|r| r.0) != Some(area) {
            let tabs = panels_in_area(world, area);
            let fallback = tabs.get(index.min(tabs.len().saturating_sub(1))).copied();
            if let Some(mut state) = world.get_mut::<Area>(area) {
                if state.selected == Some(entity) {
                    state.selected = fallback;
                }
            }
            bump_area(world, area);
        }
    });
}

pub(crate) fn panel_removed(mut world: DeferredWorld, context: HookContext) {
    if let Some(mut location) = world.get_mut::<crate::layout::PanelLocation>(context.entity) {
        location.0 = None;
    }
    member_changed(world, context);
}

pub(crate) fn area_inserted(mut world: DeferredWorld, context: HookContext) {
    let entity = context.entity;
    let layout = world.get::<InLayout>(entity).unwrap().0;
    let changed = world
        .get::<crate::layout::AreaLocation>(entity)
        .is_none_or(|old| old.0 != Some(layout));
    if let Some(mut location) = world.get_mut::<crate::layout::AreaLocation>(entity) {
        location.0 = Some(layout);
    }
    world.commands().queue(move |world: &mut World| {
        refresh_members(world, entity);
        if changed && world.get::<InLayout>(entity).map(|r| r.0) == Some(layout) {
            if let Some(mut state) = world.get_mut::<Layout>(layout) {
                if state.focused_area.is_none() {
                    state.focused_area = Some(entity);
                }
            }
            topology_changed(world, layout);
        }
    });
}

pub(crate) fn area_replaced(mut world: DeferredWorld, context: HookContext) {
    let entity = context.entity;
    let layout = world.get::<InLayout>(entity).unwrap().0;
    world.commands().queue(move |world: &mut World| {
        if world.get::<InLayout>(entity).map(|r| r.0) != Some(layout) {
            let fallback = world
                .get::<LayoutAreas>(layout)
                .and_then(|areas| areas.entities().first().copied());
            if let Some(mut state) = world.get_mut::<Layout>(layout) {
                if state.focused_area == Some(entity) {
                    state.focused_area = fallback;
                }
            }
            topology_changed(world, layout);
        }
    });
}

pub(crate) fn area_removed(mut world: DeferredWorld, context: HookContext) {
    if let Some(mut location) = world.get_mut::<crate::layout::AreaLocation>(context.entity) {
        location.0 = None;
    }
    member_changed(world, context);
}

fn bump_area(world: &mut World, area: Entity) {
    if let Some(layout) = world.get::<InLayout>(area).map(|r| r.0) {
        topology_changed(world, layout);
    }
}
pub(crate) fn topology_changed(world: &mut World, layout: Entity) {
    if let Some(mut revision) = world.get_mut::<crate::layout::LayoutRevision>(layout) {
        revision.0 += 1;
    }
    crate::composition::observe_topology(world, layout);
}

fn refresh_workspace(world: &mut World, workspace: Entity) {
    if world.get_entity(workspace).is_err() {
        return;
    }
    refresh_running(world, workspace);
    let members = world
        .get::<WorkspaceMembers>(workspace)
        .map_or_else(Vec::new, |members| members.entities().to_vec());
    for member in members {
        refresh_members(world, member);
    }
}

// Workspace metadata is mutable, so scheduled reconciliation also calls this
// after in-place changes to background_updates or the internal closing flag.
pub(crate) fn refresh_running(world: &mut World, workspace: Entity) {
    let running = world.get::<Workspace>(workspace).is_some_and(|state| {
        !world
            .get::<crate::layout::WorkspaceRuntime>(workspace)
            .unwrap()
            .closing
            && (world.get::<ActiveWorkspace>(workspace).is_some() || state.background_updates)
    });
    if running != world.get::<WorkspaceRunning>(workspace).is_some() {
        if running {
            world.entity_mut(workspace).insert(WorkspaceRunning);
        } else {
            world.entity_mut(workspace).remove::<WorkspaceRunning>();
        }
    }
}

fn refresh_members(world: &mut World, entity: Entity) {
    let mut pending = vec![entity];
    // Invalid, partially composed graphs must not make lifecycle hooks recurse.
    // Scheduled reconciliation remains responsible for validating the graph.
    let mut visited = HashSet::new();
    while let Some(entity) = pending.pop() {
        if !visited.insert(entity) || world.get_entity(entity).is_err() {
            continue;
        }
        let inherited =
            if world.get::<Area>(entity).is_some() || world.get::<InLayout>(entity).is_some() {
                Some(
                    world
                        .get::<InLayout>(entity)
                        .and_then(|parent| world.get::<InWorkspace>(parent.0))
                        .map(|owner| owner.0),
                )
            } else if world.get::<PanelInstance>(entity).is_some()
                || world.get::<InArea>(entity).is_some()
            {
                Some(
                    world
                        .get::<InArea>(entity)
                        .and_then(|parent| world.get::<InWorkspace>(parent.0))
                        .map(|owner| owner.0),
                )
            } else if let Some(panel) = world.get::<InPanel>(entity) {
                Some(world.get::<InWorkspace>(panel.0).map(|owner| owner.0))
            } else {
                None
            };
        if let Some(owner) = inherited {
            if world.get::<InWorkspace>(entity).map(|owner| owner.0) != owner {
                if let Some(owner) = owner {
                    world.entity_mut(entity).insert(InWorkspace(owner));
                } else {
                    world.entity_mut(entity).remove::<InWorkspace>();
                }
            }
        }
        let owner = world.get::<InWorkspace>(entity).map(|owner| owner.0);
        let active = owner.is_some_and(|owner| {
            world.get::<Workspace>(owner).is_some()
                && world.get::<ActiveWorkspace>(owner).is_some()
                && world
                    .get::<crate::layout::WorkspaceRuntime>(owner)
                    .is_some_and(|runtime| !runtime.closing)
        });
        let running = owner.is_some_and(|owner| world.get::<WorkspaceRunning>(owner).is_some());
        if active != world.get::<InActiveWorkspace>(entity).is_some() {
            if active {
                world.entity_mut(entity).insert(InActiveWorkspace);
            } else {
                world.entity_mut(entity).remove::<InActiveWorkspace>();
            }
        }
        if running != world.get::<InRunningWorkspace>(entity).is_some() {
            if running {
                world.entity_mut(entity).insert(InRunningWorkspace);
            } else {
                world.entity_mut(entity).remove::<InRunningWorkspace>();
            }
        }
        if let Some(areas) = world.get::<LayoutAreas>(entity) {
            pending.extend_from_slice(areas.entities());
        }
        if let Some(panels) = world.get::<AreaPanels>(entity) {
            pending.extend_from_slice(panels.entities());
        }
        if let Some(parts) = world.get::<PanelCommandParts>(entity) {
            pending.extend_from_slice(parts.entities());
        }
    }
}

#[cfg(test)]
mod tests;
