//! Image documents, workers and render systems are hosted by the main world.
use crate::{
    ImagePanel, OPEN_COMMAND, OPEN_FILE_COMMAND, PANEL_ID, PLUGIN_ID, VIEWER_ID, register_metadata,
    supported_path,
};
use bedtime_api::documents::DocumentFrame;
use bedtime_api::features::FrontendSnapshot;
use bedtime_api::gpu::GpuContext;
use bedtime_api::workbench::{
    CommandInvocation, CommandResult, Panel, PanelCreation, PanelResult, WorkbenchAction,
};
use bedtime_api::{
    documents::PanelBinding,
    features::{FeatureRegistry, FeatureSystems, HostAction, HostEffects},
};
use bedtime_editing::identity::DocumentId;
use bedtime_ui::gpu::{
    CanvasOutput, CanvasRenderInvocation, CanvasRenderResult, CanvasRenderer, RenderOutput, wgpu,
};
use bedtime_ui::icons::Icons;
use bevy::{ecs::system::SystemState, prelude::*};
use dear_imgui_rs::Ui;
use serde_json::Value;
use std::collections::HashMap;

#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageDocument(pub DocumentId);

#[derive(Default)]
pub struct ImageStates(HashMap<Entity, ImagePanel>);

impl ImageStates {
    pub fn get(&self, entity: Entity) -> Option<&ImagePanel> {
        self.0.get(&entity)
    }

    #[cfg(test)]
    pub(crate) fn get_mut(&mut self, entity: Entity) -> Option<&mut ImagePanel> {
        self.0.get_mut(&entity)
    }
}

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ImageSystems {
    Poll,
}

pub struct ImageViewerPlugin;

impl Plugin for ImageViewerPlugin {
    fn build(&self, app: &mut App) {
        bedtime_api::settings::register(
            app.world_mut(),
            PLUGIN_ID,
            vec![bedtime_api::settings::SettingDefinition::new(
                "fit_by_default",
                "Fit images to panel by default",
                true,
            )],
        )
        .expect("Image preferences install once");
        app.init_resource::<DocumentFrame>()
            .init_resource::<HostEffects>()
            .init_resource::<FeatureRegistry>()
            .init_non_send::<ImageStates>()
            .add_systems(
                PreUpdate,
                poll_images
                    .in_set(ImageSystems::Poll)
                    .in_set(FeatureSystems::Prepare),
            )
            .add_systems(Last, shutdown.after(bevy::app::OnAppExitSystems));
        app.world_mut()
            .resource_mut::<FeatureRegistry>()
            .register_with(PLUGIN_ID, register_metadata)
            .expect("Image declarations must register exactly once");
        bedtime_api::workbench::register(app.world_mut(), PLUGIN_ID, |registrar| {
            registrar.panel(PANEL_ID, "Image Viewer", create_image_panel);
            registrar.command(OPEN_COMMAND, "Open Image…", image_command);
            registrar.command(OPEN_FILE_COMMAND, "Open in Image Viewer", image_command);
            registrar.menu(OPEN_COMMAND);
            registrar.command_icon(OPEN_COMMAND, "viewer-image");
            registrar.command_icon(OPEN_FILE_COMMAND, "viewer-image");
            registrar.panel_icon(PANEL_ID, "viewer-image");
        })
        .expect("Image systems must register exactly once");
    }
}

fn shutdown(mut exits: MessageReader<bevy::app::AppExit>, mut states: NonSendMut<ImageStates>) {
    if exits.read().next().is_some() {
        states.0.clear();
    }
}

fn create_image_panel(In(creation): In<PanelCreation>, world: &mut World) -> PanelResult {
    world.entity_mut(creation.entity).insert(bedtime_api::workbench::PanelDocumentation::new(&[
        ("View", "Use Fit to fill the panel or 100% for actual size. Scroll over the image to zoom; drag to pan."),
        ("Details", "Info shows image metadata. The viewer menu lets you inspect the same file in another view."),
    ]).shortcut_actions(&[bedtime_keybindings::ZOOM_IN, bedtime_keybindings::ZOOM_OUT, bedtime_keybindings::RESET_ZOOM]));
    if world
        .get::<PanelBinding>(creation.entity)
        .is_some_and(|binding| binding.input.local_file().is_some())
    {
        return Err("Image Viewer requires an editor document".into());
    }
    let document = world
        .get::<PanelBinding>(creation.entity)
        .and_then(|binding| binding.input.document().or(binding.document))
        .or_else(|| creation.state["document"].as_u64().map(DocumentId));
    if let Some(document) = document {
        let render = world.register_system(render_image);
        world
            .entity_mut(render.entity())
            .insert(ChildOf(creation.entity));
        world.entity_mut(creation.entity).insert((
            ImageDocument(document),
            CanvasRenderer(render),
            crate::status::items(),
        ));
        world
            .non_send_mut::<ImageStates>()
            .0
            .insert(creation.entity, ImagePanel::new(document, &creation.state));
    }
    Ok(Box::new(ImageFrontend {
        entity: creation.entity,
    }))
}

fn poll_images(world: &mut World) {
    let outputs = {
        let frame = FrontendSnapshot::capture(world);
        let mut state = SystemState::<(
            NonSendMut<ImageStates>,
            Query<&ImageDocument>,
            ResMut<HostEffects>,
        )>::new(world);
        let (mut states, live, mut intents) = state
            .get_mut(world)
            .expect("Image plugin initialized its preparation resources");
        states.0.retain(|entity, _| live.get(*entity).is_ok());
        let host = frame.context();
        for panel in states.0.values_mut() {
            panel.update(&host);
        }
        if states
            .0
            .values()
            .any(|panel| panel.decoded.is_none() && panel.error.is_none())
        {
            intents.push(None, HostAction::Invalidate);
        }
        states
            .0
            .iter()
            .map(|(entity, panel)| {
                (
                    *entity,
                    panel.canvas_output(),
                    match &panel.error {
                        Some(error) => {
                            bedtime_api::workbench::PanelReadiness::Failed(error.clone())
                        }
                        None if panel.canvas_output().is_some() => {
                            bedtime_api::workbench::PanelReadiness::Ready
                        }
                        None => bedtime_api::workbench::PanelReadiness::Pending,
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

fn image_command(
    In(invocation): In<CommandInvocation>,
    frame: Res<DocumentFrame>,
    bindings: Query<&PanelBinding>,
    mut intents: ResMut<HostEffects>,
) -> CommandResult {
    let request = match invocation.command.as_str() {
        OPEN_COMMAND => Some(HostAction::OpenFileDialog {
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
            .map(|path| HostAction::OpenFile {
                path: path.to_owned(),
                viewer: Some(VIEWER_ID.into()),
            }),
        _ => return Err(format!("Unknown image command: {}", invocation.command)),
    };
    if let Some(request) = request {
        intents.push_at(invocation.source, invocation.area, request);
    }
    Ok(Vec::new())
}

fn render_image(
    In(invocation): In<CanvasRenderInvocation>,
    mut states: NonSendMut<ImageStates>,
    live: Query<&ImageDocument>,
) -> CanvasRenderResult {
    if live.get(invocation.entity).is_err() {
        return Ok(Vec::new());
    }
    let panel = states
        .0
        .get_mut(&invocation.entity)
        .expect("An image panel owns its runtime state");
    let mut encoder = invocation
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Bed image canvas system"),
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

struct ImageFrontend {
    entity: Entity,
}

impl Panel for ImageFrontend {
    fn title(&self, world: &World) -> String {
        world
            .non_send::<ImageStates>()
            .get(self.entity)
            .map_or_else(
                || "Image Viewer".into(),
                |panel| panel.panel_title(&FrontendSnapshot::capture(world).context()),
            )
    }

    fn draw(
        &mut self,
        ui: &Ui,
        world: &mut World,
        _requests: &mut Vec<WorkbenchAction>,
        shortcuts: &bedtime_api::workbench::ShortcutInput,
    ) {
        let output = {
            let frame = FrontendSnapshot::capture(world);
            let mut state = SystemState::<(
                NonSendMut<ImageStates>,
                Option<NonSend<Icons>>,
                ResMut<HostEffects>,
            )>::new(world);
            let (mut states, icons, mut intents) = state
                .get_mut(world)
                .expect("Image plugin initialized its presentation resources");
            let mut requests = Vec::new();
            let output = if let Some(panel) = states.0.get_mut(&self.entity) {
                panel.shortcuts = shortcuts.clone();
                panel.draw_prepared(ui, &frame.context(), &mut requests, icons.as_deref());
                panel.canvas_output()
            } else {
                ui.text_wrapped("Open an image to view it.");
                if ui.button("Open Image…") {
                    requests.push(HostAction::OpenFileDialog {
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
            .non_send::<ImageStates>()
            .get(self.entity)
            .map_or(Value::Null, ImagePanel::saved_state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bedtime_api::documents::{DocumentKind, PanelInput, PluginDocument};
    use dear_imgui_rs::{Condition, Context, FramePrepareOptions};
    use serde_json::json;
    use std::{
        sync::Arc,
        thread,
        time::{Duration, Instant},
    };

    fn wait_image(app: &mut App, entity: Entity, size: [u32; 2]) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            app.update();
            if app
                .world()
                .non_send::<ImageStates>()
                .get(entity)
                .unwrap()
                .decoded
                .as_ref()
                .is_some_and(|decoded| decoded.size == size)
            {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "image preparation did not finish"
            );
            thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn world_workers_follow_unsaved_source_and_ui_publishes_renderable_canvas() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut app = App::new();
        app.add_plugins(ImageViewerPlugin);
        app.world_mut().resource_mut::<DocumentFrame>().documents = vec![PluginDocument {
            id: DocumentId(4),
            path: "drawing.svg".into(),
            kind: DocumentKind::Text,
            language_id: "xml".into(),
            revision: (1, 1),
            dirty: true,
            text: None,
            bytes: Arc::from(
                &br#"<svg xmlns="http://www.w3.org/2000/svg" width="2" height="3"/>"#[..],
            ),
        }];
        let entity = app
            .world_mut()
            .spawn((
                bedtime_api::workbench::PanelPresentation::new(9),
                PanelBinding {
                    input: PanelInput::Document(DocumentId(4)),
                    document: Some(DocumentId(4)),
                    session_id: 1,
                    ..Default::default()
                },
            ))
            .id();
        let factory = app
            .world()
            .resource::<bedtime_api::workbench::WorkbenchRegistry>()
            .panel(PANEL_ID)
            .unwrap()
            .factory;
        let mut frontend = app
            .world_mut()
            .run_system_with(
                factory,
                PanelCreation {
                    entity,
                    state: json!({"zoom":1000.0,"pan":[10.0,-20.0],"fit":false}),
                },
            )
            .unwrap()
            .unwrap();
        wait_image(&mut app, entity, [2, 3]);
        let status_context = bedtime_api::status::StatusContext {
            publisher: entity,
            item_id: "bedtime.image.document",
            focused_panel: None,
            focused_frontend: Some(entity),
            source: Some(entity),
            area: 0,
        };
        let status = crate::status::presentation(app.world(), status_context).unwrap();
        assert_eq!(status.summary, "2 × 3 · 3200%");
        assert!(status.details.contains("drawing.svg"));
        assert!(
            crate::status::presentation(
                app.world(),
                bedtime_api::status::StatusContext {
                    focused_frontend: None,
                    ..status_context
                }
            )
            .is_none()
        );
        assert!(
            app.world().get::<CanvasOutput>(entity).is_none(),
            "UI chooses canvas dimensions"
        );
        assert_eq!(frontend.title(app.world()), "drawing.svg");
        assert_eq!(frontend.save(app.world())["zoom"], 32.0);

        let mut context = Context::create();
        context
            .set_ini_filename(None::<std::path::PathBuf>)
            .unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        for _ in 0..2 {
            context.prepare_frame(FramePrepareOptions::new([420.0, 320.0], 1.0 / 60.0));
            let ui = context.frame();
            ui.window("Image world fixture")
                .position([0.0, 0.0], Condition::Always)
                .size([420.0, 320.0], Condition::Always)
                .build(|| {
                    frontend.draw(
                        ui,
                        app.world_mut(),
                        &mut Vec::new(),
                        &bedtime_api::workbench::ShortcutInput::default(),
                    );
                });
            assert!(context.render_legacy().total_vtx_count() > 0);
        }
        let initial = app.world().get::<CanvasOutput>(entity).unwrap().0;
        assert!(initial.size.into_iter().all(|size| size > 0));
        assert!(app.world().get::<CanvasRenderer>(entity).is_some());

        {
            let mut frame = app.world_mut().resource_mut::<DocumentFrame>();
            frame.documents[0].revision = (1, 2);
            frame.documents[0].bytes = Arc::from(
                &br#"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="5"/>"#[..],
            );
        }
        app.update();
        assert!(
            app.world().get::<CanvasOutput>(entity).is_none(),
            "stale pixels retire while decoding"
        );
        wait_image(&mut app, entity, [4, 5]);
        let updated = app.world().get::<CanvasOutput>(entity).unwrap().0;
        assert_eq!(updated.handle, initial.handle);
        assert!(updated.revision > initial.revision);
        let sibling = app
            .world_mut()
            .spawn(PanelBinding {
                input: PanelInput::Document(DocumentId(4)),
                document: Some(DocumentId(4)),
                ..Default::default()
            })
            .id();
        app.world_mut()
            .run_system_with(
                factory,
                PanelCreation {
                    entity: sibling,
                    state: Value::Null,
                },
            )
            .unwrap()
            .unwrap();
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
        app.world_mut().despawn(entity);
        app.update();
        assert!(!app.world().entities().contains(renderer.entity()));
        assert!(app.world().entities().contains(sibling_renderer.entity()));

        assert!(app.world().non_send::<ImageStates>().get(entity).is_none());
    }

    #[test]
    fn commands_preserve_the_selected_file_and_original_area() {
        let mut app = App::new();
        app.add_plugins(ImageViewerPlugin);
        let entity = app.world_mut().spawn(PanelBinding::default()).id();
        let command = app
            .world()
            .resource::<bedtime_api::workbench::WorkbenchRegistry>()
            .command(OPEN_FILE_COMMAND)
            .unwrap()
            .handler;
        app.world_mut()
            .run_system_with(
                command,
                CommandInvocation {
                    command: OPEN_FILE_COMMAND.into(),
                    source: Some(entity),
                    area: 15,
                    arguments: json!({"path":"selected.png"}),
                },
            )
            .unwrap()
            .unwrap();
        let requests = app.world_mut().resource_mut::<HostEffects>().take();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].source, Some(entity));
        assert_eq!(requests[0].area, Some(15));
        assert!(
            matches!(&requests[0].action, HostAction::OpenFile { path, viewer }
            if path == "selected.png" && viewer.as_deref() == Some(VIEWER_ID))
        );
    }
}
