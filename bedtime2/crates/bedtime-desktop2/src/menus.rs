//! Ordered menu ownership and references to opaque action entities.
use bevy::prelude::*;
use std::collections::HashSet;

pub use muda::accelerator::{Accelerator, Code, Modifiers};

/// The application's native menu bar. At most one may be active in this
/// single-window integration. Its ordered entries must be `Menu` entities.
#[derive(Component)]
pub struct MenuBar;

/// A top-level menu or submenu. Its entries are maintained by `InMenu`.
#[derive(Component, Clone, Debug)]
pub struct Menu {
    pub label: String,
}

#[derive(Component, Clone, Debug)]
pub struct MenuItem {
    pub label: String,
    pub enabled: bool,
    pub shortcut: Option<Accelerator>,
}

impl MenuItem {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            enabled: true,
            shortcut: None,
        }
    }
}

#[derive(Component)]
pub struct MenuSeparator;

/// Ownership: moving this relationship moves an entry between menus.
#[derive(Component)]
#[relationship(relationship_target = MenuEntries)]
pub struct InMenu(pub Entity);

/// Ordered membership. Use Bevy's `add_related`, `insert_related` and
/// `replace_related` operations; removing a menu also removes its owned entries.
#[derive(Component, Default)]
#[relationship_target(relationship = InMenu, linked_spawn)]
pub struct MenuEntries(Vec<Entity>);

impl MenuEntries {
    pub fn entities(&self) -> &[Entity] {
        &self.0
    }
}

/// A presentation references an action without owning its lifetime.
/// Desktop emits the target entity; it does not know how the action executes.
#[derive(Component)]
#[relationship(relationship_target = ActivationSources)]
pub struct Activates(pub Entity);

#[derive(Component, Default)]
#[relationship_target(relationship = Activates)]
pub struct ActivationSources(Vec<Entity>);

impl ActivationSources {
    pub fn entities(&self) -> &[Entity] {
        &self.0
    }
}

/// Optional shared availability on an action entity. A menu item must also be
/// locally enabled. Command owners publish this same value for their dispatcher.
#[derive(Component, Clone, Copy, Debug)]
pub struct ActionEnabled(pub bool);

impl Default for ActionEnabled {
    fn default() -> Self {
        Self(true)
    }
}

/// Immutable native input, rebuilt from ECS between application updates.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct MenuSnapshot {
    pub bar: Option<Entity>,
    pub entries: Vec<MenuNode>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum MenuNode {
    Menu {
        entity: Entity,
        label: String,
        entries: Vec<MenuNode>,
    },
    Item {
        entity: Entity,
        label: String,
        enabled: bool,
        shortcut: Option<Accelerator>,
        target: Option<Entity>,
    },
    Separator(Entity),
}

impl MenuSnapshot {
    /// Captured native bindings expire when the source is removed, disabled,
    /// detached, or retargeted. Identity is checked before emitting an action.
    #[cfg(any(target_os = "macos", test))]
    pub(crate) fn allows(&self, source: Entity, target: Entity) -> bool {
        fn contains(nodes: &[MenuNode], source: Entity, target: Entity) -> bool {
            nodes.iter().any(|node| match node {
                MenuNode::Menu { entries, .. } => contains(entries, source, target),
                MenuNode::Item {
                    entity,
                    enabled,
                    target: current,
                    ..
                } => *entity == source && *enabled && *current == Some(target),
                MenuNode::Separator(_) => false,
            })
        }
        contains(&self.entries, source, target)
    }
}

pub(crate) fn snapshot(world: &mut World) -> Result<MenuSnapshot, String> {
    let bars: Vec<_> = world
        .query_filtered::<Entity, With<MenuBar>>()
        .iter(world)
        .collect();
    let bar = match bars.as_slice() {
        [] => return Ok(MenuSnapshot::default()),
        [bar] => *bar,
        _ => return Err("Only one native MenuBar may be active".into()),
    };
    let mut visited = HashSet::from([bar]);
    let entries = read_entries(world, bar, &mut visited)?;
    if entries
        .iter()
        .any(|entry| !matches!(entry, MenuNode::Menu { .. }))
    {
        return Err("MenuBar entries must be menus".into());
    }
    Ok(MenuSnapshot {
        bar: Some(bar),
        entries,
    })
}

fn read_entries(
    world: &World,
    parent: Entity,
    visited: &mut HashSet<Entity>,
) -> Result<Vec<MenuNode>, String> {
    let mut entries = Vec::new();
    let Some(children) = world.get::<MenuEntries>(parent) else {
        return Ok(entries);
    };
    for &entity in children.entities() {
        if !visited.insert(entity) {
            return Err("Native menus must form an acyclic tree".into());
        }
        match (
            world.get::<Menu>(entity),
            world.get::<MenuItem>(entity),
            world.get::<MenuSeparator>(entity),
        ) {
            (Some(menu), None, None) => {
                if menu.label.trim().is_empty() {
                    return Err("Menu label is empty".into());
                }
                entries.push(MenuNode::Menu {
                    entity,
                    label: menu.label.clone(),
                    entries: read_entries(world, entity, visited)?,
                });
            }
            (None, Some(item), None) => {
                if item.label.trim().is_empty() {
                    return Err("Menu item label is empty".into());
                }
                if world.get::<MenuEntries>(entity).is_some() {
                    return Err("A menu item cannot contain entries".into());
                }
                let target = world.get::<Activates>(entity).map(|action| action.0);
                let enabled = item.enabled
                    && target.is_some_and(|target| {
                        world.get_entity(target).is_ok()
                            && world
                                .get::<ActionEnabled>(target)
                                .is_none_or(|enabled| enabled.0)
                    });
                entries.push(MenuNode::Item {
                    entity,
                    label: item.label.clone(),
                    enabled,
                    shortcut: item.shortcut,
                    target,
                });
            }
            (None, None, Some(_)) => {
                if world.get::<MenuEntries>(entity).is_some() {
                    return Err("A separator cannot contain entries".into());
                }
                entries.push(MenuNode::Separator(entity));
            }
            _ => {
                return Err(format!(
                    "Menu entry {entity:?} must have exactly one of Menu, MenuItem or MenuSeparator"
                ));
            }
        }
    }
    Ok(entries)
}

#[cfg(test)]
mod tests;
