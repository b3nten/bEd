//! Workspace text edits are validated together and committed by the session.
use super::*;
use bed_editing::project_undo::{
    WorkspaceHistoryGroup, WorkspaceHistoryParticipant, history_fingerprint,
};
use bed_lsp::{jsonrpc::RpcId, message_handler::RpcEvent, workspace_edit};
use serde_json::{Value, json};
use std::collections::BTreeSet;

pub(super) struct DeferredWorkspaceEdit {
    generation: u64,
    id: RpcId,
    edit: Value,
    started: Instant,
}

pub(super) struct PendingWorkspaceEdit {
    edit: Value,
    origin: Option<LspRequestOrigin>,
    revisions: CapturedDocumentRevisions,
    unopened: BTreeSet<PathBuf>,
    applied: Option<AppliedWorkspaceEdit>,
    remote_checked: BTreeSet<DocumentId>,
    started: Instant,
    history_transition: bool,
}

pub(super) struct PendingWorkspaceHistory {
    group: WorkspaceHistoryGroup,
    undo: bool,
    remote_checked: BTreeSet<DocumentId>,
    applied: Option<PendingWorkspaceEdit>,
}

pub(super) struct WorkspaceEditReceipt {
    edit: Value,
    origin: Option<LspRequestOrigin>,
    result: Result<Vec<DocumentId>, String>,
    completed: Instant,
}

struct AppliedWorkspaceEdit {
    documents: Vec<DocumentId>,
    participants: Vec<WorkspaceHistoryParticipant>,
    before: Vec<(DocumentId, Vec<u8>, bool, bool)>,
    history: ProjectUndo,
}

fn pending() -> io::Error {
    io::Error::new(
        io::ErrorKind::WouldBlock,
        "Waiting for remote workspace edit I/O",
    )
}

fn changed(path: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("{} changed; request the LSP action again", path),
    )
}

fn group_id() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64
        ^ NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

impl DocumentSession {
    /// Capture open targets before a versionless multi-file LSP action starts.
    /// The response may only edit those buffers at their captured revisions.
    pub fn capture_lsp_workspace_revisions(&mut self, origin: &LspRequestOrigin) -> io::Result<()> {
        if !self.accepts_lsp_origin(origin) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Stale LSP request origin",
            ));
        }
        let revisions = self
            .document_ids()
            .into_iter()
            .map(|document| Ok((document, self.document_revision(document)?)))
            .collect::<io::Result<BTreeMap<_, _>>>()?;
        self.workspace_request_revisions
            .retain(|(recorded, _)| recorded != origin);
        self.workspace_request_revisions.push((*origin, revisions));
        if self.workspace_request_revisions.len() > 128 {
            self.workspace_request_revisions.remove(0);
        }
        Ok(())
    }

    pub fn lsp_workspace_edit_is_pending(
        &self,
        edit: &Value,
        origin: Option<&LspRequestOrigin>,
    ) -> bool {
        self.pending_workspace_edits
            .iter()
            .any(|pending| pending.edit == *edit && pending.origin.as_ref() == origin)
            || self
                .workspace_edit_receipts
                .iter()
                .any(|receipt| receipt.edit == *edit && receipt.origin.as_ref() == origin)
    }

    pub fn begin_lsp_command(
        &mut self,
        origin: &LspRequestOrigin,
        include_current_edit: bool,
    ) -> io::Result<()> {
        if self
            .active_lsp_commands
            .contains_key(&origin.server_generation)
        {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Another command from this language server is still running",
            ));
        }
        if origin.session_id != self.session_id
            || self
                .lsp
                .as_ref()
                .is_none_or(|pool| !pool.server_generation_is_current(origin.server_generation))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Stale LSP command origin",
            ));
        }
        if self
            .entry(origin.document_id)?
            .runtime
            .document_generation()
            != origin.document_generation
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Stale LSP command document",
            ));
        }
        let key = self
            .entry(origin.document_id)?
            .runtime
            .history_key()
            .to_owned();
        let mut group = if include_current_edit {
            self.history.borrow_mut().workspace_group(&key, true)
        } else {
            None
        };
        if include_current_edit && group.is_none() {
            let edit = self
                .history
                .borrow_mut()
                .latest_undo_edit(&key)
                .ok_or_else(|| io::Error::other("LSP command has no initiating edit"))?;
            let mut state = self.entry(origin.document_id)?.runtime.state.clone();
            let after = history_fingerprint(&state.join());
            let mut operations = EditingOperations::new();
            for step in edit.steps.iter().rev() {
                if !operations
                    .apply(
                        &mut state,
                        &EditingOperations::invert(&step.op, &step.deleted_text),
                    )
                    .ok
                {
                    return Err(io::Error::other(
                        "LSP command history could not be reconstructed",
                    ));
                }
            }
            let promoted = WorkspaceHistoryGroup {
                id: group_id(),
                participants: vec![WorkspaceHistoryParticipant {
                    path: key,
                    persisted: false,
                    before: history_fingerprint(&state.join()),
                    after,
                }],
            };
            self.history.borrow_mut().group_latest(promoted.clone())?;
            group = Some(promoted);
        }
        self.active_lsp_commands
            .insert(origin.server_generation, group);
        let revisions = self
            .document_ids()
            .into_iter()
            .map(|document| Ok((document, self.document_revision(document)?)))
            .collect::<io::Result<BTreeMap<_, _>>>()?;
        self.active_lsp_command_revisions
            .insert(origin.server_generation, revisions);
        Ok(())
    }

    pub fn finish_lsp_command(&mut self, server_generation: u64) {
        self.active_lsp_commands.remove(&server_generation);
        self.active_lsp_command_revisions.remove(&server_generation);
    }

    pub fn apply_lsp_text_edits(
        &mut self,
        document: DocumentId,
        expected_revision: (u64, u64),
        value: &Value,
    ) -> io::Result<()> {
        if self.document_revision(document)? != expected_revision {
            return Err(changed(&self.snapshot(document)?.path));
        }
        let state = &self.entry(document)?.runtime.state;
        if state.kind != DocumentKind::Text || state.read_only {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "LSP edits require an editable text document",
            ));
        }
        let edits = workspace_edit::parse_text_edits(value)?;
        let edits = workspace_edit::byte_edits(&state.join(), &edits)?
            .into_iter()
            .map(|(range, bytes)| ByteEdit { range, bytes })
            .collect::<Vec<_>>();
        self.apply_edits(document, expected_revision, &edits)
    }

    /// Remote targets use the existing asynchronous file service. On WouldBlock
    /// retry the same payload/origin after ticking the session; mutation occurs
    /// once, and success is returned only after unopened targets are saved.
    pub fn apply_lsp_workspace_edit(
        &mut self,
        value: &Value,
        origin: Option<&LspRequestOrigin>,
    ) -> io::Result<Vec<DocumentId>> {
        self.ensure_running()?;
        if let Some(index) = self
            .workspace_edit_receipts
            .iter()
            .position(|receipt| receipt.edit == *value && receipt.origin.as_ref() == origin)
        {
            return self
                .workspace_edit_receipts
                .remove(index)
                .result
                .map_err(io::Error::other);
        }
        let existing = self
            .pending_workspace_edits
            .iter()
            .position(|pending| pending.edit == *value && pending.origin.as_ref() == origin);
        let mut transaction = if let Some(index) = existing {
            self.pending_workspace_edits.remove(index)
        } else {
            let documents = workspace_edit::parse_workspace_edit(value)?;
            let mut revisions = BTreeMap::new();
            let mut unopened = BTreeSet::new();
            let captured = origin.and_then(|origin| {
                self.workspace_request_revisions
                    .iter()
                    .find(|(recorded, _)| recorded == origin)
                    .map(|(_, revisions)| revisions)
            });
            for edit in &documents {
                if let Some(id) = self.document_for_path(&edit.path) {
                    let current = self.document_revision(id)?;
                    if let Some(captured) = captured
                        && (captured
                            .get(&id)
                            .is_some_and(|revision| *revision != current)
                            || (!captured.contains_key(&id) && self.snapshot(id)?.dirty))
                    {
                        return Err(changed(&self.snapshot(id)?.path));
                    }
                    revisions.insert(id, current);
                } else {
                    unopened.insert(edit.path.clone());
                }
            }
            PendingWorkspaceEdit {
                edit: value.clone(),
                origin: origin.copied(),
                revisions,
                unopened,
                applied: None,
                remote_checked: BTreeSet::new(),
                started: Instant::now(),
                history_transition: false,
            }
        };
        let result = self.apply_pending_workspace_edit(&mut transaction);
        if result
            .as_ref()
            .is_err_and(|error| error.kind() == io::ErrorKind::WouldBlock)
        {
            self.pending_workspace_edits.push(transaction);
        }
        result
    }

    fn apply_pending_workspace_edit(
        &mut self,
        transaction: &mut PendingWorkspaceEdit,
    ) -> io::Result<Vec<DocumentId>> {
        if let Some(applied) = &transaction.applied {
            for participant in &applied.participants {
                if participant.persisted {
                    let id = self
                        .document_for_path(Path::new(&participant.path))
                        .ok_or_else(|| changed(&participant.path))?;
                    if self.save_pending(id) {
                        return Err(pending());
                    }
                    if self.snapshot(id)?.dirty {
                        let error = io::Error::other(format!(
                            "Could not save workspace edit to {}",
                            participant.path
                        ));
                        return self.rollback_workspace_edit(transaction, error);
                    }
                }
            }
            let applied = transaction.applied.take().unwrap();
            for &(document, _, _, autosave_paused) in &applied.before {
                let entry = self.entry_mut(document)?;
                entry.runtime.state.read_only = false;
                entry.workspace_edit_locked = false;
                entry.autosave_paused = autosave_paused;
            }
            if !transaction.history_transition {
                self.history
                    .borrow_mut()
                    .group_latest(WorkspaceHistoryGroup {
                        id: group_id(),
                        participants: applied.participants,
                    })?;
            }
            return Ok(applied.documents);
        }
        if transaction
            .origin
            .as_ref()
            .is_some_and(|origin| !self.accepts_lsp_origin(origin))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Stale LSP request origin",
            ));
        }
        for (&document, &revision) in &transaction.revisions {
            if self.document_revision(document)? != revision {
                return Err(changed(&self.snapshot(document)?.path));
            }
        }
        let documents = workspace_edit::parse_workspace_edit(&transaction.edit)?;
        let mut waiting = false;
        for edit in &documents {
            self.ensure_local_path_editable(&edit.path)?;
            if self.document_for_path(&edit.path).is_none() {
                match self.request_open_mode(
                    &edit.path,
                    Some(DocumentKind::Text),
                    transaction.origin,
                )? {
                    Some(id) => {
                        transaction
                            .revisions
                            .insert(id, self.document_revision(id)?);
                    }
                    None => waiting = true,
                }
            }
        }
        if waiting {
            return Err(pending());
        }
        if self.remote.is_some() {
            for edit in &documents {
                let document = self.document_for_path(&edit.path).unwrap();
                if !transaction.remote_checked.contains(&document) {
                    if self.remote_workspace_check(document)? {
                        transaction.remote_checked.insert(document);
                    } else {
                        waiting = true;
                    }
                }
            }
            if waiting {
                return Err(pending());
            }
        }
        let mut prepared = Vec::new();
        let mut participants = Vec::new();
        let mut before = Vec::new();
        let mut canonical_documents = BTreeSet::new();
        for edit in documents {
            let document = self.document_for_path(&edit.path).unwrap();
            if !canonical_documents.insert(document) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "WorkspaceEdit repeats the same file through aliases",
                ));
            }
            transaction
                .revisions
                .entry(document)
                .or_insert(self.document_revision(document)?);
            let entry = self.entry(document)?;
            let state = &entry.runtime.state;
            if state.kind != DocumentKind::Text
                || state.read_only
                || entry.removed
                || entry.runtime.disk_conflict.is_some()
            {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!("Cannot apply workspace edits to {}", state.path),
                ));
            }
            if edit.version.is_some_and(|version| state.version != version) {
                return Err(changed(&state.path));
            }
            if self.save_pending(document) {
                return Err(pending());
            }
            let bytes = state.join();
            let mut edits = workspace_edit::byte_edits(&bytes, &edit.edits)?
                .into_iter()
                .map(|(range, bytes)| ByteEdit { range, bytes })
                .collect::<Vec<_>>();
            let mut after = bytes.clone();
            for edit in &mut edits {
                edit.bytes = EditingOperations::normalize_line_endings(state, &edit.bytes);
            }
            for edit in edits.iter().rev() {
                after.splice(edit.range.clone(), edit.bytes.iter().copied());
            }
            if after == bytes {
                continue;
            }
            if after.len() + if state.utf8_bom { 3 } else { 0 } > bed_files::files::MAX_FILE_SIZE {
                return Err(bed_files::files::file_too_large(
                    Path::new(&state.path),
                    after.len() as u64,
                ));
            }
            let persisted = transaction.unopened.contains(&edit.path);
            let path = state.path.clone();
            if self.remote.is_none() {
                if std::fs::metadata(&state.path)?.permissions().readonly() {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        format!("{} is read-only", state.path),
                    ));
                }
                // Even dirty open buffers must still refer to the disk baseline
                // observed by their monitor. Polling never replaces dirty text.
                self.poll_monitor(document)?;
                if self.entry(document)?.runtime.disk_conflict.is_some() {
                    return Err(changed(&path));
                }
            }
            let entry = self.entry(document)?;
            if self.document_revision(document)? != transaction.revisions[&document] {
                return Err(changed(&entry.runtime.state.path));
            }
            participants.push(WorkspaceHistoryParticipant {
                path: entry.runtime.history_key().to_owned(),
                persisted,
                before: history_fingerprint(&bytes),
                after: history_fingerprint(&after),
            });
            before.push((
                document,
                bytes,
                entry.runtime.state.dirty,
                entry.autosave_paused,
            ));
            prepared.push((document, edits));
        }
        if prepared.is_empty() {
            return Ok(Vec::new());
        }
        let history = self.history.borrow().clone();
        let ids = prepared.iter().map(|(id, _)| *id).collect::<Vec<_>>();
        transaction.applied = Some(AppliedWorkspaceEdit {
            documents: ids.clone(),
            participants,
            before,
            history,
        });
        for (document, edits) in prepared {
            if let Err(error) = self.apply_edits(document, transaction.revisions[&document], &edits)
            {
                return self.rollback_workspace_edit(transaction, error);
            }
        }
        for &document in &ids {
            let entry = self.entry_mut(document)?;
            entry.runtime.state.read_only = true;
            entry.workspace_edit_locked = true;
            entry.autosave_paused = true;
        }
        let persisted = transaction
            .applied
            .as_ref()
            .unwrap()
            .participants
            .iter()
            .filter(|participant| participant.persisted)
            .map(|participant| participant.path.clone())
            .collect::<Vec<_>>();
        for path in persisted {
            let document = self.document_for_path(Path::new(&path)).unwrap();
            if let Err(error) = self.save(document) {
                return self.rollback_workspace_edit(transaction, error);
            }
        }
        if self.remote.is_some() && ids.iter().any(|id| self.save_pending(*id)) {
            return Err(pending());
        }
        let applied = transaction.applied.take().unwrap();
        for &(document, _, _, autosave_paused) in &applied.before {
            let entry = self.entry_mut(document)?;
            entry.runtime.state.read_only = false;
            entry.workspace_edit_locked = false;
            entry.autosave_paused = autosave_paused;
        }
        self.history
            .borrow_mut()
            .group_latest(WorkspaceHistoryGroup {
                id: group_id(),
                participants: applied.participants,
            })?;
        Ok(ids)
    }

    fn rollback_workspace_edit(
        &mut self,
        transaction: &mut PendingWorkspaceEdit,
        error: io::Error,
    ) -> io::Result<Vec<DocumentId>> {
        let Some(applied) = transaction.applied.take() else {
            return Err(error);
        };
        let mut failures = Vec::new();
        for (document, bytes, dirty, autosave_paused) in &applied.before {
            self.entry_mut(*document)?.runtime.state.read_only = false;
            self.entry_mut(*document)?.workspace_edit_locked = false;
            self.entry_mut(*document)?.autosave_paused = *autosave_paused;
            let revision = self.document_revision(*document)?;
            let size = self.entry(*document)?.runtime.state.byte_size();
            if let Err(error) = self.apply_edits(
                *document,
                revision,
                &[ByteEdit {
                    range: 0..size,
                    bytes: bytes.clone(),
                }],
            ) {
                failures.push(error.to_string());
            }
            let path = self.snapshot(*document)?.path;
            if applied
                .participants
                .iter()
                .any(|participant| participant.persisted && participant.path == path)
                && let Err(error) = self.save(*document)
            {
                failures.push(format!("{}: {}", path, error));
            }
            self.entry_mut(*document)?.runtime.state.dirty = *dirty;
        }
        let paths = applied
            .participants
            .iter()
            .map(|participant| participant.path.clone())
            .collect::<Vec<_>>();
        self.history
            .borrow_mut()
            .restore_files_from(&applied.history, &paths);
        Err(io::Error::new(
            error.kind(),
            if failures.is_empty() {
                format!("Workspace edit rolled back: {error}")
            } else {
                format!(
                    "Workspace edit failed: {error}; rollback failed for {}",
                    failures.join(", ")
                )
            },
        ))
    }

    pub(super) fn apply_workspace_history(
        &mut self,
        group: &WorkspaceHistoryGroup,
        undo: bool,
    ) -> io::Result<()> {
        if self.pending_workspace_history.is_some() {
            return Err(pending());
        }
        self.history
            .borrow_mut()
            .validate_workspace_group(group, undo)?;
        let mut request = PendingWorkspaceHistory {
            group: group.clone(),
            undo,
            remote_checked: BTreeSet::new(),
            applied: None,
        };
        match self.apply_pending_workspace_history(&mut request) {
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                self.pending_workspace_history = Some(request);
                Ok(())
            }
            result => result,
        }
    }

    fn apply_pending_workspace_history(
        &mut self,
        request: &mut PendingWorkspaceHistory,
    ) -> io::Result<()> {
        if let Some(transaction) = &mut request.applied {
            return self.apply_pending_workspace_edit(transaction).map(|_| ());
        }
        let group = &request.group;
        let undo = request.undo;
        self.history
            .borrow_mut()
            .validate_workspace_group(group, undo)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.to_string()))?;
        let mut documents = Vec::new();
        let mut waiting = false;
        for participant in &group.participants {
            let path = Path::new(&participant.path);
            self.ensure_local_path_editable(path)?;
            let document = match self.document_for_path(path) {
                Some(id) => id,
                None => match self.request_open_file(path)? {
                    Some(document) => document,
                    None => {
                        waiting = true;
                        continue;
                    }
                },
            };
            if self.remote.is_none() {
                if !path.try_exists()? {
                    return Err(changed(&participant.path));
                }
                self.poll_monitor(document)?;
            }
            let snapshot = self.snapshot(document)?;
            if snapshot.kind != DocumentKind::Text
                || snapshot.disk_conflict.is_some()
                || history_fingerprint(&snapshot.bytes)
                    != if undo {
                        participant.after
                    } else {
                        participant.before
                    }
            {
                return Err(changed(&participant.path));
            }
            if self.save_pending(document) {
                return Err(pending());
            }
            if self.remote.is_some() && !request.remote_checked.contains(&document) {
                if self.remote_workspace_check(document)? {
                    request.remote_checked.insert(document);
                } else {
                    waiting = true;
                }
            }
            if participant.persisted && self.remote.is_none() {
                let raw = crate::save_service::DocumentSave::bytes_for_save(
                    &self.entry(document)?.runtime.state,
                )?;
                if read_file_bytes(path)?.raw != raw {
                    return Err(changed(&participant.path));
                }
            }
            documents.push((document, participant.persisted));
        }
        if waiting {
            return Err(pending());
        }
        let before = documents
            .iter()
            .map(|&(document, _)| {
                let entry = self.entry(document)?;
                Ok((
                    document,
                    entry.runtime.state.join(),
                    entry.runtime.state.dirty,
                    entry.autosave_paused,
                ))
            })
            .collect::<io::Result<Vec<_>>>()?;
        let history = self.history.borrow().clone();
        let mut transaction = PendingWorkspaceEdit {
            edit: Value::Null,
            origin: None,
            revisions: BTreeMap::new(),
            unopened: BTreeSet::new(),
            remote_checked: BTreeSet::new(),
            started: Instant::now(),
            history_transition: true,
            applied: Some(AppliedWorkspaceEdit {
                documents: documents.iter().map(|(document, _)| *document).collect(),
                participants: group.participants.clone(),
                before,
                history,
            }),
        };
        self.history
            .borrow_mut()
            .allow_workspace_history(Some(group.id));
        let result = documents
            .iter()
            .try_for_each(|&(document, _)| self.history_document_ungrouped(document, undo));
        self.history.borrow_mut().allow_workspace_history(None);
        if let Err(error) = result {
            return self
                .rollback_workspace_edit(&mut transaction, error)
                .map(|_| ());
        }
        for &(document, _) in &documents {
            let entry = self.entry_mut(document)?;
            entry.workspace_edit_locked = true;
            entry.runtime.state.read_only = true;
            entry.autosave_paused = true;
        }
        for (document, persisted) in documents {
            if persisted && let Err(error) = self.save(document) {
                return self
                    .rollback_workspace_edit(&mut transaction, error)
                    .map(|_| ());
            }
        }
        let result = self
            .apply_pending_workspace_edit(&mut transaction)
            .map(|_| ());
        if result
            .as_ref()
            .is_err_and(|error| error.kind() == io::ErrorKind::WouldBlock)
        {
            request.applied = Some(transaction);
        }
        result?;
        Ok(())
    }

    pub(super) fn poll_workspace_edits(&mut self, report: &mut TickReport) {
        if let Some(mut request) = self.pending_workspace_history.take() {
            match self.apply_pending_workspace_history(&mut request) {
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    self.pending_workspace_history = Some(request);
                }
                Err(error) => self.record_error(None, "undo", &error),
                Ok(()) => {}
            }
        }
        self.workspace_edit_receipts
            .retain(|receipt| receipt.completed.elapsed() < Duration::from_secs(20));
        // Once text has changed, finish its disk commit even if the invoking UI
        // closes. Receipts let a still-active caller collect the exact result.
        let pending_edits = std::mem::take(&mut self.pending_workspace_edits);
        for mut transaction in pending_edits {
            if transaction.applied.is_none() {
                if transaction.started.elapsed() < Duration::from_secs(20)
                    && transaction
                        .origin
                        .as_ref()
                        .is_none_or(|origin| self.accepts_lsp_origin(origin))
                {
                    self.pending_workspace_edits.push(transaction);
                }
                continue;
            }
            let result = self.apply_pending_workspace_edit(&mut transaction);
            if result
                .as_ref()
                .is_err_and(|error| error.kind() == io::ErrorKind::WouldBlock)
            {
                self.pending_workspace_edits.push(transaction);
            } else {
                self.workspace_edit_receipts.push(WorkspaceEditReceipt {
                    edit: transaction.edit,
                    origin: transaction.origin,
                    result: result.map_err(|error| error.to_string()),
                    completed: Instant::now(),
                });
            }
        }
        for event in &report.lsp_events {
            if let WorkspaceLspEvent::Rpc {
                server_generation,
                event: RpcEvent::WorkspaceEditRequest { id, params },
                ..
            } = event
            {
                self.deferred_workspace_edits.push(DeferredWorkspaceEdit {
                    generation: *server_generation,
                    id: id.clone(),
                    edit: params
                        .as_ref()
                        .and_then(|params| params.get("edit"))
                        .filter(|edit| edit.is_object())
                        .cloned()
                        .unwrap_or(Value::Bool(false)),
                    started: Instant::now(),
                });
            }
        }
        let edits = std::mem::take(&mut self.deferred_workspace_edits);
        let mut waiting_generations = BTreeSet::new();
        for edit in edits {
            if waiting_generations.contains(&edit.generation) {
                self.deferred_workspace_edits.push(edit);
                continue;
            }
            let current = self
                .lsp
                .as_ref()
                .is_some_and(|pool| pool.server_generation_is_current(edit.generation));
            let previous = self
                .active_lsp_commands
                .get(&edit.generation)
                .cloned()
                .flatten();
            let already_applied = self.pending_workspace_edits.iter().any(|pending| {
                pending.edit == edit.edit && pending.origin.is_none() && pending.applied.is_some()
            }) || self
                .workspace_edit_receipts
                .iter()
                .any(|receipt| receipt.edit == edit.edit && receipt.origin.is_none());
            let valid_group = if already_applied {
                Ok(())
            } else {
                previous.as_ref().map_or(Ok(()), |group| {
                    self.history
                        .borrow_mut()
                        .validate_workspace_group(group, true)
                })
            };
            let valid_targets = if already_applied {
                Ok(())
            } else {
                workspace_edit::parse_workspace_edit(&edit.edit).and_then(|targets| {
                    let Some(revisions) = self.active_lsp_command_revisions.get(&edit.generation)
                    else {
                        return Ok(());
                    };
                    for target in targets {
                        if let Some(document) = self.document_for_path(&target.path) {
                            let current = self.document_revision(document)?;
                            if revisions
                                .get(&document)
                                .is_some_and(|revision| *revision != current)
                                || (!revisions.contains_key(&document)
                                    && self.snapshot(document)?.dirty)
                            {
                                return Err(changed(&self.snapshot(document)?.path));
                            }
                        }
                    }
                    Ok(())
                })
            };
            let result = if !current {
                Err(io::Error::new(
                    io::ErrorKind::NotConnected,
                    "Language server restarted before workspace edit",
                ))
            } else if let Err(error) = valid_group {
                Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    error.to_string(),
                ))
            } else if let Err(error) = valid_targets {
                Err(error)
            } else if !already_applied && edit.started.elapsed() >= Duration::from_secs(20) {
                Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Workspace edit timed out",
                ))
            } else {
                self.apply_lsp_workspace_edit(&edit.edit, None)
            };
            if result
                .as_ref()
                .is_err_and(|error| error.kind() == io::ErrorKind::WouldBlock)
            {
                waiting_generations.insert(edit.generation);
                self.deferred_workspace_edits.push(edit);
                continue;
            }
            let result = result.and_then(|documents| {
                if let Some(document) = documents.first() {
                    let key = self.entry(*document)?.runtime.history_key().to_owned();
                    let next = self
                        .history
                        .borrow_mut()
                        .workspace_group(&key, true)
                        .unwrap();
                    let group = if let Some(previous) = &previous {
                        self.history
                            .borrow_mut()
                            .merge_workspace_groups(previous, &next)?
                    } else {
                        next
                    };
                    if self.active_lsp_commands.contains_key(&edit.generation) {
                        self.active_lsp_commands
                            .insert(edit.generation, Some(group));
                    }
                }
                let revisions = documents
                    .iter()
                    .map(|document| Ok((*document, self.document_revision(*document)?)))
                    .collect::<io::Result<Vec<_>>>()?;
                if let Some(current) = self.active_lsp_command_revisions.get_mut(&edit.generation) {
                    current.extend(revisions);
                }
                Ok(documents)
            });
            let result = match result {
                Ok(_) => json!({"applied":true}),
                Err(error) => {
                    self.record_error(None, "lsp", &error);
                    json!({"applied":false,"failureReason":error.to_string()})
                }
            };
            if let Some(pool) = &mut self.lsp {
                let _ = pool.reply_workspace_edit(edit.generation, edit.id, Ok(result));
            }
        }
    }
}
