//! Translated from ned util/project_undo.{h,cpp}; see LICENSE and NOTICE.
//! One project store owns per-file history; document mutation stays in commands.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

use crate::editor_operations::{OpKind, TextOp};

const UNDO_FILE_NAME: &str = ".undo-redo-bed.json";
const MAX_STACK: usize = 50;
const COALESCE: Duration = Duration::from_millis(300);
const DISK_SAVE_INTERVAL: Duration = Duration::from_millis(3000);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SelectionSnapshot {
    pub head_row: i32,
    pub head_column: i32,
    pub anchor_row: i32,
    pub anchor_column: i32,
    pub preferred_column: i32,
}

impl SelectionSnapshot {
    fn collapsed(row: i32, column: i32) -> Self {
        Self {
            head_row: row,
            anchor_row: row,
            head_column: column,
            anchor_column: column,
            ..Self::default()
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HistoryStep {
    pub op: TextOp,
    pub deleted_text: Vec<u8>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HistoryEdit {
    pub steps: Vec<HistoryStep>,
    pub selections_before: Vec<SelectionSnapshot>,
    pub selections_after: Vec<SelectionSnapshot>,
    pub primary_before: usize,
    pub primary_after: usize,
}

#[derive(Default)]
struct FileStack {
    undo_stack: Vec<HistoryEdit>,
    redo_stack: Vec<HistoryEdit>,
    pending: Option<HistoryEdit>,
    last_add: Option<Instant>,
}

impl FileStack {
    fn try_coalesce(&mut self, edit: &HistoryEdit, now: Instant) -> bool {
        let Some(pending) = &mut self.pending else {
            return false;
        };
        if pending.steps.len() != 1
            || edit.steps.len() != 1
            || pending.selections_after.len() != 1
            || edit.selections_after.len() != 1
        {
            return false;
        }
        let prev = &mut pending.steps[0].op;
        let next = &edit.steps[0].op;
        if prev.kind != OpKind::Insert
            || next.kind != OpKind::Insert
            || next.row != prev.row
            || next.column != prev.column + prev.text.len() as i32
        {
            return false;
        }
        prev.text.extend_from_slice(&next.text);
        pending.selections_after.clone_from(&edit.selections_after);
        pending.primary_after = edit.primary_after;
        self.last_add = Some(now);
        true
    }

    fn record(&mut self, edit: HistoryEdit, now: Instant) {
        if edit.steps.is_empty() {
            return;
        }
        self.redo_stack.clear();
        if self.pending.is_some()
            && self
                .last_add
                .is_some_and(|last| now.duration_since(last) >= COALESCE)
        {
            self.commit_pending();
        }
        if self.try_coalesce(&edit, now) {
            return;
        }
        self.commit_pending();
        self.pending = Some(edit);
        self.last_add = Some(now);
    }

    fn commit_pending(&mut self) {
        if let Some(pending) = self.pending.take() {
            self.undo_stack.push(pending);
            if self.undo_stack.len() > MAX_STACK {
                self.undo_stack.remove(0);
            }
        }
    }

    fn undo(&mut self) -> Option<HistoryEdit> {
        self.commit_pending();
        let edit = self.undo_stack.pop()?;
        self.redo_stack.push(edit.clone());
        Some(edit)
    }

    fn redo(&mut self) -> Option<HistoryEdit> {
        self.commit_pending();
        let edit = self.redo_stack.pop()?;
        self.undo_stack.push(edit.clone());
        Some(edit)
    }

    fn update_pending_final_cursor(&mut self, row: i32, column: i32) {
        let Some(edit) = &mut self.pending else {
            return;
        };
        if edit.selections_after.is_empty() {
            return;
        }
        let index = if edit.primary_after < edit.selections_after.len() {
            edit.primary_after
        } else {
            0
        };
        edit.selections_after[index].head_row = row;
        edit.selections_after[index].head_column = column;
    }

    fn has_operations(&self) -> bool {
        self.pending.is_some() || !self.undo_stack.is_empty() || !self.redo_stack.is_empty()
    }

    fn to_json(&self) -> Result<Value, std::str::Utf8Error> {
        let mut undo = Vec::new();
        if let Some(edit) = &self.pending {
            undo.push(edit_to_json(edit)?);
        }
        for edit in &self.undo_stack[self.undo_stack.len().saturating_sub(50)..] {
            undo.push(edit_to_json(edit)?);
        }
        let redo = self.redo_stack[self.redo_stack.len().saturating_sub(20)..]
            .iter()
            .map(edit_to_json)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(json!({ "version": 3, "undoStack": undo, "redoStack": redo }))
    }

    fn from_json(value: &Value) -> Self {
        let Some(version) = value.get("version").and_then(Value::as_i64) else {
            return Self::default();
        };
        if version < 2 {
            return Self::default();
        }
        // nlohmann::json::value throws for present fields of the wrong type;
        // upstream discards the entire file stack when that happens.
        if !valid_stack_fields(value, version) {
            return Self::default();
        }
        let load = |key: &str| {
            value
                .get(key)
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter(|item| item.is_object())
                        .map(|item| edit_from_json(item, version))
                        .filter(|edit| !edit.steps.is_empty())
                        .collect()
                })
                .unwrap_or_default()
        };
        Self {
            undo_stack: load("undoStack"),
            redo_stack: load("redoStack"),
            ..Self::default()
        }
    }
}

#[derive(Default)]
pub struct ProjectUndo {
    pub project_root: String,
    stacks: BTreeMap<String, FileStack>,
    reset_paths: BTreeSet<String>,
    dirty: bool,
    last_disk_save: Option<Instant>,
}

impl ProjectUndo {
    pub fn new(project_root: String) -> Self {
        Self {
            project_root,
            ..Self::default()
        }
    }

    fn stack_for(&mut self, path: &str) -> Option<&mut FileStack> {
        if path.is_empty() {
            return None;
        }
        Some(self.stacks.entry(path.to_owned()).or_default())
    }

    pub fn ensure_file(&mut self, path: &str) {
        if path.is_empty() {
            return;
        }
        if let Some(stack) = self.stack_for(path) {
            stack.commit_pending();
        }
        self.maybe_save_to_disk();
    }

    /// A view boundary seals typing without changing the document or writing disk.
    pub fn seal_file(&mut self, path: &str) {
        if let Some(stack) = self.stack_for(path) {
            stack.commit_pending();
        }
    }

    /// Rebind a live document's history after a successful filesystem rename.
    /// Operations contain document positions, so their contents remain valid.
    pub fn rekey_file(&mut self, old: &str, new: &str) -> std::io::Result<()> {
        if old == new {
            return Ok(());
        }
        if let Some(mut stack) = self.stacks.remove(old) {
            stack.commit_pending();
            self.stacks.insert(new.to_owned(), stack);
            self.reset_paths.insert(old.to_owned());
            self.dirty = true;
        }
        Ok(())
    }

    /// External replacement invalidates operations whose byte positions refer to
    /// the old document. Persist an empty stack so a restart cannot restore it.
    pub fn forget_file(&mut self, path: &str) {
        if path.is_empty() {
            return;
        }
        self.stacks.insert(path.to_owned(), FileStack::default());
        self.reset_paths.insert(path.to_owned());
        self.dirty = true;
        self.maybe_save_to_disk();
    }
    /// Byte edits invalidate earlier text operations, but do not create a
    /// persistent history record for a file that has only been viewed as bytes.
    pub fn forget_existing_file(&mut self, path: &str) {
        if self.stacks.get(path).is_some_and(FileStack::has_operations) {
            self.forget_file(path);
        }
    }
    pub fn remove_memory_file(&mut self, path: &str) {
        self.stacks.remove(path);
        self.reset_paths.remove(path);
    }

    pub fn record(&mut self, path: &str, mut edit: HistoryEdit) {
        if path.is_empty() || edit.steps.is_empty() {
            return;
        }
        if edit.selections_before.is_empty() {
            edit.selections_before.push(SelectionSnapshot::default());
        }
        if edit.selections_after.is_empty() {
            edit.selections_after.push(SelectionSnapshot::default());
        }
        self.stack_for(path).unwrap().record(edit, Instant::now());
        self.dirty = true;
        self.maybe_save_to_disk();
    }

    pub fn update_pending_cursor(&mut self, path: &str, row: i32, column: i32) {
        if let Some(stack) = self.stack_for(path) {
            stack.update_pending_final_cursor(row, column);
        }
    }

    pub fn undo(&mut self, path: &str) -> Option<HistoryEdit> {
        let edit = self.stack_for(path)?.undo()?;
        self.dirty = true;
        self.maybe_save_to_disk();
        Some(edit)
    }

    pub fn redo(&mut self, path: &str) -> Option<HistoryEdit> {
        let edit = self.stack_for(path)?.redo()?;
        self.dirty = true;
        self.maybe_save_to_disk();
        Some(edit)
    }

    pub fn flush(&mut self) {
        if self.dirty && !self.project_root.is_empty() {
            self.save_project();
        }
    }

    fn maybe_save_to_disk(&mut self) {
        if !self.dirty || self.project_root.is_empty() {
            return;
        }
        let now = Instant::now();
        if self
            .last_disk_save
            .is_some_and(|last| now.duration_since(last) < DISK_SAVE_INTERVAL)
        {
            return;
        }
        self.save_project();
        self.last_disk_save = Some(now);
    }

    /// Explicit session persistence; commands keep project_root empty.
    pub fn flush_to(&mut self, root: &Path) -> std::io::Result<()> {
        if !self.dirty {
            return Ok(());
        }
        let mut files = serde_json::Map::new();
        for (key, stack) in &mut self.stacks {
            stack.commit_pending();
            if key.starts_with("bed:untitled:") {
                continue;
            }
            if stack.has_operations() || self.reset_paths.contains(key) {
                files.insert(key.clone(), stack.to_json().map_err(std::io::Error::other)?);
            }
        }
        if !files.is_empty() {
            let bytes = serde_json::to_vec_pretty(&json!({"files":files}))
                .map_err(std::io::Error::other)?;
            fs::write(root.join(UNDO_FILE_NAME), bytes)?;
        }
        self.dirty = false;
        self.reset_paths.clear();
        Ok(())
    }

    pub fn merge_project(&mut self, root: &str) {
        let mut loaded = Self::default();
        loaded.load_project(root);
        for (path, stack) in loaded.stacks {
            self.stacks.entry(path).or_insert(stack);
        }
    }

    fn save_project(&mut self) {
        let path = Path::new(&self.project_root).join(UNDO_FILE_NAME);
        let mut files = serde_json::Map::new();
        for (key, stack) in &mut self.stacks {
            stack.commit_pending();
            if !stack.has_operations() && !self.reset_paths.contains(key) {
                continue;
            }
            match stack.to_json() {
                Ok(value) => {
                    files.insert(key.clone(), value);
                }
                Err(error) => eprintln!("Error serializing undo for {key}: {error}"),
            }
        }
        if !files.is_empty() {
            let value = json!({"files": files});
            match serde_json::to_vec_pretty(&value)
                .map_err(std::io::Error::other)
                .and_then(|bytes| fs::write(path, bytes))
            {
                Ok(()) => {}
                Err(error) => {
                    eprintln!("Error saving undo state: {error}");
                    return;
                }
            }
        }
        self.dirty = false;
        self.reset_paths.clear();
    }

    pub fn load_project(&mut self, folder: &str) {
        if folder.is_empty() {
            return;
        }
        self.stacks.clear();
        self.reset_paths.clear();
        self.dirty = false;
        let path = Path::new(folder).join(UNDO_FILE_NAME);
        let Ok(bytes) = fs::read(&path) else {
            return;
        };
        match serde_json::from_slice::<Value>(&bytes) {
            Ok(root) => {
                if let Some(files) = root.get("files").and_then(Value::as_object) {
                    for (key, value) in files {
                        self.stacks.insert(key.clone(), FileStack::from_json(value));
                    }
                }
            }
            Err(error) => {
                eprintln!("Error loading undo state: {error}");
                let _ = fs::remove_file(path);
                self.stacks.clear();
            }
        }
    }
}

fn int(value: &Value, key: &str, fallback: i32) -> i32 {
    value
        .get(key)
        .and_then(Value::as_i64)
        .and_then(|i| i32::try_from(i).ok())
        .unwrap_or(fallback)
}

fn valid_int_fields(value: &Value, keys: &[&str]) -> bool {
    keys.iter().all(|key| {
        value.get(key).is_none_or(|field| {
            field
                .as_i64()
                .and_then(|number| i32::try_from(number).ok())
                .is_some()
        })
    })
}

fn valid_step_fields(value: &Value) -> bool {
    valid_int_fields(value, &["row", "column", "length"])
        && ["kind", "text", "deletedText"]
            .iter()
            .all(|key| value.get(key).is_none_or(Value::is_string))
}

fn valid_stack_fields(value: &Value, version: i64) -> bool {
    ["undoStack", "redoStack"].iter().all(|key| {
        value
            .get(key)
            .and_then(Value::as_array)
            .is_none_or(|items| {
                items.iter().filter(|v| v.is_object()).all(|item| {
                    if version >= 3 && item.get("steps").is_some() {
                        let steps_ok = match item.get("steps").and_then(Value::as_array) {
                            Some(steps) => steps
                                .iter()
                                .filter(|v| v.is_object())
                                .all(valid_step_fields),
                            None => valid_step_fields(item),
                        };
                        steps_ok
                            && valid_int_fields(item, &["primaryBefore", "primaryAfter"])
                            && ["selectionsBefore", "selectionsAfter"].iter().all(|key| {
                                item.get(key).and_then(Value::as_array).is_none_or(|snaps| {
                                    snaps.iter().filter(|v| v.is_object()).all(|snap| {
                                        valid_int_fields(
                                            snap,
                                            &[
                                                "headRow",
                                                "headColumn",
                                                "anchorRow",
                                                "anchorColumn",
                                                "preferredColumn",
                                            ],
                                        )
                                    })
                                })
                            })
                    } else {
                        valid_step_fields(item)
                            && valid_int_fields(
                                item,
                                &[
                                    "cursorBeforeRow",
                                    "cursorBeforeColumn",
                                    "cursorAfterRow",
                                    "cursorAfterColumn",
                                ],
                            )
                    }
                })
            })
    })
}

fn bytes(value: &Value, key: &str) -> Vec<u8> {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .as_bytes()
        .to_vec()
}

fn step_to_json(step: &HistoryStep) -> Result<Value, std::str::Utf8Error> {
    Ok(json!({
        "kind": if step.op.kind == OpKind::Insert { "insert" } else { "delete" },
        "row": step.op.row, "column": step.op.column,
        "text": std::str::from_utf8(&step.op.text)?, "length": step.op.length,
        "deletedText": std::str::from_utf8(&step.deleted_text)?
    }))
}

fn step_from_json(value: &Value) -> HistoryStep {
    HistoryStep {
        op: TextOp {
            kind: if value.get("kind").and_then(Value::as_str) == Some("delete") {
                OpKind::Delete
            } else {
                OpKind::Insert
            },
            row: int(value, "row", 0),
            column: int(value, "column", 0),
            text: bytes(value, "text"),
            length: int(value, "length", 0),
        },
        deleted_text: bytes(value, "deletedText"),
    }
}

fn snap_to_json(snap: &SelectionSnapshot) -> Value {
    json!({ "headRow": snap.head_row, "headColumn": snap.head_column, "anchorRow": snap.anchor_row,
        "anchorColumn": snap.anchor_column, "preferredColumn": snap.preferred_column })
}

fn snap_from_json(value: &Value) -> SelectionSnapshot {
    let head_row = int(value, "headRow", 0);
    let head_column = int(value, "headColumn", 0);
    SelectionSnapshot {
        head_row,
        head_column,
        anchor_row: int(value, "anchorRow", head_row),
        anchor_column: int(value, "anchorColumn", head_column),
        preferred_column: int(value, "preferredColumn", 0),
    }
}

fn edit_to_json(edit: &HistoryEdit) -> Result<Value, std::str::Utf8Error> {
    let steps = edit
        .steps
        .iter()
        .map(step_to_json)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(json!({ "steps": steps,
        "selectionsBefore": edit.selections_before.iter().map(snap_to_json).collect::<Vec<_>>(),
        "selectionsAfter": edit.selections_after.iter().map(snap_to_json).collect::<Vec<_>>(),
        "primaryBefore": edit.primary_before, "primaryAfter": edit.primary_after }))
}

fn edit_from_json(value: &Value, version: i64) -> HistoryEdit {
    if version >= 3 && value.get("steps").is_some() {
        let steps = match value.get("steps").and_then(Value::as_array) {
            Some(items) => items
                .iter()
                .filter(|item| item.is_object())
                .map(step_from_json)
                .collect(),
            None => vec![step_from_json(value)],
        };
        let snaps = |key: &str| {
            let result: Vec<_> = value
                .get(key)
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter(|item| item.is_object())
                        .map(snap_from_json)
                        .collect()
                })
                .unwrap_or_default();
            if result.is_empty() {
                vec![SelectionSnapshot::default()]
            } else {
                result
            }
        };
        HistoryEdit {
            steps,
            selections_before: snaps("selectionsBefore"),
            selections_after: snaps("selectionsAfter"),
            primary_before: int(value, "primaryBefore", 0).max(0) as usize,
            primary_after: int(value, "primaryAfter", 0).max(0) as usize,
        }
    } else {
        HistoryEdit {
            steps: vec![step_from_json(value)],
            selections_before: vec![SelectionSnapshot::collapsed(
                int(value, "cursorBeforeRow", 0),
                int(value, "cursorBeforeColumn", 0),
            )],
            selections_after: vec![SelectionSnapshot::collapsed(
                int(value, "cursorAfterRow", 0),
                int(value, "cursorAfterColumn", 0),
            )],
            ..HistoryEdit::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn insert(column: i32, text: &[u8]) -> HistoryEdit {
        HistoryEdit {
            steps: vec![HistoryStep {
                op: TextOp {
                    kind: OpKind::Insert,
                    row: 0,
                    column,
                    text: text.to_vec(),
                    length: 0,
                },
                deleted_text: vec![],
            }],
            selections_before: vec![SelectionSnapshot::collapsed(0, column)],
            selections_after: vec![SelectionSnapshot::collapsed(0, column + text.len() as i32)],
            ..HistoryEdit::default()
        }
    }

    #[test]
    fn adjacent_inserts_coalesce_until_300ms() {
        let now = Instant::now();
        let mut stack = FileStack::default();
        stack.record(insert(0, b"a"), now);
        stack.record(insert(1, b"b"), now + Duration::from_millis(299));
        stack.record(insert(2, b"c"), now + Duration::from_millis(600));
        assert_eq!(stack.undo().unwrap().steps[0].op.text, b"c");
        let edit = stack.undo().unwrap();
        assert_eq!(edit.steps[0].op.text, b"ab");
        assert_eq!(edit.selections_after[0].head_column, 2);
        assert!(stack.undo().is_none());
        stack.record(insert(0, b"z"), now + Duration::from_millis(601));
        assert!(stack.redo().is_none());
    }

    #[test]
    fn per_file_history_and_version_3_round_trip() {
        let mut undo = ProjectUndo::default();
        undo.record("a", insert(0, b"A"));
        undo.record("b", insert(0, b"B"));
        assert_eq!(undo.undo("b").unwrap().steps[0].op.text, b"B");
        assert_eq!(undo.undo("a").unwrap().steps[0].op.text, b"A");
        let stack = &undo.stacks["a"];
        let mut restored = FileStack::from_json(&stack.to_json().unwrap());
        assert_eq!(restored.redo().unwrap().steps[0].op.text, b"A");
    }

    #[test]
    fn loads_upstream_version_2_shape() {
        let value = json!({"version":2,"undoStack":[{"kind":"delete","row":3,"column":4,"length":2,
            "deletedText":"xy","cursorBeforeRow":3,"cursorBeforeColumn":6,"cursorAfterRow":3,"cursorAfterColumn":4}]});
        let edit = FileStack::from_json(&value).undo().unwrap();
        assert_eq!(edit.steps[0].op.kind, OpKind::Delete);
        assert_eq!(edit.steps[0].deleted_text, b"xy");
        assert_eq!(edit.selections_before[0].head_column, 6);
        assert_eq!(edit.selections_after[0].head_column, 4);
    }

    #[test]
    fn malformed_present_fields_discard_the_stack_as_upstream_does() {
        let value = json!({"version":3,"undoStack":[
            {"steps":[{"kind":"insert","text":"good","row":0}]},
            {"steps":[{"kind":"insert","text":"bad","row":"not a number"}]}
        ]});
        assert!(FileStack::from_json(&value).undo().is_none());
    }

    #[test]
    fn limits_history_to_50_groups_and_does_not_coalesce_multiple_carets() {
        let mut stack = FileStack::default();
        let now = Instant::now();
        for i in 0..60 {
            let mut edit = insert(i, b"x");
            edit.selections_after
                .push(SelectionSnapshot::collapsed(1, i));
            stack.record(edit, now);
        }
        stack.commit_pending();
        assert_eq!(stack.undo_stack.len(), 50);
        assert_eq!(stack.undo_stack[0].steps[0].op.column, 10);
    }
}
