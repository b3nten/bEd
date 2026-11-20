//! The 3D backdrop that shows bEd at work, as if its window were a clear
//! case: a circuit board with a chip for each of its parts (each bedtime
//! crate, and what Bevy does for it), wired to the `World` they all share.
//!
//! What lights up is what happened. Bevy stamps every resource and component
//! when it is written; each frame this looks at which were, and the chips
//! whose state changed light, more so when it is more than their usual, and
//! send a pulse down their trace to the world. Around them:
//!
//! - a card in the slots at the back for each open panel, the focused one lit;
//! - a fan that turns faster the more state is being written;
//! - a bar of lights for how long the UI took to draw;
//! - displays counting frames a second and the app's entities;
//! - lamps for power, the window's focus, a key held and the pointer moving.
//!
//! The rest (pins, sockets, capacitors, vias, the traces of other layers,
//! connectors) is a board's furniture. Everything that never changes is
//! gathered into one mesh for each stuff it is made of.
use crate::{
    backdrop::Backdrops,
    scene::{SceneImage, free_layer, hash, linear, request_frame, retire, surface_style},
};
use bedtime_ui::{UiDiagnostics, layouts::PanelPresentation, theme::Rgba, viewport_image};
use bevy::{
    asset::RenderAssetUsages,
    camera::{RenderTarget, visibility::RenderLayers},
    core_pipeline::tonemapping::{DebandDither, Tonemapping},
    ecs::{change_detection::Tick, component::ComponentId},
    image::ImageSampler,
    input::mouse::AccumulatedMouseMotion,
    light::CascadeShadowConfigBuilder,
    mesh::Indices,
    platform::collections::{HashMap, HashSet},
    prelude::*,
    render::render_resource::{Extent3d, PrimitiveTopology, TextureDimension, TextureFormat},
};
use std::{collections::BTreeMap, f32::consts::TAU};

/// The `Backdrop` setting's value for this scene.
pub const NAME: &str = "scene:internals";
pub const LABEL: &str = "Internals (3D)";

/// Half the width and depth of the part of the board the camera keeps in
/// view: the chips, the cards behind them and the gauges in front.
const VIEW: Vec2 = Vec2::new(16.8, 15.0);
/// Half the side of the world's chip, and of the socket its traces reach.
const CORE: f32 = 2.2;
const SOCKET: f32 = 2.75;
/// A chip's body.
const CHIP: Vec3 = Vec3::new(2.8, 0.45, 1.5);
/// How fast a pulse runs down a trace, a second.
const PULSE_SPEED: f32 = 26.0;
/// The least time between a part's pulses.
const PULSE_GAP: f32 = 0.12;
const CARDS: usize = 16;
const CARD_ROW: f32 = -12.4;
const GAUGE_ROW: f32 = 12.7;
/// A graded light's brightness is shown in this many steps, so its material
/// is only written when it visibly changes.
const STEPS: f32 = 16.0;
const FOV: f32 = 0.66;

/// A 3 by 5 letter, row by row.
const GLYPHS: &[(char, &str)] = &[
    ('A', "010101111101101"),
    ('B', "110101110101110"),
    ('C', "011100100100011"),
    ('D', "110101101101110"),
    ('E', "111100110100111"),
    ('F', "111100110100100"),
    ('G', "011100101101011"),
    ('H', "101101111101101"),
    ('I', "111010010010111"),
    ('J', "001001001101010"),
    ('K', "101101110101101"),
    ('L', "100100100100111"),
    ('M', "101111111101101"),
    ('N', "101111111111101"),
    ('O', "010101101101010"),
    ('P', "110101110100100"),
    ('Q', "010101101110011"),
    ('R', "110101110101101"),
    ('S', "011100010001110"),
    ('T', "111010010010010"),
    ('U', "101101101101111"),
    ('V', "101101101101010"),
    ('W', "101101111111101"),
    ('X', "101101010101101"),
    ('Y', "101101010010010"),
    ('Z', "111001010100111"),
];

/// Which of a digit's seven bars are lit: the top, then clockwise, then the
/// middle, from the lowest bit.
const DIGITS: [u8; 10] = [
    0b011_1111, 0b000_0110, 0b101_1011, 0b100_1111, 0b110_0110, 0b110_1101, 0b111_1101,
    0b000_0111, 0b111_1111, 0b110_1111,
];

/// `text` as the board's printed lettering: white on clear, a pixel a dot.
fn lettering(text: &str) -> Image {
    let letters: Vec<char> = text.chars().collect();
    let (width, height) = (letters.len() * 4 + 1, 7);
    let mut pixels = vec![255u8; width * height * 4];
    for pixel in pixels.chunks_mut(4) {
        pixel[3] = 0;
    }
    for (at, letter) in letters.iter().enumerate() {
        let Some((_, dots)) = GLYPHS.iter().find(|(glyph, _)| glyph == letter) else {
            continue;
        };
        for (dot, on) in dots.bytes().enumerate() {
            if on == b'1' {
                let (x, y) = (1 + at * 4 + dot % 3, 1 + dot / 3);
                pixels[(y * width + x) * 4 + 3] = 255;
            }
        }
    }
    let mut image = Image::new(
        Extent3d {
            width: width as u32,
            height: height as u32,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        pixels,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    image.sampler = ImageSampler::nearest();
    image
}

/// Which chip a type belongs to, from its path, and whether it is bEd's own
/// (its components are watched as well as its resources).
fn part_of(type_name: &str) -> Option<(String, bool)> {
    // The scenes aren't part of what they show.
    if ["bed::internals", "bed::scene", "bed::backdrop"]
        .iter()
        .any(|own| type_name.starts_with(own))
    {
        return None;
    }
    let krate = type_name.split("::").next()?;
    let engine = |label: &str| Some((label.to_owned(), false));
    match krate {
        "bed" => Some(("BED".into(), true)),
        "bedtime" => Some(("BEDTIME".into(), true)),
        "bevy_input" | "bevy_input_focus" => engine("INPUT"),
        "bevy_time" => engine("CLOCK"),
        "bevy_asset" => engine("ASSETS"),
        "bevy_window" | "bevy_winit" => engine("WINDOW"),
        "bevy_egui" => engine("EGUI"),
        "bevy_render" | "bevy_core_pipeline" | "bevy_pbr" | "bevy_camera" | "bevy_mesh"
        | "bevy_image" | "bevy_shader" | "bevy_light" | "bevy_anti_alias" => engine("RENDER"),
        _ => match krate.strip_prefix("bedtime_") {
            Some(name) => Some((name.replace('_', " ").to_uppercase(), true)),
            None if krate.starts_with("bevy_") => engine("BEVY"),
            None => None,
        },
    }
}

/// A part's label, the resources it keeps and, for bEd's own, its components.
type Survey = Vec<(String, Vec<ComponentId>, Vec<ComponentId>)>;

/// The app's parts, from the types it has registered.
fn survey(world: &World) -> Survey {
    let resources: HashSet<ComponentId> = world.iter_resources().map(|(id, _, _)| id).collect();
    let mut parts: BTreeMap<String, (Vec<ComponentId>, Vec<ComponentId>)> = BTreeMap::new();
    for (id, info) in world.components().iter_registered() {
        let Some((label, own)) = part_of(&info.name().to_string()) else {
            continue;
        };
        let part = parts.entry(label).or_default();
        if resources.contains(&id) {
            part.0.push(id);
        } else if own {
            part.1.push(id);
        }
    }
    parts
        .into_iter()
        .filter(|(_, (resources, components))| !resources.is_empty() || !components.is_empty())
        .map(|(label, (resources, components))| (label, resources, components))
        .collect()
}

/// Where each of `count` chips sits: two rows behind the world's chip and two
/// in front, and a column at either side, the nearest places first.
fn places(count: usize) -> Vec<Vec3> {
    // Each row's distance from the middle and how many chips it holds; the
    // third is the two side columns.
    const ROWS: [(f32, usize); 5] = [(-6.2, 8), (6.2, 8), (0.0, 6), (-9.0, 8), (9.0, 8)];
    let mut places = Vec::with_capacity(count);
    let mut left = count;
    for (row, (z, holds)) in ROWS.into_iter().enumerate() {
        let here = left.min(holds);
        left -= here;
        for at in 0..here {
            places.push(if row == 2 {
                let side = if at % 2 == 0 { -1.0 } else { 1.0 };
                let column = (here + 1 - at % 2) / 2;
                Vec3::new(
                    side * 12.8,
                    0.0,
                    ((at / 2) as f32 - (column as f32 - 1.0) / 2.0) * 2.8,
                )
            } else {
                Vec3::new((at as f32 - (here as f32 - 1.0) / 2.0) * 3.6, 0.0, z)
            });
        }
    }
    places
}

/// A trace from a chip to the world's socket: straight, then at 45 degrees,
/// as boards are routed.
fn route(from: Vec3) -> [Vec3; 3] {
    let to = Vec3::new(
        from.x.clamp(-SOCKET, SOCKET),
        0.0,
        from.z.clamp(-SOCKET, SOCKET),
    );
    let way = to - from;
    let (across, along) = (way.x.abs(), way.z.abs());
    let bend = if across > along {
        from + Vec3::new(way.x.signum() * (across - along), 0.0, 0.0)
    } else {
        from + Vec3::new(0.0, 0.0, way.z.signum() * (along - across))
    };
    [from, bend, to]
}

/// The point `along` a route, and whether that is its end.
fn on_route(route: [Vec3; 3], along: f32) -> (Vec3, bool) {
    let first = route[0].distance(route[1]);
    let second = route[1].distance(route[2]);
    if along < first {
        (route[0].lerp(route[1], along / first.max(0.001)), false)
    } else if along < first + second {
        (
            route[1].lerp(route[2], (along - first) / second.max(0.001)),
            false,
        )
    } else {
        (route[2], true)
    }
}

/// What the board's fixed things are made of. All of one stuff is one mesh.
#[derive(Clone, Copy)]
enum Stuff {
    /// Chip bodies.
    Chip,
    /// Sockets, slots and housings.
    Plastic,
    /// Pins, cans and shields.
    Tin,
    /// Contacts, pads and vias.
    Gold,
    /// What is printed on the board.
    Silk,
    /// The traces of other layers.
    Wire,
    /// Capacitor sleeves.
    Sleeve,
    /// Small ceramic parts.
    Clay,
}
const STUFFS: usize = 8;

/// Shapes gathered into one mesh.
#[derive(Default)]
struct Batch {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    indices: Vec<u32>,
}

impl Batch {
    fn corner(&mut self, at: Vec3, normal: Vec3) -> u32 {
        self.positions.push(at.to_array());
        self.normals.push(normal.to_array());
        self.positions.len() as u32 - 1
    }

    /// A box on its centre, turned about the upright. It has no underside.
    fn slab(&mut self, center: Vec3, size: Vec3, turn: f32) {
        let spin = Quat::from_rotation_y(turn);
        let half = size / 2.0;
        // Each face's normal and two edges, the edges' cross the normal.
        let faces = [
            (Vec3::Y, Vec3::Z, Vec3::X),
            (Vec3::X, Vec3::Y, Vec3::Z),
            (Vec3::NEG_X, Vec3::Z, Vec3::Y),
            (Vec3::Z, Vec3::X, Vec3::Y),
            (Vec3::NEG_Z, Vec3::Y, Vec3::X),
        ];
        for (normal, across, up) in faces {
            let middle = normal * half;
            let (across, up) = (across * half, up * half);
            let turned = spin * normal;
            let first = self.positions.len() as u32;
            for (a, b) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                self.corner(center + spin * (middle + across * a + up * b), turned);
            }
            self.indices
                .extend([first, first + 1, first + 2, first, first + 2, first + 3]);
        }
    }

    /// An upright cylinder standing on `base`, with a top.
    fn post(&mut self, base: Vec3, radius: f32, height: f32) {
        const SIDES: usize = 12;
        let around = |side: usize| {
            let angle = side as f32 / SIDES as f32 * TAU;
            Vec3::new(angle.cos(), 0.0, angle.sin())
        };
        let top = base + Vec3::Y * height;
        let middle = self.corner(top, Vec3::Y);
        for side in 0..SIDES {
            let (from, to) = (around(side), around(side + 1));
            let first = self.positions.len() as u32;
            self.corner(base + to * radius, to);
            self.corner(base + from * radius, from);
            self.corner(top + from * radius, from);
            self.corner(top + to * radius, to);
            self.indices
                .extend([first, first + 1, first + 2, first, first + 2, first + 3]);
            let rim = self.positions.len() as u32;
            self.corner(top + to * radius, Vec3::Y);
            self.corner(top + from * radius, Vec3::Y);
            self.indices.extend([middle, rim, rim + 1]);
        }
    }

    /// A flat strip on the board between two points.
    fn strip(&mut self, from: Vec3, to: Vec3, width: f32, height: f32) {
        let way = to - from;
        if way.length() < 0.01 {
            return;
        }
        self.slab(
            (from + to) / 2.0 + Vec3::Y * height / 2.0,
            Vec3::new(way.length() + width, height, width),
            (-way.z).atan2(way.x),
        );
    }

    fn mesh(self) -> Mesh {
        Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, self.positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, self.normals)
        .with_inserted_indices(Indices::U32(self.indices))
    }
}

/// One of bEd's parts: a chip, its activity light and its trace.
struct Part {
    label: String,
    resources: Vec<ComponentId>,
    components: Vec<ComponentId>,
    route: [Vec3; 3],
    light: Handle<StandardMaterial>,
    trace: Handle<StandardMaterial>,
    /// How lit it is: 1 when its state changed more than usual, then fading.
    glow: f32,
    /// How much of its state usually changes in a frame.
    usual: f32,
    /// Seconds since it last sent a pulse.
    quiet: f32,
    /// The step its light and trace show.
    shown: Option<u8>,
}

/// A pulse on its way down a part's trace.
struct Pulse {
    entity: Entity,
    part: usize,
    along: f32,
}

/// A row of lamps, lit from the left as far as a value reaches.
struct Bar {
    lamps: Vec<Entity>,
    shown: Option<usize>,
}

/// A number in seven-bar digits.
struct Display {
    digits: Vec<[Entity; 7]>,
    shown: Option<u32>,
}

/// An open panel's card.
struct Card {
    panel: Entity,
    entity: Entity,
    material: Handle<StandardMaterial>,
    shown: Option<u8>,
}

/// The theme's colours the scene is drawn in.
#[derive(Clone, Copy, PartialEq)]
struct Colors {
    background: Rgba,
    muted: Rgba,
    text: Rgba,
    accent: Rgba,
}

/// The scene's own colours, made from the theme's.
#[derive(Clone, Copy)]
struct Look {
    /// A light that is off, and one fully lit.
    dim: LinearRgba,
    lit: LinearRgba,
    /// A trace nothing is on.
    trace: LinearRgba,
    chip: LinearRgba,
}

impl Look {
    fn light(&self, level: f32) -> LinearRgba {
        self.dim.mix(&self.lit, level)
    }
}

/// The scene while it is the chosen backdrop.
#[derive(Resource)]
pub struct Internals {
    root: Entity,
    camera: Entity,
    image: Handle<Image>,
    layers: RenderLayers,
    cube: Handle<Mesh>,
    ball: Handle<Mesh>,
    /// The entities the scene is made of, which aren't the app's to count.
    own: u32,
    parts: Vec<Part>,
    /// How many types the app had registered when its parts were surveyed,
    /// and how long ago that was.
    registered: usize,
    surveyed: f32,
    /// When state was last looked at: what changed since is what's new.
    last: Tick,
    pulses: Vec<Pulse>,
    pulse: Handle<StandardMaterial>,
    cards: Vec<Card>,
    /// A lamp that is on, and one that is off: lamps swap between them.
    on: Handle<StandardMaterial>,
    off: Handle<StandardMaterial>,
    draw: Bar,
    rate: Display,
    entities: Display,
    /// Power, focus, key and pointer, and what they show.
    lamps: [Entity; 4],
    lamps_shown: Option<[bool; 4]>,
    /// The fan turns with how much state is being written.
    fan: Entity,
    load: f32,
    frames: f32,
    /// Seconds since the pointer last moved.
    still: f32,
    /// The world's chip lights as pulses reach it.
    core: Handle<StandardMaterial>,
    core_glow: f32,
    core_shown: Option<u8>,
    board: Handle<StandardMaterial>,
    stuffs: [Handle<StandardMaterial>; STUFFS],
    /// The lettering on the chips, and that printed on the board.
    labels: Vec<Handle<StandardMaterial>>,
    printed: Vec<Handle<StandardMaterial>>,
    look: Option<(Colors, Look)>,
    time: f32,
}

/// What the scene is built from.
struct Kit<'w> {
    world: &'w mut World,
    root: Entity,
    layers: RenderLayers,
    cube: Handle<Mesh>,
    plane: Handle<Mesh>,
    off: Handle<StandardMaterial>,
    batches: [Batch; STUFFS],
    /// Where something stands, so the small parts scattered last keep clear:
    /// each a middle and half a size, across the board.
    taken: Vec<(Vec2, Vec2)>,
    labels: Vec<Handle<StandardMaterial>>,
    printed: Vec<Handle<StandardMaterial>>,
    spawned: u32,
}

impl Kit<'_> {
    fn material(&mut self, material: StandardMaterial) -> Handle<StandardMaterial> {
        self.world
            .resource_mut::<Assets<StandardMaterial>>()
            .add(material)
    }

    /// A graded light's material: it shows its colour whatever shines on it.
    fn glow(&mut self) -> Handle<StandardMaterial> {
        self.material(StandardMaterial {
            unlit: true,
            ..default()
        })
    }

    fn put(&mut self, stuff: Stuff) -> &mut Batch {
        &mut self.batches[stuff as usize]
    }

    fn slab(&mut self, stuff: Stuff, center: Vec3, size: Vec3) {
        self.put(stuff).slab(center, size, 0.0);
    }

    fn take(&mut self, middle: Vec3, half: Vec2) {
        self.taken.push((Vec2::new(middle.x, middle.z), half));
    }

    fn clear(&self, middle: Vec2, half: Vec2) -> bool {
        self.taken.iter().all(|(taken, size)| {
            (middle.x - taken.x).abs() > half.x + size.x
                || (middle.y - taken.y).abs() > half.y + size.y
        })
    }

    /// A thing of its own, for what changes.
    fn block(&mut self, size: Vec3, center: Vec3, material: &Handle<StandardMaterial>) -> Entity {
        self.spawned += 1;
        self.world
            .spawn((
                ChildOf(self.root),
                self.layers.clone(),
                Mesh3d(self.cube.clone()),
                MeshMaterial3d(material.clone()),
                Transform::from_translation(center).with_scale(size),
            ))
            .id()
    }

    /// A lamp, off.
    fn lamp(&mut self, size: Vec3, center: Vec3) -> Entity {
        let off = self.off.clone();
        self.block(size, center, &off)
    }

    /// A mesh of its own, for what moves or lights as one.
    fn shape(&mut self, batch: Batch, material: &Handle<StandardMaterial>, at: Vec3) -> Entity {
        self.spawned += 1;
        let mesh = self.world.resource_mut::<Assets<Mesh>>().add(batch.mesh());
        self.world
            .spawn((
                ChildOf(self.root),
                self.layers.clone(),
                Mesh3d(mesh),
                MeshMaterial3d(material.clone()),
                Transform::from_translation(at),
            ))
            .id()
    }

    /// Lettering lying flat, read from the front, no wider than `width`: on
    /// a chip, or printed on the board.
    fn label(&mut self, text: &str, center: Vec3, width: f32, height: f32, on_chip: bool) {
        let image = lettering(text);
        let shape = image.size().as_vec2();
        let size = Vec2::new(height * shape.x / shape.y, height);
        let size = size * (width / size.x).min(1.0);
        let texture = self.world.resource_mut::<Assets<Image>>().add(image);
        let material = self.material(StandardMaterial {
            base_color_texture: Some(texture),
            unlit: true,
            alpha_mode: AlphaMode::Blend,
            ..default()
        });
        self.spawned += 1;
        self.world.spawn((
            ChildOf(self.root),
            self.layers.clone(),
            Mesh3d(self.plane.clone()),
            MeshMaterial3d(material.clone()),
            Transform::from_translation(center).with_scale(Vec3::new(size.x, 1.0, size.y)),
        ));
        if on_chip {
            self.labels.push(material);
        } else {
            self.printed.push(material);
        }
    }

    /// A printed outline round a place.
    fn outline(&mut self, middle: Vec3, half: Vec2) {
        let silk = self.put(Stuff::Silk);
        for (from, to) in [
            (Vec2::new(-1.0, -1.0), Vec2::new(1.0, -1.0)),
            (Vec2::new(1.0, -1.0), Vec2::new(1.0, 1.0)),
            (Vec2::new(1.0, 1.0), Vec2::new(-1.0, 1.0)),
            (Vec2::new(-1.0, 1.0), Vec2::new(-1.0, -1.0)),
        ] {
            let corner = |at: Vec2| middle + Vec3::new(at.x * half.x, 0.0, at.y * half.y);
            silk.strip(corner(from), corner(to), 0.05, 0.012);
        }
    }

    /// A via: a plated hole through the board.
    fn via(&mut self, at: Vec3) {
        self.put(Stuff::Gold)
            .post(Vec3::new(at.x, 0.0, at.z), 0.11, 0.028);
        self.put(Stuff::Chip)
            .post(Vec3::new(at.x, 0.0, at.z), 0.045, 0.032);
    }

    /// A chip: its body, the pins down its long sides, the outline printed
    /// round it and the dot marking its first pin.
    fn chip(&mut self, home: Vec3, size: Vec3, pins: usize) {
        self.slab(Stuff::Chip, home + Vec3::Y * size.y / 2.0, size);
        for pin in 0..pins {
            let x = (pin as f32 - (pins as f32 - 1.0) / 2.0) * (size.x - 0.4) / (pins as f32 - 1.0);
            for side in [-1.0, 1.0] {
                let z = side * (size.z / 2.0 + 0.1);
                self.slab(
                    Stuff::Tin,
                    home + Vec3::new(x, 0.07, z),
                    Vec3::new(0.13, 0.14, 0.3),
                );
                self.slab(
                    Stuff::Gold,
                    home + Vec3::new(x, 0.012, z + side * 0.12),
                    Vec3::new(0.19, 0.024, 0.3),
                );
            }
        }
        let half = Vec2::new(size.x / 2.0 + 0.22, size.z / 2.0 + 0.48);
        self.outline(home, half);
        self.put(Stuff::Silk).post(
            home + Vec3::new(-half.x - 0.2, 0.0, half.y - 0.1),
            0.09,
            0.014,
        );
        self.take(home, half + Vec2::splat(0.15));
    }

    /// A small part lying on the board with a tinned end at each side: a
    /// resistor (dark) or a capacitor (ceramic).
    fn small(&mut self, at: Vec3, turn: f32, long: f32, ceramic: bool) {
        let (stuff, high) = if ceramic {
            (Stuff::Clay, 0.22)
        } else {
            (Stuff::Chip, 0.14)
        };
        let center = Vec3::new(at.x, high / 2.0, at.z);
        self.put(stuff)
            .slab(center, Vec3::new(long * 0.62, high, 0.26), turn);
        let along = Quat::from_rotation_y(turn) * Vec3::X * long * 0.4;
        for end in [-1.0, 1.0] {
            self.put(Stuff::Tin).slab(
                center + along * end,
                Vec3::new(long * 0.2, high + 0.02, 0.28),
                turn,
            );
        }
    }

    /// An electrolytic capacitor: a sleeved can with a scored top.
    fn can(&mut self, at: Vec3, radius: f32, high: f32) {
        self.put(Stuff::Sleeve).post(at, radius, high);
        self.put(Stuff::Tin)
            .post(at + Vec3::Y * high, radius * 0.86, 0.02);
        let score = self.put(Stuff::Chip);
        score.slab(
            at + Vec3::Y * (high + 0.022),
            Vec3::new(radius * 1.3, 0.01, 0.04),
            0.0,
        );
        score.slab(
            at + Vec3::Y * (high + 0.022),
            Vec3::new(0.04, 0.01, radius * 1.3),
            0.0,
        );
        self.take(at, Vec2::splat(radius + 0.15));
    }

    /// A row of header pins standing in their strip.
    fn header(&mut self, middle: Vec3, pins: usize, along_x: bool) {
        let step = if along_x { Vec3::X } else { Vec3::Z } * 0.5;
        let side = if along_x { Vec3::Z } else { Vec3::X } * 0.25;
        let long = pins as f32 * 0.5;
        let size = if along_x {
            Vec3::new(long, 0.28, 1.0)
        } else {
            Vec3::new(1.0, 0.28, long)
        };
        self.slab(Stuff::Plastic, middle + Vec3::Y * 0.14, size);
        for pin in 0..pins {
            let at = middle + step * (pin as f32 - (pins as f32 - 1.0) / 2.0);
            for row in [-1.0, 1.0] {
                self.slab(
                    Stuff::Gold,
                    at + side * row + Vec3::Y * 0.45,
                    Vec3::new(0.1, 0.9, 0.1),
                );
            }
        }
        self.take(middle, Vec2::new(size.x, size.z) / 2.0 + Vec2::splat(0.2));
    }

    /// A labelled row of lamps starting at `left`.
    fn bar(&mut self, text: &str, left: Vec3, lamps: usize) -> Bar {
        self.label(text, left + Vec3::new(1.1, 0.03, 0.0), 2.2, 0.5, false);
        let long = lamps as f32 * 0.5;
        self.slab(
            Stuff::Plastic,
            left + Vec3::new(2.6 + long / 2.0, 0.1, 0.0),
            Vec3::new(long + 0.3, 0.2, 0.7),
        );
        let lamps = (0..lamps)
            .map(|at| {
                self.lamp(
                    Vec3::new(0.34, 0.1, 0.42),
                    left + Vec3::new(2.85 + at as f32 * 0.5, 0.25, 0.0),
                )
            })
            .collect();
        Bar { lamps, shown: None }
    }

    /// A labelled display of `digits` seven-bar digits starting at `left`.
    fn display(&mut self, text: &str, left: Vec3, label: f32, digits: usize) -> Display {
        self.label(
            text,
            left + Vec3::new(label / 2.0, 0.03, 0.0),
            label,
            0.5,
            false,
        );
        let long = digits as f32 * 0.85;
        let start = left.x + label + 0.45;
        self.slab(
            Stuff::Plastic,
            Vec3::new(start + long / 2.0, 0.13, left.z),
            Vec3::new(long + 0.3, 0.26, 1.5),
        );
        let digits = (0..digits)
            .map(|at| {
                let middle = Vec3::new(start + 0.42 + at as f32 * 0.85, 0.29, left.z);
                // Top, the right side down, bottom, the left side up, middle.
                [
                    (0.0, -0.5, true),
                    (0.26, -0.25, false),
                    (0.26, 0.25, false),
                    (0.0, 0.5, true),
                    (-0.26, 0.25, false),
                    (-0.26, -0.25, false),
                    (0.0, 0.0, true),
                ]
                .map(|(x, z, flat)| {
                    let size = if flat {
                        Vec3::new(0.4, 0.05, 0.09)
                    } else {
                        Vec3::new(0.09, 0.05, 0.4)
                    };
                    self.lamp(size, middle + Vec3::new(x, 0.0, z))
                })
            })
            .collect();
        Display {
            digits,
            shown: None,
        }
    }
}

/// A board's furniture, around what the scene watches: capacitors, a
/// regulator's heat sink, a battery, a crystal, coils, connectors, the
/// traces and vias of other layers, mounting holes, and small parts wherever
/// there is room.
fn furnish(kit: &mut Kit) {
    // Capacitors in banks.
    for (bank, (x, z, cans)) in [
        (-19.5, -6.0, 4),
        (19.5, 5.5, 4),
        (-9.5, -15.6, 5),
        (11.0, -15.4, 3),
        (-20.0, 9.5, 3),
        (19.0, -9.5, 3),
    ]
    .into_iter()
    .enumerate()
    {
        for can in 0..cans {
            let size = 0.36 + hash(bank as i64, can) * 0.2;
            kit.can(
                Vec3::new(
                    x + (can % 3) as f32 * 1.15,
                    0.0,
                    z + (can / 3) as f32 * 1.15,
                ),
                size,
                0.7 + size * 1.4,
            );
        }
    }
    // A regulator under its heat sink.
    for (x, z) in [(-16.0, -15.6), (21.5, 0.5)] {
        let middle = Vec3::new(x, 0.0, z);
        kit.slab(
            Stuff::Tin,
            middle + Vec3::Y * 0.08,
            Vec3::new(2.2, 0.16, 1.8),
        );
        for fin in 0..9 {
            kit.slab(
                Stuff::Tin,
                middle + Vec3::new(-0.96 + fin as f32 * 0.24, 0.7, 0.0),
                Vec3::new(0.07, 1.1, 1.7),
            );
        }
        kit.take(middle, Vec2::new(1.3, 1.1));
    }
    // The clock's battery in its holder, and its crystal.
    let battery = Vec3::new(5.5, 0.0, -15.8);
    kit.put(Stuff::Plastic).post(battery, 1.15, 0.12);
    kit.put(Stuff::Tin)
        .post(battery + Vec3::Y * 0.12, 0.95, 0.2);
    kit.slab(
        Stuff::Tin,
        battery + Vec3::new(0.0, 0.36, 0.0),
        Vec3::new(0.5, 0.06, 2.3),
    );
    kit.take(battery, Vec2::splat(1.3));
    for (x, z) in [(1.8, -15.6), (-21.0, 2.5)] {
        let crystal = Vec3::new(x, 0.0, z);
        kit.slab(
            Stuff::Tin,
            crystal + Vec3::Y * 0.2,
            Vec3::new(1.2, 0.4, 0.5),
        );
        kit.outline(crystal, Vec2::new(0.8, 0.45));
        kit.take(crystal, Vec2::new(0.9, 0.55));
    }
    // Coils.
    for (at, (x, z)) in [
        (-19.8, -1.0),
        (-18.2, -1.0),
        (19.2, -3.5),
        (20.8, -3.5),
        (15.2, -15.6),
    ]
    .into_iter()
    .enumerate()
    {
        let coil = Vec3::new(x, 0.0, z);
        kit.slab(
            Stuff::Chip,
            coil + Vec3::Y * 0.3,
            Vec3::new(1.1, 0.6, 1.1),
        );
        kit.put(Stuff::Gold)
            .post(coil + Vec3::Y * 0.6, 0.28 + (at % 2) as f32 * 0.06, 0.03);
        kit.take(coil, Vec2::splat(0.7));
    }
    // Connectors: headers down the sides, sockets along the back edge.
    for (x, z, pins, along_x) in [
        (-23.5, -4.0, 12, false),
        (23.5, 6.0, 12, false),
        (-23.5, 9.0, 6, false),
        (23.5, -8.0, 8, false),
        (-19.0, 15.6, 8, true),
        (19.5, 15.6, 6, true),
    ] {
        kit.header(Vec3::new(x, 0.0, z), pins, along_x);
    }
    for (x, wide) in [(-22.0, 2.6), (-18.2, 2.0), (18.5, 2.0), (22.0, 3.0)] {
        let socket = Vec3::new(x, 0.0, -19.0);
        kit.slab(
            Stuff::Tin,
            socket + Vec3::Y * 0.6,
            Vec3::new(wide, 1.2, 2.2),
        );
        kit.slab(
            Stuff::Chip,
            socket + Vec3::new(0.0, 0.6, 1.11),
            Vec3::new(wide - 0.5, 0.7, 0.02),
        );
        kit.take(socket, Vec2::new(wide / 2.0 + 0.2, 1.3));
    }
    // Mounting holes, ringed.
    for (x, z) in [(-26.0, -20.0), (26.0, -20.0), (-26.0, 17.5), (26.0, 17.5)] {
        let hole = Vec3::new(x, 0.0, z);
        kit.put(Stuff::Gold).post(hole, 0.85, 0.02);
        kit.put(Stuff::Chip).post(hole, 0.45, 0.03);
        kit.take(hole, Vec2::splat(1.0));
    }

    // Other layers' traces: runs of straight and 45 degree legs on a grid,
    // ending in vias.
    for run in 0..170 {
        let grid = |seed: i64, span: f32| ((hash(run, seed) - 0.5) * span * 2.0).round() / 2.0;
        let mut at = Vec3::new(grid(21, 29.0), 0.0, grid(22, 21.0));
        let mut heading = (hash(run, 23) * 8.0) as i32;
        kit.via(at);
        for leg in 0..2 + (hash(run, 24) * 3.0) as i64 {
            let long = 1.0 + (hash(run, 30 + leg) * 14.0).round() / 2.0;
            let angle = heading as f32 * TAU / 8.0;
            // A diagonal leg still ends on the grid.
            let way = Vec3::new(angle.cos().round(), 0.0, angle.sin().round());
            let to = at + way * long;
            kit.put(Stuff::Wire).strip(at, to, 0.07, 0.008);
            at = to;
            heading += if hash(run, 40 + leg) < 0.5 { 1 } else { -1 };
        }
        kit.via(at);
    }
    // Loose vias, in rows as they come under a chip.
    for group in 0..40 {
        let x = ((hash(group, 51) - 0.5) * 56.0).round();
        let z = ((hash(group, 52) - 0.5) * 40.0).round();
        for via in 0..3 + (hash(group, 53) * 5.0) as i64 {
            kit.via(Vec3::new(x + via as f32 * 0.4, 0.0, z));
        }
    }

    // Small parts wherever nothing stands: resistors, capacitors and little
    // three-legged chips.
    for part in 0..900 {
        let at = Vec3::new(
            ((hash(part, 61) - 0.5) * 116.0).round() / 2.0,
            0.0,
            ((hash(part, 62) - 0.5) * 84.0).round() / 2.0,
        );
        let upright = hash(part, 63) < 0.5;
        let kind = hash(part, 64);
        let long = if kind < 0.85 { 0.7 } else { 0.9 };
        let half = if upright {
            Vec2::new(0.22, long / 2.0 + 0.1)
        } else {
            Vec2::new(long / 2.0 + 0.1, 0.22)
        };
        if !kit.clear(Vec2::new(at.x, at.z), half) {
            continue;
        }
        let turn = if upright { TAU / 4.0 } else { 0.0 };
        if kind < 0.85 {
            kit.small(at, turn, long, kind < 0.4);
        } else {
            kit.put(Stuff::Chip)
                .slab(at + Vec3::Y * 0.13, Vec3::new(0.7, 0.26, 0.42), turn);
            let spin = Quat::from_rotation_y(turn);
            for (x, z) in [(-0.22, 0.3), (0.22, 0.3), (0.0, -0.3)] {
                kit.put(Stuff::Tin).slab(
                    at + spin * Vec3::new(x, 0.05, z),
                    Vec3::new(0.12, 0.1, 0.22),
                    turn,
                );
            }
        }
        kit.take(at, half);
    }
}

fn spawn(world: &mut World, parts: Survey) -> Internals {
    let layers = free_layer(world);
    let image = world
        .resource_mut::<Assets<Image>>()
        .add(viewport_image(UVec2::ONE));
    let mut meshes = world.resource_mut::<Assets<Mesh>>();
    let cube = meshes.add(Cuboid::new(1.0, 1.0, 1.0));
    let plane = meshes.add(Plane3d::default().mesh().size(1.0, 1.0));
    let ball = meshes.add(Sphere::new(0.16).mesh().ico(2).unwrap());
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
            // The theme's colours as they are.
            Tonemapping::None,
            DebandDither::Disabled,
            AmbientLight {
                brightness: 300.0,
                ..default()
            },
            Projection::Perspective(PerspectiveProjection {
                fov: FOV,
                ..default()
            }),
            Transform::IDENTITY,
        ))
        .id();
    world.spawn((
        ChildOf(root),
        layers.clone(),
        DirectionalLight {
            illuminance: 3400.0,
            shadow_maps_enabled: true,
            ..default()
        },
        CascadeShadowConfigBuilder {
            num_cascades: 2,
            maximum_distance: 110.0,
            ..default()
        }
        .build(),
        Transform::from_xyz(-12.0, 22.0, 9.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    let lamp = StandardMaterial {
        unlit: true,
        ..default()
    };
    let mut materials = world.resource_mut::<Assets<StandardMaterial>>();
    let (on, off) = (materials.add(lamp.clone()), materials.add(lamp.clone()));
    let pulse = materials.add(lamp);
    let mut kit = Kit {
        world,
        root,
        layers: layers.clone(),
        cube: cube.clone(),
        plane,
        off: off.clone(),
        batches: default(),
        taken: Vec::new(),
        labels: Vec::new(),
        printed: Vec::new(),
        // The root, the camera and the light.
        spawned: 3,
    };
    let board = kit.material(StandardMaterial {
        perceptual_roughness: 0.85,
        reflectance: 0.2,
        ..default()
    });
    // How rough each stuff is, in `Stuff`'s order.
    let stuffs = [0.4, 0.55, 0.3, 0.3, 0.9, 0.8, 0.5, 0.7].map(|roughness| {
        kit.material(StandardMaterial {
            perceptual_roughness: roughness,
            ..default()
        })
    });
    kit.block(
        Vec3::new(120.0, 0.4, 88.0),
        Vec3::new(0.0, -0.2, 0.0),
        &board,
    );

    // The world: the chip every part is wired to, in its socket, with a
    // plate that lights.
    kit.slab(
        Stuff::Plastic,
        Vec3::Y * 0.1,
        Vec3::new(SOCKET * 2.0, 0.2, SOCKET * 2.0),
    );
    kit.slab(
        Stuff::Chip,
        Vec3::Y * 0.45,
        Vec3::new(CORE * 2.0, 0.5, CORE * 2.0),
    );
    for pin in 0..17 {
        let along = (pin as f32 - 8.0) * 0.3;
        for (x, z) in [
            (along, -2.48),
            (along, 2.48),
            (-2.48, along),
            (2.48, along),
        ] {
            kit.slab(
                Stuff::Gold,
                Vec3::new(x, 0.22, z),
                Vec3::new(0.14, 0.05, 0.14),
            );
        }
    }
    // The socket's lever.
    kit.slab(
        Stuff::Tin,
        Vec3::new(SOCKET + 0.15, 0.2, 0.4),
        Vec3::new(0.12, 0.12, 4.2),
    );
    kit.outline(Vec3::ZERO, Vec2::splat(SOCKET + 0.45));
    kit.take(Vec3::ZERO, Vec2::splat(SOCKET + 0.7));
    let core = kit.glow();
    kit.block(Vec3::new(3.4, 0.04, 3.4), Vec3::Y * 0.72, &core);
    kit.label("WORLD", Vec3::Y * 0.76, 3.0, 0.9, true);

    let places = places(parts.len());
    let parts = parts
        .into_iter()
        .zip(places)
        .enumerate()
        .map(|(at, ((label, resources, components), home))| {
            let (light, trace) = (kit.glow(), kit.glow());
            let route = route(home);
            let mut run = Batch::default();
            run.strip(route[0], route[1], 0.11, 0.022);
            run.strip(route[1], route[2], 0.11, 0.022);
            kit.shape(run, &trace, Vec3::ZERO);
            kit.via(route[1]);
            kit.chip(home, CHIP, 8);
            kit.block(
                Vec3::new(2.4, 0.08, 0.16),
                home + Vec3::new(0.0, CHIP.y + 0.02, 0.52),
                &light,
            );
            kit.label(
                &label,
                home + Vec3::new(0.0, CHIP.y + 0.02, -0.12),
                2.5,
                0.5,
                true,
            );
            // Each chip's own capacitor and resistor, beside it.
            let side = if at % 2 == 0 { 1.0 } else { -1.0 };
            kit.small(
                home + Vec3::new(side * 1.95, 0.0, -0.35),
                TAU / 4.0,
                0.7,
                true,
            );
            kit.small(
                home + Vec3::new(side * 1.95, 0.0, 0.5),
                TAU / 4.0,
                0.7,
                false,
            );
            kit.take(home + Vec3::X * side * 1.95, Vec2::new(0.3, 1.0));
            Part {
                label,
                resources,
                components,
                route,
                light,
                trace,
                glow: 0.0,
                usual: 0.0,
                quiet: 0.0,
                shown: None,
            }
        })
        .collect();

    // The slots at the back, a card in one for each open panel.
    for slot in 0..CARDS {
        let at = Vec3::new(
            (slot as f32 - (CARDS as f32 - 1.0) / 2.0) * 1.8,
            0.0,
            CARD_ROW,
        );
        kit.slab(
            Stuff::Plastic,
            at + Vec3::Y * 0.14,
            Vec3::new(1.6, 0.28, 0.42),
        );
        kit.slab(
            Stuff::Gold,
            at + Vec3::new(0.0, 0.29, 0.0),
            Vec3::new(1.4, 0.02, 0.12),
        );
    }
    let slots = CARDS as f32 * 0.9;
    kit.label(
        "PANELS",
        Vec3::new(-slots + 1.2, 0.03, CARD_ROW + 0.95),
        2.6,
        0.5,
        false,
    );
    kit.outline(Vec3::new(0.0, 0.0, CARD_ROW), Vec2::new(slots + 0.3, 0.55));
    kit.take(
        Vec3::new(0.0, 0.0, CARD_ROW + 0.3),
        Vec2::new(slots + 0.5, 1.2),
    );

    // The gauges in front: the fan, the draw bar, the two displays and the
    // lamps.
    let row = Vec3::new(0.0, 0.0, GAUGE_ROW);
    let fan_at = row + Vec3::X * -14.2;
    for (x, z, size) in [
        (0.0, -1.45, Vec3::new(3.1, 0.6, 0.2)),
        (0.0, 1.45, Vec3::new(3.1, 0.6, 0.2)),
        (-1.45, 0.0, Vec3::new(0.2, 0.6, 2.7)),
        (1.45, 0.0, Vec3::new(0.2, 0.6, 2.7)),
    ] {
        kit.slab(Stuff::Plastic, fan_at + Vec3::new(x, 0.3, z), size);
    }
    kit.put(Stuff::Chip).post(fan_at, 1.36, 0.03);
    let mut rotor = Batch::default();
    rotor.post(Vec3::ZERO, 0.42, 0.34);
    for blade in 0..7 {
        let turn = blade as f32 / 7.0 * TAU;
        rotor.slab(
            Quat::from_rotation_y(turn) * Vec3::new(0.8, 0.2, 0.12),
            Vec3::new(0.95, 0.05, 0.42),
            turn - 0.35,
        );
    }
    let tin = stuffs[Stuff::Tin as usize].clone();
    let fan = kit.shape(rotor, &tin, fan_at + Vec3::Y * 0.1);
    kit.label(
        "LOAD",
        fan_at + Vec3::new(0.0, 0.03, -1.95),
        1.8,
        0.45,
        false,
    );
    let draw = kit.bar("DRAW", row + Vec3::X * -11.6, 10);
    let rate = kit.display("FPS", row + Vec3::X * -2.2, 1.5, 3);
    let entities = kit.display("ENTITIES", row + Vec3::X * 3.2, 3.2, 5);
    let names = ["PWR", "FOCUS", "KEY", "PTR"];
    let lamps = [0, 1, 2, 3].map(|at| {
        let middle = row + Vec3::X * (12.6 + at as f32 * 1.25);
        kit.slab(
            Stuff::Plastic,
            middle + Vec3::Y * 0.08,
            Vec3::new(0.6, 0.16, 0.6),
        );
        kit.label(
            names[at],
            middle + Vec3::new(0.0, 0.03, -0.75),
            1.1,
            0.34,
            false,
        );
        kit.lamp(Vec3::new(0.4, 0.14, 0.4), middle + Vec3::Y * 0.2)
    });
    kit.outline(row, Vec2::new(VIEW.x, 1.75));
    kit.take(row + Vec3::Z * -0.3, Vec2::new(VIEW.x + 0.2, 2.3));

    furnish(&mut kit);

    // All that never changes, a mesh for each stuff.
    let batches = std::mem::take(&mut kit.batches);
    for (batch, material) in batches.into_iter().zip(&stuffs) {
        if !batch.positions.is_empty() {
            kit.shape(batch, material, Vec3::ZERO);
        }
    }
    let (labels, printed, own) = (kit.labels, kit.printed, kit.spawned);

    world.insert_resource(SceneImage(image.clone()));
    Internals {
        root,
        camera,
        image,
        layers,
        cube,
        ball,
        own,
        parts,
        registered: world.components().len(),
        surveyed: 0.0,
        last: world.change_tick(),
        pulses: Vec::new(),
        pulse,
        cards: Vec::new(),
        on,
        off,
        draw,
        rate,
        entities,
        lamps,
        lamps_shown: None,
        fan,
        load: 0.0,
        frames: 0.0,
        still: 1.0,
        core,
        core_glow: 0.0,
        core_shown: None,
        board,
        stuffs,
        labels,
        printed,
        look: None,
        time: 0.0,
    }
}

/// How many of each part's resources and components were written since
/// `last`.
fn changes(world: &World, parts: &[Part], last: Tick, now: Tick) -> Vec<usize> {
    let mut counts: Vec<usize> = parts
        .iter()
        .map(|part| {
            part.resources
                .iter()
                .filter(|id| {
                    world
                        .get_resource_change_ticks_by_id(**id)
                        .is_some_and(|ticks| ticks.changed.is_newer_than(last, now))
                })
                .count()
        })
        .collect();
    let owner: HashMap<ComponentId, usize> = parts
        .iter()
        .enumerate()
        .flat_map(|(part, parts)| parts.components.iter().map(move |id| (*id, part)))
        .collect();
    for archetype in world.archetypes().iter() {
        let watched: Vec<(ComponentId, usize)> = archetype
            .components()
            .iter()
            .filter_map(|id| owner.get(id).map(|part| (*id, *part)))
            .collect();
        if watched.is_empty() {
            continue;
        }
        for entity in archetype.entities() {
            let entity = world.entity(entity.id());
            for (id, part) in &watched {
                if entity
                    .get_change_ticks_by_id(*id)
                    .is_some_and(|ticks| ticks.changed.is_newer_than(last, now))
                {
                    counts[*part] += 1;
                }
            }
        }
    }
    counts
}

fn paint(
    materials: &mut Assets<StandardMaterial>,
    material: &Handle<StandardMaterial>,
    color: LinearRgba,
) {
    if let Some(mut material) = materials.get_mut(material) {
        material.base_color = color.into();
    }
}

fn step(level: f32) -> u8 {
    (level.clamp(0.0, 1.0) * STEPS).round() as u8
}

/// What the scene reads of the app each frame, besides what was written.
struct Readings {
    draw_ms: f32,
    /// Each open panel, whether it is visible, and whether it is focused.
    panels: Vec<(Entity, bool, bool)>,
    focused: bool,
    key: bool,
    pointer: bool,
    entities: u32,
    /// Seconds since the last frame, as they were and no more than a tenth.
    elapsed: f32,
    delta: f32,
}

fn read(world: &mut World) -> Readings {
    let mut panels: Vec<(Entity, bool, bool)> = world
        .query::<(Entity, &PanelPresentation)>()
        .iter(world)
        .map(|(panel, shown)| (panel, shown.visible, shown.focused))
        .collect();
    panels.sort_by_key(|(panel, _, _)| *panel);
    panels.truncate(CARDS);
    let elapsed = world.resource::<Time<Real>>().delta_secs();
    Readings {
        draw_ms: world
            .query::<&UiDiagnostics>()
            .iter(world)
            .next()
            .map_or(0.0, |diagnostics| diagnostics.draw_ms as f32),
        panels,
        focused: world
            .query::<&Window>()
            .iter(world)
            .any(|window| window.focused),
        key: world
            .get_resource::<ButtonInput<KeyCode>>()
            .is_some_and(|keys| keys.get_pressed().next().is_some()),
        pointer: world
            .get_resource::<AccumulatedMouseMotion>()
            .is_some_and(|motion| motion.delta != Vec2::ZERO),
        entities: world.entities().len(),
        elapsed,
        // A long pause (the window was idle) isn't a leap forward.
        delta: elapsed.min(0.1),
    }
}

/// Start and stop the scene with the setting; while it shows, look at what
/// bEd did this frame and show it.
pub fn update(world: &mut World) {
    if world.resource::<Backdrops>().chosen() != NAME {
        if let Some(internals) = world.remove_resource::<Internals>() {
            world.entity_mut(internals.root).despawn();
            retire(world, &internals.image);
        }
        return;
    }
    // Built for the parts the app has: if a plugin's types appear later (its
    // first panel opens), build it again with them.
    let stale = world.get_resource::<Internals>().is_none_or(|internals| {
        internals.surveyed > 2.0 || internals.registered != world.components().len()
    });
    if stale {
        let found = survey(world);
        let same = world.get_resource::<Internals>().is_some_and(|internals| {
            internals
                .parts
                .iter()
                .map(|part| &part.label)
                .eq(found.iter().map(|(label, _, _)| label))
        });
        if same {
            let registered = world.components().len();
            let mut internals = world.resource_mut::<Internals>();
            for (part, (_, resources, components)) in internals.parts.iter_mut().zip(found) {
                part.resources = resources;
                part.components = components;
            }
            internals.registered = registered;
            internals.surveyed = 0.0;
        } else {
            if let Some(internals) = world.remove_resource::<Internals>() {
                world.entity_mut(internals.root).despawn();
            }
            let internals = spawn(world, found);
            world.insert_resource(internals);
        }
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
    let readings = read(world);
    let delta = readings.delta;
    let now = world.change_tick();

    world.resource_scope(|world, mut internals: Mut<Internals>| {
        let internals = &mut *internals;
        internals.surveyed += delta;
        if moving {
            internals.time += delta;
        }

        // What happened since last frame.
        let counts = changes(world, &internals.parts, internals.last, now);
        internals.last = now;
        let written: usize = counts.iter().sum();
        let mut started = Vec::new();
        for (at, (part, count)) in internals.parts.iter_mut().zip(counts).enumerate() {
            let count = count as f32;
            part.quiet += delta;
            part.glow *= (-delta * 4.0).exp();
            // More than its usual: something happened here.
            if count > part.usual + (part.usual * 0.25).max(0.5) {
                part.glow = 1.0;
                if part.quiet > PULSE_GAP {
                    part.quiet = 0.0;
                    started.push(at);
                }
            } else if count > 0.0 {
                // Its usual work: a steady low light.
                part.glow = part.glow.max(0.22);
            }
            part.usual += (count - part.usual) * 0.04;
        }
        for part in started {
            let entity = world
                .spawn((
                    ChildOf(internals.root),
                    internals.layers.clone(),
                    Mesh3d(internals.ball.clone()),
                    MeshMaterial3d(internals.pulse.clone()),
                    Transform::from_translation(internals.parts[part].route[0]),
                ))
                .id();
            internals.pulses.push(Pulse {
                entity,
                part,
                along: 0.0,
            });
        }
        internals.core_glow *= (-delta * 3.0).exp();
        let mut arrived = 0;
        internals.pulses.retain_mut(|pulse| {
            pulse.along += delta * PULSE_SPEED;
            let (at, done) = on_route(internals.parts[pulse.part].route, pulse.along);
            if done {
                world.entity_mut(pulse.entity).despawn();
                arrived += 1;
            } else {
                world
                    .get_mut::<Transform>(pulse.entity)
                    .unwrap()
                    .translation = at + Vec3::Y * 0.14;
            }
            !done
        });
        internals.core_glow = (internals.core_glow + arrived as f32 * 0.35).min(1.0);

        // The fan turns with how much is being written.
        internals.load += (written as f32 - internals.load) * (delta * 3.0).min(1.0);
        if moving {
            let speed = 1.2 + (internals.load / 40.0).min(1.0) * 26.0;
            world
                .get_mut::<Transform>(internals.fan)
                .unwrap()
                .rotate_y(-speed * delta);
        }

        // A card for each open panel, in the slots at the back.
        let panels = &readings.panels;
        internals.cards.retain(|card| {
            let open = panels.iter().any(|(panel, _, _)| *panel == card.panel);
            if !open {
                world.entity_mut(card.entity).despawn();
            }
            open
        });
        for (panel, _, _) in panels {
            if internals.cards.iter().any(|card| card.panel == *panel) {
                continue;
            }
            let material =
                world
                    .resource_mut::<Assets<StandardMaterial>>()
                    .add(StandardMaterial {
                        unlit: true,
                        ..default()
                    });
            let entity = world
                .spawn((
                    ChildOf(internals.root),
                    internals.layers.clone(),
                    Mesh3d(internals.cube.clone()),
                    MeshMaterial3d(material.clone()),
                    Transform::from_scale(Vec3::new(1.3, 1.25, 0.1)),
                ))
                .id();
            internals.cards.push(Card {
                panel: *panel,
                entity,
                material,
                shown: None,
            });
        }
        internals.cards.sort_by_key(|card| card.panel);
        for (slot, card) in internals.cards.iter().enumerate() {
            world.get_mut::<Transform>(card.entity).unwrap().translation = Vec3::new(
                (slot as f32 - (CARDS as f32 - 1.0) / 2.0) * 1.8,
                0.9,
                CARD_ROW,
            );
        }

        // The theme's colours, when they change.
        let restyle = internals.look.is_none_or(|(shown, _)| shown != colors);
        let mut materials = world.resource_mut::<Assets<StandardMaterial>>();
        if restyle {
            let background = linear(colors.background);
            let (text, accent) = (linear(colors.text), linear(colors.accent));
            let dark = background.luminance() < 0.2;
            let board = background.mix(&accent, 0.05);
            let chip = if dark {
                background.mix(&LinearRgba::BLACK, 0.45)
            } else {
                background.mix(&text, 0.75)
            };
            let look = Look {
                dim: board.mix(&linear(colors.muted), 0.25),
                lit: accent.mix(&text, 0.15),
                trace: board.mix(&accent, 0.22),
                chip,
            };
            // Metal keeps its own colour, a little of the theme's in it.
            let tin = LinearRgba::rgb(0.55, 0.57, 0.6).mix(&text, 0.15);
            let gold = LinearRgba::rgb(0.8, 0.5, 0.12).mix(&accent, 0.12);
            // In `Stuff`'s order.
            let stuffs = [
                chip,
                chip.mix(&board, 0.35),
                tin,
                gold,
                text.mix(&board, 0.45),
                board.mix(&accent, 0.09),
                chip.mix(&accent, 0.3),
                LinearRgba::rgb(0.45, 0.33, 0.2).mix(&board, 0.3),
            ];
            for (material, color) in internals.stuffs.iter().zip(stuffs) {
                paint(&mut materials, material, color);
            }
            paint(&mut materials, &internals.board, board);
            paint(&mut materials, &internals.pulse, look.lit.mix(&text, 0.4));
            paint(&mut materials, &internals.on, look.lit);
            paint(&mut materials, &internals.off, look.dim.mix(&chip, 0.5));
            let lettering = if dark { text } else { background };
            for label in &internals.labels {
                paint(&mut materials, label, lettering.mix(&chip, 0.25));
            }
            for label in &internals.printed {
                paint(&mut materials, label, text.mix(&board, 0.35));
            }
            internals.look = Some((colors, look));
            // Every graded light again, in the new colours.
            internals.core_shown = None;
            for part in &mut internals.parts {
                part.shown = None;
            }
            for card in &mut internals.cards {
                card.shown = None;
            }
        }
        let (_, look) = internals.look.unwrap();

        // The graded lights, where they changed.
        for part in &mut internals.parts {
            let level = step(part.glow);
            if part.shown != Some(level) {
                part.shown = Some(level);
                let level = level as f32 / STEPS;
                paint(&mut materials, &part.light, look.light(level));
                paint(
                    &mut materials,
                    &part.trace,
                    look.trace.mix(&look.lit, level * 0.8),
                );
            }
        }
        let level = step(internals.core_glow);
        if internals.core_shown != Some(level) {
            internals.core_shown = Some(level);
            paint(
                &mut materials,
                &internals.core,
                look.chip.mix(&look.lit, 0.12 + 0.6 * level as f32 / STEPS),
            );
        }
        for card in &mut internals.cards {
            let level = match panels.iter().find(|(panel, _, _)| *panel == card.panel) {
                Some((_, _, true)) => 16,
                Some((_, true, false)) => 6,
                _ => 1,
            };
            if card.shown != Some(level) {
                card.shown = Some(level);
                paint(
                    &mut materials,
                    &card.material,
                    look.light(level as f32 / STEPS),
                );
            }
        }

        // The lamps: each is one of two materials, on or off.
        let (on, off) = (internals.on.clone(), internals.off.clone());
        let switch = |world: &mut World, lamp: Entity, lit: bool| {
            let material = if lit { &on } else { &off };
            world
                .entity_mut(lamp)
                .insert(MeshMaterial3d(material.clone()));
        };
        // A lamp for each 0.4 ms of drawing.
        let lit = ((readings.draw_ms / 0.4).round() as usize).min(internals.draw.lamps.len());
        if internals.draw.shown != Some(lit) {
            internals.draw.shown = Some(lit);
            for (at, lamp) in internals.draw.lamps.iter().enumerate() {
                switch(world, *lamp, at < lit);
            }
        }
        if readings.elapsed > 0.0 {
            internals.frames += (1.0 / readings.elapsed - internals.frames) * 0.08;
        }
        let own = internals.own + (internals.pulses.len() + internals.cards.len()) as u32;
        let frames = internals.frames.round() as u32;
        for (display, value) in [
            (&mut internals.rate, frames),
            (
                &mut internals.entities,
                readings.entities.saturating_sub(own),
            ),
        ] {
            if display.shown == Some(value) {
                continue;
            }
            display.shown = Some(value);
            // The last digit first; leading places stay dark.
            let mut left = value.min(10u32.pow(display.digits.len() as u32) - 1);
            for (place, bars) in display.digits.iter().rev().enumerate() {
                let lit = if place == 0 || left > 0 {
                    DIGITS[(left % 10) as usize]
                } else {
                    0
                };
                left /= 10;
                for (bar, lamp) in bars.iter().enumerate() {
                    switch(world, *lamp, lit & (1 << bar) != 0);
                }
            }
        }
        internals.still = if readings.pointer {
            0.0
        } else {
            internals.still + delta
        };
        let lamps = [
            true,
            readings.focused,
            readings.key,
            internals.still < 0.15,
        ];
        if internals.lamps_shown != Some(lamps) {
            internals.lamps_shown = Some(lamps);
            for (lamp, lit) in internals.lamps.iter().zip(lamps) {
                switch(world, *lamp, lit);
            }
        }

        // The camera looks down on the board from the front, far enough back
        // to see all of it whatever the window's shape, and drifts a little.
        let size = world
            .resource::<Assets<Image>>()
            .get(&internals.image)
            .map_or(Vec2::ONE, |image| image.size().as_vec2());
        let reach = (FOV / 2.0).tan();
        let distance =
            (VIEW.x / (reach * size.x / size.y.max(1.0))).max(VIEW.y / reach) + 4.0;
        let turn = (internals.time * 0.07).sin() * 0.1;
        let eye = Quat::from_rotation_y(turn) * Vec3::new(0.0, 0.86, 0.5) * distance;
        *world.get_mut::<Transform>(internals.camera).unwrap() =
            Transform::from_translation(eye).looking_at(Vec3::new(0.0, 0.0, 1.4), Vec3::Y);
        world
            .get_mut::<Camera>(internals.camera)
            .unwrap()
            .clear_color = Color::from(linear(colors.background)).into();
    });
    if moving {
        request_frame(world);
    }
}
