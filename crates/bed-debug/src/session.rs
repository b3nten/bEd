use crate::transport::{DapTransport, TransportEvent};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    io,
    path::Path,
    time::{Duration, Instant},
};

#[derive(Clone, Debug)]
pub struct LaunchConfig {
    pub program: String,
    pub cwd: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub source_map: Vec<[String; 2]>,
    pub stop_on_entry: bool,
    pub breakpoints: BTreeMap<String, Vec<SourceBreakpoint>>,
    pub init_commands: Vec<String>,
}
impl Default for LaunchConfig {
    fn default() -> Self {
        Self {
            program: String::new(),
            cwd: String::new(),
            args: Vec::new(),
            env: BTreeMap::new(),
            source_map: Vec::new(),
            stop_on_entry: true,
            breakpoints: BTreeMap::new(),
            init_commands: Vec::new(),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionState {
    Initializing,
    Launching,
    Running,
    Stopped,
    Stopping,
    Terminated,
    Failed,
}
impl SessionState {
    pub fn is_finished(self) -> bool {
        matches!(self, Self::Terminated | Self::Failed)
    }
}
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct SourceBreakpoint {
    pub line: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hit_condition: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_message: Option<String>,
}
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Breakpoint {
    pub id: Option<i64>,
    pub verified: bool,
    pub line: Option<usize>,
    pub message: Option<String>,
}
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Thread {
    pub id: i64,
    pub name: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StackFrame {
    pub id: i64,
    pub name: String,
    pub source: Option<String>,
    pub line: usize,
    pub column: usize,
}
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Scope {
    pub name: String,
    pub variables_reference: i64,
    #[serde(default)]
    pub expensive: bool,
}
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Variable {
    pub name: String,
    pub value: String,
    #[serde(default, rename = "type")]
    pub type_name: Option<String>,
    #[serde(default)]
    pub variables_reference: i64,
    #[serde(default)]
    pub evaluate_name: Option<String>,
}
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EvaluateResult {
    pub result: String,
    #[serde(default, rename = "type")]
    pub type_name: Option<String>,
    #[serde(default)]
    pub variables_reference: i64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EvaluateContext {
    Watch,
    Hover,
    Repl,
}
impl EvaluateContext {
    fn as_str(self) -> &'static str {
        match self {
            Self::Watch => "watch",
            Self::Hover => "hover",
            Self::Repl => "repl",
        }
    }
}
#[derive(Clone, Debug)]
pub struct RunInTerminalRequest {
    pub request_seq: u64,
    pub args: Vec<String>,
    pub cwd: String,
    pub env: BTreeMap<String, Option<String>>,
    pub title: String,
}
#[derive(Clone, Debug)]
pub enum DebugEvent {
    Changed,
    Output {
        category: String,
        output: String,
    },
    Error(String),
    RunInTerminal(RunInTerminalRequest),
    Breakpoints {
        request_id: u64,
        path: String,
        breakpoints: Vec<Breakpoint>,
    },
    BreakpointChanged {
        reason: String,
        breakpoint: Breakpoint,
    },
    Evaluated {
        request_id: u64,
        result: Result<EvaluateResult, String>,
    },
}
#[derive(Clone, Debug)]
enum PendingKind {
    Initialize,
    Launch,
    Configure,
    Breakpoints(String),
    Threads,
    Frames,
    Scopes,
    Variables(i64),
    Evaluate,
    Resume,
    Pause,
    Disconnect,
}
struct Pending {
    kind: PendingKind,
    stop_generation: u64,
    selection_generation: u64,
    deadline: Instant,
}

/// One launched debuggee. This is polled on the host thread; stdio workers only
/// publish protocol messages. Frame/variable references never survive a resume.
pub struct DebugSession {
    pub state: SessionState,
    pub threads: Vec<Thread>,
    pub frames: Vec<StackFrame>,
    pub scopes: Vec<Scope>,
    pub variables: HashMap<i64, Vec<Variable>>,
    pub selected_thread: Option<i64>,
    pub selected_frame: Option<i64>,
    pub stop_generation: u64,
    pub stop_reason: String,
    pub exit_code: Option<i64>,
    transport: DapTransport,
    config: LaunchConfig,
    seq: u64,
    pending: HashMap<u64, Pending>,
    terminal_requests: HashSet<u64>,
    breakpoint_requests: HashMap<String, u64>,
    initialized: bool,
    configure_sent: bool,
    startup_breakpoints: usize,
    selection_generation: u64,
    supports_restart: bool,
    events: Vec<DebugEvent>,
}
impl DebugSession {
    pub fn launch(adapter: &Path, config: LaunchConfig) -> io::Result<Self> {
        Self::launch_with_args(adapter, &[], config)
    }
    pub fn launch_with_args(
        adapter: &Path,
        arguments: &[String],
        config: LaunchConfig,
    ) -> io::Result<Self> {
        if config.program.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Executable path is empty",
            ));
        }
        let cwd = (!config.cwd.is_empty()).then(|| Path::new(&config.cwd));
        let transport = DapTransport::spawn(adapter, arguments, cwd)?;
        let mut this = Self {
            state: SessionState::Initializing,
            threads: Vec::new(),
            frames: Vec::new(),
            scopes: Vec::new(),
            variables: HashMap::new(),
            selected_thread: None,
            selected_frame: None,
            stop_generation: 0,
            stop_reason: String::new(),
            exit_code: None,
            transport,
            config,
            seq: 1,
            pending: HashMap::new(),
            terminal_requests: HashSet::new(),
            breakpoint_requests: HashMap::new(),
            initialized: false,
            configure_sent: false,
            startup_breakpoints: 0,
            selection_generation: 0,
            supports_restart: false,
            events: Vec::new(),
        };
        this.request("initialize", json!({
            "clientID":"bed", "clientName":"bEd", "adapterID":"lldb", "locale":"en",
            "linesStartAt1":true, "columnsStartAt1":true, "pathFormat":"path", "supportsVariableType":true,
            "supportsRunInTerminalRequest":true, "supportsVariablePaging":true, "supportsProgressReporting":false,
            "supportsInvalidatedEvent":true, "supportsMemoryReferences":false
        }), PendingKind::Initialize)?;
        Ok(this)
    }
    pub fn adapter_process_id(&self) -> u32 {
        self.transport.process_id()
    }
    pub fn adapter_stderr(&self) -> String {
        self.transport.stderr()
    }
    fn request(&mut self, command: &str, arguments: Value, kind: PendingKind) -> io::Result<u64> {
        if self.pending.len() >= 256 {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Too many pending debugger requests",
            ));
        }
        let seq = self.seq;
        self.seq += 1;
        self.transport
            .send(&json!({"seq":seq,"type":"request","command":command,"arguments":arguments}))?;
        let timeout = match kind {
            PendingKind::Launch | PendingKind::Initialize => 30,
            PendingKind::Disconnect => 2,
            _ => 15,
        };
        self.pending.insert(
            seq,
            Pending {
                kind,
                stop_generation: self.stop_generation,
                selection_generation: self.selection_generation,
                deadline: Instant::now() + Duration::from_secs(timeout),
            },
        );
        Ok(seq)
    }
    pub fn poll(&mut self) -> Vec<DebugEvent> {
        for event in self.transport.poll() {
            if self.state.is_finished() {
                break;
            }
            match event {
                TransportEvent::Message(message) => {
                    if let Err(error) = self.handle_message(message) {
                        self.fail(error.to_string());
                    }
                }
                TransportEvent::Closed(error) => {
                    if self.state == SessionState::Stopping || self.exit_code.is_some() {
                        self.state = SessionState::Terminated;
                        self.invalidate_inspection();
                        self.transport.shutdown();
                        self.events.push(DebugEvent::Changed);
                    } else {
                        let stderr = self.transport.stderr();
                        self.fail(if stderr.is_empty() {
                            error
                        } else {
                            format!("{error}\n{stderr}")
                        });
                    }
                }
            }
        }
        if !self.state.is_finished() {
            let expired: Vec<_> = self
                .pending
                .iter()
                .filter_map(|(id, p)| (p.deadline <= Instant::now()).then_some(*id))
                .collect();
            for id in expired {
                let Some(pending) = self.pending.remove(&id) else {
                    continue;
                };
                match pending.kind {
                    PendingKind::Disconnect => {
                        self.state = SessionState::Terminated;
                        self.transport.shutdown();
                        self.events.push(DebugEvent::Changed);
                    }
                    PendingKind::Initialize | PendingKind::Launch | PendingKind::Configure => {
                        self.fail("Debugger startup timed out".into())
                    }
                    PendingKind::Evaluate => self.events.push(DebugEvent::Evaluated {
                        request_id: id,
                        result: Err("Expression evaluation timed out".into()),
                    }),
                    PendingKind::Breakpoints(_) => {
                        self.startup_breakpoints = self.startup_breakpoints.saturating_sub(1);
                        self.events
                            .push(DebugEvent::Error("Setting breakpoints timed out".into()));
                        if let Err(error) = self.finish_configuration() {
                            self.fail(error.to_string());
                        }
                    }
                    _ => self
                        .events
                        .push(DebugEvent::Error("Debugger request timed out".into())),
                }
            }
        }
        std::mem::take(&mut self.events)
    }
    fn fail(&mut self, error: String) {
        if self.state.is_finished() {
            return;
        }
        self.state = SessionState::Failed;
        self.invalidate_inspection();
        self.pending.clear();
        self.terminal_requests.clear();
        self.transport.shutdown();
        self.events.push(DebugEvent::Error(error));
        self.events.push(DebugEvent::Changed);
    }
    fn invalidate_inspection(&mut self) {
        self.stop_generation += 1;
        self.selection_generation += 1;
        self.frames.clear();
        self.scopes.clear();
        self.variables.clear();
        self.selected_frame = None;
    }
    fn require_stopped(&self) -> io::Result<()> {
        if self.state != SessionState::Stopped {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Pause the program first",
            ));
        }
        Ok(())
    }
    pub fn set_breakpoints(
        &mut self,
        path: impl Into<String>,
        breakpoints: Vec<SourceBreakpoint>,
    ) -> io::Result<u64> {
        let path = path.into();
        self.config
            .breakpoints
            .insert(path.clone(), breakpoints.clone());
        if !self.initialized {
            return Ok(0);
        }
        let seq = self.request(
            "setBreakpoints",
            json!({"source":{"path":path},"breakpoints":breakpoints,"sourceModified":false}),
            PendingKind::Breakpoints(path),
        )?;
        if let Some(Pending {
            kind: PendingKind::Breakpoints(path),
            ..
        }) = self.pending.get(&seq)
        {
            self.breakpoint_requests.insert(path.clone(), seq);
        }
        if !self.configure_sent {
            self.startup_breakpoints += 1;
        }
        Ok(seq)
    }
    fn finish_configuration(&mut self) -> io::Result<()> {
        if self.initialized && !self.configure_sent && self.startup_breakpoints == 0 {
            self.configure_sent = true;
            self.request("configurationDone", json!({}), PendingKind::Configure)?;
        }
        Ok(())
    }
    pub fn continue_execution(&mut self) -> io::Result<u64> {
        self.resume("continue")
    }
    pub fn step_over(&mut self) -> io::Result<u64> {
        self.resume("next")
    }
    pub fn step_in(&mut self) -> io::Result<u64> {
        self.resume("stepIn")
    }
    pub fn step_out(&mut self) -> io::Result<u64> {
        self.resume("stepOut")
    }
    fn resume(&mut self, command: &str) -> io::Result<u64> {
        self.require_stopped()?;
        let thread = self
            .selected_thread
            .ok_or_else(|| io::Error::other("No selected thread"))?;
        let id = self.request(
            command,
            json!({"threadId":thread,"singleThread":false}),
            PendingKind::Resume,
        )?;
        self.state = SessionState::Running;
        self.invalidate_inspection();
        self.events.push(DebugEvent::Changed);
        Ok(id)
    }
    pub fn pause(&mut self) -> io::Result<u64> {
        if self.state != SessionState::Running {
            return Err(io::Error::other("Program is not running"));
        }
        self.request(
            "pause",
            json!({"threadId":self.selected_thread.unwrap_or(0)}),
            PendingKind::Pause,
        )
    }
    /// The host normally rebuilds by disconnecting and starting a new session.
    /// This method is provided for adapter-level restart of the same executable.
    pub fn restart(&mut self) -> io::Result<u64> {
        if !self.supports_restart {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Adapter cannot restart this executable",
            ));
        }
        let id = self.request("restart", json!({}), PendingKind::Resume)?;
        self.state = SessionState::Running;
        self.invalidate_inspection();
        Ok(id)
    }
    pub fn disconnect(&mut self) -> io::Result<u64> {
        if self.state.is_finished() {
            return Ok(0);
        }
        let id = self.request(
            "disconnect",
            json!({"terminateDebuggee":true}),
            PendingKind::Disconnect,
        )?;
        self.state = SessionState::Stopping;
        self.invalidate_inspection();
        self.events.push(DebugEvent::Changed);
        Ok(id)
    }
    /// Give the adapter a short opportunity to terminate its owned debuggee
    /// before forcibly reaping the adapter. The host also stops its PTY session.
    pub fn shutdown(&mut self) {
        if !self.state.is_finished() {
            if self.state != SessionState::Stopping {
                let _ = self.disconnect();
            }
            let deadline = Instant::now() + Duration::from_millis(200);
            while !self.state.is_finished() && Instant::now() < deadline {
                let _ = self.poll();
                if !self.state.is_finished() {
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
            self.transport.shutdown();
            self.state = SessionState::Terminated;
            self.invalidate_inspection();
            self.pending.clear();
            self.terminal_requests.clear();
        }
    }
    pub fn select_thread(&mut self, id: i64) -> io::Result<u64> {
        self.require_stopped()?;
        self.selected_thread = Some(id);
        self.selection_generation += 1;
        self.frames.clear();
        self.scopes.clear();
        self.variables.clear();
        self.selected_frame = None;
        self.request(
            "stackTrace",
            json!({"threadId":id,"startFrame":0,"levels":100}),
            PendingKind::Frames,
        )
    }
    pub fn select_frame(&mut self, id: i64) -> io::Result<u64> {
        self.require_stopped()?;
        self.selected_frame = Some(id);
        self.selection_generation += 1;
        self.scopes.clear();
        self.variables.clear();
        self.request("scopes", json!({"frameId":id}), PendingKind::Scopes)
    }
    pub fn load_variables(&mut self, reference: i64) -> io::Result<u64> {
        self.require_stopped()?;
        if reference <= 0 {
            return Err(io::Error::other("Value has no children"));
        }
        if self.variables.contains_key(&reference) {
            return Ok(0);
        }
        if let Some((id, _)) = self.pending.iter().find(|(_, p)| {
            matches!(p.kind, PendingKind::Variables(r) if r == reference)
                && p.stop_generation == self.stop_generation
                && p.selection_generation == self.selection_generation
        }) {
            return Ok(*id);
        }
        self.request(
            "variables",
            json!({"variablesReference":reference,"start":0,"count":500}),
            PendingKind::Variables(reference),
        )
    }
    pub fn request_variables(&mut self, reference: i64) -> io::Result<u64> {
        self.load_variables(reference)
    }
    pub fn evaluate(
        &mut self,
        expression: impl Into<String>,
        context: EvaluateContext,
        frame_id: Option<i64>,
    ) -> io::Result<u64> {
        self.require_stopped()?;
        self.request("evaluate", json!({"expression":expression.into(),"context":context.as_str(),"frameId":frame_id.or(self.selected_frame)}), PendingKind::Evaluate)
    }
    pub fn respond_run_in_terminal(
        &mut self,
        request_seq: u64,
        result: Result<u32, String>,
    ) -> io::Result<()> {
        if !self.terminal_requests.remove(&request_seq) {
            return Err(io::Error::other("Terminal request is no longer pending"));
        }
        let seq = self.seq;
        self.seq += 1;
        let response = match result {
            Ok(pid) => {
                json!({"seq":seq,"type":"response","request_seq":request_seq,"command":"runInTerminal","success":true,"body":{"processId":pid}})
            }
            Err(error) => {
                json!({"seq":seq,"type":"response","request_seq":request_seq,"command":"runInTerminal","success":false,"message":error})
            }
        };
        self.transport.send(&response)
    }
    fn handle_message(&mut self, message: Value) -> io::Result<()> {
        match message["type"].as_str() {
            Some("response") => self.handle_response(message),
            Some("event") => self.handle_event(message),
            Some("request") => self.handle_request(message),
            _ => Err(io::Error::other("Unknown debugger protocol message type")),
        }
    }
    fn handle_request(&mut self, message: Value) -> io::Result<()> {
        let request_seq = message["seq"]
            .as_u64()
            .ok_or_else(|| io::Error::other("Debugger request has no sequence ID"))?;
        if message["command"].as_str() != Some("runInTerminal") {
            let seq = self.seq;
            self.seq += 1;
            return self.transport.send(&json!({"seq":seq,"type":"response","request_seq":request_seq,"command":message["command"],"success":false,"message":"Unsupported reverse request"}));
        }
        if self.terminal_requests.len() >= 4 {
            return Err(io::Error::other("Too many debugger terminal requests"));
        }
        let args: Vec<String> = serde_json::from_value(message["arguments"]["args"].clone())
            .map_err(io::Error::other)?;
        if args.is_empty() {
            return Err(io::Error::other("Debugger terminal command is empty"));
        }
        let arguments = &message["arguments"];
        if arguments["kind"]
            .as_str()
            .is_some_and(|kind| kind != "integrated")
        {
            return Err(io::Error::other(
                "Only integrated debug terminals are supported",
            ));
        }
        if arguments["argsCanBeInterpretedByShell"].as_bool() == Some(true) {
            return Err(io::Error::other(
                "Shell-interpreted debugger terminal arguments are unsupported",
            ));
        }
        let env = if arguments["env"].is_null() {
            BTreeMap::new()
        } else {
            serde_json::from_value(arguments["env"].clone()).map_err(io::Error::other)?
        };
        self.terminal_requests.insert(request_seq);
        self.events
            .push(DebugEvent::RunInTerminal(RunInTerminalRequest {
                request_seq,
                args,
                cwd: arguments["cwd"].as_str().unwrap_or(&self.config.cwd).into(),
                env,
                title: arguments["title"]
                    .as_str()
                    .unwrap_or("Debug program")
                    .into(),
            }));
        Ok(())
    }
    fn handle_response(&mut self, message: Value) -> io::Result<()> {
        let Some(id) = message["request_seq"].as_u64() else {
            return Err(io::Error::other("Debugger response has no request ID"));
        };
        let Some(pending) = self.pending.remove(&id) else {
            return Ok(());
        };
        if self.state == SessionState::Stopping && !matches!(pending.kind, PendingKind::Disconnect)
        {
            return Ok(());
        }
        let body = &message["body"];
        let success = message["success"].as_bool() == Some(true);
        let stale = pending.stop_generation != self.stop_generation
            || pending.selection_generation != self.selection_generation;
        if matches!(
            pending.kind,
            PendingKind::Threads
                | PendingKind::Frames
                | PendingKind::Scopes
                | PendingKind::Variables(_)
                | PendingKind::Evaluate
        ) && (stale || self.state != SessionState::Stopped)
        {
            if matches!(pending.kind, PendingKind::Evaluate) {
                self.events.push(DebugEvent::Evaluated {
                    request_id: id,
                    result: Err("Frame changed or program resumed".into()),
                });
            }
            return Ok(());
        }
        if !success {
            let error = message["message"]
                .as_str()
                .or_else(|| body["error"]["format"].as_str())
                .unwrap_or("Debugger request failed")
                .to_owned();
            match pending.kind {
                PendingKind::Initialize | PendingKind::Launch | PendingKind::Configure => {
                    self.fail(error)
                }
                PendingKind::Evaluate => self.events.push(DebugEvent::Evaluated {
                    request_id: id,
                    result: Err(error),
                }),
                PendingKind::Breakpoints(path) => {
                    self.startup_breakpoints = self.startup_breakpoints.saturating_sub(1);
                    if self.breakpoint_requests.get(&path) == Some(&id) {
                        self.events.push(DebugEvent::Breakpoints {
                            request_id: id,
                            path,
                            breakpoints: Vec::new(),
                        });
                        self.events.push(DebugEvent::Error(error));
                    }
                    self.finish_configuration()?;
                }
                PendingKind::Resume => {
                    self.state = SessionState::Stopped;
                    self.events.push(DebugEvent::Error(error));
                    self.refresh_threads()?;
                }
                PendingKind::Disconnect => {
                    self.state = SessionState::Terminated;
                    self.transport.shutdown();
                    self.events.push(DebugEvent::Changed);
                }
                _ => self.events.push(DebugEvent::Error(error)),
            }
            return Ok(());
        }
        match pending.kind {
            PendingKind::Initialize => {
                self.supports_restart = body["supportsRestartRequest"].as_bool().unwrap_or(false);
                self.state = SessionState::Launching;
                // runInTerminal remains accepted by LLVM 18+, while console was
                // introduced in LLVM 21. Sending both supports either version.
                self.request("launch", json!({
                    "program":self.config.program,"cwd":self.config.cwd,"args":self.config.args,"env":self.config.env,
                    "stopOnEntry":self.config.stop_on_entry,"sourceMap":self.config.source_map,
                    "runInTerminal":true,"console":"integratedTerminal","initCommands":self.config.init_commands,
                    "enableAutoVariableSummaries":false
                }), PendingKind::Launch)?;
            }
            PendingKind::Launch | PendingKind::Configure => {}
            PendingKind::Breakpoints(path) => {
                let breakpoints = serde_json::from_value(body["breakpoints"].clone())
                    .map_err(io::Error::other)?;
                if self.breakpoint_requests.get(&path) == Some(&id) {
                    self.events.push(DebugEvent::Breakpoints {
                        request_id: id,
                        path,
                        breakpoints,
                    });
                }
                self.startup_breakpoints = self.startup_breakpoints.saturating_sub(1);
                self.finish_configuration()?;
            }
            PendingKind::Threads => {
                self.threads =
                    serde_json::from_value(body["threads"].clone()).map_err(io::Error::other)?;
                self.threads.truncate(500);
                if !self
                    .threads
                    .iter()
                    .any(|t| Some(t.id) == self.selected_thread)
                {
                    self.selected_thread = self.threads.first().map(|t| t.id);
                }
                if let Some(id) = self.selected_thread {
                    self.select_thread(id)?;
                }
            }
            PendingKind::Frames => {
                self.frames = body["stackFrames"]
                    .as_array()
                    .ok_or_else(|| io::Error::other("Missing stack frames"))?
                    .iter()
                    .take(100)
                    .map(|f| StackFrame {
                        id: f["id"].as_i64().unwrap_or(0),
                        name: f["name"].as_str().unwrap_or("?").into(),
                        source: f["source"]["path"].as_str().map(str::to_owned),
                        line: f["line"].as_u64().unwrap_or(0) as usize,
                        column: f["column"].as_u64().unwrap_or(0) as usize,
                    })
                    .collect();
                if let Some(id) = self.frames.first().map(|f| f.id) {
                    self.select_frame(id)?;
                }
            }
            PendingKind::Scopes => {
                self.scopes =
                    serde_json::from_value(body["scopes"].clone()).map_err(io::Error::other)?;
                self.scopes.truncate(100);
                let references: Vec<_> = self
                    .scopes
                    .iter()
                    .filter(|s| !s.expensive && s.variables_reference > 0)
                    .map(|s| s.variables_reference)
                    .collect();
                for reference in references {
                    self.load_variables(reference)?;
                }
            }
            PendingKind::Variables(reference) => {
                let mut variables: Vec<Variable> =
                    serde_json::from_value(body["variables"].clone()).map_err(io::Error::other)?;
                variables.truncate(500);
                if self.variables.len() >= 256
                    && !self.variables.contains_key(&reference)
                    && let Some(key) = self.variables.keys().next().copied()
                {
                    self.variables.remove(&key);
                }
                self.variables.insert(reference, variables);
            }
            PendingKind::Evaluate => {
                let value = serde_json::from_value(body.clone()).map_err(|e| e.to_string());
                self.events.push(DebugEvent::Evaluated {
                    request_id: id,
                    result: value,
                });
            }
            PendingKind::Resume | PendingKind::Pause => {}
            PendingKind::Disconnect => {
                self.state = SessionState::Terminated;
                self.transport.shutdown();
                self.pending.clear();
            }
        }
        self.events.push(DebugEvent::Changed);
        Ok(())
    }
    fn refresh_threads(&mut self) -> io::Result<()> {
        self.request("threads", json!({}), PendingKind::Threads)?;
        Ok(())
    }
    fn handle_event(&mut self, message: Value) -> io::Result<()> {
        let body = &message["body"];
        if self.state == SessionState::Stopping
            && !matches!(
                message["event"].as_str(),
                Some("terminated" | "exited" | "output")
            )
        {
            return Ok(());
        }
        match message["event"].as_str().unwrap_or("") {
            "initialized" => {
                if !self.initialized {
                    self.initialized = true;
                    for (path, breakpoints) in self.config.breakpoints.clone() {
                        self.set_breakpoints(path, breakpoints)?;
                    }
                    self.finish_configuration()?;
                }
            }
            "stopped" => {
                self.invalidate_inspection();
                self.state = SessionState::Stopped;
                self.stop_reason = body["description"]
                    .as_str()
                    .or_else(|| body["text"].as_str())
                    .or_else(|| body["reason"].as_str())
                    .unwrap_or("Paused")
                    .into();
                self.selected_thread = body["threadId"].as_i64().or(self.selected_thread);
                self.refresh_threads()?;
            }
            "continued" => {
                self.state = SessionState::Running;
                self.invalidate_inspection();
            }
            "output" => self.events.push(DebugEvent::Output {
                category: body["category"].as_str().unwrap_or("console").into(),
                output: body["output"].as_str().unwrap_or("").into(),
            }),
            "exited" => self.exit_code = body["exitCode"].as_i64(),
            "terminated" => {
                self.state = SessionState::Terminated;
                self.invalidate_inspection();
                self.pending.clear();
                self.terminal_requests.clear();
                self.transport.shutdown();
            }
            "breakpoint" => {
                let breakpoint =
                    serde_json::from_value(body["breakpoint"].clone()).map_err(io::Error::other)?;
                self.events.push(DebugEvent::BreakpointChanged {
                    reason: body["reason"].as_str().unwrap_or("changed").into(),
                    breakpoint,
                });
            }
            "capabilities" => {
                if let Some(supported) = body["capabilities"]["supportsRestartRequest"].as_bool() {
                    self.supports_restart = supported;
                }
            }
            "invalidated" if self.state == SessionState::Stopped => {
                self.invalidate_inspection();
                self.refresh_threads()?;
            }
            "thread" if self.state == SessionState::Stopped => self.refresh_threads()?,
            _ => {}
        }
        self.events.push(DebugEvent::Changed);
        Ok(())
    }
}
impl Drop for DebugSession {
    fn drop(&mut self) {
        self.shutdown();
    }
}
