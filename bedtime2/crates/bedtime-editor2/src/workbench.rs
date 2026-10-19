//! Thin binding to Workbench's canonical documents. No second content/history
//! owner, file policy, worker, or renderer plugin is installed by this module.
use crate::*;
use bedtime_ui2::UiContext;
use bedtime_workbench2::*;
use bevy::prelude::*;

#[derive(Component, Default)]
pub struct WorkbenchEditor {
    pub view: EditorView,
    group: Option<DocumentUndoGroup>,
    document: Option<DocumentId>,
}

/// Bind a panel to a text document, resetting presentation only when rebinding.
/// Existing draft preflight remains authoritative.
pub fn bind_text_editor(
    world: &mut World,
    panel: Entity,
    document: DocumentHandle<TextBuffer>,
) -> Result<(), DocumentError> {
    let same = world
        .get::<DocumentView>(panel)
        .is_some_and(|view| view.document() == document.id());
    bind_document_view(world, panel, document)?;
    if !same || world.get::<WorkbenchEditor>(panel).is_none() {
        world.entity_mut(panel).insert(WorkbenchEditor {
            document: Some(document.id()),
            ..Default::default()
        });
    }
    Ok(())
}

pub fn unbind_text_editor(
    world: &mut World,
    panel: Entity,
) -> Result<Option<DocumentId>, DocumentError> {
    check_panel_document_draft(world, panel)?;
    if let Some(mut editor) = world.entity_mut(panel).take::<WorkbenchEditor>() {
        if let (Some(document), Some(group)) = (editor.document, editor.group.take()) {
            let _ = end_document_undo_group(world, document, group);
        }
    }
    Ok(world
        .entity_mut(panel)
        .take::<DocumentView>()
        .map(|view| view.document()))
}

/// Draw inside any host panel. Applications can add controls around this call;
/// the editor draws text, optional numbers/scroll bars and its local Find bar.
pub fn draw_editor(
    ui: &mut egui::Ui,
    world: &mut World,
    context: &UiContext<'_>,
    panel: Entity,
) -> Result<EditorOutput, EditorError> {
    let document = world
        .get::<DocumentView>(panel)
        .ok_or_else(|| EditorError("Editor has no document".into()))?
        .document();
    let handle = cast_document::<TextBuffer>(world, document).map_err(error)?;
    let mut editor = world
        .entity_mut(panel)
        .take::<WorkbenchEditor>()
        .unwrap_or_default();
    if editor.document != Some(document) {
        if world.get::<DocumentDraft>(panel).is_some() {
            world.entity_mut(panel).insert(editor);
            return Err(EditorError(
                "Cannot rebind an editor with an active composition".into(),
            ));
        }
        editor = WorkbenchEditor {
            document: Some(document),
            ..Default::default()
        };
    }
    let mut session = WorkbenchTextSession {
        world,
        handle,
        panel,
        group: &mut editor.group,
    };
    let result = editor.view.show(
        ui,
        egui::Id::new(("bedtime.editor", panel, document)),
        &mut session,
        context.motion,
    );
    world.entity_mut(panel).insert(editor);
    result
}

/// Revision-bound external control, without requiring a draw or a focused view.
pub fn navigate_editor(
    world: &mut World,
    panel: Entity,
    navigation: Navigation,
) -> Result<(), EditorError> {
    let document = world
        .get::<DocumentView>(panel)
        .ok_or_else(|| EditorError("Editor has no document".into()))?
        .document();
    let handle = cast_document::<TextBuffer>(world, document).map_err(error)?;
    let snapshot = TextSnapshot {
        text: read_document(world, handle).map_err(error)?.clone(),
        revision: inspect_document(world, document).map_err(error)?.revision,
    };
    let focus = navigation.focus;
    world
        .get_mut::<WorkbenchEditor>(panel)
        .ok_or_else(|| EditorError("Panel has no editor".into()))?
        .view
        .request_navigation(&snapshot, navigation)?;
    if focus {
        Workbench::for_entity(world, panel)
            .and_then(|mut workbench| workbench.focus_panel(panel))
            .map_err(EditorError)?;
    }
    Ok(())
}

impl DocumentModel for TextBuffer {
    type Change = BufferEdit;
    fn apply(&mut self, change: &BufferEdit) -> Result<BufferEdit, String> {
        change.apply(self)
    }
}

fn error(error: DocumentError) -> EditorError {
    EditorError(error.to_string())
}

struct WorkbenchTextSession<'a> {
    world: &'a mut World,
    handle: DocumentHandle<TextBuffer>,
    panel: Entity,
    group: &'a mut Option<DocumentUndoGroup>,
}
impl TextSession for WorkbenchTextSession<'_> {
    fn snapshot(&self) -> Result<TextSnapshot, EditorError> {
        Ok(TextSnapshot {
            text: read_document(self.world, self.handle)
                .map_err(error)?
                .clone(),
            revision: inspect_document(self.world, self.handle.id())
                .map_err(error)?
                .revision,
        })
    }
    fn changes_since(&self, revision: u64) -> Result<Option<Vec<TextCommit>>, EditorError> {
        Ok(document_changes_since(self.world, self.handle, revision)
            .map_err(error)?
            .map(|commits| {
                commits
                    .into_iter()
                    .map(|commit| TextCommit {
                        revision: commit.revision,
                        changes: commit.changes.map(|changes| {
                            changes
                                .into_iter()
                                .map(|change| TextDelta {
                                    range: change.range,
                                    inserted_bytes: change.replacement.len(),
                                })
                                .collect()
                        }),
                    })
                    .collect()
            }))
    }
    fn edit_buffer(&mut self, revision: u64, change: BufferEdit) -> Result<u64, EditorError> {
        if document_has_drafts(self.world, self.handle.id()) {
            return Err(EditorError(
                "Another view has uncommitted text composition".into(),
            ));
        }
        let group = match *self.group {
            Some(group) => group,
            None => begin_document_undo_group(self.world, self.handle.id(), self.panel)
                .map_err(error)?,
        };
        *self.group = Some(group);
        let result = edit_document(
            self.world,
            self.handle,
            revision,
            change.clone(),
            EditOptions::grouped("Typing", group),
        );
        if result == Err(DocumentError::StaleUndoGroup) {
            let group = begin_document_undo_group(self.world, self.handle.id(), self.panel)
                .map_err(error)?;
            *self.group = Some(group);
            edit_document(
                self.world,
                self.handle,
                revision,
                change,
                EditOptions::grouped("Typing", group),
            )
            .map_err(error)
        } else {
            result.map_err(error)
        }
    }
    fn history(&mut self, action: HistoryAction) -> Result<u64, EditorError> {
        let info = inspect_document(self.world, self.handle.id()).map_err(error)?;
        if info.has_drafts {
            return Err(EditorError("Finish composition before undo/redo".into()));
        }
        match action {
            HistoryAction::Undo if info.can_undo => {
                undo_document(self.world, self.handle.id()).map_err(error)
            }
            HistoryAction::Redo if info.can_redo => {
                redo_document(self.world, self.handle.id()).map_err(error)
            }
            _ => Ok(info.revision),
        }
    }
    fn end_undo_group(&mut self) {
        if let Some(group) = self.group.take() {
            let _ = end_document_undo_group(self.world, self.handle.id(), group);
        }
    }
    fn set_composing(&mut self, _view: egui::Id, composing: bool) {
        if composing {
            self.world.entity_mut(self.panel).insert(DocumentDraft);
        } else {
            self.world.entity_mut(self.panel).remove::<DocumentDraft>();
        }
    }
}

#[cfg(test)]
mod tests;
