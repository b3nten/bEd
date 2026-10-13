//! Document and caret portion of ned editor/editor_api.{h,cpp}; see LICENSE and NOTICE.
//! Document services are coordinated here, outside the command layer.

use std::io;

use crate::document_runtime::DocumentRuntime;
use bed_editing::{
    document_state::{DocumentKind, DocumentState},
    editing_events::{DidRequestExclusiveOverlay, EditingEvents, Overlay},
    text_commands::CursorReveal,
};

pub struct DocumentApi<'a> {
    runtime: &'a mut DocumentRuntime,
}

impl<'a> DocumentApi<'a> {
    pub fn new(runtime: &'a mut DocumentRuntime) -> Self {
        Self { runtime }
    }

    pub fn prepare_load(&mut self) -> io::Result<()> {
        self.runtime.save()?;
        self.reset_caret();
        Ok(())
    }

    pub fn open_document(&mut self, path: &str, raw: &[u8]) {
        self.open_document_with_kind(path, raw, DocumentKind::Text);
    }

    pub fn open_document_with_kind(&mut self, path: &str, raw: &[u8], kind: DocumentKind) {
        #[cfg(feature = "ide-services")]
        let previous = &self.runtime.state.path;
        #[cfg(feature = "ide-services")]
        if !previous.is_empty()
            && previous != path
            && let Some(client) = self.runtime.lsp_client()
        {
            let mut client = client.borrow_mut();
            if client.is_initialized()
                && let Err(error) = client.did_close(previous)
            {
                eprintln!("Bed: LSP document close failed: {error}");
            }
        }
        self.runtime.set_content_with_kind(raw, kind);
        self.runtime.state.path = path.to_owned();
        self.runtime.state.language_id = if kind == DocumentKind::Text {
            DocumentState::language_id_from_path(path)
        } else {
            String::new()
        };
        self.reset_caret();
        if kind == DocumentKind::Bytes {
            return;
        }
        self.runtime
            .with_project_undo(|undo| undo.ensure_file(path));
        #[cfg(feature = "ide-services")]
        self.runtime
            .git
            .borrow_mut()
            .on_document_opened(&self.runtime.state, self.runtime.git_changed_lines());
        #[cfg(feature = "ide-services")]
        self.runtime.notify_document_opened();
    }

    pub fn fail_open(&mut self, message: &[u8]) {
        self.runtime.set_content(message);
        self.runtime.state.path.clear();
        self.runtime.state.language_id.clear();
    }

    #[cfg(feature = "ide-services")]
    pub fn on_project_opened(&mut self, root: &str) {
        if let Some(client) = self.runtime.lsp_client() {
            client.borrow_mut().set_workspace(root);
        }
        self.runtime.git.borrow_mut().init(
            &self.runtime.state,
            root,
            self.runtime.git_changed_lines(),
        );
    }

    #[cfg(feature = "ide-services")]
    pub fn is_file_modified(&self, path: &str) -> bool {
        self.runtime.git.borrow().is_file_modified(path)
    }

    #[cfg(feature = "ide-services")]
    pub fn bind_diagnostics(
        &mut self,
        diagnostics: Option<bed_lsp::diagnostics::diagnostics_store::LspDiagnostics>,
    ) {
        self.runtime.diagnostics = diagnostics;
    }

    pub fn set_content(&mut self, raw: &[u8]) {
        self.runtime.set_content(raw);
    }

    pub fn path(&self) -> &str {
        &self.runtime.state.path
    }

    pub fn has_path(&self) -> bool {
        !self.path().is_empty()
    }

    pub fn text(&self) -> Vec<u8> {
        self.runtime.state.join()
    }

    pub fn line(&self, row: i32) -> Vec<u8> {
        self.runtime.state.line(row)
    }

    pub fn version(&self) -> i32 {
        self.runtime.state.version
    }

    pub fn language_id(&self) -> &str {
        &self.runtime.state.language_id
    }

    pub fn get_caret(&self) -> (i32, i32) {
        (self.runtime.view.row, self.runtime.view.column)
    }

    pub fn reset_caret(&mut self) {
        self.restore_caret(0, 0);
    }

    pub fn restore_caret(&mut self, row: i32, column: i32) {
        self.runtime
            .commands()
            .set_cursor(row, column, false, CursorReveal::Ensure);
    }

    pub fn center_on(&mut self, row: i32, column: i32) {
        self.runtime
            .commands()
            .set_cursor(row, column, false, CursorReveal::Center);
    }

    pub fn request_ensure_visible(&mut self) {
        self.runtime.commands().request_ensure_visible();
    }

    pub fn request_cursor_center(&mut self, row: i32, column: i32) {
        self.runtime.view.request_cursor_center(row, column);
    }

    pub fn request_focus(&mut self) {
        self.runtime.view.request_focus = true;
    }

    pub fn set_block_input(&mut self, blocked: bool) {
        self.runtime.view.block_input = blocked;
    }

    pub fn is_block_input(&self) -> bool {
        self.runtime.view.block_input
    }

    pub fn request_exclusive_overlay(&mut self, keep: Overlay) {
        self.runtime
            .events
            .emit_did_request_exclusive_overlay(&DidRequestExclusiveOverlay { keep });
    }

    pub fn close_all_overlays(&mut self) {
        self.request_exclusive_overlay(Overlay::None);
    }

    pub fn save(&mut self) -> io::Result<bool> {
        self.runtime.save()
    }

    pub fn events(&mut self) -> &mut EditingEvents {
        &mut self.runtime.events
    }
}
