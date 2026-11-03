//! The environment language servers run in: the user's login shell's, read
//! once (an app started from the Dock or Finder doesn't have the PATH a
//! terminal has, so it couldn't find Homebrew's or Cargo's programs), and
//! finding programs on its PATH.
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::OnceLock,
    time::Duration,
};

/// The login shell's environment, read the first time it's asked for (it
/// can take a moment: call it from a worker). Without a shell that answers
/// within a few seconds, this process's own.
pub fn shell_environment() -> &'static BTreeMap<String, String> {
    static ENVIRONMENT: OnceLock<BTreeMap<String, String>> = OnceLock::new();
    ENVIRONMENT
        .get_or_init(|| read_shell_environment().unwrap_or_else(|| std::env::vars().collect()))
}

const MARKER: &str = "__BEDTIME_ENVIRONMENT__";

#[cfg(unix)]
fn read_shell_environment() -> Option<BTreeMap<String, String>> {
    use std::process::{Command, Stdio};
    let shell = std::env::var("SHELL")
        .ok()
        .filter(|shell| !shell.is_empty())?;
    // Interactive and login, so both rc files run; the marker skips anything
    // they print.
    let mut child = Command::new(&shell)
        .args([
            "-l",
            "-i",
            "-c",
            &format!("printf '{MARKER}'; /usr/bin/env -0"),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut output = child.stdout.take()?;
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = std::io::Read::read_to_end(&mut output, &mut bytes);
        let _ = sender.send(bytes);
    });
    let bytes = receiver.recv_timeout(Duration::from_secs(5));
    let _ = child.kill();
    let _ = child.wait();
    parse(&String::from_utf8_lossy(&bytes.ok()?))
}

#[cfg(not(unix))]
fn read_shell_environment() -> Option<BTreeMap<String, String>> {
    None
}

/// `env -0`'s output after the marker.
fn parse(output: &str) -> Option<BTreeMap<String, String>> {
    let (_, variables) = output.split_once(MARKER)?;
    let environment: BTreeMap<String, String> = variables
        .split('\0')
        .filter_map(|line| line.split_once('='))
        .filter(|(name, _)| !name.is_empty())
        .map(|(name, value)| (name.to_owned(), value.to_owned()))
        .collect();
    environment.contains_key("PATH").then_some(environment)
}

/// Where `program` is: itself if it's a path, else the first match on
/// `environment`'s PATH. Cargo's bin folder is tried last (rustup installs
/// rust-analyzer there).
pub fn find_program(program: &str, environment: &BTreeMap<String, String>) -> Option<PathBuf> {
    let program = expand_home(program, environment);
    if program.components().count() > 1 {
        return executable(&program).then_some(program);
    }
    let path = environment.get("PATH").cloned().unwrap_or_default();
    let cargo = environment
        .get("HOME")
        .map(|home| Path::new(home).join(".cargo/bin"));
    std::env::split_paths(&path)
        .chain(cargo)
        .map(|folder| folder.join(&program))
        .find(|candidate| executable(candidate))
}

fn expand_home(program: &str, environment: &BTreeMap<String, String>) -> PathBuf {
    match (program.strip_prefix("~/"), environment.get("HOME")) {
        (Some(rest), Some(home)) => Path::new(home).join(rest),
        _ => PathBuf::from(program),
    }
}

fn executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_marker_skips_what_rc_files_print() {
        let output = format!("Welcome!\n{MARKER}PATH=/bin:/usr/bin\0HOME=/Users/me\0EMPTY=\0");
        let environment = parse(&output).unwrap();
        assert_eq!(environment["PATH"], "/bin:/usr/bin");
        assert_eq!(environment["EMPTY"], "");
        assert!(parse("no marker").is_none());
    }

    #[test]
    fn programs_are_found_on_the_path_or_as_paths() {
        let environment: BTreeMap<String, String> =
            [("PATH".to_owned(), "/nowhere:/bin".to_owned())].into();
        assert_eq!(
            find_program("sh", &environment),
            Some(PathBuf::from("/bin/sh"))
        );
        assert_eq!(
            find_program("/bin/sh", &environment),
            Some(PathBuf::from("/bin/sh"))
        );
        assert!(find_program("not-a-program-anywhere", &environment).is_none());
    }
}
