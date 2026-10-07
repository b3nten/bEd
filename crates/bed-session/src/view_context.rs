//! Scoped access to a registered view. Document mutations go through commands.
use crate::editor::Editor;
use bed_core::{
    editor_commands::{CursorReveal, EditorCommands},
    editor_events::{DidRequestExclusiveOverlay, EditorEvents, Overlay},
    editor_state::EditorState,
    editor_view_state::EditorViewState,
};
/// A borrow valid only during EditorSession::with_view. No mutable document or service ownership escapes.
pub struct ViewContext<'a> {
    editor: &'a mut Editor,
}
impl<'a> ViewContext<'a> {
    pub(crate) fn new(editor: &'a mut Editor) -> Self {
        Self { editor }
    }
    pub fn update_pending_cursor(&mut self) {
        let path = self.editor.history_key().to_owned();
        if !path.is_empty() {
            let (row, column) = (self.editor.view.row, self.editor.view.column);
            self.editor
                .project_undo()
                .update_pending_cursor(&path, row, column);
        }
    }
    pub fn commands(&mut self) -> EditorCommands<'_> {
        self.editor.commands()
    }
    pub fn view_mut(&mut self) -> &mut EditorViewState {
        &mut self.editor.view
    }
    pub fn state_and_view(&mut self) -> (&EditorState, &mut EditorViewState) {
        (&self.editor.state, &mut self.editor.view)
    }
    pub fn events(&mut self) -> &mut EditorEvents {
        &mut self.editor.events
    }
    pub fn api(&mut self) -> ViewApi<'_> {
        ViewApi {
            editor: self.editor,
        }
    }
}
impl std::ops::Deref for ViewContext<'_> {
    type Target = Editor;
    fn deref(&self) -> &Editor {
        self.editor
    }
}
/// View-local caret/focus/overlay actions, with no document lifecycle access.
pub struct ViewApi<'a> {
    editor: &'a mut Editor,
}
impl ViewApi<'_> {
    pub fn request_focus(&mut self) {
        self.editor.view.request_focus = true;
    }
    pub fn request_ensure_visible(&mut self) {
        self.editor.commands().request_ensure_visible();
    }
    pub fn request_cursor_center(&mut self, row: i32, column: i32) {
        self.editor.view.request_cursor_center(row, column);
    }
    pub fn center_on(&mut self, row: i32, column: i32) {
        self.editor
            .commands()
            .set_cursor(row, column, false, CursorReveal::Center);
    }
    pub fn request_exclusive_overlay(&mut self, keep: Overlay) {
        self.editor
            .events
            .emit_did_request_exclusive_overlay(&DidRequestExclusiveOverlay { keep });
    }
    pub fn close_all_overlays(&mut self) {
        self.request_exclusive_overlay(Overlay::None);
    }
}
