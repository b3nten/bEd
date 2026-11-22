//! The cat's model (`assets/cat.glb`, built into the crate): its skinned mesh,
//! its skeleton, and its moves.
//!
//! The model is made in Blender (`assets/cat/cat.blend`, exported by
//! `tools/cat/export.py`); each of its actions is an animation here, found
//! by name.
//!
//! The skeleton is posed here, not by Bevy's hierarchy: a `Pose` holds every
//! node's local transform, `Model::globals` multiplies them down the tree, and
//! a `Rig`'s joint entities are set to the results. So what poses the cat
//! also knows where its paws are.
use bevy::{
    asset::RenderAssetUsages,
    camera::visibility::{NoFrustumCulling, RenderLayers},
    image::{
        CompressedImageFormats, ImageAddressMode, ImageFilterMode, ImageSampler,
        ImageSamplerDescriptor, ImageType,
    },
    mesh::{
        Indices, PrimitiveTopology,
        skinning::{SkinnedMesh, SkinnedMeshInverseBindposes},
    },
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};

const GLB: &[u8] = include_bytes!("../assets/cat.glb");

/// The widest a texture is kept: the cat is drawn small.
const TEXTURE_SIZE: u32 = 512;

pub struct Node {
    pub name: String,
    pub parent: Option<usize>,
    pub rest: Transform,
}

/// Every node's local transform.
#[derive(Clone)]
pub struct Pose(pub Vec<Transform>);

impl Pose {
    /// This pose, `amount` of the way to `other`.
    pub fn blend(&mut self, other: &Pose, amount: f32) {
        if amount <= 0.0 {
            return;
        }
        for (mine, theirs) in self.0.iter_mut().zip(&other.0) {
            mine.translation = mine.translation.lerp(theirs.translation, amount);
            mine.rotation = mine.rotation.slerp(theirs.rotation, amount);
            mine.scale = mine.scale.lerp(theirs.scale, amount);
        }
    }
}

enum Values {
    Translation(Vec<Vec3>),
    Rotation(Vec<Quat>),
    Scale(Vec<Vec3>),
}

struct Track {
    node: usize,
    times: Vec<f32>,
    values: Values,
}

/// An animation from the file.
#[derive(Default)]
pub struct Clip {
    tracks: Vec<Track>,
    /// Seconds. A clip that loops ends as it began.
    pub length: f32,
}

impl Clip {
    fn sample(&self, time: f32, pose: &mut Pose) {
        for track in &self.tracks {
            let next = track.times.partition_point(|&at| at <= time);
            let (a, b) = (next.saturating_sub(1), next.min(track.times.len() - 1));
            let span = track.times[b] - track.times[a];
            let t = if span > 0.0 {
                ((time - track.times[a]) / span).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let local = &mut pose.0[track.node];
            match &track.values {
                Values::Translation(v) => local.translation = v[a].lerp(v[b], t),
                Values::Rotation(v) => local.rotation = v[a].slerp(v[b], t),
                Values::Scale(v) => local.scale = v[a].lerp(v[b], t),
            }
        }
    }
}

/// A struct of things found in the file by name.
macro_rules! named {
    ($(#[$doc:meta])* $all:ident of $one:ty { $($field:ident: $name:literal,)* }) => {
        $(#[$doc])*
        pub struct $all {
            $(pub $field: $one,)*
        }

        impl $all {
            fn find(mut find: impl FnMut(&str) -> Option<$one>) -> Result<Self, String> {
                Ok(Self {
                    $($field: find($name).ok_or(concat!("The cat has no ", $name))?,)*
                })
            }
        }
    };
}

named! {
    /// The joints the code moves itself, as node indices.
    Bones of usize {
        body: "body",
        hips: "hips",
        spine2: "spine2",
        spine3: "spine3",
        chest: "chest",
        neck1: "neck1",
        neck2: "neck2",
        head: "head",
        jaw: "jaw",
        l_upper_lid: "lid_upper.L",
        l_lower_lid: "lid_lower.L",
        r_upper_lid: "lid_upper.R",
        r_lower_lid: "lid_lower.R",
        l_ear: "ear.L",
        r_ear: "ear.R",
        l_upper_arm: "upper_arm.L",
        l_forearm: "forearm.L",
        l_hand: "hand.L",
        l_fingers: "fingers.L",
        r_upper_arm: "upper_arm.R",
        r_forearm: "forearm.R",
        r_hand: "hand.R",
        r_fingers: "fingers.R",
        l_toes: "toes.L",
        tail1: "tail1",
        tail2: "tail2",
        tail3: "tail3",
        tail4: "tail4",
        tail5: "tail5",
        tail6: "tail6",
    }
}

impl Bones {
    pub fn tail(&self) -> [usize; 6] {
        [
            self.tail1, self.tail2, self.tail3, self.tail4, self.tail5, self.tail6,
        ]
    }
}

named! {
    /// The cat's moves: the actions of `cat.blend`. All are drawn with the
    /// cat's right side to the viewer (`Model::mirrored` turns them round).
    Clips of Clip {
        stand: "stand",
        walk: "walk",
        trot: "trot",
        sit: "sit",
        groom: "groom",
        loaf: "loaf",
        sleep: "sleep",
        stretch: "stretch",
        crouch: "crouch",
        leap: "leap",
        dangle: "dangle",
    }
}

/// The whiskers' shade and how opaque they are: see-through, so they don't
/// stand out against the fur.
const WHISKER: Color = Color::srgba(0.55, 0.55, 0.55, 0.36);

/// The fur's texture, as it came.
struct Coat {
    /// RGBA.
    texels: Vec<u8>,
    size: UVec2,
}

pub struct Model {
    pub nodes: Vec<Node>,
    /// Node indices, parents before their children.
    order: Vec<usize>,
    /// The skin's joints, as node indices.
    joints: Vec<usize>,
    inverse_binds: Handle<SkinnedMeshInverseBindposes>,
    parts: Vec<(Handle<Mesh>, Handle<StandardMaterial>)>,
    /// The fur's material, and the texture it is painted in.
    fur: Handle<StandardMaterial>,
    coat: Coat,
    /// The lightness of the coat's darkest and lightest fur.
    fur_range: (f32, f32),
    pub bones: Bones,
    pub clips: Clips,
    /// The pose the mesh was modelled in: standing, tail out.
    pub bind: Pose,
    /// Some of the fur's vertices, to find where a pose puts the cat's
    /// outline: each one's position, joints (into `joints`) and weights.
    /// The last is whether the vertex is the tail's.
    outline: Vec<(Vec3, [u16; 4], [f32; 4], bool)>,
    inverse_bind_matrices: Vec<Mat4>,
    /// For each node, what makes its place in a mirrored pose from the
    /// place of the node across the cat from it (`mirrored`).
    across: Vec<Option<(usize, Mat4)>>,
}

/// Left to right, in the model's space: the cat's left is at -z.
const MIRROR: Mat4 = Mat4::from_cols_array(&[
    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
]);

impl Model {
    /// A move at `time` seconds into it.
    pub fn pose(&self, clip: &Clip, time: f32) -> Pose {
        let mut pose = self.bind.clone();
        clip.sample(time.clamp(0.0, clip.length), &mut pose);
        pose
    }

    /// A move that goes round, at `time`.
    pub fn cycle(&self, clip: &Clip, time: f32) -> Pose {
        self.pose(clip, time.rem_euclid(clip.length.max(0.001)))
    }

    /// A pose as a mirror would show it: left for right. The moves are
    /// drawn for a cat seen from its right; this is the same cat seen from
    /// its left, with what was on the far side still on the far side.
    pub fn mirrored(&self, pose: &Pose) -> Pose {
        let globals = self.globals(pose);
        let mut turned = globals.clone();
        for (node, across) in self.across.iter().enumerate() {
            if let Some((other, fit)) = across {
                turned[node] = MIRROR * globals[*other] * *fit;
            }
        }
        Pose(
            (0..self.nodes.len())
                .map(|node| match self.nodes[node].parent {
                    Some(parent) => Transform::from_matrix(turned[parent].inverse() * turned[node]),
                    None => Transform::from_matrix(turned[node]),
                })
                .collect(),
        )
    }

    /// How fast the ground goes by under a gait, in the model's units a
    /// second: how fast a planted hind paw is carried back.
    pub fn ground_speed(&self, gait: &Clip) -> f32 {
        const STEPS: usize = 96;
        let paw = self.bones.l_toes;
        let step = gait.length / STEPS as f32;
        let at: Vec<Vec3> = (0..=STEPS)
            .map(|index| {
                let pose = self.pose(gait, index as f32 * step);
                self.global(&pose, paw).w_axis.truncate()
            })
            .collect();
        let ground = at.iter().map(|at| at.y).fold(f32::INFINITY, f32::min) + 0.2;
        let (mut total, mut count) = (0.0, 0);
        for pair in at.windows(2) {
            if pair[0].y < ground && pair[1].y < ground {
                total += (pair[0].x - pair[1].x) / step;
                count += 1;
            }
        }
        if count == 0 {
            0.0
        } else {
            total / count as f32
        }
    }

    /// Where a pose puts every node, in the model's space.
    pub fn globals(&self, pose: &Pose) -> Vec<Mat4> {
        let mut globals = vec![Mat4::IDENTITY; self.nodes.len()];
        for &node in &self.order {
            let local = pose.0[node].to_matrix();
            globals[node] = match self.nodes[node].parent {
                Some(parent) => globals[parent] * local,
                None => local,
            };
        }
        globals
    }

    /// Where a pose puts one node.
    pub fn global(&self, pose: &Pose, node: usize) -> Mat4 {
        let mut global = pose.0[node].to_matrix();
        let mut above = self.nodes[node].parent;
        while let Some(node) = above {
            global = pose.0[node].to_matrix() * global;
            above = self.nodes[node].parent;
        }
        global
    }

    /// The rotation a pose gives a node's parent.
    pub fn parent_rotation(&self, pose: &Pose, node: usize) -> Quat {
        let mut rotation = Quat::IDENTITY;
        let mut above = self.nodes[node].parent;
        while let Some(node) = above {
            rotation = pose.0[node].rotation * rotation;
            above = self.nodes[node].parent;
        }
        rotation
    }

    /// How a pose has turned a node from the way it was modelled.
    pub fn turned(&self, pose: &Pose, node: usize) -> Quat {
        let now = self.parent_rotation(pose, node) * pose.0[node].rotation;
        let bind = self.parent_rotation(&self.bind, node) * self.bind.0[node].rotation;
        now * bind.inverse()
    }

    /// The boxes around the cat's outline and around all of it but the
    /// tail, for these `globals` turned by `turn`.
    pub fn extent(&self, globals: &[Mat4], turn: Quat) -> [(Vec3, Vec3); 2] {
        let turn = Mat4::from_quat(turn);
        let skin: Vec<Mat4> = self
            .joints
            .iter()
            .zip(&self.inverse_bind_matrices)
            .map(|(&node, inverse)| turn * globals[node] * *inverse)
            .collect();
        let mut boxes = [(Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)); 2];
        for (position, joints, weights, tail) in &self.outline {
            let mut at = Vec3::ZERO;
            for (&joint, &weight) in joints.iter().zip(weights) {
                if weight > 0.0 {
                    at += skin[joint as usize].transform_point3(*position) * weight;
                }
            }
            for (min, max) in &mut boxes[..if *tail { 1 } else { 2 }] {
                *min = min.min(at);
                *max = max.max(at);
            }
        }
        boxes
    }

    /// Colour the fur: its texture is painted again in `color` (`repaint`),
    /// the tabby's dark and pale fur becoming darker and lighter shades of
    /// it. `None` is the cat's own grey.
    pub fn tint(&self, world: &mut World, color: Option<[u8; 3]>) {
        let mut texels = self.coat.texels.clone();
        if let Some(color) = color {
            repaint(&mut texels, color, self.fur_range);
        }
        // A texture of its own each time: one changed in place keeps the
        // smaller sizes of the last colour, which are what a small cat is
        // drawn from.
        let texture = world
            .resource_mut::<Assets<Image>>()
            .add(mipped(texels, self.coat.size));
        // The lights are set for dark fur: a colour is toned down under them.
        let tone = if color.is_some() { 0.68 } else { 1.0 };
        if let Some(mut fur) = world
            .resource_mut::<Assets<StandardMaterial>>()
            .get_mut(&self.fur)
        {
            fur.base_color_texture = Some(texture);
            fur.base_color = Color::linear_rgb(tone, tone, tone);
        }
    }

    pub fn load(world: &mut World) -> Result<Self, String> {
        let gltf =
            gltf::Gltf::from_slice(GLB).map_err(|error| format!("Invalid cat model: {error}"))?;
        let blob = gltf.blob.as_deref().ok_or("The cat model has no data")?;
        let buffer = |_: gltf::Buffer| Some(blob);

        let mut nodes: Vec<Node> = gltf
            .nodes()
            .map(|node| {
                let (translation, rotation, scale) = node.transform().decomposed();
                Node {
                    name: node.name().unwrap_or_default().to_owned(),
                    parent: None,
                    rest: Transform {
                        translation: translation.into(),
                        rotation: Quat::from_array(rotation),
                        scale: scale.into(),
                    },
                }
            })
            .collect();
        let mut order = Vec::new();
        let scene = gltf.default_scene().ok_or("The cat model has no scene")?;
        let mut pending: Vec<_> = scene.nodes().collect();
        while let Some(node) = pending.pop() {
            order.push(node.index());
            for child in node.children() {
                nodes[child.index()].parent = Some(node.index());
                pending.push(child);
            }
        }
        let named = |name: &str| nodes.iter().position(|node| node.name == name);
        let bones = Bones::find(named)?;

        let skin = gltf.skins().next().ok_or("The cat model has no skeleton")?;
        let joints: Vec<_> = skin.joints().map(|joint| joint.index()).collect();
        let inverse_bind_matrices: Vec<Mat4> = skin
            .reader(buffer)
            .read_inverse_bind_matrices()
            .ok_or("The cat's skeleton has no bind pose")?
            .map(|matrix| Mat4::from_cols_array_2d(&matrix))
            .collect();
        // The file's nodes rest as the mesh was bound to them.
        let bind = Pose(nodes.iter().map(|node| node.rest).collect());
        // A joint's place in a mirrored pose: the mirror image of where its
        // opposite number puts the skin, as this joint would put it there.
        let mut across = vec![None; nodes.len()];
        for (&joint, inverse) in joints.iter().zip(&inverse_bind_matrices) {
            let name = &nodes[joint].name;
            let other = match name.rsplit_once('.') {
                Some((stem, "L")) => named(&format!("{stem}.R")),
                Some((stem, "R")) => named(&format!("{stem}.L")),
                _ => Some(joint),
            }
            .ok_or_else(|| format!("The cat's {name} has no opposite"))?;
            let theirs = joints
                .iter()
                .position(|&joint| joint == other)
                .ok_or_else(|| format!("The cat's {name} has no opposite in its skin"))?;
            across[joint] = Some((
                other,
                inverse_bind_matrices[theirs] * MIRROR * inverse.inverse(),
            ));
        }
        let inverse_binds = world
            .resource_mut::<Assets<SkinnedMeshInverseBindposes>>()
            .add(SkinnedMeshInverseBindposes::from(
                inverse_bind_matrices.clone(),
            ));

        let clips = Clips::find(|name| {
            let animation = gltf
                .animations()
                .find(|animation| animation.name() == Some(name))?;
            let mut clip = Clip::default();
            for channel in animation.channels() {
                let reader = channel.reader(buffer);
                let times: Vec<f32> = reader.read_inputs()?.collect();
                use gltf::animation::util::ReadOutputs;
                let values = match reader.read_outputs()? {
                    ReadOutputs::Translations(v) => {
                        Values::Translation(v.map(Vec3::from).collect())
                    }
                    ReadOutputs::Rotations(v) => {
                        Values::Rotation(v.into_f32().map(Quat::from_array).collect())
                    }
                    ReadOutputs::Scales(v) => Values::Scale(v.map(Vec3::from).collect()),
                    ReadOutputs::MorphTargetWeights(_) => continue,
                };
                if times.is_empty() {
                    continue;
                }
                clip.length = clip.length.max(*times.last().unwrap());
                clip.tracks.push(Track {
                    node: channel.target().node().index(),
                    times,
                    values,
                });
            }
            Some(clip)
        })?;

        // The textures are in the file, each a PNG.
        let picture = |material: &gltf::Material| -> Result<(Vec<u8>, UVec2), String> {
            let texture = material
                .pbr_metallic_roughness()
                .base_color_texture()
                .ok_or("The cat has a material with no texture")?;
            let gltf::image::Source::View { view, .. } = texture.texture().source().source() else {
                return Err("The cat's textures must be in its file".into());
            };
            decode(&blob[view.offset()..view.offset() + view.length()])
        };
        let mut parts = Vec::new();
        let mut coat = None;
        let mut fur = None;
        let mut outline = Vec::new();
        for mesh in gltf.meshes() {
            for primitive in mesh.primitives() {
                let source = primitive.material();
                let (texels, size) = picture(&source)?;
                let texture = world
                    .resource_mut::<Assets<Image>>()
                    .add(mipped(texels.clone(), size));
                // The eyes and whiskers keep their colour whatever the fur's.
                let roughness = match source.name() {
                    Some("eyes") => Some(0.25),
                    Some("whiskers") => Some(0.4),
                    _ => None,
                };
                // Whiskers are fine hairs: seen through, so they don't stand
                // out against the fur.
                let whisker = source.name() == Some("whiskers");
                let material =
                    world
                        .resource_mut::<Assets<StandardMaterial>>()
                        .add(StandardMaterial {
                            base_color: if whisker { WHISKER } else { Color::WHITE },
                            base_color_texture: Some(texture),
                            alpha_mode: if whisker {
                                AlphaMode::Blend
                            } else {
                                AlphaMode::Opaque
                            },
                            perceptual_roughness: roughness.unwrap_or(0.85),
                            reflectance: 0.2,
                            ..default()
                        });
                let reader = primitive.reader(buffer);
                let positions: Vec<_> = reader
                    .read_positions()
                    .ok_or("The cat has no positions")?
                    .collect();
                let normals: Vec<_> = reader
                    .read_normals()
                    .ok_or("The cat has no normals")?
                    .collect();
                let uvs: Vec<_> = reader
                    .read_tex_coords(0)
                    .ok_or("The cat has no texture coordinates")?
                    .into_f32()
                    .collect();
                let joint_indices: Vec<_> = reader
                    .read_joints(0)
                    .ok_or("The cat has no joints")?
                    .into_u16()
                    .collect();
                let weights: Vec<_> = reader
                    .read_weights(0)
                    .ok_or("The cat has no weights")?
                    .into_f32()
                    .collect();
                if roughness.is_none() {
                    outline = positions
                        .iter()
                        .zip(&joint_indices)
                        .zip(&weights)
                        .step_by(2)
                        .map(|((position, joints_of), weights)| {
                            let most = (0..4)
                                .max_by(|&a, &b| weights[a].total_cmp(&weights[b]))
                                .unwrap();
                            let tail = bones.tail().contains(&node_of(&joints, joints_of[most]));
                            (Vec3::from(*position), *joints_of, *weights, tail)
                        })
                        .collect();
                    coat = Some(Coat { texels, size });
                    fur = Some(material.clone());
                }
                let indices: Vec<_> = reader
                    .read_indices()
                    .ok_or("The cat has no indices")?
                    .into_u32()
                    .collect();
                let mesh = Mesh::new(
                    PrimitiveTopology::TriangleList,
                    RenderAssetUsages::RENDER_WORLD,
                )
                .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
                .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
                .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
                .with_inserted_attribute(
                    Mesh::ATTRIBUTE_JOINT_INDEX,
                    bevy::mesh::VertexAttributeValues::Uint16x4(joint_indices),
                )
                .with_inserted_attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT, weights)
                .with_inserted_indices(Indices::U32(indices));
                parts.push((world.resource_mut::<Assets<Mesh>>().add(mesh), material));
            }
        }
        let (coat, fur) = coat.zip(fur).ok_or("The cat has no fur")?;
        Ok(Self {
            nodes,
            order,
            joints,
            inverse_binds,
            parts,
            fur,
            fur_range: range(&coat.texels),
            coat,
            bones,
            clips,
            bind,
            outline,
            inverse_bind_matrices,
            across,
        })
    }
}

fn node_of(joints: &[usize], joint: u16) -> usize {
    joints.get(joint as usize).copied().unwrap_or(usize::MAX)
}

/// Paint a texture's fur in `color`: its darkest fur to lightest (`range`)
/// becoming a darker to a lighter shade of it. What has a colour of its own
/// (pink skin) is kept.
fn repaint(texels: &mut [u8], [r, g, b]: [u8; 3], (low, high): (f32, f32)) {
    let color = Color::srgb_u8(r, g, b).to_linear();
    let color = [color.red, color.green, color.blue];
    // Shades of the colour, by how light the fur is: 256 steps.
    let shades: Vec<[u8; 3]> = (0..256)
        .map(|step| {
            let light = ((step as f32 / 255.0 - low) / (high - low).max(0.01)).clamp(0.0, 1.0);
            let shade = 0.36 + 0.9 * light;
            color.map(|channel| {
                let encoded = Color::linear_rgb(channel * shade, 0.0, 0.0).to_srgba().red;
                (encoded.clamp(0.0, 1.0) * 255.0).round() as u8
            })
        })
        .collect();
    for texel in texels.chunks_exact_mut(4) {
        let (most, least) = (
            *texel[..3].iter().max().unwrap(),
            *texel[..3].iter().min().unwrap(),
        );
        let light = (u32::from(texel[0]) + u32::from(texel[1]) + u32::from(texel[2])) / 3;
        // Fur is grey; skin is pink, and lighter than the dark fur about it
        // (which has a little colour too). It is kept in part: the skin
        // takes some of the fur's colour.
        let pink = (f32::from(most - least) / f32::from(most.max(1)) - 0.22) / 0.2;
        let lit = (f32::from(most) - 70.0) / 50.0;
        let own = pink.clamp(0.0, 1.0) * lit.clamp(0.0, 1.0) * 0.55;
        let shade = shades[light as usize];
        for channel in 0..3 {
            let kept = f32::from(texel[channel]);
            texel[channel] = (f32::from(shade[channel]) * (1.0 - own) + kept * own) as u8;
        }
    }
}

/// How light the fur's darkest and lightest texels are, 0 to 1, leaving out
/// the odd few at either end and the atlas's unused black.
fn range(texels: &[u8]) -> (f32, f32) {
    let mut counts = [0usize; 256];
    for texel in texels.chunks_exact(4) {
        if texel[..3].iter().any(|&channel| channel > 8) {
            counts[((u32::from(texel[0]) + u32::from(texel[1]) + u32::from(texel[2])) / 3)
                as usize] += 1;
        }
    }
    let total: usize = counts.iter().sum();
    let at = |part: f32| {
        let mut seen = 0;
        for (light, count) in counts.iter().enumerate() {
            seen += count;
            if seen as f32 >= total as f32 * part {
                return light as f32 / 255.0;
            }
        }
        1.0
    };
    (at(0.04), at(0.96))
}

/// A texture from the file as RGBA texels, shrunk: drawn a couple of
/// hundred pixels high, the full 2048 would shimmer.
fn decode(bytes: &[u8]) -> Result<(Vec<u8>, UVec2), String> {
    let image = Image::from_buffer(
        bytes,
        ImageType::MimeType("image/png"),
        CompressedImageFormats::NONE,
        true,
        ImageSampler::Default,
        RenderAssetUsages::RENDER_WORLD,
    )
    .map_err(|error| format!("Invalid cat texture: {error}"))?;
    if image.texture_descriptor.format != TextureFormat::Rgba8UnormSrgb {
        return Err("The cat's textures must be 8-bit RGBA".into());
    }
    let (mut width, mut height) = (image.width(), image.height());
    let mut texels = image.data.ok_or("The cat's texture is empty")?;
    while width > TEXTURE_SIZE && width % 2 == 0 && height % 2 == 0 {
        texels = halve(&texels, width, height);
        (width, height) = (width / 2, height / 2);
    }
    Ok((texels, UVec2::new(width, height)))
}

/// A texture of these RGBA texels, with mipmaps.
fn mipped(texels: Vec<u8>, size: UVec2) -> Image {
    let (mut width, mut height) = (size.x, size.y);
    let mut level = texels.clone();
    let mut image = Image::new(
        Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        texels,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    let mut data = image.data.take().unwrap_or_default();
    let mut levels = 1;
    while width > 1 && height > 1 && width % 2 == 0 && height % 2 == 0 {
        level = halve(&level, width, height);
        (width, height) = (width / 2, height / 2);
        data.extend_from_slice(&level);
        levels += 1;
    }
    image.texture_descriptor.mip_level_count = levels;
    image.data = Some(data);
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        ..default()
    });
    image
}

/// An RGBA image at half its size, each pixel the mean of four.
fn halve(pixels: &[u8], width: u32, height: u32) -> Vec<u8> {
    let (width, height) = (width as usize, height as usize);
    let mut half = Vec::with_capacity(width * height);
    for y in 0..height / 2 {
        for x in 0..width / 2 {
            for channel in 0..4 {
                let at = |dx: usize, dy: usize| {
                    u32::from(pixels[((y * 2 + dy) * width + x * 2 + dx) * 4 + channel])
                };
                half.push(((at(0, 0) + at(1, 0) + at(0, 1) + at(1, 1) + 2) / 4) as u8);
            }
        }
    }
    half
}

/// A cat in the world: its meshes and the joints that bend them, under `root`.
pub struct Rig {
    pub root: Entity,
    joints: Vec<Entity>,
}

impl Rig {
    pub fn spawn(world: &mut World, model: &Model, parent: Entity, layers: &RenderLayers) -> Self {
        let root = world
            .spawn((Transform::IDENTITY, Visibility::Inherited, ChildOf(parent)))
            .id();
        let joints: Vec<_> = model
            .joints
            .iter()
            .map(|_| world.spawn((Transform::IDENTITY, ChildOf(root))).id())
            .collect();
        for (mesh, material) in &model.parts {
            world.spawn((
                ChildOf(root),
                layers.clone(),
                Mesh3d(mesh.clone()),
                MeshMaterial3d(material.clone()),
                SkinnedMesh {
                    inverse_bindposes: model.inverse_binds.clone(),
                    joints: joints.clone(),
                },
                // Its bounds are the bind pose's, which it leaves.
                NoFrustumCulling,
                Transform::IDENTITY,
            ));
        }
        Self { root, joints }
    }

    /// Bend the cat into the pose these `globals` (`Model::globals`) are of.
    pub fn show(&self, world: &mut World, model: &Model, globals: &[Mat4]) {
        for (&entity, &node) in self.joints.iter().zip(&model.joints) {
            if let Some(mut transform) = world.get_mut::<Transform>(entity) {
                *transform = Transform::from_matrix(globals[node]);
            }
        }
    }
}
