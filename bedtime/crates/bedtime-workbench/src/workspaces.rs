//! Workspaces (`workspaces` feature): a folder, and a store for its layout and
//! settings.
//!
//! There's always one: the folder the app was opened at, or the default
//! workspace, rooted at the home folder, for using the app without a project.
//! Both are saved the same way. Switching workspace ends the session like
//! quitting does, then rebuilds the App in the same window
//! (`bevy::winit::relaunch`, from the bevy_winit fork), so nothing from the
//! previous workspace survives.
//!
//! The config directory holds `global/` (the `GlobalStore`: app-wide settings
//! and data, such as the recent list) and `workspaces/<hash>/` (each
//! workspace's `WorkbenchStore`), keyed by a stable hash of the folder's path
//! so repositories aren't touched. A lock file stops two instances from saving
//! the same workspace over each other.
use crate::{
    ErrorDialog, ErrorDialogWorldExt, Notification, Root, WorkbenchWorldExt,
    persistence::{
        DirectoryStore, Persisted, PersistenceAppExt, Store, StoreBackend, WorkbenchStore,
        read_persisted, write_persisted,
    },
    settings::{Setting, SettingsAppExt, read_setting, settings},
};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, TryLockError},
    path::{Path, PathBuf},
    sync::Mutex,
};

/// App-wide settings and data, shared by every workspace: `<config>/global`.
pub struct GlobalStore;
impl Store for GlobalStore {}

settings! {
    "Startup" {
        /// Started without a folder, reopen the last one rather than the
        /// default workspace.
        pub ReopenLastWorkspace: bool {
            default: false,
            key: "startup.reopen_last",
            label: "Reopen last folder",
            description: "When started without a folder, open the last one instead of the default workspace",
            only_in: [GlobalStore],
        }
    }
}

const RECENT_LIMIT: usize = 10;

/// The open workspace.
#[derive(Resource, Clone, Debug, PartialEq)]
pub struct Workspace {
    pub root: PathBuf,
    /// The default workspace, rather than a folder the app was opened at. Features
    /// that need a project (git, say) can step back here.
    pub default: bool,
}

impl Workspace {
    /// The default workspace, rooted at the home folder.
    pub fn default_workspace() -> Self {
        let home = std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from);
        Self {
            root: home,
            default: true,
        }
    }

    /// `argument` as a folder workspace. A path that isn't a folder is
    /// reported and ignored.
    pub fn from_path(argument: PathBuf) -> Result<Self, String> {
        match fs::canonicalize(&argument) {
            Ok(root) if root.is_dir() => Ok(Self {
                root,
                default: false,
            }),
            Ok(root) => Err(format!("{} isn't a folder", root.display())),
            Err(error) => Err(format!("{}: {error}", argument.display())),
        }
    }

    /// For a window title or a settings panel.
    pub fn name(&self) -> String {
        if self.default {
            return "Default workspace".into();
        }
        self.root.file_name().map_or_else(
            || self.root.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        )
    }

    /// Where this workspace's store lives.
    pub fn store_directory(&self, config: &Path) -> PathBuf {
        let key = if self.default {
            "default".to_owned()
        } else {
            format!(
                "{:016x}",
                stable_hash(self.root.as_os_str().as_encoded_bytes())
            )
        };
        config.join("workspaces").join(key)
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

/// Recently opened folders, most recent first. Persisted in the global store,
/// and read from it directly before the app is built.
#[derive(Resource, Clone, Debug, Default, PartialEq, Serialize, Deserialize, Persisted)]
#[persisted(key = "recent")]
#[serde(transparent)]
pub struct RecentWorkspaces(pub Vec<PathBuf>);

impl RecentWorkspaces {
    pub fn read(store: &dyn StoreBackend) -> Self {
        read_persisted(store).ok().flatten().unwrap_or_default()
    }

    /// Move `root` to the front and save the list straight away (this happens
    /// before there's an App to save it).
    pub fn remember(&mut self, store: &dyn StoreBackend, root: &Path) -> Result<(), String> {
        self.0.retain(|path| path != root);
        self.0.insert(0, root.to_owned());
        self.0.truncate(RECENT_LIMIT);
        write_persisted(store, self)
    }

    /// Take `root` off the list; it's saved with the rest of the app.
    pub fn forget(&mut self, root: &Path) {
        self.0.retain(|path| path != root);
    }
}

/// Delete `workspace`'s saved layout and settings (never the folder itself),
/// unless another instance has it open. Its lock is held while deleting, so no
/// instance opens it halfway through.
pub fn delete(workspace: &Workspace, config: &Path) -> Result<(), String> {
    let directory = workspace.store_directory(config);
    if !directory.exists() {
        return Ok(());
    }
    let _lock = lock(&directory, workspace)?
        .ok_or_else(|| format!("{} is open in another window.", workspace.name()))?;
    fs::remove_dir_all(&directory).map_err(|error| format!("{}: {error}", directory.display()))
}

/// The workspace this instance opened, and how it went.
pub struct Opened {
    pub workspace: Workspace,
    pub lock: Option<WorkspaceLock>,
    /// Why the workspace's store wasn't claimed, to show once the window exists.
    pub unclaimed: Option<String>,
}

/// Which workspace an App opens.
#[derive(Clone, Debug)]
pub enum Start {
    /// As started from the command line: the folder named there, else (if
    /// `startup.reopen_last`) the most recent folder, else the default.
    CommandLine(Option<PathBuf>),
    /// A folder, switched to from another workspace.
    Folder(PathBuf),
    /// The default workspace.
    Default,
}

/// Opens a workspace for this App and saves it there. Built before the App,
/// since the window's title usually needs the workspace:
///
/// ```ignore
/// let workspaces = WorkspacesPlugin::new(start, config_directory, app);
/// let title = workspaces.workspace().name();
/// app.add_plugins(workspaces);
/// ```
///
/// It adds the global store and settings layer, then (for a folder) the
/// workspace's settings layer and, when it's claimed, its store; and the
/// `Workspace`, `Root` and `RecentWorkspaces` resources.
pub struct WorkspacesPlugin {
    opened: Mutex<Option<Opened>>,
    workspace: Workspace,
    settings: WorkspaceSettings,
    recent: RecentWorkspaces,
    /// Couldn't update the recent list.
    remembered: Result<(), String>,
}

/// Where workspaces are kept, and how to build the App for another one.
#[derive(Resource, Clone)]
pub struct WorkspaceSettings {
    /// The config directory; `None` saves nothing.
    pub config: Option<PathBuf>,
    /// Builds the App for another workspace, when switching.
    pub app: fn(Start) -> App,
}

impl WorkspacesPlugin {
    /// Open `start`'s workspace: read what the global store remembers (recent
    /// folders, whether to reopen the last), choose and claim the workspace,
    /// and put it first on the recent list.
    pub fn new(start: Start, config: Option<PathBuf>, app: fn(Start) -> App) -> Self {
        let global = config
            .as_ref()
            .map(|config| DirectoryStore::new(config.join("global")));
        let mut recent = RecentWorkspaces::default();
        let mut reopen_last = <ReopenLastWorkspace as Setting>::default();
        if let Some(global) = &global {
            recent = RecentWorkspaces::read(global);
            reopen_last = read_setting::<ReopenLastWorkspace>(global).unwrap_or(reopen_last);
        }
        let opened = choose(start, config.as_deref(), &recent, reopen_last);
        let remembered = match &global {
            Some(global) if !opened.workspace.default => {
                recent.remember(global, &opened.workspace.root)
            }
            _ => Ok(()),
        };
        Self {
            workspace: opened.workspace.clone(),
            opened: Mutex::new(Some(opened)),
            settings: WorkspaceSettings { config, app },
            recent,
            remembered,
        }
    }

    /// The workspace this App opens.
    pub fn workspace(&self) -> &Workspace {
        &self.workspace
    }
}

impl Plugin for WorkspacesPlugin {
    fn build(&self, app: &mut App) {
        let Opened {
            workspace,
            lock,
            unclaimed,
        } = self
            .opened
            .lock()
            .unwrap()
            .take()
            .expect("WorkspacesPlugin is added once");
        let config = self.settings.config.clone();
        if let Some(config) = &config {
            app.add_store::<GlobalStore>(DirectoryStore::new(config.join("global")));
        }
        app.add_settings_layer::<GlobalStore>()
            .register_setting::<ReopenLastWorkspace>()
            .persist::<RecentWorkspaces, GlobalStore>();
        // A folder's settings override global ones. Without a claimed store
        // they still apply, but only in memory. The default workspace has none:
        // it's just where the app is without a project.
        if !workspace.default {
            app.add_settings_layer::<WorkbenchStore>()
                .label_settings_layer::<WorkbenchStore>("Workspace");
        }
        if let (Some(lock), Some(config)) = (lock, &config) {
            app.add_store::<WorkbenchStore>(DirectoryStore::new(workspace.store_directory(config)))
                .insert_resource(lock);
        }
        if let Some(unclaimed) = unclaimed {
            app.insert_resource(Unclaimed(unclaimed));
        }
        app.insert_resource(Root(workspace.root.clone()))
            .insert_resource(workspace)
            .insert_resource(self.recent.clone())
            .insert_resource(self.settings.clone())
            .add_systems(Update, report_unclaimed);
        if let Err(error) = &self.remembered {
            app.world_mut()
                .workbench_mut()
                .notify(Notification::error(format!(
                    "Couldn't update recent folders: {error}"
                )));
        }
    }
}

/// Switch this window to `start`'s workspace. This ends the session the way
/// quitting does (unsaved documents are saved or asked about, and the user can
/// cancel), then rebuilds the App in the same window. A folder another instance
/// has open is refused rather than opened unsaved; the default workspace opens
/// unsaved, as in any other window.
pub fn switch(world: &mut World, start: Start) -> Result<(), String> {
    let current = world.resource::<Workspace>();
    let next = match &start {
        Start::Folder(path) => Workspace::from_path(path.clone())?,
        Start::Default => Workspace::default_workspace(),
        Start::CommandLine(_) => return Err("Can't switch to the command line's workspace".into()),
    };
    if next == *current {
        return Ok(());
    }
    let settings = world.resource::<WorkspaceSettings>().clone();
    if !next.default
        && let Some(config) = &settings.config
        && !available(&next.store_directory(config))
    {
        return Err(format!("{} is open in another window.", next.name()));
    }
    world
        .workbench_mut()
        .request_exit("Switch workspace", move |world| {
            bevy::winit::relaunch(world, move || (settings.app)(start));
        })
}

/// Decide which workspace to open and claim its store. A folder named
/// explicitly opens even if another instance has it, but isn't saved; one
/// only reopened from the recent list is skipped instead.
pub fn choose(
    start: Start,
    config: Option<&Path>,
    recent: &RecentWorkspaces,
    reopen_last: bool,
) -> Opened {
    let named = |path: PathBuf| match Workspace::from_path(path) {
        Ok(workspace) => Some(workspace),
        Err(error) => {
            eprintln!("{error}");
            None
        }
    };
    let (chosen, explicit) = match start {
        Start::Folder(path) | Start::CommandLine(Some(path)) => (named(path), true),
        Start::CommandLine(None) if reopen_last => (
            recent
                .0
                .iter()
                .filter(|path| path.is_dir())
                .find(|path| {
                    config.is_none_or(|config| {
                        let workspace = Workspace {
                            root: path.to_path_buf(),
                            default: false,
                        };
                        available(&workspace.store_directory(config))
                    })
                })
                .map(|root| Workspace {
                    root: root.clone(),
                    default: false,
                }),
            false,
        ),
        Start::CommandLine(None) | Start::Default => (None, false),
    };
    let workspace = chosen.unwrap_or_else(Workspace::default_workspace);
    let Some(config) = config else {
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
                workspace,
                lock: Some(lock),
                unclaimed: None,
            };
        }
        Ok(None) => format!("{} is in use by another instance", directory.display()),
        Err(error) => error,
    };
    // A second window on the default workspace is expected (more terminals);
    // it just isn't saved. Only a folder asked for by name is worth a dialog.
    Opened {
        unclaimed: (explicit && !workspace.default).then_some(unclaimed),
        workspace,
        lock: None,
    }
}

/// Whether no other instance has the workspace whose store is `directory`.
pub fn available(directory: &Path) -> bool {
    let Ok(file) = File::options().write(true).open(directory.join("lock")) else {
        // No lock file: never opened, so not open now.
        return true;
    };
    match file.try_lock() {
        Ok(()) => true,
        Err(TryLockError::WouldBlock) => false,
        Err(TryLockError::Error(_)) => true,
    }
}

/// Shown when the workspace couldn't be claimed, once the window exists.
#[derive(Resource)]
struct Unclaimed(pub String);

fn report_unclaimed(world: &mut World) {
    if let Some(Unclaimed(details)) = world.remove_resource::<Unclaimed>() {
        world.request_error_dialog(ErrorDialog {
            title: "Layout won't be saved".into(),
            message: "This folder is already open in another window, or its saved \
                          layout couldn't be claimed. Changes to this window's layout and \
                          workspace settings won't be saved."
                .into(),
            details: Some(details),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_maps_to_the_same_store_directory_every_time() {
        let workspace = Workspace {
            root: PathBuf::from("/projects/bed"),
            default: false,
        };
        let config = Path::new("/config");
        assert_eq!(
            workspace.store_directory(config),
            config.join("workspaces/3d7337650bbf4540")
        );
    }

    fn scratch(name: &str) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("bedtime-workspace-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        fs::canonicalize(directory).unwrap()
    }

    #[test]
    fn recent_folders_are_most_recent_first_without_duplicates() {
        let directory = scratch("recent");
        let store = crate::persistence::DirectoryStore::new(&directory);
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
    fn the_command_line_reopens_the_last_folder_or_opens_the_default() {
        let root = scratch("reopen");
        let (config, project) = (root.join("config"), root.join("project"));
        fs::create_dir_all(&project).unwrap();
        let recent = RecentWorkspaces(vec![root.join("deleted"), project.clone()]);
        let open = |start, reopen| choose(start, Some(&config), &recent, reopen);

        let default = open(Start::CommandLine(None), false);
        assert!(default.workspace.default && default.lock.is_some());
        assert_eq!(
            default.workspace.store_directory(&config),
            config.join("workspaces/default")
        );
        // Another window on the default workspace runs unsaved, quietly.
        let second = open(Start::Default, false);
        assert!(second.workspace.default && second.lock.is_none());
        assert!(second.unclaimed.is_none());
        drop(default);

        let first = open(Start::CommandLine(None), true);
        assert_eq!(first.workspace.root, project);
        assert!(!first.workspace.default && first.lock.is_some());
        // Open in another instance: reopening skips it for the default.
        let skipped = open(Start::CommandLine(None), true);
        assert!(skipped.workspace.default);
        // Asked for by name, it opens anyway, unsaved, and says so.
        let named = open(Start::Folder(project.clone()), true);
        assert_eq!(named.workspace.root, project);
        assert!(named.lock.is_none() && named.unclaimed.is_some());
        assert!(!available(&first.workspace.store_directory(&config)));
        drop(first);
        assert!(available(&named.workspace.store_directory(&config)));

        // A path that isn't a folder falls back to the default workspace.
        let missing = open(Start::Folder(root.join("deleted")), false);
        assert!(missing.workspace.default);
        drop((skipped, missing));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn deleting_a_workspace_removes_its_store_but_not_the_folder() {
        let root = scratch("delete");
        let (config, project) = (root.join("config"), root.join("project"));
        fs::create_dir_all(&project).unwrap();
        let recent = RecentWorkspaces::default();
        let opened = choose(
            Start::Folder(project.clone()),
            Some(&config),
            &recent,
            false,
        );
        let directory = opened.workspace.store_directory(&config);
        assert!(directory.is_dir());

        // Open (here or in another window): refused.
        assert!(delete(&opened.workspace, &config).is_err());
        assert!(directory.is_dir());
        drop(opened.lock);

        delete(&opened.workspace, &config).unwrap();
        assert!(!directory.exists());
        assert!(project.is_dir());
        // Already gone is fine.
        delete(&opened.workspace, &config).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_second_claim_on_the_same_workspace_is_refused() {
        let directory =
            std::env::temp_dir().join(format!("bedtime-workspace-lock-{}", std::process::id()));
        let workspace = Workspace {
            root: PathBuf::from("/projects/bed"),
            default: false,
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
