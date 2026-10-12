//! Immutable source facts supplied by a host; widgets return actions to their owner.
use crate::{SourceDebugPresentation, SourceGitPresentation};
use bed_document_session::editor_session::{DocumentId, ViewId};
use std::collections::HashMap;

pub type SourceRevision = (u64, u64);

/// A completed debugger evaluation. The debugger owns evaluation and its lifetime.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceHoverPresentation {
    pub row: i32,
    pub column: i32,
    pub expression: String,
    pub value: String,
    pub anchor: [f32; 2],
}

/// Owned observations from the originating editor child. No Ui borrow escapes.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceHoverObservation {
    pub info: crate::views::hover_trigger::Info,
    pub dismissed: bool,
    pub key_pressed: bool,
    pub in_pane: bool,
    pub over_popup: bool,
    pub mouse: [f32; 2],
    pub frame: usize,
    pub window_size: [f32; 2],
}

#[derive(Clone, Debug)]
struct SourceFact<T> {
    revision: SourceRevision,
    presentation: T,
}
#[derive(Clone, Debug)]
struct HoverFact {
    document: DocumentId,
    revision: SourceRevision,
    presentation: SourceHoverPresentation,
}

/// A frame's source facts. Cloning makes a snapshot, never another controller owner.
#[derive(Clone, Debug, Default)]
pub struct EditorExtensions {
    source_debug: HashMap<DocumentId, SourceFact<SourceDebugPresentation>>,
    source_git: HashMap<DocumentId, SourceFact<SourceGitPresentation>>,
    source_hover: HashMap<ViewId, HoverFact>,
}
impl EditorExtensions {
    pub fn set_source_debug(
        &mut self,
        document: DocumentId,
        revision: SourceRevision,
        presentation: SourceDebugPresentation,
    ) {
        self.source_debug.insert(
            document,
            SourceFact {
                revision,
                presentation,
            },
        );
    }
    pub fn set_source_git(
        &mut self,
        document: DocumentId,
        revision: SourceRevision,
        presentation: SourceGitPresentation,
    ) {
        self.source_git.insert(
            document,
            SourceFact {
                revision,
                presentation,
            },
        );
    }
    pub fn set_source_hover(
        &mut self,
        view: ViewId,
        document: DocumentId,
        revision: SourceRevision,
        presentation: SourceHoverPresentation,
    ) {
        self.source_hover.insert(
            view,
            HoverFact {
                document,
                revision,
                presentation,
            },
        );
    }
    pub fn source_debug(
        &self,
        document: DocumentId,
        revision: SourceRevision,
    ) -> Option<&SourceDebugPresentation> {
        self.source_debug
            .get(&document)
            .filter(|fact| fact.revision == revision)
            .map(|fact| &fact.presentation)
    }
    /// Debugging remains available across edits while its row decorations await
    /// a fresh snapshot. A document replacement or explicit removal ends it.
    pub fn source_debug_available(&self, document: DocumentId, revision: SourceRevision) -> bool {
        self.source_debug
            .get(&document)
            .is_some_and(|fact| fact.revision.0 == revision.0)
    }
    pub fn source_git(
        &self,
        document: DocumentId,
        revision: SourceRevision,
    ) -> Option<&SourceGitPresentation> {
        self.source_git
            .get(&document)
            .filter(|fact| fact.revision == revision)
            .map(|fact| &fact.presentation)
    }
    pub fn source_hover(
        &self,
        view: ViewId,
        document: DocumentId,
        revision: SourceRevision,
    ) -> Option<&SourceHoverPresentation> {
        self.source_hover
            .get(&view)
            .filter(|fact| fact.document == document && fact.revision == revision)
            .map(|fact| &fact.presentation)
    }
    pub fn clear_source_debug(&mut self, document: DocumentId) {
        self.source_debug.remove(&document);
        self.source_hover
            .retain(|_, fact| fact.document != document);
    }
    pub fn clear_source_git(&mut self, document: DocumentId) {
        self.source_git.remove(&document);
    }
    pub fn clear_source_hover(&mut self, view: ViewId) {
        self.source_hover.remove(&view);
    }
    pub fn clear_debug(&mut self) {
        self.source_debug.clear();
        self.source_hover.clear();
    }
    pub fn clear_git(&mut self) {
        self.source_git.clear();
    }
    pub fn clear(&mut self) {
        self.clear_debug();
        self.clear_git();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bed_document_session::editor_session::EditorSession;
    #[test]
    fn snapshots_are_document_specific_revision_bound_and_independent() {
        let mut session = EditorSession::new();
        let document = session.create_document(b"source").unwrap();
        let other = session.create_document(b"other").unwrap();
        let revision = session.document_revision(document).unwrap();
        let mut facts = EditorExtensions::default();
        facts.set_source_debug(document, revision, SourceDebugPresentation::default());
        let snapshot = facts.clone();
        assert!(facts.source_debug(other, revision).is_none());
        assert!(!facts.source_debug_available(other, revision));
        assert!(facts.source_debug_available(document, (revision.0, revision.1 + 1)));
        assert!(!facts.source_debug_available(document, (revision.0 + 1, revision.1)));
        assert!(
            facts
                .source_debug(document, (revision.0, revision.1 + 1))
                .is_none()
        );
        facts.clear_source_debug(document);
        assert!(facts.source_debug(document, revision).is_none());
        assert!(!facts.source_debug_available(document, revision));
        assert!(snapshot.source_debug(document, revision).is_some());
    }
}
