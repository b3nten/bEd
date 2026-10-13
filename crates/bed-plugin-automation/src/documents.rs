//! Scoped document observations independent of application transition policy.
use super::*;
use bed_api::documents::Revision;
use bed_documents::session::{DocumentId, DocumentSession};

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

fn document_handle(world: &World, document: DocumentId) -> DocumentHandle {
    DocumentHandle {
        app: world.resource::<AutomationScope>().0,
        session_id: session_id(world),
        document,
    }
}

pub(super) fn resolve_document(
    world: &World,
    document: DocumentHandle,
) -> Result<DocumentId, AutomationError> {
    let owner = world.get_non_send::<DocumentSession>().ok_or_else(|| {
        AutomationError::new(ErrorKind::Unavailable, "document session is not installed")
    })?;
    let scope = world.get_resource::<AutomationScope>().ok_or_else(|| {
        AutomationError::new(ErrorKind::Unavailable, "automation is not installed")
    })?;
    if document.app != scope.0
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

pub fn read_document(
    world: &World,
    document: DocumentHandle,
) -> Result<DocumentRead, AutomationError> {
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
        .map(|id| read_document(world, document_handle(world, id)).map(|document| document.info))
        .collect()
}
