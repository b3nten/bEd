//! A deterministic stdio language server lives inside this test executable.
//! Cargo runs this target with harness=false; no installed interpreter/server is
//! required and the application gains no test-only CLI modes.
use std::io;

use bed_core::editor_events::DocumentChange;
use bed_lsp::{
    connection::{Connection, encode_packet, write_packet},
    jsonrpc::{
        CONNECTION_CLOSED, METHOD_NOT_FOUND, Message, Packet, REQUEST_CANCELLED, Request, Response,
        ResponseError, RpcId,
    },
    lsp_client::{LspClient, ProgressToken},
    lsp_uri::LspUri,
    message_handler::{MAX_STDERR_BYTES, RpcEvent, RpcSession},
    process::ProcessOptions,
    workspace_lsp::{LspRequestOrigin, WorkspaceLsp},
};
use bed_session::{
    editor::Editor,
    editor_session::{ClosePolicy, DocumentId, EditorSession, SessionOptions, ViewId, WorkspaceId},
};
use bed_ui::{
    editor_view::{EditorView, EditorViewOptions},
    lsp::lsp_ui::{ContextLspAction, LspUi},
};
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    fs,
    io::{BufReader, Write},
    path::PathBuf,
    process::{Command, Stdio},
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

fn main() {
    let arguments: Vec<String> = std::env::args().collect();
    if arguments
        .get(1)
        .is_some_and(|argument| argument == "--stdio")
    {
        mock_server("native");
        return;
    }
    if arguments
        .get(1)
        .is_some_and(|argument| argument == "--mock-server")
    {
        mock_server(arguments.get(2).map(String::as_str).unwrap_or("basic"));
        return;
    }
    if arguments
        .get(1)
        .is_some_and(|argument| argument == "--hold-pipes")
    {
        thread::sleep(Duration::from_millis(750));
        return;
    }
    let tests: &[(&str, fn())] = &[
        (
            "initialize_and_latest_queued_open",
            initialize_and_latest_queued_open,
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
            "reload_failure_clears_configuration",
            reload_failure_clears_configuration,
        ),
        (
            "initialize_batch_preserves_handshake_order",
            initialize_batch_preserves_handshake_order,
        ),
        (
            "editor_bridge_synchronizes_edits_save_and_switch",
            editor_bridge_synchronizes_edits_save_and_switch,
        ),
        (
            "shared_session_save_diagnostics_render_in_each_custom_view",
            shared_session_save_diagnostics_render_in_each_custom_view,
        ),
        (
            "work_done_progress_is_main_thread_owned_and_resets",
            work_done_progress_is_main_thread_owned_and_resets,
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
        (
            "typed_navigation_results_follow_the_requesting_view",
            typed_navigation_results_follow_the_requesting_view,
        ),
        (
            "workbench_single_definition_returns_to_requesting_view_without_references",
            workbench_single_definition_returns_to_requesting_view_without_references,
        ),
        (
            "workbench_multiple_and_stale_definitions_use_guarded_routes",
            workbench_multiple_and_stale_definitions_use_guarded_routes,
        ),
        (
            "workbench_primary_click_sends_clicked_utf16_position",
            workbench_primary_click_sends_clicked_utf16_position,
        ),
        (
            "workspace_rebind_moves_document_between_language_sessions",
            workspace_rebind_moves_document_between_language_sessions,
        ),
        (
            "workspace_config_reload_rebinds_documents_and_parse_failure_keeps_live_servers",
            workspace_config_reload_rebinds_documents_and_parse_failure_keeps_live_servers,
        ),
    ];
    for (name, test) in tests {
        print!("test {name} ... ");
        io::stdout().flush().unwrap();
        test();
        println!("ok");
    }
    println!("test result: ok. {} passed", tests.len());
}

static TEMP_ID: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    root: PathBuf,
    client: LspClient,
    document: String,
}
impl Fixture {
    fn new(scenario: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "bed-lsp-{}-{}",
            std::process::id(),
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let config = root.join("lsp.json");
        fs::write(&config, serde_json::to_vec(&json!({"languages":[{"language_name":"mock","language_file_extensions":[".rs"],"language_server_paths":[std::env::current_exe().unwrap()]}]})).unwrap()).unwrap();
        let document = root.join("sp ace😀.rs").to_string_lossy().into_owned();
        let mut client = LspClient::new(config);
        client.set_server_arguments("mock", vec!["--mock-server".into(), scenario.into()]);
        client.set_workspace(root.to_str().unwrap());
        assert!(client.init(&document).unwrap());
        Self {
            root,
            client,
            document,
        }
    }
    fn ready(&mut self) {
        wait_until(|| {
            self.client.poll();
            self.client.is_initialized()
        });
    }
    fn request(&mut self, method: &str, params: Value) -> Result<Value, ResponseError> {
        let result = Rc::new(RefCell::new(None));
        let delivered = result.clone();
        self.client
            .send_request(method, params, move |reply| {
                *delivered.borrow_mut() = Some(reply);
            })
            .unwrap();
        wait_until(|| {
            self.client.poll();
            result.borrow().is_some()
        });
        result.borrow_mut().take().unwrap()
    }
    fn transcript(&mut self) -> Vec<Value> {
        self.request("test/transcript", json!({}))
            .unwrap()
            .as_array()
            .unwrap()
            .clone()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.client.shutdown();
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn session(scenario: &str, extra: &[String]) -> RpcSession {
    let mut arguments = vec!["--mock-server".into(), scenario.into()];
    arguments.extend_from_slice(extra);
    RpcSession::start(&std::env::current_exe().unwrap(), &arguments).unwrap()
}
fn wait_until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !condition() {
        assert!(Instant::now() < deadline, "mock language server timed out");
        thread::sleep(Duration::from_millis(2));
    }
}
fn rpc_request(session: &mut RpcSession, method: &str) -> Result<Value, ResponseError> {
    let result = Rc::new(RefCell::new(None));
    let delivered = result.clone();
    session
        .send_request(method, Some(json!({})), move |reply| {
            *delivered.borrow_mut() = Some(reply);
        })
        .unwrap();
    wait_until(|| {
        session.poll();
        result.borrow().is_some()
    });
    result.borrow_mut().take().unwrap()
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
        json!({"window":{"workDoneProgress":true},"workspace":{"workspaceFolders":true,"configuration":true},"textDocument":{"synchronization":{"didSave":true},"hover":{},"definition":{"linkSupport":true},"references":{},"publishDiagnostics":{"relatedInformation":true}},"general":{"positionEncodings":["utf-16"]}})
    );
    assert_eq!(messages[1]["method"], "initialized");
    assert_eq!(messages[2]["method"], "textDocument/didOpen");
    assert_eq!(messages[2]["params"]["textDocument"]["text"], "latest😀");
    assert_eq!(messages[2]["params"]["textDocument"]["languageId"], "mock");
    assert_eq!(messages[2]["params"]["textDocument"]["version"], 2);
    assert_eq!(
        messages
            .iter()
            .filter(|message| message["method"] == "textDocument/didOpen")
            .count(),
        1
    );
    assert!(fixture.client.is_document_open(&fixture.document));
}

fn work_done_progress_is_main_thread_owned_and_resets() {
    let mut fixture = Fixture::new("basic");
    fixture.ready();
    let reply = Rc::new(RefCell::new(None));
    let received = Rc::clone(&reply);
    fixture
        .client
        .send_request("test/progress", json!({}), move |result| {
            *received.borrow_mut() = Some(result)
        })
        .unwrap();
    thread::sleep(Duration::from_millis(30));
    assert!(
        fixture.client.progress().is_empty(),
        "workers must not mutate progress before main-thread polling"
    );
    assert!(reply.borrow().is_none());
    let mut protocol_errors = Vec::new();
    wait_until(|| {
        protocol_errors.extend(
            fixture
                .client
                .poll()
                .into_iter()
                .filter(|event| matches!(event, RpcEvent::ProtocolError(_))),
        );
        reply.borrow().is_some()
    });
    assert!(reply.borrow_mut().take().unwrap().is_ok());
    assert!(
        protocol_errors.is_empty(),
        "partial/custom progress must be ignored: {protocol_errors:?}"
    );
    let jobs = fixture.client.progress();
    assert_eq!(jobs.len(), 2);
    assert_eq!(jobs[0].token, ProgressToken::String("index".into()));
    assert_eq!(jobs[0].title, "Indexing");
    assert_eq!(jobs[0].message.as_deref(), Some("Second crate"));
    assert_eq!(jobs[0].percentage, Some(50));
    assert!(!jobs[0].finished);
    assert_eq!(jobs[1].token, ProgressToken::Number(17));
    fixture.request("test/progress/end", json!({})).unwrap();
    assert!(fixture.client.progress().iter().all(|job| job.finished));
    assert_eq!(
        fixture.client.progress()[0].message.as_deref(),
        Some("Indexed")
    );
    fixture.client.shutdown();
    assert!(fixture.client.progress().is_empty());
    assert!(fixture.client.init(&fixture.document).unwrap());
    fixture.ready();
    assert!(fixture.client.progress().is_empty());
    fixture.request("test/progress", json!({})).unwrap();
    assert!(!fixture.client.progress().is_empty());
    assert_eq!(
        fixture.request("test/crash", json!({})).unwrap_err().code,
        CONNECTION_CLOSED
    );
    assert!(
        fixture.client.progress().is_empty(),
        "disconnect must clear stale loading activity"
    );

    let mut pool = PoolFixture::new(90, "basic");
    pool.pool
        .register_document(
            DocumentId(1),
            &pool.path("progress.rs"),
            b"fn main() {}",
            0,
            "rust",
        )
        .unwrap();
    pool.ready();
    let client = pool.pool.client_for_document(DocumentId(1)).unwrap();
    let reply = Rc::new(RefCell::new(None));
    let received = Rc::clone(&reply);
    client
        .borrow_mut()
        .send_request("test/progress", json!({}), move |result| {
            *received.borrow_mut() = Some(result)
        })
        .unwrap();
    wait_until(|| {
        pool.pool.poll();
        reply.borrow().is_some()
    });
    assert!(reply.borrow_mut().take().unwrap().is_ok());
    assert_eq!(
        pool.pool
            .server_statuses()
            .into_iter()
            .find(|status| status.language == "rust")
            .unwrap()
            .progress,
        client.borrow().progress()
    );
    // The actual docked dashboard body must draw active server work inside the
    // caller's native window, then stop presenting it as active after end.
    {
        use bed_ui::lsp::lsp_dashboard::LspDashboard;
        use dear_imgui_rs::{Condition, Context, FramePrepareOptions, sys};
        let mut context = Context::create();
        context.set_ini_filename(None::<PathBuf>).unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        let mut dashboard = LspDashboard::default();
        let draw =
            |context: &mut Context, dashboard: &mut LspDashboard, pool: &mut WorkspaceLsp| {
                context.prepare_frame(FramePrepareOptions::new([1000.0, 700.0], 1.0 / 60.0));
                let ui = context.frame();
                ui.window("Work-done dashboard")
                    .position([30.0, 30.0], Condition::Always)
                    .size([900.0, 600.0], Condition::Always)
                    .build(|| {
                        let geometry = (ui.window_pos(), ui.window_size());
                        assert!(dashboard.render_workspace_body(ui, pool).is_none());
                        assert_eq!(geometry, (ui.window_pos(), ui.window_size()));
                    });
                let active_vertices = ui.with_bound_context(|| unsafe {
                    let window = &*sys::igFindWindowByName(c"Work-done dashboard".as_ptr());
                    let draw = &*window.DrawList;
                    (0..draw.VtxBuffer.Size)
                        .filter(|index| {
                            (*draw.VtxBuffer.Data.add(*index as usize)).col
                                == u32::from_le_bytes([230, 204, 102, 255])
                        })
                        .count()
                });
                assert!(context.render_legacy().draw_data().total_vtx_count() > 0);
                active_vertices
            };
        draw(&mut context, &mut dashboard, &mut pool.pool);
        assert!(
            draw(&mut context, &mut dashboard, &mut pool.pool) > 0,
            "active workspace progress must be visible in the docked dashboard"
        );
        let done = Rc::new(RefCell::new(false));
        let received = Rc::clone(&done);
        client
            .borrow_mut()
            .send_request("test/progress/end", json!({}), move |result| {
                result.unwrap();
                *received.borrow_mut() = true;
            })
            .unwrap();
        wait_until(|| {
            pool.pool.poll();
            *done.borrow()
        });
        assert_eq!(
            draw(&mut context, &mut dashboard, &mut pool.pool),
            0,
            "finished progress must not remain visually active"
        );
    }
    assert!(pool.pool.retry_language("rust").unwrap());
    assert!(client.borrow().progress().is_empty());
}

fn incremental_utf16_order_and_save() {
    let mut fixture = Fixture::new("basic");
    fixture.ready();
    fixture
        .client
        .did_open(&fixture.document, "a😀b".as_bytes(), 1, "ignored")
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
    assert!(store.for_document(&fixture.document).is_empty());
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
    let mut fixture = Fixture::new("stderr");
    fixture.ready();
    let bytes = fixture.client.take_stderr();
    assert_eq!(bytes.len(), MAX_STDERR_BYTES);
    assert!(bytes.iter().all(|byte| *byte == b'x'));
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

fn reload_failure_clears_configuration() {
    let mut fixture = Fixture::new("basic");
    fixture.ready();
    fs::write(fixture.client.config_path(), b"invalid JSON").unwrap();
    assert!(fixture.client.reload_config().is_err());
    assert!(fixture.client.language_servers().is_empty());
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

fn shared_request(client: &Rc<RefCell<LspClient>>, method: &str) -> Value {
    shared_request_params(client, method, json!({}))
}
fn shared_request_params(client: &Rc<RefCell<LspClient>>, method: &str, params: Value) -> Value {
    let reply = Rc::new(RefCell::new(None));
    let delivered = reply.clone();
    client
        .borrow_mut()
        .send_request(method, params, move |result| {
            *delivered.borrow_mut() = Some(result);
        })
        .unwrap();
    wait_until(|| {
        client.borrow_mut().poll();
        reply.borrow().is_some()
    });
    reply.borrow_mut().take().unwrap().unwrap()
}

fn editor_bridge_synchronizes_edits_save_and_switch() {
    let mut fixture = Fixture::new("basic");
    fixture.ready();
    let empty = LspClient::new(fixture.root.join("unused.json"));
    let shared = Rc::new(RefCell::new(std::mem::replace(&mut fixture.client, empty)));
    let mut editor = Editor::new();
    editor.bind_lsp_client(Some(shared.clone()));
    editor
        .api()
        .on_project_opened(fixture.root.to_str().unwrap());
    editor
        .api()
        .open_document(&fixture.document, "a😀b\n".as_bytes());
    editor
        .commands()
        .set_cursor(0, 5, false, bed_core::editor_commands::CursorReveal::Ensure);
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

fn shared_session_save_diagnostics_render_in_each_custom_view() {
    use bed_core::editor_commands::CursorReveal;
    use bed_ui::views::diagnostic_style::severity_color;
    use dear_imgui_rs::{Condition, Context, FramePrepareOptions, sys};

    let fixture = PoolFixture::new(9900, "basic");
    let path = fixture.root.join("shared😀.rs");
    fs::write(&path, "a🙂b\n".as_bytes()).unwrap();
    let mut session = EditorSession::with_options(SessionOptions {
        project_root: Some(fixture.root.clone()),
        lsp_config: Some(fixture.root.join("lsp.json")),
        ..SessionOptions::default()
    })
    .unwrap();
    session.lsp_mut().unwrap().set_server_arguments(
        "rust",
        vec!["--mock-server".into(), "session-diagnostics".into()],
    );
    let document = session.open_file(&path).unwrap();
    let canonical = session.snapshot(document).unwrap().path;
    let client = session
        .lsp()
        .unwrap()
        .client_for_document(document)
        .unwrap();
    let diagnostics = session
        .lsp()
        .unwrap()
        .diagnostics_for_document(document)
        .unwrap();
    wait_until(|| {
        let report = session.tick();
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        client.borrow().is_initialized() && client.borrow().is_document_open(&canonical)
    });
    let mut left = EditorView::new(&mut session, document).unwrap();
    let mut right = EditorView::new(&mut session, document).unwrap();
    let mut context = Context::create();
    context.set_ini_filename(None::<PathBuf>).unwrap();
    context
        .font_atlas()
        .try_claim_legacy_renderer()
        .unwrap()
        .build();
    let draw = |context: &mut Context,
                session: &mut EditorSession,
                left: &mut EditorView,
                right: &mut EditorView| {
        context.prepare_frame(FramePrepareOptions::new([1000.0, 600.0], 1.0 / 60.0));
        let ui = context.frame();
        let diagnostic_mark = ui.with_bound_context(|| unsafe {
            // Diagnostic ink adapts to the host palette; retain the geometry
            // assertions without depending on the unadjusted source color.
            sys::igColorConvertFloat4ToU32(
                bed_ui::presentation::readable_color(ui, severity_color(1)).into(),
            )
        });
        ui.window("Session diagnostic canvas")
            .position([0.0, 0.0], Condition::Always)
            .size([980.0, 550.0], Condition::Always)
            .build(|| {
                for view in [left, right] {
                    view.draw(
                        ui,
                        session,
                        &EditorViewOptions {
                            size: [450.0, 480.0],
                            rainbow_mode: false,
                            ..EditorViewOptions::default()
                        },
                    )
                    .unwrap();
                    ui.same_line();
                }
            });
        let geometry = ui.with_bound_context(|| unsafe {
            let native = &*sys::igGetCurrentContext();
            let mut text_counts = Vec::new();
            let mut gutter_counts = Vec::new();
            for index in 0..native.Windows.Size {
                let window = &**native.Windows.Data.add(index as usize);
                let name = std::ffi::CStr::from_ptr(window.Name).to_string_lossy();
                if !name.contains("Session diagnostic canvas") {
                    continue;
                }
                let draw = &*window.DrawList;
                let marks = (0..draw.VtxBuffer.Size)
                    .filter(|index| {
                        (*draw.VtxBuffer.Data.add(*index as usize)).col == diagnostic_mark
                    })
                    .count();
                if name.contains("LineNumbers") {
                    gutter_counts.push(marks);
                } else if name.contains("##editor") {
                    text_counts.push(marks);
                }
            }
            (text_counts, gutter_counts)
        });
        drop(context.render_legacy());
        geometry
    };
    assert_eq!(
        draw(&mut context, &mut session, &mut left, &mut right),
        (vec![0, 0], vec![0, 0])
    );
    session
        .with_commands(left.id(), |commands| {
            commands.set_cursor(0, 5, false, CursorReveal::Ensure);
            commands.type_text(b"INVALID");
        })
        .unwrap();
    session.save(document).unwrap();
    let malformed = session.snapshot(document).unwrap();
    assert_eq!(malformed.bytes, "a🙂INVALIDb\n".as_bytes());
    // The mock publishes on didSave. Drawing the view never polls services or
    // publishes background results into the main-thread diagnostic store.
    thread::sleep(Duration::from_millis(20));
    assert!(diagnostics.for_document(&canonical).is_empty());
    assert_eq!(
        draw(&mut context, &mut session, &mut left, &mut right),
        (vec![0, 0], vec![0, 0])
    );
    wait_until(|| {
        let report = session.tick();
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        !diagnostics.for_document(&canonical).is_empty()
    });
    let errors = diagnostics.for_document(&canonical);
    assert_eq!(errors.len(), 1);
    assert_eq!(
        (errors[0].start_character, errors[0].end_character),
        (3, 10)
    );
    assert_eq!(errors[0].message, "Session save diagnostic");
    let (text, gutter) = draw(&mut context, &mut session, &mut left, &mut right);
    assert_eq!(text.len(), 2);
    assert!(
        text.iter().all(|count| *count > 4),
        "each view paints the UTF-16 squiggle: {text:?}"
    );
    assert_eq!(gutter, vec![4, 4], "each view paints one error marker");
    let transcript = shared_request(&client, "test/transcript");
    let transcript = transcript.as_array().unwrap();
    assert_eq!(
        transcript
            .iter()
            .filter(|item| item["method"] == "textDocument/didOpen")
            .count(),
        1
    );
    let changes: Vec<_> = transcript
        .iter()
        .filter(|item| item["method"] == "textDocument/didChange")
        .collect();
    assert_eq!(changes.len(), 1);
    assert_eq!(
        changes[0]["params"]["contentChanges"][0]["range"]["start"]["character"],
        3
    );
    assert_eq!(
        changes[0]["params"]["textDocument"]["version"],
        malformed.version
    );
    let saves: Vec<_> = transcript
        .iter()
        .filter(|item| item["method"] == "textDocument/didSave")
        .collect();
    assert_eq!(saves.len(), 1);
    assert_eq!(saves[0]["params"]["text"], "a🙂INVALIDb\n");
    session
        .with_commands(right.id(), |commands| commands.undo())
        .unwrap();
    session.save(document).unwrap();
    wait_until(|| {
        session.tick();
        diagnostics.for_document(&canonical).is_empty()
    });
    assert_eq!(
        session.snapshot(document).unwrap().bytes,
        "a🙂b\n".as_bytes()
    );
    assert_eq!(
        draw(&mut context, &mut session, &mut left, &mut right),
        (vec![0, 0], vec![0, 0])
    );
    session.shutdown(ClosePolicy::Discard).unwrap();
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

struct PoolFixture {
    root: PathBuf,
    pool: WorkspaceLsp,
}
impl PoolFixture {
    fn new(id: u64, scenario: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "bed-lsp-pool-{}-{}",
            std::process::id(),
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let config = root.join("lsp.json");
        fs::write(&config, serde_json::to_vec(&json!({"languages":[
            {"language_name":"cpp","language_file_extensions":[".cpp"],"language_server_paths":[std::env::current_exe().unwrap()]},
            {"language_name":"rust","language_file_extensions":[".rs"],"language_server_paths":[std::env::current_exe().unwrap()]}
        ]})).unwrap()).unwrap();
        let mut pool = WorkspaceLsp::new(WorkspaceId(id), config, root.clone());
        for language in ["cpp", "rust"] {
            pool.set_server_arguments(language, vec!["--mock-server".into(), scenario.into()]);
        }
        Self { root, pool }
    }
    fn path(&self, name: &str) -> String {
        self.root.join(name).to_string_lossy().into_owned()
    }
    fn ready(&mut self) {
        wait_until(|| {
            self.pool.poll();
            self.pool
                .server_statuses()
                .iter()
                .filter(|status| status.process_id.is_some())
                .all(|status| status.initialized)
        });
    }
    fn transcript(&self, language: &str) -> Vec<Value> {
        shared_request(
            &self.pool.client_for_language(language).unwrap(),
            "test/transcript",
        )
        .as_array()
        .unwrap()
        .clone()
    }
}
impl Drop for PoolFixture {
    fn drop(&mut self) {
        self.pool.shutdown();
        let _ = fs::remove_dir_all(&self.root);
    }
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
        first.pool.workspace_id(),
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
        workspace_id: WorkspaceId(51),
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
    fs::write(&missing,serde_json::to_vec(&json!({"languages":[{"language_name":"rust","language_file_extensions":[".rs"],"language_server_paths":["/bed-guaranteed-no-rust-server"]}]})).unwrap()).unwrap();
    let mut pool = WorkspaceLsp::new(WorkspaceId(61), missing, fixture.root.clone());
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

struct NavigationWorkbench {
    fixture: PoolFixture,
    context: dear_imgui_rs::Context,
    workbench: bed::workbench::Workbench,
    client: Rc<RefCell<LspClient>>,
    document: DocumentId,
    requesting: ViewId,
    sibling: ViewId,
    request_tab: usize,
    sibling_tab: usize,
    path: String,
}
impl NavigationWorkbench {
    fn new() -> Self {
        use bed::{
            util::settings::Settings,
            workbench::{WindowCommand, Workbench, WorkbenchHostMode},
        };
        use bed_core::editor_commands::CursorReveal;
        let fixture = PoolFixture::new(9910, "basic");
        let path = fixture.root.join("navigation.rs");
        fs::write(&path, "é🙂symbol\nline target\nlast\n").unwrap();
        let mut settings = Settings::with_paths(
            fixture.root.join("config"),
            PathBuf::from(env!("CARGO_MANIFEST_DIR")),
        )
        .unwrap();
        fs::copy(
            fixture.root.join("lsp.json"),
            settings.config_dir.join("lsp.json"),
        )
        .unwrap();
        for key in [
            "terminal_visible",
            "sidebar_visible",
            "treesitter",
            "git_changed_lines",
            "minimap",
        ] {
            settings.settings[key] = json!(false);
        }
        settings.sidebar_visible = false;
        settings.terminal_visible = false;
        let mut context = dear_imgui_rs::Context::create();
        context.set_ini_filename(None::<PathBuf>).unwrap();
        let mut workbench = Workbench::with_settings(settings);
        workbench
            .initialize(&mut context, WorkbenchHostMode::Fullscreen)
            .unwrap();
        workbench.set_project(&fixture.root).unwrap();
        workbench
            .session
            .lsp_mut()
            .unwrap()
            .set_server_arguments("rust", vec!["--mock-server".into(), "basic".into()]);
        workbench.open_or_focus(&path).unwrap();
        let document = workbench.active_document().unwrap();
        let requesting = workbench.active_view().unwrap();
        let request_tab = workbench.active_index();
        workbench.dispatch(WindowCommand::DuplicateView).unwrap();
        let sibling = workbench.active_view().unwrap();
        let sibling_tab = workbench.active_index();
        workbench
            .session
            .with_commands(requesting, |commands| {
                commands.set_cursor(0, 2, false, CursorReveal::Ensure);
            })
            .unwrap();
        workbench
            .session
            .with_commands(sibling, |commands| {
                commands.set_cursor(2, 1, false, CursorReveal::Ensure);
            })
            .unwrap();
        let path = workbench.session.snapshot(document).unwrap().path;
        let client = workbench
            .session
            .lsp()
            .unwrap()
            .client_for_document(document)
            .unwrap();
        wait_until(|| {
            workbench.tick().unwrap();
            client.borrow().is_initialized() && client.borrow().is_document_open(&path)
        });
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        let mut result = Self {
            fixture,
            context,
            workbench,
            client,
            document,
            requesting,
            sibling,
            request_tab,
            sibling_tab,
            path,
        };
        result.focus(request_tab);
        result
    }
    fn frame(&mut self) {
        self.workbench.tick().unwrap();
        self.context
            .prepare_frame(dear_imgui_rs::FramePrepareOptions::new(
                [1600.0, 1000.0],
                1.0 / 60.0,
            ));
        let actions = self.workbench.render(self.context.frame()).unwrap();
        assert!(self.context.render_legacy().draw_data().total_vtx_count() > 0);
        for action in actions {
            self.workbench.handle_action(action).unwrap();
        }
    }
    fn focus(&mut self, tab: usize) {
        assert!(self.workbench.switch_to_tab(tab));
        self.frame();
        self.frame();
    }
    fn configure(&self, result: Value) {
        shared_request_params(
            &self.client,
            "test/definition/configure",
            json!({"result":result,"hold":true}),
        );
    }
    fn location(&self, row: i32, column: i32) -> Value {
        json!({"uri":LspUri::file_uri_from_path(&self.path).unwrap().to_string(),"range":{"start":{"line":row,"character":column},"end":{"line":row,"character":column+1}}})
    }
    fn shortcut(&mut self) {
        use dear_imgui_rs::Key;
        self.focus(self.request_tab);
        let modifier = if cfg!(target_os = "macos") {
            Key::ModSuper
        } else {
            Key::ModCtrl
        };
        self.context.io_mut().add_key_event(modifier, true);
        self.context.io_mut().add_key_event(Key::D, true);
        self.frame();
        self.context.io_mut().add_key_event(Key::D, false);
        self.context.io_mut().add_key_event(modifier, false);
        self.frame();
        assert_eq!(self.workbench.panel_count("references"), 0);
        assert!(
            shared_request(&self.client, "test/transcript")
                .as_array()
                .unwrap()
                .iter()
                .any(|message| message["method"] == "textDocument/definition"),
            "native Cmd/Ctrl+D must send the request"
        );
    }
    fn release(&self) {
        assert_eq!(shared_request(&self.client, "test/definition/release"), 1);
    }
}
impl Drop for NavigationWorkbench {
    fn drop(&mut self) {
        self.workbench.cleanup().unwrap();
        // Keep the directory owner alive until all workspace services stopped.
        let _ = &self.fixture;
    }
}

fn workbench_single_definition_returns_to_requesting_view_without_references() {
    let mut fixture = NavigationWorkbench::new();
    let bytes = fixture
        .workbench
        .session
        .snapshot(fixture.document)
        .unwrap()
        .bytes;
    fixture.configure(fixture.location(1, 2));
    fixture.shortcut();
    let other = fixture
        .workbench
        .session
        .view_snapshot(fixture.sibling)
        .unwrap()
        .selections;
    fixture.focus(fixture.sibling_tab);
    fixture.release();
    assert_eq!(fixture.workbench.panel_count("references"), 0);
    fixture.frame();
    assert_eq!(fixture.workbench.active_view(), Some(fixture.requesting));
    let view = fixture
        .workbench
        .session
        .view_snapshot(fixture.requesting)
        .unwrap();
    assert_eq!((view.row, view.column), (1, 2));
    assert_eq!(
        fixture
            .workbench
            .session
            .view_snapshot(fixture.sibling)
            .unwrap()
            .selections,
        other
    );
    assert_eq!(fixture.workbench.panel_count("references"), 0);
    assert_eq!(
        fixture
            .workbench
            .session
            .snapshot(fixture.document)
            .unwrap()
            .bytes,
        bytes
    );
}

fn workbench_multiple_and_stale_definitions_use_guarded_routes() {
    let mut fixture = NavigationWorkbench::new();
    fixture.configure(json!([fixture.location(1, 2), fixture.location(2, 0)]));
    fixture.shortcut();
    fixture.release();
    fixture.frame();
    assert_eq!(fixture.workbench.panel_count("references"), 1);
    assert_eq!(
        fixture
            .workbench
            .session
            .view_snapshot(fixture.requesting)
            .unwrap()
            .row,
        0
    );
    assert!(
        fixture
            .workbench
            .close_tab(fixture.workbench.active_index())
            .unwrap()
    );
    assert_eq!(fixture.workbench.panel_count("references"), 0);
    fixture.configure(fixture.location(1, 2));
    fixture.shortcut();
    fixture
        .workbench
        .session
        .with_commands(fixture.requesting, |commands| commands.type_text(b"x"))
        .unwrap();
    let expected = fixture
        .workbench
        .session
        .view_snapshot(fixture.requesting)
        .unwrap()
        .selections;
    fixture.focus(fixture.sibling_tab);
    fixture.release();
    fixture.frame();
    assert_eq!(fixture.workbench.active_view(), Some(fixture.sibling));
    assert_eq!(
        fixture
            .workbench
            .session
            .view_snapshot(fixture.requesting)
            .unwrap()
            .selections,
        expected
    );
    assert_eq!(fixture.workbench.panel_count("references"), 0);
    drop(fixture);
    let mut fixture = NavigationWorkbench::new();
    fixture.configure(fixture.location(1, 2));
    fixture.shortcut();
    fixture.focus(fixture.sibling_tab);
    assert!(fixture.workbench.close_tab(fixture.request_tab).unwrap());
    assert!(
        fixture
            .workbench
            .session
            .document_for_view(fixture.requesting)
            .is_none()
    );
    let expected = fixture
        .workbench
        .session
        .view_snapshot(fixture.sibling)
        .unwrap()
        .selections;
    fixture.release();
    fixture.frame();
    assert_eq!(fixture.workbench.active_view(), Some(fixture.sibling));
    assert_eq!(
        fixture
            .workbench
            .session
            .view_snapshot(fixture.sibling)
            .unwrap()
            .selections,
        expected
    );
    assert_eq!(fixture.workbench.panel_count("references"), 0);
}

fn workbench_primary_click_sends_clicked_utf16_position() {
    use bed_ui::views::view_layout::{glyph_advance, line_column_x};
    use dear_imgui_rs::{Key, MouseButton, sys};
    let mut fixture = NavigationWorkbench::new();
    fixture.configure(fixture.location(1, 2));
    fixture
        .context
        .prepare_frame(dear_imgui_rs::FramePrepareOptions::new(
            [1600.0, 1000.0],
            1.0 / 60.0,
        ));
    let ui = fixture.context.frame();
    fixture.workbench.render(ui).unwrap();
    let name = std::ffi::CString::new(format!(
        "###bed_tab_{}",
        fixture
            .workbench
            .tab_window_id(fixture.request_tab)
            .unwrap()
    ))
    .unwrap();
    let cursor_start = ui.with_bound_context(|| unsafe {
        let parent = sys::igFindWindowByName(name.as_ptr());
        assert!(!parent.is_null());
        let native = &*sys::igGetCurrentContext();
        (0..native.Windows.Size)
            .find_map(|index| {
                let window = &**native.Windows.Data.add(index as usize);
                let mut ancestor = window.ParentWindow;
                while !ancestor.is_null() && ancestor != parent {
                    ancestor = (*ancestor).ParentWindow;
                }
                (ancestor == parent
                    && std::ffi::CStr::from_ptr(window.Name)
                        .to_string_lossy()
                        .contains("##editor"))
                .then_some([window.DC.CursorStartPos.x, window.DC.CursorStartPos.y])
            })
            .expect("requesting view has an actual native canvas")
    });
    let font_size = fixture.workbench.settings.font_size();
    let font = ui.push_font_with_size(fixture.workbench.settings.font.main, font_size);
    let point = [
        line_column_x(
            ui,
            "é🙂symbol".as_bytes(),
            6,
            cursor_start[0] + font_size * 0.35,
        ) + glyph_advance(ui, "s") * 0.2,
        cursor_start[1] + font_size * 0.1 + ui.text_line_height() * 0.35,
    ];
    drop(font);
    drop(fixture.context.render_legacy());
    let before = fixture
        .workbench
        .session
        .view_snapshot(fixture.requesting)
        .unwrap()
        .selections;
    let other = fixture
        .workbench
        .session
        .view_snapshot(fixture.sibling)
        .unwrap()
        .selections;
    let modifier = if cfg!(target_os = "macos") {
        Key::ModSuper
    } else {
        Key::ModCtrl
    };
    fixture.context.io_mut().add_key_event(modifier, true);
    fixture.context.io_mut().add_mouse_pos_event(point);
    fixture
        .context
        .io_mut()
        .add_mouse_button_event(MouseButton::Left, true);
    fixture.frame();
    fixture
        .context
        .io_mut()
        .add_mouse_button_event(MouseButton::Left, false);
    fixture.context.io_mut().add_key_event(modifier, false);
    fixture.frame();
    assert_eq!(
        fixture
            .workbench
            .session
            .view_snapshot(fixture.requesting)
            .unwrap()
            .selections,
        before
    );
    assert_eq!(
        fixture
            .workbench
            .session
            .view_snapshot(fixture.sibling)
            .unwrap()
            .selections,
        other
    );
    assert_eq!(fixture.workbench.panel_count("references"), 0);
    let transcript = shared_request(&fixture.client, "test/transcript");
    let request = transcript
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["method"] == "textDocument/definition")
        .expect("native primary-click reaches LSP");
    assert_eq!(
        request["params"]["position"],
        json!({"line":0,"character":3})
    );
    fixture.release();
    fixture.frame();
    assert_eq!(
        fixture
            .workbench
            .session
            .view_snapshot(fixture.requesting)
            .unwrap()
            .row,
        1
    );
    assert_eq!(fixture.workbench.panel_count("references"), 0);
}

fn typed_navigation_results_follow_the_requesting_view() {
    let mut fixture = PoolFixture::new(70, "basic");
    let path = fixture.path("navigation.rs");
    fixture
        .pool
        .register_document(DocumentId(1), &path, b"symbol", 1, "rust")
        .unwrap();
    fixture.ready();
    let mut editor = Editor::new();
    editor.state.set_from_bytes(b"symbol");
    editor.state.path = path;
    let client = fixture.pool.client_for_document(DocumentId(1)).unwrap();
    let mut ui = LspUi::default();
    let origin = fixture
        .pool
        .request_origin(DocumentId(1), ViewId(2), 3, 0)
        .unwrap();
    assert!(ui.request_at(
        ContextLspAction::Definition,
        &mut client.borrow_mut(),
        &editor,
        origin
    ));
    assert!(
        ui.navigation_results(ContextLspAction::Definition)
            .unwrap()
            .pending
    );
    assert!(!ui.is_overlay_visible());
    wait_until(|| {
        fixture.pool.poll();
        !ui.navigation_results(ContextLspAction::Definition)
            .unwrap()
            .pending
    });
    let definition = ui.navigation_results(ContextLspAction::Definition).unwrap();
    assert_eq!(definition.locations.len(), 1);
    assert_eq!(definition.origin.unwrap().view_id, ViewId(2));
    let other = LspRequestOrigin {
        view_id: ViewId(4),
        ..origin
    };
    assert!(ui.request_at(
        ContextLspAction::References,
        &mut client.borrow_mut(),
        &editor,
        other
    ));
    wait_until(|| {
        fixture.pool.poll();
        !ui.navigation_results(ContextLspAction::References)
            .unwrap()
            .pending
    });
    let references = ui.navigation_results(ContextLspAction::References).unwrap();
    assert_eq!(references.locations.len(), 2);
    assert_eq!(references.origin.unwrap().view_id, ViewId(4));
    ui.retain_requests(|origin| origin.view_id == ViewId(4));
    assert!(
        ui.navigation_results(ContextLspAction::Definition)
            .unwrap()
            .origin
            .is_none()
    );
    assert_eq!(
        ui.navigation_results(ContextLspAction::References)
            .unwrap()
            .locations
            .len(),
        2
    );
    ui.cancel_requests();
    assert!(
        ui.navigation_results(ContextLspAction::References)
            .unwrap()
            .origin
            .is_none()
    );
}

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
    // The pinned Python config supplies --stdio. Our self-spawning executable
    // exposes that native mode, so this also exercises the preserved format.
    fs::write(fixture.pool.config_path(),serde_json::to_vec(&json!({"languages":[{"language_name":"python","language_file_extensions":[".rs"],"language_server_paths":[std::env::current_exe().unwrap()]}]})).unwrap()).unwrap();
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
    assert_eq!(fixture.pool.config().language_servers[0].language, "python");
    assert_eq!(current.borrow().process_id(), pid);
    assert!(current.borrow().is_initialized());
}

fn mock_server(scenario: &str) {
    if scenario == "blocked" {
        thread::sleep(Duration::from_secs(5));
        return;
    }
    if scenario == "descendant" {
        // The parent must exit while this short-lived descendant retains its
        // inherited pipes; waiting here would remove the shutdown regression.
        #[allow(clippy::zombie_processes)]
        let _child = Command::new(std::env::current_exe().unwrap())
            .arg("--hold-pipes")
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        return;
    }
    if scenario == "stderr" {
        io::stderr().write_all(&vec![b'x'; 256 * 1024]).unwrap();
    }
    let input = io::stdin();
    let output = io::stdout();
    let mut connection = Connection::new(BufReader::new(input.lock()));
    let mut output = output.lock();
    let mut transcript = Vec::new();
    let mut reversed: Vec<Request> = Vec::new();
    let mut handler_request = None;
    let mut last_changed_version = Value::Null;
    let mut definition_result = None;
    let mut hold_definitions = false;
    let mut held_definitions = Vec::new();
    while let Ok(packet) = connection.read_packet() {
        let messages = match packet {
            Packet::Single(message) => vec![message],
            Packet::Batch(messages) => messages,
        };
        let mut handler_responses = Vec::new();
        for message in messages {
            let Message::Request(request) = message else {
                if let Message::Response(response) = message {
                    let value = Message::Response(response).value();
                    transcript.push(value.clone());
                    handler_responses.push(value);
                }
                continue;
            };
            transcript.push(Message::Request(request.clone()).value());
            if scenario == "session-diagnostics" {
                if request.method == "textDocument/didChange" {
                    last_changed_version =
                        request.params.as_ref().unwrap()["textDocument"]["version"].clone();
                } else if request.method == "textDocument/didSave" {
                    let params = request.params.as_ref().unwrap();
                    let items = if params["text"].as_str().unwrap().contains("INVALID") {
                        json!([{"range":{"start":{"line":0,"character":3},"end":{"line":0,"character":10}},"severity":1,"source":"bed-mock","message":"Session save diagnostic"}])
                    } else {
                        json!([])
                    };
                    write_packet(&mut output, &Packet::Single(Message::Request(Request {
                        id: None,
                        method: "textDocument/publishDiagnostics".into(),
                        params: Some(json!({"uri":params["textDocument"]["uri"],"version":last_changed_version,"diagnostics":items})),
                    }))).unwrap();
                }
            }
            if scenario == "native" && request.method == "textDocument/didOpen" {
                let document = &request.params.as_ref().unwrap()["textDocument"];
                let params = json!({"uri":document["uri"],"version":document["version"],"diagnostics":[{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":4}},"severity":1,"source":"bed-mock","message":{"kind":"markdown","value":"**Mock diagnostic**\n\n```rust\nlet value = 3;\n```"}}]});
                write_packet(
                    &mut output,
                    &Packet::Single(Message::Request(Request {
                        id: None,
                        method: "textDocument/publishDiagnostics".into(),
                        params: Some(params),
                    })),
                )
                .unwrap();
            }
            if request.method == "exit" {
                return;
            }
            if scenario == "silent" {
                continue;
            }
            let Some(id) = request.id.clone() else {
                continue;
            };
            let mut result = match request.method.as_str() {
                "initialize" => {
                    if scenario == "delayed" {
                        thread::sleep(Duration::from_millis(50));
                    }
                    if scenario == "initialize-error" {
                        Err(ResponseError::new(-32002, "not ready"))
                    } else {
                        let sync = match scenario {
                            "full" => json!({"change":1,"save":false}),
                            "none" => json!({"change":0,"save":false}),
                            "save-no-text" => json!({"change":2,"save":{"includeText":false}}),
                            _ => json!({"change":2,"save":{"includeText":true}}),
                        };
                        Ok(json!({"capabilities":{"textDocumentSync":sync}}))
                    }
                }
                "shutdown" => Ok(Value::Null),
                "test/transcript" => Ok(Value::Array(transcript.clone())),
                "test/definition/configure" => {
                    let params = request.params.as_ref().unwrap();
                    definition_result = Some(params["result"].clone());
                    hold_definitions = params["hold"].as_bool().unwrap();
                    Ok(Value::Null)
                }
                "test/definition/release" => {
                    let count = held_definitions.len();
                    for (id, result) in held_definitions.drain(..) {
                        write_packet(
                            &mut output,
                            &Packet::Single(Message::Response(Response {
                                id,
                                result: Ok(result),
                            })),
                        )
                        .unwrap();
                    }
                    Ok(json!(count))
                }
                "test/progress" => {
                    for params in [
                        json!({"token":"index","value":{"kind":"begin","title":"Indexing","message":"First crate","percentage":0}}),
                        json!({"token":17,"value":{"kind":"begin","title":"Loading dependencies"}}),
                        json!({"token":"index","value":{"kind":"report","message":"Second crate","percentage":50}}),
                        json!({"token":null,"value":[{"uri":"file:///partial.rs"}]}),
                        json!({"token":true,"value":{"kind":"custom","percentage":999}}),
                    ] {
                        write_packet(
                            &mut output,
                            &Packet::Single(Message::Request(Request {
                                id: None,
                                method: "$/progress".into(),
                                params: Some(params),
                            })),
                        )
                        .unwrap();
                    }
                    Ok(Value::Null)
                }
                "test/progress/end" => {
                    for token in [json!("index"), json!(17)] {
                        write_packet(&mut output, &Packet::Single(Message::Request(Request { id:None,method:"$/progress".into(),params:Some(json!({"token":token,"value":{"kind":"end","message":"Indexed"}})) }))).unwrap();
                    }
                    Ok(Value::Null)
                }
                "test/echo" => Ok(json!(if scenario == "fragmented" {
                    "fragment😀🚀"
                } else {
                    "echo"
                })),
                "test/argv" => Ok(json!(std::env::args().collect::<Vec<_>>())),
                "test/environment" => Ok(
                    json!({"cwd":std::env::current_dir().unwrap(),"marker":std::env::var("BED_LSP_PROCESS_TEST_MARKER").ok(),"path":std::env::var_os("PATH").map(|path| path.to_string_lossy().into_owned())}),
                ),
                "test/crash" => {
                    std::process::exit(9);
                }
                "textDocument/hover" => Ok(
                    json!({"contents":{"kind":"markdown","value":"# Mock symbol\n\nA **deterministic** hover with `code`.\n\n```rust\nlet value = 3;\n```"},"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":4}}}),
                ),
                "textDocument/definition" => {
                    let result = definition_result.clone().unwrap_or_else(||
                        json!({"uri":request.params.as_ref().unwrap()["textDocument"]["uri"],"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":4}}}));
                    if hold_definitions {
                        held_definitions.push((id, result));
                        continue;
                    }
                    Ok(result)
                }
                "textDocument/references" => Ok(json!([
                    {"uri":request.params.as_ref().unwrap()["textDocument"]["uri"],"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":4}}},
                    {"uri":request.params.as_ref().unwrap()["textDocument"]["uri"],"range":{"start":{"line":1,"character":0},"end":{"line":1,"character":4}}}
                ])),
                "test/handlers" => {
                    let requests = [
                        (
                            "config",
                            "workspace/configuration",
                            Some(json!({"items":[{"section":"a"},{}]})),
                        ),
                        ("folders", "workspace/workspaceFolders", None),
                        (
                            "register",
                            "client/registerCapability",
                            Some(json!({"registrations":[{"id":"one","method":"example"}]})),
                        ),
                        (
                            "progress",
                            "window/workDoneProgress/create",
                            Some(json!({"token":"progress"})),
                        ),
                        (
                            "show",
                            "window/showMessageRequest",
                            Some(json!({"type":3,"message":"hello","actions":[{"title":"ok"}]})),
                        ),
                        ("unknown", "unknown/method", None),
                    ];
                    let mut messages = requests
                        .into_iter()
                        .map(|(id, method, params)| {
                            Message::Request(Request {
                                id: Some(RpcId::String(id.into())),
                                method: method.into(),
                                params,
                            })
                        })
                        .collect::<Vec<_>>();
                    messages.push(Message::Request(Request {
                        id: None,
                        method: "unknown/notification".into(),
                        params: None,
                    }));
                    write_packet(&mut output, &Packet::Batch(messages)).unwrap();
                    handler_request = Some(id);
                    continue;
                }
                "test/reverse" => {
                    reversed.push(request);
                    if reversed.len() == 2 {
                        let responses = reversed
                            .drain(..)
                            .rev()
                            .map(|request| {
                                Message::Response(Response {
                                    id: request.id.unwrap(),
                                    result: Ok(request.params.unwrap()["number"].clone()),
                                })
                            })
                            .collect();
                        write_packet(&mut output, &Packet::Batch(responses)).unwrap();
                    }
                    continue;
                }
                "test/diagnostics" => {
                    let uri = request.params.as_ref().unwrap()["uri"].clone();
                    for (version, text) in [(2, "**new**"), (1, "stale")] {
                        let params = json!({"uri":uri,"version":version,"diagnostics":[{"range":{"start":{"line":0,"character":2},"end":{"line":0,"character":4}},"message":{"kind":"markdown","value":text}}]});
                        write_packet(
                            &mut output,
                            &Packet::Single(Message::Request(Request {
                                id: None,
                                method: "textDocument/publishDiagnostics".into(),
                                params: Some(params),
                            })),
                        )
                        .unwrap();
                    }
                    Ok(Value::Null)
                }
                "test/malformed" => {
                    let count = if scenario == "malformed-limit" { 16 } else { 1 };
                    for _ in 0..count {
                        raw_frame(&mut output, b"not JSON", None);
                    }
                    if scenario == "malformed-limit" {
                        continue;
                    }
                    raw_frame(
                        &mut output,
                        br#"{"jsonrpc":"2.0","method":"x","method":"y"}"#,
                        None,
                    );
                    raw_frame(&mut output, b"{}", None);
                    Ok(json!("recovered"))
                }
                "test/fatal" => {
                    match scenario {
                        "negative" => output.write_all(b"Content-Length: -1\r\n\r\n").unwrap(),
                        "overflow" => output
                            .write_all(b"Content-Length: 999999999999999999999999999\r\n\r\n")
                            .unwrap(),
                        "bare-lf" => output.write_all(b"Content-Length: 2\n\n{}").unwrap(),
                        "oversized" => output
                            .write_all(b"Content-Length: 16777217\r\n\r\n")
                            .unwrap(),
                        "truncated" => output.write_all(b"Content-Length: 5\r\n\r\n{").unwrap(),
                        "eof" => {}
                        "unsupported-type" => raw_frame(&mut output, b"{}", Some("text/plain")),
                        "unsupported-charset" => raw_frame(
                            &mut output,
                            b"{}",
                            Some("application/vscode-jsonrpc; charset=utf-16"),
                        ),
                        _ => panic!("unknown fatal scenario"),
                    }
                    output.flush().unwrap();
                    return;
                }
                _ => Err(ResponseError::new(METHOD_NOT_FOUND, "unknown mock method")),
            };
            let packet = Packet::Single(Message::Response(Response {
                id,
                result: std::mem::replace(&mut result, Ok(Value::Null)),
            }));
            if scenario == "initialize-batch" && request.method == "initialize" {
                let Packet::Single(response) = packet else {
                    unreachable!();
                };
                write_packet(
                    &mut output,
                    &Packet::Batch(vec![
                        response,
                        Message::Request(Request {
                            id: Some(RpcId::String("handshake-folders".into())),
                            method: "workspace/workspaceFolders".into(),
                            params: None,
                        }),
                    ]),
                )
                .unwrap();
            } else if scenario == "fragmented" {
                for byte in encode_packet(&packet).unwrap() {
                    output.write_all(&[byte]).unwrap();
                    output.flush().unwrap();
                }
            } else {
                write_packet(&mut output, &packet).unwrap();
            }
        }
        if !handler_responses.is_empty()
            && let Some(id) = handler_request.take()
        {
            write_packet(
                &mut output,
                &Packet::Single(Message::Response(Response {
                    id,
                    result: Ok(Value::Array(handler_responses)),
                })),
            )
            .unwrap();
        }
    }
}

fn raw_frame(output: &mut impl Write, body: &[u8], content_type: Option<&str>) {
    write!(output, "Content-Length: {}\r\n", body.len()).unwrap();
    if let Some(content_type) = content_type {
        write!(output, "Content-Type: {content_type}\r\n").unwrap();
    }
    output.write_all(b"\r\n").unwrap();
    output.write_all(body).unwrap();
    output.flush().unwrap();
}
