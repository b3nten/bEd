//! Opt-in real LLDB coverage. CI installs an adapter and runs this target with
//! `--ignored`; ordinary tests remain independent of debugger permissions.
#![cfg(any(target_os = "macos", target_os = "linux"))]

use bed_debug::{
    DebugEvent, DebugSession, EvaluateContext, EvaluateResult, LaunchConfig, SessionState,
    SourceBreakpoint,
};
use bed_terminal::terminal_pty::{PtyEvent, PtyOptions, TerminalPty, TerminalShell, WindowSize};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

const CPP: &str = r#"#include <iostream>
#include <string>
__attribute__((noinline)) int calculate(int input) {
    int local = input + 7;
    std::cout << "NATIVE_READY" << std::endl; // NATIVE_BREAKPOINT
    int result = local * 2;
    return result;
}
int main(int argc, char **argv) {
    if (argc > 1) std::cout << "NATIVE_ARG:" << argv[1] << std::endl;
    int result = calculate(35);
    std::cout << "NATIVE_RESULT:" << result << std::endl;
    std::string text;
    while (!std::getline(std::cin, text)) std::cin.clear();
    std::cout << "NATIVE_INPUT:" << text << std::endl;
    return 0;
}
"#;

const RUST: &str = r#"use std::io;
#[inline(never)]
fn calculate(input: i32) -> i32 {
    let local = input + 7;
    println!("NATIVE_READY");
    let result = local * 2; // NATIVE_BREAKPOINT
    result
}
fn main() {
    println!("NATIVE_ARG:{}", std::env::args().nth(1).unwrap());
    let result = calculate(35);
    println!("NATIVE_RESULT:{result}");
    let mut text = String::new();
    io::stdin().read_line(&mut text).unwrap();
    println!("NATIVE_INPUT:{}", text.trim_end());
}
"#;

const LITERAL_ARGUMENT: &str = "literal argument $(no expansion) 日本語";
const TERMINAL_INPUT: &str = "interactive terminal input";

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    directory: PathBuf,
    source: PathBuf,
    program: PathBuf,
    breakpoint_line: u32,
}
impl Fixture {
    fn compile(language: &str, source: &str) -> Self {
        let directory = std::env::temp_dir().join(format!(
            "bed native debugger {} {} {}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed),
            language
        ));
        fs::create_dir_all(&directory).unwrap();
        let directory = directory.canonicalize().unwrap();
        let source_path = directory.join(if language == "cpp" {
            "main.cpp"
        } else {
            "main.rs"
        });
        let program = directory.join("debug program");
        fs::write(&source_path, source).unwrap();
        let mut command = Command::new(if language == "cpp" {
            "clang++"
        } else {
            "rustc"
        });
        if language == "cpp" {
            command.args(["-g", "-O0"]);
        } else {
            command.args([
                "--crate-name",
                "native_debug_fixture",
                "-g",
                "-C",
                "opt-level=0",
            ]);
            if cfg!(target_os = "macos") {
                command.args(["-C", "split-debuginfo=packed"]);
            }
        }
        let output = command
            .arg(&source_path)
            .arg("-o")
            .arg(&program)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "Fixture compile failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        Self {
            directory,
            source: source_path,
            program,
            breakpoint_line: source
                .lines()
                .position(|line| line.contains("// NATIVE_BREAKPOINT"))
                .unwrap() as u32
                + 1,
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

struct Harness {
    session: DebugSession,
    terminal: Option<TerminalPty>,
    terminal_exited: bool,
    terminal_requests: usize,
    output: Vec<u8>,
    log: String,
    evaluations: HashMap<u64, Result<EvaluateResult, String>>,
    breakpoint_verified: bool,
    expect_clean_exit: bool,
}
impl Harness {
    fn launch(adapter: &Path, fixture: &Fixture) -> Self {
        let config = LaunchConfig {
            program: fixture.program.to_str().unwrap().into(),
            cwd: fixture.directory.to_str().unwrap().into(),
            args: vec![LITERAL_ARGUMENT.into()],
            breakpoints: [(
                fixture.source.to_str().unwrap().into(),
                vec![SourceBreakpoint {
                    line: fixture.breakpoint_line as usize,
                    ..SourceBreakpoint::default()
                }],
            )]
            .into(),
            // Some Linux CI kernels forbid personality(ADDR_NO_RANDOMIZE).
            // The fixture does not depend on fixed instruction addresses.
            init_commands: vec!["settings set target.disable-aslr false".into()],
            ..LaunchConfig::default()
        };
        Self {
            session: DebugSession::launch(adapter, config).unwrap(),
            terminal: None,
            terminal_exited: false,
            terminal_requests: 0,
            output: Vec::new(),
            log: String::new(),
            evaluations: HashMap::new(),
            breakpoint_verified: false,
            expect_clean_exit: true,
        }
    }

    fn tick(&mut self) {
        for event in self.session.poll() {
            match event {
                DebugEvent::RunInTerminal(mut request) => {
                    assert!(self.terminal.is_none(), "Unexpected second debug terminal");
                    self.terminal_requests += 1;
                    assert!(
                        bed_debug::launcher::rewrite_terminal_request(
                            &mut request,
                            Path::new(env!("CARGO_BIN_EXE_bed-debug-launcher")),
                        )
                        .unwrap(),
                        "Native fixture must exercise the bEd launcher handshake"
                    );
                    self.log.push_str(&format!(
                        "runInTerminal args={:?} cwd={:?}\n",
                        request.args, request.cwd
                    ));
                    let mut options = PtyOptions {
                        shell: Some(TerminalShell::new(
                            &request.args[0],
                            request.args[1..].to_vec(),
                        )),
                        working_directory: (!request.cwd.is_empty())
                            .then(|| PathBuf::from(request.cwd)),
                        ..PtyOptions::default()
                    };
                    for (key, value) in request.env {
                        if let Some(value) = value {
                            options.env.insert(key, value);
                        } else {
                            options.env_remove.push(key);
                        }
                    }
                    let terminal = TerminalPty::spawn(
                        &options,
                        WindowSize {
                            num_cols: 160,
                            num_lines: 48,
                            cell_width: 8,
                            cell_height: 16,
                        },
                    )
                    .unwrap();
                    self.session
                        .respond_run_in_terminal(request.request_seq, Ok(terminal.process_id()))
                        .unwrap();
                    self.terminal = Some(terminal);
                }
                DebugEvent::Breakpoints {
                    path, breakpoints, ..
                } => {
                    self.log
                        .push_str(&format!("Breakpoints {path}: {breakpoints:?}\n"));
                    self.breakpoint_verified |=
                        breakpoints.iter().any(|breakpoint| breakpoint.verified);
                }
                DebugEvent::BreakpointChanged { breakpoint, .. } => {
                    self.log
                        .push_str(&format!("Breakpoint changed: {breakpoint:?}\n"));
                    self.breakpoint_verified |= breakpoint.verified;
                }
                DebugEvent::Evaluated { request_id, result } => {
                    self.evaluations.insert(request_id, result);
                }
                DebugEvent::Output { output, .. } => self.log.push_str(&output),
                DebugEvent::Error(error) => self.log.push_str(&format!("\nERROR: {error}\n")),
                DebugEvent::Changed => {}
            }
        }
        if let Some(terminal) = &mut self.terminal {
            for event in terminal.poll() {
                match event {
                    PtyEvent::Output(bytes) => self.output.extend(bytes),
                    PtyEvent::Exited(status) => {
                        self.log.push_str(&format!("PTY exited: {status:?}\n"));
                        self.terminal_exited = true;
                        if let Some(status) = status
                            && self.expect_clean_exit
                        {
                            assert!(
                                status.success(),
                                "Debug program exited unsuccessfully: {status}"
                            );
                        }
                    }
                    PtyEvent::Error(error) => panic!("Debug terminal failed: {error}"),
                }
            }
        }
        assert!(
            self.output.len() < 2 * 1024 * 1024,
            "Unexpected fixture output flood"
        );
    }

    fn wait(&mut self, description: &str, ready: impl Fn(&Self) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            self.tick();
            if ready(self) {
                return;
            }
            assert!(
                self.session.state != SessionState::Failed && Instant::now() < deadline,
                "Waiting for {description}; state={:?}, stop={:?}, frames={:?}\n{}\n{}\n{}",
                self.session.state,
                self.session.stop_reason,
                self.session.frames,
                self.log,
                self.session.adapter_stderr(),
                String::from_utf8_lossy(&self.output),
            );
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn output_contains(&self, text: &str) -> bool {
        String::from_utf8_lossy(&self.output).contains(text)
    }

    fn evaluate(&mut self, expression: &str, context: EvaluateContext) -> EvaluateResult {
        let id = self.session.evaluate(expression, context, None).unwrap();
        self.wait("expression evaluation", |host| {
            host.evaluations.contains_key(&id)
        });
        self.evaluations.remove(&id).unwrap().unwrap()
    }
}

fn debug_fixture(adapter: &Path, fixture: &Fixture) {
    let mut host = Harness::launch(adapter, fixture);
    let adapter_pid = host.session.adapter_process_id();
    host.wait("stop on entry and initial stack", |host| {
        host.session.state == SessionState::Stopped && !host.session.frames.is_empty()
    });
    assert_eq!(
        host.terminal_requests, 1,
        "Launch must use the interactive terminal"
    );
    assert!(!host.session.threads.is_empty());
    let initial_generation = host.session.stop_generation;
    host.session.continue_execution().unwrap();
    host.wait("source breakpoint and locals", |host| {
        host.session.state == SessionState::Stopped
            && host.session.stop_generation > initial_generation
            && host.session.frames.first().is_some_and(|frame| {
                frame.source.as_deref() == fixture.source.to_str()
                    && frame.line == fixture.breakpoint_line as usize
            })
            && host
                .session
                .variables
                .values()
                .flatten()
                .any(|variable| variable.name == "local")
    });
    assert!(host.breakpoint_verified);
    assert!(host.session.frames.len() >= 2);
    assert!(host.session.frames[0].name.contains("calculate"));
    let local = host
        .session
        .variables
        .values()
        .flatten()
        .find(|variable| variable.name == "local")
        .unwrap();
    assert!(local.value.contains("42"), "Wrong local value: {local:?}");
    assert!(
        host.evaluate("local", EvaluateContext::Watch)
            .result
            .contains("42")
    );
    assert!(
        host.evaluate("local", EvaluateContext::Hover)
            .result
            .contains("42")
    );

    let outer_frame = host.session.frames[1].id;
    host.session.select_frame(outer_frame).unwrap();
    host.wait("selected caller frame scopes", |host| {
        host.session.selected_frame == Some(outer_frame) && !host.session.scopes.is_empty()
    });
    let top_frame = host.session.frames[0].id;
    host.session.select_frame(top_frame).unwrap();
    host.wait("restored top frame locals", |host| {
        host.session
            .variables
            .values()
            .flatten()
            .any(|variable| variable.name == "local")
    });

    let breakpoint_generation = host.session.stop_generation;
    host.session.step_over().unwrap();
    host.wait("step over", |host| {
        host.session.state == SessionState::Stopped
            && host.session.stop_generation > breakpoint_generation
            && host
                .session
                .frames
                .first()
                .is_some_and(|frame| frame.line != fixture.breakpoint_line as usize)
    });
    let stepped_generation = host.session.stop_generation;
    host.session.step_out().unwrap();
    host.wait("step out to caller", |host| {
        host.session.state == SessionState::Stopped
            && host.session.stop_generation > stepped_generation
            && host.session.frames.first().is_some_and(|frame| {
                frame.name.contains("main") && !frame.name.contains("calculate")
            })
    });

    host.session.continue_execution().unwrap();
    host.wait("program blocked on interactive terminal input", |host| {
        host.session.state == SessionState::Running && host.output_contains("NATIVE_RESULT:84")
    });
    let running_generation = host.session.stop_generation;
    host.session.pause().unwrap();
    host.wait("pause the running program", |host| {
        host.session.state == SessionState::Stopped
            && host.session.stop_generation > running_generation
            && !host.session.frames.is_empty()
    });
    host.terminal
        .as_ref()
        .unwrap()
        .write(format!("{TERMINAL_INPUT}\n").as_bytes())
        .unwrap();
    host.session.continue_execution().unwrap();
    host.wait("clean debuggee exit and final terminal output", |host| {
        host.session.state == SessionState::Terminated
            && host.session.exit_code == Some(0)
            && host.terminal_exited
            && host.output_contains(&format!("NATIVE_INPUT:{TERMINAL_INPUT}"))
    });
    assert!(host.output_contains(&format!("NATIVE_ARG:{LITERAL_ARGUMENT}")));
    assert!(host.output_contains("NATIVE_READY"));
    assert!(host.output_contains("NATIVE_RESULT:84"));
    assert!(
        !host.log.contains("ERROR:"),
        "Unexpected debugger errors: {}",
        host.log
    );
    assert_eq!(
        unsafe { libc::kill(adapter_pid as libc::pid_t, 0) },
        -1,
        "Adapter was not cleaned up"
    );
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH)
    );
}

fn shutdown_paused_fixture(adapter: &Path, fixture: &Fixture) {
    let mut host = Harness::launch(adapter, fixture);
    host.expect_clean_exit = false;
    let adapter_pid = host.session.adapter_process_id();
    host.wait("shutdown fixture entry stop", |host| {
        host.session.state == SessionState::Stopped && !host.session.frames.is_empty()
    });
    host.session.continue_execution().unwrap();
    host.wait("shutdown fixture source breakpoint", |host| {
        host.session.state == SessionState::Stopped
            && host.session.frames.first().is_some_and(|frame| {
                frame.source.as_deref() == fixture.source.to_str()
                    && frame.line == fixture.breakpoint_line as usize
            })
    });
    let debuggee_pid = host.terminal.as_ref().unwrap().process_id();
    host.session.shutdown();
    host.wait(
        "shutdown terminates paused debuggee before PTY teardown",
        |host| host.terminal_exited && !host.terminal.as_ref().unwrap().is_alive(),
    );
    for pid in [adapter_pid, debuggee_pid] {
        assert_eq!(
            unsafe { libc::kill(pid as libc::pid_t, 0) },
            -1,
            "Process {pid} survived shutdown"
        );
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
    }
}

#[test]
#[ignore = "requires installed LLDB 18+, C++/Rust compilers, and local debugger permission"]
fn real_lldb_launch_breakpoint_variables_and_stdin() {
    let override_path = std::env::var_os("BED_LLDB_DAP").map(PathBuf::from);
    let adapter = bed_debug::discovery::discover_adapter(override_path.as_deref()).unwrap();
    let cpp = Fixture::compile("cpp", CPP);
    debug_fixture(&adapter, &cpp);
    let rust = Fixture::compile("rust", RUST);
    debug_fixture(&adapter, &rust);
    shutdown_paused_fixture(&adapter, &cpp);
}
