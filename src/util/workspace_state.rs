//! Standalone recent projects and layout persistence. Embedding owns its layout.
use serde_json::{Value, json};
use std::io;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

/// Where a workspace's files and project services live.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkspaceTarget {
    Local,
    Ssh { host: String },
}

/// A named, single-project workspace. Remote roots are agent-native paths.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceSpec {
    pub name: String,
    pub target: WorkspaceTarget,
    pub root: String,
}
impl WorkspaceSpec {
    pub fn local(root: impl Into<String>) -> Self {
        let root = root.into();
        Self {
            name: default_name(&root),
            target: WorkspaceTarget::Local,
            root,
        }
    }
    /// A stable, unambiguous key; names can change without losing layout.
    pub fn identity(&self) -> String {
        match &self.target {
            WorkspaceTarget::Local => json!(["local", self.root]).to_string(),
            WorkspaceTarget::Ssh { host, .. } => json!(["ssh", host, self.root]).to_string(),
        }
    }
    pub fn location(&self) -> String {
        match &self.target {
            WorkspaceTarget::Local => self.root.clone(),
            WorkspaceTarget::Ssh { host, .. } => format!("{host}:{}", self.root),
        }
    }
    fn to_value(&self) -> Value {
        let target = match &self.target {
            WorkspaceTarget::Local => json!({"kind":"local"}),
            WorkspaceTarget::Ssh { host } => {
                json!({"kind":"ssh", "host":host})
            }
        };
        json!({"name":self.name,"target":target,"root":self.root})
    }
    fn from_value(value: &Value) -> Option<Self> {
        let root = value.get("root")?.as_str()?.to_owned();
        let target = match value.get("target")?.get("kind")?.as_str()? {
            "local" => WorkspaceTarget::Local,
            "ssh" => WorkspaceTarget::Ssh {
                host: value["target"]["host"].as_str()?.to_owned(),
            },
            _ => return None,
        };
        Some(Self {
            name: value.get("name")?.as_str()?.to_owned(),
            target,
            root,
        })
    }
}
fn default_name(root: &str) -> String {
    root.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(root)
        .to_owned()
}

pub struct WorkspaceStore {
    path: PathBuf,
    state: Value,
}
impl WorkspaceStore {
    pub fn load(config_dir: &Path) -> io::Result<Self> {
        let path = config_dir.join("workspaces.json");
        let mut state: Value = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                json!({"version":2,"recent":[],"workspaces":{}})
            }
            Err(e) => return Err(e),
        };
        if !state.is_object() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Workspace state must be an object",
            ));
        }
        match state.get("version").and_then(Value::as_u64).unwrap_or(1) {
            1 => state = migrate_v1(&state),
            2 => {}
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Unsupported workspace state version",
                ));
            }
        }
        if !state["recent"].is_array() || !state["workspaces"].is_object() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid workspace state",
            ));
        }
        Ok(Self { path, state })
    }
    pub fn recent_workspaces(&self) -> Vec<WorkspaceSpec> {
        self.state["recent"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter_map(|id| WorkspaceSpec::from_value(&self.state["workspaces"][id]["spec"]))
            .collect()
    }
    /// The caller validates SSH projects with the agent before recording them.
    /// In particular, remote roots must never be canonicalized on this machine.
    pub fn record_workspace(&mut self, mut spec: WorkspaceSpec) -> io::Result<WorkspaceSpec> {
        match &mut spec.target {
            WorkspaceTarget::Local => {
                let root = fs::canonicalize(&spec.root)?;
                if !root.is_dir() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "Project must be a directory",
                    ));
                }
                spec.root = root
                    .to_str()
                    .ok_or_else(|| {
                        io::Error::new(io::ErrorKind::InvalidInput, "Project path is not UTF-8")
                    })?
                    .to_owned();
            }
            WorkspaceTarget::Ssh { host } => {
                if host.trim().is_empty() || !spec.root.starts_with('/') {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "SSH workspace requires a host and absolute project path",
                    ));
                }
            }
        }
        spec.name = spec.name.trim().to_owned();
        if spec.name.is_empty() {
            spec.name = default_name(&spec.root);
        }
        let id = spec.identity();
        let mut recent = self.state["recent"].as_array().cloned().unwrap_or_default();
        recent.retain(|entry| entry.as_str() != Some(&id));
        recent.insert(0, Value::String(id.clone()));
        recent.truncate(20);
        self.state["recent"] = Value::Array(recent);
        self.ensure_record(&spec);
        self.state["workspaces"][&id]["spec"] = spec.to_value();
        self.save()?;
        Ok(spec)
    }
    pub fn forget_workspace(&mut self, spec: &WorkspaceSpec) -> io::Result<()> {
        let id = spec.identity();
        if let Some(recent) = self.state["recent"].as_array_mut() {
            recent.retain(|entry| entry.as_str() != Some(&id));
        }
        self.save()
    }
    pub fn rename_workspace(
        &mut self,
        spec: &WorkspaceSpec,
        name: &str,
    ) -> io::Result<WorkspaceSpec> {
        let name = name.trim();
        if name.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Workspace name cannot be empty",
            ));
        }
        let mut renamed = self.stored_spec(spec).unwrap_or_else(|| spec.clone());
        renamed.name = name.to_owned();
        self.ensure_record(&renamed);
        self.state["workspaces"][renamed.identity()]["spec"] = renamed.to_value();
        self.save()?;
        Ok(renamed)
    }
    pub fn tree_preferences(
        &self,
        spec: &WorkspaceSpec,
    ) -> crate::files::file_tree::FileTreePreferences {
        crate::files::file_tree::FileTreePreferences::from_value(
            &self.state["workspaces"][spec.identity()]["file_tree"],
        )
    }
    pub fn set_tree_preferences(
        &mut self,
        spec: &WorkspaceSpec,
        preferences: &crate::files::file_tree::FileTreePreferences,
    ) -> io::Result<()> {
        self.ensure_record(spec);
        self.state["workspaces"][spec.identity()]["file_tree"] = preferences.to_value();
        self.save()
    }
    pub fn layout(&self, spec: &WorkspaceSpec) -> Option<&Value> {
        self.state
            .get("workspaces")?
            .get(spec.identity())?
            .get("layout")
    }
    pub fn set_layout(&mut self, spec: &WorkspaceSpec, layout: Value) -> io::Result<()> {
        self.ensure_record(spec);
        self.state["workspaces"][spec.identity()]["layout"] = layout;
        self.save()
    }
    pub(crate) fn stored_spec(&self, spec: &WorkspaceSpec) -> Option<WorkspaceSpec> {
        WorkspaceSpec::from_value(&self.state["workspaces"][spec.identity()]["spec"])
    }
    fn ensure_record(&mut self, spec: &WorkspaceSpec) {
        let id = spec.identity();
        if !self.state["workspaces"][&id].is_object() {
            self.state["workspaces"][&id] = json!({"spec":spec.to_value()});
        }
    }
    // Legacy local-project helpers remain for standalone and embedding callers.
    pub fn recent_projects(&self) -> Vec<PathBuf> {
        self.recent_workspaces()
            .into_iter()
            .filter_map(|spec| {
                matches!(spec.target, WorkspaceTarget::Local).then(|| PathBuf::from(spec.root))
            })
            .collect()
    }
    pub fn record_project(&mut self, root: &Path) -> io::Result<PathBuf> {
        let root = fs::canonicalize(root)?;
        let root_text = root.to_str().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "Project path is not UTF-8")
        })?;
        let spec = WorkspaceSpec::local(root_text);
        let spec = self.stored_spec(&spec).unwrap_or(spec);
        self.record_workspace(spec)?;
        Ok(root)
    }
    pub fn forget_project(&mut self, root: &Path) -> io::Result<()> {
        let root = root.to_str().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "Project path is not UTF-8")
        })?;
        self.forget_workspace(&WorkspaceSpec::local(root))
    }
    pub fn workspace(&self, root: &Path) -> Option<&Value> {
        self.layout(&WorkspaceSpec::local(root.to_str()?))
    }
    pub fn set_workspace(&mut self, root: &Path, workspace: Value) -> io::Result<()> {
        let root = root.to_str().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "Project path is not UTF-8")
        })?;
        self.set_layout(&WorkspaceSpec::local(root), workspace)
    }
    fn save(&self) -> io::Result<()> {
        write_atomic(&self.path, &self.state)
    }
}
fn migrate_v1(old: &Value) -> Value {
    let mut state = json!({"version":2,"recent":[],"workspaces":{}});
    if let Some(projects) = old.get("projects").and_then(Value::as_object) {
        for (root, layout) in projects {
            let spec = WorkspaceSpec::local(root);
            state["workspaces"][spec.identity()] = json!({"spec":spec.to_value(),"layout":layout});
        }
    }
    if let Some(recent) = old.get("recent").and_then(Value::as_array) {
        for root in recent.iter().filter_map(Value::as_str) {
            let spec = WorkspaceSpec::local(root);
            let id = spec.identity();
            if !state["workspaces"][&id].is_object() {
                state["workspaces"][&id] = json!({"spec":spec.to_value()});
            }
            let recent = state["recent"].as_array_mut().unwrap();
            if !recent.iter().any(|entry| entry.as_str() == Some(&id)) {
                recent.push(Value::String(id));
            }
        }
    }
    state
}
pub fn write_atomic(path: &Path, value: &Value) -> io::Result<()> {
    static NEXT_WRITE: AtomicU64 = AtomicU64::new(1);
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "Missing configuration directory",
        )
    })?;
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(
        ".bed-state-{}-{}.tmp",
        std::process::id(),
        NEXT_WRITE.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(&serde_json::to_vec_pretty(value)?)?;
        file.sync_all()?;
        drop(file);
        #[cfg(windows)]
        if path.exists() {
            use std::os::windows::ffi::OsStrExt;
            use windows_sys::Win32::Storage::FileSystem::{
                MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
            };
            let from: Vec<u16> = temporary.as_os_str().encode_wide().chain(Some(0)).collect();
            let to: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            // SAFETY: Buffers are nul-terminated and live throughout this call.
            if unsafe {
                MoveFileExW(
                    from.as_ptr(),
                    to.as_ptr(),
                    MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
                )
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
            return Ok(());
        }
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::files::test_support::TempDir;
    #[test]
    fn tree_preferences_roundtrip_without_changing_layout_or_other_workspaces() {
        use crate::files::file_tree::FileTreePreferences;
        let temp = TempDir::new();
        let mut store = WorkspaceStore::load(&temp.path("config")).unwrap();
        let local = WorkspaceSpec::local("/same/root");
        let remote = WorkspaceSpec {
            name: "remote".into(),
            root: local.root.clone(),
            target: WorkspaceTarget::Ssh {
                host: "host".into(),
            },
        };
        assert_eq!(
            store.tree_preferences(&local),
            FileTreePreferences::default()
        );
        store
            .set_layout(&local, json!({"existing":"layout"}))
            .unwrap();
        let preferences = FileTreePreferences {
            hide_gitignored: true,
            hide_hidden: true,
            hidden_paths: ["build".into(), "src/private.rs".into()].into(),
        };
        store.set_tree_preferences(&local, &preferences).unwrap();
        let mut store = WorkspaceStore::load(&temp.path("config")).unwrap();
        assert_eq!(store.tree_preferences(&local), preferences);
        assert_eq!(
            store.tree_preferences(&remote),
            FileTreePreferences::default()
        );
        assert_eq!(store.layout(&local).unwrap()["existing"], "layout");
        store
            .set_layout(&local, json!({"updated":"layout"}))
            .unwrap();
        store
            .set_tree_preferences(
                &remote,
                &FileTreePreferences {
                    hide_hidden: true,
                    ..Default::default()
                },
            )
            .unwrap();
        let store = WorkspaceStore::load(&temp.path("config")).unwrap();
        assert_eq!(store.tree_preferences(&local), preferences);
        assert!(!store.tree_preferences(&remote).hide_gitignored);
        assert!(store.tree_preferences(&remote).hide_hidden);
    }
    #[test]
    fn canonical_recents_deduplicate_and_workspace_round_trips() {
        let temp = TempDir::new();
        fs::create_dir(temp.path("project")).unwrap();
        let mut store = WorkspaceStore::load(&temp.path("config")).unwrap();
        let root = store.record_project(&temp.path("project/.")).unwrap();
        store.record_project(&root).unwrap();
        assert_eq!(store.recent_projects(), vec![root.clone()]);
        store
            .set_workspace(
                &root,
                json!({"layout":"dock","views":[{"path":"main.rs","scroll":[1,2]}]}),
            )
            .unwrap();
        let loaded = WorkspaceStore::load(&temp.path("config")).unwrap();
        assert_eq!(loaded.workspace(&root).unwrap()["layout"], "dock");
        store.forget_project(&root).unwrap();
        assert!(store.recent_projects().is_empty());
        assert!(store.workspace(&root).is_some());
    }
    #[test]
    fn failed_project_open_does_not_create_history() {
        let temp = TempDir::new();
        let mut store = WorkspaceStore::load(&temp.path("config")).unwrap();
        assert!(store.record_project(&temp.path("missing")).is_err());
        assert!(!temp.path("config/workspaces.json").exists());
    }
    fn remote(host: &str, root: &str) -> WorkspaceSpec {
        WorkspaceSpec {
            name: "Remote project".into(),
            target: WorkspaceTarget::Ssh { host: host.into() },
            root: root.into(),
        }
    }
    #[test]
    fn v1_projects_migrate_in_order_with_missing_and_unlisted_layouts() {
        let temp = TempDir::new();
        let config = temp.path("config");
        fs::create_dir_all(&config).unwrap();
        let old = json!({
            "version":1,
            "recent":["/missing/second", "/missing/first", "/missing/second"],
            "projects":{
                "/missing/first":{"dock":"first"},
                "/missing/second":{"dock":"second"},
                "/missing/unlisted":{"dock":"unlisted"}
            }
        });
        fs::write(
            config.join("workspaces.json"),
            serde_json::to_vec(&old).unwrap(),
        )
        .unwrap();
        let mut store = WorkspaceStore::load(&config).unwrap();
        assert_eq!(
            store.recent_projects(),
            vec![
                PathBuf::from("/missing/second"),
                PathBuf::from("/missing/first")
            ]
        );
        assert_eq!(store.recent_workspaces()[0].name, "second");
        assert_eq!(
            store.workspace(Path::new("/missing/unlisted")).unwrap()["dock"],
            "unlisted"
        );
        let renamed = store
            .rename_workspace(&WorkspaceSpec::local("/missing/first"), "Named project")
            .unwrap();
        assert_eq!(store.layout(&renamed).unwrap()["dock"], "first");
        let persisted: Value =
            serde_json::from_slice(&fs::read(config.join("workspaces.json")).unwrap()).unwrap();
        assert_eq!(persisted["version"], 2);
        assert!(persisted.get("projects").is_none());
        let loaded = WorkspaceStore::load(&config).unwrap();
        assert_eq!(loaded.recent_workspaces()[1].name, "Named project");
    }
    #[test]
    fn remote_hosts_and_local_target_keep_layouts_separate() {
        let temp = TempDir::new();
        fs::create_dir_all(temp.path("project")).unwrap();
        let root = fs::canonicalize(temp.path("project")).unwrap();
        let root = root.to_str().unwrap();
        let mut store = WorkspaceStore::load(&temp.path("config")).unwrap();
        let local = store.record_workspace(WorkspaceSpec::local(root)).unwrap();
        let remote_root = if cfg!(unix) { root } else { "/project" };
        let first = store
            .record_workspace(remote("first-host", remote_root))
            .unwrap();
        let second = store
            .record_workspace(remote("second-host", remote_root))
            .unwrap();
        for (spec, label) in [(&local, "local"), (&first, "first"), (&second, "second")] {
            store.set_layout(spec, json!({"dock":label})).unwrap();
        }
        let loaded = WorkspaceStore::load(&temp.path("config")).unwrap();
        assert_eq!(loaded.recent_workspaces().len(), 3);
        assert_eq!(loaded.recent_projects().len(), 1);
        assert_eq!(loaded.layout(&local).unwrap()["dock"], "local");
        assert_eq!(loaded.layout(&first).unwrap()["dock"], "first");
        assert_eq!(loaded.layout(&second).unwrap()["dock"], "second");
    }
    #[test]
    fn remote_paths_are_preserved_without_accessing_local_filesystem() {
        let temp = TempDir::new();
        let mut store = WorkspaceStore::load(&temp.path("config")).unwrap();
        let spec = remote("production", "/no-such-local-project/../a project");
        let recorded = store.record_workspace(spec.clone()).unwrap();
        assert_eq!(recorded, spec);
        store
            .set_layout(&spec, json!({"tabs":["main.rs"]}))
            .unwrap();
        let renamed = store.rename_workspace(&spec, "SSH project").unwrap();
        assert_eq!(store.layout(&renamed).unwrap()["tabs"][0], "main.rs");
        store.forget_workspace(&renamed).unwrap();
        assert!(store.recent_workspaces().is_empty());
        assert!(store.layout(&spec).is_some());
        let loaded = WorkspaceStore::load(&temp.path("config")).unwrap();
        assert_eq!(loaded.stored_spec(&spec).unwrap().name, "SSH project");
    }
    #[test]
    fn renaming_local_workspace_survives_legacy_recording_and_layout_changes() {
        let temp = TempDir::new();
        fs::create_dir_all(temp.path("project")).unwrap();
        let mut store = WorkspaceStore::load(&temp.path("config")).unwrap();
        let root = store.record_project(&temp.path("project")).unwrap();
        let spec = WorkspaceSpec::local(root.to_str().unwrap());
        store.rename_workspace(&spec, "  My project  ").unwrap();
        store.record_project(&root).unwrap();
        store
            .set_workspace(&root, json!({"dock":"updated"}))
            .unwrap();
        assert_eq!(store.recent_workspaces()[0].name, "My project");
        assert!(store.rename_workspace(&spec, "  ").is_err());
    }
    #[test]
    fn legacy_executable_overrides_are_ignored_without_losing_layout() {
        let temp = TempDir::new();
        let config = temp.path("config");
        let mut store = WorkspaceStore::load(&config).unwrap();
        let spec = store.record_workspace(remote("host", "/project")).unwrap();
        store.set_layout(&spec, json!({"dock":"existing"})).unwrap();
        store.state["workspaces"][spec.identity()]["spec"]["target"]["agent"] =
            json!("/old/custom/bed-headless");
        store.save().unwrap();
        let mut store = WorkspaceStore::load(&config).unwrap();
        assert_eq!(store.recent_workspaces(), vec![spec.clone()]);
        store.record_workspace(spec.clone()).unwrap();
        assert_eq!(store.layout(&spec).unwrap()["dock"], "existing");
        assert!(
            store.state["workspaces"][spec.identity()]["spec"]["target"]
                .get("agent")
                .is_none()
        );
    }
    #[test]
    fn remote_projects_round_trip_without_executable_configuration() {
        let temp = TempDir::new();
        let config = temp.path("config");
        let mut store = WorkspaceStore::load(&config).unwrap();
        let recorded = store.record_workspace(remote("host", "/project")).unwrap();
        let loaded = WorkspaceStore::load(&config).unwrap();
        assert_eq!(loaded.recent_workspaces(), vec![recorded.clone()]);
        assert_eq!(
            loaded.state["workspaces"][recorded.identity()]["spec"]["target"],
            json!({"kind":"ssh","host":"host"})
        );
    }
}
