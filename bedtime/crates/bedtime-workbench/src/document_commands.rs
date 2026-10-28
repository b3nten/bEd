//! Save, Undo and Redo for the document a panel references. The commands and
//! their default keys are always registered; the handlers are opt-in, so an
//! app can supply its own.
use crate::*;
use bevy::prelude::*;

pub struct SaveDocument;
impl ContextCommand for SaveDocument {
    const KEY: &'static str = "document.save";
    const LABEL: &'static str = "Save";
    const DEFAULT_KEYS: &'static [KeyChord] = &[KeyChord::primary(KeyCode::KeyS)];
}

pub struct UndoDocument;
impl ContextCommand for UndoDocument {
    const KEY: &'static str = "document.undo";
    const LABEL: &'static str = "Undo";
    const DEFAULT_KEYS: &'static [KeyChord] = &[KeyChord::primary(KeyCode::KeyZ)];
    const REPEATS: bool = true;
}

pub struct RedoDocument;
impl ContextCommand for RedoDocument {
    const KEY: &'static str = "document.redo";
    const LABEL: &'static str = "Redo";
    const DEFAULT_KEYS: &'static [KeyChord] = &[
        KeyChord::new(Modifiers::PRIMARY.union(Modifiers::SHIFT), KeyCode::KeyZ),
        KeyChord::primary(KeyCode::KeyY),
    ];
    const REPEATS: bool = true;
}

pub(crate) fn install(app: &mut App) {
    app.register_command::<SaveDocument>()
        .register_command::<UndoDocument>()
        .register_command::<RedoDocument>();
}

/// Handle Save, Undo and Redo globally, for the focused panel's document.
pub fn register_document_handlers(world: &mut World) {
    let mut registry = world.resource_mut::<CommandRegistry>();
    registry.set_global_handler::<SaveDocument>(Some(Handler::new(save).available_when(
        |world, panel| {
            info(world, panel).is_some_and(|info| info.dirty && info.writable && !info.saving)
        },
    )));
    registry.set_global_handler::<UndoDocument>(Some(Handler::new(undo).available_when(
        |world, panel| info(world, panel).is_some_and(|i| i.undo_label.is_some()),
    )));
    registry.set_global_handler::<RedoDocument>(Some(Handler::new(redo).available_when(
        |world, panel| info(world, panel).is_some_and(|i| i.redo_label.is_some()),
    )));
}

/// The state of the document `panel` references.
fn info(world: &World, panel: Option<Entity>) -> Option<DocumentInfo> {
    let reference = world.get::<DocumentReference>(panel?)?;
    inspect_document(world, reference.document()).ok()
}

fn target(world: &World, invocation: &ContextCommandInvocation) -> Result<DocumentId, String> {
    let panel = invocation.panel.ok_or("No document is focused")?;
    let reference = world
        .get::<DocumentReference>(panel)
        .ok_or("The focused panel has no document")?;
    Ok(reference.document())
}

fn save(world: &mut World, invocation: ContextCommandInvocation) -> ContextCommandResult {
    let document = target(world, &invocation)?;
    save_document(world, document)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn undo(world: &mut World, invocation: ContextCommandInvocation) -> ContextCommandResult {
    let document = target(world, &invocation)?;
    undo_document(world, document)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn redo(world: &mut World, invocation: ContextCommandInvocation) -> ContextCommandResult {
    let document = target(world, &invocation)?;
    redo_document(world, document)
        .map(|_| ())
        .map_err(|error| error.to_string())
}
