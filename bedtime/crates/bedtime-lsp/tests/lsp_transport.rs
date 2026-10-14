//! Deterministic integration coverage for this crate, independent of bEd.
use bedtime_editing::editing_events::DocumentChange;
use bedtime_editing::identity::DocumentId;
use bedtime_editing::identity::ViewId;
use bedtime_lsp::jsonrpc::CONNECTION_CLOSED;
use bedtime_lsp::jsonrpc::REQUEST_CANCELLED;
use bedtime_lsp::message_handler::MAX_STDERR_BYTES;
use bedtime_lsp::message_handler::RpcEvent;
use bedtime_lsp::process::ProcessOptions;
use bedtime_lsp::workspace_lsp::LspRequestOrigin;
include!("../../../tests/support/lsp_fixture.rs");
#[path = "../../../tests/support/runner.rs"]
mod runner;
fn main() {
    if dispatch_fixture() {
        return;
    }
    runner::run(&[
        (
            "initialize_and_latest_queued_open",
            initialize_and_latest_queued_open,
        ),
        (
            "project_check_configuration_waits_for_initialized_and_updates_live",
            project_check_configuration_waits_for_initialized_and_updates_live,
        ),
        (
            "incremental_utf16_order_and_save",
            incremental_utf16_order_and_save,
        ),
        (
            "full_none_and_save_provider_laziness",
            full_none_and_save_provider_laziness,
        ),
        (
            "registered_server_requests_and_batches",
            registered_server_requests_and_batches,
        ),
        (
            "callbacks_wait_for_main_poll_and_reverse_ids",
            callbacks_wait_for_main_poll_and_reverse_ids,
        ),
        (
            "diagnostics_wait_for_main_poll_and_versions",
            diagnostics_wait_for_main_poll_and_versions,
        ),
        ("fragmented_unicode_frames", fragmented_unicode_frames),
        (
            "malformed_complete_frames_recover",
            malformed_complete_frames_recover,
        ),
        (
            "malformed_failure_limit_disconnects",
            malformed_failure_limit_disconnects,
        ),
        (
            "fatal_framing_and_eof_fail_pending",
            fatal_framing_and_eof_fail_pending,
        ),
        (
            "crash_restart_and_initialize_error",
            crash_restart_and_initialize_error,
        ),
        (
            "literal_argv_and_missing_executable",
            literal_argv_and_missing_executable,
        ),
        ("stderr_drain_is_bounded", stderr_drain_is_bounded),
        (
            "silent_and_blocked_writer_shutdown_are_bounded",
            silent_and_blocked_writer_shutdown_are_bounded,
        ),
        (
            "inherited_descendant_pipe_does_not_hold_shutdown",
            inherited_descendant_pipe_does_not_hold_shutdown,
        ),
        (
            "reload_failure_preserves_configuration_and_server",
            reload_failure_preserves_configuration_and_server,
        ),
        (
            "initialize_batch_preserves_handshake_order",
            initialize_batch_preserves_handshake_order,
        ),
        (
            "native_fixture_supports_navigation_hover_and_diagnostics",
            native_fixture_supports_navigation_hover_and_diagnostics,
        ),
        (
            "child_workspace_and_environment_are_local",
            child_workspace_and_environment_are_local,
        ),
        (
            "workspace_languages_and_roots_are_independent",
            workspace_languages_and_roots_are_independent,
        ),
        (
            "workspace_registration_close_and_retry_are_per_document",
            workspace_registration_close_and_retry_are_per_document,
        ),
        (
            "workspace_pending_close_and_stale_request_routes",
            workspace_pending_close_and_stale_request_routes,
        ),
        (
            "workspace_failure_stderr_and_restart_are_actionable",
            workspace_failure_stderr_and_restart_are_actionable,
        ),
    ]);
}
fn initialize_and_latest_queued_open() {
    let mut fixture = Fixture::new("delayed");
    assert!(!fixture.client.is_initialized());
    fixture
        .client
        .did_open(&fixture.document, b"old", 1, "provided")
        .unwrap();
    fixture
        .client
        .did_change(&fixture.document, 2, &[], || Ok("latest😀".into()))
        .unwrap();
    fixture.ready();
    let messages = fixture.transcript();
    assert_eq!(messages[0]["method"], "initialize");
    let params = &messages[0]["params"];
    assert_eq!(params["processId"], std::process::id());
    assert_eq!(params["clientInfo"], json!({"name":"bed"}));
    assert_eq!(params["rootPath"], fixture.root.to_str().unwrap());
    assert_eq!(
        params["capabilities"],
        json!({
            "window":{"workDoneProgress":true},
            "workspace":{
                "workspaceFolders":true,"configuration":true,"applyEdit":true,
                "workspaceEdit":{"documentChanges":true,"failureHandling":"undo"},
                "diagnostics":{"refreshSupport":true}
            },
            "textDocument":{
                "diagnostic":{"dynamicRegistration":true,"relatedDocumentSupport":true},
                "synchronization":{"didSave":true},"hover":{"contentFormat":["markdown","plaintext"]},
                "definition":{"linkSupport":true},"references":{},
                "publishDiagnostics":{"relatedInformation":true},
                "completion":{
                    "contextSupport":true,
                    "completionList":{"itemDefaults":["data","editRange","insertTextFormat"]},
                    "completionItem":{
                        "snippetSupport":false,"documentationFormat":["markdown","plaintext"],
                        "resolveSupport":{"properties":["documentation","detail","additionalTextEdits"]},
                        "insertReplaceSupport":true,"labelDetailsSupport":true
                    }
                },
                "formatting":{},"rename":{"prepareSupport":true},
                "codeAction":{
                    "codeActionLiteralSupport":{"codeActionKind":{"valueSet":["quickfix"]}},
                    "isPreferredSupport":true,"disabledSupport":true,"dataSupport":true,
                    "resolveSupport":{"properties":["edit","command"]}
                }
            },
            "general":{"positionEncodings":["utf-16"]}
        })
    );
    assert_eq!(messages[1]["method"], "initialized");
    assert_eq!(messages[2]["method"], "workspace/didChangeConfiguration");
    assert_eq!(messages[3]["method"], "textDocument/didOpen");
    assert_eq!(messages[3]["params"]["textDocument"]["text"], "latest😀");
    assert_eq!(
        messages[3]["params"]["textDocument"]["languageId"],
        "provided"
    );
    assert_eq!(messages[3]["params"]["textDocument"]["version"], 2);
    assert_eq!(
        messages
            .iter()
            .filter(|message| message["method"] == "textDocument/didOpen")
            .count(),
        1
    );
    assert!(fixture.client.is_document_open(&fixture.document));
}

fn project_check_configuration_waits_for_initialized_and_updates_live() {
    let mut fixture = Fixture::with_server(
        "delayed",
        "rust-analyzer",
        json!({"rust-analyzer":{"checkOnSave":true,"cargo":{"allFeatures":true}}}),
    );
    assert!(!fixture.client.is_initialized());
    fixture
        .client
        .did_open(&fixture.document, b"fn main() {}", 1, "rust")
        .unwrap();
    // Workspace restoration can enable Bed's checks after the server starts.
    // Changes during the handshake must use the latest settings exactly once.
    fixture.client.set_project_check_owner(true);
    fixture.client.set_project_check_owner(false);
    fixture.client.set_project_check_owner(true);
    fixture.ready();
    let messages = fixture.transcript();
    assert_eq!(messages[0]["method"], "initialize");
    assert_eq!(messages[1]["method"], "initialized");
    assert_eq!(messages[2]["method"], "workspace/didChangeConfiguration");
    assert_eq!(
        messages[2]["params"]["settings"],
        json!({"rust-analyzer":{"checkOnSave":false,"cargo":{"allFeatures":true}}})
    );
    assert_eq!(messages[3]["method"], "textDocument/didOpen");
    assert_eq!(
        messages
            .iter()
            .filter(|message| message["method"] == "workspace/didChangeConfiguration")
            .count(),
        1
    );
    assert!(fixture.client.is_document_open(&fixture.document));

    fixture.client.set_project_check_owner(false);
    fixture.client.set_project_check_owner(true);
    let messages = fixture.transcript();
    let settings: Vec<_> = messages
        .iter()
        .filter(|message| message["method"] == "workspace/didChangeConfiguration")
        .map(|message| message["params"]["settings"].clone())
        .collect();
    assert_eq!(
        settings,
        [
            json!({"rust-analyzer":{"checkOnSave":false,"cargo":{"allFeatures":true}}}),
            json!({"rust-analyzer":{"checkOnSave":true,"cargo":{"allFeatures":true}}}),
            json!({"rust-analyzer":{"checkOnSave":false,"cargo":{"allFeatures":true}}}),
        ]
    );
}

fn incremental_utf16_order_and_save() {
    let mut fixture = Fixture::new("basic");
    fixture.ready();
    fixture
        .client
        .did_open(&fixture.document, "a😀b".as_bytes(), 1, "mock")
        .unwrap();
    let changes = [
        DocumentChange {
            start_line: 0,
            start_character: 3,
            end_line: 0,
            end_character: 4,
            text: b"x".to_vec(),
        },
        DocumentChange {
            start_line: 0,
            start_character: 1,
            end_line: 0,
            end_character: 3,
            text: "🚀".as_bytes().to_vec(),
        },
    ];
    fixture
        .client
        .did_change(&fixture.document, 2, &changes, || {
            panic!("incremental sync must not join document")
        })
        .unwrap();
    let invalid = [DocumentChange {
        text: vec![0xff],
        ..DocumentChange::default()
    }];
    assert_eq!(
        fixture
            .client
            .did_change(&fixture.document, 3, &invalid, || panic!(
                "invalid bytes must not join document"
            ))
            .unwrap_err()
            .kind(),
        io::ErrorKind::InvalidData
    );
    fixture
        .client
        .did_save(&fixture.document, || Ok("a🚀x".into()))
        .unwrap();
    fixture.client.did_close(&fixture.document).unwrap();
    assert!(!fixture.client.is_document_open(&fixture.document));
    let messages = fixture.transcript();
    let changed = messages
        .iter()
        .find(|message| message["method"] == "textDocument/didChange")
        .unwrap();
    assert_eq!(
        changed["params"]["contentChanges"][0]["range"]["start"]["character"],
        3
    );
    assert_eq!(changed["params"]["contentChanges"][1]["text"], "🚀");
    assert_eq!(changed["params"]["textDocument"]["version"], 2);
    let saved = messages
        .iter()
        .find(|message| message["method"] == "textDocument/didSave")
        .unwrap();
    assert_eq!(saved["params"]["text"], "a🚀x");
    assert!(
        messages
            .iter()
            .any(|message| message["method"] == "textDocument/didClose")
    );
}

fn full_none_and_save_provider_laziness() {
    for scenario in ["full", "none", "save-no-text"] {
        let mut fixture = Fixture::new(scenario);
        fixture.ready();
        fixture
            .client
            .did_open(&fixture.document, b"before", 1, "mock")
            .unwrap();
        let calls = RefCell::new(0);
        fixture
            .client
            .did_change(
                &fixture.document,
                2,
                &[DocumentChange {
                    text: b"after".to_vec(),
                    ..Default::default()
                }],
                || {
                    *calls.borrow_mut() += 1;
                    Ok("full after".into())
                },
            )
            .unwrap();
        if scenario == "none" {
            assert_eq!(*calls.borrow(), 0);
        } else if scenario == "full" {
            assert_eq!(*calls.borrow(), 1);
        }
        fixture
            .client
            .did_save(&fixture.document, || {
                panic!("save:false/options without text must not obtain text")
            })
            .unwrap();
        let messages = fixture.transcript();
        if scenario == "none" {
            assert!(
                !messages
                    .iter()
                    .any(|message| message["method"] == "textDocument/didChange")
            );
        } else if scenario == "full" {
            let change = messages
                .iter()
                .find(|message| message["method"] == "textDocument/didChange")
                .unwrap();
            assert_eq!(
                change["params"]["contentChanges"],
                json!([{"text":"full after"}])
            );
        }
        assert!(
            messages
                .iter()
                .filter(|message| message["method"] == "textDocument/didSave")
                .all(|message| message["params"].get("text").is_none())
        );
    }
}

fn registered_server_requests_and_batches() {
    let mut fixture = Fixture::new("basic");
    fixture.ready();
    let responses = fixture.request("test/handlers", json!({})).unwrap();
    let responses = responses.as_array().unwrap();
    assert_eq!(responses.len(), 6);
    let result = |id: &str| {
        responses
            .iter()
            .find(|response| response["id"] == id)
            .unwrap()
    };
    assert_eq!(result("config")["result"], json!([{}, {}]));
    assert_eq!(
        result("folders")["result"][0]["uri"],
        LspUri::file_uri_from_path(fixture.root.to_str().unwrap())
            .unwrap()
            .to_string()
    );
    for id in ["register", "progress", "show"] {
        assert!(result(id)["result"].is_null());
    }
    assert_eq!(result("unknown")["error"]["code"], METHOD_NOT_FOUND);
}

fn callbacks_wait_for_main_poll_and_reverse_ids() {
    let mut session = session("reverse", &[]);
    let replies = Rc::new(RefCell::new(Vec::new()));
    let main_thread = thread::current().id();
    for number in [1, 2] {
        let replies = replies.clone();
        session
            .send_request(
                "test/reverse",
                Some(json!({"number":number})),
                move |reply| {
                    assert_eq!(thread::current().id(), main_thread);
                    replies.borrow_mut().push(reply.unwrap());
                },
            )
            .unwrap();
    }
    thread::sleep(Duration::from_millis(50));
    assert!(replies.borrow().is_empty());
    wait_until(|| {
        session.poll();
        replies.borrow().len() == 2
    });
    assert_eq!(*replies.borrow(), vec![json!(2), json!(1)]);
    session.shutdown();
}

fn diagnostics_wait_for_main_poll_and_versions() {
    let mut fixture = Fixture::new("basic");
    fixture.ready();
    let store = fixture.client.diagnostics();
    let replied = Rc::new(RefCell::new(false));
    let received = replied.clone();
    fixture
        .client
        .send_request(
            "test/diagnostics",
            json!({"uri":LspUri::file_uri_from_path(&fixture.document).unwrap().to_string()}),
            move |reply| {
                reply.unwrap();
                *received.borrow_mut() = true;
            },
        )
        .unwrap();
    thread::sleep(Duration::from_millis(50));
    assert!(store.for_document(&fixture.document).is_empty());
    wait_until(|| {
        fixture.client.poll();
        *replied.borrow()
    });
    let items = store.for_document(&fixture.document);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].message, "**new**");
    assert_eq!(items[0].severity, 1);
    assert_eq!(items[0].start_character, 2);
    fixture
        .client
        .did_open(&fixture.document, b"xx", 3, "mock")
        .unwrap();
    fixture.client.did_close(&fixture.document).unwrap();
    assert_eq!(store.for_document(&fixture.document).len(), 1);
    assert_eq!(store.snapshot().values().map(Vec::len).sum::<usize>(), 1);
}

fn fragmented_unicode_frames() {
    let mut session = session("fragmented", &[]);
    assert_eq!(
        rpc_request(&mut session, "test/echo").unwrap(),
        "fragment😀🚀"
    );
    session.shutdown();
}

fn malformed_complete_frames_recover() {
    let mut session = session("malformed", &[]);
    let response = Rc::new(RefCell::new(None));
    let delivered = response.clone();
    session
        .send_request("test/malformed", None, move |reply| {
            *delivered.borrow_mut() = Some(reply);
        })
        .unwrap();
    let mut invalid = 0;
    wait_until(|| {
        invalid += session
            .poll()
            .iter()
            .filter(|event| matches!(event, RpcEvent::ProtocolError(_)))
            .count();
        response.borrow().is_some()
    });
    assert_eq!(response.borrow_mut().take().unwrap().unwrap(), "recovered");
    assert_eq!(invalid, 3);
    assert!(session.is_connected());
    session.shutdown();
}

fn malformed_failure_limit_disconnects() {
    let mut session = session("malformed-limit", &[]);
    let error = rpc_request(&mut session, "test/malformed").unwrap_err();
    assert_eq!(error.code, CONNECTION_CLOSED);
    assert!(!session.is_connected());
    session.shutdown();
}

fn fatal_framing_and_eof_fail_pending() {
    for scenario in [
        "negative",
        "overflow",
        "bare-lf",
        "oversized",
        "truncated",
        "eof",
        "unsupported-type",
        "unsupported-charset",
    ] {
        let mut session = session(scenario, &[]);
        let error = rpc_request(&mut session, "test/fatal").unwrap_err();
        assert_eq!(error.code, CONNECTION_CLOSED, "{scenario}");
        assert!(!session.is_connected());
        session.shutdown();
    }
}

fn crash_restart_and_initialize_error() {
    let mut fixture = Fixture::new("basic");
    fixture.ready();
    let error = fixture.request("test/crash", json!({})).unwrap_err();
    assert_eq!(error.code, CONNECTION_CLOSED);
    assert!(!fixture.client.is_initialized());
    assert!(fixture.client.init(&fixture.document).unwrap());
    fixture.ready();
    assert_eq!(fixture.request("test/echo", json!({})).unwrap(), "echo");
    let mut failed = Fixture::new("initialize-error");
    wait_until(|| {
        failed.client.poll();
        failed.client.last_error().is_some()
    });
    assert!(!failed.client.is_initialized());
    assert!(!failed.client.is_document_open(&failed.document));
    assert!(failed.client.last_error().unwrap().contains("not ready"));
}

fn literal_argv_and_missing_executable() {
    let extra = vec![
        "space argument".into(),
        "\"quotes\"".into(),
        "😀unicode".into(),
        "$(literal)".into(),
    ];
    let mut session = session("argv", &extra);
    let args = rpc_request(&mut session, "test/argv").unwrap();
    let args = args.as_array().unwrap();
    assert_eq!(
        args[3..],
        extra
            .iter()
            .map(|argument| json!(argument))
            .collect::<Vec<_>>()
    );
    session.shutdown();
    assert!(RpcSession::start(&PathBuf::from("/bed-missing-language-server"), &[]).is_err());
}

fn stderr_drain_is_bounded() {
    let mut fixture = Fixture::new("stderr-drain");
    fixture.ready();
    // Initialization arrives on stdout; its independent reader can finish
    // before the stderr reader consumes the end of the fixture's burst.
    wait_until(|| {
        fixture.client.poll();
        fixture
            .client
            .stderr_text()
            .ends_with(STDERR_DRAIN_SENTINEL)
    });
    let bytes = fixture.client.take_stderr();
    assert_eq!(bytes.len(), MAX_STDERR_BYTES);
    let prefix_length = bytes.len() - STDERR_DRAIN_SENTINEL.len();
    assert!(bytes[..prefix_length].iter().all(|byte| *byte == b'x'));
    assert_eq!(&bytes[prefix_length..], STDERR_DRAIN_SENTINEL.as_bytes());
    assert!(fixture.client.take_stderr().is_empty());
}

fn silent_and_blocked_writer_shutdown_are_bounded() {
    for scenario in ["silent", "blocked"] {
        let mut session = session(scenario, &[]);
        let result = Rc::new(RefCell::new(None));
        let delivered = result.clone();
        let payload = if scenario == "blocked" {
            json!({"large":"x".repeat(8*1024*1024)})
        } else {
            json!({})
        };
        session
            .send_request("test/never", Some(payload), move |reply| {
                *delivered.borrow_mut() = Some(reply);
            })
            .unwrap();
        let started = Instant::now();
        session.shutdown();
        assert!(
            started.elapsed() < Duration::from_millis(1500),
            "{scenario} shutdown hung"
        );
        assert_eq!(
            result.borrow_mut().take().unwrap().unwrap_err().code,
            REQUEST_CANCELLED
        );
        assert_eq!(session.pending_request_count(), 0);
        session.shutdown();
    }
}

fn inherited_descendant_pipe_does_not_hold_shutdown() {
    let mut session = session("descendant", &[]);
    let started = Instant::now();
    wait_until(|| {
        session.poll();
        !session.is_connected()
    });
    session.shutdown();
    assert!(started.elapsed() < Duration::from_millis(650));
}

fn reload_failure_preserves_configuration_and_server() {
    let mut fixture = Fixture::new("basic");
    fixture.ready();
    let pid = fixture.client.process_id();
    let configuration = fixture.client.config().clone();
    fs::write(fixture.client.config_path(), b"invalid JSON").unwrap();
    assert!(fixture.client.reload_config().is_err());
    assert_eq!(fixture.client.config(), &configuration);
    assert_eq!(fixture.client.process_id(), pid);
    assert!(fixture.client.is_initialized());
}

fn initialize_batch_preserves_handshake_order() {
    let mut fixture = Fixture::new("initialize-batch");
    fixture.ready();
    let messages = fixture.transcript();
    let initialized = messages
        .iter()
        .position(|message| message["method"] == "initialized")
        .unwrap();
    let response = messages
        .iter()
        .position(|message| message["id"] == "handshake-folders")
        .unwrap();
    assert!(initialized < response);
}

fn native_fixture_supports_navigation_hover_and_diagnostics() {
    let mut fixture = Fixture::new("native");
    fixture.ready();
    fixture
        .client
        .did_open(&fixture.document, b"fn main() {}", 0, "mock")
        .unwrap();
    wait_until(|| {
        fixture.client.poll();
        !fixture
            .client
            .diagnostics()
            .for_document(&fixture.document)
            .is_empty()
    });
    let params = json!({"textDocument":{"uri":LspUri::file_uri_from_path(&fixture.document).unwrap().to_string()},"position":{"line":0,"character":0},"context":{"includeDeclaration":false}});
    assert!(
        fixture
            .request("textDocument/hover", params.clone())
            .unwrap()["contents"]["value"]
            .as_str()
            .unwrap()
            .contains("```rust")
    );
    assert!(
        fixture
            .request("textDocument/definition", params.clone())
            .unwrap()["uri"]
            .is_string()
    );
    assert_eq!(
        fixture
            .request("textDocument/references", params)
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[allow(clippy::zombie_processes)] // Deliberately orphan one short-lived descriptor holder to test bounded cleanup.
fn child_workspace_and_environment_are_local() {
    let fixture = Fixture::new("basic");
    let marker = std::env::var_os("BED_LSP_PROCESS_TEST_MARKER");
    let cwd = std::env::current_dir().unwrap();
    let options = ProcessOptions {
        working_directory: Some(fixture.root.clone()),
        environment: vec![(
            "BED_LSP_PROCESS_TEST_MARKER".into(),
            "literal space; $not-a-shell 😀".into(),
        )],
    };
    let mut session = RpcSession::start_with_options(
        &std::env::current_exe().unwrap(),
        &["--mock-server".into(), "basic".into()],
        &options,
    )
    .unwrap();
    let environment = rpc_request(&mut session, "test/environment").unwrap();
    assert_eq!(
        fs::canonicalize(environment["cwd"].as_str().unwrap()).unwrap(),
        fs::canonicalize(&fixture.root).unwrap()
    );
    assert_eq!(environment["marker"], "literal space; $not-a-shell 😀");
    assert_eq!(std::env::current_dir().unwrap(), cwd);
    assert_eq!(std::env::var_os("BED_LSP_PROCESS_TEST_MARKER"), marker);
    session.shutdown();
}

fn workspace_languages_and_roots_are_independent() {
    for languages in [["cpp", "rust"], ["rust", "cpp"]] {
        let mut fixture = PoolFixture::new(20, "native");
        for (index, language) in languages.iter().enumerate() {
            let path = fixture.path(&format!("document.{language}").replace(".rust", ".rs"));
            fixture
                .pool
                .register_document(
                    DocumentId(index as u64 + 1),
                    &path,
                    b"let value = 1;",
                    0,
                    language,
                )
                .unwrap();
        }
        fixture.ready();
        let statuses = fixture.pool.server_statuses();
        assert!(statuses.iter().all(|status| status.initialized));
        assert_ne!(statuses[0].process_id, statuses[1].process_id);
        for language in ["cpp", "rust"] {
            let transcript = fixture.transcript(language);
            let opens: Vec<_> = transcript
                .iter()
                .filter(|message| message["method"] == "textDocument/didOpen")
                .collect();
            assert_eq!(opens.len(), 1);
            assert_eq!(opens[0]["params"]["textDocument"]["languageId"], language);
            assert_eq!(
                transcript[0]["params"]["rootPath"],
                fixture.root.to_str().unwrap()
            );
        }
        let cpp = fixture.pool.client_for_language("cpp").unwrap();
        cpp.borrow_mut().shutdown();
        assert!(
            fixture
                .pool
                .client_for_language("rust")
                .unwrap()
                .borrow()
                .is_initialized()
        );
        let rust_id = languages
            .iter()
            .position(|language| *language == "rust")
            .unwrap() as u64
            + 1;
        assert!(
            !fixture
                .pool
                .diagnostics_for_document(DocumentId(rust_id))
                .unwrap()
                .for_document(&fixture.path("document.rs"))
                .is_empty()
        );
    }
    let mut first = PoolFixture::new(30, "basic");
    let mut second = PoolFixture::new(31, "basic");
    for fixture in [&mut first, &mut second] {
        fixture
            .pool
            .register_document(
                DocumentId(1),
                &fixture.path("same.rs"),
                b"different roots",
                1,
                "rust",
            )
            .unwrap();
        fixture.ready();
        assert_eq!(
            fixture.transcript("rust")[0]["params"]["rootPath"],
            fixture.root.to_str().unwrap()
        );
    }
    assert_ne!(
        first
            .pool
            .client_for_language("rust")
            .unwrap()
            .borrow()
            .process_id(),
        second
            .pool
            .client_for_language("rust")
            .unwrap()
            .borrow()
            .process_id()
    );
    // A host may retain all document/view/workspace identities while changing
    // its root or config and replacing the pool. Epochs must never start over.
    let origin = first
        .pool
        .request_origin(DocumentId(1), ViewId(2), 3, 4)
        .unwrap();
    let mut replacement = WorkspaceLsp::new(
        first.pool.session_id(),
        first.pool.config_path().to_owned(),
        first.root.clone(),
    );
    replacement.set_server_arguments("rust", vec!["--mock-server".into(), "basic".into()]);
    replacement
        .register_document(
            DocumentId(1),
            &first.path("same.rs"),
            b"replacement",
            1,
            "rust",
        )
        .unwrap();
    let next = replacement
        .request_origin(DocumentId(1), ViewId(2), 3, 4)
        .unwrap();
    assert_ne!(next.server_generation, origin.server_generation);
    assert!(!replacement.origin_is_current(&origin));
}

fn workspace_registration_close_and_retry_are_per_document() {
    let mut fixture = PoolFixture::new(40, "basic");
    let first = fixture.path("first.rs");
    let second = fixture.path("second.rs");
    fixture
        .pool
        .register_document(DocumentId(1), &first, b"first", 1, "rust")
        .unwrap();
    fixture
        .pool
        .register_document(DocumentId(1), &first, b"first", 1, "rust")
        .unwrap();
    fixture
        .pool
        .register_document(DocumentId(2), &second, b"second", 2, "rust")
        .unwrap();
    fixture
        .pool
        .register_document(
            DocumentId(3),
            &fixture.path("other.cpp"),
            b"other",
            1,
            "cpp",
        )
        .unwrap();
    fixture.ready();
    let cpp = fixture
        .pool
        .client_for_language("cpp")
        .unwrap()
        .borrow()
        .process_id();
    assert_eq!(
        fixture
            .transcript("rust")
            .iter()
            .filter(|message| message["method"] == "textDocument/didOpen")
            .count(),
        2
    );
    fixture
        .pool
        .update_document_snapshot(DocumentId(1), &first, b"latest unsaved", 9, "rust")
        .unwrap();
    assert!(fixture.pool.retry_language("rust").unwrap());
    fixture.ready();
    assert_eq!(
        fixture
            .pool
            .client_for_language("cpp")
            .unwrap()
            .borrow()
            .process_id(),
        cpp
    );
    let transcript = fixture.transcript("rust");
    let opens: Vec<_> = transcript
        .iter()
        .filter(|message| message["method"] == "textDocument/didOpen")
        .collect();
    assert_eq!(opens.len(), 2);
    assert_eq!(opens[0]["params"]["textDocument"]["text"], "latest unsaved");
    assert_eq!(opens[0]["params"]["textDocument"]["version"], 9);
    fixture.pool.unregister_document(DocumentId(1)).unwrap();
    fixture.pool.unregister_document(DocumentId(1)).unwrap();
    let transcript = fixture.transcript("rust");
    assert_eq!(
        transcript
            .iter()
            .filter(|message| message["method"] == "textDocument/didClose")
            .count(),
        1
    );
    assert!(fixture.pool.client_for_document(DocumentId(1)).is_none());
}

fn workspace_pending_close_and_stale_request_routes() {
    let mut fixture = PoolFixture::new(50, "delayed");
    let path = fixture.path("closed.rs");
    fixture
        .pool
        .register_document(DocumentId(1), &path, b"closed before ready", 1, "rust")
        .unwrap();
    fixture.pool.unregister_document(DocumentId(1)).unwrap();
    fixture.ready();
    assert_eq!(
        fixture
            .transcript("rust")
            .iter()
            .filter(|message| message["method"] == "textDocument/didOpen")
            .count(),
        0
    );
    let path = fixture.path("kept.rs");
    fixture
        .pool
        .register_document(DocumentId(2), &path, b"kept", 2, "rust")
        .unwrap();
    let origin = fixture
        .pool
        .request_origin(DocumentId(2), ViewId(3), 4, 5)
        .unwrap();
    assert!(fixture.pool.origin_is_current(&origin));
    assert!(!fixture.pool.origin_is_current(&LspRequestOrigin {
        session_id: SessionId(51),
        ..origin
    }));
    let result = Rc::new(RefCell::new(None));
    let received = result.clone();
    fixture
        .pool
        .send_request(origin, "test/echo", json!({}), move |response| {
            *received.borrow_mut() = Some(response)
        })
        .unwrap();
    fixture
        .pool
        .update_document_snapshot(DocumentId(2), &path, b"changed", 3, "rust")
        .unwrap();
    wait_until(|| {
        fixture.pool.poll();
        result.borrow().is_some()
    });
    let response = result.borrow_mut().take().unwrap();
    assert_eq!(response.origin, origin);
    assert!(!fixture.pool.origin_is_current(&response.origin));
    assert!(fixture.pool.retry_language("rust").unwrap());
    assert!(!fixture.pool.origin_is_current(&origin));
}

fn workspace_failure_stderr_and_restart_are_actionable() {
    let mut fixture = PoolFixture::new(60, "stderr");
    let path = fixture.path("failure.rs");
    fixture
        .pool
        .register_document(DocumentId(1), &path, b"first", 0, "rust")
        .unwrap();
    fixture.ready();
    let status = fixture
        .pool
        .server_statuses()
        .into_iter()
        .find(|s| s.language == "rust")
        .unwrap();
    assert_eq!(status.stderr.len(), MAX_STDERR_BYTES);
    assert!(status.stderr.bytes().all(|byte| byte == b'x'));
    let client = fixture.pool.client_for_document(DocumentId(1)).unwrap();
    let crashed = Rc::new(RefCell::new(None));
    let received = crashed.clone();
    client
        .borrow_mut()
        .send_request("test/crash", json!({}), move |reply| {
            *received.borrow_mut() = Some(reply)
        })
        .unwrap();
    wait_until(|| {
        fixture.pool.poll();
        crashed.borrow().is_some()
    });
    let status = fixture
        .pool
        .server_statuses()
        .into_iter()
        .find(|s| s.language == "rust")
        .unwrap();
    assert!(!status.initialized);
    assert!(status.last_error.as_deref().unwrap().contains("xxxx"));
    fixture
        .pool
        .set_server_arguments("rust", vec!["--mock-server".into(), "basic".into()]);
    fixture.pool.retry_language("rust").unwrap();
    fixture.ready();
    assert!(Rc::ptr_eq(
        &client,
        &fixture.pool.client_for_document(DocumentId(1)).unwrap()
    ));
    let status = fixture
        .pool
        .server_statuses()
        .into_iter()
        .find(|s| s.language == "rust")
        .unwrap();
    assert!(status.last_error.is_none());
    assert!(status.stderr.is_empty());
    assert_eq!(
        fixture
            .transcript("rust")
            .iter()
            .filter(|m| m["method"] == "textDocument/didOpen")
            .count(),
        1
    );
    let missing = fixture.root.join("missing.json");
    fs::write(&missing,serde_json::to_vec(&json!({
        "languages":[{"name":"rust","language_id":"rust","file_types":["rs"],"language_server":"rust"}],
        "language_servers":{"rust":{"command":"/bed-guaranteed-no-rust-server"}}
    })).unwrap()).unwrap();
    let mut pool = WorkspaceLsp::new(SessionId(61), missing, fixture.root.clone());
    pool.register_document(
        DocumentId(2),
        &fixture.path("missing.rs"),
        b"retained editor bytes",
        0,
        "rust",
    )
    .unwrap();
    assert!(
        pool.server_statuses()[0]
            .last_error
            .as_deref()
            .unwrap()
            .contains("No executable found")
    );
    assert!(pool.server_statuses()[0].process_id.is_none());
}
