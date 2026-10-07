//! Owned PTY transport translated from pinned ImGui-Terminal tty/pump/teardown
//! code, using Alacritty's PTY traits. Parsing and terminal state stay on the UI
//! thread. Shell setup affects only the child, never host cwd/env/signal handlers.
use std::io;
use std::thread;

pub use alacritty_terminal::event::WindowSize;
use alacritty_terminal::tty::{ChildEvent, EventedPty, EventedReadWrite};
use polling::{Event, Events, PollMode, Poller};
use std::{
    collections::{HashMap, VecDeque},
    io::{Read, Write},
    path::PathBuf,
    process::ExitStatus,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

pub const MAX_WRITE_BYTES: usize = 1024 * 1024;
pub const MAX_QUEUED_WRITE_BYTES: usize = 4 * 1024 * 1024;
pub const OUTPUT_CHUNK_BYTES: usize = 16 * 1024;
pub const OUTPUT_QUEUE_CHUNKS: usize = 64;
const COMMAND_CAPACITY: usize = 64;

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
    budget: Arc<AtomicUsize>,
}
impl Drop for QueuedWrite {
    fn drop(&mut self) {
        self.budget.fetch_sub(self.bytes.len(), Ordering::AcqRel);
    }
}
enum PtyCommand {
    Write(QueuedWrite),
    Resize(WindowSize),
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
}

impl TerminalPty {
    pub fn spawn(options: &PtyOptions, size: WindowSize) -> io::Result<Self> {
        validate_size(size)?;
        let mut backend = platform::Pty::spawn(options, size)?;
        let process_id = backend.process_id();
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
        let worker = thread::Builder::new()
            .name("bed-terminal-pty".into())
            .spawn(move || {
                if let Err(error) =
                    run_worker(&mut backend, &receiver, &sender, &worker_stop, &worker_poll)
                {
                    let _ = sender.send(PtyEvent::Error(error.to_string()));
                }
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
            budget: Arc::new(AtomicUsize::new(0)),
            process_id,
        })
    }

    pub fn process_id(&self) -> u32 {
        self.process_id
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
        self.budget
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes.len())
                    .filter(|used| *used <= MAX_QUEUED_WRITE_BYTES)
            })
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "Terminal write queue exceeds 4 MiB",
                )
            })?;
        self.send(PtyCommand::Write(QueuedWrite {
            bytes: bytes.to_vec(),
            offset: 0,
            budget: self.budget.clone(),
        }))
    }

    pub fn resize(&self, size: WindowSize) -> io::Result<()> {
        validate_size(size)?;
        if !self.is_alive() {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "Terminal session ended",
            ));
        }
        self.send(PtyCommand::Resize(size))
    }

    fn send(&self, command: PtyCommand) -> io::Result<()> {
        self.commands
            .as_ref()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "Terminal session ended"))?
            .try_send(command)
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => {
                    io::Error::new(io::ErrorKind::WouldBlock, "Terminal command queue is full")
                }
                mpsc::TrySendError::Disconnected(_) => {
                    io::Error::new(io::ErrorKind::BrokenPipe, "Terminal worker stopped")
                }
            })?;
        self.poller.notify()
    }

    /// At most 1 MiB per poll keeps a busy child from monopolizing a UI frame.
    pub fn poll(&mut self) -> Vec<PtyEvent> {
        let mut events = Vec::new();
        if let Some(output) = &self.output {
            for _ in 0..OUTPUT_QUEUE_CHUNKS {
                match output.try_recv() {
                    Ok(event) => events.push(event),
                    Err(_) => break,
                }
            }
        }
        events
    }

    pub fn shutdown(&mut self) {
        self.output.take(); // Unblock a worker waiting on output backpressure.
        self.stop.store(true, Ordering::Release);
        self.alive.store(false, Ordering::Release);
        self.commands.take();
        let _ = self.poller.notify();
        if let Some(worker) = self.worker.take() {
            let deadline = Instant::now() + Duration::from_millis(500);
            while !worker.is_finished() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(2));
            }
            if worker.is_finished() {
                let _ = worker.join();
            }
            // A slow OS cleanup retains only owned process/pipe state. It can
            // safely finish after this handle drops, without any host references.
        }
    }
}
impl Drop for TerminalPty {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn validate_size(size: WindowSize) -> io::Result<()> {
    if size.num_cols == 0
        || size.num_lines == 0
        || size.num_cols > i16::MAX as u16
        || size.num_lines > i16::MAX as u16
    {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Terminal dimensions must be 1..32767",
        ))
    } else {
        Ok(())
    }
}

fn run_worker(
    backend: &mut platform::Pty,
    commands: &mpsc::Receiver<PtyCommand>,
    output: &mpsc::SyncSender<PtyEvent>,
    stop: &AtomicBool,
    poller: &Arc<Poller>,
) -> io::Result<()> {
    let mut writes: VecDeque<QueuedWrite> = VecDeque::new();
    let mut events = Events::new();
    let mut child_exit = None;
    let mut exit_deadline = None;
    let mut read_closed = false;
    while !stop.load(Ordering::Acquire) {
        for _ in 0..COMMAND_CAPACITY {
            match commands.try_recv() {
                Ok(PtyCommand::Write(bytes)) => writes.push_back(bytes),
                Ok(PtyCommand::Resize(size)) => backend.resize(size)?,
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => return Ok(()),
            }
        }
        let mut drained = read_closed;
        for _ in 0..16 {
            if read_closed {
                break;
            }
            let mut bytes = vec![0; OUTPUT_CHUNK_BYTES];
            match backend.reader().read(&mut bytes) {
                Ok(0) => {
                    drained = true;
                    read_closed = true;
                    break;
                }
                Ok(length) => {
                    bytes.truncate(length);
                    if output.send(PtyEvent::Output(bytes)).is_err() {
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
        if let Some(ChildEvent::Exited(status)) = backend.next_child_event() {
            child_exit = Some(status);
            exit_deadline = Some(Instant::now() + Duration::from_millis(500));
        }
        // Unix reads the kernel master directly; WouldBlock after a reaped child
        // has consumed its pending output. ConPTY has an intermediate reader
        // worker, so only EOF proves that its final bytes have reached this queue.
        let exit_drained = if cfg!(windows) { read_closed } else { drained };
        if let Some(status) = child_exit
            && (exit_drained || exit_deadline.is_some_and(|deadline| Instant::now() >= deadline))
        {
            let _ = output.send(PtyEvent::Exited(status));
            return Ok(());
        }
        for _ in 0..64 {
            let Some(write) = writes.front_mut() else {
                break;
            };
            let remaining = &write.bytes[write.offset..];
            match backend
                .writer()
                .write(&remaining[..remaining.len().min(256)])
            {
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
            Event::new(0, !read_closed, !writes.is_empty()),
            PollMode::Oneshot,
        )?;
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
            unix::process::CommandExt,
        },
        process::{Child, Command},
    };

    pub struct Pty {
        child: Child,
        file: Option<File>,
        reported_exit: bool,
    }
    impl Pty {
        pub fn spawn(options: &PtyOptions, size: WindowSize) -> io::Result<Self> {
            let user = user_info()?;
            let shell = options.shell.clone().unwrap_or_else(|| {
                let program = std::env::var("SHELL").unwrap_or_else(|_| user.2.clone());
                let args = if program.contains("zsh") {
                    vec!["+o".into(), "PROMPT_SP".into()]
                } else {
                    Vec::new()
                };
                TerminalShell {
                    program,
                    args,
                    login: true,
                }
            });
            let mut master = -1;
            let mut slave = -1;
            let mut window = winsize(size);
            // SAFETY: output descriptor pointers and winsize are valid; successful
            // descriptors are immediately placed into exclusive File ownership.
            if unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::addr_of_mut!(window),
                )
            } != 0
            {
                return Err(io::Error::last_os_error());
            }
            let file = unsafe { File::from_raw_fd(master) };
            let slave = unsafe { File::from_raw_fd(slave) };
            for descriptor in [file.as_raw_fd(), slave.as_raw_fd()] {
                if unsafe { libc::fcntl(descriptor, libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
                    return Err(io::Error::last_os_error());
                }
            }
            let flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFL) };
            if flags < 0
                || unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) }
                    < 0
            {
                return Err(io::Error::last_os_error());
            }
            let mut command = Command::new(&shell.program);
            command
                .args(&shell.args)
                .stdin(slave.try_clone()?)
                .stdout(slave.try_clone()?)
                .stderr(slave);
            if shell.login {
                let name = std::path::Path::new(&shell.program)
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy();
                command.arg0(format!("-{name}"));
            }
            if let Some(directory) = &options.working_directory {
                command.current_dir(directory);
            }
            command
                .env_remove("COLUMNS")
                .env_remove("LINES")
                .env_remove("TERMCAP")
                .env("LOGNAME", &user.0)
                .env("USER", &user.0)
                .env("HOME", &user.1)
                .env(
                    "SHELL",
                    std::env::var("SHELL").unwrap_or_else(|_| user.2.clone()),
                )
                .env("TERM", "st-256color")
                .env("TERM_PROGRAM", "st-imgui")
                .envs(&options.env);
            // SAFETY: only async-signal-safe libc calls execute between fork and
            // exec. All strings/environment/cwd/stdio setup occurs in Command.
            unsafe {
                command.pre_exec(|| {
                    if libc::setsid() < 0 {
                        return Err(io::Error::last_os_error());
                    }
                    if libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                        return Err(io::Error::last_os_error());
                    }
                    for signal in [
                        libc::SIGCHLD,
                        libc::SIGHUP,
                        libc::SIGINT,
                        libc::SIGQUIT,
                        libc::SIGTERM,
                        libc::SIGALRM,
                    ] {
                        libc::signal(signal, libc::SIG_DFL);
                    }
                    Ok(())
                });
            }
            Ok(Self {
                child: command.spawn()?,
                file: Some(file),
                reported_exit: false,
            })
        }
        pub fn process_id(&self) -> u32 {
            self.child.id()
        }
        pub fn resize(&mut self, size: WindowSize) -> io::Result<()> {
            let file = self
                .file
                .as_ref()
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "PTY closed"))?;
            let window = winsize(size);
            if unsafe { libc::ioctl(file.as_raw_fd(), libc::TIOCSWINSZ, &window) } < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        }
        pub fn shutdown(&mut self) {
            self.file.take(); // Kernel HUP reaches the foreground interactive job.
            if self.child.try_wait().ok().flatten().is_some() {
                return;
            }
            let group = -(self.child.id() as i32);
            unsafe {
                libc::kill(group, libc::SIGHUP);
                libc::kill(group, libc::SIGTERM);
            }
            let deadline = Instant::now() + Duration::from_millis(100);
            while Instant::now() < deadline {
                if self.child.try_wait().ok().flatten().is_some() {
                    return;
                }
                thread::sleep(Duration::from_millis(5));
            }
            unsafe {
                libc::kill(group, libc::SIGKILL);
            }
            let _ = self.child.kill();
            let _ = self.child.wait(); // Worker owns this wait; never the UI thread.
        }
    }
    impl Drop for Pty {
        fn drop(&mut self) {
            self.shutdown();
        }
    }
    impl EventedReadWrite for Pty {
        type Reader = File;
        type Writer = File;
        unsafe fn register(
            &mut self,
            poller: &Arc<Poller>,
            interest: Event,
            mode: PollMode,
        ) -> io::Result<()> {
            unsafe { poller.add_with_mode(self.file.as_ref().unwrap(), interest, mode) }
        }
        fn reregister(
            &mut self,
            poller: &Arc<Poller>,
            interest: Event,
            mode: PollMode,
        ) -> io::Result<()> {
            poller.modify_with_mode(self.file.as_ref().unwrap(), interest, mode)
        }
        fn deregister(&mut self, poller: &Arc<Poller>) -> io::Result<()> {
            self.file
                .as_ref()
                .map_or(Ok(()), |file| poller.delete(file))
        }
        fn reader(&mut self) -> &mut File {
            self.file.as_mut().unwrap()
        }
        fn writer(&mut self) -> &mut File {
            self.file.as_mut().unwrap()
        }
    }
    impl EventedPty for Pty {
        fn next_child_event(&mut self) -> Option<ChildEvent> {
            if self.reported_exit {
                return None;
            }
            match self.child.try_wait() {
                Ok(Some(status)) => {
                    self.reported_exit = true;
                    Some(ChildEvent::Exited(Some(status)))
                }
                Ok(None) => None,
                Err(_) => {
                    self.reported_exit = true;
                    Some(ChildEvent::Exited(None))
                }
            }
        }
    }
    fn winsize(size: WindowSize) -> libc::winsize {
        libc::winsize {
            ws_row: size.num_lines,
            ws_col: size.num_cols,
            ws_xpixel: size.num_cols.saturating_mul(size.cell_width),
            ws_ypixel: size.num_lines.saturating_mul(size.cell_height),
        }
    }
    fn user_info() -> io::Result<(String, String, String)> {
        let mut buffer = vec![0; 65536];
        let mut entry = std::mem::MaybeUninit::<libc::passwd>::uninit();
        let mut result = std::ptr::null_mut();
        let status = unsafe {
            libc::getpwuid_r(
                libc::getuid(),
                entry.as_mut_ptr(),
                buffer.as_mut_ptr(),
                buffer.len(),
                &mut result,
            )
        };
        if status != 0 {
            return Err(io::Error::from_raw_os_error(status));
        }
        if result.is_null() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "No shell user entry",
            ));
        }
        let entry = unsafe { entry.assume_init() };
        let string = |pointer: *const libc::c_char| unsafe {
            CStr::from_ptr(pointer).to_string_lossy().into_owned()
        };
        let shell = string(entry.pw_shell);
        Ok((
            string(entry.pw_name),
            string(entry.pw_dir),
            if shell.is_empty() {
                "/bin/sh".into()
            } else {
                shell
            },
        ))
    }
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::{
        ffi::{OsStr, c_void},
        fs::File,
        os::windows::{
            ffi::OsStrExt,
            io::{AsRawHandle, FromRawHandle, OwnedHandle},
            process::ExitStatusExt,
        },
        sync::Mutex,
    };
    use windows_sys::Win32::{
        Foundation::{HANDLE, S_OK, WAIT_OBJECT_0},
        System::{
            Console::{COORD, ClosePseudoConsole, CreatePseudoConsole, HPCON, ResizePseudoConsole},
            Pipes::CreatePipe,
            Threading::{
                CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DeleteProcThreadAttributeList,
                EXTENDED_STARTUPINFO_PRESENT, GetExitCodeProcess,
                InitializeProcThreadAttributeList, PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE,
                PROCESS_INFORMATION, STARTUPINFOEXW, TerminateProcess, UpdateProcThreadAttribute,
                WaitForSingleObject,
            },
        },
    };

    type Wake = Arc<Mutex<Option<Arc<Poller>>>>;
    pub struct PtyReader {
        receiver: mpsc::Receiver<Vec<u8>>,
        pending: Vec<u8>,
        offset: usize,
    }
    impl Read for PtyReader {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            if bytes.is_empty() {
                return Ok(0);
            }
            if self.offset == self.pending.len() {
                match self.receiver.try_recv() {
                    Ok(pending) => {
                        self.pending = pending;
                        self.offset = 0;
                    }
                    Err(mpsc::TryRecvError::Empty) => return Err(io::ErrorKind::WouldBlock.into()),
                    Err(mpsc::TryRecvError::Disconnected) => return Ok(0),
                }
            }
            let length = bytes.len().min(self.pending.len() - self.offset);
            bytes[..length].copy_from_slice(&self.pending[self.offset..self.offset + length]);
            self.offset += length;
            Ok(length)
        }
    }
    pub struct PtyWriter {
        sender: Option<mpsc::SyncSender<Vec<u8>>>,
        error: Arc<Mutex<Option<String>>>,
    }
    impl Write for PtyWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if let Some(error) = self.error.lock().unwrap().as_ref() {
                return Err(io::Error::new(io::ErrorKind::BrokenPipe, error.clone()));
            }
            let length = bytes.len().min(OUTPUT_CHUNK_BYTES);
            self.sender
                .as_ref()
                .ok_or(io::ErrorKind::BrokenPipe)?
                .try_send(bytes[..length].to_vec())
                .map_err(|error| match error {
                    mpsc::TrySendError::Full(_) => io::ErrorKind::WouldBlock,
                    mpsc::TrySendError::Disconnected(_) => io::ErrorKind::BrokenPipe,
                })?;
            Ok(length)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    pub struct Pty {
        console: Option<HPCON>,
        process: Option<OwnedHandle>,
        process_id: u32,
        reader: PtyReader,
        writer: PtyWriter,
        io_stop: Arc<AtomicBool>,
        io_workers: Vec<JoinHandle<()>>,
        close_console: Option<mpsc::SyncSender<HPCON>>,
        wake: Wake,
        reported_exit: bool,
    }
    impl Pty {
        pub fn spawn(options: &PtyOptions, size: WindowSize) -> io::Result<Self> {
            let (input_read, input_write) = pipe()?;
            let (output_read, output_write) = pipe()?;
            let mut console = 0;
            let result = unsafe {
                CreatePseudoConsole(
                    coord(size),
                    input_read.as_raw_handle() as HANDLE,
                    output_write.as_raw_handle() as HANDLE,
                    0,
                    &mut console,
                )
            };
            if result != S_OK {
                return Err(io::Error::other(format!(
                    "CreatePseudoConsole failed: {result:#x}"
                )));
            }
            drop(input_read);
            drop(output_write);
            let (input_sender, input_receiver) = mpsc::sync_channel::<Vec<u8>>(8);
            let (output_sender, output_receiver) = mpsc::sync_channel::<Vec<u8>>(8);
            let io_stop = Arc::new(AtomicBool::new(false));
            let wake: Wake = Arc::new(Mutex::new(None));
            let write_error = Arc::new(Mutex::new(None));
            let mut pty = Self {
                console: Some(console),
                process: None,
                process_id: 0,
                reader: PtyReader {
                    receiver: output_receiver,
                    pending: Vec::new(),
                    offset: 0,
                },
                writer: PtyWriter {
                    sender: Some(input_sender),
                    error: write_error.clone(),
                },
                io_stop: io_stop.clone(),
                io_workers: Vec::new(),
                close_console: None,
                wake: wake.clone(),
                reported_exit: false,
            };
            let reader_wake = wake.clone();
            pty.io_workers.push(
                thread::Builder::new()
                    .name("bed-conpty-read".into())
                    .spawn(move || {
                        let mut source = File::from(output_read);
                        loop {
                            let mut bytes = vec![0; OUTPUT_CHUNK_BYTES];
                            match source.read(&mut bytes) {
                                Ok(0) => break,
                                Ok(length) => bytes.truncate(length),
                                Err(error) if error.kind() == io::ErrorKind::Interrupted => {
                                    continue;
                                }
                                Err(_) => break,
                            }
                            while !io_stop.load(Ordering::Acquire) {
                                match output_sender.try_send(bytes) {
                                    Ok(()) => {
                                        notify(&reader_wake);
                                        break;
                                    }
                                    Err(mpsc::TrySendError::Full(returned)) => {
                                        bytes = returned;
                                        thread::sleep(Duration::from_millis(2));
                                    }
                                    Err(mpsc::TrySendError::Disconnected(_)) => {
                                        io_stop.store(true, Ordering::Release);
                                        break;
                                    }
                                }
                            }
                            // During ClosePseudoConsole, continue reading and discard
                            // output. Its close call blocks until this pipe is drained.
                        }
                        notify(&reader_wake);
                    })?,
            );

            // ClosePseudoConsole can wait for its output pipe to drain. Keep
            // that operation on an owned worker while the common PTY loop reads
            // the final bytes. The reader keeps draining during forced shutdown.
            let (close_sender, close_receiver) = mpsc::sync_channel(1);
            pty.io_workers.push(
                thread::Builder::new()
                    .name("bed-conpty-close".into())
                    .spawn(move || {
                        if let Ok(console) = close_receiver.recv() {
                            unsafe { ClosePseudoConsole(console) };
                        }
                    })?,
            );
            pty.close_console = Some(close_sender);
            let writer_wake = wake.clone();
            pty.io_workers.push(
                thread::Builder::new()
                    .name("bed-conpty-write".into())
                    .spawn(move || {
                        let mut sink = File::from(input_write);
                        while let Ok(bytes) = input_receiver.recv() {
                            if let Err(error) = sink.write_all(&bytes) {
                                *write_error.lock().unwrap() = Some(error.to_string());
                                break;
                            }
                            notify(&writer_wake);
                        }
                        notify(&writer_wake);
                    })?,
            );

            let mut attribute_bytes = 0;
            unsafe {
                InitializeProcThreadAttributeList(std::ptr::null_mut(), 1, 0, &mut attribute_bytes);
            }
            // An aligned allocation owns the opaque variable-size attribute list.
            let mut attributes =
                vec![0usize; attribute_bytes.div_ceil(std::mem::size_of::<usize>())];
            let mut startup: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
            startup.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
            startup.lpAttributeList = attributes.as_mut_ptr().cast();
            if unsafe {
                InitializeProcThreadAttributeList(
                    startup.lpAttributeList,
                    1,
                    0,
                    &mut attribute_bytes,
                )
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
            struct Attributes(*mut c_void);
            impl Drop for Attributes {
                fn drop(&mut self) {
                    unsafe {
                        DeleteProcThreadAttributeList(self.0);
                    }
                }
            }
            let _attributes = Attributes(startup.lpAttributeList);
            if unsafe {
                UpdateProcThreadAttribute(
                    startup.lpAttributeList,
                    0,
                    PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
                    console as *const c_void,
                    std::mem::size_of::<HPCON>(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                )
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
            let shell = options.shell.clone().unwrap_or_else(|| {
                TerminalShell::new(
                    std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into()),
                    Vec::new(),
                )
            });
            let mut command = quote_argument(&shell.program);
            for argument in &shell.args {
                command.push(' ');
                command.push_str(&quote_argument(argument));
            }
            let mut command = wide(OsStr::new(&command))?;
            let directory = options
                .working_directory
                .as_ref()
                .map(|directory| wide(directory.as_os_str()))
                .transpose()?;
            let environment = environment(options)?;
            let mut information: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
            if unsafe {
                CreateProcessW(
                    std::ptr::null(),
                    command.as_mut_ptr(),
                    std::ptr::null(),
                    std::ptr::null(),
                    0,
                    EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT,
                    environment.as_ptr().cast(),
                    directory
                        .as_ref()
                        .map_or(std::ptr::null(), |directory| directory.as_ptr()),
                    &startup.StartupInfo,
                    &mut information,
                )
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
            let process = unsafe { OwnedHandle::from_raw_handle(information.hProcess.cast()) };
            let thread = unsafe { OwnedHandle::from_raw_handle(information.hThread.cast()) };
            drop(thread);
            pty.process_id = information.dwProcessId;
            pty.process = Some(process);
            Ok(pty)
        }
        pub fn process_id(&self) -> u32 {
            self.process_id
        }
        pub fn resize(&mut self, size: WindowSize) -> io::Result<()> {
            let console = self.console.ok_or(io::ErrorKind::NotConnected)?;
            let result = unsafe { ResizePseudoConsole(console, coord(size)) };
            if result == S_OK {
                Ok(())
            } else {
                Err(io::Error::other(format!(
                    "ResizePseudoConsole failed: {result:#x}"
                )))
            }
        }
        pub fn shutdown(&mut self) {
            if let Some(process) = self.process.as_ref() {
                unsafe {
                    TerminateProcess(process.as_raw_handle() as HANDLE, 1);
                    WaitForSingleObject(process.as_raw_handle() as HANDLE, 100);
                }
            }
            self.io_stop.store(true, Ordering::Release);
            self.writer.sender.take();
            self.begin_console_close();
            self.close_console.take();
            self.process.take();
            for worker in self.io_workers.drain(..) {
                if worker.is_finished() {
                    let _ = worker.join();
                }
            }
        }
        fn begin_console_close(&mut self) {
            if let Some(console) = self.console.take() {
                if let Some(sender) = self.close_console.as_ref()
                    && sender.send(console).is_ok()
                {
                    return;
                }
                // Only thread-construction failure can leave no close worker.
                // The read worker, when present, discards and drains concurrently.
                self.io_stop.store(true, Ordering::Release);
                unsafe { ClosePseudoConsole(console) };
            }
        }
    }
    impl Drop for Pty {
        fn drop(&mut self) {
            self.shutdown();
        }
    }
    impl EventedReadWrite for Pty {
        type Reader = PtyReader;
        type Writer = PtyWriter;
        unsafe fn register(
            &mut self,
            poller: &Arc<Poller>,
            _: Event,
            _: PollMode,
        ) -> io::Result<()> {
            *self.wake.lock().unwrap() = Some(poller.clone());
            poller.notify()
        }
        fn reregister(&mut self, _: &Arc<Poller>, _: Event, _: PollMode) -> io::Result<()> {
            Ok(())
        }
        fn deregister(&mut self, _: &Arc<Poller>) -> io::Result<()> {
            *self.wake.lock().unwrap() = None;
            Ok(())
        }
        fn reader(&mut self) -> &mut PtyReader {
            &mut self.reader
        }
        fn writer(&mut self) -> &mut PtyWriter {
            &mut self.writer
        }
    }
    impl EventedPty for Pty {
        fn next_child_event(&mut self) -> Option<ChildEvent> {
            if self.reported_exit {
                return None;
            }
            let process = self.process.as_ref()?;
            if unsafe { WaitForSingleObject(process.as_raw_handle() as HANDLE, 0) } != WAIT_OBJECT_0
            {
                return None;
            }
            self.reported_exit = true;
            let mut code = 0;
            let status =
                if unsafe { GetExitCodeProcess(process.as_raw_handle() as HANDLE, &mut code) } != 0
                {
                    Some(ExitStatus::from_raw(code))
                } else {
                    None
                };
            self.begin_console_close();
            Some(ChildEvent::Exited(status))
        }
    }
    fn notify(wake: &Wake) {
        if let Some(poller) = wake.lock().unwrap().as_ref() {
            let _ = poller.notify();
        }
    }
    fn coord(size: WindowSize) -> COORD {
        COORD {
            X: size.num_cols as i16,
            Y: size.num_lines as i16,
        }
    }
    fn wide(value: &OsStr) -> io::Result<Vec<u16>> {
        let mut value: Vec<u16> = value.encode_wide().collect();
        if value.contains(&0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "NUL in Windows process option",
            ));
        }
        value.push(0);
        Ok(value)
    }
    fn pipe() -> io::Result<(OwnedHandle, OwnedHandle)> {
        let mut reader = std::ptr::null_mut();
        let mut writer = std::ptr::null_mut();
        if unsafe { CreatePipe(&mut reader, &mut writer, std::ptr::null(), 0) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(unsafe {
            (
                OwnedHandle::from_raw_handle(reader.cast()),
                OwnedHandle::from_raw_handle(writer.cast()),
            )
        })
    }
    fn environment(options: &PtyOptions) -> io::Result<Vec<u16>> {
        let mut values = std::collections::BTreeMap::new();
        for (key, value) in std::env::vars_os() {
            values.insert(key.to_ascii_uppercase(), (key, value));
        }
        let mut overrides = options.env.clone();
        // Windows environment names are case-insensitive. A host's lowercase
        // override must win just as its uppercase spelling would.
        for (key, value) in [("TERM", "st-256color"), ("MSYS", "enable_pcon")] {
            if !overrides.keys().any(|name| name.eq_ignore_ascii_case(key)) {
                overrides.insert(key.into(), value.into());
            }
        }
        for (key, value) in overrides {
            if key.is_empty() || key.contains('=') {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Invalid terminal environment key",
                ));
            }
            values.insert(
                OsStr::new(&key).to_ascii_uppercase(),
                (key.into(), value.into()),
            );
        }
        let mut result = Vec::new();
        for (_, (key, value)) in values {
            let mut pair = key;
            pair.push("=");
            pair.push(value);
            result.extend(wide(&pair)?);
        }
        result.push(0);
        Ok(result)
    }
}

#[cfg(any(windows, test))]
fn quote_argument(argument: &str) -> String {
    let mut quoted = String::from("\"");
    let mut backslashes = 0;
    for character in argument.chars() {
        if character == '\\' {
            backslashes += 1;
            continue;
        }
        if character == '"' {
            quoted.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
        } else {
            quoted.extend(std::iter::repeat_n('\\', backslashes));
        }
        backslashes = 0;
        quoted.push(character);
    }
    quoted.extend(std::iter::repeat_n('\\', backslashes * 2));
    quoted.push('"');
    quoted
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
    fn windows_argv_quotes_program_spaces_quotes_and_final_backslashes() {
        assert_eq!(
            quote_argument("C:\\Program Files\\shell.exe"),
            "\"C:\\Program Files\\shell.exe\""
        );
        assert_eq!(quote_argument(""), "\"\"");
        assert_eq!(quote_argument("a\"b"), "\"a\\\"b\"");
        assert_eq!(quote_argument("path\\"), "\"path\\\\\"");
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
