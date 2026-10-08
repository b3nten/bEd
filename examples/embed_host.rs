//! Runnable document-view host: two independent views of one shared document.
//! This example owns layout, native windows, fonts, backends and GUI/GPU frames.
use bed_core::editor_commands::CursorReveal;
use bed_session::{ClosePolicy, EditorSession, SessionOptions};
use bed_ui::{EditorView, EditorViewOptions, editor_input::HostAction};
use dear_imgui_rs::{ClipboardBackend, Condition, Context, FontId, FontSource, StyleColor};
use dear_imgui_wgpu::{FramebufferExtent, WgpuInitInfo, WgpuRenderer};
use dear_imgui_winit::{HiDpiMode, WinitPlatform};
use std::io;
use std::{
    error::Error,
    fs,
    future::Future,
    io::Write,
    path::PathBuf,
    sync::Arc,
    task::{Poll, Wake, Waker},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use winit::{
    application::ApplicationHandler,
    dpi::LogicalSize,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    window::{Window, WindowId},
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

struct ThreadWake(std::thread::Thread);
impl Wake for ThreadWake {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.unpark();
    }
}
fn block_on<T>(future: impl Future<Output = T>) -> T {
    let waker = Waker::from(Arc::new(ThreadWake(std::thread::current())));
    let mut cx = std::task::Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(value) => return value,
            Poll::Pending => std::thread::park(),
        }
    }
}

struct Fixture(PathBuf);

struct Clipboard(arboard::Clipboard);
impl ClipboardBackend for Clipboard {
    fn get(&mut self) -> Option<String> {
        self.0.get_text().ok()
    }
    fn set(&mut self, text: &str) {
        let _ = self.0.set_text(text);
    }
}
impl Fixture {
    fn new() -> io::Result<Self> {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("bed-embed-host-{}-{unique}", std::process::id()));
        fs::create_dir(&root)?;
        let fixture = Self(root);
        fs::write(
            fixture.0.join("first.rs"),
            "fn main() { println!(\"Host-owned frames 🙂\"); }\n",
        )?;
        fs::write(
            fixture.0.join("second.rs"),
            "// This tab has its own view and selections.\n",
        )?;
        Ok(fixture)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Host {
    // Cleanup explicitly stops Bed before the renderer/context. Bed never owns
    // these resources and can also safely be dropped after the host context.
    session: EditorSession,
    left: EditorView,
    right: EditorView,
    context: Context,
    platform: WinitPlatform,
    renderer: WgpuRenderer,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    window: Arc<Window>,
    host_font: FontId,
    frames: u32,
    started: Instant,
    cleaned: bool,
    reconfigure: bool,
}
impl Host {
    fn new(event_loop: &ActiveEventLoop, fixture: &Fixture) -> Result<Self> {
        let window = Arc::new(
            event_loop.create_window(
                Window::default_attributes()
                    .with_title("Bed embedding host")
                    .with_inner_size(LogicalSize::new(1400.0, 1000.0)),
            )?,
        );
        let mut context = Context::create();
        context.set_ini_filename(None::<PathBuf>)?;
        if let Ok(clipboard) = arboard::Clipboard::new() {
            context.set_clipboard_backend(Clipboard(clipboard));
        }
        let host_font = context
            .font_atlas()
            .add_font(&[FontSource::default_font_with_size(16.0)]);
        context
            .style_mut()
            .set_color(StyleColor::WindowBg, [0.08, 0.14, 0.20, 1.0]);
        let mut session = EditorSession::with_options(SessionOptions {
            project_root: Some(fixture.0.clone()),
            highlighting: true,
            ..SessionOptions::default()
        })?;
        let document = session.open_file(&fixture.0.join("first.rs"))?;
        let left = EditorView::new(&mut session, document)?;
        let right = EditorView::new(&mut session, document)?;
        session.request_focus(left.id())?;
        let mut platform = WinitPlatform::new(&mut context)?;
        platform.attach_window(Arc::clone(&window), HiDpiMode::Default, &mut context)?;
        platform.set_ime_auto_management(false);
        window.set_ime_allowed(true);

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle(
            Box::new(Arc::clone(&window)),
        ));
        let surface = instance.create_surface(Arc::clone(&window))?;
        let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            compatible_surface: Some(&surface),
            ..Default::default()
        }))?;
        let (device, queue) = block_on(
            adapter.request_device(&bed_plugin::gpu::renderer_device_descriptor(&adapter)),
        )?;
        let size = window.inner_size();
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .ok_or_else(|| io::Error::other("host surface has no compatible format"))?;
        if surface
            .get_capabilities(&adapter)
            .usages
            .contains(wgpu::TextureUsages::COPY_SRC)
        {
            config.usage |= wgpu::TextureUsages::COPY_SRC;
        }
        surface.configure(&device, &config);
        let renderer = WgpuRenderer::new(
            WgpuInitInfo::new(device.clone(), queue.clone(), config.format),
            &mut context,
        )?;
        eprintln!(
            "Bed embed host: {:?}, {}×{}, scale {}; host owns context/window/backends",
            adapter.get_info().backend,
            size.width,
            size.height,
            window.scale_factor()
        );
        Ok(Self {
            session,
            left,
            right,
            context,
            platform,
            renderer,
            surface,
            device,
            queue,
            config,
            window,
            host_font,
            frames: 0,
            started: Instant::now(),
            cleaned: false,
            reconfigure: false,
        })
    }
    fn resize(&mut self) {
        let size = self.window.inner_size();
        if size.width != 0 && size.height != 0 {
            self.config.width = size.width;
            self.config.height = size.height;
            self.surface.configure(&self.device, &self.config);
        }
    }
    fn redraw(&mut self, smoke: bool, capture: &mut Option<PathBuf>) -> Result<bool> {
        if smoke && self.started.elapsed() > Duration::from_secs(30) {
            return Err(io::Error::other("embedding smoke timed out").into());
        }
        let size = self.window.inner_size();
        if size.width == 0 || size.height == 0 {
            return Ok(false);
        }
        if self.reconfigure {
            self.resize();
            self.reconfigure = false;
        }
        let report = self.session.tick();
        for error in report.errors {
            eprintln!("Bed service {}: {}", error.service, error.message);
        }
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) => frame,
            wgpu::CurrentSurfaceTexture::Suboptimal(frame) => {
                self.reconfigure = true;
                frame
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.resize();
                return Ok(false);
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Ok(false);
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                return Err(io::Error::other("host surface validation failed").into());
            }
        };
        self.platform
            .prepare_frame(&mut self.context, &self.window)?;
        let ui = self.context.frame();
        let host_bg = ui.style_color(StyleColor::WindowBg);
        let mut actions = Vec::new();
        let mut draw_error = None;
        {
            let _font = ui.push_font(self.host_font);
            ui.window("Host controls")
                .position([20.0, 20.0], Condition::FirstUseEver)
                .size([600.0, 100.0], Condition::FirstUseEver)
                .build(|| {
                    ui.text_wrapped("The host owns these windows and the layout. Both widgets share text and undo; each keeps its own cursor and scroll.");
                    if ui.button("Focus left") { let _ = self.session.request_focus(self.left.id()); }
                    ui.same_line();
                    if ui.button("Focus right") { let _ = self.session.request_focus(self.right.id()); }
                });
            ui.window("Host document layout")
                .position([20.0, 150.0], Condition::FirstUseEver)
                .size([1320.0, 760.0], Condition::FirstUseEver)
                .build(|| {
                    let width = (ui.content_region_avail()[0] - 8.0) * 0.5;
                    for view in [&mut self.left, &mut self.right] {
                        match view.draw(
                            ui,
                            &mut self.session,
                            &EditorViewOptions {
                                size: [width, 0.0],
                                font: Some(self.host_font),
                                ..EditorViewOptions::default()
                            },
                        ) {
                            Ok(response) => actions.extend(
                                response
                                    .actions
                                    .into_iter()
                                    .map(|action| (response.document, action)),
                            ),
                            Err(error) => draw_error = Some(error),
                        }
                        ui.same_line();
                    }
                });
        }
        if let Some(error) = draw_error {
            return Err(error.into());
        }
        if ui.style_color(StyleColor::WindowBg) != host_bg
            || (smoke && self.session.view_count(self.left.document_id()) != 2)
        {
            return Err(
                io::Error::other("embedding changed host style or lost a shared view").into(),
            );
        }
        self.platform.prepare_render(ui, &self.window)?;
        let pending = self.context.render(self.renderer.renderer_consumer()?);
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.03,
                            g: 0.05,
                            b: 0.08,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            self.renderer.render(
                pending,
                &mut pass,
                FramebufferExtent::from_texture(&frame.texture),
            )?;
        }
        let readback = if self.frames == 15 && capture.is_some() {
            if !self.config.usage.contains(wgpu::TextureUsages::COPY_SRC) {
                return Err(io::Error::other("host surface does not support readback").into());
            }
            let extent = frame.texture.size();
            let stride = (extent.width * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
                * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
            let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("embedding screenshot"),
                size: u64::from(stride) * u64::from(extent.height),
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: &frame.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(stride),
                        rows_per_image: Some(extent.height),
                    },
                },
                extent,
            );
            Some((buffer, stride, extent))
        } else {
            None
        };
        self.queue.submit([encoder.finish()]);
        frame.present();
        if let Some((buffer, stride, extent)) = readback {
            let (sender, receiver) = std::sync::mpsc::channel();
            buffer
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |result| {
                    let _ = sender.send(result);
                });
            self.device.poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(Duration::from_secs(10)),
            })?;
            receiver.recv_timeout(Duration::from_secs(10))??;
            let bytes = buffer.slice(..).get_mapped_range();
            let path = capture.take().unwrap();
            let mut output = io::BufWriter::new(fs::File::create(&path)?);
            write!(output, "P6\n{} {}\n255\n", extent.width, extent.height)?;
            let bgra = matches!(
                self.config.format,
                wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
            );
            for row in bytes.chunks_exact(stride as usize) {
                for pixel in row[..extent.width as usize * 4].as_chunks::<4>().0 {
                    let rgb = if bgra {
                        [pixel[2], pixel[1], pixel[0]]
                    } else {
                        [pixel[0], pixel[1], pixel[2]]
                    };
                    output.write_all(&rgb)?;
                }
            }
            output.flush()?;
            drop(bytes);
            buffer.unmap();
            eprintln!(
                "Bed embed host: captured {}×{} GPU frame to {}",
                extent.width,
                extent.height,
                path.display()
            );
        }
        for (document, action) in actions {
            match action {
                HostAction::Save => {
                    self.session.save(document)?;
                }
                HostAction::SaveAs => {
                    if let Some(path) = rfd::FileDialog::new().save_file() {
                        self.session.save_as(document, &path)?;
                    }
                }
                HostAction::Open => {
                    if let Some(path) = rfd::FileDialog::new().pick_file() {
                        let document = self.session.open_file(&path)?;
                        self.right = EditorView::new(&mut self.session, document)?;
                        self.session.request_focus(self.right.id())?;
                    }
                }
            }
        }
        self.frames += 1;
        if smoke && self.frames == 4 {
            self.session.with_commands(self.left.id(), |commands| {
                commands.set_cursor(0, 0, false, CursorReveal::Ensure);
                commands.type_text(b"// Shared edit from the host\n");
            })?;
        }
        if smoke && self.frames == 8 {
            self.session.request_focus(self.right.id())?;
        }
        if smoke && self.frames == 12 {
            self.session
                .with_commands(self.right.id(), |commands| commands.undo())?;
        }
        if smoke && self.frames == 16 {
            eprintln!(
                "Bed embed host: 16 native GPU frames passed; shared text/undo, two views, host layout/font/style and frame ownership retained"
            );
            return Ok(true);
        }
        Ok(false)
    }
    fn cleanup(&mut self) -> Result<()> {
        if !self.cleaned {
            self.session.shutdown(ClosePolicy::Discard)?;
            self.renderer.shutdown(&mut self.context)?;
            self.cleaned = true;
        }
        Ok(())
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

struct Application {
    host: Option<Host>,
    fixture: Fixture,
    smoke: bool,
    error: Option<String>,
    capture: Option<PathBuf>,
}
impl Application {
    fn fail(&mut self, event_loop: &ActiveEventLoop, error: impl std::fmt::Display) {
        self.error = Some(error.to_string());
        event_loop.exit();
    }
}
impl ApplicationHandler for Application {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.host.is_none() {
            match Host::new(event_loop, &self.fixture) {
                Ok(host) => {
                    host.window.request_redraw();
                    self.host = Some(host);
                }
                Err(error) => self.fail(event_loop, error),
            }
        }
    }
    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let Some(host) = &mut self.host else {
            return;
        };
        if id != host.window.id() {
            return;
        }
        if let Err(error) =
            host.platform
                .handle_window_event(&mut host.context, &host.window, &event)
        {
            self.fail(event_loop, error);
            return;
        }
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(_) => host.resize(),
            WindowEvent::RedrawRequested => match host.redraw(self.smoke, &mut self.capture) {
                Ok(true) => event_loop.exit(),
                Ok(false) => {}
                Err(error) => self.fail(event_loop, error),
            },
            _ => {}
        }
    }
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(host) = &self.host {
            host.window.request_redraw();
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(
            Instant::now() + Duration::from_millis(16),
        ));
    }
    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(host) = &mut self.host
            && let Err(error) = host.cleanup()
        {
            self.error.get_or_insert(error.to_string());
        }
    }
}
fn main() -> Result<()> {
    let mut smoke = false;
    let mut capture = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--smoke-test" => smoke = true,
            "--capture-frame" => {
                capture = Some(PathBuf::from(
                    args.next().ok_or("--capture-frame requires a path")?,
                ))
            }
            _ => return Err(format!("Unknown argument: {arg}").into()),
        }
    }
    let mut application = Application {
        host: None,
        fixture: Fixture::new()?,
        smoke,
        error: None,
        capture,
    };
    EventLoop::new()?.run_app(&mut application)?;
    match application.error {
        Some(error) => Err(io::Error::other(error).into()),
        None => Ok(()),
    }
}
