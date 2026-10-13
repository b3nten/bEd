//! Optional document operations use the application's captured transition queue.
use super::*;
use bed_api::{documents::Revision, features::HostAction, workbench::PanelTarget};
use bed_app::{
    AppAction, AppRequestId, AppRequestOutcome, AppRequests, WindowCommand, capture_app_request,
    capture_app_request_from, poll_app_request_result, submit_app_request,
};
use bed_documents::session::{ByteEdit, DocumentId, DocumentSession};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DocumentHandle {
    app: u64,
    session_id: u64,
    document: DocumentId,
}

impl DocumentHandle {
    pub fn id(self) -> DocumentId {
        self.document
    }
}

#[derive(Clone, Debug)]
pub struct DocumentInfo {
    pub handle: DocumentHandle,
    pub path: String,
    pub kind: bed_documents::session::DocumentKind,
    pub revision: Revision,
    pub dirty: bool,
    pub disk_conflict: Option<String>,
}

#[derive(Clone, Debug)]
pub struct DocumentRead {
    pub info: DocumentInfo,
    pub bytes: Vec<u8>,
}

#[derive(Debug)]
pub enum AppOperation {
    OpenFile {
        path: PathBuf,
        viewer: Option<String>,
        additional: bool,
    },
    NewDocument,
    ReadDocument(DocumentHandle),
    ApplyEdits {
        document: DocumentHandle,
        revision: Revision,
        edits: Vec<ByteEdit>,
    },
    Save(Vec<DocumentHandle>),
    SaveAs {
        document: DocumentHandle,
        path: PathBuf,
    },
    Undo(DocumentHandle),
    Redo(DocumentHandle),
    SwitchViewer {
        panel: PanelHandle,
        viewer: String,
    },
    AttachWorkspace(PathBuf),
    SetWorkspace(bed_api::application::WorkspaceSpec),
    SetSetting {
        key: String,
        value: Value,
    },
    Quit,
}

pub(super) enum PendingApp {
    Request(AppRequestId),
    Read(DocumentHandle),
}

fn document_handle(world: &World, document: DocumentId) -> DocumentHandle {
    DocumentHandle {
        app: world.resource::<AutomationScope>().0,
        session_id: session_id(world),
        document,
    }
}

fn resolve_document(
    world: &World,
    document: DocumentHandle,
) -> Result<DocumentId, AutomationError> {
    let owner = world.get_non_send::<DocumentSession>().ok_or_else(|| {
        AutomationError::new(ErrorKind::Unavailable, "document session is not installed")
    })?;
    if document.app != world.resource::<AutomationScope>().0
        || document.session_id != owner.session_id().0
        || !owner.document_ids().contains(&document.document)
    {
        return Err(AutomationError::new(
            ErrorKind::StaleHandle,
            "document belongs to a retired application/session or has closed",
        ));
    }
    Ok(document.document)
}

pub(super) fn start(
    world: &mut World,
    operation: AppOperation,
) -> Result<PendingApp, AutomationError> {
    if !world.contains_resource::<AppRequests>() {
        return Err(AutomationError::new(
            ErrorKind::Unavailable,
            "editor transitions are not installed",
        ));
    }
    let action = match operation {
        AppOperation::ReadDocument(document) => {
            resolve_document(world, document)?;
            return Ok(PendingApp::Read(document));
        }
        AppOperation::OpenFile {
            path,
            viewer,
            additional,
        } => AppAction::OpenFileWithViewer {
            path,
            viewer,
            additional,
            move_existing: false,
        },
        AppOperation::NewDocument => AppAction::Command(WindowCommand::NewDocument),
        AppOperation::ApplyEdits {
            document,
            revision,
            edits,
        } => AppAction::ApplyEdits {
            document: resolve_document(world, document)?,
            revision,
            edits,
        },
        AppOperation::Save(documents) => AppAction::SaveDocuments(
            documents
                .into_iter()
                .map(|document| resolve_document(world, document))
                .collect::<Result<_, _>>()?,
        ),
        AppOperation::SaveAs { document, path } => AppAction::SaveAs {
            document: resolve_document(world, document)?,
            path,
        },
        AppOperation::Undo(document) => AppAction::HostAction(HostAction::Undo {
            document: resolve_document(world, document)?,
        }),
        AppOperation::Redo(document) => AppAction::HostAction(HostAction::Redo {
            document: resolve_document(world, document)?,
        }),
        AppOperation::SwitchViewer { panel, viewer } => AppAction::SwitchViewer {
            panel: resolve(world, panel)?,
            viewer,
        },
        AppOperation::AttachWorkspace(path) => AppAction::AttachWorkspace(path),
        AppOperation::SetWorkspace(workspace) => AppAction::SetWorkspace(workspace),
        AppOperation::SetSetting { key, value } => AppAction::HostAction(HostAction::SetSetting {
            plugin: "bed".into(),
            key,
            value,
        }),
        AppOperation::Quit => AppAction::Command(WindowCommand::Quit),
    };
    let request = capture_app_request(world, action, PanelTarget::LargestArea);
    Ok(PendingApp::Request(submit_app_request(world, request)))
}

pub(super) fn start_workbench(world: &mut World, action: WorkbenchAction) -> PendingApp {
    let source = match &action {
        WorkbenchAction::Invoke(invocation) => invocation.source,
        WorkbenchAction::FocusPanel(panel)
        | WorkbenchAction::ClosePanel(panel)
        | WorkbenchAction::SplitPanel { panel, .. } => Some(*panel),
        _ => world.resource::<WorkbenchLayout>().focused,
    };
    let request = capture_app_request_from(
        world,
        AppAction::WorkbenchAction(action),
        PanelTarget::LargestArea,
        source,
    );
    PendingApp::Request(submit_app_request(world, request))
}

pub(super) fn poll(world: &mut World, pending: &mut PendingApp) -> Option<OperationResult> {
    match pending {
        PendingApp::Read(document) => Some(read(world, *document).map(OperationValue::Document)),
        PendingApp::Request(id) => poll_app_request_result(world, *id).map(|result| {
            result
                .map(|outcome| match outcome {
                    AppRequestOutcome::Applied => OperationValue::Applied,
                    AppRequestOutcome::Ignored => OperationValue::Ignored,
                    AppRequestOutcome::Opened { panel, .. } => {
                        OperationValue::Opened(handle(world, panel))
                    }
                    AppRequestOutcome::Closed { panels } => OperationValue::Closed(
                        panels
                            .into_iter()
                            .map(|panel| handle(world, panel))
                            .collect(),
                    ),
                    AppRequestOutcome::Saved { documents } => OperationValue::Saved(documents),
                })
                .map_err(|error| {
                    let kind = if error.kind == std::io::ErrorKind::WouldBlock {
                        ErrorKind::RevisionConflict
                    } else {
                        ErrorKind::Failed
                    };
                    AutomationError::new(kind, error.message)
                })
        }),
    }
}

fn read(world: &World, document: DocumentHandle) -> Result<DocumentRead, AutomationError> {
    let document_id = resolve_document(world, document)?;
    let session = world.non_send::<DocumentSession>();
    let snapshot = session
        .document_snapshot(document_id)
        .map_err(|error| AutomationError::new(ErrorKind::Failed, error.to_string()))?;
    let revision = session
        .document_revision(document_id)
        .map_err(|error| AutomationError::new(ErrorKind::Failed, error.to_string()))?;
    Ok(DocumentRead {
        info: DocumentInfo {
            handle: document,
            path: snapshot.path,
            kind: snapshot.kind,
            revision,
            dirty: snapshot.dirty,
            disk_conflict: snapshot.disk_conflict,
        },
        bytes: snapshot.bytes,
    })
}

pub(super) fn documents(world: &World) -> Result<Vec<DocumentInfo>, AutomationError> {
    let Some(session) = world.get_non_send::<DocumentSession>() else {
        return Ok(Vec::new());
    };
    session
        .document_ids()
        .into_iter()
        .map(|id| read(world, document_handle(world, id)).map(|document| document.info))
        .collect()
}

pub(super) fn workspace(world: &World) -> Option<Value> {
    world
        .get_resource::<bed_app::WorkspaceState>()?
        .spec
        .as_ref()
        .map(|workspace| workspace.to_value())
}
