//! Frame-stamped captures of the final desktop composition, independent of content.
use bevy::prelude::*;
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Arc, Mutex},
    time::Instant,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DesktopFrameId(pub u64);

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DesktopRect {
    pub min: [f32; 2],
    pub max: [f32; 2],
}

#[derive(Clone, Debug, PartialEq)]
pub struct DesktopPanelGeometry {
    pub panel: Entity,
    /// Body/content bounds, excluding tab headers and window chrome.
    pub content: DesktopRect,
    /// Intersection of the ancestors' content clips, in logical coordinates.
    pub clip: DesktopRect,
    pub visible: bool,
}

/// Geometry and scale captured alongside the owned ImGui draw data.
#[derive(Resource, Clone, Debug, Default, PartialEq)]
pub struct DesktopFrameInfo {
    pub frame: DesktopFrameId,
    pub generation: u64,
    pub physical_size: [u32; 2],
    pub display_pos: [f32; 2],
    pub display_size: [f32; 2],
    pub framebuffer_scale: [f32; 2],
    pub panels: Vec<DesktopPanelGeometry>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DesktopCaptureId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DesktopCaptureTarget {
    Window,
    Panel(Entity),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DesktopCaptureStatus {
    Pending,
    Complete,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DesktopCaptureError {
    DeadlineExceeded,
    TargetNotVisible(Entity),
    InvalidGeometry(Entity),
    UnsupportedFormat,
    Gpu(String),
    DesktopStopped,
}

impl std::fmt::Display for DesktopCaptureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DeadlineExceeded => f.write_str("capture deadline exceeded"),
            Self::TargetNotVisible(panel) => write!(f, "panel {panel:?} has no visible content"),
            Self::InvalidGeometry(panel) => {
                write!(f, "panel {panel:?} has invalid capture geometry")
            }
            Self::UnsupportedFormat => f.write_str("unsupported capture pixel format"),
            Self::Gpu(error) => write!(f, "capture GPU readback failed: {error}"),
            Self::DesktopStopped => f.write_str("desktop stopped before capture completed"),
        }
    }
}

impl std::error::Error for DesktopCaptureError {}

/// Packed, display-encoded RGBA8 pixels, row-major, with the presented alpha.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnedRgba {
    pub size: [u32; 2],
    pub pixels: Vec<u8>,
}

impl OwnedRgba {
    fn validate(&self) -> std::io::Result<()> {
        let expected = u64::from(self.size[0])
            .checked_mul(u64::from(self.size[1]))
            .and_then(|count| count.checked_mul(4));
        if self.size.contains(&0) || expected != Some(self.pixels.len() as u64) {
            return Err(std::io::Error::other("invalid RGBA extent or pixel count"));
        }
        Ok(())
    }

    pub fn write_png(&self, path: impl AsRef<Path>) -> std::io::Result<()> {
        self.validate()?;
        image::save_buffer_with_format(
            path,
            &self.pixels,
            self.size[0],
            self.size[1],
            image::ColorType::Rgba8,
            image::ImageFormat::Png,
        )
        .map_err(std::io::Error::other)
    }

    pub fn write_ppm(&self, path: impl AsRef<Path>) -> std::io::Result<()> {
        use std::io::Write;
        self.validate()?;
        let mut file = std::io::BufWriter::new(std::fs::File::create(path)?);
        write!(file, "P6\n{} {}\n255\n", self.size[0], self.size[1])?;
        for pixel in self.pixels.as_chunks::<4>().0 {
            file.write_all(&pixel[..3])?;
        }
        file.flush()
    }

    fn crop(&self, rect: [u32; 4]) -> Self {
        let [x, y, width, height] = rect;
        let mut pixels = Vec::with_capacity((width as usize) * (height as usize) * 4);
        for row in y..y + height {
            let start = ((row as usize) * (self.size[0] as usize) + x as usize) * 4;
            pixels.extend_from_slice(&self.pixels[start..start + width as usize * 4]);
        }
        Self {
            size: [width, height],
            pixels,
        }
    }
}

#[derive(Clone, Debug)]
pub struct DesktopCapturedImage {
    pub target: DesktopCaptureTarget,
    /// [x, y, width, height] of the returned pixels in the final window target.
    pub physical_rect: [u32; 4],
    pub rgba: OwnedRgba,
}

#[derive(Clone, Debug)]
pub struct DesktopCapturedFrame {
    pub info: DesktopFrameInfo,
    pub images: Vec<DesktopCapturedImage>,
}

struct Request {
    targets: Vec<DesktopCaptureTarget>,
    deadline: Instant,
    after_frame: Option<DesktopFrameId>,
    attempt: Option<(DesktopFrameId, u64)>,
    result: Option<Result<DesktopCapturedFrame, DesktopCaptureError>>,
    last_error: Option<DesktopCaptureError>,
}

#[derive(Default)]
struct CaptureState {
    next: u64,
    requests: BTreeMap<DesktopCaptureId, Request>,
}

#[derive(Resource, Clone, Default)]
pub(crate) struct CaptureBridge(Arc<Mutex<CaptureState>>);

#[derive(Resource, Clone, Default)]
pub struct DesktopCaptureRequests {
    bridge: CaptureBridge,
}

#[derive(Resource, Clone, Default)]
pub struct DesktopCaptureResults {
    bridge: CaptureBridge,
}

impl DesktopCaptureRequests {
    /// Capture every target from one future rendered frame. `after_frame` is exclusive.
    /// An occluded native surface can still capture the current owned composition.
    /// Device replacement or unavailable geometry retains the request until its deadline.
    pub fn submit(
        &mut self,
        targets: Vec<DesktopCaptureTarget>,
        deadline: Instant,
        after_frame: Option<DesktopFrameId>,
    ) -> DesktopCaptureId {
        let mut state = self.bridge.0.lock().expect("capture state poisoned");
        state.next += 1;
        let id = DesktopCaptureId(state.next);
        state.requests.insert(
            id,
            Request {
                targets,
                deadline,
                after_frame,
                attempt: None,
                result: None,
                last_error: None,
            },
        );
        id
    }
}

impl DesktopCaptureResults {
    pub fn status(&self, id: DesktopCaptureId) -> Option<DesktopCaptureStatus> {
        self.bridge.expire(Instant::now());
        self.bridge
            .0
            .lock()
            .expect("capture state poisoned")
            .requests
            .get(&id)
            .map(|request| {
                if request.result.is_some() {
                    DesktopCaptureStatus::Complete
                } else {
                    DesktopCaptureStatus::Pending
                }
            })
    }

    pub fn take(
        &mut self,
        id: DesktopCaptureId,
    ) -> Option<Result<DesktopCapturedFrame, DesktopCaptureError>> {
        self.bridge.expire(Instant::now());
        let mut state = self.bridge.0.lock().expect("capture state poisoned");
        state.requests.get(&id)?.result.as_ref()?;
        state.requests.remove(&id)?.result
    }
}

impl CaptureBridge {
    pub(crate) fn install(&self, world: &mut World) {
        world.insert_resource(DesktopCaptureRequests {
            bridge: self.clone(),
        });
        world.insert_resource(DesktopCaptureResults {
            bridge: self.clone(),
        });
    }

    pub(crate) fn expire(&self, now: Instant) {
        for request in self
            .0
            .lock()
            .expect("capture state poisoned")
            .requests
            .values_mut()
        {
            if request.result.is_none() && now >= request.deadline {
                request.result = Some(Err(request
                    .last_error
                    .take()
                    .unwrap_or(DesktopCaptureError::DeadlineExceeded)));
            }
        }
    }

    pub(crate) fn wants_frame(&self, info: &DesktopFrameInfo) -> bool {
        self.0
            .lock()
            .expect("capture state poisoned")
            .requests
            .values()
            .any(|request| {
                request.result.is_none()
                    && request.attempt.is_none()
                    && request.after_frame.is_none_or(|after| info.frame > after)
            })
    }

    pub(crate) fn begin(&self, info: &DesktopFrameInfo) -> Vec<DesktopCaptureId> {
        self.expire(Instant::now());
        let mut state = self.0.lock().expect("capture state poisoned");
        let mut ids = Vec::new();
        for (&id, request) in &mut state.requests {
            if request.result.is_none()
                && request.attempt.is_none()
                && request.after_frame.is_none_or(|after| info.frame > after)
            {
                request.attempt = Some((info.frame, info.generation));
                ids.push(id);
            }
        }
        ids
    }

    pub(crate) fn finish(
        &self,
        ids: &[DesktopCaptureId],
        info: &DesktopFrameInfo,
        pixels: Result<OwnedRgba, DesktopCaptureError>,
    ) {
        self.expire(Instant::now());
        let mut state = self.0.lock().expect("capture state poisoned");
        for id in ids {
            let Some(request) = state.requests.get_mut(id) else {
                continue;
            };
            if request.result.is_some() || request.attempt != Some((info.frame, info.generation)) {
                continue;
            }
            if let Err(error @ DesktopCaptureError::Gpu(_)) = &pixels {
                // Mapping can fail before Bevy observes its device-lost callback.
                // Retain the request and retry; its deadline bounds repeated failures.
                request.last_error = Some(error.clone());
                request.attempt = None;
                continue;
            }
            request.result = Some(pixels.as_ref().map_err(Clone::clone).and_then(|pixels| {
                let images = request
                    .targets
                    .iter()
                    .map(|&target| {
                        let physical_rect = target_rect(info, target)?;
                        Ok(DesktopCapturedImage {
                            target,
                            physical_rect,
                            rgba: pixels.crop(physical_rect),
                        })
                    })
                    .collect::<Result<Vec<_>, DesktopCaptureError>>()?;
                Ok(DesktopCapturedFrame {
                    info: info.clone(),
                    images,
                })
            }));
        }
    }

    pub(crate) fn retry_generation(&self, generation: u64) {
        for request in self
            .0
            .lock()
            .expect("capture state poisoned")
            .requests
            .values_mut()
        {
            if request
                .attempt
                .is_some_and(|(_, attempted)| attempted == generation)
            {
                request.attempt = None;
            }
        }
    }

    pub(crate) fn stop(&self) {
        for request in self
            .0
            .lock()
            .expect("capture state poisoned")
            .requests
            .values_mut()
        {
            if request.result.is_none() {
                request.result = Some(Err(DesktopCaptureError::DesktopStopped));
            }
        }
    }
}

fn target_rect(
    info: &DesktopFrameInfo,
    target: DesktopCaptureTarget,
) -> Result<[u32; 4], DesktopCaptureError> {
    let DesktopCaptureTarget::Panel(panel) = target else {
        return Ok([0, 0, info.physical_size[0], info.physical_size[1]]);
    };
    let geometry = info
        .panels
        .iter()
        .find(|geometry| geometry.panel == panel && geometry.visible)
        .ok_or(DesktopCaptureError::TargetNotVisible(panel))?;
    let mut min = [0; 2];
    let mut max = [0; 2];
    for axis in 0..2 {
        let values = [
            geometry.content.min[axis],
            geometry.content.max[axis],
            geometry.clip.min[axis],
            geometry.clip.max[axis],
            info.display_pos[axis],
            info.framebuffer_scale[axis],
        ];
        if values.iter().any(|value| !value.is_finite()) || info.framebuffer_scale[axis] <= 0.0 {
            return Err(DesktopCaptureError::InvalidGeometry(panel));
        }
        let lower = geometry.content.min[axis].max(geometry.clip.min[axis]);
        let upper = geometry.content.max[axis].min(geometry.clip.max[axis]);
        min[axis] = ((lower - info.display_pos[axis]) * info.framebuffer_scale[axis])
            .floor()
            .clamp(0.0, info.physical_size[axis] as f32) as u32;
        max[axis] = ((upper - info.display_pos[axis]) * info.framebuffer_scale[axis])
            .ceil()
            .clamp(0.0, info.physical_size[axis] as f32) as u32;
        if upper <= lower || max[axis] <= min[axis] {
            return Err(DesktopCaptureError::TargetNotVisible(panel));
        }
    }
    Ok([min[0], min[1], max[0] - min[0], max[1] - min[1]])
}

/// Remove GPU row padding and normalize BGRA surfaces to RGBA without changing color encoding.
pub(crate) fn unpack_pixels(
    bytes: &[u8],
    size: [u32; 2],
    stride: usize,
    format: wgpu::TextureFormat,
) -> Result<OwnedRgba, DesktopCaptureError> {
    let bgra = match format {
        wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Rgba8UnormSrgb => false,
        wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb => true,
        _ => return Err(DesktopCaptureError::UnsupportedFormat),
    };
    let row = size[0] as usize * 4;
    if size.contains(&0) || stride < row || bytes.len() < stride * size[1] as usize {
        return Err(DesktopCaptureError::Gpu("invalid readback extent".into()));
    }
    let mut pixels = Vec::with_capacity(row * size[1] as usize);
    for source in bytes.chunks_exact(stride).take(size[1] as usize) {
        for pixel in source[..row].as_chunks::<4>().0 {
            if bgra {
                pixels.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
            } else {
                pixels.extend_from_slice(pixel);
            }
        }
    }
    Ok(OwnedRgba { size, pixels })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_crops_the_same_frame_with_scale_display_origin_and_clip() {
        let bridge = CaptureBridge::default();
        let mut world = World::new();
        bridge.install(&mut world);
        let panel = world.spawn_empty().id();
        let info = DesktopFrameInfo {
            frame: DesktopFrameId(7),
            generation: 2,
            physical_size: [6, 4],
            display_pos: [10., 20.],
            display_size: [3., 2.],
            framebuffer_scale: [2., 2.],
            panels: vec![DesktopPanelGeometry {
                panel,
                content: DesktopRect {
                    min: [10., 20.],
                    max: [13., 22.],
                },
                clip: DesktopRect {
                    min: [10.5, 20.5],
                    max: [12.5, 21.5],
                },
                visible: true,
            }],
        };
        let id = world.resource_mut::<DesktopCaptureRequests>().submit(
            vec![
                DesktopCaptureTarget::Window,
                DesktopCaptureTarget::Panel(panel),
            ],
            Instant::now() + std::time::Duration::from_secs(2),
            Some(DesktopFrameId(6)),
        );
        let pixels = OwnedRgba {
            size: [6, 4],
            pixels: (0..24).flat_map(|i| [i, 0, 0, 255]).collect(),
        };
        let ids = bridge.begin(&info);
        bridge.finish(&ids, &info, Ok(pixels));
        let captured = world
            .resource_mut::<DesktopCaptureResults>()
            .take(id)
            .unwrap()
            .unwrap();
        assert_eq!(captured.info, info);
        assert_eq!(captured.images[1].physical_rect, [1, 1, 4, 2]);
        assert_eq!(
            captured.images[1]
                .rgba
                .pixels
                .as_chunks::<4>()
                .0
                .iter()
                .map(|pixel| pixel[0])
                .collect::<Vec<_>>(),
            vec![7, 8, 9, 10, 13, 14, 15, 16]
        );
    }

    #[test]
    fn replacement_retries_and_ignores_old_callbacks_and_deadlines_complete() {
        let bridge = CaptureBridge::default();
        let mut world = World::new();
        bridge.install(&mut world);
        let id = world.resource_mut::<DesktopCaptureRequests>().submit(
            vec![DesktopCaptureTarget::Window],
            Instant::now() + std::time::Duration::from_secs(2),
            Some(DesktopFrameId(3)),
        );
        let mut info = DesktopFrameInfo {
            frame: DesktopFrameId(3),
            generation: 1,
            physical_size: [1, 1],
            ..default()
        };
        assert!(bridge.begin(&info).is_empty());
        info.frame = DesktopFrameId(4);
        let old = bridge.begin(&info);
        bridge.retry_generation(1);
        bridge.finish(&old, &info, Err(DesktopCaptureError::Gpu("lost".into())));
        assert_eq!(
            world.resource::<DesktopCaptureResults>().status(id),
            Some(DesktopCaptureStatus::Pending)
        );
        info.frame = DesktopFrameId(5);
        info.generation = 2;
        let new = bridge.begin(&info);
        bridge.finish(
            &new,
            &info,
            Ok(OwnedRgba {
                size: [1, 1],
                pixels: vec![1, 2, 3, 4],
            }),
        );
        assert_eq!(
            world
                .resource_mut::<DesktopCaptureResults>()
                .take(id)
                .unwrap()
                .unwrap()
                .info
                .generation,
            2
        );
        let expired =
            world
                .resource_mut::<DesktopCaptureRequests>()
                .submit(vec![], Instant::now(), None);
        bridge.expire(Instant::now());
        assert_eq!(
            world
                .resource_mut::<DesktopCaptureResults>()
                .take(expired)
                .unwrap()
                .unwrap_err(),
            DesktopCaptureError::DeadlineExceeded
        );
    }

    #[test]
    fn bgra_padding_png_and_invalid_pixels_are_checked_at_boundary() {
        let pixels = unpack_pixels(
            &[3, 2, 1, 4, 0, 0, 0, 0, 6, 5, 4, 7, 0, 0, 0, 0],
            [1, 2],
            8,
            wgpu::TextureFormat::Bgra8UnormSrgb,
        )
        .unwrap();
        assert_eq!(pixels.pixels, vec![1, 2, 3, 4, 4, 5, 6, 7]);
        let path = std::env::temp_dir().join(format!("bed-capture-{}.png", std::process::id()));
        pixels.write_png(&path).unwrap();
        let decoded = image::open(&path).unwrap().into_rgba8();
        std::fs::remove_file(path).unwrap();
        assert_eq!(decoded.into_raw(), pixels.pixels);
        assert!(
            OwnedRgba {
                size: [2, 1],
                pixels: vec![0; 4]
            }
            .write_ppm("unused-invalid-capture.ppm")
            .is_err()
        );
        assert_eq!(
            unpack_pixels(&[0; 8], [1, 1], 8, wgpu::TextureFormat::Rgba16Float).unwrap_err(),
            DesktopCaptureError::UnsupportedFormat
        );
    }
}
