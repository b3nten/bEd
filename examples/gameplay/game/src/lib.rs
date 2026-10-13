//! The game knows only Bevy. Hosts choose where its camera renders and whether
//! game execution is running; ordinary Bevy input drives the same simulation.
use bevy::{input::mouse::AccumulatedMouseMotion, prelude::*};

#[derive(Resource, Default, Debug)]
pub struct GameProgress {
    pub ticks: u64,
    pub position: Vec2,
    pub heading: f32,
}

#[derive(Resource, Debug)]
pub struct GameExecution {
    pub playing: bool,
}
impl Default for GameExecution {
    fn default() -> Self {
        Self { playing: true }
    }
}
#[derive(Component)]
pub struct GameCamera;
#[derive(Component)]
pub struct Player;

#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct GameSystems;

pub struct ExampleGamePlugin;
impl Plugin for ExampleGamePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GameProgress>()
            .init_resource::<GameExecution>()
            .add_systems(Startup, spawn_game)
            .add_systems(Update, update_game.in_set(GameSystems));
    }
}
fn spawn_game(mut commands: Commands) {
    commands.spawn((Camera2d, GameCamera));
    // All entities, including the camera, belong to the game. The editor may
    // detach its output without removing or recreating this scene.
    commands.spawn((
        Player,
        Sprite::from_color(Color::srgb(0.2, 0.75, 0.95), Vec2::new(54.0, 32.0)),
        Transform::from_xyz(0.0, 0.0, 1.0),
    ));
    for x in -4..=4 {
        for y in -3..=3 {
            if x == 0 && y == 0 {
                continue;
            }
            commands.spawn((
                Sprite::from_color(Color::srgb(0.17, 0.2, 0.27), Vec2::splat(12.0)),
                Transform::from_xyz(x as f32 * 90.0, y as f32 * 90.0, 0.0),
            ));
        }
    }
}
fn update_game(
    execution: Res<GameExecution>,
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<AccumulatedMouseMotion>,
    mut progress: ResMut<GameProgress>,
    mut players: Query<&mut Transform, With<Player>>,
) {
    if !execution.playing {
        return;
    }
    progress.ticks += 1;
    let direction = Vec2::new(
        f32::from(keys.pressed(KeyCode::KeyD)) - f32::from(keys.pressed(KeyCode::KeyA)),
        f32::from(keys.pressed(KeyCode::KeyW)) - f32::from(keys.pressed(KeyCode::KeyS)),
    );
    progress.position += direction.normalize_or_zero() * 180.0 * time.delta_secs();
    progress.heading -= mouse.delta.x * 0.01;
    for mut player in &mut players {
        player.translation = progress.position.extend(1.0);
        player.rotation = Quat::from_rotation_z(progress.heading);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    #[test]
    fn ordinary_bevy_input_and_game_owned_pause_control_execution() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::input::InputPlugin, ExampleGamePlugin));
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            Duration::from_millis(20),
        ));
        app.update();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyD);
        app.world_mut()
            .write_message(bevy::input::mouse::MouseMotion {
                delta: Vec2::new(8.0, 0.0),
            });
        app.update();
        let progress = app.world().resource::<GameProgress>();
        assert!(progress.position.x > 0.0);
        assert!(progress.heading < 0.0);
        let tick = progress.ticks;
        let position = progress.position;
        app.world_mut().resource_mut::<GameExecution>().playing = false;
        app.update();
        assert_eq!(app.world().resource::<GameProgress>().ticks, tick);
        assert_eq!(app.world().resource::<GameProgress>().position, position);
        assert_eq!(
            app.world_mut()
                .query_filtered::<Entity, With<GameCamera>>()
                .iter(app.world())
                .count(),
            1
        );
    }
}
