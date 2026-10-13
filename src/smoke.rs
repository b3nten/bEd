//! Native acceptance scenarios drive the same captured operations as editor clients.
#[cfg(feature = "terminal")]
use crate::application::WorkspaceState;
#[cfg(any(all(target_os = "macos", feature = "editor"), feature = "terminal"))]
use crate::application::panel_presentation;
use crate::startup::SmokeOptions;
#[cfg(any(all(target_os = "macos", feature = "editor"), feature = "terminal"))]
use bed_api::documents::PanelBinding;
#[cfg(all(target_os = "macos", feature = "editor"))]
use bed_desktop::DesktopContext;
use bed_desktop::{DesktopGpu, DesktopRenderStatus};
#[cfg(any(all(target_os = "macos", feature = "editor"), feature = "terminal"))]
use bed_desktop::{DesktopInjectedEvent, DesktopInputStep};
#[cfg(feature = "terminal")]
use bed_plugin_automation::Readiness;
use bed_plugin_automation::{
    AutomationRequests, AutomationResults, CaptureTarget, Operation, OperationId, OperationValue,
    PanelHandle, Snapshot,
};
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
    entities: Vec<Entity>,
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

struct Fixture(PathBuf);

#[cfg(any(all(target_os = "macos", feature = "editor"), feature = "terminal"))]
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
            entities: Vec::new(),
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

    #[cfg(any(all(target_os = "macos", feature = "editor"), feature = "terminal"))]
    fn panels(&self, kind: &str) -> Vec<PanelHandle> {
        self.snapshot.as_ref().map_or_else(Vec::new, |s| {
            s.panels
                .iter()
                .filter(|p| p.panel_type == kind)
                .map(|p| p.handle)
                .collect()
        })
    }

    #[cfg(feature = "terminal")]
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

    #[cfg(any(all(target_os = "macos", feature = "editor"), feature = "terminal"))]
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
        self.operation = None;
        self.application_operation = None;
        if let Some(fixture) = self.fixture.take() {
            self.retired_fixtures.push(fixture);
        }
        self.entities.clear();
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
                "--appearance-smoke" => appearance(world, &mut smoke)?,
                "--menu-smoke" => menu(world, &mut smoke)?,
                "--platform-smoke" => platform(world, &mut smoke)?,
                "--term-promotion-smoke" => terminal(world, &mut smoke)?,
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

fn setting(
    world: &mut World,
    smoke: &mut Smoke,
    owner: &str,
    key: &str,
    value: Value,
) -> io::Result<bool> {
    Ok(smoke
        .app_operation(
            world,
            crate::application::AppAction::Settings(bed_api::settings::SettingsRequest::new(
                owner, key, value,
            )),
            None,
        )?
        .is_some())
}

fn appearance(world: &mut World, smoke: &mut Smoke) -> io::Result<bool> {
    if smoke.phase == 0 {
        smoke.original_settings = world.non_send::<crate::config::Settings>().settings.clone();
        smoke.next(world);
    }
    let changes = [
        ("bed.ui", "font", json!("Paper Mono")),
        ("bed.ui", "fontSize", json!(24)),
        ("bed.application", "theme", json!("solarized-light")),
        ("bed.editor", "treesitter", json!(true)),
    ];
    if (1..=4).contains(&smoke.phase) {
        let (owner, key, value) = changes[(smoke.phase - 1) as usize].clone();
        if setting(world, smoke, owner, key, value)? {
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
    let index = (smoke.phase - 6) as usize;
    // Restore exactly the preferences this smoke changed in their owning namespaces.
    if let Some((owner, key, _)) = changes.get(index) {
        if setting(
            world,
            smoke,
            owner,
            key,
            smoke.original_settings[owner][key].clone(),
        )? {
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

#[cfg(all(target_os = "macos", feature = "editor"))]
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

fn visible_pixels(pixels: &[u8]) -> usize {
    pixels
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|p| p[..3].iter().any(|v| *v > 20))
        .count()
}

#[cfg(any(all(target_os = "macos", feature = "editor"), feature = "terminal"))]
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
            if setting(world, smoke, "bed.documents", "autosave", json!(true))? {
                smoke.next(world);
            }
        }
        3 => {
            if setting(
                world,
                smoke,
                "bed.documents",
                "autosave_delay_ms",
                json!(100),
            )? {
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
                    smoke.next(world);
                }
            }
        }
        14 => {
            // Native dispatch is admitted on the next frame. Saving must capture
            // the selected slot after the layout transition has completed.
            if world.resource::<crate::application::LayoutSlots>().active()
                == crate::config::LayoutSlot::Layout1
            {
                crate::app::smoke_native_menu(world, MenuAction::SetCurrentLayout)?;
                crate::app::smoke_native_menu(
                    world,
                    MenuAction::SetLayoutDefault(crate::config::LayoutSlot::Startup),
                )?;
                smoke.next(world);
            }
        }
        15 => {
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
                    default["version"] == 2
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
        16 => {
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
        17 => {
            if !native_window(world, |window| window.has_focus())? {
                crate::app::smoke_focus_window(world);
            } else {
                crate::app::smoke_native_key_equivalent(world, "3", 20, false)?;
                smoke.next(world);
            }
        }
        18 => {
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
        19 => {
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
        20 => {
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
        21 => {
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
        22 => {
            if smoke.elapsed_frames(world) >= 3 {
                ensure(
                    world.resource::<crate::application::LayoutSlots>().active()
                        == crate::config::LayoutSlot::Startup,
                    "Startup layout was not selected",
                )?;
                let panels = crate::application::active_panel_entities(world);
                ensure(
                    panels.contains(&smoke.panel.unwrap().entity())
                        && panels.iter().any(|panel| {
                            world
                                .get::<bed_workbench::PanelRecord>(*panel)
                                .is_some_and(|panel| panel.panel_type == "bed.workspaces.panel")
                        }),
                    "Startup did not retain its document and Workspaces panels",
                )?;
                crate::app::smoke_native_menu(
                    world,
                    MenuAction::UseLayout(crate::config::LayoutSlot::Layout3),
                )?;
                smoke.next(world);
            }
        }
        23 => {
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
            if setting(world, smoke, "bed.ui", "background_opacity", json!(0.35))? {
                smoke.next(world);
            }
        }
        5 => {
            if setting(
                world,
                smoke,
                "bed.application",
                "mac_blur_enabled",
                json!(false),
            )? {
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
                "bed.ui",
                "background_opacity",
                smoke.original_settings["bed.ui"]["background_opacity"].clone(),
            )? {
                smoke.next(world);
            }
        }
        8 => {
            if setting(
                world,
                smoke,
                "bed.application",
                "mac_blur_enabled",
                smoke.original_settings["bed.application"]["mac_blur_enabled"].clone(),
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

#[cfg(feature = "terminal")]
fn terminal(world: &mut World, smoke: &mut Smoke) -> io::Result<bool> {
    use bed_terminal::bed_terminal::BedTerminal;
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
