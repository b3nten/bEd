//! Application identity, storage locations and process entrypoints.
use std::{
    ffi::OsString,
    io,
    path::{Component, Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApplicationIdentity {
    pub id: String,
    pub display_name: String,
}

impl ApplicationIdentity {
    pub fn new(id: impl Into<String>, display_name: impl Into<String>) -> io::Result<Self> {
        let id = id.into();
        validate_component(&id)?;
        let display_name = display_name.into();
        if display_name.trim().is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Application display name is empty",
            ));
        }
        Ok(Self { id, display_name })
    }
}

#[derive(bevy::prelude::Resource, Clone, Debug, PartialEq, Eq)]
pub struct ApplicationPaths {
    pub config_dir: PathBuf,
    pub workspace_directory_name: String,
}

impl ApplicationPaths {
    pub fn new(
        id: &str,
        config_dir: Option<PathBuf>,
        workspace_directory_name: Option<String>,
    ) -> io::Result<Self> {
        validate_component(id)?;
        let config_dir = match config_dir {
            Some(path) => path,
            None => std::env::var_os("HOME")
                .filter(|home| !home.is_empty())
                .map(|home| PathBuf::from(home).join(".config").join(id))
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::NotFound, "Home directory is not set (HOME)")
                })?,
        };
        let workspace_directory_name = workspace_directory_name.unwrap_or_else(|| format!(".{id}"));
        validate_component(&workspace_directory_name)?;
        Ok(Self {
            config_dir,
            workspace_directory_name,
        })
    }

    pub fn workspace_directory(&self, root: &Path) -> PathBuf {
        root.join(&self.workspace_directory_name)
    }
}

fn validate_component(name: &str) -> io::Result<()> {
    let path = Path::new(name);
    if name.is_empty()
        || path.components().count() != 1
        || !path
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
        || name.contains(['/', '\\', '\0'])
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Application and workspace directory names must be single filesystem names",
        ));
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProcessSelector {
    ExecutablePrefix(String),
    FirstArgument(String),
}

pub struct ProcessEntrypoint {
    pub selector: ProcessSelector,
    /// Receives the full native argument vector, including argv[0].
    pub run: fn(Vec<OsString>) -> io::Result<Option<i32>>,
}

pub struct LaunchRegistration {
    pub owner: &'static str,
    pub entrypoints: Vec<ProcessEntrypoint>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn names_cannot_escape_storage_roots() {
        for name in [
            "",
            ".",
            "..",
            "../other",
            "/absolute",
            "nested/name",
            "nested\\name",
        ] {
            assert!(ApplicationIdentity::new(name, "Application").is_err());
            assert!(
                ApplicationPaths::new("bed", Some("/tmp/config".into()), Some(name.into()))
                    .is_err()
            );
        }
        let paths = ApplicationPaths::new("example", Some("/tmp/config".into()), None).unwrap();
        assert_eq!(
            paths.workspace_directory(Path::new("/tmp/project")),
            PathBuf::from("/tmp/project/.example")
        );
    }
}
