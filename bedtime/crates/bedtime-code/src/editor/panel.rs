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

/// How editors start out (the app's settings set them).
#[derive(Resource, Clone, Copy, Debug, PartialEq)]
pub struct EditorDefaults {
    /// Wrap long lines at the editor's width.
    pub wrap: bool,
    pub line_numbers: bool,
}

impl Default for EditorDefaults {
    fn default() -> Self {
        Self {
            wrap: false,
            line_numbers: true,
        }
    }
}

/// Wrap long lines in the focused editor, or stop.
pub struct ToggleWordWrap;
impl bedtime_workbench::Action for ToggleWordWrap {
    const KEY: &'static str = "editor.toggle_word_wrap";
    const LABEL: &'static str = "Word Wrap";
    /// Cmd/Ctrl+Alt+L: Alt+letter types a character on macOS, and text
    /// fields keep every Cmd/Ctrl+Z chord for undo.
    const DEFAULT_KEYS: &'static [bedtime_workbench::KeyChord] =
        &[bedtime_workbench::KeyChord::new(
            bedtime_workbench::Modifiers::PRIMARY.union(bedtime_workbench::Modifiers::ALT),
            KeyCode::KeyL,
        )];
}

fn toggle_word_wrap(
    world: &mut World,
    invocation: bedtime_workbench::ActionInvocation,
) -> bedtime_workbench::ActionResult {
    let panel = invocation.entity.ok_or("No editor is focused")?;
    let defaults = world
        .get_resource::<EditorDefaults>()
        .copied()
        .unwrap_or_default();
    let mut editor = world
        .get_mut::<EditorPanel>(panel)
        .ok_or("No editor is focused")?;
    let wrapping = editor.wrap.unwrap_or(defaults.wrap);
    editor.wrap = Some(!wrapping);
    Ok(())
}

impl Plugin for CodePlugin {
    fn build(&self, app: &mut App) {
        use bedtime_workbench::ActionAppExt;
        app.init_resource::<EditorDefaults>()
            .add_action_handler::<ToggleWordWrap>(
                bedtime_workbench::Handler::new(toggle_word_wrap).available_when(|world, panel| {
                    panel.is_some_and(|p| world.get::<EditorPanel>(p).is_some())
                }),
            );
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
    /// Wrap long lines in this editor; `None` follows `EditorDefaults`.
    #[reflect(default)]
    pub wrap: Option<bool>,
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

/// Draw the editor of `panel` (which has an `EditorPanel` and a document) in
/// `ui`: for a panel that hosts one in its own drawing (mark it
/// `HostedViewer`).
pub fn draw_editor(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, panel: Entity) {
    if let Err(error) = draw(ui, world, context, panel) {
        world
            .workbench_mut()
            .notify(Notification::warning(error.to_string()));
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

/// On an editor panel: show this comparison inline, over its document (old
/// lines between the new, each change's `actions` above it). Whoever adds it
/// reads `ComparisonRequest`s back.
#[derive(Component, Clone)]
pub struct EditorComparison {
    pub comparison: super::diff::Comparison,
    pub actions: Vec<super::diff::DiffAction>,
}

/// On an editor panel showing an `EditorComparison`: the action clicked
/// (taken by whoever compares).
#[derive(Component, Clone, Debug)]
pub struct ComparisonRequest(pub super::diff::DiffRequest);

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
            wrap: editor.wrap,
            ..default()
        };
    }
    editor.view.settings.read_only = !info.writable;
    let defaults = world
        .get_resource::<EditorDefaults>()
        .copied()
        .unwrap_or_default();
    editor.view.settings.wrap = editor.wrap.unwrap_or(defaults.wrap);
    editor.view.settings.line_numbers = defaults.line_numbers;
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
    let comparison = session.world.get::<EditorComparison>(panel).cloned();
    match comparison {
        Some(comparison) => editor
            .view
            .set_comparison(Some(comparison.comparison), comparison.actions),
        None if editor.view.comparing() => editor.view.set_comparison(None, Vec::new()),
        None => {}
    }
    let shown = editor.view.show(ui, id, &mut session, context.motion);
    world.entity_mut(panel).insert(editor);
    if let Ok(output) = &shown
        && let Some(request) = output.diff_request.clone()
    {
        world.entity_mut(panel).insert(ComparisonRequest(request));
    }
    if let Ok(output) = &shown {
        context_menu(world, panel, &output.response);
    }
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

/// What the right-click menu asked for.
enum MenuChoice {
    Action(bedtime_workbench::ActionId),
    Event(egui::Event),
    Paste,
}

/// The right-click menu: the language server's actions (when the file has
/// one), then Cut, Copy, Paste and Select All. A right-click outside the
/// selection has already moved the caret there.
fn context_menu(world: &mut World, panel: Entity, response: &egui::Response) {
    use bedtime_workbench::{ActionId, Keybindings};
    let shortcut = |world: &World, action: ActionId| {
        world
            .get_resource::<Keybindings>()
            .and_then(|keys| keys.keys(&action).first().map(|chord| chord.label()))
            .unwrap_or_default()
    };
    #[cfg(unix)]
    let language: Vec<(ActionId, &'static str, String)> =
        if crate::lsp::server_of(world, panel).is_some() {
            use crate::lsp::{
                CodeActions, FindReferences, FormatDocument, GoToDefinition, RenameSymbol,
            };
            use bedtime_workbench::Action;
            [
                (ActionId::of::<GoToDefinition>(), GoToDefinition::LABEL),
                (ActionId::of::<FindReferences>(), FindReferences::LABEL),
                (ActionId::of::<RenameSymbol>(), RenameSymbol::LABEL),
                (ActionId::of::<CodeActions>(), CodeActions::LABEL),
                (ActionId::of::<FormatDocument>(), FormatDocument::LABEL),
            ]
            .into_iter()
            .map(|(id, label)| (id.clone(), label, shortcut(world, id)))
            .collect()
        } else {
            Vec::new()
        };
    #[cfg(not(unix))]
    let language: Vec<(ActionId, &'static str, String)> = Vec::new();
    let read_only = world
        .get::<EditorPanel>(panel)
        .is_some_and(|editor| editor.view.settings.read_only);
    let has_selection = world
        .get::<EditorPanel>(panel)
        .is_some_and(|editor| !editor.view.selection().range().is_empty());
    let primary = if cfg!(target_os = "macos") {
        "Cmd"
    } else {
        "Ctrl"
    };
    let mut choice = None;
    response.context_menu(|ui| {
        for (action, label, keys) in &language {
            if ui
                .add(egui::Button::new(*label).shortcut_text(keys))
                .clicked()
            {
                choice = Some(MenuChoice::Action(action.clone()));
                ui.close();
            }
        }
        if !language.is_empty() {
            ui.separator();
        }
        let item = |ui: &mut egui::Ui, label: &str, key: &str, enabled: bool| {
            ui.add_enabled(
                enabled,
                egui::Button::new(label).shortcut_text(format!("{primary}+{key}")),
            )
            .clicked()
        };
        if item(ui, "Cut", "X", has_selection && !read_only) {
            choice = Some(MenuChoice::Event(egui::Event::Cut));
            ui.close();
        }
        if item(ui, "Copy", "C", has_selection) {
            choice = Some(MenuChoice::Event(egui::Event::Copy));
            ui.close();
        }
        if item(ui, "Paste", "V", !read_only) {
            choice = Some(MenuChoice::Paste);
            ui.close();
        }
        ui.separator();
        if item(ui, "Select All", "A", true) {
            choice = Some(MenuChoice::Event(egui::Event::Key {
                key: egui::Key::A,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::COMMAND,
            }));
            ui.close();
        }
    });
    let event = match choice {
        None => return,
        Some(MenuChoice::Action(action)) => {
            if let Some(mut requests) =
                world.get_resource_mut::<bedtime_workbench::ActionRequests>()
            {
                requests.submit(bedtime_workbench::ActionRequest {
                    action,
                    context: bedtime_workbench::InvocationContext::Entity(panel),
                    source: Some(panel),
                });
            }
            return;
        }
        Some(MenuChoice::Event(event)) => event,
        Some(MenuChoice::Paste) => {
            let text = world
                .get_resource_mut::<bedtime_workbench::Clipboard>()
                .and_then(|mut clipboard| clipboard.read_text().ok());
            match text {
                Some(text) => egui::Event::Paste(text),
                None => return,
            }
        }
    };
    if let Some(mut editor) = world.get_mut::<EditorPanel>(panel) {
        editor.view.queue_event(event);
    }
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
        (None, Some(cursor)) => match &state.anchor {
            Some(anchor) => TextSelection::new(text, byte(anchor), byte(cursor)).ok(),
            None => TextSelection::caret(text, byte(cursor)).ok(),
        },
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
            ..Default::default()
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
                selection: Some(4..9),
                ..Default::default()
            },
        );
        assert_eq!(selected.range, 4..9);
        assert_eq!(
            navigation(&snapshot, &TextOpenState::default()).selection,
            None
        );
    }
}
