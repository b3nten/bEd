//! A normal Bevy game: this target has no bEd dependency.
use bed_example_game::{ExampleGamePlugin, GameCamera, GameProgress};
use bevy::prelude::*;
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
    if std::env::args().any(|arg| arg == "--smoke-test") {
        app.add_systems(Update, smoke.after(bed_example_game::GameSystems));
    }
    app.run();
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
        world.write_message(AppExit::Success);
    }
}
