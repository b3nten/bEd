//! Audio documents, workers and render systems are hosted by the main world.
use crate::{
    AudioPanel, OPEN_COMMAND, OPEN_FILE_COMMAND, PANEL_ID, PLUGIN_ID, VIEWER_ID, register_metadata,
    supported_path,
};
use bed_api::documents::DocumentFrame;
use bed_api::features::FrontendSnapshot;
use bed_api::gpu::GpuContext;
use bed_api::workbench::{
    CommandInvocation, CommandResult, Panel, PanelCreation, PanelResult, WorkbenchAction,
};
use bed_api::{
    documents::PanelBinding,
    features::{FeatureRegistry, FeatureSystems, HostAction, HostEffects},
};
use bed_editing::identity::DocumentId;
use bed_ui::gpu::{
    CanvasOutput, CanvasRenderInvocation, CanvasRenderResult, CanvasRenderer, RenderOutput, wgpu,
};
use bevy::{ecs::system::SystemState, prelude::*};
use dear_imgui_rs::Ui;
use serde_json::Value;
use std::collections::HashMap;

#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioDocument(pub DocumentId);

#[derive(Default)]
pub struct AudioStates(HashMap<Entity, AudioPanel>);
impl AudioStates {
    pub fn get(&self, entity: Entity) -> Option<&AudioPanel> {
        self.0.get(&entity)
    }
    #[cfg(test)]
    pub(crate) fn get_mut(&mut self, entity: Entity) -> Option<&mut AudioPanel> {
        self.0.get_mut(&entity)
    }
}

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AudioSystems {
    Poll,
}

pub struct AudioPlayerPlugin;

impl Plugin for AudioPlayerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DocumentFrame>()
            .init_resource::<HostEffects>()
            .init_resource::<FeatureRegistry>()
            .init_non_send::<AudioStates>()
            .add_systems(
                PreUpdate,
                poll_audios
                    .in_set(AudioSystems::Poll)
                    .in_set(FeatureSystems::Prepare),
            )
            .add_systems(Last, shutdown.after(bevy::app::OnAppExitSystems));
        app.world_mut()
            .resource_mut::<FeatureRegistry>()
            .register_with(PLUGIN_ID, register_metadata)
            .expect("Audio declarations must register exactly once");
        bed_api::workbench::register(app.world_mut(), PLUGIN_ID, |registrar| {
            registrar.panel(PANEL_ID, "Audio Viewer", create_audio_panel);
            for action in crate::shortcut_actions() {
                registrar.shortcut_action(action);
            }
            registrar.command(OPEN_COMMAND, "Open Audio…", audio_command);
            registrar.command(OPEN_FILE_COMMAND, "Open in Audio Viewer", audio_command);
            registrar.command(crate::status::PLAY_COMMAND, "Play audio", playback_command);
            registrar.command(
                crate::status::PAUSE_COMMAND,
                "Pause audio",
                playback_command,
            );
            registrar.menu(OPEN_COMMAND);
        })
        .expect("Audio systems must register exactly once");
    }
}

fn shutdown(mut exits: MessageReader<bevy::app::AppExit>, mut states: NonSendMut<AudioStates>) {
    if exits.read().next().is_some() {
        states.0.clear();
    }
}

fn create_audio_panel(In(creation): In<PanelCreation>, world: &mut World) -> PanelResult {
    world.entity_mut(creation.entity).insert(bed_api::workbench::PanelDocumentation::new(&[
        ("Playback", "Use Play/Pause, Restart, and Volume to control audio. Drag the seek slider or click/drag the waveform to seek."),
        ("Analysis", "Expand Waveform, Spectrum, or Spectrogram to inspect the sound."),
    ]));
    if world
        .get::<PanelBinding>(creation.entity)
        .is_some_and(|binding| binding.input.local_file().is_some())
    {
        return Err("Audio Viewer requires an editor document".into());
    }
    let document = world
        .get::<PanelBinding>(creation.entity)
        .and_then(|binding| binding.input.document().or(binding.document))
        .or_else(|| creation.state["document"].as_u64().map(DocumentId));
    if let Some(document) = document {
        let render = world.register_system(render_audio);
        world
            .entity_mut(render.entity())
            .insert(ChildOf(creation.entity));
        world.entity_mut(creation.entity).insert((
            AudioDocument(document),
            CanvasRenderer(render),
            crate::status::items(),
        ));
        world
            .non_send_mut::<AudioStates>()
            .0
            .insert(creation.entity, AudioPanel::new(document, &creation.state));
    }
    Ok(Box::new(AudioFrontend {
        entity: creation.entity,
    }))
}

fn poll_audios(world: &mut World) {
    let outputs = {
        let frame = FrontendSnapshot::capture(world);
        let mut state = SystemState::<(
            NonSendMut<AudioStates>,
            Query<&AudioDocument>,
            ResMut<HostEffects>,
        )>::new(world);
        let (mut states, live, mut intents) = state
            .get_mut(world)
            .expect("Audio plugin initialized its preparation resources");
        states.0.retain(|entity, _| live.get(*entity).is_ok());
        let host = frame.context();
        for panel in states.0.values_mut() {
            panel.sync(&host);
        }
        if states.0.values().any(|panel| {
            host.document(panel.document).is_some()
                && ((panel.audio.is_none() && panel.error.is_none())
                    || (panel.audio.is_some()
                        && panel.analysis.is_none()
                        && panel.analysis_error.is_none())
                    || panel.spectrum_pending()
                    || panel.playing())
        }) {
            intents.push(None, HostAction::Invalidate);
        }
        states
            .0
            .iter()
            .map(|(entity, panel)| {
                (
                    *entity,
                    panel.canvas_output(),
                    match panel.error.as_ref().or(panel.analysis_error.as_ref()) {
                        Some(error) => bed_api::workbench::PanelReadiness::Failed(error.clone()),
                        None if panel.audio.is_some()
                            && panel.analysis.is_some()
                            && (!panel.spectrogram_visible || panel.canvas_output().is_some()) =>
                        {
                            bed_api::workbench::PanelReadiness::Ready
                        }
                        None => bed_api::workbench::PanelReadiness::Pending,
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

fn playback_command(
    In(invocation): In<CommandInvocation>,
    frame: Res<DocumentFrame>,
    live: Query<&AudioDocument>,
    mut states: NonSendMut<AudioStates>,
    mut intents: ResMut<HostEffects>,
) -> CommandResult {
    let revision: bed_api::documents::Revision =
        serde_json::from_value(invocation.arguments["revision"].clone())
            .map_err(|_| "Audio control requires its captured document revision".to_owned())?;
    let Some(entity) = invocation.source else {
        return Ok(Vec::new());
    };
    if live.get(entity).is_err() {
        return Ok(Vec::new());
    }
    let Some(panel) = states.0.get_mut(&entity) else {
        return Ok(Vec::new());
    };
    if panel.revision != Some(revision)
        || !frame
            .documents
            .iter()
            .any(|document| document.id == panel.document && document.revision == revision)
    {
        return Ok(Vec::new());
    }
    match invocation.command.as_str() {
        crate::status::PLAY_COMMAND if !panel.playing() => panel.toggle(),
        crate::status::PAUSE_COMMAND if panel.playing() => panel.toggle(),
        crate::status::PLAY_COMMAND | crate::status::PAUSE_COMMAND => {}
        _ => return Err(format!("Unknown audio control: {}", invocation.command)),
    }
    intents.push(Some(entity), HostAction::Invalidate);
    Ok(Vec::new())
}

fn audio_command(
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
        _ => return Err(format!("Unknown audio command: {}", invocation.command)),
    };
    if let Some(request) = request {
        intents.push_at(invocation.source, invocation.area, request);
    }
    Ok(Vec::new())
}

fn render_audio(
    In(invocation): In<CanvasRenderInvocation>,
    mut states: NonSendMut<AudioStates>,
    live: Query<&AudioDocument>,
) -> CanvasRenderResult {
    if live.get(invocation.entity).is_err() {
        return Ok(Vec::new());
    }
    let panel = states
        .0
        .get_mut(&invocation.entity)
        .expect("An audio panel owns its runtime state");
    let mut encoder = invocation
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Bed audio canvas system"),
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

struct AudioFrontend {
    entity: Entity,
}

impl Panel for AudioFrontend {
    fn title(&self, world: &World) -> String {
        world
            .non_send::<AudioStates>()
            .get(self.entity)
            .map_or_else(
                || "Audio Viewer".into(),
                |panel| panel.panel_title(&FrontendSnapshot::capture(world).context()),
            )
    }

    fn draw(
        &mut self,
        ui: &Ui,
        world: &mut World,
        _requests: &mut Vec<WorkbenchAction>,
        _shortcuts: &bed_api::workbench::ShortcutInput,
    ) {
        let output = {
            let frame = FrontendSnapshot::capture(world);
            let mut state =
                SystemState::<(NonSendMut<AudioStates>, ResMut<HostEffects>)>::new(world);
            let (mut states, mut intents) = state
                .get_mut(world)
                .expect("Audio plugin initialized its presentation resources");
            let mut requests = Vec::new();
            let output = if let Some(panel) = states.0.get_mut(&self.entity) {
                panel.shortcuts = _shortcuts.clone();
                panel.shortcuts = _shortcuts.clone();
                panel.draw_prepared(ui, &frame.context(), &mut requests);
                panel.canvas_output()
            } else {
                ui.text_wrapped("Open an audio to view it.");
                if ui.button("Open Audio…") {
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
            .non_send::<AudioStates>()
            .get(self.entity)
            .map_or(Value::Null, AudioPanel::saved_state)
    }
}

#[cfg(test)]
mod ecs_tests {
    use super::*;
    use bed_api::documents::{DocumentKind, PanelInput, PluginDocument};
    use serde_json::json;
    use std::{
        sync::Arc,
        thread,
        time::{Duration, Instant},
    };

    fn document(bytes: Arc<[u8]>, revision: u64) -> PluginDocument {
        PluginDocument {
            id: DocumentId(4),
            path: "tone.wav".into(),
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
            .spawn(PanelBinding {
                input: PanelInput::Document(DocumentId(4)),
                document: Some(DocumentId(4)),
                session_id: 1,
                ..Default::default()
            })
            .id();
        let factory = app
            .world()
            .resource::<bed_api::workbench::WorkbenchRegistry>()
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

    fn wait(app: &mut App, entity: Entity, ready: impl Fn(&AudioPanel) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            app.update();
            if ready(app.world().non_send::<AudioStates>().get(entity).unwrap()) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "Audio preparation did not finish"
            );
            thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn commands_keep_the_origin_path_and_area() {
        let mut app = App::new();
        app.add_plugins(AudioPlayerPlugin);
        app.world_mut().resource_mut::<DocumentFrame>().documents =
            vec![document(Arc::from(&b"unused"[..]), 1)];
        let (entity, _) = create(&mut app, Value::Null);
        let handler = app
            .world()
            .resource::<bed_api::workbench::WorkbenchRegistry>()
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
                    arguments: json!({"path": "context.FLAC"}),
                },
            )
            .unwrap()
            .unwrap();
        let requests = app.world_mut().resource_mut::<HostEffects>().take();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].source, Some(entity));
        assert_eq!(requests[0].area, Some(19));
        assert!(
            matches!(&requests[0].action, HostAction::OpenFile {path, viewer} if path == "context.FLAC" && viewer.as_deref() == Some(VIEWER_ID))
        );
    }

    #[test]
    fn preparation_decodes_and_analyzes_hidden_audio_and_never_autoplays() {
        let mut app = App::new();
        app.add_plugins(AudioPlayerPlugin);
        app.world_mut().resource_mut::<DocumentFrame>().documents = vec![document(
            crate::tests::wav(&[1000; 8000], 1, 8000).into(),
            1,
        )];
        let (entity, frontend) = create(&mut app, json!({"position":0.5,"volume":0.25}));
        wait(&mut app, entity, |panel| panel.analysis.is_some());
        let panel = app.world().non_send::<AudioStates>().get(entity).unwrap();
        assert!(panel.playback.is_none());
        assert_eq!(panel.audio.as_ref().unwrap().duration(), 1.0);
        assert_eq!(frontend.save(app.world())["position"], 0.5);
        let status_context = bed_api::status::StatusContext {
            publisher: entity,
            item_id: "bed.audio.document",
            focused_panel: None,
            focused_frontend: Some(entity),
            source: Some(entity),
            area: 19,
        };
        let status = crate::status::presentation(app.world(), status_context).unwrap();
        assert!(status.summary.contains("Paused"));
        assert!(
            status
                .details
                .contains("8000 Hz · 1 channels · 8000 frames")
        );
        assert!(status.details.contains("Volume 25%"));
        assert!(
            crate::status::presentation(
                app.world(),
                bed_api::status::StatusContext {
                    item_id: "bed.audio.playback",
                    ..status_context
                }
            )
            .is_none()
        );
        assert!(
            crate::status::presentation(
                app.world(),
                bed_api::status::StatusContext {
                    focused_frontend: None,
                    ..status_context
                }
            )
            .is_none()
        );
        let play = &status.commands[0];
        assert_eq!(play.command, crate::status::PLAY_COMMAND);
        let handler = app
            .world()
            .resource::<bed_api::workbench::WorkbenchRegistry>()
            .command(play.command)
            .unwrap()
            .handler;
        app.world_mut().resource_mut::<DocumentFrame>().documents[0].revision = (1, 2);
        app.world_mut()
            .run_system_with(
                handler,
                CommandInvocation {
                    command: play.command.into(),
                    source: Some(entity),
                    area: 19,
                    arguments: play.arguments.clone(),
                },
            )
            .unwrap()
            .unwrap();
        assert!(
            app.world()
                .non_send::<AudioStates>()
                .get(entity)
                .unwrap()
                .playback
                .is_none(),
            "stale Play must not start audio output"
        );
        app.world_mut().resource_mut::<DocumentFrame>().documents[0].revision = (1, 1);
        app.world_mut()
            .non_send_mut::<AudioStates>()
            .get_mut(entity)
            .unwrap()
            .spectrogram_visible = true;
        app.update();
        assert!(app.world().get::<CanvasOutput>(entity).is_some());
        app.world_mut().resource_mut::<DocumentFrame>().documents[0] =
            document(crate::tests::wav(&[0; 4000], 1, 8000).into(), 2);
        app.update();
        assert!(app.world().get::<CanvasOutput>(entity).is_none());
        wait(&mut app, entity, |panel| panel.analysis.is_some());
        let panel = app.world().non_send::<AudioStates>().get(entity).unwrap();
        assert!(panel.playback.is_none());
        assert_eq!(panel.audio.as_ref().unwrap().duration(), 0.5);
        assert_eq!(frontend.save(app.world())["position"], 0.0);
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

        assert!(app.world().non_send::<AudioStates>().get(entity).is_none());
    }

    #[test]
    fn rejected_audio_stops_requesting_frames_and_paused_spectrum_finishes_in_prepare() {
        let mut app = App::new();
        app.add_plugins(AudioPlayerPlugin);
        app.world_mut().resource_mut::<DocumentFrame>().documents = vec![document(
            crate::tests::wav(&[1000; 8000], 1, 8000).into(),
            1,
        )];
        let (entity, _) = create(&mut app, Value::Null);
        wait(&mut app, entity, |panel| {
            panel.analysis.is_some() && !panel.spectrum_pending()
        });
        {
            let mut states = app.world_mut().non_send_mut::<AudioStates>();
            let panel = states.get_mut(entity).unwrap();
            panel.seek(0.5);
            assert!(panel.spectrum_pending());
            assert!(panel.playback.is_none());
        }
        wait(&mut app, entity, |panel| !panel.spectrum_pending());
        app.world_mut().resource_mut::<DocumentFrame>().documents[0] =
            document(Arc::from(&b"corrupt audio"[..]), 2);
        wait(&mut app, entity, |panel| panel.error.is_some());
        app.world_mut().resource_mut::<HostEffects>().take();
        app.update();
        assert!(
            app.world_mut()
                .resource_mut::<HostEffects>()
                .take()
                .is_empty()
        );
    }
}
