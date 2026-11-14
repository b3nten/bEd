//! A grid of values: rows it's given, under their columns' names, with the
//! names and the row numbers staying put while the rest scrolls. Only what's
//! on screen is drawn. Click a cell to select it (arrows move, Copy copies);
//! click a name to sort by it (again to reverse, a third time to stop), drag
//! its edge to resize. Where cells can be changed, double-click, Enter, F2 or
//! typing edits the selected one (Enter keeps it and moves down, Tab right,
//! Escape doesn't keep it), Paste puts text in it, and the right-click menu
//! makes it NULL or deletes its row. A row being made shows after the others.
//!
//! The grid keeps only how it looks (widths, the selected cell, what's being
//! typed); the rows are the caller's, so sorting and every change are asked
//! of it.
use crate::db::{Input, Value};
use bedtime_ui::{UiContext, egui, theme};

const MIN_WIDTH: f32 = 48.0;
/// As much of a value as a cell lays out.
const SHOWN_CHARS: usize = 200;

/// The rows to draw.
pub struct Rows<'a> {
    pub columns: &'a [String],
    pub rows: &'a [Vec<Value>],
    /// The number of the first row, from 0 (rows before it are on other pages).
    pub first_row: u64,
    /// The column they're sorted by, and whether that's descending.
    pub sort: Option<(usize, bool)>,
    /// Which columns' cells can be changed; `None` if nothing can.
    pub editable: Option<&'a [bool]>,
    /// A row being made, shown after the others: what each column has been
    /// given so far.
    pub draft: Option<&'a [Option<Input>]>,
}

impl Rows<'_> {
    /// The rows, and the one being made.
    fn count(&self) -> usize {
        self.rows.len() + usize::from(self.draft.is_some())
    }

    fn cell(&self, row: usize, column: usize) -> Option<Cell> {
        let can = self
            .editable
            .is_some_and(|editable| editable.get(column).copied().unwrap_or(false));
        if let Some(values) = self.rows.get(row) {
            let value = values.get(column)?;
            let edit = match value {
                Value::Blob(_) => None,
                Value::Null => Some(String::new()),
                value => Some(value.text()),
            };
            return Some(Cell {
                text: value.text(),
                dim: value.is_placeholder(),
                number: value.is_number(),
                edit: edit.filter(|_| can),
            });
        }
        let (text, dim) = match self.draft?.get(column)? {
            None if can => ("default".to_owned(), true),
            // Worked out from the others.
            None => (String::new(), true),
            Some(Input::Null) => ("NULL".to_owned(), true),
            Some(Input::Text(text)) => (text.clone(), false),
        };
        Some(Cell {
            edit: can.then(|| if dim { String::new() } else { text.clone() }),
            text,
            dim,
            number: false,
        })
    }
}

/// A cell as drawn.
struct Cell {
    text: String,
    /// Not a value of its own: NULL, bytes, a default.
    dim: bool,
    number: bool,
    /// The text editing starts from; `None` if it can't be edited.
    edit: Option<String>,
}

/// What was asked for. Rows are counted as given; the one being made is the
/// one after the last.
#[derive(Clone, Debug, PartialEq)]
pub enum Asked {
    /// Sort by this column (descending if set), or stop sorting.
    Sort(Option<(usize, bool)>),
    /// Put this in a cell.
    Set {
        row: usize,
        column: usize,
        input: Input,
    },
    /// Delete a row.
    Delete { row: usize },
}

/// How a grid looks.
#[derive(Default)]
pub struct Grid {
    /// The columns `widths` were worked out for.
    sized_for: Vec<String>,
    /// Widths from the contents, and those dragged.
    widths: Vec<f32>,
    dragged: Vec<Option<f32>>,
    /// The selected cell: a row of those given, a column.
    cursor: Option<(usize, usize)>,
    editing: Option<Editing>,
    scroll_to_cursor: bool,
    to_top: bool,
    /// The rows changed: columns may need more room.
    refit: bool,
    /// After an edit: move the cursor (rows down, columns right).
    advance: Option<(isize, isize)>,
}

struct Editing {
    row: usize,
    column: usize,
    text: String,
    /// What it started as: the same when done means nothing is asked.
    original: String,
    started: bool,
}

impl Grid {
    /// The selected cell: a row of those given, a column.
    pub fn cursor(&self) -> Option<(usize, usize)> {
        self.cursor
    }

    /// Select a cell, and bring it into view.
    pub fn select(&mut self, row: usize, column: usize) {
        self.cursor = Some((row, column));
        self.scroll_to_cursor = true;
    }

    /// The row a cell is being edited in.
    pub fn editing_row(&self) -> Option<usize> {
        self.editing.as_ref().map(|editing| editing.row)
    }

    /// Drop what's being typed: the rows moved under it.
    pub fn stop_editing(&mut self) {
        self.editing = None;
    }

    /// Other rows are coming: nothing selected, back at the top.
    pub fn rewind(&mut self) {
        self.cursor = None;
        self.editing = None;
        self.to_top = true;
        self.refit = true;
    }

    /// The rows changed: widen the columns whose values got longer. None
    /// gets narrower, so nothing jumps while paging.
    pub fn refit(&mut self) {
        self.refit = true;
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        context: &UiContext<'_>,
        id: egui::Id,
        rows: &Rows<'_>,
        focused: bool,
    ) -> Vec<Asked> {
        let mut asked = Vec::new();
        let columns = rows.columns.len();
        if columns == 0 {
            return asked;
        }
        let count = rows.count();
        let p = context.theme.palette.clone();
        let font = egui::TextStyle::Body.resolve(ui.style());
        let row_height = (font.size * 1.9).round();
        self.size(rows, font.size);
        if self
            .cursor
            .is_some_and(|(row, column)| row >= count || column >= columns)
        {
            self.cursor = None;
        }
        if self
            .editing
            .as_ref()
            .is_some_and(|editing| editing.row >= count || editing.column >= columns)
        {
            self.editing = None;
        }
        let widths: Vec<f32> = self
            .widths
            .iter()
            .zip(&self.dragged)
            .map(|(auto, dragged)| dragged.unwrap_or(*auto))
            .collect();
        let lefts: Vec<f32> = widths
            .iter()
            .scan(0.0, |x, width| {
                let left = *x;
                *x += width;
                Some(left)
            })
            .collect();
        let last_number = rows.first_row + rows.rows.len() as u64;
        let gutter = (last_number.to_string().len() as f32 * font.size * 0.62 + 24.0).max(40.0);
        let total = egui::vec2(
            gutter + widths.iter().sum::<f32>(),
            row_height * (count + 1) as f32,
        );

        // Keys, while the grid (not a text box) has focus.
        let typing = ui.ctx().egui_wants_keyboard_input();
        if focused && !typing && !egui::Popup::is_any_open(ui.ctx()) {
            self.keys(ui, rows, &mut asked);
        }

        let rect = ui.available_rect_before_wrap();
        let ink = theme::color(p.text);
        let muted = theme::color(p.muted);
        let line = theme::color(theme::mix(p.border, p.panel_background, 0.45));
        let stripe = theme::color(theme::mix(p.panel_background, p.text, 0.025));
        let making = theme::color(theme::mix(p.panel_background, p.accent, 0.12));
        let head_fill = theme::color(theme::mix(p.panel_background, p.text, 0.05));
        let accent = theme::color(p.accent);
        let selection = theme::color(p.selection);

        let mut scroll = egui::ScrollArea::both()
            .id_salt(id.with("scroll"))
            .auto_shrink(false);
        let offset = ui
            .ctx()
            .data(|d| d.get_temp::<egui::Vec2>(id.with("offset")))
            .unwrap_or_default();
        if std::mem::take(&mut self.to_top) {
            scroll = scroll.scroll_offset(egui::vec2(offset.x, 0.0));
        } else if std::mem::take(&mut self.scroll_to_cursor)
            && let Some((row, column)) = self.cursor
        {
            // Keep the cursor's cell in view.
            let y = row_height + row as f32 * row_height;
            let x = gutter + lefts[column];
            let mut target = offset;
            if y - row_height < offset.y {
                target.y = y - row_height;
            } else if y + row_height > offset.y + rect.height() - 8.0 {
                target.y = y + row_height - rect.height() + 8.0;
            }
            if x - gutter < offset.x {
                target.x = x - gutter;
            } else if x + widths[column] > offset.x + rect.width() {
                target.x = x + widths[column] - rect.width();
            }
            scroll = scroll.scroll_offset(target.max(egui::Vec2::ZERO));
        }
        let output = scroll.show_viewport(ui, |ui, viewport| {
            let (_, content) = ui.allocate_space(total.max(viewport.size()));
            let origin = content.min;
            let painter = ui.painter().clone();
            let text = |painter: &egui::Painter,
                        cell: egui::Rect,
                        value: &str,
                        color: egui::Color32,
                        right: bool| {
                let shown: String = value
                    .lines()
                    .next()
                    .unwrap_or("")
                    .chars()
                    .take(SHOWN_CHARS)
                    .collect();
                let galley = painter.layout_no_wrap(shown, font.clone(), color);
                let y = cell.center().y - galley.size().y / 2.0;
                let x = if right && galley.size().x < cell.width() - 16.0 {
                    cell.right() - 8.0 - galley.size().x
                } else {
                    cell.left() + 8.0
                };
                painter
                    .with_clip_rect(
                        cell.shrink2(egui::vec2(4.0, 0.0))
                            .intersect(painter.clip_rect()),
                    )
                    .galley(egui::pos2(x, y), galley, color);
            };
            // Visible rows and columns.
            let first = ((viewport.top() / row_height).floor().max(0.0) as usize).min(count);
            let last = (((viewport.bottom() - row_height) / row_height)
                .ceil()
                .max(0.0) as usize
                + 1)
            .min(count);
            let visible_columns: Vec<usize> = (0..columns)
                .filter(|&column| {
                    let x = gutter + lefts[column];
                    x + widths[column] >= viewport.left() + gutter && x <= viewport.right()
                })
                .collect();
            let gutter_left = origin.x + viewport.left();
            // The cells.
            for index in first..last {
                let drafting = index >= rows.rows.len();
                let y = origin.y + row_height + index as f32 * row_height;
                let band = egui::Rect::from_min_size(
                    egui::pos2(gutter_left, y),
                    egui::vec2(viewport.width(), row_height),
                );
                if drafting {
                    painter.rect_filled(band, 0.0, making);
                } else if index % 2 == 1 {
                    painter.rect_filled(band, 0.0, stripe);
                }
                for &column in &visible_columns {
                    let rect = egui::Rect::from_min_size(
                        egui::pos2(origin.x + gutter + lefts[column], y),
                        egui::vec2(widths[column], row_height),
                    );
                    let Some(cell) = rows.cell(index, column) else {
                        continue;
                    };
                    // Not under the row numbers or the names.
                    let response = ui.interact(
                        rect.intersect(egui::Rect::everything_right_of(gutter_left + gutter))
                            .intersect(egui::Rect::everything_below(
                                origin.y + viewport.top() + row_height,
                            )),
                        id.with((index, column)),
                        egui::Sense::click(),
                    );
                    response.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::WidgetType::Other, true, &cell.text)
                    });
                    let selected = self.cursor == Some((index, column));
                    if selected {
                        painter.rect_filled(rect, 0.0, selection);
                    }
                    painter.vline(rect.right(), rect.y_range(), egui::Stroke::new(1.0, line));
                    let editing = self
                        .editing
                        .as_ref()
                        .is_some_and(|e| e.row == index && e.column == column);
                    if editing {
                        self.edit_cell(ui, id, rect, &mut asked);
                    } else {
                        let color = if cell.dim { muted } else { ink };
                        text(&painter, rect, &cell.text, color, cell.number);
                    }
                    if selected {
                        painter.rect_stroke(
                            rect.shrink(1.0),
                            2.0,
                            egui::Stroke::new(1.5, accent),
                            egui::StrokeKind::Inside,
                        );
                    }
                    // No egui focus: it would hide the keys from `keys`.
                    if (response.clicked() || response.secondary_clicked()) && !editing {
                        self.commit(&mut asked);
                        self.cursor = Some((index, column));
                    }
                    // egui counts a double click soon after another click as a triple.
                    if (response.double_clicked() || response.triple_clicked())
                        && let Some(from) = &cell.edit
                    {
                        self.start_editing(index, column, from.clone(), None);
                    }
                    response.context_menu(|ui| {
                        if let Some(from) = &cell.edit {
                            if ui.button("Edit").clicked() {
                                self.start_editing(index, column, from.clone(), None);
                            }
                            if ui.button("Set to NULL").clicked() {
                                asked.push(Asked::Set {
                                    row: index,
                                    column,
                                    input: Input::Null,
                                });
                            }
                            ui.separator();
                        }
                        if ui.button("Copy").clicked() {
                            ui.ctx().copy_text(cell.text.clone());
                        }
                        if let Some(values) = rows.rows.get(index) {
                            if ui.button("Copy Row").clicked() {
                                ui.ctx().copy_text(row_text(values));
                            }
                            if rows.editable.is_some() {
                                ui.separator();
                                if ui.button("Delete Row").clicked() {
                                    asked.push(Asked::Delete { row: index });
                                }
                            }
                        }
                    });
                }
            }
            // The row numbers (from 1, counting earlier pages), stuck to the left.
            let gutter_rect = egui::Rect::from_min_size(
                egui::pos2(gutter_left, origin.y + viewport.top()),
                egui::vec2(gutter, viewport.height()),
            );
            painter.rect_filled(gutter_rect, 0.0, head_fill);
            for index in first..last {
                let y = origin.y + row_height + index as f32 * row_height;
                let cell = egui::Rect::from_min_size(
                    egui::pos2(gutter_rect.left(), y),
                    egui::vec2(gutter, row_height),
                );
                if index >= rows.rows.len() {
                    text(&painter, cell, "+", accent, true);
                } else {
                    let number = rows.first_row + index as u64 + 1;
                    text(&painter, cell, &number.to_string(), muted, true);
                }
            }
            painter.vline(
                gutter_rect.right(),
                gutter_rect.y_range(),
                egui::Stroke::new(1.0, line),
            );
            // The names, stuck to the top.
            let top = origin.y + viewport.top();
            let band = egui::Rect::from_min_size(
                egui::pos2(gutter_left, top),
                egui::vec2(viewport.width(), row_height),
            );
            painter.rect_filled(band, 0.0, head_fill);
            // Scrolled under the row numbers like the cells below.
            let names = painter.with_clip_rect(
                painter
                    .clip_rect()
                    .intersect(egui::Rect::everything_right_of(gutter_rect.right())),
            );
            for &column in &visible_columns {
                let cell = egui::Rect::from_min_size(
                    egui::pos2(origin.x + gutter + lefts[column], top),
                    egui::vec2(widths[column], row_height),
                );
                let shown = cell.intersect(egui::Rect::everything_right_of(gutter_rect.right()));
                if !shown.is_positive() {
                    continue;
                }
                let name = &rows.columns[column];
                let response =
                    ui.interact(shown, id.with(("header", column)), egui::Sense::click());
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::Button, true, name)
                });
                if response.hovered() {
                    names.rect_filled(
                        shown,
                        0.0,
                        theme::color(theme::mix(p.panel_background, p.text, 0.09)),
                    );
                }
                let sorted = rows.sort.filter(|(by, _)| *by == column);
                text(
                    &names,
                    cell,
                    name,
                    if sorted.is_some() { accent } else { ink },
                    false,
                );
                // A small triangle: up for ascending, down for descending.
                if let Some((_, descending)) = sorted {
                    let c = egui::pos2(cell.right() - 12.0, cell.center().y);
                    let (tip, base) = if descending { (4.0, -3.0) } else { (-4.0, 3.0) };
                    names.add(egui::Shape::convex_polygon(
                        vec![
                            egui::pos2(c.x, c.y + tip),
                            egui::pos2(c.x + 4.0, c.y + base),
                            egui::pos2(c.x - 4.0, c.y + base),
                        ],
                        accent,
                        egui::Stroke::NONE,
                    ));
                }
                names.vline(cell.right(), cell.y_range(), egui::Stroke::new(1.0, line));
                if response.clicked() {
                    self.commit(&mut asked);
                    asked.push(Asked::Sort(match sorted {
                        Some((_, false)) => Some((column, true)),
                        Some((_, true)) => None,
                        None => Some((column, false)),
                    }));
                }
                response.context_menu(|ui| {
                    if ui.button("Sort Ascending").clicked() {
                        asked.push(Asked::Sort(Some((column, false))));
                    }
                    if ui.button("Sort Descending").clicked() {
                        asked.push(Asked::Sort(Some((column, true))));
                    }
                    if rows.sort.is_some() && ui.button("Don't Sort").clicked() {
                        asked.push(Asked::Sort(None));
                    }
                    ui.separator();
                    if ui.button("Copy Name").clicked() {
                        ui.ctx().copy_text(name.clone());
                    }
                });
                // Drag the right edge to resize.
                let edge =
                    egui::Rect::from_center_size(cell.right_center(), egui::vec2(8.0, row_height));
                let handle = ui.interact(edge, id.with(("resize", column)), egui::Sense::drag());
                if handle.hovered() || handle.dragged() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeColumn);
                }
                if handle.dragged() {
                    let width = (widths[column] + handle.drag_delta().x).max(MIN_WIDTH);
                    self.dragged[column] = Some(width);
                }
                if handle.double_clicked() {
                    self.dragged[column] = None;
                }
            }
            painter.hline(band.x_range(), band.bottom(), egui::Stroke::new(1.0, line));
            let corner = egui::Rect::from_min_size(band.min, egui::vec2(gutter, row_height));
            painter.rect_filled(corner, 0.0, head_fill);
            if count == 0 {
                let below = egui::Rect::from_min_size(
                    egui::pos2(gutter_left + gutter, top + row_height),
                    egui::vec2(viewport.width() - gutter, row_height),
                );
                text(&painter, below, "No rows", muted, false);
            }
        });
        ui.ctx()
            .data_mut(|d| d.insert_temp(id.with("offset"), output.state.offset));
        if let Some((down, right)) = self.advance.take() {
            self.move_cursor(rows, down, right);
        }
        asked
    }

    /// Column widths, from what's in them (a sample). Other rows of the same
    /// columns only widen them, and keep what was dragged.
    fn size(&mut self, rows: &Rows<'_>, font_size: f32) {
        let same = self.sized_for == rows.columns;
        if same && !std::mem::take(&mut self.refit) {
            return;
        }
        self.refit = false;
        let widths = rows.columns.iter().enumerate().map(|(column, name)| {
            let sample = rows.rows.iter().take(200).map(|row| {
                row.get(column).map_or(0, |value| {
                    value.text().lines().next().unwrap_or("").chars().count()
                })
            });
            // The name, and room for the sorting triangle.
            let chars = sample.max().unwrap_or(0).max(name.chars().count() + 2) as f32;
            (chars * font_size * 0.6 + 24.0).clamp(64.0, 320.0)
        });
        if same {
            self.widths = widths
                .zip(&self.widths)
                .map(|(new, old)| new.max(*old))
                .collect();
        } else {
            self.widths = widths.collect();
            self.dragged = vec![None; rows.columns.len()];
            self.sized_for = rows.columns.to_vec();
        }
    }

    fn move_cursor(&mut self, rows: &Rows<'_>, down: isize, right: isize) {
        let (Some((row, column)), count) = (self.cursor, rows.count()) else {
            return;
        };
        if count == 0 {
            return;
        }
        let row = (row as isize + down).clamp(0, count as isize - 1) as usize;
        let column = (column as isize + right).clamp(0, rows.columns.len() as isize - 1) as usize;
        self.cursor = Some((row, column));
        self.scroll_to_cursor = true;
    }

    /// Edit a cell, from `from` or, typed over it, from `typed`.
    fn start_editing(&mut self, row: usize, column: usize, from: String, typed: Option<String>) {
        self.cursor = Some((row, column));
        self.editing = Some(Editing {
            row,
            column,
            text: typed.unwrap_or_else(|| from.clone()),
            original: from,
            started: false,
        });
    }

    /// Keep what's typed, if it's not what was there.
    fn commit(&mut self, asked: &mut Vec<Asked>) {
        if let Some(editing) = self.editing.take()
            && editing.text != editing.original
        {
            asked.push(Asked::Set {
                row: editing.row,
                column: editing.column,
                input: Input::Text(editing.text),
            });
        }
    }

    /// The box a cell is edited in. Enter keeps it (and moves down), Tab
    /// keeps it (and moves right), Escape doesn't.
    fn edit_cell(
        &mut self,
        ui: &mut egui::Ui,
        id: egui::Id,
        cell: egui::Rect,
        asked: &mut Vec<Asked>,
    ) {
        let Some(editing) = self.editing.as_mut() else {
            return;
        };
        let edit_id = id.with("edit");
        let response = ui.put(
            cell.shrink(1.0),
            egui::TextEdit::singleline(&mut editing.text)
                .id(edit_id)
                .margin(egui::Margin::symmetric(7, 4)),
        );
        if !editing.started {
            editing.started = true;
            response.request_focus();
            let end = editing.text.chars().count();
            if let Some(mut text_state) = egui::TextEdit::load_state(ui.ctx(), edit_id) {
                text_state
                    .cursor
                    .set_char_range(Some(egui::text::CCursorRange::one(
                        egui::text::CCursor::new(end),
                    )));
                text_state.store(ui.ctx(), edit_id);
            }
            return;
        }
        let (enter, tab, escape) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::Enter),
                i.key_pressed(egui::Key::Tab),
                i.key_pressed(egui::Key::Escape),
            )
        });
        if escape {
            self.editing = None;
        } else if response.lost_focus() {
            self.commit(asked);
            if enter {
                self.advance = Some((1, 0));
            } else if tab {
                self.advance = Some((0, 1));
            }
        }
    }

    /// Arrows move the selected cell; Enter, F2 or typing edits it; Copy
    /// copies it, Paste puts text in it; Escape lets it go.
    fn keys(&mut self, ui: &mut egui::Ui, rows: &Rows<'_>, asked: &mut Vec<Asked>) {
        if self.editing.is_some() || rows.count() == 0 {
            return;
        }
        let Some((row, column)) = self.cursor else {
            if ui.input(|i| i.key_pressed(egui::Key::ArrowDown)) {
                self.cursor = Some((0, 0));
                self.scroll_to_cursor = true;
            }
            return;
        };
        let cell = rows.cell(row, column);
        let edit = cell.as_ref().and_then(|cell| cell.edit.clone());
        let mut moved = None;
        let mut start: Option<Option<String>> = None;
        let mut copy = false;
        let mut paste = None;
        ui.input_mut(|i| {
            let step = |i: &mut egui::InputState, key| i.consume_key(egui::Modifiers::NONE, key);
            if step(i, egui::Key::ArrowDown) {
                moved = Some((1isize, 0isize));
            } else if step(i, egui::Key::ArrowUp) {
                moved = Some((-1, 0));
            } else if step(i, egui::Key::ArrowRight) || step(i, egui::Key::Tab) {
                moved = Some((0, 1));
            } else if step(i, egui::Key::ArrowLeft) {
                moved = Some((0, -1));
            } else if step(i, egui::Key::Escape) {
                self.cursor = None;
            } else if edit.is_some() && (step(i, egui::Key::Enter) || step(i, egui::Key::F2)) {
                start = Some(None);
            }
            for event in &i.events {
                match event {
                    egui::Event::Copy => copy = true,
                    egui::Event::Paste(text) if edit.is_some() => paste = Some(text.clone()),
                    egui::Event::Text(text)
                        if edit.is_some() && !i.modifiers.command && !i.modifiers.ctrl =>
                    {
                        // Several keys can come in one frame.
                        match &mut start {
                            Some(Some(typed)) => typed.push_str(text),
                            _ => start = Some(Some(text.clone())),
                        }
                    }
                    _ => {}
                }
            }
            i.events.retain(|event| {
                !matches!(
                    event,
                    egui::Event::Copy | egui::Event::Paste(_) | egui::Event::Text(_)
                )
            });
        });
        if let Some((down, right)) = moved {
            self.move_cursor(rows, down, right);
        }
        if let (Some(typed), Some(from)) = (start, edit) {
            self.start_editing(row, column, from, typed);
        }
        if copy && let Some(cell) = &cell {
            ui.ctx().copy_text(cell.text.clone());
        }
        if let Some(text) = paste {
            asked.push(Asked::Set {
                row,
                column,
                input: Input::Text(text),
            });
        }
    }
}

/// A row as text: its values with tabs between, as spreadsheets paste them.
fn row_text(row: &[Value]) -> String {
    row.iter()
        .map(|value| value.text().replace(['\t', '\n'], " "))
        .collect::<Vec<_>>()
        .join("\t")
}
