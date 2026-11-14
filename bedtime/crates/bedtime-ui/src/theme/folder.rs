//! Themes kept as files: every `<id>.ron` in a folder is a `Theme` in RON. The
//! folder is looked at again about once a second, so added, edited and removed
//! files show up while the app runs.
use super::Theme;
use bevy::prelude::*;
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime},
};

const RESCAN: Duration = Duration::from_secs(1);

/// The themes in a folder, by id (the file name without `.ron`). Insert it as
/// a resource and `UiPlugin` keeps it up to date.
#[derive(Resource)]
pub struct ThemeFolder {
    path: PathBuf,
    themes: Vec<(String, Theme)>,
    /// Files that couldn't be read, with why.
    errors: Vec<(PathBuf, String)>,
    /// Each `.ron` file's path, modification time and size, as last read.
    seen: Vec<(PathBuf, Option<SystemTime>, u64)>,
    checked: Instant,
}

impl ThemeFolder {
    /// Read the themes in `path` (which needn't exist yet).
    pub fn new(path: impl Into<PathBuf>) -> Self {
        let mut folder = Self {
            path: path.into(),
            themes: Vec::new(),
            errors: Vec::new(),
            seen: Vec::new(),
            checked: Instant::now(),
        };
        folder.read();
        folder
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn get(&self, id: &str) -> Option<&Theme> {
        self.themes
            .iter()
            .find(|(theme, _)| theme == id)
            .map(|(_, theme)| theme)
    }

    /// The themes, by id.
    pub fn themes(&self) -> impl Iterator<Item = (&str, &Theme)> {
        self.themes.iter().map(|(id, theme)| (id.as_str(), theme))
    }

    /// Theme files that couldn't be read, with why.
    pub fn errors(&self) -> impl Iterator<Item = (&Path, &str)> {
        self.errors
            .iter()
            .map(|(path, error)| (path.as_path(), error.as_str()))
    }

    /// The `.ron` files now, with what tells an edit apart.
    fn look(&self) -> Vec<(PathBuf, Option<SystemTime>, u64)> {
        let Ok(entries) = std::fs::read_dir(&self.path) else {
            return Vec::new();
        };
        let mut files: Vec<_> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "ron"))
            .map(|path| {
                let metadata = std::fs::metadata(&path).ok();
                let modified = metadata.as_ref().and_then(|m| m.modified().ok());
                (path, modified, metadata.map_or(0, |m| m.len()))
            })
            .collect();
        files.sort_by(|a, b| a.0.cmp(&b.0));
        files
    }

    fn read(&mut self) {
        self.seen = self.look();
        self.themes.clear();
        self.errors.clear();
        for (path, _, _) in &self.seen {
            let Some(id) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            match std::fs::read_to_string(path)
                .map_err(|error| error.to_string())
                .and_then(|text| Theme::from_ron(&text))
            {
                Ok(theme) => self.themes.push((id.into(), theme)),
                Err(error) => self.errors.push((path.clone(), error)),
            }
        }
    }
}

/// Look at the folder about once a second, and read it again when a file was
/// added, changed or removed. Only then does the resource change.
pub(crate) fn poll_theme_folder(folder: Option<ResMut<ThemeFolder>>) {
    let Some(mut folder) = folder else {
        return;
    };
    let unchanged = folder.bypass_change_detection();
    if unchanged.checked.elapsed() < RESCAN {
        return;
    }
    unchanged.checked = Instant::now();
    if unchanged.look() != unchanged.seen {
        folder.read();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    #[test]
    fn a_folder_of_theme_files_is_read_and_kept_up_to_date() {
        let path = std::env::temp_dir().join(format!("bed-theme-folder-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        let dark = Theme::dark().to_ron();
        std::fs::write(path.join("night.ron"), &dark).unwrap();
        std::fs::write(path.join("broken.ron"), "(").unwrap();
        std::fs::write(path.join("notes.txt"), "not a theme").unwrap();

        let mut world = World::new();
        world.insert_resource(ThemeFolder::new(&path));
        let folder = world.resource::<ThemeFolder>();
        assert_eq!(folder.get("night"), Some(&Theme::dark()));
        let ids: Vec<_> = folder.themes().map(|(id, _)| id).collect();
        assert_eq!(ids, ["night"]);
        let errors: Vec<_> = folder.errors().map(|(path, _)| path.to_owned()).collect();
        assert_eq!(errors, [path.join("broken.ron")]);

        // Changes show up when the folder is next looked at.
        std::fs::write(path.join("broken.ron"), Theme::light().to_ron()).unwrap();
        std::fs::remove_file(path.join("night.ron")).unwrap();
        world.resource_mut::<ThemeFolder>().checked -= RESCAN;
        world.run_system_once(poll_theme_folder).unwrap();
        let folder = world.resource::<ThemeFolder>();
        assert!(folder.get("night").is_none());
        assert_eq!(folder.get("broken"), Some(&Theme::light()));
        assert!(folder.errors().next().is_none());
        std::fs::remove_dir_all(path).unwrap();
    }
}
