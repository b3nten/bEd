//! Source breakpoints and launch snapshots shared by debugger editor contributions.
use bed_document_session::editor_session::{DocumentId, DocumentKind, EditorSession, SessionEvent};
use bed_editing::editor_events::DocumentChange;
use bed_editor_ui::{BreakpointStatus, SourceBreakpoint, SourceDebugPresentation};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs, io,
};

#[derive(Clone, Debug)]
pub struct Breakpoint {
    pub id: u64,
    pub row: i32,
    /// UTF-16 column used only to follow edits; breakpoints remain line based.
    pub column: i32,
    pub enabled: bool,
    pub status: BreakpointStatus,
    pub message: String,
    /// Frozen compiled-source location, independent of edits to the buffer.
    pub bound_row: Option<i32>,
}

#[derive(Default)]
pub struct SourceDebugState {
    pub breakpoints: BTreeMap<String, Vec<Breakpoint>>,
    next_id: u64,
    running: bool,
    baseline: BTreeMap<String, Vec<u8>>,
    current: BTreeMap<String, Vec<u8>>,
    paths: HashMap<DocumentId, String>,
    renamed: BTreeSet<String>,
}

impl SourceDebugState {
    pub fn toggle(&mut self, path: &str, row: i32, column: i32) {
        if let Some(id) = self
            .breakpoints
            .get(path)
            .and_then(|items| items.iter().find(|item| item.row == row))
            .map(|item| item.id)
        {
            self.remove(path, id);
            return;
        }
        self.next_id += 1;
        let bound_row = (self.running && !self.changed(path)).then_some(row);
        self.breakpoints
            .entry(path.to_owned())
            .or_default()
            .push(Breakpoint {
                id: self.next_id,
                row: row.max(0),
                column: column.max(0),
                enabled: true,
                status: BreakpointStatus::Pending,
                message: String::new(),
                bound_row,
            });
        self.normalize(path);
    }

    pub fn remove(&mut self, path: &str, id: u64) {
        if let Some(items) = self.breakpoints.get_mut(path) {
            items.retain(|item| item.id != id);
            if items.is_empty() {
                self.breakpoints.remove(path);
            }
        }
    }

    pub fn set_enabled(&mut self, path: &str, id: u64, enabled: bool) {
        if let Some(item) = self
            .breakpoints
            .get_mut(path)
            .and_then(|items| items.iter_mut().find(|item| item.id == id))
        {
            item.enabled = enabled;
            item.status = BreakpointStatus::Pending;
            item.message.clear();
        }
    }

    /// Clear breakpoints without discarding the running session's source baseline.
    pub fn clear(&mut self) {
        self.breakpoints.clear();
    }

    pub fn begin_launch(&mut self, session: &EditorSession) -> io::Result<()> {
        self.baseline.clear();
        self.renamed.clear();
        for document in session.document_ids() {
            let snapshot = session.snapshot(document)?;
            if snapshot.kind == DocumentKind::Text && !snapshot.path.is_empty() {
                self.paths.insert(document, snapshot.path.clone());
                self.current
                    .insert(snapshot.path.clone(), snapshot.bytes.clone());
                self.baseline.insert(snapshot.path, snapshot.bytes);
            }
        }
        for path in self.breakpoints.keys() {
            if !self.baseline.contains_key(path) {
                match source_bytes(path) {
                    Ok(bytes) => {
                        self.current.insert(path.clone(), bytes.clone());
                        self.baseline.insert(path.clone(), bytes);
                    }
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
            }
        }
        self.running = true;
        for items in self.breakpoints.values_mut() {
            for item in items {
                item.bound_row = Some(item.row);
                item.status = BreakpointStatus::Pending;
                item.message.clear();
            }
        }
        Ok(())
    }

    pub fn end_launch(&mut self) {
        self.running = false;
        self.baseline.clear();
        self.renamed.clear();
        for items in self.breakpoints.values_mut() {
            for item in items {
                item.bound_row = None;
                item.status = BreakpointStatus::Pending;
                item.message.clear();
            }
        }
    }

    pub fn changed(&self, path: &str) -> bool {
        self.running
            && (self.renamed.contains(path)
                || self
                    .baseline
                    .get(path)
                    .is_some_and(|baseline| self.current.get(path) != Some(baseline)))
    }

    /// Follow source edits once per session event stream. Returned paths include
    /// old rename targets so the host can clear obsolete adapter breakpoints.
    pub fn on_events(
        &mut self,
        session: &EditorSession,
        events: &[SessionEvent],
    ) -> io::Result<Vec<String>> {
        let mut affected = BTreeSet::new();
        let mut touched = BTreeSet::new();
        let mut relocated = BTreeSet::new();
        let mut reloaded_final = BTreeSet::new();
        // An open and rename can be queued before the host next drains events.
        let mut previous = HashMap::new();
        for event in events {
            if let SessionEvent::PathChanged {
                document,
                previous: path,
                ..
            } = event
            {
                previous.entry(*document).or_insert_with(|| path.clone());
            }
        }
        for event in events {
            match event {
                SessionEvent::Opened { document } => {
                    if let Ok(snapshot) = session.snapshot(*document)
                        && snapshot.kind == DocumentKind::Text
                        && !snapshot.path.is_empty()
                    {
                        let path = previous.get(document).cloned().unwrap_or(snapshot.path);
                        self.paths.insert(*document, path);
                        touched.insert(*document);
                    }
                }
                SessionEvent::Edited { document, edit, .. } => {
                    let path = match self.path_for(session, *document) {
                        Ok(path) => path,
                        Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                        Err(error) => return Err(error),
                    };
                    if !reloaded_final.contains(document) {
                        for change in &edit.changes {
                            let deleted_eof = self.current.get(&path).and_then(|bytes| {
                                if change.end_line <= change.start_line {
                                    return None;
                                }
                                let rows = lines(bytes);
                                (change.end_line as usize == rows.len() - 1
                                    && change.end_character >= utf16_len(rows.last().unwrap()))
                                .then_some(change.end_line)
                            });
                            if let Some(items) = self.breakpoints.get_mut(&path) {
                                items.retain_mut(|item| {
                                    if deleted_line(item.row, change)
                                        || deleted_eof == Some(item.row)
                                    {
                                        return false;
                                    }
                                    (item.row, item.column) =
                                        relocate((item.row, item.column), change);
                                    item.status = BreakpointStatus::Pending;
                                    item.message.clear();
                                    true
                                });
                            }
                            if let Some(bytes) = self.current.get_mut(&path) {
                                apply_change(bytes, change);
                            }
                        }
                    }
                    self.normalize(&path);
                    affected.insert(path);
                    touched.insert(*document);
                    relocated.insert(*document);
                }
                SessionEvent::Reloaded { document, .. } => {
                    let path = match self.path_for(session, *document) {
                        Ok(path) => path,
                        Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                        Err(error) => return Err(error),
                    };
                    let snapshot = match session.snapshot(*document) {
                        Ok(snapshot) => snapshot,
                        Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                        Err(error) => return Err(error),
                    };
                    let rows = lines(&snapshot.bytes);
                    if reloaded_final.insert(*document) {
                        if let Some(items) = self.breakpoints.get_mut(&path) {
                            if let Some(old) = self.current.get(&path) {
                                relocate_reload(items, old, &snapshot.bytes);
                            }
                            for item in items {
                                item.row = item.row.clamp(0, rows.len().saturating_sub(1) as i32);
                                item.column = item.column.min(utf16_len(rows[item.row as usize]));
                                item.status = BreakpointStatus::Pending;
                                item.message.clear();
                            }
                        }
                        self.current.insert(path.clone(), snapshot.bytes);
                    }
                    self.normalize(&path);
                    affected.insert(path);
                    touched.insert(*document);
                    relocated.insert(*document);
                }
                SessionEvent::PathChanged {
                    document,
                    previous,
                    path,
                } => {
                    if path.is_empty() {
                        // A deleted dirty buffer has no debugger file binding.
                        // Keep breakpoints at their original filename so a
                        // future recreation can resolve them, never at "".
                        self.paths.remove(document);
                        self.current.remove(previous);
                        if let Some(items) = self.breakpoints.get_mut(previous) {
                            for item in items {
                                item.status = BreakpointStatus::Pending;
                                item.message.clear();
                            }
                        }
                        affected.insert(previous.clone());
                        continue;
                    }
                    if let Some(mut items) = self.breakpoints.remove(previous) {
                        for item in &mut items {
                            item.status = BreakpointStatus::Pending;
                            item.message.clear();
                            if self.running {
                                item.bound_row = None;
                            }
                        }
                        self.breakpoints
                            .entry(path.clone())
                            .or_default()
                            .append(&mut items);
                    }
                    if let Some(bytes) = self.current.remove(previous) {
                        self.current.insert(path.clone(), bytes);
                    }
                    if self.running {
                        self.renamed.insert(path.clone());
                    }
                    self.paths.insert(*document, path.clone());
                    self.normalize(path);
                    affected.insert(previous.clone());
                    affected.insert(path.clone());
                    touched.insert(*document);
                }
                SessionEvent::Closed { document } => {
                    if let Some(path) = self.paths.remove(document) {
                        match source_bytes(&path) {
                            Ok(bytes) => {
                                if let Some(items) = self.breakpoints.get_mut(&path)
                                    && let Some(old) = self.current.get(&path)
                                {
                                    relocate_reload(items, old, &bytes);
                                }
                                self.normalize(&path);
                                self.current.insert(path.clone(), bytes);
                            }
                            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                                self.current.remove(&path);
                            }
                            Err(error) => return Err(error),
                        }
                        affected.insert(path);
                    }
                }
                SessionEvent::Saved { document, .. }
                | SessionEvent::Conflict { document, .. }
                | SessionEvent::Removed { document, .. } => {
                    touched.insert(*document);
                }
                SessionEvent::ViewDetached { .. } => {}
            }
        }
        for document in touched {
            if let Ok(snapshot) = session.snapshot(document)
                && snapshot.kind == DocumentKind::Text
                && !snapshot.path.is_empty()
            {
                let path = snapshot.path;
                let before = self.changed(&path);
                if !relocated.contains(&document) {
                    if let Some(items) = self.breakpoints.get_mut(&path)
                        && let Some(old) = self.current.get(&path)
                    {
                        relocate_reload(items, old, &snapshot.bytes);
                    }
                    self.normalize(&path);
                }
                self.current.insert(path.clone(), snapshot.bytes);
                self.paths.insert(document, path.clone());
                if before != self.changed(&path) || self.breakpoints.contains_key(&path) {
                    affected.insert(path);
                }
            }
        }
        Ok(affected.into_iter().collect())
    }

    pub fn bindings(&self, path: &str) -> Vec<(u64, i32)> {
        let changed = self.changed(path);
        self.breakpoints
            .get(path)
            .into_iter()
            .flatten()
            .filter(|item| item.enabled)
            .filter_map(|item| {
                let row = if changed { item.bound_row? } else { item.row };
                Some((item.id, row))
            })
            .collect()
    }

    pub fn presentation(&self, path: &str, execution_row: Option<i32>) -> SourceDebugPresentation {
        let changed = self.changed(path);
        SourceDebugPresentation {
            breakpoints: self
                .breakpoints
                .get(path)
                .into_iter()
                .flatten()
                .map(|item| SourceBreakpoint {
                    row: item.row,
                    enabled: item.enabled,
                    status: if changed {
                        BreakpointStatus::Pending
                    } else {
                        item.status
                    },
                })
                .collect(),
            execution_row: execution_row.filter(|_| !changed),
        }
    }

    /// Result rows use the same zero-based convention as desired source rows.
    pub fn apply_result(
        &mut self,
        path: &str,
        ids: &[u64],
        results: &[(bool, Option<i32>, String)],
    ) {
        let changed = self.changed(path);
        let Some(items) = self.breakpoints.get_mut(path) else {
            return;
        };
        for (index, id) in ids.iter().enumerate() {
            if let Some(item) = items.iter_mut().find(|item| item.id == *id) {
                let Some(result) = results.get(index) else {
                    item.status = BreakpointStatus::Rejected;
                    item.message =
                        "Debugger adapter failed to return a breakpoint result".to_string();
                    continue;
                };
                item.status = if result.0 {
                    BreakpointStatus::Verified
                } else {
                    BreakpointStatus::Rejected
                };
                item.message = result.2.clone();
                if result.0
                    && let Some(row) = result.1.filter(|row| *row >= 0)
                {
                    item.bound_row = Some(row);
                    if !changed {
                        item.row = row;
                        item.column = 0;
                    }
                }
            }
        }
        self.normalize(path);
    }

    fn path_for(&mut self, session: &EditorSession, document: DocumentId) -> io::Result<String> {
        if let Some(path) = self.paths.get(&document) {
            return Ok(path.clone());
        }
        let path = session.with_document(document, |state| state.path.clone())?;
        self.paths.insert(document, path.clone());
        Ok(path)
    }

    fn normalize(&mut self, path: &str) {
        let Some(items) = self.breakpoints.get_mut(path) else {
            return;
        };
        items.sort_by_key(|item| (item.row, item.id));
        let mut index = 1;
        while index < items.len() {
            if items[index - 1].row == items[index].row {
                let other = items.remove(index);
                let keep = &mut items[index - 1];
                keep.enabled |= other.enabled;
                keep.bound_row = keep.bound_row.or(other.bound_row);
                keep.status = BreakpointStatus::Pending;
                keep.message.clear();
            } else {
                index += 1;
            }
        }
        if items.is_empty() {
            self.breakpoints.remove(path);
        }
    }
}

fn source_bytes(path: &str) -> io::Result<Vec<u8>> {
    let bytes = fs::read(path)?;
    Ok(bytes
        .strip_prefix(&[0xef, 0xbb, 0xbf])
        .unwrap_or(&bytes)
        .to_vec())
}

fn lines(bytes: &[u8]) -> Vec<&[u8]> {
    let mut result = Vec::new();
    let (mut start, mut index) = (0, 0);
    while index < bytes.len() {
        if bytes[index] == b'\r' || bytes[index] == b'\n' {
            result.push(&bytes[start..index]);
            if bytes[index] == b'\r' && bytes.get(index + 1) == Some(&b'\n') {
                index += 1;
            }
            start = index + 1;
        }
        index += 1;
    }
    result.push(&bytes[start..]);
    result
}

fn utf16_len(bytes: &[u8]) -> i32 {
    String::from_utf8_lossy(bytes).encode_utf16().count() as i32
}

fn apply_change(bytes: &mut Vec<u8>, change: &DocumentChange) {
    let offset = |row: i32, column: i32| {
        let (mut current_row, mut index) = (0, 0);
        while index < bytes.len() && current_row < row {
            if bytes[index] == b'\r' || bytes[index] == b'\n' {
                if bytes[index] == b'\r' && bytes.get(index + 1) == Some(&b'\n') {
                    index += 1;
                }
                current_row += 1;
            }
            index += 1;
        }
        if current_row != row {
            return None;
        }
        let start = index;
        while index < bytes.len() && bytes[index] != b'\r' && bytes[index] != b'\n' {
            index += 1;
        }
        let body = &bytes[start..index];
        Some(start + bed_editing::util::utf8::utf16_to_utf8_byte_offset(body, column) as usize)
    };
    if let (Some(start), Some(end)) = (
        offset(change.start_line, change.start_character),
        offset(change.end_line, change.end_character),
    ) && start <= end
        && end <= bytes.len()
    {
        bytes.splice(start..end, change.text.iter().copied());
    }
}

fn deleted_line(row: i32, change: &DocumentChange) -> bool {
    change.end_line > change.start_line
        && row < change.end_line
        && (row > change.start_line || row == change.start_line && change.start_character == 0)
}

fn relocate(point: (i32, i32), change: &DocumentChange) -> (i32, i32) {
    let start = (change.start_line, change.start_character);
    let end = (change.end_line, change.end_character);
    if point < start {
        return point;
    }
    let inserted = lines(&change.text);
    let new_end = (
        start.0 + inserted.len() as i32 - 1,
        utf16_len(inserted.last().unwrap()) + if inserted.len() == 1 { start.1 } else { 0 },
    );
    if point < end {
        return new_end;
    }
    if point.0 == end.0 {
        (new_end.0, new_end.1 + point.1 - end.1)
    } else {
        (point.0 + new_end.0 - end.0, point.1)
    }
}

/// Preserve equal prefix/suffix lines across whole-document reloads. Positions
/// within replaced content stay in the replacement block; deleted lines vanish.
fn relocate_reload(items: &mut Vec<Breakpoint>, old: &[u8], new: &[u8]) {
    let (old, new) = (lines(old), lines(new));
    let mut prefix = 0;
    while prefix < old.len().min(new.len()) && old[prefix] == new[prefix] {
        prefix += 1;
    }
    let (mut old_end, mut new_end) = (old.len(), new.len());
    while old_end > prefix && new_end > prefix && old[old_end - 1] == new[new_end - 1] {
        old_end -= 1;
        new_end -= 1;
    }
    items.retain_mut(|item| {
        let row = item.row as usize;
        if row >= old_end {
            item.row += new_end as i32 - old_end as i32;
        } else if row >= prefix {
            if new_end == prefix {
                return false;
            }
            item.row = (prefix + (row - prefix).min(new_end - prefix - 1)) as i32;
            item.column = 0;
        }
        true
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use bed_editing::editor_commands::CursorReveal;

    fn drain(state: &mut SourceDebugState, session: &mut EditorSession) -> Vec<String> {
        let events = session.take_events();
        state.on_events(session, &events).unwrap()
    }

    #[test]
    fn relocation_uses_utf16_right_affinity_and_counts_crlf_once() {
        let insert = DocumentChange {
            start_line: 0,
            start_character: 3,
            end_line: 0,
            end_character: 3,
            text: "é🙂\r\nnew".as_bytes().to_vec(),
        };
        assert_eq!(relocate((0, 3), &insert), (1, 3));
        assert_eq!(relocate((0, 5), &insert), (1, 5));
        assert_eq!(relocate((2, 7), &insert), (3, 7));
        assert_eq!(relocate((0, 2), &insert), (0, 2));
        let same_line = DocumentChange {
            text: "é🙂".as_bytes().to_vec(),
            ..insert.clone()
        };
        assert_eq!(relocate((0, 3), &same_line), (0, 6));
        let delete = DocumentChange {
            start_line: 0,
            start_character: 1,
            end_line: 0,
            end_character: 4,
            text: Vec::new(),
        };
        assert_eq!(relocate((0, 3), &delete), (0, 1));
        assert!(
            !deleted_line(0, &delete),
            "editing a line's content retains its breakpoint"
        );
        let delete_lines = DocumentChange {
            start_line: 0,
            start_character: 2,
            end_line: 3,
            end_character: 1,
            text: Vec::new(),
        };
        assert!(!deleted_line(0, &delete_lines));
        assert!(deleted_line(1, &delete_lines));
        assert!(deleted_line(2, &delete_lines));
        assert!(!deleted_line(3, &delete_lines));
    }

    #[test]
    fn edited_source_retains_compiled_bindings_defers_new_breakpoints_and_recovers_on_undo() {
        let dir = TempDir::new();
        let path = dir.write("program.rs", b"head\nhit\ntail\n");
        let path = path.canonicalize().unwrap();
        let path = path.to_str().unwrap();
        let mut session = EditorSession::new();
        let document = session.open_file(path.as_ref()).unwrap();
        let view = session.create_view(document).unwrap();
        let mut state = SourceDebugState::default();
        drain(&mut state, &mut session);
        state.toggle(path, 1, 0);
        let id = state.breakpoints[path][0].id;
        state.begin_launch(&session).unwrap();
        state.apply_result(path, &[id], &[(true, Some(1), String::new())]);
        session
            .with_commands(view, |commands| {
                commands.set_cursor(0, 0, false, CursorReveal::Ensure);
                commands.type_text(b"new\n");
            })
            .unwrap();
        assert_eq!(drain(&mut state, &mut session), vec![path]);
        assert!(state.changed(path));
        assert_eq!(state.breakpoints[path][0].row, 2);
        assert_eq!(state.bindings(path), vec![(id, 1)]);
        let presentation = state.presentation(path, Some(1));
        assert_eq!(presentation.execution_row, None);
        assert_eq!(
            presentation.breakpoints[0].status,
            BreakpointStatus::Pending
        );
        state.toggle(path, 3, 0);
        assert_eq!(
            state.bindings(path),
            vec![(id, 1)],
            "new changed-source markers wait"
        );
        state.set_enabled(path, id, false);
        assert!(state.bindings(path).is_empty());
        state.set_enabled(path, id, true);
        assert_eq!(state.bindings(path), vec![(id, 1)]);
        session
            .with_commands(view, |commands| commands.undo())
            .unwrap();
        drain(&mut state, &mut session);
        assert!(!state.changed(path));
        assert_eq!(state.breakpoints[path][0].row, 1);
        assert_eq!(state.presentation(path, Some(1)).execution_row, Some(1));
        assert_eq!(state.bindings(path).len(), 2);
        state.end_launch();
        assert_eq!(state.breakpoints[path].len(), 2);
        assert!(
            state.breakpoints[path]
                .iter()
                .all(|item| item.bound_row.is_none())
        );
    }

    #[test]
    fn incomplete_adapter_results_reject_unmatched_breakpoints_and_preserve_bindings() {
        let path = "program.rs";
        let mut state = SourceDebugState::default();
        state.toggle(path, 1, 0);
        state.toggle(path, 3, 0);
        let ids: Vec<_> = state.breakpoints[path].iter().map(|item| item.id).collect();
        state.apply_result(
            path,
            &ids,
            &[
                (true, Some(2), String::new()),
                (true, Some(4), String::new()),
            ],
        );
        let bindings = state.bindings(path);

        state.apply_result(path, &ids, &[(true, Some(2), String::new())]);
        assert_eq!(
            state.breakpoints[path][0].status,
            BreakpointStatus::Verified
        );
        assert_eq!(
            state.breakpoints[path][1].status,
            BreakpointStatus::Rejected
        );
        assert!(state.breakpoints[path][1].message.contains("adapter"));
        assert_eq!(state.bindings(path), bindings);

        state.apply_result(path, &ids, &[]);
        assert!(state.breakpoints[path].iter().all(|item| {
            item.status == BreakpointStatus::Rejected
                && item.message == "Debugger adapter failed to return a breakpoint result"
        }));
        assert_eq!(state.bindings(path), bindings);
        assert_eq!(state.breakpoints[path][0].row, 2);
        assert_eq!(state.breakpoints[path][1].row, 4);
        assert_eq!(state.breakpoints[path][0].bound_row, Some(2));
        assert_eq!(state.breakpoints[path][1].bound_row, Some(4));
    }

    #[test]
    fn deleting_lines_removes_their_breakpoints_and_rename_defers_bindings() {
        let dir = TempDir::new();
        let path = dir.write("before.rs", b"head\nremoved\nsurvivor\n");
        let path = path.canonicalize().unwrap();
        let renamed = dir.root().canonicalize().unwrap().join("after.rs");
        let path = path.to_str().unwrap();
        let renamed = renamed.to_str().unwrap();
        let mut session = EditorSession::new();
        let document = session.open_file(path.as_ref()).unwrap();
        let view = session.create_view(document).unwrap();
        let mut state = SourceDebugState::default();
        drain(&mut state, &mut session);
        state.toggle(path, 1, 0);
        state.toggle(path, 2, 0);
        let survivor = state.breakpoints[path][1].id;
        state.begin_launch(&session).unwrap();
        session
            .with_commands(view, |commands| {
                commands.set_selection(1, 0, 2, 0, CursorReveal::Ensure);
                commands.delete_right(false);
            })
            .unwrap();
        drain(&mut state, &mut session);
        assert_eq!(state.breakpoints[path].len(), 1);
        assert_eq!(state.breakpoints[path][0].id, survivor);
        assert_eq!(state.breakpoints[path][0].row, 1);
        assert_eq!(state.bindings(path), vec![(survivor, 2)]);
        session.rebind_path(document, renamed.as_ref()).unwrap();
        let affected = drain(&mut state, &mut session);
        assert!(affected.iter().any(|value| value == path));
        assert!(affected.iter().any(|value| value == renamed));
        assert!(!state.breakpoints.contains_key(path));
        assert!(state.changed(renamed));
        assert!(state.bindings(path).is_empty());
        assert!(state.bindings(renamed).is_empty());
        state.end_launch();
        state.begin_launch(&session).unwrap();
        assert!(!state.changed(renamed));
        assert_eq!(state.bindings(renamed), vec![(survivor, 1)]);
    }

    #[test]
    fn a_deleted_detached_buffer_keeps_breakpoints_at_the_original_path() {
        let dir = TempDir::new();
        let path = dir
            .write("removed.rs", b"fn main() {}\n")
            .canonicalize()
            .unwrap();
        let path = path.to_str().unwrap();
        let mut session = EditorSession::new();
        let document = session.open_file(path.as_ref()).unwrap();
        let view = session.create_view(document).unwrap();
        let mut state = SourceDebugState::default();
        drain(&mut state, &mut session);
        state.toggle(path, 0, 0);
        session
            .with_commands(view, |commands| commands.type_text(b"// unsaved\n"))
            .unwrap();
        drain(&mut state, &mut session);
        fs::remove_file(path).unwrap();
        session.invalidate_removed_path(document).unwrap();
        session.detach_removed_document(document).unwrap();
        drain(&mut state, &mut session);
        assert!(state.breakpoints.contains_key(path));
        assert!(!state.breakpoints.contains_key(""));
        assert!(!state.current.contains_key(""));
        assert!(!state.paths.contains_key(&document));
    }

    #[test]
    fn reload_preserves_equal_lines_and_coincident_markers_merge() {
        let dir = TempDir::new();
        let path = dir.write("reload.rs", b"head\nhit\ntail");
        let path = path.canonicalize().unwrap();
        let path = path.to_str().unwrap();
        let mut session = EditorSession::new();
        let document = session.open_file(path.as_ref()).unwrap();
        let mut state = SourceDebugState::default();
        drain(&mut state, &mut session);
        state.toggle(path, 1, 0);
        state.begin_launch(&session).unwrap();
        let id = state.breakpoints[path][0].id;
        fs::write(path, b"prefix\nhead\nhit\ntail").unwrap();
        session.reload_from_disk(document).unwrap();
        drain(&mut state, &mut session);
        assert_eq!(state.breakpoints[path][0].row, 2);
        assert!(state.changed(path));
        assert_eq!(state.bindings(path), vec![(id, 1)]);
        state.end_launch();
        state.toggle(path, 3, 0);
        let second = state.breakpoints[path][1].id;
        state.set_enabled(path, id, false);
        state.apply_result(path, &[second], &[(true, Some(2), String::new())]);
        assert_eq!(state.breakpoints[path].len(), 1);
        assert!(state.breakpoints[path][0].enabled);
        state.clear();
        assert!(state.breakpoints.is_empty());
    }

    #[test]
    fn discarding_a_document_restores_source_positions_even_when_edits_and_close_are_batched() {
        use bed_document_session::editor_session::ClosePolicy;

        let dir = TempDir::new();
        let path = dir
            .write("discard.rs", b"head\nhit\ntail")
            .canonicalize()
            .unwrap();
        let path = path.to_str().unwrap();
        let mut session = EditorSession::new();
        let document = session.open_file(path.as_ref()).unwrap();
        let view = session.create_view(document).unwrap();
        let mut state = SourceDebugState::default();
        drain(&mut state, &mut session);
        state.toggle(path, 1, 0);
        state.begin_launch(&session).unwrap();
        session
            .with_commands(view, |commands| {
                commands.set_cursor(0, 0, false, CursorReveal::Ensure);
                commands.type_text(b"unsaved\n");
            })
            .unwrap();
        session
            .close_document(document, ClosePolicy::Discard)
            .unwrap();
        drain(&mut state, &mut session);
        assert_eq!(state.breakpoints[path][0].row, 1);
        assert!(!state.changed(path));
        session.open_file(path.as_ref()).unwrap();
        drain(&mut state, &mut session);
        assert_eq!(state.breakpoints[path][0].row, 1);
        assert_eq!(state.presentation(path, Some(1)).execution_row, Some(1));
    }

    #[test]
    fn reload_and_subsequent_edit_in_one_report_do_not_move_markers_twice() {
        let dir = TempDir::new();
        let path = dir
            .write("batched.rs", b"head\nhit\ntail")
            .canonicalize()
            .unwrap();
        let path = path.to_str().unwrap();
        let mut session = EditorSession::new();
        let document = session.open_file(path.as_ref()).unwrap();
        let view = session.create_view(document).unwrap();
        let mut state = SourceDebugState::default();
        drain(&mut state, &mut session);
        state.toggle(path, 1, 0);
        state.begin_launch(&session).unwrap();
        fs::write(path, b"external\nhead\nhit\ntail").unwrap();
        session.reload_from_disk(document).unwrap();
        session
            .with_commands(view, |commands| {
                commands.set_cursor(0, 0, false, CursorReveal::Ensure);
                commands.type_text(b"editor\n");
            })
            .unwrap();
        drain(&mut state, &mut session);
        assert_eq!(state.breakpoints[path][0].row, 3);
        assert!(state.changed(path));
        let id = state.breakpoints[path][0].id;
        assert_eq!(state.bindings(path), vec![(id, 1)]);
        state.clear();
        assert!(
            state.changed(path),
            "clearing breakpoints must retain the launch snapshot"
        );
    }

    #[test]
    fn deleting_through_eof_removes_last_line_breakpoint_but_same_line_edit_keeps_it() {
        let dir = TempDir::new();
        let path = dir.write("eof.rs", b"head\nhit").canonicalize().unwrap();
        let path = path.to_str().unwrap();
        let mut session = EditorSession::new();
        let document = session.open_file(path.as_ref()).unwrap();
        let view = session.create_view(document).unwrap();
        let mut state = SourceDebugState::default();
        drain(&mut state, &mut session);
        state.toggle(path, 1, 0);
        session
            .with_commands(view, |commands| {
                commands.set_selection(1, 0, 1, 3, CursorReveal::Ensure);
                commands.type_text(b"replacement");
            })
            .unwrap();
        drain(&mut state, &mut session);
        assert_eq!(state.breakpoints[path][0].row, 1);
        session
            .with_commands(view, |commands| {
                commands.set_selection(0, 0, 1, 11, CursorReveal::Ensure);
                commands.delete_right(false);
            })
            .unwrap();
        drain(&mut state, &mut session);
        assert!(!state.breakpoints.contains_key(path));
    }
}
