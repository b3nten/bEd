//! Gltf documents, workers and render systems are hosted by the main world.
use crate::{
    GltfPanel, OPEN_COMMAND, OPEN_FILE_COMMAND, PANEL_ID, PLUGIN_ID, VIEWER_ID, register_metadata,
    supported_path,
};
use bed_editing::identity::DocumentId;
use bed_editor_api::gpu::GpuContext;
use bed_editor_api::{
    EditorFeatureSystems, EditorFrame, EditorIntents, EditorPanelBinding, HostRequest,
    Registry as EditorRegistry,
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
pub struct GltfDocument(pub DocumentId);

#[derive(Default)]
pub struct GltfStates(HashMap<Entity, GltfPanel>);

impl GltfStates {
    pub fn get(&self, entity: Entity) -> Option<&GltfPanel> {
        self.0.get(&entity)
    }

    #[cfg(test)]
    pub(crate) fn get_mut(&mut self, entity: Entity) -> Option<&mut GltfPanel> {
        self.0.get_mut(&entity)
    }
}

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GltfSystems {
    Poll,
}

/// Register the in-memory model source before Bevy's AssetPlugin builds readers.
/// Desktop applications compose this plugin before their graphics plugin group.
pub struct ModelAssetsPlugin;

impl Plugin for ModelAssetsPlugin {
    fn build(&self, app: &mut App) {
        assert!(
            !app.is_plugin_added::<bevy::asset::AssetPlugin>(),
            "ModelAssetsPlugin must be installed before AssetPlugin (or DesktopPlugins)"
        );
        crate::bevy_scene::SceneLoading::register(app);
    }
}

pub struct ModelViewerPlugin;

impl Plugin for ModelViewerPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<ModelAssetsPlugin>() {
            app.add_plugins(ModelAssetsPlugin);
        }
        if !app.is_plugin_added::<crate::skybox_blur::SkyboxBlurPlugin>() {
            app.add_plugins(crate::skybox_blur::SkyboxBlurPlugin);
        }
        app.init_resource::<EditorFrame>()
            .init_resource::<EditorIntents>()
            .init_resource::<EditorRegistry>()
            .init_non_send::<GltfStates>()
            .add_systems(
                PreUpdate,
                poll_gltfs
                    .in_set(GltfSystems::Poll)
                    .in_set(EditorFeatureSystems::Prepare),
            )
            .add_systems(Last, shutdown.after(bevy::app::OnAppExitSystems));
        app.world_mut()
            .resource_mut::<EditorRegistry>()
            .register_with(PLUGIN_ID, register_metadata)
            .expect("Gltf declarations must register exactly once");
        bed_workbench_api::register(app.world_mut(), PLUGIN_ID, |registrar| {
            registrar.panel(PANEL_ID, "Model Viewer", create_gltf_panel);
            registrar.command(OPEN_COMMAND, "Open 3D Model…", gltf_command);
            registrar.command(OPEN_FILE_COMMAND, "Open in Model Viewer", gltf_command);
            registrar.menu(OPEN_COMMAND);
        })
        .expect("Gltf systems must register exactly once");
    }
}

fn shutdown(mut exits: MessageReader<bevy::app::AppExit>, mut states: NonSendMut<GltfStates>) {
    if exits.read().next().is_some() {
        states.0.clear();
    }
}

fn create_gltf_panel(In(creation): In<PanelCreation>, world: &mut World) -> PanelResult {
    world.entity_mut(creation.entity).insert(bed_workbench_api::PanelDocumentation::new(&[
        ("Camera", "Drag with the left button to orbit, or the right/middle button to pan. Scroll to zoom."),
        ("View", "Frame All or double-click restores the full model view. Info and Appearance expose model details and rendering controls."),
    ]).shortcut_actions(&[bed_keybindings::ZOOM_IN, bed_keybindings::ZOOM_OUT, bed_keybindings::RESET_ZOOM]));
    if world
        .get::<EditorPanelBinding>(creation.entity)
        .is_some_and(|binding| binding.input.local_file().is_some())
    {
        return Err("Model Viewer requires an editor document".into());
    }
    let document = world
        .get::<EditorPanelBinding>(creation.entity)
        .and_then(|binding| binding.input.document().or(binding.document))
        .or_else(|| creation.state["document"].as_u64().map(DocumentId));
    if let Some(document) = document {
        let render = world.register_system(render_gltf);
        world
            .entity_mut(render.entity())
            .insert(ChildOf(creation.entity));
        world.entity_mut(creation.entity).insert((
            GltfDocument(document),
            CanvasRenderer(render),
            bed_editor_api::EditorPanelReadiness::Pending,
        ));
        world
            .non_send_mut::<GltfStates>()
            .0
            .insert(creation.entity, GltfPanel::new(document, &creation.state));
    }
    Ok(Box::new(GltfFrontend {
        entity: creation.entity,
    }))
}

fn poll_gltfs(world: &mut World) {
    let outputs = {
        let mut state = SystemState::<(
            Res<EditorFrame>,
            NonSendMut<GltfStates>,
            Query<&GltfDocument>,
            ResMut<EditorIntents>,
        )>::new(world);
        let (frame, mut states, live, mut intents) = state
            .get_mut(world)
            .expect("Gltf plugin initialized its preparation resources");
        states.0.retain(|entity, _| live.get(*entity).is_ok());
        let host = frame.context();
        for panel in states.0.values_mut() {
            panel.update(&host);
        }
        if states.0.values().any(|panel| {
            host.document(panel.document).is_some()
                && panel.prepared.is_none()
                && panel.error.is_none()
        }) {
            intents.push(None, HostRequest::Invalidate);
        }
        states
            .0
            .iter()
            .map(|(entity, panel)| {
                (
                    *entity,
                    panel.canvas_output(),
                    match &panel.error {
                        Some(error) => bed_editor_api::EditorPanelReadiness::Failed(error.clone()),
                        None if panel.prepared.is_some()
                            && panel.gpu.as_ref().is_some_and(|gpu| gpu.ready()) =>
                        {
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

fn gltf_command(
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
        _ => return Err(format!("Unknown gltf command: {}", invocation.command)),
    };
    if let Some(request) = request {
        intents.push_at(invocation.source, invocation.area, request);
    }
    Ok(Vec::new())
}

fn render_gltf(
    In(invocation): In<CanvasRenderInvocation>,
    world: &mut World,
) -> CanvasRenderResult {
    if world.get::<GltfDocument>(invocation.entity).is_none() {
        return Ok(Vec::new());
    }
    let mut work = world
        .non_send_mut::<GltfStates>()
        .0
        .get_mut(&invocation.entity)
        .expect("A glTF panel owns its state")
        .take_render_work();
    let mut encoder = invocation
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Bed model canvas system"),
        });
    let mut gpu = GpuContext {
        world: Some(world),
        instance: &invocation.instance,
        adapter: &invocation.adapter,
        device: &invocation.device,
        queue: &invocation.queue,
        encoder: &mut encoder,
        error_handlers: None,
        generation: invocation.generation,
    };
    let result = work.record_render(&mut gpu, &invocation.target);
    let output = {
        let mut states = world.non_send_mut::<GltfStates>();
        let panel = states
            .0
            .get_mut(&invocation.entity)
            .expect("A rendered model presentation remains alive");
        panel.finish_render_work(work);
        panel.canvas_output()
    };
    publish_output(world, invocation.entity, output);
    result.map(|()| vec![encoder.finish()])
}

fn publish_output(world: &mut World, entity: Entity, output: Option<RenderOutput>) {
    let mut entity = world.entity_mut(entity);
    if let Some(output) = output {
        entity.insert(CanvasOutput(output));
    } else {
        entity.remove::<CanvasOutput>();
    }
}

struct GltfFrontend {
    entity: Entity,
}

impl Panel for GltfFrontend {
    fn title(&self, world: &World) -> String {
        world.non_send::<GltfStates>().get(self.entity).map_or_else(
            || "Model Viewer".into(),
            |panel| panel.panel_title(&world.resource::<EditorFrame>().context()),
        )
    }

    fn draw(
        &mut self,
        ui: &Ui,
        world: &mut World,
        _requests: &mut Vec<Action>,
        shortcuts: &bed_workbench_api::ShortcutInput,
    ) {
        let viewer_menu = bed_editor_api::panel_viewer_menu(world, self.entity, VIEWER_ID);
        let output = {
            let mut state = SystemState::<(
                Res<EditorFrame>,
                NonSendMut<GltfStates>,
                ResMut<EditorIntents>,
            )>::new(world);
            let (frame, mut states, mut intents) = state
                .get_mut(world)
                .expect("Gltf plugin initialized its presentation resources");
            let mut requests = Vec::new();
            let output = if let Some(panel) = states.0.get_mut(&self.entity) {
                panel.shortcuts = shortcuts.clone();
                let mut host = frame.context();
                host.viewer_menu = viewer_menu.as_ref();
                panel.draw_prepared(ui, &host, &mut requests);
                if panel.canvas.is_some()
                    && (panel.settle_frames > 0
                        || panel.gpu.as_ref().is_some_and(|gpu| gpu.pending()))
                {
                    requests.push(HostRequest::Invalidate);
                }
                panel.canvas_output()
            } else {
                ui.text_wrapped("Open an gltf to view it.");
                if ui.button("Open 3D Model…") {
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
            .non_send::<GltfStates>()
            .get(self.entity)
            .map_or(Value::Null, GltfPanel::saved_state)
    }
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
            path: "cube.glb".into(),
            kind: DocumentKind::Bytes,
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

    fn wait(app: &mut App, entity: Entity, ready: impl Fn(&GltfPanel) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            app.update();
            if ready(app.world().non_send::<GltfStates>().get(entity).unwrap()) {
                return;
            }
            assert!(Instant::now() < deadline, "Gltf preparation did not finish");
            thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn commands_keep_the_origin_path_and_area() {
        let mut app = App::new();
        app.add_plugins(ModelViewerPlugin);
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
                    arguments: json!({"path": "context.STL"}),
                },
            )
            .unwrap()
            .unwrap();
        let requests = app.world_mut().resource_mut::<EditorIntents>().take();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].source, Some(entity));
        assert_eq!(requests[0].area, Some(19));
        assert!(
            matches!(&requests[0].request, HostRequest::OpenFile {path, viewer} if path == "context.STL" && viewer.as_deref() == Some(VIEWER_ID))
        );
    }

    #[test]
    fn preparation_loads_shared_document_revisions_and_retires_worker_state() {
        let bytes: Arc<[u8]> =
            Arc::from(include_bytes!("../../../tests/fixtures/gltf/cube.glb").as_slice());
        let mut app = App::new();
        app.add_plugins(ModelViewerPlugin);
        app.world_mut().resource_mut::<EditorFrame>().documents = vec![document(bytes.clone(), 1)];
        let (entity, frontend) = create(&mut app, json!({"render":{"normals":true}}));
        wait(&mut app, entity, |panel| panel.prepared.is_some());
        assert_eq!(frontend.title(app.world()), "cube.glb");
        assert_eq!(frontend.save(app.world())["render"]["normals"], true);
        {
            let mut states = app.world_mut().non_send_mut::<GltfStates>();
            let panel = states.get_mut(entity).unwrap();
            panel.canvas = Some(crate::CanvasState {
                pixels: [64, 32],
                camera: panel.camera,
                background: [0.0; 4],
                settings: panel.settings,
            });
        }
        app.update();
        let output = app.world().get::<CanvasOutput>(entity).unwrap().0;
        assert!(output.depth);
        assert_eq!(output.size, [64, 32]);
        app.world_mut().resource_mut::<EditorFrame>().documents[0] =
            document(Arc::from(&b"corrupt glb"[..]), 2);
        app.update();
        assert!(app.world().get::<CanvasOutput>(entity).is_none());
        wait(&mut app, entity, |panel| panel.error.is_some());
        app.world_mut().resource_mut::<EditorFrame>().documents[0] = document(bytes, 3);
        wait(&mut app, entity, |panel| panel.prepared.is_some());
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

        assert!(app.world().non_send::<GltfStates>().get(entity).is_none());
    }

    #[test]
    fn a_full_worker_queue_cannot_leave_an_older_revision_renderable() {
        let bytes: Arc<[u8]> = include_bytes!("../../../tests/fixtures/gltf/cube.glb")
            .as_slice()
            .into();
        let mut app = App::new();
        app.add_plugins(ModelViewerPlugin);
        app.world_mut().resource_mut::<EditorFrame>().documents = vec![document(bytes.clone(), 1)];
        let (entity, _) = create(&mut app, Value::Null);
        wait(&mut app, entity, |panel| panel.prepared.is_some());
        let (sender, _blocked_worker) = std::sync::mpsc::sync_channel(1);
        sender
            .try_send(crate::LoadJob {
                revision: (1, 1),
                bytes,
                stl: false,
            })
            .unwrap();
        {
            let mut states = app.world_mut().non_send_mut::<GltfStates>();
            let panel = states.get_mut(entity).unwrap();
            panel.canvas = Some(crate::CanvasState {
                pixels: [64; 2],
                camera: panel.camera,
                background: [0.0; 4],
                settings: panel.settings,
            });
            panel.loader.sender = Some(sender);
        }
        app.update();
        assert!(app.world().get::<CanvasOutput>(entity).is_some());
        app.world_mut().resource_mut::<EditorFrame>().documents[0] =
            document(Arc::from(&b"new bytes"[..]), 2);
        app.update();
        let panel = app.world().non_send::<GltfStates>().get(entity).unwrap();
        assert_eq!(panel.requested, Some((1, 1)));
        assert!(panel.prepared.is_none());
        assert!(panel.gpu.is_none());
        assert!(app.world().get::<CanvasOutput>(entity).is_none());
    }
}
