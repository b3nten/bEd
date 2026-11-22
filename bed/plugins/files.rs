//! The file tree and the viewers, and what New makes.
use bedtime_audio::AudioPlugin;
use bedtime_code::CodePlugin;
use bedtime_diagram::DiagramPlugin;
use bedtime_file_explorer::FileExplorerPlugin;
use bedtime_font::FontPlugin;
use bedtime_hex::HexPlugin;
use bedtime_image::ImagePlugin;
use bedtime_model::ModelViewerPlugin;
use bedtime_sqlite::SqlitePlugin;
use bedtime_table::TablePlugin;
use bedtime_video::VideoPlugin;
use bevy::prelude::*;

pub struct FilesPlugin;

impl Plugin for FilesPlugin {
    fn build(&self, app: &mut App) {
        // New makes an untitled text document (a terminal makes another
        // terminal itself).
        bedtime_workbench::NewAppExt::set_new_document::<bedtime_document::TextBuffer>(app);
        app.add_plugins((
            FileExplorerPlugin,
            CodePlugin,
            HexPlugin,
            ImagePlugin,
            ModelViewerPlugin,
            FontPlugin,
            AudioPlugin,
            VideoPlugin,
            TablePlugin,
            SqlitePlugin,
            DiagramPlugin,
        ));
        // The code actions' keys, as VS Code has them.
        bedtime_workbench::ActionAppExt::add_keymap(app, bedtime_code::keymap::VSCODE);
    }
}
