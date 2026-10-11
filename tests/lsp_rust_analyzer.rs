//! Opt-in acceptance against the real installed rust-analyzer, without fake
//! server responses or changes to the user's LSP configuration.
use bed_document_session::{
    ClosePolicy, EditorSession, LspConfigMode, SessionOptions,
    editor_session::{DocumentId, ViewId, WorkspaceId},
};
use bed_editing::editor_commands::CursorReveal;
use bed_lsp::{lsp_locations::from_definition_result, workspace_lsp::WorkspaceLsp};
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    collections::BTreeSet,
    fs,
    path::PathBuf,
    rc::Rc,
    thread,
    time::{Duration, Instant},
};

struct Cleanup(PathBuf);
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn existing_user_config() -> PathBuf {
    let config = bed_settings::Settings::get_user_config_dir()
        .expect("a user settings directory is required for the existing LSP configuration")
        .join("lsp.json");
    assert!(config.is_file(), "existing user LSP configuration required");
    config
}

fn observe_progress(
    session: &EditorSession,
    document: DocumentId,
    titles: &mut BTreeSet<String>,
    active_seen: &mut bool,
) {
    let client = session
        .lsp()
        .unwrap()
        .client_for_document(document)
        .unwrap();
    for job in client.borrow().progress() {
        *active_seen |= !job.finished;
        if titles.insert(job.title.clone()) {
            eprintln!("rust-analyzer work-done progress: {}", job.title);
        }
    }
}

#[test]
#[ignore = "requires the user's installed rust-analyzer and read-only ~/.config/bed/lsp.json"]
fn real_session_user_config_reports_and_clears_rust_syntax_diagnostics() {
    let root =
        std::env::temp_dir().join(format!("bed-real-session-rust-lsp-{}", std::process::id()));
    fs::create_dir_all(root.join("src")).unwrap();
    let cleanup = Cleanup(root);
    fs::write(
        cleanup.0.join("Cargo.toml"),
        "[package]\nname = \"bed_lsp_diagnostics_fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[workspace]\n",
    ).unwrap();
    let path = cleanup.0.join("src/main.rs");
    fs::write(&path, b"fn main() {}\n").unwrap();
    let mut session = EditorSession::with_options(SessionOptions {
        project_root: Some(cleanup.0.clone()),
        lsp_config: Some(existing_user_config()),
        lsp_config_mode: LspConfigMode::Layered,
        ..Default::default()
    })
    .unwrap();
    let document = session.open_file(&path).unwrap();
    let view = session.create_view(document).unwrap();
    let canonical = fs::canonicalize(&path).unwrap();
    let key = canonical.to_str().unwrap();
    let deadline = Instant::now() + Duration::from_secs(120);
    let client = loop {
        let report = session.tick();
        assert!(
            report.errors.is_empty(),
            "session errors: {:?}",
            report.errors
        );
        if let Some(client) = session.lsp().unwrap().client_for_document(document) {
            assert!(
                client.borrow().is_process_started(),
                "Rust process stopped: {:?}",
                session.lsp().unwrap().server_statuses()
            );
            if client.borrow().is_initialized() && client.borrow().is_document_open(key) {
                break client;
            }
        }
        assert!(
            Instant::now() < deadline,
            "Rust discovery/initialize/open timed out: {:?}; configuration error: {:?}",
            session.lsp().unwrap().server_statuses(),
            session.lsp().unwrap().config_error()
        );
        thread::sleep(Duration::from_millis(10));
    };
    assert!(Rc::ptr_eq(
        &client,
        &session.lsp().unwrap().client_for_path(key).unwrap()
    ));
    session
        .with_commands(view, |commands| {
            commands.set_cursor(0, 12, false, CursorReveal::Ensure);
            commands.delete_left(false);
        })
        .unwrap();
    assert_eq!(session.snapshot(document).unwrap().bytes, b"fn main() {\n");
    assert!(session.save(document).unwrap());
    assert_eq!(fs::read(&path).unwrap(), b"fn main() {\n");
    let diagnostics = session
        .lsp()
        .unwrap()
        .diagnostics_for_document(document)
        .unwrap();
    while diagnostics.for_document(key).is_empty() {
        let report = session.tick();
        assert!(
            report.errors.is_empty(),
            "session errors: {:?}",
            report.errors
        );
        assert!(
            Instant::now() < deadline,
            "Rust did not publish syntax diagnostics after Session edit/save: {:?}; events: {:?}",
            session.lsp().unwrap().server_statuses(),
            report.lsp_events
        );
        thread::sleep(Duration::from_millis(10));
    }
    let items = diagnostics.for_document(key);
    assert!(
        items.iter().any(|item| item.severity == 1),
        "expected Rust syntax error, got {items:?}"
    );
    assert!(
        items
            .iter()
            .any(|item| item.message.to_lowercase().contains("brace")
                || item.message.to_lowercase().contains("delimiter")
                || item.message.to_lowercase().contains("expected")),
        "expected missing-brace message, got {items:?}"
    );
    assert!(!diagnostics.for_line(key, 0).is_empty());
    session
        .with_commands(view, |commands| commands.undo())
        .unwrap();
    assert_eq!(session.snapshot(document).unwrap().bytes, b"fn main() {}\n");
    assert!(session.save(document).unwrap());
    while !diagnostics.for_document(key).is_empty() {
        let report = session.tick();
        assert!(
            report.errors.is_empty(),
            "session errors: {:?}",
            report.errors
        );
        assert!(
            Instant::now() < deadline,
            "Rust diagnostics did not clear after Session undo/save: {:?}; diagnostics: {:?}",
            session.lsp().unwrap().server_statuses(),
            diagnostics.for_document(key)
        );
        thread::sleep(Duration::from_millis(10));
    }
    session.shutdown(ClosePolicy::Discard).unwrap();
}

#[test]
#[ignore = "requires the user's installed rust-analyzer; edits only an unsaved Bed buffer"]
fn real_bed_repository_session_reports_unsaved_syntax_diagnostics() {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let path = repo.join("src/main.rs");
    let original = fs::read(&path).unwrap();
    let text = std::str::from_utf8(&original).unwrap();
    let (row, line) = text
        .lines()
        .enumerate()
        .filter(|(_, line)| line.trim() == "}")
        .last()
        .unwrap();
    let mut session = EditorSession::with_options(SessionOptions {
        project_root: Some(repo),
        lsp_config: Some(existing_user_config()),
        lsp_config_mode: LspConfigMode::Layered,
        ..Default::default()
    })
    .unwrap();
    let document = session.open_file(&path).unwrap();
    let view = session.create_view(document).unwrap();
    let key = session.snapshot(document).unwrap().path;
    let started = Instant::now();
    let deadline = started + Duration::from_secs(180);
    let mut progress_titles = BTreeSet::new();
    let mut active_progress_seen = false;
    loop {
        let report = session.tick();
        assert!(
            report.errors.is_empty(),
            "session errors: {:?}",
            report.errors
        );
        if let Some(client) = session.lsp().unwrap().client_for_document(document) {
            observe_progress(
                &session,
                document,
                &mut progress_titles,
                &mut active_progress_seen,
            );
            assert!(
                client.borrow().is_process_started(),
                "Bed Rust process stopped: {:?}",
                session.lsp().unwrap().server_statuses()
            );
            if client.borrow().is_initialized() && client.borrow().is_document_open(&key) {
                break;
            }
        }
        assert!(
            Instant::now() < deadline,
            "Bed Rust discovery/initialize/open timed out: {:?}; configuration error: {:?}",
            session.lsp().unwrap().server_statuses(),
            session.lsp().unwrap().config_error()
        );
        thread::sleep(Duration::from_millis(10));
    }
    eprintln!(
        "Bed Rust initialize + didOpen: {:.2}s",
        started.elapsed().as_secs_f64()
    );
    session
        .with_commands(view, |commands| {
            commands.set_cursor(row as i32, line.len() as i32, false, CursorReveal::Ensure);
            commands.delete_left(false);
        })
        .unwrap();
    assert!(session.snapshot(document).unwrap().dirty);
    let diagnostics = session
        .lsp()
        .unwrap()
        .diagnostics_for_document(document)
        .unwrap();
    loop {
        let report = session.tick();
        observe_progress(
            &session,
            document,
            &mut progress_titles,
            &mut active_progress_seen,
        );
        assert!(
            report.errors.is_empty(),
            "session errors: {:?}",
            report.errors
        );
        let items = diagnostics.for_document(&key);
        if items.iter().any(|item| {
            item.severity == 1
                && (item.message.to_lowercase().contains("brace")
                    || item.message.to_lowercase().contains("delimiter")
                    || item.message.to_lowercase().contains("expected"))
        }) {
            eprintln!(
                "Bed Rust syntax diagnostic: {:.2}s",
                started.elapsed().as_secs_f64()
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Bed Rust did not publish syntax diagnostics: {:?}; diagnostics: {:?}; events: {:?}",
            session.lsp().unwrap().server_statuses(),
            items,
            report.lsp_events
        );
        thread::sleep(Duration::from_millis(10));
    }
    session
        .with_commands(view, |commands| commands.undo())
        .unwrap();
    assert_eq!(session.snapshot(document).unwrap().bytes, original);
    while diagnostics.for_document(&key).iter().any(|item| {
        item.severity == 1
            && (item.message.to_lowercase().contains("brace")
                || item.message.to_lowercase().contains("delimiter")
                || item.message.to_lowercase().contains("expected"))
    }) {
        let report = session.tick();
        assert!(
            report.errors.is_empty(),
            "session errors: {:?}",
            report.errors
        );
        assert!(
            Instant::now() < deadline,
            "Bed Rust syntax diagnostics did not clear after undo: {:?}",
            session.lsp().unwrap().server_statuses()
        );
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        fs::read(&path).unwrap(),
        original,
        "acceptance must never save the edited repository buffer"
    );
    assert!(
        active_progress_seen,
        "real rust-analyzer must report active workspace work"
    );
    assert!(
        !progress_titles.is_empty(),
        "expected actual workspace work-done progress"
    );
    session.shutdown(ClosePolicy::Discard).unwrap();
}

#[test]
#[ignore = "requires installed rust-analyzer and rust-src; exercises real Cargo workspace analysis"]
fn real_bed_repository_hover_and_definition() {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let temporary = std::env::temp_dir().join(format!("bed-real-rust-lsp-{}", std::process::id()));
    fs::create_dir_all(&temporary).unwrap();
    let cleanup = Cleanup(temporary);
    let config = cleanup.0.join("lsp.json");
    fs::write(&config, serde_json::to_vec(&json!({
        "languages":[{"name":"rust","language_id":"rust","file_types":["rs"],"language_server":"rust-analyzer"}],
        "language_servers":{"rust-analyzer":{"command":"rust-analyzer"}}
    })).unwrap()).unwrap();
    let path = repo.join("crates/bed-lsp/src/lsp_client.rs");
    let bytes = fs::read(&path).unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    let (row, line) = text
        .lines()
        .enumerate()
        .find(|(_, line)| line.contains("config: LspConfig,"))
        .unwrap();
    let column = line.find("LspConfig").unwrap() + 2;
    let id = DocumentId(9001);
    let mut pool = WorkspaceLsp::new(WorkspaceId(9000), config, repo);
    pool.register_document(id, path.to_str().unwrap(), &bytes, 0, "rust")
        .unwrap();
    let client = pool.client_for_document(id).unwrap();
    let deadline = Instant::now() + Duration::from_secs(120);
    while !client.borrow().is_initialized() {
        pool.poll();
        assert!(
            Instant::now() < deadline,
            "rust-analyzer initialize timed out: {:?}",
            pool.server_statuses()
        );
        assert!(
            client.borrow().is_process_started(),
            "rust-analyzer exited: {:?}",
            pool.server_statuses()
        );
        thread::sleep(Duration::from_millis(10));
    }
    let params = json!({"textDocument":{"uri":bed_lsp::lsp_uri::LspUri::file_uri_from_path(path.to_str().unwrap()).unwrap().to_string()},"position":{"line":row,"character":column}});
    let mut hover_seen = false;
    let mut definition_seen = false;
    while !(hover_seen && definition_seen) {
        for method in ["textDocument/hover", "textDocument/definition"] {
            let response = Rc::new(RefCell::new(None));
            let received = response.clone();
            let origin = pool.request_origin(id, ViewId(9002), 0, 0).unwrap();
            pool.send_request(origin, method, params.clone(), move |reply| {
                *received.borrow_mut() = Some(reply)
            })
            .unwrap();
            while response.borrow().is_none() {
                pool.poll();
                assert!(
                    Instant::now() < deadline,
                    "rust-analyzer request timed out: {:?}",
                    pool.server_statuses()
                );
                thread::sleep(Duration::from_millis(10));
            }
            let response = response.borrow_mut().take().unwrap();
            assert!(pool.origin_is_current(&response.origin));
            match response.result {
                Ok(value) if method == "textDocument/hover" => {
                    if value != Value::Null {
                        hover_seen = value["contents"].to_string().contains("LspConfig");
                    }
                }
                Ok(value) => {
                    definition_seen =
                        from_definition_result(&value)
                            .unwrap()
                            .iter()
                            .any(|location| {
                                location.file.ends_with("/crates/bed-lsp/src/lsp_config.rs")
                            });
                }
                Err(error) if error.code == -32801 || error.code == -32800 => {} // workspace loaded or request superseded
                Err(error) => panic!("real rust-analyzer request failed: {error}"),
            }
        }
        assert!(
            Instant::now() < deadline,
            "rust-analyzer did not resolve real Bed symbols: {:?}",
            pool.server_statuses()
        );
        if !(hover_seen && definition_seen) {
            thread::sleep(Duration::from_millis(50));
        }
    }
    pool.unregister_document(id).unwrap();
    pool.shutdown();
}
