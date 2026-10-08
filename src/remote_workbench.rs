//! Standalone SSH workspace orchestration. Pipe I/O stays off the ImGui thread.
use super::*;
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
                    self.welcome
                        .set_recent_workspaces(store.recent_workspaces());
                }
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
                        &crate::util::remote_helpers::helpers_directory(),
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
                            self.welcome
                                .set_recent_workspaces(store.recent_workspaces());
                        }
                        self.workspace_spec = Some(spec);
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
                    self.file_explorer.file_tree.apply_directory(&path, entries);
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
        self.file_explorer
            .file_finder
            .set_remote_client(Some(client));
        self.file_explorer
            .file_finder
            .set_project_dir(&self.project_root);
        for search in self.content_search.values_mut() {
            search.set_remote_client(self.session.remote_client());
        }
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
        self.session.shutdown(ClosePolicy::Discard)?;
        self.close_plugin_panels()?;
        self.terminal.shutdown();
        self.tabs.clear();
        self.pending_tab_close = None;
        self.active = None;
        self.last_document = None;
        self.focused = None;
        self.lsp_ui.cancel_requests();
        self.content_search.clear();
        self.plugins = plugin_host::PluginRuntime::default();
        self.editor_menu_context.clear();
        self.reset_workspace_layout();
        self.remote_ui = RemoteUi {
            io: Some(RemoteIo::new(client.clone())),
            ..RemoteUi::default()
        };
        self.session = session;
        self.project_root = spec.root.clone();
        self.workspace_spec = Some(spec.clone());
        self.service_settings = None;
        self.sync_services()?;
        self.file_explorer.project_root = spec.root.clone();
        self.file_explorer.file_tree = crate::files::file_tree::FileTree {
            root_node: crate::files::file_tree::FileNode {
                name: spec.name.clone(),
                full_path: spec.root.clone(),
                is_directory: true,
                is_open: true,
                ..Default::default()
            },
            ..Default::default()
        };
        self.restore_tree_preferences(&spec);
        self.file_explorer.file_finder.set_project_dir("");
        self.file_explorer
            .file_finder
            .set_remote_client(Some(client));
        self.file_explorer.file_finder.set_project_dir(&spec.root);
        self.terminal.set_ssh_target(Some(target));
        self.terminal.set_project_root(&spec.root);
        let restored = self
            .store
            .as_ref()
            .and_then(|store| store.layout(&spec))
            .cloned();
        if let Some(store) = &mut self.store {
            self.workspace_spec = Some(store.record_workspace(spec)?);
            self.welcome
                .set_recent_workspaces(store.recent_workspaces());
        }
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
        let paths = self.file_explorer.file_tree.open_directories();
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
                    classify_gitignored: self.file_explorer.file_tree.preferences.hide_gitignored,
                },
            )?;
            io.pending_directories.insert(path);
        }
        Ok(())
    }
    pub(super) fn refresh_remote_files(&mut self) -> io::Result<()> {
        self.queue_remote_directories(true)?;
        self.file_explorer
            .file_finder
            .set_project_dir(&self.project_root);
        for search in self.content_search.values_mut() {
            search.cancel();
        }
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
                    .file_explorer
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
            .plugins
            .pending_open
            .keys()
            .find(|requested| {
                requested.as_str() == path
                    || self.session.document_for_path(Path::new(requested)) == Some(document)
            })
            .cloned();
        let viewer = pending
            .and_then(|requested| self.plugins.pending_open.remove(&requested))
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
        self.plugins
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
            if let Panel::Document(view) = &tab.panel {
                self.active = Some(view.id());
            }
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
        let Panel::Document(view) = &tab.panel else {
            return Ok(());
        };
        let selections = panel["selections"]
            .as_array()
            .into_iter()
            .flatten()
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
        self.session.with_commands(view.id(), |commands| {
            commands.set_selections(
                selections,
                panel["primary"].as_u64().unwrap_or(0) as usize,
                CursorReveal::Ensure,
            )
        })?;
        if let Some(scroll) = panel["scroll"].as_array().filter(|value| value.len() == 2) {
            self.session.set_scroll(
                view.id(),
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
        let _dialog_style = crate::util::dialog_style(ui);
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
mod tests {
    use super::*;
    use crate::files::test_support::TempDir;
    #[cfg(unix)]
    use std::process::Command;
    use std::time::Instant;

    #[cfg(unix)]
    fn client() -> RemoteClient {
        let executable = std::env::current_exe().unwrap();
        let helper = executable
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join(if cfg!(windows) {
                "bed-headless.exe"
            } else {
                "bed-headless"
            });
        if helper.is_file() {
            return RemoteClient::launch_local(helper).unwrap();
        }
        // A fresh package-only test run may not have built the sibling binary yet.
        let mut command = Command::new(env!("CARGO"));
        command.current_dir(env!("CARGO_MANIFEST_DIR")).args([
            "run",
            "--quiet",
            "--offline",
            "-p",
            "bed-headless",
            "--",
            "--stdio",
        ]);
        RemoteClient::launch_command(command).unwrap()
    }
    #[cfg(unix)]
    fn workspace(temp: &TempDir, root: &str) -> Workbench {
        let mut settings = Settings::with_paths(
            temp.path("config"),
            PathBuf::from(env!("CARGO_MANIFEST_DIR")),
        )
        .unwrap();
        settings.terminal_visible = false;
        settings.settings["terminal_visible"] = json!(false);
        settings.settings["treesitter"] = json!(false);
        settings.settings["git_changed_lines"] = json!(false);
        let mut workbench = Workbench::with_settings(settings);
        workbench
            .activate_remote_workspace(
                WorkspaceSpec {
                    name: "Remote fixture".into(),
                    target: WorkspaceTarget::Ssh {
                        host: "test-host".into(),
                    },
                    root: root.into(),
                },
                SshTarget {
                    host: "test-host".into(),
                    agent: "/cache/bed/version-one/bed-headless".into(),
                },
                client(),
            )
            .unwrap();
        let mut options = workbench.session.options().clone();
        options.autosave = None;
        options.lsp_config = None;
        workbench.session.configure(options).unwrap();
        workbench
    }
    fn wait(workbench: &mut Workbench, predicate: impl Fn(&Workbench) -> bool) {
        wait_for(workbench, Duration::from_secs(10), predicate);
    }
    fn wait_for(
        workbench: &mut Workbench,
        timeout: Duration,
        predicate: impl Fn(&Workbench) -> bool,
    ) {
        let deadline = Instant::now() + timeout;
        loop {
            workbench.tick().unwrap();
            if predicate(workbench) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "remote UI completion timed out: {:?}",
                workbench.error
            );
            thread::sleep(Duration::from_millis(5));
        }
    }
    fn structure_outline(
        workbench: &Workbench,
    ) -> Option<std::cell::Ref<'_, bed_highlight::outline::OutlineService>> {
        workbench.plugins.instances.iter().find_map(|plugin| {
            plugin
                .as_any()
                .downcast_ref::<bed_plugin_structure::StructurePlugin>()?
                .outline()
        })
    }
    #[cfg(unix)]
    #[test]
    fn remote_structure_uses_unsaved_local_buffer_and_restored_document_target() {
        let temp = TempDir::new();
        let file = temp.write("project/file.rs", b"mod demo { fn saved() {} }\n");
        let root = std::fs::canonicalize(temp.path("project")).unwrap();
        let file = std::fs::canonicalize(file).unwrap();
        let mut workbench = workspace(&temp, root.to_str().unwrap());
        let state = json!({"panels":[
            {"kind":"document","id":11,"path":file.to_str().unwrap()},
            {"kind":"structure","id":12},
            {"kind":"structure","id":13}
        ],"focused":13,"active_document_panel":11});
        workbench.restore_workspace(&state).unwrap();
        wait(&mut workbench, |w| {
            w.remote_ui.restore.is_empty()
                && structure_outline(w).is_some_and(|outline| outline.result().is_some())
        });
        assert_eq!(workbench.active_panel_id(), Some(13));
        assert_eq!(workbench.panel_count("structure"), 2);
        let view = workbench.active_view().unwrap();
        workbench
            .session
            .with_commands(view, |commands| commands.paste(b"fn unsaved() {}\n"))
            .unwrap();
        wait(&mut workbench, |w| {
            structure_outline(w).is_some_and(|outline| {
                !outline.updating()
                    && outline.result().is_some_and(|result| {
                        result.nodes.iter().any(|node| node.label == "unsaved")
                    })
            })
        });
        assert_eq!(
            std::fs::read(&file).unwrap(),
            b"mod demo { fn saved() {} }\n"
        );
        let jump = {
            let outline = structure_outline(&workbench).unwrap();
            let result = outline.result().unwrap();
            bed_plugin_structure::presentation::StructureJump {
                key: result.key.clone(),
                offset: result
                    .nodes
                    .iter()
                    .find(|node| node.label == "saved")
                    .unwrap()
                    .name_range
                    .start,
            }
        };
        let request = bed_plugin_structure::StructurePlugin::navigation_request(
            jump,
            &workbench.plugins.frame.context(),
        )
        .unwrap();
        workbench.plugins.requests.push(request);
        workbench.process_plugin_requests().unwrap();
        assert_eq!(workbench.active_view(), Some(view));
        assert_eq!(workbench.active_panel_id(), Some(11));
    }
    #[cfg(unix)]
    #[test]
    fn bulk_tab_close_waits_for_remote_saves_and_keeps_new_panels() {
        let temp = TempDir::new();
        let first = temp.write("project/a.rs", b"a");
        let second = temp.write("project/b.rs", b"b");
        let root = std::fs::canonicalize(temp.path("project")).unwrap();
        let mut workbench = workspace(&temp, root.to_str().unwrap());
        for path in [&first, &second] {
            workbench.open_or_focus(path).unwrap();
            wait(&mut workbench, |w| {
                w.session.document_ids().len() == if path == &first { 1 } else { 2 }
            });
            let view = workbench.active_view().unwrap();
            workbench
                .session
                .with_commands(view, |commands| commands.type_text(b"edited "))
                .unwrap();
        }
        let indices = (0..workbench.tabs.len()).collect();
        assert!(!workbench.close_tabs(indices).unwrap());
        assert!(workbench.pending_tab_close.is_some());
        workbench
            .dispatch(crate::workbench::WindowCommand::NewSettings)
            .unwrap();
        let added = workbench.focused;
        wait(&mut workbench, |w| w.pending_tab_close.is_none());
        assert_eq!(std::fs::read(first).unwrap(), b"edited a");
        assert_eq!(std::fs::read(second).unwrap(), b"edited b");
        assert_eq!(workbench.tabs.len(), 1);
        assert_eq!(workbench.focused, added);
        assert!(matches!(
            workbench.tabs[0].panel,
            Panel::Tool(Tool::Settings)
        ));
    }
    #[cfg(unix)]
    #[test]
    fn enabling_remote_autosave_schedules_existing_unsaved_edits() {
        let temp = TempDir::new();
        let file = temp.write("project/code.rs", b"code");
        let root = std::fs::canonicalize(temp.path("project")).unwrap();
        let mut workbench = workspace(&temp, root.to_str().unwrap());
        workbench.settings.settings["autosave"] = json!(false);
        workbench.sync_services().unwrap();
        workbench.open_or_focus(&file).unwrap();
        wait(&mut workbench, |w| w.active_document().is_some());
        let view = workbench.active_view().unwrap();
        workbench
            .session
            .with_commands(view, |commands| commands.type_text(b"edited "))
            .unwrap();
        workbench.tick().unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"code");
        workbench.settings.settings["autosave"] = json!(true);
        workbench.settings.settings["autosave_delay_ms"] = json!(100);
        workbench.sync_services().unwrap();
        wait(&mut workbench, |w| !w.active_snapshot().unwrap().dirty);
        assert_eq!(std::fs::read(file).unwrap(), b"edited code");
    }
    #[cfg(unix)]
    #[test]
    fn remote_open_save_rename_and_remove_keep_document_identity() {
        let temp = TempDir::new();
        let project = temp.path("project");
        std::fs::create_dir_all(&project).unwrap();
        let file = temp.write("project/file.txt", b"hello");
        let root = std::fs::canonicalize(&project).unwrap();
        let file = std::fs::canonicalize(file).unwrap();
        let mut workbench = workspace(&temp, root.to_str().unwrap());
        assert!(workbench.open_or_focus(&file).unwrap());
        assert!(workbench.active_document().is_none());
        wait(&mut workbench, |w| w.active_document().is_some());
        let document = workbench.active_document().unwrap();
        let view = workbench.active_view().unwrap();
        workbench
            .session
            .with_commands(view, |commands| commands.paste(b"edited "))
            .unwrap();
        let index = workbench.active_tab_index().unwrap();
        assert!(!workbench.close_tab(index).unwrap());
        assert!(workbench.session.save_pending(document));
        assert!(
            workbench
                .tabs
                .iter()
                .any(|tab| matches!(&tab.panel, Panel::Document(v) if v.id() == view))
        );
        wait(&mut workbench, |w| !w.session.save_pending(document));
        assert!(!workbench.session.snapshot(document).unwrap().dirty);
        assert_eq!(std::fs::read(&file).unwrap(), b"edited hello");
        workbench.rename_path(&file, "renamed.txt").unwrap();
        assert!(workbench.remote_ui.mutation_pending());
        wait(&mut workbench, |w| !w.remote_ui.mutation_pending());
        let renamed = root.join("renamed.txt");
        assert_eq!(
            workbench.session.snapshot(document).unwrap().path,
            renamed.to_str().unwrap()
        );
        assert_eq!(workbench.active_view(), Some(view));
        assert_eq!(workbench.session.document_for_view(view), Some(document));
        workbench.trash_path(&renamed).unwrap();
        wait(&mut workbench, |w| !w.remote_ui.mutation_pending());
        assert!(
            workbench
                .session
                .snapshot(document)
                .unwrap()
                .disk_conflict
                .is_some()
        );
        assert_eq!(
            workbench.session.snapshot(document).unwrap().bytes,
            b"edited hello"
        );
        assert_eq!(workbench.session.document_for_view(view), Some(document));
        assert!(!renamed.exists());
    }
    #[cfg(unix)]
    #[test]
    fn remote_registered_image_viewer_precedes_classifier_and_shares_bytes_with_hex() {
        let temp = TempDir::new();
        let file = temp.write(
            "project/picture.PNG",
            b"text-like payload intentionally handled by registered extension",
        );
        let root = std::fs::canonicalize(temp.path("project")).unwrap();
        let file = std::fs::canonicalize(file).unwrap();
        let mut workbench = workspace(&temp, root.to_str().unwrap());
        workbench.open_or_focus(&file).unwrap();
        wait(&mut workbench, |workbench| {
            workbench.session.document_for_path(&file).is_some()
        });
        let document = workbench.session.document_for_path(&file).unwrap();
        assert_eq!(
            workbench.session.document_kind(document).unwrap(),
            DocumentKind::Bytes
        );
        assert!(
            matches!(&workbench.tabs.last().unwrap().panel, Panel::Plugin(panel) if panel.viewer.as_deref() == Some(bed_plugin_image::VIEWER_ID))
        );
        workbench
            .open_file_with_viewer(&file, Some("bed.hex"), true)
            .unwrap();
        assert_eq!(
            workbench
                .tabs
                .iter()
                .filter(|tab| tab.panel.document() == Some(document))
                .count(),
            2
        );
        let revision = workbench.session.document_revision(document).unwrap();
        workbench
            .session
            .apply_edits(
                document,
                revision,
                &[bed_session::ByteEdit {
                    range: 0..1,
                    bytes: vec![0xff],
                }],
            )
            .unwrap();
        let bytes = workbench.session.snapshot(document).unwrap().bytes;
        let hex = workbench
            .tabs
            .iter()
            .position(|tab| matches!(tab.panel, Panel::Hex(_)))
            .unwrap();
        assert!(
            workbench.close_tab(hex).unwrap(),
            "closing a sibling view must retain the shared dirty document"
        );
        assert!(workbench.session.snapshot(document).unwrap().dirty);
        let image = workbench
            .tabs
            .iter()
            .position(|tab| tab.panel.document() == Some(document))
            .unwrap();
        assert!(
            !workbench.close_tab(image).unwrap(),
            "last attached panel must wait for queued SSH save"
        );
        wait(&mut workbench, |workbench| {
            !workbench.session.save_pending(document)
        });
        assert!(workbench.close_tab(image).unwrap());
        assert_eq!(std::fs::read(file).unwrap(), bytes);
        assert!(workbench.session.snapshot(document).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn remote_restore_preserves_hex_and_plugin_viewers_attached_to_one_byte_document() {
        let temp = TempDir::new();
        let file = temp.write("project/picture.png", b"\0\xffraw image bytes\r\n");
        let root = std::fs::canonicalize(temp.path("project")).unwrap();
        let file = std::fs::canonicalize(file).unwrap();
        let mut workbench = workspace(&temp, root.to_str().unwrap());
        let path = file.to_str().unwrap();
        let state = json!({"panels":[
            {"kind":"hex","id":51,"path":path,"viewer":"bed.hex","document_kind":"bytes","state":{"cursor":5,"anchor":3,"insert":true}},
            {"kind":"plugin","id":52,"path":path,"viewer":bed_plugin_image::VIEWER_ID,"panel_type":bed_plugin_image::PANEL_ID,"document_kind":"bytes","state":{"fit":false,"zoom":2.0,"pan":[3.0,4.0]}}
        ],"focused":51});
        workbench.restore_workspace(&state).unwrap();
        wait(&mut workbench, |workbench| {
            workbench.remote_ui.restore.is_empty() && workbench.session.document_ids().len() == 1
        });
        let document = workbench.session.document_for_path(&file).unwrap();
        assert_eq!(
            workbench.session.document_kind(document).unwrap(),
            DocumentKind::Bytes
        );
        assert_eq!(workbench.active_panel_id(), Some(51));
        let hex = workbench.tabs.iter().find(|tab| tab.id == 51).unwrap();
        let Panel::Hex(hex) = &hex.panel else {
            panic!("hex view restored as another panel");
        };
        assert_eq!(hex.state(), json!({"cursor":5,"anchor":3,"insert":true}));
        let plugin = workbench.tabs.iter().find(|tab| tab.id == 52).unwrap();
        let Panel::Plugin(plugin) = &plugin.panel else {
            panic!("image viewer restored as another panel");
        };
        assert_eq!(plugin.viewer.as_deref(), Some(bed_plugin_image::VIEWER_ID));
        assert_eq!(plugin.instance.attached_document(), Some(document));
        assert_eq!(plugin.instance.save_state()["zoom"], json!(2.0));
        assert_eq!(plugin.instance.save_state()["pan"], json!([3.0, 4.0]));
        assert_eq!(
            workbench.session.snapshot(document).unwrap().bytes,
            b"\0\xffraw image bytes\r\n"
        );
    }
    #[cfg(unix)]
    #[test]
    fn remote_layout_restores_shared_views_and_positions_after_open() {
        let temp = TempDir::new();
        let file = temp.write("project/file.txt", b"one\ntwo\nthree");
        let root = std::fs::canonicalize(temp.path("project")).unwrap();
        let file = std::fs::canonicalize(file).unwrap();
        let mut workbench = workspace(&temp, root.to_str().unwrap());
        let path = file.to_str().unwrap();
        let state = json!({"panels":[
            {"kind":"document","id":11,"path":path,"selections":[[1,2,1,0]],"primary":0,"scroll":[0,0]},
            {"kind":"document","id":12,"path":path,"selections":[[2,1,2,1]],"primary":0,"scroll":[0,0]}
        ],"focused":12,"active_document_panel":12});
        workbench.restore_workspace(&state).unwrap();
        assert_eq!(workbench.session.document_ids().len(), 0);
        wait(&mut workbench, |w| {
            w.remote_ui.restore.is_empty() && w.session.document_ids().len() == 1
        });
        let document = workbench.active_document().unwrap();
        assert_eq!(workbench.session.view_ids(document).len(), 2);
        let view = workbench
            .tabs
            .iter()
            .find_map(|tab| match &tab.panel {
                Panel::Document(v) if tab.id == 11 => Some(v.id()),
                _ => None,
            })
            .unwrap();
        let snapshot = workbench.session.view_snapshot(view).unwrap();
        assert_eq!(snapshot.selections[0].head_row, 1);
        assert_eq!(snapshot.selections[0].head_column, 2);
        assert_eq!(snapshot.selections[0].anchor_column, 0);
        assert_eq!(workbench.active_panel_id(), Some(12));
    }
    #[cfg(unix)]
    #[test]
    fn canonical_alias_and_failed_connection_preserve_active_buffer_and_views() {
        let temp = TempDir::new();
        let file = temp.write("project/file.txt", b"buffer");
        let root = std::fs::canonicalize(temp.path("project")).unwrap();
        let mut workbench = workspace(&temp, root.to_str().unwrap());
        let spec = workbench.workspace_spec.clone().unwrap();
        workbench.open_or_focus(&file).unwrap();
        wait(&mut workbench, |w| w.active_document().is_some());
        let document = workbench.active_document().unwrap();
        let view = workbench.active_view().unwrap();
        workbench
            .session
            .with_commands(view, |commands| commands.paste(b"unsaved "))
            .unwrap();
        // Connecting through ~/ or another canonical alias must retain the
        // current document, undo and remembered display name after validation.
        let mut unnamed_alias = spec.clone();
        unnamed_alias.name.clear();
        workbench.remote_ui.ready = Some(ConnectionResult {
            reconnect: false,
            result: Ok(ConnectedWorkspace {
                spec: unnamed_alias,
                target: workbench.session.ssh_target().unwrap().clone(),
                client: client(),
            }),
        });
        workbench.poll_remote_workspace().unwrap();
        assert_eq!(workbench.workspace_spec.as_ref(), Some(&spec));
        assert_eq!(workbench.active_view(), Some(view));
        assert!(workbench.session.snapshot(document).unwrap().dirty);
        assert_eq!(
            workbench.session.snapshot(document).unwrap().bytes,
            b"unsaved buffer"
        );
        workbench.remote_ui.ready = Some(ConnectionResult {
            reconnect: false,
            result: Err(io::Error::new(
                io::ErrorKind::ConnectionRefused,
                "fixture failed",
            )),
        });
        workbench.poll_remote_workspace().unwrap();
        assert_eq!(workbench.active_view(), Some(view));
        assert_eq!(
            workbench.session.snapshot(document).unwrap().bytes,
            b"unsaved buffer"
        );
        assert!(workbench.session.snapshot(document).unwrap().dirty);
        assert!(workbench.error.as_ref().unwrap().contains("fixture failed"));
    }
    #[cfg(unix)]
    #[test]
    fn disconnected_canonical_alias_reconnects_without_closing_dirty_views() {
        let temp = TempDir::new();
        let file = temp.write("project/file.txt", b"buffer");
        let root = std::fs::canonicalize(temp.path("project")).unwrap();
        let mut workbench = workspace(&temp, root.to_str().unwrap());
        let spec = workbench.workspace_spec.clone().unwrap();
        workbench.open_or_focus(&file).unwrap();
        wait(&mut workbench, |w| w.active_document().is_some());
        let document = workbench.active_document().unwrap();
        let view = workbench.active_view().unwrap();
        workbench
            .session
            .with_commands(view, |commands| commands.paste(b"unsaved "))
            .unwrap();
        workbench.session.remote_client().unwrap().disconnect();
        assert!(!workbench.session.remote_connected());
        let mut unnamed_alias = spec.clone();
        unnamed_alias.name.clear();
        workbench.remote_ui.ready = Some(ConnectionResult {
            reconnect: false,
            result: Ok(ConnectedWorkspace {
                spec: unnamed_alias,
                target: workbench.session.ssh_target().unwrap().clone(),
                client: client(),
            }),
        });
        workbench.poll_remote_workspace().unwrap();
        assert!(workbench.session.remote_connected());
        assert_eq!(workbench.workspace_spec.as_ref(), Some(&spec));
        assert_eq!(workbench.active_view(), Some(view));
        assert!(workbench.session.snapshot(document).unwrap().dirty);
        assert_eq!(
            workbench.session.snapshot(document).unwrap().bytes,
            b"unsaved buffer"
        );
        workbench
            .session
            .with_commands(view, |commands| commands.undo())
            .unwrap();
        assert_eq!(
            workbench.session.snapshot(document).unwrap().bytes,
            b"buffer"
        );
    }
    #[cfg(unix)]
    #[test]
    fn resolved_helper_is_used_by_services_without_replacing_automatic_preference() {
        let temp = TempDir::new();
        let file = temp.write("project/file.txt", b"buffer");
        let root = std::fs::canonicalize(temp.path("project")).unwrap();
        let mut workbench = workspace(&temp, root.to_str().unwrap());
        let spec = workbench.workspace_spec.clone().unwrap();
        assert_eq!(
            spec.target,
            WorkspaceTarget::Ssh {
                host: "test-host".into()
            }
        );
        assert_eq!(
            workbench.session.ssh_target().unwrap().agent,
            "/cache/bed/version-one/bed-headless"
        );
        assert!(!workbench.set_workspace(spec.clone()).unwrap());
        assert!(!workbench.remote_ui.connecting());
        assert_eq!(workbench.workspace_spec.as_ref(), Some(&spec));
        workbench.open_or_focus(&file).unwrap();
        wait(&mut workbench, |w| w.active_document().is_some());
        let document = workbench.active_document().unwrap();
        let view = workbench.active_view().unwrap();
        workbench
            .session
            .with_commands(view, |commands| commands.paste(b"unsaved "))
            .unwrap();
        workbench.remote_ui.ready = Some(ConnectionResult {
            reconnect: true,
            result: Ok(ConnectedWorkspace {
                spec: spec.clone(),
                target: SshTarget {
                    host: "test-host".into(),
                    agent: "/cache/bed/version-two/bed-headless".into(),
                },
                client: client(),
            }),
        });
        workbench.poll_remote_workspace().unwrap();
        assert_eq!(
            workbench.session.ssh_target().unwrap().agent,
            "/cache/bed/version-two/bed-headless"
        );
        assert_eq!(workbench.workspace_spec.as_ref(), Some(&spec));
        assert_eq!(
            WorkspaceStore::load(&temp.path("config"))
                .unwrap()
                .recent_workspaces(),
            vec![spec]
        );
        assert_eq!(workbench.active_view(), Some(view));
        assert_eq!(
            workbench.session.snapshot(document).unwrap().bytes,
            b"unsaved buffer"
        );
        assert!(workbench.session.snapshot(document).unwrap().dirty);
    }
    #[cfg(unix)]
    #[test]
    fn unnamed_remote_projects_derive_folder_names_and_keep_saved_labels() {
        let temp = TempDir::new();
        std::fs::create_dir_all(temp.path("first")).unwrap();
        let first = std::fs::canonicalize(temp.path("first")).unwrap();
        let mut workbench = workspace(&temp, first.to_str().unwrap());
        std::fs::create_dir_all(temp.path("folder with spaces")).unwrap();
        let root = std::fs::canonicalize(temp.path("folder with spaces")).unwrap();
        let spec = WorkspaceSpec {
            name: String::new(),
            target: WorkspaceTarget::Ssh {
                host: "test-host".into(),
            },
            root: root.to_str().unwrap().into(),
        };
        let target = workbench.session.ssh_target().unwrap().clone();
        workbench
            .activate_remote_workspace(spec.clone(), target.clone(), client())
            .unwrap();
        assert_eq!(
            workbench.workspace_spec.as_ref().unwrap().name,
            "folder with spaces"
        );
        workbench
            .store
            .as_mut()
            .unwrap()
            .rename_workspace(&spec, "Remembered label")
            .unwrap();
        workbench
            .activate_remote_workspace(spec, target, client())
            .unwrap();
        assert_eq!(
            workbench.workspace_spec.as_ref().unwrap().name,
            "Remembered label"
        );
    }
    #[test]
    fn remote_paths_use_target_separators_and_component_boundaries() {
        assert_eq!(remote_join("/project/dir", "file"), "/project/dir/file");
        assert_eq!(remote_join("/", "file"), "/file");
        assert_eq!(
            remote_rename_target("/project/old", "new").unwrap(),
            "/project/new"
        );
        assert_eq!(
            remote_rebound_path("/project/old", "/project/new", "/project/old/sub/file").unwrap(),
            "/project/new/sub/file"
        );
        assert!(remote_contains("/project/dir", "/project/dir/file"));
        assert!(!remote_contains("/project/dir", "/project/directory/file"));
        assert!(
            remote_rebound_path("/project/dir", "/project/new", "/project/directory/file").is_err()
        );
    }
    /// Opt in with BED_TEST_SSH_HOST and BED_TEST_SSH_ROOT (absolute or ~/).
    /// Creates and removes only its unique fixture child.
    #[test]
    fn live_ssh_workbench_connection_edit_conflict_and_reconnect() {
        let Ok(host) = std::env::var("BED_TEST_SSH_HOST") else {
            eprintln!(
                "Live SSH Workbench test skipped: set BED_TEST_SSH_HOST and BED_TEST_SSH_ROOT to opt in"
            );
            return;
        };
        let root = std::env::var("BED_TEST_SSH_ROOT")
            .expect("BED_TEST_SSH_ROOT must name an isolated remote test directory");
        assert!(root.starts_with('/') || root == "~" || root.starts_with("~/"));
        let temp = TempDir::new();
        let mut settings = Settings::with_paths(
            temp.path("config"),
            PathBuf::from(env!("CARGO_MANIFEST_DIR")),
        )
        .unwrap();
        settings.terminal_visible = false;
        settings.settings["terminal_visible"] = json!(false);
        settings.settings["treesitter"] = json!(false);
        settings.settings["git_changed_lines"] = json!(false);
        let mut workbench = Workbench::with_settings(settings);
        let requested = WorkspaceSpec {
            name: String::new(),
            target: WorkspaceTarget::Ssh { host },
            root,
        };
        assert!(workbench.set_workspace(requested).unwrap());
        assert!(!workbench.session.is_remote());
        wait_for(&mut workbench, Duration::from_secs(60), |w| {
            w.session.is_remote() && w.session.remote_connected() && !w.remote_ui.connecting()
        });
        let target = workbench.session.ssh_target().unwrap().clone();
        let preference = workbench.workspace_spec.clone().unwrap();
        assert!(target.agent.starts_with('/'));
        assert!(preference.root.starts_with('/'));
        assert_eq!(preference.name, WorkspaceSpec::local(&preference.root).name);
        assert_eq!(
            WorkspaceStore::load(&temp.path("config"))
                .unwrap()
                .recent_workspaces(),
            vec![preference.clone()]
        );
        let root = workbench.project_root.clone();
        let external = RemoteClient::launch_ssh(&target).unwrap();
        let token = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let fixture_root = format!(
            "{}/bed-workbench-smoke-{}-{token}",
            root.trim_end_matches('/'),
            std::process::id()
        );
        external
            .call(Request::CreateDirectory {
                root: root.clone(),
                path: fixture_root.clone(),
            })
            .unwrap();
        struct Fixture {
            client: RemoteClient,
            root: String,
            path: String,
        }
        impl Drop for Fixture {
            fn drop(&mut self) {
                if let Err(error) = self.client.call(Request::Remove {
                    root: self.root.clone(),
                    path: self.path.clone(),
                    is_directory: true,
                }) {
                    eprintln!("Unable to clean live SSH fixture {}: {error}", self.path);
                }
                self.client.disconnect();
            }
        }
        let fixture = Fixture {
            client: external,
            root,
            path: fixture_root.clone(),
        };
        let original = format!("{fixture_root}/file with spaces.txt");
        fixture
            .client
            .call(Request::WriteFile {
                root: fixture_root.clone(),
                path: original.clone(),
                bytes: b"hello".to_vec(),
                baseline: None,
            })
            .unwrap();
        let mut options = workbench.session.options().clone();
        options.autosave = None;
        options.lsp_config = None;
        workbench.session.configure(options).unwrap();
        workbench.open_or_focus(Path::new(&original)).unwrap();
        wait(&mut workbench, |w| w.active_document().is_some());
        let document = workbench.active_document().unwrap();
        let first = workbench.active_view().unwrap();
        workbench.dispatch(WindowCommand::DuplicateView).unwrap();
        let second = workbench.active_view().unwrap();
        assert_ne!(first, second);
        workbench
            .session
            .with_commands(second, |commands| commands.paste(b"over SSH "))
            .unwrap();
        workbench.handle_action(HostAction::Save).unwrap();
        assert!(workbench.session.save_pending(document));
        assert!(workbench.session.snapshot(document).unwrap().dirty);
        wait(&mut workbench, |w| !w.session.save_pending(document));
        assert!(!workbench.session.snapshot(document).unwrap().dirty);
        let Response::File { bytes, .. } = fixture
            .client
            .call(Request::ReadFile {
                root: fixture_root.clone(),
                path: original.clone(),
            })
            .unwrap()
        else {
            panic!("Expected saved file");
        };
        assert_eq!(bytes, b"over SSH hello");
        workbench
            .rename_path(Path::new(&original), "renamed.txt")
            .unwrap();
        wait(&mut workbench, |w| !w.remote_ui.mutation_pending());
        let renamed = format!("{fixture_root}/renamed.txt");
        assert_eq!(workbench.session.snapshot(document).unwrap().path, renamed);
        assert_eq!(workbench.session.document_for_view(first), Some(document));
        assert_eq!(workbench.session.document_for_view(second), Some(document));
        workbench
            .session
            .with_commands(second, |commands| commands.paste(b"unsaved "))
            .unwrap();
        let retained = workbench.session.snapshot(document).unwrap().bytes;
        let Response::File { baseline, .. } = fixture
            .client
            .call(Request::ReadFile {
                root: fixture_root.clone(),
                path: renamed.clone(),
            })
            .unwrap()
        else {
            panic!("Expected rename baseline");
        };
        fixture
            .client
            .call(Request::WriteFile {
                root: fixture_root.clone(),
                path: renamed.clone(),
                bytes: b"external edit".to_vec(),
                baseline: Some(baseline),
            })
            .unwrap();
        wait(&mut workbench, |w| {
            w.session
                .snapshot(document)
                .unwrap()
                .disk_conflict
                .is_some()
        });
        assert_eq!(
            workbench.session.snapshot(document).unwrap().bytes,
            retained
        );
        assert!(workbench.handle_action(HostAction::Save).is_err());
        workbench.session.remote_client().unwrap().disconnect();
        workbench.tick().unwrap();
        assert!(!workbench.session.remote_connected());
        assert!(workbench.reconnect_workspace().unwrap());
        wait(&mut workbench, |w| {
            w.session.remote_connected() && !w.remote_ui.connecting()
        });
        assert_eq!(workbench.workspace_spec.as_ref(), Some(&preference));
        assert_eq!(workbench.session.ssh_target(), Some(&target));
        assert_eq!(
            workbench.session.snapshot(document).unwrap().bytes,
            retained
        );
        assert_eq!(workbench.session.document_for_view(first), Some(document));
        assert_eq!(workbench.session.document_for_view(second), Some(document));
        assert!(workbench.session.snapshot(document).unwrap().dirty);
        let Response::File { bytes, .. } = fixture
            .client
            .call(Request::ReadFile {
                root: fixture_root,
                path: renamed,
            })
            .unwrap()
        else {
            panic!("Expected external file after reconnect");
        };
        assert_eq!(bytes, b"external edit");
        workbench
            .session
            .with_commands(second, |commands| commands.undo())
            .unwrap();
        assert_eq!(
            workbench.session.snapshot(document).unwrap().bytes,
            b"over SSH hello"
        );
        workbench
            .session
            .with_commands(second, |commands| commands.redo())
            .unwrap();
        assert_eq!(
            workbench.session.snapshot(document).unwrap().bytes,
            retained
        );
        workbench.session.shutdown(ClosePolicy::Discard).unwrap();
        workbench.terminal.shutdown();
        workbench.tabs.clear();
        workbench.remote_ui = RemoteUi::default();
        assert!(workbench.session.document_ids().is_empty());
        drop(fixture);
    }
}
