//! bEd as plugins: a workbench of bedtime's plugins, arranged. A library, so
//! another app can build on it; bEd itself is `src/bed/main.rs`, a few lines,
//! with the app's assets beside it.
//!
//! bEd is policy and coordination; what's reusable lives in bedtime.
//! [`BedPlugins`] is all of it but Bevy's own plugins and how the app runs:
//! bedtime's plugins, in the order of their menus, and bEd's own:
//! - `workbench`: the app under bEd's name, with documents and workspaces,
//!   its `themes` and `settings`
//! - `workspace`: what a new layout starts with
//! - `window`: the main window's surface and titlebar (`titlebar` draws the
//!   menu bar and tool buttons), and bEd's keys
//! - `git`, `search`, `terminal`, `mascots`: bedtime's plugins of those
//!   names with bEd's answers to what they leave to the app, mostly what
//!   one does with another (a folder from Git opens as the workspace,
//!   `+open` in a terminal opens a file)
//! - `visuals`: backdrop, screen effects, and bedtime's cat
//!
//! And for the app that runs them:
//! - [`default_plugins`]: Bevy's plugins with bEd's window
//! - `runtime`: how Bevy runs (logging, and bedtime's low latency)
//! - [`handle_shell_command`]: for `main`, before anything else
use bevy::{app::PluginGroupBuilder, prelude::*};

pub mod git;
pub mod mascots;
pub mod runtime;
pub mod search;
pub mod settings;
pub mod terminal;
pub mod themes;
pub mod titlebar;
pub mod visuals;
pub mod window;
pub mod workbench;
pub mod workspace;

/// Started as a terminal's shell command (`+open` and the like, which run
/// this program again): does it and returns how to exit. Call it first in
/// `main`.
pub use bedtime::terminal::handle_shell_command;

/// bEd, for an app with Bevy's plugins (see [`default_plugins`]): its
/// workbench, workspaces, panels and viewers, and its look. Drop a part with
/// `.disable()`, as with Bevy's `DefaultPlugins`.
pub struct BedPlugins;

impl PluginGroup for BedPlugins {
    fn build(self) -> PluginGroupBuilder {
        PluginGroupBuilder::start::<Self>()
            // Menu items and the status bar's items appear in the order
            // their plugins add them.
            .add(workbench::BedWorkbenchPlugin)
            .add(workspace::BedWorkspacesPlugin)
            .add(visuals::VisualsPlugin)
            // The file tree, the editor, and the viewers.
            .add(bedtime::file_explorer::FileExplorerPlugin)
            .add(bedtime::code::CodePlugin)
            .add(bedtime::hex::HexPlugin)
            .add(bedtime::image::ImagePlugin)
            .add(bedtime::model::ModelViewerPlugin)
            .add(bedtime::font::FontPlugin)
            .add(bedtime::audio::AudioPlugin)
            .add(bedtime::video::VideoPlugin)
            .add(bedtime::table::TablePlugin)
            .add(bedtime::sqlite::SqlitePlugin)
            .add(bedtime::diagram::DiagramPlugin)
            // Panels.
            .add(terminal::TerminalsPlugin)
            .add(mascots::MascotsPlugin)
            .add(git::BedGitPlugin)
            .add(bedtime::http_client::HttpClientPlugin)
            .add(bedtime::web::WebPlugin)
            .add(search::BedSearchPlugin)
            // Previews: Markdown rendered and JSON/TOML as trees, viewers
            // like any other (Open to the Side shows a document's other
            // viewer beside it).
            .add(bedtime::markdown::MarkdownPlugin)
            .add(bedtime::datatree::DataTreePlugin)
            .add(bedtime::schedules::SchedulesPlugin)
            .add(bedtime::notes::NotesPlugin)
            // In the status bar, last.
            .add(bedtime::frame_rate::FrameRatePlugin)
            .add(bedtime::automation::AutomationPlugin::from_env())
            .add(window::BedWindowPlugin)
    }
}

/// Bevy's default plugins as bEd runs them: its window
/// ([`window::window_plugin`]), and `runtime`'s logging and rendering.
pub fn default_plugins() -> PluginGroupBuilder {
    runtime::default_plugins(DefaultPlugins.build()).set(window::window_plugin())
}
