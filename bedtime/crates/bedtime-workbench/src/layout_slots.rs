//! Numbered layouts (`LayoutSlotsPlugin`): five to switch between with
//! Cmd/Ctrl+1 to 5 and a Layout menu, each with a default that's saved for
//! the whole app (the global store) and that Reset Layout goes back to.
//! With workspaces, each workspace has its own five.
//!
//! A slot with no saved default starts empty, but for what the app says
//! with [`FirstLayout`] (its file tree beside an empty area, say).
use crate::BedtimeWorldExt as _;
use crate::{
    Action, ActionAppExt, ActionInvocation, ActionResult, Handler, KeyChord, Layout,
    LayoutFallback, MenusAppExt, OnWorkspaceOpen, StandardMenu,
    persistence::{GlobalStore, LayoutTemplate, Persisted, PersistenceAppExt, ReflectPersist},
};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

pub const SLOTS: usize = 5;

/// What a slot's layout starts with when it has no saved default: given the
/// new, empty layout (one area), it adds panels and splits. Without it,
/// every slot starts empty.
#[derive(Resource, Clone, Copy)]
pub struct FirstLayout(pub fn(&mut World, LayoutSlot, Entity) -> Result<(), String>);

/// When the first layout is opened as a workspace opens: after it, a
/// workspace has an active layout.
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct OpenFirstLayout;

/// The Layout menu: the slots, then saving and resetting defaults.
const LAYOUT: StandardMenu = StandardMenu::Custom("Layout");

/// Which numbered layout this is, from 0.
#[derive(Component, Reflect, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[reflect(Component, Persist, Default)]
// Saved layouts name it by this path, wherever its module moves to.
#[type_path = "bedtime"]
pub struct LayoutSlot(pub usize);

impl LayoutSlot {
    fn name(self) -> String {
        format!("Layout {}", self.0 + 1)
    }
}

/// Each slot's default arrangement, shared by every workspace. `None` is a
/// single empty area.
#[derive(Resource, Clone, Debug, Default, PartialEq, Serialize, Deserialize, Persisted)]
#[persisted(key = "layout_defaults")]
pub struct LayoutDefaults(pub [Option<LayoutTemplate>; SLOTS]);

/// Switch to numbered layout `N` (from 0).
pub struct UseLayout<const N: usize>;
impl<const N: usize> Action for UseLayout<N> {
    const KEY: &'static str = ["layout.1", "layout.2", "layout.3", "layout.4", "layout.5"][N];
    const LABEL: &'static str = ["Layout 1", "Layout 2", "Layout 3", "Layout 4", "Layout 5"][N];
    const DEFAULT_KEYS: &'static [KeyChord] = &[KeyChord::primary(
        [
            KeyCode::Digit1,
            KeyCode::Digit2,
            KeyCode::Digit3,
            KeyCode::Digit4,
            KeyCode::Digit5,
        ][N],
    )];
}

/// Make the current layout its slot's default, for every workspace.
pub struct SaveLayoutAsDefault;
impl Action for SaveLayoutAsDefault {
    const KEY: &'static str = "layout.save_default";
    const LABEL: &'static str = "Save Layout as Default";
}

/// Replace the current layout with its slot's default.
pub struct ResetLayout;
impl Action for ResetLayout {
    const KEY: &'static str = "layout.reset";
    const LABEL: &'static str = "Reset Layout";
}

pub struct LayoutSlotsPlugin;

impl Plugin for LayoutSlotsPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<LayoutSlot>()
            .init_resource::<LayoutDefaults>()
            .persist::<LayoutDefaults, GlobalStore>()
            // Placing a panel with no layout open opens layout 1.
            .insert_resource(LayoutFallback(|world| switch(world, LayoutSlot(0))))
            .add_action_handler::<UseLayout<0>>(Handler::new(use_layout::<0>))
            .add_action_handler::<UseLayout<1>>(Handler::new(use_layout::<1>))
            .add_action_handler::<UseLayout<2>>(Handler::new(use_layout::<2>))
            .add_action_handler::<UseLayout<3>>(Handler::new(use_layout::<3>))
            .add_action_handler::<UseLayout<4>>(Handler::new(use_layout::<4>))
            .add_action_handler::<SaveLayoutAsDefault>(
                Handler::new(save_as_default).available_when(|world, _| current(world).is_some()),
            )
            .add_action_handler::<ResetLayout>(
                Handler::new(reset).available_when(|world, _| current(world).is_some()),
            )
            .add_menu_item::<UseLayout<0>>(LAYOUT, "slots")
            .add_menu_item::<UseLayout<1>>(LAYOUT, "slots")
            .add_menu_item::<UseLayout<2>>(LAYOUT, "slots")
            .add_menu_item::<UseLayout<3>>(LAYOUT, "slots")
            .add_menu_item::<UseLayout<4>>(LAYOUT, "slots")
            .add_menu_item::<SaveLayoutAsDefault>(LAYOUT, "defaults")
            .add_menu_item::<ResetLayout>(LAYOUT, "defaults")
            .add_systems(OnWorkspaceOpen, open_first.in_set(OpenFirstLayout));
    }
}

/// The active layout's slot.
pub fn current(world: &World) -> Option<LayoutSlot> {
    world
        .bedtime()
        .layouts()
        .active()
        .and_then(|layout| world.get::<LayoutSlot>(layout).copied())
}

fn layout_in(world: &World, slot: LayoutSlot) -> Option<Entity> {
    world
        .bedtime()
        .layouts()
        .all()
        .iter()
        .copied()
        .find(|&layout| world.get::<LayoutSlot>(layout) == Some(&slot))
}

/// Make `slot`'s layout active, creating it from its default if needed.
pub fn switch(world: &mut World, slot: LayoutSlot) -> Result<Entity, String> {
    let layout = match layout_in(world, slot) {
        Some(layout) => layout,
        None => create(world, slot)?,
    };
    world.bedtime_mut().layouts_mut().activate(layout)?;
    Ok(layout)
}

/// A new layout for `slot` from its default; not made active.
fn create(world: &mut World, slot: LayoutSlot) -> Result<Entity, String> {
    let default = world.resource::<LayoutDefaults>().0[slot.0].clone();
    let layout = match default {
        Some(template) => world
            .bedtime_mut()
            .layouts_mut()
            .instantiate_layout(&template)
            .map_err(|error| format!("{}'s default can't be used: {error}", slot.name()))?,
        None => {
            let layout = world.bedtime_mut().layouts_mut().create(slot.name())?;
            let first = world.get_resource::<FirstLayout>().map(|first| first.0);
            if let Some(first) = first {
                first(world, slot, layout)?;
            }
            layout
        }
    };
    world.entity_mut(layout).insert(slot);
    world.get_mut::<Layout>(layout).unwrap().name = slot.name();
    Ok(layout)
}

/// When a workspace opens: its saved active layout, or layout 1. A workbench
/// saved before slots existed gets its active layout as layout 1.
fn open_first(world: &mut World) {
    if world
        .bedtime()
        .layouts()
        .all()
        .iter()
        .all(|&layout| world.get::<LayoutSlot>(layout).is_none())
        && let Some(layout) = world.bedtime().layouts().active()
    {
        world.entity_mut(layout).insert(LayoutSlot(0));
    }
    if world.bedtime().layouts().active().is_none() {
        let result = switch(world, LayoutSlot(0));
        world
            .bedtime_mut()
            .notify_failure("Couldn't open a layout", result);
    }
}

fn use_layout<const N: usize>(world: &mut World, _: ActionInvocation) -> ActionResult {
    switch(world, LayoutSlot(N)).map(|_| ())
}

fn save_as_default(world: &mut World, _: ActionInvocation) -> ActionResult {
    let slot = current(world).ok_or("This layout has no number")?;
    let layout = world
        .bedtime()
        .layouts()
        .active()
        .ok_or("No layout is open")?;
    let template = world.bedtime_mut().layouts_mut().capture_layout(layout)?;
    world.resource_mut::<LayoutDefaults>().0[slot.0] = Some(template);
    Ok(())
}

/// Build the replacement first, so a default that can't be used leaves the
/// current layout alone.
fn reset(world: &mut World, _: ActionInvocation) -> ActionResult {
    let slot = current(world).ok_or("This layout has no number")?;
    let old = world
        .bedtime()
        .layouts()
        .active()
        .ok_or("No layout is open")?;
    let new = create(world, slot)?;
    let workbench = world.bedtime_mut().layouts_mut();
    workbench.activate(new)?;
    if let Err(error) = workbench.delete(old) {
        workbench.activate(old)?;
        workbench.world_mut().despawn(new);
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use bevy::reflect::TypePath;

    /// Saved layouts name these types by path: moving their modules must
    /// not rename them, or what was saved no longer opens.
    #[test]
    fn saved_types_keep_the_names_they_were_saved_under() {
        assert_eq!(super::LayoutSlot::type_path(), "bedtime::LayoutSlot");
        #[cfg(all(feature = "workspaces", feature = "ui"))]
        assert_eq!(
            crate::workspaces::WorkspacesPanel::type_path(),
            "bedtime::WorkspacesPanel"
        );
    }
}
