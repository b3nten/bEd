//! The JSON feature runs in the host world. Frontends retain only their entity.
use crate::{
    JsonPanel, OPEN_COMMAND, OPEN_FILE_COMMAND, PANEL_ID, PLUGIN_ID, VIEWER_ID, register_metadata,
    supported_path,
};
use bed_editing::identity::DocumentId;
use bed_editor_api::{
    EditorFeatureSystems, EditorFrame, EditorIntents, EditorPanelActions, EditorPanelBinding,
    HostRequest, PanelActionInvocation, PanelActionResult, PanelActionSystem,
    Registry as EditorRegistry, panel_viewer_menu,
};
use bed_workbench_api::{
    Action, CommandInvocation, CommandResult, Panel, PanelCreation, PanelResult,
};
use bevy::{ecs::system::SystemState, prelude::*};
use dear_imgui_rs::Ui;
use serde_json::Value;
use std::collections::HashMap;

#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct JsonDocument(pub DocumentId);

/// Main-thread workers and their current tree presentations belong to the app.
#[derive(Default)]
pub struct JsonStates(HashMap<Entity, JsonPanel>);

impl JsonStates {
    pub fn get(&self, entity: Entity) -> Option<&JsonPanel> {
        self.0.get(&entity)
    }
}

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum JsonSystems {
    Poll,
}

pub struct JsonPlugin;

#[derive(Resource)]
struct JsonActionHandler(PanelActionSystem);

impl Plugin for JsonPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<EditorFrame>()
            .init_resource::<EditorIntents>()
            .init_resource::<EditorRegistry>()
            .init_non_send::<JsonStates>()
            .add_systems(
                PreUpdate,
                poll_json
                    .in_set(JsonSystems::Poll)
                    .in_set(EditorFeatureSystems::Prepare),
            )
            .add_systems(Last, shutdown.after(bevy::app::OnAppExitSystems));
        app.world_mut()
            .resource_mut::<EditorRegistry>()
            .register_with(PLUGIN_ID, register_metadata)
            .expect("JSON declarations must register exactly once");
        let action = app.world_mut().register_system(json_action);
        app.insert_resource(JsonActionHandler(action));
        bed_workbench_api::register(app.world_mut(), PLUGIN_ID, |registrar| {
            registrar.panel(PANEL_ID, "JSON Tree", create_json_panel);
            registrar.command(OPEN_COMMAND, "Open JSON/JSONC…", json_command);
            registrar.command(OPEN_FILE_COMMAND, "Open in JSON Tree", json_command);
            registrar.menu(OPEN_COMMAND);
        })
        .expect("JSON systems must register exactly once");
    }
}

fn shutdown(mut exits: MessageReader<bevy::app::AppExit>, mut states: NonSendMut<JsonStates>) {
    if exits.read().next().is_some() {
        states.0.clear();
    }
}

fn create_json_panel(In(creation): In<PanelCreation>, world: &mut World) -> PanelResult {
    world.entity_mut(creation.entity).insert(bed_workbench_api::PanelDocumentation::new(&[
        ("Tree", "Expand objects and arrays to explore JSON or JSONC. Select values to inspect their contents."),
        ("Source", "Edit as Text opens the shared source document. Undo and redo use the document’s edit history."),
    ]).shortcut_actions(&["bed.edit.undo", "bed.edit.redo"]));
    if world
        .get::<EditorPanelBinding>(creation.entity)
        .is_some_and(|binding| binding.input.local_file().is_some())
    {
        return Err("JSON Tree requires an editor document".into());
    }
    let document = world
        .get::<EditorPanelBinding>(creation.entity)
        .and_then(|binding| binding.input.document().or(binding.document))
        .or_else(|| creation.state["document"].as_u64().map(DocumentId));
    if let Some(document) = document {
        let action_system = world.resource::<JsonActionHandler>().0;
        world
            .entity_mut(creation.entity)
            .insert((JsonDocument(document), EditorPanelActions(action_system)));
        world
            .non_send_mut::<JsonStates>()
            .0
            .insert(creation.entity, JsonPanel::new(document, &creation.state));
    }
    Ok(Box::new(JsonFrontend {
        entity: creation.entity,
    }))
}

fn poll_json(
    frame: Res<EditorFrame>,
    mut states: NonSendMut<JsonStates>,
    live: Query<&JsonDocument>,
    mut commands: Commands,
) {
    // Removing a panel invalidates its bounded worker and releases its tree.
    states.0.retain(|entity, _| live.get(*entity).is_ok());
    let host = frame.context();
    for (entity, panel) in &mut states.0 {
        panel.sync(&host);
        commands.entity(*entity).insert(match panel.error() {
            Some(error) => bed_editor_api::EditorPanelReadiness::Failed(error.into()),
            None if panel.is_ready() => bed_editor_api::EditorPanelReadiness::Ready,
            None => bed_editor_api::EditorPanelReadiness::Pending,
        });
    }
}

fn json_command(
    In(invocation): In<CommandInvocation>,
    frame: Res<EditorFrame>,
    bindings: Query<&EditorPanelBinding>,
    mut intents: ResMut<EditorIntents>,
) -> CommandResult {
    let request = match invocation.command.as_str() {
        OPEN_COMMAND => Some(HostRequest::OpenFileDialog {
            viewer: Some(VIEWER_ID.into()),
        }),
        OPEN_FILE_COMMAND => invocation.arguments["path"]
            .as_str()
            .or_else(|| {
                let entity = invocation.source?;
                let binding = bindings.get(entity).ok()?;
                let document = binding.input.document().or(binding.document)?;
                frame
                    .documents
                    .iter()
                    .find(|entry| entry.id == document)
                    .map(|document| document.path.as_str())
            })
            .filter(|path| supported_path(path))
            .map(|path| HostRequest::OpenFile {
                path: path.to_owned(),
                viewer: Some(VIEWER_ID.into()),
            }),
        _ => return Err(format!("Unknown JSON command: {}", invocation.command)),
    };
    if let Some(request) = request {
        intents.push_at(invocation.source, invocation.area, request);
    }
    Ok(Vec::new())
}

fn json_action(
    In(invocation): In<PanelActionInvocation>,
    mut states: NonSendMut<JsonStates>,
    live: Query<&JsonDocument>,
    mut intents: ResMut<EditorIntents>,
) -> PanelActionResult {
    if live.get(invocation.entity).is_err() {
        return Ok(false);
    }
    let Some(panel) = states.0.get_mut(&invocation.entity) else {
        return Ok(false);
    };
    let mut requests = Vec::new();
    let handled = panel.apply_action(invocation.action, &mut requests)?;
    for request in requests {
        intents.push(Some(invocation.entity), request);
    }
    Ok(handled)
}

struct JsonFrontend {
    entity: Entity,
}

impl Panel for JsonFrontend {
    fn title(&self, world: &World) -> String {
        world.non_send::<JsonStates>().get(self.entity).map_or_else(
            || "JSON Tree".into(),
            |panel| panel.panel_title(&world.resource::<EditorFrame>().context()),
        )
    }

    fn draw(
        &mut self,
        ui: &Ui,
        world: &mut World,
        _requests: &mut Vec<Action>,
        _shortcuts: &bed_workbench_api::ShortcutInput,
    ) {
        let menu = panel_viewer_menu(world, self.entity, VIEWER_ID);
        let mut state = SystemState::<(
            Res<EditorFrame>,
            NonSendMut<JsonStates>,
            ResMut<EditorIntents>,
        )>::new(world);
        let (frame, mut states, mut intents) = state
            .get_mut(world)
            .expect("JSON plugin initialized its presentation resources");
        let mut requests = Vec::new();
        if let Some(panel) = states.0.get_mut(&self.entity) {
            let mut host = frame.context();
            host.viewer_menu = menu.as_ref();
            panel.draw_prepared(ui, &host, &mut requests);
        } else {
            ui.text_wrapped("Open a JSON or JSONC file to view its structure.");
            if ui.button("Open JSON/JSONC…") {
                requests.push(HostRequest::OpenFileDialog {
                    viewer: Some(VIEWER_ID.into()),
                });
            }
        }
        // Publish only after drawing succeeds; panel quarantine discards partial
        // intentions if its frontend panics.
        for request in requests {
            intents.push(Some(self.entity), request);
        }
    }

    fn save(&self, world: &World) -> Value {
        world
            .non_send::<JsonStates>()
            .get(self.entity)
            .map_or(Value::Null, JsonPanel::saved_state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bed_editor_api::{DocumentKind, PanelAction, PanelInput, PluginDocument};
    use bed_workbench_api::{PanelOwner, PanelPresentation};
    use serde_json::json;
    use std::{
        sync::Arc,
        thread,
        time::{Duration, Instant},
    };

    fn document(id: u64, path: &str, source: &str, revision: u64) -> PluginDocument {
        PluginDocument {
            id: DocumentId(id),
            path: path.into(),
            kind: DocumentKind::Text,
            language_id: "json".into(),
            revision: (1, revision),
            dirty: true,
            bytes: Arc::from(source.as_bytes()),
            text: None,
        }
    }

    fn create(
        app: &mut App,
        document: Option<DocumentId>,
        state: Value,
    ) -> (Entity, Box<dyn Panel>) {
        let world = app.world_mut();
        let entity = world
            .spawn((
                PanelPresentation::new(7),
                EditorPanelBinding {
                    input: document.map(PanelInput::Document).unwrap_or_default(),
                    document,
                    viewer: Some(VIEWER_ID.into()),
                    workspace: 3,
                    ..Default::default()
                },
            ))
            .id();
        let factory = world
            .resource::<bed_workbench_api::Registry>()
            .panel(PANEL_ID)
            .unwrap()
            .factory;
        let frontend = world
            .run_system_with(factory, PanelCreation { entity, state })
            .unwrap()
            .unwrap();
        (entity, frontend)
    }

    fn wait_ready(app: &mut App, entity: Entity) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            app.update();
            if app
                .world()
                .non_send::<JsonStates>()
                .get(entity)
                .unwrap()
                .is_ready()
            {
                return;
            }
            assert!(Instant::now() < deadline, "JSON preparation did not finish");
            thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn world_prepares_unsaved_revisions_without_drawing_and_releases_closed_panels() {
        let mut app = App::new();
        app.add_plugins(JsonPlugin);
        app.world_mut().resource_mut::<EditorFrame>().documents =
            vec![document(4, "source.json", r#"{"nested":{"value":1}}"#, 1)];
        let (entity, frontend) = create(
            &mut app,
            Some(DocumentId(4)),
            json!({"scroll": [-5.0, 20.0], "expanded": ["", "/p:nested:0"]}),
        );
        wait_ready(&mut app, entity);
        assert_eq!(frontend.title(app.world()), "source.json");
        assert_eq!(frontend.save(app.world())["scroll"], json!([0.0, 20.0]));

        app.world_mut().resource_mut::<EditorFrame>().documents[0] =
            document(4, "renamed.JSONC", "[1,]", 2);
        wait_ready(&mut app, entity);
        assert_eq!(frontend.title(app.world()), "renamed.JSONC");
        assert!(
            app.world()
                .non_send::<JsonStates>()
                .get(entity)
                .unwrap()
                .error()
                .is_none()
        );

        app.world_mut().resource_mut::<EditorFrame>().documents[0] =
            document(4, "renamed.JSONC", "{", 3);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            app.update();
            if let Some(error) = app
                .world()
                .non_send::<JsonStates>()
                .get(entity)
                .unwrap()
                .error()
            {
                assert!(error.contains("line 1"));
                break;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        }
        assert!(
            !app.world()
                .non_send::<JsonStates>()
                .get(entity)
                .unwrap()
                .is_ready()
        );

        app.world_mut().despawn(entity);
        app.update();
        assert!(app.world().non_send::<JsonStates>().get(entity).is_none());
    }

    #[test]
    fn edit_as_text_switches_the_retained_frontends_owning_tab() {
        let _lock = crate::IMGUI_TEST_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut app = App::new();
        app.add_plugins(JsonPlugin);
        app.world_mut()
            .resource_mut::<EditorRegistry>()
            .register_with("fixture", |registrar| {
                registrar.panel("fixture.text-panel", "Text");
                registrar.fallback_viewer(
                    "fixture.text",
                    "Text Editor",
                    "fixture.text-panel",
                    DocumentKind::Text,
                    &[],
                );
            })
            .unwrap();
        app.world_mut().resource_mut::<EditorFrame>().documents = vec![
            document(4, "origin.json", "{", 1),
            document(5, "focused.json", "{}", 1),
        ];
        app.world_mut()
            .resource_mut::<EditorFrame>()
            .active_document = Some(DocumentId(5));
        let (entity, mut frontend) = create(&mut app, Some(DocumentId(4)), Value::Null);
        let owner = app.world_mut().spawn(PanelPresentation::new(37)).id();
        app.world_mut()
            .entity_mut(entity)
            .remove::<PanelPresentation>()
            .insert(PanelOwner(owner));
        let deadline = Instant::now() + Duration::from_secs(5);
        while app
            .world()
            .non_send::<JsonStates>()
            .get(entity)
            .unwrap()
            .error()
            .is_none()
        {
            app.update();
            assert!(Instant::now() < deadline, "JSON preparation did not finish");
            thread::sleep(Duration::from_millis(1));
        }

        let mut context = dear_imgui_rs::Context::create();
        context
            .set_ini_filename(None::<std::path::PathBuf>)
            .unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        let mut draw = |context: &mut dear_imgui_rs::Context| {
            context.prepare_frame(dear_imgui_rs::FramePrepareOptions::new(
                [600.0, 400.0],
                1.0 / 60.0,
            ));
            let ui = context.frame();
            let mut point = [0.0; 2];
            ui.window("JSON frontend")
                .position([0.0; 2], dear_imgui_rs::Condition::Always)
                .size([600.0, 400.0], dear_imgui_rs::Condition::Always)
                .build(|| {
                    frontend.draw(
                        ui,
                        app.world_mut(),
                        &mut Vec::new(),
                        &bed_workbench_api::ShortcutInput::default(),
                    );
                    let (min, max) = ui.item_rect();
                    point = [(min[0] + max[0]) * 0.5, (min[1] + max[1]) * 0.5];
                });
            drop(context.render_legacy());
            point
        };
        draw(&mut context);
        let button = draw(&mut context);
        context.io_mut().add_mouse_pos_event(button);
        draw(&mut context);
        context
            .io_mut()
            .add_mouse_button_event(dear_imgui_rs::MouseButton::Left, true);
        draw(&mut context);
        context
            .io_mut()
            .add_mouse_button_event(dear_imgui_rs::MouseButton::Left, false);
        draw(&mut context);
        let requests = app.world_mut().resource_mut::<EditorIntents>().take();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].source, Some(entity));
        assert!(matches!(
            &requests[0].request,
            HostRequest::SwitchViewer {
                tab: 37,
                document: DocumentId(4),
                viewer,
            } if viewer == "fixture.text"
        ));
    }

    #[test]
    fn registered_commands_preserve_origin_and_area_when_focus_changes() {
        let mut app = App::new();
        app.add_plugins(JsonPlugin);
        app.world_mut().resource_mut::<EditorFrame>().documents = vec![
            document(4, "origin.json", "{}", 1),
            document(5, "focused.json", "{}", 1),
        ];
        let (origin, _) = create(&mut app, Some(DocumentId(4)), Value::Null);
        app.world_mut()
            .resource_mut::<EditorFrame>()
            .active_document = Some(DocumentId(5));
        let command = app
            .world()
            .resource::<bed_workbench_api::Registry>()
            .command(OPEN_FILE_COMMAND)
            .unwrap()
            .handler;
        app.world_mut()
            .run_system_with(
                command,
                CommandInvocation {
                    command: OPEN_FILE_COMMAND.into(),
                    source: Some(origin),
                    area: 19,
                    arguments: Value::Null,
                },
            )
            .unwrap()
            .unwrap();
        let requests = app.world_mut().resource_mut::<EditorIntents>().take();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].source, Some(origin));
        assert_eq!(requests[0].area, Some(19));
        assert!(matches!(&requests[0].request,
            HostRequest::OpenFile { path, viewer }
                if path == "origin.json" && viewer.as_deref() == Some(VIEWER_ID)));
    }

    #[test]
    fn document_actions_target_the_origin_and_ignore_retired_entities() {
        let mut app = App::new();
        app.add_plugins(JsonPlugin);
        app.world_mut().resource_mut::<EditorFrame>().documents = vec![
            document(4, "origin.json", "{}", 1),
            document(5, "focused.json", "{}", 1),
        ];
        let (entity, _) = create(&mut app, Some(DocumentId(4)), Value::Null);
        app.world_mut()
            .resource_mut::<EditorFrame>()
            .active_document = Some(DocumentId(5));
        let action = app.world().get::<EditorPanelActions>(entity).unwrap().0;
        assert!(
            app.world_mut()
                .run_system_with(
                    action,
                    PanelActionInvocation {
                        entity,
                        action: PanelAction::Undo,
                    }
                )
                .unwrap()
                .unwrap()
        );
        let requests = app.world_mut().resource_mut::<EditorIntents>().take();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].source, Some(entity));
        assert!(matches!(
            requests[0].request,
            HostRequest::Undo {
                document: DocumentId(4)
            }
        ));

        app.world_mut().despawn(entity);
        assert!(
            !app.world_mut()
                .run_system_with(
                    action,
                    PanelActionInvocation {
                        entity,
                        action: PanelAction::Redo,
                    }
                )
                .unwrap()
                .unwrap()
        );
        assert!(
            app.world_mut()
                .resource_mut::<EditorIntents>()
                .take()
                .is_empty()
        );
    }
}
