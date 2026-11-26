//! A cat that lives in the app's window (`CatPlugin`): it wanders along the
//! bottom edge, sits, washes, naps, and plays with the pointer when clicked.
//! It's on unless turned off in Settings (Window).
//!
//! It is a skinned model (`rig`) posed by code (`moves`) as its `mind`
//! decides. A camera of its own draws it into a small transparent image, and
//! a view on the window's surface paints that image over everything else, so
//! the screen effects bend it with the rest of the UI.
mod mind;
mod moves;
mod paw;
mod rig;

use bedtime_ui::{
    InSurface, UiContext, UiSurface, egui, layouts::LayoutSurface, register_view, viewport_image,
};
use bedtime_workbench::BedtimeWorldExt as _;
use bedtime_workbench::settings::{SettingsAppExt, settings, ui::row};
use bevy::{
    asset::{AssetPath, embedded_asset, embedded_path},
    camera::{RenderTarget, ScalingMode},
    core_pipeline::{
        fullscreen_material::{FullscreenMaterial, FullscreenMaterialPlugin},
        tonemapping::{DebandDither, Tonemapping},
    },
    prelude::*,
    render::{
        RenderApp, extract_component::ExtractComponent, render_resource::Extent3d,
        render_resource::ShaderType,
    },
    shader::ShaderRef,
};
use bevy_egui::{EguiTextureHandle, EguiUserTextures};
use mind::{Mark, Mind, Senses};
use rig::{Model, Pose, Rig};
use std::time::Duration;

settings! {
    "Window" {
        pub ShowCat: bool {
            default: false,
            key: "appearance.cat",
            label: "kitty!",
            description: "meow",
            apply: |world, show| world.resource_mut::<Companion>().enabled = *show,
        }
        /// The colour of the cat's fur, as `#rrggbb`. Empty is its own grey.
        pub CatColor: String {
            default: String::new(),
            key: "appearance.cat_color",
            label: "kitty color",
            description: "The colour of its fur. Reset it for the cat's own grey.",
            ui: custom(color_row),
            visible: |world, layer| world.bedtime().settings().get_at::<ShowCat>(layer),
            apply: |world, color| world.resource_mut::<Companion>().color = parse_color(color),
        }
    }
}

/// The cat's own fur, about: what the colour picker shows before one is chosen.
const GREY: [u8; 3] = [96, 96, 102];

fn parse_color(hex: &str) -> Option<[u8; 3]> {
    let hex = hex.trim().strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let channel = |at: usize| u8::from_str_radix(hex.get(at..at + 2)?, 16).ok();
    Some([channel(0)?, channel(2)?, channel(4)?])
}

fn color_row(
    ui: &mut egui::Ui,
    world: &World,
    context: &UiContext<'_>,
    layer: bedtime_workbench::persistence::StoreId,
) {
    row::<CatColor>(ui, world, context, layer, None, |ui, chosen| {
        let mut color = parse_color(chosen).unwrap_or(GREY);
        let changed = ui.color_edit_button_srgb(&mut color).changed();
        if changed {
            *chosen = format!("#{:02x}{:02x}{:02x}", color[0], color[1], color[2]);
        }
        changed
    });
}

/// The part of the scene one cat is drawn in, in the model's units: the cat
/// is about 20 high and, tail out, 43 long.
const CELL: Vec2 = Vec2::new(64.0, 46.0);
/// How far above the cell's bottom edge the ground is.
const GROUND: f32 = 3.0;
/// Window points per model unit.
const SCALE: f32 = 4.6;
/// The camera looks a little down on the cat.
const PITCH: f32 = 0.14;

pub struct CatPlugin;

impl Plugin for CatPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Companion>()
            .register_setting::<ShowCat>()
            .register_setting::<CatColor>()
            .add_systems(Update, (attach, update).chain());
        // Headless apps (tests) have no renderer.
        if app.get_sub_app(RenderApp).is_some() {
            embedded_asset!(app, "premultiply.wesl");
            app.add_plugins(FullscreenMaterialPlugin::<Premultiply>::default());
        }
    }
}

/// On the cat's camera: its image is left premultiplied the way egui paints
/// it, so a partly covered pixel (a whisker's edge) isn't drawn too light.
/// See `premultiply.wesl`.
#[derive(Component, ExtractComponent, Clone, Copy, Default, ShaderType)]
#[extract_app(RenderApp)]
struct Premultiply {
    unused: f32,
}

impl FullscreenMaterial for Premultiply {
    fn fragment_shader() -> ShaderRef {
        AssetPath::from_path_buf(embedded_path!("premultiply.wesl"))
            .with_source("embedded")
            .into()
    }
}

/// The cat, while it is shown.
#[derive(Resource, Default)]
pub struct Companion {
    enabled: bool,
    /// The fur's colour, when it isn't the cat's own, and the one the fur
    /// was last given.
    color: Option<[u8; 3]>,
    tinted: Option<Option<[u8; 3]>>,
    /// The model didn't load: said once, not tried again.
    failed: bool,
    model: Option<Model>,
    stage: Option<Stage>,
    mind: Option<Mind>,
    /// What the last draw saw.
    seen: Seen,
    /// The box around the cat's body in the last pose shown: its near-ground
    /// corner and its far one, in points from its middle on the ground.
    body: (Vec2, Vec2),
}

#[derive(Default, Clone, Copy)]
struct Seen {
    /// The image's size in pixels, for its size in points.
    pixels: UVec2,
    senses: Senses,
    /// Animations are off (`MotionPolicy`): the cat keeps still.
    still: bool,
}

/// The cat's own corner of the world: the camera, and what it draws into.
struct Stage {
    root: Entity,
    image: Handle<Image>,
    rigs: Vec<Rig>,
    /// Cats across and down: one, unless this is a sheet of poses.
    grid: UVec2,
    view: Quat,
}

impl Stage {
    fn spawn(world: &mut World, model: &Model, grid: UVec2) -> Self {
        let layers = bedtime_ui::free_layer(world);
        let image = world
            .resource_mut::<Assets<Image>>()
            .add(viewport_image(UVec2::new(8, 8)));
        let root = world
            .spawn((Transform::IDENTITY, Visibility::Inherited, layers.clone()))
            .id();
        let view = Quat::from_rotation_x(-PITCH);
        let size = CELL * grid.as_vec2();
        world.spawn((
            ChildOf(root),
            layers.clone(),
            Camera3d::default(),
            Camera {
                order: -1,
                clear_color: Color::NONE.into(),
                ..default()
            },
            RenderTarget::Image(image.clone().into()),
            Msaa::Sample4,
            Tonemapping::None,
            DebandDither::Disabled,
            Premultiply::default(),
            AmbientLight {
                brightness: 900.0,
                ..default()
            },
            Projection::Orthographic(OrthographicProjection {
                scaling_mode: ScalingMode::Fixed {
                    width: size.x,
                    height: size.y,
                },
                near: 0.0,
                far: 1000.0,
                ..OrthographicProjection::default_3d()
            }),
            Transform::from_translation(view * Vec3::Z * 500.0).with_rotation(view),
        ));
        // A light over the viewer's shoulder, and one from behind that picks
        // out the cat's back against a dark window.
        for (illuminance, from) in [
            (5200.0, Vec3::new(-4.0, 6.0, 8.0)),
            (9000.0, Vec3::new(3.0, 9.0, -7.0)),
        ] {
            world.spawn((
                ChildOf(root),
                layers.clone(),
                DirectionalLight {
                    illuminance,
                    shadow_maps_enabled: false,
                    ..default()
                },
                Transform::from_translation(from).looking_at(Vec3::ZERO, Vec3::Y),
            ));
        }
        let rigs = (0..grid.x * grid.y)
            .map(|_| Rig::spawn(world, model, root, &layers))
            .collect();
        Self {
            root,
            image,
            rigs,
            grid,
            view,
        }
    }

    /// Where cat `index` stands: the middle of its cell's ground line.
    fn ground(&self, index: usize) -> Vec3 {
        let cell = UVec2::new(index as u32 % self.grid.x, index as u32 / self.grid.x).as_vec2();
        let middle = (cell + 0.5 - self.grid.as_vec2() / 2.0) * CELL;
        self.view * Vec3::new(middle.x, -middle.y - CELL.y / 2.0 + GROUND, 0.0)
    }
}

/// A name from `BED_CAT_SHEET`: show a grid of poses instead of the cat, to
/// look at a move's frames side by side.
fn sheet_name() -> Option<String> {
    std::env::var("BED_CAT_SHEET")
        .ok()
        .filter(|name| !name.is_empty())
}

/// `BED_CAT_SPEED`: how many times faster than life the cat lives, to watch
/// a long while of it in a short one.
fn hurry() -> f32 {
    static SPEED: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
    *SPEED.get_or_init(|| {
        std::env::var("BED_CAT_SPEED")
            .ok()
            .and_then(|speed| speed.parse().ok())
            .unwrap_or(1.0)
    })
}

/// One cat of a sheet: its pose, the way it faces, and whether to look
/// closely at its head.
struct Shown {
    pose: Pose,
    facing: f32,
    close: bool,
}

/// A sheet's cats: a move's frames, or (`name@degrees`) seen from an angle.
fn sheet(model: &Model, name: &str) -> Vec<Shown> {
    use moves::Puppet;
    let (name, facing) = match name.split_once('@') {
        Some((name, degrees)) => (name, degrees.parse::<f32>().unwrap_or(0.0).to_radians()),
        None => (name, 0.0),
    };
    fn face<'a>(
        model: &'a Model,
        edit: impl Fn(&mut Puppet, f32) + 'a,
    ) -> Box<dyn Fn(f32) -> Pose + 'a> {
        Box::new(move |u| {
            let mut p = Puppet::new(model, moves::sit(model, 1.0));
            edit(&mut p, u);
            p.pose
        })
    }
    // `close:<move>` looks closely at any move's head.
    let (name, near) = match name.strip_prefix("close:") {
        Some(name) => (name, true),
        None => (name, false),
    };
    let close = near || matches!(name, "eyes" | "ears" | "jaw" | "flick" | "look" | "face");
    let frame: Box<dyn Fn(f32) -> Pose> = match name {
        "stand" => Box::new(|_| moves::stand(model)),
        "poses" => Box::new(|u| match (u * 9.0).round() as i32 {
            0 => moves::stand(model),
            1 => moves::sit(model, 1.0),
            2 => moves::sleep(model, 1.0),
            3 => moves::crouch(model, 0.08),
            4 => moves::stretch(model, 0.5),
            5 => moves::dangle(model, 0.0, 0.0),
            6 => moves::bat(model, true, Vec2::new(14.0, 24.0)),
            7 => moves::bat(model, true, Vec2::new(17.0, 9.0)),
            _ => moves::bat(model, false, Vec2::new(6.0, 27.0)),
        }),
        "face" => face(model, |p, u| match (u * 9.0).round() as i32 {
            0 => {}
            1 => p.close_eyes(0.5),
            2 => p.close_eyes(1.0),
            3 => p.ears(-1.0),
            4 => p.ears(1.0),
            5 => p.flick(true, 1.0),
            6 => p.gape(0.4),
            7 => p.gape(1.0),
            _ => p.look(-60.0, 30.0),
        }),
        "walk" => Box::new(|u| moves::walk(model, false, u, 1.0)),
        "trot" => Box::new(|u| moves::walk(model, true, u, 1.0)),
        "sit" => Box::new(|_| moves::sit(model, 1.0)),
        // Each move as a cat seen from its left has it.
        "left" => Box::new(|u| match (u * 9.0).round() as i32 {
            0 => moves::sit(model, -1.0),
            1 => moves::loaf(model, -1.0),
            2 => moves::sleep(model, -1.0),
            3 => moves::bat(model, false, Vec2::new(14.0, 24.0)),
            _ => moves::groom(
                model,
                (u * 9.0 - 3.0) / 6.0 * model.clips.groom.length,
                -1.0,
            ),
        }),
        "loaf" => Box::new(|_| moves::loaf(model, 1.0)),
        "sleep" => Box::new(|_| moves::sleep(model, 1.0)),
        "stretch" => Box::new(|u| moves::stretch(model, u)),
        "crouch" => Box::new(|u| moves::crouch(model, u * model.clips.crouch.length)),
        "leap" => Box::new(|u| moves::leap(model, u * 9.0 / 8.0)),
        "bat" => Box::new(|u| {
            let to = Vec2::from_angle(-0.6 + u * 2.4) * 11.0 + Vec2::new(4.0, 13.0);
            moves::bat(model, true, to)
        }),
        "swat" => Box::new(|u| {
            let reach = moves::Reach::new(model, true);
            let to = reach
                .home()
                .lerp(reach.swing(Vec2::new(6.0, 30.0)), u * 9.0 / 8.0);
            moves::bat(model, true, reach.point(to))
        }),
        // A swat as the cat makes it, a frame every 35ms: the first from
        // sitting, `swipe2` the next (from a paw held up), `swipe3` at a
        // pointer overhead.
        "swipe" | "swipe2" | "swipe3" => {
            let reach = moves::Reach::new(model, true);
            let target = if name == "swipe3" {
                Vec2::new(7.0, 23.0)
            } else {
                Vec2::new(15.0, 12.0)
            };
            let mut paw = paw::Paw::new(5);
            paw.rest(&reach, 0.0);
            let step = |paw: &mut paw::Paw| {
                moves::bat(model, true, paw.step(&reach, Some(target), 0.0, 0.005))
            };
            if name == "swipe2" {
                // On to the second time it draws its paw back.
                let mut winds = 0;
                let mut before = paw.stage;
                for _ in 0..2000 {
                    step(&mut paw);
                    winds +=
                        usize::from(paw.stage == paw::Stage::Wind && before != paw::Stage::Wind);
                    before = paw.stage;
                    if winds == 2 {
                        break;
                    }
                }
            }
            let frames: Vec<Pose> = (0..9)
                .map(|_| (0..7).map(|_| step(&mut paw)).last().unwrap())
                .collect();
            Box::new(move |u| frames[((u * 9.0).round() as usize).min(8)].clone())
        }
        "dangle" => Box::new(|u| moves::dangle(model, 0.0, u * 0.6)),
        "eyes" => face(model, |p, u| p.close_eyes(u * 9.0 / 8.0)),
        "ears" => face(model, |p, u| p.ears(u * 9.0 / 4.0 - 1.0)),
        "flick" => face(model, |p, u| p.flick(u < 0.5, (u * 4.0).fract())),
        "jaw" => face(model, |p, u| p.gape(u * 9.0 / 8.0)),
        "look" => face(model, |p, u| {
            p.look(
                ((u * 9.0) as i32 % 3 - 1) as f32 * 60.0,
                ((u * 9.0) as i32 / 3 - 1) as f32 * 35.0,
            )
        }),
        _ => Box::new(|u| moves::groom(model, u * model.clips.groom.length, 1.0)),
    };
    (0..9)
        .map(|index| Shown {
            pose: frame(index as f32 / 9.0),
            facing: if name == "turn" {
                index as f32 * 0.7
            } else {
                facing
            },
            close,
        })
        .collect()
}

/// Put the cat's view on each layout surface.
fn attach(world: &mut World) {
    #[derive(Component)]
    struct CatView;
    let surfaces: Vec<Entity> = world
        .query_filtered::<Entity, (With<LayoutSurface>, With<UiSurface>)>()
        .iter(world)
        .collect();
    let viewed: Vec<Entity> = world
        .query_filtered::<&InSurface, With<CatView>>()
        .iter(world)
        .map(|surface| surface.0)
        .collect();
    for surface in surfaces {
        if !viewed.contains(&surface) {
            let view = register_view(world, surface, draw);
            world.entity_mut(view).insert(CatView);
        }
    }
}

fn update(world: &mut World) {
    world.resource_scope(|world, mut cat: Mut<Companion>| {
        let cat = &mut *cat;
        if !cat.enabled || cat.failed {
            if let Some(stage) = cat.stage.take() {
                world.entity_mut(stage.root).despawn();
                world.resource_mut::<Assets<Image>>().remove(&stage.image);
            }
            cat.mind = None;
            return;
        }
        if cat.model.is_none() {
            let began = std::time::Instant::now();
            match Model::load(world) {
                Ok(model) => {
                    debug!("The cat loaded in {:?}", began.elapsed());
                    cat.model = Some(model);
                }
                Err(error) => {
                    error!("{error}");
                    cat.failed = true;
                    return;
                }
            }
        }
        let model = cat.model.as_ref().unwrap();
        if cat.tinted != Some(cat.color) {
            model.tint(world, cat.color);
            cat.tinted = Some(cat.color);
        }
        let sheet_name = sheet_name();
        let stage = cat.stage.get_or_insert_with(|| {
            let grid = if sheet_name.is_some() {
                UVec2::splat(3)
            } else {
                UVec2::ONE
            };
            Stage::spawn(world, model, grid)
        });
        // The image, at the size it is drawn.
        let pixels = cat.seen.pixels.max(UVec2::splat(8));
        let mut images = world.resource_mut::<Assets<Image>>();
        if images
            .get(&stage.image)
            .is_some_and(|image| image.size() != pixels)
        {
            images.get_mut(&stage.image).unwrap().resize(Extent3d {
                width: pixels.x,
                height: pixels.y,
                depth_or_array_layers: 1,
            });
        }
        let shown = match &sheet_name {
            Some(name) => sheet(model, name),
            None => {
                let mind = cat.mind.get_or_insert_with(|| {
                    let seed = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(1, |since| since.as_nanos() as u64);
                    Mind::new(seed)
                });
                // A long pause (the window was idle) isn't a leap forward.
                let moving = if cat.seen.still { 0.0 } else { 1.0 };
                let senses = Senses {
                    dt: world.resource::<Time<Real>>().delta_secs().min(0.1) * hurry() * moving,
                    ..cat.seen.senses
                };
                cat.seen.senses.clicked = false;
                if senses.width <= 0.0 {
                    return;
                }
                let pose = mind.think(model, &senses);
                vec![Shown {
                    pose,
                    facing: mind.facing,
                    close: false,
                }]
            }
        };
        for (index, (rig, shown)) in stage.rigs.iter().zip(&shown).enumerate() {
            let globals = model.globals(&shown.pose);
            let turn = Quat::from_rotation_y(shown.facing);
            let [(low, _), body] = model.extent(&globals, turn);
            // The point of the cat that is at its place along the window:
            // a little ahead of its hips.
            let middle = globals[model.bones.hips].w_axis.truncate() + Vec3::X * 7.2;
            let turned = turn * middle;
            let offset = Vec3::new(turned.x, low.y, turned.z);
            let mut root =
                Transform::from_translation(stage.ground(index) - offset).with_rotation(turn);
            if shown.close {
                let head = turn * globals[model.bones.head].w_axis.truncate();
                let centre = stage.ground(index) + stage.view * Vec3::Y * (CELL.y / 2.0 - GROUND);
                root = Transform::from_translation(centre - head * 3.4 - Vec3::X * 6.0)
                    .with_rotation(turn)
                    .with_scale(Vec3::splat(3.4));
            }
            if let Some(mut transform) = world.get_mut::<Transform>(rig.root) {
                *transform = root;
            }
            rig.show(world, model, &globals);
            if let Some(mind) = &mut cat.mind {
                let head = turn * globals[model.bones.head].w_axis.truncate() - offset;
                mind.head = head.truncate() * SCALE;
                mind.middle = middle;
                mind.floor = low.y;
                cat.body = (
                    (body.0 - offset).truncate() * SCALE,
                    (body.1 - offset).truncate() * SCALE,
                );
            }
        }
    });
}

/// Paint the cat over the window, and tell it what the pointer is doing.
fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>) {
    let cat = world.resource::<Companion>();
    let Some(stage) = &cat.stage else {
        return;
    };
    let (image, sheet) = (stage.image.id(), stage.grid != UVec2::ONE);
    let ctx = ui.ctx().clone();
    let window = ctx.content_rect();
    // Over the panels, under menus and tooltips.
    let layer = egui::LayerId::new(egui::Order::Middle, egui::Id::new("cat"));
    let painter = ctx.layer_painter(layer);
    let whole = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    if sheet {
        let pixels = window.size() * ctx.pixels_per_point();
        world.resource_mut::<Companion>().seen.pixels =
            UVec2::new(pixels.x.ceil() as u32, pixels.y.ceil() as u32);
        let texture = world
            .resource_mut::<EguiUserTextures>()
            .add_image(EguiTextureHandle::Weak(image));
        painter.rect_filled(window, 0.0, egui::Color32::from_gray(70));
        painter.image(texture, window, whole, egui::Color32::WHITE);
        ctx.request_repaint_after(Duration::from_millis(50));
        return;
    }
    let size = egui::vec2(CELL.x, CELL.y) * SCALE;
    let pixels = size * ctx.pixels_per_point();
    let pixels = UVec2::new(pixels.x.ceil() as u32, pixels.y.ceil() as u32);
    let ground = window.bottom();
    let to_cat = |at: egui::Pos2| Vec2::new(at.x - window.left(), ground - at.y);
    let from_cat = |at: Vec2| egui::pos2(window.left() + at.x, ground - at.y);
    let mut senses = Senses {
        width: window.width(),
        pointer: ctx.pointer_latest_pos().map(to_cat),
        // A click waits for the cat to hear of it.
        clicked: cat.seen.senses.clicked,
        ..default()
    };
    let still = !context.motion.enabled;
    let Some(mind) = &cat.mind else {
        let seen = &mut world.resource_mut::<Companion>().seen;
        *seen = Seen {
            pixels,
            senses,
            still,
        };
        ctx.request_repaint();
        return;
    };
    let at = Vec2::new(mind.x, mind.lift);
    let rect = egui::Rect::from_min_size(
        from_cat(at) + egui::vec2(-size.x / 2.0, GROUND * SCALE - size.y),
        size,
    );
    let body = egui::Rect::from_min_max(
        from_cat(at + Vec2::new(cat.body.0.x, cat.body.1.y)),
        from_cat(at + Vec2::new(cat.body.1.x, cat.body.0.y)),
    )
    .intersect(window);
    let frame = Duration::from_secs_f32(1.0 / mind.frame_rate());
    let signs = mind.signs.clone();

    // The cat's body takes the pointer; the rest of its image doesn't.
    if body.is_positive() {
        let response = egui::Area::new(layer.id)
            .order(layer.order)
            .fixed_pos(body.min)
            .constrain(false)
            .movable(false)
            .show(&ctx, |ui| {
                ui.allocate_response(body.size(), egui::Sense::click_and_drag())
            })
            .inner;
        // Scripts find it by this name.
        response
            .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "The cat"));
        senses.over = response.hovered();
        senses.clicked |= response.clicked();
        senses.held = response.dragged();
    }

    let texture = world
        .resource_mut::<EguiUserTextures>()
        .add_image(EguiTextureHandle::Weak(image));
    painter.image(texture, rect, whole, egui::Color32::WHITE);
    let palette = &context.theme.palette;
    for sign in signs {
        let fade = (1.0 - sign.age / sign.life)
            .clamp(0.0, 1.0)
            .min(sign.age * 6.0);
        let at = from_cat(sign.at);
        match sign.mark {
            Mark::Z => {
                painter.text(
                    at,
                    egui::Align2::CENTER_CENTER,
                    "z",
                    egui::FontId::proportional(9.0 + sign.age * 4.0),
                    bedtime_ui::theme::color(palette.text).gamma_multiply(fade),
                );
            }
            Mark::Bang => {
                painter.text(
                    at,
                    egui::Align2::CENTER_CENTER,
                    "!",
                    egui::FontId::proportional(15.0),
                    bedtime_ui::theme::color(palette.accent).gamma_multiply(fade),
                );
            }
            Mark::Heart => {
                // One shape, so it fades as one: a fan about its middle.
                let color = egui::Color32::from_rgb(236, 88, 118).gamma_multiply(fade);
                let size = 3.2 + sign.age.min(1.0);
                let mut mesh = egui::Mesh::default();
                mesh.colored_vertex(at, color);
                const POINTS: u32 = 40;
                for point in 0..=POINTS {
                    let t = point as f32 / POINTS as f32 * std::f32::consts::TAU;
                    let x = t.sin().powi(3);
                    let y = (13.0 * t.cos()
                        - 5.0 * (2.0 * t).cos()
                        - 2.0 * (3.0 * t).cos()
                        - (4.0 * t).cos())
                        / 16.0;
                    mesh.colored_vertex(at + egui::vec2(x, -y) * size, color);
                    if point > 0 {
                        mesh.add_triangle(0, point, point + 1);
                    }
                }
                painter.add(egui::Shape::mesh(mesh));
            }
        }
    }
    world.resource_mut::<Companion>().seen = Seen {
        pixels,
        senses,
        still,
    };
    if !still {
        ctx.request_repaint_after(frame);
    }
}
