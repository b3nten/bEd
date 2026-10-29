//! Experiment: bed's workbench shell and Settings panel drawn with bevy_ui,
//! using feathers widgets through bevy_immediate. The workbench model
//! (layouts, areas, tabs) is the real one, built without its egui frontend;
//! only the drawing is new.
//!
//! `cargo run -p feathers-spike`. `SPIKE_SCREENSHOT=out.png` saves the window
//! after a few frames and exits; `SPIKE_THEME`, `SPIKE_GAP` and
//! `SPIKE_ROUNDING` preset the look for screenshots.
mod look;
mod panels;
mod shell;

use bedtime_workbench::{InArea, SplitAxis, WorkbenchCorePlugin, WorkbenchWorldExt};
use bevy::feathers::FeathersPlugins;
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};

fn main() {
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "bEd — feathers spike".into(),
                resolution: (1280, 800).into(),
                focused: false,
                ..default()
            }),
            ..default()
        }))
        .add_plugins((FeathersPlugins, WorkbenchCorePlugin))
        .add_plugins((look::plugin, shell::plugin, panels::plugin))
        .add_systems(Startup, setup)
        .add_systems(Update, screenshot)
        .run();
}

/// Three areas: Settings and Notes share the left, Workspaces is on the right,
/// and Notes again below it.
fn setup(world: &mut World) {
    world.spawn(Camera2d);
    let w = world.workbench_mut();
    let layout = w.create_layout("Spike").unwrap();
    w.activate_layout(layout).unwrap();
    let left = w.destination_area(layout).unwrap();
    let minimum = [500, 500];
    let right = w
        .split_area(left, SplitAxis::Horizontal, 6_200, minimum)
        .unwrap();
    let lower = w
        .split_area(right, SplitAxis::Vertical, 6_000, minimum)
        .unwrap();
    let settings = w
        .world_mut()
        .spawn((panels::SettingsPanel, InArea(left)))
        .id();
    w.world_mut().spawn((panels::NotesPanel, InArea(left)));
    w.world_mut()
        .spawn((panels::WorkspacesPanel, InArea(right)));
    w.world_mut().spawn((panels::NotesPanel, InArea(lower)));
    w.focus_panel(settings).unwrap();
    world.spawn(shell::ShellRoot);
}

fn screenshot(mut commands: Commands, mut frames: Local<u32>) {
    let Ok(path) = std::env::var("SPIKE_SCREENSHOT") else {
        return;
    };
    *frames += 1;
    if *frames == 60 {
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path));
    }
    // Saving happens off the main thread; give it time before exiting.
    if *frames == 120 {
        commands.write_message(AppExit::Success);
    }
}
