//! Shared documents and services for the standalone shell and host-owned views.
//! ned's editing algorithms remain in state/operations/commands. This ownership
//! extension is Bed-specific; attribution: LICENSE, NOTICE, UPSTREAM_REVISION.
use crate::editor::{Editor, SharedProjectUndo};
use bed_core::{
    editor_commands::EditorCommands,
    editor_events::{DidEdit, DidSave},
    editor_operations::{OpKind, PendingEdit},
    editor_state::EditorState,
    editor_view_state::EditorViewState,
    project_undo::ProjectUndo,
};
use bed_files::{
    file_monitor::{FileChangeKind, FileMonitor},
    files::read_file_raw,
};
use bed_lsp::workspace_lsp::{WorkspaceLsp, WorkspaceLspEvent};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
    rc::{Rc, Weak},
    time::{Duration, Instant},
};

#[path = "remote_session.rs"]
mod remote_session;
use remote_session::RemoteSession;

pub use bed_core::identity::{DocumentId, ViewId, WorkspaceId};

/// No service, settings seeding, GUI, clipboard or global directory is touched
/// by defaults. Disk/process services require explicit host configuration.
#[derive(Clone, Debug, Default)]
pub struct SessionOptions {
    pub project_root: Option<PathBuf>,
    pub autosave: Option<Duration>,
    pub monitoring: bool,
    pub git: bool,
    pub highlighting: bool,
    pub persistent_history: bool,
    pub lsp_config: Option<PathBuf>,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ClosePolicy {
    Save,
    Discard,
    #[default]
    Cancel,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceError {
    pub document: Option<DocumentId>,
    pub service: &'static str,
    pub message: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionEvent {
    Opened {
        document: DocumentId,
    },
    Edited {
        document: DocumentId,
        view: Option<ViewId>,
        generation: u64,
        edit: DidEdit,
    },
    Saved {
        document: DocumentId,
        save: DidSave,
    },
    Reloaded {
        document: DocumentId,
        generation: u64,
    },
    Conflict {
        document: DocumentId,
        message: String,
    },
    PathChanged {
        document: DocumentId,
        previous: String,
        path: String,
    },
    ViewDetached {
        document: DocumentId,
        view: ViewId,
    },
    Closed {
        document: DocumentId,
    },
}
#[derive(Debug, Default)]
pub struct TickReport {
    pub events: Vec<SessionEvent>,
    pub errors: Vec<ServiceError>,
    pub lsp_events: Vec<WorkspaceLspEvent>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocumentSnapshot {
    pub id: DocumentId,
    pub path: String,
    pub bytes: Vec<u8>,
    pub version: i32,
    pub generation: u64,
    pub dirty: bool,
    pub language_id: String,
    pub utf8_bom: bool,
    pub line_ending: Vec<u8>,
    pub disk_conflict: Option<String>,
}
enum Notification {
    Edit(DidEdit),
    Save(DidSave),
}
struct ViewEntry {
    state: EditorViewState,
    lifetime: Option<Weak<()>>,
}
impl ViewEntry {
    fn live(&self) -> bool {
        self.lifetime
            .as_ref()
            .is_none_or(|token| token.strong_count() > 0)
    }
}
struct DocumentEntry {
    editor: Editor,
    views: BTreeMap<ViewId, ViewEntry>,
    monitor: FileMonitor,
    notifications: Rc<RefCell<Vec<Notification>>>,
    highlight_edits: Vec<PendingEdit>,
    removed: bool,
    allow_recreate: bool,
    autosave_paused: bool,
    git_initialized: bool,
}
/// Main-thread, GUI-independent document registry. Public mutation uses commands
/// rather than mutable Editor/state references, preserving service/view updates.
pub struct EditorSession {
    remote: Option<RemoteSession>,
    workspace: WorkspaceId,
    options: SessionOptions,
    documents: BTreeMap<DocumentId, DocumentEntry>,
    paths: BTreeMap<PathBuf, DocumentId>,
    views: BTreeMap<ViewId, DocumentId>,
    history: SharedProjectUndo,
    lsp: Option<WorkspaceLsp>,
    queued: Vec<SessionEvent>,
    errors: Vec<ServiceError>,
    last_monitor: Option<Instant>,
    last_history: Option<Instant>,
    stopped: bool,
    theme: bed_highlight::tree_sitter::ThemeColors,
}
impl Default for EditorSession {
    fn default() -> Self {
        Self {
            remote: None,
            workspace: WorkspaceId::next(),
            options: SessionOptions::default(),
            documents: BTreeMap::new(),
            paths: BTreeMap::new(),
            views: BTreeMap::new(),
            history: Rc::new(RefCell::new(ProjectUndo::default())),
            lsp: None,
            queued: Vec::new(),
            errors: Vec::new(),
            last_monitor: None,
            last_history: None,
            stopped: false,
            theme: Default::default(),
        }
    }
}

impl EditorSession {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn with_options(mut options: SessionOptions) -> io::Result<Self> {
        if let Some(root) = &options.project_root {
            options.project_root = Some(canonical_root(root)?);
        }
        if (options.persistent_history || options.lsp_config.is_some())
            && options.project_root.is_none()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Configured services require an explicit project root",
            ));
        }
        let mut session = Self::default();
        session.options = options;
        if session.options.persistent_history {
            session
                .history
                .borrow_mut()
                .load_project(path_string(session.options.project_root.as_ref().unwrap())?);
        }
        session.initialize_lsp();
        Ok(session)
    }
    pub fn workspace_id(&self) -> WorkspaceId {
        self.workspace
    }
    pub fn options(&self) -> &SessionOptions {
        &self.options
    }
    pub fn view_ids(&self, document: DocumentId) -> Vec<ViewId> {
        self.documents
            .get(&document)
            .map_or_else(Vec::new, |entry| {
                entry
                    .views
                    .iter()
                    .filter_map(|(&id, view)| view.live().then_some(id))
                    .collect()
            })
    }
    pub fn set_highlight_theme(&mut self, theme: bed_highlight::tree_sitter::ThemeColors) {
        self.theme = theme;
        for entry in self.documents.values_mut() {
            entry.editor.highlight.set_theme_colors(self.theme.clone());
            entry.editor.refresh_highlighting();
        }
    }
    pub fn set_git_enabled(&mut self, enabled: bool) -> io::Result<()> {
        let mut options = self.options.clone();
        options.git = enabled;
        self.configure(options)
    }
    pub fn set_highlighting_enabled(&mut self, enabled: bool) -> io::Result<()> {
        let mut options = self.options.clone();
        options.highlighting = enabled;
        self.configure(options)
    }
    pub fn pause_autosave(&mut self, document: DocumentId, paused: bool) -> io::Result<()> {
        self.entry_mut(document)?.autosave_paused = paused;
        Ok(())
    }
    pub fn document_ids(&self) -> Vec<DocumentId> {
        self.documents.keys().copied().collect()
    }
    pub fn document_for_path(&self, path: &Path) -> Option<DocumentId> {
        if self.remote.is_some() {
            return self
                .paths
                .get(path)
                .copied()
                .or_else(|| self.remote_alias(path));
        }
        canonical_binding(path)
            .ok()
            .and_then(|path| self.paths.get(&path).copied())
    }
    pub fn document_for_view(&self, view: ViewId) -> Option<DocumentId> {
        self.views.get(&view).copied().filter(|id| {
            self.documents
                .get(id)
                .and_then(|d| d.views.get(&view))
                .is_some_and(ViewEntry::live)
        })
    }
    pub fn view_count(&self, document: DocumentId) -> usize {
        self.documents
            .get(&document)
            .map_or(0, |e| e.views.values().filter(|v| v.live()).count())
    }
    pub fn snapshot(&self, document: DocumentId) -> io::Result<DocumentSnapshot> {
        let entry = self.entry(document)?;
        let state = &entry.editor.state;
        Ok(DocumentSnapshot {
            id: document,
            path: state.path.clone(),
            bytes: state.join(),
            version: state.version,
            generation: entry.editor.document_generation(),
            dirty: state.dirty,
            language_id: state.language_id.clone(),
            utf8_bom: state.utf8_bom,
            line_ending: state.line_ending.clone(),
            disk_conflict: entry.editor.disk_conflict.clone(),
        })
    }
    pub fn document_snapshot(&self, document: DocumentId) -> io::Result<DocumentSnapshot> {
        self.snapshot(document)
    }
    pub fn with_document<R>(
        &self,
        document: DocumentId,
        f: impl FnOnce(&EditorState) -> R,
    ) -> io::Result<R> {
        Ok(f(&self.entry(document)?.editor.state))
    }
    pub fn view_snapshot(&self, view: ViewId) -> io::Result<EditorViewState> {
        let id = self.document_for_view(view).ok_or_else(missing_view)?;
        Ok(self.documents[&id].views[&view].state.clone())
    }
    pub fn create_document(&mut self, bytes: &[u8]) -> io::Result<DocumentId> {
        self.ensure_running()?;
        let id = DocumentId::next();
        let mut editor = Editor::new();
        editor.bind_project_undo(Rc::clone(&self.history));
        editor.set_history_key(Some(format!("bed:untitled:{}", id.0)));
        editor.set_content(bytes);
        self.install_entry(id, editor, FileMonitor::new());
        Ok(id)
    }
    pub fn open_file(&mut self, path: &Path) -> io::Result<DocumentId> {
        self.ensure_running()?;
        if self.remote.is_some() {
            return self.request_open_file(path)?.ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "Remote open queued; poll session events for completion",
                )
            });
        }
        let path = std::fs::canonicalize(path)?;
        if let Some(&id) = self.paths.get(&path) {
            return Ok(id);
        }
        let raw = read_file_raw(&path)?;
        let mut monitor = FileMonitor::new();
        // An explicit open retains its stable load baseline even when polling
        // is disabled, so enabling monitoring cannot adopt a later disk edit.
        if !raw.truncated {
            monitor.watch_bytes(&path, &raw.raw)?;
        }
        let id = DocumentId::next();
        let mut editor = Editor::new();
        editor.bind_project_undo(Rc::clone(&self.history));
        editor.set_git_changed_lines(self.options.git);
        editor.api().open_document(path_string(&path)?, &raw.raw);
        self.paths.insert(path, id);
        self.install_entry(id, editor, monitor);
        Ok(id)
    }
    fn install_entry(&mut self, id: DocumentId, mut editor: Editor, monitor: FileMonitor) {
        editor.highlight.enabled = self.options.highlighting;
        editor.highlight.use_bundled_queries();
        editor.set_git_changed_lines(self.options.git);
        editor.highlight.set_theme_colors(self.theme.clone());
        if let Some(idle) = self.options.autosave {
            editor
                .save_service
                .set_autosave_idle_ms(idle.as_millis().min(i32::MAX as u128) as i32);
        }
        if self.options.git && self.remote.is_none() {
            let root = self
                .options
                .project_root
                .as_ref()
                .and_then(|p| p.to_str())
                .unwrap_or("");
            editor.git.borrow_mut().init(&editor.state, root, true);
        }
        let notifications = Rc::new(RefCell::new(Vec::new()));
        let edits = Rc::clone(&notifications);
        editor.events.subscribe_did_edit(move |event| {
            edits.borrow_mut().push(Notification::Edit(event.clone()))
        });
        let saves = Rc::clone(&notifications);
        editor.events.subscribe_did_save(move |event| {
            saves.borrow_mut().push(Notification::Save(event.clone()))
        });
        self.documents.insert(
            id,
            DocumentEntry {
                editor,
                views: BTreeMap::new(),
                monitor,
                notifications,
                highlight_edits: Vec::new(),
                removed: false,
                allow_recreate: false,
                autosave_paused: false,
                git_initialized: self.options.git,
            },
        );
        self.attach_lsp(id);
        self.queued.push(SessionEvent::Opened { document: id });
    }
    pub fn create_view(&mut self, document: DocumentId) -> io::Result<ViewId> {
        self.create_view_entry(document, None)
    }
    pub fn create_managed_view(&mut self, document: DocumentId) -> io::Result<(ViewId, Rc<()>)> {
        let token = Rc::new(());
        let id = self.create_view_entry(document, Some(Rc::downgrade(&token)))?;
        Ok((id, token))
    }
    fn create_view_entry(
        &mut self,
        document: DocumentId,
        lifetime: Option<Weak<()>>,
    ) -> io::Result<ViewId> {
        self.ensure_running()?;
        let view = ViewId::next();
        self.entry_mut(document)?.views.insert(
            view,
            ViewEntry {
                state: EditorViewState::new(),
                lifetime,
            },
        );
        self.views.insert(view, document);
        Ok(view)
    }
    /// Borrow one view; restoration and sibling transforms run even when the closure unwinds.
    pub fn with_view<R>(
        &mut self,
        view: ViewId,
        f: impl FnOnce(&mut crate::view_context::ViewContext<'_>) -> R,
    ) -> io::Result<R> {
        self.with_editor_view(view, |editor| f(&mut editor.view_context()))
    }
    pub fn with_commands<R>(
        &mut self,
        view: ViewId,
        f: impl FnOnce(&mut EditorCommands<'_>) -> R,
    ) -> io::Result<R> {
        self.with_editor_view(view, |editor| f(&mut editor.commands()))
    }
    pub(crate) fn with_editor_view<R>(
        &mut self,
        view: ViewId,
        f: impl FnOnce(&mut Editor) -> R,
    ) -> io::Result<R> {
        self.ensure_running()?;
        self.prune_views();
        let id = self.document_for_view(view).ok_or_else(missing_view)?;
        let entry = self.documents.get_mut(&id).unwrap();
        let mut positions: BTreeMap<ViewId, Vec<(usize, usize)>> = entry
            .views
            .iter()
            .filter(|(v, _)| **v != view)
            .map(|(&v, info)| {
                (
                    v,
                    info.state
                        .selections
                        .iter()
                        .map(|s| {
                            (
                                entry
                                    .editor
                                    .state
                                    .offset_from_row_col(s.head_row, s.head_column),
                                entry
                                    .editor
                                    .state
                                    .offset_from_row_col(s.anchor_row, s.anchor_column),
                            )
                        })
                        .collect(),
                )
            })
            .collect();
        let generation = entry.editor.document_generation();
        std::mem::swap(
            &mut entry.editor.view,
            &mut entry.views.get_mut(&view).unwrap().state,
        );
        entry.editor.set_command_owner(Some(view.0));
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&mut entry.editor)));
        entry.editor.set_command_owner(None);
        std::mem::swap(
            &mut entry.editor.view,
            &mut entry.views.get_mut(&view).unwrap().state,
        );
        let edits = entry.editor.ops.take_pending();
        if generation != entry.editor.document_generation() {
            for info in entry.views.values_mut() {
                info.state.clamp_all(&entry.editor.state);
            }
        } else if !edits.is_empty() {
            for selections in positions.values_mut() {
                for (head, anchor) in selections {
                    for edit in &edits {
                        transform_offset(head, edit);
                        transform_offset(anchor, edit);
                    }
                }
            }
            for (sibling, selections) in positions {
                let info = entry.views.get_mut(&sibling).unwrap();
                for (selection, (head, anchor)) in info.state.selections.iter_mut().zip(selections)
                {
                    (selection.head_row, selection.head_column) =
                        entry.editor.state.row_col_from_offset(head);
                    (selection.anchor_row, selection.anchor_column) =
                        entry.editor.state.row_col_from_offset(anchor);
                }
                info.state.clamp_all(&entry.editor.state);
            }
        }
        let changed = generation != entry.editor.document_generation() || !edits.is_empty();
        if self.options.highlighting {
            entry.highlight_edits.extend(edits);
            if entry.highlight_edits.len() > 4096 {
                entry.highlight_edits.clear();
                entry.editor.highlight.reset_for_document(
                    &entry.editor.state,
                    entry.editor.state.line_count() as usize,
                );
            }
        }
        self.collect_notifications(id, Some(view));
        if changed {
            self.update_lsp_snapshot(id);
        }
        match result {
            Ok(value) => Ok(value),
            Err(panic) => std::panic::resume_unwind(panic),
        }
    }
    pub fn set_scroll(&mut self, view: ViewId, x: f32, y: f32) -> io::Result<()> {
        let doc = self.document_for_view(view).ok_or_else(missing_view)?;
        self.documents
            .get_mut(&doc)
            .unwrap()
            .views
            .get_mut(&view)
            .unwrap()
            .state
            .request_scroll(x, y);
        Ok(())
    }
    pub fn request_focus(&mut self, view: ViewId) -> io::Result<()> {
        let doc = self.document_for_view(view).ok_or_else(missing_view)?;
        self.documents
            .get_mut(&doc)
            .unwrap()
            .views
            .get_mut(&view)
            .unwrap()
            .state
            .request_focus = true;
        Ok(())
    }
    pub fn detach_view(&mut self, view: ViewId) -> bool {
        let Some(doc) = self.views.remove(&view) else {
            return false;
        };
        if let Some(entry) = self.documents.get_mut(&doc) {
            entry.views.remove(&view);
        }
        self.queued.push(SessionEvent::ViewDetached {
            document: doc,
            view,
        });
        true
    }
    pub fn close_view(&mut self, view: ViewId, policy: ClosePolicy) -> io::Result<bool> {
        if policy == ClosePolicy::Cancel {
            return Ok(false);
        }
        self.prune_views();
        let Some(doc) = self.document_for_view(view) else {
            return Ok(false);
        };
        if self.view_count(doc) == 1 {
            self.close_document(doc, policy)
        } else {
            Ok(self.detach_view(view))
        }
    }
    pub fn close_document(
        &mut self,
        document: DocumentId,
        policy: ClosePolicy,
    ) -> io::Result<bool> {
        if policy == ClosePolicy::Cancel || !self.documents.contains_key(&document) {
            return Ok(false);
        }
        if policy == ClosePolicy::Save && self.save_pending(document) {
            return Ok(false);
        }
        if policy == ClosePolicy::Save && self.entry(document)?.editor.state.dirty {
            if self.entry(document)?.editor.state.path.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Save As is required before closing an unnamed document",
                ));
            }
            self.save(document)?;
            if self.entry(document)?.editor.state.dirty {
                return Ok(false);
            }
        }
        if let Some(pool) = &mut self.lsp
            && let Err(error) = pool.unregister_document(document)
        {
            self.record_error(Some(document), "lsp", &error);
        }
        let mut entry = self.documents.remove(&document).unwrap();
        let history_key = entry.editor.history_key().to_owned();
        if entry.editor.state.path.is_empty() {
            self.history.borrow_mut().remove_memory_file(&history_key);
        } else if entry.removed {
            self.history.borrow_mut().forget_file(&history_key);
        }
        entry.editor.save_service.cancel_pending();
        entry.editor.bind_lsp_client(None);
        entry.editor.highlight.cancel_highlighting();
        for view in entry.views.keys() {
            self.views.remove(view);
        }
        self.paths.retain(|_, id| *id != document);
        self.forget_remote_document(document, true);
        self.queued.push(SessionEvent::Closed { document });
        Ok(true)
    }
}

impl EditorSession {
    pub fn save(&mut self, document: DocumentId) -> io::Result<bool> {
        self.ensure_running()?;
        if self.remote.is_some() {
            return self.queue_remote_save(document, None);
        }
        let monitoring = self.options.monitoring;
        let entry = self.entry_mut(document)?;
        guard_write(entry)?;
        let saved = entry.editor.save()?;
        if saved {
            entry.removed = false;
            entry.allow_recreate = false;
            if monitoring {
                monitor_saved_bytes(entry)?;
            }
        }
        self.collect_notifications(document, None);
        Ok(saved)
    }
    /// Failed destination writes preserve the old identity, history, conflict,
    /// view state and dirty flag. Successful writes then commit the new binding.
    pub fn save_as(&mut self, document: DocumentId, destination: &Path) -> io::Result<bool> {
        self.ensure_running()?;
        if self.remote.is_some() {
            return self.queue_remote_save(document, Some(path_string(destination)?.to_owned()));
        }
        let target = self.check_rebind_path(document, destination)?;
        let previous = self.entry(document)?.editor.state.path.clone();
        if previous == path_string(&target)? {
            return self.save(document);
        }
        let entry = self.entry_mut(document)?;
        let old_dirty = entry.editor.state.dirty;
        let old_conflict = entry.editor.disk_conflict.take();
        let old_lsp = entry.editor.lsp_client();
        entry.editor.bind_lsp_client(None);
        entry.editor.state.path = path_string(&target)?.to_owned();
        entry.editor.state.dirty = true;
        let result = entry.editor.save();
        entry.editor.state.path = previous;
        entry.editor.bind_lsp_client(old_lsp);
        entry.editor.disk_conflict = old_conflict;
        match result {
            Ok(saved) => {
                self.entry_mut(document)?.editor.disk_conflict = None;
                self.rebind_path(document, &target)?;
                let entry = self.entry_mut(document)?;
                entry.removed = false;
                entry.allow_recreate = false;
                if let Some(client) = entry.editor.lsp_client() {
                    let state = &entry.editor.state;
                    let result = client.borrow_mut().did_save(&state.path, || {
                        String::from_utf8(state.join())
                            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
                    });
                    if let Err(error) = result {
                        self.record_error(Some(document), "lsp", &error);
                    }
                }
                self.collect_notifications(document, None);
                Ok(saved)
            }
            Err(error) => {
                let entry = self.entry_mut(document)?;
                entry.editor.state.dirty = old_dirty;
                // The attempted save cancelled its clock. Keep the original
                // dirty document eligible for its configured autosave.
                entry.editor.save_service.on_did_edit(&entry.editor.state);
                Err(error)
            }
        }
    }
    pub fn check_rebind_path(
        &self,
        document: DocumentId,
        destination: &Path,
    ) -> io::Result<PathBuf> {
        self.entry(document)?;
        let target = if self.remote.is_some() {
            destination.to_owned()
        } else {
            canonical_binding(destination)?
        };
        if self.paths.get(&target).is_some_and(|id| *id != document) {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "Destination is open as another document",
            ));
        }
        path_string(&target)?;
        Ok(target)
    }
    pub fn rebind_path(&mut self, document: DocumentId, destination: &Path) -> io::Result<()> {
        self.ensure_running()?;
        if self.save_pending(document) {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Wait for the remote save before renaming",
            ));
        }
        let target = self.check_rebind_path(document, destination)?;
        let target_string = path_string(&target)?.to_owned();
        let previous = self.entry(document)?.editor.state.path.clone();
        if target_string == previous {
            return Ok(());
        }
        let old_key = self.entry(document)?.editor.history_key().to_owned();
        self.history
            .borrow_mut()
            .rekey_file(&old_key, &target_string)?;
        if let Some(pool) = &mut self.lsp
            && let Err(error) = pool.unregister_document(document)
        {
            self.record_error(Some(document), "lsp", &error);
        }
        let git_enabled = self.options.git;
        let remote = self.remote.is_some();
        let entry = self.entry_mut(document)?;
        entry.editor.bind_lsp_client(None);
        entry.editor.set_history_key(None);
        entry.editor.rebind_document_path(&target_string);
        if !remote {
            entry
                .editor
                .git
                .borrow_mut()
                .on_document_opened(&entry.editor.state, git_enabled);
        }
        entry.editor.save_service.cancel_pending();
        entry.editor.save_service.on_did_edit(&entry.editor.state);
        entry.highlight_edits.clear();
        entry.removed = false;
        entry.allow_recreate = false;
        let watch = if remote {
            Ok(())
        } else if entry.editor.state.dirty {
            entry.monitor.watch(&target)
        } else {
            monitor_saved_bytes(entry)
        };
        if let Err(error) = watch {
            let message = error.to_string();
            entry.editor.disk_conflict = Some(message.clone());
            self.errors.push(ServiceError {
                document: Some(document),
                service: "monitor",
                message: message.clone(),
            });
            self.queued
                .push(SessionEvent::Conflict { document, message });
        }
        self.paths.retain(|_, id| *id != document);
        self.paths.insert(target, document);
        self.forget_remote_document(document, false);
        self.attach_lsp(document);
        if remote {
            self.queue_remote_git(document);
        }
        self.queued.push(SessionEvent::PathChanged {
            document,
            previous,
            path: target_string,
        });
        Ok(())
    }
    /// After successful trash/delete, cancel writes before the next service tick.
    pub fn invalidate_removed_path(&mut self, document: DocumentId) -> io::Result<()> {
        let entry = self.entry_mut(document)?;
        entry.removed = true;
        entry.allow_recreate = false;
        entry.editor.save_service.cancel_pending();
        entry.monitor.reset();
        entry.editor.disk_conflict = Some("File was removed from disk.".into());
        self.queued.push(SessionEvent::Conflict {
            document,
            message: "File was removed from disk.".into(),
        });
        Ok(())
    }
    pub fn keep_buffer(&mut self, document: DocumentId) -> io::Result<()> {
        if self.remote.is_some() {
            return self.queue_remote_keep(document);
        }
        let entry = self.entry_mut(document)?;
        if !entry.editor.state.path.is_empty() {
            entry
                .monitor
                .accept_current(Path::new(&entry.editor.state.path))?;
        }
        entry.editor.disk_conflict = None;
        entry.allow_recreate = entry.removed;
        entry.removed = false;
        entry.editor.save_service.on_did_edit(&entry.editor.state);
        Ok(())
    }
    pub fn reload_from_disk(&mut self, document: DocumentId) -> io::Result<()> {
        if self.remote.is_some() {
            return self.queue_remote_reload(document);
        }
        let path = self.entry(document)?.editor.state.path.clone();
        let raw = read_file_raw(Path::new(&path))?;
        let monitoring = self.options.monitoring;
        let git = self.options.git;
        let entry = self.entry_mut(document)?;
        if monitoring && !raw.truncated {
            entry.monitor.watch_bytes(Path::new(&path), &raw.raw)?;
        } else {
            entry.monitor.reset();
        }
        entry.editor.set_content(&raw.raw);
        entry.editor.state.path = path;
        entry.highlight_edits.clear();
        entry.removed = false;
        entry.allow_recreate = false;
        let key = entry.editor.history_key().to_owned();
        entry.editor.project_undo().forget_file(&key);
        for view in entry.views.values_mut() {
            view.state.clamp_all(&entry.editor.state);
        }
        entry
            .editor
            .git
            .borrow_mut()
            .on_document_opened(&entry.editor.state, git);
        let generation = entry.editor.document_generation();
        if let Some(pool) = &mut self.lsp {
            let _ = pool.unregister_document(document);
        }
        self.attach_lsp(document);
        self.queued.push(SessionEvent::Reloaded {
            document,
            generation,
        });
        Ok(())
    }
    /// A content replacement invalidates position-based undo and pending jobs;
    /// every view keeps its own scroll and clamps its own carets.
    pub fn replace_content(&mut self, document: DocumentId, bytes: &[u8]) -> io::Result<()> {
        let entry = self.entry_mut(document)?;
        entry.editor.set_content(bytes);
        entry.editor.state.dirty = true;
        entry.highlight_edits.clear();
        entry.editor.save_service.on_did_edit(&entry.editor.state);
        let key = entry.editor.history_key().to_owned();
        entry.editor.project_undo().forget_file(&key);
        for view in entry.views.values_mut() {
            view.state.clamp_all(&entry.editor.state);
        }
        let generation = entry.editor.document_generation();
        if let Some(pool) = &mut self.lsp {
            let _ = pool.unregister_document(document);
        }
        self.attach_lsp(document);
        self.queued.push(SessionEvent::Reloaded {
            document,
            generation,
        });
        Ok(())
    }
    pub fn tick(&mut self) -> TickReport {
        self.prune_views();
        self.poll_remote();
        let mut report = TickReport::default();
        if self.stopped {
            return self.drain_report(report);
        }
        if let Some(pool) = &mut self.lsp {
            report.lsp_events = pool.poll();
        }
        self.refresh_lsp_bindings();
        let monitor = self.options.monitoring
            && self
                .last_monitor
                .is_none_or(|last| last.elapsed() >= Duration::from_millis(500));
        if monitor {
            self.last_monitor = Some(Instant::now());
        }
        for id in self.document_ids() {
            if monitor
                && self.remote.is_none()
                && let Err(error) = self.poll_monitor(id)
            {
                self.record_error(Some(id), "monitor", &error);
            }
            let entry = self.documents.get_mut(&id).unwrap();
            entry
                .editor
                .ops
                .install_highlight_edits(std::mem::take(&mut entry.highlight_edits));
            entry.editor.poll_document_services();
            entry.editor.ops.clear_pending();
            if self.remote.is_some() {
                self.tick_remote_document(id, monitor);
            } else if self.options.autosave.is_some() && !self.documents[&id].autosave_paused {
                let entry = self.documents.get_mut(&id).unwrap();
                let result = if entry.editor.disk_conflict.is_some() {
                    entry.editor.save_service.cancel_pending();
                    Ok(false)
                } else if entry.editor.state.dirty && !entry.editor.state.path.is_empty() {
                    guard_write(entry).and_then(|()| entry.editor.poll_save())
                } else {
                    Ok(false)
                };
                match result {
                    Ok(true) => {
                        let entry = self.documents.get_mut(&id).unwrap();
                        entry.removed = false;
                        entry.allow_recreate = false;
                        if self.options.monitoring
                            && let Err(error) = monitor_saved_bytes(entry)
                        {
                            self.record_error(Some(id), "monitor", &error);
                        }
                    }
                    Ok(false) => {}
                    Err(error) => self.record_error(Some(id), "autosave", &error),
                }
            }
            self.collect_notifications(id, None);
        }
        if self.options.persistent_history
            && self
                .last_history
                .is_none_or(|last| last.elapsed() >= Duration::from_secs(3))
            && let Some(root) = &self.options.project_root
        {
            let result = self.history.borrow_mut().flush_to(root);
            match result {
                Ok(()) => self.last_history = Some(Instant::now()),
                Err(error) => self.record_error(None, "history", &error),
            }
        }
        self.drain_report(report)
    }
    pub fn take_events(&mut self) -> Vec<SessionEvent> {
        std::mem::take(&mut self.queued)
    }
    pub fn shutdown(&mut self, policy: ClosePolicy) -> io::Result<TickReport> {
        if self.stopped {
            return Ok(self.drain_report(TickReport::default()));
        }
        if policy == ClosePolicy::Cancel {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Shutdown cancelled",
            ));
        }
        if policy == ClosePolicy::Save {
            if self
                .document_ids()
                .into_iter()
                .any(|id| self.save_pending(id))
            {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "Remote saves pending; poll before shutdown",
                ));
            }
            let mut first_error = None;
            for id in self.document_ids() {
                if self.documents[&id].editor.state.dirty {
                    let result = if self.documents[&id].editor.state.path.is_empty() {
                        Err(io::Error::new(
                            io::ErrorKind::InvalidInput,
                            "Save As is required before shutdown",
                        ))
                    } else {
                        self.save(id).and_then(|_| {
                            if self.documents[&id].editor.state.dirty {
                                Err(io::Error::new(
                                    io::ErrorKind::WouldBlock,
                                    "Remote save is pending; poll before shutdown",
                                ))
                            } else {
                                Ok(())
                            }
                        })
                    };
                    if let Err(error) = result {
                        self.record_error(Some(id), "save", &error);
                        first_error.get_or_insert(error);
                    }
                }
            }
            if let Some(error) = first_error {
                return Err(error);
            }
        }
        if self.options.persistent_history
            && let Some(root) = &self.options.project_root
        {
            self.history.borrow_mut().flush_to(root)?;
        }
        for id in self.document_ids() {
            self.close_document(id, ClosePolicy::Discard)?;
        }
        if let Some(pool) = &mut self.lsp {
            pool.shutdown();
        }
        if let Some(remote) = &self.remote {
            remote.client.disconnect();
        }
        self.stopped = true;
        Ok(self.drain_report(TickReport::default()))
    }
    pub fn configure(&mut self, mut options: SessionOptions) -> io::Result<()> {
        self.ensure_running()?;
        if self.remote.is_some() {
            return self.configure_remote(options);
        }
        if let Some(root) = &options.project_root {
            options.project_root = Some(canonical_root(root)?);
        }
        if (options.persistent_history || options.lsp_config.is_some())
            && options.project_root.is_none()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Configured services require an explicit project root",
            ));
        }
        let root_changed = options.project_root != self.options.project_root;
        let highlight_changed = options.highlighting != self.options.highlighting;
        let monitoring_changed = options.monitoring != self.options.monitoring;
        let start_autosave = options.autosave.is_some() && self.options.autosave.is_none();
        let start_history = options.persistent_history && !self.options.persistent_history;
        let lsp_changed = root_changed || options.lsp_config != self.options.lsp_config;
        if root_changed
            && self.options.persistent_history
            && let Some(root) = &self.options.project_root
        {
            self.history.borrow_mut().flush_to(root)?;
        }
        if lsp_changed {
            for entry in self.documents.values_mut() {
                entry.editor.bind_lsp_client(None);
            }
            if let Some(pool) = &mut self.lsp {
                pool.shutdown();
            }
            self.lsp = None;
        }
        self.options = options;
        if monitoring_changed {
            self.last_monitor = None;
        }
        if self.options.persistent_history && (root_changed || start_history) {
            self.history
                .borrow_mut()
                .merge_project(path_string(self.options.project_root.as_ref().unwrap())?);
        }
        self.initialize_lsp();
        for id in self.document_ids() {
            let entry = self.documents.get_mut(&id).unwrap();
            entry
                .editor
                .highlight
                .set_enabled(self.options.highlighting);
            if highlight_changed {
                entry.editor.refresh_highlighting();
                entry.highlight_edits.clear();
            }
            entry.editor.set_git_changed_lines(self.options.git);
            if let Some(idle) = self.options.autosave {
                entry
                    .editor
                    .save_service
                    .set_autosave_idle_ms(idle.as_millis().min(i32::MAX as u128) as i32);
                if start_autosave {
                    entry.editor.save_service.on_did_edit(&entry.editor.state);
                }
            } else {
                entry.editor.save_service.cancel_pending();
            }
            if root_changed || (!entry.git_initialized && self.options.git) {
                let root = self
                    .options
                    .project_root
                    .as_ref()
                    .and_then(|p| p.to_str())
                    .unwrap_or("");
                entry
                    .editor
                    .git
                    .borrow_mut()
                    .init(&entry.editor.state, root, self.options.git);
                entry.git_initialized = true;
            }
            if lsp_changed {
                self.attach_lsp(id);
            }
        }
        Ok(())
    }
    pub fn lsp(&self) -> Option<&WorkspaceLsp> {
        self.lsp.as_ref()
    }
    pub fn lsp_mut(&mut self) -> Option<&mut WorkspaceLsp> {
        self.lsp.as_mut()
    }
    pub fn reload_lsp_config(&mut self) -> io::Result<()> {
        let result = self
            .lsp
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "LSP is not enabled"))?
            .reload_config();
        self.refresh_lsp_bindings();
        result
    }
    pub fn retry_language(&mut self, language: &str) -> io::Result<bool> {
        self.lsp
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "LSP is not enabled"))?
            .retry_language(language)
    }
    pub fn accepts_lsp_origin(&self, origin: &bed_lsp::workspace_lsp::LspRequestOrigin) -> bool {
        self.document_for_view(origin.view_id) == Some(origin.document_id)
            && self
                .documents
                .get(&origin.document_id)
                .is_some_and(|entry| {
                    entry.editor.document_generation() == origin.document_generation
                        && entry.editor.state.version == origin.version
                })
            && self
                .lsp
                .as_ref()
                .is_some_and(|pool| pool.origin_is_current(origin))
    }
    fn refresh_lsp_bindings(&mut self) {
        let Some(pool) = &self.lsp else {
            return;
        };
        for (&id, entry) in &mut self.documents {
            let desired = pool.client_for_document(id);
            let current = entry.editor.lsp_client();
            let unchanged = match (&current, &desired) {
                (Some(a), Some(b)) => Rc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            };
            if !unchanged {
                entry.editor.bind_lsp_client(desired);
            }
        }
    }
    fn initialize_lsp(&mut self) {
        if self.lsp.is_none()
            && let (Some(config), Some(root)) =
                (&self.options.lsp_config, &self.options.project_root)
        {
            self.lsp = Some(if let Some(remote) = &self.remote {
                WorkspaceLsp::new_remote(
                    self.workspace,
                    config.clone(),
                    root.clone(),
                    remote.target.clone(),
                )
            } else {
                WorkspaceLsp::new(self.workspace, config.clone(), root.clone())
            });
        }
    }
    fn attach_lsp(&mut self, document: DocumentId) {
        let Some(pool) = &mut self.lsp else {
            return;
        };
        let entry = self.documents.get_mut(&document).unwrap();
        if entry.editor.state.path.is_empty() {
            return;
        }
        let state = &entry.editor.state;
        let result = pool.register_document(
            document,
            &state.path,
            &state.join(),
            state.version,
            &state.language_id,
        );
        entry
            .editor
            .bind_lsp_client(pool.client_for_document(document));
        if let Err(error) = result {
            self.record_error(Some(document), "lsp", &error);
        }
    }
    fn update_lsp_snapshot(&mut self, document: DocumentId) {
        let Some(pool) = &mut self.lsp else {
            return;
        };
        let state = &self.documents[&document].editor.state;
        if !state.path.is_empty() {
            let result = pool.update_document_snapshot(
                document,
                &state.path,
                &state.join(),
                state.version,
                &state.language_id,
            );
            if let Err(error) = result {
                self.record_error(Some(document), "lsp", &error);
            }
            self.refresh_lsp_bindings();
        }
    }
    fn collect_notifications(&mut self, id: DocumentId, view: Option<ViewId>) {
        let entry = self.documents.get(&id).unwrap();
        for message in entry.editor.take_service_errors() {
            self.errors.push(ServiceError {
                document: Some(id),
                service: "lsp",
                message,
            });
        }
        for notification in std::mem::take(&mut *entry.notifications.borrow_mut()) {
            self.queued.push(match notification {
                Notification::Edit(edit) => SessionEvent::Edited {
                    document: id,
                    view,
                    generation: entry.editor.document_generation(),
                    edit,
                },
                Notification::Save(save) => SessionEvent::Saved { document: id, save },
            });
        }
    }
    fn poll_monitor(&mut self, document: DocumentId) -> io::Result<()> {
        let entry = self.entry_mut(document)?;
        if entry.editor.state.path.is_empty()
            || entry.removed
            || entry.editor.state.byte_size() > bed_files::file_monitor::MAX_FILE_SIZE as usize
        {
            return Ok(());
        }
        let path = PathBuf::from(&entry.editor.state.path);
        let change = match entry.monitor.poll(&path) {
            Ok(change) => change,
            Err(error) => {
                entry.editor.disk_conflict = Some(error.to_string());
                entry.editor.save_service.cancel_pending();
                return Err(error);
            }
        };
        let Some(change) = change else {
            return Ok(());
        };
        if change.kind == FileChangeKind::Removed {
            return self.invalidate_removed_path(document);
        }
        let mut bytes = entry.editor.state.join();
        if entry.editor.state.utf8_bom {
            bytes.splice(0..0, [0xef, 0xbb, 0xbf]);
        }
        if change.raw.as_deref() == Some(&bytes) {
            return Ok(());
        }
        if entry.editor.state.dirty {
            let message = "File changed on disk.".to_owned();
            entry.editor.disk_conflict = Some(message.clone());
            entry.editor.save_service.cancel_pending();
            self.queued
                .push(SessionEvent::Conflict { document, message });
            Ok(())
        } else {
            match self.reload_from_disk(document) {
                Ok(()) => Ok(()),
                Err(error) => {
                    let entry = self.entry_mut(document)?;
                    entry.editor.disk_conflict = Some(error.to_string());
                    entry.editor.save_service.cancel_pending();
                    Err(error)
                }
            }
        }
    }
    fn prune_views(&mut self) {
        let dead: Vec<_> = self
            .views
            .keys()
            .copied()
            .filter(|view| self.document_for_view(*view).is_none())
            .collect();
        for view in dead {
            self.detach_view(view);
        }
    }
    fn record_error(
        &mut self,
        document: Option<DocumentId>,
        service: &'static str,
        error: &io::Error,
    ) {
        self.errors.push(ServiceError {
            document,
            service,
            message: error.to_string(),
        });
    }
    fn drain_report(&mut self, mut report: TickReport) -> TickReport {
        report.events = std::mem::take(&mut self.queued);
        report.errors = std::mem::take(&mut self.errors);
        report
    }
    fn entry(&self, id: DocumentId) -> io::Result<&DocumentEntry> {
        self.documents.get(&id).ok_or_else(missing_document)
    }
    fn entry_mut(&mut self, id: DocumentId) -> io::Result<&mut DocumentEntry> {
        self.documents.get_mut(&id).ok_or_else(missing_document)
    }
    fn ensure_running(&self) -> io::Result<()> {
        if self.stopped {
            Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "EditorSession is shut down",
            ))
        } else {
            Ok(())
        }
    }
}
impl Drop for EditorSession {
    fn drop(&mut self) {
        if let Some(remote) = &self.remote {
            remote.client.disconnect();
        }
        for entry in self.documents.values_mut() {
            entry.editor.save_service.cancel_pending();
            entry.editor.bind_lsp_client(None);
            entry.editor.highlight.cancel_highlighting();
        }
        if let Some(pool) = &mut self.lsp {
            pool.shutdown();
        }
    }
}
fn transform_offset(offset: &mut usize, edit: &PendingEdit) {
    let start = edit.start_byte as usize;
    if edit.op.kind == OpKind::Insert {
        if *offset >= start {
            *offset = offset.saturating_add(edit.op.text.len());
        }
    } else {
        let end = start.saturating_add(edit.removed_bytes.len());
        if *offset >= end {
            *offset = offset.saturating_sub(end - start);
        } else if *offset >= start {
            *offset = start;
        }
    }
}
fn guard_write(entry: &mut DocumentEntry) -> io::Result<()> {
    if !entry.editor.state.dirty || entry.editor.state.path.is_empty() {
        return Ok(());
    }
    if entry.removed
        || (!entry.allow_recreate && !Path::new(&entry.editor.state.path).try_exists()?)
    {
        entry.removed = true;
        entry.editor.disk_conflict = Some("File was removed from disk.".into());
        entry.editor.save_service.cancel_pending();
        return Err(io::Error::other(
            "File was removed; use Save As or explicitly keep the buffer before saving",
        ));
    }
    if entry.editor.disk_conflict.is_some() {
        entry.editor.save_service.cancel_pending();
    }
    Ok(())
}
fn monitor_saved_bytes(entry: &mut DocumentEntry) -> io::Result<()> {
    if entry.editor.state.byte_size() > bed_files::file_monitor::MAX_FILE_SIZE as usize {
        entry.monitor.reset();
        return Ok(());
    }
    let mut raw = entry.editor.state.join();
    if entry.editor.state.utf8_bom {
        raw.splice(0..0, [0xef, 0xbb, 0xbf]);
    }
    let result = entry
        .monitor
        .watch_bytes(Path::new(&entry.editor.state.path), &raw);
    if let Err(error) = &result {
        entry.editor.disk_conflict = Some(error.to_string());
        entry.editor.save_service.cancel_pending();
    }
    result
}
fn canonical_root(path: &Path) -> io::Result<PathBuf> {
    let root = std::fs::canonicalize(path)?;
    if !root.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Project root is not a directory",
        ));
    }
    path_string(&root)?;
    Ok(root)
}
pub(crate) fn canonical_binding(path: &Path) -> io::Result<PathBuf> {
    match std::fs::canonicalize(path) {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let name = path.file_name().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "File path has no name")
            })?;
            let parent = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new("."));
            Ok(std::fs::canonicalize(parent)?.join(name))
        }
        Err(error) => Err(error),
    }
}
fn path_string(path: &Path) -> io::Result<&str> {
    path.to_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Path is not UTF-8"))
}
fn missing_document() -> io::Error {
    io::Error::new(
        io::ErrorKind::NotFound,
        "Document is not registered in this session",
    )
}
fn missing_view() -> io::Error {
    io::Error::new(
        io::ErrorKind::NotFound,
        "View is not attached to this session",
    )
}
