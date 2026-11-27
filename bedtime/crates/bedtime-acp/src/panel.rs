//! The chat panel: one [`AcpThread`], shown. The panel saves which thread it
//! shows ([`AcpPanel`]) and keeps what's being typed ([`AcpRuntime`]); the
//! thread and its client's connection have the rest, and outlive it. Drawing
//! is read-only (`draw::with_lent`, the thread lent to it) and queues
//! [`AcpIntent`]s, applied in `AcpSystems`: the header here, the transcript
//! (`transcript`) and what's under it (`composer`).
use crate::{
    attachments::PromptAttachment,
    composer::{self, Composer},
    connection::{AcpConnection, ConnectionStatus},
    files::WorkspaceFiles,
    intents::AcpIntent,
    thread::{AcpThread, UsageInfo},
    transcript,
    worker::ApprovalMode,
};
use agent_client_protocol::v1::schema::AvailableCommand;
use bedtime_ui::{
    BuiltinIcon, Button, Row, Size, Spinner, Text, Tip, Tone, UiContext, ViewUi, draw, egui,
    picker::PickerState,
};
#[cfg(feature = "persistence")]
use bedtime_workbench::persistence::ReflectPersist;
use bedtime_workbench::{
    Action, ActionHandler, ActionInvocation, ActionResult, Handler, Panel, PanelInstance, Root,
};
use bevy::prelude::*;
use std::path::PathBuf;

/// A chat with an ACP client's agent: one thread, by its session.
///
/// Only what a restored layout needs is here; the conversation is the
/// [`AcpThread`]'s, and what's being typed the [`AcpRuntime`]'s.
#[bedtime_ui::panel]
#[derive(Reflect, Default, Clone, PartialEq, Eq)]
#[reflect(Component, Default)]
#[cfg_attr(feature = "persistence", reflect(Persist))]
#[require(
    AcpRuntime,
    ActionHandler<bedtime_workbench::New> = ActionHandler::new(Handler::new(new_chat_action)),
    ActionHandler<StopAgent> = ActionHandler::new(Handler::new(stop_action)),
    ActionHandler<ReconnectAgent> = ActionHandler::new(Handler::new(reconnect_action)),
    ActionHandler<ToggleAutoApprove> = ActionHandler::new(Handler::new(toggle_approval_action)),
)]
// Saved layouts name it by this path, wherever its module moves to.
#[type_path = "bedtime_acp"]
pub struct AcpPanel {
    /// Name of the configured ACP client (matches `AcpClientConfig::name`).
    pub client_name: String,
    /// The thread's ACP session, once it has one: loaded again when the
    /// panel is restored, where the agent can.
    pub session_id: Option<String>,
    /// The thread's title, to show until the agent gives it again.
    pub title: String,
    /// The thread's tool approval policy (`Ask` or `AutoApprove`).
    pub approval_mode: ApprovalMode,
}

/// Non-persisted state of an [`AcpPanel`]: its thread, and the prompt being
/// written.
#[derive(Component, Default)]
pub struct AcpRuntime {
    /// The thread shown; given by `bind_panels` if the panel has none.
    pub(crate) thread: Option<Entity>,
    pub(crate) input: String,
    pub(crate) attachments: Vec<PromptAttachment>,
    pub(crate) attachment_error: Option<String>,
    /// The `/` commands that fit what's typed: the list's state, its rows
    /// (with the characters matched), and how many commands they were found
    /// among.
    pub(crate) slash: PickerState,
    pub(crate) slash_rows: Vec<(AvailableCommand, Vec<usize>)>,
    pub(crate) slash_seen: usize,
    pub(crate) slash_dismissed: bool,
    /// The same for the `@` files.
    pub(crate) mention: PickerState,
    pub(crate) mention_rows: Vec<(PathBuf, String, Vec<usize>)>,
    pub(crate) mention_seen: usize,
    /// Where the `@` is that Escape closed the list for.
    pub(crate) mention_dismissed_at: Option<usize>,
    pub(crate) intents: Vec<AcpIntent>,
}

impl AcpRuntime {
    pub(crate) fn showing(thread: Entity) -> Self {
        Self {
            thread: Some(thread),
            ..Self::default()
        }
    }
}

/// Stop the focused chat's agent in the middle of its turn.
pub struct StopAgent;
impl Action for StopAgent {
    const KEY: &'static str = "agents.stop";
    const LABEL: &'static str = "Stop Agent";
}

/// Restart the agent of the focused chat's client.
pub struct ReconnectAgent;
impl Action for ReconnectAgent {
    const KEY: &'static str = "agents.reconnect";
    const LABEL: &'static str = "Reconnect Agent";
}

/// Switch the focused chat between asking before tools run and approving
/// them by itself.
pub struct ToggleAutoApprove;
impl Action for ToggleAutoApprove {
    const KEY: &'static str = "agents.toggle_auto_approve";
    const LABEL: &'static str = "Toggle Auto-approve Tools";
}

/// Ask the focused chat panel for `intent`, done with its others.
fn ask(world: &mut World, invocation: ActionInvocation, intent: AcpIntent) -> ActionResult {
    let panel = invocation.entity.ok_or("No focused chat panel")?;
    let mut runtime = world
        .get_mut::<AcpRuntime>(panel)
        .ok_or("Target is not a chat panel")?;
    runtime.intents.push(intent);
    Ok(())
}

/// `Cmd/Ctrl+N` while a chat panel is focused starts a new thread with the
/// same client, in a new panel.
fn new_chat_action(world: &mut World, invocation: ActionInvocation) -> ActionResult {
    ask(world, invocation, AcpIntent::NewChat)
}

fn stop_action(world: &mut World, invocation: ActionInvocation) -> ActionResult {
    ask(world, invocation, AcpIntent::CancelPrompt)
}

fn reconnect_action(world: &mut World, invocation: ActionInvocation) -> ActionResult {
    ask(world, invocation, AcpIntent::Reconnect)
}

fn toggle_approval_action(world: &mut World, invocation: ActionInvocation) -> ActionResult {
    let mode = invocation
        .entity
        .and_then(|panel| world.get::<AcpRuntime>(panel)?.thread)
        .and_then(|thread| world.get::<AcpThread>(thread))
        .map(|thread| thread.approval_mode)
        .ok_or("The chat has no thread yet")?;
    let toggled = match mode {
        ApprovalMode::Ask => ApprovalMode::AutoApprove,
        ApprovalMode::AutoApprove => ApprovalMode::Ask,
    };
    ask(world, invocation, AcpIntent::SetApprovalMode(toggled))
}

/// The folder agents work in: the app's `Root`. Without one no agent is
/// started (never in the process's own folder, or `/`), and this is empty.
pub(crate) fn workspace_cwd(world: &World) -> PathBuf {
    world
        .get_resource::<Root>()
        .map(|root| root.0.clone())
        .unwrap_or_default()
}

/// Keep each chat panel's tab and what it saves in step with its thread.
pub(crate) fn sync_panels(world: &mut World) {
    let panels: Vec<(Entity, Entity)> = world
        .query::<(Entity, &AcpRuntime)>()
        .iter(world)
        .filter_map(|(panel, rt)| Some((panel, rt.thread?)))
        .collect();
    for (panel, thread) in panels {
        let Some(thread) = world.get::<AcpThread>(thread) else {
            continue;
        };
        let (session_id, title, approval_mode) = (
            thread.session_id.clone(),
            thread.title.trim().to_owned(),
            thread.approval_mode,
        );
        let Some(acp) = world.get::<AcpPanel>(panel) else {
            continue;
        };
        let tab_title = if title.is_empty() {
            acp.client_name.trim().to_owned()
        } else {
            title.clone()
        };
        let stale = acp.session_id != session_id
            || acp.title != title
            || acp.approval_mode != approval_mode;
        if stale && let Some(mut acp) = world.get_mut::<AcpPanel>(panel) {
            acp.session_id = session_id;
            acp.title = title;
            acp.approval_mode = approval_mode;
        }
        if !tab_title.is_empty()
            && world
                .get::<PanelInstance>(panel)
                .is_some_and(|instance| instance.title != tab_title)
            && let Some(mut instance) = world.get_mut::<PanelInstance>(panel)
        {
            instance.title = tab_title;
        }
    }
}

/// What's lent to the panel's draw: its own state, and its thread.
struct ChatView {
    rt: AcpRuntime,
    thread: Option<AcpThread>,
}

impl Panel for AcpPanel {
    const LABEL: &'static str = "Chat";

    fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, panel: Entity) {
        let Some(rt) = world
            .get_mut::<AcpRuntime>(panel)
            .map(|mut rt| std::mem::take(&mut *rt))
        else {
            return;
        };
        let shown = rt.thread;
        let thread = shown.and_then(|thread| {
            world
                .get_mut::<AcpThread>(thread)
                .map(|mut thread| std::mem::take(&mut *thread))
        });
        draw::with_lent(
            ui,
            world,
            context,
            panel,
            ChatView { rt, thread },
            draw_acp_panel,
            |world, view| {
                if let (Some(entity), Some(thread)) = (shown, view.thread)
                    && let Some(mut slot) = world.get_mut::<AcpThread>(entity)
                {
                    *slot = thread;
                }
                if let Some(mut slot) = world.get_mut::<AcpRuntime>(panel) {
                    *slot = view.rt;
                }
            },
        );
    }
}

/// Pure UI drawing system for [`AcpPanel`]: reads the panel, its thread and
/// its client's connection, renders the layout, and queues [`AcpIntent`]s.
fn draw_acp_panel(
    ViewUi {
        ui,
        entity: panel,
        view,
        context,
    }: ViewUi<ChatView>,
    panels: Query<&AcpPanel>,
    connections: Query<&AcpConnection>,
    files: Res<WorkspaceFiles>,
    root: Option<Res<Root>>,
) {
    let Ok(saved) = panels.get(panel) else {
        return;
    };
    let ChatView { rt, thread } = view;
    let connection = thread
        .as_ref()
        .and_then(|thread| connections.get(thread.connection).ok());
    let status = connection.map_or(ConnectionStatus::Idle, |c| c.status.clone());
    let agent_info = connection.and_then(|c| c.agent_info.clone());
    let supports_image = connection.is_some_and(|c| c.capabilities.supports_image);
    let has_worker = connection.is_some_and(|c| c.worker.is_some());
    let cwd = root.map(|r| r.0.clone()).unwrap_or_default();

    // Files dropped on the panel are attached.
    let panel_rect = ui.available_rect_before_wrap();
    let pointer_in_panel = ui.ctx().input(|i| {
        i.pointer
            .hover_pos()
            .is_some_and(|pos| panel_rect.contains(pos))
    });
    if pointer_in_panel {
        let dropped: Vec<PathBuf> = ui.ctx().input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter(|f| !f.path().as_os_str().is_empty())
                .map(|f| f.path().to_path_buf())
                .collect()
        });
        if !dropped.is_empty() {
            rt.intents.push(AcpIntent::AttachPaths(dropped));
        }
    }

    // No frame of its own, so the panel's background shows through.
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE)
        .show(ui, |ui| {
            let usage_label = thread
                .as_ref()
                .and_then(|thread| thread.usage.as_ref())
                .map(format_usage);
            egui::Panel::top(egui::Id::new((panel, "acp-header")))
                .frame(egui::Frame::NONE.inner_margin(egui::Margin::symmetric(12, 6)))
                .show(ui, |ui| {
                    Row::new().show(ui, |ui| {
                        ui.add(Text::new(&saved.client_name).strong());
                        if let Some(info) = &agent_info {
                            ui.add(Text::caption(info).muted().truncate());
                        }

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let reconnect = ui.add(
                                Button::icon_only(BuiltinIcon::RefreshCw, "Reconnect")
                                    .ghost()
                                    .small(),
                            );
                            (&reconnect).tip("Restart agent connection");
                            if reconnect.clicked() {
                                rt.intents.push(AcpIntent::Reconnect);
                            }

                            match &status {
                                ConnectionStatus::Connecting => {
                                    ui.add(Spinner::new().size(Size::Sm));
                                    ui.add(Text::caption("Connecting…").muted());
                                }
                                ConnectionStatus::Connected => {
                                    if let Some(usage) = &usage_label {
                                        ui.add(Text::caption(usage).muted());
                                    }
                                }
                                ConnectionStatus::Error(_) => {
                                    ui.add(Text::caption("Disconnected").tone(Tone::Warning));
                                }
                                ConnectionStatus::Idle => {}
                            }
                        });
                    });
                });

            egui::Panel::bottom(egui::Id::new((panel, "acp-composer")))
                .frame(egui::Frame::NONE.inner_margin(egui::Margin::symmetric(12, 8)))
                .show(ui, |ui| {
                    composer::draw(
                        ui,
                        &context,
                        rt,
                        thread,
                        Composer {
                            panel,
                            saved,
                            files: files.cached(),
                            cwd: &cwd,
                            supports_image,
                            has_worker,
                            pointer_in_panel,
                        },
                    );
                });

            let mut asked = Vec::new();
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE.inner_margin(egui::Margin::symmetric(12, 8)))
                .show(ui, |ui| {
                    transcript::draw(
                        ui,
                        &context,
                        panel,
                        thread,
                        &status,
                        &saved.client_name,
                        &mut asked,
                    );
                });
            rt.intents.extend(asked);
        });
}

/// Add a button capped to `max_width` so long labels truncate with an ellipsis
/// instead of widening the scroll area and breaking prose line wrapping.
pub(crate) fn add_fitted_button(
    ui: &mut egui::Ui,
    button: Button,
    max_width: f32,
    truncated_tip: Option<&str>,
) -> egui::Response {
    let natural = button.width_in(ui);
    let max_width = max_width.max(24.0);
    let truncated = natural > max_width;
    let button = if truncated {
        button.width(max_width).align(egui::Align::Min)
    } else {
        button
    };
    let resp = ui.add(button);
    if truncated && let Some(tip) = truncated_tip {
        (&resp).tip(tip);
    }
    resp
}

pub(crate) fn one_line_preview(text: &str, max_chars: usize) -> String {
    let first = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    if first.chars().count() > max_chars {
        let short: String = first.chars().take(max_chars.saturating_sub(1)).collect();
        format!("{short}…")
    } else {
        first.to_owned()
    }
}

pub(crate) fn format_usage(usage: &UsageInfo) -> String {
    let tokens = format!(
        "{} / {}",
        format_tokens(usage.used),
        format_tokens(usage.size)
    );
    match &usage.cost {
        Some(cost) if cost.currency.eq_ignore_ascii_case("USD") => {
            format!("{tokens} · ${:.2}", cost.amount)
        }
        Some(cost) => format!("{tokens} · {:.2} {}", cost.amount, cost.currency),
        None => tokens,
    }
}

fn format_tokens(count: u64) -> String {
    if count >= 1_000_000 {
        format!("{:.1}M", count as f64 / 1_000_000.0)
    } else if count >= 1_000 {
        format!("{:.1}k", count as f64 / 1_000.0)
    } else {
        count.to_string()
    }
}
