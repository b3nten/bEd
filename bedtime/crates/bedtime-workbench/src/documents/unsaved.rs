//! Hot exit: unsaved documents are kept across sessions instead of asked
//! about. Their content is backed up in the session's store
//! (`WorkbenchStore`, the open workspace's) as its `unsaved` entry, by the
//! name of where each is kept (`document_name`): about a second after it
//! changes, on a worker, and at once when the session ends. Quitting and
//! switching workspace then go ahead without asking.
//!
//! When a document of that name opens again (its panel is restored), it
//! gets the content back as one undoable step, still unsaved. If what it's
//! kept in changed meanwhile, that's a conflict, as any outside change to
//! unsaved content is. A backup no restored panel takes (its panel was
//! closed, or its file is gone) is opened in a new one: its file, else an
//! untitled document with the content, so unsaved work is never out of
//! sight.
//!
//! Without a store to keep them in (no persistence directory), there's no
//! hot exit, and exits ask about unsaved documents as before.
use super::*;
use crate::persistence::{StoreBackend, StoreId, Stores};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    sync::Arc,
    time::{Duration, Instant},
};

/// The store entry the backups are in.
const ENTRY: &str = "unsaved";
/// How long after a change backups are written.
const DELAY: Duration = Duration::from_secs(1);

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
struct Backup {
    /// `document_name`.
    name: String,
    /// The version of what it was made from (last loaded or saved).
    base: Option<String>,
    content: Content,
}

/// Text as text, so the store stays readable; anything else as bytes.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
enum Content {
    Text(String),
    Bytes(Vec<u8>),
}

impl Content {
    fn of(bytes: Vec<u8>) -> Self {
        match String::from_utf8(bytes) {
            Ok(text) => Self::Text(text),
            Err(error) => Self::Bytes(error.into_bytes()),
        }
    }

    fn bytes(&self) -> &[u8] {
        match self {
            Self::Text(text) => text.as_bytes(),
            Self::Bytes(bytes) => bytes,
        }
    }
}

#[derive(Resource, Default)]
pub(crate) struct Unsaved {
    /// The store the backups were read from: read again when it's another
    /// (switching workspace).
    read_from: Option<usize>,
    /// Backups no document has taken yet, by name.
    waiting: BTreeMap<String, Backup>,
    /// What was last written, to write only changes.
    written: Option<Vec<Backup>>,
    /// When what's unsaved last changed, and not yet written.
    changed: Option<Instant>,
    /// Which documents are unsaved, at which revisions: a cheap way to
    /// tell it changed.
    seen: Option<Vec<(DocumentId, u64)>>,
    /// Frames since the backups were read: restored panels reopen their
    /// documents in the first, so backups none took are known after it.
    frames: u32,
}

fn backend(world: &World) -> Option<Arc<dyn StoreBackend>> {
    world
        .get_resource::<Stores>()?
        .backend(StoreId::of::<crate::persistence::WorkbenchStore>())
}

/// Whether unsaved documents are kept across sessions: there's a store to
/// keep them in.
pub(crate) fn hot_exit(world: &World) -> bool {
    backend(world).is_some()
}

pub(crate) fn install(app: &mut App) {
    app.init_resource::<Unsaved>().add_systems(
        PreUpdate,
        (read_backups, restore).chain().after(super::poll_documents),
    );
    app.add_systems(Last, back_up.before(crate::SessionSystems::Close));
}

/// Read the backups from the session's store, when it's a new one.
fn read_backups(world: &mut World) {
    let Some(backend) = backend(world) else {
        return;
    };
    let id = Arc::as_ptr(&backend) as *const () as usize;
    let mut unsaved = world.resource_mut::<Unsaved>();
    if unsaved.read_from == Some(id) {
        return;
    }
    *unsaved = Unsaved {
        read_from: Some(id),
        ..default()
    };
    let backups: Vec<Backup> = match backend.read(ENTRY) {
        Ok(Some(bytes)) => match ron::de::from_bytes(&bytes) {
            Ok(backups) => backups,
            Err(error) => {
                warn!("Unsaved changes from the last session can't be read: {error}");
                return;
            }
        },
        Ok(None) => Vec::new(),
        Err(error) => {
            warn!("Unsaved changes from the last session can't be read: {error}");
            return;
        }
    };
    let mut unsaved = world.resource_mut::<Unsaved>();
    unsaved.written = Some(backups.clone());
    unsaved.waiting = backups
        .into_iter()
        .map(|backup| (backup.name.clone(), backup))
        .collect();
}

/// Give documents that have opened their backups.
fn restore(world: &mut World) {
    if world.resource::<Unsaved>().waiting.is_empty() {
        return;
    }
    let mut unsaved = world.resource_mut::<Unsaved>();
    unsaved.frames += 1;
    #[cfg(feature = "files")]
    if unsaved.frames == 2 {
        open_untaken(world);
    }
    for id in world.bedtime().document_ids() {
        let Some(name) = world.bedtime().document_name(id) else {
            continue;
        };
        let ready = world
            .bedtime()
            .inspect_document(id)
            .is_ok_and(|info| info.status == DocumentStatus::Ready);
        if !ready {
            continue;
        }
        let Some(backup) = world
            .resource_mut::<Unsaved>()
            .waiting
            .remove(name.as_str())
        else {
            continue;
        };
        let base = backup.base.map(SourceVersion);
        let Some(mut document) = world.get_mut::<Document>(id.0) else {
            continue;
        };
        if let Err(error) = document.store.restore_unsaved(backup.content.bytes(), base) {
            let title = document_title(world, id);
            world
                .bedtime_mut()
                .notify(crate::Notification::warning(format!(
                    "Unsaved changes to {title} couldn't be restored: {error}"
                )));
        }
    }
}

/// Open the backups no restored panel took: their file, else (it's gone,
/// or it was untitled) an untitled document. They're restored when ready.
#[cfg(feature = "files")]
fn open_untaken(world: &mut World) {
    let names: Vec<DocumentId> = world.bedtime().document_ids();
    let taken: Vec<String> = names
        .into_iter()
        .filter_map(|id| world.bedtime().document_name(id))
        .map(|name| name.as_str().to_owned())
        .collect();
    let untaken: Vec<String> = world
        .resource::<Unsaved>()
        .waiting
        .keys()
        .filter(|name| !taken.contains(name))
        .cloned()
        .collect();
    for name in untaken {
        let source = SourceName::parse(name.clone());
        let file = source.as_ref().and_then(SourceName::to_file);
        if let Some(path) = file.as_ref().filter(|path| path.is_file()) {
            let options = crate::PathOptions {
                target: crate::Target::New,
                keep_focus: true,
                ..default()
            };
            if world.bedtime_mut().open_path(path, options).is_ok() {
                continue;
            }
        }
        // As an untitled document, named after where it was.
        let title = match (&file, &source) {
            (Some(path), _) => path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Untitled".into()),
            (None, Some(source)) => source.rest().to_owned(),
            (None, None) => name.clone(),
        };
        let make = world
            .get_resource::<crate::untitled::UntitledModels>()
            .and_then(|models| models.0.values().next().copied());
        let opened = make
            .ok_or_else(|| "nothing here makes untitled documents".to_owned())
            .and_then(|make| make(world, title.clone()))
            .and_then(|id| crate::opening::show_document(world, id).map(|_| id));
        let mut unsaved = world.resource_mut::<Unsaved>();
        let Some(mut backup) = unsaved.waiting.remove(&name) else {
            continue;
        };
        match opened {
            Ok(_) => {
                // Taken by the untitled document, as new content.
                backup.name = SourceName::new("untitled", &title).as_str().to_owned();
                backup.base = None;
                unsaved.waiting.insert(backup.name.clone(), backup);
            }
            Err(error) => {
                // Put back: kept for a session that can open it.
                unsaved.waiting.insert(name.clone(), backup);
                warn!("Unsaved changes to {name} couldn't be shown: {error}");
            }
        }
    }
}

fn document_title(world: &World, id: DocumentId) -> String {
    world
        .bedtime()
        .inspect_document(id)
        .map(|info| info.title)
        .unwrap_or_default()
}

/// What's unsaved now: each unsaved document with a name and a byte form,
/// and the backups still waiting for theirs.
fn current(world: &World) -> Vec<Backup> {
    let workbench = world.bedtime();
    let mut backups: Vec<Backup> = workbench
        .document_ids()
        .into_iter()
        .filter_map(|id| {
            let info = workbench.inspect_document(id).ok()?;
            if !info.dirty {
                return None;
            }
            let name = workbench.document_name(id)?;
            let store = &world.get::<Document>(id.0)?.store;
            let bytes = store.encode()?.ok()?;
            Some(Backup {
                name: name.as_str().to_owned(),
                base: store.saved_version().map(|version| version.0),
                content: Content::of(bytes),
            })
        })
        .collect();
    let unsaved = world.resource::<Unsaved>();
    for (name, backup) in &unsaved.waiting {
        if !backups.iter().any(|b| &b.name == name) {
            backups.push(backup.clone());
        }
    }
    backups.sort_by(|a, b| a.name.cmp(&b.name));
    backups
}

/// Write the backups about a second after what's unsaved changes.
fn back_up(world: &mut World) {
    if !hot_exit(world) || world.resource::<Unsaved>().read_from.is_none() {
        return;
    }
    let workbench = world.bedtime();
    let seen: Vec<(DocumentId, u64)> = workbench
        .document_ids()
        .into_iter()
        .filter_map(|id| {
            let info = workbench.inspect_document(id).ok()?;
            info.dirty.then_some((id, info.revision))
        })
        .collect();
    let mut unsaved = world.resource_mut::<Unsaved>();
    if unsaved.seen.as_ref() != Some(&seen) {
        unsaved.seen = Some(seen);
        unsaved.changed = Some(Instant::now());
    }
    if !unsaved.changed.is_some_and(|at| at.elapsed() >= DELAY) {
        return;
    }
    unsaved.changed = None;
    let now = current(world);
    let mut unsaved = world.resource_mut::<Unsaved>();
    if unsaved.written.as_ref() == Some(&now) {
        return;
    }
    unsaved.written = Some(now.clone());
    let Some(backend) = backend(world) else {
        return;
    };
    let write = move |_: crate::Cancellation| {
        if let Err(error) = write(&*backend, &now) {
            warn!("Unsaved changes couldn't be kept: {error}");
        }
    };
    let task = world
        .resource_mut::<ManagedWork>()
        .spawn_task(None, "Keep unsaved changes", write);
    if let Err(error) = task {
        warn!("Unsaved changes couldn't be kept: {error}");
    }
}

fn write(backend: &dyn StoreBackend, backups: &[Backup]) -> Result<(), String> {
    ron::ser::to_string_pretty(&backups, ron::ser::PrettyConfig::default())
        .map_err(|e| e.to_string())
        .and_then(|text| backend.write(ENTRY, text.as_bytes()))
}

/// Write the backups now: the session is ending. Returns the unsaved
/// documents that couldn't be kept (without a name or a byte form, or all
/// of them if writing failed), for the exit to ask about.
pub(crate) fn back_up_now(world: &mut World) -> Vec<DocumentId> {
    let workbench = world.bedtime();
    let unsaved: Vec<DocumentId> = workbench
        .document_ids()
        .into_iter()
        .filter(|&id| workbench.inspect_document(id).is_ok_and(|info| info.dirty))
        .collect();
    let Some(backend) = backend(world) else {
        return unsaved;
    };
    let now = current(world);
    let unkept: Vec<DocumentId> = unsaved
        .iter()
        .copied()
        .filter(|&id| {
            !world
                .bedtime()
                .document_name(id)
                .is_some_and(|name| now.iter().any(|b| b.name == name.as_str()))
        })
        .collect();
    if let Err(error) = write(&*backend, &now) {
        warn!("Unsaved changes couldn't be kept: {error}");
        return unsaved;
    }
    world.resource_mut::<Unsaved>().written = Some(now);
    unkept
}

#[cfg(all(test, feature = "files"))]
mod tests {
    use super::*;
    use crate::documents::tests::{type_text, update_until};
    use crate::persistence::{MemoryStore, PersistenceAppExt, WorkbenchStore};
    use crate::*;
    use std::path::{Path, PathBuf};

    #[bedtime_ui::panel]
    #[derive(Reflect, Default)]
    #[reflect(Component, Default, crate::persistence::Persist)]
    struct Editor;
    impl Panel for Editor {
        const LABEL: &'static str = "Editor";
        #[cfg(feature = "ui")]
        fn draw(
            _: &mut bedtime_ui::egui::Ui,
            _: &mut World,
            _: &bedtime_ui::UiContext<'_>,
            _: Entity,
        ) {
        }
    }

    /// A session keeping what it saves in `store`.
    fn session(store: &MemoryStore) -> App {
        let mut app = App::new();
        app.add_plugins(WorkbenchCorePlugins)
            .add_store::<WorkbenchStore>(store.clone())
            .register_type::<Editor>()
            .register_viewer::<TextBuffer, Editor>("Editor")
            .formats([TEXT])
            .set_new_document::<TextBuffer>();
        app.update();
        if app.world().bedtime().layouts().active().is_none() {
            crate::tests::layout(app.world_mut());
        }
        app
    }

    fn folder(name: &str) -> PathBuf {
        let folder =
            std::env::temp_dir().join(format!("bedtime-unsaved-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        folder.canonicalize().unwrap()
    }

    fn ready(app: &mut App, document: DocumentId) -> DocumentHandle<TextBuffer> {
        update_until(app, |world| {
            world
                .bedtime()
                .inspect_document(document)
                .is_ok_and(|info| info.status == DocumentStatus::Ready)
        });
        app.world().bedtime().cast_document(document).unwrap()
    }

    fn open(app: &mut App, path: &Path) -> DocumentHandle<TextBuffer> {
        let panel = app
            .world_mut()
            .bedtime_mut()
            .open_path(path, default())
            .unwrap();
        let document = app
            .world()
            .get::<DocumentReference>(panel)
            .unwrap()
            .document();
        ready(app, document)
    }

    fn quit(app: &mut App) {
        app.world_mut()
            .write_message(CloseRequested { window: None });
        update_until(app, |world| {
            !world.resource::<Messages<AppExit>>().is_empty()
        });
        assert!(
            app.world().bedtime().close_prompt().is_none(),
            "quitting doesn't ask"
        );
    }

    fn text(world: &World, document: DocumentHandle<TextBuffer>) -> String {
        world.bedtime().read_document(document).unwrap().to_string()
    }

    /// The restored document kept in `name`, once it has its backup.
    fn restored(app: &mut App, name: &str) -> DocumentHandle<TextBuffer> {
        update_until(app, |world| {
            world.bedtime().document_ids().into_iter().any(|id| {
                world
                    .bedtime()
                    .document_name(id)
                    .is_some_and(|n| n.as_str() == name)
                    && world
                        .bedtime()
                        .inspect_document(id)
                        .is_ok_and(|info| info.dirty)
            })
        });
        let world = app.world();
        let id = world
            .bedtime()
            .document_ids()
            .into_iter()
            .find(|&id| {
                world
                    .bedtime()
                    .document_name(id)
                    .is_some_and(|n| n.as_str() == name)
            })
            .unwrap();
        world.bedtime().cast_document(id).unwrap()
    }

    #[test]
    fn quitting_keeps_unsaved_files_and_untitled_documents_for_next_time() {
        let store = MemoryStore::default();
        let folder = folder("kept");
        let notes = folder.join("notes.txt");
        std::fs::write(&notes, "hello").unwrap();

        let mut first = session(&store);
        let document = open(&mut first, &notes);
        type_text(first.world_mut(), document, 5, " there");
        let world = first.world_mut();
        let panel = world
            .bedtime_mut()
            .new_untitled::<TextBuffer>(OpenOptions {
                target: Target::New,
                ..default()
            })
            .unwrap();
        let untitled = world.get::<DocumentReference>(panel).unwrap().document();
        let untitled = ready(&mut first, untitled);
        type_text(first.world_mut(), untitled, 0, "draft");
        quit(&mut first);
        // Nothing was saved to the file.
        assert_eq!(std::fs::read_to_string(&notes).unwrap(), "hello");
        drop(first);

        let mut second = session(&store);
        let name = SourceName::file(&notes).unwrap();
        let document = restored(&mut second, name.as_str());
        assert_eq!(text(second.world(), document), "hello there");
        let untitled = restored(&mut second, "untitled:Untitled");
        assert_eq!(text(second.world(), untitled), "draft");
        // Undoing goes back to what the file holds.
        second
            .world_mut()
            .bedtime_mut()
            .undo_document(document.id())
            .unwrap();
        assert_eq!(text(second.world(), document), "hello");
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn a_file_changed_since_is_a_conflict_when_restored() {
        let store = MemoryStore::default();
        let folder = folder("changed");
        let notes = folder.join("notes.txt");
        std::fs::write(&notes, "hello").unwrap();

        let mut first = session(&store);
        let document = open(&mut first, &notes);
        type_text(first.world_mut(), document, 5, " mine");
        quit(&mut first);
        drop(first);
        std::fs::write(&notes, "hello theirs, longer").unwrap();

        let mut second = session(&store);
        let name = SourceName::file(&notes).unwrap();
        let document = restored(&mut second, name.as_str());
        assert_eq!(text(second.world(), document), "hello mine");
        assert!(
            second
                .world()
                .bedtime()
                .inspect_document(document.id())
                .unwrap()
                .conflict
        );
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn unsaved_changes_to_a_file_gone_since_come_back_untitled() {
        let store = MemoryStore::default();
        let folder = folder("gone");
        let notes = folder.join("notes.txt");
        std::fs::write(&notes, "hello").unwrap();

        let mut first = session(&store);
        let document = open(&mut first, &notes);
        type_text(first.world_mut(), document, 5, " there");
        quit(&mut first);
        drop(first);
        std::fs::remove_file(&notes).unwrap();

        let mut second = session(&store);
        let document = restored(&mut second, "untitled:notes.txt");
        assert_eq!(text(second.world(), document), "hello there");
        // In a panel, for the user to save somewhere.
        assert!(
            second
                .world()
                .get::<DocumentReferences>(document.id().entity())
                .is_some_and(|references| !references.holders().is_empty())
        );
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn saved_documents_leave_nothing_behind() {
        let store = MemoryStore::default();
        let folder = folder("saved");
        let notes = folder.join("notes.txt");
        std::fs::write(&notes, "hello").unwrap();
        let mut app = session(&store);
        let document = open(&mut app, &notes);
        type_text(app.world_mut(), document, 5, "!");
        app.world_mut()
            .bedtime_mut()
            .save_document(document.id())
            .unwrap();
        update_until(&mut app, |world| {
            world
                .bedtime()
                .inspect_document(document.id())
                .is_ok_and(|info| !info.dirty && !info.saving)
        });
        quit(&mut app);
        let kept: Vec<Backup> = ron::de::from_str(&store.text(ENTRY).unwrap()).unwrap();
        assert!(kept.is_empty());
        std::fs::remove_dir_all(folder).unwrap();
    }
}
