//! File operations retain their captured origin while asynchronous I/O completes.
use crate::*;
use bedtime_api::features::{HostAction, HostRequest};
use bedtime_documents::DocumentSession;
use bedtime_files::{FileContext, FileLocation, FileProviderId, FileServices};
use bevy::prelude::*;
use std::{
    collections::HashMap,
    sync::{Mutex, mpsc},
};

struct DirectoryOperation {
    token: OperationBarrierToken,
    response: Mutex<mpsc::Receiver<Result<FileContext, String>>>,
    context: Option<FileContext>,
    prepared: bool,
}
enum FileOperation {
    Directory(DirectoryOperation),
    Open {
        token: OperationBarrierToken,
        request: HostRequest,
        path: std::path::PathBuf,
    },
}
#[derive(Resource, Default)]
pub(crate) struct FileOperations {
    active: Option<FileOperation>,
    outcomes: HashMap<OperationBarrierToken, HostOutcome>,
    restores: Vec<DocumentRestore>,
}

struct DocumentRestore {
    session: bedtime_documents::SessionId,
    panel: Entity,
    record: serde_json::Value,
    path: std::path::PathBuf,
}

pub(crate) fn queue_document_restore(
    world: &mut World,
    record: serde_json::Value,
    area: Option<u32>,
) -> Result<Entity, String> {
    let path = std::path::PathBuf::from(
        record["path"]
            .as_str()
            .ok_or("Saved document has no path")?,
    );
    world
        .non_send_mut::<DocumentSession>()
        .request_open_file_auto(&path)
        .map_err(|error| error.to_string())?;
    let panel = bedtime_workbench::spawn_panel(world, record.clone(), area, record["id"].as_u64())?;
    world
        .get_mut::<bedtime_workbench::PanelRecord>(panel)
        .unwrap()
        .unavailable = Some("Opening remote document…".into());
    let session = world.non_send::<DocumentSession>().session_id();
    world
        .entity_mut(panel)
        .insert(bedtime_api::documents::PanelBinding {
            session_id: session.0,
            viewer: record["viewer"].as_str().map(bedtime_api::ViewerId::from),
            ..Default::default()
        });
    world
        .resource_mut::<FileOperations>()
        .restores
        .push(DocumentRestore {
            session,
            panel,
            record,
            path,
        });
    Ok(panel)
}

fn poll_document_restores(world: &mut World) {
    let pending = std::mem::take(&mut world.resource_mut::<FileOperations>().restores);
    for restore in pending {
        if restore.session != world.non_send::<DocumentSession>().session_id()
            || !world.entities().contains(restore.panel)
        {
            continue;
        }
        let result = if let Some(error) = world
            .non_send_mut::<DocumentSession>()
            .take_open_file_error(&restore.path)
        {
            Err(error.to_string())
        } else {
            world
                .non_send_mut::<DocumentSession>()
                .request_open_file_auto(&restore.path)
                .map_err(|error| error.to_string())
        };
        match result {
            Ok(None) => world
                .resource_mut::<FileOperations>()
                .restores
                .push(restore),
            Ok(Some(document)) => {
                let binding = bedtime_api::documents::PanelBinding {
                    input: bedtime_api::documents::PanelInput::Document(document),
                    document: Some(document),
                    viewer: restore.record["viewer"]
                        .as_str()
                        .map(bedtime_api::ViewerId::from),
                    session_id: restore.session.0,
                    ..Default::default()
                };
                world.entity_mut(restore.panel).insert(binding.clone());
                let kind = restore.record["panel_type"]
                    .as_str()
                    .expect("validated panel type");
                world
                    .get_mut::<bedtime_workbench::PanelRecord>(restore.panel)
                    .unwrap()
                    .unavailable = None;
                if let Err(error) = crate::documents::mount_panel_frontend(
                    world,
                    restore.panel,
                    &bedtime_api::PanelTypeId::from(kind),
                    binding,
                    restore.record["state"].clone(),
                ) {
                    world.write_message(crate::CoreNotice {
                        source: Some(restore.panel),
                        message: error,
                    });
                }
            }
            Err(error) => {
                world
                    .get_mut::<bedtime_workbench::PanelRecord>(restore.panel)
                    .unwrap()
                    .unavailable = Some(error.clone());
                world.write_message(crate::CoreNotice {
                    source: Some(restore.panel),
                    message: error,
                });
            }
        }
    }
}
pub(crate) fn take_outcome(world: &mut World, token: OperationBarrierToken) -> Option<HostOutcome> {
    world
        .resource_mut::<FileOperations>()
        .outcomes
        .remove(&token)
}

pub(crate) fn pending_open(world: &World) -> Option<&HostRequest> {
    match &world.resource::<FileOperations>().active {
        Some(FileOperation::Open { request, .. }) => Some(request),
        _ => None,
    }
}

pub(crate) fn is_preview(request: &HostRequest) -> bool {
    matches!(
        request.action(),
        HostAction::PreviewFileIn { .. } | HostAction::PreviewSource { .. }
    )
}

fn open_path(request: &HostRequest) -> Option<std::path::PathBuf> {
    Some(match request.action() {
        HostAction::OpenFile { path, .. }
        | HostAction::OpenFileInstance { path, .. }
        | HostAction::OpenFileIn { path, .. }
        | HostAction::PreviewFileIn { path, .. }
        | HostAction::RevealSource { path, .. }
        | HostAction::PreviewSource { path, .. }
        | HostAction::DropFile { path } => path.into(),
        HostAction::OpenTerminalLink { path, .. } => path.clone(),
        #[cfg(feature = "language-services")]
        HostAction::NavigateLocation { path, .. } => path.into(),
        _ => return None,
    })
}

fn requested_viewer(request: &HostRequest) -> Option<&str> {
    match request.action() {
        HostAction::OpenFile { viewer, .. }
        | HostAction::OpenFileIn { viewer, .. }
        | HostAction::OpenFileInstance { viewer, .. }
        | HostAction::PreviewFileIn { viewer, .. } => viewer.as_deref(),
        _ => None,
    }
}

#[cfg(test)]
#[test]
fn a_kept_preview_upgrade_preserves_other_viewers_and_explicit_instances() {
    let capture =
        |action| HostRequest::capture(bedtime_documents::SessionId(1), None, action, 1, 1);
    let preview = capture(HostAction::PreviewFileIn {
        path: "shared.txt".into(),
        viewer: Some("text".into()),
        target: bedtime_api::workbench::PanelTarget::LargestArea,
    });
    let kept = |viewer: &str, additional| {
        capture(HostAction::OpenFileInstance {
            path: "shared.txt".into(),
            viewer: Some(viewer.into()),
            additional,
        })
    };
    assert!(supersedes_preview(&preview, &kept("text", false)));
    assert!(!supersedes_preview(&preview, &kept("structured", false)));
    assert!(!supersedes_preview(&preview, &kept("text", true)));
}

pub(crate) fn supersedes_preview(previous: &HostRequest, next: &HostRequest) -> bool {
    is_preview(previous)
        && previous.session_id() == next.session_id()
        && previous.layout_epoch() == next.layout_epoch()
        && previous.area() == next.area()
        && (is_preview(next)
            || matches!(
                next.action(),
                HostAction::OpenFile { .. }
                    | HostAction::OpenFileIn { .. }
                    | HostAction::OpenFileInstance {
                        additional: false,
                        ..
                    }
            ) && open_path(previous) == open_path(next)
                && requested_viewer(previous) == requested_viewer(next))
}

pub(crate) fn cancel_pending_open(
    world: &mut World,
    next: Option<&HostRequest>,
) -> Result<(), String> {
    let Some(FileOperation::Open { token, path, .. }) =
        world.resource_mut::<FileOperations>().active.take()
    else {
        return Err("No asynchronous file open is active".into());
    };
    if next.and_then(open_path).as_ref() != Some(&path) {
        world
            .non_send_mut::<DocumentSession>()
            .cancel_open_file(&path);
        // Session polling may already have delivered a clean read this frame.
        // It has no view until the captured open is committed.
        if let Some(document) = world.non_send::<DocumentSession>().document_for_path(&path)
            && world.non_send::<DocumentSession>().view_count(document) == 0
            && !crate::document_dirty(world, document).unwrap_or(true)
        {
            crate::retire_document(world, document)?;
        }
    }
    if next.is_some() {
        world
            .resource_mut::<FileOperations>()
            .outcomes
            .insert(token, HostOutcome::Ignored);
        finish_operation_barrier(world, token, Ok(()))
    } else {
        finish_operation_barrier(
            world,
            token,
            Err("Originating layout is no longer active".into()),
        )
    }
}

pub(crate) fn open_directory(
    world: &mut World,
    location: FileLocation,
) -> Result<OperationBarrierToken, String> {
    let services = world.resource::<FileServices>().clone();
    if services.provider_id() != &location.provider {
        return Err("Directory belongs to a different filesystem provider".into());
    }
    let root = location.path.clone();
    let token = create_operation_barrier(world);
    crate::transitions::activate_operation_barrier(world, token)?;
    let (sender, response) = mpsc::channel();
    std::thread::spawn(move || {
        let result = (|| {
            let location = services.canonicalize(&root, &location, false)?;
            match &location.provider {
                FileProviderId::Local => FileContext::local(location.path),
                FileProviderId::Ssh { host } => {
                    services.read_directory(
                        &root.to_string_lossy(),
                        &location.path.to_string_lossy(),
                        false,
                    )?;
                    FileContext::remote(host.clone(), location.path)
                }
            }
        })()
        .map_err(|error| error.to_string());
        let _ = sender.send(result);
    });
    world.resource_mut::<FileOperations>().active =
        Some(FileOperation::Directory(DirectoryOperation {
            token,
            response: Mutex::new(response),
            context: None,
            prepared: false,
        }));
    Ok(token)
}

/// Start an SSH open once; the document session's existing worker owns its I/O.
pub(crate) fn defer_remote_open(
    world: &mut World,
    request: &HostRequest,
) -> Result<Option<OperationBarrierToken>, String> {
    if !world.non_send::<DocumentSession>().is_remote() {
        return Ok(None);
    }
    let Some(path) = open_path(request) else {
        return Ok(None);
    };
    if world
        .non_send_mut::<DocumentSession>()
        .request_open_file_auto(&path)
        .map_err(|error| error.to_string())?
        .is_some()
    {
        return Ok(None);
    }
    let token = create_operation_barrier(world);
    crate::transitions::activate_operation_barrier(world, token)?;
    world.resource_mut::<FileOperations>().active = Some(FileOperation::Open {
        token,
        request: request.clone(),
        path,
    });
    Ok(Some(token))
}
pub(crate) fn poll_file_operations(world: &mut World) {
    poll_document_restores(world);
    let Some(mut operation) = world.resource_mut::<FileOperations>().active.take() else {
        return;
    };
    let token = match &operation {
        FileOperation::Directory(pending) => pending.token,
        FileOperation::Open { token, .. } => *token,
    };
    if barrier_status(world, token) == Some(OperationBarrierStatus::Waiting) {
        world.resource_mut::<FileOperations>().active = Some(operation);
        return;
    }
    let result = match &mut operation {
        FileOperation::Open {
            token,
            request,
            path,
        } => {
            let result = (|| {
                if request.session_id() != world.non_send::<DocumentSession>().session_id() {
                    return Err("Document session was replaced".into());
                }
                if let Some(error) = world
                    .non_send_mut::<DocumentSession>()
                    .take_open_file_error(path)
                {
                    return Err(error.to_string());
                }
                if world
                    .non_send_mut::<DocumentSession>()
                    .request_open_file_auto(path)
                    .map_err(|error| error.to_string())?
                    .is_none()
                {
                    return Ok(false);
                }
                let outcome = apply_document_request(world, request.clone())?;
                world
                    .resource_mut::<FileOperations>()
                    .outcomes
                    .insert(*token, outcome);
                finish_operation_barrier(world, *token, Ok(()))?;
                Ok(true)
            })();
            result.map_err(|error| (*token, error))
        }
        FileOperation::Directory(pending) => {
            let result = (|| {
                if pending.context.is_none() {
                    match pending.response.lock().unwrap().try_recv() {
                        Ok(result) => pending.context = Some(result?),
                        Err(mpsc::TryRecvError::Empty) => return Ok(false),
                        Err(mpsc::TryRecvError::Disconnected) => {
                            return Err("Directory preparation worker stopped".into());
                        }
                    }
                }
                let context = pending.context.as_ref().unwrap().clone();
                let status = if !pending.prepared {
                    let mut options = world.non_send::<DocumentSession>().options().clone();
                    options.history_path = None;
                    let change = crate::ContextChange {
                        mode: if context.provider == FileProviderId::Local {
                            crate::ContextChangeMode::RetainDocuments
                        } else {
                            crate::ContextChangeMode::ReplaceDocuments
                        },
                        context: context.clone(),
                        services: world.resource::<FileServices>().clone(),
                        options,
                    };
                    pending.prepared = true;
                    crate::prepare_context_change(world, pending.token, change)?
                } else {
                    crate::poll_context_change(world, pending.token)?
                };
                match status {
                    OperationBarrierStatus::Waiting
                    | OperationBarrierStatus::NeedsDecision(_)
                    | OperationBarrierStatus::Pending => return Ok(false),
                    OperationBarrierStatus::Failed(error) => return Err(error),
                    OperationBarrierStatus::Prepared => {}
                    _ => return Err("Directory transition lost its preflight".into()),
                }
                crate::commit_prepared_context_change(world, pending.token)?;
                finish_operation_barrier(world, pending.token, Ok(()))?;
                Ok(true)
            })();
            result.map_err(|error| (pending.token, error))
        }
    };
    match result {
        Ok(false) => world.resource_mut::<FileOperations>().active = Some(operation),
        Ok(true) => {}
        Err((token, error)) => {
            let _ = finish_operation_barrier(world, token, Err(error));
        }
    }
}

/// The consuming application locates packaged SSH helpers once at startup.
#[derive(Resource, Clone)]
pub struct RemoteHelperDirectory(pub std::path::PathBuf);

#[cfg(feature = "remote")]
pub fn begin_remote_context(
    world: &World,
    host: String,
    root: String,
) -> std::io::Result<mpsc::Receiver<std::io::Result<(FileContext, FileServices)>>> {
    use std::io;
    if host.trim().is_empty()
        || !(root.starts_with('/') || root == "~" || root.starts_with("~/"))
        || root.contains('\0')
    {
        return Err(io::Error::other(
            "SSH context requires a host and absolute directory",
        ));
    }
    let helpers = world
        .get_resource::<RemoteHelperDirectory>()
        .ok_or_else(|| io::Error::other("Packaged SSH helper directory is not configured"))?
        .0
        .clone();
    let (sender, receiver) = mpsc::channel();
    std::thread::Builder::new()
        .name("bedtime-ssh-connect".into())
        .spawn(move || {
            let result = (|| {
                let target = bedtime_remote::SshTarget::new(host.clone());
                let root = bedtime_remote::expand_ssh_path(&target, &root)?;
                let target = bedtime_remote::prepare_ssh_target(&target, &helpers)?;
                let client = bedtime_remote::RemoteClient::launch_ssh(&target)?;
                let bedtime_remote::Response::Path { path } =
                    client.call(bedtime_remote::Request::Canonicalize {
                        root,
                        path: ".".into(),
                        allow_missing: false,
                    })?
                else {
                    return Err(io::Error::other("Unexpected remote validation response"));
                };
                if !matches!(
                    client.call(bedtime_remote::Request::ReadDirectory {
                        root: path.clone(),
                        path: path.clone(),
                        classify_gitignored: false,
                    })?,
                    bedtime_remote::Response::Directory { .. }
                ) {
                    return Err(io::Error::other("Remote context is not a directory"));
                }
                Ok((
                    FileContext::remote(host, std::path::PathBuf::from(path))?,
                    FileServices::remote_target(target, client),
                ))
            })();
            let _ = sender.send(result);
        })?;
    Ok(receiver)
}
