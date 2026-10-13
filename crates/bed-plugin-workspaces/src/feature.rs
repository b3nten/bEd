//! Workspace observations live in Bevy; each launcher owns only widget drafts.
use super::{
    MODULE_ID, NEW_COMMAND, PANEL_ID, WorkspacesState, welcome::Welcome, workspace_actions,
};
use crate::WorkspaceAction;
use bed_api::features::FrontendSnapshot;
use bed_api::workbench::{
    CommandInvocation, CommandResult, Panel, PanelCreation, PanelResult, WorkbenchAction, register,
};
use bevy::prelude::*;
use dear_imgui_rs::Ui;
use serde_json::Value;

pub struct WorkspacesPlugin;

impl Plugin for WorkspacesPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<crate::WorkspacesIntegrationPlugin>() {
            app.add_plugins(crate::WorkspacesIntegrationPlugin);
        }
        app.init_resource::<bed_api::features::FeatureRegistry>();
        app.world_mut()
            .resource_mut::<bed_api::features::FeatureRegistry>()
            .register_with(MODULE_ID, |registrar| {
                registrar.panel_options(
                    PANEL_ID,
                    false,
                    bed_api::workbench::PanelPlacement::Center,
                    Some("projects"),
                );
                registrar.command_ref(NEW_COMMAND);
            })
            .expect("feature metadata is installed once");
        app.init_resource::<WorkspacesState>().add_systems(
            PreUpdate,
            observe_workspace.in_set(bed_api::features::FeatureSystems::Prepare),
        );
        register(app.world_mut(), MODULE_ID, |registrar| {
            registrar.panel(PANEL_ID, "Workspaces", create_panel);
            registrar.command(NEW_COMMAND, "New Workspaces Panel", open_panel);
            registrar.command_icon(NEW_COMMAND, "bed.core.folder");
            registrar.panel_icon(PANEL_ID, "bed.core.folder");
        })
        .expect("WorkspacesPlugin must be installed once");
    }
}

fn observe_workspace(
    context: Option<Res<bed_files::FileContext>>,
    mut state: ResMut<WorkspacesState>,
) {
    state.disconnected = context
        .is_some_and(|context| context.availability == bed_files::FileAvailability::Disconnected);
}

/// Identifies live launchers independently of their main-thread UI objects.
#[derive(Component)]
pub struct WorkspacesPanelState;

pub struct WorkspacesFrontend {
    pub entity: Entity,
    welcome: Welcome,
}

fn open_panel(In(invocation): In<CommandInvocation>) -> CommandResult {
    Ok(vec![WorkbenchAction::OpenPanel {
        panel_type: PANEL_ID.into(),
        state: Value::Null,
        area: Some(invocation.area),
    }])
}

fn create_panel(In(creation): In<PanelCreation>, world: &mut World) -> PanelResult {
    crate::refresh_workspace_observations(world).map_err(|error| error.to_string())?;
    world.entity_mut(creation.entity).insert(bed_api::workbench::PanelDocumentation::new(&[
        ("Workspaces", "Open a folder or choose a recent project to begin working. Remote projects use their saved SSH connection."),
        ("Recent entries", "Right-click a recent project to open it in a new window, rename its label, or remove the entry."),
    ]));
    world
        .entity_mut(creation.entity)
        .insert(WorkspacesPanelState);
    #[cfg(feature = "mascots")]
    {
        // A panic must retire only this preview's registered system. The
        // child relationship releases it when its presentation closes.
        let renderer = world.register_system(render_scene);
        world
            .entity_mut(renderer.entity())
            .insert(ChildOf(creation.entity));
        world.entity_mut(creation.entity).insert((
            WorkspacesPreview(bed_plugin_mascots::MascotView::new(
                bed_plugin_mascots::MascotKind::WelcomeBed,
            )),
            bed_ui::gpu::CanvasRenderer(renderer),
        ));
    }
    Ok(Box::new(WorkspacesFrontend {
        entity: creation.entity,
        welcome: Welcome::frontend(),
    }))
}

impl Panel for WorkspacesFrontend {
    fn title(&self, _: &World) -> String {
        "Workspaces".into()
    }

    fn draw(
        &mut self,
        ui: &Ui,
        world: &mut World,
        _: &mut Vec<WorkbenchAction>,
        _shortcuts: &bed_api::workbench::ShortcutInput,
    ) {
        let state = world.resource::<WorkspacesState>();
        if state.connecting {
            ui.text_disabled("Connecting over SSH…");
        }
        let reconnect = state.disconnected && {
            ui.text_disabled("SSH connection lost. Your buffers are retained.");
            ui.button("Reconnect")
        };
        let error = state.error.clone();
        self.welcome
            .set_recent_workspaces(state.recent_workspaces.clone());
        if let Some(error) = error {
            ui.text_wrapped(&error);
            if ui.small_button("Dismiss workspace error") {
                world.resource_mut::<WorkspacesState>().error = None;
            }
        }
        let frame = FrontendSnapshot::capture(world);
        #[cfg(feature = "mascots")]
        let action = {
            let mut scene = world
                .get_mut::<WorkspacesPreview>(self.entity)
                .expect("a launcher owns its preview state");
            self.welcome
                .draw_body_with_scene(ui, &frame.context(), Some(&mut scene.0))
        };
        #[cfg(not(feature = "mascots"))]
        let action = self.welcome.draw_body_with_scene(ui, &frame.context());
        #[cfg(feature = "mascots")]
        if let Some(output) = world
            .get::<WorkspacesPreview>(self.entity)
            .and_then(|preview| preview.0.render_output())
        {
            world
                .entity_mut(self.entity)
                .insert(bed_ui::gpu::CanvasOutput(output));
        }
        if reconnect {
            crate::request_workspace(world, WorkspaceAction::Reconnect, Some(self.entity), 0);
        }
        for request in workspace_actions(action) {
            crate::request_workspace(world, request, Some(self.entity), 0);
        }
    }
}

#[cfg(feature = "mascots")]
#[derive(Component)]
pub struct WorkspacesPreview(pub bed_plugin_mascots::MascotView);

#[cfg(feature = "mascots")]
fn render_scene(
    In(invocation): In<bed_ui::gpu::CanvasRenderInvocation>,
    world: &mut World,
) -> bed_ui::gpu::CanvasRenderResult {
    use bed_api::gpu::GpuContext;
    // The renderer synchronizes entities in the same world. Temporarily take
    // its component, restoring it even if a backend panic unwinds this call.
    let mut scene = world
        .get_entity_mut(invocation.entity)
        .map_err(|_| "Workspaces preview is no longer open")?
        .take::<WorkspacesPreview>()
        .ok_or("Workspaces preview is no longer open")?;
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
    use crate::WorkspaceSpec;
    use bed_workbench::{WorkbenchLayout, WorkbenchPlugin, WorkbenchRequests, capture_command};
    use dear_imgui_rs::{Condition, Context, FramePrepareOptions, MouseButton};

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins((WorkbenchPlugin, WorkspacesPlugin));
        app
    }

    fn open(app: &mut App) -> Entity {
        let command = capture_command(app.world(), NEW_COMMAND, Value::Null);
        app.world_mut()
            .resource_mut::<WorkbenchRequests>()
            .push(WorkbenchAction::Invoke(command));
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
    fn launchers_share_world_observations_and_persist_as_panels() {
        let mut app = app();
        let first = open(&mut app);
        let second = open(&mut app);
        assert_ne!(first, second);
        assert!(app.world().get::<WorkspacesPanelState>(first).is_some());
        assert!(app.world().get::<WorkspacesPanelState>(second).is_some());
        app.world_mut()
            .resource_mut::<WorkspacesState>()
            .recent_workspaces
            .push(WorkspaceSpec::local("/example/project"));
        let saved = bed_workbench::save(app.world_mut());
        assert_eq!(saved["panels"].as_array().unwrap().len(), 2);
        assert!(
            saved["panels"]
                .as_array()
                .unwrap()
                .iter()
                .all(|panel| panel["panel_type"] == PANEL_ID)
        );
        #[cfg(feature = "mascots")]
        {
            let first_preview = app.world().get::<WorkspacesPreview>(first).unwrap();
            let second_preview = app.world().get::<WorkspacesPreview>(second).unwrap();
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
                .push(WorkbenchAction::ClosePanel(first));
            app.update();
            assert!(!app.world().entities().contains(first_renderer));
            assert!(app.world().entities().contains(second_renderer));
        }
    }

    #[test]
    fn frontend_queues_owned_project_intentions_from_its_originating_entity() {
        fn frame(context: &mut Context, frontend: &mut WorkspacesFrontend, world: &mut World) {
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
                        &bed_api::workbench::ShortcutInput::default(),
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
            .resource_mut::<WorkspacesState>()
            .recent_workspaces
            .push(workspace.clone());
        let mut frontend = WorkspacesFrontend {
            entity: first,
            welcome: Welcome::frontend(),
        };
        frame(&mut context, &mut frontend, app.world_mut());
        #[cfg(feature = "mascots")]
        assert_eq!(
            app.world()
                .get::<bed_ui::gpu::CanvasOutput>(first)
                .unwrap()
                .0
                .handle,
            app.world()
                .get::<WorkspacesPreview>(first)
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
        let mut cursor = bevy::ecs::message::MessageCursor::<crate::WorkspaceRequest>::default();
        let intents = cursor
            .read(app.world().resource::<Messages<crate::WorkspaceRequest>>())
            .collect::<Vec<_>>();
        assert!(matches!(intents.as_slice(), [intent]
            if intent.source == Some(first) && matches!(intent.action, WorkspaceAction::OpenFolder)));
        assert_eq!(
            app.world().resource::<WorkspacesState>().recent_workspaces,
            vec![workspace]
        );
        assert!(app.world().get::<WorkspacesPanelState>(second).is_some());
        // Native observations are visible on the next draw without Rc copies.
        app.world_mut()
            .resource_mut::<WorkspacesState>()
            .recent_workspaces
            .clear();
        frame(&mut context, &mut frontend, app.world_mut());
    }
}
