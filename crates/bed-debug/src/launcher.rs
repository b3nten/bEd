//! The small Unix `runInTerminal` trampoline used by the desktop executable.
//!
//! LLVM normally attaches to its own adapter before that process execs the
//! debuggee. Apple's platform-signed adapter cannot itself be attached to. The
//! host instead runs this same FIFO handshake in its own executable, retaining
//! the terminal, environment, argument boundaries, and PID across `exec`.
use crate::RunInTerminalRequest;
use std::{ffi::OsString, io, path::Path};

pub const FLAG: &str = "--debug-launch-target";

/// Rewrite only LLVM's known Unix launcher invocation. Other adapters' terminal
/// requests retain their original command. Arguments following the target are
/// opaque, including any strings which resemble launcher flags.
pub fn arguments(args: &[String], host_exe: &Path) -> Option<Vec<String>> {
    #[cfg(unix)]
    if args.len() >= 7
        && args[1] == "--comm-file"
        && args[3] == "--debugger-pid"
        && args[5] == "--launch-target"
        && !args[2].is_empty()
        && !args[6].is_empty()
        && args[4].parse::<i32>().is_ok_and(|pid| pid > 0)
    {
        let executable = host_exe.to_str()?;
        let mut command = vec![executable.to_owned(), FLAG.to_owned()];
        command.extend_from_slice(&args[1..]);
        return Some(command);
    }
    #[cfg(not(unix))]
    let _ = (args, host_exe);
    None
}

pub fn rewrite_terminal_request(
    request: &mut RunInTerminalRequest,
    host_exe: &Path,
) -> io::Result<bool> {
    if let Some(args) = arguments(&request.args, host_exe) {
        request.args = args;
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Run before initializing a GUI, logger, or application services. Pass arguments
/// after `FLAG`. Success replaces this process with the requested debuggee;
/// callers must exit if an error is returned.
pub fn run(args: impl IntoIterator<Item = OsString>) -> io::Result<()> {
    #[cfg(unix)]
    {
        unix::run(args.into_iter().collect())
    }
    #[cfg(not(unix))]
    {
        let _ = args;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "The debugger launcher is supported on macOS and Linux",
        ))
    }
}

#[cfg(unix)]
mod unix {
    use super::*;
    use serde_json::{Value, json};
    use std::{
        fs::{File, OpenOptions},
        io::{Read, Write},
        os::{
            fd::AsRawFd,
            unix::{
                fs::{FileTypeExt, MetadataExt, OpenOptionsExt},
                process::CommandExt,
            },
        },
        process::Command,
        sync::mpsc,
        thread,
        time::{Duration, Instant},
    };
    const TIMEOUT: Duration = Duration::from_secs(20);
    const MAX_MESSAGE: usize = 16 * 1024;

    fn validate_fifo(path: &Path) -> io::Result<()> {
        let metadata = std::fs::symlink_metadata(path)?;
        if !metadata.file_type().is_fifo() || metadata.uid() != unsafe { libc::geteuid() } {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Debugger communication path must be a FIFO owned by this user",
            ));
        }
        Ok(())
    }
    fn wait(deadline: Instant) -> io::Result<()> {
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Timed out waiting for LLDB to attach to the terminal launcher",
            ));
        }
        thread::sleep(Duration::from_millis(2));
        Ok(())
    }
    fn open(path: &Path, write: bool, deadline: Instant) -> io::Result<File> {
        loop {
            match OpenOptions::new()
                .read(!write)
                .write(write)
                .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC | libc::O_NOFOLLOW)
                .open(path)
            {
                Ok(file) => {
                    let metadata = file.metadata()?;
                    if !metadata.file_type().is_fifo()
                        || metadata.uid() != unsafe { libc::geteuid() }
                    {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidInput,
                            "Debugger communication FIFO changed",
                        ));
                    }
                    return Ok(file);
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) if write && e.raw_os_error() == Some(libc::ENXIO) => wait(deadline)?,
                Err(e) => return Err(e),
            }
        }
    }
    fn notify_pid(path: &Path, deadline: Instant) -> io::Result<()> {
        let mut writer = open(path, true, deadline)?;
        let message = format!("{}\n", json!({"kind":"pid","pid":std::process::id()}));
        let mut remaining = message.as_bytes();
        while !remaining.is_empty() {
            match writer.write(remaining) {
                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(n) => remaining = &remaining[n..],
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => wait(deadline)?,
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
    fn open_reply_reader(path: &Path, deadline: Instant) -> io::Result<File> {
        let path = path.to_owned();
        let (sender, receiver) = mpsc::sync_channel(1);
        // The blocking open is essential: it cannot complete until LLDB opens
        // its reply writer, after consuming the PID. A nonblocking open could
        // instead consume our own PID from the shared FIFO. Only this worker
        // can remain blocked on timeout; run's caller exits the helper process.
        thread::Builder::new()
            .name("bed-debug-launcher-open".into())
            .spawn(move || {
                let result = OpenOptions::new()
                    .read(true)
                    .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
                    .open(path);
                let _ = sender.send(result);
            })?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        let file = receiver.recv_timeout(remaining).map_err(|_| {
            io::Error::new(
                io::ErrorKind::TimedOut,
                "Timed out waiting for LLDB to attach to the terminal launcher",
            )
        })??;
        let metadata = file.metadata()?;
        if !metadata.file_type().is_fifo() || metadata.uid() != unsafe { libc::geteuid() } {
            return Err(io::Error::other("Debugger communication FIFO changed"));
        }
        let flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFL) };
        if flags < 0
            || unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(file)
    }
    fn wait_attached(path: &Path, deadline: Instant) -> io::Result<()> {
        let mut reader = open_reply_reader(path, deadline)?;
        let mut message = Vec::new();
        loop {
            let mut bytes = [0; 1024];
            match reader.read(&mut bytes) {
                Ok(0) => wait(deadline)?,
                Ok(n) => {
                    message.extend_from_slice(&bytes[..n]);
                    if message.len() > MAX_MESSAGE {
                        return Err(io::Error::other("LLDB launcher message exceeds 16 KiB"));
                    }
                    if let Some(end) = message.iter().position(|b| *b == b'\n') {
                        let value: Value =
                            serde_json::from_slice(&message[..end]).map_err(io::Error::other)?;
                        return match value["kind"].as_str() {
                            Some("didAttach") => Ok(()),
                            Some("error") => Err(io::Error::other(
                                value["error"]
                                    .as_str()
                                    .unwrap_or("LLDB attach failed")
                                    .to_owned(),
                            )),
                            _ => Err(io::Error::other("Unexpected LLDB launcher message")),
                        };
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => wait(deadline)?,
                Err(e) => return Err(e),
            }
        }
    }
    pub(super) fn run(args: Vec<OsString>) -> io::Result<()> {
        if args.len() < 6
            || args[0] != "--comm-file"
            || args[2] != "--debugger-pid"
            || args[4] != "--launch-target"
            || args[5].is_empty()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Expected --comm-file FIFO --debugger-pid PID --launch-target PROGRAM [ARGS...]",
            ));
        }
        let debugger_pid = args[3]
            .to_str()
            .and_then(|p| p.parse::<i32>().ok())
            .filter(|pid| *pid > 0)
            .ok_or_else(|| io::Error::other("Invalid LLDB adapter PID"))?;
        let path = Path::new(&args[1]);
        validate_fifo(path)?;
        // This mirrors LLVM's launcher: Yama otherwise prevents an adapter
        // from tracing a sibling launched by its client.
        #[cfg(target_os = "linux")]
        if unsafe { libc::prctl(libc::PR_SET_PTRACER, debugger_pid, 0, 0, 0) } != 0 {
            return Err(io::Error::last_os_error());
        }
        #[cfg(not(target_os = "linux"))]
        let _ = debugger_pid;
        let deadline = Instant::now() + TIMEOUT;
        notify_pid(path, deadline)?;
        wait_attached(path, deadline)?;
        Err(Command::new(&args[5]).args(&args[6..]).exec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[cfg(unix)]
    fn rewrites_only_the_launcher_prefix_and_preserves_literal_target_arguments() {
        let args: Vec<String> = [
            "/llvm/lldb-dap",
            "--comm-file",
            "/tmp/fifo",
            "--debugger-pid",
            "123",
            "--launch-target",
            "/path with spaces/program",
            "$(literal) 日本語",
            "--comm-file",
        ]
        .map(str::to_owned)
        .into();
        let command = arguments(&args, Path::new("/app/bed")).unwrap();
        assert_eq!(&command[..2], ["/app/bed", FLAG]);
        assert_eq!(&command[2..], &args[1..]);
        assert!(
            arguments(
                &["/program".into(), "--comm-file".into()],
                Path::new("/app/bed")
            )
            .is_none()
        );
        let mut invalid = args;
        invalid[4] = "0".into();
        assert!(arguments(&invalid, Path::new("/app/bed")).is_none());
    }
    #[test]
    fn rejects_incomplete_helper_invocations() {
        assert!(run(Vec::<OsString>::new()).is_err());
    }
}
