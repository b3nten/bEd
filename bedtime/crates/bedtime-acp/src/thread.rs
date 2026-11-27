//! A thread: one chat with an agent (an ACP session and its transcript), as
//! an entity of its own. A thread belongs to its client's
//! [`AcpConnection`], is listed in the Agents panel, and is shown by at most
//! one [`AcpPanel`]; it keeps running with no panel open.
use crate::{
    attachments::{OpenTexts, PromptAttachment, build_prompt_blocks, open_texts},
    connection::{AcpConnection, ConnectionStatus, send},
    panel::{AcpPanel, AcpRuntime},
    worker::{AgentCapabilitiesSummary, ApprovalMode, PermissionOptionInfo, WorkerCommand},
};
use agent_client_protocol::v1::schema::{
    AvailableCommand, ContentBlock, Cost, PlanEntry, SessionConfigKind, SessionConfigOption,
    SessionConfigOptionCategory, SessionConfigSelectOptions, SessionModeState, ToolCall,
};
use bedtime_ui::markdown::MarkdownView;
use bedtime_workbench::{
    ActionResult, BedtimeWorldExt as _, InWorkspace, LayoutEdit, Notification, Placement,
};
use bevy::prelude::*;
use std::{collections::VecDeque, path::Path};

/// Where a thread's ACP session is.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SessionState {
    /// No session yet: one is made once the thread is shown and its agent
    /// is connected.
    #[default]
    New,
    /// Asked for.
    Creating,
    /// The agent has it, but it isn't loaded here: loaded once it's shown.
    Listed,
    Loading,
    /// Prompts can be sent.
    Ready,
    /// It couldn't be made or loaded; reconnecting tries again.
    Failed,
}

pub(crate) struct UsageInfo {
    pub used: u64,
    pub size: u64,
    pub cost: Option<Cost>,
}

pub(crate) struct QueuedPrompt {
    pub text: String,
    pub attachments: Vec<PromptAttachment>,
}

pub(crate) struct PendingPermission {
    pub request_id: u64,
    pub title: String,
    pub options: Vec<PermissionOptionInfo>,
}

impl PendingPermission {
    /// The option that allows it (once, if there is one).
    pub fn allow_option(&self) -> Option<String> {
        self.options
            .iter()
            .find(|o| o.is_allow && !o.is_always)
            .or_else(|| self.options.iter().find(|o| o.is_allow))
            .map(|o| o.option_id.clone())
    }
}

pub(crate) enum TranscriptItem {
    User {
        text: String,
        attachments: Vec<(String, bool)>,
    },
    Agent {
        text: String,
        view: Option<MarkdownView>,
    },
    Thought {
        text: String,
        expanded: bool,
    },
    Tool {
        call: ToolCall,
        expanded: bool,
    },
}

/// One chat with an agent. Not persisted: a chat panel saves its thread's
/// session id, and the agent keeps the rest.
#[derive(Component)]
#[require(InWorkspace)]
pub struct AcpThread {
    /// Its client's [`AcpConnection`].
    pub(crate) connection: Entity,
    pub(crate) session_id: Option<String>,
    pub(crate) session: SessionState,
    /// The agent listed it: gone from the list when it no longer does.
    pub(crate) listed: bool,
    /// Started here, for a chat panel; until something is said in it, it
    /// goes with that panel.
    pub(crate) fresh: bool,
    /// The chat panel showing it.
    pub(crate) panel: Option<Entity>,
    /// Client-side tool approval policy, for this thread only.
    pub(crate) approval_mode: ApprovalMode,
    pub(crate) title: String,
    pub(crate) items: Vec<TranscriptItem>,
    pub(crate) plan: Vec<PlanEntry>,
    pub(crate) plan_expanded: bool,
    pub(crate) available_commands: Vec<AvailableCommand>,
    pub(crate) config_options: Vec<SessionConfigOption>,
    pub(crate) modes: Option<SessionModeState>,
    pub(crate) usage: Option<UsageInfo>,
    pub(crate) streaming: bool,
    pub(crate) queued_prompts: VecDeque<QueuedPrompt>,
    pub(crate) pending_permissions: Vec<PendingPermission>,
    pub(crate) error: Option<String>,
    /// Something was added: its panel scrolls to the end when next drawn.
    pub(crate) scroll_to_bottom: bool,
}

/// What stands in for a thread lent to its panel's draw.
impl Default for AcpThread {
    fn default() -> Self {
        Self::new(Entity::PLACEHOLDER, String::new())
    }
}

impl AcpThread {
    pub(crate) fn new(connection: Entity, title: String) -> Self {
        Self {
            connection,
            session_id: None,
            session: SessionState::New,
            listed: false,
            fresh: true,
            panel: None,
            approval_mode: ApprovalMode::Ask,
            title,
            items: Vec::new(),
            plan: Vec::new(),
            plan_expanded: false,
            available_commands: Vec::new(),
            config_options: Vec::new(),
            modes: None,
            usage: None,
            streaming: false,
            queued_prompts: VecDeque::new(),
            pending_permissions: Vec::new(),
            error: None,
            scroll_to_bottom: false,
        }
    }

    /// A session the agent has that isn't loaded here.
    pub(crate) fn unloaded(connection: Entity, session_id: String, title: String) -> Self {
        Self {
            session_id: Some(session_id),
            session: SessionState::Listed,
            fresh: false,
            ..Self::new(connection, title)
        }
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    /// The agent is answering, or its session is being loaded.
    pub fn is_busy(&self) -> bool {
        self.streaming || self.session == SessionState::Loading
    }

    /// Started here and nothing was ever said in it: it goes with its panel.
    fn is_untouched(&self) -> bool {
        self.fresh && self.items.is_empty() && !self.streaming && self.queued_prompts.is_empty()
    }

    /// The model it's set to, by name, if the agent says.
    pub(crate) fn model_name(&self) -> Option<String> {
        let opt = self
            .config_options
            .iter()
            .find(|opt| matches!(opt.category, Some(SessionConfigOptionCategory::Model)))?;
        let SessionConfigKind::Select(sel) = &opt.kind else {
            return None;
        };
        let current = &sel.current_value;
        let found = match &sel.options {
            SessionConfigSelectOptions::Ungrouped(items) => items
                .iter()
                .find(|i| &i.value == current)
                .map(|i| i.name.clone()),
            SessionConfigSelectOptions::Grouped(groups) => groups
                .iter()
                .flat_map(|g| &g.options)
                .find(|i| &i.value == current)
                .map(|i| i.name.clone()),
            _ => None,
        };
        Some(found.unwrap_or_else(|| current.0.to_string()))
    }

    /// The connection was lost: nothing is running, and its session has to
    /// be loaded (or made) again.
    pub(crate) fn disconnected(&mut self) {
        self.streaming = false;
        self.pending_permissions.clear();
        self.session = if self.session_id.is_some() {
            SessionState::Listed
        } else {
            SessionState::New
        };
    }
}

pub(crate) const NEW_THREAD_TITLE: &str = "New chat";

/// Add `queued` to the transcript as sent; the blocks to send for it.
pub(crate) fn dispatch_prompt(
    thread: &mut AcpThread,
    queued: QueuedPrompt,
    caps: AgentCapabilitiesSummary,
    open: &OpenTexts,
) -> Result<Vec<ContentBlock>, String> {
    let blocks = build_prompt_blocks(&queued.text, &queued.attachments, caps, open)?;
    if thread.items.is_empty() && thread.title == NEW_THREAD_TITLE {
        thread.title = title_from_prompt(&queued.text, &queued.attachments);
    }
    let attachment_badges: Vec<(String, bool)> = queued
        .attachments
        .iter()
        .map(|a| (a.label().to_owned(), a.is_image()))
        .collect();
    thread.items.push(TranscriptItem::User {
        text: queued.text,
        attachments: attachment_badges,
    });
    thread.error = None;
    thread.fresh = false;
    thread.streaming = true;
    thread.scroll_to_bottom = true;
    Ok(blocks)
}

/// Send `entity`'s next queued prompt, if it has one and a session.
pub(crate) fn dispatch_next_queued(world: &mut World, entity: Entity) {
    let Some(connection) = world.get::<AcpThread>(entity).map(|t| t.connection) else {
        return;
    };
    let caps = world
        .get::<AcpConnection>(connection)
        .map(|c| c.capabilities)
        .unwrap_or_default();
    let open = world
        .get::<AcpThread>(entity)
        .and_then(|thread| thread.queued_prompts.front())
        .map(|queued| open_texts(world, &queued.attachments))
        .unwrap_or_default();
    let Some(mut thread) = world.get_mut::<AcpThread>(entity) else {
        return;
    };
    let Some(session_id) = thread.session_id.clone() else {
        return;
    };
    let Some(queued) = thread.queued_prompts.pop_front() else {
        return;
    };
    match dispatch_prompt(&mut thread, queued, caps, &open) {
        Ok(blocks) => send(
            world,
            connection,
            WorkerCommand::SendPrompt { session_id, blocks },
        ),
        Err(err) => {
            if let Some(mut thread) = world.get_mut::<AcpThread>(entity) {
                thread.error = Some(err);
            }
        }
    }
}

pub(crate) fn title_from_prompt(prompt: &str, attachments: &[PromptAttachment]) -> String {
    let first_line = prompt
        .lines()
        .find(|l| !l.trim().is_empty())
        .or_else(|| attachments.first().map(PromptAttachment::label))
        .unwrap_or("Chat");
    let trimmed = first_line.trim();
    if trimmed.chars().count() > 28 {
        let short: String = trimmed.chars().take(27).collect();
        format!("{short}…")
    } else {
        trimmed.to_owned()
    }
}

pub(crate) fn short_session_id(id: &str) -> String {
    if id.chars().count() > 14 {
        let short: String = id.chars().take(12).collect();
        format!("Session {short}…")
    } else {
        format!("Session {id}")
    }
}

/// The connection for the client called `name`.
pub(crate) fn connection_named(world: &mut World, name: &str) -> Option<Entity> {
    world
        .query::<(Entity, &AcpConnection)>()
        .iter(world)
        .find(|(_, connection)| connection.client_name == name)
        .map(|(entity, _)| entity)
}

/// A new thread of `connection`'s, first in its list.
pub(crate) fn spawn_thread(world: &mut World, connection: Entity, thread: AcpThread) -> Entity {
    let entity = world.spawn(thread).id();
    if let Some(mut connection) = world.get_mut::<AcpConnection>(connection) {
        connection.threads.insert(0, entity);
        connection.wanted = true;
    }
    entity
}

/// Show `thread`: focus the chat panel it has, or open one at `placement`.
pub fn open_thread(world: &mut World, thread: Entity, placement: Placement) -> ActionResult {
    let (connection, session_id, title, approval_mode, panel) = {
        let thread = world.get::<AcpThread>(thread).ok_or("The thread is gone")?;
        (
            thread.connection,
            thread.session_id.clone(),
            thread.title.clone(),
            thread.approval_mode,
            thread.panel,
        )
    };
    if let Some(panel) = panel.filter(|&panel| world.get::<AcpPanel>(panel).is_some()) {
        return world.bedtime_mut().layouts_mut().focus_panel(panel);
    }
    let client_name = world
        .get::<AcpConnection>(connection)
        .map(|connection| connection.client_name.clone())
        .ok_or("The thread's client is gone")?;
    let panel = world.bedtime_mut().open_panel_at(
        (
            AcpPanel {
                client_name,
                session_id,
                title,
                approval_mode,
            },
            AcpRuntime::showing(thread),
        ),
        placement,
    )?;
    if let Some(mut thread) = world.get_mut::<AcpThread>(thread) {
        thread.panel = Some(panel);
    }
    Ok(())
}

/// Start a thread with the client called `client_name`, in a chat panel at
/// `placement`.
pub fn new_thread(world: &mut World, client_name: &str, placement: Placement) -> ActionResult {
    let connection = connection_named(world, client_name)
        .ok_or_else(|| format!("ACP client \"{client_name}\" is not configured in Settings."))?;
    let thread = spawn_thread(
        world,
        connection,
        AcpThread::new(connection, NEW_THREAD_TITLE.into()),
    );
    open_thread(world, thread, placement)
}

/// Delete `thread` (its session too, where the agent can; closed or stopped
/// otherwise), closing the panel that shows it.
pub fn delete_thread(world: &mut World, entity: Entity) {
    let Some(thread) = world.entity_mut(entity).take::<AcpThread>() else {
        return;
    };
    world.entity_mut(entity).despawn();
    let caps = match world.get_mut::<AcpConnection>(thread.connection) {
        Some(mut connection) => {
            connection.threads.retain(|&other| other != entity);
            connection.capabilities
        }
        None => return,
    };
    if let Some(session_id) = thread.session_id {
        if caps.can_delete_session {
            send(
                world,
                thread.connection,
                WorkerCommand::DeleteSession { session_id },
            );
        } else if caps.can_close_session {
            send(
                world,
                thread.connection,
                WorkerCommand::CloseSession { session_id },
            );
        } else if thread.streaming {
            send(
                world,
                thread.connection,
                WorkerCommand::CancelSession { session_id },
            );
        }
    }
    if let Some(panel) = thread
        .panel
        .filter(|&panel| world.get::<AcpPanel>(panel).is_some())
    {
        let layouts = world.bedtime_mut().layouts_mut();
        let closed = layouts
            .panel_layout(panel)
            .and_then(|layout| layouts.user_edit(layout, &LayoutEdit::ClosePanels(vec![panel])));
        world
            .bedtime_mut()
            .notify_failure("Couldn't close the chat", closed);
    }
}

/// Give each chat panel its thread: the one it was opened for, the one with
/// the session it saved, or a new one.
pub(crate) fn bind_panels(world: &mut World) {
    let unbound: Vec<(Entity, AcpPanel)> = world
        .query::<(Entity, &AcpPanel, &AcpRuntime)>()
        .iter(world)
        .filter(|(_, _, rt)| {
            rt.thread
                .is_none_or(|thread| world.get::<AcpThread>(thread).is_none())
        })
        .map(|(panel, acp, _)| (panel, acp.clone()))
        .collect();
    for (panel, acp) in unbound {
        let Some(connection) = connection_named(world, &acp.client_name) else {
            continue;
        };
        let saved = acp.session_id.as_ref().and_then(|session_id| {
            let threads = &world.get::<AcpConnection>(connection)?.threads;
            let existing = threads.iter().copied().find(|&thread| {
                world
                    .get::<AcpThread>(thread)
                    .is_some_and(|t| t.session_id.as_ref() == Some(session_id))
            });
            Some((session_id.clone(), existing))
        });
        let shown_elsewhere = |world: &World, thread: Entity| {
            world
                .get::<AcpThread>(thread)
                .and_then(|t| t.panel)
                .is_some_and(|other| other != panel && world.get::<AcpPanel>(other).is_some())
        };
        let thread = match saved {
            Some((_, Some(existing))) if !shown_elsewhere(world, existing) => existing,
            Some((session_id, None)) => {
                let title = if acp.title.trim().is_empty() {
                    short_session_id(&session_id)
                } else {
                    acp.title.clone()
                };
                spawn_thread(
                    world,
                    connection,
                    AcpThread::unloaded(connection, session_id, title),
                )
            }
            // No session saved, or another panel has it already.
            _ => spawn_thread(
                world,
                connection,
                AcpThread::new(connection, NEW_THREAD_TITLE.into()),
            ),
        };
        if let Some(mut thread) = world.get_mut::<AcpThread>(thread) {
            thread.panel = Some(panel);
            thread.approval_mode = acp.approval_mode;
        }
        if let Some(mut connection) = world.get_mut::<AcpConnection>(connection) {
            connection.wanted = true;
        }
        if let Some(mut rt) = world.get_mut::<AcpRuntime>(panel) {
            rt.thread = Some(thread);
        }
    }
}

/// Forget panels that are gone, and with them the threads nothing was said
/// in.
pub(crate) fn release_panels(world: &mut World) {
    let released: Vec<Entity> = world
        .query::<(Entity, &AcpThread)>()
        .iter(world)
        .filter(|(entity, thread)| {
            thread.panel.is_some_and(|panel| {
                world
                    .get::<AcpRuntime>(panel)
                    .is_none_or(|rt| rt.thread != Some(*entity))
            })
        })
        .map(|(entity, _)| entity)
        .collect();
    for entity in released {
        let Some(mut thread) = world.get_mut::<AcpThread>(entity) else {
            continue;
        };
        thread.panel = None;
        if thread.is_untouched() {
            delete_thread(world, entity);
        }
    }
}

/// Make or load the session of each thread that's shown, once its agent is
/// connected.
pub(crate) fn ensure_sessions(world: &mut World, cwd: &Path) {
    let waiting: Vec<(Entity, Entity, SessionState)> = world
        .query::<(Entity, &AcpThread)>()
        .iter(world)
        .filter(|(_, thread)| {
            thread.panel.is_some()
                && matches!(thread.session, SessionState::New | SessionState::Listed)
        })
        .map(|(entity, thread)| (entity, thread.connection, thread.session))
        .collect();
    for (entity, connection, state) in waiting {
        let Some(caps) = world
            .get::<AcpConnection>(connection)
            .filter(|c| c.status == ConnectionStatus::Connected && c.worker.is_some())
            .map(|c| c.capabilities)
        else {
            continue;
        };
        let Some(mut thread) = world.get_mut::<AcpThread>(entity) else {
            continue;
        };
        let session_id = match (state, thread.session_id.clone()) {
            (SessionState::Listed, Some(session_id))
                if caps.can_load_session || caps.can_resume_session =>
            {
                Some(session_id)
            }
            // The agent can't bring a session back: start another.
            _ => None,
        };
        let mut replaced = false;
        let command = match session_id {
            Some(session_id) => {
                let use_resume = !caps.can_load_session;
                thread.session = SessionState::Loading;
                thread.error = None;
                if !use_resume {
                    // Loading replays the conversation.
                    thread.items.clear();
                }
                WorkerCommand::LoadSession {
                    local_id: entity,
                    session_id,
                    cwd: cwd.to_path_buf(),
                    use_resume,
                }
            }
            None => {
                // An earlier chat the agent can't bring back becomes a new
                // one, and says so: not the old one's name over an agent
                // that remembers none of it.
                replaced = thread.session_id.take().is_some();
                if replaced {
                    thread.listed = false;
                    if thread.items.is_empty() {
                        thread.title = NEW_THREAD_TITLE.into();
                    }
                }
                thread.session = SessionState::Creating;
                WorkerCommand::NewSession {
                    local_id: entity,
                    cwd: cwd.to_path_buf(),
                }
            }
        };
        send(world, connection, command);
        if replaced {
            let client = world
                .get::<AcpConnection>(connection)
                .map(|connection| connection.client_name.clone())
                .unwrap_or_default();
            world.bedtime_mut().notify(Notification::warning(format!(
                "{client} can't reopen earlier chats: this one starts again, and the agent doesn't remember it."
            )));
        }
    }
}
