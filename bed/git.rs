use crate::panels::PanelPolicy;
use bedtime_git::{GitHost, GitPanel, GitPlugin};
use bedtime_terminal::{TerminalOptions, TerminalPanel};
use bedtime_workbench::{
    Action, ActionAppExt, ActionInvocation, ActionResult, Handler, InArea, LayoutsWorldExt,
    PanelFactory,
};
use bevy::prelude::*;
use std::path::PathBuf;

pub struct OpenGit;
impl Action for OpenGit {
    const KEY: &'static str = "app.open_git";
    const LABEL: &'static str = "Git";
}

impl PanelPolicy for GitPanel {
    /// One Git panel shows the workspace's repository.
    type Key = ();
    fn key(_: &World, _: Entity) {}
}

pub struct BedGitPlugin;

impl Plugin for BedGitPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(GitPlugin)
            .insert_resource(GitHost {
                open_folder: Some(open_folder),
                open_terminal: Some(open_terminal),
            })
            .add_action_handler::<OpenGit>(Handler::new(open_git));
    }
}

fn open_git(world: &mut World, _: ActionInvocation) -> ActionResult {
    crate::panels::open_panel::<GitPanel>(world, (), PanelFactory::of::<GitPanel>()).map(|_| ())
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
    let terminal = world
        .spawn((TerminalPanel::new(options), InArea(area)))
        .id();
    world.flush();
    if let Err(error) = world.layouts_mut().focus_panel(terminal) {
        warn!("{error}");
    }
}
