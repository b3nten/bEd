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
    pub key_presses: u64,
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
    let viewport = UiViewport::new(image);
    world.entity_mut(view).insert((
        DemoState {
            overlay: false,
            key_presses: 0,
        },
        viewport,
    ));
    bedtime_workbench2::register_panel_input(world, view, game_input).unwrap();
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

fn game_input(
    mut input: bedtime_workbench2::PanelInput,
    mut demos: Query<(&mut DemoState, &UiViewport)>,
) {
    let (mut state, viewport) = demos.get_mut(input.panel).unwrap();
    if input.content_focused {
        input.capture_keyboard();
        if viewport.rect.contains(input.input.pointer)
            && input.input.buttons.get_pressed().next().is_some()
        {
            input.capture_pointer();
        }
        state.key_presses += input
            .input
            .events
            .iter()
            .filter(|e| e.key().is_some_and(|k| k.state.is_pressed() && !k.repeat))
            .count() as u64;
        input.consume_all();
    }
}

fn draw(mut frame: UiFrame, mut demos: Query<(&mut DemoState, &mut UiViewport)>) {
    let (mut state, mut viewport) = demos.get_mut(frame.view).unwrap();
    let padding = frame.panel_settings.padding;
    frame.padded(padding, |frame| {
        frame
            .ui
            .label("A spinning Bevy cube with its own post-processing.");
        frame.ui.weak(format!(
            "Click the scene to capture keyboard input (including Cmd/Ctrl+S). {} key presses",
            state.key_presses
        ));
        let response = frame.ui.button("Full-window scene");
        if response.clicked() {
            state.overlay = true;
        }
        frame.tooltip(
            &response,
            "The same game camera renders to the window, beneath UI.",
        );
        frame
            .ui
            .weak("Drag the divider to resize the render target.");
    });
    let size = frame.ui.available_size();
    let rounding = frame.panel_settings.rounding.round() as u8;
    viewport.corner_radius = bedtime_ui2::egui::CornerRadius {
        nw: 0,
        ne: 0,
        sw: rounding,
        se: rounding,
    };
    frame.viewport(
        "cube-scene",
        &mut viewport,
        [size.x.max(1.0), size.y.max(1.0)],
    );
}
pub(crate) fn draw_overlay(mut frame: UiFrame, world: &mut World) {
    use bedtime_ui2::{egui, theme::color};
    let (host, shell) = world
        .query::<(&WorkbenchHost, &bedtime_workbench2::WorkbenchPresentation)>()
        .single(world)
        .unwrap();
    if host.panels_visible {
        return;
    }
    let bounds = shell.bounds;
    let content = shell.panels;
    frame.scope(
        egui::UiBuilder::new()
            .id_salt("scene-controls")
            .max_rect(egui::Rect::from_min_max(
                egui::pos2(content.min.x + 14.0, content.min.y + 14.0),
                egui::pos2(content.max.x, content.max.y),
            )),
        |frame| {
            if frame.ui.button("Return to workbench").clicked() {
                for mut state in world.query::<&mut DemoState>().iter_mut(world) {
                    state.overlay = false;
                }
            }
        },
    );
    let painter = frame.ui.ctx().layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("scene-hud"),
    ));
    painter.text(
        egui::pos2(content.min.x + 24.0, content.max.y - 40.0),
        egui::Align2::LEFT_TOP,
        "Full-window HUD / scene + custom game shader + Bedtime UI",
        egui::FontId::proportional(frame.theme.metrics.font_size),
        color(frame.theme.palette.text),
    );
    painter.rect_filled(
        egui::Rect::from_min_max(
            egui::pos2(bounds.max.x - 140.0, content.max.y - 100.0),
            egui::pos2(bounds.max.x - 40.0, content.max.y - 40.0),
        ),
        0.0,
        color([0.8, 0.2, 0.3, 0.5]),
    );
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
        host.panels_visible = !overlay;
    }
    for mut camera in &mut ui_cameras {
        camera.clear_color = ClearColorConfig::Custom(Color::NONE);
        if let bevy::camera::CameraOutputMode::Write { clear_color, .. } = &mut camera.output_mode {
            *clear_color = if overlay {
                ClearColorConfig::None
            } else {
                ClearColorConfig::Custom(Color::NONE)
            };
        }
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
