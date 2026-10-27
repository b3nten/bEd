//! Local Git workflow. The workspace owns repository state and admitted work;
//! panels only observe it. All Git/file I/O stays in managed workers.
pub(crate) mod diff;
mod panel;
mod presentation;
use crate::persistence::ReflectProjectPersist;
use presentation::{ChangeSet, DirectoryKey, Group, RepositoryView, Row};
pub(crate) use presentation::{GitPanelOptions, GroupBy, ViewMode};
use std::collections::BTreeSet;
#[cfg(test)]
mod tests;

use bedtime_git::{CommitMode, DiffSide, Job, Operation, Output, StatusEntry};
use bedtime_workbench::*;
use bevy::prelude::*;
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

pub use panel::GitPanel;

#[derive(Resource)]
struct RestoredDraft(String);
#[derive(Resource)]
struct RestoredOptions(GitPanelOptions);

pub(crate) fn panel_options(world: &World) -> GitPanelOptions {
    world
        .get_resource::<GitSession>()
        .map(|s| s.options)
        .or_else(|| world.get_resource::<RestoredOptions>().map(|o| o.0))
        .unwrap_or_default()
}

pub(crate) fn restore_options(world: &mut World, options: GitPanelOptions) {
    if let Some(mut session) = world.get_resource_mut::<GitSession>() {
        if session.options != options {
            session.options = options;
            session.invalidate();
        }
    } else {
        world.insert_resource(RestoredOptions(options));
    }
}

pub(crate) fn commit_draft(world: &World) -> String {
    world
        .get_resource::<GitSession>()
        .map(|s| s.draft.clone())
        .or_else(|| world.get_resource::<RestoredDraft>().map(|d| d.0.clone()))
        .unwrap_or_default()
}

pub(crate) fn restore_draft(world: &mut World, draft: String) {
    if let Some(mut session) = world.get_resource_mut::<GitSession>() {
        if session.draft.is_empty() {
            session.draft = draft;
        }
    } else {
        world.insert_resource(RestoredDraft(draft));
    }
}

pub(crate) fn has_pending_operation(world: &World) -> bool {
    world
        .get_resource::<GitSession>()
        .is_some_and(|s| s.pending.is_some())
}

pub struct GitPlugin;
impl Plugin for GitPlugin {
    fn build(&self, app: &mut App) {
        register_menu(app);
        app.init_resource::<crate::file_editor::FileWrites>()
            .add_systems(
                Update,
                update
                    .after(crate::files::FilesSystems::Update)
                    .before(crate::close::CloseSystems::Update),
            );
        #[cfg(feature = "automation")]
        {
            use bedtime_automation::AutomationAppExt;
            app.register_automation_command("app.inspect_git", inspect);
        }
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
struct GitSession {
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
    comparisons: diff::Comparisons,
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
            comparisons: Default::default(),
        }
    }

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
        match self.pending.as_ref()? {
            Pending::Saving { .. } => Some("Saving documents…"),
            Pending::Review { .. } => Some("Reading changes…"),
            Pending::Running { operation, .. } => Some(match operation {
                Operation::Fetch => "Fetching…",
                Operation::Pull => "Pulling…",
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
        guard: Option<diff::StageGuard>,
    },
    Review {
        task: ManagedTask<Result<bedtime_git::Diff, String>>,
    },
    Running {
        operation: Operation,
        task: bedtime_filesystem::workbench::SourceChangeTask<Result<String, String>>,
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

fn source_infos(world: &mut World, root: &Path) -> Vec<(DocumentInfo, PathBuf)> {
    let ids: Vec<_> = world
        .query::<(Entity, &Document)>()
        .iter(world)
        .filter_map(|(entity, _)| {
            let id = DocumentId(entity);
            crate::file_editor::source_path(world, id)
                .filter(|p| p.starts_with(root))
                .map(|p| (id, p.to_owned()))
        })
        .collect();
    ids.into_iter()
        .filter_map(|(id, path)| inspect_document(world, id).ok().map(|info| (info, path)))
        .collect()
}

fn changes_files(operation: &Operation) -> bool {
    matches!(
        operation,
        Operation::CreateBranch { .. }
            | Operation::SwitchBranch { .. }
            | Operation::Pull
            | Operation::Resolve { .. }
            | Operation::Continue
            | Operation::Abort
            | Operation::Discard { .. }
            | Operation::DiscardRange { .. }
            | Operation::DiscardHunk { .. }
    )
}

fn needs_write_lease(operation: &Operation) -> bool {
    changes_files(operation)
        || matches!(
            operation,
            Operation::Stage { .. } | Operation::StageRange { .. } | Operation::StageHunk { .. }
        )
}

fn begin(world: &mut World, session: &mut GitSession, operation: Operation) -> Result<(), String> {
    if !crate::close::accepts_user_actions(world) {
        return Err("Cancel quit before changing Git state.".into());
    }
    if world
        .get_resource::<ManagedWork>()
        .is_none_or(ManagedWork::is_closing)
    {
        return Err("The Git workspace is closing.".into());
    }
    if !session.writable() {
        return Err(
            "Wait for Git to refresh, or open the repository root before changing Git state."
                .into(),
        );
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
            Operation::CreateBranch { .. }
            | Operation::SwitchBranch { .. }
            | Operation::Pull
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
        if (save || replaces) && (info.has_drafts || info.conflict || info.requires_source_refresh)
        {
            return Err(format!(
                "Finish composition or resolve the source conflict in {} before changing Git state.",
                path.display()
            ));
        }
        if replaces && (info.dirty || info.saving) {
            return Err(format!(
                "Save or discard editor changes in {} before replacing its working file.",
                path.display()
            ));
        }
        if save {
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
        let info = inspect_document(world, source.id).map_err(|e| e.to_string())?;
        if info.dirty && !info.saving {
            save_document(world, source.id).map_err(|e| e.to_string())?;
        }
    }
    session.error = None;
    session.pending = Some(Pending::Saving {
        operation,
        sources,
        guard: None,
    });
    session.invalidate();
    Ok(())
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
    let task =
        Workbench::new(world)
            .work()
            .spawn_task(None, "Review Git discard", move |cancel| {
                match git_job(
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
                }
            })?;
    session.pending = Some(Pending::Review { task });
    Ok(())
}

fn cancel(session: &mut GitSession) {
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
                let info = inspect_document(world, source.id).map_err(|e| e.to_string())?;
                if info.revision != source.revision
                    || crate::file_editor::source_path(world, source.id).as_ref()
                        != Some(&source.path)
                {
                    return Err(
                        "A document changed while preparing Git. Review the changes and retry."
                            .into(),
                    );
                }
                saving |= info.saving;
                if !info.saving
                    && (info.dirty
                        || info.conflict
                        || info.requires_source_refresh
                        || info.has_drafts)
                {
                    return Err(format!(
                        "Git stopped because {} was not saved safely. {:?}",
                        source.path.display(),
                        info.last_save_error
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
                    if affected
                        && (info.dirty
                            || info.saving
                            || info.has_drafts
                            || info.conflict
                            || info.requires_source_refresh)
                    {
                        return Err(format!(
                            "{} changed while preparing Git. Save or resolve it and retry.",
                            path.display()
                        ));
                    }
                }
            }
            let affected = match &operation {
                Operation::Discard { path, .. }
                | Operation::DiscardRange { path, .. }
                | Operation::DiscardHunk { path, .. }
                | Operation::Resolve { path, .. } => root.join(path),
                _ => root.clone(),
            };
            let change = bedtime_filesystem::workbench::prepare_source_change(
                world,
                needs_write_lease(&operation).then_some(root.as_path()),
                changes_files(&operation).then_some(affected.as_path()),
            )?;
            let expected_commit = matches!(operation, Operation::Commit { .. })
                .then(|| session.view.as_ref().unwrap().status.snapshot.clone());
            let requested = Arc::new(AtomicBool::new(false));
            let stop = requested.clone();
            let mut op = operation.clone();
            let task = change.spawn(world, "Git operation", move |cancel| {
                    if let Some(expected) = expected_commit {
                        let Output::Status(status) = git_job(root.clone(), Operation::Status, &cancel, &stop)? else { unreachable!() };
                        if status.snapshot != expected { return Err("Repository state changed before commit. Review the refreshed status and retry.".into()); }
                    }
                    if let Some(guard)=guard {
                        if let Operation::StageRange { path, snapshot, .. } = &mut op {
                            let Output::Diff(fresh)=git_job(root.clone(),Operation::Diff {path:path.clone(),side:DiffSide::Unstaged},&cancel,&stop)? else {unreachable!()};
                            guard.validate(&fresh)?;
                            *snapshot=fresh.snapshot;
                        }
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
                let (result, reload_errors) = result?;
                match result {
                    Ok(output) => {
                        if let Operation::Commit { message, .. } = &operation {
                            if session.draft == *message {
                                session.draft.clear();
                            }
                        }
                        if let Operation::CreateBranch { name } = &operation {
                            if session.branch_input.trim() == name {
                                session.branch_input.clear();
                            }
                        }
                        session.output = output;
                    }
                    Err(error) => session.error = Some(error),
                }
                if !reload_errors.is_empty() {
                    session.error = Some(format!(
                        "Git finished; some editor sources need Reload to compare: {}",
                        reload_errors.join("; ")
                    ));
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
    let task =
        Workbench::new(world)
            .work()
            .spawn_task(None, "Prepare Git presentation", move |_| {
                RepositoryView::prepare(status, options, collapsed)
            })?;
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
        && let Some(root) = world
            .get_resource::<crate::files::ProjectRoot>()
            .map(|r| r.0.clone())
    {
        let draft = world
            .remove_resource::<RestoredDraft>()
            .map(|d| d.0)
            .unwrap_or_default();
        let options = world
            .remove_resource::<RestoredOptions>()
            .map(|o| o.0)
            .unwrap_or_default();
        let mut session = GitSession::new(root);
        session.options = options;
        session.draft = draft;
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
                        session.error = Some("Git presentation worker stopped".into());
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
                                session.view = Some(Arc::new(view));
                                session.view_epoch = epoch;
                                if let Err(error) = reproject(world, &mut session) {
                                    session.error = Some(error);
                                }
                            }
                            Err(error) => {
                                session.error = Some(error);
                                session.epoch += 1;
                            }
                        }
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => session.scan = Some((epoch, scan)),
                Err(_) => {
                    if !closing {
                        session.error = Some("Git status worker stopped".into());
                        if epoch == session.epoch {
                            session.epoch += 1;
                        }
                    }
                }
            }
        }
        if !closing {
            if let Err(error) = poll_pending(world, &mut session) {
                session.error = Some(error);
                session.invalidate();
            }
            let interval = Duration::from_secs(if visible { 2 } else { 10 });
            if session.pending.is_none()
                && session.scan.is_none()
                && (session.refresh || (has_panels && session.last_scan.elapsed() >= interval))
                && !crate::close::shutting_down(world)
            {
                let root = session.root.clone();
                let options = session.options;
                let collapsed = session.collapsed.clone();
                let task =
                    Workbench::new(world)
                        .work()
                        .spawn_task(None, "Git status", move |cancel| {
                            match git_job(
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
                            }
                        });
                match task {
                    Ok(task) => {
                        session.scan = Some((session.epoch, task));
                        session.scans += 1;
                        session.refresh = false;
                        session.last_scan = Instant::now();
                    }
                    Err(error) => {
                        session.error = Some(error);
                        session.refresh = false;
                        session.last_scan = Instant::now();
                    }
                }
            }
        }
        if !closing {
            diff::update(world, &mut session);
        }
        world.insert_resource(session);
    }
}

#[cfg(feature = "automation")]
fn inspect(world: &mut World, _: serde_json::Value) -> bedtime_automation::AutomationResult {
    let sessions: Vec<_> = world.query::<(Entity, &GitSession)>().iter(world).map(|(workspace, session)| {
        serde_json::json!({"workspace":workspace.to_bits().to_string(),"root":session.root,"status":session.view.as_ref().map(|v| v.status.as_ref()),"busy":session.pending.is_some(),"refreshing":session.scan.is_some(),"scans":session.scans,"options":session.options,"projecting":session.projection.is_some(),"presented_options":session.view.as_ref().map(|v| v.options),"row_count":session.view.as_ref().map(|v|v.rows.len()),"draft":session.draft,"commit_mode":session.commit_mode,"error":session.error,"output":session.output,"confirmation":session.confirmation,"progress":session.progress()})
    }).collect();
    let panels: Vec<_> = world.query::<(Entity, &GitPanel)>().iter(world).map(|(entity, panel)| serde_json::json!({"entity":entity.to_bits().to_string(),"rendered_rows":panel.rendered_rows,"draw_micros":panel.draw_micros,"tree":panel.tree_diagnostics(),"controls":panel.controls.iter().map(|(key, rect)| (key.clone(),serde_json::json!({"min":[rect.min.x,rect.min.y],"max":[rect.max.x,rect.max.y]}))).collect::<serde_json::Map<String,serde_json::Value>>()})).collect();
    Ok(serde_json::json!({"sessions":sessions,"panels":panels,"diffs":diff::inspect(world)}))
}

use bedtime_workbench::persistence::{CapturedWorld, ReflectWorkbenchPersist};

#[derive(Component, Reflect, Default)]
#[reflect(Component, WorkbenchPersist)]
#[require(GitPanel)]
pub(crate) struct PersistedGitPanel;

#[derive(Resource, Reflect, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[reflect(opaque)]
#[reflect(Resource, Serialize, Deserialize, ProjectPersist)]
#[serde(deny_unknown_fields)]
pub(crate) struct PersistedGit {
    pub draft: String,
    pub options: GitPanelOptions,
}

#[derive(Component, Reflect, Clone, serde::Serialize, serde::Deserialize)]
#[reflect(opaque)]
#[reflect(Component, Serialize, Deserialize, WorkbenchPersist)]
#[require(PanelInstance=PanelInstance::of::<diff::DiffPanel>())]
pub(crate) struct PersistedDiff(diff::Descriptor);

pub(crate) fn register_persistence(app: &mut App) {
    app.register_type::<PersistedGitPanel>()
        .register_type::<PersistedGit>()
        .register_type::<PersistedDiff>();
}

pub(crate) fn project_persistence(
    world: &mut World,
    captured: &mut bedtime_workbench::persistence::CapturedWorld,
) {
    let panels: Vec<_> = world
        .query_filtered::<Entity, With<GitPanel>>()
        .iter(world)
        .collect();
    for e in panels {
        captured.insert_component(e, PersistedGitPanel);
    }
    let panels: Vec<_> = world
        .query::<(Entity, &diff::DiffPanel)>()
        .iter(world)
        .map(|(e, p)| (e, p.descriptor.clone()))
        .collect();
    for (e, p) in panels {
        captured.insert_component(e, PersistedDiff(p));
    }
}

pub(crate) fn clear_persistence(world: &mut World) {
    let panels: Vec<_> = world
        .query_filtered::<Entity, Or<(With<PersistedGitPanel>, With<PersistedDiff>)>>()
        .iter(world)
        .collect();
    for e in panels {
        world
            .entity_mut(e)
            .remove::<(PersistedGitPanel, PersistedDiff)>();
    }
}

pub(crate) fn validate_persistence(prepared: &CapturedWorld) -> Result<(), String> {
    for (_, p) in prepared.components::<PersistedDiff>() {
        p.0.validate()?;
    }
    Ok(())
}

pub(crate) fn activate_persistence(world: &mut World) {
    let panels: Vec<_> = world
        .query_filtered::<Entity, With<PersistedDiff>>()
        .iter(world)
        .collect();
    for e in panels {
        let p = world.entity_mut(e).take::<PersistedDiff>().unwrap();
        world.entity_mut(e).insert(diff::DiffPanel::new(p.0));
    }
    clear_persistence(world);
}

pub(crate) fn register_menu(app: &mut App) {
    crate::menus::register_panel::<GitPanel>(
        app,
        Some(10),
        bedtime_workbench::PanelFactory::of::<GitPanel>(),
        crate::storage::ready,
    );
}

pub(crate) fn filter_layout_default(
    capture: &mut bedtime_workbench::persistence::CapturedLayout,
    registry: &AppTypeRegistry,
) -> Result<(), String> {
    let panels: Vec<_> = capture
        .components::<PersistedDiff>()
        .into_iter()
        .map(|(panel, _)| panel)
        .collect();
    for panel in panels {
        capture.remove_panel(panel, registry)?;
    }
    Ok(())
}

pub(crate) fn validate_layout_default(
    capture: &bedtime_workbench::persistence::CapturedLayout,
) -> Result<(), String> {
    if !capture.components::<PersistedDiff>().is_empty() {
        return Err("Layout defaults cannot contain project Git comparisons".into());
    }
    Ok(())
}
