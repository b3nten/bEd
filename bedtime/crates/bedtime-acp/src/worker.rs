//! Background ACP client worker: spawns the agent subprocess via
//! `agent_client_protocol::AcpAgent`, runs the ACP v1 connection on a
//! dedicated thread, and bridges commands/events to the Bevy UI thread.
use crate::config::AcpClientConfig;
use agent_client_protocol::{
    AcpAgent, AcpAgentConfig, Agent, Client,
    schema::{MaybeUndefined, ProtocolVersion},
    v1::{
        self, ConnectionTo,
        schema::{
            AvailableCommand, CancelNotification, ClientCapabilities, CloseSessionRequest,
            ContentBlock, Cost, DeleteSessionRequest, FileSystemCapabilities, InitializeRequest,
            ListSessionsRequest, LoadSessionRequest, NewSessionRequest, PermissionOptionKind,
            PlanEntry, PromptRequest, ReadTextFileRequest, ReadTextFileResponse,
            RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
            ResumeSessionRequest, SelectedPermissionOutcome, SessionConfigId, SessionConfigOption,
            SessionConfigOptionValue, SessionInfo, SessionModeId, SessionModeState,
            SessionNotification, SessionUpdate, SetSessionConfigOptionRequest,
            SetSessionModeRequest, ToolCall, ToolCallUpdate, WriteTextFileRequest,
            WriteTextFileResponse,
        },
    },
};
use bedtime_ui::UiWaker;
use bedtime_workbench::{
    Cancellation, ManagedTask, ManagedWork,
    environment::{find_program, shell_environment},
};
use bevy::prelude::{Entity, Reflect};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::Duration,
};

/// Client-side tool permission approval policy.
#[derive(Reflect, Default, Clone, Copy, Debug, PartialEq, Eq)]
// Saved with chat panels by this path, wherever its module moves to.
#[type_path = "bedtime_acp"]
pub enum ApprovalMode {
    /// Prompt the user before allowing tool calls that request permission.
    #[default]
    Ask,
    /// Automatically approve tool permission requests using the agent's allow option.
    AutoApprove,
}

impl ApprovalMode {
    pub const ALL: [Self; 2] = [Self::Ask, Self::AutoApprove];

    pub fn label(self) -> &'static str {
        match self {
            Self::Ask => "Ask",
            Self::AutoApprove => "Auto-approve",
        }
    }
}

/// Capabilities advertised by the connected ACP agent during `initialize`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AgentCapabilitiesSummary {
    pub can_load_session: bool,
    pub can_list_sessions: bool,
    pub can_delete_session: bool,
    pub can_close_session: bool,
    pub can_resume_session: bool,
    pub supports_image: bool,
    pub supports_embedded_context: bool,
}

/// One permission choice offered by the agent.
#[derive(Clone, Debug)]
pub struct PermissionOptionInfo {
    pub option_id: String,
    pub name: String,
    pub is_allow: bool,
    pub is_always: bool,
}

/// Commands sent from the UI thread to the ACP worker.
pub enum WorkerCommand {
    NewSession {
        local_id: Entity,
        cwd: PathBuf,
    },
    ListSessions {
        cwd: Option<PathBuf>,
    },
    LoadSession {
        local_id: Entity,
        session_id: String,
        cwd: PathBuf,
        use_resume: bool,
    },
    DeleteSession {
        session_id: String,
    },
    CloseSession {
        session_id: String,
    },
    SetConfigOption {
        session_id: String,
        config_id: SessionConfigId,
        value: SessionConfigOptionValue,
    },
    SetMode {
        session_id: String,
        mode_id: SessionModeId,
    },
    SendPrompt {
        session_id: String,
        blocks: Vec<ContentBlock>,
    },
    CancelSession {
        session_id: String,
    },
    RespondPermission {
        request_id: u64,
        option_id: Option<String>,
    },
    /// What the file the agent asked to read holds, or why it couldn't be.
    FileRead {
        request_id: u64,
        result: Result<String, String>,
    },
    /// Whether the file the agent asked to write was.
    FileWritten {
        request_id: u64,
        result: Result<(), String>,
    },
    Shutdown,
}

/// Events sent from the ACP worker back to the UI thread.
pub enum WorkerEvent {
    Connected {
        agent_name: Option<String>,
        agent_version: Option<String>,
        capabilities: AgentCapabilitiesSummary,
    },
    Disconnected {
        error: Option<String>,
    },
    SessionsListed {
        sessions: Vec<SessionInfo>,
    },
    SessionCreated {
        local_id: Entity,
        session_id: String,
        config_options: Vec<SessionConfigOption>,
        modes: Option<SessionModeState>,
    },
    SessionCreateFailed {
        local_id: Entity,
        error: String,
    },
    SessionLoaded {
        local_id: Entity,
        session_id: String,
        config_options: Vec<SessionConfigOption>,
        modes: Option<SessionModeState>,
    },
    SessionLoadFailed {
        local_id: Entity,
        error: String,
    },
    SessionDeleteFailed {
        session_id: String,
        error: String,
    },
    ConfigOptionsUpdated {
        session_id: String,
        config_options: Vec<SessionConfigOption>,
    },
    ConfigOptionFailed {
        session_id: String,
        error: String,
    },
    ModeUpdated {
        session_id: String,
        mode_id: SessionModeId,
    },
    ModeFailed {
        session_id: String,
        error: String,
    },
    UserChunk {
        session_id: String,
        text: String,
    },
    AgentChunk {
        session_id: String,
        text: String,
    },
    ThoughtChunk {
        session_id: String,
        text: String,
    },
    ToolCallStarted {
        session_id: String,
        tool_call: ToolCall,
    },
    ToolCallUpdated {
        session_id: String,
        update: ToolCallUpdate,
    },
    PlanUpdated {
        session_id: String,
        entries: Vec<PlanEntry>,
    },
    AvailableCommandsUpdated {
        session_id: String,
        commands: Vec<AvailableCommand>,
    },
    UsageUpdated {
        session_id: String,
        used: u64,
        size: u64,
        cost: Option<Cost>,
    },
    SessionTitleUpdated {
        session_id: String,
        title: String,
    },
    PermissionRequested {
        request_id: u64,
        session_id: String,
        title: String,
        options: Vec<PermissionOptionInfo>,
    },
    PromptFinished {
        session_id: String,
        stop_reason: String,
    },
    /// The agent asks for a file's text (`fs/read_text_file`); answered with
    /// `WorkerCommand::FileRead`.
    ReadFile {
        request_id: u64,
        path: PathBuf,
        line: Option<u32>,
        limit: Option<u32>,
    },
    /// The agent asks to write a file (`fs/write_text_file`); answered with
    /// `WorkerCommand::FileWritten`.
    WriteFile {
        request_id: u64,
        path: PathBuf,
        content: String,
    },
    PromptFailed {
        session_id: String,
        error: String,
    },
}

/// The sessions whose tool calls are approved without asking.
type AutoApproved = Arc<Mutex<HashSet<String>>>;

/// Handle held by a client's connection to communicate with its
/// `ManagedWork`-owned ACP process.
pub struct AcpWorker {
    cmd_tx: async_channel::Sender<WorkerCommand>,
    event_rx: async_channel::Receiver<WorkerEvent>,
    auto_approved: AutoApproved,
    task: ManagedTask<()>,
}

impl AcpWorker {
    pub fn spawn(
        config: AcpClientConfig,
        waker: UiWaker,
        owner: Entity,
        work: &mut ManagedWork,
    ) -> Result<Self, String> {
        let (cmd_tx, cmd_rx) = async_channel::unbounded();
        let (event_tx, event_rx) = async_channel::unbounded();
        let auto_approved = AutoApproved::default();
        let worker_approval = Arc::clone(&auto_approved);
        let cancel_tx = cmd_tx.clone();
        let label = format!("ACP: {}", config.name);

        let task = work.spawn_task(Some(owner), label, move |cancel| {
            run_worker_managed(
                config,
                cmd_rx,
                cancel_tx,
                event_tx,
                worker_approval,
                waker,
                cancel,
            );
        })?;

        Ok(Self {
            cmd_tx,
            event_rx,
            auto_approved,
            task,
        })
    }

    /// How `session_id`'s tool calls are approved from now on.
    pub fn set_approval_mode(&self, session_id: &str, mode: ApprovalMode) {
        if let Ok(mut sessions) = self.auto_approved.lock() {
            match mode {
                ApprovalMode::AutoApprove => sessions.insert(session_id.to_owned()),
                ApprovalMode::Ask => sessions.remove(session_id),
            };
        }
    }

    pub fn send(&self, cmd: WorkerCommand) {
        let _ = self.cmd_tx.try_send(cmd);
    }

    pub fn drain_events(&self) -> Vec<WorkerEvent> {
        let mut events = Vec::new();
        while let Ok(event) = self.event_rx.try_recv() {
            events.push(event);
        }
        events
    }
}

impl Drop for AcpWorker {
    fn drop(&mut self) {
        self.task.cancel();
        let _ = self.cmd_tx.try_send(WorkerCommand::Shutdown);
        self.cmd_tx.close();
    }
}

fn emit(tx: &async_channel::Sender<WorkerEvent>, waker: &UiWaker, event: WorkerEvent) {
    let _ = tx.try_send(event);
    waker.wake();
}

/// Runs the ACP worker inside a `ManagedWork` task, bridging `Cancellation` to
/// `WorkerCommand::Shutdown` via a scoped watcher so panel despawn, workspace
/// switch, and `AppExit` shut down the subprocess and join cleanly.
fn run_worker_managed(
    config: AcpClientConfig,
    cmd_rx: async_channel::Receiver<WorkerCommand>,
    cancel_tx: async_channel::Sender<WorkerCommand>,
    event_tx: async_channel::Sender<WorkerEvent>,
    auto_approved: AutoApproved,
    waker: UiWaker,
    cancel: Cancellation,
) {
    let finished = AtomicBool::new(false);
    thread::scope(|scope| {
        scope.spawn(|| {
            while !finished.load(Ordering::Acquire) {
                if cancel.wait(Duration::from_millis(50)) {
                    let _ = cancel_tx.try_send(WorkerCommand::Shutdown);
                    cancel_tx.close();
                    break;
                }
            }
        });
        run_worker(config, cmd_rx, event_tx, auto_approved, waker);
        finished.store(true, Ordering::Release);
    });
}

fn run_worker(
    config: AcpClientConfig,
    cmd_rx: async_channel::Receiver<WorkerCommand>,
    event_tx: async_channel::Sender<WorkerEvent>,
    auto_approved: AutoApproved,
    waker: UiWaker,
) {
    // Resolve the command against the user's login shell environment so GUI-launched
    // builds find binaries in Homebrew, Cargo, nvm, ~/.local/bin, etc.
    let mut env = shell_environment().clone();
    for (k, v) in &config.env {
        env.insert(k.clone(), v.clone());
    }
    // One that isn't found is run as it's written, so the failure names it.
    let resolved_command =
        find_program(&config.command, &env).unwrap_or_else(|| config.command.clone().into());
    let agent_config = AcpAgentConfig::new(resolved_command)
        .args(config.args.clone())
        .envs(env);
    let agent = AcpAgent::new(agent_config);

    let pending_permissions: Arc<Mutex<HashMap<u64, v1::Responder<RequestPermissionResponse>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let next_permission_id = Arc::new(AtomicU64::new(1));

    let notif_tx = event_tx.clone();
    let notif_waker = waker.clone();

    let perm_tx = event_tx.clone();
    let perm_waker = waker.clone();
    let perm_map = Arc::clone(&pending_permissions);
    let perm_ids = Arc::clone(&next_permission_id);
    let perm_approval = Arc::clone(&auto_approved);

    let loop_tx = event_tx.clone();
    let loop_waker = waker.clone();
    let loop_perms = Arc::clone(&pending_permissions);

    // The agent's file reads and writes, answered by the UI thread (it has
    // the open documents).
    let pending_reads: Arc<Mutex<HashMap<u64, v1::Responder<ReadTextFileResponse>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let pending_writes: Arc<Mutex<HashMap<u64, v1::Responder<WriteTextFileResponse>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let read_tx = event_tx.clone();
    let read_waker = waker.clone();
    let read_map = Arc::clone(&pending_reads);
    let read_ids = Arc::clone(&next_permission_id);
    let write_tx = event_tx.clone();
    let write_waker = waker.clone();
    let write_map = Arc::clone(&pending_writes);
    let write_ids = Arc::clone(&next_permission_id);

    let result = futures_lite::future::block_on(async move {
        Client
            .v1()
            .name("bed")
            .on_receive_notification(
                async move |notification: SessionNotification, _cx| {
                    let session_id = notification.session_id.0.to_string();
                    match notification.update {
                        SessionUpdate::UserMessageChunk(chunk) => {
                            if let Some(text) = content_block_text(&chunk.content) {
                                emit(
                                    &notif_tx,
                                    &notif_waker,
                                    WorkerEvent::UserChunk { session_id, text },
                                );
                            }
                        }
                        SessionUpdate::AgentMessageChunk(chunk) => {
                            if let Some(text) = content_block_text(&chunk.content) {
                                emit(
                                    &notif_tx,
                                    &notif_waker,
                                    WorkerEvent::AgentChunk { session_id, text },
                                );
                            }
                        }
                        SessionUpdate::AgentThoughtChunk(chunk) => {
                            if let Some(text) = content_block_text(&chunk.content) {
                                emit(
                                    &notif_tx,
                                    &notif_waker,
                                    WorkerEvent::ThoughtChunk { session_id, text },
                                );
                            }
                        }
                        SessionUpdate::ToolCall(tool_call) => {
                            emit(
                                &notif_tx,
                                &notif_waker,
                                WorkerEvent::ToolCallStarted {
                                    session_id,
                                    tool_call,
                                },
                            );
                        }
                        SessionUpdate::ToolCallUpdate(update) => {
                            emit(
                                &notif_tx,
                                &notif_waker,
                                WorkerEvent::ToolCallUpdated { session_id, update },
                            );
                        }
                        SessionUpdate::Plan(plan) => {
                            emit(
                                &notif_tx,
                                &notif_waker,
                                WorkerEvent::PlanUpdated {
                                    session_id,
                                    entries: plan.entries,
                                },
                            );
                        }
                        SessionUpdate::AvailableCommandsUpdate(update) => {
                            emit(
                                &notif_tx,
                                &notif_waker,
                                WorkerEvent::AvailableCommandsUpdated {
                                    session_id,
                                    commands: update.available_commands,
                                },
                            );
                        }
                        SessionUpdate::ConfigOptionUpdate(update) => {
                            emit(
                                &notif_tx,
                                &notif_waker,
                                WorkerEvent::ConfigOptionsUpdated {
                                    session_id,
                                    config_options: update.config_options,
                                },
                            );
                        }
                        SessionUpdate::CurrentModeUpdate(update) => {
                            emit(
                                &notif_tx,
                                &notif_waker,
                                WorkerEvent::ModeUpdated {
                                    session_id,
                                    mode_id: update.current_mode_id,
                                },
                            );
                        }
                        SessionUpdate::UsageUpdate(usage) => {
                            emit(
                                &notif_tx,
                                &notif_waker,
                                WorkerEvent::UsageUpdated {
                                    session_id,
                                    used: usage.used,
                                    size: usage.size,
                                    cost: usage.cost,
                                },
                            );
                        }
                        SessionUpdate::SessionInfoUpdate(info) => {
                            if let MaybeUndefined::Value(title) = info.title {
                                emit(
                                    &notif_tx,
                                    &notif_waker,
                                    WorkerEvent::SessionTitleUpdated { session_id, title },
                                );
                            }
                        }
                        _ => {}
                    }
                    Ok(())
                },
                agent_client_protocol::on_receive_notification!(),
            )
            .on_receive_request(
                async move |request: RequestPermissionRequest, responder, _cx| {
                    // Auto-approve immediately if the session's thread is in AutoApprove mode.
                    let auto = perm_approval
                        .lock()
                        .is_ok_and(|sessions| sessions.contains(&*request.session_id.0));
                    if auto {
                        let chosen = request
                            .options
                            .iter()
                            .find(|opt| matches!(opt.kind, PermissionOptionKind::AllowOnce))
                            .or_else(|| {
                                request.options.iter().find(|opt| {
                                    matches!(opt.kind, PermissionOptionKind::AllowAlways)
                                })
                            });
                        if let Some(opt) = chosen {
                            let _ = responder.respond(RequestPermissionResponse::new(
                                RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
                                    opt.option_id.clone(),
                                )),
                            ));
                            return Ok(());
                        }
                    }

                    let request_id = perm_ids.fetch_add(1, Ordering::Relaxed);
                    let session_id = request.session_id.0.to_string();
                    let title = request
                        .tool_call
                        .fields
                        .title
                        .clone()
                        .unwrap_or_else(|| "Permission requested".to_owned());
                    let options = request
                        .options
                        .iter()
                        .map(|opt| PermissionOptionInfo {
                            option_id: opt.option_id.0.to_string(),
                            name: opt.name.clone(),
                            is_allow: matches!(
                                opt.kind,
                                PermissionOptionKind::AllowOnce | PermissionOptionKind::AllowAlways
                            ),
                            is_always: matches!(
                                opt.kind,
                                PermissionOptionKind::AllowAlways
                                    | PermissionOptionKind::RejectAlways
                            ),
                        })
                        .collect();

                    if let Ok(mut map) = perm_map.lock() {
                        map.insert(request_id, responder);
                    }
                    emit(
                        &perm_tx,
                        &perm_waker,
                        WorkerEvent::PermissionRequested {
                            request_id,
                            session_id,
                            title,
                            options,
                        },
                    );
                    Ok(())
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                async move |request: ReadTextFileRequest, responder, _cx| {
                    let request_id = read_ids.fetch_add(1, Ordering::Relaxed);
                    if let Ok(mut map) = read_map.lock() {
                        map.insert(request_id, responder);
                    }
                    emit(
                        &read_tx,
                        &read_waker,
                        WorkerEvent::ReadFile {
                            request_id,
                            path: request.path,
                            line: request.line,
                            limit: request.limit,
                        },
                    );
                    Ok(())
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                async move |request: WriteTextFileRequest, responder, _cx| {
                    let request_id = write_ids.fetch_add(1, Ordering::Relaxed);
                    if let Ok(mut map) = write_map.lock() {
                        map.insert(request_id, responder);
                    }
                    emit(
                        &write_tx,
                        &write_waker,
                        WorkerEvent::WriteFile {
                            request_id,
                            path: request.path,
                            content: request.content,
                        },
                    );
                    Ok(())
                },
                agent_client_protocol::on_receive_request!(),
            )
            .connect_with(agent, |connection: ConnectionTo<Agent>| async move {
                let init = connection
                    .send_request(
                        InitializeRequest::new(ProtocolVersion::V1).client_capabilities(
                            ClientCapabilities::new().fs(FileSystemCapabilities::new()
                                .read_text_file(true)
                                .write_text_file(true)),
                        ),
                    )
                    .block_task()
                    .await?;

                let (agent_name, agent_version) = match init.agent_info {
                    Some(info) => (Some(info.title.unwrap_or(info.name)), Some(info.version)),
                    None => (None, None),
                };
                let caps = init.agent_capabilities;
                let capabilities = AgentCapabilitiesSummary {
                    can_load_session: caps.load_session,
                    can_list_sessions: caps.session_capabilities.list.is_some(),
                    can_delete_session: caps.session_capabilities.delete.is_some(),
                    can_close_session: caps.session_capabilities.close.is_some(),
                    can_resume_session: caps.session_capabilities.resume.is_some(),
                    supports_image: caps.prompt_capabilities.image,
                    supports_embedded_context: caps.prompt_capabilities.embedded_context,
                };
                emit(
                    &loop_tx,
                    &loop_waker,
                    WorkerEvent::Connected {
                        agent_name,
                        agent_version,
                        capabilities,
                    },
                );

                while let Ok(cmd) = cmd_rx.recv().await {
                    match cmd {
                        WorkerCommand::NewSession { local_id, cwd } => {
                            let tx = loop_tx.clone();
                            let waker = loop_waker.clone();
                            connection
                                .send_request(NewSessionRequest::new(cwd))
                                .on_receiving_result(async move |res| {
                                    match res {
                                        Ok(response) => emit(
                                            &tx,
                                            &waker,
                                            WorkerEvent::SessionCreated {
                                                local_id,
                                                session_id: response.session_id.0.to_string(),
                                                config_options: response
                                                    .config_options
                                                    .unwrap_or_default(),
                                                modes: response.modes,
                                            },
                                        ),
                                        Err(error) => emit(
                                            &tx,
                                            &waker,
                                            WorkerEvent::SessionCreateFailed {
                                                local_id,
                                                error: error.to_string(),
                                            },
                                        ),
                                    }
                                    Ok(())
                                })?;
                        }
                        WorkerCommand::ListSessions { cwd } => {
                            let tx = loop_tx.clone();
                            let waker = loop_waker.clone();
                            let req = ListSessionsRequest::new().cwd(cwd);
                            connection
                                .send_request(req)
                                .on_receiving_result(async move |res| {
                                    if let Ok(response) = res {
                                        emit(
                                            &tx,
                                            &waker,
                                            WorkerEvent::SessionsListed {
                                                sessions: response.sessions,
                                            },
                                        );
                                    }
                                    Ok(())
                                })?;
                        }
                        WorkerCommand::LoadSession {
                            local_id,
                            session_id,
                            cwd,
                            use_resume,
                        } => {
                            let tx = loop_tx.clone();
                            let waker = loop_waker.clone();
                            let sid = session_id.clone();
                            if use_resume {
                                connection
                                    .send_request(ResumeSessionRequest::new(session_id, cwd))
                                    .on_receiving_result(async move |res| {
                                        match res {
                                            Ok(response) => emit(
                                                &tx,
                                                &waker,
                                                WorkerEvent::SessionLoaded {
                                                    local_id,
                                                    session_id: sid,
                                                    config_options: response
                                                        .config_options
                                                        .unwrap_or_default(),
                                                    modes: response.modes,
                                                },
                                            ),
                                            Err(error) => emit(
                                                &tx,
                                                &waker,
                                                WorkerEvent::SessionLoadFailed {
                                                    local_id,
                                                    error: error.to_string(),
                                                },
                                            ),
                                        }
                                        Ok(())
                                    })?;
                            } else {
                                connection
                                    .send_request(LoadSessionRequest::new(session_id, cwd))
                                    .on_receiving_result(async move |res| {
                                        match res {
                                            Ok(response) => emit(
                                                &tx,
                                                &waker,
                                                WorkerEvent::SessionLoaded {
                                                    local_id,
                                                    session_id: sid,
                                                    config_options: response
                                                        .config_options
                                                        .unwrap_or_default(),
                                                    modes: response.modes,
                                                },
                                            ),
                                            Err(error) => emit(
                                                &tx,
                                                &waker,
                                                WorkerEvent::SessionLoadFailed {
                                                    local_id,
                                                    error: error.to_string(),
                                                },
                                            ),
                                        }
                                        Ok(())
                                    })?;
                            }
                        }
                        WorkerCommand::DeleteSession { session_id } => {
                            let tx = loop_tx.clone();
                            let waker = loop_waker.clone();
                            let sid = session_id.clone();
                            connection
                                .send_request(DeleteSessionRequest::new(session_id))
                                .on_receiving_result(async move |res| {
                                    if let Err(error) = res {
                                        emit(
                                            &tx,
                                            &waker,
                                            WorkerEvent::SessionDeleteFailed {
                                                session_id: sid,
                                                error: error.to_string(),
                                            },
                                        );
                                    }
                                    Ok(())
                                })?;
                        }
                        WorkerCommand::CloseSession { session_id } => {
                            let _ = connection
                                .send_request(CloseSessionRequest::new(session_id))
                                .on_receiving_result(async move |_| Ok(()));
                        }
                        WorkerCommand::SetConfigOption {
                            session_id,
                            config_id,
                            value,
                        } => {
                            let tx = loop_tx.clone();
                            let waker = loop_waker.clone();
                            let sid = session_id.clone();
                            connection
                                .send_request(SetSessionConfigOptionRequest::new(
                                    session_id, config_id, value,
                                ))
                                .on_receiving_result(async move |res| {
                                    match res {
                                        Ok(response) => emit(
                                            &tx,
                                            &waker,
                                            WorkerEvent::ConfigOptionsUpdated {
                                                session_id: sid,
                                                config_options: response.config_options,
                                            },
                                        ),
                                        Err(error) => emit(
                                            &tx,
                                            &waker,
                                            WorkerEvent::ConfigOptionFailed {
                                                session_id: sid,
                                                error: error.to_string(),
                                            },
                                        ),
                                    }
                                    Ok(())
                                })?;
                        }
                        WorkerCommand::SetMode {
                            session_id,
                            mode_id,
                        } => {
                            let tx = loop_tx.clone();
                            let waker = loop_waker.clone();
                            let sid = session_id.clone();
                            let mid = mode_id.clone();
                            connection
                                .send_request(SetSessionModeRequest::new(session_id, mode_id))
                                .on_receiving_result(async move |res| {
                                    match res {
                                        Ok(_) => emit(
                                            &tx,
                                            &waker,
                                            WorkerEvent::ModeUpdated {
                                                session_id: sid,
                                                mode_id: mid,
                                            },
                                        ),
                                        Err(error) => emit(
                                            &tx,
                                            &waker,
                                            WorkerEvent::ModeFailed {
                                                session_id: sid,
                                                error: error.to_string(),
                                            },
                                        ),
                                    }
                                    Ok(())
                                })?;
                        }
                        WorkerCommand::SendPrompt { session_id, blocks } => {
                            let tx = loop_tx.clone();
                            let waker = loop_waker.clone();
                            let sid = session_id.clone();
                            connection
                                .send_request(PromptRequest::new(session_id, blocks))
                                .on_receiving_result(async move |res| {
                                    match res {
                                        Ok(response) => emit(
                                            &tx,
                                            &waker,
                                            WorkerEvent::PromptFinished {
                                                session_id: sid,
                                                stop_reason: format!("{:?}", response.stop_reason),
                                            },
                                        ),
                                        Err(error) => emit(
                                            &tx,
                                            &waker,
                                            WorkerEvent::PromptFailed {
                                                session_id: sid,
                                                error: error.to_string(),
                                            },
                                        ),
                                    }
                                    Ok(())
                                })?;
                        }
                        WorkerCommand::CancelSession { session_id } => {
                            let _ =
                                connection.send_notification(CancelNotification::new(session_id));
                        }
                        WorkerCommand::RespondPermission {
                            request_id,
                            option_id,
                        } => {
                            let responder = loop_perms
                                .lock()
                                .ok()
                                .and_then(|mut map| map.remove(&request_id));
                            if let Some(responder) = responder {
                                let outcome = match option_id {
                                    Some(id) => RequestPermissionOutcome::Selected(
                                        SelectedPermissionOutcome::new(id),
                                    ),
                                    None => RequestPermissionOutcome::Cancelled,
                                };
                                let _ = responder.respond(RequestPermissionResponse::new(outcome));
                            }
                        }
                        WorkerCommand::FileRead { request_id, result } => {
                            let responder = pending_reads
                                .lock()
                                .ok()
                                .and_then(|mut map| map.remove(&request_id));
                            if let Some(responder) = responder {
                                let _ = match result {
                                    Ok(content) => {
                                        responder.respond(ReadTextFileResponse::new(content))
                                    }
                                    Err(error) => responder.respond_with_internal_error(error),
                                };
                            }
                        }
                        WorkerCommand::FileWritten { request_id, result } => {
                            let responder = pending_writes
                                .lock()
                                .ok()
                                .and_then(|mut map| map.remove(&request_id));
                            if let Some(responder) = responder {
                                let _ = match result {
                                    Ok(()) => responder.respond(WriteTextFileResponse::new()),
                                    Err(error) => responder.respond_with_internal_error(error),
                                };
                            }
                        }
                        WorkerCommand::Shutdown => break,
                    }
                }

                Ok(())
            })
            .await
    });

    let error = result.err().map(|e| e.to_string());
    emit(&event_tx, &waker, WorkerEvent::Disconnected { error });
}

fn content_block_text(block: &ContentBlock) -> Option<String> {
    match block {
        ContentBlock::Text(text) => Some(text.text.clone()),
        ContentBlock::Image(img) => Some(format!("[Image: {}]\n", img.mime_type)),
        ContentBlock::ResourceLink(link) => Some(format!("[File: {}]\n", link.name)),
        ContentBlock::Resource(res) => match &res.resource {
            agent_client_protocol::v1::schema::EmbeddedResourceResource::TextResourceContents(
                t,
            ) => Some(format!("[File: {}]\n", t.uri)),
            agent_client_protocol::v1::schema::EmbeddedResourceResource::BlobResourceContents(
                b,
            ) => Some(format!("[File: {}]\n", b.uri)),
            _ => None,
        },
        _ => None,
    }
}
