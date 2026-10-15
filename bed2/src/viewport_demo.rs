//! Editor adapter only: target routing, viewport layout and full-window UI.
use bed2_demo_game::GameCamera;
use bedtime_ui2::{
    UiFrame, UiSystems, UiViewport, gallery::GalleryState, register_view, viewport_image,
};
use bevy::{
    camera::{Projection, RenderTarget},
    prelude::*,
};

#[derive(Component)]
pub(crate) struct EditorUiCamera;

#[derive(Component)]
pub(crate) struct DemoState {
    pub open: bool,
    pub overlay: bool,
    pub bounds: Rect,
}

pub(crate) fn install(app: &mut App) {
    app.add_systems(
        PostUpdate,
        route_camera
            .after(UiSystems::PrepareTargets)
            .before(bevy::camera::CameraUpdateSystems),
    );
}

pub(crate) fn spawn(world: &mut World, surface: Entity) {
    let image = world
        .resource_mut::<Assets<Image>>()
        .add(viewport_image(UVec2::new(640, 400)));
    let view = register_view(world, surface, draw);
    world.entity_mut(view).insert((
        DemoState {
            open: true,
            overlay: false,
            bounds: Rect::default(),
        },
        UiViewport::new(image),
    ));
}

pub(crate) fn show(world: &mut World) {
    for mut state in world.query::<&mut DemoState>().iter_mut(world) {
        state.open = true;
        state.overlay = false;
    }
}

fn draw(frame: UiFrame, mut demos: Query<(&mut DemoState, &mut UiViewport)>) {
    use bedtime_ui2::imgui::Condition;
    let (mut state, mut viewport) = demos.get_mut(frame.view).unwrap();
    if !state.open {
        return;
    }
    let c = frame.controls();
    let ui = frame.ui;
    let overlay = state.overlay;
    let mut open = state.open;
    ui.window(if overlay {
        "Game controls"
    } else {
        "3D viewport"
    })
    .opened(&mut open)
    .position(
        if overlay { [24.0, 24.0] } else { [600.0, 80.0] },
        Condition::FirstUseEver,
    )
    .size(
        if overlay {
            [500.0, 190.0]
        } else {
            [520.0, 500.0]
        },
        Condition::FirstUseEver,
    )
    .bg_alpha(if overlay { 0.88 } else { 1.0 })
    .build(|| {
        let position = Vec2::from_array(ui.window_pos());
        state.bounds = Rect::from_corners(position, position + Vec2::from_array(ui.window_size()));
        ui.text_wrapped("A spinning Bevy cube with its own post-processing.");
        c.muted("Normal Bevy camera; UI displays its output.");
        if c.button("Panel") {
            state.overlay = false;
        }
        c.same_line();
        if c.button("Full-window scene") {
            state.overlay = true;
        }
        c.tooltip("The same game camera renders to the window, beneath UI.");
        if overlay {
            c.text("UI blends over the scene.");
            c.muted("The HUD can draw anywhere in the window.");
        } else {
            ui.text_wrapped("Resize this panel to resize the render target.");
            let available = ui.content_region_avail();
            c.viewport(
                "cube-scene",
                &mut viewport,
                [available[0].max(1.0), available[1].max(1.0)],
            );
        }
    });
    state.open = open;
    if !open {
        state.overlay = false;
    }
    if overlay {
        let size = ui.io().display_size();
        let draw = ui.get_foreground_draw_list();
        draw.add_text(
            [24.0, size[1] - 40.0],
            ui.get_color_u32_from_rgba(frame.theme.palette.text),
            "Full-window HUD / scene + custom game shader + Bedtime UI",
        );
        // A known half-transparent patch also exercises premultiplied sRGB
        // composition in the native smoke check.
        draw.add_rect(
            [size[0] - 140.0, size[1] - 100.0],
            [size[0] - 40.0, size[1] - 40.0],
            ui.get_color_u32_from_rgba([0.8, 0.2, 0.3, 0.5]),
        )
        .filled(true)
        .build();
    }
}

fn route_camera(
    demos: Query<(&DemoState, &UiViewport)>,
    mut games: Query<(&mut Camera, &mut RenderTarget, &mut Projection), With<GameCamera>>,
    mut ui_cameras: Query<&mut Camera, (With<EditorUiCamera>, Without<GameCamera>)>,
    mut galleries: Query<&mut GalleryState>,
    mut redraw: MessageWriter<bevy::window::RequestRedraw>,
) {
    let Ok((state, viewport)) = demos.single() else {
        return;
    };
    let overlay = state.open && state.overlay;
    for mut gallery in &mut galleries {
        if gallery.visible == overlay {
            gallery.visible = !overlay;
        }
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
            RenderTarget::Image(viewport.image.clone().into())
        };
        if target.normalize(None) != requested.normalize(None) {
            *target = requested;
            projection.set_changed();
        }
        camera.order = -10;
        camera.is_active = state.open && (overlay || viewport.visible);
        if camera.is_active {
            redraw.write(bevy::window::RequestRedraw);
        }
    }
}
