//! Agent Client Protocol (ACP) plugin for bedtime.
//!
//! Configure ACP clients (name, command, arguments) in Settings under **Agents**.
//! The [`AgentsPanel`] (View → Agents) lists them, each a group of its
//! threads; choosing a thread shows it in an [`AcpPanel`]: a streaming agent
//! transcript with model/reasoning/mode pickers, slash commands, file/image
//! references, and approval controls.
//!
//! Each client has one [`AcpConnection`] (its agent process, shared by its
//! threads) and each chat one [`AcpThread`]; both outlive the panels that
//! show them, so a thread keeps running with its panel closed.
//!
//! The agent reads and writes files through bed (`agent_files`): an open
//! file as its editor has it, and a write to one as an edit there.
//!
//! Workbench owns worker and background-indexing lifetimes through
//! [`ManagedWork`](bedtime_workbench::ManagedWork); drawing is read-only and
//! queues what it asks for, done in [`AcpSystems`].
pub mod agent_files;
pub mod agents;
pub mod attachments;
pub mod composer;
pub mod config;
pub mod connection;
pub mod files;
pub mod intents;
pub mod panel;
pub mod status;
pub mod thread;
pub mod transcript;
pub mod worker;

pub use agents::AgentsPanel;
pub use attachments::PromptAttachment;
pub use config::{AcpClientConfig, AcpClients, ConfiguredClients};
pub use connection::{AcpConnection, ConnectionStatus};
pub use panel::{AcpPanel, AcpRuntime, ReconnectAgent, StopAgent, ToggleAutoApprove};
pub use thread::{AcpThread, delete_thread, new_thread, open_thread};
pub use worker::ApprovalMode;

use bedtime_ui::UiWaker;
use bedtime_workbench::{
    ActionAppExt as _, BedtimeWorldExt as _, Notification, OnWorkspaceClose, PanelPresentation,
    PanelsAppExt as _,
    settings::SettingsAppExt as _,
    status::{StatusActivity, StatusAppExt as _},
};
use bevy::prelude::*;
use connection::Notice;

/// System set in [`Update`] where ACP workers, background file indexing,
/// event draining, intents, titles, and status activity run.
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AcpSystems;

pub struct AcpPlugin;

impl Plugin for AcpPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ConfiguredClients>()
            .init_resource::<files::WorkspaceFiles>()
            .register_setting::<AcpClients>()
            .register_type::<ApprovalMode>()
            .register_type::<AcpPanel>()
            .register_action::<StopAgent>()
            .register_action::<ReconnectAgent>()
            .register_action::<ToggleAutoApprove>()
            .add_panel::<AgentsPanel>()
            .add_panel_status::<AcpPanel>(status::item)
            .add_systems(Update, update.in_set(AcpSystems))
            .add_systems(OnWorkspaceClose, files::on_workspace_close);
    }
}

/// Everything ACP does each frame, in order: connections for the configured
/// clients, threads for the chat panels, agents started and listened to
/// (also for threads nothing shows), then what the panels asked for.
fn update(world: &mut World) {
    let cwd = panel::workspace_cwd(world);
    let waker = world.get_resource::<UiWaker>().cloned().unwrap_or_default();

    connection::sync_connections(world);
    thread::release_panels(world);
    thread::bind_panels(world);
    agents::want_expanded(world);
    connection::start_wanted(world, &waker);
    files::poll_workspace_index(world, &cwd, &waker);

    let connections: Vec<Entity> = world
        .query_filtered::<Entity, With<AcpConnection>>()
        .iter(world)
        .collect();
    let mut notices = Vec::new();
    for connection in connections {
        notices.extend(connection::drain(world, connection, &cwd));
    }

    thread::ensure_sessions(world, &cwd);
    intents::apply_intents(world, &cwd);
    panel::sync_panels(world);
    show_activity(world);
    notify(world, notices);
}

/// The agents that are answering, in the status bar.
fn show_activity(world: &mut World) {
    let mut working: Vec<(String, usize)> = Vec::new();
    for connection in world.query::<&AcpConnection>().iter(world) {
        let count = connection
            .threads
            .iter()
            .filter(|&&thread| world.get::<AcpThread>(thread).is_some_and(|t| t.streaming))
            .count();
        if count > 0 {
            working.push((connection.client_name.clone(), count));
        }
    }
    let text = match working.as_slice() {
        [] => None,
        [(name, 1)] => Some(format!("{name}…")),
        [(name, count)] => Some(format!("{name} ({count} chats)…")),
        many => Some(format!("{} agents working…", many.len())),
    };
    if let Some(mut activity) = world.get_resource_mut::<StatusActivity>() {
        activity.set("ACP", text);
    }
}

/// Say what happened in threads that aren't in front of the user.
fn notify(world: &mut World, notices: Vec<Notice>) {
    let in_front = |world: &World, thread: Entity| {
        world
            .get::<AcpThread>(thread)
            .and_then(|thread| thread.panel)
            .and_then(|panel| world.get::<PanelPresentation>(panel))
            .is_some_and(|presentation| presentation.visible && presentation.focused)
    };
    let title = |world: &World, thread: Entity| {
        world
            .get::<AcpThread>(thread)
            .map(|thread| thread.title().to_owned())
            .unwrap_or_default()
    };
    for notice in notices {
        let notification = match notice {
            Notice::PermissionRequested {
                thread,
                title: what,
            } if !in_front(world, thread) => Notification::warning(format!(
                "{}: permission needed ({what})",
                title(world, thread)
            )),
            Notice::TurnFinished { thread } if !in_front(world, thread) => {
                Notification::info(format!("{} finished responding", title(world, thread)))
            }
            Notice::Failed(error) => Notification::warning(error),
            _ => continue,
        };
        world.bedtime_mut().notify(notification);
    }
}
