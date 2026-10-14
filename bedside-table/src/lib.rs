//! Bedside Table's SQL application policy, composed on the standard workbench.
mod automation;
mod panels;
pub mod storage;
#[cfg(test)]
mod test_fixtures;

use bedtime::api::workbench::{
    CommandInvocation, CommandResult, PanelAction, PanelActionInvocation, PanelActionResult,
    PanelActions,
};
use bedtime_core::bedtime_documents::DocumentId;
use bedtime_core::{DocumentBinding, DocumentModel, ModelDocument};
use bedtime_sqlite::{DataSet, Request, Response, Schema, Worker};
use bedtime_storage::{StateScope, Storage};
use bedtime_workbench::WorkbenchAction;
use bevy::prelude::*;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use storage::{ConnectionId, ConnectionRecord, MetadataStore, QueryId, QueryRecord};

pub const TREE_PANEL: &str = "bedside.connections";
pub const DATA_PANEL: &str = "bedside.table-data";
pub const SCHEMA_PANEL: &str = "bedside.table-schema";
pub const QUERY_PANEL: &str = "bedside.query";
const SESSION_SCOPE: &str = "bedside.session";

#[derive(Clone)]
pub struct QueryDocument {
    pub sql: String,
}
#[derive(Clone)]
pub struct ReplaceSql(pub String);
impl DocumentModel for QueryDocument {
    type Change = ReplaceSql;
    fn apply(&mut self, change: &ReplaceSql) -> Result<ReplaceSql, String> {
        if change.0.len() > 1024 * 1024 {
            return Err("SQL text exceeds the 1 MiB query limit".into());
        }
        Ok(ReplaceSql(std::mem::replace(
            &mut self.sql,
            change.0.clone(),
        )))
    }
    fn encode(&self) -> Result<Vec<u8>, String> {
        Ok(self.sql.as_bytes().to_vec())
    }
}

#[derive(Component)]
struct QueryIdentity(QueryId);

struct ConnectionState {
    record: ConnectionRecord,
    worker: Option<Worker>,
    serial: Option<u64>,
    generation: u64,
    schema: Option<Schema>,
    error: Option<String>,
}
impl ConnectionState {
    fn new(record: ConnectionRecord) -> Self {
        let mut worker = Worker::new(record.path.clone());
        let serial = worker.request(Request::Schema);
        Self {
            record,
            worker: Some(worker),
            serial: Some(serial),
            generation: 1,
            schema: None,
            error: None,
        }
    }
    fn refresh(&mut self) {
        if self.worker.is_none() {
            self.worker = Some(Worker::new(self.record.path.clone()));
        }
        self.generation += 1;
        self.error = None;
        self.serial = Some(self.worker.as_mut().unwrap().request(Request::Schema));
    }
}

#[derive(Resource, Default)]
struct Connections {
    entries: Vec<ConnectionState>,
    selected: Option<ConnectionId>,
}
impl Connections {
    fn get(&self, id: ConnectionId) -> Option<&ConnectionState> {
        self.entries
            .iter()
            .find(|connection| connection.record.id == id)
    }
    fn get_mut(&mut self, id: ConnectionId) -> Option<&mut ConnectionState> {
        self.entries
            .iter_mut()
            .find(|connection| connection.record.id == id)
    }
}

#[derive(Resource, Default)]
struct QueryCatalog(Vec<QueryRecord>);

struct DataState {
    connection: ConnectionId,
    table: String,
    worker: Option<Worker>,
    generation: u64,
    serial: Option<u64>,
    offset: usize,
    previous_offsets: Vec<usize>,
    result: Option<DataSet>,
    error: Option<String>,
}
#[derive(Resource, Default)]
struct DataPanels(HashMap<Entity, DataState>);

struct QueryState {
    query: QueryId,
    connection: ConnectionId,
    document: DocumentId,
    buffer: String,
    revision: u64,
    draft: bool,
    worker: Option<Worker>,
    generation: u64,
    serial: Option<u64>,
    result: Option<DataSet>,
    error: Option<String>,
    input_center: Option<[f32; 2]>,
    input_active: bool,
}
#[derive(Resource, Default)]
struct QueryPanels(HashMap<Entity, QueryState>);

#[derive(Resource, Default)]
struct Notices(Vec<String>);
#[derive(Resource)]
struct SessionPersistence {
    previous: Option<Value>,
    changed_at: Option<Instant>,
    active: bool,
}

pub struct BedsidePlugin {
    pub metadata: MetadataStore,
}
impl Plugin for BedsidePlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(self.metadata.clone())
            .insert_resource(Storage(Box::new(self.metadata.clone())))
            .insert_resource(bedtime_core::DocumentCloseDecisionPolicy::Ask)
            .init_resource::<Connections>()
            .init_resource::<QueryCatalog>()
            .init_resource::<DataPanels>()
            .init_resource::<QueryPanels>()
            .init_resource::<Notices>()
            .insert_resource(SessionPersistence {
                previous: None,
                changed_at: None,
                active: false,
            });
        bedtime_workbench::register(app.world_mut(), "bedside", |registry| {
            registry.panel(TREE_PANEL, "Databases", panels::create_tree);
            registry.panel(DATA_PANEL, "Table Data", panels::create_data);
            registry.panel(SCHEMA_PANEL, "Table Schema", panels::create_schema);
            registry.panel(QUERY_PANEL, "SQL Query", panels::create_query);
            for (id, label) in [
                ("bedside.connect", "Open Database…"),
                ("bedside.new-query", "New SQL Query"),
                ("bedside.open-query", "Open Saved Query"),
                ("bedside.select-connection", "Select Database"),
                ("bedside.refresh", "Refresh Database"),
                ("bedside.disconnect", "Disconnect Database"),
                ("bedside.open-data", "Open Table Data"),
                ("bedside.open-schema", "Open Table Schema"),
                ("bedside.page", "Change Data Page"),
                ("bedside.edit-query", "Edit SQL Query"),
                ("bedside.run-query", "Run SQL Query"),
                ("bedside.cancel-query", "Cancel SQL Query"),
            ] {
                registry.command(id, label, command);
            }
            registry.global_shortcut("bedside.connect", &["Primary+Shift+O"]);
            registry.global_shortcut("bedside.new-query", &["Primary+Shift+N"]);
            registry.panel_shortcut(
                "bedside.run-query",
                "Run Query",
                QUERY_PANEL,
                &["Primary+Enter"],
            );
            registry.panel_shortcut(
                "bedside.cancel-query",
                "Cancel Query",
                QUERY_PANEL,
                &["Escape"],
            );
            registry.menu("bedside.connect");
            registry.menu("bedside.new-query");
            registry.toolbar("bedside.connect");
            registry.toolbar("bedside.new-query");
        })
        .expect("Bedside declarations register once");
        app.world_mut()
            .resource_mut::<bedtime::api::features::FeatureRegistry>()
            .register_with("bedside", |registry| {
                for id in [DATA_PANEL, SCHEMA_PANEL, QUERY_PANEL] {
                    registry.panel_ref(id);
                    registry.panel_requires_input(id);
                }
                registry.panel_options(
                    TREE_PANEL,
                    true,
                    bedtime::api::workbench::PanelPlacement::Sidebar,
                );
            })
            .expect("Bedside metadata registers once");
        use bedtime::api::menus::{Menu, MenuCommand, MenuEntry, MenuLayout};
        let item = |id| MenuEntry::Command(MenuCommand::new(id));
        app.insert_resource(MenuLayout(vec![
            Menu::new(
                "Bedside Table",
                vec![MenuEntry::Quit(
                    MenuCommand::new("bedtime.core.quit").labeled("Quit Bedside Table"),
                )],
            ),
            Menu::new(
                "File",
                vec![
                    item("bedside.connect"),
                    item("bedside.new-query"),
                    MenuEntry::Separator,
                    item("bedtime.documents.save"),
                    item("bedtime.core.close"),
                ],
            ),
            Menu::new(
                "Edit",
                vec![
                    item("bedtime.documents.undo"),
                    item("bedtime.documents.redo"),
                ],
            ),
            Menu::new(
                "Database",
                vec![item("bedside.refresh"), item("bedside.disconnect")],
            ),
        ]));
        // The app contributes its persistence to the shared admitted lifecycle.
        // A failed metadata commit keeps the application live.
        let persist = app.world_mut().register_system(persist_before_transition);
        app.world_mut()
            .resource_mut::<bedtime_core::LifecyclePreCommit>()
            .0
            .push(persist);
        app.add_systems(Startup, restore)
            .add_systems(
                PreUpdate,
                poll.in_set(bedtime::api::features::FeatureSystems::Prepare),
            )
            .add_systems(Last, autosave_session);
    }
}

fn ensure_query_document(world: &mut World, record: &QueryRecord) -> Result<DocumentId, String> {
    if let Some(id) = world
        .query::<(&QueryIdentity, &ModelDocument)>()
        .iter(world)
        .find_map(|(key, document)| (key.0 == record.id).then_some(document.id()))
    {
        return Ok(id);
    }
    let store = world.resource::<MetadataStore>().clone();
    let current = store
        .queries()
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|query| query.id == record.id)
        .ok_or("Saved query is unavailable")?;
    if current.sql.len() > 1024 * 1024 {
        return Err("Saved SQL exceeds the 1 MiB query limit".into());
    }
    let document = bedtime_core::create_model_document(
        world,
        QueryDocument { sql: current.sql },
        current.title,
        None,
    );
    bedtime_core::bind_model_document_writer(
        world,
        document,
        record.id.0.to_string(),
        Arc::new(store),
    )?;
    world
        .entity_mut(bedtime_core::model_entity(world, document).unwrap())
        .insert(QueryIdentity(record.id));
    Ok(document)
}

fn restore(world: &mut World) {
    let result = (|| -> Result<(), String> {
        let store = world.resource::<MetadataStore>().clone();
        let connections = store.connections().map_err(|error| error.to_string())?;
        let selected = connections.first().map(|connection| connection.id);
        world.insert_resource(Connections {
            entries: connections.into_iter().map(ConnectionState::new).collect(),
            selected,
        });
        let queries = store.queries().map_err(|error| error.to_string())?;
        for query in &queries {
            ensure_query_document(world, query)?;
        }
        world.insert_resource(QueryCatalog(queries));
        let saved = world
            .resource::<Storage>()
            .0
            .load(&StateScope(SESSION_SCOPE.into()))
            .map_err(|error| error.to_string())?;
        if let Some(saved) = saved {
            let layout = saved.get("layout").ok_or("Saved session has no layout")?;
            if let Some(selected) = saved["selected_connection"].as_i64().map(ConnectionId)
                && world.resource::<Connections>().get(selected).is_some()
            {
                world.resource_mut::<Connections>().selected = Some(selected);
            }
            bedtime_workbench::restore(world, layout)?;
            world.resource_mut::<SessionPersistence>().previous = Some(saved);
        } else {
            use bedtime_workbench::workspace::tiling::{Area, Layout, Rect};
            use bedtime_workbench::workspace::tiling_state::TilingState;
            let geometry = Layout::from_areas(vec![
                Area {
                    id: 1,
                    rect: Rect {
                        min: [0, 0],
                        max: [2500, 10000],
                    },
                },
                Area {
                    id: 2,
                    rect: Rect {
                        min: [2500, 0],
                        max: [10000, 10000],
                    },
                },
            ])
            .expect("Bedside's initial layout is a complete rectangle");
            let mut tiling = TilingState::new(geometry);
            tiling.assign(1, 1);
            bedtime_workbench::restore(
                world,
                &json!({
                    "version":1,"panels":[{"id":1,"panel_type":TREE_PANEL,"state":{}}],
                    "focused":1,"focused_area":1,"tiling":tiling.to_value([1]),
                }),
            )?;
        }
        world.resource_mut::<SessionPersistence>().active = true;
        Ok(())
    })();
    if let Err(error) = result {
        world.write_message(bedtime_core::CoreNotice {
            source: None,
            message: format!("Could not restore Bedside: {error}"),
        });
        world.resource_mut::<Notices>().0.push(error);
    }
}

fn session_snapshot(world: &mut World) -> Value {
    json!({"layout":bedtime_workbench::save(world),"selected_connection":world.resource::<Connections>().selected.map(|id| id.0)})
}
pub fn persist_session(world: &mut World) -> Result<(), String> {
    if !world.resource::<SessionPersistence>().active {
        return Ok(());
    }
    let snapshot = session_snapshot(world);
    world
        .resource_mut::<Storage>()
        .0
        .save(&StateScope(SESSION_SCOPE.into()), &snapshot)
        .map_err(|error| error.to_string())?;
    world.resource_mut::<SessionPersistence>().previous = Some(snapshot);
    world.resource_mut::<SessionPersistence>().changed_at = None;
    Ok(())
}
fn persist_before_transition(
    In(_reason): In<bedtime_core::LifecycleReason>,
    world: &mut World,
) -> Result<(), String> {
    persist_session(world)
}
fn autosave_session(world: &mut World) {
    if !world.resource::<SessionPersistence>().active {
        return;
    }
    let snapshot = session_snapshot(world);
    let mut persistence = world.resource_mut::<SessionPersistence>();
    if persistence.previous.as_ref() == Some(&snapshot) {
        persistence.changed_at = None;
        return;
    }
    let since = persistence.changed_at.get_or_insert_with(Instant::now);
    if since.elapsed() < Duration::from_millis(750) {
        return;
    }
    if let Err(error) = persist_session(world) {
        world.resource_mut::<SessionPersistence>().changed_at = Some(Instant::now());
        world
            .resource_mut::<Notices>()
            .0
            .push(format!("Could not save the session: {error}"));
    }
}

fn connection_argument(world: &World, input: &CommandInvocation) -> Result<ConnectionId, String> {
    input.arguments["connection"]
        .as_i64()
        .map(ConnectionId)
        .or_else(|| {
            let source = input.source?;
            let source = world
                .get::<bedtime::api::workbench::PanelFrontend>(source)
                .map_or(source, |frontend| frontend.0);
            world
                .resource::<QueryPanels>()
                .0
                .get(&source)
                .map(|state| state.connection)
                .or_else(|| {
                    world
                        .resource::<DataPanels>()
                        .0
                        .get(&source)
                        .map(|state| state.connection)
                })
                .or_else(|| {
                    world
                        .get::<bedtime_workbench::PanelRecord>(source)
                        .and_then(|record| {
                            record.serialized["state"]["connection"]
                                .as_i64()
                                .map(ConnectionId)
                        })
                })
        })
        .or(world.resource::<Connections>().selected)
        .ok_or_else(|| "Open a database first".into())
}
fn query_record(world: &World, id: QueryId) -> Result<QueryRecord, String> {
    world
        .resource::<QueryCatalog>()
        .0
        .iter()
        .find(|record| record.id == id)
        .cloned()
        .ok_or_else(|| "Query is unavailable".into())
}

fn command(In(mut input): In<CommandInvocation>, world: &mut World) -> CommandResult {
    input.source = input.source.map(|source| {
        world
            .get::<bedtime::api::workbench::PanelFrontend>(source)
            .map_or(source, |frontend| frontend.0)
    });
    match input.command.as_str() {
        "bedside.connect" => {
            let path = match input.arguments["path"].as_str() {
                Some(path) => Some(PathBuf::from(path)),
                None => rfd::FileDialog::new()
                    .add_filter("SQLite", &["db", "sqlite", "sqlite3"])
                    .pick_file(),
            };
            if let Some(path) = path {
                let record = world
                    .resource::<MetadataStore>()
                    .add_connection(&path)
                    .map_err(|error| error.to_string())?;
                let mut connections = world.resource_mut::<Connections>();
                let id = record.id;
                if let Some(connection) = connections.get_mut(id) {
                    connection.refresh();
                } else {
                    connections.entries.push(ConnectionState::new(record));
                }
                connections.selected = Some(id);
            }
        }
        "bedside.select-connection" => {
            let id = connection_argument(world, &input)?;
            if world.resource::<Connections>().get(id).is_none() {
                return Err("Database is unavailable".into());
            }
            world.resource_mut::<Connections>().selected = Some(id);
        }
        "bedside.refresh" | "bedside.disconnect" => {
            let id = connection_argument(world, &input)?;
            let connection = world.resource_mut::<Connections>();
            let mut connection = connection;
            let state = connection.get_mut(id).ok_or("Database is unavailable")?;
            if input.command == "bedside.disconnect" {
                state.worker = None;
                state.serial = None;
                state.generation += 1;
            } else {
                state.refresh();
            }
        }
        "bedside.new-query" | "bedside.open-query" => {
            let record = if input.command == "bedside.new-query" {
                let connection = connection_argument(world, &input)?;
                let record = world
                    .resource::<MetadataStore>()
                    .new_query(connection)
                    .map_err(|error| error.to_string())?;
                world.resource_mut::<QueryCatalog>().0.push(record.clone());
                record
            } else {
                query_record(
                    world,
                    QueryId(input.arguments["query"].as_i64().ok_or("Missing query")?),
                )?
            };
            if input.command == "bedside.open-query"
                && let Some(entity) =
                    world
                        .resource::<QueryPanels>()
                        .0
                        .iter()
                        .find_map(|(entity, state)| {
                            (state.query == record.id && world.entities().contains(*entity))
                                .then_some(*entity)
                        })
            {
                return Ok(vec![WorkbenchAction::FocusPanel(entity)]);
            }
            ensure_query_document(world, &record)?;
            return Ok(vec![WorkbenchAction::OpenPanel {
                panel_type: QUERY_PANEL.into(),
                state: json!({"query":record.id.0}),
                area: None,
            }]);
        }
        "bedside.open-data" | "bedside.open-schema" => {
            let connection = connection_argument(world, &input)?;
            if world.resource::<Connections>().get(connection).is_none() {
                return Err("Database is unavailable".into());
            }
            world.resource_mut::<Connections>().selected = Some(connection);
            let table = input.arguments["table"]
                .as_str()
                .ok_or("Missing table name")?;
            let panel_type = if input.command == "bedside.open-data" {
                DATA_PANEL
            } else {
                SCHEMA_PANEL
            };
            if let Some(entity) = world
                .query::<(Entity, &bedtime_workbench::PanelRecord)>()
                .iter(world)
                .find_map(|(entity, record)| {
                    (record.panel_type == panel_type
                        && record.serialized["state"]["connection"].as_i64() == Some(connection.0)
                        && record.serialized["state"]["table"].as_str() == Some(table))
                    .then_some(entity)
                })
            {
                return Ok(vec![WorkbenchAction::FocusPanel(entity)]);
            }
            return Ok(vec![WorkbenchAction::OpenPanel {
                panel_type: panel_type.into(),
                state: json!({"connection":connection.0,"table":table,"offset":0}),
                area: None,
            }]);
        }
        "bedside.page" => {
            let entity = input.source.ok_or("Missing data panel")?;
            let state = world
                .resource::<DataPanels>()
                .0
                .get(&entity)
                .ok_or("Data panel is unavailable")?;
            let connection = world
                .resource::<Connections>()
                .get(state.connection)
                .ok_or("Database is unavailable")?;
            if connection.worker.is_none() {
                return Err("Database is disconnected".into());
            }
            let offset = input.arguments["offset"]
                .as_u64()
                .ok_or("Missing page offset")? as usize;
            let mut panels = world.resource_mut::<DataPanels>();
            let state = panels.0.get_mut(&entity).unwrap();
            if offset > state.offset {
                state.previous_offsets.push(state.offset);
            } else if offset < state.offset {
                state.previous_offsets.pop();
            }
            state.offset = offset;
            state.error = None;
            state.serial = Some(
                state
                    .worker
                    .as_mut()
                    .ok_or("Database is disconnected")?
                    .request(Request::Browse {
                        table: state.table.clone(),
                        offset,
                    }),
            );
        }
        "bedside.edit-query" => {
            commit_query(world, input.source.ok_or("Missing query panel")?)?;
        }
        "bedside.run-query" => {
            let entity = input.source.ok_or("Missing query panel")?;
            commit_query(world, entity)?;
            let state = world
                .resource::<QueryPanels>()
                .0
                .get(&entity)
                .ok_or("Query panel is unavailable")?;
            let sql = bedtime_core::model::<QueryDocument>(world, state.document)
                .ok_or("Query document is unavailable")?
                .sql
                .clone();
            let connection = world
                .resource::<Connections>()
                .get(state.connection)
                .ok_or("Database is unavailable")?;
            if connection.worker.is_none() {
                return Err("Database is disconnected".into());
            }
            let path = connection.record.path.clone();
            let generation = connection.generation;
            let mut panels = world.resource_mut::<QueryPanels>();
            let state = panels.0.get_mut(&entity).unwrap();
            if state.worker.is_none() {
                state.worker = Some(Worker::new(path));
            }
            state.generation = generation;
            state.error = None;
            state.serial = Some(
                state
                    .worker
                    .as_mut()
                    .unwrap()
                    .request(Request::Query { sql }),
            );
        }
        "bedside.cancel-query" => {
            let entity = input.source.ok_or("Missing query panel")?;
            let captured = input.arguments["serial"]
                .as_u64()
                .ok_or("Cancellation requires its captured query serial")?;
            let generation = input.arguments["generation"]
                .as_u64()
                .ok_or("Cancellation requires its captured connection generation")?;
            let mut panels = world.resource_mut::<QueryPanels>();
            let state = panels
                .0
                .get_mut(&entity)
                .ok_or("Query panel is unavailable")?;
            if Some(captured) == state.serial && generation == state.generation {
                if let Some(worker) = state.worker.as_mut() {
                    worker.cancel();
                }
                state.serial = None;
                state.error = Some("Query cancelled".into());
            }
        }
        _ => return Err(format!("Unknown Bedside command: {}", input.command)),
    }
    Ok(Vec::new())
}

fn commit_query(world: &mut World, entity: Entity) -> Result<(), String> {
    let state = world
        .resource::<QueryPanels>()
        .0
        .get(&entity)
        .ok_or("Query panel is unavailable")?;
    if !state.draft {
        return Ok(());
    }
    let (document, revision, sql) = (state.document, state.revision, state.buffer.clone());
    let next = bedtime_core::edit_model::<QueryDocument>(
        world,
        document,
        revision,
        ReplaceSql(sql),
        "Edit SQL",
    )?;
    let mut panels = world.resource_mut::<QueryPanels>();
    let state = panels.0.get_mut(&entity).unwrap();
    state.revision = next;
    state.draft = false;
    Ok(())
}
fn query_action(In(input): In<PanelActionInvocation>, world: &mut World) -> PanelActionResult {
    match input.action {
        PanelAction::CommitEdit => {
            commit_query(world, input.entity)?;
        }
        PanelAction::Undo | PanelAction::Redo => {
            commit_query(world, input.entity)?;
            let document = world
                .get::<DocumentBinding>(input.entity)
                .ok_or("Query panel is unavailable")?
                .0;
            if matches!(input.action, PanelAction::Undo) {
                bedtime_core::undo_document(world, document)?;
            } else {
                bedtime_core::redo_document(world, document)?;
            }
        }
        _ => return Ok(false),
    }
    Ok(true)
}

fn poll(world: &mut World) {
    let mut pending = false;
    {
        let mut connections = world.resource_mut::<Connections>();
        for connection in &mut connections.entries {
            let Some(worker) = connection.worker.as_mut() else {
                continue;
            };
            if let Some(reply) = worker.poll()
                && connection.serial == Some(reply.id)
            {
                connection.serial = None;
                match reply.result {
                    Ok(Response::Schema(schema)) => {
                        connection.schema = Some(schema);
                        connection.error = None;
                    }
                    Ok(_) => {
                        connection.error =
                            Some("SQLite returned an unexpected schema response".into())
                    }
                    Err(error) => connection.error = Some(error),
                }
            }
            pending |= connection.serial.is_some();
        }
    }
    world.resource_scope(|world, mut panels: Mut<DataPanels>| {
        panels
            .0
            .retain(|entity, _| world.entities().contains(*entity));
        for state in panels.0.values_mut() {
            let Some(connection) = world.resource::<Connections>().get(state.connection) else {
                continue;
            };
            if connection.worker.is_none() {
                state.worker = None;
                state.serial = None;
                state.error = Some("Database is disconnected".into());
                continue;
            }
            if state.generation != connection.generation {
                let mut worker = Worker::new(connection.record.path.clone());
                state.serial = Some(worker.request(Request::Browse {
                    table: state.table.clone(),
                    offset: state.offset,
                }));
                state.worker = Some(worker);
                state.generation = connection.generation;
                state.error = None;
            }
            if let Some(reply) = state.worker.as_mut().and_then(Worker::poll)
                && state.serial == Some(reply.id)
            {
                state.serial = None;
                match reply.result {
                    Ok(Response::Data(data)) => {
                        state.result = Some(data);
                        state.error = None;
                    }
                    Ok(_) => {
                        state.error = Some("SQLite returned an unexpected data response".into())
                    }
                    Err(error) => state.error = Some(error),
                }
            }
            pending |= state.serial.is_some();
        }
    });
    world.resource_scope(|world, mut panels: Mut<QueryPanels>| {
        panels
            .0
            .retain(|entity, _| world.entities().contains(*entity));
        for state in panels.0.values_mut() {
            let Some(connection) = world.resource::<Connections>().get(state.connection) else {
                continue;
            };
            if connection.worker.is_none() || state.generation != connection.generation {
                if connection.worker.is_none() {
                    state.error = Some("Database is disconnected".into());
                } else if state.serial.is_some() {
                    state.error = Some("Query stopped when the database was refreshed".into());
                }
                state.worker = None;
                state.serial = None;
                state.generation = connection.generation;
            }
            if let Some(reply) = state.worker.as_mut().and_then(Worker::poll)
                && state.serial == Some(reply.id)
            {
                state.serial = None;
                match reply.result {
                    Ok(Response::Data(data)) => {
                        state.result = Some(data);
                        state.error = None;
                    }
                    Ok(_) => {
                        state.error = Some("SQLite returned an unexpected query response".into())
                    }
                    Err(error) => state.error = Some(error),
                }
            }
            pending |= state.serial.is_some();
        }
    });
    if pending {
        world
            .resource_mut::<bedtime::api::features::HostEffects>()
            .push(None, bedtime::api::features::HostAction::Invalidate);
    }
}

/// Install only the shared document runtime for headless behavior tests.
pub fn headless(metadata: MetadataStore, databases: Vec<PathBuf>) -> App {
    for path in databases {
        metadata
            .add_connection(&path)
            .expect("Headless database fixture must exist");
    }
    let mut app = App::new();
    app.add_plugins(bedtime_core::DocumentHostPlugin)
        .add_plugins(BedsidePlugin { metadata });
    app.update();
    app
}

pub fn configure(app: &mut App, databases: Vec<PathBuf>) -> io::Result<()> {
    let path = app
        .world()
        .resource::<bedtime::ApplicationPaths>()
        .config_dir
        .join("metadata.sqlite");
    let metadata = MetadataStore::open(&path)?;
    for database in &databases {
        metadata.add_connection(database)?;
    }
    app.add_plugins(BedsidePlugin { metadata });
    automation::register(app.world_mut());
    Ok(())
}

#[cfg(test)]
mod tests;
