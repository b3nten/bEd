//! Worker-owned Unix PTY transport using the operating system PTY/process APIs.
//! The worker owns parsing, graphics decoding and terminal mutations.
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
    Exited(Option<ExitStatus>),
}
use polling::{Event, Events, PollMode, Poller};
use std::{
    collections::{HashMap, VecDeque},
    io::{Read, Write},
    path::PathBuf,
    process::ExitStatus,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
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
    pub fn for_ssh(&self, target: &bed_remote::SshTarget, root: &str) -> io::Result<Self> {
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
                    bed_remote::shell_quote(root)?
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
    output: Option<mpsc::Receiver<PtyEvent>>,
    worker: Option<JoinHandle<()>>,
    poller: Arc<Poller>,
    stop: Arc<AtomicBool>,
    alive: Arc<AtomicBool>,
    budget: Arc<AtomicUsize>,
    process_id: u32,
    shell_name: String,
    foreground_process: Arc<Mutex<Option<String>>>,
    snapshot: Arc<Mutex<Arc<TerminalSnapshot>>>,
    snapshot_requested: Arc<AtomicBool>,
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

    fn spawn_owned(
        options: &PtyOptions,
        size: WindowSize,
        mut terminal: Terminal,
        capture_output: bool,
    ) -> io::Result<Self> {
        validate_size(size)?;
        terminal.resize_with_pixels(
            size.num_cols as usize,
            size.num_lines as usize,
            size.cell_width as u32,
            size.cell_height as u32,
        );
        let snapshot = Arc::new(Mutex::new(terminal.snapshot()));
        let snapshot_requested = Arc::new(AtomicBool::new(false));
        let mut backend = platform::Pty::spawn(options, size)?;
        let process_id = backend.process_id();
        let shell_name = backend.shell_name().to_owned();
        let foreground_process = Arc::new(Mutex::new(None));
        let poller = Arc::new(Poller::new()?);
        // SAFETY: backend owns the registered source throughout the worker. It
        // deregisters before closing that source, including all error paths.
        unsafe {
            backend.register(&poller, Event::readable(0), PollMode::Oneshot)?;
        }
        let (commands, receiver) = mpsc::sync_channel(COMMAND_CAPACITY);
        let (sender, output) = mpsc::sync_channel(OUTPUT_QUEUE_CHUNKS);
        let stop = Arc::new(AtomicBool::new(false));
        let alive = Arc::new(AtomicBool::new(true));
        let worker_stop = stop.clone();
        let worker_alive = alive.clone();
        let worker_poll = poller.clone();
        let worker_foreground = foreground_process.clone();
        let worker_snapshot = snapshot.clone();
        let worker_requested = snapshot_requested.clone();
        let budget = Arc::new(AtomicUsize::new(0));
        let worker = thread::Builder::new()
            .name("bed-terminal-pty".into())
            .spawn(move || {
                if let Err(error) = run_worker(
                    &mut backend,
                    &receiver,
                    &sender,
                    &worker_stop,
                    &worker_poll,
                    &worker_foreground,
                    &mut terminal,
                    &worker_snapshot,
                    &worker_requested,
                    &worker_alive,
                    capture_output,
                ) {
                    let _ = sender.try_send(PtyEvent::Error(error.to_string()));
                }
                *worker_snapshot.lock().unwrap() = terminal.snapshot();
                worker_alive.store(false, Ordering::Release);
                let _ = backend.deregister(&worker_poll);
                backend.shutdown();
            })?;
        Ok(Self {
            commands: Some(commands),
            output: Some(output),
            worker: Some(worker),
            poller,
            stop,
            alive,
            budget,
            process_id,
            shell_name,
            foreground_process,
            snapshot,
            snapshot_requested,
        })
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
                return Err(io::Error::new(
                    io::ErrorKind::NotConnected,
                    "Terminal session ended",
                ));
            }
            if bytes > MAX_WRITE_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Terminal input exceeds 1 MiB",
                ));
            }
        }
        let reservation = reserve
            .map(|bytes| reserve_bytes(&self.budget, bytes, MAX_QUEUED_WRITE_BYTES))
            .transpose()?;
        self.send(PtyCommand::Terminal(command, reservation))
    }

    pub fn process_id(&self) -> u32 {
        self.process_id
    }
    pub fn shell_name(&self) -> &str {
        &self.shell_name
    }
    /// Cached by the PTY worker; reading a tab label never queries the OS.
    pub fn foreground_process_name(&self) -> Option<String> {
        self.foreground_process.lock().unwrap().clone()
    }
    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Acquire)
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
        let Some(sender) = &self.commands else {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "Terminal session ended",
            ));
        };
        match sender.try_send(command) {
            Ok(()) => self.poller.notify(),
            Err(mpsc::TrySendError::Full(_)) => Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Terminal command queue is full",
            )),
            Err(mpsc::TrySendError::Disconnected(_)) => Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "Terminal worker stopped",
            )),
        }
    }

    /// At most 1 MiB per poll keeps a busy child from monopolizing a UI frame.
    pub fn poll(&mut self) -> Vec<PtyEvent> {
        match &self.output {
            None => Vec::new(),
            Some(receiver) => receiver.try_iter().take(OUTPUT_QUEUE_CHUNKS).collect(),
        }
    }

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
    capture_output: bool,
) -> io::Result<()> {
    let mut writes: VecDeque<QueuedWrite> = VecDeque::new();
    // Protocol replies cannot consume reservations already accepted from the
    // user. Backpressure pauses reads while those replies wait for the child.
    let reply_budget = Arc::new(AtomicUsize::new(0));
    let mut events = Events::new();
    let mut child_exit = None;
    let mut exit_deadline = None;
    let mut read_closed = false;
    let mut exited = false;
    let mut next_title_poll = Instant::now();
    while !stop.load(Ordering::Acquire) {
        if !exited
            && child_exit.is_none()
            && let Some(ChildEvent::Exited(status)) = backend.next_child_event()
        {
            child_exit = Some(status);
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
                    if !exited && child_exit.is_none() {
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
                    if !exited && child_exit.is_none() {
                        backend.resize(size)?;
                    }
                }
                Ok(PtyCommand::Terminal(command, reservation)) => {
                    match command {
                        TerminalCommand::Key(_)
                        | TerminalCommand::Paste(_)
                        | TerminalCommand::Text(_)
                            if !exited && child_exit.is_none() =>
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
                            if !exited
                                && child_exit.is_none()
                                && terminal.modes().focus_reporting =>
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
                        TerminalCommand::Stop if !exited && child_exit.is_none() => {
                            // Kill/reap on the owner thread; keep the terminal model
                            // alive for selecting and copying the final transcript.
                            backend.terminate();
                            child_exit = Some(None);
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
                || (child_exit.is_none()
                    && reply_budget.load(Ordering::Acquire) >= REPLY_READ_WATERMARK)
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
                    parse_output(
                        terminal,
                        &bytes,
                        &mut writes,
                        &reply_budget,
                        child_exit.is_none(),
                    )?;
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
            if child_exit.is_none()
                && let Some(ChildEvent::Exited(status)) = backend.next_child_event()
            {
                child_exit = Some(status);
                exit_deadline = Some(Instant::now() + Duration::from_millis(500));
                alive.store(false, Ordering::Release);
                writes.clear();
            }
            if let Some(status) = child_exit
                && (drained || exit_deadline.is_some_and(|deadline| Instant::now() >= deadline))
            {
                terminal.finish_output();
                *snapshot.lock().unwrap() = terminal.snapshot();
                alive.store(false, Ordering::Release);
                let _ = output.send(PtyEvent::Exited(status));
                if capture_output {
                    return Ok(());
                }
                backend.deregister(poller)?;
                writes.clear();
                exited = true;
            }
        }
        for event in terminal.flush_sync() {
            if let TerminalEvent::Write(bytes) = event
                && !exited
                && child_exit.is_none()
            {
                enqueue_reply(&mut writes, &reply_budget, bytes)?;
            }
        }
        if snapshot_requested.swap(false, Ordering::AcqRel) {
            *snapshot.lock().unwrap() = terminal.snapshot();
        }
        if !exited && child_exit.is_none() {
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
        match poller.wait(&mut events, Some(Duration::from_millis(20))) {
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
    // Child status is read with waitpid: a debugger stop is not an exit.
    pub struct Pty {
        process: Child,
        master: Option<File>,
        program: String,
        finished: bool,
    }

    impl Pty {
        pub fn spawn(options: &PtyOptions, size: WindowSize) -> io::Result<Self> {
            let account = account()?;
            let inherited_shell = std::env::var("SHELL").unwrap_or_else(|_| account[2].clone());
            let launch = options.shell.clone().unwrap_or_else(|| TerminalShell {
                args: if inherited_shell.contains("zsh") {
                    vec!["+o".to_owned(), "PROMPT_SP".to_owned()]
                } else {
                    Vec::new()
                },
                program: inherited_shell.clone(),
                login: true,
            });
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
                program: crate::process_title::program_name(&launch.program),
                finished: false,
            })
        }

        pub fn process_id(&self) -> u32 {
            self.process.id()
        }
        pub fn shell_name(&self) -> &str {
            &self.program
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
            unsafe { poller.add_with_mode(&*self.io(), event, mode) }
        }
        pub fn reregister(
            &mut self,
            poller: &Arc<Poller>,
            event: Event,
            mode: PollMode,
        ) -> io::Result<()> {
            poller.modify_with_mode(self.io(), event, mode)
        }
        pub fn deregister(&mut self, poller: &Arc<Poller>) -> io::Result<()> {
            match &self.master {
                Some(master) => poller.delete(master),
                None => Ok(()),
            }
        }

        pub fn next_child_event(&mut self) -> Option<ChildEvent> {
            if self.finished {
                return None;
            }
            let mut status = 0;
            let result =
                unsafe { libc::waitpid(self.process.id() as _, &mut status, libc::WNOHANG) };
            let exit = match result {
                0 => return None,
                value if value > 0 => {
                    if !(libc::WIFEXITED(status) || libc::WIFSIGNALED(status)) {
                        return None;
                    }
                    Some(ExitStatus::from_raw(status))
                }
                _ => {
                    let error = io::Error::last_os_error();
                    if error.raw_os_error() == Some(libc::EINTR) {
                        return None;
                    }
                    if error.raw_os_error() == Some(libc::ECHILD) {
                        // A debugger may temporarily own wait notifications.
                        let exists = unsafe { libc::kill(self.process.id() as _, 0) } == 0;
                        if exists || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
                        {
                            return None;
                        }
                    }
                    None
                }
            };
            self.finished = true;
            Some(ChildEvent::Exited(exit))
        }

        // Keep the master open through termination so the worker can drain the
        // child's final output. Foreground jobs may own a separate process group.
        pub fn terminate(&mut self) {
            self.next_child_event();
            let leader = self.process.id() as libc::pid_t;
            let foreground = self
                .master
                .as_ref()
                .map(|master| unsafe { libc::tcgetpgrp(master.as_raw_fd()) })
                .filter(|group| *group > 0 && *group != leader);
            let mut groups = Vec::with_capacity(2);
            if !self.finished {
                groups.push(leader);
            }
            groups.extend(foreground);
            for signal in [libc::SIGHUP, libc::SIGTERM] {
                for group in &groups {
                    unsafe {
                        libc::kill(-*group, signal);
                    }
                }
            }
            let grace = Instant::now() + Duration::from_millis(100);
            loop {
                self.next_child_event();
                let job_finished = foreground.is_none_or(|group| {
                    (unsafe { libc::kill(-group, 0) }) == -1
                        && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
                });
                if self.finished && job_finished {
                    return;
                }
                if Instant::now() >= grace {
                    break;
                }
                thread::sleep(Duration::from_millis(5));
            }
            for group in groups {
                unsafe {
                    libc::kill(-group, libc::SIGKILL);
                }
            }
            if !self.finished {
                let _ = self.process.kill();
                loop {
                    let mut status = 0;
                    let result = unsafe { libc::waitpid(leader, &mut status, 0) };
                    if result > 0 && (libc::WIFEXITED(status) || libc::WIFSIGNALED(status)) {
                        self.finished = true;
                        break;
                    }
                    if result == -1
                        && io::Error::last_os_error().raw_os_error() != Some(libc::EINTR)
                    {
                        break;
                    }
                }
            }
        }
        pub fn shutdown(&mut self) {
            self.terminate();
            self.master = None;
        }
    }
    impl Drop for Pty {
        fn drop(&mut self) {
            self.shutdown();
        }
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
    #[test]
    fn ssh_terminal_uses_remote_root_and_literal_shell_quoting() {
        let target = bed_remote::SshTarget {
            host: "user@host".into(),
            agent: "bed-headless".into(),
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
