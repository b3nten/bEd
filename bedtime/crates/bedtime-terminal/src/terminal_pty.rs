//! Worker-owned Unix PTY transport using the operating system PTY/process APIs.
//! The worker owns parsing, optional graphics decoding and terminal mutations.
//! Legacy spawn helpers own a thread; prepared workers run on their caller.
//! Shell setup affects only the child, never host cwd/env/signal handlers.
use std::io;
use std::thread;

use crate::terminal::{SelectionSnap, Terminal, TerminalEvent, TerminalSnapshot, TerminalTheme};
use crate::terminal_input::{
    KeyboardModes, TerminalKeyEvent, encode_key, encode_paste, encode_text,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowSize {
    pub num_cols: u16,
    pub num_lines: u16,
    pub cell_width: u16,
    pub cell_height: u16,
}

enum ChildEvent {
    Exited,
}
use polling::{Event, Events, PollMode, Poller};
use std::{
    collections::{HashMap, VecDeque},
    io::{Read, Write},
    path::PathBuf,
    process::ExitStatus,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering},
        mpsc,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

pub const MAX_WRITE_BYTES: usize = 1024 * 1024;
pub const MAX_QUEUED_WRITE_BYTES: usize = 4 * 1024 * 1024;
const MAX_QUEUED_REPLY_BYTES: usize = 4 * 1024 * 1024;
const REPLY_READ_WATERMARK: usize = 1024 * 1024;
pub const OUTPUT_CHUNK_BYTES: usize = 16 * 1024;
pub const OUTPUT_QUEUE_CHUNKS: usize = 64;
const COMMAND_CAPACITY: usize = 64;
const PROCESS_TITLE_INTERVAL: Duration = Duration::from_millis(400);
const ACTIVE_POLL_INTERVAL: Duration = Duration::from_millis(20);
const EXITED_POLL_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalShell {
    pub program: String,
    pub args: Vec<String>,
    pub login: bool,
}

impl TerminalShell {
    pub fn new(program: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            program: program.into(),
            args,
            login: false,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct PtyOptions {
    pub working_directory: Option<PathBuf>,
    pub shell: Option<TerminalShell>,
    pub env: HashMap<String, String>,
    /// Remove inherited variables after terminal defaults are applied. Explicit
    /// entries in `env` take precedence over removal, without changing the host.
    pub env_remove: Vec<String>,
}

impl PtyOptions {
    /// Run an interactive remote login shell through the system SSH client.
    /// The target directory is never used as a local child working directory.
    #[cfg(feature = "remote")]
    pub fn for_ssh(&self, target: &bedtime_remote::SshTarget, root: &str) -> io::Result<Self> {
        if target.host.is_empty()
            || target.host.starts_with('-')
            || target.host.chars().any(char::is_whitespace)
            || target.host.contains('\0')
            || !root.starts_with('/')
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "SSH terminals require a host and absolute remote root",
            ));
        }
        let mut options = self.clone();
        options.working_directory = None;
        options.shell = Some(TerminalShell::new(
            "ssh",
            vec![
                "-tt".into(),
                "--".into(),
                target.host.clone(),
                format!(
                    "cd {} && exec \"${{SHELL:-/bin/sh}}\" -l",
                    bedtime_remote::shell_quote(root)?
                ),
            ],
        ));
        Ok(options)
    }
}

#[derive(Debug)]
pub enum PtyEvent {
    Output(Vec<u8>),
    Exited(Option<ExitStatus>),
    Error(String),
}

struct QueuedWrite {
    bytes: Vec<u8>,
    offset: usize,
    _reservation: InputReservation,
}

struct InputReservation {
    budget: Arc<AtomicUsize>,
    bytes: usize,
}

impl Drop for InputReservation {
    fn drop(&mut self) {
        self.budget.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

fn reserve_bytes(
    budget: &Arc<AtomicUsize>,
    bytes: usize,
    limit: usize,
) -> io::Result<InputReservation> {
    let mut used = budget.load(Ordering::Acquire);
    loop {
        let total = used
            .checked_add(bytes)
            .filter(|total| *total <= limit)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "Terminal write queue exceeds its byte limit",
                )
            })?;
        match budget.compare_exchange_weak(used, total, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => {
                return Ok(InputReservation {
                    bytes,
                    budget: Arc::clone(budget),
                });
            }
            Err(current) => used = current,
        }
    }
}

#[allow(clippy::large_enum_variant)] // Only 64 inline commands; avoid allocating each keystroke.
enum PtyCommand {
    Write(QueuedWrite),
    Resize(WindowSize),
    Terminal(TerminalCommand, Option<InputReservation>),
}

/// Commands are resolved in order by the terminal's sole writer.
#[derive(Debug)]
pub enum TerminalCommand {
    Key(TerminalKeyEvent),
    Paste(String),
    Text(String),
    Theme(TerminalTheme),
    Scroll(i32),
    SelectionStart {
        col: usize,
        row: usize,
        snap: SelectionSnap,
    },
    SelectionExtend {
        col: usize,
        row: usize,
        rectangular: bool,
        done: bool,
    },
    ClearSelection,
    SelectAll,
    Focus(bool),
    Stop,
}

pub struct TerminalPty {
    commands: Option<mpsc::SyncSender<PtyCommand>>,
    output: Option<Mutex<mpsc::Receiver<PtyEvent>>>,
    worker: Option<JoinHandle<()>>,
    poller: Arc<Poller>,
    stop: Arc<AtomicBool>,
    alive: Arc<AtomicBool>,
    cleanup_confirmed: Arc<AtomicBool>,
    budget: Arc<AtomicUsize>,
    process_id: Arc<AtomicU32>,
    shell_name: String,
    status: Arc<Mutex<VecDeque<PtyEvent>>>,
    foreground_process: Arc<Mutex<Option<String>>>,
    snapshot: Arc<Mutex<Arc<TerminalSnapshot>>>,
    snapshot_requested: Arc<AtomicBool>,
}

/// A prepared terminal's sole PTY/parser owner. Preparation does not launch a
/// child or a thread; call [`TerminalWorker::run`] from the admitted owner task.
/// Dropping an unstarted worker simply disconnects its handle.
pub struct TerminalWorker {
    options: PtyOptions,
    size: WindowSize,
    terminal: Terminal,
    commands: mpsc::Receiver<PtyCommand>,
    output: mpsc::SyncSender<PtyEvent>,
    poller: Arc<Poller>,
    stop: Arc<AtomicBool>,
    alive: Arc<AtomicBool>,
    process_id: Arc<AtomicU32>,
    foreground_process: Arc<Mutex<Option<String>>>,
    snapshot: Arc<Mutex<Arc<TerminalSnapshot>>>,
    snapshot_requested: Arc<AtomicBool>,
    status: Arc<Mutex<VecDeque<PtyEvent>>>,
    cleanup_confirmed: Arc<AtomicBool>,
    started: bool,
    capture_output: bool,
}

impl TerminalPty {
    pub fn spawn(options: &PtyOptions, size: WindowSize) -> io::Result<Self> {
        Self::spawn_owned(
            options,
            size,
            Terminal::new(size.num_cols as usize, size.num_lines as usize),
            true,
        )
    }

    /// Production sessions publish snapshots; raw output is only captured by
    /// the transport fixture API above, avoiding output backpressure on the UI.
    pub fn spawn_terminal(
        options: &PtyOptions,
        size: WindowSize,
        terminal: Terminal,
    ) -> io::Result<Self> {
        Self::spawn_owned(options, size, terminal, false)
    }

    /// Allocate a bounded handle and its blocking owner without starting a
    /// process or thread. Input can be queued before `run`; the PID is then 0.
    /// Shell defaults are resolved now, without modifying host cwd/environment.
    pub fn prepare_terminal(
        options: PtyOptions,
        size: WindowSize,
        terminal: Terminal,
    ) -> io::Result<(Self, TerminalWorker)> {
        Self::prepare_owned(options, size, terminal, false)
    }

    fn prepare_owned(
        mut options: PtyOptions,
        size: WindowSize,
        mut terminal: Terminal,
        capture_output: bool,
    ) -> io::Result<(Self, TerminalWorker)> {
        validate_size(size)?;
        let shell = platform::shell(&options)?;
        let shell_name = crate::process_title::program_name(&shell.program);
        options.shell = Some(shell);
        terminal.resize_with_pixels(
            size.num_cols as usize,
            size.num_lines as usize,
            size.cell_width as u32,
            size.cell_height as u32,
        );
        let snapshot = Arc::new(Mutex::new(terminal.snapshot()));
        let snapshot_requested = Arc::new(AtomicBool::new(false));
        let foreground_process = Arc::new(Mutex::new(None));
        let poller = Arc::new(Poller::new()?);
        let (commands, receiver) = mpsc::sync_channel(COMMAND_CAPACITY);
        let (sender, output) = mpsc::sync_channel(OUTPUT_QUEUE_CHUNKS);
        let stop = Arc::new(AtomicBool::new(false));
        // A prepared session accepts bounded input, even before its child starts.
        let alive = Arc::new(AtomicBool::new(true));
        let process_id = Arc::new(AtomicU32::new(0));
        let cleanup_confirmed = Arc::new(AtomicBool::new(false));
        // Only lifecycle events go here (one exit and at most one run error).
        // They cannot be lost behind, or block on, captured raw output.
        let status = Arc::new(Mutex::new(VecDeque::new()));
        let worker = TerminalWorker {
            options,
            size,
            terminal,
            commands: receiver,
            output: sender,
            poller: poller.clone(),
            stop: stop.clone(),
            alive: alive.clone(),
            process_id: process_id.clone(),
            foreground_process: foreground_process.clone(),
            snapshot: snapshot.clone(),
            snapshot_requested: snapshot_requested.clone(),
            status: status.clone(),
            cleanup_confirmed: cleanup_confirmed.clone(),
            started: false,
            capture_output,
        };
        let handle = Self {
            commands: Some(commands),
            output: Some(Mutex::new(output)),
            worker: None,
            poller,
            stop,
            alive,
            cleanup_confirmed,
            budget: Arc::new(AtomicUsize::new(0)),
            process_id,
            shell_name,
            foreground_process,
            snapshot,
            snapshot_requested,
            status,
        };
        Ok((handle, worker))
    }

    fn spawn_owned(
        options: &PtyOptions,
        size: WindowSize,
        terminal: Terminal,
        capture_output: bool,
    ) -> io::Result<Self> {
        let (mut handle, mut worker) =
            Self::prepare_owned(options.clone(), size, terminal, capture_output)?;
        // Preserve v1's synchronous child/setup errors and immediately usable PID.
        let mut backend = platform::Pty::spawn(&worker.options, size)?;
        worker.started = true;
        unsafe {
            backend.register(&handle.poller, Event::readable(0), PollMode::Oneshot)?;
        }
        handle
            .process_id
            .store(backend.process_id(), Ordering::Release);
        handle.worker = Some(
            thread::Builder::new()
                .name("bed-terminal-pty".into())
                .spawn(move || {
                    let _ = worker.run_backend(backend, || false);
                })?,
        );
        Ok(handle)
    }

    pub fn snapshot(&self) -> Arc<TerminalSnapshot> {
        self.snapshot.lock().unwrap().clone()
    }

    /// Coalesces requests; the worker publishes at most one snapshot per request.
    pub fn request_snapshot(&self) {
        if !self.snapshot_requested.swap(true, Ordering::AcqRel) {
            let _ = self.poller.notify();
        }
    }

    pub fn command(&self, command: TerminalCommand) -> io::Result<()> {
        self.try_command(command).map_err(|(_, error)| error)
    }

    /// Admit a command without cloning its payload. An error always returns the
    /// original, unqueued command, so callers can retry backpressure safely.
    pub fn try_command(
        &self,
        command: TerminalCommand,
    ) -> Result<(), (TerminalCommand, io::Error)> {
        let reserve = match &command {
            TerminalCommand::Key(event) => {
                let bytes = event.text.as_ref().map_or(0, String::len);
                Some(bytes.saturating_mul(12).saturating_add(256))
            }
            TerminalCommand::Paste(text) => Some(text.len().saturating_add(12)),
            TerminalCommand::Text(text) => Some(text.len().saturating_mul(12).saturating_add(32)),
            _ => None,
        };
        if let Some(bytes) = reserve {
            if !self.is_alive() {
                return Err((
                    command,
                    io::Error::new(io::ErrorKind::NotConnected, "Terminal session ended"),
                ));
            }
            if bytes > MAX_WRITE_BYTES {
                return Err((
                    command,
                    io::Error::new(io::ErrorKind::InvalidInput, "Terminal input exceeds 1 MiB"),
                ));
            }
        }
        let reservation = match reserve
            .map(|bytes| reserve_bytes(&self.budget, bytes, MAX_QUEUED_WRITE_BYTES))
            .transpose()
        {
            Ok(reservation) => reservation,
            Err(error) => return Err((command, error)),
        };
        self.try_send(PtyCommand::Terminal(command, reservation))
            .map_err(|(queued, error)| match queued {
                PtyCommand::Terminal(command, _) => (command, error),
                _ => unreachable!("try_command sends only terminal commands"),
            })
    }

    /// 0 until a prepared worker starts; otherwise the original shell PID.
    pub fn process_id(&self) -> u32 {
        self.process_id.load(Ordering::Acquire)
    }

    pub fn shell_name(&self) -> &str {
        &self.shell_name
    }

    /// Cached by the PTY worker; reading a tab label never queries the OS.
    pub fn foreground_process_name(&self) -> Option<String> {
        self.foreground_process.lock().unwrap().clone()
    }

    /// Whether input is accepted. False is not proof that owner cleanup has
    /// finished; externally owned tasks complete only when `run` returns.
    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Acquire)
    }

    /// True only after no child was started, or actual child/PTY/group cleanup
    /// succeeded. Task completion/cancellation alone is not that evidence.
    /// Read final `poll()` events as well before deciding whether to restart.
    pub fn cleanup_confirmed(&self) -> bool {
        self.cleanup_confirmed.load(Ordering::Acquire)
    }

    pub fn write(&self, bytes: &[u8]) -> io::Result<()> {
        if !self.is_alive() {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "Terminal session ended",
            ));
        }
        if bytes.is_empty() {
            return Ok(());
        }
        if bytes.len() > MAX_WRITE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Terminal write exceeds 1 MiB",
            ));
        }
        let reservation = reserve_bytes(&self.budget, bytes.len(), MAX_QUEUED_WRITE_BYTES)?;
        self.send(PtyCommand::Write(QueuedWrite {
            bytes: bytes.to_vec(),
            offset: 0,
            _reservation: reservation,
        }))
    }

    pub fn resize(&self, size: WindowSize) -> io::Result<()> {
        validate_size(size)?;
        self.send(PtyCommand::Resize(size))
    }

    fn send(&self, command: PtyCommand) -> io::Result<()> {
        self.try_send(command).map_err(|(_, error)| error)
    }

    fn try_send(&self, command: PtyCommand) -> Result<(), (PtyCommand, io::Error)> {
        let Some(sender) = &self.commands else {
            return Err((
                command,
                io::Error::new(io::ErrorKind::NotConnected, "Terminal session ended"),
            ));
        };
        match sender.try_send(command) {
            Ok(()) => {
                // Acceptance is final: a notification failure must not invite
                // duplicate input. The worker's bounded poll still observes it.
                let _ = self.poller.notify();
                Ok(())
            }
            Err(mpsc::TrySendError::Full(command)) => Err((
                command,
                io::Error::new(io::ErrorKind::WouldBlock, "Terminal command queue is full"),
            )),
            Err(mpsc::TrySendError::Disconnected(command)) => Err((
                command,
                io::Error::new(io::ErrorKind::BrokenPipe, "Terminal worker stopped"),
            )),
        }
    }

    /// At most 1 MiB per poll keeps a busy child from monopolizing a UI frame.
    pub fn poll(&mut self) -> Vec<PtyEvent> {
        // Lock lifecycle publication before draining output, so a concurrent
        // final read cannot make Exited overtake its captured bytes.
        let mut status = self.status.lock().unwrap();
        let mut events: Vec<_> = self
            .output
            .iter()
            .flat_map(|receiver| {
                receiver
                    .lock()
                    .unwrap()
                    .try_iter()
                    .take(OUTPUT_QUEUE_CHUNKS)
                    .collect::<Vec<_>>()
            })
            .collect();
        events.extend(status.drain(..));
        events
    }

    /// Request shutdown. A prepared/external worker is never joined here: its
    /// admitting owner must wait for `TerminalWorker::run` to finish cleanup.
    /// The legacy `spawn` APIs retain their bounded thread-join behavior.
    pub fn shutdown(&mut self) {
        self.alive.store(false, Ordering::Release);
        self.stop.store(true, Ordering::Release);
        // Dropping both channels releases queued reservations and unblocks an
        // output-capturing worker before waiting for process cleanup.
        self.commands = None;
        self.output = None;
        let _ = self.poller.notify();
        let Some(worker) = self.worker.take() else {
            return;
        };
        let deadline = Instant::now() + Duration::from_millis(500);
        loop {
            if worker.is_finished() {
                let _ = worker.join();
                return;
            }
            if Instant::now() >= deadline {
                return;
            }
            // A detached slow cleanup owns all of its data and OS resources.
            thread::park_timeout(Duration::from_millis(2));
        }
    }
}

impl TerminalWorker {
    /// Start the child on this thread and own all I/O, parsing and cleanup here.
    /// Cancellation is checked at most every 20 ms while active, or 100 ms while
    /// retaining an exited transcript (plus bounded I/O batches and OS cleanup).
    /// No thread is spawned, joined or detached by this method.
    ///
    /// After child exit the worker continues serving scroll/selection commands
    /// until cancellation or handle shutdown/drop. `Exited` is published only
    /// after PTY/foreground cleanup; `is_alive() == false` alone is not that proof.
    pub fn run(self, cancelled: impl Fn() -> bool) -> io::Result<()> {
        if self.stop.load(Ordering::Acquire) || cancelled() {
            return Ok(());
        }
        match platform::Pty::spawn(&self.options, self.size) {
            Ok(backend) => self.run_backend(backend, cancelled),
            Err(error) => {
                self.status
                    .lock()
                    .unwrap()
                    .push_back(PtyEvent::Error(error.to_string()));
                Err(error)
            }
        }
    }

    fn run_backend(
        mut self,
        mut backend: platform::Pty,
        cancelled: impl Fn() -> bool,
    ) -> io::Result<()> {
        self.started = true;
        self.process_id
            .store(backend.process_id(), Ordering::Release);
        // SAFETY: the backend retains its registration and deregisters before
        // closing the descriptor, including early errors and unwinding.
        let result =
            unsafe { backend.register(&self.poller, Event::readable(0), PollMode::Oneshot) }
                .and_then(|()| {
                    run_worker(
                        &mut backend,
                        &self.commands,
                        &self.output,
                        &self.stop,
                        &self.poller,
                        &self.foreground_process,
                        &mut self.terminal,
                        &self.snapshot,
                        &self.snapshot_requested,
                        &self.alive,
                        &self.cleanup_confirmed,
                        &self.status,
                        &cancelled,
                        self.capture_output,
                    )
                });
        // This is the admitting task's cleanup, never a detached follow-up.
        let cleanup = backend.shutdown();
        self.cleanup_confirmed
            .store(backend.cleanup_confirmed(), Ordering::Release);
        self.terminal.finish_output();
        *self.snapshot.lock().unwrap() = self.terminal.snapshot();
        self.alive.store(false, Ordering::Release);
        let result = match (result, cleanup) {
            (Err(run), Err(cleanup)) => Err(io::Error::new(
                run.kind(),
                format!("{run}; terminal cleanup also failed: {cleanup}"),
            )),
            (Err(error), _) | (_, Err(error)) => Err(error),
            (Ok(()), Ok(())) => Ok(()),
        };
        if let Err(error) = &result {
            self.status
                .lock()
                .unwrap()
                .push_back(PtyEvent::Error(error.to_string()));
        }
        result
    }
}

impl Drop for TerminalWorker {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Release);
        if !self.started {
            self.cleanup_confirmed.store(true, Ordering::Release);
        }
    }
}

impl Drop for TerminalPty {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn validate_size(size: WindowSize) -> io::Result<()> {
    let valid = 1..=i16::MAX as u16;
    if valid.contains(&size.num_cols) && valid.contains(&size.num_lines) {
        return Ok(());
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        "Terminal dimensions must be 1..32767",
    ))
}

fn keyboard_modes(terminal: &Terminal) -> KeyboardModes {
    let modes = terminal.modes();
    KeyboardModes {
        application_cursor: modes.app_cursor,
        application_keypad: modes.app_keypad,
        newline: modes.newline_mode,
        kitty: modes.kitty_keyboard,
        modify_other_keys: modes.modify_other_keys,
    }
}

fn enqueue_reserved(
    writes: &mut VecDeque<QueuedWrite>,
    mut reservation: InputReservation,
    bytes: Vec<u8>,
) -> io::Result<()> {
    if bytes.is_empty() {
        return Ok(());
    }
    if bytes.len() > reservation.bytes {
        return Err(io::Error::other(
            "Keyboard encoder exceeded its reserved byte budget",
        ));
    }
    reservation
        .budget
        .fetch_sub(reservation.bytes - bytes.len(), Ordering::AcqRel);
    reservation.bytes = bytes.len();
    writes.push_back(QueuedWrite {
        bytes,
        offset: 0,
        _reservation: reservation,
    });
    Ok(())
}

fn enqueue_reply(
    writes: &mut VecDeque<QueuedWrite>,
    budget: &Arc<AtomicUsize>,
    bytes: Vec<u8>,
) -> io::Result<()> {
    let reservation = reserve_bytes(budget, bytes.len(), MAX_QUEUED_REPLY_BYTES)?;
    enqueue_reserved(writes, reservation, bytes)
}

fn parse_output(
    terminal: &mut Terminal,
    bytes: &[u8],
    writes: &mut VecDeque<QueuedWrite>,
    budget: &Arc<AtomicUsize>,
    child_alive: bool,
) -> io::Result<()> {
    for event in terminal.feed(bytes) {
        if let TerminalEvent::Write(bytes) = event
            && child_alive
        {
            enqueue_reply(writes, budget, bytes)?;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)] // Keep the worker's ordered state transition together.
fn run_worker(
    backend: &mut platform::Pty,
    commands: &mpsc::Receiver<PtyCommand>,
    output: &mpsc::SyncSender<PtyEvent>,
    stop: &AtomicBool,
    poller: &Arc<Poller>,
    foreground_process: &Mutex<Option<String>>,
    terminal: &mut Terminal,
    snapshot: &Mutex<Arc<TerminalSnapshot>>,
    snapshot_requested: &AtomicBool,
    alive: &AtomicBool,
    cleanup_confirmed: &AtomicBool,
    status: &Mutex<VecDeque<PtyEvent>>,
    cancelled: &impl Fn() -> bool,
    capture_output: bool,
) -> io::Result<()> {
    let mut writes: VecDeque<QueuedWrite> = VecDeque::new();
    // Protocol replies cannot consume reservations already accepted from the
    // user. Backpressure pauses reads while those replies wait for the child.
    let reply_budget = Arc::new(AtomicUsize::new(0));
    let mut events = Events::new();
    let mut child_exited = false;
    let mut stopped = false;
    let mut exit_deadline = None;
    let mut read_closed = false;
    let mut exited = false;
    let mut next_title_poll = Instant::now();
    while !stop.load(Ordering::Acquire) && !cancelled() {
        if !exited
            && !child_exited
            && let Some(ChildEvent::Exited) = backend.next_child_event()?
        {
            child_exited = true;
            exit_deadline = Some(Instant::now() + Duration::from_millis(500));
            alive.store(false, Ordering::Release);
            writes.clear();
        }
        if !exited && Instant::now() >= next_title_poll {
            *foreground_process.lock().unwrap() = backend.foreground_process_name();
            next_title_poll = Instant::now() + PROCESS_TITLE_INTERVAL;
        }
        for _ in 0..COMMAND_CAPACITY {
            match commands.try_recv() {
                Ok(PtyCommand::Write(bytes)) => {
                    if !exited && !child_exited {
                        writes.push_back(bytes);
                    }
                }
                Ok(PtyCommand::Resize(size)) => {
                    terminal.resize_with_pixels(
                        size.num_cols as usize,
                        size.num_lines as usize,
                        size.cell_width as u32,
                        size.cell_height as u32,
                    );
                    if !exited && !child_exited {
                        backend.resize(size)?;
                    }
                }
                Ok(PtyCommand::Terminal(command, reservation)) => {
                    match command {
                        TerminalCommand::Key(_)
                        | TerminalCommand::Paste(_)
                        | TerminalCommand::Text(_)
                            if !exited && !child_exited =>
                        {
                            let bytes = match command {
                                TerminalCommand::Key(event) => {
                                    encode_key(&event, keyboard_modes(terminal))
                                }
                                TerminalCommand::Paste(text) => {
                                    encode_paste(&text, terminal.modes().bracket_paste)
                                }
                                TerminalCommand::Text(text) => {
                                    encode_text(&text, keyboard_modes(terminal))
                                }
                                _ => unreachable!("Input command matched above"),
                            };
                            if !bytes.is_empty() {
                                terminal.scroll_display(-(terminal.display_offset() as i32));
                                if terminal.modes().local_echo {
                                    terminal.feed_echo(&bytes);
                                }
                                enqueue_reserved(
                                    &mut writes,
                                    reservation.expect("Input command reserves bytes"),
                                    bytes,
                                )?;
                            }
                        }
                        TerminalCommand::Theme(theme) => {
                            terminal.set_theme(&theme);
                        }
                        TerminalCommand::Scroll(lines) => {
                            terminal.scroll_display(lines);
                        }
                        TerminalCommand::SelectionStart { col, row, snap } => {
                            terminal.select_start(col, row, snap)
                        }
                        TerminalCommand::SelectionExtend {
                            col,
                            row,
                            rectangular,
                            done,
                        } => terminal.select_extend(col, row, rectangular, done),
                        TerminalCommand::ClearSelection => terminal.clear_selection(),
                        TerminalCommand::SelectAll => terminal.select_all(),
                        TerminalCommand::Focus(focused)
                            if !exited && !child_exited && terminal.modes().focus_reporting =>
                        {
                            enqueue_reply(
                                &mut writes,
                                &reply_budget,
                                if focused {
                                    b"\x1b[I".to_vec()
                                } else {
                                    b"\x1b[O".to_vec()
                                },
                            )?
                        }
                        TerminalCommand::Stop if !exited && !child_exited => {
                            // Request termination here, then drain final output.
                            // shutdown closes the PTY and reaps before Exited is
                            // published; the transcript owner remains available.
                            backend.request_stop();
                            child_exited = true;
                            stopped = true;
                            exit_deadline = Some(Instant::now() + Duration::from_millis(500));
                            alive.store(false, Ordering::Release);
                            writes.clear();
                        }
                        _ => {}
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => return Ok(()),
            }
        }
        let mut drained = read_closed;
        // Fair, bounded batches keep command latency independent of child output.
        for _ in 0..16 {
            if read_closed
                || exited
                || (!child_exited && reply_budget.load(Ordering::Acquire) >= REPLY_READ_WATERMARK)
            {
                break;
            }
            let mut bytes = vec![0; OUTPUT_CHUNK_BYTES];
            match backend.io().read(&mut bytes) {
                Ok(0) => {
                    drained = true;
                    read_closed = true;
                    break;
                }
                Ok(length) => {
                    bytes.truncate(length);
                    parse_output(terminal, &bytes, &mut writes, &reply_budget, !child_exited)?;
                    if capture_output && output.send(PtyEvent::Output(bytes)).is_err() {
                        return Ok(());
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    drained = true;
                    break;
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                #[cfg(unix)]
                Err(error) if error.raw_os_error() == Some(libc::EIO) => {
                    drained = true;
                    read_closed = true;
                    break;
                }
                Err(error) => return Err(error),
            }
        }
        if !exited {
            if !child_exited && let Some(ChildEvent::Exited) = backend.next_child_event()? {
                child_exited = true;
                exit_deadline = Some(Instant::now() + Duration::from_millis(500));
                alive.store(false, Ordering::Release);
                writes.clear();
            }
            if child_exited
                && (drained || exit_deadline.is_some_and(|deadline| Instant::now() >= deadline))
            {
                terminal.finish_output();
                *snapshot.lock().unwrap() = terminal.snapshot();
                alive.store(false, Ordering::Release);
                backend.shutdown()?;
                cleanup_confirmed.store(backend.cleanup_confirmed(), Ordering::Release);
                status
                    .lock()
                    .unwrap()
                    .push_back(PtyEvent::Exited(if stopped {
                        None
                    } else {
                        backend.exit_status()
                    }));
                writes.clear();
                exited = true;
                if capture_output {
                    return Ok(());
                }
            }
        }
        for event in terminal.flush_sync() {
            if let TerminalEvent::Write(bytes) = event
                && !exited
                && !child_exited
            {
                enqueue_reply(&mut writes, &reply_budget, bytes)?;
            }
        }
        if snapshot_requested.swap(false, Ordering::AcqRel) {
            *snapshot.lock().unwrap() = terminal.snapshot();
        }
        if !exited && !child_exited {
            for _ in 0..64 {
                let Some(write) = writes.front_mut() else {
                    break;
                };
                let remaining = &write.bytes[write.offset..];
                match backend.io().write(&remaining[..remaining.len().min(4096)]) {
                    Ok(0) => break,
                    Ok(length) => {
                        write.offset += length;
                        if write.offset == write.bytes.len() {
                            writes.pop_front();
                        }
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    #[cfg(unix)]
                    Err(error) if error.raw_os_error() == Some(libc::EIO) => {
                        // The slave can close before the exit is observable. A
                        // final query may still have queued a reply; discard
                        // it and keep draining output until the child is reaped.
                        writes.clear();
                        break;
                    }
                    Err(error) => return Err(error),
                }
            }
            backend.reregister(
                poller,
                Event::new(
                    0,
                    !read_closed && reply_budget.load(Ordering::Acquire) < REPLY_READ_WATERMARK,
                    !writes.is_empty(),
                ),
                PollMode::Oneshot,
            )?;
        } else if !exited {
            backend.reregister(poller, Event::readable(0), PollMode::Oneshot)?;
        }
        events.clear();
        let interval = if exited {
            EXITED_POLL_INTERVAL
        } else {
            ACTIVE_POLL_INTERVAL
        };
        match poller.wait(&mut events, Some(interval)) {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

#[cfg(unix)]
mod platform {
    use super::*;
    use std::{
        ffi::CStr,
        fs::File,
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::process::{CommandExt, ExitStatusExt},
        },
        process::{Child, Command},
    };

    // The worker exclusively owns both the process and the master descriptor.
    // waitid observes exit without reaping: the leader PID reserves its PGID
    // until every cleanup signal is sent. A debugger stop is not an exit.
    pub struct Pty {
        process: Child,
        master: Option<File>,
        exit_observed: bool,
        leader_reserved: bool,
        exit_status: Option<ExitStatus>,
        cleanup_confirmed: bool,
        registration: Option<Arc<Poller>>,
        foreground_group: Option<libc::pid_t>,
        // Stop runs at most once before final shutdown. Keep signal failures
        // until reaping and exact group-disappearance checks can resolve races.
        stop_errors: Vec<(libc::pid_t, io::Error)>,
    }

    impl Pty {
        pub fn spawn(options: &PtyOptions, size: WindowSize) -> io::Result<Self> {
            let account = account()?;
            let inherited_shell = std::env::var("SHELL").unwrap_or_else(|_| account[2].clone());
            let launch = shell(options)?;
            let mut process = Command::new(&launch.program);
            process.args(&launch.args);
            if launch.login {
                let basename = PathBuf::from(&launch.program);
                process.arg0(format!(
                    "-{}",
                    basename.file_name().unwrap_or_default().to_string_lossy()
                ));
            }
            if let Some(cwd) = options.working_directory.as_deref() {
                process.current_dir(cwd);
            }
            for name in ["COLUMNS", "LINES", "TERMCAP"] {
                process.env_remove(name);
            }
            process
                .env("USER", &account[0])
                .env("LOGNAME", &account[0])
                .env("HOME", &account[1])
                .env("SHELL", inherited_shell)
                .env("TERM", "xterm-256color")
                .env("TERM_PROGRAM", "bed");
            for name in &options.env_remove {
                process.env_remove(name);
            }
            process.envs(&options.env);

            let mut descriptors = [-1; 2];
            let mut dimensions = dimensions(size);
            // SAFETY: openpty fills two descriptors and copies the winsize. Each
            // successful descriptor immediately receives a unique File owner.
            let result = unsafe {
                libc::openpty(
                    &mut descriptors[0],
                    &mut descriptors[1],
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    &mut dimensions,
                )
            };
            if result != 0 {
                return Err(io::Error::last_os_error());
            }
            let master = unsafe { File::from_raw_fd(descriptors[0]) };
            let slave = unsafe { File::from_raw_fd(descriptors[1]) };
            for fd in [master.as_raw_fd(), slave.as_raw_fd()] {
                let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
                if flags < 0
                    || unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) } < 0
                {
                    return Err(io::Error::last_os_error());
                }
            }
            let fd = master.as_raw_fd();
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
            if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
            {
                return Err(io::Error::last_os_error());
            }
            process
                .stdin(slave.try_clone()?)
                .stdout(slave.try_clone()?)
                .stderr(slave);
            // SAFETY: only async-signal-safe operations run in the child before
            // exec. Command has prepared all allocation, environment and stdio.
            unsafe {
                process.pre_exec(|| {
                    if libc::setsid() == -1
                        || libc::ioctl(libc::STDIN_FILENO, libc::TIOCSCTTY as _, 0) == -1
                    {
                        return Err(io::Error::last_os_error());
                    }
                    for signal in [
                        libc::SIGHUP,
                        libc::SIGINT,
                        libc::SIGQUIT,
                        libc::SIGTERM,
                        libc::SIGALRM,
                        libc::SIGCHLD,
                    ] {
                        libc::signal(signal, libc::SIG_DFL);
                    }
                    Ok(())
                });
            }
            Ok(Pty {
                process: process.spawn()?,
                master: Some(master),
                exit_observed: false,
                leader_reserved: true,
                exit_status: None,
                cleanup_confirmed: false,
                registration: None,
                foreground_group: None,
                stop_errors: Vec::new(),
            })
        }

        pub fn process_id(&self) -> u32 {
            self.process.id()
        }

        pub fn foreground_process_name(&self) -> Option<String> {
            let group = unsafe { libc::tcgetpgrp(self.master.as_ref()?.as_raw_fd()) };
            crate::process_title::process_name(if group > 0 {
                group
            } else {
                self.process.id() as _
            })
        }

        pub fn resize(&mut self, size: WindowSize) -> io::Result<()> {
            let master = self
                .master
                .as_ref()
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "PTY closed"))?;
            if unsafe { libc::ioctl(master.as_raw_fd(), libc::TIOCSWINSZ, &dimensions(size)) } == -1
            {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }

        pub fn io(&mut self) -> &mut File {
            self.master.as_mut().expect("worker owns an open PTY")
        }

        pub unsafe fn register(
            &mut self,
            poller: &Arc<Poller>,
            event: Event,
            mode: PollMode,
        ) -> io::Result<()> {
            if self.registration.is_some() {
                return Ok(());
            }
            unsafe { poller.add_with_mode(&*self.io(), event, mode)? };
            self.registration = Some(poller.clone());
            Ok(())
        }

        pub fn reregister(
            &mut self,
            poller: &Arc<Poller>,
            event: Event,
            mode: PollMode,
        ) -> io::Result<()> {
            poller.modify_with_mode(self.io(), event, mode)
        }

        fn deregister(&mut self) -> io::Result<()> {
            if let Some(poller) = &self.registration {
                if let Some(master) = &self.master {
                    poller.delete(master).map_err(|error| {
                        cleanup_error(
                            format!(
                                "shutdown: deregister PTY fd {} for shell PID {}",
                                master.as_raw_fd(),
                                self.process.id(),
                            ),
                            error,
                        )
                    })?;
                }
                self.registration = None;
            }
            Ok(())
        }

        pub fn next_child_event(&mut self) -> io::Result<Option<ChildEvent>> {
            let leader = self.process.id() as libc::pid_t;
            // Capture foreground groups only from this live PTY/session. The
            // cached number alone is never authority to signal a process group.
            if self.leader_reserved
                && let Some(master) = &self.master
            {
                let fd = master.as_raw_fd();
                let group = unsafe { libc::tcgetpgrp(fd) };
                if group > 0
                    && group != leader
                    && ((unsafe { libc::getsid(group) }) == leader
                        || (unsafe { libc::tcgetsid(fd) }) == leader)
                {
                    self.foreground_group = Some(group);
                } else if group == leader {
                    self.foreground_group = None;
                }
            }
            if !self.leader_reserved {
                return Err(io::Error::other("Terminal child wait ownership was lost"));
            }
            let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
            let result = unsafe {
                libc::waitid(
                    libc::P_PID,
                    self.process.id(),
                    &mut info,
                    libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                )
            };
            if result == -1 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() == Some(libc::EINTR) {
                    return Ok(None);
                }
                if error.raw_os_error() == Some(libc::ECHILD) {
                    self.leader_reserved = false;
                }
                return Err(cleanup_error(
                    format!("observe shell PID {leader} with waitid(WNOWAIT)"),
                    error,
                ));
            }
            if unsafe { info.si_pid() } == 0 {
                return Ok(None);
            }
            let first = !self.exit_observed;
            self.exit_observed = true;
            Ok(first.then_some(ChildEvent::Exited))
        }

        fn signal_foreground(&self, group: libc::pid_t, signal: libc::c_int) -> io::Result<()> {
            if group_finished(group) {
                return Ok(());
            }
            let leader = self.process.id() as libc::pid_t;
            let verified = self.leader_reserved
                && self.master.as_ref().is_some_and(|master| {
                    let fd = master.as_raw_fd();
                    (unsafe { libc::getsid(group) }) == leader
                        || ((unsafe { libc::tcgetsid(fd) }) == leader
                            && (unsafe { libc::tcgetpgrp(fd) }) == group)
                });
            if !verified {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!(
                        "refusing signal {signal} to cached foreground PGID {group}: live PTY/session {leader} ownership could not be verified"
                    ),
                ));
            }
            signal_group(group, signal)
        }

        pub fn exit_status(&self) -> Option<ExitStatus> {
            self.exit_status
        }

        pub fn cleanup_confirmed(&self) -> bool {
            self.cleanup_confirmed
        }

        // Signal/kill on the owner thread but do not block reaping while the
        // master remains open. On macOS a killed shell can stay in kernel tty
        // output drain until the master is read or closed. Stop returns to the
        // worker's final read loop; shutdown closes first, then reaps/confirms.
        pub fn request_stop(&mut self) {
            let observed = self.next_child_event();
            let leader = self.process.id() as libc::pid_t;
            let foreground = self.foreground_group;
            let with_context = |group, error| {
                (
                    group,
                    cleanup_error(
                        format!("request_stop: shell PID {leader}, foreground PGID {foreground:?}"),
                        error,
                    ),
                )
            };
            let mut errors = Vec::new();
            if let Err(error) = observed {
                errors.push(with_context(leader, error));
            }
            if let Some(foreground) = foreground {
                // Keep the shell alive to reap its job first. Killing both at
                // once can leave an orphan zombie in a live process group on
                // Linux hosts whose init does not promptly reap grandchildren.
                for signal in [libc::SIGHUP, libc::SIGTERM] {
                    if let Err(failure) = self.signal_foreground(foreground, signal) {
                        errors.push(with_context(foreground, failure));
                    }
                }
                let grace = Instant::now() + Duration::from_millis(100);
                while !group_finished(foreground) && Instant::now() < grace {
                    if let Err(error) = self.next_child_event() {
                        errors.push(with_context(leader, error));
                        break;
                    }
                    thread::sleep(Duration::from_millis(5));
                }
                if !group_finished(foreground)
                    && let Err(failure) = self.signal_foreground(foreground, libc::SIGKILL)
                {
                    errors.push(with_context(foreground, failure));
                }
                // Allow the still-live shell to reap its job before stopping
                // the shell itself. A tty-draining job may need the final read
                // loop or master closure first; shutdown confirms it afterward.
                let deadline = Instant::now() + Duration::from_millis(500);
                while !group_finished(foreground) && Instant::now() < deadline {
                    if let Err(error) = self.next_child_event() {
                        errors.push(with_context(leader, error));
                        break;
                    }
                    thread::sleep(Duration::from_millis(5));
                }
                if group_finished(foreground) {
                    self.foreground_group = None;
                }
            }
            if self.leader_reserved && !self.exit_observed {
                for signal in [libc::SIGHUP, libc::SIGTERM] {
                    if let Err(failure) = signal_group(leader, signal) {
                        errors.push(with_context(leader, failure));
                    }
                }
                let grace = Instant::now() + Duration::from_millis(100);
                while !self.exit_observed && self.leader_reserved && Instant::now() < grace {
                    if let Err(error) = self.next_child_event() {
                        errors.push(with_context(leader, error));
                        break;
                    }
                    if !self.exit_observed {
                        thread::sleep(Duration::from_millis(5));
                    }
                }
            }
            // Even an exited leader stays unreaped here, reserving the PGID
            // while we stop any remaining members. No signals follow final reap.
            if self.leader_reserved {
                if let Err(failure) = signal_group(leader, libc::SIGKILL) {
                    errors.push(with_context(leader, failure));
                }
                // ESRCH is harmless if it exited between the last poll and kill.
                // Do not wait here: the caller must drain or close the PTY first.
                if !self.exit_observed
                    && let Err(failure) = self.process.kill()
                    && failure.raw_os_error() != Some(libc::ESRCH)
                {
                    errors.push(with_context(
                        leader,
                        cleanup_error(format!("kill shell PID {leader} with SIGKILL"), failure),
                    ));
                }
            }
            self.stop_errors.extend(errors);
        }

        // The master must already be closed: see request_stop's tty invariant.
        // All signals precede this final reap. Afterwards numbers can be reused;
        // only non-mutating disappearance probes are safe.
        fn reap_and_confirm(&mut self) -> io::Result<()> {
            debug_assert!(self.master.is_none());
            let leader = self.process.id() as libc::pid_t;
            if !self.leader_reserved {
                return Err(io::Error::other(format!(
                    "reap_and_confirm: wait ownership of shell PID {leader} was lost"
                )));
            }
            loop {
                let mut status = 0;
                let result = unsafe { libc::waitpid(leader, &mut status, 0) };
                if result > 0 && (libc::WIFEXITED(status) || libc::WIFSIGNALED(status)) {
                    self.exit_status = Some(ExitStatus::from_raw(status));
                    self.leader_reserved = false;
                    break;
                }
                if result == -1 {
                    let failure = io::Error::last_os_error();
                    if failure.raw_os_error() == Some(libc::EINTR) {
                        continue;
                    }
                    if failure.raw_os_error() == Some(libc::ECHILD) {
                        self.leader_reserved = false;
                    }
                    return Err(cleanup_error(
                        format!("reap_and_confirm: waitpid for shell PID {leader}"),
                        failure,
                    ));
                }
            }
            for group in [self.foreground_group, Some(leader)].into_iter().flatten() {
                let deadline = Instant::now() + Duration::from_millis(500);
                while !group_finished(group) && Instant::now() < deadline {
                    thread::sleep(Duration::from_millis(5));
                }
                if !group_finished(group) {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        format!(
                            "reap_and_confirm: PGID {group} for shell PID {leader} is still present; refusing post-reap signals"
                        ),
                    ));
                }
            }
            self.foreground_group = None;
            Ok(())
        }

        pub fn shutdown(&mut self) -> io::Result<()> {
            // Exited transcript owners can live indefinitely. Once the master
            // is closed, never signal an old PID/group again after PID reuse.
            if self.master.is_none() {
                return Ok(());
            }
            // Delete before closing, including when the worker unwinds or a
            // legacy thread fails to spawn after registering its descriptor.
            let deregister = self.deregister();
            self.request_stop();
            // Closing before waitpid is essential on macOS: even SIGKILL can
            // leave a shell exiting in tty output drain while this stays open.
            self.master = None;
            let cleanup = self.reap_and_confirm();
            // macOS can reject another signal while an already-killed shell
            // is exiting. Resolve that race only after the owned child is
            // reaped and both cleanup and this error's exact target group have
            // been confirmed gone. Never suppress a still-present/denied group.
            let confirmed = cleanup.is_ok() && self.exit_status.is_some();
            let stop = self
                .stop_errors
                .drain(..)
                .find(|(group, _)| !confirmed || !group_finished(*group))
                .map_or(Ok(()), |(_, error)| Err(error));
            let result = match (deregister.and(stop), cleanup) {
                (Err(stop), Err(cleanup)) => Err(io::Error::new(
                    stop.kind(),
                    format!("{stop}; terminal reap also failed: {cleanup}"),
                )),
                (Err(error), _) | (_, Err(error)) => Err(error),
                (Ok(()), Ok(())) => Ok(()),
            };
            self.cleanup_confirmed = result.is_ok();
            result
        }
    }

    impl Drop for Pty {
        fn drop(&mut self) {
            let _ = self.shutdown();
        }
    }

    fn signal_group(group: libc::pid_t, signal: libc::c_int) -> io::Result<()> {
        if unsafe { libc::kill(-group, signal) } == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            Ok(())
        } else {
            Err(cleanup_error(
                format!("signal process group {group} with signal {signal}"),
                error,
            ))
        }
    }

    fn cleanup_error(operation: String, error: io::Error) -> io::Error {
        io::Error::new(error.kind(), format!("{operation}: {error}"))
    }

    fn group_finished(group: libc::pid_t) -> bool {
        (unsafe { libc::kill(-group, 0) }) == -1
            && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
    }

    pub fn shell(options: &PtyOptions) -> io::Result<TerminalShell> {
        if let Some(shell) = &options.shell {
            return Ok(shell.clone());
        }
        let account = account()?;
        let program = std::env::var("SHELL").unwrap_or_else(|_| account[2].clone());
        Ok(TerminalShell {
            args: if program.contains("zsh") {
                vec!["+o".to_owned(), "PROMPT_SP".to_owned()]
            } else {
                Vec::new()
            },
            program,
            login: true,
        })
    }

    fn dimensions(size: WindowSize) -> libc::winsize {
        libc::winsize {
            ws_col: size.num_cols,
            ws_row: size.num_lines,
            ws_xpixel: size.cell_width.saturating_mul(size.num_cols),
            ws_ypixel: size.cell_height.saturating_mul(size.num_lines),
        }
    }

    fn account() -> io::Result<[String; 3]> {
        let suggested = unsafe { libc::sysconf(libc::_SC_GETPW_R_SIZE_MAX) };
        let mut buffer = vec![
            0;
            usize::try_from(suggested)
                .unwrap_or(16 * 1024)
                .clamp(1024, 1024 * 1024)
        ];
        loop {
            let mut entry = std::mem::MaybeUninit::<libc::passwd>::uninit();
            let mut found = std::ptr::null_mut();
            let error = unsafe {
                libc::getpwuid_r(
                    libc::getuid(),
                    entry.as_mut_ptr(),
                    buffer.as_mut_ptr(),
                    buffer.len(),
                    &mut found,
                )
            };
            if error == libc::ERANGE && buffer.len() < 4 * 1024 * 1024 {
                buffer.resize(buffer.len() * 2, 0);
                continue;
            }
            if error != 0 {
                return Err(io::Error::from_raw_os_error(error));
            }
            if found.is_null() {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    "No shell user entry",
                ));
            }
            let entry = unsafe { entry.assume_init() };
            let mut fields = [String::new(), String::new(), String::new()];
            for (out, pointer) in
                fields
                    .iter_mut()
                    .zip([entry.pw_name, entry.pw_dir, entry.pw_shell])
            {
                if pointer.is_null() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Incomplete shell user entry",
                    ));
                }
                *out = unsafe { CStr::from_ptr(pointer) }
                    .to_string_lossy()
                    .into_owned();
            }
            if fields[2].is_empty() {
                fields[2] = "/bin/sh".to_owned();
            }
            return Ok(fields);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn size() -> WindowSize {
        WindowSize {
            num_cols: 80,
            num_lines: 6,
            cell_width: 8,
            cell_height: 16,
        }
    }

    fn shell_options(script: &str) -> PtyOptions {
        PtyOptions {
            shell: Some(TerminalShell::new(
                "/bin/sh",
                vec!["-c".into(), script.into()],
            )),
            ..PtyOptions::default()
        }
    }

    fn missing_shell() -> PtyOptions {
        PtyOptions {
            shell: Some(TerminalShell::new(
                "/bed-terminal-test/no-such-executable",
                Vec::new(),
            )),
            ..PtyOptions::default()
        }
    }

    fn prepare(options: PtyOptions) -> (TerminalPty, TerminalWorker) {
        TerminalPty::prepare_terminal(options, size(), Terminal::new_text_only(80, 6)).unwrap()
    }

    fn snapshot_text(pty: &TerminalPty) -> String {
        pty.snapshot()
            .lines
            .iter()
            .map(|row| {
                row.cells
                    .iter()
                    .map(|cell| cell.text.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[track_caller]
    fn wait_until(mut ready: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !ready() {
            assert!(Instant::now() < deadline, "terminal worker timed out");
            thread::sleep(Duration::from_millis(2));
        }
    }

    // A test-owned worker, not an implementation thread. Always cancel/join on
    // assertion failure so a regression cannot leave a shell in the test host.
    struct Running {
        pty: TerminalPty,
        cancel: Arc<AtomicBool>,
        owner: Option<JoinHandle<io::Result<()>>>,
    }

    impl Running {
        fn start(options: PtyOptions) -> Self {
            let (pty, worker) = prepare(options);
            Self::from_prepared(pty, worker)
        }

        fn from_prepared(pty: TerminalPty, worker: TerminalWorker) -> Self {
            let cancel = Arc::new(AtomicBool::new(false));
            let requested = cancel.clone();
            let owner = thread::spawn(move || worker.run(|| requested.load(Ordering::Acquire)));
            Self {
                pty,
                cancel,
                owner: Some(owner),
            }
        }

        fn finish(&mut self) -> io::Result<()> {
            self.cancel.store(true, Ordering::Release);
            wait_until(|| self.owner.as_ref().unwrap().is_finished());
            self.owner
                .take()
                .unwrap()
                .join()
                .expect("PTY owner panicked")
        }

        fn wait_text(&self, expected: &str) {
            wait_until(|| {
                self.pty.request_snapshot();
                snapshot_text(&self.pty).contains(expected)
            });
        }
    }

    impl Drop for Running {
        fn drop(&mut self) {
            self.cancel.store(true, Ordering::Release);
            self.pty.shutdown();
            if let Some(owner) = self.owner.take() {
                let _ = owner.join();
            }
        }
    }

    #[test]
    fn prepared_handle_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<TerminalPty>();
    }

    #[test]
    fn prepare_does_not_start_a_child_and_cancelled_run_does_not_launch() {
        // A nonexistent executable is accepted until run, not launched by prepare.
        let (mut pty, worker) = prepare(missing_shell());
        assert_eq!(pty.process_id(), 0);
        assert_eq!(pty.shell_name(), "no-such-executable");
        assert!(pty.is_alive());
        assert!(!pty.cleanup_confirmed());
        pty.write(b"queued").unwrap();
        worker.run(|| true).unwrap();
        assert_eq!(pty.process_id(), 0);
        assert!(!pty.is_alive());
        assert!(pty.cleanup_confirmed());
        assert_eq!(pty.budget.load(Ordering::Acquire), 0);
        assert!(pty.poll().is_empty());

        let (mut pty, worker) = prepare(missing_shell());
        pty.shutdown();
        worker.run(|| false).unwrap();
        assert_eq!(pty.process_id(), 0);
        assert!(pty.cleanup_confirmed());
        assert!(pty.poll().is_empty());
    }

    #[test]
    fn prepared_spawn_failure_returns_and_reliably_publishes_the_error() {
        let (mut pty, worker) = prepare(missing_shell());
        let error = worker.run(|| false).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert!(!pty.is_alive());
        assert!(pty.cleanup_confirmed());
        assert_eq!(pty.process_id(), 0);
        assert!(matches!(pty.poll().as_slice(), [PtyEvent::Error(_)]));
        assert!(pty.poll().is_empty());
        assert!(TerminalPty::spawn(&missing_shell(), size()).is_err());
    }

    #[test]
    fn prepared_input_and_command_limits_release_rejected_and_dropped_reservations() {
        let (pty, worker) = prepare(missing_shell());
        assert_eq!(
            pty.write(&vec![0; MAX_WRITE_BYTES + 1]).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        for _ in 0..MAX_QUEUED_WRITE_BYTES / MAX_WRITE_BYTES {
            pty.write(&vec![0; MAX_WRITE_BYTES]).unwrap();
        }
        assert_eq!(
            pty.write(b"extra").unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        assert_eq!(pty.budget.load(Ordering::Acquire), MAX_QUEUED_WRITE_BYTES);
        drop(worker);
        assert_eq!(pty.budget.load(Ordering::Acquire), 0);
        assert!(!pty.is_alive());

        let (pty, worker) = prepare(missing_shell());
        for _ in 0..COMMAND_CAPACITY {
            pty.command(TerminalCommand::Scroll(1)).unwrap();
        }
        assert_eq!(
            pty.write(b"x").unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        assert_eq!(
            pty.command(TerminalCommand::Paste("x".into()))
                .unwrap_err()
                .kind(),
            io::ErrorKind::WouldBlock
        );
        assert_eq!(pty.budget.load(Ordering::Acquire), 0);
        let text = String::from("retain this input release");
        let allocation = text.as_ptr();
        let (returned, error) = pty.try_command(TerminalCommand::Text(text)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        match returned {
            TerminalCommand::Text(text) => {
                assert_eq!(text, "retain this input release");
                assert_eq!(
                    text.as_ptr(),
                    allocation,
                    "retry must return the original allocation"
                );
            }
            other => panic!("returned a different command: {other:?}"),
        }
        assert_eq!(pty.budget.load(Ordering::Acquire), 0);
        assert_eq!(
            pty.resize(size()).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        let mut invalid = size();
        invalid.num_cols = 0;
        assert_eq!(
            pty.resize(invalid).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        drop(worker);
        assert!(pty.cleanup_confirmed());
        let (returned, error) = pty.try_command(TerminalCommand::Focus(false)).unwrap_err();
        assert!(matches!(returned, TerminalCommand::Focus(false)));
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    }

    #[cfg(unix)]
    #[test]
    fn exit_observation_reserves_leader_until_final_reap() {
        let mut backend = platform::Pty::spawn(&shell_options("exit 7"), size()).unwrap();
        let pid = backend.process_id();
        wait_until(|| backend.next_child_event().unwrap().is_some());
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        assert_eq!(
            unsafe {
                libc::waitid(
                    libc::P_PID,
                    pid,
                    &mut info,
                    libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                )
            },
            0
        );
        assert_eq!(unsafe { info.si_pid() }, pid as libc::pid_t);
        assert!(!backend.cleanup_confirmed());
        backend.shutdown().unwrap();
        assert_eq!(backend.exit_status().unwrap().code(), Some(7));
        assert!(backend.cleanup_confirmed());
        backend.shutdown().unwrap();
        assert!(backend.cleanup_confirmed());
        let mut status = 0;
        assert_eq!(
            unsafe { libc::waitpid(pid as _, &mut status, libc::WNOHANG) },
            -1
        );
        assert_eq!(
            io::Error::last_os_error().raw_os_error(),
            Some(libc::ECHILD)
        );
    }

    #[cfg(unix)]
    #[test]
    fn lost_wait_ownership_cannot_be_laundered_into_confirmed_cleanup() {
        let mut backend = platform::Pty::spawn(&shell_options("exit 0"), size()).unwrap();
        let pid = backend.process_id();
        wait_until(|| backend.next_child_event().unwrap().is_some());
        // Model an external waiter taking the reserved child, as a debugger
        // could. Shutdown must fail closed, not signal a possibly reused PID.
        let mut status = 0;
        assert_eq!(
            unsafe { libc::waitpid(pid as _, &mut status, 0) },
            pid as libc::pid_t
        );
        assert!(backend.shutdown().is_err());
        assert!(!backend.cleanup_confirmed());
        backend.shutdown().unwrap();
        assert!(!backend.cleanup_confirmed());
    }

    #[cfg(unix)]
    #[test]
    fn caller_owned_shell_drains_exit_and_retains_scroll_selection_and_resize() {
        let mut running = Running::start(shell_options(
            "i=0; while [ $i -lt 20 ]; do printf 'LINE-%02d\\n' $i; i=$((i+1)); done; \
             printf '\\033[?2026hFINAL'; exit 7",
        ));
        let mut exit = None;
        wait_until(|| {
            for event in running.pty.poll() {
                match event {
                    PtyEvent::Exited(status) => exit = status,
                    PtyEvent::Error(error) => panic!("{error}"),
                    PtyEvent::Output(_) => panic!("prepared production worker captured raw output"),
                }
            }
            exit.is_some()
        });
        assert_eq!(exit.unwrap().code(), Some(7));
        assert!(!running.pty.is_alive());
        assert!(running.pty.cleanup_confirmed());
        assert_ne!(running.pty.process_id(), 0);
        assert!(snapshot_text(&running.pty).contains("FINAL"));
        assert!(!running.owner.as_ref().unwrap().is_finished());
        assert_eq!(
            running.pty.write(b"x").unwrap_err().kind(),
            io::ErrorKind::NotConnected
        );
        let mut resized = size();
        resized.num_cols = 60;
        running.pty.resize(resized).unwrap();
        // Rio intentionally clears selection when columns change. Select the
        // retained transcript after resizing, just as a frontend would.
        running.pty.command(TerminalCommand::Scroll(10)).unwrap();
        running.pty.command(TerminalCommand::SelectAll).unwrap();
        wait_until(|| {
            running.pty.request_snapshot();
            let snapshot = running.pty.snapshot();
            snapshot.cols() == 60
                && snapshot.display_offset() > 0
                && snapshot
                    .selection_text()
                    .is_some_and(|text| text.contains("LINE-00"))
        });
        running.finish().unwrap();
        assert!(running.pty.poll().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn cancellation_returns_only_after_stubborn_child_is_reaped() {
        let mut running = Running::start(shell_options(
            "trap '' HUP TERM; printf READY; exec /bin/sleep 30",
        ));
        running.wait_text("READY");
        assert!(!running.pty.cleanup_confirmed());
        let pid = running.pty.process_id();
        let started = Instant::now();
        running.finish().unwrap();
        assert!(running.pty.cleanup_confirmed());
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(!running.pty.is_alive());
        assert_eq!(unsafe { libc::kill(pid as _, 0) }, -1);
        assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
        let mut status = 0;
        assert_eq!(
            unsafe { libc::waitpid(pid as _, &mut status, libc::WNOHANG) },
            -1
        );
        assert_eq!(
            io::Error::last_os_error().raw_os_error(),
            Some(libc::ECHILD)
        );
    }

    #[cfg(unix)]
    #[test]
    fn cancellation_cleans_foreground_group_before_killing_its_shell_reaper() {
        let mut running = Running::start(PtyOptions {
            shell: Some(TerminalShell::new(
                "/bin/bash",
                vec!["--noprofile".into(), "--norc".into(), "-i".into()],
            )),
            env: HashMap::from([
                ("PS1".into(), String::new()),
                ("PROMPT_COMMAND".into(), String::new()),
            ]),
            ..PtyOptions::default()
        });
        running.pty.write(
            b"/bin/sh -c 'trap \"\" HUP TERM; printf \"\\nJOB:%s\\n\" \"$$\"; exec /bin/sleep 30'\n",
        ).unwrap();
        let mut foreground = None;
        wait_until(|| {
            running.pty.request_snapshot();
            foreground = snapshot_text(&running.pty).lines().find_map(|line| {
                line.trim()
                    .strip_prefix("JOB:")
                    .and_then(|pid| pid.parse::<i32>().ok())
            });
            foreground.is_some()
        });
        let foreground = foreground.unwrap();
        let leader = running.pty.process_id();
        assert_ne!(foreground, leader as i32);
        running.finish().unwrap();
        assert_eq!(unsafe { libc::kill(-foreground, 0) }, -1);
        assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
        assert_eq!(unsafe { libc::kill(leader as _, 0) }, -1);
        assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
    }

    #[cfg(unix)]
    #[test]
    fn external_handle_drop_is_nonblocking_and_owner_performs_cleanup() {
        // Deliberately block cancellation after launch. Dropping the handle
        // must not join or wait for this externally admitted child's owner.
        let (pty, worker) = prepare(shell_options("trap '' HUP TERM; exec /bin/sleep 30"));
        let (entered, entered_rx) = mpsc::channel();
        let (release, release_rx) = mpsc::channel();
        let owner = thread::spawn(move || {
            let first_check = AtomicBool::new(true);
            worker.run(|| {
                if first_check.swap(false, Ordering::AcqRel) {
                    return false;
                }
                entered.send(()).unwrap();
                release_rx.recv().unwrap();
                false
            })
        });
        entered_rx.recv().unwrap();
        let pid = pty.process_id();
        assert_ne!(pid, 0);
        let before = Instant::now();
        drop(pty);
        assert!(before.elapsed() < Duration::from_millis(100));
        assert!(!owner.is_finished());
        release.send(()).unwrap();
        owner.join().unwrap().unwrap();
        assert_eq!(unsafe { libc::kill(pid as _, 0) }, -1);
        assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
    }

    #[cfg(unix)]
    #[test]
    fn queued_stop_is_owned_even_when_requested_before_run() {
        let (pty, worker) = prepare(shell_options("exec /bin/sleep 30"));
        pty.command(TerminalCommand::Stop).unwrap();
        let mut running = Running::from_prepared(pty, worker);
        let mut stopped = false;
        wait_until(|| {
            for event in running.pty.poll() {
                match event {
                    PtyEvent::Exited(_) => stopped = true,
                    PtyEvent::Error(error) => panic!("{error}"),
                    PtyEvent::Output(_) => unreachable!(),
                }
            }
            stopped
        });
        let pid = running.pty.process_id();
        assert_ne!(pid, 0);
        assert_eq!(unsafe { libc::kill(pid as _, 0) }, -1);
        running.finish().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn legacy_spawn_keeps_synchronous_pid_and_captured_output_before_exit() {
        let mut pty = TerminalPty::spawn(&shell_options("printf RAW_END; exit 9"), size()).unwrap();
        assert_ne!(pty.process_id(), 0);
        let mut output = Vec::new();
        let mut exit = None;
        wait_until(|| {
            for event in pty.poll() {
                match event {
                    PtyEvent::Output(bytes) => output.extend(bytes),
                    PtyEvent::Exited(status) => {
                        assert!(output.ends_with(b"RAW_END"));
                        exit = status;
                    }
                    PtyEvent::Error(error) => panic!("{error}"),
                }
            }
            exit.is_some()
        });
        assert_eq!(exit.unwrap().code(), Some(9));
        pty.shutdown();
    }

    #[cfg(feature = "remote")]
    #[test]
    fn ssh_terminal_uses_remote_root_and_literal_shell_quoting() {
        let target = bedtime_remote::SshTarget {
            host: "user@host".into(),
            agent: "bedtime-remote-agent".into(),
        };
        let base = PtyOptions {
            working_directory: Some(PathBuf::from("/local/project")),
            ..Default::default()
        };
        let remote = base.for_ssh(&target, "/remote/a'b $(touch bad)").unwrap();
        assert!(remote.working_directory.is_none());
        let shell = remote.shell.unwrap();
        assert_eq!(shell.program, "ssh");
        assert_eq!(&shell.args[..3], &["-tt", "--", "user@host"]);
        assert_eq!(
            shell.args[3],
            "cd '/remote/a'\\''b $(touch bad)' && exec \"${SHELL:-/bin/sh}\" -l"
        );
        assert_eq!(
            base.working_directory,
            Some(PathBuf::from("/local/project"))
        );
        assert!(base.for_ssh(&target, "relative").is_err());
        assert!(base.for_ssh(&target, "/nul\0path").is_err());
    }

    #[test]
    fn validates_dimensions_before_native_resize_or_spawn() {
        assert!(
            validate_size(WindowSize {
                num_cols: 0,
                num_lines: 24,
                cell_width: 8,
                cell_height: 16
            })
            .is_err()
        );
        assert!(
            validate_size(WindowSize {
                num_cols: u16::MAX,
                num_lines: 24,
                cell_width: 8,
                cell_height: 16
            })
            .is_err()
        );
    }
}
