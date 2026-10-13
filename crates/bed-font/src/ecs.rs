//! Font documents, workers and render systems are hosted by the main world.
use crate::{
    FontPanel, OPEN_COMMAND, OPEN_FILE_COMMAND, PANEL_ID, PLUGIN_ID, VIEWER_ID, register_metadata,
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
pub struct FontDocument(pub DocumentId);

#[derive(Default)]
pub struct FontStates(HashMap<Entity, FontPanel>);

impl FontStates {
    pub fn get(&self, entity: Entity) -> Option<&FontPanel> {
        self.0.get(&entity)
    }

    #[cfg(test)]
    pub(crate) fn get_mut(&mut self, entity: Entity) -> Option<&mut FontPanel> {
        self.0.get_mut(&entity)
    }
}

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FontSystems {
    Poll,
}

pub struct FontPlugin;

impl Plugin for FontPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<EditorFrame>()
            .init_resource::<EditorIntents>()
            .init_resource::<EditorRegistry>()
            .init_non_send::<FontStates>()
            .add_systems(
                PreUpdate,
                poll_fonts
                    .in_set(FontSystems::Poll)
                    .in_set(EditorFeatureSystems::Prepare),
            )
            .add_systems(Last, shutdown.after(bevy::app::OnAppExitSystems));
        app.world_mut()
            .resource_mut::<EditorRegistry>()
            .register_with(PLUGIN_ID, register_metadata)
            .expect("Font declarations must register exactly once");
        bed_workbench_api::register(app.world_mut(), PLUGIN_ID, |registrar| {
            registrar.panel(PANEL_ID, "Font Viewer", create_font_panel);
            registrar.command(OPEN_COMMAND, "Open Font…", font_command);
            registrar.command(OPEN_FILE_COMMAND, "Open in Font Viewer", font_command);
            registrar.menu(OPEN_COMMAND);
        })
        .expect("Font systems must register exactly once");
    }
}

fn shutdown(mut exits: MessageReader<bevy::app::AppExit>, mut states: NonSendMut<FontStates>) {
    if exits.read().next().is_some() {
        states.0.clear();
    }
}

fn create_font_panel(In(creation): In<PanelCreation>, world: &mut World) -> PanelResult {
    world.entity_mut(creation.entity).insert(bed_workbench_api::PanelDocumentation::new(&[
        ("Preview", "Explore the font’s characters and preview text at different sizes."),
        ("Glyphs", "Select a character to inspect its glyph. Copy character places it on the clipboard; Go jumps to a character."),
    ]));
    if world
        .get::<EditorPanelBinding>(creation.entity)
        .is_some_and(|binding| binding.input.local_file().is_some())
    {
        return Err("Font Viewer requires an editor document".into());
    }
    let document = world
        .get::<EditorPanelBinding>(creation.entity)
        .and_then(|binding| binding.input.document().or(binding.document))
        .or_else(|| creation.state["document"].as_u64().map(DocumentId));
    if let Some(document) = document {
        let render = world.register_system(render_font);
        world
            .entity_mut(render.entity())
            .insert(ChildOf(creation.entity));
        world
            .entity_mut(creation.entity)
            .insert((FontDocument(document), CanvasRenderer(render)));
        world
            .non_send_mut::<FontStates>()
            .0
            .insert(creation.entity, FontPanel::new(document, &creation.state));
    }
    Ok(Box::new(FontFrontend {
        entity: creation.entity,
    }))
}

fn poll_fonts(world: &mut World) {
    let outputs = {
        let mut state = SystemState::<(
            Res<EditorFrame>,
            NonSendMut<FontStates>,
            Query<&FontDocument>,
            ResMut<EditorIntents>,
        )>::new(world);
        let (frame, mut states, live, mut intents) = state
            .get_mut(world)
            .expect("Font plugin initialized its preparation resources");
        states.0.retain(|entity, _| live.get(*entity).is_ok());
        let host = frame.context();
        for panel in states.0.values_mut() {
            prepare_font(panel, &host);
        }
        if states.0.values().any(|panel| {
            host.document(panel.document).is_some()
                && ((panel.preview.is_none() && panel.error.is_none())
                    || panel.serial != panel.completed_serial)
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
                        None if panel.canvas_output().is_some() => {
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

fn font_command(
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
        _ => return Err(format!("Unknown font command: {}", invocation.command)),
    };
    if let Some(request) = request {
        intents.push_at(invocation.source, invocation.area, request);
    }
    Ok(Vec::new())
}

fn render_font(
    In(invocation): In<CanvasRenderInvocation>,
    mut states: NonSendMut<FontStates>,
    live: Query<&FontDocument>,
) -> CanvasRenderResult {
    if live.get(invocation.entity).is_err() {
        return Ok(Vec::new());
    }
    let panel = states
        .0
        .get_mut(&invocation.entity)
        .expect("An font panel owns its runtime state");
    let mut encoder = invocation
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Bed font canvas system"),
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

struct FontFrontend {
    entity: Entity,
}

impl Panel for FontFrontend {
    fn title(&self, world: &World) -> String {
        world.non_send::<FontStates>().get(self.entity).map_or_else(
            || "Font Viewer".into(),
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
        let output = {
            let mut state = SystemState::<(
                Res<EditorFrame>,
                NonSendMut<FontStates>,
                ResMut<EditorIntents>,
            )>::new(world);
            let (frame, mut states, mut intents) = state
                .get_mut(world)
                .expect("Font plugin initialized its presentation resources");
            let mut requests = Vec::new();
            let output = if let Some(panel) = states.0.get_mut(&self.entity) {
                panel.draw_prepared(ui, &frame.context(), &mut requests);
                panel.canvas_output()
            } else {
                ui.text_wrapped("Open an font to view it.");
                if ui.button("Open Font…") {
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
            .non_send::<FontStates>()
            .get(self.entity)
            .map_or(Value::Null, FontPanel::saved_state)
    }
}

fn prepare_font(panel: &mut FontPanel, host: &bed_editor_api::HostContext<'_>) {
    let Some(document) = host.document(panel.document) else {
        panel.preview = None;
        panel.canvas = None;
        panel.gpu = None;
        return;
    };
    panel.poll(document.revision);
    // Load metadata without requiring a visible UI, and rerasterize the last
    // requested view when document bytes change while the panel is hidden.
    let request = panel
        .requested
        .as_ref()
        .map(|key| key.request.clone())
        .unwrap_or_else(|| crate::RasterRequest {
            mode: crate::Mode::Preview,
            pixels: [1, 1],
            logical: [1.0, 1.0],
            label_height: 13.0,
            size: panel.size,
            sample: panel.sample.clone(),
            ligatures: panel.ligatures,
            kerning: panel.kerning,
            direction: panel.direction,
            pan: panel.pan,
            glyph_scroll: panel.glyph_scroll,
            selected: panel.selected,
            ink: [1.0; 4],
        });
    panel.submit(document.revision, &document.bytes, request);
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
            path: "font.ttf".into(),
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

    fn wait(app: &mut App, entity: Entity, ready: impl Fn(&FontPanel) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            app.update();
            if ready(app.world().non_send::<FontStates>().get(entity).unwrap()) {
                return;
            }
            assert!(Instant::now() < deadline, "Font preparation did not finish");
            thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn commands_keep_the_origin_path_and_area() {
        let mut app = App::new();
        app.add_plugins(FontPlugin);
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
                    arguments: json!({"path": "context.WOFF2"}),
                },
            )
            .unwrap()
            .unwrap();
        let requests = app.world_mut().resource_mut::<EditorIntents>().take();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].source, Some(entity));
        assert_eq!(requests[0].area, Some(19));
        assert!(
            matches!(&requests[0].request, HostRequest::OpenFile {path, viewer} if path == "context.WOFF2" && viewer.as_deref() == Some(VIEWER_ID))
        );
    }

    #[test]
    fn preparation_loads_hidden_fonts_rejects_stale_revisions_and_retires_workers() {
        let mut app = App::new();
        app.add_plugins(FontPlugin);
        app.world_mut().resource_mut::<EditorFrame>().documents = vec![document(
            Arc::from(include_bytes!("../../../tests/fixtures/fonts/DejaVuSans.ttf").as_slice()),
            1,
        )];
        let (entity, frontend) = create(&mut app, json!({"sample":"héllo", "size":42.0}));
        wait(&mut app, entity, |panel| panel.preview.is_some());
        assert_eq!(frontend.title(app.world()), "font.ttf");
        assert_eq!(frontend.save(app.world())["sample"], "héllo");
        app.world_mut()
            .non_send_mut::<FontStates>()
            .get_mut(entity)
            .unwrap()
            .canvas = Some([8, 6]);
        app.update();
        assert_eq!(
            app.world().get::<CanvasOutput>(entity).unwrap().0.size,
            [8, 6]
        );
        app.world_mut().resource_mut::<EditorFrame>().documents[0] =
            document(Arc::from(&b"corrupt edit"[..]), 2);
        app.update();
        assert!(app.world().get::<CanvasOutput>(entity).is_none());
        wait(&mut app, entity, |panel| panel.error.is_some());
        app.world_mut().resource_mut::<EditorFrame>().documents[0] = document(
            Arc::from(
                include_bytes!("../../../tests/fixtures/fonts/SourceCodePro-Regular.ttf")
                    .as_slice(),
            ),
            3,
        );
        wait(&mut app, entity, |panel| {
            panel
                .preview
                .as_ref()
                .is_some_and(|preview| preview.info.family == "Source Code Pro")
        });
        assert_eq!(frontend.save(app.world())["size"], 42.0);
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

        assert!(app.world().non_send::<FontStates>().get(entity).is_none());
    }

    #[test]
    fn pending_rasterization_keeps_the_reactive_host_awake_after_metadata_is_ready() {
        let bytes: Arc<[u8]> = include_bytes!("../../../tests/fixtures/fonts/DejaVuSans.ttf")
            .as_slice()
            .into();
        let mut app = App::new();
        app.add_plugins(FontPlugin);
        app.world_mut().resource_mut::<EditorFrame>().documents = vec![document(bytes.clone(), 1)];
        let (entity, _) = create(&mut app, Value::Null);
        wait(&mut app, entity, |panel| panel.preview.is_some());
        let (sender, _blocked_worker) = std::sync::mpsc::sync_channel(1);
        {
            let mut states = app.world_mut().non_send_mut::<FontStates>();
            let panel = states.get_mut(entity).unwrap();
            panel.worker.sender = Some(sender);
            let mut request = panel.requested.as_ref().unwrap().request.clone();
            request.sample = "updated sample".into();
            panel.submit((1, 1), &bytes, request);
            assert!(panel.preview.is_some());
            assert_ne!(panel.serial, panel.completed_serial);
        }
        app.world_mut().resource_mut::<EditorIntents>().take();
        app.update();
        assert!(
            app.world_mut()
                .resource_mut::<EditorIntents>()
                .take()
                .iter()
                .any(|intent| matches!(intent.request, HostRequest::Invalidate))
        );
    }
}
