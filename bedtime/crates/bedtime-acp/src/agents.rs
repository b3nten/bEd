//! The Agents panel: the configured ACP clients, each a group that opens on
//! its threads. Choosing a thread shows it in a chat panel, as choosing a
//! file in the file tree shows it in a viewer.
//!
//! A group that's open has its agent started, to list its sessions; the
//! panel saves which are open, so they start again with it.
use crate::{
    config::ConfiguredClients,
    connection::{AcpConnection, ConnectionStatus, reconnect, refresh_sessions},
    panel::workspace_cwd,
    thread::{AcpThread, delete_thread, new_thread, open_thread},
};
use bedtime_ui::{
    BuiltinIcon, Button, ListRow, MenuItem, PanelUi, Row, Size, Space, Spinner, Text, Tip, Tone,
    UiContext, draw, egui,
};
#[cfg(feature = "persistence")]
use bedtime_workbench::persistence::ReflectPersist;
use bedtime_workbench::{BedtimeWorldExt as _, Panel, PanelPresentation, Placement, ShowPanel};
use bevy::prelude::*;

impl ShowPanel for AgentsPanel {
    const SHOW: &'static str = "agents.show";
}

/// The list of ACP clients and their threads.
#[bedtime_ui::panel]
#[derive(Reflect, Default, Clone, PartialEq, Eq)]
#[reflect(Component, Default)]
#[cfg_attr(feature = "persistence", reflect(Persist))]
// Saved layouts name it by this path, wherever its module moves to.
#[type_path = "bedtime_acp"]
pub struct AgentsPanel {
    /// The clients whose groups are open, by name.
    pub expanded: Vec<String>,
}

impl Panel for AgentsPanel {
    const LABEL: &'static str = "Agents";

    fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, panel: Entity) {
        draw::with_system(ui, world, context, panel, draw_agents);
    }
}

/// The agents of open groups are wanted: started if they aren't running.
pub(crate) fn want_expanded(world: &mut World) {
    let expanded: Vec<String> = world
        .query::<&AgentsPanel>()
        .iter(world)
        .flat_map(|panel| panel.expanded.iter().cloned())
        .collect();
    if expanded.is_empty() {
        return;
    }
    for mut connection in world.query::<&mut AcpConnection>().iter_mut(world) {
        if !connection.wanted && expanded.contains(&connection.client_name) {
            connection.wanted = true;
        }
    }
}

fn draw_agents(
    PanelUi {
        ui,
        entity: panel,
        state,
        ..
    }: PanelUi<AgentsPanel>,
    clients: Res<ConfiguredClients>,
    connections: Query<(Entity, &AcpConnection)>,
    threads: Query<&AcpThread>,
    presentations: Query<&PanelPresentation>,
    mut commands: Commands,
) {
    let names: Vec<&str> = clients
        .0
        .iter()
        .map(|client| client.name.as_str())
        .filter(|name| !name.trim().is_empty())
        .collect();

    egui::Frame::NONE
        .inner_margin(egui::Margin::symmetric(8, 6))
        .show(ui, |ui| {
            if names.is_empty() {
                ui.add(Text::new("No ACP clients are configured.").muted().wrap());
                ui.add(
                    Text::caption("Add one in Settings, under Agents.")
                        .muted()
                        .wrap(),
                );
                return;
            }

            // The thread whose deletion is being confirmed.
            let confirm_id = egui::Id::new((panel, "acp-confirm-delete"));
            let confirming: Option<Entity> = ui.data(|data| data.get_temp(confirm_id)).flatten();
            let mut confirm = confirming;

            egui::ScrollArea::vertical()
                .id_salt((panel, "acp-agents"))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    for name in names {
                        let open = state.expanded.iter().any(|open| open == name);
                        let connection = connections
                            .iter()
                            .find(|(_, connection)| connection.client_name == name);
                        client_header(ui, panel, name, open, connection, &mut commands);
                        if !open {
                            continue;
                        }
                        let Some((_, connection)) = connection else {
                            continue;
                        };
                        ui.indent((panel, name), |ui| {
                            let can_delete = connection.capabilities.can_delete_session;
                            let mut shown = 0;
                            for &entity in &connection.threads {
                                let Ok(thread) = threads.get(entity) else {
                                    continue;
                                };
                                shown += 1;
                                let visible = thread
                                    .panel
                                    .and_then(|panel| presentations.get(panel).ok())
                                    .is_some_and(|presentation| presentation.visible);
                                let mut row = ListRow::new(thread.title())
                                    .selected(visible)
                                    .busy(thread.is_busy());
                                if !thread.pending_permissions.is_empty() {
                                    row = row.trailing("needs permission");
                                }
                                let row = ui.add(row);
                                if row.clicked() {
                                    commands.queue(move |world: &mut World| {
                                        let result =
                                            open_thread(world, entity, Placement::From(panel));
                                        world
                                            .bedtime_mut()
                                            .notify_failure("Couldn't open the chat", result);
                                    });
                                }
                                let deletes = can_delete && thread.session_id.is_some();
                                row.context_menu(|ui| {
                                    let label = if deletes { "Delete…" } else { "Close…" };
                                    if ui.add(MenuItem::new(label).danger()).clicked() {
                                        confirm = Some(entity);
                                        ui.close();
                                    }
                                });
                                if confirm == Some(entity) {
                                    Row::new().gap(Space::Xs).show(ui, |ui| {
                                        let (prompt, label) = if deletes {
                                            ("Delete?", "Delete")
                                        } else {
                                            ("Close?", "Close")
                                        };
                                        ui.add(Text::caption(prompt).muted());
                                        if ui.add(Button::new(label).small().danger()).clicked() {
                                            commands.queue(move |world: &mut World| {
                                                delete_thread(world, entity);
                                            });
                                            confirm = None;
                                        }
                                        if ui.add(Button::new("Cancel").small()).clicked() {
                                            confirm = None;
                                        }
                                    });
                                }
                            }
                            match &connection.status {
                                ConnectionStatus::Error(err) => {
                                    ui.add(Text::caption(err).tone(Tone::Warning).wrap());
                                }
                                ConnectionStatus::Connected if shown == 0 => {
                                    ui.add(Text::caption("No chats yet.").muted());
                                }
                                ConnectionStatus::Idle | ConnectionStatus::Connecting
                                    if shown == 0 =>
                                {
                                    ui.add(Text::caption("Connecting…").muted());
                                }
                                _ => {}
                            }
                        });
                        ui.add_space(Space::Xs.px(ui));
                    }
                });

            if confirm != confirming {
                ui.data_mut(|data| data.insert_temp(confirm_id, confirm));
            }
        });
}

/// A client's row: its name (which opens and closes its group), how its
/// agent is, and what can be done with it.
fn client_header(
    ui: &mut egui::Ui,
    panel: Entity,
    name: &str,
    open: bool,
    connection: Option<(Entity, &AcpConnection)>,
    commands: &mut Commands,
) {
    Row::new().show(ui, |ui| {
        let chevron = if open {
            BuiltinIcon::ChevronDown
        } else {
            BuiltinIcon::ChevronRight
        };
        if ui
            .add(Button::new(name).icon(chevron).ghost().small())
            .clicked()
        {
            let name = name.to_owned();
            draw::update(ui, panel, move |agents: &mut AgentsPanel| {
                if open {
                    agents.expanded.retain(|other| *other != name);
                } else {
                    agents.expanded.push(name);
                }
            });
        }
        let Some((entity, connection)) = connection else {
            return;
        };
        match &connection.status {
            ConnectionStatus::Connecting => {
                ui.add(Spinner::new().size(Size::Sm));
            }
            ConnectionStatus::Error(err) => {
                let label = ui.add(Text::caption("Disconnected").tone(Tone::Warning));
                (&label).tip(err);
            }
            ConnectionStatus::Idle | ConnectionStatus::Connected => {}
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let new_chat = ui.add(
                Button::icon_only(BuiltinIcon::Plus, format!("New {name} chat"))
                    .ghost()
                    .small(),
            );
            (&new_chat).tip(format!("New {name} chat"));
            if new_chat.clicked() {
                let name = name.to_owned();
                commands.queue(move |world: &mut World| {
                    let result = new_thread(world, &name, Placement::From(panel));
                    world
                        .bedtime_mut()
                        .notify_failure("Couldn't start a chat", result);
                    if let Some(mut agents) = world.get_mut::<AgentsPanel>(panel)
                        && !agents.expanded.contains(&name)
                    {
                        agents.expanded.push(name);
                    }
                });
            }
            match &connection.status {
                ConnectionStatus::Connected if connection.capabilities.can_list_sessions => {
                    let refresh = ui.add(
                        Button::icon_only(
                            BuiltinIcon::RefreshCw,
                            format!("Refresh {name} sessions"),
                        )
                        .ghost()
                        .small(),
                    );
                    (&refresh).tip("Refresh sessions");
                    if refresh.clicked() {
                        commands.queue(move |world: &mut World| {
                            let cwd = workspace_cwd(world);
                            refresh_sessions(world, entity, &cwd);
                        });
                    }
                }
                ConnectionStatus::Error(_) => {
                    let retry = ui.add(
                        Button::icon_only(BuiltinIcon::RefreshCw, format!("Reconnect {name}"))
                            .ghost()
                            .small(),
                    );
                    (&retry).tip("Restart agent connection");
                    if retry.clicked() {
                        commands.queue(move |world: &mut World| reconnect(world, entity));
                    }
                }
                _ => {}
            }
        });
    });
}
