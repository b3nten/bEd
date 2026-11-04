use bedtime_code::EditorPanel;
use bedtime_preview::{PreviewPlugin, preview_viewer};
use bedtime_workbench::{
    Action, ActionAppExt, ActionInvocation, ActionResult, Handler, KeyChord, Modifiers,
    PathOptions, Placement, ViewerFile, WorkbenchWorldExt,
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
/// else a new one beside the editor (`Placement::Beside`: with previews of
/// its kind, else in the editor's area split in two, else as a tab in it).
fn open_preview(world: &mut World, invocation: ActionInvocation) -> ActionResult {
    let editor = invocation.entity.ok_or("No editor is focused")?;
    if !previewable(world, editor) {
        return Err("This file has no preview".into());
    }
    let path = world.get::<ViewerFile>(editor).unwrap().0.clone();
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    // The preview already showing it, else a new one beside the editor; and
    // keep typing where you were (unless the preview had to go in the
    // editor's own area, where it's the tab shown).
    let options = PathOptions {
        viewer: preview_viewer(&name),
        place: Some(Placement::Beside(editor)),
        keep_focus: true,
        ..default()
    };
    world.workbench_mut().open_path(&path, options).map(|_| ())
}
