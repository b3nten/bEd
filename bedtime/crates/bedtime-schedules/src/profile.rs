//! Per-system timings captured from Bevy's `trace` spans (feature `profile`).
//!
//! Bevy wraps every system run in a `system` span (with a `name` field), every schedule run in a
//! `schedule` span and every frame of the main app in a `main app` span. [`TimingLayer`] records
//! when those spans are entered and exited, so the viewer can show a real per-frame timeline.
//!
//! Install it through `LogPlugin`:
//!
//! ```ignore
//! DefaultPlugins.set(bevy::log::LogPlugin {
//!     custom_layer: bedtime_schedules::profile::layer,
//!     ..default()
//! })
//! ```

use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex, MutexGuard, OnceLock,
        atomic::{AtomicU32, Ordering},
    },
    time::{Duration, Instant},
};

use bevy::{app::App, ecs::resource::Resource, log::BoxedLayer};
use tracing::{
    Subscriber,
    field::{Field, Visit},
    span,
};
use tracing_subscriber::{Layer, layer::Context, registry::LookupSpan};

/// One run of a system inside a frame.
#[derive(Clone, Debug)]
pub struct SystemInterval {
    /// Full system name, as `System::name()` reports it.
    pub name: String,
    /// Index of the thread that ran it (0 = first thread seen).
    pub thread: u32,
    /// Start/end relative to the frame start.
    pub start: Duration,
    pub end: Duration,
}

#[derive(Clone, Debug)]
pub struct ScheduleInterval {
    /// `Debug` rendering of the schedule label.
    pub label: String,
    pub start: Duration,
    pub end: Duration,
}

#[derive(Clone, Debug, Default)]
pub struct FrameTimings {
    pub frame: u64,
    pub duration: Duration,
    pub systems: Vec<SystemInterval>,
    pub schedules: Vec<ScheduleInterval>,
}

/// Ring of the most recent frames.
#[derive(Default)]
pub struct TimingStore {
    pub frames: VecDeque<FrameTimings>,
    pub keep: usize,
    current: FrameTimings,
    frame_start: Option<Instant>,
    frame_counter: u64,
}

/// Aggregated timing of one system over the kept frames.
#[derive(Clone, Debug)]
pub struct SystemStat {
    pub name: String,
    /// Mean total time per frame, over the frames in which the system ran.
    pub average: Duration,
    /// Total time in the most recent frame (zero if it did not run).
    pub last: Duration,
    pub max: Duration,
    /// Frames (of the kept ones) in which the system ran.
    pub frames: usize,
}

impl TimingStore {
    pub fn last(&self) -> Option<&FrameTimings> {
        self.frames.back()
    }

    /// The longest frame among the kept ones.
    pub fn slowest_frame(&self) -> Option<&FrameTimings> {
        self.frames.iter().max_by_key(|f| f.duration)
    }

    /// Mean frame duration over the kept frames.
    pub fn average_frame(&self) -> Option<Duration> {
        (!self.frames.is_empty()).then(|| {
            self.frames.iter().map(|f| f.duration).sum::<Duration>() / self.frames.len() as u32
        })
    }

    /// Systems sorted by average time per frame, descending.
    pub fn slowest_systems(&self, limit: usize) -> Vec<SystemStat> {
        let mut totals: std::collections::HashMap<&str, (Duration, usize, Duration)> =
            std::collections::HashMap::new();
        for frame in &self.frames {
            let mut in_frame: std::collections::HashMap<&str, Duration> =
                std::collections::HashMap::new();
            for s in &frame.systems {
                *in_frame.entry(s.name.as_str()).or_default() += s.end.saturating_sub(s.start);
            }
            for (name, d) in in_frame {
                let entry = totals.entry(name).or_default();
                entry.0 += d;
                entry.1 += 1;
                entry.2 = entry.2.max(d);
            }
        }
        let last = self.last();
        let mut stats: Vec<SystemStat> = totals
            .into_iter()
            .map(|(name, (sum, frames, max))| SystemStat {
                name: name.to_string(),
                average: sum / frames as u32,
                last: last
                    .map(|f| {
                        f.systems
                            .iter()
                            .filter(|s| s.name == name)
                            .map(|s| s.end.saturating_sub(s.start))
                            .sum()
                    })
                    .unwrap_or_default(),
                max,
                frames,
            })
            .collect();
        stats.sort_by(|a, b| b.average.cmp(&a.average).then(a.name.cmp(&b.name)));
        stats.truncate(limit);
        stats
    }

    /// Mean total time per frame of a system over the kept frames (frames where it ran).
    pub fn average(&self, name: &str) -> Option<Duration> {
        let mut total = Duration::ZERO;
        let mut frames = 0;
        for frame in &self.frames {
            let mut in_frame = Duration::ZERO;
            let mut ran = false;
            for s in frame.systems.iter().filter(|s| s.name == name) {
                in_frame += s.end.saturating_sub(s.start);
                ran = true;
            }
            if ran {
                total += in_frame;
                frames += 1;
            }
        }
        (frames > 0).then(|| total / frames)
    }

    fn begin_frame(&mut self, now: Instant) {
        self.frame_counter += 1;
        self.current = FrameTimings {
            frame: self.frame_counter,
            ..Default::default()
        };
        self.frame_start = Some(now);
    }

    fn end_frame(&mut self, now: Instant) {
        let Some(start) = self.frame_start.take() else {
            return;
        };
        let mut frame = std::mem::take(&mut self.current);
        frame.duration = now - start;
        self.frames.push_back(frame);
        while self.frames.len() > self.keep.max(1) {
            self.frames.pop_front();
        }
    }

    fn record_system(&mut self, name: String, thread: u32, start: Instant, end: Instant) {
        if let Some(frame_start) = self.frame_start {
            self.current.systems.push(SystemInterval {
                name,
                thread,
                start: start.saturating_duration_since(frame_start),
                end: end.saturating_duration_since(frame_start),
            });
        }
    }

    fn record_schedule(&mut self, label: String, start: Instant, end: Instant) {
        if let Some(frame_start) = self.frame_start {
            self.current.schedules.push(ScheduleInterval {
                label,
                start: start.saturating_duration_since(frame_start),
                end: end.saturating_duration_since(frame_start),
            });
        }
    }
}

/// Handle to the timing store, also inserted as a resource by [`layer`].
#[derive(Resource, Clone)]
pub struct SystemTimings(pub Arc<Mutex<TimingStore>>);

impl SystemTimings {
    pub fn lock(&self) -> MutexGuard<'_, TimingStore> {
        self.0.lock().unwrap_or_else(|p| p.into_inner())
    }
}

static STORE: OnceLock<Arc<Mutex<TimingStore>>> = OnceLock::new();

/// The process-wide timing store (there can only be one tracing subscriber).
pub fn store() -> Arc<Mutex<TimingStore>> {
    STORE
        .get_or_init(|| {
            Arc::new(Mutex::new(TimingStore {
                keep: 60,
                ..Default::default()
            }))
        })
        .clone()
}

/// `LogPlugin::custom_layer` entry point: records timings and exposes them as [`SystemTimings`].
pub fn layer(app: &mut App) -> Option<BoxedLayer> {
    app.insert_resource(SystemTimings(store()));
    Some(Box::new(TimingLayer::new()))
}

fn thread_index() -> u32 {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    thread_local! {
        static INDEX: u32 = NEXT.fetch_add(1, Ordering::Relaxed);
    }
    INDEX.with(|i| *i)
}

enum Kind {
    System(String),
    Schedule(String),
    Frame,
}

struct Entered(Instant);

#[derive(Default)]
struct NameVisitor(Option<String>);

impl Visit for NameVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "name" {
            self.0 = Some(format!("{value:?}"));
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "name" {
            self.0 = Some(value.to_string());
        }
    }
}

/// The tracing layer; use [`layer`] unless you manage the subscriber yourself.
pub struct TimingLayer {
    store: Arc<Mutex<TimingStore>>,
}

impl TimingLayer {
    pub fn new() -> Self {
        Self { store: store() }
    }

    fn lock(&self) -> MutexGuard<'_, TimingStore> {
        self.store.lock().unwrap_or_else(|p| p.into_inner())
    }
}

impl Default for TimingLayer {
    fn default() -> Self {
        Self::new()
    }
}

impl<S> Layer<S> for TimingLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &span::Attributes<'_>, id: &span::Id, ctx: Context<'_, S>) {
        let kind = match attrs.metadata().name() {
            "system" => {
                let mut visitor = NameVisitor::default();
                attrs.record(&mut visitor);
                Kind::System(visitor.0.unwrap_or_default())
            }
            "schedule" => {
                let mut visitor = NameVisitor::default();
                attrs.record(&mut visitor);
                Kind::Schedule(visitor.0.unwrap_or_default())
            }
            "main app" => Kind::Frame,
            _ => return,
        };
        if let Some(span) = ctx.span(id) {
            span.extensions_mut().insert(kind);
        }
    }

    fn on_enter(&self, id: &span::Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else {
            return;
        };
        let mut ext = span.extensions_mut();
        let is_frame = match ext.get_mut::<Kind>() {
            Some(kind) => matches!(kind, Kind::Frame),
            None => return,
        };
        let now = Instant::now();
        if is_frame {
            self.lock().begin_frame(now);
        }
        ext.replace(Entered(now));
    }

    fn on_exit(&self, id: &span::Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else {
            return;
        };
        let mut ext = span.extensions_mut();
        let Some(Entered(start)) = ext.remove::<Entered>() else {
            return;
        };
        let now = Instant::now();
        let kind = match ext.get_mut::<Kind>() {
            Some(Kind::System(name)) => Kind::System(name.clone()),
            Some(Kind::Schedule(label)) => Kind::Schedule(label.clone()),
            Some(Kind::Frame) => Kind::Frame,
            None => return,
        };
        drop(ext);
        match kind {
            Kind::System(name) => self.lock().record_system(name, thread_index(), start, now),
            Kind::Schedule(label) => self.lock().record_schedule(label, start, now),
            Kind::Frame => self.lock().end_frame(now),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(frame: u64, runs: &[(&str, u64, u64)]) -> FrameTimings {
        FrameTimings {
            frame,
            duration: Duration::from_micros(runs.iter().map(|r| r.2).max().unwrap_or(0)),
            systems: runs
                .iter()
                .map(|(name, start, end)| SystemInterval {
                    name: name.to_string(),
                    thread: 0,
                    start: Duration::from_micros(*start),
                    end: Duration::from_micros(*end),
                })
                .collect(),
            schedules: Vec::new(),
        }
    }

    #[test]
    fn slowest_systems_average_over_frames_they_ran_in() {
        let mut store = TimingStore {
            keep: 10,
            ..Default::default()
        };
        store
            .frames
            .push_back(frame(1, &[("a", 0, 100), ("b", 100, 400), ("b", 400, 500)]));
        store.frames.push_back(frame(2, &[("a", 0, 300)]));
        let stats = store.slowest_systems(10);
        assert_eq!(stats[0].name, "b");
        assert_eq!(stats[0].average, Duration::from_micros(400));
        assert_eq!(stats[0].frames, 1);
        assert_eq!(
            stats[0].last,
            Duration::ZERO,
            "b did not run in the last frame"
        );
        assert_eq!(stats[1].name, "a");
        assert_eq!(stats[1].average, Duration::from_micros(200));
        assert_eq!(stats[1].last, Duration::from_micros(300));
        assert_eq!(store.slowest_frame().unwrap().frame, 1);
        assert_eq!(store.average_frame(), Some(Duration::from_micros(400)));
    }
}
