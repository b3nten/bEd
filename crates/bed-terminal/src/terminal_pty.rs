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
        Arc, Mutex,
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
    shell_name: String,
    foreground_process: Arc<Mutex<Option<String>>>,
}

impl TerminalPty {
    pub fn spawn(options: &PtyOptions, size: WindowSize) -> io::Result<Self> {
        validate_size(size)?;
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
                ) {
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
            shell_name,
            foreground_process,
        })
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
    foreground_process: &Mutex<Option<String>>,
) -> io::Result<()> {
    let mut writes: VecDeque<QueuedWrite> = VecDeque::new();
    let mut events = Events::new();
    let mut child_exit = None;
    let mut exit_deadline = None;
    let mut read_closed = false;
    let mut next_title_poll = Instant::now();
    while !stop.load(Ordering::Acquire) {
        if Instant::now() >= next_title_poll {
            let title = backend.foreground_process_name();
            *foreground_process.lock().unwrap() = title;
            next_title_poll = Instant::now() + PROCESS_TITLE_INTERVAL;
        }
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
        // WouldBlock after a reaped child means the kernel master has drained
        // its pending output.
        if let Some(status) = child_exit
            && (drained || exit_deadline.is_some_and(|deadline| Instant::now() >= deadline))
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
            unix::process::{CommandExt, ExitStatusExt},
        },
        process::{Child, Command},
    };

    pub struct Pty {
        child: Child,
        shell_name: String,
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
                .env("TERM_PROGRAM", "st-imgui");
            for key in &options.env_remove {
                command.env_remove(key);
            }
            command.envs(&options.env);
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
                shell_name: crate::process_title::program_name(&shell.program),
                file: Some(file),
                reported_exit: false,
            })
        }
        pub fn process_id(&self) -> u32 {
            self.child.id()
        }
        pub fn shell_name(&self) -> &str {
            &self.shell_name
        }
        pub fn foreground_process_name(&self) -> Option<String> {
            let file = self.file.as_ref()?;
            // SAFETY: the master file is owned and remains open on this worker.
            let group = unsafe { libc::tcgetpgrp(file.as_raw_fd()) };
            let pid = if group > 0 {
                group
            } else {
                self.child.id() as libc::pid_t
            };
            crate::process_title::process_name(pid)
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
            if self.reported_exit || self.next_child_event().is_some() {
                return;
            }
            let group = -(self.child.id() as i32);
            unsafe {
                libc::kill(group, libc::SIGHUP);
                libc::kill(group, libc::SIGTERM);
            }
            let deadline = Instant::now() + Duration::from_millis(100);
            while Instant::now() < deadline {
                if self.next_child_event().is_some() {
                    return;
                }
                thread::sleep(Duration::from_millis(5));
            }
            unsafe {
                libc::kill(group, libc::SIGKILL);
            }
            let _ = self.child.kill();
            // Worker owns this wait; never the UI thread. Avoid Child's cached
            // status, which cannot distinguish a debugger stop from an exit.
            loop {
                let mut status = 0;
                let pid = unsafe { libc::waitpid(self.child.id() as i32, &mut status, 0) };
                if pid > 0 && (libc::WIFEXITED(status) || libc::WIFSIGNALED(status)) {
                    self.reported_exit = true;
                    break;
                }
                if pid < 0 && io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
                    break;
                }
            }
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
            let mut status = 0;
            let pid = unsafe {
                libc::waitpid(self.child.id() as libc::pid_t, &mut status, libc::WNOHANG)
            };
            if pid > 0 {
                // Traced children can report stops even without WUNTRACED.
                // Child::try_wait caches those as an ExitStatus, so use raw
                // waitpid and retain ownership through stop/continue events.
                if libc::WIFEXITED(status) || libc::WIFSIGNALED(status) {
                    self.reported_exit = true;
                    return Some(ChildEvent::Exited(Some(ExitStatus::from_raw(status))));
                }
                return None;
            }
            if pid == 0 {
                return None;
            }
            let error = io::Error::last_os_error();
            match error.raw_os_error() {
                Some(libc::EINTR) => None,
                Some(libc::ECHILD) => {
                    // LLDB/debugserver can temporarily own wait notifications
                    // after attaching to a child. Closing its PTY here would
                    // send SIGHUP and kill an otherwise live debug session.
                    let exists = unsafe { libc::kill(self.child.id() as libc::pid_t, 0) };
                    if exists == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
                    {
                        None
                    } else {
                        self.reported_exit = true;
                        Some(ChildEvent::Exited(None))
                    }
                }
                _ => {
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
