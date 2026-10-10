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
}
#[derive(Clone)]
enum Operation {
    CreateFile {
        path: String,
        target_area: Option<u32>,
    },
    CreateDirectory(String),
    Rename {
        source: String,
        target: String,
        documents: Vec<DocumentId>,
    },
    Remove {
        documents: Vec<DocumentId>,
        path: String,
    },
}
impl Operation {
    fn documents(&self) -> &[DocumentId] {
        match self {
            Self::Rename { documents, .. } | Self::Remove { documents, .. } => documents,
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
    target_area: Option<u32>,
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
    pub(super) drop_areas: HashMap<String, u32>,
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
        if matches!(spec.target, WorkspaceTarget::Local) && !self.session.is_remote() {
            return self.set_project(Path::new(&spec.root));
        }
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
                    // Finish committed path changes while the old panels and
                    // services still exist, then perform close/save preflight.
                    self.cancel_file_operations();
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
            let documents = operation.documents().to_vec();
            self.remote_ui.mutation_pending = false;
            let result = (|| match (operation, result) {
                (Operation::CreateFile { path, target_area }, Ok(Response::Unit)) => {
                    if target_area.is_none_or(|id| {
                        self.current_tiling()
                            .layout
                            .areas
                            .iter()
                            .any(|area| area.id == id)
                    }) {
                        self.open_file_from_menu(Path::new(&path), None, false, target_area)?;
                    }
                    self.refresh_file_parent(&path);
                    Ok(())
                }
                (Operation::CreateDirectory(path), Ok(Response::Unit)) => {
                    self.refresh_file_parent(&path);
                    Ok(())
                }
                (
                    Operation::Rename {
                        source,
                        target,
                        documents,
                    },
                    Ok(Response::Unit),
                ) => {
                    for id in documents {
                        if let Ok(path) = self.session.with_document(id, |state| state.path.clone())
                        {
                            let destination = remote_rebound_path(&source, &target, &path)?;
                            self.session.rebind_path(id, Path::new(&destination))?;
                        }
                    }
                    self.refresh_file_parent(&source);
                    Ok(())
                }
                (Operation::Remove { documents, path }, Ok(Response::Unit)) => {
                    for id in documents {
                        if let Ok(path) = self.session.with_document(id, |state| state.path.clone())
                        {
                            self.session.invalidate_removed_path(id)?;
                            self.handle_removed_document(id, &path)?;
                        }
                    }
                    self.refresh_file_parent(&path);
                    Ok(())
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
        self.cancel_file_operations();
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
        self.cancel_file_operations();
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
            self.apply_default_layout()?;
        }
        self.finish_workspace_open()?;
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
        if refresh {
            self.refresh_file_directories(paths);
        } else {
            self.file_explorer().file_finder.request_directories(paths);
        }
        Ok(())
    }
    fn refresh_file_parent(&mut self, path: &str) {
        if let Some((parent, _)) = path.rsplit_once('/') {
            self.refresh_file_directories([parent.to_owned()]);
        }
    }
    pub(super) fn queue_remote_file_action(
        &mut self,
        action: &FileTreeAction,
        name: &str,
    ) -> io::Result<()> {
        self.queue_remote_file_action_at(action, name, None)
    }
    pub(super) fn queue_remote_file_action_at(
        &mut self,
        action: &FileTreeAction,
        name: &str,
        area: Option<u32>,
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
                    Operation::CreateFile {
                        path: path.clone(),
                        target_area: area,
                    },
                    Request::CreateFile { root, path },
                )
            }
            FileTreeAction::NewFolder(directory) => {
                let path = remote_join(directory, name);
                (
                    Operation::CreateDirectory(path.clone()),
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
                    Operation::Remove {
                        documents,
                        path: path.clone(),
                    },
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
    pub(super) fn cancel_remote_area_contents(
        &mut self,
        removed_areas: &HashSet<u32>,
        removed_panels: &HashSet<u64>,
    ) {
        if self.remote_ui.path_dialog.as_ref().is_some_and(|dialog| {
            dialog
                .target_area
                .is_some_and(|area| removed_areas.contains(&area))
        }) {
            self.remote_ui.path_dialog = None;
            self.remote_ui.dialog_viewer = None;
        }
        let mut cancelled_paths = HashSet::new();
        for (path, panels) in &mut self.remote_ui.restore {
            let before = panels.len();
            panels.retain(|panel| {
                panel["id"]
                    .as_u64()
                    .is_none_or(|id| !removed_panels.contains(&id))
            });
            if panels.len() != before {
                cancelled_paths.insert(path.clone());
            }
        }
        for (path, area) in &self.remote_ui.drop_areas {
            if removed_areas.contains(area) {
                cancelled_paths.insert(path.clone());
            }
        }
        for path in cancelled_paths {
            // Keep an explicit empty restoration until the worker completes.
            // Removing the entry would reopen the file in a surviving area.
            self.remote_ui.restore.entry(path.clone()).or_default();
            self.remote_ui.drop_areas.remove(&path);
            self.remote_ui.navigation.remove(&path);
            self.modules.pending_open.remove(&path);
        }
        self.remote_ui
            .restore_order
            .retain(|id| !removed_panels.contains(id));
        self.remote_ui.restored_active = self
            .remote_ui
            .restored_active
            .filter(|id| !removed_panels.contains(id));
        self.remote_ui.restored_focus = self
            .remote_ui
            .restored_focus
            .filter(|id| !removed_panels.contains(id));
    }
    pub(super) fn finish_remote_open(&mut self, document: DocumentId) -> io::Result<()> {
        let path = self.session.snapshot(document)?.path;
        let drop_requested = self
            .remote_ui
            .drop_areas
            .keys()
            .find(|requested| {
                requested.as_str() == path
                    || self.session.document_for_path(Path::new(requested)) == Some(document)
            })
            .cloned();
        let drop_area = drop_requested
            .as_ref()
            .and_then(|requested| self.remote_ui.drop_areas.get(requested))
            .copied()
            .filter(|area| {
                self.current_tiling()
                    .layout
                    .areas
                    .iter()
                    .any(|value| value.id == *area)
            });
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
        let reuse_empty = pending
            .as_ref()
            .is_none_or(|path| !self.modules.pending_additional.remove(path));
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
        let mut cancelled = false;
        if let Some(panels) =
            requested.and_then(|requested| self.remote_ui.restore.remove(&requested))
        {
            cancelled = panels.is_empty();
            for panel in panels {
                let area = drop_area
                    .or_else(|| panel["id"].as_u64().and_then(|id| self.area_for_panel(id)));
                // Creating a panel focuses its temporary ID. A restored group
                // keeps its saved selection across that creation and rename;
                // an explicit file drop intentionally selects the incoming file.
                let saved_selection = if drop_area.is_none() {
                    area.and_then(|area| {
                        self.current_tiling()
                            .areas
                            .iter()
                            .find(|group| group.area == area)
                            .and_then(|group| group.selected)
                    })
                } else {
                    None
                };
                let viewer = panel["viewer"]
                    .as_str()
                    .or_else(|| match panel["kind"].as_str() {
                        Some("document") => Some("bed.text"),
                        Some("hex") => Some("bed.hex"),
                        _ => None,
                    });
                if panel["kind"].as_str() == Some("plugin") && viewer.is_none() {
                    let kind = panel["panel_type"].as_str().unwrap_or("");
                    if let Some(area) = area {
                        self.open_plugin_panel_in_area(
                            kind,
                            Some(document),
                            &panel["state"],
                            None,
                            area,
                        )?;
                    } else {
                        self.open_plugin_panel(kind, Some(document), &panel["state"], None)?;
                    }
                } else {
                    self.add_document_panel_at(document, viewer, &panel["state"], area)?;
                }
                self.apply_remote_panel_state(&panel)?;
                if let (Some(area), Some(selected)) = (area, saved_selection) {
                    self.edit_tiling()
                        .areas
                        .iter_mut()
                        .find(|group| group.area == area)
                        .expect("restored area remains live during panel creation")
                        .selected = Some(selected);
                }
            }
        } else if !reuse_empty
            || !self
                .tabs
                .iter()
                .any(|tab| tab.panel.document() == Some(document))
        {
            let viewer = self.resolved_document_viewer(document, viewer.as_deref())?;
            self.add_opened_file_panel_at(document, &viewer, drop_area, reuse_empty)?;
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
        if let Some(area) =
            drop_requested.and_then(|requested| self.remote_ui.drop_areas.remove(&requested))
            && self
                .current_tiling()
                .layout
                .areas
                .iter()
                .any(|value| value.id == area)
            && let Some(panel) = self.active_panel_id()
        {
            self.place_panel_in_area(panel, area);
        }
        if cancelled
            && !self
                .tabs
                .iter()
                .any(|tab| tab.panel.document() == Some(document))
        {
            self.session
                .close_document(document, ClosePolicy::Discard)?;
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
        self.modules
            .pending_additional
            .retain(|path| self.session.open_pending(Path::new(path)));
        self.remote_ui
            .drop_areas
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
            // Failed opens and unavailable viewers never create their reserved
            // panels. Remove those reservations once restoration is complete,
            // keeping the empty areas available to the content chooser.
            let live = self.tabs.iter().map(|tab| tab.id).collect::<Vec<_>>();
            self.edit_tiling().retain_panels(live);
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
        if let Some(id) = panel["id"].as_u64() {
            let previous = self.tabs.last().unwrap().id;
            self.replace_panel_id(previous, id);
        }
        let tab = self.tabs.last_mut().unwrap();
        tab.panel.saved_presentations =
            serde_json::from_value(panel["presentations"].clone()).unwrap_or_default();
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
        self.navigate_file_at(path, row, column, utf16, None)
    }
    pub(super) fn navigate_file_at(
        &mut self,
        path: &str,
        row: i32,
        column: i32,
        utf16: bool,
        area: Option<u32>,
    ) -> io::Result<()> {
        self.open_file_from_menu(Path::new(path), Some("bed.text"), false, area)?;
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
    pub(super) fn open_file_dialog_at(
        &mut self,
        viewer: Option<String>,
        area: Option<u32>,
    ) -> io::Result<()> {
        if self.session.is_remote() {
            if self.remote_ui.path_dialog.is_none() {
                self.remote_ui.dialog_viewer = viewer;
                self.show_remote_path_dialog_at(None, area)?;
            }
        } else if let Some(path) = rfd::FileDialog::new().pick_file() {
            self.open_file_from_menu(&path, viewer.as_deref(), false, area)?;
        }
        Ok(())
    }
    pub(super) fn show_remote_path_dialog(
        &mut self,
        document: Option<DocumentId>,
    ) -> io::Result<()> {
        self.show_remote_path_dialog_at(document, None)
    }
    fn show_remote_path_dialog_at(
        &mut self,
        document: Option<DocumentId>,
        target_area: Option<u32>,
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
                target_area,
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
                    self.open_file_from_menu(
                        Path::new(&dialog.path),
                        viewer.as_deref(),
                        false,
                        dialog.target_area,
                    )
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

#[cfg(test)]
mod tiling_tests {
    use super::*;
    use crate::{
        test_support::TempDir,
        workspace::{tiling::Layout, tiling_state::TilingState},
    };

    #[test]
    fn new_file_menu_keeps_its_previous_area_across_the_naming_dialog() {
        let dir = TempDir::new();
        dir.write("project/seed.txt", b"existing");
        let root = dir.path("project").canonicalize().unwrap();
        let mut workbench = super::super::tests::workspace(&dir);
        workbench.set_project(&root).unwrap();
        let source = workbench.tabs[0].id;
        let settings = workbench
            .open_native_panel("settings", &Value::Null)
            .unwrap();
        let mut layout = Layout::default();
        let destination = layout.split(1, 0, 7000, 1).unwrap();
        workbench.tiling = TilingState::new(layout);
        for tab in &workbench.tabs {
            workbench.tiling.assign(tab.id, 1);
        }
        workbench.tiling.assign(settings, destination);
        workbench.dock_built = true;
        let settings_index = workbench
            .tabs
            .iter()
            .position(|tab| tab.id == settings)
            .unwrap();
        let source_index = workbench
            .tabs
            .iter()
            .position(|tab| tab.id == source)
            .unwrap();
        // Work in the small area, then focus the menu's larger source area.
        workbench.switch_to_tab(settings_index);
        workbench.switch_to_tab(source_index);
        workbench
            .handle_tree_action(FileTreeAction::NewFile(root.to_string_lossy().into_owned()))
            .unwrap();
        let dialog = workbench.file_dialog.take().unwrap();
        assert_eq!(dialog.target_area, Some(destination));
        // A focus change while naming must not replace the captured target.
        workbench.switch_to_tab(settings_index);

        workbench
            .apply_file_action_at(&dialog.action, "menu.txt", dialog.target_area)
            .unwrap();
        assert_eq!(std::fs::read(root.join("menu.txt")).unwrap(), b"");
        let menu_panel = workbench.active_panel_id().unwrap();
        assert_eq!(workbench.area_for_panel(menu_panel), Some(destination));

        // Programmatic file creation still chooses the largest area, even
        // though the new menu-created document keeps the smaller one focused.
        workbench
            .apply_file_action(&dialog.action, "ordinary.txt")
            .unwrap();
        let ordinary_panel = workbench.active_panel_id().unwrap();
        assert_eq!(workbench.area_for_panel(ordinary_panel), Some(1));
        workbench.cleanup().unwrap();
    }

    #[test]
    fn remote_path_dialog_keeps_its_destination_after_focus_changes() {
        let dir = TempDir::new();
        let path = dir.write("chosen.txt", b"dialog destination");
        let mut workbench = super::super::tests::workspace(&dir);
        let projects = workbench.tabs[0].id;
        let settings = workbench
            .open_native_panel("settings", &Value::Null)
            .unwrap();
        let mut layout = Layout::default();
        let destination = layout.split(1, 0, 7000, 1).unwrap();
        workbench.tiling = TilingState::new(layout);
        workbench.tiling.assign(projects, 1);
        workbench.tiling.assign(settings, destination);
        workbench.dock_built = true;
        workbench.switch_to_tab(1);
        workbench
            .show_remote_path_dialog_at(None, Some(destination))
            .unwrap();
        workbench.switch_to_tab(0);
        assert_eq!(workbench.focused_area(), Some(1));

        let dialog = workbench.remote_ui.path_dialog.take().unwrap();
        workbench
            .open_file_from_menu(&path, None, false, dialog.target_area)
            .unwrap();
        let opened = workbench.active_panel_id().unwrap();
        assert_eq!(workbench.area_for_panel(opened), Some(destination));

        // Reopening an existing file focuses its tab without relocating it.
        workbench
            .open_file_from_menu(&path, None, false, Some(1))
            .unwrap();
        assert_eq!(workbench.active_panel_id(), Some(opened));
        assert_eq!(workbench.area_for_panel(opened), Some(destination));
        workbench.cleanup().unwrap();
    }

    #[test]
    fn an_open_dialog_rejects_its_removed_destination_before_opening() {
        let dir = TempDir::new();
        let path = dir.write(
            "not-opened.txt",
            b"destination removed while dialog was open",
        );
        let mut workbench = super::super::tests::workspace(&dir);
        let projects = workbench.tabs[0].id;
        let mut layout = Layout::default();
        let destination = layout.split(1, 0, 7000, 1).unwrap();
        workbench.tiling = TilingState::new(layout);
        workbench.tiling.assign(projects, 1);
        workbench.dock_built = true;
        workbench
            .show_remote_path_dialog_at(None, Some(destination))
            .unwrap();
        workbench.tiling = TilingState::new(Layout::default());
        workbench.tiling.assign(projects, 1);

        let dialog = workbench.remote_ui.path_dialog.take().unwrap();
        let error = workbench
            .open_file_from_menu(&path, None, false, dialog.target_area)
            .unwrap_err();
        assert_eq!(error.to_string(), "This area is no longer available");
        assert_eq!(workbench.session.document_for_path(&path), None);
        assert!(workbench.remote_ui.opening.is_empty());
        assert!(workbench.remote_ui.drop_areas.is_empty());
        assert!(workbench.modules.pending_open.is_empty());
        workbench.cleanup().unwrap();
    }

    #[test]
    fn remote_completion_preserves_saved_group_selection_across_temporary_ids() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        for selected in [41, 42] {
            let dir = TempDir::new();
            let path = dir.write("restored.txt", b"one document with two views");
            let mut workbench = super::super::tests::workspace(&dir);
            let projects = workbench.tabs[0].id;
            let mut layout = Layout::default();
            let destination = layout.split(1, 0, 3000, 1).unwrap();
            workbench.tiling = TilingState::new(layout);
            workbench.tiling.assign(projects, 1);
            workbench
                .tiling
                .sync_area(destination, vec![41, 42], Some(selected));
            workbench.next_tab = 43;
            workbench.dock_built = true;
            workbench.remote_ui.restore.insert(
                path.to_string_lossy().into_owned(),
                vec![
                    json!({"id":41,"kind":"document","viewer":"bed.text"}),
                    json!({"id":42,"kind":"document","viewer":"bed.text"}),
                ],
            );
            workbench.remote_ui.restore_order = vec![projects, 41, 42];
            workbench.remote_ui.restored_focus = Some(projects);
            let document = workbench.session.open_file(&path).unwrap();

            workbench.finish_remote_open(document).unwrap();
            workbench.finish_remote_restoration().unwrap();
            let group = workbench
                .current_tiling()
                .areas
                .iter()
                .find(|group| group.area == destination)
                .unwrap();
            assert_eq!(group.tabs, vec![41, 42]);
            assert_eq!(group.selected, Some(selected));
            assert_eq!(workbench.session.view_count(document), 2);
            assert_eq!(workbench.active_panel_id(), Some(projects));
            assert!(workbench.remote_ui.restore.is_empty());

            let mut context = Context::create();
            workbench
                .initialize(&mut context, WorkbenchHostMode::Fullscreen)
                .unwrap();
            context
                .font_atlas()
                .try_claim_legacy_renderer()
                .unwrap()
                .build();
            for _ in 0..6 {
                super::super::tests::frame(&mut context, &mut workbench);
            }
            let name = CString::new(format!("###bed_tab_{selected}")).unwrap();
            context.binding().with_bound_context(|| unsafe {
                let window = sys::igFindWindowByName(name.as_ptr());
                assert!(!window.is_null());
                assert!((*window).DockTabIsVisible());
                assert_eq!((*window).DockId, workbench.tiling_ui.docks[&destination]);
            });
            workbench.cleanup().unwrap();
        }
    }

    #[test]
    fn restored_remote_file_drop_selects_its_requested_destination() {
        let dir = TempDir::new();
        let path = dir.write("dropped.txt", b"explicit drop overrides restored placement");
        let mut workbench = super::super::tests::workspace(&dir);
        let projects = workbench.tabs[0].id;
        let settings = workbench
            .open_native_panel("settings", &Value::Null)
            .unwrap();
        let mut layout = Layout::default();
        let destination = layout.split(1, 0, 5000, 1).unwrap();
        workbench.tiling = TilingState::new(layout);
        workbench.tiling.sync_area(1, vec![projects, 41], Some(41));
        workbench.tiling.assign(settings, destination);
        workbench.next_tab = 42;
        workbench.dock_built = true;
        let requested = path.to_string_lossy().into_owned();
        workbench.remote_ui.restore.insert(
            requested.clone(),
            vec![json!({"id":41,"kind":"document","viewer":"bed.text"})],
        );
        workbench.remote_ui.restore_order = vec![projects, settings, 41];
        workbench
            .remote_ui
            .drop_areas
            .insert(requested, destination);
        let document = workbench.session.open_file(&path).unwrap();

        workbench.finish_remote_open(document).unwrap();
        workbench.finish_remote_restoration().unwrap();
        assert_eq!(workbench.area_for_panel(41), Some(destination));
        assert_eq!(workbench.active_panel_id(), Some(41));
        assert_eq!(
            workbench
                .current_tiling()
                .areas
                .iter()
                .find(|group| group.area == destination)
                .unwrap()
                .selected,
            Some(41)
        );
        assert!(workbench.remote_ui.drop_areas.is_empty());
        workbench.cleanup().unwrap();
    }

    #[test]
    fn failed_remote_restore_releases_reserved_ids_and_shows_the_empty_area() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let dir = TempDir::new();
        let mut workbench = super::super::tests::workspace(&dir);
        let mut layout = Layout::default();
        let populated = layout.split(1, 0, 5000, 1).unwrap();
        let mut staged = TilingState::new(layout.clone());
        for tab in &workbench.tabs {
            staged.assign(tab.id, populated);
        }
        staged.assign(100, 1);
        staged.assign(101, 1);
        workbench.pending_tiling = Some(staged);
        workbench.dock_built = true;
        workbench.remote_ui.restore.insert(
            "/missing.txt".into(),
            vec![json!({"id":100,"kind":"document"})],
        );
        workbench.remote_ui.restore.insert(
            "/unsupported.txt".into(),
            vec![json!({"id":101,"kind":"plugin","viewer":"unavailable"})],
        );
        workbench.remote_ui.restore_order = vec![100, 101];
        workbench.remote_ui.restored_focus = Some(100);

        // Neither request is pending anymore: both failed to create a panel.
        workbench.finish_remote_restoration().unwrap();
        assert!(workbench.remote_ui.restore.is_empty());
        assert!(workbench.remote_ui.restore_order.is_empty());
        assert_eq!(workbench.current_tiling().layout, layout);
        assert!(workbench.current_tiling().area_for(100).is_none());
        assert!(workbench.current_tiling().area_for(101).is_none());
        assert!(workbench.current_tiling().areas[0].tabs.is_empty());

        let mut context = Context::create();
        workbench
            .initialize(&mut context, WorkbenchHostMode::Fullscreen)
            .unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        for _ in 0..6 {
            super::super::tests::frame(&mut context, &mut workbench);
        }
        let name = CString::new("###bed_empty_area_1").unwrap();
        context.binding().with_bound_context(|| unsafe {
            let window = sys::igFindWindowByName(name.as_ptr());
            assert!(!window.is_null());
            assert!((*window).Active);
            assert_eq!((*window).DockId, workbench.tiling_ui.docks[&1]);
        });
        workbench.cleanup().unwrap();
    }

    #[test]
    fn unavailable_selected_remote_panel_keeps_the_surviving_group_order() {
        let dir = TempDir::new();
        let mut workbench = super::super::tests::workspace(&dir);
        let projects = workbench.tabs[0].id;
        let settings = workbench
            .open_native_panel("settings", &Value::Null)
            .unwrap();
        let mut state = TilingState::default();
        state.sync_area(1, vec![100, settings, projects], Some(100));
        workbench.tiling = state;
        workbench.dock_built = true;
        workbench.remote_ui.restore_order = vec![100, settings, projects];

        workbench.finish_remote_restoration().unwrap();
        let group = &workbench.current_tiling().areas[0];
        assert_eq!(group.tabs, vec![settings, projects]);
        assert_eq!(group.selected, Some(settings));
        assert_eq!(
            workbench.tabs.iter().map(|tab| tab.id).collect::<Vec<_>>(),
            vec![settings, projects]
        );
        workbench.cleanup().unwrap();
    }

    #[test]
    fn restored_remote_panel_keeps_its_reserved_area_order_and_selection() {
        let dir = TempDir::new();
        let mut workbench = super::super::tests::workspace(&dir);
        let mut layout = Layout::default();
        let destination = layout.split(1, 0, 5000, 1).unwrap();
        workbench.tiling = TilingState::new(layout);
        for tab in &workbench.tabs {
            workbench.tiling.assign(tab.id, 1);
        }
        workbench.tiling.assign(41, destination);
        workbench.tiling.assign(42, destination);
        workbench
            .tiling
            .areas
            .iter_mut()
            .find(|group| group.area == destination)
            .unwrap()
            .selected = Some(42);
        workbench.next_tab = 43;
        workbench.dock_built = true;

        let document = workbench
            .session
            .create_document(b"remote contents")
            .unwrap();
        let temporary = workbench
            .add_document_panel_at(document, Some("bed.text"), &Value::Null, Some(1))
            .unwrap();
        workbench
            .apply_remote_panel_state(&json!({"id":42,"scroll":[12.0,24.0]}))
            .unwrap();

        assert_eq!(workbench.tabs.last().unwrap().id, 42);
        assert_eq!(workbench.area_for_panel(temporary), None);
        assert_eq!(workbench.area_for_panel(42), Some(destination));
        let group = workbench
            .tiling
            .areas
            .iter()
            .find(|group| group.area == destination)
            .unwrap();
        assert_eq!(group.tabs, vec![41, 42]);
        assert_eq!(group.selected, Some(42));
        assert_eq!(workbench.focused, Some(42));
        assert!(workbench.tabs.last().unwrap().dock_next);
        workbench.cleanup().unwrap();
    }

    #[test]
    fn legacy_remote_restore_replaces_the_temporary_id_in_its_current_area() {
        let dir = TempDir::new();
        let mut workbench = super::super::tests::workspace(&dir);
        workbench.tiling = TilingState::default();
        for tab in &workbench.tabs {
            workbench.tiling.assign(tab.id, 1);
        }
        workbench.dock_built = true;
        let document = workbench.session.create_document(b"legacy remote").unwrap();
        let temporary = workbench
            .add_document_panel(document, Some("bed.text"), &Value::Null)
            .unwrap();
        let destination = workbench.area_for_panel(temporary).unwrap();
        workbench
            .apply_remote_panel_state(&json!({"id":100}))
            .unwrap();

        assert_eq!(workbench.area_for_panel(temporary), None);
        assert_eq!(workbench.area_for_panel(100), Some(destination));
        assert_eq!(workbench.tabs.last().unwrap().id, 100);
        assert_eq!(workbench.focused, Some(100));
        assert!(workbench.next_tab > 100);
        workbench.cleanup().unwrap();
    }

    #[test]
    fn asynchronous_file_open_consumes_its_requested_drop_area() {
        let dir = TempDir::new();
        let path = dir.write("remote-arrival.txt", b"loaded by the document worker");
        let mut workbench = super::super::tests::workspace(&dir);
        let mut layout = Layout::default();
        let destination = layout.split(1, 0, 8000, 1).unwrap();
        workbench.tiling = TilingState::new(layout);
        for tab in &workbench.tabs {
            workbench.tiling.assign(tab.id, 1);
        }
        workbench.dock_built = true;
        workbench
            .remote_ui
            .drop_areas
            .insert(path.to_string_lossy().into_owned(), destination);
        let document = workbench.session.open_file(&path).unwrap();
        workbench.finish_remote_open(document).unwrap();

        let panel = workbench.active_panel_id().unwrap();
        assert_eq!(workbench.area_for_panel(panel), Some(destination));
        assert_eq!(
            workbench.tabs.last().unwrap().panel.document(),
            Some(document)
        );
        assert!(workbench.remote_ui.drop_areas.is_empty());
        workbench.cleanup().unwrap();
    }

    #[test]
    fn closing_an_area_suppresses_reserved_and_unreserved_remote_arrivals() {
        for reserved in [false, true] {
            let dir = TempDir::new();
            let path = dir.write("cancelled-arrival.txt", b"the worker has finished loading");
            let requested = path.to_string_lossy().into_owned();
            let mut workbench = super::super::tests::workspace(&dir);
            let projects = workbench.tabs[0].id;
            let mut layout = Layout::default();
            let closed = layout.split(1, 0, 5000, 1).unwrap();
            let plan = layout.plan_join(1, closed, [1; 2]).unwrap();
            workbench.tiling = TilingState::new(layout);
            workbench.tiling.assign(projects, 1);
            if reserved {
                workbench.tiling.assign(100, closed);
                workbench.remote_ui.restore.insert(
                    requested.clone(),
                    vec![json!({"id":100,"kind":"document","viewer":"bed.text"})],
                );
            }
            workbench.dock_built = true;
            workbench.remote_ui.opening.insert(requested.clone());
            workbench
                .remote_ui
                .drop_areas
                .insert(requested.clone(), closed);
            workbench
                .remote_ui
                .navigation
                .insert(requested.clone(), (2, 3, false));
            workbench
                .modules
                .pending_open
                .insert(requested.clone(), Some("bed.text".into()));
            workbench.remote_ui.restore_order = vec![projects, 100];
            workbench.remote_ui.restored_focus = Some(100);
            workbench.remote_ui.restored_active = Some(100);
            let document = workbench.session.open_file(&path).unwrap();

            workbench.cancel_remote_area_contents(&HashSet::from([closed]), &HashSet::from([100]));
            assert_eq!(workbench.remote_ui.restore[&requested], Vec::<Value>::new());
            assert!(!workbench.remote_ui.drop_areas.contains_key(&requested));
            assert!(!workbench.remote_ui.navigation.contains_key(&requested));
            assert!(!workbench.modules.pending_open.contains_key(&requested));
            assert_eq!(workbench.remote_ui.restore_order, vec![projects]);
            assert!(workbench.remote_ui.restored_focus.is_none());
            assert!(workbench.remote_ui.restored_active.is_none());
            workbench.tiling.remove(100);
            workbench.tiling.apply_plan(plan);

            workbench.finish_remote_open(document).unwrap();
            workbench.finish_remote_restoration().unwrap();
            assert_eq!(
                workbench.tabs.iter().map(|tab| tab.id).collect::<Vec<_>>(),
                vec![projects]
            );
            assert!(
                workbench.session.snapshot(document).is_err(),
                "an unused completed document is closed"
            );
            assert_eq!(workbench.session.document_for_path(&path), None);
            assert!(workbench.remote_ui.restore.is_empty());
            assert!(workbench.remote_ui.opening.is_empty());
            workbench.cleanup().unwrap();
        }
    }

    #[test]
    fn closing_one_reserved_area_keeps_another_restored_view_of_the_same_path() {
        let dir = TempDir::new();
        let path = dir.write("shared-arrival.txt", b"shared document, surviving view");
        let requested = path.to_string_lossy().into_owned();
        let mut workbench = super::super::tests::workspace(&dir);
        let projects = workbench.tabs[0].id;
        let mut layout = Layout::default();
        let closed = layout.split(1, 0, 5000, 1).unwrap();
        let plan = layout.plan_join(1, closed, [1; 2]).unwrap();
        workbench.tiling = TilingState::new(layout);
        workbench
            .tiling
            .sync_area(1, vec![projects, 101], Some(101));
        workbench.tiling.assign(100, closed);
        workbench.next_tab = 102;
        workbench.dock_built = true;
        workbench.remote_ui.restore.insert(
            requested.clone(),
            vec![
                json!({"id":100,"kind":"document","viewer":"bed.text"}),
                json!({"id":101,"kind":"document","viewer":"bed.text"}),
            ],
        );
        workbench.remote_ui.restore_order = vec![projects, 100, 101];
        workbench.remote_ui.restored_focus = Some(100);
        workbench.remote_ui.restored_active = Some(101);
        let document = workbench.session.open_file(&path).unwrap();

        workbench.cancel_remote_area_contents(&HashSet::from([closed]), &HashSet::from([100]));
        assert_eq!(workbench.remote_ui.restore[&requested].len(), 1);
        assert_eq!(workbench.remote_ui.restore[&requested][0]["id"], json!(101));
        assert_eq!(workbench.remote_ui.restore_order, vec![projects, 101]);
        assert_eq!(workbench.remote_ui.restored_active, Some(101));
        workbench.tiling.remove(100);
        workbench.tiling.apply_plan(plan);

        workbench.finish_remote_open(document).unwrap();
        workbench.finish_remote_restoration().unwrap();
        assert_eq!(workbench.area_for_panel(100), None);
        assert_eq!(workbench.area_for_panel(101), Some(1));
        assert_eq!(workbench.session.view_count(document), 1);
        assert!(workbench.session.snapshot(document).is_ok());
        assert!(workbench.remote_ui.restore.is_empty());
        assert_eq!(
            workbench.active_view(),
            workbench.tabs.last().unwrap().panel.view_id()
        );
        workbench.cleanup().unwrap();
    }

    #[test]
    fn cancelled_remote_drop_keeps_a_document_already_used_by_a_surviving_panel() {
        let dir = TempDir::new();
        let path = dir.write("already-visible.txt", b"existing document view");
        let requested = path.to_string_lossy().into_owned();
        let mut workbench = super::super::tests::workspace(&dir);
        workbench.open_or_focus(&path).unwrap();
        let document = workbench.active_document().unwrap();
        let live = workbench.tabs.iter().map(|tab| tab.id).collect::<Vec<_>>();
        let mut layout = Layout::default();
        let closed = layout.split(1, 0, 5000, 1).unwrap();
        let plan = layout.plan_join(1, closed, [1; 2]).unwrap();
        workbench.tiling = TilingState::new(layout);
        for id in &live {
            workbench.tiling.assign(*id, 1);
        }
        workbench.dock_built = true;
        workbench.remote_ui.drop_areas.insert(requested, closed);

        workbench.cancel_remote_area_contents(&HashSet::from([closed]), &HashSet::new());
        workbench.tiling.apply_plan(plan);
        workbench.finish_remote_open(document).unwrap();

        assert_eq!(
            workbench.tabs.iter().map(|tab| tab.id).collect::<Vec<_>>(),
            live
        );
        assert_eq!(workbench.session.view_count(document), 1);
        assert!(workbench.session.snapshot(document).is_ok());
        assert!(workbench.remote_ui.restore.is_empty());
        workbench.cleanup().unwrap();
    }
}
