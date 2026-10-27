//! World-owned managed work. Workers own their I/O and return values through handles;
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

pub struct ManagedTask<T> {
    pub id: WorkId,
    receiver: Mutex<mpsc::Receiver<T>>,
    cancellation: Cancellation,
}

impl<T> ManagedTask<T> {
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

/// Cancellation and the final outcome are independent: cancellation is never rollback.
pub struct OperationOutcome<T> {
    pub cancellation_requested: bool,
    /// An abandoned/panicked producer reports unknown effects as an error.
    pub result: Result<T, String>,
}

/// A single admitted operation's producer. May be queued in an existing worker.
/// Dropping it without completion reports failure; it never silently loses an outcome.
pub struct OperationCompletion<T> {
    sender: mpsc::SyncSender<T>,
    cancellation: Cancellation,
}
impl<T> OperationCompletion<T> {
    pub fn cancellation(&self) -> Cancellation {
        self.cancellation.clone()
    }
    pub fn complete(self, value: T) {
        let _ = self.sender.send(value);
    }
}

type CompletionDelivery = Box<dyn FnOnce(&mut World) + Send>;
struct CompletionWork {
    owner: Option<Entity>,
    label: String,
    cancellation: Cancellation,
    poll: Mutex<Box<dyn FnMut() -> Option<CompletionDelivery> + Send>>,
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
pub struct ManagedProcess {
    pub id: WorkId,
    pub process_id: u32,
    input: Mutex<Option<mpsc::SyncSender<Vec<u8>>>>,
    output: Arc<Mutex<VecDeque<ProcessOutput>>>,
    exit_status: Arc<Mutex<Option<std::process::ExitStatus>>>,
    cancellation: Cancellation,
}

impl ManagedProcess {
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
pub struct ManagedWork {
    next_id: u64,
    closing: bool,
    running: BTreeMap<WorkId, RunningWork>,
    completions: BTreeMap<WorkId, CompletionWork>,
}

impl ManagedWork {
    /// Shutdown is final for this scope: no further work may be admitted.
    pub fn is_closing(&self) -> bool {
        self.closing
    }

    fn admit(&mut self) -> Result<WorkId, String> {
        if self.closing {
            return Err("Session is closing".into());
        }
        self.next_id = self.next_id.checked_add(1).expect("Work IDs exhausted");
        Ok(WorkId(self.next_id))
    }
    /// Reserve final delivery before queuing mutating work. The main-world callback
    /// runs exactly once in SessionSystems::Close, even after owner removal or shutdown.
    /// work_finished remains false until delivery. Cancellation is cooperative; the
    /// producer must complete or drop its token. World drop is only fallback cleanup.
    pub fn admit_operation<T: Send + 'static>(
        &mut self,
        owner: Option<Entity>,
        label: impl Into<String>,
        complete: impl FnOnce(&mut World, OperationOutcome<T>) + Send + 'static,
    ) -> Result<OperationCompletion<T>, String> {
        let id = self.admit()?;
        let cancellation = Cancellation::new();
        let cancel = cancellation.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        let mut complete = Some(complete);
        self.completions.insert(
            id,
            CompletionWork {
                owner,
                label: label.into(),
                cancellation: cancellation.clone(),
                poll: Mutex::new(Box::new(move || {
                    if complete.is_none() {
                        return None;
                    }
                    let result = match receiver.try_recv() {
                        Ok(value) => Ok(value),
                        Err(mpsc::TryRecvError::Empty) => return None,
                        Err(mpsc::TryRecvError::Disconnected) => Err(
                            "Operation producer stopped without a final outcome; effects may have occurred".into(),
                        ),
                    };
                    cancel.finish(result.as_ref().map(|_| ()).map_err(Clone::clone));
                    let outcome = OperationOutcome {
                        cancellation_requested: cancel.is_cancelled(),
                        result,
                    };
                    let complete = complete.take().expect("Completion delivered once");
                    Some(Box::new(move |world| complete(world, outcome)))
                })),
            },
        );
        Ok(OperationCompletion {
            sender,
            cancellation,
        })
    }

    /// Disposable calculation: cancellation may suppress its result. Use
    /// admit_operation for filesystem effects whose outcome must be reconciled.
    pub fn spawn_task<T: Send + 'static>(
        &mut self,
        owner: Option<Entity>,
        label: impl Into<String>,
        task: impl FnOnce(Cancellation) -> T + Send + 'static,
    ) -> Result<ManagedTask<T>, String> {
        let id = self.admit()?;
        let cancellation = Cancellation::new();
        let cancel = cancellation.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("managed-task".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let value = task(cancel.clone());
                    if !cancel.is_cancelled() {
                        let _ = sender.send(value);
                    }
                }));
                cancel.finish(result.map_err(|_| "Managed task panicked".into()));
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
        Ok(ManagedTask {
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
    ) -> Result<ManagedProcess, String> {
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
            .name("managed-process".into())
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
        Ok(ManagedProcess {
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
            .chain(self.completions.iter().map(|(&id, job)| WorkInfo {
                id,
                owner: job.owner,
                label: job.label.clone(),
                status: job.cancellation.status(),
            }))
            .collect()
    }

    pub fn cancel(&mut self, id: WorkId) -> Result<(), String> {
        let cancellation = self
            .running
            .get(&id)
            .map(|job| &job.cancellation)
            .or_else(|| self.completions.get(&id).map(|job| &job.cancellation))
            .ok_or("Work is unavailable")?;
        cancellation.cancel();
        Ok(())
    }

    pub fn shutdown(&mut self) {
        self.closing = true;
        for job in self.running.values() {
            job.cancellation.cancel();
        }
        for job in self.completions.values() {
            job.cancellation.cancel();
        }
    }

    pub fn is_finished(&self) -> bool {
        self.completions.is_empty() && self.running.values().all(|job| job.worker.is_finished())
    }

    /// Admitted effects whose final main-world reconciliation has not finished.
    /// Drain these before flushing final application state and cancelling work.
    pub fn operations_pending(&self) -> bool {
        !self.completions.is_empty()
    }

    pub(crate) fn finish_delivery(&mut self, id: WorkId) {
        self.completions.remove(&id).expect("Admitted completion");
    }

    pub(crate) fn poll(
        &mut self,
        owner_exists: impl Fn(Entity) -> bool,
    ) -> Vec<(WorkId, CompletionDelivery)> {
        let mut deliveries = Vec::new();
        for (&id, job) in &mut self.completions {
            if job.owner.is_some_and(|owner| !owner_exists(owner)) {
                job.cancellation.cancel();
            }
            if let Some(delivery) = (job.poll.get_mut().unwrap())() {
                deliveries.push((id, delivery));
            }
        }
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
                    .finish(Err("Managed worker panicked".into()));
            }
        }
        deliveries
    }
}

impl Drop for ManagedWork {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{WorkbenchCorePlugin, work_finished};

    #[derive(Resource, Default)]
    struct Outcomes(Vec<OperationOutcome<usize>>);

    #[test]
    fn cancellation_discards_calculations_but_retains_mutation_outcome_until_world_delivery() {
        let mut app = App::new();
        app.add_plugins(WorkbenchCorePlugin)
            .init_resource::<Outcomes>();
        let owner = app.world_mut().spawn_empty().id();
        let completion = app
            .world_mut()
            .resource_mut::<ManagedWork>()
            .admit_operation(Some(owner), "Mutation", |world, outcome| {
                assert!(world.contains_resource::<ManagedWork>());
                assert!(!work_finished(world));
                world.resource_mut::<Outcomes>().0.push(outcome);
            })
            .unwrap();
        let cancel = completion.cancellation();
        let (committed, effect) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        let task = app
            .world_mut()
            .resource_mut::<ManagedWork>()
            .spawn_task(Some(owner), "Mutation executor", move |stop| {
                // Stand in for a completed critical step, then cancellation races
                // final reporting. The completion token must retain that fact.
                committed.send(42).unwrap();
                while !stop.wait(Duration::from_millis(10)) {}
                wait.recv().unwrap();
                completion.complete(42);
                42
            })
            .unwrap();
        assert_eq!(effect.recv_timeout(Duration::from_secs(2)).unwrap(), 42);
        app.world_mut().despawn(owner);
        app.update();
        assert!(cancel.is_cancelled());
        app.world_mut().resource_mut::<ManagedWork>().shutdown();
        assert!(!work_finished(app.world()));
        release.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while matches!(task.status(), WorkStatus::Running | WorkStatus::Cancelling) {
            assert!(Instant::now() < deadline);
            thread::yield_now();
        }
        assert!(!work_finished(app.world()));
        assert!(matches!(
            task.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
        while !work_finished(app.world()) {
            app.update();
        }
        let outcomes = &app.world().resource::<Outcomes>().0;
        assert_eq!(outcomes.len(), 1);
        assert!(outcomes[0].cancellation_requested);
        assert_eq!(outcomes[0].result.as_ref().unwrap(), &42);
        app.update();
        assert_eq!(app.world().resource::<Outcomes>().0.len(), 1);
        assert!(
            app.world_mut()
                .resource_mut::<ManagedWork>()
                .admit_operation::<()>(None, "Late", |_, _| {})
                .is_err()
        );
    }

    #[test]
    fn abandoned_producer_reports_unknown_effects_instead_of_silently_finishing() {
        let mut app = App::new();
        app.add_plugins(WorkbenchCorePlugin)
            .init_resource::<Outcomes>();
        let completion = app
            .world_mut()
            .resource_mut::<ManagedWork>()
            .admit_operation(None, "Abandoned", |world, outcome| {
                world.resource_mut::<Outcomes>().0.push(outcome);
            })
            .unwrap();
        drop(completion);
        assert!(!work_finished(app.world()));
        app.update();
        assert!(work_finished(app.world()));
        let outcomes = &app.world().resource::<Outcomes>().0;
        assert_eq!(outcomes.len(), 1);
        assert!(!outcomes[0].cancellation_requested);
        assert!(
            outcomes[0]
                .result
                .as_ref()
                .unwrap_err()
                .contains("effects may have occurred")
        );
    }
}
