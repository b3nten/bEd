//! Workspace identity shared by the shell and project features.
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
