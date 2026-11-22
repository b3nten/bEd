//! Bed and Ducky (`+ducky` in a terminal).
use bedtime_mascots::{DuckPanel, MascotPlugin};
use bedtime_terminal::{ShellCommandsAppExt, ShellInvocation};
use bedtime_workbench::BedtimeWorldExt as _;
use bevy::prelude::*;

pub struct MascotsPlugin;

impl Plugin for MascotsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MascotPlugin)
            .add_shell_command(["+ducky"], ducky)
            .expect("bed's shell commands are unique");
    }
}

fn ducky(world: &mut World, invocation: ShellInvocation) {
    let result = if invocation.arguments.is_empty() {
        world
            .bedtime_mut()
            .open_panel_at(
                DuckPanel,
                bedtime_workbench::Placement::From(invocation.panel),
            )
            .map(|_| ())
    } else {
        Err(format!("usage: {}", invocation.alias))
    };
    invocation.reply.complete(result);
}
