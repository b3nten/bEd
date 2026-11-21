//! How bEd runs: logging, when frames are drawn, and how schedules run
//! their systems. Each choice here trades Bevy's default for lower latency
//! with bEd's light frames, and has an environment variable to compare.
use bevy::{
    app::PluginGroupBuilder, ecs::schedule::SingleThreadedExecutor, prelude::*,
    render::pipelined_rendering::PipelinedRenderingPlugin,
};

/// Bevy's default plugins, as bEd runs them: see [`logging`] and
/// [`rendering`].
pub fn default_plugins(plugins: PluginGroupBuilder) -> PluginGroupBuilder {
    rendering(logging(plugins))
}

/// With `profile`, the Schedules panel's timeline is told how long each
/// system took: its layer listens to Bevy's spans.
fn logging(plugins: PluginGroupBuilder) -> PluginGroupBuilder {
    #[cfg(feature = "profile")]
    let plugins = plugins.set(bevy::log::LogPlugin {
        custom_layer: bedtime_schedules::profile::layer,
        ..default()
    });
    plugins
}

/// Pipelined rendering (Bevy's default) draws a frame while the next
/// update runs: more frames when both are heavy, but a key shows a frame
/// later. bEd's frames are light, so it draws each in its own update: in
/// tools/latency.swift's timing, typing to screen went from 27 ms to
/// 16 ms (median, 120 Hz). `BED_PIPELINED=1` turns pipelining back on,
/// to compare.
fn rendering(plugins: PluginGroupBuilder) -> PluginGroupBuilder {
    if std::env::var("BED_PIPELINED").is_ok_and(|value| value == "1") {
        plugins
    } else {
        plugins.disable::<PipelinedRenderingPlugin>()
    }
}

/// Frame pacing and the executor. Add it last, so the executor's cleanup
/// sees every schedule the plugins made.
pub struct RuntimePlugin;

impl Plugin for RuntimePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((bevy_framepace::FramepacePlugin, ExecutorPlugin));
    }
}

/// Bevy's multi-threaded executor hands a schedule's systems to worker
/// threads and waits on them at each sync point. bEd's systems are small,
/// so that buys nothing, and when other processes keep every core busy
/// (a cargo build) each hand-off waits for a worker to get a turn: in
/// tests/automation timing with every core busy, an update went from
/// 4.6 ms to 9.5 ms (18 ms worst) on the multi-threaded executor, and
/// stayed at 4.2 ms (6.5 ms worst) on the single-threaded one, which was
/// also a little faster idle. Task pools (async tasks) are unaffected.
/// `BED_EXECUTOR=multi` turns the multi-threaded executor back on, to
/// compare.
struct ExecutorPlugin;

impl Plugin for ExecutorPlugin {
    fn build(&self, _: &mut App) {}

    /// After every plugin has built and finished, so every schedule
    /// exists, in the main world and the render world alike.
    fn cleanup(&self, app: &mut App) {
        if std::env::var("BED_EXECUTOR").is_ok_and(|value| value == "multi") {
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
