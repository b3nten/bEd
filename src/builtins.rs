//! The default editor is a composition of ordinary, removable Bevy plugins.
use bevy::{app::PluginGroupBuilder, prelude::*};

pub struct BedEditorPlugins;

impl PluginGroup for BedEditorPlugins {
    fn build(self) -> PluginGroupBuilder {
        let group = PluginGroupBuilder::start::<Self>();
        let group = group.add(DocsPlugin);
        #[cfg(feature = "editor")]
        let group = group.add(bed_editor::EditorPlugin);
        #[cfg(feature = "terminal")]
        let group = group.add(bed_terminal::TerminalPlugin);
        #[cfg(feature = "explorer")]
        let group = group.add(bed_explorer::ExplorerPlugin);
        #[cfg(feature = "search")]
        let group = group.add(bed_search::SearchPlugin);
        #[cfg(feature = "debug")]
        let group = group.add(bed_debug::DebugPlugin);
        #[cfg(feature = "git")]
        let group = group.add(bed_git::GitPlugin);
        #[cfg(feature = "projects")]
        let group = group.add(bed_projects::ProjectsPlugin);
        #[cfg(feature = "color")]
        let group = group.add(bed_color::ColorPlugin);
        #[cfg(feature = "scenes")]
        let group = group.add(bed_scenes::BedPlugin).add(bed_scenes::DuckPlugin);
        #[cfg(feature = "structure")]
        let group = group.add(bed_outline::OutlinePlugin);
        #[cfg(feature = "images")]
        let group = group.add(bed_image::ImagePlugin);
        #[cfg(feature = "models")]
        let group = group.add(bed_model_viewer::ModelViewerPlugin);
        #[cfg(feature = "fonts")]
        let group = group.add(bed_font::FontPlugin);
        #[cfg(feature = "audio")]
        let group = group.add(bed_audio::AudioPlugin);
        #[cfg(feature = "csv")]
        let group = group.add(bed_csv::CsvPlugin);
        #[cfg(feature = "markdown")]
        let group = group.add(bed_markdown::MarkdownPlugin);
        #[cfg(feature = "json")]
        let group = group.add(bed_json::JsonPlugin);
        #[cfg(feature = "sqlite")]
        let group = group.add(bed_sqlite::SqlitePlugin);
        #[cfg(feature = "http")]
        let group = group.add(bed_http::HttpPlugin);
        #[cfg(feature = "settings")]
        let group = group.add(bed_settings::SettingsPlugin);
        group
    }
}

/// The generic Docs panel needs only editor menu metadata at this boundary.
struct DocsPlugin;

impl Plugin for DocsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(bed_workbench::DocsPlugin);
        app.world_mut().init_resource::<bed_editor_api::Registry>();
        app.world_mut()
            .resource_mut::<bed_editor_api::Registry>()
            .register_with("bed.docs", |r| {
                r.panel(bed_workbench::DOCS_PANEL_ID, "Docs");
                r.command(bed_workbench::SHOW_DOCS_COMMAND, "Show Docs", None);
                r.menu(
                    bed_editor_api::MenuSlot::Application,
                    bed_workbench::SHOW_DOCS_COMMAND,
                );
            })
            .expect("Docs metadata is installed once");
    }
}
