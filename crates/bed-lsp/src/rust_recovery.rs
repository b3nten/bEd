//! Rustup startup failures and the narrowly scoped commands that repair them.
//! Workers collect facts/output; the workspace polling thread owns recovery.
use crate::{message_handler::MAX_STDERR_BYTES, process::ProcessOptions};
use std::{
    collections::VecDeque,
    io::{self, Read},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RustStartupIssueKind {
    MissingComponents,
    MissingToolchain,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RustStartupIssue {
    pub toolchain: String,
    pub kind: RustStartupIssueKind,
    pub install_available: bool,
}

#[derive(Clone, Debug)]
pub struct RustInstallStatus {
    pub running: bool,
    pub message: String,
    pub output: String,
}

impl RustStartupIssue {
    pub fn message(&self) -> String {
        match self.kind {
            RustStartupIssueKind::MissingComponents => format!(
                "Rust toolchain `{}` is missing rust-analyzer.",
                self.toolchain
            ),
            RustStartupIssueKind::MissingToolchain => {
                format!("Rust toolchain `{}` is not installed.", self.toolchain)
            }
        }
    }

    pub fn arguments(&self) -> Vec<String> {
        match self.kind {
            RustStartupIssueKind::MissingComponents => vec![
                "component",
                "add",
                "--toolchain",
                &self.toolchain,
                "rust-analyzer",
                "rust-src",
            ],
            RustStartupIssueKind::MissingToolchain => vec![
                "toolchain",
                "install",
                &self.toolchain,
                "--profile",
                "default",
                "--component",
                "rust-analyzer,rust-src",
                "--no-self-update",
            ],
        }
        .into_iter()
        .map(str::to_owned)
        .collect()
    }

    pub fn command(&self) -> String {
        format!("rustup {}", self.arguments().join(" "))
    }
}

/// Recognize explicit rustup failures, rather than interpreting EOF as a
/// missing dependency. Toolchain names come from external stderr.
pub(crate) fn diagnose(stderr: &str) -> Option<RustStartupIssue> {
    for line in stderr.lines() {
        let Some(line) = line.trim().strip_prefix("error: ") else {
            continue;
        };
        let (toolchain, kind) = if let Some(rest) = line
            .strip_prefix("Unknown binary 'rust-analyzer' in official toolchain '")
            .or_else(|| line.strip_prefix("unknown binary 'rust-analyzer' in toolchain '"))
        {
            (
                rest.split_once('\'')?.0,
                RustStartupIssueKind::MissingComponents,
            )
        } else if let Some(rest) =
            line.strip_prefix("'rust-analyzer' is not installed for the toolchain '")
        {
            (
                rest.split_once('\'')?.0,
                RustStartupIssueKind::MissingComponents,
            )
        } else if let Some(rest) = line
            .strip_prefix("toolchain '")
            .or_else(|| line.strip_prefix("override toolchain '"))
        {
            let (name, suffix) = rest.split_once('\'')?;
            if !suffix.starts_with(" is not installed") {
                continue;
            }
            (name, RustStartupIssueKind::MissingToolchain)
        } else {
            continue;
        };
        if official_toolchain(toolchain) {
            return Some(RustStartupIssue {
                toolchain: toolchain.into(),
                kind,
                install_available: false,
            });
        }
    }
    None
}

fn official_toolchain(name: &str) -> bool {
    if name.is_empty()
        || name.len() > 160
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.' || b == b'_')
    {
        return false;
    }
    let channel = name.split('-').next().unwrap();
    let valid_channel = matches!(channel, "stable" | "beta" | "nightly")
        || (channel.split('.').count() >= 2
            && channel.split('.').count() <= 3
            && channel
                .split('.')
                .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit())));
    if !valid_channel {
        return false;
    }
    let mut suffix: Vec<_> = name.split('-').skip(1).collect();
    if channel.as_bytes()[0].is_ascii_digit()
        && suffix
            .first()
            .is_some_and(|part| *part == "beta" || part.starts_with("beta."))
    {
        suffix.remove(0);
    }
    if suffix.len() >= 3 && suffix[0].len() == 4 && suffix[0].bytes().all(|b| b.is_ascii_digit()) {
        if !suffix[1..3]
            .iter()
            .all(|part| part.len() == 2 && part.bytes().all(|b| b.is_ascii_digit()))
        {
            return false;
        }
        suffix.drain(..3);
    }
    suffix.is_empty() || (suffix.len() >= 3 && suffix.iter().all(|part| !part.is_empty()))
}

#[derive(Clone)]
pub(crate) struct RustupLaunch {
    pub program: PathBuf,
    pub options: ProcessOptions,
}

impl RustupLaunch {
    pub fn for_server(program: &Path, options: &ProcessOptions) -> Option<Self> {
        let path = options
            .environment
            .iter()
            .rev()
            .find(|(key, _)| key == "PATH")?
            .1
            .clone();
        let rustup = std::env::split_paths(&path)
            .map(|directory| directory.join("rustup"))
            .find(|candidate| candidate.is_file())?;
        // Rustup proxies may be symlinks or hardlinks. This comparison is only
        // for diagnosis; launching still preserves the proxy's original name.
        if program.exists() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                let proxy = program.metadata().ok()?;
                let actual = rustup.metadata().ok()?;
                if proxy.dev() != actual.dev() || proxy.ino() != actual.ino() {
                    return None;
                }
            }
            #[cfg(not(unix))]
            if program.canonicalize().ok()? != rustup.canonicalize().ok()? {
                return None;
            }
        }
        // Capture inherited variables as well as configured overrides so the
        // diagnostic and installer use precisely the launch environment.
        let mut environment: std::collections::BTreeMap<_, _> = std::env::vars_os().collect();
        environment.extend(options.environment.iter().cloned());
        Some(Self {
            program: rustup,
            options: ProcessOptions {
                working_directory: options.working_directory.clone(),
                environment: environment.into_iter().collect(),
            },
        })
    }

    pub fn verify(&self, mut issue: RustStartupIssue) -> Option<RustStartupIssue> {
        let active = self.query(&["show", "active-toolchain"])?;
        match issue.kind {
            RustStartupIssueKind::MissingToolchain => {
                let selected = diagnose(&active.1)?;
                if active.0 || selected.kind != issue.kind || selected.toolchain != issue.toolchain
                {
                    return None;
                }
                // Custom toolchains cannot be installed from a distribution.
                let listed = self.query(&["toolchain", "list"])?;
                if !listed.0
                    || listed.1.lines().any(|line| {
                        line.split_whitespace().next() == Some(issue.toolchain.as_str())
                    })
                {
                    return None;
                }
            }
            RustStartupIssueKind::MissingComponents => {
                if !active.0 {
                    return None;
                }
                let selected = active.1.split_whitespace().next()?;
                if !official_toolchain(selected) {
                    return None;
                }
                if selected != issue.toolchain
                    && !selected.starts_with(&format!("{}-", issue.toolchain))
                {
                    return None;
                }
                issue.toolchain = selected.into();
                let components =
                    self.query(&["component", "list", "--toolchain", &issue.toolchain])?;
                if !components.0
                    || !components.1.lines().any(|line| {
                        line.starts_with("rust-analyzer-") && !line.contains("(installed)")
                    })
                    || !components
                        .1
                        .lines()
                        .any(|line| line == "rust-src" || line.starts_with("rust-src "))
                {
                    return None;
                }
            }
        }
        issue.install_available = true;
        Some(issue)
    }

    fn query(&self, arguments: &[&str]) -> Option<(bool, String)> {
        let mut launch = self.clone();
        launch
            .options
            .environment
            .push(("RUSTUP_AUTO_INSTALL".into(), "0".into()));
        let arguments: Vec<_> = arguments.iter().map(|arg| (*arg).to_owned()).collect();
        let output = Arc::new(Mutex::new(VecDeque::new()));
        let status = launch
            .run(&arguments, output.clone(), Some(Duration::from_secs(5)))
            .ok()?;
        Some((status.success(), output_text(&output)))
    }

    pub fn run(
        &self,
        arguments: &[String],
        output: SharedOutput,
        timeout: Option<Duration>,
    ) -> io::Result<std::process::ExitStatus> {
        let mut child = Command::new(&self.program)
            .args(arguments)
            .current_dir(
                self.options
                    .working_directory
                    .as_deref()
                    .unwrap_or(Path::new(".")),
            )
            .env_clear()
            .envs(self.options.environment.iter().cloned())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let pipes: [Box<dyn Read + Send>; 2] = [
            Box::new(child.stdout.take().unwrap()),
            Box::new(child.stderr.take().unwrap()),
        ];
        let mut readers = Vec::new();
        for mut pipe in pipes {
            let output = output.clone();
            readers.push(thread::spawn(move || {
                let mut bytes = [0; 4096];
                loop {
                    match pipe.read(&mut bytes) {
                        Ok(0) => break,
                        Ok(count) => {
                            let mut retained = output.lock().unwrap();
                            let excess = retained
                                .len()
                                .saturating_add(count)
                                .saturating_sub(MAX_STDERR_BYTES);
                            retained.drain(..excess);
                            retained.extend(&bytes[..count]);
                        }
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                        Err(_) => break,
                    }
                }
            }));
        }
        let started = Instant::now();
        let result = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Ok(status),
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break Err(error);
                }
                Ok(None) => {}
            }
            if timeout.is_some_and(|limit| started.elapsed() >= limit) {
                let _ = child.kill();
                let _ = child.wait();
                break Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Rustup diagnostic timed out",
                ));
            }
            thread::sleep(Duration::from_millis(10));
        };
        // A descendant may inherit a pipe. Do not let that hold a query open.
        let drain_deadline = Instant::now() + Duration::from_millis(100);
        while readers.iter().any(|reader| !reader.is_finished()) && Instant::now() < drain_deadline
        {
            thread::sleep(Duration::from_millis(2));
        }
        for reader in readers {
            if reader.is_finished() {
                let _ = reader.join();
            }
        }
        result
    }
}

pub(crate) type SharedOutput = Arc<Mutex<VecDeque<u8>>>;

pub(crate) fn output_text(output: &SharedOutput) -> String {
    String::from_utf8_lossy(&output.lock().unwrap().iter().copied().collect::<Vec<_>>())
        .into_owned()
}

pub(crate) fn probe(
    launch: RustupLaunch,
    issue: RustStartupIssue,
) -> mpsc::Receiver<Option<RustStartupIssue>> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let _ = sender.send(launch.verify(issue));
    });
    receiver
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_rustup_errors_and_rejects_unrelated_or_custom_failures() {
        for error in [
            "log line\nerror: Unknown binary 'rust-analyzer' in official toolchain '1.99.0-aarch64-apple-darwin'.",
            "error: unknown binary 'rust-analyzer' in toolchain 'stable-x86_64-unknown-linux-gnu'",
            "error: 'rust-analyzer' is not installed for the toolchain 'nightly-2026-10-01-x86_64-unknown-linux-gnu'",
        ] {
            assert_eq!(
                diagnose(error).unwrap().kind,
                RustStartupIssueKind::MissingComponents
            );
        }
        for error in [
            "error: toolchain '1.99.0' is not installed",
            "error: override toolchain '1.99.0' is not installed: the RUSTUP_TOOLCHAIN environment variable specifies an uninstalled toolchain",
        ] {
            assert_eq!(
                diagnose(error).unwrap().kind,
                RustStartupIssueKind::MissingToolchain
            );
        }
        for error in [
            "LSP header reached EOF",
            "error: rust-analyzer panicked",
            "error: toolchain 'my-local-build' is not installed",
            "error: toolchain 'stable-custom' is not installed",
            "error: toolchain '--override' is not installed",
            "error: toolchain 'stable; touch nope' is not installed",
            "error: toolchain '1.99.0' is unavailable",
            "error: Unknown binary 'rustfmt' in official toolchain '1.99.0-aarch64-apple-darwin'.",
        ] {
            assert!(diagnose(error).is_none(), "{error}");
        }
    }
}
