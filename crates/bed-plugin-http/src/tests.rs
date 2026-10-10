use super::*;
use bed_workbench_api::Registry;
use model::{Auth, KeyValue};
use serde_json::json;
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

fn tick(module: &mut HttpModule) {
    module.tick(
        &HostContext {
            remote: false,
            documents: &[],
            active_document: None,
            settings: &Value::Null,
            textures: &HashMap::new(),
            animations: false,
            workspace: 1,
            diagnostics: &Value::Null,
            viewer_menu: None,
            default_viewers: &Value::Null,
        },
        &mut Vec::new(),
    );
}

fn actions(module: &mut HttpModule, actions: impl IntoIterator<Item = Action>) {
    module.shared.borrow_mut().actions.extend(actions);
    tick(module);
}

#[test]
fn workspace_panel_crud_persists_drafts_without_sending_requests() {
    let mut module = HttpModule::default();
    let mut registry = Registry::default();
    registry.register(&module).unwrap();
    let panel_type = registry.panel(PANEL_ID).unwrap();
    assert!(panel_type.singleton);
    assert_eq!(panel_type.placement, PanelPlacement::Center);
    assert!(registry.viewer_for_path("api.http").is_none());
    let panel = module.create_panel(PANEL_ID, None, &Value::Null).unwrap();
    assert_eq!(panel.attached_document(), None);
    actions(&mut module, [Action::Create, Action::Create]);
    let mut first = module.shared.borrow().requests[0].clone();
    first.name = "Create user".into();
    first.method = "POST".into();
    first.url = "https://example.test/users".into();
    first.params.push(KeyValue {
        enabled: false,
        name: "draft".into(),
        value: "true".into(),
    });
    first.headers.push(KeyValue {
        enabled: true,
        name: "Content-Type".into(),
        value: "application/json".into(),
    });
    first.body = "{\"name\":\"Ada\"}".into();
    first.auth = Auth::Bearer {
        token: "saved-token".into(),
    };
    actions(
        &mut module,
        [
            Action::Update {
                request: first.clone(),
            },
            Action::Select { id: first.id },
            Action::Delete { id: 2 },
        ],
    );
    drop(panel);
    let saved = module.save_workspace();
    assert_eq!(saved["requests"].as_array().unwrap().len(), 1);
    assert_eq!(saved["selected"], first.id);
    assert!(!saved.to_string().contains("results"));
    assert_eq!(module.next_run, 0);
    let mut restored = HttpModule::default();
    restored.restore_workspace(Some(&saved), "/workspace");
    assert_eq!(restored.shared.borrow().requests, [first]);
    assert_eq!(restored.save_workspace(), saved);
    assert!(restored.shared.borrow().active.is_none());
    assert!(restored.shared.borrow().results.is_empty());
    actions(&mut restored, [Action::Create]);
    assert_eq!(restored.shared.borrow().requests[1].id, 2);
    actions(
        &mut restored,
        [Action::Delete { id: 1 }, Action::Delete { id: 2 }],
    );
    assert_eq!(restored.save_workspace()["requests"], json!([]));
    assert_eq!(restored.shared.borrow().selected, None);
    assert_eq!(restored.next_run, 0);
}

#[test]
fn immediate_save_includes_pending_ui_edits_without_executing_them() {
    let mut module = HttpModule::default();
    actions(&mut module, [Action::Create, Action::Create]);
    let mut edited = module.shared.borrow().requests[0].clone();
    edited.name = "Last keystroke before closing".into();
    edited.body = "unsent body".into();
    module.shared.borrow_mut().actions.extend([
        Action::Update {
            request: edited.clone(),
        },
        Action::Run { id: 1 },
        Action::Delete { id: 2 },
        Action::Create,
        Action::Select { id: 1 },
    ]);
    let saved = module.save_workspace();
    assert_eq!(saved["requests"][0]["name"], edited.name);
    assert_eq!(saved["requests"][0]["body"], edited.body);
    assert_eq!(saved["requests"][1]["id"], 3);
    assert_eq!(saved["selected"], 1);
    assert_eq!(module.save_workspace(), saved);
    assert_eq!(module.next_run, 0);
    assert_eq!(
        module.next_request, 2,
        "saving projects changes without mutating live state"
    );
    assert_ne!(module.shared.borrow().requests[0], edited);
    tick(&mut module);
    assert_eq!(module.save_workspace(), saved);
    assert_eq!(
        module.next_run, 0,
        "an unfinished draft cannot run during the tick either"
    );
}

#[test]
fn invalid_drafts_and_deleted_requests_never_start_execution() {
    let mut module = HttpModule::default();
    actions(&mut module, [Action::Create, Action::Run { id: 1 }]);
    assert_eq!(module.next_run, 0);
    assert!(module.shared.borrow().error.is_some());
    let mut request = module.shared.borrow().requests[0].clone();
    request.url = "file:///etc/passwd".into();
    actions(
        &mut module,
        [Action::Update { request }, Action::Run { id: 1 }],
    );
    assert_eq!(module.next_run, 0);
    actions(
        &mut module,
        [Action::Delete { id: 1 }, Action::Run { id: 1 }],
    );
    assert_eq!(module.next_run, 0);
    assert!(module.shared.borrow().active.is_none());
}

#[test]
fn failed_restoration_preserves_saved_data_and_exposes_the_error() {
    let mut module = HttpModule::default();
    let unsupported = json!({"version":99,"requests":[{"future":"data"}]});
    module.restore_workspace(Some(&unsupported), "/workspace");
    tick(&mut module);
    assert_eq!(module.save_workspace(), unsupported);
    assert!(
        module
            .shared
            .borrow()
            .error
            .as_ref()
            .unwrap()
            .contains("restore")
    );
    assert!(module.shared.borrow().requests.is_empty());
    assert_eq!(module.next_run, 0);
}

#[test]
fn hidden_panel_finishes_captured_request_after_editing_and_switching_selection() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (started, received) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    let server = thread::spawn(move || {
        listener.set_nonblocking(true).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "request did not arrive");
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("accept failed: {error}"),
            }
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut request = Vec::new();
        let mut byte = [0];
        while !request.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).unwrap();
            request.push(byte[0]);
        }
        started.send(String::from_utf8(request).unwrap()).unwrap();
        wait.recv_timeout(Duration::from_secs(5)).unwrap();
        stream.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Type: application/json\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{\"value\":1}").unwrap();
        listener
    });
    let mut module = HttpModule::default();
    let panel = module.create_panel(PANEL_ID, None, &Value::Null).unwrap();
    actions(&mut module, [Action::Create, Action::Create]);
    let mut first = module.shared.borrow().requests[0].clone();
    first.url = format!("http://{address}/first?existing=1");
    first.params.push(KeyValue {
        enabled: true,
        name: "q".into(),
        value: "a b".into(),
    });
    first.auth = Auth::Basic {
        username: "user".into(),
        password: "pass".into(),
    };
    // Run must observe edits queued by the same UI frame, and duplicate clicks share one run.
    actions(
        &mut module,
        [
            Action::Update {
                request: first.clone(),
            },
            Action::Run { id: 1 },
            Action::Run { id: 1 },
        ],
    );
    let received = received.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(received.starts_with("GET /first?existing=1&q=a+b HTTP/1.1"));
    assert!(
        received
            .to_ascii_lowercase()
            .contains("authorization: basic dxnlcjpwyxnz\r\n")
    );
    assert_eq!(module.next_run, 1);
    drop(panel);
    first.url = format!("http://{address}/edited");
    actions(
        &mut module,
        [Action::Update { request: first }, Action::Select { id: 2 }],
    );
    release.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !module.shared.borrow().results.contains_key(&1) {
        assert!(Instant::now() < deadline, "hidden panel did not finish");
        thread::sleep(Duration::from_millis(5));
        tick(&mut module);
    }
    {
        let shared = module.shared.borrow();
        assert_eq!(shared.selected, Some(2));
        assert!(!shared.results.contains_key(&2));
        let result = &shared.results[&1];
        assert!(result.request.url.contains("/first?"));
        assert_ne!(model::resolve(&shared.requests[0]).unwrap(), result.request);
        assert_eq!(result.outcome.as_ref().unwrap().status, 503);
        assert_eq!(result.outcome.as_ref().unwrap().bytes, b"{\"value\":1}");
    }
    let listener = server.join().unwrap();
    assert!(
        matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
    );
    let saved = module.save_workspace();
    module.restore_workspace(Some(&saved), "/workspace");
    assert!(module.shared.borrow().results.is_empty());
    assert!(module.shared.borrow().active.is_none());
    assert_eq!(module.save_workspace(), saved);
}

#[test]
fn deleting_and_restoring_cancel_active_runs_and_ignore_late_completions() {
    let mut module = HttpModule::default();
    actions(&mut module, [Action::Create]);
    let mut request = module.shared.borrow().requests[0].clone();
    // The worker need not connect before cancellation: the same rule owns DNS and I/O.
    request.url = "http://127.0.0.1:9/".into();
    actions(
        &mut module,
        [
            Action::Update { request },
            Action::Run { id: 1 },
            Action::Delete { id: 1 },
        ],
    );
    assert!(module.shared.borrow().active.is_none());
    assert!(module.shared.borrow().results.is_empty());
    actions(&mut module, [Action::Create]);
    let mut request = module.shared.borrow().requests[0].clone();
    request.url = "http://127.0.0.1:9/".into();
    actions(
        &mut module,
        [
            Action::Update { request },
            Action::Run { id: 2 },
            Action::Cancel,
        ],
    );
    assert!(module.shared.borrow().active.is_none());
    assert!(
        module.shared.borrow().results[&2]
            .outcome
            .as_ref()
            .unwrap_err()
            .contains("cancelled")
    );
    actions(&mut module, [Action::Run { id: 2 }]);
    module.restore_workspace(None, "/other-workspace");
    for _ in 0..5 {
        tick(&mut module);
        thread::sleep(Duration::from_millis(5));
    }
    assert!(module.shared.borrow().requests.is_empty());
    assert!(module.shared.borrow().results.is_empty());
    assert!(module.shared.borrow().active.is_none());
}
