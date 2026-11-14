//! An SQLite viewer for workbench apps: a database file in a panel. Its
//! tables and views are listed down the side; the one selected shows its
//! rows in a grid, a page at a time, sorted and filtered by SQLite, or its
//! structure: columns with their types and constraints, indexes, triggers,
//! and the statement that made it. Query runs SQL typed into it, and shows
//! the rows that gives. A table's cells can be changed in the grid, and rows
//! added and deleted; each change is written to the file at once. The file isn't read into memory: the
//! panel keeps a connection to it and asks SQLite, on a task. Changes other
//! programs make to the database show within a second.
use bedtime_document::DocumentModel;
use bedtime_filesystem::{ByteModel, FileModel};
use bedtime_ui::{BuiltinIcon, CodeView, UiContext, egui, theme, tree};
#[cfg(feature = "persistence")]
use bedtime_workbench::persistence::ReflectPersist;
use bedtime_workbench::{
    ManagedTask, ManagedWork, Panel, PanelPresentation, ViewerFile, ViewersAppExt, viewer_document,
};
use bevy::prelude::*;
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, PoisonError, mpsc::TryRecvError},
    time::Duration,
};

pub mod db;
pub mod grid;
pub mod status;
pub use db::{
    Browse, Column, Database, Edit, ForeignKey, Index, Input, Interrupter, Kind, Object, Outcome,
    PAGE_ROWS, Page, RESULT_ROWS, RowKey, RowKeys, Schema, Sort, Value,
};
pub use grid::Grid;

/// The extensions SQLite databases usually have. A file with another one
/// opens too, if it starts as a database does.
pub const SUPPORTED_EXTENSIONS: &[&str] = &["db", "sqlite", "sqlite3", "db3", "s3db", "sl3"];

/// SQLite databases: `SUPPORTED_EXTENSIONS`, or any file starting with
/// SQLite's header.
pub const SQLITE: bedtime_workbench::FormatId = bedtime_workbench::FormatId("sqlite");

pub struct SqlitePlugin;

impl Plugin for SqlitePlugin {
    fn build(&self, app: &mut App) {
        use bedtime_workbench::{Format, FormatsAppExt};
        app.register_type::<SqlitePanel>()
            .register_type::<Mode>()
            .register_format(Format::new(SQLITE).extensions(SUPPORTED_EXTENSIONS.iter().copied()))
            .register_format_detector(|path| db::is_database(path).then_some(SQLITE))
            .register_viewer::<DatabaseFile, SqlitePanel>("Database")
            .formats([SQLITE]);
        {
            use bedtime_workbench::status::StatusAppExt;
            app.add_panel_status::<SqlitePanel>(status::item);
        }
    }
}

/// A database file. Nothing of it is kept: SQLite reads it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DatabaseFile;

impl DocumentModel for DatabaseFile {
    type Change = std::convert::Infallible;
    type OpenState = ();
    fn apply(&mut self, change: &Self::Change) -> Result<Self::Change, String> {
        match *change {}
    }
}

impl ByteModel for DatabaseFile {
    fn decode(_: &[u8]) -> Result<Self, String> {
        Ok(Self)
    }
    fn encode(&self, _: &mut dyn std::io::Write) -> std::io::Result<()> {
        Err(std::io::Error::other(
            "Databases are changed in place, not saved",
        ))
    }
}

impl FileModel for DatabaseFile {
    const UNREAD: bool = true;
}

#[bedtime_ui::panel]
#[derive(Reflect, Default)]
#[reflect(Component, Default)]
#[cfg_attr(feature = "persistence", reflect(Persist))]
pub struct SqlitePanel {
    /// The table or view shown, by name.
    pub selected: Option<String>,
    /// What of it shows.
    pub mode: Mode,
    /// The SQL typed into Query.
    pub query: String,
    #[reflect(ignore)]
    state: State,
}

/// What shows of the selected table or view.
#[derive(Reflect, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[reflect(Default)]
pub enum Mode {
    /// Its rows.
    #[default]
    Data,
    /// Its columns, indexes, triggers and statement.
    Structure,
    /// SQL to run on the database, and what it gave.
    Query,
}

/// A connection, shared with the tasks that ask it things.
type Connection = Arc<Mutex<Database>>;

/// How often to ask whether another program changed the database.
const CHECK_EVERY: f64 = 1.0;

/// What a check gives: nothing if the database is as it was last read.
type Checked = Result<Option<Read>, String>;

struct Read {
    /// The connection: opened by the first read, then kept.
    database: Connection,
    schema: Schema,
    read_only: bool,
    version: i64,
    interrupt: Interrupter,
}

#[derive(Default)]
struct State {
    /// The file `database` is a connection to.
    file: Option<PathBuf>,
    database: Option<Connection>,
    read_only: bool,
    /// What the database holds, or why that can't be read.
    schema: Option<Result<Schema, String>>,
    /// SQLite's count of others' changes when `schema` was read.
    version: Option<i64>,
    /// When it was last checked (egui's time), and a check under way.
    checked: f64,
    checking: Option<ManagedTask<Checked>>,
    /// The selected object's statement, and the view showing it.
    statement: Option<(String, CodeView)>,
    /// Which rows to show: the selected table's, filtered, sorted, a page.
    browse: Browse,
    /// The filter as typed; it's applied (to `browse`) with Enter.
    filter_text: String,
    /// The rows showing (or why there are none), and what they were read for.
    page: Option<Result<Page, String>>,
    page_for: Option<Browse>,
    /// Rows being read.
    paging: Option<(Browse, ManagedTask<Result<Page, String>>)>,
    /// Another program changed the database: read the rows again.
    stale: bool,
    grid: Grid,
    query: Query,
    changes: Changes,
}

/// Changing the selected table's rows.
#[derive(Default)]
struct Changes {
    /// Changes to make, in order, and the one being made.
    edits: VecDeque<Edit>,
    writing: Option<ManagedTask<Result<(), String>>>,
    /// Why the last one wasn't made.
    error: Option<String>,
    /// A row being made: what each column has been given so far.
    draft: Option<Vec<Option<Input>>>,
    /// The row being made is among `edits`: it goes when that's done.
    inserting: bool,
    /// A row asked to be deleted, until that's confirmed: its key and number.
    deleting: Option<(RowKey, u64)>,
}

/// What was pressed under the rows.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pressed {
    AddRow,
    InsertRow,
    Discard,
    Delete,
    KeepRow,
}

/// Running what's typed into Query, and what it gave.
#[derive(Default)]
struct Query {
    /// Asked to run.
    run: bool,
    running: Option<ManagedTask<Result<Outcome, String>>>,
    outcome: Option<Result<Outcome, String>>,
    /// The result's rows sorted by a column, if asked: SQLite gave them in
    /// its order, which is kept.
    sort: Option<(usize, bool)>,
    sorted: Option<Vec<Vec<Value>>>,
    grid: Grid,
    /// Stops what the connection is running.
    interrupt: Option<Interrupter>,
}

impl Drop for Query {
    /// A panel closed while its statements run doesn't leave them running.
    fn drop(&mut self) {
        if self.running.is_some()
            && let Some(interrupt) = &self.interrupt
        {
            interrupt.interrupt();
        }
    }
}

impl SqlitePanel {
    /// What the database holds, once read.
    pub fn schema(&self) -> Option<&Schema> {
        self.state.schema.as_ref()?.as_ref().ok()
    }

    /// What the last run of Query came to.
    pub fn outcome(&self) -> Option<&Result<Outcome, String>> {
        self.state.query.outcome.as_ref()
    }

    /// The rows showing, once read.
    pub fn page(&self) -> Option<&Page> {
        self.state.page.as_ref()?.as_ref().ok()
    }

    /// Which rows `page` was read for: its table, filter, sort and page.
    pub fn page_for(&self) -> Option<&Browse> {
        self.state.page_for.as_ref()
    }

    /// The database can't be changed (the file or its folder isn't writable).
    pub fn read_only(&self) -> bool {
        self.state.read_only
    }

    /// Query's statements are running.
    pub fn running(&self) -> bool {
        self.state.query.running.is_some()
    }
}

impl Panel for SqlitePanel {
    const LABEL: &'static str = "Database";

    fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, panel: Entity) {
        // Shows loading, or why the file can't be opened.
        if viewer_document::<DatabaseFile>(ui, world, panel).is_none() {
            return;
        }
        let Some(file) = world.get::<ViewerFile>(panel).map(|file| file.0.clone()) else {
            return;
        };
        update_query(world, panel);
        update(world, panel, &file, ui.input(|input| input.time));
        update_changes(world, panel);
        update_page(world, panel);
        let focused = world
            .get::<PanelPresentation>(panel)
            .is_some_and(|presentation| presentation.focused);
        let Some(mut sqlite) = world.get_mut::<SqlitePanel>(panel) else {
            return;
        };
        let sqlite = &mut *sqlite;
        // Come back for a check's answer, and for the next check.
        ui.ctx().request_repaint_after(
            if sqlite.state.checking.is_some()
                || sqlite.state.paging.is_some()
                || sqlite.state.query.running.is_some()
                || sqlite.state.changes.writing.is_some()
                || !sqlite.state.changes.edits.is_empty()
            {
                Duration::from_millis(16)
            } else {
                Duration::from_secs_f64(CHECK_EVERY)
            },
        );
        let State {
            schema,
            statement,
            read_only,
            browse,
            filter_text,
            page,
            page_for,
            grid,
            query,
            changes,
            ..
        } = &mut sqlite.state;
        let schema = match schema {
            None => {
                ui.centered_and_justified(|ui| ui.spinner());
                return;
            }
            Some(Err(error)) => {
                egui::Frame::NONE.inner_margin(14).show(ui, |ui| {
                    ui.label(format!("This database can't be read: {error}"));
                });
                return;
            }
            Some(Ok(schema)) => schema,
        };
        let margin = egui::Margin::symmetric(8, 6);
        egui::Panel::left(egui::Id::new((panel, "sqlite-objects")))
            .resizable(true)
            .default_size(180.0)
            .size_range(100.0..=420.0)
            .frame(egui::Frame::NONE.inner_margin(egui::Margin::symmetric(0, 6)))
            .show(ui, |ui| {
                sidebar(ui, context, panel, schema, &mut sqlite.selected)
            });
        egui::Panel::top(egui::Id::new((panel, "sqlite-bar")))
            .resizable(false)
            .frame(egui::Frame::NONE.inner_margin(margin))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut sqlite.mode, Mode::Data, "Data");
                    ui.selectable_value(&mut sqlite.mode, Mode::Structure, "Structure");
                    ui.selectable_value(&mut sqlite.mode, Mode::Query, "Query");
                    if sqlite.mode == Mode::Data {
                        filter(ui, panel, filter_text, browse);
                    }
                });
            });
        if sqlite.mode == Mode::Query {
            query_view(ui, context, panel, &mut sqlite.query, query, focused);
            return;
        }
        let Some(object) = sqlite
            .selected
            .as_deref()
            .and_then(|name| schema.object(name))
        else {
            // An empty database: Query can make its first table.
            egui::Frame::NONE.inner_margin(14).show(ui, |ui| {
                ui.weak("This database has no tables.");
            });
            return;
        };
        if sqlite.mode == Mode::Data {
            // The rows read for this table; another's don't show under its name.
            let read = page
                .as_ref()
                .filter(|_| page_for.as_ref().is_some_and(|at| at.table == object.name));
            // Which columns can be changed: a table's (not a view's), in a
            // file that can be written, but for those worked out from others.
            let editable: Option<Vec<bool>> = match read {
                Some(Ok(page)) if !*read_only && page.keys.is_some() => Some(
                    page.columns
                        .iter()
                        .map(|name| object.column(name).is_some_and(|column| !column.generated))
                        .collect(),
                ),
                _ => None,
            };
            if editable.is_none()
                || changes.draft.as_ref().is_some_and(|draft| {
                    read.and_then(|read| read.as_ref().ok())
                        .is_some_and(|page| draft.len() != page.columns.len())
                })
            {
                changes.draft = None;
            }
            let mut pressed = None;
            if let Some(Ok(page)) = read {
                egui::Panel::bottom(egui::Id::new((panel, "sqlite-pages")))
                    .resizable(false)
                    .frame(egui::Frame::NONE.inner_margin(margin))
                    .show(ui, |ui| {
                        let state = match (&changes.draft, &changes.deleting) {
                            (Some(_), _) => Footer::Making,
                            (None, Some((_, number))) => Footer::Deleting(*number),
                            (None, None) if editable.is_some() => Footer::CanAdd,
                            (None, None) if page.keys.is_some() => Footer::ReadOnly,
                            (None, None) => Footer::Fixed,
                        };
                        pressed = pages(ui, context, page, browse, state, changes.error.as_deref());
                    });
            }
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ui, |ui| match read {
                    None => {
                        ui.centered_and_justified(|ui| ui.spinner());
                    }
                    Some(Err(error)) => {
                        egui::Frame::NONE.inner_margin(14).show(ui, |ui| {
                            ui.colored_label(
                                theme::color(context.theme.palette.error),
                                format!("These rows can't be read: {error}"),
                            );
                        });
                    }
                    Some(Ok(page)) => {
                        let sort = browse.sort.as_ref().and_then(|sort| {
                            let column = page.columns.iter().position(|c| *c == sort.column)?;
                            Some((column, sort.descending))
                        });
                        let rows = grid::Rows {
                            columns: &page.columns,
                            rows: &page.rows,
                            first_row: page.first_row(),
                            sort,
                            editable: editable.as_deref(),
                            draft: changes.draft.as_deref(),
                        };
                        let id = egui::Id::new((panel, "sqlite-grid"));
                        let asked = grid.show(ui, context, id, &rows, focused);
                        let declared = |column: &str| {
                            object
                                .column(column)
                                .map(|column| column.declared.clone())
                                .unwrap_or_default()
                        };
                        for asked in asked {
                            match asked {
                                grid::Asked::Sort(sort) => {
                                    browse.sort = sort.map(|(column, descending)| Sort {
                                        column: page.columns[column].clone(),
                                        descending,
                                    });
                                    browse.page = 0;
                                }
                                // The row being made: kept until it's inserted.
                                grid::Asked::Set { row, column, input }
                                    if row >= page.rows.len() =>
                                {
                                    if let Some(given) = changes
                                        .draft
                                        .as_mut()
                                        .and_then(|draft| draft.get_mut(column))
                                    {
                                        *given = Some(input);
                                    }
                                }
                                grid::Asked::Set { row, column, input } => {
                                    if let Some(row) = page.keys.as_ref().and_then(|k| k.row(row)) {
                                        changes.error = None;
                                        changes.edits.push_back(Edit::Set {
                                            table: object.name.clone(),
                                            row,
                                            declared: declared(&page.columns[column]),
                                            column: page.columns[column].clone(),
                                            input,
                                        });
                                    }
                                }
                                grid::Asked::Delete { row } => {
                                    if let Some(key) = page.keys.as_ref().and_then(|k| k.row(row)) {
                                        let number = page.first_row() + row as u64 + 1;
                                        changes.deleting = Some((key, number));
                                    }
                                }
                            }
                        }
                        // After the grid: what was being typed in the new
                        // row is kept before the row is inserted.
                        match pressed {
                            Some(Pressed::AddRow) => {
                                changes.error = None;
                                changes.draft = Some(vec![None; page.columns.len()]);
                                let first = editable
                                    .as_ref()
                                    .and_then(|can| can.iter().position(|can| *can))
                                    .unwrap_or(0);
                                grid.select(page.rows.len(), first);
                            }
                            Some(Pressed::InsertRow) if !changes.inserting => {
                                if let Some(draft) = &changes.draft {
                                    changes.error = None;
                                    changes.inserting = true;
                                    changes.edits.push_back(Edit::Insert {
                                        table: object.name.clone(),
                                        values: draft
                                            .iter()
                                            .zip(&page.columns)
                                            .filter_map(|(given, column)| {
                                                let input = given.clone()?;
                                                Some((column.clone(), declared(column), input))
                                            })
                                            .collect(),
                                    });
                                }
                            }
                            Some(Pressed::InsertRow) => {}
                            Some(Pressed::Discard) => {
                                changes.draft = None;
                                changes.error = None;
                                grid.stop_editing();
                            }
                            Some(Pressed::Delete) => {
                                if let Some((row, _)) = changes.deleting.take() {
                                    changes.error = None;
                                    changes.edits.push_back(Edit::Delete {
                                        table: object.name.clone(),
                                        row,
                                    });
                                }
                            }
                            Some(Pressed::KeepRow) => changes.deleting = None,
                            None => {}
                        }
                    }
                });
            return;
        }
        if statement.as_ref().is_none_or(|(sql, _)| *sql != object.sql) {
            *statement = Some((
                object.sql.clone(),
                CodeView::new(object.sql.clone()).language("sql"),
            ));
        }
        if let Some((sql, view)) = statement
            && !sql.is_empty()
        {
            egui::Panel::bottom(egui::Id::new((panel, "sqlite-statement")))
                .resizable(true)
                .default_size(150.0)
                .min_size(48.0)
                .frame(egui::Frame::NONE.inner_margin(margin))
                .show(ui, |ui| {
                    ui.push_id((panel, "statement"), |ui| view.show(ui, context));
                });
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.inner_margin(margin))
            .show(ui, |ui| {
                egui::ScrollArea::both()
                    .auto_shrink(false)
                    .id_salt((panel, "structure"))
                    .show(ui, |ui| structure(ui, panel, object, *read_only));
            });
    }
}

/// Query: SQL to type and run, and under it the rows that gave.
fn query_view(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    panel: Entity,
    text: &mut String,
    query: &mut Query,
    focused: bool,
) {
    let id = egui::Id::new((panel, "sqlite-query"));
    let margin = egui::Margin::symmetric(8, 6);
    egui::Panel::top(id.with("editor"))
        .resizable(true)
        .default_size(150.0)
        .min_size(80.0)
        .frame(egui::Frame::NONE.inner_margin(margin))
        .show(ui, |ui| {
            // Cmd/Ctrl+Enter runs, before the text box makes a line of it.
            let edit = id.with("text");
            if ui.memory(|memory| memory.has_focus(edit))
                && ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter))
            {
                query.run = true;
            }
            egui::Panel::bottom(id.with("run"))
                .resizable(false)
                .frame(egui::Frame::NONE.inner_margin(egui::Margin {
                    top: 6,
                    ..default()
                }))
                .show(ui, |ui| {
                    ui.horizontal(|ui| run_bar(ui, context, query));
                });
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt(id.with("scroll"))
                        .auto_shrink(false)
                        .show(ui, |ui| {
                            let response = ui.add_sized(
                                ui.available_size(),
                                egui::TextEdit::multiline(text)
                                    .id(edit)
                                    .code_editor()
                                    .hint_text("SQL to run: one statement, or several")
                                    .desired_width(f32::INFINITY),
                            );
                            response.widget_info(|| {
                                egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, "SQL")
                            });
                        });
                });
        });
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE)
        .show(ui, |ui| {
            let Some(Ok(outcome)) = &query.outcome else {
                return;
            };
            if outcome.columns.is_empty() {
                return;
            }
            let rows = grid::Rows {
                columns: &outcome.columns,
                rows: query.sorted.as_deref().unwrap_or(&outcome.rows),
                first_row: 0,
                sort: query.sort,
                // A result's rows are only looked at.
                editable: None,
                draft: None,
            };
            let asked = query
                .grid
                .show(ui, context, id.with("grid"), &rows, focused);
            for asked in asked {
                if let grid::Asked::Sort(sort) = asked {
                    query.sort = sort;
                    query.sorted = sort.map(|(column, descending)| {
                        let mut rows = outcome.rows.clone();
                        rows.sort_by(|a, b| {
                            let order = a[column].order(&b[column]);
                            if descending { order.reverse() } else { order }
                        });
                        rows
                    });
                }
            }
        });
}

/// Under the SQL: Run (or Cancel while it runs), and what the last run came to.
fn run_bar(ui: &mut egui::Ui, context: &UiContext<'_>, query: &mut Query) {
    if query.running.is_some() {
        if ui.button("Cancel").clicked()
            && let Some(interrupt) = &query.interrupt
        {
            interrupt.interrupt();
        }
        ui.spinner();
        ui.weak("Running");
        return;
    }
    let mut frame = context.frame(ui);
    let run = frame.primary_button("Run");
    let key = if cfg!(target_os = "macos") {
        "Cmd+Enter"
    } else {
        "Ctrl+Enter"
    };
    frame.tooltip(&run, format!("Run ({key})"));
    if run.clicked() {
        query.run = true;
    }
    match &query.outcome {
        None => {}
        Some(Ok(outcome)) => {
            let time = if outcome.seconds < 1.0 {
                format!("{:.0} ms", outcome.seconds * 1000.0)
            } else {
                format!("{:.2} s", outcome.seconds)
            };
            ui.weak(format!("{} · {time}", outcome.summary()));
        }
        Some(Err(error)) => {
            ui.colored_label(theme::color(context.theme.palette.error), error);
        }
    }
}

/// Take what a run came to, and start one that was asked for.
fn update_query(world: &mut World, panel: Entity) {
    let Some(mut sqlite) = world.get_mut::<SqlitePanel>(panel) else {
        return;
    };
    let sqlite = &mut *sqlite;
    let state = &mut sqlite.state;
    if let Some(task) = &state.query.running {
        let outcome = match task.try_recv() {
            Ok(outcome) => outcome,
            Err(TryRecvError::Disconnected) => Err("Running it was interrupted".into()),
            Err(TryRecvError::Empty) => return,
        };
        state.query.running = None;
        // It may have changed the database (even failing, part way), and
        // SQLite doesn't count a connection's own changes: read it all again.
        if outcome.as_ref().map_or(true, |outcome| outcome.wrote) {
            state.version = None;
            state.checked = f64::NEG_INFINITY;
            state.stale = true;
        }
        state.query.sort = None;
        state.query.sorted = None;
        state.query.grid.rewind();
        state.query.outcome = Some(outcome);
    }
    if !std::mem::take(&mut state.query.run) {
        return;
    }
    let Some(database) = state.database.clone() else {
        return;
    };
    let sql = sqlite.query.clone();
    let task =
        world
            .resource_mut::<ManagedWork>()
            .spawn_task(Some(panel), "Running SQL", move |_| {
                database
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .run(&sql)
            });
    if let (Ok(task), Some(mut sqlite)) = (task, world.get_mut::<SqlitePanel>(panel)) {
        sqlite.state.query.running = Some(task);
    }
}

/// The filter: an SQL condition, applied with Enter.
fn filter(ui: &mut egui::Ui, panel: Entity, text: &mut String, browse: &mut Browse) {
    let response = ui.add(
        egui::TextEdit::singleline(text)
            .id(egui::Id::new((panel, "sqlite-filter")))
            .hint_text("Filter: an SQL condition")
            .desired_width(f32::INFINITY),
    );
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, "Filter rows"));
    let (enter, escape) = ui.input(|i| {
        (
            i.key_pressed(egui::Key::Enter),
            i.key_pressed(egui::Key::Escape),
        )
    });
    if response.lost_focus() && enter && browse.filter != text.trim() {
        browse.filter = text.trim().to_owned();
        browse.page = 0;
    } else if response.lost_focus() && escape {
        // Back to the filter in use.
        *text = browse.filter.clone();
    }
}

/// What can be done to the rows, under them.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Footer {
    /// A view's rows: nothing.
    Fixed,
    /// A table's, in a file that can't be written.
    ReadOnly,
    CanAdd,
    /// A row is being made.
    Making,
    /// This row (its number) was asked to be deleted.
    Deleting(u64),
}

/// Under the rows: which of them show, adding and deleting a row, and the
/// page before and after.
fn pages(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    page: &Page,
    browse: &mut Browse,
    state: Footer,
    error: Option<&str>,
) -> Option<Pressed> {
    let mut pressed = None;
    ui.horizontal(|ui| {
        let first = page.first_row();
        ui.weak(match (page.total, page.rows.len() as u64) {
            (0, _) => "No rows".to_owned(),
            (1, _) => "1 row".to_owned(),
            (total, shown) if shown == total => format!("{} rows", db::grouped(total)),
            (total, shown) => format!(
                "Rows {}–{} of {}",
                db::grouped(first + 1),
                db::grouped(first + shown),
                db::grouped(total)
            ),
        });
        match state {
            Footer::Fixed => {}
            Footer::ReadOnly => {
                ui.weak("· Read-only");
            }
            Footer::CanAdd => {
                if ui.button("Add Row").clicked() {
                    pressed = Some(Pressed::AddRow);
                }
            }
            Footer::Making => {
                if context.frame(ui).primary_button("Insert Row").clicked() {
                    pressed = Some(Pressed::InsertRow);
                }
                if ui.button("Discard").clicked() {
                    pressed = Some(Pressed::Discard);
                }
            }
            Footer::Deleting(number) => {
                ui.label(format!("Delete row {}?", db::grouped(number)));
                if ui.button("Delete").clicked() {
                    pressed = Some(Pressed::Delete);
                }
                if ui.button("Keep").clicked() {
                    pressed = Some(Pressed::KeepRow);
                }
            }
        }
        if let Some(error) = error {
            ui.colored_label(theme::color(context.theme.palette.error), error);
        }
        if page.pages() == 1 {
            return;
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let mut turn = |ui: &mut egui::Ui, icon, to: Option<u64>, what: &str| {
                ui.add_enabled_ui(to.is_some(), |ui| {
                    let mut frame = context.frame(ui);
                    let response = frame.icon_button(icon);
                    frame.tooltip(&response, what);
                    if let (true, Some(to)) = (response.clicked(), to) {
                        browse.page = to;
                    }
                });
            };
            let next = (page.page + 1 < page.pages()).then_some(page.page + 1);
            turn(ui, BuiltinIcon::ChevronRight, next, "Next Page");
            ui.weak(format!(
                "Page {} of {}",
                db::grouped(page.page + 1),
                db::grouped(page.pages())
            ));
            turn(
                ui,
                BuiltinIcon::ChevronLeft,
                page.page.checked_sub(1),
                "Previous Page",
            );
        });
    });
    pressed
}

/// Take what a change came to, and make the next one waiting.
fn update_changes(world: &mut World, panel: Entity) {
    let Some(mut sqlite) = world.get_mut::<SqlitePanel>(panel) else {
        return;
    };
    let state = &mut sqlite.state;
    if let Some(task) = &state.changes.writing {
        let made = match task.try_recv() {
            Ok(made) => made,
            Err(TryRecvError::Disconnected) => Err("Changing it was interrupted".into()),
            Err(TryRecvError::Empty) => return,
        };
        state.changes.writing = None;
        // SQLite doesn't count a connection's own changes: read the rows again.
        state.stale = true;
        let inserted = state.changes.inserting
            && !state
                .changes
                .edits
                .iter()
                .any(|edit| matches!(edit, Edit::Insert { .. }));
        match made {
            Ok(()) if inserted => {
                state.changes.inserting = false;
                state.changes.draft = None;
                // Unsorted, a new row is the last: show it.
                if state.browse.sort.is_none() {
                    state.browse.page = u64::MAX;
                }
            }
            Ok(()) => {}
            Err(error) => {
                // The row being made stays, to put right.
                state.changes.inserting &= !inserted;
                state.changes.error = Some(error);
            }
        }
    }
    let Some(database) = state.database.clone() else {
        return;
    };
    let Some(edit) = state.changes.edits.pop_front() else {
        return;
    };
    let task = world.resource_mut::<ManagedWork>().spawn_task(
        Some(panel),
        "Changing the database",
        move |_| {
            database
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .apply(&edit)
        },
    );
    if let (Ok(task), Some(mut sqlite)) = (task, world.get_mut::<SqlitePanel>(panel)) {
        sqlite.state.changes.writing = Some(task);
    }
}

/// Take rows that were being read, and read others when what's asked for
/// (the table, its filter and order, the page) isn't what shows.
fn update_page(world: &mut World, panel: Entity) {
    let Some(mut sqlite) = world.get_mut::<SqlitePanel>(panel) else {
        return;
    };
    let sqlite = &mut *sqlite;
    let state = &mut sqlite.state;
    if let Some((_, task)) = &state.paging {
        let read = match task.try_recv() {
            Ok(read) => read,
            Err(TryRecvError::Disconnected) => Err("Reading them was interrupted".into()),
            Err(TryRecvError::Empty) => return,
        };
        let (mut asked, _) = state.paging.take().unwrap();
        // A page past the end gave the last one.
        if let Ok(page) = &read
            && page.page != asked.page
        {
            if state.browse == asked {
                state.browse.page = page.page;
            }
            asked.page = page.page;
        }
        // Other rows than those showing: from the top. The same ones read
        // again (they changed) stay where they are.
        if state.page_for.as_ref() != Some(&asked) {
            state.grid.rewind();
        }
        // A cell being edited whose row is another now (rows came or went
        // above it): what's typed isn't put in the wrong one.
        if let (Some(row), Some(Ok(old)), Ok(new)) = (state.grid.editing_row(), &state.page, &read)
        {
            let key = |page: &Page| {
                page.keys
                    .as_ref()
                    .and_then(|keys| keys.rows.get(row).cloned())
            };
            let moved = if row < old.rows.len() {
                key(old) != key(new)
            } else {
                old.rows.len() != new.rows.len()
            };
            if moved {
                state.grid.stop_editing();
            }
        }
        state.grid.refit();
        state.page_for = Some(asked);
        state.page = Some(read);
    }
    if sqlite.mode != Mode::Data {
        return;
    }
    let Some(selected) = &sqlite.selected else {
        return;
    };
    if state.browse.table != *selected {
        state.browse = Browse {
            table: selected.clone(),
            ..default()
        };
        state.filter_text.clear();
        // Nothing asked of the last table carries over. Changes already
        // waiting are still made: they name their table.
        state.changes.draft = None;
        state.changes.deleting = None;
        state.changes.error = None;
    }
    let Some(database) = state.database.clone() else {
        return;
    };
    // Changes first: the rows are read once they're made.
    if state.changes.writing.is_some() || !state.changes.edits.is_empty() {
        return;
    }
    if state.page_for.as_ref() == Some(&state.browse) && !state.stale {
        return;
    }
    state.stale = false;
    let browse = state.browse.clone();
    let asked = browse.clone();
    let task =
        world
            .resource_mut::<ManagedWork>()
            .spawn_task(Some(panel), "Reading rows", move |_| {
                database
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .page(&browse)
            });
    if let (Ok(task), Some(mut sqlite)) = (task, world.get_mut::<SqlitePanel>(panel)) {
        sqlite.state.paging = Some((asked, task));
    }
}

/// Take a finished check's answer, and start the next when it's due: the
/// first reads the database, later ones only if another program changed it.
fn update(world: &mut World, panel: Entity, file: &Path, now: f64) {
    let Some(mut sqlite) = world.get_mut::<SqlitePanel>(panel) else {
        return;
    };
    let sqlite = &mut *sqlite;
    let state = &mut sqlite.state;
    if let Some(checking) = &state.checking {
        let checked = match checking.try_recv() {
            Ok(checked) => checked,
            Err(TryRecvError::Disconnected) => Err("Reading it was interrupted".into()),
            Err(TryRecvError::Empty) => return,
        };
        state.checking = None;
        state.checked = now;
        match checked {
            Ok(Some(read)) => {
                state.database = Some(read.database);
                state.read_only = read.read_only;
                // Not the first read: another program changed something.
                state.stale |= state.version.is_some();
                state.version = Some(read.version);
                state.query.interrupt = Some(read.interrupt);
                // Keep what was selected if it's still there; else the first.
                let kept = sqlite
                    .selected
                    .as_deref()
                    .is_some_and(|name| read.schema.object(name).is_some());
                if !kept {
                    sqlite.selected = read.schema.objects.first().map(|o| o.name.clone());
                }
                state.schema = Some(Ok(read.schema));
            }
            Ok(None) => {}
            // A check that fails (the file is locked, say) leaves what's
            // shown; the next may do better.
            Err(_) if matches!(state.schema, Some(Ok(_))) => {}
            Err(error) => {
                state.database = None;
                state.schema = Some(Err(error));
            }
        }
    }
    if state.file.as_deref() != Some(file) {
        // Another file (it was moved): a new connection.
        *state = State {
            file: Some(file.to_owned()),
            ..default()
        };
    } else if state.schema.is_some() && now - state.checked < CHECK_EVERY {
        return;
    }
    let database = state.database.clone();
    let known = state.version;
    let path = file.to_owned();
    let task = world.resource_mut::<ManagedWork>().spawn_task(
        Some(panel),
        "Reading the database",
        move |_| -> Checked {
            let database = match database {
                Some(database) => database,
                None => Arc::new(Mutex::new(Database::open(&path)?)),
            };
            let read = {
                let database = database.lock().unwrap_or_else(PoisonError::into_inner);
                let version = database.data_version()?;
                if known == Some(version) {
                    return Ok(None);
                }
                (
                    database.schema()?,
                    database.read_only(),
                    version,
                    database.interrupter(),
                )
            };
            let (schema, read_only, version, interrupt) = read;
            Ok(Some(Read {
                database,
                schema,
                read_only,
                version,
                interrupt,
            }))
        },
    );
    if let (Ok(task), Some(mut sqlite)) = (task, world.get_mut::<SqlitePanel>(panel)) {
        sqlite.state.checking = Some(task);
    }
}

/// The tables, then the views; clicking one selects it.
fn sidebar(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    panel: Entity,
    schema: &Schema,
    selected: &mut Option<String>,
) {
    let height = tree::row_height(context);
    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .id_salt((panel, "objects"))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            if schema.objects.is_empty() {
                ui.horizontal(|ui| {
                    ui.set_height(height);
                    ui.add_space(tree::INSET + 6.0);
                    ui.weak("No tables");
                });
            }
            for (kind, heading) in [(Kind::Table, "Tables"), (Kind::View, "Views")] {
                let mut objects = schema
                    .objects
                    .iter()
                    .filter(|object| object.kind == kind)
                    .peekable();
                if objects.peek().is_none() {
                    continue;
                }
                ui.horizontal(|ui| {
                    ui.set_height(height);
                    ui.add_space(tree::INSET + 6.0);
                    ui.weak(heading);
                });
                for object in objects {
                    let (rect, _) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), height),
                        egui::Sense::hover(),
                    );
                    let chosen = selected.as_deref() == Some(object.name.as_str());
                    let row = tree::Row {
                        label: Some(&object.name),
                        depth: 0,
                        icon: None,
                        expanded: None,
                        selected: chosen,
                        ink: theme::color(context.theme.palette.text),
                        icon_ink: context.theme.palette.text,
                    };
                    let item = tree::item_rect(ui, context, &row, rect, tree::INDENT);
                    let id = egui::Id::new((panel, "sqlite-object", &object.name));
                    let response = ui.interact(item, id, egui::Sense::click());
                    response.widget_info(|| {
                        egui::WidgetInfo::selected(
                            egui::WidgetType::SelectableLabel,
                            true,
                            chosen,
                            &object.name,
                        )
                    });
                    tree::paint_row(ui, context, row, rect, Some(&response), 1.0, tree::INDENT);
                    if response.clicked() {
                        *selected = Some(object.name.clone());
                    }
                }
            }
        });
}

/// A table's or view's columns, indexes and triggers.
fn structure(ui: &mut egui::Ui, panel: Entity, object: &Object, read_only: bool) {
    ui.horizontal(|ui| {
        ui.strong(&object.name);
        let kind = match object.kind {
            Kind::Table => "Table",
            Kind::View => "View",
        };
        let columns = match object.columns.len() {
            1 => "1 column".to_owned(),
            count => format!("{count} columns"),
        };
        ui.weak(format!("{kind}, {columns}"));
        if read_only {
            ui.weak("· Read-only");
        }
    });
    ui.add_space(6.0);
    egui::Grid::new((panel, "sqlite-columns"))
        .num_columns(3)
        .spacing([20.0, 6.0])
        .show(ui, |ui| {
            ui.weak("Column");
            ui.weak("Type");
            ui.weak("Constraints");
            ui.end_row();
            for column in &object.columns {
                ui.label(&column.name);
                ui.label(&column.declared);
                ui.label(column.constraints(&object.foreign_keys));
                ui.end_row();
            }
        });
    if !object.indexes.is_empty() {
        ui.add_space(12.0);
        egui::Grid::new((panel, "sqlite-indexes"))
            .num_columns(3)
            .spacing([20.0, 6.0])
            .show(ui, |ui| {
                ui.weak("Index");
                ui.weak("Columns");
                ui.weak("");
                ui.end_row();
                for index in &object.indexes {
                    ui.label(&index.name);
                    ui.label(index.columns.join(", "));
                    ui.label(match (index.unique, index.automatic) {
                        (true, true) => "unique, from a constraint",
                        (true, false) => "unique",
                        (false, true) => "from a constraint",
                        (false, false) => "",
                    });
                    ui.end_row();
                }
            });
    }
    if !object.triggers.is_empty() {
        ui.add_space(12.0);
        ui.weak("Triggers");
        ui.add_space(4.0);
        for trigger in &object.triggers {
            ui.label(trigger);
        }
    }
}
