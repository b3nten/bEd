//! Shared save admission and revision receipts for local and remote documents.
use crate::{commit_document_drafts, document_revision, models};
use bedtime_api::documents::SavedDocument;
use bedtime_documents::{DocumentId, DocumentSession, SessionId, session::TickReport};
use bevy::prelude::*;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Mutex, mpsc},
};

#[derive(Clone, Debug)]
pub struct DocumentSaveTarget {
    pub document: DocumentId,
    pub destination: Option<PathBuf>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DocumentSaveId(u64);

struct CapturedSave {
    saved: SavedDocument,
    path: Option<String>,
    acknowledged: bool,
    settled: bool,
    model: Option<ModelWrite>,
    remote_destination: Option<RemoteDestination>,
}
struct RemoteDestination {
    original_path: String,
    revision: bedtime_api::documents::Revision,
    version: i32,
}
struct ModelWrite {
    capture: models::ModelSaveCapture,
    response: Mutex<mpsc::Receiver<Result<(), String>>>,
}
struct PendingSave {
    session: SessionId,
    targets: Vec<CapturedSave>,
    failure: Option<String>,
}
#[derive(Resource, Default)]
pub(crate) struct DocumentSaves {
    next: u64,
    pending: HashMap<DocumentSaveId, PendingSave>,
    results: HashMap<DocumentSaveId, Result<Vec<SavedDocument>, String>>,
    replies: HashMap<DocumentSaveId, (String, bedtime_api::documents::SaveToken)>,
}
pub(crate) fn saves_pending(world: &World) -> bool {
    !world.resource::<DocumentSaves>().pending.is_empty()
}
pub(crate) fn reserve_save_id(world: &mut World) -> DocumentSaveId {
    let mut saves = world.resource_mut::<DocumentSaves>();
    saves.next = saves.next.checked_add(1).expect("save id overflow");
    DocumentSaveId(saves.next)
}
pub(crate) fn take_save_result(
    world: &mut World,
    id: DocumentSaveId,
) -> Option<Result<Vec<SavedDocument>, String>> {
    world.resource_mut::<DocumentSaves>().results.remove(&id)
}
#[cfg(feature = "language-services")]
pub(crate) fn complete_save(
    world: &mut World,
    id: DocumentSaveId,
    result: Result<Vec<SavedDocument>, String>,
) {
    world
        .resource_mut::<DocumentSaves>()
        .results
        .insert(id, result);
    flush_save_replies(world);
}

pub(crate) fn attach_document_save_reply(
    world: &mut World,
    id: DocumentSaveId,
    recipient: String,
    token: bedtime_api::documents::SaveToken,
) {
    world
        .resource_mut::<DocumentSaves>()
        .replies
        .insert(id, (recipient, token));
    flush_save_replies(world);
}
fn flush_save_replies(world: &mut World) {
    let ready = world
        .resource::<DocumentSaves>()
        .replies
        .iter()
        .filter_map(|(id, (recipient, token))| {
            world
                .resource::<DocumentSaves>()
                .results
                .get(id)
                .cloned()
                .map(|result| (*id, recipient.clone(), *token, result))
        })
        .collect::<Vec<_>>();
    for (id, recipient, token, result) in ready {
        world.resource_mut::<DocumentSaves>().replies.remove(&id);
        world.write_message(bedtime_api::documents::DocumentReply::Save {
            recipient,
            token,
            result,
        });
    }
}

/// Admit every destination before starting writes. Results name the revision
/// written, even when a subsequent edit leaves the live document dirty.
pub fn begin_document_save(
    world: &mut World,
    targets: &[DocumentSaveTarget],
) -> Result<DocumentSaveId, String> {
    assert!(
        !bedtime_workbench::frontend_active(world),
        "saves resolve after drawing"
    );
    world.init_resource::<DocumentSaves>();
    let mut targets = targets.to_vec();
    targets.sort_by_key(|target| target.document);
    targets.dedup_by_key(|target| target.document);
    let ids = targets
        .iter()
        .map(|target| target.document)
        .collect::<Vec<_>>();
    commit_document_drafts(world, &ids)?;
    let session_id = world.non_send::<DocumentSession>().session_id();
    let mut captured = Vec::new();
    for target in &targets {
        if world
            .resource::<DocumentSaves>()
            .pending
            .values()
            .any(|save| {
                save.targets
                    .iter()
                    .any(|entry| entry.saved.document == target.document)
            })
        {
            return Err("Document already has a pending save".into());
        }
        if models::model_entity(world, target.document).is_some() {
            let capture =
                models::capture_model_save(world, target.document, target.destination.as_deref())?;
            captured.push((
                target.clone(),
                (0, capture.revision),
                match &capture.destination {
                    models::ModelSaveDestination::File(path) => {
                        Some(path.to_string_lossy().into_owned())
                    }
                    models::ModelSaveDestination::Writer { .. } => None,
                },
                Some(capture),
                None,
            ));
            continue;
        }
        let session = world.non_send::<DocumentSession>();
        let snapshot = session
            .snapshot(target.document)
            .map_err(|error| error.to_string())?;
        if session.is_snapshot_document(target.document) {
            return Err("Comparison documents cannot be saved".into());
        }
        if session.save_pending(target.document) {
            return Err("Document has a pending write".into());
        }
        if target.destination.is_none() && snapshot.disk_conflict.is_some() {
            return Err("Resolve the disk conflict before saving".into());
        }
        let destination = match &target.destination {
            Some(path) if session.is_remote() => path.clone(),
            Some(path) => session
                .check_rebind_path(target.document, path)
                .map_err(|error| error.to_string())?,
            None => PathBuf::from(&snapshot.path),
        };
        if destination.as_os_str().is_empty() {
            return Err("Save As is required before saving this document".into());
        }
        let mut revision = document_revision(world, target.document)?;
        let remote_destination =
            (session.is_remote() && target.destination.is_some()).then(|| RemoteDestination {
                original_path: snapshot.path.clone(),
                revision,
                version: snapshot.version,
            });
        let path = destination.to_string_lossy().into_owned();
        if remote_destination.is_none() && path != snapshot.path {
            revision = (revision.0.wrapping_add(1), revision.1.wrapping_add(1));
        }
        captured.push((
            DocumentSaveTarget {
                document: target.document,
                destination: target.destination.as_ref().map(|_| destination),
            },
            revision,
            Some(path),
            None,
            remote_destination,
        ));
    }
    let mut pending = PendingSave {
        session: session_id,
        targets: Vec::new(),
        failure: None,
    };
    for (target, revision, path, model, remote_destination) in captured {
        let model = if let Some(mut capture) = model {
            let bytes = capture.bytes.clone();
            let (sender, response) = mpsc::channel();
            match &capture.destination {
                models::ModelSaveDestination::Writer { key, writer } => {
                    let key = key.clone();
                    let writer = std::sync::Arc::clone(writer);
                    std::thread::spawn(move || {
                        let _ = sender.send(writer.write(&key, &bytes));
                    });
                }
                models::ModelSaveDestination::File(path) => {
                    let context = world.resource::<bedtime_files::FileContext>().clone();
                    let services = world
                        .resource::<bedtime_files::FileServices>()
                        .for_context(&context);
                    let location = context.location(path);
                    let root = if services.is_remote() {
                        context.directory.clone()
                    } else {
                        let Some(parent) = location.path.parent() else {
                            pending.failure = Some("Model destination has no parent".into());
                            break;
                        };
                        parent.to_path_buf()
                    };
                    capture.destination = models::ModelSaveDestination::File(location.path.clone());
                    std::thread::spawn(move || {
                        let result = (|| {
                            let baseline = match services.read(&root, &location) {
                                Ok((_, baseline)) => Some(baseline),
                                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                                Err(error) => return Err(error),
                            };
                            services
                                .write_atomic(&root, &location, &bytes, baseline.as_ref())
                                .map(|_| ())
                        })()
                        .map_err(|error| error.to_string());
                        let _ = sender.send(result);
                    });
                }
            }
            Some(ModelWrite {
                capture,
                response: Mutex::new(response),
            })
        } else {
            let result = match target.destination {
                Some(destination) => world
                    .non_send_mut::<DocumentSession>()
                    .save_as(target.document, &destination),
                None => world
                    .non_send_mut::<DocumentSession>()
                    .save(target.document),
            };
            if let Err(error) = result {
                pending.failure = Some(error.to_string());
                break;
            }
            None
        };
        pending.targets.push(CapturedSave {
            saved: SavedDocument {
                document: target.document,
                revision,
            },
            path,
            acknowledged: false,
            settled: false,
            model,
            remote_destination,
        });
    }
    let id = reserve_save_id(world);
    world
        .resource_mut::<DocumentSaves>()
        .pending
        .insert(id, pending);
    poll_document_saves(world, &TickReport::default());
    Ok(id)
}

pub fn poll_document_save(
    world: &mut World,
    id: DocumentSaveId,
) -> Option<Result<Vec<SavedDocument>, String>> {
    poll_document_saves(world, &TickReport::default());
    take_save_result(world, id)
}

pub(crate) fn poll_document_saves(world: &mut World, report: &TickReport) {
    let pending = std::mem::take(&mut world.resource_mut::<DocumentSaves>().pending);
    for (id, mut save) in pending {
        let same_session = world.non_send::<DocumentSession>().session_id() == save.session;
        if !same_session {
            save.failure
                .get_or_insert_with(|| "Document session changed while saving".into());
        }
        let mut complete = true;
        for target in &mut save.targets {
            if target.settled {
                continue;
            }
            let result = (|| {
                if let Some(write) = &target.model {
                    let received = write
                        .response
                        .lock()
                        .expect("model save response")
                        .try_recv();
                    match received {
                        Ok(result) => {
                            result?;
                            models::complete_model_save(world, &write.capture)?;
                            target.acknowledged = true;
                        }
                        Err(mpsc::TryRecvError::Empty) => {
                            return Ok(false);
                        }
                        Err(mpsc::TryRecvError::Disconnected) => {
                            return Err("Model save worker stopped".into());
                        }
                    }
                    target.model = None;
                    return Ok(true);
                }
                if !same_session {
                    return Err("Document session changed while saving".into());
                }
                if let Some(saved) = report.events.iter().find_map(|event| match event {
                    bedtime_documents::SessionEvent::Saved { document, save }
                        if *document == target.saved.document
                            && target.remote_destination.as_ref().map_or_else(
                                || Some(save.path.as_str()) == target.path.as_deref(),
                                |destination| save.version == destination.version,
                            ) =>
                    {
                        Some(save)
                    }
                    _ => None,
                }) {
                    if let Some(destination) = &target.remote_destination {
                        let rebound = report.events.iter().any(|event| matches!(event,
                            bedtime_documents::SessionEvent::PathChanged { document, previous, path }
                                if *document == target.saved.document && previous == &destination.original_path && path == &saved.path));
                        let revision = destination.revision;
                        target.saved.revision = if rebound {
                            (revision.0.wrapping_add(1), revision.1.wrapping_add(1))
                        } else {
                            revision
                        };
                        target.path = Some(saved.path.clone());
                    }
                    // This write reached storage. A later edit, autosave or
                    // transport disconnect cannot change that receipt.
                    target.acknowledged = true;
                    return Ok(true);
                }
                if let Some(error) = report.errors.iter().find(|error| {
                    error.document == Some(target.saved.document)
                        && error.service == bedtime_documents::ServiceKind::Save
                }) {
                    return Err(error.message.clone());
                }
                let session = world.non_send::<DocumentSession>();
                if session.save_pending(target.saved.document) {
                    return Ok(false);
                }
                if session.is_remote()
                    && !world.resource::<bedtime_files::FileServices>().connected()
                {
                    return Err("SSH disconnected before the save completed".into());
                }
                let snapshot = session
                    .snapshot(target.saved.document)
                    .map_err(|error| error.to_string())?;
                if Some(snapshot.path.as_str()) != target.path.as_deref() {
                    return Err("Document moved while saving".into());
                }
                if !target.acknowledged && (snapshot.dirty || snapshot.disk_conflict.is_some()) {
                    return Err(
                        "Document was not saved; resolve the file error and try again".into(),
                    );
                }
                target.acknowledged = true;
                Ok(true)
            })();
            match result {
                Ok(false) => complete = false,
                Ok(true) => target.settled = true,
                Err(error) => {
                    target.settled = true;
                    save.failure.get_or_insert(error);
                }
            }
        }
        if complete {
            let result = match save.failure {
                Some(error) => Err(error),
                None => Ok(save
                    .targets
                    .into_iter()
                    .map(|target| target.saved)
                    .collect()),
            };
            world
                .resource_mut::<DocumentSaves>()
                .results
                .insert(id, result);
        } else {
            // An aggregate failure does not cancel writes already admitted.
            // Keep their guard and acknowledge each checkpoint before returning
            // the aggregate receipt, including successful earlier model writes.
            world
                .resource_mut::<DocumentSaves>()
                .pending
                .insert(id, save);
        }
    }
    flush_save_replies(world);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::temp_dir;
    use crate::{DocumentHostPlugin, DocumentModel, create_model_document, edit_model};

    #[derive(Clone)]
    struct Point(i32);
    impl DocumentModel for Point {
        type Change = i32;
        fn apply(&mut self, change: &i32) -> Result<i32, String> {
            Ok(std::mem::replace(&mut self.0, *change))
        }
        fn encode(&self) -> Result<Vec<u8>, String> {
            Ok(self.0.to_string().into_bytes())
        }
    }

    #[test]
    fn application_writers_acknowledge_the_captured_model_checkpoint() {
        struct Writer {
            captured: mpsc::Sender<(String, Vec<u8>)>,
            release: Mutex<mpsc::Receiver<Result<(), String>>>,
        }
        impl models::DocumentWriter for Writer {
            fn write(&self, key: &str, bytes: &[u8]) -> Result<(), String> {
                self.captured.send((key.into(), bytes.to_vec())).unwrap();
                self.release.lock().unwrap().recv().unwrap()
            }
        }
        let mut app = App::new();
        app.add_plugins(DocumentHostPlugin);
        let world = app.world_mut();
        let document = create_model_document(world, Point(0), "Database query", None);
        let (captured_tx, captured_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        models::bind_model_document_writer(
            world,
            document,
            "query-1",
            std::sync::Arc::new(Writer {
                captured: captured_tx,
                release: Mutex::new(release_rx),
            }),
        )
        .unwrap();
        edit_model::<Point>(world, document, 0, 1, "Edit").unwrap();
        let save = begin_document_save(
            world,
            &[DocumentSaveTarget {
                document,
                destination: None,
            }],
        )
        .unwrap();
        assert_eq!(
            captured_rx
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap(),
            ("query-1".into(), b"1".to_vec())
        );
        edit_model::<Point>(world, document, 1, 2, "Later edit").unwrap();
        release_tx.send(Ok(())).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let saved = loop {
            if let Some(result) = poll_document_save(world, save) {
                break result.unwrap();
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        };
        assert_eq!(saved[0].revision, (0, 1));
        assert!(models::model_document(world, document).unwrap().dirty());
        models::undo_document(world, document).unwrap();
        assert!(!models::model_document(world, document).unwrap().dirty());
        assert!(
            models::model_document(world, document)
                .unwrap()
                .path
                .is_none()
        );

        edit_model::<Point>(world, document, 3, 9, "Edit before failure").unwrap();
        let save = begin_document_save(
            world,
            &[DocumentSaveTarget {
                document,
                destination: None,
            }],
        )
        .unwrap();
        captured_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        release_tx
            .send(Err("Metadata database is read-only".into()))
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            if let Some(result) = poll_document_save(world, save) {
                assert_eq!(result.unwrap_err(), "Metadata database is read-only");
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert!(models::model_document(world, document).unwrap().dirty());
        assert!(
            begin_document_save(
                world,
                &[DocumentSaveTarget {
                    document,
                    destination: Some("export.sql".into())
                }]
            )
            .is_err()
        );
    }

    #[test]
    fn a_discard_transition_drains_an_earlier_model_write_before_admission() {
        let mut app = App::new();
        app.add_plugins(DocumentHostPlugin);
        let world = app.world_mut();
        let document = create_model_document(world, Point(0), "Point", Some("point.model".into()));
        edit_model::<Point>(world, document, 0, 1, "Move").unwrap();
        let capture = models::capture_model_save(world, document, None).unwrap();
        let save = reserve_save_id(world);
        // Hold the asynchronous storage acknowledgement to exercise admission
        // deterministically, independently of filesystem scheduling.
        let (sender, response) = mpsc::channel();
        world.resource_mut::<DocumentSaves>().pending.insert(
            save,
            PendingSave {
                session: capture.session,
                failure: None,
                targets: vec![CapturedSave {
                    saved: SavedDocument {
                        document,
                        revision: (0, capture.revision),
                    },
                    path: match &capture.destination {
                        models::ModelSaveDestination::File(path) => {
                            Some(path.to_string_lossy().into_owned())
                        }
                        models::ModelSaveDestination::Writer { .. } => None,
                    },
                    acknowledged: false,
                    settled: false,
                    model: Some(ModelWrite {
                        capture,
                        response: Mutex::new(response),
                    }),
                    remote_destination: None,
                }],
            },
        );
        let barrier = crate::capture_operation_barrier(world, None);
        crate::resolve_documents(world);
        assert_eq!(
            crate::barrier_status(world, barrier),
            Some(crate::OperationBarrierStatus::Waiting)
        );
        assert_eq!(
            crate::prepare_document_transition(
                world,
                barrier,
                bedtime_documents::ClosePolicy::Discard
            )
            .unwrap(),
            crate::OperationBarrierStatus::Waiting
        );
        sender.send(Ok(())).unwrap();
        crate::poll_document_services(world);
        assert_eq!(
            crate::barrier_status(world, barrier),
            Some(crate::OperationBarrierStatus::Ready)
        );
        assert_eq!(
            poll_document_save(world, save).unwrap().unwrap()[0].revision,
            (0, 1)
        );
        assert!(!crate::document_dirty(world, document).unwrap());
        assert_eq!(
            crate::prepare_document_transition(
                world,
                barrier,
                bedtime_documents::ClosePolicy::Discard
            )
            .unwrap(),
            crate::OperationBarrierStatus::Prepared
        );
        crate::finish_operation_barrier(world, barrier, Ok(())).unwrap();
    }

    #[test]
    fn canonical_destination_receipts_use_save_facts_and_preserve_the_written_revision() {
        let temp = temp_dir::TempDir::new();
        let source = temp.write("original.txt", b"original");
        std::fs::create_dir(temp.root().join("nested")).unwrap();
        let mut app = App::new();
        app.add_plugins(DocumentHostPlugin);
        let world = app.world_mut();
        let document = world
            .non_send_mut::<DocumentSession>()
            .open_file(&source)
            .unwrap();
        // Repeated saves also cover divergence between content version and the
        // revision counter after a path rebind.
        for filename in ["original.txt", "copy.txt", "copy.txt"] {
            let before = crate::document_revision(world, document).unwrap();
            world
                .non_send_mut::<DocumentSession>()
                .apply_edits(
                    document,
                    before,
                    &[bedtime_documents::ByteEdit {
                        range: 0..0,
                        bytes: b"change ".to_vec(),
                    }],
                )
                .unwrap();
            let revision = crate::document_revision(world, document).unwrap();
            let snapshot = world
                .non_send::<DocumentSession>()
                .snapshot(document)
                .unwrap();
            let destination = temp.root().join("nested/..").join(filename);
            let save = reserve_save_id(world);
            let session = world.non_send::<DocumentSession>().session_id();
            world.resource_mut::<DocumentSaves>().pending.insert(
                save,
                PendingSave {
                    session,
                    failure: None,
                    targets: vec![CapturedSave {
                        saved: SavedDocument { document, revision },
                        path: Some(destination.to_string_lossy().into_owned()),
                        acknowledged: false,
                        settled: false,
                        model: None,
                        remote_destination: Some(RemoteDestination {
                            original_path: snapshot.path.clone(),
                            revision,
                            version: snapshot.version,
                        }),
                    }],
                },
            );
            // Local and SSH providers publish the same Saved/PathChanged
            // contract. Produce real save facts through the shared backend.
            world
                .non_send_mut::<DocumentSession>()
                .save_as(document, &destination)
                .unwrap();
            let persisted_revision = crate::document_revision(world, document).unwrap();
            world
                .non_send_mut::<DocumentSession>()
                .apply_edits(
                    document,
                    persisted_revision,
                    &[bedtime_documents::ByteEdit {
                        range: 0..0,
                        bytes: b"later ".to_vec(),
                    }],
                )
                .unwrap();
            let report = world.non_send_mut::<DocumentSession>().tick();
            poll_document_saves(world, &report);
            let receipt = poll_document_save(world, save).unwrap().unwrap();
            assert_eq!(receipt[0].revision, persisted_revision);
            assert!(crate::document_dirty(world, document).unwrap());
            assert_ne!(
                crate::document_revision(world, document).unwrap(),
                persisted_revision
            );
        }
    }
}
