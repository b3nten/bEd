//! Workspace ownership of language-server processes and document bindings.
//! Filesystem workers report discoveries; all process and binding transitions
//! happen here on the host's polling thread.
use crate::{
    diagnostics::LspDiagnostics,
    jsonrpc::{REQUEST_CANCELLED, ResponseError, RpcId},
    lsp_client::{LspClient, SharedLspClient, WorkDoneProgress},
    lsp_config::{LanguageConfiguration, LspConfig},
    lsp_project::{layered_config, resolve_root},
    message_handler::RpcEvent,
};
pub use bed_editing::identity::{DocumentId, ViewId, WorkspaceId};
use bed_editing::util::doc_path;
use bed_remote::RemoteClient;
use serde_json::Value;
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    io,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread,
};

fn next_server_generation() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ServerInstanceId {
    pub server: String,
    pub root: PathBuf,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LspRequestOrigin {
    pub workspace_id: WorkspaceId,
    pub server_generation: u64,
    pub document_id: DocumentId,
    pub document_generation: u64,
    pub version: i32,
    pub view_id: ViewId,
    pub ticket: u64,
}
#[derive(Debug)]
pub struct RoutedLspResponse {
    pub origin: LspRequestOrigin,
    pub result: Result<Value, ResponseError>,
}
#[derive(Debug)]
pub enum WorkspaceLspEvent {
    Rpc {
        language: String,
        server_generation: u64,
        event: RpcEvent,
    },
    DocumentError {
        document_id: DocumentId,
        message: String,
    },
}
#[derive(Clone, Debug)]
pub struct WorkspaceServerStatus {
    pub language: String,
    pub instance: ServerInstanceId,
    pub path: Option<PathBuf>,
    pub process_id: Option<u32>,
    pub initialized: bool,
    pub last_error: Option<String>,
    pub stderr: String,
    pub generation: u64,
    pub progress: Vec<WorkDoneProgress>,
}
struct ServerSlot {
    client: SharedLspClient,
    generation: Rc<Cell<u64>>,
}
struct RegisteredDocument {
    path: String,
    bytes: Vec<u8>,
    version: i32,
    language: String,
    language_id: String,
    instance: Option<ServerInstanceId>,
    registration: u64,
    resolve_pending: bool,
    resolve_queued: bool,
}

enum DiscoveryJob {
    Config {
        epoch: u64,
        path: PathBuf,
        root: PathBuf,
        remote: Option<RemoteClient>,
    },
    Root {
        epoch: u64,
        document: DocumentId,
        registration: u64,
        parent: PathBuf,
        workspace: PathBuf,
        language: LanguageConfiguration,
        remote: Option<RemoteClient>,
    },
    Clear,
}
enum DiscoveryResult {
    Config {
        epoch: u64,
        result: io::Result<(LspConfig, LspConfig)>,
    },
    Root {
        epoch: u64,
        document: DocumentId,
        registration: u64,
        result: io::Result<PathBuf>,
    },
}
struct DiscoveryWorker {
    sender: SyncSender<DiscoveryJob>,
    receiver: Receiver<DiscoveryResult>,
    stop: Arc<AtomicBool>,
}
impl DiscoveryWorker {
    fn new() -> Self {
        let (sender, jobs) = mpsc::sync_channel(32);
        let (results, receiver) = mpsc::sync_channel(32);
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        thread::spawn(move || {
            let mut listings = BTreeMap::<PathBuf, Vec<OsString>>::new();
            while let Ok(job) = jobs.recv() {
                if stopped.load(Ordering::Acquire) {
                    break;
                }
                let result = match job {
                    DiscoveryJob::Config {
                        epoch,
                        path,
                        root,
                        remote,
                    } => {
                        listings.clear();
                        DiscoveryResult::Config {
                            epoch,
                            result: layered_config(&path, &root, remote.as_ref()),
                        }
                    }
                    DiscoveryJob::Root {
                        epoch,
                        document,
                        registration,
                        parent,
                        workspace,
                        language,
                        remote,
                    } => DiscoveryResult::Root {
                        epoch,
                        document,
                        registration,
                        result: resolve_root(
                            &parent,
                            &workspace,
                            &language,
                            remote.as_ref(),
                            &mut listings,
                        ),
                    },
                    DiscoveryJob::Clear => {
                        listings.clear();
                        continue;
                    }
                };
                if stopped.load(Ordering::Acquire) || results.send(result).is_err() {
                    break;
                }
            }
        });
        Self {
            sender,
            receiver,
            stop,
        }
    }
}
impl Drop for DiscoveryWorker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

pub struct WorkspaceLsp {
    ssh_target: Option<bed_remote::SshTarget>,
    remote_client: Option<RemoteClient>,
    workspace_id: WorkspaceId,
    config_path: PathBuf,
    root: PathBuf,
    config: LspConfig,
    outside_config: LspConfig,
    config_error: Option<String>,
    servers: BTreeMap<ServerInstanceId, ServerSlot>,
    documents: BTreeMap<DocumentId, RegisteredDocument>,
    indexed_paths: BTreeSet<String>,
    events: Vec<WorkspaceLspEvent>,
    project_check_owner: bool,
    file_observations: bool,
    discovery: Option<DiscoveryWorker>,
    epoch: u64,
    config_ready: bool,
    pending_config: Option<DiscoveryJob>,
    config_queued: bool,
    clear_cache_pending: bool,
}
impl WorkspaceLsp {
    /// Exact supplied configuration, preserving the embedded-host contract.
    pub fn new(workspace_id: WorkspaceId, config_path: PathBuf, root: PathBuf) -> Self {
        let mut workspace = Self::unloaded(workspace_id, config_path, root);
        match LspConfig::load(&workspace.config_path) {
            Ok(config) => {
                workspace.outside_config = config.clone();
                workspace.config = config;
            }
            Err(error) => workspace.config_error = Some(error.to_string()),
        }
        workspace
    }
    fn unloaded(workspace_id: WorkspaceId, config_path: PathBuf, root: PathBuf) -> Self {
        Self {
            ssh_target: None,
            remote_client: None,
            workspace_id,
            config_path,
            root,
            outside_config: LspConfig::default(),
            config: LspConfig::default(),
            config_error: None,
            servers: BTreeMap::new(),
            documents: BTreeMap::new(),
            indexed_paths: BTreeSet::new(),
            events: Vec::new(),
            project_check_owner: false,
            file_observations: false,
            discovery: None,
            epoch: next_server_generation(),
            config_ready: true,
            pending_config: None,
            config_queued: false,
            clear_cache_pending: false,
        }
    }
    pub fn new_remote(
        workspace_id: WorkspaceId,
        config_path: PathBuf,
        root: PathBuf,
        target: bed_remote::SshTarget,
    ) -> Self {
        let mut workspace = Self::new(workspace_id, config_path, root);
        workspace.ssh_target = Some(target);
        workspace
    }
    pub fn new_layered(workspace_id: WorkspaceId, config_path: PathBuf, root: PathBuf) -> Self {
        let root = PathBuf::from(doc_path::normalize(&root.to_string_lossy()));
        let mut workspace = Self::unloaded(workspace_id, config_path, root);
        workspace.begin_layered(None);
        workspace
    }
    pub fn new_remote_layered(
        workspace_id: WorkspaceId,
        config_path: PathBuf,
        root: PathBuf,
        target: bed_remote::SshTarget,
        remote: RemoteClient,
    ) -> Self {
        let mut workspace = Self::unloaded(workspace_id, config_path, root);
        workspace.ssh_target = Some(target);
        workspace.begin_layered(Some(remote));
        workspace
    }
    fn begin_layered(&mut self, remote: Option<RemoteClient>) {
        self.remote_client = remote;
        self.config = LspConfig::default();
        self.outside_config = LspConfig::default();
        self.config_error = None;
        self.config_ready = false;
        self.discovery = Some(DiscoveryWorker::new());
        self.queue_config();
    }
    fn queue_config(&mut self) {
        self.epoch = next_server_generation();
        for document in self
            .documents
            .values_mut()
            .filter(|document| document.resolve_pending)
        {
            document.registration = next_server_generation();
            document.resolve_queued = false;
        }
        self.pending_config = Some(DiscoveryJob::Config {
            epoch: self.epoch,
            path: self.config_path.clone(),
            root: self.root.clone(),
            remote: self.remote_client.clone(),
        });
        self.config_queued = false;
        self.submit_discovery();
    }
    pub fn configuration_sources(&self) -> Vec<PathBuf> {
        if self.discovery.is_some() {
            vec![
                PathBuf::from("<bundled>/lsp.json"),
                self.config_path.clone(),
                self.root.join(".bed/lsp.json"),
            ]
        } else {
            vec![self.config_path.clone()]
        }
    }
    pub fn is_discovering(&self) -> bool {
        self.pending_config.is_some()
            || self.config_queued
            || self
                .documents
                .values()
                .any(|document| document.resolve_pending)
    }
    pub fn ssh_target(&self) -> Option<&bed_remote::SshTarget> {
        self.ssh_target.as_ref()
    }
    fn document_key(&self, path: &str) -> String {
        if self.ssh_target.is_some() {
            path.to_owned()
        } else {
            doc_path::normalize(path)
        }
    }
    pub fn workspace_id(&self) -> WorkspaceId {
        self.workspace_id
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn config(&self) -> &LspConfig {
        &self.config
    }
    pub fn config_path(&self) -> &Path {
        &self.config_path
    }
    pub fn config_error(&self) -> Option<&str> {
        self.config_error.as_deref()
    }
    fn config_for_path(&self, path: &str) -> &LspConfig {
        if self.discovery.is_some() && !Path::new(path).starts_with(&self.root) {
            &self.outside_config
        } else {
            &self.config
        }
    }
    pub fn set_server_arguments(&mut self, language: &str, arguments: Vec<String>) {
        let id = self
            .config
            .server_id(language)
            .unwrap_or(language)
            .to_owned();
        if let Some(server) = self.config.language_servers.get_mut(&id) {
            server.args = arguments.clone();
        }
        if let Some(server) = self.outside_config.language_servers.get_mut(&id) {
            server.args = arguments.clone();
        }
        for (instance, slot) in &self.servers {
            if instance.server == id {
                slot.client
                    .borrow_mut()
                    .set_server_arguments(&id, arguments.clone());
            }
        }
    }
    fn ensure_slot(&mut self, instance: &ServerInstanceId, outside: bool) -> &mut ServerSlot {
        self.servers.entry(instance.clone()).or_insert_with(|| {
            let config = if outside {
                &self.outside_config
            } else {
                &self.config
            };
            let mut client = LspClient::with_config(&self.config_path, config.clone());
            client.set_ssh_target(self.ssh_target.clone());
            client.set_workspace(&instance.root.to_string_lossy());
            client.set_project_check_owner(self.project_check_owner);
            client.set_file_observations_available(self.file_observations);
            ServerSlot {
                client: Rc::new(RefCell::new(client)),
                generation: Rc::new(Cell::new(next_server_generation())),
            }
        })
    }
    pub fn register_document(
        &mut self,
        id: DocumentId,
        path: &str,
        bytes: &[u8],
        version: i32,
        language_id: &str,
    ) -> io::Result<()> {
        if self.documents.contains_key(&id) {
            return self.update_document_snapshot(id, path, bytes, version, language_id);
        }
        let path = self.document_key(path);
        if self
            .documents
            .values()
            .any(|document| document.path == path)
        {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "Document path already registered in this workspace",
            ));
        }
        self.documents.insert(
            id,
            RegisteredDocument {
                path,
                bytes: bytes.to_owned(),
                version,
                language: String::new(),
                language_id: language_id.to_owned(),
                instance: None,
                registration: next_server_generation(),
                resolve_pending: false,
                resolve_queued: false,
            },
        );
        if self.config_ready {
            self.prepare_document(id)?;
        }
        self.submit_discovery();
        Ok(())
    }
    fn prepare_document(&mut self, id: DocumentId) -> io::Result<()> {
        let document = &self.documents[&id];
        let language = self
            .config_for_path(&document.path)
            .detect_language_with_content(&document.path, &document.bytes)
            .cloned();
        let Some(language) = language else {
            return Ok(());
        };
        let server = language.language_server.clone();
        let document = self.documents.get_mut(&id).unwrap();
        document.language = language.name;
        document.language_id = language.language_id;
        if let Some(server) = server {
            if self.discovery.is_some() {
                document.resolve_pending = true;
                document.resolve_queued = false;
            } else {
                let instance = ServerInstanceId {
                    server,
                    root: self.root.clone(),
                };
                self.bind_document(id, instance)?;
            }
        }
        Ok(())
    }
    fn bind_document(&mut self, id: DocumentId, instance: ServerInstanceId) -> io::Result<()> {
        let document = self.documents.get_mut(&id).unwrap();
        document.resolve_pending = false;
        document.resolve_queued = false;
        if document.instance.as_ref() == Some(&instance) {
            return Ok(());
        }
        if let Some(previous) = document.instance.take() {
            if let Some(slot) = self.servers.get(&previous) {
                slot.client.borrow_mut().did_close(&document.path)?;
            }
        }
        document.instance = Some(instance.clone());
        let path = document.path.clone();
        let bytes = document.bytes.clone();
        let version = document.version;
        let language_id = document.language_id.clone();
        let outside = self.discovery.is_some() && !Path::new(&path).starts_with(&self.root);
        let result = {
            let mut client = self.ensure_slot(&instance, outside).client.borrow_mut();
            client
                .start_server(&instance.server, "")
                .and_then(|started| {
                    if started {
                        client.did_open(&path, &bytes, version, &language_id)
                    } else {
                        Ok(())
                    }
                })
        };
        if let Err(error) = result {
            self.events.push(WorkspaceLspEvent::DocumentError {
                document_id: id,
                message: error.to_string(),
            });
        }
        Ok(())
    }
    /// Snapshots support retries and edits made before asynchronous discovery.
    /// The editor's listeners remain the sole source of ordinary didChange/save.
    pub fn update_document_snapshot(
        &mut self,
        id: DocumentId,
        path: &str,
        bytes: &[u8],
        version: i32,
        language_id: &str,
    ) -> io::Result<()> {
        let path = self.document_key(path);
        let language = self
            .config_for_path(&path)
            .detect_language_with_content(&path, bytes)
            .map(|language| language.name.clone())
            .unwrap_or_default();
        let Some(document) = self.documents.get_mut(&id) else {
            return self.register_document(id, &path, bytes, version, language_id);
        };
        if document.path != path || (self.config_ready && document.language != language) {
            self.unregister_document(id)?;
            return self.register_document(id, &path, bytes, version, language_id);
        }
        document.bytes.clear();
        document.bytes.extend_from_slice(bytes);
        document.version = version;
        Ok(())
    }
    pub fn unregister_document(&mut self, id: DocumentId) -> io::Result<()> {
        if let Some(document) = self.documents.remove(&id) {
            if let Some(instance) = document.instance {
                if let Some(slot) = self.servers.get(&instance) {
                    slot.client.borrow_mut().did_close(&document.path)?;
                }
            }
        }
        Ok(())
    }
    pub fn client_for_document(&self, id: DocumentId) -> Option<SharedLspClient> {
        self.client_for_instance(self.documents.get(&id)?.instance.as_ref()?)
    }
    pub fn client_for_instance(&self, id: &ServerInstanceId) -> Option<SharedLspClient> {
        self.servers.get(id).map(|slot| slot.client.clone())
    }
    pub fn client_for_path(&self, path: &str) -> Option<SharedLspClient> {
        let path = self.document_key(path);
        let document = self
            .documents
            .iter()
            .find(|(_, document)| document.path == path)?
            .0;
        self.client_for_document(*document)
    }
    /// Compatibility for single-root hosts. Instance-based lookup is precise.
    pub fn client_for_language(&self, language: &str) -> Option<SharedLspClient> {
        let server = self.config.server_id(language).unwrap_or(language);
        self.servers
            .iter()
            .find(|(id, _)| id.server == server)
            .map(|(_, slot)| slot.client.clone())
    }
    pub fn diagnostics_for_document(&self, id: DocumentId) -> Option<LspDiagnostics> {
        self.client_for_document(id)
            .map(|client| client.borrow().diagnostics())
    }
    pub fn set_project_check_owner(&mut self, enabled: bool) {
        self.project_check_owner = enabled;
        for slot in self.servers.values() {
            slot.client.borrow_mut().set_project_check_owner(enabled);
        }
    }
    pub fn set_file_observations_available(&mut self, available: bool) {
        self.file_observations = available;
        for slot in self.servers.values() {
            slot.client
                .borrow_mut()
                .set_file_observations_available(available);
        }
    }
    pub fn refresh_workspace_diagnostics(&mut self) {
        for slot in self.servers.values() {
            slot.client.borrow_mut().refresh_workspace_diagnostics();
        }
    }
    /// Index facts inform coverage without launching additional processes.
    pub fn note_project_paths(&mut self, paths: &[String]) {
        for path in paths {
            self.indexed_paths.insert(self.document_key(path));
        }
    }
    pub fn observe_file_changes(
        &mut self,
        changes: &[bed_remote::FilesystemChange],
    ) -> io::Result<()> {
        let changes: Vec<_> = changes
            .iter()
            .map(|change| {
                use bed_remote::FilesystemChange::*;
                match change {
                    Created { path } => Created {
                        path: self.document_key(path),
                    },
                    Modified { path } => Modified {
                        path: self.document_key(path),
                    },
                    Removed { path } => Removed {
                        path: self.document_key(path),
                    },
                    Renamed { from, to } => Renamed {
                        from: self.document_key(from),
                        to: self.document_key(to),
                    },
                }
            })
            .collect();
        for change in &changes {
            match change {
                bed_remote::FilesystemChange::Created { path }
                | bed_remote::FilesystemChange::Modified { path } => {
                    self.indexed_paths.insert(path.clone());
                }
                bed_remote::FilesystemChange::Removed { path } => {
                    self.indexed_paths
                        .retain(|candidate| !Path::new(candidate).starts_with(path));
                }
                bed_remote::FilesystemChange::Renamed { from, to } => {
                    self.indexed_paths
                        .retain(|candidate| !Path::new(candidate).starts_with(from));
                    self.indexed_paths.insert(to.clone());
                }
            }
        }
        for slot in self.servers.values() {
            slot.client.borrow_mut().observe_file_changes(&changes)?;
        }
        if self.discovery.is_some() {
            // Root markers and boundaries can change independently of documents.
            // Clear cached directory listings; future bindings see current facts.
            self.clear_cache_pending = true;
            let paths: Vec<&str> = changes
                .iter()
                .flat_map(|change| match change {
                    bed_remote::FilesystemChange::Created { path }
                    | bed_remote::FilesystemChange::Modified { path }
                    | bed_remote::FilesystemChange::Removed { path } => vec![path.as_str()],
                    bed_remote::FilesystemChange::Renamed { from, to } => {
                        vec![from.as_str(), to.as_str()]
                    }
                })
                .collect();
            for document in self.documents.values_mut() {
                let config = if Path::new(&document.path).starts_with(&self.root) {
                    &self.config
                } else {
                    &self.outside_config
                };
                let Some(language) = config.language(&document.language) else {
                    continue;
                };
                if paths.iter().any(|path| {
                    let path = Path::new(path);
                    path.parent()
                        .is_some_and(|parent| Path::new(&document.path).starts_with(parent))
                        && path.file_name().is_some_and(|name| {
                            language.roots.iter().any(|marker| {
                                globset::Glob::new(marker)
                                    .expect("validated root glob")
                                    .compile_matcher()
                                    .is_match(name)
                            })
                        })
                }) {
                    document.registration = next_server_generation();
                    document.resolve_pending = true;
                    document.resolve_queued = false;
                }
            }
            self.submit_discovery();
        }
        Ok(())
    }
    pub fn diagnostic_coverage_complete(&self) -> bool {
        !self.servers.is_empty()
            && !self.is_discovering()
            && self
                .servers
                .values()
                .all(|slot| slot.client.borrow().workspace_diagnostics_complete())
            && self.indexed_paths.iter().all(|path| {
                let config = self.config_for_path(path);
                let language = config.detect_language(path);
                let Some(server) = config.server_id(&language) else {
                    return true;
                };
                self.servers.keys().any(|instance| {
                    instance.server == server && Path::new(path).starts_with(&instance.root)
                })
            })
    }
    pub fn diagnostic_snapshot(&self) -> BTreeMap<String, Vec<crate::diagnostics::DiagnosticItem>> {
        let mut result = BTreeMap::<String, Vec<crate::diagnostics::DiagnosticItem>>::new();
        for slot in self.servers.values() {
            for (path, items) in slot.client.borrow().diagnostics().snapshot() {
                let entries = result.entry(path).or_default();
                for item in items {
                    if !entries.contains(&item) {
                        entries.push(item);
                    }
                }
            }
        }
        result
    }
    pub fn clear_diagnostics(&self, path: &str) {
        for slot in self.servers.values() {
            slot.client.borrow().diagnostics().clear(path);
        }
    }
    /// Explicit prewarming for snapshot hosts. Desktop index updates never call it.
    pub fn start_project_languages(&mut self, paths: &[String]) {
        if self.discovery.is_some() {
            return;
        }
        for path in paths {
            if let Some(server) = self
                .config
                .server_id(&self.config.detect_language(path))
                .map(str::to_owned)
            {
                let instance = ServerInstanceId {
                    server,
                    root: self.root.clone(),
                };
                if !self.servers.contains_key(&instance) {
                    let _ = self
                        .ensure_slot(&instance, false)
                        .client
                        .borrow_mut()
                        .start_server(&instance.server, "");
                }
            }
        }
    }
    fn submit_discovery(&mut self) {
        let Some(worker) = &self.discovery else {
            return;
        };
        if let Some(job) = self.pending_config.take() {
            match worker.sender.try_send(job) {
                Ok(()) => self.config_queued = true,
                Err(mpsc::TrySendError::Full(job)) => {
                    self.pending_config = Some(job);
                    return;
                }
                Err(mpsc::TrySendError::Disconnected(_)) => {
                    self.config_error = Some("LSP discovery worker stopped".into());
                    return;
                }
            }
        }
        if self.config_queued || !self.config_ready {
            return;
        }
        if self.clear_cache_pending {
            if worker.sender.try_send(DiscoveryJob::Clear).is_err() {
                return;
            }
            self.clear_cache_pending = false;
        }
        for (&id, document) in &mut self.documents {
            if !document.resolve_pending || document.resolve_queued {
                continue;
            }
            let config = if Path::new(&document.path).starts_with(&self.root) {
                &self.config
            } else {
                &self.outside_config
            };
            let Some(language) = config.language(&document.language).cloned() else {
                continue;
            };
            let parent = Path::new(&document.path)
                .parent()
                .unwrap_or(&self.root)
                .to_owned();
            let job = DiscoveryJob::Root {
                epoch: self.epoch,
                document: id,
                registration: document.registration,
                parent,
                workspace: self.root.clone(),
                language,
                remote: self.remote_client.clone(),
            };
            match worker.sender.try_send(job) {
                Ok(()) => document.resolve_queued = true,
                Err(mpsc::TrySendError::Full(_)) => break,
                Err(mpsc::TrySendError::Disconnected(_)) => {
                    self.config_error = Some("LSP discovery worker stopped".into());
                    break;
                }
            }
        }
    }
    fn poll_discovery(&mut self) {
        loop {
            let result = self
                .discovery
                .as_ref()
                .and_then(|worker| worker.receiver.try_recv().ok());
            let Some(result) = result else {
                break;
            };
            match result {
                DiscoveryResult::Config { epoch, result } if epoch == self.epoch => {
                    self.config_queued = false;
                    match result {
                        Ok((config, outside)) => {
                            self.apply_config(config, outside);
                        }
                        Err(error) => {
                            self.config_error = Some(error.to_string());
                        }
                    }
                }
                DiscoveryResult::Root {
                    epoch,
                    document,
                    registration,
                    result,
                } if epoch == self.epoch => {
                    if !self
                        .documents
                        .get(&document)
                        .is_some_and(|doc| doc.registration == registration && doc.resolve_pending)
                    {
                        continue;
                    }
                    match result {
                        Ok(root) => {
                            let doc = &self.documents[&document];
                            let server = self
                                .config_for_path(&doc.path)
                                .server_id(&doc.language)
                                .map(str::to_owned);
                            if let Some(server) = server {
                                if let Err(error) =
                                    self.bind_document(document, ServerInstanceId { server, root })
                                {
                                    self.events.push(WorkspaceLspEvent::DocumentError {
                                        document_id: document,
                                        message: error.to_string(),
                                    });
                                }
                            }
                        }
                        Err(error) => {
                            let doc = self.documents.get_mut(&document).unwrap();
                            doc.resolve_pending = false;
                            doc.resolve_queued = false;
                            self.events.push(WorkspaceLspEvent::DocumentError {
                                document_id: document,
                                message: format!("LSP root discovery: {error}"),
                            });
                        }
                    }
                }
                _ => {}
            }
        }
        self.submit_discovery();
    }
    pub fn poll(&mut self) -> Vec<WorkspaceLspEvent> {
        self.poll_discovery();
        let mut events = std::mem::take(&mut self.events);
        for (instance, slot) in &self.servers {
            events.extend(slot.client.borrow_mut().poll().into_iter().map(|event| {
                WorkspaceLspEvent::Rpc {
                    language: instance.server.clone(),
                    server_generation: slot.generation.get(),
                    event,
                }
            }));
        }
        events
    }
    pub fn retry_language(&mut self, language: &str) -> io::Result<bool> {
        let server = self
            .config
            .server_id(language)
            .unwrap_or(language)
            .to_owned();
        if self.config.server(&server).is_none() {
            return Ok(false);
        }
        let instances: Vec<_> = self
            .servers
            .keys()
            .filter(|id| id.server == server)
            .cloned()
            .collect();
        if instances.is_empty() {
            return self.retry_server(&ServerInstanceId {
                server,
                root: self.root.clone(),
            });
        }
        let mut started = false;
        for instance in instances {
            started |= self.retry_server(&instance)?;
        }
        Ok(started)
    }
    pub fn retry_server(&mut self, instance: &ServerInstanceId) -> io::Result<bool> {
        if self.config.server(&instance.server).is_none()
            && self.outside_config.server(&instance.server).is_none()
        {
            return Ok(false);
        }
        let outside = self.discovery.is_some() && !instance.root.starts_with(&self.root);
        let slot = self.ensure_slot(instance, outside);
        slot.generation.set(next_server_generation());
        let client = slot.client.clone();
        let mut client = client.borrow_mut();
        client.shutdown();
        if !client.start_server(&instance.server, "")? {
            return Ok(false);
        }
        for document in self
            .documents
            .values()
            .filter(|document| document.instance.as_ref() == Some(instance))
        {
            client.did_open(
                &document.path,
                &document.bytes,
                document.version,
                &document.language_id,
            )?;
        }
        Ok(true)
    }
    fn apply_config(&mut self, config: LspConfig, outside: LspConfig) {
        self.shutdown();
        self.servers.clear();
        self.config = config;
        self.outside_config = outside;
        self.config_error = None;
        self.config_ready = true;
        let ids: Vec<_> = self.documents.keys().copied().collect();
        for id in ids {
            let document = self.documents.get_mut(&id).unwrap();
            document.instance = None;
            document.registration = next_server_generation();
            document.resolve_pending = false;
            document.resolve_queued = false;
            document.language.clear();
            if let Err(error) = self.prepare_document(id) {
                self.events.push(WorkspaceLspEvent::DocumentError {
                    document_id: id,
                    message: error.to_string(),
                });
            }
        }
    }
    pub fn reload_config(&mut self) -> io::Result<()> {
        if self.discovery.is_some() {
            self.queue_config();
            return Ok(());
        }
        let config = match LspConfig::load(&self.config_path) {
            Ok(config) => config,
            Err(error) => {
                self.config_error = Some(error.to_string());
                return Err(error);
            }
        };
        self.apply_config(config.clone(), config);
        Ok(())
    }
    pub fn server_statuses(&self) -> Vec<WorkspaceServerStatus> {
        let mut statuses = Vec::new();
        for (instance, slot) in &self.servers {
            let client = slot.client.borrow();
            statuses.push(WorkspaceServerStatus {
                language: instance.server.clone(),
                instance: instance.clone(),
                path: client.find_server_path(&instance.server),
                process_id: client.process_id(),
                initialized: client.is_initialized(),
                last_error: client.last_error().map(str::to_owned),
                stderr: client.stderr_text(),
                generation: slot.generation.get(),
                progress: client.progress(),
            });
        }
        for (server, configured) in &self.config.language_servers {
            if self
                .servers
                .keys()
                .any(|instance| instance.server == *server)
            {
                continue;
            }
            statuses.push(WorkspaceServerStatus {
                language: server.clone(),
                instance: ServerInstanceId {
                    server: server.clone(),
                    root: self.root.clone(),
                },
                path: if self.ssh_target.is_some() {
                    configured.command.first().map(PathBuf::from)
                } else {
                    self.config.find_server_path(server)
                },
                process_id: None,
                initialized: false,
                last_error: None,
                stderr: String::new(),
                generation: 0,
                progress: Vec::new(),
            });
        }
        statuses.sort_by(|a, b| a.instance.cmp(&b.instance));
        statuses
    }
    pub fn request_origin(
        &self,
        id: DocumentId,
        view_id: ViewId,
        document_generation: u64,
        ticket: u64,
    ) -> Option<LspRequestOrigin> {
        let document = self.documents.get(&id)?;
        let slot = self.servers.get(document.instance.as_ref()?)?;
        Some(LspRequestOrigin {
            workspace_id: self.workspace_id,
            server_generation: slot.generation.get(),
            document_id: id,
            document_generation,
            version: document.version,
            view_id,
            ticket,
        })
    }
    pub fn origin_is_current(&self, origin: &LspRequestOrigin) -> bool {
        origin.workspace_id == self.workspace_id
            && self
                .documents
                .get(&origin.document_id)
                .is_some_and(|document| {
                    document.version == origin.version
                        && document
                            .instance
                            .as_ref()
                            .and_then(|id| self.servers.get(id))
                            .is_some_and(|slot| slot.generation.get() == origin.server_generation)
                })
    }
    pub fn send_request(
        &mut self,
        origin: LspRequestOrigin,
        method: &str,
        params: Value,
        callback: impl FnOnce(RoutedLspResponse) + 'static,
    ) -> io::Result<RpcId> {
        if !self.origin_is_current(&origin) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Stale LSP request origin",
            ));
        }
        let document = &self.documents[&origin.document_id];
        let slot = &self.servers[document.instance.as_ref().unwrap()];
        let generation = slot.generation.clone();
        slot.client
            .borrow_mut()
            .send_request(method, params, move |result| {
                let result = if generation.get() == origin.server_generation {
                    result
                } else {
                    Err(ResponseError::new(
                        REQUEST_CANCELLED,
                        "Language server restarted",
                    ))
                };
                callback(RoutedLspResponse { origin, result });
            })
    }
    pub fn shutdown(&mut self) {
        for slot in self.servers.values() {
            slot.generation.set(next_server_generation());
            slot.client.borrow_mut().shutdown();
        }
    }
}
impl Drop for WorkspaceLsp {
    fn drop(&mut self) {
        self.shutdown();
    }
}
