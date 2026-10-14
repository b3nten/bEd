use super::*;
use bedtime_core::{CloseDecision, CloseDecisionPrompt};
use bedtime_workbench::{PanelRecord, WorkbenchRequests};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "bedside-tests-{}-{}",
            std::process::id(),
            bevy::asset::uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn store(&self) -> MetadataStore {
        MetadataStore::open(&self.0.join("metadata.sqlite")).unwrap()
    }
    fn databases(&self) -> Vec<PathBuf> {
        test_fixtures::seed_databases(&self.0).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn invalid_metadata_is_reported_without_overwriting_saved_state() {
    let fixture = Fixture::new();
    let mut store = fixture.store();
    let invalid = json!({"layout":{"version":999},"selected_connection":null});
    bedtime_storage::StateStore::save(&mut store, &StateScope(SESSION_SCOPE.into()), &invalid)
        .unwrap();
    let mut app = headless(store, Vec::new());
    assert!(!app.world().resource::<Notices>().0.is_empty());
    app.world_mut()
        .resource_mut::<SessionPersistence>()
        .changed_at = Some(Instant::now() - Duration::from_secs(2));
    app.update();
    persist_session(app.world_mut()).unwrap();
    assert_eq!(
        bedtime_storage::StateStore::load(&fixture.store(), &StateScope(SESSION_SCOPE.into()))
            .unwrap(),
        Some(invalid)
    );
    let path = fixture.0.join("metadata.sqlite");
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection.pragma_update(None, "user_version", -1).unwrap();
    let error = MetadataStore::open(&path).err().unwrap();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert!(error.to_string().contains("schema version: -1"));
}

fn invoke(app: &mut App, id: &str, arguments: Value, source: Option<Entity>) {
    let world = app.world_mut();
    let mut input = bedtime_workbench::capture_command(world, id, arguments);
    input.source = source;
    world
        .resource_mut::<WorkbenchRequests>()
        .push(WorkbenchAction::Invoke(input));
    bedtime_core::resolve_documents(world);
    let errors = world.resource_mut::<WorkbenchRequests>().take_errors();
    assert!(errors.is_empty(), "{errors:?}");
}
fn wait(app: &mut App, condition: impl Fn(&World) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition(app.world()) {
        assert!(Instant::now() < deadline, "SQLite operation did not finish");
        app.update();
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn query_panel(app: &App) -> Entity {
    *app.world()
        .resource::<QueryPanels>()
        .0
        .keys()
        .find(|entity| app.world().entities().contains(**entity))
        .unwrap()
}
fn draft(app: &mut App, entity: Entity, sql: &str) {
    let mut states = app.world_mut().resource_mut::<QueryPanels>();
    let state = states.0.get_mut(&entity).unwrap();
    state.buffer = sql.into();
    state.draft = true;
}

#[test]
fn app_owned_sqlite_persists_query_models_and_layout_without_a_workspace() {
    let fixture = Fixture::new();
    let databases = fixture.databases();
    let mut app = headless(fixture.store(), databases.clone());
    wait(&mut app, |world| {
        world
            .resource::<Connections>()
            .entries
            .iter()
            .all(|connection| connection.schema.is_some())
    });
    let first = app.world().resource::<Connections>().entries[0].record.id;
    let second = app.world().resource::<Connections>().entries[1].record.id;
    invoke(
        &mut app,
        "bedside.open-data",
        json!({"connection":first.0,"table":"items"}),
        None,
    );
    invoke(
        &mut app,
        "bedside.open-schema",
        json!({"connection":first.0,"table":"items"}),
        None,
    );
    invoke(
        &mut app,
        "bedside.open-data",
        json!({"connection":second.0,"table":"items"}),
        None,
    );
    invoke(
        &mut app,
        "bedside.new-query",
        json!({"connection":first.0}),
        None,
    );
    let entity = query_panel(&app);
    let document = app.world().get::<DocumentBinding>(entity).unwrap().0;
    let query_id = app.world().resource::<QueryPanels>().0[&entity].query;
    invoke(
        &mut app,
        "bedside.open-query",
        json!({"query":query_id.0}),
        None,
    );
    assert_eq!(app.world().resource::<QueryPanels>().0.len(), 1);
    assert_eq!(
        app.world()
            .resource::<bedtime_workbench::WorkbenchLayout>()
            .focused,
        bedtime::api::workbench::panel_owner(app.world(), entity)
    );
    invoke(
        &mut app,
        "bedtime.workbench.duplicate_view",
        Value::Null,
        Some(entity),
    );
    let second_view = *app
        .world()
        .resource::<QueryPanels>()
        .0
        .keys()
        .find(|panel| **panel != entity)
        .unwrap();
    assert_eq!(
        app.world().get::<DocumentBinding>(second_view).unwrap().0,
        document
    );
    draft(&mut app, entity, "SELECT count(*) FROM items");
    invoke(&mut app, "bedside.edit-query", Value::Null, Some(entity));
    assert!(bedtime_core::document_dirty(app.world(), document).unwrap());
    invoke(
        &mut app,
        "bedtime.documents.undo",
        Value::Null,
        Some(second_view),
    );
    assert_eq!(
        bedtime_core::model::<QueryDocument>(app.world(), document)
            .unwrap()
            .sql,
        ""
    );
    invoke(
        &mut app,
        "bedtime.documents.redo",
        Value::Null,
        Some(entity),
    );
    invoke(
        &mut app,
        "bedtime.core.close",
        Value::Null,
        Some(second_view),
    );
    assert!(!app.world().contains_resource::<CloseDecisionPrompt>());
    app.update();
    assert!(app.world().entities().contains(entity));
    invoke(
        &mut app,
        "bedside.select-connection",
        json!({"connection":second.0}),
        None,
    );
    invoke(&mut app, "bedside.run-query", Value::Null, Some(entity));
    wait(&mut app, |world| {
        world.resource::<QueryPanels>().0[&entity].result.is_some()
    });
    assert_eq!(
        app.world().resource::<QueryPanels>().0[&entity]
            .result
            .as_ref()
            .unwrap()
            .rows[0][0],
        bedtime_sqlite::Cell::Integer(625)
    );
    assert_eq!(app.world().resource::<DataPanels>().0.len(), 2);
    invoke(
        &mut app,
        "bedtime.documents.save",
        json!({"document":document.0}),
        Some(entity),
    );
    wait(&mut app, |world| {
        !bedtime_core::document_dirty(world, document).unwrap()
    });
    assert_eq!(
        fixture.store().queries().unwrap()[0].sql,
        "SELECT count(*) FROM items"
    );
    persist_session(app.world_mut()).unwrap();
    assert_eq!(
        app.world_mut()
            .query_filtered::<&PanelRecord, With<bedtime_workbench::PanelPresentation>>()
            .iter(app.world())
            .filter(|record| record.panel_type == SCHEMA_PANEL)
            .count(),
        1
    );
    drop(app);
    let mut reopened = headless(fixture.store(), Vec::new());
    let entity = query_panel(&reopened);
    let restored = reopened.world().resource::<QueryPanels>().0[&entity].document;
    assert_ne!(restored, document);
    assert_eq!(
        reopened.world().resource::<QueryPanels>().0[&entity].query,
        query_id
    );
    assert_eq!(
        bedtime_core::model::<QueryDocument>(reopened.world(), restored)
            .unwrap()
            .sql,
        "SELECT count(*) FROM items"
    );
    assert!(!bedtime_core::document_dirty(reopened.world(), restored).unwrap());
    assert!(
        reopened.world().resource::<QueryPanels>().0[&entity]
            .serial
            .is_none()
    );
    assert!(
        reopened.world().resource::<QueryPanels>().0[&entity]
            .result
            .is_none()
    );
    assert_eq!(reopened.world().resource::<Connections>().entries.len(), 2);
    assert_eq!(reopened.world().resource::<DataPanels>().0.len(), 2);
    invoke(
        &mut reopened,
        "bedside.run-query",
        Value::Null,
        Some(entity),
    );
    wait(&mut reopened, |world| {
        world.resource::<QueryPanels>().0[&entity].result.is_some()
    });
    assert_eq!(
        reopened.world().resource::<QueryPanels>().0[&entity]
            .result
            .as_ref()
            .unwrap()
            .rows[0][0],
        bedtime_sqlite::Cell::Integer(625)
    );
}

#[test]
fn failed_metadata_save_keeps_the_query_model_and_panel_live() {
    let fixture = Fixture::new();
    let mut app = headless(fixture.store(), fixture.databases());
    let connection = app.world().resource::<Connections>().entries[0].record.id;
    invoke(
        &mut app,
        "bedside.new-query",
        json!({"connection":connection.0}),
        None,
    );
    let entity = query_panel(&app);
    draft(&mut app, entity, "SELECT 'must remain live'");
    invoke(&mut app, "bedside.edit-query", Value::Null, Some(entity));
    let document = app.world().get::<DocumentBinding>(entity).unwrap().0;
    rusqlite::Connection::open(fixture.0.join("metadata.sqlite"))
        .unwrap()
        .execute("DELETE FROM queries", [])
        .unwrap();
    let save = bedtime_core::begin_document_save(
        app.world_mut(),
        &[bedtime_core::DocumentSaveTarget {
            document,
            destination: None,
        }],
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(result) = bedtime_core::poll_document_save(app.world_mut(), save) {
            assert!(result.unwrap_err().contains("no longer available"));
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert!(bedtime_core::document_dirty(app.world(), document).unwrap());
    assert!(app.world().entities().contains(entity));
    assert_eq!(
        bedtime_core::model::<QueryDocument>(app.world(), document)
            .unwrap()
            .sql,
        "SELECT 'must remain live'"
    );
}

#[test]
fn cancellation_and_read_only_execution_are_scoped_to_the_originating_query() {
    let fixture = Fixture::new();
    let databases = fixture.databases();
    let mut app = headless(fixture.store(), databases.clone());
    let connection = app.world().resource::<Connections>().entries[0].record.id;
    invoke(
        &mut app,
        "bedside.new-query",
        json!({"connection":connection.0}),
        None,
    );
    let first = query_panel(&app);
    invoke(
        &mut app,
        "bedside.new-query",
        json!({"connection":connection.0}),
        None,
    );
    let second = *app
        .world()
        .resource::<QueryPanels>()
        .0
        .keys()
        .find(|entity| **entity != first)
        .unwrap();
    draft(
        &mut app,
        first,
        "WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<1000000000) SELECT sum(x) FROM n",
    );
    invoke(&mut app, "bedside.run-query", Value::Null, Some(first));
    let cancelled_serial = app.world().resource::<QueryPanels>().0[&first]
        .serial
        .unwrap();
    let cancelled_generation = app.world().resource::<QueryPanels>().0[&first].generation;
    draft(&mut app, second, "SELECT 42");
    invoke(&mut app, "bedside.run-query", Value::Null, Some(second));
    invoke(
        &mut app,
        "bedside.cancel-query",
        json!({"serial":cancelled_serial,"generation":cancelled_generation}),
        Some(first),
    );
    wait(&mut app, |world| {
        world.resource::<QueryPanels>().0[&second].result.is_some()
    });
    assert_eq!(
        app.world().resource::<QueryPanels>().0[&second]
            .result
            .as_ref()
            .unwrap()
            .rows[0][0],
        bedtime_sqlite::Cell::Integer(42)
    );
    assert!(
        app.world().resource::<QueryPanels>().0[&first]
            .serial
            .is_none()
    );

    draft(&mut app, first, "SELECT 7");
    invoke(&mut app, "bedside.run-query", Value::Null, Some(first));
    invoke(
        &mut app,
        "bedside.cancel-query",
        json!({"serial":cancelled_serial,"generation":cancelled_generation}),
        Some(first),
    );
    wait(&mut app, |world| {
        world.resource::<QueryPanels>().0[&first].result.is_some()
    });
    assert_eq!(
        app.world().resource::<QueryPanels>().0[&first]
            .result
            .as_ref()
            .unwrap()
            .rows[0][0],
        bedtime_sqlite::Cell::Integer(7)
    );
    invoke(
        &mut app,
        "bedside.refresh",
        json!({"connection":connection.0}),
        None,
    );
    app.update();
    draft(&mut app, first, "SELECT 9");
    invoke(&mut app, "bedside.run-query", Value::Null, Some(first));
    invoke(
        &mut app,
        "bedside.cancel-query",
        json!({"serial":cancelled_serial,"generation":cancelled_generation}),
        Some(first),
    );
    wait(&mut app, |world| {
        world.resource::<QueryPanels>().0[&first].serial.is_none()
    });
    assert_eq!(
        app.world().resource::<QueryPanels>().0[&first]
            .result
            .as_ref()
            .unwrap()
            .rows[0][0],
        bedtime_sqlite::Cell::Integer(9)
    );
    let original = std::fs::read(&databases[0]).unwrap();
    draft(&mut app, first, "DELETE FROM items");
    invoke(&mut app, "bedside.run-query", Value::Null, Some(first));
    wait(&mut app, |world| {
        world.resource::<QueryPanels>().0[&first].error.is_some()
    });
    assert_eq!(std::fs::read(&databases[0]).unwrap(), original);
}

#[test]
fn dirty_query_close_cancel_and_save_use_the_shared_protocol() {
    let fixture = Fixture::new();
    let mut app = headless(fixture.store(), fixture.databases());
    let connection = app.world().resource::<Connections>().entries[0].record.id;
    invoke(
        &mut app,
        "bedside.new-query",
        json!({"connection":connection.0}),
        None,
    );
    let entity = query_panel(&app);
    draft(&mut app, entity, "SELECT 'saved in metadata'");
    invoke(&mut app, "bedside.edit-query", Value::Null, Some(entity));
    let mut close =
        bedtime_workbench::capture_command(app.world(), "bedtime.core.close", Value::Null);
    close.source = Some(entity);
    let close_id = app
        .world_mut()
        .resource_mut::<WorkbenchRequests>()
        .submit(WorkbenchAction::Invoke(close));
    bedtime_core::resolve_documents(app.world_mut());
    let token = app.world().resource::<CloseDecisionPrompt>().token;
    assert!(bedtime_core::answer_close_prompt(
        app.world_mut(),
        token,
        CloseDecision::Cancel
    ));
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        app.update();
        if let Some(result) = app
            .world_mut()
            .resource_mut::<WorkbenchRequests>()
            .take_result(close_id)
        {
            assert_eq!(result.unwrap_err(), "Operation cancelled");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Cancelled close did not acknowledge its request"
        );
    }
    assert!(app.world().entities().contains(entity));
    assert!(!app.world().contains_resource::<CloseDecisionPrompt>());
    assert!(
        bedtime_core::document_dirty(
            app.world(),
            app.world().get::<DocumentBinding>(entity).unwrap().0
        )
        .unwrap()
    );
    invoke(&mut app, "bedtime.core.close", Value::Null, Some(entity));
    let token = app.world().resource::<CloseDecisionPrompt>().token;
    assert!(bedtime_core::answer_close_prompt(
        app.world_mut(),
        token,
        CloseDecision::Save
    ));
    wait(&mut app, |world| !world.entities().contains(entity));
    assert_eq!(
        fixture.store().queries().unwrap()[0].sql,
        "SELECT 'saved in metadata'"
    );
    let query = fixture.store().queries().unwrap()[0].id;
    invoke(
        &mut app,
        "bedside.open-query",
        json!({"query":query.0}),
        None,
    );
    let entity = query_panel(&app);
    let document = app.world().get::<DocumentBinding>(entity).unwrap().0;
    draft(&mut app, entity, "SELECT 'discard this change'");
    invoke(&mut app, "bedside.edit-query", Value::Null, Some(entity));
    invoke(&mut app, "bedtime.core.close", Value::Null, Some(entity));
    let token = app.world().resource::<CloseDecisionPrompt>().token;
    assert!(bedtime_core::answer_close_prompt(
        app.world_mut(),
        token,
        CloseDecision::Discard
    ));
    wait(&mut app, |world| !world.entities().contains(entity));
    assert!(bedtime_core::model_document(app.world(), document).is_none());
    invoke(
        &mut app,
        "bedside.open-query",
        json!({"query":query.0}),
        None,
    );
    let entity = query_panel(&app);
    let document = app.world().get::<DocumentBinding>(entity).unwrap().0;
    assert_eq!(
        bedtime_core::model::<QueryDocument>(app.world(), document)
            .unwrap()
            .sql,
        "SELECT 'saved in metadata'"
    );
    assert!(!bedtime_core::document_dirty(app.world(), document).unwrap());
}
