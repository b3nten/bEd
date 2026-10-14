//! Paths and direct filesystem operations for bEd's named theme files.
use super::file::{BUILTIN_THEMES, ThemeFile};
use bevy::prelude::Resource;
use serde_json::Value;
use std::{
    fs, io,
    path::{Path, PathBuf},
};

#[derive(Resource, Clone)]
pub struct ThemeFiles {
    config_dir: PathBuf,
    resources_root: PathBuf,
}

impl ThemeFiles {
    pub fn new(config_dir: PathBuf, resources_root: PathBuf) -> Self {
        Self {
            config_dir,
            resources_root,
        }
    }
    pub fn theme_path(&self, selection: &str) -> io::Result<PathBuf> {
        if BUILTIN_THEMES.contains(&selection) {
            return Ok(self
                .resources_root
                .join("resources/themes")
                .join(format!("{selection}.json")));
        }
        let path = Path::new(selection);
        if selection.starts_with("themes/")
            && path.extension().is_some_and(|e| e == "json")
            && path
                .components()
                .all(|c| matches!(c, std::path::Component::Normal(_)))
        {
            return Ok(self.config_dir.join(path));
        }
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("Unknown theme '{selection}'; choose a built-in ID or themes/<name>.json"),
        ))
    }

    pub fn list_themes(&self) -> Vec<(String, String)> {
        let mut themes: Vec<_> = BUILTIN_THEMES
            .iter()
            .filter_map(|id| {
                ThemeFile::load(&self.theme_path(id).ok()?)
                    .ok()
                    .map(|t| (id.to_string(), t.name))
            })
            .collect();
        if let Ok(entries) = fs::read_dir(self.config_dir.join("themes")) {
            let mut custom: Vec<_> = entries
                .flatten()
                .filter_map(|entry| {
                    let path = entry.path();
                    if !path.is_file() || path.extension().is_none_or(|e| e != "json") {
                        return None;
                    }
                    let label = ThemeFile::load(&path).map_or_else(
                        |_| format!("{} (invalid JSON)", entry.file_name().to_string_lossy()),
                        |t| t.name,
                    );
                    Some((
                        format!("themes/{}", entry.file_name().to_string_lossy()),
                        format!("{label} (custom)"),
                    ))
                })
                .collect();
            custom.sort();
            themes.extend(custom);
        }
        themes
    }

    pub fn load_theme(&self, selection: &str) -> io::Result<ThemeFile> {
        ThemeFile::load(&self.theme_path(selection)?)
    }

    /// Save a validated color-only theme. New drafts receive a unique filename;
    /// an existing custom selection is the only destination that can be updated.
    pub(super) fn save_custom_theme(
        &self,
        destination: Option<&str>,
        value: &Value,
    ) -> io::Result<String> {
        let theme = ThemeFile::from_json(value)?;
        let value = theme.to_json();
        if let Some(selection) = destination {
            if !selection.starts_with("themes/") || Path::new(selection).components().count() != 2 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Only custom themes can be edited",
                ));
            }
            let path = self.theme_path(selection)?;
            if !path.is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    "Custom theme no longer exists; clone it to save a new copy",
                ));
            }
            super::persistence::write_atomic(&path, &value)?;
            return Ok(selection.to_owned());
        }
        let mut slug = String::new();
        for c in theme.name.chars() {
            if c.is_ascii_alphanumeric() {
                slug.push(c.to_ascii_lowercase());
            } else if !slug.is_empty() && !slug.ends_with('-') {
                slug.push('-');
            }
            if slug.len() >= 80 {
                break;
            }
        }
        let slug = slug.trim_end_matches('-');
        let slug = if slug.is_empty() {
            "custom-theme"
        } else {
            slug
        };
        for index in 1.. {
            let selection = if index == 1 {
                format!("themes/{slug}.json")
            } else {
                format!("themes/{slug}-{index}.json")
            };
            let path = self.theme_path(&selection)?;
            match super::persistence::write_atomic_new(&path, &value) {
                Ok(()) => return Ok(selection),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        unreachable!()
    }
}
