//! Translated from ned editor/editor_events.{h,cpp}; see LICENSE and NOTICE.
//! Notifications are synchronous and stay on the editor's main thread.

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DocumentChange {
    pub start_line: i32,
    pub start_character: i32,
    pub end_line: i32,
    pub end_character: i32,
    pub text: Vec<u8>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DidEdit {
    pub version: i32,
    pub first_row: i32,
    pub last_row: i32,
    pub changes: Vec<DocumentChange>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DidSave {
    pub path: String,
    pub version: i32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Overlay {
    #[default]
    None,
    Settings,
    LineJump,
    FileFinder,
    Find,
    ContentSearch,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DidRequestExclusiveOverlay {
    pub keep: Overlay,
}

type Listener<T> = Box<dyn FnMut(&T)>;
type DocumentListener = Box<dyn FnMut(&DidEdit, &super::editor_state::EditorState)>;
type SavedDocumentListener = Box<dyn FnMut(&DidSave, &super::editor_state::EditorState)>;
type WeakDocumentListener = Box<dyn FnMut(&DidEdit, &super::editor_state::EditorState) -> bool>;
type WeakOverlayListener = Box<dyn FnMut(Option<&DidRequestExclusiveOverlay>) -> bool>;

#[derive(Default)]
pub struct EditorEvents {
    did_edit_listeners: Vec<Listener<DidEdit>>,
    document_edit_listeners: Vec<DocumentListener>,
    did_save_listeners: Vec<Listener<DidSave>>,
    saved_document_listeners: Vec<SavedDocumentListener>,
    exclusive_overlay_listeners: Vec<Listener<DidRequestExclusiveOverlay>>,
    weak_document_listeners: Vec<WeakDocumentListener>,
    weak_overlay_listeners: Vec<(Option<u64>, WeakOverlayListener)>,
    view_scope: Option<u64>,
}

impl EditorEvents {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn subscribe_did_edit(&mut self, listener: impl FnMut(&DidEdit) + 'static) {
        self.did_edit_listeners.push(Box::new(listener));
    }

    /// Rust listeners borrow the post-action state instead of retaining the
    /// C++ services' pointers into their owning editor.
    pub fn subscribe_did_edit_document(
        &mut self,
        listener: impl FnMut(&DidEdit, &super::editor_state::EditorState) + 'static,
    ) {
        self.document_edit_listeners.push(Box::new(listener));
    }

    /// View subscriptions return false after their weak owner expires, so a
    /// shared document does not retain closed frames or accumulate dead hooks.
    pub fn subscribe_did_edit_document_weak(
        &mut self,
        listener: impl FnMut(&DidEdit, &super::editor_state::EditorState) -> bool + 'static,
    ) {
        self.weak_document_listeners.push(Box::new(listener));
    }
    pub fn subscribe_overlay_weak(
        &mut self,
        listener: impl FnMut(Option<&DidRequestExclusiveOverlay>) -> bool + 'static,
    ) {
        self.weak_overlay_listeners
            .push((self.view_scope, Box::new(listener)));
    }
    pub fn set_view_scope(&mut self, view: Option<u64>) {
        self.view_scope = view;
    }

    pub fn subscribe_did_save(&mut self, listener: impl FnMut(&DidSave) + 'static) {
        self.did_save_listeners.push(Box::new(listener));
    }

    pub fn subscribe_did_save_document(
        &mut self,
        listener: impl FnMut(&DidSave, &super::editor_state::EditorState) + 'static,
    ) {
        self.saved_document_listeners.push(Box::new(listener));
    }

    pub fn subscribe_did_request_exclusive_overlay(
        &mut self,
        listener: impl FnMut(&DidRequestExclusiveOverlay) + 'static,
    ) {
        self.exclusive_overlay_listeners.push(Box::new(listener));
    }

    pub fn emit_did_edit(&mut self, event: &DidEdit) {
        for listener in &mut self.did_edit_listeners {
            listener(event);
        }
    }

    pub fn emit_did_edit_document(
        &mut self,
        event: &DidEdit,
        state: &super::editor_state::EditorState,
    ) {
        self.emit_did_edit(event);
        for listener in &mut self.document_edit_listeners {
            listener(event, state);
        }
        self.weak_document_listeners
            .retain_mut(|listener| listener(event, state));
    }

    pub fn emit_did_save(&mut self, event: &DidSave) {
        for listener in &mut self.did_save_listeners {
            listener(event);
        }
    }

    pub fn emit_did_save_document(
        &mut self,
        event: &DidSave,
        state: &super::editor_state::EditorState,
    ) {
        self.emit_did_save(event);
        for listener in &mut self.saved_document_listeners {
            listener(event, state);
        }
    }

    pub fn emit_did_request_exclusive_overlay(&mut self, event: &DidRequestExclusiveOverlay) {
        for listener in &mut self.exclusive_overlay_listeners {
            listener(event);
        }
        let scope = self.view_scope;
        self.weak_overlay_listeners.retain_mut(|(owner, listener)| {
            listener((scope.is_none() || owner.is_none() || *owner == scope).then_some(event))
        });
    }

    pub fn clear(&mut self) {
        self.did_edit_listeners.clear();
        self.document_edit_listeners.clear();
        self.did_save_listeners.clear();
        self.saved_document_listeners.clear();
        self.exclusive_overlay_listeners.clear();
        self.weak_document_listeners.clear();
        self.weak_overlay_listeners.clear();
    }
}
