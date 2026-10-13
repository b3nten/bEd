//! Explicit format-on-save is shared editing policy; lifecycle saves bypass it.
use crate::{
    DocumentSaveId, DocumentSaveTarget, begin_document_save, commit_document_drafts, saves,
};
use bed_api::documents::{DocumentId, LspRequestOrigin, Revision};
use bed_documents::DocumentSession;
use bed_lsp::{jsonrpc::RpcId, lsp_uri::LspUri};
use bevy::prelude::*;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{Mutex, mpsc},
    time::{Duration, Instant},
};

struct FormatRequest {
    ids: Vec<DocumentSaveId>,
    origin: LspRequestOrigin,
    revision: Revision,
    rpc: RpcId,
    response: Mutex<mpsc::Receiver<Result<Value, String>>>,
    started: Instant,
    autosave_was_paused: bool,
    temporary_view: bool,
}
struct FormatWrite {
    ids: Vec<DocumentSaveId>,
    save: DocumentSaveId,
}
#[derive(Resource, Default)]
pub(crate) struct ExplicitDocumentSaves {
    formats: Vec<FormatRequest>,
    writes: Vec<FormatWrite>,
    warnings: HashMap<DocumentSaveId, String>,
}

pub(crate) fn formatting_pending(world: &World) -> bool {
    world
        .get_resource::<ExplicitDocumentSaves>()
        .is_some_and(|saves| !saves.formats.is_empty() || !saves.writes.is_empty())
}

pub fn begin_explicit_document_save(
    world: &mut World,
    document: DocumentId,
    format: bool,
) -> Result<DocumentSaveId, String> {
    world.init_resource::<ExplicitDocumentSaves>();
    if !format || crate::models::model_entity(world, document).is_some() {
        return begin_document_save(
            world,
            &[DocumentSaveTarget {
                document,
                destination: None,
            }],
        );
    }
    let ready = {
        let session = world.non_send::<DocumentSession>();
        session
            .document_kind(document)
            .map_err(|error| error.to_string())?
            == bed_documents::DocumentKind::Text
            && !session.is_snapshot_document(document)
            && session
                .lsp()
                .and_then(|pool| pool.client_for_document(document))
                .is_some_and(|client| {
                    let client = client.borrow();
                    client.is_initialized() && client.supports_method("textDocument/formatting")
                })
    };
    if !ready {
        return begin_document_save(
            world,
            &[DocumentSaveTarget {
                document,
                destination: None,
            }],
        );
    }
    let id = saves::reserve_save_id(world);
    if let Some(pending) = world
        .resource_mut::<ExplicitDocumentSaves>()
        .formats
        .iter_mut()
        .find(|pending| pending.origin.document_id == document)
    {
        pending.ids.push(id);
        return Ok(id);
    }
    commit_document_drafts(world, &[document])?;
    let mut session = world.non_send_mut::<DocumentSession>();
    let snapshot = session
        .snapshot(document)
        .map_err(|error| error.to_string())?;
    if snapshot.path.is_empty() || snapshot.disk_conflict.is_some() {
        return begin_document_save(
            world,
            &[DocumentSaveTarget {
                document,
                destination: None,
            }],
        );
    }
    let existing = session.view_ids(document).into_iter().next();
    let view = existing
        .map_or_else(|| session.create_view(document), Ok)
        .map_err(|error| error.to_string())?;
    let revision = session
        .document_revision(document)
        .map_err(|error| error.to_string())?;
    let origin = session
        .lsp()
        .and_then(|pool| pool.request_origin(document, view, snapshot.generation, 0))
        .ok_or("Formatting document is no longer bound to a server")?;
    let uri = if session.is_remote() {
        LspUri::file_uri_from_remote_path(&snapshot.path)
    } else {
        LspUri::file_uri_from_path(&snapshot.path)
    }
    .map_err(|error| error.to_string())?;
    let params =
        json!({"textDocument":{"uri":uri.to_string()},"options":{"tabSize":4,"insertSpaces":true}});
    let (sender, response) = mpsc::channel();
    let rpc = session.lsp_mut().unwrap().send_request(
        origin,
        "textDocument/formatting",
        params,
        move |reply| {
            let _ = sender.send(reply.result.map_err(|error| error.to_string()));
        },
    );
    let rpc = match rpc {
        Ok(rpc) => rpc,
        Err(error) => {
            if existing.is_none() {
                session.detach_view(view);
            }
            warning(
                world,
                &[id],
                format!("Formatting failed; saving current text: {error}"),
            );
            let save = begin_document_save(
                world,
                &[DocumentSaveTarget {
                    document,
                    destination: None,
                }],
            )?;
            world
                .resource_mut::<ExplicitDocumentSaves>()
                .writes
                .push(FormatWrite {
                    ids: vec![id],
                    save,
                });
            return Ok(id);
        }
    };
    let autosave_was_paused = session
        .autosave_paused(document)
        .map_err(|error| error.to_string())?;
    session
        .pause_autosave(document, true)
        .map_err(|error| error.to_string())?;
    world
        .resource_mut::<ExplicitDocumentSaves>()
        .formats
        .push(FormatRequest {
            ids: vec![id],
            origin,
            revision,
            rpc,
            response: Mutex::new(response),
            started: Instant::now(),
            autosave_was_paused,
            temporary_view: existing.is_none(),
        });
    Ok(id)
}
pub fn take_document_save_warning(world: &mut World, id: DocumentSaveId) -> Option<String> {
    world
        .resource_mut::<ExplicitDocumentSaves>()
        .warnings
        .remove(&id)
}
fn warning(world: &mut World, ids: &[DocumentSaveId], message: String) {
    for &id in ids {
        world
            .resource_mut::<ExplicitDocumentSaves>()
            .warnings
            .insert(id, message.clone());
    }
    world.write_message(crate::CoreNotice {
        source: None,
        message,
    });
}
fn release(world: &mut World, pending: &FormatRequest) {
    let mut session = world.non_send_mut::<DocumentSession>();
    if session.session_id() != pending.origin.session_id {
        return;
    }
    if let Some(client) = session
        .lsp()
        .and_then(|pool| pool.client_for_document(pending.origin.document_id))
    {
        let _ = client.borrow_mut().cancel_request(&pending.rpc);
    }
    let _ = session.pause_autosave(pending.origin.document_id, pending.autosave_was_paused);
    if pending.temporary_view {
        session.detach_view(pending.origin.view_id);
    }
}
pub(crate) fn poll_explicit_document_saves(world: &mut World) {
    let requests = std::mem::take(&mut world.resource_mut::<ExplicitDocumentSaves>().formats);
    for pending in requests {
        let current = world
            .non_send::<DocumentSession>()
            .accepts_lsp_origin(&pending.origin);
        let reply = match pending.response.lock().unwrap().try_recv() {
            Ok(reply) => Some(reply),
            Err(mpsc::TryRecvError::Disconnected) => Some(Err("Server disconnected".into())),
            Err(mpsc::TryRecvError::Empty)
                if pending.started.elapsed() >= Duration::from_secs(3) =>
            {
                Some(Err("Formatting timed out".into()))
            }
            Err(mpsc::TryRecvError::Empty) => None,
        };
        if reply.is_none() && current {
            world
                .resource_mut::<ExplicitDocumentSaves>()
                .formats
                .push(pending);
            continue;
        }
        let document = pending.origin.document_id;
        let same_session =
            world.non_send::<DocumentSession>().session_id() == pending.origin.session_id;
        if same_session && let Err(error) = commit_document_drafts(world, &[document]) {
            release(world, &pending);
            for id in pending.ids {
                saves::complete_save(world, id, Err(error.clone()));
            }
            continue;
        }
        // A panel draft may have advanced the canonical revision while the
        // formatter was pending. Its old response must not overwrite it.
        let current = world
            .non_send::<DocumentSession>()
            .accepts_lsp_origin(&pending.origin);
        let result = if current {
            match reply.unwrap() {
                Ok(edits) => world
                    .non_send_mut::<DocumentSession>()
                    .apply_lsp_text_edits(document, pending.revision, &edits)
                    .map_err(|error| error.to_string()),
                Err(error) => Err(error),
            }
        } else {
            Ok(())
        };
        release(world, &pending);
        if let Err(error) = result {
            warning(
                world,
                &pending.ids,
                format!("Formatting failed; saving current text: {error}"),
            );
        }
        let save = if same_session {
            begin_document_save(
                world,
                &[DocumentSaveTarget {
                    document,
                    destination: None,
                }],
            )
        } else {
            Err("Document session changed while formatting".into())
        };
        match save {
            Ok(save) => world
                .resource_mut::<ExplicitDocumentSaves>()
                .writes
                .push(FormatWrite {
                    ids: pending.ids,
                    save,
                }),
            Err(error) => {
                for id in pending.ids {
                    saves::complete_save(world, id, Err(error.clone()));
                }
            }
        }
    }
    let writes = std::mem::take(&mut world.resource_mut::<ExplicitDocumentSaves>().writes);
    for pending in writes {
        if let Some(result) = saves::take_save_result(world, pending.save) {
            for id in pending.ids {
                saves::complete_save(world, id, result.clone());
            }
        } else {
            world
                .resource_mut::<ExplicitDocumentSaves>()
                .writes
                .push(pending);
        }
    }
}
pub fn cancel_explicit_document_saves(world: &mut World) {
    let requests = std::mem::take(&mut world.resource_mut::<ExplicitDocumentSaves>().formats);
    for pending in requests {
        release(world, &pending);
        for id in pending.ids {
            saves::complete_save(
                world,
                id,
                Err("Document session changed while formatting".into()),
            );
        }
    }
}
