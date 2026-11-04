use bedtime_code::EditorPanel;
use bedtime_document::TextBuffer;
use bedtime_terminal::TerminalPanel;
use bedtime_workbench::{
    Action, ActionAppExt, ActionInvocation, ActionResult, DocumentId, DocumentReference, Handler,
    InArea, KeyChord, Notification, OpenOptions, Place, Target, WorkbenchWorldExt,
    workspaces::Workspace,
};
use bevy::prelude::*;
use std::{future::Future, pin::Pin};

pub struct New;
impl Action for New {
    const KEY: &'static str = "app.new";
    const LABEL: &'static str = "New";
    const DEFAULT_KEYS: &'static [KeyChord] = &[KeyChord::primary(KeyCode::KeyN)];
}

pub struct NewPlugin;

impl Plugin for NewPlugin {
    fn build(&self, app: &mut App) {
        app.init_non_send::<SaveDialog>()
            .add_action_handler::<New>(Handler::new(new))
            .add_systems(Update, poll_save_dialog);
        app.world_mut().workbench_mut().set_save_as(ask_where);
    }
}

fn new(world: &mut World, invocation: ActionInvocation) -> ActionResult {
    let focused = invocation
        .entity
        .filter(|&panel| world.get_entity(panel).is_ok());
    let area = focused.and_then(|panel| world.get::<InArea>(panel).map(|area| area.0));
    if let (Some(panel), Some(area)) = (focused, area)
        && world.get::<TerminalPanel>(panel).is_some()
    {
        world.spawn((TerminalPanel::default(), Place::in_area(area)));
        world.flush();
        return Ok(());
    }
    // Beside the focused document, else where documents open.
    let beside = focused.filter(|&panel| {
        world.get::<DocumentReference>(panel).is_some() || world.get::<EditorPanel>(panel).is_some()
    });
    let target = match beside.and(area) {
        Some(area) => Target::New { area: Some(area) },
        None => Target::New { area: None },
    };
    world
        .workbench_mut()
        .new_untitled::<TextBuffer>(OpenOptions {
            target,
            ..default()
        })
        .map(|_| ())
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
    let mut dialog = rfd::AsyncFileDialog::new()
        .set_title("Save")
        .set_file_name(format!("{name}.txt"));
    if let Some(workspace) = world.get_resource::<Workspace>() {
        dialog = dialog.set_directory(&workspace.root);
    }
    let previous = world
        .non_send_mut::<SaveDialog>()
        .0
        .replace((document, Box::pin(dialog.save_file())));
    // One picker at a time: the one it replaced is given up.
    if let Some((other, _)) = previous {
        world.workbench_mut().cancel_save_as(other);
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
            if let Err(error) = world
                .workbench_mut()
                .save_untitled_to(document, file.path())
            {
                world
                    .workbench_mut()
                    .notify(Notification::error(format!("Couldn't save: {error}")));
            }
        }
        None => world.workbench_mut().cancel_save_as(document),
    }
}
