//! The workspace's lifetime, as two schedules plugins add systems to. They
//! run with or without the `workspaces` feature, so a plugin never asks
//! whether workspaces exist:
//!
//! - [`OnWorkspaceOpen`] runs once at startup, in `PostStartup` (after stores
//!   have loaded and the app's `Startup` has run), and again after each
//!   switch. Set up what belongs to the workspace here, not in `Startup`.
//! - [`OnWorkspaceClose`] runs before a switch, while the workspace is still
//!   there: drop resources that belong to it (a session for its folder, say).
//!   It doesn't run when the app quits.
//!
//! What a workspace has is mostly on its panels, which go with its layouts.
use bevy::{ecs::schedule::ScheduleLabel, prelude::*};

/// Runs when a workspace opens: at startup, and after each switch.
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct OnWorkspaceOpen;

/// Runs before the workspace closes for another one.
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct OnWorkspaceClose;

pub(crate) fn install(app: &mut App) {
    app.init_schedule(OnWorkspaceOpen)
        .init_schedule(OnWorkspaceClose)
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
}
