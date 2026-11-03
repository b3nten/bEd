use crate::panels::PanelPolicy;
use bedtime_code::EditorPanel;
use bedtime_document::TextBuffer;
use bedtime_search::{FindInFiles, SearchPanel, SearchPlugin};
use bedtime_workbench::{
    ActionAppExt, ActionInvocation, ActionResult, DocumentReference, Handler, PanelFactory,
    WorkbenchWorldExt,
};
use bevy::prelude::*;

impl PanelPolicy for SearchPanel {
    /// One Search panel searches the workspace's folder.
    type Key = ();
    fn key(_: &World, _: Entity) {}
}

pub struct BedSearchPlugin;

impl Plugin for BedSearchPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(SearchPlugin)
            .add_action_handler::<FindInFiles>(Handler::new(find_in_files));
    }
}

fn find_in_files(world: &mut World, invocation: ActionInvocation) -> ActionResult {
    let selected = invocation.entity.and_then(|panel| selection(world, panel));
    let panel =
        crate::panels::open_panel::<SearchPanel>(world, (), PanelFactory::of::<SearchPanel>())?;
    bedtime_search::seed(world, panel, selected);
    Ok(())
}

/// The text selected in `panel`'s editor, if it has one.
fn selection(world: &World, panel: Entity) -> Option<String> {
    let range = world.get::<EditorPanel>(panel)?.view.selection().range();
    if range.is_empty() {
        return None;
    }
    let document = world.get::<DocumentReference>(panel)?.document();
    let workbench = world.workbench();
    let text = workbench
        .read_document(workbench.cast_document::<TextBuffer>(document).ok()?)
        .ok()?;
    Some(text.range_text(range))
}
