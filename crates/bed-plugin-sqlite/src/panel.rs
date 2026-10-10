use crate::{
    backend::{Cell, DataSet, ObjectKind, PAGE_SIZE, Schema},
    worker::{Request, Response, Worker},
};
use bed_workbench_api::{HostContext, HostRequest, ModulePanel};
use dear_imgui_rs::{
    ChildFlags, FocusedFlags, Key, ListClipper, StyleColor, StyleVar, TabItemFlags,
    TableColumnFlags, TableColumnWidth, TableFlags, TableRowFlags, Ui, WindowFlags,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{any::Any, path::PathBuf};

// ImGui tables have a fixed column ceiling. Include a row-number column.
const VISIBLE_COLUMNS: usize = 255;
const CELL_PADDING: [f32; 2] = [8.0, 4.0];
const LOAD_FADE_SECONDS: f32 = 0.18;

#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
enum Tab {
    #[default]
    Data,
    Schema,
    Query,
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
enum TableTab {
    #[default]
    Data,
    Schema,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct ViewState {
    selected_table: Option<String>,
    sql: String,
    tab: Tab,
    tables_tab: TableTab,
    widths: Vec<f32>,
    first_column: usize,
}

#[derive(Clone, Copy)]
enum Pending {
    Schema,
    Browse,
    Query,
    Count,
}

pub struct SqlitePanel {
    path: PathBuf,
    worker: Worker,
    state: ViewState,
    schema: Option<Schema>,
    browse: Option<DataSet>,
    query: Option<DataSet>,
    offset: usize,
    previous_offsets: Vec<usize>,
    total_rows: Option<usize>,
    count_attempted: bool,
    serial: u64,
    pending: Option<Pending>,
    error: Option<String>,
    selection: Option<[usize; 2]>,
    inspector: bool,
    restore_tab: bool,
    restore_table_tab: bool,
    reveal: f32,
}

impl SqlitePanel {
    pub fn new(path: PathBuf, saved: &Value) -> Self {
        let mut state: ViewState = serde_json::from_value(saved.clone()).unwrap_or_default();
        match state.tab {
            Tab::Data => state.tables_tab = TableTab::Data,
            Tab::Schema => state.tables_tab = TableTab::Schema,
            Tab::Query => {}
        }
        state.widths.truncate(2000);
        for width in &mut state.widths {
            *width = if width.is_finite() {
                width.clamp(40.0, 2000.0)
            } else {
                150.0
            };
        }
        state.first_column = state.first_column / VISIBLE_COLUMNS * VISIBLE_COLUMNS;
        let mut worker = Worker::new(path.clone());
        let serial = worker.request(Request::Schema);
        Self {
            path,
            worker,
            state,
            schema: None,
            browse: None,
            query: None,
            offset: 0,
            previous_offsets: Vec::new(),
            total_rows: None,
            count_attempted: false,
            serial,
            pending: Some(Pending::Schema),
            error: None,
            selection: None,
            inspector: false,
            restore_tab: true,
            restore_table_tab: true,
            reveal: 1.0,
        }
    }

    /// Polling is also exposed for embedding hosts without an active UI frame.
    pub fn poll(&mut self) {
        while let Some(reply) = self.worker.poll() {
            if reply.id != self.serial || self.pending.is_none() {
                continue;
            }
            let pending = self.pending.take().unwrap();
            match reply.result {
                Err(error) => {
                    // Counting is optional; browsing remains usable if a view's
                    // count is too expensive or the database changes meanwhile.
                    if !matches!(pending, Pending::Count) {
                        self.error = Some(error);
                    }
                }
                Ok(Response::Schema(schema)) => {
                    if !self.state.selected_table.as_ref().is_some_and(|selected| {
                        schema.objects.iter().any(|object| &object.name == selected)
                    }) {
                        self.state.selected_table =
                            schema.objects.first().map(|object| object.name.clone());
                        self.offset = 0;
                        self.previous_offsets.clear();
                    }
                    self.schema = Some(schema);
                    self.error = None;
                    if self.state.tab == Tab::Data {
                        self.browse_table();
                    }
                }
                Ok(Response::Data(data)) => {
                    self.state.widths.resize(data.columns.len(), 150.0);
                    if self.state.first_column >= data.columns.len() {
                        self.state.first_column = 0;
                    }
                    self.selection = None;
                    self.inspector = false;
                    match pending {
                        Pending::Browse => {
                            self.browse = Some(data);
                            self.reveal = 0.0;
                        }
                        Pending::Query => self.query = Some(data),
                        Pending::Schema | Pending::Count => {
                            self.error =
                                Some("SQLite worker returned rows for a schema request".into());
                            continue;
                        }
                    }
                    self.error = None;
                }
                Ok(Response::Count(rows)) => self.total_rows = Some(rows),
            }
        }
        if self.pending.is_none()
            && self.state.tab == Tab::Data
            && self.browse.is_some()
            && self.total_rows.is_none()
            && !self.count_attempted
        {
            self.count_attempted = true;
            let table = self.state.selected_table.clone().unwrap();
            self.submit(Request::Count { table }, Pending::Count);
        }
    }

    pub fn is_ready(&self) -> bool {
        self.schema.is_some() && self.pending.is_none()
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    fn submit(&mut self, request: Request, pending: Pending) {
        if matches!(self.pending, Some(Pending::Count)) {
            self.count_attempted = false;
        }
        self.serial = self.worker.request(request);
        self.pending = Some(pending);
        self.error = None;
        self.selection = None;
        self.inspector = false;
    }

    fn browse_table(&mut self) {
        let Some(table) = self.state.selected_table.clone() else {
            return;
        };
        self.browse = None;
        self.submit(
            Request::Browse {
                table,
                offset: self.offset,
            },
            Pending::Browse,
        );
    }

    fn run_query(&mut self) {
        if matches!(self.pending, Some(Pending::Schema)) {
            return;
        }
        if self.state.sql.trim().is_empty() {
            self.error = Some("Enter a read-only SQL query.".into());
            return;
        }
        self.query = None;
        self.submit(
            Request::Query {
                sql: self.state.sql.clone(),
            },
            Pending::Query,
        );
    }

    fn cancel(&mut self) {
        if matches!(self.pending, Some(Pending::Count)) {
            self.count_attempted = true;
        }
        self.worker.cancel();
        self.pending = None;
        self.error = Some("Cancelled.".into());
    }

    fn active_data(&self) -> Option<&DataSet> {
        match self.state.tab {
            Tab::Data => self.browse.as_ref(),
            Tab::Query => self.query.as_ref(),
            Tab::Schema => None,
        }
    }

    fn select_tab(&mut self, tab: Tab) {
        if self.state.tab == tab {
            return;
        }
        self.state.tab = tab;
        match tab {
            Tab::Data => self.state.tables_tab = TableTab::Data,
            Tab::Schema => self.state.tables_tab = TableTab::Schema,
            Tab::Query => {}
        }
        self.selection = None;
        self.inspector = false;
        self.state.first_column = 0;
        if tab == Tab::Data
            && self.browse.is_none()
            && self.schema.is_some()
            && !matches!(self.pending, Some(Pending::Schema))
        {
            self.browse_table();
        }
    }

    fn copy_cell(&self, ui: &Ui) {
        if let Some([row, column]) = self.selection
            && let Some(cell) = self
                .active_data()
                .and_then(|data| data.rows.get(row))
                .and_then(|row| row.get(column))
        {
            copy_text(ui, &cell.display_text());
        }
    }

    fn copy_row(&self, ui: &Ui, row: usize) {
        if let Some(row) = self.active_data().and_then(|data| data.rows.get(row)) {
            copy_text(ui, &row_text(row));
        }
    }

    fn tab_actions(&mut self, ui: &Ui, status: Option<&str>) {
        if let Some(status) = status {
            ui.align_text_to_frame_padding();
            ui.text_disabled(status);
            ui.same_line();
            if ui.button("Cancel") {
                self.cancel();
            }
            ui.same_line();
        }
        if ui.button("Refresh") {
            self.browse = None;
            self.offset = 0;
            self.previous_offsets.clear();
            self.total_rows = None;
            self.count_attempted = false;
            self.submit(Request::Schema, Pending::Schema);
        }
    }

    fn sidebar(&mut self, ui: &Ui) {
        ui.text_disabled("Tables and views");
        ui.separator();
        let mut selected = None;
        if let Some(schema) = &self.schema {
            if schema.objects.is_empty() {
                ui.text_wrapped("This database has no tables or views.");
            }
            for (index, object) in schema.objects.iter().enumerate() {
                let _id = ui.push_id(index);
                let kind = match object.kind {
                    ObjectKind::Table => "Table",
                    ObjectKind::View => "View",
                };
                if ui
                    .selectable_config(widget_label(&object.name))
                    .selected(self.state.selected_table.as_deref() == Some(&object.name))
                    .build()
                {
                    selected = Some(object.name.clone());
                }
                if ui.is_item_hovered() {
                    ui.tooltip_text(format!("{kind}: {}", object.name));
                }
            }
        } else if self.pending.is_some() {
            ui.text_disabled("Reading schema…");
        }
        if let Some(selected) = selected {
            if matches!(self.pending, Some(Pending::Browse | Pending::Count)) {
                self.worker.cancel();
                self.pending = None;
            }
            self.state.selected_table = Some(selected);
            self.offset = 0;
            self.previous_offsets.clear();
            self.total_rows = None;
            self.count_attempted = false;
            self.browse = None;
            self.selection = None;
            self.inspector = false;
            self.state.first_column = 0;
            if self.state.tab == Tab::Data && !matches!(self.pending, Some(Pending::Schema)) {
                self.browse_table();
            }
        }
    }

    fn tabs(&mut self, ui: &Ui) {
        let style = ui.clone_style();
        let button_padding = 2.0 * style.frame_padding()[0];
        let mut actions_width = ui.calc_text_size("Refresh")[0] + button_padding;
        let status = self.pending.map(|pending| {
            if matches!(pending, Pending::Count) {
                "Counting rows…"
            } else {
                "Loading…"
            }
        });
        if let Some(status) = status {
            actions_width += ui.calc_text_size(status)[0]
                + ui.calc_text_size("Cancel")[0]
                + button_padding
                + 2.0 * style.item_spacing()[0];
        }
        let mut visible = false;
        {
            let _padding = ui.push_style_var(StyleVar::CellPadding([0.0, 0.0]));
            if let Some(_header) = ui.begin_table_with_flags(
                "sqlite-header",
                2,
                TableFlags::NO_SAVED_SETTINGS
                    | TableFlags::NO_PAD_OUTER_X
                    | TableFlags::NO_PAD_INNER_X,
            ) {
                ui.table_setup_column(
                    "Tabs",
                    TableColumnFlags::NONE,
                    Some(TableColumnWidth::Stretch(1.0)),
                );
                ui.table_setup_column(
                    "Actions",
                    TableColumnFlags::NONE,
                    Some(TableColumnWidth::Fixed(actions_width)),
                );
                ui.table_next_column();
                if let Some(_tabs) = ui.tab_bar("sqlite-tabs") {
                    let query_selected = self.state.tab == Tab::Query;
                    for (query, label) in [(false, "Tables"), (true, "Query")] {
                        let flags = if self.restore_tab && query_selected == query {
                            TabItemFlags::SET_SELECTED
                        } else {
                            TabItemFlags::NONE
                        };
                        if let Some(_tab) = ui.tab_item_with_flags(label, None, flags) {
                            if self.restore_tab && query_selected != query {
                                continue;
                            }
                            self.restore_tab = false;
                            if query {
                                self.select_tab(Tab::Query);
                            } else if self.state.tab == Tab::Query {
                                self.select_tab(match self.state.tables_tab {
                                    TableTab::Data => Tab::Data,
                                    TableTab::Schema => Tab::Schema,
                                });
                                self.restore_table_tab = true;
                            }
                            visible = true;
                        }
                    }
                }
                ui.table_next_column();
                self.tab_actions(ui, status);
            }
        }
        if !visible {
            return;
        }
        if self.state.tab == Tab::Query {
            self.query_tab(ui);
            return;
        }
        let size = ui.content_region_avail();
        let sidebar_width = (size[0] * 0.22).clamp(100.0, 220.0).min(size[0] * 0.4);
        ui.child_window("sqlite-objects")
            .size([sidebar_width, 0.0])
            .flags(WindowFlags::HORIZONTAL_SCROLLBAR)
            .build(ui, || self.sidebar(ui));
        ui.same_line();
        ui.child_window("sqlite-content")
            .size([0.0, 0.0])
            .build(ui, || {
                if let Some(_tabs) = ui.tab_bar("sqlite-table-tabs") {
                    let selected = self.state.tab;
                    for (tab, label) in [(Tab::Data, "Data"), (Tab::Schema, "Schema")] {
                        let flags = if self.restore_table_tab && selected == tab {
                            TabItemFlags::SET_SELECTED
                        } else {
                            TabItemFlags::NONE
                        };
                        if let Some(_tab) = ui.tab_item_with_flags(label, None, flags) {
                            if self.restore_table_tab && selected != tab {
                                continue;
                            }
                            self.restore_table_tab = false;
                            self.select_tab(tab);
                            match tab {
                                Tab::Data => self.data_tab(ui),
                                Tab::Schema => self.schema_tab(ui),
                                Tab::Query => unreachable!(),
                            }
                        }
                    }
                }
            });
    }

    fn data_tab(&mut self, ui: &Ui) {
        let Some(table) = &self.state.selected_table else {
            ui.text_disabled("Select a table or view to browse its rows.");
            return;
        };
        ui.text(table);
        let ready = self.pending.is_none() || matches!(self.pending, Some(Pending::Count));
        let can_previous = self.offset > 0 && ready;
        let can_next = self.browse.as_ref().is_some_and(|data| data.has_more) && ready;
        let mut page = None;
        {
            let _disabled = ui.begin_disabled_with_cond(!can_previous);
            if ui.button("Previous page") {
                page =
                    Some(self.previous_offsets.pop().unwrap_or_else(|| {
                        (self.offset.saturating_sub(1) / PAGE_SIZE) * PAGE_SIZE
                    }));
            }
        }
        ui.same_line();
        {
            let _disabled = ui.begin_disabled_with_cond(!ready || self.total_rows.is_none());
            ui.set_next_item_width(140.0);
            let current = self.offset / PAGE_SIZE;
            let label = if self.offset % PAGE_SIZE == 0 {
                format!("{}", current + 1)
            } else {
                format!("{} (continued)", current + 1)
            };
            if let Some(_combo) = ui.begin_combo("Page", label) {
                let pages = self.total_rows.unwrap_or(0).div_ceil(PAGE_SIZE).max(1);
                let mut clipper = ListClipper::new(pages).begin(ui);
                clipper.include_item_by_index(current.min(pages - 1));
                for index in clipper.iter() {
                    if ui
                        .selectable_config((index + 1).to_string())
                        .selected(index == current)
                        .build()
                    {
                        self.previous_offsets.clear();
                        page = Some(index * PAGE_SIZE);
                    }
                    if index == current {
                        ui.set_item_default_focus();
                    }
                }
            }
        }
        ui.same_line();
        {
            let _disabled = ui.begin_disabled_with_cond(!can_next);
            if ui.button("Next page") {
                self.previous_offsets.push(self.offset);
                page = Some(self.offset + self.browse.as_ref().unwrap().rows.len());
            }
        }
        if let Some(data) = &self.browse {
            ui.same_line();
            if data.rows.is_empty() {
                ui.text_disabled("No rows");
            } else {
                ui.text_disabled(format!(
                    "Rows {}–{}",
                    self.offset + 1,
                    self.offset + data.rows.len()
                ));
            }
        }
        if let Some(offset) = page {
            self.offset = offset;
            self.browse_table();
        }
        ui.text_disabled("Use SQL in Query to sort or filter.");
        self.results(ui);
    }

    fn query_tab(&mut self, ui: &Ui) {
        let input_height = (ui.content_region_avail()[1] * 0.25).clamp(65.0, 160.0);
        ui.input_text_multiline("##sqlite-sql", &mut self.state.sql, [-1.0, input_height])
            .build();
        {
            let _disabled =
                ui.begin_disabled_with_cond(matches!(self.pending, Some(Pending::Schema)));
            if ui.button("Run query") {
                self.run_query();
            }
        }
        ui.same_line();
        ui.text_disabled("⌘/Ctrl+Enter · 1,000 rows · 8 MiB · 10 seconds");
        self.results(ui);
    }

    fn schema_tab(&self, ui: &Ui) {
        let _padding = ui.push_style_var(StyleVar::CellPadding(CELL_PADDING));
        ui.child_window("sqlite-schema")
            .size([0.0, 0.0])
            .flags(WindowFlags::HORIZONTAL_SCROLLBAR)
            .build(ui, || {
                let object = self.schema.as_ref().and_then(|schema| {
                    schema
                        .objects
                        .iter()
                        .find(|object| Some(&object.name) == self.state.selected_table.as_ref())
                });
                let Some(object) = object else {
                    ui.text_disabled("Select a table or view to inspect its schema.");
                    return;
                };
                ui.text(&object.name);
                if let Some(sql) = &object.create_sql {
                    let _id = ui.push_id(&object.name);
                    let height = (ui.content_region_avail()[1] * 0.4).clamp(200.0, 360.0);
                    ui.child_window("sqlite-create")
                        .size([0.0, height])
                        .child_flags(ChildFlags::FRAME_STYLE)
                        .flags(WindowFlags::HORIZONTAL_SCROLLBAR)
                        .build(ui, || {
                            ui.text(sql);
                            if let Some(_popup) = ui.begin_popup_context_window() {
                                if ui.menu_item("Copy SQL") {
                                    copy_text(ui, sql);
                                }
                            }
                        });
                }
                ui.separator();
                ui.text("Columns");
                if let Some(_table) = ui.begin_table_with_flags(
                    "sqlite-schema-columns",
                    5,
                    TableFlags::RESIZABLE | TableFlags::BORDERS_INNER | TableFlags::ROW_BG,
                ) {
                    for name in ["Name", "Type", "Constraints", "Default", "Hidden/generated"] {
                        ui.table_setup_column(name, TableColumnFlags::NONE, None);
                    }
                    ui.table_headers_row();
                    for column in &object.columns {
                        ui.table_next_row();
                        ui.table_next_column();
                        ui.text(&column.name);
                        ui.table_next_column();
                        ui.text(&column.declared_type);
                        let mut constraints = Vec::new();
                        if column.not_null {
                            constraints.push("NOT NULL".to_owned());
                        }
                        if column.primary_key_position > 0 {
                            constraints.push(format!("PK {}", column.primary_key_position));
                        }
                        ui.table_next_column();
                        ui.text(constraints.join(", "));
                        ui.table_next_column();
                        ui.text(column.default_value.as_deref().unwrap_or(""));
                        ui.table_next_column();
                        ui.text(match column.hidden {
                            0 => "",
                            1 => "Hidden",
                            2 => "Virtual",
                            3 => "Stored",
                            _ => "Unknown",
                        });
                    }
                }
                ui.separator();
                ui.text("Indexes");
                if object.indexes.is_empty() {
                    ui.text_disabled("None");
                }
                for index in &object.indexes {
                    let columns = index
                        .columns
                        .iter()
                        .map(|column| column.as_deref().unwrap_or("(expression)"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    ui.text_wrapped(format!(
                        "{} ({}){}{}",
                        index.name,
                        columns,
                        if index.unique { " · UNIQUE" } else { "" },
                        if index.partial { " · partial" } else { "" }
                    ));
                    if let Some(sql) = &index.create_sql {
                        ui.text_wrapped(sql);
                    }
                }
                ui.separator();
                ui.text("Foreign keys");
                if object.foreign_keys.is_empty() {
                    ui.text_disabled("None");
                }
                for key in &object.foreign_keys {
                    ui.text_wrapped(format!(
                        "Key {}, part {}: {} → {}.{} · ON UPDATE {} · ON DELETE {}",
                        key.id + 1,
                        key.sequence + 1,
                        key.from,
                        key.table,
                        key.to.as_deref().unwrap_or("(primary key)"),
                        key.on_update,
                        key.on_delete
                    ));
                }
            });
    }

    fn results(&mut self, ui: &Ui) {
        let Some(data) = self.active_data() else {
            if self.pending.is_none() && self.error.is_none() && self.state.tab == Tab::Query {
                ui.text_disabled("read-only query");
            }
            return;
        };
        let columns = data.columns.len();
        let rows = data.rows.len();
        let truncated = data.truncated;
        let elapsed = data.elapsed;
        let limit_message = match (truncated, self.state.tab) {
            (true, Tab::Data) => " · more rows available",
            (true, _) => " · result limit reached",
            (false, _) => "",
        };
        ui.text_disabled(format!(
            "{rows} rows · {columns} columns · {:.3} s{}",
            elapsed.as_secs_f64(),
            limit_message
        ));
        if columns > VISIBLE_COLUMNS {
            if ui.small_button("Previous columns") {
                self.state.first_column = self.state.first_column.saturating_sub(VISIBLE_COLUMNS);
            }
            ui.same_line();
            if ui.small_button("Next columns")
                && self.state.first_column + VISIBLE_COLUMNS < columns
            {
                self.state.first_column += VISIBLE_COLUMNS;
            }
            ui.same_line();
            ui.text_disabled(format!(
                "Columns {}–{} of {}",
                self.state.first_column + 1,
                (self.state.first_column + VISIBLE_COLUMNS).min(columns),
                columns
            ));
        }
        if self.selection.is_some() {
            if ui.small_button("Copy cell") {
                self.copy_cell(ui);
            }
            ui.same_line();
            if ui.small_button("Copy row") {
                self.copy_row(ui, self.selection.unwrap()[0]);
            }
            ui.same_line();
            ui.checkbox("Inspect cell", &mut self.inspector);
        }
        if self.inspector {
            self.cell_inspector(ui);
        }
        if rows == 0 {
            ui.text_disabled("No rows returned.");
        }
        self.grid(ui);
    }

    fn cell_inspector(&self, ui: &Ui) {
        let Some([row, column]) = self.selection else {
            return;
        };
        let Some(data) = self.active_data() else {
            return;
        };
        let Some(cell) = data.rows.get(row).and_then(|row| row.get(column)) else {
            return;
        };
        ui.separator();
        ui.text(format!("{} · {}", data.columns[column], cell.kind_name()));
        match cell {
            Cell::Text { byte_len, .. } if cell.is_truncated() => {
                ui.text_disabled(format!(
                    "Showing a bounded preview of {byte_len} bytes. Copy uses this preview."
                ));
            }
            Cell::Blob { preview, byte_len } if preview.len() < *byte_len => {
                ui.text_disabled(format!(
                    "Showing {} of {byte_len} bytes. Copy uses this preview.",
                    preview.len()
                ));
            }
            _ => {}
        }
        let mut text = cell.display_text();
        ui.input_text_multiline("##sqlite-inspector", &mut text, [-1.0, 85.0])
            .read_only(true)
            .build();
        ui.separator();
    }

    fn grid(&mut self, ui: &Ui) {
        let Some(data) = self.active_data() else {
            return;
        };
        let start = self.state.first_column;
        let end = (start + VISIBLE_COLUMNS).min(data.columns.len());
        if start >= end {
            return;
        }
        let data = match self.state.tab {
            Tab::Data => self.browse.as_ref().unwrap(),
            Tab::Query => self.query.as_ref().unwrap(),
            Tab::Schema => return,
        };
        let flags = TableFlags::RESIZABLE
            | TableFlags::SCROLL_X
            | TableFlags::SCROLL_Y
            | TableFlags::BORDERS_INNER
            | TableFlags::ROW_BG
            | TableFlags::NO_SAVED_SETTINGS;
        let _padding = ui.push_style_var(StyleVar::CellPadding(CELL_PADDING));
        let eased = 1.0 - (1.0 - self.reveal).powi(3);
        let _alpha = ui.push_style_var(StyleVar::Alpha(
            ui.clone_style().alpha() * (0.35 + 0.65 * eased),
        ));
        let Some(_table) = ui.begin_table_with_sizing(
            "sqlite-results",
            end - start + 1,
            flags,
            [0.0, ui.content_region_avail()[1].max(60.0)],
            0.0,
        ) else {
            return;
        };
        ui.table_setup_column(
            "#",
            TableColumnFlags::NO_RESIZE,
            Some(TableColumnWidth::Fixed(65.0)),
        );
        for column in start..end {
            ui.table_setup_column(
                format!("{}##{column}", widget_label(&data.columns[column])),
                TableColumnFlags::NONE,
                Some(TableColumnWidth::Fixed(
                    self.state.widths.get(column).copied().unwrap_or(150.0),
                )),
            );
        }
        ui.table_setup_scroll_freeze(1, 1);
        ui.table_headers_row();
        let cell_height = ui.text_line_height();
        let row_height = cell_height + 2.0 * CELL_PADDING[1];
        let _spacing = ui.push_style_var(StyleVar::ItemSpacing([0.0, 0.0]));
        let mut copy_cell = false;
        let mut copy_row = None;
        for row in ListClipper::new(data.rows.len())
            .items_height(row_height)
            .begin(ui)
            .iter()
        {
            let _row_id = ui.push_id(row);
            ui.table_next_row_with_flags(TableRowFlags::NONE, row_height);
            ui.table_next_column();
            let row_number = row
                + 1
                + if self.state.tab == Tab::Data {
                    self.offset
                } else {
                    0
                };
            {
                let _hover = ui.push_style_color(StyleColor::HeaderHovered, [0.0; 4]);
                let _active = ui.push_style_color(StyleColor::HeaderActive, [0.0; 4]);
                if ui
                    .selectable_config(row_number.to_string())
                    .size([0.0, cell_height])
                    .build()
                {
                    self.selection = Some([row, start]);
                }
            }
            if let Some(_popup) = ui.begin_popup_context_item_with_label(Some("row-menu")) {
                if ui.menu_item("Copy row") {
                    copy_row = Some(row);
                }
            }
            for column in start..end {
                if !ui.table_next_column() {
                    continue;
                }
                let _cell_id = ui.push_id(column);
                let cell = &data.rows[row][column];
                let color = if matches!(cell, Cell::Null) {
                    Some(ui.push_style_color(
                        StyleColor::Text,
                        ui.style_color(StyleColor::TextDisabled),
                    ))
                } else {
                    None
                };
                let label = format!("{}##cell", widget_label(&cell_preview(cell)));
                {
                    let selected = self.selection == Some([row, column]);
                    let highlight = if selected {
                        ui.style_color(StyleColor::Header)
                    } else {
                        [0.0; 4]
                    };
                    let _hover = ui.push_style_color(StyleColor::HeaderHovered, highlight);
                    let _active = ui.push_style_color(StyleColor::HeaderActive, highlight);
                    if ui
                        .selectable_config(label)
                        .selected(selected)
                        .allow_double_click(true)
                        .size([0.0, cell_height])
                        .build()
                    {
                        self.selection = Some([row, column]);
                        if ui.is_mouse_double_clicked(dear_imgui_rs::MouseButton::Left) {
                            self.inspector = true;
                        }
                    }
                }
                drop(color);
                if ui.is_item_hovered() {
                    ui.tooltip_text(format!(
                        "{} · {}\n{}",
                        data.columns[column],
                        cell.kind_name(),
                        cell_preview(cell)
                    ));
                }
                if let Some(_popup) = ui.begin_popup_context_item_with_label(Some("cell-menu")) {
                    self.selection = Some([row, column]);
                    if ui.menu_item("Copy cell") {
                        copy_cell = true;
                    }
                    if ui.menu_item("Copy row") {
                        copy_row = Some(row);
                    }
                    if ui.menu_item("Inspect cell") {
                        self.inspector = true;
                    }
                }
            }
        }
        // The binding exposes initial widths; capture resized widths in the active table scope.
        ui.binding().with_bound_context(|| unsafe {
            let table = dear_imgui_sys::igGetCurrentTable();
            if !table.is_null() && !(*table).Columns.Data.is_null() {
                for column in start..end {
                    let width = (*(*table).Columns.Data.add(column - start + 1)).WidthGiven;
                    if width.is_finite()
                        && width >= 40.0
                        && let Some(saved) = self.state.widths.get_mut(column)
                    {
                        *saved = width;
                    }
                }
            }
        });
        if copy_cell {
            self.copy_cell(ui);
        }
        if let Some(row) = copy_row {
            self.copy_row(ui, row);
        }
    }
}

impl ModulePanel for SqlitePanel {
    fn title(&self, _: &HostContext<'_>) -> String {
        self.path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "SQLite Viewer".into())
    }

    fn draw(&mut self, ui: &Ui, host: &HostContext<'_>, requests: &mut Vec<HostRequest>) {
        self.poll();
        if host.animations {
            if self.active_data().is_some() {
                self.reveal = (self.reveal + ui.io().delta_time() / LOAD_FADE_SECONDS).min(1.0);
            }
        } else {
            self.reveal = 1.0;
        }
        if let Some(error) = &self.error {
            ui.text_wrapped(error);
        }
        self.tabs(ui);
        if ui.is_window_focused_with_flags(FocusedFlags::ROOT_AND_CHILD_WINDOWS) {
            let primary = ui.io().key_ctrl() || ui.io().key_super();
            if primary
                && ui.is_key_pressed_with_repeat(Key::Enter, false)
                && self.state.tab == Tab::Query
            {
                self.run_query();
            }
            if primary && ui.is_key_pressed_with_repeat(Key::C, false) && !ui.is_any_item_active() {
                self.copy_cell(ui);
            }
            if ui.is_key_pressed_with_repeat(Key::Escape, false) && self.pending.is_some() {
                self.cancel();
            }
        }
        // Keep the native event loop polling while database work is in flight.
        if self.pending.is_some() || (self.reveal < 1.0 && self.active_data().is_some()) {
            requests.push(HostRequest::Invalidate);
        }
    }

    fn save_state(&self) -> Value {
        serde_json::to_value(&self.state).expect("SQLite view state is serializable")
    }

    fn close(&mut self, _: &mut Vec<HostRequest>) {
        self.worker.cancel();
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

fn widget_label(text: &str) -> String {
    text.replace('\0', "␀").replace("##", "#\u{200b}#")
}

fn copy_text(ui: &Ui, text: &str) {
    let text =
        std::ffi::CString::new(text.replace('\0', "␀")).expect("clipboard preview contains no NUL");
    ui.binding().with_bound_context(|| unsafe {
        dear_imgui_sys::igSetClipboardText(text.as_ptr());
    });
}

fn cell_preview(cell: &Cell) -> String {
    let full = cell.display_text();
    let mut preview: String = full
        .chars()
        .take(120)
        .map(|c| match c {
            '\n' => '↵',
            '\r' => '␍',
            '\t' => '⇥',
            '\0' => '␀',
            c => c,
        })
        .collect();
    if full.chars().count() > 120 || cell.is_truncated() {
        preview.push('…');
    }
    preview
}

fn row_text(row: &[Cell]) -> String {
    row.iter()
        .map(|cell| {
            let text = cell.display_text();
            if text.contains(['\t', '\r', '\n', '"']) {
                format!("\"{}\"", text.replace('"', "\"\""))
            } else {
                text
            }
        })
        .collect::<Vec<_>>()
        .join("\t")
}

#[cfg(test)]
mod tests {
    use super::*;
    use dear_imgui_rs::{Condition, Context, FramePrepareOptions};
    use rusqlite::Connection;
    use serde_json::json;
    use std::{
        cell::RefCell,
        collections::HashMap,
        ffi::CStr,
        rc::Rc,
        sync::{
            Mutex,
            atomic::{AtomicU64, Ordering},
        },
        thread,
        time::{Duration, Instant},
    };

    static IMGUI_LOCK: Mutex<()> = Mutex::new(());
    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

    struct Database(PathBuf);
    impl Database {
        fn new(sql: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "bed-sqlite-panel-{}-{}.db",
                std::process::id(),
                NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
            ));
            Connection::open(&path).unwrap().execute_batch(sql).unwrap();
            Self(path)
        }
    }
    impl Drop for Database {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    struct Clipboard(Rc<RefCell<String>>);
    impl dear_imgui_rs::ClipboardBackend for Clipboard {
        fn get(&mut self) -> Option<String> {
            Some(self.0.borrow().clone())
        }
        fn set(&mut self, value: &str) {
            *self.0.borrow_mut() = value.into();
        }
    }

    struct Harness {
        context: Context,
        panel: SqlitePanel,
        clipboard: Rc<RefCell<String>>,
        tab_positions: HashMap<String, [f32; 2]>,
        refresh_position: Option<[f32; 2]>,
        sql_pane: Option<SqlPane>,
    }

    #[derive(Clone, Copy)]
    struct SqlPane {
        center: [f32; 2],
        scroll: [f32; 2],
        scroll_max: [f32; 2],
        scrollbars: [bool; 2],
    }

    enum DataAction {
        Control(&'static CStr),
        Page(&'static CStr),
    }
    impl Harness {
        fn new(database: &Database, state: Value) -> Self {
            let mut context = Context::create();
            context.set_ini_filename(None::<PathBuf>).unwrap();
            context
                .font_atlas()
                .try_claim_legacy_renderer()
                .unwrap()
                .build();
            let clipboard = Rc::new(RefCell::new(String::new()));
            context.set_clipboard_backend(Clipboard(Rc::clone(&clipboard)));
            Self {
                context,
                panel: SqlitePanel::new(database.0.clone(), &state),
                clipboard,
                tab_positions: HashMap::new(),
                refresh_position: None,
                sql_pane: None,
            }
        }

        fn frame(&mut self) -> usize {
            self.context
                .prepare_frame(FramePrepareOptions::new([1000.0, 700.0], 1.0 / 60.0));
            let ui = self.context.frame();
            let host = HostContext {
                default_viewers: &Value::Null,
                viewer_menu: None,
                documents: &[],
                active_document: None,
                settings: &Value::Null,
                textures: &HashMap::new(),
                animations: false,
                workspace: 1,
                remote: false,
                diagnostics: &Value::Null,
            };
            let mut requests = Vec::new();
            ui.window("SQLite fixture")
                .position([0.0, 0.0], Condition::Always)
                .size([1000.0, 700.0], Condition::Always)
                .flags(WindowFlags::NO_TITLE_BAR)
                .build(|| self.panel.draw(ui, &host, &mut requests));
            self.tab_positions.clear();
            self.refresh_position = None;
            self.sql_pane = None;
            let refresh_width = ui.calc_text_size("Refresh")[0];
            // Use the rendered tabs' hitboxes so interaction tests survive font,
            // padding, and window-layout changes.
            ui.binding().with_bound_context(|| unsafe {
                let context = dear_imgui_sys::igGetCurrentContext();
                let bars = &(*context).TabBars.Buf;
                for index in 0..bars.Size {
                    let bar = bars.Data.add(index as usize);
                    if (*bar).CurrFrameVisible != (*context).FrameCount {
                        continue;
                    }
                    for index in 0..(*bar).Tabs.Size {
                        let tab = (*bar).Tabs.Data.add(index as usize);
                        let name = CStr::from_ptr(dear_imgui_sys::igTabBarGetTabName(bar, tab))
                            .to_string_lossy()
                            .into_owned();
                        self.tab_positions.insert(
                            name,
                            [
                                (*bar).BarRect.Min.x + (*tab).Offset - (*bar).ScrollingAnim
                                    + (*tab).Width / 2.0,
                                ((*bar).BarRect.Min.y + (*bar).BarRect.Max.y) / 2.0,
                            ],
                        );
                    }
                }
                let windows = &(*context).Windows;
                for index in 0..windows.Size {
                    let window = *windows.Data.add(index as usize);
                    if (*window).LastFrameActive != (*context).FrameCount {
                        continue;
                    }
                    let name = CStr::from_ptr((*window).Name).to_string_lossy();
                    if name == "SQLite fixture" {
                        // Clicking at the content's right edge and the native
                        // tab row verifies Refresh's alignment without fixed
                        // pixels or inspecting presentation colors.
                        let width = refresh_width + 2.0 * (*context).Style.FramePadding.x;
                        self.refresh_position = self
                            .tab_positions
                            .get("Tables")
                            .map(|tab| [(*window).WorkRect.Max.x - width / 2.0, tab[1]]);
                    }
                    if !name.contains("/sqlite-create_") {
                        continue;
                    }
                    self.sql_pane = Some(SqlPane {
                        center: [
                            ((*window).InnerRect.Min.x + (*window).InnerRect.Max.x) / 2.0,
                            ((*window).InnerRect.Min.y + (*window).InnerRect.Max.y) / 2.0,
                        ],
                        scroll: [(*window).Scroll.x, (*window).Scroll.y],
                        scroll_max: [(*window).ScrollMax.x, (*window).ScrollMax.y],
                        scrollbars: [(*window).ScrollbarX, (*window).ScrollbarY],
                    });
                }
            });
            assert!(
                requests
                    .iter()
                    .all(|request| matches!(request, HostRequest::Invalidate)),
                "read-only viewer emitted a document mutation"
            );
            self.context.render_legacy().total_vtx_count()
        }

        fn click_tab(&mut self, name: &str) {
            self.frame();
            let position = self.tab_positions[name];
            self.click_at(position);
        }

        fn click_refresh(&mut self) {
            self.frame();
            self.click_at(
                self.refresh_position
                    .expect("SQLite header was not rendered"),
            );
        }

        fn click_at(&mut self, position: [f32; 2]) {
            self.context.io_mut().add_mouse_pos_event(position);
            self.frame();
            self.context
                .io_mut()
                .add_mouse_button_event(dear_imgui_rs::MouseButton::Left, true);
            self.frame();
            self.context
                .io_mut()
                .add_mouse_button_event(dear_imgui_rs::MouseButton::Left, false);
            self.frame();
            self.frame();
        }

        fn data_frame(&mut self, action: Option<DataAction>) {
            self.context
                .prepare_frame(FramePrepareOptions::new([1000.0, 700.0], 1.0 / 60.0));
            let ui = self.context.frame();
            self.panel.poll();
            ui.window("SQLite paging fixture")
                .position([0.0, 0.0], Condition::Always)
                .size([1000.0, 700.0], Condition::Always)
                .flags(WindowFlags::NO_TITLE_BAR)
                .build(|| {
                    // Activate the public control labels through ImGui's native
                    // input path instead of assigning paging state directly.
                    if let Some(action) = action {
                        ui.binding().with_bound_context(|| unsafe {
                            let id = match action {
                                DataAction::Control(label) => {
                                    dear_imgui_sys::igGetID_Str(label.as_ptr())
                                }
                                DataAction::Page(label) => {
                                    let context = dear_imgui_sys::igGetCurrentContext();
                                    let popups = &(*context).OpenPopupStack;
                                    assert!(popups.Size > 0, "page selector is not open");
                                    let popup = popups.Data.add(popups.Size as usize - 1);
                                    assert!(!(*popup).Window.is_null());
                                    dear_imgui_sys::ImGuiWindow_GetID_Str(
                                        (*popup).Window,
                                        label.as_ptr(),
                                        std::ptr::null(),
                                    )
                                }
                            };
                            dear_imgui_sys::igActivateItemByID(id);
                        });
                    }
                    self.panel.data_tab(ui);
                });
            let _ = self.context.render_legacy();
        }

        fn activate_data(&mut self, action: DataAction) {
            self.data_frame(Some(action));
            self.data_frame(None);
            self.data_frame(None);
        }

        fn wait_data(&mut self) {
            let deadline = Instant::now() + Duration::from_secs(10);
            while self.panel.pending.is_some() {
                self.data_frame(None);
                assert!(
                    Instant::now() < deadline,
                    "SQLite paging request did not finish"
                );
                thread::sleep(Duration::from_millis(1));
            }
            assert!(self.panel.error().is_none(), "{:?}", self.panel.error());
        }

        fn wait(&mut self) {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                self.frame();
                if self.panel.pending.is_none() {
                    break;
                }
                assert!(Instant::now() < deadline, "SQLite worker did not finish");
                thread::sleep(Duration::from_millis(1));
            }
        }

        fn copy_selection(&mut self, row: bool) {
            self.context
                .prepare_frame(FramePrepareOptions::new([1000.0, 700.0], 1.0 / 60.0));
            let ui = self.context.frame();
            if row {
                self.panel.copy_row(ui, self.panel.selection.unwrap()[0]);
            } else {
                self.panel.copy_cell(ui);
            }
            let _ = self.context.render_legacy();
        }

        fn key(&mut self, key: Key, primary: bool) {
            if primary {
                self.context.io_mut().add_key_event(Key::ModCtrl, true);
            }
            self.context.io_mut().add_key_event(key, true);
            self.frame();
            self.context.io_mut().add_key_event(key, false);
            if primary {
                self.context.io_mut().add_key_event(Key::ModCtrl, false);
            }
            self.frame();
        }
    }

    #[test]
    fn restored_query_draft_stays_inert_and_explicit_writes_fail() {
        let _guard = IMGUI_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let database =
            Database::new("CREATE TABLE items(value); INSERT INTO items VALUES ('kept');");
        let saved = json!({"tab":"Query", "selected_table":"items", "sql":"DELETE FROM items"});
        let mut harness = Harness::new(&database, saved);
        harness.wait();
        for _ in 0..3 {
            harness.frame();
        }
        assert!(harness.panel.error().is_none());
        assert!(harness.panel.query.is_none());
        assert_eq!(harness.panel.save_state()["sql"], "DELETE FROM items");
        assert_eq!(harness.panel.save_state()["tab"], "Query");
        assert_eq!(harness.panel.attached_document(), None);
        harness.key(Key::Enter, true);
        harness.wait();
        assert!(
            harness.panel.error().is_some(),
            "write query must be rejected"
        );
        let count: i64 = Connection::open(&database.0)
            .unwrap()
            .query_row("SELECT count(*) FROM items", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn keyboard_run_cancel_and_rerun_preserve_the_sql_draft() {
        let _guard = IMGUI_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let database = Database::new("CREATE TABLE items(value);");
        let sql = "WITH RECURSIVE n(x) AS (VALUES(0) UNION ALL SELECT x+1 FROM n WHERE x<100000000) SELECT sum(x) FROM n";
        let mut harness = Harness::new(&database, json!({"tab":"Query", "sql":sql}));
        harness.wait();
        harness.key(Key::Enter, true);
        assert!(matches!(harness.panel.pending, Some(Pending::Query)));
        harness.key(Key::Escape, false);
        assert!(harness.panel.pending.is_none());
        assert_eq!(harness.panel.error(), Some("Cancelled."));
        assert_eq!(harness.panel.save_state()["sql"], sql);
        harness.panel.state.sql = "SELECT 42 AS answer".into();
        harness.key(Key::Enter, true);
        harness.wait();
        assert!(
            harness.panel.error().is_none(),
            "{:?}",
            harness.panel.error()
        );
        let result = harness
            .panel
            .query
            .as_ref()
            .expect("keyboard Run produced a result");
        assert_eq!(result.columns, ["answer"]);
        assert_eq!(result.rows[0][0], Cell::Integer(42));
    }

    #[test]
    fn wide_table_pages_columns_clips_rows_and_copies_inspected_values() {
        let _guard = IMGUI_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let definitions = (0..300)
            .map(|n| format!("column{n} INTEGER"))
            .collect::<Vec<_>>()
            .join(",");
        let values = vec!["x"; 300].join(",");
        let database = Database::new(&format!(
            "CREATE TABLE wide({definitions});
            WITH RECURSIVE numbers(x) AS (VALUES(0) UNION ALL SELECT x+1 FROM numbers WHERE x<599)
            INSERT INTO wide SELECT {values} FROM numbers;"
        ));
        let mut harness = Harness::new(&database, Value::Null);
        harness.wait();
        assert!(
            harness.panel.error().is_none(),
            "{:?}",
            harness.panel.error()
        );
        assert_eq!(harness.panel.browse.as_ref().unwrap().rows.len(), 200);
        let vertices = harness.frame();
        assert!(
            vertices > 100 && vertices < 100_000,
            "unbounded result geometry: {vertices}"
        );
        harness.panel.state.first_column = 255;
        harness.panel.selection = Some([3, 299]);
        harness.panel.inspector = true;
        harness.frame();
        harness.copy_selection(false);
        assert_eq!(&*harness.clipboard.borrow(), "3");
        harness.copy_selection(true);
        assert_eq!(harness.clipboard.borrow().split('\t').count(), 300);
    }

    #[test]
    fn schema_and_preview_render_null_unicode_binary_and_composite_keys() {
        let _guard = IMGUI_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let database = Database::new(
            "CREATE TABLE parent(a,b,PRIMARY KEY(a,b)) WITHOUT ROWID;
            INSERT INTO parent VALUES (1,2),(3,4);
            CREATE TABLE child(a,b, note TEXT, payload BLOB,
                FOREIGN KEY(a,b) REFERENCES parent(a,b));
            CREATE UNIQUE INDEX child_pair ON child(a,b);
            INSERT INTO child VALUES (1,2,'東京'||char(0)||char(10)||'##',x'00FF'),(3,4,NULL,x'');",
        );
        let mut harness = Harness::new(&database, json!({"selected_table":"child"}));
        harness.wait();
        assert!(harness.panel.error().is_none());
        harness.panel.selection = Some([0, 2]);
        harness.panel.inspector = true;
        harness.frame();
        harness.copy_selection(false);
        assert_eq!(&*harness.clipboard.borrow(), "東京␀\n##");
        harness.panel.selection = Some([1, 2]);
        harness.copy_selection(false);
        assert_eq!(&*harness.clipboard.borrow(), "NULL");
        let mut saved = harness.panel.save_state();
        saved["tab"] = json!("Schema");
        harness.panel = SqlitePanel::new(database.0.clone(), &saved);
        harness.wait();
        for _ in 0..3 {
            harness.frame();
        }
        assert_eq!(harness.panel.save_state()["tab"], "Schema");
        assert_eq!(harness.panel.save_state()["tables_tab"], "Schema");
        assert!(harness.frame() > 100);
    }

    #[test]
    fn nested_tables_remember_schema_when_query_is_restored_and_visited() {
        let _guard = IMGUI_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let database = Database::new("CREATE TABLE items(value); INSERT INTO items VALUES (1);");
        let mut harness = Harness::new(
            &database,
            json!({
                "tab":"Query", "tables_tab":"Schema", "selected_table":"items", "sql":"DELETE FROM items"
            }),
        );
        harness.wait();
        for _ in 0..3 {
            harness.frame();
        }
        assert_eq!(harness.panel.save_state()["tab"], "Query");
        assert_eq!(harness.panel.save_state()["tables_tab"], "Schema");
        assert!(harness.panel.query.is_none());
        Connection::open(&database.0)
            .unwrap()
            .execute_batch("CREATE TABLE added_after_open(value)")
            .unwrap();
        harness.click_refresh();
        harness.wait();
        assert!(
            harness
                .panel
                .schema
                .as_ref()
                .unwrap()
                .objects
                .iter()
                .any(|object| object.name == "added_after_open"),
            "right-aligned Refresh on the tab row did not reload schema"
        );
        assert_eq!(harness.panel.save_state()["tab"], "Query");
        assert_eq!(harness.panel.save_state()["sql"], "DELETE FROM items");
        assert!(harness.panel.query.is_none());
        harness.click_tab("Tables");
        assert_eq!(harness.panel.save_state()["tab"], "Schema");
        assert_eq!(harness.panel.save_state()["tables_tab"], "Schema");
        harness.click_tab("Query");
        assert_eq!(harness.panel.save_state()["tab"], "Query");
        harness.click_tab("Tables");
        assert_eq!(harness.panel.save_state()["tab"], "Schema");
        assert!(harness.panel.query.is_none());
        assert!(harness.panel.error().is_none());
    }

    #[test]
    fn schema_sql_scrolls_both_axes_and_keeps_its_position() {
        let _guard = IMGUI_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let columns = (0..60)
            .map(|index| {
                format!(
                    "column_{index:02} TEXT DEFAULT '{}'",
                    "wide schema definition ".repeat(12)
                )
            })
            .collect::<Vec<_>>()
            .join(",\n");
        let database = Database::new(&format!("CREATE TABLE long_schema (\n{columns}\n);"));
        let mut harness = Harness::new(
            &database,
            json!({"tab":"Schema", "selected_table":"long_schema"}),
        );
        harness.wait();
        for _ in 0..4 {
            harness.frame();
        }
        let pane = harness.sql_pane.expect("CREATE SQL pane was not rendered");
        assert!(pane.scroll_max.iter().all(|maximum| *maximum > 0.0));
        assert!(pane.scrollbars.iter().all(|visible| *visible));
        harness.context.io_mut().add_mouse_pos_event(pane.center);
        harness.frame();
        harness.context.io_mut().add_mouse_wheel_event([0.0, -3.0]);
        harness.frame();
        harness.frame();
        let vertical = harness.sql_pane.unwrap().scroll;
        assert!(
            vertical[1] > pane.scroll[1],
            "vertical wheel did not scroll SQL"
        );
        harness.context.io_mut().add_mouse_wheel_event([-4.0, 0.0]);
        harness.frame();
        harness.frame();
        let scrolled = harness.sql_pane.unwrap().scroll;
        assert!(
            scrolled[0] > vertical[0],
            "horizontal wheel did not scroll SQL"
        );
        assert_eq!(scrolled[1], vertical[1]);
        for _ in 0..4 {
            harness.frame();
            assert_eq!(harness.sql_pane.unwrap().scroll, scrolled);
        }
        assert!(harness.panel.error().is_none());
    }

    #[test]
    fn page_dropdown_jump_keeps_previous_and_next_row_ranges_contiguous() {
        let _guard = IMGUI_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let database = Database::new(
            "CREATE TABLE items(id INTEGER PRIMARY KEY);
            WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<650)
            INSERT INTO items SELECT x FROM n;",
        );
        let mut harness = Harness::new(&database, Value::Null);
        harness.wait();
        assert_eq!(harness.panel.total_rows, Some(650));
        harness.activate_data(DataAction::Control(c"Page"));
        harness.activate_data(DataAction::Page(c"3"));
        harness.wait_data();
        let data = harness.panel.browse.as_ref().unwrap();
        assert_eq!(data.rows.first().unwrap()[0], Cell::Integer(401));
        assert_eq!(data.rows.last().unwrap()[0], Cell::Integer(600));
        harness.activate_data(DataAction::Control(c"Previous page"));
        harness.wait_data();
        let data = harness.panel.browse.as_ref().unwrap();
        assert_eq!(data.rows.first().unwrap()[0], Cell::Integer(201));
        assert_eq!(data.rows.last().unwrap()[0], Cell::Integer(400));
        harness.activate_data(DataAction::Control(c"Next page"));
        harness.wait_data();
        let data = harness.panel.browse.as_ref().unwrap();
        assert_eq!(data.rows.first().unwrap()[0], Cell::Integer(401));
        assert_eq!(data.rows.last().unwrap()[0], Cell::Integer(600));
        harness.activate_data(DataAction::Control(c"Next page"));
        harness.wait_data();
        let data = harness.panel.browse.as_ref().unwrap();
        assert_eq!(data.rows.first().unwrap()[0], Cell::Integer(601));
        assert_eq!(data.rows.last().unwrap()[0], Cell::Integer(650));
        assert!(!data.has_more);
    }
}
