use bedtime_audio::AudioPlugin;
use bedtime_code::CodePlugin;
use bedtime_filetree::FileTreePlugin;
use bedtime_font::FontPlugin;
use bedtime_hex::HexPlugin;
use bedtime_image::ImagePlugin;
use bedtime_model::ModelViewerPlugin;
use bedtime_table::TablePlugin;
use bedtime_video::VideoPlugin;
use bedtime_workbench::{Notification, OpenersAppExt, PanelFactory, WorkbenchWorldExt};
use bevy::prelude::*;
use std::path::Path;

pub struct FilesPlugin;

impl Plugin for FilesPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(bedtime_workbench::FileDrops(dropped))
            .init_resource::<DroppedFolder>()
            .add_systems(Update, open_dropped_folder)
            .add_plugins((
                FileTreePlugin,
                CodePlugin,
                HexPlugin,
                ImagePlugin,
                ModelViewerPlugin,
                FontPlugin,
                AudioPlugin,
                VideoPlugin,
                TablePlugin,
            ))
            .set_open_fallback(nothing_opens);
        // Save, Undo and Redo act on the focused panel's document.
        app.world_mut().workbench_mut().register_document_handlers();
    }
}

fn nothing_opens(world: &mut World, path: &Path, _: String) {
    let name = path.file_name().unwrap_or(path.as_os_str()).display();
    world
        .workbench_mut()
        .notify(Notification::warning(format!("bEd can't open {name}")));
}

/// A folder dropped on the window, to open as the workspace (in `Update`,
/// not while the layout draws).
#[derive(Resource, Default)]
struct DroppedFolder(Option<std::path::PathBuf>);

/// Files dropped on a layout open where they're dropped; a folder opens as
/// the workspace.
fn dropped(world: &mut World, paths: Vec<std::path::PathBuf>) -> Option<PanelFactory> {
    if let [folder] = paths.as_slice()
        && folder.is_dir()
    {
        world.resource_mut::<DroppedFolder>().0 = Some(folder.clone());
        return None;
    }
    let files: Vec<_> = paths.into_iter().filter(|path| !path.is_dir()).collect();
    (!files.is_empty()).then(|| bedtime_filetree::open_files_factory(files))
}

fn open_dropped_folder(world: &mut World) {
    if let Some(folder) = world.resource_mut::<DroppedFolder>().0.take() {
        crate::menus::switch(world, crate::workspace::Start::Folder(folder));
    }
}
