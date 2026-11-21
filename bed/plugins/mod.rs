//! bEd's wiring for bedtime's plugins: each module adds one plugin (or a
//! family, like the viewers) with bEd's answers to what it leaves to the
//! app (where a dropped folder goes, what `+open` does, what New makes).
pub mod documents;
pub mod files;
pub mod git;
pub mod mascots;
pub mod preview;
pub mod search;
pub mod terminal;

use bevy::prelude::*;

/// bEd's panels and viewers. Their order is their order in the menus
/// they share (View's tools and panels, Edit's find).
pub struct BedtimePlugins;

impl Plugin for BedtimePlugins {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            files::FilesPlugin,
            terminal::TerminalsPlugin,
            mascots::MascotsPlugin,
            documents::DocumentsPlugin,
            git::BedGitPlugin,
            bedtime_http_client::HttpClientPlugin,
            bedtime_web::WebPlugin,
            search::BedSearchPlugin,
            preview::BedPreviewPlugin,
            bedtime_schedules::SchedulesPlugin,
            bedtime_notes::NotesPlugin,
        ));
    }
}
