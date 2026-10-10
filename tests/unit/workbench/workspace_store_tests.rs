use super::*;
use crate::test_support::TempDir;
#[test]
fn independent_instances_preserve_other_projects_and_fields() {
    let temp = TempDir::new();
    let config = temp.path("config");
    let mut first = WorkspaceStore::load(&config).unwrap();
    let mut second = WorkspaceStore::load(&config).unwrap();
    let a = first.record_workspace(remote("host", "/a")).unwrap();
    let b = second.record_workspace(remote("host", "/b")).unwrap();
    first.set_layout(&a, json!({"panel":"first"})).unwrap();
    second.set_layout(&b, json!({"panel":"second"})).unwrap();
    first.rename_workspace(&b, "Renamed elsewhere").unwrap();
    second
        .set_module_settings(&b, "test.visibility", json!({"hidden":true}))
        .unwrap();
    first.forget_workspace(&a).unwrap();
    second.set_layout(&b, json!({"panel":"updated"})).unwrap();

    let loaded = WorkspaceStore::load(&config).unwrap();
    assert_eq!(loaded.recent_workspaces().len(), 1);
    assert_eq!(loaded.recent_workspaces()[0].name, "Renamed elsewhere");
    assert_eq!(loaded.layout(&a).unwrap()["panel"], "first");
    assert_eq!(loaded.layout(&b).unwrap()["panel"], "updated");
    assert_eq!(
        loaded.module_settings(&b, "test.visibility"),
        Some(&json!({"hidden":true}))
    );
}
#[test]
fn concurrent_instances_serialize_workspace_updates() {
    let temp = TempDir::new();
    let config = temp.path("config");
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
    let workers = (0..8)
        .map(|index| {
            let config = config.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let mut store = WorkspaceStore::load(&config).unwrap();
                barrier.wait();
                let spec = store
                    .record_workspace(remote("host", &format!("/project-{index}")))
                    .unwrap();
                for revision in 0..5 {
                    store
                        .set_layout(&spec, json!({"revision":revision}))
                        .unwrap();
                }
                spec
            })
        })
        .collect::<Vec<_>>();
    let specs = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect::<Vec<_>>();
    let loaded = WorkspaceStore::load(&config).unwrap();
    assert_eq!(loaded.recent_workspaces().len(), specs.len());
    for spec in specs {
        assert_eq!(loaded.layout(&spec).unwrap()["revision"], 4);
    }
}
#[test]
fn module_settings_roundtrip_without_changing_layout_or_other_workspaces() {
    let temp = TempDir::new();
    let mut store = WorkspaceStore::load(&temp.path("config")).unwrap();
    let local = WorkspaceSpec::local("/same/root");
    let remote = WorkspaceSpec {
        name: "remote".into(),
        root: local.root.clone(),
        target: WorkspaceTarget::Ssh {
            host: "host".into(),
        },
    };
    assert_eq!(store.module_settings(&local, "test.feature"), None);
    store
        .set_layout(&local, json!({"existing":"layout"}))
        .unwrap();
    let settings = json!({"enabled":true,"paths":["build","src/private.rs"]});
    store
        .set_module_settings(&local, "test.feature", settings.clone())
        .unwrap();
    let mut store = WorkspaceStore::load(&temp.path("config")).unwrap();
    assert_eq!(
        store.module_settings(&local, "test.feature"),
        Some(&settings)
    );
    assert_eq!(store.module_settings(&remote, "test.feature"), None);
    assert_eq!(store.layout(&local).unwrap()["existing"], "layout");
    store
        .set_layout(&local, json!({"updated":"layout"}))
        .unwrap();
    store
        .set_module_settings(&remote, "test.feature", json!({"enabled":false}))
        .unwrap();
    let store = WorkspaceStore::load(&temp.path("config")).unwrap();
    assert_eq!(
        store.module_settings(&local, "test.feature"),
        Some(&settings)
    );
    assert_eq!(
        store.module_settings(&remote, "test.feature"),
        Some(&json!({"enabled":false}))
    );
}

#[test]
fn independent_module_updates_preserve_sibling_namespaces_on_the_same_workspace() {
    let temp = TempDir::new();
    let config = temp.path("config");
    let mut first = WorkspaceStore::load(&config).unwrap();
    let mut second = WorkspaceStore::load(&config).unwrap();
    let spec = remote("host", "/project");
    first
        .set_module_settings(&spec, "test.first", json!({"revision":1}))
        .unwrap();
    second
        .set_module_settings(&spec, "test.second", json!(["opaque", "feature", "value"]))
        .unwrap();
    first
        .set_module_settings(&spec, "test.first", Value::Null)
        .unwrap();
    let loaded = WorkspaceStore::load(&config).unwrap();
    assert_eq!(
        loaded.module_settings(&spec, "test.first"),
        Some(&Value::Null)
    );
    assert_eq!(
        loaded.module_settings(&spec, "test.second"),
        Some(&json!(["opaque", "feature", "value"]))
    );
}

#[test]
fn legacy_module_settings_migrate_without_overwriting_sibling_modules() {
    for (module, field) in [("bed.explorer", "file_tree"), ("bed.debug", "debug")] {
        let temp = TempDir::new();
        let config = temp.path("config");
        let mut store = WorkspaceStore::load(&config).unwrap();
        let spec = store.record_workspace(remote("host", "/project")).unwrap();
        let legacy = json!({"legacy":true,"paths":["retained"]});
        store.state["workspaces"][spec.identity()][field] = legacy.clone();
        store.state["workspaces"][spec.identity()]["modules"] =
            json!({"test.other":{"enabled":true}});
        store.save().unwrap();

        let mut store = WorkspaceStore::load(&config).unwrap();
        assert_eq!(store.module_settings(&spec, module), Some(&legacy));
        let updated = json!({"legacy":false,"paths":["updated"]});
        store
            .set_module_settings(&spec, module, updated.clone())
            .unwrap();
        let store = WorkspaceStore::load(&config).unwrap();
        assert_eq!(store.module_settings(&spec, module), Some(&updated));
        assert_eq!(
            store.module_settings(&spec, "test.other"),
            Some(&json!({"enabled":true}))
        );
        assert!(
            store.state["workspaces"][spec.identity()]
                .get(field)
                .is_none()
        );
    }
}

#[test]
fn canonical_module_snapshots_win_over_legacy_fields_when_a_workspace_is_reactivated() {
    let temp = TempDir::new();
    let config = temp.path("config");
    let mut store = WorkspaceStore::load(&config).unwrap();
    let spec = store.record_workspace(remote("host", "/project")).unwrap();
    store.state["workspaces"][spec.identity()]["file_tree"] = json!({"legacy":true});
    store.state["workspaces"][spec.identity()]["debug"] = json!({"target":"legacy"});
    store.save().unwrap();
    let settings = json!({
        "bed.explorer":{"paths":["generated"]},
        "bed.debug":{"target":"canonical"},
        "test.other":{"enabled":true}
    });
    store.save_module_settings(&spec, settings.clone()).unwrap();
    assert!(
        store.state["workspaces"][spec.identity()]
            .get("file_tree")
            .is_none()
    );
    assert!(
        store.state["workspaces"][spec.identity()]
            .get("debug")
            .is_none()
    );

    // Stale aliases cannot override canonical snapshots, including JSON null.
    store.state["workspaces"][spec.identity()]["file_tree"] = json!({"legacy":true});
    store.state["workspaces"][spec.identity()]["debug"] = json!({"target":"legacy"});
    store.save().unwrap();
    let mut store = WorkspaceStore::load(&config).unwrap();
    for module in ["bed.explorer", "bed.debug", "test.other"] {
        assert_eq!(store.module_settings(&spec, module), settings.get(module));
    }
    store.record_workspace(remote("host", "/other")).unwrap();
    store.record_workspace(spec.clone()).unwrap();
    store
        .save_session_layout(Some(&spec), json!({"tabs":[]}))
        .unwrap();
    // An otherwise unchanged snapshot still removes legacy aliases.
    store.save_module_settings(&spec, settings.clone()).unwrap();
    let mut store = WorkspaceStore::load(&config).unwrap();
    assert_eq!(store.last_workspace(), Some(spec.clone()));
    assert_eq!(
        store.state["workspaces"][spec.identity()]["modules"],
        settings
    );
    assert!(
        store.state["workspaces"][spec.identity()]
            .get("file_tree")
            .is_none()
    );
    assert!(
        store.state["workspaces"][spec.identity()]
            .get("debug")
            .is_none()
    );
    store
        .set_module_settings(&spec, "bed.debug", Value::Null)
        .unwrap();
    store.state["workspaces"][spec.identity()]["debug"] = json!({"target":"legacy"});
    store.save().unwrap();
    assert_eq!(
        WorkspaceStore::load(&config)
            .unwrap()
            .module_settings(&spec, "bed.debug"),
        Some(&Value::Null)
    );
}
#[test]
fn canonical_recents_deduplicate_and_workspace_round_trips() {
    let temp = TempDir::new();
    fs::create_dir(temp.path("project")).unwrap();
    let mut store = WorkspaceStore::load(&temp.path("config")).unwrap();
    let root = store.record_project(&temp.path("project/.")).unwrap();
    store.record_project(&root).unwrap();
    assert_eq!(store.recent_projects(), vec![root.clone()]);
    store
        .set_workspace(
            &root,
            json!({"layout":"dock","views":[{"path":"main.rs","scroll":[1,2]}]}),
        )
        .unwrap();
    let loaded = WorkspaceStore::load(&temp.path("config")).unwrap();
    assert_eq!(loaded.workspace(&root).unwrap()["layout"], "dock");
    store.forget_project(&root).unwrap();
    assert!(store.recent_projects().is_empty());
    assert!(store.workspace(&root).is_some());
}
#[test]
fn failed_project_open_does_not_create_history() {
    let temp = TempDir::new();
    let mut store = WorkspaceStore::load(&temp.path("config")).unwrap();
    assert!(store.record_project(&temp.path("missing")).is_err());
    assert!(!temp.path("config/workspaces.json").exists());
}
fn remote(host: &str, root: &str) -> WorkspaceSpec {
    WorkspaceSpec {
        name: "Remote project".into(),
        target: WorkspaceTarget::Ssh { host: host.into() },
        root: root.into(),
    }
}
#[test]
fn v1_projects_migrate_in_order_with_missing_and_unlisted_layouts() {
    let temp = TempDir::new();
    let config = temp.path("config");
    fs::create_dir_all(&config).unwrap();
    let old = json!({
        "version":1,
        "recent":["/missing/second", "/missing/first", "/missing/second"],
        "projects":{
            "/missing/first":{"dock":"first"},
            "/missing/second":{"dock":"second"},
            "/missing/unlisted":{"dock":"unlisted"}
        }
    });
    fs::write(
        config.join("workspaces.json"),
        serde_json::to_vec(&old).unwrap(),
    )
    .unwrap();
    let mut store = WorkspaceStore::load(&config).unwrap();
    assert_eq!(
        store.recent_projects(),
        vec![
            PathBuf::from("/missing/second"),
            PathBuf::from("/missing/first")
        ]
    );
    assert_eq!(store.recent_workspaces()[0].name, "second");
    assert_eq!(
        store.workspace(Path::new("/missing/unlisted")).unwrap()["dock"],
        "unlisted"
    );
    let renamed = store
        .rename_workspace(&WorkspaceSpec::local("/missing/first"), "Named project")
        .unwrap();
    assert_eq!(store.layout(&renamed).unwrap()["dock"], "first");
    let persisted: Value =
        serde_json::from_slice(&fs::read(config.join("workspaces.json")).unwrap()).unwrap();
    assert_eq!(persisted["version"], 2);
    assert!(persisted.get("projects").is_none());
    let loaded = WorkspaceStore::load(&config).unwrap();
    assert_eq!(loaded.recent_workspaces()[1].name, "Named project");
}
#[test]
fn remote_hosts_and_local_target_keep_layouts_separate() {
    let temp = TempDir::new();
    fs::create_dir_all(temp.path("project")).unwrap();
    let root = fs::canonicalize(temp.path("project")).unwrap();
    let root = root.to_str().unwrap();
    let mut store = WorkspaceStore::load(&temp.path("config")).unwrap();
    let local = store.record_workspace(WorkspaceSpec::local(root)).unwrap();
    let remote_root = if cfg!(unix) { root } else { "/project" };
    let first = store
        .record_workspace(remote("first-host", remote_root))
        .unwrap();
    let second = store
        .record_workspace(remote("second-host", remote_root))
        .unwrap();
    for (spec, label) in [(&local, "local"), (&first, "first"), (&second, "second")] {
        store.set_layout(spec, json!({"dock":label})).unwrap();
    }
    let loaded = WorkspaceStore::load(&temp.path("config")).unwrap();
    assert_eq!(loaded.recent_workspaces().len(), 3);
    assert_eq!(loaded.recent_projects().len(), 1);
    assert_eq!(loaded.layout(&local).unwrap()["dock"], "local");
    assert_eq!(loaded.layout(&first).unwrap()["dock"], "first");
    assert_eq!(loaded.layout(&second).unwrap()["dock"], "second");
}
#[test]
fn remote_paths_are_preserved_without_accessing_local_filesystem() {
    let temp = TempDir::new();
    let mut store = WorkspaceStore::load(&temp.path("config")).unwrap();
    let spec = remote("production", "/no-such-local-project/../a project");
    let recorded = store.record_workspace(spec.clone()).unwrap();
    assert_eq!(recorded, spec);
    store
        .set_layout(&spec, json!({"tabs":["main.rs"]}))
        .unwrap();
    let renamed = store.rename_workspace(&spec, "SSH project").unwrap();
    assert_eq!(store.layout(&renamed).unwrap()["tabs"][0], "main.rs");
    store.forget_workspace(&renamed).unwrap();
    assert!(store.recent_workspaces().is_empty());
    assert!(store.layout(&spec).is_some());
    let loaded = WorkspaceStore::load(&temp.path("config")).unwrap();
    assert_eq!(loaded.stored_spec(&spec).unwrap().name, "SSH project");
}
#[test]
fn renaming_local_workspace_survives_legacy_recording_and_layout_changes() {
    let temp = TempDir::new();
    fs::create_dir_all(temp.path("project")).unwrap();
    let mut store = WorkspaceStore::load(&temp.path("config")).unwrap();
    let root = store.record_project(&temp.path("project")).unwrap();
    let spec = WorkspaceSpec::local(root.to_str().unwrap());
    store.rename_workspace(&spec, "  My project  ").unwrap();
    store.record_project(&root).unwrap();
    store
        .set_workspace(&root, json!({"dock":"updated"}))
        .unwrap();
    assert_eq!(store.recent_workspaces()[0].name, "My project");
    assert!(store.rename_workspace(&spec, "  ").is_err());
}
#[test]
fn legacy_executable_overrides_are_ignored_without_losing_layout() {
    let temp = TempDir::new();
    let config = temp.path("config");
    let mut store = WorkspaceStore::load(&config).unwrap();
    let spec = store.record_workspace(remote("host", "/project")).unwrap();
    store.set_layout(&spec, json!({"dock":"existing"})).unwrap();
    store.state["workspaces"][spec.identity()]["spec"]["target"]["agent"] =
        json!("/old/custom/bed-headless");
    store.save().unwrap();
    let mut store = WorkspaceStore::load(&config).unwrap();
    assert_eq!(store.recent_workspaces(), vec![spec.clone()]);
    store.record_workspace(spec.clone()).unwrap();
    assert_eq!(store.layout(&spec).unwrap()["dock"], "existing");
    assert!(
        store.state["workspaces"][spec.identity()]["spec"]["target"]
            .get("agent")
            .is_none()
    );
}
#[test]
fn remote_projects_round_trip_without_executable_configuration() {
    let temp = TempDir::new();
    let config = temp.path("config");
    let mut store = WorkspaceStore::load(&config).unwrap();
    let recorded = store.record_workspace(remote("host", "/project")).unwrap();
    let loaded = WorkspaceStore::load(&config).unwrap();
    assert_eq!(loaded.recent_workspaces(), vec![recorded.clone()]);
    assert_eq!(
        loaded.state["workspaces"][recorded.identity()]["spec"]["target"],
        json!({"kind":"ssh","host":"host"})
    );
}
