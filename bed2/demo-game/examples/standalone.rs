use bed2_demo_game::CubeGamePlugin;
use bevy::{
    prelude::*,
    render::view::screenshot::{Screenshot, ScreenshotCaptured, save_to_disk},
};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

fn main() {
    let mut app = App::new();
    app.add_plugins((
        DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Cube game — standalone Bevy".into(),
                resolution: (960, 700).into(),
                ..default()
            }),
            ..default()
        }),
        CubeGamePlugin,
    ));
    let mut args = std::env::args();
    if args.any(|arg| arg == "--smoke-dir") {
        let directory = PathBuf::from(args.next().expect("--smoke-dir needs a directory"));
        std::fs::create_dir_all(&directory).unwrap();
        let ready = PipelinesReady::default();
        app.insert_resource(ready.clone());
        app.sub_app_mut(bevy::render::RenderApp)
            .insert_resource(ready)
            .add_systems(
                bevy::render::Render,
                (|cache: Res<bevy::render::render_resource::PipelineCache>,
                  ready: Res<PipelinesReady>| {
                    ready.0.store(
                        cache.waiting_pipelines().next().is_none(),
                        Ordering::Release,
                    );
                })
                .in_set(bevy::render::RenderSystems::Cleanup),
            );
        app.insert_resource(Smoke {
            directory,
            frame: 0,
            attempts: 0,
            hashes: Vec::new(),
        })
        .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            std::time::Duration::from_secs_f64(1.0 / 60.0),
        ))
        .add_systems(Update, smoke);
    }
    app.run();
}

#[derive(Resource, Clone, Default)]
struct PipelinesReady(Arc<AtomicBool>);

#[derive(Resource)]
struct Smoke {
    directory: PathBuf,
    frame: u32,
    attempts: u32,
    hashes: Vec<u64>,
}
fn smoke(
    mut state: ResMut<Smoke>,
    mut commands: Commands,
    mut exit: MessageWriter<AppExit>,
    ready: Res<PipelinesReady>,
) {
    state.attempts += 1;
    assert!(
        state.attempts < 4000,
        "Standalone pipelines/captures timed out"
    );
    if !ready.0.load(Ordering::Acquire) {
        return;
    }
    state.frame += 1;
    if state.frame >= 120 && state.frame.is_multiple_of(60) && state.hashes.len() < 2 {
        let name = format!("standalone-{}.png", state.frame);
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(state.directory.join(name)))
            .observe(
                |capture: On<ScreenshotCaptured>, mut state: ResMut<Smoke>| {
                    let image = capture.image.clone().try_into_dynamic().unwrap().to_rgba8();
                    let mut bright = 0;
                    let mut hash = 0_u64;
                    for y in (image.height() / 4..image.height() * 3 / 4).step_by(4) {
                        for x in (image.width() / 4..image.width() * 3 / 4).step_by(4) {
                            let rgba = image.get_pixel(x, y).0;
                            bright +=
                                usize::from(rgba[..3].iter().map(|&v| v as u32).sum::<u32>() > 220);
                            for value in rgba {
                                hash = hash.wrapping_mul(16777619) ^ value as u64;
                            }
                        }
                    }
                    assert!(
                        bright > 80,
                        "Standalone game must produce visible cube pixels"
                    );
                    state.hashes.push(hash);
                },
            );
    }
    if state.hashes.len() == 2 {
        assert_ne!(
            state.hashes[0], state.hashes[1],
            "Standalone cube must visibly rotate"
        );
        info!("Standalone game smoke passed without any Bedtime plugins");
        exit.write(AppExit::Success);
    }
    assert!(state.frame < 2000, "Standalone capture timed out");
}
