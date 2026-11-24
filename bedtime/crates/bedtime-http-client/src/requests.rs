//! The workspace's requests as a document (`HttpRequests`), kept in its store
//! (`http_requests.ron`): every change is saved as it's made, can be undone,
//! and the requests are read again if the file is changed elsewhere.
//!
//! The panel edits a copy of the requests (`Collection`'s own methods, and
//! its widgets); what differs from the document becomes changes
//! (`RequestChange`, by `changes`), made together. Edits close in time are
//! one undo step.
use crate::{Collection, Field, SavedRequest};
use bedtime_workbench::{
    BedtimeWorldExt, ByteModel, DocumentError, DocumentHandle, DocumentModel, DocumentReference,
    DocumentStatus, DocumentUndoGroup, EditOptions, persistence::WorkbenchStore,
};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

/// The store entry the requests are kept in.
pub(crate) const ENTRY: &str = "http_requests";
/// An edit after a pause this long is a new undo step.
const PAUSE: Duration = Duration::from_secs(1);

/// The workspace's requests and variables: a document, kept in its store.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HttpRequests(pub Collection);

/// A change to the requests. Each returns its inverse, so it can be undone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RequestChange {
    /// Put `request` at `at` in the list (or last, if that's past the end).
    Insert {
        at: usize,
        request: SavedRequest,
    },
    Remove {
        id: u64,
    },
    /// Replace the request with the same id.
    Set(SavedRequest),
    SetVariables(Vec<Field>),
}

impl DocumentModel for HttpRequests {
    type Change = RequestChange;
    type OpenState = ();

    fn apply(&mut self, change: &RequestChange) -> Result<RequestChange, String> {
        let requests = &mut self.0.requests;
        let index = |requests: &[SavedRequest], id: u64| {
            requests
                .iter()
                .position(|request| request.id == id)
                .ok_or_else(|| format!("There's no request {id}"))
        };
        match change {
            RequestChange::Insert { at, request } => {
                if requests.iter().any(|r| r.id == request.id) {
                    return Err(format!("There's already a request {}", request.id));
                }
                requests.insert((*at).min(requests.len()), request.clone());
                Ok(RequestChange::Remove { id: request.id })
            }
            RequestChange::Remove { id } => {
                let at = index(requests, *id)?;
                let request = requests.remove(at);
                Ok(RequestChange::Insert { at, request })
            }
            RequestChange::Set(request) => {
                let at = index(requests, request.id)?;
                let old = std::mem::replace(&mut requests[at], request.clone());
                Ok(RequestChange::Set(old))
            }
            RequestChange::SetVariables(variables) => Ok(RequestChange::SetVariables(
                std::mem::replace(&mut self.0.variables, variables.clone()),
            )),
        }
    }
}

/// Kept as RON (`http_requests.ron`). Nothing there yet is no requests.
impl ByteModel for HttpRequests {
    fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.iter().all(u8::is_ascii_whitespace) {
            return Ok(Self::default());
        }
        ron::de::from_bytes(bytes).map_err(|e| e.to_string())
    }

    fn encode(&self, out: &mut dyn std::io::Write) -> std::io::Result<()> {
        let text = ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default())
            .map_err(std::io::Error::other)?;
        out.write_all(text.as_bytes())
    }
}

/// The changes that turn `old` into `new`: removals, then insertions in
/// order, then changed requests and variables. (Requests aren't reordered.)
pub fn changes(old: &Collection, new: &Collection) -> Vec<RequestChange> {
    let mut changes: Vec<RequestChange> = old
        .requests
        .iter()
        .filter(|request| new.get(request.id).is_none())
        .map(|request| RequestChange::Remove { id: request.id })
        .collect();
    for (at, request) in new.requests.iter().enumerate() {
        match old.get(request.id) {
            None => changes.push(RequestChange::Insert {
                at,
                request: request.clone(),
            }),
            Some(before) if before != request => changes.push(RequestChange::Set(request.clone())),
            Some(_) => {}
        }
    }
    if old.variables != new.variables {
        changes.push(RequestChange::SetVariables(new.variables.clone()));
    }
    changes
}

/// Editing in a panel: one undo step until a pause, or another kind of
/// change.
#[derive(Component, Default)]
pub(crate) struct Editing(Option<(DocumentUndoGroup, Instant)>);

/// The requests document `panel` shows, if it has one.
pub(crate) fn document(world: &World, panel: Entity) -> Option<DocumentHandle<HttpRequests>> {
    let id = world.get::<DocumentReference>(panel)?.document();
    world.bedtime().cast_document(id).ok()
}

/// The requests `panel` shows, once they're read.
pub(crate) fn collection(world: &World, panel: Entity) -> Option<&Collection> {
    let handle = document(world, panel)?;
    let info = world.bedtime().inspect_document(handle.id()).ok()?;
    if info.status != DocumentStatus::Ready {
        return None;
    }
    Some(&world.bedtime().read_document(handle).ok()?.0)
}

/// Give HTTP client panels the workspace's requests: restored, opened, or
/// after switching workspace.
pub(crate) fn attach_documents(
    world: &mut World,
    panels: &mut QueryState<Entity, (With<crate::HttpClientPanel>, Without<DocumentReference>)>,
) {
    let panels: Vec<Entity> = panels.iter(world).collect();
    for panel in panels {
        match world
            .bedtime_mut()
            .store_document::<HttpRequests, WorkbenchStore>(ENTRY, "HTTP Requests")
        {
            Ok(requests) => {
                world
                    .entity_mut(panel)
                    .insert(DocumentReference::of(requests));
            }
            Err(error) => warn!("The requests couldn't be opened: {error}"),
        }
    }
}

/// Make what `edited` changed, from `panel`'s requests, as one step
/// labelled `label`. `typing` (a field edited) joins the last step unless
/// there's been a pause.
pub(crate) fn edit(
    world: &mut World,
    panel: Entity,
    edited: &Collection,
    label: &str,
    typing: bool,
) -> Result<(), DocumentError> {
    let Some(handle) = document(world, panel) else {
        return Ok(());
    };
    let Some(current) = collection(world, panel) else {
        return Ok(());
    };
    let changes = changes(current, edited);
    if changes.is_empty() {
        return Ok(());
    }
    let id = handle.id();
    let revision = world.bedtime().inspect_document(id)?.revision;
    let now = Instant::now();
    let last = world.get::<Editing>(panel).and_then(|editing| editing.0);
    let options = match (typing, last) {
        (true, Some((group, at))) if now - at < PAUSE => EditOptions::grouped(label, group),
        (true, _) => {
            let group = world.bedtime_mut().begin_document_undo_group(id, panel)?;
            EditOptions::grouped(label, group)
        }
        (false, _) => EditOptions::new(label),
    };
    let group = options.group;
    let result =
        world
            .bedtime_mut()
            .edit_document_transaction(handle, revision, changes.clone(), options);
    match result {
        Ok(_) => {
            if let Some(mut editing) = world.get_mut::<Editing>(panel) {
                editing.0 = group.map(|group| (group, now));
            }
            Ok(())
        }
        // Something else changed the requests since (an undo): a new step.
        Err(DocumentError::StaleUndoGroup) if last.is_some() => {
            if let Some(mut editing) = world.get_mut::<Editing>(panel) {
                editing.0 = None;
            }
            edit(world, panel, edited, label, typing)
        }
        Err(error) => Err(error),
    }
}
