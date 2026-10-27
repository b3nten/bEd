//! Standard save policies, independent of application settings and rendering.
use super::*;
use std::{
    collections::{BTreeMap, BTreeSet},
    time::{Duration, Instant},
};
#[derive(Clone, Copy, Debug)]
pub enum DocumentAutosavePolicy {
    Never,
    AfterIdle(Duration),
    OnFocusChange,
}
#[derive(Resource, Default)]
pub struct Autosaves {
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
/// Admit due saves and return failures for the host's normal error presentation.
pub fn tick_document_autosave(
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

#[derive(Clone)]
pub struct DocumentSaveAll {
    snapshot: Vec<(DocumentId, u64)>,
    awaited: Vec<DocumentId>,
}
impl DocumentSaveAll {
    pub fn poll(&self, world: &World) -> Result<bool, String> {
        let snapshot = save_snapshot(world)?;
        if snapshot != self.snapshot {
            return Err(
                "Documents changed while Save All was pending. Review them before quitting.".into(),
            );
        }
        let infos: Vec<_> = self
            .snapshot
            .iter()
            .map(|(id, _)| inspect_document(world, *id).map_err(|e| e.to_string()))
            .collect::<Result<_, _>>()?;
        if infos.iter().any(|i| i.saving) {
            return Ok(false);
        }
        for info in infos {
            if self.awaited.contains(&info.id) {
                if let Some(error) = info.last_save_error {
                    return Err(format!("Save failed for {:?}: {error}", info.title));
                }
            }
        }
        crate::preflight_session_close(world, crate::SessionCloseMode::Saved)?;
        Ok(true)
    }
}
fn save_snapshot(world: &World) -> Result<Vec<(DocumentId, u64)>, String> {
    document_ids(world)
        .into_iter()
        .map(|id| {
            inspect_document(world, id)
                .map(|i| (id, i.revision))
                .map_err(|e| e.to_string())
        })
        .collect()
}
pub fn begin_document_save_all(world: &mut World) -> Result<DocumentSaveAll, String> {
    let snapshot = save_snapshot(world)?;
    let mut infos = Vec::new();
    for (id, _) in &snapshot {
        let info = inspect_document(world, *id).map_err(|e| e.to_string())?;
        if info.has_drafts
            || info.conflict
            || info.requires_source_refresh
            || (info.dirty && !info.has_save_binding)
        {
            return Err(format!(
                "Resolve unsaved work or source conflicts for {:?} before quitting",
                info.title
            ));
        }
        infos.push(info);
    }
    let mut awaited = Vec::new();
    for info in infos {
        if info.saving {
            awaited.push(info.id);
        } else if info.dirty {
            save_document(world, info.id).map_err(|e| e.to_string())?;
            awaited.push(info.id);
        }
    }
    Ok(DocumentSaveAll { snapshot, awaited })
}
