//! Editor adapter only: target routing, viewport layout and full-window UI.
use bed2_demo_game::GameCamera;
use bedtime_ui2::{UiFrame, UiSystems, UiViewport, viewport_image};
use bedtime_workbench2::{
    InArea, InLayout, InWorkspace, PanelPresentation, Workbench, WorkbenchHost, register_panel,
};
use bevy::{
    camera::{Projection, RenderTarget},
    prelude::*,
};

#[derive(Component)]
pub(crate) struct EditorUiCamera;

#[derive(Component)]
pub(crate) struct DemoState {
    pub overlay: bool,
}

pub(crate) fn install(app: &mut App) {
    app.add_systems(
        PostUpdate,
        route_camera
            .after(UiSystems::PrepareTargets)
            .after(bedtime_workbench2::WorkbenchSystems::Apply)
            .before(bevy::camera::CameraUpdateSystems),
    );
}

pub(crate) fn spawn(world: &mut World, layout: Entity) -> Entity {
    let image = world
        .resource_mut::<Assets<Image>>()
        .add(viewport_image(UVec2::new(640, 400)));
    let view = register_panel(world, layout, "3D viewport", draw).unwrap();
    world
        .entity_mut(view)
        .insert((DemoState { overlay: false }, UiViewport::new(image)));
    view
}

pub(crate) fn show(world: &mut World) {
    let existing = world
        .query_filtered::<Entity, With<DemoState>>()
        .iter(world)
        .next();
    let panel = if let Some(panel) = existing {
        let layout = Workbench::new(world).panel_layout(panel).unwrap();
        let workspace = world.get::<InWorkspace>(layout).unwrap().0;
        let mut workbench = Workbench::new(world);
        workbench.activate_workspace(workspace).unwrap();
        workbench.activate_layout(layout).unwrap();
        panel
    } else {
        let layout = Workbench::new(world).active_layout();
        spawn(world, layout)
    };
    let mut state = world.get_mut::<DemoState>(panel).unwrap();
    state.overlay = false;
    Workbench::new(world).focus_panel(panel).unwrap();
}

fn draw(frame: UiFrame, mut demos: Query<(&mut DemoState, &mut UiViewport)>) {
    let (mut state, mut viewport) = demos.get_mut(frame.view).unwrap();
    let c = frame.controls();
    let ui = frame.ui;
    ui.text_wrapped("A spinning Bevy cube with its own post-processing.");
    c.muted("Normal Bevy camera; UI displays its output.");
    if c.button("Full-window scene") {
        state.overlay = true;
    }
    c.tooltip("The same game camera renders to the window, beneath UI.");
    c.muted("Drag the divider to resize the render target.");
    let available = ui.content_region_avail();
    c.viewport(
        "cube-scene",
        &mut viewport,
        [available[0].max(1.0), available[1].max(1.0)],
    );
}

pub(crate) fn draw_overlay(frame: &UiFrame) {
    let ui = frame.ui;
    let size = ui.io().display_size();
    let draw = ui.get_foreground_draw_list();
    draw.add_text(
        [24.0, size[1] - 40.0],
        ui.get_color_u32_from_rgba(frame.theme.palette.text),
        "Full-window HUD / scene + custom game shader + Bedtime UI",
    );
    draw.add_rect(
        [size[0] - 140.0, size[1] - 100.0],
        [size[0] - 40.0, size[1] - 40.0],
        ui.get_color_u32_from_rgba([0.8, 0.2, 0.3, 0.5]),
    )
    .filled(true)
    .build();
}

fn route_camera(
    demos: Query<(&DemoState, &UiViewport, &PanelPresentation, &InArea)>,
    active_workspace: Res<bedtime_workbench2::ActiveWorkspace>,
    workspaces: Query<&bedtime_workbench2::Workspace>,
    areas: Query<&InLayout>,
    mut games: Query<(&mut Camera, &mut RenderTarget, &mut Projection), With<GameCamera>>,
    mut ui_cameras: Query<&mut Camera, (With<EditorUiCamera>, Without<GameCamera>)>,
    mut hosts: Query<&mut WorkbenchHost>,
    mut redraw: MessageWriter<bevy::window::RequestRedraw>,
) {
    let demo = demos.iter().next();
    let active_layout = workspaces
        .get(active_workspace.entity())
        .unwrap()
        .active_layout();
    let overlay = demo.is_some_and(|(state, _, _, layout)| {
        state.overlay && areas.get(layout.0).unwrap().0 == active_layout
    });
    for mut host in &mut hosts {
        host.visible = !overlay;
    }
    for mut camera in &mut ui_cameras {
        camera.clear_color = if overlay {
            ClearColorConfig::None
        } else {
            ClearColorConfig::Default
        };
    }
    for (mut camera, mut target, mut projection) in &mut games {
        let requested = if overlay {
            RenderTarget::default()
        } else {
            demo.map_or_else(RenderTarget::default, |(_, viewport, _, _)| {
                RenderTarget::Image(viewport.image.clone().into())
            })
        };
        if target.normalize(None) != requested.normalize(None) {
            *target = requested;
            projection.set_changed();
        }
        camera.order = -10;
        camera.is_active = demo.is_some_and(|(_, viewport, presentation, _)| {
            overlay || (presentation.visible && viewport.visible)
        });
        if camera.is_active {
            redraw.write(bevy::window::RequestRedraw);
        }
    }
}
