#![cfg(unix)]
#[path = "../../../tests/support/rustup_fixture.rs"]
mod fixture;
use bedtime_lsp::{
    lsp_client::LspClient,
    rust_recovery::RustStartupIssueKind,
    workspace_lsp::{DocumentId, ServerInstanceId, SessionId, WorkspaceLsp},
};
use fixture::{RustupFixture, TOOLCHAIN};
use std::{
    fs, thread,
    time::{Duration, Instant},
};

fn until(pool: &mut WorkspaceLsp, ready: impl Fn(&WorkspaceLsp) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(12);
    while !ready(pool) {
        pool.poll();
        assert!(
            Instant::now() < deadline,
            "Rust fixture stalled: {:?}",
            pool.server_statuses()
        );
        thread::sleep(Duration::from_millis(5));
    }
}

fn setup(fixture: &RustupFixture) -> (WorkspaceLsp, ServerInstanceId) {
    let root = fixture.root("first");
    let instance = ServerInstanceId {
        server: "rust-analyzer".into(),
        root: root.clone(),
    };
    let mut pool = WorkspaceLsp::new(SessionId(1), fixture.config.clone(), root.clone());
    pool.register_document(
        DocumentId(1),
        root.join("main.rs").to_str().unwrap(),
        b"fn main() {}",
        7,
        "rust",
    )
    .unwrap();
    (pool, instance)
}

#[test]
fn missing_components_install_progress_and_restart_only_the_selected_root() {
    let fixture = RustupFixture::new();
    fixture.touch("wait-install");
    let (mut pool, instance) = setup(&fixture);
    let second = ServerInstanceId {
        server: instance.server.clone(),
        root: fixture.root("second"),
    };
    pool.retry_server(&second).unwrap();
    until(&mut pool, |pool| {
        pool.server_statuses()
            .iter()
            .all(|s| s.rust_issue.is_some())
    });
    let before = pool.server_statuses();
    let issue = before[0].rust_issue.as_ref().unwrap();
    assert_eq!(issue.toolchain, TOOLCHAIN);
    assert_eq!(issue.kind, RustStartupIssueKind::MissingComponents);
    assert_eq!(
        issue.command(),
        format!("rustup component add --toolchain {TOOLCHAIN} rust-analyzer rust-src")
    );
    assert!(
        before[0]
            .last_error
            .as_ref()
            .unwrap()
            .contains("is missing rust-analyzer")
    );
    assert!(
        before[0]
            .startup_details
            .as_ref()
            .unwrap()
            .contains("Unknown binary")
    );
    let second_generation = before
        .iter()
        .find(|s| s.instance == second)
        .unwrap()
        .generation;
    let selected_generation = before
        .iter()
        .find(|s| s.instance == instance)
        .unwrap()
        .generation;
    pool.install_rust_tooling(&instance).unwrap();
    assert!(pool.install_rust_tooling(&instance).is_err());
    assert_eq!(
        pool.retry_server(&instance).unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    until(&mut pool, |pool| {
        pool.server_statuses().iter().any(|s| {
            s.rust_install.as_ref().is_some_and(|install| {
                install.running && install.output.contains("downloading component")
            })
        })
    });
    assert!(!fixture.dir.path("installed").exists());
    fixture.touch("allow-install");
    until(&mut pool, |pool| {
        pool.server_statuses()
            .iter()
            .any(|s| s.instance == instance && s.initialized)
    });
    until(&mut pool, |_| {
        fixture
            .messages(&instance.root)
            .iter()
            .any(|m| m["method"] == "textDocument/didOpen")
    });
    let after = pool.server_statuses();
    let selected = after.iter().find(|s| s.instance == instance).unwrap();
    assert_ne!(selected.generation, selected_generation);
    assert!(selected.rust_issue.is_none());
    assert!(selected.last_error.is_none());
    assert!(!selected.rust_install.as_ref().unwrap().running);
    assert_eq!(
        after
            .iter()
            .find(|s| s.instance == second)
            .unwrap()
            .generation,
        second_generation
    );
    let opened = fixture
        .messages(&instance.root)
        .into_iter()
        .find(|m| m["method"] == "textDocument/didOpen")
        .unwrap();
    assert_eq!(opened["params"]["textDocument"]["version"], 7);
    assert_eq!(opened["params"]["textDocument"]["text"], "fn main() {}");
    let calls = fixture.calls();
    assert!(
        calls
            .iter()
            .filter(|c| c["args"][0] == "show" || c["args"][1] == "list")
            .all(|c| c["auto_install"] == "0")
    );
    let installs: Vec<_> = calls.iter().filter(|c| c["args"][1] == "add").collect();
    assert_eq!(installs.len(), 1);
    assert_eq!(installs[0]["cwd"], instance.root.to_str().unwrap());
    assert_eq!(
        installs[0]["args"],
        serde_json::json!([
            "component",
            "add",
            "--toolchain",
            TOOLCHAIN,
            "rust-analyzer",
            "rust-src"
        ])
    );
    assert_eq!(
        installs[0]["literal"],
        "a'b $HOME $(touch never-created) `echo nope`"
    );
    assert!(!instance.root.join("never-created").exists());
}

#[test]
fn missing_toolchain_installs_the_project_selection() {
    let fixture = RustupFixture::new();
    let root = fixture.root("first");
    fs::write(
        root.join("selection"),
        "nightly-2026-10-01-x86_64-unknown-linux-gnu",
    )
    .unwrap();
    fs::write(root.join("missing-toolchain"), "").unwrap();
    let (mut pool, instance) = setup(&fixture);
    until(&mut pool, |pool| {
        pool.server_statuses()[0].rust_issue.is_some()
    });
    let issue = pool.server_statuses()[0].rust_issue.clone().unwrap();
    assert_eq!(issue.kind, RustStartupIssueKind::MissingToolchain);
    assert_eq!(
        issue.toolchain,
        "nightly-2026-10-01-x86_64-unknown-linux-gnu"
    );
    pool.install_rust_tooling(&instance).unwrap();
    until(&mut pool, |pool| pool.server_statuses()[0].initialized);
    let install = fixture
        .calls()
        .into_iter()
        .find(|c| c["args"][1] == "install")
        .unwrap();
    assert_eq!(install["args"], serde_json::json!(issue.arguments()));
}

#[test]
fn failed_install_retains_output_and_can_be_retried() {
    let fixture = RustupFixture::new();
    fixture.touch("install-fails");
    let (mut pool, instance) = setup(&fixture);
    until(&mut pool, |pool| {
        pool.server_statuses()[0].rust_issue.is_some()
    });
    let generation = pool.server_statuses()[0].generation;
    pool.install_rust_tooling(&instance).unwrap();
    until(&mut pool, |pool| !pool.is_installing_rust());
    let status = &pool.server_statuses()[0];
    assert_eq!(status.generation, generation);
    assert!(status.rust_issue.as_ref().unwrap().install_available);
    let result = status.rust_install.as_ref().unwrap();
    assert!(result.message.contains("42"));
    assert!(result.output.contains("download failed"));
    fs::remove_file(fixture.dir.path("install-fails")).unwrap();
    pool.install_rust_tooling(&instance).unwrap();
    until(&mut pool, |pool| pool.server_statuses()[0].initialized);
}

#[test]
fn completed_install_does_not_restart_a_replaced_server() {
    let fixture = RustupFixture::new();
    fixture.touch("wait-install");
    let (mut pool, instance) = setup(&fixture);
    until(&mut pool, |pool| {
        pool.server_statuses()[0].rust_issue.is_some()
    });
    pool.install_rust_tooling(&instance).unwrap();
    until(&mut pool, |_| fixture.dir.path("install-started").exists());
    pool.reload_config().unwrap();
    until(&mut pool, |pool| {
        pool.server_statuses()[0].rust_issue.is_some()
    });
    let replacement_generation = pool.server_statuses()[0].generation;
    assert!(pool.install_rust_tooling(&instance).is_err());
    fixture.touch("allow-install");
    until(&mut pool, |pool| !pool.is_installing_rust());
    let status = &pool.server_statuses()[0];
    assert_eq!(status.generation, replacement_generation);
    assert!(!status.initialized);
    assert!(fixture.messages(&instance.root).is_empty());
}

#[test]
fn selection_change_before_install_prevents_mutation() {
    let fixture = RustupFixture::new();
    let (mut pool, instance) = setup(&fixture);
    until(&mut pool, |pool| {
        pool.server_statuses()[0].rust_issue.is_some()
    });
    fs::write(
        instance.root.join("selection"),
        "stable-aarch64-apple-darwin",
    )
    .unwrap();
    pool.install_rust_tooling(&instance).unwrap();
    until(&mut pool, |pool| !pool.is_installing_rust());
    assert!(
        pool.server_statuses()[0]
            .rust_install
            .as_ref()
            .unwrap()
            .message
            .contains("selection")
    );
    assert!(!fixture.dir.path("install-started").exists());
}

#[test]
fn unsupported_components_and_unrelated_failures_do_not_offer_installation() {
    for mode in ["unavailable", "unrelated", "custom"] {
        let fixture = RustupFixture::new();
        let root = fixture.root("first");
        match mode {
            "unavailable" => fixture.touch("unavailable"),
            "unrelated" => {
                fs::write(root.join("unrelated"), "").unwrap();
            }
            _ => {
                fs::write(root.join("selection"), "custom-local-build").unwrap();
            }
        }
        let (mut pool, instance) = setup(&fixture);
        until(&mut pool, |pool| {
            pool.server_statuses()[0].last_error.is_some()
        });
        // Allow background verification to complete; no installation may be
        // offered at any point for these failures.
        let deadline = Instant::now() + Duration::from_millis(300);
        while Instant::now() < deadline {
            pool.poll();
            assert!(pool.server_statuses()[0].rust_issue.is_none());
            thread::sleep(Duration::from_millis(5));
        }
        assert!(pool.install_rust_tooling(&instance).is_err());
        assert!(!fixture.dir.path("install-started").exists());
    }
}

#[test]
fn configured_environment_override_is_used_for_diagnosis() {
    let fixture = RustupFixture::new();
    let root = fixture.root("first");
    fs::write(root.join("selection"), "stable-aarch64-apple-darwin").unwrap();
    let mut config = fixture.config();
    config
        .language_servers
        .get_mut("rust-analyzer")
        .unwrap()
        .environment
        .insert(
            "RUSTUP_TOOLCHAIN".into(),
            "nightly-x86_64-unknown-linux-gnu".into(),
        );
    let mut client = LspClient::with_config(fixture.config.clone(), config);
    client.set_workspace(root.to_str().unwrap());
    client.start_server("rust", "").unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while client.rust_startup_issue().is_none() {
        client.poll();
        assert!(Instant::now() < deadline, "{:?}", client.last_error());
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        client.rust_startup_issue().unwrap().toolchain,
        "nightly-x86_64-unknown-linux-gnu"
    );
}

#[test]
fn diagnostic_timeout_keeps_original_error_and_polling_responsive() {
    let fixture = RustupFixture::new();
    fixture.touch("hang-query");
    let (mut pool, instance) = setup(&fixture);
    until(&mut pool, |pool| {
        pool.server_statuses()[0].last_error.is_some()
    });
    let deadline = Instant::now() + Duration::from_secs(6);
    while Instant::now() < deadline {
        let before = Instant::now();
        pool.poll();
        assert!(before.elapsed() < Duration::from_millis(250));
        assert!(pool.server_statuses()[0].rust_issue.is_none());
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        pool.server_statuses()[0]
            .last_error
            .as_ref()
            .unwrap()
            .contains("Unknown binary")
    );
    assert!(pool.install_rust_tooling(&instance).is_err());
}

#[test]
fn remote_install_requests_never_start_local_rustup() {
    let fixture = RustupFixture::new();
    let root = fixture.root("first");
    let mut pool = WorkspaceLsp::new_remote(
        SessionId(2),
        fixture.config.clone(),
        root.clone(),
        bedtime_remote::SshTarget::new("fixture-host"),
    );
    let instance = ServerInstanceId {
        server: "rust-analyzer".into(),
        root,
    };
    assert!(
        pool.install_rust_tooling(&instance)
            .unwrap_err()
            .to_string()
            .contains("remote host")
    );
    assert!(fixture.calls().is_empty());
    assert!(!fixture.dir.path("install-started").exists());
}

#[test]
fn closing_workspace_leaves_installer_to_finish_without_restart() {
    let fixture = RustupFixture::new();
    fixture.touch("wait-install");
    let (mut pool, instance) = setup(&fixture);
    until(&mut pool, |pool| {
        pool.server_statuses()[0].rust_issue.is_some()
    });
    pool.install_rust_tooling(&instance).unwrap();
    until(&mut pool, |_| fixture.dir.path("install-started").exists());
    drop(pool);
    fixture.touch("allow-install");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !fixture.dir.path("installed").exists() {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(5));
    }
    assert!(fixture.messages(&instance.root).is_empty());
}

#[test]
fn installer_output_is_bounded_and_keeps_the_final_message() {
    let fixture = RustupFixture::new();
    fixture.touch("burst-output");
    let (mut pool, instance) = setup(&fixture);
    until(&mut pool, |pool| {
        pool.server_statuses()[0].rust_issue.is_some()
    });
    pool.install_rust_tooling(&instance).unwrap();
    until(&mut pool, |pool| !pool.is_installing_rust());
    let status = pool.server_statuses();
    let output = &status[0].rust_install.as_ref().unwrap().output;
    assert_eq!(output.len(), bedtime_lsp::message_handler::MAX_STDERR_BYTES);
    assert!(output.ends_with("info: installation complete\n"));
}

#[test]
fn a_standalone_executable_is_not_repaired_as_a_rustup_proxy() {
    let fixture = RustupFixture::new();
    let root = fixture.root("first");
    let mut config = fixture.config();
    // Identical script content with a different inode is a standalone binary,
    // not a rustup proxy. Its failure must not authorize rustup installation.
    // The fixture dispatches by basename, so give the copy that name in another directory.
    let standalone = fixture.root("standalone").join("rust-analyzer");
    fs::copy(fixture.dir.path("rustup"), &standalone).unwrap();
    config
        .language_servers
        .get_mut("rust-analyzer")
        .unwrap()
        .command = vec![standalone.to_string_lossy().into_owned()];
    let mut client = LspClient::with_config(fixture.config.clone(), config);
    client.set_workspace(root.to_str().unwrap());
    client.start_server("rust", "").unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while client.last_error().is_none() {
        client.poll();
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(5));
    }
    assert!(client.last_error().unwrap().contains("Unknown binary"));
    assert!(client.rust_startup_issue().is_none());
    assert!(fixture.calls().is_empty());
}
