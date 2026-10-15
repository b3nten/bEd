//! Document snapshots, identities, edits and viewer capabilities.
pub use crate::document_frame::DocumentFrame;
use crate::features::{FeatureRegistry, HostAction};
use crate::{PanelTypeId, ViewerId};
use bedtime_editing::buffer::text_buffer::Snapshot;
pub use bedtime_editing::{
    document_state::DocumentKind,
    identity::{DocumentId, SessionId, ViewId},
};
#[cfg(feature = "language-services")]
pub use bedtime_lsp::workspace_lsp::LspRequestOrigin;
use dear_imgui_rs::Ui;
use std::{
    io,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

/// Frozen revision for replacing an opted-in viewer's pristine default input.
/// The document owner requires `ReusableDocumentInput` and validates this stamp
/// against the canonical document before replacing it.
#[derive(bevy::prelude::Component, Clone, Copy, Debug)]
pub struct PanelDefaultInput {
    pub document: bedtime_documents::DocumentId,
    pub revision: Revision,
}

/// A viewer opts into replacing its pristine default document when opening a file.
/// The document owner also requires a valid revision-bound `PanelDefaultInput`.
#[derive(bevy::prelude::Component)]
pub struct ReusableDocumentInput;

pub type Revision = (u64, u64);

/// Correlates a queued edit with its acknowledgement without exposing host panels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EditToken(pub u64);

impl EditToken {
    pub fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

/// Correlates a module's save barrier with completed disk writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SaveToken(pub u64);

impl SaveToken {
    pub fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedDocument {
    pub document: DocumentId,
    pub revision: Revision,
}

/// Completion of a feature's revision-bound edit or asynchronous save barrier.
#[derive(bevy::prelude::Message, Clone, Debug)]
pub enum DocumentReply {
    Edit {
        recipient: String,
        token: EditToken,
        result: Result<Revision, String>,
    },
    Save {
        recipient: String,
        token: SaveToken,
        result: Result<Vec<SavedDocument>, String>,
    },
}

/// Facts emitted by the sole document session owner for independent features.
#[derive(bevy::prelude::Message, Clone, Debug)]
pub struct DocumentEvent(pub bedtime_documents::session::SessionEvent);

#[derive(Clone, Debug)]
pub struct PluginDocument {
    pub id: DocumentId,
    pub path: String,
    pub kind: DocumentKind,
    pub language_id: String,
    pub revision: Revision,
    pub dirty: bool,
    pub bytes: Arc<[u8]>,
    /// Cheap immutable rope snapshot for source-processing workers.
    pub text: Option<Snapshot>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectionContext {
    pub document: DocumentId,
    pub revision: Revision,
    pub ranges: Vec<std::ops::Range<usize>>,
    pub text: String,
}

/// Captured at the originating UI surface, never reinterpreted using later focus.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommandContext {
    pub path: Option<String>,
    pub document: Option<DocumentId>,
    pub revision: Option<Revision>,
    pub selection: Option<SelectionContext>,
}

/// The resource supplied to one panel factory. Local files are opened by the
/// feature itself and never installed as editable session documents.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum PanelInput {
    #[default]
    None,
    Document(DocumentId),
    LocalFile(PathBuf),
}

impl PanelInput {
    pub fn document(&self) -> Option<DocumentId> {
        match self {
            Self::Document(id) => Some(*id),
            _ => None,
        }
    }

    pub fn local_file(&self) -> Option<&Path> {
        match self {
            Self::LocalFile(path) => Some(path),
            _ => None,
        }
    }
}

impl From<Option<DocumentId>> for PanelInput {
    fn from(document: Option<DocumentId>) -> Self {
        document.map(Self::Document).unwrap_or(Self::None)
    }
}

/// Document associations are independent of the workbench's presentation and layout.
#[derive(Clone, Debug, Default, PartialEq, bevy::prelude::Component)]
pub struct PanelBinding {
    pub input: PanelInput,
    pub document: Option<DocumentId>,
    pub view: Option<bedtime_documents::session::ViewId>,
    pub terminal: Option<u64>,
    pub viewer: Option<ViewerId>,
    pub session_id: u64,
}

#[derive(Clone, Debug)]
pub struct PanelEditResult {
    pub entity: bevy::prelude::Entity,
    pub token: EditToken,
    pub result: Result<Revision, String>,
}

pub type PanelEditSystem = bevy::ecs::system::SystemId<bevy::prelude::In<PanelEditResult>>;

#[derive(Clone, Copy, bevy::prelude::Component)]
pub struct PanelEdits(pub PanelEditSystem);

#[derive(Clone, Debug)]
pub struct ViewerChoice {
    pub id: ViewerId,
    pub label: String,
    pub text_editor: bool,
}

#[derive(Clone, Debug)]
pub struct ViewerMenu {
    pub tab: u64,
    pub document: DocumentId,
    pub current: ViewerId,
    pub choices: Vec<ViewerChoice>,
}

pub fn panel_viewer_menu(
    world: &bevy::prelude::World,
    entity: bevy::prelude::Entity,
    fallback: &ViewerId,
) -> Option<ViewerMenu> {
    let binding = world.get::<PanelBinding>(entity)?;
    let document = binding.input.document().or(binding.document)?;
    let frame = world.get_resource::<DocumentFrame>()?;
    let snapshot = frame
        .documents
        .iter()
        .find(|snapshot| snapshot.id == document)?;
    let registry = world.get_resource::<FeatureRegistry>()?;
    let owner = crate::workbench::panel_owner(world, entity)?;
    let panel = world.get::<crate::workbench::PanelPresentation>(owner)?;
    let current = binding.viewer.as_ref().unwrap_or(fallback);
    let choices = registry
        .compatible_viewers(&snapshot.path, Some(snapshot.kind))
        .into_iter()
        .filter(|viewer| viewer.supports_document_kind(snapshot.kind))
        .map(|viewer| ViewerChoice {
            id: viewer.id.clone(),
            label: viewer.label.into(),
            text_editor: viewer.fallback && viewer.document_kind() == Some(DocumentKind::Text),
        })
        .collect::<Vec<_>>();
    choices
        .iter()
        .any(|choice| choice.id != *current)
        .then(|| ViewerMenu {
            tab: panel.id,
            document,
            current: current.into(),
            choices,
        })
}

impl ViewerMenu {
    /// Append requests to open another viewer to an existing panel context menu.
    pub fn draw(&self, ui: &Ui, requests: &mut Vec<HostAction>) {
        let alternatives: Vec<_> = self
            .choices
            .iter()
            .filter(|v| v.id != self.current)
            .collect();
        let mut selected = None;
        for choice in alternatives.iter().filter(|v| v.text_editor) {
            if ui.menu_item("Open in Text Editor") {
                selected = Some(choice.id.clone());
            }
        }
        let formatted: Vec<_> = alternatives.iter().filter(|v| !v.text_editor).collect();
        if formatted.len() == 1 {
            let choice = formatted[0];
            if ui.menu_item(format!("Open in {}", choice.label)) {
                selected = Some(choice.id.clone());
            }
        } else if !formatted.is_empty()
            && let Some(_menu) = ui.begin_menu("Open With")
        {
            for choice in formatted {
                if ui.menu_item(&choice.label) {
                    selected = Some(choice.id.clone());
                }
            }
        }
        if let Some(viewer) = selected {
            requests.push(HostAction::OpenDocumentViewer {
                tab: self.tab,
                document: self.document,
                viewer,
            });
        }
    }
}

/// Capture source selection as immutable byte ranges and revision-bound text.
/// Navigation and menu commands use the same document facts as the editor widget.
pub fn selection_context(
    session: &bedtime_documents::DocumentSession,
    view: bedtime_documents::ViewId,
) -> io::Result<CommandContext> {
    let document = session
        .document_for_view(view)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Document view is detached"))?;
    let revision = session.document_revision(document)?;
    let selections = session.view_snapshot(view)?.selections;
    let (path, ranges, text) = session.with_document(document, |state| {
        let mut ranges = Vec::new();
        let mut selected = Vec::new();
        for selection in selections.iter().filter(|selection| !selection.empty()) {
            let (ar, ac, br, bc) = selection.ordered();
            let range = state.offset_from_row_col(ar, ac)..state.offset_from_row_col(br, bc);
            let mut bytes = vec![0; range.len()];
            state.copy_bytes(range.start, range.len(), &mut bytes);
            selected.push(String::from_utf8_lossy(&bytes).into_owned());
            ranges.push(range);
        }
        (state.path.clone(), ranges, selected.join("\n"))
    })?;
    Ok(CommandContext {
        path: Some(path),
        document: Some(document),
        revision: Some(revision),
        selection: Some(SelectionContext {
            document,
            revision,
            ranges,
            text,
        }),
    })
}

/// Storage a viewer consumes. File-backed viewers have no document kind.
#[derive(Clone, Debug)]
pub enum ViewerBacking {
    Document {
        kind: DocumentKind,
        supported_kinds: &'static [DocumentKind],
    },
    LocalFile,
}

#[derive(Clone, Debug)]
pub struct FileViewer {
    /// The default for this document kind when no file extension matches. At
    /// most one module can provide a fallback for each kind.
    pub fallback: bool,
    pub id: ViewerId,
    pub plugin: &'static str,
    pub label: &'static str,
    pub panel_type: PanelTypeId,
    /// Lowercase extensions without the dot. Matching ignores ASCII case.
    pub extensions: &'static [&'static str],
    pub backing: ViewerBacking,
}

impl FileViewer {
    pub fn document_kind(&self) -> Option<DocumentKind> {
        match self.backing {
            ViewerBacking::Document { kind, .. } => Some(kind),
            ViewerBacking::LocalFile => None,
        }
    }

    pub fn is_local_file(&self) -> bool {
        matches!(self.backing, ViewerBacking::LocalFile)
    }

    pub fn single_document_kind(&self) -> Option<DocumentKind> {
        match self.backing {
            ViewerBacking::Document {
                kind,
                supported_kinds,
            } if supported_kinds.len() == 1 => Some(kind),
            _ => None,
        }
    }

    /// Match only the filename's extension, ignoring ASCII case.
    pub fn matches_path(&self, path: &str) -> bool {
        let filename = path.rsplit(['/', '\\']).next().unwrap_or_default();
        let Some((_, extension)) = filename.rsplit_once('.') else {
            return false;
        };
        self.extensions
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(extension))
    }

    pub(crate) fn supports_content_kind(&self, content_kind: Option<DocumentKind>) -> bool {
        self.is_local_file()
            || self.document_kind() == Some(DocumentKind::Bytes)
            || content_kind.is_some_and(|kind| self.supports_document_kind(kind))
    }

    pub fn supports_document_kind(&self, kind: DocumentKind) -> bool {
        match &self.backing {
            ViewerBacking::Document {
                supported_kinds, ..
            } => supported_kinds.contains(&kind),
            ViewerBacking::LocalFile => false,
        }
    }
}
