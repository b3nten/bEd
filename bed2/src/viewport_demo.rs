//! Explicit application extraction: workspace simulation, host render entities.
use bedtime_ui2::{UiFrame, UiViewport};
use bedtime_workbench2::{Workbench, WorkspaceViewport, register_panel};
use bevy::{camera::RenderTarget, prelude::*};
#[derive(Component)]
pub(crate) struct SceneCamera;
#[derive(Component)]
pub(crate) struct SceneCube;
pub(crate) fn spawn(world: &mut World, layout: Entity, image: Handle<Image>) -> Entity {
    let panel = register_panel(world, layout, "3D viewport", draw).unwrap();
    world.entity_mut(panel).insert(UiViewport::new(image));
    panel
}
fn draw(mut frame: UiFrame, mut views: Query<&mut UiViewport>) {
    let mut viewport = views.get_mut(frame.view).unwrap();
    let size = frame.ui.available_size();
    frame.viewport(
        "workspace-scene",
        &mut viewport,
        [size.x.max(1.0), size.y.max(1.0)],
    );
}
pub(crate) fn setup(world: &mut World) {
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
    world.spawn((
        Mesh3d(mesh),
        MeshMaterial3d(material),
        Transform::default(),
        SceneCube,
    ));
    world.spawn((
        PointLight {
            intensity: 500_000.0,
            ..default()
        },
        Transform::from_xyz(3.0, 5.0, 4.0),
    ));
    world.spawn((
        Camera3d::default(),
        Camera {
            is_active: false,
            clear_color: Color::srgb(0.06, 0.1, 0.16).into(),
            ..default()
        },
        Transform::from_xyz(3.5, 2.5, 5.0).looking_at(Vec3::ZERO, Vec3::Y),
        bevy::core_pipeline::tonemapping::Tonemapping::Reinhard,
        SceneCamera,
    ));
}
pub(crate) fn extract(world: &mut World) {
    let active = Workbench::new(world).workspace_id();
    let angle = Workbench::new(world)
        .world()
        .resource::<crate::workbench_demo::Session>()
        .angle;
    let viewport = world
        .query::<(&WorkspaceViewport, &UiViewport)>()
        .iter(world)
        .find(|(source, viewport)| source.workspace == active && viewport.visible)
        .map(|(_, v)| v.clone());
    for mut transform in world
        .query_filtered::<&mut Transform, With<SceneCube>>()
        .iter_mut(world)
    {
        transform.rotation = Quat::from_euler(EulerRot::XYZ, angle * 0.35, angle, 0.0);
    }
    for (mut camera, mut target) in world
        .query_filtered::<(&mut Camera, &mut RenderTarget), With<SceneCamera>>()
        .iter_mut(world)
    {
        camera.is_active = viewport.is_some();
        if let Some(viewport) = &viewport {
            *target = RenderTarget::Image(viewport.image.clone().into());
        }
    }
}
