//! Workspace observations live in Bevy; each launcher owns only widget drafts.
use super::{MODULE_ID, NEW_COMMAND, PANEL_ID, ProjectsState, project_requests, welcome::Welcome};
use bed_editor_api::{EditorFrame, EditorIntents, HostRequest, ProjectRequest};
use bed_workbench_api::{
    Action, CommandInvocation, CommandResult, Panel, PanelCreation, PanelPersistence, PanelResult,
    register,
};
use bevy::prelude::*;
use dear_imgui_rs::Ui;
use serde_json::Value;

pub struct ProjectsPlugin;
impl Plugin for ProjectsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<bed_editor_api::Registry>();
        app.world_mut()
            .resource_mut::<bed_editor_api::Registry>()
            .register_with(MODULE_ID, |registrar| {
                registrar.panel_options(
                    PANEL_ID,
                    "Startup",
                    false,
                    bed_editor_api::PanelPlacement::Center,
                    Some("projects"),
                );
                registrar.command(NEW_COMMAND, "New Startup Panel", Some("folder"));
            })
            .expect("feature metadata is installed once");
        app.init_resource::<ProjectsState>()
            .init_resource::<EditorIntents>()
            .add_systems(
                PreUpdate,
                observe_workspace.in_set(bed_editor_api::EditorFeatureSystems::Prepare),
            );
        register(app.world_mut(), MODULE_ID, |registrar| {
            registrar.panel(PANEL_ID, "Startup", create_panel);
            registrar.command(NEW_COMMAND, "New Startup Panel", open_panel);
        })
        .expect("ProjectsPlugin must be installed once");
    }
}

fn observe_workspace(frame: Option<Res<EditorFrame>>, mut state: ResMut<ProjectsState>) {
    if let Some(frame) = frame {
        state.recent_workspaces.clone_from(&frame.recent_workspaces);
        state.connecting = frame.remote_connecting;
        state.disconnected = frame.remote_disconnected;
    }
}

/// Identifies live launchers independently of their main-thread UI objects.
#[derive(Component)]
pub struct ProjectsPanelState;

pub struct ProjectsFrontend {
    pub entity: Entity,
    welcome: Welcome,
}

fn open_panel(In(invocation): In<CommandInvocation>) -> CommandResult {
    Ok(vec![Action::OpenPanel {
        panel_type: PANEL_ID.into(),
        state: Value::Null,
        area: Some(invocation.area),
    }])
}

fn create_panel(In(creation): In<PanelCreation>, world: &mut World) -> PanelResult {
    world
        .entity_mut(creation.entity)
        .insert((ProjectsPanelState, PanelPersistence(false)));
    #[cfg(feature = "scenes")]
    {
        // A panic must retire only this preview's registered system. The
        // child relationship releases it when its presentation closes.
        let renderer = world.register_system(render_scene);
        world
            .entity_mut(renderer.entity())
            .insert(ChildOf(creation.entity));
        world.entity_mut(creation.entity).insert((
            ProjectsPreview(bed_scenes::SceneView::new(
                bed_scenes::SceneKind::WelcomeBed,
            )),
            bed_ui::gpu::CanvasRenderer(renderer),
        ));
    }
    Ok(Box::new(ProjectsFrontend {
        entity: creation.entity,
        welcome: Welcome::frontend(),
    }))
}

impl Panel for ProjectsFrontend {
    fn title(&self, _: &World) -> String {
        "Startup".into()
    }

    fn draw(
        &mut self,
        ui: &Ui,
        world: &mut World,
        _: &mut Vec<Action>,
        _shortcuts: &bed_workbench_api::ShortcutInput,
    ) {
        let state = world.resource::<ProjectsState>();
        if state.connecting {
            ui.text_disabled("Connecting over SSH…");
        }
        let reconnect = state.disconnected && {
            ui.text_disabled("SSH connection lost. Your buffers are retained.");
            ui.button("Reconnect")
        };
        self.welcome
            .set_recent_workspaces(state.recent_workspaces.clone());
        let frame = world
            .get_resource::<EditorFrame>()
            .cloned()
            .unwrap_or_default();
        #[cfg(feature = "scenes")]
        let action = {
            let mut scene = world
                .get_mut::<ProjectsPreview>(self.entity)
                .expect("a launcher owns its preview state");
            self.welcome
                .draw_body_with_scene(ui, &frame.context(), Some(&mut scene.0))
        };
        #[cfg(not(feature = "scenes"))]
        let action = self.welcome.draw_body_with_scene(ui, &frame.context());
        #[cfg(feature = "scenes")]
        if let Some(output) = world
            .get::<ProjectsPreview>(self.entity)
            .and_then(|preview| preview.0.render_output())
        {
            world
                .entity_mut(self.entity)
                .insert(bed_ui::gpu::CanvasOutput(output));
        }
        let mut intents = world.resource_mut::<EditorIntents>();
        if reconnect {
            intents.push(
                Some(self.entity),
                HostRequest::Project(ProjectRequest::Reconnect),
            );
        }
        for request in project_requests(action) {
            intents.push(Some(self.entity), HostRequest::Project(request));
        }
    }
}

#[cfg(feature = "scenes")]
#[derive(Component)]
pub struct ProjectsPreview(pub bed_scenes::SceneView);

#[cfg(feature = "scenes")]
fn render_scene(
    In(invocation): In<bed_ui::gpu::CanvasRenderInvocation>,
    world: &mut World,
) -> bed_ui::gpu::CanvasRenderResult {
    use bed_editor_api::gpu::GpuContext;
    // The renderer synchronizes entities in the same world. Temporarily take
    // its component, restoring it even if a backend panic unwinds this call.
    let mut scene = world
        .get_entity_mut(invocation.entity)
        .map_err(|_| "Startup preview is no longer open")?
        .take::<ProjectsPreview>()
        .ok_or("Startup preview is no longer open")?;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut encoder = invocation
            .device
            .create_command_encoder(&Default::default());
        scene.0.render(
            &mut GpuContext {
                world: Some(world),
                instance: &invocation.instance,
                adapter: &invocation.adapter,
                device: &invocation.device,
                queue: &invocation.queue,
                encoder: &mut encoder,
                error_handlers: None,
                generation: invocation.generation,
            },
            &invocation.target,
        )?;
        Ok(vec![encoder.finish()])
    }));
    world.entity_mut(invocation.entity).insert(scene);
    match result {
        Ok(result) => result,
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bed_editor_api::workspace::WorkspaceSpec;
    use bed_workbench::{WorkbenchLayout, WorkbenchPlugin, WorkbenchRequests, capture_command};
    use dear_imgui_rs::{Condition, Context, FramePrepareOptions, MouseButton};

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins((WorkbenchPlugin, ProjectsPlugin));
        app
    }

    fn open(app: &mut App) -> Entity {
        let command = capture_command(app.world(), NEW_COMMAND, Value::Null);
        app.world_mut()
            .resource_mut::<WorkbenchRequests>()
            .push(Action::Invoke(command));
        app.update();
        assert!(
            app.world_mut()
                .resource_mut::<WorkbenchRequests>()
                .take_errors()
                .is_empty()
        );
        app.world().resource::<WorkbenchLayout>().focused.unwrap()
    }

    #[test]
    fn launchers_share_world_observations_and_are_omitted_from_sessions() {
        let mut app = app();
        let first = open(&mut app);
        let second = open(&mut app);
        assert_ne!(first, second);
        assert!(app.world().get::<ProjectsPanelState>(first).is_some());
        assert!(app.world().get::<ProjectsPanelState>(second).is_some());
        app.world_mut()
            .resource_mut::<ProjectsState>()
            .recent_workspaces
            .push(WorkspaceSpec::local("/example/project"));
        assert!(
            bed_workbench::save(app.world_mut())["panels"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        #[cfg(feature = "scenes")]
        {
            let first_preview = app.world().get::<ProjectsPreview>(first).unwrap();
            let second_preview = app.world().get::<ProjectsPreview>(second).unwrap();
            assert_ne!(first_preview.0.handle(), second_preview.0.handle());
            assert_ne!(
                app.world()
                    .get::<bed_ui::gpu::CanvasRenderer>(first)
                    .unwrap()
                    .0,
                app.world()
                    .get::<bed_ui::gpu::CanvasRenderer>(second)
                    .unwrap()
                    .0
            );
            for panel in [first, second] {
                let renderer = app
                    .world()
                    .get::<bed_ui::gpu::CanvasRenderer>(panel)
                    .unwrap()
                    .0;
                assert_eq!(
                    app.world()
                        .get::<ChildOf>(renderer.entity())
                        .unwrap()
                        .parent(),
                    panel
                );
            }
            let first_renderer = app
                .world()
                .get::<bed_ui::gpu::CanvasRenderer>(first)
                .unwrap()
                .0
                .entity();
            let second_renderer = app
                .world()
                .get::<bed_ui::gpu::CanvasRenderer>(second)
                .unwrap()
                .0
                .entity();
            app.world_mut()
                .resource_mut::<WorkbenchRequests>()
                .push(Action::ClosePanel(first));
            app.update();
            assert!(!app.world().entities().contains(first_renderer));
            assert!(app.world().entities().contains(second_renderer));
        }
    }

    #[test]
    fn frontend_queues_owned_project_intentions_from_its_originating_entity() {
        fn frame(context: &mut Context, frontend: &mut ProjectsFrontend, world: &mut World) {
            context.prepare_frame(FramePrepareOptions::new([1000.0, 900.0], 1.0 / 60.0));
            let ui = context.frame();
            ui.window("Bevy project fixture")
                .position([0.0; 2], Condition::Always)
                .size([1000.0, 900.0], Condition::Always)
                .build(|| {
                    frontend.draw(
                        ui,
                        world,
                        &mut Vec::new(),
                        &bed_workbench_api::ShortcutInput::default(),
                    )
                });
            drop(context.render_legacy());
        }
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = Context::create();
        context
            .set_ini_filename(None::<std::path::PathBuf>)
            .unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        let mut app = app();
        let first = open(&mut app);
        let second = open(&mut app);
        let workspace = WorkspaceSpec::local("/example/project");
        app.world_mut()
            .resource_mut::<ProjectsState>()
            .recent_workspaces
            .push(workspace.clone());
        let mut frontend = ProjectsFrontend {
            entity: first,
            welcome: Welcome::frontend(),
        };
        frame(&mut context, &mut frontend, app.world_mut());
        #[cfg(feature = "scenes")]
        assert_eq!(
            app.world()
                .get::<bed_ui::gpu::CanvasOutput>(first)
                .unwrap()
                .0
                .handle,
            app.world()
                .get::<ProjectsPreview>(first)
                .unwrap()
                .0
                .handle()
        );
        context
            .io_mut()
            .add_mouse_pos_event(frontend.welcome.item_center("open_folder"));
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, true);
        frame(&mut context, &mut frontend, app.world_mut());
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, false);
        frame(&mut context, &mut frontend, app.world_mut());
        let intents = app.world_mut().resource_mut::<EditorIntents>().take();
        assert!(matches!(intents.as_slice(), [intent]
            if intent.source == Some(first) && matches!(intent.request, HostRequest::Project(ProjectRequest::OpenFolder))));
        assert_eq!(
            app.world().resource::<ProjectsState>().recent_workspaces,
            vec![workspace]
        );
        assert!(app.world().get::<ProjectsPanelState>(second).is_some());
        // Native observations are visible on the next draw without Rc copies.
        app.world_mut()
            .resource_mut::<ProjectsState>()
            .recent_workspaces
            .clear();
        frame(&mut context, &mut frontend, app.world_mut());
    }
}
