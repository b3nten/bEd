//! Small, windowless decorative scenes. Bed owns input, targets and GPU recovery;
//! each view owns its Bevy world and advances it only while its surface is drawn.
mod mascot;
#[cfg(test)]
mod native_tests;
mod renderer;

use bed_workbench_api::{
    TextureHandle,
    gpu::{GpuContext, RenderOutput, RenderTarget},
};
pub use mascot::{BedModule, DuckModule};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SceneKind {
    Bed,
    WelcomeBed,
    Duck,
    Bedtime,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SceneFrame {
    pub background: [f32; 4],
    pub accent: [f32; 4],
    /// Pointer position relative to the surface, clamped to -1..1 on each axis.
    pub mouse: [f32; 2],
    pub animations: bool,
    /// One frame impulse, used by the mascots' squash animations.
    pub pressed: bool,
}
impl Default for SceneFrame {
    fn default() -> Self {
        Self {
            background: [0.04, 0.045, 0.07, 1.0],
            accent: [0.4, 0.65, 1.0, 1.0],
            mouse: [0.0; 2],
            animations: true,
            pressed: false,
        }
    }
}

pub struct SceneView {
    kind: SceneKind,
    texture: TextureHandle,
    size: Option<[u32; 2]>,
    frame: SceneFrame,
    time: f32,
    impulse: f32,
    last: Instant,
    revision: u64,
    settle: u8,
    renderer: Option<renderer::SceneRenderer>,
    failed_generation: Option<u64>,
}
impl SceneView {
    pub fn new(kind: SceneKind) -> Self {
        Self {
            kind,
            texture: TextureHandle::next(),
            size: None,
            frame: SceneFrame::default(),
            time: 0.0,
            impulse: 0.0,
            last: Instant::now(),
            revision: 1,
            settle: 12,
            renderer: None,
            failed_generation: None,
        }
    }
    pub fn handle(&self) -> TextureHandle {
        self.texture
    }
    pub fn update(&mut self, pixels: [u32; 2], mut frame: SceneFrame) {
        frame.mouse = frame.mouse.map(|x| {
            if x.is_finite() {
                x.clamp(-1.0, 1.0)
            } else {
                0.0
            }
        });
        if !frame.animations {
            frame.mouse = [0.0; 2];
            frame.pressed = false;
        }
        let elapsed = self.last.elapsed();
        let tick = elapsed >= Duration::from_secs_f32(1.0 / 30.0);
        if !tick && !frame.pressed {
            frame.mouse = self.frame.mouse;
        }
        if tick {
            self.last = Instant::now();
            if frame.animations {
                // A hidden panel resumes its pose, without simulating hidden time.
                self.time += elapsed.as_secs_f32().min(0.1);
                self.impulse = (self.impulse - elapsed.as_secs_f32().min(0.1) * 3.0).max(0.0);
            }
        }
        if frame.pressed && frame.animations {
            self.impulse = 1.0;
        }
        let mut stable_frame = frame;
        stable_frame.mouse = self.frame.mouse;
        if self.size != Some(pixels)
            || self.frame != stable_frame
            || (tick && self.frame.mouse != frame.mouse)
            || (tick && (frame.animations || self.settle > 0))
        {
            self.revision = self.revision.wrapping_add(1);
        }
        if tick {
            self.settle = self.settle.saturating_sub(1);
        }
        self.frame = frame;
        self.size = Some(pixels);
    }
    pub fn render_output(&self) -> Option<RenderOutput> {
        Some(RenderOutput {
            handle: self.texture,
            size: self.size?,
            depth: false,
            revision: self.revision,
        })
    }
    pub fn render(
        &mut self,
        gpu: &mut GpuContext<'_>,
        target: &RenderTarget,
    ) -> Result<(), String> {
        if self.failed_generation == Some(gpu.generation) {
            return Ok(());
        }
        self.failed_generation = None;
        if self
            .renderer
            .as_ref()
            .is_none_or(|r| r.generation != gpu.generation)
        {
            match renderer::SceneRenderer::new(gpu, self.kind) {
                Ok(renderer) => {
                    self.renderer = Some(renderer);
                    self.settle = 12;
                }
                Err(error) => {
                    self.failed_generation = Some(gpu.generation);
                    self.renderer = None;
                    return Err(error);
                }
            }
        }
        let result = self.renderer.as_mut().unwrap().render(
            gpu,
            target,
            self.frame,
            self.time,
            self.impulse,
        );
        if result.is_err() {
            self.failed_generation = Some(gpu.generation);
            self.renderer = None;
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stationary_scenes_keep_their_output_until_input_changes() {
        let mut view = SceneView::new(SceneKind::Duck);
        view.settle = 0;
        let frame = SceneFrame {
            animations: false,
            ..Default::default()
        };
        view.update([300, 200], frame);
        let output = view.render_output().unwrap();
        view.last -= Duration::from_secs(1);
        view.update([300, 200], frame);
        assert_eq!(view.render_output().unwrap(), output);
        view.last -= Duration::from_secs(1);
        view.update(
            [300, 200],
            SceneFrame {
                accent: [1.0, 0.0, 0.0, 1.0],
                ..frame
            },
        );
        assert!(view.render_output().unwrap().revision > output.revision);
    }
}
