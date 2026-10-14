use super::*;
use crate::{WorkspaceRepository, test_support::TempDir};
use bedtime_storage::{StateScope, StateStore};

fn paths(temp: &TempDir) -> ApplicationPaths {
    ApplicationPaths::new("fixture", Some(temp.path("config")), None).unwrap()
}
fn local(temp: &TempDir, name: &str) -> WorkspaceSpec {
    let root = temp.path(name);
    fs::create_dir_all(&root).unwrap();
    WorkspaceSpec::local(fs::canonicalize(root).unwrap().to_str().unwrap())
}
fn remote(host: &str, root: &str) -> WorkspaceSpec {
    WorkspaceSpec {
        name: "Remote".into(),
        target: WorkspaceTarget::Ssh { host: host.into() },
        root: root.into(),
    }
}
fn snapshot(layouts: Value, plugins: Value) -> crate::WorkspaceSnapshot {
    crate::WorkspaceSnapshot {
        layouts,
        plugins,
        history: Some(json!({"files":{}})),
    }
}

#[test]
fn unknown_directories_stay_temporary_and_state_writes_do_not_create_markers() {
    let temp = TempDir::new();
    let paths = paths(&temp);
    let spec = local(&temp, "project");
    let store = WorkspaceStore::load(&paths).unwrap();
    assert!(store.stored_spec(&spec).unwrap().is_none());
    assert!(!paths.config_dir.exists());
    assert!(!paths.workspace_directory(Path::new(&spec.root)).exists());
    assert_eq!(
        store
            .state_store()
            .save(&StateScope(spec.identity()), &json!({"plugin":1}))
            .unwrap_err()
            .kind(),
        io::ErrorKind::NotFound
    );
    assert!(!paths.workspace_directory(Path::new(&spec.root)).exists());
    assert!(!paths.config_dir.exists());
}

#[test]
fn local_marker_is_authoritative_without_a_client_registry_entry() {
    let temp = TempDir::new();
    let paths = paths(&temp);
    let mut store = WorkspaceStore::load(&paths).unwrap();
    let layout = json!(["unrelated app",{"floating":true}]);
    let spec = store
        .commit_snapshot(
            local(&temp, "project"),
            snapshot(layout.clone(), json!({"feature":{"value":1}})),
        )
        .unwrap();
    let marker = paths
        .workspace_directory(Path::new(&spec.root))
        .join("workspace.json");
    assert!(marker.is_file());
    let second = ApplicationPaths::new("fixture", Some(temp.path("second-client")), None).unwrap();
    let repository = WorkspaceStore::load(&second).unwrap().repository();
    assert!(repository.recent().unwrap().is_empty());
    assert_eq!(repository.find(&spec).unwrap(), Some(spec.clone()));
    let saved = repository.load(&spec).unwrap().unwrap();
    assert_eq!(saved.layouts, layout);
    assert_eq!(saved.plugins, json!({"feature":{"value":1}}));
    assert!(saved.history.unwrap()["files"].is_object());
    assert!(!second.config_dir.exists());
}

#[test]
fn opaque_layout_replacement_has_no_numbered_slot_or_shape_assumptions() {
    let temp = TempDir::new();
    let paths = paths(&temp);
    let mut repository = WorkspaceStore::load(&paths).unwrap().repository();
    let spec = repository
        .commit(
            local(&temp, "project"),
            snapshot(json!({"some application":"custom"}), json!({})),
        )
        .unwrap();
    repository
        .commit(spec.clone(), snapshot(json!(42), json!({"current":true})))
        .unwrap();
    let saved = repository.load(&spec).unwrap().unwrap();
    assert_eq!(saved.layouts, json!(42));
    assert_eq!(saved.plugins, json!({"current":true}));
}

#[test]
fn malformed_marker_is_reported_at_open_without_blocking_registry_loading() {
    let temp = TempDir::new();
    let paths = paths(&temp);
    let spec = local(&temp, "project");
    let directory = paths.workspace_directory(Path::new(&spec.root));
    fs::create_dir(&directory).unwrap();
    let store = WorkspaceStore::load(&paths).unwrap();
    assert!(store.stored_spec(&spec).unwrap().is_none());
    fs::write(directory.join("workspace.json"), b"{bad json}").unwrap();
    assert_eq!(
        store.stored_spec(&spec).unwrap_err().kind(),
        io::ErrorKind::InvalidData
    );
    assert!(WorkspaceStore::load(&paths).is_ok());
}

#[test]
fn remote_snapshots_are_client_local_and_separated_by_host_and_root() {
    let temp = TempDir::new();
    let paths = paths(&temp);
    let existing = local(&temp, "project");
    let mut store = WorkspaceStore::load(&paths).unwrap();
    let a = store
        .record_workspace(remote("host-a", &existing.root))
        .unwrap();
    let b = store
        .record_workspace(remote("host-b", &existing.root))
        .unwrap();
    let c = store
        .record_workspace(remote("host-a", "/different"))
        .unwrap();
    let a_path = store.workspace_path(&a).unwrap().unwrap();
    let b_path = store.workspace_path(&b).unwrap().unwrap();
    let c_path = store.workspace_path(&c).unwrap().unwrap();
    assert!(a_path.starts_with(paths.config_dir.join("remote")));
    assert_ne!(a_path, b_path);
    assert_ne!(a_path, c_path);
    assert!(
        !paths
            .workspace_directory(Path::new(&existing.root))
            .exists()
    );
    let mut swapped = read_json(&a_path).unwrap().unwrap();
    swapped["identity"] = json!(b.identity());
    write_atomic(&a_path, &swapped).unwrap();
    assert_eq!(
        store.read_snapshot(&a).unwrap_err().kind(),
        io::ErrorKind::InvalidData
    );
}

#[test]
fn plugin_fragment_updates_merge_under_lock_and_keep_layout_and_history() {
    let temp = TempDir::new();
    let paths = paths(&temp);
    let mut store = WorkspaceStore::load(&paths).unwrap();
    let spec = store
        .commit_snapshot(
            local(&temp, "project"),
            snapshot(json!({"layout":"one"}), json!({"absent":{"opaque":1}})),
        )
        .unwrap();
    let mut jobs = Vec::new();
    for index in 0..6 {
        let paths = paths.clone();
        let spec = spec.clone();
        jobs.push(std::thread::spawn(move || {
            WorkspaceStore::load(&paths)
                .unwrap()
                .set_plugin_state(&spec, &format!("owner-{index}"), json!(index))
                .unwrap();
        }));
    }
    for job in jobs {
        job.join().unwrap();
    }
    let loaded = store.repository().load(&spec).unwrap().unwrap();
    assert_eq!(loaded.layouts, json!({"layout":"one"}));
    assert!(loaded.history.is_some());
    assert_eq!(loaded.plugins["absent"], json!({"opaque":1}));
    for index in 0..6 {
        assert_eq!(loaded.plugins[format!("owner-{index}")], json!(index));
    }
}

#[test]
fn failed_plugin_validation_keeps_previous_snapshot_and_name() {
    let temp = TempDir::new();
    let paths = paths(&temp);
    let mut repository = WorkspaceStore::load(&paths).unwrap().repository();
    let spec = repository
        .commit(
            local(&temp, "project"),
            snapshot(json!("layout"), json!({"saved":true})),
        )
        .unwrap();
    assert!(
        repository
            .commit(spec.clone(), snapshot(Value::Null, Value::Null))
            .is_err()
    );
    assert_eq!(
        repository.load(&spec).unwrap().unwrap().plugins,
        json!({"saved":true})
    );
    let renamed = repository.rename(&spec, "Custom name").unwrap();
    assert_eq!(renamed.name, "Custom name");
    assert_eq!(
        repository.load(&renamed).unwrap().unwrap().layouts,
        json!("layout")
    );
    repository.forget(&renamed).unwrap();
    assert!(repository.recent().unwrap().is_empty());
    assert!(repository.find(&renamed).unwrap().is_some());
}

#[test]
fn renaming_and_forgetting_do_not_reopen_a_location() {
    let temp = TempDir::new();
    let paths = paths(&temp);
    let mut store = WorkspaceStore::load(&paths).unwrap();
    let first = store.record_workspace(local(&temp, "first")).unwrap();
    let second = store.record_workspace(local(&temp, "second")).unwrap();
    let renamed = store.rename_workspace(&first, "First renamed").unwrap();
    assert_eq!(store.last_workspace(), Some(second.clone()));
    assert_eq!(
        store.recent_workspaces(),
        vec![second.clone(), renamed.clone()]
    );
    store.forget_workspace(&second).unwrap();
    assert_eq!(store.last_workspace(), Some(renamed));
    store.forget_workspace(&first).unwrap();
    assert!(store.last_workspace().is_none());
}

#[test]
fn index_failure_does_not_undo_an_authoritative_workspace_commit() {
    let temp = TempDir::new();
    let paths = paths(&temp);
    let mut store = WorkspaceStore::load(&paths).unwrap();
    fs::create_dir_all(paths.config_dir.join("registry.json")).unwrap();
    let spec = store
        .commit_snapshot(local(&temp, "project"), snapshot(json!("saved"), json!({})))
        .unwrap();
    assert!(store.take_index_error().is_some());
    assert_eq!(
        WorkspaceStore::empty(&paths).stored_spec(&spec).unwrap(),
        Some(spec)
    );
}
