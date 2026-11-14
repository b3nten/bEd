use crate::{Error, ErrorKind, Result};
use std::{
    io::{Read, Write},
    path::Path,
    process::{Command, ExitStatus, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

pub const MAX_BYTES: usize = 16 * 1024 * 1024;
/// A patch holds both versions of what changed, each line with a marker
/// before it: a file within `MAX_BYTES` that was wholly rewritten still fits.
pub const MAX_PATCH_BYTES: usize = 4 * MAX_BYTES;
const MAX_LOG_BYTES: usize = 256 * 1024;
const TERMINATION_GRACE: Duration = Duration::from_secs(1);

pub struct Run {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl Run {
    pub fn log(&self) -> String {
        let mut bytes = self.stdout.clone();
        if !bytes.is_empty() && !self.stderr.is_empty() && !bytes.ends_with(b"\n") {
            bytes.push(b'\n');
        }
        bytes.extend_from_slice(&self.stderr);
        let truncated = bytes.len() > MAX_LOG_BYTES;
        if truncated {
            bytes.drain(..bytes.len() - MAX_LOG_BYTES);
        }
        let text = String::from_utf8_lossy(&bytes).trim().to_owned();
        if truncated {
            format!("[earlier output omitted]\n{text}")
        } else {
            text
        }
    }

    pub fn checked(self) -> Result<Self> {
        if self.status.success() {
            Ok(self)
        } else {
            Err(Error {
                kind: ErrorKind::Failed,
                message: "Git command failed".into(),
                output: self.log(),
            })
        }
    }
}

fn read_bounded(mut pipe: impl Read, limit: usize) -> std::io::Result<(Vec<u8>, bool)> {
    let mut bytes = Vec::new();
    let mut chunk = [0; 8192];
    let mut large = false;
    loop {
        let count = pipe.read(&mut chunk)?;
        if count == 0 {
            break;
        }
        let available = limit.saturating_sub(bytes.len());
        bytes.extend_from_slice(&chunk[..count.min(available)]);
        large |= count > available;
    }
    Ok((bytes, large))
}

pub fn git(
    root: &Path,
    args: &[&str],
    input: Option<&[u8]>,
    canceled: &Arc<AtomicBool>,
) -> Result<Run> {
    git_within(root, args, input, canceled, MAX_BYTES)
}

/// As `git`, with its own limit on the bytes of output kept.
pub fn git_within(
    root: &Path,
    args: &[&str],
    input: Option<&[u8]>,
    canceled: &Arc<AtomicBool>,
    limit: usize,
) -> Result<Run> {
    if canceled.load(Ordering::Acquire) {
        return Err(Error::new(ErrorKind::Canceled, "Git operation canceled"));
    }
    let mut command = Command::new("git");
    command
        .env("LC_ALL", "C")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_EDITOR", "true")
        .env("GIT_SEQUENCE_EDITOR", "true")
        .args([
            "--literal-pathspecs",
            "-c",
            "color.ui=false",
            "-c",
            "core.quotePath=true",
            "-C",
        ])
        .arg(root)
        .args(args)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Its own session: a process group to signal, and no terminal. Git,
        // ssh and gpg then fail at once when they'd ask a question (or use a
        // graphical helper), rather than wait on the terminal that started
        // the app, where nobody is looking.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    let mut child = command
        .spawn()
        .map_err(|error| Error::new(ErrorKind::Failed, format!("Could not start Git: {error}")))?;
    let output = child.stdout.take().unwrap();
    let errors = child.stderr.take().unwrap();
    let out_thread = thread::spawn(move || read_bounded(output, limit));
    let err_thread = thread::spawn(move || read_bounded(errors, MAX_BYTES));
    // A writer thread avoids blocking cancellation when a hook or Git itself
    // stops reading stdin. Terminating the group closes its pipe and releases it.
    let writer = input.map(|bytes| {
        let bytes = bytes.to_vec();
        let mut pipe = child.stdin.take().unwrap();
        thread::spawn(move || pipe.write_all(&bytes))
    });
    let started = Instant::now();
    let mut interrupted_at = None;
    let mut killed_at = None;
    let mut status = None;
    let status = loop {
        if interrupted_at.is_none()
            && (canceled.load(Ordering::Acquire) || started.elapsed() > Duration::from_secs(600))
        {
            interrupted_at = Some(Instant::now());
            // Git's signal handler removes the lock files it owns. SIGKILL
            // bypasses that cleanup, so allow Git and its hooks to exit first.
            #[cfg(unix)]
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGTERM);
            }
            #[cfg(not(unix))]
            {
                let _ = child.kill();
                killed_at = Some(Instant::now());
            }
        }
        if killed_at.is_none() && interrupted_at.is_some_and(|at| at.elapsed() >= TERMINATION_GRACE)
        {
            #[cfg(unix)]
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            if status.is_none() {
                let _ = child.kill();
            }
            killed_at = Some(Instant::now());
        }
        if status.is_none() {
            status = child.try_wait()?;
        }
        // A hook may retain Git's pipes after Git exits. Keep the same
        // cancellation deadline active until all inherited handles close.
        let draining = !out_thread.is_finished()
            || !err_thread.is_finished()
            || writer.as_ref().is_some_and(|thread| !thread.is_finished());
        if let Some(status) = status
            && !draining
        {
            break status;
        }
        if status.is_some()
            && killed_at.is_some_and(|at| at.elapsed() >= Duration::from_millis(100))
        {
            return Err(Error::new(
                ErrorKind::Canceled,
                "Git operation canceled; a detached hook still holds its output pipe. Refresh to check the operation's outcome",
            ));
        }
        thread::sleep(Duration::from_millis(5));
    };
    if let Some(writer) = writer {
        let _ = writer.join();
    }
    let (stdout, stdout_large) = out_thread
        .join()
        .map_err(|_| Error::new(ErrorKind::Failed, "Git output reader stopped"))??;
    let (stderr, stderr_large) = err_thread
        .join()
        .map_err(|_| Error::new(ErrorKind::Failed, "Git error reader stopped"))??;
    if interrupted_at.is_some() {
        return Err(Error {
            kind: ErrorKind::Canceled,
            message: "Git operation canceled or timed out; refresh to check its outcome".into(),
            output: Run {
                status,
                stdout,
                stderr,
            }
            .log(),
        });
    }
    if stdout_large || stderr_large {
        return Err(Error::new(
            ErrorKind::TooLarge,
            format!("Git output exceeds the {} MiB limit", limit / (1024 * 1024)),
        ));
    }
    Ok(Run {
        status,
        stdout,
        stderr,
    })
}
