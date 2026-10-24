//! Immediate layout selection and attachment bookkeeping.
use crate::*;
use bevy::{
    ecs::{lifecycle::HookContext, world::DeferredWorld},
    prelude::*,
};
pub(crate) fn layout_selection_changed(mut world: DeferredWorld, _: HookContext) {
    let mut revision = world.resource_mut::<crate::lifecycle::LayoutSelectionRevision>();
    revision.0 = revision
        .0
        .checked_add(1)
        .expect("Layout selection revision exhausted");
}
pub(crate) fn member_changed(mut world: DeferredWorld, context: HookContext) {
    crate::commands::handler_scope_changed(&mut world, context.entity);
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

pub(crate) fn layout_added(mut world: DeferredWorld, context: HookContext) {
    world
        .resource_mut::<crate::layout::LayoutOrder>()
        .0
        .push(context.entity);
}
pub(crate) fn layout_removed(mut world: DeferredWorld, context: HookContext) {
    world
        .resource_mut::<crate::layout::LayoutOrder>()
        .0
        .retain(|e| *e != context.entity);
}
