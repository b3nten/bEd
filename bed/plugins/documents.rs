//! New and Save As: New makes an untitled text document; saving one asks
//! where.
use bedtime_document::TextBuffer;
use bedtime_workbench::{
    BedtimeWorldExt, DocumentId, NewAppExt, Notification, workspaces::Workspace,
};
use bevy::prelude::*;
use std::{future::Future, pin::Pin};

/// New makes an untitled text document (workbench's `New`; a terminal makes
/// another terminal itself). Saving one asks where, with the system picker.
pub struct DocumentsPlugin;

impl Plugin for DocumentsPlugin {
    fn build(&self, app: &mut App) {
        app.init_non_send::<SaveDialog>()
            .set_new_document::<TextBuffer>()
            .add_systems(Update, poll_save_dialog);
        app.world_mut().bedtime_mut().set_save_as(ask_where);
    }
}

/// The save picker, while it's open: for which document. AppKit owns the
/// dialog, so its future is polled on the main thread.
#[derive(Default)]
struct SaveDialog(Option<(DocumentId, Picking)>);

/// The picker's answer: where to save, or nothing (dismissed).
type Picking = Pin<Box<dyn Future<Output = Option<rfd::FileHandle>>>>;

fn ask_where(world: &mut World, document: DocumentId) {
    let name = world
        .get::<bedtime_workbench::Untitled>(document.0)
        .map(|untitled| untitled.name.clone())
        .unwrap_or_default();
    // A diagram is saved as one; anything else is text.
    let diagram = world
        .bedtime()
        .cast_document::<bedtime_diagram::Diagram>(document)
        .is_ok();
    let extension = if diagram {
        bedtime_diagram::EXTENSION
    } else {
        "txt"
    };
    let mut dialog = rfd::AsyncFileDialog::new()
        .set_title("Save")
        .set_file_name(format!("{name}.{extension}"));
    if let Some(workspace) = world.get_resource::<Workspace>() {
        dialog = dialog.set_directory(&workspace.root);
    }
    let previous = world
        .non_send_mut::<SaveDialog>()
        .0
        .replace((document, Box::pin(dialog.save_file())));
    // One picker at a time: the one it replaced is given up.
    if let Some((other, _)) = previous {
        world.bedtime_mut().cancel_save_as(other);
    }
}

fn poll_save_dialog(world: &mut World) {
    let mut dialog = world.non_send_mut::<SaveDialog>();
    let Some((document, future)) = dialog.0.as_mut() else {
        return;
    };
    let document = *document;
    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
    let std::task::Poll::Ready(chosen) = future.as_mut().poll(&mut context) else {
        return;
    };
    dialog.0 = None;
    match chosen {
        Some(file) => {
            if let Err(error) = world.bedtime_mut().save_untitled_to(document, file.path()) {
                world
                    .bedtime_mut()
                    .notify(Notification::error(format!("Couldn't save: {error}")));
            }
        }
        None => world.bedtime_mut().cancel_save_as(document),
    }
}
