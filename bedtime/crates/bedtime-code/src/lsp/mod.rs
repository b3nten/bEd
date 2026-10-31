//! Language servers: local stdio servers kept in sync with the documents
//! editors show. The backend (one worker per server) owns the process,
//! protocol state and I/O; `documents` connects it to workbench documents,
//! diagnostics in the editor and go-to-definition.
mod activity;
mod intelligence;
pub use intelligence::*;
#[cfg(unix)]
mod backend;
mod protocol;
pub use activity::{
    MessageKind, ProgressToken, ProjectStatus, ServerActivity, ServerMessage, WorkProgress,
};
#[cfg(unix)]
mod transport;
#[cfg(unix)]
pub use backend::*;
pub use protocol::{Encoding, FileUri, Position, ProtocolRange};
#[cfg(unix)]
pub(crate) mod assist;
#[cfg(unix)]
mod documentation;
#[cfg(unix)]
mod documents;
#[cfg(unix)]
pub use documents::*;

use crate::editor::Diagnostics;
use bedtime_document::{BufferEdit, TextBuffer};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

pub const MAX_SOURCE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

/// Application-owned launch policy. Arguments are literal, never shell input.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerConfig {
    pub program: String,
    pub arguments: Vec<String>,
    pub environment: BTreeMap<String, String>,
    pub settings: serde_json::Value,
    pub initialization_options: serde_json::Value,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            program: "rust-analyzer".into(),
            arguments: Vec::new(),
            environment: BTreeMap::new(),
            settings: serde_json::json!({}),
            initialization_options: serde_json::Value::Null,
        }
    }
}

impl ServerConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.program.trim().is_empty()
            || self.program.len() > 4096
            || self.program.contains('\0')
        {
            return Err("Server program must be a nonempty path or executable name".into());
        }
        if self.arguments.len() > 128
            || self
                .arguments
                .iter()
                .any(|a| a.len() > 16384 || a.contains('\0'))
            || self.environment.len() > 128
            || self.environment.iter().any(|(k, v)| {
                k.is_empty()
                    || k.contains(['=', '\0'])
                    || v.contains('\0')
                    || k.len() + v.len() > 16384
            })
        {
            return Err("Invalid language server arguments or environment".into());
        }
        if serde_json::to_vec(&(&self.settings, &self.initialization_options))
            .map_err(|e| e.to_string())?
            .len()
            > 256 * 1024
        {
            return Err("Language server settings exceed 256 KiB".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct Launch {
    pub config: ServerConfig,
    pub root: PathBuf,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DocumentKey {
    pub id: u64,
    pub generation: u64,
}
/// Changes are relative to `base_revision`, and use successive pre-edit states.
/// None explicitly requests a complete refresh. TextBuffer clones share roots.
pub struct DocumentUpdate {
    pub key: DocumentKey,
    pub uri: FileUri,
    pub language: String,
    pub revision: u64,
    pub text: TextBuffer,
    pub changes: Option<(u64, Vec<BufferEdit>)>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RequestOrigin {
    pub document: DocumentKey,
    pub revision: u64,
    pub panel: u64,
    pub ticket: u64,
}
#[derive(Clone, Debug)]
pub struct Location {
    pub uri: FileUri,
    pub range: ProtocolRange,
    pub encoding: Encoding,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerStatus {
    Starting,
    Ready,
    Failed(Arc<str>),
    Stopped,
}
#[derive(Debug)]
pub enum Event {
    Diagnostics {
        document: DocumentKey,
        report: Arc<Diagnostics>,
    },
    Definition {
        origin: RequestOrigin,
        result: Result<Vec<Location>, String>,
    },
    Intelligence {
        origin: RequestOrigin,
        kind: QueryKind,
        result: Result<Intelligence, String>,
    },
    DocumentError {
        document: DocumentKey,
        message: String,
    },
}
