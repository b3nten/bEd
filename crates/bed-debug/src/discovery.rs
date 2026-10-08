use crate::{
    process::{self, Stream},
    profile::{CargoTarget, CargoTargetKind},
};
use serde::Deserialize;
use std::{
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

/// Resolve an installed LLVM adapter without changing the host's PATH.
pub fn discover_adapter(override_path: Option<&Path>) -> io::Result<PathBuf> {
    fn executable(path: &Path) -> bool {
        let Ok(metadata) = path.metadata() else {
            return false;
        };
        if !metadata.is_file() {
            return false;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            metadata.permissions().mode() & 0o111 != 0
        }
        #[cfg(not(unix))]
        {
            true
        }
    }
    fn find(name: &Path) -> Option<PathBuf> {
        if name.is_absolute() || name.components().count() > 1 {
            return executable(name).then(|| name.to_owned());
        }
        std::env::var_os("PATH")
            .as_deref()
            .map(std::env::split_paths)
            .into_iter()
            .flatten()
            .map(|p| p.join(name))
            .find(|p| executable(p))
    }
    if let Some(path) = override_path.filter(|p| !p.as_os_str().is_empty()) {
        return find(path).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "Configured LLDB adapter is not executable: {}",
                    path.display()
                ),
            )
        });
    }
    for name in [
        "lldb-dap",
        "lldb-dap-23",
        "lldb-dap-22",
        "lldb-dap-21",
        "lldb-dap-20",
        "lldb-dap-19",
        "lldb-dap-18",
        "lldb-vscode",
    ] {
        if let Some(path) = find(Path::new(name)) {
            return Ok(path);
        }
    }
    #[cfg(target_os = "macos")]
    {
        let output = Command::new("/usr/bin/xcrun")
            .args(["--find", "lldb-dap"])
            .output()?;
        if output.status.success() {
            let path = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
            if executable(&path) {
                return Ok(path);
            }
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "LLDB DAP was not found. Install LLVM 18 or newer, or configure its executable path in Debug.",
    ))
}

#[derive(Clone, Debug)]
pub struct CargoDiscoveredTarget {
    pub package: String,
    pub package_id: String,
    pub manifest_path: PathBuf,
    pub target: CargoTarget,
    pub required_features: Vec<String>,
}
#[derive(Clone, Debug)]
pub struct CargoWorkspace {
    pub root: PathBuf,
    pub target_directory: PathBuf,
    pub targets: Vec<CargoDiscoveredTarget>,
}
#[derive(Deserialize)]
struct Metadata {
    packages: Vec<Package>,
    workspace_members: Vec<String>,
    workspace_root: PathBuf,
    target_directory: PathBuf,
}
#[derive(Deserialize)]
struct Package {
    name: String,
    id: String,
    manifest_path: PathBuf,
    targets: Vec<Target>,
}
#[derive(Deserialize)]
struct Target {
    name: String,
    kind: Vec<String>,
    #[serde(default)]
    crate_types: Vec<String>,
    #[serde(default)]
    test: bool,
    #[serde(default, rename = "required-features")]
    required_features: Vec<String>,
}

pub fn parse_metadata(bytes: &[u8]) -> Result<CargoWorkspace, String> {
    let metadata: Metadata = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    let mut targets = Vec::new();
    for package in metadata.packages {
        if !metadata.workspace_members.contains(&package.id) {
            continue;
        }
        for target in package.targets {
            let mut kinds = Vec::new();
            if target.kind.iter().any(|k| k == "bin") {
                kinds.push(CargoTargetKind::Binary);
                if target.test {
                    kinds.push(CargoTargetKind::BinaryTests);
                }
            } else if target.kind.iter().any(|k| k == "example")
                && target.crate_types.iter().any(|k| k == "bin")
            {
                kinds.push(CargoTargetKind::Example);
            } else if target.kind.iter().any(|k| k == "test") {
                kinds.push(CargoTargetKind::IntegrationTest);
            } else if target.test
                && target.kind.iter().any(|k| {
                    matches!(
                        k.as_str(),
                        "lib" | "rlib" | "dylib" | "staticlib" | "cdylib" | "proc-macro"
                    )
                })
            {
                kinds.push(CargoTargetKind::LibraryTests);
            }
            for kind in kinds {
                targets.push(CargoDiscoveredTarget {
                    package: package.name.clone(),
                    package_id: package.id.clone(),
                    manifest_path: package.manifest_path.clone(),
                    target: CargoTarget {
                        name: target.name.clone(),
                        kind,
                    },
                    required_features: target.required_features.clone(),
                });
            }
        }
    }
    targets.sort_by(|a, b| {
        (&a.package, a.target.kind.label(), &a.target.name).cmp(&(
            &b.package,
            b.target.kind.label(),
            &b.target.name,
        ))
    });
    Ok(CargoWorkspace {
        root: metadata.workspace_root,
        target_directory: metadata.target_directory,
        targets,
    })
}
pub(crate) fn discover(
    manifest: &Path,
    cancel: &AtomicBool,
    environment: &std::collections::BTreeMap<String, String>,
) -> Result<CargoWorkspace, String> {
    let mut command = Command::new("cargo");
    command
        .args([
            "metadata",
            "--no-deps",
            "--format-version",
            "1",
            "--manifest-path",
        ])
        .arg(manifest);
    if let Some(parent) = manifest.parent() {
        command.current_dir(parent);
    }
    if let Some(path) = process::child_path() {
        command.env("PATH", path);
    }
    command.envs(environment);
    let mut bytes = Vec::new();
    let mut errors = Vec::new();
    let status = process::run(command, cancel, |stream, data| {
        let target = match stream {
            Stream::Stdout => &mut bytes,
            Stream::Stderr => &mut errors,
        };
        if target.len().saturating_add(data.len()) > 16 * 1024 * 1024 {
            return Err("Cargo metadata exceeds 16 MiB".into());
        }
        target.extend_from_slice(data);
        Ok(())
    })?;
    if !status.success() {
        return Err(format!(
            "Cargo metadata failed: {}",
            String::from_utf8_lossy(&errors)
        ));
    }
    parse_metadata(&bytes)
}

pub struct CargoDiscovery {
    cancel: Arc<AtomicBool>,
    receiver: mpsc::Receiver<Result<CargoWorkspace, String>>,
    outcome: Option<Result<CargoWorkspace, String>>,
}
impl CargoDiscovery {
    pub fn start(manifest: &Path) -> io::Result<Self> {
        let manifest = if manifest.is_absolute() {
            manifest.to_owned()
        } else {
            std::env::current_dir()?.join(manifest)
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let (sender, receiver) = mpsc::channel();
        thread::Builder::new()
            .name("bed-debug-cargo-discovery".into())
            .spawn(move || {
                let result = discover(&manifest, &worker_cancel, &Default::default());
                let _ = sender.send(result);
            })?;
        Ok(Self {
            cancel,
            receiver,
            outcome: None,
        })
    }
    pub fn poll(&mut self) -> bool {
        if self.outcome.is_some() {
            return false;
        }
        match self.receiver.try_recv() {
            Ok(outcome) => {
                self.outcome = Some(outcome);
                true
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.outcome = Some(Err("Cargo discovery worker ended".into()));
                true
            }
            Err(mpsc::TryRecvError::Empty) => false,
        }
    }
    pub fn outcome(&self) -> Option<&Result<CargoWorkspace, String>> {
        self.outcome.as_ref()
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }
}
impl Drop for CargoDiscovery {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn discovery_lists_launchable_targets_and_test_modes_only() {
        let value = serde_json::json!({"workspace_members":["p"],"workspace_root":"/p","target_directory":"/out","packages":[{"name":"p","id":"p","manifest_path":"/p/Cargo.toml","targets":[{"name":"app","kind":["bin"],"test":true},{"name":"p","kind":["lib"],"test":true},{"name":"demo","kind":["example"],"crate_types":["bin"]},{"name":"helper","kind":["example"],"crate_types":["lib"]},{"name":"integration","kind":["test"]},{"name":"speed","kind":["bench"]}]}]});
        let result = parse_metadata(&serde_json::to_vec(&value).unwrap()).unwrap();
        assert_eq!(result.targets.len(), 5);
        assert_eq!(result.target_directory, PathBuf::from("/out"));
        assert!(
            result
                .targets
                .iter()
                .any(|t| t.target.kind == CargoTargetKind::LibraryTests)
        );
        assert!(
            !result
                .targets
                .iter()
                .any(|t| t.target.name == "helper" || t.target.name == "speed")
        );
    }
}
