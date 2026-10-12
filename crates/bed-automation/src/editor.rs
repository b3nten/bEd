//! Optional document operations use the editor's captured transition queue.
use super::*;
use bed_document_session::editor_session::{ByteEdit, DocumentId, EditorSession};
use bed_editor_api::{HostRequest, PanelTarget, Revision};
use bed_editor_workbench::{
    EditorAction, EditorRequestId, EditorRequestOutcome, EditorRequests, WindowCommand,
    capture_editor_request, capture_editor_request_from, poll_editor_request_result,
    submit_editor_request,
};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DocumentHandle {
    app: u64,
    session: u64,
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
    pub kind: bed_document_session::editor_session::DocumentKind,
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
pub enum EditorOperation {
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
    SetWorkspace(bed_editor_api::workspace::WorkspaceSpec),
    SetSetting {
        key: String,
        value: Value,
    },
    Quit,
}

pub(super) enum PendingEditor {
    Request(EditorRequestId),
    Read(DocumentHandle),
}
fn document_handle(world: &World, document: DocumentId) -> DocumentHandle {
    DocumentHandle {
        app: world.resource::<AutomationScope>().0,
        session: session(world),
        document,
    }
}
fn resolve_document(
    world: &World,
    document: DocumentHandle,
) -> Result<DocumentId, AutomationError> {
    let owner = world.get_non_send::<EditorSession>().ok_or_else(|| {
        AutomationError::new(ErrorKind::Unavailable, "document session is not installed")
    })?;
    if document.app != world.resource::<AutomationScope>().0
        || document.session != owner.workspace_id().0
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
    operation: EditorOperation,
) -> Result<PendingEditor, AutomationError> {
    if !world.contains_resource::<EditorRequests>() {
        return Err(AutomationError::new(
            ErrorKind::Unavailable,
            "editor transitions are not installed",
        ));
    }
    let action = match operation {
        EditorOperation::ReadDocument(document) => {
            resolve_document(world, document)?;
            return Ok(PendingEditor::Read(document));
        }
        EditorOperation::OpenFile {
            path,
            viewer,
            additional,
        } => EditorAction::OpenFileWithViewer {
            path,
            viewer,
            additional,
            move_existing: false,
        },
        EditorOperation::NewDocument => EditorAction::Command(WindowCommand::NewDocument),
        EditorOperation::ApplyEdits {
            document,
            revision,
            edits,
        } => EditorAction::ApplyEdits {
            document: resolve_document(world, document)?,
            revision,
            edits,
        },
        EditorOperation::Save(documents) => EditorAction::SaveDocuments(
            documents
                .into_iter()
                .map(|document| resolve_document(world, document))
                .collect::<Result<_, _>>()?,
        ),
        EditorOperation::SaveAs { document, path } => EditorAction::SaveAs {
            document: resolve_document(world, document)?,
            path,
        },
        EditorOperation::Undo(document) => EditorAction::HostRequest(HostRequest::Undo {
            document: resolve_document(world, document)?,
        }),
        EditorOperation::Redo(document) => EditorAction::HostRequest(HostRequest::Redo {
            document: resolve_document(world, document)?,
        }),
        EditorOperation::SwitchViewer { panel, viewer } => EditorAction::SwitchViewer {
            panel: resolve(world, panel)?,
            viewer,
        },
        EditorOperation::AttachWorkspace(path) => EditorAction::AttachWorkspace(path),
        EditorOperation::SetWorkspace(workspace) => EditorAction::SetWorkspace(workspace),
        EditorOperation::SetSetting { key, value } => {
            EditorAction::HostRequest(HostRequest::SetSetting {
                plugin: "bed".into(),
                key,
                value,
            })
        }
        EditorOperation::Quit => EditorAction::Command(WindowCommand::Quit),
    };
    let request = capture_editor_request(world, action, PanelTarget::LargestArea);
    Ok(PendingEditor::Request(submit_editor_request(
        world, request,
    )))
}
pub(super) fn start_workbench(world: &mut World, action: Action) -> PendingEditor {
    let source = match &action {
        Action::Invoke(invocation) => invocation.source,
        Action::FocusPanel(panel)
        | Action::ClosePanel(panel)
        | Action::SplitPanel { panel, .. } => Some(*panel),
        _ => world.resource::<WorkbenchLayout>().focused,
    };
    let request = capture_editor_request_from(
        world,
        EditorAction::WorkbenchAction(action),
        PanelTarget::LargestArea,
        source,
    );
    PendingEditor::Request(submit_editor_request(world, request))
}
pub(super) fn poll(world: &mut World, pending: &mut PendingEditor) -> Option<OperationResult> {
    match pending {
        PendingEditor::Read(document) => Some(read(world, *document).map(OperationValue::Document)),
        PendingEditor::Request(id) => poll_editor_request_result(world, *id).map(|result| {
            result
                .map(|outcome| match outcome {
                    EditorRequestOutcome::Applied => OperationValue::Applied,
                    EditorRequestOutcome::Ignored => OperationValue::Ignored,
                    EditorRequestOutcome::Opened { panel, .. } => {
                        OperationValue::Opened(handle(world, panel))
                    }
                    EditorRequestOutcome::Closed { panels } => OperationValue::Closed(
                        panels
                            .into_iter()
                            .map(|panel| handle(world, panel))
                            .collect(),
                    ),
                    EditorRequestOutcome::Saved { documents } => OperationValue::Saved(documents),
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
    let session = world.non_send::<EditorSession>();
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
    let Some(session) = world.get_non_send::<EditorSession>() else {
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
        .get_resource::<bed_editor_workbench::EditorWorkspace>()?
        .spec
        .as_ref()
        .map(|workspace| workspace.to_value())
}
