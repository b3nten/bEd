//! Application-owned status row. Workbench owns its placement and workspace
//! lifetime; this renderer projects language/document observations directly.
use bedtime_editor2::workbench::WorkbenchEditor;
use bedtime_lsp2::{
    ServerStatus,
    workbench::{self, DocumentLsp},
};
use bedtime_ui2::{BuiltinIcon, UiContext, UiSystems, egui};
use bedtime_workbench2::*;
use bevy::prelude::*;

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
            use bedtime_automation2::AutomationAppExt;
            app.register_automation_command("app.inspect_status_bar",|world,_| {
                let bars:Vec<_>=world.query::<(Entity,&StatusBar,&InWorkspace)>().iter(world).map(|(entity,bar,owner)| {
                    let s=&bar.snapshot;
                    serde_json::json!({"entity":entity.to_bits().to_string(),"workspace":owner.0.to_bits().to_string(),"panel":s.panel.map(|p|p.to_bits().to_string()),"document":s.document.map(|d|d.0.to_bits().to_string()),"title":s.title,"lsp":s.lsp,"server_errors":s.server_errors,"errors":s.errors,"warnings":s.warnings,"stale_diagnostics":s.stale_diagnostics,"no_report":s.no_report,"work":s.work,"unversioned":s.unversioned,"document_error":s.document_error,"details":s.details,"menu_rect":bar.menu_rect.map(|r|serde_json::json!({"min":[r.min.x,r.min.y],"max":[r.max.x,r.max.y]})),"rect":bar.rect.map(|r|serde_json::json!({"min":[r.min.x,r.min.y],"max":[r.max.x,r.max.y]}))})
                }).collect();Ok(serde_json::json!({"bars":bars}))
            });
        }
    }
}

fn install(world: &mut World) {
    let workspaces: Vec<_> = world
        .query_filtered::<Entity, With<WorkspaceUi>>()
        .iter(world)
        .collect();
    for workspace in workspaces {
        let state = snapshot(world, workspace);
        let show = !state.lsp.is_empty() || state.document_error.is_some();
        let Ok(mut workbench) = Workbench::for_entity(world, workspace) else {
            continue;
        };
        if !show {
            if workbench
                .status_bar()
                .is_some_and(|bar| workbench.world().get::<StatusBar>(bar).is_some())
            {
                workbench.remove_status_bar();
            }
            continue;
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

fn snapshot(world: &mut World, workspace: Entity) -> StatusSnapshot {
    // Startup/restore may intentionally have a workspace without any layout.
    let layout = world
        .query_filtered::<(Entity, &InWorkspace), With<ActiveLayout>>()
        .iter(world)
        .find_map(|(layout, owner)| (owner.0 == workspace).then_some(layout));
    let focused = layout.and_then(|layout| {
        Workbench::for_entity(world, workspace)
            .ok()
            .and_then(|w| w.focused_panel(layout))
    });
    let panel = focused
        .filter(|p| {
            world.get::<WorkbenchEditor>(*p).is_some() && world.get::<DocumentView>(*p).is_some()
        })
        .or_else(|| {
            world
                .get::<bed_structure2::StructureSource>(workspace)
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
    let servers = workbench::workspace_servers(world, workspace);
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
    let status = selected_status
        .or_else(|| {
            servers
                .iter()
                .find(|s| matches!(s.status, ServerStatus::Failed(_)))
                .map(|s| s.status.clone())
        })
        .or_else(|| {
            document
                .is_none()
                .then(|| {
                    servers
                        .iter()
                        .find(|s| matches!(s.status, ServerStatus::Failed(_)))
                        .or_else(|| servers.iter().find(|s| s.status == ServerStatus::Starting))
                        .or_else(|| servers.first())
                        .map(|s| s.status.clone())
                })
                .flatten()
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
            snapshot.document_error = panel
                .and_then(|p| crate::lsp::error_for_panel(world, p))
                .map(str::to_owned);
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
    let Some(workspace) = world.get::<InWorkspace>(context.view).map(|w| w.0) else {
        return;
    };
    let Some(state) = world
        .get::<StatusBar>(context.view)
        .map(|bar| bar.snapshot.clone())
    else {
        return;
    };
    let mut restart = None;
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
        ui.set_max_width(520.0);
        ui.strong(if state.lsp.is_empty() {
            "Document error"
        } else {
            &state.lsp
        });
        if !state.title.is_empty() {
            ui.strong(&state.title);
        }
        egui::ScrollArea::vertical()
            .max_height(240.0)
            .show(ui, |ui| {
                for detail in &state.details {
                    ui.add(egui::Label::new(detail).wrap());
                }
            });
        let servers = workbench::workspace_servers(world, workspace);
        if ui.button("Open Language Servers").clicked() {
            if let Some(layout) = world
                .query_filtered::<(Entity, &InWorkspace), With<ActiveLayout>>()
                .iter(world)
                .find_map(|(e, o)| (o.0 == workspace).then_some(e))
            {
                let _ = crate::layouts::open_or_focus(
                    world,
                    layout,
                    crate::layouts::ToolKind::LanguageServers,
                );
            }
            ui.close();
        }
        if !servers.is_empty() {
            ui.separator();
        }
        for server in servers {
            if ui
                .add_enabled(
                    !server.retiring,
                    egui::Button::new(format!("Restart {}", server.program)),
                )
                .clicked()
            {
                restart = Some(server.entity);
                ui.close();
            }
        }
    });
    if state.starting || !state.work.is_empty() {
        ui.spinner();
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
                ui.text_style_height(&egui::TextStyle::Body),
            ),
            egui::Label::new(label).truncate(),
        )
        .on_hover_text(state.work.join("\n"));
    }
    if state.server_errors > 0 {
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
        );
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
            ui.colored_label(ui.visuals().error_fg_color, "Document error")
                .on_hover_text(error);
        }
    }
    if let Some(server) = restart {
        let _ = workbench::restart_instance(world, server);
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
    use bedtime_editor2::{TextBuffer, workbench::bind_text_editor};

    #[test]
    fn missing_lsp_reserves_no_status_row_in_either_workspace() {
        let mut app = App::new();
        app.add_plugins(WorkbenchCorePlugin);
        let a = Workspaces::new(app.world_mut()).open("A").entity();
        let b = Workspaces::new(app.world_mut()).open("B").entity();
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
        let workspace = app.world_mut().spawn(Workspace::default()).id();
        install(app.world_mut());
        assert_eq!(snapshot(app.world_mut(), workspace).document, None);
        assert_eq!(snapshot(app.world_mut(), workspace).lsp, "");
    }

    #[test]
    fn status_uses_the_focused_document_and_reports_save_errors_without_lsp() {
        let mut app = App::new();
        app.add_plugins(WorkbenchCorePlugin);
        let w = Workspaces::new(app.world_mut()).open("Code").entity();
        let document = open_document(
            app.world_mut(),
            w,
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
            let state = snapshot(app.world_mut(), w);
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
        assert_eq!(app.world().get::<InWorkspace>(bar).unwrap().0, w);
        app.world_mut().despawn(panel);
        install(app.world_mut());
        assert_eq!(
            Workbench::for_entity(app.world_mut(), w)
                .unwrap()
                .status_bar(),
            None
        );
        assert_eq!(snapshot(app.world_mut(), w).document, None);
    }
}
