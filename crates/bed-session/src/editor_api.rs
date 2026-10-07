//! Document and caret portion of ned editor/editor_api.{h,cpp}; see LICENSE and NOTICE.
//! Document services are coordinated here, outside the command layer.

use std::io;

use crate::editor::Editor;
use bed_core::{
    editor_commands::CursorReveal,
    editor_events::{DidRequestExclusiveOverlay, EditorEvents, Overlay},
    editor_state::{DocumentKind, EditorState},
};

pub struct EditorApi<'a> {
    editor: &'a mut Editor,
}

impl<'a> EditorApi<'a> {
    pub fn new(editor: &'a mut Editor) -> Self {
        Self { editor }
    }

    pub fn prepare_load(&mut self) -> io::Result<()> {
        self.editor.save()?;
        self.reset_caret();
        Ok(())
    }

    pub fn open_document(&mut self, path: &str, raw: &[u8]) {
        self.open_document_with_kind(path, raw, DocumentKind::Text);
    }

    pub fn open_document_with_kind(&mut self, path: &str, raw: &[u8], kind: DocumentKind) {
        let previous = &self.editor.state.path;
        if !previous.is_empty()
            && previous != path
            && let Some(client) = self.editor.lsp_client()
        {
            let mut client = client.borrow_mut();
            if client.is_initialized()
                && let Err(error) = client.did_close(previous)
            {
                eprintln!("Bed: LSP document close failed: {error}");
            }
        }
        self.editor.set_content_with_kind(raw, kind);
        self.editor.state.path = path.to_owned();
        self.editor.state.language_id = if kind == DocumentKind::Text {
            EditorState::language_id_from_path(path)
        } else {
            String::new()
        };
        self.reset_caret();
        if kind == DocumentKind::Bytes {
            return;
        }
        self.editor.with_project_undo(|undo| undo.ensure_file(path));
        self.editor
            .git
            .borrow_mut()
            .on_document_opened(&self.editor.state, self.editor.git_changed_lines());
        self.editor.notify_document_opened();
    }

    pub fn fail_open(&mut self, message: &[u8]) {
        self.editor.set_content(message);
        self.editor.state.path.clear();
        self.editor.state.language_id.clear();
    }

    pub fn on_project_opened(&mut self, root: &str) {
        if let Some(client) = self.editor.lsp_client() {
            client.borrow_mut().set_workspace(root);
        }
        self.editor.git.borrow_mut().init(
            &self.editor.state,
            root,
            self.editor.git_changed_lines(),
        );
    }

    pub fn is_file_modified(&self, path: &str) -> bool {
        self.editor.git.borrow().is_file_modified(path)
    }
    pub fn bind_diagnostics(
        &mut self,
        diagnostics: Option<bed_lsp::diagnostics::diagnostics_store::LspDiagnostics>,
    ) {
        self.editor.diagnostics = diagnostics;
    }

    pub fn set_content(&mut self, raw: &[u8]) {
        self.editor.set_content(raw);
    }
    pub fn path(&self) -> &str {
        &self.editor.state.path
    }
    pub fn has_path(&self) -> bool {
        !self.path().is_empty()
    }
    pub fn text(&self) -> Vec<u8> {
        self.editor.state.join()
    }
    pub fn line(&self, row: i32) -> Vec<u8> {
        self.editor.state.line(row)
    }
    pub fn version(&self) -> i32 {
        self.editor.state.version
    }
    pub fn language_id(&self) -> &str {
        &self.editor.state.language_id
    }
    pub fn get_caret(&self) -> (i32, i32) {
        (self.editor.view.row, self.editor.view.column)
    }
    pub fn reset_caret(&mut self) {
        self.restore_caret(0, 0);
    }
    pub fn restore_caret(&mut self, row: i32, column: i32) {
        self.editor
            .commands()
            .set_cursor(row, column, false, CursorReveal::Ensure);
    }
    pub fn center_on(&mut self, row: i32, column: i32) {
        self.editor
            .commands()
            .set_cursor(row, column, false, CursorReveal::Center);
    }
    pub fn request_ensure_visible(&mut self) {
        self.editor.commands().request_ensure_visible();
    }
    pub fn request_cursor_center(&mut self, row: i32, column: i32) {
        self.editor.view.request_cursor_center(row, column);
    }
    pub fn request_focus(&mut self) {
        self.editor.view.request_focus = true;
    }
    pub fn set_block_input(&mut self, blocked: bool) {
        self.editor.view.block_input = blocked;
    }
    pub fn is_block_input(&self) -> bool {
        self.editor.view.block_input
    }
    pub fn request_exclusive_overlay(&mut self, keep: Overlay) {
        self.editor
            .events
            .emit_did_request_exclusive_overlay(&DidRequestExclusiveOverlay { keep });
    }
    pub fn close_all_overlays(&mut self) {
        self.request_exclusive_overlay(Overlay::None);
    }
    pub fn save(&mut self) -> io::Result<bool> {
        self.editor.save()
    }
    pub fn events(&mut self) -> &mut EditorEvents {
        &mut self.editor.events
    }
}
