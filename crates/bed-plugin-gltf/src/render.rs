//! A windowless Bevy renderer driven by Bed, sharing its device, queue and output texture.
use crate::{
    camera::Camera as OrbitCamera,
    debug_views::{DebugViews, DisplayMode},
    model::{Scene, TextureInfo},
};
use bed_plugin::gpu::{GpuContext, RenderTarget, wgpu};
use bevy::{
    anti_alias::taa::TemporalAntiAliasing,
    app::{PanicHandlerPlugin, TerminalCtrlCHandlerPlugin},
    asset::RenderAssetUsages,
    camera::{Exposure, Hdr, ManualTextureViewHandle, RenderTarget as BevyTarget},
    core_pipeline::tonemapping::Tonemapping,
    image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor},
    light::{CascadeShadowConfigBuilder, GeneratedEnvironmentMapLight, Skybox},
    log::LogPlugin,
    mesh::{Indices, PrimitiveTopology, UvChannel},
    pbr::{ScreenSpaceAmbientOcclusion, ScreenSpaceAmbientOcclusionQualityLevel},
    prelude::*,
    render::{
        RenderApp, RenderPlugin,
        error_handler::{ErrorType, RenderErrorHandler, RenderErrorPolicy},
        pipelined_rendering::PipelinedRenderingPlugin,
        render_resource::{
            CachedPipelineState, Extent3d, Face, PipelineCache, TextureDimension, TextureFormat,
            TextureViewDescriptor, TextureViewDimension,
        },
        renderer::{
            RenderAdapter, RenderAdapterInfo, RenderDevice, RenderInstance, RenderQueue,
            WgpuWrapper,
        },
        settings::RenderCreation,
        texture::{ManualTextureView, ManualTextureViews},
    },
    shader::ShaderCacheError,
    window::{ExitCondition, WindowPlugin},
};
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc};

const OUTPUT: ManualTextureViewHandle = ManualTextureViewHandle(1);
pub(super) const SETTLE_FRAMES: u8 = 16;

#[derive(Default, Resource)]
struct RenderFailure(Option<(bool, String)>);

fn error_scopes(device: &wgpu::Device) -> [wgpu::ErrorScopeGuard; 3] {
    [
        device.push_error_scope(wgpu::ErrorFilter::OutOfMemory),
        device.push_error_scope(wgpu::ErrorFilter::Internal),
        device.push_error_scope(wgpu::ErrorFilter::Validation),
    ]
}
fn check_scopes(scopes: [wgpu::ErrorScopeGuard; 3]) -> Result<(), String> {
    let mut failure = None;
    for scope in scopes.into_iter().rev() {
        if let Some(error) = bevy::tasks::block_on(scope.pop()) {
            failure = Some(error.to_string());
        }
    }
    failure.map_or(Ok(()), Err)
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) enum Lighting {
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

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Settings {
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
impl Settings {
    pub fn restore(state: &Value) -> Self {
        let mut result = Self::default();
        let value = &state["render"];
        result.lighting = Lighting::from_index(value["lighting"].as_u64().unwrap_or(0) as usize);
        result.display = DisplayMode::from_index(value["display"].as_u64().unwrap_or(0) as usize);
        result.normals = value["normals"].as_bool().unwrap_or(result.normals);
        result.normal_length = value["normal_length"]
            .as_f64()
            .filter(|x| x.is_finite())
            .unwrap_or(f64::from(result.normal_length))
            .clamp(0.01, 0.3) as f32;
        result.skybox = value["skybox"].as_bool().unwrap_or(result.skybox);
        result.skybox_blur = value["skybox_blur"]
            .as_f64()
            .filter(|x| x.is_finite())
            .unwrap_or(0.0)
            .clamp(0.0, 1.0) as f32;
        result.horizon = value["horizon"]
            .as_f64()
            .filter(|x| x.is_finite())
            .unwrap_or(f64::from(result.horizon))
            .clamp(-45.0, 45.0) as f32;
        result.shadows = value["shadows"].as_bool().unwrap_or(result.shadows);
        result.ao = value["ao"].as_bool().unwrap_or(result.ao);
        result.exposure = value["exposure"]
            .as_f64()
            .filter(|x| x.is_finite())
            .unwrap_or(0.0)
            .clamp(-4.0, 4.0) as f32;
        result
    }
    pub fn save(self) -> Value {
        json!({ "lighting": self.lighting.index(), "display": self.display.index(),
            "normals": self.normals, "normal_length": self.normal_length,
            "skybox": self.skybox, "skybox_blur": self.skybox_blur, "horizon": self.horizon,
            "shadows": self.shadows, "ao": self.ao, "exposure": self.exposure })
    }
}

pub(super) struct SceneGpu {
    pub generation: u64,
    pub ao_supported: bool,
    app: App,
    camera: Entity,
    key: Entity,
    fill: Entity,
    environments: [Handle<Image>; 5],
    skybox_image: Handle<Image>,
    debug: DebugViews,
    center: Vec3,
    radius: f32,
    settings: Option<Settings>,
}
impl SceneGpu {
    pub fn new(gpu: &GpuContext<'_>, scene: &Scene) -> Result<Self, String> {
        if gpu.device.limits().max_storage_buffers_per_shader_stage < 6 {
            return Err(
                "The model renderer requires at least six storage buffers per shader stage".into(),
            );
        }
        let limits = gpu.device.limits();
        if limits.max_storage_textures_per_shader_stage < 6
            || limits.max_compute_workgroup_storage_size == 0
            || limits.max_compute_workgroup_size_x == 0
            || !gpu
                .adapter
                .get_downlevel_capabilities()
                .flags
                .contains(wgpu::DownlevelFlags::COMPUTE_SHADERS)
        {
            return Err("The model renderer requires GPU compute support and at least six storage textures per shader stage for environment lighting".into());
        }
        let scopes = error_scopes(gpu.device);
        let resources = RenderCreation::manual(
            RenderDevice::from(gpu.device.clone()),
            RenderQueue(Arc::new(WgpuWrapper::new(gpu.queue.clone()))),
            RenderAdapterInfo(WgpuWrapper::new(gpu.adapter.get_info())),
            RenderAdapter(Arc::new(WgpuWrapper::new(gpu.adapter.clone()))),
            RenderInstance(Arc::new(WgpuWrapper::new(gpu.instance.clone()))),
        );
        let mut app = App::new();
        app.add_plugins(
            DefaultPlugins
                .set(WindowPlugin {
                    primary_window: None,
                    exit_condition: ExitCondition::DontExit,
                    ..default()
                })
                .set(RenderPlugin {
                    render_creation: resources,
                    synchronous_pipeline_compilation: true,
                    ..default()
                })
                .disable::<PipelinedRenderingPlugin>()
                .disable::<PanicHandlerPlugin>()
                .disable::<TerminalCtrlCHandlerPlugin>()
                .disable::<LogPlugin>(),
        );
        let skybox_image = crate::skybox_blur::install(&mut app);
        app.init_resource::<RenderFailure>();
        app.add_plugins(bevy_stl::StlPlugin);
        app.insert_resource(RenderErrorHandler(|error, world, _| {
            world.resource_mut::<RenderFailure>().0 = Some((
                error.ty == ErrorType::DeviceLost,
                format!(
                    "Bevy rendering error: {:?}: {}",
                    error.ty, error.description
                ),
            ));
            RenderErrorPolicy::StopRendering
        }));
        app.finish();
        // Bevy installs its own device callbacks during finish. Restore Bed's
        // handlers before any render so every panel shares host-owned recovery.
        gpu.restore_host_handlers();
        app.cleanup();
        app.insert_resource(GlobalAmbientLight {
            brightness: 0.0,
            ..default()
        });
        let radius = scene.radius();
        let center = Vec3::from_array(scene.center().to_array());
        // Work in a unit-radius scene so CAD millimeters and very large scenes
        // share sensible shadow, depth and SSAO precision.
        let normalization =
            Transform::from_scale(Vec3::splat(radius.recip())).with_translation(-center / radius);
        let mut textures = HashMap::new();
        let mut surfaces = Vec::with_capacity(scene.primitives.len());
        for primitive in &scene.primitives {
            let mut mesh = Mesh::new(
                PrimitiveTopology::TriangleList,
                RenderAssetUsages::RENDER_WORLD,
            );
            mesh.insert_attribute(
                Mesh::ATTRIBUTE_POSITION,
                primitive
                    .vertices
                    .iter()
                    .map(|v| v.position)
                    .collect::<Vec<_>>(),
            );
            mesh.insert_attribute(
                Mesh::ATTRIBUTE_NORMAL,
                primitive
                    .vertices
                    .iter()
                    .map(|v| v.normal)
                    .collect::<Vec<_>>(),
            );
            let uv0 = primitive.vertices.iter().map(|v| v.uv0).collect::<Vec<_>>();
            mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uv0.clone());
            mesh.insert_attribute(
                Mesh::ATTRIBUTE_UV_1,
                primitive.vertices.iter().map(|v| v.uv1).collect::<Vec<_>>(),
            );
            mesh.insert_attribute(
                Mesh::ATTRIBUTE_COLOR,
                primitive
                    .vertices
                    .iter()
                    .map(|v| v.color)
                    .collect::<Vec<_>>(),
            );
            mesh.insert_indices(Indices::U32(primitive.indices.clone()));
            if primitive.material.normal_texture.is_some() {
                if primitive.has_tangents {
                    mesh.insert_attribute(
                        Mesh::ATTRIBUTE_TANGENT,
                        primitive
                            .vertices
                            .iter()
                            .map(|v| v.tangent)
                            .collect::<Vec<_>>(),
                    );
                } else {
                    if primitive
                        .material
                        .normal_texture
                        .is_some_and(|texture| texture.tex_coord == 1)
                    {
                        mesh.insert_attribute(
                            Mesh::ATTRIBUTE_UV_0,
                            primitive.vertices.iter().map(|v| v.uv1).collect::<Vec<_>>(),
                        );
                    }
                    mesh.generate_tangents().map_err(|error| {
                        format!("Could not generate glTF normal-map tangents: {error}")
                    })?;
                    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uv0);
                }
            }
            let source = &primitive.material;
            let channel = |texture: Option<TextureInfo>| {
                if texture.is_some_and(|t| t.tex_coord == 1) {
                    UvChannel::Uv1
                } else {
                    UvChannel::Uv0
                }
            };
            let mut images = app.world_mut().resource_mut::<Assets<Image>>();
            let mut texture =
                |info, variant| image_handle(&mut images, &mut textures, scene, info, variant);
            let material = StandardMaterial {
                base_color: Color::linear_rgba(
                    source.color[0],
                    source.color[1],
                    source.color[2],
                    source.color[3],
                ),
                base_color_texture: texture(source.texture, ImageKind::Color),
                base_color_channel: channel(source.texture),
                metallic: source.metallic,
                perceptual_roughness: source.roughness,
                metallic_roughness_texture: texture(
                    source.metallic_roughness_texture,
                    ImageKind::Linear,
                ),
                metallic_roughness_channel: channel(source.metallic_roughness_texture),
                normal_map_texture: texture(
                    source.normal_texture,
                    ImageKind::Normal(source.normal_scale),
                ),
                normal_map_channel: channel(source.normal_texture),
                occlusion_texture: texture(
                    source.occlusion_texture,
                    ImageKind::Occlusion(source.occlusion_strength),
                ),
                occlusion_channel: channel(source.occlusion_texture),
                emissive: LinearRgba::rgb(
                    source.emissive[0],
                    source.emissive[1],
                    source.emissive[2],
                ),
                emissive_texture: texture(source.emissive_texture, ImageKind::Color),
                emissive_channel: channel(source.emissive_texture),
                alpha_mode: match source.alpha {
                    gltf::material::AlphaMode::Opaque => AlphaMode::Opaque,
                    gltf::material::AlphaMode::Mask => AlphaMode::Mask(source.cutoff),
                    gltf::material::AlphaMode::Blend => AlphaMode::Blend,
                },
                double_sided: source.double_sided,
                cull_mode: if source.double_sided {
                    None
                } else {
                    Some(Face::Back)
                },
                unlit: source.unlit,
                ..default()
            };
            let mesh = app.world_mut().resource_mut::<Assets<Mesh>>().add(mesh);
            let material = app
                .world_mut()
                .resource_mut::<Assets<StandardMaterial>>()
                .add(material);
            surfaces.push(
                app.world_mut()
                    .spawn((Mesh3d(mesh), MeshMaterial3d(material), normalization))
                    .id(),
            );
        }
        let debug = DebugViews::new(app.world_mut(), scene, &surfaces, normalization);
        let mut environments = Vec::with_capacity(Lighting::NAMES.len());
        for index in 0..Lighting::NAMES.len() {
            let image = match index {
                3.. => crate::environment::bundled(index - 3)?,
                _ => environment(Lighting::from_index(index)),
            };
            environments.push(app.world_mut().resource_mut::<Assets<Image>>().add(image));
        }
        let environments = environments.try_into().unwrap();
        let camera = app
            .world_mut()
            .spawn((
                Camera3d::default(),
                Camera::default(),
                BevyTarget::TextureView(OUTPUT),
                Hdr,
                Msaa::Off,
                Tonemapping::AcesFitted,
                Exposure { ev100: 12.0 },
                TemporalAntiAliasing::default(),
                Transform::IDENTITY,
            ))
            .id();
        let key = app
            .world_mut()
            .spawn((
                DirectionalLight::default(),
                Transform::from_xyz(0.5, 0.8, 0.6).looking_at(Vec3::ZERO, Vec3::Y),
            ))
            .id();
        let fill = app
            .world_mut()
            .spawn((
                DirectionalLight::default(),
                Transform::from_xyz(-0.6, 0.3, -0.5).looking_at(Vec3::ZERO, Vec3::Y),
            ))
            .id();
        check_scopes(scopes)?;
        Ok(Self {
            generation: gpu.generation,
            ao_supported: limits.max_storage_textures_per_shader_stage >= 5,
            app,
            camera,
            key,
            fill,
            environments,
            skybox_image,
            debug,
            center,
            radius,
            settings: None,
        })
    }
    pub fn render(
        &mut self,
        gpu: &mut GpuContext<'_>,
        target: &RenderTarget,
        camera: OrbitCamera,
        _radius: f32,
        background: [f32; 4],
        settings: Settings,
    ) -> Result<(), String> {
        let scopes = error_scopes(gpu.device);
        let view = target.texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(wgpu::TextureFormat::Rgba8UnormSrgb),
            ..default()
        });
        self.app
            .world_mut()
            .resource_mut::<ManualTextureViews>()
            .insert(
                OUTPUT,
                ManualTextureView {
                    texture_view: view.into(),
                    size: UVec2::from_array(target.size),
                    view_format: TextureFormat::Rgba8UnormSrgb,
                },
            );
        let eye = (Vec3::from_array(camera.eye().to_array()) - self.center) / self.radius;
        let focus = (Vec3::from_array(camera.target.to_array()) - self.center) / self.radius;
        {
            let mut entity = self.app.world_mut().entity_mut(self.camera);
            *entity.get_mut::<Transform>().unwrap() =
                Transform::from_translation(eye).looking_at(focus, Vec3::Y);
            entity.get_mut::<Camera>().unwrap().clear_color =
                Color::srgba(background[0], background[1], background[2], background[3]).into();
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
            self.configure(settings);
            self.settings = Some(settings);
        }
        self.debug
            .update_view(self.app.world_mut(), (eye - focus).normalize_or_zero());
        // Skyboxes are infinitely distant: moving the model cannot lower their
        // horizon. Pitch the background around the camera's right axis instead,
        // keeping the model and environment lighting fixed as the view orbits.
        {
            let mut entity = self.app.world_mut().entity_mut(self.camera);
            let right = entity.get::<Transform>().unwrap().right().as_vec3();
            if let Some(mut skybox) = entity.get_mut::<Skybox>() {
                skybox.rotation = Quat::from_axis_angle(right, settings.horizon.to_radians());
            }
        }
        self.app.world_mut().entity_mut(self.key).insert(
            CascadeShadowConfigBuilder {
                num_cascades: 1,
                minimum_distance: 0.001,
                maximum_distance: (camera.distance / self.radius + 3.0).max(4.0),
                ..default()
            }
            .build(),
        );
        self.app.update();
        check_scopes(scopes)?;
        if let Some((lost, message)) = self
            .app
            .world_mut()
            .resource_mut::<RenderFailure>()
            .0
            .take()
        {
            if lost {
                gpu.report_device_lost(message.clone());
            }
            return Err(message);
        }
        if let Some(error) = self
            .app
            .sub_app(RenderApp)
            .world()
            .resource::<PipelineCache>()
            .pipelines()
            .find_map(|pipeline| match &pipeline.state {
                CachedPipelineState::Err(
                    ShaderCacheError::ShaderNotLoaded(_)
                    | ShaderCacheError::ShaderImportNotYetAvailable,
                ) => None,
                CachedPipelineState::Err(error) => Some(error.to_string()),
                _ => None,
            })
        {
            return Err(format!("Could not compile model shader: {error}"));
        }
        Ok(())
    }
    pub fn pending(&self) -> bool {
        self.app
            .sub_app(RenderApp)
            .world()
            .resource::<PipelineCache>()
            .waiting_pipelines()
            .next()
            .is_some()
    }
    fn configure(&mut self, settings: Settings) {
        self.debug.apply(
            self.app.world_mut(),
            settings.display,
            settings.normals,
            settings.normal_length,
        );
        let (key, fill, tint, intensity) = match settings.lighting {
            Lighting::Studio => (18000.0, 4500.0, Color::srgb(1.0, 0.94, 0.86), 1800.0),
            Lighting::Outdoor => (32000.0, 1000.0, Color::srgb(1.0, 0.97, 0.90), 2400.0),
            Lighting::Neutral => (10000.0, 10000.0, Color::WHITE, 1200.0),
            // Let the photographed environment supply the lighting rather than
            // baking the procedural presets' key/fill into HDRI appearances.
            Lighting::StudioSmall08 => (0.0, 0.0, Color::WHITE, 1800.0),
            Lighting::KiaraDawn => (0.0, 0.0, Color::WHITE, 1800.0),
        };
        self.app
            .world_mut()
            .entity_mut(self.key)
            .insert(DirectionalLight {
                color: tint,
                illuminance: key,
                shadow_maps_enabled: settings.shadows && key > 0.0,
                ..default()
            });
        self.app
            .world_mut()
            .entity_mut(self.fill)
            .insert(DirectionalLight {
                color: Color::srgb(0.83, 0.9, 1.0),
                illuminance: fill,
                ..default()
            });
        let environment = self.environments[settings.lighting.index()].clone();
        crate::skybox_blur::configure(&mut self.app, settings.skybox_blur);
        let mut entity = self.app.world_mut().entity_mut(self.camera);
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
        if self
            .settings
            .is_some_and(|old| old.skybox_blur != settings.skybox_blur)
        {
            // Discard stale background samples so the blur slider responds
            // immediately rather than blending in the previous skybox detail.
            entity.get_mut::<TemporalAntiAliasing>().unwrap().reset = true;
        }
        if settings.ao && self.ao_supported {
            entity.insert(ScreenSpaceAmbientOcclusion {
                quality_level: ScreenSpaceAmbientOcclusionQualityLevel::Medium,
                constant_object_thickness: 0.1,
            });
        } else {
            entity.remove::<ScreenSpaceAmbientOcclusion>();
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum ImageKind {
    Color,
    Linear,
    Normal(f32),
    Occlusion(f32),
}
fn image_handle(
    images: &mut Assets<Image>,
    cache: &mut HashMap<String, Handle<Image>>,
    scene: &Scene,
    info: Option<TextureInfo>,
    kind: ImageKind,
) -> Option<Handle<Image>> {
    let info = info?;
    let key = format!("{}:{:?}:{kind:?}", info.image, info.sampler);
    Some(
        cache
            .entry(key)
            .or_insert_with(|| {
                let source = &scene.images[info.image];
                let mut rgba = source.rgba.to_vec();
                adjust_pixels(&mut rgba, kind);
                let mut image = Image::new(
                    Extent3d {
                        width: source.size[0],
                        height: source.size[1],
                        depth_or_array_layers: 1,
                    },
                    TextureDimension::D2,
                    rgba,
                    if matches!(kind, ImageKind::Color) {
                        TextureFormat::Rgba8UnormSrgb
                    } else {
                        TextureFormat::Rgba8Unorm
                    },
                    RenderAssetUsages::RENDER_WORLD,
                );
                let wrap = |value| match value {
                    gltf::texture::WrappingMode::ClampToEdge => ImageAddressMode::ClampToEdge,
                    gltf::texture::WrappingMode::MirroredRepeat => ImageAddressMode::MirrorRepeat,
                    gltf::texture::WrappingMode::Repeat => ImageAddressMode::Repeat,
                };
                image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
                    address_mode_u: wrap(info.sampler.wrap[0]),
                    address_mode_v: wrap(info.sampler.wrap[1]),
                    mag_filter: if info.sampler.mag_filter
                        == Some(gltf::texture::MagFilter::Nearest)
                    {
                        ImageFilterMode::Nearest
                    } else {
                        ImageFilterMode::Linear
                    },
                    min_filter: if matches!(
                        info.sampler.min_filter,
                        Some(
                            gltf::texture::MinFilter::Nearest
                                | gltf::texture::MinFilter::NearestMipmapNearest
                                | gltf::texture::MinFilter::NearestMipmapLinear
                        )
                    ) {
                        ImageFilterMode::Nearest
                    } else {
                        ImageFilterMode::Linear
                    },
                    ..default()
                });
                images.add(image)
            })
            .clone(),
    )
}
fn adjust_pixels(rgba: &mut [u8], kind: ImageKind) {
    for pixel in rgba.as_chunks_mut::<4>().0 {
        match kind {
            ImageKind::Normal(scale) if scale != 1.0 => {
                let mut normal = Vec3::new(
                    f32::from(pixel[0]) / 127.5 - 1.0,
                    f32::from(pixel[1]) / 127.5 - 1.0,
                    f32::from(pixel[2]) / 127.5 - 1.0,
                );
                // Divide by the largest possible factor before multiplying to
                // keep even extreme finite glTF normal scales well-defined.
                let divisor = scale.abs().max(1.0);
                normal = Vec3::new(
                    normal.x * (scale / divisor),
                    normal.y * (scale / divisor),
                    normal.z / divisor,
                )
                .normalize_or_zero();
                for (channel, value) in pixel[..3].iter_mut().zip(normal.to_array()) {
                    *channel = ((value * 0.5 + 0.5) * 255.0).round() as u8;
                }
            }
            ImageKind::Occlusion(strength) => {
                pixel[0] = (255.0 + strength * (f32::from(pixel[0]) - 255.0)).round() as u8
            }
            _ => {}
        }
    }
}

// Small built-in cubemaps keep presets self-contained, including SSH/offline use.
// Bevy filters these into diffuse irradiance and roughness-dependent reflections.
fn environment(lighting: Lighting) -> Image {
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
        RenderAssetUsages::RENDER_WORLD,
    );
    image.texture_view_descriptor = Some(TextureViewDescriptor {
        dimension: Some(TextureViewDimension::Cube),
        ..default()
    });
    image.sampler = ImageSampler::linear();
    image
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normal_and_occlusion_factors_are_applied_in_linear_space() {
        let mut ao = [0, 40, 90, 255];
        adjust_pixels(&mut ao, ImageKind::Occlusion(0.5));
        assert_eq!(ao, [128, 40, 90, 255]);
        let mut normal = [200, 120, 220, 255];
        adjust_pixels(&mut normal, ImageKind::Normal(0.0));
        assert_eq!(normal, [128, 128, 255, 255]);
    }
    #[test]
    fn render_settings_restore_old_sessions_and_bound_exposure() {
        assert_eq!(Settings::restore(&Value::Null), Settings::default());
        let settings = Settings {
            lighting: Lighting::KiaraDawn,
            display: DisplayMode::WireframeOverlay,
            normals: true,
            normal_length: 0.12,
            exposure: 2.0,
            skybox: true,
            skybox_blur: 0.65,
            horizon: -20.0,
            ..default()
        };
        assert_eq!(
            Settings::restore(&json!({"render": settings.save()})),
            settings
        );
        assert_eq!(
            Settings::restore(&json!({"render": {"exposure": 999, "lighting": 99}})).exposure,
            4.0
        );
        let invalid = Settings::restore(&json!({"render": {
            "display": 99, "normal_length": -5.0, "horizon": -999.0, "skybox_blur": 999.0
        }}));
        assert_eq!(invalid.display, DisplayMode::Shaded);
        assert_eq!(invalid.normal_length, 0.01);
        assert_eq!(invalid.horizon, -45.0);
        assert_eq!(invalid.skybox_blur, 1.0);
        assert_eq!(
            Settings::restore(&json!({"render": {"skybox_blur": -5.0}})).skybox_blur,
            0.0
        );
        let legacy = Settings::restore(&json!({"render": {"lighting": 1, "skybox": true}}));
        assert_eq!(legacy.lighting, Lighting::Outdoor);
        assert_eq!(legacy.horizon, -12.0);
        assert_eq!(legacy.skybox_blur, 0.0);
        assert_eq!(legacy.display, DisplayMode::Shaded);
    }
}
