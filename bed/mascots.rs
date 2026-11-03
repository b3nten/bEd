use crate::panels::PanelPolicy;
use bedtime_mascots::{BedPanel, DuckPanel, MascotPlugin};
use bedtime_terminal::{ShellCommandsAppExt, ShellInvocation};
use bedtime_workbench::{
    Action, ActionAppExt, ActionInvocation, ActionResult, Handler, PanelFactory,
};
use bevy::prelude::*;

pub struct OpenBed;
impl Action for OpenBed {
    const KEY: &'static str = "app.open_bed";
    const LABEL: &'static str = "Bed";
}

impl PanelPolicy for BedPanel {
    type Key = ();
    fn key(_: &World, _: Entity) {}
}

impl PanelPolicy for DuckPanel {
    type Key = ();
    fn key(_: &World, _: Entity) {}
}

pub struct MascotsPlugin;

impl Plugin for MascotsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MascotPlugin)
            .add_action_handler::<OpenBed>(Handler::new(open_bed))
            .add_shell_command(["+ducky"], ducky)
            .expect("bed's shell commands are unique");
    }
}

fn open_bed(world: &mut World, _: ActionInvocation) -> ActionResult {
    crate::panels::open_panel::<BedPanel>(world, (), PanelFactory::of::<BedPanel>()).map(|_| ())
}

fn ducky(world: &mut World, invocation: ShellInvocation) {
    let result = if invocation.arguments.is_empty() {
        crate::panels::open_panel::<DuckPanel>(world, (), PanelFactory::of::<DuckPanel>())
            .map(|_| ())
    } else {
        Err(format!("usage: {}", invocation.alias))
    };
    invocation.reply.complete(result);
}
