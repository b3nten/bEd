//! Native GPU regression for the vendored Bevy screenshot readback patch.
//! cargo run --offline --locked -p bedtime-automation --example screenshot_without_drawable
use bevy::{
    prelude::*,
    render::{
        RenderApp, RenderPlugin,
        renderer::{RenderGraph, RenderGraphSystems},
        view::{
            ViewTarget,
            screenshot::{Screenshot, ScreenshotCaptured},
            window::ExtractedWindow,
        },
    },
    winit::WinitSettings,
};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[derive(Resource, Clone, Default)]
struct RemovedDrawables(Arc<AtomicUsize>);

#[derive(Resource, Default)]
struct Captures {
    completed: usize,
    pending: bool,
    frames: usize,
}

fn main() {
    let mut app = App::new();
    let removed = RemovedDrawables::default();
    app.add_plugins(
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: Some(Window {
                    title: "Screenshot readback regression".into(),
                    resolution: bevy::window::WindowResolution::new(360, 240)
                        .with_scale_factor_override(1.0),
                    ..default()
                }),
                ..default()
            })
            .set(RenderPlugin {
                synchronous_pipeline_compilation: true,
                ..default()
            }),
    )
    .insert_resource(WinitSettings::game())
    .insert_resource(removed.clone())
    .init_resource::<Captures>()
    .add_systems(Startup, |mut commands: Commands| {
        commands.spawn((Camera2d, bevy::camera::CompositingSpace::Linear));
    })
    .add_systems(Update, capture);
    app.sub_app_mut(RenderApp)
        .insert_resource(removed)
        .add_systems(
            RenderGraph,
            remove_drawable.in_set(RenderGraphSystems::Finish),
        );
    app.run();
}

fn capture(mut commands: Commands, mut state: ResMut<Captures>, mut cameras: Query<&mut Camera>) {
    state.frames += 1;
    assert!(state.frames < 600, "Screenshot regression timed out");
    if state.pending || state.frames < 20 {
        return;
    }
    let transparent = state.completed == 1;
    cameras.single_mut().unwrap().clear_color = ClearColorConfig::Custom(if transparent {
        Color::NONE
    } else {
        Color::srgb(0.25, 0.5, 0.75)
    });
    state.pending = true;
    commands.spawn(Screenshot::primary_window()).observe(move |
        capture: On<ScreenshotCaptured>,
        mut state: ResMut<Captures>,
        removed: Res<RemovedDrawables>,
        mut exit: MessageWriter<AppExit>,
    | {
        assert!(removed.0.load(Ordering::Relaxed) > state.completed,
            "Regression did not remove the capture's drawable");
        let image = capture.image.clone().try_into_dynamic().unwrap().to_rgba8();
        assert_eq!(image.dimensions(), (360, 240));
        let expected: [u8; 4] = if transparent { [0, 0, 0, 0] } else { [64, 128, 191, 255] };
        for pixel in image.pixels() {
            assert!(pixel.0.iter().zip(expected).all(|(a, b)| a.abs_diff(b) <= 1),
                "Screenshot readback mismatch: {:?}, expected {expected:?}", pixel.0);
        }
        state.completed += 1;
        state.pending = false;
        if state.completed == 2 {
            println!("Screenshot readback passed without a swapchain drawable: colored and transparent captures");
            exit.write(AppExit::Success);
        }
    });
}

// Keep the rendered capture target intact and deterministically reproduce the
// missing drawable observed on Metal. This runs before Bevy copies screenshots.
fn remove_drawable(
    targets: Query<&ViewTarget>,
    mut windows: Query<&mut ExtractedWindow>,
    removed: Res<RemovedDrawables>,
) {
    if targets.iter().any(|target| {
        target.out_texture().is_some_and(|view| {
            view.texture()
                .usage()
                .contains(bevy::render::render_resource::TextureUsages::COPY_SRC)
        })
    }) {
        for mut window in &mut windows {
            window.swap_chain_texture_view = None;
            removed.0.fetch_add(1, Ordering::Relaxed);
        }
    }
}
