//! bEd: a workbench of bedtime's plugins, arranged. A library, so another
//! app can build on it: `src/bed.rs` is the app itself, a few lines.
//!
//! bEd is policy and coordination; what's reusable lives in bedtime.
//! [`BedPlugins`] is all of it but Bevy's own plugins and how the app runs:
//! - `workbench`: workbench under bEd's name, with `themes`, `settings` and
//!   `menus`; `titlebar` draws the menu bar and tool buttons
//! - `workspace`: switching folders, numbered layouts, the Workspaces panel
//! - `plugins`: bedtime's panels and viewers, with bEd's answers to what
//!   they leave to the app
//! - `visuals`: backdrop, screen effects and the cat
//! - `perf`: the frame-rate status item
//! - `window`: the main window's surface, titlebar and status bar, bEd's
//!   keys and the dock menu
//!
//! And for the app that runs them:
//! - [`default_plugins`]: Bevy's plugins with bEd's window
//! - `runtime`: how Bevy runs (logging, frame drawing, the executor)
//! - [`handle_shell_command`]: for `main`, before anything else
use bevy::{app::PluginGroupBuilder, prelude::*};

pub mod menus;
pub mod perf;
pub mod plugins;
pub mod runtime;
pub mod settings;
pub mod themes;
pub mod titlebar;
pub mod visuals;
pub mod window;
pub mod workbench;
pub mod workspace;

/// Started as a terminal's shell command (`+open` and the like, which run
/// this program again): does it and returns how to exit. Call it first in
/// `main`.
pub use bedtime_terminal::handle_shell_command;

/// bEd, for an app with Bevy's plugins (see [`default_plugins`]): its
/// workbench, workspaces, panels and viewers, and its look. Drop a part with
/// `.disable()`, as with Bevy's `DefaultPlugins`.
pub struct BedPlugins;

impl PluginGroup for BedPlugins {
    fn build(self) -> PluginGroupBuilder {
        PluginGroupBuilder::start::<Self>()
            // Menu items and the status bar's items appear in the order
            // their plugins add them: the frame rate is last.
            .add(workbench::BedWorkbenchPlugin)
            .add(workspace::BedWorkspacesPlugin)
            .add(visuals::VisualsPlugin)
            .add(plugins::BedtimePlugins)
            .add(perf::PerfPlugin)
            .add(bedtime_automation::AutomationPlugin::from_env())
            .add(window::BedWindowPlugin)
    }
}

/// Bevy's default plugins as bEd runs them: its window
/// ([`window::window_plugin`]), and `runtime`'s logging and rendering.
pub fn default_plugins() -> PluginGroupBuilder {
    runtime::default_plugins(DefaultPlugins.build()).set(window::window_plugin())
}
