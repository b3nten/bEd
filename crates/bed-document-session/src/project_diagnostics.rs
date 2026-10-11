//! Workspace diagnostic collection and serialized disk-based checks.
use bed_lsp::{diagnostics::DiagnosticItem, lsp_config::LspConfig};
use bed_remote::{CheckJob, CheckOutput, RemoteClient, Request, Response};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectCheckConfig {
    pub program: String,
    pub arguments: Vec<String>,
    pub directory: String,
    pub format: CheckFormat,
    pub automatic: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckFormat {
    CargoJson,
    LspJsonLines,
}
impl Default for ProjectCheckConfig {
    fn default() -> Self {
        Self {
            program: "cargo".into(),
            arguments: [
                "check",
                "--workspace",
                "--all-targets",
                "--keep-going",
                "--message-format=json",
                "--color=never",
            ]
            .map(str::to_owned)
            .to_vec(),
            directory: ".".into(),
            format: CheckFormat::CargoJson,
            automatic: true,
        }
    }
}
impl ProjectCheckConfig {
    pub fn to_json(&self) -> Value {
        json!({"program":self.program,"arguments":self.arguments,"directory":self.directory,"format":if self.format == CheckFormat::CargoJson {"cargo-json"} else {"lsp-jsonl"},"automatic":self.automatic})
    }
    pub fn from_json(value: &Value) -> Option<Self> {
        Some(Self {
            program: value["program"].as_str()?.into(),
            arguments: value["arguments"]
                .as_array()?
                .iter()
                .map(|value| value.as_str().map(str::to_owned))
                .collect::<Option<Vec<_>>>()?,
            directory: value["directory"].as_str().unwrap_or(".").into(),
            format: match value["format"].as_str()? {
                "cargo-json" => CheckFormat::CargoJson,
                "lsp-jsonl" => CheckFormat::LspJsonLines,
                _ => return None,
            },
            automatic: value["automatic"].as_bool().unwrap_or(true),
        })
    }
}
#[derive(Clone, Debug, Default)]
pub struct ProjectDiagnosticsSnapshot {
    pub by_path: BTreeMap<String, Vec<DiagnosticItem>>,
    pub checking: bool,
    pub complete: bool,
    pub stale: bool,
    pub status: String,
    pub errors: usize,
    pub warnings: usize,
}
type ParsedCheck = (BTreeMap<String, Vec<DiagnosticItem>>, bool);

fn is_non_rust_source(path: &str) -> bool {
    static CATALOG: OnceLock<LspConfig> = OnceLock::new();
    let catalog = CATALOG.get_or_init(|| {
        LspConfig::from_layers(None, None).expect("Bundled language catalog must be valid")
    });
    matches!(
        catalog.detect_language(path).as_str(),
        "c" | "cpp"
            | "javascript"
            | "jsx"
            | "typescript"
            | "tsx"
            | "python"
            | "go"
            | "java"
            | "csharp"
            | "lua"
            | "bash"
            | "ruby"
            | "php"
            | "swift"
            | "elixir"
            | "erlang"
            | "ocaml"
            | "clojure"
            | "dart"
    )
}
struct Worker {
    canceled: Arc<AtomicBool>,
    result: mpsc::Receiver<Result<ParsedCheck, String>>,
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.canceled.store(true, Ordering::Release);
    }
}
struct DiscoveredFiles {
    has_cargo: bool,
    has_non_rust_source: bool,
    paths: Vec<String>,
    complete: bool,
}
struct Discovery {
    result: mpsc::Receiver<Result<DiscoveredFiles, String>>,
}
#[derive(Default)]
pub(crate) struct ProjectDiagnostics {
    root: Option<PathBuf>,
    remote: Option<RemoteClient>,
    pub config: ProjectCheckConfig,
    configured: bool,
    available: bool,
    check_covers_project: bool,
    discovery_complete: bool,
    discovery: Option<Discovery>,
    discovered_paths: Option<Vec<String>>,
    worker: Option<Worker>,
    pending: Option<Instant>,
    diagnostics: BTreeMap<String, Vec<DiagnosticItem>>,
    complete: bool,
    stale: bool,
    status: String,
}
impl ProjectDiagnostics {
    pub fn bind(&mut self, root: Option<PathBuf>, remote: Option<RemoteClient>) {
        let same_remote = match (&self.remote, &remote) {
            (None, None) => true,
            (Some(a), Some(b)) => a.same_connection(b),
            _ => false,
        };
        if root == self.root && same_remote {
            return;
        }
        self.worker = None;
        self.root = root.clone();
        self.remote = remote.clone();
        self.pending = None;
        self.diagnostics.clear();
        self.complete = false;
        self.stale = false;
        self.available = false;
        self.discovery_complete = false;
        self.discovered_paths = None;
        self.status = if root.is_some() {
            "Discovering project checks and languages…".into()
        } else {
            "Open a project to collect diagnostics".into()
        };
        self.discovery = root.map(|root| {
            let (sender, result) = mpsc::sync_channel(1);
            thread::spawn(move || {
                let discovery = if let Some(client) = remote {
                    match client.call(Request::ListFiles {
                        root: root.to_string_lossy().into_owned(),
                    }) {
                        Ok(Response::Files { paths }) => Ok(DiscoveredFiles {
                            has_cargo: paths.iter().any(|path| {
                                Path::new(path) == root.join("Cargo.toml") || path == "Cargo.toml"
                            }),
                            has_non_rust_source: paths.iter().any(|path| is_non_rust_source(path)),
                            paths,
                            complete: true,
                        }),
                        Ok(_) => Err("Unexpected project discovery response".into()),
                        Err(error) => Err(error.to_string()),
                    }
                } else {
                    let (paths, complete) = discover_files(&root);
                    Ok(DiscoveredFiles {
                        has_cargo: root.join("Cargo.toml").is_file(),
                        has_non_rust_source: paths.iter().any(|path| is_non_rust_source(path)),
                        paths,
                        complete,
                    })
                };
                let _ = sender.send(discovery);
            });
            Discovery { result }
        });
    }
    pub fn configure(&mut self, config: ProjectCheckConfig, explicit: bool) {
        if self.config == config && self.configured == explicit {
            return;
        }
        self.worker = None;
        self.config = config;
        self.configured = explicit;
        self.complete = false;
        self.stale = true;
        if self.config.automatic {
            self.request();
        } else {
            self.pending = None;
        }
    }
    pub fn request(&mut self) {
        if self.root.is_some() {
            self.pending = Some(Instant::now());
            self.stale = true;
        }
    }
    pub fn saved(&mut self) {
        self.stale = true;
        if self.config.automatic && (self.available || self.configured) {
            self.pending = Some(Instant::now() + Duration::from_secs(1));
        }
    }
    pub fn edited(&mut self) {
        self.stale = true;
    }
    pub fn clear_path(&mut self, path: &str) {
        self.diagnostics
            .retain(|candidate, _| !Path::new(candidate).starts_with(path));
    }
    pub fn note_file(&mut self, path: &str) {
        if is_non_rust_source(path) {
            self.check_covers_project = false;
        }
    }
    pub fn relevant_file(&self, path: &str) -> bool {
        let path = Path::new(path);
        let relative = self
            .root
            .as_ref()
            .and_then(|root| path.strip_prefix(root).ok())
            .unwrap_or(path);
        if relative.components().any(|part| {
            matches!(
                part.as_os_str().to_str(),
                Some(".git" | "target" | "node_modules" | ".venv" | "vendor")
            )
        }) {
            return false;
        }
        self.configured
            || matches!(
                path.file_name().and_then(|part| part.to_str()),
                Some("Cargo.toml" | "Cargo.lock")
            )
            || path.extension().and_then(|part| part.to_str()) == Some("rs")
            || is_non_rust_source(&path.to_string_lossy())
    }
    pub fn cancel(&mut self) {
        self.worker = None;
        self.pending = None;
        self.stale = true;
        self.status = "Project check canceled; previous results are stale".into();
    }
    pub fn owns_cargo_check(&self) -> bool {
        self.available
            && self.config.automatic
            && self.config.program == "cargo"
            && self.config.format == CheckFormat::CargoJson
    }
    pub fn take_discovered_paths(&mut self) -> Option<Vec<String>> {
        self.discovered_paths.take()
    }
    pub fn tick(&mut self) {
        if let Some(discovery) = &self.discovery
            && let Ok(result) = discovery.result.try_recv()
        {
            self.discovery = None;
            match result {
                Ok(DiscoveredFiles {
                    has_cargo: cargo,
                    has_non_rust_source,
                    paths,
                    complete,
                }) => {
                    self.discovery_complete = complete;
                    self.available = cargo;
                    self.check_covers_project = complete && !has_non_rust_source;
                    self.discovered_paths = Some(paths);
                    self.status = if cargo || self.configured {
                        "Project check ready".into()
                    } else {
                        "Language-server diagnostics; project check is not configured".into()
                    };
                    if self.config.automatic && (cargo || self.configured) {
                        self.request();
                    }
                }
                Err(error) => self.status = format!("Project discovery failed: {error}"),
            }
        }
        if let Some(worker) = &self.worker
            && let Ok(result) = worker.result.try_recv()
        {
            self.worker = None;
            match result {
                Ok((diagnostics, successful)) => {
                    self.diagnostics = diagnostics;
                    self.complete = true;
                    self.stale = self.pending.is_some();
                    self.status = if successful {
                        "Project check complete".into()
                    } else {
                        "Project check found errors".into()
                    };
                }
                Err(error) => {
                    self.stale = true;
                    self.status = format!("Project check failed: {error}");
                }
            }
        }
        if self.worker.is_none() && self.pending.is_some_and(|due| Instant::now() >= due) {
            self.pending = None;
            let Some(root) = self.root.clone() else {
                return;
            };
            self.status = "Checking project…".into();
            let config = self.config.clone();
            let remote = self.remote.clone();
            let canceled = Arc::new(AtomicBool::new(false));
            let stop = canceled.clone();
            let (sender, result) = mpsc::sync_channel(1);
            thread::spawn(move || {
                let directory = root.join(&config.directory);
                let format = config.format;
                let remote_paths = remote.is_some();
                let output = if let Some(client) = remote {
                    run_remote(&client, root, &config, stop)
                } else {
                    let mut job =
                        CheckJob::start(root, config.program, config.arguments, config.directory);
                    loop {
                        if stop.load(Ordering::Acquire) {
                            job.cancel();
                        }
                        if let Some(result) = job.try_result() {
                            break result;
                        }
                        thread::sleep(Duration::from_millis(20));
                    }
                };
                let _ = sender
                    .send(output.and_then(|output| {
                        parse_output(&directory, format, &output, remote_paths)
                    }));
            });
            self.worker = Some(Worker { canceled, result });
        }
    }
    pub fn snapshot(
        &self,
        lsp: BTreeMap<String, Vec<DiagnosticItem>>,
        unsaved: bool,
        lsp_complete: bool,
    ) -> ProjectDiagnosticsSnapshot {
        let mut result = ProjectDiagnosticsSnapshot {
            by_path: lsp,
            checking: self.worker.is_some() || self.pending.is_some() || self.discovery.is_some(),
            complete: (self.complete
                && (self.check_covers_project
                    || (self.configured && self.config.format == CheckFormat::LspJsonLines)))
                || (lsp_complete && self.discovery_complete),
            stale: unsaved || (self.stale && (self.available || self.configured || !lsp_complete)),
            status: self.status.clone(),
            ..Default::default()
        };
        for (path, items) in &self.diagnostics {
            let entries = result.by_path.entry(path.clone()).or_default();
            for item in items {
                if !entries.iter().any(|old| {
                    old.start_line == item.start_line
                        && old.start_character == item.start_character
                        && old.severity == item.severity
                        && old.message == item.message
                }) {
                    entries.push(item.clone());
                }
            }
        }
        result.by_path.retain(|_, items| !items.is_empty());
        for item in result.by_path.values().flatten() {
            match item.severity {
                1 => result.errors += 1,
                2 => result.warnings += 1,
                _ => {}
            }
        }
        result
    }
}
fn discover_files(root: &Path) -> (Vec<String>, bool) {
    let mut complete = true;
    let mut paths = Vec::new();
    let mut directories = vec![root.to_owned()];
    while let Some(directory) = directories.pop() {
        let Ok(entries) = std::fs::read_dir(directory) else {
            complete = false;
            continue;
        };
        for entry in entries {
            let Ok(entry) = entry else {
                complete = false;
                continue;
            };
            if paths.len() >= 50_000 {
                return (paths, false);
            }
            let Ok(kind) = entry.file_type() else {
                complete = false;
                continue;
            };
            if kind.is_dir()
                && !matches!(
                    entry.file_name().to_str(),
                    Some(".git" | "target" | "node_modules" | ".venv" | "vendor")
                )
            {
                directories.push(entry.path());
            } else if kind.is_file() {
                paths.push(entry.path().to_string_lossy().into_owned());
            }
        }
    }
    (paths, complete)
}
fn run_remote(
    client: &RemoteClient,
    root: PathBuf,
    config: &ProjectCheckConfig,
    stop: Arc<AtomicBool>,
) -> Result<CheckOutput, String> {
    let Response::CheckStarted { job_id } = client
        .call(Request::CheckStart {
            root: root.to_string_lossy().into_owned(),
            program: config.program.clone(),
            arguments: config.arguments.clone(),
            directory: config.directory.clone(),
        })
        .map_err(|e| e.to_string())?
    else {
        return Err("Unexpected check start response".into());
    };
    let result = loop {
        if stop.load(Ordering::Acquire) {
            let _ = client.call(Request::CheckCancel { job_id });
            break Err("Project check canceled".into());
        }
        match client.call(Request::CheckPoll { job_id }) {
            Ok(Response::CheckJob {
                result: Some(result),
            }) => break result,
            Ok(Response::CheckJob { result: None }) => thread::sleep(Duration::from_millis(100)),
            Ok(_) => break Err("Unexpected check poll response".into()),
            Err(error) => break Err(error.to_string()),
        }
    };
    let _ = client.call(Request::CheckRelease { job_id });
    result
}
fn parse_output(
    root: &Path,
    format: CheckFormat,
    output: &CheckOutput,
    remote_paths: bool,
) -> Result<ParsedCheck, String> {
    let mut diagnostics = BTreeMap::<String, Vec<DiagnosticItem>>::new();
    let mut build_finished = false;
    for line in output
        .stdout
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        let value: Value = match serde_json::from_slice(line) {
            Ok(value) => value,
            Err(error) if format == CheckFormat::LspJsonLines => {
                return Err(format!("Invalid diagnostic JSON: {error}"));
            }
            Err(_) => continue,
        };
        match format {
            CheckFormat::CargoJson => {
                if value["reason"] == "build-finished" {
                    build_finished = true;
                }
                if value["reason"] != "compiler-message" {
                    continue;
                }
                let message = &value["message"];
                let severity = match message["level"].as_str() {
                    Some("error" | "failure-note") => 1,
                    Some("warning") => 2,
                    _ => continue,
                };
                let Some(text) = message["message"].as_str() else {
                    continue;
                };
                let spans: Vec<_> = message["spans"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|span| span["is_primary"].as_bool() == Some(true))
                    .collect();
                if spans.is_empty() {
                    diagnostics
                        .entry(diagnostic_path(root.join("Cargo.toml"), remote_paths))
                        .or_default()
                        .push(DiagnosticItem {
                            severity,
                            message: text.into(),
                            source: "cargo".into(),
                            ..Default::default()
                        });
                }
                for span in spans {
                    let Some(file) = span["file_name"].as_str() else {
                        continue;
                    };
                    let path = diagnostic_path(root.join(file), remote_paths);
                    let index = |name: &str| {
                        span[name]
                            .as_i64()
                            .unwrap_or(1)
                            .saturating_sub(1)
                            .clamp(0, i32::MAX as i64) as i32
                    };
                    diagnostics.entry(path).or_default().push(DiagnosticItem {
                        start_line: index("line_start"),
                        start_character: index("column_start"),
                        end_line: index("line_end"),
                        end_character: index("column_end"),
                        severity,
                        message: text.into(),
                        source: "cargo".into(),
                    });
                }
            }
            CheckFormat::LspJsonLines => {
                let params = value.get("params").unwrap_or(&value);
                let uri = params["uri"]
                    .as_str()
                    .ok_or("Diagnostic notification lacks URI")?;
                let path = bed_lsp::lsp_uri::LspUri::parse(uri)
                    .map_err(|error| error.to_string())?
                    .fs_path();
                let path = diagnostic_path(PathBuf::from(path), remote_paths);
                let items = params["diagnostics"]
                    .as_array()
                    .ok_or("Diagnostic notification lacks diagnostics")?;
                let mut entries = Vec::new();
                for item in items {
                    let coordinate = |side: &str, axis: &str| {
                        item["range"][side][axis]
                            .as_i64()
                            .filter(|n| *n >= 0 && *n <= i32::MAX as i64)
                            .map(|n| n as i32)
                            .ok_or("Invalid diagnostic range")
                    };
                    entries.push(DiagnosticItem {
                        start_line: coordinate("start", "line")?,
                        start_character: coordinate("start", "character")?,
                        end_line: coordinate("end", "line")?,
                        end_character: coordinate("end", "character")?,
                        severity: item["severity"].as_i64().unwrap_or(1).clamp(1, 4) as i32,
                        message: item["message"]
                            .as_str()
                            .ok_or("Diagnostic lacks message")?
                            .into(),
                        source: item["source"].as_str().unwrap_or("project check").into(),
                    });
                }
                diagnostics.insert(path, entries);
            }
        }
    }
    let successful = output.exit_code == Some(0);
    if !successful && diagnostics.is_empty() {
        return Err(String::from_utf8_lossy(&output.stderr)
            .chars()
            .take(4096)
            .collect::<String>());
    }
    if format == CheckFormat::CargoJson && !build_finished {
        return Err("Cargo did not finish reporting diagnostics".into());
    }
    Ok((diagnostics, successful))
}

fn diagnostic_path(path: PathBuf, remote: bool) -> String {
    if !remote {
        return bed_editing::util::doc_path::normalize(&path.to_string_lossy());
    }
    // Remove redundant dots without consulting this computer's filesystem or
    // resolving a target-side symlink / parent component.
    path.components()
        .filter(|component| !matches!(component, std::path::Component::CurDir))
        .collect::<PathBuf>()
        .to_string_lossy()
        .into_owned()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cargo_reports_closed_files_and_success_clears_previous_errors() {
        let root = Path::new("/project");
        let output = CheckOutput { exit_code:Some(1),stderr:vec![],stdout:format!("{}\n{}\n",json!({"reason":"compiler-message","message":{"level":"error","message":"unknown name","spans":[{"file_name":"src/closed.rs","line_start":3,"line_end":3,"column_start":2,"column_end":6,"is_primary":true}]}}),json!({"reason":"build-finished","success":false})).into_bytes() };
        let (items, success) = parse_output(root, CheckFormat::CargoJson, &output, false).unwrap();
        assert!(!success);
        assert_eq!(items["/project/src/closed.rs"][0].start_line, 2);
        let output = CheckOutput {
            exit_code: Some(0),
            stderr: vec![],
            stdout: b"{\"reason\":\"build-finished\",\"success\":true}\n".to_vec(),
        };
        assert!(
            parse_output(root, CheckFormat::CargoJson, &output, false)
                .unwrap()
                .0
                .is_empty()
        );
    }
    #[test]
    fn jsonl_requires_valid_ranges_and_latest_file_report_replaces() {
        let output = CheckOutput {
            exit_code: Some(0),
            stderr: vec![],
            stdout: b"{\"uri\":\"file:///project/closed.py\",\"diagnostics\":[]}\n".to_vec(),
        };
        assert!(
            parse_output(
                Path::new("/project"),
                CheckFormat::LspJsonLines,
                &output,
                false
            )
            .unwrap()
            .0["/project/closed.py"]
                .is_empty()
        );
        let output = CheckOutput {
            stdout: b"{\"uri\":\"file:///project/a.py\",\"diagnostics\":[{\"message\":\"bad\"}]}\n"
                .to_vec(),
            ..output
        };
        assert!(
            parse_output(
                Path::new("/project"),
                CheckFormat::LspJsonLines,
                &output,
                false
            )
            .is_err()
        );
    }
    #[test]
    fn incomplete_discovery_does_not_claim_full_language_server_coverage() {
        let service = ProjectDiagnostics {
            discovery_complete: false,
            ..Default::default()
        };
        assert!(!service.snapshot(BTreeMap::new(), false, true).complete);
    }
    #[test]
    fn saved_cargo_configuration_does_not_expand_language_coverage() {
        let service = ProjectDiagnostics {
            complete: true,
            configured: true,
            check_covers_project: false,
            ..Default::default()
        };
        assert!(!service.snapshot(BTreeMap::new(), false, false).complete);
    }
    #[test]
    fn coverage_and_freshness_do_not_claim_unknown_project_is_clear() {
        let service = ProjectDiagnostics::default();
        assert!(!service.snapshot(BTreeMap::new(), false, false).complete);
        let mut service = service;
        service.complete = true;
        assert!(service.snapshot(BTreeMap::new(), true, false).stale);
    }
}
