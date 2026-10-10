//! Bounded project-check processes, shared by local sessions and remote connections.
use crate::{ErrorKind, RemoteError, Request, Response};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    io::{self, Read},
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

const MAX_OUTPUT: usize = 16 * 1024 * 1024;
const MAX_JOBS: usize = 4;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct CheckOutput {
    pub exit_code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

pub struct CheckJob {
    canceled: Arc<AtomicBool>,
    completed: mpsc::Receiver<Result<CheckOutput, String>>,
}
impl CheckJob {
    pub fn start(
        root: PathBuf,
        program: String,
        arguments: Vec<String>,
        directory: String,
    ) -> Self {
        let canceled = Arc::new(AtomicBool::new(false));
        let stop = canceled.clone();
        let (sender, completed) = mpsc::sync_channel(1);
        thread::spawn(move || {
            let _ = sender
                .send(run(root, program, arguments, directory, stop).map_err(|e| e.to_string()));
        });
        Self {
            canceled,
            completed,
        }
    }
    pub fn cancel(&self) {
        self.canceled.store(true, Ordering::Release);
    }
    pub fn try_result(&mut self) -> Option<Result<CheckOutput, String>> {
        match self.completed.try_recv() {
            Ok(result) => Some(result),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                Some(Err("Project check worker stopped".into()))
            }
        }
    }
}
impl Drop for CheckJob {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn read_bounded(mut reader: impl Read) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut buffer = [0; 8192];
    let mut exceeded = false;
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        let remaining = MAX_OUTPUT.saturating_sub(bytes.len());
        exceeded |= count > remaining;
        bytes.extend_from_slice(&buffer[..count.min(remaining)]);
    }
    if exceeded {
        Err(io::Error::other("Project check output exceeds 16 MiB"))
    } else {
        Ok(bytes)
    }
}
fn run(
    root: PathBuf,
    program: String,
    arguments: Vec<String>,
    directory: String,
    canceled: Arc<AtomicBool>,
) -> io::Result<CheckOutput> {
    if program.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Check executable is empty",
        ));
    }
    let root = root.canonicalize()?;
    let cwd = root.join(directory).canonicalize()?;
    if !cwd.starts_with(&root) || !cwd.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Check working directory must be inside the project",
        ));
    }
    let mut command = Command::new(program);
    command
        .args(arguments)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("NO_COLOR", "1");
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command.spawn()?;
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let out = thread::spawn(move || read_bounded(stdout));
    let err = thread::spawn(move || read_bounded(stderr));
    let start = Instant::now();
    let stop = |child: &mut std::process::Child| {
        #[cfg(unix)]
        unsafe {
            libc::kill(-(child.id() as i32), libc::SIGKILL);
        }
        let _ = child.kill();
    };
    let status = loop {
        if canceled.load(Ordering::Acquire) || start.elapsed() > Duration::from_secs(600) {
            stop(&mut child);
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Project check canceled or timed out",
            ));
        }
        if let Some(status) = child.try_wait()? {
            break status;
        }
        thread::sleep(Duration::from_millis(10));
    };
    while !out.is_finished() || !err.is_finished() {
        if canceled.load(Ordering::Acquire) || start.elapsed() > Duration::from_secs(600) {
            stop(&mut child);
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Project check canceled while draining output",
            ));
        }
        thread::sleep(Duration::from_millis(10));
    }
    Ok(CheckOutput {
        exit_code: status.code(),
        stdout: out
            .join()
            .map_err(|_| io::Error::other("Check output reader stopped"))??,
        stderr: err
            .join()
            .map_err(|_| io::Error::other("Check error reader stopped"))??,
    })
}

#[derive(Default)]
pub(crate) struct CheckJobs {
    next: u64,
    jobs: HashMap<u64, Entry>,
}
struct Entry {
    job: CheckJob,
    result: Option<Result<CheckOutput, String>>,
}
impl CheckJobs {
    pub fn handles(request: &Request) -> bool {
        matches!(
            request,
            Request::CheckStart { .. }
                | Request::CheckPoll { .. }
                | Request::CheckCancel { .. }
                | Request::CheckRelease { .. }
        )
    }
    pub fn call(&mut self, request: Request) -> Result<Response, RemoteError> {
        let missing = || RemoteError::new(ErrorKind::NotFound, "Project check job expired");
        match request {
            Request::CheckStart {
                root,
                program,
                arguments,
                directory,
            } => {
                if self.jobs.len() >= MAX_JOBS {
                    return Err(RemoteError::new(
                        ErrorKind::Conflict,
                        "Project check job limit reached",
                    ));
                }
                let root = PathBuf::from(root).canonicalize()?;
                self.next = self.next.checked_add(1).ok_or_else(|| {
                    RemoteError::new(ErrorKind::Other, "Check identifiers exhausted")
                })?;
                let job_id = self.next;
                self.jobs.insert(
                    job_id,
                    Entry {
                        job: CheckJob::start(root, program, arguments, directory),
                        result: None,
                    },
                );
                Ok(Response::CheckStarted { job_id })
            }
            Request::CheckPoll { job_id } => {
                let entry = self.jobs.get_mut(&job_id).ok_or_else(missing)?;
                if entry.result.is_none() {
                    entry.result = entry.job.try_result();
                }
                Ok(Response::CheckJob {
                    result: entry.result.clone(),
                })
            }
            Request::CheckCancel { job_id } => {
                self.jobs.get(&job_id).ok_or_else(missing)?.job.cancel();
                Ok(Response::Unit)
            }
            Request::CheckRelease { job_id } => {
                self.jobs.remove(&job_id);
                Ok(Response::Unit)
            }
            _ => Err(RemoteError::new(
                ErrorKind::InvalidInput,
                "Not a project check request",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn check_rejects_directory_outside_project() {
        let root = std::env::temp_dir();
        let mut job = CheckJob::start(root, "echo".into(), vec![], "..".into());
        let start = Instant::now();
        loop {
            if let Some(result) = job.try_result() {
                assert!(result.unwrap_err().contains("inside the project"));
                break;
            }
            assert!(start.elapsed() < Duration::from_secs(5));
            thread::sleep(Duration::from_millis(5));
        }
    }
    #[cfg(unix)]
    #[test]
    fn cancel_kills_check_and_descendants_without_blocking() {
        let mut job = CheckJob::start(
            std::env::temp_dir(),
            "sh".into(),
            vec!["-c".into(), "sleep 30 & wait".into()],
            ".".into(),
        );
        thread::sleep(Duration::from_millis(30));
        job.cancel();
        let start = Instant::now();
        loop {
            if let Some(result) = job.try_result() {
                assert!(result.unwrap_err().contains("canceled"));
                break;
            }
            assert!(start.elapsed() < Duration::from_secs(3));
            thread::sleep(Duration::from_millis(5));
        }
    }
}
