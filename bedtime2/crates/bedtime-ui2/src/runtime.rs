use crate::{
    platform::UiPlatform,
    render::{DeviceBridge, FrameSlot, Packet, Renderer},
    *,
};
use bevy::{
    ecs::message::MessageCursor,
    winit::{RawWinitWindowEvent, WINIT_WINDOWS},
};
use dear_imgui_rs::{ClipboardBackend, ConfigFlags, Context, FontSource};
use dear_imgui_wgpu::{WgpuInitInfo, WgpuSnapshotContext, WgpuSnapshotRenderer};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex},
    time::Instant,
};

#[derive(Default)]
struct ClipboardSlot {
    service: Option<bedtime_desktop2::DesktopClipboard>,
    errors: Vec<String>,
}

struct ClipboardAdapter(Rc<RefCell<ClipboardSlot>>);

impl ClipboardBackend for ClipboardAdapter {
    fn get(&mut self) -> Option<String> {
        let mut slot = self.0.borrow_mut();
        match slot.service.as_mut()?.read_text() {
            Ok(value) => Some(value),
            Err(error) => {
                slot.errors.push(error.to_string());
                None
            }
        }
    }
    fn set(&mut self, value: &str) {
        let mut slot = self.0.borrow_mut();
        if let Some(service) = slot.service.as_mut() {
            if let Err(error) = service.write_text(value) {
                slot.errors.push(error.to_string());
            }
        }
    }
}

/// Main-thread native ownership only. Application presentation state lives in ECS.
pub(crate) struct Runtime {
    context: Context,
    platform: UiPlatform,
    owner: Option<WgpuSnapshotContext>,
    renderer: Option<Renderer>,
    generation: u64,
    theme: Option<Theme>,
    events: MessageCursor<RawWinitWindowEvent>,
    clipboard: Rc<RefCell<ClipboardSlot>>,
    animations: RefCell<crate::motion::Animations>,
    icons: RefCell<crate::icon_cache::IconCache>,
    icon_surface: Option<Entity>,
    viewports: RefCell<crate::viewport_textures::ViewportTextures>,
}

impl Runtime {
    pub fn new() -> Self {
        let mut context = Context::create();
        context
            .set_ini_filename(None::<std::path::PathBuf>)
            .expect("UI ini disabled; persistence belongs to host");
        context
            .io_mut()
            .set_config_flags(ConfigFlags::NAV_ENABLE_KEYBOARD);
        let clipboard = Rc::new(RefCell::new(ClipboardSlot::default()));
        context.set_clipboard_backend(ClipboardAdapter(clipboard.clone()));
        let platform = UiPlatform::new(&mut context);
        Self {
            context,
            platform,
            owner: None,
            renderer: None,
            generation: 0,
            theme: None,
            events: MessageCursor::default(),
            clipboard,
            animations: RefCell::default(),
            icons: RefCell::default(),
            icon_surface: None,
            viewports: RefCell::default(),
        }
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        if let (Some(renderer), Some(owner)) = (&self.renderer, &mut self.owner) {
            // Outstanding render snapshots may still be retiring on a pipelined
            // render thread. Owner Drop safely ends their consumer generation.
            let _ = renderer.lock().unwrap().shutdown(owner, &mut self.context);
        }
        self.owner.take();
        self.platform.shutdown(&mut self.context);
    }
}

pub(crate) fn draw(world: &mut World) {
    let surfaces: Vec<_> = world
        .query_filtered::<Entity, With<UiSurface>>()
        .iter(world)
        .collect();
    if surfaces.is_empty() {
        let devices = world.resource::<DeviceBridge>().0.lock().unwrap().clone();
        let mut runtime = world.non_send_mut::<Runtime>();
        let runtime = &mut *runtime;
        *runtime.animations.get_mut() = Default::default();
        if runtime.icon_surface.take().is_some() {
            runtime.icons.get_mut().clear(&mut runtime.context);
        }
        if let (Some(devices), Some(renderer)) = (devices, runtime.renderer.as_ref()) {
            runtime
                .viewports
                .borrow_mut()
                .begin_frame(&devices, renderer);
        }
        return;
    }
    if surfaces.len() != 1 {
        report(
            world,
            Some(surfaces[0]),
            "Stage 2 supports exactly one UI surface".into(),
        );
        return;
    }
    let surface = surfaces[0];
    let result = (|| {
        let theme_id = world
            .get::<UsesTheme>(surface)
            .ok_or("UI surface needs a UsesTheme relationship")?
            .0;
        let theme = world
            .get::<Theme>(theme_id)
            .ok_or("Selected theme entity has no Theme component")?
            .clone();
        if world.non_send::<Runtime>().theme.as_ref() != Some(&theme) {
            theme.validate()?;
        }
        let background = bedtime_desktop2::WindowBackground::new(theme.palette.background);
        if world.get::<bedtime_desktop2::WindowBackground>(surface) != Some(&background) {
            world.entity_mut(surface).insert(background);
        }
        let devices = world.resource::<DeviceBridge>().0.lock().unwrap().clone();
        let Some(devices) = devices else {
            return Ok(());
        };
        // Temporarily own thread-bound machinery so drawing systems can query the
        // world normally. Nothing exposes Context or a raw frame pointer in ECS.
        let mut runtime = world
            .remove_non_send::<Runtime>()
            .expect("UI runtime installed");
        runtime.clipboard.borrow_mut().service =
            world.remove_non_send::<bedtime_desktop2::DesktopClipboard>();
        let result = WINIT_WINDOWS.with(|windows| {
            let windows = windows.borrow();
            let Some(window) = windows.get_window(surface) else {
                return Ok(());
            };
            let physical = window.inner_size();
            if physical.width == 0 || physical.height == 0 {
                return Ok(());
            }
            if runtime.generation != devices.generation {
                runtime.renderer.take();
                runtime.owner.take();
                let (renderer, owner) = WgpuSnapshotRenderer::new(
                    WgpuInitInfo::new(
                        devices.device.clone(),
                        devices.queue.clone(),
                        wgpu::TextureFormat::Rgba8Unorm,
                    ),
                    &mut runtime.context,
                )
                .map_err(|e| e.to_string())?;
                runtime.renderer = Some(Arc::new(Mutex::new(renderer)));
                runtime.owner = Some(owner);
                runtime.generation = devices.generation;
            }
            runtime
                .owner
                .as_mut()
                .unwrap()
                .poll_completions(&mut runtime.context)
                .map_err(|e| e.to_string())?;
            // Color/spacing changes never rebuild the atlas. One bundled regular
            // face is shared by all contexts; ImGui handles dynamic DPI rasterization.
            if runtime.theme.as_ref().map(|t| t.metrics.font_size) != Some(theme.metrics.font_size)
            {
                let atlas = runtime.context.font_atlas();
                atlas.clear();
                let data = include_bytes!("../assets/fonts/PaperMono-Regular.ttf");
                // SAFETY: embedded font bytes live for the lifetime of the process.
                let source =
                    unsafe { FontSource::ttf_data_with_size(data, theme.metrics.font_size) };
                atlas.add_font(&[source]);
            }
            if runtime.theme.as_ref() != Some(&theme) {
                theme.apply(&mut runtime.context);
                runtime.theme = Some(theme.clone());
            }
            for event in runtime
                .events
                .read(world.resource::<Messages<RawWinitWindowEvent>>())
            {
                if event.window_id == window.id() {
                    runtime
                        .platform
                        .handle_event(&mut runtime.context, window, &event.event);
                }
            }
            runtime.platform.prepare_frame(&mut runtime.context, window);
            runtime
                .viewports
                .borrow_mut()
                .begin_frame(&devices, runtime.renderer.as_ref().unwrap());
            if runtime.icon_surface != Some(surface) {
                runtime.icons.get_mut().clear(&mut runtime.context);
                runtime.icon_surface = Some(surface);
            }
            let previous_error = runtime.icons.borrow().status.error.clone();
            let common_pixels =
                (theme.metrics.font_size * window.scale_factor() as f32).ceil() as u32;
            if let Err(error) = runtime.icons.get_mut().prepare(
                &mut runtime.context,
                world.resource::<Assets<Icon>>(),
                world.resource::<Assets<IconSet>>(),
                world.resource::<crate::icons::BundledIcons>(),
                world.get::<UiIcons>(surface),
                world.resource::<AssetServer>(),
                common_pixels,
            ) {
                runtime.icons.get_mut().status.error = Some(error.to_string());
            }
            let status = runtime.icons.borrow().status.clone();
            if status.error != previous_error {
                if let Some(error) = &status.error {
                    world.write_message(UiError(format!("Icons: {error}")));
                }
            }
            *world.get_mut::<UiIconStatus>(surface).unwrap() = status;
            let views = world
                .get::<SurfaceViews>(surface)
                .map(|v| v.collection().to_vec())
                .unwrap_or_default();
            let began = Instant::now();
            let token = runtime.context.begin_frame();
            runtime.animations.borrow_mut().begin_frame(
                token.ui().frame_count(),
                world.resource::<Time<Real>>().elapsed_secs_f64(),
            );
            for view in views {
                if world.get::<UiView>(view).is_none() {
                    continue;
                }
                let frame = UiFrame {
                    view,
                    surface,
                    ui: token.ui(),
                    theme: &theme,
                    motion: *world.get::<MotionPolicy>(surface).unwrap(),
                    animations: &runtime.animations,
                    icons: &runtime.icons,
                    viewports: &runtime.viewports,
                };
                let id = SystemId::<UiFrame<'static>>::from_entity(view);
                world
                    .run_system_with(id, frame)
                    .map_err(|e| e.to_string())?;
            }
            let ui = token.ui();
            let (effects, active_animations) =
                runtime.animations.borrow_mut().finish_frame(|view| {
                    world.get::<UiView>(view).is_some()
                        && world
                            .get::<OnSurface>(view)
                            .is_some_and(|owner| owner.0 == surface)
                });
            crate::motion::apply_popups(ui, &effects);
            *world.get_mut::<UiCapture>(surface).unwrap() = UiCapture {
                keyboard: ui.io().want_capture_keyboard(),
                pointer: ui.io().want_capture_mouse(),
                text: ui.io().want_text_input(),
            };
            runtime.platform.prepare_render(ui, window);
            let snapshot = token
                .render_snapshot(
                    runtime
                        .owner
                        .as_ref()
                        .unwrap()
                        .renderer_consumer()
                        .map_err(|e| e.to_string())?,
                )
                .map_err(|e| e.to_string())?;
            runtime.platform.update_ime(window);
            let mut diag = world.get_mut::<UiDiagnostics>(surface).unwrap();
            diag.frames += 1;
            diag.vertices = snapshot
                .draw_data()
                .draw_lists
                .iter()
                .map(|list| list.vtx.len())
                .sum();
            diag.indices = snapshot
                .draw_data()
                .draw_lists
                .iter()
                .map(|list| list.idx.len())
                .sum();
            diag.draw_ms = began.elapsed().as_secs_f64() * 1000.0;
            diag.error = None;
            diag.active_animations = active_animations;
            diag.icon_textures = runtime.icons.borrow().textures();
            diag.icon_rasterizations = runtime.icons.borrow().rasterizations;
            let viewport_textures = runtime.viewports.borrow().frame_textures();
            diag.viewport_textures = viewport_textures.len();
            world.resource_mut::<FrameSlot>().0 = Some(Packet {
                snapshot,
                renderer: runtime.renderer.as_ref().unwrap().clone(),
                window: surface,
                size: [physical.width, physical.height],
                generation: runtime.generation,
                images: viewport_textures,
            });
            // A reactive desktop event loop must keep producing frames after
            // the pointer stops moving, until our transitions have settled.
            if active_animations > 0 || runtime.icons.borrow().needs_redraw {
                world.write_message(bevy::window::RequestRedraw);
            }
            Ok(())
        });
        let (clipboard, errors) = {
            let mut slot = runtime.clipboard.borrow_mut();
            (slot.service.take(), std::mem::take(&mut slot.errors))
        };
        if let Some(clipboard) = clipboard {
            world.insert_non_send(clipboard);
        }
        world.insert_non_send(runtime);
        for error in errors {
            world.write_message(UiError(format!("Clipboard: {error}")));
        }
        result
    })();
    if let Err(error) = result {
        report(world, Some(surface), error);
    }
}

fn report(world: &mut World, surface: Option<Entity>, error: String) {
    if let Some(surface) = surface {
        let mut diag = world.get_mut::<UiDiagnostics>(surface).unwrap();
        if diag.error.as_ref() == Some(&error) {
            return;
        }
        diag.error = Some(error.clone());
    }
    world.write_message(UiError(error));
}

pub(crate) fn shutdown(world: &mut World) {
    if !world.resource::<Messages<AppExit>>().is_empty() {
        // Publish retirement before taking the renderer lock in Runtime::drop.
        // A pipelined frame either finishes while holding that lock or observes
        // retirement after acquiring it and drops its owned snapshot.
        world.resource::<DeviceBridge>().0.lock().unwrap().take();
        world.resource_mut::<FrameSlot>().0.take();
        world.remove_non_send::<Runtime>();
    }
}
