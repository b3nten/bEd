//! Composition root translated from ned editor/editor.{h,cpp}; see LICENSE and NOTICE.
//! Rust commands borrow the subsystems for one action instead of keeping self pointers.

use std::{
    cell::{Cell, RefCell},
    io,
    path::Path,
    rc::Rc,
};

use crate::editor_api::EditorApi;
use crate::{git::git_service::EditorGit, save_service::EditorSave};
use bed_core::editor_commands::EditorCommands;
pub use bed_core::editor_commands::ProjectUndoGuard;
use bed_core::{
    editor_events::EditorEvents, editor_operations::EditorOperations, editor_state::EditorState,
    editor_view_state::EditorViewState, project_undo::ProjectUndo,
};
use bed_highlight::highlight_service::EditorHighlight;
use bed_lsp::{diagnostics::diagnostics_store::LspDiagnostics, lsp_client::LspClient};

pub type SharedLspClient = Rc<RefCell<LspClient>>;
/// One project history and disk writer, shared by every embedded editor tab.
pub type SharedProjectUndo = Rc<RefCell<ProjectUndo>>;
type LspBinding = Rc<RefCell<Option<SharedLspClient>>>;

pub struct Editor {
    pub state: EditorState,
    pub view: EditorViewState,
    pub ops: EditorOperations,
    pub undo: ProjectUndo,
    shared_project_undo: Option<SharedProjectUndo>,
    pub events: EditorEvents,
    pub save_service: EditorSave,
    pub highlight: EditorHighlight,
    pub git: Rc<RefCell<EditorGit>>,
    pub diagnostics: Option<LspDiagnostics>,
    git_changed_lines: Rc<Cell<bool>>,
    highlight_dirty: Rc<Cell<bool>>,
    pub disk_conflict: Option<String>,
    document_generation: u64,
    lsp_binding: LspBinding,
    history_key_override: Option<String>,
    active_command_owner: Option<u64>,
    last_command_owner: Option<u64>,
    service_errors: Rc<RefCell<Vec<String>>>,
}

impl Default for Editor {
    fn default() -> Self {
        let mut events = EditorEvents::new();
        let save_service = EditorSave::new();
        save_service.subscribe(&mut events);
        let highlight_dirty = Rc::new(Cell::new(false));
        let dirty = Rc::clone(&highlight_dirty);
        events.subscribe_did_edit(move |_| dirty.set(true));
        let git = Rc::new(RefCell::new(EditorGit::new()));
        let git_changed_lines = Rc::new(Cell::new(true));
        let git_listener = Rc::clone(&git);
        let git_enabled = Rc::clone(&git_changed_lines);
        events.subscribe_did_edit_document(move |event, state| {
            git_listener.borrow_mut().on_did_edit(
                state,
                event.first_row,
                event.last_row,
                git_enabled.get(),
            );
        });
        let lsp_binding: LspBinding = Rc::new(RefCell::new(None));
        let service_errors = Rc::new(RefCell::new(Vec::new()));
        let edit_errors = Rc::clone(&service_errors);
        let lsp_edits = Rc::clone(&lsp_binding);
        events.subscribe_did_edit_document(move |event, state| {
            if let Some(client) = lsp_edits.borrow().as_ref()
                && let Err(error) = client.borrow_mut().did_change(
                    &state.path,
                    event.version,
                    &event.changes,
                    || document_text(state),
                )
            {
                eprintln!("Bed: LSP document change failed: {error}");
                edit_errors.borrow_mut().push(error.to_string());
            }
        });
        let lsp_saves = Rc::clone(&lsp_binding);
        let save_errors = Rc::clone(&service_errors);
        events.subscribe_did_save_document(move |event, state| {
            if let Some(client) = lsp_saves.borrow().as_ref()
                && let Err(error) = client
                    .borrow_mut()
                    .did_save(&event.path, || document_text(state))
            {
                eprintln!("Bed: LSP document save failed: {error}");
                save_errors.borrow_mut().push(error.to_string());
            }
        });
        Self {
            state: EditorState::new(),
            view: EditorViewState::new(),
            ops: EditorOperations::new(),
            undo: ProjectUndo::default(),
            shared_project_undo: None,
            events,
            save_service,
            highlight: EditorHighlight::new(),
            git,
            diagnostics: None,
            git_changed_lines,
            highlight_dirty,
            disk_conflict: None,
            document_generation: 0,
            lsp_binding,
            history_key_override: None,
            active_command_owner: None,
            last_command_owner: None,
            service_errors,
        }
    }
}

impl Editor {
    /// Scoped command/view access for a standalone document composition.
    pub fn view_context(&mut self) -> crate::view_context::ViewContext<'_> {
        crate::view_context::ViewContext::new(self)
    }

    pub fn new() -> Self {
        Self::default()
    }

    pub fn commands(&mut self) -> EditorCommands<'_> {
        let history_key = self.history_key().to_owned();
        if self.active_command_owner != self.last_command_owner {
            self.project_undo().seal_file(&history_key);
            self.last_command_owner = self.active_command_owner;
        }
        let undo = match &self.shared_project_undo {
            Some(shared) => ProjectUndoGuard::Shared(shared.borrow_mut()),
            None => ProjectUndoGuard::Borrowed(&mut self.undo),
        };
        EditorCommands::new_with_project_undo(
            &mut self.state,
            &mut self.view,
            &mut self.ops,
            undo,
            &mut self.events,
        )
        .with_history_key(history_key)
    }

    pub(crate) fn set_command_owner(&mut self, owner: Option<u64>) {
        self.active_command_owner = owner;
        self.events.set_view_scope(owner);
    }

    pub(crate) fn set_history_key(&mut self, key: Option<String>) {
        self.history_key_override = key;
    }

    pub(crate) fn history_key(&self) -> &str {
        self.history_key_override
            .as_deref()
            .unwrap_or(&self.state.path)
    }
    pub(crate) fn take_service_errors(&self) -> Vec<String> {
        std::mem::take(&mut *self.service_errors.borrow_mut())
    }

    /// Bind before opening a tab's document. The shell owns the shared store
    /// and loads it once when opening a project, as in upstream Workbench.
    /// Existing standalone history remains in `undo`; it is not merged.
    pub fn bind_project_undo(&mut self, undo: SharedProjectUndo) {
        self.shared_project_undo = Some(undo);
    }

    /// Borrow the history used by commands, document opens and disk reloads.
    pub fn project_undo(&mut self) -> ProjectUndoGuard<'_> {
        match &self.shared_project_undo {
            Some(shared) => ProjectUndoGuard::Shared(shared.borrow_mut()),
            None => ProjectUndoGuard::Borrowed(&mut self.undo),
        }
    }

    pub fn with_project_undo<R>(&mut self, f: impl FnOnce(&mut ProjectUndo) -> R) -> R {
        f(&mut self.project_undo())
    }

    pub fn api(&mut self) -> EditorApi<'_> {
        EditorApi::new(self)
    }

    pub fn set_content(&mut self, raw: &[u8]) {
        self.document_generation = self.document_generation.wrapping_add(1);
        self.disk_conflict = None;
        self.save_service.cancel_pending();
        self.state.set_from_bytes(raw);
        self.ops.clear_pending();
        self.ops.bump_generation();
        self.highlight
            .reset_for_document(&self.state, self.state.line_count() as usize);
        self.highlight_dirty.set(true);
    }

    /// Distinguishes replacing a document from sequential edits, whose version
    /// restarts at zero on install. Presentation caches observe this identity.
    pub fn document_generation(&self) -> u64 {
        self.document_generation
    }

    pub(crate) fn rebind_document_path(&mut self, path: &str) {
        self.document_generation = self.document_generation.wrapping_add(1);
        self.state.path = path.to_owned();
        self.state.language_id = EditorState::language_id_from_path(path);
        self.ops.bump_generation();
        self.highlight
            .reset_for_document(&self.state, self.state.line_count() as usize);
        self.refresh_highlighting();
    }

    pub fn open(&mut self, path: &Path) -> io::Result<()> {
        let absolute = std::fs::canonicalize(path)?;
        let path = absolute
            .to_str()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Path is not UTF-8"))?;
        // Save before reading: reopening the current path must install the bytes
        // just saved, rather than an earlier disk version. Reset the view only
        // after a successful read so failed opens preserve the current caret.
        self.save()?;
        let raw = std::fs::read(&absolute)?;
        self.api().open_document(path, &raw);
        self.view.request_focus = true;
        Ok(())
    }

    pub fn save(&mut self) -> io::Result<bool> {
        if self.state.dirty && self.disk_conflict.is_some() {
            return Err(io::Error::other(
                "File changed on disk; reload or keep the buffer before saving",
            ));
        }
        self.save_service.save(&mut self.state, &mut self.events)
    }

    pub fn poll_save(&mut self) -> io::Result<bool> {
        if self.disk_conflict.is_some() {
            self.save_service.cancel_pending();
            return Ok(false);
        }
        self.save_service.poll(&mut self.state, &mut self.events)
    }
    pub fn poll_services(&mut self) {
        if let Some(client) = self.lsp_client() {
            client.borrow_mut().poll();
        }
        self.poll_document_services();
    }

    pub(crate) fn poll_document_services(&mut self) {
        self.git.borrow_mut().poll();
        if self.highlight_dirty.replace(false) {
            self.highlight.highlight_content(&self.state, &mut self.ops);
        }
        self.highlight.poll(&self.state, &self.ops);
    }
    pub fn refresh_highlighting(&mut self) {
        self.highlight_dirty.set(true);
    }
    pub fn set_git_changed_lines(&mut self, enabled: bool) {
        self.git_changed_lines.set(enabled);
    }
    pub fn git_changed_lines(&self) -> bool {
        self.git_changed_lines.get()
    }

    pub fn bind_lsp_client(&mut self, client: Option<SharedLspClient>) {
        self.diagnostics = client.as_ref().map(|client| client.borrow().diagnostics());
        *self.lsp_binding.borrow_mut() = client;
    }
    pub fn lsp_client(&self) -> Option<SharedLspClient> {
        self.lsp_binding.borrow().clone()
    }
    pub fn notify_document_opened(&self) {
        if self.state.path.is_empty() {
            return;
        }
        if let Some(client) = self.lsp_client() {
            let mut client = client.borrow_mut();
            let result = client.init(&self.state.path).and_then(|_| {
                client.did_open(
                    &self.state.path,
                    &self.state.join(),
                    self.state.version,
                    &self.state.language_id,
                )
            });
            if let Err(error) = result {
                eprintln!("Bed: LSP document open failed: {error}");
            }
        }
    }
}

fn document_text(state: &EditorState) -> io::Result<String> {
    String::from_utf8(state.join())
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}
