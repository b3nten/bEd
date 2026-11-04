use bedtime_audio::AudioPlugin;
use bedtime_code::CodePlugin;
use bedtime_filetree::FileTreePlugin;
use bedtime_font::FontPlugin;
use bedtime_hex::HexPlugin;
use bedtime_image::ImagePlugin;
use bedtime_model::ModelViewerPlugin;
use bedtime_table::TablePlugin;
use bedtime_video::VideoPlugin;
use bedtime_workbench::{PanelFactory, WorkbenchWorldExt};
use bevy::prelude::*;

pub struct FilesPlugin;

impl Plugin for FilesPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(bedtime_workbench::FileDrops(dropped))
            .init_resource::<DroppedFolder>()
            .add_systems(Update, open_dropped_folder)
            .add_plugins((
                FileTreePlugin,
                CodePlugin,
                HexPlugin,
                ImagePlugin,
                ModelViewerPlugin,
                FontPlugin,
                AudioPlugin,
                VideoPlugin,
                TablePlugin,
            ));
        // Save, Undo and Redo act on the focused panel's document.
        app.world_mut().workbench_mut().register_document_handlers();
    }
}

/// A folder dropped on the window, to open as the workspace (in `Update`,
/// not while the layout draws).
#[derive(Resource, Default)]
struct DroppedFolder(Option<std::path::PathBuf>);

/// Files dropped on a layout open where they're dropped; a folder opens as
/// the workspace.
fn dropped(world: &mut World, paths: Vec<std::path::PathBuf>) -> Option<PanelFactory> {
    if let [folder] = paths.as_slice()
        && folder.is_dir()
    {
        world.resource_mut::<DroppedFolder>().0 = Some(folder.clone());
        return None;
    }
    let files: Vec<_> = paths.into_iter().filter(|path| !path.is_dir()).collect();
    (!files.is_empty()).then(|| bedtime_filetree::open_files_factory(files))
}

fn open_dropped_folder(world: &mut World) {
    if let Some(folder) = world.resource_mut::<DroppedFolder>().0.take() {
        crate::menus::switch(world, crate::workspace::Start::Folder(folder));
    }
}

#[cfg(test)]
mod tests {
    use bedtime_workbench::{WorkbenchCorePlugins, WorkbenchWorldExt};
    use bevy::prelude::*;

    /// Which viewers bed's plugins offer for files, the one each opens with
    /// first.
    #[test]
    fn files_open_with_the_viewer_their_format_suits() {
        let mut app = App::new();
        app.add_plugins((
            WorkbenchCorePlugins,
            bedtime_code::CodePlugin,
            bedtime_hex::HexPlugin,
            bedtime_image::ImagePlugin,
            bedtime_table::TablePlugin,
            bedtime_preview::PreviewPlugin,
        ));
        let directory = std::env::temp_dir().join(format!("bed-formats-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let offered = |name: &str, contents: &[u8]| {
            let path = directory.join(name);
            std::fs::write(&path, contents).unwrap();
            app.world()
                .workbench()
                .viewers_for(&path)
                .into_iter()
                .map(|(_, label)| label)
                .collect::<Vec<_>>()
        };
        assert_eq!(offered("a.csv", b"a,b"), ["Table", "Text Editor", "Hex"]);
        assert_eq!(
            offered("README.md", b"# Hi"),
            ["Text Editor", "Markdown Preview", "Hex"]
        );
        assert_eq!(
            offered("Cargo.lock", b"version = 4"),
            ["Text Editor", "Tree View", "Hex"]
        );
        assert_eq!(
            offered("logo.svg", b"<svg/>"),
            ["Image", "Text Editor", "Hex"]
        );
        assert_eq!(
            offered("photo.png", &[137, 80, 78, 71, 0]),
            ["Image", "Hex"]
        );
        assert_eq!(offered("main.rs", b"fn main() {}"), ["Text Editor", "Hex"]);
        assert_eq!(offered("notes.poop", b"plain"), ["Text Editor", "Hex"]);
        assert_eq!(offered("blob", &[0, 1, 2]), ["Hex"]);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
