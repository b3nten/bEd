use super::*;
#[path = "../../../tests/support/temp_dir.rs"]
#[allow(dead_code)]
mod test_support;
use crate::WorkspaceRepository;
use bed_storage::{StateScope, StateStore};
use test_support::TempDir;

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
fn layout(label: &str) -> Value {
    json!({"version":2,"panels":[],"label":label})
}
fn snapshot(slot: LayoutSlot, plugins: Value) -> crate::WorkspaceSnapshot {
    crate::WorkspaceSnapshot {
        layouts: json!({"layouts":{slot.key():layout(slot.key())},"active_layout_slot":slot.to_value()}),
        plugins,
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
    let mut state = store.state_store();
    assert_eq!(
        state
            .save(&StateScope(spec.identity()), &json!({"plugin":1}))
            .unwrap_err()
            .kind(),
        io::ErrorKind::NotFound
    );
    assert!(!paths.workspace_directory(Path::new(&spec.root)).exists());
    assert!(!paths.config_dir.exists());
}

#[test]
fn local_marker_is_authoritative_and_discoverable_without_a_client_entry() {
    let temp = TempDir::new();
    let paths = paths(&temp);
    let mut store = WorkspaceStore::load(&paths).unwrap();
    let spec = store
        .commit_snapshot(
            local(&temp, "project"),
            snapshot(LayoutSlot::Layout3, json!({"feature":{"value":1}})),
        )
        .unwrap();
    let marker = paths
        .workspace_directory(Path::new(&spec.root))
        .join("workspace.json");
    assert!(marker.is_file());
    assert_eq!(
        store.history_path(&spec),
        Some(marker.with_file_name("history.json"))
    );
    let registry = read_json(&paths.config_dir.join("registry.json"))
        .unwrap()
        .unwrap();
    assert!(registry["entries"][spec.identity()]["plugins"].is_null());
    let second = ApplicationPaths::new("fixture", Some(temp.path("second-client")), None).unwrap();
    let repository = WorkspaceStore::load(&second).unwrap().repository();
    assert!(repository.recent().unwrap().is_empty());
    assert_eq!(repository.find(&spec).unwrap(), Some(spec.clone()));
    assert_eq!(
        repository.load(&spec).unwrap().unwrap().plugins,
        json!({"feature":{"value":1}})
    );
    assert!(!second.config_dir.exists());
    assert!(!paths.config_dir.join("workspaces.json").exists());
}

#[test]
fn empty_directory_is_not_a_saved_marker_and_bad_marker_is_reported_at_open() {
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
    let existing_local = local(&temp, "project");
    let mut store = WorkspaceStore::load(&paths).unwrap();
    let a = store
        .record_workspace(remote("host-a", &existing_local.root))
        .unwrap();
    let b = store
        .record_workspace(remote("host-b", &existing_local.root))
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
    assert!(a_path.is_file());
    assert!(b_path.is_file());
    assert!(c_path.is_file());
    assert!(store.history_path(&a).is_none());
    assert!(
        !paths
            .workspace_directory(Path::new(&existing_local.root))
            .exists()
    );
    let mut swapped = read_json(&a_path).unwrap().unwrap();
    swapped["identity"] = json!(b.identity());
    write_atomic(&a_path, &swapped).unwrap();
    assert_eq!(
        store.stored_spec(&a).unwrap_err().kind(),
        io::ErrorKind::InvalidData
    );
}

#[test]
fn interrupted_remote_locator_reservation_does_not_save_or_add_recents() {
    let temp = TempDir::new();
    let paths = paths(&temp);
    let spec = remote("host", "/project");
    let mut store = WorkspaceStore::load(&paths).unwrap();
    store.reserve_remote_locator(&spec).unwrap();
    let loaded = WorkspaceStore::load(&paths).unwrap();
    assert!(loaded.stored_spec(&spec).unwrap().is_none());
    assert!(loaded.recent_workspaces().is_empty());
    assert!(loaded.last_workspace().is_none());
    let path = loaded.workspace_path(&spec).unwrap().unwrap();
    store.record_workspace(spec.clone()).unwrap();
    assert_eq!(store.workspace_path(&spec).unwrap().unwrap(), path);
}

#[test]
fn snapshot_updates_preserve_absent_plugins_and_unvisited_layout_slots() {
    let temp = TempDir::new();
    let paths = paths(&temp);
    let mut first = WorkspaceStore::load(&paths).unwrap();
    let spec = first
        .commit_snapshot(
            local(&temp, "project"),
            snapshot(
                LayoutSlot::Layout1,
                json!({"absent":{"future":1},"installed":0}),
            ),
        )
        .unwrap();
    let mut second = WorkspaceStore::load(&paths).unwrap();
    first
        .commit_snapshot(
            spec.clone(),
            snapshot(LayoutSlot::Layout3, json!({"installed":1})),
        )
        .unwrap();
    second
        .commit_snapshot(
            spec.clone(),
            snapshot(LayoutSlot::Layout4, json!({"other":2})),
        )
        .unwrap();
    let loaded = WorkspaceStore::load(&paths).unwrap();
    assert_eq!(
        loaded.plugin_state(&spec, "absent"),
        Some(&json!({"future":1}))
    );
    assert_eq!(loaded.plugin_state(&spec, "installed"), Some(&json!(1)));
    assert_eq!(loaded.plugin_state(&spec, "other"), Some(&json!(2)));
    assert_eq!(
        loaded
            .session_layouts(&spec)
            .unwrap()
            .as_object()
            .unwrap()
            .len(),
        3
    );
    assert_eq!(loaded.active_layout_slot(&spec), LayoutSlot::Layout4);
}

#[test]
fn sidecar_locks_preserve_independent_concurrent_plugin_updates() {
    let temp = TempDir::new();
    let paths = paths(&temp);
    let mut store = WorkspaceStore::load(&paths).unwrap();
    let spec = store.record_workspace(local(&temp, "project")).unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
    let workers = (0..8)
        .map(|index| {
            let paths = paths.clone();
            let spec = spec.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let mut store = WorkspaceStore::load(&paths).unwrap();
                barrier.wait();
                for value in 0..4 {
                    store
                        .set_plugin_state(&spec, &format!("plugin-{index}"), json!(value))
                        .unwrap();
                }
            })
        })
        .collect::<Vec<_>>();
    for worker in workers {
        worker.join().unwrap();
    }
    let loaded = WorkspaceStore::load(&paths).unwrap();
    let plugins = loaded.all_plugin_state(&spec).unwrap();
    assert_eq!(plugins.as_object().unwrap().len(), 8);
    for index in 0..8 {
        assert_eq!(plugins[format!("plugin-{index}")], json!(3));
    }
}

#[test]
fn state_only_writes_from_another_instance_preserve_the_authoritative_name() {
    let temp = TempDir::new();
    let paths = paths(&temp);
    let mut first = WorkspaceStore::load(&paths).unwrap();
    let spec = first.record_workspace(local(&temp, "project")).unwrap();
    let mut second = WorkspaceStore::load(&paths).unwrap();
    let renamed = first.rename_workspace(&spec, "Renamed").unwrap();
    // Simulate an earlier snapshot writer finishing its locator update after
    // this instance's rename. Its captured old label cannot replace the new one.
    second.record_locator(&spec).unwrap();
    assert_eq!(
        WorkspaceStore::load(&paths).unwrap().recent_workspaces()[0].name,
        "Renamed"
    );
    second
        .set_plugin_state(&spec, "plugin", json!({"draft":true}))
        .unwrap();

    let loaded = WorkspaceStore::load(&paths).unwrap();
    assert_eq!(loaded.stored_spec(&spec).unwrap(), Some(renamed));
    assert_eq!(
        loaded.plugin_state(&spec, "plugin"),
        Some(&json!({"draft":true}))
    );
    assert_eq!(loaded.recent_workspaces()[0].name, "Renamed");
}

#[test]
fn local_snapshot_success_is_retained_when_recent_index_update_fails() {
    let temp = TempDir::new();
    let paths = paths(&temp);
    let mut store = WorkspaceStore::load(&paths).unwrap();
    fs::create_dir_all(paths.config_dir.join("registry.json")).unwrap();
    let spec = store
        .commit_snapshot(
            local(&temp, "project"),
            snapshot(LayoutSlot::Layout1, json!({"feature":1})),
        )
        .unwrap();
    assert!(store.workspace_path(&spec).unwrap().unwrap().is_file());
    assert!(
        store
            .take_index_error()
            .unwrap()
            .starts_with("Workspace saved")
    );
    assert_eq!(store.stored_spec(&spec).unwrap(), Some(spec));
}

#[test]
fn recent_removal_keeps_snapshot_and_rename_updates_its_marker() {
    let temp = TempDir::new();
    let paths = paths(&temp);
    let mut store = WorkspaceStore::load(&paths).unwrap();
    let spec = store.record_workspace(local(&temp, "project")).unwrap();
    let renamed = store.rename_workspace(&spec, "  Renamed  ").unwrap();
    assert_eq!(renamed.name, "Renamed");
    store.forget_workspace(&renamed).unwrap();
    assert!(store.recent_workspaces().is_empty());
    assert_eq!(store.stored_spec(&spec).unwrap().unwrap().name, "Renamed");
    assert!(store.workspace_path(&spec).unwrap().unwrap().is_file());
}

#[test]
fn global_layout_templates_are_separate_from_workspace_and_settings_files() {
    let temp = TempDir::new();
    let paths = paths(&temp);
    let mut store = WorkspaceStore::load(&paths).unwrap();
    store
        .set_layout_default(LayoutSlot::Layout3, Some(layout("template")))
        .unwrap();
    let loaded = WorkspaceStore::load(&paths).unwrap();
    assert_eq!(
        loaded.layout_default(LayoutSlot::Layout3),
        Some(layout("template"))
    );
    assert!(paths.config_dir.join("layouts.json").is_file());
    assert!(!paths.config_dir.join("registry.json").exists());
    assert!(!paths.config_dir.join("settings.json").exists());
    assert!(loaded.recent_workspaces().is_empty());
}
