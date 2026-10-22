//! Application diagnostics for workspace-owned language servers. Closing this
//! observer never stops integrations; restart uses the backend's retirement gate.
use bedtime_lsp2::{MessageKind, ServerStatus, workbench::*};
use bedtime_ui2::{UiContext, egui};
use bedtime_workbench2::*;
use bevy::prelude::*;

pub struct LspPanelPlugin;
impl Plugin for LspPanelPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(crate::status_bar::StatusBarPlugin);
        #[cfg(feature = "automation")]
        {
            use bedtime_automation2::AutomationAppExt;
            app.register_automation_command("app.inspect_lsp_servers", inspect);
        }
    }
}

#[cfg(feature = "automation")]
fn inspect(world: &mut World, _: serde_json::Value) -> bedtime_automation2::AutomationResult {
    use serde_json::json;
    let workspaces: Vec<_> = world
        .query_filtered::<Entity, With<Workspace>>()
        .iter(world)
        .collect();
    let mut servers = Vec::new();
    for workspace in workspaces {
        for s in workspace_servers(world, workspace) {
            servers.push(json!({"entity":s.entity.to_bits().to_string(),"workspace":workspace.to_bits().to_string(),"program":s.program,"root":s.root,"languages":s.languages,"status":status_text(&s.status),"retiring":s.retiring,"pid":s.activity.process_id,"analysis_epoch":s.activity.analysis_epoch,"project_status":s.activity.project_status.as_ref().map(|p|json!({"health":p.health,"quiescent":p.quiescent,"message":p.message})),"definition":s.activity.supports_definition,"hover":s.activity.supports_hover,"completion":s.activity.supports_completion,"completion_resolve":s.activity.supports_completion_resolve,"name":s.activity.server_name,"version":s.activity.server_version,"pending_requests":s.activity.pending_requests,"documents":s.activity.documents,"progress":s.activity.progress.iter().map(|p|json!({"token":p.token,"title":p.title,"message":p.message,"percentage":p.percentage})).collect::<Vec<_>>(),"messages":s.activity.messages.iter().map(|m|json!({"kind":format!("{:?}",m.kind),"text":m.text})).collect::<Vec<_>>(),"stderr":s.activity.stderr.as_ref()}));
        }
    }
    let panels: Vec<_> = world
        .query_filtered::<Entity, With<LspPanel>>()
        .iter(world)
        .map(|e| e.to_bits().to_string())
        .collect();
    Ok(json!({"servers":servers,"panels":panels}))
}

#[derive(Component, Default)]
#[require(PanelInstance = PanelInstance::of::<Self>())]
pub struct LspPanel;

pub(crate) fn diagnostic_state(
    report: Option<&bedtime_editor2::Diagnostics>,
    revision: u64,
) -> &'static str {
    match report {
        None => "No report",
        Some(report) if report.revision != revision => "Stale",
        Some(report) if !report.verified => "Current (unversioned)",
        Some(_) => "Current",
    }
}
pub(crate) fn status_text(status: &ServerStatus) -> &'static str {
    match status {
        ServerStatus::Starting => "Starting",
        ServerStatus::Ready => "Ready",
        ServerStatus::Failed(_) => "Failed",
        ServerStatus::Stopped => "Stopped",
    }
}
impl Panel for LspPanel {
    const KEY: &'static str = "bed.lsp";
    const LABEL: &'static str = "Language Servers";
    fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, panel: Entity) {
        let Some(workspace) = world.get::<InWorkspace>(panel).map(|o| o.0) else {
            return;
        };
        let servers = workspace_servers(world, workspace);
        let configured = world.get::<LspWorkspace>(workspace).cloned();
        let docs: Vec<_> = world
            .query::<(Entity, &LspDocument, &InWorkspace)>()
            .iter(world)
            .filter(|(_, _, owner)| owner.0 == workspace)
            .map(|(entity, source, _)| (entity, source.clone()))
            .collect();
        let surface = *world.get::<WorkbenchSurface>(context.surface).unwrap();
        let mut restart = None;
        context.frame(ui).scroll("language-servers", surface.rounding, |mut frame| {
            frame.padded(surface.padding, |frame| {
                let ui = frame.ui;
                ui.heading(Self::LABEL);
                if let Some(project) = &configured {
                    ui.label(project.root.display().to_string());
                    if project.servers.is_empty() {
                        ui.weak("No language servers configured. Configure them in Settings → Editor.");
                    }
                    for (language, config) in &project.servers {
                        if !servers.iter().any(|s| !s.retiring && s.languages.contains(language)) {
                            ui.label(format!("{language} · {} · Not started", config.program));
                            ui.weak("Starts when an eligible file is opened (up to 8 MiB).");
                        }
                    }
                } else {
                    ui.weak("Language tools are disabled or this workspace is not ready for project servers. Open a folder and check Settings → Editor.");
                }
                for server in &servers {
                    let activity = &server.activity;
                    let state = if server.retiring { "Stopping" } else { status_text(&server.status) };
                    let name = activity.server_name.as_deref().unwrap_or_else(|| {
                        std::path::Path::new(&server.program).file_name()
                            .and_then(|s| s.to_str()).unwrap_or(&server.program)
                    });
                    egui::CollapsingHeader::new(format!("{name} · {state}"))
                        .id_salt(server.entity).default_open(true).show(ui, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                if let Some(pid) = activity.process_id {
                                    ui.label(format!("PID {pid}"));
                                }
                                ui.label(server.languages.join(", "));
                                if ui.add_enabled(
                                    !server.retiring && crate::close::accepts_user_actions(world),
                                    egui::Button::new("Restart"),
                                ).clicked() {
                                    restart = Some(server.entity);
                                }
                            });
                            ui.label(format!("Root: {}", server.root.display()));
                            ui.add(egui::Label::new(format!("Executable: {}", server.program)).wrap().selectable(true));
                            if let Some(name) = &activity.server_name {
                                ui.label(format!("{name} {}", activity.server_version.as_deref().unwrap_or("")));
                            }
                            if let ServerStatus::Failed(error) = &server.status {
                                ui.colored_label(ui.visuals().error_fg_color, error.as_ref());
                            }
                            ui.label(format!("{} documents · {} pending requests", activity.documents, activity.pending_requests));
                            if let Some(sync) = activity.synchronization {
                                let encoding = match activity.encoding {
                                    Some(bedtime_lsp2::Encoding::Utf8) => "UTF-8",
                                    Some(bedtime_lsp2::Encoding::Utf16) => "UTF-16",
                                    None => "Unknown encoding",
                                };
                                let definition = if activity.supports_definition { "supported" } else { "unsupported" };
                                ui.add(egui::Label::new(format!("{sync} synchronization · {encoding} · Definition {definition}")).wrap());
                                ui.weak(format!("Hover {} · Completion {}", if activity.supports_hover { "supported" } else { "unsupported" }, if activity.supports_completion { "supported" } else { "unsupported" }));
                            }
                            if let Some(project) = &activity.project_status {
                                ui.weak(if project.quiescent { "Project analysis settled" } else { "Project analysis loading" });
                                if let Some(message) = &project.message {
                                    let color = match project.health.as_str() { "error" => ui.visuals().error_fg_color,"warning" => ui.visuals().warn_fg_color,_ => ui.visuals().weak_text_color() };
                                    ui.colored_label(color,message);
                                }
                            }
                            if let Some(last) = activity.last_activity {
                                ui.weak(format!("Last server message {} s ago", last.elapsed().as_secs()));
                            }
                            for progress in &activity.progress {
                                ui.horizontal_wrapped(|ui| {
                                    ui.spinner();
                                    ui.label(progress.label());
                                });
                                if progress.updated_at.elapsed().as_secs() >= 30 {
                                    ui.weak(format!("No progress update for {} s; the server has not ended this operation.", progress.updated_at.elapsed().as_secs()));
                                }
                            }
                            if activity.progress.is_empty() && server.status == ServerStatus::Ready {
                                ui.weak("No reported background work.");
                            }
                            if !activity.messages.is_empty() {
                                egui::CollapsingHeader::new("Recent server messages (latest 64)").show(ui, |ui| {
                                    for message in &activity.messages {
                                        let color = match message.kind {
                                            MessageKind::Error => ui.visuals().error_fg_color,
                                            MessageKind::Warning => ui.visuals().warn_fg_color,
                                            _ => ui.visuals().text_color(),
                                        };
                                        ui.add(egui::Label::new(egui::RichText::new(format!("{:?}: {}", message.kind, message.text)).color(color)).wrap().selectable(true));
                                    }
                                });
                            }
                            if !activity.stderr.is_empty() {
                                egui::CollapsingHeader::new("Process stderr (latest 64 KiB)").show(ui, |ui| {
                                    ui.add(egui::Label::new(activity.stderr.as_ref()).wrap().selectable(true));
                                });
                            }
                        });
                }
                ui.separator();
                ui.strong("Documents");
                if docs.is_empty() {
                    ui.weak("No eligible documents. Servers start lazily when a configured language is opened.");
                }
                for (entity, source) in &docs {
                    ui.label(format!("{} · {}", source.uri.path().display(), source.language));
                    if let Some(state) = world.get::<DocumentLsp>(*entity) {
                        if let Some(error) = &state.error {
                            ui.colored_label(ui.visuals().error_fg_color, error);
                        }
                        if let Ok(info) = inspect_document(world, DocumentId(*entity)) {
                            ui.weak(format!("Revision {} · Diagnostics: {}", info.revision, diagnostic_state(state.diagnostics.as_deref(), info.revision)));
                        }
                    } else {
                        let oversized = cast_document::<bedtime_editor2::TextBuffer>(world, DocumentId(*entity)).ok()
                            .and_then(|handle| read_document(world, handle).ok())
                            .is_some_and(|text| text.len() > bedtime_lsp2::MAX_SOURCE_BYTES);
                        let reason = if oversized {
                            "Language tools are limited to 8 MiB; plain editing remains available."
                        } else if configured.is_none() {
                            "Language tools are disabled or the workspace is not ready."
                        } else if configured.as_ref().is_some_and(|p| !p.servers.contains_key(&source.language)) {
                            "No server is configured for this language."
                        } else {
                            "Not attached: a clean hidden document may be detached, or its server is restarting."
                        };
                        ui.weak(reason);
                    }
                }
            });
        });
        if let Some(entity) = restart {
            let _ = restart_instance(world, entity);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bedtime_editor2::Diagnostics;
    #[test]
    fn no_report_and_stale_reports_do_not_claim_background_work() {
        assert_eq!(diagnostic_state(None, 3), "No report");
        let report = Diagnostics::new(2, true, vec![]);
        assert_eq!(diagnostic_state(Some(&report), 3), "Stale");
        assert_eq!(diagnostic_state(Some(&report), 2), "Current");
    }
}
