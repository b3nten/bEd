use crate::workbench::{WindowCommand, Workbench, WorkbenchHostMode};
#[cfg(test)]
use bed_document_session::editor::Editor;
#[cfg(test)]
use bed_editor_ui::editor_input::EditorInput;
use bed_effects::{
    shader_manager::ShaderManager, shader_types::OFFSCREEN_FORMAT,
    viewport_effects::ViewportEffectsFactory,
};
use bed_settings::Settings;
use bed_terminal::terminal_input::{
    KeyLocation, KeyState, TerminalKey, TerminalKeyEvent, TerminalModifiers,
};
use bed_workbench_api::gpu::{
    DeviceErrorHandlers, GpuContext, RenderTarget, renderer_device_descriptor,
};
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
    event::{ElementState, Ime, KeyEvent, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{Key as WinitKey, KeyCode, ModifiersState, NamedKey, PhysicalKey},
    platform::modifier_supplement::KeyEventExtModifierSupplement,
    window::{Window, WindowId},
};

type HostResult<T> = Result<T, Box<dyn Error>>;

fn terminal_key_event(event: &KeyEvent, modifiers: TerminalModifiers) -> TerminalKeyEvent {
    let key = match &event.logical_key {
        WinitKey::Character(text) => TerminalKey::Character(text.to_string()),
        WinitKey::Named(named) => terminal_named_key(*named),
        _ => TerminalKey::Unknown,
    };
    let unshifted_key = match event.key_without_modifiers() {
        WinitKey::Character(text) => Some(text.to_string()),
        _ => None,
    };
    let shifted_key = modifiers
        .shift
        .then(|| {
            if let WinitKey::Character(text) = &event.logical_key {
                Some(text.to_string())
            } else {
                None
            }
        })
        .flatten();
    TerminalKeyEvent {
        key,
        unshifted_key,
        shifted_key,
        base_layout_key: match event.physical_key {
            PhysicalKey::Code(code) => terminal_base_key(code),
            _ => None,
        },
        text: event.text.as_ref().map(ToString::to_string),
        location: match event.location {
            winit::keyboard::KeyLocation::Standard => KeyLocation::Standard,
            winit::keyboard::KeyLocation::Left => KeyLocation::Left,
            winit::keyboard::KeyLocation::Right => KeyLocation::Right,
            winit::keyboard::KeyLocation::Numpad => KeyLocation::Numpad,
        },
        modifiers,
        state: if event.state == ElementState::Released {
            KeyState::Release
        } else if event.repeat {
            KeyState::Repeat
        } else {
            KeyState::Press
        },
    }
}

fn terminal_named_key(key: NamedKey) -> TerminalKey {
    match key {
        NamedKey::Enter => TerminalKey::Enter,
        NamedKey::Tab => TerminalKey::Tab,
        NamedKey::Backspace => TerminalKey::Backspace,
        NamedKey::Escape => TerminalKey::Escape,
        NamedKey::ArrowUp => TerminalKey::Up,
        NamedKey::ArrowDown => TerminalKey::Down,
        NamedKey::ArrowLeft => TerminalKey::Left,
        NamedKey::ArrowRight => TerminalKey::Right,
        NamedKey::Home => TerminalKey::Home,
        NamedKey::End => TerminalKey::End,
        NamedKey::Insert => TerminalKey::Insert,
        NamedKey::Delete => TerminalKey::Delete,
        NamedKey::PageUp => TerminalKey::PageUp,
        NamedKey::PageDown => TerminalKey::PageDown,
        NamedKey::CapsLock => TerminalKey::CapsLock,
        NamedKey::NumLock => TerminalKey::NumLock,
        NamedKey::ScrollLock => TerminalKey::ScrollLock,
        NamedKey::PrintScreen => TerminalKey::PrintScreen,
        NamedKey::Pause => TerminalKey::Pause,
        NamedKey::ContextMenu => TerminalKey::Menu,
        NamedKey::Shift => TerminalKey::Shift,
        NamedKey::Control => TerminalKey::Control,
        NamedKey::Alt => TerminalKey::Alt,
        NamedKey::Super => TerminalKey::Super,
        NamedKey::Space => TerminalKey::Character(" ".into()),
        NamedKey::F1 => TerminalKey::F(1),
        NamedKey::F2 => TerminalKey::F(2),
        NamedKey::F3 => TerminalKey::F(3),
        NamedKey::F4 => TerminalKey::F(4),
        NamedKey::F5 => TerminalKey::F(5),
        NamedKey::F6 => TerminalKey::F(6),
        NamedKey::F7 => TerminalKey::F(7),
        NamedKey::F8 => TerminalKey::F(8),
        NamedKey::F9 => TerminalKey::F(9),
        NamedKey::F10 => TerminalKey::F(10),
        NamedKey::F11 => TerminalKey::F(11),
        NamedKey::F12 => TerminalKey::F(12),
        NamedKey::F13 => TerminalKey::F(13),
        NamedKey::F14 => TerminalKey::F(14),
        NamedKey::F15 => TerminalKey::F(15),
        NamedKey::F16 => TerminalKey::F(16),
        NamedKey::F17 => TerminalKey::F(17),
        NamedKey::F18 => TerminalKey::F(18),
        NamedKey::F19 => TerminalKey::F(19),
        NamedKey::F20 => TerminalKey::F(20),
        NamedKey::F21 => TerminalKey::F(21),
        NamedKey::F22 => TerminalKey::F(22),
        NamedKey::F23 => TerminalKey::F(23),
        NamedKey::F24 => TerminalKey::F(24),
        NamedKey::F25 => TerminalKey::F(25),
        NamedKey::F26 => TerminalKey::F(26),
        NamedKey::F27 => TerminalKey::F(27),
        NamedKey::F28 => TerminalKey::F(28),
        NamedKey::F29 => TerminalKey::F(29),
        NamedKey::F30 => TerminalKey::F(30),
        NamedKey::F31 => TerminalKey::F(31),
        NamedKey::F32 => TerminalKey::F(32),
        NamedKey::F33 => TerminalKey::F(33),
        NamedKey::F34 => TerminalKey::F(34),
        NamedKey::F35 => TerminalKey::F(35),
        _ => TerminalKey::Unknown,
    }
}

fn terminal_base_key(key: KeyCode) -> Option<char> {
    Some(match key {
        KeyCode::KeyA => 'a',
        KeyCode::KeyB => 'b',
        KeyCode::KeyC => 'c',
        KeyCode::KeyD => 'd',
        KeyCode::KeyE => 'e',
        KeyCode::KeyF => 'f',
        KeyCode::KeyG => 'g',
        KeyCode::KeyH => 'h',
        KeyCode::KeyI => 'i',
        KeyCode::KeyJ => 'j',
        KeyCode::KeyK => 'k',
        KeyCode::KeyL => 'l',
        KeyCode::KeyM => 'm',
        KeyCode::KeyN => 'n',
        KeyCode::KeyO => 'o',
        KeyCode::KeyP => 'p',
        KeyCode::KeyQ => 'q',
        KeyCode::KeyR => 'r',
        KeyCode::KeyS => 's',
        KeyCode::KeyT => 't',
        KeyCode::KeyU => 'u',
        KeyCode::KeyV => 'v',
        KeyCode::KeyW => 'w',
        KeyCode::KeyX => 'x',
        KeyCode::KeyY => 'y',
        KeyCode::KeyZ => 'z',
        KeyCode::Digit0 => '0',
        KeyCode::Digit1 => '1',
        KeyCode::Digit2 => '2',
        KeyCode::Digit3 => '3',
        KeyCode::Digit4 => '4',
        KeyCode::Digit5 => '5',
        KeyCode::Digit6 => '6',
        KeyCode::Digit7 => '7',
        KeyCode::Digit8 => '8',
        KeyCode::Digit9 => '9',
        KeyCode::Space => ' ',
        KeyCode::Backquote => '`',
        KeyCode::Minus => '-',
        KeyCode::Equal => '=',
        KeyCode::BracketLeft => '[',
        KeyCode::BracketRight => ']',
        KeyCode::Backslash => '\\',
        KeyCode::Semicolon => ';',
        KeyCode::Quote => '\'',
        KeyCode::Comma => ',',
        KeyCode::Period => '.',
        KeyCode::Slash => '/',
        _ => return None,
    })
}

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
struct NativeFileClipboard(arboard::Clipboard);
impl bed_workbench_api::FileClipboardService for NativeFileClipboard {
    fn read_files(&mut self) -> io::Result<Vec<PathBuf>> {
        self.0.get().file_list().map_err(io::Error::other)
    }
    fn write_files(&mut self, paths: &[PathBuf]) -> io::Result<()> {
        self.0.set().file_list(paths).map_err(io::Error::other)
    }
}
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

fn transparent_alpha_mode(
    backend: wgpu::Backend,
    supported: &[wgpu::CompositeAlphaMode],
) -> wgpu::CompositeAlphaMode {
    use wgpu::CompositeAlphaMode as Alpha;
    // wgpu 29's Metal backend calls its premultiplied CAMetalLayer mode
    // PostMultiplied. Keep our premultiplied pixels and select that mode until
    // the pin includes https://github.com/gfx-rs/wgpu/pull/9922.
    [
        Alpha::PreMultiplied,
        if backend == wgpu::Backend::Metal {
            Alpha::PostMultiplied
        } else {
            Alpha::Inherit
        },
        Alpha::Inherit,
        Alpha::Opaque,
    ]
    .into_iter()
    .find(|mode| supported.contains(mode))
    .unwrap_or(Alpha::Auto)
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
    plugin_textures: HashMap<bed_workbench_api::TextureHandle, PluginGpuTexture>,
    generation: u64,
}

struct PluginGpuTexture {
    external: dear_imgui_wgpu::ExternalTextureId,
    target: RenderTarget,
    rendered_revision: Option<u64>,
}

/// The native runtime owns this transient output; it never becomes a dock panel.
#[allow(clippy::too_many_arguments)]
fn draw_bedtime(
    ui: &dear_imgui_rs::Ui,
    viewport_id: u32,
    gpu: &mut Gpu,
    scene: &mut bed_scenes::SceneView,
    texture: &mut Option<PluginGpuTexture>,
    background: [f32; 4],
    accent: [f32; 4],
    animations: bool,
) -> HostResult<()> {
    let Some(viewport) = ui.find_viewport_by_id(dear_imgui_rs::Id::from(viewport_id)) else {
        return Ok(());
    };
    let pos = viewport.work_pos();
    let size = viewport.work_size();
    if size[0] < 1.0 || size[1] < 1.0 {
        return Ok(());
    }
    let pixels = crate::bedtime::scene_pixels(
        size,
        viewport.framebuffer_scale(),
        ui.io().display_framebuffer_scale(),
    );
    scene.update(
        pixels,
        bed_scenes::SceneFrame {
            background,
            accent,
            animations,
            ..Default::default()
        },
    );
    if let Some(output) = scene.render_output() {
        if texture
            .as_ref()
            .is_none_or(|current| current.target.size != output.size)
        {
            let target = RenderTarget::new(&gpu.device, output).map_err(io::Error::other)?;
            let external = if let Some(current) = texture.as_ref() {
                gpu.route
                    .update_external_texture(current.external, &target.view)?;
                current.external
            } else {
                gpu.route.register_external_texture(&target.view)?
            };
            *texture = Some(PluginGpuTexture {
                external,
                target,
                rendered_revision: None,
            });
        }
        if let Some(current) = texture.as_mut()
            && current.rendered_revision != Some(output.revision)
        {
            let mut encoder = gpu
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("bEdtime scene"),
                });
            let result = scene.render(
                &mut GpuContext {
                    instance: &gpu.instance,
                    adapter: &gpu.adapter,
                    device: &gpu.device,
                    queue: &gpu.queue,
                    encoder: &mut encoder,
                    error_handlers: Some(&gpu.error_handlers),
                    generation: gpu.generation,
                },
                &current.target,
            );
            gpu.error_handlers.install(&gpu.device);
            result.map_err(io::Error::other)?;
            gpu.queue.submit([encoder.finish()]);
            current.rendered_revision = Some(output.revision);
        }
    }
    // SAFETY: the viewport and foreground list belong to this live, bound frame.
    // Select the foreground draw list for the focused native window.
    let draw = unsafe {
        let viewport = dear_imgui_rs::sys::igFindViewportByID(viewport_id);
        dear_imgui_rs::DrawListMut::from_raw_mut(
            ui,
            dear_imgui_rs::sys::igGetForegroundDrawList_ViewportPtr(viewport),
        )
    };
    let end = [pos[0] + size[0], pos[1] + size[1]];
    draw.add_rect(pos, end, [0.0, 0.0, 0.0, 0.8])
        .filled(true)
        .build();
    if let Some(current) = texture.as_ref() {
        draw.add_image(
            current.external.texture_id(),
            pos,
            end,
            [0.0, 0.0],
            [1.0, 1.0],
            [1.0; 4],
        );
    }
    Ok(())
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
        config.alpha_mode =
            transparent_alpha_mode(adapter.get_info().backend, &capabilities.alpha_modes);
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

    fn upload_rgba(&mut self, image: &bed_ui::icons::RgbaImage) -> HostResult<TextureId> {
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
    fn retire_plugin_textures(&mut self, workbench: &mut Workbench) -> HostResult<()> {
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
        Ok(())
    }

    /// Add or resize outputs without removing IDs that this frame may have drawn.
    fn sync_plugin_textures(&mut self, workbench: &mut Workbench) -> HostResult<()> {
        for output in workbench.plugin_render_outputs() {
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
        for output in workbench.plugin_render_outputs() {
            let Some(texture) = self.plugin_textures.get_mut(&output.handle) else {
                continue;
            };
            if texture.rendered_revision == Some(output.revision) {
                continue;
            }
            // Each panel owns this batch. Commands recorded by a failed panel
            // are discarded without affecting another panel's submission.
            let mut encoder = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("Bed plugin canvas"),
                });
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
                    self.queue.submit([encoder.finish()]);
                }
                Err(error) => workbench.error = Some(error),
            }
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
    terminal_modifiers: TerminalModifiers,
    terminal_composing: bool,
    terminal_reserved_keys: HashSet<PhysicalKey>,
    external_drag_paths: Vec<PathBuf>,
    external_drag_viewport: u32,
    pending_file_drop: Option<bed_workbench_api::ExternalFileDrag>,
    #[cfg(target_os = "macos")]
    native_menu: crate::platform::macos_menu::MacOsMenu,
    #[cfg(target_os = "macos")]
    native_window: crate::platform::macos_window::MacOsWindow,
    #[cfg(target_os = "macos")]
    viewport_themes: std::collections::HashMap<WindowId, ([f32; 4], [f32; 4])>,
    window: Arc<Window>,
    gpu: Gpu,
    platform: WinitPlatform,
    context: Context,
    workbench: Workbench,
    rendered_frames: u32,
    frame_pending: bool,
    focus_needs_frame: bool,
    bedtime: crate::bedtime::IdleController,
    bedtime_scene: bed_scenes::SceneView,
    bedtime_texture: Option<PluginGpuTexture>,
    secondary_presentations: u64,
    shutdown_done: bool,
    capture_path: Option<PathBuf>,
    capture_after_frames: u32,
    capture_completed: bool,
    debugger_smoke: bool,
    debugger_ready: bool,
    git_smoke: bool,
    git_ready: bool,
    lifecycle: Option<LifecycleSmoke>,
    appearance: Option<AppearanceSmoke>,
    #[cfg(target_os = "macos")]
    native_smoke: Option<NativeAppearanceSmoke>,
    #[cfg(target_os = "macos")]
    menu_smoke: Option<MenuEditSmoke>,
    plugin_smoke: Option<PluginSmoke>,
    effects_enabled_override: Option<bool>,
    terminal_smoke: bool,
    terminal_promotion_smoke: bool,
    terminal_initial_size: Option<[f32; 2]>,
    terminal_split_verified: bool,
    terminal_promotion_identity: Option<TerminalPromotionIdentity>,
    started: Instant,
}

struct TerminalPromotionIdentity {
    process: u32,
    window: WindowId,
    root: PathBuf,
    terminal: u64,
    panel: u64,
    panels: Vec<u64>,
    terminals: Vec<(u64, u32)>,
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
    git: bool,
    plugins: bool,
    effects_enabled_override: Option<bool>,
    terminal_smoke: bool,
    terminal_promotion_smoke: bool,
}
const PLUGIN_SMOKE_BYTES: &[u8] = &[0, 255, 13, 10, 239, 187, 191, 128, 0, 1];
struct PluginSmoke {
    image: PathBuf,
    binary: PathBuf,
    model: PathBuf,
    embedded_model: PathBuf,
    model_capture: Option<PathBuf>,
    model_size: [u32; 2],
    frames_at_resize: u32,
    original_handle: Option<bed_workbench_api::TextureHandle>,
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
    fn terminal_window_event(&mut self, event: &WindowEvent) -> io::Result<()> {
        match event {
            WindowEvent::KeyboardInput { event, .. }
                if event.state == ElementState::Released
                    && self.terminal_reserved_keys.remove(&event.physical_key) =>
            {
                return Ok(());
            }
            WindowEvent::KeyboardInput { event, .. }
                if event.state == ElementState::Pressed && !event.repeat =>
            {
                match event.logical_key {
                    WinitKey::Named(NamedKey::CapsLock) => {
                        self.terminal_modifiers.caps_lock = !self.terminal_modifiers.caps_lock;
                    }
                    WinitKey::Named(NamedKey::NumLock) => {
                        self.terminal_modifiers.num_lock = !self.terminal_modifiers.num_lock;
                    }
                    _ => {}
                }
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                let state = modifiers.state();
                self.terminal_modifiers.shift = state.contains(ModifiersState::SHIFT);
                self.terminal_modifiers.alt = state.contains(ModifiersState::ALT);
                self.terminal_modifiers.control = state.contains(ModifiersState::CONTROL);
                self.terminal_modifiers.super_key = state.contains(ModifiersState::SUPER);
                return Ok(());
            }
            WindowEvent::Focused(false) => {
                self.terminal_modifiers = TerminalModifiers {
                    caps_lock: self.terminal_modifiers.caps_lock,
                    num_lock: self.terminal_modifiers.num_lock,
                    ..Default::default()
                };
                self.terminal_reserved_keys.clear();
                self.terminal_composing = false;
                self.workbench.terminal.queue_ime_preedit(String::new());
                return Ok(());
            }
            _ => {}
        }
        if !self.workbench.focused_terminal() || !self.workbench.terminal.is_focused() {
            return Ok(());
        }
        match event {
            WindowEvent::KeyboardInput {
                event,
                is_synthetic,
                ..
            } if !is_synthetic => {
                if self.terminal_reserved_keys.contains(&event.physical_key) {
                    return Ok(());
                }
                let key = terminal_key_event(event, self.terminal_modifiers);
                if self.terminal_composing || matches!(event.logical_key, WinitKey::Dead(_)) {
                    return Ok(());
                }
                let shortcut_key = match event.key_without_modifiers() {
                    WinitKey::Character(text) => text.to_lowercase(),
                    _ => String::new(),
                };
                let modifiers = key.modifiers;
                let clipboard_modifier = if cfg!(target_os = "macos") {
                    modifiers.super_key && !modifiers.control && !modifiers.alt
                } else {
                    modifiers.control && modifiers.shift && !modifiers.super_key && !modifiers.alt
                };
                let clipboard_action = clipboard_modifier
                    && matches!(shortcut_key.as_str(), "c" | "v")
                    || matches!(key.key, TerminalKey::Insert)
                        && modifiers.shift
                        && !modifiers.control
                        && !modifiers.alt
                        && !modifiers.super_key
                    || cfg!(target_os = "macos") && clipboard_modifier && shortcut_key == "a";
                if clipboard_action {
                    if event.state == ElementState::Pressed && !event.repeat {
                        self.terminal_reserved_keys.insert(event.physical_key);
                        match shortcut_key.as_str() {
                            "c" => {
                                if let Some(text) = self.workbench.terminal.native_copy() {
                                    self.context.set_clipboard_text(text);
                                }
                            }
                            "a" => self.workbench.terminal.native_select_all()?,
                            _ => {
                                if let Some(text) = self.context.clipboard_text() {
                                    self.workbench.terminal.native_paste(text)?;
                                }
                            }
                        }
                    }
                    return Ok(());
                }
                if self.terminal_application_shortcut(event, &key) {
                    if event.state == ElementState::Pressed {
                        self.terminal_reserved_keys.insert(event.physical_key);
                    }
                    return Ok(());
                }
                self.workbench.terminal.queue_key(key)?;
            }
            WindowEvent::Ime(Ime::Preedit(text, _)) => {
                self.terminal_composing = !text.is_empty();
                self.workbench.terminal.queue_ime_preedit(text.clone());
            }
            WindowEvent::Ime(Ime::Commit(text)) => {
                self.terminal_composing = false;
                self.workbench.terminal.queue_ime_preedit(String::new());
                self.workbench.terminal.queue_ime_commit(text.clone())?;
            }
            WindowEvent::Ime(Ime::Disabled) => {
                self.terminal_composing = false;
                self.workbench.terminal.queue_ime_preedit(String::new());
            }
            _ => {}
        }
        Ok(())
    }

    fn terminal_application_shortcut(&self, event: &KeyEvent, key: &TerminalKeyEvent) -> bool {
        let application_modifier = if cfg!(target_os = "macos") {
            key.modifiers.super_key && !key.modifiers.control
        } else {
            key.modifiers.control && !key.modifiers.super_key
        };
        if !application_modifier || key.modifiers.alt {
            return false;
        }
        let imgui_key = match event.key_without_modifiers() {
            WinitKey::Character(text) => bed_settings::keybinds::string_to_imgui_key(&text),
            _ => None,
        };
        let Some(imgui_key) = imgui_key else {
            return false;
        };
        use dear_imgui_rs::Key;
        if matches!(
            imgui_key,
            Key::Slash
                | Key::Key1
                | Key::Key2
                | Key::Key3
                | Key::Key4
                | Key::Key5
                | Key::Key6
                | Key::Key7
                | Key::Key8
                | Key::Key9
        ) || key.modifiers.shift && imgui_key == Key::F
        {
            return true;
        }
        [
            "toggle_settings_window",
            "toggle_terminal",
            "toggle_file_finder",
            "find_in_project",
        ]
        .into_iter()
        .any(|name| self.workbench.settings.keybinds.get_action_key(name) == Some(imgui_key))
    }

    fn external_drag_modifiers(&self) -> bed_workbench_api::ExternalFileModifiers {
        #[cfg(target_os = "macos")]
        if let Some(modifiers) = crate::platform::macos_window::external_drag_modifiers() {
            return modifiers;
        }
        #[cfg(target_os = "linux")]
        if let Some([control, shift, alt, super_key]) =
            crate::platform::linux_window::external_drag_modifiers(&self.window)
        {
            return bed_workbench_api::ExternalFileModifiers {
                control,
                shift,
                alt,
                super_key,
            };
        }
        let io = self.context.io();
        bed_workbench_api::ExternalFileModifiers {
            control: io.key_ctrl(),
            shift: io.key_shift(),
            alt: io.key_alt(),
            super_key: io.key_super(),
        }
    }
    fn external_drag_position(&self) -> [f32; 2] {
        #[cfg(target_os = "macos")]
        if let Some(mut position) = crate::platform::macos_window::external_drag_position() {
            if !self
                .context
                .io()
                .config_flags()
                .contains(dear_imgui_rs::ConfigFlags::VIEWPORTS_ENABLE)
                && let Ok(origin) = self.window.inner_position()
            {
                let origin = origin.to_logical::<f32>(self.window.scale_factor());
                position[0] -= origin.x;
                position[1] -= origin.y;
            }
            return position;
        }
        #[cfg(target_os = "linux")]
        if let Some(position) = crate::platform::linux_window::external_drag_position(
            &self.window,
            self.context
                .io()
                .config_flags()
                .contains(dear_imgui_rs::ConfigFlags::VIEWPORTS_ENABLE),
        ) {
            return position;
        }
        self.context.io().mouse_pos()
    }
    fn new(
        event_loop: &ActiveEventLoop,
        mut workbench: Workbench,
        options: RuntimeOptions,
    ) -> HostResult<Self> {
        let attributes = Window::default_attributes()
            .with_title(workbench.window_title())
            .with_transparent(true)
            .with_inner_size(LogicalSize::new(1200.0, 800.0));
        #[cfg(target_os = "macos")]
        let attributes = {
            use winit::platform::macos::WindowAttributesExtMacOS;
            attributes
                .with_titlebar_transparent(true)
                .with_title_hidden(true)
                .with_fullsize_content_view(true)
                .with_has_shadow(true)
        };
        let window = Arc::new(event_loop.create_window(attributes)?);
        let mut context = Context::create();
        if let Ok(clipboard) = arboard::Clipboard::new() {
            context.set_clipboard_backend(NativeClipboard(clipboard));
        }
        if let Ok(clipboard) = arboard::Clipboard::new() {
            workbench.set_file_clipboard(Box::new(NativeFileClipboard(clipboard)));
        }
        workbench.initialize(&mut context, WorkbenchHostMode::Fullscreen)?;
        let mut platform = WinitPlatform::new(&mut context)?;
        platform.attach_window(Arc::clone(&window), HiDpiMode::Default, &mut context)?;
        platform.set_ime_auto_management(false);
        // Version one keeps every workspace panel in this native window.
        // The viewport renderer remains available for a future host policy.
        let flags = context.io().config_flags() & !dear_imgui_rs::ConfigFlags::VIEWPORTS_ENABLE;
        context.io_mut().set_config_flags(flags);
        window.set_ime_allowed(true);
        let mut gpu = Gpu::new(Arc::clone(&window), &mut context, &platform)?;
        #[cfg(target_os = "macos")]
        let mut native_window = crate::platform::macos_window::MacOsWindow::configure(
            &window,
            workbench.settings.background_opacity(),
            workbench.settings.bool("mac_blur_enabled", true),
            &workbench.icons,
        )?;
        #[cfg(target_os = "macos")]
        {
            workbench.root_top_inset = native_window.titlebar_inset();
        }
        #[cfg(target_os = "macos")]
        let mut native_menu = crate::platform::macos_menu::MacOsMenu::install(&workbench.settings)?;
        #[cfg(target_os = "macos")]
        {
            native_menu.set_workspace_available(workbench.workspace_spec.is_some());
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
            opacity: workbench.settings.settings["background_opacity"].clone(),
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
            frames_at_resize: 0,
            original_handle: None,
            phase: 0,
        });
        Ok(Self {
            terminal_modifiers: TerminalModifiers::default(),
            terminal_composing: false,
            terminal_reserved_keys: HashSet::new(),
            external_drag_paths: Vec::new(),
            external_drag_viewport: 0,
            pending_file_drop: None,
            #[cfg(target_os = "macos")]
            native_menu,
            #[cfg(target_os = "macos")]
            native_window,
            #[cfg(target_os = "macos")]
            viewport_themes: Default::default(),
            window,
            gpu,
            platform,
            context,
            workbench,
            rendered_frames: 0,
            frame_pending: true,
            focus_needs_frame: false,
            bedtime: crate::bedtime::IdleController::new(Instant::now()),
            bedtime_scene: bed_scenes::SceneView::new(bed_scenes::SceneKind::Bedtime),
            bedtime_texture: None,
            secondary_presentations: 0,
            shutdown_done: false,
            capture_path: options.capture_path,
            capture_after_frames: options.capture_after_frames.unwrap_or(2),
            capture_completed: false,
            debugger_smoke: options.debugger,
            debugger_ready: false,
            git_smoke: options.git,
            git_ready: false,
            lifecycle: options.lifecycle.then(LifecycleSmoke::new),
            appearance,
            #[cfg(target_os = "macos")]
            native_smoke,
            #[cfg(target_os = "macos")]
            menu_smoke,
            plugin_smoke,
            effects_enabled_override: options.effects_enabled_override,
            terminal_smoke: options.terminal_smoke,
            terminal_promotion_smoke: options.terminal_promotion_smoke,
            terminal_initial_size: None,
            terminal_split_verified: false,
            terminal_promotion_identity: None,
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
    #[cfg(target_os = "macos")]
    fn update_viewport_themes(&mut self) -> HostResult<()> {
        let windows = self.windows()?;
        self.viewport_themes.retain(|id, _| {
            windows
                .iter()
                .any(|viewport| !viewport.is_main && viewport.window.id() == *id)
        });
        let theme = (
            self.workbench.settings.text_color(),
            self.workbench.settings.window_background_color(),
        );
        for viewport in windows.into_iter().filter(|viewport| !viewport.is_main) {
            let id = viewport.window.id();
            if self.viewport_themes.get(&id) == Some(&theme) {
                continue;
            }
            crate::platform::macos_window::MacOsWindow::apply_theme_to_window(
                &viewport.window,
                theme.0,
                theme.1,
            )?;
            self.viewport_themes.insert(id, theme);
        }
        Ok(())
    }
    fn open_window(
        &mut self,
        workspace: Option<&bed_workbench_api::workspace::WorkspaceSpec>,
    ) -> HostResult<()> {
        #[cfg(target_os = "macos")]
        let wait = self.menu_smoke.is_some();
        #[cfg(not(target_os = "macos"))]
        let wait = false;
        let mut command = new_instance_command(
            &std::env::current_exe()?,
            &self.workbench.settings.config_dir,
            wait,
            workspace,
        )?;
        #[cfg(target_os = "macos")]
        if wait {
            command.arg("--main-only-smoke");
        }
        let mut child = command.spawn()?;
        #[cfg(target_os = "macos")]
        if let Some(smoke) = &mut self.menu_smoke {
            smoke.instances.push(child);
            return Ok(());
        }
        std::thread::Builder::new()
            .name("bed-instance-wait".into())
            .spawn(move || {
                let _ = child.wait();
            })?;
        Ok(())
    }
    fn tick_workbench(&mut self) {
        if let Err(error) = self.workbench.tick() {
            self.workbench.error = Some(error.to_string());
        }
        for workspace in self.workbench.take_workspace_windows() {
            if let Err(error) = self.open_window(Some(&workspace)) {
                self.workbench.error = Some(format!("Could not open workspace window: {error}"));
            }
        }
    }
    fn redraw(&mut self, event_loop: &ActiveEventLoop) -> HostResult<bool> {
        self.tick_workbench();
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
            self.bedtime_texture = None;
            self.gpu.route.shutdown(&mut self.context)?;
            // Existing secondary windows have already run Renderer_CreateWindow.
            // Recreate platform ownership so the replacement renderer receives
            // those callbacks again; ImGui retains the panels and their layout.
            if self.platform.viewports_enabled() {
                self.platform.disable_viewports(&mut self.context)?;
                self.platform.enable_viewports(&mut self.context)?;
            }
            self.gpu = Gpu::new(Arc::clone(&self.window), &mut self.context, &self.platform)?;
            self.workbench.invalidate_plugin_textures();
            self.workbench.terminal.invalidate_textures();
            self.gpu.upload_frame_assets(&mut self.workbench)?;
        }
        if let Err(error) = self.gpu.retire_plugin_textures(&mut self.workbench) {
            self.workbench.error = Some(error.to_string());
        }
        if let Err(error) = self.gpu.sync_plugin_textures(&mut self.workbench) {
            self.workbench.error = Some(error.to_string());
        }
        self.workbench.apply_settings(&mut self.context)?;
        let framebuffer_scale = self.window.scale_factor() as f32;
        self.workbench.terminal.prepare_frame(
            &mut self.context,
            &self.workbench.terminal_fonts,
            framebuffer_scale,
        )?;
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
            self.native_menu
                .set_workspace_available(self.workbench.workspace_spec.is_some());
            self.native_menu.update(
                &self.workbench.settings,
                self.workbench.active_document().is_some(),
                self.workbench.focused_terminal(),
                self.context.io().want_text_input(),
            )?;
            self.native_window.update(
                self.workbench.settings.background_opacity(),
                self.workbench.settings.bool("mac_blur_enabled", true),
            )?;
            let text = self.workbench.settings.text_color();
            let background = self.workbench.settings.window_background_color();
            self.native_window.update_theme(text, background)?;
            self.workbench.root_top_inset = self.native_window.titlebar_inset();
            for id in self.native_window.take_command_ids() {
                if self.bedtime.activity(Instant::now()) {
                    continue;
                }
                self.workbench.dispatch_command_from_menu(&id)?;
            }
        }
        #[cfg(target_os = "macos")]
        self.update_viewport_themes()?;
        let focused_viewports: HashSet<u32> = self
            .windows()?
            .into_iter()
            .filter(|native| native.window.has_focus())
            .map(|native| native.viewport_id)
            .collect();
        self.platform
            .prepare_frame(&mut self.context, &self.window)?;
        let frame = self.context.begin_frame();
        let ui = frame.ui();
        #[cfg(target_os = "linux")]
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
                self.workbench.dispatch_command_from_menu(&id)?;
            }
        }
        let actions = self.workbench.render(ui)?;
        if let Some(smoke) = &mut self.plugin_smoke
            && smoke.phase == 2
            && self.capture_path.is_none()
            && self.rendered_frames >= 16
        {
            // Regression: the current ImGui frame already contains the image.
            // Closing its panel must retain that texture until submission ends.
            self.workbench.dispatch(WindowCommand::Close)?;
            smoke.phase = 3;
            eprintln!("bEd: plugin smoke closed an output after UI drawing");
        }
        let blocked = !self.external_drag_paths.is_empty()
            || ui.is_mouse_down(dear_imgui_rs::MouseButton::Left)
            || ui.is_mouse_down(dear_imgui_rs::MouseButton::Right);
        let eligible = focused_viewports.iter().copied().min().filter(|_| !blocked);
        let delay = self
            .workbench
            .settings
            .number("bedtime_delay_seconds", 60.0);
        let delay = if delay.is_finite() {
            delay.clamp(1.0, 86400.0)
        } else {
            60.0
        };
        if let Some(viewport) = self.bedtime.update(
            Instant::now(),
            eligible,
            self.workbench.settings.bool("bedtime_enabled", true),
            Duration::from_secs_f32(delay),
        ) {
            if let Err(error) = draw_bedtime(
                ui,
                viewport,
                &mut self.gpu,
                &mut self.bedtime_scene,
                &mut self.bedtime_texture,
                self.workbench.settings.window_background_color(),
                ui.style_color(dear_imgui_rs::StyleColor::CheckMark),
                self.workbench.settings.bool("ui_animations", true),
            ) {
                self.workbench.error = Some(error.to_string());
            }
        } else if let Some(texture) = self.bedtime_texture.take() {
            self.gpu
                .route
                .unregister_external_texture(texture.external)?;
        }
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
        // Each visible window draws its own background once. Clearing to the
        // theme background would composite another translucent layer below it.
        let clear = wgpu::Color::TRANSPARENT;
        self.gpu.route.set_viewport_clear_color(clear)?;

        // The route completes every secondary surface before acquiring the main
        // one. A minimized or temporarily unavailable main window cannot stop it.
        let prepared = self.gpu.route.prepare(event_loop, frame)?;
        self.secondary_presentations += prepared.secondary_presentations() as u64;
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
                && (!self.git_smoke || self.git_ready)
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
        self.advance_plugin_smoke()?;
        self.advance_terminal_smoke()?;
        if self.terminal_promotion_smoke && self.rendered_frames == 60 {
            let terminal = self
                .workbench
                .terminal
                .active_session_id()
                .ok_or_else(|| io::Error::other("Terminal promotion requires a live shell"))?;
            let identity = TerminalPromotionIdentity {
                process: std::process::id(),
                window: self.window.id(),
                root: self.workbench.terminal.live_working_directory(terminal)?,
                terminal,
                panel: self
                    .workbench
                    .terminal_panel_id(terminal)
                    .ok_or_else(|| io::Error::other("Calling terminal has no panel"))?,
                panels: (0..self.workbench.tab_count())
                    .filter_map(|index| self.workbench.tab_window_id(index))
                    .collect(),
                terminals: self
                    .workbench
                    .terminal
                    .session_ids()
                    .into_iter()
                    .map(|session| {
                        self.workbench
                            .terminal
                            .process_id(session)
                            .map(|pid| (session, pid))
                            .ok_or_else(|| io::Error::other("A smoke shell is not running"))
                    })
                    .collect::<io::Result<_>>()?,
            };
            // Exercise the installed helper in the actual calling shell.
            self.workbench.terminal.write_active(b"+workspace\n")?;
            self.terminal_promotion_smoke = false;
            self.terminal_promotion_identity = Some(identity);
            eprintln!("bEd: requested workspace attachment through +workspace");
        }
        if self.git_smoke && !self.git_ready {
            self.git_ready = self.workbench.git_smoke_ready()?;
            if self.git_ready {
                self.workbench.git_smoke_focus_comparison();
                // Give the loaded comparison its own complete layout frame.
                self.capture_after_frames = self.rendered_frames + 5;
                eprintln!("bEd: Git smoke loaded staged and editable working comparisons");
            } else if self.started.elapsed() > Duration::from_secs(30) {
                return Err(io::Error::other("Git smoke timed out loading comparisons").into());
            }
        }
        for action in actions {
            self.workbench.handle_action(action)?;
        }
        Ok(true)
    }

    fn advance_terminal_smoke(&mut self) -> HostResult<()> {
        if !self.terminal_smoke {
            return Ok(());
        }
        if self.terminal_promotion_identity.is_some()
            && (self.workbench.workspace_spec.is_none()
                || self.workbench.terminal.native_copy().is_none())
            && self.started.elapsed() > Duration::from_secs(30)
        {
            return Err(io::Error::other(
                "+workspace smoke timed out waiting for attachment and transcript",
            )
            .into());
        }
        match self.rendered_frames {
            5 => {
                let sessions = self.workbench.terminal.session_ids();
                if sessions.len() != 1 {
                    return Err(io::Error::other(format!(
                        "Terminal smoke must start with one shell, found {}",
                        sessions.len()
                    ))
                    .into());
                }
                self.terminal_initial_size = self
                    .workbench
                    .terminal
                    .session_ids()
                    .first()
                    .and_then(|session| self.workbench.terminal_panel_id(*session))
                    .and_then(|panel| {
                        self.context
                            .binding()
                            .with_bound_context(|| terminal_window_metadata(panel))
                    })
                    .map(|(_, size)| size);
                self.write_terminal_smoke_fixture()?;
            }
            10 | 20 => {
                let command = if self.rendered_frames == 10 {
                    WindowCommand::SplitRight
                } else {
                    WindowCommand::SplitDown
                };
                self.workbench.dispatch(command)?;
                let expected = if self.rendered_frames == 10 { 2 } else { 3 };
                let actual = self.workbench.terminal.session_ids().len();
                if actual != expected {
                    return Err(io::Error::other(format!(
                        "Terminal split expected {expected} shells, found {actual}"
                    ))
                    .into());
                }
            }
            15 | 25 => self.write_terminal_smoke_fixture()?,
            30 | 40 => {
                let name = if self.rendered_frames == 30 {
                    "results.csv"
                } else {
                    "chart.svg"
                };
                self.workbench.open_terminal_companion(
                    &PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                        .join("crates/bedterm/examples")
                        .join(name),
                    None,
                )?;
            }
            55 => {
                self.context
                    .binding()
                    .with_bound_context(|| self.check_terminal_splits())?;
                self.terminal_split_verified = true;
            }
            70.. if self.terminal_promotion_identity.is_some()
                && self.workbench.workspace_spec.is_some()
                && self.workbench.terminal.native_copy().is_none() =>
            {
                // The workspace template shrinks the terminal. Select the whole
                // transcript so verification includes text moved into scrollback.
                self.workbench.terminal.native_select_all()?;
            }
            _ => {}
        }
        Ok(())
    }

    fn write_terminal_smoke_fixture(&mut self) -> io::Result<()> {
        let session = self
            .workbench
            .terminal
            .session_ids()
            .last()
            .copied()
            .ok_or_else(|| io::Error::other("Terminal smoke has no shell session"))?;
        self.workbench.terminal.focus_session(session);
        // The one-pixel RGBA payload is displayed at six columns by three rows,
        // exercising managed color textures without a shell-side image utility.
        // Leave row zero for shell hooks that redraw a working-directory header.
        let output = concat!(
            "printf '\\033[2J\\033[H\\r\\n",
            "Graphemes: é 日本語 👩‍💻\\r\\n",
            "\\033[4:3mcurly\\033[4:2m double\\033[0m\\r\\n",
            "AGENTS.md\\tNOTICE\\t\\033[34mvendor\\033[0m\\r\\n",
            "\\033]8;;https://example.com\\033\\\\OSC 8 link\\033]8;;\\033\\\\\\r\\n",
            "\\033_Ga=T,f=32,s=1,v=1,c=6,r=3,i=1,q=2;/wAA/w==\\033\\\\",
            "\\r\\nTERMINAL_GPU_READY\\r\\n'\n"
        );
        self.workbench.terminal.write_active(output.as_bytes())
    }

    fn check_terminal_smoke(&self) -> HostResult<()> {
        if let Some(error) = &self.workbench.error {
            return Err(io::Error::other(error.clone()).into());
        }
        let sessions = self.workbench.terminal.session_ids();
        let expected_sessions = if self.terminal_promotion_identity.is_some() {
            1
        } else {
            3
        };
        if sessions.len() != expected_sessions {
            return Err(io::Error::other(format!(
                "Terminal smoke expected {expected_sessions} independent shells, found {}",
                sessions.len()
            ))
            .into());
        }
        if !self.terminal_split_verified {
            return Err(io::Error::other("Terminal split layout was not verified").into());
        }
        if self.workbench.workspace_spec.is_none() {
            self.check_terminal_splits()?;
        }
        let snapshot = self
            .workbench
            .terminal
            .active_terminal()
            .ok_or_else(|| io::Error::other("Terminal smoke has no active snapshot"))?;
        let text = if self.terminal_promotion_identity.is_some() {
            snapshot
                .selection_text()
                .ok_or_else(|| io::Error::other("Terminal transcript is unavailable"))?
        } else {
            snapshot
                .lines
                .iter()
                .flat_map(|row| row.cells.iter())
                .filter(|cell| cell.width != 0)
                .map(|cell| cell.text.as_ref())
                .collect::<String>()
        };
        let missing = ["é", "日本語", "👩‍💻", "TERMINAL_GPU_READY"]
            .into_iter()
            .filter(|sample| !text.contains(sample))
            .collect::<Vec<_>>();
        let has_link = snapshot.lines.iter().any(|row| {
            row.cells
                .iter()
                .any(|cell| cell.hyperlink.as_deref() == Some("https://example.com"))
        });
        if !missing.is_empty() || snapshot.images.is_empty() || !has_link {
            return Err(io::Error::other(format!(
                "Terminal smoke fixture incomplete in active session {:?}: missing {missing:?}, images {}, OSC 8 {has_link}, viewport {text:?}",
                self.workbench.terminal.active_session_id(),
                snapshot.images.len(),
            ))
            .into());
        }
        if let Some(identity) = &self.terminal_promotion_identity {
            let panels: HashSet<_> = (0..self.workbench.tab_count())
                .filter_map(|index| self.workbench.tab_window_id(index))
                .collect();
            if std::process::id() != identity.process
                || self.window.id() != identity.window
                || self
                    .workbench
                    .workspace_spec
                    .as_ref()
                    .map(|spec| Path::new(&spec.root))
                    != Some(identity.root.as_path())
                || !panels.contains(&identity.panel)
                || identity
                    .panels
                    .iter()
                    .any(|panel| *panel != identity.panel && panels.contains(panel))
                || identity.terminals.iter().any(|(session, pid)| {
                    self.workbench.terminal.process_id(*session)
                        != (*session == identity.terminal).then_some(*pid)
                })
            {
                return Err(io::Error::other(
                    "Workspace upgrade did not promote only the calling shell",
                )
                .into());
            }
            self.workbench
                .terminal
                .live_working_directory(identity.terminal)?;
            eprintln!(
                "bEd: upgrade preserved the calling shell and closed other panels at {}",
                identity.root.display()
            );
        }
        if !self
            .terminal_initial_size
            .is_some_and(|size| size[0] > 900.0 && size[1] > 500.0)
        {
            return Err(io::Error::other("Initial terminal did not fill the window").into());
        }
        let expected_companions = usize::from(self.terminal_promotion_identity.is_none());
        if self.workbench.panel_count(bed_plugin_csv::PANEL_ID) != expected_companions
            || self.workbench.panel_count(bed_plugin_image::PANEL_ID) != expected_companions
            || (expected_companions != 0 && self.workbench.plugin_render_outputs().is_empty())
        {
            return Err(
                io::Error::other("Terminal smoke expected CSV and image companions").into(),
            );
        }
        eprintln!(
            "bEd: terminal smoke passed — full-size shell, terminal splits and {expected_sessions} surviving sessions"
        );
        Ok(())
    }

    fn check_terminal_splits(&self) -> HostResult<()> {
        let sessions = self.workbench.terminal.session_ids();
        let processes: HashSet<_> = sessions
            .iter()
            .map(|session| {
                self.workbench
                    .terminal
                    .process_id(*session)
                    .ok_or_else(|| io::Error::other("A smoke shell is not running"))
            })
            .collect::<io::Result<_>>()?;
        if sessions.len() != 3 || processes.len() != 3 {
            return Err(io::Error::other(format!(
                "Terminal smoke expected three independent processes, found {} shells and {} processes",
                sessions.len(),
                processes.len()
            ))
            .into());
        }
        let docks: HashSet<_> = self
            .workbench
            .terminal
            .session_ids()
            .into_iter()
            .filter_map(|session| self.workbench.terminal_panel_id(session))
            .filter_map(terminal_window_metadata)
            .map(|(dock, _)| dock)
            .collect();
        if docks.len() != 3 || docks.contains(&0) {
            return Err(
                io::Error::other("Terminal smoke expected three docked shell groups").into(),
            );
        }
        Ok(())
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
                settings.settings["font"] = serde_json::json!("Paper Mono");
                settings.settings["fontSize"] = serde_json::json!(24.0);
                settings.settings["treesitter"] = serde_json::json!(true);
                settings.request_apply();
                smoke.phase = 1;
            }
            (1, 4..) => {
                settings.select_theme("solarized-light")?;
                settings.request_apply();
                smoke.phase = 2;
            }
            (2, 6..) => {
                settings.settings = smoke.original.clone();
                settings.request_apply();
                smoke.phase = 3;
                eprintln!("bEd: appearance smoke restored font/theme settings");
            }
            _ => {}
        }
        Ok(())
    }
    #[cfg(target_os = "macos")]
    fn advance_native_appearance_after_frame(&mut self) -> HostResult<()> {
        use crate::platform::macos_window::TitlebarAction;
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
                    .perform_for_smoke(crate::platform::macos_menu::MenuAction::Find)?;
                smoke.phase = 1;
            }
            (1, 4..) => {
                if self.workbench.active_overlay() != bed_editing::editor_events::Overlay::Find {
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
                self.workbench.settings.settings["background_opacity"] = serde_json::json!(0.35);
                self.workbench.settings.settings["mac_blur_enabled"] = serde_json::json!(false);
                smoke.phase = 2;
            }
            (2, 6..) => {
                if self.native_window.material_is_visible()
                    || self.native_window.content_is_opaque() != Some(false)
                    || self
                        .native_window
                        .content_opacity()
                        .is_none_or(|v| (v - 1.0).abs() > 0.001)
                {
                    return Err(
                        io::Error::other("native appearance opacity/blur reload failed").into(),
                    );
                }
                self.workbench.settings.settings["background_opacity"] = smoke.opacity.clone();
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
                    crate::platform::macos_menu::MenuAction::NewExplorer,
                    crate::platform::macos_menu::MenuAction::NewTerminal,
                    crate::platform::macos_menu::MenuAction::NewSettings,
                    crate::platform::macos_menu::MenuAction::NewContentSearch,
                    crate::platform::macos_menu::MenuAction::NewDiagnostics,
                    crate::platform::macos_menu::MenuAction::NewStructure,
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
                self.native_menu
                    .perform_for_smoke(crate::platform::macos_menu::MenuAction::SplitRight)?;
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
                self.native_menu
                    .perform_for_smoke(crate::platform::macos_menu::MenuAction::SplitDown)?;
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
                    "bEd: native menu/titlebar/material smoke passed; six tools created three panels each, both split menu actions created distinct document panes; toolbar frames {:?}",
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
        use crate::platform::macos_menu::MenuAction;
        let Some(smoke) = &mut self.menu_smoke else {
            return Ok(());
        };
        if smoke.phase < 14 && self.started.elapsed() > Duration::from_secs(30) {
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
                        bed_editing::editor_commands::CursorReveal::Ensure,
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
                    // ImGui can defer text behind native mouse events. Wait
                    // for delivery, then require the exact edit below.
                    let input_pending = self.context.binding().with_bound_context(|| unsafe {
                        let queue = &(*dear_imgui_rs::sys::igGetCurrentContext()).InputEventsQueue;
                        (0..queue.Size as usize).any(|index| {
                            (*queue.Data.add(index)).Type
                                == dear_imgui_rs::sys::ImGuiInputEventType_Text
                        })
                    });
                    if input_pending {
                        return Ok(());
                    }
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
            (11, _) => {
                self.native_menu
                    .perform_for_smoke(MenuAction::SaveDefaultLayout)?;
                smoke.phase = 12;
            }
            (12, _) => {
                let path = self.workbench.settings.config_dir.join("workspaces.json");
                let stored: serde_json::Value = serde_json::from_slice(&std::fs::read(path)?)?;
                let default = &stored["default_layout"];
                if default["version"] != 1
                    || default["panels"].as_array().is_none_or(|panels| {
                        panels.is_empty()
                            || panels.iter().any(|panel| {
                                panel.get("state").is_some() || panel.get("document").is_some()
                            })
                    })
                    || snapshot.bytes != smoke.edited
                {
                    return Err(io::Error::other(
                        "native Save Default Layout did not save a fresh arrangement",
                    )
                    .into());
                }
                self.native_menu
                    .perform_for_smoke(MenuAction::ResetDefaultLayout)?;
                smoke.phase = 13;
            }
            (13, _) => {
                let path = self.workbench.settings.config_dir.join("workspaces.json");
                let stored: serde_json::Value = serde_json::from_slice(&std::fs::read(path)?)?;
                if stored.get("default_layout").is_some() || snapshot.bytes != smoke.edited {
                    return Err(io::Error::other(
                        "native Reset Default Layout did not clear the saved arrangement",
                    )
                    .into());
                }
                smoke.phase = 14;
                eprintln!(
                    "bEd: native Save/Reset Default Layout passed and preserved the active document"
                );
            }
            _ => {}
        }
        Ok(())
    }
    #[cfg(target_os = "macos")]
    fn process_native_menu(&mut self) -> HostResult<bool> {
        use crate::platform::macos_menu::MenuAction;
        // NewFrame reconciles the native focused viewport before the workspace
        // identifies its active panel. Keep menu events queued until that frame.
        if self.focus_needs_frame {
            return Ok(false);
        }
        self.native_menu
            .set_workspace_available(self.workbench.workspace_spec.is_some());
        for dispatch in self.native_menu.poll() {
            if self.bedtime.activity(Instant::now()) {
                continue;
            }
            let action = dispatch.action;
            if action == MenuAction::Quit {
                return Ok(true);
            }
            if action == MenuAction::NewWindow {
                self.open_window(None)?;
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
                if terminal {
                    match action {
                        MenuAction::Copy => {
                            if let Some(text) = self.workbench.terminal.native_copy() {
                                self.context.set_clipboard_text(text);
                            }
                        }
                        MenuAction::Paste => {
                            if let Some(text) = self.context.clipboard_text() {
                                self.workbench.terminal.native_paste(text)?;
                            }
                        }
                        MenuAction::SelectAll => self.workbench.terminal.native_select_all()?,
                        _ => {}
                    }
                    continue;
                }
                if !dispatch.keyboard && !terminal && !self.context.io().want_text_input() {
                    let panel_action = match action {
                        MenuAction::Undo => Some(bed_workbench_api::PanelAction::Undo),
                        MenuAction::Redo => Some(bed_workbench_api::PanelAction::Redo),
                        _ => None,
                    };
                    if let Some(panel_action) = panel_action
                        && self.workbench.focused_plugin_action(panel_action)?
                    {
                        continue;
                    }
                }
                let route = if dispatch.keyboard {
                    crate::platform::macos_menu::input_shortcut(action, &self.workbench.settings)
                        .map(|(key, shift)| NativeEditRoute::Shortcut(key, shift || dispatch.shift))
                        .unwrap_or(NativeEditRoute::Ignore)
                } else {
                    native_edit_route(action, self.context.io().want_text_input())
                };
                match route {
                    NativeEditRoute::Ignore => {}
                    NativeEditRoute::SelectAllDocument => {
                        self.workbench
                            .focused_plugin_action(bed_workbench_api::PanelAction::SelectAll)?;
                    }
                    NativeEditRoute::Shortcut(key, shift) => {
                        queue_native_edit_shortcut(&mut self.context, key, shift)
                    }
                }
                continue;
            }
            if let Some(command) = native_menu_command(action, dispatch.keyboard) {
                if dispatch.keyboard {
                    self.workbench.dispatch(command)?;
                } else {
                    self.workbench.dispatch_from_menu(command)?;
                }
            }
        }
        for id in self.native_menu.poll_plugin_commands() {
            self.workbench.dispatch_command_from_menu(&id)?;
        }
        Ok(false)
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
                self.window.focus_window();
                let _ = self.window.request_inner_size(target);
                eprintln!(
                    "bEd: lifecycle requesting native resize to {}×{}",
                    life.target.width, life.target.height
                );
            }
            LifecyclePhase::Resizing
                if life.saw_resize
                    && self.window.has_focus()
                    && self.rendered_frames >= life.frames_at_transition + 2 =>
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
        if life.phase == LifecyclePhase::Restoring && self.window.is_minimized() == Some(false) {
            // Restoring is asynchronous; a focus request made while the
            // native window is still minimized can be ignored by macOS.
            if self.window.has_focus() {
                life.saw_focus_gain = true;
            } else {
                self.window.focus_window();
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
                        self.workbench.dispatch_command_from_menu(&id)?;
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
                smoke.frames_at_resize = self.rendered_frames;
                smoke.phase = 10;
            }
            10 if self.rendered_frames > smoke.frames_at_resize + 4 => {
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
                // JSON glTF opens as text by default; select its model viewer
                // through the same command exposed in the user interface.
                self.workbench
                    .dispatch_command(bed_plugin_gltf::OPEN_FILE_COMMAND)?;
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
                    "bEd: plugin smoke verified image/glTF GPU outputs, GLB/embedded glTF routing, resize, close/reopen and GPU recovery"
                );
            }
            _ => {}
        }
        Ok(())
    }
    fn smoke_complete(&self, smoke_test: bool) -> bool {
        if self.terminal_smoke && self.rendered_frames < 90 {
            return false;
        }
        if self.terminal_promotion_identity.is_some()
            && (self.workbench.workspace_spec.is_none()
                || self.workbench.terminal.native_copy().is_none())
        {
            return false;
        }
        if self.debugger_smoke && !self.debugger_ready {
            return false;
        }
        if self.git_smoke && !self.git_ready {
            return false;
        }
        if self
            .plugin_smoke
            .as_ref()
            .is_some_and(|smoke| smoke.phase < 15)
        {
            return false;
        }
        #[cfg(target_os = "macos")]
        if self
            .menu_smoke
            .as_ref()
            .is_some_and(|smoke| smoke.phase < 14)
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
        let consumed = runtime.bedtime.intercept(&event, Instant::now());
        // Capture the physical key/modifiers before ImGui's macOS remapping.
        // The terminal view consumes these events once; editor shortcuts still
        // receive their normal ImGui events through the platform below.
        if !consumed && let Err(error) = runtime.terminal_window_event(&event) {
            runtime.workbench.error = Some(error.to_string());
        }
        // Keep the pointer position current when movement wakes the overlay,
        // so the next fresh click reaches the position the user can see.
        if (!consumed || matches!(event, WindowEvent::CursorMoved { .. }))
            && let Err(error) =
                runtime
                    .platform
                    .handle_window_event(&mut runtime.context, &native.window, &event)
        {
            self.fail(event_loop, error);
            return;
        }
        if consumed {
            runtime.frame_pending = true;
            native.window.request_redraw();
            return;
        }
        match event {
            WindowEvent::PinchGesture { delta, phase, .. } => {
                use bed_workbench::shell::EditorPinchPhase;
                let phase = match phase {
                    winit::event::TouchPhase::Started => EditorPinchPhase::Started,
                    winit::event::TouchPhase::Moved => EditorPinchPhase::Moved,
                    winit::event::TouchPhase::Ended => EditorPinchPhase::Ended,
                    winit::event::TouchPhase::Cancelled => EditorPinchPhase::Cancelled,
                };
                let position = runtime.external_drag_position();
                runtime
                    .workbench
                    .queue_editor_pinch(native.viewport_id, position, delta, phase);
                runtime.frame_pending = true;
                native.window.request_redraw();
            }
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
                if let Err(error) = runtime.workbench.terminal.set_window_focused(focused) {
                    runtime.workbench.error = Some(error.to_string());
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
                        if runtime.terminal_smoke {
                            #[cfg(target_os = "macos")]
                            if let Err(error) = runtime.native_window.validate_control_layout() {
                                self.fail(event_loop, error);
                                return;
                            }
                            let checked = runtime
                                .context
                                .binding()
                                .with_bound_context(|| runtime.check_terminal_smoke());
                            if let Err(error) = checked {
                                self.fail(event_loop, error);
                                return;
                            }
                        }
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
                let position = runtime.external_drag_position();
                let modifiers = runtime.external_drag_modifiers();
                let event = runtime.pending_file_drop.get_or_insert_with(|| {
                    bed_workbench_api::ExternalFileDrag {
                        paths: Vec::new(),
                        position,
                        viewport: native.viewport_id,
                        phase: bed_workbench_api::ExternalFileDragPhase::Drop,
                        modifiers,
                    }
                });
                event.paths.push(path);
            }
            WindowEvent::HoveredFile(path) => {
                runtime.external_drag_viewport = native.viewport_id;
                if !runtime.external_drag_paths.contains(&path) {
                    runtime.external_drag_paths.push(path);
                }
                let event = bed_workbench_api::ExternalFileDrag {
                    paths: runtime.external_drag_paths.clone(),
                    position: runtime.external_drag_position(),
                    viewport: native.viewport_id,
                    phase: bed_workbench_api::ExternalFileDragPhase::Hover,
                    modifiers: runtime.external_drag_modifiers(),
                };
                if let Err(error) = runtime.workbench.external_file_drag(event) {
                    runtime.workbench.error = Some(error.to_string());
                }
            }
            WindowEvent::HoveredFileCancelled => {
                runtime.external_drag_paths.clear();
                let event = bed_workbench_api::ExternalFileDrag {
                    paths: Vec::new(),
                    position: runtime.external_drag_position(),
                    viewport: native.viewport_id,
                    phase: bed_workbench_api::ExternalFileDragPhase::Cancel,
                    modifiers: runtime.external_drag_modifiers(),
                };
                if let Err(error) = runtime.workbench.external_file_drag(event) {
                    runtime.workbench.error = Some(error.to_string());
                }
            }
            WindowEvent::CursorMoved { .. } if !runtime.external_drag_paths.is_empty() => {
                let event = bed_workbench_api::ExternalFileDrag {
                    paths: runtime.external_drag_paths.clone(),
                    position: runtime.external_drag_position(),
                    viewport: native.viewport_id,
                    phase: bed_workbench_api::ExternalFileDragPhase::Hover,
                    modifiers: runtime.external_drag_modifiers(),
                };
                if let Err(error) = runtime.workbench.external_file_drag(event) {
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
        if let Some(runtime) = &mut self.runtime
            && let Some(event) = runtime.pending_file_drop.take()
        {
            runtime.external_drag_paths.clear();
            if let Err(error) = runtime.workbench.external_file_drag(event) {
                runtime.workbench.error = Some(error.to_string());
            }
        }
        if let Some(runtime) = &mut self.runtime
            && !runtime.external_drag_paths.is_empty()
        {
            let event = bed_workbench_api::ExternalFileDrag {
                paths: runtime.external_drag_paths.clone(),
                position: runtime.external_drag_position(),
                viewport: runtime.external_drag_viewport,
                phase: bed_workbench_api::ExternalFileDragPhase::Hover,
                modifiers: runtime.external_drag_modifiers(),
            };
            if let Err(error) = runtime.workbench.external_file_drag(event) {
                runtime.workbench.error = Some(error.to_string());
            }
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
            runtime.tick_workbench();
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

fn new_instance_command(
    executable: &Path,
    config_dir: &Path,
    wait: bool,
    workspace: Option<&bed_workbench_api::workspace::WorkspaceSpec>,
) -> io::Result<std::process::Command> {
    let config_dir = std::fs::canonicalize(config_dir)?;
    // Launch Services needs -n to start another process for a running bundle.
    // Source builds run the executable directly. Neither path inherits the
    // parent's project arguments or smoke flags.
    #[cfg(target_os = "macos")]
    let bundle = executable
        .parent()
        .filter(|path| path.file_name().is_some_and(|name| name == "MacOS"))
        .and_then(Path::parent)
        .filter(|path| path.file_name().is_some_and(|name| name == "Contents"))
        .and_then(Path::parent)
        .filter(|path| path.extension().is_some_and(|extension| extension == "app"));
    #[cfg(target_os = "macos")]
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
    #[cfg(not(target_os = "macos"))]
    let mut command = std::process::Command::new(executable);
    command
        .arg("--new-window")
        .arg("--config-dir")
        .arg(config_dir)
        .stdin(std::process::Stdio::null());
    if let Some(workspace) = workspace {
        command
            .arg("--workspace")
            .arg(workspace.to_value().to_string());
    }
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
    action: crate::platform::macos_menu::MenuAction,
    keyboard: bool,
) -> Option<WindowCommand> {
    use crate::platform::macos_menu::MenuAction;
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
        MenuAction::SaveDefaultLayout => WindowCommand::SaveDefaultLayout,
        MenuAction::ResetDefaultLayout => WindowCommand::ResetDefaultLayout,
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
    action: crate::platform::macos_menu::MenuAction,
    text_input_focused: bool,
) -> NativeEditRoute {
    use crate::platform::macos_menu::MenuAction;
    use dear_imgui_rs::Key;
    match action {
        MenuAction::Undo => NativeEditRoute::Shortcut(Key::Z, false),
        MenuAction::Redo => NativeEditRoute::Shortcut(Key::Z, true),
        MenuAction::Cut => NativeEditRoute::Shortcut(Key::X, false),
        MenuAction::Copy => NativeEditRoute::Shortcut(Key::C, false),
        MenuAction::Paste => NativeEditRoute::Shortcut(Key::V, false),
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
    // the existing focused ImGui/editor input handler. Dear ImGui's
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
    let mut smoke_test = false;
    let mut options = RuntimeOptions::default();
    let mut config_dir = None;
    let mut startup_options = crate::startup::StartupOptions::default();
    let mut arguments = std::env::args_os().skip(1);
    while let Some(argument) = arguments.next() {
        if argument == "--term" || argument == "--edit" {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "--term and --edit have been removed; choose Startup in General settings",
            )
            .into());
        } else if argument == "--terminal-smoke" || argument == "--term-promotion-smoke" {
            smoke_test = true;
            options.terminal_smoke = true;
            options.terminal_promotion_smoke = argument == "--term-promotion-smoke";
        } else if argument == "--workspace" {
            let value = arguments.next().ok_or_else(|| {
                io::Error::other("--workspace requires a workspace specification")
            })?;
            let value: serde_json::Value = serde_json::from_str(
                value
                    .to_str()
                    .ok_or_else(|| io::Error::other("Workspace specification is not UTF-8"))?,
            )?;
            startup_options.requested_workspace = Some(
                bed_workbench_api::workspace::WorkspaceSpec::from_value(&value)
                    .ok_or_else(|| io::Error::other("Invalid workspace specification"))?,
            );
        } else if argument == "--cwd" {
            startup_options.cwd =
                Some(PathBuf::from(arguments.next().ok_or_else(|| {
                    io::Error::other("--cwd requires a directory")
                })?));
        } else if argument == "--smoke-test" || argument == "--main-only-smoke" {
            smoke_test = true;
        } else if argument == "--viewports-smoke" {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "--viewports-smoke is unavailable: detached native windows are not supported in v1",
            )
            .into());
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
        } else if argument == "--git-smoke" {
            options.git = true;
            smoke_test = true;
        } else if argument == "--config-dir" {
            config_dir =
                Some(PathBuf::from(arguments.next().ok_or_else(|| {
                    io::Error::other("--config-dir requires a directory")
                })?));
        } else if argument == "--new-window" {
            startup_options.new_window = true;
        } else if argument == "--help" || argument == "-h" {
            println!(
                "Usage: bed [FILE_OR_FOLDER ...] [--cwd DIRECTORY]\n           [--smoke-test | --lifecycle-smoke | --effects-smoke]\n           [--appearance-smoke] [--platform-smoke] [--menu-smoke] [--plugin-smoke]\n           [--debug-smoke] [--git-smoke] [--main-only-smoke]\n           [--terminal-smoke | --term-promotion-smoke]\n           [--capture-frame OUTPUT.ppm] [--capture-after-frames N] [--config-dir DIRECTORY]\n\nGeneral → Startup selects Startup screen (default), Terminal, or Last project.\nLast project falls back to Startup when none exists; standalone launches use home.\nNew Window always opens Startup.\nWindow → Save Default Layout captures fresh panels for the first open of a workspace.\nOnly an exact saved workspace folder attaches at launch; other folders stay standalone. The last folder wins.\nExplicit paths and --cwd suppress Last project restoration.\nFile arguments open after the initial panels in their original order.\n--cwd sets the initial working directory without selecting a workspace.\nRun +open FILE or +o FILE inside a shell to open a viewer beside that shell.\nRun +workspace or +w to create or open a workspace while keeping the calling shell alive.\nCmd/Ctrl+O open · Cmd/Ctrl+S save · Cmd/Ctrl+F find · Cmd/Ctrl+; go to line\n--menu-smoke uses isolated fixtures to verify native editing, new windows, and default layout actions."
            );
            return Ok(());
        } else {
            let path = PathBuf::from(argument);
            paths.push(path);
        }
    }
    let _terminal_fixture = if options.terminal_smoke {
        if options.menu_edit || options.plugins || options.git || options.debugger {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Run terminal smoke separately from other feature smoke fixtures",
            )
            .into());
        }
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "bed-terminal-smoke-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir(&root)?;
        let fixture = TemporaryMenuFixture(root);
        // Promotion attaches this fresh root, so saved user workspace terminals
        // cannot become part of the fixture's three-shell count.
        paths.clear();
        startup_options.cwd = Some(fixture.0.clone());
        config_dir = Some(fixture.0.join("config"));
        Some(fixture)
    } else {
        None
    };
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
        if options.menu_edit || options.native_appearance || options.appearance {
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
        let resources = Settings::get_app_resources_path();
        let icon = resources.join("resources/icons/bed.png");
        let icon = if icon.is_file() {
            icon
        } else {
            resources.join("assets/bEd-iOS-Default-1024@1x.png")
        };
        std::fs::copy(icon, &image)?;
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
    let _git_fixture = if options.git {
        if options.menu_edit || options.plugins || options.native_appearance || options.appearance {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Run --git-smoke separately from other feature smoke fixtures",
            )
            .into());
        }
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("bed-git-smoke-{}-{unique}", std::process::id()));
        std::fs::create_dir(&root)?;
        let fixture = TemporaryMenuFixture(root);
        let project = fixture.0.join("project");
        std::fs::create_dir(&project)?;
        let git = |args: &[&str]| -> io::Result<()> {
            let output = std::process::Command::new("git")
                .arg("-C")
                .arg(&project)
                .args(args)
                .output()?;
            if !output.status.success() {
                return Err(io::Error::other(
                    String::from_utf8_lossy(&output.stderr).into_owned(),
                ));
            }
            Ok(())
        };
        git(&["init", "-q"])?;
        git(&["config", "user.name", "bEd Smoke"])?;
        git(&["config", "user.email", "smoke@example.invalid"])?;
        git(&["config", "core.hooksPath", ".git/hooks"])?;
        let original = b"fn greeting(name: &str) {\n    println!(\"Hello, {name}!\");\n}\n\nfn main() {\n    greeting(\"world\");\n}\n";
        let staged = b"fn greeting(name: &str) {\n    println!(\"Welcome, {name}!\");\n}\n\nfn main() {\n    greeting(\"world\");\n}\n";
        let working = b"fn greeting(name: &str) {\n    println!(\"Welcome, {name}!\");\n}\n\nfn main() {\n    let name = \"bEd\";\n    greeting(name);\n}\n";
        std::fs::write(project.join("main.rs"), original)?;
        git(&["add", "--", "main.rs"])?;
        git(&[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "Initial greeting",
        ])?;
        std::fs::write(project.join("main.rs"), staged)?;
        git(&["add", "--", "main.rs"])?;
        std::fs::write(project.join("main.rs"), working)?;
        std::fs::write(project.join("notes.txt"), b"A new file, ready to stage.\n")?;
        paths.clear();
        paths.push(project.clone());
        paths.push(project.join("main.rs"));
        config_dir = Some(fixture.0.join("config"));
        Some(fixture)
    } else {
        None
    };
    let settings = match config_dir {
        Some(path) => Settings::with_paths(path, Settings::get_app_resources_path())?,
        None => Settings::new()?,
    };
    if options.debugger
        || options.git
        || options.plugins
        || options.menu_edit
        || options.native_appearance
        || options.appearance
    {
        let mut store =
            bed_workbench::workspace::store::WorkspaceStore::load(&settings.config_dir)?;
        for root in paths.iter().filter(|path| path.is_dir()) {
            store.record_project(root)?;
        }
    }
    let process_cwd = std::env::current_dir().ok();
    let startup = startup_options.resolve(paths, process_cwd.as_deref())?;
    if options.terminal_smoke && options.capture_after_frames.is_none() {
        options.capture_after_frames = Some(90);
    }
    let mut workbench = startup.into_workbench(settings)?;
    if options.terminal_smoke {
        // Keep smoke output deterministic without running the user's shell RC.
        // Retain the terminal service so its shell bridge environment survives.
        workbench
            .terminal
            .configure_shell(bed_terminal::terminal_pty::TerminalShell::new(
                "/bin/sh",
                vec!["-i".into()],
            ));
        workbench
            .terminal
            .configure_shell_environment(HashMap::from([
                ("ENV".into(), String::new()),
                ("BASH_ENV".into(), String::new()),
                ("PS1".into(), "bed smoke> ".into()),
                ("PS2".into(), "> ".into()),
            ]));
        // The native smoke fixture starts with a single terminal independently
        // of the user's default layout; ordinary launch selection has one path.
        while workbench.tab_count() > 0 {
            if !workbench.close_tab(0)? {
                return Err(
                    io::Error::other("Could not clear terminal smoke fixture panels").into(),
                );
            }
        }
        workbench.dispatch(WindowCommand::ResetLayout)?;
        workbench.dispatch(WindowCommand::NewTerminal)?;
    }

    if options.debugger {
        workbench.debug_smoke_setup()?;
    }
    if options.git {
        workbench.git_smoke_setup("main.rs")?;
    }
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

fn terminal_window_metadata(panel: u64) -> Option<(u32, [f32; 2])> {
    unsafe {
        let context = dear_imgui_rs::sys::igGetCurrentContext();
        if context.is_null() {
            return None;
        }
        let suffix = format!("###bed_tab_{panel}");
        let windows = &(*context).Windows;
        for index in 0..windows.Size as usize {
            let window = &**windows.Data.add(index);
            let name = std::ffi::CStr::from_ptr(window.Name).to_string_lossy();
            if name.ends_with(&suffix) {
                return Some((window.DockId, [window.Size.x, window.Size.y]));
            }
        }
        None
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
#[path = "../tests/unit/native/host_tests.rs"]
mod tests;
