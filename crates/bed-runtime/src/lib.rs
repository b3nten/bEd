//! Core schedules for Bed's shared Bevy app.
//!
//! This crate has no editor or UI dependencies. Games install ordinary Bevy
//! plugins into this app; editor builds add workbench plugins to the same world.

pub use bevy;

#[cfg(feature = "render")]
pub mod render;

use bevy::{app::TaskPoolPlugin, input::InputPlugin, prelude::*, time::TimePlugin};

/// Core schedules, time and input, without a window or an application runner.
/// Install graphics plugins first when using Bevy's `DefaultPlugins` so their
/// foundational plugins are reused.
pub struct BedRuntimePlugin;

impl Plugin for BedRuntimePlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<TaskPoolPlugin>() {
            app.add_plugins(TaskPoolPlugin::default());
        }
        if !app.is_plugin_added::<TimePlugin>() {
            app.add_plugins(TimePlugin);
        }
        if !app.is_plugin_added::<InputPlugin>() {
            app.add_plugins(InputPlugin);
        }
        app.init_resource::<InputCapture>();
    }
}

/// The native frontend updates capture before forwarding input to gameplay.
/// Workbench commands still use their captured panel target independently.
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputCapture {
    pub keyboard: bool,
    pub pointer: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::time::TimeUpdateStrategy;
    use std::time::Duration;

    #[derive(Resource, Default)]
    struct GameTicks(u32);

    #[test]
    fn headless_runtime_runs_standard_game_schedules() {
        let mut app = App::new();
        app.add_plugins(BedRuntimePlugin)
            .init_resource::<GameTicks>()
            .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
                1.0 / 30.0,
            )))
            .add_systems(FixedUpdate, |mut ticks: ResMut<GameTicks>| ticks.0 += 1);
        app.update();
        app.update();
        assert!(app.world().resource::<GameTicks>().0 > 0);
        assert!(app.world().resource::<Time>().elapsed() > Duration::ZERO);
    }

    #[test]
    fn existing_foundational_plugins_are_reused() {
        let mut app = App::new();
        app.add_plugins((TaskPoolPlugin::default(), TimePlugin, InputPlugin))
            .add_plugins(BedRuntimePlugin);
        app.update();
    }
}
