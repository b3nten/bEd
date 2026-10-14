//! A single-window Bevy desktop with a stack-scoped Dear ImGui frontend.
//! Bevy owns native presentation and the device lifecycle. Only owned frame
//! snapshots and GPU handles cross from the main world to the render world.
pub use bevy;
pub use dear_imgui_rs;
pub use dear_imgui_wgpu;
pub use wgpu;
pub use winit;
mod canvases;
pub use canvases::*;
mod appearance;
pub use appearance::*;
pub mod native;
pub use native::*;
mod capture;
pub use capture::*;
mod injected_input;
pub use injected_input::*;
mod frame_timings;
mod input_map;
mod input_router;
mod memory_assets;
mod platform;
mod render;
pub use frame_timings::{FrameStage, FrameStageTiming, measure_frame_stage};
pub use memory_assets::{MEMORY_ASSET_SOURCE, MemoryAssets};
#[cfg(feature = "preview")]
pub mod preview;
#[cfg(feature = "preview")]
mod previews;
#[cfg(feature = "preview")]
pub use previews::DesktopPreviewIntegrationPlugin;
#[cfg(test)]
static IMGUI_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
pub use platform::DesktopPlatform;
use render::DesktopRenderer;
pub use render::{DesktopComposition, DesktopCompositionFn, DesktopGraph};

use bevy::{
    app::TaskPoolPlugin,
    diagnostic,
    input::InputSystems,
    prelude::*,
    render::{
        ExtractSchedule, RenderApp, RenderPlugin, RenderStartup,
        pipelined_rendering::PipelinedRenderingPlugin,
        renderer::{RenderAdapter, RenderDevice, RenderInstance, RenderQueue},
        settings::WgpuSettings,
    },
    window::PrimaryWindow,
    winit::{RawWinitWindowEvent, WINIT_WINDOWS},
};
use dear_imgui_rs::{Context, Ui, render::FrameSnapshot};
use dear_imgui_wgpu::{ExternalTextureId, WgpuInitInfo, WgpuSnapshotContext, WgpuSnapshotRenderer};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

/// Ordered before Bevy normalizes input and runs gameplay systems.
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DesktopSystems {
    Gpu,
    InjectInput,
    InterceptInput,
    Input,
    /// Project application configuration into requested runtime state.
    Configure,
    /// Settings/font preparation completes with DesktopContext installed again.
    Prepare,
    Ui,
    AfterUi,
    RouteInput,
}

/// Input owners run in priority order before the surviving events reach ImGui.
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DesktopInputStages {
    Overlay,
    Panel,
    Commands,
}

pub struct DesktopPlugins {
    pub window: Window,
    pub wgpu: WgpuSettings,
    pub close_when_requested: bool,
}

impl Default for DesktopPlugins {
    fn default() -> Self {
        Self {
            window: Window {
                title: "Desktop application".into(),
                resolution: (1200, 800).into(),
                ..default()
            },
            wgpu: WgpuSettings::default(),
            close_when_requested: true,
        }
    }
}

#[derive(Resource, Clone)]
struct DesktopSettings(WgpuSettings);

impl PluginGroup for DesktopPlugins {
    fn build(self) -> bevy::app::PluginGroupBuilder {
        bevy::app::PluginGroupBuilder::start::<Self>()
            .add(DesktopHostPlugin(self))
            .add(DesktopIntegrationPlugin)
    }
}

struct DesktopHostPlugin(DesktopPlugins);

impl Plugin for DesktopHostPlugin {
    fn build(&self, app: &mut App) {
        MemoryAssets::register(app);
        let mut plugins = DefaultPlugins
            .set(bevy::window::WindowPlugin {
                primary_window: Some(self.0.window.clone()),
                close_when_requested: self.0.close_when_requested,
                ..default()
            })
            .set(RenderPlugin {
                render_creation: self.0.wgpu.clone().into(),
                ..default()
            })
            .disable::<PipelinedRenderingPlugin>();
        if app.is_plugin_added::<TaskPoolPlugin>() {
            plugins = plugins.disable::<TaskPoolPlugin>();
        }
        if app.is_plugin_added::<bevy::time::TimePlugin>() {
            plugins = plugins.disable::<bevy::time::TimePlugin>();
        }
        if app.is_plugin_added::<bevy::input::InputPlugin>() {
            plugins = plugins.disable::<bevy::input::InputPlugin>();
        }
        if app.is_plugin_added::<diagnostic::FrameCountPlugin>() {
            plugins = plugins.disable::<diagnostic::FrameCountPlugin>();
        }
        app.insert_resource(DesktopSettings(self.0.wgpu.clone()));
        if !app
            .world()
            .contains_resource::<bevy::render::error_handler::RenderErrorHandler>()
        {
            app.insert_resource(bevy::render::error_handler::RenderErrorHandler(
                default_render_error,
            ));
        }
        app.add_plugins(plugins);
    }
}

/// Internal second stage of `DesktopPlugins`, after the host installs its
/// window, renderer and asset sources.
#[doc(hidden)]
pub struct DesktopIntegrationPlugin;

impl Plugin for DesktopIntegrationPlugin {
    fn build(&self, app: &mut App) {
        validate_integration(app);
        #[cfg(target_os = "macos")]
        app.insert_resource(native::NativeClosePolicy(
            app.get_added_plugins::<bevy::window::WindowPlugin>()[0].close_when_requested,
        ));
        let mut context = Context::create();
        context
            .set_ini_filename(None::<PathBuf>)
            .expect("desktop ini filename");
        let flags = context.io().config_flags() | dear_imgui_rs::ConfigFlags::DOCKING_ENABLE;
        context.io_mut().set_config_flags(flags);
        let platform = DesktopPlatform::new(&mut context);
        app.insert_non_send(DesktopContext {
            context,
            platform,
            snapshot_context: None,
        })
        .init_resource::<DesktopFrame>()
        .init_resource::<DesktopFrameInfo>()
        .insert_resource(frame_timings::FrameTimings::from_environment())
        .init_resource::<DesktopScreenshotRequest>()
        .init_resource::<DesktopScreenshotResult>();
        let captures = capture::CaptureBridge::default();
        captures.install(app.world_mut());

        app.insert_resource(captures.clone());
        let bridge = DeviceBridge::default();
        let status = DesktopRenderStatus::default();
        app.insert_resource(bridge.clone())
            .insert_resource(status.clone());
        let policy = app
            .world_mut()
            .remove_resource::<bevy::render::error_handler::RenderErrorHandler>()
            .unwrap_or_default();
        app.insert_resource(HostRenderErrorHandler(policy.0));
        app.insert_resource(bevy::render::error_handler::RenderErrorHandler(
            render_error,
        ));
        install_input_routing(app);
        app.configure_sets(
            PreUpdate,
            (
                DesktopInputStages::Overlay,
                DesktopInputStages::Panel,
                DesktopInputStages::Commands,
            )
                .chain()
                .in_set(DesktopSystems::InterceptInput),
        );
        app.add_systems(Startup, spawn_desktop_camera)
            .add_systems(PreUpdate, ensure_renderer.in_set(DesktopSystems::Gpu))
            .add_systems(PreUpdate, draw_frontend.in_set(DesktopSystems::Ui))
            .add_systems(
                PreUpdate,
                request_screenshot.in_set(DesktopSystems::AfterUi),
            )
            .add_systems(Last, shutdown_on_exit.after(bevy::app::OnAppExitSystems));
        app.sub_app_mut(RenderApp)
            .insert_resource(bridge)
            .insert_resource(captures)
            .insert_resource(status)
            .init_resource::<render::RenderFrame>()
            .add_systems(RenderStartup, publish_device)
            .add_systems(ExtractSchedule, extract_frame);
        render::install(app);
        app.add_plugins((DesktopThemePlugin, DesktopNativePlugin, DesktopCanvasPlugin));
    }
}

fn validate_integration(app: &App) {
    assert!(
        !app.is_plugin_added::<PipelinedRenderingPlugin>(),
        "DesktopIntegrationPlugin requires serial rendering; disable PipelinedRenderingPlugin before adding DefaultPlugins"
    );
    assert!(
        app.is_plugin_added::<bevy::window::WindowPlugin>()
            && app.is_plugin_added::<bevy::asset::AssetPlugin>()
            && app.is_plugin_added::<RenderPlugin>()
            && app.is_plugin_added::<bevy::camera::CameraPlugin>()
            && app.is_plugin_added::<bevy::core_pipeline::CorePipelinePlugin>()
            && app.is_plugin_added::<bevy::input::InputPlugin>()
            && app.is_plugin_added::<bevy::winit::WinitPlugin>()
            && app.get_sub_app(RenderApp).is_some(),
        "DesktopIntegrationPlugin requires the host's window, assets, renderer, camera, core pipeline, input and Winit plugins"
    );
}

type RenderErrorCallback = for<'a> fn(
    &'a bevy::render::error_handler::RenderError,
    &'a mut World,
    &'a mut World,
) -> bevy::render::error_handler::RenderErrorPolicy;

#[derive(Resource)]
struct HostRenderErrorHandler(RenderErrorCallback);

/// UI-thread state. The frame token and Ui never become resources.
pub struct DesktopContext {
    pub context: Context,
    pub platform: DesktopPlatform,
    pub snapshot_context: Option<WgpuSnapshotContext>,
}

impl Drop for DesktopContext {
    fn drop(&mut self) {
        self.snapshot_context.take();
        self.platform.shutdown(&mut self.context);
    }
}

/// A custom frontend owns its entire prepare/draw/capture transaction.
#[derive(Resource)]
pub struct DesktopFrontend(pub fn(&mut World, &mut DesktopContext));

/// Default frontend callback. Its Ui borrow ends before commands/gameplay run.
#[derive(Resource)]
pub struct DesktopUi(pub fn(&mut World, &Ui));

pub struct DesktopInputEvent {
    pub event: RawWinitWindowEvent,
    /// Prevent this event from reaching ImGui.
    pub consumed: bool,
    /// Prevent gameplay delivery while retaining ImGui delivery. For example,
    /// a terminal owns keys that it consumes before a command changes focus.
    pub gameplay_consumed: bool,
}

#[derive(Resource, Default)]
pub struct DesktopInputEvents(pub Vec<DesktopInputEvent>);

#[derive(Resource, Default, Clone, Copy)]
pub struct DesktopCapture {
    pub keyboard: bool,
    pub pointer: bool,
}

#[derive(Resource, Default)]
pub struct DesktopFrame {
    pub snapshot: Option<FrameSnapshot>,
    pub canvases: Vec<wgpu::CommandBuffer>,
    pub size: [u32; 2],
    pub panels: Vec<DesktopPanelGeometry>,
}

impl DesktopFrame {
    pub fn publish_panel(&mut self, geometry: DesktopPanelGeometry) {
        self.panels.push(geometry);
    }
}

type RendererOwner = Arc<Mutex<Option<WgpuSnapshotRenderer>>>;

#[derive(Resource, Clone)]
pub struct DesktopGpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub adapter: wgpu::Adapter,
    pub instance: wgpu::Instance,
    pub generation: u64,
    renderer: RendererOwner,
}

impl DesktopGpu {
    /// Upload a validated packed RGBA image on this generation's shared device.
    /// The caller owns the texture and its external registration lifetime.
    pub fn upload_rgba(
        &self,
        size: [u32; 2],
        pixels: &[u8],
        srgb: bool,
    ) -> Result<wgpu::Texture, String> {
        let maximum = self.device.limits().max_texture_dimension_2d;
        let expected = u64::from(size[0])
            .checked_mul(u64::from(size[1]))
            .and_then(|count| count.checked_mul(4));
        if size.contains(&0)
            || size.iter().any(|value| *value > maximum)
            || expected != Some(pixels.len() as u64)
        {
            return Err("RGBA dimensions do not match the pixels or exceed GPU limits".into());
        }
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Desktop RGBA image"),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: if srgb {
                wgpu::TextureFormat::Rgba8UnormSrgb
            } else {
                wgpu::TextureFormat::Rgba8Unorm
            },
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            texture.as_image_copy(),
            pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size[0] * 4),
                rows_per_image: Some(size[1]),
            },
            texture.size(),
        );
        Ok(texture)
    }

    pub fn register_external_texture(
        &self,
        view: &wgpu::TextureView,
    ) -> Result<ExternalTextureId, String> {
        self.with_renderer(|renderer| renderer.register_external_texture(view))
    }

    pub fn update_external_texture(
        &self,
        texture: ExternalTextureId,
        view: &wgpu::TextureView,
    ) -> Result<(), String> {
        self.with_renderer(|renderer| renderer.update_external_texture(texture, view))
    }

    pub fn unregister_external_texture(&self, texture: ExternalTextureId) -> Result<(), String> {
        self.with_renderer(|renderer| renderer.unregister_external_texture(texture))
    }

    fn with_renderer<T>(
        &self,
        action: impl FnOnce(&mut WgpuSnapshotRenderer) -> dear_imgui_wgpu::RendererResult<T>,
    ) -> Result<T, String> {
        let mut renderer = self
            .renderer
            .lock()
            .expect("desktop renderer mutex poisoned");
        action(renderer.as_mut().ok_or("desktop GPU is recovering")?)
            .map_err(|error| error.to_string())
    }
}

#[derive(Clone, Default)]
pub struct DesktopRenderState {
    pub completed_frames: u64,
    pub error: Option<String>,
    pub recoveries: u64,
    pub recovering: bool,
}

#[derive(Resource, Clone, Default)]
pub struct DesktopRenderStatus(Arc<Mutex<DesktopRenderState>>);

impl DesktopRenderStatus {
    pub fn read(&self) -> DesktopRenderState {
        self.0
            .lock()
            .expect("desktop status mutex poisoned")
            .clone()
    }

    pub fn take_error(&self) -> Option<String> {
        self.0
            .lock()
            .expect("desktop status mutex poisoned")
            .error
            .take()
    }
}

#[derive(Resource, Default)]
pub struct DesktopScreenshotRequest {
    pub path: Option<PathBuf>,
}

#[derive(Resource, Default)]
pub struct DesktopScreenshotResult {
    pub completed: Option<PathBuf>,
    pub error: Option<String>,
}

#[derive(Clone)]
struct Devices {
    device: wgpu::Device,
    queue: wgpu::Queue,
    adapter: wgpu::Adapter,
    instance: wgpu::Instance,
    generation: u64,
}

#[derive(Resource, Clone, Default)]
struct DeviceBridge(Arc<Mutex<(u64, Option<Devices>)>>);

fn publish_device(world: &mut World) {
    // SurfaceData is not a GPU resource automatically reset by Bevy 0.20.
    // Removing both components drops acquired textures and recreates the surface
    // on the new instance/device in the following extraction/preparation.
    use bevy::render::view::{ExtractedWindow, SurfaceData};
    let windows: Vec<_> = world
        .query_filtered::<Entity, With<ExtractedWindow>>()
        .iter(world)
        .collect();
    for entity in windows {
        world
            .entity_mut(entity)
            .remove::<(ExtractedWindow, SurfaceData)>();
    }
    let bridge = world.resource::<DeviceBridge>().clone();
    let mut slot = bridge.0.lock().expect("device bridge poisoned");
    slot.0 += 1;
    slot.1 = Some(Devices {
        device: world.resource::<RenderDevice>().wgpu_device().clone(),
        queue: world.resource::<RenderQueue>().clone().into_inner(),
        adapter: world.resource::<RenderAdapter>().clone().into_inner(),
        instance: world.resource::<RenderInstance>().clone().into_inner(),
        generation: slot.0,
    });
    world.remove_resource::<DesktopRenderer>();
    render::reset_capture_targets(world);
    let mut status = world.resource::<DesktopRenderStatus>().0.lock().unwrap();
    status.recovering = false;
    status.error = None;
}

fn ensure_renderer(world: &mut World) {
    world
        .resource::<capture::CaptureBridge>()
        .expire(std::time::Instant::now());
    let devices = world
        .resource::<DeviceBridge>()
        .0
        .lock()
        .expect("device bridge poisoned")
        .1
        .clone();
    let Some(devices) = devices else {
        return;
    };
    if devices.device.poll(wgpu::PollType::Poll).is_err() {
        return;
    }
    if world
        .get_resource::<DesktopGpu>()
        .is_some_and(|gpu| gpu.generation == devices.generation)
    {
        return;
    }
    let result = {
        let mut state = bevy::ecs::system::SystemState::<NonSendMut<DesktopContext>>::new(world);
        let mut desktop = state.get_mut(world).expect("desktop context installed");
        WgpuSnapshotRenderer::new(
            WgpuInitInfo::new(
                devices.device.clone(),
                devices.queue.clone(),
                wgpu::TextureFormat::Rgba8Unorm,
            )
            .with_adapter(devices.adapter.clone()),
            &mut desktop.context,
        )
        .map(|(renderer, owner)| {
            desktop.snapshot_context = Some(owner);
            renderer
        })
    };
    match result {
        Ok(renderer) => {
            world.insert_resource(DesktopGpu {
                device: devices.device,
                queue: devices.queue,
                adapter: devices.adapter,
                instance: devices.instance,
                generation: devices.generation,
                renderer: Arc::new(Mutex::new(Some(renderer))),
            });
        }
        Err(error) => {
            world
                .resource::<DesktopRenderStatus>()
                .0
                .lock()
                .unwrap()
                .error = Some(error.to_string())
        }
    }
}

fn collect_events(
    mut reader: MessageReader<RawWinitWindowEvent>,
    mut events: ResMut<DesktopInputEvents>,
) {
    events.0.clear();
    events
        .0
        .extend(reader.read().cloned().map(|event| DesktopInputEvent {
            event,
            consumed: false,
            gameplay_consumed: false,
        }));
}

fn install_input_routing(app: &mut App) {
    let queue = injected_input::InputQueue::default();
    queue.install(app.world_mut());
    app.insert_resource(queue)
        .init_resource::<DesktopFrameInfo>();
    app.init_resource::<DesktopCapture>()
        .init_resource::<DesktopInputEvents>()
        .init_resource::<input_router::InputBridge>()
        .configure_sets(
            PreUpdate,
            (
                DesktopSystems::Gpu,
                DesktopSystems::InjectInput,
                DesktopSystems::InterceptInput,
                DesktopSystems::Input,
                DesktopSystems::Configure,
                DesktopSystems::Prepare,
                DesktopSystems::Ui,
                DesktopSystems::AfterUi,
                DesktopSystems::RouteInput,
            )
                .chain()
                .before(InputSystems),
        )
        .add_systems(First, collect_events)
        .add_systems(
            PreUpdate,
            injected_input::prepare.in_set(DesktopSystems::InjectInput),
        )
        .add_systems(
            PreUpdate,
            injected_input::receipt.in_set(DesktopSystems::AfterUi),
        )
        .add_systems(PreUpdate, dispatch_input.in_set(DesktopSystems::Input))
        .add_systems(PreUpdate, route_input.in_set(DesktopSystems::RouteInput));
}

fn dispatch_input(world: &mut World) {
    world.resource_scope(|world, mut router: Mut<input_router::InputBridge>| router.collect(world));
    let primary = world
        .query_filtered::<Entity, With<PrimaryWindow>>()
        .iter(world)
        .next();
    let mut state = bevy::ecs::system::SystemState::<(
        NonSendMut<DesktopContext>,
        Res<DesktopInputEvents>,
        Res<DesktopInjectedInputEvents>,
        ResMut<input_router::InputBridge>,
        ResMut<injected_input::InjectedState>,
    )>::new(world);
    let (mut desktop, events, injected, mut router, mut owned) = state
        .get_mut(world)
        .expect("desktop input resources installed");
    for event in &events.0 {
        router.observe(
            &event.event.event,
            event.consumed || event.gameplay_consumed,
        );
    }
    if let Some(primary) = primary {
        WINIT_WINDOWS.with(|windows| {
            if let Some(window) = windows.borrow().get_window(primary) {
                for event in &events.0 {
                    if event.event.window_id == window.id()
                        && (!event.consumed
                            || matches!(
                                event.event.event,
                                winit::event::WindowEvent::CursorMoved { .. }
                            ))
                    {
                        let DesktopContext {
                            context, platform, ..
                        } = &mut *desktop;
                        platform.handle_event(context, window, &event.event.event);
                    }
                }
            }
        });
        for event in &injected.0 {
            let releasing = matches!(
                &event.event,
                DesktopInjectedEvent::Key { pressed: false, .. }
                    | DesktopInjectedEvent::PointerButton { pressed: false, .. }
            ) || matches!(&event.event,DesktopInjectedEvent::Modifiers(modifiers) if *modifiers==DesktopModifiers::default());
            if !event.consumed || releasing {
                injected_input::feed(&mut desktop, &event.event);
            }
            owned.observe(&event.event);
            router.inject(
                &event.event,
                primary,
                event.consumed || event.gameplay_consumed,
            );
        }
    }
}

fn route_input(world: &mut World) {
    let capture = *world.resource::<DesktopCapture>();
    world.resource_scope(|world, mut router: Mut<input_router::InputBridge>| {
        router.flush(world, capture);
    });
}

fn draw_frontend(world: &mut World) {
    let _timing = measure_frame_stage(world, FrameStage::Ui);
    let Some(gpu) = world.get_resource::<DesktopGpu>() else {
        return;
    };
    if gpu.device.poll(wgpu::PollType::Poll).is_err() {
        return;
    }
    world.resource_mut::<DesktopFrame>().panels.clear();
    let mut desktop = world
        .remove_non_send::<DesktopContext>()
        .expect("desktop context installed");
    if let Some(frontend) = world
        .get_resource::<DesktopFrontend>()
        .map(|frontend| frontend.0)
    {
        frontend(world, &mut desktop);
    } else {
        default_frontend(world, &mut desktop);
    }
    world.insert_non_send(desktop);
    let info = {
        let frame = world.resource::<DesktopFrame>();
        frame.snapshot.as_ref().map(|snapshot| {
            let draw = snapshot.draw_data();
            DesktopFrameInfo {
                frame: DesktopFrameId(world.resource::<DesktopFrameInfo>().frame.0 + 1),
                generation: world.resource::<DesktopGpu>().generation,
                physical_size: frame.size,
                display_pos: draw.display_pos,
                display_size: draw.display_size,
                framebuffer_scale: draw.framebuffer_scale,
                panels: frame.panels.clone(),
            }
        })
    };
    if let Some(info) = info {
        world.insert_resource(info);
    }
}

/// Draw one complete frontend with its prepared context and shared motion.
/// Custom DesktopFrontend transactions call this between BeginFrame and render.
pub fn draw_ui_frame(world: &mut World, ui: &Ui, draw: impl FnOnce(&mut World, &Ui)) {
    let context = world.non_send::<bedtime_ui::UiRuntime>().frame_context();
    context.assert_matches(ui);
    world.non_send_mut::<bedtime_ui::UiRuntime>().begin(ui);
    context.scope(|| draw(world, ui));
    world.non_send_mut::<bedtime_ui::UiRuntime>().end(ui);
}

fn default_frontend(world: &mut World, desktop: &mut DesktopContext) {
    let primary = world
        .query_filtered::<Entity, With<PrimaryWindow>>()
        .iter(world)
        .next();
    let Some(primary) = primary else {
        return;
    };
    WINIT_WINDOWS.with(|windows| {
        let windows = windows.borrow();
        let Some(window) = windows.get_window(primary) else {
            return;
        };
        let DesktopContext {
            context,
            platform,
            snapshot_context,
        } = desktop;
        let Some(owner) = snapshot_context.as_mut() else {
            return;
        };
        if let Err(error) = owner.poll_completions(context) {
            world
                .resource::<DesktopRenderStatus>()
                .0
                .lock()
                .unwrap()
                .error = Some(error.to_string());
            return;
        }
        platform.prepare_frame(context, window);
        let size = window.inner_size();
        world.resource_mut::<DesktopFrame>().size = [size.width, size.height];
        let frame = context.begin_frame();
        if let Some(draw) = world.get_resource::<DesktopUi>().map(|draw| draw.0) {
            draw_ui_frame(world, frame.ui(), draw);
        }
        world.insert_resource(DesktopCapture {
            keyboard: frame.ui().io().want_capture_keyboard() || frame.ui().io().want_text_input(),
            pointer: frame.ui().io().want_capture_mouse(),
        });
        platform.prepare_render(frame.ui(), window);
        match frame.render_snapshot(owner.renderer_consumer().expect("active renderer consumer")) {
            Ok(snapshot) => world.resource_mut::<DesktopFrame>().snapshot = Some(snapshot),
            Err(error) => {
                world
                    .resource::<DesktopRenderStatus>()
                    .0
                    .lock()
                    .unwrap()
                    .error = Some(error.to_string())
            }
        }
        platform.update_ime(window);
    });
}

fn extract_frame(mut commands: Commands, mut main: ResMut<bevy::render::MainWorld>) {
    {
        let info = main.resource::<DesktopFrameInfo>().clone();
        let mut frame = main.resource_mut::<DesktopFrame>();
        commands.insert_resource(render::RenderFrame {
            info,
            snapshot: frame.snapshot.take(),
            canvases: std::mem::take(&mut frame.canvases),
            size: frame.size,
        });
    }
    if let Some(timings) = main.get_resource::<frame_timings::FrameTimings>() {
        commands.insert_resource(timings.clone());
    }
    if let Some(gpu) = main.get_resource::<DesktopGpu>() {
        commands.insert_resource(gpu.clone());
    }
    if let Some(composition) = main.get_resource::<DesktopComposition>() {
        commands.insert_resource(*composition);
    }
}

fn spawn_desktop_camera(mut commands: Commands) {
    commands.spawn((
        Camera {
            order: isize::MAX,
            ..default()
        },
        bevy::camera::Projection::Orthographic(bevy::camera::OrthographicProjection::default_2d()),
        bevy::render::camera::CameraRenderGraph::new(DesktopGraph),
        bevy::render::view::Msaa::Off,
    ));
}

fn render_error(
    error: &bevy::render::error_handler::RenderError,
    main: &mut World,
    render: &mut World,
) -> bevy::render::error_handler::RenderErrorPolicy {
    use bevy::render::error_handler::{ErrorType, RenderErrorPolicy};
    // Ask the owner first, then clean up desktop attachments if that decision
    // abandons the current device. Keep the owner's exact recovery settings.
    let callback = main.resource::<HostRenderErrorHandler>().0;
    let policy = callback(error, main, render);
    let status = main.resource::<DesktopRenderStatus>().clone();
    if error.ty != ErrorType::DeviceLost {
        status.0.lock().unwrap().error = Some(error.description.clone());
        return policy;
    }
    if matches!(policy, RenderErrorPolicy::Ignore) {
        return policy;
    }
    // Device loss is an owned recovery transition. Publish it through
    // recovering/recoveries; hosts may treat error as a terminal failure.
    main.resource_mut::<DesktopFrame>().snapshot.take();
    main.resource_mut::<DesktopFrame>().canvases.clear();
    render.resource_mut::<render::RenderFrame>().snapshot.take();
    render
        .resource_mut::<render::RenderFrame>()
        .canvases
        .clear();
    if let Some(gpu) = main.remove_resource::<DesktopGpu>() {
        main.resource::<capture::CaptureBridge>()
            .retry_generation(gpu.generation);
        let result = {
            let mut state = bevy::ecs::system::SystemState::<NonSendMut<DesktopContext>>::new(main);
            let mut desktop = state.get_mut(main).expect("desktop context installed");
            let DesktopContext {
                context,
                snapshot_context,
                ..
            } = &mut *desktop;
            let result = if let Some(owner) = snapshot_context.as_mut()
                && let Some(renderer) = gpu.renderer.lock().unwrap().as_mut()
            {
                renderer
                    .shutdown(owner, context)
                    .map_err(|error| error.to_string())
            } else {
                Ok(())
            };
            snapshot_context.take();
            result
        };
        if let Err(error) = result {
            status.0.lock().unwrap().error = Some(error);
        }
        gpu.renderer.lock().unwrap().take();
    }
    main.resource::<DeviceBridge>().0.lock().unwrap().1.take();
    render.remove_resource::<DesktopGpu>();
    render.remove_resource::<DesktopRenderer>();
    render::reset_capture_targets(render);
    if matches!(policy, RenderErrorPolicy::Recover(_)) {
        status.0.lock().unwrap().recoveries += 1;
        status.0.lock().unwrap().recovering = true;
    }
    policy
}

fn default_render_error(
    error: &bevy::render::error_handler::RenderError,
    main: &mut World,
    _: &mut World,
) -> bevy::render::error_handler::RenderErrorPolicy {
    use bevy::render::error_handler::{ErrorType, RenderErrorPolicy};
    if error.ty == ErrorType::DeviceLost {
        RenderErrorPolicy::Recover(main.resource::<DesktopSettings>().0.clone().into())
    } else {
        RenderErrorPolicy::StopRendering
    }
}

fn shutdown_on_exit(world: &mut World) {
    if world.resource::<Messages<bevy::app::AppExit>>().is_empty() {
        return;
    }
    world.resource::<capture::CaptureBridge>().stop();
    world.resource::<injected_input::InputQueue>().stop();
    world.resource_mut::<DesktopFrame>().snapshot.take();
    world.resource_mut::<DesktopFrame>().canvases.clear();
    let Some(gpu) = world.remove_resource::<DesktopGpu>() else {
        return;
    };
    let result = {
        let mut state = bevy::ecs::system::SystemState::<NonSendMut<DesktopContext>>::new(world);
        let mut desktop = state.get_mut(world).expect("desktop context installed");
        let DesktopContext {
            context,
            snapshot_context,
            ..
        } = &mut *desktop;
        let result = if let Some(owner) = snapshot_context.as_mut()
            && let Some(renderer) = gpu.renderer.lock().unwrap().as_mut()
        {
            renderer
                .shutdown(owner, context)
                .map_err(|error| error.to_string())
        } else {
            Ok(())
        };
        snapshot_context.take();
        result
    };
    gpu.renderer.lock().unwrap().take();
    if let Err(error) = result {
        world
            .resource::<DesktopRenderStatus>()
            .0
            .lock()
            .unwrap()
            .error = Some(error);
    }
}

#[derive(Resource, Default)]
struct ScreenshotPending(Vec<(DesktopCaptureId, PathBuf)>);

fn request_screenshot(world: &mut World) {
    if let Some(path) = world.resource_mut::<DesktopScreenshotRequest>().path.take() {
        let id = world.resource_mut::<DesktopCaptureRequests>().submit(
            vec![DesktopCaptureTarget::Window],
            std::time::Instant::now() + std::time::Duration::from_secs(30),
            None,
        );
        if !world.contains_resource::<ScreenshotPending>() {
            world.init_resource::<ScreenshotPending>();
        }
        world.resource_mut::<ScreenshotPending>().0.push((id, path));
    }
    let Some(mut pending) = world.remove_resource::<ScreenshotPending>() else {
        return;
    };
    pending.0.retain(|(id, path)| {
        let Some(captured) = world.resource_mut::<DesktopCaptureResults>().take(*id) else {
            return true;
        };
        let result = captured
            .map_err(|error| error.to_string())
            .and_then(|captured| {
                captured.images[0]
                    .rgba
                    .write_ppm(path)
                    .map_err(|error| error.to_string())
            });
        match result {
            Ok(()) => {
                world.resource_mut::<DesktopScreenshotResult>().completed = Some(path.clone())
            }
            Err(error) => world.resource_mut::<DesktopScreenshotResult>().error = Some(error),
        }
        false
    });
    world.insert_resource(pending);
}

#[cfg(test)]
fn save_ppm(path: &std::path::Path, image: &bevy::image::Image) -> std::io::Result<()> {
    use std::io::Write;
    let data = image
        .data
        .as_deref()
        .ok_or_else(|| std::io::Error::other("screenshot has no CPU pixels"))?;
    let bgra = matches!(
        image.texture_descriptor.format,
        wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
    );
    if !bgra
        && !matches!(
            image.texture_descriptor.format,
            wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Rgba8UnormSrgb
        )
    {
        return Err(std::io::Error::other("unsupported screenshot format"));
    }
    let mut file = std::io::BufWriter::new(std::fs::File::create(path)?);
    write!(file, "P6\n{} {}\n255\n", image.width(), image.height())?;
    for pixel in data.as_chunks::<4>().0 {
        let rgb = if bgra {
            [pixel[2], pixel[1], pixel[0]]
        } else {
            [pixel[0], pixel[1], pixel[2]]
        };
        file.write_all(&rgb)?;
    }
    file.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::asset::RenderAssetUsages;

    #[test]
    fn attachment_rejects_pipelined_rendering_before_installing_desktop_state() {
        let mut app = App::new();
        app.add_plugins(PipelinedRenderingPlugin);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            app.add_plugins(DesktopIntegrationPlugin);
        }));
        assert!(result.is_err());
        assert!(!app.world().contains_resource::<DesktopFrame>());
        assert!(app.world().get_non_send::<DesktopContext>().is_none());
    }

    #[test]
    fn attachment_requires_the_existing_bevy_stack_before_installing_state() {
        let mut app = App::new();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            app.add_plugins(DesktopIntegrationPlugin);
        }));
        assert!(result.is_err());
        assert!(!app.world().contains_resource::<DesktopFrame>());
    }

    #[test]
    fn desktop_recovery_preserves_host_policy_and_exact_recovery_configuration() {
        use bevy::render::{
            error_handler::{ErrorType, RenderError, RenderErrorPolicy},
            settings::RenderCreation,
        };
        #[derive(Resource, Default)]
        struct Calls(u32);
        fn recover(_: &RenderError, main: &mut World, _: &mut World) -> RenderErrorPolicy {
            main.resource_mut::<Calls>().0 += 1;
            RenderErrorPolicy::Recover(
                WgpuSettings {
                    device_label: Some("host-owned replacement".into()),
                    ..default()
                }
                .into(),
            )
        }
        fn ignore(_: &RenderError, main: &mut World, _: &mut World) -> RenderErrorPolicy {
            main.resource_mut::<Calls>().0 += 1;
            RenderErrorPolicy::Ignore
        }
        let mut main = World::new();
        main.init_resource::<Calls>();
        main.init_resource::<DesktopFrame>();
        main.init_resource::<DesktopRenderStatus>();
        main.init_resource::<DeviceBridge>();
        main.insert_resource(HostRenderErrorHandler(recover));
        let mut render = World::new();
        render.init_resource::<render::RenderFrame>();
        let error = RenderError {
            ty: ErrorType::DeviceLost,
            description: "fixture".into(),
            source: None,
        };
        let RenderErrorPolicy::Recover(RenderCreation::Automatic(settings)) =
            render_error(&error, &mut main, &mut render)
        else {
            panic!("host recovery must remain recovery");
        };
        assert_eq!(
            settings.device_label.as_deref(),
            Some("host-owned replacement")
        );
        assert_eq!(main.resource::<Calls>().0, 1);
        assert_eq!(main.resource::<DesktopRenderStatus>().read().recoveries, 1);
        main.insert_resource(HostRenderErrorHandler(ignore));
        assert!(matches!(
            render_error(&error, &mut main, &mut render),
            RenderErrorPolicy::Ignore
        ));
        assert_eq!(main.resource::<Calls>().0, 2);
        assert_eq!(main.resource::<DesktopRenderStatus>().read().recoveries, 1);
    }

    #[test]
    fn screenshot_exports_rgba_and_bgra_as_display_rgb() {
        for (format, pixels) in [
            (
                wgpu::TextureFormat::Rgba8Unorm,
                vec![10, 20, 30, 255, 40, 50, 60, 0],
            ),
            (
                wgpu::TextureFormat::Rgba8UnormSrgb,
                vec![10, 20, 30, 255, 40, 50, 60, 0],
            ),
            (
                wgpu::TextureFormat::Bgra8Unorm,
                vec![30, 20, 10, 255, 60, 50, 40, 0],
            ),
            (
                wgpu::TextureFormat::Bgra8UnormSrgb,
                vec![30, 20, 10, 255, 60, 50, 40, 0],
            ),
        ] {
            let image = bevy::image::Image::new(
                wgpu::Extent3d {
                    width: 2,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                wgpu::TextureDimension::D2,
                pixels,
                format,
                RenderAssetUsages::MAIN_WORLD,
            );
            let path = std::env::temp_dir().join(format!(
                "bed-desktop-screenshot-{}-{format:?}.ppm",
                std::process::id()
            ));
            save_ppm(&path, &image).unwrap();
            let bytes = std::fs::read(&path).unwrap();
            std::fs::remove_file(path).unwrap();
            assert_eq!(&bytes, b"P6\n2 1\n255\n\x0a\x14\x1e\x28\x32\x3c");
        }
    }

    #[test]
    fn screenshot_rejects_missing_pixels_and_unsupported_format_before_creating_file() {
        let path = std::env::temp_dir().join(format!(
            "bed-desktop-screenshot-invalid-{}.ppm",
            std::process::id()
        ));
        let mut image = bevy::image::Image::new_uninit(
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            wgpu::TextureDimension::D2,
            wgpu::TextureFormat::Rgba8Unorm,
            RenderAssetUsages::MAIN_WORLD,
        );
        assert!(
            save_ppm(&path, &image)
                .unwrap_err()
                .to_string()
                .contains("no CPU pixels")
        );
        image.data = Some(vec![0; 16]);
        image.texture_descriptor.format = wgpu::TextureFormat::Rgba32Float;
        assert!(
            save_ppm(&path, &image)
                .unwrap_err()
                .to_string()
                .contains("unsupported")
        );
        assert!(!path.exists());
    }
}
