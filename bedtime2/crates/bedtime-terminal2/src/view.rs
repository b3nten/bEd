//! Fixed-grid egui terminal canvas, with no process/transport ownership.
//!
//! Each `TerminalCell::text` is shaped as an indivisible cluster by egui, placed
//! in the cell's one- or two-column span, and clipped to that span. Continuation
//! cells are never drawn as independent characters. This preserves combining
//! marks and wide-cell geometry; contextual shaping *between* terminal cells,
//! bidi reordering, and color emoji are not implemented here. Glyph coverage and
//! within-cluster shaping are supplied by the host's egui fonts. Bold uses UI2's
//! matching monospace bold face; italic uses egui's text format.
//!
//! Immutable row Arcs retain prepared galleys/geometry. Selection, focus and
//! cursor/text blinking are paint overlays and do not invalidate text layout.
pub(crate) use crate::input::TerminalInput;
use crate::input::{InputMapper, KeyboardAction, MappedEvent, mouse_modifiers, mouse_packet};
use bedtime_terminal::{
    terminal::*,
    terminal_input::{TerminalKey, TerminalKeyEvent},
    terminal_pty::{TerminalCommand, WindowSize},
};
use bedtime_ui2::{
    Theme,
    egui::{self, Color32, FontId, Painter, PointerButton, Pos2, Rect, Stroke, Vec2, pos2, vec2},
};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

pub(crate) const PADDING: Vec2 = vec2(14.0, 12.0);
const CURSOR_PERIOD: f64 = 0.6;
const TEXT_PERIOD: f64 = 0.8;
const DEFAULT_FG: usize = 258;
const DEFAULT_BG: usize = 259;

pub(crate) struct ViewOutput {
    pub size: WindowSize,
    pub inputs: Vec<TerminalInput>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct CellPoint {
    row: usize,
    col: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Gesture {
    #[default]
    None,
    Local {
        rectangular: bool,
    },
    Application {
        sgr: bool,
        x10: bool,
        motion: bool,
    },
}

#[derive(Clone)]
struct LocalSelection {
    anchor: CellPoint,
    end: CellPoint,
    snap: SelectionSnap,
    rectangular: bool,
}

struct PreparedCluster {
    col: usize,
    width: usize,
    galley: Option<Arc<egui::Galley>>,
    color: Color32,
    underline: TerminalUnderline,
    underline_color: Color32,
    struck: bool,
    blink: bool,
}

struct Background {
    start: usize,
    end: usize,
    color: Color32,
}

struct PreparedRow {
    source: Arc<TerminalRow>,
    backgrounds: Vec<Background>,
    clusters: Vec<PreparedCluster>,
}

#[derive(PartialEq)]
struct PaintKey {
    font: FontId,
    cell: Vec2,
    pixels_per_point: f32,
    palette: Arc<[[u8; 3]; 260]>,
    faint_palette: Option<Arc<[[u8; 3]; 260]>>,
    reverse: bool,
}

/// One instance belongs to one panel. All egui identity is scoped under the
/// caller's Ui ID; no global focus, selection, font, or transport state is used.
#[derive(Default)]
pub(crate) struct TerminalView {
    keyboard: InputMapper,
    keyboard_owned: bool,
    process_focused: bool,
    popup_was_open: bool,
    gestures: [Gesture; 3],
    pointer: Option<Pos2>,
    pointer_modifiers: egui::Modifiers,
    last_cell: Option<CellPoint>,
    last_press: Option<(f64, Pos2, u8)>,
    scroll_remainder: f32,
    mouse_wheel_remainder: [f32; 2],
    last_autoscroll: f64,
    local_selection: Option<LocalSelection>,
    frozen_selection: bool,
    selection_requested_at: Option<u64>,
    selection_viewport: Option<(usize, usize, usize)>,
    last_cursor_revision: Option<u64>,
    cursor_started: f64,
    paint_key: Option<PaintKey>,
    atlas_probe: Option<Arc<egui::Galley>>,
    rows: HashMap<usize, PreparedRow>,
    #[cfg(test)]
    prepared_rows: usize,
}

impl TerminalView {
    /// Retire input ownership when the host stops drawing this panel. This is
    /// idempotent; it never consumes another widget's events or touches I/O.
    /// The host sends the resulting cleanup in order, filtering process input
    /// if the child is already gone just as it does for `show`.
    pub(crate) fn suspend(&mut self) -> Vec<TerminalInput> {
        let mut inputs = self.keyboard.suspend(true);
        self.end_application_gestures(true, &mut inputs);
        if let Gesture::Local { rectangular } = self.gestures[0] {
            let point = self.last_cell.unwrap_or_default();
            inputs.push(TerminalInput::Command(TerminalCommand::SelectionExtend {
                col: point.col,
                row: point.row,
                rectangular,
                done: true,
            }));
        }
        if self.process_focused {
            inputs.push(TerminalInput::Command(TerminalCommand::Focus(false)));
        }
        self.gestures = [Gesture::None; 3];
        self.keyboard_owned = false;
        self.process_focused = false;
        self.pointer = None;
        self.pointer_modifiers = egui::Modifiers::NONE;
        self.last_cell = None;
        self.last_press = None;
        self.scroll_remainder = 0.0;
        self.mouse_wheel_remainder = [0.0; 2];
        inputs
    }

    /// `panel_focused` is the host's panel eligibility; egui still owns actual
    /// widget focus. `allow_input` controls PTY input, not read-only inspection.
    /// The host must also withhold permission for a popup/modal's closing frame
    /// if it closed before this canvas was drawn. Current/prior popup state is
    /// checked locally as well, without consuming any popup's keyboard events.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        snapshot: &Arc<TerminalSnapshot>,
        theme: &Theme,
        panel_focused: bool,
        allow_input: bool,
        reserved: &[egui::KeyboardShortcut],
    ) -> ViewOutput {
        let density = ui.ctx().pixels_per_point();
        let font = FontId::monospace(theme.metrics.font_size);
        let cell = ui.ctx().fonts_mut(|fonts| {
            vec2(
                pixel_ceil(fonts.glyph_width(&font, 'M').max(1.0), density),
                pixel_ceil(fonts.row_height(&font).max(1.0), density),
            )
        });
        let (_, rect) = ui.allocate_space(ui.available_size().max(Vec2::splat(1.0)));
        let id = ui.id().with("terminal_canvas");
        let response = ui.interact(rect, id, egui::Sense::click_and_drag());
        let mut inner = rect.shrink2(PADDING);
        // Every column/row starts on a physical pixel, even when panel layout,
        // zoom or display scaling puts the allocation on a fractional point.
        inner.min = pos2(
            pixel_ceil(inner.min.x, density),
            pixel_ceil(inner.min.y, density),
        );
        let clip = ui.clip_rect().intersect(rect);
        let cols = (inner.width().max(0.0) / cell.x)
            .floor()
            .clamp(2.0, u16::MAX as f32) as u16;
        let rows = (inner.height().max(0.0) / cell.y)
            .floor()
            .clamp(1.0, u16::MAX as f32) as u16;
        let mut output = ViewOutput {
            size: WindowSize {
                num_cols: cols,
                num_lines: rows,
                cell_width: (cell.x * density).round().clamp(1.0, u16::MAX as f32) as u16,
                cell_height: (cell.y * density).round().clamp(1.0, u16::MAX as f32) as u16,
            },
            inputs: Vec::new(),
        };
        let now = ui.input(|input| input.time);
        // any_popup_open observes popups already drawn in this pass; the
        // memory-backed check also catches an open menu drawn after us.
        let popup_open = ui.ctx().any_popup_open()
            || egui::Popup::is_any_open(ui.ctx())
            || ui.ctx().memory(|memory| memory.top_modal_layer().is_some());
        let blocked = popup_open || self.popup_was_open;
        self.popup_was_open = popup_open;
        let visible = ui.is_rect_visible(rect);
        let pointer_allowed =
            visible && ui.is_enabled() && ui.input(|input| input.focused) && !blocked;
        if pointer_allowed && panel_focused && ui.ctx().memory(|memory| memory.focused().is_none())
        {
            response.request_focus();
        }
        if pointer_allowed && (response.is_pointer_button_down_on() || response.clicked()) {
            response.request_focus();
        }
        let keyboard_owned = pointer_allowed && panel_focused && response.has_focus();
        let process_focused = keyboard_owned && allow_input;
        if keyboard_owned {
            ui.ctx().memory_mut(|memory| {
                memory.set_focus_lock_filter(
                    id,
                    egui::EventFilter {
                        tab: true,
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        escape: true,
                    },
                )
            });
        }
        if self.keyboard_owned && !keyboard_owned || self.process_focused && !process_focused {
            // These are releases of this canvas's earlier presses, not input
            // from the popup/window which took focus. The host filters them if
            // the child exited; suspension must not leave Kitty keys held.
            output.inputs.extend(self.keyboard.suspend(true));
        }
        if self.process_focused != process_focused {
            output
                .inputs
                .push(TerminalInput::Command(TerminalCommand::Focus(
                    process_focused,
                )));
            self.cursor_started = now;
        }
        self.keyboard_owned = keyboard_owned;
        self.process_focused = process_focused;
        if !process_focused {
            self.end_application_gestures(true, &mut output.inputs);
        }
        if !pointer_allowed {
            self.end_local_gesture(snapshot, allow_input, &mut output.inputs);
        }
        // When the retained emulator acknowledges a read-only selection, prefer
        // its authoritative (potentially full-scrollback) text over the frozen
        // viewport fallback. The fallback also works without a running worker.
        if self
            .selection_requested_at
            .is_some_and(|revision| revision != snapshot.revision)
            && (snapshot.selection_text.is_some() || !snapshot.selection.is_empty())
        {
            self.frozen_selection = false;
            self.selection_requested_at = None;
        }
        let viewport = (snapshot.cols, snapshot.rows, snapshot.display_offset);
        if self
            .selection_viewport
            .is_some_and(|previous| previous != viewport)
        {
            self.local_selection = None;
            self.frozen_selection = false;
        }
        self.selection_viewport = Some(viewport);
        if allow_input {
            self.frozen_selection = false;
        }

        // Take/reinsert the remaining event stream rather than calling bulk
        // consume_key: order, repeats, releases, and reserved chords survive.
        // Do not hold an input lock while requesting focus or clipboard output.
        let events = ui.input_mut(|input| std::mem::take(&mut input.events));
        let mapped = if keyboard_owned {
            self.keyboard
                .map_events(&events, allow_input, reserved, cfg!(target_os = "macos"))
        } else {
            (0..events.len()).map(|_| MappedEvent::default()).collect()
        };
        let mut remaining = Vec::with_capacity(events.len());
        let mut consumed_wheel = false;
        let pointer_owner = response.is_pointer_button_down_on()
            || response.clicked()
            || response.dragged()
            || response.contains_pointer();
        if self.pointer.is_none() {
            self.pointer = ui.input(|input| input.pointer.latest_pos());
        }
        for (event, mapped) in events.into_iter().zip(mapped) {
            for action in mapped.actions {
                match action {
                    KeyboardAction::Input(input) => output.inputs.push(input),
                    KeyboardAction::Copy => {
                        if let Some(text) = self.selected_text(snapshot) {
                            ui.ctx().copy_text(text);
                        }
                    }
                    KeyboardAction::SelectAll if allow_input => {
                        output
                            .inputs
                            .push(TerminalInput::Command(TerminalCommand::SelectAll));
                        self.local_selection = None;
                        self.frozen_selection = false;
                    }
                    KeyboardAction::SelectAll => {
                        self.local_selection = Some(LocalSelection {
                            anchor: CellPoint::default(),
                            end: CellPoint {
                                row: snapshot.rows.saturating_sub(1),
                                col: snapshot.cols.saturating_sub(1),
                            },
                            snap: SelectionSnap::Line,
                            rectangular: false,
                        });
                        self.frozen_selection = true;
                        self.selection_requested_at = Some(snapshot.revision);
                        output
                            .inputs
                            .push(TerminalInput::Command(TerminalCommand::SelectAll));
                    }
                    KeyboardAction::ScrollPage(direction) => {
                        output
                            .inputs
                            .push(TerminalInput::Command(TerminalCommand::Scroll(
                                direction
                                    * snapshot
                                        .rows
                                        .saturating_sub(1)
                                        .max(1)
                                        .min(i32::MAX as usize)
                                        as i32,
                            )))
                    }
                    KeyboardAction::ScrollEdge { top } => {
                        output
                            .inputs
                            .push(TerminalInput::Command(TerminalCommand::Scroll(if top {
                                snapshot
                                    .history_size
                                    .saturating_sub(snapshot.display_offset)
                                    .min(i32::MAX as usize) as i32
                            } else {
                                -(snapshot.display_offset.min(i32::MAX as usize) as i32)
                            })))
                    }
                }
            }
            let mut consumed = mapped.consumed;
            match &event {
                egui::Event::PointerButton {
                    pos,
                    button,
                    pressed,
                    modifiers,
                } if pointer_allowed => {
                    self.pointer = Some(*pos);
                    self.pointer_modifiers = *modifiers;
                    if let Some(button_index) = button_index(*button) {
                        let point = cell_at(*pos, inner.min, cell, snapshot);
                        if *pressed && pointer_owner && hit(ui, clip, *pos) {
                            response.request_focus();
                            let application = process_focused
                                && !modifiers.shift
                                && mouse_reporting(snapshot.modes);
                            if application {
                                let x10 = snapshot.modes.mouse_x10;
                                output.inputs.push(TerminalInput::Bytes(mouse_packet(
                                    button_index as u8,
                                    point.col,
                                    point.row,
                                    if x10 { 0 } else { mouse_modifiers(*modifiers) },
                                    false,
                                    snapshot.modes.mouse_sgr,
                                )));
                                self.gestures[button_index] = Gesture::Application {
                                    sgr: snapshot.modes.mouse_sgr,
                                    x10,
                                    motion: snapshot.modes.mouse_motion
                                        || snapshot.modes.mouse_many,
                                };
                            } else {
                                self.gestures[button_index] = Gesture::Local {
                                    rectangular: modifiers.alt,
                                };
                                if button_index == 0 {
                                    let count = self
                                        .last_press
                                        .filter(|(time, last, _)| {
                                            now - time < 0.35 && last.distance(*pos) < 6.0
                                        })
                                        .map_or(
                                            1,
                                            |(_, _, count)| if count == 3 { 1 } else { count + 1 },
                                        );
                                    self.last_press = Some((now, *pos, count));
                                    let snap = match count {
                                        3 => SelectionSnap::Line,
                                        2 => SelectionSnap::Word,
                                        _ => SelectionSnap::None,
                                    };
                                    self.local_selection = Some(LocalSelection {
                                        anchor: point,
                                        end: point,
                                        snap,
                                        rectangular: modifiers.alt,
                                    });
                                    self.frozen_selection = !allow_input;
                                    self.selection_requested_at = Some(snapshot.revision);
                                    output.inputs.push(TerminalInput::Command(
                                        TerminalCommand::SelectionStart {
                                            col: point.col,
                                            row: point.row,
                                            snap,
                                        },
                                    ));
                                }
                            }
                            self.last_cell = Some(point);
                            consumed = true;
                        } else if !pressed {
                            match self.gestures[button_index] {
                                Gesture::Application {
                                    sgr, x10: false, ..
                                } if allow_input => {
                                    output.inputs.push(TerminalInput::Bytes(mouse_packet(
                                        button_index as u8,
                                        point.col,
                                        point.row,
                                        mouse_modifiers(*modifiers),
                                        true,
                                        sgr,
                                    )))
                                }
                                Gesture::Local { rectangular } if button_index == 0 => {
                                    self.extend_selection(point, rectangular, !allow_input);
                                    self.selection_requested_at = Some(snapshot.revision);
                                    output.inputs.push(TerminalInput::Command(
                                        TerminalCommand::SelectionExtend {
                                            col: point.col,
                                            row: point.row,
                                            rectangular,
                                            done: true,
                                        },
                                    ));
                                }
                                _ => {}
                            }
                            consumed |= self.gestures[button_index] != Gesture::None;
                            self.gestures[button_index] = Gesture::None;
                            self.last_cell = Some(point);
                        }
                    }
                }
                egui::Event::PointerMoved(pos) if pointer_allowed => {
                    self.pointer = Some(*pos);
                    let point = cell_at(*pos, inner.min, cell, snapshot);
                    if self.last_cell != Some(point) {
                        if let Gesture::Local { rectangular } = self.gestures[0] {
                            self.extend_selection(point, rectangular, !allow_input);
                            self.selection_requested_at = Some(snapshot.revision);
                            output.inputs.push(TerminalInput::Command(
                                TerminalCommand::SelectionExtend {
                                    col: point.col,
                                    row: point.row,
                                    rectangular,
                                    done: false,
                                },
                            ));
                            consumed = true;
                        } else if process_focused {
                            let held =
                                self.gestures
                                    .iter()
                                    .enumerate()
                                    .find_map(|(index, gesture)| match gesture {
                                        Gesture::Application {
                                            sgr,
                                            x10: false,
                                            motion: true,
                                        } => Some((index as u8, *sgr)),
                                        _ => None,
                                    });
                            let hover_motion = held.is_none()
                                && self.gestures == [Gesture::None; 3]
                                && snapshot.modes.mouse_many
                                && !self.pointer_modifiers.shift
                                && hit(ui, clip, *pos);
                            if let Some((button, sgr)) = held
                                .or_else(|| hover_motion.then_some((3, snapshot.modes.mouse_sgr)))
                            {
                                output.inputs.push(TerminalInput::Bytes(mouse_packet(
                                    button + 32,
                                    point.col,
                                    point.row,
                                    mouse_modifiers(self.pointer_modifiers),
                                    false,
                                    sgr,
                                )));
                                consumed = true;
                            }
                        }
                    }
                    self.last_cell = Some(point);
                }
                egui::Event::MouseWheel {
                    unit,
                    delta,
                    modifiers,
                    ..
                } if pointer_allowed && self.pointer.is_some_and(|pos| hit(ui, clip, pos)) => {
                    self.pointer_modifiers = *modifiers;
                    let point = cell_at(self.pointer.unwrap(), inner.min, cell, snapshot);
                    let application =
                        process_focused && !modifiers.shift && mouse_reporting(snapshot.modes);
                    let scale = match unit {
                        egui::MouseWheelUnit::Point => vec2(1.0 / cell.x, 1.0 / cell.y),
                        egui::MouseWheelUnit::Line => {
                            Vec2::splat(if application { 1.0 } else { 3.0 })
                        }
                        egui::MouseWheelUnit::Page => {
                            vec2(snapshot.cols as f32, snapshot.rows as f32)
                        }
                    };
                    if application {
                        for (axis, delta) in [delta.y * scale.y, delta.x * scale.x]
                            .into_iter()
                            .enumerate()
                        {
                            let steps = wheel_steps(&mut self.mouse_wheel_remainder[axis], delta);
                            let button = match (axis, steps > 0) {
                                (0, true) => 64,
                                (0, false) => 65,
                                (_, true) => 66,
                                (_, false) => 67,
                            };
                            for _ in 0..steps.unsigned_abs() {
                                output.inputs.push(TerminalInput::Bytes(mouse_packet(
                                    button,
                                    point.col,
                                    point.row,
                                    if snapshot.modes.mouse_x10 {
                                        0
                                    } else {
                                        mouse_modifiers(*modifiers)
                                    },
                                    false,
                                    snapshot.modes.mouse_sgr,
                                )));
                            }
                        }
                    } else {
                        let steps = wheel_steps(&mut self.scroll_remainder, delta.y * scale.y);
                        if snapshot.modes.alt_screen && process_focused && !modifiers.shift {
                            for _ in 0..steps.unsigned_abs() {
                                output
                                    .inputs
                                    .push(TerminalInput::Command(TerminalCommand::Key(
                                        TerminalKeyEvent {
                                            key: if steps > 0 {
                                                TerminalKey::Up
                                            } else {
                                                TerminalKey::Down
                                            },
                                            ..TerminalKeyEvent::default()
                                        },
                                    )));
                            }
                        } else if steps != 0 {
                            output
                                .inputs
                                .push(TerminalInput::Command(TerminalCommand::Scroll(steps)));
                        }
                    }
                    consumed = true;
                    consumed_wheel = true;
                }
                egui::Event::ModifiersChanged(modifiers) => self.pointer_modifiers = *modifiers,
                egui::Event::PointerGone => self.pointer = None,
                _ => {}
            }
            if !consumed {
                remaining.push(event);
            }
        }
        ui.input_mut(|input| {
            input.events = remaining;
            if consumed_wheel || pointer_allowed && response.contains_pointer() {
                input.smooth_scroll_delta = Vec2::ZERO;
            }
        });
        // A local drag keeps its owner outside the canvas, including stationary
        // autoscroll. An app drag likewise reports its own release outside.
        if pointer_allowed
            && allow_input
            && !snapshot.modes.alt_screen
            && let Gesture::Local { rectangular } = self.gestures[0]
            && let Some(pos) = self.pointer
            && (pos.y < inner.top() || pos.y > inner.bottom())
        {
            if now - self.last_autoscroll >= 0.05 {
                let distance = if pos.y < inner.top() {
                    inner.top() - pos.y
                } else {
                    inner.bottom() - pos.y
                };
                let steps = (distance / cell.y).ceil().clamp(-4.0, 4.0) as i32;
                output
                    .inputs
                    .push(TerminalInput::Command(TerminalCommand::Scroll(steps)));
                let point = cell_at(pos, inner.min, cell, snapshot);
                output
                    .inputs
                    .push(TerminalInput::Command(TerminalCommand::SelectionExtend {
                        col: point.col,
                        row: point.row,
                        rectangular,
                        done: false,
                    }));
                self.last_autoscroll = now;
            }
            ui.ctx().request_repaint_after(Duration::from_millis(50));
        }

        if !visible {
            return output;
        }
        self.prepare_rows(ui, snapshot, font.clone(), cell, density);
        let canvas_painter = ui.painter_at(rect);
        let background = resolve(
            TerminalColor::Indexed(if snapshot.modes.reverse {
                DEFAULT_FG
            } else {
                DEFAULT_BG
            }),
            &snapshot.palette,
        );
        // Workbench already painted panel_background, including its alpha. A
        // second default fill would either erase that opacity or apply it twice.
        // Only an explicit shell background or reverse video replaces the panel.
        if snapshot.background_is_overridden || snapshot.modes.reverse {
            canvas_painter.rect_filled(rect, 0.0, background);
        }
        // A retained snapshot can still have the old grid dimensions for a frame
        // after resizing. Its cells, selection and cursor must not eat the inset.
        let painter = canvas_painter.with_clip_rect(inner.intersect(clip));
        let spans = if self.frozen_selection {
            self.local_selection
                .as_ref()
                .map_or_else(Vec::new, |selection| selection.spans(snapshot))
        } else {
            snapshot.selection.clone()
        };
        let text_on = !allow_input || (now / TEXT_PERIOD).floor() as u64 % 2 == 0;
        let selected_color = bedtime_ui2::theme::color(theme.palette.text);
        let selection_fill = bedtime_ui2::theme::color(theme.palette.selection);
        let mut has_blink = false;
        for (row_index, source) in snapshot.lines.iter().enumerate() {
            let origin = inner.min + vec2(0.0, row_index as f32 * cell.y);
            if origin.y >= inner.bottom().min(clip.bottom()) {
                break;
            }
            let row = &self.rows[&(Arc::as_ptr(source) as usize)];
            for fill in &row.backgrounds {
                painter.rect_filled(
                    Rect::from_min_size(
                        origin + vec2(fill.start as f32 * cell.x, 0.0),
                        vec2((fill.end - fill.start) as f32 * cell.x, cell.y),
                    ),
                    0.0,
                    fill.color,
                );
            }
            for span in spans.iter().filter(|span| span.row == row_index) {
                painter.rect_filled(
                    Rect::from_min_size(
                        origin + vec2(span.start as f32 * cell.x, 0.0),
                        vec2(span.end.saturating_sub(span.start) as f32 * cell.x, cell.y),
                    ),
                    0.0,
                    selection_fill,
                );
            }
            for cluster in &row.clusters {
                has_blink |= cluster.blink;
                if cluster.blink && !text_on {
                    continue;
                }
                let selected = spans.iter().any(|span| {
                    span.row == row_index
                        && span.start < cluster.col + cluster.width
                        && span.end > cluster.col
                });
                paint_cluster(
                    &painter,
                    cluster,
                    origin,
                    cell,
                    density,
                    selected.then_some(selected_color),
                );
            }
        }
        if self.last_cursor_revision != Some(snapshot.cursor_revision) {
            self.last_cursor_revision = Some(snapshot.cursor_revision);
            self.cursor_started = now;
        }
        let cursor_on =
            ((now - self.cursor_started).max(0.0) / CURSOR_PERIOD).floor() as u64 % 2 == 0;
        if allow_input && snapshot.display_offset == 0 && snapshot.cursor.visible {
            self.paint_cursor(
                &painter,
                snapshot,
                inner.min,
                cell,
                density,
                process_focused,
                cursor_on,
            );
            if process_focused && snapshot.cursor.blinking {
                repaint_at_boundary(ui.ctx(), now - self.cursor_started, CURSOR_PERIOD);
            }
        }
        if has_blink && allow_input {
            repaint_at_boundary(ui.ctx(), now, TEXT_PERIOD);
        }
        if process_focused {
            let point = CellPoint {
                col: snapshot.cursor.col,
                row: snapshot.cursor.row,
            };
            let cursor_rect = Rect::from_min_size(
                inner.min + vec2(point.col as f32 * cell.x, point.row as f32 * cell.y),
                vec2(1.0 / density, cell.y),
            )
            .intersect(rect);
            ui.ctx().output_mut(|output| {
                output.ime = Some(egui::output::IMEOutput {
                    purpose: egui::IMEPurpose::Terminal,
                    rect,
                    cursor_rect,
                    should_interrupt_composition: false,
                })
            });
            if !self.keyboard.preedit.is_empty() {
                let galley =
                    painter.layout_no_wrap(self.keyboard.preedit.clone(), font, selected_color);
                let position = cursor_rect.left_bottom() - vec2(0.0, cell.y);
                let preedit_rect = Rect::from_min_size(position, galley.size());
                painter.rect_filled(preedit_rect, 0.0, background);
                painter.galley(position, galley, selected_color);
                painter.line_segment(
                    [preedit_rect.left_bottom(), preedit_rect.right_bottom()],
                    Stroke::new(1.0 / density, selected_color),
                );
            }
        }
        if snapshot.history_size > 0 {
            let total = (snapshot.history_size + snapshot.rows).max(1) as f32;
            let track_height = inner.height().max(0.0);
            let height = (track_height * snapshot.rows as f32 / total)
                .max(8.0)
                .min(track_height);
            let fraction = 1.0 - snapshot.display_offset as f32 / snapshot.history_size as f32;
            let top = inner.top() + (track_height - height) * fraction;
            canvas_painter.rect_filled(
                Rect::from_min_size(
                    pos2(pixel_ceil(rect.right() - PADDING.x * 0.5, density), top),
                    vec2(2.0 / density, height),
                ),
                0.0,
                bedtime_ui2::theme::color(theme.palette.muted),
            );
        }
        if pointer_allowed && response.hovered() {
            ui.ctx().set_cursor_icon(
                if allow_input
                    && mouse_reporting(snapshot.modes)
                    && !ui.input(|input| input.modifiers.shift)
                {
                    egui::CursorIcon::Default
                } else {
                    egui::CursorIcon::Text
                },
            );
        }
        output
    }

    fn end_application_gestures(&mut self, allow_input: bool, inputs: &mut Vec<TerminalInput>) {
        let point = self.last_cell.unwrap_or_default();
        for (button, gesture) in self.gestures.iter_mut().enumerate() {
            if let Gesture::Application { sgr, x10, .. } = *gesture {
                if allow_input && !x10 {
                    inputs.push(TerminalInput::Bytes(mouse_packet(
                        button as u8,
                        point.col,
                        point.row,
                        0,
                        true,
                        sgr,
                    )));
                }
                *gesture = Gesture::None;
            }
        }
    }

    fn end_local_gesture(
        &mut self,
        snapshot: &TerminalSnapshot,
        allow_input: bool,
        inputs: &mut Vec<TerminalInput>,
    ) {
        if let Gesture::Local { rectangular } = self.gestures[0] {
            let point = self.last_cell.unwrap_or_default();
            self.extend_selection(point, rectangular, !allow_input);
            inputs.push(TerminalInput::Command(TerminalCommand::SelectionExtend {
                col: point.col.min(snapshot.cols.saturating_sub(1)),
                row: point.row.min(snapshot.rows.saturating_sub(1)),
                rectangular,
                done: true,
            }));
        }
        self.gestures.iter_mut().for_each(|gesture| {
            if matches!(gesture, Gesture::Local { .. }) {
                *gesture = Gesture::None;
            }
        });
    }

    fn extend_selection(&mut self, point: CellPoint, rectangular: bool, frozen: bool) {
        if let Some(selection) = self.local_selection.as_mut() {
            selection.end = point;
            selection.rectangular = rectangular;
            self.frozen_selection = frozen;
        }
    }

    fn selected_text(&self, snapshot: &TerminalSnapshot) -> Option<String> {
        if self.frozen_selection {
            self.local_selection
                .as_ref()
                .map(|selection| selection.text(snapshot))
        } else {
            snapshot.selection_text.as_deref().map(str::to_owned)
        }
    }

    fn prepare_rows(
        &mut self,
        ui: &egui::Ui,
        snapshot: &TerminalSnapshot,
        font: FontId,
        cell: Vec2,
        density: f32,
    ) {
        let key = PaintKey {
            font: font.clone(),
            cell,
            pixels_per_point: density,
            palette: snapshot.palette.clone(),
            faint_palette: snapshot.faint_palette.clone(),
            reverse: snapshot.modes.reverse,
        };
        // Galleys contain ephemeral atlas UVs. epaint 0.36 exposes no public
        // atlas generation through FontsView; Fonts::begin_pass recreates its
        // galley cache when the atlas fills or TextOptions change, even with
        // unchanged definitions/DPI. Touch one memoized glyph each visible
        // frame as a generation witness. Its Arc stays identical on cache hits
        // (including cursor blinks), and changes after those resets or a fresh
        // Context. Cache eviction while hidden conservatively rebuilds once.
        let atlas_probe =
            ui.painter()
                .layout_no_wrap("M".into(), font.clone(), Color32::TRANSPARENT);
        let atlas_changed = self
            .atlas_probe
            .as_ref()
            .is_none_or(|previous| !Arc::ptr_eq(previous, &atlas_probe));
        self.atlas_probe = Some(atlas_probe);
        if self.paint_key.as_ref() != Some(&key) || atlas_changed {
            self.rows.clear();
            self.paint_key = Some(key);
        }
        let mut visible = HashSet::with_capacity(snapshot.lines.len());
        for source in &snapshot.lines {
            let identity = Arc::as_ptr(source) as usize;
            visible.insert(identity);
            if self.rows.get(&identity).is_some_and(|row| {
                row.source.revision == source.revision && Arc::ptr_eq(&row.source, source)
            }) {
                continue;
            }
            self.rows.insert(
                identity,
                prepare_row(ui.painter(), source, snapshot, &font, cell),
            );
            #[cfg(test)]
            {
                self.prepared_rows += 1;
            }
        }
        self.rows.retain(|identity, _| visible.contains(identity));
    }

    fn paint_cursor(
        &self,
        painter: &Painter,
        snapshot: &TerminalSnapshot,
        origin: Pos2,
        cell: Vec2,
        density: f32,
        focused: bool,
        cursor_on: bool,
    ) {
        let cursor = snapshot.cursor;
        let Some(source) = snapshot.lines.get(cursor.row) else {
            return;
        };
        if cursor.col >= source.cells.len() || focused && cursor.blinking && !cursor_on {
            return;
        }
        let col = cluster_start(&source.cells, cursor.col);
        let width = source.cells[col].width.max(1) as usize;
        let rect = Rect::from_min_size(
            origin + vec2(col as f32 * cell.x, cursor.row as f32 * cell.y),
            vec2(width as f32 * cell.x, cell.y),
        );
        let color = resolve(TerminalColor::Indexed(256), &snapshot.palette);
        let thickness = (2.0 / density).max(1.0 / density);
        if !focused {
            painter.rect_stroke(
                rect,
                0.0,
                Stroke::new(1.0 / density, color),
                egui::StrokeKind::Inside,
            );
        } else {
            match cursor.shape {
                3 | 4 => {
                    painter.rect_filled(
                        Rect::from_min_max(
                            pos2(rect.left(), rect.bottom() - thickness),
                            rect.right_bottom(),
                        ),
                        0.0,
                        color,
                    );
                }
                5 | 6 => {
                    painter.rect_filled(
                        Rect::from_min_size(rect.min, vec2(thickness, cell.y)),
                        0.0,
                        color,
                    );
                }
                _ => {
                    painter.rect_filled(rect, 0.0, color);
                    if let Some(cluster) = self.rows[&(Arc::as_ptr(source) as usize)]
                        .clusters
                        .iter()
                        .find(|cluster| cluster.col == col)
                    {
                        paint_cluster(
                            painter,
                            cluster,
                            origin + vec2(0.0, cursor.row as f32 * cell.y),
                            cell,
                            density,
                            Some(resolve(TerminalColor::Indexed(257), &snapshot.palette)),
                        );
                    }
                }
            }
        }
    }
}

fn pixel_ceil(value: f32, density: f32) -> f32 {
    (value * density).ceil() / density
}

fn hit(ui: &egui::Ui, clip: Rect, pos: Pos2) -> bool {
    clip.contains(pos) && ui.ctx().layer_id_at(pos) == Some(ui.layer_id())
}

fn cell_at(pos: Pos2, origin: Pos2, cell: Vec2, snapshot: &TerminalSnapshot) -> CellPoint {
    CellPoint {
        col: (((pos.x - origin.x) / cell.x).floor().max(0.0) as usize)
            .min(snapshot.cols.saturating_sub(1)),
        row: (((pos.y - origin.y) / cell.y).floor().max(0.0) as usize)
            .min(snapshot.rows.saturating_sub(1)),
    }
}

fn button_index(button: PointerButton) -> Option<usize> {
    match button {
        PointerButton::Primary => Some(0),
        PointerButton::Middle => Some(1),
        PointerButton::Secondary => Some(2),
        _ => None,
    }
}

fn mouse_reporting(modes: TerminalModes) -> bool {
    modes.mouse_x10 || modes.mouse_button || modes.mouse_motion || modes.mouse_many
}

fn wheel_steps(remainder: &mut f32, delta: f32) -> i32 {
    let total = (*remainder + delta).clamp(-32.0, 32.0);
    let steps = total.trunc() as i32;
    *remainder = total - steps as f32;
    steps
}

fn repaint_at_boundary(context: &egui::Context, time: f64, period: f64) {
    context.request_repaint_after(Duration::from_secs_f64(
        (period - time.max(0.0).rem_euclid(period)).max(0.001),
    ));
}

fn resolve(color: TerminalColor, palette: &[[u8; 3]; 260]) -> Color32 {
    let [r, g, b] = match color {
        TerminalColor::Indexed(index) => palette[index],
        TerminalColor::Rgb(rgb) => rgb,
    };
    Color32::from_rgb(r, g, b)
}

fn cell_colors(cell: &TerminalCell, snapshot: &TerminalSnapshot) -> (Color32, Color32) {
    let mut fg = resolve(cell.fg, &snapshot.palette);
    let mut bg = resolve(cell.bg, &snapshot.palette);
    if cell.mode & ATTR_BOLD != 0
        && cell.mode & ATTR_FAINT == 0
        && let TerminalColor::Indexed(index @ 0..=7) = cell.fg
    {
        fg = resolve(TerminalColor::Indexed(index + 8), &snapshot.palette);
    }
    if cell.mode & ATTR_FAINT != 0 {
        let faint = match cell.fg {
            TerminalColor::Indexed(index)
                if (index < 16 || index == DEFAULT_FG)
                    && cell.bg == TerminalColor::Indexed(DEFAULT_BG)
                    && !snapshot.modes.reverse =>
            {
                snapshot
                    .faint_palette
                    .as_ref()
                    .map(|palette| resolve(TerminalColor::Indexed(index), palette))
            }
            _ => None,
        };
        fg = faint.unwrap_or_else(|| {
            Color32::from_rgb(
                ((u16::from(fg.r()) + u16::from(bg.r())) / 2) as u8,
                ((u16::from(fg.g()) + u16::from(bg.g())) / 2) as u8,
                ((u16::from(fg.b()) + u16::from(bg.b())) / 2) as u8,
            )
        });
    }
    if snapshot.modes.reverse ^ (cell.mode & ATTR_REVERSE != 0) {
        std::mem::swap(&mut fg, &mut bg);
    }
    (fg, bg)
}

fn prepare_row(
    painter: &Painter,
    source: &Arc<TerminalRow>,
    snapshot: &TerminalSnapshot,
    font: &FontId,
    cell_size: Vec2,
) -> PreparedRow {
    let mut row = PreparedRow {
        source: source.clone(),
        backgrounds: Vec::new(),
        clusters: Vec::new(),
    };
    for (col, cell) in source.cells.iter().enumerate() {
        if cell.width == 0 {
            continue;
        }
        let width = (cell.width as usize).min(source.cells.len() - col);
        let (fg, bg) = cell_colors(cell, snapshot);
        // Default cells inherit the canvas/host background. Do not skip by RGB:
        // an explicit ANSI/truecolor background can equal the theme's color and
        // must remain opaque. Reverse cells also need their swapped background.
        if cell.bg != TerminalColor::Indexed(DEFAULT_BG)
            || cell.mode & ATTR_REVERSE != 0
            || snapshot.modes.reverse
        {
            if let Some(previous) = row.backgrounds.last_mut()
                && previous.end == col
                && previous.color == bg
            {
                previous.end += width;
            } else {
                row.backgrounds.push(Background {
                    start: col,
                    end: col + width,
                    color: bg,
                });
            }
        }
        if cell.mode & ATTR_INVISIBLE != 0 {
            continue;
        }
        let galley =
            if !cell.text.is_empty() && cell.text.as_ref() != " " && cell.text.as_ref() != "\t" {
                let mut job = egui::text::LayoutJob::single_section(
                    cell.text.to_string(),
                    egui::TextFormat {
                        font_id: if cell.mode & ATTR_BOLD != 0 && cell.mode & ATTR_FAINT == 0 {
                            bedtime_ui2::fonts::monospace_bold(font.size)
                        } else {
                            font.clone()
                        },
                        color: fg,
                        italics: cell.mode & ATTR_ITALIC != 0,
                        line_height: Some(cell_size.y),
                        ..egui::TextFormat::default()
                    },
                );
                job.break_on_newline = false;
                Some(painter.layout_job(job))
            } else {
                None
            };
        let underline =
            if cell.underline == TerminalUnderline::None && cell.mode & ATTR_UNDERLINE != 0 {
                TerminalUnderline::Single
            } else {
                cell.underline
            };
        if galley.is_none() && underline == TerminalUnderline::None && cell.mode & ATTR_STRUCK == 0
        {
            continue;
        }
        row.clusters.push(PreparedCluster {
            col,
            width,
            galley,
            color: fg,
            underline,
            underline_color: cell
                .underline_color
                .map_or(fg, |color| resolve(color, &snapshot.palette)),
            struck: cell.mode & ATTR_STRUCK != 0,
            blink: cell.mode & ATTR_BLINK != 0,
        });
    }
    row
}

fn paint_cluster(
    painter: &Painter,
    cluster: &PreparedCluster,
    origin: Pos2,
    cell: Vec2,
    density: f32,
    override_color: Option<Color32>,
) {
    let rect = Rect::from_min_size(
        origin + vec2(cluster.col as f32 * cell.x, 0.0),
        vec2(cluster.width as f32 * cell.x, cell.y),
    );
    let painter = painter.with_clip_rect(rect.intersect(painter.clip_rect()));
    let fg = override_color.unwrap_or(cluster.color);
    if let Some(galley) = &cluster.galley {
        // Center a fallback's natural-width glyph inside its terminal span; do
        // not stretch it, and never let it move following cells off the grid.
        let position = rect.min
            + vec2(
                ((rect.width() - galley.size().x) * 0.5).max(0.0),
                (cell.y - galley.size().y) * 0.5,
            );
        let position = pos2(
            (position.x * density).round() / density,
            (position.y * density).round() / density,
        );
        painter.galley_with_override_text_color(position, galley.clone(), fg);
    }
    let underline_color = override_color.unwrap_or(cluster.underline_color);
    let pixel = 1.0 / density;
    let y = rect.bottom() - 2.0 * pixel;
    let stroke = Stroke::new(pixel, underline_color);
    match cluster.underline {
        TerminalUnderline::None => {}
        TerminalUnderline::Single => {
            painter.line_segment([pos2(rect.left(), y), pos2(rect.right(), y)], stroke);
        }
        TerminalUnderline::Double => {
            painter.line_segment([pos2(rect.left(), y), pos2(rect.right(), y)], stroke);
            painter.line_segment(
                [
                    pos2(rect.left(), y - 2.0 * pixel),
                    pos2(rect.right(), y - 2.0 * pixel),
                ],
                stroke,
            );
        }
        TerminalUnderline::Dotted | TerminalUnderline::Dashed => {
            let dash = if cluster.underline == TerminalUnderline::Dotted {
                pixel
            } else {
                3.0 * pixel
            };
            let mut x = rect.left();
            while x < rect.right() {
                painter.line_segment([pos2(x, y), pos2((x + dash).min(rect.right()), y)], stroke);
                x += dash + 2.0 * pixel;
            }
        }
        TerminalUnderline::Curly => {
            let mut x = rect.left();
            let mut up = false;
            while x < rect.right() {
                let end = (x + 2.0 * pixel).min(rect.right());
                painter.line_segment(
                    [
                        pos2(x, y - if up { 2.0 * pixel } else { 0.0 }),
                        pos2(end, y - if up { 0.0 } else { 2.0 * pixel }),
                    ],
                    stroke,
                );
                up = !up;
                x = end;
            }
        }
    }
    if cluster.struck {
        painter.line_segment(
            [
                pos2(rect.left(), rect.center().y),
                pos2(rect.right(), rect.center().y),
            ],
            Stroke::new(pixel, fg),
        );
    }
}

fn cluster_start(cells: &[TerminalCell], col: usize) -> usize {
    let mut col = col.min(cells.len().saturating_sub(1));
    while col > 0 && cells[col].width == 0 {
        col -= 1;
    }
    col
}

impl LocalSelection {
    /// Frozen-snapshot selection is intentionally viewport-local. The worker's
    /// selection is authoritative while live and can span the full scrollback.
    fn spans(&self, snapshot: &TerminalSnapshot) -> Vec<TerminalSelectionSpan> {
        if snapshot.lines.is_empty() || snapshot.cols == 0 {
            return Vec::new();
        }
        let endpoint = |point: CellPoint| {
            let row = point.row.min(snapshot.lines.len() - 1);
            let cells = &snapshot.lines[row].cells;
            let col = cluster_start(cells, point.col);
            (
                row,
                col,
                col + cells.get(col).map_or(1, |cell| cell.width.max(1) as usize),
            )
        };
        let (ar, ac, ae) = endpoint(self.anchor);
        let (er, ec, ee) = endpoint(self.end);
        let ((first_row, first_col), (last_row, last_end)) = if (ar, ac) <= (er, ec) {
            ((ar, ac), (er, ee))
        } else {
            ((er, ec), (ar, ae))
        };
        let mut spans = Vec::new();
        for row in first_row..=last_row {
            let cells = &snapshot.lines[row].cells;
            let mut start = if self.rectangular {
                ac.min(ec)
            } else if row == first_row {
                first_col
            } else {
                0
            };
            let mut end = if self.rectangular {
                ae.max(ee)
            } else if row == last_row {
                last_end
            } else {
                cells.len()
            };
            match self.snap {
                SelectionSnap::Line if !self.rectangular => {
                    start = 0;
                    end = cells.len();
                }
                SelectionSnap::Word if !self.rectangular => {
                    if row == first_row {
                        start = word_bounds(cells, start).0;
                    }
                    if row == last_row {
                        end = word_bounds(cells, end.saturating_sub(1)).1;
                    }
                }
                _ => {}
            }
            if start < cells.len() {
                start = cluster_start(cells, start);
            }
            if end > 0 && end <= cells.len() {
                let last = cluster_start(cells, end - 1);
                end = last + cells[last].width.max(1) as usize;
            }
            spans.push(TerminalSelectionSpan {
                row,
                start: start.min(cells.len()),
                end: end.min(cells.len()),
            });
        }
        spans
    }

    fn text(&self, snapshot: &TerminalSnapshot) -> String {
        let spans = self.spans(snapshot);
        let mut result = String::new();
        for (index, span) in spans.iter().enumerate() {
            let source = &snapshot.lines[span.row];
            let mut line = String::new();
            for cell in &source.cells[span.start..span.end] {
                if cell.width != 0 {
                    line.push_str(&cell.text);
                }
            }
            let continues_soft_wrap =
                source.wrapped && span.end == source.cells.len() && index + 1 < spans.len();
            result.push_str(if self.rectangular || continues_soft_wrap {
                &line
            } else {
                line.trim_end_matches(' ')
            });
            if index + 1 < spans.len() && (self.rectangular || !source.wrapped) {
                result.push('\n');
            }
        }
        result
    }
}

fn word_bounds(cells: &[TerminalCell], col: usize) -> (usize, usize) {
    if cells.is_empty() {
        return (0, 0);
    }
    let col = cluster_start(cells, col);
    let class = |cell: &TerminalCell| {
        if cell.character.is_whitespace() {
            0
        } else if cell.character.is_alphanumeric() || cell.character == '_' {
            1
        } else {
            2
        }
    };
    let target = class(&cells[col]);
    let mut start = col;
    while start > 0 {
        let previous = cluster_start(cells, start - 1);
        if class(&cells[previous]) != target {
            break;
        }
        start = previous;
    }
    let mut end = col + cells[col].width.max(1) as usize;
    while end < cells.len() && class(&cells[end]) == target {
        end += cells[end].width.max(1) as usize;
    }
    (start, end.min(cells.len()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bedtime_ui2::themes;

    fn context() -> egui::Context {
        let context = egui::Context::default();
        context.set_fonts(bedtime_ui2::fonts::definitions());
        context
    }

    fn frame(
        view: &mut TerminalView,
        context: &egui::Context,
        snapshot: &Arc<TerminalSnapshot>,
        events: Vec<egui::Event>,
        focused: bool,
        allow_input: bool,
        time: f64,
    ) -> (ViewOutput, egui::FullOutput) {
        let mut result = None;
        let mut output = context.run_ui(
            egui::RawInput {
                time: Some(time),
                events,
                focused: true,
                ..Default::default()
            },
            |ui| {
                ui.set_min_size(vec2(300.0, 160.0));
                result = Some(view.show(ui, snapshot, &themes::dark(), focused, allow_input, &[]));
            },
        );
        // These are CPU interaction/layout tests, not a renderer. Explicitly
        // retire font texture deltas rather than violating epaint's ownership
        // assertion when a frame's FullOutput is dropped.
        output.textures_delta.clear();
        (result.unwrap(), output)
    }

    #[test]
    fn immutable_rows_reuse_cluster_galleys_across_cursor_blink_and_selection_changes() {
        let context = context();
        let mut terminal = Terminal::new_text_only(20, 4);
        terminal.feed("A界e\u{301}".as_bytes());
        let snapshot = terminal.snapshot();
        let mut view = TerminalView::default();
        frame(&mut view, &context, &snapshot, vec![], true, true, 0.0);
        let prepared = view.prepared_rows;
        let row = &view.rows[&(Arc::as_ptr(&snapshot.lines[0]) as usize)];
        let wide = row
            .clusters
            .iter()
            .find(|cluster| cluster.col == 1)
            .unwrap();
        assert_eq!(wide.width, 2);
        let combining = row
            .clusters
            .iter()
            .find(|cluster| cluster.col == 3)
            .unwrap();
        assert_eq!(combining.galley.as_ref().unwrap().job.text, "e\u{301}");
        let galley = combining.galley.as_ref().unwrap().clone();
        let mut changed = (*snapshot).clone();
        changed.cursor_revision += 1;
        changed.cursor.col = 1;
        changed.selection.push(TerminalSelectionSpan {
            row: 0,
            start: 1,
            end: 3,
        });
        let changed = Arc::new(changed);
        frame(&mut view, &context, &changed, vec![], true, true, 0.7);
        assert_eq!(view.prepared_rows, prepared);
        let cached = &view.rows[&(Arc::as_ptr(&snapshot.lines[0]) as usize)]
            .clusters
            .iter()
            .find(|cluster| cluster.col == 3)
            .unwrap()
            .galley;
        assert!(Arc::ptr_eq(cached.as_ref().unwrap(), &galley));
    }

    #[test]
    fn a_fresh_egui_session_does_not_reuse_old_font_atlas_galleys() {
        let mut terminal = Terminal::new_text_only(20, 4);
        terminal.feed(b"same font definitions, different atlas");
        let snapshot = terminal.snapshot();
        let mut view = TerminalView::default();
        let first = context();
        frame(&mut view, &first, &snapshot, vec![], true, true, 0.0);
        let prepared = view.prepared_rows;
        let second = context();
        frame(&mut view, &second, &snapshot, vec![], true, true, 0.1);
        assert_eq!(view.prepared_rows, prepared + snapshot.lines.len());
    }

    #[test]
    fn in_place_font_atlas_resets_refresh_uvs_then_resume_row_cache_reuse() {
        for fill_atlas in [false, true] {
            let context = context();
            let mut terminal = Terminal::new_text_only(20, 4);
            terminal.feed(b"Cached atlas glyphs");
            let snapshot = terminal.snapshot();
            let mut view = TerminalView::default();
            let mut first = context.run_ui(
                egui::RawInput {
                    max_texture_side: Some(1024),
                    time: Some(0.0),
                    focused: true,
                    ..Default::default()
                },
                |ui| {
                    let _ = view.show(ui, &snapshot, &themes::dark(), true, true, &[]);
                },
            );
            first.textures_delta.clear();
            let definitions = context.fonts(|fonts| fonts.definitions().clone());
            let density = context.pixels_per_point();
            let cell = view.paint_key.as_ref().unwrap().cell;
            if fill_atlas {
                let mut fill_ratio = 0.0;
                let mut filled = context.run_ui(
                    egui::RawInput {
                        time: Some(0.1),
                        focused: true,
                        ..Default::default()
                    },
                    |ui| {
                        // Touch the probe in this pass too, so the next change is
                        // an actual atlas reset, not ordinary galley-cache eviction.
                        let _ = view.show(ui, &snapshot, &themes::dark(), true, true, &[]);
                        ui.ctx().fonts_mut(|fonts| {
                            for step in 0..96 {
                                if fonts.font_atlas_fill_ratio() > 0.8 {
                                    break;
                                }
                                let _ = fonts.layout_no_wrap(
                                    "WwMmi0".into(),
                                    FontId::monospace(96.0 + step as f32 * 4.0),
                                    Color32::WHITE,
                                );
                            }
                            fill_ratio = fonts.font_atlas_fill_ratio();
                        });
                    },
                );
                filled.textures_delta.clear();
                assert!(
                    fill_ratio > 0.8,
                    "exercise Fonts::begin_pass's full-atlas reset"
                );
            } else {
                context.global_style_mut(|style| {
                    style.visuals.text_options.font_hinting =
                        !style.visuals.text_options.font_hinting;
                });
            }
            let prepared = view.prepared_rows;
            let distinct_rows = view.rows.len();
            let previous_probe = view.atlas_probe.as_ref().unwrap().clone();
            let previous_galley = view.rows[&(Arc::as_ptr(&snapshot.lines[0]) as usize)].clusters
                [0]
            .galley
            .as_ref()
            .unwrap()
            .clone();
            frame(&mut view, &context, &snapshot, vec![], true, true, 0.2);
            assert_eq!(
                context.fonts(|fonts| fonts.definitions().clone()),
                definitions
            );
            assert_eq!(context.pixels_per_point(), density);
            assert_eq!(view.paint_key.as_ref().unwrap().cell, cell);
            assert_eq!(view.prepared_rows, prepared + distinct_rows);
            assert!(!Arc::ptr_eq(
                view.atlas_probe.as_ref().unwrap(),
                &previous_probe
            ));
            let refreshed = view.rows[&(Arc::as_ptr(&snapshot.lines[0]) as usize)].clusters[0]
                .galley
                .as_ref()
                .unwrap()
                .clone();
            assert!(!Arc::ptr_eq(&refreshed, &previous_galley));
            let prepared_after_reset = view.prepared_rows;
            frame(&mut view, &context, &snapshot, vec![], true, true, 0.9);
            assert_eq!(view.prepared_rows, prepared_after_reset);
            assert!(Arc::ptr_eq(
                view.rows[&(Arc::as_ptr(&snapshot.lines[0]) as usize)].clusters[0]
                    .galley
                    .as_ref()
                    .unwrap(),
                &refreshed
            ));
        }
    }

    #[test]
    fn real_bold_italic_and_faint_text_stay_on_the_pixel_aligned_grid() {
        for density in [1.0, 1.25, 2.0] {
            let context = context();
            context.set_pixels_per_point(density);
            let mut terminal = Terminal::new_text_only(20, 4);
            terminal.feed(b"M\x1b[1mM\x1b[3mM\x1b[2mM\x1b[0m");
            let snapshot = terminal.snapshot();
            let mut view = TerminalView::default();
            let mut output = context.run_ui(
                egui::RawInput {
                    focused: true,
                    ..Default::default()
                },
                |ui| {
                    // Exercise an off-pixel panel allocation, as seen during
                    // layout animation and nonintegral display scaling.
                    let mut child = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(Rect::from_min_size(pos2(0.3, 0.7), vec2(300.0, 160.0))),
                    );
                    view.show(&mut child, &snapshot, &themes::dark(), true, true, &[]);
                },
            );
            let text: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.job.text == "M" => {
                        Some((shape.clip_rect, text))
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(
                text.len(),
                4,
                "Bold draws one real glyph, not a second overstrike"
            );
            let font = FontId::monospace(themes::dark().metrics.font_size);
            assert_eq!(text[0].1.galley.job.sections[0].format.font_id, font);
            assert_eq!(
                text[1].1.galley.job.sections[0].format.font_id,
                bedtime_ui2::fonts::monospace_bold(font.size)
            );
            assert!(text[2].1.galley.job.sections[0].format.italics);
            assert_eq!(
                text[3].1.galley.job.sections[0].format.font_id, font,
                "Faint suppresses bold weight as well as bold-to-bright color"
            );
            let cell = view.paint_key.as_ref().unwrap().cell;
            for (index, (clip, glyph)) in text.iter().enumerate() {
                for value in [
                    clip.left(),
                    clip.top(),
                    clip.right(),
                    clip.bottom(),
                    glyph.pos.x,
                    glyph.pos.y,
                ] {
                    assert!(
                        (value * density - (value * density).round()).abs() < 0.001,
                        "Grid/text origin must land on physical pixels: {value} at {density}"
                    );
                }
                assert!((clip.width() - cell.x).abs() < 0.001);
                assert!((clip.left() - text[0].0.left() - index as f32 * cell.x).abs() < 0.001);
            }
            output.textures_delta.clear();
        }
    }

    #[test]
    fn padded_canvas_sizes_and_clips_a_stale_larger_grid_to_its_content() {
        let context = context();
        let mut terminal = Terminal::new_text_only(80, 24);
        terminal.feed(b"\x1b[41m\x1b[2J\x1b[Hcontent");
        terminal.select_all();
        let snapshot = terminal.snapshot();
        let mut view = TerminalView::default();
        let canvas = Rect::from_min_size(pos2(20.0, 30.0), vec2(240.0, 120.0));
        let content = canvas.shrink2(PADDING);
        let mut size = None;
        let mut output = context.run_ui(egui::RawInput::default(), |ui| {
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(canvas));
            size = Some(
                view.show(&mut child, &snapshot, &themes::dark(), false, false, &[])
                    .size,
            );
        });
        output.textures_delta.clear();
        let cell = view.paint_key.as_ref().unwrap().cell;
        let size = size.unwrap();
        assert_eq!(size.num_cols, (content.width() / cell.x).floor() as u16);
        assert_eq!(size.num_lines, (content.height() / cell.y).floor() as u16);
        assert!(usize::from(size.num_cols) < snapshot.cols);
        assert!(usize::from(size.num_lines) < snapshot.rows);
        let mut painted_text = false;
        for shape in &output.shapes {
            match &shape.shape {
                // An explicit shell background may fill the padding. Otherwise
                // Workbench's single alpha-bearing panel fill shows through.
                egui::Shape::Rect(rect) if rect.rect == canvas => {}
                // Only the scroll indicator lives in the right-hand inset, and
                // its ends must still clear the panel's top/bottom corners.
                egui::Shape::Rect(rect) if rect.rect.left() >= content.right() => {
                    assert!(rect.rect.right() <= canvas.right());
                    assert!(rect.rect.top() >= content.top());
                    assert!(rect.rect.bottom() <= content.bottom());
                }
                egui::Shape::Rect(_) | egui::Shape::Text(_) => {
                    assert!(
                        content.contains_rect(shape.clip_rect),
                        "Old grid cells must not paint into the padding: content={content:?}, clip={:?}, bounds={:?}",
                        shape.clip_rect,
                        shape.shape.visual_bounding_rect()
                    );
                    painted_text |= matches!(shape.shape, egui::Shape::Text(_));
                }
                _ => {}
            }
        }
        assert!(painted_text);
    }

    #[test]
    fn default_background_inherits_the_host_once_and_opacity_edits_reuse_text() {
        let context = context();
        let mut theme = themes::dark();
        let mut terminal = Terminal::new_text_only(20, 4);
        terminal.set_theme(&crate::terminal_theme(&theme));
        terminal.feed(b"text");
        let snapshot = terminal.snapshot();
        let mut view = TerminalView::default();
        let canvas = Rect::from_min_size(pos2(20.0, 30.0), vec2(300.0, 160.0));
        let content = canvas.shrink2(PADDING);
        let mut prepared = None;
        for alpha in [1.0, 0.5, 0.0, 0.25] {
            theme.palette.panel_background[3] = alpha;
            let host_color = bedtime_ui2::theme::color(theme.palette.panel_background);
            let mut output = context.run_ui(egui::RawInput::default(), |ui| {
                // Match Workbench: the host paints its alpha-bearing surface
                // once before invoking the terminal content renderer.
                ui.painter().rect_filled(canvas, 0.0, host_color);
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(canvas));
                view.show(&mut child, &snapshot, &theme, false, false, &[]);
            });
            output.textures_delta.clear();
            let mut host_fills = 0;
            for shape in &output.shapes {
                match &shape.shape {
                    egui::Shape::Rect(rect) if rect.rect == canvas => {
                        host_fills += 1;
                        assert_eq!(
                            rect.fill, host_color,
                            "No opaque or repeated terminal background layer"
                        );
                    }
                    egui::Shape::Rect(rect) => assert!(
                        !rect.rect.intersects(content),
                        "Default cells must not cover the host's background"
                    ),
                    egui::Shape::Text(text) => assert_eq!(
                        text.override_text_color.unwrap().a(),
                        255,
                        "Panel opacity affects the background, not terminal glyphs"
                    ),
                    _ => {}
                }
            }
            assert!(host_fills <= 1, "Panel alpha must be applied exactly once");
            if alpha > 0.0 {
                assert_eq!(host_fills, 1);
            }
            if let Some(prepared) = prepared {
                assert_eq!(view.prepared_rows, prepared);
            }
            prepared = Some(view.prepared_rows);
        }
        // The worker may still hold the previous RGB after a theme edit. Host
        // ownership must not be inferred from equality with its stale palette.
        theme.palette.panel_background = [0.4, 0.3, 0.2, 0.5];
        let mut output = context.run_ui(egui::RawInput::default(), |ui| {
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(canvas));
            view.show(&mut child, &snapshot, &theme, false, false, &[]);
        });
        output.textures_delta.clear();
        assert!(!output.shapes.iter().any(|shape| matches!(&shape.shape,
            egui::Shape::Rect(rect) if rect.rect == canvas)));
        assert_eq!(Some(view.prepared_rows), prepared);
    }

    #[test]
    fn explicit_and_reversed_backgrounds_stay_opaque_even_when_rgb_matches_the_panel() {
        let context = context();
        let theme = themes::dark();
        let mut terminal = Terminal::new_text_only(20, 4);
        terminal.set_theme(&crate::terminal_theme(&theme));
        let [r, g, b] = terminal.palette()[DEFAULT_BG];
        terminal.feed(format!("N\x1b[41mR\x1b[0;48;2;{r};{g};{b}mT\x1b[0;7mI\x1b[0m").as_bytes());
        let snapshot = terminal.snapshot();
        let mut view = TerminalView::default();
        let (_, output) = frame(&mut view, &context, &snapshot, vec![], false, false, 0.0);
        for (symbol, expected) in [
            ("N", None),
            (
                "R",
                Some(resolve(TerminalColor::Indexed(1), &snapshot.palette)),
            ),
            ("T", Some(Color32::from_rgb(r, g, b))),
            (
                "I",
                Some(resolve(
                    TerminalColor::Indexed(DEFAULT_FG),
                    &snapshot.palette,
                )),
            ),
        ] {
            let cell = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.job.text == symbol => {
                        Some(shape.clip_rect)
                    }
                    _ => None,
                })
                .unwrap();
            let fill = output.shapes.iter().find_map(|shape| match &shape.shape {
                egui::Shape::Rect(rect) if rect.rect == cell => Some(rect.fill),
                _ => None,
            });
            assert_eq!(fill, expected, "Background for {symbol}");
        }
    }

    #[test]
    fn osc_background_override_and_snapshot_reverse_replace_the_host_until_reset() {
        let context = context();
        let mut terminal = Terminal::new_text_only(20, 4);
        terminal.set_theme(&crate::terminal_theme(&themes::dark()));
        let mut view = TerminalView::default();
        for (sequence, reverse, expected) in [
            (b"".as_slice(), false, None),
            (
                b"\x1b]11;#123456\x07",
                false,
                Some(Color32::from_rgb(0x12, 0x34, 0x56)),
            ),
            (b"\x1b]111\x07", false, None),
            (
                b"",
                true,
                Some(resolve(
                    TerminalColor::Indexed(DEFAULT_FG),
                    terminal.palette(),
                )),
            ),
            (b"", false, None),
        ] {
            terminal.feed(sequence);
            let mut snapshot = terminal.snapshot();
            // The shared core does not currently report DEC private mode 5.
            // Exercise this frontend's snapshot flag, not an unsupported CSI.
            Arc::make_mut(&mut snapshot).modes.reverse = reverse;
            let (_, output) = frame(&mut view, &context, &snapshot, vec![], false, false, 0.0);
            // The canvas fill is the only rectangle with the canvas's clip and
            // bounds. Cell/reverse backgrounds remain clipped to the inset.
            let fill = output.shapes.iter().find_map(|shape| match &shape.shape {
                egui::Shape::Rect(rect) if rect.rect == shape.clip_rect => Some(rect.fill),
                _ => None,
            });
            assert_eq!(fill, expected);
        }
    }

    #[test]
    fn selection_theme_changes_repaint_without_preparing_unchanged_text() {
        let context = context();
        let mut terminal = Terminal::new_text_only(20, 4);
        terminal.feed(b"selected");
        terminal.select_start(0, 0, SelectionSnap::Word);
        let snapshot = terminal.snapshot();
        let mut view = TerminalView::default();
        frame(&mut view, &context, &snapshot, vec![], true, true, 0.0);
        let prepared = view.prepared_rows;
        let mut theme = themes::dark();
        theme.palette.selection = [0.2, 0.3, 0.6, 1.0];
        theme.palette.text = [1.0, 1.0, 1.0, 1.0];
        let mut output = context.run_ui(egui::RawInput::default(), |ui| {
            view.show(ui, &snapshot, &theme, true, true, &[]);
        });
        assert_eq!(view.prepared_rows, prepared);
        assert!(output.shapes.iter().any(|shape| matches!(
            &shape.shape, egui::Shape::Rect(rect) if rect.fill == bedtime_ui2::theme::color(theme.palette.selection)
        )));
        assert!(output.shapes.iter().any(|shape| matches!(
            &shape.shape, egui::Shape::Text(text) if text.galley.job.text == "s" && text.override_text_color == Some(Color32::WHITE)
        )));
        output.textures_delta.clear();
    }

    #[test]
    fn frozen_selection_copies_whole_wide_combining_clusters_and_respects_soft_wraps() {
        let mut terminal = Terminal::new_text_only(6, 3);
        terminal.feed("a界e\u{301}xyZ".as_bytes());
        let snapshot = terminal.snapshot();
        let selection = LocalSelection {
            anchor: CellPoint { row: 0, col: 2 }, // continuation half of 界
            end: CellPoint { row: 1, col: 0 },
            snap: SelectionSnap::None,
            rectangular: false,
        };
        assert_eq!(selection.text(&snapshot), "界e\u{301}xyZ");
        let word = LocalSelection {
            anchor: CellPoint { row: 0, col: 3 },
            end: CellPoint { row: 0, col: 3 },
            snap: SelectionSnap::Word,
            rectangular: false,
        };
        assert_eq!(word.spans(&snapshot)[0].start, 0);
    }

    #[test]
    fn colors_resolve_truecolor_faint_and_reverse_without_touching_layout() {
        let mut terminal = Terminal::new_text_only(4, 1);
        let snapshot = terminal.snapshot();
        let mut cell = snapshot.lines[0].cells[0].clone();
        cell.fg = TerminalColor::Rgb([200, 100, 40]);
        cell.bg = TerminalColor::Rgb([20, 40, 60]);
        assert_eq!(
            cell_colors(&cell, &snapshot),
            (
                Color32::from_rgb(200, 100, 40),
                Color32::from_rgb(20, 40, 60)
            )
        );
        cell.mode = ATTR_FAINT | ATTR_REVERSE;
        assert_eq!(
            cell_colors(&cell, &snapshot),
            (
                Color32::from_rgb(20, 40, 60),
                Color32::from_rgb(110, 70, 50)
            )
        );
    }

    #[test]
    fn pointer_drag_keeps_local_selection_owner_outside_until_release_and_can_copy_after_exit() {
        let context = context();
        let mut terminal = Terminal::new_text_only(20, 4);
        terminal.feed(b"copy this transcript");
        let snapshot = terminal.snapshot();
        let mut view = TerminalView::default();
        frame(&mut view, &context, &snapshot, vec![], true, false, 0.0);
        let position = pos2(5.0, 5.0);
        let press = egui::Event::PointerButton {
            pos: position,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        };
        let (pressed, _) = frame(
            &mut view,
            &context,
            &snapshot,
            vec![egui::Event::PointerMoved(position), press],
            true,
            false,
            0.1,
        );
        assert!(pressed.inputs.iter().any(|input| matches!(
            input,
            TerminalInput::Command(TerminalCommand::SelectionStart { .. })
        )));
        let outside = pos2(10000.0, 10000.0);
        let (dragged, _) = frame(
            &mut view,
            &context,
            &snapshot,
            vec![egui::Event::PointerMoved(outside)],
            true,
            false,
            0.2,
        );
        assert!(dragged.inputs.iter().any(|input| matches!(
            input,
            TerminalInput::Command(TerminalCommand::SelectionExtend { done: false, .. })
        )));
        let release = egui::Event::PointerButton {
            pos: outside,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        };
        let (released, _) = frame(
            &mut view,
            &context,
            &snapshot,
            vec![release],
            true,
            false,
            0.3,
        );
        assert!(released.inputs.iter().any(|input| matches!(
            input,
            TerminalInput::Command(TerminalCommand::SelectionExtend { done: true, .. })
        )));
        assert_eq!(view.gestures[0], Gesture::None);
        let (_, copied) = frame(
            &mut view,
            &context,
            &snapshot,
            vec![egui::Event::Copy],
            true,
            false,
            0.4,
        );
        assert!(copied.platform_output.commands.iter().any(|command| matches!(command, egui::OutputCommand::CopyText(text) if text.starts_with("copy this transcript"))));
    }

    #[test]
    fn unfocused_and_popup_frames_do_not_consume_or_forward_keyboard_input() {
        for popup_before_canvas in [true, false] {
            let context = context();
            let mut terminal = Terminal::new_text_only(20, 4);
            let snapshot = terminal.snapshot();
            let mut view = TerminalView::default();
            let text = egui::Event::Text("must not reach PTY".into());
            let (output, _) = frame(
                &mut view,
                &context,
                &snapshot,
                vec![text.clone()],
                false,
                true,
                0.0,
            );
            assert!(output.inputs.is_empty());
            assert_eq!(context.input(|input| input.events.clone()), [text.clone()]);
            frame(&mut view, &context, &snapshot, vec![], true, true, 0.1);

            let mut result = None;
            let mut popup_drawn = false;
            let mut drawn_before_canvas = false;
            let mut output = context.run_ui(
                egui::RawInput {
                    time: Some(0.2),
                    events: vec![text.clone()],
                    focused: true,
                    ..Default::default()
                },
                |ui| {
                    ui.set_min_size(vec2(300.0, 160.0));
                    let popup_id = ui.id().with("terminal_test_popup");
                    egui::Popup::open_id(ui.ctx(), popup_id);
                    let mut draw_popup = |ui: &mut egui::Ui| {
                        popup_drawn = egui::Popup::new(
                            popup_id,
                            ui.ctx().clone(),
                            pos2(350.0, 10.0),
                            ui.layer_id(),
                        )
                        .open_memory(None::<egui::SetOpenCommand>)
                        .show(|ui| {
                            ui.label("An actual open popup");
                        })
                        .is_some();
                    };
                    if popup_before_canvas {
                        draw_popup(ui);
                    }
                    drawn_before_canvas = ui.ctx().any_popup_open();
                    result = Some(view.show(ui, &snapshot, &themes::dark(), true, true, &[]));
                    if !popup_before_canvas {
                        draw_popup(ui);
                    }
                },
            );
            output.textures_delta.clear();
            assert!(popup_drawn);
            assert_eq!(drawn_before_canvas, popup_before_canvas);
            assert!(
                !result
                    .unwrap()
                    .inputs
                    .iter()
                    .any(|input| matches!(input, TerminalInput::Command(TerminalCommand::Text(_))))
            );
            assert_eq!(context.input(|input| input.events.clone()), [text]);
        }
    }

    #[test]
    fn application_mouse_drag_retains_its_press_owner_across_shift_and_outside_release() {
        let context = context();
        let mut terminal = Terminal::new_text_only(20, 4);
        terminal.feed(b"\x1b[?1002h\x1b[?1006h");
        let snapshot = terminal.snapshot();
        let mut view = TerminalView::default();
        frame(&mut view, &context, &snapshot, vec![], true, true, 0.0);
        let inside = pos2(5.0, 5.0);
        let (press, _) = frame(
            &mut view,
            &context,
            &snapshot,
            vec![
                egui::Event::PointerMoved(inside),
                egui::Event::PointerButton {
                    pos: inside,
                    button: PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            true,
            true,
            0.1,
        );
        assert!(press.inputs.iter().any(
            |input| matches!(input, TerminalInput::Bytes(bytes) if bytes.starts_with(b"\x1b[<0;"))
        ));
        let outside = pos2(10000.0, 10000.0);
        let (motion, _) = frame(
            &mut view,
            &context,
            &snapshot,
            vec![
                egui::Event::ModifiersChanged(egui::Modifiers::SHIFT),
                egui::Event::PointerMoved(outside),
            ],
            true,
            true,
            0.2,
        );
        assert!(motion.inputs.iter().any(
            |input| matches!(input, TerminalInput::Bytes(bytes) if bytes == b"\x1b[<36;20;4M")
        ));
        let (release, _) = frame(
            &mut view,
            &context,
            &snapshot,
            vec![egui::Event::PointerButton {
                pos: outside,
                button: PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::SHIFT,
            }],
            true,
            true,
            0.3,
        );
        assert!(release.inputs.iter().any(
            |input| matches!(input, TerminalInput::Bytes(bytes) if bytes == b"\x1b[<4;20;4m")
        ));
        assert!(!release.inputs.iter().any(|input| matches!(
            input,
            TerminalInput::Command(TerminalCommand::SelectionExtend { .. })
        )));
        let (local, _) = frame(
            &mut view,
            &context,
            &snapshot,
            vec![
                egui::Event::PointerMoved(inside),
                egui::Event::PointerButton {
                    pos: inside,
                    button: PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::SHIFT,
                },
            ],
            true,
            true,
            0.4,
        );
        assert!(local.inputs.iter().any(|input| matches!(
            input,
            TerminalInput::Command(TerminalCommand::SelectionStart { .. })
        )));
        assert!(
            !local
                .inputs
                .iter()
                .any(|input| matches!(input, TerminalInput::Bytes(_)))
        );
    }

    #[test]
    fn hidden_panel_suspension_finishes_owned_input_once() {
        let context = context();
        let mut terminal = Terminal::new_text_only(20, 4);
        let snapshot = terminal.snapshot();
        let mut view = TerminalView::default();
        frame(&mut view, &context, &snapshot, vec![], true, true, 0.0);
        frame(
            &mut view,
            &context,
            &snapshot,
            vec![egui::Event::Key {
                key: egui::Key::ArrowUp,
                physical_key: Some(egui::Key::ArrowUp),
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            true,
            true,
            0.1,
        );
        let cleanup = view.suspend();
        assert!(cleanup.iter().any(|input| matches!(input, TerminalInput::Command(TerminalCommand::Key(event)) if event.key == TerminalKey::Up && event.state == bedtime_terminal::terminal_input::KeyState::Release)));
        assert!(
            cleanup.iter().any(|input| matches!(
                input,
                TerminalInput::Command(TerminalCommand::Focus(false))
            ))
        );
        assert!(view.suspend().is_empty());
    }

    #[test]
    fn pixel_to_cell_and_fractional_scroll_are_bounded() {
        let mut terminal = Terminal::new_text_only(8, 3);
        let snapshot = terminal.snapshot();
        assert_eq!(
            cell_at(pos2(-100.0, 400.0), Pos2::ZERO, vec2(8.0, 16.0), &snapshot),
            CellPoint { col: 0, row: 2 }
        );
        let mut remainder = 0.0;
        assert_eq!(wheel_steps(&mut remainder, 0.4), 0);
        assert_eq!(wheel_steps(&mut remainder, 0.7), 1);
        assert!((remainder - 0.1).abs() < 0.001);
        assert_eq!(wheel_steps(&mut remainder, 10000.0), 32);
        assert_eq!(wheel_steps(&mut remainder, 0.0), 0);
    }
}
