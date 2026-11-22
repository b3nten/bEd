//! How bEd runs: bedtime's low latency (`bedtime_ui::latency`: no pipelined
//! rendering, here; the single-threaded executor and frame pacing,
//! `LowLatencyPlugin`, in `src/bed.rs`), and its logging.
use bevy::app::PluginGroupBuilder;

/// Bevy's default plugins, as bEd runs them: see [`logging`] and
/// `bedtime_ui::low_latency`.
pub fn default_plugins(plugins: PluginGroupBuilder) -> PluginGroupBuilder {
    bedtime_ui::low_latency(logging(plugins))
}

/// With `profile`, the Schedules panel's timeline is told how long each
/// system took: its layer listens to Bevy's spans.
fn logging(plugins: PluginGroupBuilder) -> PluginGroupBuilder {
    #[cfg(feature = "profile")]
    let plugins = plugins.set(bevy::log::LogPlugin {
        custom_layer: bedtime_schedules::profile::layer,
        ..Default::default()
    });
    plugins
}
