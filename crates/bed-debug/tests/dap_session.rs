//! An in-process fixture executable doubles as a deterministic DAP adapter.
use bed_debug::transport::{encode_frame, read_frame};
use bed_debug::{
    DebugEvent, DebugSession, EvaluateContext, LaunchConfig, SessionState, SourceBreakpoint,
};
use serde_json::{Value, json};
use std::{
    io::{self, BufReader, Write},
    thread,
    time::{Duration, Instant},
};

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
fn adapter(scenario: &str) {
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

fn session(scenario: &str) -> DebugSession {
    DebugSession::launch_with_args(
        &std::env::current_exe().unwrap(),
        &["--mock-adapter".into(), scenario.into()],
        LaunchConfig {
            program: "/unused/program".into(),
            cwd: std::env::temp_dir().to_string_lossy().into(),
            breakpoints: [(
                "/tmp/test.cpp".into(),
                vec![SourceBreakpoint {
                    line: 3,
                    ..Default::default()
                }],
            )]
            .into(),
            ..Default::default()
        },
    )
    .unwrap()
}
fn poll_until(
    session: &mut DebugSession,
    mut done: impl FnMut(&DebugSession, &[DebugEvent]) -> bool,
) -> Vec<DebugEvent> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut all = Vec::new();
    loop {
        let events = session.poll();
        for event in &events {
            if let DebugEvent::RunInTerminal(request) = event {
                assert_eq!(request.args[1], "space arg");
                assert_eq!(request.args[2], "$literal");
                assert_eq!(request.env["DROP"], None);
                session
                    .respond_run_in_terminal(request.request_seq, Ok(12345))
                    .unwrap();
            }
        }
        let complete = done(session, &events);
        all.extend(events);
        if complete {
            return all;
        }
        assert!(
            Instant::now() < deadline,
            "adapter timeout: {:?} {}",
            session.state,
            session.adapter_stderr()
        );
        thread::sleep(Duration::from_millis(2));
    }
}
fn complete_session() {
    let mut session = session("basic");
    let events = poll_until(&mut session, |s, _| s.variables.contains_key(&20));
    assert_eq!(session.state, SessionState::Stopped);
    assert_eq!(session.frames[0].source.as_deref(), Some("/tmp/test.cpp"));
    assert_eq!(session.frames[0].line, 3);
    assert_eq!(session.variables[&20][0].value, "42");
    assert!(events.iter().any(
        |e| matches!(e, DebugEvent::Breakpoints { breakpoints,.. } if breakpoints[0].verified)
    ));
    let request = session
        .evaluate("count", EvaluateContext::Watch, None)
        .unwrap();
    let events = poll_until(&mut session, |_, events| {
        events
            .iter()
            .any(|e| matches!(e, DebugEvent::Evaluated {request_id,..} if *request_id==request))
    });
    assert!(
        events.iter().any(
            |e| matches!(e,DebugEvent::Evaluated{result:Ok(value),..} if value.result=="42 λ")
        )
    );
    session.continue_execution().unwrap();
    assert!(session.frames.is_empty());
    poll_until(&mut session, |s, _| s.state == SessionState::Terminated);
    assert_eq!(session.exit_code, Some(0));
}
fn stale_evaluation_is_rejected() {
    let mut session = session("stale");
    poll_until(&mut session, |s, _| s.variables.contains_key(&20));
    let request = session
        .evaluate("count", EvaluateContext::Watch, None)
        .unwrap();
    session.continue_execution().unwrap();
    let events = poll_until(&mut session, |_, events| {
        events
            .iter()
            .any(|e| matches!(e,DebugEvent::Evaluated {request_id,..} if *request_id==request))
    });
    assert!(
        events
            .iter()
            .any(|e| matches!(e, DebugEvent::Evaluated { result: Err(_), .. }))
    );
    assert!(session.variables.is_empty());
    session.disconnect().unwrap();
    poll_until(&mut session, |s, _| s.state == SessionState::Terminated);
}
fn adapter_crash_is_reported() {
    let mut session = session("crash");
    let events = poll_until(&mut session, |s, _| s.state == SessionState::Failed);
    assert!(events.iter().any(|e| matches!(e, DebugEvent::Error(_))));
}
fn drop_silent_adapter_is_bounded() {
    let started = Instant::now();
    drop(session("basic"));
    assert!(started.elapsed() < Duration::from_secs(2));
}
fn superseded_breakpoint_responses_never_replace_latest_locations() {
    let mut session = session("breakpoint-order");
    poll_until(&mut session, |s, _| s.variables.contains_key(&20));
    let old = session
        .set_breakpoints(
            "/tmp/test.cpp",
            vec![SourceBreakpoint {
                line: 4,
                ..Default::default()
            }],
        )
        .unwrap();
    let latest = session
        .set_breakpoints(
            "/tmp/test.cpp",
            vec![SourceBreakpoint {
                line: 5,
                ..Default::default()
            }],
        )
        .unwrap();
    let mut events = poll_until(&mut session, |_, events| {
        events
            .iter()
            .any(|e| matches!(e,DebugEvent::Breakpoints{request_id,..} if *request_id==latest))
    });
    for _ in 0..5 {
        thread::sleep(Duration::from_millis(2));
        events.extend(session.poll());
    }
    assert!(
        !events
            .iter()
            .any(|e| matches!(e,DebugEvent::Breakpoints{request_id,..} if *request_id==old))
    );
    assert!(events.iter().any(|e| matches!(e,DebugEvent::Breakpoints{request_id,breakpoints,..} if *request_id==latest && breakpoints[0].line==Some(5))));
}
fn variable_expansion_requests_are_deduplicated() {
    let mut session = session("basic");
    poll_until(&mut session, |s, _| s.variables.contains_key(&20));
    assert_eq!(session.load_variables(20).unwrap(), 0);
    let first = session.load_variables(30).unwrap();
    assert_eq!(session.load_variables(30).unwrap(), first);
    poll_until(&mut session, |s, _| s.variables.contains_key(&30));
    assert_eq!(session.load_variables(30).unwrap(), 0);
}
fn globals_and_registers_load_only_after_expansion() {
    let mut session = session("lazy-scopes");
    poll_until(&mut session, |s, _| s.variables.contains_key(&20));
    assert_eq!(session.scopes.len(), 3);
    assert!(!session.variables.contains_key(&21));
    assert!(!session.variables.contains_key(&22));
    let request = session
        .evaluate("allow_expansion", EvaluateContext::Watch, None)
        .unwrap();
    poll_until(&mut session, |_, events| {
        events.iter().any(|event| matches!(event, DebugEvent::Evaluated {request_id,..} if *request_id == request))
    });
    session.load_variables(21).unwrap();
    session.load_variables(22).unwrap();
    poll_until(&mut session, |s, _| {
        s.variables.contains_key(&21) && s.variables.contains_key(&22)
    });
}
fn failed_variables_require_explicit_retry(scenario: &str) {
    let mut session = session(scenario);
    poll_until(&mut session, |s, _| s.variables.contains_key(&20));
    session.load_variables(30).unwrap();
    let deadline = Instant::now() + Duration::from_secs(65);
    while !session.variable_errors.contains_key(&30) {
        let events = session.poll();
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, DebugEvent::Error(_))),
            "A recoverable variable failure must not leave a global error"
        );
        assert_eq!(session.state, SessionState::Stopped);
        assert!(Instant::now() < deadline, "variable error was not recorded");
        thread::sleep(Duration::from_millis(2));
    }
    assert!(!session.variables.contains_key(&30));
    for _ in 0..3 {
        assert_eq!(session.load_variables(30).unwrap(), 0);
        session.poll();
    }
    assert!(session.retry_variables(30).unwrap() > 0);
    assert!(!session.variable_errors.contains_key(&30));
    poll_until(&mut session, |s, _| s.variables.contains_key(&30));
    assert_eq!(session.variables[&30][0].value, "42");
}
fn variable_errors_are_cleared_when_the_frame_changes() {
    let mut session = session("variable-error");
    poll_until(&mut session, |s, _| s.variables.contains_key(&20));
    session.load_variables(30).unwrap();
    poll_until(&mut session, |s, _| s.variable_errors.contains_key(&30));
    session.select_frame(11).unwrap();
    assert!(session.variable_errors.is_empty());
    poll_until(&mut session, |s, _| s.variables.contains_key(&20));
}
fn stale_variable_errors_are_discarded() {
    let mut session = session("stale-variable");
    poll_until(&mut session, |s, _| s.variables.contains_key(&20));
    session.load_variables(30).unwrap();
    session.select_frame(11).unwrap();
    let events = poll_until(&mut session, |s, _| s.variables.contains_key(&20));
    assert!(session.variable_errors.is_empty());
    assert!(!session.variables.contains_key(&30));
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, DebugEvent::Error(_)))
    );
}
fn main() {
    let arguments: Vec<_> = std::env::args().collect();
    if arguments.get(1).map(String::as_str) == Some("--mock-adapter") {
        adapter(arguments.get(2).map(String::as_str).unwrap_or("basic"));
        return;
    }
    for (name, test) in [
        (
            "handshake_terminal_breakpoint_inspection_and_exit",
            complete_session as fn(),
        ),
        ("stale_evaluation_is_rejected", stale_evaluation_is_rejected),
        ("adapter_crash_is_reported", adapter_crash_is_reported),
        (
            "drop_silent_adapter_is_bounded",
            drop_silent_adapter_is_bounded,
        ),
        (
            "superseded_breakpoint_responses",
            superseded_breakpoint_responses_never_replace_latest_locations,
        ),
        (
            "variable_expansion_deduplicates",
            variable_expansion_requests_are_deduplicated,
        ),
        (
            "scope_expansion_is_lazy",
            globals_and_registers_load_only_after_expansion,
        ),
        ("variable_failure_requires_retry", || {
            failed_variables_require_explicit_retry("variable-error")
        }),
        ("variable_timeout_requires_retry", || {
            failed_variables_require_explicit_retry("variable-timeout")
        }),
        (
            "frame_change_clears_variable_errors",
            variable_errors_are_cleared_when_the_frame_changes,
        ),
        (
            "stale_variable_errors_are_discarded",
            stale_variable_errors_are_discarded,
        ),
    ] {
        test();
        println!("test {name} ... ok");
    }
}
