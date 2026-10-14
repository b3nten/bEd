//! Deterministic integration coverage for this crate, independent of bEd.
use bedtime_editing::identity::DocumentId;
use bedtime_editing::identity::ViewId;
include!("../../support/lsp_fixture.rs");
use bedtime_documents::{
    document_runtime::DocumentRuntime,
    session::{ClosePolicy, DocumentSession, SessionOptions},
};

#[test]
fn editor_bridge_synchronizes_edits_save_and_switch() {
    let mut fixture = Fixture::new("basic");
    fixture.ready();
    let empty = LspClient::new(fixture.root.join("unused.json"));
    let shared = Rc::new(RefCell::new(std::mem::replace(&mut fixture.client, empty)));
    let mut editor = DocumentRuntime::new();
    editor.bind_lsp_client(Some(shared.clone()));
    editor
        .api()
        .on_project_opened(fixture.root.to_str().unwrap());
    editor
        .api()
        .open_document(&fixture.document, "a😀b\n".as_bytes());
    editor.commands().set_cursor(
        0,
        5,
        false,
        bedtime_editing::text_commands::CursorReveal::Ensure,
    );
    editor.commands().type_text(b"X");
    assert!(editor.save().unwrap());
    assert_eq!(fs::read(&fixture.document).unwrap(), "a😀Xb\n".as_bytes());
    let second = fixture.root.join("other.rs").to_string_lossy().into_owned();
    editor.api().open_document(&second, b"second");
    assert!(shared.borrow().is_document_open(&second));
    assert!(!shared.borrow().is_document_open(&fixture.document));
    let messages = shared_request(&shared, "test/transcript");
    let messages = messages.as_array().unwrap();
    let change = messages
        .iter()
        .find(|message| message["method"] == "textDocument/didChange")
        .unwrap();
    assert_eq!(
        change["params"]["contentChanges"][0]["range"]["start"]["character"],
        3
    );
    assert_eq!(change["params"]["contentChanges"][0]["text"], "X");
    let save = messages
        .iter()
        .find(|message| message["method"] == "textDocument/didSave")
        .unwrap();
    assert_eq!(save["params"]["text"], "a😀Xb\n");
    let close = messages
        .iter()
        .position(|message| message["method"] == "textDocument/didClose")
        .unwrap();
    let open = messages
        .iter()
        .rposition(|message| message["method"] == "textDocument/didOpen")
        .unwrap();
    assert!(close < open);
    editor.bind_lsp_client(None);
    shared.borrow_mut().shutdown();
}

#[test]
fn shared_session_pulls_unsaved_diagnostics_and_clears_them_on_undo() {
    use bedtime_editing::text_commands::CursorReveal;
    let fixture = PoolFixture::new(9901, "basic");
    let path = fixture.root.join("unsaved.rs");
    fs::write(&path, b"valid\n").unwrap();
    let mut session = DocumentSession::with_options(SessionOptions {
        project_root: Some(fixture.root.clone()),
        lsp_config: Some(fixture.root.join("lsp.json")),
        ..SessionOptions::default()
    })
    .unwrap();
    session
        .lsp_mut()
        .unwrap()
        .set_server_arguments("rust", vec!["--mock-server".into(), "document-pull".into()]);
    let document = session.open_file(&path).unwrap();
    let canonical = session.snapshot(document).unwrap().path;
    let client = session
        .lsp()
        .unwrap()
        .client_for_document(document)
        .unwrap();
    let diagnostics = client.borrow().diagnostics();
    wait_until(|| {
        session.tick();
        client.borrow().is_initialized()
    });
    let left = session.create_view(document).unwrap();
    let right = session.create_view(document).unwrap();
    session
        .with_commands(left, |commands| {
            commands.set_cursor(0, 5, false, CursorReveal::Ensure);
            commands.type_text(b"INVALID");
        })
        .unwrap();
    wait_until(|| {
        session.tick();
        !diagnostics.for_document(&canonical).is_empty()
    });
    assert_eq!(
        diagnostics.for_document(&canonical)[0].message,
        "Unsaved pull diagnostic"
    );
    assert_eq!(fs::read(&path).unwrap(), b"valid\n");
    assert!(session.snapshot(document).unwrap().dirty);
    assert!(!session.lsp().unwrap().diagnostic_coverage_complete());
    session
        .with_commands(right, |commands| commands.undo())
        .unwrap();
    wait_until(|| {
        session.tick();
        diagnostics.for_document(&canonical).is_empty()
    });
    assert_eq!(session.snapshot(document).unwrap().bytes, b"valid\n");
    let transcript = shared_request(&client, "test/transcript");
    let transcript = transcript.as_array().unwrap();
    assert_eq!(
        transcript
            .iter()
            .filter(|message| message["method"] == "textDocument/didOpen")
            .count(),
        1
    );
    assert!(
        transcript
            .iter()
            .any(|message| message["method"] == "textDocument/diagnostic")
    );
    assert!(
        !transcript
            .iter()
            .any(|message| message["method"] == "textDocument/didSave")
    );
    session.shutdown(ClosePolicy::Discard).unwrap();
}

#[test]
fn workspace_rebind_moves_document_between_language_sessions() {
    for scenario in ["basic", "delayed"] {
        let mut fixture = PoolFixture::new(80, "delayed");
        let old = fixture.path("before.rs");
        fixture
            .pool
            .register_document(DocumentId(1), &old, b"old", 1, "rust")
            .unwrap();
        if scenario == "basic" {
            fixture.ready();
        }
        let old_client = fixture.pool.client_for_document(DocumentId(1)).unwrap();
        let origin = fixture
            .pool
            .request_origin(DocumentId(1), ViewId(2), 3, 0)
            .unwrap();
        let new = fixture.path("after.cpp");
        fixture
            .pool
            .update_document_snapshot(DocumentId(1), &new, b"latest", 2, "cpp")
            .unwrap();
        fixture.ready();
        let new_client = fixture.pool.client_for_document(DocumentId(1)).unwrap();
        assert!(!Rc::ptr_eq(&old_client, &new_client));
        assert!(!fixture.pool.origin_is_current(&origin));
        let rust = fixture.transcript("rust");
        assert_eq!(
            rust.iter()
                .filter(|m| m["method"] == "textDocument/didOpen")
                .count(),
            usize::from(scenario == "basic")
        );
        assert_eq!(
            rust.iter()
                .filter(|m| m["method"] == "textDocument/didClose")
                .count(),
            usize::from(scenario == "basic")
        );
        let cpp = fixture.transcript("cpp");
        let opens: Vec<_> = cpp
            .iter()
            .filter(|m| m["method"] == "textDocument/didOpen")
            .collect();
        assert_eq!(opens.len(), 1);
        assert_eq!(opens[0]["params"]["textDocument"]["text"], "latest");
        assert_eq!(opens[0]["params"]["textDocument"]["languageId"], "cpp");
        fixture.pool.unregister_document(DocumentId(1)).unwrap();
        assert_eq!(
            fixture
                .transcript("cpp")
                .iter()
                .filter(|m| m["method"] == "textDocument/didClose")
                .count(),
            1
        );
    }
}

#[test]
fn workspace_config_reload_rebinds_documents_and_parse_failure_keeps_live_servers() {
    let mut fixture = PoolFixture::new(90, "basic");
    let first = fixture.path("first.rs");
    let second = fixture.path("second.rs");
    fixture
        .pool
        .register_document(DocumentId(1), &first, b"old", 1, "rust")
        .unwrap();
    fixture
        .pool
        .register_document(DocumentId(2), &second, b"second", 2, "rust")
        .unwrap();
    fixture.ready();
    let old = fixture.pool.client_for_document(DocumentId(1)).unwrap();
    let origin = fixture
        .pool
        .request_origin(DocumentId(1), ViewId(2), 3, 4)
        .unwrap();
    fixture
        .pool
        .update_document_snapshot(DocumentId(1), &first, b"latest", 3, "rust")
        .unwrap();
    fs::write(fixture.pool.config_path(),serde_json::to_vec(&json!({
        "languages":[{"name":"python","language_id":"python","file_types":["rs"],"language_server":"python"}],
        "language_servers":{"python":{"command":crate::fixtures::executable(),"args":["--stdio"]}}
    })).unwrap()).unwrap();
    fixture.pool.reload_config().unwrap();
    fixture.ready();
    let current = fixture.pool.client_for_document(DocumentId(1)).unwrap();
    assert!(!Rc::ptr_eq(&old, &current));
    assert!(!old.borrow().is_process_started());
    assert!(Rc::ptr_eq(
        &current,
        &fixture.pool.client_for_document(DocumentId(2)).unwrap()
    ));
    assert!(!fixture.pool.origin_is_current(&origin));
    let transcript = fixture.transcript("python");
    let opens: Vec<_> = transcript
        .iter()
        .filter(|m| m["method"] == "textDocument/didOpen")
        .collect();
    assert_eq!(opens.len(), 2);
    assert_eq!(opens[0]["params"]["textDocument"]["text"], "latest");
    assert_eq!(opens[0]["params"]["textDocument"]["version"], 3);
    let pid = current.borrow().process_id();
    fs::write(fixture.pool.config_path(), b"invalid JSON").unwrap();
    assert!(fixture.pool.reload_config().is_err());
    assert!(fixture.pool.config_error().is_some());
    assert!(
        fixture
            .pool
            .config()
            .language_servers
            .contains_key("python")
    );
    assert_eq!(current.borrow().process_id(), pid);
    assert!(current.borrow().is_initialized());
}

#[test]
fn layered_shared_servers_roots_and_reload() {
    let root = std::env::temp_dir().join(format!(
        "bed-layered-lsp-{}-{}",
        std::process::id(),
        TEMP_ID.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(root.join("packages/a/src")).unwrap();
    fs::create_dir_all(root.join("packages/b/src")).unwrap();
    fs::create_dir_all(root.join(".bed")).unwrap();
    let root = fs::canonicalize(root).unwrap();
    fs::write(root.join("Cargo.toml"), "workspace").unwrap();
    fs::write(root.join("packages/a/Cargo.toml"), "nested").unwrap();
    fs::write(root.join("packages/b/Cargo.toml"), "nested").unwrap();
    let config = root.join("user.json");
    fs::write(&config, serde_json::to_vec(&json!({
        "languages":[
            {"name":"foundation-a","language_id":"first-language","file_types":["foo"],"roots":["Cargo.toml"],"workspace_lsp_roots":["packages/a","packages/b"],"language_server":"foundation"},
            {"name":"foundation-b","language_id":"second-language","file_types":["bar"],"roots":["Cargo.toml"],"workspace_lsp_roots":["packages/a","packages/b"],"language_server":"foundation"}
        ],
        "language_servers":{"foundation":{"command":crate::fixtures::executable(),"args":["--mock-server","native"],"environment":{"BED_LSP_PROCESS_TEST_MARKER":"user"},"settings":{"foundation":{"value":"user"}}}}
    })).unwrap()).unwrap();
    let project = root.join(".bed/lsp.json");
    fs::write(&project, serde_json::to_vec(&json!({"language_servers":{"foundation":{"environment":{"BED_LSP_PROCESS_TEST_MARKER":"project"},"settings":{"foundation":{"value":"project"}}}}})).unwrap()).unwrap();
    let mut pool = WorkspaceLsp::new_layered(
        SessionId(1000),
        config,
        root.clone(),
        Some(PathBuf::from(".bed/lsp.json")),
    );
    let path = |relative: &str| root.join(relative).to_string_lossy().into_owned();
    let first = path("packages/a/src/first.foo");
    let second = path("packages/a/src/second.bar");
    let third = path("packages/b/src/third.foo");
    pool.register_document(DocumentId(1), &first, b"old", 1, "foo")
        .unwrap();
    pool.register_document(DocumentId(2), &second, b"second", 2, "bar")
        .unwrap();
    pool.register_document(DocumentId(3), &third, b"third", 3, "foo")
        .unwrap();
    pool.register_document(
        DocumentId(4),
        &path("packages/a/src/closed.foo"),
        b"closed before discovery",
        0,
        "foo",
    )
    .unwrap();
    pool.unregister_document(DocumentId(4)).unwrap();
    pool.update_document_snapshot(DocumentId(1), &first, b"latest unsaved", 4, "foo")
        .unwrap();
    wait_until(|| {
        pool.poll();
        !pool.is_discovering()
            && (1..=3).all(|id| {
                pool.client_for_document(DocumentId(id))
                    .is_some_and(|client| client.borrow().is_initialized())
            })
    });
    let a = pool.client_for_document(DocumentId(1)).unwrap();
    let b = pool.client_for_document(DocumentId(2)).unwrap();
    let c = pool.client_for_document(DocumentId(3)).unwrap();
    assert!(Rc::ptr_eq(&a, &b));
    assert!(!Rc::ptr_eq(&a, &c));
    let transcript = shared_request(&a, "test/transcript");
    let opens: Vec<_> = transcript
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["method"] == "textDocument/didOpen")
        .collect();
    assert_eq!(opens.len(), 2);
    assert_eq!(
        opens[0]["params"]["textDocument"]["languageId"],
        "first-language"
    );
    assert_eq!(opens[0]["params"]["textDocument"]["text"], "latest unsaved");
    assert_eq!(opens[0]["params"]["textDocument"]["version"], 4);
    assert_eq!(
        opens[1]["params"]["textDocument"]["languageId"],
        "second-language"
    );
    assert_eq!(
        transcript[0]["params"]["rootPath"],
        root.join("packages/a").to_str().unwrap()
    );
    assert_eq!(
        transcript
            .as_array()
            .unwrap()
            .iter()
            .find(|message| message["method"] == "workspace/didChangeConfiguration")
            .unwrap()["params"]["settings"]["foundation"]["value"],
        "project"
    );
    assert_eq!(shared_request(&a, "test/environment")["marker"], "project");
    // Expanding the index must not create a server for an unopened language.
    pool.note_project_paths(&[path("unopened.py")]);
    assert_eq!(
        pool.server_statuses()
            .iter()
            .filter(|status| status.process_id.is_some())
            .count(),
        2
    );
    assert!(!pool.diagnostic_coverage_complete());
    let origin = pool.request_origin(DocumentId(1), ViewId(1), 1, 0).unwrap();
    let pid = a.borrow().process_id();
    let pending = path("packages/a/src/pending.bar");
    pool.register_document(DocumentId(5), &pending, b"pending", 1, "bar")
        .unwrap();
    pool.update_document_snapshot(DocumentId(5), &pending, b"latest pending", 2, "bar")
        .unwrap();
    fs::write(&project, b"broken JSON").unwrap();
    pool.reload_config().unwrap();
    wait_until(|| {
        pool.poll();
        !pool.is_discovering() && pool.client_for_document(DocumentId(5)).is_some()
    });
    assert!(pool.config_error().unwrap().contains(".bed/lsp.json"));
    assert_eq!(a.borrow().process_id(), pid);
    assert!(pool.origin_is_current(&origin));
    let transcript = shared_request(&a, "test/transcript");
    let pending_open = transcript
        .as_array()
        .unwrap()
        .iter()
        .find(|message| {
            message["method"] == "textDocument/didOpen"
                && message["params"]["textDocument"]["uri"]
                    == LspUri::file_uri_from_path(&pending).unwrap().to_string()
        })
        .unwrap();
    assert_eq!(
        pending_open["params"]["textDocument"]["text"],
        "latest pending"
    );
    assert_eq!(pending_open["params"]["textDocument"]["version"], 2);
    // Marker observations invalidate cached listings and move only affected
    // documents to the newly resolved root.
    fs::remove_file(root.join("packages/a/Cargo.toml")).unwrap();
    fs::write(root.join("packages/a/src/Cargo.toml"), "new root").unwrap();
    pool.observe_file_changes(&[
        bedtime_remote::FilesystemChange::Removed {
            path: path("packages/a/Cargo.toml"),
        },
        bedtime_remote::FilesystemChange::Created {
            path: path("packages/a/src/Cargo.toml"),
        },
    ])
    .unwrap();
    wait_until(|| {
        pool.poll();
        !pool.is_discovering()
            && pool
                .client_for_document(DocumentId(1))
                .is_some_and(|client| client.borrow().is_initialized())
    });
    let moved = pool.client_for_document(DocumentId(1)).unwrap();
    assert!(!Rc::ptr_eq(&moved, &a));
    assert!(Rc::ptr_eq(
        &moved,
        &pool.client_for_document(DocumentId(2)).unwrap()
    ));
    assert!(Rc::ptr_eq(
        &c,
        &pool.client_for_document(DocumentId(3)).unwrap()
    ));
    assert_eq!(
        moved.borrow().workspace(),
        root.join("packages/a/src").to_str().unwrap()
    );
    assert!(!pool.origin_is_current(&origin));
    // Successful reload invalidates old replies and rebinds only enabled docs.
    fs::write(
        &project,
        serde_json::to_vec(&json!({"languages":[{"name":"foundation-a","enabled":false}]}))
            .unwrap(),
    )
    .unwrap();
    pool.reload_config().unwrap();
    wait_until(|| {
        pool.poll();
        !pool.is_discovering()
            && pool
                .client_for_document(DocumentId(2))
                .is_some_and(|client| client.borrow().is_initialized())
    });
    assert!(pool.client_for_document(DocumentId(1)).is_none());
    assert!(pool.client_for_document(DocumentId(3)).is_none());
    assert!(!pool.origin_is_current(&origin));
    assert!(!a.borrow().is_process_started());
    pool.shutdown();
    drop(pool);
    fs::remove_dir_all(root).unwrap();
}
