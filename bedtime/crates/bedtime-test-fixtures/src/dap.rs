use bedtime_debugger::transport::{encode_frame, read_frame};
use serde_json::{Value, json};
use std::io::{self, BufReader, Write};
fn send(value: Value) {
    let bytes = encode_frame(&value).unwrap();
    // Deliberately fragment every header, UTF-8 body, and frame boundary.
    let mut stdout = io::stdout().lock();
    for chunk in bytes.chunks(3) {
        stdout.write_all(chunk).unwrap();
    }
    stdout.flush().unwrap();
}
fn response(request: &Value, body: Value) {
    send(
        json!({"seq":100,"type":"response","request_seq":request["seq"],"command":request["command"],"success":true,"body":body}),
    );
}
pub fn adapter(scenario: &str) {
    let mut input = BufReader::new(io::stdin());
    let mut launch = None;
    let mut configured = false;
    let mut delayed: Option<Value> = None;
    let mut delayed_breakpoint = None;
    let mut expansion_allowed = false;
    let mut variable_attempts = 0;
    while let Ok(request) = read_frame(&mut input) {
        if request["type"] == "response" {
            assert_eq!(request["command"], "runInTerminal");
            assert_eq!(request["body"]["processId"], 12345);
            continue;
        }
        match request["command"].as_str().unwrap() {
            "initialize" => {
                assert_eq!(request["arguments"]["supportsRunInTerminalRequest"], true);
                response(
                    &request,
                    json!({"supportsConfigurationDoneRequest":true,"supportsRestartRequest":true}),
                );
            }
            "launch" => {
                if scenario == "crash" {
                    return;
                }
                assert_eq!(request["arguments"]["runInTerminal"], true);
                launch = Some(request);
                send(
                    json!({"seq":91,"type":"request","command":"runInTerminal","arguments":{"args":["/literal/path","space arg","$literal"],"cwd":"/tmp","env":{"DROP":null,"KEEP":"λ"},"kind":"integrated"}}),
                );
                send(json!({"seq":92,"type":"event","event":"initialized"}));
            }
            "setBreakpoints" => {
                if scenario == "breakpoint-order" && configured {
                    if let Some(previous) = delayed_breakpoint.take() {
                        response(
                            &request,
                            json!({"breakpoints":[{"verified":true,"line":5,"id":5}]}),
                        );
                        response(
                            &previous,
                            json!({"breakpoints":[{"verified":true,"line":4,"id":4}]}),
                        );
                    } else {
                        delayed_breakpoint = Some(request);
                    }
                    continue;
                }
                let lines = request["arguments"]["breakpoints"].as_array().unwrap();
                response(
                    &request,
                    json!({"breakpoints":lines.iter().map(|v| json!({"verified":true,"line":v["line"],"id":1})).collect::<Vec<_>>()}),
                );
            }
            "configurationDone" => {
                configured = true;
                response(&request, json!({}));
                response(launch.as_ref().unwrap(), json!({}));
                send(
                    json!({"seq":93,"type":"event","event":"stopped","body":{"reason":"breakpoint","threadId":1}}),
                );
            }
            "threads" => {
                assert!(configured);
                response(&request, json!({"threads":[{"id":1,"name":"main λ"}]}));
            }
            "stackTrace" => response(
                &request,
                json!({"stackFrames":[{"id":10,"name":"main","source":{"path":"/tmp/test.cpp"},"line":3,"column":1}],"totalFrames":1}),
            ),
            "scopes" => {
                if let Some(previous) = delayed.take() {
                    send(
                        json!({"seq":101,"type":"response","request_seq":previous["seq"],"command":"variables","success":false,"message":"Old frame failed"}),
                    );
                }
                let mut scopes =
                    vec![json!({"name":"Locals","variablesReference":20,"expensive":false})];
                if scenario == "lazy-scopes" {
                    scopes
                        .push(json!({"name":"Globals","variablesReference":21,"expensive":false}));
                    scopes.push(
                        json!({"name":"Registers","variablesReference":22,"expensive":false}),
                    );
                }
                response(&request, json!({"scopes":scopes}));
            }
            "variables" => {
                let reference = request["arguments"]["variablesReference"].as_i64().unwrap();
                if scenario == "lazy-scopes" && reference != 20 {
                    assert!(expansion_allowed, "scope loaded before expansion");
                }
                if reference == 30 {
                    variable_attempts += 1;
                    if scenario == "stale-variable" {
                        delayed = Some(request);
                        continue;
                    }
                    if variable_attempts == 1 {
                        if scenario == "variable-timeout" {
                            continue;
                        }
                        if scenario == "variable-error" {
                            send(
                                json!({"seq":101,"type":"response","request_seq":request["seq"],"command":"variables","success":false,"message":"Cannot inspect this value"}),
                            );
                            continue;
                        }
                    }
                }
                response(
                    &request,
                    json!({"variables":[{"name":"count","value":"42","type":"int","variablesReference":0}]}),
                );
            }
            "evaluate" => {
                expansion_allowed = true;
                if scenario == "stale" {
                    delayed = Some(request);
                } else {
                    response(
                        &request,
                        json!({"result":"42 λ","type":"int","variablesReference":0}),
                    );
                }
            }
            "continue" | "next" | "stepIn" | "stepOut" => {
                response(&request, json!({}));
                send(json!({"seq":94,"type":"event","event":"continued","body":{"threadId":1}}));
                if let Some(request) = delayed.take() {
                    response(&request, json!({"result":"STALE","variablesReference":0}));
                }
                if scenario != "stale" {
                    send(json!({"seq":95,"type":"event","event":"exited","body":{"exitCode":0}}));
                    send(json!({"seq":96,"type":"event","event":"terminated"}));
                }
            }
            "disconnect" => {
                response(&request, json!({}));
                return;
            }
            other => panic!("unexpected request {other}"),
        }
    }
}
