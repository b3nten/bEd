use bedtime_git::{GitHost, GitPlugin};
use bedtime_terminal::{TerminalOptions, TerminalPanel};
use bedtime_workbench::Place;
use bevy::prelude::*;
use std::path::PathBuf;

pub struct BedGitPlugin;

impl Plugin for BedGitPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(GitPlugin).insert_resource(GitHost {
            open_folder: Some(open_folder),
            open_terminal: Some(open_terminal),
        });
    }
}

/// The repository's folder as the workspace (after this frame's drawing).
fn open_folder(world: &mut World, folder: PathBuf) {
    world.commands().queue(move |world: &mut World| {
        crate::menus::switch(world, crate::workspace::Start::Folder(folder));
    });
}

fn open_terminal(world: &mut World, folder: PathBuf, area: Entity) {
    let options = TerminalOptions {
        working_directory: Some(folder),
        ..default()
    };
    world.spawn((TerminalPanel::new(options), Place::in_area(area)));
    world.flush();
}
