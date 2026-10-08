//! Standalone SSH workspace orchestration. Pipe I/O stays off the ImGui thread.
use super::*;
use bed_editing::util::utf8::utf16_to_utf8_byte_offset;
use bed_remote::{RemoteClient, Request, Response, SshTarget};
use std::{sync::mpsc, thread};

fn remote_join(directory: &str, name: &str) -> String {
    format!("{}/{name}", directory.trim_end_matches('/'))
}
pub(super) fn remote_rename_target(source: &str, name: &str) -> io::Result<String> {
    if !source.starts_with('/') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Remote path must be absolute",
        ));
    }
    let (parent, filename) = source.rsplit_once('/').ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "Remote path must be absolute")
    })?;
    if filename.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Choose a file or folder inside the remote project",
        ));
    }
    Ok(remote_join(parent, name))
}
fn remote_contains(parent: &str, path: &str) -> bool {
    path == parent
        || path
            .strip_prefix(parent.trim_end_matches('/'))
            .is_some_and(|suffix| suffix.starts_with('/'))
}
fn remote_rebound_path(source: &str, target: &str, path: &str) -> io::Result<String> {
    if path == source {
        return Ok(target.to_owned());
    }
    let suffix = path
        .strip_prefix(source)
        .and_then(|suffix| suffix.strip_prefix('/'))
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "Document is outside the renamed remote path",
            )
        })?;
    Ok(remote_join(target, suffix))
}

struct ConnectionResult {
    reconnect: bool,
    result: io::Result<ConnectedWorkspace>,
}
struct ConnectedWorkspace {
    spec: WorkspaceSpec,
    target: SshTarget,
    client: RemoteClient,
}
struct RemoteIo {
    requests: mpsc::SyncSender<(Operation, Request)>,
    completed: mpsc::Receiver<(Operation, io::Result<Response>)>,
    pending_directories: HashSet<String>,
    loaded_directories: HashSet<String>,
}
#[derive(Clone)]
enum Operation {
    Directory(String),
    CreateFile(String),
    CreateDirectory,
    Rename {
        source: String,
        target: String,
        documents: Vec<DocumentId>,
    },
    Remove {
        documents: Vec<DocumentId>,
    },
}
impl Operation {
    fn documents(&self) -> &[DocumentId] {
        match self {
            Self::Rename { documents, .. } | Self::Remove { documents } => documents,
            _ => &[],
        }
    }
}
impl RemoteIo {
    fn new(client: RemoteClient) -> Self {
        let (requests, receiver) = mpsc::sync_channel::<(Operation, Request)>(128);
        let (results, completed) = mpsc::channel();
        thread::spawn(move || {
            while let Ok((operation, request)) = receiver.recv() {
                let result = client.call(request);
                if results.send((operation, result)).is_err() {
                    break;
                }
            }
        });
        Self {
            requests,
            completed,
            pending_directories: HashSet::new(),
            loaded_directories: HashSet::new(),
        }
    }
    fn queue(&self, operation: Operation, request: Request) -> io::Result<()> {
        self.requests
            .try_send((operation, request))
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "Remote file operation queue is full",
                ),
                mpsc::TrySendError::Disconnected(_) => {
                    io::Error::new(io::ErrorKind::NotConnected, "Remote file worker stopped")
                }
            })
    }
}
struct PathDialog {
    document: Option<DocumentId>,
    path: String,
    appearing: bool,
    error: Option<String>,
}
#[derive(Default)]
pub(super) struct RemoteUi {
    connection: Option<mpsc::Receiver<ConnectionResult>>,
    ready: Option<ConnectionResult>,
    io: Option<RemoteIo>,
    path_dialog: Option<PathDialog>,
    pub(super) dialog_viewer: Option<String>,
    mutation_documents: HashSet<DocumentId>,
    mutation_pending: bool,
    navigation: HashMap<String, (i32, i32, bool)>,
    pub(super) opening: HashSet<String>,
    pub(super) restore: HashMap<String, Vec<Value>>,
    pub(super) pending_local: Option<WorkspaceSpec>,
    pub(super) restored_active: Option<u64>,
    pub(super) restored_focus: Option<u64>,
    pub(super) restore_order: Vec<u64>,
}
impl RemoteUi {
    pub(super) fn cancel_connection(&mut self) {
        self.connection = None;
        self.ready = None;
    }
    pub(super) fn connecting(&self) -> bool {
        self.connection.is_some() || self.ready.is_some()
    }
    pub(super) fn path_dialog_pending(&self) -> bool {
        self.path_dialog.is_some()
    }
    pub(super) fn mutation_pending(&self) -> bool {
        self.mutation_pending
    }
    pub(super) fn document_mutating(&self, id: DocumentId) -> bool {
        self.mutation_documents.contains(&id)
    }
}
impl Workbench {
    /// Opening an SSH workspace starts a background connection and validation.
    /// Existing documents remain available until validation and close preflight succeed.
    pub fn set_workspace(&mut self, spec: WorkspaceSpec) -> io::Result<bool> {
        if self.defer_workspace_switch(&spec) {
            return Ok(true);
        }
        if matches!(spec.target, WorkspaceTarget::Local) {
            return self.set_local_workspace(spec);
        }
        if self.workspace_spec.as_ref().is_some_and(|active| {
            active.identity() == spec.identity() && active.target == spec.target
        }) {
            if self.session.remote_connected() {
                let mut spec = spec;
                if spec.name.trim().is_empty() {
                    spec.name = self.workspace_spec.as_ref().unwrap().name.clone();
                }
                if let Some(store) = &mut self.store {
                    self.workspace_spec = Some(store.record_workspace(spec)?);
                }
                self.refresh_projects();
                return Ok(false);
            }
            return self.reconnect_workspace();
        }
        self.start_remote_connection(spec, false)?;
        Ok(true)
    }
    pub fn reconnect_workspace(&mut self) -> io::Result<bool> {
        let spec = self
            .workspace_spec
            .clone()
            .ok_or_else(|| io::Error::other("No active SSH workspace"))?;
        if !matches!(spec.target, WorkspaceTarget::Ssh { .. }) {
            return Ok(false);
        }
        self.start_remote_connection(spec, true)?;
        Ok(true)
    }
    fn start_remote_connection(&mut self, spec: WorkspaceSpec, reconnect: bool) -> io::Result<()> {
        let WorkspaceTarget::Ssh { host } = &spec.target else {
            return Ok(());
        };
        if !(spec.root.starts_with('/') || spec.root == "~" || spec.root.starts_with("~/"))
            || spec.root.contains('\0')
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Enter an absolute remote project path or a path beginning with ~/",
            ));
        }
        let target = SshTarget::new(host.clone());
        let (sender, receiver) = mpsc::channel();
        thread::Builder::new()
            .name("bed-ssh-connect".into())
            .spawn(move || {
                let result = (|| {
                    let root = bed_remote::expand_ssh_path(&target, &spec.root)?;
                    let target = bed_remote::prepare_ssh_target(
                        &target,
                        &crate::workspace::remote_helpers::helpers_directory(),
                    )?;
                    let client = RemoteClient::launch_ssh(&target)?;
                    let Response::Path { path } = client.call(Request::Canonicalize {
                        root,
                        path: ".".into(),
                        allow_missing: false,
                    })?
                    else {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "Unexpected remote project validation response",
                        ));
                    };
                    // Validate that the canonical root is a readable directory before switching.
                    if !matches!(
                        client.call(Request::ReadDirectory {
                            root: path.clone(),
                            path: path.clone(),
                            classify_gitignored: false,
                        })?,
                        Response::Directory { .. }
                    ) {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "Remote project is not a directory",
                        ));
                    }
                    let mut spec = spec;
                    spec.root = path;
                    Ok(ConnectedWorkspace {
                        spec,
                        target,
                        client,
                    })
                })();
                let _ = sender.send(ConnectionResult { reconnect, result });
            })?;
        self.remote_ui.pending_local = None;
        self.remote_ui.connection = Some(receiver);
        self.remote_ui.ready = None;
        self.refresh_projects();
        Ok(())
    }
    pub(super) fn poll_remote_workspace(&mut self) -> io::Result<()> {
        if let Some(spec) = self.remote_ui.pending_local.take()
            && let Err(error) = self.set_local_workspace(spec)
        {
            self.error = Some(error.to_string());
        }
        if let Some(receiver) = &self.remote_ui.connection {
            match receiver.try_recv() {
                Ok(result) => {
                    self.remote_ui.ready = Some(result);
                    self.remote_ui.connection = None;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.remote_ui.connection = None;
                    self.error = Some("SSH connection worker stopped".into());
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if let Some(ready) = self.remote_ui.ready.take() {
            match ready.result {
                Err(error) => self.error = Some(format!("SSH connection failed: {error}")),
                Ok(ConnectedWorkspace {
                    spec,
                    target,
                    client,
                }) if ready.reconnect => {
                    if self
                        .workspace_spec
                        .as_ref()
                        .is_some_and(|active| active.identity() == spec.identity())
                    {
                        self.finish_remote_reconnect(target, client)?;
                    }
                }
                Ok(ConnectedWorkspace {
                    mut spec,
                    target,
                    client,
                }) => {
                    if self
                        .workspace_spec
                        .as_ref()
                        .is_some_and(|active| active.identity() == spec.identity())
                    {
                        if self.session.remote_connected() {
                            client.disconnect();
                        } else {
                            self.finish_remote_reconnect(target, client)?;
                        }
                        if spec.name.trim().is_empty() {
                            spec.name = self.workspace_spec.as_ref().unwrap().name.clone();
                        }
                        if let Some(store) = &mut self.store {
                            spec = store.record_workspace(spec)?;
                        }
                        self.workspace_spec = Some(spec);
                        self.refresh_projects();
                        return Ok(());
                    }
                    match self.preflight_close(&(0..self.tabs.len()).collect::<Vec<_>>()) {
                        Ok(true) => self.activate_remote_workspace(spec, target, client)?,
                        Ok(false) => {
                            // Keep a validated connection while an acknowledged save is pending.
                            if self
                                .session
                                .document_ids()
                                .into_iter()
                                .any(|id| self.session.save_pending(id))
                                || self.remote_ui.mutation_pending()
                                || self.remote_ui.path_dialog_pending()
                            {
                                self.remote_ui.ready = Some(ConnectionResult {
                                    reconnect: false,
                                    result: Ok(ConnectedWorkspace {
                                        spec,
                                        target,
                                        client,
                                    }),
                                });
                            }
                        }
                        Err(error) => self.error = Some(error.to_string()),
                    }
                }
            }
        }
        let completions: Vec<_> = self
            .remote_ui
            .io
            .as_ref()
            .map(|io| io.completed.try_iter().collect())
            .unwrap_or_default();
        for (operation, result) in completions {
            if let Operation::Directory(path) = &operation
                && let Some(io) = &mut self.remote_ui.io
            {
                io.pending_directories.remove(path);
                io.loaded_directories.insert(path.clone());
            }
            let documents = operation.documents().to_vec();
            if !matches!(operation, Operation::Directory(_)) {
                self.remote_ui.mutation_pending = false;
            }
            let result = (|| match (operation, result) {
                (Operation::Directory(path), Ok(Response::Directory { entries, warning })) => {
                    self.modules.explorer.apply_directory(&path, entries);
                    if let Some(warning) = warning {
                        self.error = Some(warning);
                    }
                    Ok(())
                }
                (Operation::CreateFile(path), Ok(Response::Unit)) => {
                    self.open_or_focus(Path::new(&path))?;
                    self.refresh_remote_files()
                }
                (Operation::CreateDirectory, Ok(Response::Unit)) => self.refresh_remote_files(),
                (
                    Operation::Rename {
                        source,
                        target,
                        documents,
                    },
                    Ok(Response::Unit),
                ) => {
                    for id in documents {
                        if let Ok(snapshot) = self.session.snapshot(id) {
                            let destination =
                                remote_rebound_path(&source, &target, &snapshot.path)?;
                            self.session.rebind_path(id, Path::new(&destination))?;
                        }
                    }
                    self.refresh_remote_files()
                }
                (Operation::Remove { documents }, Ok(Response::Unit)) => {
                    for id in documents {
                        if self.session.snapshot(id).is_ok() {
                            self.session.invalidate_removed_path(id)?;
                        }
                    }
                    self.refresh_remote_files()
                }
                (_, Err(error)) => Err(error),
                _ => Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Unexpected remote file operation response",
                )),
            })();
            for id in documents {
                let _ = self.session.pause_autosave(id, false);
                self.remote_ui.mutation_documents.remove(&id);
            }
            if let Err(error) = result {
                self.error = Some(format!("Remote files: {error}"));
            }
        }
        self.refresh_projects();
        Ok(())
    }
    fn finish_remote_reconnect(
        &mut self,
        target: SshTarget,
        client: RemoteClient,
    ) -> io::Result<()> {
        self.session
            .reconnect_remote_with_target(client.clone(), target.clone())?;
        self.terminal.set_ssh_target(Some(target));
        for id in self.remote_ui.mutation_documents.drain() {
            let _ = self.session.pause_autosave(id, false);
        }
        self.remote_ui.mutation_pending = false;
        self.remote_ui.io = Some(RemoteIo::new(client.clone()));
        self.modules.explorer.set_remote_client(Some(client));
        self.modules
            .search
            .set_remote_client(self.session.remote_client());
        self.refresh_projects();
        self.queue_remote_directories(true)
    }

    pub(super) fn activate_remote_workspace(
        &mut self,
        mut spec: WorkspaceSpec,
        target: SshTarget,
        client: RemoteClient,
    ) -> io::Result<()> {
        if spec.name.trim().is_empty() {
            spec.name = self
                .store
                .as_ref()
                .and_then(|store| store.stored_spec(&spec))
                .map(|stored| stored.name)
                .unwrap_or_else(|| WorkspaceSpec::local(&spec.root).name);
        }
        let WorkspaceTarget::Ssh { host, .. } = &spec.target else {
            return Err(io::Error::other("Expected SSH workspace"));
        };
        if target.host != *host {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Resolved SSH target does not match the workspace host",
            ));
        }
        let session = EditorSession::with_remote_options(
            self.session_options(Some(PathBuf::from(&spec.root))),
            target.clone(),
            client.clone(),
            spec.root.clone(),
        )?;
        self.persist_workspace()?;
        self.shutdown_modules()?;
        self.close_plugin_panels()?;
        self.session.shutdown(ClosePolicy::Discard)?;
        self.terminal.shutdown();
        self.tabs.clear();
        self.pending_tab_close = None;
        self.active = None;
        self.last_document = None;
        self.focused = None;
        self.modules.editor_runtime.borrow_mut().cancel_requests();
        self.modules.search.cancel_all();
        self.modules = module_host::ModuleRuntime::from_composition((self.module_factory)());
        self.reset_workspace_layout();
        self.remote_ui = RemoteUi {
            io: Some(RemoteIo::new(client.clone())),
            ..RemoteUi::default()
        };
        self.session = session;
        self.project_root = spec.root.clone();
        self.workspace_spec = Some(spec.clone());
        self.restore_module_settings();
        self.service_settings = None;
        self.sync_services()?;
        {
            let mut explorer = self.modules.explorer.borrow_mut();
            explorer.project_root = spec.root.clone();
            explorer.file_tree = bed_module_explorer::file_tree::FileTree {
                root_node: bed_module_explorer::file_tree::FileNode {
                    name: spec.name.clone(),
                    full_path: spec.root.clone(),
                    is_directory: true,
                    is_open: true,
                    ..Default::default()
                },
                ..Default::default()
            };
        }
        self.restore_tree_preferences(&spec);
        self.modules.explorer.set_remote_client(Some(client));
        self.modules
            .search
            .set_remote_client(self.session.remote_client());
        self.terminal.set_ssh_target(Some(target));
        self.terminal.set_project_root(&spec.root);
        let restored = self
            .store
            .as_ref()
            .and_then(|store| store.layout(&spec))
            .cloned();
        if let Some(store) = &mut self.store {
            self.workspace_spec = Some(store.record_workspace(spec)?);
        }
        self.refresh_projects();
        if let Some(state) = restored {
            self.restore_workspace(&state)?;
        } else {
            self.show_tool(Tool::Explorer);
            if self.settings.terminal_visible {
                self.new_terminal();
            }
        }
        self.queue_remote_directories(true)?;
        self.scene += 1;
        Ok(())
    }
    pub(super) fn queue_remote_directories(&mut self, refresh: bool) -> io::Result<()> {
        if !self
            .session
            .remote_client()
            .is_some_and(|client| client.is_connected())
        {
            return Ok(());
        }
        let paths = self.modules.explorer.open_directories();
        let classify_gitignored = self
            .modules
            .explorer
            .borrow()
            .file_tree
            .preferences
            .hide_gitignored;
        let Some(io) = &mut self.remote_ui.io else {
            return Ok(());
        };
        for path in paths {
            if io.pending_directories.contains(&path)
                || (!refresh && io.loaded_directories.contains(&path))
            {
                continue;
            }
            io.queue(
                Operation::Directory(path.clone()),
                Request::ReadDirectory {
                    root: self.project_root.clone(),
                    path: path.clone(),
                    classify_gitignored,
                },
            )?;
            io.pending_directories.insert(path);
        }
        Ok(())
    }
    pub(super) fn refresh_remote_files(&mut self) -> io::Result<()> {
        self.queue_remote_directories(true)?;
        self.modules.explorer.refresh_finder();
        self.modules.search.cancel_all();
        Ok(())
    }
    pub(super) fn queue_remote_file_action(
        &mut self,
        action: &FileTreeAction,
        name: &str,
    ) -> io::Result<()> {
        if let FileTreeAction::Open(path) = action {
            self.open_or_focus(Path::new(path))?;
            return Ok(());
        }
        if self.remote_ui.mutation_pending {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Wait for the current remote file action",
            ));
        }
        if let FileTreeAction::Rename(source) | FileTreeAction::Trash(source) = action
            && self
                .remote_ui
                .opening
                .iter()
                .any(|path| remote_contains(source, path))
        {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Wait for files at this path to finish opening",
            ));
        }
        if !matches!(action, FileTreeAction::Trash(_)) {
            file_actions::validate_name(name)?;
        }
        let root = self.project_root.clone();
        let (operation, request) = match action {
            FileTreeAction::NewFile(directory) => {
                let path = remote_join(directory, name);
                (
                    Operation::CreateFile(path.clone()),
                    Request::CreateFile { root, path },
                )
            }
            FileTreeAction::NewFolder(directory) => {
                let path = remote_join(directory, name);
                (
                    Operation::CreateDirectory,
                    Request::CreateDirectory { root, path },
                )
            }
            FileTreeAction::Rename(source) => {
                let target = remote_rename_target(source, name)?;
                if target == *source {
                    return Ok(());
                }
                let documents = self
                    .affected_documents(Path::new(source))
                    .into_iter()
                    .map(|(id, _)| id)
                    .collect::<Vec<_>>();
                for id in &documents {
                    let snapshot = self.session.snapshot(*id)?;
                    let destination = remote_rebound_path(source, &target, &snapshot.path)?;
                    self.session
                        .check_rebind_path(*id, Path::new(&destination))?;
                }
                (
                    Operation::Rename {
                        source: source.clone(),
                        target: target.clone(),
                        documents,
                    },
                    Request::Rename {
                        root,
                        from: source.clone(),
                        to: target,
                    },
                )
            }
            FileTreeAction::Trash(path) => {
                let documents = self
                    .affected_documents(Path::new(path))
                    .into_iter()
                    .map(|(id, _)| id)
                    .collect();
                let is_directory = self
                    .modules
                    .explorer
                    .borrow()
                    .file_tree
                    .is_directory(path)
                    .unwrap_or(false);
                (
                    Operation::Remove { documents },
                    Request::Remove {
                        root,
                        path: path.clone(),
                        is_directory,
                    },
                )
            }
            _ => unreachable!("only file operations reach the remote mutation queue"),
        };
        for id in operation.documents() {
            if self.session.save_pending(*id) {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "Wait for the remote save before changing this path",
                ));
            }
        }
        self.remote_ui
            .io
            .as_ref()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "No remote file worker"))?
            .queue(operation.clone(), request)?;
        self.remote_ui.mutation_pending = true;
        for id in operation.documents() {
            self.session.pause_autosave(*id, true)?;
            self.remote_ui.mutation_documents.insert(*id);
        }
        Ok(())
    }
    pub(super) fn finish_remote_open(&mut self, document: DocumentId) -> io::Result<()> {
        let path = self.session.snapshot(document)?.path;
        self.remote_ui.opening.retain(|requested| {
            self.session.document_for_path(Path::new(requested)) != Some(document)
        });
        let pending = self
            .modules
            .pending_open
            .keys()
            .find(|requested| {
                requested.as_str() == path
                    || self.session.document_for_path(Path::new(requested)) == Some(document)
            })
            .cloned();
        let viewer = pending
            .and_then(|requested| self.modules.pending_open.remove(&requested))
            .flatten();
        let requested = self
            .remote_ui
            .restore
            .keys()
            .find(|requested| {
                requested.as_str() == path
                    || self.session.document_for_path(Path::new(requested)) == Some(document)
            })
            .cloned();
        if let Some(panels) =
            requested.and_then(|requested| self.remote_ui.restore.remove(&requested))
        {
            for panel in panels {
                let viewer = panel["viewer"]
                    .as_str()
                    .or_else(|| match panel["kind"].as_str() {
                        Some("document") => Some("bed.text"),
                        Some("hex") => Some("bed.hex"),
                        _ => None,
                    });
                if panel["kind"].as_str() == Some("plugin") && viewer.is_none() {
                    self.open_plugin_panel(
                        panel["panel_type"].as_str().unwrap_or(""),
                        Some(document),
                        &panel["state"],
                        None,
                    )?;
                } else {
                    self.add_document_panel(document, viewer, &panel["state"])?;
                }
                self.apply_remote_panel_state(&panel)?;
            }
        } else if !self
            .tabs
            .iter()
            .any(|tab| tab.panel.document() == Some(document))
        {
            self.add_document_panel(document, viewer.as_deref(), &Value::Null)?;
        }
        let requested = self
            .remote_ui
            .navigation
            .keys()
            .find(|requested| {
                requested.as_str() == path
                    || self.session.document_for_path(Path::new(requested)) == Some(document)
            })
            .cloned();
        if let Some((row, column, utf16)) =
            requested.and_then(|requested| self.remote_ui.navigation.remove(&requested))
        {
            self.position_active(row, column, utf16)?;
        }
        Ok(())
    }
    pub(super) fn finish_remote_restoration(&mut self) -> io::Result<()> {
        self.remote_ui
            .opening
            .retain(|path| self.session.open_pending(Path::new(path)));
        self.modules
            .pending_open
            .retain(|path, _| self.session.open_pending(Path::new(path)));
        let stale = self
            .remote_ui
            .restore
            .keys()
            .filter(|path| !self.session.open_pending(Path::new(path)))
            .cloned()
            .collect::<Vec<_>>();
        for path in stale {
            self.remote_ui.restore.remove(&path);
        }
        if !self.remote_ui.restore.is_empty() {
            return Ok(());
        }
        if !self.remote_ui.restore_order.is_empty() {
            let order = std::mem::take(&mut self.remote_ui.restore_order);
            self.tabs.sort_by_key(|tab| {
                order
                    .iter()
                    .position(|id| *id == tab.id)
                    .unwrap_or(usize::MAX)
            });
        }
        if let Some(id) = self.remote_ui.restored_active.take()
            && let Some(tab) = self.tabs.iter().find(|tab| tab.id == id)
        {
            self.last_document = tab.panel.document();
            self.active = tab.panel.view_id();
        }
        if let Some(id) = self.remote_ui.restored_focus.take()
            && let Some(index) = self.tabs.iter().position(|tab| tab.id == id)
        {
            self.switch_to_tab(index);
        }
        Ok(())
    }
    fn apply_remote_panel_state(&mut self, panel: &Value) -> io::Result<()> {
        let tab = self.tabs.last_mut().unwrap();
        if let Some(id) = panel["id"].as_u64() {
            tab.id = id;
            self.next_tab = self.next_tab.max(id + 1);
        }
        let Some(view) = tab.panel.view_id() else {
            return Ok(());
        };
        if let Some(selections) = panel["selections"].as_array() {
            let selections = selections
                .iter()
                .filter_map(|value| {
                    let v = value.as_array()?;
                    Some(Selection {
                        head_row: v.first()?.as_i64()? as i32,
                        head_column: v.get(1)?.as_i64()? as i32,
                        anchor_row: v.get(2)?.as_i64()? as i32,
                        anchor_column: v.get(3)?.as_i64()? as i32,
                        preferred_column: 0,
                    })
                })
                .collect();
            self.session.with_commands(view, |commands| {
                commands.set_selections(
                    selections,
                    panel["primary"].as_u64().unwrap_or(0) as usize,
                    CursorReveal::Ensure,
                )
            })?;
        }
        if let Some(scroll) = panel["scroll"].as_array().filter(|value| value.len() == 2) {
            self.session.set_scroll(
                view,
                scroll[0].as_f64().unwrap_or(0.0) as f32,
                scroll[1].as_f64().unwrap_or(0.0) as f32,
            )?;
        }
        Ok(())
    }
    pub(super) fn navigate_file(
        &mut self,
        path: &str,
        row: i32,
        column: i32,
        utf16: bool,
    ) -> io::Result<()> {
        self.open_file_with_viewer(Path::new(path), Some("bed.text"), false)?;
        if self
            .session
            .document_for_path(Path::new(path))
            .is_some_and(|document| {
                self.active
                    .and_then(|view| self.session.document_for_view(view))
                    == Some(document)
                    && self.session.document_kind(document).ok() == Some(DocumentKind::Text)
            })
        {
            self.position_active(row, column, utf16)?;
        } else {
            self.remote_ui
                .navigation
                .insert(path.to_owned(), (row, column, utf16));
        }
        Ok(())
    }
    pub(super) fn position_active(&mut self, row: i32, column: i32, utf16: bool) -> io::Result<()> {
        if let Some(view) = self.active {
            self.session.with_view(view, |editor| {
                let row = row.clamp(0, editor.state.line_count() - 1);
                let column = if utf16 {
                    utf16_to_utf8_byte_offset(&editor.state.line(row), column)
                } else {
                    column
                };
                editor.api().center_on(row, column);
            })?;
        }
        Ok(())
    }
    pub(super) fn show_remote_path_dialog(
        &mut self,
        document: Option<DocumentId>,
    ) -> io::Result<()> {
        let path = match document {
            Some(id) => {
                let path = self.session.snapshot(id)?.path;
                if path.is_empty() {
                    format!("{}/", self.project_root.trim_end_matches('/'))
                } else {
                    path
                }
            }
            None => format!("{}/", self.project_root.trim_end_matches('/')),
        };
        if self.remote_ui.path_dialog.is_none() {
            self.remote_ui.path_dialog = Some(PathDialog {
                document,
                path,
                appearing: true,
                error: None,
            });
        }
        Ok(())
    }
    pub(super) fn draw_remote_path_dialog(&mut self, ui: &Ui) -> io::Result<()> {
        let _dialog_style = bed_ui::util::popup_style::dialog_style(ui);
        let Some(mut dialog) = self.remote_ui.path_dialog.take() else {
            return Ok(());
        };
        if dialog.appearing {
            ui.open_popup("Remote file path");
            dialog.appearing = false;
        }
        let mut done = false;
        if let Some(_popup) = ui.begin_modal_popup("Remote file path") {
            let title = if dialog.document.is_some() {
                "Save As"
            } else {
                "Open"
            };
            ui.text("Absolute path on SSH host");
            ui.input_text("Path", &mut dialog.path).build();
            if let Some(error) = &dialog.error {
                ui.text_wrapped(error);
            }
            if ui.button("Cancel") {
                self.remote_ui.cancel_connection();
                self.remote_ui.pending_local = None;
                self.pending_tab_close = None;
                done = true;
                ui.close_current_popup();
            }
            ui.same_line();
            if ui.button(title) {
                let result = if !dialog.path.starts_with('/') || dialog.path.ends_with('/') {
                    Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "Enter an absolute file path",
                    ))
                } else if let Some(id) = dialog.document {
                    self.session
                        .save_as(id, Path::new(&dialog.path))
                        .map(|_| ())
                } else {
                    let viewer = self.remote_ui.dialog_viewer.clone();
                    self.open_file_with_viewer(Path::new(&dialog.path), viewer.as_deref(), false)
                        .map(|_| ())
                };
                match result {
                    Ok(()) => {
                        done = true;
                        ui.close_current_popup();
                    }
                    Err(error) => dialog.error = Some(error.to_string()),
                }
            }
        }
        if !done {
            self.remote_ui.path_dialog = Some(dialog);
        } else {
            self.remote_ui.dialog_viewer = None;
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/workbench/remote_workspace_tests.rs"]
mod tests;
