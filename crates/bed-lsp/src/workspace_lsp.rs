//! Workspace language-server ownership above ned's single-session client.
//! Each document registers once regardless of how many views display it.
use crate::{
    diagnostics::LspDiagnostics,
    jsonrpc::{REQUEST_CANCELLED, ResponseError, RpcId},
    lsp_client::{LspClient, SharedLspClient, WorkDoneProgress},
    lsp_config::LspConfig,
    message_handler::RpcEvent,
};
use bed_editing::{
    identity::{DocumentId, ViewId, WorkspaceId},
    util::doc_path,
};
use serde_json::Value;
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
};

// Workspace roots/configuration can be replaced while the host keeps the same
// document/view IDs. A process-wide token prevents an old pool's replies from
// matching generation 1 in its replacement pool or another language slot.
fn next_server_generation() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
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
    language_id: String,
    server_language: String,
}

pub struct WorkspaceLsp {
    ssh_target: Option<bed_remote::SshTarget>,
    workspace_id: WorkspaceId,
    config_path: PathBuf,
    root: PathBuf,
    config: LspConfig,
    config_error: Option<String>,
    servers: BTreeMap<String, ServerSlot>,
    documents: BTreeMap<DocumentId, RegisteredDocument>,
    events: Vec<WorkspaceLspEvent>,
    project_check_owner: bool,
}
impl WorkspaceLsp {
    pub fn new(workspace_id: WorkspaceId, config_path: PathBuf, root: PathBuf) -> Self {
        let (config, config_error) = match LspConfig::load(&config_path) {
            Ok(config) => (config, None),
            Err(error) => (LspConfig::default(), Some(error.to_string())),
        };
        Self {
            ssh_target: None,
            workspace_id,
            config_path,
            root,
            config,
            config_error,
            servers: BTreeMap::new(),
            documents: BTreeMap::new(),
            events: Vec::new(),
            project_check_owner: false,
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
    pub fn set_server_arguments(&mut self, language: &str, arguments: Vec<String>) {
        if let Some(server) = self
            .config
            .language_servers
            .iter_mut()
            .find(|server| server.language == language)
        {
            server.server_args = arguments.clone();
        }
        if let Some(slot) = self.servers.get(language) {
            slot.client
                .borrow_mut()
                .set_server_arguments(language, arguments);
        }
    }
    fn ensure_slot(&mut self, language: &str) -> &mut ServerSlot {
        self.servers.entry(language.to_owned()).or_insert_with(|| {
            let mut client = LspClient::with_config(&self.config_path, self.config.clone());
            client.set_ssh_target(self.ssh_target.clone());
            client.set_workspace(&self.root.to_string_lossy());
            client.set_project_check_owner(self.project_check_owner);
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
        let language = self.config.detect_language(&path);
        self.documents.insert(
            id,
            RegisteredDocument {
                path: path.clone(),
                bytes: bytes.to_owned(),
                version,
                language_id: language_id.to_owned(),
                server_language: language.clone(),
            },
        );
        if !language.is_empty() {
            let result = {
                let mut client = self.ensure_slot(&language).client.borrow_mut();
                client.init(&path).and_then(|started| {
                    if started {
                        client.did_open(&path, bytes, version, language_id)
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
        }
        Ok(())
    }
    /// Refresh retry snapshots only. The document's Editor event listeners send
    /// changes/save; views must not duplicate those notifications.
    pub fn update_document_snapshot(
        &mut self,
        id: DocumentId,
        path: &str,
        bytes: &[u8],
        version: i32,
        language_id: &str,
    ) -> io::Result<()> {
        let path = self.document_key(path);
        let language = self.config.detect_language(&path);
        let Some(document) = self.documents.get_mut(&id) else {
            return self.register_document(id, &path, bytes, version, language_id);
        };
        if document.path != path || document.server_language != language {
            self.unregister_document(id)?;
            return self.register_document(id, &path, bytes, version, language_id);
        }
        document.bytes.clear();
        document.bytes.extend_from_slice(bytes);
        document.version = version;
        document.language_id = language_id.to_owned();
        Ok(())
    }
    pub fn unregister_document(&mut self, id: DocumentId) -> io::Result<()> {
        let Some(document) = self.documents.remove(&id) else {
            return Ok(());
        };
        if let Some(slot) = self.servers.get(&document.server_language) {
            slot.client.borrow_mut().did_close(&document.path)?;
        }
        Ok(())
    }
    pub fn client_for_document(&self, id: DocumentId) -> Option<SharedLspClient> {
        self.client_for_language(&self.documents.get(&id)?.server_language)
    }
    pub fn client_for_path(&self, path: &str) -> Option<SharedLspClient> {
        self.client_for_language(&self.config.detect_language(path))
    }
    pub fn client_for_language(&self, language: &str) -> Option<SharedLspClient> {
        self.servers
            .get(language)
            .map(|slot| Rc::clone(&slot.client))
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
    pub fn diagnostic_coverage_complete(&self) -> bool {
        !self.servers.is_empty()
            && self
                .servers
                .values()
                .all(|slot| slot.client.borrow().workspace_diagnostics_complete())
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
    /// Start project languages from an index without manufacturing documents.
    pub fn start_project_languages(&mut self, paths: &[String]) {
        let mut languages = std::collections::BTreeSet::new();
        for path in paths {
            let language = self.config.detect_language(path);
            if !language.is_empty() {
                languages.insert(language);
            }
        }
        for language in languages {
            if self.servers.contains_key(&language) {
                continue;
            }
            let client = self.ensure_slot(&language).client.clone();
            if let Err(error) = client.borrow_mut().start_server(&language, "") {
                // The server retains its startup error for the dashboard.
                let _ = error;
            }
        }
    }
    pub fn poll(&mut self) -> Vec<WorkspaceLspEvent> {
        let mut events = std::mem::take(&mut self.events);
        for (language, slot) in &self.servers {
            events.extend(slot.client.borrow_mut().poll().into_iter().map(|event| {
                WorkspaceLspEvent::Rpc {
                    language: language.clone(),
                    server_generation: slot.generation.get(),
                    event,
                }
            }));
        }
        events
    }
    pub fn retry_language(&mut self, language: &str) -> io::Result<bool> {
        if !self
            .config
            .language_servers
            .iter()
            .any(|server| server.language == language)
        {
            return Ok(false);
        }
        let slot = self.ensure_slot(language);
        slot.generation.set(next_server_generation());
        let client = Rc::clone(&slot.client);
        let mut client = client.borrow_mut();
        client.shutdown();
        if !client.start_server(language, "")? {
            return Ok(false);
        }
        for document in self
            .documents
            .values()
            .filter(|document| document.server_language == language)
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
    pub fn reload_config(&mut self) -> io::Result<()> {
        let config = match LspConfig::load(&self.config_path) {
            Ok(config) => config,
            Err(error) => {
                self.config_error = Some(error.to_string());
                return Err(error);
            }
        };
        self.config_error = None;
        self.shutdown();
        self.config = config;
        for slot in self.servers.values() {
            slot.client
                .borrow_mut()
                .set_configuration(self.config.clone());
        }
        for document in self.documents.values_mut() {
            document.server_language = self.config.detect_language(&document.path);
        }
        let languages: Vec<_> = self
            .documents
            .values()
            .map(|document| document.server_language.clone())
            .filter(|language| !language.is_empty())
            .collect();
        let mut startup_error = None;
        for language in languages {
            if !self
                .servers
                .get(&language)
                .is_some_and(|slot| slot.client.borrow().is_process_started())
                && let Err(error) = self.retry_language(&language)
            {
                for (id, document) in &self.documents {
                    if document.server_language == language {
                        self.events.push(WorkspaceLspEvent::DocumentError {
                            document_id: *id,
                            message: error.to_string(),
                        });
                    }
                }
                if startup_error.is_none() {
                    startup_error = Some(error);
                }
            }
        }
        startup_error.map_or(Ok(()), Err)
    }
    pub fn server_statuses(&self) -> Vec<WorkspaceServerStatus> {
        self.config
            .language_servers
            .iter()
            .map(|configured| {
                let slot = self.servers.get(&configured.language);
                let client = slot.map(|slot| slot.client.borrow());
                WorkspaceServerStatus {
                    language: configured.language.clone(),
                    path: if self.ssh_target.is_some() {
                        configured.server_paths.first().map(PathBuf::from)
                    } else {
                        super::lsp_config::resolve_server_paths(&configured.server_paths)
                    },
                    process_id: client.as_ref().and_then(|client| client.process_id()),
                    initialized: client
                        .as_ref()
                        .is_some_and(|client| client.is_initialized()),
                    last_error: client
                        .as_ref()
                        .and_then(|client| client.last_error().map(str::to_owned)),
                    stderr: client
                        .as_ref()
                        .map(|client| client.stderr_text())
                        .unwrap_or_default(),
                    generation: slot.map_or(0, |slot| slot.generation.get()),
                    progress: client
                        .as_ref()
                        .map(|client| client.progress())
                        .unwrap_or_default(),
                }
            })
            .collect()
    }
    pub fn request_origin(
        &self,
        id: DocumentId,
        view_id: ViewId,
        document_generation: u64,
        ticket: u64,
    ) -> Option<LspRequestOrigin> {
        let document = self.documents.get(&id)?;
        let server = self.servers.get(&document.server_language)?;
        Some(LspRequestOrigin {
            workspace_id: self.workspace_id,
            server_generation: server.generation.get(),
            document_id: id,
            document_generation,
            version: document.version,
            view_id,
            ticket,
        })
    }
    /// Also validate the host's live document generation, view attachment and
    /// ticket before presenting a reply. This check covers pool-owned state.
    pub fn origin_is_current(&self, origin: &LspRequestOrigin) -> bool {
        origin.workspace_id == self.workspace_id
            && self
                .documents
                .get(&origin.document_id)
                .is_some_and(|document| {
                    document.version == origin.version
                        && self
                            .servers
                            .get(&document.server_language)
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
        let slot = &self.servers[&document.server_language];
        let generation = Rc::clone(&slot.generation);
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
