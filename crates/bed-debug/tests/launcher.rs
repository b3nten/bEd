#![cfg(unix)]

use bed_debug::launcher::FLAG;
use serde_json::{Value, json};
use std::{
    ffi::CString,
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Write},
    os::unix::ffi::OsStrExt,
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!(
            "bed launcher {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).unwrap();
        Self(directory)
    }
    fn fifo(&self) -> PathBuf {
        let path = self.0.join("comm fifo");
        let c_path = CString::new(path.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
        path
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
struct Owner(Child);
impl Drop for Owner {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn launch(fifo: &std::path::Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_bed-debug-launcher"));
    command
        .args([FLAG, "--comm-file"])
        .arg(fifo)
        .arg("--debugger-pid")
        .arg(std::process::id().to_string())
        .args([
            "--launch-target",
            "/bin/sh",
            "-c",
            "printf 'launch_pid=%s\\narg=%s\\n' \"$$\" \"$1\"",
            "--",
            "literal $(no expansion) 日本語",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

#[test]
fn fifo_handshake_exec_preserves_pid_and_literal_arguments() {
    let fixture = Fixture::new();
    let fifo = fixture.fifo();
    let (sender, receiver) = mpsc::channel();
    let server_fifo = fifo.clone();
    let server = thread::spawn(move || {
        let mut line = String::new();
        let reader = fs::File::open(&server_fifo).unwrap();
        // Delay consuming the PID after opening the FIFO. The helper must not
        // steal its own PID when it reverses the communication direction.
        thread::sleep(Duration::from_millis(50));
        BufReader::new(reader).read_line(&mut line).unwrap();
        let message: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(message["kind"], "pid");
        sender.send(message["pid"].as_u64().unwrap()).unwrap();
        let mut writer = OpenOptions::new().write(true).open(server_fifo).unwrap();
        let body = format!("{}\n", json!({"kind":"didAttach"}));
        for bytes in body.as_bytes().chunks(2) {
            writer.write_all(bytes).unwrap();
        }
    });
    let mut child = Owner(launch(&fifo).spawn().unwrap());
    let pid = receiver.recv_timeout(Duration::from_secs(3)).unwrap();
    assert_eq!(pid, u64::from(child.0.id()));
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        assert!(Instant::now() < deadline, "Launcher failed to exec target");
        thread::sleep(Duration::from_millis(2));
    }
    let mut output = String::new();
    use std::io::Read;
    child
        .0
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut output)
        .unwrap();
    assert_eq!(
        output,
        format!("launch_pid={pid}\narg=literal $(no expansion) 日本語\n")
    );
    server.join().unwrap();
}

#[test]
fn rejects_regular_communication_files_without_modifying_them() {
    let fixture = Fixture::new();
    let path = fixture.0.join("not a fifo");
    fs::write(&path, "keep this data").unwrap();
    let output = launch(&path).output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("must be a FIFO"));
    assert_eq!(fs::read_to_string(path).unwrap(), "keep this data");
}

#[test]
fn waiting_launcher_can_be_terminated_without_starting_a_target() {
    let fixture = Fixture::new();
    let fifo = fixture.fifo();
    let mut child = Owner(launch(&fifo).spawn().unwrap());
    thread::sleep(Duration::from_millis(30));
    assert!(child.0.try_wait().unwrap().is_none());
    let started = Instant::now();
    child.0.kill().unwrap();
    assert!(!child.0.wait().unwrap().success());
    assert!(started.elapsed() < Duration::from_secs(1));
}
