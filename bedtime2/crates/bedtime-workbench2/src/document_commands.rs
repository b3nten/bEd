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
    register_context_command::<SaveDocument>(world).expect("Document Save definition");
    register_context_command::<UndoDocument>(world).expect("Document Undo definition");
    register_context_command::<RedoDocument>(world).expect("Document Redo definition");
}

/// Opt into the standard world-scoped document behavior. Definitions are supplied
/// by WorkbenchCorePlugin; handlers and shortcuts remain application choices.
/// Like scoped handler installation, a second installation is rejected.
pub fn register_document_handlers(world: &mut World) -> Result<(), String> {
    // Check the entire set before installing, avoiding a partially installed set
    // when an application has already supplied one of the standard handlers.
    let commands = [
        ContextCommandId::of::<SaveDocument>(),
        ContextCommandId::of::<UndoDocument>(),
        ContextCommandId::of::<RedoDocument>(),
    ];
    for command in commands {
        if commands::global_handler_exists(world, command) {
            return Err("A standard document command already has a world handler".into());
        }
    }
    install_global_handler::<SaveDocument, _>(world, save)?;
    install_global_handler::<UndoDocument, _>(world, undo)?;
    install_global_handler::<RedoDocument, _>(world, redo)?;
    Ok(())
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
    let workspace = invocation
        .workspace
        .ok_or("No document workspace is available")?;
    let panel = invocation.panel.ok_or("No document viewer is focused")?;
    if world.get::<PanelInstance>(panel).is_none()
        || world.get::<InWorkspace>(panel).map(|owner| owner.0) != Some(workspace.entity())
    {
        return Err("The admitted document viewer is no longer available".into());
    }
    let document = world
        .get::<DocumentView>(panel)
        .ok_or("The admitted panel is not a document viewer")?
        .document();
    let info = inspect_document(world, document).map_err(|error| error.to_string())?;
    if info.workspace != workspace.entity() {
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
