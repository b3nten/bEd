//! Markdown documents, workers and render systems are hosted by the main world.
use crate::{
    MarkdownPanel, OPEN_COMMAND, OPEN_FILE_COMMAND, PANEL_ID, PLUGIN_ID, VIEWER_ID,
    register_metadata, supported_path,
};
use bed_editing::identity::DocumentId;
use bed_editor_api::gpu::GpuContext;
use bed_editor_api::{
    EditorFeatureSystems, EditorFrame, EditorIntents, EditorPanelActions, EditorPanelBinding,
    HostRequest, PanelActionInvocation, PanelActionResult, Registry as EditorRegistry,
};
use bed_ui::gpu::{
    CanvasOutput, CanvasRenderInvocation, CanvasRenderResult, CanvasRenderer, RenderOutput, wgpu,
};
use bed_workbench_api::{
    Action, CommandInvocation, CommandResult, Panel, PanelCreation, PanelResult,
};
use bevy::{ecs::system::SystemState, prelude::*};
use dear_imgui_rs::Ui;
use serde_json::Value;
use std::collections::HashMap;

#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct MarkdownDocument(pub DocumentId);

#[derive(Default)]
pub struct MarkdownStates(HashMap<Entity, MarkdownPanel>);
impl MarkdownStates {
    pub fn get(&self, entity: Entity) -> Option<&MarkdownPanel> {
        self.0.get(&entity)
    }
    #[cfg(test)]
    pub(crate) fn get_mut(&mut self, entity: Entity) -> Option<&mut MarkdownPanel> {
        self.0.get_mut(&entity)
    }
}

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MarkdownSystems {
    Poll,
}

#[derive(Resource)]
struct MarkdownActions(bed_editor_api::PanelActionSystem);
pub struct MarkdownPlugin;
impl Plugin for MarkdownPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<EditorFrame>()
            .init_resource::<EditorIntents>()
            .init_resource::<EditorRegistry>()
            .init_non_send::<MarkdownStates>()
            .add_systems(
                PreUpdate,
                poll_markdowns
                    .in_set(MarkdownSystems::Poll)
                    .in_set(EditorFeatureSystems::Prepare),
            )
            .add_systems(Last, shutdown.after(bevy::app::OnAppExitSystems));
        app.world_mut()
            .resource_mut::<EditorRegistry>()
            .register_with(PLUGIN_ID, register_metadata)
            .expect("Markdown declarations must register exactly once");
        let action = app.world_mut().register_system(markdown_action);
        app.insert_resource(MarkdownActions(action));
        bed_workbench_api::register(app.world_mut(), PLUGIN_ID, |registrar| {
            registrar.panel(PANEL_ID, "Markdown Preview", create_markdown_panel);
            registrar.command(OPEN_COMMAND, "Open Markdown…", markdown_command);
            registrar.command(
                OPEN_FILE_COMMAND,
                "Open in Markdown Preview",
                markdown_command,
            );
            registrar.menu(OPEN_COMMAND);
        })
        .expect("Markdown systems must register exactly once");
    }
}

fn shutdown(mut exits: MessageReader<bevy::app::AppExit>, mut states: NonSendMut<MarkdownStates>) {
    if exits.read().next().is_some() {
        states.0.clear();
    }
}

fn create_markdown_panel(In(creation): In<PanelCreation>, world: &mut World) -> PanelResult {
    if world
        .get::<EditorPanelBinding>(creation.entity)
        .is_some_and(|binding| binding.input.local_file().is_some())
    {
        return Err("Markdown Preview requires an editor document".into());
    }
    let document = world
        .get::<EditorPanelBinding>(creation.entity)
        .and_then(|binding| binding.input.document().or(binding.document))
        .or_else(|| creation.state["document"].as_u64().map(DocumentId));
    if let Some(document) = document {
        let panel = MarkdownPanel::new(document, &creation.state)?;
        let render = world.register_system(render_markdown);
        world
            .entity_mut(render.entity())
            .insert(ChildOf(creation.entity));
        let action = world.resource::<MarkdownActions>().0;
        world.entity_mut(creation.entity).insert((
            MarkdownDocument(document),
            CanvasRenderer(render),
            EditorPanelActions(action),
        ));
        world
            .non_send_mut::<MarkdownStates>()
            .0
            .insert(creation.entity, panel);
    }
    Ok(Box::new(MarkdownFrontend {
        entity: creation.entity,
    }))
}

fn poll_markdowns(world: &mut World) {
    let workspace = world
        .get_non_send::<bed_document_session::editor_session::EditorSession>()
        .map(|session| {
            (
                session
                    .options()
                    .project_root
                    .as_ref()
                    .map(|root| root.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                session.remote_client(),
            )
        })
        .unwrap_or_default();
    let outputs = {
        let mut state = SystemState::<(
            Res<EditorFrame>,
            NonSendMut<MarkdownStates>,
            Query<&MarkdownDocument>,
            ResMut<EditorIntents>,
        )>::new(world);
        let (frame, mut states, live, mut intents) = state
            .get_mut(world)
            .expect("Markdown plugin initialized its preparation resources");
        states.0.retain(|entity, _| live.get(*entity).is_ok());
        let host = frame.context();
        for panel in states.0.values_mut() {
            panel.prepare(&host, &workspace.0, workspace.1.clone());
        }
        if states.0.values().any(MarkdownPanel::pending) {
            intents.push(None, HostRequest::Invalidate);
        }
        states
            .0
            .iter()
            .map(|(entity, panel)| {
                (
                    *entity,
                    panel.canvas_output(),
                    match panel.error() {
                        Some(error) => bed_editor_api::EditorPanelReadiness::Failed(error.into()),
                        None if panel.is_ready() && !panel.pending() => {
                            bed_editor_api::EditorPanelReadiness::Ready
                        }
                        None => bed_editor_api::EditorPanelReadiness::Pending,
                    },
                )
            })
            .collect::<Vec<_>>()
    };
    for (entity, output, readiness) in outputs {
        world.entity_mut(entity).insert(readiness);
        publish_output(world, entity, output);
    }
}

fn markdown_command(
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
                let binding = bindings.get(invocation.source?).ok()?;
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
        _ => return Err(format!("Unknown markdown command: {}", invocation.command)),
    };
    if let Some(request) = request {
        intents.push_at(invocation.source, invocation.area, request);
    }
    Ok(Vec::new())
}

fn render_markdown(
    In(invocation): In<CanvasRenderInvocation>,
    mut states: NonSendMut<MarkdownStates>,
    live: Query<&MarkdownDocument>,
) -> CanvasRenderResult {
    if live.get(invocation.entity).is_err() {
        return Ok(Vec::new());
    }
    let panel = states
        .0
        .get_mut(&invocation.entity)
        .expect("An markdown panel owns its runtime state");
    let mut encoder = invocation
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Bed markdown canvas system"),
        });
    {
        let mut gpu = GpuContext {
            world: None,
            instance: &invocation.instance,
            adapter: &invocation.adapter,
            device: &invocation.device,
            queue: &invocation.queue,
            encoder: &mut encoder,
            error_handlers: None,
            generation: invocation.generation,
        };
        panel.record_render(&mut gpu, &invocation.target)?;
    }
    Ok(vec![encoder.finish()])
}

fn publish_output(world: &mut World, entity: Entity, output: Option<RenderOutput>) {
    let mut entity = world.entity_mut(entity);
    if let Some(output) = output {
        entity.insert(CanvasOutput(output));
    } else {
        entity.remove::<CanvasOutput>();
    }
}

struct MarkdownFrontend {
    entity: Entity,
}
impl Panel for MarkdownFrontend {
    fn title(&self, world: &World) -> String {
        world
            .non_send::<MarkdownStates>()
            .get(self.entity)
            .map_or_else(
                || "Markdown Preview".into(),
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
        let viewer_menu = bed_editor_api::panel_viewer_menu(world, self.entity, VIEWER_ID);
        let output = {
            let mut state = SystemState::<(
                Res<EditorFrame>,
                NonSendMut<MarkdownStates>,
                ResMut<EditorIntents>,
            )>::new(world);
            let (frame, mut states, mut intents) = state
                .get_mut(world)
                .expect("Markdown plugin initialized its presentation resources");
            let mut requests = Vec::new();
            let output = if let Some(panel) = states.0.get_mut(&self.entity) {
                let mut host = frame.context();
                host.viewer_menu = viewer_menu.as_ref();
                panel.draw_prepared(ui, &host, &mut requests);
                panel.canvas_output()
            } else {
                ui.text_wrapped("Open an markdown to view it.");
                if ui.button("Open Markdown…") {
                    requests.push(HostRequest::OpenFileDialog {
                        viewer: Some(VIEWER_ID.into()),
                    });
                }
                None
            };
            for request in requests {
                intents.push(Some(self.entity), request);
            }
            output
        };
        publish_output(world, self.entity, output);
    }

    fn save(&self, world: &World) -> Value {
        world
            .non_send::<MarkdownStates>()
            .get(self.entity)
            .map_or(Value::Null, MarkdownPanel::saved_state)
    }
}

fn markdown_action(
    In(invocation): In<PanelActionInvocation>,
    frame: Res<EditorFrame>,
    mut states: NonSendMut<MarkdownStates>,
    live: Query<&MarkdownDocument>,
    mut intents: ResMut<EditorIntents>,
) -> PanelActionResult {
    if live.get(invocation.entity).is_err() {
        return Ok(false);
    }
    let panel = states
        .0
        .get_mut(&invocation.entity)
        .expect("A Markdown panel owns its runtime state");
    let mut requests = Vec::new();
    let handled = panel.apply_action(invocation.action, &frame.context(), &mut requests)?;
    for request in requests {
        intents.push(Some(invocation.entity), request);
    }
    Ok(handled)
}

#[cfg(test)]
mod ecs_tests {
    use super::*;
    use bed_editor_api::{DocumentKind, PanelInput, PluginDocument};
    use serde_json::json;
    use std::{
        sync::Arc,
        thread,
        time::{Duration, Instant},
    };

    fn document(bytes: Arc<[u8]>, revision: u64) -> PluginDocument {
        PluginDocument {
            id: DocumentId(4),
            path: "readme.md".into(),
            kind: DocumentKind::Text,
            language_id: String::new(),
            revision: (1, revision),
            dirty: true,
            bytes,
            text: None,
        }
    }
    fn create(app: &mut App, state: Value) -> (Entity, Box<dyn Panel>) {
        let entity = app
            .world_mut()
            .spawn(EditorPanelBinding {
                input: PanelInput::Document(DocumentId(4)),
                document: Some(DocumentId(4)),
                workspace: 1,
                ..Default::default()
            })
            .id();
        let factory = app
            .world()
            .resource::<bed_workbench_api::Registry>()
            .panel(PANEL_ID)
            .unwrap()
            .factory;
        let panel = app
            .world_mut()
            .run_system_with(factory, PanelCreation { entity, state })
            .unwrap()
            .unwrap();
        (entity, panel)
    }
    fn wait(app: &mut App, entity: Entity, ready: impl Fn(&MarkdownPanel) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            app.update();
            if ready(
                app.world()
                    .non_send::<MarkdownStates>()
                    .get(entity)
                    .unwrap(),
            ) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "Markdown preparation did not finish"
            );
            thread::sleep(Duration::from_millis(1));
        }
    }
    #[test]
    fn commands_keep_the_origin_path_and_area() {
        let mut app = App::new();
        app.add_plugins(MarkdownPlugin);
        app.world_mut().resource_mut::<EditorFrame>().documents =
            vec![document(Arc::from(&b"unused"[..]), 1)];
        let (entity, _) = create(&mut app, Value::Null);
        let handler = app
            .world()
            .resource::<bed_workbench_api::Registry>()
            .command(OPEN_FILE_COMMAND)
            .unwrap()
            .handler;
        app.world_mut()
            .run_system_with(
                handler,
                CommandInvocation {
                    command: OPEN_FILE_COMMAND.into(),
                    source: Some(entity),
                    area: 19,
                    arguments: json!({"path": "context.MD"}),
                },
            )
            .unwrap()
            .unwrap();
        let requests = app.world_mut().resource_mut::<EditorIntents>().take();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].source, Some(entity));
        assert_eq!(requests[0].area, Some(19));
        assert!(
            matches!(&requests[0].request, HostRequest::OpenFile {path, viewer} if path == "context.MD" && viewer.as_deref() == Some(VIEWER_ID))
        );
    }

    #[test]
    fn preparation_reads_workspace_images_and_actions_keep_document_origin() {
        use bed_document_session::editor_session::{EditorSession, SessionOptions};
        let root = std::env::temp_dir().join(format!(
            "bed-markdown-world-{}-{}",
            std::process::id(),
            DocumentId::next().0
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("image.svg"), br#"<svg xmlns="http://www.w3.org/2000/svg" width="2" height="3"><rect width="2" height="3" fill="red"/></svg>"#).unwrap();
        let mut app = App::new();
        app.add_plugins(MarkdownPlugin);
        app.world_mut().insert_non_send(
            EditorSession::with_options(SessionOptions {
                project_root: Some(root.clone()),
                ..Default::default()
            })
            .unwrap(),
        );
        let mut source = document(
            Arc::from(
                &b"# Test
![image](image.svg)"[..],
            ),
            1,
        );
        source.path = root.join("readme.md").to_string_lossy().into_owned();
        app.world_mut().resource_mut::<EditorFrame>().documents = vec![source];
        let (entity, frontend) = create(&mut app, json!({"scroll":[8.0,12.0]}));
        wait(&mut app, entity, |panel| {
            panel.is_ready() && !panel.pending()
        });
        assert!(app.world().get::<CanvasOutput>(entity).is_some());
        assert_eq!(frontend.save(app.world())["scroll"], json!([8.0, 12.0]));
        let action = app.world().get::<EditorPanelActions>(entity).unwrap().0;
        app.world_mut().resource_mut::<EditorIntents>().take();
        app.world_mut()
            .resource_mut::<EditorFrame>()
            .active_document = Some(DocumentId(99));
        assert!(
            app.world_mut()
                .run_system_with(
                    action,
                    PanelActionInvocation {
                        entity,
                        action: bed_editor_api::PanelAction::Undo
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
        {
            let mut frame = app.world_mut().resource_mut::<EditorFrame>();
            frame.documents[0].revision = (1, 2);
            frame.documents[0].bytes = Arc::from(&b"# Image removed"[..]);
        }
        app.update();
        assert!(app.world().get::<CanvasOutput>(entity).is_none());
        wait(&mut app, entity, MarkdownPanel::is_ready);
        let (sibling, _) = create(&mut app, Value::Null);
        let renderer = app.world().get::<CanvasRenderer>(entity).unwrap().0;
        let sibling_renderer = app.world().get::<CanvasRenderer>(sibling).unwrap().0;
        assert_ne!(
            renderer, sibling_renderer,
            "render failures belong to one presentation"
        );
        assert_eq!(
            app.world()
                .get::<ChildOf>(renderer.entity())
                .unwrap()
                .parent(),
            entity
        );
        app.world_mut().entity_mut(entity).despawn();
        app.update();
        assert!(!app.world().entities().contains(renderer.entity()));
        assert!(app.world().entities().contains(sibling_renderer.entity()));

        assert!(
            app.world()
                .non_send::<MarkdownStates>()
                .get(entity)
                .is_none()
        );
        drop(app);
        std::fs::remove_dir_all(root).unwrap();
    }
}
