//! Plain Bevy game code. This crate has no Bedtime dependency.
use bevy::prelude::*;

mod grade;

#[derive(Component)]
pub struct GameCamera;
#[derive(Component)]
pub struct SpinningCube;

#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GameSetup;

pub struct CubeGamePlugin;
impl Plugin for CubeGamePlugin {
    fn build(&self, app: &mut App) {
        grade::install(app);
        app.add_systems(Startup, setup.in_set(GameSetup))
            .add_systems(Update, spin);
    }
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.spawn((
        Mesh3d(meshes.add(Cuboid::new(1.8, 1.8, 1.8))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.35, 0.8, 0.7),
            perceptual_roughness: 0.45,
            ..default()
        })),
        Transform::from_rotation(Quat::from_rotation_x(0.3)),
        SpinningCube,
    ));
    commands.spawn((
        PointLight {
            intensity: 500_000.0,
            ..default()
        },
        Transform::from_xyz(3.0, 5.0, 4.0),
    ));
    commands.spawn((
        Camera3d::default(),
        Camera {
            clear_color: Color::srgb(0.06, 0.10, 0.16).into(),
            ..default()
        },
        bevy::camera::Hdr,
        bevy::core_pipeline::tonemapping::Tonemapping::Reinhard,
        Transform::from_xyz(3.5, 2.5, 5.0).looking_at(Vec3::ZERO, Vec3::Y),
        GameCamera,
        grade::DemoGrade,
    ));
}

fn spin(time: Res<Time>, mut cubes: Query<&mut Transform, With<SpinningCube>>) {
    for mut cube in &mut cubes {
        cube.rotate_y(time.delta_secs() * 0.7);
        cube.rotate_x(time.delta_secs() * 0.25);
    }
}
