//! Session status is available even when no debugger panel is open.
use super::{DebugPresentation, DebugState, LaunchBarrier};
use crate::{CONTINUE_COMMAND, PAUSE_COMMAND, SHOW_COMMAND, STOP_COMMAND};
use bedtime_api::{
    status::{StatusContext, StatusDrawContext, StatusItem, StatusItems, StatusText},
    workbench::WorkbenchAction,
};
use bedtime_debugger::SessionState;
use bevy::prelude::*;
use dear_imgui_rs::Ui;

const ACTIVE: &str = "bedtime.debug.session-status";
const CONTEXT: &str = "bedtime.debug.context-status";

pub(super) fn install(app: &mut App) {
    app.world_mut().spawn(StatusItems(
        [(ACTIVE, "Debug session", 250), (CONTEXT, "Debugger", 50)]
            .map(|(id, label, priority)| StatusItem {
                id,
                label,
                priority,
                icon: "debug",
                prefer_icon: false,
                measure,
                draw,
                tooltip,
            })
            .to_vec(),
    ));
}

fn measure(world: &World, ui: &Ui, context: StatusContext) -> Option<f32> {
    presentation(world, context).map(|status| status.measure(ui))
}

fn draw(
    world: &mut World,
    ui: &Ui,
    context: StatusDrawContext,
    actions: &mut Vec<WorkbenchAction>,
) {
    if let Some(status) = presentation(world, context.context) {
        status.draw(ui, context, actions);
    }
}

fn tooltip(world: &World, ui: &Ui, context: StatusContext) {
    if let Some(status) = presentation(world, context) {
        status.tooltip(ui);
    }
}

fn presentation(world: &World, context: StatusContext) -> Option<StatusText> {
    let debugger = &world.get_non_send::<DebugState>()?.0;
    let saving = world
        .get_resource::<LaunchBarrier>()
        .is_some_and(|barrier| barrier.0.is_some());
    let active =
        saving || debugger.active() || debugger.discovery.is_some() || debugger.error.is_some();
    if context.item_id == ACTIVE {
        if !active {
            return None;
        }
    } else if context.item_id == CONTEXT {
        if active
            || !context
                .focused_frontend
                .or(context.focused_panel)
                .is_some_and(|entity| world.get::<DebugPresentation>(entity).is_some())
            || world
                .get_resource::<crate::DebugTarget>()
                .is_none_or(|target| {
                    target.remote
                        || target.directory.is_empty()
                        || !cfg!(any(target_os = "macos", target_os = "linux"))
                })
        {
            return None;
        }
    } else {
        return None;
    }
    let state = if debugger.error.is_some() {
        "Debugger error".into()
    } else if saving {
        "Saving before launch…".into()
    } else if debugger.build.is_some() {
        "Building…".into()
    } else if debugger.discovery.is_some() {
        "Discovering debug targets…".into()
    } else if let Some(session) = &debugger.session {
        match session.state {
            SessionState::Initializing | SessionState::Launching => "Starting debugger…".into(),
            SessionState::Running => "Running".into(),
            SessionState::Stopped => format!("Paused: {}", session.stop_reason),
            SessionState::Stopping => "Stopping…".into(),
            SessionState::Terminated => format!(
                "Exited{}",
                session
                    .exit_code
                    .map(|code| format!(" ({code})"))
                    .unwrap_or_default()
            ),
            SessionState::Failed => "Debugger failed".into(),
        }
    } else {
        "Ready".into()
    };
    let profile = debugger
        .launch_profile
        .as_ref()
        .or_else(|| debugger.profiles.get(debugger.selected))
        .map(|profile| profile.name.as_str())
        .filter(|name| !name.is_empty());
    let summary = match profile {
        Some(profile) => format!("{profile}: {state}"),
        None => format!("Debug: {state}"),
    };
    let mut details = Vec::new();
    if let Some(error) = &debugger.error {
        details.push(error.clone());
    }
    if let Some(notice) = &debugger.source_notice {
        details.push(notice.clone());
    }
    if let Some(session) = &debugger.session
        && let Some(frame) = session
            .frames
            .iter()
            .find(|frame| Some(frame.id) == session.selected_frame)
    {
        details.push(format!(
            "{}\n{}:{}",
            frame.name,
            frame.source.as_deref().unwrap_or("Unknown source"),
            frame.line
        ));
    }
    let mut text = StatusText {
        error: debugger.error.is_some()
            || debugger
                .session
                .as_ref()
                .is_some_and(|session| session.state == SessionState::Failed),
        ..StatusText::new(summary, details.join("\n\n"))
            .command("Open Debug", SHOW_COMMAND)
            .command("Continue", CONTINUE_COMMAND)
            .command("Pause", PAUSE_COMMAND)
            .command("Stop", STOP_COMMAND)
    };
    text.commands[1].enabled = debugger
        .session
        .as_ref()
        .is_some_and(|session| session.state == SessionState::Stopped);
    text.commands[2].enabled = debugger
        .session
        .as_ref()
        .is_some_and(|session| session.state == SessionState::Running);
    text.commands[3].enabled = saving || debugger.active();
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DebuggerPlugin;
    use serde_json::Value;

    fn context(publisher: Entity, item_id: &'static str, focused: Option<Entity>) -> StatusContext {
        StatusContext {
            publisher,
            item_id,
            focused_panel: focused,
            focused_frontend: focused,
            source: None,
            area: 3,
        }
    }

    #[test]
    fn debug_errors_publish_without_a_panel_and_idle_status_requires_debug_focus() {
        let mut app = App::new();
        app.add_plugins(DebuggerPlugin);
        let publisher = app
            .world_mut()
            .query::<(Entity, &StatusItems)>()
            .single(app.world())
            .unwrap()
            .0;
        assert!(presentation(app.world(), context(publisher, ACTIVE, None)).is_none());
        assert!(presentation(app.world(), context(publisher, CONTEXT, None)).is_none());
        let panel = app.world_mut().spawn(DebugPresentation).id();
        assert!(
            presentation(app.world(), context(publisher, CONTEXT, Some(panel))).is_none(),
            "a projectless debugger only has its panel placeholder"
        );
        app.world_mut()
            .resource_mut::<crate::DebugTarget>()
            .directory = "/workspace".into();
        let ready = presentation(app.world(), context(publisher, CONTEXT, Some(panel))).unwrap();
        assert_eq!(ready.summary, "Debug: Ready");
        assert!(
            ready
                .commands
                .iter()
                .skip(1)
                .all(|command| !command.enabled)
        );
        app.world_mut().non_send_mut::<DebugState>().0.error = Some("Adapter disconnected".into());
        let failed = presentation(app.world(), context(publisher, ACTIVE, None)).unwrap();
        assert!(failed.error);
        assert!(failed.details.contains("Adapter disconnected"));
        assert!(presentation(app.world(), context(publisher, CONTEXT, Some(panel))).is_none());
        let WorkbenchAction::Invoke(command) = context(publisher, ACTIVE, None)
            .invoke(failed.commands[0].command.clone(), Value::Null)
        else {
            unreachable!()
        };
        assert_eq!(command.command, SHOW_COMMAND);
        assert_eq!(command.source, None);
        assert_eq!(command.area, 3);
        assert!(
            app.world()
                .get::<bedtime_api::workbench::PanelPresentation>(publisher)
                .is_none()
        );
    }

    #[test]
    fn queued_hover_from_a_closed_document_or_previous_workspace_has_no_error_status() {
        use bedtime_documents::session::{ClosePolicy, DocumentSession};
        use bedtime_editor_widgets::{
            extensions::SourceHoverObservation,
            source_intents::{SourceHoverRequest, SourceRequests},
            views::hover_trigger::{Info, Zone},
        };

        for replace_workspace in [false, true] {
            let mut app = App::new();
            app.add_plugins(DebuggerPlugin);
            let publisher = app
                .world_mut()
                .query::<(Entity, &StatusItems)>()
                .single(app.world())
                .unwrap()
                .0;
            let (document, view, revision) = {
                let mut documents = app.world_mut().non_send_mut::<DocumentSession>();
                let document = documents.create_document(b"let value = 1;").unwrap();
                let view = documents.create_view(document).unwrap();
                let revision = documents.document_revision(document).unwrap();
                (document, view, revision)
            };
            app.world_mut()
                .non_send_mut::<SourceRequests>()
                .hover
                .push(SourceHoverRequest {
                    document,
                    view,
                    revision,
                    observation: SourceHoverObservation {
                        info: Info {
                            active: true,
                            zone: Zone::Text,
                            row: 0,
                            column: 4,
                        },
                        dismissed: false,
                        key_pressed: false,
                        in_pane: true,
                        over_popup: false,
                        mouse: [20.0, 20.0],
                        frame: 1,
                        window_size: [600.0, 400.0],
                    },
                });
            if replace_workspace {
                app.world_mut().insert_non_send(DocumentSession::new());
            } else {
                app.world_mut()
                    .non_send_mut::<DocumentSession>()
                    .close_document(document, ClosePolicy::Discard)
                    .unwrap();
            }
            app.update();

            assert!(app.world().non_send::<SourceRequests>().hover.is_empty());
            assert!(presentation(app.world(), context(publisher, ACTIVE, None)).is_none());
        }
    }

    #[test]
    fn launch_save_barrier_has_ambient_feedback_and_its_existing_stop_command_cancels_it() {
        let mut app = App::new();
        app.add_plugins(DebuggerPlugin);
        let publisher = app
            .world_mut()
            .query::<(Entity, &StatusItems)>()
            .single(app.world())
            .unwrap()
            .0;
        let context = context(publisher, ACTIVE, None);
        let WorkbenchAction::Invoke(invocation) = context.invoke(crate::START_COMMAND, Value::Null)
        else {
            unreachable!()
        };
        let workspace = app
            .world()
            .non_send::<bedtime_documents::DocumentSession>()
            .session_id();
        app.world_mut().resource_mut::<LaunchBarrier>().0 = Some(super::super::PendingLaunch {
            invocation,
            action: crate::controller::DebuggerAction::Start,
            token: bedtime_api::documents::SaveToken::next(),
            documents: Vec::new(),
            session_id: workspace,
            target: super::super::DebugTarget::default(),
            result: None,
        });
        let text = presentation(app.world(), context).unwrap();
        assert_eq!(text.summary, "Debug: Saving before launch…");
        let stop = text
            .commands
            .iter()
            .find(|command| command.command == STOP_COMMAND)
            .unwrap();
        assert!(stop.enabled);
        let WorkbenchAction::Invoke(invocation) = context.invoke(stop.command.clone(), Value::Null)
        else {
            unreachable!()
        };
        let handler = app
            .world()
            .resource::<bedtime_api::workbench::WorkbenchRegistry>()
            .command(&bedtime_api::CommandId::from(STOP_COMMAND))
            .unwrap()
            .handler;
        app.world_mut()
            .run_system_with(handler, invocation)
            .unwrap()
            .unwrap();
        assert!(app.world().resource::<LaunchBarrier>().0.is_none());
        assert!(presentation(app.world(), context).is_none());
    }
}
