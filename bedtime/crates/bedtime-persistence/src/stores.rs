//! Stores, their backends, and the entries saved in them.
use crate::{CaptureHook, Persisted, PersistenceWorldExt, RestoreCheck, RestoreHook, world::Hooks};
use bevy::prelude::*;
use std::{
    any::{TypeId, type_name},
    collections::HashMap,
    fmt, fs,
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const SAVE_INTERVAL: Duration = Duration::from_secs(1);
/// The world snapshot's entry.
pub const WORLD_ENTRY: &str = "world";

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

/// Where a store's bytes live, as named entries. Methods may block; they're
/// called on the main thread for now.
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

/// Entries kept in memory; clones share them. For tests, and apps that save
/// somewhere of their own.
#[derive(Clone, Default)]
pub struct MemoryStore(Arc<Mutex<MemoryEntries>>);

#[derive(Default)]
struct MemoryEntries {
    entries: HashMap<String, Vec<u8>>,
    writes: usize,
    versions: u64,
}

impl MemoryStore {
    /// The entry as text, if it exists.
    pub fn text(&self, entry: &str) -> Option<String> {
        let entries = self.0.lock().unwrap();
        let bytes = entries.entries.get(entry)?;
        Some(String::from_utf8_lossy(bytes).into_owned())
    }

    /// How many writes there have been.
    pub fn writes(&self) -> usize {
        self.0.lock().unwrap().writes
    }
}

impl StoreBackend for MemoryStore {
    fn read(&self, entry: &str) -> Result<Option<Vec<u8>>, String> {
        Ok(self.0.lock().unwrap().entries.get(entry).cloned())
    }

    fn write(&self, entry: &str, bytes: &[u8]) -> Result<(), String> {
        let mut entries = self.0.lock().unwrap();
        entries.entries.insert(entry.to_owned(), bytes.to_vec());
        entries.writes += 1;
        entries.versions += 1;
        Ok(())
    }

    fn version(&self, entry: &str) -> Result<Option<String>, String> {
        let entries = self.0.lock().unwrap();
        Ok(entries
            .entries
            .contains_key(entry)
            .then(|| format!("{}:{}", entry, entries.versions)))
    }
}

/// A store's entry couldn't be loaded or saved.
#[derive(Message, Clone, Debug, PartialEq, Eq)]
pub struct StoreError {
    pub store: StoreId,
    pub entry: String,
    pub kind: StoreErrorKind,
    pub error: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreErrorKind {
    /// Saved data couldn't be read or restored, so it won't be saved over.
    Load,
    /// Saving failed; it's retried.
    Save,
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            StoreErrorKind::Load => write!(
                f,
                "Couldn't restore saved {}, so it won't be saved over ({})",
                self.entry, self.error
            ),
            StoreErrorKind::Save => {
                write!(f, "Couldn't save {}; retrying ({})", self.entry, self.error)
            }
        }
    }
}

/// The stores the app added, what's saved in them, and how it's going.
#[derive(Resource, Default)]
pub struct Stores {
    backends: Vec<(StoreId, Arc<dyn StoreBackend>)>,
    entries: Vec<Entry>,
}

/// An entry's bytes, or `None` when there's nothing to save.
type SaveEntry = fn(&World) -> Option<Result<Vec<u8>, String>>;
type LoadEntry = fn(&mut World, &[u8]) -> Result<(), String>;

struct Entry {
    store: StoreId,
    name: &'static str,
    save: SaveEntry,
    load: LoadEntry,
    /// What the backend holds, as far as we know.
    written: Option<Vec<u8>>,
    /// Why this entry can't be saved (its data couldn't be read or restored).
    blocked: Option<String>,
    /// The last save error, reported once.
    error: Option<String>,
}

impl Stores {
    /// Store `store`'s backend, if it was added.
    pub fn backend(&self, store: StoreId) -> Option<Arc<dyn StoreBackend>> {
        self.backends
            .iter()
            .find(|(id, _)| *id == store)
            .map(|(_, backend)| backend.clone())
    }

    /// The store the world snapshot is saved in, if any.
    pub fn world_store(&self) -> Option<StoreId> {
        self.entries
            .iter()
            .find(|entry| entry.name == WORLD_ENTRY)
            .map(|entry| entry.store)
    }

    /// Why something in store `S` isn't being saved, or why its last save failed.
    pub fn error<S: Store>(&self) -> Option<&str> {
        self.error_in(StoreId::of::<S>())
    }

    /// As `error`, for a store known by its id.
    pub fn error_in(&self, store: StoreId) -> Option<&str> {
        self.entries
            .iter()
            .filter(|entry| entry.store == store)
            .find_map(|entry| entry.blocked.as_deref().or(entry.error.as_deref()))
    }

    fn add_entry(&mut self, store: StoreId, name: &'static str, save: SaveEntry, load: LoadEntry) {
        assert!(
            self.entries
                .iter()
                .all(|entry| !(entry.store == store && entry.name == name)),
            "{name} is already saved in {}",
            store.name()
        );
        self.entries.push(Entry {
            store,
            name,
            save,
            load,
            written: None,
            blocked: None,
            error: None,
        });
    }
}

/// Loading runs in `PreStartup`, saving in `Last`.
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StoreSystems {
    Load,
    Save,
}

pub trait PersistenceAppExt {
    /// Save store `S` through `backend`. Without this, `S` isn't saved.
    fn add_store<S: Store>(&mut self, backend: impl StoreBackend) -> &mut Self;
    /// Save the world snapshot (everything marked `Persist`) in store `S`.
    /// Only one store holds it.
    fn persist_world<S: Store>(&mut self) -> &mut Self;
    /// Save resource `T` as its own entry in store `S`.
    fn persist<T: Persisted, S: Store>(&mut self) -> &mut Self;
    /// Edit the world snapshot before it's saved.
    fn on_capture(&mut self, hook: CaptureHook) -> &mut Self;
    /// Refuse a world snapshot before anything from it is written.
    fn check_restore(&mut self, check: RestoreCheck) -> &mut Self;
    /// Fix things up after a snapshot's entities are written; an error undoes
    /// them.
    fn on_restore(&mut self, hook: RestoreHook) -> &mut Self;
}

impl PersistenceAppExt for App {
    fn add_store<S: Store>(&mut self, backend: impl StoreBackend) -> &mut Self {
        let mut stores = self.world_mut().get_resource_or_init::<Stores>();
        assert!(
            stores.backend(StoreId::of::<S>()).is_none(),
            "{} was added twice",
            type_name::<S>()
        );
        stores
            .backends
            .push((StoreId::of::<S>(), Arc::new(backend)));
        self
    }

    fn persist_world<S: Store>(&mut self) -> &mut Self {
        let mut stores = self.world_mut().get_resource_or_init::<Stores>();
        if let Some(store) = stores.world_store() {
            panic!(
                "The world is already saved in {}, so it can't be in {}",
                store.name(),
                type_name::<S>()
            );
        }
        stores.add_entry(
            StoreId::of::<S>(),
            WORLD_ENTRY,
            |world| Some(world.persistence().capture().map(String::into_bytes)),
            |world, bytes| {
                let text = std::str::from_utf8(bytes).map_err(|e| e.to_string())?;
                world.persistence_mut().restore(text)
            },
        );
        self
    }

    fn persist<T: Persisted, S: Store>(&mut self) -> &mut Self {
        self.world_mut().get_resource_or_init::<Stores>().add_entry(
            StoreId::of::<S>(),
            T::KEY,
            |world| world.get_resource::<T>().map(crate::resources::to_bytes),
            |world, bytes| {
                let value = crate::resources::from_bytes::<T>(bytes)?;
                world.insert_resource(value);
                Ok(())
            },
        );
        self
    }

    fn on_capture(&mut self, hook: CaptureHook) -> &mut Self {
        hooks(self).capture.push(hook);
        self
    }

    fn check_restore(&mut self, check: RestoreCheck) -> &mut Self {
        hooks(self).check.push(check);
        self
    }

    fn on_restore(&mut self, hook: RestoreHook) -> &mut Self {
        hooks(self).restore.push(hook);
        self
    }
}

fn hooks(app: &mut App) -> Mut<'_, Hooks> {
    app.world_mut().get_resource_or_init::<Hooks>()
}

/// Read each entry from its store and load it. A failure blocks the entry.
pub(crate) fn load(world: &mut World) {
    let count = world.resource::<Stores>().entries.len();
    for index in 0..count {
        let stores = world.resource::<Stores>();
        let entry = &stores.entries[index];
        let (store, name, load) = (entry.store, entry.name, entry.load);
        let Some(backend) = stores.backend(store) else {
            continue;
        };
        let result = backend.read(name).and_then(|bytes| {
            let Some(bytes) = bytes else {
                return Ok(None);
            };
            load(world, &bytes)?;
            Ok(Some(bytes))
        });
        let entry = &mut world.resource_mut::<Stores>().entries[index];
        match result {
            Ok(written) => entry.written = written,
            Err(error) => {
                entry.blocked = Some(error.clone());
                world.write_message(StoreError {
                    store,
                    entry: name.into(),
                    kind: StoreErrorKind::Load,
                    error,
                });
            }
        }
    }
}

/// Save each entry that changed, about once a second and when the app exits.
pub(crate) fn save(world: &mut World, mut last: Local<Option<Instant>>) {
    let exiting = world
        .get_resource::<Messages<AppExit>>()
        .is_some_and(|messages| !messages.is_empty());
    if !exiting && last.is_some_and(|at| at.elapsed() < SAVE_INTERVAL) {
        return;
    }
    *last = Some(Instant::now());
    let count = world.resource::<Stores>().entries.len();
    for index in 0..count {
        let stores = world.resource::<Stores>();
        let entry = &stores.entries[index];
        let (store, name, save) = (entry.store, entry.name, entry.save);
        let Some(backend) = stores.backend(store) else {
            continue;
        };
        if entry.blocked.is_some() {
            continue;
        }
        let Some(result) = save(world) else {
            continue;
        };
        let written = &world.resource::<Stores>().entries[index].written;
        let result = result.and_then(|bytes| {
            if written.as_ref() != Some(&bytes) {
                backend.write(name, &bytes)?;
            }
            Ok(bytes)
        });
        let entry = &mut world.resource_mut::<Stores>().entries[index];
        match result {
            Ok(bytes) => {
                entry.written = Some(bytes);
                entry.error = None;
            }
            Err(error) if entry.error.as_ref() != Some(&error) => {
                entry.error = Some(error.clone());
                world.write_message(StoreError {
                    store,
                    entry: name.into(),
                    kind: StoreErrorKind::Save,
                    error,
                });
            }
            Err(_) => {}
        }
    }
}
