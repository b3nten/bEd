//! Files: the workspace folder's file tree, what opens which file, saving
//! them, and Structure. Text opens in the editor; images, models, fonts,
//! sound, video and tables in their viewers; anything else in hex.
use crate::panels::PanelPolicy;
use bedtime_audio::AudioPlugin;
use bedtime_code::{CodePlugin, StructurePanel};
use bedtime_filetree::{FileTree, FileTreePlugin};
use bedtime_font::FontPlugin;
use bedtime_hex::HexPlugin;
use bedtime_image::ImagePlugin;
use bedtime_model::ModelViewerPlugin;
use bedtime_table::TablePlugin;
use bedtime_video::VideoPlugin;
use bedtime_workbench::{
    Action, ActionAppExt, ActionInvocation, ActionResult, Handler, Notification, OpenersAppExt,
    PanelFactory, WorkbenchWorldExt,
};
use bevy::prelude::*;
use std::path::Path;

pub struct OpenFiles;
impl Action for OpenFiles {
    const KEY: &'static str = "app.open_files";
    const LABEL: &'static str = "Files";
}

pub struct OpenStructure;
impl Action for OpenStructure {
    const KEY: &'static str = "app.open_structure";
    const LABEL: &'static str = "Structure";
}

impl PanelPolicy for StructurePanel {
    /// One Structure follows the focused editor.
    type Key = ();
    fn key(_: &World, _: Entity) {}
}

impl PanelPolicy for FileTree {
    /// One tree shows the workspace folder.
    type Key = ();
    fn key(_: &World, _: Entity) {}
}

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
            .set_open_fallback(nothing_opens)
            .add_action_handler::<OpenFiles>(Handler::new(open_files))
            .add_action_handler::<OpenStructure>(Handler::new(open_structure));
        // Save, Undo and Redo act on the focused panel's document.
        app.world_mut().workbench_mut().register_document_handlers();
    }
}

fn open_structure(world: &mut World, _: ActionInvocation) -> ActionResult {
    let factory = PanelFactory::of::<StructurePanel>();
    crate::panels::open_panel::<StructurePanel>(world, (), factory).map(|_| ())
}

fn open_files(world: &mut World, _: ActionInvocation) -> ActionResult {
    crate::panels::open_panel::<FileTree>(world, (), PanelFactory::of::<FileTree>()).map(|_| ())
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
