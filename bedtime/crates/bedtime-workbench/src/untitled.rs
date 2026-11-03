//! Untitled documents: new contents that aren't in a file yet ("Untitled",
//! "Untitled 2", …). Saving one asks the app where (`set_save_as`, a file
//! picker) and keeps it there (`save_untitled_to`): the same document, with
//! its history, now a file's. While the picker is open it counts as saving,
//! so closing waits for it. Without a picker, saving one is an error.
use crate::*;
use bedtime_document::MemorySource;
use bevy::prelude::*;

/// On an untitled document's entity.
#[derive(Component)]
pub struct Untitled {
    pub name: String,
    /// The app's picker is open for it.
    pub(crate) choosing: bool,
    #[cfg(feature = "files")]
    save_to: fn(&mut World, DocumentId, &std::path::Path) -> Result<(), String>,
}

/// The app's way to ask where an untitled document goes; it answers with
/// `save_untitled_to` or `cancel_save_as`.
#[derive(Resource, Clone, Copy)]
pub struct SaveAs(pub fn(&mut World, DocumentId));

impl Workbench {
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
        let source = MemorySource::unsaved(name.clone(), M::default());
        let document = world
            .workbench_mut()
            .load_document::<M>(source)
            .map_err(|e| e.to_string())?;
        world.entity_mut(document.id().0).insert(Untitled {
            name,
            choosing: false,
            save_to: save_to::<M>,
        });
        world
            .workbench_mut()
            .open_document_handle(document, options)
    }

    /// Save an untitled document at `path` (overwriting what's there: the
    /// picker asked). It stays open, as that file's.
    #[cfg(feature = "files")]
    pub fn save_untitled_to(
        &mut self,
        id: DocumentId,
        path: &std::path::Path,
    ) -> Result<(), String> {
        let world = &mut self.world;
        // No longer untitled while it saves (so saving doesn't ask again).
        let mut untitled = world
            .entity_mut(id.0)
            .take::<Untitled>()
            .ok_or("That document is already a file's")?;
        let result = (untitled.save_to)(world, id, path);
        if result.is_err() {
            untitled.choosing = false;
            world.entity_mut(id.0).insert(untitled);
        }
        result
    }

    /// The picker was dismissed.
    pub fn cancel_save_as(&mut self, id: DocumentId) {
        if let Some(mut untitled) = self.world.get_mut::<Untitled>(id.0) {
            untitled.choosing = false;
        }
    }

    pub fn set_save_as(&mut self, save_as: fn(&mut World, DocumentId)) {
        self.world.insert_resource(SaveAs(save_as));
    }
}

/// Saving an untitled document: ask where (once at a time).
pub(crate) fn ask_where(world: &mut World, id: DocumentId) -> Result<(), DocumentError> {
    if world
        .get::<Untitled>(id.0)
        .is_none_or(|untitled| untitled.choosing)
    {
        return Ok(());
    }
    let Some(SaveAs(save_as)) = world.get_resource::<SaveAs>().copied() else {
        return Err(DocumentError::NotSaved(
            "Choose where to save it first".into(),
        ));
    };
    world.get_mut::<Untitled>(id.0).unwrap().choosing = true;
    save_as(world, id);
    Ok(())
}

#[cfg(feature = "files")]
fn save_to<M: bedtime_filesystem::FileModel>(
    world: &mut World,
    id: DocumentId,
    path: &std::path::Path,
) -> Result<(), String> {
    use bedtime_document::DocumentSource;
    let source = bedtime_filesystem::Filesystem::local()
        .new_document_source(path)
        .map_err(|e| e.to_string())?;
    // Another document already is that file.
    let source_id = <bedtime_filesystem::FsDocumentSource as DocumentSource<M>>::id(&source);
    if crate::documents::document_for_source(world, source_id).is_some_and(|other| other != id) {
        return Err(format!("{} is open already", path.display()));
    }
    let expected = <bedtime_filesystem::FsDocumentSource as DocumentSource<M>>::version(&source)
        .unwrap_or(None);
    let source_path = source.path();
    crate::documents::replace_source::<M>(world, id, std::sync::Arc::new(source), expected)?;
    world
        .workbench_mut()
        .save_document(id)
        .map_err(|e| e.to_string())?;
    // Viewers of it reopen it from its file after a restart.
    let file = source_path;
    let viewers: Vec<Entity> = world
        .get::<DocumentReferences>(id.0)
        .map(|references| references.holders().to_vec())
        .unwrap_or_default();
    for viewer in viewers {
        if world.get::<PanelInstance>(viewer).is_some() {
            world.entity_mut(viewer).insert(ViewerFile(file.clone()));
        }
    }
    Ok(())
}
