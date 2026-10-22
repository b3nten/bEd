use crate::{transport::Transport, *};
use bedtime_editor2::{Diagnostic, DiagnosticSeverity};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io::Write,
    os::unix::net::UnixStream,
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

const COMMAND_BYTES: usize = 32 * 1024 * 1024;
const MAX_DOCUMENTS: usize = 128;
const MAX_REQUESTS: usize = 128;

enum Command {
    LanguageQuery {
        origin: RequestOrigin,
        query: LanguageQuery,
    },
    Cancel {
        panel: u64,
        kind: QueryKind,
    },
    Update(DocumentUpdate),
    Close(DocumentKey),
    Saved {
        document: DocumentKey,
        text: TextBuffer,
    },
    Definition {
        origin: RequestOrigin,
        byte: usize,
    },
}
struct Admitted {
    command: Option<Command>,
    bytes: usize,
    budget: Arc<AtomicUsize>,
}
impl Drop for Admitted {
    fn drop(&mut self) {
        self.budget.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

/// Nonblocking host handle. Dropping it requests retirement; no Drop waits.
pub struct ServerHandle {
    sender: mpsc::SyncSender<Admitted>,
    receiver: Mutex<mpsc::Receiver<Event>>,
    wake: Mutex<UnixStream>,
    budget: Arc<AtomicUsize>,
    status: Arc<Mutex<ServerStatus>>,
    activity: Arc<Mutex<Arc<ServerActivity>>>,
}
pub struct ServerWorker {
    launch: Launch,
    commands: mpsc::Receiver<Admitted>,
    events: mpsc::SyncSender<Event>,
    wake: UnixStream,
    status: Arc<Mutex<ServerStatus>>,
    activity: Arc<Mutex<Arc<ServerActivity>>>,
}
impl ServerHandle {
    /// Prepares channels only. The managed worker performs launch and all I/O.
    pub fn prepare(launch: Launch) -> Result<(Self, ServerWorker), String> {
        launch.config.validate()?;
        let (sender, commands) = mpsc::sync_channel(64);
        let (events, receiver) = mpsc::sync_channel(64);
        let (wake, worker_wake) = UnixStream::pair().map_err(|e| e.to_string())?;
        wake.set_nonblocking(true).map_err(|e| e.to_string())?;
        let status = Arc::new(Mutex::new(ServerStatus::Starting));
        let activity = Arc::new(Mutex::new(Arc::new(ServerActivity::default())));
        let budget = Arc::new(AtomicUsize::new(0));
        Ok((
            Self {
                sender,
                receiver: Mutex::new(receiver),
                wake: Mutex::new(wake),
                budget,
                status: status.clone(),
                activity: activity.clone(),
            },
            ServerWorker {
                launch,
                commands,
                events,
                wake: worker_wake,
                status,
                activity,
            },
        ))
    }
    fn admit(&self, command: Command, bytes: usize) -> Result<(), String> {
        if matches!(
            self.status(),
            ServerStatus::Failed(_) | ServerStatus::Stopped
        ) {
            return Err("Language server is stopped".into());
        }
        self.budget
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                n.checked_add(bytes).filter(|n| *n <= COMMAND_BYTES)
            })
            .map_err(|_| "Language server command backlog exceeds 32 MiB")?;
        let admitted = Admitted {
            command: Some(command),
            bytes,
            budget: self.budget.clone(),
        };
        self.sender.try_send(admitted).map_err(|e| match e {
            mpsc::TrySendError::Full(_) => "Language server command queue is full",
            mpsc::TrySendError::Disconnected(_) => "Language server worker has stopped",
        })?;
        // A full wake pipe already guarantees readiness; the queue owns commands.
        let _ = self.wake.lock().unwrap().write(&[1]);
        Ok(()) // Admission succeeded; the finite poll also observes a lost wake.
    }
    pub fn update(&self, update: DocumentUpdate) -> Result<(), String> {
        let bytes =
            update
                .text
                .len()
                .saturating_add(update.changes.as_ref().map_or(0, |(_, edits)| {
                    edits.iter().map(|e| e.replacement().len()).sum()
                }));
        self.admit(Command::Update(update), bytes)
    }
    pub fn close(&self, document: DocumentKey) -> Result<(), String> {
        self.admit(Command::Close(document), 0)
    }
    pub fn saved(&self, document: DocumentKey, text: TextBuffer) -> Result<(), String> {
        let bytes = text.len();
        self.admit(Command::Saved { document, text }, bytes)
    }
    pub fn definition(&self, origin: RequestOrigin, byte: usize) -> Result<(), String> {
        self.admit(Command::Definition { origin, byte }, 0)
    }
    pub fn query(&self, origin: RequestOrigin, query: LanguageQuery) -> Result<(), String> {
        let bytes = match &query {
            LanguageQuery::Resolve { item, .. } => item.bytes,
            _ => 0,
        };
        self.admit(Command::LanguageQuery { origin, query }, bytes)
    }
    pub fn cancel_query(&self, panel: u64, kind: QueryKind) -> Result<(), String> {
        self.admit(Command::Cancel { panel, kind }, 0)
    }
    pub fn status(&self) -> ServerStatus {
        self.status.lock().unwrap().clone()
    }
    /// Latest coalesced observations; progress/log floods cannot fill the
    /// lossless diagnostics/definition queue. Cloning shares the snapshot.
    pub fn activity(&self) -> Arc<ServerActivity> {
        self.activity.lock().unwrap().clone()
    }
    pub fn try_recv(&self) -> Result<Event, mpsc::TryRecvError> {
        self.receiver.lock().unwrap().try_recv()
    }
}
#[derive(Default)]
struct Capabilities {
    encoding: Encoding,
    open_close: bool,
    change: u64,
    save: bool,
    include_text: bool,
    definition: bool,
    hover: bool,
    completion: bool,
    resolve: bool,
}
struct SyncedDocument {
    update: DocumentUpdate,
    version: i32,
    opened: bool,
    saved: Option<TextBuffer>,
}
enum Pending {
    Initialize,
    Definition(RequestOrigin),
    LanguageQuery {
        origin: RequestOrigin,
        kind: QueryKind,
        byte: usize,
        item: Option<Value>,
    },
    Shutdown,
}
struct Request {
    kind: Pending,
    method: String,
    params: Value,
    bytes: usize,
    attempts: u8,
    retry_at: Option<Instant>,
    deadline: Instant,
}
struct Runtime {
    io: Transport,
    capabilities: Capabilities,
    ready: bool,
    documents: HashMap<DocumentKey, SyncedDocument>,
    requests: HashMap<i32, Request>,
    next_request: i32,
    next_version: i32,
    shutdown_answered: bool,
    activity: crate::activity::Activity,
    stderr_revision: u64,
    observed_at: Instant,
}
impl ServerWorker {
    pub fn run(self, cancelled: impl Fn() -> bool) {
        let status = self.status.clone();
        let activity = self.activity.clone();
        let result = self.run_inner(&cancelled);
        *status.lock().unwrap() = match result {
            Ok(()) => ServerStatus::Stopped,
            Err(_) if cancelled() => ServerStatus::Stopped,
            Err(error) => ServerStatus::Failed(error.into()),
        };
        let mut snapshot = (**activity.lock().unwrap()).clone();
        snapshot.progress.clear();
        snapshot.pending_requests = 0;
        *activity.lock().unwrap() = Arc::new(snapshot);
    }
    fn run_inner(self, cancelled: &impl Fn() -> bool) -> Result<(), String> {
        if cancelled() {
            return Ok(());
        }
        let root_uri = FileUri::from_path(&self.launch.root)?;
        let io = Transport::start(&self.launch, self.wake)?;
        let mut runtime = Runtime {
            io,
            capabilities: Capabilities::default(),
            ready: false,
            documents: HashMap::new(),
            requests: HashMap::new(),
            next_request: 0,
            next_version: 0,
            shutdown_answered: false,
            activity: Default::default(),
            stderr_revision: 0,
            observed_at: Instant::now(),
        };
        runtime.activity.value.process_id = Some(runtime.io.process_id());
        runtime.publish_activity(&self.activity);
        runtime.request("initialize", json!({
            "processId":std::process::id(),"clientInfo":{"name":"bed2"},"rootUri":root_uri.as_str(),
            "workspaceFolders":[{"uri":root_uri.as_str(),"name":self.launch.root.file_name().unwrap_or_default().to_string_lossy()}],
            "initializationOptions":self.launch.config.initialization_options,
                "capabilities":{"experimental":{"serverStatusNotification":true},"general":{"positionEncodings":["utf-8","utf-16"]},"window":{"workDoneProgress":true},
                "workspace":{"configuration":true,"workspaceFolders":true,"applyEdit":false},
                "textDocument":{"synchronization":{"didSave":true,"dynamicRegistration":false},"definition":{"linkSupport":true},
                    "hover":{"contentFormat":["markdown","plaintext"]},
                    "completion":{"contextSupport":true,"completionItem":{"snippetSupport":true,"documentationFormat":["markdown","plaintext"],"insertReplaceSupport":true,"resolveSupport":{"properties":["documentation","detail","additionalTextEdits"]}}},"publishDiagnostics":{"versionSupport":true}}}
        }), Pending::Initialize)?;
        let result = (|| {
            while !cancelled() {
                for _ in 0..32 {
                    if cancelled() {
                        return Ok(());
                    }
                    match self.commands.try_recv() {
                        Ok(mut admitted) => {
                            runtime.command(admitted.command.take().unwrap(), &self.events)?
                        }
                        Err(mpsc::TryRecvError::Empty) => break,
                        Err(mpsc::TryRecvError::Disconnected) => return Ok(()),
                    }
                }
                for message in runtime.io.pump(Duration::from_millis(100))? {
                    if cancelled() {
                        return Ok(());
                    }
                    runtime.message(message, &self.launch, &self.events)?;
                    if runtime.ready {
                        *self.status.lock().unwrap() = ServerStatus::Ready;
                    }
                }
                runtime.expire(&self.events)?;
                if runtime.observed_at.elapsed() >= Duration::from_millis(100) {
                    runtime.publish_activity(&self.activity);
                }
                if runtime.io.exited()? {
                    return Err("Language server exited".into());
                }
            }
            Ok(())
        })();
        let stderr =
            String::from_utf8_lossy(&runtime.io.stderr.iter().copied().collect::<Vec<_>>())
                .into_owned();
        runtime.shutdown(&self.launch, &self.events);
        runtime.activity.stopped();
        runtime.publish_activity(&self.activity);
        result.map_err(|error| {
            if stderr.trim().is_empty() {
                error
            } else {
                format!("{error}\n{}", stderr.trim())
            }
        })
    }
}
impl Runtime {
    fn publish_activity(&mut self, shared: &Mutex<Arc<ServerActivity>>) {
        self.observed_at = Instant::now();
        let value = &mut self.activity.value;
        if value.documents != self.documents.len() || value.pending_requests != self.requests.len()
        {
            value.documents = self.documents.len();
            value.pending_requests = self.requests.len();
            self.activity.dirty = true;
        }
        if self.stderr_revision != self.io.stderr_revision {
            value.stderr =
                String::from_utf8_lossy(&self.io.stderr.iter().copied().collect::<Vec<_>>())
                    .into_owned()
                    .into();
            self.stderr_revision = self.io.stderr_revision;
            self.activity.dirty = true;
        }
        if self.activity.dirty {
            *shared.lock().unwrap() = Arc::new(self.activity.value.clone());
            self.activity.dirty = false;
        }
    }
    fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        self.io
            .send(json!({"jsonrpc":"2.0","method":method,"params":params}))
    }
    fn request(&mut self, method: &str, params: Value, kind: Pending) -> Result<(), String> {
        let retained_bytes: usize = self.requests.values().map(|r| r.bytes).sum();
        let bytes = params.to_string().len();
        if retained_bytes.saturating_add(bytes) > COMMAND_BYTES {
            return Err("Pending language request payloads exceed 32 MiB".into());
        }
        if self.requests.len() >= MAX_REQUESTS {
            return Err("Too many language server requests".into());
        }
        self.next_request = self
            .next_request
            .checked_add(1)
            .ok_or("Language server request IDs exhausted; restart required")?;
        self.io.send(
            json!({"jsonrpc":"2.0","id":self.next_request,"method":method,"params":params}),
        )?;
        self.requests.insert(
            self.next_request,
            Request {
                kind,
                method: method.to_owned(),
                params,
                bytes,
                attempts: 0,
                retry_at: None,
                deadline: Instant::now() + Duration::from_secs(20),
            },
        );
        Ok(())
    }
    fn publish(events: &mpsc::SyncSender<Event>, event: Event) -> Result<(), String> {
        events
            .try_send(event)
            .map_err(|_| "Language server result queue is full or closed".into())
    }
    fn command(
        &mut self,
        command: Command,
        events: &mpsc::SyncSender<Event>,
    ) -> Result<(), String> {
        match command {
            Command::Cancel { panel, kind } => {
                self.cancel_queries(panel, kind)?;
            }
            Command::LanguageQuery { origin, query } => {
                let kind = query.kind();
                self.cancel_queries(origin.panel, kind)?;
                let byte = query.byte();
                let result = (|| {
                    if !self.ready
                        || !match kind {
                            QueryKind::Hover => self.capabilities.hover,
                            QueryKind::Completion => self.capabilities.completion,
                            QueryKind::Resolve => self.capabilities.resolve,
                        }
                    {
                        return Err(
                            "This language server has no ready provider for this request".into(),
                        );
                    }
                    let doc = self
                        .documents
                        .get(&origin.document)
                        .ok_or("LSP document is detached")?;
                    if doc.update.revision != origin.revision {
                        return Err("Language request is stale".into());
                    }
                    if self.capabilities.change == 0 && doc.update.revision != 0 {
                        return Err("Server does not synchronize document changes".into());
                    }
                    let position = self
                        .capabilities
                        .encoding
                        .position(&doc.update.text, byte)?;
                    let mut params =
                        json!({"textDocument":{"uri":doc.update.uri.as_str()},"position":position});
                    let (method, item) = match query {
                        LanguageQuery::Hover { .. } => ("textDocument/hover", None),
                        LanguageQuery::Completion { context, .. } => {
                            params["context"] = context;
                            ("textDocument/completion", None)
                        }
                        LanguageQuery::Resolve { item, .. } => {
                            params = (*item.value).clone();
                            ("completionItem/resolve", Some((*item.value).clone()))
                        }
                    };
                    self.request(
                        method,
                        params,
                        Pending::LanguageQuery {
                            origin,
                            kind,
                            byte,
                            item,
                        },
                    )
                })();
                if let Err(error) = result {
                    Self::publish(
                        events,
                        Event::Intelligence {
                            origin,
                            kind,
                            result: Err(error),
                        },
                    )?;
                }
            }
            Command::Update(mut update) => {
                if self
                    .documents
                    .iter()
                    .any(|(key, doc)| *key != update.key && doc.update.uri == update.uri)
                {
                    return Self::publish(
                        events,
                        Event::DocumentError {
                            document: update.key,
                            message:
                                "Another canonical document already owns this URI in the server"
                                    .into(),
                        },
                    );
                }
                if update.text.len() > MAX_SOURCE_BYTES {
                    self.close(update.key)?;
                    return Self::publish(
                        events,
                        Event::DocumentError {
                            document: update.key,
                            message: "LSP is limited to 8 MiB; plain editing remains available"
                                .into(),
                        },
                    );
                }
                if !self.documents.contains_key(&update.key)
                    && self.documents.len() >= MAX_DOCUMENTS
                {
                    return Self::publish(
                        events,
                        Event::DocumentError {
                            document: update.key,
                            message: "Server document limit (128) reached".into(),
                        },
                    );
                }
                let old = self.documents.remove(&update.key);
                if old.as_ref().is_some_and(|old| {
                    old.update.uri != update.uri || old.update.language != update.language
                }) {
                    return Err(
                        "LSP source identity changed without retiring its attachment".into(),
                    );
                }
                self.cancel_document(update.key, events)?;
                let (version, opened, saved) = if let Some(old) = old {
                    self.sync_update(&old, &update)?;
                    (
                        if old.update.revision == update.revision {
                            old.version
                        } else {
                            self.next_version
                        },
                        old.opened,
                        old.saved,
                    )
                } else {
                    if self.ready {
                        self.open(&update)?;
                    }
                    (
                        self.next_version,
                        self.ready && self.capabilities.open_close,
                        None,
                    )
                };
                // The retained canonical root is sufficient for the next batch.
                // Applied journal entries can otherwise retain obsolete history.
                update.changes = None;
                self.documents.insert(
                    update.key,
                    SyncedDocument {
                        update,
                        version,
                        opened,
                        saved,
                    },
                );
            }
            Command::Close(key) => {
                self.cancel_document(key, events)?;
                self.close(key)?;
            }
            Command::Saved { document, text } => {
                if let Some(mut doc) = self.documents.remove(&document) {
                    if self.ready {
                        self.save(&doc.update.uri, &text)?;
                    } else {
                        doc.saved = Some(text);
                    }
                    self.documents.insert(document, doc);
                }
            }
            Command::Definition { origin, byte } => {
                let result = (|| {
                    if !self.ready || !self.capabilities.definition {
                        return Err("This language server has no ready definition provider".into());
                    }
                    let doc = self
                        .documents
                        .get(&origin.document)
                        .ok_or("LSP document is detached")?;
                    if doc.update.revision != origin.revision {
                        return Err("Definition request is stale".into());
                    }
                    if self.capabilities.change == 0 && doc.version > 0 && doc.update.revision != 0
                    {
                        return Err("Server does not synchronize document changes".into());
                    }
                    let position = self
                        .capabilities
                        .encoding
                        .position(&doc.update.text, byte)?;
                    self.request(
                        "textDocument/definition",
                        json!({"textDocument":{"uri":doc.update.uri.as_str()},"position":position}),
                        Pending::Definition(origin),
                    )
                })();
                if let Err(error) = result {
                    Self::publish(
                        events,
                        Event::Definition {
                            origin,
                            result: Err(error),
                        },
                    )?;
                }
            }
        }
        Ok(())
    }
    fn cancel_queries(&mut self, panel: u64, kind: QueryKind) -> Result<(), String> {
        let ids: Vec<_> = self.requests.iter().filter_map(|(id,r)| matches!(r.kind, Pending::LanguageQuery { origin, kind: k, .. } if origin.panel == panel && k == kind).then_some(*id)).collect();
        for id in ids {
            self.requests.remove(&id);
            self.notify("$/cancelRequest", json!({"id":id}))?;
        }
        Ok(())
    }
    fn version(&mut self) -> Result<i32, String> {
        self.next_version = self
            .next_version
            .checked_add(1)
            .ok_or("LSP versions exhausted; restart required")?;
        Ok(self.next_version)
    }
    fn open(&mut self, update: &DocumentUpdate) -> Result<(), String> {
        let version = self.version()?;
        if self.capabilities.open_close {
            self.notify("textDocument/didOpen",json!({"textDocument":{"uri":update.uri.as_str(),"languageId":update.language,"version":version,"text":update.text.to_string()}}))?;
        }
        Ok(())
    }
    fn sync_update(&mut self, old: &SyncedDocument, update: &DocumentUpdate) -> Result<(), String> {
        if !self.ready || old.update.revision == update.revision {
            return Ok(());
        }
        let version = self.version()?;
        if self.capabilities.change == 0 {
            return Ok(());
        }
        let mut changes = Vec::new();
        if self.capabilities.change == 2 {
            if let Some((base, edits)) = &update.changes {
                if *base == old.update.revision {
                    let mut text = old.update.text.clone();
                    for edit in edits {
                        let range = edit.range();
                        // Canonical UTF-8 edits can split CRLF (for example a
                        // literal Find/Replace). LSP has no position between
                        // CR and LF; synchronize that batch as a full refresh.
                        let (Ok(start), Ok(end)) = (
                            self.capabilities.encoding.position(&text, range.start),
                            self.capabilities.encoding.position(&text, range.end),
                        ) else {
                            changes.clear();
                            break;
                        };
                        changes.push(json!({"range":{"start":start,"end":end},"text":edit.replacement().to_string()}));
                        edit.apply(&mut text)?;
                    }
                }
            }
        }
        if changes.is_empty() {
            changes.push(json!({"text":update.text.to_string()}));
        }
        self.notify("textDocument/didChange",json!({"textDocument":{"uri":update.uri.as_str(),"version":version},"contentChanges":changes}))
    }
    fn close(&mut self, key: DocumentKey) -> Result<(), String> {
        if let Some(doc) = self.documents.remove(&key) {
            if doc.opened {
                self.notify(
                    "textDocument/didClose",
                    json!({"textDocument":{"uri":doc.update.uri.as_str()}}),
                )?;
            }
        }
        Ok(())
    }
    fn save(&mut self, uri: &FileUri, text: &TextBuffer) -> Result<(), String> {
        if !self.capabilities.save {
            return Ok(());
        }
        let mut params = json!({"textDocument":{"uri":uri.as_str()}});
        if self.capabilities.include_text {
            params["text"] = Value::String(text.to_string());
        }
        self.notify("textDocument/didSave", params)
    }
    fn cancel_document(
        &mut self,
        document: DocumentKey,
        events: &mpsc::SyncSender<Event>,
    ) -> Result<(), String> {
        let ids: Vec<_> = self
            .requests
            .iter()
            .filter_map(|(id, r)| {
                matches!(r.kind,Pending::Definition(origin) | Pending::LanguageQuery { origin, .. } if origin.document == document)
                    .then_some(*id)
            })
            .collect();
        for id in ids {
            let request = self.requests.remove(&id).unwrap();
            self.notify("$/cancelRequest", json!({"id":id}))?;
            if let Pending::Definition(origin) = request.kind {
                Self::publish(
                    events,
                    Event::Definition {
                        origin,
                        result: Err("Document changed".into()),
                    },
                )?;
            }
        }
        Ok(())
    }
    fn message(
        &mut self,
        message: Value,
        launch: &Launch,
        events: &mpsc::SyncSender<Event>,
    ) -> Result<(), String> {
        if let Value::Array(messages) = message {
            for message in messages {
                self.message(message, launch, events)?;
            }
            return Ok(());
        }
        self.activity.value.last_activity = Some(Instant::now());
        self.activity.dirty = true;
        if let Some(method) = message.get("method") {
            let method = method.as_str().ok_or("Invalid LSP method")?;
            if let Some(id) = message.get("id") {
                if !(id.is_string() || id.as_i64().is_some_and(|n| i32::try_from(n).is_ok())) {
                    return Err("Invalid LSP request ID".into());
                }
                let params = &message["params"];
                let result = match method {
                    "workspace/configuration" => {
                        let items = params["items"]
                            .as_array()
                            .ok_or("Invalid configuration request")?;
                        if items.len() > 1024 {
                            return Err("Configuration request exceeds limit".into());
                        }
                        Some(Value::Array(
                            items
                                .iter()
                                .map(|item| {
                                    let section = item["section"].as_str().unwrap_or("");
                                    if section.is_empty() {
                                        launch.config.settings.clone()
                                    } else if let Some(value) = launch.config.settings.get(section)
                                    {
                                        value.clone()
                                    } else {
                                        section
                                            .split('.')
                                            .fold(Some(&launch.config.settings), |v, k| {
                                                v.and_then(|v| v.get(k))
                                            })
                                            .cloned()
                                            .unwrap_or(Value::Null)
                                    }
                                })
                                .collect(),
                        ))
                    }
                    "workspace/workspaceFolders" => Some(
                        json!([{"uri":FileUri::from_path(&launch.root)?.as_str(),"name":launch.root.file_name().unwrap_or_default().to_string_lossy()}]),
                    ),
                    "window/workDoneProgress/create" => {
                        if let Err(error) = self.activity.create(&params["token"]) {
                            self.io.send(json!({"jsonrpc":"2.0","id":id,"error":{"code":-32602,"message":error}}))?;
                            return Ok(());
                        }
                        Some(Value::Null)
                    }
                    "window/showMessageRequest" => {
                        self.activity.message(params);
                        Some(Value::Null)
                    }
                    "workspace/applyEdit" => Some(
                        json!({"applied":false,"failureReason":"Workspace edits are not supported"}),
                    ),
                    _ => None,
                };
                self.io.send(match result { Some(result) => json!({"jsonrpc":"2.0","id":id,"result":result}), None => json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Method not supported"}}) })?;
            } else if method == "textDocument/publishDiagnostics" && self.ready {
                self.diagnostics(&message["params"], events)?;
            } else if method == "experimental/serverStatus" {
                if let Err(error) = self.activity.project_status(&message["params"]) {
                    self.activity.log(MessageKind::Warning, &error);
                }
            } else if method == "$/progress" {
                if let Err(error) = self.activity.progress(&message["params"]) {
                    self.activity.log(MessageKind::Warning, &error);
                }
            } else if matches!(method, "window/logMessage" | "window/showMessage") {
                self.activity.message(&message["params"]);
            }
            return Ok(());
        }
        let id = message["id"]
            .as_i64()
            .and_then(|id| i32::try_from(id).ok())
            .ok_or("Invalid LSP response ID")?;
        if message.get("result").is_some() == message.get("error").is_some() {
            return Err("LSP response requires exactly one result or error".into());
        }
        let Some(mut request) = self.requests.remove(&id) else {
            return Ok(());
        }; // Late cancelled reply.
        let empty = message
            .get("result")
            .is_some_and(|v| v.is_null() || v.as_array().is_some_and(Vec::is_empty));
        if empty
            && self
                .activity
                .value
                .project_status
                .as_ref()
                .is_some_and(|s| !s.quiescent)
            && matches!(
                request.kind,
                Pending::Definition(_)
                    | Pending::LanguageQuery {
                        kind: QueryKind::Hover,
                        ..
                    }
            )
        {
            request.retry_at = Some(Instant::now() + Duration::from_millis(100));
            self.requests.insert(id, request);
            return Ok(());
        }
        // Servers can invalidate analysis during workspace reloads independently
        // of document edits. Retry only explicit transient protocol errors, with
        // the captured origin and a finite backoff; ordinary failures stay errors.
        if matches!(message["error"]["code"].as_i64(), Some(-32801 | -32802))
            && request.attempts < 3
            && matches!(
                request.kind,
                Pending::Definition(_) | Pending::LanguageQuery { .. }
            )
        {
            request.attempts += 1;
            request.retry_at =
                Some(Instant::now() + Duration::from_millis(100 << (request.attempts - 1)));
            self.requests.insert(id, request);
            return Ok(());
        }
        let result = if let Some(error) = message.get("error") {
            Err(error["message"]
                .as_str()
                .unwrap_or("Language server request failed")
                .to_owned())
        } else {
            Ok(&message["result"])
        };
        match request.kind {
            Pending::Initialize => {
                let result = result?;
                self.capabilities = capabilities(result)?;
                self.activity.value.server_name = result["serverInfo"]["name"]
                    .as_str()
                    .map(crate::activity::bounded);
                self.activity.value.server_version = result["serverInfo"]["version"]
                    .as_str()
                    .map(crate::activity::bounded);
                self.activity.value.encoding = Some(self.capabilities.encoding);
                self.activity.value.synchronization = Some(match self.capabilities.change {
                    1 => "Full",
                    2 => "Incremental",
                    _ => "None",
                });
                self.activity.value.supports_definition = self.capabilities.definition;
                self.activity.value.supports_hover = self.capabilities.hover;
                self.activity.value.supports_completion = self.capabilities.completion;
                self.activity.value.supports_completion_resolve = self.capabilities.resolve;
                self.activity.value.completion_triggers =
                    result["capabilities"]["completionProvider"]["triggerCharacters"]
                        .as_array()
                        .map(|values| {
                            values
                                .iter()
                                .filter_map(Value::as_str)
                                .take(64)
                                .map(str::to_owned)
                                .collect()
                        })
                        .unwrap_or_default();
                self.notify("initialized", json!({}))?;
                self.notify(
                    "workspace/didChangeConfiguration",
                    json!({"settings":launch.config.settings}),
                )?;
                self.ready = true;
                let docs = std::mem::take(&mut self.documents);
                for (key, mut doc) in docs {
                    self.open(&doc.update)?;
                    doc.version = self.next_version;
                    doc.opened = self.capabilities.open_close;
                    if let Some(text) = doc.saved.take() {
                        self.save(&doc.update.uri, &text)?;
                    }
                    self.documents.insert(key, doc);
                }
            }
            Pending::Definition(origin) => {
                let result = result.and_then(|value| locations(value, self.capabilities.encoding));
                Self::publish(events, Event::Definition { origin, result })?;
            }
            Pending::LanguageQuery {
                origin,
                kind,
                byte,
                item,
            } => {
                let parsed = result.and_then(|value| {
                    let doc = self
                        .documents
                        .get(&origin.document)
                        .ok_or("LSP document is detached")?;
                    if doc.update.revision != origin.revision {
                        return Err("Language request is stale".into());
                    }
                    match kind {
                        QueryKind::Hover => Ok(Intelligence::Hover(parse_hover(
                            value,
                            &doc.update.text,
                            self.capabilities.encoding,
                        )?)),
                        QueryKind::Completion => Ok(Intelligence::Completion(parse_completions(
                            value,
                            &doc.update.text,
                            byte,
                            self.capabilities.encoding,
                        )?)),
                        QueryKind::Resolve => {
                            let mut merged = item.unwrap_or_default();
                            let object = value.as_object().ok_or("Invalid resolved completion")?;
                            for (key, value) in object {
                                merged[key] = value.clone();
                            }
                            Ok(Intelligence::Resolved(parse_completion(
                                &merged,
                                &doc.update.text,
                                byte,
                                self.capabilities.encoding,
                            )?))
                        }
                    }
                });
                Self::publish(
                    events,
                    Event::Intelligence {
                        origin,
                        kind,
                        result: parsed,
                    },
                )?;
            }
            Pending::Shutdown => self.shutdown_answered = true,
        }
        Ok(())
    }
    fn diagnostics(
        &mut self,
        params: &Value,
        events: &mpsc::SyncSender<Event>,
    ) -> Result<(), String> {
        let uri = FileUri::parse(params["uri"].as_str().ok_or("Diagnostic URI missing")?)?;
        let Some(doc) = self.documents.values().find(|doc| doc.update.uri == uri) else {
            return Ok(());
        };
        let version = match params.get("version").filter(|v| !v.is_null()) {
            Some(v) => Some(
                v.as_i64()
                    .and_then(|v| i32::try_from(v).ok())
                    .ok_or("Invalid diagnostic version")?,
            ),
            None => None,
        };
        if version.is_some_and(|v| v != doc.version) {
            return Ok(());
        }
        let raw = params["diagnostics"]
            .as_array()
            .ok_or("Invalid diagnostics")?;
        if raw.len() > 10000 {
            return Self::publish(
                events,
                Event::DocumentError {
                    document: doc.update.key,
                    message: "Diagnostic report exceeds 10,000 entries".into(),
                },
            );
        }
        let mut items = Vec::with_capacity(raw.len());
        for value in raw {
            let range = serde_json::from_value(value["range"].clone())
                .map_err(|_| "Invalid diagnostic range")?;
            let range = self.capabilities.encoding.range(&doc.update.text, range)?;
            let message = value["message"]
                .as_str()
                .ok_or("Invalid diagnostic message")?;
            let message: String = message.chars().take(4096).collect();
            let severity = match value["severity"].as_u64().unwrap_or(1) {
                1 => DiagnosticSeverity::Error,
                2 => DiagnosticSeverity::Warning,
                3 => DiagnosticSeverity::Information,
                _ => DiagnosticSeverity::Hint,
            };
            items.push(Diagnostic {
                range,
                severity,
                message: message.into(),
            });
        }
        let result = Self::publish(
            events,
            Event::Diagnostics {
                document: doc.update.key,
                report: Arc::new(Diagnostics::new(
                    doc.update.revision,
                    version.is_some(),
                    items,
                )),
            },
        );
        if result.is_ok() {
            self.activity.analysis_changed();
        }
        result
    }
    fn expire(&mut self, events: &mpsc::SyncSender<Event>) -> Result<(), String> {
        let retry_ids: Vec<_> = self
            .requests
            .iter()
            .filter_map(|(id, r)| r.retry_at.filter(|at| *at <= Instant::now()).map(|_| *id))
            .collect();
        for id in retry_ids {
            let mut request = self.requests.remove(&id).unwrap();
            if request.deadline <= Instant::now() {
                self.requests.insert(id, request);
                continue;
            }
            let origin = match request.kind {
                Pending::Definition(origin) | Pending::LanguageQuery { origin, .. } => origin,
                _ => unreachable!(),
            };
            if !self
                .documents
                .get(&origin.document)
                .is_some_and(|doc| doc.update.revision == origin.revision)
            {
                continue;
            }
            if request.deadline > Instant::now()
                && self
                    .activity
                    .value
                    .project_status
                    .as_ref()
                    .is_some_and(|s| !s.quiescent)
                && matches!(
                    request.kind,
                    Pending::Definition(_)
                        | Pending::LanguageQuery {
                            kind: QueryKind::Hover,
                            ..
                        }
                )
            {
                request.retry_at = Some(Instant::now() + Duration::from_millis(100));
                self.requests.insert(id, request);
                continue;
            }
            self.next_request = self
                .next_request
                .checked_add(1)
                .ok_or("Language request IDs exhausted")?;
            self.io.send(json!({"jsonrpc":"2.0","id":self.next_request,"method":request.method,"params":request.params}))?;
            request.retry_at = None;
            self.requests.insert(self.next_request, request);
        }
        let ids: Vec<_> = self
            .requests
            .iter()
            .filter_map(|(id, r)| (r.deadline <= Instant::now()).then_some(*id))
            .collect();
        for id in ids {
            let request = self.requests.remove(&id).unwrap();
            match request.kind {
                Pending::Initialize => {
                    return Err("Language server initialization timed out".into());
                }
                Pending::Definition(origin) => {
                    self.notify("$/cancelRequest", json!({"id":id}))?;
                    Self::publish(
                        events,
                        Event::Definition {
                            origin,
                            result: Err("Definition request timed out".into()),
                        },
                    )?;
                }
                Pending::LanguageQuery { origin, kind, .. } => {
                    self.notify("$/cancelRequest", json!({"id":id}))?;
                    Self::publish(
                        events,
                        Event::Intelligence {
                            origin,
                            kind,
                            result: Err("Language request timed out".into()),
                        },
                    )?;
                }
                Pending::Shutdown => {}
            }
        }
        Ok(())
    }
    fn shutdown(&mut self, launch: &Launch, events: &mpsc::SyncSender<Event>) {
        self.requests.clear();
        if self.ready
            && self
                .request("shutdown", Value::Null, Pending::Shutdown)
                .is_ok()
        {
            let until = Instant::now() + Duration::from_millis(500);
            while !self.shutdown_answered && Instant::now() < until {
                let Ok(messages) = self.io.pump(Duration::from_millis(20)) else {
                    break;
                };
                for message in messages {
                    if message.get("method").is_none() {
                        let _ = self.message(message, launch, events);
                    }
                }
                if self.io.exited().unwrap_or(true) {
                    break;
                }
            }
        }
        let _ = self.notify("exit", Value::Null);
        let until = Instant::now() + Duration::from_millis(100);
        // Draining exit bytes is not the same as the server having consumed
        // them. Give it the remaining grace period to exit before signalling.
        while Instant::now() < until && !self.io.exited().unwrap_or(true) {
            if self.io.pump(Duration::from_millis(10)).is_err() {
                break;
            }
        }
        self.io.stop();
    }
}
fn capabilities(result: &Value) -> Result<Capabilities, String> {
    let c = result["capabilities"]
        .as_object()
        .ok_or("Invalid initialize capabilities")?;
    let encoding = match c
        .get("positionEncoding")
        .and_then(Value::as_str)
        .unwrap_or("utf-16")
    {
        "utf-8" => Encoding::Utf8,
        "utf-16" => Encoding::Utf16,
        _ => return Err("Unsupported server position encoding".into()),
    };
    let mut out = Capabilities {
        encoding,
        definition: c
            .get("definitionProvider")
            .is_some_and(|v| v == true || v.is_object()),
        hover: c
            .get("hoverProvider")
            .is_some_and(|v| v == true || v.is_object()),
        completion: c.get("completionProvider").is_some_and(Value::is_object),
        resolve: c
            .get("completionProvider")
            .is_some_and(|v| v["resolveProvider"] == true),
        ..Default::default()
    };
    if let Some(sync) = c.get("textDocumentSync").filter(|s| !s.is_null()) {
        if let Some(options) = sync.as_object() {
            out.open_close = match options.get("openClose") {
                Some(Value::Bool(v)) => *v,
                None => false,
                _ => return Err("Invalid openClose capability".into()),
            };
            out.change = match options.get("change") {
                Some(v) => v
                    .as_u64()
                    .filter(|n| *n <= 2)
                    .ok_or("Invalid synchronization kind")?,
                None => 0,
            };
            match options.get("save").filter(|v| !v.is_null()) {
                Some(Value::Bool(value)) => out.save = *value,
                Some(Value::Object(save)) => {
                    out.save = true;
                    out.include_text = match save.get("includeText") {
                        Some(Value::Bool(v)) => *v,
                        None => false,
                        _ => return Err("Invalid save includeText capability".into()),
                    };
                }
                None => {}
                _ => return Err("Invalid save capability".into()),
            }
        } else {
            out.change = sync
                .as_u64()
                .filter(|n| *n <= 2)
                .ok_or("Invalid synchronization kind")?;
            out.open_close = out.change != 0;
        }
    }
    Ok(out)
}
fn locations(value: &Value, encoding: Encoding) -> Result<Vec<Location>, String> {
    if value.is_null() {
        return Ok(Vec::new());
    }
    let values = if let Some(values) = value.as_array() {
        values.as_slice()
    } else {
        std::slice::from_ref(value)
    };
    if values.len() > 128 {
        return Err("Definition result exceeds 128 targets".into());
    }
    values
        .iter()
        .map(|v| {
            let uri = FileUri::parse(
                v.get("targetUri")
                    .or_else(|| v.get("uri"))
                    .and_then(Value::as_str)
                    .ok_or("Missing definition URI")?,
            )?;
            let range = serde_json::from_value(
                v.get("targetSelectionRange")
                    .or_else(|| v.get("range"))
                    .ok_or("Missing definition range")?
                    .clone(),
            )
            .map_err(|_| "Invalid definition range")?;
            Ok(Location {
                uri,
                range,
                encoding,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn capabilities_follow_protocol_defaults_and_boolean_semantics() {
        let c = capabilities(&json!({"capabilities":{}})).unwrap();
        assert_eq!(c.change, 0);
        assert!(!c.open_close);
        assert!(!c.save);
        assert_eq!(c.encoding, Encoding::Utf16);
        let c=capabilities(&json!({"capabilities":{"positionEncoding":"utf-8","textDocumentSync":{"openClose":false,"change":2,"save":false}}})).unwrap();
        assert_eq!(c.encoding, Encoding::Utf8);
        assert!(!c.open_close);
        assert!(!c.save);
        for c in [
            json!({"textDocumentSync":2.5}),
            json!({"positionEncoding":"utf-32"}),
            json!({"textDocumentSync":{"change":4}}),
        ] {
            assert!(capabilities(&json!({"capabilities":c})).is_err());
        }
    }
}
