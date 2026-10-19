//! Plain Bevy game code. This crate has no Bedtime dependency.
use bevy::prelude::*;

mod grade;

#[derive(Component)]
pub struct GameCamera;
#[derive(Component)]
pub struct SpinningCube;

#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GameSetup;

#[derive(Component)]
pub struct GameScene {
    pub camera: Entity,
    pub cube: Entity,
    pub running: bool,
}

#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GameSimulation;

pub struct CubeGamePlugin;
impl Plugin for CubeGamePlugin {
    fn build(&self, app: &mut App) {
        grade::install(app);
        app.add_systems(Update, spin.in_set(GameSimulation));
    }
}

/// Spawn an ordinary Bevy scene. The caller chooses its lifetime and camera target.
pub fn spawn_scene(world: &mut World) -> Entity {
    let mesh = world
        .resource_mut::<Assets<Mesh>>()
        .add(Cuboid::new(1.8, 1.8, 1.8));
    let material = world
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial {
            base_color: Color::srgb(0.35, 0.8, 0.7),
            perceptual_roughness: 0.45,
            ..default()
        });
    let root = world
        .spawn((Transform::default(), Visibility::default()))
        .id();
    let cube = world
        .spawn((
            Mesh3d(mesh),
            MeshMaterial3d(material),
            Transform::from_rotation(Quat::from_rotation_x(0.3)),
            SpinningCube,
            ChildOf(root),
        ))
        .id();
    world.spawn((
        PointLight {
            intensity: 500_000.0,
            ..default()
        },
        Transform::from_xyz(3.0, 5.0, 4.0),
        ChildOf(root),
    ));
    let camera = world
        .spawn((
            Camera3d::default(),
            Camera {
                clear_color: Color::srgb(0.06, 0.10, 0.16).into(),
                ..default()
            },
            bevy::camera::Hdr,
            bevy::core_pipeline::tonemapping::Tonemapping::Reinhard,
            Transform::from_xyz(3.5, 2.5, 5.0).looking_at(Vec3::ZERO, Vec3::Y),
            GameCamera,
            bevy::camera::ShadowLodOrigin,
            grade::DemoGrade,
            ChildOf(root),
        ))
        .id();
    world.entity_mut(root).insert(GameScene {
        camera,
        cube,
        running: true,
    });
    root
}

/// Standalone examples create one scene; an editor can create several explicitly.
pub fn setup(world: &mut World) {
    spawn_scene(world);
}

fn spin(
    time: Res<Time>,
    scenes: Query<&GameScene>,
    mut cubes: Query<&mut Transform, With<SpinningCube>>,
) {
    for scene in &scenes {
        if scene.running {
            let mut cube = cubes.get_mut(scene.cube).unwrap();
            cube.rotate_y(time.delta_secs() * 0.7);
            cube.rotate_x(time.delta_secs() * 0.25);
        }
    }
}
