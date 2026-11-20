//! The 3D backdrop: a slow flight down a valley of low hills at night, under
//! a moon, in the theme's colours. Its camera draws into an image the window
//! shows as its backdrop (`SurfaceBackdrop::Target`); fog the colour of the
//! theme's background hides where the land ends.
//!
//! The land is a grid of cells. It is built again each time the flight
//! crosses into the next row, a row further on, and slides towards the camera
//! in between.
use crate::backdrop::{Backdrops, FRAME_RATE};
use bedtime_ui::{MotionPolicy, Theme, layouts::LayoutSurface, theme::Rgba, viewport_image};
use bedtime_workbench::LayoutCamera;
use bevy::{
    asset::RenderAssetUsages,
    camera::{
        RenderTarget,
        visibility::{NoFrustumCulling, RenderLayers},
    },
    core_pipeline::tonemapping::Tonemapping,
    pbr::{DistanceFog, FogFalloff},
    prelude::*,
    render::render_resource::PrimitiveTopology,
};
use std::time::Duration;

/// The `Backdrop` setting's value for this scene: no file's name has a colon.
pub const NAME: &str = "scene:flyover";
pub const LABEL: &str = "Flyover (3D)";

/// bEd's scenes: each one's `Backdrop` setting and name in Settings.
pub const SCENES: &[(&str, &str)] = &[(NAME, LABEL), (crate::internals::NAME, crate::internals::LABEL)];

/// What the chosen scene's camera draws into: the window's backdrop.
#[derive(Resource)]
pub struct SceneImage(pub Handle<Image>);

/// A scene that stopped takes its image back, unless another scene's is
/// already there in its place.
pub fn retire(world: &mut World, image: &Handle<Image>) {
    if world
        .get_resource::<SceneImage>()
        .is_some_and(|shown| shown.0 == *image)
    {
        world.remove_resource::<SceneImage>();
    }
}

/// A render layer nothing uses yet: a scene's camera sees only its scene, and
/// no other camera sees it.
pub fn free_layer(world: &mut World) -> RenderLayers {
    let used: Vec<_> = world.query::<&RenderLayers>().iter(world).cloned().collect();
    let layer = (1..)
        .find(|&layer| {
            used.iter()
                .all(|layers| !layers.intersects(&RenderLayers::layer(layer)))
        })
        .unwrap();
    RenderLayers::layer(layer)
}

/// Ask for the window's next frame, for a scene that moves.
pub fn request_frame(world: &mut World) {
    let frame = Duration::from_secs_f32(1.0 / FRAME_RATE);
    let mut contexts = world.query_filtered::<&mut bevy_egui::EguiContext, With<LayoutCamera>>();
    for mut context in contexts.iter_mut(world) {
        context.get_mut().request_repaint_after(frame);
    }
}

/// The first layout surface's theme, and whether animations are on.
pub fn surface_style(world: &mut World) -> Option<(Theme, bool)> {
    world
        .query_filtered::<(&Theme, Option<&MotionPolicy>), With<LayoutSurface>>()
        .iter(world)
        .next()
        .map(|(theme, motion)| (theme.clone(), motion.is_none_or(|motion| motion.enabled)))
}

pub fn linear(color: Rgba) -> LinearRgba {
    let [r, g, b, _] = color;
    Color::srgb(r, g, b).to_linear()
}

/// Cells across and ahead, each a unit square.
const ACROSS: i64 = 72;
const AHEAD: i64 = 84;
/// Rows of land behind the camera, so its near edge is out of sight.
const BEHIND: i64 = 3;
/// Cells flown over each second.
const SPEED: f32 = 1.6;
const EYE: Vec3 = Vec3::new(0.0, 3.2, 0.0);

/// The theme's colours the scene is drawn in.
#[derive(Clone, Copy, PartialEq)]
struct Colors {
    background: Rgba,
    muted: Rgba,
    text: Rgba,
    accent: Rgba,
}

/// The scene while it is the chosen backdrop.
#[derive(Resource)]
pub struct Flyover {
    image: Handle<Image>,
    root: Entity,
    camera: Entity,
    land: Entity,
    mesh: Handle<Mesh>,
    moon: Handle<StandardMaterial>,
    stars: Handle<StandardMaterial>,
    /// How far the flight has come, in cells.
    flown: f32,
    /// The row and colours the land was last built for.
    built: Option<(i64, Colors)>,
}

pub fn hash(x: i64, z: i64) -> f32 {
    let mut n = (x.wrapping_mul(374_761_393) ^ z.wrapping_mul(668_265_263)) as u64;
    n = (n ^ (n >> 13)).wrapping_mul(1_274_126_177);
    ((n ^ (n >> 16)) & 0xffff) as f32 / 65535.0
}

fn noise(p: Vec2) -> f32 {
    let (x, z) = (p.x.floor() as i64, p.y.floor() as i64);
    let f = p - p.floor();
    let u = f * f * (Vec2::splat(3.0) - 2.0 * f);
    let near = hash(x, z) + (hash(x + 1, z) - hash(x, z)) * u.x;
    let far = hash(x, z + 1) + (hash(x + 1, z + 1) - hash(x, z + 1)) * u.x;
    near + (far - near) * u.y
}

/// How high the land is at a grid point: rolling hills, low along the middle
/// (the valley flown down) and rising to either side, a little rough.
fn height(x: i64, z: i64) -> f32 {
    let p = Vec2::new(x as f32, z as f32);
    let hills = noise(p * 0.07) * 0.65 + noise(p * 0.19 + 31.7) * 0.35;
    // The valley winds.
    let middle = (noise(Vec2::new(z as f32 * 0.035, 9.3)) - 0.5) * 14.0;
    let side = ((x as f32 - middle).abs() - 2.5).clamp(0.0, 16.0) / 16.0;
    let rise = side * side * (3.0 - 2.0 * side);
    hills * (0.5 + 9.0 * rise) + hash(x, z) * 0.25
}

/// The land from row `row` on, as flat-shaded triangles coloured by height.
fn land(row: i64, colors: Colors) -> Mesh {
    let low = linear(colors.background).mix(&linear(colors.muted), 0.3);
    let high = linear(colors.muted).mix(&linear(colors.accent), 0.55);
    let point = |i: i64, j: i64| {
        let x = i - ACROSS / 2;
        Vec3::new(x as f32, height(x, row + j), (BEHIND - j) as f32)
    };
    let triangles = (ACROSS * AHEAD * 2) as usize;
    let mut positions = Vec::with_capacity(triangles * 3);
    let mut normals = Vec::with_capacity(triangles * 3);
    let mut tints = Vec::with_capacity(triangles * 3);
    for j in 0..AHEAD {
        for i in 0..ACROSS {
            let (a, b, c, d) = (
                point(i, j),
                point(i + 1, j),
                point(i, j + 1),
                point(i + 1, j + 1),
            );
            for [p, q, r] in [[a, b, c], [b, d, c]] {
                let normal = (q - p).cross(r - p).normalize_or_zero();
                let tint = low
                    .mix(&high, ((p.y + q.y + r.y) / 3.0 / 7.0).clamp(0.0, 1.0))
                    .to_f32_array();
                for corner in [p, q, r] {
                    positions.push(corner.to_array());
                    normals.push(normal.to_array());
                    tints.push(tint);
                }
            }
        }
    }
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, tints)
}

fn spawn(world: &mut World) -> Flyover {
    let layers = free_layer(world);
    let image = world
        .resource_mut::<Assets<Image>>()
        .add(viewport_image(UVec2::ONE));
    let mut meshes = world.resource_mut::<Assets<Mesh>>();
    let mesh = meshes.add(Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    ));
    let sphere = meshes.add(Sphere::new(1.0).mesh().ico(3).unwrap());
    let mut materials = world.resource_mut::<Assets<StandardMaterial>>();
    let ground = materials.add(StandardMaterial {
        perceptual_roughness: 1.0,
        reflectance: 0.1,
        ..default()
    });
    // The moon and stars glow on their own, in front of the fog.
    let glowing = StandardMaterial {
        unlit: true,
        fog_enabled: false,
        ..default()
    };
    let moon = materials.add(glowing.clone());
    let stars = materials.add(glowing);

    let root = world
        .spawn((Transform::IDENTITY, Visibility::Inherited, layers.clone()))
        .id();
    let camera = world
        .spawn((
            ChildOf(root),
            layers.clone(),
            Camera3d::default(),
            Camera {
                order: -1,
                ..default()
            },
            RenderTarget::Image(image.clone().into()),
            Msaa::Sample4,
            // The theme's colours as they are: the fog must match the clear
            // colour, which isn't tone-mapped.
            Tonemapping::None,
            bevy::core_pipeline::tonemapping::DebandDither::Disabled,
            AmbientLight {
                brightness: 150.0,
                ..default()
            },
            DistanceFog {
                falloff: FogFalloff::Linear {
                    start: 6.0,
                    end: (AHEAD - BEHIND - 6) as f32,
                },
                ..default()
            },
            Projection::Perspective(PerspectiveProjection {
                fov: 50f32.to_radians(),
                ..default()
            }),
            Transform::from_translation(EYE),
        ))
        .id();
    world.spawn((
        ChildOf(root),
        layers.clone(),
        DirectionalLight {
            illuminance: 2600.0,
            shadow_maps_enabled: false,
            ..default()
        },
        Transform::from_xyz(-22.0, 20.0, -6.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    let land = world
        .spawn((
            ChildOf(root),
            layers.clone(),
            Mesh3d(mesh.clone()),
            MeshMaterial3d(ground),
            // It is always in view, and its bounds change as it is rebuilt.
            NoFrustumCulling,
            Transform::IDENTITY,
        ))
        .id();
    world.spawn((
        ChildOf(root),
        layers.clone(),
        Mesh3d(sphere.clone()),
        MeshMaterial3d(moon.clone()),
        Transform::from_xyz(-34.0, 27.0, -150.0).with_scale(Vec3::splat(7.0)),
    ));
    // Stars over the sky ahead, the same ones each time.
    for star in 0..160 {
        let (across, up, size) = (hash(star, 1), hash(star, 2), hash(star, 3));
        world.spawn((
            ChildOf(root),
            layers.clone(),
            Mesh3d(sphere.clone()),
            MeshMaterial3d(stars.clone()),
            Transform::from_xyz((across - 0.5) * 520.0, 8.0 + up * up * 190.0, -300.0)
                .with_scale(Vec3::splat(0.25 + size * 0.4)),
        ));
    }
    world.insert_resource(SceneImage(image.clone()));
    Flyover {
        image,
        root,
        camera,
        land,
        mesh,
        moon,
        stars,
        flown: 0.0,
        built: None,
    }
}

/// Start and stop the scene with the setting; while it shows, fly on, follow
/// the theme, and ask for the next frame.
pub fn update(world: &mut World) {
    if world.resource::<Backdrops>().chosen() != NAME {
        if let Some(flyover) = world.remove_resource::<Flyover>() {
            world.entity_mut(flyover.root).despawn();
            retire(world, &flyover.image);
        }
        return;
    }
    if !world.contains_resource::<Flyover>() {
        let flyover = spawn(world);
        world.insert_resource(flyover);
    }
    let Some((theme, moving)) = surface_style(world) else {
        return;
    };
    let palette = &theme.palette;
    let colors = Colors {
        background: palette.background,
        muted: palette.muted,
        text: palette.text,
        accent: palette.accent,
    };
    // A long pause (the window was idle) isn't a leap forward.
    let delta = world.resource::<Time<Real>>().delta_secs().min(0.1);
    world.resource_scope(|world, mut flyover: Mut<Flyover>| {
        if moving {
            flyover.flown += delta * SPEED;
        }
        let row = flyover.flown.floor() as i64;
        let restyle = flyover.built.is_none_or(|(_, built)| built != colors);
        if restyle || flyover.built.is_none_or(|(built, _)| built != row) {
            let mesh = land(row, colors);
            let _ = world
                .resource_mut::<Assets<Mesh>>()
                .insert(flyover.mesh.id(), mesh);
            flyover.built = Some((row, colors));
        }
        if restyle {
            let background = linear(colors.background);
            let mut materials = world.resource_mut::<Assets<StandardMaterial>>();
            if let Some(mut moon) = materials.get_mut(&flyover.moon) {
                moon.base_color = linear(colors.accent).mix(&linear(colors.text), 0.5).into();
            }
            if let Some(mut stars) = materials.get_mut(&flyover.stars) {
                stars.base_color = background.mix(&linear(colors.text), 0.6).into();
            }
            let mut camera = world.entity_mut(flyover.camera);
            camera.get_mut::<Camera>().unwrap().clear_color = Color::from(background).into();
            camera.get_mut::<DistanceFog>().unwrap().color = background.into();
        }
        // The land slides a cell's length towards the camera, then is built
        // a row further on.
        world.get_mut::<Transform>(flyover.land).unwrap().translation.z = flyover.flown.fract();
        // The camera sways a little, looking down the valley.
        let sway = flyover.flown * 0.05;
        *world.get_mut::<Transform>(flyover.camera).unwrap() =
            Transform::from_translation(EYE + Vec3::new(sway.sin() * 0.8, (sway * 1.7).sin() * 0.25, 0.0))
                .looking_at(Vec3::new((sway * 0.6).sin() * 3.0, 3.4, -30.0), Vec3::Y);
    });
    if moving {
        request_frame(world);
    }
}
