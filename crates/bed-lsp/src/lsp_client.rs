//! ned's client composition: configuration, initialization, synchronization and
//! server handlers. UI navigation/hover components bind this main-thread client.

use crate::{
    diagnostics::{DiagnosticItem, LspDiagnostics},
    jsonrpc::{ResponseError, RpcId},
    lsp_config::{LanguageServerInfo, LspConfig, server_child_path},
    lsp_document_sync::LspDocumentSync,
    lsp_uri::LspUri,
    message_handler::{MAX_STDERR_BYTES, RpcEvent, RpcSession},
    process::ProcessOptions,
};
use bed_editing::editor_events::DocumentChange;
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    collections::{BTreeMap, VecDeque},
    io,
    path::{Path, PathBuf},
    rc::Rc,
    sync::mpsc,
    time::{Duration, Instant},
};

/// Standard LSP work-done progress, added above the pinned client's handlers.
/// See https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#workDoneProgress.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProgressToken {
    Number(i32),
    String(String),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkDoneProgress {
    pub token: ProgressToken,
    pub title: String,
    pub message: Option<String>,
    pub percentage: Option<u32>,
    pub finished: bool,
}
pub const MAX_ACTIVE_PROGRESS: usize = 32;
pub const MAX_COMPLETED_PROGRESS: usize = 16;
pub const MAX_PROGRESS_TEXT_BYTES: usize = 4096;
pub const MAX_PROGRESS_TOKEN_BYTES: usize = 1024;

#[derive(Default)]
struct ProgressState {
    active: VecDeque<WorkDoneProgress>,
    completed: VecDeque<WorkDoneProgress>,
}
impl ProgressState {
    fn snapshot(&self) -> Vec<WorkDoneProgress> {
        self.active.iter().chain(&self.completed).cloned().collect()
    }
    fn apply(&mut self, params: Option<&Value>) -> Result<(), ResponseError> {
        let Some(params) = params.and_then(Value::as_object) else {
            return Ok(());
        };
        let Some(value) = params.get("value").and_then(Value::as_object) else {
            return Ok(());
        };
        let kind = value.get("kind").and_then(Value::as_str).unwrap_or("");
        if !matches!(kind, "begin" | "report" | "end") {
            // $/progress also transports partial results and arbitrary custom
            // values. They are not work-done state and must not fail a session.
            return Ok(());
        }
        let token = progress_token(params.get("token"))?;
        let text = |key| -> Result<Option<String>, ResponseError> {
            value
                .get(key)
                .filter(|value| !value.is_null())
                .map(|value| {
                    value
                        .as_str()
                        .map(progress_text)
                        .ok_or_else(|| ResponseError::invalid_params("Expected work-done text"))
                })
                .transpose()
        };
        let message = text("message")?;
        let percentage = value
            .get("percentage")
            .filter(|value| !value.is_null())
            .map(|value| {
                value
                    .as_u64()
                    .filter(|value| *value <= 100)
                    .map(|value| value as u32)
                    .ok_or_else(|| {
                        ResponseError::invalid_params("Expected work-done percentage from 0 to 100")
                    })
            })
            .transpose()?;
        match kind {
            "begin" => {
                let title = text("title")?
                    .ok_or_else(|| ResponseError::invalid_params("Expected work-done title"))?;
                self.active.retain(|job| job.token != token);
                self.completed.retain(|job| job.token != token);
                if self.active.len() == MAX_ACTIVE_PROGRESS {
                    self.active.pop_front();
                }
                self.active.push_back(WorkDoneProgress {
                    token,
                    title,
                    message,
                    percentage,
                    finished: false,
                });
            }
            "report" => {
                if let Some(job) = self.active.iter_mut().find(|job| job.token == token) {
                    if message.is_some() {
                        job.message = message;
                    }
                    if percentage.is_some() {
                        job.percentage = percentage;
                    }
                }
            }
            "end" => {
                if let Some(index) = self.active.iter().position(|job| job.token == token) {
                    let mut job = self.active.remove(index).unwrap();
                    if message.is_some() {
                        job.message = message;
                    }
                    job.finished = true;
                    if self.completed.len() == MAX_COMPLETED_PROGRESS {
                        self.completed.pop_front();
                    }
                    self.completed.push_back(job);
                }
            }
            _ => unreachable!(),
        }
        Ok(())
    }
}
fn progress_token(value: Option<&Value>) -> Result<ProgressToken, ResponseError> {
    if let Some(token) = value.and_then(Value::as_str) {
        if token.len() <= MAX_PROGRESS_TOKEN_BYTES && !token.contains('\0') {
            return Ok(ProgressToken::String(token.to_owned()));
        }
        return Err(ResponseError::invalid_params("Invalid work-done token"));
    }
    value
        .and_then(Value::as_i64)
        .and_then(|token| i32::try_from(token).ok())
        .map(ProgressToken::Number)
        .ok_or_else(|| {
            ResponseError::invalid_params("Expected string or signed 32-bit progress token")
        })
}
fn progress_text(value: &str) -> String {
    // Server text reaches native text widgets. Replace embedded NULs and cut
    // at a UTF-8 boundary, bounding each retained title/message independently.
    let mut result = String::new();
    for character in value.chars() {
        let character = if character == '\0' {
            '\u{fffd}'
        } else {
            character
        };
        if result.len() + character.len_utf8() > MAX_PROGRESS_TEXT_BYTES {
            break;
        }
        result.push(character);
    }
    result
}

#[derive(Default)]
struct DiagnosticPull {
    enabled: bool,
    static_enabled: bool,
    registrations: BTreeMap<String, bool>,
    identifier: Option<String>,
    pending_id: Option<RpcId>,
    generation: u64,
    refresh: bool,
    pending: bool,
    completed: bool,
    failed: bool,
    last: Option<Instant>,
    result_ids: BTreeMap<String, String>,
    replies: Vec<Result<Value, ResponseError>>,
}

pub struct LspClient {
    ssh_target: Option<bed_remote::SshTarget>,
    config_path: PathBuf,
    config: LspConfig,
    workspace: String,
    current_language: String,
    global_server_argument: String,
    diagnostics: LspDiagnostics,
    sync: LspDocumentSync,
    session: Option<RpcSession>,
    initialize_result: Option<mpsc::Receiver<Result<Value, ResponseError>>>,
    last_error: Option<String>,
    stderr_tail: VecDeque<u8>,
    stderr_pending: RefCell<VecDeque<u8>>,
    progress: Rc<RefCell<ProgressState>>,
    pull: Rc<RefCell<DiagnosticPull>>,
    server_configuration: Rc<RefCell<Value>>,
    project_check_owner: bool,
    configured_check_on_save: Option<Value>,
}

impl LspClient {
    /// Read only this supplied configuration path; constructing an embedded
    /// client never creates or changes a user's home configuration.
    pub fn new(config_path: impl Into<PathBuf>) -> Self {
        let diagnostics = LspDiagnostics::new();
        let mut client = Self {
            ssh_target: None,
            config_path: config_path.into(),
            config: LspConfig::default(),
            workspace: String::new(),
            current_language: String::new(),
            global_server_argument: String::new(),
            sync: LspDocumentSync::new(diagnostics.clone()),
            diagnostics,
            session: None,
            initialize_result: None,
            last_error: None,
            stderr_tail: VecDeque::new(),
            stderr_pending: RefCell::new(VecDeque::new()),
            progress: Rc::new(RefCell::new(ProgressState::default())),
            pull: Rc::new(RefCell::new(DiagnosticPull::default())),
            server_configuration: Rc::new(RefCell::new(json!({}))),
            project_check_owner: false,
            configured_check_on_save: None,
        };
        let _ = client.reload_config();
        client
    }

    pub fn with_config(config_path: impl Into<PathBuf>, config: LspConfig) -> Self {
        let mut client = Self::new(config_path);
        client.set_configuration(config);
        client
    }
    pub fn set_configuration(&mut self, config: LspConfig) {
        self.sync.set_languages(&config.language_servers);
        self.config = config;
        self.last_error = None;
    }

    pub fn reload_config(&mut self) -> io::Result<()> {
        // Upstream clears the existing list even when the new file cannot load.
        self.config = LspConfig::default();
        self.sync.set_languages(&[]);
        match LspConfig::load(&self.config_path) {
            Ok(config) => {
                self.config = config;
                self.sync.set_languages(&self.config.language_servers);
                self.last_error = None;
                Ok(())
            }
            Err(error) => {
                self.last_error = Some(error.to_string());
                Err(error)
            }
        }
    }

    pub fn set_workspace(&mut self, workspace: &str) {
        if self.workspace != workspace {
            self.shutdown();
            self.workspace = workspace.into();
        }
    }
    /// Use target-native paths and launch language servers on this SSH host.
    /// Configuration stays local; binary discovery and process cwd stay remote.
    pub fn set_ssh_target(&mut self, target: Option<bed_remote::SshTarget>) {
        if self.ssh_target != target {
            self.shutdown();
            self.sync.set_remote_paths(target.is_some());
            self.diagnostics.set_remote_paths(target.is_some());
            self.ssh_target = target;
        }
    }

    pub fn workspace(&self) -> &str {
        &self.workspace
    }
    pub fn config(&self) -> &LspConfig {
        &self.config
    }
    pub fn config_path(&self) -> &Path {
        &self.config_path
    }
    pub fn current_language(&self) -> &str {
        &self.current_language
    }
    pub fn language_servers(&self) -> &[LanguageServerInfo] {
        &self.config.language_servers
    }
    pub fn supported_languages(&self) -> Vec<String> {
        self.config.supported_languages()
    }
    pub fn detect_language_from_file(&self, path: &str) -> String {
        self.config.detect_language(path)
    }
    pub fn find_server_path(&self, language: &str) -> Option<PathBuf> {
        if self.ssh_target.is_some() {
            self.config
                .language_servers
                .iter()
                .find(|server| server.language == language)?
                .server_paths
                .first()
                .map(PathBuf::from)
        } else {
            self.config.find_server_path(language)
        }
    }
    pub fn diagnostics(&self) -> LspDiagnostics {
        self.diagnostics.clone()
    }
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }
    pub fn stderr_text(&self) -> String {
        String::from_utf8_lossy(&self.stderr_tail.iter().copied().collect::<Vec<_>>()).into_owned()
    }
    /// Active jobs followed by recent completed jobs; initialization alone is
    /// not a statement that workspace loading/indexing has completed.
    pub fn progress(&self) -> Vec<WorkDoneProgress> {
        self.progress.borrow().snapshot()
    }
    pub fn is_initialized(&self) -> bool {
        self.sync.is_ready()
    }
    pub fn is_process_started(&self) -> bool {
        self.session.as_ref().is_some_and(RpcSession::is_connected)
    }
    pub fn is_document_open(&self, path: &str) -> bool {
        self.sync.is_document_open(path)
    }
    pub fn process_id(&self) -> Option<u32> {
        self.session.as_ref().map(RpcSession::process_id)
    }

    /// Host configuration can supply argv directly. The on-disk format remains
    /// upstream's format, which only supplies the TypeScript/Python --stdio default.
    pub fn set_server_arguments(&mut self, language: &str, arguments: Vec<String>) {
        if let Some(server) = self
            .config
            .language_servers
            .iter_mut()
            .find(|server| server.language == language)
        {
            server.server_args = arguments;
        }
    }

    pub fn set_global_server_argument(&mut self, argument: &str) {
        self.global_server_argument = argument.into();
    }

    pub fn init(&mut self, path: &str) -> io::Result<bool> {
        if self.is_process_started() {
            return Ok(true);
        }
        if self.workspace.is_empty() {
            self.last_error =
                Some("Open a project folder before starting a language server".into());
            return Ok(false);
        }
        let language = self.config.detect_language(path);
        if language.is_empty() {
            return Ok(false);
        }
        self.start_server(&language, "")
    }

    pub fn start_server(&mut self, language: &str, path: &str) -> io::Result<bool> {
        if self.is_process_started() {
            return Ok(true);
        }
        self.session.take();
        self.sync.disconnect();
        self.current_language = language.into();
        let program = if path.is_empty() {
            let Some(program) = self.find_server_path(language) else {
                self.last_error = Some(format!(
                    "No executable found for {language}; check its configured paths, PATH, and installed language-server components"
                ));
                return Ok(false);
            };
            program
        } else {
            PathBuf::from(path)
        };
        let mut arguments = self
            .config
            .language_servers
            .iter()
            .find(|server| server.language == language)
            .map(|server| server.server_args.clone())
            .unwrap_or_default();
        if !self.global_server_argument.is_empty() {
            arguments.push(self.global_server_argument.clone());
        }
        let candidates = if self.ssh_target.is_some() && path.is_empty() {
            self.config
                .language_servers
                .iter()
                .find(|server| server.language == language)
                .map(|server| server.server_paths.clone())
                .unwrap_or_default()
        } else {
            vec![program.to_string_lossy().into_owned()]
        };
        let result = self.start_session(&program, &arguments, &candidates);
        if let Err(error) = &result {
            self.last_error = Some(error.to_string());
        }
        result.map(|()| true)
    }

    fn start_session(
        &mut self,
        program: &Path,
        arguments: &[String],
        candidates: &[String],
    ) -> io::Result<()> {
        let remote = self.ssh_target.is_some();
        let mut initialize = initialize_params(&self.workspace, remote)?;
        // Per-language settings are optional extensions of the existing lsp.json.
        let entry = std::fs::read(&self.config_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .and_then(|value| {
                value["languages"]
                    .as_array()?
                    .iter()
                    .find(|entry| entry["language_name"].as_str() == Some(&self.current_language))
                    .cloned()
            });
        let mut configuration = entry
            .as_ref()
            .and_then(|entry| entry.get("settings"))
            .filter(|value| value.is_object())
            .cloned()
            .unwrap_or_else(|| {
                if self.current_language == "python" {
                    json!({"python":{"analysis":{"diagnosticMode":"workspace"}}})
                } else {
                    json!({})
                }
            });
        self.configured_check_on_save = configuration
            .get("rust-analyzer")
            .and_then(|value| value.get("checkOnSave"))
            .cloned();
        if self.current_language == "rust" && self.project_check_owner {
            configuration["rust-analyzer"]["checkOnSave"] = json!(false);
        }
        self.server_configuration = Rc::new(RefCell::new(configuration));
        if let Some(options) = entry.and_then(|entry| entry.get("initialization_options").cloned())
        {
            initialize["initializationOptions"] = options;
        }
        self.pull = Rc::new(RefCell::new(DiagnosticPull::default()));
        let (program, arguments, options) = if let Some(target) = &self.ssh_target {
            ssh_server_launch(target, &self.workspace, candidates, arguments)?
        } else {
            (
                program.to_owned(),
                arguments.to_vec(),
                ProcessOptions {
                    working_directory: (!self.workspace.is_empty())
                        .then(|| PathBuf::from(&self.workspace)),
                    environment: vec![("PATH".into(), server_child_path(program)?)],
                },
            )
        };
        self.stderr_tail.clear();
        self.stderr_pending.borrow_mut().clear();
        *self.progress.borrow_mut() = ProgressState::default();
        let mut session = RpcSession::start_with_options(&program, &arguments, &options)?;
        register_server_handlers(
            &mut session,
            &self.workspace,
            remote,
            self.diagnostics.clone(),
            Rc::clone(&self.progress),
            Rc::clone(&self.pull),
            self.server_configuration.clone(),
        )?;
        let (sender, receiver) = mpsc::channel();
        session.send_request("initialize", Some(initialize), move |result| {
            let _ = sender.send(result);
        })?;
        self.sync.set_languages(&self.config.language_servers);
        self.sync.connect();
        self.initialize_result = Some(receiver);
        self.session = Some(session);
        self.last_error = None;
        Ok(())
    }

    /// Decode results and publish diagnostics only while the application polls.
    pub fn poll(&mut self) -> Vec<RpcEvent> {
        let Some(session) = self.session.as_mut() else {
            return Vec::new();
        };
        let sync = &mut self.sync;
        let initialize_result = &mut self.initialize_result;
        let last_error = &mut self.last_error;
        let events = session.poll_after_message(|session| {
            consume_initialize_result(initialize_result, sync, session, last_error, &self.pull);
        });
        consume_initialize_result(initialize_result, sync, session, last_error, &self.pull);
        let bytes = session.take_stderr();
        retain_stderr(&mut self.stderr_tail, &bytes);
        retain_stderr(&mut self.stderr_pending.borrow_mut(), &bytes);
        for event in &events {
            match event {
                RpcEvent::ProtocolError(error) => {
                    self.last_error = Some(error.to_string());
                }
                RpcEvent::Disconnected(error) => {
                    *self.progress.borrow_mut() = ProgressState::default();
                    self.sync.disconnect();
                    self.pull.borrow_mut().completed = false;
                    self.pull.borrow_mut().enabled = false;
                    self.initialize_result = None;
                    let stderr = self.stderr_text();
                    self.last_error = Some(if stderr.trim().is_empty() {
                        error.clone()
                    } else {
                        format!("{error}\n{}", stderr.trim())
                    });
                }
                _ => {}
            }
        }
        self.poll_workspace_diagnostics();
        events
    }
    /// True only after a workspace pull has completed successfully.
    pub fn workspace_diagnostics_complete(&self) -> bool {
        self.pull.borrow().completed
    }
    pub fn set_project_check_owner(&mut self, enabled: bool) {
        if self.project_check_owner == enabled {
            return;
        }
        self.project_check_owner = enabled;
        if self.current_language == "rust" {
            let mut configuration = self.server_configuration.borrow_mut();
            if enabled {
                configuration["rust-analyzer"]["checkOnSave"] = json!(false);
            } else if let Some(value) = &self.configured_check_on_save {
                configuration["rust-analyzer"]["checkOnSave"] = value.clone();
            } else if let Some(analyzer) = configuration
                .get_mut("rust-analyzer")
                .and_then(Value::as_object_mut)
            {
                analyzer.remove("checkOnSave");
            }
            drop(configuration);
            let _ = self.send_notification(
                "workspace/didChangeConfiguration",
                Some(json!({"settings":self.server_configuration.borrow().clone()})),
            );
        }
    }
    pub fn refresh_workspace_diagnostics(&mut self) {
        let mut pull = self.pull.borrow_mut();
        pull.refresh = true;
        pull.completed = false;
    }
    fn poll_workspace_diagnostics(&mut self) {
        let replies = std::mem::take(&mut self.pull.borrow_mut().replies);
        for reply in replies {
            {
                let mut pull = self.pull.borrow_mut();
                pull.pending = false;
                pull.pending_id = None;
                pull.completed = false;
            }
            match reply {
                Ok(report) => {
                    let mut pull = self.pull.borrow_mut();
                    match apply_workspace_report(&self.diagnostics, &mut pull, &report) {
                        Ok(()) => pull.completed = !pull.refresh && pull.enabled && !pull.failed,
                        Err(error) => {
                            pull.failed = true;
                            self.last_error = Some(error.to_string());
                        }
                    }
                }
                Err(error) => self.last_error = Some(format!("Workspace diagnostics: {error}")),
            }
        }
        let ready = self.is_initialized();
        let mut pull = self.pull.borrow_mut();
        if pull.pending
            && (!pull.enabled
                || pull.refresh
                || pull
                    .last
                    .is_some_and(|last| last.elapsed() >= Duration::from_secs(30)))
        {
            let id = pull.pending_id.take();
            pull.pending = false;
            pull.completed = false;
            pull.generation += 1;
            drop(pull);
            if let Some(id) = id
                && let Some(session) = &mut self.session
            {
                let _ = session.cancel_request(&id);
            }
            pull = self.pull.borrow_mut();
        }
        if !ready
            || !pull.enabled
            || pull.pending
            || (!pull.refresh
                && pull
                    .last
                    .is_some_and(|last| last.elapsed() < Duration::from_secs(30)))
        {
            return;
        }
        let ids: Vec<_> = pull
            .result_ids
            .iter()
            .map(|(uri, value)| json!({"uri":uri,"value":value}))
            .collect();
        pull.pending = true;
        pull.completed = false;
        pull.failed = false;
        pull.refresh = false;
        pull.last = Some(Instant::now());
        pull.generation += 1;
        let generation = pull.generation;
        let mut params = json!({"previousResultIds":ids,"partialResultToken":format!("bed.workspace-diagnostics-{generation}")});
        if let Some(identifier) = &pull.identifier {
            params["identifier"] = json!(identifier);
        }
        drop(pull);
        let state = self.pull.clone();
        match self.send_request("workspace/diagnostic", params, move |reply| {
            let mut pull = state.borrow_mut();
            if pull.generation == generation {
                pull.replies.push(reply);
            }
        }) {
            Ok(id) => self.pull.borrow_mut().pending_id = Some(id),
            Err(error) => {
                self.pull.borrow_mut().pending = false;
                self.last_error = Some(error.to_string());
            }
        }
    }

    pub fn send_request(
        &mut self,
        method: &str,
        params: Value,
        callback: impl FnOnce(Result<Value, ResponseError>) + 'static,
    ) -> io::Result<RpcId> {
        self.session
            .as_mut()
            .ok_or_else(not_connected)?
            .send_request(method, Some(params), callback)
    }

    pub fn send_notification(&self, method: &str, params: Option<Value>) -> io::Result<()> {
        self.session
            .as_ref()
            .ok_or_else(not_connected)?
            .send_notification(method, params)
    }

    pub fn did_open(
        &mut self,
        path: &str,
        bytes: &[u8],
        version: i32,
        language_id: &str,
    ) -> io::Result<()> {
        let Some(session) = self.session.as_ref() else {
            return Ok(());
        };
        let text = std::str::from_utf8(bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        self.sync
            .did_open(path, text, version, language_id, session)
    }

    pub fn did_change(
        &mut self,
        path: &str,
        version: i32,
        changes: &[DocumentChange],
        full_text: impl FnOnce() -> io::Result<String>,
    ) -> io::Result<()> {
        let Some(session) = self.session.as_ref() else {
            return Ok(());
        };
        self.sync
            .did_change(path, version, changes, full_text, session)
    }

    pub fn did_save(
        &mut self,
        path: &str,
        full_text: impl FnOnce() -> io::Result<String>,
    ) -> io::Result<()> {
        let Some(session) = self.session.as_ref() else {
            return Ok(());
        };
        self.sync.did_save(path, full_text, session)
    }

    pub fn did_close(&mut self, path: &str) -> io::Result<()> {
        self.sync.cancel_pending_open(path);
        let Some(session) = self.session.as_ref() else {
            return Ok(());
        };
        self.sync.did_close(path, session)
    }

    pub fn take_stderr(&self) -> Vec<u8> {
        let bytes = self
            .session
            .as_ref()
            .map(RpcSession::take_stderr)
            .unwrap_or_default();
        let mut pending = self.stderr_pending.borrow_mut();
        retain_stderr(&mut pending, &bytes);
        pending.drain(..).collect()
    }

    pub fn stop_server(&mut self) {
        if let Some(mut session) = self.session.take() {
            session.shutdown();
        }
        self.initialize_result = None;
        self.sync.disconnect();
        self.current_language.clear();
        self.pull = Rc::new(RefCell::new(DiagnosticPull::default()));
        *self.progress.borrow_mut() = ProgressState::default();
    }

    pub fn shutdown(&mut self) {
        let started = self.is_process_started();
        self.stop_server();
        if started {
            self.diagnostics.clear_all();
        }
    }
}

fn retain_stderr(retained: &mut VecDeque<u8>, bytes: &[u8]) {
    let bytes = &bytes[bytes.len().saturating_sub(MAX_STDERR_BYTES)..];
    let excess = retained
        .len()
        .saturating_add(bytes.len())
        .saturating_sub(MAX_STDERR_BYTES);
    retained.drain(..excess);
    retained.extend(bytes);
}

impl Drop for LspClient {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn not_connected() -> io::Error {
    io::Error::new(io::ErrorKind::NotConnected, "LSP server is disconnected")
}

fn workspace_uri(workspace: &str, remote: bool) -> io::Result<LspUri> {
    if remote {
        LspUri::file_uri_from_remote_path(workspace)
    } else {
        LspUri::file_uri_from_path(workspace)
    }
}

fn ssh_server_launch(
    target: &bed_remote::SshTarget,
    workspace: &str,
    candidates: &[String],
    arguments: &[String],
) -> io::Result<(PathBuf, Vec<String>, ProcessOptions)> {
    if target.host.is_empty()
        || target.host.starts_with('-')
        || target.host.chars().any(char::is_whitespace)
        || target.host.contains('\0')
        || candidates.is_empty()
        || !workspace.starts_with('/')
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "SSH language server requires a host, executable and absolute remote root",
        ));
    }
    let mut remote = vec![
        target.agent.clone(),
        "exec".into(),
        "--cwd".into(),
        workspace.into(),
    ];
    for candidate in candidates {
        remote.extend(["--candidate".into(), candidate.clone()]);
    }
    remote.push("--".into());
    remote.extend_from_slice(arguments);
    Ok((
        PathBuf::from("ssh"),
        vec![
            "-T".into(),
            "--".into(),
            target.host.clone(),
            bed_remote::remote_command(&remote)?,
        ],
        ProcessOptions::default(),
    ))
}

fn workspace_folders(workspace: &str, remote: bool) -> io::Result<Value> {
    if workspace.is_empty() {
        return Ok(json!([]));
    }
    let name = Path::new(workspace)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or(workspace);
    Ok(json!([{"uri":workspace_uri(workspace, remote)?.to_string(),"name":name}]))
}

fn initialize_params(workspace: &str, remote: bool) -> io::Result<Value> {
    Ok(
        json!({"processId":if remote { None } else { Some(std::process::id()) },"rootUri":workspace_uri(workspace, remote)?.to_string(),"rootPath":workspace,
        "clientInfo":{"name":"bed"},"workspaceFolders":workspace_folders(workspace, remote)?,
        "capabilities":{"window":{"workDoneProgress":true},"workspace":{"workspaceFolders":true,"configuration":true,"diagnostics":{"refreshSupport":true}},
            "textDocument":{"diagnostic":{"dynamicRegistration":true,"relatedDocumentSupport":true},"synchronization":{"didSave":true},"hover":{},"definition":{"linkSupport":true},"references":{},"publishDiagnostics":{"relatedInformation":true}},
            "general":{"positionEncodings":["utf-16"]}}}),
    )
}

fn consume_initialize_result(
    receiver: &mut Option<mpsc::Receiver<Result<Value, ResponseError>>>,
    sync: &mut LspDocumentSync,
    session: &RpcSession,
    last_error: &mut Option<String>,
    pull: &Rc<RefCell<DiagnosticPull>>,
) {
    let Some(result) = receiver
        .as_ref()
        .and_then(|receiver| receiver.try_recv().ok())
    else {
        return;
    };
    *receiver = None;
    match result {
        Ok(result) => {
            let provider = &result["capabilities"]["diagnosticProvider"];
            let mut diagnostics = pull.borrow_mut();
            diagnostics.static_enabled =
                provider["workspaceDiagnostics"].as_bool().unwrap_or(false);
            diagnostics.enabled = diagnostics.static_enabled
                || diagnostics.registrations.values().any(|enabled| *enabled);
            diagnostics.identifier = provider["identifier"].as_str().map(str::to_owned);
            drop(diagnostics);
            let ready =
                super::lsp_document_sync::validate_initialize_result(&result).and_then(|()| {
                    sync.apply_capabilities(&result);
                    session.send_notification("initialized", Some(json!({})))?;
                    sync.mark_handshake_ready(session)
                });
            if let Err(error) = ready {
                *last_error = Some(error.to_string());
            }
        }
        Err(error) => {
            *last_error = Some(format!("Initialize failed: {error}"));
        }
    }
}

fn register_server_handlers(
    session: &mut RpcSession,
    workspace: &str,
    remote: bool,
    diagnostics: LspDiagnostics,
    progress: Rc<RefCell<ProgressState>>,
    pull: Rc<RefCell<DiagnosticPull>>,
    configuration: Rc<RefCell<Value>>,
) -> io::Result<()> {
    let partial = pull.clone();
    let partial_store = diagnostics.clone();
    session.register_notification_handler("$/progress", move |params| {
        let token = params.as_ref().and_then(|params| params["token"].as_str());
        if token.is_some_and(|token| token.starts_with("bed.workspace-diagnostics-")) {
            let mut pull = partial.borrow_mut();
            if pull.pending
                && token == Some(format!("bed.workspace-diagnostics-{}", pull.generation).as_str())
                && !pull.refresh
            {
                let result = apply_workspace_report(
                    &partial_store,
                    &mut pull,
                    &params.as_ref().unwrap()["value"],
                );
                if result.is_err() {
                    pull.completed = false;
                    pull.failed = true;
                }
                result
            } else {
                Ok(())
            }
        } else {
            progress.borrow_mut().apply(params.as_ref())
        }
    });
    let refresh = pull.clone();
    session.register_request_handler("workspace/diagnostic/refresh", move |_| {
        let mut pull = refresh.borrow_mut();
        pull.refresh = true;
        pull.completed = false;
        Ok(Value::Null)
    });
    session.register_notification_handler("textDocument/publishDiagnostics", move |params| {
        apply_diagnostics(&diagnostics, params.as_ref())
    });
    session.register_request_handler("workspace/configuration", move |params| {
        let items = params
            .as_ref()
            .and_then(|params| params.get("items"))
            .and_then(Value::as_array)
            .ok_or_else(|| ResponseError::invalid_params("Expected configuration items"))?;
        if items.iter().any(|item| !item.is_object()) {
            return Err(ResponseError::invalid_params(
                "Expected configuration item object",
            ));
        }
        let configuration = configuration.borrow();
        Ok(Value::Array(
            items
                .iter()
                .map(|item| {
                    let section = item["section"].as_str().unwrap_or("");
                    if section.is_empty() {
                        return configuration.clone();
                    }
                    if let Some(value) = configuration.get(section) {
                        return value.clone();
                    }
                    let mut value = &*configuration;
                    for segment in section.split('.') {
                        value = &value[segment];
                    }
                    if value.is_null() {
                        json!({})
                    } else {
                        value.clone()
                    }
                })
                .collect(),
        ))
    });
    let folders = workspace_folders(workspace, remote)?;
    session.register_request_handler("workspace/workspaceFolders", move |_| Ok(folders.clone()));
    let registration_state = pull.clone();
    session.register_request_handler("client/registerCapability", move |params| {
        let registrations = params
            .as_ref()
            .and_then(|params| params.get("registrations"))
            .and_then(Value::as_array)
            .ok_or_else(|| ResponseError::invalid_params("Expected registrations"))?;
        for registration in registrations {
            if registration["method"].as_str() == Some("workspace/diagnostic")
                || registration["method"].as_str() == Some("textDocument/diagnostic")
            {
                let id = registration["id"]
                    .as_str()
                    .ok_or_else(|| ResponseError::invalid_params("Expected registration id"))?;
                let mut pull = registration_state.borrow_mut();
                pull.registrations.insert(
                    id.into(),
                    registration["registerOptions"]["workspaceDiagnostics"]
                        .as_bool()
                        .unwrap_or(false),
                );
                pull.enabled =
                    pull.static_enabled || pull.registrations.values().any(|enabled| *enabled);
                pull.identifier = registration["registerOptions"]["identifier"]
                    .as_str()
                    .map(str::to_owned);
                pull.refresh = true;
                pull.completed = false;
            }
            if registration.get("id").and_then(Value::as_str).is_none()
                || registration.get("method").and_then(Value::as_str).is_none()
            {
                return Err(ResponseError::invalid_params(
                    "Expected registration id and method",
                ));
            }
        }
        Ok(Value::Null)
    });
    session.register_request_handler("client/unregisterCapability", move |params| {
        let values = params
            .as_ref()
            .and_then(|params| {
                params
                    .get("unregisterations")
                    .or_else(|| params.get("unregistrations"))
            })
            .and_then(Value::as_array)
            .ok_or_else(|| ResponseError::invalid_params("Expected unregistrations"))?;
        let mut pull = pull.borrow_mut();
        for registration in values {
            let id = registration["id"]
                .as_str()
                .ok_or_else(|| ResponseError::invalid_params("Expected unregistration id"))?;
            pull.registrations.remove(id);
        }
        pull.enabled = pull.static_enabled || pull.registrations.values().any(|enabled| *enabled);
        pull.completed = false;
        Ok(Value::Null)
    });
    session.register_request_handler("window/workDoneProgress/create", |params| {
        let token = params.as_ref().and_then(|params| params.get("token"));
        if !matches!(token, Some(Value::String(_))) && protocol_integer(token).is_err() {
            return Err(ResponseError::invalid_params("Expected progress token"));
        }
        Ok(Value::Null)
    });
    session.register_request_handler("window/showMessageRequest", |params| {
        let params = params
            .as_ref()
            .ok_or_else(|| ResponseError::invalid_params("Expected message parameters"))?;
        if protocol_integer(params.get("type")).is_err()
            || params.get("message").and_then(Value::as_str).is_none()
        {
            return Err(ResponseError::invalid_params(
                "Expected message type and text",
            ));
        }
        if let Some(actions) = params.get("actions").filter(|actions| !actions.is_null()) {
            let actions = actions
                .as_array()
                .ok_or_else(|| ResponseError::invalid_params("Expected message actions"))?;
            if actions
                .iter()
                .any(|action| action.get("title").and_then(Value::as_str).is_none())
            {
                return Err(ResponseError::invalid_params(
                    "Expected message action title",
                ));
            }
        }
        Ok(Value::Null)
    });
    Ok(())
}

fn apply_workspace_report(
    store: &LspDiagnostics,
    pull: &mut DiagnosticPull,
    report: &Value,
) -> Result<(), ResponseError> {
    let items = report["items"]
        .as_array()
        .ok_or_else(|| ResponseError::invalid_params("Expected workspace diagnostic items"))?;
    let mut validated = Vec::new();
    for item in items {
        let uri = item["uri"]
            .as_str()
            .ok_or_else(|| ResponseError::invalid_params("Expected workspace diagnostic URI"))?;
        let report = match item["kind"].as_str() {
            Some("full") => Some(decode_diagnostics(
                store,
                Some(&json!({"uri":uri,"version":item["version"],"diagnostics":item["items"]})),
            )?),
            Some("unchanged") => None,
            _ => {
                return Err(ResponseError::invalid_params(
                    "Expected full or unchanged diagnostic report",
                ));
            }
        };
        validated.push((
            uri.to_owned(),
            item["resultId"].as_str().map(str::to_owned),
            report,
        ));
    }
    for (uri, id, report) in validated {
        if let Some(id) = id {
            pull.result_ids.insert(uri.clone(), id);
        } else if report.is_some() {
            pull.result_ids.remove(&uri);
        }
        if let Some((path, items, version)) = report {
            store.replace(&path, items, version);
        }
    }
    Ok(())
}

fn apply_diagnostics(store: &LspDiagnostics, params: Option<&Value>) -> Result<(), ResponseError> {
    let (path, items, version) = decode_diagnostics(store, params)?;
    store.replace(&path, items, version);
    Ok(())
}
fn decode_diagnostics(
    store: &LspDiagnostics,
    params: Option<&Value>,
) -> Result<(String, Vec<DiagnosticItem>, i32), ResponseError> {
    let params = params.ok_or_else(|| ResponseError::invalid_params("Missing diagnostics"))?;
    let uri = params
        .get("uri")
        .and_then(Value::as_str)
        .ok_or_else(|| ResponseError::invalid_params("Expected diagnostic URI"))?;
    let uri =
        LspUri::parse(uri).map_err(|error| ResponseError::invalid_params(error.to_string()))?;
    let path = if store.uses_remote_paths() {
        uri.path().to_owned()
    } else {
        uri.fs_path()
    };
    let values = params
        .get("diagnostics")
        .and_then(Value::as_array)
        .ok_or_else(|| ResponseError::invalid_params("Expected diagnostics array"))?;
    let numeric = protocol_integer;
    let mut items = Vec::with_capacity(values.len());
    for value in values {
        let start = value.get("range").and_then(|range| range.get("start"));
        let end = value.get("range").and_then(|range| range.get("end"));
        let message = match value.get("message") {
            Some(Value::String(message)) => message.clone(),
            Some(message) if message.get("kind").and_then(Value::as_str).is_some() => message
                .get("value")
                .and_then(Value::as_str)
                .ok_or_else(|| ResponseError::invalid_params("Expected diagnostic markup text"))?
                .into(),
            _ => return Err(ResponseError::invalid_params("Expected diagnostic message")),
        };
        let source = value
            .get("source")
            .filter(|source| !source.is_null())
            .map(|source| {
                source
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| ResponseError::invalid_params("Expected diagnostic source"))
            })
            .transpose()?
            .unwrap_or_default();
        let severity = value
            .get("severity")
            .filter(|severity| !severity.is_null())
            .map(|severity| numeric(Some(severity)))
            .transpose()?
            .unwrap_or(1);
        items.push(DiagnosticItem {
            start_line: numeric(start.and_then(|start| start.get("line")))?,
            start_character: numeric(start.and_then(|start| start.get("character")))?,
            end_line: numeric(end.and_then(|end| end.get("line")))?,
            end_character: numeric(end.and_then(|end| end.get("character")))?,
            message,
            source,
            severity,
        });
    }
    let version = params
        .get("version")
        .filter(|version| !version.is_null())
        .map(|version| numeric(Some(version)))
        .transpose()?
        .unwrap_or(-1);
    Ok((path, items, version))
}

fn protocol_integer(value: Option<&Value>) -> Result<i32, ResponseError> {
    value
        .and_then(Value::as_number)
        .and_then(|number| super::jsonrpc::integer(number).ok())
        .ok_or_else(|| ResponseError::invalid_params("Expected signed 32-bit protocol integer"))
}

/// Shared main-thread language-server handle.
pub type SharedLspClient = std::rc::Rc<std::cell::RefCell<LspClient>>;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn workspace_reports_cover_closed_files_reuse_ids_and_validate_atomically() {
        let store = LspDiagnostics::new();
        let mut pull = DiagnosticPull::default();
        let item = json!({"range":{"start":{"line":3,"character":2},"end":{"line":3,"character":6}},"message":"closed-file error","severity":1});
        let report = json!({"items":[{"uri":"file:///tmp/closed.rs","kind":"full","resultId":"first","items":[item]}]});
        apply_workspace_report(&store, &mut pull, &report).unwrap();
        assert_eq!(store.for_document("/tmp/closed.rs")[0].start_line, 3);
        assert_eq!(pull.result_ids["file:///tmp/closed.rs"], "first");
        apply_workspace_report(&store,&mut pull,&json!({"items":[{"uri":"file:///tmp/closed.rs","kind":"unchanged","resultId":"second"}]})).unwrap();
        assert_eq!(store.for_document("/tmp/closed.rs").len(), 1);
        assert_eq!(pull.result_ids["file:///tmp/closed.rs"], "second");
        assert!(apply_workspace_report(&store,&mut pull,&json!({"items":[{"uri":"file:///tmp/closed.rs","kind":"full","items":[]},{"uri":"file:///tmp/bad.rs","kind":"full","items":[false]}]})).is_err());
        assert_eq!(store.for_document("/tmp/closed.rs").len(), 1);
        assert_eq!(pull.result_ids["file:///tmp/closed.rs"], "second");
    }
    #[test]
    fn bed_cargo_ownership_restores_user_check_on_save_configuration() {
        for original in [Some(json!(false)), Some(json!(true)), None] {
            let mut client = LspClient::new("/missing/lsp.json");
            client.current_language = "rust".into();
            client.configured_check_on_save = original.clone();
            if let Some(value) = &original {
                client.server_configuration.borrow_mut()["rust-analyzer"]["checkOnSave"] =
                    value.clone();
            }
            client.set_project_check_owner(true);
            assert_eq!(
                client.server_configuration.borrow()["rust-analyzer"]["checkOnSave"],
                false
            );
            client.set_project_check_owner(false);
            assert_eq!(
                client
                    .server_configuration
                    .borrow()
                    .get("rust-analyzer")
                    .and_then(|value| value.get("checkOnSave"))
                    .cloned(),
                original
            );
        }
    }
    #[test]
    fn refresh_invalidates_workspace_coverage_immediately() {
        let mut client = LspClient::new("/missing/lsp.json");
        client.pull.borrow_mut().completed = true;
        assert!(client.workspace_diagnostics_complete());
        client.refresh_workspace_diagnostics();
        assert!(!client.workspace_diagnostics_complete());
        assert!(client.pull.borrow().refresh);
    }
    #[test]
    fn remote_launch_quotes_all_candidates_without_local_cwd_or_path() {
        let target = bed_remote::SshTarget {
            host: "user@host".into(),
            agent: "/remote/a'b/bed-headless".into(),
        };
        let (program, args, options) = ssh_server_launch(
            &target,
            "/target/$(project)",
            &["missing/server".into(), "rust-analyzer".into()],
            &["--arg=a'b".into()],
        )
        .unwrap();
        assert_eq!(program, PathBuf::from("ssh"));
        assert_eq!(&args[..3], &["-T", "--", "user@host"]);
        assert_eq!(
            args[3],
            "'/remote/a'\\''b/bed-headless' 'exec' '--cwd' '/target/$(project)' '--candidate' 'missing/server' '--candidate' 'rust-analyzer' '--' '--arg=a'\\''b'"
        );
        assert!(options.working_directory.is_none());
        assert!(options.environment.is_empty());
        let params = initialize_params("/target/project", true).unwrap();
        assert_eq!(params["processId"], Value::Null);
        assert_eq!(params["rootUri"], "file:///target/project");
    }
    #[test]
    fn remote_discovery_preserves_target_paths_without_local_existence_checks() {
        let mut client = LspClient::with_config(
            "no-local-config",
            LspConfig {
                language_servers: vec![LanguageServerInfo {
                    language: "rust".into(),
                    server_paths: vec!["/remote/only/rust-analyzer".into()],
                    ..Default::default()
                }],
            },
        );
        assert_eq!(client.find_server_path("rust"), None);
        client.set_ssh_target(Some(bed_remote::SshTarget::new("host")));
        assert_eq!(
            client.find_server_path("rust"),
            Some(PathBuf::from("/remote/only/rust-analyzer"))
        );
    }
    #[test]
    fn initialize_advertises_original_features_and_utf16() {
        let params = initialize_params(env!("CARGO_MANIFEST_DIR"), false).unwrap();
        assert_eq!(params["clientInfo"]["name"], "bed");
        assert_eq!(params["capabilities"]["window"]["workDoneProgress"], true);
        assert_eq!(
            params["capabilities"]["general"]["positionEncodings"],
            json!(["utf-16"])
        );
        assert_eq!(
            params["capabilities"]["textDocument"]["definition"],
            json!({"linkSupport":true})
        );
        assert_eq!(
            params["workspaceFolders"][0]["name"],
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
        );
    }
    #[test]
    fn work_done_progress_preserves_phases_and_partial_results_are_ignored() {
        let mut state = ProgressState::default();
        state.apply(Some(&json!({"token":"index","value":{"kind":"begin","title":"Indexing","message":"First crate","percentage":1}}))).unwrap();
        state
            .apply(Some(
                &json!({"token":7,"value":{"kind":"begin","title":"Loading"}}),
            ))
            .unwrap();
        state.apply(Some(&json!({"token":"index","value":{"kind":"report","message":"Next crate","percentage":70}}))).unwrap();
        assert_eq!(
            state.snapshot()[0],
            WorkDoneProgress {
                token: ProgressToken::String("index".into()),
                title: "Indexing".into(),
                message: Some("Next crate".into()),
                percentage: Some(70),
                finished: false
            }
        );
        for value in [
            json!([{"uri":"file:///partial.rs"}]),
            json!({"kind":"custom","percentage":999}),
            json!(null),
        ] {
            state
                .apply(Some(&json!({"token":null,"value":value})))
                .unwrap();
        }
        assert_eq!(state.snapshot().len(), 2);
        state
            .apply(Some(
                &json!({"token":"index","value":{"kind":"end","message":"Ready"}}),
            ))
            .unwrap();
        assert_eq!(state.snapshot()[0].token, ProgressToken::Number(7));
        let done = &state.snapshot()[1];
        assert!(done.finished);
        assert_eq!(done.title, "Indexing");
        assert_eq!(done.message.as_deref(), Some("Ready"));
        state
            .apply(Some(
                &json!({"token":"index","value":{"kind":"begin","title":"Reloading"}}),
            ))
            .unwrap();
        assert_eq!(state.snapshot().len(), 2);
        assert!(state.snapshot().iter().all(|job| !job.finished));
    }
    #[test]
    fn work_done_progress_bounds_unicode_text_jobs_and_invalid_percentages() {
        let mut state = ProgressState::default();
        for token in 0..(MAX_ACTIVE_PROGRESS + 10) {
            state.apply(Some(&json!({"token":token,"value":{"kind":"begin","title":"Indexing","message":format!("\0{}", "🙂".repeat(MAX_PROGRESS_TEXT_BYTES))}}))).unwrap();
        }
        assert_eq!(state.snapshot().len(), MAX_ACTIVE_PROGRESS);
        for job in state.snapshot() {
            assert!(!job.message.as_ref().unwrap().contains('\0'));
            assert!(job.message.as_ref().unwrap().len() <= MAX_PROGRESS_TEXT_BYTES);
        }
        let before = state.snapshot();
        for percentage in [
            json!(-1),
            json!(101),
            json!(2.5),
            json!("25"),
            json!(u64::MAX),
        ] {
            assert!(
                state
                    .apply(Some(
                        &json!({"token":10,"value":{"kind":"report","percentage":percentage}})
                    ))
                    .is_err()
            );
            assert!(
                state
                    .apply(Some(
                        &json!({"token":2.5,"value":{"kind":"begin","title":"Fractional token"}})
                    ))
                    .is_err()
            );
        }
        assert_eq!(state.snapshot(), before);
        for token in 10..(MAX_ACTIVE_PROGRESS + 10) {
            state
                .apply(Some(&json!({"token":token,"value":{"kind":"end"}})))
                .unwrap();
        }
        assert_eq!(state.snapshot().len(), MAX_COMPLETED_PROGRESS);
        assert!(state.snapshot().iter().all(|job| job.finished));
        assert!(state.apply(Some(&json!({"token":"x".repeat(MAX_PROGRESS_TOKEN_BYTES + 1),"value":{"kind":"begin","title":"Overflow"}}))).is_err());
    }
    #[test]
    fn diagnostic_notifications_validate_whole_payload_and_keep_versions() {
        let store = LspDiagnostics::new();
        let uri = LspUri::file_uri_from_path("/tmp/diagnostic.rs")
            .unwrap()
            .to_string();
        let item = json!({"range":{"start":{"line":0,"character":2},"end":{"line":0,"character":4}},"message":{"kind":"markdown","value":"**message**"}});
        apply_diagnostics(
            &store,
            Some(&json!({"uri":uri,"version":2,"diagnostics":[item]})),
        )
        .unwrap();
        assert_eq!(
            store.for_document("/tmp/diagnostic.rs")[0].message,
            "**message**"
        );
        assert_eq!(store.for_document("/tmp/diagnostic.rs")[0].severity, 1);
        assert!(
            apply_diagnostics(&store, Some(&json!({"uri":uri,"diagnostics":[false]}))).is_err()
        );
        apply_diagnostics(
            &store,
            Some(&json!({"uri":uri,"version":1,"diagnostics":[]})),
        )
        .unwrap();
        assert_eq!(store.for_document("/tmp/diagnostic.rs").len(), 1);
    }
}
