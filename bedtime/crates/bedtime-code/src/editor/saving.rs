//! Saving from an editor tidies the text first: formatting it (by its
//! language server), trimming trailing whitespace and ending it with a
//! newline, as the settings say. Only saves someone asks for: editor panels
//! handle Save themselves, which autosave, closing and other programmatic
//! saves don't go through.
//!
//! Formatting is a request: the save waits for its answer (applied, refused
//! or failed) up to `FORMAT_WAIT`, then saves without it.
use super::*;
use bedtime_workbench::{
    Action, ActionAppExt, ActionHandler, ActionInvocation, ActionResult, DocumentId, DocumentInfo,
    DocumentReference, EditOptions, Handler, MenusAppExt, Notification, SaveDocument, StandardMenu,
    WorkbenchWorldExt,
};
use bevy::prelude::*;
use std::time::{Duration, Instant};

/// How long a save waits for formatting.
const FORMAT_WAIT: Duration = Duration::from_secs(2);

/// Format the focused editor's text (and tidy it as the settings say), then
/// save it: whatever format-on-save says.
pub struct FormatAndSave;
impl Action for FormatAndSave {
    const KEY: &'static str = "editor.format_and_save";
    const LABEL: &'static str = "Format and Save";
}

/// Saves waiting for formatting.
#[derive(Resource, Default)]
pub(crate) struct PendingSaves(Vec<Pending>);

struct Pending {
    panel: Entity,
    document: DocumentId,
    /// The formatting request, once the server could take it (it may not
    /// have the latest text yet).
    ticket: Option<u64>,
    since: Instant,
}

pub(crate) fn install(app: &mut App) {
    app.init_resource::<PendingSaves>()
        .add_observer(handle_save)
        .add_action_handler::<FormatAndSave>(Handler::new(format_and_save).available_when(
            |world, panel| {
                panel.is_some_and(|p| world.get::<EditorPanel>(p).is_some())
                    && info(world, panel).is_some_and(|i| i.writable && !i.saving)
            },
        ))
        .add_menu_item::<FormatAndSave>(StandardMenu::File, "save")
        .add_systems(Update, progress);
}

/// Editor panels save themselves (instead of the app's Save).
fn handle_save(add: On<Add<EditorPanel>>, mut commands: Commands) {
    commands
        .entity(add.entity)
        .insert(ActionHandler::<SaveDocument>::new(
            Handler::new(save).available_when(|world, panel| {
                info(world, panel).is_some_and(|i| i.dirty && i.writable && !i.saving)
            }),
        ));
}

fn info(world: &World, panel: Option<Entity>) -> Option<DocumentInfo> {
    let document = world.get::<DocumentReference>(panel?)?.document();
    world.workbench().inspect_document(document).ok()
}

fn save(world: &mut World, invocation: ActionInvocation) -> ActionResult {
    let format = world
        .get_resource::<EditorDefaults>()
        .is_some_and(|defaults| defaults.format_on_save);
    start(world, invocation, format)
}

fn format_and_save(world: &mut World, invocation: ActionInvocation) -> ActionResult {
    start(world, invocation, true)
}

fn start(world: &mut World, invocation: ActionInvocation, format: bool) -> ActionResult {
    let panel = invocation.entity.ok_or("No document is focused")?;
    let document = world
        .get::<DocumentReference>(panel)
        .ok_or("The focused panel has no document")?
        .document();
    #[cfg(unix)]
    if format {
        if crate::lsp::server_of(world, panel).is_some() {
            let mut pending = world.resource_mut::<PendingSaves>();
            pending.0.retain(|p| p.document != document);
            pending.0.push(Pending {
                panel,
                document,
                ticket: None,
                since: Instant::now(),
            });
            progress(world);
            return Ok(());
        }
        // Asked for by name: say why it saved without.
        if invocation.action.as_str() == FormatAndSave::KEY {
            world.workbench_mut().notify(Notification::info(
                "Not formatted: no language server for this file",
            ));
        }
    }
    #[cfg(not(unix))]
    let _ = format;
    finish(world, document)
}

/// Formatting request `ticket` was answered (and its edit applied, if
/// any): save what was waiting for it.
pub(crate) fn formatted(world: &mut World, ticket: u64) {
    let mut pending = world.resource_mut::<PendingSaves>();
    let Some(index) = pending.0.iter().position(|p| p.ticket == Some(ticket)) else {
        return;
    };
    let document = pending.0.remove(index).document;
    let result = finish(world, document);
    report(world, result);
}

/// Ask for formatting once the server can take it (it has the latest
/// text); saves still waiting after `FORMAT_WAIT` go ahead without it.
fn progress(world: &mut World) {
    if world.resource::<PendingSaves>().0.is_empty() {
        return;
    }
    let pending = std::mem::take(&mut world.resource_mut::<PendingSaves>().0);
    let mut waiting = Vec::with_capacity(pending.len());
    for mut save in pending {
        if save.since.elapsed() >= FORMAT_WAIT {
            #[cfg(unix)]
            if let Some(ticket) = save.ticket {
                crate::lsp::actions::forget_format(world, ticket);
            }
            world.workbench_mut().notify(Notification::info(
                "Formatting took too long; saved without it",
            ));
            let result = finish(world, save.document);
            report(world, result);
            continue;
        }
        #[cfg(unix)]
        if save.ticket.is_none() {
            save.ticket = crate::lsp::actions::format_for_save(world, save.panel).ok();
        }
        waiting.push(save);
    }
    // Saves started meanwhile (none: this runs alone) come after these.
    let mut saves = world.resource_mut::<PendingSaves>();
    waiting.append(&mut saves.0);
    saves.0 = waiting;
}

fn report(world: &mut World, result: ActionResult) {
    if let Err(error) = result {
        world
            .workbench_mut()
            .notify(Notification::warning(format!("Save: {error}")));
    }
}

/// Tidy the text as the settings say, then save it.
fn finish(world: &mut World, document: DocumentId) -> ActionResult {
    // Tidying is a nicety: a document that can't take it now still saves.
    let _ = tidy(world, document);
    world
        .workbench_mut()
        .save_document(document)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// Trim trailing whitespace and end the text with a newline, as one undo
/// step. Markdown keeps trailing spaces (two make a line break).
fn tidy(world: &mut World, document: DocumentId) -> Result<(), String> {
    let defaults = world
        .get_resource::<EditorDefaults>()
        .copied()
        .unwrap_or_default();
    if !defaults.trim_trailing_whitespace && !defaults.insert_final_newline {
        return Ok(());
    }
    let info = world
        .workbench()
        .inspect_document(document)
        .map_err(|e| e.to_string())?;
    let markdown =
        crate::syntax::document_language(world, document, &info.title) == Some("markdown");
    let workbench = world.workbench();
    let handle = workbench
        .cast_document::<TextBuffer>(document)
        .map_err(|e| e.to_string())?;
    let text = workbench
        .read_document(handle)
        .map_err(|e| e.to_string())?
        .clone();
    let edits = tidy_edits(
        &text,
        defaults.trim_trailing_whitespace && !markdown,
        defaults.insert_final_newline,
    );
    if edits.is_empty() {
        return Ok(());
    }
    world
        .workbench_mut()
        .edit_document_transaction(handle, info.revision, edits, EditOptions::new("Tidy"))
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// The changes tidying makes, last first (each range still where it was).
pub(crate) fn tidy_edits(text: &TextBuffer, trim: bool, final_newline: bool) -> Vec<BufferEdit> {
    let mut edits = Vec::new();
    if final_newline
        && !text.is_empty()
        && !matches!(text.byte(text.len() - 1), Some(b'\n' | b'\r'))
    {
        edits.push(BufferEdit::new(
            text.len()..text.len(),
            whitespace::ending(text),
        ));
    }
    if trim {
        for row in (0..text.line_count()).rev() {
            let line = text.line_range(row);
            let start = text.trailing_whitespace_start(line.clone());
            if start < line.end {
                edits.push(BufferEdit::new(start..line.end, ""));
            }
        }
    }
    edits
}
