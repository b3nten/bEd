//! Save, Undo and Redo for the document a panel references. The commands and
//! their default keys are always registered; the handlers are opt-in, so an
//! app can supply its own.
use crate::*;
use crate::{Workbench, WorkbenchWorldExt};
use bevy::prelude::*;

pub struct SaveDocument;
impl Action for SaveDocument {
    const KEY: &'static str = "document.save";
    const LABEL: &'static str = "Save";
    const DEFAULT_KEYS: &'static [KeyChord] = &[KeyChord::primary(KeyCode::KeyS)];
}

pub struct UndoDocument;
impl Action for UndoDocument {
    const KEY: &'static str = "document.undo";
    const LABEL: &'static str = "Undo";
    const DEFAULT_KEYS: &'static [KeyChord] = &[KeyChord::primary(KeyCode::KeyZ)];
    const REPEATS: bool = true;
}

pub struct RedoDocument;
impl Action for RedoDocument {
    const KEY: &'static str = "document.redo";
    const LABEL: &'static str = "Redo";
    const DEFAULT_KEYS: &'static [KeyChord] = &[
        KeyChord::new(Modifiers::PRIMARY.union(Modifiers::SHIFT), KeyCode::KeyZ),
        KeyChord::primary(KeyCode::KeyY),
    ];
    const REPEATS: bool = true;
}

pub(crate) fn install(app: &mut App) {
    app.register_action::<SaveDocument>()
        .register_action::<UndoDocument>()
        .register_action::<RedoDocument>();
}

impl Workbench {
    /// Handle Save, Undo and Redo globally, for the focused panel's document.
    pub fn register_document_handlers(&mut self) {
        let world = &mut self.world;
        let mut registry = world.resource_mut::<ActionRegistry>();
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
}

/// The state of the document `panel` references.
fn info(world: &World, panel: Option<Entity>) -> Option<DocumentInfo> {
    let reference = world.get::<DocumentReference>(panel?)?;
    world
        .workbench()
        .inspect_document(reference.document())
        .ok()
}

fn target(world: &World, invocation: &ActionInvocation) -> Result<DocumentId, String> {
    let panel = invocation.entity.ok_or("No document is focused")?;
    let reference = world
        .get::<DocumentReference>(panel)
        .ok_or("The focused panel has no document")?;
    Ok(reference.document())
}

fn save(world: &mut World, invocation: ActionInvocation) -> ActionResult {
    let document = target(world, &invocation)?;
    world
        .workbench_mut()
        .save_document(document)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn undo(world: &mut World, invocation: ActionInvocation) -> ActionResult {
    let document = target(world, &invocation)?;
    world
        .workbench_mut()
        .undo_document(document)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn redo(world: &mut World, invocation: ActionInvocation) -> ActionResult {
    let document = target(world, &invocation)?;
    world
        .workbench_mut()
        .redo_document(document)
        .map(|_| ())
        .map_err(|error| error.to_string())
}
