//! Document-owned memory assets and inspection snapshots of Bevy's glTF scenes.
#[cfg(test)]
use crate::model::Material;
use crate::model::{Primitive, Scene, Vertex};
use bevy::{
    asset::{
        AssetApp, LoadState, RecursiveDependencyLoadState, RenderAssetUsages,
        io::{
            AssetSourceBuilder,
            memory::{Dir, MemoryAssetReader},
        },
    },
    gltf::{
        Gltf, GltfLoaderSettings, GltfMaterial, GltfMesh,
        convert_coordinates::GltfConvertCoordinates,
    },
    mesh::{
        Indices, PrimitiveTopology, UvChannel, VertexAttributeValues,
        skinning::{SkinnedMesh, SkinnedMeshInverseBindposes},
    },
    prelude::*,
    render::render_resource::Face,
    world_serialization::{WorldInstance, WorldInstanceSpawner},
};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};

const SOURCE: &str = "bed-model";

// Each load captures its immutable worker-prepared meshes. Old revisions may
// finish asynchronously; path lookup keeps them separate from the next load.
type PendingMeshes = Arc<Mutex<BTreeMap<PathBuf, Arc<crate::mesh_compat::Meshes>>>>;

#[derive(Clone, Default)]
struct MeshAdapter {
    pending: PendingMeshes,
    meshes: Arc<crate::mesh_compat::Meshes>,
}

impl bevy::gltf::extensions::GltfExtensionHandler for MeshAdapter {
    fn dyn_clone(&self) -> Box<dyn bevy::gltf::extensions::ErasedGltfExtensionHandler> {
        Box::new(self.clone())
    }

    fn on_root(
        &mut self,
        context: &mut bevy::asset::LoadContext<'_>,
        _: &gltf::Gltf,
        _: &GltfLoaderSettings,
    ) {
        self.meshes = self
            .pending
            .lock()
            .unwrap()
            .get(context.path().path())
            .cloned()
            .unwrap_or_default();
    }

    async fn on_gltf_primitive(
        &mut self,
        _: &mut bevy::asset::LoadContext<'_>,
        _: &gltf::Gltf,
        mesh: &gltf::Mesh<'_>,
        primitive: &gltf::Primitive<'_>,
        _: &[Vec<u8>],
        _: &bevy::platform::collections::HashMap<Box<str>, bevy::mesh::MeshVertexAttribute>,
        _: bool,
        _: bool,
        user_mesh: &mut Option<Mesh>,
    ) {
        if let Some(mesh) = self.meshes.get(&(mesh.index(), primitive.index())) {
            *user_mesh = Some(mesh.clone());
        }
    }

    fn on_spawn_mesh_and_material(
        &mut self,
        _: &mut bevy::asset::LoadContext<'_>,
        primitive: &gltf::Primitive<'_>,
        mesh: &gltf::Mesh<'_>,
        _: &gltf::Material<'_>,
        entity: &mut EntityWorldMut<'_>,
        _: &str,
    ) {
        use bevy::camera::primitives::MeshAabb;
        // Quantized accessor min/max describe the encoded integers. Culling
        // needs the decoded positions, while the authored document stays intact.
        if let Some(bounds) = self
            .meshes
            .get(&(mesh.index(), primitive.index()))
            .and_then(|m| m.compute_aabb())
        {
            entity.insert(bounds);
        }
    }
}

pub(super) struct LoadedScene {
    pub snapshot: Scene,
    pub surfaces: Vec<Entity>,
    #[cfg(test)]
    pub root: Entity,
    pub normalization: Transform,
}

struct InspectionSnapshot {
    snapshot: Scene,
    surfaces: Vec<Entity>,
    collapsed: Vec<Entity>,
    skin_materials: Vec<SkinMaterialCorrection>,
}

struct SkinMaterialCorrection {
    entity: Entity,
    cull_mode: Option<Face>,
    invert_normal_map_y: bool,
}

pub(super) struct SceneLoading {
    memory: Dir,
    meshes: PendingMeshes,
    revision: u64,
    path: Option<PathBuf>,
    gltf: Option<Handle<Gltf>>,
    root: Option<Entity>,
    complete: bool,
}

impl SceneLoading {
    /// Register before AssetPlugin builds its asset sources.
    pub fn install(app: &mut App) -> Self {
        let memory = Dir::default();
        let reader = MemoryAssetReader {
            root: memory.clone(),
        };
        app.register_asset_source(
            SOURCE,
            AssetSourceBuilder::new(move || Box::new(reader.clone())),
        );
        let meshes = PendingMeshes::default();
        app.init_resource::<bevy::gltf::extensions::GltfExtensionHandlers>();
        app.world()
            .resource::<bevy::gltf::extensions::GltfExtensionHandlers>()
            .0
            .write_blocking()
            .push(Box::new(MeshAdapter {
                pending: Arc::clone(&meshes),
                meshes: Arc::default(),
            }));
        Self {
            memory,
            meshes,
            revision: 0,
            path: None,
            gltf: None,
            root: None,
            complete: true,
        }
    }

    pub fn start(
        &mut self,
        app: &mut App,
        bytes: Arc<[u8]>,
        meshes: Arc<crate::mesh_compat::Meshes>,
    ) {
        self.clear(app);
        self.revision += 1;
        let path = PathBuf::from(format!("revision-{}.glb", self.revision));
        self.meshes.lock().unwrap().insert(path.clone(), meshes);
        self.memory.insert_asset(&path, bytes.to_vec());
        let asset_path = format!("{SOURCE}://{}", path.display());
        self.gltf = Some(
            app.world()
                .resource::<AssetServer>()
                .load_builder()
                .with_settings(|settings: &mut GltfLoaderSettings| {
                    settings.load_meshes =
                        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD;
                    settings.load_materials =
                        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD;
                    settings.load_cameras = false;
                    settings.load_lights = false;
                    settings.load_animations = false;
                    // The document preparation boundary has already validated core
                    // glTF and the supported extension semantics.
                    settings.validate = false;
                    settings.include_source = true;
                    settings.convert_coordinates = Some(GltfConvertCoordinates::default());
                })
                .load(asset_path),
        );
        self.path = Some(path);
        self.complete = false;
    }

    pub fn pending(&self) -> bool {
        !self.complete
    }

    /// The caller pumps app.update(), including asset and transform schedules.
    pub fn poll(&mut self, app: &mut App) -> Result<Option<LoadedScene>, String> {
        if self.complete {
            return Ok(None);
        }
        let handle = self
            .gltf
            .as_ref()
            .expect("pending document has an asset handle");
        let server = app.world().resource::<AssetServer>();
        let failure = match server.get_load_states(handle.id()) {
            Some((LoadState::Failed(error), _, _)) => Some(error.to_string()),
            Some((_, _, RecursiveDependencyLoadState::Failed(error))) => Some(error.to_string()),
            _ => None,
        };
        if let Some(error) = failure {
            self.complete = true;
            return Err(format!("Could not load glTF scene: {error}"));
        }
        if !server.is_loaded_with_dependencies(handle.id()) {
            return Ok(None);
        }
        if self.root.is_none() {
            correct_uv1_tangents(app.world_mut(), handle)?;
            let gltf = app
                .world()
                .resource::<Assets<Gltf>>()
                .get(handle)
                .expect("loaded glTF asset exists");
            let scene = gltf
                .default_scene
                .as_ref()
                .or_else(|| gltf.scenes.first())
                .ok_or("glTF has no scene")?
                .clone();
            self.root = Some(
                app.world_mut()
                    .spawn((
                        WorldAssetRoot(scene),
                        Transform::IDENTITY,
                        Visibility::Inherited,
                    ))
                    .id(),
            );
            return Ok(None);
        }
        let root = self.root.unwrap();
        let Some(instance) = app.world().get::<WorldInstance>(root) else {
            return Ok(None);
        };
        let spawner = app.world().resource::<WorldInstanceSpawner>();
        if !spawner.instance_is_ready(**instance) {
            return Ok(None);
        }
        let mut entities: Vec<_> = spawner.iter_instance_entities(**instance).collect();
        entities.sort_unstable();
        let InspectionSnapshot {
            mut snapshot,
            surfaces,
            collapsed,
            skin_materials,
        } = inspection_snapshot(app.world(), &entities)?;
        let gltf = app.world().resource::<Assets<Gltf>>().get(handle).unwrap();
        snapshot.animations = gltf
            .source
            .as_ref()
            .map_or(0, |source| source.animations().len());
        snapshot.textures = gltf
            .source
            .as_ref()
            .map_or(0, |source| source.images().len());
        #[cfg(test)]
        {
            snapshot.source_bytes = self
                .path
                .as_ref()
                .and_then(|path| self.memory.get_asset(path))
                .map(|asset| Arc::from(asset.value()));
        }
        let center = Vec3::from_array(snapshot.center().to_array());
        let radius = snapshot.radius();
        let normalization =
            Transform::from_scale(Vec3::splat(radius.recip())).with_translation(-center / radius);
        for entity in collapsed {
            app.world_mut()
                .entity_mut(entity)
                .insert(Visibility::Hidden);
        }
        // The authored pose can reflect a skin independently of its mesh node.
        // Bevy chooses culling and tangent handedness from that node. Correct
        // the stock material instances, leaving meshes and GPU skinning to Bevy.
        for correction in skin_materials {
            let Some(handle) = app
                .world()
                .get::<MeshMaterial3d<StandardMaterial>>(correction.entity)
                .map(|material| material.0.clone())
            else {
                continue;
            };
            let material = app
                .world()
                .resource::<Assets<StandardMaterial>>()
                .get(&handle)
                .expect("loaded skin material exists");
            let cull_mode = if material.double_sided {
                None
            } else {
                correction.cull_mode
            };
            let flip_normal_map_y = material.flip_normal_map_y ^ correction.invert_normal_map_y;
            if material.cull_mode == cull_mode && material.flip_normal_map_y == flip_normal_map_y {
                continue;
            }
            let mut material = material.clone();
            material.cull_mode = cull_mode;
            material.flip_normal_map_y = flip_normal_map_y;
            let handle = app
                .world_mut()
                .resource_mut::<Assets<StandardMaterial>>()
                .add(material);
            app.world_mut()
                .entity_mut(correction.entity)
                .insert(MeshMaterial3d(handle));
        }
        app.world_mut().entity_mut(root).insert(normalization);
        self.complete = true;
        Ok(Some(LoadedScene {
            snapshot,
            surfaces,
            #[cfg(test)]
            root,
            normalization,
        }))
    }

    pub fn clear(&mut self, app: &mut App) {
        if let Some(root) = self.root.take() {
            app.world_mut().despawn(root);
        }
        self.gltf = None;
        if let Some(path) = self.path.take() {
            self.memory.remove_asset(&path);
            self.meshes.lock().unwrap().remove(&path);
        }
        self.complete = true;
    }
}

// Bevy generates missing tangents from UV0. Keep its material and mesh loader,
// correcting only this unsupported UV1 normal-map case before scene spawning.
fn correct_uv1_tangents(world: &mut World, handle: &Handle<Gltf>) -> Result<(), String> {
    let gltf = world.resource::<Assets<Gltf>>().get(handle).unwrap();
    let gltf_meshes = world.resource::<Assets<GltfMesh>>();
    let materials = world.resource::<Assets<GltfMaterial>>();
    let mut affected = Vec::new();
    for mesh_handle in &gltf.meshes {
        let mesh = gltf_meshes
            .get(mesh_handle)
            .ok_or("Loaded glTF mesh metadata is missing")?;
        for primitive in &mesh.primitives {
            let Some(material) = primitive
                .material
                .as_ref()
                .and_then(|handle| materials.get(handle))
            else {
                continue;
            };
            if material.normal_map_texture.is_none()
                || material.normal_map_channel != UvChannel::Uv1
            {
                continue;
            }
            let authored = gltf
                .source
                .as_ref()
                .and_then(|source| source.meshes().nth(mesh.index))
                .and_then(|mesh| mesh.primitives().nth(primitive.index))
                .is_some_and(|primitive| primitive.get(&gltf::Semantic::Tangents).is_some());
            if !authored {
                affected.push(primitive.mesh.clone());
            }
        }
    }
    let mut meshes = world.resource_mut::<Assets<Mesh>>();
    for handle in affected {
        let mut mesh = meshes
            .get_mut(&handle)
            .ok_or("Loaded glTF normal-map mesh is missing")?;
        let uv1 = mesh
            .attribute(Mesh::ATTRIBUTE_UV_1)
            .ok_or("Normal map references missing TEXCOORD_1")?
            .clone();
        let uv0 = mesh.remove_attribute(Mesh::ATTRIBUTE_UV_0);
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uv1);
        let generated = mesh.generate_tangents();
        mesh.remove_attribute(Mesh::ATTRIBUTE_UV_0);
        if let Some(uv0) = uv0 {
            mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uv0);
        }
        generated
            .map_err(|error| format!("Could not generate glTF normal-map tangents: {error}"))?;
    }
    Ok(())
}

/// CPU geometry is solely for framing and portable inspection overlays. The
/// shaded scene continues to use Bevy's original meshes, hierarchy and skin.
fn inspection_snapshot(world: &World, entities: &[Entity]) -> Result<InspectionSnapshot, String> {
    let meshes = world.resource::<Assets<Mesh>>();
    let inverse_bindposes = world.resource::<Assets<SkinnedMeshInverseBindposes>>();
    let mut minimum = glam::Vec3::splat(f32::INFINITY);
    let mut maximum = glam::Vec3::splat(f32::NEG_INFINITY);
    let mut primitives = Vec::new();
    let mut surfaces = Vec::new();
    let mut collapsed = Vec::new();
    let mut skin_materials = Vec::new();
    let mut posed_meshes = 0;
    #[cfg(test)]
    let mut images = Vec::new();
    for &entity in entities {
        let Some(mesh_handle) = world.get::<Mesh3d>(entity) else {
            continue;
        };
        let mesh = meshes
            .get(&mesh_handle.0)
            .ok_or("Loaded glTF mesh is missing from the main world")?;
        if mesh.primitive_topology() != PrimitiveTopology::TriangleList {
            return Err("The model preview supports triangle meshes".into());
        }
        let Some(VertexAttributeValues::Float32x3(values)) =
            mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            return Err("Loaded glTF mesh has no float positions".into());
        };
        let mut positions = values.clone();
        let mut normals = match mesh.attribute(Mesh::ATTRIBUTE_NORMAL) {
            Some(VertexAttributeValues::Float32x3(values)) => values.clone(),
            _ => return Err("Loaded glTF mesh has no float normals".into()),
        };
        #[cfg(test)]
        let mut tangents = match mesh.attribute(Mesh::ATTRIBUTE_TANGENT) {
            Some(VertexAttributeValues::Float32x4(values)) => Some(values.clone()),
            _ => None,
        };
        let mut indices: Vec<u32> = match mesh.indices() {
            Some(Indices::U16(values)) => values.iter().map(|&value| u32::from(value)).collect(),
            Some(Indices::U32(values)) => values.clone(),
            None => (0..positions.len() as u32).collect(),
        };
        if indices.len() % 3 != 0
            || indices
                .iter()
                .any(|&index| index as usize >= positions.len())
        {
            return Err("Loaded glTF mesh contains invalid triangle indices".into());
        }
        if let Some(skin) = world.get::<SkinnedMesh>(entity) {
            let inverse = inverse_bindposes
                .get(&skin.inverse_bindposes)
                .ok_or("Loaded skin has no inverse-bind matrices")?;
            if inverse.len() != skin.joints.len() {
                return Err("Loaded skin inverse-bind matrices do not match its joints".into());
            }
            let palette: Result<Vec<_>, String> = skin
                .joints
                .iter()
                .zip(inverse.iter())
                .map(|(&joint, &inverse)| {
                    let transform = world
                        .get::<GlobalTransform>(joint)
                        .ok_or("Loaded skin has a missing joint transform")?;
                    Ok(glam::Mat4::from_cols_array(
                        &(transform.to_matrix() * inverse).to_cols_array(),
                    ))
                })
                .collect();
            let Some(VertexAttributeValues::Uint16x4(joints)) =
                mesh.attribute(Mesh::ATTRIBUTE_JOINT_INDEX)
            else {
                return Err("Loaded skin has no joint indices".into());
            };
            let Some(VertexAttributeValues::Float32x4(weights)) =
                mesh.attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT)
            else {
                return Err("Loaded skin has no joint weights".into());
            };
            #[cfg(test)]
            let tangents_for_bake = tangents.as_deref_mut();
            #[cfg(not(test))]
            let tangents_for_bake = None;
            let palette = palette?;
            crate::skin::bake(
                &mut positions,
                Some(&mut normals),
                tangents_for_bake,
                joints,
                weights,
                &palette,
            )?;
            let cull_mode = posed_skin_culling(joints, weights, &palette);
            if cull_mode == Some(Face::Front) {
                for triangle in indices.chunks_exact_mut(3) {
                    triangle.swap(1, 2);
                }
            }
            let mesh_reflected = world
                .get::<GlobalTransform>(entity)
                .ok_or("Loaded skin has no mesh-node transform")?
                .affine()
                .matrix3
                .determinant()
                .is_sign_negative();
            let invert_normal_map_y =
                cull_mode.is_some_and(|face| (face == Face::Front) != mesh_reflected);
            skin_materials.push(SkinMaterialCorrection {
                entity,
                cull_mode,
                invert_normal_map_y,
            });
            posed_meshes += 1;
        } else {
            let matrix = glam::Mat4::from_cols_array(
                &world
                    .get::<GlobalTransform>(entity)
                    .ok_or("Loaded mesh has no world transform")?
                    .to_matrix()
                    .to_cols_array(),
            );
            let normal_matrix = glam::Mat3::from_mat4(matrix);
            let determinant = normal_matrix.determinant();
            if !matrix.is_finite() || !determinant.is_finite() {
                return Err("glTF node has a non-finite transform".into());
            }
            // Preserve the preview's treatment of zero-scale nodes: omit their
            // geometry and return their entities for the loader to hide.
            if determinant == 0.0 {
                collapsed.push(entity);
                continue;
            }
            let normal_matrix = normal_matrix.inverse().transpose();
            for (position, normal) in positions.iter_mut().zip(&mut normals) {
                *position = matrix
                    .transform_point3(glam::Vec3::from_array(*position))
                    .to_array();
                *normal = (normal_matrix * glam::Vec3::from_array(*normal))
                    .normalize_or_zero()
                    .to_array();
            }
            #[cfg(test)]
            if let Some(tangents) = &mut tangents {
                for tangent in tangents {
                    let direction = matrix
                        .transform_vector3(glam::Vec3::from_slice(&tangent[..3]))
                        .normalize_or_zero();
                    *tangent = [
                        direction.x,
                        direction.y,
                        direction.z,
                        tangent[3] * determinant.signum(),
                    ];
                }
            }
            if determinant < 0.0 {
                for triangle in indices.chunks_exact_mut(3) {
                    triangle.swap(1, 2);
                }
            }
        }
        #[cfg(test)]
        let uv = |attribute| match mesh.attribute(attribute) {
            Some(VertexAttributeValues::Float32x2(values)) => Some(values.as_slice()),
            _ => None,
        };
        #[cfg(test)]
        let uv0 = uv(Mesh::ATTRIBUTE_UV_0);
        #[cfg(test)]
        let uv1 = uv(Mesh::ATTRIBUTE_UV_1);
        #[cfg(test)]
        let colors = match mesh.attribute(Mesh::ATTRIBUTE_COLOR) {
            Some(VertexAttributeValues::Float32x4(values)) => Some(values.as_slice()),
            _ => None,
        };
        if normals.len() != positions.len() {
            return Err("Loaded glTF mesh attributes have mismatched lengths".into());
        }
        #[cfg(test)]
        if tangents
            .as_ref()
            .is_some_and(|values| values.len() != positions.len())
        {
            return Err("Loaded glTF mesh attributes have mismatched lengths".into());
        }
        let mut vertices = Vec::with_capacity(positions.len());
        for (index, position) in positions.into_iter().enumerate() {
            let position_vector = glam::Vec3::from_array(position);
            if !position_vector.is_finite() || !glam::Vec3::from_array(normals[index]).is_finite() {
                return Err("glTF geometry exceeds the preview's numeric range".into());
            }
            minimum = minimum.min(position_vector);
            maximum = maximum.max(position_vector);
            vertices.push(Vertex {
                position,
                normal: normals[index],
                #[cfg(test)]
                uv0: uv0.map_or([0.0; 2], |values| values[index]),
                #[cfg(test)]
                uv1: uv1.map_or([0.0; 2], |values| values[index]),
                #[cfg(test)]
                tangent: tangents.as_ref().map_or([0.0; 4], |values| values[index]),
                #[cfg(test)]
                color: colors.map_or([1.0; 4], |values| values[index]),
            });
        }
        #[cfg(test)]
        let mut material = inspection_material(world, entity);
        #[cfg(test)]
        populate_texture_snapshot(world, entity, &mut material, &mut images)?;
        primitives.push(Primitive {
            vertices,
            indices,
            #[cfg(test)]
            material,
            #[cfg(test)]
            has_tangents: tangents.is_some(),
        });
        surfaces.push(entity);
    }
    if primitives.is_empty() || !minimum.is_finite() || !maximum.is_finite() {
        return Err("glTF scene has no triangle geometry".into());
    }
    let extent = maximum - minimum;
    if !extent.is_finite() || extent.length() > 1e15 || !(minimum + maximum).is_finite() {
        return Err("glTF bounds are too large for a stable preview".into());
    }
    let scene = Scene {
        primitives,
        #[cfg(test)]
        images,
        minimum,
        maximum,
        nodes: entities
            .iter()
            .filter(|&&entity| world.get::<Mesh3d>(entity).is_none())
            .count()
            .saturating_sub(1),
        animations: 0,
        textures: 0,
        draco_primitives: 0,
        meshopt_views: 0,
        posed_meshes,
        #[cfg(test)]
        source_bytes: None,
    };
    Ok(InspectionSnapshot {
        snapshot: scene,
        surfaces,
        collapsed,
        skin_materials,
    })
}

fn posed_skin_culling(
    joints: &[[u16; 4]],
    weights: &[[f32; 4]],
    palette: &[glam::Mat4],
) -> Option<Face> {
    // skin::bake has established lengths, finite blends and valid influences.
    let mut orientation = None;
    for (joints, weights) in joints.iter().zip(weights) {
        let total: f32 = weights.iter().sum();
        let matrix = joints
            .iter()
            .zip(weights)
            .fold(glam::Mat4::ZERO, |matrix, (&joint, &weight)| {
                matrix + palette[joint as usize] * (weight / total)
            });
        let determinant = glam::Mat3::from_mat4(matrix).determinant();
        if determinant == 0.0 {
            return None;
        }
        let reflected = determinant < 0.0;
        if orientation.is_some_and(|orientation| orientation != reflected) {
            return None;
        }
        orientation = Some(reflected);
    }
    orientation.map(|reflected| if reflected { Face::Front } else { Face::Back })
}

#[cfg(test)]
fn populate_texture_snapshot(
    world: &World,
    entity: Entity,
    material: &mut Material,
    images: &mut Vec<crate::model::Image>,
) -> Result<(), String> {
    use crate::model::{TextureInfo, TextureSampler};
    use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
    use bevy::render::render_resource::TextureFormat;
    let Some(source) = world
        .get::<MeshMaterial3d<StandardMaterial>>(entity)
        .and_then(|handle| world.resource::<Assets<StandardMaterial>>().get(&handle.0))
    else {
        return Ok(());
    };
    let assets = world.resource::<Assets<Image>>();
    let mut texture = |handle: &Option<Handle<Image>>,
                       channel: UvChannel|
     -> Result<Option<TextureInfo>, String> {
        let Some(handle) = handle else {
            return Ok(None);
        };
        let image = assets
            .get(handle)
            .ok_or("Loaded glTF texture is missing from the main world")?;
        let size = [image.width(), image.height()];
        let rgba = if matches!(
            image.texture_descriptor.format,
            TextureFormat::Rgba8Unorm | TextureFormat::Rgba8UnormSrgb
        ) {
            image
                .data
                .as_ref()
                .ok_or("Loaded glTF texture has no CPU pixels")?
                .clone()
        } else {
            image
                .clone()
                .try_into_dynamic()
                .map_err(|error| format!("Could not inspect glTF texture pixels: {error}"))?
                .into_rgba8()
                .into_raw()
        };
        let index = images
            .iter()
            .position(|image| image.size == size && image.rgba.as_ref() == rgba.as_slice())
            .unwrap_or_else(|| {
                images.push(crate::model::Image {
                    size,
                    rgba: Arc::from(rgba.as_slice()),
                });
                images.len() - 1
            });
        let descriptor = match &image.sampler {
            ImageSampler::Descriptor(descriptor) => descriptor.clone(),
            ImageSampler::Default => ImageSamplerDescriptor::linear(),
        };
        let wrap = |mode| match mode {
            ImageAddressMode::Repeat => gltf::texture::WrappingMode::Repeat,
            ImageAddressMode::MirrorRepeat => gltf::texture::WrappingMode::MirroredRepeat,
            _ => gltf::texture::WrappingMode::ClampToEdge,
        };
        let min_filter = match (descriptor.min_filter, descriptor.mipmap_filter) {
            (ImageFilterMode::Nearest, ImageFilterMode::Nearest) => {
                gltf::texture::MinFilter::NearestMipmapNearest
            }
            (ImageFilterMode::Nearest, ImageFilterMode::Linear) => {
                gltf::texture::MinFilter::NearestMipmapLinear
            }
            (ImageFilterMode::Linear, ImageFilterMode::Nearest) => {
                gltf::texture::MinFilter::LinearMipmapNearest
            }
            (ImageFilterMode::Linear, ImageFilterMode::Linear) => {
                gltf::texture::MinFilter::LinearMipmapLinear
            }
        };
        Ok(Some(TextureInfo {
            image: index,
            tex_coord: if channel == UvChannel::Uv1 { 1 } else { 0 },
            sampler: TextureSampler {
                wrap: [
                    wrap(descriptor.address_mode_u),
                    wrap(descriptor.address_mode_v),
                ],
                mag_filter: Some(match descriptor.mag_filter {
                    ImageFilterMode::Nearest => gltf::texture::MagFilter::Nearest,
                    ImageFilterMode::Linear => gltf::texture::MagFilter::Linear,
                }),
                min_filter: Some(min_filter),
            },
        }))
    };
    material.texture = texture(
        &source.base_color_texture,
        source.base_color_channel.clone(),
    )?;
    material.metallic_roughness_texture = texture(
        &source.metallic_roughness_texture,
        source.metallic_roughness_channel.clone(),
    )?;
    material.normal_texture = texture(
        &source.normal_map_texture,
        source.normal_map_channel.clone(),
    )?;
    material.occlusion_texture =
        texture(&source.occlusion_texture, source.occlusion_channel.clone())?;
    material.emissive_texture = texture(&source.emissive_texture, source.emissive_channel.clone())?;
    Ok(())
}

#[cfg(test)]
fn inspection_material(world: &World, entity: Entity) -> Material {
    let material = world
        .get::<MeshMaterial3d<StandardMaterial>>(entity)
        .and_then(|handle| world.resource::<Assets<StandardMaterial>>().get(&handle.0));
    let color = material.map_or(LinearRgba::WHITE, |material| {
        material.base_color.to_linear()
    });
    let emissive = material.map_or(LinearRgba::BLACK, |material| material.emissive);
    Material {
        color: color.to_f32_array(),
        texture: None,
        metallic: material.map_or(0.0, |material| material.metallic),
        roughness: material.map_or(0.5, |material| material.perceptual_roughness),
        metallic_roughness_texture: None,
        normal_texture: None,
        normal_scale: 1.0,
        occlusion_texture: None,
        occlusion_strength: 1.0,
        emissive: [emissive.red, emissive.green, emissive.blue],
        emissive_texture: None,
        alpha: match material.map(|material| material.alpha_mode) {
            Some(AlphaMode::Mask(_)) => gltf::material::AlphaMode::Mask,
            Some(AlphaMode::Blend) => gltf::material::AlphaMode::Blend,
            _ => gltf::material::AlphaMode::Opaque,
        },
        cutoff: match material.map(|material| material.alpha_mode) {
            Some(AlphaMode::Mask(cutoff)) => cutoff,
            _ => 0.5,
        },
        double_sided: material.is_some_and(|material| material.double_sided),
        unlit: material.is_some_and(|material| material.unlit),
    }
}

#[cfg(test)]
fn headless_app() -> (App, SceneLoading) {
    let mut app = App::new();
    let loading = SceneLoading::install(&mut app);
    app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default()));
    app.init_asset::<bevy::shader::Shader>()
        .init_asset_loader::<bevy::shader::ShaderLoader>();
    app.add_plugins((
        bevy::render::sync_world::SyncWorldPlugin,
        bevy::transform::TransformPlugin,
        bevy::world_serialization::WorldSerializationPlugin,
        bevy::image::ImagePlugin::default(),
        bevy::mesh::MeshPlugin,
        bevy::camera::CameraPlugin,
        bevy::light::LightPlugin,
        bevy::gltf::GltfPlugin::default(),
        bevy::pbr::PbrPlugin::default(),
    ));
    app.finish();
    app.cleanup();
    (app, loading)
}

#[cfg(test)]
pub(super) fn load_snapshot(bytes: &[u8]) -> Result<Scene, String> {
    load_snapshot_with_meshes(bytes, Arc::default())
}

#[cfg(test)]
pub(super) fn load_prepared_snapshot(
    source: &crate::prepare::PreparedGltf,
) -> Result<Scene, String> {
    load_snapshot_with_meshes(&source.bytes, Arc::clone(&source.meshes))
}

#[cfg(test)]
fn load_snapshot_with_meshes(
    bytes: &[u8],
    meshes: Arc<crate::mesh_compat::Meshes>,
) -> Result<Scene, String> {
    let (mut app, mut loading) = headless_app();
    loading.start(&mut app, Arc::from(bytes), meshes);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        app.update();
        if let Some(scene) = loading.poll(&mut app)? {
            return Ok(scene.snapshot);
        }
        if std::time::Instant::now() >= deadline {
            return Err("Timed out loading the Bevy glTF scene".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bevy_memory_scene_keeps_authored_coordinates_and_geometry() {
        let scene = load_snapshot(include_bytes!("../../../tests/fixtures/gltf/cube.glb")).unwrap();
        assert_eq!(scene.triangles(), 12);
        assert_eq!(scene.minimum, glam::Vec3::splat(-1.0));
        assert_eq!(scene.maximum, glam::Vec3::splat(1.0));
    }

    #[test]
    fn invalid_memory_scene_returns_the_asset_failure() {
        let error = load_snapshot(b"invalid GLB").err().unwrap();
        assert!(error.starts_with("Could not load glTF scene:"), "{error}");
    }

    #[test]
    fn default_scene_and_first_scene_fallback_are_instantiated() {
        let mut source: serde_json::Value =
            serde_json::from_slice(include_bytes!("../../../tests/fixtures/gltf/cube.gltf"))
                .unwrap();
        source["nodes"] = serde_json::json!([{"mesh":0},{"mesh":0,"translation":[4.0,0.0,0.0]}]);
        source["scenes"] = serde_json::json!([{"nodes":[0]},{"nodes":[1]}]);
        source["scene"] = serde_json::json!(1);
        let scene = load_snapshot(&serde_json::to_vec(&source).unwrap()).unwrap();
        assert_eq!(scene.minimum, glam::Vec3::new(3.0, -1.0, -1.0));
        assert_eq!(scene.maximum, glam::Vec3::new(5.0, 1.0, 1.0));
        source.as_object_mut().unwrap().remove("scene");
        let scene = load_snapshot(&serde_json::to_vec(&source).unwrap()).unwrap();
        assert_eq!(scene.minimum, glam::Vec3::splat(-1.0));
        assert_eq!(scene.maximum, glam::Vec3::splat(1.0));
    }

    #[test]
    fn collapsed_nodes_do_not_change_visible_scene_framing() {
        let mut source: serde_json::Value =
            serde_json::from_slice(include_bytes!("../../../tests/fixtures/gltf/cube.gltf"))
                .unwrap();
        source["nodes"] = serde_json::json!([
            {"mesh":0,"scale":[0.0,1.0,1.0],"translation":[1000.0,0.0,0.0]},
            {"mesh":0}
        ]);
        source["scenes"] = serde_json::json!([{"nodes":[0,1]}]);
        let scene = load_snapshot(&serde_json::to_vec(&source).unwrap()).unwrap();
        assert_eq!(scene.primitives.len(), 1);
        assert_eq!(scene.minimum, glam::Vec3::splat(-1.0));
        assert_eq!(scene.maximum, glam::Vec3::splat(1.0));
    }

    #[test]
    fn replacing_the_memory_document_removes_its_scene_and_source() {
        let (mut app, mut loading) = headless_app();
        loading.start(
            &mut app,
            Arc::from(include_bytes!("../../../tests/fixtures/gltf/cube.glb").as_slice()),
            Arc::default(),
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let loaded = loop {
            app.update();
            if let Some(loaded) = loading.poll(&mut app).unwrap() {
                break loaded;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(1));
        };
        let previous_path = loading.path.clone().unwrap();
        loading.start(
            &mut app,
            Arc::from(b"invalid GLB".as_slice()),
            Arc::default(),
        );
        assert!(app.world().get_entity(loaded.root).is_err());
        assert!(
            loaded
                .surfaces
                .iter()
                .all(|&entity| app.world().get_entity(entity).is_err())
        );
        assert!(loading.memory.get_asset(&previous_path).is_none());
        assert!(loading.pending());
    }
}
