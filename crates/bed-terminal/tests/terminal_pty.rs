//! Real PTY fixtures run inside this test executable (harness=false). No shell
//! installation or global test environment changes are needed for the portable
//! cases; Unix also exercises a real interactive shell and foreground SIGINT.
use bed_terminal::{
    terminal::{Terminal, TerminalEvent},
    terminal_pty::{
        MAX_QUEUED_WRITE_BYTES, MAX_WRITE_BYTES, PtyEvent, PtyOptions, TerminalPty, TerminalShell,
        WindowSize,
    },
};
use std::io;
#[cfg(unix)]
use std::process::Command;
use std::{
    fs,
    io::{IsTerminal, Read, Write},
    path::PathBuf,
};
use std::{
    process::ExitStatus,
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

fn main() {
    let arguments: Vec<String> = std::env::args().collect();
    if arguments.get(1).map(String::as_str) == Some("--pty-child") {
        child(
            arguments
                .get(2)
                .map(String::as_str)
                .unwrap_or("interactive"),
        );
        return;
    }
    let tests: &[(&str, fn())] = &[
        (
            "child_cwd_environment_tty_and_literal_argv",
            child_environment,
        ),
        ("initial_size_and_queued_resize", resize),
        ("fifo_utf8_and_control_bytes", input),
        ("main_thread_parser_replies_to_real_child", parser_replies),
        ("exit_drains_stdout_and_stderr", exit_drain),
        ("simultaneous_sessions_are_independent", simultaneous),
        ("bounded_input_queue_and_silent_teardown", bounded_input),
        ("bounded_output_queue_and_teardown", bounded_output),
        ("spawn_errors_do_not_disturb_live_session", spawn_errors),
        #[cfg(unix)]
        ("large_output_is_drained_without_loss", output_drain),
        #[cfg(unix)]
        ("foreground_sigint_and_reaping_are_local", foreground_signal),
        #[cfg(unix)]
        ("stubborn_child_is_killed_and_reaped", stubborn_teardown),
        #[cfg(unix)]
        ("real_interactive_shell_working_directory", real_shell),
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        (
            "tab_name_tracks_foreground_job_and_returns_to_shell",
            foreground_title,
        ),
    ];
    for (name, test) in tests {
        test();
        println!("test {name} ... ok");
    }
    println!("{} PTY integration groups passed", tests.len());
}

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);
struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "bed-pty-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn size(cols: u16, rows: u16) -> WindowSize {
    WindowSize {
        num_cols: cols,
        num_lines: rows,
        cell_width: 8,
        cell_height: 16,
    }
}
fn options(scenario: &str) -> PtyOptions {
    PtyOptions {
        shell: Some(TerminalShell::new(
            std::env::current_exe().unwrap().to_string_lossy(),
            vec!["--pty-child".into(), scenario.into()],
        )),
        ..PtyOptions::default()
    }
}
fn spawn(scenario: &str) -> TerminalPty {
    TerminalPty::spawn(&options(scenario), size(512, 48)).unwrap()
}
struct Capture {
    bytes: Vec<u8>,
    model: Terminal,
    exit: Option<ExitStatus>,
    exited: bool,
    parse_output: bool,
}
impl Capture {
    fn new() -> Self {
        Self {
            bytes: Vec::new(),
            model: Terminal::new(512, 48),
            exit: None,
            exited: false,
            parse_output: true,
        }
    }
    fn poll(&mut self, pty: &mut TerminalPty) {
        for event in pty.poll() {
            match event {
                PtyEvent::Output(bytes) => {
                    self.bytes.extend_from_slice(&bytes);
                    // This is the same main-thread parse/reply boundary used by
                    // the overlay. Workers never touch terminal or GUI state.
                    if self.parse_output {
                        for event in self.model.feed(&bytes) {
                            if let TerminalEvent::Write(bytes) = event {
                                pty.write(&bytes).unwrap();
                            }
                        }
                    }
                }
                PtyEvent::Exited(status) => {
                    self.exited = true;
                    self.exit = status;
                }
                PtyEvent::Error(error) => panic!("PTY worker failed: {error}"),
            }
        }
    }
    fn visible(&self) -> String {
        let mut output = String::new();
        for row in 0..self.model.rows() {
            for col in 0..self.model.cols() {
                output.push(self.model.cell(row, col).character);
            }
            output.push('\n');
        }
        output
    }
    fn contains(&self, text: &str) -> bool {
        self.bytes
            .windows(text.len())
            .any(|bytes| bytes == text.as_bytes())
            || self.visible().contains(text)
    }
    fn wait(&mut self, pty: &mut TerminalPty, text: &str) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            self.poll(pty);
            if self.contains(text) {
                return;
            }
            thread::sleep(Duration::from_millis(2));
        }
        panic!(
            "Missing {text:?}, alive={}, output_bytes={}, output_tail={:?}",
            pty.is_alive(),
            self.bytes.len(),
            String::from_utf8_lossy(&self.bytes[self.bytes.len().saturating_sub(512)..])
        );
    }
    fn wait_exit(&mut self, pty: &mut TerminalPty) -> ExitStatus {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            self.poll(pty);
            if self.exited {
                return self.exit.expect("Native child supplied an exit status");
            }
            thread::sleep(Duration::from_millis(2));
        }
        panic!("Child did not exit");
    }
}
fn ready(pty: &mut TerminalPty) -> Capture {
    let mut capture = Capture::new();
    capture.wait(pty, "PTY_READY");
    capture
}
fn bounded_shutdown(pty: &mut TerminalPty) {
    let start = Instant::now();
    pty.shutdown();
    assert!(start.elapsed() < Duration::from_millis(750));
    assert!(!pty.is_alive());
    pty.shutdown();
    assert_eq!(
        pty.write(b"x").unwrap_err().kind(),
        io::ErrorKind::NotConnected
    );
}

fn child_environment() {
    let directory = Directory::new();
    let host_cwd = std::env::current_dir().unwrap();
    let host_term = std::env::var_os("TERM");
    let host_program = std::env::var_os("TERM_PROGRAM");
    let host_custom = std::env::var_os("BED_PTY_TEST");
    #[cfg(unix)]
    let host_sigchld = sigchld_handler();
    let executable = directory.0.join(if cfg!(windows) {
        "fixture with spaces.exe"
    } else {
        "fixture with spaces"
    });
    fs::copy(std::env::current_exe().unwrap(), &executable).unwrap();
    let literal = vec![
        "one two",
        "quote\"inside",
        "final\\",
        "$(no_expand)",
        "日本語😀",
    ];
    let mut configuration = options("interactive");
    configuration.working_directory = Some(directory.0.clone());
    configuration
        .env
        .insert("BED_PTY_TEST".into(), "child value".into());
    let shell = configuration.shell.as_mut().unwrap();
    shell.program = executable.to_string_lossy().into_owned();
    shell.args.extend(literal.iter().map(|text| (*text).into()));
    let mut pty = TerminalPty::spawn(&configuration, size(512, 48)).unwrap();
    let mut capture = ready(&mut pty);
    pty.write(b"REPORT\n").unwrap();
    capture.wait(&mut pty, "REPORT_END");
    assert!(capture.contains("TTY:111"));
    assert!(capture.contains("ENV_TERM:st-256color"));
    #[cfg(unix)]
    assert!(capture.contains("ENV_PROGRAM:st-imgui"));
    #[cfg(windows)]
    assert!(capture.contains(&format!(
        "ENV_PROGRAM:{}",
        host_program.as_deref().unwrap_or_default().to_string_lossy()
    )));
    assert!(capture.contains("ENV_CUSTOM:child value"));
    assert!(capture.contains(&format!(
        "CWD:{}",
        directory.0.canonicalize().unwrap().display()
    )));
    assert!(capture.contains(&format!(
        "ARGV:{}",
        serde_json::to_string(&literal).unwrap()
    )));
    bounded_shutdown(&mut pty);
    assert_eq!(std::env::current_dir().unwrap(), host_cwd);
    assert_eq!(std::env::var_os("TERM"), host_term);
    assert_eq!(std::env::var_os("TERM_PROGRAM"), host_program);
    assert_eq!(std::env::var_os("BED_PTY_TEST"), host_custom);
    #[cfg(unix)]
    assert_eq!(sigchld_handler(), host_sigchld);
}
fn resize() {
    let mut pty = spawn("interactive");
    let mut capture = ready(&mut pty);
    pty.write(b"SIZE\n").unwrap();
    capture.wait(&mut pty, "SIZE:512,48");
    pty.resize(size(91, 37)).unwrap();
    pty.write(b"SIZE\n").unwrap();
    capture.wait(&mut pty, "SIZE:91,37");
    #[cfg(unix)]
    assert!(capture.contains("PIXELS:728,592"));
    bounded_shutdown(&mut pty);
}
fn input() {
    let mut pty = spawn("interactive");
    let mut capture = ready(&mut pty);
    for bytes in [
        b"ECHO:first\n".as_slice(),
        "ECHO:α日本😀\n".as_bytes(),
        b"ECHO:last\n",
    ] {
        pty.write(bytes).unwrap();
    }
    capture.wait(&mut pty, "ECHOED:last");
    assert!(capture.contains("ECHOED:first"));
    assert!(capture.contains("ECHOED:α日本😀"));
    #[cfg(unix)]
    {
        let text = String::from_utf8_lossy(&capture.bytes);
        assert!(text.find("ECHOED:first").unwrap() < text.find("ECHOED:α日本😀").unwrap());
        assert!(text.find("ECHOED:α日本😀").unwrap() < text.find("ECHOED:last").unwrap());
    }
    pty.write(b"HEX\n\x1b[A\x00\x7f\n").unwrap();
    capture.wait(&mut pty, "HEX:1b5b41007f0a");
    bounded_shutdown(&mut pty);
}
fn parser_replies() {
    let mut pty = spawn("interactive");
    let mut capture = ready(&mut pty);
    pty.write(b"REPLY\n").unwrap();
    capture.wait(&mut pty, "REPLY:1b5b313b3152");
    bounded_shutdown(&mut pty);
}
fn exit_drain() {
    let mut pty = spawn("interactive");
    let mut capture = ready(&mut pty);
    pty.write(b"QUIT\n").unwrap();
    let status = capture.wait_exit(&mut pty);
    assert_eq!(status.code(), Some(7));
    assert!(capture.contains("FINAL_STDOUT"));
    assert!(capture.contains("FINAL_STDERR"));
    bounded_shutdown(&mut pty);
}
fn simultaneous() {
    let mut one = spawn("interactive");
    let mut two = spawn("interactive");
    let mut capture_one = ready(&mut one);
    let mut capture_two = ready(&mut two);
    one.write(b"ECHO:one\n").unwrap();
    two.write(b"ECHO:two\n").unwrap();
    capture_one.wait(&mut one, "ECHOED:one");
    capture_two.wait(&mut two, "ECHOED:two");
    assert!(!capture_one.contains("ECHOED:two"));
    assert!(!capture_two.contains("ECHOED:one"));
    bounded_shutdown(&mut one);
    two.write(b"ECHO:still alive\n").unwrap();
    capture_two.wait(&mut two, "ECHOED:still alive");
    bounded_shutdown(&mut two);
}
fn bounded_input() {
    let mut pty = spawn("silent");
    let _capture = ready(&mut pty);
    assert_eq!(
        pty.write(&vec![b'x'; MAX_WRITE_BYTES + 1])
            .unwrap_err()
            .kind(),
        io::ErrorKind::InvalidInput
    );
    for _ in 0..MAX_QUEUED_WRITE_BYTES / MAX_WRITE_BYTES {
        pty.write(&vec![b'x'; MAX_WRITE_BYTES]).unwrap();
    }
    assert_eq!(
        pty.write(b"extra").unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
    bounded_shutdown(&mut pty);
}
fn bounded_output() {
    let mut pty = spawn("interactive");
    let _capture = ready(&mut pty);
    pty.write(b"FLOOD\n").unwrap();
    // Leave output unconsumed until the 1 MiB queue and OS pipe apply backpressure.
    thread::sleep(Duration::from_millis(200));
    bounded_shutdown(&mut pty);
}
fn spawn_errors() {
    let mut existing = spawn("interactive");
    let mut capture = ready(&mut existing);
    let directory = Directory::new();
    let missing = PtyOptions {
        shell: Some(TerminalShell::new(
            directory.0.join("missing").to_string_lossy(),
            Vec::new(),
        )),
        ..PtyOptions::default()
    };
    assert!(TerminalPty::spawn(&missing, size(80, 24)).is_err());
    let mut configuration = options("interactive");
    configuration.working_directory = Some(directory.0.join("missing-directory"));
    assert!(TerminalPty::spawn(&configuration, size(80, 24)).is_err());
    assert!(TerminalPty::spawn(&options("interactive"), size(0, 24)).is_err());
    assert!(existing.resize(size(u16::MAX, 24)).is_err());
    existing.write(b"ECHO:survived\n").unwrap();
    capture.wait(&mut existing, "ECHOED:survived");
    bounded_shutdown(&mut existing);
}
#[cfg(unix)]
fn output_drain() {
    let mut pty = spawn("interactive");
    let mut capture = ready(&mut pty);
    capture.bytes.clear();
    // This checks byte transport and output backpressure. Parser correctness is
    // exercised separately with real replies; avoid timing 2 MiB of glyph work.
    capture.parse_output = false;
    pty.write(b"FLOOD\n").unwrap();
    capture.wait(&mut pty, "FLOOD_END");
    assert_eq!(
        capture.bytes.iter().filter(|byte| **byte == b'X').count(),
        2 * 1024 * 1024
    );
    bounded_shutdown(&mut pty);
}
#[cfg(unix)]
fn foreground_signal() {
    use std::os::unix::process::ExitStatusExt;
    let before = sigchld_handler();
    let mut unrelated = Command::new("/bin/sh")
        .args(["-c", "exit 9"])
        .spawn()
        .unwrap();
    let mut pty = spawn("interactive");
    let pid = pty.process_id();
    let mut capture = ready(&mut pty);
    pty.write(&[3]).unwrap();
    assert_eq!(capture.wait_exit(&mut pty).signal(), Some(libc::SIGINT));
    bounded_shutdown(&mut pty);
    assert_reaped(pid);
    assert_eq!(unrelated.wait().unwrap().code(), Some(9));
    assert_eq!(sigchld_handler(), before);
}
#[cfg(unix)]
fn stubborn_teardown() {
    let mut pty = spawn("stubborn");
    let _capture = ready(&mut pty);
    let pid = pty.process_id();
    bounded_shutdown(&mut pty);
    assert_reaped(pid);
}
#[cfg(unix)]
fn real_shell() {
    let directory = Directory::new();
    let configuration = PtyOptions {
        working_directory: Some(directory.0.clone()),
        shell: Some(TerminalShell::new("/bin/sh", vec!["-i".into()])),
        ..PtyOptions::default()
    };
    let mut pty = TerminalPty::spawn(&configuration, size(512, 48)).unwrap();
    let mut capture = Capture::new();
    pty.write(
        b"printf '\\033[31mREAL_SHELL_READY\\033[0m\\n'; pwd; printf 'TERM:%s\\n' \"$TERM\"\n",
    )
    .unwrap();
    capture.wait(&mut pty, "TERM:st-256color");
    assert!(capture.contains("REAL_SHELL_READY"));
    assert!(capture.contains(&directory.0.canonicalize().unwrap().to_string_lossy()));
    pty.write(b"exit 0\n").unwrap();
    assert!(capture.wait_exit(&mut pty).success());
    bounded_shutdown(&mut pty);
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn foreground_title() {
    let configuration = PtyOptions {
        shell: Some(TerminalShell::new(
            "/bin/bash",
            vec!["--noprofile".into(), "--norc".into(), "-i".into()],
        )),
        ..PtyOptions::default()
    };
    let mut pty = TerminalPty::spawn(&configuration, size(80, 24)).unwrap();
    let mut capture = Capture::new();
    assert_eq!(pty.shell_name(), "bash");
    let wait_title = |pty: &mut TerminalPty, capture: &mut Capture, expected: &str| {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            capture.poll(pty);
            if pty.foreground_process_name().as_deref() == Some(expected) {
                return;
            }
            thread::sleep(Duration::from_millis(5));
        }
        panic!(
            "Expected foreground {expected:?}, got {:?}",
            pty.foreground_process_name()
        );
    };
    wait_title(&mut pty, &mut capture, "bash");
    pty.write(b"/bin/sleep 10\n").unwrap();
    wait_title(&mut pty, &mut capture, "sleep");
    pty.write(&[3]).unwrap();
    wait_title(&mut pty, &mut capture, "bash");
    pty.write(b"exit 0\n").unwrap();
    assert!(capture.wait_exit(&mut pty).success());
    bounded_shutdown(&mut pty);
}

fn child(scenario: &str) {
    raw_child_stdio();
    #[cfg(unix)]
    if scenario == "stubborn" {
        unsafe {
            libc::signal(libc::SIGHUP, libc::SIG_IGN);
            libc::signal(libc::SIGTERM, libc::SIG_IGN);
        }
    }
    println!("PTY_READY");
    io::stdout().flush().unwrap();
    if scenario == "silent" || scenario == "stubborn" {
        thread::sleep(Duration::from_secs(10));
        return;
    }
    loop {
        let line = read_until(b'\n');
        let command = String::from_utf8_lossy(&line);
        match command.trim_end_matches('\n') {
            "REPORT" => {
                println!(
                    "TTY:{}{}{}",
                    u8::from(io::stdin().is_terminal()),
                    u8::from(io::stdout().is_terminal()),
                    u8::from(io::stderr().is_terminal())
                );
                println!("ENV_TERM:{}", std::env::var("TERM").unwrap());
                println!(
                    "ENV_PROGRAM:{}",
                    std::env::var("TERM_PROGRAM").unwrap_or_default()
                );
                println!(
                    "ENV_CUSTOM:{}",
                    std::env::var("BED_PTY_TEST").unwrap_or_default()
                );
                println!(
                    "CWD:{}",
                    std::env::current_dir()
                        .unwrap()
                        .canonicalize()
                        .unwrap()
                        .display()
                );
                println!(
                    "ARGV:{}",
                    serde_json::to_string(&std::env::args().skip(3).collect::<Vec<_>>()).unwrap()
                );
                println!("REPORT_END");
            }
            "SIZE" => child_size(),
            "HEX" => println!("HEX:{}", hexadecimal(&read_until(b'\n'))),
            "REPLY" => {
                print!("\x1b[2J\x1b[H\x1b[6n");
                io::stdout().flush().unwrap();
                println!("REPLY:{}", hexadecimal(&read_until(b'R')));
            }
            "FLOOD" => {
                let bytes = vec![b'X'; 2 * 1024 * 1024];
                io::stdout().write_all(&bytes).unwrap();
                println!("\nFLOOD_END");
            }
            "QUIT" => {
                println!("FINAL_STDOUT");
                eprintln!("FINAL_STDERR");
                io::stdout().flush().unwrap();
                io::stderr().flush().unwrap();
                std::process::exit(7);
            }
            value if value.starts_with("ECHO:") => println!("ECHOED:{}", &value[5..]),
            value => panic!("Unexpected PTY command {value:?}"),
        }
        io::stdout().flush().unwrap();
    }
}
fn read_until(last: u8) -> Vec<u8> {
    let mut bytes = Vec::new();
    loop {
        let mut byte = [0];
        io::stdin().read_exact(&mut byte).unwrap();
        bytes.push(byte[0]);
        if byte[0] == last {
            return bytes;
        }
        assert!(
            bytes.len() < 4096,
            "PTY input protocol exceeded fixture limit"
        );
    }
}
fn hexadecimal(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
#[cfg(unix)]
fn raw_child_stdio() {
    let mut attributes = std::mem::MaybeUninit::<libc::termios>::uninit();
    assert_eq!(unsafe { libc::tcgetattr(0, attributes.as_mut_ptr()) }, 0);
    let mut attributes = unsafe { attributes.assume_init() };
    unsafe { libc::cfmakeraw(&mut attributes) };
    attributes.c_lflag |= libc::ISIG;
    assert_eq!(unsafe { libc::tcsetattr(0, libc::TCSANOW, &attributes) }, 0);
}
#[cfg(unix)]
fn child_size() {
    let mut size = std::mem::MaybeUninit::<libc::winsize>::uninit();
    assert_eq!(
        unsafe { libc::ioctl(0, libc::TIOCGWINSZ, size.as_mut_ptr()) },
        0
    );
    let size = unsafe { size.assume_init() };
    println!("SIZE:{},{}", size.ws_col, size.ws_row);
    println!("PIXELS:{},{}", size.ws_xpixel, size.ws_ypixel);
}
#[cfg(unix)]
fn sigchld_handler() -> (usize, i32) {
    let mut action = std::mem::MaybeUninit::<libc::sigaction>::uninit();
    assert_eq!(
        unsafe { libc::sigaction(libc::SIGCHLD, std::ptr::null(), action.as_mut_ptr()) },
        0
    );
    let action = unsafe { action.assume_init() };
    (action.sa_sigaction, action.sa_flags)
}
#[cfg(unix)]
fn assert_reaped(pid: u32) {
    let mut status = 0;
    assert_eq!(
        unsafe { libc::waitpid(pid as i32, &mut status, libc::WNOHANG) },
        -1
    );
    assert_eq!(
        io::Error::last_os_error().raw_os_error(),
        Some(libc::ECHILD)
    );
}
#[cfg(windows)]
fn raw_child_stdio() {
    use windows_sys::Win32::System::Console::*;
    let input = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    let output = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    let mut mode = 0;
    assert_ne!(unsafe { GetConsoleMode(input, &mut mode) }, 0);
    mode &= !(ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT | ENABLE_PROCESSED_INPUT);
    mode |= ENABLE_VIRTUAL_TERMINAL_INPUT;
    assert_ne!(unsafe { SetConsoleMode(input, mode) }, 0);
    assert_ne!(unsafe { GetConsoleMode(output, &mut mode) }, 0);
    assert_ne!(
        unsafe { SetConsoleMode(output, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING) },
        0
    );
}
#[cfg(windows)]
fn child_size() {
    use windows_sys::Win32::System::Console::*;
    let output = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    let mut info = std::mem::MaybeUninit::<CONSOLE_SCREEN_BUFFER_INFO>::uninit();
    assert_ne!(
        unsafe { GetConsoleScreenBufferInfo(output, info.as_mut_ptr()) },
        0
    );
    let info = unsafe { info.assume_init() };
    println!("SIZE:{},{}", info.dwSize.X, info.dwSize.Y);
}
