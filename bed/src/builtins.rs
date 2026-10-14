//! bEd's feature panels and commands. Native composition installs desktop effects and idle behavior.
use bevy::prelude::*;

pub struct BedBuiltinPlugins;

impl Plugin for BedBuiltinPlugins {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<bedtime_workbench::DocsPlugin>() {
            app.add_plugins(bedtime_workbench::DocsPlugin);
        }
        #[cfg(feature = "editor")]
        if !app.is_plugin_added::<bedtime_plugin_editor::EditorPlugin>() {
            app.add_plugins(bedtime_plugin_editor::EditorPlugin);
        }
        #[cfg(feature = "terminal")]
        if !app.is_plugin_added::<bedtime_plugin_terminal::TerminalPlugin>() {
            app.add_plugins(bedtime_plugin_terminal::TerminalPlugin);
        }
        #[cfg(feature = "file-explorer")]
        if !app.is_plugin_added::<bedtime_plugin_file_explorer::FileExplorerPlugin>() {
            app.add_plugins(bedtime_plugin_file_explorer::FileExplorerPlugin);
        }
        #[cfg(feature = "file-search")]
        if !app.is_plugin_added::<bedtime_plugin_file_search::FileSearchPlugin>() {
            app.add_plugins(bedtime_plugin_file_search::FileSearchPlugin);
        }
        #[cfg(feature = "debugger")]
        if !app.is_plugin_added::<bedtime_plugin_debugger::DebuggerPlugin>() {
            app.add_plugins(bedtime_plugin_debugger::DebuggerPlugin);
        }
        #[cfg(feature = "git")]
        if !app.is_plugin_added::<bedtime_plugin_git::GitPlugin>() {
            app.add_plugins(bedtime_plugin_git::GitPlugin);
        }
        #[cfg(feature = "workspaces")]
        if !app.is_plugin_added::<bedtime_plugin_workspaces::WorkspacesPlugin>() {
            app.add_plugins(bedtime_plugin_workspaces::WorkspacesPlugin);
        }
        #[cfg(feature = "color-picker")]
        if !app.is_plugin_added::<bedtime_plugin_color_picker::ColorPickerPlugin>() {
            app.add_plugins(bedtime_plugin_color_picker::ColorPickerPlugin);
        }
        #[cfg(feature = "mascots")]
        if !app.is_plugin_added::<bedtime_plugin_mascots::BedMascotPlugin>() {
            app.add_plugins(bedtime_plugin_mascots::BedMascotPlugin);
        }
        #[cfg(feature = "mascots")]
        if !app.is_plugin_added::<bedtime_plugin_mascots::DuckMascotPlugin>() {
            app.add_plugins(bedtime_plugin_mascots::DuckMascotPlugin);
        }
        #[cfg(feature = "source-outline")]
        if !app.is_plugin_added::<bedtime_plugin_source_outline::SourceOutlinePlugin>() {
            app.add_plugins(bedtime_plugin_source_outline::SourceOutlinePlugin);
        }
        #[cfg(feature = "image-viewer")]
        if !app.is_plugin_added::<bedtime_plugin_image_viewer::ImageViewerPlugin>() {
            app.add_plugins(bedtime_plugin_image_viewer::ImageViewerPlugin);
        }
        #[cfg(feature = "model-viewer")]
        if !app.is_plugin_added::<bedtime_plugin_model_viewer::ModelViewerPlugin>() {
            app.add_plugins(bedtime_plugin_model_viewer::ModelViewerPlugin);
        }
        #[cfg(feature = "font-viewer")]
        if !app.is_plugin_added::<bedtime_plugin_font_viewer::FontViewerPlugin>() {
            app.add_plugins(bedtime_plugin_font_viewer::FontViewerPlugin);
        }
        #[cfg(feature = "audio-player")]
        if !app.is_plugin_added::<bedtime_plugin_audio_player::AudioPlayerPlugin>() {
            app.add_plugins(bedtime_plugin_audio_player::AudioPlayerPlugin);
        }
        #[cfg(feature = "csv-table")]
        if !app.is_plugin_added::<bedtime_plugin_csv_table::CsvTablePlugin>() {
            app.add_plugins(bedtime_plugin_csv_table::CsvTablePlugin);
        }
        #[cfg(feature = "markdown-preview")]
        if !app.is_plugin_added::<bedtime_plugin_markdown_preview::MarkdownPreviewPlugin>() {
            app.add_plugins(bedtime_plugin_markdown_preview::MarkdownPreviewPlugin);
        }
        #[cfg(feature = "json-tree")]
        if !app.is_plugin_added::<bedtime_plugin_json_tree::JsonTreePlugin>() {
            app.add_plugins(bedtime_plugin_json_tree::JsonTreePlugin);
        }
        #[cfg(feature = "sqlite-browser")]
        if !app.is_plugin_added::<bedtime_plugin_sqlite_browser::SqliteBrowserPlugin>() {
            app.add_plugins(bedtime_plugin_sqlite_browser::SqliteBrowserPlugin);
        }
        #[cfg(feature = "http-client")]
        if !app.is_plugin_added::<bedtime_plugin_http_client::HttpClientPlugin>() {
            app.add_plugins(bedtime_plugin_http_client::HttpClientPlugin);
        }
        #[cfg(feature = "settings")]
        if !app.is_plugin_added::<crate::settings_view::BedSettingsPagesPlugin>() {
            app.add_plugins(crate::settings_view::BedSettingsPagesPlugin);
        }
    }
}
