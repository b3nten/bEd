//! Copy a bundled native helper into a private, content-addressed SSH cache.
use crate::{PROTOCOL_VERSION, SshTarget, shell_quote};
use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    path::Path,
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

const HOST_PROBE: &str = "uname -s && uname -m && printf '%s\\n' \"$HOME\"";
const HOME_PROBE: &str = "printf '%s' \"$HOME\"";
const DEPLOY_TIMEOUT: Duration = Duration::from_secs(60);
const OUTPUT_LIMIT: usize = 64 * 1024;

/// Resolve the helper by installing the bundled Linux executable over SSH.
/// The incoming agent field is ignored; callers receive the managed cache path.
/// Run this blocking operation on a consumer service/connection worker.
pub fn prepare_ssh_target(target: &SshTarget, helpers_dir: &Path) -> io::Result<SshTarget> {
    prepare_with(target, helpers_dir, |command, input| {
        run_ssh(target, command, input)
    })
}

/// Expand a leading `~` using the SSH account's HOME, preserving the remaining
/// path literally. Absolute paths require no SSH probe or local normalization.
/// Run this blocking operation on the consumer's connection worker.
pub fn expand_ssh_path(target: &SshTarget, path: &str) -> io::Result<String> {
    expand_with(path, || {
        validate_host(&target.host)?;
        run_ssh(target, HOME_PROBE, None).map_err(|error| contextual("SSH home probe", error))
    })
}

fn expand_with(path: &str, home: impl FnOnce() -> io::Result<String>) -> io::Result<String> {
    if path.contains('\0') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Remote project path contains NUL",
        ));
    }
    if path.starts_with('/') {
        return Ok(path.to_owned());
    }
    let suffix = if path == "~" {
        ""
    } else if let Some(suffix) = path.strip_prefix("~/") {
        suffix
    } else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Remote project path must be absolute or begin with ~/; ~otheruser and relative paths are unsupported",
        ));
    };
    let home = home()?;
    if !home.starts_with('/') || home.contains('\0') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "SSH account HOME must be an absolute path",
        ));
    }
    if suffix.is_empty() {
        return Ok(home);
    }
    Ok(format!("{}/{}", home.trim_end_matches('/'), suffix))
}

fn prepare_with(
    target: &SshTarget,
    helpers_dir: &Path,
    mut run: impl FnMut(&str, Option<File>) -> io::Result<String>,
) -> io::Result<SshTarget> {
    validate_host(&target.host)?;
    let probe = run(HOST_PROBE, None).map_err(|error| contextual("SSH host probe", error))?;
    let (triple, home) = parse_host(&probe)?;
    let artifact = helpers_dir.join(triple).join("bed-headless");
    let mut input = File::open(&artifact).map_err(|error| {
        io::Error::new(error.kind(), format!("Bundled SSH helper is unavailable at {}: {error}. Install a Bed package containing Linux remote helpers.", artifact.display()))
    })?;
    let (length, fingerprint) = fingerprint(&mut input)?;
    let cache = format!(
        "{}/.cache/bed/helpers/v{}-{}-{:016x}",
        home.trim_end_matches('/'),
        PROTOCOL_VERSION,
        length,
        fingerprint
    );
    let executable = format!("{cache}/bed-headless");
    let quoted_executable = shell_quote(&executable)?;
    let cached = format!(
        "if test -x {quoted_executable} && version=$({quoted_executable} --protocol-version 2>/dev/null) && test \"$version\" = {PROTOCOL_VERSION}; then printf 'ready\\n'; else printf 'missing\\n'; fi"
    );
    if run(&cached, None)
        .map_err(|error| contextual("SSH helper cache probe", error))?
        .trim()
        != "ready"
    {
        let directory = shell_quote(&cache)?;
        let temporary = shell_quote(&format!("{cache}/.upload.XXXXXX"))?;
        // A private temporary file receives stdin. Signals, failed transfers,
        // and executable/protocol probe failures remove it before returning.
        // Atomic replacement only touches our content-addressed cache entry.
        let install = format!(
            "set -eu; umask 077; mkdir -p {directory}; temp=$(mktemp {temporary}); trap 'rm -f \"$temp\"' 0 1 2 15; cat > \"$temp\"; test \"$(wc -c < \"$temp\")\" -eq {length}; chmod 700 \"$temp\"; version=$(\"$temp\" --protocol-version); test \"$version\" = {PROTOCOL_VERSION}; mv -f \"$temp\" {quoted_executable}; trap - 0 1 2 15; printf 'ready\\n'"
        );
        let installed = run(&install, Some(input))
            .map_err(|error| contextual("SSH helper installation", error))?;
        if installed.trim() != "ready" {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "SSH helper installation returned an unexpected response",
            ));
        }
    }
    Ok(SshTarget {
        host: target.host.clone(),
        agent: executable,
    })
}

fn parse_host(probe: &str) -> io::Result<(&'static str, &str)> {
    let mut lines = probe.lines();
    let system = lines.next().unwrap_or_default();
    let architecture = lines.next().unwrap_or_default();
    let home = lines.next().unwrap_or_default();
    if home.is_empty() || !home.starts_with('/') || home.contains('\0') || lines.next().is_some() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Unexpected SSH host probe response; remote HOME must be absolute and shell initialization must not print to stdout",
        ));
    }
    let triple = match (system, architecture) {
        ("Linux", "x86_64" | "amd64") => "x86_64-unknown-linux-musl",
        ("Linux", "aarch64" | "arm64") => "aarch64-unknown-linux-musl",
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!(
                    "Automatic SSH helper installation does not support {system}/{architecture}. Bundled helpers support Linux x86_64 and aarch64."
                ),
            ));
        }
    };
    Ok((triple, home))
}

fn validate_host(host: &str) -> io::Result<()> {
    if host.is_empty()
        || host.starts_with('-')
        || host.contains('\0')
        || host.chars().any(char::is_whitespace)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid SSH host or configuration alias",
        ));
    }
    Ok(())
}

fn fingerprint(input: &mut File) -> io::Result<(u64, u64)> {
    let metadata = input.metadata()?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > 128 * 1024 * 1024 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "bundled SSH helper must be a nonempty regular executable under 128 MiB",
        ));
    }
    let mut length = 0_u64;
    let mut hash = 0xcbf29ce484222325_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        length += count as u64;
        if length > 128 * 1024 * 1024 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "bundled SSH helper exceeds 128 MiB",
            ));
        }
        for byte in &buffer[..count] {
            hash = (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3);
        }
    }
    if length != metadata.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "bundled SSH helper changed while being read",
        ));
    }
    input.seek(SeekFrom::Start(0))?;
    Ok((length, hash))
}

fn contextual(operation: &str, error: io::Error) -> io::Error {
    io::Error::new(error.kind(), format!("{operation} failed: {error}"))
}

fn run_ssh(target: &SshTarget, command: &str, input: Option<File>) -> io::Result<String> {
    let mut process = Command::new("ssh");
    process.args([
        "-T",
        "-o",
        "BatchMode=yes",
        "-o",
        "ConnectTimeout=10",
        "-o",
        "ServerAliveInterval=5",
        "-o",
        "ServerAliveCountMax=2",
        "--",
        &target.host,
    ]);
    // Select POSIX sh explicitly; the account's interactive shell may use a
    // different assignment/conditional syntax (for example fish).
    process.arg(format!("sh -c {}", shell_quote(command)?));
    run_process(process, input, DEPLOY_TIMEOUT)
}

fn run_process(mut command: Command, input: Option<File>, timeout: Duration) -> io::Result<String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    command
        .stdin(input.map(Stdio::from).unwrap_or_else(Stdio::null))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn()?;
    let stdout = capture(
        child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("SSH stdout unavailable"))?,
    );
    let stderr = capture(
        child
            .stderr
            .take()
            .ok_or_else(|| io::Error::other("SSH stderr unavailable"))?,
    );
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < timeout => thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "SSH helper operation timed out",
                ));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        }
    };
    let output = stdout
        .recv_timeout(timeout.saturating_sub(started.elapsed()))
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "SSH stdout did not close"))??;
    let errors = stderr
        .recv_timeout(timeout.saturating_sub(started.elapsed()))
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "SSH stderr did not close"))??;
    if !status.success() {
        let message = String::from_utf8_lossy(&errors).trim().to_owned();
        return Err(io::Error::other(if message.is_empty() {
            format!("SSH exited with {status}")
        } else {
            message
        }));
    }
    String::from_utf8(output).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn capture(mut input: impl Read + Send + 'static) -> mpsc::Receiver<io::Result<Vec<u8>>> {
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let result = (|| {
            let mut output = Vec::new();
            let mut buffer = [0_u8; 4096];
            loop {
                let count = input.read(&mut buffer)?;
                if count == 0 {
                    return Ok(output);
                }
                let retained = count.min(OUTPUT_LIMIT.saturating_sub(output.len()));
                output.extend_from_slice(&buffer[..retained]);
            }
        })();
        let _ = sender.send(result);
    });
    receiver
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_detection_and_managed_installation_are_explicit() {
        assert_eq!(
            parse_host("Linux\nx86_64\n/home/user\n").unwrap().0,
            "x86_64-unknown-linux-musl"
        );
        assert_eq!(
            parse_host("Linux\naarch64\n/home/user\n").unwrap().0,
            "aarch64-unknown-linux-musl"
        );
        assert_eq!(
            parse_host("Darwin\narm64\n/Users/user\n")
                .unwrap_err()
                .kind(),
            io::ErrorKind::Unsupported
        );
        assert_eq!(
            parse_host("Linux\nriscv64\n/home/user\n")
                .unwrap_err()
                .kind(),
            io::ErrorKind::Unsupported
        );
        assert!(parse_host("banner\nLinux\naarch64\n/home/user\n").is_err());
        let obsolete = SshTarget {
            host: "alias".into(),
            agent: "/custom/bed helper".into(),
        };
        let probed = std::cell::Cell::new(false);
        let error = prepare_with(&obsolete, Path::new("/missing"), |command, _| {
            assert_eq!(command, HOST_PROBE);
            probed.set(true);
            Ok("Linux\naarch64\n/home/user\n".into())
        })
        .unwrap_err();
        assert!(probed.get());
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert!(!error.to_string().contains("explicit remote executable"));
    }

    #[cfg(unix)]
    #[test]
    fn install_reuses_cache_quotes_home_and_cleans_failed_uploads() {
        use std::{
            fs,
            sync::atomic::{AtomicU64, Ordering},
        };
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let temp = std::env::temp_dir().join(format!(
            "bed-deployment-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let home = temp.join("home with spaces ' and $(literal)");
        let helpers = temp.join("bundled");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(helpers.join("aarch64-unknown-linux-musl")).unwrap();
        let binary = helpers.join("aarch64-unknown-linux-musl/bed-headless");
        fs::write(
            &binary,
            format!("#!/bin/sh\nprintf '{}\\n'\n", PROTOCOL_VERSION),
        )
        .unwrap();
        let target = SshTarget {
            host: "alias".into(),
            agent: "/obsolete/custom helper".into(),
        };
        let uploads = std::cell::Cell::new(0);
        let mut run = |command: &str, input: Option<File>| {
            if command == HOST_PROBE {
                return Ok(format!("Linux\naarch64\n{}\n", home.display()));
            }
            if input.is_some() {
                uploads.set(uploads.get() + 1);
            }
            let mut shell = Command::new("sh");
            shell.args(["-c", command]);
            run_process(shell, input, Duration::from_secs(5))
        };
        let resolved = prepare_with(&target, &helpers, &mut run).unwrap();
        assert_ne!(resolved.agent, target.agent);
        assert!(Path::new(&resolved.agent).is_file());
        assert_eq!(prepare_with(&target, &helpers, &mut run).unwrap(), resolved);
        assert_eq!(uploads.get(), 1);
        fs::write(
            &binary,
            format!("#!/bin/sh\n# new build\nprintf '{}\\n'\n", PROTOCOL_VERSION),
        )
        .unwrap();
        let fresh = prepare_with(&target, &helpers, &mut run).unwrap();
        assert_ne!(fresh.agent, resolved.agent);
        assert!(Path::new(&resolved.agent).is_file());
        assert_eq!(uploads.get(), 2);
        fs::write(&fresh.agent, "#!/bin/sh\nprintf 'unsupported\\n'\n").unwrap();
        assert_eq!(prepare_with(&target, &helpers, &mut run).unwrap(), fresh);
        assert_eq!(uploads.get(), 3);
        fs::write(&binary, "#!/bin/sh\nexit 9\n").unwrap();
        let failure = prepare_with(&target, &helpers, |command, input| {
            if command == HOST_PROBE {
                return Ok(format!("Linux\naarch64\n{}\n", home.display()));
            }
            let mut shell = Command::new("sh");
            shell.args(["-c", command]);
            run_process(shell, input, Duration::from_secs(5))
        })
        .unwrap_err();
        assert!(failure.to_string().contains("installation"));
        for directory in fs::read_dir(home.join(".cache/bed/helpers")).unwrap() {
            for entry in fs::read_dir(directory.unwrap().path()).unwrap() {
                assert!(
                    !entry
                        .unwrap()
                        .file_name()
                        .to_string_lossy()
                        .starts_with(".upload.")
                );
            }
        }
        fs::remove_dir_all(temp).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn process_timeout_and_diagnostics_are_bounded() {
        let mut command = Command::new("sh");
        command.args(["-c", "exec sleep 5"]);
        let started = Instant::now();
        assert_eq!(
            run_process(command, None, Duration::from_millis(50))
                .unwrap_err()
                .kind(),
            io::ErrorKind::TimedOut
        );
        assert!(started.elapsed() < Duration::from_secs(2));
        let mut command = Command::new("sh");
        command.args(["-c", "printf 'permission denied' >&2; exit 3"]);
        assert!(
            run_process(command, None, Duration::from_secs(2))
                .unwrap_err()
                .to_string()
                .contains("permission denied")
        );
    }

    #[test]
    fn remote_tilde_expansion_preserves_literal_paths_and_rejects_ambiguous_forms() {
        let home = "/remote/home with spaces ' and $(literal)";
        for path in ["~", "~/"] {
            assert_eq!(expand_with(path, || Ok(home.into())).unwrap(), home);
        }
        let suffix = "Dev/project ' $(touch marker) [*]?";
        assert_eq!(
            expand_with(&format!("~/{suffix}"), || Ok(home.into())).unwrap(),
            format!("{home}/{suffix}")
        );
        assert_eq!(expand_with("~/child", || Ok("/".into())).unwrap(), "/child");
        let absolute = "/remote/path ' $(literal) [*]?";
        assert_eq!(
            expand_with(absolute, || panic!("absolute paths must not probe HOME")).unwrap(),
            absolute
        );
        for path in [
            "",
            "relative",
            "./project",
            "~otheruser/project",
            "~otheruser",
            "~\\project",
            "/has\0nul",
            "~/has\0nul",
        ] {
            assert_eq!(
                expand_with(path, || panic!("invalid paths must not probe HOME"))
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::InvalidInput
            );
        }
        for invalid_home in ["", "relative", "/has\0nul"] {
            assert_eq!(
                expand_with("~/project", || Ok(invalid_home.into()))
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::InvalidData
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn home_probe_reads_environment_without_evaluating_its_contents() {
        let home = "/home/a ' $(echo unsafe) [*]?";
        let mut command = Command::new("sh");
        command.args(["-c", HOME_PROBE]).env("HOME", home);
        let read = run_process(command, None, Duration::from_secs(2)).unwrap();
        assert_eq!(read, home);
        assert_eq!(
            expand_with("~/Dev/foo", || Ok(read)).unwrap(),
            format!("{home}/Dev/foo")
        );
    }

    #[test]
    #[ignore = "requires BED_TEST_SSH_HOST and an authorized SSH test server"]
    fn ssh_tilde_expansion_uses_remote_home_and_keeps_literal_suffix() {
        let host = std::env::var("BED_TEST_SSH_HOST").expect("set BED_TEST_SSH_HOST");
        let target = SshTarget::new(host);
        let home = expand_ssh_path(&target, "~").unwrap();
        assert!(home.starts_with('/'));
        let suffix = "Dev/bed path ' $(touch bed-unexpected-marker) [*]?";
        let expanded = expand_ssh_path(&target, &format!("~/{suffix}")).unwrap();
        assert_eq!(expanded, format!("{}/{suffix}", home.trim_end_matches('/')));
        println!("Remote HOME: {home}");
        println!("Literal expanded path: {expanded}");
    }
}
