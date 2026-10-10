use crate::{DIFF_PANEL_ID, MODULE_ID, jobs::Worker};
use bed_document_session::{DocumentId, EditorSession};
use bed_editor_ui::{DiffAction, DiffActionKind, SourceGitPresentation};
use bed_git::{Diff, DiffSide, Operation, Output, RepositoryOperation, Status, StatusEntry};
use bed_remote::{RemoteClient, WorkspaceFilesystem, WorkspaceUpdate};
use bed_workbench_api::{HostRequest, ModuleServices, SaveToken, SavedDocument};
use serde_json::json;
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet, VecDeque},
    io,
    path::{Path, PathBuf},
    sync::{Arc, mpsc::Receiver},
    time::{Duration, Instant},
};

pub(crate) type DiffKey = (String, DiffSide);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StagingState {
    Unstaged,
    Partial,
    Staged,
}
impl StagingState {
    pub fn from_entry(entry: &StatusEntry) -> Self {
        if matches!(entry.index_status, ' ' | '.' | '?') || entry.conflicted {
            Self::Unstaged
        } else if !matches!(entry.worktree_status, ' ' | '.') {
            Self::Partial
        } else {
            Self::Staged
        }
    }
}

// Presentation only: repository status remains authoritative for Git operations.
struct OptimisticStaging {
    paths: HashSet<String>,
    stage: bool,
    awaiting_status: bool,
}

struct Task {
    operation: Operation,
    purpose: Purpose,
}
enum Purpose {
    Status,
    Diff(DiffKey),
    HunkRefresh {
        key: DiffKey,
        action: DiffAction,
        baseline: Vec<u8>,
    },
    Mutation {
        commit: Option<String>,
        saved: Vec<SavedDocument>,
    },
}
impl Purpose {
    fn blocks_actions(&self) -> bool {
        matches!(self, Self::Mutation { .. } | Self::HunkRefresh { .. })
    }
}

pub(crate) struct PendingSave {
    token: SaveToken,
    action: Operation,
    result: Option<Result<Vec<SavedDocument>, String>>,
    hunk: Option<(DiffKey, DiffAction, Vec<u8>)>,
}

pub(crate) struct GitController {
    pub root: String,
    pub transient: bool,
    pub status: Option<Status>,
    pub draft: String,
    pub branch_input: String,
    pub error: Option<String>,
    pub output: String,
    pub refresh: bool,
    pub visible: usize,
    pub diffs: HashMap<DiffKey, Arc<Diff>>,
    pub comparisons: HashMap<DiffKey, u64>,
    pub diff_users: HashMap<DiffKey, usize>,
    pub conflict_cache:
        RefCell<HashMap<DocumentId, ((u64, u64), RepositoryOperation, SourceGitPresentation)>>,
    pub pending_save: Option<PendingSave>,
    active: Option<(Task, Worker)>,
    tasks: VecDeque<Task>,
    remote: Option<RemoteClient>,
    next_comparison: u64,
    filesystem: Option<Receiver<WorkspaceUpdate>>,
    last_refresh: Instant,
    last_event: Option<Instant>,
    known_head: Option<Option<String>>,
    pending_absolute_diffs: Vec<String>,
    optimistic_staging: Option<OptimisticStaging>,
}
impl Default for GitController {
    fn default() -> Self {
        Self {
            root: String::new(),
            transient: false,
            status: None,
            draft: String::new(),
            branch_input: String::new(),
            error: None,
            output: String::new(),
            refresh: true,
            visible: 0,
            diffs: HashMap::new(),
            comparisons: HashMap::new(),
            diff_users: HashMap::new(),
            conflict_cache: RefCell::new(HashMap::new()),
            pending_save: None,
            active: None,
            tasks: VecDeque::new(),
            remote: None,
            next_comparison: 1,
            filesystem: None,
            last_refresh: Instant::now(),
            last_event: None,
            known_head: None,
            pending_absolute_diffs: Vec::new(),
            optimistic_staging: None,
        }
    }
}
impl GitController {
    pub fn capture_directory(&mut self, services: &ModuleServices<'_>) {
        if self.root.is_empty() {
            self.root = services.working_directory.into();
            self.transient = services.project_root.is_empty();
            self.refresh = true;
        }
    }
    pub fn busy(&self) -> bool {
        self.active.is_some() || self.pending_save.is_some() || !self.tasks.is_empty()
    }
    /// Cached reads stay interactive; only saves and writes block another action.
    pub fn mutation_pending(&self) -> bool {
        self.optimistic_staging.is_some()
            || self.pending_save.is_some()
            || self
                .active
                .as_ref()
                .is_some_and(|(task, _)| task.purpose.blocks_actions())
            || self.tasks.iter().any(|task| task.purpose.blocks_actions())
    }
    pub fn writable(&self) -> bool {
        self.status.as_ref().is_some_and(|s| s.writable)
    }
    pub fn staging_state(&self, entry: &StatusEntry) -> StagingState {
        if let Some(pending) = &self.optimistic_staging
            && pending.paths.contains(&entry.path)
        {
            return if pending.stage {
                StagingState::Staged
            } else {
                StagingState::Unstaged
            };
        }
        StagingState::from_entry(entry)
    }
    pub fn repo_root(&self) -> &str {
        self.status
            .as_ref()
            .map(|s| s.root.as_str())
            .filter(|r| !r.is_empty())
            .unwrap_or(&self.root)
    }
    pub fn absolute(&self, path: &str) -> PathBuf {
        Path::new(self.repo_root()).join(path)
    }
    fn document_replacement_error(
        &self,
        operation: &Operation,
        session: &EditorSession,
    ) -> Option<&'static str> {
        if let Operation::Discard { path, .. }
        | Operation::DiscardHunk { path, .. }
        | Operation::DiscardRange { path, .. }
        | Operation::Resolve { path, .. } = operation
            && session
                .document_for_path(&self.absolute(path))
                .and_then(|id| session.snapshot(id).ok())
                .is_some_and(|s| s.dirty)
        {
            return Some("Save or discard editor changes before replacing the working file.");
        }
        if matches!(operation, Operation::Abort)
            && session.document_ids().iter().any(|id| {
                session.snapshot(*id).ok().is_some_and(|s| {
                    s.dirty
                        && !s.path.is_empty()
                        && Path::new(&s.path).starts_with(self.repo_root())
                })
            })
        {
            return Some("Save or discard editor changes before aborting the Git operation.");
        }
        None
    }
    pub fn cancel(&mut self) {
        if let Some((_, worker)) = self.active.take() {
            worker.cancel();
        }
        self.tasks.clear();
        self.pending_save = None;
        self.pending_absolute_diffs.clear();
        self.optimistic_staging = None;
    }
    pub fn cancel_operation(&mut self) {
        if let Some((_, worker)) = &self.active {
            worker.cancel();
        }
        self.pending_save = None;
        self.tasks.clear();
        self.optimistic_staging = None;
        self.refresh = true;
    }
    pub fn request_diff(&mut self, key: DiffKey) {
        let exists =
            self.tasks
                .iter()
                .any(|task| matches!(&task.purpose, Purpose::Diff(k) if k == &key))
                || self.active.as_ref().is_some_and(
                    |(task, _)| matches!(&task.purpose, Purpose::Diff(k) if k == &key),
                );
        if !exists {
            self.tasks.push_back(Task {
                operation: Operation::Diff {
                    path: key.0.clone(),
                    side: key.1,
                },
                purpose: Purpose::Diff(key),
            });
        }
    }
    fn request_status(&mut self) -> bool {
        if !self
            .tasks
            .iter()
            .any(|task| matches!(task.purpose, Purpose::Status))
            && !self
                .active
                .as_ref()
                .is_some_and(|(task, _)| matches!(task.purpose, Purpose::Status))
        {
            self.tasks.push_back(Task {
                operation: Operation::Status,
                purpose: Purpose::Status,
            });
            true
        } else {
            false
        }
    }
    pub fn mutate(
        &mut self,
        operation: Operation,
        session: &EditorSession,
        requests: &mut Vec<HostRequest>,
    ) {
        if !self.writable() {
            self.error =
                Some("Open the repository root as your workspace to change Git state.".into());
            return;
        }
        if self.mutation_pending() {
            self.error = Some("Wait for the current Git operation to finish.".into());
            return;
        }
        if let Some(error) = self.document_replacement_error(&operation, session) {
            self.error = Some(error.into());
            return;
        }
        self.error = None;
        if let Operation::Stage { paths } | Operation::Unstage { paths } = &operation {
            self.optimistic_staging = Some(OptimisticStaging {
                paths: self
                    .status
                    .as_ref()
                    .unwrap()
                    .entries
                    .iter()
                    .filter(|entry| paths.is_empty() || paths.contains(&entry.path))
                    .map(|entry| entry.path.clone())
                    .collect(),
                stage: matches!(operation, Operation::Stage { .. }),
                awaiting_status: false,
            });
        }
        let documents = match &operation {
            Operation::Stage { paths } if !paths.is_empty() => paths
                .iter()
                .filter_map(|path| session.document_for_path(&self.absolute(path)))
                .collect(),
            Operation::StageHunk { path, .. } | Operation::StageRange { path, .. } => session
                .document_for_path(&self.absolute(path))
                .into_iter()
                .collect(),
            Operation::Resolve { path, .. } => session
                .document_for_path(&self.absolute(path))
                .into_iter()
                .collect(),
            Operation::Stage { .. }
            | Operation::CreateBranch { .. }
            | Operation::SwitchBranch { .. }
            | Operation::Pull
            | Operation::Continue => session
                .document_ids()
                .into_iter()
                .filter(|id| {
                    session.snapshot(*id).ok().is_some_and(|s| {
                        !s.path.is_empty() && Path::new(&s.path).starts_with(self.repo_root())
                    })
                })
                .collect(),
            _ => Vec::new(),
        };
        if documents.is_empty() {
            self.queue_mutation(operation);
        } else {
            let token = SaveToken::next();
            self.pending_save = Some(PendingSave {
                token,
                action: operation,
                result: None,
                hunk: None,
            });
            requests.push(HostRequest::SaveDocumentsWithResult {
                recipient: MODULE_ID.into(),
                token,
                documents,
            });
        }
    }
    fn queue_mutation(&mut self, operation: Operation) {
        self.queue_saved_mutation(operation, Vec::new());
    }
    fn queue_saved_mutation(&mut self, operation: Operation, saved: Vec<SavedDocument>) {
        let commit = if let Operation::Commit { message } = &operation {
            Some(message.clone())
        } else {
            None
        };
        self.tasks.push_front(Task {
            operation,
            purpose: Purpose::Mutation { commit, saved },
        });
    }
    pub fn save_result(&mut self, token: SaveToken, result: Result<Vec<SavedDocument>, String>) {
        if let Some(pending) = &mut self.pending_save
            && pending.token == token
        {
            pending.result = Some(result);
        }
    }
    fn accept_save(&mut self, session: &EditorSession) {
        if self
            .pending_save
            .as_ref()
            .is_none_or(|s| s.result.is_none())
        {
            return;
        }
        let pending = self.pending_save.take().unwrap();
        match pending.result.unwrap() {
            Err(error) => {
                self.optimistic_staging = None;
                self.error = Some(format!(
                    "Git operation stopped because saving failed: {error}"
                ))
            }
            Ok(documents) => {
                let valid = saved_documents_unchanged(session, &documents);
                if valid {
                    if let Some((key, action, baseline)) = pending.hunk {
                        if documents
                            .iter()
                            .any(|saved| saved.revision != action.revision)
                        {
                            self.error = Some("An editor draft changed while saving. Review the refreshed diff and choose the changes again.".into());
                            self.refresh = true;
                            return;
                        }
                        self.tasks.push_front(Task {
                            operation: Operation::Diff {
                                path: key.0.clone(),
                                side: key.1,
                            },
                            purpose: Purpose::HunkRefresh {
                                key,
                                action,
                                baseline,
                            },
                        });
                    } else {
                        self.queue_saved_mutation(pending.action, documents);
                    }
                } else {
                    self.optimistic_staging = None;
                    self.error = Some(
                        "A document changed after saving. Review the changes and retry.".into(),
                    );
                    self.refresh = true;
                }
            }
        }
    }
    pub fn tick(
        &mut self,
        services: &mut ModuleServices<'_>,
        requests: &mut Vec<HostRequest>,
    ) -> io::Result<()> {
        if self.root.is_empty() {
            self.root = services.project_root.into();
        }
        if self.root.is_empty() {
            return Ok(());
        }
        let remote = services.documents.remote_client();
        let changed = match (&remote, &self.remote) {
            (Some(a), Some(b)) => !a.same_connection(b),
            (None, None) => false,
            _ => true,
        };
        if changed {
            if self
                .active
                .as_ref()
                .is_some_and(|(task, _)| task.operation.is_mutating())
            {
                self.error = Some("Git connection changed during an operation; its outcome is unknown. Status will refresh.".into());
            }
            self.cancel();
            self.remote = remote;
            self.refresh = true;
            self.filesystem = None;
        }
        if self.filesystem.is_none()
            && let Some(filesystem) = services.resources.take::<WorkspaceFilesystem>()
        {
            self.filesystem = Some(filesystem.subscribe());
        }
        if let Some(events) = &self.filesystem {
            let mut changed = false;
            while let Ok(event) = events.try_recv() {
                changed |= filesystem_changed(&event);
            }
            if changed {
                self.last_event = Some(Instant::now());
            }
        }
        if self
            .last_event
            .is_some_and(|at| at.elapsed() >= Duration::from_millis(150))
        {
            self.refresh = true;
            self.last_event = None;
        }
        self.accept_save(services.documents);
        let result = self.active.as_mut().and_then(|(_, worker)| worker.poll());
        if let Some(result) = result {
            let (task, _) = self.active.take().unwrap();
            match (task.purpose, result) {
                (Purpose::Status, Ok(Output::Status(status))) => {
                    // A status read already in flight when the click occurred
                    // cannot acknowledge the later write.
                    if self
                        .optimistic_staging
                        .as_ref()
                        .is_some_and(|pending| pending.awaiting_status)
                    {
                        self.optimistic_staging = None;
                    }
                    // Unsaved windows operate on the whole discovered repository.
                    // Read it again at that root before enabling mutations, keeping
                    // the repository service's root/scope checks unchanged.
                    let expanding =
                        self.transient && !status.root.is_empty() && self.root != status.root;
                    if expanding {
                        self.root = status.root.clone();
                        self.refresh = true;
                    }
                    if self
                        .known_head
                        .as_ref()
                        .is_some_and(|head| head != &status.head)
                    {
                        services.documents.invalidate_git_baselines();
                    }
                    self.known_head = Some(status.head.clone());
                    self.status = Some(status);
                    if !expanding {
                        for path in std::mem::take(&mut self.pending_absolute_diffs) {
                            self.open_absolute_diff(&path, services, requests);
                        }
                    }
                    self.last_refresh = Instant::now();
                }
                (Purpose::Diff(key), Ok(Output::Diff(diff))) => {
                    let changed = self
                        .diffs
                        .get(&key)
                        .is_none_or(|old| old.snapshot != diff.snapshot);
                    if changed {
                        self.comparisons.insert(key.clone(), self.next_comparison);
                        self.next_comparison = self.next_comparison.wrapping_add(1);
                    }
                    if self.diff_users.contains_key(&key) {
                        self.diffs.insert(key, Arc::new(diff));
                    }
                }
                (
                    Purpose::HunkRefresh {
                        key,
                        action,
                        baseline,
                    },
                    Ok(Output::Diff(diff)),
                ) => {
                    let mut saved_disk = bed_editing::editor_state::EditorState::new();
                    saved_disk.set_from_bytes(&diff.new_bytes);
                    let disk_bytes = saved_disk.join();
                    let unchanged = services
                        .documents
                        .document_for_path(&self.absolute(&key.0))
                        .is_some_and(|document| {
                            services.documents.document_revision(document).ok()
                                == Some(action.revision)
                                && services.documents.snapshot(document).ok().is_some_and(|s| {
                                    !s.dirty
                                        && s.bytes == disk_bytes
                                        && s.utf8_bom == saved_disk.utf8_bom
                                })
                        });
                    if !unchanged {
                        self.error = Some("The document or working file changed while refreshing. Review the diff and choose the changes again.".into());
                    } else if diff.old_bytes != baseline {
                        self.error = Some("The index changed while saving. Review the refreshed diff and choose the changes again.".into());
                    } else {
                        self.queue_mutation(Operation::StageRange {
                            path: key.0.clone(),
                            snapshot: diff.snapshot.clone(),
                            old_range: action.old_range,
                            new_range: action.new_range,
                        });
                    }
                    self.comparisons.insert(key.clone(), self.next_comparison);
                    self.next_comparison = self.next_comparison.wrapping_add(1);
                    self.diffs.insert(key, Arc::new(diff));
                }
                (Purpose::Mutation { commit, .. }, Ok(Output::Done { output })) => {
                    if let Some(pending) = &mut self.optimistic_staging {
                        pending.awaiting_status = true;
                    }
                    if commit
                        .as_ref()
                        .is_some_and(|message| message == &self.draft)
                    {
                        self.draft.clear();
                    }
                    self.output = bound_output(output);
                    self.refresh = true;
                }
                (purpose, Err(error)) => {
                    if matches!(purpose, Purpose::Mutation { .. })
                        || matches!(purpose, Purpose::Status)
                            && self
                                .optimistic_staging
                                .as_ref()
                                .is_some_and(|pending| pending.awaiting_status)
                    {
                        self.optimistic_staging = None;
                    }
                    self.error = Some(error);
                    if matches!(purpose, Purpose::Mutation { .. }) {
                        self.refresh = true;
                    }
                    if matches!(purpose, Purpose::Status) {
                        self.last_refresh = Instant::now();
                    }
                }
                _ => {
                    self.optimistic_staging = None;
                    self.error = Some("Unexpected Git worker response".into());
                }
            }
            requests.push(HostRequest::Invalidate);
        }
        let interval = Duration::from_secs(if self.visible > 0 { 2 } else { 5 });
        // A slow status read must not requeue finished comparisons every frame.
        if (self.refresh || self.last_refresh.elapsed() >= interval) && self.request_status() {
            self.refresh = false;
            self.last_refresh = Instant::now();
            for key in self.diff_users.keys().cloned().collect::<Vec<_>>() {
                self.request_diff(key);
            }
        }
        if self.active.is_none()
            && self.pending_save.is_none()
            && let Some(task) = self.tasks.pop_front()
        {
            if let Purpose::Mutation { saved, .. } = &task.purpose {
                // A background read may have delayed this task after saving.
                let error = if !saved_documents_unchanged(services.documents, saved) {
                    Some("A document changed after saving. Review the changes and retry.")
                } else {
                    self.document_replacement_error(&task.operation, services.documents)
                };
                if let Some(error) = error {
                    self.optimistic_staging = None;
                    self.error = Some(error.into());
                    self.refresh = true;
                    requests.push(HostRequest::Invalidate);
                    return Ok(());
                }
            }
            let worker = Worker::start(
                self.root.clone(),
                task.operation.clone(),
                self.remote.clone(),
            );
            self.active = Some((task, worker));
            requests.push(HostRequest::Invalidate);
        }
        Ok(())
    }
    pub fn progress_label(&self) -> Option<&'static str> {
        self.active
            .iter()
            .map(|(task, _)| &task.operation)
            .chain(self.tasks.iter().map(|task| &task.operation))
            .chain(self.pending_save.iter().map(|pending| &pending.action))
            .find_map(|operation| match operation {
                Operation::Fetch => Some("Fetching remote changes…"),
                Operation::Pull => Some("Pulling remote changes…"),
                Operation::Push => Some("Pushing branch…"),
                _ => None,
            })
            .or_else(|| self.status.is_none().then_some("Discovering repository…"))
    }
    pub fn open_diff(&mut self, path: String, side: DiffSide, requests: &mut Vec<HostRequest>) {
        self.request_diff((path.clone(), side));
        requests.push(HostRequest::OpenPanel {
            panel_type: DIFF_PANEL_ID.into(), document: None,
            state: json!({"path":path,"side":if side == DiffSide::Staged {"staged"} else {"unstaged"}}),
        });
    }
    pub fn open_absolute_diff(
        &mut self,
        path: &str,
        _: &mut ModuleServices<'_>,
        requests: &mut Vec<HostRequest>,
    ) {
        if self.transient && self.status.is_none() {
            self.pending_absolute_diffs.push(path.into());
            self.refresh = true;
            return;
        }
        if self
            .status
            .as_ref()
            .is_some_and(|status| status.root.is_empty())
        {
            self.error = Some("This directory is not in a Git repository.".into());
            return;
        }
        if let Ok(path) = Path::new(path).strip_prefix(self.repo_root()) {
            self.open_diff(path.to_string_lossy().into(), DiffSide::Unstaged, requests);
        } else {
            self.error = Some("The file is outside this repository.".into());
        }
    }
    pub fn hunk_action(
        &mut self,
        key: &DiffKey,
        action: DiffAction,
        session: &EditorSession,
        requests: &mut Vec<HostRequest>,
    ) {
        let Some(diff) = self.diffs.get(key) else {
            return;
        };
        if self.comparisons.get(key).copied() != Some(action.comparison) {
            self.error = Some("This diff changed. Refresh before applying these changes.".into());
            return;
        }
        if action.kind == DiffActionKind::Stage {
            if self.mutation_pending() || !self.writable() {
                self.error = Some("Wait for Git to finish before staging these changes.".into());
                return;
            }
            if let Some(document) = session.document_for_path(&self.absolute(&key.0)) {
                if session.document_revision(document).ok() != Some(action.revision) {
                    self.error = Some(
                        "The document changed after these changes were drawn. Choose them again."
                            .into(),
                    );
                    return;
                }
                let baseline = diff.old_bytes.clone();
                let token = SaveToken::next();
                self.pending_save = Some(PendingSave {
                    token,
                    action: Operation::Stage {
                        paths: vec![key.0.clone()],
                    },
                    result: None,
                    hunk: Some((key.clone(), action, baseline)),
                });
                requests.push(HostRequest::SaveDocumentsWithResult {
                    recipient: MODULE_ID.into(),
                    token,
                    documents: vec![document],
                });
                return;
            }
        }
        let path = key.0.clone();
        let snapshot = diff.snapshot.clone();
        let operation = match action.kind {
            DiffActionKind::Stage => Operation::StageRange {
                path,
                snapshot,
                old_range: action.old_range,
                new_range: action.new_range,
            },
            DiffActionKind::Unstage => Operation::UnstageRange {
                path,
                snapshot,
                old_range: action.old_range,
                new_range: action.new_range,
            },
            DiffActionKind::Discard => Operation::DiscardRange {
                path,
                snapshot,
                old_range: action.old_range,
                new_range: action.new_range,
            },
        };
        self.mutate(operation, session, requests);
    }
    pub fn can_commit(&self) -> bool {
        self.writable()
            && !self.mutation_pending()
            && !self.draft.trim().is_empty()
            && self.status.as_ref().is_some_and(|s| {
                s.entries.iter().all(|e| !e.conflicted)
                    && (s.entries.iter().any(|e| {
                        e.index_status != ' ' && e.index_status != '?' && e.index_status != '.'
                    }) || s.operation == RepositoryOperation::Merge)
            })
    }
}
fn saved_documents_unchanged(session: &EditorSession, documents: &[SavedDocument]) -> bool {
    documents.iter().all(|saved| {
        session.document_revision(saved.document).ok() == Some(saved.revision)
            && session
                .snapshot(saved.document)
                .ok()
                .is_some_and(|s| !s.dirty && s.disk_conflict.is_none())
    })
}
fn filesystem_changed(update: &WorkspaceUpdate) -> bool {
    !update.changes.is_empty()
        || update.indexed_files.is_some()
        || !update.indexed_added.is_empty()
        || !update.indexed_removed.is_empty()
}
fn bound_output(output: String) -> String {
    const LIMIT: usize = 32 * 1024;
    if output.len() <= LIMIT {
        return output;
    }
    let mut start = output.len() - LIMIT;
    while !output.is_char_boundary(start) {
        start += 1;
    }
    format!("…earlier output omitted…\n{}", &output[start..])
}

#[cfg(test)]
mod tests {
    use super::*;
    use bed_document_session::editor_session::ByteEdit;
    use bed_workbench_api::{FileDialogService, ScopedServices, TerminalLaunch, TerminalService};
    use std::{
        fs,
        process::Command,
        sync::{
            atomic::{AtomicBool, AtomicU64, Ordering},
            mpsc,
        },
        thread,
    };

    struct Repository(PathBuf);
    impl Repository {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "bed-git-module-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            let repo = Self(path);
            repo.git(&["init", "-q"]);
            repo.git(&["config", "user.name", "Module test"]);
            repo.git(&["config", "user.email", "module@example.invalid"]);
            repo.git(&["config", "commit.gpgsign", "false"]);
            repo
        }
        fn git(&self, args: &[&str]) {
            let output = Command::new("git")
                .args(args)
                .current_dir(&self.0)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        fn controller(&self) -> GitController {
            let Output::Status(status) = bed_git::execute(&self.0, &Operation::Status).unwrap()
            else {
                panic!()
            };
            GitController {
                root: self.0.to_string_lossy().into(),
                status: Some(status),
                refresh: false,
                ..Default::default()
            }
        }
    }
    impl Drop for Repository {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    struct Terminals;
    impl TerminalService for Terminals {
        fn spawn(&mut self, _: TerminalLaunch) -> io::Result<(u64, u32)> {
            Err(io::Error::other("unused"))
        }
        fn stop(&mut self, _: u64) {}
        fn release(&mut self, _: u64) {}
    }
    struct Dialogs;
    impl FileDialogService for Dialogs {
        fn pick_file(&mut self, _: &Path, _: &[&str]) -> Option<PathBuf> {
            None
        }
    }
    fn tick_once(controller: &mut GitController, session: &mut EditorSession) {
        let root = controller.root.clone();
        let mut terminals = Terminals;
        let mut dialogs = Dialogs;
        let mut services = ModuleServices {
            documents: session,
            terminals: &mut terminals,
            dialogs: &mut dialogs,
            project_root: &root,
            working_directory: &root,
            active_view: None,
            resources: ScopedServices::default(),
            settings_ui: None,
        };
        controller.tick(&mut services, &mut vec![]).unwrap();
    }
    /// A read remains in flight until the test explicitly publishes its result.
    fn hold_read(
        controller: &mut GitController,
        operation: Operation,
        purpose: Purpose,
    ) -> mpsc::Sender<Result<Output, String>> {
        let (sender, result) = mpsc::channel();
        controller.active = Some((
            Task { operation, purpose },
            Worker::Remote {
                result,
                cancelled: Arc::new(AtomicBool::new(false)),
            },
        ));
        sender
    }
    fn complete(controller: &mut GitController, session: &mut EditorSession) {
        let root = controller.root.clone();
        let mut terminals = Terminals;
        let mut dialogs = Dialogs;
        let mut services = ModuleServices {
            documents: session,
            terminals: &mut terminals,
            dialogs: &mut dialogs,
            project_root: &root,
            working_directory: &root,
            active_view: None,
            resources: ScopedServices::default(),
            settings_ui: None,
        };
        let start = Instant::now();
        loop {
            controller.tick(&mut services, &mut vec![]).unwrap();
            if !controller.busy() {
                return;
            }
            assert!(
                start.elapsed() < Duration::from_secs(15),
                "Git worker did not finish"
            );
            thread::sleep(Duration::from_millis(5));
        }
    }
    #[test]
    fn staging_is_optimistic_until_a_fresh_status_confirms_the_write() {
        let repo = Repository::new();
        fs::write(repo.0.join("a"), b"base\n").unwrap();
        repo.git(&["add", "a"]);
        repo.git(&["commit", "-qm", "base"]);
        fs::write(repo.0.join("a"), b"staged\n").unwrap();
        repo.git(&["add", "a"]);
        fs::write(repo.0.join("a"), b"working\n").unwrap();
        fs::write(repo.0.join("new"), b"new\n").unwrap();
        let mut controller = repo.controller();
        let before = controller.status.clone().unwrap();
        let entry = before
            .entries
            .iter()
            .find(|entry| entry.path == "a")
            .unwrap();
        assert_eq!(controller.staging_state(entry), StagingState::Partial);
        let mut session = EditorSession::default();
        let read = hold_read(&mut controller, Operation::Status, Purpose::Status);
        controller.mutate(
            Operation::Stage {
                paths: vec!["a".into(), "new".into()],
            },
            &session,
            &mut vec![],
        );
        assert_eq!(
            controller.status.as_ref().unwrap().entries,
            before.entries,
            "optimism does not overwrite repository facts"
        );
        assert!(
            before
                .entries
                .iter()
                .all(|entry| controller.staging_state(entry) == StagingState::Staged)
        );
        assert_eq!(
            controller.progress_label(),
            None,
            "local changes stay quiet"
        );
        read.send(Ok(Output::Status(before.clone()))).unwrap();
        tick_once(&mut controller, &mut session);
        assert!(
            before
                .entries
                .iter()
                .all(|entry| controller.staging_state(entry) == StagingState::Staged),
            "an older in-flight read cannot undo the checkbox update"
        );
        complete(&mut controller, &mut session);
        assert!(!controller.mutation_pending());
        let staged = controller.status.as_ref().unwrap().entries.clone();
        assert!(
            staged
                .iter()
                .all(|entry| controller.staging_state(entry) == StagingState::Staged)
        );
        controller.mutate(
            Operation::Unstage {
                paths: vec!["a".into(), "new".into()],
            },
            &session,
            &mut vec![],
        );
        assert!(
            staged
                .iter()
                .all(|entry| controller.staging_state(entry) == StagingState::Unstaged)
        );
        complete(&mut controller, &mut session);
        assert!(!controller.mutation_pending());
        assert!(
            controller
                .status
                .as_ref()
                .unwrap()
                .entries
                .iter()
                .all(|entry| controller.staging_state(entry) == StagingState::Unstaged)
        );
        assert!(controller.error.is_none(), "{:?}", controller.error);
    }
    #[test]
    fn failed_or_cancelled_staging_restores_the_checkbox_state() {
        let repo = Repository::new();
        fs::write(repo.0.join("new"), b"new\n").unwrap();
        let mut controller = repo.controller();
        let entry = controller.status.as_ref().unwrap().entries[0].clone();
        let mut session = EditorSession::default();
        fs::write(repo.0.join(".git/index.lock"), b"locked").unwrap();
        controller.mutate(
            Operation::Stage {
                paths: vec![entry.path.clone()],
            },
            &session,
            &mut vec![],
        );
        assert_eq!(controller.staging_state(&entry), StagingState::Staged);
        complete(&mut controller, &mut session);
        assert!(controller.error.is_some());
        assert_eq!(controller.staging_state(&entry), StagingState::Unstaged);
        fs::remove_file(repo.0.join(".git/index.lock")).unwrap();
        controller.mutate(
            Operation::Stage {
                paths: vec![entry.path.clone()],
            },
            &session,
            &mut vec![],
        );
        assert_eq!(controller.staging_state(&entry), StagingState::Staged);
        controller.cancel_operation();
        assert_eq!(controller.staging_state(&entry), StagingState::Unstaged);
        assert!(!controller.mutation_pending());
    }
    #[test]
    fn remote_operations_report_progress_while_background_reads_stay_quiet() {
        let repo = Repository::new();
        let mut controller = repo.controller();
        let _read = hold_read(&mut controller, Operation::Status, Purpose::Status);
        assert_eq!(controller.progress_label(), None);
        controller.mutate(Operation::Push, &EditorSession::default(), &mut vec![]);
        assert_eq!(controller.progress_label(), Some("Pushing branch…"));
        controller.cancel_operation();
        assert_eq!(controller.progress_label(), None);
    }
    #[test]
    fn unsaved_directory_discovers_the_repository_and_resolves_conflicts() {
        let repo = Repository::new();
        let file = repo.0.join("nested/note.txt");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, b"base\n").unwrap();
        repo.git(&["add", "."]);
        repo.git(&["commit", "-qm", "base"]);
        repo.git(&["checkout", "-qb", "incoming"]);
        fs::write(&file, b"incoming\n").unwrap();
        repo.git(&["commit", "-qam", "incoming"]);
        repo.git(&["checkout", "-qb", "current", "HEAD~"]);
        fs::write(&file, b"current\n").unwrap();
        repo.git(&["commit", "-qam", "current"]);
        assert!(
            !Command::new("git")
                .args(["merge", "incoming"])
                .current_dir(&repo.0)
                .output()
                .unwrap()
                .status
                .success()
        );

        let directory = file.parent().unwrap().canonicalize().unwrap();
        let mut session = EditorSession::default();
        let mut terminals = Terminals;
        let mut dialogs = Dialogs;
        let mut services = ModuleServices {
            documents: &mut session,
            terminals: &mut terminals,
            dialogs: &mut dialogs,
            project_root: "",
            working_directory: directory.to_str().unwrap(),
            active_view: None,
            resources: ScopedServices::default(),
            settings_ui: None,
        };
        let mut controller = GitController::default();
        controller.capture_directory(&services);
        let mut requests = vec![];
        controller.open_absolute_diff(
            file.canonicalize().unwrap().to_str().unwrap(),
            &mut services,
            &mut requests,
        );
        assert!(requests.is_empty(), "Diff waits for repository discovery");
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            controller.tick(&mut services, &mut requests).unwrap();
            if !controller.busy() {
                break;
            }
            assert!(Instant::now() < deadline, "Git discovery did not finish");
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            controller.root,
            repo.0.canonicalize().unwrap().to_string_lossy()
        );
        assert!(controller.writable(), "{:?}", controller.error);
        assert!(requests.iter().any(|request| matches!(request,
            HostRequest::OpenPanel { panel_type, state, .. }
                if panel_type == DIFF_PANEL_ID && state["path"] == "nested/note.txt")));
        controller.mutate(
            Operation::Resolve {
                path: "nested/note.txt".into(),
                choice: bed_git::ConflictChoice::Incoming,
            },
            services.documents,
            &mut requests,
        );
        drop(services);
        complete(&mut controller, &mut session);
        assert_eq!(fs::read(&file).unwrap(), b"incoming\n");
        controller.mutate(
            Operation::Stage {
                paths: vec!["nested/note.txt".into()],
            },
            &session,
            &mut requests,
        );
        complete(&mut controller, &mut session);
        let output = Command::new("git")
            .args(["ls-files", "-u"])
            .current_dir(&repo.0)
            .output()
            .unwrap();
        assert!(output.status.success() && output.stdout.is_empty());
        assert!(session.options().project_root.is_none());
        assert!(!session.options().persistent_history);
    }
    #[test]
    fn truncated_command_output_retains_utf8_boundaries_and_latest_output() {
        let output = bound_output(format!("{}last line", "é".repeat(40_000)));
        assert!(output.ends_with("last line"));
        assert!(output.len() < 33_000);
    }
    #[test]
    fn duplicate_diff_requests_share_one_worker_task() {
        let mut controller = GitController::default();
        controller.request_diff(("a.rs".into(), DiffSide::Unstaged));
        controller.request_diff(("a.rs".into(), DiffSide::Unstaged));
        assert_eq!(controller.tasks.len(), 1);
    }
    #[test]
    fn cached_status_and_diff_refreshes_keep_commit_and_mutation_actions_available() {
        let repo = Repository::new();
        fs::write(repo.0.join("a"), b"staged\n").unwrap();
        repo.git(&["add", "a"]);
        let mut controller = repo.controller();
        controller.draft = "Ready to commit".into();
        let key: DiffKey = ("a".into(), DiffSide::Staged);
        let Output::Diff(diff) = bed_git::execute(
            &repo.0,
            &Operation::Diff {
                path: key.0.clone(),
                side: key.1,
            },
        )
        .unwrap() else {
            panic!()
        };
        controller.diffs.insert(key.clone(), Arc::new(diff));
        let status_result = hold_read(&mut controller, Operation::Status, Purpose::Status);
        controller.request_diff(key.clone());
        assert!(
            controller.busy(),
            "Background workers still count for full readiness"
        );
        assert!(!controller.mutation_pending());
        assert!(controller.can_commit());
        assert_eq!(controller.progress_label(), None);

        controller.tasks.clear();
        controller.active.take();
        drop(status_result);
        let _diff_result = hold_read(
            &mut controller,
            Operation::Diff {
                path: key.0.clone(),
                side: key.1,
            },
            Purpose::Diff(key),
        );
        assert!(controller.request_status());
        assert!(!controller.mutation_pending());
        assert!(controller.can_commit());
        assert_eq!(controller.progress_label(), None);
    }
    #[test]
    fn stage_during_a_read_takes_priority_over_queued_diffs_and_rejects_a_second_write() {
        let repo = Repository::new();
        fs::write(repo.0.join("a"), b"new\n").unwrap();
        let mut session = EditorSession::default();
        let mut controller = repo.controller();
        let status = controller.status.clone().unwrap();
        let read = hold_read(&mut controller, Operation::Status, Purpose::Status);
        controller.request_diff(("a".into(), DiffSide::Unstaged));
        controller.mutate(
            Operation::Stage {
                paths: vec!["a".into()],
            },
            &session,
            &mut vec![],
        );
        assert!(controller.error.is_none());
        assert!(controller.mutation_pending());
        assert!(matches!(
            controller.tasks.front().unwrap().operation,
            Operation::Stage { .. }
        ));
        assert!(matches!(
            controller.tasks.back().unwrap().purpose,
            Purpose::Diff(_)
        ));
        controller.mutate(
            Operation::Unstage {
                paths: vec!["a".into()],
            },
            &session,
            &mut vec![],
        );
        assert!(controller.error.as_ref().unwrap().contains("Wait for"));
        assert_eq!(
            controller
                .tasks
                .iter()
                .filter(|task| matches!(task.purpose, Purpose::Mutation { .. }))
                .count(),
            1
        );
        // Dismissing that rejected second action does not cancel the first one.
        controller.error = None;
        read.send(Ok(Output::Status(status))).unwrap();
        tick_once(&mut controller, &mut session);
        assert!(matches!(
            controller.active.as_ref().unwrap().0.operation,
            Operation::Stage { .. }
        ));
        complete(&mut controller, &mut session);
        assert!(controller.error.is_none());
        let indexed = Command::new("git")
            .args(["ls-files", "--cached"])
            .current_dir(&repo.0)
            .output()
            .unwrap();
        assert_eq!(indexed.stdout, b"a\n");
    }
    #[test]
    fn pending_saves_and_hunk_preparation_block_actions_even_though_preparation_reads_git() {
        let mut controller = GitController::default();
        controller.pending_save = Some(PendingSave {
            token: SaveToken::next(),
            action: Operation::Stage { paths: vec![] },
            result: None,
            hunk: None,
        });
        assert!(controller.mutation_pending());
        controller.pending_save = None;
        let key = ("a".into(), DiffSide::Unstaged);
        let prepare = || Purpose::HunkRefresh {
            key: key.clone(),
            action: DiffAction {
                kind: DiffActionKind::Stage,
                comparison: 1,
                old_range: 0..1,
                new_range: 0..1,
                revision: (1, 1),
            },
            baseline: b"old\n".to_vec(),
        };
        controller.tasks.push_back(Task {
            operation: Operation::Diff {
                path: key.0.clone(),
                side: key.1,
            },
            purpose: prepare(),
        });
        assert!(controller.mutation_pending());
        controller.tasks.clear();
        let _result = hold_read(
            &mut controller,
            Operation::Diff {
                path: key.0.clone(),
                side: key.1,
            },
            prepare(),
        );
        assert!(controller.mutation_pending());
    }
    #[test]
    fn queued_saved_stage_revalidates_document_after_the_background_read_finishes() {
        let repo = Repository::new();
        fs::write(repo.0.join("a"), b"saved\n").unwrap();
        let mut session = EditorSession::default();
        let document = session.open_file(&repo.0.join("a")).unwrap();
        let mut controller = repo.controller();
        let status = controller.status.clone().unwrap();
        let read = hold_read(&mut controller, Operation::Status, Purpose::Status);
        let mut requests = vec![];
        controller.mutate(
            Operation::Stage {
                paths: vec!["a".into()],
            },
            &session,
            &mut requests,
        );
        let HostRequest::SaveDocumentsWithResult { token, .. } = requests.pop().unwrap() else {
            panic!()
        };
        session.save(document).unwrap();
        let revision = session.document_revision(document).unwrap();
        controller.save_result(token, Ok(vec![SavedDocument { document, revision }]));
        controller.accept_save(&session);
        assert!(
            matches!(&controller.tasks.front().unwrap().purpose, Purpose::Mutation { saved, .. } if saved.len() == 1)
        );
        session
            .apply_edits(
                document,
                revision,
                &[ByteEdit {
                    range: 0..0,
                    bytes: b"later edit\n".to_vec(),
                }],
            )
            .unwrap();
        read.send(Ok(Output::Status(status))).unwrap();
        tick_once(&mut controller, &mut session);
        assert!(controller.active.is_none());
        assert!(
            controller
                .error
                .as_ref()
                .unwrap()
                .contains("changed after saving")
        );
        assert!(
            !controller
                .tasks
                .iter()
                .any(|task| matches!(task.purpose, Purpose::Mutation { .. }))
        );
        assert_eq!(fs::read(repo.0.join("a")).unwrap(), b"saved\n");
        let indexed = Command::new("git")
            .args(["ls-files", "--cached"])
            .current_dir(&repo.0)
            .output()
            .unwrap();
        assert!(indexed.stdout.is_empty());
    }
    #[test]
    fn overdue_polling_does_not_requeue_finished_diffs_while_status_is_pending() {
        let repo = Repository::new();
        let mut controller = repo.controller();
        let mut session = EditorSession::default();
        let active_key = ("active".into(), DiffSide::Unstaged);
        let completed_key = ("completed".into(), DiffSide::Unstaged);
        controller.diff_users.insert(active_key.clone(), 1);
        controller.diff_users.insert(completed_key.clone(), 1);
        let _read = hold_read(
            &mut controller,
            Operation::Diff {
                path: active_key.0.clone(),
                side: active_key.1,
            },
            Purpose::Diff(active_key),
        );
        assert!(controller.request_status());
        // This is the state after one diff completed in a slow refresh cycle.
        controller.last_refresh = Instant::now() - Duration::from_secs(30);
        for _ in 0..4 {
            tick_once(&mut controller, &mut session);
            assert_eq!(controller.tasks.len(), 1);
            assert!(matches!(
                controller.tasks.front().unwrap().purpose,
                Purpose::Status
            ));
            assert!(
                !controller.tasks.iter().any(
                    |task| matches!(&task.purpose, Purpose::Diff(key) if key == &completed_key)
                )
            );
        }
    }
    #[test]
    fn filesystem_metadata_updates_do_not_schedule_git_refresh_but_file_deltas_do() {
        let metadata = WorkspaceUpdate {
            root: "/workspace".into(),
            generation: 42,
            ready: true,
            degraded: Some("Watcher reconnecting".into()),
            dirty_directories: vec!["/workspace/src".into()],
            directories: vec![bed_remote::DirectoryListing {
                path: "/workspace/src".into(),
                entries: vec![],
                warning: None,
            }],
            ..Default::default()
        };
        assert!(!filesystem_changed(&metadata));
        for update in [
            WorkspaceUpdate {
                changes: vec![bed_remote::FilesystemChange::Modified {
                    path: "/workspace/a".into(),
                }],
                ..Default::default()
            },
            WorkspaceUpdate {
                indexed_files: Some(vec![]),
                ..Default::default()
            },
            WorkspaceUpdate {
                indexed_added: vec!["/workspace/new".into()],
                ..Default::default()
            },
            WorkspaceUpdate {
                indexed_removed: vec!["/workspace/old".into()],
                ..Default::default()
            },
        ] {
            assert!(filesystem_changed(&update));
        }
    }
    #[test]
    fn discard_queued_during_a_read_rechecks_unsaved_edits_before_replacing_disk() {
        let repo = Repository::new();
        fs::write(repo.0.join("a"), b"base\n").unwrap();
        repo.git(&["add", "a"]);
        repo.git(&["commit", "-qm", "initial"]);
        fs::write(repo.0.join("a"), b"working\n").unwrap();
        let mut session = EditorSession::default();
        let document = session.open_file(&repo.0.join("a")).unwrap();
        let mut controller = repo.controller();
        let status = controller.status.clone().unwrap();
        let Output::Diff(diff) = bed_git::execute(
            &repo.0,
            &Operation::Diff {
                path: "a".into(),
                side: DiffSide::Unstaged,
            },
        )
        .unwrap() else {
            panic!()
        };
        let read = hold_read(&mut controller, Operation::Status, Purpose::Status);
        controller.mutate(
            Operation::Discard {
                path: "a".into(),
                snapshot: diff.snapshot,
            },
            &session,
            &mut vec![],
        );
        assert!(controller.error.is_none());
        let revision = session.document_revision(document).unwrap();
        session
            .apply_edits(
                document,
                revision,
                &[ByteEdit {
                    range: 0..0,
                    bytes: b"unsaved\n".to_vec(),
                }],
            )
            .unwrap();
        read.send(Ok(Output::Status(status))).unwrap();
        tick_once(&mut controller, &mut session);
        assert!(controller.active.is_none());
        assert!(controller.error.is_some());
        assert_eq!(fs::read(repo.0.join("a")).unwrap(), b"working\n");
        assert_eq!(
            session.snapshot(document).unwrap().bytes,
            b"unsaved\nworking\n"
        );
    }
    #[test]
    fn abort_queued_during_a_read_rechecks_all_workspace_documents_before_dispatch() {
        let repo = Repository::new();
        fs::write(repo.0.join("a"), b"clean\n").unwrap();
        let mut session = EditorSession::default();
        let document = session.open_file(&repo.0.join("a")).unwrap();
        let mut controller = repo.controller();
        let status = controller.status.clone().unwrap();
        let read = hold_read(&mut controller, Operation::Status, Purpose::Status);
        controller.mutate(Operation::Abort, &session, &mut vec![]);
        assert!(controller.error.is_none());
        let revision = session.document_revision(document).unwrap();
        session
            .apply_edits(
                document,
                revision,
                &[ByteEdit {
                    range: 0..0,
                    bytes: b"new edits\n".to_vec(),
                }],
            )
            .unwrap();
        read.send(Ok(Output::Status(status))).unwrap();
        tick_once(&mut controller, &mut session);
        assert!(controller.active.is_none());
        assert!(controller.error.as_ref().unwrap().contains("aborting"));
        assert_eq!(fs::read(repo.0.join("a")).unwrap(), b"clean\n");
        assert_eq!(
            session.snapshot(document).unwrap().bytes,
            b"new edits\nclean\n"
        );
    }
    #[test]
    fn unmatched_save_acknowledgements_cannot_start_operations() {
        let mut controller = GitController::default();
        let token = SaveToken::next();
        controller.pending_save = Some(PendingSave {
            token,
            action: Operation::Stage { paths: vec![] },
            result: None,
            hunk: None,
        });
        controller.save_result(SaveToken::next(), Ok(vec![]));
        assert!(controller.pending_save.as_ref().unwrap().result.is_none());
    }
    #[test]
    fn failed_save_and_edits_after_save_both_prevent_staging() {
        let repo = Repository::new();
        fs::write(repo.0.join("a"), b"original\n").unwrap();
        let mut session = EditorSession::default();
        let document = session.open_file(&repo.0.join("a")).unwrap();
        let mut controller = repo.controller();
        let mut requests = vec![];
        controller.mutate(
            Operation::Stage {
                paths: vec!["a".into()],
            },
            &session,
            &mut requests,
        );
        let HostRequest::SaveDocumentsWithResult { token, .. } = requests.pop().unwrap() else {
            panic!()
        };
        controller.save_result(token, Err("CSV draft invalid".into()));
        controller.accept_save(&session);
        assert!(!controller.mutation_pending());
        assert!(controller.tasks.is_empty());
        assert!(
            controller
                .error
                .as_ref()
                .unwrap()
                .contains("CSV draft invalid")
        );

        controller.mutate(
            Operation::Stage {
                paths: vec!["a".into()],
            },
            &session,
            &mut requests,
        );
        let HostRequest::SaveDocumentsWithResult { token, .. } = requests.pop().unwrap() else {
            panic!()
        };
        let revision = session.document_revision(document).unwrap();
        controller.save_result(token, Ok(vec![SavedDocument { document, revision }]));
        session
            .apply_edits(
                document,
                revision,
                &[ByteEdit {
                    range: 0..0,
                    bytes: b"later\n".to_vec(),
                }],
            )
            .unwrap();
        controller.accept_save(&session);
        assert!(controller.tasks.is_empty());
        assert!(!controller.mutation_pending());
        assert!(
            controller
                .error
                .as_ref()
                .unwrap()
                .contains("changed after saving")
        );
        assert!(
            Command::new("git")
                .args(["ls-files", "--cached"])
                .current_dir(&repo.0)
                .output()
                .unwrap()
                .stdout
                .is_empty()
        );
    }
    #[test]
    fn edited_hunk_saves_then_refreshes_and_stages_only_that_hunk() {
        let repo = Repository::new();
        fs::write(
            repo.0.join("a"),
            b"one\ntwo\nthree\nfour\nfive\nsix\nseven\n",
        )
        .unwrap();
        repo.git(&["add", "a"]);
        repo.git(&["commit", "-qm", "initial"]);
        fs::write(
            repo.0.join("a"),
            b"ONE\ntwo\nthree\nfour\nfive\nsix\nSEVEN\n",
        )
        .unwrap();
        let mut session = EditorSession::default();
        let document = session.open_file(&repo.0.join("a")).unwrap();
        let mut controller = repo.controller();
        let Output::Diff(diff) = bed_git::execute(
            &repo.0,
            &Operation::Diff {
                path: "a".into(),
                side: DiffSide::Unstaged,
            },
        )
        .unwrap() else {
            panic!()
        };
        let key = ("a".into(), DiffSide::Unstaged);
        controller.diffs.insert(key.clone(), Arc::new(diff));
        controller.comparisons.insert(key.clone(), 7);
        session
            .apply_edits(
                document,
                session.document_revision(document).unwrap(),
                &[ByteEdit {
                    range: 0..3,
                    bytes: b"EDITED".to_vec(),
                }],
            )
            .unwrap();
        let action = DiffAction {
            kind: DiffActionKind::Stage,
            comparison: 7,
            old_range: 0..1,
            new_range: 0..1,
            revision: session.document_revision(document).unwrap(),
        };
        let mut requests = vec![];
        controller.hunk_action(&key, action, &session, &mut requests);
        let HostRequest::SaveDocumentsWithResult {
            token, documents, ..
        } = requests.pop().unwrap()
        else {
            panic!()
        };
        assert_eq!(documents, vec![document]);
        assert!(controller.tasks.is_empty());
        session.save(document).unwrap();
        controller.save_result(
            token,
            Ok(vec![SavedDocument {
                document,
                revision: session.document_revision(document).unwrap(),
            }]),
        );
        complete(&mut controller, &mut session);
        assert!(controller.error.is_none(), "{:?}", controller.error);
        let staged = Command::new("git")
            .args(["show", ":a"])
            .current_dir(&repo.0)
            .output()
            .unwrap();
        assert_eq!(
            staged.stdout,
            b"EDITED\ntwo\nthree\nfour\nfive\nsix\nseven\n"
        );
        assert!(
            session
                .snapshot(document)
                .unwrap()
                .bytes
                .ends_with(b"SEVEN\n")
        );
    }
    #[test]
    fn external_disk_edit_after_save_cannot_replace_the_reviewed_hunk() {
        let repo = Repository::new();
        fs::write(repo.0.join("a"), b"old\n").unwrap();
        repo.git(&["add", "a"]);
        repo.git(&["commit", "-qm", "initial"]);
        fs::write(repo.0.join("a"), b"reviewed\n").unwrap();
        // No monitor can update the document before the Git worker reads disk.
        let mut session = EditorSession::default();
        let document = session.open_file(&repo.0.join("a")).unwrap();
        let mut controller = repo.controller();
        let Output::Diff(diff) = bed_git::execute(
            &repo.0,
            &Operation::Diff {
                path: "a".into(),
                side: DiffSide::Unstaged,
            },
        )
        .unwrap() else {
            panic!()
        };
        let key = ("a".into(), DiffSide::Unstaged);
        controller.diffs.insert(key.clone(), Arc::new(diff));
        controller.comparisons.insert(key.clone(), 1);
        let revision = session.document_revision(document).unwrap();
        let mut requests = vec![];
        controller.hunk_action(
            &key,
            DiffAction {
                kind: DiffActionKind::Stage,
                comparison: 1,
                old_range: 0..1,
                new_range: 0..1,
                revision,
            },
            &session,
            &mut requests,
        );
        let HostRequest::SaveDocumentsWithResult { token, .. } = requests.pop().unwrap() else {
            panic!()
        };
        session.save(document).unwrap();
        controller.save_result(token, Ok(vec![SavedDocument { document, revision }]));
        controller.accept_save(&session);
        fs::write(repo.0.join("a"), b"unreviewed external edit\n").unwrap();
        complete(&mut controller, &mut session);
        assert!(
            controller
                .error
                .as_ref()
                .unwrap()
                .contains("working file changed")
        );
        assert_eq!(session.document_revision(document).unwrap(), revision);
        assert_eq!(session.snapshot(document).unwrap().bytes, b"reviewed\n");
        let staged = Command::new("git")
            .args(["show", ":a"])
            .current_dir(&repo.0)
            .output()
            .unwrap();
        assert_eq!(staged.stdout, b"old\n");
    }
    #[test]
    fn locally_grouped_range_with_context_stages_exactly_the_reviewed_slice() {
        let repo = Repository::new();
        fs::write(repo.0.join("a"), b"a\nkeep\nb\noutside\nc\n").unwrap();
        repo.git(&["add", "a"]);
        repo.git(&["commit", "-qm", "initial"]);
        fs::write(repo.0.join("a"), b"A\nkeep\nB\noutside\nC\n").unwrap();
        let mut session = EditorSession::default();
        let document = session.open_file(&repo.0.join("a")).unwrap();
        let mut controller = repo.controller();
        let Output::Diff(diff) = bed_git::execute(
            &repo.0,
            &Operation::Diff {
                path: "a".into(),
                side: DiffSide::Unstaged,
            },
        )
        .unwrap() else {
            panic!()
        };
        assert_eq!(diff.hunks.len(), 3);
        let key = ("a".into(), DiffSide::Unstaged);
        controller.diffs.insert(key.clone(), Arc::new(diff));
        controller.comparisons.insert(key.clone(), 1);
        let revision = session.document_revision(document).unwrap();
        let mut requests = vec![];
        // A local preview may group two Git hunks with unchanged context.
        controller.hunk_action(
            &key,
            DiffAction {
                kind: DiffActionKind::Stage,
                comparison: 1,
                old_range: 0..3,
                new_range: 0..3,
                revision,
            },
            &session,
            &mut requests,
        );
        let HostRequest::SaveDocumentsWithResult { token, .. } = requests.pop().unwrap() else {
            panic!()
        };
        session.save(document).unwrap();
        controller.save_result(token, Ok(vec![SavedDocument { document, revision }]));
        complete(&mut controller, &mut session);
        assert!(controller.error.is_none(), "{:?}", controller.error);
        let staged = Command::new("git")
            .args(["show", ":a"])
            .current_dir(&repo.0)
            .output()
            .unwrap();
        assert_eq!(staged.stdout, b"A\nkeep\nB\noutside\nc\n");
        assert_eq!(
            fs::read(repo.0.join("a")).unwrap(),
            b"A\nkeep\nB\noutside\nC\n"
        );
    }
    #[cfg(unix)]
    #[test]
    fn failed_commit_preserves_message_and_success_clears_only_submitted_draft() {
        use std::os::unix::fs::PermissionsExt;
        let repo = Repository::new();
        fs::write(repo.0.join("a"), b"text\n").unwrap();
        repo.git(&["add", "a"]);
        let hook = repo.0.join(".git/hooks/pre-commit");
        fs::write(&hook, "#!/bin/sh\necho hook-failed >&2\nexit 1\n").unwrap();
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
        let mut controller = repo.controller();
        controller.draft = "Keep this message".into();
        let mut session = EditorSession::default();
        controller.mutate(
            Operation::Commit {
                message: controller.draft.clone(),
            },
            &session,
            &mut vec![],
        );
        complete(&mut controller, &mut session);
        assert_eq!(controller.draft, "Keep this message");
        assert!(controller.error.as_ref().unwrap().contains("hook-failed"));
        fs::remove_file(hook).unwrap();
        controller.mutate(
            Operation::Commit {
                message: controller.draft.clone(),
            },
            &session,
            &mut vec![],
        );
        controller.draft = "Draft for the next commit".into();
        complete(&mut controller, &mut session);
        assert!(controller.error.is_none());
        assert_eq!(controller.draft, "Draft for the next commit");
    }
}
