//! Viewport-only text editor for workbench documents. Canonical text and
//! history belong to the workbench document; views own selection, composition
//! and navigation. No full-file string or galley is constructed while drawing
//! or performing ordinary edits.
mod background;
mod caret;
mod carets;
pub(super) mod saving;
pub use saving::FormatAndSave;
pub(crate) use saving::formatted;
mod comments;
mod completion;
pub mod diff;
mod lines;
mod pairs;
pub use completion::*;
mod folding;
mod search;
mod search_ui;
pub mod status;
pub mod syntax;
mod whitespace;
mod wrapping;
pub use search::SearchQuery;
pub use whitespace::Indentation;
mod colors;
mod commands;
pub(crate) use commands::EditorCommandsAppExt;
pub use commands::{
    AddCaretAbove, AddCaretBelow, AddCaretsToLineEnds, AddNextOccurrence, CopyLineDown, CopyLineUp,
    DeleteLine, EditorAction, EditorCommand, ExpandSelection, Fold, FoldAll, IndentLines,
    InsertLineAbove, InsertLineBelow, JoinLines, JumpToBracket, MoveLineDown, MoveLineUp,
    OutdentLines, SelectAllOccurrences, SelectLine, ShrinkSelection, ToggleBlockComment,
    ToggleFold, ToggleLineComment, Unfold, UnfoldAll,
};
mod diagnostics;
mod document;
mod layout;
mod overview;
pub use diagnostics::*;
mod text {
    pub use bedtime_document::text::*;
}
pub use document::*;
pub use text::{BufferEdit, TextBuffer};
mod panel;
use bedtime_ui::{MotionPolicy, egui};
use bevy::prelude::Component;
#[cfg(feature = "persistence")]
pub use panel::{
    AutoCloseBrackets, Breadcrumbs, FormatOnSave, HighlightOccurrences, InsertFinalNewline,
    LineNumbers, TrimTrailingWhitespace, WrapLines,
};
pub use panel::{
    CodePlugin, ComparisonRequest, EditorComparison, EditorDefaults, EditorPanel, ToggleWordWrap,
    draw_editor, editor_id,
};
use std::ops::Range;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TextSelection {
    anchor: usize,
    head: usize,
}
impl TextSelection {
    pub fn new(text: &TextBuffer, anchor: usize, head: usize) -> Result<Self, EditorError> {
        if !text.is_char_boundary(anchor) || !text.is_char_boundary(head) {
            return Err(EditorError(
                "Selection must use valid UTF-8 byte boundaries".into(),
            ));
        }
        Ok(Self { anchor, head })
    }
    pub fn caret(text: &TextBuffer, position: usize) -> Result<Self, EditorError> {
        Self::new(text, position, position)
    }
    pub fn anchor(self) -> usize {
        self.anchor
    }
    pub fn head(self) -> usize {
        self.head
    }
    pub fn range(self) -> Range<usize> {
        self.anchor.min(self.head)..self.anchor.max(self.head)
    }
    fn mapped(self, change: &TextDelta) -> Self {
        Self {
            anchor: change.map_position(self.anchor),
            head: change.map_position(self.head),
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Scroll {
    #[default]
    Minimal,
    Center,
}
#[derive(Clone, Debug)]
pub struct Navigation {
    pub revision: u64,
    pub range: Range<usize>,
    pub selection: Option<TextSelection>,
    pub reveal: Scroll,
    pub focus: bool,
}
#[derive(Clone, Debug)]
pub struct EditorSettings {
    pub line_numbers: bool,
    pub read_only: bool,
    pub animate_caret: bool,
    pub wrap: bool,
    pub smart_indent: bool,
    pub indentation: Indentation,
    /// Close brackets and quotes as they're typed.
    pub auto_close: bool,
    /// Highlight the other occurrences of the selected text.
    pub highlight_occurrences: bool,
}
impl Default for EditorSettings {
    fn default() -> Self {
        Self {
            line_numbers: true,
            read_only: false,
            animate_caret: true,
            wrap: false,
            smart_indent: true,
            indentation: Indentation::default(),
            auto_close: true,
            highlight_occurrences: true,
        }
    }
}
struct Composition {
    revision: u64,
    selection: TextSelection,
    text: String,
}
#[derive(Component, Default)]
pub struct EditorView {
    pub settings: EditorSettings,
    folding: folding::Folding,
    diff: diff::DiffState,
    /// A language server's diagnostics, drawn only at their revision.
    diagnostics: Option<std::sync::Arc<Diagnostics>>,
    /// Their marks along the scroll bar.
    overview: overview::Overview,
    /// Report the word under the pointer (for hover) and Cmd/Ctrl-clicks on
    /// it (go to definition) instead of moving the caret.
    pub language_tools: bool,
    assistance: completion::Assistance,
    /// Colors in the text: underlined, with a picker on hover.
    color_picker: colors::Picker,
    /// Whether the text has a language: reserves the fold column before its
    /// first analysis arrives, so text doesn't shift.
    pub has_syntax: bool,
    /// The text's language (before any analysis): comments are its.
    pub language: Option<&'static str>,
    /// The current analysis (folds, outline) and the one colours come from,
    /// which may be older while a new one runs.
    syntax: Option<std::sync::Arc<crate::syntax::Observation>>,
    syntax_identity: Option<(u64, u64)>,
    syntax_highlights: Option<std::sync::Arc<crate::syntax::Observation>>,
    pub syntax_palette: syntax::SyntaxPalette,
    /// The primary selection: the one completion, search and language tools
    /// use. Other carets are in `others`.
    selection: TextSelection,
    others: carets::Others,
    /// What Copy took as whole lines (nothing selected): pasted above lines.
    line_clipboard: Option<String>,
    expansions: lines::Expansions,
    /// The bracket pair at the primary caret, for (revision, caret, has an
    /// analysis).
    brackets: Option<((u64, usize, bool), Option<(usize, usize)>)>,
    /// A box selection's starting line and column, while dragging.
    box_anchor: Option<(usize, usize)>,
    observed_revision: Option<u64>,
    navigation: Option<Navigation>,
    composition: Option<Composition>,
    caret: caret::CaretAnimation,
    last_edit: Option<f64>,
    preferred_column: Option<usize>,
    upstream: bool,
    focused: bool,
    request_focus: bool,
    scroll: egui::Vec2,
    scroll_gesture: ScrollGesture,
    drag_anchor: Option<usize>,
    pointer_presses: PointerPresses,
    wrapping: wrapping::Wrapping,
    wrap_active: bool,
    wrap_reveal: Option<(Range<usize>, Scroll)>,
    search: search::Search,
    indent_job: Option<(
        u64,
        TextSelection,
        background::Job<(BufferEdit, TextSelection)>,
    )>,
    edit_message: Option<String>,
    /// A range another panel points at (Structure's hovered entry), shaded
    /// while it's renewed each pass: its revision, the range, the egui pass.
    pub(crate) peek: Option<(u64, Range<usize>, u64)>,
    /// Editing asked for from outside the keyboard (a context menu's Copy,
    /// Paste…, an editor action from a menu), handled on the next draw as
    /// if typed.
    queued: Vec<Queued>,
}
enum Queued {
    Event(egui::Event),
    Command(EditorCommand),
}
/// Lock each wheel input to its dominant axis. Keep that choice only for
/// egui's smoothed tail, never across a new independent input.
#[derive(Default)]
struct ScrollGesture {
    axis: Option<usize>,
}
impl ScrollGesture {
    fn multiplier(&mut self, delta: egui::Vec2) -> egui::Vec2 {
        if delta != egui::Vec2::ZERO {
            self.axis = Some(if delta.x.abs() > delta.y.abs() { 0 } else { 1 });
        }
        match self.axis {
            Some(0) => egui::vec2(1.0, 0.0),
            Some(_) => egui::vec2(0.0, 1.0),
            None => egui::Vec2::splat(1.0),
        }
    }
}

#[derive(Default)]
struct PointerPresses {
    last: Option<(egui::Pos2, f64)>,
    count: u8,
}
impl PointerPresses {
    fn press(&mut self, position: egui::Pos2, now: f64, max_delay: f64, max_distance: f32) -> u8 {
        let continues = self.last.is_some_and(|(previous, time)| {
            now - time < max_delay && previous.distance(position) < max_distance
        });
        self.count = if continues { self.count % 3 + 1 } else { 1 };
        self.last = Some((position, now));
        self.count
    }
}
/// A word or character under the pointer.
#[derive(Clone, Debug)]
pub struct SymbolHit {
    pub byte: usize,
    pub range: Range<usize>,
    pub rect: egui::Rect,
}

pub struct EditorOutput {
    /// The word under the pointer (with `language_tools`).
    /// A comparison's action clicked this frame (Stage, Discard, …).
    pub diff_request: Option<diff::DiffRequest>,
    pub diff_controls: Vec<(u64, usize, egui::Rect)>,
    pub hovered_symbol: Option<SymbolHit>,
    /// The character under the pointer (with `language_tools`).
    pub hovered_text: Option<SymbolHit>,
    /// Cmd/Ctrl-click on a word: its byte.
    pub definition_clicked: Option<usize>,
    /// Ctrl-Space.
    pub completion_requested: bool,
    /// The text before the caret changed under an open menu (Backspace).
    pub completion_refresh: bool,
    /// The menu entry accepted (Enter, Tab, click).
    pub completion_accepted: Option<usize>,
    pub completion_rect: Option<egui::Rect>,
    /// Escape closed the menu (or a snippet).
    pub completion_dismissed: bool,
    /// Text typed this frame.
    pub typed: Option<String>,
    pub response: egui::Response,
    pub text_rect: egui::Rect,
    pub caret_rect: egui::Rect,
    pub selection: TextSelection,
    pub revision: u64,
    pub composing: bool,
    pub scroll: egui::Vec2,
    pub rendered_lines: usize,
    pub laid_out_bytes: usize,
}
impl Navigation {
    /// Zero-based logical line. Reveal-only preserves the view's caret.
    pub fn scroll_to_line(
        snapshot: &TextSnapshot,
        line: usize,
        reveal: Scroll,
        focus: bool,
    ) -> Result<Self, EditorError> {
        if line >= snapshot.text.line_count() {
            return Err(EditorError("Line is outside the document".into()));
        }
        let byte = snapshot.text.line_start(line);
        Ok(Self {
            revision: snapshot.revision,
            range: byte..byte,
            selection: None,
            reveal,
            focus,
        })
    }
}
impl EditorView {
    pub fn scroll_to_line(
        &mut self,
        snapshot: &TextSnapshot,
        line: usize,
        reveal: Scroll,
        focus: bool,
    ) -> Result<(), EditorError> {
        self.request_navigation(
            snapshot,
            Navigation::scroll_to_line(snapshot, line, reveal, focus)?,
        )
    }
    /// A comparison is shown (folding is off meanwhile).
    fn diff_active(&self) -> bool {
        self.diff.active()
    }
    /// Whether a comparison is set.
    pub fn comparing(&self) -> bool {
        self.diff.active()
    }
    fn prepare_folds(&mut self, snapshot: &TextSnapshot, width: Option<usize>) {
        let wrap = self
            .wrapping
            .index
            .as_ref()
            .filter(|w| width.is_some() && w.snapshot.revision == snapshot.revision);
        self.folding.prepare(
            &snapshot.text,
            wrap,
            self.composition.is_none() && !self.diff_active(),
        );
    }
    fn reconcile_folds(
        &mut self,
        session: &dyn TextSession,
        snapshot: &TextSnapshot,
    ) -> Result<(), EditorError> {
        if self.folding.revision() == Some(snapshot.revision) {
            return Ok(());
        }
        if self.folding.collapsed.is_empty() {
            self.folding.reconcile(snapshot, None);
            return Ok(());
        }
        let changes = if let Some(before) = self.folding.revision() {
            session.changes_since(before)?.and_then(|commits| {
                commits.into_iter().try_fold(Vec::new(), |mut all, c| {
                    all.extend(c.changes?);
                    Some(all)
                })
            })
        } else {
            None
        };
        self.folding.reconcile(snapshot, changes.as_deref());
        Ok(())
    }

    /// Diagnostics to underline; `None` removes them. Shown only while the
    /// text is at the report's revision.
    pub fn set_diagnostics(&mut self, report: Option<std::sync::Arc<Diagnostics>>) {
        self.diagnostics = report;
    }
    /// Handle `event` (`Copy`, `Cut`, `Paste(text)`, a key) on the next draw,
    /// as if it were typed.
    pub fn queue_event(&mut self, event: egui::Event) {
        self.queued.push(Queued::Event(event));
    }
    /// Run `command` on the next draw, as if its keys were pressed.
    pub fn queue_command(&mut self, command: EditorCommand) {
        self.queued.push(Queued::Command(command));
    }
    pub fn selection(&self) -> TextSelection {
        self.selection
    }
    /// The carets there are: the primary and the others.
    pub fn caret_count(&self) -> usize {
        1 + self.others.list.len()
    }
    pub fn scroll_offset(&self) -> egui::Vec2 {
        self.scroll
    }
    pub fn wrapping_ready(&self) -> bool {
        self.settings.wrap && self.wrap_active
    }
    pub fn search_query(&self) -> Option<&SearchQuery> {
        self.search.open.then_some(&self.search.query)
    }
    pub fn search_match_count(&self) -> Option<usize> {
        self.search
            .results
            .as_ref()
            .filter(|r| {
                self.search.open
                    && r.query == self.search.query
                    && Some(r.revision) == self.observed_revision
            })
            .map(|r| r.count)
    }
    /// Whether the text had the keyboard when last drawn (not the Find
    /// field, a popup, another panel): its commands run only then.
    pub fn has_keyboard(&self) -> bool {
        self.focused
    }
    pub fn is_composing(&self) -> bool {
        self.composition.is_some()
    }
    /// Take a requested navigation's selection now, as the next draw would.
    #[cfg(test)]
    pub(crate) fn settle_navigation(&mut self) {
        if let Some(selection) = self.navigation.take().and_then(|nav| nav.selection) {
            self.clear_others();
            self.selection = selection;
        }
    }
    pub fn request_navigation(
        &mut self,
        snapshot: &TextSnapshot,
        navigation: Navigation,
    ) -> Result<(), EditorError> {
        if self.is_composing() {
            return Err(EditorError("Finish composition before navigating".into()));
        }
        if navigation.revision != snapshot.revision
            || navigation.range.start > navigation.range.end
            || !snapshot.text.is_char_boundary(navigation.range.start)
            || !snapshot.text.is_char_boundary(navigation.range.end)
        {
            return Err(EditorError(
                "Navigation has a stale revision or invalid byte range".into(),
            ));
        }
        if let Some(selection) = navigation.selection {
            TextSelection::new(&snapshot.text, selection.anchor, selection.head)?;
        }
        self.navigation = Some(navigation);
        Ok(())
    }
    fn cancel_composition(&mut self, session: &mut dyn TextSession, id: egui::Id) {
        self.composition = None;
        session.set_composing(id, false);
    }
    fn replace_selection(
        &mut self,
        id: egui::Id,
        session: &mut dyn TextSession,
        snapshot: &mut TextSnapshot,
        text: String,
        now: f64,
    ) -> Result<(), EditorError> {
        let range = self.selection.range();
        let head = range.start + text.len();
        session.set_edit_owner(id);
        session.edit(
            snapshot.revision,
            TextEdit {
                range,
                replacement: text,
            },
        )?;
        *snapshot = session.snapshot()?;
        self.selection = TextSelection::caret(&snapshot.text, head)?;
        self.upstream = false;
        self.preferred_column = None;
        self.last_edit = Some(now);
        Ok(())
    }
    fn apply_edit(
        &mut self,
        id: egui::Id,
        session: &mut dyn TextSession,
        snapshot: &mut TextSnapshot,
        edit: TextEdit,
        selection: TextSelection,
        now: f64,
    ) -> Result<(), EditorError> {
        self.apply_buffer_edit(
            id,
            session,
            snapshot,
            BufferEdit::new(edit.range, edit.replacement),
            selection,
            now,
        )
    }
    fn apply_buffer_edit(
        &mut self,
        id: egui::Id,
        session: &mut dyn TextSession,
        snapshot: &mut TextSnapshot,
        edit: BufferEdit,
        selection: TextSelection,
        now: f64,
    ) -> Result<(), EditorError> {
        session.set_edit_owner(id);
        session.edit_buffer(snapshot.revision, edit)?;
        *snapshot = session.snapshot()?;
        self.selection = TextSelection::new(&snapshot.text, selection.anchor, selection.head)?;
        self.upstream = false;
        self.preferred_column = None;
        self.last_edit = Some(now);
        Ok(())
    }
    fn active_wrap(
        &self,
        snapshot: &TextSnapshot,
        width: Option<usize>,
    ) -> Option<&wrapping::WrapIndex> {
        self.wrapping
            .index
            .as_ref()
            .filter(|index| width.is_some() && index.snapshot.revision == snapshot.revision)
    }
    fn prepare_wrapping(
        &mut self,
        ctx: &egui::Context,
        session: &dyn TextSession,
        snapshot: &TextSnapshot,
        width: Option<usize>,
    ) -> Result<(), EditorError> {
        let changes = if let Some(index) = &self.wrapping.index {
            if index.snapshot.revision != snapshot.revision {
                session
                    .changes_since(index.snapshot.revision)?
                    .and_then(|commits| {
                        commits
                            .into_iter()
                            .try_fold(Vec::new(), |mut changes, commit| {
                                changes.extend(commit.changes?);
                                Some(changes)
                            })
                    })
            } else {
                Some(Vec::new())
            }
        } else {
            None
        };
        self.wrapping
            .prepare(ctx, snapshot, width, changes.as_deref());
        Ok(())
    }
    pub(crate) fn show(
        &mut self,
        ui: &mut egui::Ui,
        id: egui::Id,
        session: &mut dyn TextSession,
        motion: MotionPolicy,
    ) -> Result<EditorOutput, EditorError> {
        let now = ui.input(|i| i.time);
        let mut snapshot = session.snapshot()?;
        if self
            .composition
            .as_ref()
            .is_some_and(|c| c.revision != snapshot.revision)
        {
            ui.label("The document changed during text composition.");
            if ui.button("Discard composition").clicked() {
                self.cancel_composition(session, id);
            } else {
                return Err(EditorError(
                    "Composition retained until explicitly discarded".into(),
                ));
            }
        }
        if let Some(before) = self.observed_revision {
            if before != snapshot.revision {
                self.selection = map_selection(session, before, &snapshot, self.selection)?;
                session.end_undo_group();
            }
            self.sync_others(session, &snapshot)?;
        } else {
            session.end_undo_group();
        }
        self.prepare_assistance(id, session, &mut snapshot)?;
        if let Some((revision, selection, job)) = &self.indent_job {
            let (revision, selection) = (*revision, *selection);
            match job.poll() {
                Ok(Some((edit, mapped))) => {
                    self.indent_job = None;
                    if revision != snapshot.revision
                        || selection != self.selection
                        || self.settings.read_only
                        || self.navigation.is_some()
                        || self.is_composing()
                    {
                        self.edit_message =
                            Some("Editor changed; indentation was not applied. Try again.".into());
                    } else {
                        session.end_undo_group();
                        let result =
                            self.apply_buffer_edit(id, session, &mut snapshot, edit, mapped, now);
                        session.end_undo_group();
                        if let Err(error) = result {
                            self.edit_message =
                                Some(format!("Indentation was not applied: {error}"));
                        }
                    }
                }
                Err(()) => {
                    self.indent_job = None;
                    self.edit_message =
                        Some("Indentation worker stopped; no changes applied".into());
                }
                _ => {}
            }
        }
        if self.indent_job.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Preparing indentation…");
                if ui.button("Cancel indentation").clicked() {
                    self.indent_job = None;
                }
            });
        }
        if let Some(message) = self.edit_message.clone() {
            ui.horizontal_wrapped(|ui| {
                ui.weak(message);
                if ui.button("Dismiss").clicked() {
                    self.edit_message = None;
                }
            });
        }
        self.reconcile_folds(session, &snapshot)?;
        self.search_ui(ui, id, session, &mut snapshot)?;
        let navigation = self.navigation.take();
        if let Some(nav) = &navigation {
            if nav.revision != snapshot.revision {
                return Err(EditorError(
                    "Navigation became stale before presentation".into(),
                ));
            }
            self.folding.reveal(&snapshot.text, &nav.range);
            if let Some(selection) = nav.selection {
                self.clear_others();
                self.selection = selection;
                self.upstream = false;
            }
            if nav.focus {
                ui.memory_mut(|memory| memory.request_focus(id));
            }
            self.preferred_column = None;
            session.end_undo_group();
        }
        if self.last_edit.is_some_and(|time| now - time > 0.65) {
            session.end_undo_group();
            self.last_edit = None;
        }
        let metrics = layout::Metrics::new(ui);
        let available = ui.available_size().max(egui::vec2(1.0, 1.0));
        let gutter_width = if self.settings.line_numbers {
            snapshot.text.line_count().to_string().len().max(2) as f32 * metrics.cell + 8.0
        } else {
            0.0
        };
        let fold_gutter = self.has_syntax && !self.diff_active();
        // A comparison shows old and new line numbers.
        let gutter_width = if self.diff_active() {
            gutter_width * 2.0 + metrics.cell * 2.0
        } else {
            gutter_width
        };
        // The fold column, or (numbers are drawn 7 points in from the right)
        // the same inset on the left as with one.
        let gutter_width = gutter_width
            + if fold_gutter {
                14.0
            } else if self.settings.line_numbers && !self.diff_active() {
                7.0
            } else {
                0.0
            };
        let hovered = ui.rect_contains_pointer(ui.available_rect_before_wrap());
        let wheel = if hovered {
            let delta = ui.input(|i| {
                i.events
                    .iter()
                    .filter_map(|event| {
                        if let egui::Event::MouseWheel {
                            delta, modifiers, ..
                        } = event
                        {
                            Some(if modifiers.shift {
                                egui::vec2(delta.x + delta.y, 0.0)
                            } else {
                                *delta
                            })
                        } else {
                            None
                        }
                    })
                    .fold(egui::Vec2::ZERO, |total, delta| total + delta)
            });
            self.scroll_gesture.multiplier(delta)
        } else {
            egui::Vec2::splat(1.0)
        };
        let mut reveal = navigation
            .as_ref()
            .map(|nav| (nav.range.clone(), nav.reveal));
        let mut failure = None;
        let mut number_rows = Vec::new();
        let mut historical_numbers = Vec::new();
        let mut diff_request = None;
        let mut diff_controls = Vec::new();
        let mut caret_rect = egui::Rect::NOTHING;
        let mut hovered_symbol = None;
        let mut hovered_text = None;
        let mut color_hits: Vec<colors::Hit> = Vec::new();
        let mut definition_clicked = None;
        let mut rendered_lines = 0;
        let mut laid_out_bytes = 0;
        let (gutter, scroll) = ui
            .horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                let gutter = ui
                    .allocate_exact_size(
                        egui::vec2(gutter_width, available.y),
                        egui::Sense::hover(),
                    )
                    .0;
                let mut scroll = egui::ScrollArea::new([!self.settings.wrap, true]);
                if self.settings.wrap {
                    // Reset an unwrapped view's offset even while its wrap
                    // index is still being prepared.
                    scroll = scroll.horizontal_scroll_offset(0.0);
                }
                let scroll = scroll
                    .id_salt(id.with("scroll"))
                    .wheel_scroll_multiplier(wheel)
                    .auto_shrink([false, false])
                    .max_width((available.x - gutter_width).max(1.0))
                    .max_height(available.y)
                    .scroll_source(egui::scroll_area::ScrollSource {
                        drag: egui::scroll_area::DragScroll::Never,
                        ..Default::default()
                    })
                    .show_viewport(ui, |ui, viewport| {
                        let origin = ui.cursor().min;
                        let wrap_width = self.settings.wrap.then_some(
                            (((viewport.width() - 16.0) / metrics.cell).floor() as usize).max(8),
                        );
                        if let Err(error) =
                            self.prepare_wrapping(ui.ctx(), session, &snapshot, wrap_width)
                        {
                            failure = Some(error);
                        }
                        {
                            let wrap = self.active_wrap(&snapshot, wrap_width).cloned();
                            self.diff.prepare(ui.ctx(), &snapshot, wrap);
                        }
                        self.prepare_folds(&snapshot, wrap_width);
                        self.folding.animate(
                            &snapshot.text,
                            self.wrapping.index.as_ref().filter(|index| {
                                wrap_width.is_some() && index.snapshot.revision == snapshot.revision
                            }),
                            self.composition.is_none() && !self.diff_active(),
                            now,
                            motion.enabled,
                            ui.ctx(),
                        );
                        let geometry = layout::Geometry {
                            text: &snapshot.text,
                            metrics: &metrics,
                            wrap: self.active_wrap(&snapshot, wrap_width),
                            folds: &self.folding.display_map,
                            diff: self
                                .diff
                                .current(snapshot.revision, self.active_wrap(&snapshot, wrap_width))
                                .filter(|_| self.composition.is_none()),
                        };
                        let size = geometry.size(viewport.size());
                        let rect = egui::Rect::from_min_size(origin, size);
                        ui.allocate_rect(rect, egui::Sense::hover());
                        let response = ui.interact(rect, id, egui::Sense::click_and_drag());
                        ui.ctx().register_text_input(id);
                        if response.hovered() {
                            ui.set_cursor_icon(egui::CursorIcon::Text);
                        }
                        // Shift+Alt-drag (or a middle-button drag) selects a box:
                        // one selection per line, between two columns.
                        let box_press = response.hovered()
                            && self.composition.is_none()
                            && ui.input(|i| {
                                i.pointer.primary_pressed() && i.modifiers.alt && i.modifiers.shift
                                    || i.pointer.button_pressed(egui::PointerButton::Middle)
                            });
                        let box_drag = self.box_anchor.is_some()
                            && (response.dragged_by(egui::PointerButton::Primary)
                                || response.dragged_by(egui::PointerButton::Middle));
                        if (box_press || box_drag)
                            && let Some(position) = ui.input(|i| i.pointer.interact_pos())
                        {
                            let geometry = layout::Geometry {
                                text: &snapshot.text,
                                metrics: &metrics,
                                wrap: self.active_wrap(&snapshot, wrap_width),
                                folds: &self.folding.display_map,
                                diff: self.diff.current(
                                    snapshot.revision,
                                    self.active_wrap(&snapshot, wrap_width),
                                ),
                            };
                            let local = position - origin.to_vec2();
                            let hit = geometry.hit(ui, local);
                            let point = (
                                snapshot.text.line_of(hit),
                                ((local.x - 8.0) / metrics.cell).round().max(0.0) as usize,
                            );
                            if box_press {
                                self.cancel_composition(session, id);
                                response.request_focus();
                                session.end_undo_group();
                                self.pointer_presses.last = None;
                                self.box_anchor = Some(point);
                            }
                            if let Some(anchor) = self.box_anchor {
                                self.select_box(&snapshot, anchor, point);
                                reveal = Some((hit..hit, Scroll::Minimal));
                            }
                        }
                        if ui.input(|i| i.pointer.any_released()) {
                            self.box_anchor = None;
                        }
                        let pressed = ui.input(|i| i.pointer.primary_pressed());
                        if response.hovered()
                            && pressed
                            && !box_press
                            && !(self.language_tools && ui.input(|i| i.modifiers.command))
                        {
                            self.cancel_composition(session, id);
                            response.request_focus();
                            let position = ui.input(|i| i.pointer.interact_pos()).unwrap();
                            let geometry = layout::Geometry {
                                text: &snapshot.text,
                                metrics: &metrics,
                                wrap: self.active_wrap(&snapshot, wrap_width),
                                folds: &self.folding.display_map,
                                diff: self
                                    .diff
                                    .current(
                                        snapshot.revision,
                                        self.active_wrap(&snapshot, wrap_width),
                                    )
                                    .filter(|_| self.composition.is_none()),
                            };
                            let local = position - origin.to_vec2();
                            let hit = geometry.hit(ui, local);
                            self.upstream = geometry.visual_row(hit)
                                > (((local.y - 6.0) / metrics.height).floor().max(0.0) as usize);
                            session.end_undo_group();
                            self.preferred_column = None;
                            let clicks = ui.ctx().options(|options| {
                                self.pointer_presses.press(
                                    position,
                                    now,
                                    options.input_options.max_double_click_delay,
                                    options.input_options.max_click_dist,
                                )
                            });
                            // Alt-click adds a caret (or removes one); other
                            // clicks leave just the primary.
                            let alt = ui.input(|i| i.modifiers.alt);
                            if alt {
                                if let Err(error) = self.sync_others(&*session, &snapshot) {
                                    failure = Some(error);
                                }
                            } else {
                                self.clear_others();
                            }
                            if clicks == 3 {
                                let range = snapshot.text.line_range(snapshot.text.line_of(hit));
                                self.selection = TextSelection {
                                    anchor: range.start,
                                    head: range.end,
                                };
                            } else if clicks == 2 {
                                let range = word_range(&snapshot.text, hit);
                                self.selection = TextSelection {
                                    anchor: range.start,
                                    head: range.end,
                                };
                            } else if ui.input(|i| i.modifiers.shift) {
                                self.selection.head = hit;
                            } else if alt {
                                self.toggle_caret(snapshot.revision, hit);
                            } else {
                                self.selection = TextSelection {
                                    anchor: hit,
                                    head: hit,
                                };
                            }
                            self.drag_anchor = Some(self.selection.anchor);
                            self.selection =
                                self.select_through_folds(&snapshot.text, self.selection);
                        }
                        // A right-click outside the selection moves the caret
                        // there first, so a context menu acts where you clicked.
                        if response.hovered()
                            && ui.input(|i| i.pointer.secondary_pressed())
                            && self.composition.is_none()
                            && let Some(position) = ui.input(|i| i.pointer.interact_pos())
                        {
                            let geometry = layout::Geometry {
                                text: &snapshot.text,
                                metrics: &metrics,
                                wrap: self.active_wrap(&snapshot, wrap_width),
                                folds: &self.folding.display_map,
                                diff: self
                                    .diff
                                    .current(
                                        snapshot.revision,
                                        self.active_wrap(&snapshot, wrap_width),
                                    )
                                    .filter(|_| self.composition.is_none()),
                            };
                            let hit = geometry.hit(ui, position - origin.to_vec2());
                            let selected = self.selection.range();
                            if !(selected.start..=selected.end).contains(&hit)
                                || selected.is_empty()
                            {
                                session.end_undo_group();
                                self.preferred_column = None;
                                self.clear_others();
                                self.selection = TextSelection {
                                    anchor: hit,
                                    head: hit,
                                };
                            }
                            response.request_focus();
                        }
                        if response.dragged_by(egui::PointerButton::Primary)
                            && self.box_anchor.is_none()
                            && !(self.language_tools && ui.input(|i| i.modifiers.command))
                        {
                            self.pointer_presses.last = None;
                            if let Some(position) = ui.input(|i| i.pointer.interact_pos()) {
                                let geometry = layout::Geometry {
                                    text: &snapshot.text,
                                    metrics: &metrics,
                                    wrap: self.active_wrap(&snapshot, wrap_width),
                                    folds: &self.folding.display_map,
                                    diff: self
                                        .diff
                                        .current(
                                            snapshot.revision,
                                            self.active_wrap(&snapshot, wrap_width),
                                        )
                                        .filter(|_| self.composition.is_none()),
                                };
                                let local = position - origin.to_vec2();
                                let hit = geometry.hit(ui, local);
                                self.upstream = geometry.visual_row(hit)
                                    > (((local.y - 6.0) / metrics.height).floor().max(0.0)
                                        as usize);
                                self.selection = self.select_through_folds(
                                    &snapshot.text,
                                    TextSelection {
                                        anchor: self.drag_anchor.unwrap_or(self.selection.anchor),
                                        head: hit,
                                    },
                                );
                                reveal = Some((hit..hit, Scroll::Minimal));
                            }
                        }
                        let focused =
                            response.has_focus() && ui.input(|i| i.focused) && ui.is_enabled();
                        if !focused && self.composition.is_some() {
                            self.cancel_composition(session, id);
                        }
                        if focused {
                            self.lock_focus(ui, id);
                            let queued = std::mem::take(&mut self.queued);
                            for queued in &queued {
                                let handled = match queued {
                                    Queued::Event(event) => self.handle_event(
                                        ui,
                                        id,
                                        session,
                                        &mut snapshot,
                                        event,
                                        now,
                                        (viewport.height() / metrics.height).max(1.0) as usize,
                                    ),
                                    Queued::Command(command) => self.run_command(
                                        ui,
                                        id,
                                        session,
                                        &mut snapshot,
                                        *command,
                                        now,
                                    ),
                                };
                                match handled {
                                    Ok(true) => {
                                        reveal = Some((
                                            self.selection.head..self.selection.head,
                                            Scroll::Minimal,
                                        ))
                                    }
                                    Ok(false) => {}
                                    Err(error) => failure = Some(error),
                                }
                            }
                            let events = ui.input(|input| input.events.clone());
                            let mut consumed = Vec::new();
                            for (index, event) in events.iter().enumerate() {
                                match self.handle_event(
                                    ui,
                                    id,
                                    session,
                                    &mut snapshot,
                                    event,
                                    now,
                                    (viewport.height() / metrics.height).max(1.0) as usize,
                                ) {
                                    Ok(handled) => {
                                        if handled {
                                            consumed.push(index);
                                            reveal = Some((
                                                self.selection.head..self.selection.head,
                                                Scroll::Minimal,
                                            ));
                                        }
                                    }
                                    Err(error) => {
                                        consumed.push(index);
                                        failure = Some(error);
                                        break;
                                    }
                                }
                            }
                            ui.input_mut(|input| {
                                let mut index = 0;
                                input.events.retain(|_| {
                                    let keep = !consumed.contains(&index);
                                    index += 1;
                                    keep
                                });
                            });
                            // What the events changed (carets added) counts next frame.
                            self.lock_focus(ui, id);
                        }
                        if self.focused && !focused {
                            session.end_undo_group();
                        }
                        self.focused = focused;
                        if let Err(error) = self.sync_others(session, &snapshot) {
                            failure = Some(error);
                        }
                        let bracket_pair = if self.composition.is_none() {
                            self.bracket_pair(&snapshot)
                        } else {
                            None
                        };
                        let pass = ui.ctx().cumulative_pass_nr();
                        let peek = self
                            .peek
                            .clone()
                            .filter(|(revision, _, at)| {
                                *revision == snapshot.revision && at + 1 >= pass
                            })
                            .map(|(_, range, _)| range);
                        let occurrence =
                            if self.composition.is_none() && self.settings.highlight_occurrences {
                                self.occurrence(&snapshot)
                            } else {
                                None
                            };
                        let (display, selection) = if let Some(composition) = &self.composition {
                            let mut display = snapshot.text.clone();
                            BufferEdit::new(
                                composition.selection.range(),
                                composition.text.as_str(),
                            )
                            .apply(&mut display)
                            .expect("Validated draft range");
                            let head = composition.selection.range().start + composition.text.len();
                            (display, TextSelection { anchor: head, head })
                        } else {
                            (snapshot.text.clone(), self.selection)
                        };
                        if let Err(error) =
                            self.prepare_wrapping(ui.ctx(), session, &snapshot, wrap_width)
                        {
                            failure = Some(error);
                        }
                        self.reconcile_folds(session, &snapshot)
                            .unwrap_or_else(|error| failure = Some(error));
                        if let Some((range, _)) = &reveal {
                            self.folding.reveal(&snapshot.text, range);
                        }
                        let highlight_changes = self
                            .highlight_changes(session, &snapshot)
                            .unwrap_or_else(|error| {
                                failure = Some(error);
                                None
                            });
                        let active_wrap = self.wrapping.index.as_ref().filter(|index| {
                            wrap_width.is_some() && index.snapshot.revision == snapshot.revision
                        });
                        let draft_wrap = if let (Some(index), Some(composition)) =
                            (active_wrap, &self.composition)
                        {
                            let mut draft = index.clone();
                            let change = TextDelta {
                                range: composition.selection.range(),
                                inserted_bytes: composition.text.len(),
                            };
                            draft
                                .update(
                                    TextSnapshot {
                                        revision: snapshot.revision,
                                        text: display.clone(),
                                    },
                                    &[change],
                                )
                                .then_some(draft)
                        } else {
                            None
                        };
                        let wrap = if self.composition.is_some() {
                            draft_wrap.as_ref()
                        } else {
                            active_wrap
                        };
                        self.folding.prepare(
                            &display,
                            wrap,
                            self.composition.is_none() && !self.diff_active(),
                        );
                        self.folding.animate(
                            &display,
                            wrap,
                            self.composition.is_none() && !self.diff_active(),
                            now,
                            motion.enabled,
                            ui.ctx(),
                        );
                        let wrapped = wrap.is_some();
                        let geometry = layout::Geometry {
                            text: &display,
                            metrics: &metrics,
                            wrap,
                            folds: &self.folding.display_map,
                            diff: self
                                .diff
                                .current(snapshot.revision, self.active_wrap(&snapshot, wrap_width))
                                .filter(|_| self.composition.is_none()),
                        };
                        // Over a comparison's old lines (or its buttons) there's no
                        // document text to ask about.
                        let on_document = |pointer: egui::Pos2| {
                            geometry.diff.is_none_or(|projection| {
                                let y = (pointer - origin.to_vec2()).y - 6.0;
                                let row = geometry.row_at(y / metrics.height);
                                matches!(projection.row(row), diff::Row::Document(_))
                            })
                        };
                        if self.language_tools
                            && self.composition.is_none()
                            && response.hovered()
                            && let Some(pointer) = ui.input(|i| i.pointer.hover_pos())
                            && on_document(pointer)
                        {
                            let local = pointer - origin.to_vec2();
                            let mut byte = geometry.hit(ui, local);
                            if geometry.caret(byte).left() > local.x && byte > 0 {
                                byte = display.previous_grapheme(byte);
                            }
                            if byte < display.len()
                                && byte < display.line_range(display.line_of(byte)).end
                                && local.x >= 8.0
                                && (geometry.caret(byte).top()..geometry.caret(byte).bottom())
                                    .contains(&local.y)
                            {
                                let end = display.next_grapheme(byte);
                                let start_rect = geometry.caret(byte).translate(origin.to_vec2());
                                let end_rect = geometry.caret(end).translate(origin.to_vec2());
                                hovered_text = Some(SymbolHit {
                                    byte,
                                    range: byte..end,
                                    rect: start_rect.union(end_rect),
                                });
                            }
                            let range = word_range(&display, byte);
                            if byte < display.len()
                                && class(&display, byte) == 1
                                && !range.is_empty()
                                && local.x >= 8.0
                                && (geometry.caret(byte).top()..geometry.caret(byte).bottom())
                                    .contains(&local.y)
                            {
                                let start = geometry.caret(range.start).translate(origin.to_vec2());
                                let end = geometry.caret(range.end).translate(origin.to_vec2());
                                let rect = if start.top() == end.top() {
                                    start.union(end).expand2(egui::vec2(1.0, 0.0))
                                } else {
                                    egui::Rect::from_min_size(
                                        pointer,
                                        egui::vec2(metrics.cell, metrics.height),
                                    )
                                };
                                hovered_symbol = Some(SymbolHit { byte, range, rect });
                                if ui.input(|i| i.modifiers.command) {
                                    ui.set_cursor_icon(egui::CursorIcon::PointingHand);
                                    if response.clicked() {
                                        definition_clicked = Some(byte);
                                    }
                                }
                            }
                        }
                        // The horizontal parent layout advances the cursor past
                        // the canvas. Extend from its origin after edits instead
                        // of reserving another canvas at that cursor.
                        ui.expand_to_include_rect(egui::Rect::from_min_size(
                            origin,
                            geometry.size(viewport.size()),
                        ));
                        let clip = ui.clip_rect();
                        let painter = ui.painter().with_clip_rect(clip);
                        let first = geometry.row_at((viewport.top() - 6.0) / metrics.height);
                        let last = (geometry.row_at((viewport.bottom() + 6.0) / metrics.height)
                            + 2)
                        .min(geometry.rows());
                        let first_cell =
                            ((viewport.left() - 8.0) / metrics.cell).floor().max(0.0) as usize;
                        let last_cell =
                            ((viewport.right() + 8.0) / metrics.cell).ceil().max(1.0) as usize + 4;
                        let mut local_caret = geometry.caret_for(selection.head, self.upstream);
                        let caret_row = geometry.visual_row_for(selection.head, self.upstream);
                        // The other carets (not while composing, which shifts
                        // the text they'd be drawn in).
                        let others: Vec<TextSelection> = if self.composition.is_none() {
                            self.others
                                .list
                                .iter()
                                .map(|other| other.selection)
                                .collect()
                        } else {
                            Vec::new()
                        };
                        let mut selected_ranges: Vec<Range<usize>> = others
                            .iter()
                            .map(|other| other.range())
                            .chain([selection.range()])
                            .collect();
                        selected_ranges.sort_by_key(|range| (range.start, range.end));
                        let mut other_carets: Vec<(usize, usize)> = others
                            .iter()
                            .map(|other| (geometry.visual_row_for(other.head, false), other.head))
                            .collect();
                        other_carets.sort_unstable();
                        let mut other_caret_rects = Vec::new();
                        let bracket_rows: Vec<(usize, usize)> = bracket_pair
                            .into_iter()
                            .flat_map(|(a, b)| [a, b])
                            .map(|byte| (geometry.visual_row_for(byte, false), byte))
                            .collect();
                        for row in first..last {
                            if let Some(projection) = geometry.diff {
                                let y = origin.y + 6.0 + geometry.row_y(row) * metrics.height;
                                match projection.row(row) {
                                    diff::Row::Control(chunk) => {
                                        let rect = egui::Rect::from_min_size(
                                            egui::pos2(origin.x + 8.0, y),
                                            egui::vec2(viewport.width().max(160.0), metrics.height),
                                        );
                                        let mut controls = ui.new_child(
                                            egui::UiBuilder::new()
                                                .id_salt((id, "diff", chunk))
                                                .max_rect(rect)
                                                .layout(egui::Layout::left_to_right(
                                                    egui::Align::Center,
                                                )),
                                        );
                                        controls.spacing_mut().interact_size.y = metrics.height;
                                        for action in &self.diff.actions {
                                            let response = controls.add_enabled(
                                                action.enabled,
                                                egui::Button::new(&action.label).small(),
                                            );
                                            diff_controls.push((action.id, chunk, response.rect));
                                            if response.clicked() {
                                                diff_request = Some(diff::DiffRequest {
                                                    action: action.id,
                                                    comparison: projection.comparison,
                                                    revision: projection.revision,
                                                    range: projection.ranges[chunk].clone(),
                                                });
                                            }
                                        }
                                        continue;
                                    }
                                    diff::Row::Historical(old_row) => {
                                        let segment = projection.historical_segment(old_row);
                                        let line = if projection.width.is_some() {
                                            layout::VisibleLine::fragment(
                                                ui,
                                                &projection.baseline,
                                                &metrics,
                                                segment.bytes,
                                                (segment.indent as f32
                                                    - segment.columns.start as f32)
                                                    * metrics.cell,
                                            )
                                        } else {
                                            layout::VisibleLine::new(
                                                ui,
                                                &projection.baseline,
                                                &metrics,
                                                segment.line,
                                                first_cell..last_cell,
                                            )
                                        };
                                        painter.rect_filled(
                                            egui::Rect::from_min_max(
                                                egui::pos2(clip.left(), y),
                                                egui::pos2(clip.right(), y + metrics.height),
                                            ),
                                            0.0,
                                            ui.visuals().error_fg_color.gamma_multiply(0.12),
                                        );
                                        painter.galley(
                                            egui::pos2(origin.x + 8.0 + line.left(), y),
                                            line.galley.clone(),
                                            ui.visuals().text_color(),
                                        );
                                        if segment.part == 0 {
                                            historical_numbers.push((segment.line, y));
                                        }
                                        rendered_lines += 1;
                                        laid_out_bytes += line.source_bytes;
                                        continue;
                                    }
                                    diff::Row::Document(_) => {}
                                }
                            }
                            let segment = geometry.segment(row);
                            let mut line = geometry.line(ui, row, first_cell..last_cell);
                            if let (Some(observation), Some(changes)) =
                                (&self.syntax_highlights, &highlight_changes)
                            {
                                line.apply_syntax(
                                    ui,
                                    &metrics,
                                    observation,
                                    changes,
                                    &self.syntax_palette,
                                );
                            }
                            laid_out_bytes += line.source_bytes;
                            rendered_lines += 1;
                            let y = origin.y + 6.0 + geometry.row_y(row) * metrics.height;
                            if segment.part == 0 {
                                number_rows.push((
                                    segment.line,
                                    y,
                                    geometry.row_height(row) * metrics.height,
                                ));
                            }
                            if let Some(projection) = geometry.diff {
                                if projection.old_line(segment.line).is_none() {
                                    painter.rect_filled(
                                        egui::Rect::from_min_max(
                                            egui::pos2(clip.left(), y),
                                            egui::pos2(clip.right(), y + metrics.height),
                                        ),
                                        0.0,
                                        egui::Color32::from_rgb(70, 180, 90).gamma_multiply(0.12),
                                    );
                                }
                            }
                            let row_height = geometry.row_height(row) * metrics.height;
                            if let Some(peek) = &peek
                                && segment.bytes.start < peek.end
                                && segment.bytes.end >= peek.start
                            {
                                painter.rect_filled(
                                    egui::Rect::from_min_max(
                                        egui::pos2(clip.left(), y),
                                        egui::pos2(clip.right(), y + row_height),
                                    ),
                                    0.0,
                                    ui.visuals().selection.bg_fill.gamma_multiply(0.22),
                                );
                            }
                            let painter =
                                painter.with_clip_rect(clip.intersect(egui::Rect::from_min_max(
                                    egui::pos2(clip.left(), y),
                                    egui::pos2(clip.right(), y + row_height),
                                )));
                            let body = segment.bytes.clone();
                            let row_end = if wrapped {
                                body.end
                            } else {
                                display.line_start(segment.line + 1)
                            };
                            if let Some((needle, whole_word)) = &occurrence
                                && body.len() <= MAX_OCCURRENCE_LINE
                            {
                                let row_text = display.slice(body.clone()).to_string();
                                for (offset, _) in row_text.match_indices(needle.as_str()) {
                                    let after = offset + needle.len();
                                    let word = |c: Option<char>| {
                                        c.is_some_and(|c| c.is_alphanumeric() || c == '_')
                                    };
                                    if *whole_word
                                        && (word(row_text[..offset].chars().next_back())
                                            || word(row_text[after..].chars().next()))
                                    {
                                        continue;
                                    }
                                    painter.rect_filled(
                                        egui::Rect::from_min_max(
                                            egui::pos2(
                                                origin.x + 8.0 + line.x_at(body.start + offset),
                                                y,
                                            ),
                                            egui::pos2(
                                                origin.x + 8.0 + line.x_at(body.start + after),
                                                y + metrics.height,
                                            ),
                                        ),
                                        2.0,
                                        ui.visuals().selection.bg_fill.gamma_multiply(0.35),
                                    );
                                }
                            }
                            // Selections are in order and apart: those on this row.
                            let from = selected_ranges.partition_point(|r| r.end <= body.start);
                            for selected in selected_ranges[from..]
                                .iter()
                                .take_while(|r| r.start < row_end)
                                .filter(|r| !r.is_empty())
                            {
                                let left = if selected.start <= body.start {
                                    segment.indent as f32 * metrics.cell
                                } else {
                                    line.x_at(selected.start)
                                };
                                let right = if selected.end > body.end
                                    && geometry
                                        .fold_end_below(row)
                                        .is_some_and(|end| selected.end <= end)
                                {
                                    // Ending in what a fold hides: through its " …".
                                    geometry.fold_marker_right(row)
                                } else if selected.end > body.end {
                                    viewport.right().max(line.right())
                                } else {
                                    line.x_at(selected.end)
                                };
                                painter.rect_filled(
                                    egui::Rect::from_min_max(
                                        egui::pos2(origin.x + 8.0 + left, y),
                                        egui::pos2(origin.x + 8.0 + right, y + metrics.height),
                                    ),
                                    0.0,
                                    ui.visuals().selection.bg_fill,
                                );
                            }
                            if self.search.open
                                && self.composition.is_none()
                                && let Some(results) = &self.search.results
                                && results.revision == snapshot.revision
                                && results.query == self.search.query
                            {
                                for range in
                                    search::visible_matches(&display, results, line.start..line.end)
                                {
                                    painter.rect_filled(
                                        egui::Rect::from_min_max(
                                            egui::pos2(
                                                origin.x
                                                    + 8.0
                                                    + line.x_at(range.start.max(body.start)),
                                                y,
                                            ),
                                            egui::pos2(
                                                origin.x + 8.0 + line.x_at(range.end.min(body.end)),
                                                y + metrics.height,
                                            ),
                                        ),
                                        0.0,
                                        ui.visuals().warn_fg_color.gamma_multiply(0.25),
                                    );
                                }
                            }
                            // After the folded line's last part, where what it
                            // hides would follow (and where a caret there is).
                            if segment.bytes.end == display.line_range(segment.line).end
                                && self.folding.collapsed_at(&display, segment.line)
                            {
                                painter.text(
                                    egui::pos2(
                                        origin.x + 8.0 + layout::FOLD_MARKER_GAP + line.right(),
                                        y,
                                    ),
                                    egui::Align2::LEFT_TOP,
                                    layout::FOLD_MARKER,
                                    metrics.font.clone(),
                                    ui.visuals().weak_text_color(),
                                );
                            }
                            if ui.input(|i| i.modifiers.command)
                                && let Some(hit) = &hovered_symbol
                                && hit.range.start < line.end
                                && hit.range.end > line.start
                            {
                                painter.line_segment(
                                    [
                                        egui::pos2(
                                            origin.x
                                                + 8.0
                                                + line.x_at(hit.range.start.max(line.start)),
                                            y + metrics.height - 1.0,
                                        ),
                                        egui::pos2(
                                            origin.x + 8.0 + line.x_at(hit.range.end.min(line.end)),
                                            y + metrics.height - 1.0,
                                        ),
                                    ],
                                    egui::Stroke::new(1.0, ui.visuals().hyperlink_color),
                                );
                            }
                            painter.galley(
                                egui::pos2(origin.x + 8.0 + line.left(), y),
                                line.galley.clone(),
                                ui.visuals().text_color(),
                            );
                            if self.composition.is_none()
                                && let Some(report) = self
                                    .diagnostics
                                    .as_ref()
                                    .filter(|d| d.revision == snapshot.revision)
                            {
                                // Their messages show in the language hover
                                // (`lsp::assist`), not a tooltip of their own.
                                for diagnostic in
                                    report.intersecting(line.start..line.end).take(256)
                                {
                                    let left = line.x_at(diagnostic.range.start.max(line.start));
                                    let right = line
                                        .x_at(diagnostic.range.end.min(line.end))
                                        .max(left + metrics.cell);
                                    let color = match diagnostic.severity {
                                        DiagnosticSeverity::Error => ui.visuals().error_fg_color,
                                        DiagnosticSeverity::Warning => ui.visuals().warn_fg_color,
                                        _ => ui.visuals().weak_text_color(),
                                    }
                                    .gamma_multiply(if report.verified { 1.0 } else { 0.6 });
                                    painter.line_segment(
                                        [
                                            egui::pos2(
                                                origin.x + 8.0 + left,
                                                y + metrics.height - 1.0,
                                            ),
                                            egui::pos2(
                                                origin.x + 8.0 + right,
                                                y + metrics.height - 1.0,
                                            ),
                                        ],
                                        egui::Stroke::new(1.0, color),
                                    );
                                }
                            }
                            // Colors written in the line: underlined in their color.
                            if self.composition.is_none() {
                                let line_start = display.line_start(segment.line);
                                let line_end =
                                    display.line_start(segment.line + 1).min(display.len());
                                let scan = if line_end - line_start > colors::LONGEST_LINE {
                                    display.clamp_boundary(
                                        line.start.saturating_sub(96).max(line_start),
                                    )
                                        ..display.clamp_boundary((line.end + 96).min(line_end))
                                } else {
                                    line_start..line_end
                                };
                                for mut literal in colors::find(&display.range_text(scan.clone())) {
                                    literal.range = scan.start + literal.range.start
                                        ..scan.start + literal.range.end;
                                    if literal.range.end <= line.start
                                        || literal.range.start >= line.end
                                    {
                                        continue;
                                    }
                                    let left = line.x_at(literal.range.start.max(line.start));
                                    let right = line.x_at(literal.range.end.min(line.end));
                                    let underline = y + metrics.height - 1.0;
                                    painter.line_segment(
                                        [
                                            egui::pos2(origin.x + 8.0 + left, underline),
                                            egui::pos2(origin.x + 8.0 + right, underline),
                                        ],
                                        egui::Stroke::new(
                                            2.0,
                                            colors::to_color32(literal.color).to_opaque(),
                                        ),
                                    );
                                    let rect = egui::Rect::from_min_max(
                                        egui::pos2(origin.x + 8.0 + left, y),
                                        egui::pos2(origin.x + 8.0 + right, y + metrics.height),
                                    )
                                    .intersect(painter.clip_rect());
                                    if rect.is_positive() {
                                        color_hits.push(colors::Hit {
                                            range: literal.range.clone(),
                                            literal,
                                            rect,
                                        });
                                    }
                                }
                            }
                            for &(_, byte) in bracket_rows.iter().filter(|&&(r, _)| r == row) {
                                painter.rect_stroke(
                                    egui::Rect::from_min_max(
                                        egui::pos2(origin.x + 8.0 + line.x_at(byte), y),
                                        egui::pos2(
                                            origin.x + 8.0 + line.x_at(byte + 1),
                                            y + metrics.height,
                                        ),
                                    ),
                                    2.0,
                                    egui::Stroke::new(1.0, ui.visuals().weak_text_color()),
                                    egui::StrokeKind::Inside,
                                );
                            }
                            let from = other_carets.partition_point(|&(r, _)| r < row);
                            for &(_, head) in
                                other_carets[from..].iter().take_while(|&&(r, _)| r == row)
                            {
                                other_caret_rects.push(egui::Rect::from_min_size(
                                    egui::pos2(
                                        8.0 + line.x_at(head),
                                        6.0 + geometry.row_y(row) * metrics.height,
                                    ),
                                    egui::vec2(0.0, metrics.height),
                                ));
                            }
                            if caret_row == row {
                                local_caret = egui::Rect::from_min_size(
                                    egui::pos2(
                                        8.0 + line.x_at(selection.head),
                                        6.0 + geometry.row_y(row) * metrics.height,
                                    ),
                                    egui::vec2(0.0, metrics.height),
                                );
                            }
                            if let Some(composition) = &self.composition {
                                let range = composition.selection.range().start..selection.head;
                                if range.start <= body.end && range.end >= body.start {
                                    painter.line_segment(
                                        [
                                            egui::pos2(
                                                origin.x
                                                    + 8.0
                                                    + line.x_at(range.start.max(body.start)),
                                                y + metrics.height - 1.0,
                                            ),
                                            egui::pos2(
                                                origin.x + 8.0 + line.x_at(range.end.min(body.end)),
                                                y + metrics.height - 1.0,
                                            ),
                                        ],
                                        egui::Stroke::new(1.0, ui.visuals().text_color()),
                                    );
                                }
                            }
                        }
                        caret_rect = local_caret.translate(origin.to_vec2());
                        // Screen readers and scripts see the visible rows.
                        response.widget_info(|| {
                            let visible: Vec<String> = (first..last)
                                .map(|row| {
                                    // An unwrapped row is its whole line, which
                                    // may be megabytes: its start is enough.
                                    let bytes = geometry.segment(row).bytes;
                                    let end = display.clamp_boundary(
                                        bytes.end.min(bytes.start + MAX_DESCRIBED_ROW),
                                    );
                                    let text = display.range_text(bytes.start..end);
                                    text.trim_end_matches(['\n', '\r']).to_owned()
                                })
                                .collect();
                            egui::WidgetInfo::text_edit(ui.is_enabled(), "", visible.join("\n"), "")
                        });
                        if wrapped != self.wrap_active && reveal.is_none() {
                            reveal = self
                                .wrap_reveal
                                .take()
                                .or(Some((selection.head..selection.head, Scroll::Minimal)));
                        }
                        if wrap_width.is_some() && !wrapped {
                            self.wrap_reveal = reveal.clone().or_else(|| self.wrap_reveal.take());
                        }
                        // Geometry borrows wrap metadata, so update the flag only
                        // after all mappings below have been read.
                        if let Some((range, alignment)) = &reveal {
                            let target = if self.composition.is_some() {
                                local_caret
                            } else {
                                geometry.caret(range.start).union(geometry.caret(range.end))
                            };
                            ui.scroll_to_rect(
                                target.translate(origin.to_vec2()).expand(2.0),
                                if *alignment == Scroll::Center {
                                    Some(egui::Align::Center)
                                } else {
                                    None
                                },
                            );
                        }
                        self.wrap_active = wrapped;
                        if focused && !self.settings.read_only {
                            let transform = ui
                                .ctx()
                                .layer_transform_to_global(ui.layer_id())
                                .unwrap_or_default();
                            ui.output_mut(|output| {
                                output.ime = Some(egui::output::IMEOutput {
                                    purpose: egui::IMEPurpose::Normal,
                                    rect: transform * clip,
                                    cursor_rect: transform * caret_rect,
                                    should_interrupt_composition: false,
                                })
                            });
                        }
                        if focused
                            && !self.settings.read_only
                            && ui.input(|i| i.focused)
                            && self.caret.blink_on(ui.input(|i| i.time))
                        {
                            let painter =
                                ui.painter().with_clip_rect(clip.intersect(ui.clip_rect()));
                            let ink = bedtime_ui::rainbow_ink(ui, motion);
                            for rect in &other_caret_rects {
                                painter.rect_filled(
                                    egui::Rect::from_center_size(
                                        rect.center() + origin.to_vec2(),
                                        egui::vec2(2.0, rect.height()),
                                    ),
                                    0.0,
                                    ink,
                                );
                            }
                        }
                        self.caret.paint(
                            ui,
                            clip,
                            origin,
                            focused,
                            selection.head,
                            local_caret,
                            motion.enabled && self.settings.animate_caret,
                            bedtime_ui::rainbow_ink(ui, motion),
                            !self.settings.read_only,
                        );
                        response
                    });
                (gutter, scroll)
            })
            .inner;
        if self.request_focus {
            // Request only after the canvas has registered its current rect.
            // Removing the find bar changes the scroll layout in this pass.
            scroll.inner.request_focus();
            self.request_focus = false;
        }
        self.scroll = scroll.state.offset;
        self.observed_revision = Some(snapshot.revision);
        if self.composition.is_none()
            && let Some(report) = self
                .diagnostics
                .clone()
                .filter(|d| d.revision == snapshot.revision)
        {
            let mut overview = std::mem::take(&mut self.overview);
            let wrap = self.active_wrap(&snapshot, self.settings.wrap.then_some(1));
            let geometry = layout::Geometry {
                text: &snapshot.text,
                metrics: &metrics,
                wrap,
                folds: &self.folding.display_map,
                diff: self.diff.current(snapshot.revision, wrap),
            };
            overview.paint(
                ui,
                scroll.inner_rect,
                &report,
                &geometry,
                &self.folding.collapsed,
                scroll.content_size.y,
            );
            self.overview = overview;
        }
        if self.settings.line_numbers {
            if let Some(projection) = self.diff.current(
                snapshot.revision,
                self.active_wrap(&snapshot, self.settings.wrap.then_some(1)),
            ) {
                let painter = ui
                    .painter()
                    .with_clip_rect(gutter.intersect(ui.clip_rect()));
                let x = gutter.left() + gutter.width() / 2.0 - 6.0;
                for (row, y) in historical_numbers {
                    painter.text(
                        egui::pos2(x, y),
                        egui::Align2::RIGHT_TOP,
                        (row + 1).to_string(),
                        metrics.font.clone(),
                        ui.visuals().error_fg_color,
                    );
                }
                for &(row, y, _) in &number_rows {
                    if let Some(old) = projection.old_line(row) {
                        painter.text(
                            egui::pos2(x, y),
                            egui::Align2::RIGHT_TOP,
                            (old + 1).to_string(),
                            metrics.font.clone(),
                            ui.visuals().weak_text_color(),
                        );
                    }
                }
            }
        }
        if self.settings.line_numbers || fold_gutter {
            let painter = ui
                .painter()
                .with_clip_rect(gutter.intersect(ui.clip_rect()));
            for (row, y, height) in number_rows {
                let painter = painter.with_clip_rect(gutter.intersect(egui::Rect::from_min_max(
                    egui::pos2(gutter.left(), y),
                    egui::pos2(gutter.right(), y + height),
                )));
                // A disclosure triangle where a fold starts.
                if fold_gutter
                    && (self.folding.collapsed_at(&snapshot.text, row)
                        || self.syntax.as_ref().is_some_and(|o| {
                            o.key.revision == snapshot.revision
                                && o.folds.binary_search_by_key(&row, |f| f.start_line).is_ok()
                        }))
                {
                    let rect = egui::Rect::from_min_size(
                        egui::pos2(gutter.right() - 14.0, y),
                        egui::vec2(14.0, metrics.height),
                    );
                    bedtime_ui::paint_chevron(
                        &painter,
                        rect.center(),
                        self.folding.openness(&snapshot.text, row, now),
                        ui.visuals().weak_text_color(),
                    );
                    let toggle = ui.interact(
                        rect.intersect(painter.clip_rect()),
                        id.with(("fold", row)),
                        egui::Sense::click(),
                    );
                    let collapsed = self.folding.collapsed_at(&snapshot.text, row);
                    toggle.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::WidgetType::Button,
                            true,
                            format!(
                                "{} line {}",
                                if collapsed { "Unfold" } else { "Fold" },
                                row + 1
                            ),
                        )
                    });
                    if toggle.clicked() {
                        if let Err(error) = self.toggle_fold(&snapshot, row) {
                            failure = Some(error);
                        }
                        // The arrow isn't somewhere to type: the text keeps focus.
                        ui.memory_mut(|memory| memory.request_focus(id));
                        ui.ctx().request_repaint();
                    }
                }
                if self.settings.line_numbers {
                    painter.text(
                        egui::pos2(
                            gutter.right() - 3.0 - if fold_gutter { 14.0 } else { 7.0 },
                            y,
                        ),
                        egui::Align2::RIGHT_TOP,
                        (row + 1).to_string(),
                        metrics.font.clone(),
                        if row
                            == self
                                .folding
                                .visible_line(&snapshot.text, self.selection.head)
                        {
                            bedtime_ui::rainbow_ink(ui, motion)
                        } else {
                            ui.visuals().weak_text_color()
                        },
                    );
                }
            }
        }
        if let Some(error) = failure {
            return Err(error);
        }
        if self.color_picker(ui, id, session, &mut snapshot, &color_hits, now)? {
            // The picker is what's hovered, not a symbol to ask the server about.
            hovered_symbol = None;
            hovered_text = None;
        }
        self.completion_popup(
            ui,
            id,
            caret_rect,
            snapshot.revision,
            scroll.inner.has_focus(),
        );
        Ok(EditorOutput {
            diff_request,
            diff_controls,
            hovered_symbol,
            hovered_text,
            definition_clicked,
            completion_requested: self.assistance.requested,
            completion_refresh: self.assistance.refresh,
            completion_accepted: self.assistance.accepted,
            completion_rect: self.assistance.popup_rect,
            completion_dismissed: self.assistance.dismissed,
            typed: self.assistance.typed.take(),
            response: scroll.inner,
            text_rect: scroll.inner_rect,
            caret_rect,
            selection: self.selection,
            revision: snapshot.revision,
            composing: self.is_composing(),
            scroll: self.scroll,
            rendered_lines,
            laid_out_bytes,
        })
    }

    /// Open, draw and close the color picker; a new color replaces the text,
    /// written as it was. Returns whether the pointer is on a color or the
    /// picker.
    fn color_picker(
        &mut self,
        ui: &egui::Ui,
        id: egui::Id,
        session: &mut dyn TextSession,
        snapshot: &mut TextSnapshot,
        hits: &[colors::Hit],
        now: f64,
    ) -> Result<bool, EditorError> {
        let (pointer, down) = ui.input(|i| (i.pointer.hover_pos(), i.pointer.primary_down()));
        // An edit from elsewhere (typing, another view) ends it.
        let keep = self.color_picker.open.as_ref().is_some_and(|open| {
            open.revision == snapshot.revision
                && (open.dragging || pointer.is_some_and(|p| self.color_picker.holds(p)))
        });
        if !keep
            && let Some(open) = self.color_picker.open.take()
            && open.edited
        {
            session.end_undo_group();
        }
        if self.color_picker.open.is_none()
            && !self.settings.read_only
            && self.composition.is_none()
            && !down
            && let Some(hit) = self.color_picker.rest(ui.ctx(), hits, pointer, now)
        {
            self.color_picker.open = Some(colors::Open {
                range: hit.range.clone(),
                revision: snapshot.revision,
                form: hit.literal.form.clone(),
                color: colors::to_hsva(hit.literal.color),
                anchor: hit.rect,
                popup: None,
                dragging: false,
                edited: false,
            });
        }
        let on_color = pointer.is_some_and(|p| {
            hits.iter().any(|hit| hit.rect.contains(p)) || self.color_picker.holds(p)
        });
        let Some(open) = self.color_picker.open.as_mut() else {
            return Ok(on_color);
        };
        let alpha = if open.form.has_alpha() {
            egui::color_picker::Alpha::OnlyBlend
        } else {
            egui::color_picker::Alpha::Opaque
        };
        let mut color = open.color;
        let mut changed = false;
        // The editor's style: its panel's zoom, as the completions are.
        let style = ui.style().clone();
        let shown = egui::Area::new(id.with("color-picker"))
            .order(egui::Order::Foreground)
            .fixed_pos(open.anchor.left_bottom() + egui::vec2(0.0, 2.0))
            .constrain_to(ui.ctx().content_rect().shrink(8.0))
            .show(ui.ctx(), |ui| {
                ui.set_style(style);
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    // A square big enough to pick from, not much past the numbers.
                    ui.spacing_mut().slider_width =
                        ui.text_style_height(&egui::TextStyle::Body) * 8.0;
                    changed = egui::color_picker::color_picker_hsva_2d(ui, &mut color, alpha);
                });
            });
        open.popup = Some(shown.response.rect);
        open.dragging = down && (open.dragging || shown.response.contains_pointer());
        if !changed {
            return Ok(true);
        }
        open.color = color;
        let replacement = colors::write(colors::from_hsva(color), &open.form);
        let range = open.range.clone();
        if snapshot.text.range_text(range.clone()) == replacement {
            return Ok(true);
        }
        if !open.edited {
            session.end_undo_group();
            open.edited = true;
        }
        let end = range.start + replacement.len();
        // The selection keeps its place around the color.
        let map = |byte: usize| {
            if byte >= range.end {
                byte - range.len() + replacement.len()
            } else {
                byte.min(range.start + replacement.len())
            }
        };
        let selection = self.selection;
        let mapped = TextSelection {
            anchor: map(selection.anchor),
            head: map(selection.head),
        };
        self.apply_edit(
            id,
            session,
            snapshot,
            TextEdit {
                range: range.clone(),
                replacement,
            },
            mapped,
            now,
        )?;
        if let Some(open) = self.color_picker.open.as_mut() {
            open.range = range.start..end;
            open.revision = snapshot.revision;
        }
        // This view's own edit: the next frame mustn't take it for another's
        // (which would end the undo group, splitting the drag into steps).
        self.observed_revision = Some(snapshot.revision);
        Ok(true)
    }

    /// Keys the editor keeps while focused, instead of egui moving focus.
    fn lock_focus(&self, ui: &egui::Ui, id: egui::Id) {
        ui.memory_mut(|memory| {
            memory.set_focus_lock_filter(
                id,
                egui::EventFilter {
                    tab: true,
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    escape: self.language_tools
                        || self.has_others()
                        || self.search.open
                        || self.is_composing()
                        || self.assistance.menu.is_some()
                        || self.assistance.snippet.is_some(),
                },
            )
        });
    }
    fn handle_event(
        &mut self,
        ui: &egui::Ui,
        id: egui::Id,
        session: &mut dyn TextSession,
        snapshot: &mut TextSnapshot,
        event: &egui::Event,
        now: f64,
        page: usize,
    ) -> Result<bool, EditorError> {
        if self.assistance_event(id, session, snapshot, event)? {
            return Ok(true);
        }
        self.reconcile_folds(session, snapshot)?;
        let width = self
            .wrapping
            .index
            .as_ref()
            .filter(|_| self.settings.wrap)
            .map(|w| w.width);
        self.prepare_folds(snapshot, width);
        self.sync_others(session, snapshot)?;
        match event {
            egui::Event::Ime(egui::ImeEvent::Preedit { text, .. }) if !self.settings.read_only => {
                if text.is_empty() {
                    self.cancel_composition(session, id);
                } else {
                    let selection = self
                        .composition
                        .as_ref()
                        .map_or(self.selection, |c| c.selection);
                    self.composition = Some(Composition {
                        revision: snapshot.revision,
                        selection,
                        text: text.clone(),
                    });
                    session.set_composing(id, true);
                }
                return Ok(true);
            }
            egui::Event::Ime(egui::ImeEvent::Commit(text)) if !self.settings.read_only => {
                if let Some(composition) = self.composition.take() {
                    self.selection = composition.selection;
                } else if text.is_empty() {
                    return Ok(true);
                }
                session.set_composing(id, false);
                session.end_undo_group();
                if self.has_others() {
                    self.edit_carets(id, session, snapshot, now, |_, selection| {
                        let head = selection.range().start + text.len();
                        Some((
                            BufferEdit::new(selection.range(), text.as_str()),
                            TextSelection { anchor: head, head },
                        ))
                    })?;
                } else {
                    self.replace_selection(id, session, snapshot, text.clone(), now)?;
                }
                session.end_undo_group();
                return Ok(true);
            }
            egui::Event::Key {
                key: egui::Key::Escape,
                pressed: true,
                ..
            } if self.is_composing() => {
                self.cancel_composition(session, id);
                ui.memory_mut(|memory| memory.interrupt_ime());
                return Ok(true);
            }
            _ => {}
        }
        if self.is_composing() {
            return Ok(matches!(
                event,
                egui::Event::Key { .. }
                    | egui::Event::Text(_)
                    | egui::Event::Paste(_)
                    | egui::Event::Cut
            ));
        }
        if let Some(handled) = self.line_clipboard_event(ui, id, session, snapshot, event, now)? {
            return Ok(handled);
        }
        if let Some(handled) = self.pair_event(id, session, snapshot, event, now)? {
            return Ok(handled);
        }
        if self.has_others()
            && let Some(handled) = self.multi_caret_event(ui, id, session, snapshot, event, now)?
        {
            return Ok(handled);
        }
        match event {
            egui::Event::Text(text) if matches!(text.as_str(), "\n" | "\r" | "\t") => Ok(true),
            egui::Event::Text(text) | egui::Event::Paste(text) => {
                if !self.settings.read_only {
                    let paste = matches!(event, egui::Event::Paste(_));
                    if paste {
                        session.end_undo_group();
                    }
                    if !paste
                        && self.settings.smart_indent
                        && self.selection.range().is_empty()
                        && matches!(text.as_str(), "}" | "]" | ")")
                        && self.selection.head
                            > snapshot
                                .text
                                .line_start(snapshot.text.line_of(self.selection.head))
                        && whitespace::leading_end(
                            &snapshot.text,
                            snapshot.text.line_of(self.selection.head),
                            self.selection.head,
                        ) == self.selection.head
                    {
                        let target = whitespace::backspace(
                            &snapshot.text,
                            self.selection.head,
                            self.settings.indentation,
                        );
                        let head = target + text.len();
                        session.end_undo_group();
                        self.apply_edit(
                            id,
                            session,
                            snapshot,
                            TextEdit {
                                range: target..self.selection.head,
                                replacement: text.clone(),
                            },
                            TextSelection { anchor: head, head },
                            now,
                        )?;
                        session.end_undo_group();
                    } else {
                        self.replace_selection(id, session, snapshot, text.clone(), now)?;
                    }
                    if paste {
                        session.end_undo_group();
                    }
                }
                Ok(true)
            }
            egui::Event::Copy | egui::Event::Cut => {
                if !self.selection.range().is_empty() {
                    ui.ctx()
                        .copy_text(snapshot.text.slice(self.selection.range()).to_string());
                    if matches!(event, egui::Event::Cut) && !self.settings.read_only {
                        session.end_undo_group();
                        self.replace_selection(id, session, snapshot, String::new(), now)?;
                        session.end_undo_group();
                    }
                }
                Ok(true)
            }
            egui::Event::Key {
                key,
                modifiers,
                pressed: true,
                ..
            } => {
                let primary = modifiers.command;
                let mac = ui.ctx().os() == egui::os::OperatingSystem::Mac;
                let word = modifiers.alt || (!mac && modifiers.ctrl);
                if primary && !modifiers.alt {
                    match key {
                        egui::Key::A => {
                            self.clear_others();
                            self.selection = TextSelection {
                                anchor: 0,
                                head: snapshot.text.len(),
                            };
                            session.end_undo_group();
                            return Ok(true);
                        }
                        // The UI runtime adds a Copy, Cut or Paste event
                        // for these chords; the key itself does nothing.
                        egui::Key::C | egui::Key::X | egui::Key::V => return Ok(true),
                        egui::Key::Z | egui::Key::Y => {
                            if !self.settings.read_only {
                                session.end_undo_group();
                                let before = snapshot.revision;
                                session.history(if modifiers.shift || *key == egui::Key::Y {
                                    HistoryAction::Redo
                                } else {
                                    HistoryAction::Undo
                                })?;
                                *snapshot = session.snapshot()?;
                                self.selection =
                                    map_selection(session, before, snapshot, self.selection)?;
                                self.preferred_column = None;
                            }
                            return Ok(true);
                        }
                        _ => {}
                    }
                }
                let head = self.selection.head;
                // The line the caret is drawn on (a fold's first, beside it).
                let row = self.folding.visible_line(&snapshot.text, head);
                let body = snapshot.text.line_range(row);
                let selection = self.selection.range();
                if self.move_carets(ui, snapshot, *key, *modifiers, page) {
                    session.end_undo_group();
                    return Ok(true);
                }
                match key {
                    egui::Key::Backspace | egui::Key::Delete => {
                        if !self.settings.read_only {
                            if selection.is_empty() {
                                let target = if *key == egui::Key::Backspace {
                                    if mac && primary {
                                        body.start
                                    } else if word {
                                        word_boundary(&snapshot.text, head, false)
                                    } else {
                                        if self.settings.smart_indent {
                                            whitespace::backspace(
                                                &snapshot.text,
                                                head,
                                                self.settings.indentation,
                                            )
                                        } else {
                                            snapshot.text.previous_grapheme(head)
                                        }
                                    }
                                } else if mac && primary {
                                    body.end
                                } else if word {
                                    word_boundary(&snapshot.text, head, true)
                                } else {
                                    snapshot.text.next_grapheme(head)
                                };
                                self.selection.anchor = target;
                            }
                            self.replace_selection(id, session, snapshot, String::new(), now)?;
                        }
                        Ok(true)
                    }
                    egui::Key::Enter | egui::Key::Tab if !primary && !modifiers.alt => {
                        if !self.settings.read_only {
                            if *key == egui::Key::Enter {
                                let (edit, head) = whitespace::newline(
                                    &snapshot.text,
                                    self.selection,
                                    self.settings.indentation,
                                    self.settings.smart_indent,
                                );
                                session.end_undo_group();
                                self.apply_buffer_edit(
                                    id,
                                    session,
                                    snapshot,
                                    edit,
                                    TextSelection { anchor: head, head },
                                    now,
                                )?;
                                session.end_undo_group();
                            } else if modifiers.shift || !self.selection.range().is_empty() {
                                let range = self.selection.range();
                                let first =
                                    snapshot.text.line_start(snapshot.text.line_of(range.start));
                                let last = snapshot
                                    .text
                                    .line_start(snapshot.text.line_of(range.end) + 1);
                                if last - first > 256 * 1024 {
                                    let source = snapshot.clone();
                                    let selection = self.selection;
                                    let style = self.settings.indentation;
                                    let outdent = modifiers.shift;
                                    self.indent_job = Some((
                                        source.revision,
                                        selection,
                                        background::Job::start(ui.ctx(), move |cancel| {
                                            whitespace::indent_cancellable(
                                                &source.text,
                                                selection,
                                                style,
                                                outdent,
                                                cancel,
                                            )
                                        }),
                                    ));
                                } else {
                                    let (edit, selection) = whitespace::indent(
                                        &snapshot.text,
                                        self.selection,
                                        self.settings.indentation,
                                        modifiers.shift,
                                    );
                                    session.end_undo_group();
                                    self.apply_buffer_edit(
                                        id, session, snapshot, edit, selection, now,
                                    )?;
                                    session.end_undo_group();
                                }
                            } else {
                                let text = if self.settings.indentation == Indentation::Tabs {
                                    "\t".into()
                                } else {
                                    " ".repeat(
                                        self.settings.indentation.width()
                                            - snapshot.text.column(head)
                                                % self.settings.indentation.width(),
                                    )
                                };
                                self.replace_selection(id, session, snapshot, text, now)?;
                            }
                        }
                        Ok(true)
                    }
                    _ => Ok(false),
                }
            }
            _ => Ok(false),
        }
    }
}
/// Occurrences aren't looked for in longer lines (minified code).
const MAX_OCCURRENCE_LINE: usize = 4096;
/// The bytes of a visible row given to screen readers and scripts.
const MAX_DESCRIBED_ROW: usize = 4096;

impl EditorView {
    /// What to highlight elsewhere: nothing for a bare caret; a selected
    /// word (whole words), or another short selection within a line
    /// (exactly).
    fn occurrence(&self, snapshot: &TextSnapshot) -> Option<(String, bool)> {
        let text = &snapshot.text;
        let range = self.selection.range();
        if range.is_empty()
            || range.len() > 128
            || text.line_of(range.start) != text.line_of(range.end)
        {
            return None;
        }
        let selected = text.slice(range.clone()).to_string();
        if selected.trim().is_empty() {
            return None;
        }
        let word = class(text, range.start) == 1 && word_at(text, range.start) == range;
        Some((selected, word))
    }
}

fn map_selection(
    session: &dyn TextSession,
    before: u64,
    after: &TextSnapshot,
    selection: TextSelection,
) -> Result<TextSelection, EditorError> {
    Ok(map_selections(session, before, after, &[selection])?[0])
}
fn map_selections(
    session: &dyn TextSession,
    before: u64,
    after: &TextSnapshot,
    selections: &[TextSelection],
) -> Result<Vec<TextSelection>, EditorError> {
    if before == after.revision {
        return Ok(selections.to_vec());
    }
    if let Some(commits) = session.changes_since(before)?
        && commits.iter().all(|commit| commit.changes.is_some())
    {
        let changes: Vec<_> = commits
            .iter()
            .flat_map(|commit| commit.changes.as_ref().unwrap())
            .collect();
        return Ok(selections
            .iter()
            .map(|&selection| {
                changes
                    .iter()
                    .fold(selection, |selection, change| selection.mapped(change))
            })
            .collect());
    }
    // A replacement or journal gap invalidates position knowledge. Clamp to
    // valid positions rather than diffing a potentially enormous document on UI.
    Ok(selections
        .iter()
        .map(|selection| TextSelection {
            anchor: after.text.clamp_boundary(selection.anchor),
            head: after.text.clamp_boundary(selection.head),
        })
        .collect())
}
fn character_class(ch: char) -> u8 {
    if ch.is_whitespace() {
        0
    } else if ch.is_alphanumeric() || ch == '_' {
        1
    } else if unicode_width::UnicodeWidthChar::width(ch) == Some(0) {
        3
    } else {
        2
    }
}
fn class(text: &TextBuffer, byte: usize) -> u8 {
    if byte >= text.len() {
        return 0;
    }
    let (chunk, start) = text.chunk_at(byte);
    character_class(chunk[byte - start..].chars().next().unwrap())
}
/// Scan leaf slices rather than performing a tree lookup for every character.
/// Zero-width marks inherit their surrounding run, preserving grapheme words.
fn run_boundary(text: &TextBuffer, from: usize, category: u8, forward: bool) -> usize {
    let mut byte = from;
    if forward {
        while byte < text.len() {
            let (chunk, start) = text.chunk_at(byte);
            for (offset, ch) in chunk[byte - start..].char_indices() {
                let current = character_class(ch);
                if current != category && current != 3 {
                    return byte + offset;
                }
            }
            byte = start + chunk.len();
        }
    } else {
        while byte > 0 {
            let (chunk, start) = text.chunk_at(byte - 1);
            for (offset, ch) in chunk[..byte - start].char_indices().rev() {
                let current = character_class(ch);
                if current != category && current != 3 {
                    return start + offset + ch.len_utf8();
                }
            }
            byte = start;
        }
    }
    byte
}
fn word_boundary(text: &TextBuffer, from: usize, forward: bool) -> usize {
    if forward {
        let category = class(text, from);
        let edge = run_boundary(text, from, category, true);
        if category != 0 && class(text, edge) == 0 {
            run_boundary(text, edge, 0, true)
        } else {
            edge
        }
    } else {
        let edge = run_boundary(text, from, 0, false);
        if edge == 0 {
            0
        } else {
            run_boundary(text, edge, class(text, text.previous_grapheme(edge)), false)
        }
    }
}
/// The word (or run of other characters) at `byte`.
pub fn word_at(text: &TextBuffer, byte: usize) -> Range<usize> {
    word_range(text, byte)
}
fn word_range(text: &TextBuffer, from: usize) -> Range<usize> {
    let category = class(text, from);
    run_boundary(text, from, category, false)..run_boundary(text, from, category, true)
}
#[cfg(test)]
mod tests;

#[cfg(test)]
mod everyday_tests;

#[cfg(test)]
mod syntax_tests;

#[cfg(test)]
mod carets_tests;

#[cfg(test)]
mod lines_tests;

#[cfg(test)]
mod comments_tests;

#[cfg(test)]
mod pairs_tests;

#[cfg(test)]
mod polish_tests;

#[cfg(test)]
mod keys_tests;
