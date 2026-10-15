//! Shared native-smoke fixtures and operation receipts; no application policy.
#![allow(dead_code, unused_imports)]
pub use bedtime::{
    api as bedtime_api, bevy, core as bedtime_core, desktop as bedtime_desktop, ui as bedtime_ui,
};
use bedtime_api::documents::PanelBinding;
pub use bedtime_core::bedtime_documents;
use bedtime_desktop::DesktopRenderStatus;
pub use bedtime_plugin_automation::{
    AutomationRequests, AutomationResults, CaptureTarget, Operation, OperationId, OperationValue,
    PanelHandle, Readiness, Snapshot,
};
pub use bevy::prelude::*;
use bevy::{window::PrimaryWindow, winit::WINIT_WINDOWS};
pub use serde_json::{Value, json};
pub use std::{
    io,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

pub struct Fixture(pub PathBuf);
impl Fixture {
    pub fn new(name: &str) -> io::Result<Self> {
        let path = std::env::temp_dir().join(format!("bed-native-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&path)?;
        Ok(Self(path.canonicalize()?))
    }
    pub fn write(&self, name: &str, bytes: impl AsRef<[u8]>) -> io::Result<PathBuf> {
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

#[derive(Resource)]
pub struct Smoke {
    pub verified: Arc<AtomicBool>,
    pub phase: u32,
    pub phase_frame: u64,
    pub started: Instant,
    pub operation: Option<OperationId>,
    pub application_operation: Option<bedtime_core::HostRequestId>,
    pub inspect: Option<OperationId>,
    pub snapshot: Option<Snapshot>,
    pub fixture: Option<Fixture>,
    pub panel: Option<PanelHandle>,
    pub other: Option<PanelHandle>,
    pub handle: Option<bedtime_ui::TextureHandle>,
    pub generation: u64,
    pub baseline: Vec<u8>,
    pub size: [u32; 2],
    pub entities: Vec<Entity>,
    pub gameplay: Option<Entity>,
}
impl Default for Smoke {
    fn default() -> Self {
        Self {
            verified: Arc::default(),
            phase: 0,
            phase_frame: 0,
            started: Instant::now(),
            operation: None,
            application_operation: None,
            inspect: None,
            snapshot: None,
            fixture: None,
            panel: None,
            other: None,
            handle: None,
            generation: 0,
            baseline: Vec::new(),
            size: [0; 2],
            entities: Vec::new(),
            gameplay: None,
        }
    }
}
impl Smoke {
    pub fn next(&mut self, world: &World) {
        self.phase += 1;
        self.phase_frame = presented(world);
        eprintln!("native smoke phase {}", self.phase);
    }
    pub fn elapsed_frames(&self, world: &World) -> u64 {
        presented(world).saturating_sub(self.phase_frame)
    }
    pub fn host_operation(
        &mut self,
        world: &mut World,
        action: bedtime_api::features::HostAction,
        source: Option<Entity>,
    ) -> io::Result<Option<bedtime_core::HostOutcome>> {
        let Some(id) = self.application_operation else {
            self.application_operation =
                Some(bedtime_core::submit_host_request(world, source, action));
            return Ok(None);
        };
        let Some(result) = bedtime_core::take_host_result(world, id) else {
            return Ok(None);
        };
        self.application_operation = None;
        result.map(Some).map_err(io::Error::other)
    }
    pub fn open(
        &mut self,
        world: &mut World,
        path: PathBuf,
        viewer: Option<&str>,
    ) -> io::Result<bool> {
        if let Some(result) = self.host_operation(
            world,
            bedtime_api::features::HostAction::OpenFile {
                path: path.to_string_lossy().into_owned(),
                viewer: viewer.map(bedtime_api::ViewerId::from),
            },
            None,
        )? {
            let bedtime_core::HostOutcome::Opened(panel) = result else {
                return Err(io::Error::other(format!("OpenFile returned {result:?}")));
            };
            self.panel = Some(
                bedtime_plugin_automation::panel_handle(world, panel).map_err(io::Error::other)?,
            );
            Ok(true)
        } else {
            Ok(false)
        }
    }
    pub fn poll(&mut self, world: &mut World) -> io::Result<()> {
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

    pub fn operation(
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

    pub fn poll_operation(
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

    pub fn panels(&self, kind: &str) -> Vec<PanelHandle> {
        self.snapshot.as_ref().map_or_else(Vec::new, |s| {
            s.panels
                .iter()
                .filter(|p| p.panel_type == kind)
                .map(|p| p.handle)
                .collect()
        })
    }

    pub fn ready(&self, panel: PanelHandle) -> io::Result<bool> {
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

    pub fn capture(
        &mut self,
        world: &mut World,
        targets: Vec<CaptureTarget>,
    ) -> io::Result<Option<bedtime_desktop::DesktopCapturedFrame>> {
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
}
pub fn submit(world: &mut World, operation: Operation) -> OperationId {
    world.resource_mut::<AutomationRequests>().submit(operation)
}

pub fn presented(world: &World) -> u64 {
    world
        .get_resource::<DesktopRenderStatus>()
        .map_or(0, |status| status.read().completed_frames)
}

pub fn ensure(condition: bool, message: impl Into<String>) -> io::Result<()> {
    if condition {
        Ok(())
    } else {
        Err(io::Error::other(message.into()))
    }
}

pub fn native_window<T>(
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

pub fn basic(world: &mut World, smoke: &mut Smoke) -> io::Result<bool> {
    if let Some(frame) = smoke.capture(world, vec![CaptureTarget::Window])? {
        ensure(
            visible_pixels(&frame.images[0].rgba.pixels) > 0,
            "desktop capture is blank",
        )?;
        return Ok(true);
    }
    Ok(false)
}

pub fn binding(world: &World, panel: PanelHandle) -> io::Result<PanelBinding> {
    let entity = panel_presentation(world, panel.entity())
        .ok_or_else(|| io::Error::other("presentation missing"))?;
    world
        .get::<PanelBinding>(entity)
        .cloned()
        .ok_or_else(|| io::Error::other("editor binding missing"))
}

pub fn document_bytes(world: &World, panel: PanelHandle) -> io::Result<(Vec<u8>, bool, String)> {
    let document = binding(world, panel)?
        .document
        .ok_or_else(|| io::Error::other("document binding missing"))?;
    let snapshot = world
        .non_send::<bedtime_documents::DocumentSession>()
        .snapshot(document)?;
    Ok((snapshot.bytes.to_vec(), snapshot.dirty, snapshot.path))
}

pub fn visible_pixels(pixels: &[u8]) -> usize {
    pixels
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|p| p[..3].iter().any(|v| *v > 20))
        .count()
}

pub fn focus_window(world: &mut World) {
    let _ = native_window(world, |window| {
        window.set_visible(true);
        window.focus_window();
    });
}
pub fn fixture_directory(world: &mut World, smoke: &mut Smoke, name: &str) -> io::Result<bool> {
    if smoke.fixture.is_none() {
        smoke.fixture = Some(Fixture::new(name)?);
    }
    let root = smoke.fixture.as_ref().unwrap().0.clone();
    Ok(smoke
        .host_operation(
            world,
            bedtime_api::features::HostAction::OpenDirectory {
                location: bedtime_api::files::FileLocation {
                    provider: bedtime_api::files::FileProviderId::Local,
                    path: root,
                },
            },
            None,
        )?
        .is_some())
}

pub fn panel_presentation(world: &World, panel: Entity) -> Option<Entity> {
    if let Some(frontend) = world.get::<bedtime_api::workbench::PanelFrontend>(panel) {
        Some(frontend.0)
    } else if bedtime_api::workbench::panel_owner(world, panel).is_some() {
        Some(panel)
    } else {
        None
    }
}
