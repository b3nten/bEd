//! Abstract ordered menus. Native and egui presenters use the same snapshots.
use crate::{Action, ActionId, ActionRequest, InvocationContext, Invokes, KeyChord};
use crate::{Workbench, WorkbenchWorldExt};
use bevy::prelude::*;
use std::collections::HashSet;

#[derive(Component)]
pub struct MenuBar;

#[derive(Component, Clone, Debug)]
pub struct Menu {
    pub label: String,
}

/// A menu entry. With `Invokes` it runs that command, and is enabled when the
/// command is available in the menu's context.
#[derive(Component, Clone, Debug)]
pub struct MenuItem {
    pub label: String,
}

impl MenuItem {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
        }
    }

    /// An item running command `C`, labelled with the command's label.
    pub fn action<C: Action>() -> (Self, Invokes) {
        (Self::new(C::LABEL), Invokes::of::<C>())
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
        target: Option<ActionId>,
        context: InvocationContext,
    },
    Separator(Entity),
}

impl MenuSnapshot {
    /// Revalidate a captured native binding against today's ownership, action
    /// type and availability. Removed or retargeted sources cannot invoke it.
    pub fn request(&self, source: Entity, target: ActionId) -> Option<ActionRequest> {
        fn find(nodes: &[MenuNode], source: Entity, target: ActionId) -> Option<ActionRequest> {
            nodes.iter().find_map(|node| match node {
                MenuNode::Menu { entries, .. } => find(entries, source, target.clone()),
                MenuNode::Item {
                    entity,
                    enabled,
                    target: current,
                    context,
                    ..
                } if *entity == source && *enabled && *current == Some(target.clone()) => {
                    Some(ActionRequest {
                        command: target.clone(),
                        context: *context,
                        source: Some(source),
                    })
                }
                _ => None,
            })
        }
        find(&self.entries, source, target)
    }
}

impl Workbench {
    /// Snapshot a menu bar or contextual menu root. No native window is required.
    /// InvocationContext components on the root or its entries override `context`.
    pub fn menu_snapshot(
        &mut self,
        root: Entity,
        context: InvocationContext,
    ) -> Result<MenuSnapshot, String> {
        let world = &mut self.world;
        if world.get::<MenuBar>(root).is_none() && world.get::<Menu>(root).is_none() {
            return Err("Menu root is unavailable".into());
        }
        let context = capture_context(world, context);
        let mut visited = HashSet::from([root]);
        let entries = read_entries(world, root, &mut visited, context)?;
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
}

fn capture_context(world: &mut World, context: InvocationContext) -> InvocationContext {
    match context {
        InvocationContext::Focused => crate::actions::focused_panel(world)
            .map_or(InvocationContext::Global, InvocationContext::Panel),
        explicit => explicit,
    }
}

fn read_entries(
    world: &mut World,
    parent: Entity,
    visited: &mut HashSet<Entity>,
    inherited: InvocationContext,
) -> Result<Vec<MenuNode>, String> {
    let context = world
        .get::<InvocationContext>(parent)
        .copied()
        .unwrap_or(inherited);
    let context = capture_context(world, context);
    let children = world
        .get::<MenuEntries>(parent)
        .map_or_else(Vec::new, |e| e.0.clone());
    let mut entries = Vec::new();
    for entity in children {
        if !visited.insert(entity) {
            return Err("Menus must form an acyclic tree".into());
        }
        let context = world
            .get::<InvocationContext>(entity)
            .copied()
            .unwrap_or(context);
        let context = capture_context(world, context);
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
                    entries: read_entries(world, entity, visited, context)?,
                });
            }
            (None, Some(item), false) => {
                if item.label.trim().is_empty() || world.get::<MenuEntries>(entity).is_some() {
                    return Err("Menu item requires a label and cannot contain entries".into());
                }
                let target = world.get::<Invokes>(entity).map(|a| a.0.clone());
                let enabled = target
                    .as_ref()
                    .is_some_and(|t| world.workbench().action_available(t, context));
                let shortcut = target
                    .as_ref()
                    .and_then(|t| world.workbench().shortcut_for(t, context));
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
