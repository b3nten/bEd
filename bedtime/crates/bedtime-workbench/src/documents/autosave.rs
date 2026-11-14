//! Autosave. Applications choose the policy with the `DocumentAutosave`
//! resource; workbench applies it to every document.
use super::*;
use crate::LayoutsWorldExt;
use crate::WorkbenchWorldExt;
use bevy::ecs::message::MessageCursor;
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocumentAutosavePolicy {
    Never,
    /// Once a document has gone this long without an edit.
    AfterIdle(Duration),
    /// When focus moves away from the panel showing it.
    OnFocusChange,
}

/// The session's autosave policy. With any policy other than `Never`, closing the
/// last reference to an unsaved document saves it instead of asking.
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

#[derive(Default)]
pub(crate) struct Pending {
    revision: u64,
    edited_at: Option<Instant>,
    focus_left: bool,
    /// The revision last tried, so a failing save isn't retried every frame.
    attempted: Option<u64>,
    saving: bool,
}

pub(crate) fn autosave(
    world: &mut World,
    mut pending: Local<BTreeMap<DocumentId, Pending>>,
    mut focused: Local<Option<DocumentId>>,
    mut finished: Local<MessageCursor<DocumentSaveFinished>>,
) {
    if world.resource::<ManagedWork>().is_closing() {
        return;
    }
    let mut errors: Vec<String> = finished
        .read(world.resource::<Messages<DocumentSaveFinished>>())
        .filter(|save| pending.get(&save.document).is_some_and(|p| p.saving))
        .filter_map(|save| save.result.clone().err())
        .collect();
    let now = Instant::now();
    let policy = world.resource::<DocumentAutosave>().0;
    let current = world.layouts().active().and_then(|layout| {
        let panel = world.layouts().focused_panel(layout)?;
        Some(world.get::<DocumentReference>(panel)?.document())
    });
    let departed = (*focused != current).then_some(*focused).flatten();
    *focused = current;

    let infos: Vec<_> = world
        .workbench()
        .document_ids()
        .into_iter()
        .filter_map(|id| world.workbench().inspect_document(id).ok())
        .collect();
    pending.retain(|id, _| infos.iter().any(|info| info.id == *id));
    for info in infos {
        let p = pending.entry(info.id).or_default();
        p.saving &= info.saving;
        if p.edited_at.is_none() || p.revision != info.revision {
            p.revision = info.revision;
            p.edited_at = Some(now);
        }
        if departed == Some(info.id) {
            p.focus_left = true;
        }
        if !info.dirty {
            p.focus_left = false;
            continue;
        }
        let due = match policy {
            DocumentAutosavePolicy::Never => false,
            DocumentAutosavePolicy::OnFocusChange => p.focus_left,
            DocumentAutosavePolicy::AfterIdle(delay) => {
                p.edited_at.is_some_and(|at| now - at >= delay)
            }
        };
        if !due
            || info.saving
            || info.conflict
            || info.missing
            || !info.writable
            || info.has_drafts
            || p.attempted == Some(info.revision)
        {
            continue;
        }
        p.attempted = Some(info.revision);
        p.focus_left = false;
        match world.workbench_mut().save_document(info.id) {
            Ok(()) => p.saving = true,
            Err(error) => errors.push(error.to_string()),
        }
    }
    for error in errors {
        crate::closing::report_error(
            world,
            "Autosave failed",
            "A document couldn't be saved automatically.",
            Some(error),
        );
    }
}

#[cfg(feature = "persistence")]
crate::settings! {
    "Documents" {
        /// Save a document a second after the last edit (and when closing it,
        /// rather than asking).
        pub Autosave: bool {
            default: false,
            key: "editor.autosave",
            label: "Autosave",
            apply: |world, on| {
                world.insert_resource(DocumentAutosave(if *on {
                    DocumentAutosavePolicy::AfterIdle(Duration::from_secs(1))
                } else {
                    DocumentAutosavePolicy::Never
                }))
            },
        }
    }
}

/// The documents' settings (`Autosave`), in `WorkbenchPlugins`. Without it
/// the app sets `DocumentAutosave` itself.
#[cfg(feature = "persistence")]
pub struct DocumentSettingsPlugin;

#[cfg(feature = "persistence")]
impl Plugin for DocumentSettingsPlugin {
    fn build(&self, app: &mut App) {
        crate::settings::SettingsAppExt::register_setting::<Autosave>(app);
    }
}
