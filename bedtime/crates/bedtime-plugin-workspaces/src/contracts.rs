//! Optional saved-workspace contracts, independent from editing and file access.
use serde_json::{Value, json};

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

    pub fn to_value(&self) -> Value {
        let target = match &self.target {
            WorkspaceTarget::Local => json!({"kind":"local"}),
            WorkspaceTarget::Ssh { host } => {
                json!({"kind":"ssh", "host":host})
            }
        };
        json!({"name":self.name,"target":target,"root":self.root})
    }

    pub fn from_value(value: &Value) -> Option<Self> {
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

pub fn default_name(root: &str) -> String {
    root.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(root)
        .to_owned()
}

use std::path::PathBuf;

#[derive(Clone, Debug)]
pub enum WorkspaceAction {
    OpenFolder,
    OpenLocalWorkspace(PathBuf),
    OpenWorkspace(WorkspaceSpec),
    OpenWorkspaceWindow(WorkspaceSpec),
    RemoveRecent(PathBuf),
    RemoveWorkspace(WorkspaceSpec),
    RenameWorkspace(WorkspaceSpec, String),
    Reconnect,
    SaveCurrent { name: Option<String> },
    Promote(PathBuf),
    Persist,
    Error(String),
}

/// The active location is usable independently of this optional attachment.
#[derive(Clone, Debug, Default, PartialEq, Eq, bevy::prelude::Resource)]
pub enum WorkspaceAttachment {
    #[default]
    Temporary,
    Saved(WorkspaceSpec),
}

impl WorkspaceAttachment {
    pub fn saved(&self) -> Option<&WorkspaceSpec> {
        match self {
            Self::Temporary => None,
            Self::Saved(spec) => Some(spec),
        }
    }
}

#[derive(Clone, Debug)]
pub struct WorkspaceSnapshot {
    pub layouts: serde_json::Value,
    pub plugins: serde_json::Value,
    /// Editing history is opaque to the selected repository.
    pub history: Option<serde_json::Value>,
}

impl Default for WorkspaceSnapshot {
    fn default() -> Self {
        Self {
            layouts: Value::Null,
            plugins: json!({}),
            history: None,
        }
    }
}

/// Applications choose where workspace definitions and snapshots are persisted.
/// A successful commit establishes the saved definition and snapshot. Updating
/// a separate recent-location index may fail afterwards without undoing that save.
pub trait WorkspaceRepository: Send + Sync + 'static {
    fn recent(&self) -> std::io::Result<Vec<WorkspaceSpec>>;
    fn find(&self, location: &WorkspaceSpec) -> std::io::Result<Option<WorkspaceSpec>>;
    fn load(&self, spec: &WorkspaceSpec) -> std::io::Result<Option<WorkspaceSnapshot>>;
    fn commit(
        &mut self,
        spec: WorkspaceSpec,
        snapshot: WorkspaceSnapshot,
    ) -> std::io::Result<WorkspaceSpec>;
    fn forget(&mut self, spec: &WorkspaceSpec) -> std::io::Result<()>;
    fn rename(&mut self, spec: &WorkspaceSpec, name: &str) -> std::io::Result<WorkspaceSpec>;
    fn take_index_error(&mut self) -> Option<String> {
        None
    }
}

#[derive(bevy::prelude::Resource)]
pub struct WorkspaceRepositoryResource(pub Box<dyn WorkspaceRepository>);

#[derive(Default)]
pub struct MemoryWorkspaceRepository {
    entries: std::collections::HashMap<String, (WorkspaceSpec, WorkspaceSnapshot)>,
    recent: Vec<String>,
}
impl WorkspaceRepository for MemoryWorkspaceRepository {
    fn recent(&self) -> std::io::Result<Vec<WorkspaceSpec>> {
        Ok(self
            .recent
            .iter()
            .filter_map(|id| self.entries.get(id).map(|(spec, _)| spec.clone()))
            .collect())
    }
    fn find(&self, location: &WorkspaceSpec) -> std::io::Result<Option<WorkspaceSpec>> {
        Ok(self
            .entries
            .get(&location.identity())
            .map(|(spec, _)| spec.clone()))
    }
    fn load(&self, spec: &WorkspaceSpec) -> std::io::Result<Option<WorkspaceSnapshot>> {
        Ok(self
            .entries
            .get(&spec.identity())
            .map(|(_, snapshot)| snapshot.clone()))
    }
    fn commit(
        &mut self,
        mut spec: WorkspaceSpec,
        snapshot: WorkspaceSnapshot,
    ) -> std::io::Result<WorkspaceSpec> {
        bedtime_storage::validate_snapshot(&snapshot.plugins)?;
        if spec.root.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Workspace directory is empty",
            ));
        }
        spec.name = spec.name.trim().to_owned();
        if spec.name.is_empty() {
            spec.name = default_name(&spec.root);
        }
        let identity = spec.identity();
        self.entries
            .insert(identity.clone(), (spec.clone(), snapshot));
        self.recent.retain(|id| id != &identity);
        self.recent.insert(0, identity);
        self.recent.truncate(20);
        Ok(spec)
    }
    fn forget(&mut self, spec: &WorkspaceSpec) -> std::io::Result<()> {
        self.recent.retain(|id| id != &spec.identity());
        Ok(())
    }
    fn rename(&mut self, spec: &WorkspaceSpec, name: &str) -> std::io::Result<WorkspaceSpec> {
        if name.trim().is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Workspace name cannot be empty",
            ));
        }
        let mut renamed = self.find(spec)?.ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "Workspace has not been saved")
        })?;
        renamed.name = name.trim().to_owned();
        if let Some((stored, _)) = self.entries.get_mut(&spec.identity()) {
            *stored = renamed.clone();
        }
        Ok(renamed)
    }
}
impl Default for WorkspaceRepositoryResource {
    fn default() -> Self {
        Self(Box::<MemoryWorkspaceRepository>::default())
    }
}

impl WorkspaceAction {
    pub fn to_value(&self) -> Value {
        match self {
            Self::OpenFolder => json!({"action":"open_folder"}),
            Self::OpenLocalWorkspace(path) => json!({"action":"open_local","path":path}),
            Self::OpenWorkspace(spec) => json!({"action":"open","spec":spec.to_value()}),
            Self::OpenWorkspaceWindow(spec) => json!({"action":"window","spec":spec.to_value()}),
            Self::RemoveRecent(path) => json!({"action":"remove_recent","path":path}),
            Self::RemoveWorkspace(spec) => json!({"action":"remove","spec":spec.to_value()}),
            Self::RenameWorkspace(spec, name) => {
                json!({"action":"rename","spec":spec.to_value(),"name":name})
            }
            Self::Reconnect => json!({"action":"reconnect"}),
            Self::SaveCurrent { name } => json!({"action":"save","name":name}),
            Self::Promote(path) => json!({"action":"promote","path":path}),
            Self::Persist => json!({"action":"persist"}),
            Self::Error(message) => json!({"action":"error","message":message}),
        }
    }
    pub fn from_value(value: &Value) -> Option<Self> {
        Some(match value["action"].as_str()? {
            "open_folder" => Self::OpenFolder,
            "open_local" => Self::OpenLocalWorkspace(value["path"].as_str()?.into()),
            "open" => Self::OpenWorkspace(WorkspaceSpec::from_value(&value["spec"])?),
            "window" => Self::OpenWorkspaceWindow(WorkspaceSpec::from_value(&value["spec"])?),
            "remove_recent" => Self::RemoveRecent(value["path"].as_str()?.into()),
            "remove" => Self::RemoveWorkspace(WorkspaceSpec::from_value(&value["spec"])?),
            "rename" => Self::RenameWorkspace(
                WorkspaceSpec::from_value(&value["spec"])?,
                value["name"].as_str()?.into(),
            ),
            "reconnect" => Self::Reconnect,
            "save" => Self::SaveCurrent {
                name: value["name"].as_str().map(str::to_owned),
            },
            "promote" => Self::Promote(value["path"].as_str()?.into()),
            "persist" => Self::Persist,
            "error" => Self::Error(value["message"].as_str()?.into()),
            _ => return None,
        })
    }
}
