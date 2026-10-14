//! Child processes for integration tests. No desktop or application dependencies.
mod bed_remote;
mod dap;
#[cfg(unix)]
mod input;
mod lsp;
mod navigation_transport;
#[cfg(unix)]
mod pty;
mod remote_documents;
mod remote_io;
mod remote_services;

// Keep the remote-navigation assertions in their owner module. This one case
// runs with a child-only PATH containing a fake SSH executable.
#[allow(dead_code, unused_imports)]
#[path = "../../../tests/integration/documents/lsp_navigation.rs"]
mod navigation;

mod agent {
    include!("../../bedtime-remote-agent/src/main.rs");
    pub fn entry() -> std::process::ExitCode {
        main()
    }
}

// The navigation child launches another fixture, never the integration harness.
mod fixtures {
    pub fn executable() -> std::path::PathBuf {
        std::env::current_exe().unwrap()
    }
    pub fn command(mode: &str) -> std::process::Command {
        let mut command = std::process::Command::new(executable());
        command.env("BED_TEST_FIXTURE_MODE", mode);
        command
    }
}

pub fn run() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("--mock-server") => {
            lsp::mock_server(args.get(2).map(String::as_str).unwrap_or("basic"))
        }
        Some("--hold-pipes") => std::thread::sleep(std::time::Duration::from_millis(750)),
        Some("--mock-adapter") => dap::adapter(args.get(2).map(String::as_str).unwrap_or("basic")),
        #[cfg(unix)]
        Some("--pty-child") => pty::child(args.get(2).map(String::as_str).unwrap_or("interactive")),
        #[cfg(unix)]
        Some("--input-child") => input::child(),
        Some("--stdio") => match std::env::var("BED_TEST_FIXTURE_MODE").as_deref() {
            Ok("bed-remote") => bed_remote::run(),
            Ok("remote-io") => remote_io::run(),
            Ok("remote-documents") => remote_documents::run(),
            Ok("remote-services") => remote_services::run(),
            _ => lsp::mock_server("native"),
        },
        _ => panic!("unknown test fixture arguments: {args:?}"),
    }
    std::process::ExitCode::SUCCESS
}

pub fn run_remote_agent() -> std::process::ExitCode {
    agent::entry()
}
pub fn run_debug_launcher() -> std::process::ExitCode {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(std::ffi::OsStr::new(bedtime_debugger::launcher::FLAG)) {
        eprintln!("Expected {}", bedtime_debugger::launcher::FLAG);
        return std::process::ExitCode::from(2);
    }
    match bedtime_debugger::launcher::run(args) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Debugger launcher: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

/// This fixture alone links the document session for child-only PATH validation.
pub fn run_navigation() -> std::process::ExitCode {
    match std::env::args().nth(1).as_deref() {
        #[cfg(unix)]
        Some("--remote-navigation") => navigation::unix::remote_navigation(),
        Some("--remote-transport") => navigation_transport::run(),
        other => panic!("unknown navigation fixture argument: {other:?}"),
    }
    std::process::ExitCode::SUCCESS
}
