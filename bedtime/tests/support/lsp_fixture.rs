// Deterministic stdio server and fixtures shared by owner-specific integration executables.
use bedtime_editing::identity::SessionId;
use bedtime_lsp::{
    connection::{Connection, encode_packet, write_packet},
    jsonrpc::{METHOD_NOT_FOUND, Message, Packet, Request, Response, ResponseError, RpcId},
    lsp_client::LspClient,
    lsp_uri::LspUri,
    message_handler::RpcSession,
    workspace_lsp::WorkspaceLsp,
};
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    fs,
    io::{self, BufReader, Write},
    path::PathBuf,
    process::{Command, Stdio},
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

static TEMP_ID: AtomicU64 = AtomicU64::new(0);
const STDERR_DRAIN_SENTINEL: &str = "\n[stderr complete]\n";

struct Fixture {
    root: PathBuf,
    client: LspClient,
    document: String,
}

#[allow(dead_code)] // This shared fixture serves targets exercising different layers.
impl Fixture {
    fn new(scenario: &str) -> Self {
        Self::with_server(scenario, "mock", json!({}))
    }

    fn with_server(scenario: &str, server: &str, settings: Value) -> Self {
        let root = std::env::temp_dir().join(format!(
            "bed-lsp-{}-{}",
            std::process::id(),
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let config = root.join("lsp.json");
        fs::write(&config, serde_json::to_vec(&json!({
            "languages":[{"name":"mock","language_id":"mock","file_types":["rs"],"language_server":server}],
            "language_servers":{(server):{"command":std::env::current_exe().unwrap(),"args":["--mock-server",scenario],"settings":settings}}
        })).unwrap()).unwrap();
        let document = root.join("sp ace😀.rs").to_string_lossy().into_owned();
        let mut client = LspClient::new(config);
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

#[allow(dead_code)] // This shared fixture serves targets exercising different layers.
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

#[allow(dead_code)] // This shared fixture serves targets exercising different layers.
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

struct PoolFixture {
    root: PathBuf,
    pool: WorkspaceLsp,
}

#[allow(dead_code)] // This shared fixture serves targets exercising different layers.
impl PoolFixture {
    fn new(id: u64, scenario: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "bed-lsp-pool-{}-{}",
            std::process::id(),
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let config = root.join("lsp.json");
        fs::write(&config, serde_json::to_vec(&json!({
            "languages":[
                {"name":"cpp","language_id":"cpp","file_types":["cpp"],"language_server":"cpp"},
                {"name":"rust","language_id":"rust","file_types":["rs"],"language_server":"rust"}
            ],
            "language_servers":{
                "cpp":{"command":std::env::current_exe().unwrap(),"args":["--mock-server",scenario]},
                "rust":{"command":std::env::current_exe().unwrap(),"args":["--mock-server",scenario]}
            }
        })).unwrap()).unwrap();
        let pool = WorkspaceLsp::new(SessionId(id), config, root.clone());
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
    if matches!(scenario, "stderr" | "stderr-drain") {
        let mut stderr = io::stderr().lock();
        stderr.write_all(&vec![b'x'; 256 * 1024]).unwrap();
        if scenario == "stderr-drain" {
            stderr.write_all(STDERR_DRAIN_SENTINEL.as_bytes()).unwrap();
        }
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
    let mut definition_errors = std::collections::VecDeque::new();
    let mut hold_definitions = false;
    let mut held_definitions = Vec::new();
    let mut pull_documents = std::collections::BTreeMap::<String, (String, i32)>::new();
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
            if scenario == "document-pull" {
                if matches!(
                    request.method.as_str(),
                    "textDocument/didOpen" | "textDocument/didChange"
                ) {
                    let params = request.params.as_ref().unwrap();
                    let document = &params["textDocument"];
                    let text = if request.method == "textDocument/didOpen" {
                        &document["text"]
                    } else {
                        &params["contentChanges"][0]["text"]
                    };
                    pull_documents.insert(
                        document["uri"].as_str().unwrap().into(),
                        (
                            text.as_str().unwrap().into(),
                            document["version"].as_i64().unwrap() as i32,
                        ),
                    );
                } else if request.method == "textDocument/didClose" {
                    pull_documents.remove(
                        request.params.as_ref().unwrap()["textDocument"]["uri"]
                            .as_str()
                            .unwrap(),
                    );
                }
            }
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
            if scenario == "navigation-cold" && request.method == "initialized" {
                write_packet(&mut output, &Packet::Single(Message::Request(Request {
                    id: None,
                    method: "$/progress".into(),
                    params: Some(json!({"token":"cold-roots","value":{"kind":"begin","title":"Roots scanned","message":"Still indexing","percentage":25}})),
                }))).unwrap();
            }
            if scenario == "silent" {
                continue;
            }
            let Some(id) = request.id.clone() else {
                continue;
            };
            let mut result = match request.method.as_str() {
                "initialize" => {
                    if matches!(scenario, "delayed" | "navigation-cold") {
                        thread::sleep(Duration::from_millis(50));
                    }
                    if scenario == "initialize-error" {
                        Err(ResponseError::new(-32002, "not ready"))
                    } else {
                        let sync = match scenario {
                            "full" => json!({"change":1,"save":false}),
                            "document-pull" => json!({"change":1,"save":true}),
                            "none" => json!({"change":0,"save":false}),
                            "save-no-text" => json!({"change":2,"save":{"includeText":false}}),
                            _ => json!({"change":2,"save":{"includeText":true}}),
                        };
                        let mut capabilities = json!({"textDocumentSync":sync,"hoverProvider":true,"definitionProvider":true,"referencesProvider":true});
                        if scenario == "document-pull" {
                            capabilities["diagnosticProvider"] = json!({"identifier":"fixture","interFileDependencies":false,"workspaceDiagnostics":false});
                        }
                        Ok(json!({"capabilities":capabilities}))
                    }
                }
                "textDocument/diagnostic" if scenario == "document-pull" => {
                    let params = request.params.as_ref().unwrap();
                    assert_eq!(params["identifier"], "fixture");
                    let (text, version) =
                        &pull_documents[params["textDocument"]["uri"].as_str().unwrap()];
                    let result_id = format!("pull-{version}");
                    if params["previousResultId"] == result_id {
                        Ok(json!({"kind":"unchanged","resultId":result_id}))
                    } else {
                        let items = if text.contains("INVALID") {
                            json!([{"range":{"start":{"line":0,"character":5},"end":{"line":0,"character":12}},"severity":1,"message":"Unsaved pull diagnostic"}])
                        } else {
                            json!([])
                        };
                        Ok(json!({"kind":"full","resultId":result_id,"items":items}))
                    }
                }
                "shutdown" => Ok(Value::Null),
                "test/transcript" => Ok(Value::Array(transcript.clone())),
                "test/definition/configure" => {
                    let params = request.params.as_ref().unwrap();
                    definition_result = Some(params["result"].clone());
                    hold_definitions = params["hold"].as_bool().unwrap();
                    definition_errors = params["errors"]
                        .as_array()
                        .map(|codes| {
                            codes
                                .iter()
                                .map(|code| {
                                    ResponseError::new(
                                        code.as_i64().unwrap() as i32,
                                        "Configured navigation error",
                                    )
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    Ok(Value::Null)
                }
                "test/definition/release" => {
                    let count = held_definitions.len();
                    for (id, result) in held_definitions.drain(..) {
                        write_packet(
                            &mut output,
                            &Packet::Single(Message::Response(Response { id, result })),
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
                    let result = definition_errors.pop_front().map_or_else(|| Ok(definition_result.clone().unwrap_or_else(|| {
                        let row = if scenario == "navigation-cold" { 1 } else { 0 };
                        let column = if scenario == "navigation-cold" { 2 } else { 0 };
                        json!({"uri":request.params.as_ref().unwrap()["textDocument"]["uri"],"range":{"start":{"line":row,"character":column},"end":{"line":row,"character":4}}})
                    })), Err);
                    if hold_definitions {
                        held_definitions.push((id, result));
                        continue;
                    }
                    result
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

fn dispatch_fixture() -> bool {
    let arguments: Vec<String> = std::env::args().collect();
    match arguments.get(1).map(String::as_str) {
        Some("--stdio") => mock_server("native"),
        Some("--mock-server") => {
            mock_server(arguments.get(2).map(String::as_str).unwrap_or("basic"))
        }
        Some("--hold-pipes") => thread::sleep(Duration::from_millis(750)),
        _ => return false,
    }
    true
}
