//! The default editor is a composition of ordinary, removable Bevy plugins.
use bevy::{app::PluginGroupBuilder, prelude::*};

pub struct BedBuiltinPlugins;

impl PluginGroup for BedBuiltinPlugins {
    fn build(self) -> PluginGroupBuilder {
        let group = PluginGroupBuilder::start::<Self>();
        let group = group.add(DocsPlugin);
        #[cfg(feature = "editor")]
        let group = group.add(bed_plugin_editor::EditorPlugin);
        #[cfg(feature = "terminal")]
        let group = group
            .add(bed_plugin_terminal::TerminalPlugin)
            .add(bed_plugin_terminal::TerminalAppIntegrationPlugin);
        #[cfg(feature = "file-explorer")]
        let group = group.add(bed_plugin_file_explorer::FileExplorerPlugin);
        #[cfg(feature = "file-search")]
        let group = group.add(bed_plugin_file_search::FileSearchPlugin);
        #[cfg(feature = "debugger")]
        let group = group.add(bed_plugin_debugger::DebuggerPlugin);
        #[cfg(feature = "git")]
        let group = group.add(bed_plugin_git::GitPlugin);
        #[cfg(feature = "workspaces")]
        let group = group.add(bed_plugin_workspaces::WorkspacesPlugin);
        #[cfg(feature = "color-picker")]
        let group = group.add(bed_plugin_color_picker::ColorPickerPlugin);
        #[cfg(feature = "mascots")]
        let group = group
            .add(bed_plugin_mascots::BedMascotPlugin)
            .add(bed_plugin_mascots::DuckMascotPlugin);
        #[cfg(feature = "source-outline")]
        let group = group.add(bed_plugin_source_outline::SourceOutlinePlugin);
        #[cfg(feature = "image-viewer")]
        let group = group.add(bed_plugin_image_viewer::ImageViewerPlugin);
        #[cfg(feature = "model-viewer")]
        let group = group.add(bed_plugin_model_viewer::ModelViewerPlugin);
        #[cfg(feature = "font-viewer")]
        let group = group.add(bed_plugin_font_viewer::FontViewerPlugin);
        #[cfg(feature = "audio-player")]
        let group = group.add(bed_plugin_audio_player::AudioPlayerPlugin);
        #[cfg(feature = "csv-table")]
        let group = group.add(bed_plugin_csv_table::CsvTablePlugin);
        #[cfg(feature = "markdown-preview")]
        let group = group.add(bed_plugin_markdown_preview::MarkdownPreviewPlugin);
        #[cfg(feature = "json-tree")]
        let group = group.add(bed_plugin_json_tree::JsonTreePlugin);
        #[cfg(feature = "sqlite-browser")]
        let group = group.add(bed_plugin_sqlite_browser::SqliteBrowserPlugin);
        #[cfg(feature = "http-client")]
        let group = group.add(bed_plugin_http_client::HttpClientPlugin);
        #[cfg(feature = "settings")]
        let group = group.add(bed_plugin_settings::SettingsPlugin);
        group
    }
}

/// The generic Docs panel needs only editor menu metadata at this boundary.
struct DocsPlugin;

impl Plugin for DocsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(bed_workbench::DocsPlugin);
        app.world_mut()
            .init_resource::<bed_api::features::FeatureRegistry>();
        app.world_mut()
            .resource_mut::<bed_api::features::FeatureRegistry>()
            .register_with("bed.docs", |r| {
                r.panel_ref(bed_workbench::DOCS_PANEL_ID);
                r.command_ref(bed_workbench::SHOW_DOCS_COMMAND);
                r.menu(
                    bed_api::features::MenuSlot::Application,
                    bed_workbench::SHOW_DOCS_COMMAND,
                );
            })
            .expect("Docs metadata is installed once");
    }
}
