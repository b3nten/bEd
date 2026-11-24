//! Documents kept in the app's own stores: an entry of a persistence store
//! (`WorkbenchStore`, the open workspace's, say), in its model's byte form.
//! For what an app keeps of its own that people edit, such as notes or
//! saved requests: they get undo, and notice the entry being changed
//! elsewhere (by hand, or by another window), like any document.
//!
//! Every change is saved as it's made (`saves_every_change`), so they're
//! never unsaved. An entry not written yet is the model's default.
use super::*;
use crate::persistence::{Store, StoreBackend, StoreId, Stores};
use bedtime_document::{ByteModel, SourceChanged, SourceWatch};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
};

/// A store entry as a document source.
pub struct StoreSource<M> {
    id: SourceId,
    store: StoreId,
    entry: String,
    title: String,
    backend: Arc<dyn StoreBackend>,
    marker: PhantomData<fn() -> M>,
}

impl<M> StoreSource<M> {
    /// Entry `entry` of `store`, kept by `backend`. Sources of the same
    /// entry of the same backend are the same source.
    pub fn new(
        store: StoreId,
        backend: Arc<dyn StoreBackend>,
        entry: impl Into<String>,
        title: impl Into<String>,
    ) -> Self {
        static IDS: OnceLock<Mutex<HashMap<(usize, String), SourceId>>> = OnceLock::new();
        let entry = entry.into();
        let key = (Arc::as_ptr(&backend) as *const () as usize, entry.clone());
        let id = *IDS
            .get_or_init(Default::default)
            .lock()
            .unwrap()
            .entry(key)
            .or_default();
        Self {
            id,
            store,
            entry,
            title: title.into(),
            backend,
            marker: PhantomData,
        }
    }
}

fn version(backend: &dyn StoreBackend, entry: &str) -> Result<Option<SourceVersion>, SourceError> {
    backend
        .version(entry)
        .map(|version| version.map(SourceVersion))
        .map_err(SourceError::Failed)
}

impl<M: ByteModel + Default> DocumentSource<M> for StoreSource<M> {
    fn id(&self) -> SourceId {
        self.id
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn name(&self) -> Option<SourceName> {
        Some(SourceName::new(
            "store",
            &format!("{}/{}", self.store.name(), self.entry),
        ))
    }

    fn load(&self) -> Result<SourceLoad<M>, SourceError> {
        let version = version(&*self.backend, &self.entry)?;
        let model = match self
            .backend
            .read(&self.entry)
            .map_err(SourceError::Failed)?
        {
            Some(bytes) => M::decode(&bytes).map_err(SourceError::Failed)?,
            None => M::default(),
        };
        Ok(SourceLoad {
            model,
            version,
            saved: true,
        })
    }

    fn save(
        &self,
        model: M,
        expected: Option<SourceVersion>,
    ) -> Result<Option<SourceVersion>, SourceError> {
        if version(&*self.backend, &self.entry)? != expected {
            return Err(SourceError::Changed);
        }
        let mut bytes = Vec::new();
        model
            .encode(&mut bytes)
            .map_err(|e| SourceError::Failed(e.to_string()))?;
        self.backend
            .write(&self.entry, &bytes)
            .map_err(SourceError::Failed)?;
        version(&*self.backend, &self.entry)
    }

    fn version(&self) -> Result<Option<SourceVersion>, SourceError> {
        version(&*self.backend, &self.entry)
    }

    fn saves_every_change(&self) -> bool {
        true
    }

    /// Watched when the entry is a file (a directory store), with the
    /// filesystem's watcher.
    fn watch(&self, changed: SourceChanged) -> Option<SourceWatch> {
        #[cfg(feature = "files")]
        {
            let path = self.backend.file(&self.entry)?;
            // The store's folder may not be written yet (a new workspace).
            if let Some(folder) = path.parent() {
                let _ = std::fs::create_dir_all(folder);
            }
            let file = bedtime_filesystem::Filesystem::local()
                .new_document_source(path)
                .ok()?;
            let watch = file.watch_changes(changed)?;
            // The watch follows the file's source while it's kept.
            Some(SourceWatch::new((file, watch)))
        }
        #[cfg(not(feature = "files"))]
        {
            let _ = changed;
            None
        }
    }
}

/// Stores' entries for this run only, for stores not kept anywhere.
#[derive(Resource, Default)]
struct InMemory(Vec<(StoreId, Arc<dyn StoreBackend>)>);

impl Bedtime {
    /// The document kept in entry `entry` of store `S`, titled `title`:
    /// opened if it isn't (the same document each time, while `S` is kept
    /// in the same place). If `S` isn't kept anywhere (no persistence
    /// directory), in memory for this run.
    pub fn store_document<M: ByteModel + Default, S: Store>(
        &mut self,
        entry: &str,
        title: &str,
    ) -> Result<DocumentHandle<M>, DocumentError> {
        let store = StoreId::of::<S>();
        let kept = self
            .world
            .get_resource::<Stores>()
            .and_then(|stores| stores.backend(store));
        let backend = match kept {
            Some(backend) => backend,
            None => {
                let mut memory = self.world.get_resource_or_init::<InMemory>();
                match memory.0.iter().find(|(kept, _)| *kept == store) {
                    Some((_, backend)) => backend.clone(),
                    None => {
                        let backend: Arc<dyn StoreBackend> =
                            Arc::new(crate::persistence::MemoryStore::default());
                        memory.0.push((store, backend.clone()));
                        backend
                    }
                }
            }
        };
        self.load_document(StoreSource::<M>::new(store, backend, entry, title))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::documents::tests::update_until;
    use crate::persistence::{MemoryStore, PersistenceAppExt, WorkbenchStore};
    use crate::*;

    fn session(store: &MemoryStore) -> App {
        let mut app = App::new();
        app.add_plugins(WorkbenchCorePlugins)
            .add_store::<WorkbenchStore>(store.clone());
        app.update();
        app
    }

    fn ready(app: &mut App) -> DocumentHandle<TextBuffer> {
        let document = app
            .world_mut()
            .bedtime_mut()
            .store_document::<TextBuffer, WorkbenchStore>("scratch", "Scratch")
            .unwrap();
        update_until(app, |world| {
            world
                .bedtime()
                .inspect_document(document.id())
                .is_ok_and(|info| info.status == DocumentStatus::Ready)
        });
        document
    }

    #[test]
    fn every_change_is_saved_to_the_entry_and_undo_works() {
        let store = MemoryStore::default();
        let mut app = session(&store);
        let document = ready(&mut app);
        // The same document each time.
        assert_eq!(
            app.world_mut()
                .bedtime_mut()
                .store_document::<TextBuffer, WorkbenchStore>("scratch", "Scratch")
                .unwrap(),
            document
        );
        crate::documents::tests::type_text(app.world_mut(), document, 0, "hello");
        let info = app
            .world()
            .bedtime()
            .inspect_document(document.id())
            .unwrap();
        assert!(!info.dirty, "never unsaved");
        assert!(info.saving, "about to be saved");
        update_until(&mut app, |_| {
            store.text("scratch").as_deref() == Some("hello")
        });
        update_until(&mut app, |world| {
            !world
                .bedtime()
                .inspect_document(document.id())
                .unwrap()
                .saving
        });
        app.world_mut()
            .bedtime_mut()
            .undo_document(document.id())
            .unwrap();
        update_until(&mut app, |_| store.text("scratch").as_deref() == Some(""));
        assert_eq!(
            app.world()
                .bedtime()
                .document_name(document.id())
                .unwrap()
                .as_str(),
            format!("store:{}/scratch", StoreId::of::<WorkbenchStore>().name())
        );
    }

    #[test]
    fn a_change_to_the_entry_elsewhere_is_read_again() {
        let store = MemoryStore::default();
        store.write("scratch", b"mine").unwrap();
        let mut app = session(&store);
        let document = ready(&mut app);
        assert_eq!(
            app.world()
                .bedtime()
                .read_document(document)
                .unwrap()
                .to_string(),
            "mine"
        );
        store.write("scratch", b"theirs").unwrap();
        app.world_mut()
            .bedtime_mut()
            .refresh_document(document.id())
            .unwrap();
        update_until(&mut app, |world| {
            world.bedtime().read_document(document).unwrap().to_string() == "theirs"
        });
    }
}
