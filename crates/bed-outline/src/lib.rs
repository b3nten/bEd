//! Source outline plugin. It consumes immutable host snapshots and requests navigation.
mod ecs;
pub mod presentation;
use bed_editor_api::{DocumentKind, HostContext, HostRequest, MenuSlot, Registrar};
use bed_highlight::outline::{OutlineKey, OutlineService};
pub use ecs::{OutlinePlugin, StructureOutline, StructureSurface};

pub const PLUGIN_ID: &str = "bed.structure";
pub const PANEL_ID: &str = "bed.structure.panel";
pub const OPEN_COMMAND: &str = "bed.structure.open";

pub fn navigation_request(
    jump: presentation::StructureJump,
    host: &HostContext<'_>,
) -> Option<HostRequest> {
    let active = host.active()?;
    if active.id != jump.key.document
        || active.revision != (jump.key.generation, jump.key.revision)
        || active.path != jump.key.path
        || active.language_id != jump.key.language_id
    {
        return None;
    }
    Some(HostRequest::Navigate {
        document: jump.key.document,
        byte_offset: jump.offset,
        revision: active.revision,
    })
}

fn sync_outline(outline: &mut Option<OutlineService>, host: &HostContext<'_>, live: bool) {
    if !live {
        *outline = None;
        return;
    }
    let outline = outline.get_or_insert_with(OutlineService::default);
    let target = host
        .active()
        .filter(|document| document.kind == DocumentKind::Text);
    if let Some(document) = target {
        let key = OutlineKey {
            document: document.id,
            generation: document.revision.0,
            revision: document.revision.1,
            path: document.path.clone(),
            language_id: document.language_id.clone(),
        };
        if outline.requested() != Some(&key)
            && let Some(text) = &document.text
        {
            outline.request(key, text.clone());
        }
        outline.poll();
    } else {
        outline.clear();
    }
}

fn register_metadata(registrar: &mut Registrar<'_>) {
    registrar.panel(PANEL_ID, "Structure");
    registrar.command(OPEN_COMMAND, "Structure", Some("structure"));
    registrar.toolbar(OPEN_COMMAND);
    registrar.menu(MenuSlot::Application, OPEN_COMMAND);
    registrar.menu(MenuSlot::TextSelection, OPEN_COMMAND);
}
