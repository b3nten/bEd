//! Concrete text-editor extension contracts. The panel host does not interpret these.
use crate::{EditorView, SourceDebugAction, SourceDebugPresentation};
use bed_document_session::editor_session::{DocumentId, EditorSession};
use dear_imgui_rs::Ui;
use std::{
    cell::RefCell,
    io,
    rc::{Rc, Weak},
};

/// Source debugging is one editor capability, independent of the panel host.
/// Providers own their state and can serve every view of a document.
pub trait SourceDebugExtension {
    /// Return `None` for documents this provider does not support. The first
    /// applicable provider owns this view's source actions and hover rendering.
    fn presentation(
        &self,
        session: &EditorSession,
        document: DocumentId,
    ) -> io::Result<Option<SourceDebugPresentation>>;
    fn action(
        &mut self,
        session: &EditorSession,
        document: DocumentId,
        action: SourceDebugAction,
    ) -> io::Result<()>;
    /// Called inside the originating editor child, after its document borrow is
    /// released. It is skipped when the child is hidden or its text changed
    /// after collecting the presentation for this frame.
    fn draw_hover(
        &mut self,
        _ui: &Ui,
        _session: &EditorSession,
        _view: &EditorView,
    ) -> io::Result<()> {
        Ok(())
    }
}

type Provider = Rc<RefCell<dyn SourceDebugExtension>>;
type ProviderRegistration = Weak<RefCell<dyn SourceDebugExtension>>;

/// A workspace owns a registry; registrations are weak so unloading a module
/// removes its contributions without keeping a debug session alive.
#[derive(Clone, Default)]
pub struct EditorExtensions {
    source_debug: Rc<RefCell<Vec<ProviderRegistration>>>,
}

impl std::fmt::Debug for EditorExtensions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EditorExtensions").finish_non_exhaustive()
    }
}

impl EditorExtensions {
    pub fn register_source_debug(&self, provider: &Provider) {
        let mut entries = self.source_debug.borrow_mut();
        entries.retain(|entry| entry.strong_count() > 0);
        let registration = Rc::downgrade(provider);
        if !entries.iter().any(|entry| entry.ptr_eq(&registration)) {
            entries.push(registration);
        }
    }

    pub(crate) fn source_debug(
        &self,
        session: &EditorSession,
        document: DocumentId,
    ) -> io::Result<Option<(Provider, SourceDebugPresentation)>> {
        // Clone providers before calling user code; registration never stays borrowed.
        let providers: Vec<_> = {
            let mut entries = self.source_debug.borrow_mut();
            entries.retain(|entry| entry.strong_count() > 0);
            entries.iter().filter_map(Weak::upgrade).collect()
        };
        for provider in providers {
            let presentation = provider.borrow().presentation(session, document)?;
            if let Some(presentation) = presentation {
                return Ok(Some((provider, presentation)));
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    struct Applicable {
        document: DocumentId,
        calls: Rc<Cell<usize>>,
    }

    impl SourceDebugExtension for Applicable {
        fn presentation(
            &self,
            _session: &EditorSession,
            document: DocumentId,
        ) -> io::Result<Option<SourceDebugPresentation>> {
            self.calls.set(self.calls.get() + 1);
            Ok((document == self.document).then(SourceDebugPresentation::default))
        }

        fn action(
            &mut self,
            _session: &EditorSession,
            _document: DocumentId,
            _action: SourceDebugAction,
        ) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn shared_registrations_are_weak_unique_and_document_specific() {
        let mut session = EditorSession::new();
        let document = session.create_document(b"debuggable").unwrap();
        let other = session.create_document(b"ordinary").unwrap();
        let calls = Rc::new(Cell::new(0));
        let provider: Provider = Rc::new(RefCell::new(Applicable {
            document,
            calls: calls.clone(),
        }));
        let extensions = EditorExtensions::default();
        let sibling_view_extensions = extensions.clone();
        extensions.register_source_debug(&provider);
        sibling_view_extensions.register_source_debug(&provider);
        assert_eq!(Rc::strong_count(&provider), 1);
        assert!(extensions.source_debug(&session, other).unwrap().is_none());
        assert_eq!(
            calls.get(),
            1,
            "duplicate registration must not query twice"
        );
        let (selected, _) = sibling_view_extensions
            .source_debug(&session, document)
            .unwrap()
            .unwrap();
        assert!(Rc::ptr_eq(&selected, &provider));
        drop(selected);
        drop(provider);
        assert!(
            extensions
                .source_debug(&session, document)
                .unwrap()
                .is_none()
        );
        assert!(extensions.source_debug.borrow().is_empty());
    }

    #[test]
    fn presentation_callbacks_can_register_another_provider() {
        struct RegisterDuringQuery {
            extensions: EditorExtensions,
            next: Provider,
        }
        impl SourceDebugExtension for RegisterDuringQuery {
            fn presentation(
                &self,
                _session: &EditorSession,
                _document: DocumentId,
            ) -> io::Result<Option<SourceDebugPresentation>> {
                self.extensions.register_source_debug(&self.next);
                Ok(None)
            }
            fn action(
                &mut self,
                _session: &EditorSession,
                _document: DocumentId,
                _action: SourceDebugAction,
            ) -> io::Result<()> {
                Ok(())
            }
        }
        let mut session = EditorSession::new();
        let document = session.create_document(b"source").unwrap();
        let extensions = EditorExtensions::default();
        let next: Provider = Rc::new(RefCell::new(Applicable {
            document,
            calls: Rc::new(Cell::new(0)),
        }));
        let provider: Provider = Rc::new(RefCell::new(RegisterDuringQuery {
            extensions: extensions.clone(),
            next: next.clone(),
        }));
        extensions.register_source_debug(&provider);
        assert!(
            extensions
                .source_debug(&session, document)
                .unwrap()
                .is_none()
        );
        let (selected, _) = extensions
            .source_debug(&session, document)
            .unwrap()
            .unwrap();
        assert!(Rc::ptr_eq(&selected, &next));
    }
}
