//! Application-owned status row. Workbench owns its placement and workspace
//! lifetime; this renderer projects language/document observations directly.
use bedtime_editor::workbench::WorkbenchEditor;
use bedtime_lsp::{
    ServerStatus,
    workbench::{self, DocumentLsp},
};
use bedtime_ui::{BuiltinIcon, UiContext, UiSystems, egui};
use bedtime_workbench::*;
use bevy::prelude::*;
use std::time::{Duration, Instant};

const ERROR_DURATION: Duration = Duration::from_secs(8);

/// Keep the observed error after dismissal so unchanged source state cannot
/// recreate its notification every frame. Document failures remain authoritative.
#[derive(Resource, Default)]
struct ErrorNotice {
    observed: Option<(Option<DocumentId>, String, Instant)>,
    reported_at: Option<Instant>,
}

impl ErrorNotice {
    fn visible(&mut self, document: Option<DocumentId>, error: Option<&str>, now: Instant) -> bool {
        let Some(error) = error else {
            self.observed = None;
            return false;
        };
        if self
            .observed
            .as_ref()
            .is_none_or(|(previous_document, previous, _)| {
                *previous_document != document || previous != error
            })
        {
            self.observed = Some((document, error.to_owned(), now));
        }
        now.duration_since(self.observed.as_ref().unwrap().2) < ERROR_DURATION
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct StatusSnapshot {
    panel: Option<Entity>,
    document: Option<DocumentId>,
    title: String,
    lsp: String,
    starting: bool,
    server_errors: usize,
    errors: usize,
    warnings: usize,
    stale_diagnostics: bool,
    no_report: bool,
    work: Vec<String>,
    unversioned: bool,
    document_error: Option<String>,
    document_error_at: Option<Instant>,
    details: Vec<String>,
}

#[derive(Component, Default)]
struct StatusBar {
    snapshot: StatusSnapshot,
    rect: Option<egui::Rect>,
    menu_rect: Option<egui::Rect>,
}

pub struct StatusBarPlugin;
impl Plugin for StatusBarPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PreUpdate, install.before(UiSystems::Draw));
        #[cfg(feature = "automation")]
        {
            use bedtime_automation::AutomationAppExt;
            app.register_automation_command("app.inspect_status_bar",|world,_| {
                let bars: Vec<_> = world.query::<(Entity,&StatusBar)>().iter(world).map(|(entity,bar)| {
                    let s=&bar.snapshot;
                    serde_json::json!({
                        "entity": entity.to_bits().to_string(),
                        "panel": s.panel.map(|p| p.to_bits().to_string()),
                        "document": s.document.map(|d| d.0.to_bits().to_string()),
                        "title": s.title,
                        "lsp": s.lsp,
                        "server_errors": s.server_errors,
                        "errors": s.errors,
                        "warnings": s.warnings,
                        "stale_diagnostics": s.stale_diagnostics,
                        "no_report": s.no_report,
                        "work": s.work,
                        "unversioned": s.unversioned,
                        "document_error": s.document_error,
                        "details": s.details,
                        "menu_rect": bar.menu_rect.map(|r|
                            serde_json::json!({
                                "min": [r.min.x, r.min.y],
                                "max": [r.max.x,r.max.y]
                            })
                        ),
                        "rect": bar.rect.map(|r| serde_json::json!({"min": [r.min.x, r.min.y], "max":[r.max.x,r.max.y]}))
                    })
                }).collect();
                Ok(serde_json::json!({"bars":bars}))
            });
        }
    }
}

fn install(world: &mut World) {
    {
        let mut state = snapshot(world);
        let mut notice = world.remove_resource::<ErrorNotice>().unwrap_or_default();
        if notice.reported_at != state.document_error_at {
            notice.observed = None;
            notice.reported_at = state.document_error_at;
        }
        if !notice.visible(
            state.document,
            state.document_error.as_deref(),
            Instant::now(),
        ) {
            state.document_error = None;
        }
        world.insert_resource(notice);
        let show = !state.lsp.is_empty() || state.document_error.is_some();
        let mut workbench = Workbench::new(world);
        if !show {
            if workbench
                .status_bar()
                .is_some_and(|bar| workbench.world().get::<StatusBar>(bar).is_some())
            {
                workbench.remove_status_bar();
            }
            return;
        }
        if workbench.status_bar().is_none() {
            let renderer = workbench.register_status_bar(draw);
            workbench
                .world_mut()
                .entity_mut(renderer)
                .insert(StatusBar {
                    snapshot: state,
                    rect: None,
                    menu_rect: None,
                });
        } else if let Some(mut bar) = workbench
            .status_bar()
            .and_then(|bar| workbench.world_mut().get_mut::<StatusBar>(bar))
        {
            bar.snapshot = state;
        }
    }
}

fn snapshot(world: &mut World) -> StatusSnapshot {
    // Startup/restore may intentionally have a workspace without any layout.
    let focused =
        active_layout(world).and_then(|layout| Workbench::new(world).focused_panel(layout));
    let panel = focused
        .filter(|p| {
            world.get::<WorkbenchEditor>(*p).is_some() && world.get::<DocumentView>(*p).is_some()
        })
        .or_else(|| {
            world
                .get_resource::<bed_structure::StructureSource>()
                .and_then(|s| s.source)
                .filter(|s| {
                    world
                        .get::<DocumentView>(s.panel)
                        .is_some_and(|v| v.document() == s.document)
                })
                .map(|s| s.panel)
        });
    let mut snapshot = StatusSnapshot {
        panel,
        ..Default::default()
    };
    let servers = workbench::session_servers(world);
    for server in &servers {
        if server.retiring {
            continue;
        }
        if let ServerStatus::Failed(error) = &server.status {
            snapshot.server_errors += 1;
            snapshot
                .details
                .push(format!("{}: {}", server.program, error));
        }
    }
    let document = panel
        .and_then(|p| world.get::<DocumentView>(p))
        .map(DocumentView::document);
    let selected_status = document.and_then(|doc| workbench::server_status(world, doc));
    // The menu stays available after Stop detaches the focused document, so
    // the user can restart that workspace's retained instance.
    let status = selected_status.or_else(|| {
        servers
            .iter()
            .find(|s| matches!(s.status, ServerStatus::Failed(_)))
            .or_else(|| servers.iter().find(|s| s.status == ServerStatus::Starting))
            .or_else(|| servers.first())
            .map(|s| s.status.clone())
    });
    snapshot.lsp = match status {
        Some(ServerStatus::Ready) => "LSP ready",
        Some(ServerStatus::Starting) => {
            snapshot.starting = true;
            "LSP starting…"
        }
        Some(ServerStatus::Failed(_)) => "LSP error",
        Some(ServerStatus::Stopped) => "LSP stopped",
        None => "",
    }
    .into();
    let selected_server = document.and_then(|doc| workbench::document_server(world, doc));
    let stopping = selected_server
        .map(|entity| servers.iter().any(|s| s.entity == entity && s.retiring))
        .unwrap_or_else(|| !servers.is_empty() && servers.iter().all(|s| s.retiring));
    if stopping {
        snapshot.lsp = "LSP stopping…".into();
        snapshot.starting = true;
    }
    for server in &servers {
        if server.retiring || server.status != ServerStatus::Ready {
            continue;
        }
        for progress in &server.activity.progress {
            let label = progress.label();
            snapshot
                .details
                .push(format!("{}: {}", server.program, label));
            if selected_server.is_none_or(|entity| entity == server.entity) {
                snapshot.work.push(match progress.percentage {
                    Some(percent) => format!("{percent}% · {}", progress.title),
                    None => progress.title.clone(),
                });
            }
            if progress.updated_at.elapsed().as_secs() >= 30 {
                snapshot.details.push(format!(
                    "No progress update for {} s; the server has not ended this operation.",
                    progress.updated_at.elapsed().as_secs()
                ));
            }
        }
    }
    if let Some(document) = document {
        snapshot.document = Some(document);
        if let Ok(info) = inspect_document(world, document) {
            snapshot.title = info.title;
            if let Some(error) = info.last_save_error {
                snapshot.document_error = Some(error.to_string());
            } else if info.conflict {
                snapshot.document_error = Some(
                    "The file changed on disk. Resolve its source conflict before saving.".into(),
                );
            }
            if let Some(state) = world.get::<DocumentLsp>(document.0) {
                if snapshot.document_error.is_none() {
                    snapshot.document_error = state.error.clone();
                }
                if let Some(report) = &state.diagnostics {
                    snapshot.stale_diagnostics = report.revision != info.revision;
                    snapshot.unversioned = !report.verified;
                    if !snapshot.stale_diagnostics {
                        let counts = report.counts();
                        snapshot.errors = counts[0];
                        snapshot.warnings = counts[1];
                        snapshot
                            .details
                            .extend(report.items().iter().take(8).map(|d| d.message.to_string()));
                    }
                } else {
                    snapshot.no_report = true;
                }
            }
        }
        if snapshot.document_error.is_none() {
            if let Some((error, reported_at)) =
                panel.and_then(|p| crate::lsp::error_for_panel(world, p))
            {
                snapshot.document_error = Some(error.to_owned());
                snapshot.document_error_at = Some(reported_at);
            }
        }
    }
    if let Some(error) = &snapshot.document_error {
        snapshot.details.insert(0, error.clone());
    }
    if snapshot.stale_diagnostics {
        snapshot
            .details
            .push("The diagnostic report is from an older document revision. No current counts are available; this does not imply server activity.".into());
    }
    if snapshot.no_report {
        snapshot.details.push("The server has not published a diagnostic report for this document. Servers may publish only on save or when diagnostics change.".into());
    }
    if snapshot.unversioned {
        snapshot.details.push("The server did not supply a diagnostic version; these reports are best-effort observations.".into());
    }
    snapshot
}

fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>) {
    let Some(state) = world
        .get::<StatusBar>(context.view)
        .map(|bar| bar.snapshot.clone())
    else {
        return;
    };
    let mut action = None;
    let menu = context
        .frame(ui)
        .icon_button_sized(BuiltinIcon::Diagnostics, 16.0);
    menu.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Language servers")
    });
    let menu = menu.on_hover_text(if state.lsp.is_empty() {
        "Document error"
    } else {
        &state.lsp
    });
    if state.server_errors > 0 || state.document_error.is_some() {
        ui.painter().circle_filled(
            menu.rect.right_top() + egui::vec2(-3.0, 3.0),
            2.5,
            ui.visuals().error_fg_color,
        );
    }
    egui::Popup::menu(&menu).show(|ui| {
        let servers = workbench::session_servers(world);
        if ui.button("Open Language Servers").clicked() {
            if let Some(layout) = active_layout(world) {
                let _ = crate::menus::open_or_focus(
                    world,
                    layout,
                    <crate::lsp_panel::LspPanel as bedtime_workbench::Panel>::KEY,
                );
            }
            ui.close();
        }
        if !servers.is_empty() {
            ui.separator();
        }
        for server in servers {
            let name = server.activity.server_name.as_deref().unwrap_or_else(|| {
                std::path::Path::new(&server.program)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or(&server.program)
            });
            ui.push_id(server.entity, |ui| {
                ui.menu_button(name, |ui| {
                    if ui
                        .add_enabled(!server.retiring, egui::Button::new("Restart"))
                        .clicked()
                    {
                        action = Some((server.entity, true));
                        ui.close();
                    }
                    if ui
                        .add_enabled(
                            !server.retiring && server.status != ServerStatus::Stopped,
                            egui::Button::new("Stop"),
                        )
                        .clicked()
                    {
                        action = Some((server.entity, false));
                        ui.close();
                    }
                });
            });
        }
    });
    if state.starting || !state.work.is_empty() {
        ui.add(egui::Spinner::new().size(16.0));
    }
    if let Some(work) = state.work.first() {
        let label = if state.work.len() > 1 {
            format!("{work} (+{} tasks)", state.work.len() - 1)
        } else {
            work.clone()
        };
        ui.add_sized(
            egui::vec2(
                (ui.available_width() * 0.4).min(260.0),
                ui.available_height(),
            ),
            egui::Label::new(label).truncate(),
        )
        .on_hover_text(state.work.join("\n"));
    }
    if state.server_errors > 0 {
        let details = workbench::session_servers(world)
            .into_iter()
            .filter_map(|server| match server.status {
                ServerStatus::Failed(error) if !server.retiring => {
                    Some(format!("{}: {error}", server.program))
                }
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        ui.colored_label(
            ui.visuals().error_fg_color,
            format!(
                "{} server {}",
                state.server_errors,
                if state.server_errors == 1 {
                    "error"
                } else {
                    "errors"
                }
            ),
        )
        .on_hover_text(details);
    }
    if state.document.is_some() {
        if state.errors > 0 {
            ui.colored_label(
                ui.visuals().error_fg_color,
                format!(
                    "{} {}",
                    state.errors,
                    if state.errors == 1 { "error" } else { "errors" }
                ),
            );
        }
        if state.warnings > 0 {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                format!(
                    "{} {}",
                    state.warnings,
                    if state.warnings == 1 {
                        "warning"
                    } else {
                        "warnings"
                    }
                ),
            );
        }
        if let Some(error) = &state.document_error {
            if let Some((_, _, since)) = world
                .get_resource::<ErrorNotice>()
                .and_then(|n| n.observed.as_ref())
            {
                ui.ctx()
                    .request_repaint_after(ERROR_DURATION.saturating_sub(since.elapsed()));
            }
            let summary = error
                .lines()
                .find(|line| !line.trim().is_empty())
                .unwrap_or(error);
            let width = ui.available_width().min(420.0);
            ui.add_sized(
                egui::vec2(width, ui.available_height()),
                egui::Label::new(egui::RichText::new(summary).color(ui.visuals().error_fg_color))
                    .truncate(),
            )
            .on_hover_text(error);
        }
    }
    if let Some((server, restart)) = action {
        if restart {
            let _ = workbench::restart_instance(world, server);
        } else {
            let _ = workbench::stop_instance(world, server);
        }
    }
    if let Some(mut bar) = world.get_mut::<StatusBar>(context.view) {
        bar.snapshot = state;
        bar.rect = Some(ui.max_rect());
        bar.menu_rect = Some(menu.rect);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bedtime_editor::{TextBuffer, workbench::bind_text_editor};

    #[test]
    fn error_notices_expire_without_reappearing_and_new_errors_get_their_own_time() {
        let mut notice = ErrorNotice::default();
        let now = Instant::now();
        assert!(notice.visible(None, Some("No definition found"), now));
        assert!(notice.visible(None, Some("No definition found"), now + ERROR_DURATION / 2));
        assert!(!notice.visible(None, Some("No definition found"), now + ERROR_DURATION));
        assert!(!notice.visible(None, Some("No definition found"), now + ERROR_DURATION * 2));
        assert!(notice.visible(
            None,
            Some("Opening definition timed out"),
            now + ERROR_DURATION * 2
        ));
        assert!(!notice.visible(None, None, now + ERROR_DURATION * 2));
        assert!(notice.visible(
            None,
            Some("Opening definition timed out"),
            now + ERROR_DURATION * 3
        ));
    }

    #[test]
    fn missing_lsp_reserves_no_status_row_in_either_workspace() {
        let mut app = App::new();
        app.add_plugins(WorkbenchCorePlugin);
        let a = crate::test_layout(app.world_mut());
        let b = crate::test_layout(app.world_mut());
        install(app.world_mut());
        install(app.world_mut());
        assert_eq!(
            app.world_mut()
                .query::<&StatusBar>()
                .iter(app.world())
                .count(),
            0
        );
        assert_eq!(
            Workbench::for_entity(app.world_mut(), a)
                .unwrap()
                .status_bar(),
            None
        );
        assert_eq!(
            Workbench::for_entity(app.world_mut(), b)
                .unwrap()
                .status_bar(),
            None
        );
    }

    #[test]
    fn empty_workspaces_render_before_session_restoration_creates_a_layout() {
        let mut app = App::new();
        app.add_plugins(WorkbenchCorePlugin);
        let _workspace = app.world_mut().spawn_empty().id();
        install(app.world_mut());
        assert_eq!(snapshot(app.world_mut()).document, None);
        assert_eq!(snapshot(app.world_mut()).lsp, "");
    }

    #[test]
    fn status_uses_the_focused_document_and_reports_save_errors_without_lsp() {
        let mut app = App::new();
        app.add_plugins(WorkbenchCorePlugin);
        let w = crate::test_layout(app.world_mut());
        let document = open_memory_document(
            app.world_mut(),
            TextBuffer::from("text"),
            DocumentOptions::loaded("notes.txt"),
        )
        .unwrap();
        let panel = {
            let mut workbench = Workbench::for_entity(app.world_mut(), w).unwrap();
            let l = workbench.active_layout();
            workbench.create_panel(l, "Code").unwrap()
        };
        bind_text_editor(app.world_mut(), panel, document).unwrap();
        Workbench::for_entity(app.world_mut(), panel)
            .unwrap()
            .focus_panel(panel)
            .unwrap();
        bind_document_save(
            app.world_mut(),
            document,
            DocumentSaveBinding::new(None, |_: DocumentSaveSnapshot<TextBuffer>| {
                Err(DocumentSaveError::Failed("Disk is unavailable".into()))
            }),
        )
        .unwrap();
        save_document(app.world_mut(), document.id()).unwrap();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            app.update();
            let state = snapshot(app.world_mut());
            if let Some(error) = state.document_error {
                assert!(error.contains("Disk is unavailable"));
                assert_eq!(state.document, Some(document.id()));
                assert_eq!(state.title, "notes.txt");
                break;
            }
            assert!(std::time::Instant::now() < until);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        install(app.world_mut());
        let bar = Workbench::for_entity(app.world_mut(), w)
            .unwrap()
            .status_bar()
            .unwrap();
        install(app.world_mut());
        assert_eq!(
            Workbench::for_entity(app.world_mut(), w)
                .unwrap()
                .status_bar(),
            Some(bar)
        );
        assert_eq!(Workbench::new(app.world_mut()).status_bar(), Some(bar));
        app.world_mut().despawn(panel);
        install(app.world_mut());
        assert_eq!(
            Workbench::for_entity(app.world_mut(), w)
                .unwrap()
                .status_bar(),
            None
        );
        assert_eq!(snapshot(app.world_mut()).document, None);
    }
}
