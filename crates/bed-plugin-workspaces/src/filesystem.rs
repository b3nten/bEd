//! Application-selected workspace snapshots, a client locator index, and layout templates.
use bed_api::launch::ApplicationPaths;
use bed_plugin_layouts::LayoutSlot;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

pub use crate::{WorkspaceSpec, WorkspaceTarget};

pub struct WorkspaceStore {
    paths: ApplicationPaths,
    registry: Value,
    templates: Value,
    snapshots: HashMap<String, Value>,
    index_error: Option<String>,
}

impl WorkspaceStore {
    /// Disabling workspace integration touches no saved snapshots or locator index.
    pub fn empty(paths: &ApplicationPaths) -> Self {
        Self {
            paths: paths.clone(),
            registry: empty_registry(),
            templates: empty_templates(),
            snapshots: HashMap::new(),
            index_error: None,
        }
    }

    pub fn load(paths: &ApplicationPaths) -> io::Result<Self> {
        let mut store = Self::empty(paths);
        store.registry =
            read_json(&paths.config_dir.join("registry.json"))?.unwrap_or_else(empty_registry);
        validate_registry(&store.registry)?;
        store.templates =
            read_json(&paths.config_dir.join("layouts.json"))?.unwrap_or_else(empty_templates);
        validate_templates(&store.templates)?;
        for entry in store.registry["entries"].as_object().unwrap().values() {
            let spec = WorkspaceSpec::from_value(&entry["spec"]).unwrap();
            // Invalid markers are reported when their location is opened, rather than
            // preventing startup because an unrelated recent location is damaged.
            if let Ok(Some(snapshot)) = store.read_snapshot(&spec) {
                store.snapshots.insert(spec.identity(), snapshot);
            }
        }
        Ok(store)
    }

    pub fn reload(&mut self) -> io::Result<()> {
        let mut loaded = Self::load(&self.paths)?;
        loaded.index_error = self.index_error.take();
        *self = loaded;
        Ok(())
    }

    pub fn take_index_error(&mut self) -> Option<String> {
        self.index_error.take()
    }

    pub fn recent_workspaces(&self) -> Vec<WorkspaceSpec> {
        self.registry["recent"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .filter_map(|id| WorkspaceSpec::from_value(&self.registry["entries"][id]["spec"]))
            .collect()
    }

    pub fn last_workspace(&self) -> Option<WorkspaceSpec> {
        let id = self.registry["last_workspace"].as_str()?;
        WorkspaceSpec::from_value(&self.registry["entries"][id]["spec"])
    }

    pub fn stored_spec(&self, spec: &WorkspaceSpec) -> io::Result<Option<WorkspaceSpec>> {
        let Some(snapshot) = self.read_snapshot(spec)? else {
            return Ok(None);
        };
        let mut stored = spec.clone();
        stored.name = snapshot["name"].as_str().unwrap().to_owned();
        Ok(Some(stored))
    }

    /// Local markers are discoverable without an entry in the client index.
    pub fn workspace_path(&self, spec: &WorkspaceSpec) -> io::Result<Option<PathBuf>> {
        match &spec.target {
            WorkspaceTarget::Local => Ok(Some(
                self.paths
                    .workspace_directory(Path::new(&spec.root))
                    .join("workspace.json"),
            )),
            WorkspaceTarget::Ssh { .. } => {
                Ok(self.registry["entries"][spec.identity()]["remote_id"]
                    .as_u64()
                    .map(|id| {
                        self.paths
                            .config_dir
                            .join("remote")
                            .join(id.to_string())
                            .join("workspace.json")
                    }))
            }
        }
    }

    pub fn history_path(&self, spec: &WorkspaceSpec) -> Option<PathBuf> {
        matches!(spec.target, WorkspaceTarget::Local).then(|| {
            self.paths
                .workspace_directory(Path::new(&spec.root))
                .join("history.json")
        })
    }

    pub fn record_workspace(&mut self, spec: WorkspaceSpec) -> io::Result<WorkspaceSpec> {
        self.commit_snapshot(spec, crate::WorkspaceSnapshot::default())
    }

    pub fn forget_workspace(&mut self, spec: &WorkspaceSpec) -> io::Result<()> {
        let _lock = self.lock_registry()?;
        self.registry["recent"]
            .as_array_mut()
            .unwrap()
            .retain(|entry| entry.as_str() != Some(&spec.identity()));
        self.save_registry()
    }

    pub fn rename_workspace(
        &mut self,
        spec: &WorkspaceSpec,
        name: &str,
    ) -> io::Result<WorkspaceSpec> {
        if name.trim().is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Workspace name cannot be empty",
            ));
        }
        let mut renamed = self.stored_spec(spec)?.ok_or_else(not_saved)?;
        renamed.name = name.trim().to_owned();
        self.commit_snapshot(renamed, crate::WorkspaceSnapshot::default())
    }

    pub fn layout(&self, spec: &WorkspaceSpec) -> Option<&Value> {
        self.session_layouts(spec)?
            .get(self.active_layout_slot(spec).key())
    }

    pub fn all_plugin_state(&self, spec: &WorkspaceSpec) -> Option<&Value> {
        self.snapshots.get(&spec.identity())?.get("plugins")
    }

    pub fn plugin_state(&self, spec: &WorkspaceSpec, owner: &str) -> Option<&Value> {
        self.all_plugin_state(spec)?.get(owner)
    }

    pub fn set_plugin_state(
        &mut self,
        spec: &WorkspaceSpec,
        owner: &str,
        value: Value,
    ) -> io::Result<()> {
        self.save_plugin_state(spec, json!({owner:value}))
    }

    pub fn save_plugin_state(&mut self, spec: &WorkspaceSpec, plugins: Value) -> io::Result<()> {
        self.write_snapshot(
            spec.clone(),
            crate::WorkspaceSnapshot {
                layouts: Value::Null,
                plugins,
            },
            None,
        )?;
        Ok(())
    }

    pub fn session_layouts(&self, spec: &WorkspaceSpec) -> Option<&Value> {
        self.snapshots.get(&spec.identity())?.get("layouts")
    }

    pub fn active_layout_slot(&self, spec: &WorkspaceSpec) -> LayoutSlot {
        self.snapshots
            .get(&spec.identity())
            .and_then(|snapshot| snapshot.get("active_layout_slot"))
            .and_then(LayoutSlot::from_value)
            .unwrap_or_default()
    }

    pub fn save_session_layout(&mut self, spec: &WorkspaceSpec, layouts: Value) -> io::Result<()> {
        validate_layout_input(&layouts)?;
        self.write_snapshot(
            spec.clone(),
            crate::WorkspaceSnapshot {
                layouts,
                plugins: json!({}),
            },
            None,
        )?;
        Ok(())
    }

    pub fn set_layout(&mut self, spec: &WorkspaceSpec, layout: Value) -> io::Result<()> {
        let slot = self.active_layout_slot(spec);
        self.save_session_layout(
            spec,
            json!({"layouts":{slot.key():layout},"active_layout_slot":slot.to_value()}),
        )
    }

    pub fn layout_default(&self, slot: LayoutSlot) -> Option<Value> {
        self.templates["defaults"].get(slot.key()).cloned()
    }

    pub fn set_layout_default(
        &mut self,
        slot: LayoutSlot,
        layout: Option<Value>,
    ) -> io::Result<()> {
        if let Some(layout) = &layout {
            validate_layout(layout)?;
        }
        let _lock = self.lock_templates()?;
        let defaults = self.templates["defaults"].as_object_mut().unwrap();
        match layout {
            Some(layout) => {
                defaults.insert(slot.key().into(), layout);
            }
            None => {
                defaults.remove(slot.key());
            }
        }
        self.save_templates()
    }

    /// Snapshot success establishes the saved workspace. Later index failure is
    /// reported separately and never rolls back the authoritative snapshot.
    pub fn commit_snapshot(
        &mut self,
        spec: WorkspaceSpec,
        snapshot: crate::WorkspaceSnapshot,
    ) -> io::Result<WorkspaceSpec> {
        let name = spec.name.clone();
        self.write_snapshot(spec, snapshot, Some(name))
    }

    /// An explicit name permits creating a saved workspace. State-only updates
    /// preserve the authoritative label and require an existing marker.
    fn write_snapshot(
        &mut self,
        mut spec: WorkspaceSpec,
        snapshot: crate::WorkspaceSnapshot,
        name_update: Option<String>,
    ) -> io::Result<WorkspaceSpec> {
        bed_storage::validate_snapshot(&snapshot.plugins)?;
        if !snapshot.layouts.is_null() {
            validate_layout_input(&snapshot.layouts)?;
        }
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
            WorkspaceTarget::Ssh { host } => {
                if host.trim().is_empty() || !spec.root.starts_with('/') {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "SSH workspace requires a host and absolute root",
                    ));
                }
                if name_update.is_some() {
                    self.reserve_remote_locator(&spec)?;
                }
            }
        }
        if let Some(name) = &name_update {
            spec.name = name.trim().to_owned();
            if spec.name.is_empty() {
                spec.name = crate::default_name(&spec.root);
            }
        }
        let path = self.workspace_path(&spec)?.ok_or_else(not_saved)?;
        if name_update.is_none() && read_json(&path)?.is_none() {
            // A failed state write must not create a directory or sidecar lock.
            return Err(not_saved());
        }
        {
            let _lock = lock_path(&path)?;
            let mut saved = match read_json(&path)? {
                Some(saved) => saved,
                None if name_update.is_some() => empty_snapshot(&spec),
                None => return Err(not_saved()),
            };
            validate_workspace(&spec, &saved)?;
            if name_update.is_some() {
                saved["name"] = json!(spec.name);
            } else {
                spec.name = saved["name"].as_str().unwrap().to_owned();
            }
            if !snapshot.layouts.is_null() {
                merge_layouts(
                    &mut saved,
                    &snapshot.layouts,
                    "layouts",
                    "active_layout_slot",
                );
            }
            saved["plugins"]
                .as_object_mut()
                .unwrap()
                .extend(snapshot.plugins.as_object().unwrap().clone());
            write_atomic(&path, &saved)?;
            self.snapshots.insert(spec.identity(), saved);
        }
        self.index_error = None;
        if let Err(error) = self.record_locator(&spec) {
            self.index_error = Some(format!(
                "Workspace saved, but the recent-workspace index could not be updated: {error}"
            ));
        }
        Ok(spec)
    }

    pub fn repository(&self) -> WorkspaceStoreRepository {
        WorkspaceStoreRepository {
            paths: self.paths.clone(),
            index_error: None,
        }
    }

    pub fn state_store(&self) -> WorkspaceStateStore {
        WorkspaceStateStore {
            paths: self.paths.clone(),
        }
    }

    fn read_snapshot(&self, spec: &WorkspaceSpec) -> io::Result<Option<Value>> {
        let Some(path) = self.workspace_path(spec)? else {
            return Ok(None);
        };
        let Some(value) = read_json(&path)? else {
            return Ok(None);
        };
        validate_workspace(spec, &value).map_err(|error| {
            io::Error::new(error.kind(), format!("{}: {error}", path.display()))
        })?;
        Ok(Some(value))
    }

    fn reserve_remote_locator(&mut self, spec: &WorkspaceSpec) -> io::Result<()> {
        let _lock = self.lock_registry()?;
        if self.registry["entries"][spec.identity()]["remote_id"]
            .as_u64()
            .is_some()
        {
            return Ok(());
        }
        let remote = self.paths.config_dir.join("remote");
        fs::create_dir_all(&remote)?;
        let mut id = self.registry["next_remote_id"].as_u64().unwrap();
        loop {
            match fs::create_dir(remote.join(id.to_string())) {
                Ok(()) => break,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    id = id
                        .checked_add(1)
                        .ok_or_else(|| io::Error::other("Remote locator IDs exhausted"))?
                }
                Err(error) => return Err(error),
            }
        }
        self.registry["next_remote_id"] = json!(
            id.checked_add(1)
                .ok_or_else(|| io::Error::other("Remote locator IDs exhausted"))?
        );
        self.registry["entries"][spec.identity()] = json!({"spec":spec.to_value(),"remote_id":id});
        // Reservation records only the location; no marker and no recents means
        // an interrupted first save remains temporary.
        self.save_registry()
    }

    fn record_locator(&mut self, spec: &WorkspaceSpec) -> io::Result<()> {
        let _lock = self.lock_registry()?;
        let id = spec.identity();
        // Another instance may have renamed the marker after this writer released
        // its snapshot lock. The locator uses that authoritative current label.
        let current = self.stored_spec(spec)?.ok_or_else(not_saved)?;
        self.registry["entries"][&id]["spec"] = current.to_value();
        let recent = self.registry["recent"].as_array_mut().unwrap();
        recent.retain(|entry| entry.as_str() != Some(&id));
        recent.insert(0, json!(id));
        recent.truncate(20);
        self.registry["last_workspace"] = json!(id);
        self.save_registry()
    }

    fn lock_registry(&mut self) -> io::Result<fs::File> {
        let path = self.paths.config_dir.join("registry.json");
        let lock = lock_path(&path)?;
        self.registry = read_json(&path)?.unwrap_or_else(empty_registry);
        validate_registry(&self.registry)?;
        Ok(lock)
    }

    fn lock_templates(&mut self) -> io::Result<fs::File> {
        let path = self.paths.config_dir.join("layouts.json");
        let lock = lock_path(&path)?;
        self.templates = read_json(&path)?.unwrap_or_else(empty_templates);
        validate_templates(&self.templates)?;
        Ok(lock)
    }

    fn save_registry(&self) -> io::Result<()> {
        write_atomic(&self.paths.config_dir.join("registry.json"), &self.registry)
    }
    fn save_templates(&self) -> io::Result<()> {
        write_atomic(&self.paths.config_dir.join("layouts.json"), &self.templates)
    }
}

fn empty_registry() -> Value {
    json!({"version":1,"entries":{},"recent":[],"last_workspace":null,"next_remote_id":1})
}
fn empty_templates() -> Value {
    json!({"version":1,"defaults":{}})
}
fn empty_snapshot(spec: &WorkspaceSpec) -> Value {
    let mut snapshot = json!({"version":1,"name":spec.name,"layouts":{},"active_layout_slot":"startup","plugins":{}});
    if matches!(spec.target, WorkspaceTarget::Ssh { .. }) {
        snapshot["identity"] = json!(spec.identity());
    }
    snapshot
}
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn not_saved() -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, "Workspace has not been saved")
}
fn read_json(path: &Path) -> io::Result<Option<Value>> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}
fn validate_registry(value: &Value) -> io::Result<()> {
    if value["version"].as_u64() != Some(1)
        || !value["entries"].is_object()
        || !value["recent"].is_array()
        || value["next_remote_id"].as_u64().is_none_or(|id| id == 0)
        || !(value["last_workspace"].is_null() || value["last_workspace"].is_string())
    {
        return Err(invalid("Invalid workspace locator index"));
    }
    for (id, entry) in value["entries"].as_object().unwrap() {
        let spec = WorkspaceSpec::from_value(&entry["spec"])
            .ok_or_else(|| invalid("Invalid workspace locator"))?;
        if !Path::new(&spec.root).is_absolute()
            || matches!(&spec.target, WorkspaceTarget::Ssh { host } if host.trim().is_empty())
        {
            return Err(invalid(
                "Workspace locators require an absolute root and valid host",
            ));
        }
        if &spec.identity() != id
            || (matches!(spec.target, WorkspaceTarget::Ssh { .. })
                && entry["remote_id"].as_u64().is_none_or(|id| id == 0))
        {
            return Err(invalid("Invalid workspace locator identity"));
        }
    }
    if value["recent"].as_array().unwrap().iter().any(|id| {
        id.as_str()
            .is_none_or(|id| !value["entries"][id].is_object())
    }) {
        return Err(invalid("Invalid recent workspace locator"));
    }
    Ok(())
}
fn validate_layout(layout: &Value) -> io::Result<()> {
    if layout["version"] == 2
        && layout["panels"]
            .as_array()
            .is_some_and(|panels| panels.iter().all(|panel| panel["panel_type"].is_string()))
    {
        Ok(())
    } else {
        Err(invalid(
            "Saved layout must have version 2 and a panels array",
        ))
    }
}
fn validate_layouts(layouts: &Value, active: &Value) -> io::Result<()> {
    if LayoutSlot::from_value(active).is_none() || !layouts.is_object() {
        return Err(invalid("Invalid saved layout slots"));
    }
    for (slot, layout) in layouts.as_object().unwrap() {
        if LayoutSlot::from_key(slot).is_none() {
            return Err(invalid("Invalid saved layout slot"));
        }
        validate_layout(layout)?;
    }
    Ok(())
}
fn validate_layout_input(layouts: &Value) -> io::Result<()> {
    validate_layouts(&layouts["layouts"], &layouts["active_layout_slot"])
}
fn validate_workspace(spec: &WorkspaceSpec, value: &Value) -> io::Result<()> {
    if value["version"].as_u64() != Some(1)
        || value["name"]
            .as_str()
            .is_none_or(|name| name.trim().is_empty())
    {
        return Err(invalid("Invalid workspace marker"));
    }
    if matches!(spec.target, WorkspaceTarget::Ssh { .. })
        && value["identity"].as_str() != Some(&spec.identity())
    {
        return Err(invalid("Remote snapshot belongs to a different location"));
    }
    bed_storage::validate_snapshot(&value["plugins"])?;
    validate_layouts(&value["layouts"], &value["active_layout_slot"])
}
fn validate_templates(value: &Value) -> io::Result<()> {
    if value["version"].as_u64() != Some(1) || !value["defaults"].is_object() {
        return Err(invalid("Invalid layout templates"));
    }
    for (slot, layout) in value["defaults"].as_object().unwrap() {
        if LayoutSlot::from_key(slot).is_none() {
            return Err(invalid("Invalid default layout slot"));
        }
        validate_layout(layout)?;
    }
    Ok(())
}
fn merge_layouts(target: &mut Value, incoming: &Value, layouts_field: &str, active_field: &str) {
    target[layouts_field]
        .as_object_mut()
        .unwrap()
        .extend(incoming["layouts"].as_object().unwrap().clone());
    target[active_field] = incoming["active_layout_slot"].clone();
}
fn lock_path(path: &Path) -> io::Result<fs::File> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "State path requires a parent")
    })?;
    fs::create_dir_all(parent)?;
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path.with_extension("lock"))?;
    file.lock()?;
    Ok(file)
}
pub fn write_atomic(path: &Path, value: &Value) -> io::Result<()> {
    static NEXT_WRITE: AtomicU64 = AtomicU64::new(1);
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "State path requires a parent")
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

/// The reusable plugin consumes this app-selected repository without knowing file locations.
pub struct WorkspaceStoreRepository {
    paths: ApplicationPaths,
    index_error: Option<String>,
}
impl WorkspaceStoreRepository {
    fn store_for(&self, spec: &WorkspaceSpec) -> io::Result<WorkspaceStore> {
        match spec.target {
            WorkspaceTarget::Local => Ok(WorkspaceStore::empty(&self.paths)),
            WorkspaceTarget::Ssh { .. } => WorkspaceStore::load(&self.paths),
        }
    }
}
impl crate::WorkspaceRepository for WorkspaceStoreRepository {
    fn recent(&self) -> io::Result<Vec<WorkspaceSpec>> {
        Ok(WorkspaceStore::load(&self.paths)?.recent_workspaces())
    }
    fn find(&self, spec: &WorkspaceSpec) -> io::Result<Option<WorkspaceSpec>> {
        self.store_for(spec)?.stored_spec(spec)
    }
    fn load(&self, spec: &WorkspaceSpec) -> io::Result<Option<crate::WorkspaceSnapshot>> {
        let store = self.store_for(spec)?;
        let Some(saved) = store.read_snapshot(spec)? else {
            return Ok(None);
        };
        Ok(Some(crate::WorkspaceSnapshot {
            layouts: json!({"layouts":saved["layouts"],"active_layout_slot":saved["active_layout_slot"]}),
            plugins: saved["plugins"].clone(),
        }))
    }
    fn commit(
        &mut self,
        spec: WorkspaceSpec,
        snapshot: crate::WorkspaceSnapshot,
    ) -> io::Result<WorkspaceSpec> {
        let mut store = self.store_for(&spec)?;
        let saved = store.commit_snapshot(spec, snapshot)?;
        self.index_error = store.take_index_error();
        Ok(saved)
    }
    fn forget(&mut self, spec: &WorkspaceSpec) -> io::Result<()> {
        WorkspaceStore::load(&self.paths)?.forget_workspace(spec)
    }
    fn rename(&mut self, spec: &WorkspaceSpec, name: &str) -> io::Result<WorkspaceSpec> {
        let mut store = self.store_for(spec)?;
        let saved = store.rename_workspace(spec, name)?;
        self.index_error = store.take_index_error();
        Ok(saved)
    }
    fn take_index_error(&mut self) -> Option<String> {
        self.index_error.take()
    }
}

/// A state write updates an existing marker and cannot promote a temporary directory.
pub struct WorkspaceStateStore {
    paths: ApplicationPaths,
}
impl bed_storage::StateStore for WorkspaceStateStore {
    fn load(&self, scope: &bed_storage::StateScope) -> io::Result<Option<Value>> {
        let spec = scope_spec(scope)?;
        let store = match spec.target {
            WorkspaceTarget::Local => WorkspaceStore::empty(&self.paths),
            WorkspaceTarget::Ssh { .. } => WorkspaceStore::load(&self.paths)?,
        };
        Ok(store
            .read_snapshot(&spec)?
            .map(|snapshot| snapshot["plugins"].clone()))
    }
    fn save(&mut self, scope: &bed_storage::StateScope, snapshot: &Value) -> io::Result<()> {
        let spec = scope_spec(scope)?;
        let mut store = match spec.target {
            WorkspaceTarget::Local => WorkspaceStore::empty(&self.paths),
            WorkspaceTarget::Ssh { .. } => WorkspaceStore::load(&self.paths)?,
        };
        store.save_plugin_state(&spec, snapshot.clone())
    }
}
fn scope_spec(scope: &bed_storage::StateScope) -> io::Result<WorkspaceSpec> {
    let value: Value = serde_json::from_str(&scope.0)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    let parts = value.as_array().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "Invalid workspace state scope")
    })?;
    match parts.as_slice() {
        [kind, root] if kind.as_str() == Some("local") && root.is_string() => {
            Ok(WorkspaceSpec::local(root.as_str().unwrap()))
        }
        [kind, host, root]
            if kind.as_str() == Some("ssh") && host.is_string() && root.is_string() =>
        {
            Ok(WorkspaceSpec {
                name: crate::default_name(root.as_str().unwrap()),
                target: WorkspaceTarget::Ssh {
                    host: host.as_str().unwrap().into(),
                },
                root: root.as_str().unwrap().into(),
            })
        }
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Invalid workspace state scope",
        )),
    }
}

#[cfg(test)]
#[path = "filesystem_tests.rs"]
mod tests;
