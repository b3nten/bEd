use bedtime_code::EditorPanel;
use bedtime_document::TextBuffer;
use bedtime_search::{SearchPanel, SearchPlugin, SearchWorkspace};
use bedtime_workbench::{
    ActionAppExt, ActionInvocation, ActionResult, DocumentReference, Handler, PanelsWorldExt,
    WorkbenchWorldExt,
};
use bevy::prelude::*;

pub struct BedSearchPlugin;

impl Plugin for BedSearchPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(SearchPlugin)
            .add_action_handler::<SearchWorkspace>(Handler::new(search_workspace));
    }
}

fn search_workspace(world: &mut World, invocation: ActionInvocation) -> ActionResult {
    let selected = invocation.entity.and_then(|panel| selection(world, panel));
    let panel = world.focus_or_open::<SearchPanel>()?;
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
