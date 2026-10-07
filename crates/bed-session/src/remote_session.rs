//! Connection-scoped remote I/O around the unchanged local document engine.
use super::*;
use bed_remote::{FileBaseline, RemoteClient, Request, Response, SshTarget};
use std::{collections::BTreeSet, sync::mpsc, thread};

enum Operation {
    Open {
        requested: String,
        kind: Option<DocumentKind>,
    },
    Save {
        document: DocumentId,
        path: String,
        generation: u64,
        version: i32,
    },
    Read {
        document: DocumentId,
        generation: u64,
        version: i32,
        mode: ReadMode,
    },
    Git {
        document: DocumentId,
        generation: u64,
    },
    GitStatus,
}
#[derive(Clone, Copy)]
enum ReadMode {
    Monitor,
    Reload,
    Keep,
}
struct Completion {
    operation: Operation,
    result: io::Result<Response>,
}
pub(super) struct RemoteSession {
    pub target: SshTarget,
    pub client: RemoteClient,
    root: String,
    requests: mpsc::SyncSender<(Operation, Request)>,
    completed: mpsc::Receiver<Completion>,
    baselines: BTreeMap<DocumentId, FileBaseline>,
    saving: BTreeSet<DocumentId>,
    reading: BTreeSet<DocumentId>,
    exclusive_reads: BTreeSet<DocumentId>,
    opening: BTreeSet<String>,
    aliases: BTreeMap<String, DocumentId>,
    modified_paths: BTreeSet<String>,
    last_git: Option<Instant>,
    connected: bool,
}
impl RemoteSession {
    fn new(target: SshTarget, client: RemoteClient, root: String) -> Self {
        let (requests, pending) = mpsc::sync_channel::<(Operation, Request)>(128);
        let (results, completed) = mpsc::channel();
        let transport = client.clone();
        thread::spawn(move || {
            while let Ok((mut operation, mut request)) = pending.recv() {
                let result = (|| {
                    if let (
                        Operation::Save { path, .. },
                        Request::WriteFile {
                            root,
                            path: destination,
                            ..
                        },
                    ) = (&mut operation, &mut request)
                    {
                        match transport.call(Request::Canonicalize {
                            root: root.clone(),
                            path: destination.clone(),
                            allow_missing: true,
                        })? {
                            Response::Path { path: canonical } => {
                                path.clone_from(&canonical);
                                *destination = canonical;
                            }
                            _ => {
                                return Err(io::Error::new(
                                    io::ErrorKind::InvalidData,
                                    "Unexpected remote save-path response",
                                ));
                            }
                        }
                    }
                    transport.call(request)
                })();
                if results.send(Completion { operation, result }).is_err() {
                    break;
                }
            }
        });
        Self {
            target,
            client,
            root,
            requests,
            completed,
            baselines: BTreeMap::new(),
            saving: BTreeSet::new(),
            reading: BTreeSet::new(),
            exclusive_reads: BTreeSet::new(),
            opening: BTreeSet::new(),
            aliases: BTreeMap::new(),
            modified_paths: BTreeSet::new(),
            last_git: None,
            connected: true,
        }
    }
    fn queue(&self, operation: Operation, request: Request) -> io::Result<()> {
        if !self.connected {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "SSH workspace disconnected; reconnect before remote operations",
            ));
        }
        self.requests
            .try_send((operation, request))
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => {
                    io::Error::new(io::ErrorKind::WouldBlock, "Remote operation queue is full")
                }
                mpsc::TrySendError::Disconnected(_) => {
                    io::Error::new(io::ErrorKind::NotConnected, "Remote worker stopped")
                }
            })
    }
}
impl EditorSession {
    /// The caller validates the root through the remote agent before construction.
    /// Remote history stays in memory; this never reads/writes a local project path.
    pub fn with_remote_options(
        mut options: SessionOptions,
        target: SshTarget,
        client: RemoteClient,
        root: String,
    ) -> io::Result<Self> {
        if !root.starts_with('/') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Remote project root must be absolute",
            ));
        }
        options.project_root = Some(PathBuf::from(&root));
        options.persistent_history = false;
        let mut session = Self::default();
        session.options = options;
        session.remote = Some(RemoteSession::new(target, client, root));
        session.initialize_lsp();
        Ok(session)
    }
    pub fn is_remote(&self) -> bool {
        self.remote.is_some()
    }
    pub fn remote_connected(&self) -> bool {
        self.remote
            .as_ref()
            .is_some_and(|r| r.connected && r.client.is_connected())
    }
    pub fn remote_client(&self) -> Option<RemoteClient> {
        self.remote.as_ref().map(|r| r.client.clone())
    }
    pub fn ssh_target(&self) -> Option<&SshTarget> {
        self.remote.as_ref().map(|r| &r.target)
    }
    pub fn remote_file_modified(&self, path: &str) -> bool {
        self.remote
            .as_ref()
            .is_some_and(|r| r.modified_paths.contains(path))
    }
    pub fn save_pending(&self, document: DocumentId) -> bool {
        self.remote
            .as_ref()
            .is_some_and(|r| r.saving.contains(&document))
    }
    pub fn open_pending(&self, path: &Path) -> bool {
        self.remote
            .as_ref()
            .is_some_and(|r| path.to_str().is_some_and(|path| r.opening.contains(path)))
    }
    pub(super) fn remote_alias(&self, path: &Path) -> Option<DocumentId> {
        self.remote
            .as_ref()?
            .aliases
            .get(path.to_str()?)
            .copied()
            .filter(|id| self.documents.contains_key(id))
    }
    pub(super) fn forget_remote_document(&mut self, document: DocumentId, closing: bool) {
        if let Some(remote) = &mut self.remote {
            remote.aliases.retain(|_, id| *id != document);
            if closing {
                remote.baselines.remove(&document);
            }
        }
    }

    /// Install a freshly authenticated connection while retaining all local buffers/undo.
    pub fn reconnect_remote(&mut self, client: RemoteClient) -> io::Result<()> {
        let target = self
            .remote
            .as_ref()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Session is local"))?
            .target
            .clone();
        self.reconnect_remote_with_target(client, target)
    }

    /// Reconnect after preparing a new helper version, updating subsequent LSP
    /// launches to use the newly resolved executable as well.
    pub fn reconnect_remote_with_target(
        &mut self,
        client: RemoteClient,
        target: SshTarget,
    ) -> io::Result<()> {
        self.ensure_running()?;
        let old = self
            .remote
            .as_ref()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Session is local"))?;
        if target.host != old.target.host {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Reconnect must use the same SSH host",
            ));
        }
        old.client.disconnect();
        let mut remote = RemoteSession::new(target, client, old.root.clone());
        remote.baselines = old.baselines.clone();
        remote.aliases = old.aliases.clone();
        self.remote = Some(remote);
        for entry in self.documents.values_mut() {
            entry.editor.bind_lsp_client(None);
        }
        if let Some(pool) = &mut self.lsp {
            pool.shutdown();
        }
        self.lsp = None;
        self.initialize_lsp();
        for id in self.document_ids() {
            self.attach_lsp(id);
        }
        for id in self.document_ids() {
            if let Err(error) = self.queue_read(id, ReadMode::Monitor)
                && error.kind() != io::ErrorKind::WouldBlock
            {
                self.record_error(Some(id), "remote", &error);
            }
        }
        Ok(())
    }

    /// Immediate for local files; remote completion arrives as SessionEvent::Opened.
    pub fn request_open_file(&mut self, path: &Path) -> io::Result<Option<DocumentId>> {
        self.request_open_file_with_kind(path, DocumentKind::Text)
    }
    pub fn request_open_file_with_kind(
        &mut self,
        path: &Path,
        kind: DocumentKind,
    ) -> io::Result<Option<DocumentId>> {
        self.request_open_mode(path, Some(kind))
    }
    pub fn request_open_file_auto(&mut self, path: &Path) -> io::Result<Option<DocumentId>> {
        self.request_open_mode(path, None)
    }
    fn request_open_mode(
        &mut self,
        path: &Path,
        kind: Option<DocumentKind>,
    ) -> io::Result<Option<DocumentId>> {
        self.ensure_running()?;
        if self.remote.is_none() {
            return match kind {
                Some(kind) => self.open_file_with_kind(path, kind),
                None => self.open_file_auto(path),
            }
            .map(Some);
        }
        if let Some(id) = self.document_for_path(path) {
            if kind.is_some_and(|kind| self.documents[&id].editor.state.kind != kind) {
                return Err(document_kind_conflict());
            }
            return Ok(Some(id));
        }
        let path = path_string(path)?.to_owned();
        let remote = self.remote.as_mut().unwrap();
        if remote.opening.contains(&path) {
            return Ok(None);
        }
        remote.queue(
            Operation::Open {
                requested: path.clone(),
                kind,
            },
            Request::ReadFile {
                root: remote.root.clone(),
                path: path.clone(),
            },
        )?;
        remote.opening.insert(path);
        Ok(None)
    }
    pub(super) fn queue_remote_save(
        &mut self,
        document: DocumentId,
        destination: Option<String>,
    ) -> io::Result<bool> {
        let entry = self.entry(document)?;
        if destination.is_none()
            && (!entry.editor.state.dirty || entry.editor.state.path.is_empty())
        {
            return Ok(false);
        }
        if destination.is_none() && entry.editor.disk_conflict.is_some() {
            return Err(io::Error::other(
                "File changed on disk; reload or keep the buffer before saving",
            ));
        }
        let path = destination.unwrap_or_else(|| entry.editor.state.path.clone());
        if !path.starts_with('/') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Remote save path must be absolute",
            ));
        }
        if self
            .paths
            .get(Path::new(&path))
            .is_some_and(|id| *id != document)
        {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "Destination is open as another document",
            ));
        }
        let bytes = crate::save_service::EditorSave::bytes_for_save(&entry.editor.state)?;
        let generation = entry.editor.document_generation();
        let version = entry.editor.state.version;
        let same_path = entry.editor.state.path == path;
        let remote = self.remote.as_mut().unwrap();
        if remote.exclusive_reads.contains(&document) {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Wait for remote reload/keep before saving",
            ));
        }
        if remote.saving.contains(&document) {
            return Ok(false);
        }
        let baseline = same_path
            .then(|| remote.baselines.get(&document).cloned())
            .flatten();
        remote.queue(
            Operation::Save {
                document,
                path: path.clone(),
                generation,
                version,
            },
            Request::WriteFile {
                root: remote.root.clone(),
                path,
                bytes,
                baseline,
            },
        )?;
        remote.saving.insert(document);
        self.entry_mut(document)?
            .editor
            .save_service
            .cancel_pending();
        Ok(false)
    }
    fn queue_read(&mut self, document: DocumentId, mode: ReadMode) -> io::Result<()> {
        let entry = self.entry(document)?;
        let path = entry.editor.state.path.clone();
        let generation = entry.editor.document_generation();
        let version = entry.editor.state.version;
        let remote = self.remote.as_mut().unwrap();
        if path.is_empty() {
            return Ok(());
        }
        if remote.saving.contains(&document) {
            return if matches!(mode, ReadMode::Monitor) {
                Ok(())
            } else {
                Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "Wait for the remote save before reload/keep",
                ))
            };
        }
        if remote.exclusive_reads.contains(&document) {
            return if matches!(mode, ReadMode::Monitor) {
                Ok(())
            } else {
                Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "Remote reload/keep is pending",
                ))
            };
        }
        if remote.reading.contains(&document) && matches!(mode, ReadMode::Monitor) {
            return Ok(());
        }
        remote.queue(
            Operation::Read {
                document,
                generation,
                version,
                mode,
            },
            Request::ReadFile {
                root: remote.root.clone(),
                path,
            },
        )?;
        remote.reading.insert(document);
        if !matches!(mode, ReadMode::Monitor) {
            remote.exclusive_reads.insert(document);
        }
        Ok(())
    }
    pub(super) fn queue_remote_reload(&mut self, document: DocumentId) -> io::Result<()> {
        self.queue_read(document, ReadMode::Reload)
    }
    pub(super) fn queue_remote_keep(&mut self, document: DocumentId) -> io::Result<()> {
        self.queue_read(document, ReadMode::Keep)
    }
    pub(super) fn tick_remote_document(&mut self, id: DocumentId, monitor: bool) {
        if !self.remote_connected() {
            return;
        }
        if self.remote.as_ref().unwrap().exclusive_reads.contains(&id) {
            return;
        }
        let entry = &self.documents[&id];
        let autosave = self.options.autosave.is_some()
            && !entry.autosave_paused
            && entry.editor.state.dirty
            && entry.editor.disk_conflict.is_none()
            && entry.editor.save_service.is_due();
        let result = if autosave {
            self.queue_remote_save(id, None).map(|_| ())
        } else if monitor {
            self.queue_read(id, ReadMode::Monitor)
        } else {
            Ok(())
        };
        if let Err(error) = result {
            self.record_error(Some(id), "remote", &error);
        }
    }
    pub(super) fn configure_remote(&mut self, mut options: SessionOptions) -> io::Result<()> {
        if options.project_root != self.options.project_root {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Open a new session to change remote roots",
            ));
        }
        options.persistent_history = false;
        let start_autosave = options.autosave.is_some() && self.options.autosave.is_none();
        let lsp_changed = options.lsp_config != self.options.lsp_config;
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
        self.initialize_lsp();
        for id in self.document_ids() {
            let entry = self.documents.get_mut(&id).unwrap();
            entry.editor.highlight.set_enabled(
                self.options.highlighting && entry.editor.state.kind == DocumentKind::Text,
            );
            entry.editor.refresh_highlighting();
            entry.editor.set_git_changed_lines(
                self.options.git && entry.editor.state.kind == DocumentKind::Text,
            );
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
            if lsp_changed {
                self.attach_lsp(id);
            }
        }
        Ok(())
    }
    pub(super) fn poll_remote(&mut self) {
        let completions: Vec<_> = self
            .remote
            .as_ref()
            .map(|r| r.completed.try_iter().collect())
            .unwrap_or_default();
        for Completion { operation, result } in completions {
            let document = match &operation {
                Operation::Open { requested, .. } => {
                    self.remote.as_mut().unwrap().opening.remove(requested);
                    None
                }
                Operation::Save { document, .. } => {
                    self.remote.as_mut().unwrap().saving.remove(document);
                    Some(*document)
                }
                Operation::Read { document, mode, .. } => {
                    self.remote.as_mut().unwrap().reading.remove(document);
                    if !matches!(mode, ReadMode::Monitor) {
                        self.remote
                            .as_mut()
                            .unwrap()
                            .exclusive_reads
                            .remove(document);
                    }
                    Some(*document)
                }
                Operation::Git { document, .. } => Some(*document),
                Operation::GitStatus => None,
            };
            if result.is_err() {
                let origin = match &operation {
                    Operation::Read {
                        document,
                        generation,
                        ..
                    }
                    | Operation::Save {
                        document,
                        generation,
                        ..
                    }
                    | Operation::Git {
                        document,
                        generation,
                    } => Some((*document, *generation)),
                    _ => None,
                };
                if origin.is_some_and(|(id, generation)| {
                    self.documents
                        .get(&id)
                        .is_none_or(|entry| entry.editor.document_generation() != generation)
                }) {
                    continue;
                }
            }
            let response = match result {
                Ok(response) => response,
                Err(error) => {
                    if matches!(
                        error.kind(),
                        io::ErrorKind::BrokenPipe
                            | io::ErrorKind::UnexpectedEof
                            | io::ErrorKind::NotConnected
                            | io::ErrorKind::TimedOut
                    ) {
                        self.remote.as_mut().unwrap().connected = false;
                    }
                    if let Some(id) = document
                        && let Some(entry) = self.documents.get_mut(&id)
                        && matches!(
                            error.kind(),
                            io::ErrorKind::WouldBlock | io::ErrorKind::NotFound
                        )
                    {
                        if matches!(
                            operation,
                            Operation::Read {
                                mode: ReadMode::Keep,
                                ..
                            }
                        ) && error.kind() == io::ErrorKind::NotFound
                        {
                            self.remote.as_mut().unwrap().baselines.remove(&id);
                            entry.editor.disk_conflict = None;
                            entry.removed = false;
                            entry.allow_recreate = true;
                            entry.editor.save_service.on_did_edit(&entry.editor.state);
                            continue;
                        }
                        entry.editor.disk_conflict = Some(error.to_string());
                        entry.editor.save_service.cancel_pending();
                        entry.removed = error.kind() == io::ErrorKind::NotFound;
                        self.queued.push(SessionEvent::Conflict {
                            document: id,
                            message: error.to_string(),
                        });
                    }
                    self.record_error(document, "remote", &error);
                    continue;
                }
            };
            match (operation, response) {
                (
                    Operation::Open { requested, kind },
                    Response::File {
                        path,
                        bytes,
                        baseline,
                    },
                ) => {
                    if let Some(&id) = self.paths.get(Path::new(&path)) {
                        if kind.is_some_and(|kind| kind != self.documents[&id].editor.state.kind) {
                            self.record_error(None, "remote", &document_kind_conflict());
                            continue;
                        }
                        self.remote.as_mut().unwrap().aliases.insert(requested, id);
                        self.queued.push(SessionEvent::Opened { document: id });
                        continue;
                    }
                    let kind = kind.unwrap_or_else(|| bed_files::files::classify_bytes(&bytes));
                    if kind == DocumentKind::Text
                        && let Err(error) = bed_files::files::validate_text_bytes(&bytes)
                    {
                        self.record_error(None, "remote", &error);
                        continue;
                    }
                    let id = DocumentId::next();
                    let mut editor = Editor::new();
                    if kind == DocumentKind::Text {
                        editor.bind_project_undo(Rc::clone(&self.history));
                    }
                    editor.set_git_changed_lines(self.options.git && kind == DocumentKind::Text);
                    editor.api().open_document_with_kind(&path, &bytes, kind);
                    self.paths.insert(PathBuf::from(path), id);
                    self.remote.as_mut().unwrap().baselines.insert(id, baseline);
                    self.remote.as_mut().unwrap().aliases.insert(requested, id);
                    self.install_entry(id, editor, FileMonitor::new());
                    self.queue_remote_git(id);
                }
                (
                    Operation::Save {
                        document: id,
                        path,
                        generation,
                        version,
                    },
                    Response::Written { baseline },
                ) => {
                    let Some(entry) = self.documents.get_mut(&id) else {
                        continue;
                    };
                    if entry.editor.state.kind == DocumentKind::Bytes {
                        self.history.borrow_mut().forget_existing_file(&path);
                    }
                    if entry.editor.document_generation() != generation {
                        if entry.editor.state.path == path {
                            self.remote.as_mut().unwrap().baselines.insert(id, baseline);
                            entry.editor.save_service.on_did_edit(&entry.editor.state);
                            self.queued.push(SessionEvent::Saved {
                                document: id,
                                save: DidSave { path, version },
                            });
                        }
                        continue;
                    }
                    let previous = entry.editor.state.path.clone();
                    if previous != path {
                        self.remote
                            .as_mut()
                            .unwrap()
                            .aliases
                            .retain(|_, doc| *doc != id);
                        let old_key = entry.editor.history_key().to_owned();
                        if entry.editor.state.kind == DocumentKind::Text
                            && let Err(error) =
                                self.history.borrow_mut().rekey_file(&old_key, &path)
                        {
                            self.errors.push(ServiceError {
                                document: Some(id),
                                service: "history",
                                message: error.to_string(),
                            });
                        }
                        entry.editor.set_history_key(None);
                        entry.editor.rebind_document_path(&path);
                        self.paths.retain(|_, doc| *doc != id);
                        self.paths.insert(PathBuf::from(&path), id);
                        self.queued.push(SessionEvent::PathChanged {
                            document: id,
                            previous,
                            path: path.clone(),
                        });
                    }
                    self.remote.as_mut().unwrap().baselines.insert(id, baseline);
                    entry.editor.disk_conflict = None;
                    entry.removed = false;
                    entry.allow_recreate = false;
                    let saved_current_version = entry.editor.state.version == version;
                    if saved_current_version {
                        entry.editor.state.mark_saved();
                    } else {
                        entry.editor.save_service.on_did_edit(&entry.editor.state);
                    }
                    let save = DidSave { path, version };
                    if saved_current_version {
                        entry
                            .editor
                            .events
                            .emit_did_save_document(&save, &entry.editor.state);
                    } else {
                        self.queued.push(SessionEvent::Saved { document: id, save });
                    }
                    self.collect_notifications(id, None);
                    if self.documents[&id].editor.document_generation() != generation {
                        if let Some(pool) = &mut self.lsp {
                            let _ = pool.unregister_document(id);
                        }
                        self.attach_lsp(id);
                    } else {
                        self.update_lsp_snapshot(id);
                    }
                    self.queue_remote_git(id);
                }
                (
                    Operation::Read {
                        document: id,
                        generation,
                        version,
                        mode,
                    },
                    Response::File {
                        bytes, baseline, ..
                    },
                ) => {
                    let Some(entry) = self.documents.get_mut(&id) else {
                        continue;
                    };
                    if entry.editor.document_generation() != generation {
                        continue;
                    }
                    if matches!(mode, ReadMode::Monitor)
                        && self.remote.as_ref().unwrap().saving.contains(&id)
                    {
                        continue;
                    }
                    let changed =
                        self.remote
                            .as_ref()
                            .unwrap()
                            .baselines
                            .get(&id)
                            .is_none_or(|old| {
                                old.fingerprint != baseline.fingerprint || old.len != baseline.len
                            });
                    if matches!(mode, ReadMode::Keep) {
                        self.remote.as_mut().unwrap().baselines.insert(id, baseline);
                        entry.editor.disk_conflict = None;
                        entry.removed = false;
                        entry.editor.save_service.on_did_edit(&entry.editor.state);
                    } else if matches!(mode, ReadMode::Reload) || changed {
                        if entry.editor.state.version != version
                            || (entry.editor.state.dirty && matches!(mode, ReadMode::Monitor))
                        {
                            let message =
                                "File changed remotely; reload or keep your buffer before saving"
                                    .to_owned();
                            entry.editor.disk_conflict = Some(message.clone());
                            entry.editor.save_service.cancel_pending();
                            self.queued.push(SessionEvent::Conflict {
                                document: id,
                                message,
                            });
                        } else {
                            if entry.editor.state.kind == DocumentKind::Text
                                && let Err(error) = bed_files::files::validate_text_bytes(&bytes)
                            {
                                entry.editor.disk_conflict = Some(error.to_string());
                                entry.editor.save_service.cancel_pending();
                                self.record_error(Some(id), "remote", &error);
                                continue;
                            }
                            if entry.editor.state.kind == DocumentKind::Bytes
                                && !entry.editor.state.bytes_equal(&bytes)
                            {
                                self.history
                                    .borrow_mut()
                                    .forget_existing_file(&entry.editor.state.path);
                            }
                            self.remote.as_mut().unwrap().baselines.insert(id, baseline);
                            entry.editor.set_content(&bytes);
                            entry.byte_history = ByteHistory::default();
                            entry.byte_splices.clear();
                            entry.highlight_edits.clear();
                            entry.removed = false;
                            let key = entry.editor.history_key().to_owned();
                            entry.editor.project_undo().forget_file(&key);
                            for view in entry.views.values_mut() {
                                view.state.clamp_all(&entry.editor.state);
                            }
                            let generation = entry.editor.document_generation();
                            if let Some(pool) = &mut self.lsp {
                                let _ = pool.unregister_document(id);
                            }
                            self.attach_lsp(id);
                            self.queued.push(SessionEvent::Reloaded {
                                document: id,
                                generation,
                            });
                            self.queue_remote_git(id);
                        }
                    } else {
                        // A metadata-only change must not replace the buffer or clear undo.
                        self.remote.as_mut().unwrap().baselines.insert(id, baseline);
                    }
                }
                (
                    Operation::Git {
                        document: id,
                        generation,
                    },
                    Response::GitBaseline { bytes },
                ) => {
                    if let Some(entry) = self.documents.get_mut(&id)
                        && entry.editor.document_generation() == generation
                    {
                        entry.editor.git.borrow_mut().install_remote_baseline(
                            &entry.editor.state,
                            bytes.as_deref(),
                            self.options.git,
                        );
                    }
                }
                (Operation::GitStatus, Response::GitStatus { entries }) => {
                    self.remote.as_mut().unwrap().modified_paths =
                        entries.into_iter().map(|entry| entry.path).collect();
                }
                _ => self.record_error(
                    document,
                    "remote",
                    &io::Error::new(io::ErrorKind::InvalidData, "Unexpected remote response"),
                ),
            }
        }
        if self.options.git
            && self.remote_connected()
            && self
                .remote
                .as_ref()
                .unwrap()
                .last_git
                .is_none_or(|last| last.elapsed() >= Duration::from_secs(3))
        {
            let remote = self.remote.as_mut().unwrap();
            remote.last_git = Some(Instant::now());
            let _ = remote.queue(
                Operation::GitStatus,
                Request::GitStatus {
                    root: remote.root.clone(),
                },
            );
            for id in self.document_ids() {
                self.queue_remote_git(id);
            }
        }
    }
    pub(super) fn queue_remote_git(&mut self, document: DocumentId) {
        if !self.options.git || self.documents[&document].editor.state.kind == DocumentKind::Bytes {
            return;
        }
        let entry = &self.documents[&document];
        let remote = self.remote.as_ref().unwrap();
        let _ = remote.queue(
            Operation::Git {
                document,
                generation: entry.editor.document_generation(),
            },
            Request::GitBaseline {
                root: remote.root.clone(),
                path: entry.editor.state.path.clone(),
            },
        );
    }
}
