//! The file tree and the viewers, and what New makes.
use bedtime::audio::AudioPlugin;
use bedtime::code::CodePlugin;
use bedtime::diagram::DiagramPlugin;
use bedtime::file_explorer::FileExplorerPlugin;
use bedtime::font::FontPlugin;
use bedtime::hex::HexPlugin;
use bedtime::image::ImagePlugin;
use bedtime::model::ModelViewerPlugin;
use bedtime::sqlite::SqlitePlugin;
use bedtime::table::TablePlugin;
use bedtime::video::VideoPlugin;
use bevy::prelude::*;

pub struct FilesPlugin;

impl Plugin for FilesPlugin {
    fn build(&self, app: &mut App) {
        // New makes an untitled text document (a terminal makes another
        // terminal itself).
        bedtime::workbench::NewAppExt::set_new_document::<bedtime::document::TextBuffer>(app);
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
        bedtime::workbench::ActionAppExt::add_keymap(app, bedtime::code::keymap::VSCODE);
    }
}
