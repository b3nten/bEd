#[path = "../../../tests/support/native_smoke.rs"]
mod support;
use bedtime_desktop::{DesktopCanvasTargets, DesktopGpu};
use bedtime_ui::{TextureHandle, gpu::CanvasOutput};
use support::*;
#[derive(Component)]
struct SmokeGameplay(u32);
#[derive(Resource, Default)]
struct ReopenDiagnostics(Option<Instant>);

fn main() -> io::Result<()> {
    let verified = Arc::<AtomicBool>::default();
    let launch = match bedtime::awaken("example.viewers", "viewers smoke")?.dispatch()? {
        bedtime::LaunchOutcome::Handled(exit) => std::process::exit(exit),
        bedtime::LaunchOutcome::Desktop(launch) => *launch,
    };
    let options = bedtime::DesktopOptions {
        config_directory: Some(
            std::env::temp_dir().join(format!("bed-viewers-config-{}", std::process::id())),
        ),
        desktop: bedtime_desktop::DesktopPlugins {
            window: Window {
                resolution: (1200, 800).into(),
                ..default()
            },
            ..default()
        },
        ..Default::default()
    };
    let exit = launch.run(options, |app| {
        app.add_plugins((
            bedtime_plugin_editor::EditorPlugin,
            bedtime_plugin_image_viewer::ImageViewerPlugin,
            bedtime_plugin_model_viewer::ModelViewerPlugin,
            bedtime_plugin_automation::AutomationPlugin,
        ))
        .insert_resource(Smoke {
            verified: Arc::clone(&verified),
            ..Default::default()
        })
        .init_resource::<ReopenDiagnostics>()
        .add_systems(Last, advance.before(bevy::app::OnAppExitSystems));
        Ok(())
    })?;
    ensure(
        verified.load(Ordering::SeqCst),
        "native smoke exited before completing its assertions",
    )?;
    if exit.is_error() {
        return Err(io::Error::other("viewers smoke failed"));
    }
    Ok(())
}
fn advance(world: &mut World) {
    world.resource_scope(|world, mut smoke: Mut<Smoke>| {
        let result = (|| -> io::Result<bool> {
            ensure(
                smoke.started.elapsed() < Duration::from_secs(90),
                format!("viewers smoke timed out in phase {}", smoke.phase),
            )?;
            if !world.contains_resource::<bedtime_desktop::DesktopGpu>() {
                return Ok(false);
            }
            smoke.poll(world)?;
            if smoke.snapshot.is_none() {
                return Ok(false);
            }
            if presented(world) < 2 {
                focus_window(world);
                return Ok(false);
            }
            viewers(world, &mut smoke)
        })();
        match result {
            Ok(true) => {
                smoke.verified.store(true, Ordering::SeqCst);
                eprintln!("viewers smoke PASS");
                world.write_message(AppExit::Success);
            }
            Ok(false) => {}
            Err(error) => {
                if smoke.phase == 7 {
                    log_reopened_image(world, &smoke);
                }
                eprintln!("viewers smoke failed: {error}");
                world.write_message(AppExit::error());
            }
        }
    });
}
fn viewers(world: &mut World, smoke: &mut Smoke) -> io::Result<bool> {
    match smoke.phase {
        0 => {
            if fixture_directory(world, smoke, "plugins")? {
                let f = smoke.fixture.as_ref().unwrap();
                f.write(
                    "image.png",
                    include_bytes!("../../../tests/fixtures/images/sample.png"),
                )?;
                f.write("bytes.bin", [0, 255, 13, 10, 239, 187, 191, 128, 0, 1])?;
                f.write(
                    "cube.glb",
                    include_bytes!("../../../tests/fixtures/gltf/cube.glb"),
                )?;
                f.write(
                    "cube.gltf",
                    include_bytes!("../../../tests/fixtures/gltf/cube.gltf"),
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
                if let Some(frame) = smoke.capture(
                    world,
                    vec![CaptureTarget::Window, CaptureTarget::Panel(panel)],
                )? {
                    ensure(
                        visible_pixels(&frame.images[1].rgba.pixels) > 1000,
                        "image viewer capture is blank",
                    )?;
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
                smoke.panels("bedtime.editor.hex-panel").len() == 1,
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
            if (smoke.application_operation.is_some()
                || (smoke.panels("bedtime.image.panel").is_empty()
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
            let log = {
                let mut diagnostics = world.resource_mut::<ReopenDiagnostics>();
                if diagnostics
                    .0
                    .is_none_or(|last| last.elapsed() >= Duration::from_secs(5))
                {
                    diagnostics.0 = Some(Instant::now());
                    true
                } else {
                    false
                }
            };
            if log {
                log_reopened_image(world, smoke);
            }
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
            } else if let Some(panel) = smoke.panels("bedtime.editor.hex-panel").first().copied() {
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
                        With<bedtime_desktop::preview::PreviewView>,
                        With<bedtime_api::workbench::PanelPresentation>,
                    )>>()
                    .iter(world)
                    .collect();
                ensure(
                    world
                        .query::<(&bedtime_desktop::preview::PreviewView, &bevy::mesh::Mesh3d)>()
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
                    Some(bedtime_plugin_editor::TEXT_VIEWER),
                )?
            {
                smoke.next(world);
            }
        }
        17 => {
            let origin = smoke.panel.unwrap();
            let origin_binding = binding(world, origin)?;
            let tab = world
                .get::<bedtime_api::workbench::PanelPresentation>(origin.entity())
                .unwrap()
                .id;
            if let Some(result) = smoke.host_operation(
                world,
                bedtime_api::features::HostAction::OpenDocumentViewer {
                    tab,
                    document: origin_binding.document.unwrap(),
                    viewer: "bedtime.gltf.viewer".into(),
                },
                Some(origin.entity()),
            )? {
                let bedtime_core::HostOutcome::Opened(panel) = result else {
                    return Err(io::Error::other(format!("Viewer open returned {result:?}")));
                };
                smoke.other = Some(origin);
                ensure(
                    panel != origin.entity(),
                    "viewer request reused the originating tab",
                )?;
                ensure(
                    world.entities().contains(origin.entity()),
                    "viewer request retired the origin",
                )?;
                smoke.panel = Some(
                    bedtime_plugin_automation::panel_handle(world, panel)
                        .map_err(io::Error::other)?,
                );
                ensure(
                    binding(world, smoke.panel.unwrap())?.document == origin_binding.document,
                    "viewer request opened a different document",
                )?;
                smoke.next(world);
            }
        }
        18 => {
            if smoke.ready(smoke.panel.unwrap())? && smoke.elapsed_frames(world) >= 20 {
                let (handle, _) = canvas(world, smoke.panel.unwrap())?;
                smoke.handle = Some(handle);
                if let Some(frame) =
                    smoke.capture(world, vec![CaptureTarget::Panel(smoke.panel.unwrap())])?
                {
                    ensure(
                        visible_pixels(&frame.images[0].rgba.pixels) > 1000,
                        "new model viewer capture is blank",
                    )?;
                    std::fs::create_dir_all("target")?;
                    frame.images[0]
                        .rgba
                        .write_png("target/native-viewers.png")?;
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
            if let Some(id) = smoke.operation {
                // The origin may retire before its close receipt is consumed.
                if smoke.poll_operation(world, id)?.is_some() {
                    smoke.next(world);
                }
            } else if smoke.panels("bedtime.gltf.panel").is_empty()
                && world
                    .resource::<DesktopCanvasTargets>()
                    .target(smoke.handle.unwrap())
                    .is_none()
            {
                let origin = smoke.other.unwrap();
                ensure(
                    world.entities().contains(origin.entity()),
                    "closing the viewer retired its code-origin tab",
                )?;
                if smoke
                    .operation(world, Operation::ClosePanel(origin))?
                    .is_some()
                {
                    smoke.next(world);
                }
            }
        }
        21 => {
            return Ok(!world.entities().contains(smoke.other.unwrap().entity())
                && smoke.panels("bedtime.gltf.panel").is_empty());
        }
        _ => unreachable!(),
    }
    Ok(false)
}

fn log_reopened_image(world: &World, smoke: &Smoke) {
    let Some(panel) = smoke.panel else {
        eprintln!("reopened image diagnostic: panel missing");
        return;
    };
    let entity = panel.entity();
    let surface = panel_presentation(world, entity);
    let presentation = world.get::<bedtime_api::workbench::PanelPresentation>(entity);
    let layout = world.resource::<bedtime_workbench::WorkbenchLayout>();
    let group = presentation.and_then(|presentation| {
        layout
            .tiling
            .areas
            .iter()
            .find(|group| group.tabs.contains(&presentation.id))
    });
    let binding = binding(world, panel).ok();
    let document = binding.as_ref().and_then(|binding| binding.document);
    let cached = world
        .resource::<bedtime_api::documents::DocumentFrame>()
        .documents
        .iter()
        .find(|cached| Some(cached.id) == document)
        .map(|cached| (cached.id, cached.revision, cached.bytes.len()));
    let session = document.map(|document| {
        world
            .non_send::<bedtime_documents::DocumentSession>()
            .snapshot(document)
            .map(|snapshot| {
                (
                    snapshot.id,
                    snapshot.version,
                    snapshot.generation,
                    snapshot.bytes.len(),
                )
            })
            .map_err(|error| error.to_string())
    });
    let observed = smoke
        .snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.panels.iter().find(|info| info.handle == panel));
    let readiness =
        surface.and_then(|surface| world.get::<bedtime_api::workbench::PanelReadiness>(surface));
    let output = surface.and_then(|surface| world.get::<CanvasOutput>(surface));
    let visible = surface.and_then(|surface| world.get::<bedtime_ui::gpu::CanvasVisible>(surface));
    let image_output = surface.and_then(|surface| {
        world
            .non_send::<bedtime_plugin_image_viewer::ImageStates>()
            .get(surface)
            .map(|image| (image.attached_document(), image.render_output()))
    });
    eprintln!(
        "reopened image diagnostic: presented={} since_phase={} ui_frame={:?} panel={entity:?} surface={surface:?} focused={:?} presentation={presentation:?} group={group:?} geometry={:?} observed={observed:?} readiness={readiness:?} document={session:?} cached={cached:?} image={image_output:?} canvas={output:?} canvas_visible={visible:?}",
        presented(world),
        smoke.elapsed_frames(world),
        world
            .get_resource::<bedtime_desktop::DesktopFrameInfo>()
            .map(|info| info.frame),
        layout.focused,
        world.get::<bedtime_api::workbench::PanelGeometry>(entity),
    );
}

fn canvas(
    world: &World,
    panel: PanelHandle,
) -> io::Result<(TextureHandle, bedtime_ui::gpu::RenderTarget)> {
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

fn read_target(world: &World, target: &bedtime_ui::gpu::RenderTarget) -> io::Result<Vec<u8>> {
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
}
