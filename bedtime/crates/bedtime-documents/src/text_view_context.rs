//! Scoped access to a registered view. Document mutations go through commands.
use crate::document_runtime::DocumentRuntime;
use bedtime_editing::{
    document_state::DocumentState,
    editing_events::{DidRequestExclusiveOverlay, EditingEvents, Overlay},
    text_commands::{CursorReveal, TextCommands},
    text_view_state::TextViewState,
};

/// A borrow valid only during DocumentSession::with_view. No mutable document or service ownership escapes.
pub struct TextViewContext<'a> {
    runtime: &'a mut DocumentRuntime,
}

impl<'a> TextViewContext<'a> {
    pub(crate) fn new(runtime: &'a mut DocumentRuntime) -> Self {
        Self { runtime }
    }

    pub fn update_pending_cursor(&mut self) {
        let path = self.runtime.history_key().to_owned();
        if !path.is_empty() {
            let (row, column) = (self.runtime.view.row, self.runtime.view.column);
            self.runtime
                .project_undo()
                .update_pending_cursor(&path, row, column);
        }
    }

    pub fn commands(&mut self) -> TextCommands<'_> {
        self.runtime.commands()
    }

    pub fn view_mut(&mut self) -> &mut TextViewState {
        &mut self.runtime.view
    }

    pub fn state_and_view(&mut self) -> (&DocumentState, &mut TextViewState) {
        (&self.runtime.state, &mut self.runtime.view)
    }

    /// Maintain the fold source identity, and optionally poll syntax candidates.
    /// Stale syntax results never replace ranges transformed by edit handling.
    pub fn refresh_folding(&mut self) {
        let generation = self.runtime.document_generation();
        self.runtime.view.folds.set_source(
            generation,
            &self.runtime.state.path,
            &self.runtime.state.language_id,
        );
        #[cfg(feature = "ide-services")]
        if let Some(ranges) = self.runtime.folding.update(
            &self.runtime.state,
            generation,
            self.runtime.ops.generation(),
        ) {
            self.runtime.view.folds.set_ranges(ranges.to_vec());
        }
    }

    pub fn events(&mut self) -> &mut EditingEvents {
        &mut self.runtime.events
    }

    pub fn api(&mut self) -> TextViewApi<'_> {
        TextViewApi {
            runtime: self.runtime,
        }
    }
}

impl std::ops::Deref for TextViewContext<'_> {
    type Target = DocumentRuntime;

    fn deref(&self) -> &DocumentRuntime {
        self.runtime
    }
}

/// View-local caret/focus/overlay actions, with no document lifecycle access.
pub struct TextViewApi<'a> {
    runtime: &'a mut DocumentRuntime,
}

impl TextViewApi<'_> {
    pub fn request_focus(&mut self) {
        self.runtime.view.request_focus = true;
    }

    pub fn request_ensure_visible(&mut self) {
        self.runtime.commands().request_ensure_visible();
    }

    pub fn request_cursor_center(&mut self, row: i32, column: i32) {
        self.runtime.view.request_cursor_center(row, column);
    }

    pub fn center_on(&mut self, row: i32, column: i32) {
        self.runtime
            .commands()
            .set_cursor(row, column, false, CursorReveal::Center);
    }

    pub fn request_exclusive_overlay(&mut self, keep: Overlay) {
        self.runtime
            .events
            .emit_did_request_exclusive_overlay(&DidRequestExclusiveOverlay { keep });
    }

    pub fn close_all_overlays(&mut self) {
        self.request_exclusive_overlay(Overlay::None);
    }
}
