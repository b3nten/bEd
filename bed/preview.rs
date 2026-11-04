use bedtime_code::EditorPanel;
use bedtime_preview::{PreviewPlugin, is_preview, preview_viewer};
use bedtime_workbench::{
    Action, ActionAppExt, ActionInvocation, ActionResult, Area, DocumentReference, Handler, InArea,
    KeyChord, LayoutsWorldExt, Modifiers, PathOptions, SplitAxis, Target, ViewerFile,
    WorkbenchWorldExt,
};
use bevy::prelude::*;

pub struct OpenPreview;
/// Not Cmd/Ctrl+Shift+V: every Cmd/Ctrl+V chord is the editor's paste.
impl Action for OpenPreview {
    const KEY: &'static str = "app.open_preview";
    const LABEL: &'static str = "Open Preview to the Side";
    const DEFAULT_KEYS: &'static [KeyChord] = &[KeyChord::new(
        Modifiers::PRIMARY.union(Modifiers::SHIFT),
        KeyCode::KeyM,
    )];
}

pub struct BedPreviewPlugin;

impl Plugin for BedPreviewPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(PreviewPlugin)
            .add_action_handler::<OpenPreview>(
                Handler::new(open_preview)
                    .available_when(|world, panel| panel.is_some_and(|p| previewable(world, p))),
            );
    }
}

/// An editor showing a file that has a preview.
fn previewable(world: &World, panel: Entity) -> bool {
    world.get::<EditorPanel>(panel).is_some()
        && world
            .get::<ViewerFile>(panel)
            .and_then(|file| file.0.file_name())
            .is_some_and(|name| preview_viewer(&name.to_string_lossy()).is_some())
}

/// The editor's document in its preview: the one already in the layout,
/// else a new one beside the other previews, else in the editor's area
/// split in two (or, too small to split, as a tab in it).
fn open_preview(world: &mut World, invocation: ActionInvocation) -> ActionResult {
    let editor = invocation.entity.ok_or("No editor is focused")?;
    if !previewable(world, editor) {
        return Err("This file has no preview".into());
    }
    let path = world.get::<ViewerFile>(editor).unwrap().0.clone();
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let viewer = preview_viewer(&name);
    let document = world
        .get::<DocumentReference>(editor)
        .ok_or("The editor shows no document")?
        .document();
    let own = world
        .get::<InArea>(editor)
        .ok_or("The editor isn't in an area")?
        .0;
    let layout = world.layouts().active().ok_or("No layout is open")?;
    let previews: Vec<Entity> = world
        .layouts()
        .panels(layout)
        .into_iter()
        .filter(|&panel| is_preview(world, panel))
        .collect();
    let shows = |panel: Entity| {
        world
            .get::<DocumentReference>(panel)
            .is_some_and(|reference| reference.document() == document)
    };
    let elsewhere = previews
        .iter()
        .filter_map(|&panel| world.get::<InArea>(panel).map(|area| area.0))
        .find(|&area| area != own);
    let target = if let Some(&panel) = previews.iter().find(|&&panel| shows(panel)) {
        Target::Panel(panel)
    } else if let Some(area) = elsewhere {
        Target::New { area: Some(area) }
    } else {
        let rect = world
            .get::<Area>(own)
            .ok_or("The editor isn't in an area")?
            .rect();
        let at = (rect.min[0] + rect.max[0]) / 2;
        let area = world
            .layouts_mut()
            .split_area(own, SplitAxis::Horizontal, at, [1000, 1000])
            .unwrap_or(own);
        Target::New { area: Some(area) }
    };
    let options = PathOptions {
        target,
        viewer,
        ..default()
    };
    let preview = world.workbench_mut().open_path(&path, options)?;
    // Keep typing where you were, unless that hides the preview (a tab
    // behind the editor).
    let area = |world: &World, panel| world.get::<InArea>(panel).map(|area| area.0);
    if area(world, preview) == area(world, editor) {
        return Ok(());
    }
    world.layouts_mut().focus_panel(editor)
}
