use crate::{
    camera::Camera as OrbitCamera,
    debug_views::{DebugViews, DisplayMode},
    model::{PreparedModel, Scene},
};
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::{
    anti_alias::taa::TemporalAntiAliasing,
    asset::RenderAssetUsages,
    camera::{Exposure, Hdr, RenderTarget},
    image::ImageSampler,
    light::{CascadeShadowConfigBuilder, GeneratedEnvironmentMapLight, Skybox},
    pbr::{ScreenSpaceAmbientOcclusion, ScreenSpaceAmbientOcclusionQualityLevel},
    prelude::*,
    render::render_resource::{
        Extent3d, TextureDimension, TextureFormat, TextureViewDescriptor, TextureViewDimension,
    },
};
use std::sync::Arc;
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Lighting {
    #[default]
    Studio,
    Outdoor,
    Neutral,
    StudioSmall08,
    KiaraDawn,
}

impl Lighting {
    pub const NAMES: [&'static str; 5] = [
        "Studio",
        "Outdoor",
        "Neutral",
        "Studio Small 08 (HDRI)",
        "Kiara Dawn (HDRI)",
    ];

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn from_index(index: usize) -> Self {
        match index {
            1 => Self::Outdoor,
            2 => Self::Neutral,
            3 => Self::StudioSmall08,
            4 => Self::KiaraDawn,
            _ => Self::Studio,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Settings {
    pub lighting: Lighting,
    pub display: DisplayMode,
    pub normals: bool,
    pub normal_length: f32,
    pub skybox: bool,
    pub skybox_blur: f32,
    pub horizon: f32,
    pub shadows: bool,
    pub ao: bool,
    pub exposure: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            lighting: Lighting::Studio,
            display: DisplayMode::Shaded,
            normals: false,
            normal_length: 0.08,
            skybox: false,
            skybox_blur: 0.0,
            horizon: -12.0,
            shadows: true,
            ao: true,
            exposure: 0.0,
        }
    }
}

pub(super) struct SceneGpu {
    pub ao_supported: bool,
    layers: bevy::camera::visibility::RenderLayers,
    pub camera: Entity,
    pub root: Entity,
    key: Entity,
    fill: Entity,
    environments: [Handle<Image>; 5],
    skybox_image: Handle<Image>,
    debug: Option<DebugViews>,
    loading: crate::bevy_scene::SceneLoading,
    snapshot: Option<Scene>,
    source_counts: (usize, usize, usize, usize),
    center: Vec3,
    radius: f32,
    settings: Option<Settings>,
}

impl SceneGpu {
    pub fn ready(&self) -> bool {
        !self.loading.pending() && self.snapshot.is_some()
    }
    pub fn new(
        world: &mut World,
        owner: Entity,
        prepared: &PreparedModel,
        image: Handle<Image>,
    ) -> Self {
        let used: Vec<_> = world
            .query::<&bevy::camera::visibility::RenderLayers>()
            .iter(world)
            .cloned()
            .collect();
        let layer = (1..)
            .find(|&layer| {
                used.iter().all(|layers| {
                    !layers.intersects(&bevy::camera::visibility::RenderLayers::layer(layer))
                })
            })
            .unwrap();
        let layers = bevy::camera::visibility::RenderLayers::layer(layer);
        let root = world
            .spawn((
                ChildOf(owner),
                Transform::IDENTITY,
                Visibility::Inherited,
                layers.clone(),
            ))
            .id();
        let view = root.to_bits();
        let mut loading = crate::bevy_scene::SceneLoading::new(world, view, Some(layers.clone()));
        let (center, radius, debug, snapshot, source_counts) = match prepared {
            PreparedModel::Gltf(source) => {
                loading.start(world, Arc::clone(&source.bytes), Arc::clone(&source.meshes));
                (
                    Vec3::ZERO,
                    1.0,
                    None,
                    None,
                    (
                        source.nodes,
                        source.animations,
                        source.draco_primitives,
                        source.meshopt_views,
                    ),
                )
            }
            PreparedModel::Stl(source) => {
                let scene = &source.inspection;
                let radius = scene.radius();
                let center = Vec3::from_array(scene.center().to_array());
                let normalization = Transform::from_scale(Vec3::splat(radius.recip()))
                    .with_translation(-center / radius);
                let mesh = world
                    .resource_mut::<Assets<Mesh>>()
                    .add(source.mesh.clone());
                let material =
                    world
                        .resource_mut::<Assets<StandardMaterial>>()
                        .add(StandardMaterial {
                            base_color: Color::linear_rgb(0.65, 0.65, 0.65),
                            perceptual_roughness: 0.65,
                            ..default()
                        });
                let surface = world
                    .spawn((
                        ChildOf(root),
                        layers.clone(),
                        Mesh3d(mesh),
                        MeshMaterial3d(material),
                        normalization,
                    ))
                    .id();
                let debug = DebugViews::new(
                    world,
                    scene,
                    &[surface],
                    normalization,
                    Some((ChildOf(root), layers.clone())),
                );
                (
                    center,
                    radius,
                    Some(debug),
                    Some(scene.clone()),
                    (1, 0, 0, 0),
                )
            }
        };
        let environments = world.resource::<crate::Environments>().0.clone();
        let camera = world
            .spawn((
                Camera3d::default(),
                bevy::pbr::ScreenSpaceTransmission::default(),
                Camera {
                    order: -1,
                    is_active: false,
                    clear_color: Color::NONE.into(),
                    ..default()
                },
                AmbientLight {
                    brightness: 0.0,
                    ..default()
                },
                ChildOf(root),
                layers.clone(),
                RenderTarget::Image(image.into()),
                Hdr,
                Msaa::Off,
                Tonemapping::AcesFitted,
                Exposure { ev100: 12.0 },
                TemporalAntiAliasing::default(),
                Transform::IDENTITY,
            ))
            .id();
        let key = world
            .spawn((
                ChildOf(root),
                layers.clone(),
                DirectionalLight::default(),
                Transform::from_xyz(0.5, 0.8, 0.6).looking_at(Vec3::ZERO, Vec3::Y),
            ))
            .id();
        let fill = world
            .spawn((
                ChildOf(root),
                layers.clone(),
                DirectionalLight::default(),
                Transform::from_xyz(-0.6, 0.3, -0.5).looking_at(Vec3::ZERO, Vec3::Y),
            ))
            .id();
        let skybox_image = crate::skybox_blur::create(world, camera);
        let ao_supported = world
            .get_resource::<bevy::render::renderer::RenderDevice>()
            .is_none_or(|device| device.limits().max_storage_textures_per_shader_stage >= 5);
        Self {
            ao_supported,
            root,
            layers,
            camera,
            key,
            fill,
            environments,
            skybox_image,
            debug,
            loading,
            snapshot,
            source_counts,
            center,
            radius,
            settings: None,
        }
    }
    pub fn sync(
        &mut self,
        world: &mut World,
        camera: OrbitCamera,
        settings: Settings,
    ) -> Result<(), String> {
        if self.loading.pending() {
            if let Some(mut loaded) = self.loading.poll(world)? {
                loaded.snapshot.nodes = self.source_counts.0;
                loaded.snapshot.animations = self.source_counts.1;
                loaded.snapshot.draco_primitives = self.source_counts.2;
                loaded.snapshot.meshopt_views = self.source_counts.3;
                self.center = Vec3::from_array(loaded.snapshot.center().to_array());
                self.radius = loaded.snapshot.radius();
                self.debug = Some(DebugViews::new(
                    world,
                    &loaded.snapshot,
                    &loaded.surfaces,
                    loaded.normalization,
                    Some((ChildOf(self.root), self.layers.clone())),
                ));
                self.snapshot = Some(loaded.snapshot);
                self.settings = None;
            }
            if self.loading.pending() {
                return Ok(());
            }
        }
        let eye = (Vec3::from_array(camera.eye().to_array()) - self.center) / self.radius;
        let focus = (Vec3::from_array(camera.target.to_array()) - self.center) / self.radius;
        {
            let mut entity = world.entity_mut(self.camera);
            *entity.get_mut::<Transform>().unwrap() =
                Transform::from_translation(eye).looking_at(focus, Vec3::Y);
            // Let the host panel show through instead of tonemapping its theme
            // color along with the model when no skybox is visible.
            entity.get_mut::<Camera>().unwrap().is_active = true;
            entity.get_mut::<Camera>().unwrap().clear_color = if settings.skybox {
                Color::BLACK
            } else {
                Color::NONE
            }
            .into();
            entity.insert(Projection::Perspective(PerspectiveProjection {
                fov: std::f32::consts::FRAC_PI_4,
                near: 0.001,
                far: (camera.distance / self.radius + 20.0).max(1.0),
                ..default()
            }));
            entity.insert(Exposure {
                ev100: 12.0 - settings.exposure,
            });
        }
        if self.settings != Some(settings) {
            self.configure(world, settings);
            self.settings = Some(settings);
        }
        if let Some(debug) = &self.debug {
            debug.update_view(world, (eye - focus).normalize_or_zero());
        }
        // Skyboxes are infinitely distant: moving the model cannot lower their
        // horizon. Pitch the background around the camera's right axis instead,
        // keeping the model and environment lighting fixed as the view orbits.
        {
            let mut entity = world.entity_mut(self.camera);
            let right = entity.get::<Transform>().unwrap().right().as_vec3();
            if let Some(mut skybox) = entity.get_mut::<Skybox>() {
                skybox.rotation = Quat::from_axis_angle(right, settings.horizon.to_radians());
            }
        }
        world.entity_mut(self.key).insert(
            CascadeShadowConfigBuilder {
                num_cascades: 1,
                minimum_distance: 0.001,
                maximum_distance: (camera.distance / self.radius + 3.0).max(4.0),
                ..default()
            }
            .build(),
        );
        Ok(())
    }

    pub fn snapshot(&self) -> Option<&Scene> {
        self.snapshot.as_ref()
    }
    pub fn clear(&mut self, world: &mut World) {
        self.loading.clear(world);
        let _ = world.try_despawn(self.root);
    }
    fn configure(&mut self, world: &mut World, settings: Settings) {
        if let Some(debug) = &mut self.debug {
            debug.apply(
                world,
                settings.display,
                settings.normals,
                settings.normal_length,
            );
        }
        let (key, fill, tint, intensity) = match settings.lighting {
            Lighting::Studio => (18000.0, 4500.0, Color::srgb(1.0, 0.94, 0.86), 1800.0),
            Lighting::Outdoor => (32000.0, 1000.0, Color::srgb(1.0, 0.97, 0.90), 2400.0),
            Lighting::Neutral => (10000.0, 10000.0, Color::WHITE, 1200.0),
            // Let the photographed environment supply the lighting rather than
            // baking the procedural presets' key/fill into HDRI appearances.
            Lighting::StudioSmall08 => (0.0, 0.0, Color::WHITE, 1800.0),
            Lighting::KiaraDawn => (0.0, 0.0, Color::WHITE, 1800.0),
        };
        world.entity_mut(self.key).insert(DirectionalLight {
            color: tint,
            illuminance: key,
            shadow_maps_enabled: settings.shadows && key > 0.0,
            ..default()
        });
        world.entity_mut(self.fill).insert(DirectionalLight {
            color: Color::srgb(0.83, 0.9, 1.0),
            illuminance: fill,
            ..default()
        });
        let environment = self.environments[settings.lighting.index()].clone();
        crate::skybox_blur::configure(world, self.camera, settings.skybox_blur);
        let mut entity = world.entity_mut(self.camera);
        // Bevy allocates filtered maps only without EnvironmentMapLight. Drop
        // the previous outputs when the source changes so their dimensions and
        // mip chain match both the 64px presets and the 256px HDRI cubemaps.
        if self
            .settings
            .is_none_or(|old| old.lighting != settings.lighting)
        {
            entity.remove::<EnvironmentMapLight>();
            entity.insert(GeneratedEnvironmentMapLight {
                environment_map: environment.clone(),
                intensity,
                ..default()
            });
        }
        if settings.skybox {
            entity.insert(Skybox {
                image: Some(if settings.skybox_blur > 0.0 {
                    self.skybox_image.clone()
                } else {
                    environment
                }),
                brightness: intensity,
                ..default()
            });
        } else {
            entity.remove::<Skybox>();
        }
        if self.settings.is_some_and(|old| {
            old.skybox != settings.skybox || old.skybox_blur != settings.skybox_blur
        }) {
            // Discard stale background samples when toggling the skybox or
            // changing its blur so the background responds immediately.
            entity.get_mut::<TemporalAntiAliasing>().unwrap().reset = true;
        }
        if settings.ao && self.ao_supported {
            entity.insert(ScreenSpaceAmbientOcclusion {
                quality_level: ScreenSpaceAmbientOcclusionQualityLevel::Medium,
                constant_object_thickness: 0.1,
                ..default()
            });
        } else {
            entity.remove::<ScreenSpaceAmbientOcclusion>();
        }
    }
}

// Small built-in cubemaps keep presets self-contained, including SSH/offline use.
// Bevy filters these into diffuse irradiance and roughness-dependent reflections.
pub(crate) fn environment(lighting: Lighting) -> Image {
    const SIDE: u32 = 64;
    let mut rgba = Vec::with_capacity((SIDE * SIDE * 6 * 4) as usize);
    for face in 0..6 {
        for y in 0..SIDE {
            for x in 0..SIDE {
                let u = (x as f32 + 0.5) / SIDE as f32 * 2.0 - 1.0;
                let v = (y as f32 + 0.5) / SIDE as f32 * 2.0 - 1.0;
                let direction = match face {
                    0 => Vec3::new(1.0, -v, -u),
                    1 => Vec3::new(-1.0, -v, u),
                    2 => Vec3::new(u, 1.0, v),
                    3 => Vec3::new(u, -1.0, -v),
                    4 => Vec3::new(u, -v, 1.0),
                    _ => Vec3::new(-u, -v, -1.0),
                }
                .normalize();
                let color = match lighting {
                    Lighting::Studio => {
                        let softbox = direction
                            .dot(Vec3::new(0.4, 0.8, 0.5).normalize())
                            .max(0.0)
                            .powi(24);
                        Vec3::new(0.25, 0.28, 0.34)
                            + Vec3::splat(softbox * 0.75)
                            + Vec3::splat(direction.y.max(0.0) * 0.15)
                    }
                    Lighting::Outdoor => {
                        let height = direction.y.max(0.0).sqrt();
                        if direction.y < 0.0 {
                            Vec3::new(0.25, 0.23, 0.18)
                        } else {
                            Vec3::new(0.75, 0.82, 0.9).lerp(Vec3::new(0.20, 0.42, 0.8), height)
                        }
                    }
                    Lighting::Neutral => Vec3::splat(0.5 + direction.y * 0.15),
                    Lighting::StudioSmall08 | Lighting::KiaraDawn => unreachable!(),
                };
                rgba.extend(
                    color
                        .clamp(Vec3::ZERO, Vec3::ONE)
                        .to_array()
                        .map(|v| (v * 255.0).round() as u8),
                );
                rgba.push(255);
            }
        }
    }
    let mut image = Image::new(
        Extent3d {
            width: SIDE,
            height: SIDE,
            depth_or_array_layers: 6,
        },
        TextureDimension::D2,
        rgba,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    );
    image.texture_view_descriptor = Some(TextureViewDescriptor {
        dimension: Some(TextureViewDimension::Cube),
        ..default()
    });
    image.sampler = ImageSampler::linear();
    image
}
