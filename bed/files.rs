//! Files: the workspace folder's file tree, and what opens which file. Text
//! opens in the text viewer; anything else is reported.
use crate::{panels::PanelPolicy, text::TextViewer};
use bedtime_document::TextBuffer;
use bedtime_filetree::{FileTree, FileTreePlugin};
use bedtime_workbench::{
    Action, ActionAppExt, ActionInvocation, ActionResult, Fit, Handler, Notification, Opener,
    OpenersAppExt, PanelFactory, WorkbenchWorldExt,
};
use bevy::prelude::*;
use std::path::Path;

pub struct OpenFiles;
impl Action for OpenFiles {
    const KEY: &'static str = "app.open_files";
    const LABEL: &'static str = "Files";
}

impl PanelPolicy for FileTree {
    /// One tree shows the workspace folder.
    type Key = ();
    fn key(_: &World, _: Entity) {}
}

pub struct FilesPlugin;

impl Plugin for FilesPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(FileTreePlugin)
            .register_opener(Opener::<TextBuffer>::new::<TextViewer>("Text").files(text_fit))
            .register_default_opener::<TextViewer>()
            .set_open_fallback(nothing_opens)
            .add_action_handler::<OpenFiles>(Handler::new(open_files));
    }
}

fn open_files(world: &mut World, _: ActionInvocation) -> ActionResult {
    crate::panels::open_panel::<FileTree>(world, (), PanelFactory::of::<FileTree>()).map(|_| ())
}

/// Text: no NUL bytes, and UTF-8 (the start may end mid-character).
fn text_fit(_: &Path, head: &[u8]) -> Fit {
    let utf8 = match std::str::from_utf8(head) {
        Ok(_) => true,
        Err(error) => error.error_len().is_none(),
    };
    if utf8 && !head.contains(&0) {
        Fit::Preferred
    } else {
        Fit::No
    }
}

fn nothing_opens(world: &mut World, path: &Path, _: String) {
    let name = path.file_name().unwrap_or(path.as_os_str()).display();
    world
        .workbench_mut()
        .notify(Notification::warning(format!("bEd can't open {name}")));
}
