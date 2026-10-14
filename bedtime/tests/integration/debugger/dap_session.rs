//! Real subprocess coverage against the lightweight deterministic DAP fixture.
use bedtime_debugger::{
    DebugEvent, DebugSession, EvaluateContext, LaunchConfig, SessionState, SourceBreakpoint,
};
use std::{
    thread,
    time::{Duration, Instant},
};

fn session(scenario: &str) -> DebugSession {
    DebugSession::launch_with_args(
        &crate::fixtures::executable(),
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

#[test]
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

#[test]
fn adapter_crash_is_reported() {
    let mut session = session("crash");
    let events = poll_until(&mut session, |s, _| s.state == SessionState::Failed);
    assert!(events.iter().any(|e| matches!(e, DebugEvent::Error(_))));
}

#[test]
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

#[test]
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

#[test]
fn handshake_terminal_breakpoint_inspection_and_exit() {
    complete_session();
}

#[test]
fn superseded_breakpoint_responses() {
    superseded_breakpoint_responses_never_replace_latest_locations();
}

#[test]
fn variable_expansion_deduplicates() {
    variable_expansion_requests_are_deduplicated();
}

#[test]
fn scope_expansion_is_lazy() {
    globals_and_registers_load_only_after_expansion();
}

#[test]
fn frame_change_clears_variable_errors() {
    variable_errors_are_cleared_when_the_frame_changes();
}

#[test]
fn variable_failure_requires_retry() {
    failed_variables_require_explicit_retry("variable-error");
}

#[test]
fn variable_timeout_requires_retry() {
    failed_variables_require_explicit_retry("variable-timeout");
}
