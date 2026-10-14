//! Source outline plugin. It consumes immutable host snapshots and requests navigation.
mod ecs;
pub mod presentation;
use bedtime_api::{
    documents::DocumentKind,
    features::{FeatureRegistrar, HostAction, HostContext, MenuSlot},
};
use bedtime_highlight::outline::{OutlineKey, OutlineService};
pub use ecs::{SourceOutlinePlugin, StructureOutline, StructureSurface};

pub const PLUGIN_ID: &str = "bedtime.structure";
pub const PANEL_ID: &str = "bedtime.structure.panel";
pub const OPEN_COMMAND: &str = "bedtime.structure.open";

pub fn navigation_request(
    jump: presentation::StructureJump,
    host: &HostContext<'_>,
) -> Option<HostAction> {
    navigation_for_document(jump, host.active())
}

fn navigation_for_document(
    jump: presentation::StructureJump,
    active: Option<&bedtime_api::documents::PluginDocument>,
) -> Option<HostAction> {
    let active = active?;
    if active.id != jump.key.document
        || active.revision != (jump.key.generation, jump.key.revision)
        || active.path != jump.key.path
        || active.language_id != jump.key.language_id
    {
        return None;
    }
    Some(HostAction::Navigate {
        document: jump.key.document,
        byte_offset: jump.offset,
        revision: active.revision,
    })
}

fn sync_outline(
    outline: &mut Option<OutlineService>,
    active: Option<&bedtime_api::documents::PluginDocument>,
    live: bool,
) {
    if !live {
        *outline = None;
        return;
    }
    let outline = outline.get_or_insert_with(OutlineService::default);
    let target = active.filter(|document| document.kind == DocumentKind::Text);
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

fn register_metadata(registrar: &mut FeatureRegistrar<'_>) {
    registrar.panel_ref(PANEL_ID);
    registrar.command_ref(OPEN_COMMAND);
    registrar.toolbar(OPEN_COMMAND);
    registrar.menu(MenuSlot::Application, OPEN_COMMAND);
    registrar.menu(MenuSlot::TextSelection, OPEN_COMMAND);
}
