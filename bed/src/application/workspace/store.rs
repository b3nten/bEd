//! bEd owns numbered layout templates; workspace snapshots use the generic filesystem adapter.
use crate::config::LayoutSlot;
use bedtime_api::launch::ApplicationPaths;
pub use bedtime_plugin_workspaces::filesystem::{WorkspaceStateStore, WorkspaceStoreRepository};
pub use bedtime_plugin_workspaces::{WorkspaceSpec, WorkspaceTarget};
use serde_json::{Value, json};
use std::{
    fs, io,
    ops::{Deref, DerefMut},
    path::Path,
};

pub struct WorkspaceStore {
    backend: bedtime_plugin_workspaces::filesystem::WorkspaceStore,
    paths: ApplicationPaths,
    templates: Value,
}

impl Deref for WorkspaceStore {
    type Target = bedtime_plugin_workspaces::filesystem::WorkspaceStore;
    fn deref(&self) -> &Self::Target {
        &self.backend
    }
}
impl DerefMut for WorkspaceStore {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.backend
    }
}

impl WorkspaceStore {
    pub fn empty(paths: &ApplicationPaths) -> Self {
        Self {
            backend: bedtime_plugin_workspaces::filesystem::WorkspaceStore::empty(paths),
            paths: paths.clone(),
            templates: json!({"version":1,"defaults":{}}),
        }
    }

    /// Global layout defaults work without installing or loading workspaces.
    pub fn load_templates(paths: &ApplicationPaths) -> io::Result<Self> {
        Ok(Self {
            templates: read_templates(&paths.config_dir.join("layouts.json"))?,
            ..Self::empty(paths)
        })
    }
    pub fn load(paths: &ApplicationPaths) -> io::Result<Self> {
        let mut store = Self::load_templates(paths)?;
        store.backend = bedtime_plugin_workspaces::filesystem::WorkspaceStore::load(paths)?;
        Ok(store)
    }

    pub fn reload(&mut self) -> io::Result<()> {
        let next = Self::load(&self.paths)?;
        *self = next;
        Ok(())
    }

    pub fn session_layouts(&self, spec: &WorkspaceSpec) -> Option<&Value> {
        self.backend.layout_snapshot(spec)?.get("layouts")
    }

    pub fn active_layout_slot(&self, spec: &WorkspaceSpec) -> LayoutSlot {
        self.backend
            .layout_snapshot(spec)
            .and_then(|value| value.get("active_layout_slot"))
            .and_then(LayoutSlot::from_value)
            .unwrap_or_default()
    }

    pub fn layout(&self, spec: &WorkspaceSpec) -> Option<&Value> {
        self.session_layouts(spec)?
            .get(self.active_layout_slot(spec).key())
    }

    pub fn save_session_layout(&mut self, spec: &WorkspaceSpec, incoming: Value) -> io::Result<()> {
        validate_slots(&incoming)?;
        let mut complete = self.backend.layout_snapshot(spec).cloned().unwrap_or_else(
            || json!({"layouts":{},"active_layout_slot":incoming["active_layout_slot"]}),
        );
        let mut slots = complete["layouts"].as_object().cloned().unwrap_or_default();
        slots.extend(
            incoming["layouts"]
                .as_object()
                .expect("validated slots")
                .clone(),
        );
        complete["layouts"] = Value::Object(slots);
        complete["active_layout_slot"] = incoming["active_layout_slot"].clone();
        self.backend.save_layout_snapshot(spec, complete)
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
        fs::create_dir_all(&self.paths.config_dir)?;
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.paths.config_dir.join("layouts.lock"))?;
        lock.lock()?;
        let mut next = read_templates(&self.paths.config_dir.join("layouts.json"))?;
        let defaults = next["defaults"]
            .as_object_mut()
            .expect("validated templates");
        match layout {
            Some(layout) => {
                defaults.insert(slot.key().into(), layout);
            }
            None => {
                defaults.remove(slot.key());
            }
        }
        bedtime_plugin_workspaces::filesystem::write_atomic(
            &self.paths.config_dir.join("layouts.json"),
            &next,
        )?;
        self.templates = next;
        Ok(())
    }
}

fn validate_layout(value: &Value) -> io::Result<()> {
    bedtime_plugin_layouts::LayoutDefinition::from_value(value).map(|_| ())
}

fn validate_slots(value: &Value) -> io::Result<()> {
    let slots = value["layouts"]
        .as_object()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Invalid numbered layouts"))?;
    if LayoutSlot::from_value(&value["active_layout_slot"]).is_none() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Invalid active layout slot",
        ));
    }
    for (key, layout) in slots {
        if LayoutSlot::from_key(key).is_none() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid layout slot",
            ));
        }
        bedtime_core::validate_document_layout(layout).map_err(io::Error::other)?;
    }
    Ok(())
}

fn read_templates(path: &Path) -> io::Result<Value> {
    let value: Value = match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => json!({"version":1,"defaults":{}}),
        Err(error) => return Err(error),
    };
    let defaults = value["defaults"]
        .as_object()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Invalid layout templates"))?;
    if value["version"] != 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Unsupported layout templates",
        ));
    }
    for (key, layout) in defaults {
        if LayoutSlot::from_key(key).is_none() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid default layout slot",
            ));
        }
        validate_layout(layout)?;
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn global_layout_defaults_load_and_save_without_a_workspace_registry() {
        let temp = crate::test_support::TempDir::new();
        temp.write("config/registry.json", b"{invalid workspace registry");
        let paths = ApplicationPaths::new("bed", Some(temp.path("config")), None).unwrap();
        let definition =
            bedtime_plugin_layouts::LayoutDefinition::new(Default::default()).to_value();
        let mut store = WorkspaceStore::load_templates(&paths).unwrap();
        store
            .set_layout_default(LayoutSlot::Layout8, Some(definition.clone()))
            .unwrap();
        let reloaded = WorkspaceStore::load_templates(&paths).unwrap();
        assert_eq!(
            reloaded.layout_default(LayoutSlot::Layout8),
            Some(definition)
        );
        assert!(WorkspaceStore::load(&paths).is_err());
    }
}
