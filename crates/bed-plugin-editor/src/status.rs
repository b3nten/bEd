//! Editor-owned status facts; the workbench only arranges their widgets.
use crate::{
    DIAGNOSTICS_PANEL_TYPE, EditorRuntime, LSP_DASHBOARD_PANEL_TYPE, NavigationOutcomeKind,
    NavigationStage, SHOW_DIAGNOSTICS_COMMAND, SHOW_LSP_COMMAND, lsp::lsp_ui::ContextLspAction,
};
use bed_api::{
    documents::PanelBinding,
    features::{HostAction, HostEffects, HostFrame},
};
use bed_api::{
    status::{StatusContext, StatusDrawContext, StatusItem, StatusItems, StatusText},
    workbench::{CommandInvocation, CommandResult, WorkbenchAction},
};
use bed_documents::{DocumentId, DocumentSession, ViewId};
use bevy::prelude::*;
use dear_imgui_rs::Ui;
use serde_json::Value;

const NAVIGATION: &str = "bed.editor.navigation-status";
const LSP: &str = "bed.editor.language-server-status";
const DIAGNOSTICS: &str = "bed.editor.diagnostics-status";
const DOCUMENT: &str = "bed.editor.document-status";

pub(crate) fn install(app: &mut App) {
    app.world_mut().spawn(StatusItems(
        [
            (NAVIGATION, "Navigation", 300, "bed.editor.navigation"),
            (LSP, "Language servers", 200, "bed.editor.language-server"),
            (DIAGNOSTICS, "Diagnostics", 100, "bed.editor.diagnostics"),
            (DOCUMENT, "Document", 50, "bed.editor.document"),
        ]
        .map(|(id, label, priority, icon)| StatusItem {
            id,
            label,
            priority,
            icon: icon,
            prefer_icon: false,
            measure,
            draw,
            tooltip,
        })
        .to_vec(),
    ));
}

pub(crate) fn show_panel(
    In(invocation): In<CommandInvocation>,
    world: &mut World,
) -> CommandResult {
    let panel_type = match invocation.command.as_str() {
        SHOW_DIAGNOSTICS_COMMAND => DIAGNOSTICS_PANEL_TYPE,
        SHOW_LSP_COMMAND => LSP_DASHBOARD_PANEL_TYPE,
        _ => return Err("Unknown editor status command".into()),
    };
    world.resource_mut::<HostEffects>().push_at(
        invocation.source,
        invocation.area,
        HostAction::ShowPanel {
            panel_type: panel_type.into(),
            document: None,
            state: Value::Null,
            action: None,
        },
    );
    Ok(Vec::new())
}

fn measure(world: &World, ui: &Ui, context: StatusContext) -> Option<f32> {
    presentation(world, ui, context).map(|status| status.measure(ui))
}

fn draw(
    world: &mut World,
    ui: &Ui,
    context: StatusDrawContext,
    actions: &mut Vec<WorkbenchAction>,
) {
    if let Some(status) = presentation(world, ui, context.context) {
        status.draw(ui, context, actions);
    }
}

fn tooltip(world: &World, ui: &Ui, context: StatusContext) {
    if let Some(status) = presentation(world, ui, context) {
        status.tooltip(ui);
    }
}

fn presentation(world: &World, ui: &Ui, context: StatusContext) -> Option<StatusText> {
    let session = world.get_non_send::<DocumentSession>()?;
    match context.item_id {
        NAVIGATION => navigation(world, session, context),
        LSP => language_servers(session),
        DIAGNOSTICS => {
            let diagnostics = session.project_diagnostics();
            if diagnostics.errors == 0 && diagnostics.warnings == 0 && !diagnostics.checking {
                return None;
            }
            let mut summary = format!("E: {}  W: {}", diagnostics.errors, diagnostics.warnings);
            if diagnostics.checking {
                summary.push_str("  Checking workspace…");
            }
            let mut details = format!(
                "Workspace diagnostics: {} errors, {} warnings",
                diagnostics.errors, diagnostics.warnings
            );
            if diagnostics.stale {
                details.push_str("\nResults may be out of date while files have unsaved changes.");
            }
            if !diagnostics.status.is_empty() {
                details.push('\n');
                details.push_str(&diagnostics.status);
            }
            Some(
                StatusText::new(summary, details)
                    .command("Open Diagnostics", SHOW_DIAGNOSTICS_COMMAND),
            )
        }
        DOCUMENT => document(world, session, ui, context),
        _ => None,
    }
}

// The host's active document survives tool focus. A focused file-backed viewer
// deliberately supersedes that fallback even when it has no editor document.
fn displayed_document(
    world: &World,
    context: StatusContext,
) -> Option<(DocumentId, Option<ViewId>)> {
    if let Some(binding) = context
        .focused_frontend
        .or(context.focused_panel)
        .and_then(|entity| world.get::<PanelBinding>(entity))
        .filter(|binding| binding.document.is_some() || binding.input.local_file().is_some())
    {
        return binding.document.map(|document| (document, binding.view));
    }
    let frame = world.get_resource::<HostFrame>()?;
    frame
        .active_document
        .map(|document| (document, frame.active_view))
}

fn navigation(
    world: &World,
    session: &DocumentSession,
    context: StatusContext,
) -> Option<StatusText> {
    let runtime = world.get_non_send::<EditorRuntime>()?;
    let (document, _) = displayed_document(world, context)?;
    let label = |action| match action {
        ContextLspAction::Definition => "Go to definition",
        ContextLspAction::References => "Find references",
        ContextLspAction::Symbol => "Find symbol",
    };
    let (view, summary, error) = if let Some(activity) = runtime.navigation_activity() {
        let stage = match activity.stage {
            NavigationStage::WaitingForServer => "waiting for language server",
            NavigationStage::Requesting => "loading",
            NavigationStage::Retrying => "retrying",
        };
        (
            activity.view,
            format!("{}: {stage}…", label(activity.action)),
            false,
        )
    } else if let Some(outcome) = runtime.navigation_outcome() {
        let (summary, error) = match &outcome.kind {
            NavigationOutcomeKind::Empty => (
                match outcome.action {
                    ContextLspAction::Definition => "No definition found",
                    ContextLspAction::References => "No references found",
                    ContextLspAction::Symbol => "No symbol found",
                }
                .into(),
                false,
            ),
            NavigationOutcomeKind::Failed(message) => {
                (format!("{}: {message}", label(outcome.action)), true)
            }
            NavigationOutcomeKind::TimedOut => {
                (format!("{} timed out", label(outcome.action)), true)
            }
            NavigationOutcomeKind::Cancelled => return None,
        };
        (outcome.view, summary, error)
    } else {
        return None;
    };
    if session.document_for_view(view) != Some(document) {
        return None;
    }
    Some(StatusText {
        error,
        ..StatusText::new(summary, "").command("Open Language Servers", SHOW_LSP_COMMAND)
    })
}

fn language_servers(session: &DocumentSession) -> Option<StatusText> {
    let mut activity = Vec::new();
    let mut details = Vec::new();
    let mut error = false;
    if let Some(pool) = session.lsp() {
        if let Some(message) = pool.config_error() {
            error = true;
            activity.push("Language server configuration error".into());
            details.push(message.into());
        }
        if pool.is_discovering() {
            activity.push("Language servers: starting…".into());
            details.push("Discovering language server configuration and project roots…".into());
        }
        // These are in-memory clients. Rendering must never rediscover binaries
        // or project roots, nor start an otherwise idle server.
        for shared in pool.server_clients() {
            let client = shared.borrow();
            let server = client.current_server();
            if let Some(message) = client.last_error() {
                error = true;
                activity.push(format!("{server}: error"));
                details.push(format!("{server}: {message}"));
            } else if client.is_process_started() && !client.is_initialized() {
                activity.push(format!("{server}: starting…"));
                details.push(format!("{server}: starting language server…"));
            }
            for progress in client.progress().iter().filter(|job| !job.finished) {
                let mut summary = format!("{server}: {}", progress.title);
                if let Some(percentage) = progress.percentage {
                    summary.push_str(&format!(" ({percentage}%)"));
                }
                let mut detail = summary.clone();
                if let Some(message) = progress
                    .message
                    .as_ref()
                    .filter(|message| !message.is_empty())
                {
                    detail.push('\n');
                    detail.push_str(message);
                }
                activity.push(summary);
                details.push(detail);
            }
        }
    }
    let first = activity.first()?;
    let summary = if activity.len() > 1 {
        format!("{first} (+{})", activity.len() - 1)
    } else {
        first.clone()
    };
    Some(StatusText {
        error,
        ..StatusText::new(summary, details.join("\n\n"))
            .command("Open Language Servers", SHOW_LSP_COMMAND)
    })
}

fn document(
    world: &World,
    session: &DocumentSession,
    ui: &Ui,
    context: StatusContext,
) -> Option<StatusText> {
    let (document, view) = displayed_document(world, context)?;
    let language = session
        .with_document(document, |state| state.language_id.clone())
        .ok()?;
    let mut summary = String::new();
    if let Some(view) = view
        && let Ok(state) = session.view_snapshot(view)
    {
        let selection = state.primary();
        let column = session
            .with_document(document, |state| {
                use bed_editor_widgets::views::view_layout::{glyph_advance, line_column_x};
                let line = state.line(selection.head_row);
                (line_column_x(ui, &line, selection.head_column, 0.0) / glyph_advance(ui, " "))
                    .round() as i32
                    + 1
            })
            .ok()?;
        summary = format!("Ln {}, Col {column}", selection.head_row + 1);
    }
    if !language.is_empty() {
        if !summary.is_empty() {
            summary.push_str("    ");
        }
        summary.push_str(&language);
    }
    (!summary.is_empty()).then(|| StatusText::new(summary, ""))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EditorPlugin;

    fn context(publisher: Entity, item_id: &'static str, focused: Option<Entity>) -> StatusContext {
        StatusContext {
            publisher,
            item_id,
            focused_panel: focused,
            focused_frontend: focused,
            source: None,
            area: 7,
        }
    }

    fn imgui() -> dear_imgui_rs::Context {
        let mut context = dear_imgui_rs::Context::create();
        context
            .set_ini_filename(None::<std::path::PathBuf>)
            .unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        context
    }

    #[test]
    fn status_commands_capture_area_and_reveal_existing_tools() {
        let mut app = App::new();
        app.add_plugins(EditorPlugin);
        let publisher = app
            .world_mut()
            .query::<(Entity, &StatusItems)>()
            .single(app.world())
            .unwrap()
            .0;
        assert!(
            app.world()
                .get::<bed_api::workbench::PanelPresentation>(publisher)
                .is_none()
        );
        for (command, expected) in [
            (SHOW_DIAGNOSTICS_COMMAND, DIAGNOSTICS_PANEL_TYPE),
            (SHOW_LSP_COMMAND, LSP_DASHBOARD_PANEL_TYPE),
        ] {
            let WorkbenchAction::Invoke(invocation) =
                context(publisher, DIAGNOSTICS, None).invoke(command, Value::Null)
            else {
                unreachable!()
            };
            let handler = app
                .world()
                .resource::<bed_api::workbench::WorkbenchRegistry>()
                .command(command)
                .unwrap()
                .handler;
            app.world_mut()
                .run_system_with(handler, invocation)
                .unwrap()
                .unwrap();
            let requests = app.world_mut().resource_mut::<HostEffects>().take();
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0].area, Some(7));
            assert!(
                matches!(&requests[0].action, HostAction::ShowPanel { panel_type, .. } if panel_type == expected)
            );
        }
    }

    #[test]
    fn unicode_cursor_uses_last_document_but_nontext_viewers_hide_the_cursor() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut app = App::new();
        app.add_plugins(EditorPlugin);
        let (document, view) = {
            let mut session = app.world_mut().non_send_mut::<DocumentSession>();
            let document = session
                .create_snapshot_document("\tAé🦀\n".as_bytes(), "rs")
                .unwrap();
            let view = session.create_view(document).unwrap();
            session
                .with_commands(view, |commands| {
                    commands.set_cursor(
                        0,
                        8,
                        false,
                        bed_editing::text_commands::CursorReveal::Ensure,
                    )
                })
                .unwrap();
            (document, view)
        };
        let publisher = app
            .world_mut()
            .query::<(Entity, &StatusItems)>()
            .single(app.world())
            .unwrap()
            .0;
        let viewer = app
            .world_mut()
            .spawn(PanelBinding {
                document: Some(document),
                view: None,
                ..Default::default()
            })
            .id();
        {
            let mut frame = app.world_mut().resource_mut::<HostFrame>();
            frame.active_document = Some(document);
            frame.active_view = Some(view);
        }
        let mut imgui = imgui();
        imgui.prepare_frame(dear_imgui_rs::FramePrepareOptions::new(
            [800.0, 600.0],
            1.0 / 60.0,
        ));
        let ui = imgui.frame();
        ui.window("status test").build(|| {
            let text = presentation(app.world(), ui, context(publisher, DOCUMENT, None)).unwrap();
            assert_eq!(text.summary, "Ln 1, Col 8    rs");
            assert_eq!(
                presentation(app.world(), ui, context(publisher, DOCUMENT, Some(viewer)))
                    .unwrap()
                    .summary,
                "rs"
            );
            assert!(
                presentation(app.world(), ui, context(publisher, LSP, None)).is_none(),
                "idle LSPs stay quiet"
            );
            assert!(
                presentation(app.world(), ui, context(publisher, DIAGNOSTICS, None)).is_none(),
                "clear diagnostics stay quiet"
            );
        });
        let _ = imgui.render_legacy();
    }

    #[test]
    fn navigation_feedback_follows_its_live_document_and_survives_tool_focus() {
        let mut app = App::new();
        app.add_plugins(EditorPlugin);
        let mut runtime = EditorRuntime::default();
        let (first, second, view) = {
            let mut session = app.world_mut().non_send_mut::<DocumentSession>();
            let first = session.create_document(b"first").unwrap();
            let second = session.create_document(b"second").unwrap();
            let view = session.create_view(first).unwrap();
            runtime
                .request_lsp(&mut session, view, ContextLspAction::Definition, None)
                .unwrap();
            (first, second, view)
        };
        app.world_mut().insert_non_send(runtime);
        let publisher = app
            .world_mut()
            .query::<(Entity, &StatusItems)>()
            .single(app.world())
            .unwrap()
            .0;
        let tool = app.world_mut().spawn_empty().id();
        let context = context(publisher, NAVIGATION, Some(tool));
        app.world_mut().resource_mut::<HostFrame>().active_document = Some(first);
        assert_eq!(
            navigation(
                app.world(),
                app.world().non_send::<DocumentSession>(),
                context
            )
            .unwrap()
            .summary,
            "Go to definition: Language servers are unavailable for this workspace"
        );
        app.world_mut().resource_mut::<HostFrame>().active_document = Some(second);
        assert!(
            navigation(
                app.world(),
                app.world().non_send::<DocumentSession>(),
                context
            )
            .is_none()
        );
        app.world_mut().resource_mut::<HostFrame>().active_document = Some(first);
        app.world_mut()
            .non_send_mut::<DocumentSession>()
            .close_view(view, bed_documents::ClosePolicy::Discard)
            .unwrap();
        assert!(
            navigation(
                app.world(),
                app.world().non_send::<DocumentSession>(),
                context
            )
            .is_none(),
            "closed origins never retain feedback"
        );
    }

    #[test]
    fn cached_lsp_configuration_failure_is_visible_without_a_server_or_panel() {
        let directory = crate::test_support::TempDir::new();
        let config = directory.write("lsp.json", b"{");
        let mut session = DocumentSession::new();
        session
            .configure(bed_documents::SessionOptions {
                project_root: Some(directory.root().to_owned()),
                lsp_config: Some(config),
                lsp_config_mode: bed_documents::LspConfigMode::Snapshot,
                ..Default::default()
            })
            .unwrap();
        let text = language_servers(&session).unwrap();
        assert!(text.error);
        assert_eq!(text.summary, "Language server configuration error");
        assert!(text.details.contains("EOF"));
        assert_eq!(session.lsp().unwrap().server_clients().count(), 0);
    }
}
