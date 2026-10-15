//! Shared documents and services for the standalone shell and host-owned views.
//! ned's editing algorithms remain in state/operations/commands. This ownership
//! extension is Bed-specific; attribution: LICENSE and NOTICE.
use crate::document_runtime::{DocumentRuntime, SharedProjectUndo};
use bedtime_editing::{
    document_state::DocumentState,
    editing_events::{DidEdit, DidSave},
    editing_operations::{EditingOperations, OpKind, PendingEdit, TextOp},
    project_undo::ProjectUndo,
    text_commands::TextCommands,
    text_view_state::TextViewState,
};
use bedtime_files::{
    file_monitor::{FileChangeKind, FileMonitor},
    files::{read_file_bytes, read_file_raw},
};
#[cfg(feature = "ide-services")]
use bedtime_lsp::workspace_lsp::{LspRequestOrigin, WorkspaceLsp, WorkspaceLspEvent};
use std::{
    cell::RefCell,
    collections::{BTreeMap, VecDeque},
    io,
    path::{Path, PathBuf},
    rc::{Rc, Weak},
    time::{Duration, Instant},
};

#[cfg(feature = "remote")]
#[path = "remote_session.rs"]
mod remote_session;
#[cfg(feature = "remote")]
use remote_session::RemoteSession;
#[cfg(feature = "ide-services")]
#[path = "workspace_edits.rs"]
mod workspace_edits;
#[cfg(feature = "ide-services")]
use workspace_edits::{
    DeferredWorkspaceEdit, PendingWorkspaceEdit, PendingWorkspaceHistory, WorkspaceEditReceipt,
};

pub use bedtime_editing::document_state::DocumentKind;
pub use bedtime_editing::identity::{DocumentId, SessionId, ViewId};

#[cfg(feature = "ide-services")]
type OpenOrigin = LspRequestOrigin;
#[cfg(feature = "ide-services")]
type CapturedDocumentRevisions = BTreeMap<DocumentId, (u64, u64)>;
#[cfg(not(feature = "ide-services"))]
type OpenOrigin = ();

/// Ranges refer to the unchanged document at `expected_revision`. Transactions
/// must be non-overlapping; replacement bytes are normalized only for text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ByteEdit {
    pub range: std::ops::Range<usize>,
    pub bytes: Vec<u8>,
}

#[derive(Default)]
struct ByteHistory {
    undo: Vec<ByteTransaction>,
    redo: Vec<ByteTransaction>,
}

#[derive(Clone, Copy)]
struct ByteSplice {
    revision: u64,
    start: usize,
    removed: usize,
    inserted: usize,
}

struct ByteTransaction {
    undo: Vec<ByteEdit>,
    redo: Vec<ByteEdit>,
}

/// How the configured LSP file relates to the bundled language catalog.
#[cfg(feature = "ide-services")]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LspConfigMode {
    /// The supplied file is a complete configuration, for embedded hosts.
    #[default]
    Snapshot,
    /// Merge bundled defaults, the supplied user overrides, and project overrides.
    Layered,
}

/// No service, settings seeding, GUI, clipboard or global directory is touched
/// by defaults. Disk/process services require explicit host configuration.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionOptions {
    pub project_root: Option<PathBuf>,
    pub autosave: Option<Duration>,
    pub monitoring: bool,
    #[cfg(feature = "ide-services")]
    pub git: bool,
    #[cfg(feature = "ide-services")]
    pub highlighting: bool,
    /// Exact local destination for text history; remote sessions keep history in memory.
    pub history_path: Option<PathBuf>,
    #[cfg(feature = "ide-services")]
    pub lsp_config: Option<PathBuf>,
    /// Optional project-relative language configuration for layered mode.
    /// No project file is read unless its location is supplied explicitly.
    #[cfg(feature = "ide-services")]
    pub lsp_project_config: Option<PathBuf>,
    #[cfg(feature = "ide-services")]
    pub lsp_config_mode: LspConfigMode,
    /// The host forwards project-wide filesystem observations to this session.
    #[cfg(feature = "ide-services")]
    pub lsp_file_observations: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ClosePolicy {
    Save,
    Discard,
    #[default]
    Cancel,
}

/// The session operation that produced an error. Messages remain presentation text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceKind {
    Save,
    Autosave,
    Monitor,
    History,
    Lsp,
    Remote,
    Undo,
}

impl ServiceKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Save => "save",
            Self::Autosave => "autosave",
            Self::Monitor => "monitor",
            Self::History => "history",
            Self::Lsp => "lsp",
            Self::Remote => "remote",
            Self::Undo => "undo",
        }
    }
}

impl std::fmt::Display for ServiceKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceError {
    pub document: Option<DocumentId>,
    pub service: ServiceKind,
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
    /// The host must finish panel drafts before closing clean documents or
    /// detaching dirty ones. Writes are blocked until that decision is made.
    Removed {
        document: DocumentId,
        path: String,
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
    #[cfg(feature = "ide-services")]
    pub lsp_events: Vec<WorkspaceLspEvent>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocumentSnapshot {
    pub kind: DocumentKind,
    pub id: DocumentId,
    pub path: String,
    /// Display name retained after deletion; an empty `path` requires Save As.
    pub original_path: Option<String>,
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
    state: TextViewState,
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
    byte_history: ByteHistory,
    byte_splices: VecDeque<ByteSplice>,
    runtime: DocumentRuntime,
    views: BTreeMap<ViewId, ViewEntry>,
    monitor: FileMonitor,
    notifications: Rc<RefCell<Vec<Notification>>>,
    highlight_edits: Vec<PendingEdit>,
    removed: bool,
    removal_pending_since: Option<Instant>,
    original_path: Option<String>,
    allow_recreate: bool,
    autosave_paused: bool,
    #[cfg(feature = "ide-services")]
    git_initialized: bool,
    workspace_edit_locked: bool,
}

/// Main-thread, GUI-independent document registry. Public mutation uses commands
/// rather than mutable DocumentRuntime/state references, preserving service/view updates.
pub struct DocumentSession {
    #[cfg(feature = "remote")]
    remote: Option<RemoteSession>,
    session_id: SessionId,
    options: SessionOptions,
    services: bedtime_files::FileServices,
    documents: BTreeMap<DocumentId, DocumentEntry>,
    paths: BTreeMap<PathBuf, DocumentId>,
    views: BTreeMap<ViewId, DocumentId>,
    history: SharedProjectUndo,
    #[cfg(feature = "ide-services")]
    lsp: Option<WorkspaceLsp>,
    #[cfg(feature = "ide-services")]
    project_diagnostics: crate::project_diagnostics::ProjectDiagnostics,
    #[cfg(feature = "ide-services")]
    document_diagnostics: bedtime_lsp::diagnostics::LspDiagnostics,
    #[cfg(feature = "ide-services")]
    git_statuses: crate::git::status::SharedStatuses,
    queued: Vec<SessionEvent>,
    errors: Vec<ServiceError>,
    last_monitor: Option<Instant>,
    last_history: Option<Instant>,
    stopped: bool,
    #[cfg(feature = "ide-services")]
    theme: bedtime_highlight::tree_sitter::ThemeColors,
    #[cfg(feature = "ide-services")]
    pending_workspace_edits: Vec<PendingWorkspaceEdit>,
    #[cfg(feature = "ide-services")]
    deferred_workspace_edits: Vec<DeferredWorkspaceEdit>,
    #[cfg(feature = "ide-services")]
    active_lsp_commands:
        BTreeMap<u64, Option<bedtime_editing::project_undo::WorkspaceHistoryGroup>>,
    #[cfg(feature = "ide-services")]
    workspace_edit_receipts: Vec<WorkspaceEditReceipt>,
    #[cfg(feature = "ide-services")]
    pending_workspace_history: Option<PendingWorkspaceHistory>,
    #[cfg(feature = "ide-services")]
    workspace_request_revisions: Vec<(LspRequestOrigin, CapturedDocumentRevisions)>,
    #[cfg(feature = "ide-services")]
    active_lsp_command_revisions: BTreeMap<u64, CapturedDocumentRevisions>,
}

impl Default for DocumentSession {
    fn default() -> Self {
        Self {
            #[cfg(feature = "remote")]
            remote: None,
            session_id: SessionId::next(),
            options: SessionOptions::default(),
            services: bedtime_files::FileServices::local(),
            documents: BTreeMap::new(),
            paths: BTreeMap::new(),
            views: BTreeMap::new(),
            history: Rc::new(RefCell::new(ProjectUndo::default())),
            #[cfg(feature = "ide-services")]
            lsp: None,
            #[cfg(feature = "ide-services")]
            project_diagnostics: Default::default(),
            #[cfg(feature = "ide-services")]
            document_diagnostics: Default::default(),
            #[cfg(feature = "ide-services")]
            git_statuses: Default::default(),
            queued: Vec::new(),
            errors: Vec::new(),
            last_monitor: None,
            last_history: None,
            stopped: false,
            #[cfg(feature = "ide-services")]
            theme: Default::default(),
            #[cfg(feature = "ide-services")]
            pending_workspace_edits: Vec::new(),
            #[cfg(feature = "ide-services")]
            deferred_workspace_edits: Vec::new(),
            #[cfg(feature = "ide-services")]
            active_lsp_commands: BTreeMap::new(),
            #[cfg(feature = "ide-services")]
            workspace_edit_receipts: Vec::new(),
            #[cfg(feature = "ide-services")]
            pending_workspace_history: None,
            #[cfg(feature = "ide-services")]
            workspace_request_revisions: Vec::new(),
            #[cfg(feature = "ide-services")]
            active_lsp_command_revisions: BTreeMap::new(),
        }
    }
}

impl DocumentSession {
    /// Workspace and application adapters choose where this opaque history lives.
    pub fn history_snapshot(&mut self) -> io::Result<serde_json::Value> {
        self.ensure_running()?;
        self.history.borrow_mut().snapshot()
    }

    pub fn restore_history_snapshot(&mut self, snapshot: &serde_json::Value) -> io::Result<()> {
        self.ensure_running()?;
        Self::validate_history_snapshot(snapshot)?;
        self.history.borrow_mut().load_snapshot(snapshot)
    }

    /// Validate stored history before retiring the current session.
    pub fn validate_history_snapshot(snapshot: &serde_json::Value) -> io::Result<()> {
        ProjectUndo::validate_snapshot(snapshot)
    }

    pub fn merge_history_snapshot(&mut self, snapshot: &serde_json::Value) -> io::Result<()> {
        self.ensure_running()?;
        Self::validate_history_snapshot(snapshot)?;
        self.history.borrow_mut().merge_snapshot(snapshot)
    }

    pub fn mark_history_persisted(&mut self) {
        self.history.borrow_mut().mark_persisted();
    }

    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_options(mut options: SessionOptions) -> io::Result<Self> {
        if let Some(root) = &options.project_root {
            options.project_root = Some(canonical_root(root)?);
        }
        #[cfg(feature = "ide-services")]
        if options.lsp_config.is_some() && options.project_root.is_none() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Configured services require an explicit project root",
            ));
        }
        let mut session = Self::default();
        session.options = options;
        if let Some(path) = &session.options.history_path {
            session.history.borrow_mut().load_from(path)?;
        }
        #[cfg(feature = "ide-services")]
        session.initialize_lsp();
        Ok(session)
    }

    /// Construct document access from a live directory, without saved-workspace policy.
    pub fn with_file_services(
        mut options: SessionOptions,
        context: &bedtime_files::FileContext,
        services: &bedtime_files::FileServices,
    ) -> io::Result<Self> {
        if &context.provider != services.provider_id() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "File context and services use different providers",
            ));
        }
        if context.availability != bedtime_files::FileAvailability::Available
            || !services.connected()
        {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "File provider is unavailable",
            ));
        }
        options.project_root = Some(context.directory.clone());
        if services.is_remote() {
            #[cfg(feature = "remote")]
            return Self::with_remote_options(
                options,
                services
                    .ssh_target()
                    .expect("SSH services retain their target")
                    .clone(),
                services
                    .remote_client()
                    .expect("SSH services retain their authenticated connection"),
                context.directory_string(),
            );
            #[cfg(not(feature = "remote"))]
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Enable remote document support for SSH file access",
            ));
        }
        let mut session = Self::with_options(options)?;
        session.services = services.clone();
        Ok(session)
    }

    /// Reconfigure services while retaining document identity. Replacing the
    /// provider or remote directory requires a newly prepared session.
    pub fn configure_file_services(
        &mut self,
        mut options: SessionOptions,
        context: &bedtime_files::FileContext,
        services: &bedtime_files::FileServices,
    ) -> io::Result<()> {
        if &context.provider != services.provider_id() || self.is_remote() != services.is_remote() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Prepare a new document session to replace its provider",
            ));
        }
        if context.availability != bedtime_files::FileAvailability::Available
            || !services.connected()
        {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "File provider is unavailable",
            ));
        }
        options.project_root = Some(context.directory.clone());
        if services.is_remote() {
            #[cfg(feature = "remote")]
            {
                if self.options.project_root.as_ref() != Some(&context.directory) {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "Prepare a new document session to replace its directory",
                    ));
                }
                if !self.services.same_provider(services) {
                    self.reconnect_remote_with_target(
                        services.remote_client().expect("SSH connection"),
                        services.ssh_target().expect("SSH target").clone(),
                    )?;
                }
            }
        }
        self.configure(options)?;
        self.services = services.clone();
        Ok(())
    }

    /// Identity of this document container; project attachment does not replace it.
    pub fn session_id(&self) -> SessionId {
        self.session_id
    }

    /// Whether a file-service request belongs to this document provider.
    /// Connection identity matters during reconnect as well as host identity.
    pub fn file_provider_matches(&self, services: &bedtime_files::FileServices) -> bool {
        services.connected() && self.services.same_provider(services)
    }

    /// Whether this container still accepts document work.
    pub fn is_running(&self) -> bool {
        !self.stopped
    }

    #[cfg(not(feature = "remote"))]
    pub fn is_remote(&self) -> bool {
        false
    }

    #[cfg(not(feature = "remote"))]
    pub fn save_pending(&self, _document: DocumentId) -> bool {
        false
    }

    #[cfg(not(feature = "remote"))]
    pub fn open_pending(&self, _path: &Path) -> bool {
        false
    }

    #[cfg(not(feature = "remote"))]
    pub fn cancel_open_file(&mut self, _path: &Path) {}

    #[cfg(not(feature = "remote"))]
    pub fn take_open_file_error(&mut self, _path: &Path) -> Option<io::Error> {
        None
    }

    #[cfg(not(feature = "remote"))]
    pub fn request_open_file(&mut self, path: &Path) -> io::Result<Option<DocumentId>> {
        self.open_file(path).map(Some)
    }

    #[cfg(not(feature = "remote"))]
    pub fn request_open_file_auto(&mut self, path: &Path) -> io::Result<Option<DocumentId>> {
        self.open_file_auto(path).map(Some)
    }

    pub fn options(&self) -> &SessionOptions {
        &self.options
    }

    /// Immutable canonical absolute paths; presentation never starts repository I/O.
    pub fn git_modified_paths(&self) -> Rc<std::collections::HashSet<String>> {
        #[cfg(feature = "ide-services")]
        return self.git_statuses.borrow_mut().snapshot();
        #[cfg(not(feature = "ide-services"))]
        Rc::default()
    }

    pub fn git_file_modified(&self, path: &str) -> bool {
        #[cfg(feature = "ide-services")]
        return self
            .git_modified_paths()
            .contains(&bedtime_editing::util::doc_path::normalize(path));
        #[cfg(not(feature = "ide-services"))]
        {
            let _ = path;
            false
        }
    }

    #[cfg(all(feature = "remote", not(feature = "ide-services")))]
    pub fn observe_project_file_changes(&mut self, _changes: &[bedtime_remote::FilesystemChange]) {}

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

    #[cfg(feature = "ide-services")]
    pub fn set_highlight_theme(&mut self, theme: bedtime_highlight::tree_sitter::ThemeColors) {
        if self.theme == theme {
            return;
        }
        self.theme = theme;
        for entry in self.documents.values_mut() {
            entry.runtime.highlight.set_theme_colors(self.theme.clone());
            entry.runtime.refresh_highlighting();
        }
    }

    #[cfg(feature = "ide-services")]
    pub fn set_git_enabled(&mut self, enabled: bool) -> io::Result<()> {
        let mut options = self.options.clone();
        options.git = enabled;
        self.configure(options)
    }

    /// Refresh repository comparisons without replacing text or resetting undo.
    #[cfg(feature = "ide-services")]
    pub fn invalidate_git_baselines(&mut self) {
        if let Some(remote) = &mut self.remote {
            remote.comparison_epoch = remote.comparison_epoch.wrapping_add(1);
        }
        for document in self.document_ids() {
            if self.is_snapshot_document(document) {
                continue;
            }
            if self.is_remote() {
                self.queue_remote_git(document);
            } else if let Some(entry) = self.documents.get_mut(&document) {
                entry
                    .runtime
                    .git
                    .borrow_mut()
                    .invalidate_baseline(&entry.runtime.state, self.options.git);
            }
        }
    }

    #[cfg(feature = "ide-services")]
    pub fn set_highlighting_enabled(&mut self, enabled: bool) -> io::Result<()> {
        let mut options = self.options.clone();
        options.highlighting = enabled;
        self.configure(options)
    }

    pub fn pause_autosave(&mut self, document: DocumentId, paused: bool) -> io::Result<()> {
        self.entry_mut(document)?.autosave_paused = paused;
        Ok(())
    }

    pub fn autosave_paused(&self, document: DocumentId) -> io::Result<bool> {
        Ok(self.entry(document)?.autosave_paused)
    }

    pub fn document_ids(&self) -> Vec<DocumentId> {
        self.documents.keys().copied().collect()
    }

    pub fn document_for_path(&self, path: &Path) -> Option<DocumentId> {
        #[cfg(feature = "remote")]
        if self.is_remote() {
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
        let state = &entry.runtime.state;
        Ok(DocumentSnapshot {
            kind: state.kind,
            id: document,
            path: state.path.clone(),
            original_path: entry.original_path.clone(),
            bytes: state.join(),
            version: state.version,
            generation: entry.runtime.document_generation(),
            dirty: state.dirty,
            language_id: state.language_id.clone(),
            utf8_bom: state.utf8_bom,
            line_ending: state.line_ending.clone(),
            disk_conflict: entry.runtime.disk_conflict.clone(),
        })
    }

    /// Cheap cache identity: document replacement generation and text edit revision.
    /// Unlike a full snapshot, this never copies the document's bytes.
    pub fn document_revision(&self, document: DocumentId) -> io::Result<(u64, u64)> {
        let runtime = &self.entry(document)?.runtime;
        Ok((runtime.document_generation(), runtime.ops.generation()))
    }

    pub fn document_snapshot(&self, document: DocumentId) -> io::Result<DocumentSnapshot> {
        self.snapshot(document)
    }

    pub fn with_document<R>(
        &self,
        document: DocumentId,
        f: impl FnOnce(&DocumentState) -> R,
    ) -> io::Result<R> {
        Ok(f(&self.entry(document)?.runtime.state))
    }

    pub fn view_snapshot(&self, view: ViewId) -> io::Result<TextViewState> {
        let id = self.document_for_view(view).ok_or_else(missing_view)?;
        Ok(self.documents[&id].views[&view].state.clone())
    }

    pub fn create_document(&mut self, bytes: &[u8]) -> io::Result<DocumentId> {
        self.create_document_with_kind(bytes, DocumentKind::Text)
    }

    pub fn create_document_with_kind(
        &mut self,
        bytes: &[u8],
        kind: DocumentKind,
    ) -> io::Result<DocumentId> {
        self.ensure_running()?;
        let id = DocumentId::next();
        let mut runtime = DocumentRuntime::new();
        if kind == DocumentKind::Text {
            runtime.bind_project_undo(Rc::clone(&self.history));
        }
        runtime.set_history_key(Some(format!("bedtime:untitled:{}", id.0)));
        runtime.set_content_with_kind(bytes, kind);
        self.install_entry(id, runtime, FileMonitor::new(), None);
        Ok(id)
    }

    pub fn document_kind(&self, document: DocumentId) -> io::Result<DocumentKind> {
        Ok(self.entry(document)?.runtime.state.kind)
    }

    /// A service-free immutable document for index/commit comparisons.
    pub fn create_snapshot_document(
        &mut self,
        bytes: &[u8],
        language_id: &str,
    ) -> io::Result<DocumentId> {
        self.ensure_running()?;
        let id = DocumentId::next();
        let mut runtime = DocumentRuntime::new();
        runtime.set_content(bytes);
        runtime.state.language_id = language_id.to_owned();
        runtime.state.read_only = true;
        runtime.state.mark_saved();
        self.install_entry(id, runtime, FileMonitor::new(), None);
        self.entry_mut(id)?.autosave_paused = true;
        Ok(id)
    }

    pub fn is_snapshot_document(&self, document: DocumentId) -> bool {
        self.documents
            .get(&document)
            .is_some_and(|entry| entry.runtime.state.read_only && !entry.workspace_edit_locked)
    }

    pub fn replace_snapshot_document(
        &mut self,
        document: DocumentId,
        bytes: &[u8],
    ) -> io::Result<()> {
        self.ensure_running()?;
        if !self.is_snapshot_document(document) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Expected a snapshot document",
            ));
        }
        let entry = self.entry_mut(document)?;
        entry.runtime.set_content(bytes);
        entry.runtime.state.mark_saved();
        entry.highlight_edits.clear();
        for view in entry.views.values_mut() {
            view.state.folds.set_ranges(Vec::new());
            view.state.clamp_all(&entry.runtime.state);
        }
        let generation = entry.runtime.document_generation();
        self.queued.push(SessionEvent::Reloaded {
            document,
            generation,
        });
        Ok(())
    }

    pub fn open_file(&mut self, path: &Path) -> io::Result<DocumentId> {
        self.open_file_with_kind(path, DocumentKind::Text)
    }

    pub fn open_file_with_kind(
        &mut self,
        path: &Path,
        kind: DocumentKind,
    ) -> io::Result<DocumentId> {
        self.open_file_in_context(path, kind, None)
    }

    /// Open a definition target using the requesting server's analysis context
    /// when the target is a new compatible file outside this workspace.
    #[cfg(feature = "ide-services")]
    pub fn open_file_from_lsp(
        &mut self,
        path: &Path,
        origin: LspRequestOrigin,
    ) -> io::Result<DocumentId> {
        if !self.accepts_lsp_origin(&origin) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Stale LSP request origin",
            ));
        }
        self.open_file_in_context(path, DocumentKind::Text, Some(origin))
    }

    fn open_file_in_context(
        &mut self,
        path: &Path,
        kind: DocumentKind,
        origin: Option<OpenOrigin>,
    ) -> io::Result<DocumentId> {
        self.ensure_running()?;
        #[cfg(feature = "remote")]
        if self.is_remote() {
            return self
                .request_open_mode(path, Some(kind), origin)?
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::WouldBlock,
                        "Remote open queued; poll session events for completion",
                    )
                });
        }
        let path = std::fs::canonicalize(path)?;
        if let Some(&id) = self.paths.get(&path) {
            if self.document_kind(id)? != kind {
                return Err(document_kind_conflict());
            }
            return Ok(id);
        }
        let raw = self.read_local_file(&path, kind)?;
        self.install_local_file(path, kind, raw.raw, origin)
    }

    /// Open using the host's default text/bytes classifier.
    pub fn open_file_auto(&mut self, path: &Path) -> io::Result<DocumentId> {
        self.ensure_running()?;
        #[cfg(feature = "remote")]
        if self.is_remote() {
            return self
                .request_open_file_auto(path)?
                .ok_or_else(|| io::Error::new(io::ErrorKind::WouldBlock, "Remote open queued"));
        }
        let path = std::fs::canonicalize(path)?;
        if let Some(&id) = self.paths.get(&path) {
            return Ok(id);
        }
        let raw = self.read_local_file(&path, DocumentKind::Bytes)?;
        self.install_local_file(
            path,
            bedtime_files::files::classify_bytes(&raw.raw),
            raw.raw,
            None,
        )
    }

    fn read_local_file(
        &self,
        path: &Path,
        kind: DocumentKind,
    ) -> io::Result<bedtime_files::files::ReadFile> {
        #[cfg(feature = "file-services")]
        {
            let metadata = std::fs::metadata(path)?;
            if metadata.len() > bedtime_files::files::MAX_FILE_SIZE as u64 {
                return Err(bedtime_files::files::file_too_large(path, metadata.len()));
            }
            if kind == DocumentKind::Text {
                // Preserve the bounded binary probe before a full service read.
                let mut prefix = Vec::new();
                use std::io::Read;
                std::fs::File::open(path)?
                    .take(1024)
                    .read_to_end(&mut prefix)?;
                bedtime_files::files::validate_text_bytes(&prefix)?;
            }
            let location = bedtime_files::FileLocation {
                provider: bedtime_files::FileProviderId::Local,
                path: path.to_path_buf(),
            };
            let (raw, _) = self
                .services
                .read(path.parent().unwrap_or(Path::new(".")), &location)?;
            Ok(bedtime_files::files::ReadFile { raw })
        }
        #[cfg(not(feature = "file-services"))]
        if kind == DocumentKind::Text {
            read_file_raw(path)
        } else {
            read_file_bytes(path)
        }
    }

    fn install_local_file(
        &mut self,
        path: PathBuf,
        kind: DocumentKind,
        bytes: Vec<u8>,
        origin: Option<OpenOrigin>,
    ) -> io::Result<DocumentId> {
        let mut monitor = FileMonitor::new();
        monitor.watch_bytes(&path, &bytes)?;
        let id = DocumentId::next();
        let mut runtime = DocumentRuntime::new();
        if kind == DocumentKind::Text {
            runtime.bind_project_undo(Rc::clone(&self.history));
        }
        #[cfg(feature = "ide-services")]
        runtime.set_git_changed_lines(self.options.git && kind == DocumentKind::Text);
        runtime
            .api()
            .open_document_with_kind(path_string(&path)?, &bytes, kind);
        self.paths.insert(path, id);
        self.install_entry(id, runtime, monitor, origin);
        Ok(id)
    }

    fn install_entry(
        &mut self,
        id: DocumentId,
        mut runtime: DocumentRuntime,
        monitor: FileMonitor,
        origin: Option<OpenOrigin>,
    ) {
        #[cfg(feature = "ide-services")]
        let text = runtime.state.kind == DocumentKind::Text;
        #[cfg(feature = "ide-services")]
        {
            runtime
                .git
                .borrow_mut()
                .bind_statuses(Rc::clone(&self.git_statuses));
            runtime.highlight.enabled = self.options.highlighting && text;
            runtime.set_git_changed_lines(self.options.git && text);
            runtime.highlight.set_theme_colors(self.theme.clone());
        }
        #[cfg(not(feature = "ide-services"))]
        let _ = origin;
        if let Some(idle) = self.options.autosave {
            runtime
                .save_service
                .set_autosave_idle_ms(idle.as_millis().min(i32::MAX as u128) as i32);
        }
        #[cfg(feature = "ide-services")]
        if self.options.git && text && !runtime.state.read_only && !self.is_remote() {
            let root = self
                .options
                .project_root
                .as_ref()
                .and_then(|p| p.to_str())
                .unwrap_or("");
            runtime.git.borrow_mut().init(&runtime.state, root, true);
        }
        let notifications = Rc::new(RefCell::new(Vec::new()));
        let edits = Rc::clone(&notifications);
        runtime.events.subscribe_did_edit(move |event| {
            edits.borrow_mut().push(Notification::Edit(event.clone()))
        });
        let saves = Rc::clone(&notifications);
        runtime.events.subscribe_did_save(move |event| {
            saves.borrow_mut().push(Notification::Save(event.clone()))
        });
        self.documents.insert(
            id,
            DocumentEntry {
                byte_history: ByteHistory::default(),
                byte_splices: VecDeque::new(),
                runtime,
                views: BTreeMap::new(),
                monitor,
                notifications,
                highlight_edits: Vec::new(),
                removed: false,
                removal_pending_since: None,
                original_path: None,
                allow_recreate: false,
                autosave_paused: false,
                #[cfg(feature = "ide-services")]
                git_initialized: self.options.git && text,
                workspace_edit_locked: false,
            },
        );
        #[cfg(feature = "ide-services")]
        self.attach_lsp(id, origin);
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

    /// Retain an existing managed view while its presentation owner changes.
    /// This never creates a view or revives a released owner.
    pub fn retain_managed_view(&self, view: ViewId) -> io::Result<Rc<()>> {
        let document = self.document_for_view(view).ok_or_else(missing_view)?;
        self.entry(document)?
            .views
            .get(&view)
            .and_then(|entry| entry.lifetime.as_ref())
            .and_then(Weak::upgrade)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Existing view has no live managed presentation",
                )
            })
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
                state: TextViewState::new(),
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
        f: impl FnOnce(&mut crate::text_view_context::TextViewContext<'_>) -> R,
    ) -> io::Result<R> {
        let document = self.document_for_view(view).ok_or_else(missing_view)?;
        if self.document_kind(document)? != DocumentKind::Text {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Text view contexts require a text document",
            ));
        }
        self.with_runtime_view(view, |runtime| f(&mut runtime.text_view_context()))
    }

    pub fn with_commands<R>(
        &mut self,
        view: ViewId,
        f: impl FnOnce(&mut TextCommands<'_>) -> R,
    ) -> io::Result<R> {
        let document = self.document_for_view(view).ok_or_else(missing_view)?;
        if self.document_kind(document)? != DocumentKind::Text {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Text commands require a text document; use byte transactions for binary edits",
            ));
        }
        self.with_runtime_view(view, |runtime| f(&mut runtime.commands()))
    }

    pub(crate) fn with_runtime_view<R>(
        &mut self,
        view: ViewId,
        f: impl FnOnce(&mut DocumentRuntime) -> R,
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
                                    .runtime
                                    .state
                                    .offset_from_row_col(s.head_row, s.head_column),
                                entry
                                    .runtime
                                    .state
                                    .offset_from_row_col(s.anchor_row, s.anchor_column),
                            )
                        })
                        .collect(),
                )
            })
            .collect();
        let generation = entry.runtime.document_generation();
        std::mem::swap(
            &mut entry.runtime.view,
            &mut entry.views.get_mut(&view).unwrap().state,
        );
        entry.runtime.set_command_owner(Some(view.0));
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&mut entry.runtime)));
        entry.runtime.set_command_owner(None);
        std::mem::swap(
            &mut entry.runtime.view,
            &mut entry.views.get_mut(&view).unwrap().state,
        );
        let edits = entry.runtime.ops.take_pending();
        if generation != entry.runtime.document_generation() {
            for info in entry.views.values_mut() {
                info.state.folds.set_ranges(Vec::new());
                info.state.clamp_all(&entry.runtime.state);
            }
        } else if !edits.is_empty() {
            let changes: Vec<_> = edits.iter().map(PendingEdit::to_document_change).collect();
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
                info.state.folds.apply_changes(&changes);
                for (selection, (head, anchor)) in info.state.selections.iter_mut().zip(selections)
                {
                    (selection.head_row, selection.head_column) =
                        entry.runtime.state.row_col_from_offset(head);
                    (selection.anchor_row, selection.anchor_column) =
                        entry.runtime.state.row_col_from_offset(anchor);
                }
                info.state.clamp_all(&entry.runtime.state);
            }
        }
        #[cfg(feature = "ide-services")]
        let changed = generation != entry.runtime.document_generation() || !edits.is_empty();
        #[cfg(feature = "ide-services")]
        if self.options.highlighting {
            entry.highlight_edits.extend(edits);
            if entry.highlight_edits.len() > 4096 {
                entry.highlight_edits.clear();
                entry.runtime.highlight.reset_for_document(
                    &entry.runtime.state,
                    entry.runtime.state.line_count() as usize,
                );
            }
        }
        self.collect_notifications(id, Some(view));
        #[cfg(feature = "ide-services")]
        if changed {
            self.update_lsp_snapshot(id);
        }
        match result {
            Ok(value) => Ok(value),
            Err(panic) => std::panic::resume_unwind(panic),
        }
    }

    /// Apply one document-level undo unit. Validate revision, ranges, text
    /// boundaries and final size before changing any state or publishing events.
    pub fn apply_edits(
        &mut self,
        document: DocumentId,
        expected_revision: (u64, u64),
        edits: &[ByteEdit],
    ) -> io::Result<()> {
        self.ensure_running()?;
        if self.is_snapshot_document(document) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Comparison documents are read-only",
            ));
        }
        if self.document_revision(document)? != expected_revision {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Document changed; refresh before applying edits",
            ));
        }
        let state = &self.entry(document)?.runtime.state;
        if state.read_only {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Document is read-only while a workspace edit is saving",
            ));
        }
        let mut ordered = edits.to_vec();
        ordered.sort_by_key(|edit| (edit.range.start, edit.range.end));
        let mut previous_end = None;
        let mut previous_start = None;
        let mut final_size = state.byte_size() + if state.utf8_bom { 3 } else { 0 };
        for edit in &mut ordered {
            if edit.range.start > edit.range.end
                || edit.range.end > state.byte_size()
                || previous_end.is_some_and(|end| edit.range.start < end)
                || previous_start == Some(edit.range.start)
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Edit ranges are invalid or overlap",
                ));
            }
            previous_end = Some(edit.range.end);
            previous_start = Some(edit.range.start);
            if state.kind == DocumentKind::Text {
                for offset in [edit.range.start, edit.range.end] {
                    let (row, col) = state.row_col_from_offset(offset);
                    if state.offset_from_row_col(row, col) != offset {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidInput,
                            "A text edit cannot split a line ending",
                        ));
                    }
                }
                edit.bytes = EditingOperations::normalize_line_endings(state, &edit.bytes);
            }
            final_size = final_size
                .checked_sub(edit.range.len())
                .and_then(|size| size.checked_add(edit.bytes.len()))
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Edit size overflow"))?;
        }
        if final_size > bedtime_files::files::MAX_FILE_SIZE {
            return Err(bedtime_files::files::file_too_large(
                Path::new(&state.path),
                final_size as u64,
            ));
        }
        ordered.retain(|edit| !edit.range.is_empty() || !edit.bytes.is_empty());
        if ordered.is_empty() {
            return Ok(());
        }
        ordered.reverse();
        if state.kind == DocumentKind::Bytes {
            let inverse = self.apply_byte_sequence(document, &ordered)?;
            let history = &mut self.entry_mut(document)?.byte_history;
            history.undo.push(ByteTransaction {
                undo: inverse,
                redo: ordered,
            });
            if history.undo.len() > 50 {
                history.undo.remove(0);
            }
            history.redo.clear();
        } else {
            let mut operations = Vec::new();
            for edit in ordered {
                let (row, column) = state.row_col_from_offset(edit.range.start);
                if !edit.range.is_empty() {
                    operations.push(TextOp {
                        kind: OpKind::Delete,
                        row,
                        column,
                        length: edit.range.len() as i32,
                        ..Default::default()
                    });
                }
                if !edit.bytes.is_empty() {
                    operations.push(TextOp {
                        kind: OpKind::Insert,
                        row,
                        column,
                        text: edit.bytes,
                        ..Default::default()
                    });
                }
            }
            let existing = self.view_ids(document).into_iter().next();
            let view = match existing {
                Some(view) => view,
                None => self.create_view(document)?,
            };
            let result =
                self.with_commands(view, |commands| commands.apply_transaction(&operations));
            if existing.is_none() {
                self.detach_view(view);
            }
            result?;
        }
        Ok(())
    }

    /// Keep independent byte-view positions attached to their original content.
    /// Returns false after a replacement or an expired change history; positions
    /// are still clamped safely. This journal contains lengths, never file data.
    pub fn transform_byte_offsets(
        &self,
        document: DocumentId,
        since: (u64, u64),
        offsets: &mut [usize],
    ) -> io::Result<bool> {
        let entry = self.entry(document)?;
        let state = &entry.runtime.state;
        if state.kind != DocumentKind::Bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Byte positions require a byte document",
            ));
        }
        let current = (
            entry.runtime.document_generation(),
            entry.runtime.ops.generation(),
        );
        let available = since.0 == current.0
            && since.1 <= current.1
            && (since.1 == current.1
                || entry
                    .byte_splices
                    .front()
                    .is_some_and(|first| since.1 >= first.revision.saturating_sub(1)));
        if available {
            for change in entry
                .byte_splices
                .iter()
                .filter(|change| change.revision > since.1)
            {
                for offset in offsets.iter_mut() {
                    if *offset >= change.start + change.removed {
                        *offset = offset
                            .saturating_sub(change.removed)
                            .saturating_add(change.inserted);
                    } else if *offset >= change.start {
                        *offset = change.start + change.inserted;
                    }
                }
            }
        }
        for offset in offsets {
            *offset = (*offset).min(state.byte_size());
        }
        Ok(available)
    }

    fn apply_byte_sequence(
        &mut self,
        document: DocumentId,
        edits: &[ByteEdit],
    ) -> io::Result<Vec<ByteEdit>> {
        let entry = self.entry_mut(document)?;
        let mut inverse = Vec::with_capacity(edits.len());
        for edit in edits {
            let removed = entry
                .runtime
                .ops
                .splice_bytes(&mut entry.runtime.state, edit.range.clone(), &edit.bytes)
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "Invalid byte history range")
                })?;
            entry.byte_splices.push_back(ByteSplice {
                revision: entry.runtime.ops.generation(),
                start: edit.range.start,
                removed: edit.range.len(),
                inserted: edit.bytes.len(),
            });
            if entry.byte_splices.len() > 4096 {
                entry.byte_splices.pop_front();
            }
            inverse.push(ByteEdit {
                range: edit.range.start..edit.range.start + edit.bytes.len(),
                bytes: removed,
            });
        }
        inverse.reverse();
        let event = DidEdit {
            version: entry.runtime.state.version,
            first_row: 0,
            last_row: 0,
            changes: Vec::new(),
        };
        entry
            .runtime
            .events
            .emit_did_edit_document(&event, &entry.runtime.state);
        let path = entry.runtime.state.path.clone();
        self.history.borrow_mut().forget_existing_file(&path);
        self.collect_notifications(document, None);
        Ok(inverse)
    }

    pub fn undo_document(&mut self, document: DocumentId) -> io::Result<()> {
        self.history_document(document, true)
    }

    pub fn redo_document(&mut self, document: DocumentId) -> io::Result<()> {
        self.history_document(document, false)
    }

    fn history_document(&mut self, document: DocumentId, undo: bool) -> io::Result<()> {
        self.ensure_running()?;
        if self.entry(document)?.workspace_edit_locked {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Wait for the workspace edit to finish saving before undo",
            ));
        }
        if self.is_snapshot_document(document) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Comparison documents are read-only",
            ));
        }
        let key = self.entry(document)?.runtime.history_key().to_owned();
        let group = self.history.borrow_mut().workspace_group(&key, undo);
        if let Some(_group) = group {
            #[cfg(feature = "ide-services")]
            return self.apply_workspace_history(&_group, undo);
            #[cfg(not(feature = "ide-services"))]
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Multi-document language-server history requires editor-services",
            ));
        }
        self.history_document_ungrouped(document, undo)
    }

    pub(super) fn history_document_ungrouped(
        &mut self,
        document: DocumentId,
        undo: bool,
    ) -> io::Result<()> {
        if self.document_kind(document)? == DocumentKind::Bytes {
            let history = &mut self.entry_mut(document)?.byte_history;
            let transaction = if undo {
                history.undo.pop()
            } else {
                history.redo.pop()
            };
            if let Some(transaction) = transaction {
                self.apply_byte_sequence(
                    document,
                    if undo {
                        &transaction.undo
                    } else {
                        &transaction.redo
                    },
                )?;
                let history = &mut self.entry_mut(document)?.byte_history;
                if undo {
                    history.redo.push(transaction);
                } else {
                    history.undo.push(transaction);
                }
            }
        } else {
            let existing = self.view_ids(document).into_iter().next();
            let view = match existing {
                Some(view) => view,
                None => self.create_view(document)?,
            };
            let result = self.with_commands(view, |commands| {
                if undo {
                    commands.undo()
                } else {
                    commands.redo()
                }
            });
            if existing.is_none() {
                self.detach_view(view);
            }
            result?;
        }
        Ok(())
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
        if self.entry(document)?.workspace_edit_locked {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Wait for the workspace edit to finish saving before closing",
            ));
        }
        if policy == ClosePolicy::Save && self.save_pending(document) {
            return Ok(false);
        }
        if policy == ClosePolicy::Save && self.entry(document)?.runtime.state.dirty {
            if self.entry(document)?.runtime.state.path.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Save As is required before closing an unnamed document",
                ));
            }
            self.save(document)?;
            if self.entry(document)?.runtime.state.dirty {
                return Ok(false);
            }
        }
        let key = self.entry(document)?.runtime.history_key().to_owned();
        if self.history.borrow().has_workspace_history(&key)
            && self.entry(document)?.runtime.state.dirty
        {
            for view in self.view_ids(document) {
                self.detach_view(view);
            }
            self.queued.push(SessionEvent::Closed { document });
            return Ok(true);
        }
        #[cfg(feature = "ide-services")]
        if let Some(pool) = &mut self.lsp
            && let Err(error) = pool.unregister_document(document)
        {
            self.record_error(Some(document), ServiceKind::Lsp, &error);
        }
        let entry = self.documents.remove(&document).unwrap();
        #[cfg(feature = "ide-services")]
        let mut entry = entry;
        let history_key = entry.runtime.history_key().to_owned();
        if entry.runtime.state.path.is_empty() {
            self.history.borrow_mut().remove_memory_file(&history_key);
        } else if entry.removed {
            self.history.borrow_mut().forget_file(&history_key);
        }
        entry.runtime.save_service.cancel_pending();
        #[cfg(feature = "ide-services")]
        entry.runtime.bind_lsp_client(None);
        #[cfg(feature = "ide-services")]
        entry.runtime.highlight.cancel_highlighting();
        for view in entry.views.keys() {
            self.views.remove(view);
        }
        self.paths.retain(|_, id| *id != document);
        #[cfg(feature = "remote")]
        self.forget_remote_document(document, true);
        self.queued.push(SessionEvent::Closed { document });
        Ok(true)
    }
}

impl DocumentSession {
    pub fn save(&mut self, document: DocumentId) -> io::Result<bool> {
        self.ensure_running()?;
        if self.is_snapshot_document(document) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Comparison documents cannot be saved",
            ));
        }
        #[cfg(feature = "remote")]
        if self.is_remote() {
            return self.queue_remote_save(document, None);
        }
        let entry = self.entry(document)?;
        if !entry.runtime.state.path.is_empty()
            && (entry.removed || !Path::new(&entry.runtime.state.path).try_exists()?)
        {
            self.invalidate_removed_path(document)?;
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "File was removed; use Save As before saving",
            ));
        }
        let monitoring = self.options.monitoring;
        let entry = self.entry_mut(document)?;
        guard_write(entry)?;
        let saved = entry.runtime.save()?;
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
        if self.entry(document)?.workspace_edit_locked {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Workspace edit is saving",
            ));
        }
        if self.is_snapshot_document(document) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Comparison documents cannot be saved",
            ));
        }
        #[cfg(feature = "remote")]
        if self.is_remote() {
            return self.queue_remote_save(document, Some(path_string(destination)?.to_owned()));
        }
        let target = self.check_rebind_path(document, destination)?;
        let previous = self.entry(document)?.runtime.state.path.clone();
        if previous == path_string(&target)? {
            return self.save(document);
        }
        let entry = self.entry_mut(document)?;
        let old_dirty = entry.runtime.state.dirty;
        let old_conflict = entry.runtime.disk_conflict.take();
        #[cfg(feature = "ide-services")]
        let old_lsp = entry.runtime.lsp_client();
        #[cfg(feature = "ide-services")]
        entry.runtime.bind_lsp_client(None);
        entry.runtime.state.path = path_string(&target)?.to_owned();
        entry.runtime.state.dirty = true;
        let result = entry.runtime.save();
        entry.runtime.state.path = previous;
        #[cfg(feature = "ide-services")]
        entry.runtime.bind_lsp_client(old_lsp);
        entry.runtime.disk_conflict = old_conflict;
        match result {
            Ok(saved) => {
                if saved && self.document_kind(document)? == DocumentKind::Bytes {
                    self.history
                        .borrow_mut()
                        .forget_existing_file(path_string(&target)?);
                }
                self.entry_mut(document)?.runtime.disk_conflict = None;
                self.rebind_path(document, &target)?;
                let entry = self.entry_mut(document)?;
                entry.removed = false;
                entry.allow_recreate = false;
                #[cfg(feature = "ide-services")]
                if let Some(client) = entry.runtime.lsp_client() {
                    let state = &entry.runtime.state;
                    let result = client.borrow_mut().did_save(&state.path, || {
                        String::from_utf8(state.join())
                            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
                    });
                    if let Err(error) = result {
                        self.record_error(Some(document), ServiceKind::Lsp, &error);
                    }
                }
                self.collect_notifications(document, None);
                Ok(saved)
            }
            Err(error) => {
                let entry = self.entry_mut(document)?;
                entry.runtime.state.dirty = old_dirty;
                // The attempted save cancelled its clock. Keep the original
                // dirty document eligible for its configured autosave.
                entry.runtime.save_service.on_did_edit(&entry.runtime.state);
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
        let target = if self.is_remote() {
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
        if self.entry(document)?.workspace_edit_locked {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Workspace edit is saving",
            ));
        }
        self.ensure_running()?;
        if self.save_pending(document) {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Wait for the remote save before renaming",
            ));
        }
        let target = self.check_rebind_path(document, destination)?;
        let target_string = path_string(&target)?.to_owned();
        let previous = self.entry(document)?.runtime.state.path.clone();
        if target_string == previous {
            return Ok(());
        }
        let old_key = self.entry(document)?.runtime.history_key().to_owned();
        if self.document_kind(document)? == DocumentKind::Text {
            self.history
                .borrow_mut()
                .rekey_file(&old_key, &target_string)?;
        }
        #[cfg(feature = "ide-services")]
        if let Some(pool) = &mut self.lsp
            && let Err(error) = pool.unregister_document(document)
        {
            self.record_error(Some(document), ServiceKind::Lsp, &error);
        }
        #[cfg(feature = "ide-services")]
        let git_enabled = self.options.git && self.document_kind(document)? == DocumentKind::Text;
        let remote = self.is_remote();
        let entry = self.entry_mut(document)?;
        #[cfg(feature = "ide-services")]
        entry.runtime.bind_lsp_client(None);
        entry.runtime.set_history_key(None);
        entry.runtime.rebind_document_path(&target_string);
        #[cfg(feature = "ide-services")]
        if !remote {
            entry
                .runtime
                .git
                .borrow_mut()
                .on_document_opened(&entry.runtime.state, git_enabled);
        }
        entry.runtime.save_service.cancel_pending();
        entry.runtime.save_service.on_did_edit(&entry.runtime.state);
        entry.highlight_edits.clear();
        entry.removed = false;
        entry.original_path = None;
        entry.removal_pending_since = None;
        entry.allow_recreate = false;
        let watch = if remote {
            Ok(())
        } else if entry.runtime.state.dirty {
            entry.monitor.watch(&target)
        } else {
            monitor_saved_bytes(entry)
        };
        if let Err(error) = watch {
            let message = error.to_string();
            entry.runtime.disk_conflict = Some(message.clone());
            self.errors.push(ServiceError {
                document: Some(document),
                service: ServiceKind::Monitor,
                message: message.clone(),
            });
            self.queued
                .push(SessionEvent::Conflict { document, message });
        }
        self.paths.retain(|_, id| *id != document);
        self.paths.insert(target, document);
        #[cfg(feature = "remote")]
        self.forget_remote_document(document, false);
        #[cfg(feature = "ide-services")]
        self.attach_lsp(document, None);
        #[cfg(feature = "ide-services")]
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
        if entry.removed || entry.runtime.state.path.is_empty() {
            return Ok(());
        }
        entry.removed = true;
        entry.removal_pending_since = None;
        entry.allow_recreate = false;
        entry.runtime.save_service.cancel_pending();
        entry.monitor.reset();
        entry.runtime.disk_conflict = None;
        let path = entry.runtime.state.path.clone();
        #[cfg(feature = "ide-services")]
        {
            self.project_diagnostics.clear_path(&path);
            if self.project_diagnostics.relevant_file(&path) {
                self.project_diagnostics.saved();
            }
            #[cfg(feature = "ide-services")]
            if let Some(pool) = &self.lsp {
                pool.clear_diagnostics(&path);
            }
        }
        self.queued.push(SessionEvent::Removed { document, path });
        Ok(())
    }

    /// Preserve a deleted document and its views/history as a Save As buffer.
    /// The host calls this after attempting to commit every attached panel draft.
    pub fn detach_removed_document(&mut self, document: DocumentId) -> io::Result<()> {
        if self.entry(document)?.workspace_edit_locked {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Workspace edit is saving",
            ));
        }
        self.ensure_running()?;
        let previous = self.entry(document)?.runtime.state.path.clone();
        if previous.is_empty() {
            return Ok(());
        }
        let key = format!("bedtime:untitled:{}", document.0);
        if self.document_kind(document)? == DocumentKind::Text {
            let old_key = self.entry(document)?.runtime.history_key().to_owned();
            self.history.borrow_mut().rekey_file(&old_key, &key)?;
        }
        #[cfg(feature = "ide-services")]
        if let Some(pool) = &mut self.lsp
            && let Err(error) = pool.unregister_document(document)
        {
            self.record_error(Some(document), ServiceKind::Lsp, &error);
        }
        let entry = self.entry_mut(document)?;
        #[cfg(feature = "ide-services")]
        entry.runtime.bind_lsp_client(None);
        entry.runtime.set_history_key(Some(key));
        entry.runtime.detach_document_path();
        // Even a clean disk buffer with an invalid panel draft must prompt for
        // Save As when subsequently closed, rather than silently losing it.
        entry.runtime.state.dirty = true;
        entry.runtime.save_service.cancel_pending();
        entry.runtime.disk_conflict = None;
        #[cfg(feature = "ide-services")]
        entry
            .runtime
            .git
            .borrow_mut()
            .on_document_opened(&entry.runtime.state, false);
        entry.monitor.reset();
        entry.original_path = Some(previous.clone());
        entry.removed = false;
        entry.removal_pending_since = None;
        entry.allow_recreate = false;
        self.paths.retain(|_, id| *id != document);
        #[cfg(feature = "remote")]
        self.forget_remote_document(document, true);
        self.queued.push(SessionEvent::PathChanged {
            document,
            previous,
            path: String::new(),
        });
        Ok(())
    }

    pub fn original_path(&self, document: DocumentId) -> io::Result<Option<&str>> {
        Ok(self.entry(document)?.original_path.as_deref())
    }

    pub fn keep_buffer(&mut self, document: DocumentId) -> io::Result<()> {
        if self.entry(document)?.workspace_edit_locked {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Workspace edit is saving",
            ));
        }
        if self.entry(document)?.removed {
            return self.detach_removed_document(document);
        }
        #[cfg(feature = "remote")]
        if self.is_remote() {
            return self.queue_remote_keep(document);
        }
        let entry = self.entry_mut(document)?;
        if !entry.runtime.state.path.is_empty() {
            entry
                .monitor
                .accept_current(Path::new(&entry.runtime.state.path))?;
        }
        entry.runtime.disk_conflict = None;
        entry.allow_recreate = entry.removed;
        entry.removed = false;
        entry.runtime.save_service.on_did_edit(&entry.runtime.state);
        Ok(())
    }

    pub fn reload_from_disk(&mut self, document: DocumentId) -> io::Result<()> {
        if self.entry(document)?.workspace_edit_locked {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Workspace edit is saving",
            ));
        }
        #[cfg(feature = "remote")]
        if self.is_remote() {
            return self.queue_remote_reload(document);
        }
        let path = self.entry(document)?.runtime.state.path.clone();
        let raw = match if self.document_kind(document)? == DocumentKind::Text {
            read_file_raw(Path::new(&path))
        } else {
            read_file_bytes(Path::new(&path))
        } {
            Ok(raw) => raw,
            Err(error) => {
                let entry = self.entry_mut(document)?;
                entry.runtime.disk_conflict = Some(error.to_string());
                entry.runtime.save_service.cancel_pending();
                return Err(error);
            }
        };
        let monitoring = self.options.monitoring;
        #[cfg(feature = "ide-services")]
        let git = self.options.git && self.document_kind(document)? == DocumentKind::Text;
        let entry = self.entry_mut(document)?;
        if monitoring {
            entry.monitor.watch_bytes(Path::new(&path), &raw.raw)?;
        } else {
            entry.monitor.reset();
        }
        let invalidate_text_history = entry.runtime.state.kind == DocumentKind::Bytes
            && !entry.runtime.state.bytes_equal(&raw.raw);
        entry.runtime.set_content(&raw.raw);
        entry.byte_history = ByteHistory::default();
        entry.byte_splices.clear();
        entry.runtime.state.path = path;
        entry.highlight_edits.clear();
        entry.removed = false;
        entry.allow_recreate = false;
        let key = entry.runtime.history_key().to_owned();
        entry.runtime.project_undo().forget_file(&key);
        for view in entry.views.values_mut() {
            view.state.folds.set_ranges(Vec::new());
            view.state.clamp_all(&entry.runtime.state);
        }
        #[cfg(feature = "ide-services")]
        entry
            .runtime
            .git
            .borrow_mut()
            .on_document_opened(&entry.runtime.state, git);
        let generation = entry.runtime.document_generation();
        if invalidate_text_history {
            self.history.borrow_mut().forget_existing_file(&key);
        }
        #[cfg(feature = "ide-services")]
        self.reopen_lsp(document);
        self.queued.push(SessionEvent::Reloaded {
            document,
            generation,
        });
        Ok(())
    }

    /// A content replacement invalidates position-based undo and pending jobs;
    /// every view keeps its own scroll and clamps its own carets.
    pub fn replace_content(&mut self, document: DocumentId, bytes: &[u8]) -> io::Result<()> {
        if self.entry(document)?.workspace_edit_locked {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Workspace edit is saving",
            ));
        }
        if self.is_snapshot_document(document) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Comparison documents are read-only",
            ));
        }
        let entry = self.entry_mut(document)?;
        let invalidate_text_history = entry.runtime.state.kind == DocumentKind::Bytes
            && !entry.runtime.state.bytes_equal(bytes);
        entry.runtime.set_content(bytes);
        entry.byte_history = ByteHistory::default();
        entry.byte_splices.clear();
        entry.runtime.state.dirty = true;
        entry.highlight_edits.clear();
        entry.runtime.save_service.on_did_edit(&entry.runtime.state);
        let key = entry.runtime.history_key().to_owned();
        entry.runtime.project_undo().forget_file(&key);
        for view in entry.views.values_mut() {
            view.state.folds.set_ranges(Vec::new());
            view.state.clamp_all(&entry.runtime.state);
        }
        let generation = entry.runtime.document_generation();
        if invalidate_text_history {
            self.history.borrow_mut().forget_existing_file(&key);
        }
        #[cfg(feature = "ide-services")]
        self.reopen_lsp(document);
        self.queued.push(SessionEvent::Reloaded {
            document,
            generation,
        });
        Ok(())
    }

    pub fn tick(&mut self) -> TickReport {
        self.prune_views();
        #[cfg(feature = "remote")]
        self.poll_remote();
        let report = TickReport::default();
        #[cfg(feature = "ide-services")]
        let mut report = report;
        if self.stopped {
            return self.drain_report(report);
        }
        #[cfg(feature = "ide-services")]
        {
            self.project_diagnostics
                .bind(self.options.project_root.clone(), self.remote_client());
            self.project_diagnostics.tick();
            if let Some(paths) = self.project_diagnostics.take_discovered_paths()
                && let Some(pool) = &mut self.lsp
            {
                pool.note_project_paths(&paths);
            }
            #[cfg(feature = "ide-services")]
            if let Some(pool) = &mut self.lsp {
                pool.set_project_check_owner(self.project_diagnostics.owns_cargo_check());
                report.lsp_events = pool.poll();
            }
            self.refresh_lsp_bindings();
            self.poll_workspace_edits(&mut report);
        }
        let monitor = self.options.monitoring
            && self
                .last_monitor
                .is_none_or(|last| last.elapsed() >= Duration::from_millis(500));
        if monitor {
            self.last_monitor = Some(Instant::now());
        }
        for id in self.document_ids() {
            if self.documents[&id].workspace_edit_locked {
                continue;
            }
            let confirm_removal = self.documents[&id]
                .removal_pending_since
                .is_some_and(|since| since.elapsed() >= Duration::from_millis(250));
            if !self.is_snapshot_document(id)
                && (monitor || confirm_removal)
                && !self.is_remote()
                && let Err(error) = self.poll_monitor(id)
            {
                self.record_error(Some(id), ServiceKind::Monitor, &error);
            }
            let entry = self.documents.get_mut(&id).unwrap();
            entry
                .runtime
                .ops
                .install_highlight_edits(std::mem::take(&mut entry.highlight_edits));
            entry.runtime.poll_document_services();
            entry.runtime.ops.clear_pending();
            #[cfg(feature = "remote")]
            if self.is_remote() && !self.is_snapshot_document(id) {
                self.tick_remote_document(id, monitor);
            }
            if !self.is_remote()
                && self.options.autosave.is_some()
                && !self.documents[&id].autosave_paused
            {
                let entry = self.documents.get_mut(&id).unwrap();
                let result = if entry.removed
                    || entry.removal_pending_since.is_some()
                    || entry.runtime.disk_conflict.is_some()
                {
                    entry.runtime.save_service.cancel_pending();
                    Ok(false)
                } else if entry.runtime.state.dirty && !entry.runtime.state.path.is_empty() {
                    guard_write(entry).and_then(|()| entry.runtime.poll_save())
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
                            self.record_error(Some(id), ServiceKind::Monitor, &error);
                        }
                    }
                    Ok(false) => {}
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        let _ = self.invalidate_removed_path(id);
                    }
                    Err(error) => self.record_error(Some(id), ServiceKind::Autosave, &error),
                }
            }
            self.collect_notifications(id, None);
        }
        if let Some(path) = &self.options.history_path
            && self
                .last_history
                .is_none_or(|last| last.elapsed() >= Duration::from_secs(3))
        {
            let result = self.history.borrow_mut().flush_to(path);
            match result {
                Ok(()) => self.last_history = Some(Instant::now()),
                Err(error) => self.record_error(None, ServiceKind::History, &error),
            }
        }
        #[cfg(feature = "ide-services")]
        self.refresh_document_diagnostics();
        self.drain_report(report)
    }

    pub fn take_events(&mut self) -> Vec<SessionEvent> {
        std::mem::take(&mut self.queued)
    }

    /// A shared workspace watcher can refresh an affected open document without
    /// waiting for the standalone monitor interval. Missing paths are confirmed
    /// after a short grace period so atomic saves and renames can settle.
    pub fn notify_disk_change(&mut self, document: DocumentId) -> io::Result<()> {
        #[cfg(feature = "remote")]
        if self.is_remote() {
            return self.queue_remote_monitor(document);
        }
        self.poll_monitor(document)
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
                if self.documents[&id].runtime.state.dirty {
                    let result = if self.documents[&id].runtime.state.path.is_empty() {
                        Err(io::Error::new(
                            io::ErrorKind::InvalidInput,
                            "Save As is required before shutdown",
                        ))
                    } else {
                        self.save(id).and_then(|_| {
                            if self.documents[&id].runtime.state.dirty {
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
                        self.record_error(Some(id), ServiceKind::Save, &error);
                        first_error.get_or_insert(error);
                    }
                }
            }
            if let Some(error) = first_error {
                return Err(error);
            }
        }
        if let Some(path) = &self.options.history_path {
            self.history.borrow_mut().flush_to(path)?;
        }
        for id in self.document_ids() {
            self.close_document(id, ClosePolicy::Discard)?;
        }
        #[cfg(feature = "ide-services")]
        if let Some(pool) = &mut self.lsp {
            pool.shutdown();
        }
        #[cfg(feature = "remote")]
        if let Some(remote) = &self.remote {
            remote.client.disconnect();
        }
        self.stopped = true;
        Ok(self.drain_report(TickReport::default()))
    }

    pub fn configure(&mut self, mut options: SessionOptions) -> io::Result<()> {
        self.ensure_running()?;
        #[cfg(feature = "remote")]
        if self.is_remote() {
            return self.configure_remote(options);
        }
        if let Some(root) = &options.project_root {
            options.project_root = Some(canonical_root(root)?);
        }
        #[cfg(feature = "ide-services")]
        if options.lsp_config.is_some() && options.project_root.is_none() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Configured services require an explicit project root",
            ));
        }
        #[cfg(feature = "ide-services")]
        let root_changed = options.project_root != self.options.project_root;
        #[cfg(feature = "ide-services")]
        if root_changed {
            self.document_diagnostics.invalidate_document_keys();
        }
        #[cfg(feature = "ide-services")]
        let highlight_changed = options.highlighting != self.options.highlighting;
        let monitoring_changed = options.monitoring != self.options.monitoring;
        let start_autosave = options.autosave.is_some() && self.options.autosave.is_none();
        let history_changed = options.history_path != self.options.history_path;
        let next_history = if history_changed {
            options
                .history_path
                .as_ref()
                .map(|path| {
                    let mut history = self.history.borrow().clone();
                    history.merge_from(path)?;
                    Ok::<_, io::Error>(history)
                })
                .transpose()?
        } else {
            None
        };
        #[cfg(feature = "ide-services")]
        let lsp_changed = root_changed
            || options.lsp_config != self.options.lsp_config
            || options.lsp_project_config != self.options.lsp_project_config
            || options.lsp_config_mode != self.options.lsp_config_mode
            || options.lsp_file_observations != self.options.lsp_file_observations;
        if history_changed && let Some(path) = &self.options.history_path {
            self.history.borrow_mut().flush_to(path)?;
        }
        #[cfg(feature = "ide-services")]
        if lsp_changed {
            for entry in self.documents.values_mut() {
                #[cfg(feature = "ide-services")]
                entry.runtime.bind_lsp_client(None);
            }
            #[cfg(feature = "ide-services")]
            if let Some(pool) = &mut self.lsp {
                pool.shutdown();
            }
            self.lsp = None;
        }
        self.options = options;
        if monitoring_changed {
            self.last_monitor = None;
        }
        if let Some(history) = next_history {
            *self.history.borrow_mut() = history;
        }
        if history_changed {
            self.last_history = None;
        }
        #[cfg(feature = "ide-services")]
        self.initialize_lsp();
        for id in self.document_ids() {
            let entry = self.documents.get_mut(&id).unwrap();
            #[cfg(feature = "ide-services")]
            entry.runtime.highlight.set_enabled(
                self.options.highlighting && entry.runtime.state.kind == DocumentKind::Text,
            );
            #[cfg(feature = "ide-services")]
            if highlight_changed {
                entry.runtime.refresh_highlighting();
                entry.highlight_edits.clear();
            }
            #[cfg(feature = "ide-services")]
            entry.runtime.set_git_changed_lines(
                self.options.git && entry.runtime.state.kind == DocumentKind::Text,
            );
            if let Some(idle) = self.options.autosave {
                entry
                    .runtime
                    .save_service
                    .set_autosave_idle_ms(idle.as_millis().min(i32::MAX as u128) as i32);
                if start_autosave {
                    entry.runtime.save_service.on_did_edit(&entry.runtime.state);
                }
            } else {
                entry.runtime.save_service.cancel_pending();
            }
            #[cfg(feature = "ide-services")]
            if entry.runtime.state.kind == DocumentKind::Text
                && !entry.runtime.state.read_only
                && (root_changed || (!entry.git_initialized && self.options.git))
            {
                let root = self
                    .options
                    .project_root
                    .as_ref()
                    .and_then(|p| p.to_str())
                    .unwrap_or("");
                entry
                    .runtime
                    .git
                    .borrow_mut()
                    .init(&entry.runtime.state, root, self.options.git);
                entry.git_initialized = true;
            }
            #[cfg(feature = "ide-services")]
            if lsp_changed {
                #[cfg(feature = "ide-services")]
                self.attach_lsp(id, None);
            }
        }
        Ok(())
    }

    #[cfg(feature = "ide-services")]
    pub fn project_diagnostics(&self) -> crate::ProjectDiagnosticsSnapshot {
        let lsp = self
            .lsp
            .as_ref()
            .map(WorkspaceLsp::diagnostic_snapshot)
            .unwrap_or_default();
        let unsaved = self
            .documents
            .values()
            .any(|entry| entry.runtime.state.dirty);
        let complete = self
            .lsp
            .as_ref()
            .is_some_and(WorkspaceLsp::diagnostic_coverage_complete);
        self.project_diagnostics.snapshot(lsp, unsaved, complete)
    }

    #[cfg(feature = "ide-services")]
    fn refresh_document_diagnostics(&mut self) {
        // Bed owns Cargo checks, so rust-analyzer does not publish their
        // semantic errors. Inline views and the diagnostics panel must consume
        // the same merged results while server payloads remain intact for fixes.
        self.document_diagnostics
            .replace_snapshot(self.project_diagnostics().by_path, self.is_remote());
        let enabled = self.options.project_root.is_some() || self.lsp.is_some();
        for entry in self.documents.values_mut() {
            entry.runtime.diagnostics = (enabled
                && entry.runtime.state.kind == DocumentKind::Text
                && !entry.runtime.state.path.is_empty()
                && (!entry.runtime.state.read_only || entry.workspace_edit_locked))
                .then(|| {
                    self.document_diagnostics
                        .register_document(&entry.runtime.state.path);
                    self.document_diagnostics.clone()
                });
        }
    }

    /// Index notifications include unopened files. Build outputs are excluded
    /// so a check cannot schedule itself by writing its target directory.
    #[cfg(feature = "ide-services")]
    pub fn observe_project_file_changes(&mut self, changes: &[bedtime_remote::FilesystemChange]) {
        if !changes.is_empty() {
            self.document_diagnostics.invalidate_document_keys();
        }
        #[cfg(feature = "ide-services")]
        if let Some(pool) = &mut self.lsp
            && let Err(error) = pool.observe_file_changes(changes)
        {
            self.record_error(None, ServiceKind::Lsp, &error);
        }
        let mut paths = Vec::new();
        let mut removed = Vec::new();
        for change in changes {
            match change {
                bedtime_remote::FilesystemChange::Created { path }
                | bedtime_remote::FilesystemChange::Modified { path } => paths.push(path.clone()),
                bedtime_remote::FilesystemChange::Removed { path } => removed.push(path.clone()),
                bedtime_remote::FilesystemChange::Renamed { from, to } => {
                    removed.push(from.clone());
                    paths.push(to.clone());
                }
            }
        }
        for path in &paths {
            self.project_diagnostics.note_file(path);
        }
        for path in &removed {
            self.project_diagnostics.clear_path(path);
            #[cfg(feature = "ide-services")]
            if let Some(pool) = &self.lsp {
                for candidate in pool
                    .diagnostic_snapshot()
                    .keys()
                    .filter(|candidate| Path::new(candidate).starts_with(path))
                {
                    pool.clear_diagnostics(candidate);
                }
            }
        }
        if paths
            .iter()
            .chain(&removed)
            .any(|path| self.project_diagnostics.relevant_file(path))
        {
            self.project_diagnostics.saved();
            #[cfg(feature = "ide-services")]
            if let Some(pool) = &mut self.lsp {
                pool.refresh_workspace_diagnostics();
            }
        }
    }

    #[cfg(feature = "ide-services")]
    pub fn project_check_config(&self) -> &crate::ProjectCheckConfig {
        &self.project_diagnostics.config
    }

    #[cfg(feature = "ide-services")]
    pub fn configure_project_check(&mut self, config: crate::ProjectCheckConfig) {
        self.project_diagnostics.configure(config, true);
    }

    #[cfg(feature = "ide-services")]
    pub fn reset_project_check(&mut self) {
        self.project_diagnostics
            .configure(Default::default(), false);
    }

    #[cfg(feature = "ide-services")]
    pub fn request_project_check(&mut self) {
        self.project_diagnostics.request();
    }

    #[cfg(feature = "ide-services")]
    pub fn cancel_project_check(&mut self) {
        self.project_diagnostics.cancel();
    }

    #[cfg(feature = "ide-services")]
    pub fn lsp(&self) -> Option<&WorkspaceLsp> {
        self.lsp.as_ref()
    }

    #[cfg(feature = "ide-services")]
    pub fn lsp_mut(&mut self) -> Option<&mut WorkspaceLsp> {
        self.lsp.as_mut()
    }

    #[cfg(feature = "ide-services")]
    pub fn reload_lsp_config(&mut self) -> io::Result<()> {
        let result = self
            .lsp
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "LSP is not enabled"))?
            .reload_config();
        self.refresh_lsp_bindings();
        result
    }

    #[cfg(feature = "ide-services")]
    pub fn retry_language(&mut self, language: &str) -> io::Result<bool> {
        self.lsp
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "LSP is not enabled"))?
            .retry_language(language)
    }

    #[cfg(feature = "ide-services")]
    pub fn retry_server(
        &mut self,
        instance: &bedtime_lsp::workspace_lsp::ServerInstanceId,
    ) -> io::Result<bool> {
        self.lsp
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "LSP is not enabled"))?
            .retry_server(instance)
    }

    #[cfg(feature = "ide-services")]
    pub fn accepts_lsp_origin(
        &self,
        origin: &bedtime_lsp::workspace_lsp::LspRequestOrigin,
    ) -> bool {
        self.document_for_view(origin.view_id) == Some(origin.document_id)
            && self
                .documents
                .get(&origin.document_id)
                .is_some_and(|entry| {
                    entry.runtime.document_generation() == origin.document_generation
                        && entry.runtime.state.version == origin.version
                })
            && self
                .lsp
                .as_ref()
                .is_some_and(|pool| pool.origin_is_current(origin))
    }

    #[cfg(feature = "ide-services")]
    fn refresh_lsp_bindings(&mut self) {
        let Some(pool) = &self.lsp else {
            return;
        };
        for (&id, entry) in &mut self.documents {
            let desired = pool.client_for_document(id);
            let current = entry.runtime.lsp_client();
            let unchanged = match (&current, &desired) {
                (Some(a), Some(b)) => Rc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            };
            if !unchanged {
                entry.runtime.bind_lsp_client(desired);
            }
        }
    }

    #[cfg(feature = "ide-services")]
    fn initialize_lsp(&mut self) {
        if self.lsp.is_none()
            && let (Some(config), Some(root)) =
                (&self.options.lsp_config, &self.options.project_root)
        {
            self.lsp = Some(match (&self.remote, self.options.lsp_config_mode) {
                (Some(remote), LspConfigMode::Layered) => WorkspaceLsp::new_remote_layered(
                    self.session_id,
                    config.clone(),
                    root.clone(),
                    remote.target.clone(),
                    remote.client.clone(),
                    self.options.lsp_project_config.clone(),
                ),
                (Some(remote), LspConfigMode::Snapshot) => WorkspaceLsp::new_remote(
                    self.session_id,
                    config.clone(),
                    root.clone(),
                    remote.target.clone(),
                ),
                (None, LspConfigMode::Layered) => WorkspaceLsp::new_layered(
                    self.session_id,
                    config.clone(),
                    root.clone(),
                    self.options.lsp_project_config.clone(),
                ),
                (None, LspConfigMode::Snapshot) => {
                    WorkspaceLsp::new(self.session_id, config.clone(), root.clone())
                }
            });
        }
        #[cfg(feature = "ide-services")]
        if let Some(pool) = &mut self.lsp {
            pool.set_file_observations_available(self.options.lsp_file_observations);
        }
    }

    #[cfg(feature = "ide-services")]
    fn attach_lsp(&mut self, document: DocumentId, origin: Option<OpenOrigin>) {
        let Some(pool) = &mut self.lsp else {
            return;
        };
        let entry = self.documents.get_mut(&document).unwrap();
        if entry.runtime.state.kind == DocumentKind::Bytes || entry.runtime.state.path.is_empty() {
            return;
        }
        let state = &entry.runtime.state;
        let result = if let Some(origin) = origin {
            pool.register_document_from_origin(
                document,
                &state.path,
                &state.join(),
                state.version,
                &state.language_id,
                &origin,
            )
        } else {
            pool.register_document(
                document,
                &state.path,
                &state.join(),
                state.version,
                &state.language_id,
            )
        };
        entry
            .runtime
            .bind_lsp_client(pool.client_for_document(document));
        if let Err(error) = result {
            self.record_error(Some(document), ServiceKind::Lsp, &error);
        }
    }

    #[cfg(feature = "ide-services")]
    fn reopen_lsp(&mut self, document: DocumentId) {
        let Some(pool) = &mut self.lsp else {
            return;
        };
        let state = &self.documents[&document].runtime.state;
        if state.kind == DocumentKind::Bytes || state.path.is_empty() {
            return;
        }
        let result = pool.reopen_document(
            document,
            &state.path,
            &state.join(),
            state.version,
            &state.language_id,
        );
        self.documents
            .get_mut(&document)
            .unwrap()
            .runtime
            .bind_lsp_client(pool.client_for_document(document));
        if let Err(error) = result {
            self.record_error(Some(document), ServiceKind::Lsp, &error);
        }
    }

    #[cfg(feature = "ide-services")]
    fn update_lsp_snapshot(&mut self, document: DocumentId) {
        let Some(pool) = &mut self.lsp else {
            return;
        };
        let state = &self.documents[&document].runtime.state;
        if state.kind == DocumentKind::Text && !state.path.is_empty() {
            let result = pool.update_document_snapshot(
                document,
                &state.path,
                &state.join(),
                state.version,
                &state.language_id,
            );
            if let Err(error) = result {
                self.record_error(Some(document), ServiceKind::Lsp, &error);
            }
            self.refresh_lsp_bindings();
        }
    }

    fn collect_notifications(&mut self, id: DocumentId, view: Option<ViewId>) {
        let entry = self.documents.get(&id).unwrap();
        for message in entry.runtime.take_service_errors() {
            self.errors.push(ServiceError {
                document: Some(id),
                service: ServiceKind::Lsp,
                message,
            });
        }
        for notification in std::mem::take(&mut *entry.notifications.borrow_mut()) {
            self.queued.push(match notification {
                Notification::Edit(edit) => {
                    #[cfg(feature = "ide-services")]
                    self.project_diagnostics.edited();
                    SessionEvent::Edited {
                        document: id,
                        view,
                        generation: entry.runtime.document_generation(),
                        edit,
                    }
                }
                Notification::Save(save) => {
                    #[cfg(feature = "ide-services")]
                    self.project_diagnostics.saved();
                    #[cfg(feature = "ide-services")]
                    if let Some(pool) = &mut self.lsp {
                        pool.refresh_workspace_diagnostics();
                    }
                    SessionEvent::Saved { document: id, save }
                }
            });
        }
    }

    fn poll_monitor(&mut self, document: DocumentId) -> io::Result<()> {
        let entry = self.entry_mut(document)?;
        if entry.runtime.state.path.is_empty()
            || entry.removed
            || entry.runtime.state.byte_size() > bedtime_files::file_monitor::MAX_FILE_SIZE as usize
        {
            return Ok(());
        }
        let path = PathBuf::from(&entry.runtime.state.path);
        if let Some(since) = entry.removal_pending_since {
            if !path.try_exists()? {
                return if since.elapsed() >= Duration::from_millis(250) {
                    self.invalidate_removed_path(document)
                } else {
                    Ok(())
                };
            }
            entry.removal_pending_since = None;
            entry.runtime.save_service.on_did_edit(&entry.runtime.state);
        }
        let change = match entry.monitor.poll(&path) {
            Ok(change) => change,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
            Err(error) => {
                let message = error.to_string();
                let repeated = entry.runtime.disk_conflict.as_deref() == Some(&message);
                entry.runtime.disk_conflict = Some(message);
                entry.runtime.save_service.cancel_pending();
                // Keep retrying the disk read and blocking writes, while an
                // unchanged failure should not reopen a dismissed error window.
                return if repeated { Ok(()) } else { Err(error) };
            }
        };
        let Some(change) = change else {
            return Ok(());
        };
        if change.kind == FileChangeKind::Removed {
            entry.removal_pending_since = Some(Instant::now());
            entry.runtime.save_service.cancel_pending();
            return Ok(());
        }
        let mut bytes = entry.runtime.state.join();
        if entry.runtime.state.utf8_bom {
            bytes.splice(0..0, [0xef, 0xbb, 0xbf]);
        }
        if change.raw.as_deref() == Some(&bytes) {
            return Ok(());
        }
        if entry.runtime.state.dirty {
            let message = "File changed on disk.".to_owned();
            entry.runtime.disk_conflict = Some(message.clone());
            entry.runtime.save_service.cancel_pending();
            self.queued
                .push(SessionEvent::Conflict { document, message });
            Ok(())
        } else {
            match self.reload_from_disk(document) {
                Ok(()) => Ok(()),
                Err(error) => {
                    let entry = self.entry_mut(document)?;
                    entry.runtime.disk_conflict = Some(error.to_string());
                    entry.runtime.save_service.cancel_pending();
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
        service: ServiceKind,
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
                "DocumentSession is shut down",
            ))
        } else {
            Ok(())
        }
    }
}

impl Drop for DocumentSession {
    fn drop(&mut self) {
        #[cfg(feature = "remote")]
        if let Some(remote) = &self.remote {
            remote.client.disconnect();
        }
        for entry in self.documents.values_mut() {
            entry.runtime.save_service.cancel_pending();
            #[cfg(feature = "ide-services")]
            entry.runtime.bind_lsp_client(None);
            #[cfg(feature = "ide-services")]
            entry.runtime.highlight.cancel_highlighting();
        }
        #[cfg(feature = "ide-services")]
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
    if !entry.runtime.state.dirty || entry.runtime.state.path.is_empty() {
        return Ok(());
    }
    if entry.removed
        || (!entry.allow_recreate && !Path::new(&entry.runtime.state.path).try_exists()?)
    {
        entry.runtime.save_service.cancel_pending();
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "File was removed; use Save As before saving",
        ));
    }
    if entry.runtime.disk_conflict.is_some() {
        entry.runtime.save_service.cancel_pending();
    }
    Ok(())
}

fn monitor_saved_bytes(entry: &mut DocumentEntry) -> io::Result<()> {
    if entry.runtime.state.byte_size() > bedtime_files::file_monitor::MAX_FILE_SIZE as usize {
        entry.monitor.reset();
        return Ok(());
    }
    let mut raw = entry.runtime.state.join();
    if entry.runtime.state.utf8_bom {
        raw.splice(0..0, [0xef, 0xbb, 0xbf]);
    }
    let result = entry
        .monitor
        .watch_bytes(Path::new(&entry.runtime.state.path), &raw);
    if let Err(error) = &result {
        entry.runtime.disk_conflict = Some(error.to_string());
        entry.runtime.save_service.cancel_pending();
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

fn document_kind_conflict() -> io::Error {
    io::Error::new(
        io::ErrorKind::AlreadyExists,
        "File is open in another editing mode; save and close its views before switching modes",
    )
}
