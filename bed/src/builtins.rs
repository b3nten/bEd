//! bEd's feature panels and commands. Native composition installs desktop effects and idle behavior.
use bevy::prelude::*;

pub struct BedBuiltinPlugins;

impl Plugin for BedBuiltinPlugins {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<bedtime_workbench::DocsPlugin>() {
            app.add_plugins(bedtime_workbench::DocsPlugin);
        }
        if !app.is_plugin_added::<bedtime_plugin_editor::EditorPlugin>() {
            app.add_plugins(bedtime_plugin_editor::EditorPlugin);
        }
        if !app.is_plugin_added::<bedtime_plugin_terminal::TerminalPlugin>() {
            app.add_plugins(bedtime_plugin_terminal::TerminalPlugin);
        }
        if !app.is_plugin_added::<bedtime_plugin_file_explorer::FileExplorerPlugin>() {
            app.add_plugins(bedtime_plugin_file_explorer::FileExplorerPlugin);
        }
        if !app.is_plugin_added::<bedtime_plugin_file_search::FileSearchPlugin>() {
            app.add_plugins(bedtime_plugin_file_search::FileSearchPlugin);
        }
        if !app.is_plugin_added::<bedtime_plugin_debugger::DebuggerPlugin>() {
            app.add_plugins(bedtime_plugin_debugger::DebuggerPlugin);
        }
        if !app.is_plugin_added::<bedtime_plugin_git::GitPlugin>() {
            app.add_plugins(bedtime_plugin_git::GitPlugin);
        }
        if !app.is_plugin_added::<bedtime_plugin_workspaces::WorkspacesPlugin>() {
            app.add_plugins(bedtime_plugin_workspaces::WorkspacesPlugin);
        }
        if !app.is_plugin_added::<bedtime_plugin_color_picker::ColorPickerPlugin>() {
            app.add_plugins(bedtime_plugin_color_picker::ColorPickerPlugin);
        }
        if !app.is_plugin_added::<bedtime_plugin_mascots::BedMascotPlugin>() {
            app.add_plugins(bedtime_plugin_mascots::BedMascotPlugin);
        }
        if !app.is_plugin_added::<bedtime_plugin_mascots::DuckMascotPlugin>() {
            app.add_plugins(bedtime_plugin_mascots::DuckMascotPlugin);
        }
        if !app.is_plugin_added::<bedtime_plugin_source_outline::SourceOutlinePlugin>() {
            app.add_plugins(bedtime_plugin_source_outline::SourceOutlinePlugin);
        }
        if !app.is_plugin_added::<bedtime_plugin_image_viewer::ImageViewerPlugin>() {
            app.add_plugins(bedtime_plugin_image_viewer::ImageViewerPlugin);
        }
        if !app.is_plugin_added::<bedtime_plugin_model_viewer::ModelViewerPlugin>() {
            app.add_plugins(bedtime_plugin_model_viewer::ModelViewerPlugin);
        }
        if !app.is_plugin_added::<bedtime_plugin_font_viewer::FontViewerPlugin>() {
            app.add_plugins(bedtime_plugin_font_viewer::FontViewerPlugin);
        }
        if !app.is_plugin_added::<bedtime_plugin_audio_player::AudioPlayerPlugin>() {
            app.add_plugins(bedtime_plugin_audio_player::AudioPlayerPlugin);
        }
        if !app.is_plugin_added::<bedtime_plugin_csv_table::CsvTablePlugin>() {
            app.add_plugins(bedtime_plugin_csv_table::CsvTablePlugin);
        }
        if !app.is_plugin_added::<bedtime_plugin_markdown_preview::MarkdownPreviewPlugin>() {
            app.add_plugins(bedtime_plugin_markdown_preview::MarkdownPreviewPlugin);
        }
        if !app.is_plugin_added::<bedtime_plugin_json_tree::JsonTreePlugin>() {
            app.add_plugins(bedtime_plugin_json_tree::JsonTreePlugin);
        }
        if !app.is_plugin_added::<bedtime_plugin_sqlite_browser::SqliteBrowserPlugin>() {
            app.add_plugins(bedtime_plugin_sqlite_browser::SqliteBrowserPlugin);
        }
        if !app.is_plugin_added::<bedtime_plugin_http_client::HttpClientPlugin>() {
            app.add_plugins(bedtime_plugin_http_client::HttpClientPlugin);
        }
        if !app.is_plugin_added::<crate::settings_view::BedSettingsPagesPlugin>() {
            app.add_plugins(crate::settings_view::BedSettingsPagesPlugin);
        }
    }
}
