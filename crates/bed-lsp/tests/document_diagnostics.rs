//! Observable stdio behavior for document diagnostic providers and stale replies.
#![cfg(unix)]
use bed_editing::editor_events::DocumentChange;
use bed_lsp::{lsp_client::LspClient, lsp_config::LspConfig, lsp_uri::LspUri};
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "bed-document-pulls-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let script = root.join("server.py");
        fs::write(&script, format!("#!/usr/bin/env python3\n{SCRIPT}")).unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        Self(root.canonicalize().unwrap())
    }
    fn path(&self, name: &str) -> String {
        let path = self.0.join(name);
        fs::write(&path, b"one").unwrap();
        path.to_string_lossy().into_owned()
    }
    fn client(&self, provider: Option<Value>) -> LspClient {
        let mut capabilities =
            json!({"textDocumentSync":{"openClose":true,"change":2,"save":true}});
        if let Some(provider) = provider {
            capabilities["diagnosticProvider"] = provider;
        }
        let config = LspConfig::from_json(&json!({
            "languages":[{"name":"fixture", "language_id":"fixture", "file_types":["py","txt"], "language_server":"fixture"}],
            "language_servers":{"fixture":{"command":self.0.join("server.py"), "args":[capabilities.to_string()]}}
        })).unwrap();
        let mut client = LspClient::with_config(self.0.join("lsp.json"), config);
        client.set_workspace(self.0.to_str().unwrap());
        assert!(client.start_server("fixture", "").unwrap());
        client
    }
    fn wire(&self) -> Vec<Value> {
        fs::read_to_string(self.0.join("wire.jsonl"))
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }
    fn wait_message(
        &self,
        client: &mut LspClient,
        predicate: impl Fn(&Value) -> bool,
        occurrence: usize,
    ) -> Value {
        let mut found = None;
        until(client, |_| {
            found = self.wire().into_iter().filter(&predicate).nth(occurrence);
            found.is_some()
        });
        found.unwrap()
    }
    fn pull(&self, client: &mut LspClient, path: &str, occurrence: usize) -> Value {
        let uri = uri(path);
        self.wait_message(
            client,
            |message| {
                message["method"] == "textDocument/diagnostic"
                    && message["params"]["textDocument"]["uri"] == uri
            },
            occurrence,
        )
    }
    fn send(&self, client: &LspClient, message: Value) {
        client
            .send_notification("fixture/control", Some(json!({"send":message})))
            .unwrap();
    }
    fn reply(&self, client: &LspClient, request: &Value, result: Value) {
        self.send(
            client,
            json!({"jsonrpc":"2.0", "id":request["id"], "result":result}),
        );
    }
    fn error(&self, client: &LspClient, request: &Value, error: Value) {
        self.send(
            client,
            json!({"jsonrpc":"2.0", "id":request["id"], "error":error}),
        );
    }
    fn barrier(&self, client: &mut LspClient) -> Value {
        let result = Rc::new(RefCell::new(None));
        let output = result.clone();
        client
            .send_request("fixture/barrier", json!({}), move |reply| {
                *output.borrow_mut() = Some(reply.unwrap());
            })
            .unwrap();
        until(client, |_| result.borrow().is_some());
        result.borrow_mut().take().unwrap()
    }
    fn registration(&self, client: &LspClient, id: &str, identifier: &str, pattern: &str) {
        self.send(client, json!({
            "jsonrpc":"2.0", "id":format!("register-{id}"), "method":"client/registerCapability",
            "params":{"registrations":[{"id":id,"method":"textDocument/diagnostic","registerOptions":{
                "documentSelector":[{"scheme":"file","pattern":pattern}], "identifier":identifier,
                "interFileDependencies":false,"workspaceDiagnostics":false
            }}]}
        }));
    }
    fn unregister(&self, client: &LspClient, id: &str) {
        self.send(client, json!({
            "jsonrpc":"2.0", "id":format!("unregister-{id}"), "method":"client/unregisterCapability",
            "params":{"unregisterations":[{"id":id,"method":"textDocument/diagnostic"}]}
        }));
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn until(client: &mut LspClient, mut ready: impl FnMut(&LspClient) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        client.poll();
        if ready(client) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "Document diagnostic fixture stalled: {:?}\n{}",
            client.last_error(),
            client.stderr_text()
        );
        thread::sleep(Duration::from_millis(2));
    }
}
fn settle(client: &mut LspClient) {
    let deadline = Instant::now() + Duration::from_millis(250);
    while Instant::now() < deadline {
        client.poll();
        thread::sleep(Duration::from_millis(2));
    }
}
fn uri(path: &str) -> String {
    LspUri::file_uri_from_path(path).unwrap().to_string()
}
fn provider(inter_file: bool) -> Value {
    json!({"identifier":"static-fixture","interFileDependencies":inter_file,"workspaceDiagnostics":false})
}
fn full(result_id: &str, message: &str) -> Value {
    json!({"kind":"full","resultId":result_id,"items":if message.is_empty() {vec![]} else {vec![json!({
        "range":{"start":{"line":0,"character":0},"end":{"line":0,"character":3}},
        "severity":1,"message":message
    })]}})
}
fn messages(client: &LspClient, path: &str) -> Vec<String> {
    client
        .diagnostics()
        .for_document(path)
        .into_iter()
        .map(|item| item.message)
        .collect()
}
fn change(client: &mut LspClient, path: &str, version: i32) {
    client
        .did_change(
            path,
            version,
            &[DocumentChange {
                end_character: 3,
                text: b"two".to_vec(),
                ..Default::default()
            }],
            || Ok("two".into()),
        )
        .unwrap();
}
fn save(client: &mut LspClient, path: &str) {
    client.did_save(path, || Ok("two".into())).unwrap();
}

const SCRIPT: &str = r#"
import json, sys, pathlib
capabilities=json.loads(sys.argv[1])
wire=open('wire.jsonl','a',buffering=1)
pending={}

def send(value):
    body=json.dumps(value).encode()
    sys.stdout.buffer.write(('Content-Length: %d\r\n\r\n'%len(body)).encode()+body)
    sys.stdout.buffer.flush()

def read():
    length=None
    while True:
        line=sys.stdin.buffer.readline()
        if not line: return None
        if line in (b'\r\n',b'\n'): break
        if line.lower().startswith(b'content-length:'): length=int(line.split(b':',1)[1])
    return json.loads(sys.stdin.buffer.read(length))

while True:
    message=read()
    if message is None: break
    wire.write(json.dumps(message)+'\n')
    method=message.get('method')
    if method=='initialize':
        send({'jsonrpc':'2.0','id':message['id'],'result':{'capabilities':capabilities}})
    elif method=='textDocument/diagnostic':
        pending[message['id']]=message
    elif method=='$/cancelRequest':
        pending.pop(message['params']['id'],None)
    elif method=='fixture/control':
        outgoing=message['params']['send']
        if 'result' in outgoing or 'error' in outgoing: pending.pop(outgoing.get('id'),None)
        send(outgoing)
    elif method=='fixture/barrier':
        send({'jsonrpc':'2.0','id':message['id'],'result':{'pending':len(pending)}})
    elif method=='shutdown':
        send({'jsonrpc':'2.0','id':message['id'],'result':None})
    elif method=='exit': break
"#;

#[test]
fn document_only_provider_uses_latest_pending_open_and_reuses_full_and_unchanged_results() {
    let fixture = Fixture::new();
    let path = fixture.path("latest.py");
    let related = fixture.path("related.py");
    let mut client = fixture.client(Some(provider(false)));
    client.did_open(&path, b"one", 1, "fixture").unwrap();
    change(&mut client, &path, 2);
    let first = fixture.pull(&mut client, &path, 0);
    let initialize = fixture
        .wire()
        .into_iter()
        .find(|message| message["method"] == "initialize")
        .unwrap();
    assert_eq!(
        initialize["params"]["capabilities"]["textDocument"]["diagnostic"]["dynamicRegistration"],
        true
    );
    assert_eq!(
        initialize["params"]["capabilities"]["textDocument"]["diagnostic"]["relatedDocumentSupport"],
        true
    );
    assert_eq!(first["params"]["identifier"], "static-fixture");
    assert!(first["params"].get("previousResultId").is_none());
    let opened = fixture
        .wire()
        .into_iter()
        .find(|message| message["method"] == "textDocument/didOpen")
        .unwrap();
    assert_eq!(opened["params"]["textDocument"]["version"], 2);
    assert_eq!(opened["params"]["textDocument"]["text"], "two");
    let mut report = full("primary-1", "latest error");
    report["relatedDocuments"] = json!({uri(&related):full("related-1","related error")});
    fixture.reply(&client, &first, report);
    until(&mut client, |client| {
        messages(client, &path) == ["latest error"]
            && messages(client, &related) == ["related error"]
    });
    save(&mut client, &path);
    let second = fixture.pull(&mut client, &path, 1);
    assert_eq!(second["params"]["previousResultId"], "primary-1");
    let mut malformed = full("invalid-primary", "must not apply primary");
    malformed["relatedDocuments"] =
        json!({uri(&related):{"kind":"full","resultId":"invalid-related","items":[false]}});
    fixture.reply(&client, &second, malformed);
    until(&mut client, |client| client.last_error().is_some());
    fixture.barrier(&mut client);
    assert_eq!(messages(&client, &path), ["latest error"]);
    assert_eq!(messages(&client, &related), ["related error"]);
    client.diagnostics().clear(&path);
    client.diagnostics().clear(&related);
    assert!(messages(&client, &path).is_empty());
    assert!(messages(&client, &related).is_empty());
    save(&mut client, &path);
    let unchanged = fixture.pull(&mut client, &path, 2);
    assert_eq!(unchanged["params"]["previousResultId"], "primary-1");
    fixture.reply(
        &client,
        &unchanged,
        json!({"kind":"unchanged","resultId":"primary-2","relatedDocuments":{
            uri(&related):{"kind":"unchanged","resultId":"related-2"}
        }}),
    );
    fixture.barrier(&mut client);
    assert_eq!(messages(&client, &path), ["latest error"]);
    assert_eq!(messages(&client, &related), ["related error"]);
    save(&mut client, &path);
    let third = fixture.pull(&mut client, &path, 3);
    assert_eq!(third["params"]["previousResultId"], "primary-2");
    let mut clear = full("primary-3", "");
    clear["relatedDocuments"] = json!({uri(&related):full("related-3", "")});
    fixture.reply(&client, &third, clear);
    until(&mut client, |client| {
        messages(client, &path).is_empty() && messages(client, &related).is_empty()
    });
    assert!(!client.workspace_diagnostics_complete());
    assert!(
        !fixture
            .wire()
            .iter()
            .any(|message| message["method"] == "workspace/diagnostic")
    );
}

#[test]
fn older_related_report_preserves_a_newer_own_report_at_the_same_document_version() {
    let fixture = Fixture::new();
    let primary = fixture.path("older-primary.py");
    let related = fixture.path("newer-related.py");
    let mut client = fixture.client(Some(provider(false)));
    client.did_open(&related, b"one", 1, "fixture").unwrap();
    let baseline = fixture.pull(&mut client, &related, 0);
    fixture.reply(
        &client,
        &baseline,
        full("own-baseline", "baseline related report"),
    );
    until(&mut client, |client| {
        messages(client, &related) == ["baseline related report"]
    });
    client.did_open(&primary, b"one", 1, "fixture").unwrap();
    let primary_request = fixture.pull(&mut client, &primary, 0);
    save(&mut client, &related);
    let own_request = fixture.pull(&mut client, &related, 1);
    fixture.reply(&client, &own_request, full("own-newer", "newer own report"));
    until(&mut client, |client| {
        messages(client, &related) == ["newer own report"]
    });
    let mut report = full("primary-current", "current primary report");
    report["relatedDocuments"] = json!({uri(&related):full("old-related", "older related report")});
    fixture.reply(&client, &primary_request, report);
    until(&mut client, |client| {
        messages(client, &primary) == ["current primary report"]
    });
    assert_eq!(messages(&client, &related), ["newer own report"]);
    save(&mut client, &related);
    let next = fixture.pull(&mut client, &related, 2);
    assert_eq!(next["params"]["previousResultId"], "own-newer");
    fixture.reply(
        &client,
        &next,
        json!({"kind":"unchanged","resultId":"own-current"}),
    );
    fixture.barrier(&mut client);
    assert_eq!(messages(&client, &related), ["newer own report"]);
}

#[test]
fn dependency_edits_and_server_refresh_repull_other_open_documents() {
    let fixture = Fixture::new();
    let first_path = fixture.path("first.py");
    let second_path = fixture.path("second.py");
    let mut client = fixture.client(Some(provider(true)));
    client.did_open(&first_path, b"one", 1, "fixture").unwrap();
    client.did_open(&second_path, b"one", 1, "fixture").unwrap();
    let first = fixture.pull(&mut client, &first_path, 0);
    let second = fixture.pull(&mut client, &second_path, 0);
    fixture.reply(&client, &first, full("first-1", "first error"));
    fixture.reply(&client, &second, full("second-1", "second error"));
    until(&mut client, |client| {
        messages(client, &second_path) == ["second error"]
    });
    change(&mut client, &first_path, 2);
    let edited = fixture.pull(&mut client, &first_path, 1);
    let dependency = fixture.pull(&mut client, &second_path, 1);
    assert_eq!(edited["params"]["previousResultId"], "first-1");
    assert_eq!(dependency["params"]["previousResultId"], "second-1");
    fixture.reply(&client, &edited, full("first-2", ""));
    fixture.reply(&client, &dependency, full("second-2", "dependency changed"));
    until(&mut client, |client| {
        messages(client, &second_path) == ["dependency changed"]
    });
    fixture.send(
        &client,
        json!({"jsonrpc":"2.0","id":"refresh","method":"workspace/diagnostic/refresh"}),
    );
    fixture.wait_message(
        &mut client,
        |message| message["id"] == "refresh" && message.get("result").is_some(),
        0,
    );
    let refreshed_first = fixture.pull(&mut client, &first_path, 2);
    let refreshed_second = fixture.pull(&mut client, &second_path, 2);
    fixture.reply(&client, &refreshed_first, full("first-3", ""));
    fixture.reply(&client, &refreshed_second, full("second-3", ""));
    until(&mut client, |client| {
        messages(client, &second_path).is_empty()
    });
}

#[test]
fn edited_related_documents_and_reopened_primary_reject_old_reports() {
    let fixture = Fixture::new();
    let primary = fixture.path("primary.py");
    let related = fixture.path("related.py");
    let mut client = fixture.client(Some(provider(false)));
    client.did_open(&primary, b"one", 1, "fixture").unwrap();
    client.did_open(&related, b"one", 1, "fixture").unwrap();
    let primary_request = fixture.pull(&mut client, &primary, 0);
    let related_request = fixture.pull(&mut client, &related, 0);
    change(&mut client, &related, 2);
    let mut report = full("primary-1", "accepted primary");
    let related_alias = uri(&related).replace("related.py", "%72elated.py");
    report["relatedDocuments"] =
        json!({related_alias:full("obsolete-related", "obsolete related")});
    fixture.reply(&client, &primary_request, report);
    fixture.reply(
        &client,
        &related_request,
        full("obsolete-own", "obsolete own"),
    );
    let fresh_related = fixture.pull(&mut client, &related, 1);
    fixture.barrier(&mut client);
    assert_eq!(messages(&client, &primary), ["accepted primary"]);
    assert!(messages(&client, &related).is_empty());
    assert!(
        fixture
            .wire()
            .iter()
            .any(|message| message["method"] == "$/cancelRequest"
                && message["params"]["id"] == related_request["id"])
    );
    fixture.reply(
        &client,
        &fresh_related,
        full("related-2", "current related"),
    );
    until(&mut client, |client| {
        messages(client, &related) == ["current related"]
    });
    save(&mut client, &primary);
    let before_close = fixture.pull(&mut client, &primary, 1);
    client.did_close(&primary).unwrap();
    client.did_open(&primary, b"one", 1, "fixture").unwrap();
    let reopened = fixture.pull(&mut client, &primary, 2);
    let mut obsolete = full("obsolete-primary", "closed primary");
    obsolete["relatedDocuments"] =
        json!({uri(&related):full("obsolete-related-2", "closed related")});
    fixture.reply(&client, &before_close, obsolete);
    fixture.barrier(&mut client);
    assert!(
        !messages(&client, &primary)
            .iter()
            .any(|message| message == "closed primary")
    );
    assert_eq!(messages(&client, &related), ["current related"]);
    client.did_close(&related).unwrap();
    client.did_open(&related, b"one", 2, "fixture").unwrap();
    let related_reopened = fixture.pull(&mut client, &related, 2);
    let mut reopened_report = full("reopened-1", "reopened primary");
    reopened_report["relatedDocuments"] =
        json!({uri(&related):full("old-reopened-related", "wrong reopened lifetime")});
    fixture.reply(&client, &reopened, reopened_report);
    until(&mut client, |client| {
        messages(client, &primary) == ["reopened primary"]
    });
    assert!(
        !messages(&client, &related)
            .iter()
            .any(|message| message == "wrong reopened lifetime")
    );
    fixture.reply(
        &client,
        &related_reopened,
        full("reopened-related-2", "new related lifetime"),
    );
    until(&mut client, |client| {
        messages(client, &related) == ["new related lifetime"]
    });
    assert!(
        fixture
            .wire()
            .iter()
            .any(|message| message["method"] == "$/cancelRequest"
                && message["params"]["id"] == before_close["id"])
    );
}

#[test]
fn refresh_guards_primary_related_and_optional_partial_reports() {
    let fixture = Fixture::new();
    let primary = fixture.path("partial.py");
    let related = fixture.path("closed.py");
    let mut client = fixture.client(Some(provider(false)));
    client.did_open(&primary, b"one", 1, "fixture").unwrap();
    let old = fixture.pull(&mut client, &primary, 0);
    let token = old["params"]["partialResultToken"].clone();
    if token.is_string() {
        fixture.send(
            &client,
            json!({"jsonrpc":"2.0","method":"$/progress","params":{"token":token,"value":{
                "relatedDocuments":{uri(&related):full("partial-related", "partial error")}
            }}}),
        );
    } else {
        fixture.send(
            &client,
            json!({"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{
                "uri":uri(&related),"diagnostics":full("seed", "partial error")["items"]
            }}),
        );
    }
    until(&mut client, |client| {
        messages(client, &related) == ["partial error"]
    });
    client.refresh_workspace_diagnostics();
    let refreshed = fixture.pull(&mut client, &primary, 1);
    if token.is_string() {
        fixture.send(
            &client,
            json!({"jsonrpc":"2.0","method":"$/progress","params":{"token":token,"value":{
                "relatedDocuments":{uri(&related):full("late-partial", "late partial")}
            }}}),
        );
    }
    let mut stale = full("stale", "late primary");
    stale["relatedDocuments"] = json!({uri(&related):full("stale-related", "late final")});
    fixture.reply(&client, &old, stale);
    fixture.barrier(&mut client);
    assert_eq!(messages(&client, &related), ["partial error"]);
    assert!(
        !messages(&client, &primary)
            .iter()
            .any(|message| message == "late primary")
    );
    // Opening a related URI changes its lifetime while the primary is pending.
    client.did_open(&related, b"one", 1, "fixture").unwrap();
    let related_pull = fixture.pull(&mut client, &related, 0);
    let mut final_report = full("refreshed", "refreshed primary");
    final_report["relatedDocuments"] =
        json!({uri(&related):full("old-related-lifetime", "old lifetime")});
    fixture.reply(&client, &refreshed, final_report);
    fixture.reply(
        &client,
        &related_pull,
        full("opened-related", "current lifetime"),
    );
    until(&mut client, |client| {
        messages(client, &primary) == ["refreshed primary"]
            && messages(client, &related) == ["current lifetime"]
    });
}

#[test]
fn dynamic_selectors_identifiers_and_unregistration_control_document_requests() {
    let fixture = Fixture::new();
    let python = fixture.path("selected.py");
    let text = fixture.path("other file.txt");
    let mut client = fixture.client(None);
    client.did_open(&python, b"one", 1, "fixture").unwrap();
    client.did_open(&text, b"one", 1, "fixture").unwrap();
    until(&mut client, LspClient::is_initialized);
    assert_eq!(fixture.barrier(&mut client)["pending"], 0);
    fixture.registration(&client, "python", "python-provider", "**/*.py");
    let selected = fixture.pull(&mut client, &python, 0);
    assert_eq!(selected["params"]["identifier"], "python-provider");
    assert_eq!(fixture.barrier(&mut client)["pending"], 1);
    fixture.reply(&client, &selected, full("python-1", "selected provider"));
    until(&mut client, |client| {
        messages(client, &python) == ["selected provider"]
    });
    fixture.registration(&client, "text", "text-provider", "**/*.txt");
    let text_request = fixture.pull(&mut client, &text, 0);
    assert_eq!(text_request["params"]["identifier"], "text-provider");
    fixture.reply(&client, &text_request, full("text-1", "text provider"));
    until(&mut client, |client| {
        messages(client, &text) == ["text provider"]
    });
    fixture.unregister(&client, "python");
    fixture.wait_message(
        &mut client,
        |message| message["id"] == "unregister-python" && message.get("result").is_some(),
        0,
    );
    let python_requests = fixture
        .wire()
        .iter()
        .filter(|message| {
            message["method"] == "textDocument/diagnostic"
                && message["params"]["textDocument"]["uri"] == uri(&python)
        })
        .count();
    change(&mut client, &python, 2);
    save(&mut client, &text);
    let remaining = fixture.pull(&mut client, &text, 1);
    assert_eq!(remaining["params"]["identifier"], "text-provider");
    assert_eq!(remaining["params"]["previousResultId"], "text-1");
    fixture.unregister(&client, "text");
    fixture.wait_message(
        &mut client,
        |message| message["id"] == "unregister-text" && message.get("result").is_some(),
        0,
    );
    fixture.reply(
        &client,
        &remaining,
        full("stale-text", "unregistered report"),
    );
    settle(&mut client);
    fixture.barrier(&mut client);
    assert_eq!(
        fixture
            .wire()
            .iter()
            .filter(|message| message["method"] == "textDocument/diagnostic"
                && message["params"]["textDocument"]["uri"] == uri(&python))
            .count(),
        python_requests
    );
    assert!(
        !messages(&client, &text)
            .iter()
            .any(|message| message == "unregistered report")
    );
}

#[test]
fn server_cancelled_retries_by_default_and_explicit_false_waits_for_a_new_trigger() {
    let fixture = Fixture::new();
    let path = fixture.path("failure.py");
    let mut client = fixture.client(Some(provider(false)));
    client.did_open(&path, b"one", 1, "fixture").unwrap();
    let first = fixture.pull(&mut client, &path, 0);
    fixture.error(
        &client,
        &first,
        json!({"code":-32802,"message":"retry cancellation"}),
    );
    let retried = fixture.pull(&mut client, &path, 1);
    fixture.error(
        &client,
        &retried,
        json!({"code":-32603,"message":"fixture failure"}),
    );
    until(&mut client, |client| {
        client
            .last_error()
            .is_some_and(|error| error.contains("fixture failure"))
    });
    settle(&mut client);
    assert_eq!(fixture.barrier(&mut client)["pending"], 0);
    save(&mut client, &path);
    let triggered = fixture.pull(&mut client, &path, 2);
    fixture.error(
        &client,
        &triggered,
        json!({"code":-32802,"message":"no retrigger","data":{"retriggerRequest":false}}),
    );
    settle(&mut client);
    assert_eq!(fixture.barrier(&mut client)["pending"], 0);
    save(&mut client, &path);
    let recovered = fixture.pull(&mut client, &path, 3);
    fixture.reply(&client, &recovered, full("recovered", "recovered report"));
    until(&mut client, |client| {
        messages(client, &path) == ["recovered report"]
    });
}

#[test]
fn concurrent_document_pulls_are_bounded_and_completion_releases_capacity() {
    let fixture = Fixture::new();
    let paths: Vec<_> = (0..6)
        .map(|index| fixture.path(&format!("bounded-{index}.py")))
        .collect();
    let mut client = fixture.client(Some(provider(false)));
    for path in &paths {
        client.did_open(path, b"one", 1, "fixture").unwrap();
    }
    fixture.wait_message(
        &mut client,
        |message| message["method"] == "textDocument/diagnostic",
        3,
    );
    assert_eq!(fixture.barrier(&mut client)["pending"], 4);
    let first = fixture
        .wire()
        .into_iter()
        .find(|message| message["method"] == "textDocument/diagnostic")
        .unwrap();
    fixture.reply(&client, &first, full("finished", ""));
    fixture.wait_message(
        &mut client,
        |message| message["method"] == "textDocument/diagnostic",
        4,
    );
    assert_eq!(fixture.barrier(&mut client)["pending"], 4);
}

#[test]
fn provider_identifiers_keep_independent_caches_and_merge_one_documents_diagnostics() {
    let fixture = Fixture::new();
    let path = fixture.path("providers.py");
    let mut client = fixture.client(None);
    client.did_open(&path, b"one", 1, "fixture").unwrap();
    until(&mut client, LspClient::is_initialized);
    fixture.send(&client, json!({
        "jsonrpc":"2.0","id":"register-both","method":"client/registerCapability","params":{"registrations":[
            {"id":"first","method":"textDocument/diagnostic","registerOptions":{"documentSelector":null,"identifier":"first-provider","interFileDependencies":false,"workspaceDiagnostics":false}},
            {"id":"second","method":"textDocument/diagnostic","registerOptions":{"documentSelector":null,"identifier":"second-provider","interFileDependencies":false,"workspaceDiagnostics":false}}
        ]}
    }));
    let first = fixture.wait_message(
        &mut client,
        |message| {
            message["method"] == "textDocument/diagnostic"
                && message["params"]["identifier"] == "first-provider"
        },
        0,
    );
    let second = fixture.wait_message(
        &mut client,
        |message| {
            message["method"] == "textDocument/diagnostic"
                && message["params"]["identifier"] == "second-provider"
        },
        0,
    );
    fixture.reply(&client, &first, full("first-cache", "first provider"));
    fixture.reply(&client, &second, full("second-cache", "second provider"));
    until(&mut client, |client| {
        let mut result = messages(client, &path);
        result.sort();
        result == ["first provider", "second provider"]
    });
    save(&mut client, &path);
    let first_next = fixture.wait_message(
        &mut client,
        |message| {
            message["method"] == "textDocument/diagnostic"
                && message["params"]["identifier"] == "first-provider"
        },
        1,
    );
    let second_next = fixture.wait_message(
        &mut client,
        |message| {
            message["method"] == "textDocument/diagnostic"
                && message["params"]["identifier"] == "second-provider"
        },
        1,
    );
    assert_eq!(first_next["params"]["previousResultId"], "first-cache");
    assert_eq!(second_next["params"]["previousResultId"], "second-cache");
    fixture.reply(
        &client,
        &first_next,
        full("first-next", "first provider changed"),
    );
    fixture.reply(
        &client,
        &second_next,
        json!({"kind":"unchanged","resultId":"second-unchanged"}),
    );
    until(&mut client, |client| {
        let mut result = messages(client, &path);
        result.sort();
        result == ["first provider changed", "second provider"]
    });
    fixture.unregister(&client, "first");
    fixture.wait_message(
        &mut client,
        |message| message["id"] == "unregister-first" && message.get("result").is_some(),
        0,
    );
    until(&mut client, |client| {
        messages(client, &path) == ["second provider"]
    });
    save(&mut client, &path);
    let remaining = fixture.wait_message(
        &mut client,
        |message| {
            message["method"] == "textDocument/diagnostic"
                && message["params"]["identifier"] == "second-provider"
        },
        2,
    );
    assert_eq!(remaining["params"]["previousResultId"], "second-unchanged");
    fixture.reply(
        &client,
        &remaining,
        json!({"kind":"unchanged","resultId":"second-3"}),
    );
    fixture.barrier(&mut client);
    assert_eq!(messages(&client, &path), ["second provider"]);
    fixture.registration(&client, "first", "first-provider", "**/*.py");
    let registered_again = fixture.wait_message(
        &mut client,
        |message| {
            message["method"] == "textDocument/diagnostic"
                && message["params"]["identifier"] == "first-provider"
        },
        2,
    );
    assert!(registered_again["params"].get("previousResultId").is_none());
    fixture.reply(
        &client,
        &registered_again,
        full("first-new-registration", "first registered again"),
    );
    until(&mut client, |client| {
        let mut result = messages(client, &path);
        result.sort();
        result == ["first registered again", "second provider"]
    });
    assert!(!client.workspace_diagnostics_complete());
}

#[test]
fn workspace_reports_started_before_document_open_cannot_overwrite_new_document_results() {
    let fixture = Fixture::new();
    let path = fixture.path("workspace-overlap.py");
    let mut capabilities = provider(false);
    capabilities["workspaceDiagnostics"] = json!(true);
    let mut client = fixture.client(Some(capabilities));
    let workspace = fixture.wait_message(
        &mut client,
        |message| message["method"] == "workspace/diagnostic",
        0,
    );
    client.did_open(&path, b"one", 1, "fixture").unwrap();
    let document = fixture.pull(&mut client, &path, 0);
    fixture.reply(
        &client,
        &document,
        full("document-new", "fresh document report"),
    );
    until(&mut client, |client| {
        messages(client, &path) == ["fresh document report"]
    });
    let mut stale = full("workspace-old", "stale workspace report");
    stale["uri"] = json!(uri(&path));
    stale["version"] = json!(1);
    fixture.send(
        &client,
        json!({"jsonrpc":"2.0","method":"$/progress","params":{
            "token":workspace["params"]["partialResultToken"],"value":{"items":[stale.clone()]}
        }}),
    );
    fixture.reply(&client, &workspace, json!({"items":[stale]}));
    fixture.barrier(&mut client);
    assert_eq!(messages(&client, &path), ["fresh document report"]);
    client.refresh_workspace_diagnostics();
    let next_document = fixture.pull(&mut client, &path, 1);
    let next_workspace = fixture.wait_message(
        &mut client,
        |message| message["method"] == "workspace/diagnostic",
        1,
    );
    fixture.reply(
        &client,
        &next_document,
        full("document-newer", "newer same-version document report"),
    );
    until(&mut client, |client| {
        messages(client, &path) == ["newer same-version document report"]
    });
    let mut workspace_partial = full("workspace-equal-version", "equal-version workspace report");
    workspace_partial["uri"] = json!(uri(&path));
    workspace_partial["version"] = json!(1);
    fixture.send(&client, json!({"jsonrpc":"2.0","method":"$/progress","params":{
        "token":next_workspace["params"]["partialResultToken"],"value":{"items":[workspace_partial]}
    }}));
    let mut workspace_final = full("workspace-null-version", "null-version workspace report");
    workspace_final["uri"] = json!(uri(&path));
    workspace_final["version"] = Value::Null;
    fixture.reply(&client, &next_workspace, json!({"items":[workspace_final]}));
    fixture.barrier(&mut client);
    assert_eq!(
        messages(&client, &path),
        ["newer same-version document report"]
    );
}

#[test]
fn workspace_provider_identifiers_keep_reports_caches_and_refresh_independent() {
    let fixture = Fixture::new();
    let document_path = fixture.path("workspace-providers.py");
    let closed_path = fixture.path("workspace-closed.py");
    let mut client = fixture.client(None);
    client
        .did_open(&document_path, b"one", 1, "fixture")
        .unwrap();
    until(&mut client, LspClient::is_initialized);
    fixture.send(&client, json!({
        "jsonrpc":"2.0","id":"register-workspaces","method":"client/registerCapability","params":{"registrations":[
            {"id":"workspace-first","method":"textDocument/diagnostic","registerOptions":{"documentSelector":[{"language":"*","scheme":"*"}],"identifier":"workspace-first","interFileDependencies":false,"workspaceDiagnostics":true}},
            {"id":"workspace-second","method":"textDocument/diagnostic","registerOptions":{"documentSelector":[{"language":"*","scheme":"*"}],"identifier":"workspace-second","interFileDependencies":false,"workspaceDiagnostics":true}}
        ]}
    }));
    let request = |message: &Value, method: &str, identifier: &str| {
        message["method"] == method && message["params"]["identifier"] == identifier
    };
    let first_document = fixture.wait_message(
        &mut client,
        |message| request(message, "textDocument/diagnostic", "workspace-first"),
        0,
    );
    let second_document = fixture.wait_message(
        &mut client,
        |message| request(message, "textDocument/diagnostic", "workspace-second"),
        0,
    );
    let first_workspace = fixture.wait_message(
        &mut client,
        |message| request(message, "workspace/diagnostic", "workspace-first"),
        0,
    );
    let second_workspace = fixture.wait_message(
        &mut client,
        |message| request(message, "workspace/diagnostic", "workspace-second"),
        0,
    );
    fixture.reply(
        &client,
        &first_document,
        full("first-document", "first document provider"),
    );
    fixture.reply(
        &client,
        &second_document,
        full("second-document", "second document provider"),
    );
    let mut first_closed = full("first-workspace", "first closed provider");
    first_closed["uri"] = json!(uri(&closed_path));
    first_closed["version"] = Value::Null;
    fixture.reply(&client, &first_workspace, json!({"items":[first_closed]}));
    until(&mut client, |client| {
        messages(client, &closed_path) == ["first closed provider"]
    });
    assert!(
        !client.workspace_diagnostics_complete(),
        "Both workspace providers must finish"
    );
    let mut second_closed = full("second-workspace", "second closed provider");
    second_closed["uri"] = json!(uri(&closed_path));
    second_closed["version"] = Value::Null;
    fixture.reply(&client, &second_workspace, json!({"items":[second_closed]}));
    until(&mut client, |client| {
        let mut result = messages(client, &closed_path);
        result.sort();
        result == ["first closed provider", "second closed provider"]
            && client.workspace_diagnostics_complete()
    });
    client.refresh_workspace_diagnostics();
    let first_refreshed = fixture.wait_message(
        &mut client,
        |message| request(message, "workspace/diagnostic", "workspace-first"),
        1,
    );
    let second_refreshed = fixture.wait_message(
        &mut client,
        |message| request(message, "workspace/diagnostic", "workspace-second"),
        1,
    );
    for (pull, own, other) in [
        (&first_refreshed, "first-workspace", "second-workspace"),
        (&second_refreshed, "second-workspace", "first-workspace"),
    ] {
        let ids = pull["params"]["previousResultIds"].as_array().unwrap();
        assert!(
            ids.iter()
                .any(|item| item["uri"] == uri(&closed_path) && item["value"] == own)
        );
        assert!(!ids.iter().any(|item| item["value"] == other));
    }
    let first_document_next = fixture.wait_message(
        &mut client,
        |message| request(message, "textDocument/diagnostic", "workspace-first"),
        1,
    );
    let second_document_next = fixture.wait_message(
        &mut client,
        |message| request(message, "textDocument/diagnostic", "workspace-second"),
        1,
    );
    assert_eq!(
        first_document_next["params"]["previousResultId"],
        "first-document"
    );
    assert_eq!(
        second_document_next["params"]["previousResultId"],
        "second-document"
    );
    fixture.reply(&client, &first_refreshed, json!({"items":[{"uri":uri(&closed_path),"version":null,"kind":"unchanged","resultId":"first-workspace"}]}));
    fixture.reply(&client, &second_refreshed, json!({"items":[{"uri":uri(&closed_path),"version":null,"kind":"unchanged","resultId":"second-workspace"}]}));
    fixture.reply(
        &client,
        &first_document_next,
        json!({"kind":"unchanged","resultId":"first-document-2"}),
    );
    fixture.reply(
        &client,
        &second_document_next,
        json!({"kind":"unchanged","resultId":"second-document-2"}),
    );
    until(&mut client, LspClient::workspace_diagnostics_complete);
    fixture.unregister(&client, "workspace-first");
    fixture.wait_message(
        &mut client,
        |message| message["id"] == "unregister-workspace-first" && message.get("result").is_some(),
        0,
    );
    until(&mut client, |client| {
        messages(client, &closed_path) == ["second closed provider"]
            && messages(client, &document_path) == ["second document provider"]
    });
    fixture.barrier(&mut client);
    let wire = fixture.wire();
    let first_requests = wire
        .iter()
        .filter(|message| {
            request(message, "workspace/diagnostic", "workspace-first")
                || request(message, "textDocument/diagnostic", "workspace-first")
        })
        .count();
    let workspace_requests = wire
        .iter()
        .filter(|message| request(message, "workspace/diagnostic", "workspace-second"))
        .count();
    let document_requests = wire
        .iter()
        .filter(|message| request(message, "textDocument/diagnostic", "workspace-second"))
        .count();
    client.refresh_workspace_diagnostics();
    let remaining_workspace = fixture.wait_message(
        &mut client,
        |message| request(message, "workspace/diagnostic", "workspace-second"),
        workspace_requests,
    );
    let remaining_document = fixture.wait_message(
        &mut client,
        |message| request(message, "textDocument/diagnostic", "workspace-second"),
        document_requests,
    );
    assert_eq!(
        remaining_document["params"]["previousResultId"],
        "second-document-2"
    );
    assert!(
        remaining_workspace["params"]["previousResultIds"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["uri"] == uri(&closed_path) && item["value"] == "second-workspace")
    );
    fixture.reply(&client, &remaining_workspace, json!({"items":[{"uri":uri(&closed_path),"version":null,"kind":"unchanged","resultId":"second-workspace-3"}]}));
    fixture.reply(
        &client,
        &remaining_document,
        json!({"kind":"unchanged","resultId":"second-document-3"}),
    );
    until(&mut client, LspClient::workspace_diagnostics_complete);
    assert_eq!(
        fixture
            .wire()
            .iter()
            .filter(
                |message| request(message, "workspace/diagnostic", "workspace-first")
                    || request(message, "textDocument/diagnostic", "workspace-first")
            )
            .count(),
        first_requests
    );
    assert_eq!(messages(&client, &closed_path), ["second closed provider"]);
}

#[test]
fn static_registration_id_can_be_unregistered_and_registered_again_without_old_cache() {
    let fixture = Fixture::new();
    let path = fixture.path("static-registration.py");
    let mut options = provider(false);
    options["id"] = json!("static-registration");
    let mut client = fixture.client(Some(options));
    client.did_open(&path, b"one", 1, "fixture").unwrap();
    let initial = fixture.pull(&mut client, &path, 0);
    fixture.reply(
        &client,
        &initial,
        full("static-cache", "static provider report"),
    );
    until(&mut client, |client| {
        messages(client, &path) == ["static provider report"]
    });
    fixture.unregister(&client, "static-registration");
    fixture.wait_message(
        &mut client,
        |message| {
            message["id"] == "unregister-static-registration" && message.get("result").is_some()
        },
        0,
    );
    until(&mut client, |client| messages(client, &path).is_empty());
    save(&mut client, &path);
    settle(&mut client);
    assert_eq!(fixture.barrier(&mut client)["pending"], 0);
    fixture.registration(&client, "static-registration", "static-fixture", "**/*.py");
    let restored = fixture.pull(&mut client, &path, 1);
    assert!(restored["params"].get("previousResultId").is_none());
    fixture.reply(&client, &restored, full("new-cache", "registered again"));
    until(&mut client, |client| {
        messages(client, &path) == ["registered again"]
    });
}

#[test]
fn stale_workspace_report_preserves_push_diagnostics_for_an_open_document_outside_the_selector() {
    let fixture = Fixture::new();
    let path = fixture.path("outside-selector.txt");
    let mut client = fixture.client(None);
    client.did_open(&path, b"one", 0, "fixture").unwrap();
    until(&mut client, LspClient::is_initialized);
    fixture.send(&client, json!({
        "jsonrpc":"2.0","id":"register-selected-workspace","method":"client/registerCapability",
        "params":{"registrations":[{
            "id":"selected-workspace","method":"textDocument/diagnostic","registerOptions":{
                "documentSelector":[{"scheme":"file","pattern":"**/*.py"}],
                "identifier":"selected-workspace","interFileDependencies":false,"workspaceDiagnostics":true
            }
        }]}
    }));
    let workspace = fixture.wait_message(
        &mut client,
        |message| {
            message["method"] == "workspace/diagnostic"
                && message["params"]["identifier"] == "selected-workspace"
        },
        0,
    );
    change(&mut client, &path, 1);
    fixture.send(&client, json!({
        "jsonrpc":"2.0","method":"textDocument/publishDiagnostics",
        "params":{"uri":uri(&path),"version":1,"diagnostics":full("unused","fresh push")["items"]}
    }));
    until(&mut client, |client| {
        messages(client, &path) == ["fresh push"]
    });
    let mut stale = full("stale-workspace", "stale workspace");
    stale["uri"] = json!(uri(&path));
    stale["version"] = json!(0);
    fixture.reply(&client, &workspace, json!({"items":[stale]}));
    fixture.barrier(&mut client);
    assert_eq!(messages(&client, &path), ["fresh push"]);
    assert!(!fixture.wire().iter().any(|message| {
        message["method"] == "textDocument/diagnostic"
            && message["params"]["textDocument"]["uri"] == uri(&path)
    }));
}

#[test]
fn closing_a_primary_document_preserves_related_results_owned_by_an_open_selected_document() {
    let fixture = Fixture::new();
    let primary = fixture.path("primary-owner.py");
    let related = fixture.path("related-owner.py");
    let mut client = fixture.client(Some(provider(false)));
    client.did_open(&related, b"one", 1, "fixture").unwrap();
    let baseline = fixture.pull(&mut client, &related, 0);
    fixture.reply(
        &client,
        &baseline,
        full("related-baseline", "related own baseline"),
    );
    until(&mut client, |client| {
        messages(client, &related) == ["related own baseline"]
    });
    client.did_open(&primary, b"one", 1, "fixture").unwrap();
    let primary_pull = fixture.pull(&mut client, &primary, 0);
    let mut report = full("primary-report", "primary report");
    report["relatedDocuments"] =
        json!({uri(&related):full("related-newer","newer related report")});
    fixture.reply(&client, &primary_pull, report);
    until(&mut client, |client| {
        messages(client, &related) == ["newer related report"]
    });
    client.did_close(&primary).unwrap();
    fixture.barrier(&mut client);
    assert!(messages(&client, &primary).is_empty());
    assert_eq!(messages(&client, &related), ["newer related report"]);
    save(&mut client, &related);
    let next = fixture.pull(&mut client, &related, 1);
    assert_eq!(next["params"]["previousResultId"], "related-newer");
    fixture.reply(
        &client,
        &next,
        json!({"kind":"unchanged","resultId":"related-retained"}),
    );
    fixture.barrier(&mut client);
    assert_eq!(messages(&client, &related), ["newer related report"]);
}
