//! bEd's workspaces: a window shows one folder's workspace (or the default
//! one), with bedtime's numbered layouts and its Workspaces panel to switch
//! between them. What's bEd's own is what a new layout 1 starts with.
pub use bedtime_workbench::workspaces::*;
use bedtime_workbench::{
    BedtimeWorldExt as _, PanelFactory, SplitAxis,
    layout_slots::{FirstLayout, LayoutSlot, LayoutSlotsPlugin},
};
use bevy::prelude::*;

/// bEd's workspaces on top of workbench's (`WorkspacesPlugin`, part of
/// `BedWorkbenchPlugin`): the numbered layouts and the Workspaces panel.
pub struct BedWorkspacesPlugin;

impl Plugin for BedWorkspacesPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((LayoutSlotsPlugin, WorkspacesPanelPlugin))
            .insert_resource(FirstLayout(first_layout));
    }
}

/// Layout 1, with no saved default: the default workspace starts with the
/// Workspaces panel; a folder's, with its files in the left quarter, so
/// files open on the right. The other layouts start empty.
fn first_layout(world: &mut World, slot: LayoutSlot, layout: Entity) -> Result<(), String> {
    if slot != LayoutSlot(0) {
        return Ok(());
    }
    let area = world.bedtime_mut().layouts_mut().destination_area(layout)?;
    if world.resource::<Workspace>().default {
        (PanelFactory::of::<WorkspacesPanel>().create)(world, area);
    } else {
        world.bedtime_mut().layouts_mut().split_area(
            area,
            SplitAxis::Horizontal,
            2500,
            [100, 100],
        )?;
        (PanelFactory::of::<bedtime_file_explorer::FileExplorer>().create)(world, area);
    }
    Ok(())
}
