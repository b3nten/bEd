    use bedtime_ui::{UiSystems, egui};
    use bedtime_workbench::{
        ActiveLayout, LayoutCamera,
        persistence::StoreId,
        settings::{SettingsAppExt, SettingsWorldExt, settings},
    };
    use bevy::{
        asset::{AssetPath, embedded_asset, embedded_path},
        core_pipeline::{Core2d, FullscreenShader, upscaling::upscaling},
        prelude::*,
        render::{
            Render, RenderApp, RenderStartup, RenderSystems,
            extract_component::{
                ComponentUniforms, DynamicUniformIndex, ExtractComponent, ExtractComponentPlugin,
                UniformComponentPlugin,
            },
            render_resource::{
                binding_types::{sampler, texture_2d, uniform_buffer},
                *,
            },
            renderer::{RenderContext, RenderDevice, ViewQuery},
            view::ViewTarget,
        },
    };
    use bevy_egui::{EguiInput, EguiPreUpdateSet, render::egui_pass};
    use std::ops::RangeInclusive;

    const INTENSITY: RangeInclusive<f32> = 0.0..=1.0;

    macro_rules! effect_settings {
    ($($name:ident: $default:expr, $key:literal, $field:ident $(, $label:literal)?;)*) => {
        $(settings! {
            "Appearance" {
                pub $name: f32 {
                    default: $default,
                    key: $key,
                    $(label: $label,)?
                    ui: slider(INTENSITY),
                    visible: effects_on,
                    apply: |world, value| {
                        world.resource_mut::<Effects>().$field =
                            bedtime_workbench::settings::limit(*value, INTENSITY, $default)
                    },
                }
            }
        })*
        fn register_intensities(app: &mut App) {
            $(app.register_setting::<$name>();)*
        }
    };
}

    settings! {
        "Appearance" {
            pub EffectsEnabled: bool {
                default: false,
                key: "effects.enabled",
                label: "Screen effects",
                apply: |world, enabled| world.resource_mut::<Effects>().enabled = *enabled,
            }
        }
    }

    fn effects_on(world: &World, layer: StoreId) -> bool {
        world.settings().get_at::<EffectsEnabled>(layer)
    }

    effect_settings! {
        Scanlines: 0.06, "effects.scanlines", scanlines;
        Vignette: 0.04, "effects.vignette", vignette;
        Bloom: 0.025, "effects.bloom", bloom;
        Noise: 0.0, "effects.noise", noise;
        Curvature: 0.0, "effects.curvature", curvature;
        PixelGrid: 0.0, "effects.pixel_grid", pixel_grid;
        BurnIn: 0.0, "effects.burn_in", burn_in, "Burn-in";
        Pulse: 0.0, "effects.pulse", pulse;
    }

    /// The effects as the settings resolve them.
    #[derive(Resource, Clone, Copy, Debug, PartialEq)]
    pub struct Effects {
        pub enabled: bool,
        pub scanlines: f32,
        pub vignette: f32,
        pub bloom: f32,
        pub noise: f32,
        pub curvature: f32,
        pub pixel_grid: f32,
        pub burn_in: f32,
        pub pulse: f32,
    }

    impl Default for Effects {
        fn default() -> Self {
            use bedtime_workbench::settings::Setting;
            Self {
                enabled: EffectsEnabled::default(),
                scanlines: Scanlines::default(),
                vignette: Vignette::default(),
                bloom: Bloom::default(),
                noise: Noise::default(),
                curvature: Curvature::default(),
                pixel_grid: PixelGrid::default(),
                burn_in: BurnIn::default(),
                pulse: Pulse::default(),
            }
        }
    }

    pub struct EffectsPlugin;
    impl Plugin for EffectsPlugin {
        fn build(&self, app: &mut App) {
            app.init_resource::<Effects>()
                .register_setting::<EffectsEnabled>();
            register_intensities(app);
            app.add_systems(
                PreUpdate,
                prepare_input
                    .after(EguiPreUpdateSet::ProcessInput)
                    .before(UiSystems::Draw),
            );
            // Headless apps (tests) have no renderer.
            if app.get_sub_app(RenderApp).is_none() {
                return;
            }
            // bed's sources sit at the crate root, not under `src`.
            embedded_asset!(app, "bed", "effects.wesl");
            app.add_plugins((
                ExtractComponentPlugin::<EffectsFrame>::default(),
                ExtractComponentPlugin::<EffectUniform>::default(),
                UniformComponentPlugin::<EffectUniform>::default(),
            ));
            app.sub_app_mut(RenderApp)
                .add_systems(RenderStartup, prepare_pipeline)
                .add_systems(
                    Render,
                    prepare_targets.in_set(RenderSystems::PrepareBindGroups),
                )
                .add_systems(
                    Core2d,
                    render
                        .after(egui_pass)
                        .before(UiSystems::PrepareOutput)
                        .before(upscaling),
                );
        }
    }

    #[derive(Component, Clone, Copy, ExtractComponent)]
    #[extract_app(RenderApp)]
    struct EffectsFrame {
        enabled: bool,
        /// The active layout: when it changes, burn-in history starts over.
        scene: Option<Entity>,
    }

    #[derive(Component, Clone, Copy, ShaderType, ExtractComponent)]
    #[extract_app(RenderApp)]
    struct EffectUniform {
        // Scanlines, vignette, bloom, noise.
        light: Vec4,
        // Curvature, pixel grid, persistence, pulse.
        geometry: Vec4,
        // Real elapsed seconds, frame-rate-independent persistence decay.
        clock: Vec4,
    }

    impl EffectUniform {
        fn new(effects: &Effects, time: &Time<Real>) -> Self {
            Self {
                light: Vec4::new(
                    effects.scanlines,
                    effects.vignette,
                    effects.bloom,
                    effects.noise,
                ),
                geometry: Vec4::new(
                    effects.curvature,
                    effects.pixel_grid,
                    effects.burn_in,
                    effects.pulse,
                ),
                clock: Vec4::new(
                    time.elapsed_secs(),
                    effects.burn_in.powf((time.delta_secs() * 240.0).max(0.001)),
                    0.0,
                    0.0,
                ),
            }
        }
    }

    #[derive(Default)]
    struct PointerState {
        camera: Option<Entity>,
        raw: Option<egui::Pos2>,
        mapped: Option<egui::Pos2>,
    }

    fn prepare_input(
        mut commands: Commands,
        effects: Res<Effects>,
        layouts: Query<Entity, With<ActiveLayout>>,
        mut cameras: Query<(Entity, &mut EguiInput), With<LayoutCamera>>,
        time: Res<Time<Real>>,
        mut pointer: Local<PointerState>,
    ) {
        let scene = layouts.iter().next();
        let curvature = if effects.enabled {
            effects.curvature
        } else {
            0.0
        };
        for (camera, mut input) in &mut cameras {
            if pointer.camera != Some(camera) {
                *pointer = PointerState {
                    camera: Some(camera),
                    ..default()
                };
            }
            if let Some(rect) = input.screen_rect {
                map_events(&mut input, rect, curvature, &mut pointer);
            }
            // Captured before the UI draws: a change made in this frame's UI
            // reaches input and rendering together, next frame.
            commands.entity(camera).insert((
                EffectUniform::new(&effects, &time),
                EffectsFrame {
                    enabled: effects.enabled,
                    scene,
                },
            ));
        }
    }

    /// The shader maps display UV to source UV, so input uses the same function,
    /// not its inverse. Radial symmetry makes this identical in top-origin egui UV.
    fn source_position(position: egui::Pos2, rect: egui::Rect, curvature: f32) -> egui::Pos2 {
        if curvature == 0.0 || rect.width() <= 0.0 || rect.height() <= 0.0 {
            return position;
        }
        let uv = (position - rect.min) / rect.size();
        let center = uv - egui::vec2(0.5, 0.5);
        let warped = egui::vec2(0.5, 0.5) + center * (1.0 + curvature * 0.25 * center.length_sq());
        let uv = warped.clamp(egui::vec2(0.001, 0.001), egui::vec2(0.999, 0.999));
        rect.min + uv * rect.size()
    }

    fn map_events(
        input: &mut egui::RawInput,
        rect: egui::Rect,
        curvature: f32,
        pointer: &mut PointerState,
    ) {
        let mut moved = false;
        for event in &mut input.events {
            match event {
                egui::Event::PointerMoved(position) => {
                    pointer.raw = Some(*position);
                    *position = source_position(*position, rect, curvature);
                    pointer.mapped = Some(*position);
                    moved = true;
                }
                egui::Event::PointerButton { pos, .. } => {
                    pointer.raw = Some(*pos);
                    *pos = source_position(*pos, rect, curvature);
                }
                egui::Event::Touch { pos, .. } => *pos = source_position(*pos, rect, curvature),
                egui::Event::PointerGone => {
                    pointer.raw = None;
                    pointer.mapped = None;
                }
                _ => {}
            }
        }
        // egui retains its last pointer coordinate. Reproject a stationary pointer
        // when curvature or viewport size changes, including when disabled.
        if !moved && let Some(raw) = pointer.raw {
            let mapped = source_position(raw, rect, curvature);
            if pointer.mapped != Some(mapped) {
                input.events.insert(0, egui::Event::PointerMoved(mapped));
                pointer.mapped = Some(mapped);
            }
        }
    }

    #[derive(Resource)]
    struct EffectPipeline {
        layout: BindGroupLayoutDescriptor,
        sampler: Sampler,
        crt: CachedRenderPipelineId,
        burn: CachedRenderPipelineId,
        bloom_downsample: CachedRenderPipelineId,
        bloom_horizontal: CachedRenderPipelineId,
        bloom_vertical: CachedRenderPipelineId,
    }

    const BLOOM_FORMAT: TextureFormat = TextureFormat::Rgba16Float;

    fn prepare_pipeline(
        mut commands: Commands,
        device: Res<RenderDevice>,
        assets: Res<AssetServer>,
        fullscreen: Res<FullscreenShader>,
        cache: Res<PipelineCache>,
    ) {
        let layout = BindGroupLayoutDescriptor::new(
            "bEd effects",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::FRAGMENT,
                (
                    texture_2d(TextureSampleType::Float { filterable: true }),
                    sampler(SamplerBindingType::Filtering),
                    uniform_buffer::<EffectUniform>(true),
                    texture_2d(TextureSampleType::Float { filterable: true }),
                    texture_2d(TextureSampleType::Float { filterable: true }),
                ),
            ),
        );
        let shader = assets.load(
            AssetPath::from_path_buf(embedded_path!("bed", "effects.wesl")).with_source("embedded"),
        );
        let pipeline = |entry: &'static str, format| {
            cache.queue_render_pipeline(RenderPipelineDescriptor {
                label: Some(format!("bEd {entry}").into()),
                layout: vec![layout.clone()],
                vertex: fullscreen.to_vertex_state(),
                fragment: Some(FragmentState {
                    shader: shader.clone(),
                    entry_point: Some(entry.into()),
                    targets: vec![Some(ColorTargetState {
                        format,
                        blend: None,
                        write_mask: ColorWrites::ALL,
                    })],
                    ..default()
                }),
                ..default()
            })
        };
        commands.insert_resource(EffectPipeline {
            crt: pipeline("crt", TextureFormat::Rgba8UnormSrgb),
            burn: pipeline("burn", TextureFormat::Rgba8UnormSrgb),
            bloom_downsample: pipeline("bloom_downsample", BLOOM_FORMAT),
            bloom_horizontal: pipeline("bloom_horizontal", BLOOM_FORMAT),
            bloom_vertical: pipeline("bloom_vertical", BLOOM_FORMAT),
            layout,
            sampler: device.create_sampler(&SamplerDescriptor {
                mag_filter: FilterMode::Linear,
                min_filter: FilterMode::Linear,
                ..default()
            }),
        });
    }

    #[derive(Component)]
    struct History {
        textures: [Texture; 2],
        views: [TextureView; 2],
        previous: usize,
        valid: bool,
        // Only effect values and visible scene identity invalidate history; unrelated
        // editor/appearance settings and the continuously advancing clock do not.
        values: (Vec4, Vec4),
        scene: Option<Entity>,
    }

    #[derive(Component)]
    struct BloomTextures {
        textures: [Texture; 2],
        views: [TextureView; 2],
    }

    fn bloom_extent(size: Extent3d) -> Extent3d {
        Extent3d {
            width: size.width.div_ceil(2).max(1),
            height: size.height.div_ceil(2).max(1),
            depth_or_array_layers: 1,
        }
    }

    /// A camera's effect state in the render world.
    type EffectCameras<'w, 's> = Query<
        'w,
        's,
        (
            Entity,
            &'static ViewTarget,
            &'static EffectsFrame,
            &'static EffectUniform,
            Option<&'static mut History>,
            Option<&'static BloomTextures>,
        ),
    >;

    fn prepare_targets(
        mut commands: Commands,
        device: Res<RenderDevice>,
        mut cameras: EffectCameras,
    ) {
        for (entity, target, frame, uniform, history, bloom) in &mut cameras {
            let size = target.main_texture().size();
            if !frame.enabled || uniform.light.z == 0.0 {
                if bloom.is_some() {
                    commands.entity(entity).remove::<BloomTextures>();
                }
            } else {
                let size = bloom_extent(size);
                if bloom.is_none_or(|bloom| bloom.textures[0].size() != size) {
                    let textures = std::array::from_fn(|_| {
                        device.create_texture(&TextureDescriptor {
                            label: Some("bEd reduced bloom"),
                            size,
                            mip_level_count: 1,
                            sample_count: 1,
                            dimension: TextureDimension::D2,
                            format: BLOOM_FORMAT,
                            usage: TextureUsages::RENDER_ATTACHMENT
                                | TextureUsages::TEXTURE_BINDING,
                            view_formats: &[],
                        })
                    });
                    let views = textures
                        .each_ref()
                        .map(|texture| texture.create_view(&TextureViewDescriptor::default()));
                    commands
                        .entity(entity)
                        .insert(BloomTextures { textures, views });
                }
            }
            if !frame.enabled || uniform.geometry.z == 0.0 {
                if history.is_some() {
                    commands.entity(entity).remove::<History>();
                }
                continue;
            }
            let values = (uniform.light, uniform.geometry);
            if let Some(mut history) = history
                && history.textures[0].size() == size
            {
                if history.values != values || history.scene != frame.scene {
                    history.valid = false;
                    history.values = values;
                    history.scene = frame.scene;
                }
                continue;
            }
            let textures = std::array::from_fn(|_| {
                device.create_texture(&TextureDescriptor {
                    label: Some("bEd phosphor history"),
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: TextureDimension::D2,
                    format: TextureFormat::Rgba8UnormSrgb,
                    usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
            });
            let views = textures
                .each_ref()
                .map(|texture| texture.create_view(&TextureViewDescriptor::default()));
            commands.entity(entity).insert(History {
                textures,
                views,
                previous: 0,
                valid: false,
                values,
                scene: frame.scene,
            });
        }
    }

    /// The view being rendered, with its effect state.
    type EffectView<'w, 's> = ViewQuery<
        'w,
        's,
        (
            &'static ViewTarget,
            &'static EffectsFrame,
            &'static DynamicUniformIndex<EffectUniform>,
            Option<&'static mut History>,
            Option<&'static BloomTextures>,
        ),
    >;

    fn render(
        view: EffectView,
        effects: Res<EffectPipeline>,
        cache: Res<PipelineCache>,
        uniforms: Res<ComponentUniforms<EffectUniform>>,
        mut ctx: RenderContext,
    ) {
        let (target, frame, index, history, bloom) = view.into_inner();
        if !frame.enabled || target.main_texture_format() != TextureFormat::Rgba8UnormSrgb {
            return;
        }
        let Some(crt) = cache.get_render_pipeline(effects.crt) else {
            return;
        };
        let Some(binding) = uniforms.uniforms().binding() else {
            return;
        };
        let burn = if history.is_some() {
            let Some(burn) = cache.get_render_pipeline(effects.burn) else {
                return;
            };
            Some(burn)
        } else {
            None
        };
        let bloom_pipelines = if bloom.is_some() {
            let (Some(downsample), Some(horizontal), Some(vertical)) = (
                cache.get_render_pipeline(effects.bloom_downsample),
                cache.get_render_pipeline(effects.bloom_horizontal),
                cache.get_render_pipeline(effects.bloom_vertical),
            ) else {
                return;
            };
            Some((downsample, horizontal, vertical))
        } else {
            None
        };
        // All fallible prerequisites must be resolved before this flips the target.
        let post = target.post_process_write();
        let mut pass = |source: &TextureView,
                        previous: &TextureView,
                        bloom: &TextureView,
                        output: &TextureView,
                        pipeline: &RenderPipeline,
                        label: &'static str| {
            let group = ctx.render_device().create_bind_group(
                label,
                &cache.get_bind_group_layout(&effects.layout),
                &BindGroupEntries::sequential((
                    source,
                    &effects.sampler,
                    binding.clone(),
                    previous,
                    bloom,
                )),
            );
            let mut pass = ctx
                .command_encoder()
                .begin_render_pass(&RenderPassDescriptor {
                    label: Some(label),
                    color_attachments: &[Some(RenderPassColorAttachment {
                        view: output,
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
        };
        let source = if let (Some(history), Some(burn)) = (history, burn) {
            let history = history.into_inner();
            let next = 1 - history.previous;
            let previous = if history.valid {
                &history.views[history.previous]
            } else {
                post.source
            };
            pass(
                post.source,
                previous,
                post.source,
                &history.views[next],
                burn,
                "bEd phosphor persistence",
            );
            history.previous = next;
            history.valid = true;
            &history.views[next]
        } else {
            post.source
        };
        let bloom_source = if let (Some(bloom), Some((downsample, horizontal, vertical))) =
            (bloom, bloom_pipelines)
        {
            pass(
                source,
                source,
                source,
                &bloom.views[0],
                downsample,
                "bEd bloom downsample",
            );
            pass(
                &bloom.views[0],
                source,
                source,
                &bloom.views[1],
                horizontal,
                "bEd bloom horizontal",
            );
            pass(
                &bloom.views[1],
                source,
                source,
                &bloom.views[0],
                vertical,
                "bEd bloom vertical",
            );
            &bloom.views[0]
        } else {
            source
        };
        pass(
            source,
            source,
            bloom_source,
            post.destination,
            crt,
            "bEd CRT",
        );
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn rect() -> egui::Rect {
            egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0))
        }

        #[test]
        fn curvature_bends_pointer_positions_like_the_shader() {
            let rect = rect();
            let corner = egui::pos2(40.0, 30.0);
            // No curvature, or the centre: unchanged.
            assert_eq!(source_position(corner, rect, 0.0), corner);
            let centre = rect.center();
            assert_eq!(source_position(centre, rect, 0.8), centre);
            // Towards the edges the display shows content from further out, as the
            // shader's display→source map does.
            let bent = source_position(corner, rect, 0.8);
            assert!(bent.x < corner.x && bent.y < corner.y, "{bent:?}");
            // Never outside the window.
            let edge = source_position(egui::pos2(0.0, 0.0), rect, 1.0);
            assert!(rect.contains(edge));
        }

        #[test]
        fn a_still_pointer_is_moved_when_curvature_changes() {
            let rect = rect();
            let mut pointer = PointerState::default();
            let mut input = egui::RawInput {
                events: vec![egui::Event::PointerMoved(egui::pos2(40.0, 30.0))],
                ..default()
            };
            map_events(&mut input, rect, 0.0, &mut pointer);
            assert_eq!(
                input.events,
                [egui::Event::PointerMoved(egui::pos2(40.0, 30.0))]
            );

            // No new pointer events, but curvature changed: egui is told where the
            // pointer now lands.
            let mut input = egui::RawInput::default();
            map_events(&mut input, rect, 0.8, &mut pointer);
            let [egui::Event::PointerMoved(moved)] = input.events[..] else {
                panic!("{:?}", input.events);
            };
            assert_eq!(moved, source_position(egui::pos2(40.0, 30.0), rect, 0.8));
            // Unchanged next frame: nothing more to send.
            let mut input = egui::RawInput::default();
            map_events(&mut input, rect, 0.8, &mut pointer);
            assert!(input.events.is_empty());
        }

        #[test]
        fn effects_follow_their_settings_and_stay_in_range() {
            use bedtime_workbench::{WorkbenchCorePlugins, settings::SettingsAppExt};
            let mut app = App::new();
            app.add_plugins((WorkbenchCorePlugins, EffectsPlugin))
                .init_resource::<Time<Real>>()
                .add_settings_layer::<crate::workspace::GlobalStore>();
            app.update();
            let world = app.world_mut();
            assert!(!world.resource::<Effects>().enabled);
            let global = StoreId::of::<crate::workspace::GlobalStore>();
            world
                .settings_mut()
                .set::<EffectsEnabled>(global, true)
                .unwrap();
            world.settings_mut().set::<Curvature>(global, 3.0).unwrap();
            let effects = world.resource::<Effects>();
            assert!(effects.enabled);
            assert_eq!(effects.curvature, 1.0);
            assert_eq!(effects.scanlines, 0.06);
        }
    }