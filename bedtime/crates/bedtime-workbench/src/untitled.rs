//! Save As, and untitled documents.
//!
//! Save As keeps a document somewhere else from now on: the same document,
//! with its history, now that place's. The app asks where (`SaveAs`: with
//! the `native` and `files` features, the system's picker, unless the app
//! sets its own with `set_save_as`) and answers with a place:
//! `save_document_to` for a file, or `move_document` for any source. While
//! it asks, the document counts as saving, so closing waits for it.
//!
//! Untitled documents ("Untitled", "Untitled 2", …) are new contents with
//! nowhere to be saved yet: saving one is Save As.
use crate::*;
#[cfg(feature = "files")]
use bedtime_document::MemorySource;
use bedtime_document::{DocumentModel, DocumentSource};
use bevy::prelude::*;

/// On an untitled document's entity.
#[derive(Component)]
pub struct Untitled {
    pub name: String,
    /// What a file of it is saved as (`FileModel::EXTENSION`).
    pub extension: &'static str,
}

/// On a document while the app asks where to save it.
#[derive(Component)]
pub(crate) struct ChoosingWhere;

/// On a document that can be saved to a file (Save As): what writes it
/// there, knowing its model.
#[cfg(feature = "files")]
#[derive(Component, Clone, Copy)]
pub(crate) struct FileDocument {
    save_to: fn(&mut World, DocumentId, &std::path::Path) -> Result<(), String>,
}

#[cfg(feature = "files")]
impl FileDocument {
    pub(crate) fn of<M: bedtime_filesystem::FileModel>() -> Self {
        Self {
            save_to: save_to::<M>,
        }
    }
}

/// The app's way to ask where a document goes; it answers with
/// `save_document_to`, `move_document`, or `cancel_save_as`.
#[derive(Resource, Clone, Copy)]
pub struct SaveAs(pub fn(&mut World, DocumentId));

impl BedtimeRef {
    /// Whether Save As can ask where `id` goes: the app has a way to ask,
    /// and the document can be kept somewhere else (it's writable, or
    /// untitled).
    pub fn can_save_document_as(&self, id: DocumentId) -> bool {
        let world = &self.world;
        let Ok(info) = world.bedtime().inspect_document(id) else {
            return false;
        };
        world.contains_resource::<SaveAs>()
            && info.status == DocumentStatus::Ready
            && !info.saving
            && (info.writable || info.untitled)
    }
}

impl Bedtime {
    /// A new, empty untitled document of model `M`, shown as `options` says.
    #[cfg(feature = "files")]
    pub fn new_untitled<M: bedtime_filesystem::FileModel + Default>(
        &mut self,
        options: OpenOptions<M>,
    ) -> Result<Entity, String> {
        let world = &mut self.world;
        let taken: Vec<String> = world
            .query::<&Untitled>()
            .iter(world)
            .map(|untitled| untitled.name.clone())
            .collect();
        let name = (1..)
            .map(|n| {
                if n == 1 {
                    "Untitled".to_owned()
                } else {
                    format!("Untitled {n}")
                }
            })
            .find(|name| !taken.contains(name))
            .unwrap();
        register_untitled::<M>(world);
        let document = untitled_document::<M>(world, name)?;
        world.bedtime_mut().open_document_handle(document, options)
    }

    /// Ask where to keep `id` from now on, and save it there (Save As).
    /// Nothing happens if the app is asking already.
    pub fn save_document_as(&mut self, id: DocumentId) -> Result<(), DocumentError> {
        let world = &mut self.world;
        if world.get::<ChoosingWhere>(id.0).is_some() {
            return Ok(());
        }
        let Some(SaveAs(save_as)) = world.get_resource::<SaveAs>().copied() else {
            return Err(DocumentError::NotSaved(
                "There's no way to choose where to save it".into(),
            ));
        };
        let info = world.bedtime().inspect_document(id)?;
        if info.saving {
            return Err(DocumentError::Saving);
        }
        if !(info.writable || info.untitled) {
            return Err(DocumentError::ReadOnly);
        }
        world.entity_mut(id.0).insert(ChoosingWhere);
        save_as(world, id);
        Ok(())
    }

    /// Keep `handle`'s document in `source` from now on, and save it there:
    /// Save As, for any kind of source. Refused if another document already
    /// holds that source.
    pub fn move_document<M: DocumentModel>(
        &mut self,
        handle: DocumentHandle<M>,
        source: impl DocumentSource<M>,
    ) -> Result<(), String> {
        let world = &mut self.world;
        let id = handle.id();
        world.entity_mut(id.0).remove::<ChoosingWhere>();
        if crate::documents::document_for_source(world, source.id())
            .is_some_and(|other| other != id)
        {
            return Err(format!("{} is open already", source.title()));
        }
        // What's there now: written over (the app asked first), or nothing.
        let expected = source.version().unwrap_or(None);
        crate::documents::replace_source::<M>(world, id, std::sync::Arc::new(source), expected)?;
        // No longer untitled, so saving doesn't ask again.
        let untitled = world.entity_mut(id.0).take::<Untitled>();
        let saved = world.bedtime_mut().save_document(id);
        if let Err(error) = saved {
            if let Some(untitled) = untitled {
                world.entity_mut(id.0).insert(untitled);
            }
            return Err(error.to_string());
        }
        Ok(())
    }

    /// Keep document `id` in the file at `path` from now on, and save it
    /// there (overwriting what's there: the picker asked). It stays open, as
    /// that file's.
    #[cfg(feature = "files")]
    pub fn save_document_to(
        &mut self,
        id: DocumentId,
        path: &std::path::Path,
    ) -> Result<(), String> {
        let world = &mut self.world;
        world.entity_mut(id.0).remove::<ChoosingWhere>();
        let FileDocument { save_to } = *world
            .get::<FileDocument>(id.0)
            .ok_or("This document can't be saved to a file")?;
        save_to(world, id, path)
    }

    /// The app stopped asking where (the picker was dismissed).
    pub fn cancel_save_as(&mut self, id: DocumentId) {
        if let Ok(mut entity) = self.world.get_entity_mut(id.0) {
            entity.remove::<ChoosingWhere>();
        }
    }

    pub fn set_save_as(&mut self, save_as: fn(&mut World, DocumentId)) {
        self.world.insert_resource(SaveAs(save_as));
    }
}

/// Ask where with the system's picker: beside the file it's in now, else in
/// the app's folder (`Root`). The way to ask unless the app sets another.
#[cfg(all(feature = "native", feature = "files"))]
pub(crate) fn ask_with_the_systems_picker(world: &mut World, id: DocumentId) {
    let (name, folder) = match world.get::<Untitled>(id.0) {
        Some(untitled) => (format!("{}.{}", untitled.name, untitled.extension), None),
        None => {
            let file = world.bedtime().document_file(id);
            let name = file
                .as_ref()
                .and_then(|file| file.file_name())
                .map(|name| name.to_string_lossy().into_owned())
                .or_else(|| world.bedtime().inspect_document(id).ok().map(|i| i.title))
                .unwrap_or_default();
            (
                name,
                file.and_then(|file| file.parent().map(ToOwned::to_owned)),
            )
        }
    };
    let mut picker = crate::PathPicker::new("Save", crate::Pick::SaveAs { name });
    if let Some(folder) =
        folder.or_else(|| world.get_resource::<crate::Root>().map(|r| r.0.clone()))
    {
        picker = picker.in_directory(&folder);
    }
    world.bedtime_mut().pick_path(picker, move |world, chosen| {
        let Some(path) = chosen else {
            world.bedtime_mut().cancel_save_as(id);
            return;
        };
        if let Err(error) = world.bedtime_mut().save_document_to(id, &path) {
            world
                .bedtime_mut()
                .notify(Notification::error(format!("Couldn't save: {error}")));
        }
    });
}

/// How to make an untitled document of each model that has them (by its
/// `TypeId`): for reopening one, by name, in a restored panel.
#[cfg(feature = "files")]
#[derive(Resource, Default)]
pub(crate) struct UntitledModels(
    pub(crate)  std::collections::HashMap<
        std::any::TypeId,
        fn(&mut World, String) -> Result<DocumentId, String>,
    >,
);

#[cfg(feature = "files")]
pub(crate) fn register_untitled<M: bedtime_filesystem::FileModel + Default>(world: &mut World) {
    world
        .get_resource_or_init::<UntitledModels>()
        .0
        .insert(std::any::TypeId::of::<M>(), |world, name| {
            untitled_document::<M>(world, name).map(|handle| handle.id())
        });
}

/// A new, empty untitled document called `name`.
#[cfg(feature = "files")]
pub(crate) fn untitled_document<M: bedtime_filesystem::FileModel + Default>(
    world: &mut World,
    name: String,
) -> Result<DocumentHandle<M>, String> {
    let source = MemorySource::unsaved(name.clone(), M::default());
    let document = world
        .bedtime_mut()
        .load_document::<M>(source)
        .map_err(|e| e.to_string())?;
    // A byte form, so its unsaved content can be kept across sessions.
    world
        .bedtime_mut()
        .set_byte_codec(document)
        .map_err(|e| e.to_string())?;
    world.entity_mut(document.id().0).insert((
        Untitled {
            name,
            extension: M::EXTENSION,
        },
        FileDocument::of::<M>(),
    ));
    Ok(document)
}

#[cfg(feature = "files")]
fn save_to<M: bedtime_filesystem::FileModel>(
    world: &mut World,
    id: DocumentId,
    path: &std::path::Path,
) -> Result<(), String> {
    let source = bedtime_filesystem::Filesystem::local()
        .new_document_source(path)
        .map_err(|e| e.to_string())?;
    let handle = world
        .bedtime()
        .cast_document::<M>(id)
        .map_err(|e| e.to_string())?;
    // Its viewers' saved names follow (`ViewerSource`).
    world.bedtime_mut().move_document(handle, source)
}
