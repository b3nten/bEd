//! Immutable bEd paths and bundled defaults. Core owns live preferences and writes.
use serde_json::Value;
use std::{fs, io, path::PathBuf};

pub const SETTINGS_FILE: &str = "settings.json";
pub struct AppConfig {
    pub config_dir: PathBuf,
    pub resources_root: PathBuf,
    pub settings_path: PathBuf,
    defaults: Value,
}
impl AppConfig {
    pub fn with_paths(config_dir: PathBuf, resources_root: PathBuf) -> io::Result<Self> {
        Ok(Self {
            settings_path: config_dir.join(SETTINGS_FILE),
            config_dir,
            resources_root,
            defaults: serde_json::from_str(include_str!("../../resources/config/settings.json"))?,
        })
    }
    pub(crate) fn application_defaults(&self) -> Value {
        self.defaults.clone()
    }
    pub fn get_app_resources_path() -> PathBuf {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let mut candidates = vec![
            cwd.clone(),
            cwd.join("bed"),
            cwd.join("../bed"),
            cwd.join("../../bed"),
            cwd.join("bed/bed"),
        ];
        if let Ok(binary) = std::env::current_exe()
            && let Some(dir) = binary.parent()
        {
            #[cfg(target_os = "macos")]
            candidates.push(dir.join("../Resources"));
            candidates.extend([dir.to_owned(), dir.join(".."), dir.join("../share/Bed")]);
        }
        candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")));
        candidates
            .into_iter()
            .find(|path| path.join("resources/config").is_dir())
            .map(|path| fs::canonicalize(&path).unwrap_or(path))
            .unwrap_or(cwd)
    }
}
