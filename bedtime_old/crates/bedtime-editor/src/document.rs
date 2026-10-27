use crate::{BufferEdit, TextBuffer};
use bedtime_ui::egui;
use std::{collections::HashSet, fmt, ops::Range};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditorError(pub String);
impl fmt::Display for EditorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for EditorError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextEdit {
    pub range: Range<usize>,
    pub replacement: String,
}
impl TextEdit {
    pub fn between(before: &str, after: &str) -> Self {
        let prefix = before
            .chars()
            .zip(after.chars())
            .take_while(|(a, b)| a == b)
            .map(|(a, _)| a.len_utf8())
            .sum::<usize>();
        let suffix = before[prefix..]
            .chars()
            .rev()
            .zip(after[prefix..].chars().rev())
            .take_while(|(a, b)| a == b)
            .map(|(a, _)| a.len_utf8())
            .sum::<usize>();
        Self {
            range: prefix..before.len() - suffix,
            replacement: after[prefix..after.len() - suffix].into(),
        }
    }
}

/// Position transformation only. Inserted lengths keep journal readers cheap
/// even after a large paste or deletion.
#[derive(Clone, Debug)]
pub struct TextDelta {
    pub range: Range<usize>,
    pub inserted_bytes: usize,
}
impl TextDelta {
    pub(crate) fn map_position(&self, position: usize) -> usize {
        if position < self.range.start {
            position
        } else if position <= self.range.end {
            self.range.start + self.inserted_bytes
        } else {
            position - self.range.len() + self.inserted_bytes
        }
    }
}

impl From<&BufferEdit> for TextDelta {
    fn from(edit: &BufferEdit) -> Self {
        Self {
            range: edit.range(),
            inserted_bytes: edit.replacement().len(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct TextSnapshot {
    pub revision: u64,
    pub text: TextBuffer,
}

#[derive(Clone, Debug)]
pub struct TextCommit {
    pub revision: u64,
    pub changes: Option<Vec<TextDelta>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HistoryAction {
    Undo,
    Redo,
}

/// Canonical text/history boundary. Reading clones a persistent root, never the
/// document bytes. Edits use UTF-8 byte ranges; journal entries use successive
/// states. A missing journal requires refreshing derived positions.
pub trait TextSession {
    fn snapshot(&self) -> Result<TextSnapshot, EditorError>;
    fn changes_since(&self, revision: u64) -> Result<Option<Vec<TextCommit>>, EditorError>;
    fn edit(&mut self, revision: u64, change: TextEdit) -> Result<u64, EditorError> {
        self.edit_buffer(revision, BufferEdit::new(change.range, change.replacement))
    }
    /// Accept a worker-built replacement without flattening it on the UI thread.
    fn edit_buffer(&mut self, revision: u64, change: BufferEdit) -> Result<u64, EditorError>;
    fn history(&mut self, action: HistoryAction) -> Result<u64, EditorError>;
    fn end_undo_group(&mut self);
    fn set_composing(&mut self, view: egui::Id, composing: bool);
    /// The host may already bind one session adapter to a viewer. Standalone
    /// shared documents use this to keep undo groups owned by the editing view.
    fn set_edit_owner(&mut self, _view: egui::Id) {}
}

/// In-memory document for hosts without Workbench. Several views borrow this
/// same owner; snapshots and undo roots share immutable rope nodes.
pub struct MemoryTextSession {
    history: bedtime_document::History<TextBuffer, u64>,
    group: u64,
    composing: HashSet<egui::Id>,
    edit_owner: Option<egui::Id>,
}

impl MemoryTextSession {
    pub fn new(text: impl Into<TextBuffer>) -> Self {
        Self {
            history: bedtime_document::History::new(text.into(), 100),
            group: 0,
            composing: HashSet::new(),
            edit_owner: None,
        }
    }
    pub fn text(&self) -> &TextBuffer {
        self.history.content()
    }
}
impl TextSession for MemoryTextSession {
    fn snapshot(&self) -> Result<TextSnapshot, EditorError> {
        Ok(TextSnapshot {
            revision: self.history.revision(),
            text: self.history.content().clone(),
        })
    }
    fn changes_since(&self, revision: u64) -> Result<Option<Vec<TextCommit>>, EditorError> {
        if revision > self.history.revision() {
            return Err(EditorError("Future document revision".into()));
        }
        Ok(self.history.changes_since(revision).map(|commits| {
            commits
                .into_iter()
                .map(|c| TextCommit {
                    revision: c.revision,
                    changes: c
                        .changes
                        .map(|changes| changes.iter().map(TextDelta::from).collect()),
                })
                .collect()
        }))
    }
    fn edit_buffer(&mut self, revision: u64, change: BufferEdit) -> Result<u64, EditorError> {
        if revision != self.history.revision() {
            return Err(EditorError("Stale document revision".into()));
        }
        if !self.composing.is_empty() {
            return Err(EditorError("Finish composition before editing".into()));
        }
        self.history
            .edit([change], "Typing".into(), Some(self.group))
            .map_err(EditorError)
    }
    fn history(&mut self, action: HistoryAction) -> Result<u64, EditorError> {
        if !self.composing.is_empty() {
            return Err(EditorError("Finish composition before undo/redo".into()));
        }
        self.group += 1;
        Ok(self
            .history
            .undo(action == HistoryAction::Redo)
            .map_err(EditorError)?
            .unwrap_or(self.history.revision()))
    }
    fn end_undo_group(&mut self) {
        self.group += 1;
    }

    fn set_composing(&mut self, view: egui::Id, composing: bool) {
        if composing {
            self.composing.insert(view);
        } else {
            self.composing.remove(&view);
        }
    }

    fn set_edit_owner(&mut self, view: egui::Id) {
        if self.edit_owner != Some(view) {
            self.group += 1;
            self.edit_owner = Some(view);
        }
    }
}
