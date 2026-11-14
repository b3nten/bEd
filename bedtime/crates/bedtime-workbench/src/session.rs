//! The workspace's lifetime, as two schedules plugins add systems to. They
//! run with or without the `workspaces` feature, so a plugin never asks
//! whether workspaces exist:
//!
//! - [`OnWorkspaceOpen`] runs once at startup, in `PostStartup` (after stores
//!   have loaded and the app's `Startup` has run), and again after each
//!   switch. Set up what belongs to the workspace here, not in `Startup`.
//! - [`OnWorkspaceClosing`] runs once a switch can no longer be cancelled
//!   (unsaved documents are settled), before the workspace's work is stopped:
//!   take anything that should outlive the workspace out of it here.
//! - [`OnWorkspaceClose`] runs before a switch, while the workspace is still
//!   there: drop resources that belong to it (a session for its folder, say).
//!   It doesn't run when the app quits.
//!
//! What a workspace has is its [`InWorkspace`] entities: its layouts, their
//! areas and panels, and whatever else plugins mark. A switch despawns them
//! after `OnWorkspaceClose`, and stops their work (and work with no owner)
//! before that.
use bevy::{ecs::schedule::ScheduleLabel, prelude::*};

/// Runs when a workspace opens: at startup, and after each switch.
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct OnWorkspaceOpen;

/// Runs once a switch is certain, before the workspace's work is stopped.
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct OnWorkspaceClosing;

/// Runs before the workspace closes for another one.
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct OnWorkspaceClose;

/// Belongs to the workspace: despawned when it closes, and its work (tasks it
/// owns) stopped first. Layouts, areas and panels have it; remove it from an
/// entity (and take the entity out of its layout) to keep it past a switch.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct InWorkspace;

pub(crate) fn install(app: &mut App) {
    app.init_schedule(OnWorkspaceOpen)
        .init_schedule(OnWorkspaceClosing)
        .init_schedule(OnWorkspaceClose)
        .register_required_components::<crate::Layout, InWorkspace>()
        .register_required_components::<crate::Area, InWorkspace>()
        .register_required_components::<crate::PanelInstance, InWorkspace>()
        .add_systems(PostStartup, open);
}

fn open(world: &mut World) {
    world.run_schedule(OnWorkspaceOpen);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Resource, Default)]
    struct Opened(Vec<&'static str>);

    #[test]
    fn a_workspace_opens_once_after_startup() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, crate::WorkbenchCorePlugins))
            .init_resource::<Opened>()
            .add_systems(Startup, |mut opened: ResMut<Opened>| {
                opened.0.push("startup")
            })
            .add_systems(OnWorkspaceOpen, |mut opened: ResMut<Opened>| {
                opened.0.push("open")
            })
            .add_systems(OnWorkspaceClose, |mut opened: ResMut<Opened>| {
                opened.0.push("close")
            });
        app.update();
        app.update();
        assert_eq!(app.world().resource::<Opened>().0, ["startup", "open"]);
    }

    #[test]
    fn layouts_areas_and_panels_are_in_the_workspace() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, crate::WorkbenchCorePlugins));
        let world = app.world_mut();
        let layout = world.spawn(crate::Layout::default()).id();
        let area = world
            .spawn((crate::Area::default(), crate::InLayout(layout)))
            .id();
        let panel = world
            .spawn((crate::PanelInstance::default(), crate::InArea(area)))
            .id();
        let other = world.spawn_empty().id();
        for entity in [layout, area, panel] {
            assert!(world.get::<InWorkspace>(entity).is_some());
        }
        assert!(world.get::<InWorkspace>(other).is_none());
    }
}
