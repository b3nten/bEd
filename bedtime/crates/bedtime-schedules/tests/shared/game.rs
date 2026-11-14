//! A small top-down arena shooter for the tests to read the schedules of.
//!
//! It is deliberately "typical": states, nested system sets, messages, `Commands`, fixed-step
//! movement, run conditions, an exclusive system, one accidental ambiguity, and a HUD, so that the
//! schedule and data-flow views have something realistic to show. Nothing is drawn: its camera, sprites and
//! text are plain components here, standing in for the engine's.

#![allow(dead_code, clippy::type_complexity)]

use bevy::prelude::*;

pub const ARENA: f32 = 380.0;

// --- Stand-ins for the engine's drawing components ----------------------------------------------

#[derive(Component)]
pub struct Camera2d;

#[derive(Component)]
pub struct Sprite;

impl Sprite {
    fn from_color(_rgb: [f32; 3], _size: Vec2) -> Self {
        Self
    }
}

#[derive(Component)]
pub struct Text(pub String);

impl Text {
    fn new(text: &str) -> Self {
        Self(text.to_owned())
    }
}

pub struct GamePlugin {
    /// Spawn the 2D camera (disable when the host provides its own, e.g. headless rendering).
    pub spawn_camera: bool,
    /// Skip the menu and start playing immediately.
    pub start_playing: bool,
}

impl Default for GamePlugin {
    fn default() -> Self {
        Self {
            spawn_camera: true,
            start_playing: false,
        }
    }
}

#[derive(States, Default, Debug, Clone, PartialEq, Eq, Hash)]
pub enum GameState {
    #[default]
    Menu,
    Playing,
    GameOver,
}

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum GameSet {
    Input,
    Spawn,
    Combat,
    Cleanup,
    Ui,
}

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum CombatSet {
    Collide,
    Damage,
    Death,
}

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct Movement;

// --- Components ---------------------------------------------------------------------------------

#[derive(Component)]
pub struct Player;
#[derive(Component)]
pub struct Enemy;
#[derive(Component)]
pub struct Projectile {
    pub damage: f32,
}
#[derive(Component, Default)]
pub struct Velocity(pub Vec2);
#[derive(Component)]
pub struct Health(pub f32);
#[derive(Component)]
pub struct Lifetime(pub Timer);
#[derive(Component)]
pub struct Collider {
    pub radius: f32,
}
#[derive(Component, Clone, Copy, PartialEq, Eq)]
pub enum Team {
    Friendly,
    Hostile,
}
#[derive(Component)]
pub struct Weapon {
    pub cooldown: Timer,
}
#[derive(Component)]
pub struct Hud;

// --- Resources & messages -----------------------------------------------------------------------

#[derive(Resource, Default)]
pub struct Score(pub u32);
#[derive(Resource, Default)]
pub struct HighScore(pub u32);
#[derive(Resource)]
pub struct WaveState {
    pub number: u32,
    pub timer: Timer,
}
#[derive(Resource)]
pub struct GameSettings {
    pub player_speed: f32,
    pub enemy_speed: f32,
    pub projectile_speed: f32,
}
#[derive(Resource, Default)]
pub struct EnemyCount(pub usize);
#[derive(Resource)]
pub struct Rng(pub u64);

#[derive(Message)]
pub struct DamageMessage {
    pub target: Entity,
    pub amount: f32,
}
#[derive(Message)]
pub struct EnemyDied {
    pub position: Vec2,
}

impl Rng {
    pub fn next_f32(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 40) as f32) / ((1u64 << 24) as f32)
    }
}

#[derive(Resource)]
struct SpawnCamera(bool);

impl Plugin for GamePlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(SpawnCamera(self.spawn_camera));
        if self.start_playing {
            app.insert_state(GameState::Playing);
        } else {
            app.init_state::<GameState>();
        }
        app.init_resource::<Score>()
            .init_resource::<HighScore>()
            .init_resource::<EnemyCount>()
            .insert_resource(Rng(7))
            .insert_resource(WaveState {
                number: 0,
                timer: Timer::from_seconds(4.0, TimerMode::Repeating),
            })
            .insert_resource(GameSettings {
                player_speed: 180.0,
                enemy_speed: 60.0,
                projectile_speed: 420.0,
            })
            .add_message::<DamageMessage>()
            .add_message::<EnemyDied>()
            .configure_sets(
                Update,
                (
                    GameSet::Input,
                    GameSet::Spawn,
                    GameSet::Combat,
                    GameSet::Cleanup,
                )
                    .chain()
                    .run_if(in_state(GameState::Playing)),
            )
            .configure_sets(
                Update,
                (CombatSet::Collide, CombatSet::Damage, CombatSet::Death)
                    .chain()
                    .in_set(GameSet::Combat),
            )
            .configure_sets(FixedUpdate, Movement.run_if(in_state(GameState::Playing)))
            .configure_sets(PostUpdate, GameSet::Ui)
            .add_systems(Startup, setup_arena)
            .add_systems(OnEnter(GameState::Playing), reset_game)
            .add_systems(OnEnter(GameState::GameOver), show_game_over)
            .add_systems(
                Update,
                (
                    start_on_enter.run_if(in_state(GameState::Menu)),
                    restart_on_enter.run_if(in_state(GameState::GameOver)),
                    (read_player_input, fire_weapon).in_set(GameSet::Input),
                    (wave_timer, count_enemies, spawn_wave.run_if(wave_ready))
                        .chain()
                        .in_set(GameSet::Spawn),
                    collide_projectiles.in_set(CombatSet::Collide),
                    apply_damage.in_set(CombatSet::Damage),
                    (check_deaths, award_score).chain().in_set(CombatSet::Death),
                    (despawn_expired, despawn_out_of_bounds).in_set(GameSet::Cleanup),
                    // Reads `Score` without ordering against `award_score`: an ambiguity.
                    log_score,
                    world_stats,
                ),
            )
            .add_systems(
                FixedUpdate,
                (
                    enemy_chase,
                    integrate_velocity,
                    clamp_player_to_arena,
                    tick_lifetimes,
                )
                    .chain()
                    .in_set(Movement),
            )
            .add_systems(
                PostUpdate,
                update_hud
                    .in_set(GameSet::Ui)
                    .run_if(resource_changed::<Score>.or_else(resource_changed::<WaveState>)),
            );
    }
}

// --- Startup / state ----------------------------------------------------------------------------

fn setup_arena(mut commands: Commands, spawn_camera: Res<SpawnCamera>) {
    if spawn_camera.0 {
        commands.spawn(Camera2d);
    }
    commands.spawn((Hud, Text::new("arena — press Enter to start")));
    commands.spawn((
        Player,
        Team::Friendly,
        Velocity::default(),
        Health(100.0),
        Collider { radius: 12.0 },
        Weapon {
            cooldown: Timer::from_seconds(0.25, TimerMode::Once),
        },
        Sprite::from_color([0.3, 0.85, 0.4], Vec2::splat(24.0)),
        Transform::default(),
    ));
}

fn start_on_enter(keys: Res<ButtonInput<KeyCode>>, mut next: ResMut<NextState<GameState>>) {
    if keys.just_pressed(KeyCode::Enter) {
        next.set(GameState::Playing);
    }
}

fn restart_on_enter(keys: Res<ButtonInput<KeyCode>>, mut next: ResMut<NextState<GameState>>) {
    if keys.just_pressed(KeyCode::Enter) {
        next.set(GameState::Playing);
    }
}

fn reset_game(
    mut commands: Commands,
    mut score: ResMut<Score>,
    mut wave: ResMut<WaveState>,
    leftovers: Query<Entity, Or<(With<Enemy>, With<Projectile>)>>,
    mut players: Query<(&mut Transform, &mut Health), With<Player>>,
) {
    score.0 = 0;
    wave.number = 0;
    wave.timer.reset();
    for entity in &leftovers {
        commands.entity(entity).despawn();
    }
    for (mut transform, mut health) in &mut players {
        transform.translation = Vec3::ZERO;
        health.0 = 100.0;
    }
}

fn show_game_over(
    score: Res<Score>,
    mut high: ResMut<HighScore>,
    mut hud: Query<&mut Text, With<Hud>>,
) {
    high.0 = high.0.max(score.0);
    for mut text in &mut hud {
        text.0 = format!(
            "game over — score {} (best {}) — Enter to restart",
            score.0, high.0
        );
    }
}

// --- Input --------------------------------------------------------------------------------------

fn read_player_input(
    keys: Res<ButtonInput<KeyCode>>,
    settings: Res<GameSettings>,
    mut players: Query<&mut Velocity, With<Player>>,
) {
    let mut dir = Vec2::ZERO;
    if keys.pressed(KeyCode::ArrowLeft) || keys.pressed(KeyCode::KeyA) {
        dir.x -= 1.0;
    }
    if keys.pressed(KeyCode::ArrowRight) || keys.pressed(KeyCode::KeyD) {
        dir.x += 1.0;
    }
    if keys.pressed(KeyCode::ArrowUp) || keys.pressed(KeyCode::KeyW) {
        dir.y += 1.0;
    }
    if keys.pressed(KeyCode::ArrowDown) || keys.pressed(KeyCode::KeyS) {
        dir.y -= 1.0;
    }
    for mut velocity in &mut players {
        velocity.0 = dir.normalize_or_zero() * settings.player_speed;
    }
}

fn fire_weapon(
    mut commands: Commands,
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    settings: Res<GameSettings>,
    mut players: Query<(&Transform, &Velocity, &mut Weapon), With<Player>>,
) {
    for (transform, velocity, mut weapon) in &mut players {
        weapon.cooldown.tick(time.delta());
        if keys.pressed(KeyCode::Space) && weapon.cooldown.is_finished() {
            weapon.cooldown.reset();
            let dir = if velocity.0 == Vec2::ZERO {
                Vec2::Y
            } else {
                velocity.0.normalize()
            };
            commands.spawn((
                Projectile { damage: 10.0 },
                Team::Friendly,
                Velocity(dir * settings.projectile_speed),
                Lifetime(Timer::from_seconds(1.5, TimerMode::Once)),
                Collider { radius: 4.0 },
                Sprite::from_color([1.0, 0.9, 0.3], Vec2::splat(6.0)),
                Transform::from_translation(transform.translation),
            ));
        }
    }
}

// --- Spawning -----------------------------------------------------------------------------------

fn wave_timer(time: Res<Time>, mut wave: ResMut<WaveState>) {
    wave.timer.tick(time.delta());
    if wave.timer.just_finished() {
        wave.number += 1;
    }
}

fn wave_ready(wave: Res<WaveState>) -> bool {
    wave.timer.just_finished()
}

fn count_enemies(enemies: Query<(), With<Enemy>>, mut count: ResMut<EnemyCount>) {
    count.0 = enemies.iter().count();
}

fn spawn_wave(
    mut commands: Commands,
    wave: Res<WaveState>,
    count: Res<EnemyCount>,
    mut rng: ResMut<Rng>,
) {
    let wanted = (2 + wave.number as usize * 2).saturating_sub(count.0);
    for _ in 0..wanted {
        let angle = rng.next_f32() * std::f32::consts::TAU;
        let position = Vec2::from_angle(angle) * ARENA * 0.9;
        commands.spawn((
            Enemy,
            Team::Hostile,
            Velocity::default(),
            Health(20.0 + wave.number as f32 * 5.0),
            Collider { radius: 10.0 },
            Sprite::from_color([0.9, 0.3, 0.3], Vec2::splat(18.0)),
            Transform::from_translation(position.extend(0.0)),
        ));
    }
}

// --- Movement (fixed step) ----------------------------------------------------------------------

fn enemy_chase(
    settings: Res<GameSettings>,
    players: Query<&Transform, With<Player>>,
    mut enemies: Query<(&Transform, &mut Velocity), (With<Enemy>, Without<Player>)>,
) {
    let Ok(target) = players.single() else {
        return;
    };
    for (transform, mut velocity) in &mut enemies {
        let to_player = (target.translation - transform.translation).truncate();
        velocity.0 = to_player.normalize_or_zero() * settings.enemy_speed;
    }
}

fn integrate_velocity(time: Res<Time>, mut bodies: Query<(&mut Transform, &Velocity)>) {
    for (mut transform, velocity) in &mut bodies {
        transform.translation += (velocity.0 * time.delta_secs()).extend(0.0);
    }
}

fn clamp_player_to_arena(mut players: Query<&mut Transform, With<Player>>) {
    for mut transform in &mut players {
        transform.translation.x = transform.translation.x.clamp(-ARENA, ARENA);
        transform.translation.y = transform.translation.y.clamp(-ARENA, ARENA);
    }
}

fn tick_lifetimes(time: Res<Time>, mut lifetimes: Query<&mut Lifetime>) {
    for mut lifetime in &mut lifetimes {
        lifetime.0.tick(time.delta());
    }
}

// --- Combat -------------------------------------------------------------------------------------

fn collide_projectiles(
    mut commands: Commands,
    mut damage: MessageWriter<DamageMessage>,
    projectiles: Query<(Entity, &Transform, &Collider, &Projectile, &Team)>,
    targets: Query<(Entity, &Transform, &Collider, &Team), Without<Projectile>>,
) {
    for (projectile, p_transform, p_collider, projectile_data, p_team) in &projectiles {
        for (target, t_transform, t_collider, t_team) in &targets {
            if p_team == t_team {
                continue;
            }
            let distance = p_transform.translation.distance(t_transform.translation);
            if distance < p_collider.radius + t_collider.radius {
                damage.write(DamageMessage {
                    target,
                    amount: projectile_data.damage,
                });
                commands.entity(projectile).despawn();
                break;
            }
        }
    }
}

fn apply_damage(mut messages: MessageReader<DamageMessage>, mut healths: Query<&mut Health>) {
    for message in messages.read() {
        if let Ok(mut health) = healths.get_mut(message.target) {
            health.0 -= message.amount;
        }
    }
}

fn check_deaths(
    mut commands: Commands,
    mut died: MessageWriter<EnemyDied>,
    mut next: ResMut<NextState<GameState>>,
    entities: Query<(Entity, &Health, &Transform, Has<Enemy>, Has<Player>)>,
) {
    for (entity, health, transform, is_enemy, is_player) in &entities {
        if health.0 > 0.0 {
            continue;
        }
        if is_player {
            next.set(GameState::GameOver);
        } else if is_enemy {
            died.write(EnemyDied {
                position: transform.translation.truncate(),
            });
            commands.entity(entity).despawn();
        }
    }
}

fn award_score(mut died: MessageReader<EnemyDied>, mut score: ResMut<Score>) {
    for _ in died.read() {
        score.0 += 10;
    }
}

fn log_score(score: Res<Score>, mut last: Local<u32>) {
    if score.0 != *last {
        *last = score.0;
        info!("score {}", score.0);
    }
}

// --- Cleanup ------------------------------------------------------------------------------------

fn despawn_expired(mut commands: Commands, expired: Query<(Entity, &Lifetime)>) {
    for (entity, lifetime) in &expired {
        if lifetime.0.is_finished() {
            commands.entity(entity).despawn();
        }
    }
}

fn despawn_out_of_bounds(
    mut commands: Commands,
    bodies: Query<(Entity, &Transform), (With<Projectile>, Without<Player>)>,
) {
    for (entity, transform) in &bodies {
        if transform.translation.truncate().abs().max_element() > ARENA * 1.5 {
            commands.entity(entity).despawn();
        }
    }
}

/// An exclusive system: periodic stats straight from the world.
fn world_stats(world: &mut World, mut frames: Local<u32>) {
    *frames += 1;
    if frames.is_multiple_of(600) {
        info!("{} entities alive", world.entities().len());
    }
}

// --- UI -----------------------------------------------------------------------------------------

fn update_hud(
    score: Res<Score>,
    wave: Res<WaveState>,
    enemies: Res<EnemyCount>,
    mut hud: Query<&mut Text, With<Hud>>,
) {
    for mut text in &mut hud {
        text.0 = format!(
            "score {}  wave {}  enemies {}  — WASD move, Space shoot",
            score.0, wave.number, enemies.0
        );
    }
}
