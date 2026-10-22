//! Abstract ordered menus. Native and egui presenters use the same snapshots.
use crate::{
    ContextCommandId, ContextCommandRequest, InvocationContext, Invokes, KeyChord, WorkspaceId,
    commands::shortcut_for,
};
use bevy::prelude::*;
use std::collections::HashSet;

#[derive(Component)]
pub struct MenuBar;

#[derive(Component, Clone, Debug)]
pub struct Menu {
    pub label: String,
}

#[derive(Component, Clone, Debug)]
pub struct MenuItem {
    pub label: String,
    pub enabled: bool,
}

impl MenuItem {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            enabled: true,
        }
    }
}

#[derive(Component)]
pub struct MenuSeparator;

#[derive(Component)]
#[relationship(relationship_target = MenuEntries)]
pub struct InMenu(pub Entity);

#[derive(Component, Default)]
#[relationship_target(relationship = InMenu, linked_spawn)]
pub struct MenuEntries(Vec<Entity>);

impl MenuEntries {
    pub fn entities(&self) -> &[Entity] {
        &self.0
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MenuSnapshot {
    pub root: Option<Entity>,
    pub entries: Vec<MenuNode>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuNode {
    Menu {
        entity: Entity,
        label: String,
        entries: Vec<MenuNode>,
    },
    Item {
        entity: Entity,
        label: String,
        enabled: bool,
        shortcut: Option<KeyChord>,
        target: Option<ContextCommandId>,
        context: InvocationContext,
    },
    Separator(Entity),
}

impl MenuSnapshot {
    /// Revalidate a captured native binding against today's ownership, action
    /// type and availability. Removed or retargeted sources cannot invoke it.
    pub fn request(
        &self,
        source: Entity,
        target: ContextCommandId,
    ) -> Option<ContextCommandRequest> {
        fn find(
            nodes: &[MenuNode],
            source: Entity,
            target: ContextCommandId,
        ) -> Option<ContextCommandRequest> {
            nodes.iter().find_map(|node| match node {
                MenuNode::Menu { entries, .. } => find(entries, source, target.clone()),
                MenuNode::Item {
                    entity,
                    enabled,
                    target: current,
                    context,
                    ..
                } if *entity == source && *enabled && *current == Some(target.clone()) => {
                    Some(ContextCommandRequest {
                        command: target.clone(),
                        source: Some(source),
                        context: *context,
                    })
                }
                _ => None,
            })
        }
        find(&self.entries, source, target)
    }
}

pub(crate) fn optional_menu_workspace(world: &World, root: Entity) -> Option<WorkspaceId> {
    crate::workspaces::owner(world, root)
        .or_else(|| {
            world
                .get::<crate::InPanel>(root)
                .and_then(|panel| crate::workspaces::owner(world, panel.0))
        })
        .or_else(|| crate::workspaces::selected(world))
}

/// Snapshot a menu bar or contextual menu root. No native window is required.
/// Root/descendant InvocationContext components override the supplied context.
pub fn menu_snapshot(
    world: &mut World,
    root: Entity,
    context: InvocationContext,
) -> Result<MenuSnapshot, String> {
    if world.get::<MenuBar>(root).is_none() && world.get::<Menu>(root).is_none() {
        return Err("Menu root is unavailable".into());
    }
    let workspace = optional_menu_workspace(world, root);
    let context = world
        .get::<crate::InPanel>(root)
        .map_or(context, |p| InvocationContext::Panel(p.0));
    let context = capture_context(world, workspace, context);
    let mut visited = HashSet::from([root]);
    let entries = read_entries(world, workspace, root, &mut visited, context)?;
    if world.get::<MenuBar>(root).is_some()
        && entries.iter().any(|e| !matches!(e, MenuNode::Menu { .. }))
    {
        return Err("MenuBar entries must be menus".into());
    }
    Ok(MenuSnapshot {
        root: Some(root),
        entries,
    })
}

fn capture_context(
    world: &mut World,
    workspace: Option<WorkspaceId>,
    context: InvocationContext,
) -> InvocationContext {
    match context {
        InvocationContext::Focused => workspace
            .and_then(|w| crate::commands::focused_panel_in(world, w))
            .map_or(InvocationContext::Global, InvocationContext::Panel),
        explicit => explicit,
    }
}

fn read_entries(
    world: &mut World,
    workspace: Option<WorkspaceId>,
    parent: Entity,
    visited: &mut HashSet<Entity>,
    inherited: InvocationContext,
) -> Result<Vec<MenuNode>, String> {
    let context = world
        .get::<InvocationContext>(parent)
        .copied()
        .unwrap_or(inherited);
    let context = capture_context(world, workspace, context);
    let children = world
        .get::<MenuEntries>(parent)
        .map_or_else(Vec::new, |e| e.0.clone());
    let mut entries = Vec::new();
    for entity in children {
        // Panel-owned entries participate only in that panel's menu context.
        // Both presenters use this snapshot, including native activation checks.
        if world
            .get::<crate::InPanel>(entity)
            .is_some_and(|owner| context != InvocationContext::Panel(owner.0))
        {
            continue;
        }
        if !visited.insert(entity) {
            return Err("Menus must form an acyclic tree".into());
        }
        let context = world
            .get::<InvocationContext>(entity)
            .copied()
            .unwrap_or(context);
        let context = capture_context(world, workspace, context);
        match (
            world.get::<Menu>(entity).cloned(),
            world.get::<MenuItem>(entity).cloned(),
            world.get::<MenuSeparator>(entity).is_some(),
        ) {
            (Some(menu), None, false) => {
                if menu.label.trim().is_empty() {
                    return Err("Menu label is empty".into());
                }
                entries.push(MenuNode::Menu {
                    entity,
                    label: menu.label,
                    entries: read_entries(world, workspace, entity, visited, context)?,
                });
            }
            (None, Some(item), false) => {
                if item.label.trim().is_empty() || world.get::<MenuEntries>(entity).is_some() {
                    return Err("Menu item requires a label and cannot contain entries".into());
                }
                let target = world.get::<Invokes>(entity).map(|a| a.0.clone());
                let enabled = item.enabled
                    && target.clone().is_some_and(|t| {
                        crate::commands::resolve(
                            world,
                            workspace,
                            ContextCommandRequest {
                                command: t,
                                source: Some(entity),
                                context,
                            },
                        )
                        .1
                        .is_ok()
                    });
                let shortcut = target
                    .clone()
                    .and_then(|t| shortcut_for(world, workspace, t, context));
                entries.push(MenuNode::Item {
                    entity,
                    label: item.label,
                    enabled,
                    shortcut,
                    target,
                    context,
                });
            }
            (None, None, true) => {
                if world.get::<MenuEntries>(entity).is_some() {
                    return Err("A separator cannot contain entries".into());
                }
                entries.push(MenuNode::Separator(entity));
            }
            _ => {
                return Err(format!(
                    "Menu entry {entity:?} must have exactly one presentation kind"
                ));
            }
        }
    }
    Ok(entries)
}
#[cfg(test)]
mod tests;
