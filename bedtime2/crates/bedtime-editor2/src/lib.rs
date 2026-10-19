//! Viewport-only plain text editor. Canonical text and history belong to a
//! TextSession; views own selection, composition and navigation. No full-file
//! string or galley is constructed while drawing or performing ordinary edits.
mod caret;
mod document;
mod layout;
mod text;
pub use document::*;
pub use text::{BufferEdit, TextBuffer};
#[cfg(feature = "workbench")]
pub mod workbench;
use bedtime_ui2::{MotionPolicy, egui};
use bevy::prelude::Component;
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
}
impl Default for EditorSettings {
    fn default() -> Self {
        Self {
            line_numbers: true,
            read_only: false,
            animate_caret: true,
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
    selection: TextSelection,
    observed_revision: Option<u64>,
    navigation: Option<Navigation>,
    composition: Option<Composition>,
    caret: caret::CaretAnimation,
    last_edit: Option<f64>,
    preferred_column: Option<usize>,
    focused: bool,
    scroll: egui::Vec2,
    drag_anchor: Option<usize>,
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
impl EditorView {
    pub fn selection(&self) -> TextSelection {
        self.selection
    }
    pub fn scroll_offset(&self) -> egui::Vec2 {
        self.scroll
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
        self.preferred_column = None;
        self.last_edit = Some(now);
        Ok(())
    }
    pub fn show(
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
        let navigation = self.navigation.take();
        if let Some(nav) = &navigation {
            if nav.revision != snapshot.revision {
                return Err(EditorError(
                    "Navigation became stale before presentation".into(),
                ));
            }
            if let Some(selection) = nav.selection {
                self.selection = selection;
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
            snapshot.text.line_count().to_string().len().max(2) as f32 * metrics.cell + 22.0
        } else {
            0.0
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
                let scroll = egui::ScrollArea::both()
                    .id_salt(id.with("scroll"))
                    .auto_shrink([false, false])
                    .max_width((available.x - gutter_width).max(1.0))
                    .max_height(available.y)
                    .scroll_source(egui::scroll_area::ScrollSource {
                        drag: egui::scroll_area::DragScroll::Never,
                        ..Default::default()
                    })
                    .show_viewport(ui, |ui, viewport| {
                        let origin = ui.cursor().min;
                        let size = metrics.content_size(&snapshot.text).max(viewport.size());
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
                            let hit = layout::hit(
                                ui,
                                &snapshot.text,
                                &metrics,
                                position - origin.to_vec2(),
                            );
                            session.end_undo_group();
                            self.preferred_column = None;
                            if ui.input(|i| i.modifiers.shift) {
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
                            if let Some(position) = ui.input(|i| i.pointer.interact_pos()) {
                                let hit = layout::hit(
                                    ui,
                                    &snapshot.text,
                                    &metrics,
                                    position - origin.to_vec2(),
                                );
                                self.selection = TextSelection {
                                    anchor: self.drag_anchor.unwrap_or(self.selection.anchor),
                                    head: hit,
                                };
                                reveal = Some((hit..hit, Reveal::Minimal));
                            }
                        }
                        if response.double_clicked() {
                            let range = word_range(&snapshot.text, self.selection.head);
                            self.selection = TextSelection {
                                anchor: range.start,
                                head: range.end,
                            };
                        } else if response.triple_clicked() {
                            let row = snapshot.text.line_of(self.selection.head);
                            self.selection = TextSelection {
                                anchor: snapshot.text.line_start(row),
                                head: snapshot.text.line_start(row + 1),
                            };
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
                                        ..Default::default()
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
                        ui.set_min_size(metrics.content_size(&display).max(viewport.size()));
                        let clip = ui.clip_rect();
                        let painter = ui.painter().with_clip_rect(clip);
                        let first =
                            ((viewport.top() - 6.0) / metrics.height).floor().max(0.0) as usize;
                        let last = (((viewport.bottom() + 6.0) / metrics.height).ceil() as usize
                            + 1)
                        .min(display.line_count());
                        let first_cell =
                            ((viewport.left() - 8.0) / metrics.cell).floor().max(0.0) as usize;
                        let last_cell =
                            ((viewport.right() + 8.0) / metrics.cell).ceil().max(1.0) as usize + 4;
                        let mut local_caret = metrics.caret(&display, selection.head);
                        let caret_row = display.line_of(selection.head);
                        for row in first..last {
                            let line = layout::VisibleLine::new(
                                ui,
                                &display,
                                &metrics,
                                row,
                                first_cell..last_cell,
                            );
                            laid_out_bytes += line.source_bytes;
                            rendered_lines += 1;
                            let y = origin.y + 6.0 + row as f32 * metrics.height;
                            number_rows.push((row, y));
                            let selected = selection.range();
                            let body = display.line_range(row);
                            if selected.start < display.line_start(row + 1)
                                && selected.end > body.start
                                && !selected.is_empty()
                            {
                                let left = if selected.start <= body.start {
                                    0.0
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
                            painter.galley(
                                egui::pos2(origin.x + 8.0 + line.left(), y),
                                line.galley.clone(),
                                ui.visuals().text_color(),
                            );
                            if caret_row == row {
                                local_caret = egui::Rect::from_min_size(
                                    egui::pos2(
                                        8.0 + line.x_at(selection.head),
                                        6.0 + row as f32 * metrics.height,
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
                        if let Some((range, alignment)) = &reveal {
                            let target = if self.composition.is_some() {
                                local_caret
                            } else {
                                metrics
                                    .caret(&display, range.start)
                                    .union(metrics.caret(&display, range.end))
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
                            ui.visuals().text_color(),
                            !self.settings.read_only,
                        );
                        response
                    });
                (gutter, scroll)
            })
            .inner;
        self.scroll = scroll.state.offset;
        self.observed_revision = Some(snapshot.revision);
        if self.settings.line_numbers {
            let painter = ui
                .painter()
                .with_clip_rect(gutter.intersect(ui.clip_rect()));
            for (row, y) in number_rows {
                painter.text(
                    egui::pos2(gutter.right() - 10.0, y),
                    egui::Align2::RIGHT_TOP,
                    (row + 1).to_string(),
                    metrics.font.clone(),
                    ui.visuals().weak_text_color(),
                );
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
                    self.replace_selection(id, session, snapshot, text.clone(), now)?;
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
                    egui::Key::Home => Some(if primary { 0 } else { body.start }),
                    egui::Key::End => Some(if primary {
                        snapshot.text.len()
                    } else {
                        body.end
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
                            let column = *self
                                .preferred_column
                                .get_or_insert_with(|| snapshot.text.column(head));
                            let step = if matches!(key, egui::Key::PageUp | egui::Key::PageDown) {
                                page
                            } else {
                                1
                            };
                            let row = if matches!(key, egui::Key::ArrowUp | egui::Key::PageUp) {
                                row.saturating_sub(step)
                            } else {
                                (row + step).min(snapshot.text.line_count() - 1)
                            };
                            Some(snapshot.text.byte_at_column(row, column))
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
                                        snapshot.text.previous_grapheme(head)
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
                            let first_end = snapshot.text.line_start(1);
                            let ending = if first_end > 0
                                && snapshot.text.byte(first_end - 1) == Some(b'\r')
                            {
                                "\r"
                            } else if first_end > 1
                                && snapshot.text.byte(first_end - 1) == Some(b'\n')
                                && snapshot.text.byte(first_end - 2) == Some(b'\r')
                            {
                                "\r\n"
                            } else {
                                "\n"
                            };
                            self.replace_selection(
                                id,
                                session,
                                snapshot,
                                if *key == egui::Key::Tab { "\t" } else { ending }.into(),
                                now,
                            )?;
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
    if let Some(commits) = session.changes_since(before)? {
        if commits.iter().all(|commit| commit.changes.is_some()) {
            return Ok(commits
                .iter()
                .flat_map(|commit| commit.changes.as_ref().unwrap())
                .fold(selection, |selection, change| selection.mapped(change)));
        }
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
