//! Portable mesh inspection views. Line-list meshes need no optional GPU features.
use crate::model::{Primitive, Scene};
use bevy::{
    asset::RenderAssetUsages,
    light::{NotShadowCaster, NotShadowReceiver},
    mesh::{Indices, PrimitiveTopology},
    prelude::*,
};

const MAX_NORMAL_VECTORS: usize = 6_000;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) enum DisplayMode {
    #[default]
    Shaded,
    Wireframe,
    WireframeOverlay,
    Normals,
}
impl DisplayMode {
    pub const NAMES: [&'static str; 4] = ["Shaded", "Wireframe", "Wireframe overlay", "Normals"];
    pub fn index(self) -> usize {
        self as usize
    }
    pub fn from_index(index: usize) -> Self {
        match index {
            1 => Self::Wireframe,
            2 => Self::WireframeOverlay,
            3 => Self::Normals,
            _ => Self::Shaded,
        }
    }
}

pub(super) struct DebugViews {
    surfaces: Vec<Entity>,
    wires: Vec<Entity>,
    normal_colors: Vec<Entity>,
    normal_vectors: Entity,
    vectors: Vec<(Vec3, Vec3)>,
    normal_length: f32,
    normalization: Transform,
}
impl DebugViews {
    pub fn new(
        world: &mut World,
        scene: &Scene,
        surfaces: &[Entity],
        normalization: Transform,
    ) -> Self {
        let mut materials = world.resource_mut::<Assets<StandardMaterial>>();
        let wire_material = materials.add(StandardMaterial {
            base_color: Color::srgb(1.0, 0.62, 0.12),
            unlit: true,
            cull_mode: None,
            ..default()
        });
        let normal_material = materials.add(StandardMaterial {
            base_color: Color::WHITE,
            unlit: true,
            cull_mode: None,
            ..default()
        });
        let vector_material = materials.add(StandardMaterial {
            base_color: Color::srgb(0.1, 0.95, 1.0),
            unlit: true,
            cull_mode: None,
            ..default()
        });
        let mut wires = Vec::with_capacity(scene.primitives.len());
        let mut normal_colors = Vec::with_capacity(scene.primitives.len());
        for primitive in &scene.primitives {
            let (wire, normals) = inspection_meshes(primitive);
            let wire = world.resource_mut::<Assets<Mesh>>().add(wire);
            let normals = world.resource_mut::<Assets<Mesh>>().add(normals);
            wires.push(
                world
                    .spawn((
                        Mesh3d(wire),
                        MeshMaterial3d(wire_material.clone()),
                        normalization,
                        Visibility::Hidden,
                        NotShadowCaster,
                        NotShadowReceiver,
                    ))
                    .id(),
            );
            normal_colors.push(
                world
                    .spawn((
                        Mesh3d(normals),
                        MeshMaterial3d(normal_material.clone()),
                        normalization,
                        Visibility::Hidden,
                        NotShadowCaster,
                        NotShadowReceiver,
                    ))
                    .id(),
            );
        }
        let vectors = sampled_vectors(scene, normalization);
        let normal_length = 0.08;
        let lines = world
            .resource_mut::<Assets<Mesh>>()
            .add(vector_mesh(&vectors, normal_length));
        let normal_vectors = world
            .spawn((
                Mesh3d(lines),
                MeshMaterial3d(vector_material),
                Transform::IDENTITY,
                Visibility::Hidden,
                NotShadowCaster,
                NotShadowReceiver,
            ))
            .id();
        Self {
            surfaces: surfaces.to_vec(),
            wires,
            normal_colors,
            normal_vectors,
            vectors,
            normal_length,
            normalization,
        }
    }

    pub fn update_view(&self, world: &mut World, toward_camera: Vec3) {
        // WebGPU forbids depth bias for line topology. Move inspection lines
        // slightly forward in normalized scene space to avoid surface fighting.
        let offset = toward_camera.normalize_or_zero() * 0.0002;
        let mut wire_transform = self.normalization;
        wire_transform.translation += offset;
        for entity in &self.wires {
            world.entity_mut(*entity).insert(wire_transform);
        }
        world
            .entity_mut(self.normal_vectors)
            .insert(Transform::from_translation(offset));
    }

    pub fn apply(
        &mut self,
        world: &mut World,
        mode: DisplayMode,
        normal_vectors: bool,
        normal_length: f32,
    ) {
        for entity in &self.surfaces {
            world.entity_mut(*entity).insert(visibility(matches!(
                mode,
                DisplayMode::Shaded | DisplayMode::WireframeOverlay
            )));
        }
        for entity in &self.wires {
            world.entity_mut(*entity).insert(visibility(matches!(
                mode,
                DisplayMode::Wireframe | DisplayMode::WireframeOverlay
            )));
        }
        for entity in &self.normal_colors {
            world
                .entity_mut(*entity)
                .insert(visibility(mode == DisplayMode::Normals));
        }
        let normal_length = if normal_length.is_finite() {
            normal_length.clamp(0.01, 0.3)
        } else {
            0.08
        };
        if normal_length != self.normal_length {
            let mesh = world
                .resource_mut::<Assets<Mesh>>()
                .add(vector_mesh(&self.vectors, normal_length));
            world.entity_mut(self.normal_vectors).insert(Mesh3d(mesh));
            self.normal_length = normal_length;
        }
        world
            .entity_mut(self.normal_vectors)
            .insert(visibility(normal_vectors));
    }
}

fn visibility(visible: bool) -> Visibility {
    if visible {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    }
}

fn inspection_meshes(primitive: &Primitive) -> (Mesh, Mesh) {
    let positions = primitive
        .vertices
        .iter()
        .map(|vertex| vertex.position)
        .collect::<Vec<_>>();
    let normals = primitive
        .vertices
        .iter()
        .map(|vertex| vertex.normal)
        .collect::<Vec<_>>();
    let mut edges = Vec::with_capacity(primitive.indices.len() * 2);
    for &[a, b, c] in primitive.indices.as_chunks::<3>().0 {
        edges.extend_from_slice(&[a, b, b, c, c, a]);
    }
    let wire = Mesh::new(PrimitiveTopology::LineList, RenderAssetUsages::RENDER_WORLD)
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions.clone())
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals.clone())
        .with_inserted_indices(Indices::U32(edges));
    let colors = normals
        .iter()
        .map(|normal| {
            let normal = Vec3::from_array(*normal).normalize_or_zero();
            let rgb = normal * 0.5 + Vec3::splat(0.5);
            [rgb.x, rgb.y, rgb.z, 1.0]
        })
        .collect::<Vec<_>>();
    let normals = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, colors)
    .with_inserted_indices(Indices::U32(primitive.indices.clone()));
    (wire, normals)
}

fn sampled_vectors(scene: &Scene, normalization: Transform) -> Vec<(Vec3, Vec3)> {
    let vertices: usize = scene.primitives.iter().map(|p| p.vertices.len()).sum();
    let stride = vertices.div_ceil(MAX_NORMAL_VECTORS).max(1);
    scene
        .primitives
        .iter()
        .flat_map(|primitive| &primitive.vertices)
        .step_by(stride)
        .take(MAX_NORMAL_VECTORS)
        .filter_map(|vertex| {
            let normal = Vec3::from_array(vertex.normal).normalize_or_zero();
            (normal != Vec3::ZERO).then(|| {
                (
                    normalization.transform_point(Vec3::from_array(vertex.position)),
                    normal,
                )
            })
        })
        .collect()
}

fn vector_mesh(vectors: &[(Vec3, Vec3)], length: f32) -> Mesh {
    let positions = vectors
        .iter()
        .flat_map(|&(position, normal)| {
            [position.to_array(), (position + normal * length).to_array()]
        })
        .collect::<Vec<_>>();
    let normals = vec![[0.0, 1.0, 0.0]; positions.len()];
    Mesh::new(PrimitiveTopology::LineList, RenderAssetUsages::RENDER_WORLD)
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::mesh::VertexAttributeValues;

    fn triangle() -> Scene {
        crate::stl::load(
            b"solid test\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid test\n",
        )
        .unwrap()
    }

    #[test]
    fn inspection_meshes_keep_triangle_edges_and_normal_axis_colors() {
        let scene = triangle();
        let (wire, normals) = inspection_meshes(&scene.primitives[0]);
        assert_eq!(wire.primitive_topology(), PrimitiveTopology::LineList);
        assert_eq!(
            wire.indices().unwrap().iter().collect::<Vec<_>>(),
            [0, 1, 1, 2, 2, 0]
        );
        let Some(VertexAttributeValues::Float32x4(colors)) =
            normals.attribute(Mesh::ATTRIBUTE_COLOR)
        else {
            panic!("normal colors are missing");
        };
        assert!(colors.iter().all(|color| *color == [0.5, 0.5, 1.0, 1.0]));
    }

    #[test]
    fn vectors_are_bounded_globally_and_lengths_use_normalized_scene_space() {
        let mut scene = triangle();
        let source = scene.primitives[0].vertices[0];
        scene.primitives[0].vertices = vec![source; MAX_NORMAL_VECTORS + 1];
        let mut second = triangle().primitives.remove(0);
        second.vertices = vec![source; MAX_NORMAL_VECTORS + 1];
        scene.primitives.push(second);
        let transform =
            Transform::from_scale(Vec3::splat(0.01)).with_translation(Vec3::new(1.0, 2.0, 3.0));
        let vectors = sampled_vectors(&scene, transform);
        assert!(!vectors.is_empty());
        assert!(vectors.len() <= MAX_NORMAL_VECTORS);
        assert_eq!(
            vectors[0].0,
            transform.transform_point(Vec3::from_array(source.position))
        );
        let mesh = vector_mesh(&vectors, 0.08);
        let Some(VertexAttributeValues::Float32x3(points)) =
            mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            panic!("normal vectors are missing");
        };
        let distance = Vec3::from_array(points[1]).distance(Vec3::from_array(points[0]));
        assert!((distance - 0.08).abs() < 0.00001);
    }
}
