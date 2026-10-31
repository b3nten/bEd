//! Files: the workspace folder's file tree, what opens which file, saving
//! them, and Structure. Text opens in the editor; anything else is reported.
use crate::panels::PanelPolicy;
use bedtime_code::{CodePlugin, StructurePanel};
use bedtime_filetree::{FileTree, FileTreePlugin};
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
        app.add_plugins((FileTreePlugin, CodePlugin))
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
