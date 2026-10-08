//! Bed's standalone winit/wgpu host. Editing and document code remain
//! independent of these backends.
use crate::util::settings::Settings;
use crate::workbench::{WindowCommand, Workbench, WorkbenchHostMode};
use bed_effects::{
    shader_manager::ShaderManager, shader_types::OFFSCREEN_FORMAT,
    viewport_effects::ViewportEffectsFactory,
};
use bed_plugin::gpu::{DeviceErrorHandlers, GpuContext, RenderTarget, renderer_device_descriptor};
#[cfg(test)]
use bed_session::editor::Editor;
#[cfg(test)]
use bed_ui::editor_input::EditorInput;
use dear_imgui_rs::{ClipboardBackend, Context, TextureId};
use dear_imgui_wgpu::{
    FramebufferExtent, WgpuInitInfo, WgpuRenderer, WgpuViewportSurfaceConfig,
    multi_viewport::WinitViewportRoute,
};
use dear_imgui_winit::{HiDpiMode, WinitPlatform};
use std::io;
use std::{
    collections::{HashMap, HashSet},
    error::Error,
    future::Future,
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
    task::{Poll, Wake, Waker},
    time::{Duration, Instant},
};
use winit::{
    application::ApplicationHandler,
    dpi::{LogicalSize, PhysicalSize},
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    window::{Window, WindowId},
};

type HostResult<T> = Result<T, Box<dyn Error>>;

struct ThreadWake(std::thread::Thread);
impl Wake for ThreadWake {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.unpark();
    }
}

pub(crate) fn block_on<T>(future: impl Future<Output = T>) -> T {
    let waker = Waker::from(Arc::new(ThreadWake(std::thread::current())));
    let mut context = std::task::Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(value) => return value,
            Poll::Pending => std::thread::park(),
        }
    }
}

struct NativeClipboard(arboard::Clipboard);
impl ClipboardBackend for NativeClipboard {
    fn get(&mut self) -> Option<String> {
        self.0.get_text().ok()
    }
    fn set(&mut self, text: &str) {
        let _ = self.0.set_text(text);
    }
}

enum SurfaceRenderer {
    Native(WinitViewportRoute),
    MainOnly(Box<WgpuRenderer>),
}
enum PreparedFrame<'frame> {
    Native(dear_imgui_wgpu::multi_viewport::WgpuPreparedViewportFrame<'frame>),
    MainOnly(dear_imgui_rs::render::PendingFrame<'frame>),
}
impl PreparedFrame<'_> {
    fn secondary_presentations(&self) -> usize {
        match self {
            Self::Native(frame) => frame
                .secondary_report()
                .present_submitted_viewport_ids()
                .len(),
            Self::MainOnly(_) => 0,
        }
    }
}
impl SurfaceRenderer {
    fn prepare<'frame>(
        &self,
        event_loop: &ActiveEventLoop,
        frame: dear_imgui_rs::FrameToken<'frame>,
    ) -> HostResult<PreparedFrame<'frame>> {
        Ok(match self {
            Self::Native(route) => PreparedFrame::Native(route.prepare(event_loop, frame)?),
            Self::MainOnly(renderer) => {
                PreparedFrame::MainOnly(frame.render(renderer.renderer_consumer()?))
            }
        })
    }
    fn render_main(
        &mut self,
        frame: PreparedFrame<'_>,
        pass: &mut wgpu::RenderPass<'_>,
        extent: FramebufferExtent,
    ) -> HostResult<()> {
        match (self, frame) {
            (Self::Native(route), PreparedFrame::Native(frame)) => {
                route.render_main(frame, pass, extent)?
            }
            (Self::MainOnly(renderer), PreparedFrame::MainOnly(frame)) => {
                renderer.render(frame, pass, extent)?
            }
            _ => {
                return Err(io::Error::other("renderer changed during a frame transaction").into());
            }
        }
        Ok(())
    }
    fn set_viewport_clear_color(&self, color: wgpu::Color) -> HostResult<()> {
        if let Self::Native(route) = self {
            route.set_viewport_clear_color(color)?;
        }
        Ok(())
    }
    fn register_external_texture(
        &mut self,
        view: &wgpu::TextureView,
    ) -> HostResult<dear_imgui_wgpu::ExternalTextureId> {
        Ok(match self {
            Self::Native(route) => route.register_external_texture(view)?,
            Self::MainOnly(renderer) => renderer.register_external_texture(view)?,
        })
    }
    fn update_external_texture(
        &mut self,
        texture: dear_imgui_wgpu::ExternalTextureId,
        view: &wgpu::TextureView,
    ) -> HostResult<()> {
        match self {
            Self::Native(route) => route.update_external_texture(texture, view)?,
            Self::MainOnly(renderer) => renderer.update_external_texture(texture, view)?,
        }
        Ok(())
    }
    fn unregister_external_texture(
        &mut self,
        texture: dear_imgui_wgpu::ExternalTextureId,
    ) -> HostResult<()> {
        match self {
            Self::Native(route) => route.unregister_external_texture(texture)?,
            Self::MainOnly(renderer) => renderer.unregister_external_texture(texture)?,
        }
        Ok(())
    }
    fn shutdown(&mut self, context: &mut Context) -> HostResult<()> {
        match self {
            Self::Native(route) => route.shutdown(context)?,
            Self::MainOnly(renderer) => renderer.shutdown(context)?,
        }
        Ok(())
    }
}

struct Gpu {
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    route: SurfaceRenderer,
    effect_factory: ViewportEffectsFactory,
    scene_generation: u64,
    error_handlers: DeviceErrorHandlers,
    reconfigure_next_frame: bool,
    effects: ShaderManager,
    image_textures: Vec<wgpu::Texture>,
    plugin_textures: HashMap<bed_plugin::TextureHandle, PluginGpuTexture>,
    generation: u64,
}

struct PluginGpuTexture {
    external: dear_imgui_wgpu::ExternalTextureId,
    target: RenderTarget,
    rendered_revision: Option<u64>,
}

impl Gpu {
    fn new(
        window: Arc<Window>,
        context: &mut Context,
        platform: &WinitPlatform,
    ) -> HostResult<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle(
            Box::new(Arc::clone(&window)),
        ));
        let surface = instance.create_surface(Arc::clone(&window))?;
        let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }))?;
        let (device, queue) =
            block_on(adapter.request_device(&renderer_device_descriptor(&adapter)))?;
        let size = window.inner_size();
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .ok_or_else(|| io::Error::other("GPU cannot present to the bEd window"))?;
        let capabilities = surface.get_capabilities(&adapter);
        if let Some(format) = capabilities
            .formats
            .iter()
            .copied()
            .find(wgpu::TextureFormat::is_srgb)
        {
            config.format = format;
        }
        // NED disables swap-interval pacing and caps the application loop from
        // settings. Fall back to a supported surface mode on each backend.
        config.present_mode = wgpu::PresentMode::AutoNoVsync;
        if capabilities.usages.contains(wgpu::TextureUsages::COPY_SRC) {
            config.usage |= wgpu::TextureUsages::COPY_SRC;
        }
        surface.configure(&device, &config);
        let error_handlers = DeviceErrorHandlers::default();
        error_handlers.install(&device);
        let effect_factory = ViewportEffectsFactory::default();
        let mut viewport_config = WgpuViewportSurfaceConfig::from(&config);
        viewport_config.output_format = Some(config.format);
        viewport_config.additional_usage = config.usage & wgpu::TextureUsages::COPY_SRC;
        let renderer = WgpuRenderer::new(
            WgpuInitInfo::new(device.clone(), queue.clone(), OFFSCREEN_FORMAT)
                .with_instance(instance.clone())
                .with_adapter(adapter.clone())
                .with_viewport_surface_config(viewport_config)
                .with_viewport_postprocessor(std::rc::Rc::new(effect_factory.clone())),
            context,
        )?;
        let route = if platform.viewports_enabled() {
            SurfaceRenderer::Native(
                WinitViewportRoute::attach(context, platform, renderer)
                    .map_err(|error| io::Error::other(error.to_string()))?,
            )
        } else {
            SurfaceRenderer::MainOnly(Box::new(renderer))
        };
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let effects = ShaderManager::new(&device, config.width, config.height, config.format);
        if let Some(error) = block_on(validation.pop()) {
            return Err(io::Error::other(error.to_string()).into());
        }
        eprintln!(
            "bEd: renderer ready ({:?}, {:?}, {}×{}, scale {})",
            adapter.get_info().backend,
            config.format,
            size.width,
            size.height,
            window.scale_factor()
        );
        Ok(Self {
            instance,
            adapter,
            surface,
            device,
            queue,
            config,
            route,
            effect_factory,
            scene_generation: 0,
            error_handlers,
            reconfigure_next_frame: false,
            effects,
            image_textures: Vec::new(),
            plugin_textures: HashMap::new(),
            generation: {
                static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            },
        })
    }

    fn upload_rgba(&mut self, image: &crate::util::icons::RgbaImage) -> HostResult<TextureId> {
        let texture = self.create_rgba_texture(image.width, image.height, &image.pixels)?;
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let texture_id = self.route.register_external_texture(&view)?.texture_id();
        self.image_textures.push(texture);
        Ok(texture_id)
    }

    fn create_rgba_texture(
        &self,
        width: u32,
        height: u32,
        pixels: &[u8],
    ) -> HostResult<wgpu::Texture> {
        let expected = u64::from(width) * u64::from(height) * 4;
        let maximum = self.device.limits().max_texture_dimension_2d;
        if width == 0
            || height == 0
            || expected != pixels.len() as u64
            || width > maximum
            || height > maximum
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "RGBA image dimensions do not match its pixels or exceed the GPU texture limit",
            )
            .into());
        }
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Bed RGBA image"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            texture.size(),
        );
        Ok(texture)
    }

    /// Run between frame transactions: no viewport can still refer to a removed view.
    fn sync_plugin_textures(&mut self, workbench: &mut Workbench) -> HostResult<()> {
        let outputs = workbench.plugin_render_outputs();
        let live: HashSet<_> = outputs.iter().map(|output| output.handle).collect();
        let removed: Vec<_> = self
            .plugin_textures
            .keys()
            .filter(|handle| !live.contains(handle))
            .copied()
            .collect();
        for handle in removed {
            if let Some(texture) = self.plugin_textures.remove(&handle) {
                self.route.unregister_external_texture(texture.external)?;
            }
        }
        workbench.retain_plugin_textures(&live);
        for output in outputs {
            let handle = output.handle;
            if let Some(texture) = self.plugin_textures.get(&handle)
                && texture.target.size == output.size
                && texture.target.depth.is_some() == output.depth
            {
                workbench.set_plugin_texture(handle, texture.external.texture_id());
                continue;
            }
            let target = RenderTarget::new(&self.device, output).map_err(io::Error::other)?;
            let external = if let Some(texture) = self.plugin_textures.get(&handle) {
                let external = texture.external;
                self.route.update_external_texture(external, &target.view)?;
                external
            } else {
                self.route.register_external_texture(&target.view)?
            };
            workbench.set_plugin_texture(handle, external.texture_id());
            self.plugin_textures.insert(
                handle,
                PluginGpuTexture {
                    external,
                    target,
                    rendered_revision: None,
                },
            );
        }
        Ok(())
    }

    fn render_plugin_outputs(&mut self, workbench: &mut Workbench) -> HostResult<()> {
        self.sync_plugin_textures(workbench)?;
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Bed plugin canvases"),
            });
        let mut rendered = false;
        for output in workbench.plugin_render_outputs() {
            let Some(texture) = self.plugin_textures.get_mut(&output.handle) else {
                continue;
            };
            if texture.rendered_revision == Some(output.revision) {
                continue;
            }
            let mut gpu = GpuContext {
                instance: &self.instance,
                adapter: &self.adapter,
                device: &self.device,
                queue: &self.queue,
                encoder: &mut encoder,
                error_handlers: Some(&self.error_handlers),
                generation: self.generation,
            };
            let result = workbench.render_plugin_output(output.handle, &mut gpu, &texture.target);
            // Embedded engines may install their own device callbacks at startup.
            // Keep later canvases, UI submission and recovery owned by this host.
            self.error_handlers.install(&self.device);
            match result {
                Ok(()) => {
                    texture.rendered_revision = Some(output.revision);
                    rendered = true;
                }
                Err(error) => workbench.error = Some(error),
            }
        }
        if rendered {
            self.queue.submit([encoder.finish()]);
        }
        Ok(())
    }

    fn upload_frame_assets(&mut self, frame: &mut Workbench) -> HostResult<()> {
        frame.icons.textures.clear();
        for (key, image) in &frame.icons.images {
            frame
                .icons
                .textures
                .insert(key.clone(), self.upload_rgba(image)?);
        }
        Ok(())
    }

    fn resize(&mut self, size: PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 {
            return;
        }
        self.config.width = size.width;
        self.config.height = size.height;
        self.surface.configure(&self.device, &self.config);
        self.effects
            .initialize_framebuffers(&self.device, size.width, size.height);
    }

    fn acquire(&mut self, window: &Arc<Window>) -> HostResult<Option<wgpu::SurfaceTexture>> {
        if self.reconfigure_next_frame {
            self.resize(window.inner_size());
            self.reconfigure_next_frame = false;
        }
        match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) => Ok(Some(frame)),
            wgpu::CurrentSurfaceTexture::Suboptimal(frame) => {
                // Present this valid frame; reconfigure before the next acquisition.
                self.reconfigure_next_frame = true;
                Ok(Some(frame))
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                Ok(None)
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.resize(window.inner_size());
                Ok(None)
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                eprintln!("bEd: recreating lost window surface");
                self.surface = self.instance.create_surface(Arc::clone(window))?;
                self.resize(window.inner_size());
                Ok(None)
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                Err(io::Error::other("GPU surface validation failed").into())
            }
        }
    }
}

struct Runtime {
    #[cfg(target_os = "macos")]
    native_menu: crate::util::macos_menu::MacOsMenu,
    #[cfg(target_os = "macos")]
    native_window: crate::util::macos_window::MacOsWindow,
    #[cfg(target_os = "windows")]
    native_window: crate::util::windows_window::WindowsWindow,
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    viewport_themes: std::collections::HashMap<WindowId, ([f32; 4], [f32; 4])>,
    window: Arc<Window>,
    gpu: Gpu,
    platform: WinitPlatform,
    context: Context,
    workbench: Workbench,
    rendered_frames: u32,
    frame_pending: bool,
    focus_needs_frame: bool,
    secondary_presentations: u64,
    shutdown_done: bool,
    capture_path: Option<PathBuf>,
    viewport_capture_prefix: Option<PathBuf>,
    capture_after_frames: u32,
    capture_completed: bool,
    debugger_smoke: bool,
    debugger_ready: bool,
    lifecycle: Option<LifecycleSmoke>,
    appearance: Option<AppearanceSmoke>,
    #[cfg(target_os = "macos")]
    native_smoke: Option<NativeAppearanceSmoke>,
    #[cfg(target_os = "macos")]
    menu_smoke: Option<MenuEditSmoke>,
    viewport_smoke: Option<ViewportSmoke>,
    plugin_smoke: Option<PluginSmoke>,
    effects_enabled_override: Option<bool>,
    started: Instant,
}

#[derive(Clone, Default)]
struct RuntimeOptions {
    capture_path: Option<PathBuf>,
    capture_after_frames: Option<u32>,
    lifecycle: bool,
    appearance: bool,
    native_appearance: bool,
    menu_edit: bool,
    debugger: bool,
    plugins: bool,
    viewports: bool,
    main_only: bool,
    effects_enabled_override: Option<bool>,
}
const PLUGIN_SMOKE_BYTES: &[u8] = &[0, 255, 13, 10, 239, 187, 191, 128, 0, 1];
struct PluginSmoke {
    image: PathBuf,
    binary: PathBuf,
    model: PathBuf,
    embedded_model: PathBuf,
    model_capture: Option<PathBuf>,
    model_size: [u32; 2],
    secondary_before: u64,
    original_handle: Option<bed_plugin::TextureHandle>,
    phase: u8,
}
struct AppearanceSmoke {
    original: serde_json::Value,
    phase: u8,
}
#[cfg(target_os = "macos")]
struct NativeAppearanceSmoke {
    opacity: serde_json::Value,
    blur: serde_json::Value,
    panel_counts: [usize; 6],
    document_path: Option<String>,
    split_source_panel: Option<u64>,
    document_panels_before_split: usize,
    phase: u8,
}
#[cfg(target_os = "macos")]
struct MenuEditSmoke {
    original: Vec<u8>,
    edited: Vec<u8>,
    keyboard_save_received: bool,
    instances: Vec<std::process::Child>,
    phase: u8,
}
struct ViewportSmoke {
    phase: u8,
    panel: Option<u64>,
    presentations_before: u64,
    restore_at: Instant,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LifecyclePhase {
    Initial,
    Resizing,
    Minimized,
    Restoring,
    Complete,
}
struct LifecycleSmoke {
    phase: LifecyclePhase,
    deadline: Instant,
    restore_at: Instant,
    frames_at_transition: u32,
    target: PhysicalSize<u32>,
    saw_resize: bool,
    saw_minimized: bool,
    saw_focus_loss: bool,
    saw_focus_gain: bool,
}
impl LifecycleSmoke {
    fn new() -> Self {
        Self {
            phase: LifecyclePhase::Initial,
            deadline: Instant::now() + Duration::from_secs(15),
            restore_at: Instant::now(),
            frames_at_transition: 0,
            target: PhysicalSize::new(0, 0),
            saw_resize: false,
            saw_minimized: false,
            saw_focus_loss: false,
            saw_focus_gain: false,
        }
    }
}

impl Runtime {
    fn new(
        event_loop: &ActiveEventLoop,
        mut workbench: Workbench,
        options: RuntimeOptions,
    ) -> HostResult<Self> {
        let attributes = Window::default_attributes()
            .with_title(workbench.window_title())
            .with_inner_size(LogicalSize::new(1200.0, 800.0));
        #[cfg(target_os = "macos")]
        let attributes = {
            use winit::platform::macos::WindowAttributesExtMacOS;
            attributes
                .with_transparent(true)
                .with_titlebar_transparent(true)
                .with_title_hidden(true)
                .with_fullsize_content_view(true)
                .with_has_shadow(true)
        };
        #[cfg(target_os = "windows")]
        let attributes = attributes.with_decorations(false);
        let window = Arc::new(event_loop.create_window(attributes)?);
        let mut context = Context::create();
        // Tests use disposable configuration and no persisted desktop geometry.
        context.set_ini_filename(
            if options.viewports
                || options.menu_edit
                || options.plugins
                || options.lifecycle
                || options.appearance
                || options.native_appearance
                || options.capture_path.is_some()
            {
                None
            } else {
                Some(workbench.settings.config_dir.join("workspace.ini"))
            },
        )?;
        if let Ok(clipboard) = arboard::Clipboard::new() {
            context.set_clipboard_backend(NativeClipboard(clipboard));
        }
        workbench.initialize(&mut context, WorkbenchHostMode::Fullscreen)?;
        let mut platform = WinitPlatform::new(&mut context)?;
        platform.attach_window(Arc::clone(&window), HiDpiMode::Default, &mut context)?;
        platform.set_ime_auto_management(false);
        let native_viewports = !options.main_only
            && (!cfg!(target_os = "linux") || std::env::var_os("DISPLAY").is_some());
        if native_viewports {
            let flags = context.io().config_flags() | dear_imgui_rs::ConfigFlags::VIEWPORTS_ENABLE;
            context.io_mut().set_config_flags(flags);
            platform.enable_viewports(&mut context)?;
            context.io_mut().set_config_viewports_no_auto_merge(true);
        } else {
            if options.viewports {
                return Err(io::Error::new(io::ErrorKind::Unsupported,"Detached native windows require X11/XWayland; this Wayland session supports internal docking and floating panels").into());
            }
            eprintln!(
                "bEd: using internal docking/floating panels; native detached windows disabled for this platform/test"
            );
        }
        window.set_ime_allowed(true);
        let mut gpu = Gpu::new(Arc::clone(&window), &mut context, &platform)?;
        let viewport_capture_prefix = options
            .viewports
            .then(|| options.capture_path.clone())
            .flatten();
        if viewport_capture_prefix.is_some() {
            gpu.effect_factory.enable_capture();
        }
        #[cfg(target_os = "macos")]
        let mut native_window = crate::util::macos_window::MacOsWindow::configure(
            &window,
            workbench.settings.number("mac_background_opacity", 0.5),
            workbench.settings.bool("mac_blur_enabled", true),
        )?;
        #[cfg(target_os = "windows")]
        let native_window =
            crate::util::windows_window::WindowsWindow::configure(Arc::clone(&window))?;
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            workbench.root_top_inset = native_window.titlebar_inset();
        }
        #[cfg(target_os = "macos")]
        let mut native_menu = crate::util::macos_menu::MacOsMenu::install(&workbench.settings)?;
        #[cfg(target_os = "macos")]
        {
            native_window.set_commands(&workbench.toolbar_commands())?;
            native_menu.set_plugin_commands(&workbench.application_commands())?;
        }
        gpu.upload_frame_assets(&mut workbench)?;
        #[cfg(target_os = "macos")]
        let menu_smoke = if options.menu_edit {
            let original = workbench
                .active_snapshot()
                .ok_or_else(|| io::Error::other("menu fixture has no document"))?
                .bytes;
            let mut edited = b"NATIVE_EDIT_".to_vec();
            edited.extend_from_slice(&original);
            Some(MenuEditSmoke {
                original,
                edited,
                keyboard_save_received: false,
                instances: Vec::new(),
                phase: 0,
            })
        } else {
            None
        };
        let appearance = options.appearance.then(|| AppearanceSmoke {
            original: workbench.settings.settings.clone(),
            phase: 0,
        });
        #[cfg(target_os = "macos")]
        let native_smoke = options.native_appearance.then(|| NativeAppearanceSmoke {
            opacity: workbench.settings.settings["mac_background_opacity"].clone(),
            blur: workbench.settings.settings["mac_blur_enabled"].clone(),
            panel_counts: [0; 6],
            document_path: workbench.active_snapshot().map(|document| document.path),
            split_source_panel: None,
            document_panels_before_split: 0,
            phase: 0,
        });
        let plugin_smoke = options.plugins.then(|| PluginSmoke {
            image: PathBuf::from(&workbench.project_root).join("image.png"),
            binary: PathBuf::from(&workbench.project_root).join("bytes.bin"),
            model: PathBuf::from(&workbench.project_root).join("cube.glb"),
            embedded_model: PathBuf::from(&workbench.project_root).join("cube.gltf"),
            model_capture: options
                .capture_path
                .as_ref()
                .map(|path| path.with_file_name("gltf.ppm")),
            model_size: [0; 2],
            secondary_before: 0,
            original_handle: None,
            phase: 0,
        });
        Ok(Self {
            #[cfg(target_os = "macos")]
            native_menu,
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            native_window,
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            viewport_themes: Default::default(),
            window,
            gpu,
            platform,
            context,
            workbench,
            rendered_frames: 0,
            frame_pending: true,
            focus_needs_frame: false,
            secondary_presentations: 0,
            shutdown_done: false,
            capture_path: options.capture_path,
            viewport_capture_prefix,
            capture_after_frames: options.capture_after_frames.unwrap_or(2),
            capture_completed: false,
            debugger_smoke: options.debugger,
            debugger_ready: false,
            lifecycle: options.lifecycle.then(LifecycleSmoke::new),
            appearance,
            #[cfg(target_os = "macos")]
            native_smoke,
            #[cfg(target_os = "macos")]
            menu_smoke,
            plugin_smoke,
            viewport_smoke: options.viewports.then(|| ViewportSmoke {
                phase: 0,
                panel: None,
                presentations_before: 0,
                restore_at: Instant::now(),
            }),
            effects_enabled_override: options.effects_enabled_override,
            started: Instant::now(),
        })
    }
    fn windows(&self) -> HostResult<Vec<dear_imgui_winit::multi_viewport::ViewportWindow>> {
        if self.platform.viewports_enabled() {
            Ok(self.platform.owned_viewport_windows()?)
        } else {
            let viewport_id = self
                .context
                .binding()
                .with_bound_context(|| unsafe { (*dear_imgui_rs::sys::igGetMainViewport()).ID });
            Ok(vec![dear_imgui_winit::multi_viewport::ViewportWindow {
                viewport_id,
                window: Arc::clone(&self.window),
                is_main: true,
            }])
        }
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    fn update_viewport_themes(&mut self) -> HostResult<()> {
        let windows = self.windows()?;
        self.viewport_themes.retain(|id, _| {
            windows
                .iter()
                .any(|viewport| !viewport.is_main && viewport.window.id() == *id)
        });
        let theme = (
            self.workbench.settings.text_color(),
            self.workbench.settings.background_color(),
        );
        for viewport in windows.into_iter().filter(|viewport| !viewport.is_main) {
            let id = viewport.window.id();
            if self.viewport_themes.get(&id) == Some(&theme) {
                continue;
            }
            #[cfg(target_os = "macos")]
            crate::util::macos_window::MacOsWindow::apply_theme_to_window(
                &viewport.window,
                theme.0,
                theme.1,
            )?;
            #[cfg(target_os = "windows")]
            crate::util::windows_window::WindowsWindow::apply_theme_to_window(
                &viewport.window,
                theme.0,
                theme.1,
            )?;
            self.viewport_themes.insert(id, theme);
        }
        Ok(())
    }
    fn redraw(&mut self, event_loop: &ActiveEventLoop) -> HostResult<bool> {
        if let Err(error) = self.workbench.tick() {
            self.workbench.error = Some(error.to_string());
        }
        if self.debugger_smoke {
            self.debugger_ready = self.workbench.debug_smoke_ready()?;
            if !self.debugger_ready && self.rendered_frames > 1800 {
                return Err(io::Error::other(
                    "Debugger smoke timed out before inspection was ready",
                )
                .into());
            }
        }
        if let Some(error) = self.gpu.error_handlers.take_error() {
            self.workbench.error = Some(format!("GPU rendering error: {error}"));
        }
        let lost = self.gpu.error_handlers.take_device_lost();
        if let Some(message) = lost {
            eprintln!("bEd: recreating GPU after device loss: {message}");
            self.gpu.route.shutdown(&mut self.context)?;
            // Existing secondary windows have already run Renderer_CreateWindow.
            // Recreate platform ownership so the replacement renderer receives
            // those callbacks again; ImGui retains the panels and their layout.
            if self.platform.viewports_enabled() {
                self.platform.disable_viewports(&mut self.context)?;
                self.platform.enable_viewports(&mut self.context)?;
            }
            self.gpu = Gpu::new(Arc::clone(&self.window), &mut self.context, &self.platform)?;
            if self.viewport_capture_prefix.is_some() {
                self.gpu.effect_factory.enable_capture();
            }
            self.workbench.invalidate_plugin_textures();
            self.gpu.upload_frame_assets(&mut self.workbench)?;
        }
        if let Err(error) = self.gpu.sync_plugin_textures(&mut self.workbench) {
            self.workbench.error = Some(error.to_string());
        }
        self.workbench.apply_settings(&mut self.context)?;
        let title = self.workbench.window_title();
        if self.window.title() != title {
            self.window.set_title(&title);
        }
        #[cfg(target_os = "macos")]
        {
            self.native_window.set_title(&title);
            self.native_window
                .set_commands(&self.workbench.toolbar_commands())?;
            self.native_menu
                .set_plugin_commands(&self.workbench.application_commands())?;
            self.native_menu.update(
                &self.workbench.settings,
                self.workbench.active_document().is_some(),
                self.workbench.focused_terminal(),
                self.context.io().want_text_input(),
            )?;
            self.native_window.update(
                self.workbench
                    .settings
                    .number("mac_background_opacity", 0.5),
                self.workbench.settings.bool("mac_blur_enabled", true),
            )?;
            let text = self.workbench.settings.text_color();
            let background = self.workbench.settings.background_color();
            self.native_window.update_theme(text, background)?;
            self.workbench.root_top_inset = self.native_window.titlebar_inset();
            for id in self.native_window.take_command_ids() {
                self.workbench.dispatch_command(&id)?;
            }
        }
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        self.update_viewport_themes()?;
        self.platform
            .prepare_frame(&mut self.context, &self.window)?;
        let frame = self.context.begin_frame();
        let ui = frame.ui();
        #[cfg(target_os = "windows")]
        {
            let commands = self.workbench.toolbar_commands();
            let application_commands = self.workbench.application_commands();
            let toolbar_actions = self.native_window.draw_titlebar_commands_with_menu(
                ui,
                &self.workbench.settings,
                &self.workbench.icons,
                &title,
                &commands,
                &application_commands,
            );
            for id in toolbar_actions {
                self.workbench.dispatch_command(&id)?;
            }
            self.workbench.root_top_inset = self.native_window.titlebar_inset();
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            let toolbar = self.workbench.toolbar_commands();
            let application = self.workbench.application_commands();
            let mut command_ids = Vec::new();
            if let Some(_bar) = ui.begin_main_menu_bar() {
                if let Some(_tools) = ui.begin_menu("Tools") {
                    for command in &application {
                        if ui.menu_item_enabled_selected_no_shortcut(
                            &command.label,
                            false,
                            command.enabled,
                        ) {
                            command_ids.push(command.id.clone());
                        }
                    }
                }
                for command in &toolbar {
                    let _id = ui.push_id(&command.id);
                    let _disabled = ui.begin_disabled_with_cond(!command.enabled);
                    if ui.small_button(&command.label) {
                        command_ids.push(command.id.clone());
                    }
                }
            }
            for id in command_ids {
                self.workbench.dispatch_command(&id)?;
            }
        }
        let actions = self.workbench.render(ui)?;
        // Output textures must be ready before prepare() submits secondary windows.
        if let Err(error) = self.gpu.render_plugin_outputs(&mut self.workbench) {
            self.workbench.error = Some(error.to_string());
        }
        self.platform.prepare_render(ui, &self.window)?;
        let mut effect_settings = self.workbench.settings.shader_settings();
        if let Some(enabled) = self.effects_enabled_override {
            effect_settings.enabled = enabled;
        }
        let time = self.started.elapsed().as_secs_f32();
        let generation = self.workbench.scene_generation();
        if generation != self.gpu.scene_generation {
            self.gpu.effects.invalidate_history();
            self.gpu.scene_generation = generation;
        }
        self.gpu
            .effect_factory
            .update(effect_settings, time, generation);
        let background = self.workbench.settings.background_color();
        let clear = wgpu::Color {
            r: f64::from(background[0]),
            g: f64::from(background[1]),
            b: f64::from(background[2]),
            a: f64::from(background[3]),
        };
        self.gpu.route.set_viewport_clear_color(clear)?;

        // The route completes every secondary surface before acquiring the main
        // one. A minimized or temporarily unavailable main window cannot stop it.
        let prepared = self.gpu.route.prepare(event_loop, frame)?;
        self.secondary_presentations += prepared.secondary_presentations() as u64;
        for capture in self
            .gpu
            .effect_factory
            .finish_captures()
            .map_err(io::Error::other)?
        {
            if let Some(prefix) = &self.viewport_capture_prefix {
                let stem = prefix.file_stem().unwrap_or_default().to_string_lossy();
                let path = prefix.with_file_name(format!("{stem}-secondary-{}.ppm", capture.id));
                write_ppm(
                    &path,
                    &capture.pixels,
                    capture.width,
                    capture.height,
                    capture.stride,
                    capture.format,
                )?;
                eprintln!(
                    "bEd: captured secondary final CRT output {}×{} to {}",
                    capture.width,
                    capture.height,
                    path.display()
                );
            }
        }
        let size = self.window.inner_size();
        let surface_frame =
            if size.width == 0 || size.height == 0 || self.window.is_minimized() == Some(true) {
                None
            } else {
                self.gpu.acquire(&self.window)?
            };
        if let Some(surface_frame) = surface_frame {
            let view = surface_frame
                .texture
                .create_view(&wgpu::TextureViewDescriptor::default());
            let mut encoder =
                self.gpu
                    .device
                    .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                        label: Some("Bed frame"),
                    });
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Bed main viewport"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &self.gpu.effects.fb.view,
                        resolve_target: None,
                        depth_slice: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(clear),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                });
                self.gpu.route.render_main(
                    prepared,
                    &mut pass,
                    FramebufferExtent::from_texture(&self.gpu.effects.fb.texture),
                )?;
            }
            self.gpu.effects.render_with_effects(
                &mut encoder,
                &self.gpu.queue,
                &view,
                time,
                &effect_settings,
            );
            let capture_now = self.capture_path.is_some()
                && (!self.debugger_smoke || self.debugger_ready)
                && self.rendered_frames >= self.capture_after_frames.saturating_sub(1)
                && self
                    .appearance
                    .as_ref()
                    .is_none_or(|smoke| smoke.phase == 3)
                && self
                    .lifecycle
                    .as_ref()
                    .is_none_or(|life| life.phase == LifecyclePhase::Complete);
            let readback = if capture_now {
                if !self
                    .gpu
                    .config
                    .usage
                    .contains(wgpu::TextureUsages::COPY_SRC)
                {
                    return Err(io::Error::other(
                        "The surface does not support GPU screenshot readback",
                    )
                    .into());
                }
                let extent = surface_frame.texture.size();
                let row_stride = (extent.width * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
                    * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
                let buffer = self.gpu.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("Bed screenshot readback"),
                    size: u64::from(row_stride) * u64::from(extent.height),
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                });
                encoder.copy_texture_to_buffer(
                    wgpu::TexelCopyTextureInfo {
                        texture: &surface_frame.texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::TexelCopyBufferInfo {
                        buffer: &buffer,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(row_stride),
                            rows_per_image: Some(extent.height),
                        },
                    },
                    extent,
                );
                Some((buffer, row_stride, extent))
            } else {
                None
            };
            self.gpu.queue.submit([encoder.finish()]);
            surface_frame.present();
            if let Some((buffer, stride, extent)) = readback {
                let (sender, receiver) = std::sync::mpsc::channel();
                buffer
                    .slice(..)
                    .map_async(wgpu::MapMode::Read, move |result| {
                        let _ = sender.send(result);
                    });
                self.gpu.device.poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: Some(Duration::from_secs(10)),
                })?;
                receiver.recv_timeout(Duration::from_secs(10))??;
                let bytes = buffer.slice(..).get_mapped_range();
                let path = self.capture_path.take().expect("capture was requested");
                write_ppm(
                    &path,
                    &bytes,
                    extent.width,
                    extent.height,
                    stride,
                    self.gpu.config.format,
                )?;
                drop(bytes);
                buffer.unmap();
                self.capture_completed = true;
                eprintln!(
                    "bEd: captured {}×{} GPU frame to {}",
                    extent.width,
                    extent.height,
                    path.display()
                );
            }
        } else {
            drop(prepared);
        }
        self.rendered_frames += 1;
        self.focus_needs_frame = false;
        self.advance_lifecycle_after_frame();
        // Settings changes are applied between frames, and never alter fonts
        // while any viewport owns the current frame token.
        self.advance_appearance()?;
        #[cfg(target_os = "macos")]
        {
            self.advance_native_appearance_after_frame()?;
            self.advance_menu_edit_after_frame()?;
        }
        self.advance_viewport_smoke()?;
        self.advance_plugin_smoke()?;
        for action in actions {
            self.workbench.handle_action(action)?;
        }
        Ok(true)
    }
    fn advance_appearance(&mut self) -> HostResult<()> {
        let Some(mut smoke) = self.appearance.take() else {
            return Ok(());
        };
        let result = self.advance_appearance_settings(&mut smoke);
        self.appearance = Some(smoke);
        result
    }
    fn advance_appearance_settings(&mut self, smoke: &mut AppearanceSmoke) -> HostResult<()> {
        let settings = &mut self.workbench.settings;
        match (smoke.phase, self.rendered_frames) {
            (0, 2..) => {
                settings.settings["font"] = serde_json::json!("JetBrainsMonoNL-Regular");
                settings.settings["fontSize"] = serde_json::json!(24.0);
                settings.settings["treesitter"] = serde_json::json!(true);
                settings.request_apply();
                smoke.phase = 1;
            }
            (1, 4..) => {
                settings.settings =
                    crate::util::settings::read_json(&settings.config_dir.join("amber.json"))?;
                settings.request_apply();
                smoke.phase = 2;
            }
            (2, 6..) => {
                settings.settings = smoke.original.clone();
                settings.request_apply();
                smoke.phase = 3;
                eprintln!("bEd: appearance smoke restored font/profile settings");
            }
            _ => {}
        }
        Ok(())
    }
    #[cfg(target_os = "macos")]
    fn advance_native_appearance_after_frame(&mut self) -> HostResult<()> {
        use crate::util::macos_window::TitlebarAction;
        let Some(smoke) = &mut self.native_smoke else {
            return Ok(());
        };
        if !self.native_window.preserves_winit_content_view() || !self.native_menu.is_installed() {
            return Err(io::Error::other("native window/menu ownership changed").into());
        }
        self.native_window.validate_control_layout()?;
        match (smoke.phase, self.rendered_frames) {
            (0, 2..) => {
                self.native_menu
                    .perform_for_smoke(crate::util::macos_menu::MenuAction::Find)?;
                smoke.phase = 1;
            }
            (1, 4..) => {
                if self.workbench.active_overlay() != bed_core::editor_events::Overlay::Find {
                    return Err(
                        io::Error::other("native menu Find did not target active view").into(),
                    );
                }
                smoke.panel_counts = [
                    "explorer",
                    "terminal",
                    "settings",
                    "search",
                    "diagnostics",
                    "structure",
                ]
                .map(|kind| self.workbench.panel_count(kind));
                for _ in 0..2 {
                    for action in [
                        TitlebarAction::Sidebar,
                        TitlebarAction::Terminal,
                        TitlebarAction::Settings,
                        TitlebarAction::Search,
                        TitlebarAction::Diagnostics,
                        TitlebarAction::Structure,
                    ] {
                        self.native_window.click_control(action);
                    }
                }
                self.workbench.settings.settings["mac_background_opacity"] =
                    serde_json::json!(0.35);
                self.workbench.settings.settings["mac_blur_enabled"] = serde_json::json!(false);
                smoke.phase = 2;
            }
            (2, 6..) => {
                if self.native_window.material_is_visible()
                    || self
                        .native_window
                        .content_opacity()
                        .is_none_or(|v| (v - 0.35).abs() > 0.001)
                {
                    return Err(
                        io::Error::other("native appearance opacity/blur reload failed").into(),
                    );
                }
                self.workbench.settings.settings["mac_background_opacity"] = smoke.opacity.clone();
                self.workbench.settings.settings["mac_blur_enabled"] = smoke.blur.clone();
                for (kind, before) in [
                    "explorer",
                    "terminal",
                    "settings",
                    "search",
                    "diagnostics",
                    "structure",
                ]
                .into_iter()
                .zip(smoke.panel_counts)
                {
                    if self.workbench.panel_count(kind) != before + 2 {
                        return Err(io::Error::other(format!(
                            "native titlebar did not create two distinct {kind} panels"
                        ))
                        .into());
                    }
                }
                for action in [
                    crate::util::macos_menu::MenuAction::NewExplorer,
                    crate::util::macos_menu::MenuAction::NewTerminal,
                    crate::util::macos_menu::MenuAction::NewSettings,
                    crate::util::macos_menu::MenuAction::NewContentSearch,
                    crate::util::macos_menu::MenuAction::NewDiagnostics,
                    crate::util::macos_menu::MenuAction::NewStructure,
                ] {
                    self.native_menu.perform_for_smoke(action)?;
                }
                smoke.phase = 3;
            }
            (3, 8..) => {
                for (kind, before) in [
                    "explorer",
                    "terminal",
                    "settings",
                    "search",
                    "diagnostics",
                    "structure",
                ]
                .into_iter()
                .zip(smoke.panel_counts)
                {
                    if self.workbench.panel_count(kind) != before + 3 {
                        return Err(io::Error::other(format!(
                            "native Window menu did not create another {kind} panel"
                        ))
                        .into());
                    }
                }
                if let Some(path) = &smoke.document_path {
                    self.workbench.open_or_focus(Path::new(path))?;
                }
                smoke.document_panels_before_split = self.workbench.panel_count("document");
                smoke.split_source_panel = self.workbench.active_panel_id();
                self.native_window.click_control(TitlebarAction::SplitRight);
                smoke.phase = 4;
            }
            (4, 10..) => {
                validate_native_split(
                    &self.context,
                    smoke.split_source_panel,
                    self.workbench.active_panel_id(),
                    WindowCommand::SplitRight,
                )?;
                if self.workbench.panel_count("document") != smoke.document_panels_before_split + 1
                {
                    return Err(io::Error::other(
                        "native Split Right did not create a document view",
                    )
                    .into());
                }
                smoke.split_source_panel = self.workbench.active_panel_id();
                self.native_window.click_control(TitlebarAction::SplitDown);
                smoke.phase = 5;
            }
            (5, 12..) => {
                validate_native_split(
                    &self.context,
                    smoke.split_source_panel,
                    self.workbench.active_panel_id(),
                    WindowCommand::SplitDown,
                )?;
                if self.workbench.panel_count("document") != smoke.document_panels_before_split + 2
                {
                    return Err(io::Error::other(
                        "native Split Down did not create a document view",
                    )
                    .into());
                }
                eprintln!(
                    "bEd: native menu/titlebar/material smoke passed; six tools created three panels each, both split controls created distinct document panes; toolbar frames {:?}",
                    self.native_window.control_frames()
                );
                smoke.phase = 6;
            }
            _ => {}
        }
        Ok(())
    }
    #[cfg(target_os = "macos")]
    fn advance_menu_edit_after_frame(&mut self) -> HostResult<()> {
        use crate::util::macos_menu::MenuAction;
        let Some(smoke) = &mut self.menu_smoke else {
            return Ok(());
        };
        if smoke.phase < 11 && self.started.elapsed() > Duration::from_secs(30) {
            return Err(io::Error::other(format!(
                "native menu smoke timed out in phase {}",
                smoke.phase
            ))
            .into());
        }
        let snapshot = self
            .workbench
            .active_snapshot()
            .ok_or_else(|| io::Error::other("menu fixture document missing"))?;
        match (smoke.phase, self.rendered_frames) {
            (0, 2..) => {
                self.workbench.with_active_view(|editor| {
                    editor.api().close_all_overlays();
                    editor.commands().set_cursor(
                        0,
                        0,
                        false,
                        bed_core::editor_commands::CursorReveal::Ensure,
                    );
                    editor.view_mut().request_focus = true;
                })?;
                let mut options = self.workbench.session.options().clone();
                options.autosave = Some(Duration::ZERO);
                self.workbench.session.configure(options)?;
                self.window.focus_window();
                smoke.phase = 1;
            }
            (1, 4..) => {
                let blocked = self
                    .workbench
                    .with_active_view(|editor| editor.view.block_input)?
                    .unwrap_or(true);
                if !self.window.has_focus() || blocked {
                    return Ok(());
                };
                self.context
                    .io_mut()
                    .add_input_characters_utf8("NATIVE_EDIT_");
                smoke.phase = 2;
            }
            (2, 6..) => {
                if snapshot.bytes != smoke.edited {
                    return Err(
                        io::Error::other("menu fixture typing did not reach active view").into(),
                    );
                }
                if snapshot.dirty || std::fs::read(&snapshot.path)? != smoke.edited {
                    return Ok(());
                };
                self.native_menu.perform_for_smoke(MenuAction::Undo)?;
                smoke.phase = 3;
            }
            (3, 8..) => {
                if snapshot.bytes != smoke.original {
                    return Err(io::Error::other("NSMenu Undo failed after autosave").into());
                }
                if snapshot.dirty || std::fs::read(&snapshot.path)? != smoke.original {
                    return Ok(());
                };
                self.native_menu.perform_for_smoke(MenuAction::Redo)?;
                smoke.phase = 4;
            }
            (4, 10..) => {
                if snapshot.bytes != smoke.edited {
                    return Err(io::Error::other("NSMenu Redo failed after autosave").into());
                }
                if snapshot.dirty || std::fs::read(&snapshot.path)? != smoke.edited {
                    return Ok(());
                };
                self.native_menu.perform_for_smoke(MenuAction::Save)?;
                smoke.phase = 5;
            }
            (5, 12..) => {
                self.native_menu.key_equivalent_for_smoke("s", 1, false)?;
                smoke.phase = 6;
            }
            (6, 14..) => {
                let cursor_visible = self
                    .workbench
                    .with_active_view(|editor| {
                        !editor.view.block_input && !editor.view.selections.is_empty()
                    })?
                    .unwrap_or(false);
                if !smoke.keyboard_save_received
                    || snapshot.bytes != smoke.edited
                    || std::fs::read(&snapshot.path)? != smoke.edited
                    || !cursor_visible
                {
                    return Err(io::Error::other(
                        "native Cmd+S did not save the active document and preserve cursor focus",
                    )
                    .into());
                }
                smoke.phase = 7;
                eprintln!(
                    "bEd: native menu typing/autosave/Undo/Redo/keyboard Save passed for the active shared document"
                );
            }
            (7, 16..) => {
                // Exercise the keyboard while this app still has focus; child
                // processes may become the active application when they launch.
                self.native_menu.key_equivalent_for_smoke("n", 45, true)?;
                smoke.phase = 8;
            }
            (8, 18..) => {
                if smoke.instances.is_empty() {
                    return Ok(());
                }
                self.native_menu.perform_for_smoke(MenuAction::NewWindow)?;
                smoke.phase = 9;
            }
            (9, 20..) => {
                if smoke.instances.len() < 2 {
                    return Ok(());
                }
                self.native_menu.perform_dock_new_window_for_smoke()?;
                smoke.phase = 10;
            }
            (10, 22..) => {
                if smoke.instances.len() < 3 {
                    return Ok(());
                }
                if smoke.instances.len() != 3 {
                    return Err(io::Error::other(
                        "File, Dock and shortcut actions must each launch one instance",
                    )
                    .into());
                }
                for child in &mut smoke.instances {
                    match child.try_wait()? {
                        Some(status) if status.success() => {}
                        Some(status) => {
                            return Err(
                                io::Error::other(format!("new instance failed: {status}")).into()
                            );
                        }
                        None => return Ok(()),
                    }
                }
                smoke.phase = 11;
                eprintln!(
                    "bEd: File, Dock and Cmd+Shift+N launched separate instances and all exited cleanly"
                );
            }
            _ => {}
        }
        Ok(())
    }
    #[cfg(target_os = "macos")]
    fn process_native_menu(&mut self) -> HostResult<bool> {
        use crate::util::macos_menu::MenuAction;
        // NewFrame reconciles the native focused viewport before the workspace
        // identifies its active panel. Keep menu events queued until that frame.
        if self.focus_needs_frame {
            return Ok(false);
        }
        for dispatch in self.native_menu.poll() {
            let action = dispatch.action;
            if action == MenuAction::Quit {
                return Ok(true);
            }
            if action == MenuAction::NewWindow {
                let mut command = new_instance_command(
                    &std::env::current_exe()?,
                    &self.workbench.settings.config_dir,
                    self.menu_smoke.is_some(),
                )?;
                if self.menu_smoke.is_some() {
                    command.arg("--main-only-smoke");
                }
                let mut child = command.spawn()?;
                if let Some(smoke) = &mut self.menu_smoke {
                    smoke.instances.push(child);
                } else {
                    std::thread::Builder::new()
                        .name("bed-instance-wait".into())
                        .spawn(move || {
                            let _ = child.wait();
                        })?;
                }
                continue;
            }
            if matches!(action, MenuAction::Save | MenuAction::SaveAs)
                && dispatch.keyboard
                && let Some(smoke) = &mut self.menu_smoke
            {
                smoke.keyboard_save_received = true;
            }
            if matches!(
                action,
                MenuAction::Undo
                    | MenuAction::Redo
                    | MenuAction::Cut
                    | MenuAction::Copy
                    | MenuAction::Paste
                    | MenuAction::SelectAll
            ) {
                let terminal = self.workbench.focused_terminal();
                let route = if dispatch.keyboard {
                    crate::util::macos_menu::input_shortcut(action, &self.workbench.settings)
                        .map(|(key, shift)| NativeEditRoute::Shortcut(key, shift || dispatch.shift))
                        .unwrap_or(NativeEditRoute::Ignore)
                } else {
                    native_edit_route(action, terminal, self.context.io().want_text_input())
                };
                match route {
                    NativeEditRoute::Ignore => {}
                    NativeEditRoute::SelectAllDocument => {
                        if self
                            .workbench
                            .focused_plugin_action(bed_plugin::PanelAction::SelectAll)?
                        {
                            continue;
                        }
                        if self.workbench.focused_hex() {
                            queue_native_edit_shortcut(
                                &mut self.context,
                                dear_imgui_rs::Key::A,
                                false,
                            );
                        } else if self
                            .workbench
                            .active_view()
                            .and_then(|view| self.workbench.session.document_for_view(view))
                            == self.workbench.active_document()
                        {
                            self.workbench
                                .with_active_view(|editor| editor.commands().select_all())?;
                        }
                    }
                    NativeEditRoute::Shortcut(key, shift) => {
                        queue_native_edit_shortcut(&mut self.context, key, shift)
                    }
                }
                continue;
            }
            if let Some(command) = native_menu_command(action, dispatch.keyboard) {
                self.workbench.dispatch(command)?;
            }
        }
        for id in self.native_menu.poll_plugin_commands() {
            self.workbench.dispatch_command(&id)?;
        }
        Ok(false)
    }
    fn advance_viewport_smoke(&mut self) -> HostResult<()> {
        let Some(smoke) = &mut self.viewport_smoke else {
            return Ok(());
        };
        if smoke.phase < 4 && self.started.elapsed() > Duration::from_secs(25) {
            return Err(io::Error::other(format!(
                "viewport smoke timed out in phase {}",
                smoke.phase
            ))
            .into());
        }
        let windows = self.platform.owned_viewport_windows()?;
        match (smoke.phase, self.rendered_frames) {
            (0, 4..) => {
                self.workbench.dispatch(WindowCommand::DuplicateView)?;
                let panel = self
                    .workbench
                    .active_panel_id()
                    .ok_or_else(|| io::Error::other("duplicate view has no panel"))?;
                let outer = self.window.outer_position().unwrap_or_default();
                let coordinate_scale = if cfg!(target_os = "macos") {
                    self.window.scale_factor() as f32
                } else {
                    1.0
                };
                self.workbench.detach_panel_for_smoke(
                    panel,
                    [
                        outer.x as f32 / coordinate_scale + 130.0,
                        outer.y as f32 / coordinate_scale + 100.0,
                    ],
                );
                smoke.panel = Some(panel);
                smoke.phase = 1;
                eprintln!("bEd: viewport smoke detached shared view panel {panel}");
            }
            (1, 6..) => {
                if let Some(secondary) = windows.iter().find(|window| !window.is_main) {
                    eprintln!(
                        "bEd: viewport smoke found native secondary {}",
                        secondary.viewport_id
                    );
                    secondary.window.set_ime_allowed(true);
                    secondary.window.focus_window();
                    let _ = secondary
                        .window
                        .request_inner_size(LogicalSize::new(680.0, 500.0));
                    smoke.presentations_before = self.secondary_presentations;
                    self.window.set_minimized(true);
                    smoke.restore_at = Instant::now() + Duration::from_millis(600);
                    smoke.phase = 2;
                }
            }
            (2, _) => {
                if self.window.is_minimized() != Some(true) || Instant::now() < smoke.restore_at {
                    return Ok(());
                };
                if self.secondary_presentations <= smoke.presentations_before {
                    return Err(io::Error::other(
                        "secondary did not present while primary minimized",
                    )
                    .into());
                }
                self.window.set_minimized(false);
                self.window.focus_window();
                smoke.phase = 3;
                eprintln!(
                    "bEd: independent secondary CRT presentation continued while main minimized"
                );
            }
            (3, 12..) => {
                if self.window.is_minimized() == Some(true) {
                    return Ok(());
                };
                let panel = smoke.panel.unwrap();
                let viewport = self
                    .workbench
                    .panel_viewport_id(panel)
                    .ok_or_else(|| io::Error::other("detached panel lost viewport"))?;
                if !self.workbench.close_viewport(viewport)? {
                    return Err(io::Error::other("clean secondary group refused close").into());
                }
                smoke.phase = 4;
                eprintln!(
                    "bEd: detached native window resize/focus/minimize/group-close smoke passed"
                );
            }
            _ => {}
        }
        Ok(())
    }
    fn advance_lifecycle_after_frame(&mut self) {
        let Some(life) = &mut self.lifecycle else {
            return;
        };
        match life.phase {
            LifecyclePhase::Initial if self.rendered_frames >= 2 => {
                let target = LogicalSize::new(840.0, 600.0);
                life.target = target.to_physical(self.window.scale_factor());
                life.frames_at_transition = self.rendered_frames;
                life.phase = LifecyclePhase::Resizing;
                let _ = self.window.request_inner_size(target);
                eprintln!(
                    "bEd: lifecycle requesting native resize to {}×{}",
                    life.target.width, life.target.height
                );
            }
            LifecyclePhase::Resizing
                if life.saw_resize && self.rendered_frames >= life.frames_at_transition + 2 =>
            {
                let configured = (self.gpu.config.width, self.gpu.config.height);
                self.gpu.resize(PhysicalSize::new(0, 0));
                assert_eq!(configured, (self.gpu.config.width, self.gpu.config.height));
                life.phase = LifecyclePhase::Minimized;
                life.restore_at = Instant::now() + Duration::from_millis(700);
                life.frames_at_transition = self.rendered_frames;
                self.window.set_minimized(true);
                eprintln!(
                    "bEd: lifecycle requesting native minimize; zero-size configure guard passed"
                );
            }
            LifecyclePhase::Restoring
                if life.saw_focus_gain
                    && self.window.is_minimized() == Some(false)
                    && self.rendered_frames >= life.frames_at_transition + 2 =>
            {
                life.phase = LifecyclePhase::Complete;
                eprintln!(
                    "bEd: lifecycle native resize, minimize, restore, focus and redraw passed"
                );
            }
            _ => {}
        }
    }

    fn drive_lifecycle(&mut self) -> HostResult<()> {
        let Some(life) = &mut self.lifecycle else {
            return Ok(());
        };
        if life.phase != LifecyclePhase::Complete && Instant::now() > life.deadline {
            return Err(io::Error::other(format!("Native lifecycle smoke timed out in {:?} (resize {}, minimized {}, focus loss {}, focus gain {})",
                life.phase, life.saw_resize, life.saw_minimized, life.saw_focus_loss, life.saw_focus_gain)).into());
        }
        if life.phase == LifecyclePhase::Minimized {
            if self.window.is_minimized() == Some(true) {
                life.saw_minimized = true;
            }
            if life.saw_minimized && life.saw_focus_loss && Instant::now() >= life.restore_at {
                self.window.set_minimized(false);
                self.window.focus_window();
                life.phase = LifecyclePhase::Restoring;
                life.frames_at_transition = self.rendered_frames;
                eprintln!(
                    "bEd: lifecycle observed native minimized state and focus loss; restoring"
                );
            }
        }
        Ok(())
    }

    fn advance_plugin_smoke(&mut self) -> HostResult<()> {
        let Some(mut smoke) = self.plugin_smoke.take() else {
            return Ok(());
        };
        let result = self.advance_plugin_fixture(&mut smoke);
        self.plugin_smoke = Some(smoke);
        result
    }
    fn advance_plugin_fixture(&mut self, smoke: &mut PluginSmoke) -> HostResult<()> {
        if smoke.phase < 15 && self.started.elapsed() > Duration::from_secs(45) {
            return Err(io::Error::other(format!(
                "Plugin smoke timed out in phase {}: {:?}",
                smoke.phase, self.workbench.error
            ))
            .into());
        }
        let images = self.workbench.plugin_render_outputs();
        match smoke.phase {
            0 if images.len() == 1 && self.gpu.plugin_textures.len() == 1 => {
                if self.workbench.panel_count("bed.image.panel") != 1 {
                    return Err(
                        io::Error::other("PNG extension did not select image plugin").into(),
                    );
                }
                smoke.original_handle = Some(images[0].handle);
                #[cfg(target_os = "macos")]
                {
                    self.native_menu
                        .perform_plugin_for_smoke("bed.structure.open")?;
                    self.process_native_menu()?;
                    if self.workbench.panel_count("structure") != 1 {
                        return Err(io::Error::other(
                            "Native Tools menu did not dispatch plugin command",
                        )
                        .into());
                    }
                    self.workbench.dispatch(WindowCommand::Close)?;
                    if !self.native_window.click_command("bed.structure.open") {
                        return Err(io::Error::other("Plugin toolbar command is missing").into());
                    }
                    for id in self.native_window.take_command_ids() {
                        self.workbench.dispatch_command(&id)?;
                    }
                    if self.workbench.panel_count("structure") != 1 {
                        return Err(io::Error::other(
                            "Native toolbar did not dispatch plugin command",
                        )
                        .into());
                    }
                    self.workbench.dispatch(WindowCommand::Close)?;
                }
                self.workbench.open_or_focus(&smoke.binary)?;
                smoke.phase = 1;
            }
            1 => {
                let document = self
                    .workbench
                    .active_snapshot()
                    .ok_or_else(|| io::Error::other("Binary fixture is not active"))?;
                if self.workbench.panel_count("hex") != 1 || document.bytes != PLUGIN_SMOKE_BYTES {
                    return Err(io::Error::other(
                        "Binary fallback did not preserve exact bytes in hex editor",
                    )
                    .into());
                }
                self.workbench.open_or_focus(&smoke.image)?;
                smoke.phase = 2;
            }
            2 if self.capture_path.is_none() && self.rendered_frames >= 16 => {
                self.workbench.dispatch(WindowCommand::Close)?;
                smoke.phase = 3;
            }
            3 if images.is_empty() && self.gpu.plugin_textures.is_empty() => {
                self.workbench.open_or_focus(&smoke.image)?;
                smoke.phase = 4;
            }
            4 if images.len() == 1 && self.gpu.plugin_textures.len() == 1 => {
                if Some(images[0].handle) == smoke.original_handle {
                    return Err(
                        io::Error::other("Closed image panel retained its texture handle").into(),
                    );
                }
                self.gpu
                    .error_handlers
                    .report_device_lost("plugin smoke recovery fixture");
                smoke.phase = 5;
            }
            5 => {
                if images.len() != 1 || self.gpu.plugin_textures.len() != 1 {
                    return Err(io::Error::other(
                        "Plugin texture was not restored after GPU recreation",
                    )
                    .into());
                }
                self.workbench.dispatch(WindowCommand::Close)?;
                self.workbench.open_or_focus(&smoke.binary)?;
                self.workbench.dispatch(WindowCommand::Close)?;
                smoke.phase = 6;
            }
            6 if images.is_empty() && self.gpu.plugin_textures.is_empty() => {
                if self.workbench.panel_count("bed.image.panel") != 0
                    || self.workbench.panel_count("hex") != 0
                    || std::fs::read(&smoke.binary)? != PLUGIN_SMOKE_BYTES
                {
                    return Err(io::Error::other(
                        "Plugin/hex close leaked a panel or changed the fixture bytes",
                    )
                    .into());
                }
                smoke.phase = 7;
                self.workbench.open_or_focus(&smoke.model)?;
            }
            7 if images.len() == 1 && self.gpu.plugin_textures.len() == 1 => {
                if self.workbench.panel_count("bed.gltf.panel") != 1 || !images[0].depth {
                    return Err(io::Error::other(
                        "GLB did not select the depth-enabled glTF plugin output",
                    )
                    .into());
                }
                smoke.model_size = images[0].size;
                self.capture_path = smoke.model_capture.take();
                // Allow Bevy asset preparation and temporal AO to settle before
                // recording the model viewer's native presentation.
                self.capture_after_frames = self.rendered_frames + 20;
                smoke.phase = 8;
            }
            8 if self.capture_path.is_none() => {
                let _ = self
                    .window
                    .request_inner_size(PhysicalSize::new(1800, 1200));
                smoke.phase = 9;
            }
            9 if images.len() == 1 && images[0].size != smoke.model_size => {
                if matches!(self.gpu.route, SurfaceRenderer::Native(_)) {
                    let panel = self
                        .workbench
                        .active_panel_id()
                        .ok_or_else(|| io::Error::other("glTF panel is not active"))?;
                    self.workbench.detach_panel_for_smoke(panel, [140.0, 140.0]);
                }
                smoke.secondary_before = self.secondary_presentations;
                smoke.phase = 10;
            }
            10 if !matches!(self.gpu.route, SurfaceRenderer::Native(_))
                || self.secondary_presentations > smoke.secondary_before + 4 =>
            {
                self.gpu
                    .error_handlers
                    .report_device_lost("glTF smoke recovery fixture");
                smoke.phase = 11;
            }
            11 if images.len() == 1 && self.gpu.plugin_textures.len() == 1 => {
                if !images[0].depth {
                    return Err(io::Error::other(
                        "glTF depth target was lost during GPU recreation",
                    )
                    .into());
                }
                self.workbench.open_or_focus(&smoke.model)?;
                self.workbench.dispatch(WindowCommand::Close)?;
                smoke.phase = 12;
            }
            12 if images.is_empty() && self.gpu.plugin_textures.is_empty() => {
                self.workbench.open_or_focus(&smoke.embedded_model)?;
                smoke.phase = 13;
            }
            13 if images.len() == 1 && self.gpu.plugin_textures.len() == 1 => {
                self.workbench.dispatch(WindowCommand::Close)?;
                smoke.phase = 14;
            }
            14 if images.is_empty() && self.gpu.plugin_textures.is_empty() => {
                if self.workbench.panel_count("bed.gltf.panel") != 0 {
                    return Err(io::Error::other("Closed glTF panel leaked").into());
                }
                smoke.phase = 15;
                eprintln!(
                    "bEd: plugin smoke verified image/glTF GPU outputs, GLB/embedded glTF routing, resize, detached presentation, close/reopen and GPU recovery"
                );
            }
            _ => {}
        }
        Ok(())
    }
    fn smoke_complete(&self, smoke_test: bool) -> bool {
        if self.debugger_smoke && !self.debugger_ready {
            return false;
        }
        if self
            .plugin_smoke
            .as_ref()
            .is_some_and(|smoke| smoke.phase < 15)
        {
            return false;
        }
        if self
            .viewport_smoke
            .as_ref()
            .is_some_and(|smoke| smoke.phase < 4)
        {
            return false;
        }
        #[cfg(target_os = "macos")]
        if self
            .menu_smoke
            .as_ref()
            .is_some_and(|smoke| smoke.phase < 11)
        {
            return false;
        }
        #[cfg(target_os = "macos")]
        if self
            .native_smoke
            .as_ref()
            .is_some_and(|smoke| smoke.phase < 6)
        {
            return false;
        }
        if self.capture_path.is_some() {
            return false;
        }
        if self.appearance.is_some() && self.rendered_frames < 8 {
            return false;
        }
        if let Some(life) = &self.lifecycle {
            life.phase == LifecyclePhase::Complete && self.capture_path.is_none()
        } else {
            (smoke_test && self.rendered_frames >= 2) || self.capture_completed
        }
    }

    fn shutdown(&mut self) -> HostResult<()> {
        if self.shutdown_done {
            return Ok(());
        }
        self.workbench.cleanup()?;
        self.gpu.route.shutdown(&mut self.context)?;
        self.platform.shutdown(&mut self.context)?;
        self.shutdown_done = true;
        Ok(())
    }
}

struct Bed {
    workbench: Option<Workbench>,
    runtime: Option<Runtime>,
    error: Option<String>,
    closing: bool,
    smoke_test: bool,
    options: RuntimeOptions,
    next_frame: Instant,
}
impl Bed {
    fn fail(&mut self, event_loop: &ActiveEventLoop, error: impl std::fmt::Display) {
        self.error = Some(error.to_string());
        self.closing = true;
        event_loop.exit();
    }
    fn request_close(&mut self, event_loop: &ActiveEventLoop) {
        let Some(runtime) = &mut self.runtime else {
            self.closing = true;
            event_loop.exit();
            return;
        };
        match runtime.workbench.request_close_all() {
            Ok(true) => {
                self.closing = true;
                event_loop.exit();
            }
            Ok(false) => {}
            Err(error) => runtime.workbench.error = Some(error.to_string()),
        }
    }
}
impl ApplicationHandler for Bed {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.runtime.is_none() {
            let Some(workbench) = self.workbench.take() else {
                return;
            };
            match Runtime::new(event_loop, workbench, self.options.clone()) {
                Ok(runtime) => {
                    runtime.window.request_redraw();
                    self.runtime = Some(runtime);
                }
                Err(error) => self.fail(event_loop, error),
            }
        }
    }
    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if self.closing {
            return;
        }
        let Some(runtime) = &mut self.runtime else {
            return;
        };
        let windows = match runtime.windows() {
            Ok(windows) => windows,
            Err(error) => {
                self.fail(event_loop, error);
                return;
            }
        };
        let Some(native) = windows.into_iter().find(|native| native.window.id() == id) else {
            return;
        };
        if matches!(event, WindowEvent::CloseRequested) {
            if native.is_main {
                self.request_close(event_loop);
                return;
            }
            match runtime.workbench.close_viewport(native.viewport_id) {
                Ok(true) => {}
                Ok(false) => return,
                Err(error) => {
                    runtime.workbench.error = Some(error.to_string());
                    return;
                }
            }
        }
        if let Err(error) =
            runtime
                .platform
                .handle_window_event(&mut runtime.context, &native.window, &event)
        {
            self.fail(event_loop, error);
            return;
        }
        match event {
            WindowEvent::Resized(size) => {
                if native.is_main {
                    runtime.gpu.resize(size);
                    if let Some(life) = &mut runtime.lifecycle
                        && life.phase == LifecyclePhase::Resizing
                        && size == life.target
                    {
                        life.saw_resize = true;
                    }
                }
                runtime.window.request_redraw();
            }
            WindowEvent::Focused(focused) => {
                if focused {
                    runtime.context.binding().with_bound_context(|| {
                        runtime.workbench.focus_viewport(native.viewport_id);
                    });
                    runtime.focus_needs_frame = true;
                    runtime.frame_pending = true;
                }
                if native.is_main
                    && let Some(life) = &mut runtime.lifecycle
                {
                    if !focused && life.phase == LifecyclePhase::Minimized {
                        life.saw_focus_loss = true;
                    }
                    if focused && life.phase == LifecyclePhase::Restoring {
                        life.saw_focus_gain = true;
                    }
                }
                runtime.window.request_redraw();
            }
            WindowEvent::ScaleFactorChanged { .. } => {
                if native.is_main {
                    runtime.gpu.resize(runtime.window.inner_size());
                }
                runtime.window.request_redraw();
            }
            WindowEvent::RedrawRequested => {
                // Secondary redraws are serviced by the same context transaction;
                // throttle duplicate notifications through the application clock.
                if !runtime.frame_pending && Instant::now() < self.next_frame {
                    return;
                }
                runtime.frame_pending = false;
                match runtime.redraw(event_loop) {
                    Ok(_) if runtime.smoke_complete(self.smoke_test) => {
                        eprintln!(
                            "bEd: smoke test rendered {} frames / {} secondary presentations; closing cleanly",
                            runtime.rendered_frames, runtime.secondary_presentations
                        );
                        self.closing = true;
                        event_loop.exit();
                    }
                    Ok(_) => {}
                    Err(error) => self.fail(event_loop, error),
                }
            }
            WindowEvent::DroppedFile(path) => {
                let result = if path.is_dir() {
                    runtime.workbench.set_project(&path)
                } else {
                    runtime.workbench.open_or_focus(&path)
                };
                if let Err(error) = result {
                    runtime.workbench.error = Some(error.to_string());
                }
            }
            _ => {}
        }
    }
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.closing {
            return;
        }
        #[cfg(target_os = "macos")]
        if let Some(runtime) = &mut self.runtime {
            match runtime.process_native_menu() {
                Ok(true) => {
                    self.request_close(event_loop);
                    return;
                }
                Ok(false) => {}
                Err(error) => runtime.workbench.error = Some(error.to_string()),
            }
        }
        let now = Instant::now();
        if let Some(runtime) = &mut self.runtime {
            if let Err(error) = runtime.workbench.tick() {
                runtime.workbench.error = Some(error.to_string());
            }
            if now >= self.next_frame {
                if let Err(error) = runtime.drive_lifecycle() {
                    self.fail(event_loop, error);
                    return;
                }
                // Request redraws from every owned window. A secondary remains a
                // frame driver even when the primary is minimized/zero-sized.
                let windows = match runtime.windows() {
                    Ok(windows) => windows,
                    Err(error) => {
                        self.fail(event_loop, error);
                        return;
                    }
                };
                let focused = windows.iter().any(|native| native.window.has_focus());
                runtime.frame_pending = true;
                for native in windows {
                    if native.window.is_minimized() != Some(true) {
                        native.window.request_redraw();
                    }
                }
                let settings = &runtime.workbench.settings;
                let fps = if focused {
                    settings.number("fps_target", 60.0)
                } else {
                    settings.number("fps_target_unfocused", 30.0)
                };
                if settings.bool("fps_toggle", true) && fps > 0.0 && fps < 900.0 {
                    self.next_frame = now + Duration::from_secs_f64(1.0 / f64::from(fps));
                } else {
                    self.next_frame = now;
                    event_loop.set_control_flow(ControlFlow::Poll);
                    return;
                }
            }
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_frame));
    }
    fn exiting(&mut self, _: &ActiveEventLoop) {
        if let Some(runtime) = &mut self.runtime {
            if let Some(smoke) = &runtime.appearance {
                runtime.workbench.settings.settings = smoke.original.clone();
            }
            if let Err(error) = runtime.shutdown() {
                self.error.get_or_insert_with(|| error.to_string());
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn new_instance_command(
    executable: &Path,
    config_dir: &Path,
    wait: bool,
) -> io::Result<std::process::Command> {
    let config_dir = std::fs::canonicalize(config_dir)?;
    // Launch Services needs -n to start another process for a running bundle.
    // Source builds run the executable directly. Neither path inherits the
    // parent's project arguments or smoke flags.
    let bundle = executable
        .parent()
        .filter(|path| path.file_name().is_some_and(|name| name == "MacOS"))
        .and_then(Path::parent)
        .filter(|path| path.file_name().is_some_and(|name| name == "Contents"))
        .and_then(Path::parent)
        .filter(|path| path.extension().is_some_and(|extension| extension == "app"));
    let mut command = if let Some(bundle) = bundle {
        let mut command = std::process::Command::new("/usr/bin/open");
        command.arg("-n");
        if wait {
            command.arg("-W");
        }
        command.arg(bundle).arg("--args");
        command
    } else {
        std::process::Command::new(executable)
    };
    command
        .arg("--new-window")
        .arg("--config-dir")
        .arg(config_dir)
        .stdin(std::process::Stdio::null());
    use std::os::unix::process::CommandExt;
    command.process_group(0);
    if !wait {
        command
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
    }
    Ok(command)
}

#[cfg(target_os = "macos")]
fn validate_native_split(
    context: &Context,
    source: Option<u64>,
    destination: Option<u64>,
    direction: WindowCommand,
) -> io::Result<()> {
    use dear_imgui_rs::sys;
    use std::ffi::CString;
    context.binding().with_bound_context(|| {
        let rect = |panel: Option<u64>| -> io::Result<(u32, [f32; 4])> {
            let panel =
                panel.ok_or_else(|| io::Error::other("split has no active document panel"))?;
            let name = CString::new(format!("###bed_tab_{panel}")).unwrap();
            // Read native docking geometry only while this context is bound.
            unsafe {
                let window = sys::igFindWindowByName(name.as_ptr());
                if window.is_null()
                    || (*window).DockNode.is_null()
                    || !sys::ImGuiDockNode_IsLeafNode((*window).DockNode)
                    || !(*window).DockTabIsVisible()
                {
                    return Err(io::Error::other(
                        "split document is not visible in a dock leaf",
                    ));
                }
                let rect = sys::ImGuiDockNode_Rect((*window).DockNode);
                Ok((
                    (*window).DockId,
                    [rect.Min.x, rect.Min.y, rect.Max.x, rect.Max.y],
                ))
            }
        };
        let (source_dock, source) = rect(source)?;
        let (destination_dock, destination) = rect(destination)?;
        let positioned = match direction {
            WindowCommand::SplitRight => destination[0] >= source[2] - 1.0,
            WindowCommand::SplitDown => destination[1] >= source[3] - 1.0,
            _ => false,
        };
        if source_dock == destination_dock || !positioned {
            return Err(io::Error::other(format!(
                "native split geometry failed: {source:?} -> {destination:?}"
            )));
        }
        Ok(())
    })
}

#[cfg(target_os = "macos")]
fn native_menu_command(
    action: crate::util::macos_menu::MenuAction,
    keyboard: bool,
) -> Option<WindowCommand> {
    use crate::util::macos_menu::MenuAction;
    Some(match action {
        MenuAction::NewDocument => WindowCommand::NewDocument,
        MenuAction::NewTerminal => WindowCommand::NewTerminal,
        MenuAction::NewExplorer => WindowCommand::NewExplorer,
        MenuAction::NewSettings => WindowCommand::NewSettings,
        MenuAction::NewProjects | MenuAction::Projects => WindowCommand::NewProjects,
        MenuAction::NewDiagnostics | MenuAction::Diagnostics => WindowCommand::NewDiagnostics,
        MenuAction::Debug => WindowCommand::Debug,
        MenuAction::Structure | MenuAction::NewStructure => WindowCommand::NewStructure,
        MenuAction::NewReferences => WindowCommand::NewReferences,
        MenuAction::NewLspDashboard | MenuAction::LspDashboard => WindowCommand::NewLspDashboard,
        MenuAction::NewContentSearch => WindowCommand::NewContentSearch,
        MenuAction::DuplicateView => WindowCommand::DuplicateView,
        MenuAction::SplitRight => WindowCommand::SplitRight,
        MenuAction::SplitDown => WindowCommand::SplitDown,
        MenuAction::ResetLayout => WindowCommand::ResetLayout,
        MenuAction::OpenFolder => WindowCommand::OpenFolder,
        MenuAction::OpenFile => WindowCommand::OpenFile,
        MenuAction::Save => WindowCommand::Save,
        MenuAction::SaveAs => WindowCommand::SaveAs,
        MenuAction::Close => WindowCommand::Close,
        MenuAction::Find => WindowCommand::Find,
        MenuAction::GoToLine => WindowCommand::GoToLine,
        MenuAction::FindFile => WindowCommand::FindFile,
        MenuAction::FindProject if keyboard => WindowCommand::FindProject,
        MenuAction::FindProject => WindowCommand::NewContentSearch,
        MenuAction::Explorer if keyboard => WindowCommand::Explorer,
        MenuAction::Explorer => WindowCommand::NewExplorer,
        MenuAction::Terminal if keyboard => WindowCommand::Terminal,
        MenuAction::Terminal => WindowCommand::NewTerminal,
        MenuAction::Settings if keyboard => WindowCommand::Settings,
        MenuAction::Settings => WindowCommand::NewSettings,
        _ => return None,
    })
}

#[cfg(target_os = "macos")]
#[derive(Debug, PartialEq, Eq)]
enum NativeEditRoute {
    Ignore,
    SelectAllDocument,
    Shortcut(dear_imgui_rs::Key, bool),
}
#[cfg(target_os = "macos")]
fn native_edit_route(
    action: crate::util::macos_menu::MenuAction,
    terminal_focused: bool,
    text_input_focused: bool,
) -> NativeEditRoute {
    use crate::util::macos_menu::MenuAction;
    use dear_imgui_rs::Key;
    if terminal_focused
        && matches!(
            action,
            MenuAction::Undo | MenuAction::Redo | MenuAction::Cut | MenuAction::SelectAll
        )
    {
        return NativeEditRoute::Ignore;
    }
    match action {
        MenuAction::Undo => NativeEditRoute::Shortcut(Key::Z, false),
        MenuAction::Redo => NativeEditRoute::Shortcut(Key::Z, true),
        MenuAction::Cut => NativeEditRoute::Shortcut(Key::X, false),
        MenuAction::Copy => NativeEditRoute::Shortcut(Key::C, terminal_focused),
        MenuAction::Paste => NativeEditRoute::Shortcut(Key::V, terminal_focused),
        MenuAction::SelectAll if text_input_focused => NativeEditRoute::Shortcut(Key::A, false),
        MenuAction::SelectAll => NativeEditRoute::SelectAllDocument,
        _ => NativeEditRoute::Ignore,
    }
}

#[cfg(target_os = "macos")]
fn queue_native_edit_shortcut(context: &mut Context, key: dear_imgui_rs::Key, shift: bool) {
    use dear_imgui_rs::Key;
    let io = context.io_mut();
    // Native menus consume the key-equivalent event. Route edit actions through
    // the existing focused ImGui/editor/terminal input handler. Dear ImGui's
    // macOS behavior maps physical Command (Super) to logical Ctrl.
    io.add_key_event(Key::ModSuper, true);
    io.add_key_event(Key::LeftSuper, true);
    if shift {
        io.add_key_event(Key::ModShift, true);
        io.add_key_event(Key::LeftShift, true);
    }
    io.add_key_event(key, true);
    io.add_key_event(key, false);
    if shift {
        io.add_key_event(Key::LeftShift, false);
        io.add_key_event(Key::ModShift, false);
    }
    io.add_key_event(Key::LeftSuper, false);
    io.add_key_event(Key::ModSuper, false);
}

pub fn run() -> HostResult<()> {
    let mut paths = Vec::new();
    let mut resume_workspace = true;
    let mut smoke_test = false;
    let mut options = RuntimeOptions::default();
    let mut config_dir = None;
    let mut arguments = std::env::args_os().skip(1);
    while let Some(argument) = arguments.next() {
        if argument == "--smoke-test" {
            smoke_test = true;
        } else if argument == "--main-only-smoke" {
            options.main_only = true;
            smoke_test = true;
        } else if argument == "--viewports-smoke" {
            options.viewports = true;
            smoke_test = true;
        } else if argument == "--lifecycle-smoke" {
            options.lifecycle = true;
        } else if argument == "--capture-frame" {
            options.capture_path =
                Some(PathBuf::from(arguments.next().ok_or_else(|| {
                    io::Error::other("--capture-frame requires a PPM output path")
                })?));
        } else if argument == "--capture-after-frames" {
            let frames = arguments
                .next()
                .ok_or_else(|| {
                    io::Error::other("--capture-after-frames requires a positive count")
                })?
                .to_string_lossy()
                .parse::<u32>()?;
            if frames == 0 {
                return Err(io::Error::other("--capture-after-frames must be at least 1").into());
            }
            options.capture_after_frames = Some(frames);
        } else if argument == "--effects-smoke" {
            options.effects_enabled_override = Some(true);
            smoke_test = true;
        } else if argument == "--appearance-smoke" {
            options.appearance = true;
            smoke_test = true;
        } else if argument == "--platform-smoke" {
            options.native_appearance = true;
            smoke_test = true;
        } else if argument == "--menu-smoke" {
            options.menu_edit = true;
            smoke_test = true;
        } else if argument == "--debug-smoke" {
            options.debugger = true;
            smoke_test = true;
        } else if argument == "--plugin-smoke" {
            options.plugins = true;
            smoke_test = true;
        } else if argument == "--config-dir" {
            config_dir =
                Some(PathBuf::from(arguments.next().ok_or_else(|| {
                    io::Error::other("--config-dir requires a directory")
                })?));
        } else if argument == "--new-window" {
            resume_workspace = false;
        } else if argument == "--help" || argument == "-h" {
            println!(
                "Usage: bed [FILE_OR_FOLDER] [--smoke-test | --lifecycle-smoke | --effects-smoke]\n           [--appearance-smoke] [--platform-smoke] [--menu-smoke] [--plugin-smoke] [--viewports-smoke]\n           [--debug-smoke] [--main-only-smoke] [--capture-frame OUTPUT.ppm] [--capture-after-frames N]\n           [--config-dir DIRECTORY]\n\nCmd/Ctrl+O open · Cmd/Ctrl+S save · Cmd/Ctrl+F find · Cmd/Ctrl+; go to line\n--menu-smoke uses an isolated temporary document/config to verify autosave and native Undo/Redo."
            );
            return Ok(());
        } else {
            let path = PathBuf::from(argument);
            paths.push(path);
        }
    }
    let _menu_fixture = if options.menu_edit {
        if !cfg!(target_os = "macos") {
            return Err(
                io::Error::new(io::ErrorKind::Unsupported, "--menu-smoke requires macOS").into(),
            );
        }
        if options.native_appearance || options.appearance {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Run --menu-smoke separately from appearance/platform changes",
            )
            .into());
        }
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("bed-native-menu-{}-{unique}", std::process::id()));
        std::fs::create_dir(&root)?;
        let fixture = TemporaryMenuFixture(root);
        let document = fixture.0.join("document.txt");
        std::fs::write(&document, b"native menu baseline\n")?;
        paths.clear();
        paths.push(fixture.0.clone());
        paths.push(document);
        if config_dir.is_none() {
            config_dir = Some(fixture.0.join("config"));
        }
        Some(fixture)
    } else {
        None
    };
    let _plugin_fixture = if options.plugins {
        if options.menu_edit || options.native_appearance || options.appearance || options.viewports
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Run --plugin-smoke separately from other feature smoke fixtures",
            )
            .into());
        }
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("bed-plugin-smoke-{}-{unique}", std::process::id()));
        std::fs::create_dir(&root)?;
        let fixture = TemporaryMenuFixture(root);
        let image = fixture.0.join("image.png");
        std::fs::copy(
            Settings::get_app_resources_path().join("resources/icons/bed.png"),
            &image,
        )?;
        std::fs::write(fixture.0.join("bytes.bin"), PLUGIN_SMOKE_BYTES)?;
        std::fs::write(
            fixture.0.join("cube.glb"),
            include_bytes!("../tests/fixtures/gltf/cube.glb"),
        )?;
        std::fs::write(
            fixture.0.join("cube.gltf"),
            include_bytes!("../tests/fixtures/gltf/cube.gltf"),
        )?;
        paths.clear();
        paths.push(fixture.0.clone());
        paths.push(image);
        if config_dir.is_none() {
            config_dir = Some(fixture.0.join("config"));
        }
        Some(fixture)
    } else {
        None
    };
    let settings = match config_dir {
        Some(path) => Settings::with_paths(path, Settings::get_app_resources_path())?,
        None => Settings::new()?,
    };
    let mut workbench = Workbench::with_settings(settings);
    if paths.is_empty()
        && resume_workspace
        && let Err(error) = workbench.restore_last_workspace()
    {
        workbench.error = Some(format!("Unable to restore workspace: {error}"));
    }
    for path in paths {
        if path.is_dir() {
            workbench.set_project(&path)?;
        } else {
            workbench.open_or_focus(&path)?;
        }
    }
    if options.debugger {
        workbench.debug_smoke_setup()?;
    }
    #[cfg(target_os = "linux")]
    let event_loop = {
        use winit::platform::x11::EventLoopBuilderExtX11;
        // This backend needs global desktop coordinates for detached windows.
        // Select X11 explicitly, including XWayland on Wayland desktops.
        if std::env::var_os("DISPLAY").is_some() {
            EventLoop::builder().with_x11().build()?
        } else {
            EventLoop::new()?
        }
    };
    #[cfg(not(target_os = "linux"))]
    let event_loop = EventLoop::new()?;
    let mut app = Bed {
        workbench: Some(workbench),
        runtime: None,
        error: None,
        closing: false,
        smoke_test,
        options,
        next_frame: Instant::now(),
    };
    event_loop.run_app(&mut app)?;
    if let Some(error) = app.error {
        return Err(io::Error::other(error).into());
    }
    Ok(())
}

struct TemporaryMenuFixture(PathBuf);
impl Drop for TemporaryMenuFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn write_ppm(
    path: &Path,
    bytes: &[u8],
    width: u32,
    height: u32,
    stride: u32,
    format: wgpu::TextureFormat,
) -> io::Result<()> {
    let bgra = match format {
        wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb => true,
        wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Rgba8UnormSrgb => false,
        _ => {
            return Err(io::Error::other(
                "PPM screenshot requires an 8-bit RGBA/BGRA target",
            ));
        }
    };
    let row_bytes = width
        .checked_mul(4)
        .ok_or_else(|| io::Error::other("Screenshot width overflow"))?;
    let byte_count = u64::from(stride) * u64::from(height);
    if stride < row_bytes || byte_count > bytes.len() as u64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Screenshot readback is too short for its extent",
        ));
    }
    let mut output = io::BufWriter::new(std::fs::File::create(path)?);
    writeln!(output, "P6\n{width} {height}\n255")?;
    for row in bytes.chunks_exact(stride as usize).take(height as usize) {
        for pixel in row[..row_bytes as usize].as_chunks::<4>().0 {
            output.write_all(&[
                pixel[if bgra { 2 } else { 0 }],
                pixel[1],
                pixel[if bgra { 0 } else { 2 }],
            ])?;
        }
    }
    output.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn native_edit_shortcuts_undo_and_redo_once_without_inserting_characters() {
        use bed_ui::views::view_layout::ViewLayout;
        use dear_imgui_rs::{Condition, FramePrepareOptions, Key};
        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = Context::create();
        context.set_ini_filename(None::<PathBuf>).unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        let mut editor = Editor::new();
        // A named in-memory document records upstream undo without touching any
        // source file or scheduling a disk save in this input-only fixture.
        editor
            .api()
            .open_document("test://native-menu", b"baseline");
        editor.commands().type_text(b"!");
        let mut input = EditorInput::default();
        let mut render = |context: &mut Context, editor: &mut Editor| {
            context.prepare_frame(FramePrepareOptions::new([640.0, 480.0], 1.0 / 60.0));
            let ui = context.frame();
            ui.window("Native menu editing")
                .position([0.0; 2], Condition::Always)
                .size([640.0, 480.0], Condition::Always)
                .build(|| {
                    ui.set_window_focus(None);
                    assert!(
                        input
                            .process(ui, &mut editor.view_context(), &ViewLayout::default())
                            .is_empty()
                    );
                });
            drop(context.render_legacy());
        };
        render(&mut context, &mut editor);
        assert_eq!(editor.state.join(), b"!baseline");
        queue_native_edit_shortcut(&mut context, Key::Z, false);
        for _ in 0..4 {
            render(&mut context, &mut editor);
        }
        assert_eq!(editor.state.join(), b"baseline");
        queue_native_edit_shortcut(&mut context, Key::Z, true);
        for _ in 0..4 {
            render(&mut context, &mut editor);
        }
        assert_eq!(editor.state.join(), b"!baseline");
        assert!(!context.io().key_ctrl());
        assert!(!context.io().key_super());
        assert!(!context.io().key_shift());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_mouse_menu_copy_and_paste_use_terminal_clipboard_shortcuts() {
        use crate::util::macos_menu::MenuAction;
        use bed_terminal::{
            terminal::{SelectionSnap, Terminal},
            terminal_font::TerminalFonts,
            terminal_view::{TerminalIo, TerminalView},
        };
        use dear_imgui_rs::{Condition, FramePrepareOptions};
        use std::{cell::RefCell, rc::Rc};
        #[derive(Default)]
        struct Pipe(Vec<Vec<u8>>);
        impl TerminalIo for Pipe {
            fn pump(&mut self, _: &mut Terminal) -> io::Result<bool> {
                Ok(false)
            }
            fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
                self.0.push(bytes.to_vec());
                Ok(())
            }
            fn resize(&mut self, _: usize, _: usize, _: f32, _: f32) -> io::Result<()> {
                Ok(())
            }
        }
        struct Clipboard(Rc<RefCell<String>>);
        impl ClipboardBackend for Clipboard {
            fn get(&mut self) -> Option<String> {
                Some(self.0.borrow().clone())
            }
            fn set(&mut self, text: &str) {
                *self.0.borrow_mut() = text.into();
            }
        }
        fn render(
            context: &mut Context,
            view: &mut TerminalView,
            terminal: &mut Terminal,
            pipe: &mut Pipe,
        ) {
            context.prepare_frame(FramePrepareOptions::new([640.0, 480.0], 1.0 / 60.0));
            let ui = context.frame();
            let font = ui.current_font();
            let fonts = TerminalFonts {
                regular: Some(font),
                bold: Some(font),
                italic: Some(font),
                bold_italic: Some(font),
                size: 13.0,
                ..TerminalFonts::default()
            };
            ui.window("Native menu terminal canvas")
                .position([0.0; 2], Condition::Always)
                .size([640.0, 480.0], Condition::Always)
                .build(|| {
                    ui.set_window_focus(None);
                    ui.set_keyboard_focus_here();
                    view.draw(ui, terminal, &fonts, pipe).unwrap();
                });
            drop(context.render_legacy());
        }
        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = Context::create();
        context.set_ini_filename(None::<PathBuf>).unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        let clipboard = Rc::new(RefCell::new(String::new()));
        context.set_clipboard_backend(Clipboard(Rc::clone(&clipboard)));
        let mut view = TerminalView::default();
        let mut terminal = Terminal::new(80, 24);
        let mut pipe = Pipe::default();
        for _ in 0..2 {
            render(&mut context, &mut view, &mut terminal, &mut pipe);
        }
        terminal.feed(b"abc");
        terminal.select_start(0, 0, SelectionSnap::None);
        terminal.select_extend(2, 0, false, false);
        terminal.select_extend(2, 0, false, true);
        let NativeEditRoute::Shortcut(key, shift) =
            native_edit_route(MenuAction::Copy, true, false)
        else {
            panic!("copy must route to terminal input")
        };
        queue_native_edit_shortcut(&mut context, key, shift);
        for _ in 0..4 {
            render(&mut context, &mut view, &mut terminal, &mut pipe);
        }
        assert_eq!(&*clipboard.borrow(), "abc");
        assert!(
            pipe.0.is_empty(),
            "mouse Copy must never send Ctrl-C to the shell"
        );
        terminal.feed(b"\x1b[?2004h");
        *clipboard.borrow_mut() = "raw\n\x1b[31m".into();
        let NativeEditRoute::Shortcut(key, shift) =
            native_edit_route(MenuAction::Paste, true, false)
        else {
            panic!("paste must route to terminal input")
        };
        queue_native_edit_shortcut(&mut context, key, shift);
        for _ in 0..4 {
            render(&mut context, &mut view, &mut terminal, &mut pipe);
        }
        assert_eq!(pipe.0.concat(), b"\x1b[200~raw\n\x1b[31m\x1b[201~");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_panel_clicks_create_and_configured_shortcuts_reveal() {
        use crate::util::macos_menu::MenuAction;
        for (action, create, reveal) in [
            (
                MenuAction::Explorer,
                WindowCommand::NewExplorer,
                WindowCommand::Explorer,
            ),
            (
                MenuAction::Terminal,
                WindowCommand::NewTerminal,
                WindowCommand::Terminal,
            ),
            (
                MenuAction::Settings,
                WindowCommand::NewSettings,
                WindowCommand::Settings,
            ),
            (
                MenuAction::FindProject,
                WindowCommand::NewContentSearch,
                WindowCommand::FindProject,
            ),
        ] {
            assert_eq!(native_menu_command(action, false), Some(create));
            assert_eq!(native_menu_command(action, true), Some(reveal));
        }
        for (action, command) in [
            (MenuAction::NewExplorer, WindowCommand::NewExplorer),
            (MenuAction::NewTerminal, WindowCommand::NewTerminal),
            (MenuAction::NewSettings, WindowCommand::NewSettings),
            (MenuAction::Projects, WindowCommand::NewProjects),
            (MenuAction::Diagnostics, WindowCommand::NewDiagnostics),
            (MenuAction::Structure, WindowCommand::NewStructure),
            (MenuAction::NewStructure, WindowCommand::NewStructure),
            (MenuAction::NewReferences, WindowCommand::NewReferences),
            (MenuAction::LspDashboard, WindowCommand::NewLspDashboard),
        ] {
            assert_eq!(native_menu_command(action, false), Some(command));
            assert_eq!(native_menu_command(action, true), Some(command));
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_edit_actions_respect_terminal_and_text_input_ownership() {
        use crate::util::macos_menu::MenuAction;
        use dear_imgui_rs::Key;
        for action in [
            MenuAction::Undo,
            MenuAction::Redo,
            MenuAction::Cut,
            MenuAction::SelectAll,
        ] {
            assert_eq!(
                native_edit_route(action, true, false),
                NativeEditRoute::Ignore
            );
        }
        assert_eq!(
            native_edit_route(MenuAction::Copy, true, false),
            NativeEditRoute::Shortcut(Key::C, true)
        );
        assert_eq!(
            native_edit_route(MenuAction::Paste, true, false),
            NativeEditRoute::Shortcut(Key::V, true)
        );
        assert_eq!(
            native_edit_route(MenuAction::SelectAll, false, true),
            NativeEditRoute::Shortcut(Key::A, false)
        );
        assert_eq!(
            native_edit_route(MenuAction::SelectAll, false, false),
            NativeEditRoute::SelectAllDocument
        );
    }

    #[test]
    fn screenshot_exports_bgra_padded_rows_in_display_order() {
        let path =
            std::env::temp_dir().join(format!("bed-screenshot-test-{}.ppm", std::process::id()));
        let pixels = [
            30, 20, 10, 255, 60, 50, 40, 255, 0, 0, 0, 0, 90, 80, 70, 255, 120, 110, 100, 255, 0,
            0, 0, 0,
        ];
        write_ppm(
            &path,
            &pixels,
            2,
            2,
            12,
            wgpu::TextureFormat::Bgra8UnormSrgb,
        )
        .unwrap();
        let bytes = std::fs::read(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        assert_eq!(&bytes[..11], b"P6\n2 2\n255\n");
        assert_eq!(
            &bytes[11..],
            &[10, 20, 30, 40, 50, 60, 70, 80, 90, 100, 110, 120]
        );
    }

    #[test]
    fn screenshot_rejects_incompatible_extent_before_creating_file() {
        let path =
            std::env::temp_dir().join(format!("bed-screenshot-invalid-{}.ppm", std::process::id()));
        assert!(write_ppm(&path, &[0; 4], 2, 2, 8, wgpu::TextureFormat::Rgba8Unorm).is_err());
        assert!(write_ppm(&path, &[0; 16], 2, 2, 4, wgpu::TextureFormat::Bgra8Unorm).is_err());
        assert!(!path.exists());
    }
}
