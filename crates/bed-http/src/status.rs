use crate::{HttpRuntime, HttpSurface, model};
use bed_workbench_api::{
    Action, StatusCommand, StatusContext, StatusDrawContext, StatusItem, StatusItems, StatusText,
};
use bevy::prelude::*;
use dear_imgui_rs::Ui;

pub(crate) const SHOW_COMMAND: &str = "bed.http.show";
pub(crate) const CANCEL_COMMAND: &str = "bed.http.cancel-run";
pub(crate) fn items() -> StatusItems {
    StatusItems(vec![
        StatusItem {
            id: "bed.http.activity",
            label: "HTTP request",
            priority: 150,
            icon: None,
            prefer_icon: false,
            measure,
            draw,
            tooltip,
        },
        StatusItem {
            id: "bed.http.result",
            label: "HTTP response",
            priority: 50,
            icon: None,
            prefer_icon: false,
            measure,
            draw,
            tooltip,
        },
    ])
}
pub(super) fn presentation(world: &World, context: StatusContext) -> Option<StatusText> {
    let catalog = &world.get_non_send::<HttpRuntime>()?.catalog;
    let show = StatusCommand {
        label: "Show HTTP Client",
        command: SHOW_COMMAND,
        arguments: context
            .focused_frontend
            .filter(|entity| world.get::<HttpSurface>(*entity).is_some())
            .map_or(
                serde_json::Value::Null,
                |entity| serde_json::json!({"panel": entity.to_bits()}),
            ),
        enabled: true,
    };
    if context.item_id == "bed.http.activity" {
        let active = catalog.active.as_ref()?;
        let name = catalog
            .requests
            .iter()
            .find(|request| request.id == active.id)
            .map(|request| request.name.as_str())
            .unwrap_or("request");
        let mut text = StatusText::new(
            format!("Running {} {name}…", active.request.method),
            format!("{} {}", active.request.method, active.request.url),
        );
        text.commands.push(show);
        text.commands.push(StatusCommand {
            label: "Cancel",
            command: CANCEL_COMMAND,
            arguments: serde_json::json!({"run": active.run}),
            enabled: true,
        });
        return Some(text);
    }
    if catalog.active.is_some()
        || !context
            .focused_frontend
            .is_some_and(|entity| world.get::<HttpSurface>(entity).is_some())
    {
        return None;
    }
    let id = catalog.selected?;
    let request = catalog
        .actions
        .iter()
        .rev()
        .find_map(|action| match action {
            crate::Action::Update { request } if request.id == id => Some(request),
            _ => None,
        })
        .or_else(|| catalog.requests.iter().find(|request| request.id == id))?;
    let result = catalog.results.get(&id)?;
    let stale = model::resolve(request).ok().as_ref() != Some(&result.request);
    let prefix = if stale { "Previous response · " } else { "" };
    let mut text = match &result.outcome {
        Ok(response) => StatusText::new(
            format!(
                "{prefix}{} · {:.0} ms · {} bytes{}",
                response.status,
                response.elapsed.as_secs_f64() * 1000.0,
                response.bytes.len(),
                if response.truncated {
                    " · truncated"
                } else {
                    ""
                }
            ),
            format!(
                "{}\n{} {}\n{}{}",
                request.name,
                response.version,
                response.status,
                response.final_url,
                if response.truncated {
                    "\nResponse collection stopped at 8 MiB."
                } else {
                    ""
                }
            ),
        ),
        Err(error) => {
            let mut text = StatusText::new(
                format!("{prefix}HTTP request failed"),
                format!("{}\n{error}", request.name),
            );
            text.error = true;
            text
        }
    };
    if stale {
        text.details
            .push_str("\nRequest inputs changed after this response.");
    }
    text.commands.push(show);
    Some(text)
}
fn measure(world: &World, ui: &Ui, context: StatusContext) -> Option<f32> {
    Some(presentation(world, context)?.measure(ui))
}
fn draw(world: &mut World, ui: &Ui, context: StatusDrawContext, actions: &mut Vec<Action>) {
    if let Some(text) = presentation(world, context.context) {
        text.draw(ui, context, actions);
    }
}
fn tooltip(world: &World, ui: &Ui, context: StatusContext) {
    if let Some(text) = presentation(world, context) {
        text.tooltip(ui);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ActiveRun, RunResult,
        worker::{BodyPresentation, Response},
    };
    use std::time::Duration;

    fn setup() -> (App, StatusContext, model::ResolvedRequest) {
        let mut app = App::new();
        app.add_plugins(crate::HttpPlugin);
        let publisher = app.world_mut().spawn_empty().id();
        let panel = app.world_mut().spawn(HttpSurface).id();
        let mut request = model::Request::new(1);
        request.name = "Health check".into();
        request.url = "http://127.0.0.1:1/health".into();
        let resolved = model::resolve(&request).unwrap();
        let mut runtime = app.world_mut().non_send_mut::<HttpRuntime>();
        runtime.catalog.requests.push(request);
        runtime.catalog.selected = Some(1);
        let context = StatusContext {
            publisher,
            item_id: "bed.http.result",
            focused_panel: None,
            focused_frontend: Some(panel),
            source: None,
            area: 3,
        };
        (app, context, resolved)
    }

    #[test]
    fn selected_response_tracks_focus_truncation_and_unapplied_draft_edits() {
        let (mut app, context, request) = setup();
        app.world_mut()
            .non_send_mut::<HttpRuntime>()
            .catalog
            .results
            .insert(
                1,
                RunResult {
                    run: 7,
                    request,
                    outcome: Ok(Response {
                        status: 200,
                        version: "HTTP/1.1".into(),
                        final_url: "http://127.0.0.1:1/health".into(),
                        elapsed: Duration::from_millis(25),
                        headers: Vec::new(),
                        bytes: vec![0; 32],
                        truncated: true,
                        presentation: BodyPresentation::Text {
                            raw: "body".into(),
                            pretty_json: None,
                        },
                    }),
                },
            );
        let text = presentation(app.world(), context).unwrap();
        assert_eq!(text.summary, "200 · 25 ms · 32 bytes · truncated");
        assert!(text.details.contains("8 MiB"));
        let original = context.focused_frontend.unwrap();
        app.world_mut().spawn(HttpSurface);
        let command = &text.commands[0];
        let handler = app
            .world()
            .resource::<bed_workbench_api::Registry>()
            .command(SHOW_COMMAND)
            .unwrap()
            .handler;
        let actions = app
            .world_mut()
            .run_system_with(
                handler,
                bed_workbench_api::CommandInvocation {
                    command: SHOW_COMMAND.into(),
                    source: None,
                    area: 3,
                    arguments: command.arguments.clone(),
                },
            )
            .unwrap()
            .unwrap();
        assert!(
            matches!(actions.as_slice(), [Action::FocusPanel(entity)] if *entity == original),
            "status commands retain the HTTP frontend that supplied the result"
        );
        assert!(
            presentation(
                app.world(),
                StatusContext {
                    focused_frontend: None,
                    ..context
                }
            )
            .is_none()
        );
        let mut changed = app.world().non_send::<HttpRuntime>().catalog.requests[0].clone();
        changed.body = "new inputs".into();
        app.world_mut()
            .non_send_mut::<HttpRuntime>()
            .catalog
            .actions
            .push(crate::Action::Update { request: changed });
        assert!(
            presentation(app.world(), context)
                .unwrap()
                .summary
                .starts_with("Previous response")
        );
        assert_eq!(
            app.world().non_send::<HttpRuntime>().next_run,
            0,
            "status inspection cannot send requests"
        );
    }

    #[test]
    fn running_cancel_is_bound_to_the_captured_run_and_show_focuses_existing_panel() {
        let (mut app, context, request) = setup();
        app.world_mut().non_send_mut::<HttpRuntime>().catalog.active = Some(ActiveRun {
            run: 7,
            id: 1,
            request,
        });
        let context = StatusContext {
            item_id: "bed.http.activity",
            focused_frontend: None,
            ..context
        };
        let text = presentation(app.world(), context).unwrap();
        assert_eq!(text.summary, "Running GET Health check…");
        let cancel = text
            .commands
            .iter()
            .find(|command| command.command == CANCEL_COMMAND)
            .unwrap();
        let handler = app
            .world()
            .resource::<bed_workbench_api::Registry>()
            .command(CANCEL_COMMAND)
            .unwrap()
            .handler;
        app.world_mut()
            .run_system_with(
                handler,
                bed_workbench_api::CommandInvocation {
                    command: CANCEL_COMMAND.into(),
                    source: None,
                    area: 3,
                    arguments: cancel.arguments.clone(),
                },
            )
            .unwrap()
            .unwrap();
        app.world_mut()
            .non_send_mut::<HttpRuntime>()
            .catalog
            .active
            .as_mut()
            .unwrap()
            .run = 8;
        app.update();
        assert_eq!(
            app.world()
                .non_send::<HttpRuntime>()
                .catalog
                .active
                .as_ref()
                .unwrap()
                .run,
            8,
            "an old cancellation must not cancel the next request"
        );
        let cancel = presentation(app.world(), context)
            .unwrap()
            .commands
            .into_iter()
            .find(|command| command.command == CANCEL_COMMAND)
            .unwrap();
        app.world_mut()
            .run_system_with(
                handler,
                bed_workbench_api::CommandInvocation {
                    command: CANCEL_COMMAND.into(),
                    source: None,
                    area: 3,
                    arguments: cancel.arguments,
                },
            )
            .unwrap()
            .unwrap();
        app.update();
        assert!(
            app.world()
                .non_send::<HttpRuntime>()
                .catalog
                .active
                .is_none()
        );
        assert!(presentation(app.world(), context).is_none());
        let show = app
            .world()
            .resource::<bed_workbench_api::Registry>()
            .command(SHOW_COMMAND)
            .unwrap()
            .handler;
        let actions = app
            .world_mut()
            .run_system_with(
                show,
                bed_workbench_api::CommandInvocation {
                    command: SHOW_COMMAND.into(),
                    source: None,
                    area: 3,
                    arguments: serde_json::Value::Null,
                },
            )
            .unwrap()
            .unwrap();
        assert!(
            matches!(actions.as_slice(), [Action::FocusPanel(entity)] if app.world().get::<HttpSurface>(*entity).is_some())
        );
        assert_eq!(app.world().non_send::<HttpRuntime>().next_run, 0);
    }
}
