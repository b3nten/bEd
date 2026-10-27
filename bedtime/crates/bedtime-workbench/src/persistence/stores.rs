//! Stores: named places persisted data is saved. Workbench defines one,
//! `WorkbenchStore`; applications add their own and decide where each lives
//! by giving it a backend.
//!
//! A store's world data is read and restored once, before `Startup`, and saved
//! about once a second while it changes, and on exit. Change detection is a
//! comparison with what was last written. A store whose saved data couldn't be
//! read or restored is never written, so a damaged file isn't overwritten.
use super::*;
use std::{
    any::TypeId,
    collections::HashMap,
    fs,
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

const SAVE_INTERVAL: Duration = Duration::from_secs(1);
const WORLD_ENTRY: &str = "world";

/// A store's identity. Implemented by empty marker types.
pub trait Store: Send + Sync + 'static {}

/// A store type as a value, e.g. for saying which settings layer to change.
#[derive(Clone, Copy, Debug)]
pub struct StoreId {
    id: TypeId,
    name: &'static str,
}

impl StoreId {
    pub fn of<S: Store>() -> Self {
        Self {
            id: TypeId::of::<S>(),
            name: type_name::<S>(),
        }
    }
    pub fn name(&self) -> &'static str {
        self.name
    }
}

impl PartialEq for StoreId {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}
impl Eq for StoreId {}

/// The workbench's own store: layouts, areas, panels and their persisted
/// components, and persisted resources not redirected elsewhere.
pub struct WorkbenchStore;
impl Store for WorkbenchStore {}

/// Where a store's bytes live, as named entries ("world", later "settings").
/// Methods may block; they're called on the main thread for now.
pub trait StoreBackend: Send + Sync + 'static {
    /// The entry's contents, or `None` if it doesn't exist yet.
    fn read(&self, entry: &str) -> Result<Option<Vec<u8>>, String>;
    fn write(&self, entry: &str, bytes: &[u8]) -> Result<(), String>;
    /// A cheap token that changes when the entry changes, if the backend can tell.
    fn version(&self, _entry: &str) -> Result<Option<String>, String> {
        Ok(None)
    }
}

/// One `<entry>.ron` file per entry in a directory, written atomically.
pub struct DirectoryStore {
    directory: PathBuf,
}

impl DirectoryStore {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
        }
    }

    fn path(&self, entry: &str) -> PathBuf {
        self.directory.join(format!("{entry}.ron"))
    }
}

impl StoreBackend for DirectoryStore {
    fn read(&self, entry: &str) -> Result<Option<Vec<u8>>, String> {
        let path = self.path(entry);
        match fs::read(&path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    fn write(&self, entry: &str, bytes: &[u8]) -> Result<(), String> {
        atomic_write(&self.path(entry), bytes)
    }

    fn version(&self, entry: &str) -> Result<Option<String>, String> {
        match fs::metadata(self.path(entry)).and_then(|m| m.modified()) {
            Ok(time) => Ok(Some(format!("{time:?}"))),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path.parent().ok_or("The store path has no directory")?;
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

/// Which store each persisted resource belongs in; `WorkbenchStore` unless redirected.
#[derive(Resource, Default)]
pub(crate) struct PersistRouting(HashMap<TypeId, TypeId>);

impl PersistRouting {
    pub(crate) fn store_of(&self, resource: TypeId) -> TypeId {
        self.0
            .get(&resource)
            .copied()
            .unwrap_or(TypeId::of::<WorkbenchStore>())
    }
}

/// The stores the app added, and how saving each one is going.
#[derive(Resource, Default)]
pub struct Stores(Vec<StoreState>);

struct StoreState {
    id: TypeId,
    name: &'static str,
    backend: Arc<dyn StoreBackend>,
    capture: fn(&mut World) -> Result<String, String>,
    restore: fn(&mut World, &str) -> Result<(), String>,
    /// What the backend holds, as far as we know.
    written: Option<Vec<u8>>,
    /// Why this store can't be saved (its data couldn't be read or restored).
    blocked: Option<String>,
    /// The last save error, reported once.
    error: Option<String>,
}

impl Stores {
    pub(crate) fn backend(&self, store: StoreId) -> Option<Arc<dyn StoreBackend>> {
        self.0
            .iter()
            .find(|state| state.id == store.id)
            .map(|state| state.backend.clone())
    }

    /// Why store `S` isn't being saved, or why its last save failed.
    pub fn error<S: Store>(&self) -> Option<&str> {
        self.0
            .iter()
            .find(|store| store.id == TypeId::of::<S>())
            .and_then(|store| store.blocked.as_deref().or(store.error.as_deref()))
    }
}

pub trait StoreAppExt {
    /// Save store `S` through `backend`. Without this, `S` isn't persisted.
    fn add_store<S: Store>(&mut self, backend: impl StoreBackend) -> &mut Self;
    /// Save resource `T` in store `S` instead of `WorkbenchStore`.
    fn persist_in<T: Resource, S: Store>(&mut self) -> &mut Self;
}

impl StoreAppExt for App {
    fn add_store<S: Store>(&mut self, backend: impl StoreBackend) -> &mut Self {
        let mut stores = self.world_mut().get_resource_or_init::<Stores>();
        assert!(
            stores.0.iter().all(|store| store.id != TypeId::of::<S>()),
            "{} was added twice",
            type_name::<S>()
        );
        stores.0.push(StoreState {
            id: TypeId::of::<S>(),
            name: type_name::<S>(),
            backend: Arc::new(backend),
            capture: capture::<S>,
            restore: restore::<S>,
            written: None,
            blocked: None,
            error: None,
        });
        self
    }

    fn persist_in<T: Resource, S: Store>(&mut self) -> &mut Self {
        self.world_mut()
            .get_resource_or_init::<PersistRouting>()
            .0
            .insert(TypeId::of::<T>(), TypeId::of::<S>());
        self
    }
}

/// Settings layers load before world data, so restored panels see final values.
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StoreSystems {
    LoadSettings,
    Load,
}

pub(super) fn install(app: &mut App) {
    app.configure_sets(
        PreStartup,
        StoreSystems::LoadSettings.before(StoreSystems::Load),
    );
    app.init_resource::<Stores>()
        .init_resource::<PersistRouting>()
        .add_systems(PreStartup, load_stores.in_set(StoreSystems::Load))
        .add_systems(Last, save_stores.before(SessionSystems::Close));
}

fn load_stores(world: &mut World) {
    let count = world.resource::<Stores>().0.len();
    for index in 0..count {
        let (backend, restore, name) = {
            let store = &world.resource::<Stores>().0[index];
            (store.backend.clone(), store.restore, store.name)
        };
        let result = backend.read(WORLD_ENTRY).and_then(|bytes| {
            let Some(bytes) = bytes else {
                return Ok(None);
            };
            let text = std::str::from_utf8(&bytes).map_err(|e| e.to_string())?;
            restore(world, text)?;
            Ok(Some(bytes))
        });
        let store = &mut world.resource_mut::<Stores>().0[index];
        match result {
            Ok(written) => store.written = written,
            Err(error) => {
                store.blocked = Some(error.clone());
                crate::closing::report_error(
                    world,
                    "Couldn't restore",
                    "Saved state couldn't be restored, so it won't be saved over.",
                    format!("{name}: {error}"),
                );
            }
        }
    }
}

fn save_stores(world: &mut World, mut last: Local<Option<Instant>>) {
    let exiting = world
        .get_resource::<Messages<AppExit>>()
        .is_some_and(|messages| !messages.is_empty());
    if !exiting && last.is_some_and(|at| at.elapsed() < SAVE_INTERVAL) {
        return;
    }
    *last = Some(Instant::now());
    let count = world.resource::<Stores>().0.len();
    for index in 0..count {
        let (backend, capture, blocked) = {
            let store = &world.resource::<Stores>().0[index];
            (
                store.backend.clone(),
                store.capture,
                store.blocked.is_some(),
            )
        };
        if blocked {
            continue;
        }
        let result = capture(world).and_then(|text| {
            let bytes = text.into_bytes();
            if world.resource::<Stores>().0[index].written.as_ref() != Some(&bytes) {
                backend.write(WORLD_ENTRY, &bytes)?;
            }
            Ok(bytes)
        });
        let mut stores = world.resource_mut::<Stores>();
        let store = &mut stores.0[index];
        match result {
            Ok(bytes) => {
                store.written = Some(bytes);
                store.error = None;
            }
            Err(error) if store.error.as_ref() != Some(&error) => {
                let name = store.name;
                store.error = Some(error.clone());
                crate::closing::report_error(
                    world,
                    "Couldn't save",
                    "Changes couldn't be saved. Saving will keep retrying.",
                    format!("{name}: {error}"),
                );
            }
            Err(_) => {}
        }
    }
}
