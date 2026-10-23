//! Application policy for local files: explicit roots, text editors and storage.
//! The reusable tree never chooses cwd, a viewer, or a persistence destination.
#[cfg(any(test, feature = "automation"))]
use crate::opening::open_folder;
use crate::opening::{FolderOpens, poll_folder_opens};
#[cfg(test)]
use bedtime_editor2::BufferEdit;
use bedtime_editor2::TextBuffer;
#[cfg(any(test, feature = "automation"))]
use bedtime_filetree2::FileTreePreferences;
#[cfg(feature = "automation")]
use bedtime_filetree2::FileTreeSettings;
use bedtime_filetree2::{
    FileTreeActivation, FileTreeChange, FileTreeOpenRequested, FileTreeOperationFinished,
    FileTreePanel, FileTreePlugin, FileTreeRoot,
};
#[cfg(test)]
use bedtime_filetree2::{FileTreePreferencesChanged, FileTreeVisibility};
use bedtime_ui2::{UiContext, egui};
use bedtime_workbench2::*;
use bevy::{ecs::message::MessageCursor, prelude::*};
#[cfg(any(test, feature = "automation"))]
#[cfg(feature = "automation")]
use std::collections::BTreeSet;
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

const MAX_PREVIEW_BYTES: u64 = 256 * 1024 * 1024;
#[cfg(test)]
const MAX_PREFERENCES_BYTES: u64 = 4 * 1024 * 1024;
const PREVIEW_CHECK_INTERVAL: Duration = Duration::from_millis(500);
const MAX_PREVIEW_CHECKS: usize = 64;

pub struct FilesPlugin;

#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FilesSystems {
    Update,
}

impl Plugin for FilesPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((FileTreePlugin, crate::file_editor::FileEditorPlugin))
            .init_resource::<StartupRoot>()
            .init_resource::<FolderOpens>()
            .init_resource::<FileEvents>()
            .add_systems(
                Update,
                update_files
                    .in_set(FilesSystems::Update)
                    .after(bedtime_filetree2::FileTreeSystems::Update)
                    .before(bedtime_editor2::workbench::update_document_analysis),
            )
            // UI preference changes must be admitted BEFORE workspace.close can
            // cancel storage. No filesystem work happens in this system.
            .add_systems(
                PreUpdate,
                admit_preferences
                    .after(bedtime_ui2::UiSystems::Draw)
                    .after(WorkbenchSystems::Apply)
                    .before(WorkspaceSystems::Dispatch),
            );
        #[cfg(feature = "automation")]
        {
            use bedtime_automation2::AutomationAppExt;
            app.register_automation_command("app.inspect_filetree", inspect)
                .register_automation_command("app.open_folder", |world, arguments| {
                    let path = arguments
                        .get("path")
                        .and_then(|value| value.as_str())
                        .ok_or("path must be a string")?;
                    open_folder(world, PathBuf::from(path))?;
                    Ok(serde_json::json!({"admitted": true}))
                })
                .register_automation_command("app.set_filetree_root", set_tree_root)
                .register_automation_command("app.set_filetree_settings", set_tree_settings)
                .register_automation_command("app.drop_files", drop_files);
        }
    }
}

/// Stable project identity belongs to its workspace, never to the active window.
#[derive(Component, Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceRoot(pub PathBuf);

/// Explicit folder-backed identity. A browsing/cwd root alone never opts into
/// workspace storage: the default home workspace remains entirely in memory.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct FolderWorkspace;

#[derive(Resource, Default)]
struct StartupRoot {
    // None means no application default was requested, including headless hosts.
    // Some(None) requests the user's home root, without enabling project storage.
    requested: Option<Option<PathBuf>>,
    persistent: bool,
    policy_resolved: bool,
    restoring_last: bool,
    loading: Option<(Entity, WorkspaceTask<Result<PathBuf, String>>)>,
    root: Option<PathBuf>,
    assigned: Option<Entity>,
    error: Option<String>,
    reported_error: Option<String>,
}

/// Main calls this explicitly; adding FilesPlugin alone leaves roots unconfigured.
/// Path resolution/canonicalization happens on managed work, not during startup.
pub fn configure_startup(app: &mut App) {
    let argument = std::env::args_os()
        .skip(1)
        .find(|argument| !argument.to_string_lossy().starts_with('-'))
        .map(PathBuf::from);
    app.insert_resource(StartupRoot {
        persistent: argument.is_some(),
        requested: Some(argument),
        ..default()
    });
}

fn home_directory() -> Result<PathBuf, String> {
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("USERPROFILE")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        })
        .ok_or_else(|| "The user's home directory is unavailable".into())
}

/// No terminal may start in a launch-directory fallback while its application
/// root is still resolving. Headless hosts that chose no startup policy remain
/// valid; production's startup request targets exactly one workspace.
pub fn root_ready(world: &World, workspace: Entity) -> bool {
    if world.get::<WorkspaceRoot>(workspace).is_some() {
        return true;
    }
    let Some(startup) = world.get_resource::<StartupRoot>() else {
        return true;
    };
    if startup
        .loading
        .as_ref()
        .is_some_and(|(owner, _)| *owner == workspace)
    {
        return false;
    }
    startup.requested.is_none() && startup.error.is_none()
}

pub fn workspace_directory(world: &World, workspace: Entity) -> Option<PathBuf> {
    world
        .get::<WorkspaceRoot>(workspace)
        .map(|root| root.0.clone())
        .or_else(|| home_directory().ok())
}

pub(crate) fn resolve_root(requested: Option<PathBuf>) -> Result<PathBuf, String> {
    let path = match requested {
        Some(path) if path.is_absolute() => path,
        Some(path) => std::env::current_dir()
            .map_err(|error| error.to_string())?
            .join(path),
        None => home_directory()?,
    };
    let canonical =
        fs::canonicalize(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    if !canonical.is_dir() {
        return Err(format!("{} is not a folder", canonical.display()));
    }
    Ok(canonical)
}

#[cfg(feature = "automation")]
fn lexical_path(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                result.pop();
            }
            other => result.push(other.as_os_str()),
        }
    }
    result
}

#[derive(Resource, Default)]
struct FileEvents {
    opens: MessageCursor<FileTreeOpenRequested>,
    operations: MessageCursor<FileTreeOperationFinished>,
    activations: BTreeMap<Entity, ActivationCounts>,
    errors: BTreeMap<Entity, String>,
}

#[derive(Default)]
struct ActivationCounts {
    single: u64,
    double: u64,
    keyboard: u64,
    context: u64,
}

fn update_files(world: &mut World) {
    update_startup_root(world);
    poll_folder_opens(world);
    configure_panels(world);
    admit_preferences(world);
    world.resource_scope(|world, mut events: Mut<FileEvents>| {
        let opens: Vec<_> = events
            .opens
            .read(world.resource::<Messages<FileTreeOpenRequested>>())
            .cloned()
            .collect();
        for request in opens {
            let counts = events.activations.entry(request.panel).or_default();
            match request.activation {
                FileTreeActivation::SingleClick => counts.single += 1,
                FileTreeActivation::DoubleClick => counts.double += 1,
                FileTreeActivation::Keyboard => counts.keyboard += 1,
                FileTreeActivation::ContextMenu => counts.context += 1,
            }
            match open_requested(world, &request) {
                Ok(_) => {
                    events.errors.remove(&request.panel);
                }
                Err(error) => {
                    request_error_dialog(
                        world,
                        preview_error_dialog(Some(&request.path), error.clone()),
                    );
                    events.errors.insert(request.panel, error);
                }
            }
        }
        let operations: Vec<_> = events
            .operations
            .read(world.resource::<Messages<FileTreeOperationFinished>>())
            .map(|message| message.result.clone())
            .collect();
        for result in operations {
            for change in &result.completed {
                match change {
                    FileTreeChange::Moved {
                        source,
                        destination,
                    } => {
                        move_previews(world, source, destination);
                    }
                    FileTreeChange::Trashed { path } => trash_previews(world, path),
                    _ => {}
                }
            }
        }
        events
            .activations
            .retain(|entity, _| world.get::<FileTreePanel>(*entity).is_some());
        events
            .errors
            .retain(|entity, _| world.get::<FileTreePanel>(*entity).is_some());
    });
    observe_preview_sources(world);
    poll_previews(world);
    report_file_errors(world);
}

fn update_startup_root(world: &mut World) {
    if crate::close::shutting_down(world) {
        return;
    }
    world.resource_scope(|world, mut startup: Mut<StartupRoot>| {
        if !startup.policy_resolved {
            if !crate::storage::global_ready(world) {
                return;
            }
            if startup.requested == Some(None)
                && let Some(preferences) = world.get_resource::<crate::settings::StartupPreferences>()
                && preferences.open_last_workspace
                && let Some(root) = &preferences.last_workspace
            {
                startup.requested = Some(Some(root.clone()));
                startup.persistent = true;
                startup.restoring_last = true;
            }
            startup.policy_resolved = true;
        }
        if let Some((workspace, task)) = &startup.loading {
            match task.try_recv() {
                Ok(Ok(root)) => {
                    if world.get::<Workspace>(*workspace).is_some()
                        && world.get::<WorkspaceRoot>(*workspace).is_none()
                    {
                        world
                            .entity_mut(*workspace)
                            .insert(WorkspaceRoot(root.clone()));
                        if startup.persistent {
                            world.entity_mut(*workspace).insert(FolderWorkspace);
                            let name = root
                                .file_name()
                                .unwrap_or(root.as_os_str())
                                .to_string_lossy()
                                .into_owned();
                            crate::session::initialize_folder_name(world, *workspace, name);
                        }
                    }
                    // Even a retired target consumes this startup request; its
                    // project must not silently migrate to another workspace.
                    startup.assigned = Some(*workspace);
                    startup.root = Some(root);
                    startup.loading = None;
                    startup.requested = None;
                }
                Ok(Err(error)) => {
                    startup.loading = None;
                    if startup.restoring_last {
                        startup.restoring_last = false;
                        startup.persistent = false;
                        startup.requested = Some(None);
                        request_error_dialog(world, ErrorDialog {
                            title: "Open last workspace".into(),
                            message: "The last workspace couldn't be opened. Opening a terminal in your home directory.".into(),
                            details: Some(error),
                        });
                    } else {
                        startup.error = Some(error);
                        startup.requested = None;
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    let closing = world
                        .get::<WorkspaceWork>(*workspace)
                        .is_none_or(|work| work.is_closing());
                    if !closing {
                        startup.error =
                            Some(format!("Root resolution stopped: {:?}", task.status()));
                    }
                    startup.requested = None;
                    startup.loading = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
        }
        if startup.root.is_none()
            && startup.loading.is_none()
            && let Some(requested) = startup.requested.clone()
        {
            let workspace = world
                .query::<(Entity, &WorkspaceWork)>()
                .iter(world)
                .find_map(|(entity, work)| (!work.is_closing()).then_some(entity));
            if let Some(workspace) = workspace {
                let task = world
                    .get_mut::<WorkspaceWork>(workspace)
                    .unwrap()
                    .spawn_task(None, "Resolve project root", move |_| {
                        resolve_root(requested)
                    });
                match task {
                    Ok(task) => startup.loading = Some((workspace, task)),
                    Err(error) => startup.error = Some(error),
                }
            }
        }
    });
}

fn configure_panels(world: &mut World) {
    // The command-line root seeds one workspace only. Later workspaces must
    // explicitly supply their own project identity rather than inheriting it.
    if world.resource::<StartupRoot>().assigned.is_none()
        && let Some(root) = world.resource::<StartupRoot>().root.clone()
    {
        let workspace = world
            .query_filtered::<Entity, With<Workspace>>()
            .iter(world)
            .next();
        if let Some(workspace) = workspace {
            if world.get::<WorkspaceRoot>(workspace).is_none() {
                world.entity_mut(workspace).insert(WorkspaceRoot(root));
                if world.resource::<StartupRoot>().persistent {
                    world.entity_mut(workspace).insert(FolderWorkspace);
                }
            }
            world.resource_mut::<StartupRoot>().assigned = Some(workspace);
        }
    }
    let panels: Vec<_> = world
        .query_filtered::<(Entity, &InWorkspace), (With<FileTreePanel>, Without<FileTreeRoot>)>()
        .iter(world)
        .filter_map(|(panel, owner)| {
            world
                .get::<WorkspaceRoot>(owner.0)
                .map(|root| (panel, root.0.clone()))
        })
        .collect();
    for (panel, root) in panels {
        world.entity_mut(panel).insert(FileTreeRoot(root));
    }
}

/// Focus or open Files in this layout, never the currently active workspace by accident.
#[cfg(test)]
pub fn open_files(world: &mut World, layout: Entity) {
    let result = (|| {
        let mut workbench = Workbench::for_entity(world, layout)?;
        let existing = workbench
            .panels(layout)
            .into_iter()
            .find(|panel| workbench.world().get::<FileTreePanel>(*panel).is_some());
        let panel = if let Some(panel) = existing {
            workbench.focus_panel(panel)?;
            panel
        } else {
            let area = workbench.destination_area(layout)?;
            let panel = workbench
                .world_mut()
                .spawn((FileTreePanel::default(), InArea(area)))
                .id();
            workbench.focus_panel(panel)?;
            panel
        };
        let workspace = workbench.workspace_id().entity();
        let root = workbench
            .world()
            .get::<WorkspaceRoot>(workspace)
            .map(|root| root.0.clone());
        if workbench.world().get::<FileTreeRoot>(panel).is_none()
            && let Some(root) = root
        {
            workbench
                .world_mut()
                .entity_mut(panel)
                .insert(FileTreeRoot(root));
        }
        Ok::<_, String>(())
    })();
    if let Err(error) = result {
        let mut startup = world.resource_mut::<StartupRoot>();
        startup.error = Some(error);
        // A new explicit attempt may report the same failure again.
        startup.reported_error = None;
    }
}

#[derive(Clone, Debug)]
enum PreviewContent {
    Text(TextBuffer),
    Metadata(Arc<str>),
}

#[derive(Debug)]
enum PreviewReadError {
    Missing,
    Failed(String),
}

fn source_is_missing(error: &std::io::Error) -> bool {
    // A parent replaced by a file also proves this source path no longer exists.
    matches!(
        error.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
    )
}

fn preview_read_error(path: &Path, error: std::io::Error) -> PreviewReadError {
    if source_is_missing(&error) {
        PreviewReadError::Missing
    } else {
        PreviewReadError::Failed(format!("{}: {error}", path.display()))
    }
}

struct PreviewLoad {
    generation: u64,
    path: PathBuf,
    workspace: Entity,
    document_stamp: Option<(DocumentId, u64)>,
    handle: WorkspaceTask<Result<PreviewContent, PreviewReadError>>,
}

/// A replaceable file tab. Text binds to a shared document; unsupported files
/// keep a bounded metadata presentation. The loaded bytes are a source snapshot.
#[derive(Component, Default)]
#[require(PanelInstance = PanelInstance::of::<Self>())]
pub struct FilePreviewPanel {
    path: Option<PathBuf>,
    generation: u64,
    load: Option<PreviewLoad>,
    content: Option<PreviewContent>,
    error: Option<String>,
    // One-shot presentation admission for actual read/load failures. A missing
    // source awaiting an own move's outcome is diagnostic only, not a dialog.
    report_error: bool,
    missing: bool,
    editor_rect: Option<egui::Rect>,
    caret_rect: Option<egui::Rect>,
    rendered_lines: usize,
    laid_out_bytes: usize,
    editor_error: Option<String>,
}

impl Panel for FilePreviewPanel {
    const KEY: &'static str = "bed.code";
    const LABEL: &'static str = "Code";

    fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, panel: Entity) {
        let surface = *world.get::<WorkbenchSurface>(context.surface).unwrap();
        let state = world.get::<Self>(panel).unwrap();
        let path = state.path.clone();
        let loading = state.load.is_some();
        let metadata = match &state.content {
            Some(PreviewContent::Metadata(summary)) => Some(summary.clone()),
            _ => None,
        };
        let document = world.get::<DocumentView>(panel).map(DocumentView::document);
        let info = document.and_then(|id| inspect_document(world, id).ok());
        let mut reload = false;
        let mut resolution = None;
        context.frame(ui).padded(surface.padding, |frame| {
                    let ui = frame.ui;
                    if document.is_none() {
                        if let Some(path) = &path {
                            ui.label(path.to_string_lossy());
                        } else {
                            ui.weak("Open a file from Files.");
                        }
                    }
                    if loading {
                        ui.spinner();
                    }
                    if let Some(info) = &info {
                        if let Some(error) = &info.last_save_error { ui.colored_label(ui.visuals().error_fg_color, error.to_string()); }
                        if info.conflict {
                            ui.horizontal_wrapped(|ui| {
                                ui.label("The file changed on disk.");
                                if info.requires_source_refresh { if ui.button("Reload to compare").clicked() { reload = true; } }
                                else {
                                    if ui.button("Keep my edits").clicked() { resolution = Some(DocumentConflictResolution::KeepLocal); }
                                    if ui.button("Use disk contents").clicked() { resolution = Some(DocumentConflictResolution::AcceptIncoming); }
                                }
                            });
                        }
                    }
                    if document.is_some() {
                        let language_tools = bedtime_lsp2::workbench::language_activity(world,panel).is_some();
                        if let Some(mut editor) = world.get_mut::<bedtime_editor2::workbench::WorkbenchEditor>(panel) { editor.view.language_tools = language_tools; }
                        match bedtime_editor2::workbench::draw_editor(ui, world, context, panel) {
                            Ok(output) => {
                                crate::lsp::editor_input(ui, world, panel, &output);
                                output.response.context_menu(|ui| {
                                    crate::menus::editor_context_menu(ui, world, panel);
                                    crate::lsp::context_menu(ui, world, panel);
                                });
                                let mut state = world.get_mut::<Self>(panel).unwrap();
                                state.editor_rect = Some(output.text_rect);
                                state.caret_rect = Some(output.caret_rect);
                                state.rendered_lines = output.rendered_lines;
                                state.laid_out_bytes = output.laid_out_bytes;
                                if info.as_ref().is_some_and(|info| info.revision != output.revision) { state.editor_error = None; }
                            }
                            Err(error) => {
                                let composing = world.get::<bedtime_editor2::workbench::WorkbenchEditor>(panel).is_some_and(|editor| editor.view.is_composing());
                                let error = error.to_string();
                                if !composing && world.get::<Self>(panel).unwrap().editor_error.as_ref() != Some(&error) {
                                    world.get_mut::<Self>(panel).unwrap().editor_error = Some(error.clone());
                                    request_error_dialog(world,ErrorDialog { title: "Unable to apply edit".into(), message: "Your edit was not applied. Finish text composition in other views, or reload the document, then try again.".into(), details: Some(error) });
                                }
                            }
                        }
                    } else if let Some(summary) = &metadata {
                        egui::ScrollArea::vertical().show(ui, |ui| { ui.monospace(summary.as_ref()); });
                    }
        });
        if document.is_some_and(|id| {
            inspect_document(world, id).is_ok_and(|info| info.dirty || info.has_drafts)
        }) {
            world.get_mut::<PanelInstance>(panel).unwrap().preview = false;
        }
        let result = (|| {
            if let Some(resolution) = resolution {
                let id = document.unwrap();
                let info = inspect_document(world, id).map_err(|e| e.to_string())?;
                let handle = cast_document::<TextBuffer>(world, id).map_err(|e| e.to_string())?;
                resolve_document_conflict(
                    world,
                    handle,
                    info.revision,
                    info.source_generation,
                    resolution,
                )
                .map_err(|e| e.to_string())?;
            }
            if reload && let Some(path) = path {
                load_preview(world, panel, path, true)?;
            }
            Ok::<_, String>(())
        })();
        if let Err(error) = result {
            let mut state = world.get_mut::<Self>(panel).unwrap();
            state.error = Some(error);
            state.report_error = true;
        }
    }
}

#[cfg(test)]
fn split_editor(world: &mut World, panel: Entity) -> Result<Entity, String> {
    let id = world
        .get::<DocumentView>(panel)
        .ok_or("No editor document")?
        .document();
    let handle = cast_document::<TextBuffer>(world, id).map_err(|e| e.to_string())?;
    let original = world.get::<FilePreviewPanel>(panel).unwrap();
    let path = original.path.clone();
    let content = original.content.clone();
    let title = world.get::<PanelInstance>(panel).unwrap().title.clone();
    let source_area = world.get::<InArea>(panel).unwrap().0;
    let mut workbench = Workbench::for_entity(world, panel)?;
    let bounds = workbench.world().get::<Area>(source_area).unwrap().rect;
    let at = bounds.min[0] + (bounds.max[0] - bounds.min[0]) / 2;
    let area = workbench.split_area(source_area, SplitAxis::Horizontal, at, [1, 1])?;
    let other = workbench
        .world_mut()
        .spawn((
            FilePreviewPanel {
                path,
                content,
                ..Default::default()
            },
            InArea(area),
        ))
        .id();
    let mut instance = workbench
        .world_mut()
        .get_mut::<PanelInstance>(other)
        .unwrap();
    instance.title = title;
    instance.preview = false;
    bedtime_editor2::workbench::bind_text_editor(workbench.world_mut(), other, handle)
        .map_err(|e| e.to_string())?;
    crate::settings::configure_editor(workbench.world_mut(), other);
    workbench.focus_panel(other)?;
    Ok(other)
}

fn open_requested(world: &mut World, request: &FileTreeOpenRequested) -> Result<Entity, String> {
    let owner = world
        .get::<InWorkspace>(request.panel)
        .ok_or("Source Files panel closed")?
        .0;
    if owner != request.workspace || world.get::<FileTreePanel>(request.panel).is_none() {
        return Err("File open request no longer belongs to its source workspace".into());
    }
    if !request.path.is_absolute() {
        return Err("File source path must be absolute".into());
    }
    let tree = world.get::<FileTreePanel>(request.panel).unwrap();
    let root = if tree.snapshot().root.as_os_str().is_empty() {
        &world
            .get::<FileTreeRoot>(request.panel)
            .ok_or("Files panel is unconfigured")?
            .0
    } else {
        &tree.snapshot().root
    };
    if !request.path.starts_with(root) {
        return Err("File open request belongs to an older root".into());
    }
    let workbench = Workbench::for_entity(world, request.panel)?;
    let layout = workbench.panel_layout(request.panel)?;
    let mode = match request.activation {
        FileTreeActivation::SingleClick => crate::opening::FileOpenMode::Preview,
        FileTreeActivation::ContextMenu => crate::opening::FileOpenMode::KeepOpen,
        FileTreeActivation::DoubleClick | FileTreeActivation::Keyboard => {
            crate::opening::FileOpenMode::Focus
        }
    };
    crate::opening::open_file(world, layout, request.path.clone(), mode).map(|opened| opened.panel)
}

/// A selected file preview supplies Structure even while Files retains focus.
pub(crate) fn select_structure_source(world: &mut World, panel: Entity) {
    let Some(owner) = world.get::<InWorkspace>(panel).map(|owner| owner.0) else {
        return;
    };
    let source = bedtime_editor2::workbench::code_document_for_panel(world, panel)
        .map(|document| bed_structure2::SourceBinding { panel, document });
    world
        .entity_mut(owner)
        .insert(bed_structure2::StructureSource { source });
}

/// A session descriptor, not canonical content or an ECS identity.
pub fn panel_path(world: &World, panel: Entity) -> Option<PathBuf> {
    world
        .get::<FilePreviewPanel>(panel)
        .and_then(|state| state.path.clone())
}

pub(crate) fn file_open_generation(world: &World, panel: Entity) -> u64 {
    world.get::<FilePreviewPanel>(panel).unwrap().generation
}

pub(crate) fn file_open_result(
    world: &World,
    panel: Entity,
    generation: u64,
) -> Option<Result<(), String>> {
    let Some(viewer) = world.get::<FilePreviewPanel>(panel) else {
        return Some(Err("File viewer closed before opening completed".into()));
    };
    if viewer.generation != generation {
        return Some(Err("File viewer changed before opening completed".into()));
    }
    if viewer.load.is_some() {
        return None;
    }
    Some(if let Some(error) = &viewer.error {
        Err(error.clone())
    } else if viewer.missing {
        Err("File no longer exists".into())
    } else if viewer.content.is_none() {
        Err("File content is unavailable".into())
    } else {
        Ok(())
    })
}

/// Restore file capabilities on an already composed concrete Code panel. Reading
/// remains asynchronous and uses the ordinary generation-gated document flow.
pub(crate) fn load_preview(
    world: &mut World,
    panel: Entity,
    path: PathBuf,
    retain: bool,
) -> Result<(), String> {
    let workspace = world
        .get::<InWorkspace>(panel)
        .ok_or("Preview has no workspace")?
        .0;
    let same_source = crate::file_editor::same_source(world, panel, &path);
    if world.get::<DocumentDraft>(panel).is_some() {
        return Err("Finish text composition before reloading or replacing this editor.".into());
    }
    if !same_source && !crate::file_editor::can_replace(world, panel) {
        return Err("This editor has unsaved changes or a pending save. Keep it open before opening another file.".into());
    }
    let document_stamp = crate::file_editor::load_stamp(world, panel, &path);
    let snapshots = crate::file_editor::load_snapshots(world, panel, &path);
    if !same_source {
        crate::file_editor::detach(world, panel)?;
    }
    let generation = {
        let mut state = world
            .get_mut::<FilePreviewPanel>(panel)
            .ok_or("Missing file preview")?;
        if let Some(load) = state.load.take() {
            load.handle.cancel();
        }
        state.generation = state
            .generation
            .checked_add(1)
            .expect("Preview generation exhausted");
        state.path = Some(path.clone());
        state.error = None;
        state.report_error = false;
        state.missing = false;
        if !retain {
            state.content = None;
        }
        state.generation
    };
    world.get_mut::<PanelInstance>(panel).unwrap().title = path
        .file_name()
        .unwrap_or_else(|| path.as_os_str())
        .to_string_lossy()
        .into_owned();
    let source = path.clone();
    let task = if let Some(mut work) = world.get_mut::<WorkspaceWork>(workspace) {
        work.spawn_task(Some(panel), "Read file preview", move |cancel| {
            if cancel.is_cancelled() {
                return Err(PreviewReadError::Failed("Preview cancelled".into()));
            }
            let mut content = read_preview(&source)?;
            if let PreviewContent::Text(text) = &mut content {
                if let Some(snapshot) = snapshots.iter().find(|snapshot| *snapshot == text) {
                    *text = snapshot.clone();
                }
            }
            Ok(content)
        })
    } else {
        Err("Missing workspace work".into())
    };
    match task {
        Ok(handle) => {
            world.get_mut::<FilePreviewPanel>(panel).unwrap().load = Some(PreviewLoad {
                generation,
                path,
                workspace,
                document_stamp,
                handle,
            });
            Ok(())
        }
        Err(error) => {
            let mut state = world.get_mut::<FilePreviewPanel>(panel).unwrap();
            state.error = Some(error.clone());
            state.report_error = true;
            Err(error)
        }
    }
}

fn read_preview(path: &Path) -> Result<PreviewContent, PreviewReadError> {
    let metadata = fs::metadata(path).map_err(|error| preview_read_error(path, error))?;
    if !metadata.is_file() {
        return Ok(PreviewContent::Metadata(Arc::from(
            "Not a regular file; content was not read.",
        )));
    }
    let summary = |reason: &str| {
        PreviewContent::Metadata(Arc::from(format!(
            "{reason}\nSize: {} bytes\nRead-only permissions: {}\nModified: {:?}",
            metadata.len(),
            metadata.permissions().readonly(),
            metadata.modified().ok()
        )))
    };
    if metadata.len() > MAX_PREVIEW_BYTES {
        return Ok(summary("File is larger than the 256 MiB text file limit."));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    File::open(path)
        .and_then(|file| file.take(MAX_PREVIEW_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|error| preview_read_error(path, error))?;
    if bytes.len() as u64 > MAX_PREVIEW_BYTES {
        return Ok(summary("File grew beyond the 256 MiB text file limit."));
    }
    if bytes.contains(&0) {
        return Ok(summary("Binary file; content is not shown as text."));
    }
    match String::from_utf8(bytes) {
        Ok(text) => Ok(PreviewContent::Text(TextBuffer::from(text))),
        Err(_) => Ok(summary(
            "Binary or non-UTF-8 file; content is not shown as text.",
        )),
    }
}

fn poll_previews(world: &mut World) {
    let mutations_pending = preview_mutations_pending(world);
    let panels: Vec<_> = world
        .query_filtered::<Entity, With<FilePreviewPanel>>()
        .iter(world)
        .collect();
    for panel in panels {
        let owner = world.get::<InWorkspace>(panel).map(|owner| owner.0);
        let closing = owner
            .and_then(|owner| world.get::<WorkspaceWork>(owner))
            .is_some_and(WorkspaceWork::is_closing);
        let mut state = world.get_mut::<FilePreviewPanel>(panel).unwrap();
        let Some(load) = &state.load else { continue };
        if owner != Some(load.workspace)
            || state.path.as_ref() != Some(&load.path)
            || state.generation != load.generation
        {
            load.handle.cancel();
            state.load = None;
            continue;
        }
        let mut report_failure = true;
        let result = match load.handle.try_recv() {
            Ok(result) => Some(result),
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                let status = load.handle.status();
                // Keep cancellation diagnostics, but a normal workspace shutdown
                // is not an actionable file error for the user to dismiss.
                report_failure =
                    !closing || !matches!(status, WorkStatus::Cancelling | WorkStatus::Cancelled);
                Some(Err(PreviewReadError::Failed(format!(
                    "Preview worker stopped: {status:?}"
                ))))
            }
        };
        if let Some(result) = result {
            let stamp = state.load.as_ref().and_then(|load| load.document_stamp);
            let path = state.path.clone();
            state.load = None;
            match result {
                Ok(content) => {
                    state.content = Some(content.clone());
                    state.error = None;
                    state.missing = false;
                    if let (Some(path), PreviewContent::Text(text)) = (path, content) {
                        if let Err(error) =
                            crate::file_editor::bind_loaded(world, panel, &path, &text, stamp)
                        {
                            let mut state = world.get_mut::<FilePreviewPanel>(panel).unwrap();
                            state.error = Some(error);
                            state.report_error = true;
                        }
                    } else if world.get::<DocumentView>(panel).is_some() {
                        if crate::file_editor::can_replace(world, panel) {
                            let _ = crate::file_editor::detach(world, panel);
                        } else {
                            let mut state = world.get_mut::<FilePreviewPanel>(panel).unwrap();
                            state.error = Some("The source is no longer a supported text file. Your edits were retained.".into());
                            state.report_error = true;
                        }
                    }
                }
                Err(PreviewReadError::Missing) if !mutations_pending => {
                    unload_deleted_preview(world, panel);
                }
                Err(PreviewReadError::Missing) => {
                    // Wait for an own move's confirmed rebind before treating
                    // its old path's absence as deletion. Observation retries.
                    state.error = Some("Source is currently unavailable.".into());
                }
                Err(PreviewReadError::Failed(error)) => {
                    state.error = Some(error);
                    state.report_error = report_failure;
                }
            }
        }
    }
}

fn move_previews(world: &mut World, source: &Path, destination: &Path) {
    crate::file_editor::move_sources(world, source, destination);
    // The physical source is shared even across layouts/workspaces. Only an own
    // confirmed outcome supplies identity; external renames are disappearance.
    let moved: Vec<_> = world
        .query::<(Entity, &FilePreviewPanel)>()
        .iter(world)
        .filter_map(|(panel, state)| {
            let suffix = state.path.as_ref()?.strip_prefix(source).ok()?;
            let path = if suffix.as_os_str().is_empty() {
                destination.to_owned()
            } else {
                destination.join(suffix)
            };
            Some((panel, path))
        })
        .collect();
    for (panel, path) in moved {
        if world.get::<DocumentView>(panel).is_some() {
            // A confirmed move changes identity, not document content. Keep
            // edits, history and active composition without starting a reload.
            world.get_mut::<FilePreviewPanel>(panel).unwrap().path = Some(path.clone());
            world.get_mut::<PanelInstance>(panel).unwrap().title = path
                .file_name()
                .unwrap_or(path.as_os_str())
                .to_string_lossy()
                .into_owned();
        } else {
            let _ = load_preview(world, panel, path, true);
        }
    }
}

fn trash_previews(world: &mut World, path: &Path) {
    let deleted: Vec<_> = world
        .query::<(Entity, &FilePreviewPanel)>()
        .iter(world)
        .filter_map(|(panel, state)| {
            state
                .path
                .as_ref()
                .filter(|source| source.starts_with(path))?;
            Some(panel)
        })
        .collect();
    for panel in deleted {
        unload_deleted_preview(world, panel);
    }
}

/// Both confirmed Trash outcomes and independent filesystem observations enter
/// here. Never retain deleted-source bytes or a reload association. Sequential
/// closes leave one unloaded tab if every tab in an area lost its source.
fn unload_deleted_preview(world: &mut World, panel: Entity) {
    let retained = crate::file_editor::retain_deleted(world, panel);
    if !retained {
        if let Err(error) = crate::file_editor::detach(world, panel) {
            if let Some(mut state) = world.get_mut::<FilePreviewPanel>(panel) {
                state.error = Some(error);
                state.report_error = true;
            }
            return;
        }
    }
    let Some(mut state) = world.get_mut::<FilePreviewPanel>(panel) else {
        return;
    };
    if let Some(load) = state.load.take() {
        load.handle.cancel();
    }
    state.generation = state
        .generation
        .checked_add(1)
        .expect("Preview generation exhausted");
    state.path = None;
    if !retained {
        state.content = None;
    }
    state.error = None;
    state.report_error = false;
    state.missing = true;
    if retained {
        return;
    }
    let mut instance = world.get_mut::<PanelInstance>(panel).unwrap();
    instance.title = FilePreviewPanel::LABEL.into();
    instance.preview = false;
    let has_other_tabs = world
        .get::<InArea>(panel)
        .is_some_and(|area| panels_in_area(world, area.0).len() > 1);
    if has_other_tabs {
        // Use Workbench's close path to retain tab selection/layout invariants.
        // These read-only viewers have no document draft or save to preflight.
        if let Err(error) = Workbench::for_entity(world, panel)
            .and_then(|mut workbench| workbench.close_panel(panel))
        {
            world.get_mut::<FilePreviewPanel>(panel).unwrap().error = Some(error);
        }
    }
}

#[derive(Clone)]
struct PreviewSource {
    panel: Entity,
    path: PathBuf,
    generation: u64,
}

/// Short, bounded workspace-owned tasks, not one watcher/thread per viewer.
/// No tree snapshot can prove absence outside its lazy, filtered projection.
#[derive(Component, Default)]
struct PreviewObservation {
    pending: Option<WorkspaceTask<Vec<PreviewSource>>>,
    next_check: Option<Instant>,
    cursor: Option<Entity>,
}

fn missing_preview_sources(
    sources: Vec<PreviewSource>,
    cancel: Cancellation,
) -> Vec<PreviewSource> {
    let mut observations = BTreeMap::new();
    sources
        .into_iter()
        .take_while(|_| !cancel.is_cancelled())
        .filter(|source| {
            *observations.entry(source.path.clone()).or_insert_with(|| {
                // Match the preview reader's source semantics (including
                // symlink targets). Permission/temporary errors are not absence.
                fs::metadata(&source.path).is_err_and(|error| source_is_missing(&error))
            })
        })
        .collect()
}

fn preview_mutations_pending(world: &mut World) -> bool {
    // A rename can remove the old path before its outcome reaches the main
    // World. Defer absence decisions during admitted mutations, and reconcile
    // outcomes first. This also covers a source viewed in another workspace.
    world
        .query_filtered::<Entity, With<Workspace>>()
        .iter(world)
        .any(|workspace| bedtime_filetree2::filetree_operation_count(world, workspace) != 0)
}

fn observe_preview_sources(world: &mut World) {
    let mutations_pending = preview_mutations_pending(world);
    let mut sources = BTreeMap::<Entity, BTreeMap<Entity, PreviewSource>>::new();
    for (panel, owner, state) in world
        .query::<(Entity, &InWorkspace, &FilePreviewPanel)>()
        .iter(world)
    {
        if let Some(path) = &state.path {
            sources.entry(owner.0).or_default().insert(
                panel,
                PreviewSource {
                    panel,
                    path: path.clone(),
                    generation: state.generation,
                },
            );
        }
    }
    let retired: Vec<_> = world
        .query_filtered::<Entity, With<PreviewObservation>>()
        .iter(world)
        .filter(|workspace| !sources.contains_key(workspace))
        .collect();
    for workspace in retired {
        if let Some(observation) = world.entity_mut(workspace).take::<PreviewObservation>()
            && let Some(task) = observation.pending
        {
            task.cancel();
        }
    }
    for (workspace, sources) in sources {
        let Some(work) = world.get::<WorkspaceWork>(workspace) else {
            continue;
        };
        if work.is_closing() {
            continue;
        }
        world
            .entity_mut(workspace)
            .entry::<PreviewObservation>()
            .or_default();
        let mut observation = world.get_mut::<PreviewObservation>(workspace).unwrap();
        let result = observation
            .pending
            .as_ref()
            .and_then(|task| match task.try_recv() {
                Ok(missing) => Some(missing),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => Some(Vec::new()),
                Err(std::sync::mpsc::TryRecvError::Empty) => None,
            });
        if let Some(missing) = result {
            observation.pending = None;
            // Discard, rather than postpone, observations that overlap an own
            // operation. The next batch rechecks the current source binding.
            if !mutations_pending {
                for source in missing {
                    if world
                        .get::<InWorkspace>(source.panel)
                        .is_some_and(|owner| owner.0 == workspace)
                        && world
                            .get::<FilePreviewPanel>(source.panel)
                            .is_some_and(|state| {
                                state.generation == source.generation
                                    && state.path.as_ref() == Some(&source.path)
                            })
                    {
                        unload_deleted_preview(world, source.panel);
                    }
                }
            }
        }
        let observation = world.get::<PreviewObservation>(workspace).unwrap();
        let now = Instant::now();
        if mutations_pending
            || observation.pending.is_some()
            || observation.next_check.is_some_and(|next| now < next)
        {
            continue;
        }
        // Rotate over all viewers, including hidden/inactive layouts and paths
        // outside every current tree root; no hard cap silently drops a source.
        let cursor = observation.cursor;
        let batch: Vec<_> = sources
            .values()
            .filter(|source| cursor.is_none_or(|cursor| source.panel > cursor))
            .chain(
                sources
                    .values()
                    .filter(|source| cursor.is_some_and(|cursor| source.panel <= cursor)),
            )
            .take(MAX_PREVIEW_CHECKS)
            .filter(|source| {
                world
                    .get::<FilePreviewPanel>(source.panel)
                    .is_some_and(|state| {
                        state.path.as_ref() == Some(&source.path)
                            && state.generation == source.generation
                    })
            })
            .cloned()
            .collect();
        let last = batch.last().map(|source| source.panel);
        if batch.is_empty() {
            continue;
        }
        let task = world
            .get_mut::<WorkspaceWork>(workspace)
            .unwrap()
            .spawn_task(None, "Observe file preview sources", move |cancel| {
                missing_preview_sources(batch, cancel)
            });
        let mut observation = world.get_mut::<PreviewObservation>(workspace).unwrap();
        observation.next_check = Some(now + PREVIEW_CHECK_INTERVAL);
        observation.cursor = last;
        if let Ok(task) = task {
            observation.pending = Some(task);
        }
    }
}

// Capture application metadata before workspace retirement can remove its owner.
pub fn admit_preferences(world: &mut World) {
    // Remember only a resolved active folder. The temporary home workspace does
    // not erase the last folder, and preference loading must finish first.
    if crate::storage::global_ready(world) {
        let root = world
            .query_filtered::<&WorkspaceRoot, (With<ActiveWorkspace>, With<FolderWorkspace>)>()
            .iter(world)
            .next()
            .map(|root| root.0.clone());
        if let Some(root) = root
            && let Some(mut preferences) =
                world.get_resource_mut::<crate::settings::StartupPreferences>()
        {
            preferences.last_workspace = Some(root);
        }
    }
    crate::session::admit(world);
}

pub fn flush_preferences(world: &mut World) -> Result<bool, String> {
    if !world.contains_resource::<FileEvents>() {
        return Ok(true);
    }
    // Startup identity is part of the persistence barrier: don't discover a
    // folder-backed workspace only after the final flush/cancellation point.
    update_startup_root(world);
    if world.resource::<StartupRoot>().loading.is_some() {
        return Ok(false);
    }
    if world.resource::<StartupRoot>().requested.is_some() {
        let has_target = world
            .query::<&WorkspaceWork>()
            .iter(world)
            .any(|work| !work.is_closing());
        if has_target {
            return Ok(false);
        }
        world.resource_mut::<StartupRoot>().requested = None;
    }
    let workspaces: Vec<_> = world
        .query_filtered::<Entity, With<Workspace>>()
        .iter(world)
        .collect();
    if workspaces
        .iter()
        .any(|&workspace| bedtime_filetree2::filetree_operation_count(world, workspace) != 0)
    {
        return Ok(false);
    }
    Ok(true)
}

#[cfg(test)]
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    crate::storage::atomic_write(path, bytes)
}

fn preview_error_dialog(path: Option<&Path>, error: String) -> ErrorDialog {
    ErrorDialog {
        title: path.and_then(Path::file_name).map_or_else(
            || "File preview".into(),
            |name| format!("File preview — {}", name.to_string_lossy()),
        ),
        message: "We couldn't load this file. Check access and try Reload.".into(),
        details: Some(match path {
            Some(path) => format!("File: {}\n\n{error}", path.display()),
            None => error,
        }),
    }
}

/// Presentation is admitted from Update, not panel/titlebar drawing: hidden
/// previews and background preference failures must be reported as well. Keep
/// raw diagnostics for inspection, but do not reopen a dismissed persistent error.
fn report_file_errors(world: &mut World) {
    let mut dialogs = Vec::new();
    {
        let mut startup = world.resource_mut::<StartupRoot>();
        if startup.error != startup.reported_error {
            startup.reported_error = startup.error.clone();
            if let Some(error) = &startup.error {
                dialogs.push(ErrorDialog {
                    title: "Files".into(),
                    message: "We couldn't open Files. Check the project folder and try again."
                        .into(),
                    details: Some(error.clone()),
                });
            }
        }
    }
    for mut state in world.query::<&mut FilePreviewPanel>().iter_mut(world) {
        if !state.report_error {
            continue;
        }
        state.report_error = false;
        if let Some(error) = &state.error {
            dialogs.push(preview_error_dialog(state.path.as_deref(), error.clone()));
        }
    }
    for dialog in dialogs {
        request_error_dialog(world, dialog);
    }
}

#[cfg(feature = "automation")]
fn set_tree_root(
    world: &mut World,
    arguments: serde_json::Value,
) -> bedtime_automation2::AutomationResult {
    let entity = arguments
        .get("panel")
        .and_then(|value| value.as_str())
        .ok_or("panel must be an entity ID string")?
        .parse::<u64>()
        .map_err(|error| error.to_string())?;
    let entity = Entity::try_from_bits(entity).ok_or("Invalid panel entity ID")?;
    if world.get::<FileTreePanel>(entity).is_none() {
        return Err("Not a live Files panel".into());
    }
    let path = PathBuf::from(
        arguments
            .get("path")
            .and_then(|value| value.as_str())
            .ok_or("path must be a string")?,
    );
    if !path.is_absolute() {
        return Err("Automation roots must be absolute".into());
    }
    // FileTree owns async canonicalization/scanning and generation gating.
    world
        .entity_mut(entity)
        .insert(FileTreeRoot(lexical_path(&path)));
    Ok(serde_json::json!({"panel": entity.to_bits().to_string()}))
}

#[cfg(feature = "automation")]
fn set_tree_settings(
    world: &mut World,
    arguments: serde_json::Value,
) -> bedtime_automation2::AutomationResult {
    let object = arguments.as_object().ok_or("Expected an argument object")?;
    if object
        .keys()
        .any(|key| !matches!(key.as_str(), "panel" | "read_only" | "indent_size"))
    {
        return Err(
            "app.set_filetree_settings accepts only panel, read_only and indent_size".into(),
        );
    }
    let bits = object
        .get("panel")
        .and_then(|value| value.as_str())
        .ok_or("panel must be an entity ID string")?
        .parse::<u64>()
        .map_err(|_| "Invalid panel entity ID")?;
    let panel = Entity::try_from_bits(bits).ok_or("Invalid panel entity ID")?;
    if world.get::<FileTreePanel>(panel).is_none() {
        return Err("Not a live Files panel".into());
    }
    if !object.contains_key("read_only") && !object.contains_key("indent_size") {
        return Err("Supply read_only and/or indent_size".into());
    }
    let mut settings = world
        .get::<FileTreeSettings>(panel)
        .copied()
        .unwrap_or_default();
    if let Some(value) = object.get("read_only") {
        settings.read_only = value.as_bool().ok_or("read_only must be a boolean")?;
    }
    if let Some(value) = object.get("indent_size") {
        let indent = value
            .as_f64()
            .ok_or("indent_size must be a finite number in 0..=64")?;
        if !indent.is_finite() || !(0.0..=64.0).contains(&indent) {
            return Err("indent_size must be a finite number in 0..=64".into());
        }
        settings.indent_size = indent as f32;
    }
    // Commit only after the complete request validates. Unlike direct component
    // composition this automation boundary rejects, rather than clamps, input.
    world.entity_mut(panel).insert(settings);
    Ok(serde_json::json!({
        "panel": panel.to_bits().to_string(),
        "settings": {"read_only": settings.read_only, "indent_size": settings.indent_size},
    }))
}

/// Inject the upstream Bevy window message, not a tree action or an egui drop.
/// This exercises bevy_egui's ordinary conversion to raw.dropped_files. It is
/// evidence of event-bridge handling, NOT an OS Finder/Explorer drag simulation.
#[cfg(feature = "automation")]
fn drop_files(
    world: &mut World,
    arguments: serde_json::Value,
) -> bedtime_automation2::AutomationResult {
    use bevy::window::{FileDragAndDrop, PrimaryWindow};
    let object = arguments.as_object().ok_or("Expected an argument object")?;
    if object
        .keys()
        .any(|key| !matches!(key.as_str(), "paths" | "window"))
    {
        return Err("app.drop_files accepts only paths and optional window".into());
    }
    let values = object
        .get("paths")
        .and_then(|value| value.as_array())
        .ok_or("paths must be an array of absolute path strings")?;
    if values.is_empty() || values.len() > 32 {
        return Err("paths must contain 1 through 32 entries".into());
    }
    // Validate the COMPLETE request before admitting any native input message.
    // Filesystem existence/type checks belong to the ordinary tree worker.
    let mut paths = Vec::with_capacity(values.len());
    let mut unique = BTreeSet::new();
    for value in values {
        let text = value.as_str().ok_or("Each path must be a string")?;
        let path = PathBuf::from(text);
        if text.contains('\0') || !path.is_absolute() {
            return Err("Each dropped path must be absolute and contain no NUL".into());
        }
        if !unique.insert(path.clone()) {
            return Err("Duplicate dropped paths are not allowed".into());
        }
        paths.push(path);
    }
    let window = if let Some(value) = object.get("window") {
        let bits = value
            .as_str()
            .ok_or("window must be an entity ID string")?
            .parse::<u64>()
            .map_err(|_| "Invalid window entity ID")?;
        Entity::try_from_bits(bits).ok_or("Invalid window entity ID")?
    } else {
        world
            .query_filtered::<Entity, (With<Window>, With<PrimaryWindow>)>()
            .single(world)
            .map_err(|_| "Select a unique primary window or supply window")?
    };
    if world.get::<Window>(window).is_none() {
        return Err("Not a live Window entity".into());
    }
    let mut messages = world
        .get_resource_mut::<Messages<FileDragAndDrop>>()
        .ok_or("File drop input requires Bevy's WindowPlugin")?;
    let count = paths.len();
    messages.write_batch(
        paths
            .into_iter()
            .map(|path_buf| FileDragAndDrop::DroppedFile { window, path_buf }),
    );
    Ok(
        serde_json::json!({"window": window.to_bits().to_string(), "injected_events": count,
        "boundary": "Bevy FileDragAndDrop::DroppedFile (injected event, not OS drag)"}),
    )
}

#[cfg(feature = "automation")]
fn inspect(world: &mut World, _: serde_json::Value) -> bedtime_automation2::AutomationResult {
    use serde_json::json;
    let rect = |rect: egui::Rect| json!({"min": [rect.min.x, rect.min.y], "max": [rect.max.x, rect.max.y]});
    let events = world.resource::<FileEvents>();
    let counts: BTreeMap<_, _> = events
        .activations
        .iter()
        .map(|(panel, counts)| {
            (
                *panel,
                json!({
                    "SingleClick": counts.single, "DoubleClick": counts.double,
                    "Keyboard": counts.keyboard, "ContextMenu": counts.context,
                }),
            )
        })
        .collect();
    let errors = events.errors.clone();
    let trees: Vec<_> = world.query::<(Entity, &FileTreePanel, &InWorkspace, &PanelPresentation)>()
        .iter(world).map(|(entity, tree, owner, presentation)| {
            let snapshot = tree.snapshot();
            let settings = world.get::<FileTreeSettings>(entity).copied().unwrap_or_default();
            json!({
                "entity": entity.to_bits().to_string(), "workspace": owner.0.to_bits().to_string(),
                "layout": world.get::<InArea>(entity).and_then(|area| world.get::<InLayout>(area.0))
                    .map(|layout| layout.0.to_bits().to_string()),
                "root": snapshot.root.to_string_lossy(), "generation": snapshot.generation,
                "revision": snapshot.revision, "version": snapshot.version,
                "loading": snapshot.loading, "busy": tree.is_busy(), "animating": tree.is_animating(), "error": tree.error(),
                "snapshot_error": snapshot.error, "watcher_error": snapshot.watcher_error,
                "cached_directories": snapshot.cached_directories,
                "settings": {"read_only": settings.read_only, "indent_size": settings.indent_size},
                "selection": tree.selection().iter().map(|path| path.to_string_lossy()).collect::<Vec<_>>(),
                "rows": snapshot.rows.iter().map(|row| json!({
                    "path": row.path.to_string_lossy(), "label": row.label.as_ref(),
                    "kind": format!("{:?}", row.kind), "depth": row.depth, "hidden": row.hidden,
                    "directory_state": format!("{:?}", row.directory_state),
                })).collect::<Vec<_>>(),
                "row_rects": tree.row_rects().iter().map(|(path, bounds)| json!({
                    "path": path.to_string_lossy(), "rect": rect(*bounds),
                })).collect::<Vec<_>>(),
                "activations": counts.get(&entity).cloned().unwrap_or_else(|| json!({
                    "SingleClick": 0, "DoubleClick": 0, "Keyboard": 0, "ContextMenu": 0,
                })),
                "open_error": errors.get(&entity),
                "presentation": {"visible": presentation.visible, "focused": presentation.focused,
                    "rect": {"min": presentation.rect.min.to_array(), "max": presentation.rect.max.to_array()},
                    "tab_rect": {"min": presentation.tab_rect.min.to_array(), "max": presentation.tab_rect.max.to_array()}}, 
            })
        }).collect();
    let previews: Vec<_> = world.query::<(Entity, &FilePreviewPanel, &PanelInstance, &InWorkspace)>()
        .iter(world).map(|(entity, preview, instance, owner)| {
            let document = world.get::<DocumentView>(entity).map(DocumentView::document);
            let canonical = document.and_then(|id| cast_document::<TextBuffer>(world, id).ok()).and_then(|handle| read_document(world, handle).ok());
            let text = canonical.or_else(|| match &preview.content { Some(PreviewContent::Text(text)) => Some(text), _ => None });
            let (kind, content) = if let Some(text) = text {
                ("text", Some(text.range_text(0..text.clamp_boundary(text.len().min(16*1024)))))
            } else {
                match &preview.content {
                    Some(PreviewContent::Metadata(text)) => ("metadata",Some(text.to_string())),
                    _ => ("empty",None),
                }
            };
            json!({"entity": entity.to_bits().to_string(), "workspace": owner.0.to_bits().to_string(),
                "area": world.get::<InArea>(entity).map(|area| area.0.to_bits().to_string()),
                "layout": world.get::<InArea>(entity).and_then(|area| world.get::<InLayout>(area.0))
                    .map(|layout| layout.0.to_bits().to_string()),
                "path": preview.path.as_ref().map(|path| path.to_string_lossy()),
                "generation": preview.generation, "loading": preview.load.is_some(),
                "pinned": !instance.preview, "title": instance.title, "kind": kind,
                "content": content, "byte_len": text.map(TextBuffer::len), "line_count": text.map(TextBuffer::line_count), "content_truncated": text.is_some_and(|text| text.len() > 16*1024), "rendered_lines": preview.rendered_lines, "laid_out_bytes": preview.laid_out_bytes, "error": preview.error, "missing": preview.missing,
                "unloaded": preview.path.is_none() && document.is_none(),
                "document": document.map(|id| id.entity().to_bits().to_string()),
                "document_info": document.and_then(|id| inspect_document(world, id).ok()).map(|info| json!({"revision": info.revision, "dirty": info.dirty, "saving": info.saving, "conflict": info.conflict, "draft": info.has_drafts, "save_binding": info.has_save_binding, "source_generation": info.source_generation, "save_error": info.last_save_error.map(|e| e.to_string())})),
                "editor_rect": preview.editor_rect.map(|rect| json!({"min": [rect.min.x,rect.min.y], "max": [rect.max.x,rect.max.y]})),
                "caret_rect": preview.caret_rect.map(|rect| json!({"min": [rect.min.x,rect.min.y], "max": [rect.max.x,rect.max.y]})),
                "editor": world.get::<bedtime_editor2::workbench::WorkbenchEditor>(entity).map(|editor| json!({"anchor": editor.view.selection().anchor(), "head": editor.view.selection().head(), "scroll": [editor.view.scroll_offset().x,editor.view.scroll_offset().y], "line_numbers": editor.view.settings.line_numbers, "wrap": editor.view.settings.wrap, "wrapping_ready": editor.view.wrapping_ready(), "search_query": editor.view.search_query().map(|query| query.text.as_str()), "search_matches": editor.view.search_match_count()})),
            })
        }).collect();
    let storage = crate::session::inspect_storage(world);
    let preferences: Vec<_> = world.query::<(Entity, &FileTreePreferences)>().iter(world)
        .map(|(entity, preferences)| json!({
            "workspace": entity.to_bits().to_string(),
            "roots": preferences.roots.iter().map(|(root, visibility)| json!({
                "root": root.to_string_lossy(), "hide_hidden": visibility.hide_hidden,
                "hidden_paths": visibility.hidden_paths.iter().map(|path| path.to_string_lossy()).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
        })).collect();
    let clipboard = world.get_resource::<Clipboard>().map(|clipboard| {
        json!({
            "mode": format!("{:?}", clipboard.mode()), "revision": clipboard.revision(),
            "initialization_error": clipboard.initialization_error(),
        })
    });
    let roots: Vec<_> = world.query::<(Entity, &WorkspaceRoot, Option<&FolderWorkspace>)>().iter(world)
        .map(|(workspace, root, folder)| json!({ "workspace": workspace.to_bits().to_string(), "root": root.0.to_string_lossy(), "persistent": folder.is_some(), "session_ready": crate::session::ready(world, workspace), "settings_ready": crate::storage::ready(world, workspace) })).collect();
    let startup = world.resource::<StartupRoot>();
    Ok(
        json!({"panels": trees, "previews": previews, "storage": storage, "clipboard": clipboard,
        "preferences": preferences, "workspace_roots": roots, "startup_root": startup.root.as_ref().map(|path| path.to_string_lossy()),
        "startup_error": startup.error}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "bed2-files-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&root).unwrap();
            Self(fs::canonicalize(root).unwrap())
        }
        fn path(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
        fn text(&self, name: &str, text: &str) -> PathBuf {
            let path = self.path(name);
            fs::write(&path, text).unwrap();
            path
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn host() -> App {
        let mut app = App::new();
        app.add_plugins((WorkbenchCorePlugin, FilesPlugin));
        register_document_handlers(app.world_mut()).unwrap();
        register_document_shortcuts(app.world_mut());
        app
    }
    #[test]
    fn restarting_with_last_workspace_restores_its_layouts_and_active_slot() {
        let fixture = Fixture::new();
        let config = fixture.path("config");
        let boot = |argument: Option<PathBuf>| {
            let mut app = host();
            app.add_plugins((
                crate::settings::SettingsPlugin,
                crate::session::SessionPlugin,
                crate::layouts::LayoutsPlugin,
            ));
            crate::storage::configure_directory(&mut app, config.clone());
            let (owner, _) = workspace(&mut app, "Default");
            crate::session::remember_initial_state(app.world_mut(), owner).unwrap();
            app.insert_resource(StartupRoot {
                persistent: argument.is_some(),
                requested: Some(argument),
                ..default()
            });
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                app.update();
                if app.world().get::<WorkspaceRoot>(owner).is_some()
                    && crate::storage::ready(app.world(), owner)
                    && app
                        .world()
                        .get::<WorkspaceMembers>(owner)
                        .unwrap()
                        .entities()
                        .iter()
                        .any(|&e| app.world().get::<crate::layouts::LayoutSlot>(e).is_some())
                {
                    break;
                }
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(1));
            }
            (app, owner)
        };
        let (mut app, owner) = boot(Some(fixture.0.clone()));
        app.world_mut()
            .resource_mut::<crate::settings::StartupPreferences>()
            .open_last_workspace = true;
        crate::layouts::switch(app.world_mut(), owner, crate::layouts::LayoutSlot::Two).unwrap();
        let layout = Workbench::for_entity(app.world_mut(), owner)
            .unwrap()
            .active_layout();
        let area = Workbench::for_entity(app.world_mut(), layout)
            .unwrap()
            .destination_area(layout)
            .unwrap();
        crate::layouts::spawn_tool(app.world_mut(), area, crate::layouts::ToolKind::Settings);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            app.update();
            let global = crate::storage::flush(app.world_mut()).unwrap();
            let session = crate::session::flush(app.world_mut()).unwrap();
            if global && session {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        drop(app);
        let (mut app, owner) = boot(None);
        assert_eq!(
            app.world().get::<WorkspaceRoot>(owner).unwrap().0,
            fixture.0
        );
        assert!(app.world().get::<FolderWorkspace>(owner).is_some());
        let layout = Workbench::for_entity(app.world_mut(), owner)
            .unwrap()
            .active_layout();
        assert_eq!(
            app.world().get::<crate::layouts::LayoutSlot>(layout),
            Some(&crate::layouts::LayoutSlot::Two)
        );
        assert_eq!(
            app.world_mut()
                .query::<&crate::settings::SettingsPanel>()
                .iter(app.world())
                .count(),
            1
        );
        assert_eq!(
            app.world_mut()
                .query::<&bedtime_terminal2::TerminalPanel>()
                .iter(app.world())
                .count(),
            1
        );
        assert_eq!(
            app.world_mut().query::<&Layout>().iter(app.world()).count(),
            2
        );
    }

    #[test]
    fn startup_policy_defaults_to_home_restores_opt_in_and_honors_explicit_folder() {
        let fixture = Fixture::new();
        for (enabled, explicit, missing) in [
            (false, false, false),
            (true, false, false),
            (true, true, false),
            (true, false, true),
        ] {
            let mut app = host();
            app.add_plugins((
                crate::settings::SettingsPlugin,
                crate::layouts::LayoutsPlugin,
            ));
            let (owner, _) = workspace(&mut app, "Default");
            let last = if missing {
                fixture.path("missing")
            } else {
                fixture.0.clone()
            };
            *app.world_mut()
                .resource_mut::<crate::settings::StartupPreferences>() =
                crate::settings::StartupPreferences {
                    open_last_workspace: enabled,
                    last_workspace: Some(last),
                };
            app.insert_resource(StartupRoot {
                requested: Some(explicit.then(|| fixture.0.clone())),
                persistent: explicit,
                ..default()
            });
            let deadline = Instant::now() + Duration::from_secs(5);
            while app.world().get::<WorkspaceRoot>(owner).is_none() {
                app.update();
                assert!(Instant::now() < deadline, "startup did not resolve");
                std::thread::sleep(Duration::from_millis(1));
            }
            app.update();
            let persistent = explicit || (enabled && !missing);
            let expected = if persistent {
                fixture.0.clone()
            } else {
                resolve_root(None).unwrap()
            };
            assert_eq!(app.world().get::<WorkspaceRoot>(owner).unwrap().0, expected);
            assert_eq!(
                app.world().get::<FolderWorkspace>(owner).is_some(),
                persistent
            );
            let layouts = app
                .world()
                .get::<WorkspaceMembers>(owner)
                .unwrap()
                .entities()
                .iter()
                .filter(|&&e| app.world().get::<crate::layouts::LayoutSlot>(e).is_some())
                .count();
            assert_eq!(layouts, 1);
            assert_eq!(
                app.world_mut()
                    .query::<&bedtime_terminal2::TerminalPanel>()
                    .iter(app.world())
                    .count(),
                1
            );
            assert_eq!(
                app.world_mut()
                    .query::<&FileTreePanel>()
                    .iter(app.world())
                    .count(),
                0
            );
        }
    }

    fn workspace(app: &mut App, name: &str) -> (Entity, Entity) {
        let workspace = Workspaces::new(app.world_mut()).open(name).entity();
        let layout = Workbench::for_entity(app.world_mut(), workspace)
            .unwrap()
            .active_layout();
        (workspace, layout)
    }
    fn preview_panel(world: &mut World, layout: Entity) -> Entity {
        let area = Workbench::for_entity(world, layout)
            .unwrap()
            .destination_area(layout)
            .unwrap();
        let panel = world
            .spawn((FilePreviewPanel::default(), InArea(area)))
            .id();
        Workbench::for_entity(world, panel)
            .unwrap()
            .focus_panel(panel)
            .unwrap();
        panel
    }
    fn tree(app: &mut App, layout: Entity, root: &Path) -> Entity {
        let area = Workbench::for_entity(app.world_mut(), layout)
            .unwrap()
            .destination_area(layout)
            .unwrap();
        let panel = app
            .world_mut()
            .spawn((FileTreePanel::default(), InArea(area)))
            .id();
        Workbench::for_entity(app.world_mut(), panel)
            .unwrap()
            .focus_panel(panel)
            .unwrap();
        app.world_mut()
            .entity_mut(panel)
            .insert(FileTreeRoot(root.to_owned()));
        panel
    }
    fn request(app: &mut App, panel: Entity, path: &Path, activation: FileTreeActivation) {
        let workspace = app.world().get::<InWorkspace>(panel).unwrap().0;
        app.world_mut().write_message(FileTreeOpenRequested {
            panel,
            workspace,
            path: path.to_owned(),
            activation,
        });
        app.update();
    }
    fn wait(app: &mut App, predicate: impl Fn(&mut World) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            app.update();
            if predicate(app.world_mut()) {
                return;
            }
            assert!(Instant::now() < deadline, "Files test did not settle");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    fn stop(app: &mut App) {
        admit_preferences(app.world_mut());
        Workspaces::new(app.world_mut()).shutdown_work().unwrap();
        wait(app, |world| Workspaces::new(world).work_finished());
    }
    fn viewers(app: &mut App, workspace: Entity) -> Vec<Entity> {
        app.world_mut()
            .query::<(Entity, &InWorkspace, &FilePreviewPanel)>()
            .iter(app.world())
            .filter_map(|(panel, owner, _)| (owner.0 == workspace).then_some(panel))
            .collect()
    }
    fn text(app: &App, panel: Entity) -> &TextBuffer {
        if let Some(view) = app.world().get::<DocumentView>(panel) {
            let handle = cast_document::<TextBuffer>(app.world(), view.document()).unwrap();
            return read_document(app.world(), handle).unwrap();
        }
        match &app.world().get::<FilePreviewPanel>(panel).unwrap().content {
            Some(PreviewContent::Text(text)) => text,
            content => panic!("Expected text, got {content:?}"),
        }
    }

    fn edit_text(app: &mut App, panel: Entity, replacement: &str) -> DocumentHandle<TextBuffer> {
        let id = app.world().get::<DocumentView>(panel).unwrap().document();
        let handle = cast_document::<TextBuffer>(app.world(), id).unwrap();
        let revision = inspect_document(app.world(), id).unwrap().revision;
        let len = read_document(app.world(), handle).unwrap().len();
        edit_document(
            app.world_mut(),
            handle,
            revision,
            BufferEdit::new(0..len, replacement),
            EditOptions::new("Test edit"),
        )
        .unwrap();
        handle
    }

    #[test]
    fn terminal_creation_waits_for_the_workspace_root_instead_of_using_launch_cwd() {
        let fixture = Fixture::new();
        let mut app = host();
        let (workspace, layout) = workspace(&mut app, "Default");
        app.world_mut().resource_mut::<StartupRoot>().requested = Some(None);
        assert!(!root_ready(app.world(), workspace));
        assert!(!crate::storage::ready(app.world(), workspace));
        assert!(
            crate::menus::open_panel(
                app.world_mut(),
                layout,
                bedtime_terminal2::TerminalPanel::KEY
            )
            .is_err()
        );
        assert_eq!(
            app.world_mut()
                .query::<&bedtime_terminal2::TerminalPanel>()
                .iter(app.world())
                .count(),
            0
        );
        app.world_mut()
            .entity_mut(workspace)
            .insert(WorkspaceRoot(fixture.0.clone()));
        app.world_mut().resource_mut::<StartupRoot>().requested = None;
        let panel = crate::menus::open_panel(
            app.world_mut(),
            layout,
            bedtime_terminal2::TerminalPanel::KEY,
        )
        .unwrap();
        assert_eq!(
            app.world()
                .get::<bedtime_terminal2::TerminalPanel>(panel)
                .unwrap()
                .options
                .working_directory
                .as_ref(),
            Some(&fixture.0)
        );
    }

    #[test]
    fn default_root_is_home_not_the_launch_directory() {
        assert_eq!(
            resolve_root(None).unwrap(),
            fs::canonicalize(home_directory().unwrap()).unwrap()
        );
        let fixture = Fixture::new();
        assert_eq!(resolve_root(Some(fixture.0.clone())).unwrap(), fixture.0);
        assert!(resolve_root(Some(fixture.path("missing"))).is_err());
    }

    #[test]
    fn default_workspace_visibility_changes_never_create_project_storage() {
        let fixture = Fixture::new();
        let mut app = host();
        let (workspace, layout) = workspace(&mut app, "Default");
        app.world_mut().entity_mut(workspace).insert((
            WorkspaceRoot(fixture.0.clone()),
            FileTreePreferences::default(),
        ));
        let panel = tree(&mut app, layout, &fixture.0);
        app.update();
        app.world_mut()
            .get_mut::<FileTreePreferences>(workspace)
            .unwrap()
            .roots
            .insert(fixture.0.clone(), FileTreeVisibility::default());
        app.world_mut()
            .write_message(FileTreePreferencesChanged { workspace });
        app.update();
        assert!(flush_preferences(app.world_mut()).unwrap());
        assert!(app.world().get::<FileTreePanel>(panel).is_some());
        assert!(!fixture.path(".bed").exists());
    }

    #[test]
    fn explicit_open_folder_creates_or_reuses_a_folder_workspace_without_promoting_default() {
        let fixture = Fixture::new();
        let mut app = host();
        let (default_workspace, _) = workspace(&mut app, "Default");
        app.world_mut()
            .entity_mut(default_workspace)
            .insert(WorkspaceRoot(home_directory().unwrap()));
        open_folder(app.world_mut(), fixture.0.clone()).unwrap();
        wait(&mut app, |world| {
            world
                .query_filtered::<Entity, With<FolderWorkspace>>()
                .iter(world)
                .count()
                == 1
        });
        let folder = app
            .world_mut()
            .query_filtered::<Entity, With<FolderWorkspace>>()
            .single(app.world())
            .unwrap();
        assert_ne!(folder, default_workspace);
        assert_eq!(
            app.world().get::<WorkspaceRoot>(folder).unwrap().0,
            fixture.0
        );
        assert!(
            app.world()
                .get::<FolderWorkspace>(default_workspace)
                .is_none()
        );
        let reopened = open_folder(app.world_mut(), fixture.0.clone()).unwrap();
        wait(&mut app, |_| reopened.lock().unwrap().try_recv().is_ok());
        assert_eq!(
            app.world_mut()
                .query_filtered::<Entity, With<Workspace>>()
                .iter(app.world())
                .count(),
            2
        );
        assert_eq!(Workspaces::new(app.world_mut()).active().entity(), folder);
        assert!(
            !fixture.path(".bed").exists(),
            "opening alone is not a persisted mutation"
        );
        Workspaces::new(app.world_mut()).shutdown_work().unwrap();
        wait(&mut app, |world| Workspaces::new(world).work_finished());
    }

    #[test]
    fn project_identity_configures_only_its_own_workspace() {
        let first_root = Fixture::new();
        let second_root = Fixture::new();
        let mut app = host();
        let (first, first_layout) = workspace(&mut app, "First");
        let (second, second_layout) = workspace(&mut app, "Second");
        app.world_mut()
            .entity_mut(first)
            .insert((WorkspaceRoot(first_root.0.clone()), FolderWorkspace));
        app.world_mut()
            .entity_mut(second)
            .insert((WorkspaceRoot(second_root.0.clone()), FolderWorkspace));
        open_files(app.world_mut(), first_layout);
        open_files(app.world_mut(), second_layout);
        app.update();
        for (panel, owner) in app
            .world_mut()
            .query_filtered::<(Entity, &InWorkspace), With<FileTreePanel>>()
            .iter(app.world())
        {
            let expected = if owner.0 == first {
                &first_root.0
            } else {
                &second_root.0
            };
            assert_eq!(&app.world().get::<FileTreeRoot>(panel).unwrap().0, expected);
        }
        assert_eq!(
            app.world().get::<WorkspaceRoot>(first).unwrap().0,
            first_root.0
        );
        assert_eq!(
            app.world().get::<WorkspaceRoot>(second).unwrap().0,
            second_root.0
        );
        stop(&mut app);
    }

    #[test]
    fn selecting_a_preview_does_not_change_area_focus() {
        let fixture = Fixture::new();
        let path = fixture.text("preview.txt", "preview");
        let mut app = host();
        let (workspace, layout) = workspace(&mut app, "Project");
        let tree = tree(&mut app, layout, &fixture.0);
        request(&mut app, tree, &path, FileTreeActivation::SingleClick);
        let preview = viewers(&mut app, workspace)[0];
        let area = app.world().get::<InArea>(preview).unwrap().0;
        assert_eq!(
            app.world().get::<Area>(area).unwrap().selected_panel(),
            Some(preview)
        );
        assert_eq!(
            Workbench::for_entity(app.world_mut(), layout)
                .unwrap()
                .focused_panel(layout),
            Some(tree)
        );
        request(&mut app, tree, &path, FileTreeActivation::Keyboard);
        assert_eq!(
            Workbench::for_entity(app.world_mut(), layout)
                .unwrap()
                .focused_panel(layout),
            Some(preview)
        );
        stop(&mut app);
    }

    #[test]
    fn single_click_replaces_the_structure_source_without_focusing_the_editor() {
        let fixture = Fixture::new();
        let first = fixture.text("first.rs", "fn first() {}\n");
        let second = fixture.text("second.rs", "fn second() {}\n");
        let mut app = host();
        app.add_plugins(crate::code::CodePlugin);
        let (workspace, layout) = workspace(&mut app, "Preview structure");
        let tree = tree(&mut app, layout, &fixture.0);
        let mut previous = None;
        for (path, label) in [(&first, "first"), (&second, "second"), (&first, "first")] {
            request(&mut app, tree, path, FileTreeActivation::SingleClick);
            wait(&mut app, |world| {
                world
                    .get::<bed_structure2::StructureSource>(workspace)
                    .and_then(|state| state.source)
                    .and_then(|source| {
                        bedtime_editor2::workbench::analysis_for_panel(world, source.panel)
                    })
                    .is_some_and(|observation| {
                        observation
                            .outline
                            .first()
                            .is_some_and(|item| item.label == label)
                    })
            });
            let source = app
                .world()
                .get::<bed_structure2::StructureSource>(workspace)
                .unwrap()
                .source
                .unwrap();
            if let Some(document) = previous {
                assert_ne!(source.document, document);
            }
            previous = Some(source.document);
            assert_eq!(
                Workbench::for_entity(app.world_mut(), layout)
                    .unwrap()
                    .focused_panel(layout),
                Some(tree)
            );
        }
        stop(&mut app);
    }

    #[test]
    fn opened_files_supply_language_analysis_structure_and_shared_views() {
        use bedtime_editor2::workbench::{CodeLanguage, analysis_for_panel};
        let fixture = Fixture::new();
        let path = fixture.text("cats.rs", "fn cat() {\n let age=42;\n}\n");
        let mut app = host();
        app.add_plugins(crate::code::CodePlugin);
        let (workspace, layout) = workspace(&mut app, "Code");
        let tree = tree(&mut app, layout, &fixture.0);
        request(&mut app, tree, &path, FileTreeActivation::SingleClick);
        wait(&mut app, |world| {
            world
                .query_filtered::<Entity, With<FilePreviewPanel>>()
                .iter(world)
                .any(|panel| analysis_for_panel(world, panel).is_some())
        });
        let panel = viewers(&mut app, workspace)[0];
        let document = app.world().get::<DocumentView>(panel).unwrap().document();
        assert_eq!(
            app.world()
                .get::<CodeLanguage>(document.entity())
                .unwrap()
                .0,
            "rust"
        );
        let observation = analysis_for_panel(app.world(), panel).unwrap();
        assert_eq!(observation.outline[0].label, "cat");
        assert_eq!(
            app.world()
                .get::<bed_structure2::StructureSource>(workspace)
                .unwrap()
                .source
                .unwrap()
                .panel,
            panel
        );
        let other = split_editor(app.world_mut(), panel).unwrap();
        app.update();
        assert!(Arc::ptr_eq(
            &observation,
            &analysis_for_panel(app.world(), other).unwrap()
        ));
        app.world_mut()
            .entity_mut(document.entity())
            .insert(CodeLanguage("python".into()));
        load_preview(app.world_mut(), panel, path, true).unwrap();
        wait(&mut app, |world| {
            world.get::<FilePreviewPanel>(panel).unwrap().load.is_none()
        });
        assert_eq!(
            app.world()
                .get::<CodeLanguage>(document.entity())
                .unwrap()
                .0,
            "python"
        );
        let empty = preview_panel(app.world_mut(), layout);
        app.update();
        assert!(
            app.world().get::<DocumentView>(empty).is_none(),
            "An empty Code panel must not create a sample document"
        );
    }

    #[test]
    fn editable_files_share_history_across_split_views_and_save_real_bytes() {
        let fixture = Fixture::new();
        let path = fixture.text("edit.txt", "original\r\nUnicode é🙂\r\n");
        let mut app = host();
        let (workspace, layout) = workspace(&mut app, "Editor");
        let tree = tree(&mut app, layout, &fixture.0);
        request(&mut app, tree, &path, FileTreeActivation::SingleClick);
        wait(&mut app, |world| {
            world
                .query::<(&FilePreviewPanel, &DocumentView)>()
                .iter(world)
                .any(|(state, _)| state.path.as_ref() == Some(&path))
        });
        let panel = viewers(&mut app, workspace)[0];
        let other = split_editor(app.world_mut(), panel).unwrap();
        let handle = edit_text(&mut app, panel, "edited\r\nUnicode é🙂\r\n");
        assert_eq!(
            app.world().get::<DocumentView>(other).unwrap().document(),
            handle.id()
        );
        assert_eq!(text(&app, other), "edited\r\nUnicode é🙂\r\n");
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "original\r\nUnicode é🙂\r\n"
        );
        undo_document(app.world_mut(), handle.id()).unwrap();
        assert_eq!(text(&app, other), "original\r\nUnicode é🙂\r\n");
        redo_document(app.world_mut(), handle.id()).unwrap();
        save_document(app.world_mut(), handle.id()).unwrap();
        wait(&mut app, |world| {
            !inspect_document(world, handle.id()).unwrap().saving
        });
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "edited\r\nUnicode é🙂\r\n"
        );
        assert!(!inspect_document(app.world(), handle.id()).unwrap().dirty);
        stop(&mut app);
    }

    #[test]
    fn opening_another_file_never_replaces_a_dirty_preview() {
        let fixture = Fixture::new();
        let first = fixture.text("first.txt", "first");
        let next = fixture.text("next.txt", "next");
        let mut app = host();
        let (workspace, layout) = workspace(&mut app, "Editor");
        let tree = tree(&mut app, layout, &fixture.0);
        request(&mut app, tree, &first, FileTreeActivation::SingleClick);
        wait(&mut app, |world| {
            world.query::<&DocumentView>().iter(world).next().is_some()
        });
        let panel = viewers(&mut app, workspace)[0];
        edit_text(&mut app, panel, "unsaved");
        request(&mut app, tree, &next, FileTreeActivation::SingleClick);
        wait(&mut app, |world| {
            world
                .query::<(&FilePreviewPanel, &DocumentView)>()
                .iter(world)
                .any(|(state, _)| state.path.as_ref() == Some(&next))
        });
        assert_eq!(text(&app, panel), "unsaved");
        assert_eq!(
            app.world()
                .get::<FilePreviewPanel>(panel)
                .unwrap()
                .path
                .as_ref(),
            Some(&first)
        );
        assert_eq!(viewers(&mut app, workspace).len(), 2);
        stop(&mut app);
    }

    #[test]
    fn reopening_a_retained_dirty_file_through_files_preserves_edits_and_history() {
        let fixture = Fixture::new();
        let path = fixture.text("retained.rs", "fn base() {}\n");
        let mut app = host();
        let (workspace, layout) = workspace(&mut app, "Retained");
        let tree = tree(&mut app, layout, &fixture.0);
        request(&mut app, tree, &path, FileTreeActivation::Keyboard);
        wait(&mut app, |world| {
            world.query::<&DocumentView>().iter(world).next().is_some()
        });
        let panel = viewers(&mut app, workspace)[0];
        let handle = edit_text(&mut app, panel, "fn edited() {}\n");
        Workbench::for_entity(app.world_mut(), panel)
            .unwrap()
            .close_panel(panel)
            .unwrap();
        request(&mut app, tree, &path, FileTreeActivation::Keyboard);
        wait(&mut app, |world| {
            world
                .query::<&DocumentView>()
                .iter(world)
                .any(|v| v.document() == handle.id())
        });
        let other = viewers(&mut app, workspace)[0];
        assert_eq!(text(&app, other), "fn edited() {}\n");
        let info = inspect_document(app.world(), handle.id()).unwrap();
        assert!(info.dirty && info.can_undo && !info.conflict);
        undo_document(app.world_mut(), handle.id()).unwrap();
        assert_eq!(text(&app, other), "fn base() {}\n");
        stop(&mut app);
    }

    #[test]
    fn changed_disk_bytes_reject_save_and_reload_preserves_edits_for_resolution() {
        let fixture = Fixture::new();
        let path = fixture.text("conflict.txt", "base");
        let mut app = host();
        let (workspace, layout) = workspace(&mut app, "Editor");
        let tree = tree(&mut app, layout, &fixture.0);
        request(&mut app, tree, &path, FileTreeActivation::SingleClick);
        wait(&mut app, |world| {
            world.query::<&DocumentView>().iter(world).next().is_some()
        });
        let panel = viewers(&mut app, workspace)[0];
        let handle = edit_text(&mut app, panel, "mine");
        fs::write(&path, "theirs").unwrap();
        save_document(app.world_mut(), handle.id()).unwrap();
        wait(&mut app, |world| {
            !inspect_document(world, handle.id()).unwrap().saving
        });
        assert_eq!(fs::read_to_string(&path).unwrap(), "theirs");
        assert_eq!(text(&app, panel), "mine");
        assert!(
            inspect_document(app.world(), handle.id())
                .unwrap()
                .requires_source_refresh
        );
        load_preview(app.world_mut(), panel, path.clone(), true).unwrap();
        wait(&mut app, |world| {
            world.get::<FilePreviewPanel>(panel).unwrap().load.is_none()
        });
        let conflict = document_conflict(app.world(), handle).unwrap().unwrap();
        assert_eq!(conflict.local, "mine");
        assert_eq!(conflict.incoming, "theirs");
        resolve_document_conflict(
            app.world_mut(),
            handle,
            conflict.revision,
            conflict.source_generation,
            DocumentConflictResolution::KeepLocal,
        )
        .unwrap();
        save_document(app.world_mut(), handle.id()).unwrap();
        wait(&mut app, |world| {
            !inspect_document(world, handle.id()).unwrap().saving
        });
        assert_eq!(fs::read_to_string(&path).unwrap(), "mine");
        stop(&mut app);
    }

    #[test]
    fn confirmed_rename_keeps_unsaved_text_and_history_and_saves_to_new_path() {
        let fixture = Fixture::new();
        let path = fixture.text("old.txt", "base");
        let renamed = fixture.path("new.txt");
        let mut app = host();
        let (workspace, layout) = workspace(&mut app, "Editor");
        let tree = tree(&mut app, layout, &fixture.0);
        request(&mut app, tree, &path, FileTreeActivation::SingleClick);
        wait(&mut app, |world| {
            world.query::<&DocumentView>().iter(world).next().is_some()
        });
        let panel = viewers(&mut app, workspace)[0];
        let handle = edit_text(&mut app, panel, "mine");
        fs::rename(&path, &renamed).unwrap();
        move_previews(app.world_mut(), &path, &renamed);
        assert_eq!(text(&app, panel), "mine");
        assert_eq!(
            app.world().get::<DocumentView>(panel).unwrap().document(),
            handle.id()
        );
        undo_document(app.world_mut(), handle.id()).unwrap();
        assert_eq!(text(&app, panel), "base");
        redo_document(app.world_mut(), handle.id()).unwrap();
        save_document(app.world_mut(), handle.id()).unwrap();
        wait(&mut app, |world| {
            !inspect_document(world, handle.id()).unwrap().saving
        });
        assert!(
            inspect_document(app.world(), handle.id())
                .unwrap()
                .last_save_error
                .is_none()
        );
        assert!(!path.exists());
        assert_eq!(fs::read_to_string(renamed).unwrap(), "mine");
        stop(&mut app);
    }

    #[test]
    fn deleting_a_dirty_source_retains_edits_and_removes_save_binding() {
        let fixture = Fixture::new();
        let path = fixture.text("deleted.txt", "base");
        let mut app = host();
        let (workspace, layout) = workspace(&mut app, "Editor");
        let tree = tree(&mut app, layout, &fixture.0);
        request(&mut app, tree, &path, FileTreeActivation::SingleClick);
        wait(&mut app, |world| {
            world.query::<&DocumentView>().iter(world).next().is_some()
        });
        let panel = viewers(&mut app, workspace)[0];
        let handle = edit_text(&mut app, panel, "mine");
        fs::remove_file(&path).unwrap();
        unload_deleted_preview(app.world_mut(), panel);
        assert_eq!(text(&app, panel), "mine");
        let info = inspect_document(app.world(), handle.id()).unwrap();
        assert!(info.dirty);
        assert!(!info.has_save_binding);
        assert_eq!(
            save_document(app.world_mut(), handle.id()),
            Err(DocumentError::NoSaveBinding)
        );
        assert!(
            preflight_workspace_close(app.world(), workspace, WorkspaceCloseMode::Saved).is_err()
        );
        Workbench::for_entity(app.world_mut(), panel)
            .unwrap()
            .close_panel(panel)
            .unwrap();
        assert_eq!(read_document(app.world(), handle).unwrap(), "mine");
        assert!(inspect_document(app.world(), handle.id()).unwrap().dirty);
        stop(&mut app);
    }

    fn operation(
        app: &mut App,
        workspace: Entity,
        panel: Entity,
        root: &Path,
        change: FileTreeChange,
    ) {
        app.world_mut().write_message(FileTreeOperationFinished {
            workspace,
            panel,
            root: root.to_owned(),
            generation: 1,
            result: Arc::new(bedtime_filetree2::FileTreeOperationResult {
                id: 1,
                completed: vec![change],
                errors: Vec::new(),
                cancelled: false,
            }),
        });
        app.update();
    }

    #[cfg(feature = "automation")]
    #[test]
    fn injected_drop_validates_entire_request_before_writing_window_messages() {
        use bevy::window::{FileDragAndDrop, PrimaryWindow};
        use serde_json::json;
        let fixture = Fixture::new();
        let first = fixture.path("one.txt");
        let second = fixture.path("folder");
        let mut app = App::new();
        app.add_message::<FileDragAndDrop>();
        let window = app
            .world_mut()
            .spawn((Window::default(), PrimaryWindow))
            .id();
        for payload in [
            json!({"paths": [first, "relative.txt"]}),
            json!({"paths": [first, 42]}),
            json!({"paths": [first, format!("{}\0bad", first.display())]}),
            json!({"paths": []}),
            json!({"paths": vec![first.clone(); 33]}),
            json!({"paths": [first, first]}),
            json!({"paths": [first], "action": "copy"}),
            json!({"paths": [first], "window": "not-an-entity"}),
            json!({"paths": [first], "window": Entity::PLACEHOLDER.to_bits().to_string()}),
        ] {
            assert!(drop_files(app.world_mut(), payload).is_err());
        }
        {
            let messages = app.world().resource::<Messages<FileDragAndDrop>>();
            assert_eq!(messages.get_cursor().read(messages).count(), 0);
        }
        let result = drop_files(app.world_mut(), json!({"paths": [first, second]})).unwrap();
        assert_eq!(result["injected_events"], 2);
        assert_eq!(result["window"], window.to_bits().to_string());
        let messages = app.world().resource::<Messages<FileDragAndDrop>>();
        let delivered: Vec<_> = messages
            .get_cursor()
            .read(messages)
            .map(|message| match message {
                FileDragAndDrop::DroppedFile {
                    window: target,
                    path_buf,
                } => {
                    assert_eq!(*target, window);
                    path_buf.clone()
                }
                _ => panic!("Only dropped-file messages should be injected"),
            })
            .collect();
        assert_eq!(delivered, vec![first, second]);
        assert!(
            !fixture.path("one.txt").exists(),
            "The command must not perform filesystem actions"
        );
    }

    #[cfg(feature = "automation")]
    #[test]
    fn settings_automation_validates_atomically_and_only_updates_target_panel() {
        use serde_json::json;
        let fixture = Fixture::new();
        let mut app = host();
        let (_, layout) = workspace(&mut app, "Project");
        let first = tree(&mut app, layout, &fixture.0);
        let second = tree(&mut app, layout, &fixture.0);
        let id = first.to_bits().to_string();
        for payload in [
            json!([]),
            json!({"panel": id}),
            json!({"panel": id, "read_only": true, "indent_size": -1}),
            json!({"panel": id, "read_only": true, "indent_size": 65}),
            json!({"panel": id, "read_only": true, "indent_size": "16"}),
            json!({"panel": id, "read_only": true, "indent_size": null}),
            json!({"panel": id, "read_only": 1, "indent_size": 32}),
            json!({"panel": id, "read_only": null}),
            json!({"panel": id, "read_only": true, "unknown": false}),
            json!({"panel": "not-an-entity", "read_only": true}),
            json!({"panel": Entity::PLACEHOLDER.to_bits().to_string(), "read_only": true}),
        ] {
            assert!(set_tree_settings(app.world_mut(), payload).is_err());
            let settings = app
                .world()
                .get::<FileTreeSettings>(first)
                .copied()
                .unwrap_or_default();
            assert!(!settings.read_only);
            assert_eq!(settings.indent_size, 16.0);
        }
        set_tree_settings(
            app.world_mut(),
            json!({"panel": id, "read_only": true, "indent_size": 24}),
        )
        .unwrap();
        let settings = app.world().get::<FileTreeSettings>(first).unwrap();
        assert!(settings.read_only);
        assert_eq!(settings.indent_size, 24.0);
        assert!(
            !app.world()
                .get::<FileTreeSettings>(second)
                .copied()
                .unwrap_or_default()
                .read_only
        );
        set_tree_settings(app.world_mut(), json!({"panel": id, "indent_size": 0})).unwrap();
        assert!(
            app.world()
                .get::<FileTreeSettings>(first)
                .unwrap()
                .read_only
        );
        assert_eq!(
            app.world()
                .get::<FileTreeSettings>(first)
                .unwrap()
                .indent_size,
            0.0
        );
        set_tree_settings(
            app.world_mut(),
            json!({"panel": id, "read_only": false, "indent_size": 64}),
        )
        .unwrap();
        let inspected = inspect(app.world_mut(), serde_json::Value::Null).unwrap();
        let target = inspected["panels"]
            .as_array()
            .unwrap()
            .iter()
            .find(|panel| panel["entity"] == id)
            .unwrap();
        assert_eq!(
            target["settings"],
            json!({"read_only": false, "indent_size": 64.0})
        );
        stop(&mut app);
    }

    #[test]
    fn file_error_dialogs_explain_recovery_and_keep_raw_errors_in_details() {
        let raw = "Permission denied (os error 13)";
        let preview = preview_error_dialog(Some(Path::new("/project/notes.txt")), raw.into());
        assert!(preview.title.contains("notes.txt"));
        assert!(preview.message.contains("Check access"));
        assert!(preview.message.contains("Reload"));
        assert!(!preview.message.contains(raw));
        assert!(preview.details.as_deref().unwrap().contains(raw));
    }

    #[test]
    fn open_routing_errors_report_once_per_attempt_without_drawing() {
        let fixture = Fixture::new();
        let mut app = host();
        let (_, layout) = workspace(&mut app, "Project");
        let panel = tree(&mut app, layout, &fixture.0);
        let relative = Path::new("not-an-absolute-source.txt");
        request(&mut app, panel, relative, FileTreeActivation::Keyboard);
        assert_eq!(app.world().resource::<ErrorDialogs>().len(), 1);
        assert!(
            app.world()
                .resource::<FileEvents>()
                .errors
                .get(&panel)
                .unwrap()
                .contains("absolute")
        );
        // Removing the queue models consuming/dismissing it without depending
        // on window creation or the modal renderer in an application unit test.
        app.world_mut().remove_resource::<ErrorDialogs>();
        for _ in 0..3 {
            app.update();
        }
        assert!(
            app.world()
                .get_resource::<ErrorDialogs>()
                .is_none_or(ErrorDialogs::is_empty)
        );
        request(&mut app, panel, relative, FileTreeActivation::Keyboard);
        assert_eq!(app.world().resource::<ErrorDialogs>().len(), 1);
        stop(&mut app);
    }

    #[test]
    fn preview_load_admission_errors_are_reported_and_deduplicated_without_drawing() {
        let fixture = Fixture::new();
        let source = fixture.text("source.txt", "content");
        let mut app = host();
        let (_, layout) = workspace(&mut app, "Project");
        let viewer = preview_panel(app.world_mut(), layout);
        Workspaces::new(app.world_mut()).shutdown_work().unwrap();
        let error = load_preview(app.world_mut(), viewer, source.clone(), false).unwrap_err();
        // Open routing may report this same admission failure before Update
        // collects the viewer's pending notification. Only one modal is needed.
        request_error_dialog(
            app.world_mut(),
            preview_error_dialog(Some(&source), error.clone()),
        );
        app.update();
        assert_eq!(app.world().resource::<ErrorDialogs>().len(), 1);
        assert_eq!(
            app.world()
                .get::<FilePreviewPanel>(viewer)
                .unwrap()
                .error
                .as_ref(),
            Some(&error)
        );
        app.world_mut().remove_resource::<ErrorDialogs>();
        for _ in 0..3 {
            app.update();
        }
        assert!(app.world().get_resource::<ErrorDialogs>().is_none());
        wait(&mut app, |world| Workspaces::new(world).work_finished());
    }

    #[test]
    fn startup_errors_are_edge_reported_even_without_a_workspace_or_window() {
        let mut app = host();
        app.world_mut().resource_mut::<StartupRoot>().error =
            Some("getcwd failed: os error 13".into());
        app.update();
        assert_eq!(app.world().resource::<ErrorDialogs>().len(), 1);
        app.world_mut().remove_resource::<ErrorDialogs>();
        for _ in 0..3 {
            app.update();
        }
        assert!(app.world().get_resource::<ErrorDialogs>().is_none());
        app.world_mut().resource_mut::<StartupRoot>().error =
            Some("a different root resolution failure".into());
        app.update();
        assert_eq!(app.world().resource::<ErrorDialogs>().len(), 1);
    }

    #[test]
    fn plugin_does_not_choose_cwd_or_create_storage() {
        let fixture = Fixture::new();
        let mut app = host();
        let (_, layout) = workspace(&mut app, "Unconfigured");
        open_files(app.world_mut(), layout);
        app.update();
        let panel = app
            .world_mut()
            .query_filtered::<Entity, With<FileTreePanel>>()
            .single(app.world())
            .unwrap();
        assert!(app.world().get::<FileTreeRoot>(panel).is_none());
        assert!(app.world().resource::<StartupRoot>().root.is_none());
        assert!(!fixture.path(".bed").exists());
        stop(&mut app);
    }

    #[test]
    fn buffered_opens_use_real_source_and_origin_layout_and_reuse_preview() {
        let fixture = Fixture::new();
        let first = fixture.text("one.txt", "actual disk text one");
        let second = fixture.text("two.txt", "actual disk text two");
        let third = fixture.text("three.txt", "actual disk text three");
        let mut app = host();
        let (origin, layout) = workspace(&mut app, "Origin");
        let panel = tree(&mut app, layout, &fixture.0);
        let (other, other_layout) = workspace(&mut app, "Other");
        let other_id = Workbench::for_entity(app.world_mut(), other)
            .unwrap()
            .workspace_id();
        Workspaces::new(app.world_mut()).activate(other_id).unwrap();
        request(&mut app, panel, &first, FileTreeActivation::SingleClick);
        let viewer = viewers(&mut app, origin)[0];
        wait(&mut app, |world| {
            world
                .get::<FilePreviewPanel>(viewer)
                .unwrap()
                .load
                .is_none()
        });
        assert_eq!(text(&app, viewer), "actual disk text one");
        assert!(app.world().get::<ActiveWorkspace>(other).is_some());
        assert!(viewers(&mut app, other).is_empty());
        assert_eq!(
            Workbench::for_entity(app.world_mut(), viewer)
                .unwrap()
                .panel_layout(viewer)
                .unwrap(),
            layout
        );
        assert_eq!(
            app.world().get::<PanelInstance>(viewer).unwrap().title,
            "one.txt"
        );
        assert!(app.world().get::<PanelInstance>(viewer).unwrap().preview);
        request(&mut app, panel, &second, FileTreeActivation::SingleClick);
        wait(&mut app, |world| {
            world
                .get::<FilePreviewPanel>(viewer)
                .unwrap()
                .load
                .is_none()
        });
        assert_eq!(viewers(&mut app, origin), vec![viewer]);
        assert_eq!(text(&app, viewer), "actual disk text two");
        request(&mut app, panel, &second, FileTreeActivation::DoubleClick);
        assert!(!app.world().get::<PanelInstance>(viewer).unwrap().preview);
        request(&mut app, panel, &third, FileTreeActivation::SingleClick);
        assert_eq!(viewers(&mut app, origin).len(), 2);
        request(&mut app, panel, &second, FileTreeActivation::Keyboard);
        request(&mut app, panel, &second, FileTreeActivation::ContextMenu);
        assert_eq!(viewers(&mut app, origin).len(), 2);
        let other_panel = tree(&mut app, other_layout, &fixture.0);
        request(
            &mut app,
            other_panel,
            &second,
            FileTreeActivation::SingleClick,
        );
        assert_eq!(viewers(&mut app, other).len(), 1);
        assert_ne!(viewers(&mut app, other)[0], viewer);
        stop(&mut app);
    }

    #[test]
    fn preview_slots_are_layout_local_even_in_same_workspace() {
        let fixture = Fixture::new();
        let file = fixture.text("same.txt", "same source");
        let mut app = host();
        let (workspace, layout) = workspace(&mut app, "Origin");
        let other_layout = app
            .world_mut()
            .spawn((Layout::default(), InWorkspace(workspace)))
            .id();
        app.world_mut()
            .spawn((Area::default(), InLayout(other_layout)));
        let left = tree(&mut app, layout, &fixture.0);
        let right = tree(&mut app, other_layout, &fixture.0);
        request(&mut app, left, &file, FileTreeActivation::SingleClick);
        request(&mut app, right, &file, FileTreeActivation::SingleClick);
        assert_eq!(viewers(&mut app, workspace).len(), 2);
        stop(&mut app);
    }

    #[test]
    fn preview_reads_are_bounded_and_binary_is_metadata_only() {
        let fixture = Fixture::new();
        let binary = fixture.path("binary.bin");
        fs::write(&binary, [0, 255, 128]).unwrap();
        assert!(matches!(
            read_preview(&binary).unwrap(),
            PreviewContent::Metadata(_)
        ));
        let large = fixture.path("large.txt");
        File::create(&large)
            .unwrap()
            .set_len(MAX_PREVIEW_BYTES + 1)
            .unwrap();
        let PreviewContent::Metadata(summary) = read_preview(&large).unwrap() else {
            panic!()
        };
        assert!(summary.contains("256 MiB"));
        let missing = fixture.path("missing.txt");
        assert!(matches!(
            read_preview(&missing),
            Err(PreviewReadError::Missing)
        ));
        for kind in [
            std::io::ErrorKind::PermissionDenied,
            std::io::ErrorKind::Interrupted,
            std::io::ErrorKind::TimedOut,
            std::io::ErrorKind::WouldBlock,
        ] {
            assert!(matches!(
                preview_read_error(&missing, kind.into()),
                PreviewReadError::Failed(_)
            ));
        }
    }

    #[test]
    fn stale_preview_completion_cannot_replace_newer_source() {
        let fixture = Fixture::new();
        let file = fixture.text("new.txt", "new result");
        let mut app = host();
        let (workspace, layout) = workspace(&mut app, "Origin");
        let viewer = preview_panel(app.world_mut(), layout);
        let old_path = fixture.path("old.txt");
        let delayed = app
            .world_mut()
            .get_mut::<WorkspaceWork>(workspace)
            .unwrap()
            .spawn_task(Some(viewer), "Old preview", |cancel| {
                cancel.wait(Duration::from_secs(2));
                Ok(PreviewContent::Text(TextBuffer::from("stale result")))
            })
            .unwrap();
        {
            let mut preview = app.world_mut().get_mut::<FilePreviewPanel>(viewer).unwrap();
            preview.generation = 1;
            preview.path = Some(old_path.clone());
            preview.load = Some(PreviewLoad {
                generation: 1,
                workspace,
                path: old_path,
                handle: delayed,
                document_stamp: None,
            });
        }
        load_preview(app.world_mut(), viewer, file.clone(), false).unwrap();
        wait(&mut app, |world| {
            world
                .get::<FilePreviewPanel>(viewer)
                .unwrap()
                .load
                .is_none()
        });
        assert_eq!(text(&app, viewer), "new result");
        assert_eq!(
            app.world()
                .get::<FilePreviewPanel>(viewer)
                .unwrap()
                .path
                .as_ref(),
            Some(&file)
        );
        stop(&mut app);
    }

    #[test]
    fn moved_preview_rebinds_and_confirmed_trash_unloads_sole_tab() {
        let fixture = Fixture::new();
        let source = fixture.text("before.txt", "retained data");
        let destination = fixture.path("after.txt");
        let mut app = host();
        let (workspace, layout) = workspace(&mut app, "Origin");
        let panel = tree(&mut app, layout, &fixture.0);
        request(&mut app, panel, &source, FileTreeActivation::Keyboard);
        let viewer = viewers(&mut app, workspace)[0];
        wait(&mut app, |world| {
            world
                .get::<FilePreviewPanel>(viewer)
                .unwrap()
                .load
                .is_none()
        });
        fs::rename(&source, &destination).unwrap();
        operation(
            &mut app,
            workspace,
            panel,
            &fixture.0,
            FileTreeChange::Moved {
                source,
                destination: destination.clone(),
            },
        );
        wait(&mut app, |world| {
            world
                .get::<FilePreviewPanel>(viewer)
                .unwrap()
                .load
                .is_none()
        });
        assert_eq!(
            app.world()
                .get::<FilePreviewPanel>(viewer)
                .unwrap()
                .path
                .as_ref(),
            Some(&destination)
        );
        assert_eq!(
            app.world().get::<PanelInstance>(viewer).unwrap().title,
            "after.txt"
        );
        fs::remove_file(&destination).unwrap();
        operation(
            &mut app,
            workspace,
            panel,
            &fixture.0,
            FileTreeChange::Trashed { path: destination },
        );
        let state = app.world().get::<FilePreviewPanel>(viewer).unwrap();
        assert!(state.path.is_none());
        assert!(state.content.is_none());
        assert!(state.load.is_none());
        assert!(state.missing);
        assert_eq!(
            app.world().get::<PanelInstance>(viewer).unwrap().title,
            FilePreviewPanel::LABEL
        );
        assert!(
            app.world()
                .get_resource::<ErrorDialogs>()
                .is_none_or(ErrorDialogs::is_empty)
        );
        stop(&mut app);
    }

    #[test]
    fn own_directory_move_rebinds_all_layouts_before_late_missing_observation() {
        let fixture = Fixture::new();
        fs::create_dir(fixture.path("before")).unwrap();
        let source = fixture.text("before/source.txt", "follow the own move");
        let mut app = host();
        let (workspace, layout) = workspace(&mut app, "Project");
        let other_layout = app
            .world_mut()
            .spawn((Layout::default(), InWorkspace(workspace)))
            .id();
        app.world_mut()
            .spawn((Area::default(), InLayout(other_layout)));
        let first_tree = tree(&mut app, layout, &fixture.0);
        let second_tree = tree(&mut app, other_layout, &fixture.0);
        request(&mut app, first_tree, &source, FileTreeActivation::Keyboard);
        request(&mut app, second_tree, &source, FileTreeActivation::Keyboard);
        let panels = viewers(&mut app, workspace);
        wait(&mut app, |world| {
            panels.iter().all(|panel| {
                world
                    .get::<FilePreviewPanel>(*panel)
                    .unwrap()
                    .content
                    .is_some()
            }) && world
                .get::<PreviewObservation>(workspace)
                .is_some_and(|observation| observation.pending.is_none())
        });
        fs::rename(fixture.path("before"), fixture.path("after")).unwrap();
        app.world_mut()
            .get_mut::<PreviewObservation>(workspace)
            .unwrap()
            .next_check = None;
        observe_preview_sources(app.world_mut());
        // Finish real managed metadata work WITHOUT applying its old-path
        // NotFound result, then deliver the confirmed own move in the same frame.
        let deadline = Instant::now() + Duration::from_secs(8);
        while app
            .world()
            .get::<PreviewObservation>(workspace)
            .unwrap()
            .pending
            .as_ref()
            .unwrap()
            .status()
            == WorkStatus::Running
        {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        operation(
            &mut app,
            workspace,
            first_tree,
            &fixture.0,
            FileTreeChange::Moved {
                source: fixture.path("before"),
                destination: fixture.path("after"),
            },
        );
        let destination = fixture.path("after/source.txt");
        wait(&mut app, |world| {
            panels.iter().all(|panel| {
                world
                    .get::<FilePreviewPanel>(*panel)
                    .is_some_and(|state| state.load.is_none())
            })
        });
        for panel in panels {
            let state = app.world().get::<FilePreviewPanel>(panel).unwrap();
            assert_eq!(state.path.as_ref(), Some(&destination));
            assert!(!state.missing);
            assert_eq!(text(&app, panel), "follow the own move");
        }
        stop(&mut app);
    }

    #[test]
    fn external_parent_deletion_unloads_hidden_preview_outside_current_tree_roots() {
        let fixture = Fixture::new();
        fs::create_dir(fixture.path("never-expanded")).unwrap();
        let file = fixture.text("never-expanded/source.txt", "remove these bytes");
        let other_root = Fixture::new();
        let mut app = host();
        let (origin, layout) = workspace(&mut app, "Origin");
        let panel = tree(&mut app, layout, &fixture.0);
        request(&mut app, panel, &file, FileTreeActivation::Keyboard);
        let viewer = viewers(&mut app, origin)[0];
        wait(&mut app, |world| {
            world
                .get::<FilePreviewPanel>(viewer)
                .unwrap()
                .content
                .is_some()
        });
        // The nested parent was never expanded. Retargeting the tree removes
        // even its root watch; the preview must own its source observation.
        app.world_mut()
            .entity_mut(panel)
            .insert(FileTreeRoot(other_root.0.clone()));
        app.world_mut()
            .get_mut::<Workspace>(origin)
            .unwrap()
            .background_updates = false;
        let (other, _) = workspace(&mut app, "Foreground");
        let other_id = Workbench::for_entity(app.world_mut(), other)
            .unwrap()
            .workspace_id();
        Workspaces::new(app.world_mut()).activate(other_id).unwrap();
        let generation = app
            .world()
            .get::<FilePreviewPanel>(viewer)
            .unwrap()
            .generation;
        fs::remove_dir_all(fixture.path("never-expanded")).unwrap();
        wait(&mut app, |world| {
            world
                .get::<FilePreviewPanel>(viewer)
                .unwrap()
                .path
                .is_none()
        });
        let state = app.world().get::<FilePreviewPanel>(viewer).unwrap();
        assert!(state.content.is_none());
        assert!(state.load.is_none());
        assert!(state.error.is_none());
        assert!(state.missing);
        assert!(state.generation > generation);
        assert_eq!(
            app.world().get::<PanelInstance>(viewer).unwrap().title,
            FilePreviewPanel::LABEL
        );
        assert!(!app.world().get::<PanelInstance>(viewer).unwrap().preview);
        assert!(app.world().get::<ActiveWorkspace>(other).is_some());
        stop(&mut app);
    }

    #[test]
    fn external_delete_closes_shared_area_tab_but_unloads_sole_tab_in_other_layout() {
        let fixture = Fixture::new();
        let shared = fixture.text("shared.txt", "deleted content");
        let retained = fixture.text("retained.txt", "surviving content");
        let mut app = host();
        let (workspace, layout) = workspace(&mut app, "Project");
        let other_layout = app
            .world_mut()
            .spawn((Layout::default(), InWorkspace(workspace)))
            .id();
        app.world_mut()
            .spawn((Area::default(), InLayout(other_layout)));
        let left_tree = tree(&mut app, layout, &fixture.0);
        let right_tree = tree(&mut app, other_layout, &fixture.0);
        request(&mut app, left_tree, &shared, FileTreeActivation::Keyboard);
        let left = viewers(&mut app, workspace)[0];
        request(&mut app, left_tree, &retained, FileTreeActivation::Keyboard);
        let survivor = viewers(&mut app, workspace)
            .into_iter()
            .find(|panel| *panel != left)
            .unwrap();
        request(&mut app, right_tree, &shared, FileTreeActivation::Keyboard);
        let right = viewers(&mut app, workspace)
            .into_iter()
            .find(|panel| *panel != left && *panel != survivor)
            .unwrap();
        assert_eq!(
            app.world().get::<InArea>(left).unwrap().0,
            app.world().get::<InArea>(survivor).unwrap().0
        );
        wait(&mut app, |world| {
            [left, right, survivor].iter().all(|panel| {
                world
                    .get::<FilePreviewPanel>(*panel)
                    .is_some_and(|state| state.content.is_some())
            })
        });
        fs::remove_file(&shared).unwrap();
        wait(&mut app, |world| {
            world.get::<FilePreviewPanel>(left).is_none()
                && world
                    .get::<FilePreviewPanel>(right)
                    .is_some_and(|state| state.path.is_none())
        });
        assert!(
            app.world()
                .get::<FilePreviewPanel>(right)
                .unwrap()
                .content
                .is_none()
        );
        assert_eq!(text(&app, survivor), "surviving content");
        let area = app.world().get::<InArea>(survivor).unwrap().0;
        assert_eq!(
            app.world().get::<Area>(area).unwrap().selected,
            Some(survivor)
        );
        stop(&mut app);
    }

    #[test]
    fn confirmed_directory_trash_clears_all_descendants_without_touching_siblings() {
        let fixture = Fixture::new();
        fs::create_dir(fixture.path("folder")).unwrap();
        let first = fixture.text("folder/one.txt", "one");
        let second = fixture.text("folder/two.txt", "two");
        let sibling = fixture.text("folder-sibling.txt", "sibling");
        let mut app = host();
        let (workspace, layout) = workspace(&mut app, "Project");
        let tree = tree(&mut app, layout, &fixture.0);
        for file in [&first, &second, &sibling] {
            request(&mut app, tree, file, FileTreeActivation::Keyboard);
        }
        let panels = viewers(&mut app, workspace);
        let retained = panels
            .iter()
            .copied()
            .find(|panel| {
                app.world()
                    .get::<FilePreviewPanel>(*panel)
                    .unwrap()
                    .path
                    .as_ref()
                    == Some(&sibling)
            })
            .unwrap();
        wait(&mut app, |world| {
            panels.iter().all(|panel| {
                world
                    .get::<FilePreviewPanel>(*panel)
                    .unwrap()
                    .content
                    .is_some()
            })
        });
        // Do not invoke OS Trash in tests. Delete the disposable fixture and
        // inject only the confirmed own-operation outcome before polling.
        fs::remove_dir_all(fixture.path("folder")).unwrap();
        operation(
            &mut app,
            workspace,
            tree,
            &fixture.0,
            FileTreeChange::Trashed {
                path: fixture.path("folder"),
            },
        );
        assert_eq!(viewers(&mut app, workspace), vec![retained]);
        assert_eq!(text(&app, retained), "sibling");
        stop(&mut app);
    }

    #[test]
    fn late_read_cannot_resurrect_source_deleted_during_initial_loading() {
        let fixture = Fixture::new();
        let source = fixture.text("source.txt", "old disk bytes");
        let mut app = host();
        let (workspace, layout) = workspace(&mut app, "Project");
        let viewer = preview_panel(app.world_mut(), layout);
        let (release, waiting) = std::sync::mpsc::channel();
        let (finished, completion) = std::sync::mpsc::channel();
        let task = app
            .world_mut()
            .get_mut::<WorkspaceWork>(workspace)
            .unwrap()
            .spawn_task(Some(viewer), "Delayed file read", move |_| {
                // Model a successful read whose completion races with delete;
                // cancellation alone must not be the data-integrity guarantee.
                waiting.recv_timeout(Duration::from_secs(8)).unwrap();
                finished.send(()).unwrap();
                Ok(PreviewContent::Text(TextBuffer::from("late deleted bytes")))
            })
            .unwrap();
        {
            let mut state = app.world_mut().get_mut::<FilePreviewPanel>(viewer).unwrap();
            state.path = Some(source.clone());
            state.generation = 1;
            state.load = Some(PreviewLoad {
                generation: 1,
                path: source.clone(),
                workspace,
                handle: task,
                document_stamp: None,
            });
        }
        // A real existing source with an outstanding read must NOT unload just
        // because its tree snapshot/preview content is currently Loading/empty.
        wait(&mut app, |world| {
            world
                .get::<PreviewObservation>(workspace)
                .is_some_and(|observation| {
                    observation.next_check.is_some() && observation.pending.is_none()
                })
        });
        assert_eq!(
            app.world()
                .get::<FilePreviewPanel>(viewer)
                .unwrap()
                .path
                .as_ref(),
            Some(&source)
        );
        assert!(
            app.world()
                .get::<FilePreviewPanel>(viewer)
                .unwrap()
                .load
                .is_some()
        );
        fs::remove_file(&source).unwrap();
        wait(&mut app, |world| {
            world
                .get::<FilePreviewPanel>(viewer)
                .unwrap()
                .path
                .is_none()
        });
        release.send(()).unwrap();
        completion.recv_timeout(Duration::from_secs(8)).unwrap();
        app.update();
        let state = app.world().get::<FilePreviewPanel>(viewer).unwrap();
        assert!(state.content.is_none());
        assert!(state.load.is_none());
        assert!(state.path.is_none());
        assert!(state.generation > 1);
        assert!(
            app.world()
                .get_resource::<ErrorDialogs>()
                .is_none_or(ErrorDialogs::is_empty)
        );
        stop(&mut app);
    }

    #[test]
    fn bounded_source_batches_rotate_past_first_sixty_four_viewers() {
        let fixture = Fixture::new();
        let retained = fixture.text("retained.txt", "retain");
        let deleted = fixture.text("deleted.txt", "delete");
        let mut app = host();
        let (_, layout) = workspace(&mut app, "Project");
        let mut panels = Vec::new();
        for _ in 0..=MAX_PREVIEW_CHECKS {
            let panel = preview_panel(app.world_mut(), layout);
            let mut state = app.world_mut().get_mut::<FilePreviewPanel>(panel).unwrap();
            state.path = Some(retained.clone());
            state.content = Some(PreviewContent::Text(TextBuffer::from("retain")));
            panels.push(panel);
        }
        // Entity order is the batch order; leave the first full batch intact so
        // repeatedly taking only that batch would never notice this deletion.
        let last = *panels.iter().max().unwrap();
        app.world_mut()
            .get_mut::<FilePreviewPanel>(last)
            .unwrap()
            .path = Some(deleted.clone());
        fs::remove_file(deleted).unwrap();
        wait(&mut app, |world| {
            world.get::<FilePreviewPanel>(last).is_none()
        });
        for panel in panels.into_iter().filter(|panel| *panel != last) {
            assert_eq!(text(&app, panel), "retain");
        }
        stop(&mut app);
    }

    #[cfg(unix)]
    #[test]
    fn permission_errors_do_not_drop_loaded_content() {
        use std::os::unix::fs::PermissionsExt;
        let fixture = Fixture::new();
        fs::create_dir(fixture.path("restricted")).unwrap();
        let source = fixture.text("restricted/source.txt", "retained on uncertain metadata");
        let parent = source.parent().unwrap().to_owned();
        struct RestorePermissions(PathBuf, fs::Permissions);
        impl Drop for RestorePermissions {
            fn drop(&mut self) {
                let _ = fs::set_permissions(&self.0, self.1.clone());
            }
        }
        let _restore =
            RestorePermissions(parent.clone(), fs::metadata(&parent).unwrap().permissions());
        fs::set_permissions(&parent, fs::Permissions::from_mode(0)).unwrap();
        let Err(error) = fs::metadata(&source) else {
            return;
        }; // privileged/capability-limited host
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        let mut app = host();
        let (workspace, layout) = workspace(&mut app, "Project");
        let viewer = preview_panel(app.world_mut(), layout);
        {
            let mut state = app.world_mut().get_mut::<FilePreviewPanel>(viewer).unwrap();
            state.path = Some(source.clone());
            state.content = Some(PreviewContent::Text(TextBuffer::from(
                "retained on uncertain metadata",
            )));
        }
        wait(&mut app, |world| {
            world
                .get::<PreviewObservation>(workspace)
                .is_some_and(|observation| {
                    observation.next_check.is_some() && observation.pending.is_none()
                })
        });
        assert_eq!(
            app.world()
                .get::<FilePreviewPanel>(viewer)
                .unwrap()
                .path
                .as_ref(),
            Some(&source)
        );
        assert_eq!(text(&app, viewer), "retained on uncertain metadata");
        // An actual read failure is reported from Update even without drawing
        // this viewer; uncertainty must still retain its previously loaded data.
        load_preview(app.world_mut(), viewer, source.clone(), true).unwrap();
        wait(&mut app, |world| {
            world
                .get_resource::<ErrorDialogs>()
                .is_some_and(|dialogs| !dialogs.is_empty())
        });
        assert_eq!(app.world().resource::<ErrorDialogs>().len(), 1);
        assert!(
            app.world()
                .get::<FilePreviewPanel>(viewer)
                .unwrap()
                .error
                .is_some()
        );
        assert_eq!(text(&app, viewer), "retained on uncertain metadata");
        app.world_mut().remove_resource::<ErrorDialogs>();
        for _ in 0..3 {
            app.update();
        }
        assert!(app.world().get_resource::<ErrorDialogs>().is_none());
        load_preview(app.world_mut(), viewer, source, true).unwrap();
        wait(&mut app, |world| {
            world
                .get_resource::<ErrorDialogs>()
                .is_some_and(|dialogs| !dialogs.is_empty())
        });
        assert_eq!(app.world().resource::<ErrorDialogs>().len(), 1);
        stop(&mut app);
    }

    #[test]
    fn a_parent_replaced_by_a_file_is_confirmed_source_absence() {
        let fixture = Fixture::new();
        fs::create_dir(fixture.path("parent")).unwrap();
        let source = fixture.text("parent/child.txt", "content");
        fs::remove_dir_all(source.parent().unwrap()).unwrap();
        fs::write(source.parent().unwrap(), b"replacement file").unwrap();
        assert!(matches!(
            read_preview(&source),
            Err(PreviewReadError::Missing)
        ));
    }

    #[test]
    fn oversized_preferences_cannot_replace_readable_storage_or_create_new_storage() {
        let fixture = Fixture::new();
        let path = fixture.path(".bed/filetree2.json");
        let original = FileTreePreferences::default().encode().unwrap();
        atomic_write(&path, &original).unwrap();
        let oversized = vec![b' '; MAX_PREFERENCES_BYTES as usize + 1];
        assert!(
            atomic_write(&path, &oversized)
                .unwrap_err()
                .contains("4 MiB")
        );
        assert_eq!(fs::read(&path).unwrap(), original);
        let fresh = fixture.path("new/.bed/filetree2.json");
        assert!(atomic_write(&fresh, &oversized).is_err());
        assert!(!fixture.path("new").exists());
    }
    #[test]
    fn shared_file_route_opens_without_a_tree_and_preserves_existing_documents() {
        use crate::opening::{FileOpenMode, open_file};
        let fixture = Fixture::new();
        let first = fixture.text("first.rs", "fn first() {}\n");
        let second = fixture.text("second.rs", "fn second() {}\n");
        let image = fixture.path("foo.png");
        fs::write(&image, b"\x89PNG\0binary").unwrap();
        let mut app = host();
        let (owner, layout) = workspace(&mut app, "Shared route");
        let mut workbench = Workbench::for_entity(app.world_mut(), layout).unwrap();
        let source = workbench.destination_area(layout).unwrap();
        let largest = workbench
            .split_area(source, SplitAxis::Horizontal, 2500, [1, 1])
            .unwrap();
        let opened =
            open_file(app.world_mut(), layout, first.clone(), FileOpenMode::Focus).unwrap();
        assert_eq!(app.world().get::<InArea>(opened.panel).unwrap().0, largest);
        wait(&mut app, |world| opened.result(world).is_some());
        opened.result(app.world()).unwrap().unwrap();
        assert!(
            !app.world()
                .get::<PanelInstance>(opened.panel)
                .unwrap()
                .preview
        );
        let document = edit_text(&mut app, opened.panel, "fn edited() {}\n");
        let next = open_file(app.world_mut(), layout, second, FileOpenMode::Focus).unwrap();
        wait(&mut app, |world| next.result(world).is_some());
        next.result(app.world()).unwrap().unwrap();
        assert_ne!(opened.panel, next.panel);
        let reopened = open_file(app.world_mut(), layout, first, FileOpenMode::Focus).unwrap();
        assert_eq!(reopened.panel, opened.panel);
        assert_eq!(
            app.world()
                .get::<DocumentView>(reopened.panel)
                .unwrap()
                .document(),
            document.id()
        );
        assert_eq!(text(&app, reopened.panel), "fn edited() {}\n");
        let image = open_file(app.world_mut(), layout, image, FileOpenMode::Focus).unwrap();
        wait(&mut app, |world| image.result(world).is_some());
        image.result(app.world()).unwrap().unwrap();
        assert!(matches!(
            app.world()
                .get::<FilePreviewPanel>(image.panel)
                .unwrap()
                .content,
            Some(PreviewContent::Metadata(_))
        ));
        assert_eq!(viewers(&mut app, owner).len(), 3);
        assert_eq!(
            app.world_mut()
                .query_filtered::<Entity, With<FileTreePanel>>()
                .iter(app.world())
                .count(),
            0
        );
        stop(&mut app);
    }

    #[test]
    fn shared_file_open_completion_rejects_replaced_or_closed_viewers() {
        use crate::opening::{FileOpenMode, open_file};
        let fixture = Fixture::new();
        let first = fixture.text("first.rs", "first");
        let second = fixture.text("second.rs", "second");
        let mut app = host();
        let (_, layout) = workspace(&mut app, "Open completion");
        let first = open_file(app.world_mut(), layout, first, FileOpenMode::Preview).unwrap();
        let second = open_file(app.world_mut(), layout, second, FileOpenMode::Preview).unwrap();
        assert_eq!(first.panel, second.panel);
        assert!(
            first
                .result(app.world())
                .unwrap()
                .unwrap_err()
                .contains("changed")
        );
        app.world_mut().despawn(second.panel);
        assert!(
            second
                .result(app.world())
                .unwrap()
                .unwrap_err()
                .contains("closed")
        );
        stop(&mut app);
    }
}
