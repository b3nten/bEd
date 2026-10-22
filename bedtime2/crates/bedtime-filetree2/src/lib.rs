//! Local filesystem panels with explicit application-supplied roots. Filesystem
//! I/O and projection happen in workspace-managed work; opening is a buffered
//! request for the application, never a callback from inside drawing.
mod backend;
mod errors;
#[cfg(target_os = "macos")]
mod mac_watch;
mod menu;
mod operations;
mod preferences;
mod view;

pub use backend::{FileTreeDirectoryState, FileTreeEntryKind, FileTreeRow, FileTreeSnapshot};
pub use operations::{
    FileTreeChange, FileTreeOperation, FileTreeOperationError, FileTreeOperationProgress,
    FileTreeOperationRequest, FileTreeOperationResult,
};
pub use preferences::{FileTreePreferences, FileTreeVisibility};

use backend::{Backend, ViewRequest};
use bedtime_clipboard2::Clipboard;
use bedtime_ui2::{UiContext, egui};
use bedtime_workbench2::{InWorkspace, Panel, PanelInstance, WorkStatus, WorkspaceWork};
use bevy::prelude::*;
use std::{
    collections::{BTreeSet, HashMap, VecDeque},
    ffi::OsString,
    path::{Path, PathBuf},
    sync::Arc,
};

/// The application supplies this component. Missing roots are unconfigured, not
/// an implicit request to browse the process's current directory. Relative roots
/// are resolved by the worker; filesystem identity is never inferred during draw.
#[derive(Component, Clone, Debug, PartialEq, Eq)]
pub struct FileTreeRoot(pub PathBuf);

/// Per-panel filetree policy and layout, independent of document loading policy.
/// Read-only panels may browse/open/copy and change visibility, but cannot admit
/// filesystem mutations. Configuration changes do not cancel admitted work.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct FileTreeSettings {
    pub read_only: bool,
    /// Logical UI points per tree level. Drawing clamps to 0..=64 and uses 16
    /// for non-finite values, so direct ECS configuration cannot poison layout.
    pub indent_size: f32,
}
impl Default for FileTreeSettings {
    fn default() -> Self {
        Self {
            read_only: false,
            indent_size: 16.0,
        }
    }
}
impl FileTreeSettings {
    fn effective_indent(self) -> f32 {
        if self.indent_size.is_finite() {
            self.indent_size.clamp(0.0, 64.0)
        } else {
            16.0
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileTreeActivation {
    SingleClick,
    DoubleClick,
    Keyboard,
    ContextMenu,
}

/// A captured activation, not an instruction to use any particular viewer. The
/// first click of a double click is intentionally delivered without a delay.
#[derive(Message, Clone, Debug)]
pub struct FileTreeOpenRequested {
    pub panel: Entity,
    pub workspace: Entity,
    pub path: PathBuf,
    pub activation: FileTreeActivation,
}

/// Custom workspace data is not part of Workbench's layout snapshot. Hosts use
/// this notification to persist FileTreePreferences through their own storage.
#[derive(Message, Clone, Debug)]
pub struct FileTreePreferencesChanged {
    pub workspace: Entity,
}

/// Reported even after the originating panel closes or changes root. A cancelled
/// operation can have completed effects; applications must inspect the result.
#[derive(Message, Clone, Debug)]
pub struct FileTreeOperationFinished {
    pub workspace: Entity,
    pub panel: Entity,
    pub root: PathBuf,
    pub generation: u64,
    pub result: Arc<FileTreeOperationResult>,
}

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FileTreeSystems {
    Update,
}

/// Admitted mutations, including those whose originating panel has closed.
/// Hosts can defer destructive workspace/quit policy until outcomes have been
/// reconciled and their own persistence has admitted any resulting changes.
/// Calling cancellation is not evidence that a mutation had no effects.
pub fn filetree_operation_count(world: &World, workspace: Entity) -> usize {
    world
        .get_resource::<OperationArchive>()
        .and_then(|archive| archive.0.get(&workspace))
        .map_or(0, |pending| pending.origins.len())
}

pub struct FileTreePlugin;
impl Plugin for FileTreePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Clipboard>()
            .init_resource::<FileTreeClipboard>()
            .init_resource::<OperationArchive>()
            .add_message::<FileTreeOpenRequested>()
            .add_message::<FileTreePreferencesChanged>()
            .add_message::<FileTreeOperationFinished>()
            .add_systems(Update, update_filetrees.in_set(FileTreeSystems::Update));
    }
}

/// Per-panel navigation and editing. The worker supplies immutable observations;
/// selection, expansion and pending user intentions remain in the application World.
#[derive(Component)]
#[require(FileTreeSettings, PanelInstance = PanelInstance::of::<Self>())]
pub struct FileTreePanel {
    snapshot: Arc<FileTreeSnapshot>,
    retirement: Option<backend::SnapshotRetirementHandle>,
    root_input: Option<PathBuf>,
    workspace: Option<Entity>,
    generation: u64,
    revision: u64,
    sent_revision: u64,
    expanded: Arc<BTreeSet<PathBuf>>,
    root_expanded: bool,
    selection: BTreeSet<PathBuf>,
    cursor: Option<PathBuf>,
    anchor: Option<PathBuf>,
    visibility: FileTreeVisibility,
    visibility_root: Option<PathBuf>,
    preferences_revision: u64,
    show_hidden: bool,
    error: Option<String>,
    reported: errors::ReportedErrors,
    actions: VecDeque<Action>,
    menu_target: Option<MenuTarget>,
    editing: Option<InlineEdit>,
    pending: BTreeSet<u64>,
    progress: Option<FileTreeOperationProgress>,
    view: view::TreeView,
}

impl Default for FileTreePanel {
    fn default() -> Self {
        Self {
            snapshot: Arc::new(FileTreeSnapshot::default()),
            retirement: None,
            root_input: None,
            workspace: None,
            generation: 0,
            revision: 0,
            sent_revision: 0,
            expanded: Arc::default(),
            root_expanded: true,
            selection: BTreeSet::new(),
            cursor: None,
            anchor: None,
            visibility: FileTreeVisibility::default(),
            visibility_root: None,
            preferences_revision: u64::MAX,
            show_hidden: false,
            error: None,
            reported: errors::ReportedErrors::default(),
            actions: VecDeque::new(),
            menu_target: None,
            editing: None,
            pending: BTreeSet::new(),
            progress: None,
            view: view::TreeView::default(),
        }
    }
}

impl FileTreePanel {
    pub fn snapshot(&self) -> &Arc<FileTreeSnapshot> {
        &self.snapshot
    }
    pub fn selection(&self) -> &BTreeSet<PathBuf> {
        &self.selection
    }
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref().or(self.snapshot.error.as_deref())
    }
    /// Only viewport rows, in logical egui coordinates. Intended for automation
    /// and diagnostics, not as a second source of filesystem or selection state.
    pub fn row_rects(&self) -> &[(PathBuf, egui::Rect)] {
        &self.view.row_rects
    }
    /// Last drawn eligible drop destination and its viewport-clipped folder region.
    pub fn drop_target(&self) -> Option<(&Path, egui::Rect)> {
        self.view
            .drop_target
            .as_ref()
            .map(|(path, rect)| (path.as_path(), *rect))
    }
    pub fn is_busy(&self) -> bool {
        !self.pending.is_empty()
    }
    /// Whether rows are moving/fading or the latest observation has not yet been
    /// drawn. Diagnostic consumers should observe this rather than assuming a
    /// number of frames equals wall time. Unseen panels remain pending redraw.
    pub fn is_animating(&self) -> bool {
        self.view.animating || self.view.drawn_version != self.snapshot.version
    }
    pub fn refresh(&mut self) {
        self.queue(Action::Refresh);
    }
    fn changed(&mut self) {
        self.revision = self
            .revision
            .checked_add(1)
            .expect("File tree revision exhausted");
    }
    fn queue(&mut self, action: Action) {
        if self.actions.len() < 32 {
            self.actions.push_back(action);
        } else {
            self.error = Some("Too many pending file actions; wait for the current work.".into());
        }
    }
    fn is_expanded(&self, path: &Path) -> bool {
        if path == self.snapshot.root {
            self.root_expanded
        } else {
            self.expanded.contains(path)
        }
    }
    fn toggle(&mut self, path: &Path) {
        let collapsing = if path == self.snapshot.root {
            self.root_expanded = !self.root_expanded;
            !self.root_expanded
        } else {
            let expanded = Arc::make_mut(&mut self.expanded);
            if expanded.remove(path) {
                true
            } else {
                expanded.insert(path.to_owned());
                false
            }
        };
        if collapsing {
            self.view.collapsing = Some(path.to_owned());
        }
        self.changed();
    }
    fn select(&mut self, path: &Path, toggle: bool, range: bool) {
        let Some(index) = self.snapshot.row_index(path) else {
            return;
        };
        if range {
            let first = self
                .anchor
                .as_deref()
                .and_then(|anchor| self.snapshot.row_index(anchor))
                .unwrap_or(index);
            if !toggle {
                self.selection.clear();
            }
            for row in &self.snapshot.rows[first.min(index)..=first.max(index)] {
                self.selection.insert(row.path.clone());
            }
        } else if toggle {
            if !self.selection.remove(path) {
                self.selection.insert(path.to_owned());
            }
            self.anchor = Some(path.to_owned());
        } else {
            self.selection.clear();
            self.selection.insert(path.to_owned());
            self.anchor = Some(path.to_owned());
        }
        self.cursor = Some(path.to_owned());
    }
    fn replace_snapshot(&mut self, snapshot: Arc<FileTreeSnapshot>) {
        let retired = std::mem::replace(&mut self.snapshot, snapshot);
        if !retired.rows.is_empty()
            && let Some(retirement) = &self.retirement
        {
            // The worker now owns this reference, rather than racing our release
            // of the last panel Arc. Saturation/stopped teardown falls back to
            // ordinary synchronous destruction, never blocks or grows a queue.
            let _ = retirement.retire_snapshot(retired);
        }
    }
    fn reset(&mut self, workspace: Entity, root: Option<PathBuf>) {
        self.generation = self
            .generation
            .checked_add(1)
            .expect("File tree generation exhausted");
        self.workspace = Some(workspace);
        self.root_input = root;
        self.revision = 1;
        self.sent_revision = 0;
        self.replace_snapshot(Arc::new(FileTreeSnapshot::default()));
        self.expanded = Arc::default();
        self.root_expanded = true;
        self.selection.clear();
        self.cursor = None;
        self.anchor = None;
        self.menu_target = None;
        self.editing = None;
        self.show_hidden = false;
        self.visibility_root = None;
        self.preferences_revision = u64::MAX;
        self.error = None;
        self.reported = errors::ReportedErrors::default();
        self.actions.clear();
        self.view = view::TreeView::default();
        // Admitted mutations are workspace-owned and still report their outcomes.
        self.pending.clear();
        self.progress = None;
    }
}

impl Drop for FileTreePanel {
    fn drop(&mut self) {
        // Taking and reinserting the component during drawing/update moves it;
        // only actual destruction takes this path (including panel closure).
        self.replace_snapshot(Arc::new(FileTreeSnapshot::default()));
    }
}

impl Panel for FileTreePanel {
    const KEY: &'static str = "bedtime.filetree";
    const LABEL: &'static str = "Files";
    fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, panel: Entity) {
        view::draw(ui, world, context, panel);
    }
}

#[derive(Clone)]
struct MenuTarget {
    path: PathBuf,
    directory: PathBuf,
    paths: Vec<PathBuf>,
    generation: u64,
}

#[derive(Clone)]
enum Action {
    Open {
        path: PathBuf,
        activation: FileTreeActivation,
    },
    Refresh,
    NewFile {
        directory: PathBuf,
    },
    NewFolder {
        directory: PathBuf,
    },
    Rename {
        path: PathBuf,
    },
    Copy {
        paths: Vec<PathBuf>,
    },
    Cut {
        paths: Vec<PathBuf>,
    },
    Paste {
        directory: PathBuf,
    },
    Transfer {
        sources: Vec<PathBuf>,
        directory: PathBuf,
        copy: bool,
    },
    Duplicate {
        paths: Vec<PathBuf>,
    },
    Trash {
        paths: Vec<PathBuf>,
    },
    Hide {
        paths: Vec<PathBuf>,
    },
    Unhide {
        paths: Vec<PathBuf>,
    },
    ToggleHiddenFiles,
    ToggleRevealHidden,
    ClearHidden,
    SubmitEdit,
    CancelOperation,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EditKind {
    Rename,
    File,
    Directory,
}
struct InlineEdit {
    kind: EditKind,
    path: PathBuf,
    text: String,
    original: OsString,
    focus: bool,
    pending: Option<u64>,
    error: Option<String>,
}

#[derive(Resource, Default)]
struct FileTreeClipboard(Option<FileClipboardIntent>);
#[derive(Clone)]
struct FileClipboardIntent {
    is_cut: bool,
    panel: Entity,
    root: PathBuf,
    paths: Vec<PathBuf>,
    clipboard_revision: u64,
    native_revision: Option<i64>,
    clipboard_snapshot: Arc<bedtime_clipboard2::ClipboardFilesSnapshot>,
}

#[derive(Component)]
struct FilesystemRuntime {
    backend: Backend,
    panels: BTreeSet<Entity>,
    next_operation: u64,
    preferences_revision: u64,
}

/// Results outlive workspace entity destruction: the managed worker can finish
/// between Update and Workbench's Last-stage workspace retirement. Entries exist
/// only while admitted operations are outstanding, never for idle directory views.
#[derive(Resource, Default)]
struct OperationArchive(HashMap<Entity, WorkspaceOperations>);
struct WorkspaceOperations {
    results: backend::OperationResultsHandle,
    origins: HashMap<u64, OperationOrigin>,
}
struct OperationOrigin {
    panel: Entity,
    error_title: &'static str,
    root: PathBuf,
    generation: u64,
    cut_snapshot: Option<Arc<bedtime_clipboard2::ClipboardFilesSnapshot>>,
}

/// One visible execution point owns admission, observation application, and
/// reconciliation. This runs for inactive/hidden/closing workspaces as well.
fn update_filetrees(world: &mut World) {
    let panels: Vec<_> = world
        .query::<(Entity, &InWorkspace, &FileTreePanel)>()
        .iter(world)
        .map(|(panel, owner, _)| (panel, owner.0))
        .collect();
    let mut by_workspace: HashMap<Entity, BTreeSet<Entity>> = HashMap::new();
    for &(panel, workspace) in &panels {
        by_workspace.entry(workspace).or_default().insert(panel);
    }
    for &workspace in by_workspace.keys() {
        if world.get::<FileTreePreferences>(workspace).is_none() {
            world
                .entity_mut(workspace)
                .insert(FileTreePreferences::default());
        }
        if world.get::<FilesystemRuntime>(workspace).is_none() {
            let result = world
                .get_mut::<WorkspaceWork>(workspace)
                .ok_or_else(|| "Workspace has no managed work scope".to_string())
                .and_then(|mut scope| Backend::spawn(&mut scope));
            match result {
                Ok(backend) => {
                    world.entity_mut(workspace).insert(FilesystemRuntime {
                        backend,
                        panels: BTreeSet::new(),
                        next_operation: 0,
                        preferences_revision: 0,
                    });
                }
                Err(error) => {
                    for &panel in &by_workspace[&workspace] {
                        world.get_mut::<FileTreePanel>(panel).unwrap().error = Some(error.clone());
                    }
                }
            }
        }
    }
    let workspaces: Vec<_> = world
        .query_filtered::<Entity, With<FilesystemRuntime>>()
        .iter(world)
        .collect();
    for workspace in workspaces {
        let Some(mut runtime) = world.entity_mut(workspace).take::<FilesystemRuntime>() else {
            continue;
        };
        if world
            .entity(workspace)
            .get_ref::<FileTreePreferences>()
            .is_some_and(|p| p.is_changed())
        {
            runtime.preferences_revision = runtime.preferences_revision.wrapping_add(1);
        }
        let live = by_workspace.get(&workspace).cloned().unwrap_or_default();
        for &removed in runtime.panels.difference(&live) {
            runtime.backend.remove_view(removed);
        }
        runtime.panels = live;
        for &panel in &runtime.panels.clone() {
            let root = world.get::<FileTreeRoot>(panel).map(|r| r.0.clone());
            let Some(mut state) = world.entity_mut(panel).take::<FileTreePanel>() else {
                continue;
            };
            let reset = state.root_input != root || state.workspace != Some(workspace);
            if reset {
                runtime.backend.remove_view(panel);
                state.reset(workspace, root);
            }
            if reset || state.retirement.is_none() {
                state.retirement = Some(runtime.backend.snapshot_retirement_handle());
            }
            if world
                .get::<FileTreeSettings>(panel)
                .is_some_and(|settings| settings.read_only)
                && state
                    .editing
                    .as_ref()
                    .is_some_and(|edit| edit.pending.is_none())
            {
                state.editing = None;
            }
            if let Some(snapshot) = runtime.backend.snapshot(panel)
                && snapshot.generation == state.generation
                && snapshot.revision == state.revision
                && snapshot.version != state.snapshot.version
            {
                state.view.snapshot_changed(&state.snapshot, &snapshot);
                state.replace_snapshot(snapshot);
                // Absence during a parent reload is not authoritative deletion:
                // a just-created/moved destination may not be in its old listing.
                // Collapsed branches still retire invisible operation targets.
                state.selection.retain(|p| {
                    state.snapshot.row_index(p).is_some()
                        || temporarily_missing(
                            &state.snapshot,
                            &state.expanded,
                            state.root_expanded,
                            p,
                        )
                });
                if state.cursor.as_deref().is_none_or(|p| {
                    state.snapshot.row_index(p).is_none()
                        && !temporarily_missing(
                            &state.snapshot,
                            &state.expanded,
                            state.root_expanded,
                            p,
                        )
                }) {
                    let nearest = state.cursor.as_deref().and_then(|path| {
                        path.ancestors()
                            .find(|ancestor| state.snapshot.row_index(ancestor).is_some())
                            .map(Path::to_owned)
                    });
                    state.cursor =
                        nearest.or_else(|| state.snapshot.rows.first().map(|row| row.path.clone()));
                }
                if let Some(edit) = &state.editing
                    && edit.kind == EditKind::Rename
                    && edit.pending.is_none()
                    && state.snapshot.row_index(&edit.path).is_none()
                    && !temporarily_missing(
                        &state.snapshot,
                        &state.expanded,
                        state.root_expanded,
                        &edit.path,
                    )
                {
                    state.editing = None;
                }
            }
            let root_key = if state.snapshot.root.as_os_str().is_empty() {
                state.root_input.clone()
            } else {
                Some(state.snapshot.root.clone())
            };
            if state.preferences_revision != runtime.preferences_revision
                || state.visibility_root != root_key
            {
                let visibility = root_key
                    .as_ref()
                    .and_then(|root| {
                        world
                            .get::<FileTreePreferences>(workspace)?
                            .roots
                            .get(root)
                            .cloned()
                    })
                    .unwrap_or_default();
                if state.visibility != visibility {
                    state.visibility = visibility;
                    state.changed();
                }
                state.visibility_root = root_key;
                state.preferences_revision = runtime.preferences_revision;
            }
            while let Some(action) = state.actions.pop_front() {
                let title = errors::action_title(&action);
                if let Err(error) =
                    apply_action(world, workspace, panel, &mut runtime, &mut state, action)
                {
                    bedtime_workbench2::request_error_dialog(
                        world,
                        errors::action_dialog(title, error.clone()),
                    );
                    state.error = Some(error.clone());
                    state.reported.action = Some(error.clone());
                    if let Some(edit) = &mut state.editing {
                        edit.error = Some(error);
                    }
                }
            }
            if let Some(root) = &state.root_input
                && state.revision != state.sent_revision
            {
                let request = ViewRequest {
                    panel,
                    generation: state.generation,
                    revision: state.revision,
                    root: root.clone(),
                    expanded: state.expanded.clone(),
                    root_expanded: state.root_expanded,
                    visibility: state.visibility.clone(),
                    show_hidden: state.show_hidden,
                };
                match runtime.backend.set_view(request) {
                    Ok(()) => {
                        state.sent_revision = state.revision;
                    }
                    Err(error) => {
                        state.error = Some(error);
                    }
                }
            }
            state.progress = runtime
                .backend
                .progress()
                .filter(|(id, _)| state.pending.contains(id))
                .map(|(_, progress)| progress);
            if matches!(runtime.backend.status(), WorkStatus::Failed(_)) {
                state.error = Some(format!(
                    "Filesystem worker failed: {:?}",
                    runtime.backend.status()
                ));
            }
            world.entity_mut(panel).insert(state);
        }
        // WorkspaceWork prevents new admission while closing, but the separate
        // result mailbox still lets this system observe already committed writes.
        if world.get_entity(workspace).is_ok() {
            world.entity_mut(workspace).insert(runtime);
        }
    }
    world.resource_scope(|world, mut archive: Mut<OperationArchive>| {
        for (&workspace, pending) in &mut archive.0 {
            let mut results = pending.results.take_operation_results();
            let failed = world.get::<FilesystemRuntime>(workspace)
                .is_some_and(|runtime| matches!(runtime.backend.status(), WorkStatus::Failed(_)));
            if failed || world.get_entity(workspace).is_err() {
                // A panic cannot be interpreted as rollback. Preserve real results
                // first, then explicitly report that remaining effects are unknown.
                let received: BTreeSet<_> = results.iter().map(|r| r.id).collect();
                results.extend(pending.origins.iter().filter(|(id, _)| !received.contains(id)).map(|(&id, origin)| FileTreeOperationResult {
                    id, completed: Vec::new(), cancelled: false,
                    errors: vec![FileTreeOperationError {
                        path: origin.root.clone(), kind: std::io::ErrorKind::BrokenPipe, message: "Filesystem worker stopped without a final result; some effects may have occurred. Refresh and inspect the affected paths before retrying.".into(),
                    }],
                }));
            }
            for result in results {
                if let Some(origin) = pending.origins.remove(&result.id) {
                    reconcile_operation(world, workspace, &origin, &result);
                    world.write_message(FileTreeOperationFinished {
                        workspace, panel: origin.panel, root: origin.root,
                        generation: origin.generation, result: Arc::new(result),
                    });
                }
            }
        }
        archive.0.retain(|_, pending| !pending.origins.is_empty());
    });
    errors::report(world);
}

fn apply_action(
    world: &mut World,
    workspace: Entity,
    panel: Entity,
    runtime: &mut FilesystemRuntime,
    state: &mut FileTreePanel,
    action: Action,
) -> Result<(), String> {
    if matches!(action, Action::Refresh) {
        state.error = None;
        runtime.backend.refresh(panel);
        return Ok(());
    }
    if matches!(action, Action::CancelOperation) {
        for &id in &state.pending {
            runtime.backend.cancel_operation(id);
        }
        return Ok(());
    }
    if world
        .get::<FileTreeSettings>(panel)
        .is_some_and(|settings| settings.read_only)
        && matches!(
            &action,
            Action::NewFile { .. }
                | Action::NewFolder { .. }
                | Action::Rename { .. }
                | Action::SubmitEdit
                | Action::Cut { .. }
                | Action::Paste { .. }
                | Action::Duplicate { .. }
                | Action::Trash { .. }
                | Action::Transfer { .. }
        )
    {
        return Ok(());
    }
    // A rejected drag is a silent no-op, not an editor error or a queued job.
    if matches!(&action, Action::Transfer { .. }) && (state.is_busy() || state.editing.is_some()) {
        return Ok(());
    }
    if state.root_input.is_none() || state.snapshot.root.as_os_str().is_empty() {
        return Err("The application must supply an accessible directory root first.".into());
    }
    let root = state.snapshot.root.clone();
    let mut cut_snapshot = None;
    let new_file = matches!(&action, Action::NewFile { .. });
    let cut_action = matches!(&action, Action::Cut { .. });
    let hide_action = matches!(&action, Action::Hide { .. });
    let submitting_edit = matches!(&action, Action::SubmitEdit);
    let operation = match action {
        Action::Open { path, activation } => {
            let index = state
                .snapshot
                .row_index(&path)
                .ok_or("The activated row is no longer visible")?;
            if state.snapshot.rows[index].kind == FileTreeEntryKind::Directory {
                state.toggle(&path);
            } else {
                world.write_message(FileTreeOpenRequested {
                    panel,
                    workspace,
                    path,
                    activation,
                });
            }
            return Ok(());
        }
        Action::NewFile { directory } | Action::NewFolder { directory } => {
            let kind = if new_file {
                EditKind::File
            } else {
                EditKind::Directory
            };
            state.editing = Some(InlineEdit {
                kind,
                path: directory,
                text: String::new(),
                original: OsString::new(),
                focus: true,
                pending: None,
                error: None,
            });
            return Ok(());
        }
        Action::Rename { path } => {
            if path == root {
                return Err("The directory root cannot be renamed from the tree.".into());
            }
            let original = path
                .file_name()
                .ok_or("The selected path has no filename")?
                .to_os_string();
            state.editing = Some(InlineEdit {
                kind: EditKind::Rename,
                text: original.to_string_lossy().into_owned(),
                original,
                path,
                focus: true,
                pending: None,
                error: None,
            });
            state.view.reveal = state.cursor.clone();
            return Ok(());
        }
        Action::SubmitEdit => {
            let edit = state.editing.as_ref().ok_or("No name is being edited")?;
            if edit.pending.is_some() {
                return Ok(());
            }
            let name =
                if edit.kind == EditKind::Rename && edit.text == edit.original.to_string_lossy() {
                    edit.original.clone()
                } else {
                    OsString::from(&edit.text)
                };
            match edit.kind {
                EditKind::Rename => FileTreeOperation::Rename {
                    path: edit.path.clone(),
                    name,
                },
                EditKind::File => FileTreeOperation::CreateFile {
                    directory: edit.path.clone(),
                    name,
                },
                EditKind::Directory => FileTreeOperation::CreateDirectory {
                    directory: edit.path.clone(),
                    name,
                },
            }
        }
        Action::Copy { paths } | Action::Cut { paths } => {
            let cut = cut_action;
            let paths = operations::top_level_paths(&paths);
            if paths.is_empty() || paths.iter().any(|p| p == &root) {
                return Err("Select files or folders below the directory root.".into());
            }
            let mut clipboard = world.resource::<Clipboard>().clone();
            if cut && clipboard_identity(&clipboard).is_none() {
                return Err("Native cut ownership cannot be verified on this platform. Use drag to move, or Copy/Paste.".into());
            }
            clipboard
                .write_files(&paths)
                .map_err(|e| format!("File clipboard: {e}"))?;
            world.resource_mut::<FileTreeClipboard>().0 = {
                let native_revision = clipboard_identity(&clipboard);
                let clipboard_snapshot = Arc::new(
                    clipboard
                        .read_files_snapshot()
                        .map_err(|e| format!("File clipboard: {e}"))?,
                );
                Some(FileClipboardIntent {
                    is_cut: cut,
                    panel,
                    root,
                    paths,
                    clipboard_revision: clipboard.revision(),
                    native_revision,
                    clipboard_snapshot,
                })
            };
            return Ok(());
        }
        Action::Paste { directory } => {
            let mut clipboard = world.resource::<Clipboard>().clone();
            let paths = clipboard
                .read_files()
                .map_err(|e| format!("File clipboard: {e}"))?;
            let intent = world.resource::<FileTreeClipboard>().0.as_ref();
            let same_write = if let Some(cut) = intent {
                clipboard
                    .matches_files_snapshot(&cut.clipboard_snapshot)
                    .map_err(|e| format!("File clipboard: {e}"))?
            } else {
                false
            };
            let owned = intent
                .filter(|intent| {
                    same_write
                        && intent.clipboard_revision == clipboard.revision()
                        && intent.native_revision.is_some()
                        && intent.native_revision == clipboard_identity(&clipboard)
                })
                .cloned();
            // Native formats canonicalize symlinks. Use the authenticated native
            // snapshot only as ownership proof; our selected paths are the copy
            // or move sources. Otherwise copying a link would copy its target.
            match owned {
                Some(intent)
                    if intent.is_cut
                        && intent.root == root
                        && !world
                            .get::<FileTreeSettings>(intent.panel)
                            .is_some_and(|settings| settings.read_only) =>
                {
                    cut_snapshot = Some(intent.clipboard_snapshot);
                    FileTreeOperation::Move {
                        sources: intent.paths,
                        directory,
                    }
                }
                Some(mut intent) => {
                    let sources = intent.paths.clone();
                    intent.is_cut = false;
                    world.resource_mut::<FileTreeClipboard>().0 = Some(intent);
                    FileTreeOperation::Copy { sources, directory }
                }
                None => {
                    world.resource_mut::<FileTreeClipboard>().0 = None;
                    FileTreeOperation::Copy {
                        sources: paths,
                        directory,
                    }
                }
            }
        }
        Action::Transfer {
            mut sources,
            directory,
            copy,
        } => {
            if !view::transfer_allowed(&sources, &directory, copy) {
                return Ok(());
            }
            if !copy {
                sources.retain(|source| source.parent() != Some(directory.as_path()));
            }
            if copy {
                FileTreeOperation::Copy { sources, directory }
            } else {
                FileTreeOperation::Move { sources, directory }
            }
        }
        Action::Duplicate { paths } => FileTreeOperation::Duplicate { sources: paths },
        Action::Trash { paths } => FileTreeOperation::Trash { sources: paths },
        Action::Hide { paths } | Action::Unhide { paths } => {
            let hide = hide_action;
            let mut preferences = world.get_mut::<FileTreePreferences>(workspace).unwrap();
            let visibility = preferences.roots.entry(root.clone()).or_default();
            for path in paths {
                let relative = path
                    .strip_prefix(&root)
                    .map_err(|_| "Path is outside this root")?;
                if hide {
                    visibility.hide(relative.to_owned())?;
                } else {
                    visibility.unhide(relative);
                }
            }
            drop(preferences);
            runtime.preferences_revision = runtime.preferences_revision.wrapping_add(1);
            world.write_message(FileTreePreferencesChanged { workspace });
            return Ok(());
        }
        Action::ToggleHiddenFiles | Action::ClearHidden => {
            let mut preferences = world.get_mut::<FileTreePreferences>(workspace).unwrap();
            let visibility = preferences.roots.entry(root).or_default();
            if matches!(action, Action::ToggleHiddenFiles) {
                visibility.hide_hidden = !visibility.hide_hidden;
            } else {
                visibility.hidden_paths.clear();
            }
            drop(preferences);
            runtime.preferences_revision = runtime.preferences_revision.wrapping_add(1);
            world.write_message(FileTreePreferencesChanged { workspace });
            return Ok(());
        }
        Action::ToggleRevealHidden => {
            state.show_hidden = !state.show_hidden;
            state.changed();
            return Ok(());
        }
        Action::Refresh | Action::CancelOperation => unreachable!(),
    };
    if state.is_busy() {
        return Err("Wait for this panel's current file operation to finish.".into());
    }
    if state.editing.is_some() && !submitting_edit {
        return Err(
            "Finish or cancel the current filename edit before another file operation.".into(),
        );
    }
    if world
        .get::<WorkspaceWork>(workspace)
        .is_none_or(|scope| scope.is_closing())
    {
        return Err("The workspace is closing.".into());
    }
    runtime.next_operation = runtime
        .next_operation
        .checked_add(1)
        .expect("File operation IDs exhausted");
    let id = runtime.next_operation;
    let error_title = errors::operation_title(&operation);
    runtime.backend.submit_operation(FileTreeOperationRequest {
        id,
        root: root.clone(),
        operation,
    })?;
    let results = runtime.backend.operation_results_handle();
    world
        .resource_mut::<OperationArchive>()
        .0
        .entry(workspace)
        .or_insert_with(|| WorkspaceOperations {
            results,
            origins: HashMap::new(),
        })
        .origins
        .insert(
            id,
            OperationOrigin {
                panel,
                error_title,
                root,
                generation: state.generation,
                cut_snapshot,
            },
        );
    state.pending.insert(id);
    state.error = None;
    if let Some(edit) = &mut state.editing {
        edit.pending = Some(id);
        edit.error = None;
    }
    Ok(())
}

fn temporarily_missing(
    snapshot: &FileTreeSnapshot,
    expanded: &BTreeSet<PathBuf>,
    root_expanded: bool,
    path: &Path,
) -> bool {
    if !path.starts_with(&snapshot.root) {
        return false;
    }
    for ancestor in path.ancestors().skip(1) {
        if let Some(index) = snapshot.row_index(ancestor) {
            let row = &snapshot.rows[index];
            return row.kind == FileTreeEntryKind::Directory
                && (if ancestor == snapshot.root {
                    root_expanded
                } else {
                    expanded.contains(ancestor)
                })
                && matches!(
                    row.directory_state,
                    FileTreeDirectoryState::Loading | FileTreeDirectoryState::Unloaded
                );
        }
    }
    false
}

fn remap(path: &Path, source: &Path, destination: &Path) -> Option<PathBuf> {
    path.strip_prefix(source)
        .ok()
        .map(|suffix| destination.join(suffix))
}
fn remap_set(set: &mut BTreeSet<PathBuf>, source: &Path, destination: &Path) {
    *set = set
        .iter()
        .map(|p| remap(p, source, destination).unwrap_or_else(|| p.clone()))
        .collect();
}

fn reconcile_operation(
    world: &mut World,
    workspace: Entity,
    origin: &OperationOrigin,
    result: &FileTreeOperationResult,
) {
    let panels: Vec<_> = world
        .query::<(Entity, &InWorkspace, &FileTreePanel)>()
        .iter(world)
        .filter(|(_, owner, _)| owner.0 == workspace)
        .map(|(panel, _, _)| panel)
        .collect();
    let mut preferences_changed = false;
    for change in &result.completed {
        if let FileTreeChange::Moved {
            source,
            destination,
        } = change
        {
            for &panel in &panels {
                let mut state = world.get_mut::<FileTreePanel>(panel).unwrap();
                if state.snapshot.root != origin.root {
                    continue;
                }
                remap_set(Arc::make_mut(&mut state.expanded), source, destination);
                remap_set(&mut state.selection, source, destination);
                if let Some(path) = state
                    .cursor
                    .as_deref()
                    .and_then(|p| remap(p, source, destination))
                {
                    state.cursor = Some(path);
                }
                if let Some(path) = state
                    .anchor
                    .as_deref()
                    .and_then(|p| remap(p, source, destination))
                {
                    state.anchor = Some(path);
                }
                state.changed();
            }
            if let (Ok(source), Ok(destination)) = (
                source.strip_prefix(&origin.root),
                destination.strip_prefix(&origin.root),
            ) && let Some(mut preferences) = world.get_mut::<FileTreePreferences>(workspace)
                && let Some(visibility) = preferences.roots.get_mut(&origin.root)
            {
                let old = visibility.hidden_paths.clone();
                remap_set(&mut visibility.hidden_paths, source, destination);
                preferences_changed |= old != visibility.hidden_paths;
            }
        }
    }
    if preferences_changed {
        if let Some(mut runtime) = world.get_mut::<FilesystemRuntime>(workspace) {
            runtime.preferences_revision = runtime.preferences_revision.wrapping_add(1);
        }
        world.write_message(FileTreePreferencesChanged { workspace });
    }
    let mut clipboard_error = None;
    if let Some(snapshot) = &origin.cut_snapshot {
        // An operation may complete after another panel issues a new Cut. Its
        // completion must never consume or rewrite that newer clipboard intent.
        let cut = world
            .resource::<FileTreeClipboard>()
            .0
            .as_ref()
            .filter(|cut| {
                cut.is_cut
                    && Arc::ptr_eq(&cut.clipboard_snapshot, snapshot)
                    && cut.root == origin.root
            })
            .cloned();
        if let Some(mut cut) = cut {
            let mut clipboard = world.resource::<Clipboard>().clone();
            let same_write = match clipboard.matches_files_snapshot(&cut.clipboard_snapshot) {
                Ok(owned) => owned,
                Err(error) => {
                    clipboard_error = Some(format!(
                        "File operation finished, but checking clipboard ownership failed: {error}"
                    ));
                    false
                }
            };
            let owned = same_write
                && clipboard.revision() == cut.clipboard_revision
                && clipboard_identity(&clipboard) == cut.native_revision;
            if owned {
                for change in &result.completed {
                    if let FileTreeChange::Moved { source, .. } = change {
                        cut.paths.retain(|path| !path.starts_with(source));
                    }
                }
                let updated = if cut.paths.is_empty() {
                    clipboard.clear()
                } else {
                    clipboard.write_files(&cut.paths)
                };
                match updated {
                    Ok(()) => {
                        cut.clipboard_revision = clipboard.revision();
                        cut.native_revision = clipboard_identity(&clipboard);
                        if !cut.paths.is_empty() {
                            match clipboard.read_files_snapshot() {
                                Ok(snapshot) => {
                                    cut.clipboard_snapshot = Arc::new(snapshot);
                                }
                                Err(error) => {
                                    clipboard_error = Some(format!(
                                        "Files moved, but observing the remaining clipboard failed: {error}"
                                    ));
                                }
                            }
                        }
                    }
                    Err(error) => {
                        clipboard_error = Some(format!(
                            "Files moved, but updating the clipboard failed: {error}"
                        ));
                    }
                }
                world.resource_mut::<FileTreeClipboard>().0 = if cut.paths.is_empty() {
                    None
                } else {
                    Some(cut)
                };
            } else {
                world.resource_mut::<FileTreeClipboard>().0 = None;
            }
        }
    }
    let dialog = errors::operation_dialog(origin.error_title, result).or_else(|| clipboard_error.as_ref().map(|details| bedtime_workbench2::ErrorDialog {
        title: "Couldn't update the file clipboard".into(),
        message: "The file operation has finished, but the clipboard couldn't be checked or updated. Copy the remaining items again before another paste.".into(),
        details: Some(details.clone()),
    }));
    if let Some(mut state) = world.get_mut::<FileTreePanel>(origin.panel)
        && state.workspace == Some(workspace)
        && state.generation == origin.generation
    {
        state.pending.remove(&result.id);
        state.progress = None;
        let error = if !result.errors.is_empty() {
            Some(
                result
                    .errors
                    .iter()
                    .take(4)
                    .map(|error| format!("{}: {}", error.path.display(), error.message))
                    .collect::<Vec<_>>()
                    .join("\n"),
            )
        } else {
            clipboard_error
        };
        state.error = error.clone();
        state.reported.action = error.clone();
        if state
            .editing
            .as_ref()
            .is_some_and(|edit| edit.pending == Some(result.id))
        {
            if error.is_none() && (!result.cancelled || !result.completed.is_empty()) {
                state.editing = None;
            } else if let Some(edit) = &mut state.editing {
                edit.pending = None;
                edit.error = error;
                edit.focus = true;
            }
        }
        if let Some(path) = result.completed.last().and_then(|change| match change {
            FileTreeChange::Created { path } => Some(path),
            FileTreeChange::Copied { destination, .. }
            | FileTreeChange::Moved { destination, .. } => Some(destination),
            FileTreeChange::Trashed { .. } => None,
        }) {
            state.cursor = Some(path.clone());
            state.selection = BTreeSet::from([path.clone()]);
            state.view.reveal = Some(path.clone());
            if let Some(parent) = path.parent() {
                Arc::make_mut(&mut state.expanded).insert(parent.to_owned());
            }
            state.changed();
        }
    }
    if let Some(dialog) = dialog {
        bedtime_workbench2::request_error_dialog(world, dialog);
    }
}

fn clipboard_identity(clipboard: &Clipboard) -> Option<i64> {
    if clipboard.mode() == bedtime_clipboard2::ClipboardMode::Memory {
        Some(-1)
    } else {
        native_clipboard_revision()
    }
}

#[cfg(target_os = "macos")]
fn native_clipboard_revision() -> Option<i64> {
    // Reading the pasteboard's change counter has no content conversion or I/O.
    Some(objc2_app_kit::NSPasteboard::generalPasteboard().changeCount() as i64)
}
#[cfg(target_os = "windows")]
fn native_clipboard_revision() -> Option<i64> {
    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetClipboardSequenceNumber() -> u32;
    }
    let sequence = unsafe { GetClipboardSequenceNumber() };
    (sequence != 0).then_some(sequence as i64)
}
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn native_clipboard_revision() -> Option<i64> {
    None
}

#[cfg(test)]
mod tests;
