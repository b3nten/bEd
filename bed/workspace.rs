//! Workspaces: a project folder, and a store for its layout and settings.
//!
//! One workspace per process: `bed <folder>` opens it, and opening another
//! folder means another instance. Without a folder, bed still has a workbench,
//! saved in the config directory.
//!
//! A workspace's store lives under the config directory, keyed by a stable
//! hash of the folder's path, so repositories aren't touched. A lock file
//! stops two instances from saving the same workspace over each other.
use bedtime_workbench::{ErrorDialog, persistence::StoreBackend, request_error_dialog};
use bevy::prelude::*;
use std::{
    fs::{self, File, TryLockError},
    path::{Path, PathBuf},
};

const RECENT_ENTRY: &str = "recent";
const RECENT_LIMIT: usize = 10;

/// The open folder.
#[derive(Resource, Clone, Debug)]
pub struct Workspace {
    pub root: PathBuf,
}

impl Workspace {
    /// `argument` as a folder. A path that isn't one is reported and ignored.
    pub fn from_path(argument: PathBuf) -> Option<Self> {
        match fs::canonicalize(&argument) {
            Ok(root) if root.is_dir() => Some(Self { root }),
            Ok(root) => {
                eprintln!("{} isn't a folder", root.display());
                None
            }
            Err(error) => {
                eprintln!("{}: {error}", argument.display());
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

/// Recently opened folders, most recent first. Kept in the global store's
/// directory as its own entry, so it can be read before the app is built.
#[derive(Resource, Clone, Debug, Default, PartialEq)]
pub struct RecentWorkspaces(pub Vec<PathBuf>);

impl RecentWorkspaces {
    pub fn read(store: &dyn StoreBackend) -> Self {
        let read = || -> Option<Vec<PathBuf>> {
            let bytes = store.read(RECENT_ENTRY).ok()??;
            ron::de::from_bytes(&bytes).ok()
        };
        Self(read().unwrap_or_default())
    }

    /// Move `root` to the front and save the list.
    pub fn remember(&mut self, store: &dyn StoreBackend, root: &Path) -> Result<(), String> {
        self.0.retain(|path| path != root);
        self.0.insert(0, root.to_owned());
        self.0.truncate(RECENT_LIMIT);
        let text = ron::ser::to_string_pretty(&self.0, default()).map_err(|e| e.to_string())?;
        store.write(RECENT_ENTRY, text.as_bytes())
    }
}

/// The workspace this instance opened, and how it went.
pub struct Opened {
    pub workspace: Option<Workspace>,
    pub lock: Option<WorkspaceLock>,
    /// Why the workspace's store wasn't claimed, to show once the window exists.
    pub unclaimed: Option<String>,
}

/// Decide which workspace to open: the folder named on the command line, else
/// (if `reopen_last`) the most recent one that still exists. A reopened
/// workspace that's open elsewhere is skipped rather than shared.
pub fn open(
    argument: Option<PathBuf>,
    config: Option<&Path>,
    recent: &RecentWorkspaces,
    reopen_last: bool,
) -> Opened {
    let named = argument.is_some();
    let workspace = match argument {
        Some(argument) => Workspace::from_path(argument),
        None if reopen_last => recent
            .0
            .iter()
            .find(|path| path.is_dir())
            .map(|root| Workspace { root: root.clone() }),
        None => None,
    };
    let (Some(workspace), Some(config)) = (workspace.clone(), config) else {
        return Opened {
            workspace,
            lock: None,
            unclaimed: None,
        };
    };
    let directory = workspace.store_directory(config);
    let unclaimed = match lock(&directory, &workspace) {
        Ok(Some(lock)) => {
            return Opened {
                workspace: Some(workspace),
                lock: Some(lock),
                unclaimed: None,
            };
        }
        Ok(None) => format!("{} is in use by another instance", directory.display()),
        Err(error) => error,
    };
    if named {
        Opened {
            workspace: Some(workspace),
            lock: None,
            unclaimed: Some(unclaimed),
        }
    } else {
        Opened {
            workspace: None,
            lock: None,
            unclaimed: None,
        }
    }
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

    fn scratch(name: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!("bed-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        fs::canonicalize(directory).unwrap()
    }

    #[test]
    fn recent_folders_are_most_recent_first_without_duplicates() {
        let directory = scratch("recent");
        let store = bedtime_workbench::persistence::DirectoryStore::new(&directory);
        let mut recent = RecentWorkspaces::read(&store);
        assert!(recent.0.is_empty());
        for name in ["a", "b", "a"] {
            recent.remember(&store, Path::new(name)).unwrap();
        }
        assert_eq!(recent.0, [PathBuf::from("a"), PathBuf::from("b")]);
        for index in 0..20 {
            recent
                .remember(&store, Path::new(&index.to_string()))
                .unwrap();
        }
        let reread = RecentWorkspaces::read(&store);
        assert_eq!(reread.0.len(), RECENT_LIMIT);
        assert_eq!(reread.0[0], PathBuf::from("19"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn without_a_folder_the_last_one_reopens_unless_it_is_open_elsewhere() {
        let root = scratch("reopen");
        let (config, project) = (root.join("config"), root.join("project"));
        fs::create_dir_all(&project).unwrap();
        let recent = RecentWorkspaces(vec![root.join("deleted"), project.clone()]);

        let opened = open(None, Some(&config), &recent, false);
        assert!(opened.workspace.is_none());

        let first = open(None, Some(&config), &recent, true);
        assert_eq!(first.workspace.as_ref().unwrap().root, project);
        assert!(first.lock.is_some());

        // Open in another instance: start without a folder instead.
        let second = open(None, Some(&config), &recent, true);
        assert!(second.workspace.is_none() && second.unclaimed.is_none());
        // Named explicitly, it opens anyway, unclaimed.
        let named = open(Some(project.clone()), Some(&config), &recent, true);
        assert!(named.workspace.is_some() && named.unclaimed.is_some());

        drop(first);
        fs::remove_dir_all(root).unwrap();
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
