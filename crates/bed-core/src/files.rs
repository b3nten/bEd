//! File operations retain their captured origin while asynchronous I/O completes.
use crate::*;
use bed_api::features::{HostAction, HostRequest};
use bed_documents::DocumentSession;
use bed_files::{FileContext, FileLocation, FileProviderId, FileServices};
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
    opened: HashMap<OperationBarrierToken, Entity>,
}
pub(crate) fn take_opened(world: &mut World, token: OperationBarrierToken) -> Option<Entity> {
    world.resource_mut::<FileOperations>().opened.remove(&token)
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
    let path = match request.action() {
        HostAction::OpenFile { path, .. }
        | HostAction::OpenFileIn { path, .. }
        | HostAction::PreviewFileIn { path, .. }
        | HostAction::RevealSource { path, .. }
        | HostAction::PreviewSource { path, .. }
        | HostAction::DropFile { path } => std::path::PathBuf::from(path),
        HostAction::OpenTerminalLink { path, .. } => path.clone(),
        #[cfg(feature = "language-services")]
        HostAction::NavigateLocation { path, .. } => std::path::PathBuf::from(path),
        _ => return Ok(None),
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
                if let HostOutcome::Opened(entity) = outcome {
                    world
                        .resource_mut::<FileOperations>()
                        .opened
                        .insert(*token, entity);
                }
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
                if pending.context.as_ref().unwrap().provider == FileProviderId::Local {
                    let context = pending.context.as_ref().unwrap();
                    let documents = crate::transitions::transition_documents(world);
                    crate::commit_document_drafts(world, &documents)?;
                    let mut options = world.non_send::<DocumentSession>().options().clone();
                    options.project_root = Some(context.directory.clone());
                    options.persistent_history = false;
                    let services = world.resource::<FileServices>().clone();
                    world
                        .non_send_mut::<DocumentSession>()
                        .configure_file_services(options, context, &services)
                        .map_err(|error| error.to_string())?;
                    world.resource_mut::<FileContext>().replace(context.clone());
                    finish_operation_barrier(world, pending.token, Ok(()))?;
                    return Ok(true);
                }
                if !pending.prepared {
                    let policy = world.resource::<DocumentClosePolicy>().0;
                    match prepare_document_transition(world, pending.token, policy)? {
                        OperationBarrierStatus::NeedsDecision(_) => {
                            return Err(
                                "Save or discard documents before changing directories".into()
                            );
                        }
                        OperationBarrierStatus::Pending | OperationBarrierStatus::Prepared => {
                            pending.prepared = true
                        }
                        _ => return Err("Directory transition could not prepare documents".into()),
                    }
                }
                match poll_document_transition(world, pending.token)? {
                    OperationBarrierStatus::Pending => return Ok(false),
                    OperationBarrierStatus::Failed(error) => return Err(error),
                    OperationBarrierStatus::Prepared => {}
                    _ => return Err("Directory transition lost its preflight".into()),
                }
                validate_document_transition(world, pending.token)?;
                let context = pending.context.as_ref().unwrap();
                let mut options = world.non_send::<DocumentSession>().options().clone();
                options.project_root = Some(context.directory.clone());
                options.persistent_history = false;
                let session = DocumentSession::with_file_services(
                    options,
                    context,
                    world.resource::<FileServices>(),
                )
                .map_err(|error| error.to_string())?;
                let panels = world
                    .query_filtered::<Entity, With<bed_workbench::PanelRecord>>()
                    .iter(world)
                    .filter(|entity| {
                        world
                            .get::<bed_api::workbench::PanelPresentation>(*entity)
                            .is_some()
                    })
                    .collect::<Vec<_>>();
                if !bed_workbench::request_close_many(world, &panels)? {
                    return Err("Panels are not ready to change directories".into());
                }
                commit_context_change(world, pending.token, session)?;
                world.resource_mut::<FileContext>().replace(context.clone());
                *world.resource_mut::<bed_workbench::WorkbenchLayout>() =
                    bed_workbench::WorkbenchLayout::default();
                bed_workbench::advance_layout_epoch(world);
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
