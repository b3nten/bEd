//! Requests captured from a source widget; owning features apply them after drawing.
use crate::{
    ViewResponse,
    extensions::{SourceHoverObservation, SourceRevision},
};
use bed_document_session::{DocumentId, ViewId};
use serde_json::{Value, json};

pub struct SourceHoverRequest {
    pub document: DocumentId,
    pub view: ViewId,
    pub revision: SourceRevision,
    pub observation: SourceHoverObservation,
}
#[derive(Default)]
pub struct SourceRequests {
    pub hover: Vec<SourceHoverRequest>,
}
impl SourceRequests {
    pub fn observe(&mut self, response: &ViewResponse) {
        if let Some(observation) = response.source_hover.clone() {
            self.hover.push(SourceHoverRequest {
                document: response.document,
                view: response.view,
                revision: response.source_revision,
                observation,
            });
        }
    }
}
/// Commands retain the originating document and revision even if focus changes.
pub fn source_commands(response: &ViewResponse) -> Vec<(&'static str, Value)> {
    response.source_actions.iter().map(|action| ("bed.debug.source-action", json!({"document":response.document.0,"revision":response.source_revision,"action":action})))
        .chain(response.source_git_actions.iter().map(|action| ("bed.git.source-action", json!({"document":response.document.0,"revision":response.source_revision,"action":action}))))
        .collect()
}
