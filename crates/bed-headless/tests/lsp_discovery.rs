//! Remote project evidence comes from the headless filesystem, not local LSP I/O.
use bed_lsp::workspace_lsp::{DocumentId, WorkspaceId, WorkspaceLsp};
use bed_remote::{RemoteClient, SshTarget};
use std::{
    fs,
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

#[test]
fn remote_project_overrides_roots_and_reload_use_headless_filesystem() {
    let directory = std::env::temp_dir().join(format!("bed-headless-lsp-{}", std::process::id()));
    fs::create_dir_all(directory.join("packages/app/src")).unwrap();
    fs::create_dir_all(directory.join(".bed")).unwrap();
    let root = fs::canonicalize(&directory).unwrap();
    fs::write(root.join("project.marker"), "highest").unwrap();
    fs::write(root.join("packages/app/project.marker"), "nested").unwrap();
    let project = root.join(".bed/lsp.json");
    fs::write(&project, r#"{
        "languages":[{"name":"remote-test","file_types":["remotetest"],"roots":["project.marker"],"workspace_lsp_roots":["packages/app"],"language_id":"remote-canonical","language_server":"remote-server"}],
        "language_servers":{"remote-server":{"command":"only-on-target","settings":{"location":"remote"}}}
    }"#).unwrap();
    let client = RemoteClient::launch_local(env!("CARGO_BIN_EXE_bed-headless")).unwrap();
    // Empty host deliberately fails launch validation before any SSH process;
    // root discovery and project reads still use the real headless transport.
    let target = SshTarget {
        host: String::new(),
        agent: "bed-headless".into(),
    };
    let mut pool = WorkspaceLsp::new_remote_layered(
        WorkspaceId(100),
        root.join("missing-user.json"),
        root.clone(),
        target,
        client.clone(),
    );
    let path = root
        .join("packages/app/src/example.remotetest")
        .to_string_lossy()
        .into_owned();
    pool.register_document(DocumentId(1), &path, b"unsaved", 2, "remotetest")
        .unwrap();
    until(&mut pool);
    assert!(pool.config_error().is_none());
    let status = pool
        .server_statuses()
        .into_iter()
        .find(|status| status.instance.server == "remote-server")
        .unwrap();
    assert_eq!(status.instance.root, root.join("packages/app"));
    assert_eq!(status.path, Some(PathBuf::from("only-on-target")));
    assert!(
        status
            .last_error
            .unwrap()
            .contains("SSH language server requires")
    );
    assert_eq!(
        pool.config().server("remote-server").unwrap().settings["location"],
        "remote"
    );
    assert_eq!(pool.config().detect_language(&path), "remote-test");
    fs::write(&project, "broken").unwrap();
    pool.reload_config().unwrap();
    until(&mut pool);
    assert!(pool.config_error().unwrap().contains(".bed/lsp.json"));
    assert_eq!(pool.config().detect_language(&path), "remote-test");
    drop(pool);
    client.disconnect();
    fs::remove_dir_all(directory).unwrap();
}

fn until(pool: &mut WorkspaceLsp) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        pool.poll();
        if !pool.is_discovering() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "Discovery timed out: {:?}",
            pool.config_error()
        );
        thread::sleep(Duration::from_millis(5));
    }
}
