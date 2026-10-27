//! A serde resource mirrored to a RON file. The file is read once while the plugin
//! builds, so the resource is final before any system runs. Changes are written
//! shortly after they stop, and again on exit. A file that can't be read is never
//! overwritten; `reload` retries after the user repairs it.
use bevy::prelude::*;
use serde::{Serialize, de::DeserializeOwned};
use std::{
    fs,
    io::{ErrorKind, Write},
    marker::PhantomData,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const SAVE_DELAY: Duration = Duration::from_millis(500);

pub trait Config: Resource + Default + Clone + PartialEq + Serialize + DeserializeOwned {}
impl<T: Resource + Default + Clone + PartialEq + Serialize + DeserializeOwned> Config for T {}

/// Without a path the resource lives in memory only.
pub struct ConfigPlugin<T> {
    path: Option<PathBuf>,
    marker: PhantomData<fn() -> T>,
}

impl<T> ConfigPlugin<T> {
    pub fn new(path: Option<PathBuf>) -> Self {
        Self {
            path,
            marker: PhantomData,
        }
    }
}

impl<T: Config> Plugin for ConfigPlugin<T> {
    fn build(&self, app: &mut App) {
        let mut file = ConfigFile::<T> {
            path: self.path.clone(),
            saved: None,
            changed_at: None,
            error: None,
        };
        let value = file.read().unwrap_or_default();
        app.insert_resource(value)
            .insert_resource(file)
            .add_systems(Update, save_after_changes::<T>)
            .add_systems(Last, save_on_exit::<T>);
    }
}

#[derive(Resource)]
pub struct ConfigFile<T> {
    path: Option<PathBuf>,
    /// The file's contents as last read or written. `None` until a read succeeds,
    /// which is also what blocks writing over a file we couldn't understand.
    saved: Option<T>,
    changed_at: Option<Instant>,
    error: Option<String>,
}

impl<T: Config> ConfigFile<T> {
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// The most recent read or write failure.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// A missing file reads as the default value.
    fn read(&mut self) -> Option<T> {
        let path = self.path.as_ref()?;
        let result = match fs::read_to_string(path) {
            Ok(text) => ron::from_str(&text).map_err(|e| format!("{}: {e}", path.display())),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(T::default()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        };
        match result {
            Ok(value) => {
                self.saved = Some(value.clone());
                self.error = None;
                Some(value)
            }
            Err(error) => {
                self.error = Some(error);
                None
            }
        }
    }

    fn write(&mut self, value: &T) {
        let (Some(path), Some(saved)) = (&self.path, &self.saved) else {
            return;
        };
        if saved == value {
            return;
        }
        let result = ron::ser::to_string_pretty(value, ron::ser::PrettyConfig::default())
            .map_err(|e| e.to_string())
            .and_then(|text| atomic_write(path, text.as_bytes()));
        match result {
            Ok(()) => {
                self.saved = Some(value.clone());
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
    }
}

/// Re-read the file, replacing the resource if that succeeds.
pub fn reload<T: Config>(world: &mut World) {
    if let Some(value) = world.resource_mut::<ConfigFile<T>>().read() {
        world.insert_resource(value);
    }
}

fn save_after_changes<T: Config>(value: Res<T>, mut file: ResMut<ConfigFile<T>>) {
    if value.is_changed() {
        file.changed_at = Some(Instant::now());
    }
    if file.changed_at.is_some_and(|at| at.elapsed() >= SAVE_DELAY) {
        file.changed_at = None;
        file.write(&value);
    }
}

fn save_on_exit<T: Config>(
    mut exit: MessageReader<AppExit>,
    value: Res<T>,
    mut file: ResMut<ConfigFile<T>>,
) {
    if exit.read().count() > 0 {
        file.write(&value);
    }
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path.parent().ok_or("Path has no parent directory")?;
    let temporary = parent.join(format!(".{}.tmp", std::process::id()));
    let result = (|| {
        fs::create_dir_all(parent)?;
        let mut file = fs::File::create(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Resource, Default, Clone, PartialEq, Debug, Serialize, serde::Deserialize)]
    #[serde(default)]
    struct Values {
        size: u32,
        name: String,
    }

    fn file(path: PathBuf) -> ConfigFile<Values> {
        ConfigFile {
            path: Some(path),
            saved: None,
            changed_at: None,
            error: None,
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("bed-config-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        directory.join("values.ron")
    }

    #[test]
    fn missing_file_reads_as_default_and_writes_only_after_a_change() {
        let path = scratch("missing");
        let mut config = file(path.clone());
        assert_eq!(config.read(), Some(Values::default()));
        config.write(&Values::default());
        assert!(!path.exists());

        let changed = Values {
            size: 3,
            name: "a".into(),
        };
        config.write(&changed);
        assert_eq!(file(path).read(), Some(changed));
    }

    #[test]
    fn unreadable_file_is_preserved() {
        let path = scratch("broken");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "(size: oops)").unwrap();
        let mut config = file(path.clone());
        assert_eq!(config.read(), None);
        assert!(config.error().is_some());

        config.write(&Values {
            size: 1,
            name: String::new(),
        });
        assert_eq!(fs::read_to_string(&path).unwrap(), "(size: oops)");
    }

    #[test]
    fn missing_fields_take_defaults() {
        let path = scratch("partial");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "(size: 7)").unwrap();
        assert_eq!(
            file(path).read(),
            Some(Values {
                size: 7,
                name: String::new()
            })
        );
    }
}
