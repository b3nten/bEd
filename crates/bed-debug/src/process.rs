//! Cancellable worker-side process execution, shared by discovery and builds.
use std::{
    io::{self, Read},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Copy)]
pub(crate) enum Stream {
    Stdout,
    Stderr,
}
enum ReadEvent {
    Bytes(Stream, Vec<u8>),
    Eof,
    Error(String),
}
struct Owner {
    child: Child,
    finished: bool,
}
impl Drop for Owner {
    fn drop(&mut self) {
        if !self.finished {
            #[cfg(unix)]
            unsafe {
                libc::kill(-(self.child.id() as i32), libc::SIGKILL);
            }
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}
pub(crate) fn run(
    mut command: Command,
    cancel: &AtomicBool,
    mut output: impl FnMut(Stream, &[u8]) -> Result<(), String>,
) -> Result<ExitStatus, String> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let child = command.spawn().map_err(|e| e.to_string())?;
    let mut owner = Owner {
        child,
        finished: false,
    };
    let stdout = owner.child.stdout.take().expect("piped stdout");
    let stderr = owner.child.stderr.take().expect("piped stderr");
    let (sender, receiver) = mpsc::sync_channel(16);
    fn reader(
        mut pipe: impl Read + Send + 'static,
        stream: Stream,
        sender: mpsc::SyncSender<ReadEvent>,
    ) -> io::Result<()> {
        thread::Builder::new()
            .name("bed-debug-build-pipe".into())
            .spawn(move || {
                let mut bytes = [0; 16 * 1024];
                loop {
                    match pipe.read(&mut bytes) {
                        Ok(0) => break,
                        Ok(n) => {
                            if sender
                                .send(ReadEvent::Bytes(stream, bytes[..n].to_vec()))
                                .is_err()
                            {
                                return;
                            }
                        }
                        Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                        Err(e) => {
                            let _ = sender.send(ReadEvent::Error(e.to_string()));
                            break;
                        }
                    }
                }
                let _ = sender.send(ReadEvent::Eof);
            })?;
        Ok(())
    }
    reader(stdout, Stream::Stdout, sender.clone()).map_err(|e| e.to_string())?;
    reader(stderr, Stream::Stderr, sender.clone()).map_err(|e| e.to_string())?;
    drop(sender);
    let mut status = None;
    let mut exited_at = None;
    let mut closed_pipes = 0;
    loop {
        if cancel.load(Ordering::Acquire) {
            return Err("Build cancelled".into());
        }
        match receiver.recv_timeout(Duration::from_millis(10)) {
            Ok(ReadEvent::Bytes(stream, bytes)) => output(stream, &bytes)?,
            Ok(ReadEvent::Eof) => closed_pipes += 1,
            Ok(ReadEvent::Error(error)) => return Err(error),
            Err(mpsc::RecvTimeoutError::Timeout | mpsc::RecvTimeoutError::Disconnected) => {}
        }
        if status.is_none() {
            status = owner.child.try_wait().map_err(|e| e.to_string())?;
            if status.is_some() {
                exited_at = Some(Instant::now());
            }
        }
        if let Some(status) = status {
            if closed_pipes == 2 {
                owner.finished = true;
                return Ok(status);
            }
            if exited_at.is_some_and(|t| t.elapsed() > Duration::from_secs(1)) {
                // A background descendant inherited a pipe. Its output must not
                // hold the UI's build state forever; Owner terminates that group.
                return Ok(status);
            }
        }
    }
}

pub(crate) fn child_path() -> Option<std::ffi::OsString> {
    let mut paths = Vec::new();
    if let Some(cargo) = std::env::var_os("CARGO_HOME") {
        paths.push(std::path::PathBuf::from(cargo).join("bin"));
    } else if let Some(home) = std::env::var_os("HOME") {
        paths.push(std::path::PathBuf::from(home).join(".cargo/bin"));
    }
    paths.extend(
        std::env::var_os("PATH")
            .as_deref()
            .map(std::env::split_paths)
            .into_iter()
            .flatten(),
    );
    std::env::join_paths(paths).ok()
}

pub(crate) fn log(
    sender: &mpsc::SyncSender<String>,
    mut message: String,
    cancel: &Arc<AtomicBool>,
) {
    loop {
        match sender.try_send(message) {
            Ok(()) | Err(mpsc::TrySendError::Disconnected(_)) => return,
            Err(mpsc::TrySendError::Full(returned)) => message = returned,
        }
        if cancel.load(Ordering::Acquire) {
            return;
        }
        thread::sleep(Duration::from_millis(5));
    }
}
