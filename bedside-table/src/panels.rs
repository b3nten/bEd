use super::*;
use bedtime::api::workbench::{Panel, PanelCreation, PanelResult, ShortcutInput};
use dear_imgui_rs::{
    InputTextCallback, InputTextCallbackHandler, InputTextMultilineFlags, ListClipper,
    TableColumnFlags, TableColumnWidth, TableFlags, TextCallbackData, Ui,
};

fn invoke(
    world: &World,
    source: Entity,
    id: &str,
    arguments: Value,
    requests: &mut Vec<WorkbenchAction>,
) {
    let mut invocation = bedtime_workbench::capture_command(world, id, arguments);
    invocation.source = Some(source);
    requests.push(WorkbenchAction::Invoke(invocation));
}

pub(super) fn create_tree(In(input): In<PanelCreation>) -> PanelResult {
    Ok(Box::new(TreePanel(input.entity)))
}
struct TreePanel(Entity);
impl Panel for TreePanel {
    fn title(&self, _: &World) -> String {
        "Databases".into()
    }
    fn draw(
        &mut self,
        ui: &Ui,
        world: &mut World,
        requests: &mut Vec<WorkbenchAction>,
        _: &ShortcutInput,
    ) {
        if ui.button("Open Database…") {
            invoke(world, self.0, "bedside.connect", Value::Null, requests);
        }
        ui.text_disabled("SQLite · read only");
        for notice in world.resource::<Notices>().0.iter().rev().take(3) {
            ui.text_wrapped(notice);
        }
        ui.separator();
        let connections = world.resource::<Connections>();
        if connections.entries.is_empty() {
            ui.text_wrapped("Open a local SQLite database to browse its tables and views.");
        }
        for connection in &connections.entries {
            let _id = ui.push_id(connection.record.id.0 as i32);
            let label = connection.record.label.replace("##", "# #");
            let selected = connections.selected == Some(connection.record.id);
            let node = ui
                .tree_node_config("database")
                .label(&label)
                .selected(selected)
                .default_open(true)
                .push();
            if ui.is_item_clicked() {
                invoke(
                    world,
                    self.0,
                    "bedside.select-connection",
                    json!({"connection":connection.record.id.0}),
                    requests,
                );
            }
            if let Some(_popup) = ui.begin_popup_context_item_with_label(Some("connection-menu")) {
                for (label, command) in [
                    ("New Query", "bedside.new-query"),
                    ("Refresh / Reconnect", "bedside.refresh"),
                    ("Disconnect", "bedside.disconnect"),
                ] {
                    if ui.menu_item(label) {
                        invoke(
                            world,
                            self.0,
                            command,
                            json!({"connection":connection.record.id.0}),
                            requests,
                        );
                    }
                }
            }
            if let Some(_node) = node {
                if connection.worker.is_none() {
                    ui.text_disabled("Disconnected");
                } else if connection.serial.is_some() {
                    ui.text_disabled("Loading schema…");
                }
                if let Some(error) = &connection.error {
                    ui.text_wrapped(error);
                }
                if let Some(schema) = &connection.schema {
                    for object in &schema.objects {
                        let _id = ui.push_id(&object.name);
                        if ui.selectable(format!(
                            "{}{}",
                            object.name.replace("##", "# #"),
                            if object.kind == bedtime_sqlite::ObjectKind::View {
                                " (view)"
                            } else {
                                ""
                            }
                        )) {
                            invoke(
                                world,
                                self.0,
                                "bedside.open-data",
                                json!({"connection":connection.record.id.0,"table":object.name}),
                                requests,
                            );
                        }
                        if let Some(_popup) =
                            ui.begin_popup_context_item_with_label(Some("table-menu"))
                        {
                            if ui.menu_item("Open Data") {
                                invoke(
                                    world,
                                    self.0,
                                    "bedside.open-data",
                                    json!({"connection":connection.record.id.0,"table":object.name}),
                                    requests,
                                );
                            }
                            if ui.menu_item("Open Schema") {
                                invoke(
                                    world,
                                    self.0,
                                    "bedside.open-schema",
                                    json!({"connection":connection.record.id.0,"table":object.name}),
                                    requests,
                                );
                            }
                        }
                    }
                }
                if ui.button("New Query") {
                    invoke(
                        world,
                        self.0,
                        "bedside.new-query",
                        json!({"connection":connection.record.id.0}),
                        requests,
                    );
                }
                for query in world
                    .resource::<QueryCatalog>()
                    .0
                    .iter()
                    .filter(|query| query.connection == connection.record.id)
                {
                    if ui.selectable(format!("{}##query-{}", query.title, query.id.0)) {
                        invoke(
                            world,
                            self.0,
                            "bedside.open-query",
                            json!({"query":query.id.0}),
                            requests,
                        );
                    }
                }
            }
        }
    }
    fn save(&self, _: &World) -> Value {
        json!({})
    }
}

pub(super) fn create_data(In(input): In<PanelCreation>, world: &mut World) -> PanelResult {
    let connection_id = ConnectionId(
        input.state["connection"]
            .as_i64()
            .ok_or("Missing database")?,
    );
    let table = input.state["table"]
        .as_str()
        .ok_or("Missing table name")?
        .to_owned();
    let offset = input.state["offset"].as_u64().unwrap_or(0) as usize;
    let connection = world
        .resource::<Connections>()
        .get(connection_id)
        .ok_or("Database is unavailable")?;
    let mut worker = connection
        .worker
        .as_ref()
        .map(|_| Worker::new(connection.record.path.clone()));
    let serial = worker.as_mut().map(|worker| {
        worker.request(Request::Browse {
            table: table.clone(),
            offset,
        })
    });
    let generation = connection.generation;
    world.resource_mut::<DataPanels>().0.insert(
        input.entity,
        DataState {
            connection: connection_id,
            table,
            worker,
            generation,
            serial,
            offset,
            previous_offsets: input.state["previous_offsets"]
                .as_array()
                .map(|offsets| {
                    offsets
                        .iter()
                        .filter_map(Value::as_u64)
                        .map(|offset| offset as usize)
                        .collect()
                })
                .unwrap_or_default(),
            result: None,
            error: None,
        },
    );
    Ok(Box::new(DataPanel(input.entity)))
}
struct DataPanel(Entity);
impl Panel for DataPanel {
    fn title(&self, world: &World) -> String {
        world.resource::<DataPanels>().0.get(&self.0).map_or_else(
            || "Table Data".into(),
            |state| format!("{} · Data", state.table),
        )
    }
    fn draw(
        &mut self,
        ui: &Ui,
        world: &mut World,
        requests: &mut Vec<WorkbenchAction>,
        _: &ShortcutInput,
    ) {
        let Some(state) = world.resource::<DataPanels>().0.get(&self.0) else {
            return;
        };
        if let Some(connection) = world.resource::<Connections>().get(state.connection) {
            ui.text_disabled(&connection.record.label);
        }
        if ui.button("Schema") {
            invoke(
                world,
                self.0,
                "bedside.open-schema",
                json!({"connection":state.connection.0,"table":state.table}),
                requests,
            );
        }
        ui.same_line();
        if ui.button("Refresh") {
            invoke(
                world,
                self.0,
                "bedside.page",
                json!({"offset":state.offset}),
                requests,
            );
        }
        ui.same_line();
        if ui.button_with_size("Previous", [0.0, 0.0]) && state.offset > 0 {
            invoke(
                world,
                self.0,
                "bedside.page",
                json!({"offset":state.previous_offsets.last().copied().unwrap_or_else(|| state.offset.saturating_sub(bedtime_sqlite::PAGE_SIZE))}),
                requests,
            );
        }
        ui.same_line();
        if ui.button("Next") && state.result.as_ref().is_some_and(|data| data.has_more) {
            let offset = state.offset + state.result.as_ref().unwrap().rows.len();
            invoke(
                world,
                self.0,
                "bedside.page",
                json!({"offset":offset}),
                requests,
            );
        }
        if state.serial.is_some() {
            ui.text_disabled("Loading…");
        }
        if let Some(error) = &state.error {
            ui.text_wrapped(error);
        }
        if let Some(data) = &state.result {
            draw_data(ui, data, state.offset);
        }
    }
    fn save(&self, world: &World) -> Value {
        let state = &world.resource::<DataPanels>().0[&self.0];
        json!({"connection":state.connection.0,"table":state.table,"offset":state.offset,"previous_offsets":state.previous_offsets})
    }
}

pub(super) fn create_schema(In(input): In<PanelCreation>, world: &mut World) -> PanelResult {
    let connection = ConnectionId(
        input.state["connection"]
            .as_i64()
            .ok_or("Missing database")?,
    );
    if world.resource::<Connections>().get(connection).is_none() {
        return Err("Database is unavailable".into());
    }
    Ok(Box::new(SchemaPanel {
        entity: input.entity,
        connection,
        table: input.state["table"]
            .as_str()
            .ok_or("Missing table name")?
            .to_owned(),
    }))
}
struct SchemaPanel {
    entity: Entity,
    connection: ConnectionId,
    table: String,
}
impl Panel for SchemaPanel {
    fn title(&self, _: &World) -> String {
        format!("{} · Schema", self.table)
    }
    fn draw(
        &mut self,
        ui: &Ui,
        world: &mut World,
        requests: &mut Vec<WorkbenchAction>,
        _: &ShortcutInput,
    ) {
        let Some(connection) = world.resource::<Connections>().get(self.connection) else {
            ui.text_wrapped("Database is unavailable");
            return;
        };
        ui.text_disabled(&connection.record.label);
        if ui.button("Data") {
            invoke(
                world,
                self.entity,
                "bedside.open-data",
                json!({"connection":self.connection.0,"table":self.table}),
                requests,
            );
        }
        if connection.worker.is_none() {
            ui.text_wrapped("Database is disconnected");
            return;
        }
        if let Some(error) = &connection.error {
            ui.text_wrapped(error);
        }
        let Some(object) = connection.schema.as_ref().and_then(|schema| {
            schema
                .objects
                .iter()
                .find(|object| object.name == self.table)
        }) else {
            ui.text_wrapped(if connection.serial.is_some() {
                "Loading schema…"
            } else {
                "Table or view is unavailable; refresh the database"
            });
            return;
        };
        if let Some(sql) = &object.create_sql {
            ui.separator();
            ui.text_wrapped(sql);
        }
        ui.separator();
        ui.text("Columns");
        for column in &object.columns {
            ui.text_wrapped(format!(
                "{}: {}{}{}",
                column.name,
                column.declared_type,
                if column.not_null { " · NOT NULL" } else { "" },
                if column.primary_key_position > 0 {
                    " · PRIMARY KEY"
                } else {
                    ""
                }
            ));
            if let Some(default) = &column.default_value {
                ui.text_disabled(format!("Default: {default}"));
            }
        }
        if !object.indexes.is_empty() {
            ui.separator();
            ui.text("Indexes");
        }
        for index in &object.indexes {
            ui.text_wrapped(format!(
                "{}{} ({})",
                index.name,
                if index.unique { " · UNIQUE" } else { "" },
                index
                    .columns
                    .iter()
                    .map(|column| column.as_deref().unwrap_or("expression"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if !object.foreign_keys.is_empty() {
            ui.separator();
            ui.text("Foreign keys");
        }
        for key in &object.foreign_keys {
            ui.text_wrapped(format!(
                "{} → {}.{}",
                key.from,
                key.table,
                key.to.as_deref().unwrap_or("primary key")
            ));
        }
    }
    fn save(&self, _: &World) -> Value {
        json!({"connection":self.connection.0,"table":self.table})
    }
}

pub(super) fn create_query(In(input): In<PanelCreation>, world: &mut World) -> PanelResult {
    let record = query_record(
        world,
        QueryId(input.state["query"].as_i64().ok_or("Missing saved query")?),
    )?;
    let document = ensure_query_document(world, &record)?;
    let sql = bedtime_core::model::<QueryDocument>(world, document)
        .unwrap()
        .sql
        .clone();
    let revision = bedtime_core::model_document(world, document)
        .unwrap()
        .revision();
    let connection = world
        .resource::<Connections>()
        .get(record.connection)
        .ok_or("Database is unavailable")?;
    let generation = connection.generation;
    let action = world.register_system(query_action);
    world
        .entity_mut(input.entity)
        .insert((DocumentBinding(document), PanelActions(action)));
    world.resource_mut::<QueryPanels>().0.insert(
        input.entity,
        QueryState {
            query: record.id,
            connection: record.connection,
            document,
            buffer: sql,
            revision,
            draft: false,
            worker: None,
            generation,
            serial: None,
            result: None,
            error: None,
            input_center: None,
            input_active: false,
        },
    );
    Ok(Box::new(QueryPanel {
        entity: input.entity,
        title: record.title,
    }))
}
struct QueryPanel {
    entity: Entity,
    title: String,
}

struct SynchronizeInput(Option<String>);
impl InputTextCallbackHandler for SynchronizeInput {
    fn on_always(&mut self, mut data: TextCallbackData<'_>) {
        if let Some(sql) = self.0.take() {
            let old_position = data.cursor_pos();
            let len = data.str().len();
            data.remove_chars(0, len);
            data.insert_chars(0, &sql);
            let mut position = old_position.min(sql.len());
            while !sql.is_char_boundary(position) {
                position -= 1;
            }
            data.set_cursor_pos(position);
            data.clear_selection();
        }
    }
}
impl Panel for QueryPanel {
    fn title(&self, _: &World) -> String {
        self.title.clone()
    }
    fn draw(
        &mut self,
        ui: &Ui,
        world: &mut World,
        requests: &mut Vec<WorkbenchAction>,
        shortcuts: &ShortcutInput,
    ) {
        world.resource_scope(|world, mut panels: Mut<QueryPanels>| {
            let Some(state) = panels.0.get_mut(&self.entity) else {
                return;
            };
            let Some(document) = bedtime_core::model::<QueryDocument>(world, state.document) else {
                return;
            };
            let info = bedtime_core::model_document(world, state.document).unwrap();
            let mut synchronize = None;
            if !state.draft && state.revision != info.revision() {
                // ImGui retains active text between frames. Keep enough backing
                // capacity for that text until the callback replaces it.
                state.buffer.clear();
                state.buffer.push_str(&document.sql);
                state.revision = info.revision();
                synchronize = Some(state.buffer.clone());
            }
            let canonical = document.sql.clone();
            if let Some(connection) = world.resource::<Connections>().get(state.connection) {
                ui.text_disabled(format!("{} · read-only SQL", connection.record.label));
            }
            if ui
                .input_text_multiline("##query-sql", &mut state.buffer, [-1.0, 150.0])
                .flags(
                    InputTextMultilineFlags::NO_UNDO_REDO
                        | InputTextMultilineFlags::ALLOW_TAB_INPUT,
                )
                .callback(InputTextCallback::ALWAYS, SynchronizeInput(synchronize))
                .build()
            {
                state.draft = state.buffer != canonical;
                if state.draft {
                    invoke(
                        world,
                        self.entity,
                        "bedside.edit-query",
                        Value::Null,
                        requests,
                    );
                }
            }
            let min = ui.item_rect_min();
            let max = ui.item_rect_max();
            state.input_center = Some([(min[0] + max[0]) * 0.5, (min[1] + max[1]) * 0.5]);
            state.input_active = ui.is_item_active();
            if ui.button("Run Query") || shortcuts.take(ui, "bedside.run-query") {
                invoke(
                    world,
                    self.entity,
                    "bedside.run-query",
                    Value::Null,
                    requests,
                );
            }
            ui.same_line();
            if ui.button("Save") {
                invoke(
                    world,
                    self.entity,
                    "bedtime.documents.save",
                    json!({"document":state.document.0}),
                    requests,
                );
            }
            ui.same_line();
            if ui.button("Undo") || shortcuts.take(ui, "bedtime.edit.undo") {
                invoke(
                    world,
                    self.entity,
                    "bedtime.documents.undo",
                    Value::Null,
                    requests,
                );
            }
            ui.same_line();
            if ui.button("Redo") || shortcuts.take(ui, "bedtime.edit.redo") {
                invoke(
                    world,
                    self.entity,
                    "bedtime.documents.redo",
                    Value::Null,
                    requests,
                );
            }
            if let Some(serial) = state.serial {
                ui.same_line();
                if ui.button("Cancel") || shortcuts.take(ui, "bedside.cancel-query") {
                    invoke(
                        world,
                        self.entity,
                        "bedside.cancel-query",
                        json!({"serial":serial,"generation":state.generation}),
                        requests,
                    );
                }
                ui.text_disabled("Running…");
            }
            if let Some(error) = &state.error {
                ui.text_wrapped(error);
            }
            if let Some(data) = &state.result {
                draw_data(ui, data, 0);
            }
        });
    }
    fn save(&self, world: &World) -> Value {
        json!({"query":world.resource::<QueryPanels>().0[&self.entity].query.0})
    }
}

fn draw_data(ui: &Ui, data: &DataSet, offset: usize) {
    ui.text_disabled(format!(
        "{} rows · {} columns · {:.3}s{}",
        data.rows.len(),
        data.columns.len(),
        data.elapsed.as_secs_f64(),
        if data.truncated {
            " · result limit reached"
        } else {
            ""
        }
    ));
    let columns = data.columns.len().min(255);
    if columns == 0 {
        return;
    }
    if data.columns.len() > columns {
        ui.text_wrapped("Showing the first 255 columns.");
    }
    let Some(_table) = ui.begin_table_with_sizing(
        "results",
        columns + 1,
        TableFlags::RESIZABLE
            | TableFlags::SCROLL_X
            | TableFlags::SCROLL_Y
            | TableFlags::BORDERS_INNER
            | TableFlags::ROW_BG,
        [0.0, ui.content_region_avail()[1].max(60.0)],
        0.0,
    ) else {
        return;
    };
    ui.table_setup_column(
        "#",
        TableColumnFlags::NO_RESIZE,
        Some(TableColumnWidth::Fixed(55.0)),
    );
    for (index, column) in data.columns.iter().take(columns).enumerate() {
        ui.table_setup_column(
            format!("{}##{index}", column.replace("##", "# #")),
            TableColumnFlags::NONE,
            Some(TableColumnWidth::Fixed(150.0)),
        );
    }
    ui.table_setup_scroll_freeze(1, 1);
    ui.table_headers_row();
    for row in ListClipper::new(data.rows.len()).begin(ui).iter() {
        let _row = ui.push_id(row);
        ui.table_next_row();
        ui.table_next_column();
        ui.text((offset + row + 1).to_string());
        for (column, cell) in data.rows[row].iter().take(columns).enumerate() {
            if !ui.table_next_column() {
                continue;
            }
            let _column = ui.push_id(column);
            let text = cell.display_text();
            let mut preview = text.replace('\n', "↵").replace('\r', "").replace('\t', "⇥");
            if preview.chars().count() > 160 {
                preview = preview.chars().take(160).collect::<String>() + "…";
            }
            ui.text(&preview);
            if ui.is_item_hovered() {
                bedtime::ui::components::tooltip_text(
                    ui,
                    &bedtime::ui::UiFrameContext::for_ui(ui),
                    format!(
                        "{}{}\n{text}",
                        cell.kind_name(),
                        if cell.is_truncated() {
                            " · value preview"
                        } else {
                            ""
                        }
                    ),
                );
            }
            if let Some(_popup) = ui.begin_popup_context_item_with_label(Some("cell-menu"))
                && ui.menu_item(if cell.is_truncated() {
                    "Copy preview"
                } else {
                    "Copy cell"
                })
            {
                let text = std::ffi::CString::new(text.replace('\0', "␀")).unwrap();
                ui.binding().with_bound_context(|| unsafe {
                    dear_imgui_rs::sys::igSetClipboardText(text.as_ptr());
                });
            }
        }
    }
}
