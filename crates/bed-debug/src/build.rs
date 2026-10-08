use crate::{
    discovery,
    process::{self, Stream},
    profile::{CargoLaunch, DebugProfile},
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
};

#[derive(Clone, Debug)]
pub struct BuildArtifact {
    pub program: PathBuf,
    pub environment: BTreeMap<String, String>,
}
#[derive(Clone, Debug)]
pub enum BuildEvent {
    Output(String),
    Finished(Result<BuildArtifact, String>),
}
#[derive(Clone, Debug)]
enum BuildCommand {
    None,
    Shell(String),
    Cargo(CargoLaunch),
}
#[derive(Clone, Debug)]
pub struct BuildRequest {
    command: BuildCommand,
    workspace: PathBuf,
    env: BTreeMap<String, String>,
    program: PathBuf,
}
fn resolve(root: &Path, path: &str) -> PathBuf {
    let path = Path::new(path);
    if path.is_absolute() {
        path.to_owned()
    } else {
        root.join(path)
    }
}
impl BuildRequest {
    pub fn for_profile(profile: &DebugProfile, workspace: &Path) -> io::Result<Self> {
        let workspace = if workspace.is_absolute() {
            workspace.to_owned()
        } else {
            std::env::current_dir()?.join(workspace)
        };
        let command = if let Some(cargo) = &profile.cargo {
            BuildCommand::Cargo(cargo.clone())
        } else if !profile.build_command.trim().is_empty() {
            BuildCommand::Shell(profile.build_command.clone())
        } else {
            BuildCommand::None
        };
        if profile.cargo.is_none() && profile.program.trim().is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Select an executable to debug",
            ));
        }
        Ok(Self {
            command,
            program: resolve(&workspace, &profile.program),
            workspace,
            env: profile.env.clone(),
        })
    }
}

pub struct BuildJob {
    cancel: Arc<AtomicBool>,
    logs: mpsc::Receiver<String>,
    receiver: mpsc::Receiver<Result<BuildArtifact, String>>,
    outcome: Option<Result<BuildArtifact, String>>,
}
impl BuildJob {
    pub fn start(request: BuildRequest) -> io::Result<Self> {
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let (log_sender, logs) = mpsc::sync_channel(64);
        let (sender, receiver) = mpsc::channel();
        thread::Builder::new()
            .name("bed-debug-build".into())
            .spawn(move || {
                let result = build(request, &worker_cancel, &log_sender);
                let _ = sender.send(result);
            })?;
        Ok(Self {
            cancel,
            logs,
            receiver,
            outcome: None,
        })
    }
    pub fn poll(&mut self) -> Vec<BuildEvent> {
        let mut events: Vec<_> = self
            .logs
            .try_iter()
            .take(256)
            .map(BuildEvent::Output)
            .collect();
        if self.outcome.is_none() {
            let result = match self.receiver.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Disconnected) => {
                    Some(Err("Build worker ended unexpectedly".into()))
                }
                Err(mpsc::TryRecvError::Empty) => None,
            };
            if let Some(result) = result {
                events.extend(self.logs.try_iter().map(BuildEvent::Output));
                events.push(BuildEvent::Finished(result.clone()));
                self.outcome = Some(result);
            }
        }
        events
    }
    pub fn is_finished(&self) -> bool {
        self.outcome.is_some()
    }
    pub fn outcome(&self) -> Option<&Result<BuildArtifact, String>> {
        self.outcome.as_ref()
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }
}
impl Drop for BuildJob {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn build(
    request: BuildRequest,
    cancel: &Arc<AtomicBool>,
    logs: &mpsc::SyncSender<String>,
) -> Result<BuildArtifact, String> {
    let mut environment = request.env.clone();
    let program = match request.command {
        BuildCommand::None => request.program,
        BuildCommand::Shell(script) => {
            #[cfg(unix)]
            let mut command = {
                let mut c =
                    Command::new(std::env::var_os("SHELL").unwrap_or_else(|| "/bin/sh".into()));
                c.args(["-lc", &script]);
                c
            };
            #[cfg(windows)]
            let mut command = {
                let mut c = Command::new("cmd.exe");
                c.args(["/C", &script]);
                c
            };
            command.current_dir(&request.workspace);
            if let Some(path) = process::child_path() {
                command.env("PATH", path);
            }
            command.envs(&request.env);
            process::log(logs, format!("$ {script}\n"), cancel);
            let status = process::run(command, cancel, |_, bytes| {
                process::log(logs, String::from_utf8_lossy(bytes).into_owned(), cancel);
                Ok(())
            })?;
            if !status.success() {
                return Err(format!("Build failed ({status})"));
            }
            request.program
        }
        BuildCommand::Cargo(launch) => {
            let manifest = resolve(&request.workspace, &launch.manifest_path);
            let metadata = discovery::discover(&manifest, cancel, &request.env)?;
            let selected = metadata
                .targets
                .iter()
                .find(|t| t.package == launch.package && t.target == launch.target)
                .ok_or_else(|| {
                    "Selected Cargo package/target no longer exists; refresh targets".to_owned()
                })?;
            let mut command = Command::new("cargo");
            command.arg(if launch.target.kind.is_test() {
                "test"
            } else {
                "build"
            });
            command
                .arg("--manifest-path")
                .arg(&manifest)
                .arg("--package")
                .arg(&selected.package_id);
            if launch.target.kind.is_test() {
                command.arg("--no-run");
            }
            command.arg(match launch.target.kind.cargo_kind() {
                "bin" => "--bin",
                "example" => "--example",
                "lib" => "--lib",
                _ => "--test",
            });
            if launch.target.kind.cargo_kind() != "lib" {
                command.arg(&launch.target.name);
            }
            if !launch.features.is_empty() {
                command.arg("--features").arg(launch.features.join(","));
            }
            if !launch.default_features {
                command.arg("--no-default-features");
            }
            command.args(["--message-format=json", "--color=never"]);
            command.current_dir(&request.workspace);
            if let Some(path) = process::child_path() {
                command.env("PATH", path);
            }
            command.envs(&request.env);
            let arguments = command
                .get_args()
                .map(|a| a.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" ");
            process::log(logs, format!("$ cargo {arguments}\n"), cancel);
            let mut parser = CargoOutput::new(selected.package_id.clone(), launch);
            let status = process::run(command, cancel, |stream, bytes| {
                match stream {
                    Stream::Stderr => {
                        process::log(logs, String::from_utf8_lossy(bytes).into_owned(), cancel)
                    }
                    Stream::Stdout => {
                        for message in parser.push(bytes)? {
                            process::log(logs, message, cancel);
                        }
                    }
                }
                Ok(())
            })?;
            for message in parser.finish()? {
                process::log(logs, message, cancel);
            }
            if !status.success() || parser.build_success == Some(false) {
                return Err(format!("Cargo build failed ({status})"));
            }
            let artifact = parser.artifact()?;
            // Match Cargo's loader paths for ordinary native shared dependencies.
            // User-provided values remain explicit overrides.
            let variable = if cfg!(target_os = "macos") {
                "DYLD_FALLBACK_LIBRARY_PATH"
            } else {
                "LD_LIBRARY_PATH"
            };
            if !environment.contains_key(variable) {
                let mut paths: Vec<PathBuf> = parser
                    .library_paths
                    .into_iter()
                    .filter(|p| p.starts_with(&metadata.target_directory))
                    .collect();
                if let Some(parent) = artifact.parent() {
                    paths.push(parent.to_owned());
                    if parent
                        .file_name()
                        .is_some_and(|n| n == "deps" || n == "examples")
                    {
                        if let Some(base) = parent.parent() {
                            paths.push(base.to_owned());
                            paths.push(base.join("deps"));
                        }
                    } else {
                        paths.push(parent.join("deps"));
                    }
                }
                if let Some(path) =
                    rustc_library_directory(&request.workspace, &request.env, cancel)
                {
                    paths.push(path);
                }
                paths.extend(
                    std::env::var_os(variable)
                        .as_deref()
                        .map(std::env::split_paths)
                        .into_iter()
                        .flatten(),
                );
                if cfg!(target_os = "macos") && std::env::var_os(variable).is_none() {
                    if let Some(home) = std::env::var_os("HOME") {
                        paths.push(PathBuf::from(home).join("lib"));
                    }
                    paths.extend([PathBuf::from("/usr/local/lib"), PathBuf::from("/usr/lib")]);
                }
                if let Ok(value) = std::env::join_paths(paths) {
                    environment.insert(variable.into(), value.to_string_lossy().into_owned());
                }
            }
            artifact
        }
    };
    if cancel.load(Ordering::Acquire) {
        return Err("Build cancelled".into());
    }
    if !program.is_file() {
        return Err(format!("Executable does not exist: {}", program.display()));
    }
    Ok(BuildArtifact {
        program,
        environment,
    })
}

fn rustc_library_directory(
    workspace: &Path,
    env: &BTreeMap<String, String>,
    cancel: &AtomicBool,
) -> Option<PathBuf> {
    let compiler = env
        .get("RUSTC")
        .map(std::ffi::OsString::from)
        .or_else(|| std::env::var_os("RUSTC"))
        .unwrap_or_else(|| "rustc".into());
    let mut command = Command::new(compiler);
    command
        .args(["--print", "target-libdir"])
        .current_dir(workspace);
    if let Some(path) = process::child_path() {
        command.env("PATH", path);
    }
    command.envs(env);
    let mut bytes = Vec::new();
    let status = process::run(command, cancel, |stream, data| {
        if matches!(stream, Stream::Stdout) {
            if bytes.len().saturating_add(data.len()) > 64 * 1024 {
                return Err("rustc library path response exceeds 64 KiB".into());
            }
            bytes.extend_from_slice(data);
        }
        Ok(())
    })
    .ok()?;
    if !status.success() {
        return None;
    }
    let path = PathBuf::from(std::str::from_utf8(&bytes).ok()?.trim());
    path.is_dir().then_some(path)
}

struct CargoOutput {
    pending: Vec<u8>,
    package: String,
    launch: CargoLaunch,
    artifacts: BTreeSet<PathBuf>,
    library_paths: BTreeSet<PathBuf>,
    build_success: Option<bool>,
}
impl CargoOutput {
    fn new(package: String, launch: CargoLaunch) -> Self {
        Self {
            pending: Vec::new(),
            package,
            launch,
            artifacts: BTreeSet::new(),
            library_paths: BTreeSet::new(),
            build_success: None,
        }
    }
    fn push(&mut self, bytes: &[u8]) -> Result<Vec<String>, String> {
        if self.pending.len().saturating_add(bytes.len()) > 16 * 1024 * 1024 {
            return Err("Cargo output line exceeds 16 MiB".into());
        }
        self.pending.extend(bytes);
        let mut output = Vec::new();
        while let Some(end) = self.pending.iter().position(|b| *b == b'\n') {
            let line: Vec<_> = self.pending.drain(..=end).collect();
            if let Some(text) = self.line(&line) {
                output.push(text);
            }
        }
        Ok(output)
    }
    fn finish(&mut self) -> Result<Vec<String>, String> {
        let bytes = std::mem::take(&mut self.pending);
        Ok(if bytes.is_empty() {
            Vec::new()
        } else {
            self.line(&bytes).into_iter().collect()
        })
    }
    fn line(&mut self, bytes: &[u8]) -> Option<String> {
        let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
            return Some(String::from_utf8_lossy(bytes).into_owned());
        };
        match value["reason"].as_str().unwrap_or("") {
            "compiler-artifact" => {
                if value["package_id"].as_str() == Some(&self.package)
                    && value["target"]["name"].as_str() == Some(&self.launch.target.name)
                    && value["target"]["kind"].as_array().is_some_and(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .any(|k| self.launch.target.kind.matches_cargo_kind(k))
                    })
                    && value["profile"]["test"].as_bool().unwrap_or(false)
                        == self.launch.target.kind.is_test()
                    && let Some(executable) = value["executable"].as_str()
                {
                    self.artifacts.insert(executable.into());
                }
                None
            }
            "compiler-message" => value["message"]["rendered"].as_str().map(str::to_owned),
            "build-finished" => {
                self.build_success = value["success"].as_bool();
                None
            }
            "build-script-executed" => {
                if let Some(paths) = value["linked_paths"].as_array() {
                    for path in paths.iter().filter_map(Value::as_str) {
                        self.library_paths
                            .insert(PathBuf::from(path.split_once('=').map_or(path, |(_, p)| p)));
                    }
                }
                None
            }
            _ => None,
        }
    }
    fn artifact(&self) -> Result<PathBuf, String> {
        if self.artifacts.len() != 1 {
            return Err(format!(
                "Expected one executable for selected Cargo target, found {}",
                self.artifacts.len()
            ));
        }
        Ok(self.artifacts.first().expect("one artifact").clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{CargoTarget, CargoTargetKind};
    #[test]
    fn artifact_selection_ignores_dependencies_and_regular_binary_for_tests() {
        let launch = CargoLaunch {
            target: CargoTarget {
                name: "app".into(),
                kind: CargoTargetKind::BinaryTests,
            },
            ..Default::default()
        };
        let mut parser = CargoOutput::new("p".into(), launch);
        for (package, test, path) in [
            ("dependency", true, "/wrong-dep"),
            ("p", false, "/regular-bin"),
            ("p", true, "/custom/out/deps/app-hash"),
        ] {
            let message = serde_json::json!({"reason":"compiler-artifact","package_id":package,"target":{"name":"app","kind":["bin"]},"profile":{"test":test},"executable":path,"fresh":true});
            let mut bytes = serde_json::to_vec(&message).unwrap();
            bytes.push(b'\n');
            for chunk in bytes.chunks(3) {
                parser.push(chunk).unwrap();
            }
        }
        assert_eq!(
            parser.artifact().unwrap(),
            PathBuf::from("/custom/out/deps/app-hash")
        );
    }
    #[test]
    fn ambiguous_or_absent_artifacts_never_guess_a_binary() {
        let mut parser = CargoOutput::new("p".into(), CargoLaunch::default());
        assert!(parser.artifact().is_err());
        parser
            .artifacts
            .extend([PathBuf::from("a"), PathBuf::from("b")]);
        assert!(parser.artifact().is_err());
    }
}
