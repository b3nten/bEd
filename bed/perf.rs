//! The frame rate in the status bar (`ShowFrameRate`, off by default), with
//! more on hover: how long frames take, the UI's share, and how bEd is
//! updating.
//!
//! bEd updates on input (and requested repaints) unless something animates,
//! so the rate is what it actually draws, not what it could: idle, it's low.
//! While shown, the counter repaints once a second to stay current.
use bedtime_ui::{UiContext, UiDiagnostics, egui};
use bedtime_workbench::{
    settings::{SettingsAppExt, settings},
    status::StatusPieces,
};
use bevy::{
    prelude::*,
    render::{Render, RenderApp, RenderSystems},
    winit::{UpdateMode, WinitSettings},
};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

/// The span the stats cover.
const WINDOW: Duration = Duration::from_secs(1);

settings! {
    "Window" {
        pub ShowFrameRate: bool {
            default: false,
            key: "status.frame_rate",
            label: "Frame rate in status bar",
            description: "Frames drawn in the last second. Hover it for frame times.",
            apply: |world, show| world.resource_mut::<FrameStats>().shown = *show,
        }
    }
}

pub struct PerfPlugin;

impl Plugin for PerfPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FrameStats>()
            .register_setting::<ShowFrameRate>()
            .add_systems(First, start_frame.run_if(shown))
            .add_systems(Last, end_update.run_if(shown));
        // When the render world finishes a frame, for the update + render time.
        // Headless apps (tests) have no renderer.
        let rendered = app.world().resource::<FrameStats>().rendered.clone();
        if let Some(render) = app.get_sub_app_mut(RenderApp) {
            render.add_systems(
                Render,
                (move || *rendered.lock().unwrap() = Some(Instant::now()))
                    .in_set(RenderSystems::PostCleanup),
            );
        }
    }
}

/// Recent frames' timings, recorded while shown.
#[derive(Resource, Default)]
pub struct FrameStats {
    shown: bool,
    /// Frames finished within `WINDOW`, oldest first.
    frames: VecDeque<Frame>,
    /// The frame being updated: when it started, and how long its update
    /// took once done.
    current: Option<(Instant, Option<Duration>)>,
    /// When the render world last finished a frame.
    rendered: Arc<Mutex<Option<Instant>>>,
}

#[derive(Clone, Copy, Debug)]
struct Frame {
    started: Instant,
    /// The main schedule, `First` to `Last`.
    update: Duration,
    /// The update and drawing it, when the render world reported in.
    total: Option<Duration>,
}

fn shown(stats: Res<FrameStats>) -> bool {
    stats.shown
}

/// Finish the last frame (now that it's rendered) and start this one.
fn start_frame(mut stats: ResMut<FrameStats>) {
    let now = Instant::now();
    let rendered = *stats.rendered.lock().unwrap();
    if let Some((started, Some(update))) = stats.current.take() {
        let total = rendered
            .filter(|&at| at >= started + update)
            .map(|at| at - started);
        stats.push(Frame {
            started,
            update,
            total,
        });
    }
    stats.forget_before(now);
    stats.current = Some((now, None));
}

fn end_update(mut stats: ResMut<FrameStats>) {
    if let Some((started, update)) = &mut stats.current {
        *update = Some(started.elapsed());
    }
}

impl FrameStats {
    fn push(&mut self, frame: Frame) {
        self.frames.push_back(frame);
    }

    fn forget_before(&mut self, now: Instant) {
        while self
            .frames
            .front()
            .is_some_and(|frame| now.duration_since(frame.started) > WINDOW)
        {
            self.frames.pop_front();
        }
    }

    /// The frames that started within `WINDOW` of `now`.
    fn summary(&self, now: Instant) -> Summary {
        let recent: Vec<&Frame> = self
            .frames
            .iter()
            .filter(|frame| now.duration_since(frame.started) <= WINDOW)
            .collect();
        let intervals: Vec<Duration> = recent
            .windows(2)
            .map(|pair| pair[1].started - pair[0].started)
            .collect();
        Summary {
            fps: recent.len(),
            interval: Spread::of(intervals),
            update: Spread::of(recent.iter().map(|frame| frame.update)),
            total: Spread::of(recent.iter().filter_map(|frame| frame.total)),
        }
    }
}

#[derive(Debug, PartialEq)]
struct Summary {
    fps: usize,
    /// Between the starts of consecutive frames.
    interval: Option<Spread>,
    update: Option<Spread>,
    total: Option<Spread>,
}

/// The average and worst of some durations.
#[derive(Debug, PartialEq)]
struct Spread {
    average: Duration,
    worst: Duration,
}

impl Spread {
    fn of(durations: impl IntoIterator<Item = Duration>) -> Option<Self> {
        let (count, sum, worst) = durations
            .into_iter()
            .fold((0u32, Duration::ZERO, Duration::ZERO), |(n, sum, worst), d| {
                (n + 1, sum + d, worst.max(d))
            });
        (count > 0).then(|| Spread {
            average: sum / count,
            worst,
        })
    }

    fn text(spread: &Option<Self>) -> String {
        match spread {
            Some(spread) => format!(
                "{} avg · {} worst",
                ms(spread.average),
                ms(spread.worst)
            ),
            None => "–".into(),
        }
    }
}

fn ms(duration: Duration) -> String {
    format!("{:.1} ms", duration.as_secs_f64() * 1000.0)
}

/// A status bar item (`app.add_status_item`): frames drawn in the last
/// second, with frame times and the UI's stats on hover.
pub fn status_item(ui: &mut egui::Ui, world: &World, context: &UiContext<'_>) {
    let Some(stats) = world.get_resource::<FrameStats>() else {
        return;
    };
    if !stats.shown {
        return;
    }
    let summary = stats.summary(Instant::now());
    let label = StatusPieces::new(ui, context).weak(format!("{} fps", summary.fps));

    let mut rows = vec![
        ("Frame rate", format!("{} fps", summary.fps)),
        ("Frame interval", Spread::text(&summary.interval)),
        ("Update", Spread::text(&summary.update)),
        ("Update + render", Spread::text(&summary.total)),
    ];
    if let Some(diagnostics) = world.get::<UiDiagnostics>(context.surface) {
        rows.extend([
            ("UI draw", format!("{:.1} ms", diagnostics.draw_ms)),
            (
                "UI mesh",
                format!(
                    "{} vertices, {} indices",
                    diagnostics.vertices, diagnostics.indices
                ),
            ),
            ("Animations", diagnostics.active_animations.to_string()),
            (
                "Icons",
                format!(
                    "{} textures, {} rasterized",
                    diagnostics.icon_textures, diagnostics.icon_rasterizations
                ),
            ),
            ("UI frames", diagnostics.frames.to_string()),
        ]);
    }
    rows.push(("Entities", world.entities().count_spawned().to_string()));
    if let Some(winit) = world.get_resource::<WinitSettings>() {
        let mode = match winit.focused_mode {
            UpdateMode::Continuous => "continuously",
            UpdateMode::Reactive { .. } => "on input",
        };
        rows.push(("Updating", mode.into()));
    }
    context.frame(ui).tooltip_ui(&label, |frame| {
        egui::Grid::new("frame-stats")
            .num_columns(2)
            .spacing([context.metrics.spacing[0] * 3.0, context.metrics.spacing[1]])
            .show(frame.ui, |ui| {
                for (name, value) in rows {
                    ui.weak(name);
                    ui.label(value);
                    ui.end_row();
                }
            });
    });
    ui.ctx().request_repaint_after(WINDOW);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(stats: &mut FrameStats, start: Instant, at_ms: u64, update_ms: u64) {
        stats.push(Frame {
            started: start + Duration::from_millis(at_ms),
            update: Duration::from_millis(update_ms),
            total: Some(Duration::from_millis(update_ms * 2)),
        });
    }

    #[test]
    fn the_summary_covers_the_last_second() {
        let start = Instant::now();
        let mut stats = FrameStats::default();
        frame(&mut stats, start, 0, 9);
        frame(&mut stats, start, 600, 2);
        frame(&mut stats, start, 610, 4);
        frame(&mut stats, start, 640, 3);
        let now = start + Duration::from_millis(1500);
        stats.forget_before(now);
        let summary = stats.summary(now);
        assert_eq!(summary.fps, 3, "the frame at 0 ms is over a second old");
        assert_eq!(
            summary.interval,
            Some(Spread {
                average: Duration::from_millis(20),
                worst: Duration::from_millis(30),
            })
        );
        assert_eq!(
            summary.update,
            Some(Spread {
                average: Duration::from_millis(3),
                worst: Duration::from_millis(4),
            })
        );
        assert_eq!(summary.total.unwrap().worst, Duration::from_millis(8));
    }

    #[test]
    fn nothing_recent_reads_zero() {
        let summary = FrameStats::default().summary(Instant::now());
        assert_eq!(summary.fps, 0);
        assert_eq!(summary.interval, None);
    }

    #[test]
    fn frames_are_recorded_only_while_shown() {
        use bedtime_workbench::{
            WorkbenchCorePlugins,
            persistence::StoreId,
            settings::SettingsWorldExt,
        };
        let mut app = App::new();
        app.add_plugins((WorkbenchCorePlugins, PerfPlugin))
            .add_settings_layer::<crate::workspace::GlobalStore>();
        app.update();
        app.update();
        assert!(app.world().resource::<FrameStats>().frames.is_empty());

        let global = StoreId::of::<crate::workspace::GlobalStore>();
        app.world_mut()
            .settings_mut()
            .set::<ShowFrameRate>(global, true)
            .unwrap();
        app.update();
        app.update();
        app.update();
        let stats = app.world().resource::<FrameStats>();
        assert!(stats.shown);
        assert_eq!(stats.frames.len(), 2, "the third is still updating");
        assert!(stats.frames.iter().all(|frame| frame.total.is_none()));
    }
}
