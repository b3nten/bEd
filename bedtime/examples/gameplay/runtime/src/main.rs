//! A normal Bevy game: this target has no bEd dependency.
use bedtime_example_game::{ExampleGamePlugin, GameCamera, GameProgress};
use bevy::prelude::*;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
#[derive(Resource)]
struct Verified(Arc<AtomicBool>);
fn main() {
    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: "Game runtime — no bEd".into(),
            ..default()
        }),
        ..default()
    }))
    .add_plugins(ExampleGamePlugin);
    let smoke_test = std::env::args().any(|arg| arg == "--smoke-test");
    let verified = Arc::<AtomicBool>::default();
    if smoke_test {
        app.insert_resource(Verified(Arc::clone(&verified)));
        app.add_systems(Update, smoke.after(bedtime_example_game::GameSystems));
    }
    let exit = app.run();
    if smoke_test {
        assert_eq!(exit, AppExit::Success);
        assert!(
            verified.load(Ordering::SeqCst),
            "game runtime smoke exited before completing its assertions"
        );
    }
}
fn smoke(world: &mut World) {
    let ticks = world.resource::<GameProgress>().ticks;
    if ticks == 5 {
        world
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyW);
    }
    if ticks == 20 {
        assert!(world.resource::<GameProgress>().position.y > 0.0);
        assert_eq!(
            world
                .query_filtered::<Entity, With<GameCamera>>()
                .iter(world)
                .count(),
            1
        );
        let target = world
            .query_filtered::<&bevy::camera::RenderTarget, With<GameCamera>>()
            .single(world)
            .unwrap();
        assert!(matches!(target, bevy::camera::RenderTarget::Window(_)));
        println!("Game runtime smoke passed: ordinary Bevy camera and input without bEd");
        world.resource::<Verified>().0.store(true, Ordering::SeqCst);
        world.write_message(AppExit::Success);
    }
}
