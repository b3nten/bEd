//! Workspaces: a project folder, and a store for its layout and settings.
//!
//! One workspace per process: `bed <folder>` opens it, and opening another
//! folder means another instance. Without a folder, bed still has a workbench,
//! saved in the config directory.
//!
//! A workspace's store lives under the config directory, keyed by a stable
//! hash of the folder's path, so repositories aren't touched. A lock file
//! stops two instances from saving the same workspace over each other.
use bedtime_workbench::{ErrorDialog, request_error_dialog};
use bevy::prelude::*;
use std::{
    fs::{self, File, TryLockError},
    path::{Path, PathBuf},
};

/// The open folder.
#[derive(Resource, Clone, Debug)]
pub struct Workspace {
    pub root: PathBuf,
}

impl Workspace {
    /// The folder named on the command line, if any. A path that isn't a
    /// folder is reported and ignored.
    pub fn from_args() -> Option<Self> {
        let argument = std::env::args_os().nth(1)?;
        match fs::canonicalize(&argument) {
            Ok(root) if root.is_dir() => Some(Self { root }),
            Ok(root) => {
                eprintln!("{} isn't a folder", root.display());
                None
            }
            Err(error) => {
                eprintln!("{}: {error}", PathBuf::from(argument).display());
                None
            }
        }
    }

    /// The folder's name, for the window title.
    pub fn name(&self) -> String {
        self.root.file_name().map_or_else(
            || self.root.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        )
    }

    /// Where this workspace's store lives.
    pub fn store_directory(&self, config: &Path) -> PathBuf {
        config.join("workspaces").join(format!(
            "{:016x}",
            stable_hash(self.root.as_os_str().as_encoded_bytes())
        ))
    }
}

/// FNV-1a: stable across Rust versions and platforms, unlike `std`'s hasher,
/// so a folder maps to the same directory every time.
fn stable_hash(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    })
}

/// Holds the workspace's lock for as long as the app runs. The OS releases it
/// when the process exits, including on a crash.
#[derive(Resource)]
pub struct WorkspaceLock(#[allow(dead_code)] File);

/// Claim a workspace's store directory. `Ok(None)` means another instance has
/// it. Also records which folder the directory belongs to, for people
/// browsing the config directory.
pub fn lock(directory: &Path, workspace: &Workspace) -> Result<Option<WorkspaceLock>, String> {
    let claim = || -> std::io::Result<Option<WorkspaceLock>> {
        fs::create_dir_all(directory)?;
        let file = File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(directory.join("lock"))?;
        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => return Ok(None),
            Err(TryLockError::Error(error)) => return Err(error),
        }
        fs::write(
            directory.join("path.txt"),
            workspace.root.as_os_str().as_encoded_bytes(),
        )?;
        Ok(Some(WorkspaceLock(file)))
    };
    claim().map_err(|e| format!("{}: {e}", directory.display()))
}

/// Shown when the workspace couldn't be claimed, once the window exists.
#[derive(Resource)]
pub struct Unclaimed(pub String);

pub fn report_unclaimed(world: &mut World) {
    if let Some(Unclaimed(details)) = world.remove_resource::<Unclaimed>() {
        request_error_dialog(
            world,
            ErrorDialog {
                title: "Layout won't be saved".into(),
                message: "This folder is already open in another bEd window, or its saved \
                          layout couldn't be claimed. Changes to this window's layout and \
                          workspace settings won't be saved."
                    .into(),
                details: Some(details),
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_maps_to_the_same_store_directory_every_time() {
        let workspace = Workspace {
            root: PathBuf::from("/projects/bed"),
        };
        let config = Path::new("/config");
        assert_eq!(
            workspace.store_directory(config),
            config.join("workspaces/3d7337650bbf4540")
        );
    }

    #[test]
    fn a_second_claim_on_the_same_workspace_is_refused() {
        let directory = std::env::temp_dir().join(format!("bed-lock-{}", std::process::id()));
        let workspace = Workspace {
            root: PathBuf::from("/projects/bed"),
        };
        let first = lock(&directory, &workspace).unwrap();
        assert!(first.is_some());
        assert!(lock(&directory, &workspace).unwrap().is_none());
        drop(first);
        assert!(lock(&directory, &workspace).unwrap().is_some());
        assert_eq!(
            fs::read_to_string(directory.join("path.txt")).unwrap(),
            "/projects/bed"
        );
        fs::remove_dir_all(&directory).unwrap();
    }
}
