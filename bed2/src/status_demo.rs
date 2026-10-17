//! Contributions execute in their workspace world.
use bedtime_ui2::{BuiltinIcon, StatusItem, StatusSide, UiFrame, register_status_item};
use bedtime_workbench2::{Workbench, Workspace, WorkspaceWork};
use bevy::prelude::*;
pub(crate) fn setup(world: &mut World) {
    let bar = Workbench::new(world).status_bar();
    register_status_item(
        world,
        bar,
        StatusItem::new(StatusSide::Left, BuiltinIcon::Folder, "Workspace", 220.0),
        |frame: UiFrame, workspace: Res<Workspace>| {
            frame.ui.label(&workspace.name);
        },
    );
    register_status_item(
        world,
        bar,
        StatusItem::new(StatusSide::Right, BuiltinIcon::Config, "Work", 230.0),
        |frame: UiFrame, work: Res<WorkspaceWork>, session: Res<crate::workbench_demo::Session>| {
            frame.ui.label(format!(
                "{} jobs · {} updates",
                work.list().len(),
                session.ticks
            ));
        },
    );
}
