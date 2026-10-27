//! One nonblocking stdio owner. No detached pipe threads; idle I/O waits are at
//! most 100 ms even when a descendant holds a pipe or stdin is blocked.
use crate::{Launch, MAX_FRAME_BYTES};
use serde_json::Value;
use std::{
    collections::VecDeque,
    io::{self, Read, Write},
    os::fd::{AsRawFd, RawFd},
    os::unix::{net::UnixStream, process::CommandExt},
    process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio},
    time::{Duration, Instant},
};

const IO_BUDGET: usize = 256 * 1024;
const MAX_HEADER: usize = 64 * 1024;
pub(crate) const MAX_WRITE_BYTES: usize = 32 * 1024 * 1024;

pub(crate) struct Transport {
    child: Child,
    input: ChildStdin,
    output: ChildStdout,
    error: ChildStderr,
    wake: UnixStream,
    received: Vec<u8>,
    writes: VecDeque<Vec<u8>>,
    written: usize,
    queued: usize,
    pub stderr: VecDeque<u8>,
    pub stderr_revision: u64,
}
fn nonblocking(fd: RawFd) -> Result<(), String> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error().to_string());
    }
    Ok(())
}
impl Transport {
    pub fn start(launch: &Launch, wake: UnixStream) -> Result<Self, String> {
        launch.config.validate()?;
        if !launch.root.is_absolute() {
            return Err("Language server root must be absolute".into());
        }
        let mut program = std::path::PathBuf::from(&launch.config.program);
        // GUI launch environments often omit Cargo's bin directory. Preserve
        // rustup's dispatch filename; canonicalizing this proxy changes behavior.
        if launch.config.program == "rust-analyzer" {
            let path = launch
                .config
                .environment
                .get("PATH")
                .cloned()
                .or_else(|| std::env::var("PATH").ok())
                .unwrap_or_default();
            if !std::env::split_paths(&path).any(|p| p.join(&program).is_file()) {
                if let Some(home) = std::env::var_os("HOME") {
                    let candidate = std::path::PathBuf::from(home).join(".cargo/bin/rust-analyzer");
                    if candidate.is_file() {
                        program = candidate;
                    }
                }
            }
        }
        let mut child = Command::new(program)
            .args(&launch.config.arguments)
            .envs(&launch.config.environment)
            .current_dir(&launch.root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()
            .map_err(|e| format!("{}: {e}", launch.config.program))?;
        let input = child.stdin.take().unwrap();
        let output = child.stdout.take().unwrap();
        let error = child.stderr.take().unwrap();
        let transport = Self {
            child,
            input,
            output,
            error,
            wake,
            received: Vec::new(),
            writes: VecDeque::new(),
            written: 0,
            queued: 0,
            stderr: VecDeque::new(),
            stderr_revision: 0,
        };
        for fd in [
            transport.input.as_raw_fd(),
            transport.output.as_raw_fd(),
            transport.error.as_raw_fd(),
            transport.wake.as_raw_fd(),
        ] {
            nonblocking(fd)?;
        }
        Ok(transport)
    }
    pub fn send(&mut self, value: Value) -> Result<(), String> {
        let body = serde_json::to_vec(&value).map_err(|e| e.to_string())?;
        if body.len() > MAX_FRAME_BYTES {
            return Err("Encoded LSP message exceeds 16 MiB".into());
        }
        let mut frame = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
        frame.extend_from_slice(&body);
        if self.queued + frame.len() > MAX_WRITE_BYTES {
            return Err("Language server write backlog exceeds 32 MiB".into());
        }
        self.queued += frame.len();
        self.writes.push_back(frame);
        Ok(())
    }
    pub fn has_writes(&self) -> bool {
        !self.writes.is_empty()
    }
    pub fn process_id(&self) -> u32 {
        self.child.id()
    }
    pub fn exited(&self) -> Result<bool, String> {
        // Keep the leader's PID reserved until Drop has stopped the group.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        if unsafe {
            libc::waitid(
                libc::P_PID,
                self.child.id(),
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        } < 0
        {
            return Err(io::Error::last_os_error().to_string());
        }
        Ok(unsafe { info.si_pid() } != 0)
    }
    pub fn pump(&mut self, timeout: Duration) -> Result<Vec<Value>, String> {
        let mut messages = Vec::new();
        while messages.len() < 64 {
            let Some(value) = decode(&mut self.received)? else {
                break;
            };
            messages.push(value);
        }
        let mut polls = [
            libc::pollfd {
                fd: self.wake.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: self.output.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: self.error.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: self.input.as_raw_fd(),
                events: if self.has_writes() { libc::POLLOUT } else { 0 },
                revents: 0,
            },
        ];
        let wait = if !messages.is_empty() {
            0
        } else {
            timeout.as_millis().min(100) as i32
        };
        let result = unsafe { libc::poll(polls.as_mut_ptr(), polls.len() as _, wait) };
        if result < 0 && io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            return Err(io::Error::last_os_error().to_string());
        }
        let mut bytes = [0; 8192];
        // Drain wake bytes and stderr with bounded work even for a noisy server.
        for _ in 0..32 {
            match self.wake.read(&mut bytes) {
                Ok(0) => break,
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.to_string()),
            }
        }
        for _ in 0..32 {
            match self.error.read(&mut bytes) {
                Ok(0) => break,
                Ok(n) => {
                    self.stderr.extend(&bytes[..n]);
                    self.stderr_revision = self.stderr_revision.wrapping_add(1);
                    while self.stderr.len() > 64 * 1024 {
                        self.stderr.pop_front();
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.to_string()),
            }
        }
        let mut used = 0;
        while let Some(frame) = self.writes.front() {
            let end = frame.len().min(self.written + IO_BUDGET - used);
            match self.input.write(&frame[self.written..end]) {
                Ok(0) => return Err("Language server stdin closed".into()),
                Ok(n) => {
                    self.written += n;
                    used += n;
                    if self.written == frame.len() {
                        self.queued -= frame.len();
                        self.writes.pop_front();
                        self.written = 0;
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.to_string()),
            }
            if used >= IO_BUDGET {
                break;
            }
        }
        // Decode after each read so several valid frames cannot overflow the
        // single-frame bound merely because they arrived together.
        for _ in 0..32 {
            match self.output.read(&mut bytes) {
                Ok(0) => break,
                Ok(n) => self.received.extend_from_slice(&bytes[..n]),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.to_string()),
            }
            while messages.len() < 64 {
                let Some(value) = decode(&mut self.received)? else {
                    break;
                };
                messages.push(value);
            }
            if messages.len() >= 64 {
                break;
            }
        }
        while messages.len() < 64 {
            let Some(value) = decode(&mut self.received)? else {
                break;
            };
            messages.push(value);
        }
        Ok(messages)
    }
    pub fn stop(&mut self) {
        unsafe {
            libc::kill(-(self.child.id() as i32), libc::SIGTERM);
        }
        let deadline = Instant::now() + Duration::from_millis(250);
        while Instant::now() < deadline {
            if self.exited().unwrap_or(true) {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
impl Drop for Transport {
    fn drop(&mut self) {
        unsafe {
            libc::kill(-(self.child.id() as i32), libc::SIGKILL);
        }
        let _ = self.child.wait();
    }
}
fn decode(bytes: &mut Vec<u8>) -> Result<Option<Value>, String> {
    let Some(end) = bytes.windows(4).position(|b| b == b"\r\n\r\n") else {
        if bytes.len() > MAX_HEADER {
            return Err("LSP header exceeds 64 KiB".into());
        }
        return Ok(None);
    };
    if end > MAX_HEADER {
        return Err("LSP header exceeds 64 KiB".into());
    }
    let header = std::str::from_utf8(&bytes[..end]).map_err(|_| "Invalid LSP header")?;
    let mut length = None;
    for line in header.split("\r\n") {
        let (key, value) = line.split_once(':').ok_or("Invalid LSP header field")?;
        if key.eq_ignore_ascii_case("Content-Length") {
            if length.is_some() {
                return Err("Duplicate Content-Length".into());
            }
            let n = value.trim();
            if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) {
                return Err("Invalid Content-Length".into());
            }
            length = Some(n.parse::<usize>().map_err(|_| "Invalid Content-Length")?);
        }
        if key.eq_ignore_ascii_case("Content-Type") {
            let mut parts = value.trim().split(';').map(str::trim);
            if parts.next() != Some("application/vscode-jsonrpc") {
                return Err("Unsupported LSP content type".into());
            }
            for parameter in parts {
                if let Some(charset) = parameter.strip_prefix("charset=") {
                    if charset != "utf-8" && charset != "utf8" {
                        return Err("Unsupported LSP charset".into());
                    }
                }
            }
        }
    }
    let length = length.ok_or("Missing Content-Length")?;
    if length > MAX_FRAME_BYTES {
        return Err("LSP frame exceeds 16 MiB".into());
    }
    if bytes.len() < end + 4 + length {
        return Ok(None);
    }
    let value: Value = serde_json::from_slice(&bytes[end + 4..end + 4 + length])
        .map_err(|e| format!("Invalid LSP JSON: {e}"))?;
    bytes.drain(..end + 4 + length);
    let envelope = |v: &Value| v.is_object() && v["jsonrpc"] == "2.0";
    if !envelope(&value)
        && !value.as_array().is_some_and(|items| {
            !items.is_empty() && items.len() <= 256 && items.iter().all(envelope)
        })
    {
        return Err("Invalid LSP envelope".into());
    }
    Ok(Some(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fragmented_frames_preserve_utf8_and_enforce_bounds() {
        let body = br#"{"jsonrpc":"2.0","method":"x","params":"hello"}"#;
        let frame = [
            format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes(),
            body,
        ]
        .concat();
        for split in 0..frame.len() {
            let mut bytes = frame[..split].to_vec();
            assert!(decode(&mut bytes).unwrap().is_none());
            bytes.extend_from_slice(&frame[split..]);
            assert_eq!(decode(&mut bytes).unwrap().unwrap()["method"], "x");
            assert!(bytes.is_empty());
        }
        for header in [
            "Content-Length: -1",
            "Content-Length: 16777217",
            "Content-Length: 1\r\nContent-Length: 1",
        ] {
            assert!(decode(&mut format!("{header}\r\n\r\n").into_bytes()).is_err());
        }
    }
}
