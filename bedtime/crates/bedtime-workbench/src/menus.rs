//! Menus as entities: a menu bar or menu holds entries (`InMenu`) that are
//! submenus, items or separators. An item `Invokes` an action, and is enabled
//! when the action is available in its context. A snapshot reads a menu tree
//! with that state filled in; the egui renderer (`ui` feature) and the native
//! menu bar (`native` feature) both draw snapshots.
//!
//! An entry can belong to a panel, and is left out of snapshots otherwise:
//! `ForPanel::of::<P>()` while a `P` is open in the active layout,
//! `ForPanel::instance(panel)` while that panel is focused (it goes when the
//! panel does). A whole menu can belong to one the same way.
#[cfg(feature = "native")]
mod native;
mod standard;
#[cfg(test)]
mod tests;
#[cfg(feature = "ui")]
pub mod ui;

#[cfg(feature = "native")]
pub use native::{NativeMenuBar, NativeMenuError, NativeMenuPlugin, NativeMenuSystems};
pub use standard::{
    MenuContributions, MenusAppExt, StandardMenu, StandardMenus, StandardMenusPlugin,
};

use crate::actions::{
    Action, ActionId, ActionRequest, ActionsWorldExt, InvocationContext, Invokes, KeyChord,
};
use bevy::prelude::*;
use std::collections::HashSet;

#[derive(Component)]
pub struct MenuBar;

#[derive(Component, Clone, Debug)]
pub struct Menu {
    pub label: String,
}

/// A menu entry. With `Invokes` it runs that action, and is enabled when the
/// action is available in the menu's context.
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

    /// An item running action `A`, labelled with the action's label.
    pub fn action<A: Action>() -> (Self, Invokes) {
        (Self::new(A::LABEL), Invokes::of::<A>())
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

/// On a menu entry: shown only while a panel of this type is open in the
/// active layout. `ForPanel::instance` makes one for a single panel instead.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct ForPanel(std::any::TypeId);

impl ForPanel {
    /// Shown while a panel of type `P` is open in the active layout.
    pub fn of<P: 'static>() -> Self {
        Self(std::any::TypeId::of::<P>())
    }
    /// Shown while `panel` is focused, and despawned with it.
    pub fn instance(panel: Entity) -> ForPanelInstance {
        ForPanelInstance(panel)
    }
}

/// On a menu entry: shown only while this panel is focused; it goes when the
/// panel does (`ForPanel::instance`).
#[derive(Component, Debug)]
#[relationship(relationship_target = PanelMenuEntries)]
pub struct ForPanelInstance(pub Entity);

/// On a panel: the menu entries that are its own.
#[derive(Component, Debug, Default)]
#[relationship_target(relationship = ForPanelInstance, linked_spawn)]
pub struct PanelMenuEntries(Vec<Entity>);

/// Whether `entry` belongs in menus now: always, unless it's a panel's.
fn shown(world: &World, entry: Entity) -> bool {
    use bedtime_ui::layouts::{LayoutsWorldExt, PanelInstance};
    let active = world.layouts().active();
    if let Some(ForPanel(kind)) = world.get::<ForPanel>(entry) {
        let open =
            active.is_some_and(|layout| {
                world.layouts().panels(layout).into_iter().any(|panel| {
                    world.get::<PanelInstance>(panel).and_then(|p| p.kind) == Some(*kind)
                })
            });
        if !open {
            return false;
        }
    }
    if let Some(ForPanelInstance(panel)) = world.get::<ForPanelInstance>(entry) {
        let focused = active.and_then(|layout| world.layouts().focused_panel(layout));
        if focused != Some(*panel) {
            return false;
        }
    }
    true
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
                        action: target.clone(),
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

/// Reading menus: `world.menus()`.
#[repr(transparent)]
pub struct Menus {
    world: World,
}

/// `world.menus()`.
pub trait MenusWorldExt {
    fn menus(&self) -> &Menus;
}

impl MenusWorldExt for World {
    fn menus(&self) -> &Menus {
        // SAFETY: Menus is a repr(transparent) wrapper of World.
        unsafe { &*(self as *const World).cast::<Menus>() }
    }
}

impl Menus {
    /// Snapshot a menu bar or contextual menu root. No native window is
    /// required. `InvocationContext` components on the root or its entries
    /// override `context`.
    pub fn snapshot(
        &self,
        root: Entity,
        context: InvocationContext,
    ) -> Result<MenuSnapshot, String> {
        let world = &self.world;
        if world.get::<MenuBar>(root).is_none() && world.get::<Menu>(root).is_none() {
            return Err("Menu root is unavailable".into());
        }
        let context = capture_context(world, context);
        let mut visited = HashSet::from([root]);
        let mut entries = read_entries(world, root, &mut visited, context)?;
        if world.get::<MenuBar>(root).is_some() {
            if entries.iter().any(|e| !matches!(e, MenuNode::Menu { .. })) {
                return Err("MenuBar entries must be menus".into());
            }
            // A bar leaves out menus with nothing in them (now).
            entries.retain(|e| !matches!(e, MenuNode::Menu { entries, .. } if entries.is_empty()));
        }
        Ok(MenuSnapshot {
            root: Some(root),
            entries,
        })
    }
}

/// The focused context, captured as the entity focused now, so a menu opened
/// for one panel still acts on it after focus moves.
fn capture_context(world: &World, context: InvocationContext) -> InvocationContext {
    match context {
        InvocationContext::Focused => world
            .actions()
            .entity(context)
            .map_or(InvocationContext::Global, InvocationContext::Entity),
        explicit => explicit,
    }
}

fn read_entries(
    world: &World,
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
        if !shown(world, entity) {
            continue;
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
                    .is_some_and(|t| world.actions().available(t, context));
                let shortcut = target
                    .as_ref()
                    .and_then(|t| world.actions().shortcut_for(t, context));
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
