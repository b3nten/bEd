use super::*;
use crate::MemoryWorkspaceRepository;
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use crate::test_support;

fn fixture(root: &std::path::Path) -> App {
    let mut app = App::new();
    app.insert_resource(bedtime_files::FileContext::local(root).unwrap());
    app.add_plugins((
        bedtime_core::DocumentHostPlugin,
        WorkspacesIntegrationPlugin,
    ));
    app.update();
    app
}

fn resolve(app: &mut App, id: WorkspaceRequestId) -> Result<(), String> {
    for _ in 0..30 {
        app.update();
        if let Some(result) = take_workspace_result(app.world_mut(), id) {
            return result;
        }
    }
    panic!("workspace operation did not complete");
}

#[test]
fn explicit_repository_wins_in_either_plugin_order() {
    for before in [false, true] {
        let mut repository = MemoryWorkspaceRepository::default();
        let spec = WorkspaceSpec::local("/fixture/previous");
        repository
            .commit(spec.clone(), WorkspaceSnapshot::default())
            .unwrap();
        let mut app = App::new();
        if before {
            app.add_plugins(WorkspacePersistencePlugin::new(repository));
            app.add_plugins(WorkspacesIntegrationPlugin);
        } else {
            app.add_plugins(WorkspacesIntegrationPlugin);
            app.add_plugins(WorkspacePersistencePlugin::new(repository));
        }
        app.update();
        assert_eq!(
            app.world().resource::<WorkspacesState>().recent_workspaces,
            vec![spec]
        );
    }
}

#[test]
fn saved_state_accepts_an_arbitrary_layout_payload() {
    let dir = test_support::TempDir::new();
    let mut app = fixture(&dir.0);
    let value = json!(["named application arrangement", {"custom": true}]);
    let spec =
        save_current_workspace(app.world_mut(), Some("Custom".into()), value.clone()).unwrap();
    let saved = app
        .world()
        .resource::<WorkspaceRepositoryResource>()
        .0
        .load(&spec)
        .unwrap()
        .unwrap();
    assert_eq!(saved.layouts, value);
    assert!(saved.history.unwrap()["files"].is_object());
}

struct FailingRepository {
    memory: MemoryWorkspaceRepository,
    fail: Arc<AtomicBool>,
}
impl WorkspaceRepository for FailingRepository {
    fn recent(&self) -> io::Result<Vec<WorkspaceSpec>> {
        self.memory.recent()
    }
    fn find(&self, spec: &WorkspaceSpec) -> io::Result<Option<WorkspaceSpec>> {
        self.memory.find(spec)
    }
    fn load(&self, spec: &WorkspaceSpec) -> io::Result<Option<WorkspaceSnapshot>> {
        self.memory.load(spec)
    }
    fn commit(
        &mut self,
        spec: WorkspaceSpec,
        snapshot: WorkspaceSnapshot,
    ) -> io::Result<WorkspaceSpec> {
        if self.fail.load(Ordering::Relaxed) {
            Err(io::Error::other("database is full"))
        } else {
            self.memory.commit(spec, snapshot)
        }
    }
    fn forget(&mut self, spec: &WorkspaceSpec) -> io::Result<()> {
        self.memory.forget(spec)
    }
    fn rename(&mut self, spec: &WorkspaceSpec, name: &str) -> io::Result<WorkspaceSpec> {
        self.memory.rename(spec, name)
    }
}

#[test]
fn failed_save_does_not_attach_a_temporary_session() {
    let dir = test_support::TempDir::new();
    let mut app = fixture(&dir.0);
    app.add_plugins(WorkspacePersistencePlugin::new(FailingRepository {
        memory: Default::default(),
        fail: Arc::new(AtomicBool::new(true)),
    }));
    let id = submit_workspace_request(
        app.world_mut(),
        WorkspaceAction::SaveCurrent { name: None },
        None,
        0,
    );
    assert!(
        resolve(&mut app, id)
            .unwrap_err()
            .contains("database is full")
    );
    assert_eq!(
        *app.world().resource::<WorkspaceAttachment>(),
        WorkspaceAttachment::Temporary
    );
}

#[test]
fn failed_pre_commit_keeps_context_documents_and_saved_attachment_alive() {
    let dir = test_support::TempDir::new();
    let next = dir.path("next");
    std::fs::create_dir(&next).unwrap();
    let next = next.canonicalize().unwrap();
    let fail = Arc::new(AtomicBool::new(false));
    let mut app = fixture(&dir.0);
    app.add_plugins(WorkspacePersistencePlugin::new(FailingRepository {
        memory: Default::default(),
        fail: Arc::clone(&fail),
    }));
    let saved = save_current_workspace(app.world_mut(), None, json!({})).unwrap();
    app.world_mut()
        .resource_mut::<WorkspaceRepositoryResource>()
        .0
        .commit(
            WorkspaceSpec::local(next.to_string_lossy()),
            WorkspaceSnapshot::default(),
        )
        .unwrap();
    let session = app
        .world()
        .non_send::<bedtime_documents::DocumentSession>()
        .session_id();
    let document = app
        .world_mut()
        .non_send_mut::<bedtime_documents::DocumentSession>()
        .create_document(b"still live")
        .unwrap();
    app.world_mut()
        .insert_resource(bedtime_core::DocumentCloseDecisionPolicy::Ask);
    fail.store(true, Ordering::Relaxed);
    let id = submit_workspace_request(
        app.world_mut(),
        WorkspaceAction::OpenLocalWorkspace(next),
        None,
        0,
    );
    let result = (0..30)
        .find_map(|_| {
            app.update();
            if let Some(prompt) = app
                .world()
                .get_resource::<bedtime_core::CloseDecisionPrompt>()
            {
                let token = prompt.token;
                bedtime_core::answer_close_prompt(
                    app.world_mut(),
                    token,
                    bedtime_core::CloseDecision::Discard,
                );
            }
            take_workspace_result(app.world_mut(), id)
        })
        .expect("workspace request completed");
    assert!(result.unwrap_err().contains("database is full"));
    assert_eq!(
        app.world()
            .resource::<bedtime_files::FileContext>()
            .directory,
        dir.0.canonicalize().unwrap()
    );
    assert_eq!(
        app.world()
            .non_send::<bedtime_documents::DocumentSession>()
            .session_id(),
        session
    );
    assert_eq!(
        app.world()
            .non_send::<bedtime_documents::DocumentSession>()
            .snapshot(document)
            .unwrap()
            .bytes,
        b"still live"
    );
    assert_eq!(
        app.world().resource::<WorkspaceAttachment>(),
        &WorkspaceAttachment::Saved(saved)
    );
    assert!(!bedtime_core::operation_barrier_active(app.world()));
}

#[test]
fn adopting_a_saved_local_directory_preserves_dirty_documents_and_identity() {
    let dir = test_support::TempDir::new();
    let project = dir.path("project");
    std::fs::create_dir(&project).unwrap();
    let project = project.canonicalize().unwrap();
    let mut app = fixture(&dir.0);
    let session = app
        .world()
        .non_send::<bedtime_documents::DocumentSession>()
        .session_id();
    let document = app
        .world_mut()
        .non_send_mut::<bedtime_documents::DocumentSession>()
        .create_document(b"carried draft")
        .unwrap();
    let id = submit_workspace_request(
        app.world_mut(),
        WorkspaceAction::OpenLocalWorkspace(project.clone()),
        None,
        0,
    );
    resolve(&mut app, id).unwrap();
    assert_eq!(
        app.world()
            .non_send::<bedtime_documents::DocumentSession>()
            .session_id(),
        session
    );
    assert_eq!(
        app.world()
            .non_send::<bedtime_documents::DocumentSession>()
            .snapshot(document)
            .unwrap()
            .bytes,
        b"carried draft"
    );
    assert_eq!(
        app.world()
            .resource::<bedtime_files::FileContext>()
            .directory,
        project
    );
    assert!(matches!(
        app.world().resource::<WorkspaceAttachment>(),
        WorkspaceAttachment::Saved(_)
    ));
}

#[test]
fn global_workspace_open_is_independent_of_an_unavailable_originating_panel() {
    let dir = test_support::TempDir::new();
    let project = dir.path("project");
    std::fs::create_dir(&project).unwrap();
    let project = project.canonicalize().unwrap();
    let mut app = fixture(&dir.0);
    let mut tiling = bedtime_workbench::workspace::tiling_state::TilingState::default();
    tiling.assign(1, 1);
    let layout = json!({
        "version": 2,
        "panels": [{"id": 1, "panel_type": "absent.plugin.panel", "state": {"keep": true}}],
        "tiling": tiling.to_value([1])
    });
    bedtime_core::restore_document_layout(app.world_mut(), &layout, false, None).unwrap();
    let source = bedtime_core::panel_entities(app.world_mut())[0];
    assert!(
        app.world()
            .get::<bedtime_workbench::PanelRecord>(source)
            .unwrap()
            .unavailable
            .is_some()
    );
    let id = submit_workspace_request(
        app.world_mut(),
        WorkspaceAction::OpenLocalWorkspace(project.clone()),
        Some(source),
        1,
    );
    resolve(&mut app, id).unwrap();
    assert_eq!(
        app.world()
            .resource::<bedtime_files::FileContext>()
            .directory,
        project
    );
    assert!(matches!(
        app.world().resource::<WorkspaceAttachment>(),
        WorkspaceAttachment::Saved(_)
    ));
}

#[test]
fn malformed_history_is_rejected_before_context_replacement() {
    let dir = test_support::TempDir::new();
    let next = dir.path("next");
    std::fs::create_dir(&next).unwrap();
    let next = next.canonicalize().unwrap();
    let mut app = fixture(&dir.0);
    let current = save_current_workspace(app.world_mut(), None, json!({})).unwrap();
    let session = app
        .world()
        .non_send::<bedtime_documents::DocumentSession>()
        .session_id();
    for history in [
        json!({"files":false}),
        json!({"files":{"file.txt":{"version":4,"undoStack":false,"redoStack":[]}}}),
        json!({"files":{"file.txt":{"version":4,"undoStack":["invalid edit"],"redoStack":[]}}}),
    ] {
        app.world_mut()
            .resource_mut::<WorkspaceRepositoryResource>()
            .0
            .commit(
                WorkspaceSpec::local(next.to_string_lossy()),
                WorkspaceSnapshot {
                    history: Some(history),
                    ..Default::default()
                },
            )
            .unwrap();
        let id = submit_workspace_request(
            app.world_mut(),
            WorkspaceAction::OpenLocalWorkspace(next.clone()),
            None,
            0,
        );
        assert!(resolve(&mut app, id).is_err());
    }
    assert_eq!(
        app.world()
            .non_send::<bedtime_documents::DocumentSession>()
            .session_id(),
        session
    );
    assert_eq!(
        app.world().resource::<WorkspaceAttachment>(),
        &WorkspaceAttachment::Saved(current)
    );
    assert_eq!(
        app.world()
            .resource::<bedtime_files::FileContext>()
            .directory,
        dir.0.canonicalize().unwrap()
    );
}

#[test]
fn malformed_default_layout_is_rejected_before_retiring_the_current_session() {
    let dir = test_support::TempDir::new();
    let next = dir.path("next");
    std::fs::create_dir(&next).unwrap();
    let next = next.canonicalize().unwrap();
    let mut app = fixture(&dir.0);
    let current = save_current_workspace(app.world_mut(), None, Value::Null).unwrap();
    let session = app
        .world()
        .non_send::<bedtime_documents::DocumentSession>()
        .session_id();
    app.world_mut()
        .resource_mut::<WorkspaceRepositoryResource>()
        .0
        .commit(
            WorkspaceSpec::local(next.to_string_lossy()),
            WorkspaceSnapshot {
                layouts: json!({"version":1,"panels":[{"id":0,"panel_type":"test"}]}),
                ..Default::default()
            },
        )
        .unwrap();
    let id = submit_workspace_request(
        app.world_mut(),
        WorkspaceAction::OpenLocalWorkspace(next),
        None,
        0,
    );
    assert!(resolve(&mut app, id).unwrap_err().contains("panel ID"));
    assert_eq!(
        app.world()
            .non_send::<bedtime_documents::DocumentSession>()
            .session_id(),
        session
    );
    assert_eq!(
        app.world().resource::<WorkspaceAttachment>(),
        &WorkspaceAttachment::Saved(current)
    );
}

#[test]
fn failed_restore_retains_the_saved_snapshot_through_persist_autosave_and_quit() {
    let dir = test_support::TempDir::new();
    let next = dir.path("next");
    std::fs::create_dir(&next).unwrap();
    let next = next.canonicalize().unwrap();
    let mut app = fixture(&dir.0);
    save_current_workspace(app.world_mut(), None, Value::Null).unwrap();
    let spec = WorkspaceSpec::local(next.to_string_lossy());
    let expected = WorkspaceSnapshot {
        layouts: json!({"application":"retained arrangement"}),
        plugins: json!({"absent-plugin":{"draft":"retained"}}),
        ..Default::default()
    };
    let assert_saved = |world: &World| {
        let saved = world
            .resource::<WorkspaceRepositoryResource>()
            .0
            .load(&spec)
            .unwrap()
            .unwrap();
        assert_eq!(saved.layouts, expected.layouts);
        assert_eq!(saved.plugins, expected.plugins);
        assert_eq!(saved.history, expected.history);
    };
    app.world_mut()
        .resource_mut::<WorkspaceRepositoryResource>()
        .0
        .commit(spec.clone(), expected.clone())
        .unwrap();
    let validate = app.world_mut().register_system(|_: In<Value>| Ok(()));
    let restore =
        app.world_mut()
            .register_system(|_: In<WorkspaceRestore>| -> Result<(), String> {
                Err("application restore failed".into())
            });
    app.insert_resource(WorkspaceLayoutPreflight(validate))
        .insert_resource(WorkspaceLayoutRestore(restore));
    let id = submit_workspace_request(
        app.world_mut(),
        WorkspaceAction::OpenLocalWorkspace(next.clone()),
        None,
        0,
    );
    assert_eq!(
        resolve(&mut app, id).unwrap_err(),
        "Workspace opened, but restoring its state failed: application restore failed"
    );
    assert_eq!(
        app.world()
            .resource::<bedtime_files::FileContext>()
            .directory,
        next
    );
    assert_eq!(
        app.world().resource::<WorkspaceAttachment>(),
        &WorkspaceAttachment::Saved(spec.clone())
    );
    assert!(
        persist_workspace(app.world_mut())
            .unwrap_err()
            .to_string()
            .contains("restoring it failed")
    );
    app.world_mut()
        .resource_mut::<WorkspaceAutosave>()
        .last_saved = Instant::now() - Duration::from_secs(3);
    app.update();
    assert_saved(app.world());

    let quit = bedtime_core::submit_quit(app.world_mut());
    let quit_result = (0..30)
        .find_map(|_| {
            app.update();
            bedtime_core::take_host_result(app.world_mut(), quit)
        })
        .expect("quit completed without replacing the retained snapshot");
    assert!(quit_result.is_ok());
    assert_saved(app.world());
}

#[test]
fn a_retired_request_source_fails_without_blocking_later_core_requests() {
    let dir = test_support::TempDir::new();
    let mut app = fixture(&dir.0);
    let source = app.world_mut().spawn_empty().id();
    let id = submit_workspace_request(
        app.world_mut(),
        WorkspaceAction::SaveCurrent { name: None },
        Some(source),
        0,
    );
    app.world_mut().despawn(source);
    let later = bedtime_core::submit_host_request(
        app.world_mut(),
        None,
        bedtime_api::features::HostAction::Notify {
            message: "after failed workspace".into(),
        },
    );
    assert!(resolve(&mut app, id).is_err());
    app.update();
    assert_eq!(
        bedtime_core::take_host_result(app.world_mut(), later),
        Some(Ok(bedtime_core::HostOutcome::Applied))
    );
    assert_eq!(
        app.world().resource::<WorkspaceAttachment>(),
        &WorkspaceAttachment::Temporary
    );
}

struct SqliteRepository(Mutex<rusqlite::Connection>);
impl SqliteRepository {
    fn new(path: &std::path::Path) -> Self {
        let connection = rusqlite::Connection::open(path).unwrap();
        connection.execute_batch("CREATE TABLE IF NOT EXISTS workspaces (identity TEXT PRIMARY KEY, spec TEXT NOT NULL, snapshot TEXT NOT NULL, position INTEGER NOT NULL);").unwrap();
        Self(Mutex::new(connection))
    }
}
impl WorkspaceRepository for SqliteRepository {
    fn recent(&self) -> io::Result<Vec<WorkspaceSpec>> {
        let connection = self.0.lock().unwrap();
        let mut statement = connection
            .prepare("SELECT spec FROM workspaces WHERE position >= 0 ORDER BY position DESC")
            .map_err(io::Error::other)?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(io::Error::other)?;
        rows.map(|row| {
            let value: Value =
                serde_json::from_str(&row.map_err(io::Error::other)?).map_err(io::Error::other)?;
            WorkspaceSpec::from_value(&value)
                .ok_or_else(|| io::Error::other("invalid workspace definition"))
        })
        .collect()
    }
    fn find(&self, spec: &WorkspaceSpec) -> io::Result<Option<WorkspaceSpec>> {
        use rusqlite::OptionalExtension;
        let value = self
            .0
            .lock()
            .unwrap()
            .query_row(
                "SELECT spec FROM workspaces WHERE identity = ?",
                [spec.identity()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(io::Error::other)?;
        value
            .map(|value| {
                let value: Value = serde_json::from_str(&value).map_err(io::Error::other)?;
                WorkspaceSpec::from_value(&value)
                    .ok_or_else(|| io::Error::other("invalid workspace definition"))
            })
            .transpose()
    }
    fn load(&self, spec: &WorkspaceSpec) -> io::Result<Option<WorkspaceSnapshot>> {
        use rusqlite::OptionalExtension;
        let value = self
            .0
            .lock()
            .unwrap()
            .query_row(
                "SELECT snapshot FROM workspaces WHERE identity = ?",
                [spec.identity()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(io::Error::other)?;
        value
            .map(|value| {
                let value: Value = serde_json::from_str(&value).map_err(io::Error::other)?;
                Ok(WorkspaceSnapshot {
                    layouts: value["layouts"].clone(),
                    plugins: value["plugins"].clone(),
                    history: value
                        .get("history")
                        .filter(|value| !value.is_null())
                        .cloned(),
                })
            })
            .transpose()
    }
    fn commit(
        &mut self,
        spec: WorkspaceSpec,
        snapshot: WorkspaceSnapshot,
    ) -> io::Result<WorkspaceSpec> {
        bedtime_storage::validate_snapshot(&snapshot.plugins)?;
        let value = json!({"layouts":snapshot.layouts,"plugins":snapshot.plugins,"history":snapshot.history});
        let mut connection = self.0.lock().unwrap();
        let transaction = connection.transaction().map_err(io::Error::other)?;
        transaction.execute("INSERT INTO workspaces(identity,spec,snapshot,position) VALUES (?1,?2,?3,(SELECT COALESCE(MAX(position),0)+1 FROM workspaces)) ON CONFLICT(identity) DO UPDATE SET spec=excluded.spec,snapshot=excluded.snapshot,position=excluded.position", rusqlite::params![spec.identity(),spec.to_value().to_string(),value.to_string()]).map_err(io::Error::other)?;
        transaction.commit().map_err(io::Error::other)?;
        Ok(spec)
    }
    fn forget(&mut self, spec: &WorkspaceSpec) -> io::Result<()> {
        self.0
            .lock()
            .unwrap()
            .execute(
                "UPDATE workspaces SET position=-1 WHERE identity=?",
                [spec.identity()],
            )
            .map_err(io::Error::other)?;
        Ok(())
    }
    fn rename(&mut self, spec: &WorkspaceSpec, name: &str) -> io::Result<WorkspaceSpec> {
        let mut renamed = self
            .find(spec)?
            .ok_or_else(|| io::Error::other("missing workspace"))?;
        renamed.name = name.into();
        let snapshot = self.load(spec)?.unwrap();
        self.commit(renamed, snapshot)
    }
}

#[test]
fn application_database_repository_restores_opaque_state_and_recent_entries() {
    let dir = test_support::TempDir::new();
    let path = dir.path("state.sqlite");
    let mut app = fixture(&dir.0);
    app.add_plugins(WorkspacePersistencePlugin::new(SqliteRepository::new(
        &path,
    )));
    let spec = save_current_workspace(
        app.world_mut(),
        Some("Database persisted".into()),
        json!({"named":"main","custom_slots":[1,2]}),
    )
    .unwrap();
    drop(app);
    let mut restarted = fixture(&dir.0);
    restarted.add_plugins(WorkspacePersistencePlugin::new(SqliteRepository::new(
        &path,
    )));
    refresh_workspace_observations(restarted.world_mut()).unwrap();
    assert_eq!(
        restarted
            .world()
            .resource::<WorkspacesState>()
            .recent_workspaces,
        vec![spec.clone()]
    );
    assert_eq!(
        restarted
            .world()
            .resource::<WorkspaceRepositoryResource>()
            .0
            .load(&spec)
            .unwrap()
            .unwrap()
            .layouts,
        json!({"named":"main","custom_slots":[1,2]})
    );
    assert!(!dir.path(".fixture").exists());
}
