//! Local text documents and application-owned file save policy. The editor
//! crate knows neither paths nor filesystem operations.
use bedtime_editor2::TextBuffer;
use bedtime_editor2::workbench::{bind_text_editor, unbind_text_editor};
use bedtime_workbench2::*;
use bevy::prelude::*;
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

const MAX_TEXT_BYTES: u64 = 256 * 1024 * 1024;

/// File identity is application-owned and can follow a confirmed rename without
/// replacing the document or losing history. Workbench source_key stays unset.
#[derive(Component)]
struct FileSource {
    path: Option<PathBuf>,
    pending_binding: bool,
    identity_generation: Arc<AtomicU64>,
    checkpoint: Arc<std::sync::Mutex<TextBuffer>>,
    saved: Arc<std::sync::Mutex<Option<SavedText>>>,
}

struct SavedText {
    revision: u64,
    identity: u64,
    text: TextBuffer,
}

pub(crate) fn source_path(world: &World, document: DocumentId) -> Option<&Path> {
    world.get::<FileSource>(document.0)?.path.as_deref()
}

/// Pair the worker's actual write capture with Workbench's acknowledged receipt.
/// SourceChanged and failed writes never become language-server save events.
pub(crate) fn take_saved_text(
    world: &World,
    completion: &DocumentSaveFinished,
) -> Option<TextBuffer> {
    let source = world.get::<FileSource>(completion.document.0)?;
    let mut saved = source.saved.lock().unwrap();
    if saved
        .as_ref()
        .is_none_or(|s| s.revision != completion.captured_revision)
    {
        return None;
    }
    let saved = saved.take().unwrap();
    (completion.result.is_ok()
        && saved.identity == source.identity_generation.load(Ordering::Acquire))
    .then_some(saved.text)
}

pub struct FileEditorPlugin;
impl Plugin for FileEditorPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            reconcile_bindings.after(crate::files::FilesSystems::Update),
        );
        #[cfg(feature = "automation")]
        {
            use bedtime_automation2::AutomationAppExt;
            app.register_automation_command("app.navigate_editor", navigate);
        }
    }
}

#[cfg(feature = "automation")]
fn navigate(
    world: &mut World,
    arguments: serde_json::Value,
) -> bedtime_automation2::AutomationResult {
    use bedtime_editor2::{Navigation, Reveal, TextSelection};
    let object = arguments.as_object().ok_or("Expected an argument object")?;
    if object.keys().any(|key| {
        !matches!(
            key.as_str(),
            "panel" | "revision" | "start" | "end" | "anchor" | "head" | "reveal" | "focus"
        )
    }) {
        return Err(
            "Accepted navigation fields: panel, revision, start, end, anchor, head, reveal, focus"
                .into(),
        );
    }
    let bits = object
        .get("panel")
        .and_then(|v| v.as_str())
        .ok_or("panel must be an entity ID string")?
        .parse::<u64>()
        .map_err(|_| "Invalid panel ID")?;
    let panel = Entity::try_from_bits(bits).ok_or("Invalid panel ID")?;
    let document = world
        .get::<DocumentView>(panel)
        .ok_or("Panel has no document")?
        .document();
    let handle = cast_document::<TextBuffer>(world, document).map_err(|e| e.to_string())?;
    let number = |key: &str| {
        object
            .get(key)
            .and_then(|v| v.as_u64())
            .ok_or_else(|| format!("{key} must be an unsigned integer"))
    };
    let offset = |key: &str| {
        usize::try_from(number(key)?)
            .map_err(|_| format!("{key} exceeds the supported offset range"))
    };
    let selection = if object.contains_key("anchor") || object.contains_key("head") {
        Some(
            TextSelection::new(
                read_document(world, handle).map_err(|e| e.to_string())?,
                offset("anchor")?,
                offset("head")?,
            )
            .map_err(|e| e.to_string())?,
        )
    } else {
        None
    };
    let reveal = match object.get("reveal").and_then(|v| v.as_str()) {
        None | Some("minimal") => Reveal::Minimal,
        Some("center") => Reveal::Center,
        _ => return Err("reveal must be minimal or center".into()),
    };
    let focus = object
        .get("focus")
        .map(|v| v.as_bool().ok_or("focus must be a boolean"))
        .transpose()?
        .unwrap_or(false);
    bedtime_editor2::workbench::navigate_editor(
        world,
        panel,
        Navigation {
            revision: number("revision")?,
            range: offset("start")?..offset("end")?,
            selection,
            reveal,
            focus,
        },
    )
    .map_err(|e| e.to_string())?;
    Ok(serde_json::json!({"panel": panel.to_bits().to_string()}))
}

fn find_source(
    world: &mut World,
    workspace: Entity,
    path: &Path,
) -> Option<DocumentHandle<TextBuffer>> {
    let id = world
        .query::<(Entity, &FileSource, &InWorkspace)>()
        .iter(world)
        .find_map(|(entity, source, owner)| {
            (owner.0 == workspace && source.path.as_deref() == Some(path))
                .then_some(DocumentId(entity))
        })?;
    cast_document(world, id).ok()
}

pub fn load_stamp(world: &mut World, panel: Entity, path: &Path) -> Option<(DocumentId, u64)> {
    let workspace = world.get::<InWorkspace>(panel)?.0;
    let handle = find_source(world, workspace, path)?;
    Some((
        handle.id(),
        inspect_document(world, handle.id()).ok()?.source_generation,
    ))
}

/// Equality against large source/canonical snapshots is resolved on the loader
/// worker. Reusing an equal root makes main-world reconciliation constant-time.
pub fn load_snapshots(world: &mut World, panel: Entity, path: &Path) -> Vec<TextBuffer> {
    let Some(workspace) = world.get::<InWorkspace>(panel).map(|owner| owner.0) else {
        return Vec::new();
    };
    let Some(handle) = find_source(world, workspace, path) else {
        return Vec::new();
    };
    let mut snapshots = vec![read_document(world, handle).unwrap().clone()];
    snapshots.push(
        world
            .get::<FileSource>(handle.id().entity())
            .unwrap()
            .checkpoint
            .lock()
            .unwrap()
            .clone(),
    );
    snapshots
}

pub fn bind_loaded(
    world: &mut World,
    panel: Entity,
    path: &Path,
    text: &TextBuffer,
    stamp: Option<(DocumentId, u64)>,
) -> Result<(), String> {
    let workspace = world
        .get::<InWorkspace>(panel)
        .ok_or("Editor has no workspace")?
        .0;
    let handle = if let Some(handle) = find_source(world, workspace, path) {
        if let Some((document, generation)) = stamp.filter(|(document, _)| *document == handle.id())
        {
            match offer_document_external_update(
                world,
                handle,
                generation,
                text.clone(),
                Some(SourceVersion(text.fingerprint())),
            ) {
                Ok(_) => {
                    *world
                        .get::<FileSource>(handle.id().entity())
                        .unwrap()
                        .checkpoint
                        .lock()
                        .unwrap() = text.clone();
                }
                Err(DocumentError::StaleSourceGeneration { .. }) => {}
                Err(error) => return Err(error.to_string()),
            }
            debug_assert_eq!(document, handle.id());
        }
        handle
    } else {
        let title = path
            .file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy();
        let mut options = DocumentOptions::loaded(title);
        options.initial_state =
            DocumentInitialState::Loaded(Some(SourceVersion(text.fingerprint())));
        let handle =
            open_document(world, workspace, text.clone(), options).map_err(|e| e.to_string())?;
        world.entity_mut(handle.id().entity()).insert(FileSource {
            path: Some(path.to_owned()),
            pending_binding: true,
            identity_generation: Arc::new(AtomicU64::new(0)),
            checkpoint: Arc::new(std::sync::Mutex::new(text.clone())),
            saved: Arc::new(std::sync::Mutex::new(None)),
        });
        update_binding(world, handle.id()).map_err(|e| e.to_string())?;
        handle
    };
    // Detection runs only for a newly attached language. Explicit choices survive
    // reloads and sibling views; an unknown extension remains a plain text editor.
    if world
        .get::<bedtime_editor2::workbench::CodeLanguage>(handle.id().entity())
        .is_none()
    {
        let language =
            bedtime_treesitter2::language_name(&path.to_string_lossy()).unwrap_or("text");
        world
            .entity_mut(handle.id().entity())
            .insert(bedtime_editor2::workbench::CodeLanguage(language.into()));
    }
    bind_text_editor(world, panel, handle).map_err(|e| e.to_string())?;
    crate::settings::configure_editor(world, panel);
    if world
        .get::<InArea>(panel)
        .and_then(|owner| world.get::<Area>(owner.0))
        .is_some_and(|area| area.selected_panel() == Some(panel))
    {
        crate::files::select_structure_source(world, panel);
    }
    Ok(())
}

pub fn can_replace(world: &World, panel: Entity) -> bool {
    world.get::<DocumentView>(panel).is_none_or(|view| {
        inspect_document(world, view.document())
            .is_ok_and(|info| !info.dirty && !info.saving && !info.has_drafts && !info.conflict)
    })
}

pub fn same_source(world: &World, panel: Entity, path: &Path) -> bool {
    world
        .get::<DocumentView>(panel)
        .and_then(|view| world.get::<FileSource>(view.document().entity()))
        .is_some_and(|source| source.path.as_deref() == Some(path))
}

pub fn detach(world: &mut World, panel: Entity) -> Result<(), String> {
    let document = unbind_text_editor(world, panel).map_err(|e| e.to_string())?;
    if let Some(document) = document {
        let viewed = world
            .query::<&DocumentView>()
            .iter(world)
            .any(|view| view.document() == document);
        if !viewed
            && inspect_document(world, document)
                .is_ok_and(|info| !info.dirty && !info.saving && !info.conflict && !info.has_drafts)
        {
            close_document(world, document).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// Preserve local work when a backing file disappears; clean viewers retain the
/// application's existing unload behavior. Missing sources have no save binding.
pub fn retain_deleted(world: &mut World, panel: Entity) -> bool {
    let Some(document) = world.get::<DocumentView>(panel).map(DocumentView::document) else {
        return false;
    };
    if !can_replace(world, panel) {
        if let Some(mut source) = world.get_mut::<FileSource>(document.entity()) {
            source.path = None;
            source.pending_binding = true;
            source.identity_generation.fetch_add(1, Ordering::AcqRel);
        }
        let _ = update_binding(world, document);
        world.get_mut::<PanelInstance>(panel).unwrap().preview = false;
        true
    } else {
        false
    }
}

pub fn move_sources(world: &mut World, from: &Path, to: &Path) {
    let moved: Vec<_> = world
        .query::<(Entity, &FileSource)>()
        .iter(world)
        .filter_map(|(entity, source)| {
            let suffix = source.path.as_ref()?.strip_prefix(from).ok()?;
            Some((
                DocumentId(entity),
                if suffix.as_os_str().is_empty() {
                    to.to_owned()
                } else {
                    to.join(suffix)
                },
            ))
        })
        .collect();
    for (id, path) in moved {
        let mut source = world.get_mut::<FileSource>(id.entity()).unwrap();
        source.path = Some(path);
        source.pending_binding = true;
        source.identity_generation.fetch_add(1, Ordering::AcqRel);
        let _ = update_binding(world, id);
    }
}

fn update_binding(world: &mut World, id: DocumentId) -> Result<(), DocumentError> {
    let handle = cast_document::<TextBuffer>(world, id)?;
    match world.get::<FileSource>(id.entity()).unwrap().path.clone() {
        Some(path) => {
            let key = path.to_string_lossy().into_owned();
            let identity = world
                .get::<FileSource>(id.entity())
                .unwrap()
                .identity_generation
                .clone();
            let generation = identity.load(Ordering::Acquire);
            let checkpoint = world
                .get::<FileSource>(id.entity())
                .unwrap()
                .checkpoint
                .clone();
            let saved = world.get::<FileSource>(id.entity()).unwrap().saved.clone();
            bind_document_save(
                world,
                handle,
                DocumentSaveBinding::new(
                    Some(key),
                    move |snapshot: DocumentSaveSnapshot<TextBuffer>| {
                        let observation = SavedText {
                            revision: snapshot.revision,
                            identity: generation,
                            text: snapshot.model.clone(),
                        };
                        let result = save_text(&path, snapshot, &checkpoint, || {
                            identity.load(Ordering::Acquire) == generation
                        });
                        if result.is_ok() {
                            *saved.lock().unwrap() = Some(observation);
                        }
                        result
                    },
                ),
            )?;
        }
        None => unbind_document_save(world, id)?,
    }
    world
        .get_mut::<FileSource>(id.entity())
        .unwrap()
        .pending_binding = false;
    Ok(())
}

pub(crate) fn reconcile_bindings(world: &mut World) {
    let pending: Vec<_> = world
        .query::<(Entity, &FileSource)>()
        .iter(world)
        .filter_map(|(entity, source)| source.pending_binding.then_some(DocumentId(entity)))
        .collect();
    for id in pending {
        let _ = update_binding(world, id);
    }
}

/// Compare source bytes incrementally against the expected immutable checkpoint.
/// This never makes a second full-file string or rope during a save.
fn source_matches(path: &Path, expected: &TextBuffer) -> Result<bool, DocumentSaveError> {
    let metadata =
        fs::metadata(path).map_err(|error| DocumentSaveError::Failed(error.to_string()))?;
    if !metadata.is_file() {
        return Err(DocumentSaveError::Failed(
            "The source is no longer a regular file.".into(),
        ));
    }
    if metadata.len() != expected.len() as u64 {
        return Ok(false);
    }
    let mut file =
        File::open(path).map_err(|error| DocumentSaveError::Failed(error.to_string()))?;
    let mut scratch = [0; 4096];
    for chunk in expected.chunks() {
        file.read_exact(&mut scratch[..chunk.len()])
            .map_err(|error| DocumentSaveError::Failed(error.to_string()))?;
        if &scratch[..chunk.len()] != chunk.as_bytes() {
            return Ok(false);
        }
    }
    let mut extra = [0];
    Ok(file
        .read(&mut extra)
        .map_err(|error| DocumentSaveError::Failed(error.to_string()))?
        == 0)
}

/// Ordinary filesystems have no atomic content CAS: another process can write
/// between the final comparison and rename. Known application identity changes
/// are generation-gated before replacement and before accepting the receipt.
fn save_text(
    path: &Path,
    snapshot: DocumentSaveSnapshot<TextBuffer>,
    checkpoint: &std::sync::Mutex<TextBuffer>,
    identity_matches: impl Fn() -> bool,
) -> Result<Option<SourceVersion>, DocumentSaveError> {
    if snapshot.model.len() as u64 > MAX_TEXT_BYTES {
        return Err(DocumentSaveError::Failed(
            "Text exceeds the editor's 256 MiB file limit; the file was preserved.".into(),
        ));
    }
    let baseline = checkpoint
        .lock()
        .map_err(|_| DocumentSaveError::Failed("Source checkpoint lock failed".into()))?
        .clone();
    let expected = snapshot.expected_source_version.as_ref().ok_or_else(|| {
        DocumentSaveError::Failed(
            "Reload the file before saving without a source checkpoint.".into(),
        )
    })?;
    if expected.0 != baseline.fingerprint() {
        return Err(DocumentSaveError::Conflict(
            "Source checkpoint changed; reload before saving.".into(),
        ));
    }
    let check = || -> Result<(), DocumentSaveError> {
        if !identity_matches() {
            return Err(DocumentSaveError::Conflict(
                "The file moved or was deleted during save. Reload its current source.".into(),
            ));
        }
        if !source_matches(path, &baseline)? {
            return Err(DocumentSaveError::Conflict(
                "The file changed on disk. Reload to compare before saving.".into(),
            ));
        }
        Ok(())
    };
    check()?;
    let target =
        fs::canonicalize(path).map_err(|error| DocumentSaveError::Failed(error.to_string()))?;
    let metadata =
        fs::metadata(&target).map_err(|error| DocumentSaveError::Failed(error.to_string()))?;
    if !metadata.is_file() || metadata.permissions().readonly() {
        return Err(DocumentSaveError::Failed(
            "The file is read-only or is not a regular file.".into(),
        ));
    }
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let temp = target.parent().unwrap().join(format!(
        ".bed-editor-{}-{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|error| DocumentSaveError::Failed(error.to_string()))?;
        file.set_permissions(metadata.permissions())
            .map_err(|error| DocumentSaveError::Failed(error.to_string()))?;
        for chunk in snapshot.model.chunks() {
            file.write_all(chunk.as_bytes())
                .map_err(|error| DocumentSaveError::Failed(error.to_string()))?;
        }
        file.sync_all()
            .map_err(|error| DocumentSaveError::Failed(error.to_string()))?;
        drop(file);
        check()?;
        if fs::canonicalize(path).ok().as_ref() != Some(&target) {
            return Err(DocumentSaveError::Conflict(
                "The file's destination changed during save.".into(),
            ));
        }
        fs::rename(&temp, &target).map_err(|error| DocumentSaveError::Failed(error.to_string()))?;
        if !identity_matches() {
            return Err(DocumentSaveError::Conflict(
                "The source moved during save; refresh its new destination.".into(),
            ));
        }
        let mut current = checkpoint
            .lock()
            .map_err(|_| DocumentSaveError::Failed("Source checkpoint lock failed".into()))?;
        if *current != baseline {
            return Err(DocumentSaveError::Conflict(
                "Source knowledge changed during save; reload before retrying.".into(),
            ));
        }
        *current = snapshot.model.clone();
        Ok(Some(SourceVersion(snapshot.model.fingerprint())))
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}
