//! Immutable document observations published by the document owner.
use crate::documents::{DocumentId, PluginDocument};
use bed_documents::DocumentKind;
use std::{io, sync::Arc};
#[derive(Clone, Default, bevy::prelude::Resource)]
pub struct DocumentFrame {
    pub active_view: Option<bed_documents::ViewId>,
    pub documents: Vec<PluginDocument>,
    pub active_document: Option<DocumentId>,
    pub session_id: u64,
    pub diagnostics: serde_json::Value,
}
impl DocumentFrame {
    /// Refresh revision-bound snapshots from the canonical document session.
    pub fn refresh_documents(
        &mut self,
        documents: &bed_documents::session::DocumentSession,
    ) -> io::Result<()> {
        let ids = documents.document_ids();
        self.documents.retain(|doc| ids.contains(&doc.id));
        for id in ids {
            let revision = documents.document_revision(id)?;
            let existing = self.documents.iter().position(|doc| doc.id == id);
            if let Some(index) =
                existing.filter(|&index| self.documents[index].revision == revision)
            {
                documents.with_document(id, |state| {
                    let cached = &mut self.documents[index];
                    cached.path.clone_from(&state.path);
                    cached.language_id.clone_from(&state.language_id);
                    cached.dirty = state.dirty;
                })?;
            } else {
                let doc = documents.snapshot(id)?;
                let text = if doc.kind == DocumentKind::Text {
                    Some(documents.with_document(id, |state| state.snapshot())?)
                } else {
                    None
                };
                let document = PluginDocument {
                    id,
                    path: doc.path,
                    kind: doc.kind,
                    language_id: doc.language_id,
                    revision,
                    dirty: doc.dirty,
                    bytes: Arc::from(doc.bytes),
                    text,
                };
                if let Some(index) = existing {
                    self.documents[index] = document;
                } else {
                    self.documents.push(document);
                }
            }
        }
        Ok(())
    }
}
