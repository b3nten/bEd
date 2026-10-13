//! Native acceptance scenarios drive the same captured operations as editor clients.
#[cfg(feature = "terminal")]
use crate::application::WorkspaceState;
#[cfg(any(
    all(feature = "image-viewer", feature = "model-viewer"),
    all(target_os = "macos", feature = "editor"),
    feature = "git",
    feature = "terminal"
))]
use crate::application::panel_presentation;
use crate::startup::SmokeOptions;
#[cfg(any(
    all(target_os = "macos", feature = "editor"),
    feature = "git",
    feature = "terminal"
))]
use bed_api::documents::PanelBinding;
#[cfg(all(feature = "image-viewer", feature = "model-viewer"))]
use bed_desktop::DesktopCanvasTargets;
#[cfg(all(target_os = "macos", feature = "editor"))]
use bed_desktop::DesktopContext;
use bed_desktop::{DesktopGpu, DesktopRenderStatus};
#[cfg(any(all(target_os = "macos", feature = "editor"), feature = "terminal"))]
use bed_desktop::{DesktopInjectedEvent, DesktopInputStep};
#[cfg(any(
    all(feature = "image-viewer", feature = "model-viewer"),
    feature = "git",
    feature = "terminal"
))]
use bed_plugin_automation::Readiness;
use bed_plugin_automation::{
    AutomationRequests, AutomationResults, CaptureTarget, Operation, OperationId, OperationValue,
    PanelHandle, Snapshot,
};
use bed_ui::TextureHandle;
#[cfg(all(feature = "image-viewer", feature = "model-viewer"))]
use bed_ui::gpu::CanvasOutput;
use bevy::{prelude::*, window::PrimaryWindow, winit::WINIT_WINDOWS};
use serde_json::{Value, json};
#[cfg(any(all(target_os = "macos", feature = "editor"), feature = "terminal"))]
use std::collections::HashSet;
use std::{
    io,
    path::PathBuf,
    time::{Duration, Instant},
};

#[derive(Resource, Default)]
pub(crate) struct SmokeFailure(pub std::sync::Arc<std::sync::Mutex<Option<String>>>);
pub(crate) struct SmokePlugin(pub SmokeOptions);
impl Plugin for SmokePlugin {
    fn build(&self, app: &mut App) {
        if !self.0.requested() {
            return;
        }
        app.world_mut()
            .resource_mut::<crate::app::NativeSmokeMode>()
            .0 = true;
        app.init_resource::<SmokeFailure>()
            .insert_resource(Smoke::new(self.0.clone()))
            .add_systems(Last, advance.before(bevy::app::OnAppExitSystems));
    }
}

/// One scenario owns one transition at a time. Inspect is observational and
/// never substitutes for an operation receipt or a completed GPU readback.
#[derive(Resource)]
struct Smoke {
    options: SmokeOptions,
    scenario: usize,
    phase: u32,
    activated: bool,
    presentation_progress: (u64, Instant),
    started: Instant,
    phase_frame: u64,
    operation: Option<OperationId>,
    application_operation: Option<crate::application::AppRequestId>,
    inspect: Option<OperationId>,
    snapshot: Option<Snapshot>,
    fixture: Option<Fixture>,
    // Loaded assets and workers retain their source files until app shutdown.
    retired_fixtures: Vec<Fixture>,
    panel: Option<PanelHandle>,
    other: Option<PanelHandle>,
    handle: Option<TextureHandle>,
    #[cfg(all(feature = "image-viewer", feature = "model-viewer"))]
    generation: u64,
    baseline: Vec<u8>,
    size: [u32; 2],
    entities: Vec<Entity>,
    gameplay: Option<Entity>,
    restore_at: Option<Instant>,
    saw_focus_loss: bool,
    saw_focus_gain: bool,
    original_settings: Value,
    #[cfg(feature = "terminal")]
    terminal_ids: Vec<(u64, u32)>,
    #[cfg(feature = "terminal")]
    terminal_panel: Option<Entity>,
    #[cfg(feature = "terminal")]
    terminal_window: Option<winit::window::WindowId>,
    children: Vec<std::process::Child>,
    #[cfg(all(target_os = "macos", feature = "editor"))]
    keyboard_save: bool,
}

#[cfg(all(feature = "image-viewer", feature = "model-viewer"))]
#[derive(Component)]
struct SmokeGameplay(u32);

struct Fixture(PathBuf);

#[cfg(any(
    all(feature = "image-viewer", feature = "model-viewer"),
    all(target_os = "macos", feature = "editor"),
    feature = "git",
    feature = "debugger",
    feature = "terminal"
))]
impl Fixture {
    fn new(name: &str) -> io::Result<Self> {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let path = std::env::temp_dir().join(format!(
            "bed-native-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path)?;
        Ok(Self(path.canonicalize()?))
    }

    fn write(&self, name: &str, bytes: impl AsRef<[u8]>) -> io::Result<PathBuf> {
        let path = self.0.join(name);
        std::fs::write(&path, bytes)?;
        Ok(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl Drop for Smoke {
    fn drop(&mut self) {
        for child in &mut self.children {
            if child.try_wait().ok().flatten().is_none() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

impl Smoke {
    fn new(options: SmokeOptions) -> Self {
        Self {
            options,
            scenario: 0,
            phase: 0,
            activated: false,
            presentation_progress: (0, Instant::now()),
            started: Instant::now(),
            phase_frame: 0,
            operation: None,
            application_operation: None,
            inspect: None,
            snapshot: None,
            fixture: None,
            retired_fixtures: Vec::new(),
            panel: None,
            other: None,
            handle: None,
            #[cfg(all(feature = "image-viewer", feature = "model-viewer"))]
            generation: 0,
            baseline: Vec::new(),
            size: [0; 2],
            entities: Vec::new(),
            gameplay: None,
            restore_at: None,
            saw_focus_loss: false,
            saw_focus_gain: false,
            original_settings: Value::Null,
            #[cfg(feature = "terminal")]
            terminal_ids: Vec::new(),
            #[cfg(feature = "terminal")]
            terminal_panel: None,
            #[cfg(feature = "terminal")]
            terminal_window: None,
            children: Vec::new(),
            #[cfg(all(target_os = "macos", feature = "editor"))]
            keyboard_save: false,
        }
    }

    fn name(&self) -> &str {
        self.options
            .scenarios
            .get(self.scenario)
            .map_or("capture", String::as_str)
    }

    fn next(&mut self, world: &World) {
        self.phase += 1;
        self.phase_frame = presented(world);
        eprintln!("bEd smoke: {} phase {}", self.name(), self.phase);
    }

    fn elapsed_frames(&self, world: &World) -> u64 {
        presented(world).saturating_sub(self.phase_frame)
    }

    fn poll(&mut self, world: &mut World) -> io::Result<()> {
        if let Some(id) = self.inspect
            && let Some(result) = world.resource_mut::<AutomationResults>().take(id)
        {
            self.inspect = None;
            match result.map_err(io::Error::other)? {
                OperationValue::Snapshot(snapshot) => self.snapshot = Some(snapshot),
                other => return Err(io::Error::other(format!("Inspect returned {other:?}"))),
            }
        }
        if self.inspect.is_none() {
            self.inspect = Some(submit(world, Operation::Inspect));
        }
        Ok(())
    }

    fn operation(
        &mut self,
        world: &mut World,
        operation: Operation,
    ) -> io::Result<Option<OperationValue>> {
        let Some(id) = self.operation else {
            self.operation = Some(submit(world, operation));
            return Ok(None);
        };
        self.poll_operation(world, id)
    }

    fn app_operation(
        &mut self,
        world: &mut World,
        action: crate::application::AppAction,
        source: Option<Entity>,
    ) -> io::Result<Option<crate::application::AppRequestOutcome>> {
        let Some(id) = self.application_operation else {
            self.application_operation = Some(crate::automation::submit(world, action, source));
            return Ok(None);
        };
        let Some(result) = crate::automation::poll(world, id) else {
            return Ok(None);
        };
        self.application_operation = None;
        result.map(Some)
    }

    fn poll_operation(
        &mut self,
        world: &mut World,
        id: OperationId,
    ) -> io::Result<Option<OperationValue>> {
        let Some(result) = world.resource_mut::<AutomationResults>().take(id) else {
            return Ok(None);
        };
        self.operation = None;
        result.map(Some).map_err(io::Error::other)
    }

    #[cfg(any(
        all(feature = "image-viewer", feature = "model-viewer"),
        all(target_os = "macos", feature = "editor"),
        feature = "git",
        feature = "terminal"
    ))]
    fn panels(&self, kind: &str) -> Vec<PanelHandle> {
        self.snapshot.as_ref().map_or_else(Vec::new, |s| {
            s.panels
                .iter()
                .filter(|p| p.panel_type == kind)
                .map(|p| p.handle)
                .collect()
        })
    }

    #[cfg(any(
        all(feature = "image-viewer", feature = "model-viewer"),
        feature = "git",
        feature = "terminal"
    ))]
    fn ready(&self, panel: PanelHandle) -> io::Result<bool> {
        let Some(panel) = self
            .snapshot
            .as_ref()
            .and_then(|s| s.panels.iter().find(|p| p.handle == panel))
        else {
            return Ok(false);
        };
        match &panel.readiness {
            Readiness::Ready => Ok(true),
            Readiness::Pending => Ok(false),
            Readiness::Failed(error) => Err(io::Error::other(error.clone())),
        }
    }

    #[cfg(any(
        all(feature = "image-viewer", feature = "model-viewer"),
        all(target_os = "macos", feature = "editor"),
        feature = "git",
        feature = "debugger",
        feature = "terminal"
    ))]
    fn open(&mut self, world: &mut World, path: PathBuf, viewer: Option<&str>) -> io::Result<bool> {
        if let Some(result) = self.app_operation(
            world,
            crate::application::AppAction::OpenFileWithViewer {
                path,
                viewer: viewer.map(String::from),
                additional: false,
                move_existing: false,
            },
            None,
        )? {
            let crate::application::AppRequestOutcome::Opened { panel, .. } = result else {
                return Err(io::Error::other(format!("OpenFile returned {result:?}")));
            };
            self.panel =
                Some(bed_plugin_automation::panel_handle(world, panel).map_err(io::Error::other)?);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn capture(
        &mut self,
        world: &mut World,
        targets: Vec<CaptureTarget>,
    ) -> io::Result<Option<bed_desktop::DesktopCapturedFrame>> {
        match self.operation(
            world,
            Operation::Capture {
                targets,
                after: None,
                timeout: Duration::from_secs(10),
            },
        )? {
            Some(OperationValue::Captured(frame)) => Ok(Some(frame)),
            Some(other) => Err(io::Error::other(format!("Capture returned {other:?}"))),
            None => Ok(None),
        }
    }

    fn complete(&mut self, world: &mut World) {
        eprintln!(
            "bEd: {} passed after {} presented frames",
            self.name(),
            presented(world)
        );
        self.scenario += 1;
        self.phase = 0;
        self.activated = false;
        self.phase_frame = presented(world);
        self.started = Instant::now();
        self.panel = None;
        self.other = None;
        self.handle = None;
        self.operation = None;
        self.application_operation = None;
        if let Some(fixture) = self.fixture.take() {
            self.retired_fixtures.push(fixture);
        }
        self.baseline.clear();
        self.entities.clear();
        if let Some(entity) = self.gameplay.take() {
            let _ = world.try_despawn(entity);
        }
    }
}

fn submit(world: &mut World, operation: Operation) -> OperationId {
    world.resource_mut::<AutomationRequests>().submit(operation)
}

fn presented(world: &World) -> u64 {
    world
        .get_resource::<DesktopRenderStatus>()
        .map_or(0, |status| status.read().completed_frames)
}

fn ensure(condition: bool, message: impl Into<String>) -> io::Result<()> {
    if condition {
        Ok(())
    } else {
        Err(io::Error::other(message.into()))
    }
}

fn advance(world: &mut World) {
    world.resource_scope(|world, mut smoke: Mut<Smoke>| {
        let result = (|| -> io::Result<()> {
            ensure(
                smoke.started.elapsed() < Duration::from_secs(90),
                format!("{} timed out in phase {}", smoke.name(), smoke.phase),
            )?;
            if !world.contains_resource::<DesktopGpu>() {
                return Ok(());
            }
            smoke.poll(world)?;
            #[cfg(feature = "terminal")]
            if smoke.name().contains("terminal") || smoke.name().contains("term-promotion") {
                let events = &world.resource::<bed_desktop::DesktopInjectedInputEvents>().0;
                ensure(
                    events.iter().filter(|event| matches!(&event.event, DesktopInjectedEvent::Text(_) | DesktopInjectedEvent::Key { .. })).all(|event| event.consumed),
                    "focused terminal did not consume its injected text/key batch"
                )?;
            }
            if smoke.snapshot.is_none() {
                return Ok(());
            }
            if !smoke.activated {
                // Native acceptance requires an actual visible, focused OS window.
                crate::app::smoke_focus_window(world);
                smoke.activated = true;
                return Ok(());
            }
            let count = presented(world);
            let waiting_for_children =
                smoke.name() == "--menu-smoke" && (11..=13).contains(&smoke.phase);
            if smoke.presentation_progress.0 != count {
                smoke.presentation_progress = (count, Instant::now());
            } else if !waiting_for_children
                && smoke.presentation_progress.1.elapsed() >= Duration::from_secs(2)
            {
                // Native acceptance requires real presentations. Restore a
                // stalled fixture's visibility, except while a menu parent
                // waits for its child windows to finish their own checks.
                crate::app::smoke_focus_window(world);
                smoke.presentation_progress.1 = Instant::now();
            }
            if count < 2 {
                return Ok(());
            }
            if smoke.scenario >= smoke.options.scenarios.len() {
                if let Some(path) = smoke.options.capture.clone() {
                    if presented(world) < u64::from(smoke.options.after_frames) {
                        return Ok(());
                    }
                    if let Some(frame) = smoke.capture(world, vec![CaptureTarget::Window])? {
                        let image = &frame.images[0].rgba;
                        if path
                            .extension()
                            .is_some_and(|extension| extension.eq_ignore_ascii_case("png"))
                        {
                            image.write_png(&path)?;
                        } else {
                            image.write_ppm(&path)?;
                        }
                        eprintln!(
                            "bEd: completed capture frame {} GPU {} to {}",
                            frame.info.frame.0,
                            frame.info.generation,
                            path.display()
                        );
                        smoke.options.capture = None;
                    } else {
                        return Ok(());
                    }
                }
                world.write_message(AppExit::Success);
                return Ok(());
            }
            let done = match smoke.name() {
                "--smoke-test" | "--main-only-smoke" => basic(world, &mut smoke)?,
                "--effects-smoke" => effects(world, &mut smoke)?,
                "--appearance-smoke" => appearance(world, &mut smoke)?,
                "--lifecycle-smoke" => lifecycle(world, &mut smoke)?,
                "--plugin-smoke" => plugins(world, &mut smoke)?,
                "--menu-smoke" => menu(world, &mut smoke)?,
                "--platform-smoke" => platform(world, &mut smoke)?,
                "--terminal-smoke" | "--term-promotion-smoke" => terminal(world, &mut smoke)?,
                "--git-smoke" => git(world, &mut smoke)?,
                "--debug-smoke" => debug(world, &mut smoke)?,
                name => return Err(io::Error::other(format!("unknown smoke scenario {name}"))),
            };
            if done {
                smoke.complete(world);
            }
            Ok(())
        })();
        if let Err(error) = result {
            eprintln!("bEd smoke failed: {error}");
            if let Some(snapshot) = &smoke.snapshot {
                eprintln!("smoke panel observations: {:?}", snapshot.panels);
            }
            let cameras = world.query::<(Entity, &Camera, &bevy::camera::RenderTarget)>().iter(world)
                .map(|(entity, camera, target)| (entity, camera.is_active, target.clone())).collect::<Vec<_>>();
            eprintln!("smoke cameras: {cameras:?}");
            let status = world.resource::<DesktopRenderStatus>().read();
            let frame = world.resource::<bed_desktop::DesktopFrameInfo>();
            eprintln!("smoke frame observation: UI {} {:?}, presented {}, GPU {}, recovering {}, renderer {:?}", frame.frame.0, frame.physical_size, status.completed_frames, frame.generation, status.recovering, status.error);
            if let Ok(size) = native_window(world, |window| window.inner_size()) {
                let resolution = world.query_filtered::<&Window, With<PrimaryWindow>>().iter(world).next().map(|window| window.resolution.physical_size());
                eprintln!("smoke window observation: native {size:?}, Bevy {resolution:?}");
            }
            #[cfg(feature = "terminal")]
            if smoke.name().contains("terminal") || smoke.name().contains("term-promotion") {
                let terminal = world.non_send::<bed_terminal::bed_terminal::BedTerminal>();
                eprintln!(
                    "smoke terminal observation: sessions {:?}, active {:?}, focused {}",
                    terminal.session_ids(), terminal.active_session_id(), terminal.is_focused()
                );
                if let Some(snapshot) = terminal.active_terminal() {
                    let text = snapshot.lines.iter().map(|row| row.cells.iter()
                        .filter(|cell| cell.width != 0).map(|cell| cell.text.as_ref()).collect::<String>().trim_end().to_string()).collect::<Vec<_>>().join("\n");
                    eprintln!("smoke terminal transcript: {text:?}; images {}", snapshot.images.len());
                }
            }
            *world
                .resource::<SmokeFailure>()
                .0
                .lock()
                .expect("smoke failure mutex poisoned") = Some(error.to_string());
            world.write_message(AppExit::error());
        }
    });
}

fn basic(world: &mut World, smoke: &mut Smoke) -> io::Result<bool> {
    if let Some(frame) = smoke.capture(world, vec![CaptureTarget::Window])? {
        ensure(
            visible_pixels(&frame.images[0].rgba.pixels) > 0,
            "desktop capture is blank",
        )?;
        return Ok(true);
    }
    Ok(false)
}

fn setting(world: &mut World, smoke: &mut Smoke, key: &str, value: Value) -> io::Result<bool> {
    Ok(smoke
        .app_operation(
            world,
            crate::application::AppAction::Settings(bed_api::settings::SettingsRequest::new(
                crate::config::settings_owner(key),
                key,
                value,
            )),
            None,
        )?
        .is_some())
}

fn effects(world: &mut World, smoke: &mut Smoke) -> io::Result<bool> {
    #[cfg(feature = "effects")]
    {
        if smoke.phase == 0 {
            if setting(world, smoke, "effect_preset", json!("sharp"))? {
                smoke.next(world);
            }
            return Ok(false);
        }
        basic(world, smoke)
    }
    #[cfg(not(feature = "effects"))]
    {
        let _ = (world, smoke);
        Err(io::Error::other("effects smoke requires effects feature"))
    }
}

fn appearance(world: &mut World, smoke: &mut Smoke) -> io::Result<bool> {
    if smoke.phase == 0 {
        smoke.original_settings = world.non_send::<crate::config::Settings>().settings.clone();
        smoke.next(world);
    }
    let changes = [
        ("font", json!("Paper Mono")),
        ("fontSize", json!(24)),
        ("theme", json!("solarized-light")),
        ("treesitter", json!(true)),
    ];
    if (1..=4).contains(&smoke.phase) {
        let (key, value) = changes[(smoke.phase - 1) as usize].clone();
        if setting(world, smoke, key, value)? {
            smoke.next(world);
        }
        return Ok(false);
    }
    if smoke.phase == 5 {
        if smoke.elapsed_frames(world) >= 2
            && smoke.capture(world, vec![CaptureTarget::Window])?.is_some()
        {
            smoke.next(world);
        }
        return Ok(false);
    }
    let entries = smoke
        .original_settings
        .as_object()
        .ok_or_else(|| io::Error::other("settings fixture is not an object"))?
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect::<Vec<_>>();
    let index = (smoke.phase - 6) as usize;
    if let Some((key, value)) = entries.get(index) {
        if setting(world, smoke, key, value.clone())? {
            smoke.next(world);
        }
        Ok(false)
    } else {
        ensure(
            world.non_send::<crate::config::Settings>().settings == smoke.original_settings,
            "appearance smoke did not restore preferences",
        )?;
        Ok(smoke.elapsed_frames(world) >= 2)
    }
}

fn native_window<T>(
    world: &mut World,
    callback: impl FnOnce(&winit::window::Window) -> T,
) -> io::Result<T> {
    let primary = world
        .query_filtered::<Entity, With<PrimaryWindow>>()
        .iter(world)
        .next()
        .ok_or_else(|| io::Error::other("primary window missing"))?;
    WINIT_WINDOWS.with(|windows| {
        let windows = windows.borrow();
        let window = windows
            .get_window(primary)
            .ok_or_else(|| io::Error::other("native window missing"))?;
        Ok(callback(window))
    })
}

fn lifecycle(world: &mut World, smoke: &mut Smoke) -> io::Result<bool> {
    for event in &world.resource::<bed_desktop::DesktopInputEvents>().0 {
        if let winit::event::WindowEvent::Focused(focused) = event.event.event {
            smoke.saw_focus_loss |= !focused;
            smoke.saw_focus_gain |= focused;
        }
    }
    match smoke.phase {
        0 => {
            smoke.size = native_window(world, |w| {
                w.focus_window();
                let target =
                    winit::dpi::LogicalSize::new(840., 600.).to_physical::<u32>(w.scale_factor());
                let _ = w.request_inner_size(target);
                [target.width, target.height]
            })?;
            smoke.next(world);
        }
        1 => {
            if native_window(world, |w| {
                [w.inner_size().width, w.inner_size().height] == smoke.size && w.has_focus()
            })? && smoke.elapsed_frames(world) >= 2
            {
                native_window(world, |w| w.set_minimized(true))?;
                smoke.restore_at = Some(Instant::now() + Duration::from_millis(700));
                smoke.saw_focus_gain = false;
                smoke.next(world);
            }
        }
        2 => {
            if Instant::now() >= smoke.restore_at.unwrap() {
                ensure(
                    native_window(world, |w| w.is_minimized() == Some(true))?,
                    "native minimize not observed",
                )?;
                ensure(smoke.saw_focus_loss, "minimize did not report focus loss")?;
                native_window(world, |w| {
                    w.set_minimized(false);
                    w.focus_window();
                })?;
                smoke.next(world);
            }
        }
        3 => {
            if !native_window(world, |w| w.has_focus())? {
                native_window(world, |w| w.focus_window())?;
            }
            if native_window(world, |w| w.is_minimized() == Some(false) && w.has_focus())?
                && smoke.saw_focus_gain
                && smoke.elapsed_frames(world) >= 2
            {
                return basic(world, smoke);
            }
        }
        _ => unreachable!(),
    }
    Ok(false)
}

#[cfg(any(
    all(feature = "image-viewer", feature = "model-viewer"),
    all(target_os = "macos", feature = "editor"),
    feature = "git",
    feature = "debugger"
))]
fn fixture_workspace(world: &mut World, smoke: &mut Smoke, name: &str) -> io::Result<bool> {
    if smoke.fixture.is_none() {
        smoke.fixture = Some(Fixture::new(name)?);
    }
    let root = smoke.fixture.as_ref().unwrap().0.clone();
    Ok(smoke
        .app_operation(
            world,
            crate::application::AppAction::HostAction(
                bed_api::features::HostAction::OpenDirectory {
                    location: bed_api::files::FileLocation {
                        provider: bed_api::files::FileProviderId::Local,
                        path: root,
                    },
                },
            ),
            None,
        )?
        .is_some())
}

#[cfg(all(feature = "image-viewer", feature = "model-viewer"))]
fn canvas(
    world: &World,
    panel: PanelHandle,
) -> io::Result<(TextureHandle, bed_ui::gpu::RenderTarget)> {
    let entity = panel_presentation(world, panel.entity())
        .ok_or_else(|| io::Error::other("panel presentation missing"))?;
    let handle = world
        .get::<CanvasOutput>(entity)
        .ok_or_else(|| io::Error::other("panel canvas output missing"))?
        .0
        .handle;
    let target = world
        .resource::<DesktopCanvasTargets>()
        .target(handle)
        .ok_or_else(|| io::Error::other("panel target missing"))?
        .clone();
    Ok((handle, target))
}

fn plugins(world: &mut World, smoke: &mut Smoke) -> io::Result<bool> {
    #[cfg(all(feature = "image-viewer", feature = "model-viewer"))]
    {
        match smoke.phase {
            0 => {
                if fixture_workspace(world, smoke, "plugins")? {
                    let f = smoke.fixture.as_ref().unwrap();
                    f.write(
                        "image.png",
                        include_bytes!("../assets/bEd-iOS-Default-1024@1x.png"),
                    )?;
                    f.write("bytes.bin", [0, 255, 13, 10, 239, 187, 191, 128, 0, 1])?;
                    f.write(
                        "cube.glb",
                        include_bytes!("../tests/fixtures/gltf/cube.glb"),
                    )?;
                    f.write(
                        "cube.gltf",
                        include_bytes!("../tests/fixtures/gltf/cube.gltf"),
                    )?;
                    smoke.gameplay = Some(world.spawn(SmokeGameplay(42)).id());
                    smoke.next(world);
                }
            }
            1 => {
                if smoke.open(
                    world,
                    smoke.fixture.as_ref().unwrap().0.join("image.png"),
                    None,
                )? {
                    smoke.next(world);
                }
            }
            2 => {
                let panel = smoke.panel.unwrap();
                if smoke.ready(panel)? && smoke.elapsed_frames(world) >= 2 {
                    let (handle, _) = canvas(world, panel)?;
                    smoke.handle = Some(handle);
                    if smoke
                        .capture(
                            world,
                            vec![CaptureTarget::Window, CaptureTarget::Panel(panel)],
                        )?
                        .is_some()
                    {
                        smoke.other = Some(panel);
                        smoke.next(world);
                    }
                }
            }
            3 => {
                if smoke.open(
                    world,
                    smoke.fixture.as_ref().unwrap().0.join("bytes.bin"),
                    None,
                )? {
                    smoke.next(world);
                }
            }
            4 => {
                ensure(
                    smoke.panels("bed.editor.hex-panel").len() == 1,
                    "binary not routed to hex editor",
                )?;
                if smoke
                    .operation(world, Operation::FocusPanel(smoke.other.unwrap()))?
                    .is_some()
                {
                    smoke.next(world);
                }
            }
            5 => {
                if smoke.elapsed_frames(world) >= 2
                    && smoke
                        .operation(world, Operation::ClosePanel(smoke.other.unwrap()))?
                        .is_some()
                {
                    smoke.next(world);
                }
            }
            6 => {
                if (smoke.operation.is_some()
                    || (smoke.panels("bed.image.panel").is_empty()
                        && world
                            .resource::<DesktopCanvasTargets>()
                            .target(smoke.handle.unwrap())
                            .is_none()))
                    && smoke.open(
                        world,
                        smoke.fixture.as_ref().unwrap().0.join("image.png"),
                        None,
                    )?
                {
                    smoke.next(world);
                }
            }
            7 => {
                if smoke.ready(smoke.panel.unwrap())? && smoke.elapsed_frames(world) >= 2 {
                    let (handle, _) = canvas(world, smoke.panel.unwrap())?;
                    ensure(
                        Some(handle) != smoke.handle,
                        "reopened canvas reused retired handle",
                    )?;
                    smoke.handle = Some(handle);
                    smoke.generation = world.resource::<DesktopGpu>().generation;
                    destroy_device(world)?;
                    smoke.next(world);
                }
            }
            8 => {
                if world.resource::<DesktopGpu>().generation != smoke.generation
                    && smoke.elapsed_frames(world) >= 4
                    && smoke.ready(smoke.panel.unwrap())?
                {
                    ensure(
                        canvas(world, smoke.panel.unwrap())?.0 == smoke.handle.unwrap(),
                        "image handle changed after replacement",
                    )?;
                    if smoke
                        .capture(world, vec![CaptureTarget::Panel(smoke.panel.unwrap())])?
                        .is_some()
                    {
                        smoke.next(world);
                    }
                }
            }
            9 => {
                if smoke
                    .operation(world, Operation::ClosePanel(smoke.panel.unwrap()))?
                    .is_some()
                {
                    smoke.next(world);
                }
            }
            10 => {
                if let Some(id) = smoke.operation {
                    // Once submitted, consume the close receipt even after the tab is gone.
                    if smoke.poll_operation(world, id)?.is_some() {
                        smoke.next(world);
                    }
                } else if let Some(panel) = smoke.panels("bed.editor.hex-panel").first().copied() {
                    if smoke
                        .operation(world, Operation::ClosePanel(panel))?
                        .is_some()
                    {
                        smoke.next(world);
                    }
                } else {
                    smoke.next(world);
                }
            }
            11 => {
                ensure(
                    std::fs::read(smoke.fixture.as_ref().unwrap().0.join("bytes.bin"))?
                        == [0, 255, 13, 10, 239, 187, 191, 128, 0, 1],
                    "binary view modified disk bytes",
                )?;
                if smoke.open(
                    world,
                    smoke.fixture.as_ref().unwrap().0.join("cube.glb"),
                    None,
                )? {
                    smoke.next(world);
                }
            }
            12 => {
                if smoke.ready(smoke.panel.unwrap())? && smoke.elapsed_frames(world) >= 20 {
                    let (handle, target) = canvas(world, smoke.panel.unwrap())?;
                    ensure(target.depth.is_some(), "model canvas lacks depth")?;
                    smoke.handle = Some(handle);
                    smoke.size = target.size;
                    native_window(world, |w| {
                        let _ = w.request_inner_size(winit::dpi::PhysicalSize::new(1800, 1200));
                    })?;
                    smoke.next(world);
                }
            }
            13 => {
                if smoke.ready(smoke.panel.unwrap())? && smoke.elapsed_frames(world) >= 20 {
                    let (_, target) = canvas(world, smoke.panel.unwrap())?;
                    ensure(target.size != smoke.size, "model output did not resize")?;
                    smoke.size = target.size;
                    smoke.baseline = read_target(world, &target)?;
                    ensure(
                        visible_pixels(&smoke.baseline) > 1000,
                        "model render is blank",
                    )?;
                    smoke.entities = world
                        .query_filtered::<Entity, Or<(
                            With<bed_desktop::preview::PreviewView>,
                            With<crate::application::PanelPresentations>,
                        )>>()
                        .iter(world)
                        .collect();
                    ensure(
                        world
                            .query::<(&bed_desktop::preview::PreviewView, &bevy::mesh::Mesh3d)>()
                            .iter(world)
                            .any(|(view, _)| view.0 == smoke.handle.unwrap().0),
                        "model mesh missing",
                    )?;
                    smoke.generation = world.resource::<DesktopGpu>().generation;
                    destroy_device(world)?;
                    smoke.next(world);
                }
            }
            14 => {
                if world.resource::<DesktopGpu>().generation != smoke.generation
                    && smoke.ready(smoke.panel.unwrap())?
                    && smoke.elapsed_frames(world) >= 20
                {
                    let (handle, target) = canvas(world, smoke.panel.unwrap())?;
                    ensure(
                        handle == smoke.handle.unwrap()
                            && target.size == smoke.size
                            && target.depth.is_some(),
                        "model output identity/extent/depth changed during recovery",
                    )?;
                    ensure(
                        smoke.entities.iter().all(|e| world.entities().contains(*e))
                            && world
                                .get::<SmokeGameplay>(smoke.gameplay.unwrap())
                                .is_some_and(|v| v.0 == 42),
                        "device recovery replaced live editor/game entities",
                    )?;
                    compare_recovered_pixels(&smoke.baseline, &read_target(world, &target)?)?;
                    smoke.next(world);
                }
            }
            15 => {
                if smoke
                    .operation(world, Operation::ClosePanel(smoke.panel.unwrap()))?
                    .is_some()
                {
                    smoke.next(world);
                }
            }
            16 => {
                if world
                    .resource::<DesktopCanvasTargets>()
                    .target(smoke.handle.unwrap())
                    .is_none()
                    && smoke.open(
                        world,
                        smoke.fixture.as_ref().unwrap().0.join("cube.gltf"),
                        None,
                    )?
                {
                    smoke.next(world);
                }
            }
            17 => {
                if smoke
                    .app_operation(
                        world,
                        crate::application::AppAction::SwitchViewer {
                            panel: bed_plugin_automation::resolve_panel(
                                world,
                                smoke.panel.unwrap(),
                            )
                            .map_err(io::Error::other)?,
                            viewer: "bed.gltf.viewer".into(),
                        },
                        Some(smoke.panel.unwrap().entity()),
                    )?
                    .is_some()
                {
                    smoke.next(world);
                }
            }
            18 => {
                if smoke.ready(smoke.panel.unwrap())? && smoke.elapsed_frames(world) >= 20 {
                    let (handle, _) = canvas(world, smoke.panel.unwrap())?;
                    smoke.handle = Some(handle);
                    if smoke
                        .capture(world, vec![CaptureTarget::Panel(smoke.panel.unwrap())])?
                        .is_some()
                    {
                        smoke.next(world);
                    }
                }
            }
            19 => {
                if smoke
                    .operation(world, Operation::ClosePanel(smoke.panel.unwrap()))?
                    .is_some()
                {
                    smoke.next(world);
                }
            }
            20 => {
                return Ok(smoke.panels("bed.gltf.panel").is_empty()
                    && world
                        .resource::<DesktopCanvasTargets>()
                        .target(smoke.handle.unwrap())
                        .is_none());
            }
            _ => unreachable!(),
        }
        Ok(false)
    }
    #[cfg(not(all(feature = "image-viewer", feature = "model-viewer")))]
    {
        let _ = (world, smoke);
        Err(io::Error::other("plugin smoke requires images/models"))
    }
}

#[cfg(all(feature = "image-viewer", feature = "model-viewer"))]
fn destroy_device(world: &World) -> io::Result<()> {
    let device = &world.resource::<DesktopGpu>().device;
    // The desktop renders on this thread. Complete submitted work and deliver
    // its readback callbacks before deliberately destroying the device: an
    // already successful map must be consumed while its buffer is still live.
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(Duration::from_secs(10)),
        })
        .map_err(io::Error::other)?;
    device.destroy();
    let _ = device.poll(wgpu::PollType::Poll);
    Ok(())
}

fn visible_pixels(pixels: &[u8]) -> usize {
    pixels
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|p| p[..3].iter().any(|v| *v > 20))
        .count()
}

#[cfg(any(test, all(feature = "image-viewer", feature = "model-viewer")))]
fn compare_recovered_pixels(before: &[u8], after: &[u8]) -> io::Result<()> {
    ensure(
        before.len() == after.len(),
        "recovered model pixel extent differs",
    )?;
    let count = visible_pixels(before);
    ensure(
        count > 1000 && visible_pixels(after) >= count * 3 / 4,
        "recovered model is blank",
    )?;
    let difference = before
        .as_chunks::<4>()
        .0
        .iter()
        .zip(after.as_chunks::<4>().0.iter())
        .filter(|(p, _)| p[..3].iter().any(|v| *v > 20))
        .map(|(a, b)| {
            a[..3]
                .iter()
                .zip(&b[..3])
                .map(|(a, b)| u64::from(a.abs_diff(*b)))
                .sum::<u64>()
        })
        .sum::<u64>();
    ensure(
        difference <= count as u64 * 3 * 8,
        format!("recovered model differs from baseline ({difference} across {count} pixels)"),
    )
}

#[cfg(all(feature = "image-viewer", feature = "model-viewer"))]
fn read_target(world: &World, target: &bed_ui::gpu::RenderTarget) -> io::Result<Vec<u8>> {
    let gpu = world.resource::<DesktopGpu>();
    let row = target.size[0] * 4;
    let stride = row.div_ceil(256) * 256;
    let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("native smoke readback"),
        size: u64::from(stride) * u64::from(target.size[1]),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    encoder.copy_texture_to_buffer(
        target.texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(stride),
                rows_per_image: Some(target.size[1]),
            },
        },
        wgpu::Extent3d {
            width: target.size[0],
            height: target.size[1],
            depth_or_array_layers: 1,
        },
    );
    gpu.queue.submit([encoder.finish()]);
    let (tx, rx) = std::sync::mpsc::channel();
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
    gpu.device
        .poll(wgpu::PollType::Wait {
            timeout: Some(Duration::from_secs(10)),
            submission_index: None,
        })
        .map_err(io::Error::other)?;
    rx.recv_timeout(Duration::from_secs(10))
        .map_err(io::Error::other)?
        .map_err(io::Error::other)?;
    let data = buffer
        .slice(..)
        .get_mapped_range()
        .map_err(io::Error::other)?;
    let pixels = data
        .chunks_exact(stride as usize)
        .flat_map(|row_bytes| row_bytes[..row as usize].iter().copied())
        .collect();
    drop(data);
    buffer.unmap();
    Ok(pixels)
}

#[cfg(any(
    all(target_os = "macos", feature = "editor"),
    feature = "git",
    feature = "terminal"
))]
fn binding(world: &World, panel: PanelHandle) -> io::Result<PanelBinding> {
    let entity = panel_presentation(world, panel.entity())
        .ok_or_else(|| io::Error::other("presentation missing"))?;
    world
        .get::<PanelBinding>(entity)
        .cloned()
        .ok_or_else(|| io::Error::other("editor binding missing"))
}

#[cfg(all(target_os = "macos", feature = "editor"))]
fn document_bytes(world: &World, panel: PanelHandle) -> io::Result<(Vec<u8>, bool, String)> {
    let document = binding(world, panel)?
        .document
        .ok_or_else(|| io::Error::other("document binding missing"))?;
    let snapshot = world
        .non_send::<bed_documents::DocumentSession>()
        .snapshot(document)?;
    Ok((snapshot.bytes.to_vec(), snapshot.dirty, snapshot.path))
}

#[cfg(all(target_os = "macos", feature = "editor"))]
fn editor_focused(world: &mut World, panel: PanelHandle) -> io::Result<bool> {
    let binding = binding(world, panel)?;
    let Some(view) = binding.view else {
        return Ok(false);
    };
    let unblocked = world
        .non_send_mut::<bed_documents::DocumentSession>()
        .with_view(view, |editor| !editor.view.block_input)?;
    let focused = world
        .non_send::<DesktopContext>()
        .context
        .binding()
        .with_bound_context(|| unsafe {
            let native = &*dear_imgui_rs::sys::igGetCurrentContext();
            let pending = (0..native.InputEventsQueue.Size as usize).any(|index| {
                (*native.InputEventsQueue.Data.add(index)).Type
                    == dear_imgui_rs::sys::ImGuiInputEventType_Focus
            });
            native.NavWindow.as_ref().is_some_and(|window| {
                let name = std::ffi::CStr::from_ptr(window.Name).to_string_lossy();
                name.contains(&format!("##bed_view_{}", view.0))
                    && name.contains("##editor")
                    && window.Active
                    && !window.Hidden
            }) && !native.IO.AppFocusLost
                && !pending
        });
    Ok(unblocked && focused && native_window(world, |window| window.has_focus())?)
}

#[cfg(all(target_os = "macos", feature = "editor"))]
fn menu(world: &mut World, smoke: &mut Smoke) -> io::Result<bool> {
    use crate::native_commands::MenuAction;
    let observations =
        std::mem::take(&mut world.resource_mut::<crate::app::NativeMenuObservations>().0);
    smoke.keyboard_save |= observations
        .iter()
        .any(|dispatch| dispatch.action == MenuAction::Save && dispatch.keyboard);
    smoke
        .children
        .append(&mut world.resource_mut::<crate::app::NativeSpawnedInstances>().0);
    match smoke.phase {
        0 => {
            if fixture_workspace(world, smoke, "menu")? {
                smoke
                    .fixture
                    .as_ref()
                    .unwrap()
                    .write("document.txt", b"native menu baseline\n")?;
                smoke.next(world);
            }
        }
        1 => {
            if smoke.open(
                world,
                smoke.fixture.as_ref().unwrap().0.join("document.txt"),
                None,
            )? {
                smoke.next(world);
            }
        }
        2 => {
            if setting(world, smoke, "autosave", json!(true))? {
                smoke.next(world);
            }
        }
        3 => {
            if setting(world, smoke, "autosave_delay_ms", json!(100))? {
                smoke.next(world);
            }
        }
        4 => {
            if smoke
                .operation(world, Operation::FocusPanel(smoke.panel.unwrap()))?
                .is_some()
            {
                crate::app::smoke_focus_window(world);
                smoke.next(world);
            }
        }
        5 => {
            if smoke.operation.is_none() && !native_window(world, |window| window.has_focus())? {
                crate::app::smoke_focus_window(world);
                return Ok(false);
            }
            if smoke.operation.is_none() && !editor_focused(world, smoke.panel.unwrap())? {
                crate::application::focus_panel(world, smoke.panel.unwrap().entity(), true)?;
                return Ok(false);
            }
            if (smoke.operation.is_some() || editor_focused(world, smoke.panel.unwrap())?)
                && smoke
                    .operation(
                        world,
                        Operation::Input(vec![DesktopInputStep::from(vec![
                            DesktopInjectedEvent::Text("NATIVE_EDIT_".into()),
                        ])]),
                    )?
                    .is_some()
            {
                smoke.next(world);
            }
        }
        6 => {
            let (bytes, dirty, path) = document_bytes(world, smoke.panel.unwrap())?;
            if bytes == b"NATIVE_EDIT_native menu baseline\n"
                && !dirty
                && std::fs::read(path)? == bytes
            {
                crate::app::smoke_native_menu(world, MenuAction::Undo)?;
                smoke.next(world);
            }
        }
        7 => {
            if smoke.elapsed_frames(world) >= 2 {
                let (bytes, dirty, path) = document_bytes(world, smoke.panel.unwrap())?;
                if bytes == b"native menu baseline\n" && !dirty && std::fs::read(path)? == bytes {
                    crate::app::smoke_native_menu(world, MenuAction::Redo)?;
                    smoke.next(world);
                }
            }
        }
        8 => {
            if smoke.elapsed_frames(world) >= 2 {
                let (bytes, dirty, path) = document_bytes(world, smoke.panel.unwrap())?;
                if bytes == b"NATIVE_EDIT_native menu baseline\n"
                    && !dirty
                    && std::fs::read(path)? == bytes
                {
                    crate::app::smoke_native_menu(world, MenuAction::Save)?;
                    crate::app::smoke_focus_window(world);
                    smoke.next(world);
                }
            }
        }
        9 => {
            if !native_window(world, |window| window.has_focus())? {
                crate::app::smoke_focus_window(world);
            } else if smoke.elapsed_frames(world) >= 2 {
                crate::app::smoke_native_key_equivalent(world, "s", 1, false)?;
                smoke.next(world);
            }
        }
        10 => {
            if smoke.keyboard_save && smoke.elapsed_frames(world) >= 2 {
                let (bytes, _, path) = document_bytes(world, smoke.panel.unwrap())?;
                let saved = std::fs::read(path)?;
                let focused = editor_focused(world, smoke.panel.unwrap())?;
                ensure(
                    bytes == b"NATIVE_EDIT_native menu baseline\n" && saved == bytes && focused,
                    format!(
                        "native Cmd+S lost content, save or cursor focus: buffer {bytes:?}, saved {saved:?}, editor focused {focused}"
                    ),
                )?;
                crate::app::smoke_native_key_equivalent(world, "n", 45, true)?;
                smoke.next(world);
            }
        }
        11 => {
            if smoke.children.len() == 1 {
                crate::app::smoke_native_menu(world, MenuAction::NewWindow)?;
                smoke.next(world);
            }
        }
        12 => {
            if smoke.children.len() == 2 {
                crate::app::smoke_native_dock_new_window(world)?;
                smoke.next(world);
            }
        }
        13 => {
            if smoke.children.len() >= 3 {
                ensure(
                    smoke.children.len() == 3,
                    "native window actions launched duplicate instances",
                )?;
                let mut done = true;
                for child in &mut smoke.children {
                    match child.try_wait()? {
                        Some(status) => {
                            ensure(status.success(), format!("new instance failed: {status}"))?
                        }
                        None => done = false,
                    }
                }
                if done {
                    // Directory-only fixtures retain Startup. Select a distinct
                    // current slot before verifying both global template commands.
                    crate::app::smoke_native_menu(
                        world,
                        MenuAction::UseLayout(crate::config::LayoutSlot::Layout1),
                    )?;
                    crate::application::process_app_requests(world)?;
                    crate::app::smoke_native_menu(world, MenuAction::SetCurrentLayout)?;
                    crate::app::smoke_native_menu(
                        world,
                        MenuAction::SetLayoutDefault(crate::config::LayoutSlot::Startup),
                    )?;
                    smoke.next(world);
                }
            }
        }
        14 => {
            if smoke.elapsed_frames(world) >= 2 {
                let store = &world
                    .non_send::<crate::application::WorkspacePersistence>()
                    .store;
                let default = store
                    .layout_default(crate::config::LayoutSlot::Layout1)
                    .ok_or_else(|| io::Error::other("missing saved layout template"))?;
                ensure(
                    store.layout_default(crate::config::LayoutSlot::Startup)
                        == Some(default.clone()),
                    "Set Other Layout did not save the Startup template",
                )?;
                ensure(
                    default["version"] == 1
                        && default["panels"].as_array().is_some_and(|panels| {
                            !panels.is_empty()
                                && panels.iter().all(|p| {
                                    p.get("state").is_none() && p.get("document").is_none()
                                })
                        }),
                    "Save Default Layout did not persist a fresh arrangement",
                )?;
                crate::app::smoke_native_menu(
                    world,
                    MenuAction::RestoreBuiltinLayout(crate::config::LayoutSlot::Layout1),
                )?;
                crate::app::smoke_native_menu(
                    world,
                    MenuAction::RestoreBuiltinLayout(crate::config::LayoutSlot::Startup),
                )?;
                smoke.next(world);
            }
        }
        15 => {
            if smoke.elapsed_frames(world) >= 2 {
                ensure(
                    world
                        .non_send::<crate::application::WorkspacePersistence>()
                        .store
                        .layout_default(crate::config::LayoutSlot::Layout1)
                        .is_none()
                        && world
                            .non_send::<crate::application::WorkspacePersistence>()
                            .store
                            .layout_default(crate::config::LayoutSlot::Startup)
                            .is_none(),
                    "Reset Default Layout did not clear template",
                )?;
                ensure(
                    document_bytes(world, smoke.panel.unwrap())?.0
                        == b"NATIVE_EDIT_native menu baseline\n",
                    "default layout altered document",
                )?;
                crate::app::smoke_focus_window(world);
                smoke.next(world);
            }
        }
        16 => {
            if !native_window(world, |window| window.has_focus())? {
                crate::app::smoke_focus_window(world);
            } else {
                crate::app::smoke_native_key_equivalent(world, "3", 20, false)?;
                smoke.next(world);
            }
        }
        17 => {
            if smoke.elapsed_frames(world) >= 8 {
                let panels = crate::application::active_panel_entities(world);
                ensure(
                    world.resource::<crate::application::LayoutSlots>().active()
                        == crate::config::LayoutSlot::Layout3
                        && panels.len() == 4,
                    "terminal layout did not create four panels",
                )?;
                let areas = panels
                    .iter()
                    .map(|panel| {
                        let id = world
                            .get::<bed_workbench::PanelPresentation>(*panel)
                            .unwrap()
                            .id;
                        world
                            .resource::<bed_workbench::WorkbenchLayout>()
                            .tiling
                            .area_for(id)
                    })
                    .collect::<HashSet<_>>();
                ensure(
                    areas.len() == 4 && !areas.contains(&None),
                    "terminal grid does not have four distinct areas",
                )?;
                for panel in &panels {
                    ensure(
                        world
                            .get::<PanelBinding>(*panel)
                            .is_some_and(|binding| binding.terminal.is_some()),
                        "terminal grid has an unavailable terminal",
                    )?;
                    ensure(
                        world
                            .get::<bed_workbench::api::PanelGeometry>(*panel)
                            .is_some_and(|geometry| {
                                geometry.visible
                                    && geometry.content_max[0] > geometry.content_min[0]
                                    && geometry.content_max[1] > geometry.content_min[1]
                            }),
                        "terminal grid panel has no visible content",
                    )?;
                }
                smoke.entities = panels;
                crate::app::smoke_native_menu(world, MenuAction::ResetLayout)?;
                smoke.next(world);
            }
        }
        18 => {
            if smoke.elapsed_frames(world) >= 3 {
                ensure(
                    crate::application::active_panel_entities(world) == smoke.entities,
                    "reset replaced live terminals",
                )?;
                crate::app::smoke_native_menu(
                    world,
                    MenuAction::UseLayout(crate::config::LayoutSlot::Layout1),
                )?;
                smoke.next(world);
            }
        }
        19 => {
            if smoke.elapsed_frames(world) >= 3 {
                ensure(
                    world.resource::<crate::application::LayoutSlots>().active()
                        == crate::config::LayoutSlot::Layout1,
                    "layout 1 was not restored",
                )?;
                ensure(
                    document_bytes(world, smoke.panel.unwrap())?.0
                        == b"NATIVE_EDIT_native menu baseline\n",
                    "slot switching changed the document",
                )?;
                crate::app::smoke_native_menu(
                    world,
                    MenuAction::UseLayout(crate::config::LayoutSlot::Layout3),
                )?;
                smoke.next(world);
            }
        }
        20 => {
            if smoke.elapsed_frames(world) >= 8 {
                ensure(
                    crate::application::active_panel_entities(world) == smoke.entities,
                    "switching recreated terminal sessions",
                )?;
                crate::app::smoke_native_menu(
                    world,
                    MenuAction::UseLayout(crate::config::LayoutSlot::Startup),
                )?;
                smoke.next(world);
            }
        }
        21 => {
            if smoke.elapsed_frames(world) >= 3 {
                ensure(
                    world.resource::<crate::application::LayoutSlots>().active()
                        == crate::config::LayoutSlot::Startup,
                    "Startup layout was not selected",
                )?;
                let panels = crate::application::active_panel_entities(world);
                ensure(
                    panels.len() == 1
                        && world
                            .get::<bed_workbench::PanelRecord>(panels[0])
                            .is_some_and(|panel| panel.panel_type == "bed.workspaces.panel"),
                    "Startup did not restore the built-in Workspaces panel",
                )?;
                crate::app::smoke_native_menu(
                    world,
                    MenuAction::UseLayout(crate::config::LayoutSlot::Layout3),
                )?;
                smoke.next(world);
            }
        }
        22 => {
            if smoke.elapsed_frames(world) >= 8 {
                ensure(
                    crate::application::active_panel_entities(world) == smoke.entities,
                    "Startup switching recreated terminal sessions",
                )?;
                return basic(world, smoke);
            }
        }
        _ => unreachable!(),
    }
    Ok(false)
}

#[cfg(not(all(target_os = "macos", feature = "editor")))]
fn menu(_: &mut World, _: &mut Smoke) -> io::Result<bool> {
    Err(io::Error::other("menu smoke requires macOS/editor"))
}

#[cfg(all(target_os = "macos", feature = "editor"))]
fn platform(world: &mut World, smoke: &mut Smoke) -> io::Result<bool> {
    use crate::native_commands::MenuAction;
    crate::app::smoke_native_chrome(world)?;
    const KINDS: [&str; 6] = [
        "bed.explorer.panel",
        "bed.terminal.panel",
        "bed.settings.panel",
        "bed.search.panel",
        "bed.editor.diagnostics-panel",
        "bed.structure.panel",
    ];
    const COMMANDS: [&str; 6] = [
        "bed.explorer.new",
        "bed.terminal.new",
        "bed.settings.new",
        "bed.search.new",
        bed_plugin_editor::NEW_DIAGNOSTICS_COMMAND,
        "bed.structure.open",
    ];
    match smoke.phase {
        0 => {
            if fixture_workspace(world, smoke, "platform")? {
                smoke
                    .fixture
                    .as_ref()
                    .unwrap()
                    .write("document.txt", b"platform fixture\n")?;
                smoke.next(world);
            }
        }
        1 => {
            if smoke.open(
                world,
                smoke.fixture.as_ref().unwrap().0.join("document.txt"),
                None,
            )? {
                smoke.original_settings =
                    world.non_send::<crate::config::Settings>().settings.clone();
                smoke.next(world);
            }
        }
        2 => {
            if smoke.elapsed_frames(world) >= 2 {
                crate::app::smoke_native_menu(world, MenuAction::Find)?;
                smoke.next(world);
            }
        }
        3 => {
            let entity = panel_presentation(world, smoke.panel.unwrap().entity()).unwrap();
            if world
                .non_send::<bed_plugin_editor::EditorViews>()
                .text(entity)
                .is_some_and(|view| view.find_visible())
            {
                smoke.entities = smoke
                    .snapshot
                    .as_ref()
                    .unwrap()
                    .panels
                    .iter()
                    .map(|p| p.handle.entity())
                    .collect();
                for _ in 0..2 {
                    for command in COMMANDS {
                        ensure(
                            crate::app::smoke_native_toolbar(world, command),
                            format!("native toolbar {command} unavailable"),
                        )?;
                    }
                }
                smoke.next(world);
            }
        }
        4 => {
            if setting(world, smoke, "background_opacity", json!(0.35))? {
                smoke.next(world);
            }
        }
        5 => {
            if setting(world, smoke, "mac_blur_enabled", json!(false))? {
                smoke.next(world);
            }
        }
        6 => {
            if smoke.elapsed_frames(world) >= 2 {
                let (visible, opaque, opacity) = crate::app::smoke_native_material(world);
                ensure(
                    !visible
                        && opaque == Some(false)
                        && opacity.is_some_and(|v| (v - 1.).abs() < 0.001),
                    "native opacity/blur reload failed",
                )?;
                for kind in KINDS {
                    let count = smoke
                        .snapshot
                        .as_ref()
                        .unwrap()
                        .panels
                        .iter()
                        .filter(|p| {
                            p.panel_type == kind && !smoke.entities.contains(&p.handle.entity())
                        })
                        .count();
                    ensure(
                        count == 2,
                        format!("native toolbar {kind} created {count}, expected 2"),
                    )?;
                }
                smoke.next(world);
            }
        }
        7 => {
            if setting(
                world,
                smoke,
                "background_opacity",
                smoke.original_settings["background_opacity"].clone(),
            )? {
                smoke.next(world);
            }
        }
        8 => {
            if setting(
                world,
                smoke,
                "mac_blur_enabled",
                smoke.original_settings["mac_blur_enabled"].clone(),
            )? {
                for action in [
                    MenuAction::NewExplorer,
                    MenuAction::NewTerminal,
                    MenuAction::NewSettings,
                    MenuAction::NewContentSearch,
                    MenuAction::NewDiagnostics,
                ] {
                    crate::app::smoke_native_menu(world, action)?;
                }
                world
                    .non_send::<bed_desktop::DesktopNativePlatform>()
                    .menu
                    .as_ref()
                    .ok_or_else(|| io::Error::other("native menu is not initialized"))?
                    .perform_plugin_for_smoke("bed.structure.open")?;
                smoke.next(world);
            }
        }
        9 => {
            if smoke.elapsed_frames(world) >= 2 {
                for kind in KINDS {
                    let count = smoke
                        .snapshot
                        .as_ref()
                        .unwrap()
                        .panels
                        .iter()
                        .filter(|p| {
                            p.panel_type == kind && !smoke.entities.contains(&p.handle.entity())
                        })
                        .count();
                    ensure(
                        count == 3,
                        format!("native menu {kind} created {count}, expected 3"),
                    )?;
                }
                if smoke
                    .operation(world, Operation::FocusPanel(smoke.panel.unwrap()))?
                    .is_some()
                {
                    smoke.next(world);
                }
            }
        }
        10 => {
            if smoke.elapsed_frames(world) >= 2 {
                crate::app::smoke_native_menu(world, MenuAction::SplitRight)?;
                smoke.next(world);
            }
        }
        11 => {
            let panels = smoke.panels("bed.editor.text-panel");
            if panels.len() >= 2 && smoke.elapsed_frames(world) >= 2 {
                let geometry = smoke
                    .snapshot
                    .as_ref()
                    .unwrap()
                    .panels
                    .iter()
                    .filter(|p| panels.contains(&p.handle))
                    .filter_map(|p| p.visible_content)
                    .collect::<Vec<_>>();
                ensure(
                    geometry.len() >= 2
                        && geometry
                            .iter()
                            .all(|g| g.max[0] > g.min[0] && g.max[1] > g.min[1]),
                    "split-right leaves have no visible geometry",
                )?;
                smoke.other = panels.into_iter().find(|p| Some(*p) != smoke.panel);
                crate::app::smoke_native_menu(world, MenuAction::SplitDown)?;
                smoke.next(world);
            }
        }
        12 => {
            if smoke.panels("bed.editor.text-panel").len() >= 3 && smoke.elapsed_frames(world) >= 2
            {
                let areas = smoke
                    .snapshot
                    .as_ref()
                    .unwrap()
                    .panels
                    .iter()
                    .filter(|p| p.panel_type == "bed.editor.text-panel")
                    .filter_map(|p| p.area)
                    .collect::<HashSet<_>>();
                ensure(
                    areas.len() >= 3,
                    "native splits did not create distinct leaves",
                )?;
                smoke.next(world);
            }
        }
        13 => return basic(world, smoke),
        _ => unreachable!(),
    }
    Ok(false)
}

#[cfg(not(all(target_os = "macos", feature = "editor")))]
fn platform(_: &mut World, _: &mut Smoke) -> io::Result<bool> {
    Err(io::Error::other("platform smoke requires macOS/editor"))
}

#[cfg(feature = "git")]
fn git(world: &mut World, smoke: &mut Smoke) -> io::Result<bool> {
    match smoke.phase {
        0 => {
            if smoke.fixture.is_none() {
                let fixture = Fixture::new("git")?;
                fixture.write("main.rs",b"fn greeting(name: &str) {\n    println!(\"Hello, {name}!\");\n}\n\nfn main() {\n    greeting(\"world\");\n}\n")?;
                for args in [
                    vec!["init", "-q"],
                    vec!["config", "user.name", "bEd Smoke"],
                    vec!["config", "user.email", "smoke@example.invalid"],
                    vec!["config", "core.hooksPath", ".git/hooks"],
                    vec!["add", "main.rs"],
                    vec!["-c", "commit.gpgsign=false", "commit", "-qm", "Initial"],
                ] {
                    let output = std::process::Command::new("git")
                        .current_dir(&fixture.0)
                        .args(args)
                        .output()?;
                    ensure(
                        output.status.success(),
                        String::from_utf8_lossy(&output.stderr),
                    )?;
                }
                fixture.write("main.rs",b"fn greeting(name: &str) {\n    println!(\"Welcome, {name}!\");\n}\n\nfn main() {\n    greeting(\"world\");\n}\n")?;
                ensure(
                    std::process::Command::new("git")
                        .current_dir(&fixture.0)
                        .args(["add", "main.rs"])
                        .status()?
                        .success(),
                    "git stage fixture failed",
                )?;
                fixture.write("main.rs",b"fn greeting(name: &str) {\n    println!(\"Welcome, {name}!\");\n}\n\nfn main() {\n    let name = \"bEd\";\n    greeting(name);\n}\n")?;
                fixture.write("notes.txt", b"A new file, ready to stage.\n")?;
                smoke.fixture = Some(fixture);
            }
            if fixture_workspace(world, smoke, "git")? {
                smoke.next(world);
            }
        }
        1 => {
            if smoke.open(
                world,
                smoke.fixture.as_ref().unwrap().0.join("main.rs"),
                None,
            )? {
                smoke.next(world);
            }
        }
        2 => {
            if smoke
                .operation(
                    world,
                    Operation::InvokeCommand {
                        command: bed_plugin_git::SHOW_COMMAND.into(),
                        source: smoke.panel,
                        arguments: Value::Null,
                    },
                )?
                .is_some()
            {
                smoke.next(world);
            }
        }
        3 | 4 => {
            let side = if smoke.phase == 3 {
                "staged"
            } else {
                "unstaged"
            };
            if let Some(OperationValue::Opened(panel)) = smoke.operation(
                world,
                Operation::OpenPanel {
                    panel_type: bed_plugin_git::DIFF_PANEL_ID.into(),
                    state: json!({"path":"main.rs","side":side}),
                    area: None,
                },
            )? {
                if smoke.phase == 3 {
                    smoke.other = Some(panel);
                } else {
                    smoke.panel = Some(panel);
                }
                smoke.next(world);
            }
        }
        5 => {
            if smoke.ready(smoke.panel.unwrap())? && smoke.ready(smoke.other.unwrap())? {
                ensure(
                    smoke.panels(bed_plugin_git::DIFF_PANEL_ID).len() == 2,
                    "Git comparisons did not load both sides",
                )?;
                if smoke
                    .operation(world, Operation::FocusPanel(smoke.panel.unwrap()))?
                    .is_some()
                {
                    smoke.next(world);
                }
            }
        }
        6 => {
            let staged = binding(world, smoke.other.unwrap())?
                .document
                .ok_or_else(|| io::Error::other("staged comparison has no document"))?;
            let working = binding(world, smoke.panel.unwrap())?
                .document
                .ok_or_else(|| io::Error::other("working comparison has no document"))?;
            {
                let session = world.non_send::<bed_documents::DocumentSession>();
                ensure(
                    session.is_snapshot_document(staged) && !session.is_snapshot_document(working),
                    "Git comparisons did not retain read-only index and editable working documents",
                )?;
                let staged_bytes = session.snapshot(staged)?.bytes;
                let working_bytes = session.snapshot(working)?.bytes;
                ensure(
                    staged_bytes
                        .windows(b"Welcome".len())
                        .any(|bytes| bytes == b"Welcome")
                        && !staged_bytes
                            .windows(b"let name".len())
                            .any(|bytes| bytes == b"let name")
                        && working_bytes
                            .windows(b"let name = \"bEd\"".len())
                            .any(|bytes| bytes == b"let name = \"bEd\""),
                    "Git comparisons displayed the wrong revisions",
                )?;
            }
            if let Some(frame) = smoke.capture(
                world,
                vec![
                    CaptureTarget::Window,
                    CaptureTarget::Panel(smoke.panel.unwrap()),
                ],
            )? {
                ensure(
                    visible_pixels(&frame.images[1].rgba.pixels) > 1000,
                    "Git comparison render is blank",
                )?;
                return Ok(true);
            }
        }
        _ => unreachable!(),
    }
    Ok(false)
}

#[cfg(not(feature = "git"))]
fn git(_: &mut World, _: &mut Smoke) -> io::Result<bool> {
    Err(io::Error::other("Git smoke requires git feature"))
}

#[cfg(feature = "debugger")]
fn debug(world: &mut World, smoke: &mut Smoke) -> io::Result<bool> {
    match smoke.phase {
        0 => {
            if fixture_workspace(world, smoke, "debug")? {
                smoke.fixture.as_ref().unwrap().write("main.rs",b"fn main() {\n    let number = 42;\n    println!(\"debug smoke {number}\");\n}\n")?;
                smoke.next(world);
            }
        }
        1 => {
            if smoke.open(
                world,
                smoke.fixture.as_ref().unwrap().0.join("main.rs"),
                None,
            )? {
                smoke.next(world);
            }
        }
        2 => {
            let root = &smoke.fixture.as_ref().unwrap().0;
            let executable = root.join("debug-fixture");
            let profile = bed_debugger::DebugProfile {
                name: "Native smoke".into(),
                program: executable.to_string_lossy().into_owned(),
                build_command: format!(
                    "rustc -g --edition=2024 main.rs -o '{}'",
                    executable.display()
                ),
                cwd: root.to_string_lossy().into_owned(),
                stop_on_entry: true,
                ..default()
            };
            {
                let mut debugger = world.non_send_mut::<bed_plugin_debugger::DebugState>();
                debugger.0.profiles = vec![profile];
                debugger.0.selected = 0;
            }
            smoke.next(world);
        }
        3 => {
            if smoke
                .operation(
                    world,
                    Operation::InvokeCommand {
                        command: bed_plugin_debugger::BREAKPOINT_COMMAND.into(),
                        source: smoke.panel,
                        arguments: Value::Null,
                    },
                )?
                .is_some()
            {
                smoke.next(world);
            }
        }
        4 => {
            if smoke
                .operation(
                    world,
                    Operation::InvokeCommand {
                        command: bed_plugin_debugger::START_COMMAND.into(),
                        source: smoke.panel,
                        arguments: Value::Null,
                    },
                )?
                .is_some()
            {
                smoke.next(world);
            }
        }
        5 => {
            if world
                .non_send::<bed_plugin_debugger::DebugState>()
                .0
                .debug_smoke_ready()?
            {
                crate::app::smoke_focus_window(world);
                smoke.next(world);
            }
        }
        6 => {
            if smoke.capture(world, vec![CaptureTarget::Window])?.is_some() {
                smoke.next(world);
            }
        }
        7 => {
            if smoke
                .operation(
                    world,
                    Operation::InvokeCommand {
                        command: bed_plugin_debugger::STOP_COMMAND.into(),
                        source: smoke.panel,
                        arguments: Value::Null,
                    },
                )?
                .is_some()
            {
                return Ok(true);
            }
        }
        _ => unreachable!(),
    }
    Ok(false)
}

#[cfg(not(feature = "debugger"))]
fn debug(_: &mut World, _: &mut Smoke) -> io::Result<bool> {
    Err(io::Error::other("debug smoke requires debug feature"))
}

#[cfg(feature = "terminal")]
fn terminal(world: &mut World, smoke: &mut Smoke) -> io::Result<bool> {
    use bed_terminal::bed_terminal::BedTerminal;
    let promotion = smoke.name() == "--term-promotion-smoke";
    match smoke.phase {
        0 => {
            if smoke.fixture.is_none() {
                smoke.fixture = Some(Fixture::new("terminal")?);
                let mut terminal = world.non_send_mut::<BedTerminal>();
                terminal.configure_shell(bed_terminal::terminal_pty::TerminalShell::new(
                    "/bin/sh",
                    vec!["-i".into()],
                ));
                terminal.configure_shell_environment(std::collections::HashMap::from([
                    ("ENV".into(), String::new()),
                    ("BASH_ENV".into(), String::new()),
                    ("PS1".into(), "bed smoke> ".into()),
                    ("PS2".into(), "> ".into()),
                ]));
            }
            let panels = smoke.snapshot.as_ref().unwrap().panels.clone();
            if let Some(panel) = panels.first() {
                if smoke
                    .operation(world, Operation::ClosePanel(panel.handle))?
                    .is_some()
                {
                    smoke.snapshot = None;
                }
            } else {
                smoke.next(world);
            }
        }
        1 => {
            let cwd = smoke
                .fixture
                .as_ref()
                .unwrap()
                .0
                .to_string_lossy()
                .into_owned();
            if let Some(OperationValue::Opened(panel)) = smoke.operation(
                world,
                Operation::OpenPanel {
                    panel_type: "bed.terminal.panel".into(),
                    state: json!({"cwd":cwd}),
                    area: None,
                },
            )? {
                smoke.panel = Some(panel);
                crate::app::smoke_focus_window(world);
                smoke.next(world);
            }
        }
        2 => {
            if smoke.elapsed_frames(world) >= 4
                && world.non_send::<BedTerminal>().is_focused()
                && native_window(world, |window| window.has_focus())?
            {
                let geometry = smoke
                    .snapshot
                    .as_ref()
                    .unwrap()
                    .panels
                    .iter()
                    .find(|p| Some(p.handle) == smoke.panel)
                    .and_then(|p| p.visible_content)
                    .ok_or_else(|| io::Error::other("initial terminal geometry missing"))?;
                ensure(
                    geometry.max[0] - geometry.min[0] > 900.
                        && geometry.max[1] - geometry.min[1] > 500.,
                    "initial terminal did not fill native window",
                )?;
                if (smoke.operation.is_some()
                    || terminal_panel_focused(world, smoke.panel.unwrap())?)
                    && smoke
                        .operation(
                            world,
                            Operation::Input(terminal_input(terminal_fixture_command())),
                        )?
                        .is_some()
                {
                    smoke.next(world);
                }
            }
        }
        3 => {
            if (smoke.operation.is_some() || terminal_fixture_ready(world)?)
                && smoke
                    .operation(
                        world,
                        Operation::SplitPanel {
                            panel: smoke.panel.unwrap(),
                            axis: 0,
                            high: true,
                        },
                    )?
                    .is_some()
            {
                smoke.next(world);
            }
        }
        4 => {
            if smoke.panels("bed.terminal.panel").len() == 2 {
                let panel = smoke
                    .snapshot
                    .as_ref()
                    .unwrap()
                    .panels
                    .iter()
                    .find(|p| p.focused && p.panel_type == "bed.terminal.panel")
                    .unwrap()
                    .handle;
                smoke.panel = Some(panel);
                if (smoke.operation.is_some()
                    || terminal_panel_focused(world, smoke.panel.unwrap())?)
                    && smoke
                        .operation(
                            world,
                            Operation::Input(terminal_input(terminal_fixture_command())),
                        )?
                        .is_some()
                {
                    smoke.next(world);
                }
            }
        }
        5 => {
            if (smoke.operation.is_some() || terminal_fixture_ready(world)?)
                && smoke
                    .operation(
                        world,
                        Operation::SplitPanel {
                            panel: smoke.panel.unwrap(),
                            axis: 1,
                            high: true,
                        },
                    )?
                    .is_some()
            {
                smoke.next(world);
            }
        }
        6 => {
            if smoke.panels("bed.terminal.panel").len() == 3 {
                let panel = smoke
                    .snapshot
                    .as_ref()
                    .unwrap()
                    .panels
                    .iter()
                    .find(|p| p.focused && p.panel_type == "bed.terminal.panel")
                    .unwrap()
                    .handle;
                smoke.panel = Some(panel);
                if (smoke.operation.is_some()
                    || terminal_panel_focused(world, smoke.panel.unwrap())?)
                    && smoke
                        .operation(
                            world,
                            Operation::Input(terminal_input(terminal_fixture_command())),
                        )?
                        .is_some()
                {
                    smoke.next(world);
                }
            }
        }
        7 => {
            if terminal_fixture_ready(world)? {
                verify_terminal_splits(world, smoke)?;
                smoke.other = smoke.panel;
                let f = smoke.fixture.as_ref().unwrap();
                f.write("results.csv", b"name,value\nred,1\nblue,2\n")?;
                f.write("chart.svg",br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><rect width="100" height="100" fill="#ff8000"/></svg>"##)?;
                smoke.next(world);
            }
        }
        8 => {
            if smoke.open(
                world,
                smoke.fixture.as_ref().unwrap().0.join("results.csv"),
                None,
            )? {
                smoke.next(world);
            }
        }
        9 => {
            if smoke.open(
                world,
                smoke.fixture.as_ref().unwrap().0.join("chart.svg"),
                None,
            )? {
                smoke.next(world);
            }
        }
        10 => {
            if smoke.ready(smoke.panel.unwrap())? {
                ensure(
                    smoke.panels("bed.csv.panel").len() == 1
                        && smoke.panels("bed.image.panel").len() == 1,
                    "terminal companions missing",
                )?;
                if smoke
                    .operation(world, Operation::FocusPanel(smoke.other.unwrap()))?
                    .is_some()
                {
                    smoke.panel = smoke.other;
                    smoke.next(world);
                }
            }
        }
        11 => {
            if smoke.elapsed_frames(world) >= 2
                && terminal_fixture_ready(world)?
                && let Some(frame) = smoke.capture(
                    world,
                    vec![
                        CaptureTarget::Window,
                        CaptureTarget::Panel(smoke.panel.unwrap()),
                    ],
                )?
            {
                verify_terminal_capture(&frame, smoke.panel.unwrap())?;
                if !promotion {
                    return Ok(true);
                }
                let terminal = world.non_send::<BedTerminal>();
                smoke.terminal_ids = terminal
                    .session_ids()
                    .into_iter()
                    .map(|id| {
                        terminal
                            .process_id(id)
                            .map(|pid| (id, pid))
                            .ok_or_else(|| io::Error::other("shell pid missing"))
                    })
                    .collect::<io::Result<_>>()?;
                smoke.terminal_panel = Some(smoke.panel.unwrap().entity());
                smoke.terminal_window = Some(native_window(world, |w| w.id())?);
                smoke.next(world);
            }
        }
        12 => {
            if smoke
                .operation(world, Operation::Input(terminal_input("+workspace")))?
                .is_some()
            {
                smoke.next(world);
            }
        }
        13 => {
            if world.resource::<WorkspaceState>().spec.is_some() {
                let terminal = world.non_send::<BedTerminal>();
                let calling = world
                    .get::<PanelBinding>(smoke.terminal_panel.unwrap())
                    .and_then(|b| b.terminal)
                    .ok_or_else(|| io::Error::other("promoted terminal binding missing"))?;
                ensure(
                    terminal.session_count() == 1
                        && smoke.terminal_ids.iter().all(|(id, pid)| {
                            terminal.process_id(*id) == (*id == calling).then_some(*pid)
                        }),
                    "promotion did not preserve only calling shell",
                )?;
                ensure(
                    world.entities().contains(smoke.terminal_panel.unwrap())
                        && smoke.panels("bed.terminal.panel").len() == 1
                        && smoke.panels("bed.csv.panel").is_empty()
                        && smoke.panels("bed.image.panel").is_empty(),
                    "promotion did not retire other panels",
                )?;
                let cwd = terminal.live_working_directory(calling)?.canonicalize()?;
                ensure(
                    world
                        .resource::<WorkspaceState>()
                        .spec
                        .as_ref()
                        .unwrap()
                        .root
                        == cwd.to_string_lossy(),
                    "promotion workspace differs from calling cwd",
                )?;
                ensure(
                    native_window(world, |w| w.id())? == smoke.terminal_window.unwrap(),
                    "promotion replaced native window",
                )?;
                world.non_send_mut::<BedTerminal>().native_select_all()?;
                smoke.next(world);
            }
        }
        14 => {
            let ready = world
                .non_send::<BedTerminal>()
                .active_terminal()
                .is_some_and(|snapshot| {
                    snapshot
                        .selection_text()
                        .as_deref()
                        .is_some_and(terminal_fixture_text_ready)
                        && !snapshot.images.is_empty()
                });
            if ready {
                return basic(world, smoke);
            }
            // Resize clears selection and moves fixture rows into scrollback.
            // Their visible rendering and link attributes were checked before promotion.
            world.non_send::<BedTerminal>().native_select_all()?;
        }
        _ => unreachable!(),
    }
    Ok(false)
}

#[cfg(feature = "terminal")]
fn verify_terminal_capture(
    frame: &bed_desktop::DesktopCapturedFrame,
    panel: PanelHandle,
) -> io::Result<()> {
    let image = frame
        .images
        .iter()
        .find(|image| image.target == bed_desktop::DesktopCaptureTarget::Panel(panel.entity()))
        .ok_or_else(|| io::Error::other("terminal panel capture is missing"))?;
    let width = image.rgba.size[0] as usize;
    // Exclude the cwd/header region. Only terminal glyphs and the fixture's
    // red Kitty image in the body can satisfy this rendered-output check.
    let first_row = (48.0 * frame.info.framebuffer_scale[1]).ceil() as usize;
    let mut text = 0;
    let mut image_pixels = 0;
    for row in image.rgba.pixels.chunks_exact(width * 4).skip(first_row) {
        for pixel in row.as_chunks::<4>().0 {
            let [r, g, b, _] = *pixel;
            if r.min(g).min(b) > 110 && r.max(g).max(b) - r.min(g).min(b) < 70 {
                text += 1;
            }
            if r > 160 && g < 90 && b < 90 {
                image_pixels += 1;
            }
        }
    }
    ensure(
        text > 400 && image_pixels > 20,
        format!(
            "terminal body did not render fixture glyphs/image: {text} text pixels, {image_pixels} red image pixels"
        ),
    )
}

#[cfg(feature = "terminal")]
fn terminal_input(text: &str) -> Vec<DesktopInputStep> {
    use winit::keyboard::{Key, KeyCode, NamedKey};
    vec![
        DesktopInputStep::from(vec![DesktopInjectedEvent::Text(
            text.trim_end_matches('\n').into(),
        )]),
        DesktopInputStep::from(vec![DesktopInjectedEvent::Key {
            physical: KeyCode::Enter,
            logical: Key::Named(NamedKey::Enter),
            pressed: true,
        }]),
        DesktopInputStep::from(vec![DesktopInjectedEvent::Key {
            physical: KeyCode::Enter,
            logical: Key::Named(NamedKey::Enter),
            pressed: false,
        }]),
    ]
}

#[cfg(feature = "terminal")]
fn terminal_panel_focused(world: &mut World, panel: PanelHandle) -> io::Result<bool> {
    let session = binding(world, panel)?.terminal;
    let focused = {
        let terminal = world.non_send::<bed_terminal::bed_terminal::BedTerminal>();
        terminal.is_focused() && terminal.active_session_id() == session
    };
    Ok(focused && native_window(world, |window| window.has_focus())?)
}

#[cfg(feature = "terminal")]
fn terminal_fixture_command() -> &'static str {
    "printf '\\033[2J\\033[H\\r\\nGraphemes: é 日本語 👩‍💻\\r\\n\\033[4:3mcurly\\033[4:2m double\\033[0m\\r\\nAGENTS.md\\tNOTICE\\t\\033[34mvendor\\033[0m\\r\\n\\033]8;;https://example.com\\033\\\\OSC 8 link\\033]8;;\\033\\\\\\r\\n\\033_Ga=T,f=32,s=1,v=1,c=6,r=3,i=1,q=2;/wAA/w==\\033\\\\\\r\\nTERMINAL_GPU_READY\\r\\n'\n"
}

#[cfg(feature = "terminal")]
fn terminal_fixture_text_ready(text: &str) -> bool {
    ["é", "日本語", "👩‍💻", "TERMINAL_GPU_READY"]
        .iter()
        .all(|value| text.contains(value))
}

#[cfg(feature = "terminal")]
fn terminal_fixture_ready(world: &World) -> io::Result<bool> {
    let terminal = world.non_send::<bed_terminal::bed_terminal::BedTerminal>();
    let Some(snapshot) = terminal.active_terminal() else {
        return Ok(false);
    };
    let text = snapshot.selection_text().unwrap_or_else(|| {
        snapshot
            .lines
            .iter()
            .flat_map(|r| r.cells.iter())
            .filter(|c| c.width != 0)
            .map(|c| c.text.as_ref())
            .collect()
    });
    Ok(terminal_fixture_text_ready(&text)
        && !snapshot.images.is_empty()
        && snapshot
            .lines
            .iter()
            .flat_map(|r| r.cells.iter())
            .any(|c| c.hyperlink.as_deref() == Some("https://example.com")))
}

#[cfg(feature = "terminal")]
fn verify_terminal_splits(world: &World, smoke: &Smoke) -> io::Result<()> {
    let terminal = world.non_send::<bed_terminal::bed_terminal::BedTerminal>();
    let processes = terminal
        .session_ids()
        .into_iter()
        .filter_map(|id| terminal.process_id(id))
        .collect::<HashSet<_>>();
    let areas = smoke
        .snapshot
        .as_ref()
        .unwrap()
        .panels
        .iter()
        .filter(|p| p.panel_type == "bed.terminal.panel")
        .filter_map(|p| p.area)
        .collect::<HashSet<_>>();
    let docks = smoke
        .snapshot
        .as_ref()
        .unwrap()
        .panels
        .iter()
        .filter(|panel| panel.panel_type == "bed.terminal.panel")
        .filter_map(|panel| {
            world.get::<bed_api::workbench::PanelPresentation>(panel.handle.entity())
        })
        .map(|panel| panel.dock_id)
        .collect::<HashSet<_>>();
    ensure(
        terminal.session_count() == 3
            && processes.len() == 3
            && areas.len() == 3
            && docks.len() == 3
            && !docks.contains(&0),
        "terminal splits did not create three independent docked shells/leaves",
    )
}

#[cfg(not(feature = "terminal"))]
fn terminal(_: &mut World, _: &mut Smoke) -> io::Result<bool> {
    Err(io::Error::other("terminal smoke requires terminal feature"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn model_recovery_requires_visible_similar_pixels() {
        let pixels = [80u8, 60, 40, 255].repeat(1024);
        compare_recovered_pixels(&pixels, &pixels).unwrap();
        assert!(compare_recovered_pixels(&pixels, &[0u8; 4096]).is_err());
        assert!(compare_recovered_pixels(&pixels, &[255u8; 4096]).is_err());
        assert!(compare_recovered_pixels(&pixels, &pixels[..4092]).is_err());
    }
    #[cfg(feature = "terminal")]
    #[test]
    fn shell_input_separates_committed_unicode_text_and_enter_frames() {
        let steps = terminal_input("printf 'é 日本語 👩‍💻'\n");
        assert_eq!(steps.len(), 3);
        assert!(
            matches!(&steps[0].events[0], DesktopInjectedEvent::Text(text) if text == "printf 'é 日本語 👩‍💻'")
        );
        assert!(matches!(
            &steps[1].events[0],
            DesktopInjectedEvent::Key {
                pressed: true,
                logical: winit::keyboard::Key::Named(winit::keyboard::NamedKey::Enter),
                ..
            }
        ));
        assert!(matches!(
            &steps[2].events[0],
            DesktopInjectedEvent::Key { pressed: false, .. }
        ));
    }
}
