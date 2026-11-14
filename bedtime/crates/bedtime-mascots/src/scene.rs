//! Native Bevy scene construction; all spawned entities descend from the owning view.
use crate::MascotKind;
use bevy::{
    asset::RenderAssetUsages,
    camera::{Exposure, RenderTarget, visibility::RenderLayers},
    core_pipeline::tonemapping::Tonemapping,
    image::{CompressedImageFormats, ImageSampler, ImageType},
    mesh::{Indices, PrimitiveTopology},
    prelude::*,
};

#[derive(Resource, Clone)]
pub(crate) struct BedModel {
    parts: Vec<(Handle<Mesh>, Handle<StandardMaterial>, bool)>,
    bounds: (Vec3, Vec3),
}

#[derive(Resource, Clone)]
struct DuckModel {
    sphere: Handle<Mesh>,
    colors: [Handle<StandardMaterial>; 4],
}

fn load_bed(world: &mut World) -> Result<BedModel, String> {
    let gltf = gltf::Gltf::from_slice(include_bytes!("../resources/bed.glb"))
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
            RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
        )
        .map_err(|error| format!("Invalid bed texture: {error}"))?;
        textures.push(world.resource_mut::<Assets<Image>>().add(image));
    }
    let mut materials = Vec::new();
    let mut cover_material = None;
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
            let mut cover_texture = world
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
            material.base_color_texture =
                Some(world.resource_mut::<Assets<Image>>().add(cover_texture));
        }
        let handle = world
            .resource_mut::<Assets<StandardMaterial>>()
            .add(material);
        if tinted {
            cover_material = Some(handle.clone());
        }
        materials.push(handle);
    }
    if cover_material.is_none() {
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
                                .transform_vector3(Vec3::new(tangent[0], tangent[1], tangent[2]))
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
    let bounds = ((min - center) * scale, (max - center) * scale);
    let mut parts = Vec::new();
    for (positions, normals, tangents, uvs, indices, material) in meshes {
        let positions: Vec<_> = positions
            .into_iter()
            .map(|position| ((position - center) * scale).to_array())
            .collect();
        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
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
        let tinted = material == *cover_material.as_ref().unwrap();
        parts.push((
            world.resource_mut::<Assets<Mesh>>().add(mesh),
            material,
            tinted,
        ));
    }
    Ok(BedModel { parts, bounds })
}

#[derive(Clone, Copy)]
enum Motion {
    Bed,
    DuckBody,
    DuckNeck,
}

#[derive(Component, Clone)]
pub(crate) struct Scene {
    pub root: Entity,
    pub camera: Entity,
    pub layers: RenderLayers,
    kind: MascotKind,
    bed_cover_material: Option<Handle<StandardMaterial>>,
    motions: Vec<(Entity, Transform, Motion)>,
    bed_bounds: Option<(Vec3, Vec3)>,
}

impl Scene {
    pub fn new(
        world: &mut World,
        owner: Entity,
        kind: MascotKind,
        image: Handle<Image>,
        layers: RenderLayers,
    ) -> Result<Self, String> {
        // Validate the bundled model before creating any owned entities.
        if kind == MascotKind::Bed && !world.contains_resource::<BedModel>() {
            let model = load_bed(world)?;
            world.insert_resource(model);
        }
        if kind == MascotKind::Duck && !world.contains_resource::<DuckModel>() {
            let sphere = world
                .resource_mut::<Assets<Mesh>>()
                .add(Sphere::new(1.0).mesh().uv(24, 16));
            let colors = [
                [1.0, 0.78, 0.055],
                [1.0, 0.38, 0.035],
                [0.015, 0.015, 0.022],
                [1.0; 3],
            ]
            .map(|color| {
                world
                    .resource_mut::<Assets<StandardMaterial>>()
                    .add(StandardMaterial {
                        base_color: Color::srgb(color[0], color[1], color[2]),
                        perceptual_roughness: 0.65,
                        ..default()
                    })
            });
            world.insert_resource(DuckModel { sphere, colors });
        }
        let root = world
            .spawn((Transform::IDENTITY, Visibility::Inherited, ChildOf(owner)))
            .id();
        let camera = world
            .spawn((
                Camera3d::default(),
                Camera {
                    order: -1,
                    is_active: false,
                    clear_color: Color::NONE.into(),
                    ..default()
                },
                RenderTarget::Image(image.into()),
                layers.clone(),
                AmbientLight {
                    brightness: 1800.0,
                    color: Color::WHITE,
                    ..default()
                },
                Msaa::Sample4,
                Tonemapping::AcesFitted,
                Exposure { ev100: 10.0 },
                Transform::IDENTITY,
                ChildOf(root),
            ))
            .id();
        world.spawn((
            DirectionalLight {
                illuminance: 2500.0,
                shadow_maps_enabled: false,
                ..default()
            },
            Transform::from_xyz(3.0, 5.0, 2.0).looking_at(Vec3::ZERO, Vec3::Y),
            layers.clone(),
            ChildOf(root),
        ));
        world.spawn((
            PointLight {
                intensity: 40_000.0,
                range: 8.0,
                radius: 1.0,
                ..default()
            },
            Transform::from_xyz(-2.0, 2.0, 3.0),
            layers.clone(),
            ChildOf(root),
        ));
        let mut scene = Self {
            root,
            camera,
            layers,
            kind,
            bed_cover_material: None,
            motions: Vec::new(),
            bed_bounds: None,
        };
        match kind {
            MascotKind::Bed => {
                let model = world.resource::<BedModel>().clone();
                scene.bed_bounds = Some(model.bounds);
                let pose = world
                    .spawn((Transform::IDENTITY, Visibility::Inherited, ChildOf(root)))
                    .id();
                scene.motions.push((pose, Transform::IDENTITY, Motion::Bed));
                for (mesh, material, tinted) in model.parts {
                    let material = if tinted {
                        if let Some(handle) = &scene.bed_cover_material {
                            handle.clone()
                        } else {
                            let value = world
                                .resource::<Assets<StandardMaterial>>()
                                .get(&material)
                                .unwrap()
                                .clone();
                            let handle =
                                world.resource_mut::<Assets<StandardMaterial>>().add(value);
                            scene.bed_cover_material = Some(handle.clone());
                            handle
                        }
                    } else {
                        material
                    };
                    world.spawn((
                        Mesh3d(mesh),
                        MeshMaterial3d(material),
                        scene.layers.clone(),
                        Transform::IDENTITY,
                        ChildOf(pose),
                    ));
                }
            }
            MascotKind::Duck => scene.duck(world),
        }
        Ok(scene)
    }
    fn ball(
        &self,
        world: &mut World,
        scale: [f32; 3],
        pos: [f32; 3],
        material: Handle<StandardMaterial>,
    ) -> Entity {
        let mesh = world.resource::<DuckModel>().sphere.clone();
        world
            .spawn((
                ChildOf(self.root),
                self.layers.clone(),
                Mesh3d(mesh),
                MeshMaterial3d(material),
                Transform::from_translation(Vec3::from_array(pos))
                    .with_scale(Vec3::from_array(scale)),
            ))
            .id()
    }

    fn duck(&mut self, world: &mut World) {
        // Two joints are enough for this rigid rubber duck: the body owns the
        // bob and squash, while the neck turns the entire face together.
        let body_pose = Transform::IDENTITY;
        let body = world
            .spawn((
                ChildOf(self.root),
                self.layers.clone(),
                body_pose,
                Visibility::Inherited,
            ))
            .id();
        self.motions.push((body, body_pose, Motion::DuckBody));
        let neck_pose = Transform::from_xyz(0.0, 0.28, 0.12);
        let neck = world
            .spawn((
                self.layers.clone(),
                neck_pose,
                Visibility::Inherited,
                ChildOf(body),
            ))
            .id();
        self.motions.push((neck, neck_pose, Motion::DuckNeck));
        let [yellow, orange, eye, shine] = world.resource::<DuckModel>().colors.clone();
        let torso = self.ball(world, [0.85, 0.65, 0.72], [0.0, -0.03, 0.0], yellow.clone());
        world.entity_mut(torso).insert(ChildOf(body));
        let head = self.ball(world, [0.53; 3], [0.0, 0.34, 0.11], yellow.clone());
        world.entity_mut(head).insert(ChildOf(neck));
        let beak = self.ball(world, [0.36, 0.105, 0.35], [0.0, 0.2, 0.6], orange);
        world.entity_mut(beak).insert(ChildOf(neck));
        for sign in [-1.0, 1.0] {
            let eye = self.ball(
                world,
                [0.11, 0.12, 0.055],
                [sign * 0.23, 0.45, 0.545],
                eye.clone(),
            );
            world.entity_mut(eye).insert(ChildOf(neck));
            let glint = self.ball(
                world,
                [0.027; 3],
                [sign * 0.23 - 0.027, 0.49, 0.59],
                shine.clone(),
            );
            world.entity_mut(glint).insert(ChildOf(neck));
            let wing = self.ball(
                world,
                [0.23, 0.35, 0.48],
                [sign * 0.66, 0.03, -0.08],
                yellow.clone(),
            );
            world.entity_mut(wing).insert(ChildOf(body));
        }
        let tail = self.ball(world, [0.28, 0.24, 0.37], [0.0, 0.12, -0.59], yellow);
        world.entity_mut(tail).insert(ChildOf(body));
    }

    pub fn sync(
        &self,
        world: &mut World,
        size: UVec2,
        pointer: [f32; 2],
        accent: [f32; 4],
        time: f32,
        impulse: f32,
    ) {
        let (eye, focus) = match self.kind {
            MascotKind::Bed => (Vec3::new(2.6, 2.5, 3.8), Vec3::ZERO),
            MascotKind::Duck => (
                Vec3::new(pointer[0] * 0.18, 0.9, 3.8),
                Vec3::new(0.0, 0.2, 0.0),
            ),
        };
        let mut camera_transform = Transform::from_translation(eye).looking_at(focus, Vec3::Y);
        if self.kind == MascotKind::Duck {
            // Fit the duck to the limiting canvas dimension, including room for
            // its turned head and click squash. A portrait sidebar needs more
            // distance than a wide panel, whose height determines the fit.
            let aspect = size.x as f32 / size.y as f32;
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
            let aspect = size.x as f32 / size.y as f32;
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
        }
        *world.get_mut::<Transform>(self.camera).unwrap() = camera_transform;
        if let Some(handle) = &self.bed_cover_material {
            let mut materials = world.resource_mut::<Assets<StandardMaterial>>();
            let mut material = materials.get_mut(handle).unwrap();
            let color = Color::srgb(accent[0], accent[1], accent[2]);
            if material.base_color != color {
                material.base_color = color;
            }
        }
        for (entity, base, motion) in &self.motions {
            let mut transform = *base;
            match motion {
                Motion::Bed => transform = bed_pose(pointer, impulse),
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
            }
            *world.get_mut::<Transform>(*entity).unwrap() = transform;
        }
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
