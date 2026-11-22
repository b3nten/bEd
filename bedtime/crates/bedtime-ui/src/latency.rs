//! Low latency, for an app whose frames are light (a desktop app's, not a
//! game's): each choice here trades one of Bevy's defaults for a key or a
//! click showing sooner, and has an environment variable to compare. An
//! app opts in, where it adds Bevy's plugins:
//!
//! ```rust,ignore
//! App::new().add_plugins((
//!     bedtime_ui::low_latency(DefaultPlugins.build()),
//!     // …
//!     bedtime_ui::LowLatencyPlugin,
//! ))
//! ```
//!
//! An app that runs something heavy in the same world (a game in an editor)
//! leaves it out.
use bevy::{
    app::PluginGroupBuilder, ecs::schedule::SingleThreadedExecutor, prelude::*,
    render::pipelined_rendering::PipelinedRenderingPlugin,
};

/// Bevy's plugins without pipelined rendering.
///
/// Pipelined rendering (Bevy's default) draws a frame while the next update
/// runs: more frames when both are heavy, but a key shows a frame later.
/// With light frames, drawing each in its own update is sooner: in
/// tools/latency.swift's timing of bEd, typing to screen went from 27 ms to
/// 16 ms (median, 120 Hz). `BEDTIME_PIPELINED=1` turns pipelining back on,
/// to compare.
pub fn low_latency(plugins: PluginGroupBuilder) -> PluginGroupBuilder {
    let pipelined = std::env::var("BEDTIME_PIPELINED").is_ok_and(|value| value == "1");
    if pipelined || !plugins.contains::<PipelinedRenderingPlugin>() {
        plugins
    } else {
        plugins.disable::<PipelinedRenderingPlugin>()
    }
}

/// Frame pacing, and the single-threaded executor for every schedule.
///
/// Bevy's multi-threaded executor hands a schedule's systems to worker
/// threads and waits on them at each sync point. With small systems that
/// buys nothing, and when other processes keep every core busy (a cargo
/// build) each hand-off waits for a worker to get a turn: in bEd's
/// tests/automation timing with every core busy, an update went from 4.6 ms
/// to 9.5 ms (18 ms worst) on the multi-threaded executor, and stayed at
/// 4.2 ms (6.5 ms worst) on the single-threaded one, which was also a little
/// faster idle. Task pools (async tasks) are unaffected.
/// `BEDTIME_EXECUTOR=multi` turns the multi-threaded executor back on, to
/// compare.
pub struct LowLatencyPlugin;

impl Plugin for LowLatencyPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(bevy_framepace::FramepacePlugin);
    }

    /// After every plugin has built and finished, so every schedule exists,
    /// in the main world and the render world alike.
    fn cleanup(&self, app: &mut App) {
        if std::env::var("BEDTIME_EXECUTOR").is_ok_and(|value| value == "multi") {
            return;
        }
        for sub_app in app.sub_apps_mut().iter_mut() {
            if let Some(mut schedules) = sub_app.world_mut().get_resource_mut::<Schedules>() {
                for (_, schedule) in schedules.iter_mut() {
                    schedule.set_executor(SingleThreadedExecutor::new());
                }
            }
        }
    }
}
