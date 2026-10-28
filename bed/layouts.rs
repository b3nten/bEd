//! Five numbered layouts per workspace, switched with Cmd+1…5. Each is an
//! ordinary workbench layout saved with the workspace; `LayoutSlot` says which
//! number it is. A slot's layout is made the first time it's opened, from that
//! slot's default: a template saved in the global store with "Save Layout as
//! Default", or a single empty area.
use crate::settings::GlobalStore;
use bedtime_workbench::{
    CommandAppExt, ContextCommand, ContextCommandInvocation, ContextCommandResult, Handler,
    KeyChord, Layout, Workbench, active_layout, layouts,
    persistence::{
        LayoutTemplate, ReflectPersist, StoreAppExt, capture_layout, instantiate_layout,
    },
};
use bevy::prelude::*;

pub const SLOTS: usize = 5;

/// Which numbered layout this is, from 0.
#[derive(Component, Reflect, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[reflect(Component, Persist, Default)]
pub struct LayoutSlot(pub usize);

impl LayoutSlot {
    fn name(self) -> String {
        format!("Layout {}", self.0 + 1)
    }
}

/// Each slot's default arrangement, shared by every workspace. `None` is a
/// single empty area.
#[derive(Resource, Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Resource, Persist, Default)]
pub struct LayoutDefaults(pub [Option<LayoutTemplate>; SLOTS]);

/// Switch to numbered layout `N` (from 0).
pub struct UseLayout<const N: usize>;
impl<const N: usize> ContextCommand for UseLayout<N> {
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
impl ContextCommand for SaveLayoutAsDefault {
    const KEY: &'static str = "layout.save_default";
    const LABEL: &'static str = "Save Layout as Default";
}

/// Replace the current layout with its slot's default.
pub struct ResetLayout;
impl ContextCommand for ResetLayout {
    const KEY: &'static str = "layout.reset";
    const LABEL: &'static str = "Reset Layout";
}

pub struct LayoutsPlugin;

impl Plugin for LayoutsPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<LayoutSlot>()
            .register_type::<LayoutDefaults>()
            .init_resource::<LayoutDefaults>()
            .persist_in::<LayoutDefaults, GlobalStore>()
            .add_command_handler::<UseLayout<0>>(Handler::new(use_layout::<0>))
            .add_command_handler::<UseLayout<1>>(Handler::new(use_layout::<1>))
            .add_command_handler::<UseLayout<2>>(Handler::new(use_layout::<2>))
            .add_command_handler::<UseLayout<3>>(Handler::new(use_layout::<3>))
            .add_command_handler::<UseLayout<4>>(Handler::new(use_layout::<4>))
            .add_command_handler::<SaveLayoutAsDefault>(
                Handler::new(save_as_default).available_when(|world, _| current(world).is_some()),
            )
            .add_command_handler::<ResetLayout>(
                Handler::new(reset).available_when(|world, _| current(world).is_some()),
            )
            .add_systems(Startup, open_first);
    }
}

/// The active layout's slot.
pub fn current(world: &World) -> Option<LayoutSlot> {
    active_layout(world).and_then(|layout| world.get::<LayoutSlot>(layout).copied())
}

fn layout_in(world: &World, slot: LayoutSlot) -> Option<Entity> {
    layouts(world)
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
    Workbench::new(world).activate_layout(layout)?;
    Ok(layout)
}

/// A new layout for `slot` from its default; not made active.
fn create(world: &mut World, slot: LayoutSlot) -> Result<Entity, String> {
    let default = world.resource::<LayoutDefaults>().0[slot.0].clone();
    let layout = match default {
        Some(template) => instantiate_layout(world, &template)
            .map_err(|error| format!("{}'s default can't be used: {error}", slot.name()))?,
        None => Workbench::new(world).create_layout(slot.name())?,
    };
    world.entity_mut(layout).insert(slot);
    world.get_mut::<Layout>(layout).unwrap().name = slot.name();
    Ok(layout)
}

/// At startup: the workspace's saved active layout, or layout 1. A workbench
/// saved before slots existed gets its active layout as layout 1.
fn open_first(world: &mut World) {
    if layouts(world)
        .iter()
        .all(|&layout| world.get::<LayoutSlot>(layout).is_none())
        && let Some(layout) = active_layout(world)
    {
        world.entity_mut(layout).insert(LayoutSlot(0));
    }
    if active_layout(world).is_none()
        && let Err(error) = switch(world, LayoutSlot(0))
    {
        warn!("{error}");
    }
}

fn use_layout<const N: usize>(
    world: &mut World,
    _: ContextCommandInvocation,
) -> ContextCommandResult {
    switch(world, LayoutSlot(N)).map(|_| ())
}

fn save_as_default(world: &mut World, _: ContextCommandInvocation) -> ContextCommandResult {
    let slot = current(world).ok_or("This layout has no number")?;
    let layout = active_layout(world).ok_or("No layout is open")?;
    let template = capture_layout(world, layout)?;
    world.resource_mut::<LayoutDefaults>().0[slot.0] = Some(template);
    Ok(())
}

/// Build the replacement first, so a default that can't be used leaves the
/// current layout alone.
fn reset(world: &mut World, _: ContextCommandInvocation) -> ContextCommandResult {
    let slot = current(world).ok_or("This layout has no number")?;
    let old = active_layout(world).ok_or("No layout is open")?;
    let new = create(world, slot)?;
    let mut workbench = Workbench::new(world);
    workbench.activate_layout(new)?;
    if let Err(error) = workbench.delete_layout(old) {
        workbench.activate_layout(old)?;
        workbench.world_mut().despawn(new);
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{panels::open_panel, settings::SettingsPanel};
    use bedtime_workbench::{
        ContextCommandId, ContextCommandRequest, ContextCommandRequests, InvocationContext,
        PanelFactory, WorkbenchCorePlugin,
    };

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins((WorkbenchCorePlugin, LayoutsPlugin))
            .register_type::<SettingsPanel>();
        app.update();
        app
    }

    fn run<C: ContextCommand>(app: &mut App) {
        app.world_mut()
            .resource_mut::<ContextCommandRequests>()
            .submit(ContextCommandRequest {
                command: ContextCommandId::of::<C>(),
                context: InvocationContext::Focused,
                source: None,
            });
        app.update();
    }

    fn settings_panels(world: &mut World) -> usize {
        let layout = active_layout(world).unwrap();
        Workbench::new(world)
            .panels(layout)
            .into_iter()
            .filter(|&panel| world.get::<SettingsPanel>(panel).is_some())
            .count()
    }

    fn open_settings(world: &mut World) {
        open_panel::<SettingsPanel>(world, (), PanelFactory::of::<SettingsPanel>()).unwrap();
    }

    #[test]
    fn slots_are_made_when_first_opened_and_keep_their_panels() {
        let mut app = app();
        assert_eq!(current(app.world()), Some(LayoutSlot(0)));
        open_settings(app.world_mut());

        run::<UseLayout<2>>(&mut app);
        let world = app.world_mut();
        assert_eq!(current(world), Some(LayoutSlot(2)));
        assert_eq!(layouts(world).len(), 2);
        assert_eq!(settings_panels(world), 0);
        let name = &world.get::<Layout>(active_layout(world).unwrap()).unwrap().name;
        assert_eq!(name, "Layout 3");

        run::<UseLayout<0>>(&mut app);
        assert_eq!(settings_panels(app.world_mut()), 1);
        assert_eq!(layouts(app.world()).len(), 2);
    }

    #[test]
    fn a_saved_default_builds_new_slots_and_resets_the_layout() {
        let mut app = app();
        open_settings(app.world_mut());
        run::<SaveLayoutAsDefault>(&mut app);
        let defaults = app.world().resource::<LayoutDefaults>().clone();
        assert!(defaults.0[0].is_some() && defaults.0[1].is_none());

        // Another workspace (another App) shares the defaults.
        let mut other = App::new();
        other
            .add_plugins((WorkbenchCorePlugin, LayoutsPlugin))
            .register_type::<SettingsPanel>()
            .insert_resource(defaults);
        other.update();
        assert_eq!(settings_panels(other.world_mut()), 1);

        // Reset puts back the default: Settings returns once closed.
        let world = other.world_mut();
        let layout = active_layout(world).unwrap();
        for panel in Workbench::new(world).panels(layout) {
            Workbench::new(world).close_panel(panel).unwrap();
        }
        assert_eq!(settings_panels(world), 0);
        run::<ResetLayout>(&mut other);
        let world = other.world_mut();
        assert_eq!(settings_panels(world), 1);
        assert_eq!(current(world), Some(LayoutSlot(0)));
        assert_eq!(layouts(world).len(), 1);
    }

    #[test]
    fn a_workbench_saved_before_slots_keeps_its_layout_as_layout_1() {
        let mut app = App::new();
        app.add_plugins((WorkbenchCorePlugin, LayoutsPlugin));
        let world = app.world_mut();
        let layout = Workbench::new(world).create_layout("Old").unwrap();
        Workbench::new(world).activate_layout(layout).unwrap();
        app.update();
        assert_eq!(app.world().get::<LayoutSlot>(layout), Some(&LayoutSlot(0)));
        assert_eq!(layouts(app.world()).len(), 1);
    }
}
