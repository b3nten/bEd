//! Autosave. Applications choose the policy through the `DocumentAutosave`
//! resource; workbench applies it to every document.
use super::*;
use std::{
    collections::{BTreeMap, BTreeSet},
    time::{Duration, Instant},
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocumentAutosavePolicy {
    Never,
    AfterIdle(Duration),
    OnFocusChange,
}
/// The session's autosave policy. With any policy other than `Never`, closing the
/// last view of an unsaved document saves it instead of asking.
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq)]
pub struct DocumentAutosave(pub DocumentAutosavePolicy);

impl Default for DocumentAutosave {
    fn default() -> Self {
        Self(DocumentAutosavePolicy::Never)
    }
}

impl DocumentAutosave {
    pub fn enabled(self) -> bool {
        self.0 != DocumentAutosavePolicy::Never
    }
}

#[derive(Resource, Default)]
pub(crate) struct Autosaves {
    documents: BTreeMap<DocumentId, Pending>,
    focused: Option<(Entity, DocumentId)>,
    saves: bevy::ecs::message::MessageCursor<DocumentSaveFinished>,
}
struct Pending {
    revision: u64,
    edited_at: Instant,
    focus_left: bool,
    attempted: Option<(u64, u64)>,
    work: Option<WorkId>,
}
pub(crate) fn autosave(world: &mut World) {
    if world.resource::<ManagedWork>().is_closing() {
        return;
    }
    let policy = world.resource::<DocumentAutosave>().0;
    let focused = crate::active_layout(world).and_then(|layout| {
        let panel = Workbench::new(world).focused_panel(layout)?;
        Some((panel, world.get::<DocumentReference>(panel)?.document()))
    });
    let errors = tick_document_autosave(world, Instant::now(), focused, policy, true);
    for error in errors {
        crate::closing::report_error(
            world,
            "Autosave failed",
            "A document couldn't be saved automatically.",
            error,
        );
    }
}

/// Admit due saves and return failures for the host's normal error presentation.
fn tick_document_autosave(
    world: &mut World,
    now: Instant,
    focused: Option<(Entity, DocumentId)>,
    policy: DocumentAutosavePolicy,
    enabled: bool,
) -> Vec<String> {
    if !enabled {
        return Vec::new();
    }
    world.init_resource::<Autosaves>();
    let infos: Vec<_> = document_ids(world)
        .into_iter()
        .filter_map(|id| inspect_document(world, id).ok())
        .collect();
    let mut errors = Vec::new();
    world.resource_scope(|world, state: Mut<Autosaves>| {
        let state = state.into_inner();
        let outcomes: Vec<_> = state
            .saves
            .read(world.resource::<Messages<DocumentSaveFinished>>())
            .cloned()
            .collect();
        for outcome in outcomes {
            if let Some(p) = state.documents.get_mut(&outcome.document) {
                if p.work == Some(outcome.work) {
                    p.work = None;
                    if let Err(e) = outcome.result {
                        errors.push(e.to_string());
                    }
                }
            }
        }
        let live: BTreeSet<_> = infos.iter().map(|i| i.id).collect();
        state.documents.retain(|id, _| live.contains(id));
        let departed = if state.focused != focused {
            state.focused.map(|(_, id)| id)
        } else {
            None
        };
        state.focused = focused;
        for info in infos {
            let p = state.documents.entry(info.id).or_insert(Pending {
                revision: info.revision,
                edited_at: now,
                focus_left: false,
                attempted: None,
                work: None,
            });
            if p.revision != info.revision {
                p.revision = info.revision;
                p.edited_at = now;
            }
            if departed == Some(info.id) {
                p.focus_left = true;
            }
            if !info.dirty {
                p.focus_left = false;
                continue;
            }
            let due = match policy {
                DocumentAutosavePolicy::Never => {
                    p.focus_left = false;
                    false
                }
                DocumentAutosavePolicy::OnFocusChange => p.focus_left,
                DocumentAutosavePolicy::AfterIdle(delay) => {
                    now.saturating_duration_since(p.edited_at) >= delay
                }
            };
            let attempt = (info.revision, info.source_generation);
            if !due
                || info.saving
                || info.has_drafts
                || info.conflict
                || info.requires_source_refresh
                || !info.has_save_binding
                || p.attempted == Some(attempt)
                || document_write_blocked(world, info.id)
            {
                continue;
            }
            match save_document(world, info.id) {
                Ok(work) => {
                    p.attempted = Some(attempt);
                    p.focus_left = false;
                    p.work = Some(work);
                }
                Err(DocumentError::DestinationBusy(_)) => {}
                Err(e) => {
                    p.attempted = Some(attempt);
                    p.focus_left = false;
                    errors.push(e.to_string());
                }
            }
        }
    });
    errors
}
