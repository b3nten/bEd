//! Git for workbench apps: the Git panel (changes, staging, committing,
//! branches, remotes, conflicts) and diff panels, over the workbench's
//! `Root`. One `GitSession` owns the repository's state and the operation in
//! flight; panels only show it. All Git and file I/O runs on workers.
pub mod diff;
mod panel;
mod presentation;
mod status;
use presentation::{ChangeSet, DirectoryKey, Group, RepositoryView, Row};
pub use presentation::{GitPanelOptions, GroupBy, StagingStyle, ViewMode};
use std::collections::{BTreeSet, VecDeque};

use crate::{CommitMode, DiffSide, Job, Operation, Output, StatusEntry};
use bedtime_workbench::{status::StatusAppExt, *};
use bevy::prelude::*;
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

pub use diff::DiffPanel;
pub use panel::{GitHost, GitPanel};

/// Whether a Git operation is under way.
pub fn has_pending_operation(world: &World) -> bool {
    world
        .get_resource::<GitSession>()
        .is_some_and(|s| s.pending.is_some() || !s.queue.is_empty())
}

/// Open a Git panel: a new one, in the focused area.
pub struct ShowGit;
impl Action for ShowGit {
    const KEY: &'static str = "git.show";
    const LABEL: &'static str = "Git";
}

/// The Git and diff panels, and the session behind them.
pub struct GitPlugin;
impl Plugin for GitPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<GitPanel>()
            .add_action_handler::<ShowGit>(Handler::new(open_new::<GitPanel>))
            .add_menu_item::<ShowGit>(StandardMenu::View, "panels")
            .register_type::<DiffPanel>()
            .add_panel_status::<GitPanel>(status::item)
            .add_systems(Update, update)
            // The session is the workspace folder's; the next one makes its own.
            .add_systems(OnWorkspaceClose, |world: &mut World| {
                world.remove_resource::<GitSession>();
            });
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Staging {
    Unstaged,
    Partial,
    Staged,
}

fn staging(entry: &StatusEntry) -> Staging {
    if entry.conflicted || matches!(entry.index_status, ' ' | '.' | '?') {
        Staging::Unstaged
    } else if !matches!(entry.worktree_status, ' ' | '.') {
        Staging::Partial
    } else {
        Staging::Staged
    }
}

#[derive(Resource)]
pub struct GitSession {
    root: PathBuf,
    view: Option<Arc<RepositoryView>>,
    view_epoch: u64,
    options: GitPanelOptions,
    collapsed: BTreeSet<DirectoryKey>,
    projection: Option<(u64, ManagedTask<RepositoryView>)>,
    scan: Option<(u64, ManagedTask<Result<RepositoryView, String>>)>,
    epoch: u64,
    refresh: bool,
    last_scan: Instant,
    pending: Option<Pending>,
    confirmation: Option<Operation>,
    draft: String,
    commit_mode: CommitMode,
    branch_input: String,
    error: Option<String>,
    output: String,
    scans: u64,
    had_panels: bool,
    /// What was asked for while Git was busy, to run in turn.
    queue: VecDeque<Request>,
    /// Next steps offered with `error`.
    remedies: Vec<(&'static str, Operation)>,
    /// `error` is a failed status scan's: the next good scan clears it.
    scan_failed: bool,
}

/// Something asked of Git from the panel.
#[derive(Clone, PartialEq, Eq)]
enum Request {
    Operation(Operation),
    /// Review a file's working changes, then ask to discard them.
    Discard(String),
}

impl GitSession {
    fn new(root: PathBuf) -> Self {
        Self {
            root,
            view: None,
            view_epoch: 0,
            options: GitPanelOptions::default(),
            collapsed: BTreeSet::new(),
            projection: None,
            scan: None,
            epoch: 0,
            refresh: true,
            last_scan: Instant::now(),
            pending: None,
            confirmation: None,
            draft: String::new(),
            commit_mode: CommitMode::default(),
            branch_input: String::new(),
            error: None,
            output: String::new(),
            scans: 0,
            had_panels: false,
            queue: VecDeque::new(),
            remedies: Vec::new(),
            scan_failed: false,
        }
    }

    /// Whether the panel takes requests: they run now, or in turn.
    fn accepts(&self) -> bool {
        self.view.as_ref().is_some_and(|v| v.status.writable) && self.confirmation.is_none()
    }

    fn fail(&mut self, error: impl Into<String>) {
        self.error = Some(error.into());
        self.remedies.clear();
        self.scan_failed = false;
        // What was waiting was asked for expecting this to work.
        self.queue.clear();
    }

    fn dismiss(&mut self) {
        self.error = None;
        self.remedies.clear();
        self.scan_failed = false;
    }

    /// Whether Git can start an operation now: the status shown is current.
    fn writable(&self) -> bool {
        self.view.as_ref().is_some_and(|v| v.status.writable)
            && self.view_epoch == self.epoch
            && self.pending.is_none()
            && !self.refresh
            && self.confirmation.is_none()
    }

    fn invalidate(&mut self) {
        self.epoch += 1;
        self.refresh = true;
    }

    fn progress(&self) -> Option<&'static str> {
        let Some(pending) = self.pending.as_ref() else {
            return (!self.queue.is_empty()).then_some("Updating repository…");
        };
        match pending {
            Pending::Saving { .. } => Some("Saving documents…"),
            Pending::Review { .. } => Some("Reading changes…"),
            Pending::Running { operation, .. } => Some(match operation {
                Operation::Fetch => "Fetching…",
                Operation::Pull | Operation::PullRebase | Operation::PullMerge => "Pulling…",
                Operation::Push => "Pushing…",
                Operation::Commit { .. } => "Committing…",
                Operation::Stage { .. } => "Staging…",
                Operation::Unstage { .. } => "Unstaging…",
                _ => "Updating repository…",
            }),
        }
    }
}

struct SavedSource {
    id: DocumentId,
    revision: u64,
    path: PathBuf,
}

enum Pending {
    Saving {
        operation: Operation,
        sources: Vec<SavedSource>,
        /// Staging a change from a diff: checked again before it's applied.
        guard: Option<diff::StageGuard>,
    },
    Review {
        task: ManagedTask<Result<crate::Diff, String>>,
    },
    Running {
        operation: Operation,
        task: ManagedTask<Result<String, String>>,
        cancel: Arc<AtomicBool>,
    },
}

fn git_job(
    root: PathBuf,
    operation: Operation,
    cancel: &Cancellation,
    requested: &AtomicBool,
) -> Result<Output, String> {
    let mut job = Job::start(root, operation);
    loop {
        if cancel.is_cancelled() || requested.load(Ordering::Acquire) {
            job.cancel();
        }
        if let Some(result) = job.try_result() {
            return result.map_err(|e| e.to_string());
        }
        // Continue draining after cancellation. Dropping Job alone can detach it.
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The file a document shows, from its viewers' `ViewerFile`.
fn source_path(world: &mut World, id: DocumentId) -> Option<PathBuf> {
    world
        .query::<(&DocumentReference, &ViewerFile)>()
        .iter(world)
        .find(|(reference, _)| reference.document() == id)
        .map(|(_, file)| file.0.clone())
}

/// Open documents of files under `root`, and their files.
fn source_infos(world: &mut World, root: &Path) -> Vec<(DocumentInfo, PathBuf)> {
    let files: Vec<(DocumentId, PathBuf)> = world
        .query::<(&DocumentReference, &ViewerFile)>()
        .iter(world)
        .filter(|(_, file)| file.0.starts_with(root))
        .map(|(reference, file)| (reference.document(), file.0.clone()))
        .collect();
    let mut seen = std::collections::HashSet::new();
    files
        .into_iter()
        .filter(|(id, _)| seen.insert(*id))
        .filter_map(|(id, path)| {
            world
                .workbench()
                .inspect_document(id)
                .ok()
                .map(|info| (info, path))
        })
        .collect()
}

fn changes_files(operation: &Operation) -> bool {
    matches!(
        operation,
        Operation::SwitchBranch { .. }
            | Operation::Pull
            | Operation::PullRebase
            | Operation::PullMerge
            | Operation::Resolve { .. }
            | Operation::Continue
            | Operation::Abort
            | Operation::Discard { .. }
            | Operation::DiscardRange { .. }
            | Operation::DiscardHunk { .. }
    )
}

fn begin(world: &mut World, session: &mut GitSession, operation: Operation) -> Result<(), String> {
    if world
        .get_resource::<ManagedWork>()
        .is_none_or(ManagedWork::is_closing)
    {
        return Err("The Git workspace is closing.".into());
    }
    if !session.view.as_ref().is_some_and(|v| v.status.writable) {
        return Err("Open the repository root before changing Git state.".into());
    }
    if !session.writable() {
        return Err("Git is busy: try again in a moment.".into());
    }
    let root = PathBuf::from(&session.view.as_ref().unwrap().status.root);
    let infos = source_infos(world, &root);
    let mut sources = Vec::new();
    for (info, path) in infos {
        let relative = path.strip_prefix(&root).unwrap().to_string_lossy();
        let save = match &operation {
            Operation::Stage { paths } => {
                paths.is_empty() || paths.iter().any(|p| p == relative.as_ref())
            }
            Operation::StageRange { path, .. } | Operation::StageHunk { path, .. } => {
                path == relative.as_ref()
            }
            // A new branch keeps the working files: nothing to save for it.
            Operation::SwitchBranch { .. }
            | Operation::Pull
            | Operation::PullRebase
            | Operation::PullMerge
            | Operation::Continue => true,
            _ => false,
        };
        let replaces = match &operation {
            Operation::Discard { path, .. }
            | Operation::DiscardRange { path, .. }
            | Operation::DiscardHunk { path, .. }
            | Operation::Resolve { path, .. } => path == relative.as_ref(),
            Operation::Abort => true,
            _ => false,
        };
        if (save || replaces) && (info.has_drafts || info.conflict) {
            return Err(format!(
                "Finish composition or resolve the source conflict in {} before changing Git state.",
                path.display()
            ));
        }
        // A file another checkout removed is harmless while the editor holds
        // nothing unsaved for it: Git may bring it back, and the reload clears
        // `missing`. Unsaved edits to it can't be saved, so they still block.
        if (save || replaces) && info.missing && info.dirty {
            return Err(format!(
                "{} no longer exists on disk and has unsaved editor changes. Save it elsewhere or discard them before changing Git state.",
                path.display()
            ));
        }
        if replaces && (info.dirty || info.saving) {
            return Err(format!(
                "Save or discard editor changes in {} before replacing its working file.",
                path.display()
            ));
        }
        if save && !info.missing {
            sources.push(SavedSource {
                id: info.id,
                revision: info.revision,
                path,
            });
        }
    }
    // Validate the whole set before admitting any saves. Failed preparation never
    // starts Git; a save that did finish remains an ordinary acknowledged save.
    for source in &sources {
        let info = world
            .workbench()
            .inspect_document(source.id)
            .map_err(|e| e.to_string())?;
        if info.dirty && !info.saving {
            world
                .workbench_mut()
                .save_document(source.id)
                .map_err(|e| e.to_string())?;
        }
    }
    session.dismiss();
    session.pending = Some(Pending::Saving {
        operation,
        sources,
        guard: None,
    });
    session.invalidate();
    Ok(())
}

/// Run a request now, or after what's running and the refresh that follows
/// it: a click while Git is busy waits its turn rather than failing.
fn submit(world: &mut World, session: &mut GitSession, request: Request) -> Result<(), String> {
    if session.writable() && session.queue.is_empty() {
        return start(world, session, request);
    }
    if !session.view.as_ref().is_some_and(|v| v.status.writable) {
        return Err("Open the repository root before changing Git state.".into());
    }
    let running = match &session.pending {
        Some(Pending::Saving { operation, .. } | Pending::Running { operation, .. }) => {
            Some(operation)
        }
        _ => None,
    };
    // The same thing asked twice (a double click) is asked once.
    if session.queue.back() == Some(&request)
        || matches!(&request, Request::Operation(operation) if running == Some(operation))
    {
        return Ok(());
    }
    // Files staged (or unstaged) one after another go to Git together.
    match (session.queue.back_mut(), &request) {
        (
            Some(Request::Operation(Operation::Stage { paths: queued })),
            Request::Operation(Operation::Stage { paths }),
        )
        | (
            Some(Request::Operation(Operation::Unstage { paths: queued })),
            Request::Operation(Operation::Unstage { paths }),
        ) if !queued.is_empty() && !paths.is_empty() => {
            for path in paths {
                if !queued.contains(path) {
                    queued.push(path.clone());
                }
            }
        }
        _ => session.queue.push_back(request),
    }
    Ok(())
}

fn start(world: &mut World, session: &mut GitSession, request: Request) -> Result<(), String> {
    match request {
        Request::Operation(operation) => begin(world, session, operation),
        Request::Discard(path) => request_discard(world, session, path),
    }
}

/// The entries a commit takes, to tell whether they're still the ones shown.
fn staged(status: &crate::Status) -> Vec<(String, char)> {
    status
        .entries
        .iter()
        .filter(|entry| staging(entry) != Staging::Unstaged)
        .map(|entry| (entry.path.clone(), entry.index_status))
        .collect()
}

/// Next steps for a failed operation, from what Git said.
fn remedies(operation: &Operation, error: &str) -> Vec<(&'static str, Operation)> {
    let has = |text: &str| error.contains(text);
    let pull = matches!(
        operation,
        Operation::Pull | Operation::PullRebase | Operation::PullMerge
    );
    if pull && (has("divergent branches") || has("Not possible to fast-forward")) {
        return vec![
            ("Pull with rebase", Operation::PullRebase),
            ("Pull with merge", Operation::PullMerge),
        ];
    }
    if *operation == Operation::Push
        && (has("[rejected]") || has("fetch first") || has("non-fast-forward"))
    {
        return vec![("Pull", Operation::Pull)];
    }
    // What can simply be asked again: nothing that names a diff it was read from.
    let repeatable = matches!(
        operation,
        Operation::Stage { .. }
            | Operation::Unstage { .. }
            | Operation::Commit { .. }
            | Operation::CreateBranch { .. }
            | Operation::SwitchBranch { .. }
            | Operation::Fetch
            | Operation::Pull
            | Operation::PullRebase
            | Operation::PullMerge
            | Operation::Push
    );
    if repeatable
        && (has(".lock': File exists")
            || has("Could not resolve host")
            || has("Another Git operation is still running"))
    {
        return vec![("Retry", operation.clone())];
    }
    Vec::new()
}

fn request_discard(
    world: &mut World,
    session: &mut GitSession,
    path: String,
) -> Result<(), String> {
    if !session.writable() {
        return Err("Wait for Git to refresh before discarding.".into());
    }
    let root = session.root.clone();
    let task = world.resource_mut::<ManagedWork>().spawn_task(
        None,
        "Review Git discard",
        move |cancel| match git_job(
            root,
            Operation::Diff {
                path,
                side: DiffSide::Unstaged,
            },
            &cancel,
            &AtomicBool::new(false),
        )? {
            Output::Diff(diff) => Ok(diff),
            _ => Err("Unexpected Git diff response".into()),
        },
    )?;
    session.pending = Some(Pending::Review { task });
    Ok(())
}

fn cancel(session: &mut GitSession) {
    session.queue.clear();
    match &session.pending {
        Some(Pending::Running { cancel, .. }) => {
            cancel.store(true, Ordering::Release);
        }
        Some(Pending::Review { task }) => {
            task.cancel();
            session.pending = None;
            session.invalidate();
        }
        Some(Pending::Saving { .. }) => {
            session.pending = None;
            session.invalidate();
        }
        None => {}
    }
}

fn poll_pending(world: &mut World, session: &mut GitSession) -> Result<(), String> {
    let Some(pending) = session.pending.take() else {
        return Ok(());
    };
    match pending {
        Pending::Review { task } => match task.try_recv() {
            Ok(Ok(diff)) => {
                session.confirmation = Some(Operation::Discard {
                    path: diff.path,
                    snapshot: diff.snapshot,
                });
            }
            Ok(Err(error)) => return Err(error),
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                session.pending = Some(Pending::Review { task })
            }
            Err(_) => return Err("Git review was cancelled.".into()),
        },
        Pending::Saving {
            operation,
            sources,
            guard,
        } => {
            let mut saving = false;
            for source in &sources {
                let info = world
                    .workbench()
                    .inspect_document(source.id)
                    .map_err(|e| e.to_string())?;
                if info.revision != source.revision
                    || source_path(world, source.id).as_ref() != Some(&source.path)
                {
                    return Err(
                        "A document changed while preparing Git. Review the changes and retry."
                            .into(),
                    );
                }
                saving |= info.saving;
                if !info.saving && (info.dirty || info.conflict || info.missing || info.has_drafts)
                {
                    return Err(format!(
                        "Git stopped because {} wasn't saved: {}",
                        source.path.display(),
                        info.error.unwrap_or_else(|| "it changed".into())
                    ));
                }
            }
            if saving || session.scan.is_some() {
                session.pending = Some(Pending::Saving {
                    operation,
                    sources,
                    guard,
                });
                return Ok(());
            }
            // Check destructive gates again after queued reads/saves. New edits
            // may have happened since the user confirmed an operation.
            let root = PathBuf::from(&session.view.as_ref().unwrap().status.root);
            if changes_files(&operation) {
                for (info, path) in source_infos(world, &root) {
                    let affected = match &operation {
                        Operation::Discard { path: p, .. }
                        | Operation::DiscardRange { path: p, .. }
                        | Operation::DiscardHunk { path: p, .. }
                        | Operation::Resolve { path: p, .. } => path == root.join(p),
                        _ => true,
                    };
                    // A clean `missing` document has nothing to lose (see `begin`).
                    if affected && (info.dirty || info.saving || info.has_drafts || info.conflict) {
                        return Err(format!(
                            "{} changed while preparing Git. Save or resolve it and retry.",
                            path.display()
                        ));
                    }
                }
            }
            // A commit takes what's staged: check that it's still what was
            // shown. Other changes in the folder (a build, a formatter, an
            // agent at work) are no reason to stop.
            let expected_commit = matches!(operation, Operation::Commit { .. })
                .then(|| staged(&session.view.as_ref().unwrap().status));
            let requested = Arc::new(AtomicBool::new(false));
            let stop = requested.clone();
            let mut op = operation.clone();
            let task = world.resource_mut::<ManagedWork>().spawn_task(None, "Git operation", move |cancel| {
                    if let Some(expected) = expected_commit {
                        let Output::Status(status) = git_job(root.clone(), Operation::Status, &cancel, &stop)? else { unreachable!() };
                        if staged(&status) != expected { return Err("The staged changes are no longer the ones shown. Review them and commit again.".into()); }
                    }
                    if let Some(guard) = guard
                        && let Operation::StageRange { path, snapshot, .. } = &mut op
                    {
                        let Output::Diff(fresh) = git_job(
                            root.clone(),
                            Operation::Diff {
                                path: path.clone(),
                                side: DiffSide::Unstaged,
                            },
                            &cancel,
                            &stop,
                        )?
                        else {
                            return Err("Git answered with something else".into());
                        };
                        guard.validate(&fresh)?;
                        *snapshot = fresh.snapshot;
                    }
                    match git_job(root, op, &cancel, &stop)? { Output::Done { output } => Ok(output), _ => Err("Unexpected Git operation response".into()) }
            })?;
            session.pending = Some(Pending::Running {
                operation,
                task,
                cancel: requested,
            });
        }
        Pending::Running {
            operation,
            task,
            cancel,
        } => match task.try_recv() {
            Ok(result) => {
                match result {
                    Ok(output) => {
                        if let Operation::Commit { message, .. } = &operation
                            && session.draft == *message
                        {
                            session.draft.clear();
                        }
                        if let Operation::CreateBranch { name } = &operation
                            && session.branch_input.trim() == name
                        {
                            session.branch_input.clear();
                        }
                        session.output = output;
                    }
                    Err(error) => {
                        let remedies = remedies(&operation, &error);
                        session.fail(error);
                        session.remedies = remedies;
                    }
                }
                session.invalidate();
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                session.pending = Some(Pending::Running {
                    operation,
                    task,
                    cancel,
                })
            }
            Err(_) => {
                session.invalidate();
                return Err("Git worker stopped; refresh to check the operation's outcome.".into());
            }
        },
    }
    Ok(())
}

fn reproject(world: &mut World, session: &mut GitSession) -> Result<(), String> {
    if let Some((_, task)) = session.projection.take() {
        task.cancel();
    }
    let Some(view) = &session.view else {
        return Ok(());
    };
    if view.options == session.options && view.collapsed == session.collapsed {
        return Ok(());
    }
    let status = view.status.clone();
    let options = session.options;
    let collapsed = session.collapsed.clone();
    let task = world.resource_mut::<ManagedWork>().spawn_task(
        None,
        "Prepare Git presentation",
        move |_| RepositoryView::prepare(status, options, collapsed),
    )?;
    session.projection = Some((session.epoch, task));
    Ok(())
}

fn update(world: &mut World) {
    let has_panels = world
        .query_filtered::<Entity, Or<(With<GitPanel>, With<diff::DiffPanel>)>>()
        .iter(world)
        .next()
        .is_some();
    if has_panels
        && !world.contains_resource::<GitSession>()
        && let Some(root) = world.get_resource::<Root>().map(|r| r.0.clone())
    {
        // A restored panel brings back its draft and view options.
        let restored = world
            .query::<&GitPanel>()
            .iter(world)
            .next()
            .map(|panel| (panel.draft.clone(), panel.options));
        let mut session = GitSession::new(root);
        if let Some((draft, options)) = restored {
            session.draft = draft;
            session.options = options;
        }
        world.insert_resource(session);
    }
    if let Some(mut session) = world.remove_resource::<GitSession>() {
        let closing = world.resource::<ManagedWork>().is_closing();
        let visible = world
            .query_filtered::<&PanelPresentation, Or<(With<GitPanel>, With<diff::DiffPanel>)>>()
            .iter(world)
            .any(|p| p.visible);
        if has_panels && !session.had_panels {
            session.invalidate();
        }
        session.had_panels = has_panels;
        if let Some((epoch, task)) = session.projection.take() {
            match task.try_recv() {
                Ok(view)
                    if epoch == session.epoch
                        && session
                            .view
                            .as_ref()
                            .is_some_and(|current| Arc::ptr_eq(&current.status, &view.status))
                        && view.options == session.options
                        && view.collapsed == session.collapsed =>
                {
                    session.view = Some(Arc::new(view));
                }
                Ok(_) => {}
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    session.projection = Some((epoch, task))
                }
                Err(_) => {
                    if !closing {
                        session.fail("Git presentation worker stopped");
                    }
                }
            }
        }
        if let Some((epoch, scan)) = session.scan.take() {
            match scan.try_recv() {
                Ok(result) => {
                    if epoch == session.epoch {
                        match result {
                            Ok(view) => {
                                if session.scan_failed {
                                    session.dismiss();
                                }
                                session.view = Some(Arc::new(view));
                                session.view_epoch = epoch;
                                if let Err(error) = reproject(world, &mut session) {
                                    session.fail(error);
                                }
                            }
                            Err(error) => {
                                session.fail(error);
                                session.scan_failed = true;
                                session.epoch += 1;
                            }
                        }
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => session.scan = Some((epoch, scan)),
                Err(_) => {
                    if !closing {
                        session.fail("Git status worker stopped");
                        if epoch == session.epoch {
                            session.epoch += 1;
                        }
                    }
                }
            }
        }
        if !closing {
            if let Err(error) = poll_pending(world, &mut session) {
                session.fail(error);
                session.invalidate();
            }
            // What was asked for meanwhile, once the status is current again.
            if session.writable()
                && let Some(request) = session.queue.pop_front()
                && let Err(error) = start(world, &mut session, request)
            {
                session.fail(error);
            }
            let interval = Duration::from_secs(if visible { 2 } else { 10 });
            if session.pending.is_none()
                && session.scan.is_none()
                && (session.refresh || (has_panels && session.last_scan.elapsed() >= interval))
            {
                let root = session.root.clone();
                let options = session.options;
                let collapsed = session.collapsed.clone();
                let task = world.resource_mut::<ManagedWork>().spawn_task(
                    None,
                    "Git status",
                    move |cancel| match git_job(
                        root,
                        Operation::Status,
                        &cancel,
                        &AtomicBool::new(false),
                    )? {
                        Output::Status(status) => Ok(RepositoryView::prepare(
                            Arc::new(status),
                            options,
                            collapsed,
                        )),
                        _ => Err("Unexpected Git status response".into()),
                    },
                );
                match task {
                    Ok(task) => {
                        session.scan = Some((session.epoch, task));
                        session.scans += 1;
                        session.refresh = false;
                        session.last_scan = Instant::now();
                    }
                    Err(error) => {
                        session.fail(error);
                        session.refresh = false;
                        session.last_scan = Instant::now();
                    }
                }
            }
        }
        if !closing {
            diff::update(world, &mut session);
        }
        // Panels keep the draft and view options, to come back with the layout.
        let panels: Vec<Entity> = world
            .query_filtered::<Entity, With<GitPanel>>()
            .iter(world)
            .collect();
        for panel in panels {
            let mut state = world.get_mut::<GitPanel>(panel).unwrap();
            if state.draft != session.draft {
                state.draft.clone_from(&session.draft);
            }
            if state.options != session.options {
                state.options = session.options;
            }
        }
        world.insert_resource(session);
    }
}
