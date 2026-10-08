//! Content-Length framing and owned stdio workers. Workers never access UI state.
use serde_json::Value;
use std::{
    io::{self, BufRead, BufReader, Read, Write},
    path::Path,
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    thread,
};

pub const MAX_HEADER_BYTES: usize = 16 * 1024;
pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_STDERR_BYTES: usize = 64 * 1024;
const MAX_QUEUE: usize = 64;
const INBOUND_CAPACITY: usize = 8;
pub const MAX_QUEUED_WRITE_BYTES: usize = 4 * 1024 * 1024;

pub fn read_frame(reader: &mut impl BufRead) -> io::Result<Value> {
    let mut total = 0;
    let mut length = None;
    loop {
        let mut line = Vec::new();
        loop {
            let bytes = reader.fill_buf()?;
            if bytes.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "Debugger closed its output",
                ));
            }
            let consumed = bytes
                .iter()
                .position(|b| *b == b'\n')
                .map_or(bytes.len(), |n| n + 1);
            total += consumed;
            if total > MAX_HEADER_BYTES {
                return Err(io::Error::other("DAP header exceeds 16 KiB"));
            }
            line.extend_from_slice(&bytes[..consumed]);
            reader.consume(consumed);
            if line.ends_with(b"\n") {
                break;
            }
        }
        if !line.ends_with(b"\r\n") {
            return Err(io::Error::other("DAP headers require CRLF"));
        }
        line.truncate(line.len() - 2);
        if line.is_empty() {
            break;
        }
        if let Some(colon) = line.iter().position(|b| *b == b':')
            && line[..colon].eq_ignore_ascii_case(b"Content-Length")
        {
            if length.is_some() {
                return Err(io::Error::other("Duplicate DAP Content-Length"));
            }
            let value = std::str::from_utf8(&line[colon + 1..])
                .map_err(io::Error::other)?
                .trim();
            if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
                return Err(io::Error::other("Invalid DAP Content-Length"));
            }
            length = Some(value.parse::<usize>().map_err(io::Error::other)?);
        }
    }
    let length = length.ok_or_else(|| io::Error::other("Missing DAP Content-Length"))?;
    if length > MAX_FRAME_BYTES {
        return Err(io::Error::other("DAP body exceeds 16 MiB"));
    }
    let mut bytes = vec![0; length];
    reader.read_exact(&mut bytes)?;
    serde_json::from_slice(&bytes).map_err(io::Error::other)
}

pub fn encode_frame(message: &Value) -> io::Result<Vec<u8>> {
    let bytes = serde_json::to_vec(message).map_err(io::Error::other)?;
    if bytes.len() > MAX_FRAME_BYTES {
        return Err(io::Error::other("DAP body exceeds 16 MiB"));
    }
    let mut frame = format!("Content-Length: {}\r\n\r\n", bytes.len()).into_bytes();
    frame.extend(bytes);
    Ok(frame)
}

#[derive(Debug)]
pub enum TransportEvent {
    Message(Value),
    Closed(String),
}
struct QueuedFrame {
    bytes: Vec<u8>,
    budget: Arc<AtomicUsize>,
}
impl Drop for QueuedFrame {
    fn drop(&mut self) {
        self.budget.fetch_sub(self.bytes.len(), Ordering::AcqRel);
    }
}

pub struct DapTransport {
    child: Child,
    outgoing: Option<SyncSender<QueuedFrame>>,
    incoming: Option<Receiver<TransportEvent>>,
    stderr: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    write_budget: Arc<AtomicUsize>,
}
impl DapTransport {
    pub fn spawn(program: &Path, arguments: &[String], cwd: Option<&Path>) -> io::Result<Self> {
        let mut command = Command::new(program);
        command
            .args(arguments)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(cwd) = cwd {
            command.current_dir(cwd);
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = command.spawn()?;
        let mut stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let mut child_stderr = child.stderr.take().expect("piped stderr");
        let (sender, incoming) = mpsc::sync_channel(INBOUND_CAPACITY);
        let (outgoing, receiver) = mpsc::sync_channel::<QueuedFrame>(MAX_QUEUE);
        let stderr = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        // Construct the owner before starting workers so partial startup failure
        // drops/reaps the child and closes both bounded queues.
        let owner = Self {
            child,
            outgoing: Some(outgoing),
            incoming: Some(incoming),
            stderr: stderr.clone(),
            write_budget: Arc::new(AtomicUsize::new(0)),
        };
        let errors = sender.clone();
        thread::Builder::new()
            .name("bed-dap-read".into())
            .spawn(move || {
                let mut reader = BufReader::new(stdout);
                loop {
                    match read_frame(&mut reader) {
                        Ok(message) => {
                            if sender.send(TransportEvent::Message(message)).is_err() {
                                break;
                            }
                        }
                        Err(error) => {
                            let _ = sender.send(TransportEvent::Closed(error.to_string()));
                            break;
                        }
                    }
                }
            })?;
        thread::Builder::new()
            .name("bed-dap-write".into())
            .spawn(move || {
                while let Ok(frame) = receiver.recv() {
                    if let Err(error) = stdin.write_all(&frame.bytes).and_then(|()| stdin.flush()) {
                        let _ = errors.send(TransportEvent::Closed(error.to_string()));
                        break;
                    }
                }
            })?;
        thread::Builder::new()
            .name("bed-dap-stderr".into())
            .spawn(move || {
                let mut bytes = [0; 4096];
                loop {
                    match child_stderr.read(&mut bytes) {
                        Ok(0) => break,
                        Ok(n) => {
                            let mut log = stderr.lock().unwrap_or_else(|p| p.into_inner());
                            let excess =
                                log.len().saturating_add(n).saturating_sub(MAX_STDERR_BYTES);
                            log.drain(..excess);
                            log.extend_from_slice(&bytes[..n]);
                        }
                        Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                        Err(_) => break,
                    }
                }
            })?;
        Ok(owner)
    }
    pub fn process_id(&self) -> u32 {
        self.child.id()
    }
    pub fn send(&self, message: &Value) -> io::Result<()> {
        let bytes = encode_frame(message)?;
        self.write_budget
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes.len())
                    .filter(|total| *total <= MAX_QUEUED_WRITE_BYTES)
            })
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "Debugger write queue exceeds 4 MiB",
                )
            })?;
        let frame = QueuedFrame {
            bytes,
            budget: self.write_budget.clone(),
        };
        self.outgoing
            .as_ref()
            .ok_or_else(|| io::Error::other("Debugger is closed"))?
            .try_send(frame)
            .map_err(|error| match error {
                TrySendError::Full(_) => {
                    io::Error::new(io::ErrorKind::WouldBlock, "Debugger write queue is full")
                }
                TrySendError::Disconnected(_) => {
                    io::Error::new(io::ErrorKind::BrokenPipe, "Debugger writer ended")
                }
            })
    }
    pub fn poll(&self) -> Vec<TransportEvent> {
        self.incoming
            .as_ref()
            .map(|r| r.try_iter().take(INBOUND_CAPACITY).collect())
            .unwrap_or_default()
    }
    pub fn stderr(&self) -> String {
        String::from_utf8_lossy(&self.stderr.lock().unwrap_or_else(|p| p.into_inner())).into_owned()
    }
    pub fn shutdown(&mut self) {
        self.outgoing.take();
        self.incoming.take();
        if self.child.try_wait().ok().flatten().is_none() {
            #[cfg(unix)]
            unsafe {
                libc::kill(-(self.child.id() as i32), libc::SIGKILL);
            }
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}
impl Drop for DapTransport {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    #[test]
    fn utf8_framing_uses_byte_length_and_handles_adjacent_frames() {
        let value = serde_json::json!({"type":"event","output":"héλlo"});
        let mut data = encode_frame(&value).unwrap();
        data.extend(encode_frame(&value).unwrap());
        let mut reader = BufReader::with_capacity(1, Cursor::new(data));
        assert_eq!(read_frame(&mut reader).unwrap(), value);
        assert_eq!(read_frame(&mut reader).unwrap(), value);
    }
    #[test]
    fn rejects_invalid_duplicate_missing_and_oversized_lengths() {
        for bytes in [
            b"\r\n".as_slice(),
            b"Content-Length: -1\r\n\r\n",
            b"Content-Length: 2\n\n{}",
            b"Content-Length: 1\r\nContent-Length: 1\r\n\r\n0",
            b"Content-Length: 16777217\r\n\r\n",
        ] {
            assert!(read_frame(&mut Cursor::new(bytes)).is_err());
        }
    }
}
