//! A table editor for workbench apps: CSV and TSV text as a grid. Click a
//! cell to select it; double-click, Enter or typing edits it (Enter keeps
//! it, Escape doesn't); arrows move, Delete clears. Click a column's header
//! to sort by it (again to reverse, a third time to stop), drag its edge to
//! resize. The floating filter shows only rows containing some text.
//! Sorting and filtering change only the view. The right-click menus insert
//! and delete rows and columns. The table is a view of a text document, the
//! one a text editor shows, so the two follow each other's edits; Undo, Redo
//! and Save work as for any file. Edits change only the text they need to.
use bedtime_document::TextBuffer;
use bedtime_ui::{BuiltinIcon, Floating, UiContext, egui, theme};
#[cfg(feature = "persistence")]
use bedtime_workbench::persistence::ReflectPersist;
use bedtime_workbench::{
    EditOptions, ManagedTask, ManagedWork, Notification, Panel, PanelPresentation, ViewersAppExt,
    WorkbenchWorldExt, viewer_document,
};
use bevy::prelude::*;
use std::time::Duration;

pub mod csv;
mod model;
mod view;
pub use model::{Table, TableChange};
pub use view::{Sort, compare, looks_like_header, shown_rows};

pub const SUPPORTED_EXTENSIONS: &[&str] = &["csv", "tsv", "tab"];

pub struct TablePlugin;

/// Tables in text: CSV and TSV (`SUPPORTED_EXTENSIONS`). Made for them, the
/// table viewer opens them; the editor is in "Open With".
pub const CSV: bedtime_workbench::FormatId = bedtime_workbench::FormatId("csv");

impl Plugin for TablePlugin {
    fn build(&self, app: &mut App) {
        use bedtime_workbench::{Format, FormatsAppExt, TEXT};
        app.register_type::<TablePanel>()
            .register_format(
                Format::new(CSV)
                    .extensions(SUPPORTED_EXTENSIONS.iter().copied())
                    .conforms_to([TEXT]),
            )
            .register_viewer::<TextBuffer, TablePanel>("Table")
            .formats([CSV]);
    }
}

#[bedtime_ui::panel]
#[derive(Reflect, Default)]
#[reflect(Component, Default)]
#[cfg_attr(feature = "persistence", reflect(Persist))]
pub struct TablePanel {
    /// Whether the first row names the columns; `None` guesses.
    pub header: Option<bool>,
    #[reflect(ignore)]
    state: State,
}

#[derive(Default)]
struct State {
    /// The table, and the text revision it was read from (or edited to).
    table: Option<(u64, Table)>,
    /// Reading a newer revision.
    parsing: Option<(u64, ManagedTask<Table>)>,
    sort: Option<Sort>,
    filter: String,
    filtering: bool,
    focus_filter: bool,
    /// The selected cell: a row of the table (not of the view), a column.
    cursor: Option<(usize, usize)>,
    editing: Option<Editing>,
    /// Widths worked out from the contents, and those dragged.
    widths: Option<(u64, Vec<f32>)>,
    dragged: Vec<Option<f32>>,
    order: Option<Order>,
    scroll_to_cursor: bool,
    /// Asked from a menu: whether the first row is the header.
    toggle_header: Option<bool>,
    /// After an edit: move the cursor (rows down, columns right) in the view.
    advance: Option<(isize, isize)>,
}

/// The rows shown, and what they were worked out for: revision, header,
/// filter and sort.
type Order = (u64, bool, String, Option<Sort>, Vec<usize>);

struct Editing {
    row: usize,
    column: usize,
    text: String,
    started: bool,
}

impl TablePanel {
    pub fn cursor(&self) -> Option<(usize, usize)> {
        self.state.cursor
    }
}

impl Panel for TablePanel {
    const LABEL: &'static str = "Table";

    fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, panel: Entity) {
        let Some(document) = viewer_document::<TextBuffer>(ui, world, panel) else {
            return;
        };
        update(world, panel, &document.model, document.revision);
        let focused = world
            .get::<PanelPresentation>(panel)
            .is_some_and(|presentation| presentation.focused);
        let Some(mut table_panel) = world.get_mut::<TablePanel>(panel) else {
            return;
        };
        if table_panel.state.parsing.is_some() {
            ui.ctx().request_repaint_after(Duration::from_millis(16));
        }
        let Some((read, mut table)) = table_panel.state.table.take() else {
            ui.centered_and_justified(|ui| ui.spinner());
            return;
        };
        let edits = draw(ui, context, panel, &mut table_panel, &table, read, focused);
        if edits.is_empty() {
            table_panel.state.table = Some((read, table));
            return;
        }
        // Edits are made to the text the table was read from; while a newer
        // revision is being read, they'd land in the wrong places.
        if read != document.revision {
            table_panel.state.table = Some((read, table));
            world.workbench_mut().notify(Notification::warning(
                "The table is still reading the latest text; try again",
            ));
            return;
        }
        let mut text = document.model;
        let mut revision = document.revision;
        for (change, label) in edits {
            let options = EditOptions {
                viewer: Some(panel),
                ..EditOptions::new(label)
            };
            let made = table.edit(&text, &change).and_then(|edit| {
                let next = world
                    .workbench_mut()
                    .edit_document(document.handle, revision, edit.clone(), options)
                    .map_err(|error| error.to_string())?;
                edit.apply(&mut text)?;
                Ok(next)
            });
            match made {
                Ok(next) => revision = next,
                Err(error) => {
                    world.workbench_mut().notify(Notification::warning(error));
                    // Read the text again, as it is.
                    revision = u64::MAX;
                    break;
                }
            }
        }
        if let Some(mut table_panel) = world.get_mut::<TablePanel>(panel) {
            table_panel.state.table = Some((revision, table));
        }
    }
}

/// Collect a finished read, and start one for a newer revision.
fn update(world: &mut World, panel: Entity, text: &TextBuffer, revision: u64) {
    let Some(mut table_panel) = world.get_mut::<TablePanel>(panel) else {
        return;
    };
    let state = &mut table_panel.state;
    if let Some((read, task)) = &state.parsing {
        match task.try_recv() {
            Ok(table) => {
                state.table = Some((*read, table));
                state.parsing = None;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => state.parsing = None,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
        }
    }
    if state.table.as_ref().is_some_and(|(read, _)| *read == revision) {
        return;
    }
    let text = text.clone();
    let task = world.resource_mut::<ManagedWork>().spawn_task(
        Some(panel),
        "Reading the table",
        move |_| Table::parse(&text.range_text(0..text.len())),
    );
    if let (Ok(task), Some(mut table_panel)) = (task, world.get_mut::<TablePanel>(panel)) {
        table_panel.state.parsing = Some((revision, task));
    }
}

type Edits = Vec<(TableChange, &'static str)>;

const MIN_WIDTH: f32 = 48.0;

#[allow(clippy::too_many_arguments)]
fn draw(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    panel: Entity,
    table_panel: &mut TablePanel,
    table: &Table,
    revision: u64,
    focused: bool,
) -> Edits {
    let id = egui::Id::new(("bedtime.table", panel));
    let mut edits = Edits::new();
    let header = table_panel
        .header
        .unwrap_or_else(|| looks_like_header(table));
    let state = &mut table_panel.state;
    let columns = table.columns().max(1);
    let p = context.theme.palette.clone();
    let font = egui::TextStyle::Body.resolve(ui.style());
    let row_height = (font.size * 1.9).round();

    // Which rows show, worked out again only when something changed.
    let stale = state.order.as_ref().is_none_or(|(at, h, filter, sort, _)| {
        *at != revision || *h != header || *filter != state.filter || *sort != state.sort
    });
    if stale {
        let rows = shown_rows(table, header, &state.filter, state.sort);
        state.order = Some((revision, header, state.filter.clone(), state.sort, rows));
    }
    let order = state.order.as_ref().unwrap().4.clone();

    // Column widths, from what's in them (a sample), unless dragged.
    if state
        .widths
        .as_ref()
        .is_none_or(|(at, widths)| *at != revision || widths.len() != columns)
    {
        let widths = (0..columns)
            .map(|column| {
                let sample = table.rows().iter().take(200).map(|row| {
                    row.get(column)
                        .map_or(0, |cell| cell.lines().next().unwrap_or("").chars().count())
                });
                let chars = sample.max().unwrap_or(0).max(3) as f32;
                (chars * font.size * 0.6 + 24.0).clamp(64.0, 320.0)
            })
            .collect();
        state.widths = Some((revision, widths));
    }
    state.dragged.resize(columns, None);
    let widths: Vec<f32> = state
        .widths
        .as_ref()
        .unwrap()
        .1
        .iter()
        .zip(&state.dragged)
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
    let gutter =
        (format!("{}", table.rows().len()).len() as f32 * font.size * 0.62 + 24.0).max(40.0);
    let total = egui::vec2(
        gutter + widths.iter().sum::<f32>(),
        row_height * (order.len() + usize::from(header)) as f32,
    );

    // Keys, while the table (not a text box) has focus.
    let typing = ui.ctx().egui_wants_keyboard_input();
    if focused && !typing && !egui::Popup::is_any_open(ui.ctx()) {
        keys(ui, state, table, &order, columns, &mut edits);
    }
    if focused && ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::F)) {
        state.filtering = true;
        state.focus_filter = true;
    }

    let rect = ui.available_rect_before_wrap();
    let ink = theme::color(p.text);
    let muted = theme::color(p.muted);
    let line = theme::color(theme::mix(p.border, p.panel_background, 0.45));
    let stripe = theme::color(theme::mix(p.panel_background, p.text, 0.025));
    let head_fill = theme::color(theme::mix(p.panel_background, p.text, 0.05));
    let accent = theme::color(p.accent);
    let selection = theme::color(p.selection);
    let header_top = usize::from(header) as f32 * row_height;

    let mut scroll = egui::ScrollArea::both()
        .id_salt(id.with("scroll"))
        .auto_shrink(false);
    if std::mem::take(&mut state.scroll_to_cursor)
        && let Some((row, column)) = state.cursor
        && let Some(index) = order.iter().position(|&r| r == row)
    {
        // Keep the cursor's cell in view.
        let y = header_top + index as f32 * row_height;
        let x = gutter + lefts[column.min(columns - 1)];
        let offset = ui
            .ctx()
            .data(|d| d.get_temp::<egui::Vec2>(id.with("offset")))
            .unwrap_or_default();
        let view = rect.size() - egui::vec2(gutter, header_top + row_height);
        let mut target = offset;
        if y - header_top < offset.y {
            target.y = y - header_top;
        } else if y + row_height > offset.y + rect.height() - 8.0 {
            target.y = y + row_height - rect.height() + 8.0;
        }
        let width = widths[column.min(columns - 1)];
        if x - gutter < offset.x {
            target.x = x - gutter;
        } else if x + width > offset.x + view.x + gutter {
            target.x = x + width - view.x - gutter;
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
            let shown = value.lines().next().unwrap_or("");
            let galley = painter.layout_no_wrap(shown.to_owned(), font.clone(), color);
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
        let numeric = |value: &str| value.trim().replace(',', "").parse::<f64>().is_ok();
        // Visible rows and columns.
        let first = ((viewport.top() / row_height).floor().max(0.0) as usize).min(order.len());
        let last = (((viewport.bottom() - header_top) / row_height)
            .ceil()
            .max(0.0) as usize
            + 1)
        .min(order.len());
        let visible_columns: Vec<usize> = (0..columns)
            .filter(|&column| {
                let x = gutter + lefts[column];
                x + widths[column] >= viewport.left() + gutter && x <= viewport.right()
            })
            .collect();
        let gutter_left = origin.x + viewport.left();
        // The cells.
        for (index, &row) in order.iter().enumerate().take(last).skip(first) {
            let y = origin.y + header_top + index as f32 * row_height;
            let band = egui::Rect::from_min_size(
                egui::pos2(origin.x + viewport.left(), y),
                egui::vec2(viewport.width(), row_height),
            );
            if index % 2 == 1 {
                painter.rect_filled(band, 0.0, stripe);
            }
            for &column in &visible_columns {
                let cell = egui::Rect::from_min_size(
                    egui::pos2(origin.x + gutter + lefts[column], y),
                    egui::vec2(widths[column], row_height),
                );
                let value = table.cell(row, column);
                // Not under the row numbers.
                let response = ui.interact(
                    cell.intersect(egui::Rect::everything_right_of(gutter_left + gutter)),
                    id.with((row, column)),
                    egui::Sense::click(),
                );
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::Other, true, value)
                });
                let selected = state.cursor == Some((row, column));
                if selected {
                    painter.rect_filled(cell, 0.0, selection);
                }
                painter.vline(cell.right(), cell.y_range(), egui::Stroke::new(1.0, line));
                let editing = state
                    .editing
                    .as_ref()
                    .is_some_and(|e| e.row == row && e.column == column);
                if editing {
                    edit_cell(ui, id, cell, state, &mut edits);
                } else {
                    text(&painter, cell, value, ink, numeric(value));
                }
                if selected {
                    painter.rect_stroke(
                        cell.shrink(1.0),
                        2.0,
                        egui::Stroke::new(1.5, accent),
                        egui::StrokeKind::Inside,
                    );
                }
                // No egui focus: it would hide the keys from `keys`.
                if response.clicked() || response.secondary_clicked() {
                    commit(state, &mut edits);
                    state.cursor = Some((row, column));
                }
                // egui counts a double click soon after another click as a triple.
                if response.double_clicked() || response.triple_clicked() {
                    start_editing(state, table, row, column, None);
                }
                response.context_menu(|ui| {
                    cell_menu(ui, state, table, row, column, columns, header, &mut edits)
                });
            }
        }
        // The row numbers (file rows, from 1), stuck to the left.
        let gutter_rect = egui::Rect::from_min_size(
            egui::pos2(origin.x + viewport.left(), origin.y + viewport.top()),
            egui::vec2(gutter, viewport.height()),
        );
        painter.rect_filled(gutter_rect, 0.0, head_fill);
        for (index, &row) in order.iter().enumerate().take(last).skip(first) {
            let y = origin.y + header_top + index as f32 * row_height;
            let cell = egui::Rect::from_min_size(
                egui::pos2(gutter_rect.left(), y),
                egui::vec2(gutter, row_height),
            );
            text(&painter, cell, &(row + 1).to_string(), muted, true);
        }
        painter.vline(
            gutter_rect.right(),
            gutter_rect.y_range(),
            egui::Stroke::new(1.0, line),
        );
        // The header, stuck to the top.
        if header {
            let top = origin.y + viewport.top();
            let band = egui::Rect::from_min_size(
                egui::pos2(origin.x + viewport.left(), top),
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
                let name = table.cell(0, column);
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
                let editing = state
                    .editing
                    .as_ref()
                    .is_some_and(|e| e.row == 0 && e.column == column);
                if editing {
                    edit_cell(ui, id, cell, state, &mut edits);
                } else {
                    let sorted = state.sort.filter(|sort| sort.column == column);
                    text(
                        &names,
                        cell,
                        name,
                        if sorted.is_some() { accent } else { ink },
                        false,
                    );
                    // A small triangle: up for ascending, down for descending.
                    if let Some(sort) = sorted {
                        let c = egui::pos2(cell.right() - 12.0, cell.center().y);
                        let (tip, base) = if sort.descending {
                            (4.0, -3.0)
                        } else {
                            (-4.0, 3.0)
                        };
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
                }
                names.vline(cell.right(), cell.y_range(), egui::Stroke::new(1.0, line));
                if response.clicked() {
                    state.sort = match state.sort {
                        Some(sort) if sort.column == column && !sort.descending => Some(Sort {
                            column,
                            descending: true,
                        }),
                        Some(sort) if sort.column == column => None,
                        _ => Some(Sort {
                            column,
                            descending: false,
                        }),
                    };
                }
                if response.double_clicked() || response.triple_clicked() {
                    start_editing(state, table, 0, column, None);
                }
                response.context_menu(|ui| header_menu(ui, state, table, column, &mut edits));
                // Drag the right edge to resize.
                let edge =
                    egui::Rect::from_center_size(cell.right_center(), egui::vec2(8.0, row_height));
                let handle = ui.interact(edge, id.with(("resize", column)), egui::Sense::drag());
                if handle.hovered() || handle.dragged() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeColumn);
                }
                if handle.dragged() {
                    let width = (widths[column] + handle.drag_delta().x).max(MIN_WIDTH);
                    state.dragged[column] = Some(width);
                }
                if handle.double_clicked() {
                    state.dragged[column] = None;
                }
            }
            painter.hline(band.x_range(), band.bottom(), egui::Stroke::new(1.0, line));
            let corner = egui::Rect::from_min_size(band.min, egui::vec2(gutter, row_height));
            painter.rect_filled(corner, 0.0, head_fill);
        }
    });
    ui.ctx()
        .data_mut(|d| d.insert_temp(id.with("offset"), output.state.offset));
    if let Some((down, right)) = state.advance.take() {
        move_cursor(state, &order, columns, down, right);
    }

    // Floating: the filter, and how many rows.
    let mut frame = context.frame(ui);
    let all = table.rows().len().saturating_sub(usize::from(header));
    let count = if order.len() == all {
        format!("{all} rows · {columns} columns")
    } else {
        format!("{} of {all} rows", order.len())
    };
    Floating::new(id.with("count"), rect, egui::Align2::RIGHT_BOTTOM)
        .shown(order.len() != all)
        .show(&mut frame, |f| {
            f.floating_label(count);
        });
    Floating::new(id.with("filter"), rect, egui::Align2::RIGHT_TOP)
        .shown(state.filtering || !state.filter.is_empty())
        .show(&mut frame, |f| {
            if state.filtering {
                let response = f.ui.add(
                    egui::TextEdit::singleline(&mut state.filter)
                        .id(id.with("filter-text"))
                        .hint_text("Filter rows")
                        .desired_width(180.0)
                        .frame(egui::Frame::NONE)
                        .margin(egui::Margin::symmetric(6, 3)),
                );
                if std::mem::take(&mut state.focus_filter) {
                    response.request_focus();
                }
                if response.has_focus() && f.ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    state.filtering = false;
                    state.filter.clear();
                }
                if f.floating_button(BuiltinIcon::Close, "Clear Filter (Escape)")
                    .clicked()
                {
                    state.filtering = false;
                    state.filter.clear();
                }
            } else if f
                .floating_toggle(
                    BuiltinIcon::Filter,
                    "Filter Rows (Cmd+F)",
                    !state.filter.is_empty(),
                )
                .clicked()
            {
                state.filtering = true;
                state.focus_filter = true;
            }
        });
    if let Some(header) = table_panel.state.toggle_header.take() {
        table_panel.header = Some(header);
    }
    edits
}

/// Move the cursor through the rows as shown (sorted, filtered).
fn move_cursor(state: &mut State, order: &[usize], columns: usize, down: isize, right: isize) {
    let Some((row, column)) = state.cursor else {
        return;
    };
    let index = order.iter().position(|&r| r == row).map_or(0, |index| {
        (index as isize + down).clamp(0, order.len() as isize - 1) as usize
    });
    let next_row = order.get(index).copied().unwrap_or(row);
    let next_column = (column as isize + right).clamp(0, columns as isize - 1) as usize;
    state.cursor = Some((next_row, next_column));
    state.scroll_to_cursor = true;
}

fn start_editing(
    state: &mut State,
    table: &Table,
    row: usize,
    column: usize,
    text: Option<String>,
) {
    state.cursor = Some((row, column));
    state.editing = Some(Editing {
        row,
        column,
        text: text.unwrap_or_else(|| table.cell(row, column).to_owned()),
        started: false,
    });
}

/// Keep what's typed.
fn commit(state: &mut State, edits: &mut Edits) {
    if let Some(editing) = state.editing.take() {
        edits.push((
            TableChange::SetCell {
                row: editing.row,
                column: editing.column,
                text: editing.text,
            },
            "Edit Cell",
        ));
    }
}

/// The box a cell is edited in. Enter keeps it (and moves down), Tab keeps
/// it (and moves right), Escape doesn't.
fn edit_cell(
    ui: &mut egui::Ui,
    id: egui::Id,
    cell: egui::Rect,
    state: &mut State,
    edits: &mut Edits,
) {
    let Some(editing) = state.editing.as_mut() else {
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
        state.editing = None;
    } else if response.lost_focus() {
        commit(state, edits);
        if enter {
            state.advance = Some((1, 0));
        } else if tab {
            state.advance = Some((0, 1));
        }
    }
}

/// Arrows, editing, clearing, copying and pasting the selected cell.
fn keys(
    ui: &mut egui::Ui,
    state: &mut State,
    table: &Table,
    order: &[usize],
    columns: usize,
    edits: &mut Edits,
) {
    if state.editing.is_some() {
        return;
    }
    let Some((row, column)) = state.cursor else {
        if !order.is_empty() && ui.input(|i| i.key_pressed(egui::Key::ArrowDown)) {
            state.cursor = Some((order[0], 0));
        }
        return;
    };
    let mut moved = None;
    let mut start = None;
    let mut clear = false;
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
        } else if step(i, egui::Key::Enter) || step(i, egui::Key::F2) {
            start = Some(None);
        } else if step(i, egui::Key::Delete) || step(i, egui::Key::Backspace) {
            clear = true;
        } else if step(i, egui::Key::Escape) {
            state.cursor = None;
        }
        for event in &i.events {
            match event {
                egui::Event::Copy => copy = true,
                egui::Event::Paste(text) => paste = Some(text.clone()),
                egui::Event::Text(text) if !i.modifiers.command && !i.modifiers.ctrl => {
                    start = Some(Some(text.clone()));
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
        move_cursor(state, order, columns, down, right);
    }
    if let Some(text) = start {
        start_editing(state, table, row, column, text);
    }
    if clear && !table.cell(row, column).is_empty() {
        edits.push((
            TableChange::SetCell {
                row,
                column,
                text: String::new(),
            },
            "Clear Cell",
        ));
    }
    if copy {
        ui.ctx().copy_text(table.cell(row, column).to_owned());
    }
    if let Some(text) = paste {
        edits.push((TableChange::SetCell { row, column, text }, "Paste"));
    }
}

#[allow(clippy::too_many_arguments)]
fn cell_menu(
    ui: &mut egui::Ui,
    state: &mut State,
    table: &Table,
    row: usize,
    column: usize,
    columns: usize,
    header: bool,
    edits: &mut Edits,
) {
    state.cursor = Some((row, column));
    let blank = vec![String::new(); columns];
    if ui.button("Edit").clicked() {
        start_editing(state, table, row, column, None);
    }
    if ui.button("Copy").clicked() {
        ui.ctx().copy_text(table.cell(row, column).to_owned());
    }
    if ui.button("Clear").clicked() {
        edits.push((
            TableChange::SetCell {
                row,
                column,
                text: String::new(),
            },
            "Clear Cell",
        ));
    }
    ui.separator();
    if ui.button("Insert Row Above").clicked() {
        edits.push((
            TableChange::InsertRow {
                at: row.max(usize::from(header)),
                cells: blank.clone(),
            },
            "Insert Row",
        ));
    }
    if ui.button("Insert Row Below").clicked() {
        edits.push((
            TableChange::InsertRow {
                at: row + 1,
                cells: blank,
            },
            "Insert Row",
        ));
    }
    if ui.button("Delete Row").clicked() {
        edits.push((TableChange::DeleteRow { at: row }, "Delete Row"));
        state.cursor = None;
    }
    ui.separator();
    column_items(ui, state, column, edits);
    ui.separator();
    header_item(ui, state, header);
}

/// Whether the first row names the columns.
fn header_item(ui: &mut egui::Ui, state: &mut State, header: bool) {
    let mut on = header;
    if ui.checkbox(&mut on, "First Row Is the Header").clicked() {
        state.toggle_header = Some(on);
    }
}

fn column_items(ui: &mut egui::Ui, state: &mut State, column: usize, edits: &mut Edits) {
    if ui.button("Sort Ascending").clicked() {
        state.sort = Some(Sort {
            column,
            descending: false,
        });
    }
    if ui.button("Sort Descending").clicked() {
        state.sort = Some(Sort {
            column,
            descending: true,
        });
    }
    if state.sort.is_some() && ui.button("Don't Sort").clicked() {
        state.sort = None;
    }
    ui.separator();
    if ui.button("Insert Column Left").clicked() {
        edits.push((
            TableChange::InsertColumn {
                at: column,
                cells: Vec::new(),
            },
            "Insert Column",
        ));
    }
    if ui.button("Insert Column Right").clicked() {
        edits.push((
            TableChange::InsertColumn {
                at: column + 1,
                cells: Vec::new(),
            },
            "Insert Column",
        ));
    }
    if ui.button("Delete Column").clicked() {
        edits.push((TableChange::DeleteColumn { at: column }, "Delete Column"));
        if state.sort.is_some_and(|sort| sort.column == column) {
            state.sort = None;
        }
    }
}

fn header_menu(
    ui: &mut egui::Ui,
    state: &mut State,
    table: &Table,
    column: usize,
    edits: &mut Edits,
) {
    if ui.button("Rename Column").clicked() {
        start_editing(state, table, 0, column, None);
    }
    ui.separator();
    column_items(ui, state, column, edits);
    ui.separator();
    header_item(ui, state, true);
}
