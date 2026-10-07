//! Child-process ownership replacing lsp-framework's platform process wrappers.

use std::{
    ffi::OsString,
    io,
    path::{Path, PathBuf},
    process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, ExitStatus, Stdio},
    time::{Duration, Instant},
};

pub struct Process {
    child: Child,
}

pub struct ProcessPipes {
    pub stdin: ChildStdin,
    pub stdout: ChildStdout,
    pub stderr: ChildStderr,
}

#[derive(Clone, Debug, Default)]
pub struct ProcessOptions {
    pub working_directory: Option<PathBuf>,
    pub environment: Vec<(OsString, OsString)>,
}

impl Process {
    pub fn start(program: &Path, arguments: &[String]) -> io::Result<(Self, ProcessPipes)> {
        Self::start_with_options(program, arguments, &ProcessOptions::default())
    }
    pub fn start_with_options(
        program: &Path,
        arguments: &[String],
        options: &ProcessOptions,
    ) -> io::Result<(Self, ProcessPipes)> {
        let mut command = Command::new(program);
        command
            .args(arguments)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(directory) = &options.working_directory {
            command.current_dir(directory);
        }
        command.envs(
            options
                .environment
                .iter()
                .map(|(name, value)| (name, value)),
        );
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW, as upstream.
        }
        let mut child = command.spawn()?;
        let pipes = ProcessPipes {
            stdin: child.stdin.take().expect("requested child stdin"),
            stdout: child.stdout.take().expect("requested child stdout"),
            stderr: child.stderr.take().expect("requested child stderr"),
        };
        Ok((Self { child }, pipes))
    }

    pub fn id(&self) -> u32 {
        self.child.id()
    }
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }

    pub fn terminate(&mut self, grace: Duration) -> io::Result<ExitStatus> {
        let deadline = Instant::now() + grace;
        loop {
            if let Some(status) = self.child.try_wait()? {
                return Ok(status);
            }
            if Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        // Reap the child after killing it. Pipe workers may outlive it only when a
        // descendant inherited a descriptor; they own all their state and pipes.
        match self.child.kill() {
            Ok(()) => self.child.wait(),
            Err(error) => self.child.try_wait()?.ok_or(error),
        }
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.terminate(Duration::ZERO);
    }
}
