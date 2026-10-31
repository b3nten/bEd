//! The editor as a workbench panel: a viewer of text documents. Its text and
//! history are the panel's document (`DocumentReference`); the panel keeps
//! only the view (selection, scroll, composition).
use super::*;
use bedtime_document::{TextOpenState, TextPosition};
use bedtime_ui::UiContext;
#[cfg(feature = "persistence")]
use bedtime_workbench::persistence::ReflectPersist;
use bedtime_workbench::{
    DocumentDraft, DocumentError, DocumentHandle, DocumentId, DocumentReference, DocumentStatus,
    DocumentUndoGroup, EditOptions, Fit, Notification, Opener, OpenersAppExt, Panel, PanelInstance,
    PanelPresentation, Viewer, WorkbenchWorldExt,
};
use bevy::prelude::*;

/// Registers the editor as the default viewer for text files, analyses the
/// documents editors show, and keeps Structure following the focused editor.
pub struct CodePlugin;

impl Plugin for CodePlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<EditorPanel>()
            .register_opener(
                Opener::<TextBuffer>::new::<EditorPanel>("Text Editor").files(text_fit),
            )
            .register_default_opener::<EditorPanel>()
            .register_type::<crate::structure::StructurePanel>()
            .add_plugins(crate::lsp::LanguageServersPlugin)
            .init_resource::<crate::structure::StructureSource>()
            .add_systems(
                Update,
                (
                    crate::syntax::update_syntax,
                    crate::structure::update_source,
                )
                    .chain(),
            );
    }
}

/// Text: no NUL bytes, and UTF-8 (the start may end mid-character).
pub fn text_fit(_: &std::path::Path, head: &[u8]) -> Fit {
    let utf8 = match std::str::from_utf8(head) {
        Ok(_) => true,
        Err(error) => error.error_len().is_none(),
    };
    if utf8 && !head.contains(&0) {
        Fit::Preferred
    } else {
        Fit::No
    }
}

#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
#[cfg_attr(feature = "persistence", reflect(Persist))]
#[require(PanelInstance = PanelInstance::of::<Self>())]
pub struct EditorPanel {
    #[reflect(ignore)]
    pub view: EditorView,
    #[reflect(ignore)]
    group: Option<DocumentUndoGroup>,
    /// The document `view`'s state belongs to; another one resets it.
    #[reflect(ignore)]
    state_for: Option<DocumentId>,
    /// An open's state, applied once the text is loaded.
    #[reflect(ignore)]
    pending: Option<TextOpenState>,
    /// A popup of the panel's own (code actions) has keyboard focus.
    #[reflect(ignore)]
    pub(crate) popup: bool,
    /// The panel had the layout's focus when last drawn, and on which frame:
    /// a hidden tab isn't drawn.
    #[reflect(ignore)]
    focused: bool,
    #[reflect(ignore)]
    drawn: u64,
}

impl Viewer<TextBuffer> for EditorPanel {
    fn new(_: DocumentHandle<TextBuffer>) -> Self {
        Self::default()
    }

    fn apply(world: &mut World, panel: Entity, state: &TextOpenState) {
        if let Some(mut editor) = world.get_mut::<EditorPanel>(panel) {
            editor.pending = Some(state.clone());
        }
    }
}

impl Panel for EditorPanel {
    const LABEL: &'static str = "Editor";

    fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, panel: Entity) {
        if let Err(error) = draw(ui, world, context, panel) {
            world
                .workbench_mut()
                .notify(Notification::warning(error.to_string()));
        }
    }
}

/// The egui id of the editor in `panel` showing `document`.
pub fn editor_id(panel: Entity, document: DocumentId) -> egui::Id {
    egui::Id::new(("bedtime.editor", panel, document))
}

fn draw(
    ui: &mut egui::Ui,
    world: &mut World,
    context: &UiContext<'_>,
    panel: Entity,
) -> Result<(), EditorError> {
    let Some(document) = world.get::<DocumentReference>(panel).map(|r| r.document()) else {
        return Ok(());
    };
    let info = world
        .workbench()
        .inspect_document(document)
        .map_err(error)?;
    if let Some(mut instance) = world.get_mut::<PanelInstance>(panel)
        && instance.title != info.title
    {
        instance.title = info.title.clone();
    }
    match info.status {
        DocumentStatus::Loading => {
            ui.centered_and_justified(|ui| ui.spinner());
            return Ok(());
        }
        DocumentStatus::Failed(error) => {
            egui::Frame::NONE.inner_margin(14).show(ui, |ui| {
                ui.label(format!("This file can't be opened: {error}"));
            });
            return Ok(());
        }
        DocumentStatus::Ready => {}
    }
    let handle = world
        .workbench()
        .cast_document::<TextBuffer>(document)
        .map_err(error)?;
    let id = editor_id(panel, document);
    let Some(mut editor) = world.entity_mut(panel).take::<EditorPanel>() else {
        return Ok(());
    };
    if editor.state_for != Some(document) && world.get::<DocumentDraft>(panel).is_none() {
        editor = EditorPanel {
            state_for: Some(document),
            pending: editor.pending.take(),
            ..default()
        };
    }
    editor.view.settings.read_only = !info.writable;
    editor.view.has_syntax = crate::syntax::language_name(&info.title).is_some();
    match world.get::<crate::syntax::DocumentSyntax>(document.entity()) {
        Some(syntax) => editor.view.set_analysis(&syntax.session),
        None => editor.view.set_syntax(None),
    }
    editor.view.syntax_palette = syntax::SyntaxPalette::from_theme(context.theme);
    #[cfg(unix)]
    {
        editor.view.language_tools = crate::lsp::server_of(world, panel).is_some();
    }
    #[cfg(unix)]
    editor.view.set_diagnostics(
        world
            .get::<crate::lsp::DocumentServer>(document.entity())
            .and_then(|server| server.diagnostics.clone()),
    );
    // Keyboard focus follows the panel's: the text takes it when the panel
    // becomes focused, and gives it up when another panel is.
    let focused = world
        .get::<PanelPresentation>(panel)
        .is_some_and(|presentation| presentation.focused);
    let frame = ui.ctx().cumulative_frame_nr();
    let shown_last_frame = editor.drawn + 1 >= frame;
    // A clicked tab button takes egui's focus: take it back unless a text
    // field (this editor's Find bar, say) has it.
    let typing_elsewhere = ui.ctx().text_edit_focused();
    if focused && !editor.popup && (!(editor.focused && shown_last_frame) || !typing_elsewhere) {
        ui.memory_mut(|memory| memory.request_focus(id));
    } else if !focused && ui.memory(|memory| memory.has_focus(id)) {
        ui.memory_mut(|memory| memory.surrender_focus(id));
    }
    editor.focused = focused;
    editor.drawn = frame;
    let mut session = DocumentSession {
        world,
        handle,
        panel,
        group: &mut editor.group,
    };
    let mut result = Ok(());
    if let Some(state) = editor.pending.take() {
        result = session.snapshot().and_then(|snapshot| {
            editor
                .view
                .request_navigation(&snapshot, navigation(&snapshot, &state))
        });
    }
    let shown = editor.view.show(ui, id, &mut session, context.motion);
    world.entity_mut(panel).insert(editor);
    // Hover, completions and Cmd/Ctrl-click, from the language server.
    #[cfg(unix)]
    if let Ok(output) = &shown
        && crate::lsp::server_of(world, panel).is_some()
    {
        crate::lsp::assist::draw(ui, world, panel, output);
        crate::lsp::actions::draw(ui, world, panel, output);
    }
    result.and(shown.map(|_| ()))
}

/// Where an open's state puts the cursor, revealed in the middle. Focus is
/// the panel's to give.
fn navigation(snapshot: &TextSnapshot, state: &TextOpenState) -> Navigation {
    let text = &snapshot.text;
    let byte = |position: &TextPosition| match *position {
        // The column counts characters (a tab is one), up to the line's end.
        TextPosition::LineColumn { line, column } => {
            let line = (line as usize).min(text.line_count().saturating_sub(1));
            let range = text.line_range(line);
            let start = range.start;
            let chars = text.range_text(range);
            let chars = chars.trim_end_matches(['\n', '\r']);
            start
                + chars
                    .char_indices()
                    .nth(column as usize)
                    .map_or(chars.len(), |(offset, _)| offset)
        }
        TextPosition::Byte(byte) => text.clamp_boundary(byte.min(text.len())),
    };
    let selection = match (&state.selection, &state.cursor) {
        (Some(range), _) => {
            let start = text.clamp_boundary(range.start.min(text.len()));
            let end = text.clamp_boundary(range.end.min(text.len()));
            TextSelection::new(text, start, end).ok()
        }
        (None, Some(cursor)) => TextSelection::caret(text, byte(cursor)).ok(),
        (None, None) => None,
    };
    let range = selection.map_or(0..0, TextSelection::range);
    Navigation {
        revision: snapshot.revision,
        range,
        selection,
        reveal: Reveal::Center,
        focus: false,
    }
}

fn error(error: DocumentError) -> EditorError {
    EditorError(error.to_string())
}

/// A view's text session over the panel's workbench document.
struct DocumentSession<'a> {
    world: &'a mut World,
    handle: DocumentHandle<TextBuffer>,
    panel: Entity,
    group: &'a mut Option<DocumentUndoGroup>,
}

impl DocumentSession<'_> {
    fn edit_in_group(
        &mut self,
        revision: u64,
        change: BufferEdit,
        group: DocumentUndoGroup,
    ) -> Result<u64, DocumentError> {
        self.world.workbench_mut().edit_document(
            self.handle,
            revision,
            change,
            EditOptions::grouped("Typing", group),
        )
    }
}

impl TextSession for DocumentSession<'_> {
    fn snapshot(&self) -> Result<TextSnapshot, EditorError> {
        let workbench = self.world.workbench();
        Ok(TextSnapshot {
            text: workbench.read_document(self.handle).map_err(error)?.clone(),
            revision: workbench
                .inspect_document(self.handle.id())
                .map_err(error)?
                .revision,
        })
    }

    fn changes_since(&self, revision: u64) -> Result<Option<Vec<TextCommit>>, EditorError> {
        Ok(self
            .world
            .workbench()
            .document_changes_since(self.handle, revision)
            .map_err(error)?
            .map(|commits| {
                commits
                    .into_iter()
                    .map(|commit| TextCommit {
                        revision: commit.revision,
                        changes: commit
                            .changes
                            .map(|changes| changes.iter().map(TextDelta::from).collect()),
                    })
                    .collect()
            }))
    }

    fn edit_buffer(&mut self, revision: u64, change: BufferEdit) -> Result<u64, EditorError> {
        let document = self.handle.id();
        if self.world.workbench().document_has_drafts(document) {
            return Err(EditorError(
                "Another view has uncommitted text composition".into(),
            ));
        }
        let group = match *self.group {
            Some(group) => group,
            None => self
                .world
                .workbench_mut()
                .begin_document_undo_group(document, self.panel)
                .map_err(error)?,
        };
        *self.group = Some(group);
        match self.edit_in_group(revision, change.clone(), group) {
            // The group ended elsewhere (a save, another view): start another.
            Err(DocumentError::StaleUndoGroup) => {
                let group = self
                    .world
                    .workbench_mut()
                    .begin_document_undo_group(document, self.panel)
                    .map_err(error)?;
                *self.group = Some(group);
                self.edit_in_group(revision, change, group).map_err(error)
            }
            result => result.map_err(error),
        }
    }

    fn history(&mut self, action: HistoryAction) -> Result<u64, EditorError> {
        let document = self.handle.id();
        let info = self
            .world
            .workbench()
            .inspect_document(document)
            .map_err(error)?;
        let workbench = self.world.workbench_mut();
        match action {
            HistoryAction::Undo if info.undo_label.is_some() => {
                workbench.undo_document(document).map_err(error)
            }
            HistoryAction::Redo if info.redo_label.is_some() => {
                workbench.redo_document(document).map_err(error)
            }
            _ => Ok(info.revision),
        }
    }

    fn end_undo_group(&mut self) {
        if let Some(group) = self.group.take() {
            let _ = self
                .world
                .workbench_mut()
                .end_document_undo_group(self.handle.id(), group);
        }
    }

    fn set_composing(&mut self, _: egui::Id, composing: bool) {
        if composing {
            self.world.entity_mut(self.panel).insert(DocumentDraft);
        } else {
            self.world.entity_mut(self.panel).remove::<DocumentDraft>();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bedtime_document::MemorySource;
    use bedtime_workbench::{LayoutsWorldExt, OpenOptions, WorkbenchCorePlugins};

    /// A workbench with the editor, and `text` open in two editor panels.
    fn host(text: &str) -> (App, DocumentHandle<TextBuffer>, Entity, Entity) {
        let mut app = App::new();
        app.add_plugins((WorkbenchCorePlugins, CodePlugin));
        let world = app.world_mut();
        let layout = world.layouts_mut().create("Test").unwrap();
        world.layouts_mut().activate(layout).unwrap();
        let source = MemorySource::new("notes", TextBuffer::from(text));
        let first = world
            .workbench_mut()
            .open_document(source.clone(), OpenOptions::default())
            .unwrap();
        let second = world
            .workbench_mut()
            .open_document(
                source,
                OpenOptions {
                    target: bedtime_workbench::Target::New { area: None },
                    ..default()
                },
            )
            .unwrap();
        let document = world.get::<DocumentReference>(first).unwrap().document();
        let handle = world.workbench().cast_document(document).unwrap();
        // Loading happens on a worker.
        for _ in 0..200 {
            app.update();
            let info = app.world().workbench().inspect_document(document).unwrap();
            if info.status == DocumentStatus::Ready {
                return (app, handle, first, second);
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        panic!("the document never loaded");
    }

    fn session<'a>(
        world: &'a mut World,
        handle: DocumentHandle<TextBuffer>,
        panel: Entity,
        group: &'a mut Option<DocumentUndoGroup>,
    ) -> DocumentSession<'a> {
        DocumentSession {
            world,
            handle,
            panel,
            group,
        }
    }

    #[test]
    fn views_share_the_documents_text_and_history() {
        let (mut app, handle, first, second) = host("original");
        let (mut one, mut two) = (None, None);
        let mut editing = session(app.world_mut(), handle, first, &mut one);
        editing
            .edit(
                0,
                TextEdit {
                    range: 0..0,
                    replacement: "é".into(),
                },
            )
            .unwrap();
        assert_eq!(editing.snapshot().unwrap().text, "éoriginal");
        // A stale revision changes nothing.
        assert!(
            editing
                .edit(
                    0,
                    TextEdit {
                        range: 0..0,
                        replacement: "x".into(),
                    },
                )
                .is_err()
        );
        let mut other = session(app.world_mut(), handle, second, &mut two);
        other.history(HistoryAction::Undo).unwrap();
        assert_eq!(other.snapshot().unwrap().text, "original");
        other.history(HistoryAction::Redo).unwrap();
        assert_eq!(other.snapshot().unwrap().text, "éoriginal");
        let info = app
            .world()
            .workbench()
            .inspect_document(handle.id())
            .unwrap();
        assert!(info.dirty);

        // Composing in one view blocks edits from the other.
        session(app.world_mut(), handle, first, &mut one).set_composing(egui::Id::NULL, true);
        let revision = info.revision;
        let mut other = session(app.world_mut(), handle, second, &mut two);
        assert!(
            other
                .edit(
                    revision,
                    TextEdit {
                        range: 0..0,
                        replacement: "y".into(),
                    },
                )
                .is_err()
        );
    }

    #[test]
    fn open_states_become_a_selection_revealed_in_the_middle() {
        let snapshot = TextSnapshot {
            revision: 3,
            text: TextBuffer::from("one\n\ttwo\nthree"),
        };
        let at = |cursor| TextOpenState {
            cursor: Some(cursor),
            selection: None,
        };
        let line = navigation(
            &snapshot,
            &at(TextPosition::LineColumn { line: 1, column: 4 }),
        );
        // Columns count characters: the tab is one.
        assert_eq!(line.selection.unwrap().head(), 8);
        assert_eq!(
            (line.revision, line.reveal, line.focus),
            (3, Reveal::Center, false)
        );
        let past_the_end = navigation(
            &snapshot,
            &at(TextPosition::LineColumn { line: 9, column: 0 }),
        );
        // The last line's start.
        assert_eq!(past_the_end.selection.unwrap().head(), 9);
        let selected = navigation(
            &snapshot,
            &TextOpenState {
                cursor: None,
                selection: Some(4..9),
            },
        );
        assert_eq!(selected.range, 4..9);
        assert_eq!(
            navigation(&snapshot, &TextOpenState::default()).selection,
            None
        );
    }
}
