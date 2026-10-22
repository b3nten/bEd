use bedtime_ui2::{UiContext, UiSystems, UiViewport, egui, viewport_image};
use bedtime_workbench2::{InActiveWorkspace, Panel, PanelInstance};
use bevy::{camera::RenderTarget, core_pipeline::tonemapping::Tonemapping, prelude::*};
use std::f32::consts::{FRAC_PI_2, TAU};

pub struct CubePlugin;

impl Plugin for CubePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_scene)
            .add_systems(
                Update,
                (prepare_viewports, update_camera_transforms).chain(),
            )
            .add_systems(PostUpdate, sync_cameras.in_set(UiSystems::SyncViewports));

        #[cfg(feature = "automation")]
        {
            use bedtime_automation2::AutomationAppExt;
            app.register_automation_command("app.inspect_cube", inspect);
        }
    }
}

/// The scene is application-owned. Each viewport has an independent camera.
fn spawn_scene(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.spawn((
        Name::new("Cube"),
        Mesh3d(meshes.add(Cuboid::new(1.5, 1.5, 1.5))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.35, 0.8, 0.7),
            perceptual_roughness: 0.5,
            ..default()
        })),
        Transform::from_xyz(0.0, 0.75, 0.0),
    ));
    commands.spawn((
        Name::new("Ground"),
        Mesh3d(meshes.add(Plane3d::default().mesh().size(12.0, 12.0))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.22, 0.25, 0.28),
            perceptual_roughness: 1.0,
            ..default()
        })),
    ));
    commands.spawn((
        Name::new("Sun"),
        DirectionalLight {
            illuminance: 10_000.0,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(4.0, 7.0, 5.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
}

#[derive(Component, Clone, Copy, Debug, PartialEq)]
struct OrbitCamera {
    target: Vec3,
    yaw: f32,
    pitch: f32,
    distance: f32,
}

impl Default for OrbitCamera {
    fn default() -> Self {
        Self {
            target: Vec3::new(0.0, 0.6, 0.0),
            yaw: 0.65,
            pitch: 0.4,
            distance: 7.0,
        }
    }
}

impl OrbitCamera {
    fn rotation(&self) -> Quat {
        Quat::from_euler(EulerRot::YXZ, self.yaw, -self.pitch, 0.0)
    }

    fn transform(&self) -> Transform {
        let rotation = self.rotation();
        Transform {
            translation: self.target + rotation * Vec3::Z * self.distance,
            rotation,
            ..default()
        }
    }
}

#[derive(Component, Default)]
#[require(PanelInstance = PanelInstance::of::<Self>(), OrbitCamera)]
pub struct CubeViewport;

#[derive(Component)]
struct PreparedViewport;

/// GPU preparation is explicit scheduled work, not a panel-validity condition.
/// A composed viewport can remain pending until the renderer's image assets exist.
fn prepare_viewports(world: &mut World) {
    if !world.contains_resource::<Assets<Image>>() {
        return;
    }
    let pending: Vec<_> = world
        .query_filtered::<Entity, (With<CubeViewport>, Without<PreparedViewport>)>()
        .iter(world)
        .collect();
    for panel in pending {
        let image = if let Some(viewport) = world.get::<UiViewport>(panel) {
            viewport.image.clone()
        } else {
            world
                .resource_mut::<Assets<Image>>()
                .add(viewport_image(UVec2::ONE))
        };
        let camera = world
            .get::<Camera>(panel)
            .cloned()
            .unwrap_or_else(|| Camera {
                order: -1,
                is_active: false,
                clear_color: Color::srgb(0.07, 0.10, 0.14).into(),
                ..default()
            });
        let camera_3d = world.get::<Camera3d>(panel).cloned().unwrap_or_default();
        let tonemapping = world
            .get::<Tonemapping>(panel)
            .copied()
            .unwrap_or(Tonemapping::Reinhard);
        let transform = world.get::<OrbitCamera>(panel).unwrap().transform();
        if world.get::<UiViewport>(panel).is_none() {
            world
                .entity_mut(panel)
                .insert(UiViewport::new(image.clone()));
        }
        // The panel owns its camera and image, so closure needs no separate owner.
        world.entity_mut(panel).insert((
            PreparedViewport,
            camera_3d,
            camera,
            tonemapping,
            RenderTarget::Image(image.into()),
            transform,
        ));
    }
}

impl Panel for CubeViewport {
    const KEY: &'static str = "bed.cube";
    const LABEL: &'static str = "3D Viewport";

    fn draw(ui: &mut egui::Ui, world: &mut World, _: &UiContext<'_>, panel: Entity) {
        if world.get::<PreparedViewport>(panel).is_none() {
            ui.spinner();
            ui.weak("Preparing viewport…");
            return;
        }
        let reset = ui
            .horizontal_wrapped(|ui| {
                let reset = ui.button("Reset view").clicked();
                ui.weak("Drag: orbit · Shift/middle-drag: pan · Scroll: zoom");
                reset
            })
            .inner;
        let response = UiViewport::show(ui, world, panel);
        let Projection::Perspective(projection) = world.get::<Projection>(panel).unwrap() else {
            panic!("Cube viewport requires a perspective camera");
        };
        let fov = projection.fov;
        let mut orbit = world.get_mut::<OrbitCamera>(panel).unwrap();
        if reset {
            *orbit = default();
        }

        // This viewport owns its egui drag, even after the pointer leaves its
        // rectangle. Other widgets, popups, and inactive windows keep input.
        if !ui.input(|i| i.focused)
            || !response.enabled()
            || ui.ctx().any_popup_open()
            || ui.memory(|m| m.top_modal_layer().is_some())
        {
            if response.is_pointer_button_down_on() || response.dragged() {
                ui.ctx().stop_dragging();
            }
            return;
        }
        let delta = response.drag_delta();
        let primary = response.dragged_by(egui::PointerButton::Primary);
        let pan = response.dragged_by(egui::PointerButton::Middle)
            || (primary && ui.input(|i| i.modifiers.shift));
        if pan {
            let world_per_point =
                2.0 * orbit.distance * (fov * 0.5).tan() / response.rect.height().max(1.0);
            let movement = orbit.rotation() * Vec3::new(-delta.x, delta.y, 0.0) * world_per_point;
            orbit.target += movement;
        } else if primary {
            orbit.yaw = (orbit.yaw - delta.x * 0.008).rem_euclid(TAU);
            orbit.pitch =
                (orbit.pitch + delta.y * 0.008).clamp(-FRAC_PI_2 + 0.01, FRAC_PI_2 - 0.01);
        }
        // egui's hit testing allows a small aim radius outside a widget. Wheel
        // zoom must stay inside the image rather than borrowing its toolbar.
        let inside_image = ui.input(|i| {
            i.pointer
                .hover_pos()
                .is_some_and(|p| response.rect.contains(p))
        });
        if response.hovered() && inside_image {
            let scroll = ui.input_mut(|i| std::mem::take(&mut i.smooth_scroll_delta.y));
            orbit.distance = (orbit.distance * (-scroll * 0.003).exp()).clamp(0.25, 1000.0);
        }
        if response.dragged() {
            ui.ctx().set_cursor_icon(if pan {
                egui::CursorIcon::Grabbing
            } else {
                egui::CursorIcon::Move
            });
        }
    }
}

fn update_camera_transforms(
    mut cameras: Query<(&OrbitCamera, &mut Transform), Changed<OrbitCamera>>,
) {
    for (orbit, mut transform) in &mut cameras {
        *transform = orbit.transform();
    }
}

fn sync_cameras(
    mut cameras: Query<(&UiViewport, Has<InActiveWorkspace>, &mut Camera), With<OrbitCamera>>,
) {
    for (viewport, active_workspace, mut camera) in &mut cameras {
        camera.is_active = viewport.visible && active_workspace;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composed_viewports_are_valid_before_gpu_preparation_and_keep_local_state() {
        let mut world = World::new();
        let orbit = OrbitCamera {
            distance: 12.0,
            ..default()
        };
        let panel = world.spawn((CubeViewport, orbit)).id();
        assert_eq!(
            world.get::<PanelInstance>(panel).unwrap().title,
            CubeViewport::LABEL
        );
        prepare_viewports(&mut world);
        assert!(world.get::<UiViewport>(panel).is_none());
        assert_eq!(*world.get::<OrbitCamera>(panel).unwrap(), orbit);
        world.init_resource::<Assets<Image>>();
        prepare_viewports(&mut world);
        assert!(world.get::<UiViewport>(panel).is_some());
        assert!(world.get::<Camera3d>(panel).is_some());
        assert_eq!(*world.get::<OrbitCamera>(panel).unwrap(), orbit);
        let image = world.get::<UiViewport>(panel).unwrap().image.id();
        prepare_viewports(&mut world);
        assert_eq!(world.get::<UiViewport>(panel).unwrap().image.id(), image);
    }
}

#[cfg(feature = "automation")]
fn inspect(world: &mut World, _: serde_json::Value) -> bedtime_automation2::AutomationResult {
    use serde_json::json;
    Ok(json!({
        "viewports": world.query::<(Entity, &OrbitCamera, &Transform, &Camera, &Projection, &UiViewport)>()
            .iter(world).map(|(entity, orbit, transform, camera, projection, viewport)| {
                let Projection::Perspective(projection) = projection else { unreachable!() };
                json!({
                    "entity": entity.to_bits().to_string(),
                    "target": orbit.target.to_array(),
                    "yaw": orbit.yaw,
                    "pitch": orbit.pitch,
                    "distance": orbit.distance,
                    "translation": transform.translation.to_array(),
                    "rotation": transform.rotation.to_array(),
                    "active": camera.is_active,
                    "aspect_ratio": projection.aspect_ratio,
                    "physical_size": viewport.physical_size.to_array(),
                    "target_size": camera.physical_target_size().map(|size| size.to_array()),
                    "image": format!("{:?}", viewport.image.id()),
                })
            }).collect::<Vec<_>>()
    }))
}
