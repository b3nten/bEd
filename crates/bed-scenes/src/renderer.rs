use crate::{SceneFrame, SceneKind};
use bed_workbench_api::gpu::{GpuContext, RenderTarget, wgpu};
use bevy::{
    app::{PanicHandlerPlugin, TerminalCtrlCHandlerPlugin},
    asset::RenderAssetUsages,
    camera::{Exposure, ManualTextureViewHandle, RenderTarget as BevyTarget},
    core_pipeline::tonemapping::Tonemapping,
    image::{CompressedImageFormats, ImageSampler, ImageType},
    log::LogPlugin,
    mesh::{Indices, PrimitiveTopology},
    prelude::*,
    render::{
        RenderPlugin,
        error_handler::{ErrorType, RenderErrorHandler, RenderErrorPolicy},
        pipelined_rendering::PipelinedRenderingPlugin,
        render_resource::{TextureFormat, TextureViewDescriptor},
        renderer::{
            RenderAdapter, RenderAdapterInfo, RenderDevice, RenderInstance, RenderQueue,
            WgpuWrapper,
        },
        settings::RenderCreation,
        texture::{ManualTextureView, ManualTextureViews},
    },
    window::{ExitCondition, WindowPlugin},
};
use std::sync::Arc;

const OUTPUT: ManualTextureViewHandle = ManualTextureViewHandle(1);

#[derive(Default, Resource)]
struct Failure(Option<(bool, String)>);

#[derive(Clone, Copy)]
enum Motion {
    Dust,
    Bed,
    DuckBody,
    DuckNeck,
    Blanket,
}

pub(crate) struct SceneRenderer {
    pub generation: u64,
    app: App,
    kind: SceneKind,
    camera: Entity,
    accent_light: Entity,
    accents: Vec<(Handle<StandardMaterial>, [f32; 3])>,
    bed_cover_material: Option<Handle<StandardMaterial>>,
    motions: Vec<(Entity, Transform, Motion)>,
    bed_bounds: Option<(Vec3, Vec3)>,
}

impl SceneRenderer {
    pub fn new(gpu: &GpuContext<'_>, kind: SceneKind) -> Result<Self, String> {
        if gpu.device.limits().max_storage_buffers_per_shader_stage < 6 {
            return Err(
                "Decorative scenes require six GPU storage buffers per shader stage".into(),
            );
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
        app.init_resource::<Failure>();
        app.insert_resource(RenderErrorHandler(|error, world, _| {
            world.resource_mut::<Failure>().0 = Some((
                error.ty == ErrorType::DeviceLost,
                format!(
                    "Scene rendering error: {:?}: {}",
                    error.ty, error.description
                ),
            ));
            RenderErrorPolicy::StopRendering
        }));
        app.finish();
        gpu.restore_host_handlers();
        app.cleanup();
        app.insert_resource(GlobalAmbientLight {
            brightness: if kind != SceneKind::Bedtime {
                1800.0
            } else {
                350.0
            },
            color: if kind != SceneKind::Bedtime {
                Color::WHITE
            } else {
                Color::srgb(0.65, 0.72, 0.9)
            },
            ..default()
        });
        let camera = app
            .world_mut()
            .spawn((
                Camera3d::default(),
                BevyTarget::TextureView(OUTPUT),
                Camera::default(),
                Msaa::Sample4,
                Tonemapping::AcesFitted,
                Exposure { ev100: 10.0 },
                Transform::IDENTITY,
            ))
            .id();
        app.world_mut().spawn((
            DirectionalLight {
                illuminance: match kind {
                    SceneKind::Bed | SceneKind::WelcomeBed | SceneKind::Duck => 2500.0,
                    SceneKind::Bedtime => 1200.0,
                },
                shadow_maps_enabled: kind == SceneKind::Bedtime,
                ..default()
            },
            Transform::from_xyz(3.0, 5.0, 2.0).looking_at(Vec3::ZERO, Vec3::Y),
        ));
        let accent_light = app
            .world_mut()
            .spawn((
                PointLight {
                    intensity: 300_000.0,
                    range: 8.0,
                    radius: if kind != SceneKind::Bedtime {
                        1.0
                    } else {
                        0.15
                    },
                    ..default()
                },
                Transform::from_xyz(1.0, 1.5, 1.0),
            ))
            .id();
        let mut renderer = Self {
            generation: gpu.generation,
            app,
            kind,
            camera,
            accent_light,
            accents: Vec::new(),
            bed_cover_material: None,
            motions: Vec::new(),
            bed_bounds: None,
        };
        match kind {
            SceneKind::Bed | SceneKind::WelcomeBed => renderer.bed()?,
            SceneKind::Duck => renderer.duck(),
            SceneKind::Bedtime => renderer.bedtime(),
        }
        check_scopes(scopes)?;
        Ok(renderer)
    }

    fn bed(&mut self) -> Result<(), String> {
        // This bundled, static GLB has PBR materials and embedded PNGs. Load
        // it directly so the mascot is ready on the first frame and GPU recovery
        // never depends on a filesystem path or an asynchronous asset lifecycle.
        let gltf = gltf::Gltf::from_slice(include_bytes!("../../../assets/bed.glb"))
            .map_err(|error| format!("Invalid bundled bed model: {error}"))?;
        let blob = gltf
            .blob
            .as_deref()
            .ok_or("Bed model has no embedded buffer")?;
        let mut textures = Vec::new();
        for source in gltf.images() {
            let gltf::image::Source::View { view, mime_type } = source.source() else {
                return Err("Bed textures must be embedded".into());
            };
            let is_color = gltf.materials().any(|material| {
                material
                    .pbr_metallic_roughness()
                    .base_color_texture()
                    .is_some_and(|info| info.texture().source().index() == source.index())
            });
            let image = Image::from_buffer(
                &blob[view.offset()..view.offset() + view.length()],
                ImageType::MimeType(mime_type),
                CompressedImageFormats::NONE,
                is_color,
                ImageSampler::default(),
                RenderAssetUsages::RENDER_WORLD,
            )
            .map_err(|error| format!("Invalid bed texture: {error}"))?;
            textures.push(
                self.app
                    .world_mut()
                    .resource_mut::<Assets<Image>>()
                    .add(image),
            );
        }
        let mut materials = Vec::new();
        for source in gltf.materials() {
            let tinted = source.name() == Some("Tinted");
            let pbr = source.pbr_metallic_roughness();
            let color = pbr.base_color_factor();
            let mut material = StandardMaterial {
                base_color: Color::linear_rgba(color[0], color[1], color[2], color[3]),
                base_color_texture: pbr
                    .base_color_texture()
                    .map(|info| textures[info.texture().source().index()].clone()),
                metallic: pbr.metallic_factor(),
                perceptual_roughness: pbr.roughness_factor(),
                metallic_roughness_texture: pbr
                    .metallic_roughness_texture()
                    .map(|info| textures[info.texture().source().index()].clone()),
                normal_map_texture: source
                    .normal_texture()
                    .map(|info| textures[info.texture().source().index()].clone()),
                occlusion_texture: source
                    .occlusion_texture()
                    .map(|info| textures[info.texture().source().index()].clone()),
                cull_mode: None,
                double_sided: source.double_sided(),
                ..default()
            };
            if tinted {
                // Both materials share the atlas. Neutralize only Tinted's
                // copy, keeping its pattern and the original Untinted colors.
                let mut cover_texture = self
                    .app
                    .world()
                    .resource::<Assets<Image>>()
                    .get(
                        material
                            .base_color_texture
                            .as_ref()
                            .ok_or("Tinted bed material has no color texture")?,
                    )
                    .unwrap()
                    .clone();
                for pixel in cover_texture.data.as_mut().unwrap().as_chunks_mut::<4>().0 {
                    // The orange fabric's green channel is 102, and its cream
                    // motifs are 204. Normalize that contrast to neutral shades.
                    let shade = (f32::from(pixel[1]) / 204.0 * 255.0).min(255.0).round() as u8;
                    pixel[..3].fill(shade);
                }
                material.base_color_texture = Some(
                    self.app
                        .world_mut()
                        .resource_mut::<Assets<Image>>()
                        .add(cover_texture),
                );
            }
            let handle = self
                .app
                .world_mut()
                .resource_mut::<Assets<StandardMaterial>>()
                .add(material);
            if tinted {
                self.bed_cover_material = Some(handle.clone());
            }
            materials.push(handle);
        }
        if self.bed_cover_material.is_none() {
            return Err("Bed model has no Tinted material".into());
        }
        let scene = gltf
            .default_scene()
            .ok_or("Bed model has no default scene")?;
        let mut nodes: Vec<_> = scene.nodes().map(|node| (node, Mat4::IDENTITY)).collect();
        let mut meshes = Vec::new();
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);
        while let Some((node, parent)) = nodes.pop() {
            let world = parent * Mat4::from_cols_array_2d(&node.transform().matrix());
            nodes.extend(node.children().map(|child| (child, world)));
            if let Some(mesh) = node.mesh() {
                for primitive in mesh.primitives() {
                    let material = &materials[primitive
                        .material()
                        .index()
                        .ok_or("Bed mesh has no material")?];
                    let reader = primitive.reader(|_| Some(blob));
                    let positions: Vec<_> = reader
                        .read_positions()
                        .ok_or("Bed mesh has no positions")?
                        .map(|position| world.transform_point3(Vec3::from_array(position)))
                        .collect();
                    for &position in &positions {
                        min = min.min(position);
                        max = max.max(position);
                    }
                    let normal_transform = world.inverse().transpose();
                    let normals: Vec<_> = reader
                        .read_normals()
                        .ok_or("Bed mesh has no normals")?
                        .map(|normal| {
                            normal_transform
                                .transform_vector3(Vec3::from_array(normal))
                                .normalize()
                                .to_array()
                        })
                        .collect();
                    let tangents = reader.read_tangents().map(|tangents| {
                        tangents
                            .map(|tangent| {
                                let direction = world
                                    .transform_vector3(Vec3::new(
                                        tangent[0], tangent[1], tangent[2],
                                    ))
                                    .normalize();
                                [direction.x, direction.y, direction.z, tangent[3]]
                            })
                            .collect::<Vec<_>>()
                    });
                    let uvs: Vec<_> = reader
                        .read_tex_coords(0)
                        .ok_or("Bed mesh has no texture coordinates")?
                        .into_f32()
                        .collect();
                    let indices: Vec<_> = reader
                        .read_indices()
                        .ok_or("Bed mesh has no indices")?
                        .into_u32()
                        .collect();
                    meshes.push((positions, normals, tangents, uvs, indices, material.clone()));
                }
            }
        }
        let center = (min + max) * 0.5;
        let scale = 2.4 / (max - min).max_element();
        self.bed_bounds = Some(((min - center) * scale, (max - center) * scale));
        let root = self.app.world_mut().spawn(Transform::IDENTITY).id();
        self.motions.push((root, Transform::IDENTITY, Motion::Bed));
        for (positions, normals, tangents, uvs, indices, material) in meshes {
            let positions: Vec<_> = positions
                .into_iter()
                .map(|position| ((position - center) * scale).to_array())
                .collect();
            let mut mesh = Mesh::new(
                PrimitiveTopology::TriangleList,
                RenderAssetUsages::RENDER_WORLD,
            )
            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
            .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
            .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
            .with_inserted_indices(Indices::U32(indices));
            if let Some(tangents) = tangents {
                mesh.insert_attribute(Mesh::ATTRIBUTE_TANGENT, tangents);
            } else {
                // Blender exports can omit tangents; the atlas normal map
                // still needs them for the blanket folds and wood detail.
                mesh.generate_tangents()
                    .map_err(|error| format!("Invalid bed mesh tangents: {error}"))?;
            }
            let entity = self.mesh(mesh, material, Transform::IDENTITY, None);
            self.app
                .world_mut()
                .entity_mut(entity)
                .insert(ChildOf(root));
        }
        Ok(())
    }

    fn material(
        &mut self,
        color: [f32; 3],
        accent: bool,
        emissive: f32,
    ) -> Handle<StandardMaterial> {
        let material = self
            .app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial {
                base_color: Color::srgb(color[0], color[1], color[2]),
                emissive: LinearRgba::rgb(
                    color[0] * emissive,
                    color[1] * emissive,
                    color[2] * emissive,
                ),
                perceptual_roughness: 0.65,
                ..default()
            });
        if accent {
            self.accents.push((material.clone(), color));
        }
        material
    }
    fn mesh(
        &mut self,
        mesh: Mesh,
        material: Handle<StandardMaterial>,
        transform: Transform,
        motion: Option<Motion>,
    ) -> Entity {
        let mesh = self
            .app
            .world_mut()
            .resource_mut::<Assets<Mesh>>()
            .add(mesh);
        let entity = self
            .app
            .world_mut()
            .spawn((Mesh3d(mesh), MeshMaterial3d(material), transform))
            .id();
        if let Some(motion) = motion {
            self.motions.push((entity, transform, motion));
        }
        entity
    }
    fn cube(
        &mut self,
        size: [f32; 3],
        pos: [f32; 3],
        mat: Handle<StandardMaterial>,
        motion: Option<Motion>,
    ) -> Entity {
        self.mesh(
            Cuboid::new(size[0], size[1], size[2]).into(),
            mat,
            Transform::from_translation(Vec3::from_array(pos)),
            motion,
        )
    }
    fn ball(
        &mut self,
        scale: [f32; 3],
        pos: [f32; 3],
        mat: Handle<StandardMaterial>,
        motion: Option<Motion>,
    ) -> Entity {
        self.mesh(
            Sphere::new(1.0).mesh().uv(24, 16),
            mat,
            Transform::from_translation(Vec3::from_array(pos)).with_scale(Vec3::from_array(scale)),
            motion,
        )
    }

    fn duck(&mut self) {
        // Two joints are enough for this rigid rubber duck: the body owns the
        // bob and squash, while the neck turns the entire face together.
        let body_pose = Transform::IDENTITY;
        let body = self
            .app
            .world_mut()
            .spawn((body_pose, Visibility::Inherited))
            .id();
        self.motions.push((body, body_pose, Motion::DuckBody));
        let neck_pose = Transform::from_xyz(0.0, 0.28, 0.12);
        let neck = self
            .app
            .world_mut()
            .spawn((neck_pose, Visibility::Inherited, ChildOf(body)))
            .id();
        self.motions.push((neck, neck_pose, Motion::DuckNeck));
        let yellow = self.material([1.0, 0.78, 0.055], false, 0.0);
        let orange = self.material([1.0, 0.38, 0.035], false, 0.0);
        let eye = self.material([0.015, 0.015, 0.022], false, 0.0);
        let shine = self.material([1.0; 3], false, 0.0);
        let torso = self.ball([0.85, 0.65, 0.72], [0.0, -0.03, 0.0], yellow.clone(), None);
        self.app.world_mut().entity_mut(torso).insert(ChildOf(body));
        let head = self.ball([0.53; 3], [0.0, 0.34, 0.11], yellow.clone(), None);
        self.app.world_mut().entity_mut(head).insert(ChildOf(neck));
        let beak = self.ball([0.36, 0.105, 0.35], [0.0, 0.2, 0.6], orange, None);
        self.app.world_mut().entity_mut(beak).insert(ChildOf(neck));
        for sign in [-1.0, 1.0] {
            let eye = self.ball(
                [0.11, 0.12, 0.055],
                [sign * 0.23, 0.45, 0.545],
                eye.clone(),
                None,
            );
            self.app.world_mut().entity_mut(eye).insert(ChildOf(neck));
            let glint = self.ball(
                [0.027; 3],
                [sign * 0.23 - 0.027, 0.49, 0.59],
                shine.clone(),
                None,
            );
            self.app.world_mut().entity_mut(glint).insert(ChildOf(neck));
            let wing = self.ball(
                [0.23, 0.35, 0.48],
                [sign * 0.66, 0.03, -0.08],
                yellow.clone(),
                None,
            );
            self.app.world_mut().entity_mut(wing).insert(ChildOf(body));
        }
        let tail = self.ball([0.28, 0.24, 0.37], [0.0, 0.12, -0.59], yellow, None);
        self.app.world_mut().entity_mut(tail).insert(ChildOf(body));
    }

    fn bedtime(&mut self) {
        let wood = self.material([0.28, 0.14, 0.08], false, 0.0);
        let mattress = self.material([0.9, 0.88, 0.8], false, 0.0);
        let blanket = self.material([0.25, 0.45, 0.8], true, 0.0);
        let pillow = self.material([0.95, 0.94, 0.86], false, 0.0);
        let skin = self.material([0.89, 0.62, 0.42], false, 0.0);
        let hair = self.material([0.16, 0.09, 0.055], false, 0.0);
        let eyes = self.material([0.08, 0.045, 0.03], false, 0.0);
        self.cube([1.6, 0.28, 2.5], [0.0, -0.6, 0.0], wood.clone(), None);
        self.cube([1.7, 1.15, 0.15], [0.0, -0.13, -1.2], wood.clone(), None);
        for x in [-0.65, 0.65] {
            for z in [-1.0, 1.0] {
                self.cube([0.12, 0.5, 0.12], [x, -0.83, z], wood.clone(), None);
            }
        }
        self.cube([1.53, 0.2, 2.3], [0.0, -0.35, 0.0], mattress, None);
        self.ball([0.61, 0.13, 0.32], [0.0, -0.19, -0.75], pillow, None);
        self.ball(
            [0.63, 0.23, 0.87],
            [0.0, -0.09, 0.3],
            blanket,
            Some(Motion::Blanket),
        );
        self.ball([0.255, 0.19, 0.25], [0.0, 0.0, -0.65], skin, None);
        self.ball([0.26, 0.11, 0.22], [0.0, 0.1, -0.72], hair, None);
        for x in [-0.09, 0.09] {
            self.cube([0.095, 0.012, 0.017], [x, 0.14, -0.51], eyes.clone(), None);
        }
        let floor = self.material([0.085, 0.09, 0.15], false, 0.0);
        self.cube([4.4, 0.1, 4.0], [0.0, -1.09, 0.0], floor, None);
        self.cube([0.6, 0.45, 0.6], [1.3, -0.84, -0.75], wood, None);
        let moon = self.material([0.8, 0.85, 1.0], false, 3.0);
        self.ball([0.22; 3], [-1.2, 1.1, -1.3], moon.clone(), None);
        for i in 0..12 {
            self.ball(
                [0.012; 3],
                [
                    (i as f32 * 1.8).sin() * 1.7,
                    0.9 + (i as f32 * 2.4).cos() * 0.65,
                    -1.7,
                ],
                moon.clone(),
                Some(Motion::Dust),
            );
        }
    }

    pub fn render(
        &mut self,
        gpu: &mut GpuContext<'_>,
        target: &RenderTarget,
        frame: SceneFrame,
        time: f32,
        impulse: f32,
    ) -> Result<(), String> {
        let scopes = error_scopes(gpu.device);
        let view = target.texture.create_view(&TextureViewDescriptor {
            format: Some(TextureFormat::Rgba8UnormSrgb),
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
        let pointer = if frame.animations {
            frame.mouse
        } else {
            [0.0; 2]
        };
        let (eye, focus) = match self.kind {
            SceneKind::Bed | SceneKind::WelcomeBed => (Vec3::new(2.6, 2.5, 3.8), Vec3::ZERO),
            SceneKind::Duck => (
                Vec3::new(pointer[0] * 0.18, 0.9, 3.8),
                Vec3::new(0.0, 0.2, 0.0),
            ),
            SceneKind::Bedtime => (Vec3::new(2.5, 2.0, 3.7), Vec3::new(0.0, -0.25, 0.0)),
        };
        let mut camera_transform = Transform::from_translation(eye).looking_at(focus, Vec3::Y);
        if self.kind == SceneKind::Duck {
            // Fit the duck to the limiting canvas dimension, including room for
            // its turned head and click squash. A portrait sidebar needs more
            // distance than a wide panel, whose height determines the fit.
            let aspect = target.size[0] as f32 / target.size[1] as f32;
            let half_fov = PerspectiveProjection::default().fov * 0.5;
            let distance = (0.98 / (half_fov.tan() * 0.99))
                .max(1.06 / (aspect * half_fov.tan() * 0.96))
                + 0.35;
            // Lift the camera along its up axis to place Ducky slightly lower.
            camera_transform.translation =
                focus + (eye - focus).normalize() * distance + camera_transform.up() * 0.1;
        }
        if let Some((min, max)) = self.bed_bounds {
            // Fit the rotated bounding box in either a sidebar or a wide panel.
            let aspect = target.size[0] as f32 / target.size[1] as f32;
            let tan_fov = (PerspectiveProjection::default().fov * 0.5).tan();
            let pose = bed_pose(pointer, impulse);
            let camera_inverse = camera_transform.rotation.inverse();
            let mut distance = 0.0_f32;
            for x in [min.x, max.x] {
                for y in [min.y, max.y] {
                    for z in [min.z, max.z] {
                        let corner = camera_inverse * pose.transform_point(Vec3::new(x, y, z));
                        distance = distance.max(
                            corner.z
                                + (corner.x.abs() / aspect).max(corner.y.abs()) / (tan_fov * 0.82),
                        );
                    }
                }
            }
            camera_transform.translation = eye.normalize() * distance;
            if self.kind == SceneKind::WelcomeBed {
                // A fixed fit for every rotation keeps the welcome model from
                // appearing to zoom as its silhouette changes.
                let radius = min.abs().max(max.abs()).length();
                let half_fov = (tan_fov * aspect.min(1.0)).atan();
                camera_transform.translation = eye.normalize() * radius / (half_fov.sin() * 0.88);
            }
        }
        {
            let mut camera = self.app.world_mut().entity_mut(self.camera);
            *camera.get_mut::<Transform>().unwrap() = camera_transform;
            camera.get_mut::<Camera>().unwrap().clear_color = if self.kind == SceneKind::Bedtime {
                Color::srgba(
                    frame.background[0],
                    frame.background[1],
                    frame.background[2],
                    1.0,
                )
            } else {
                Color::NONE
            }
            .into();
        }
        {
            let mut materials = self
                .app
                .world_mut()
                .resource_mut::<Assets<StandardMaterial>>();
            if let Some(handle) = &self.bed_cover_material {
                materials.get_mut(handle).unwrap().base_color =
                    Color::srgb(frame.accent[0], frame.accent[1], frame.accent[2]);
            }
            for (handle, original) in &self.accents {
                if let Some(mut material) = materials.get_mut(handle) {
                    let tint: [f32; 3] =
                        std::array::from_fn(|i| original[i] * 0.35 + frame.accent[i] * 0.65);
                    material.base_color = Color::srgb(tint[0], tint[1], tint[2]);
                }
            }
        }
        {
            let mut light = self.app.world_mut().entity_mut(self.accent_light);
            match self.kind {
                SceneKind::Bed | SceneKind::WelcomeBed | SceneKind::Duck => {
                    // A fixed, neutral fill keeps the mascots evenly lit as
                    // they turn, without a moving highlight or self shadows.
                    *light.get_mut::<Transform>().unwrap() = Transform::from_xyz(-2.0, 2.0, 3.0);
                    let mut point = light.get_mut::<PointLight>().unwrap();
                    point.color = Color::WHITE;
                    point.intensity = 40_000.0;
                }
                SceneKind::Bedtime => {
                    *light.get_mut::<Transform>().unwrap() = Transform::from_xyz(
                        time.cos() * 1.2,
                        1.2 + (time * 0.7).sin() * 0.25,
                        (time * 0.65).sin() * 1.2,
                    );
                    let mut point = light.get_mut::<PointLight>().unwrap();
                    point.color = Color::srgb(frame.accent[0], frame.accent[1], frame.accent[2]);
                    point.intensity = 40_000.0;
                }
            }
        }
        for (i, (entity, base, motion)) in self.motions.iter().enumerate() {
            let mut transform = *base;
            match motion {
                Motion::Bed => {
                    transform = if self.kind == SceneKind::WelcomeBed {
                        Transform::from_rotation(Quat::from_rotation_y(time * 0.16))
                    } else {
                        bed_pose(pointer, impulse)
                    };
                }
                Motion::Dust => {
                    transform.translation.y += (time * 0.3 + i as f32).sin() * 0.08;
                }
                Motion::DuckBody => {
                    let squash = Vec3::new(
                        1.0 + impulse * 0.18,
                        1.0 - impulse * 0.24,
                        1.0 + impulse * 0.18,
                    );
                    transform.translation.y += (time * 0.45).sin() * 0.015;
                    transform.rotation =
                        Quat::from_rotation_y(pointer[0] * 0.1 + (time * 0.23).sin() * 0.02)
                            * Quat::from_rotation_z(impulse * 0.08);
                    transform.scale *= squash;
                }
                Motion::DuckNeck => {
                    transform.rotation = Quat::from_rotation_y(pointer[0] * 0.65)
                        * Quat::from_rotation_x(pointer[1] * 0.32);
                }
                Motion::Blanket => {
                    transform.scale.y *= 1.0 + (time * 1.7).sin() * 0.045;
                }
            }
            let mut object = self.app.world_mut().entity_mut(*entity);
            *object.get_mut::<Transform>().unwrap() = transform;
        }
        self.app.update();
        if let Some((lost, message)) = self.app.world_mut().resource_mut::<Failure>().0.take() {
            if lost {
                gpu.report_device_lost(message.clone());
            }
            return Err(message);
        }
        check_scopes(scopes)
    }
}

fn bed_pose(pointer: [f32; 2], impulse: f32) -> Transform {
    let bounce = impulse * ((1.0 - impulse) * std::f32::consts::TAU * 1.5).cos();
    Transform::IDENTITY
        .with_rotation(
            Quat::from_rotation_y(pointer[0] * 0.28)
                * Quat::from_rotation_x(pointer[1] * 0.12)
                * Quat::from_rotation_z(bounce * 0.025),
        )
        .with_scale(Vec3::new(
            1.0 + bounce * 0.035,
            1.0 - bounce * 0.14,
            1.0 + bounce * 0.035,
        ))
}

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
