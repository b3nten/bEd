//! Find the active Rust toolchain's LLDB formatters without invoking a shell.
use crate::process::{self, Stream};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicBool, Ordering},
};

#[derive(Clone, Debug, Default)]
pub struct RustFormatters {
    /// LLDB startup commands that load Rust formatters and disable recursive struct summaries.
    pub commands: Vec<String>,
    /// An unavailable formatter installation does not prevent debugging.
    pub notice: Option<String>,
}

// Rust's generic struct summary recursively asks every field for its summary.
// Large application types such as Bevy App can time out inspection requests,
// blocking every later request. Keep their synthetic children for
// explicit expansion, along with the useful String and collection summaries.
const DISABLE_RECURSIVE_STRUCT_SUMMARIES: &str = concat!(
    "script _bed_rust = lldb.debugger.GetCategory(\"Rust\"); ",
    "[_bed_rust.DeleteTypeSummary(_bed_rust.GetTypeNameSpecifierForSummaryAtIndex(i)) ",
    "for i in reversed(range(_bed_rust.GetNumSummaries())) ",
    "if _bed_rust.GetSummaryAtIndex(i).IsFunctionName() ",
    "and _bed_rust.GetSummaryAtIndex(i).GetData() == ",
    "\"lldb_lookup.StructSummaryProvider\"]"
);

impl RustFormatters {
    fn unavailable(reason: impl std::fmt::Display) -> Self {
        Self {
            commands: Vec::new(),
            notice: Some(format!("Rust pretty printers unavailable: {reason}")),
        }
    }
}

/// Probe `rustc` in the build workspace so rustup honors the same local toolchain
/// files and environment as the project's build command.
/// This is a worker-side operation; cancellation terminates the probe process.
/// Missing tools or formatter files return a nonfatal notice instead of an error.
pub fn discover(
    workspace: &Path,
    environment: &BTreeMap<String, String>,
    cancel: &AtomicBool,
) -> Result<RustFormatters, String> {
    if cancel.load(Ordering::Acquire) {
        return Err("Build cancelled".into());
    }
    let workspace = if workspace.is_absolute() {
        workspace.to_owned()
    } else {
        match std::env::current_dir() {
            Ok(directory) => directory.join(workspace),
            Err(error) => return Ok(RustFormatters::unavailable(error)),
        }
    };
    let compiler = environment
        .get("RUSTC")
        .map(std::ffi::OsString::from)
        .or_else(|| std::env::var_os("RUSTC"))
        .unwrap_or_else(|| "rustc".into());
    let mut command = Command::new(compiler);
    command.args(["--print", "sysroot"]).current_dir(&workspace);
    if let Some(path) = process::child_path() {
        command.env("PATH", path);
    }
    command.envs(environment);
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let result = process::run(command, cancel, |stream, bytes| {
        let output = match stream {
            Stream::Stdout => &mut stdout,
            Stream::Stderr => &mut stderr,
        };
        if output.len().saturating_add(bytes.len()) > 64 * 1024 {
            return Err("rustc sysroot output exceeds 64 KiB".into());
        }
        output.extend_from_slice(bytes);
        Ok(())
    });
    if cancel.load(Ordering::Acquire) {
        return Err("Build cancelled".into());
    }
    match result {
        Ok(status) if status.success() => {}
        Ok(status) => {
            let details = String::from_utf8_lossy(&stderr);
            return Ok(RustFormatters::unavailable(format!(
                "rustc --print sysroot failed ({status}): {}",
                details.trim()
            )));
        }
        Err(error) => return Ok(RustFormatters::unavailable(error)),
    }
    let sysroot = match std::str::from_utf8(&stdout).map(|path| path.trim_end_matches(['\r', '\n']))
    {
        Ok(path) if !path.is_empty() => PathBuf::from(path),
        _ => {
            return Ok(RustFormatters::unavailable(
                "rustc returned no valid sysroot",
            ));
        }
    };
    let sysroot = if sysroot.is_absolute() {
        sysroot
    } else {
        workspace.join(sysroot)
    };
    Ok(commands_for_sysroot(&sysroot))
}

fn commands_for_sysroot(sysroot: &Path) -> RustFormatters {
    let directory = sysroot.join("lib/rustlib/etc");
    let lookup = directory.join("lldb_lookup.py");
    if !lookup.is_file() {
        return RustFormatters::unavailable(format!("{} is missing", lookup.display()));
    }
    let lookup = match quote_path(&lookup) {
        Ok(path) => path,
        Err(reason) => return RustFormatters::unavailable(reason),
    };
    // Recent toolchains register summaries and synthetic children from the
    // imported module. Older toolchains ship the registrations in a second file.
    let mut commands = vec![format!("command script import {lookup}")];
    let registrations = directory.join("lldb_commands");
    if registrations.is_file() {
        let registrations = match quote_path(&registrations) {
            Ok(path) => path,
            Err(reason) => return RustFormatters::unavailable(reason),
        };
        commands.push(format!("command source {registrations}"));
    }
    commands.push(DISABLE_RECURSIVE_STRUCT_SUMMARIES.into());
    RustFormatters {
        commands,
        notice: None,
    }
}

fn quote_path(path: &Path) -> Result<String, &'static str> {
    let path = path.to_str().ok_or("formatter path is not valid UTF-8")?;
    if path.chars().any(char::is_control) {
        return Err("formatter path contains a control character");
    }
    Ok(format!(
        "\"{}\"",
        path.replace('\\', "\\\\").replace('"', "\\\"")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        sync::atomic::AtomicU64,
        time::{Duration, Instant},
    };

    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "bed Rust printers {} {}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path.canonicalize().unwrap())
        }
        fn write(&self, name: &str, contents: &str) -> PathBuf {
            let path = self.0.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, contents).unwrap();
            path
        }
        fn printers(&self, legacy: bool) -> PathBuf {
            let root = self.0.join("toolchain with spaces");
            self.write(
                "toolchain with spaces/lib/rustlib/etc/lldb_lookup.py",
                "# fixture\n",
            );
            if legacy {
                self.write(
                    "toolchain with spaces/lib/rustlib/etc/lldb_commands",
                    "# fixture\n",
                );
            }
            root
        }
        #[cfg(unix)]
        fn compiler(&self, script: &str) -> PathBuf {
            use std::os::unix::fs::PermissionsExt;
            let path = self.write("fake compiler", script);
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            path
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn modern_import_registers_printers_and_legacy_commands_follow_import() {
        let fixture = Fixture::new();
        let sysroot = fixture.printers(false);
        let modern = commands_for_sysroot(&sysroot);
        assert_eq!(modern.commands.len(), 2);
        assert!(modern.commands[0].starts_with("command script import \""));
        assert!(modern.commands[0].ends_with("lldb_lookup.py\""));
        assert!(modern.notice.is_none());
        assert_eq!(modern.commands[1], DISABLE_RECURSIVE_STRUCT_SUMMARIES);
        fixture.printers(true);
        let legacy = commands_for_sysroot(&sysroot);
        assert_eq!(legacy.commands.len(), 3);
        assert_eq!(legacy.commands[0], modern.commands[0]);
        assert!(legacy.commands[1].starts_with("command source \""));
        assert!(legacy.commands[1].ends_with("lldb_commands\""));
        assert_eq!(legacy.commands[2], DISABLE_RECURSIVE_STRUCT_SUMMARIES);
    }

    #[test]
    fn missing_formatters_are_nonfatal() {
        let fixture = Fixture::new();
        let result = commands_for_sysroot(&fixture.0);
        assert!(result.commands.is_empty());
        assert!(result.notice.unwrap().contains("lldb_lookup.py"));
    }

    #[test]
    fn lldb_paths_escape_quotes_and_backslashes_and_reject_controls() {
        assert_eq!(
            quote_path(Path::new("directory with spaces/a\"b\\c.py")).unwrap(),
            "\"directory with spaces/a\\\"b\\\\c.py\""
        );
        for path in ["a\nb", "a\rb", "a\0b", "a\tb", "a\u{7f}b"] {
            assert!(quote_path(Path::new(path)).is_err(), "{path:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn probe_respects_compiler_environment_and_workspace_toolchain_directory() {
        let fixture = Fixture::new();
        let sysroot = fixture.printers(false);
        fixture.write("rust-toolchain.toml", "fixture marker");
        let compiler = fixture.compiler(
            "#!/bin/sh\n[ \"$1\" = --print ] && [ \"$2\" = sysroot ] || exit 10\n[ -f rust-toolchain.toml ] || exit 11\n[ \"$RUSTUP_TOOLCHAIN\" = fixture-toolchain ] || exit 12\nprintf '%s\\n' \"$FIXTURE_SYSROOT\"\n",
        );
        let environment = BTreeMap::from([
            ("RUSTC".into(), compiler.to_string_lossy().into_owned()),
            ("RUSTUP_TOOLCHAIN".into(), "fixture-toolchain".into()),
            (
                "FIXTURE_SYSROOT".into(),
                sysroot.to_string_lossy().into_owned(),
            ),
        ]);
        let result = discover(&fixture.0, &environment, &AtomicBool::new(false)).unwrap();
        assert_eq!(result.commands.len(), 2, "{:?}", result.notice);
        assert!(result.commands[0].contains("toolchain with spaces"));
    }

    #[cfg(unix)]
    #[test]
    fn probe_respects_explicit_path_and_workspace_directory() {
        let fixture = Fixture::new();
        let sysroot = fixture.printers(false);
        let compiler = fixture.compiler(
            "#!/bin/sh\n[ \"$PWD\" = \"$FIXTURE_WORKSPACE\" ] || exit 10\nprintf '%s\\n' \"$FIXTURE_SYSROOT\"\n",
        );
        let named_compiler = fixture.0.join("rustc");
        fs::rename(compiler, named_compiler).unwrap();
        let environment = BTreeMap::from([
            ("RUSTC".into(), "rustc".into()),
            ("PATH".into(), fixture.0.to_string_lossy().into_owned()),
            (
                "FIXTURE_WORKSPACE".into(),
                fixture.0.to_string_lossy().into_owned(),
            ),
            (
                "FIXTURE_SYSROOT".into(),
                sysroot.to_string_lossy().into_owned(),
            ),
        ]);
        let result = discover(&fixture.0, &environment, &AtomicBool::new(false)).unwrap();
        assert_eq!(result.commands.len(), 2, "{:?}", result.notice);
    }

    #[cfg(unix)]
    #[test]
    fn relative_sysroot_resolves_against_absolute_or_relative_workspace() {
        let fixture = Fixture::new();
        fixture.printers(false);
        let compiler = fixture.compiler("#!/bin/sh\nprintf 'toolchain with spaces\\n'\n");
        let environment =
            BTreeMap::from([("RUSTC".into(), compiler.to_string_lossy().into_owned())]);
        let mut relative_workspace = PathBuf::new();
        let current = std::env::current_dir().unwrap();
        for _ in current.ancestors().skip(1) {
            relative_workspace.push("..");
        }
        relative_workspace.push(fixture.0.strip_prefix("/").unwrap());
        for workspace in [&fixture.0, &relative_workspace] {
            let result = discover(workspace, &environment, &AtomicBool::new(false)).unwrap();
            let absolute = if workspace.is_absolute() {
                workspace.to_owned()
            } else {
                current.join(workspace)
            };
            let lookup = absolute.join("toolchain with spaces/lib/rustlib/etc/lldb_lookup.py");
            assert_eq!(
                result.commands,
                [
                    format!("command script import {}", quote_path(&lookup).unwrap()),
                    DISABLE_RECURSIVE_STRUCT_SUMMARIES.into(),
                ],
                "{:?}",
                result.notice
            );
        }
    }

    #[test]
    fn missing_compiler_is_nonfatal() {
        let fixture = Fixture::new();
        let environment = BTreeMap::from([(
            "RUSTC".into(),
            fixture
                .0
                .join("missing rustc")
                .to_string_lossy()
                .into_owned(),
        )]);
        let result = discover(&fixture.0, &environment, &AtomicBool::new(false)).unwrap();
        assert!(result.commands.is_empty());
        assert!(result.notice.is_some());
    }

    #[cfg(unix)]
    #[test]
    fn failed_or_invalid_probe_is_nonfatal() {
        let fixture = Fixture::new();
        for script in [
            "#!/bin/sh\nprintf 'toolchain unavailable' >&2\nexit 17\n",
            "#!/bin/sh\nprintf '\\377\\n'\n",
            "#!/bin/sh\nprintf '\\n'\n",
        ] {
            let compiler = fixture.compiler(script);
            let environment =
                BTreeMap::from([("RUSTC".into(), compiler.to_string_lossy().into_owned())]);
            let result = discover(&fixture.0, &environment, &AtomicBool::new(false)).unwrap();
            assert!(result.commands.is_empty());
            assert!(result.notice.is_some());
        }
    }

    #[test]
    fn cancelled_probe_does_not_run_the_compiler() {
        let fixture = Fixture::new();
        let error = discover(&fixture.0, &BTreeMap::new(), &AtomicBool::new(true)).unwrap_err();
        assert!(error.contains("cancelled"));
    }

    #[cfg(unix)]
    #[test]
    fn cancellation_terminates_a_running_compiler() {
        let fixture = Fixture::new();
        let compiler = fixture.compiler("#!/bin/sh\nprintf started > probe-started\nsleep 30\n");
        let environment =
            BTreeMap::from([("RUSTC".into(), compiler.to_string_lossy().into_owned())]);
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let workspace = fixture.0.clone();
        let worker = std::thread::spawn(move || discover(&workspace, &environment, &worker_cancel));
        let start = Instant::now();
        while !fixture.0.join("probe-started").is_file() {
            assert!(start.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(5));
        }
        cancel.store(true, Ordering::Release);
        assert!(worker.join().unwrap().unwrap_err().contains("cancelled"));
        assert!(start.elapsed() < Duration::from_secs(5));
    }
}
