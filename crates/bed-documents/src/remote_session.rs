//! Connection-scoped remote I/O around the unchanged local document engine.
use super::*;
use bed_remote::{FileBaseline, RemoteClient, Request, Response, SshTarget};
use std::{collections::BTreeSet, sync::mpsc, thread};

enum Operation {
    #[cfg(feature = "ide-services")]
    WorkspaceCheck {
        document: DocumentId,
        baseline: FileBaseline,
    },
    Open {
        requested: String,
        kind: Option<DocumentKind>,
        origin: Option<OpenOrigin>,
    },
    Save {
        document: DocumentId,
        path: String,
        generation: u64,
        version: i32,
        previous: Option<Box<(String, Option<FileBaseline>)>>,
    },
    Read {
        document: DocumentId,
        generation: u64,
        version: i32,
        mode: ReadMode,
    },
    #[cfg(feature = "ide-services")]
    Git {
        document: DocumentId,
        generation: u64,
        comparison_epoch: u64,
    },
    #[cfg(feature = "ide-services")]
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
    open_errors: BTreeMap<String, io::Error>,
    aliases: BTreeMap<String, DocumentId>,
    modified_paths: BTreeSet<String>,
    #[cfg(feature = "ide-services")]
    last_git: Option<Instant>,
    #[cfg(feature = "ide-services")]
    pub(super) comparison_epoch: u64,
    connected: bool,
    #[cfg(feature = "ide-services")]
    workspace_checks: BTreeMap<DocumentId, Option<Result<(), String>>>,
}

impl RemoteSession {
    fn new(target: SshTarget, client: RemoteClient, root: String) -> Self {
        let (requests, pending) = mpsc::sync_channel::<(Operation, Request)>(128);
        let (results, completed) = mpsc::channel();
        let transport = bed_files::FileServices::remote_target(target.clone(), client.clone());
        thread::spawn(move || {
            while let Ok((mut operation, mut request)) = pending.recv() {
                let result = (|| {
                    #[cfg(feature = "ide-services")]
                    if let (Operation::WorkspaceCheck { .. }, Request::ReadFile { root, path }) =
                        (&operation, &request)
                    {
                        match transport.call(Request::FileInfo {
                            root: root.clone(),
                            path: path.clone(),
                        })? {
                            Response::FileInfo { info } if !info.readonly && !info.is_directory => {
                            }
                            Response::FileInfo { .. } => {
                                return Err(io::Error::new(
                                    io::ErrorKind::PermissionDenied,
                                    format!("{} is not a writable text file", path),
                                ));
                            }
                            _ => {
                                return Err(io::Error::new(
                                    io::ErrorKind::InvalidData,
                                    "Unexpected workspace permission response",
                                ));
                            }
                        }
                    }
                    if let (
                        Operation::Save { path, previous, .. },
                        Request::WriteFile {
                            root,
                            path: destination,
                            baseline,
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
                                if let Some(previous) = previous
                                    && canonical == previous.0
                                {
                                    *baseline = previous.1.clone();
                                }
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
            open_errors: BTreeMap::new(),
            aliases: BTreeMap::new(),
            modified_paths: BTreeSet::new(),
            #[cfg(feature = "ide-services")]
            last_git: None,
            #[cfg(feature = "ide-services")]
            comparison_epoch: 0,
            connected: true,
            #[cfg(feature = "ide-services")]
            workspace_checks: BTreeMap::new(),
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

impl DocumentSession {
    #[cfg(feature = "ide-services")]
    pub(super) fn remote_workspace_check(&mut self, document: DocumentId) -> io::Result<bool> {
        let path = self.entry(document)?.runtime.state.path.clone();
        let remote = self.remote.as_mut().unwrap();
        if let Some(result) = remote.workspace_checks.get(&document) {
            return match result {
                None => Ok(false),
                Some(Ok(())) => {
                    remote.workspace_checks.remove(&document);
                    Ok(true)
                }
                Some(Err(message)) => {
                    let message = message.clone();
                    remote.workspace_checks.remove(&document);
                    Err(io::Error::new(io::ErrorKind::InvalidInput, message))
                }
            };
        }
        if remote.saving.contains(&document) || remote.exclusive_reads.contains(&document) {
            return Ok(false);
        }
        let baseline = remote
            .baselines
            .get(&document)
            .cloned()
            .ok_or_else(|| io::Error::other("Remote document has no disk baseline"))?;
        remote.queue(
            Operation::WorkspaceCheck { document, baseline },
            Request::ReadFile {
                root: remote.root.clone(),
                path,
            },
        )?;
        remote.workspace_checks.insert(document, None);
        Ok(false)
    }

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
        options.history_path = None;
        let mut session = Self::default();
        session.options = options;
        session.services = bed_files::FileServices::remote_target(target.clone(), client.clone());
        session.remote = Some(RemoteSession::new(target, client, root));
        #[cfg(feature = "ide-services")]
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

    /// Consume the result for this exact asynchronous open, without guessing
    /// from unrelated service errors. A new admission clears the prior result.
    pub fn take_open_file_error(&mut self, path: &Path) -> Option<io::Error> {
        self.remote.as_mut()?.open_errors.remove(path.to_str()?)
    }

    fn record_open_file_error(&mut self, path: &str, error: &io::Error) {
        self.remote
            .as_mut()
            .unwrap()
            .open_errors
            .insert(path.into(), io::Error::new(error.kind(), error.to_string()));
        self.record_error(None, "remote", error);
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
        self.services =
            bed_files::FileServices::remote_target(remote.target.clone(), remote.client.clone());
        self.remote = Some(remote);
        #[cfg(feature = "ide-services")]
        {
            for entry in self.documents.values_mut() {
                entry.runtime.bind_lsp_client(None);
            }
            if let Some(pool) = &mut self.lsp {
                pool.shutdown();
            }
            self.lsp = None;
            #[cfg(feature = "ide-services")]
            self.initialize_lsp();
            for id in self.document_ids() {
                self.attach_lsp(id, None);
            }
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
        self.request_open_mode(path, Some(kind), None)
    }

    pub fn request_open_file_auto(&mut self, path: &Path) -> io::Result<Option<DocumentId>> {
        self.request_open_mode(path, None, None)
    }

    pub(super) fn request_open_mode(
        &mut self,
        path: &Path,
        kind: Option<DocumentKind>,
        origin: Option<OpenOrigin>,
    ) -> io::Result<Option<DocumentId>> {
        self.ensure_running()?;
        if self.remote.is_none() {
            return match kind {
                Some(kind) => self.open_file_in_context(path, kind, origin),
                None => self.open_file_auto(path),
            }
            .map(Some);
        }
        if let Some(id) = self.document_for_path(path) {
            if kind.is_some_and(|kind| self.documents[&id].runtime.state.kind != kind) {
                return Err(document_kind_conflict());
            }
            return Ok(Some(id));
        }
        let path = path_string(path)?.to_owned();
        let remote = self.remote.as_mut().unwrap();
        if remote.opening.contains(&path) {
            return Ok(None);
        }
        remote.open_errors.remove(&path);
        remote.queue(
            Operation::Open {
                requested: path.clone(),
                kind,
                origin,
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
        let destination_requested = destination.is_some();
        let original_path = entry.runtime.state.path.clone();
        if destination.is_none()
            && (!entry.runtime.state.dirty || entry.runtime.state.path.is_empty())
        {
            return Ok(false);
        }
        if destination.is_none() && entry.removed {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "File was removed; use Save As before saving",
            ));
        }
        if destination.is_none() && entry.runtime.disk_conflict.is_some() {
            return Err(io::Error::other(
                "File changed on disk; reload or keep the buffer before saving",
            ));
        }
        let path = destination.unwrap_or_else(|| entry.runtime.state.path.clone());
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
        let bytes = crate::save_service::DocumentSave::bytes_for_save(&entry.runtime.state)?;
        let generation = entry.runtime.document_generation();
        let version = entry.runtime.state.version;
        let same_path = entry.runtime.state.path == path;
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
        let previous = destination_requested
            .then(|| Box::new((original_path, remote.baselines.get(&document).cloned())));
        remote.queue(
            Operation::Save {
                document,
                path: path.clone(),
                generation,
                version,
                previous,
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
            .runtime
            .save_service
            .cancel_pending();
        Ok(false)
    }

    fn queue_read(&mut self, document: DocumentId, mode: ReadMode) -> io::Result<()> {
        let entry = self.entry(document)?;
        let path = entry.runtime.state.path.clone();
        let generation = entry.runtime.document_generation();
        let version = entry.runtime.state.version;
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

    pub(super) fn queue_remote_monitor(&mut self, document: DocumentId) -> io::Result<()> {
        self.queue_read(document, ReadMode::Monitor)
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
        if entry.removed || entry.runtime.state.path.is_empty() {
            return;
        }
        if let Some(since) = entry.removal_pending_since {
            if since.elapsed() >= Duration::from_millis(250)
                && let Err(error) = self.queue_read(id, ReadMode::Monitor)
            {
                self.record_error(Some(id), "remote", &error);
            }
            return;
        }
        let autosave = self.options.autosave.is_some()
            && !entry.autosave_paused
            && entry.runtime.state.dirty
            && entry.runtime.disk_conflict.is_none()
            && entry.runtime.save_service.is_due();
        let result = if autosave {
            self.queue_remote_save(id, None).map(|_| ())
        } else if monitor {
            self.queue_read(id, ReadMode::Monitor)
        } else {
            Ok(())
        };
        if let Err(error) = result {
            self.record_error(Some(id), if autosave { "save" } else { "remote" }, &error);
        }
    }

    pub(super) fn configure_remote(&mut self, mut options: SessionOptions) -> io::Result<()> {
        if options.project_root != self.options.project_root {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Open a new session to change remote roots",
            ));
        }
        options.history_path = None;
        let start_autosave = options.autosave.is_some() && self.options.autosave.is_none();
        #[cfg(feature = "ide-services")]
        let lsp_changed = options.lsp_config != self.options.lsp_config
            || options.lsp_config_mode != self.options.lsp_config_mode
            || options.lsp_file_observations != self.options.lsp_file_observations;
        #[cfg(feature = "ide-services")]
        if lsp_changed {
            for entry in self.documents.values_mut() {
                entry.runtime.bind_lsp_client(None);
            }
            if let Some(pool) = &mut self.lsp {
                pool.shutdown();
            }
            self.lsp = None;
        }
        self.options = options;
        #[cfg(feature = "ide-services")]
        self.initialize_lsp();
        for id in self.document_ids() {
            let entry = self.documents.get_mut(&id).unwrap();
            #[cfg(feature = "ide-services")]
            {
                entry.runtime.highlight.set_enabled(
                    self.options.highlighting && entry.runtime.state.kind == DocumentKind::Text,
                );
                entry.runtime.refresh_highlighting();
                entry.runtime.set_git_changed_lines(
                    self.options.git && entry.runtime.state.kind == DocumentKind::Text,
                );
            }
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
            if lsp_changed {
                self.attach_lsp(id, None);
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
            let operation_service = if matches!(&operation, Operation::Save { .. }) {
                "save"
            } else {
                "remote"
            };
            let open_path = match &operation {
                Operation::Open { requested, .. } => Some(requested.clone()),
                _ => None,
            };
            #[cfg(feature = "ide-services")]
            if let Operation::WorkspaceCheck {
                document,
                baseline: before,
            } = &operation
            {
                let result = match result {
                    Ok(Response::File { baseline, .. })
                        if baseline.fingerprint == before.fingerprint
                            && baseline.len == before.len =>
                    {
                        self.remote
                            .as_mut()
                            .unwrap()
                            .baselines
                            .insert(*document, baseline);
                        Ok(())
                    }
                    Ok(Response::File { path, .. }) => Err(format!(
                        "{} changed remotely; request the LSP action again",
                        path
                    )),
                    Ok(_) => Err("Unexpected workspace preflight response".to_owned()),
                    Err(error) => Err(error.to_string()),
                };
                self.remote
                    .as_mut()
                    .unwrap()
                    .workspace_checks
                    .insert(*document, Some(result));
                continue;
            }
            let document = match &operation {
                #[cfg(feature = "ide-services")]
                Operation::WorkspaceCheck { .. } => unreachable!(),
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
                #[cfg(feature = "ide-services")]
                Operation::Git { document, .. } => Some(*document),
                #[cfg(feature = "ide-services")]
                Operation::GitStatus => None,
            };
            #[cfg(feature = "ide-services")]
            if let Operation::Open {
                origin: Some(origin),
                ..
            } = &operation
                && !self.accepts_lsp_origin(origin)
            {
                // The request was valid when queued; an edit, view close
                // or server restart makes its delayed result obsolete.
                continue;
            }
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
                    } => Some((*document, *generation)),
                    #[cfg(feature = "ide-services")]
                    Operation::Git {
                        document,
                        generation,
                        ..
                    } => Some((*document, *generation)),
                    _ => None,
                };
                if origin.is_some_and(|(id, generation)| {
                    self.documents
                        .get(&id)
                        .is_none_or(|entry| entry.runtime.document_generation() != generation)
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
                    let removed_document = document.filter(|id| {
                        error.kind() == io::ErrorKind::NotFound
                            && self
                                .documents
                                .get(id)
                                .is_some_and(|entry| match &operation {
                                    Operation::Read { .. } => true,
                                    Operation::Save { path, .. } => {
                                        *path == entry.runtime.state.path
                                    }
                                    _ => false,
                                })
                    });
                    if let Some(id) = removed_document {
                        let monitor_read = matches!(
                            operation,
                            Operation::Read {
                                mode: ReadMode::Monitor,
                                ..
                            }
                        );
                        let entry = self.documents.get_mut(&id).unwrap();
                        if monitor_read
                            && !entry
                                .removal_pending_since
                                .is_some_and(|since| since.elapsed() >= Duration::from_millis(250))
                        {
                            entry.removal_pending_since.get_or_insert_with(Instant::now);
                            entry.runtime.save_service.cancel_pending();
                        } else {
                            let _ = self.invalidate_removed_path(id);
                        }
                        if operation_service == "save" {
                            self.record_error(Some(id), operation_service, &error);
                        }
                        continue;
                    }
                    if let Some(id) = document
                        && let Some(entry) = self.documents.get_mut(&id)
                        && matches!(
                            error.kind(),
                            io::ErrorKind::WouldBlock | io::ErrorKind::NotFound
                        )
                    {
                        entry.runtime.disk_conflict = Some(error.to_string());
                        entry.runtime.save_service.cancel_pending();
                        entry.removed = error.kind() == io::ErrorKind::NotFound;
                        self.queued.push(SessionEvent::Conflict {
                            document: id,
                            message: error.to_string(),
                        });
                    }
                    if let Some(path) = &open_path {
                        self.record_open_file_error(path, &error);
                    } else {
                        self.record_error(document, operation_service, &error);
                    }
                    continue;
                }
            };
            match (operation, response) {
                (
                    Operation::Open {
                        requested,
                        kind,
                        origin,
                    },
                    Response::File {
                        path,
                        bytes,
                        baseline,
                    },
                ) => {
                    if let Some(&id) = self.paths.get(Path::new(&path)) {
                        if kind.is_some_and(|kind| kind != self.documents[&id].runtime.state.kind) {
                            self.record_open_file_error(&requested, &document_kind_conflict());
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
                        self.record_open_file_error(&requested, &error);
                        continue;
                    }
                    let id = DocumentId::next();
                    let mut runtime = DocumentRuntime::new();
                    if kind == DocumentKind::Text {
                        runtime.bind_project_undo(Rc::clone(&self.history));
                    }
                    #[cfg(feature = "ide-services")]
                    runtime.set_git_changed_lines(self.options.git && kind == DocumentKind::Text);
                    runtime.api().open_document_with_kind(&path, &bytes, kind);
                    self.paths.insert(PathBuf::from(path), id);
                    self.remote.as_mut().unwrap().baselines.insert(id, baseline);
                    self.remote.as_mut().unwrap().aliases.insert(requested, id);
                    self.install_entry(id, runtime, FileMonitor::new(), origin);
                    #[cfg(feature = "ide-services")]
                    self.queue_remote_git(id);
                }
                (
                    Operation::Save {
                        document: id,
                        path,
                        generation,
                        version,
                        ..
                    },
                    Response::Written { baseline },
                ) => {
                    let Some(entry) = self.documents.get_mut(&id) else {
                        continue;
                    };
                    if entry.runtime.state.kind == DocumentKind::Bytes {
                        self.history.borrow_mut().forget_existing_file(&path);
                    }
                    if entry.runtime.document_generation() != generation {
                        if entry.runtime.state.path == path {
                            self.remote.as_mut().unwrap().baselines.insert(id, baseline);
                            entry.runtime.save_service.on_did_edit(&entry.runtime.state);
                            self.queued.push(SessionEvent::Saved {
                                document: id,
                                save: DidSave { path, version },
                            });
                        }
                        continue;
                    }
                    let previous = entry.runtime.state.path.clone();
                    if previous != path {
                        self.remote
                            .as_mut()
                            .unwrap()
                            .aliases
                            .retain(|_, doc| *doc != id);
                        let old_key = entry.runtime.history_key().to_owned();
                        if entry.runtime.state.kind == DocumentKind::Text
                            && let Err(error) =
                                self.history.borrow_mut().rekey_file(&old_key, &path)
                        {
                            self.errors.push(ServiceError {
                                document: Some(id),
                                service: "history",
                                message: error.to_string(),
                            });
                        }
                        entry.runtime.set_history_key(None);
                        entry.runtime.rebind_document_path(&path);
                        self.paths.retain(|_, doc| *doc != id);
                        self.paths.insert(PathBuf::from(&path), id);
                        self.queued.push(SessionEvent::PathChanged {
                            document: id,
                            previous,
                            path: path.clone(),
                        });
                    }
                    self.remote.as_mut().unwrap().baselines.insert(id, baseline);
                    entry.runtime.disk_conflict = None;
                    entry.removed = false;
                    entry.original_path = None;
                    entry.removal_pending_since = None;
                    entry.allow_recreate = false;
                    let saved_current_version = entry.runtime.state.version == version;
                    if saved_current_version {
                        entry.runtime.state.mark_saved();
                    } else {
                        entry.runtime.save_service.on_did_edit(&entry.runtime.state);
                    }
                    let save = DidSave { path, version };
                    if saved_current_version {
                        entry
                            .runtime
                            .events
                            .emit_did_save_document(&save, &entry.runtime.state);
                    } else {
                        self.queued.push(SessionEvent::Saved { document: id, save });
                    }
                    self.collect_notifications(id, None);
                    #[cfg(feature = "ide-services")]
                    if self.documents[&id].runtime.document_generation() != generation {
                        self.reopen_lsp(id);
                    } else {
                        self.update_lsp_snapshot(id);
                    }
                    #[cfg(feature = "ide-services")]
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
                    if entry.runtime.document_generation() != generation {
                        continue;
                    }
                    if matches!(mode, ReadMode::Monitor)
                        && self.remote.as_ref().unwrap().saving.contains(&id)
                    {
                        continue;
                    }
                    if entry.removal_pending_since.take().is_some() {
                        entry.runtime.save_service.on_did_edit(&entry.runtime.state);
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
                        entry.runtime.disk_conflict = None;
                        entry.removed = false;
                        entry.runtime.save_service.on_did_edit(&entry.runtime.state);
                    } else if matches!(mode, ReadMode::Reload) || changed {
                        if entry.runtime.state.version != version
                            || (entry.runtime.state.dirty && matches!(mode, ReadMode::Monitor))
                        {
                            let message =
                                "File changed remotely; reload or keep your buffer before saving"
                                    .to_owned();
                            entry.runtime.disk_conflict = Some(message.clone());
                            entry.runtime.save_service.cancel_pending();
                            self.queued.push(SessionEvent::Conflict {
                                document: id,
                                message,
                            });
                        } else {
                            if entry.runtime.state.kind == DocumentKind::Text
                                && let Err(error) = bed_files::files::validate_text_bytes(&bytes)
                            {
                                entry.runtime.disk_conflict = Some(error.to_string());
                                entry.runtime.save_service.cancel_pending();
                                self.record_error(Some(id), "remote", &error);
                                continue;
                            }
                            if entry.runtime.state.kind == DocumentKind::Bytes
                                && !entry.runtime.state.bytes_equal(&bytes)
                            {
                                self.history
                                    .borrow_mut()
                                    .forget_existing_file(&entry.runtime.state.path);
                            }
                            self.remote.as_mut().unwrap().baselines.insert(id, baseline);
                            entry.runtime.set_content(&bytes);
                            entry.byte_history = ByteHistory::default();
                            entry.byte_splices.clear();
                            entry.highlight_edits.clear();
                            entry.removed = false;
                            let key = entry.runtime.history_key().to_owned();
                            entry.runtime.project_undo().forget_file(&key);
                            for view in entry.views.values_mut() {
                                view.state.folds.set_ranges(Vec::new());
                                view.state.clamp_all(&entry.runtime.state);
                            }
                            let generation = entry.runtime.document_generation();
                            #[cfg(feature = "ide-services")]
                            self.reopen_lsp(id);
                            self.queued.push(SessionEvent::Reloaded {
                                document: id,
                                generation,
                            });
                            #[cfg(feature = "ide-services")]
                            self.queue_remote_git(id);
                        }
                    } else {
                        // A metadata-only change must not replace the buffer or clear undo.
                        self.remote.as_mut().unwrap().baselines.insert(id, baseline);
                    }
                }
                #[cfg(feature = "ide-services")]
                (
                    Operation::Git {
                        document: id,
                        generation,
                        comparison_epoch,
                    },
                    Response::GitBaseline { bytes },
                ) => {
                    #[cfg(feature = "ide-services")]
                    if self
                        .remote
                        .as_ref()
                        .is_some_and(|remote| remote.comparison_epoch == comparison_epoch)
                        && let Some(entry) = self.documents.get_mut(&id)
                        && entry.runtime.document_generation() == generation
                    {
                        entry.runtime.git.borrow_mut().install_remote_baseline(
                            &entry.runtime.state,
                            bytes.as_deref(),
                            self.options.git,
                        );
                    }
                }
                #[cfg(feature = "ide-services")]
                (Operation::GitStatus, Response::GitStatus { entries }) => {
                    self.remote.as_mut().unwrap().modified_paths =
                        entries.into_iter().map(|entry| entry.path).collect();
                }
                _ => {
                    let error =
                        io::Error::new(io::ErrorKind::InvalidData, "Unexpected remote response");
                    if let Some(path) = open_path {
                        self.record_open_file_error(&path, &error);
                    } else {
                        self.record_error(document, operation_service, &error);
                    }
                }
            }
        }
        #[cfg(feature = "ide-services")]
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

    #[cfg(feature = "ide-services")]
    pub(super) fn queue_remote_git(&mut self, document: DocumentId) {
        if !self.options.git || self.documents[&document].runtime.state.kind == DocumentKind::Bytes
        {
            return;
        }
        let entry = &self.documents[&document];
        let remote = self.remote.as_ref().unwrap();
        let _ = remote.queue(
            Operation::Git {
                document,
                generation: entry.runtime.document_generation(),
                comparison_epoch: remote.comparison_epoch,
            },
            Request::GitBaseline {
                root: remote.root.clone(),
                path: entry.runtime.state.path.clone(),
            },
        );
    }
}
