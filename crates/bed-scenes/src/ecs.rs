//! Decorative panels own their scenes in the main Bevy world.
use crate::{SceneKind, SceneView, mascot::MascotPanel};
use bed_editor_api::{
    EditorFeatureSystems, EditorFrame, EditorIntents, EditorPanelBinding, HostRequest,
    Registry as EditorRegistry, gpu::GpuContext,
};
use bed_ui::gpu::{CanvasOutput, CanvasRenderInvocation, CanvasRenderResult, CanvasRenderer, wgpu};
use bed_workbench_api::{
    Action, CommandInvocation, CommandResult, Panel, PanelCreation, PanelResult,
};
use bevy::{ecs::system::SystemState, prelude::*};
use dear_imgui_rs::Ui;
use serde_json::Value;
use std::collections::HashMap;

#[derive(Component, Clone, Copy, Debug)]
pub struct ScenePanel(pub SceneKind);
#[derive(Default)]
pub struct SceneStates(HashMap<Entity, MascotPanel>);
impl SceneStates {
    pub fn get(&self, entity: Entity) -> Option<&SceneView> {
        self.0.get(&entity).map(|panel| &panel.scene)
    }
    #[cfg(test)]
    pub(crate) fn get_mut(&mut self, entity: Entity) -> Option<&mut SceneView> {
        self.0.get_mut(&entity).map(|panel| &mut panel.scene)
    }
}
#[derive(Resource)]
struct SceneSystemsInstalled;

fn initialize_scenes(app: &mut App) {
    app.init_resource::<EditorFrame>()
        .init_resource::<EditorIntents>()
        .init_resource::<EditorRegistry>()
        .init_non_send::<SceneStates>();
    if !app.world().contains_resource::<SceneSystemsInstalled>() {
        app.insert_resource(SceneSystemsInstalled).add_systems(
            PreUpdate,
            retire_scenes.in_set(EditorFeatureSystems::Prepare),
        );
    }
}

pub struct BedPlugin;
impl Plugin for BedPlugin {
    fn build(&self, app: &mut App) {
        initialize_scenes(app);
        app.world_mut()
            .resource_mut::<EditorRegistry>()
            .register_with("bed.mascot", crate::mascot::register_bed_metadata)
            .expect("Bed declarations must register exactly once");
        bed_workbench_api::register(app.world_mut(), "bed.mascot", |registrar| {
            registrar.panel("bed.mascot.panel", "Bed", create_bed);
            registrar.command("bed.mascot.open", "Bed", bed_command);
            registrar.menu("bed.mascot.open");
        })
        .expect("Bed systems must register exactly once");
    }
}
pub struct DuckPlugin;
impl Plugin for DuckPlugin {
    fn build(&self, app: &mut App) {
        initialize_scenes(app);
        app.world_mut()
            .resource_mut::<EditorRegistry>()
            .register_with("bed.duck", crate::mascot::register_duck_metadata)
            .expect("Duck declarations must register exactly once");
        bed_workbench_api::register(app.world_mut(), "bed.duck", |registrar| {
            registrar.panel(crate::mascot::PANEL_ID, "Ducky", create_duck);
        })
        .expect("Duck systems must register exactly once");
    }
}

fn create_bed(In(creation): In<PanelCreation>, world: &mut World) -> PanelResult {
    create_scene(creation, world, SceneKind::Bed)
}
fn create_duck(In(creation): In<PanelCreation>, world: &mut World) -> PanelResult {
    create_scene(creation, world, SceneKind::Duck)
}
fn create_scene(creation: PanelCreation, world: &mut World, kind: SceneKind) -> PanelResult {
    if world
        .get::<EditorPanelBinding>(creation.entity)
        .is_some_and(|binding| {
            binding.document.is_some() || !matches!(binding.input, bed_editor_api::PanelInput::None)
        })
    {
        return Err("Mascots are tool panels without document or local-file input".into());
    }
    let render = world.register_system(render_scene);
    world
        .entity_mut(render.entity())
        .insert(ChildOf(creation.entity));
    world.entity_mut(creation.entity).insert((
        ScenePanel(kind),
        CanvasRenderer(render),
        bed_workbench_api::PanelPadding([0.0; 2]),
    ));
    world
        .non_send_mut::<SceneStates>()
        .0
        .insert(creation.entity, MascotPanel::new(kind));
    Ok(Box::new(SceneFrontend {
        entity: creation.entity,
    }))
}
fn bed_command(
    In(invocation): In<CommandInvocation>,
    mut intents: ResMut<EditorIntents>,
) -> CommandResult {
    if invocation.command != "bed.mascot.open" {
        return Err(format!("Unknown Bed command: {}", invocation.command));
    }
    intents.push_at(
        invocation.source,
        invocation.area,
        HostRequest::OpenPanel {
            panel_type: "bed.mascot.panel".into(),
            document: None,
            state: Value::Null,
        },
    );
    Ok(Vec::new())
}
fn retire_scenes(mut states: NonSendMut<SceneStates>, live: Query<&ScenePanel>) {
    states.0.retain(|entity, _| live.get(*entity).is_ok());
}

fn render_scene(
    In(invocation): In<CanvasRenderInvocation>,
    world: &mut World,
) -> CanvasRenderResult {
    if world.get::<ScenePanel>(invocation.entity).is_none() {
        return Ok(Vec::new());
    }
    // Only the derived GPU renderer is moved into this render transaction.
    // SceneStates and the scene's authoritative frame remain available to systems.
    let (mut renderer, kind, handle, frame, time, impulse) = {
        let mut states = world.non_send_mut::<SceneStates>();
        let scene = &mut states
            .0
            .get_mut(&invocation.entity)
            .expect("Scene entity owns its state")
            .scene;
        if scene.failed_generation == Some(invocation.generation) {
            return Ok(Vec::new());
        }
        (
            scene.renderer.take(),
            scene.kind,
            scene.texture.0,
            scene.frame,
            scene.time,
            scene.impulse,
        )
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut encoder =
            invocation
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("Bed decorative scene system"),
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
        if renderer.is_none() {
            renderer = Some(crate::renderer::SceneRenderer::new(&mut gpu, kind, handle)?);
        }
        let renderer = renderer.as_mut().expect("scene renderer prepared");
        renderer.generation = invocation.generation;
        renderer.sync(&mut gpu, &invocation.target, frame, time, impulse)?;
        Ok(vec![encoder.finish()])
    }));
    {
        let mut states = world.non_send_mut::<SceneStates>();
        let scene = &mut states
            .0
            .get_mut(&invocation.entity)
            .expect("Scene entity owns its state")
            .scene;
        if matches!(&result, Ok(Ok(_))) {
            scene.renderer = renderer;
            scene.failed_generation = None;
        } else {
            scene.failed_generation = Some(invocation.generation);
        }
    }
    match result {
        Ok(result) => result,
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

struct SceneFrontend {
    entity: Entity,
}
impl Panel for SceneFrontend {
    fn title(&self, world: &World) -> String {
        world
            .non_send::<SceneStates>()
            .0
            .get(&self.entity)
            .expect("A scene presentation owns its runtime state")
            .panel_title(&world.resource::<EditorFrame>().context())
    }
    fn draw(
        &mut self,
        ui: &Ui,
        world: &mut World,
        _: &mut Vec<Action>,
        _shortcuts: &bed_workbench_api::ShortcutInput,
    ) {
        let output = {
            let mut state = SystemState::<(
                Res<EditorFrame>,
                NonSendMut<SceneStates>,
                ResMut<EditorIntents>,
            )>::new(world);
            let (frame, mut states, mut intents) = state
                .get_mut(world)
                .expect("Scene plugins initialized their presentation resources");
            let panel = states
                .0
                .get_mut(&self.entity)
                .expect("A scene presentation owns its runtime state");
            panel.draw_canvas(ui, &frame.context());
            if frame.animations || panel.scene.settle > 0 {
                intents.push(Some(self.entity), HostRequest::Invalidate);
            }
            panel.scene.render_output()
        };
        if let Some(output) = output {
            world.entity_mut(self.entity).insert(CanvasOutput(output));
        }
    }
    fn save(&self, _: &World) -> Value {
        Value::Null
    }
}

/// Spawn an owned transient scene surface, for example the idle overlay.
pub fn spawn_scene(world: &mut World, kind: SceneKind) -> Entity {
    let entity = world.spawn_empty().id();
    create_scene(
        PanelCreation {
            entity,
            state: Value::Null,
        },
        world,
        kind,
    )
    .expect("transient scene requires no document");
    entity
}
pub fn update_scene(world: &mut World, entity: Entity, pixels: [u32; 2], frame: crate::SceneFrame) {
    let output = {
        let mut states = world.non_send_mut::<SceneStates>();
        let scene = &mut states.0.get_mut(&entity).expect("live scene entity").scene;
        scene.update(pixels, frame);
        scene.render_output()
    };
    if let Some(output) = output {
        world.entity_mut(entity).insert(CanvasOutput(output));
    }
}

/// GPU scene support without contributing a workbench panel or menu item.
pub struct SceneRenderingPlugin;
impl Plugin for SceneRenderingPlugin {
    fn build(&self, app: &mut App) {
        initialize_scenes(app);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use bed_editor_api::PanelInput;
    #[test]
    fn each_scene_presentation_owns_its_registered_renderer() {
        let mut app = App::new();
        app.add_plugins(BedPlugin);
        let factory = app
            .world()
            .resource::<bed_workbench_api::Registry>()
            .panel("bed.mascot.panel")
            .unwrap()
            .factory;
        let first = app.world_mut().spawn_empty().id();
        let second = app.world_mut().spawn_empty().id();
        for entity in [first, second] {
            app.world_mut()
                .run_system_with(
                    factory,
                    PanelCreation {
                        entity,
                        state: Value::Null,
                    },
                )
                .unwrap()
                .unwrap();
        }
        let first_renderer = app.world().get::<CanvasRenderer>(first).unwrap().0;
        let second_renderer = app.world().get::<CanvasRenderer>(second).unwrap().0;
        assert_ne!(first_renderer, second_renderer);
        assert_eq!(
            app.world()
                .get::<ChildOf>(first_renderer.entity())
                .unwrap()
                .parent(),
            first
        );
        app.world_mut().despawn(first);
        app.update();
        assert!(!app.world().entities().contains(first_renderer.entity()));
        assert!(app.world().entities().contains(second_renderer.entity()));
        assert!(app.world().non_send::<SceneStates>().get(first).is_none());
        assert!(app.world().non_send::<SceneStates>().get(second).is_some());
    }
    #[test]
    fn independent_scene_plugins_register_tool_panels_and_retire_world_state() {
        let mut app = App::new();
        app.add_plugins((BedPlugin, DuckPlugin));
        assert_eq!(app.world().resource::<EditorRegistry>().menus().len(), 1);
        for (id, kind, title) in [
            ("bed.mascot.panel", SceneKind::Bed, "Bed"),
            ("bed.duck.panel", SceneKind::Duck, "Ducky"),
        ] {
            let entity = app.world_mut().spawn_empty().id();
            let factory = app
                .world()
                .resource::<bed_workbench_api::Registry>()
                .panel(id)
                .unwrap()
                .factory;
            let frontend = app
                .world_mut()
                .run_system_with(
                    factory,
                    PanelCreation {
                        entity,
                        state: serde_json::json!({"muted":true}),
                    },
                )
                .unwrap()
                .unwrap();
            assert_eq!(frontend.title(app.world()), title);
            assert_eq!(frontend.save(app.world()), Value::Null);
            assert_eq!(app.world().get::<ScenePanel>(entity).unwrap().0, kind);
            assert!(app.world().get::<CanvasRenderer>(entity).is_some());
            assert!(app.world().non_send::<SceneStates>().get(entity).is_some());
            app.world_mut().entity_mut(entity).despawn();
            app.update();
            assert!(app.world().non_send::<SceneStates>().get(entity).is_none());
            let document_entity = app
                .world_mut()
                .spawn(EditorPanelBinding {
                    input: PanelInput::Document(bed_document_session::DocumentId(4)),
                    ..Default::default()
                })
                .id();
            assert!(
                app.world_mut()
                    .run_system_with(
                        factory,
                        PanelCreation {
                            entity: document_entity,
                            state: Value::Null
                        }
                    )
                    .unwrap()
                    .is_err()
            );
        }
    }
    #[test]
    fn opening_bed_preserves_the_command_origin() {
        let mut app = App::new();
        app.add_plugins(BedPlugin);
        let source = app.world_mut().spawn_empty().id();
        let handler = app
            .world()
            .resource::<bed_workbench_api::Registry>()
            .command("bed.mascot.open")
            .unwrap()
            .handler;
        app.world_mut()
            .run_system_with(
                handler,
                CommandInvocation {
                    command: "bed.mascot.open".into(),
                    source: Some(source),
                    area: 7,
                    arguments: Value::Null,
                },
            )
            .unwrap()
            .unwrap();
        let requests = app.world_mut().resource_mut::<EditorIntents>().take();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].source, Some(source));
        assert_eq!(requests[0].area, Some(7));
        assert!(
            matches!(&requests[0].request,HostRequest::OpenPanel{panel_type,document:None,..} if panel_type == "bed.mascot.panel")
        );
    }
}
