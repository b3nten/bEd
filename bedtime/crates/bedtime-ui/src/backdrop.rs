//! What a surface shows behind its UI, in place of the theme's background
//! colour: a fragment shader, or any image a camera draws into.
//!
//! Put a [`SurfaceBackdrop`] on a UI surface. Both kinds end up as an image
//! the surface paints first, under everything else, so whatever is done to the
//! finished UI (post-processing, screenshots) includes it.
use crate::{MotionPolicy, Theme, theme::Rgba, viewport_image};
use bevy::{
    camera::{RenderTarget, visibility::RenderLayers},
    core_pipeline::{Core2d, upscaling::upscaling},
    ecs::{lifecycle::HookContext, world::DeferredWorld},
    platform::collections::{HashMap, HashSet},
    prelude::*,
    render::{
        Render, RenderApp, RenderStartup, RenderSystems,
        extract_component::{
            ComponentUniforms, DynamicUniformIndex, ExtractComponent, ExtractComponentPlugin,
            UniformComponentPlugin,
        },
        render_resource::{binding_types::uniform_buffer, *},
        renderer::{RenderContext, ViewQuery},
        view::ViewTarget,
    },
    shader::{Shader, ShaderCacheError},
};
use bevy_egui::{EguiTextureHandle, EguiUserTextures, render::egui_pass};
use std::sync::{Arc, Mutex};

/// What a UI surface shows instead of its theme's background colour.
#[derive(Component, Clone, Debug, PartialEq)]
pub enum SurfaceBackdrop {
    /// An image something else draws: a camera's render target, say (make it
    /// with [`viewport_image`]). It is resized to the surface, in pixels.
    Target(Handle<Image>),
    /// A fragment shader, in WGSL, defining
    /// `fn backdrop(uv: vec2<f32>, pixel: vec2<f32>) -> vec4<f32>`. It can
    /// read `ui`: the time, the window's size, the pointer and the theme's
    /// colours (see `backdrop.wgsl`, which is added after it).
    Shader {
        source: String,
        /// How often to redraw, per second, for a shader that moves. Zero
        /// draws it only when the UI redraws anyway.
        frame_rate: f32,
    },
}

/// How a surface's [`SurfaceBackdrop`] is doing. bedtime-ui keeps it on the
/// surface. Until it is `Ready`, the surface shows its background colour.
#[derive(Component, Clone, Debug, Default, PartialEq)]
pub enum BackdropStatus {
    /// A shader that hasn't compiled yet.
    #[default]
    Pending,
    Ready,
    /// Why a shader didn't compile.
    Failed(String),
}

/// What to fill a surface's window areas (titlebar, gaps, status bar) with:
/// the theme's background, or nothing where a backdrop shows instead.
pub fn window_fill(world: &World, surface: Entity, theme: &Theme) -> egui::Color32 {
    if world.get::<BackdropStatus>(surface) == Some(&BackdropStatus::Ready) {
        egui::Color32::TRANSPARENT
    } else {
        crate::theme::color(theme.palette.background)
    }
}

/// A backdrop's image, and the size its surface was last drawn at, in pixels:
/// the image is resized to it (`viewport::resize_targets`).
#[derive(Component)]
pub(crate) struct BackdropImage {
    pub image: Handle<Image>,
    pub size: UVec2,
}

impl BackdropImage {
    fn new(image: Handle<Image>) -> Self {
        Self {
            image,
            size: UVec2::ONE,
        }
    }
}

/// The backdrop a surface was last set up for, and the entity drawing it: its
/// `BackdropImage` and, for a shader, the camera rendering it.
#[derive(Component)]
#[component(on_discard = despawn_drawing)]
struct BackdropState {
    drawing: Entity,
    backdrop: SurfaceBackdrop,
    shader: Option<AssetId<Shader>>,
}

fn despawn_drawing(mut world: DeferredWorld, context: HookContext) {
    let drawing = world.get::<BackdropState>(context.entity).unwrap().drawing;
    world.commands().entity(drawing).try_despawn();
}

/// A camera drawing this shader over its whole target.
#[derive(Component, Clone, ExtractComponent)]
#[extract_app(RenderApp)]
struct BackdropPass(Handle<Shader>);

/// `Ui` in `backdrop.wgsl`.
#[derive(Component, Clone, Copy, Default, ShaderType, ExtractComponent)]
#[extract_app(RenderApp)]
struct BackdropUniform {
    resolution: Vec2,
    pointer: Vec2,
    time: f32,
    delta: f32,
    scale: f32,
    frame: f32,
    background: Vec4,
    panel: Vec4,
    text: Vec4,
    muted: Vec4,
    accent: Vec4,
}

/// Whether each shader compiled, from the renderer to `sync`.
#[derive(Resource, Clone, Default)]
struct Reports(Arc<Mutex<Vec<(AssetId<Shader>, Result<(), String>)>>>);

pub(crate) fn install(app: &mut App) {
    let reports = Reports::default();
    app.insert_resource(reports.clone())
        .add_plugins((
            ExtractComponentPlugin::<BackdropPass>::default(),
            ExtractComponentPlugin::<BackdropUniform>::default(),
            UniformComponentPlugin::<BackdropUniform>::default(),
        ))
        .add_systems(PreUpdate, sync.before(crate::UiSystems::Draw));
    app.sub_app_mut(RenderApp)
        .insert_resource(reports)
        .add_systems(RenderStartup, prepare_layout)
        .add_systems(Render, queue_pipelines.in_set(RenderSystems::Queue))
        .add_systems(Core2d, render.after(egui_pass).before(upscaling));
}

/// A shader's whole module: its own source first, so a compile error's line
/// numbers are the file's.
fn module(source: &str) -> String {
    format!("{source}\n{}", include_str!("backdrop.wgsl"))
}

type Surfaces<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        Option<&'static SurfaceBackdrop>,
        Option<&'static BackdropState>,
        Option<&'static mut BackdropStatus>,
    ),
    Or<(With<SurfaceBackdrop>, With<BackdropState>)>,
>;

/// Set up, replace and take down what draws each surface's backdrop, and
/// record whether its shader compiled.
fn sync(
    mut commands: Commands,
    mut surfaces: Surfaces,
    mut images: ResMut<Assets<Image>>,
    mut shaders: ResMut<Assets<Shader>>,
    reports: Res<Reports>,
    mut compiled: Local<u64>,
) {
    let reports = std::mem::take(&mut *reports.0.lock().unwrap());
    for (surface, backdrop, state, status) in &mut surfaces {
        let Some(backdrop) = backdrop else {
            commands
                .entity(surface)
                .remove::<(BackdropState, BackdropStatus)>();
            continue;
        };
        if let Some(state) = state.filter(|state| state.backdrop == *backdrop) {
            if let (Some(mut status), Some((_, result))) = (
                status,
                reports
                    .iter()
                    .rev()
                    .find(|(id, _)| Some(*id) == state.shader),
            ) {
                let reported = match result {
                    Ok(()) => BackdropStatus::Ready,
                    Err(error) => BackdropStatus::Failed(error.clone()),
                };
                if *status != reported {
                    *status = reported;
                }
            }
            continue;
        }
        let (drawing, shader, status) = match backdrop {
            SurfaceBackdrop::Target(image) => (
                commands.spawn(BackdropImage::new(image.clone())).id(),
                None,
                BackdropStatus::Ready,
            ),
            SurfaceBackdrop::Shader { source, .. } => {
                *compiled += 1;
                let shader = shaders.add(Shader::from_wgsl(
                    module(&source),
                    format!("backdrop://{surface}/{}.wgsl", *compiled),
                ));
                let image = images.add(viewport_image(UVec2::ONE));
                let drawing = commands
                    .spawn((
                        BackdropImage::new(image.clone()),
                        Camera2d,
                        Camera {
                            order: -1,
                            clear_color: Color::NONE.into(),
                            ..default()
                        },
                        RenderTarget::Image(image.into()),
                        // Nothing in the world is this camera's to draw.
                        RenderLayers::none(),
                        Msaa::Off,
                        BackdropUniform::default(),
                        BackdropPass(shader.clone()),
                    ))
                    .id();
                (drawing, Some(shader.id()), BackdropStatus::Pending)
            }
        };
        commands.entity(surface).insert((
            BackdropState {
                drawing,
                backdrop: backdrop.clone(),
                shader,
            },
            status,
        ));
    }
}

fn linear(color: Rgba) -> Vec4 {
    let [r, g, b, a] = color;
    let linear = Color::srgba(r, g, b, a).to_linear();
    Vec4::new(linear.red, linear.green, linear.blue, linear.alpha)
}

/// Paint the surface's backdrop over `ui`'s whole rectangle, and tell what
/// draws it how large that is and what a shader can read this frame.
pub(crate) fn paint(
    ui: &mut egui::Ui,
    world: &mut World,
    surface: Entity,
    theme: &Theme,
    motion: MotionPolicy,
) {
    let Some(state) = world.get::<BackdropState>(surface) else {
        return;
    };
    let drawing = state.drawing;
    let frame_rate = match state.backdrop {
        SurfaceBackdrop::Shader { frame_rate, .. } => Some(frame_rate),
        SurfaceBackdrop::Target(_) => None,
    };
    let rect = ui.max_rect();
    if !rect.is_positive() {
        return;
    }
    let scale = ui.ctx().pixels_per_point();
    let size = (Vec2::new(rect.width(), rect.height()) * scale).ceil();
    let Some(mut drawn) = world.get_mut::<BackdropImage>(drawing) else {
        return;
    };
    let pixels = size.as_uvec2().max(UVec2::ONE);
    if drawn.size != pixels {
        drawn.size = pixels;
    }
    let image = drawn.image.id();
    if let Some(frame_rate) = frame_rate {
        let animated = motion.enabled && frame_rate > 0.0;
        let time = world.get_resource::<Time<Real>>();
        let pointer = ui.ctx().pointer_latest_pos().filter(|p| rect.contains(*p));
        let palette = &theme.palette;
        let frame = world
            .get::<BackdropUniform>(drawing)
            .map_or(0.0, |uniform| uniform.frame + 1.0);
        let uniform = BackdropUniform {
            resolution: size,
            pointer: pointer.map_or(Vec2::NEG_ONE, |p| {
                Vec2::new(p.x - rect.min.x, p.y - rect.min.y) * scale
            }),
            time: time
                .filter(|_| animated)
                .map_or(0.0, |time| time.elapsed_secs_wrapped()),
            delta: time.map_or(0.0, |time| time.delta_secs()),
            scale,
            frame,
            background: linear(palette.background),
            panel: linear(palette.panel_background),
            text: linear(palette.text),
            muted: linear(palette.muted),
            accent: linear(palette.accent),
        };
        world.entity_mut(drawing).insert(uniform);
        if animated {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs_f32(1.0 / frame_rate));
        }
    }
    if world.get::<BackdropStatus>(surface) != Some(&BackdropStatus::Ready) {
        return;
    }
    let texture = world
        .resource_mut::<EguiUserTextures>()
        .add_image(EguiTextureHandle::Weak(image));
    egui::Image::new(egui::load::SizedTexture::new(texture, rect.size())).paint_at(ui, rect);
}

#[derive(Resource)]
struct BackdropPipelines {
    layout: BindGroupLayoutDescriptor,
    /// Each shader's pipeline, and whether how it compiled has been reported.
    by_shader: HashMap<AssetId<Shader>, (CachedRenderPipelineId, bool)>,
}

const FORMAT: TextureFormat = TextureFormat::Rgba8UnormSrgb;

fn prepare_layout(mut commands: Commands) {
    commands.insert_resource(BackdropPipelines {
        layout: BindGroupLayoutDescriptor::new(
            "UI backdrop",
            &BindGroupLayoutEntries::single(
                ShaderStages::FRAGMENT,
                uniform_buffer::<BackdropUniform>(true),
            ),
        ),
        by_shader: HashMap::default(),
    });
}

fn queue_pipelines(
    passes: Query<&BackdropPass>,
    mut pipelines: ResMut<BackdropPipelines>,
    cache: Res<PipelineCache>,
    reports: Res<Reports>,
) {
    let pipelines = &mut *pipelines;
    let mut used = HashSet::new();
    for BackdropPass(shader) in &passes {
        used.insert(shader.id());
        let (pipeline, reported) = pipelines.by_shader.entry(shader.id()).or_insert_with(|| {
            let pipeline = cache.queue_render_pipeline(RenderPipelineDescriptor {
                label: Some("UI backdrop".into()),
                layout: vec![pipelines.layout.clone()],
                vertex: VertexState {
                    shader: shader.clone(),
                    shader_defs: Vec::new(),
                    entry_point: Some("backdrop_vertex".into()),
                    buffers: Vec::new(),
                    constants: Vec::new(),
                },
                fragment: Some(FragmentState {
                    shader: shader.clone(),
                    entry_point: Some("backdrop_fragment".into()),
                    targets: vec![Some(ColorTargetState {
                        format: FORMAT,
                        blend: None,
                        write_mask: ColorWrites::ALL,
                    })],
                    ..default()
                }),
                ..default()
            });
            (pipeline, false)
        });
        if *reported {
            continue;
        }
        let result = match cache.get_render_pipeline_state(*pipeline) {
            CachedPipelineState::Ok(_) => Ok(()),
            CachedPipelineState::Err(
                ShaderCacheError::ProcessShaderError(error)
                | ShaderCacheError::CreateShaderModule(error),
            ) => Err(error.clone()),
            // Still on its way.
            _ => continue,
        };
        *reported = true;
        reports.0.lock().unwrap().push((shader.id(), result));
    }
    pipelines.by_shader.retain(|shader, _| used.contains(shader));
}

fn render(
    view: ViewQuery<(
        &ViewTarget,
        &BackdropPass,
        &DynamicUniformIndex<BackdropUniform>,
    )>,
    pipelines: Res<BackdropPipelines>,
    cache: Res<PipelineCache>,
    uniforms: Res<ComponentUniforms<BackdropUniform>>,
    mut ctx: RenderContext,
) {
    let (target, BackdropPass(shader), index) = view.into_inner();
    if target.main_texture_format() != FORMAT {
        return;
    }
    let Some(pipeline) = pipelines
        .by_shader
        .get(&shader.id())
        .and_then(|(pipeline, _)| cache.get_render_pipeline(*pipeline))
    else {
        return;
    };
    let Some(binding) = uniforms.uniforms().binding() else {
        return;
    };
    let group = ctx.render_device().create_bind_group(
        "UI backdrop",
        &cache.get_bind_group_layout(&pipelines.layout),
        &BindGroupEntries::single(binding),
    );
    let mut pass = ctx
        .command_encoder()
        .begin_render_pass(&RenderPassDescriptor {
            label: Some("UI backdrop"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: target.main_texture_view(),
                depth_slice: None,
                resolve_target: None,
                ops: Operations::default(),
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, &group, &[index.index()]);
    pass.draw(0..3, 0..1);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default()))
            .init_asset::<Image>()
            .init_asset::<Shader>()
            .init_resource::<Reports>()
            .add_systems(Update, sync);
        app
    }

    fn shader(source: &str) -> SurfaceBackdrop {
        SurfaceBackdrop::Shader {
            source: source.into(),
            frame_rate: 30.0,
        }
    }

    fn state(app: &App, surface: Entity) -> (Entity, Option<AssetId<Shader>>) {
        let state = app.world().get::<BackdropState>(surface).unwrap();
        (state.drawing, state.shader)
    }

    #[test]
    fn a_shader_backdrop_shows_once_it_compiled_and_says_why_it_did_not() {
        let mut app = app();
        let surface = app.world_mut().spawn(shader("one")).id();
        app.update();
        let world = app.world();
        let theme = Theme::dark();
        assert_eq!(
            world.get::<BackdropStatus>(surface),
            Some(&BackdropStatus::Pending)
        );
        // Until it compiled, the background colour.
        assert_ne!(
            window_fill(world, surface, &theme),
            egui::Color32::TRANSPARENT
        );
        let (drawing, id) = state(&app, surface);
        assert!(world.get::<Camera>(drawing).is_some());

        let reports = app.world().resource::<Reports>().clone();
        reports.0.lock().unwrap().push((id.unwrap(), Ok(())));
        app.update();
        assert_eq!(
            app.world().get::<BackdropStatus>(surface),
            Some(&BackdropStatus::Ready)
        );
        assert_eq!(
            window_fill(app.world(), surface, &theme),
            egui::Color32::TRANSPARENT
        );

        // An edited shader starts over, with a new camera; the old one goes.
        app.world_mut().entity_mut(surface).insert(shader("two"));
        app.update();
        let (replaced, id) = state(&app, surface);
        assert_ne!(replaced, drawing);
        assert!(app.world().get_entity(drawing).is_err());
        assert_eq!(
            app.world().get::<BackdropStatus>(surface),
            Some(&BackdropStatus::Pending)
        );
        reports
            .0
            .lock()
            .unwrap()
            .push((id.unwrap(), Err("line 3".into())));
        app.update();
        assert_eq!(
            app.world().get::<BackdropStatus>(surface),
            Some(&BackdropStatus::Failed("line 3".into()))
        );

        // Without a backdrop, nothing of it is left.
        app.world_mut().entity_mut(surface).remove::<SurfaceBackdrop>();
        app.update();
        assert!(app.world().get::<BackdropState>(surface).is_none());
        assert!(app.world().get::<BackdropStatus>(surface).is_none());
        assert!(app.world().get_entity(replaced).is_err());
    }

    #[test]
    fn a_target_backdrop_is_ready_at_once_and_goes_with_its_surface() {
        let mut app = app();
        let image = app
            .world_mut()
            .resource_mut::<Assets<Image>>()
            .add(viewport_image(UVec2::ONE));
        let surface = app
            .world_mut()
            .spawn(SurfaceBackdrop::Target(image.clone()))
            .id();
        app.update();
        assert_eq!(
            app.world().get::<BackdropStatus>(surface),
            Some(&BackdropStatus::Ready)
        );
        let (drawing, _) = state(&app, surface);
        assert_eq!(
            app.world().get::<BackdropImage>(drawing).unwrap().image,
            image
        );
        // Someone else's image: no camera of ours.
        assert!(app.world().get::<Camera>(drawing).is_none());
        app.world_mut().despawn(surface);
        app.update();
        assert!(app.world().get_entity(drawing).is_err());
    }

    #[test]
    fn a_shader_module_keeps_the_sources_line_numbers() {
        let module = module("fn backdrop() {}");
        assert!(module.starts_with("fn backdrop() {}\n"));
        assert!(module.contains("fn backdrop_fragment"));
    }
}
