//! Application editor presentation; source I/O and checkpoints belong to Bedtime.
use bedtime_editor::{
    TextBuffer,
    workbench::{bind_text_editor, unbind_text_editor},
};
pub(crate) use bedtime_filesystem::workbench::FileWrites;
use bedtime_workbench::*;
use bevy::prelude::*;
use std::path::{Path, PathBuf};
pub(crate) fn source_path(world: &World, id: DocumentId) -> Option<PathBuf> {
    document_source_path(world, id)
}
pub(crate) fn take_saved_text(
    world: &World,
    completion: &DocumentSaveFinished,
) -> Option<TextBuffer> {
    saved_document_capture(world, completion)
}
pub struct FileEditorPlugin;
impl Plugin for FileEditorPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(bedtime_filesystem::workbench::FilesystemDocumentsPlugin);
        #[cfg(feature = "automation")]
        {
            use bedtime_automation::AutomationAppExt;
            app.register_automation_command("app.navigate_editor", navigate);
        }
    }
}
#[cfg(test)]
pub fn bind_loaded(
    world: &mut World,
    panel: Entity,
    path: &Path,
    _text: &TextBuffer,
    _stamp: Option<(DocumentId, u64)>,
) -> Result<(), String> {
    let source = bedtime_filesystem::Filesystem::local()
        .document_source(path)
        .map_err(|e| e.to_string())?;
    let loaded =
        <bedtime_filesystem::FsDocumentSource as DocumentSource<TextBuffer>>::load(&source)
            .map_err(|e| e.to_string())?;
    let h = open_prepared_document(world, source, loaded).map_err(|e| e.to_string())?;
    bind(world, panel, h.id())
}
pub(crate) fn bind(world: &mut World, panel: Entity, id: DocumentId) -> Result<(), String> {
    let handle = cast_document::<TextBuffer>(world, id).map_err(|e| e.to_string())?;
    if world
        .get::<crate::files::FilePanel>(panel)
        .is_some_and(|p| {
            matches!(
                p.viewer,
                crate::opening::ViewerId::Image
                    | crate::opening::ViewerId::Model
                    | crate::opening::ViewerId::Hex
            )
        })
    {
        return bind_document_view(world, panel, handle).map_err(|e| e.to_string());
    }
    if world
        .get::<bedtime_editor::workbench::CodeLanguage>(id.0)
        .is_none()
    {
        let language = source_path(world, id)
            .and_then(|p| {
                bedtime_treesitter::language_name(&p.to_string_lossy()).map(str::to_owned)
            })
            .unwrap_or_else(|| "text".into());
        world
            .entity_mut(id.0)
            .insert(bedtime_editor::workbench::CodeLanguage(language));
    }
    bind_text_editor(world, panel, handle).map_err(|e| e.to_string())?;
    crate::settings::configure_editor(world, panel);
    if world
        .get::<InArea>(panel)
        .and_then(|a| world.get::<Area>(a.0))
        .is_some_and(|a| a.selected_panel() == Some(panel))
    {
        crate::files::select_structure_source(world, panel);
    }
    Ok(())
}
pub fn can_replace(world: &World, panel: Entity) -> bool {
    world.get::<DocumentView>(panel).is_none_or(|v| {
        inspect_document(world, v.document())
            .is_ok_and(|i| !i.dirty && !i.saving && !i.has_drafts && !i.conflict)
    })
}
pub fn same_source(world: &World, panel: Entity, path: &Path) -> bool {
    world
        .get::<DocumentView>(panel)
        .and_then(|v| source_path(world, v.document()))
        .as_deref()
        == Some(path)
}
pub fn detach(world: &mut World, panel: Entity) -> Result<(), String> {
    unbind_text_editor(world, panel)
        .map(|_| ())
        .map_err(|e| e.to_string())
}
#[cfg(feature = "automation")]
fn navigate(
    world: &mut World,
    arguments: serde_json::Value,
) -> bedtime_automation::AutomationResult {
    use bedtime_editor::{Navigation, Reveal, TextSelection};
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
    bedtime_editor::workbench::navigate_editor(
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
