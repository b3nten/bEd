//! One ordered system owns request definitions and network execution.
use crate::{
    MODULE_ID, OPEN_COMMAND, PANEL_ID, Shared, panel::HttpView, register_metadata, restore_catalog,
    saved_catalog, update_catalog, worker,
};
use bed_editor_api::{
    EditorFeatureSystems, EditorFrame, EditorIntents, EditorWorkspaceContribution,
    EditorWorkspaceRestore, Registry as EditorRegistry,
};
use bed_workbench_api::{
    Action, CommandInvocation, CommandResult, Panel, PanelCreation, PanelResult,
};
use bevy::prelude::*;
use dear_imgui_rs::Ui;
use serde_json::Value;

pub struct HttpRuntime {
    pub(crate) catalog: Shared,
    pub(crate) worker: worker::Worker,
    pub(crate) next_request: u64,
    pub(crate) next_run: u64,
    pub(crate) unrestored: Option<Value>,
}
impl Default for HttpRuntime {
    fn default() -> Self {
        Self {
            catalog: Shared::default(),
            worker: worker::Worker::new(),
            next_request: 0,
            next_run: 0,
            unrestored: None,
        }
    }
}
#[derive(Component)]
pub struct HttpSurface;
pub struct HttpPlugin;
impl Plugin for HttpPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<EditorFrame>()
            .init_resource::<EditorIntents>()
            .init_resource::<EditorRegistry>()
            .init_non_send::<HttpRuntime>()
            .add_systems(PreUpdate, prepare.in_set(EditorFeatureSystems::Prepare))
            .add_systems(Last, shutdown.after(bevy::app::OnAppExitSystems));
        app.world_mut()
            .resource_mut::<EditorRegistry>()
            .register_with(MODULE_ID, register_metadata)
            .expect("HTTP editor declarations are unique");
        let save = app.world_mut().register_system(save_workspace);
        let restore = app.world_mut().register_system(restore_workspace);
        app.world_mut().spawn(EditorWorkspaceContribution {
            owner: MODULE_ID,
            save,
            restore,
        });
        bed_workbench_api::register(app.world_mut(), MODULE_ID, |registrar| {
            registrar.panel(PANEL_ID, "HTTP Client", create);
            registrar.command(OPEN_COMMAND, "HTTP Client", open);
            registrar.command(crate::status::SHOW_COMMAND, "Show HTTP Client", show);
            registrar.command(
                crate::status::CANCEL_COMMAND,
                "Cancel HTTP request",
                cancel_run,
            );
            registrar.menu(OPEN_COMMAND);
        })
        .expect("HttpPlugin is installed once");
        app.world_mut().spawn(crate::status::items());
    }
}
fn shutdown(mut exits: MessageReader<AppExit>, mut runtime: NonSendMut<HttpRuntime>) {
    if exits.read().next().is_some() {
        runtime.worker.shutdown();
    }
}

fn open(In(invocation): In<CommandInvocation>) -> CommandResult {
    Ok(vec![Action::OpenPanel {
        panel_type: PANEL_ID.into(),
        state: Value::Null,
        area: Some(invocation.area),
    }])
}
fn show(
    In(invocation): In<CommandInvocation>,
    panels: Query<Entity, With<HttpSurface>>,
) -> CommandResult {
    let target = invocation.arguments["panel"]
        .as_u64()
        .and_then(Entity::try_from_bits)
        .or(invocation.source);
    let existing = target
        .filter(|entity| panels.contains(*entity))
        .or_else(|| panels.iter().min_by_key(|entity| entity.to_bits()));
    Ok(if let Some(entity) = existing {
        vec![Action::FocusPanel(entity)]
    } else {
        vec![Action::OpenPanel {
            panel_type: PANEL_ID.into(),
            state: Value::Null,
            area: Some(invocation.area),
        }]
    })
}
fn cancel_run(
    In(invocation): In<CommandInvocation>,
    mut runtime: NonSendMut<HttpRuntime>,
) -> CommandResult {
    let run = invocation.arguments["run"]
        .as_u64()
        .ok_or("HTTP cancellation requires a captured run ID")?;
    runtime
        .catalog
        .actions
        .push(crate::Action::CancelRun { run });
    Ok(Vec::new())
}
fn create(In(creation): In<PanelCreation>, world: &mut World) -> PanelResult {
    world.entity_mut(creation.entity).insert(bed_workbench_api::PanelDocumentation::new(&[
        ("Requests", "Create or select a request, then edit its method, URL, headers, and body."),
        ("Response", "Run sends the request; Cancel stops it. Inspect response headers and body, or use Copy to export the response."),
    ]));
    world.entity_mut(creation.entity).insert(HttpSurface);
    Ok(Box::new(HttpFrontend {
        entity: creation.entity,
        view: HttpView::new(&creation.state),
    }))
}
fn save_workspace(runtime: NonSend<HttpRuntime>) -> Value {
    saved_catalog(
        &runtime.catalog,
        runtime.next_request,
        runtime.unrestored.as_ref(),
    )
}
fn restore_workspace(
    In(payload): In<EditorWorkspaceRestore>,
    mut runtime: NonSendMut<HttpRuntime>,
) -> Result<(), String> {
    let HttpRuntime {
        catalog,
        worker,
        next_request,
        unrestored,
        ..
    } = &mut *runtime;
    restore_catalog(
        catalog,
        worker,
        next_request,
        unrestored,
        payload.state.as_ref(),
    );
    Ok(())
}
fn prepare(
    mut runtime: NonSendMut<HttpRuntime>,
    mut intents: ResMut<EditorIntents>,
    live: Query<Entity, With<HttpSurface>>,
    mut commands: Commands,
) {
    let HttpRuntime {
        catalog,
        worker,
        next_request,
        next_run,
        unrestored,
    } = &mut *runtime;
    let mut requests = Vec::new();
    update_catalog(
        catalog,
        worker,
        next_request,
        next_run,
        unrestored,
        &mut requests,
    );
    for request in requests {
        intents.push(None, request);
    }
    let error = catalog.error.as_ref().or_else(|| {
        let selected = catalog.selected?;
        let result = catalog.results.get(&selected)?;
        let request = catalog
            .requests
            .iter()
            .find(|request| request.id == selected)?;
        (crate::model::resolve(request).ok().as_ref() == Some(&result.request))
            .then(|| result.outcome.as_ref().err())
            .flatten()
    });
    let readiness = if catalog.active.is_some() {
        bed_editor_api::EditorPanelReadiness::Pending
    } else if let Some(error) = error {
        bed_editor_api::EditorPanelReadiness::Failed(error.clone())
    } else {
        bed_editor_api::EditorPanelReadiness::Ready
    };
    for entity in &live {
        commands.entity(entity).insert(readiness.clone());
    }
}

struct HttpFrontend {
    entity: Entity,
    view: HttpView,
}
impl Panel for HttpFrontend {
    fn title(&self, _: &World) -> String {
        "HTTP Client".into()
    }
    fn save(&self, _: &World) -> Value {
        self.view.saved_state()
    }
    fn draw(
        &mut self,
        ui: &Ui,
        world: &mut World,
        _: &mut Vec<Action>,
        _shortcuts: &bed_workbench_api::ShortcutInput,
    ) {
        self.view.persistent = !world.resource::<EditorFrame>().project_root.is_empty();
        let mut requests = Vec::new();
        {
            let mut runtime = world.non_send_mut::<HttpRuntime>();
            let actions = self.view.draw_catalog(ui, &runtime.catalog, &mut requests);
            runtime.catalog.actions.extend(actions);
        }
        for request in requests {
            world
                .resource_mut::<EditorIntents>()
                .push(Some(self.entity), request);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Action as HttpAction, model::Request};
    use bed_workbench::{WorkbenchPlugin, WorkbenchRequests, capture_command};
    use serde_json::json;

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins((WorkbenchPlugin, HttpPlugin));
        app
    }
    fn restore(app: &mut App, value: Value) {
        let handler = app
            .world_mut()
            .query::<&EditorWorkspaceContribution>()
            .single(app.world())
            .unwrap()
            .restore;
        app.world_mut()
            .run_system_with(
                handler,
                EditorWorkspaceRestore {
                    workspace: 1,
                    root: "/workspace".into(),
                    state: Some(value),
                },
            )
            .unwrap()
            .unwrap();
    }
    #[test]
    fn restoring_definitions_and_opening_frontends_never_runs_requests() {
        let mut app = app();
        restore(
            &mut app,
            json!({"version":1,"requests":[{"id":4,"name":"Restored","method":"GET","url":"http://127.0.0.1:1"}],"selected":4}),
        );
        let command = capture_command(app.world(), OPEN_COMMAND, Value::Null);
        app.world_mut()
            .resource_mut::<WorkbenchRequests>()
            .push(Action::Invoke(command));
        for _ in 0..3 {
            app.update();
        }
        let runtime = app.world().non_send::<HttpRuntime>();
        assert_eq!(runtime.catalog.requests.len(), 1);
        assert!(runtime.catalog.active.is_none());
        assert_eq!(runtime.next_run, 0);
        assert!(runtime.catalog.results.is_empty());
    }
    #[test]
    fn workspace_snapshot_captures_queued_edits_without_executing_run() {
        let mut app = app();
        let mut definition = Request::new(1);
        definition.url = "http://127.0.0.1:1".into();
        app.world_mut()
            .non_send_mut::<HttpRuntime>()
            .catalog
            .actions
            .extend([
                HttpAction::Create,
                HttpAction::Update {
                    request: definition.clone(),
                },
                HttpAction::Run { id: 1 },
            ]);
        let handler = app
            .world_mut()
            .query::<&EditorWorkspaceContribution>()
            .single(app.world())
            .unwrap()
            .save;
        let saved = app.world_mut().run_system(handler).unwrap();
        assert_eq!(saved["requests"][0]["url"], definition.url);
        assert_eq!(app.world().non_send::<HttpRuntime>().next_run, 0);
        assert!(
            app.world()
                .non_send::<HttpRuntime>()
                .catalog
                .active
                .is_none()
        );
    }
    #[test]
    fn unsupported_workspace_is_retained_until_explicit_catalog_creation() {
        let mut app = app();
        let value = json!({"version":91,"requests":["future"]});
        restore(&mut app, value.clone());
        app.update();
        let handler = app
            .world_mut()
            .query::<&EditorWorkspaceContribution>()
            .single(app.world())
            .unwrap()
            .save;
        assert_eq!(app.world_mut().run_system(handler).unwrap(), value);
        app.world_mut()
            .non_send_mut::<HttpRuntime>()
            .catalog
            .actions
            .push(HttpAction::Create);
        app.update();
        assert_eq!(app.world_mut().run_system(handler).unwrap()["version"], 1);
    }
}
