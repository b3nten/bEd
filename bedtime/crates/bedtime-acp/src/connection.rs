//! A client's connection: one entity for each configured ACP client, with
//! its agent process (started when something wants it) and its threads.
//! Chat panels and the Agents panel share it.
use crate::{
    agent_files,
    config::ConfiguredClients,
    thread::{
        AcpThread, PendingPermission, SessionState, TranscriptItem, UsageInfo,
        dispatch_next_queued, short_session_id,
    },
    worker::{AcpWorker, AgentCapabilitiesSummary, ApprovalMode, WorkerCommand, WorkerEvent},
};
use bedtime_ui::UiWaker;
use bedtime_workbench::{InWorkspace, ManagedWork};
use bevy::prelude::*;
use std::{collections::HashSet, path::Path};

#[derive(Default, Clone, Debug, PartialEq, Eq)]
pub enum ConnectionStatus {
    /// Not started: nothing has wanted it yet.
    #[default]
    Idle,
    Connecting,
    Connected,
    Error(String),
}

/// A configured ACP client, by name, and its agent process while it runs.
#[derive(Component)]
#[require(InWorkspace)]
pub struct AcpConnection {
    /// Matches `AcpClientConfig::name`.
    pub(crate) client_name: String,
    pub(crate) worker: Option<AcpWorker>,
    pub(crate) status: ConnectionStatus,
    pub(crate) agent_info: Option<String>,
    pub(crate) capabilities: AgentCapabilitiesSummary,
    /// Something needs the agent (a chat, or its group open in an Agents
    /// panel): it's started.
    pub(crate) wanted: bool,
    /// Its threads, in the order they're listed.
    pub(crate) threads: Vec<Entity>,
}

impl AcpConnection {
    fn new(client_name: String) -> Self {
        Self {
            client_name,
            worker: None,
            status: ConnectionStatus::Idle,
            agent_info: None,
            capabilities: AgentCapabilitiesSummary::default(),
            wanted: false,
            threads: Vec::new(),
        }
    }

    pub fn client_name(&self) -> &str {
        &self.client_name
    }

    pub fn status(&self) -> &ConnectionStatus {
        &self.status
    }

    #[cfg(test)]
    pub(crate) fn connecting_for_test(client_name: &str) -> Self {
        Self {
            status: ConnectionStatus::Connecting,
            ..Self::new(client_name.into())
        }
    }
}

/// Send `command` to `connection`'s agent, if it's running.
pub(crate) fn send(world: &World, connection: Entity, command: WorkerCommand) {
    if let Some(worker) = world
        .get::<AcpConnection>(connection)
        .and_then(|connection| connection.worker.as_ref())
    {
        worker.send(command);
    }
}

/// One connection for each configured client: made for new ones, and gone
/// (with their threads) for those no longer configured.
pub(crate) fn sync_connections(world: &mut World) {
    let names: Vec<String> = world
        .get_resource::<ConfiguredClients>()
        .map(|clients| {
            clients
                .0
                .iter()
                .map(|client| client.name.clone())
                .filter(|name| !name.trim().is_empty())
                .collect()
        })
        .unwrap_or_default();
    let existing: Vec<(Entity, String)> = world
        .query::<(Entity, &AcpConnection)>()
        .iter(world)
        .map(|(entity, connection)| (entity, connection.client_name.clone()))
        .collect();
    for (entity, name) in &existing {
        if !names.contains(name) {
            let threads = world
                .get::<AcpConnection>(*entity)
                .map(|connection| connection.threads.clone())
                .unwrap_or_default();
            for thread in threads {
                if let Ok(thread) = world.get_entity_mut(thread) {
                    thread.despawn();
                }
            }
            world.entity_mut(*entity).despawn();
        }
    }
    for name in names {
        if !existing.iter().any(|(_, existing)| *existing == name) {
            world.spawn(AcpConnection::new(name));
        }
    }
}

/// Start the agents that are wanted and not running. One that failed stays
/// stopped until `reconnect`.
pub(crate) fn start_wanted(world: &mut World, waker: &UiWaker) {
    if world
        .get_resource::<ManagedWork>()
        .is_none_or(|work| work.is_closing())
    {
        return;
    }
    let wanted: Vec<Entity> = world
        .query::<(Entity, &AcpConnection)>()
        .iter(world)
        .filter(|(_, connection)| {
            connection.wanted
                && connection.worker.is_none()
                && connection.status == ConnectionStatus::Idle
        })
        .map(|(entity, _)| entity)
        .collect();
    let has_folder = world.contains_resource::<bedtime_workbench::Root>();
    for entity in wanted {
        if has_folder {
            start(world, entity, waker);
        } else if let Some(mut connection) = world.get_mut::<AcpConnection>(entity) {
            // An agent works in a folder, and acts there without asking when
            // it's set to: not one it was never given.
            connection.status =
                ConnectionStatus::Error("Open a folder to start this agent.".into());
        }
    }
}

fn start(world: &mut World, entity: Entity, waker: &UiWaker) {
    let Some(name) = world
        .get::<AcpConnection>(entity)
        .map(|connection| connection.client_name.clone())
    else {
        return;
    };
    let config = world
        .get_resource::<ConfiguredClients>()
        .and_then(|clients| clients.find(&name).cloned());
    let result = match (config, world.get_resource_mut::<ManagedWork>()) {
        (None, _) => Err(format!(
            "ACP client \"{name}\" is not configured in Settings."
        )),
        (_, None) => Err("ManagedWork resource is unavailable.".to_owned()),
        (Some(config), Some(mut work)) => {
            AcpWorker::spawn(config, waker.clone(), entity, &mut work)
                .map_err(|err| format!("Couldn't start agent: {err}"))
        }
    };
    let Some(mut connection) = world.get_mut::<AcpConnection>(entity) else {
        return;
    };
    match result {
        Ok(worker) => {
            connection.status = ConnectionStatus::Connecting;
            connection.worker = Some(worker);
        }
        Err(err) => connection.status = ConnectionStatus::Error(err),
    }
}

/// Stop `entity`'s agent, if it's running, and start it again.
pub fn reconnect(world: &mut World, entity: Entity) {
    stopped(world, entity, ConnectionStatus::Idle);
    if let Some(mut connection) = world.get_mut::<AcpConnection>(entity) {
        connection.wanted = true;
    }
}

/// Ask `entity`'s agent for its sessions again.
pub fn refresh_sessions(world: &World, entity: Entity, cwd: &Path) {
    if world
        .get::<AcpConnection>(entity)
        .is_some_and(|connection| connection.capabilities.can_list_sessions)
    {
        send(
            world,
            entity,
            WorkerCommand::ListSessions {
                cwd: Some(cwd.to_path_buf()),
            },
        );
    }
}

/// The agent is gone: nothing of its threads is running.
fn stopped(world: &mut World, entity: Entity, status: ConnectionStatus) {
    let Some(mut connection) = world.get_mut::<AcpConnection>(entity) else {
        return;
    };
    connection.worker = None;
    connection.status = status;
    connection.agent_info = None;
    connection.capabilities = AgentCapabilitiesSummary::default();
    for thread in connection.threads.clone() {
        if let Some(mut thread) = world.get_mut::<AcpThread>(thread) {
            thread.disconnected();
        }
    }
}

/// What happened in a thread that its user may not be looking at.
pub(crate) enum Notice {
    PermissionRequested { thread: Entity, title: String },
    TurnFinished { thread: Entity },
    Failed(String),
}

/// `connection`'s thread with this session.
fn thread_of(world: &World, connection: Entity, session_id: &str) -> Option<Entity> {
    world
        .get::<AcpConnection>(connection)?
        .threads
        .iter()
        .copied()
        .find(|&thread| {
            world
                .get::<AcpThread>(thread)
                .is_some_and(|thread| thread.session_id.as_deref() == Some(session_id))
        })
}

/// Take what `entity`'s agent has sent and make its threads match.
pub(crate) fn drain(world: &mut World, entity: Entity, cwd: &Path) -> Vec<Notice> {
    let mut notices = Vec::new();
    let Some(events) = world
        .get::<AcpConnection>(entity)
        .and_then(|connection| connection.worker.as_ref())
        .map(AcpWorker::drain_events)
    else {
        return notices;
    };
    for event in events {
        apply(world, entity, cwd, event, &mut notices);
    }
    notices
}

fn apply(
    world: &mut World,
    entity: Entity,
    cwd: &Path,
    event: WorkerEvent,
    notices: &mut Vec<Notice>,
) {
    // The thread an event is about, to change.
    macro_rules! thread {
        ($session_id:expr) => {
            thread_of(world, entity, &$session_id)
                .and_then(|thread| world.get_mut::<AcpThread>(thread))
        };
    }
    match event {
        WorkerEvent::Connected {
            agent_name,
            agent_version,
            capabilities,
        } => {
            if let Some(mut connection) = world.get_mut::<AcpConnection>(entity) {
                connection.status = ConnectionStatus::Connected;
                connection.capabilities = capabilities;
                connection.agent_info = match (agent_name, agent_version) {
                    (Some(name), Some(ver)) => Some(format!("{name} {ver}")),
                    (Some(name), None) => Some(name),
                    _ => None,
                };
            }
            refresh_sessions(world, entity, cwd);
        }
        WorkerEvent::Disconnected { error } => {
            let error = error.unwrap_or_else(|| "Agent process exited.".into());
            stopped(world, entity, ConnectionStatus::Error(error));
        }
        WorkerEvent::SessionsListed { sessions } => {
            let listed: HashSet<String> = sessions
                .iter()
                .map(|info| info.session_id.0.to_string())
                .collect();
            for info in sessions {
                let session_id = info.session_id.0.to_string();
                let title = info.title.filter(|title| !title.trim().is_empty());
                if let Some(mut thread) = thread!(session_id) {
                    thread.listed = true;
                    if let Some(title) = title {
                        thread.title = title;
                    }
                } else {
                    let title = title.unwrap_or_else(|| short_session_id(&session_id));
                    let mut thread = AcpThread::unloaded(entity, session_id, title);
                    thread.listed = true;
                    let thread = world.spawn(thread).id();
                    if let Some(mut connection) = world.get_mut::<AcpConnection>(entity) {
                        connection.threads.push(thread);
                    }
                }
            }
            // Sessions the agent no longer has, that nothing shows.
            let threads = world
                .get::<AcpConnection>(entity)
                .map(|connection| connection.threads.clone())
                .unwrap_or_default();
            for thread in threads {
                let gone = world.get::<AcpThread>(thread).is_some_and(|t| {
                    t.listed
                        && t.panel.is_none()
                        && t.session == SessionState::Listed
                        && t.session_id.as_ref().is_some_and(|id| !listed.contains(id))
                });
                if gone {
                    world.entity_mut(thread).despawn();
                    if let Some(mut connection) = world.get_mut::<AcpConnection>(entity) {
                        connection.threads.retain(|&other| other != thread);
                    }
                }
            }
        }
        WorkerEvent::SessionCreated {
            local_id,
            session_id,
            config_options,
            modes,
        }
        | WorkerEvent::SessionLoaded {
            local_id,
            session_id,
            config_options,
            modes,
        } => {
            let Some(mut thread) = world.get_mut::<AcpThread>(local_id) else {
                return;
            };
            thread.session_id = Some(session_id.clone());
            thread.session = SessionState::Ready;
            thread.config_options = config_options;
            thread.modes = modes;
            thread.error = None;
            thread.scroll_to_bottom = true;
            let approval_mode = thread.approval_mode;
            if let Some(worker) = world
                .get::<AcpConnection>(entity)
                .and_then(|connection| connection.worker.as_ref())
            {
                worker.set_approval_mode(&session_id, approval_mode);
            }
            dispatch_next_queued(world, local_id);
        }
        WorkerEvent::SessionCreateFailed { local_id, error } => {
            if let Some(mut thread) = world.get_mut::<AcpThread>(local_id) {
                thread.session = SessionState::Failed;
                thread.streaming = false;
                thread.queued_prompts.clear();
                thread.error = Some(format!("Failed to create session: {error}"));
            }
        }
        WorkerEvent::SessionLoadFailed { local_id, error } => {
            if let Some(mut thread) = world.get_mut::<AcpThread>(local_id) {
                thread.session = SessionState::Failed;
                thread.error = Some(format!("Failed to load session: {error}"));
            }
        }
        WorkerEvent::SessionDeleteFailed { error, .. } => {
            notices.push(Notice::Failed(format!("Failed to delete session: {error}")));
        }
        WorkerEvent::ConfigOptionsUpdated {
            session_id,
            config_options,
        } => {
            if let Some(mut thread) = thread!(session_id) {
                thread.config_options = config_options;
            }
        }
        WorkerEvent::ConfigOptionFailed { session_id, error } => {
            if let Some(mut thread) = thread!(session_id) {
                thread.error = Some(format!("Failed to update option: {error}"));
            }
        }
        WorkerEvent::ModeUpdated {
            session_id,
            mode_id,
        } => {
            if let Some(mut thread) = thread!(session_id)
                && let Some(modes) = &mut thread.modes
            {
                modes.current_mode_id = mode_id;
            }
        }
        WorkerEvent::ModeFailed { session_id, error } => {
            if let Some(mut thread) = thread!(session_id) {
                thread.error = Some(format!("Failed to change mode: {error}"));
            }
        }
        WorkerEvent::UserChunk { session_id, text } => {
            if let Some(mut thread) = thread!(session_id) {
                match thread.items.last_mut() {
                    Some(TranscriptItem::User { text: existing, .. }) => {
                        existing.push_str(&text);
                    }
                    _ => thread.items.push(TranscriptItem::User {
                        text,
                        attachments: Vec::new(),
                    }),
                }
            }
        }
        WorkerEvent::AgentChunk { session_id, text } => {
            if let Some(mut thread) = thread!(session_id) {
                match thread.items.last_mut() {
                    Some(TranscriptItem::Agent {
                        text: existing,
                        view,
                    }) => {
                        existing.push_str(&text);
                        *view = None;
                    }
                    _ => {
                        thread.items.push(TranscriptItem::Agent { text, view: None });
                    }
                }
                thread.scroll_to_bottom = true;
            }
        }
        WorkerEvent::ThoughtChunk { session_id, text } => {
            if let Some(mut thread) = thread!(session_id) {
                match thread.items.last_mut() {
                    Some(TranscriptItem::Thought { text: existing, .. }) => {
                        existing.push_str(&text);
                    }
                    _ => {
                        thread.items.push(TranscriptItem::Thought {
                            text,
                            expanded: false,
                        });
                    }
                }
                thread.scroll_to_bottom = true;
            }
        }
        WorkerEvent::ToolCallStarted {
            session_id,
            tool_call,
        } => {
            if let Some(mut thread) = thread!(session_id) {
                if let Some(TranscriptItem::Tool { call: existing, .. }) =
                    thread.items.iter_mut().find(|item| {
                        matches!(item, TranscriptItem::Tool { call, .. } if call.tool_call_id == tool_call.tool_call_id)
                    })
                {
                    *existing = tool_call;
                } else {
                    thread.items.push(TranscriptItem::Tool {
                        call: tool_call,
                        expanded: false,
                    });
                }
                thread.scroll_to_bottom = true;
            }
        }
        WorkerEvent::ToolCallUpdated { session_id, update } => {
            if let Some(mut thread) = thread!(session_id)
                && let Some(TranscriptItem::Tool { call: existing, .. }) =
                    thread.items.iter_mut().find(|item| {
                        matches!(item, TranscriptItem::Tool { call, .. } if call.tool_call_id == update.tool_call_id)
                    })
            {
                existing.update(update.fields);
            }
        }
        WorkerEvent::PlanUpdated {
            session_id,
            entries,
        } => {
            if let Some(mut thread) = thread!(session_id) {
                thread.plan = entries;
            }
        }
        WorkerEvent::AvailableCommandsUpdated {
            session_id,
            commands,
        } => {
            if let Some(mut thread) = thread!(session_id) {
                thread.available_commands = commands;
            }
        }
        WorkerEvent::UsageUpdated {
            session_id,
            used,
            size,
            cost,
        } => {
            if let Some(mut thread) = thread!(session_id) {
                thread.usage = Some(UsageInfo { used, size, cost });
            }
        }
        WorkerEvent::SessionTitleUpdated { session_id, title } => {
            if let Some(mut thread) = thread!(session_id)
                && !title.trim().is_empty()
            {
                thread.title = title;
            }
        }
        WorkerEvent::PermissionRequested {
            request_id,
            session_id,
            title,
            options,
        } => {
            let Some(target) = thread_of(world, entity, &session_id) else {
                return;
            };
            let Some(mut thread) = world.get_mut::<AcpThread>(target) else {
                return;
            };
            let permission = PendingPermission {
                request_id,
                title,
                options,
            };
            // Asked before the worker knew the thread approves by itself.
            let allowed = (thread.approval_mode == ApprovalMode::AutoApprove)
                .then(|| permission.allow_option())
                .flatten();
            if let Some(option_id) = allowed {
                send(
                    world,
                    entity,
                    WorkerCommand::RespondPermission {
                        request_id,
                        option_id: Some(option_id),
                    },
                );
            } else {
                notices.push(Notice::PermissionRequested {
                    thread: target,
                    title: permission.title.clone(),
                });
                thread.pending_permissions.push(permission);
                thread.scroll_to_bottom = true;
            }
        }
        WorkerEvent::PromptFinished { session_id, .. } => {
            let Some(target) = thread_of(world, entity, &session_id) else {
                return;
            };
            let Some(mut thread) = world.get_mut::<AcpThread>(target) else {
                return;
            };
            thread.streaming = false;
            thread.pending_permissions.clear();
            if thread.queued_prompts.is_empty() {
                notices.push(Notice::TurnFinished { thread: target });
            } else {
                dispatch_next_queued(world, target);
            }
        }
        WorkerEvent::ReadFile {
            request_id,
            path,
            line,
            limit,
        } => {
            let result = agent_files::read(world, &cwd.join(path), line, limit);
            send(
                world,
                entity,
                WorkerCommand::FileRead { request_id, result },
            );
        }
        WorkerEvent::WriteFile {
            request_id,
            path,
            content,
        } => {
            let result = agent_files::write(world, &cwd.join(path), &content);
            send(
                world,
                entity,
                WorkerCommand::FileWritten { request_id, result },
            );
        }
        WorkerEvent::PromptFailed { session_id, error } => {
            if let Some(mut thread) = thread!(session_id) {
                thread.streaming = false;
                thread.pending_permissions.clear();
                thread.error = Some(error);
            }
        }
    }
}
