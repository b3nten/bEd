//! Standalone recent projects and layout persistence. Embedding owns its layout.
use bed_app_config::LayoutSlot;
use serde_json::{Value, json};
use std::io;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use bed_plugin_workspaces::default_name;
pub use bed_plugin_workspaces::{WorkspaceSpec, WorkspaceTarget};

pub struct WorkspaceStore {
    path: PathBuf,
    state: Value,
}

impl WorkspaceStore {
    /// Disabled workspace integration never reads or writes saved session state.
    pub fn empty(config_dir: &Path) -> Self {
        Self {
            path: config_dir.join("workspaces.json"),
            state: json!({"version":2,"recent":[],"workspaces":{}}),
        }
    }
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
        for (layouts, active) in state["workspaces"]
            .as_object()
            .unwrap()
            .values()
            .map(|workspace| {
                (
                    workspace.get("layouts"),
                    workspace.get("active_layout_slot"),
                )
            })
            .chain(std::iter::once((
                state.get("standalone_layouts"),
                state.get("standalone_active_layout_slot"),
            )))
        {
            if active.is_some_and(|active| LayoutSlot::from_value(active).is_none())
                || layouts.is_some_and(|layouts| {
                    !layouts.as_object().is_some_and(|layouts| {
                        layouts.iter().all(|(slot, layout)| {
                            LayoutSlot::from_key(slot).is_some() && layout["panels"].is_array()
                        })
                    })
                })
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Invalid saved layout slots",
                ));
            }
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

    pub fn default_layout(&self) -> Option<&Value> {
        self.state
            .get("default_layout")
            .filter(|value| !value.is_null())
    }

    pub fn set_default_layout(&mut self, layout: Value) -> io::Result<()> {
        let _lock = self.reload_for_write()?;
        self.state["default_layout"] = layout;
        self.save()
    }

    pub fn reset_default_layout(&mut self) -> io::Result<()> {
        let _lock = self.reload_for_write()?;
        self.state.as_object_mut().unwrap().remove("default_layout");
        self.save()
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
        let _lock = self.reload_for_write()?;
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
        let _lock = self.reload_for_write()?;
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
        let _lock = self.reload_for_write()?;
        let mut renamed = self.stored_spec(spec).unwrap_or_else(|| spec.clone());
        renamed.name = name.to_owned();
        self.ensure_record(&renamed);
        self.state["workspaces"][renamed.identity()]["spec"] = renamed.to_value();
        self.save()?;
        Ok(renamed)
    }

    pub fn layout(&self, spec: &WorkspaceSpec) -> Option<&Value> {
        let workspace = self.state.get("workspaces")?.get(spec.identity())?;
        workspace
            .get("layouts")
            .and_then(|layouts| layouts.get(self.active_layout_slot(Some(spec)).to_string()))
            .or_else(|| workspace.get("layout"))
    }

    /// Module state belongs to the workspace, independent of its visible panels.
    pub fn all_module_settings(&self, spec: &WorkspaceSpec) -> Option<&Value> {
        self.state["workspaces"][spec.identity()].get("modules")
    }

    pub fn all_standalone_module_settings(&self) -> Option<&Value> {
        self.state.get("standalone_modules")
    }

    pub fn module_settings(&self, spec: &WorkspaceSpec, module: &str) -> Option<&Value> {
        let workspace = self.state.get("workspaces")?.get(spec.identity())?;
        workspace
            .get("modules")
            .and_then(|modules| modules.get(module))
            .or_else(|| workspace.get(legacy_module_field(module)?))
    }

    /// No-project module state is independent of the standalone window layout.
    pub fn standalone_module_settings(&self, module: &str) -> Option<&Value> {
        self.state.get("standalone_modules")?.get(module)
    }

    pub fn save_standalone_module_settings(&mut self, settings: Value) -> io::Result<()> {
        let _lock = self.reload_for_write()?;
        if self.state.get("standalone_modules") == Some(&settings) {
            return Ok(());
        }
        self.state["standalone_modules"] = settings;
        self.save()
    }

    /// Update one module without replacing other modules' workspace state.
    pub fn set_module_settings(
        &mut self,
        spec: &WorkspaceSpec,
        module: &str,
        value: Value,
    ) -> io::Result<()> {
        let _lock = self.reload_for_write()?;
        self.ensure_record(spec);
        let workspace = &mut self.state["workspaces"][spec.identity()];
        if !workspace["modules"].is_object() {
            workspace["modules"] = json!({});
        }
        workspace["modules"][module] = value;
        if let Some(legacy) = legacy_module_field(module) {
            workspace.as_object_mut().unwrap().remove(legacy);
        }
        self.save()
    }

    /// Replace the complete snapshot of registered modules' workspace state.
    pub fn save_module_settings(
        &mut self,
        spec: &WorkspaceSpec,
        settings: Value,
    ) -> io::Result<()> {
        let _lock = self.reload_for_write()?;
        self.ensure_record(spec);
        let workspace = &mut self.state["workspaces"][spec.identity()];
        let legacy: Vec<_> = settings
            .as_object()
            .into_iter()
            .flat_map(|modules| modules.keys())
            .filter_map(|module| legacy_module_field(module))
            .filter(|field| workspace.get(*field).is_some())
            .collect();
        if workspace.get("modules") == Some(&settings) && legacy.is_empty() {
            return Ok(());
        }
        for field in legacy {
            workspace.as_object_mut().unwrap().remove(field);
        }
        workspace["modules"] = settings;
        self.save()
    }

    /// Older configurations resume the most recent project. An explicit null
    /// means the last window had no project open.
    pub fn last_workspace(&self) -> Option<WorkspaceSpec> {
        match self.state.get("last_workspace") {
            Some(Value::String(id)) => {
                WorkspaceSpec::from_value(&self.state["workspaces"][id]["spec"])
            }
            Some(_) => None,
            None => self.recent_workspaces().into_iter().next(),
        }
    }

    pub fn standalone_layout(&self) -> Option<&Value> {
        self.session_layouts(None)
            .and_then(|layouts| layouts.get(self.active_layout_slot(None).to_string()))
            .or_else(|| self.state.get("standalone_layout"))
    }

    pub fn session_layouts(&self, spec: Option<&WorkspaceSpec>) -> Option<&Value> {
        match spec {
            Some(spec) => self.state["workspaces"][spec.identity()].get("layouts"),
            None => self.state.get("standalone_layouts"),
        }
    }

    pub fn active_layout_slot(&self, spec: Option<&WorkspaceSpec>) -> LayoutSlot {
        let value = match spec {
            Some(spec) => &self.state["workspaces"][spec.identity()]["active_layout_slot"],
            None => &self.state["standalone_active_layout_slot"],
        };
        LayoutSlot::from_value(value).unwrap_or_default()
    }

    pub fn save_session_layout(
        &mut self,
        spec: Option<&WorkspaceSpec>,
        layout: Value,
    ) -> io::Result<()> {
        let _lock = self.reload_for_write()?;
        if let Some(layouts) = layout.get("layouts") {
            let slot = LayoutSlot::from_value(&layout["active_layout_slot"]).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "Invalid layout slot")
            })?;
            if !layouts.is_object()
                || layouts
                    .as_object()
                    .unwrap()
                    .keys()
                    .any(|key| LayoutSlot::from_key(key).is_none())
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Invalid layout slots",
                ));
            }
            if let Some(spec) = spec {
                self.ensure_record(spec);
                let id = spec.identity();
                let workspace = &mut self.state["workspaces"][&id];
                workspace["layouts"] = layouts.clone();
                workspace["active_layout_slot"] = slot.to_value();
                workspace.as_object_mut().unwrap().remove("layout");
                self.state["last_workspace"] = json!(id);
            } else {
                self.state["standalone_layouts"] = layouts.clone();
                self.state["standalone_active_layout_slot"] = slot.to_value();
                self.state
                    .as_object_mut()
                    .unwrap()
                    .remove("standalone_layout");
                self.state["last_workspace"] = Value::Null;
            }
            return self.save();
        }
        if let Some(spec) = spec {
            self.ensure_record(spec);
            let id = spec.identity();
            if self.state["workspaces"][&id]["layouts"].is_object() {
                let slot = self.active_layout_slot(Some(spec)).to_string();
                self.state["workspaces"][&id]["layouts"][slot] = layout;
            } else {
                self.state["workspaces"][&id]["layout"] = layout;
            }
            self.state["last_workspace"] = json!(id);
        } else {
            if self.state["standalone_layouts"].is_object() {
                let slot = self.active_layout_slot(None).to_string();
                self.state["standalone_layouts"][slot] = layout;
            } else {
                self.state["standalone_layout"] = layout;
            }
            self.state["last_workspace"] = Value::Null;
        }
        self.save()
    }

    pub fn set_layout(&mut self, spec: &WorkspaceSpec, layout: Value) -> io::Result<()> {
        let _lock = self.reload_for_write()?;
        self.ensure_record(spec);
        let id = spec.identity();
        if self.state["workspaces"][&id]["layouts"].is_object() {
            let slot = self.active_layout_slot(Some(spec)).to_string();
            self.state["workspaces"][&id]["layouts"][slot] = layout;
        } else {
            self.state["workspaces"][&id]["layout"] = layout;
        }
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

    /// Save a live session atomically without restoring or replacing its state.
    pub fn commit_snapshot(
        &mut self,
        mut spec: WorkspaceSpec,
        snapshot: bed_plugin_workspaces::WorkspaceSnapshot,
    ) -> io::Result<WorkspaceSpec> {
        bed_storage::validate_snapshot(&snapshot.plugins)?;
        match &spec.target {
            WorkspaceTarget::Local => {
                let root = fs::canonicalize(&spec.root)?;
                if !root.is_dir() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "Workspace must be a directory",
                    ));
                }
                spec.root = root
                    .to_str()
                    .ok_or_else(|| {
                        io::Error::new(io::ErrorKind::InvalidInput, "Workspace path is not UTF-8")
                    })?
                    .to_owned();
            }
            WorkspaceTarget::Ssh { host }
                if host.trim().is_empty() || !spec.root.starts_with('/') =>
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "SSH workspace requires a host and absolute project path",
                ));
            }
            _ => {}
        }
        spec.name = spec.name.trim().to_owned();
        if spec.name.is_empty() {
            spec.name = default_name(&spec.root);
        }
        let _lock = self.reload_for_write()?;
        let previous = self.state.clone();
        let id = spec.identity();
        self.ensure_record(&spec);
        let entry = &mut self.state["workspaces"][&id];
        entry["spec"] = spec.to_value();
        if let Some(incoming) = snapshot.layouts.get("layouts") {
            let Some(active) = snapshot.layouts.get("active_layout_slot") else {
                self.state = previous;
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Missing active layout slot",
                ));
            };
            if LayoutSlot::from_value(active).is_none()
                || !incoming.as_object().is_some_and(|slots| {
                    slots.iter().all(|(slot, value)| {
                        LayoutSlot::from_key(slot).is_some() && value["panels"].is_array()
                    })
                })
            {
                self.state = previous;
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Invalid layout slots",
                ));
            }
            let mut slots = entry["layouts"].as_object().cloned().unwrap_or_default();
            slots.extend(incoming.as_object().unwrap().clone());
            entry["layouts"] = Value::Object(slots);
            entry["active_layout_slot"] = active.clone();
            entry.as_object_mut().unwrap().remove("layout");
        } else if !snapshot.layouts.is_null() {
            if !snapshot.layouts["panels"].is_array() {
                self.state = previous;
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Invalid layout",
                ));
            }
            entry["layout"] = snapshot.layouts;
        }
        let mut plugins = entry["modules"].as_object().cloned().unwrap_or_default();
        plugins.extend(snapshot.plugins.as_object().unwrap().clone());
        entry["modules"] = Value::Object(plugins);
        let mut recent = self.state["recent"].as_array().cloned().unwrap_or_default();
        recent.retain(|entry| entry.as_str() != Some(&id));
        recent.insert(0, Value::String(id.clone()));
        recent.truncate(20);
        self.state["recent"] = Value::Array(recent);
        self.state["last_workspace"] = Value::String(id);
        if self.state == previous {
            return Ok(spec);
        }
        if let Err(error) = self.save() {
            self.state = previous;
            return Err(error);
        }
        Ok(spec)
    }

    pub fn state_store(&self) -> WorkspaceStateStore {
        WorkspaceStateStore {
            config_dir: self.path.parent().unwrap().to_owned(),
        }
    }

    pub fn repository(&self) -> WorkspaceStoreRepository {
        WorkspaceStoreRepository {
            config_dir: self.path.parent().unwrap().to_owned(),
        }
    }
    pub fn reload(&mut self) -> io::Result<()> {
        *self = Self::load(self.path.parent().unwrap())?;
        Ok(())
    }

    fn save(&self) -> io::Result<()> {
        write_atomic(&self.path, &self.state)
    }

    fn reload_for_write(&mut self) -> io::Result<fs::File> {
        let parent = self
            .path
            .parent()
            .expect("workspace store has a config directory");
        fs::create_dir_all(parent)?;
        // Lock a stable sidecar: saving atomically replaces workspaces.json's
        // inode. The lock is released on close, including errors/process exit.
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.path.with_extension("lock"))?;
        lock.lock()?;
        self.state = Self::load(parent)?.state;
        Ok(lock)
    }
}

// Old application fields are compatibility aliases, never feature-owned types.
fn legacy_module_field(module: &str) -> Option<&'static str> {
    match module {
        "bed.debug" => Some("debug"),
        "bed.explorer" => Some("file_tree"),
        _ => None,
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
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;

/// bEd's disk adapter keeps remote UI state on the client and shares the existing file lock.
pub struct WorkspaceStoreRepository {
    config_dir: PathBuf,
}
impl bed_plugin_workspaces::WorkspaceRepository for WorkspaceStoreRepository {
    fn recent(&self) -> io::Result<Vec<WorkspaceSpec>> {
        Ok(WorkspaceStore::load(&self.config_dir)?.recent_workspaces())
    }
    fn find(&self, spec: &WorkspaceSpec) -> io::Result<Option<WorkspaceSpec>> {
        Ok(WorkspaceStore::load(&self.config_dir)?.stored_spec(spec))
    }
    fn load(
        &self,
        spec: &WorkspaceSpec,
    ) -> io::Result<Option<bed_plugin_workspaces::WorkspaceSnapshot>> {
        let store = WorkspaceStore::load(&self.config_dir)?;
        if store.stored_spec(spec).is_none() {
            return Ok(None);
        }
        let layouts = match store.session_layouts(Some(spec)) {
            Some(layouts) => {
                json!({"layouts":layouts,"active_layout_slot":store.active_layout_slot(Some(spec)).to_value()})
            }
            None => store.layout(spec).cloned().unwrap_or(Value::Null),
        };
        if let Some(state) = store.all_module_settings(spec) {
            bed_storage::validate_snapshot(state)?;
        }
        let mut plugins = store
            .all_module_settings(spec)
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        for owner in ["bed.debug", "bed.explorer"] {
            if !plugins.contains_key(owner)
                && let Some(value) = store.module_settings(spec, owner)
            {
                plugins.insert(owner.to_owned(), value.clone());
            }
        }
        let plugins = Value::Object(plugins);
        Ok(Some(bed_plugin_workspaces::WorkspaceSnapshot {
            layouts,
            plugins,
        }))
    }
    fn commit(
        &mut self,
        spec: WorkspaceSpec,
        snapshot: bed_plugin_workspaces::WorkspaceSnapshot,
    ) -> io::Result<WorkspaceSpec> {
        WorkspaceStore::load(&self.config_dir)?.commit_snapshot(spec, snapshot)
    }
    fn forget(&mut self, spec: &WorkspaceSpec) -> io::Result<()> {
        WorkspaceStore::load(&self.config_dir)?.forget_workspace(spec)
    }
    fn rename(&mut self, spec: &WorkspaceSpec, name: &str) -> io::Result<WorkspaceSpec> {
        let mut store = WorkspaceStore::load(&self.config_dir)?;
        if store.stored_spec(spec).is_none() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "Workspace has not been saved",
            ));
        }
        store.rename_workspace(spec, name)
    }
}

/// Plugin-state adapter uses saved workspace identities as opaque scopes.
/// A state write cannot create a workspace definition or promote a temporary session.
pub struct WorkspaceStateStore {
    config_dir: PathBuf,
}
impl bed_storage::StateStore for WorkspaceStateStore {
    fn load(&self, scope: &bed_storage::StateScope) -> io::Result<Option<Value>> {
        let store = WorkspaceStore::load(&self.config_dir)?;
        Ok(store.state["workspaces"][&scope.0].get("modules").cloned())
    }
    fn save(&mut self, scope: &bed_storage::StateScope, snapshot: &Value) -> io::Result<()> {
        bed_storage::validate_snapshot(snapshot)?;
        let mut store = WorkspaceStore::load(&self.config_dir)?;
        let _lock = store.reload_for_write()?;
        let entry = store.state["workspaces"].get_mut(&scope.0).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "State scope has no saved workspace",
            )
        })?;
        let mut state = entry["modules"].as_object().cloned().unwrap_or_default();
        state.extend(snapshot.as_object().unwrap().clone());
        entry["modules"] = Value::Object(state);
        store.save()
    }
}
