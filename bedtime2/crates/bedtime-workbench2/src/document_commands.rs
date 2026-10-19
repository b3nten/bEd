//! Contextual document operations. UI bindings are opt-in so hosts retain their
//! own shortcut policy; command handlers always use the admitted panel/workspace.
use crate::*;
use bevy::prelude::*;

pub struct SaveDocument;
impl ContextCommand for SaveDocument {
    const KEY: &'static str = "workbench.document.save";
    const LABEL: &'static str = "Save Document";
}

pub struct UndoDocument;
impl ContextCommand for UndoDocument {
    const KEY: &'static str = "workbench.document.undo";
    const LABEL: &'static str = "Undo";
}

pub struct RedoDocument;
impl ContextCommand for RedoDocument {
    const KEY: &'static str = "workbench.document.redo";
    const LABEL: &'static str = "Redo";
}

pub(crate) fn install(app: &mut App) {
    let world = app.world_mut();
    commands::install_global_handler::<SaveDocument, _>(world, save)
        .expect("Unique document Save handler");
    commands::install_global_handler::<UndoDocument, _>(world, undo)
        .expect("Unique document Undo handler");
    commands::install_global_handler::<RedoDocument, _>(world, redo)
        .expect("Unique document Redo handler");
}

/// Install conventional application-wide Save/Undo/Redo bindings once. Text
/// viewers must route undo/redo before their widget handles input; the general
/// shortcut collector deliberately leaves text-editing chords to widgets.
pub fn register_document_shortcuts(world: &mut World) {
    let bindings = [
        (
            ContextCommandId::of::<SaveDocument>(),
            KeyChord::new(Modifiers::PRIMARY, KeyCode::KeyS),
        ),
        (
            ContextCommandId::of::<UndoDocument>(),
            KeyChord::new(Modifiers::PRIMARY, KeyCode::KeyZ),
        ),
        (
            ContextCommandId::of::<RedoDocument>(),
            KeyChord::new(Modifiers::PRIMARY | Modifiers::SHIFT, KeyCode::KeyZ),
        ),
        (
            ContextCommandId::of::<RedoDocument>(),
            KeyChord::new(Modifiers::PRIMARY, KeyCode::KeyY),
        ),
    ];
    for (command, chord) in bindings {
        let exists = world
            .query::<(&Binding, Option<&InPanel>, Option<&InWorkspace>)>()
            .iter(world)
            .any(|(binding, panel, workspace)| {
                panel.is_none()
                    && workspace.is_none()
                    && binding.command == command
                    && binding.chord == chord
            });
        if !exists {
            world.spawn(Binding {
                command,
                chord,
                repeat: false,
            });
        }
    }
}

fn target(world: &World, invocation: &ContextCommandInvocation) -> Result<DocumentId, String> {
    let panel = invocation.panel.ok_or("No document viewer is focused")?;
    if world.get::<PanelInstance>(panel).is_none()
        || world.get::<InWorkspace>(panel).map(|owner| owner.0)
            != Some(invocation.workspace.entity())
    {
        return Err("The admitted document viewer is no longer available".into());
    }
    let document = world
        .get::<DocumentView>(panel)
        .ok_or("The admitted panel is not a document viewer")?
        .document();
    let info = inspect_document(world, document).map_err(|error| error.to_string())?;
    if info.workspace != invocation.workspace.entity() {
        return Err("The admitted document belongs to a different workspace".into());
    }
    Ok(document)
}

fn save(invocation: In<ContextCommandInvocation>, world: &mut World) -> ContextCommandResult {
    let document = target(world, &invocation)?;
    save_document(world, document)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn undo(invocation: In<ContextCommandInvocation>, world: &mut World) -> ContextCommandResult {
    let document = target(world, &invocation)?;
    undo_document(world, document)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn redo(invocation: In<ContextCommandInvocation>, world: &mut World) -> ContextCommandResult {
    let document = target(world, &invocation)?;
    redo_document(world, document)
        .map(|_| ())
        .map_err(|error| error.to_string())
}
