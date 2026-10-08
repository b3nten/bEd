use std::{borrow::Cow, io::Cursor};
use thiserror::Error;

use bevy::{
    asset::{io::Reader, AssetLoader, LoadContext, RenderAssetUsages},
    mesh::{Indices, Mesh, VertexAttributeValues},
    prelude::*,
    reflect::TypePath,
    render::render_resource::PrimitiveTopology,
};

#[derive(Default)]
pub struct StlPlugin;

impl Plugin for StlPlugin {
    fn build(&self, app: &mut App) {
        app.init_asset_loader::<StlLoader>();
    }
}

#[derive(Default, TypePath)]
pub struct StlLoader;

impl StlLoader {
    /// Loads an STL mesh from an in-memory document snapshot.
    ///
    /// This uses the same parser and triangle conversion as the asset loader,
    /// without requiring an asset server or a local filesystem path.
    pub fn load_bytes(bytes: &[u8]) -> Result<Mesh, StlError> {
        let stl = parse_stl(bytes)?;
        Ok(stl_to_triangle_mesh(&stl))
    }
}

impl AssetLoader for StlLoader {
    type Asset = Mesh;
    type Settings = ();
    type Error = StlError;
    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &(),
        #[allow(unused_variables)] load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        let stl = parse_stl(&bytes)?;

        #[cfg(feature = "wireframe")]
        load_context
            .labeled_asset_scope::<_, ()>("wireframe".to_string(), |_load_context| {
                let mesh = stl_to_wireframe_mesh(&stl);
                Ok(mesh)
            })
            .unwrap();

        Ok(stl_to_triangle_mesh(&stl))
    }

    fn extensions(&self) -> &[&str] {
        static EXTENSIONS: &[&str] = &["stl"];
        EXTENSIONS
    }
}

#[derive(Error, Debug)]
pub enum StlError {
    #[error("Failed to load STL")]
    Io(#[from] std::io::Error),
}

fn parse_stl(bytes: &[u8]) -> Result<stl_io::IndexedMesh, StlError> {
    let binary = bytes.get(80..84).is_some_and(|count| {
        let count = u32::from_le_bytes(count.try_into().unwrap()) as usize;
        count.checked_mul(50).and_then(|size| size.checked_add(84)) == Some(bytes.len())
    });
    // stl_io probes only the first line for "solid ". A binary STL's arbitrary
    // header can also contain that line, so prefer its exact count/length and
    // mask the unused header to select the original binary parser reliably.
    let bytes = if binary && bytes.starts_with(b"solid ") {
        let mut masked = bytes.to_vec();
        masked[0] = b'_';
        Cow::Owned(masked)
    } else {
        Cow::Borrowed(bytes)
    };
    Ok(stl_io::read_stl(&mut Cursor::new(bytes))?)
}

fn stl_to_triangle_mesh(stl: &stl_io::IndexedMesh) -> Mesh {
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );

    let vertex_count = stl.faces.len() * 3;

    let mut positions = Vec::with_capacity(vertex_count);
    let mut normals = Vec::with_capacity(vertex_count);
    let mut indices = Vec::with_capacity(vertex_count);

    for (i, face) in stl.faces.iter().enumerate() {
        for j in 0..3 {
            let vertex = stl.vertices[face.vertices[j]];
            positions.push([vertex[0], vertex[1], vertex[2]]);
            normals.push([face.normal[0], face.normal[1], face.normal[2]]);
            indices.push((i * 3 + j) as u32);
        }
    }

    let uvs = vec![[0.0, 0.0]; vertex_count];

    mesh.insert_attribute(
        Mesh::ATTRIBUTE_POSITION,
        VertexAttributeValues::Float32x3(positions),
    );
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_NORMAL,
        VertexAttributeValues::Float32x3(normals),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, VertexAttributeValues::Float32x2(uvs));
    mesh.insert_indices(Indices::U32(indices));

    mesh
}

#[cfg(feature = "wireframe")]
fn stl_to_wireframe_mesh(stl: &stl_io::IndexedMesh) -> Mesh {
    let mut mesh = Mesh::new(PrimitiveTopology::LineList, RenderAssetUsages::default());

    let positions = stl.vertices.iter().map(|v| [v[0], v[1], v[2]]).collect();
    let mut indices = Vec::with_capacity(stl.faces.len() * 3);
    let normals = vec![[1.0, 0.0, 0.0]; stl.vertices.len()];
    let uvs = vec![[0.0, 0.0]; stl.vertices.len()];
    for face in &stl.faces {
        for j in 0..3 {
            indices.push(face.vertices[j] as u32);
            indices.push(face.vertices[(j + 1) % 3] as u32);
        }
    }

    mesh.insert_attribute(
        Mesh::ATTRIBUTE_POSITION,
        VertexAttributeValues::Float32x3(positions),
    );
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_NORMAL,
        VertexAttributeValues::Float32x3(normals),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, VertexAttributeValues::Float32x2(uvs));
    mesh.insert_indices(Indices::U32(indices));

    mesh
}
