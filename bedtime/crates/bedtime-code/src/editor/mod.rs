//! Viewport-only text editor for workbench documents. Canonical text and
//! history belong to the workbench document; views own selection, composition
//! and navigation. No full-file string or galley is constructed while drawing
//! or performing ordinary edits.
mod background;
mod caret;
mod completion;
pub use completion::*;
mod folding;
mod search;
mod search_ui;
pub mod syntax;
mod whitespace;
mod wrapping;
pub use search::SearchQuery;
pub use whitespace::Indentation;
mod diagnostics;
mod document;
mod layout;
pub use diagnostics::*;
mod text {
    pub use bedtime_document::text::*;
}
pub use document::*;
pub use text::{BufferEdit, TextBuffer};
mod panel;
use bedtime_ui::{MotionPolicy, egui};
use bevy::prelude::Component;
pub use panel::{CodePlugin, EditorPanel, editor_id, text_fit};
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
pub enum Reveal {
    #[default]
    Minimal,
    Center,
}
#[derive(Clone, Debug)]
pub struct Navigation {
    pub revision: u64,
    pub range: Range<usize>,
    pub selection: Option<TextSelection>,
    pub reveal: Reveal,
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
    /// A language server's diagnostics, drawn only at their revision.
    diagnostics: Option<std::sync::Arc<Diagnostics>>,
    /// Whether the text has a language: reserves the fold column before its
    /// first analysis arrives, so text doesn't shift.
    pub has_syntax: bool,
    /// The current analysis (folds, outline) and the one colours come from,
    /// which may be older while a new one runs.
    syntax: Option<std::sync::Arc<crate::syntax::Observation>>,
    syntax_identity: Option<(u64, u64)>,
    syntax_highlights: Option<std::sync::Arc<crate::syntax::Observation>>,
    pub syntax_palette: syntax::SyntaxPalette,
    selection: TextSelection,
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
    wrap_reveal: Option<(Range<usize>, Reveal)>,
    search: search::Search,
    indent_job: Option<(
        u64,
        TextSelection,
        background::Job<(BufferEdit, TextSelection)>,
    )>,
    edit_message: Option<String>,
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
pub struct EditorOutput {
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
        reveal: Reveal,
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
        reveal: Reveal,
        focus: bool,
    ) -> Result<(), EditorError> {
        self.request_navigation(
            snapshot,
            Navigation::scroll_to_line(snapshot, line, reveal, focus)?,
        )
    }
    fn prepare_folds(&mut self, snapshot: &TextSnapshot, width: Option<usize>) {
        let wrap = self
            .wrapping
            .index
            .as_ref()
            .filter(|w| width.is_some() && w.snapshot.revision == snapshot.revision);
        self.folding
            .prepare(&snapshot.text, wrap, self.composition.is_none());
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
    pub fn selection(&self) -> TextSelection {
        self.selection
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
    pub fn is_composing(&self) -> bool {
        self.composition.is_some()
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
        } else {
            session.end_undo_group();
        }
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
        let fold_gutter = self.has_syntax;
        let gutter_width = gutter_width + if fold_gutter { 14.0 } else { 0.0 };
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
        let mut caret_rect = egui::Rect::NOTHING;
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
                        self.prepare_folds(&snapshot, wrap_width);
                        self.folding.animate(
                            &snapshot.text,
                            self.wrapping.index.as_ref().filter(|index| {
                                wrap_width.is_some() && index.snapshot.revision == snapshot.revision
                            }),
                            self.composition.is_none(),
                            now,
                            motion.enabled,
                            ui.ctx(),
                        );
                        let geometry = layout::Geometry {
                            text: &snapshot.text,
                            metrics: &metrics,
                            wrap: self.active_wrap(&snapshot, wrap_width),
                            folds: &self.folding.display_map,
                        };
                        let size = geometry.size(viewport.size());
                        let rect = egui::Rect::from_min_size(origin, size);
                        ui.allocate_rect(rect, egui::Sense::hover());
                        let response = ui.interact(rect, id, egui::Sense::click_and_drag());
                        ui.ctx().register_text_input(id);
                        if response.hovered() {
                            ui.set_cursor_icon(egui::CursorIcon::Text);
                        }
                        let pressed = ui.input(|i| i.pointer.primary_pressed());
                        if response.hovered() && pressed {
                            self.cancel_composition(session, id);
                            response.request_focus();
                            let position = ui.input(|i| i.pointer.interact_pos()).unwrap();
                            let geometry = layout::Geometry {
                                text: &snapshot.text,
                                metrics: &metrics,
                                wrap: self.active_wrap(&snapshot, wrap_width),
                                folds: &self.folding.display_map,
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
                            } else {
                                self.selection = TextSelection {
                                    anchor: hit,
                                    head: hit,
                                };
                            }
                            self.drag_anchor = Some(self.selection.anchor);
                        }
                        if response.dragged_by(egui::PointerButton::Primary) {
                            self.pointer_presses.last = None;
                            if let Some(position) = ui.input(|i| i.pointer.interact_pos()) {
                                let geometry = layout::Geometry {
                                    text: &snapshot.text,
                                    metrics: &metrics,
                                    wrap: self.active_wrap(&snapshot, wrap_width),
                                    folds: &self.folding.display_map,
                                };
                                let local = position - origin.to_vec2();
                                let hit = geometry.hit(ui, local);
                                self.upstream = geometry.visual_row(hit)
                                    > (((local.y - 6.0) / metrics.height).floor().max(0.0)
                                        as usize);
                                self.selection = TextSelection {
                                    anchor: self.drag_anchor.unwrap_or(self.selection.anchor),
                                    head: hit,
                                };
                                reveal = Some((hit..hit, Reveal::Minimal));
                            }
                        }
                        let focused =
                            response.has_focus() && ui.input(|i| i.focused) && ui.is_enabled();
                        if !focused && self.composition.is_some() {
                            self.cancel_composition(session, id);
                        }
                        if focused {
                            ui.memory_mut(|memory| {
                                memory.set_focus_lock_filter(
                                    id,
                                    egui::EventFilter {
                                        tab: true,
                                        horizontal_arrows: true,
                                        vertical_arrows: true,
                                        escape: self.search.open || self.is_composing(),
                                    },
                                )
                            });
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
                                                Reveal::Minimal,
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
                        }
                        if self.focused && !focused {
                            session.end_undo_group();
                        }
                        self.focused = focused;
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
                        self.folding
                            .prepare(&display, wrap, self.composition.is_none());
                        self.folding.animate(
                            &display,
                            wrap,
                            self.composition.is_none(),
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
                        };
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
                        for row in first..last {
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
                            let row_height = geometry.row_height(row) * metrics.height;
                            let painter =
                                painter.with_clip_rect(clip.intersect(egui::Rect::from_min_max(
                                    egui::pos2(clip.left(), y),
                                    egui::pos2(clip.right(), y + row_height),
                                )));
                            let selected = selection.range();
                            let body = segment.bytes.clone();
                            if selected.start
                                < if wrapped {
                                    body.end
                                } else {
                                    display.line_start(segment.line + 1)
                                }
                                && selected.end > body.start
                                && !selected.is_empty()
                            {
                                let left = if selected.start <= body.start {
                                    segment.indent as f32 * metrics.cell
                                } else {
                                    line.x_at(selected.start)
                                };
                                let right = if selected.end > body.end {
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
                            if segment.part == 0
                                && self.folding.collapsed_at(&display, segment.line)
                            {
                                painter.text(
                                    egui::pos2(origin.x + 12.0 + line.right(), y),
                                    egui::Align2::LEFT_TOP,
                                    " …",
                                    metrics.font.clone(),
                                    ui.visuals().weak_text_color(),
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
                                for (index, diagnostic) in report
                                    .intersecting(line.start..line.end)
                                    .take(256)
                                    .enumerate()
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
                                    let rect = egui::Rect::from_min_max(
                                        egui::pos2(origin.x + 8.0 + left, y),
                                        egui::pos2(origin.x + 8.0 + right, y + metrics.height),
                                    )
                                    .intersect(painter.clip_rect());
                                    if rect.is_positive() {
                                        ui.interact(
                                            rect,
                                            id.with(("diagnostic", row, index)),
                                            egui::Sense::hover(),
                                        )
                                        .on_hover_text(diagnostic.message.as_ref());
                                    }
                                }
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
                                    let text =
                                        display.slice(geometry.segment(row).bytes).to_string();
                                    text.trim_end_matches(['\n', '\r']).to_owned()
                                })
                                .collect();
                            egui::WidgetInfo::text_edit(ui.is_enabled(), "", visible.join("\n"), "")
                        });
                        if wrapped != self.wrap_active && reveal.is_none() {
                            reveal = self
                                .wrap_reveal
                                .take()
                                .or(Some((selection.head..selection.head, Reveal::Minimal)));
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
                                if *alignment == Reveal::Center {
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
                        if row == snapshot.text.line_of(self.selection.head) {
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
        Ok(EditorOutput {
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
        self.reconcile_folds(session, snapshot)?;
        let width = self
            .wrapping
            .index
            .as_ref()
            .filter(|_| self.settings.wrap)
            .map(|w| w.width);
        self.prepare_folds(snapshot, width);
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
                self.replace_selection(id, session, snapshot, text.clone(), now)?;
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
                            self.selection = TextSelection {
                                anchor: 0,
                                head: snapshot.text.len(),
                            };
                            session.end_undo_group();
                            return Ok(true);
                        }
                        egui::Key::C | egui::Key::X => {
                            return self.handle_event(
                                ui,
                                id,
                                session,
                                snapshot,
                                &if *key == egui::Key::C {
                                    egui::Event::Copy
                                } else {
                                    egui::Event::Cut
                                },
                                now,
                                page,
                            );
                        }
                        egui::Key::V => return Ok(true),
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
                let row = snapshot.text.line_of(head);
                let body = snapshot.text.line_range(row);
                let selection = self.selection.range();
                let mut target_upstream = matches!(key, egui::Key::ArrowLeft | egui::Key::End);
                let wrapped_segment = self
                    .wrapping
                    .index
                    .as_ref()
                    .filter(|index| {
                        self.settings.wrap && index.snapshot.revision == snapshot.revision
                    })
                    .map(|index| {
                        let mut visual = index.visual_row(head);
                        if self.upstream && visual > 0 && index.segment(visual).bytes.start == head
                        {
                            visual -= 1;
                        }
                        index.segment(visual)
                    });
                let target = match key {
                    egui::Key::ArrowLeft => Some(if !modifiers.shift && !selection.is_empty() {
                        selection.start
                    } else if mac && primary {
                        body.start
                    } else if word {
                        word_boundary(&snapshot.text, head, false)
                    } else {
                        snapshot.text.previous_grapheme(head)
                    }),
                    egui::Key::ArrowRight => Some(if !modifiers.shift && !selection.is_empty() {
                        selection.end
                    } else if mac && primary {
                        body.end
                    } else if word {
                        word_boundary(&snapshot.text, head, true)
                    } else {
                        snapshot.text.next_grapheme(head)
                    }),
                    egui::Key::Home => Some(if primary {
                        0
                    } else {
                        if let Some(segment) = &wrapped_segment {
                            if segment.part > 0 {
                                segment.bytes.start
                            } else {
                                let first = whitespace::leading_end(&snapshot.text, row, body.end);
                                if head == first { body.start } else { first }
                            }
                        } else {
                            let first = whitespace::leading_end(&snapshot.text, row, body.end);
                            if head == first { body.start } else { first }
                        }
                    }),
                    egui::Key::End => Some(if primary {
                        snapshot.text.len()
                    } else {
                        wrapped_segment
                            .as_ref()
                            .map_or(body.end, |segment| segment.bytes.end)
                    }),
                    egui::Key::ArrowUp
                    | egui::Key::ArrowDown
                    | egui::Key::PageUp
                    | egui::Key::PageDown => {
                        if mac && primary && *key == egui::Key::ArrowUp {
                            Some(0)
                        } else if mac && primary && *key == egui::Key::ArrowDown {
                            Some(snapshot.text.len())
                        } else {
                            let wrap = self.wrapping.index.as_ref().filter(|index| {
                                self.settings.wrap && index.snapshot.revision == snapshot.revision
                            });
                            let geometry = layout::Geometry {
                                text: &snapshot.text,
                                metrics: &layout::Metrics::new(ui),
                                wrap,
                                folds: &self.folding.map,
                            };
                            let column = *self
                                .preferred_column
                                .get_or_insert_with(|| geometry.column_for(head, self.upstream));
                            let step = if matches!(key, egui::Key::PageUp | egui::Key::PageDown) {
                                page
                            } else {
                                1
                            };
                            let step = if matches!(key, egui::Key::ArrowUp | egui::Key::PageUp) {
                                -(step as isize)
                            } else {
                                step as isize
                            };
                            let (target, upstream) =
                                geometry.vertical(head, column, step, self.upstream);
                            target_upstream = upstream;
                            Some(target)
                        }
                    }
                    _ => None,
                };
                if let Some(target) = target {
                    if !matches!(
                        key,
                        egui::Key::ArrowUp
                            | egui::Key::ArrowDown
                            | egui::Key::PageUp
                            | egui::Key::PageDown
                    ) {
                        self.preferred_column = None;
                    }
                    self.upstream = target_upstream;
                    self.selection = TextSelection {
                        anchor: if modifiers.shift {
                            self.selection.anchor
                        } else {
                            target
                        },
                        head: target,
                    };
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
fn map_selection(
    session: &dyn TextSession,
    before: u64,
    after: &TextSnapshot,
    selection: TextSelection,
) -> Result<TextSelection, EditorError> {
    if before == after.revision {
        return Ok(selection);
    }
    if let Some(commits) = session.changes_since(before)?
        && commits.iter().all(|commit| commit.changes.is_some())
    {
        return Ok(commits
            .iter()
            .flat_map(|commit| commit.changes.as_ref().unwrap())
            .fold(selection, |selection, change| selection.mapped(change)));
    }
    // A replacement or journal gap invalidates position knowledge. Clamp to
    // valid positions rather than diffing a potentially enormous document on UI.
    Ok(TextSelection {
        anchor: after.text.clamp_boundary(selection.anchor),
        head: after.text.clamp_boundary(selection.head),
    })
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
