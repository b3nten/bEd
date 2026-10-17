//! Workspace-owned work. Workers own their I/O and return values through handles;
//! they never borrow an ECS world. Closing requests cancellation without waiting.
use bevy::prelude::*;
use std::{
    collections::{BTreeMap, VecDeque},
    ffi::OsString,
    io::{self, Read, Write},
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WorkId(pub u64);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkStatus {
    Running,
    Cancelling,
    Cancelled,
    Finished,
    Failed(String),
}

#[derive(Clone)]
pub struct Cancellation(Arc<Control>);
struct Control {
    cancelled: AtomicBool,
    status: Mutex<WorkStatus>,
    wake: Condvar,
}

impl Cancellation {
    fn new() -> Self {
        Self(Arc::new(Control {
            cancelled: AtomicBool::new(false),
            status: Mutex::new(WorkStatus::Running),
            wake: Condvar::new(),
        }))
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::Acquire)
    }

    pub fn cancel(&self) {
        self.0.cancelled.store(true, Ordering::Release);
        let mut status = self.0.status.lock().unwrap();
        if matches!(*status, WorkStatus::Running) {
            *status = WorkStatus::Cancelling;
        }
        self.0.wake.notify_all();
    }

    /// Interruptible waiting for cooperative tasks; returns true on cancellation.
    pub fn wait(&self, duration: Duration) -> bool {
        let guard = self.0.status.lock().unwrap();
        let _ = self
            .0
            .wake
            .wait_timeout_while(guard, duration, |_| !self.is_cancelled())
            .unwrap();
        self.is_cancelled()
    }

    pub fn status(&self) -> WorkStatus {
        self.0.status.lock().unwrap().clone()
    }

    fn finish(&self, result: Result<(), String>) {
        *self.0.status.lock().unwrap() = match result {
            Err(error) => WorkStatus::Failed(error),
            Ok(()) if self.is_cancelled() => WorkStatus::Cancelled,
            Ok(()) => WorkStatus::Finished,
        };
        self.0.wake.notify_all();
    }
}

pub struct WorkspaceTask<T> {
    pub id: WorkId,
    receiver: Mutex<mpsc::Receiver<T>>,
    cancellation: Cancellation,
}

impl<T> WorkspaceTask<T> {
    pub fn try_recv(&self) -> Result<T, mpsc::TryRecvError> {
        self.receiver.lock().unwrap().try_recv()
    }

    pub fn cancel(&self) {
        self.cancellation.cancel();
    }

    pub fn status(&self) -> WorkStatus {
        self.cancellation.status()
    }
}

#[derive(Clone, Debug)]
pub struct ProcessSpec {
    pub program: OsString,
    pub arguments: Vec<OsString>,
    pub directory: Option<PathBuf>,
    pub environment: Vec<(OsString, OsString)>,
}

impl ProcessSpec {
    pub fn new(program: impl Into<OsString>) -> Self {
        Self {
            program: program.into(),
            arguments: Vec::new(),
            directory: None,
            environment: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ProcessOutput {
    pub stderr: bool,
    pub bytes: Vec<u8>,
}

/// Bounded retained output: workers always drain pipes, including while inactive.
pub struct WorkspaceProcess {
    pub id: WorkId,
    pub process_id: u32,
    input: Mutex<Option<mpsc::SyncSender<Vec<u8>>>>,
    output: Arc<Mutex<VecDeque<ProcessOutput>>>,
    exit_status: Arc<Mutex<Option<std::process::ExitStatus>>>,
    cancellation: Cancellation,
}

impl WorkspaceProcess {
    /// Queue at most 8 KiB per write. Backpressure is reported to the caller.
    pub fn write(&self, bytes: &[u8]) -> io::Result<()> {
        if self.cancellation.is_cancelled() {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "Process is stopping",
            ));
        }
        if bytes.len() > 8192 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Process writes are limited to 8 KiB",
            ));
        }
        self.input
            .lock()
            .unwrap()
            .as_ref()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "Process stdin is closed"))?
            .try_send(bytes.to_vec())
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => {
                    io::Error::new(io::ErrorKind::WouldBlock, "Process input queue is full")
                }
                mpsc::TrySendError::Disconnected(_) => {
                    io::Error::new(io::ErrorKind::BrokenPipe, "Process has exited")
                }
            })
    }

    /// Finish queued input and close the pipe so tools waiting for EOF can exit.
    pub fn close_stdin(&self) {
        self.input.lock().unwrap().take();
    }

    pub fn drain_output(&self) -> Vec<ProcessOutput> {
        self.output.lock().unwrap().drain(..).collect()
    }

    pub fn exit_status(&self) -> Option<std::process::ExitStatus> {
        *self.exit_status.lock().unwrap()
    }

    pub fn cancel(&self) {
        self.cancellation.cancel();
    }

    pub fn status(&self) -> WorkStatus {
        self.cancellation.status()
    }
}

struct RunningWork {
    owner: Option<Entity>,
    label: String,
    cancellation: Cancellation,
    worker: JoinHandle<()>,
    process: Option<Arc<Mutex<OwnedChild>>>,
}

#[derive(Clone, Debug)]
pub struct WorkInfo {
    pub id: WorkId,
    pub owner: Option<Entity>,
    pub label: String,
    pub status: WorkStatus,
}

#[derive(Resource, Default)]
pub struct WorkspaceWork {
    next_id: u64,
    closing: bool,
    running: BTreeMap<WorkId, RunningWork>,
}

impl WorkspaceWork {
    fn admit(&mut self) -> Result<WorkId, String> {
        if self.closing {
            return Err("Workspace is closing".into());
        }
        self.next_id = self.next_id.checked_add(1).expect("Work IDs exhausted");
        Ok(WorkId(self.next_id))
    }
    pub fn spawn_task<T: Send + 'static>(
        &mut self,
        owner: Option<Entity>,
        label: impl Into<String>,
        task: impl FnOnce(Cancellation) -> T + Send + 'static,
    ) -> Result<WorkspaceTask<T>, String> {
        let id = self.admit()?;
        let cancellation = Cancellation::new();
        let cancel = cancellation.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("workspace-task".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let value = task(cancel.clone());
                    if !cancel.is_cancelled() {
                        let _ = sender.send(value);
                    }
                }));
                cancel.finish(result.map_err(|_| "Workspace task panicked".into()));
            })
            .map_err(|e| e.to_string())?;
        self.running.insert(
            id,
            RunningWork {
                owner,
                label: label.into(),
                cancellation: cancellation.clone(),
                worker,
                process: None,
            },
        );
        Ok(WorkspaceTask {
            id,
            receiver: Mutex::new(receiver),
            cancellation,
        })
    }

    /// Starts an owned pipe process. PTY/protocol adapters can use spawn_task to
    /// own their backend and perform its graceful shutdown on cancellation.
    pub fn spawn_process(
        &mut self,
        owner: Option<Entity>,
        label: impl Into<String>,
        spec: ProcessSpec,
    ) -> Result<WorkspaceProcess, String> {
        let id = self.admit()?;
        let mut command = Command::new(&spec.program);
        command
            .args(&spec.arguments)
            .envs(spec.environment.iter().cloned())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(directory) = spec.directory {
            command.current_dir(directory);
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = command.spawn().map_err(|e| e.to_string())?;
        let process_id = child.id();
        let mut stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let output = Arc::new(Mutex::new(VecDeque::new()));
        let exit_status = Arc::new(Mutex::new(None));
        let cancellation = Cancellation::new();
        let cancel = cancellation.clone();
        let (input, receiver) = mpsc::sync_channel::<Vec<u8>>(16);
        let chunks = output.clone();
        let exit = exit_status.clone();
        // The supervisor owns normal child transitions; the scope can force-stop
        // the same child during Drop. Failed thread creation drops this guard.
        let process = Arc::new(Mutex::new(OwnedChild {
            child,
            force_stopped: false,
        }));
        let child = process.clone();
        let worker = thread::Builder::new()
            .name("workspace-process".into())
            .spawn(move || {
                let reader =
                    |mut pipe: Box<dyn Read + Send>,
                     stderr: bool,
                     chunks: Arc<Mutex<VecDeque<ProcessOutput>>>| {
                        thread::spawn(move || {
                            let mut buffer = [0; 4096];
                            loop {
                                match pipe.read(&mut buffer) {
                                    Ok(0) => break,
                                    Ok(n) => {
                                        let mut chunks = chunks.lock().unwrap();
                                        if chunks.len() == 64 {
                                            chunks.pop_front();
                                        }
                                        chunks.push_back(ProcessOutput {
                                            stderr,
                                            bytes: buffer[..n].to_vec(),
                                        });
                                    }
                                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                                    Err(_) => break,
                                }
                            }
                        })
                    };
                let readers = [
                    reader(Box::new(stdout), false, chunks.clone()),
                    reader(Box::new(stderr), true, chunks),
                ];
                let writer_stop = Arc::new(AtomicBool::new(false));
                let stop_writer = writer_stop.clone();
                let writer = thread::spawn(move || {
                    while !stop_writer.load(Ordering::Acquire) {
                        match receiver.recv_timeout(Duration::from_millis(10)) {
                            Ok(bytes) => {
                                if stdin.write_all(&bytes).is_err() {
                                    break;
                                }
                            }
                            Err(mpsc::RecvTimeoutError::Timeout) => continue,
                            Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        }
                    }
                });
                let mut stopping = None;
                let result = loop {
                    if cancel.is_cancelled() {
                        let deadline = stopping.get_or_insert_with(|| {
                            child.lock().unwrap().signal(false);
                            Instant::now() + Duration::from_millis(250)
                        });
                        if Instant::now() >= *deadline {
                            child.lock().unwrap().signal(true);
                        }
                    }
                    match child.lock().unwrap().try_wait() {
                        Ok(Some(status)) => {
                            *exit.lock().unwrap() = Some(status);
                            break Ok(());
                        }
                        Ok(None) => thread::sleep(Duration::from_millis(5)),
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                        Err(error) => break Err(error.to_string()),
                    }
                };
                // Also stop descendants that retained a pipe after their parent exited.
                child.lock().unwrap().signal(true);
                writer_stop.store(true, Ordering::Release);
                let _ = writer.join();
                for reader in readers {
                    let _ = reader.join();
                }
                cancel.finish(result);
            })
            .map_err(|e| e.to_string())?;
        self.running.insert(
            id,
            RunningWork {
                owner,
                label: label.into(),
                cancellation: cancellation.clone(),
                worker,
                process: Some(process),
            },
        );
        Ok(WorkspaceProcess {
            id,
            process_id,
            input: Mutex::new(Some(input)),
            output,
            exit_status,
            cancellation,
        })
    }

    pub fn list(&self) -> Vec<WorkInfo> {
        self.running
            .iter()
            .map(|(&id, job)| WorkInfo {
                id,
                owner: job.owner,
                label: job.label.clone(),
                status: job.cancellation.status(),
            })
            .collect()
    }

    pub fn cancel(&mut self, id: WorkId) -> Result<(), String> {
        self.running
            .get(&id)
            .ok_or("Work is unavailable")?
            .cancellation
            .cancel();
        Ok(())
    }

    pub fn shutdown(&mut self) {
        self.closing = true;
        for job in self.running.values() {
            job.cancellation.cancel();
        }
    }

    pub fn is_finished(&self) -> bool {
        self.running.values().all(|job| job.worker.is_finished())
    }

    pub(crate) fn poll(&mut self, owner_exists: impl Fn(Entity) -> bool) {
        for job in self.running.values() {
            if job.owner.is_some_and(|owner| !owner_exists(owner)) {
                job.cancellation.cancel();
            }
        }
        let finished: Vec<_> = self
            .running
            .iter()
            .filter_map(|(&id, job)| job.worker.is_finished().then_some(id))
            .collect();
        for id in finished {
            let job = self.running.remove(&id).unwrap();
            if job.worker.join().is_err() {
                job.cancellation
                    .finish(Err("Workspace worker panicked".into()));
            }
        }
    }
}

impl Drop for WorkspaceWork {
    fn drop(&mut self) {
        self.shutdown();
        // Drop is a forceful fallback. Signal owned processes immediately even
        // if the application is exiting before their supervisors can run again.
        for job in self.running.values() {
            if let Some(process) = &job.process {
                process.lock().unwrap().signal(true);
            }
        }
    }
}

struct OwnedChild {
    child: std::process::Child,
    force_stopped: bool,
}

impl OwnedChild {
    fn try_wait(&mut self) -> io::Result<Option<std::process::ExitStatus>> {
        #[cfg(unix)]
        {
            // Keep the exited leader unreaped until its process group is stopped.
            // Its reserved PID prevents signaling a newly reused group ID.
            let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
            let result = unsafe {
                libc::waitid(
                    libc::P_PID,
                    self.child.id(),
                    &mut info,
                    libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                )
            };
            if result == -1 {
                return Err(io::Error::last_os_error());
            }
            if unsafe { info.si_pid() } == 0 {
                return Ok(None);
            }
            self.signal(true);
        }
        self.child.try_wait()
    }

    fn signal(&mut self, force: bool) {
        if self.force_stopped {
            return;
        }
        #[cfg(unix)]
        unsafe {
            libc::kill(
                -(self.child.id() as i32),
                if force { libc::SIGKILL } else { libc::SIGTERM },
            );
        }
        #[cfg(not(unix))]
        {
            let _ = self.child.kill();
        }
        if force {
            self.force_stopped = true;
        }
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        self.signal(true);
        let _ = self.child.wait();
    }
}
