//! Requests that change code or find it: code actions, rename, references,
//! formatting. Their answers are parsed here (on the worker); edits keep the
//! server's coordinates, converted where they're applied, against the text
//! there.
use super::{Encoding, FileUri, Location, ProtocolRange};
use serde_json::Value;
use std::{ops::Range, sync::Arc};

/// A request about a document, at the revision of its `RequestOrigin`.
#[derive(Clone, Debug)]
pub enum Ask {
    /// What can be done at `range` (quick fixes, refactorings).
    CodeActions { range: Range<usize> },
    /// Fill in a code action's edit (servers send it lazily).
    ResolveCodeAction(Arc<Value>),
    /// Run a server command (from a code action).
    Command(Arc<Value>),
    /// Rename the symbol at `byte` everywhere.
    Rename { byte: usize, name: String },
    /// Every use of the symbol at `byte`.
    References { byte: usize },
    /// Format the whole document.
    Format { tab_size: u32, spaces: bool },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AskKind {
    CodeActions,
    ResolveCodeAction,
    Command,
    Rename,
    References,
    Format,
}

impl Ask {
    pub fn kind(&self) -> AskKind {
        match self {
            Self::CodeActions { .. } => AskKind::CodeActions,
            Self::ResolveCodeAction(_) => AskKind::ResolveCodeAction,
            Self::Command(_) => AskKind::Command,
            Self::Rename { .. } => AskKind::Rename,
            Self::References { .. } => AskKind::References,
            Self::Format { .. } => AskKind::Format,
        }
    }
}

#[derive(Clone, Debug)]
pub enum Answer {
    CodeActions(Vec<CodeAction>),
    CodeAction(CodeAction),
    Edit(WorkspaceEdit),
    References(Vec<Location>),
    /// A command ran (its edits, if any, come as `Event::ApplyEdit`).
    Done,
}

/// Something the server offers to do. Either it has an edit, or a command to
/// run, or (unresolved) neither yet.
#[derive(Clone, Debug)]
pub struct CodeAction {
    pub title: String,
    pub kind: Option<String>,
    /// Why it can't be used here, if it can't.
    pub disabled: Option<String>,
    pub raw: Arc<Value>,
}

impl CodeAction {
    fn parse(value: &Value) -> Result<Self, String> {
        let title = value["title"]
            .as_str()
            .ok_or("A code action has no title")?
            .chars()
            .take(512)
            .collect();
        // A bare Command is an action that runs it.
        let raw = if value["command"].is_string() {
            serde_json::json!({ "title": value["title"], "command": value })
        } else {
            value.clone()
        };
        Ok(Self {
            title,
            kind: value["kind"].as_str().map(str::to_owned),
            disabled: value["disabled"]["reason"].as_str().map(str::to_owned),
            raw: Arc::new(raw),
        })
    }

    /// Its edit, if it has one.
    pub fn edit(&self, encoding: Encoding) -> Option<Result<WorkspaceEdit, String>> {
        let edit = self.raw.get("edit").filter(|edit| !edit.is_null())?;
        Some(parse_workspace_edit(edit, encoding))
    }

    /// Its command, if it has one.
    pub fn command(&self) -> Option<Arc<Value>> {
        let command = self.raw.get("command").filter(|c| c.is_object())?;
        Some(Arc::new(command.clone()))
    }

    /// Whether it needs resolving before it can be applied.
    pub fn unresolved(&self) -> bool {
        self.raw.get("edit").is_none_or(Value::is_null) && self.command().is_none()
    }
}

/// Edits to files, in the server's coordinates.
#[derive(Clone, Debug, Default)]
pub struct WorkspaceEdit {
    pub files: Vec<FileEdit>,
    pub encoding: Encoding,
}

#[derive(Clone, Debug)]
pub struct FileEdit {
    pub uri: FileUri,
    pub edits: Vec<(ProtocolRange, String)>,
}

pub(crate) fn parse_code_actions(value: &Value) -> Result<Vec<CodeAction>, String> {
    if value.is_null() {
        return Ok(Vec::new());
    }
    let values = value.as_array().ok_or("Invalid code actions")?;
    values.iter().take(256).map(CodeAction::parse).collect()
}

pub(crate) fn parse_code_action(value: &Value) -> Result<CodeAction, String> {
    CodeAction::parse(value)
}

pub(crate) fn parse_text_edits(value: &Value) -> Result<Vec<(ProtocolRange, String)>, String> {
    if value.is_null() {
        return Ok(Vec::new());
    }
    let edits = value.as_array().ok_or("Invalid text edits")?;
    if edits.len() > 100_000 {
        return Err("Too many edits".into());
    }
    edits
        .iter()
        .map(|edit| {
            let range = serde_json::from_value(edit["range"].clone())
                .map_err(|_| "Invalid edit range".to_owned())?;
            let text = edit["newText"].as_str().ok_or("Invalid edit text")?;
            Ok((range, text.to_owned()))
        })
        .collect()
}

/// `changes` or `documentChanges` (text edits only: creating, renaming and
/// deleting files is refused).
pub fn parse_workspace_edit(value: &Value, encoding: Encoding) -> Result<WorkspaceEdit, String> {
    let mut files: Vec<FileEdit> = Vec::new();
    if let Some(changes) = value["documentChanges"].as_array() {
        for change in changes {
            if change.get("kind").is_some() {
                return Err("Creating, renaming or deleting files isn't supported".into());
            }
            let uri = FileUri::parse(
                change["textDocument"]["uri"]
                    .as_str()
                    .ok_or("Invalid edited document")?,
            )?;
            files.push(FileEdit {
                uri,
                edits: parse_text_edits(&change["edits"])?,
            });
        }
    } else if let Some(changes) = value["changes"].as_object() {
        for (uri, edits) in changes {
            files.push(FileEdit {
                uri: FileUri::parse(uri)?,
                edits: parse_text_edits(edits)?,
            });
        }
    }
    if files.len() > 4096 {
        return Err("The edit touches too many files".into());
    }
    Ok(WorkspaceEdit { files, encoding })
}

/// Reference locations (up to 10,000).
pub(crate) fn parse_references(value: &Value, encoding: Encoding) -> Result<Vec<Location>, String> {
    if value.is_null() {
        return Ok(Vec::new());
    }
    let values = value.as_array().ok_or("Invalid references")?;
    values
        .iter()
        .take(10_000)
        .map(|v| {
            Ok(Location {
                uri: FileUri::parse(v["uri"].as_str().ok_or("Missing reference URI")?)?,
                range: serde_json::from_value(v["range"].clone())
                    .map_err(|_| "Invalid reference range")?,
                encoding,
            })
        })
        .collect()
}

/// Whether two protocol ranges overlap (or touch).
pub(crate) fn overlaps(a: &ProtocolRange, b: &ProtocolRange) -> bool {
    let key = |p: &super::Position| (p.line, p.character);
    key(&a.start) <= key(&b.end) && key(&b.start) <= key(&a.end)
}
