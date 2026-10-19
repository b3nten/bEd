//! Lifecycle maintenance for workspace ownership and its query markers.
use crate::*;
use bevy::{
    ecs::{lifecycle::HookContext, world::DeferredWorld},
    prelude::*,
};
use std::collections::HashSet;

pub(crate) fn workspace_changed(mut world: DeferredWorld, context: HookContext) {
    world.commands().queue(move |world: &mut World| {
        refresh_workspace(world, context.entity);
    });
}

pub(crate) fn member_changed(mut world: DeferredWorld, context: HookContext) {
    world.commands().queue(move |world: &mut World| {
        refresh_members(world, context.entity);
    });
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
    }
}

#[cfg(test)]
mod tests;
