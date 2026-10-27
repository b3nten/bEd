//! Application policy for local files: explicit roots, text editors and storage.
//! The reusable tree never chooses cwd, a viewer, or a persistence destination.
#[cfg(any(test, feature = "automation"))]
use crate::opening::open_folder;
use crate::opening::{FolderOpens, poll_folder_opens};
use crate::persistence::ReflectProjectPersist;
#[cfg(test)]
use bedtime_editor2::BufferEdit;
use bedtime_editor2::TextBuffer;
use bedtime_filesystem2::FileSource as ByteSource;
#[cfg(any(test, feature = "automation"))]
use bedtime_filetree2::FileTreePreferences;
#[cfg(feature = "automation")]
use bedtime_filetree2::FileTreeSettings;
use bedtime_filetree2::{
    FileTreeActivation, FileTreeOpenRequested, FileTreePanel, FileTreePanelDrag, FileTreePlugin,
    FileTreeRoot,
};
#[cfg(test)]
use bedtime_filetree2::{
    FileTreeChange, FileTreeOperationFinished, FileTreePreferencesChanged, FileTreeVisibility,
};
use bedtime_ui2::{UiContext, egui};
use bedtime_workbench2::*;
use bevy::{ecs::message::MessageCursor, prelude::*};
#[cfg(feature = "automation")]
use std::collections::BTreeSet;
#[cfg(test)]
use std::time::{Duration, Instant};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};

#[cfg(test)]
const MAX_PREVIEW_BYTES: u64 = 256 * 1024 * 1024;
#[cfg(test)]
const MAX_PREFERENCES_BYTES: u64 = 4 * 1024 * 1024;

pub struct FilesPlugin;

#[cfg(feature = "automation")]
#[derive(Resource, Default)]
struct FolderAutomation(Option<crate::opening::FolderOpenResult>);

#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FilesSystems {
    Update,
}

impl Plugin for FilesPlugin {
    fn build(&self, app: &mut App) {
        register_menu(app);
        app.add_plugins((FileTreePlugin, crate::file_editor::FileEditorPlugin))
            .insert_resource(FileTreePanelDrag(crate::opening::file_panel_factory))
            .insert_resource(bedtime_filetree2::FileTreeOnOpen(open_tree_source))
            .insert_resource(bedtime_filetree2::FileTreeOpenWith(
                crate::opening::tree_viewers,
            ))
            .init_resource::<StartupRoot>()
            .init_resource::<FolderOpens>()
            .init_resource::<FileEvents>()
            .add_systems(
                Update,
                update_files
                    .in_set(FilesSystems::Update)
                    .after(bedtime_filetree2::FileTreeSystems::Update)
                    .after(bedtime_filesystem2::workbench::FilesystemDocumentSystems::Update)
                    .before(bedtime_editor2::workbench::update_document_analysis),
            )
            // UI preference changes must be admitted before shutdown can
            // cancel storage. No filesystem work happens in this system.
            .add_systems(
                PreUpdate,
                admit_preferences
                    .after(bedtime_ui2::UiSystems::Draw)
                    .after(WorkbenchSystems::Apply)
                    .before(SessionSystems::Sync),
            );
        #[cfg(feature = "automation")]
        {
            use bedtime_automation2::AutomationAppExt;
            app.init_resource::<FolderAutomation>();
            app.register_automation_command("app.open_file", crate::opening::automation_open)
                .register_automation_command("app.inspect_filetree", inspect)
                .register_automation_command("app.open_folder", |world, arguments| {
                    let path = arguments
                        .get("path")
                        .and_then(|value| value.as_str())
                        .ok_or("path must be a string")?;
                    let result = open_folder(world, PathBuf::from(path))?;
                    world.resource_mut::<FolderAutomation>().0 = Some(result);
                    Ok(serde_json::json!({"admitted": true}))
                })
                .register_automation_command("app.inspect_folder_open", |world, _| {
                    let state = world.resource::<FolderAutomation>();
                    let result = state
                        .0
                        .as_ref()
                        .map(|reply| reply.lock().unwrap().try_recv());
                    Ok(match result {
                        Some(Ok(Ok(crate::opening::FolderOpened::Reused))) => {
                            serde_json::json!({"reused":true})
                        }
                        Some(Ok(Ok(crate::opening::FolderOpened::Launched(pid)))) => {
                            serde_json::json!({"pid":pid})
                        }
                        Some(Ok(Err(error))) => serde_json::json!({"error":error}),
                        _ => serde_json::json!({"pending":true}),
                    })
                })
                .register_automation_command("app.set_filetree_root", set_tree_root)
                .register_automation_command("app.set_filetree_settings", set_tree_settings)
                .register_automation_command("app.drop_files", drop_files);
        }
    }
}

/// The World owns its canonical project root for this session.
#[derive(Resource, Clone, Debug, PartialEq, Eq)]
pub struct ProjectRoot(pub PathBuf);

/// Explicit folder-backed identity. A browsing/cwd root alone never opts into
/// project storage: the default home session remains entirely in memory.
#[derive(Resource, Clone, Copy, Debug, Default)]
pub struct ProjectFolder;

#[derive(Resource, Default)]
struct StartupRoot {
    // None means no application default was requested, including headless hosts.
    // Some(None) requests the user's home root, without enabling project storage.
    requested: Option<Option<PathBuf>>,
    persistent: bool,
    policy_resolved: bool,
    restoring_last: bool,
    loading: Option<ManagedTask<Result<(PathBuf, Option<crate::project::ProjectLock>), String>>>,
    root: Option<PathBuf>,
    error: Option<String>,
    reported_error: Option<String>,
    remembered: Option<PathBuf>,
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
/// valid; production's startup request targets exactly one session.
pub fn root_ready(world: &World) -> bool {
    if world.contains_resource::<ProjectRoot>() {
        return true;
    }
    world.get_resource::<StartupRoot>().is_none_or(|startup| {
        startup.loading.is_none() && startup.requested.is_none() && startup.error.is_none()
    })
}

pub fn project_directory(world: &World) -> Option<PathBuf> {
    world
        .get_resource::<ProjectRoot>()
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
    let canonical = ByteSource::local(&path)
        .canonicalize()
        .map_err(|error| format!("{}: {error}", path.display()))?
        .path()
        .to_owned();
    if !ByteSource::local(&canonical)
        .metadata()
        .map_err(|e| e.to_string())?
        .is_dir()
    {
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
    open_with: MessageCursor<bedtime_filetree2::FileTreeOpenWithRequested>,
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
        let explicit: Vec<_> = events
            .open_with
            .read(world.resource::<Messages<bedtime_filetree2::FileTreeOpenWithRequested>>())
            .cloned()
            .collect();
        for request in explicit {
            let result = (|| {
                let viewer =
                    crate::opening::ViewerId::from_id(&request.choice).ok_or("Unknown viewer")?;
                let tree = world
                    .get::<FileTreePanel>(request.panel)
                    .ok_or("Source Files panel closed")?;
                if !request.path.starts_with(&tree.snapshot().root) {
                    return Err("Opening source changed".into());
                }
                let layout =
                    Workbench::for_entity(world, request.panel)?.panel_layout(request.panel)?;
                crate::opening::open_file_with(
                    world,
                    layout,
                    request.path,
                    crate::opening::FileOpenMode::Focus,
                    Some(viewer),
                    None,
                )?;
                Ok::<_, String>(())
            })();
            if let Err(error) = result {
                request_error_dialog(
                    world,
                    ErrorDialog {
                        title: "Open With".into(),
                        message: error,
                        details: None,
                    },
                );
            }
        }
        events
            .activations
            .retain(|entity, _| world.get::<FileTreePanel>(*entity).is_some());
        events
            .errors
            .retain(|entity, _| world.get::<FileTreePanel>(*entity).is_some());
    });

    crate::images::update(world);
    let fallbacks: Vec<_> = world
        .query::<(
            Entity,
            &FilePanel,
            &crate::images::ImagePreview,
            &DocumentView,
        )>()
        .iter(world)
        .filter(|(_, p, i, _)| !p.explicit && i.error.is_some())
        .map(|(p, _, i, v)| (p, v.document(), i.error.clone().unwrap()))
        .collect();
    for (panel, id, error) in fallbacks {
        if crate::file_editor::can_replace(world, panel) {
            let viewer = if cast_document::<TextBuffer>(world, id).is_ok() {
                crate::opening::ViewerId::Code
            } else {
                crate::opening::ViewerId::Hex
            };
            if set_viewer(world, panel, viewer).is_ok() {
                accept_opened_document(world, panel, Ok(id));
                world.get_mut::<FilePanel>(panel).unwrap().notice = Some(format!(
                    "Image could not be decoded; showing file contents. {error}"
                ));
            }
        }
    }

    crate::models::update(world);
    bedtime_hex2::workbench::update(world);
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
                && let Some(preferences) =
                    world.get_resource::<crate::settings::StartupPreferences>()
                && preferences.open_last_workspace
                && let Some(root) = &preferences.last_workspace
            {
                startup.requested = Some(Some(root.clone()));
                startup.persistent = true;
                startup.restoring_last = true;
            }
            startup.policy_resolved = true;
        }
        if let Some(task) = &startup.loading {
            let result = match task.try_recv() {
                Ok(result) => Some(result),
                Err(std::sync::mpsc::TryRecvError::Empty) => None,
                Err(_) => Some(Err("Project root resolution stopped".into())),
            };
            if let Some(result) = result {
                startup.loading = None;
                startup.requested = None;
                match result {
                    Ok((root, claim)) => {
                        world.insert_resource(ProjectRoot(root.clone()));
                        if let Some(claim) = claim {
                            world.insert_resource(claim);
                            world.insert_resource(ProjectFolder);
                            crate::session::initialize_folder_name(
                                world,
                                root.file_name()
                                    .unwrap_or(root.as_os_str())
                                    .to_string_lossy()
                                    .into_owned(),
                            );
                        }
                        startup.root = Some(root);
                    }
                    Err(error) if startup.restoring_last => {
                        startup.restoring_last = false;
                        startup.persistent = false;
                        startup.requested = Some(None);
                        request_error_dialog(
                            world,
                            ErrorDialog {
                                title: "Open last workspace".into(),
                                message:
                                    "The last project couldn't be opened. Opening home instead."
                                        .into(),
                                details: Some(error),
                            },
                        );
                    }
                    Err(error) => startup.error = Some(error),
                }
            }
        }
        if startup.root.is_none()
            && startup.loading.is_none()
            && let Some(requested) = startup.requested.clone()
        {
            let persistent = startup.persistent;
            let task = world.resource_mut::<ManagedWork>().spawn_task(
                None,
                "Resolve project root",
                move |_| {
                    let root = resolve_root(requested)?;
                    let claim = if persistent {
                        Some(crate::project::ProjectLock::acquire(&root)?)
                    } else {
                        None
                    };
                    Ok((root, claim))
                },
            );
            match task {
                Ok(task) => startup.loading = Some(task),
                Err(error) => {
                    startup.error = Some(error);
                    startup.requested = None;
                }
            }
        }
    });
}

fn configure_panels(world: &mut World) {
    let Some(root) = world.get_resource::<ProjectRoot>().map(|r| r.0.clone()) else {
        return;
    };
    let panels: Vec<_> = world
        .query_filtered::<Entity, (With<FileTreePanel>, Without<FileTreeRoot>)>()
        .iter(world)
        .collect();
    for panel in panels {
        world.entity_mut(panel).insert(FileTreeRoot(root.clone()));
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
        let root = workbench
            .world()
            .get_resource::<ProjectRoot>()
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
enum FilePresentation {
    Document,
    #[cfg(test)]
    Text(TextBuffer),
    Metadata(Arc<str>),
}

#[cfg(test)]
#[derive(Debug)]
enum PreviewReadError {
    Missing,
    Failed(String),
}

#[cfg(test)]
impl std::fmt::Display for PreviewReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing => f.write_str("Source missing"),
            Self::Failed(error) => f.write_str(error),
        }
    }
}

#[cfg(test)]
fn source_is_missing(error: &std::io::Error) -> bool {
    // A parent replaced by a file also proves this source path no longer exists.
    matches!(
        error.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
    )
}

#[cfg(test)]
fn preview_read_error(path: &Path, error: impl Into<std::io::Error>) -> PreviewReadError {
    let error = error.into();
    if source_is_missing(&error) {
        PreviewReadError::Missing
    } else {
        PreviewReadError::Failed(format!("{}: {error}", path.display()))
    }
}

/// A replaceable file tab. Text binds to a shared document; unsupported files
/// keep a bounded metadata presentation. The loaded bytes are a source snapshot.
#[derive(Component, Default)]
#[require(PanelInstance = PanelInstance::of::<Self>())]
pub struct FilePanel {
    pub(crate) viewer: crate::opening::ViewerId,
    pub(crate) explicit: bool,
    notice: Option<String>,
    path: Option<PathBuf>,
    generation: u64,
    loading: bool,
    content: Option<FilePresentation>,
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

impl Panel for FilePanel {
    const KEY: &'static str = "bed.code";
    const LABEL: &'static str = "Code";

    fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, panel: Entity) {
        let surface = *world.get::<WorkbenchSurface>(context.surface).unwrap();
        let state = world.get::<Self>(panel).unwrap();
        let image = state.viewer == crate::opening::ViewerId::Image;
        let model = state.viewer == crate::opening::ViewerId::Model;
        let hex = state.viewer == crate::opening::ViewerId::Hex;
        let notice = state.notice.clone();
        let path = state.path.clone();
        let error = state.error.clone();
        let loading = state.loading;
        let metadata = match &state.content {
            Some(FilePresentation::Metadata(summary)) => Some(summary.clone()),
            _ => None,
        };
        let document = world.get::<DocumentView>(panel).map(DocumentView::document);
        let info = document.and_then(|id| inspect_document(world, id).ok());
        let mut reload = false;
        let mut resolution = None;
        context.frame(ui).padded(if model { [0.0; 2] } else { surface.padding }, |frame| {
                    let ui = frame.ui;
                    let mut model_notices;
                    let ui = if model {
                        let mut bounds = ui.available_rect_before_wrap().shrink(8.0);
                        bounds.min.y += 32.0;
                        crate::models::draw(ui, world, context, panel);
                        model_notices = ui.new_child(egui::UiBuilder::new().id_salt("model-notices").max_rect(bounds).layout(egui::Layout::top_down(egui::Align::LEFT)));
                        &mut model_notices
                    } else { ui };
                    if let Some(error) = &error { ui.colored_label(ui.visuals().error_fg_color, error); }
                    if let Some(notice) = &notice { ui.weak(notice); }
                    if document.is_none() && !image && !hex && !model {
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
                    if model { return; }
                    if hex {
                        if let Some(path) = panel_path(world, panel) {
                            if bedtime_hex2::workbench::draw(ui, world, panel,
                                |ui, world| crate::opening::viewer_menu(ui, world, panel)) {
                                let _ = open_file_panel(world, panel, path, true);
                            }
                        }
                    } else if image {
                        crate::images::draw(ui, world, context, panel);
                    } else if document.is_some() {
                        let language_tools = bedtime_lsp2::workbench::language_activity(world,panel).is_some();
                        if let Some(mut editor) = world.get_mut::<bedtime_editor2::workbench::WorkbenchEditor>(panel) { editor.view.language_tools = language_tools; }
                        match bedtime_editor2::workbench::draw_editor(ui, world, context, panel) {
                            Ok(output) => {
                                crate::lsp::editor_input(ui, world, panel, &output);
                                output.response.context_menu(|ui| {
                                    crate::menus::editor_context_menu(ui, world, panel);
                                    crate::lsp::context_menu(ui, world, panel);
                                    crate::opening::viewer_menu(ui, world, panel);
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
                        let response = egui::ScrollArea::vertical().show(ui, |ui| { ui.monospace(summary.as_ref()) }).inner;
                        response.context_menu(|ui| crate::opening::viewer_menu(ui, world, panel));
                    } else {
                        let (_, response) = ui.allocate_exact_size(ui.available_size(), egui::Sense::click());
                        response.context_menu(|ui| crate::opening::viewer_menu(ui, world, panel));
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
                open_file_panel(world, panel, path, true)?;
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
    let original = world.get::<FilePanel>(panel).unwrap();
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
            FilePanel {
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

fn open_tree_source(
    world: &mut World,
    request: bedtime_filetree2::FileTreeSourceOpen,
) -> Result<(), String> {
    let layout = Workbench::for_entity(world, request.panel)?.panel_layout(request.panel)?;
    let mode = match request.activation {
        FileTreeActivation::SingleClick => crate::opening::FileOpenMode::Preview,
        FileTreeActivation::ContextMenu => crate::opening::FileOpenMode::KeepOpen,
        _ => crate::opening::FileOpenMode::Focus,
    };
    let viewer = request
        .choice
        .as_deref()
        .map(|id| crate::opening::ViewerId::from_id(id).ok_or("Unknown viewer"))
        .transpose()?;
    {
        let mut events = world.resource_mut::<FileEvents>();
        let counts = events.activations.entry(request.panel).or_default();
        match request.activation {
            FileTreeActivation::SingleClick => counts.single += 1,
            FileTreeActivation::DoubleClick => counts.double += 1,
            FileTreeActivation::Keyboard => counts.keyboard += 1,
            FileTreeActivation::ContextMenu => counts.context += 1,
        };
    }
    crate::opening::open_source_file(world, layout, request.source, mode, viewer, None).map(|_| ())
}

fn open_requested(world: &mut World, request: &FileTreeOpenRequested) -> Result<Entity, String> {
    if world.get::<FileTreePanel>(request.panel).is_none() {
        return Err("Source Files panel closed".into());
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
    let source = bedtime_editor2::workbench::code_document_for_panel(world, panel)
        .map(|document| bed_structure2::SourceBinding { panel, document });
    world.insert_resource(bed_structure2::StructureSource { source });
}

/// Viewer changes are admitted only on replaceable tabs, before a new source load.
pub(crate) fn set_viewer(
    world: &mut World,
    panel: Entity,
    viewer: crate::opening::ViewerId,
) -> Result<(), String> {
    if world
        .get::<FilePanel>(panel)
        .ok_or("Missing file viewer")?
        .viewer
        != viewer
    {
        if !crate::file_editor::can_replace(world, panel) {
            return Err("This viewer has unsaved work".into());
        }
        crate::file_editor::detach(world, panel)?;
    }
    world.get_mut::<FilePanel>(panel).unwrap().viewer = viewer;
    world.get_mut::<PanelInstance>(panel).unwrap().key = match viewer {
        crate::opening::ViewerId::Code => "bed.code",
        crate::opening::ViewerId::Image => "bed.image",
        crate::opening::ViewerId::Hex => "bed.hex",
        crate::opening::ViewerId::Model => "bed.model",
    }
    .into();
    if viewer == crate::opening::ViewerId::Image {
        world
            .entity_mut(panel)
            .insert(crate::images::ImagePreview::default());
    } else {
        world
            .entity_mut(panel)
            .remove::<crate::images::ImagePreview>();
    }
    if viewer == crate::opening::ViewerId::Model {
        if world.get::<crate::models::ModelPreview>(panel).is_none() {
            world.entity_mut(panel).insert((
                crate::models::ModelPreview::default(),
                bedtime_model2::ModelScene::default(),
            ));
        }
    } else {
        world
            .entity_mut(panel)
            .remove::<(crate::models::ModelPreview, bedtime_model2::ModelScene)>();
    }
    if viewer != crate::opening::ViewerId::Hex {
        world
            .entity_mut(panel)
            .remove::<bedtime_hex2::workbench::HexPanel>();
    }
    Ok(())
}

/// A session descriptor, not canonical content or an ECS identity.
pub fn panel_path(world: &World, panel: Entity) -> Option<PathBuf> {
    world
        .get::<DocumentView>(panel)
        .and_then(|view| document_source_path(world, view.document()))
        .or_else(|| {
            world
                .get::<FilePanel>(panel)
                .and_then(|state| state.path.clone())
        })
}

pub(crate) fn file_open_generation(world: &World, panel: Entity) -> u64 {
    world.get::<FilePanel>(panel).unwrap().generation
}

pub(crate) fn file_open_result(
    world: &World,
    panel: Entity,
    generation: u64,
) -> Option<Result<(), String>> {
    let Some(viewer) = world.get::<FilePanel>(panel) else {
        return Some(Err("File viewer closed before opening completed".into()));
    };
    if viewer.generation != generation {
        return Some(Err("File viewer changed before opening completed".into()));
    }
    if viewer.loading {
        return None;
    }
    if viewer.viewer == crate::opening::ViewerId::Model && viewer.error.is_none() && !viewer.missing
    {
        return crate::models::open_result(world, panel);
    }
    if viewer.viewer == crate::opening::ViewerId::Image && viewer.error.is_none() && !viewer.missing
    {
        return crate::images::open_result(world, panel);
    }
    if viewer.viewer == crate::opening::ViewerId::Hex && viewer.error.is_none() && !viewer.missing {
        return bedtime_hex2::workbench::open_result(world, panel);
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
pub(crate) fn open_file_panel(
    world: &mut World,
    panel: Entity,
    path: PathBuf,
    retain: bool,
) -> Result<(), String> {
    open_panel_source(world, panel, path, retain, None)
}
pub(crate) fn open_resolved_file_panel(
    world: &mut World,
    panel: Entity,
    source: bedtime_filesystem2::FsDocumentSource,
    retain: bool,
) -> Result<(), String> {
    open_panel_source(world, panel, source.path(), retain, Some(source))
}
fn open_panel_source(
    world: &mut World,
    panel: Entity,
    path: PathBuf,
    retain: bool,
    source: Option<bedtime_filesystem2::FsDocumentSource>,
) -> Result<(), String> {
    let same = crate::file_editor::same_source(world, panel, &path);
    if world.get::<DocumentDraft>(panel).is_some() {
        return Err("Finish text composition before opening or reloading".into());
    }
    if retain && same {
        if let Some(view) = world.get::<DocumentView>(panel) {
            let id = view.document();
            world.get_mut::<FilePanel>(panel).unwrap().loading = true;
            return bedtime_filesystem2::workbench::refresh_file_document(
                world,
                panel,
                id,
                accept_opened_document,
            );
        }
    }
    if !same && !crate::file_editor::can_replace(world, panel) {
        return Err("This document has unsaved work".into());
    }
    crate::file_editor::detach(world, panel)?;
    {
        let mut state = world
            .get_mut::<FilePanel>(panel)
            .ok_or("Missing file panel")?;
        state.generation = state
            .generation
            .checked_add(1)
            .expect("File opening generation exhausted");
        state.path = Some(path.clone());
        state.content = None;
        state.error = None;
        state.notice = None;
        state.missing = false;
        state.report_error = false;
    }
    world.get_mut::<PanelInstance>(panel).unwrap().title = path
        .file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned();
    world.get_mut::<FilePanel>(panel).unwrap().loading = true;
    let result = match source {
        Some(source) => bedtime_filesystem2::workbench::open_resolved_document(
            world,
            panel,
            source,
            accept_opened_document,
        ),
        None => bedtime_filesystem2::workbench::open_file_document(
            world,
            panel,
            path,
            accept_opened_document,
        ),
    };
    if let Err(error) = &result {
        let mut state = world.get_mut::<FilePanel>(panel).unwrap();
        state.loading = false;
        state.error = Some(error.clone());
        state.report_error = true;
    }
    result
}
fn accept_opened_document(world: &mut World, panel: Entity, result: Result<DocumentId, String>) {
    if world.get::<FilePanel>(panel).is_none() {
        return;
    }
    world.get_mut::<FilePanel>(panel).unwrap().loading = false;
    let result = (|| {
        let document = result?;
        let text = cast_document::<TextBuffer>(world, document).ok();
        let state = world.get::<FilePanel>(panel).unwrap();
        let automatic = !state.explicit;
        let viewer = state.viewer;
        if text.is_none() && viewer == crate::opening::ViewerId::Code {
            if automatic {
                set_viewer(world, panel, crate::opening::ViewerId::Hex)?;
            } else {
                return Err("This source has a BinaryDocument and cannot be opened as text".into());
            }
        }
        if let Some(handle) = text {
            crate::file_editor::bind(world, panel, document)?;
            let _ = handle;
            world.get_mut::<FilePanel>(panel).unwrap().content = Some(FilePresentation::Document);
        } else {
            let handle =
                cast_document::<BinaryDocument>(world, document).map_err(|e| e.to_string())?;
            bind_document_view(world, panel, handle).map_err(|e| e.to_string())?;
            world.get_mut::<FilePanel>(panel).unwrap().content =
                Some(FilePresentation::Metadata(Arc::from("Binary document")));
        }
        world
            .entity_mut(panel)
            .insert(bedtime_filesystem2::workbench::FileDocumentPanel::new(
                file_document_location,
            ));
        if world.get::<FilePanel>(panel).unwrap().viewer == crate::opening::ViewerId::Hex {
            let source = document_bytes(world, document).map_err(|e| e.to_string())?;
            world
                .entity_mut(panel)
                .insert(bedtime_hex2::workbench::HexPanel::new(source));
        }
        Ok::<_, String>(())
    })();
    if let Err(error) = result {
        let mut state = world.get_mut::<FilePanel>(panel).unwrap();
        state.error = Some(error);
        state.report_error = true;
    }
}

fn file_document_location(
    world: &mut World,
    panel: Entity,
    location: bedtime_filesystem2::workbench::FileDocumentLocation,
) {
    use bedtime_filesystem2::workbench::FileDocumentLocation;
    match location {
        FileDocumentLocation::Available(path) => {
            if let Some(mut state) = world.get_mut::<FilePanel>(panel) {
                state.path = Some(path);
                state.missing = false;
            }
        }
        FileDocumentLocation::Missing { retained } => {
            if let Some(mut state) = world.get_mut::<FilePanel>(panel) {
                state.path = None;
                state.loading = false;
                state.generation += 1;
                state.missing = true;
                state.error = None;
                state.report_error = false;
                if !retained {
                    state.content = None;
                }
            }
            if !retained {
                world.entity_mut(panel).remove::<(
                    crate::images::ImagePreview,
                    crate::models::ModelPreview,
                    bedtime_model2::ModelScene,
                    bedtime_hex2::workbench::HexPanel,
                )>();
                world.get_mut::<PanelInstance>(panel).unwrap().title =
                    world.get::<FilePanel>(panel).unwrap().viewer.label().into();
            }
        }
    }
}

#[cfg(test)]
fn read_preview(path: &Path) -> Result<FilePresentation, PreviewReadError> {
    let source = ByteSource::local(path);
    let metadata = source
        .metadata()
        .map_err(|error| preview_read_error(path, error))?;
    if !metadata.is_file() {
        return Ok(FilePresentation::Metadata(Arc::from(
            "Not a regular file; content was not read.",
        )));
    }
    let summary = |reason: &str| {
        FilePresentation::Metadata(Arc::from(format!(
            "{reason}\nSize: {} bytes\nRead-only permissions: {}\nModified: {:?}",
            metadata.len, metadata.readonly, metadata.modified
        )))
    };
    if metadata.len > MAX_PREVIEW_BYTES {
        return Ok(summary("File is larger than the 256 MiB text file limit."));
    }
    let bytes = match source.read(MAX_PREVIEW_BYTES) {
        Ok(snapshot) => snapshot.bytes,
        Err(bedtime_filesystem2::Error::TooLarge { .. }) => {
            return Ok(summary("File grew beyond the 256 MiB text file limit."));
        }
        Err(error) => return Err(preview_read_error(path, error)),
    };
    if bytes.contains(&0) {
        return Ok(summary("Binary file; content is not shown as text."));
    }
    match String::from_utf8(bytes) {
        Ok(text) => Ok(FilePresentation::Text(TextBuffer::from(text))),
        Err(_) => Ok(summary(
            "Binary or non-UTF-8 file; content is not shown as text.",
        )),
    }
}

// Capture application metadata before workspace retirement can remove its owner.
pub fn admit_preferences(world: &mut World) {
    // Remember only a resolved active folder. The temporary home workspace does
    // not erase the last folder, and preference loading must finish first.
    if crate::storage::global_ready(world) {
        let root = world
            .get_resource::<ProjectFolder>()
            .and_then(|_| world.get_resource::<ProjectRoot>())
            .map(|r| r.0.clone());
        if let Some(root) = root
            && world.resource::<StartupRoot>().remembered.as_ref() != Some(&root)
            && let Some(mut preferences) =
                world.get_resource_mut::<crate::settings::StartupPreferences>()
        {
            preferences.last_workspace = Some(root.clone());
            world.resource_mut::<StartupRoot>().remembered = Some(root);
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
        let has_target = !world.resource::<ManagedWork>().is_closing();
        if has_target {
            return Ok(false);
        }
        world.resource_mut::<StartupRoot>().requested = None;
    }
    if bedtime_filetree2::filetree_operation_count(world) != 0 {
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
    for mut state in world.query::<&mut FilePanel>().iter_mut(world) {
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
    let trees: Vec<_> = world.query::<(Entity, &FileTreePanel, &PanelPresentation)>()
        .iter(world).map(|(entity, tree, presentation)| {
            let snapshot = tree.snapshot();
            let settings = world.get::<FileTreeSettings>(entity).copied().unwrap_or_default();
            json!({
                "entity": entity.to_bits().to_string(),
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
    let previews: Vec<_> = world.query::<(Entity, &FilePanel, &PanelInstance)>()
        .iter(world).map(|(entity, preview, instance)| {
            let document = world.get::<DocumentView>(entity).map(DocumentView::document);
            let canonical = document.and_then(|id| cast_document::<TextBuffer>(world, id).ok()).and_then(|handle| read_document(world, handle).ok());
            let text = canonical;
            let (kind, content) = if let Some(text) = text {
                ("text", Some(text.range_text(0..text.clamp_boundary(text.len().min(16*1024)))))
            } else {
                match &preview.content {
                    Some(FilePresentation::Metadata(text)) => ("metadata",Some(text.to_string())),
                    _ => ("empty",None),
                }
            };
            json!({"entity": entity.to_bits().to_string(),
                "area": world.get::<InArea>(entity).map(|area| area.0.to_bits().to_string()),
                "layout": world.get::<InArea>(entity).and_then(|area| world.get::<InLayout>(area.0))
                    .map(|layout| layout.0.to_bits().to_string()),
                "path": preview.path.as_ref().map(|path| path.to_string_lossy()),
                "viewer": preview.viewer.id(),
                "viewer_explicit": preview.explicit,
                "hex": bedtime_hex2::workbench::inspect(world,entity),
                "image": world.get::<crate::images::ImagePreview>(entity).map(|image| json!({"zoom": image.view.transform.zoom(), "center": [image.view.transform.center().x,image.view.transform.center().y], "dimensions": image.view.dimensions().map(|s| [s.x,s.y]), "rect": image.view.rect().map(|r| json!({"min":[r.min.x,r.min.y],"max":[r.max.x,r.max.y]})), "error": image.error})),
                "model": world.get::<bedtime_model2::ModelScene>(entity).map(|scene| json!({"ready":scene.is_ready(),"loading":scene.is_loading(),"info":scene.info(),"generation":scene.generation(),"error":scene.error(),"panel_rect":world.get::<PanelPresentation>(entity).map(|p|json!({"min":p.rect.min.to_array(),"max":p.rect.max.to_array()})),"view":scene.view,"rect":world.get::<bedtime_ui2::UiViewport>(entity).map(|v|json!({"min":[v.rect.min.x,v.rect.min.y],"max":[v.rect.max.x,v.rect.max.y]}))})),
                "generation": preview.generation, "loading": preview.loading || (preview.viewer == crate::opening::ViewerId::Hex && bedtime_hex2::workbench::open_result(world,entity).is_none()) || (preview.viewer == crate::opening::ViewerId::Image && crate::images::open_result(world, entity).is_none()) || (preview.viewer == crate::opening::ViewerId::Model && crate::models::open_result(world, entity).is_none()),
                "pinned": !instance.preview, "title": instance.title, "kind": if preview.viewer == crate::opening::ViewerId::Model { "model" } else if preview.viewer == crate::opening::ViewerId::Image { "image" } else if preview.viewer == crate::opening::ViewerId::Hex { "hex" } else { kind },
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
    let preferences = world.resource::<FileTreePreferences>().roots.iter().map(|(root, visibility)| json!({
        "root":root.to_string_lossy(), "hide_hidden":visibility.hide_hidden,
        "hidden_paths":visibility.hidden_paths.iter().map(|p|p.to_string_lossy()).collect::<Vec<_>>()
    })).collect::<Vec<_>>();
    let clipboard = world.get_resource::<Clipboard>().map(|clipboard| {
        json!({
            "mode": format!("{:?}", clipboard.mode()), "revision": clipboard.revision(),
            "initialization_error": clipboard.initialization_error(),
        })
    });
    let project = world.get_resource::<ProjectRoot>().map(|root| json!({ "root":root.0.to_string_lossy(), "persistent":world.contains_resource::<ProjectFolder>(), "session_ready":crate::session::ready(world), "settings_ready":crate::storage::ready(world) }));
    let startup = world.resource::<StartupRoot>();
    Ok(
        json!({"panels": trees, "previews": previews, "storage": storage, "clipboard": clipboard,
        "preferences": preferences, "project": project, "startup_root": startup.root.as_ref().map(|path| path.to_string_lossy()),
        "startup_error": startup.error}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use bedtime_editor2::workbench::WorkbenchEditor;
    use std::fs::{self, File};
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
    fn preference_failure_preserves_editing_opening_and_language_policy_until_repaired() {
        use bedtime_lsp2::workbench::LspProject;
        let fixture = Fixture::new();
        let path = fixture.text("first.txt", "first");
        let second = fixture.text("second.txt", "second");
        let config = fixture.path("config");
        fs::create_dir_all(&config).unwrap();
        let preference_path = config.join("preferences.bed");
        fs::write(&preference_path, b"malformed initial preferences").unwrap();
        let mut app = host();
        app.add_plugins((
            crate::settings::SettingsPlugin,
            crate::session::SessionPlugin,
            crate::lsp::LanguageToolsPlugin,
        ));
        app.insert_resource(ProjectRoot(fixture.0.clone()))
            .insert_resource(ProjectFolder);
        crate::session::remember_initial_state(app.world_mut()).unwrap();
        crate::storage::configure_directory(&mut app, config);
        let (_, layout) = workspace(&mut app, "Policy");
        let preview = preview_panel(app.world_mut(), layout);
        app.world_mut()
            .entity_mut(preview)
            .insert(WorkbenchEditor::default());
        wait(&mut app, |world| crate::storage::flush(world).is_err());
        assert!(!crate::storage::ready(app.world()));
        crate::settings::configure_editor(app.world_mut(), preview);
        assert!(
            app.world()
                .get::<WorkbenchEditor>(preview)
                .unwrap()
                .view
                .settings
                .read_only
        );
        assert!(
            crate::opening::open_file(
                app.world_mut(),
                layout,
                path.clone(),
                crate::opening::FileOpenMode::KeepOpen
            )
            .is_err()
        );
        assert!(!app.world().contains_resource::<LspProject>());
        // Retry initial loading still keeps the barrier until successful admission.
        fs::remove_file(&preference_path).unwrap();
        crate::storage::retry_failed(app.world_mut());
        assert!(!crate::storage::global_ready(app.world()));
        wait(&mut app, |world| crate::storage::ready(world));
        app.update();
        assert!(app.world().contains_resource::<LspProject>());
        let first = crate::opening::open_file(
            app.world_mut(),
            layout,
            path,
            crate::opening::FileOpenMode::KeepOpen,
        )
        .unwrap();
        wait(&mut app, |world| first.result(world).is_some());
        assert!(
            !app.world()
                .get::<WorkbenchEditor>(first.panel)
                .unwrap()
                .view
                .settings
                .read_only
        );
        // A directory at the file destination forces atomic replacement to fail.
        fs::create_dir(&preference_path).unwrap();
        app.world_mut()
            .resource_mut::<crate::settings::GlobalPreferences>()
            .0
            .panel_gap = 12.0;
        wait(&mut app, |world| crate::storage::flush(world).is_err());
        app.update();
        assert!(crate::storage::ready(app.world()));
        assert!(app.world().contains_resource::<LspProject>());
        assert!(
            !app.world()
                .get::<WorkbenchEditor>(first.panel)
                .unwrap()
                .view
                .settings
                .read_only
        );
        let doc = edit_text(&mut app, first.panel, "edited despite storage failure");
        assert_eq!(
            read_document(app.world(), doc).unwrap(),
            "edited despite storage failure"
        );
        let opened = crate::opening::open_file(
            app.world_mut(),
            layout,
            second,
            crate::opening::FileOpenMode::KeepOpen,
        )
        .unwrap();
        wait(&mut app, |world| opened.result(world).is_some());
        assert!(opened.result(app.world()).unwrap().is_ok());
        assert!(
            !app.world()
                .get::<WorkbenchEditor>(opened.panel)
                .unwrap()
                .view
                .settings
                .read_only
        );
        fs::remove_dir(&preference_path).unwrap();
        crate::storage::retry_failed(app.world_mut());
        assert!(crate::storage::ready(app.world()));
        wait(&mut app, |world| crate::storage::flush(world).unwrap());
        // A subsequent refresh failure also retains the admitted policy.
        fs::write(&preference_path, b"malformed refresh").unwrap();
        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            app.update();
            if crate::storage::flush(app.world_mut()).is_err() {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(crate::storage::ready(app.world()));
        assert!(app.world().contains_resource::<LspProject>());
        stop(&mut app);
    }

    #[cfg(unix)]
    #[test]
    fn unavailable_non_utf8_paths_survive_preferences_and_session_metadata_restart() {
        use std::{ffi::OsString, os::unix::ffi::OsStringExt};
        let fixture = Fixture::new();
        // No native source is created: APFS rejects these names, and unavailable
        // sources must still be persistable on every supported filesystem.
        let root = fixture.0.join(OsString::from_vec(b"project-\xff".to_vec()));
        let path = root.join(OsString::from_vec(b"file-\xfe.txt".to_vec()));
        let config = fixture.path("config");
        let boot = || {
            let mut app = App::new();
            app.add_plugins((WorkbenchCorePlugin, crate::file_editor::FileEditorPlugin));
            app.init_resource::<FileTreePreferences>();
            app.add_plugins((
                crate::settings::SettingsPlugin,
                crate::session::SessionPlugin,
            ));
            app.insert_resource(ProjectRoot(root.clone()))
                .insert_resource(ProjectFolder);
            crate::storage::configure_directory(&mut app, config.clone());
            crate::session::remember_initial_state(app.world_mut()).unwrap();
            wait(&mut app, |world| crate::storage::ready(world));
            app
        };
        let mut app = boot();
        let (_, layout) = workspace(&mut app, "Native metadata");
        let panel = preview_panel(app.world_mut(), layout);
        app.world_mut().get_mut::<FilePanel>(panel).unwrap().path = Some(path.clone());
        tree(&mut app, layout, &root);
        *app.world_mut()
            .resource_mut::<crate::settings::StartupPreferences>() =
            crate::settings::StartupPreferences {
                open_last_workspace: true,
                last_workspace: Some(root.clone()),
            };
        let mut visibility = FileTreeVisibility::default();
        visibility
            .hide(PathBuf::from(path.file_name().unwrap()))
            .unwrap();
        app.world_mut()
            .resource_mut::<FileTreePreferences>()
            .roots
            .insert(root.clone(), visibility.clone());
        wait(&mut app, |world| {
            crate::storage::flush(world).unwrap() && crate::session::flush(world).unwrap()
        });
        let global_before = fs::read(config.join("preferences.bed")).unwrap();
        let project_path = crate::storage::workspace_path(app.world(), &root).unwrap();
        let project_before = fs::read(&project_path).unwrap();
        shutdown_work(app.world_mut()).unwrap();
        wait(&mut app, |world| work_finished(world));
        drop(app);
        let mut restored = boot();
        assert_eq!(
            restored
                .world()
                .resource::<crate::settings::StartupPreferences>()
                .last_workspace,
            Some(root.clone())
        );
        let panels = viewers(&mut restored);
        assert_eq!(panels.len(), 1);
        assert_eq!(panel_path(restored.world(), panels[0]), Some(path));
        let roots: Vec<_> = restored
            .world_mut()
            .query::<&FileTreeRoot>()
            .iter(restored.world())
            .map(|r| r.0.clone())
            .collect();
        assert_eq!(roots, [root.clone()]);
        assert_eq!(
            restored.world().resource::<FileTreePreferences>().roots[&root],
            visibility
        );
        wait(&mut restored, |world| {
            crate::storage::flush(world).unwrap() && crate::session::flush(world).unwrap()
        });
        assert!(crate::storage::ready(restored.world()));
        assert_eq!(
            fs::read(config.join("preferences.bed")).unwrap(),
            global_before
        );
        assert_eq!(fs::read(project_path).unwrap(), project_before);
        shutdown_work(restored.world_mut()).unwrap();
        wait(&mut restored, |world| work_finished(world));
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_project_and_file_survive_storage_and_startup_reopening() {
        use std::{ffi::OsString, os::unix::ffi::OsStringExt};
        let fixture = Fixture::new();
        let root = fixture.0.join(OsString::from_vec(b"project-\xff".to_vec()));
        match fs::create_dir(&root) {
            Ok(()) => {}
            Err(error)
                if cfg!(target_os = "macos") && error.raw_os_error() == Some(libc::EILSEQ) =>
            {
                eprintln!(
                    "APFS rejects invalid UTF-8 names; metadata restart is covered separately: {error}"
                );
                return;
            }
            Err(error) => panic!("Cannot create non-UTF-8 project fixture: {error}"),
        }
        let path = root.join(OsString::from_vec(b"file-\xfe.txt".to_vec()));
        fs::write(&path, "native contents").unwrap();
        let config = fixture.path("config");
        let boot = |argument: Option<PathBuf>| {
            let mut app = host();
            app.add_plugins((
                crate::settings::SettingsPlugin,
                crate::session::SessionPlugin,
                crate::layouts::LayoutsPlugin,
            ));
            crate::storage::configure_directory(&mut app, config.clone());
            crate::session::remember_initial_state(app.world_mut()).unwrap();
            app.insert_resource(StartupRoot {
                persistent: argument.is_some(),
                requested: Some(argument),
                ..default()
            });
            wait(&mut app, |world| {
                world.contains_resource::<ProjectRoot>()
                    && crate::storage::ready(world)
                    && active_layout(world).is_some()
            });
            app
        };
        let mut app = boot(Some(root.clone()));
        let root = fs::canonicalize(root).unwrap();
        let path = root.join(path.file_name().unwrap());
        let layout = active_layout(app.world()).unwrap();
        let opened = crate::opening::open_file(
            app.world_mut(),
            layout,
            path.clone(),
            crate::opening::FileOpenMode::KeepOpen,
        )
        .unwrap();
        tree(&mut app, layout, &root);
        wait(&mut app, |world| opened.result(world).is_some());
        assert!(opened.result(app.world()).unwrap().is_ok());
        app.world_mut()
            .resource_mut::<crate::settings::StartupPreferences>()
            .open_last_workspace = true;
        let mut visibility = FileTreeVisibility::default();
        visibility
            .hide(PathBuf::from(path.file_name().unwrap()))
            .unwrap();
        app.world_mut()
            .resource_mut::<FileTreePreferences>()
            .roots
            .insert(root.clone(), visibility.clone());
        wait(&mut app, |world| {
            crate::storage::flush(world).unwrap() && crate::session::flush(world).unwrap()
        });
        let global_before = fs::read(config.join("preferences.bed")).unwrap();
        let project_path = crate::storage::workspace_path(app.world(), &root).unwrap();
        let project_before = fs::read(&project_path).unwrap();
        stop(&mut app);
        drop(app);
        let mut restored = boot(None);
        assert_eq!(restored.world().resource::<ProjectRoot>().0, root);
        assert_eq!(
            restored
                .world()
                .resource::<crate::settings::StartupPreferences>()
                .last_workspace,
            Some(root.clone())
        );
        wait(&mut restored, |world| {
            world
                .query::<&FilePanel>()
                .iter(world)
                .any(|p| p.path.as_ref() == Some(&path) && p.content.is_some())
        });
        assert_eq!(
            restored.world().resource::<FileTreePreferences>().roots[&root],
            visibility
        );
        let tree_roots: Vec<_> = restored
            .world_mut()
            .query::<&FileTreeRoot>()
            .iter(restored.world())
            .map(|r| r.0.clone())
            .collect();
        assert_eq!(tree_roots, [root]);
        let panel = viewers(&mut restored)[0];
        assert!(
            !restored
                .world()
                .get::<WorkbenchEditor>(panel)
                .unwrap()
                .view
                .settings
                .read_only
        );
        wait(&mut restored, |world| {
            crate::storage::flush(world).unwrap() && crate::session::flush(world).unwrap()
        });
        assert_eq!(
            fs::read(config.join("preferences.bed")).unwrap(),
            global_before
        );
        assert_eq!(fs::read(project_path).unwrap(), project_before);
        stop(&mut restored);
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
            crate::session::remember_initial_state(app.world_mut()).unwrap();
            app.insert_resource(StartupRoot {
                persistent: argument.is_some(),
                requested: Some(argument),
                ..default()
            });
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                app.update();
                if app.world().get_resource::<ProjectRoot>().is_some()
                    && crate::storage::ready(app.world())
                    && app
                        .world()
                        .try_query::<&crate::layouts::LayoutSlot>()
                        .is_some()
                {
                    break;
                }
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(1));
            }
            (app, owner)
        };
        let (mut app, _owner) = boot(Some(fixture.0.clone()));
        app.world_mut()
            .resource_mut::<crate::settings::StartupPreferences>()
            .open_last_workspace = true;
        crate::layouts::switch(app.world_mut(), crate::layouts::LayoutSlot::Two).unwrap();
        let layout = Workbench::new(app.world_mut()).active_layout();
        let area = Workbench::for_entity(app.world_mut(), layout)
            .unwrap()
            .destination_area(layout)
            .unwrap();
        app.world_mut()
            .spawn((crate::settings::SettingsPanel, InArea(area)));
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
        let (mut app, _owner) = boot(None);
        assert_eq!(
            app.world().get_resource::<ProjectRoot>().unwrap().0,
            fixture.0
        );
        assert!(app.world().get_resource::<ProjectFolder>().is_some());
        let layout = Workbench::new(app.world_mut()).active_layout();
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
            app.insert_resource(crate::storage::ConfigDirectory(fixture.path("config")));
            app.add_plugins((
                crate::settings::SettingsPlugin,
                crate::session::SessionPlugin,
                crate::layouts::LayoutsPlugin,
            ));
            let (_owner, _) = workspace(&mut app, "Default");
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
            while app.world().get_resource::<ProjectRoot>().is_none() {
                app.update();
                assert!(Instant::now() < deadline, "startup did not resolve");
                std::thread::sleep(Duration::from_millis(1));
            }
            app.update();
            let persistent = explicit || (enabled && !missing);
            let expected = if persistent {
                fs::canonicalize(&fixture.0).unwrap()
            } else {
                resolve_root(None).unwrap()
            };
            assert_eq!(
                app.world().get_resource::<ProjectRoot>().unwrap().0,
                expected
            );
            assert_eq!(
                app.world().get_resource::<ProjectFolder>().is_some(),
                persistent
            );
            let layouts = app
                .world_mut()
                .query::<&crate::layouts::LayoutSlot>()
                .iter(app.world())
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

    fn workspace(app: &mut App, _name: &str) -> (Entity, Entity) {
        let layout = crate::test_layout(app.world_mut());
        (layout, layout)
    }
    fn preview_panel(world: &mut World, layout: Entity) -> Entity {
        let area = Workbench::for_entity(world, layout)
            .unwrap()
            .destination_area(layout)
            .unwrap();
        let panel = world.spawn((FilePanel::default(), InArea(area))).id();
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
        app.world_mut().write_message(FileTreeOpenRequested {
            panel,
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
    #[test]
    fn file_drops_pin_viewers_in_the_target_area_and_share_documents() {
        let fixture = Fixture::new();
        let path = fixture.text("dropped.txt", "shared contents");
        let mut app = host();
        let (_, layout) = workspace(&mut app, "Drops");
        let existing = crate::opening::open_file(
            app.world_mut(),
            layout,
            path.clone(),
            crate::opening::FileOpenMode::Preview,
        )
        .unwrap()
        .panel;
        wait(&mut app, |world| {
            file_open_result(world, existing, file_open_generation(world, existing)).is_some()
        });
        edit_text(&mut app, existing, "retained unsaved edits");
        let original_area = app.world().get::<InArea>(existing).unwrap().0;
        let target = Workbench::for_entity(app.world_mut(), layout)
            .unwrap()
            .split_area(original_area, SplitAxis::Horizontal, 5000, [100, 100])
            .unwrap();
        for split in [None, Some((SplitAxis::Vertical, true))] {
            let mut w = Workbench::for_entity(app.world_mut(), layout).unwrap();
            let stamp = w.stamp();
            w.requests().submit(WorkbenchRequest::Edit {
                stamp,
                edit: LayoutEdit::DropPanel {
                    area: target,
                    factory: crate::opening::file_panel_factory(path.clone()),
                    index: 0,
                    split,
                    minimum: [100, 100],
                },
            });
            app.update();
        }
        wait(&mut app, |world| {
            world
                .query::<&FilePanel>()
                .iter(world)
                .all(|p| p.loading == false && p.content.is_some())
        });
        let panels = Workbench::for_entity(app.world_mut(), layout)
            .unwrap()
            .panels(layout);
        assert_eq!(panels.len(), 3);
        let document = app
            .world()
            .get::<DocumentView>(existing)
            .unwrap()
            .document();
        for panel in panels.into_iter().filter(|&panel| panel != existing) {
            assert!(!app.world().get::<PanelInstance>(panel).unwrap().preview);
            assert_ne!(app.world().get::<InArea>(panel).unwrap().0, original_area);
            assert_eq!(panel_path(app.world(), panel), Some(path.clone()));
            assert_eq!(text(&app, panel), "retained unsaved edits");
            assert_eq!(
                app.world().get::<DocumentView>(panel).unwrap().document(),
                document
            );
        }
        assert_eq!(text(&app, existing), "retained unsaved edits");
        assert_eq!(fs::read_to_string(path).unwrap(), "shared contents");
        stop(&mut app);
    }

    fn stop(app: &mut App) {
        admit_preferences(app.world_mut());
        shutdown_work(app.world_mut()).unwrap();
        wait(app, |world| work_finished(world));
    }
    fn viewers(app: &mut App) -> Vec<Entity> {
        app.world_mut()
            .query_filtered::<Entity, With<FilePanel>>()
            .iter(app.world())
            .collect()
    }
    fn text(app: &App, panel: Entity) -> &TextBuffer {
        if let Some(view) = app.world().get::<DocumentView>(panel) {
            let handle = cast_document::<TextBuffer>(app.world(), view.document()).unwrap();
            return read_document(app.world(), handle).unwrap();
        }
        match &app.world().get::<FilePanel>(panel).unwrap().content {
            Some(FilePresentation::Text(text)) => text,
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
        crate::terminal::register_menu(&mut app);
        let (_workspace, layout) = workspace(&mut app, "Default");
        app.world_mut().resource_mut::<StartupRoot>().requested = Some(None);
        assert!(!root_ready(app.world()));
        assert!(!crate::storage::ready(app.world()));
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
            .insert_resource(ProjectRoot(fixture.0.clone()));
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
            ProjectRoot(fixture.0.clone()),
            FileTreePreferences::default(),
        ));
        let panel = tree(&mut app, layout, &fixture.0);
        app.update();
        app.world_mut()
            .get_resource_mut::<FileTreePreferences>()
            .unwrap()
            .roots
            .insert(fixture.0.clone(), FileTreeVisibility::default());
        app.world_mut().write_message(FileTreePreferencesChanged {});
        app.update();
        assert!(flush_preferences(app.world_mut()).unwrap());
        assert!(app.world().get::<FileTreePanel>(panel).is_some());
        assert!(!fixture.path(".bed").exists());
    }

    #[test]
    fn project_root_configures_trees_across_layouts() {
        let fixture = Fixture::new();
        let mut app = host();
        let (_, first) = workspace(&mut app, "First");
        let (_, second) = workspace(&mut app, "Second");
        app.insert_resource(ProjectRoot(fixture.0.clone()));
        open_files(app.world_mut(), first);
        open_files(app.world_mut(), second);
        app.update();
        for root in app
            .world_mut()
            .query_filtered::<&FileTreeRoot, With<FileTreePanel>>()
            .iter(app.world())
        {
            assert_eq!(root.0, fixture.0);
        }
        stop(&mut app);
    }

    #[test]
    fn selecting_a_preview_does_not_change_area_focus() {
        let fixture = Fixture::new();
        let path = fixture.text("preview.txt", "preview");
        let mut app = host();
        let (_workspace, layout) = workspace(&mut app, "Project");
        let tree = tree(&mut app, layout, &fixture.0);
        request(&mut app, tree, &path, FileTreeActivation::SingleClick);
        let preview = viewers(&mut app)[0];
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
        let (_workspace, layout) = workspace(&mut app, "Preview structure");
        let tree = tree(&mut app, layout, &fixture.0);
        let mut previous = None;
        for (path, label) in [(&first, "first"), (&second, "second"), (&first, "first")] {
            request(&mut app, tree, path, FileTreeActivation::SingleClick);
            wait(&mut app, |world| {
                world
                    .get_resource::<bed_structure2::StructureSource>()
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
                .get_resource::<bed_structure2::StructureSource>()
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
        let (_workspace, layout) = workspace(&mut app, "Code");
        let tree = tree(&mut app, layout, &fixture.0);
        request(&mut app, tree, &path, FileTreeActivation::SingleClick);
        wait(&mut app, |world| {
            world
                .query_filtered::<Entity, With<FilePanel>>()
                .iter(world)
                .any(|panel| analysis_for_panel(world, panel).is_some())
        });
        let panel = viewers(&mut app)[0];
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
                .get_resource::<bed_structure2::StructureSource>()
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
        open_file_panel(app.world_mut(), panel, path, true).unwrap();
        wait(&mut app, |world| {
            world.get::<FilePanel>(panel).unwrap().loading == false
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
        let (_workspace, layout) = workspace(&mut app, "Editor");
        let tree = tree(&mut app, layout, &fixture.0);
        request(&mut app, tree, &path, FileTreeActivation::SingleClick);
        wait(&mut app, |world| {
            world
                .query::<(&FilePanel, &DocumentView)>()
                .iter(world)
                .any(|(state, _)| state.path.as_ref() == Some(&path))
        });
        let panel = viewers(&mut app)[0];
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
        let (_workspace, layout) = workspace(&mut app, "Editor");
        let tree = tree(&mut app, layout, &fixture.0);
        request(&mut app, tree, &first, FileTreeActivation::SingleClick);
        wait(&mut app, |world| {
            world.query::<&DocumentView>().iter(world).next().is_some()
        });
        let panel = viewers(&mut app)[0];
        edit_text(&mut app, panel, "unsaved");
        request(&mut app, tree, &next, FileTreeActivation::SingleClick);
        wait(&mut app, |world| {
            world
                .query::<(&FilePanel, &DocumentView)>()
                .iter(world)
                .any(|(state, _)| state.path.as_ref() == Some(&next))
        });
        assert_eq!(text(&app, panel), "unsaved");
        assert_eq!(
            app.world().get::<FilePanel>(panel).unwrap().path.as_ref(),
            Some(&first)
        );
        assert_eq!(viewers(&mut app).len(), 2);
        stop(&mut app);
    }

    #[test]
    fn reopening_a_retained_dirty_file_through_files_preserves_edits_and_history() {
        let fixture = Fixture::new();
        let path = fixture.text("retained.rs", "fn base() {}\n");
        let mut app = host();
        let (_workspace, layout) = workspace(&mut app, "Retained");
        let tree = tree(&mut app, layout, &fixture.0);
        request(&mut app, tree, &path, FileTreeActivation::Keyboard);
        wait(&mut app, |world| {
            world.query::<&DocumentView>().iter(world).next().is_some()
        });
        let panel = viewers(&mut app)[0];
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
        let other = viewers(&mut app)[0];
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
        let (_workspace, layout) = workspace(&mut app, "Editor");
        let tree = tree(&mut app, layout, &fixture.0);
        request(&mut app, tree, &path, FileTreeActivation::SingleClick);
        wait(&mut app, |world| {
            world.query::<&DocumentView>().iter(world).next().is_some()
        });
        let panel = viewers(&mut app)[0];
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
        open_file_panel(app.world_mut(), panel, path.clone(), true).unwrap();
        wait(&mut app, |world| {
            world.get::<FilePanel>(panel).unwrap().loading == false
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
        let (_workspace, layout) = workspace(&mut app, "Editor");
        let tree = tree(&mut app, layout, &fixture.0);
        request(&mut app, tree, &path, FileTreeActivation::SingleClick);
        wait(&mut app, |world| {
            world.query::<&DocumentView>().iter(world).next().is_some()
        });
        let panel = viewers(&mut app)[0];
        let handle = edit_text(&mut app, panel, "mine");
        fs::rename(&path, &renamed).unwrap();
        bedtime_filesystem2::Filesystem::local().move_document_sources(&path, &renamed);
        app.update();
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
                .is_none(),
            "{:?}",
            inspect_document(app.world(), handle.id())
                .unwrap()
                .last_save_error
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
        let (_workspace, layout) = workspace(&mut app, "Editor");
        let tree = tree(&mut app, layout, &fixture.0);
        request(&mut app, tree, &path, FileTreeActivation::SingleClick);
        wait(&mut app, |world| {
            world.query::<&DocumentView>().iter(world).next().is_some()
        });
        let panel = viewers(&mut app)[0];
        let handle = edit_text(&mut app, panel, "mine");
        fs::remove_file(&path).unwrap();
        let id = app.world().get::<DocumentView>(panel).unwrap().document();
        mark_document_source_missing(app.world_mut(), id).unwrap();
        app.update();
        assert_eq!(text(&app, panel), "mine");
        let info = inspect_document(app.world(), handle.id()).unwrap();
        assert!(info.dirty);
        assert!(!info.has_save_binding);
        assert_eq!(
            save_document(app.world_mut(), handle.id()),
            Err(DocumentError::NoSaveBinding)
        );
        assert!(preflight_session_close(app.world(), SessionCloseMode::Saved).is_err());
        Workbench::for_entity(app.world_mut(), panel)
            .unwrap()
            .close_panel(panel)
            .unwrap();
        assert_eq!(read_document(app.world(), handle).unwrap(), "mine");
        assert!(inspect_document(app.world(), handle.id()).unwrap().dirty);
        stop(&mut app);
    }

    fn operation(app: &mut App, panel: Entity, root: &Path, change: FileTreeChange) {
        match &change {
            FileTreeChange::Moved {
                source,
                destination,
            } => {
                bedtime_filesystem2::Filesystem::local().move_document_sources(source, destination)
            }
            FileTreeChange::Trashed { path } => {
                for id in document_ids(app.world()) {
                    if document_source_path(app.world(), id).is_some_and(|p| p.starts_with(path)) {
                        let _ = mark_document_source_missing(app.world_mut(), id);
                    }
                }
            }
            _ => {}
        }
        app.world_mut().write_message(FileTreeOperationFinished {
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
        shutdown_work(app.world_mut()).unwrap();
        let error = open_file_panel(app.world_mut(), viewer, source.clone(), false).unwrap_err();
        // Open routing may report this same admission failure before Update
        // collects the viewer's pending notification. Only one modal is needed.
        request_error_dialog(
            app.world_mut(),
            preview_error_dialog(Some(&source), error.clone()),
        );
        app.update();
        assert_eq!(app.world().resource::<ErrorDialogs>().len(), 1);
        assert_eq!(
            app.world().get::<FilePanel>(viewer).unwrap().error.as_ref(),
            Some(&error)
        );
        app.world_mut().remove_resource::<ErrorDialogs>();
        for _ in 0..3 {
            app.update();
        }
        assert!(app.world().get_resource::<ErrorDialogs>().is_none());
        wait(&mut app, |world| work_finished(world));
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
        let (_origin, layout) = workspace(&mut app, "Origin");
        let panel = tree(&mut app, layout, &fixture.0);
        let (other, other_layout) = workspace(&mut app, "Other");
        Workbench::new(app.world_mut())
            .activate_layout(other)
            .unwrap();
        request(&mut app, panel, &first, FileTreeActivation::SingleClick);
        let viewer = viewers(&mut app)[0];
        wait(&mut app, |world| {
            world.get::<FilePanel>(viewer).unwrap().loading == false
        });
        assert_eq!(text(&app, viewer), "actual disk text one");
        assert_eq!(active_layout(app.world()), Some(other));
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
            world.get::<FilePanel>(viewer).unwrap().loading == false
        });
        assert_eq!(viewers(&mut app), vec![viewer]);
        assert_eq!(text(&app, viewer), "actual disk text two");
        request(&mut app, panel, &second, FileTreeActivation::DoubleClick);
        assert!(!app.world().get::<PanelInstance>(viewer).unwrap().preview);
        request(&mut app, panel, &third, FileTreeActivation::SingleClick);
        assert_eq!(viewers(&mut app).len(), 2);
        request(&mut app, panel, &second, FileTreeActivation::Keyboard);
        request(&mut app, panel, &second, FileTreeActivation::ContextMenu);
        assert_eq!(viewers(&mut app).len(), 2);
        let other_panel = tree(&mut app, other_layout, &fixture.0);
        request(
            &mut app,
            other_panel,
            &second,
            FileTreeActivation::SingleClick,
        );
        assert_eq!(
            Workbench::new(app.world_mut())
                .panels(other_layout)
                .iter()
                .filter(|&&p| app.world().get::<FilePanel>(p).is_some())
                .count(),
            1
        );
        assert_eq!(viewers(&mut app).len(), 3);
        stop(&mut app);
    }

    #[test]
    fn preview_slots_are_layout_local_even_in_same_workspace() {
        let fixture = Fixture::new();
        let file = fixture.text("same.txt", "same source");
        let mut app = host();
        let (_workspace, layout) = workspace(&mut app, "Origin");
        let other_layout = app.world_mut().spawn(Layout::default()).id();
        app.world_mut()
            .spawn((Area::default(), InLayout(other_layout)));
        let left = tree(&mut app, layout, &fixture.0);
        let right = tree(&mut app, other_layout, &fixture.0);
        request(&mut app, left, &file, FileTreeActivation::SingleClick);
        request(&mut app, right, &file, FileTreeActivation::SingleClick);
        assert_eq!(viewers(&mut app).len(), 2);
        stop(&mut app);
    }

    #[test]
    fn preview_reads_are_bounded_and_binary_is_metadata_only() {
        let fixture = Fixture::new();
        let binary = fixture.path("binary.bin");
        fs::write(&binary, [0, 255, 128]).unwrap();
        assert!(matches!(
            read_preview(&binary).unwrap(),
            FilePresentation::Metadata(_)
        ));
        let large = fixture.path("large.txt");
        File::create(&large)
            .unwrap()
            .set_len(MAX_PREVIEW_BYTES + 1)
            .unwrap();
        let FilePresentation::Metadata(summary) = read_preview(&large).unwrap() else {
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
                preview_read_error(&missing, kind),
                PreviewReadError::Failed(_)
            ));
        }
    }

    #[test]
    fn stale_preview_completion_cannot_replace_newer_source() {
        let fixture = Fixture::new();
        let old = fixture.text("old.txt", "old result");
        let file = fixture.text("new.txt", "new result");
        let mut app = host();
        let (_, layout) = workspace(&mut app, "Origin");
        let panel = preview_panel(app.world_mut(), layout);
        open_file_panel(app.world_mut(), panel, old, false).unwrap();
        open_file_panel(app.world_mut(), panel, file.clone(), false).unwrap();
        wait(&mut app, |world| {
            world.get::<FilePanel>(panel).unwrap().loading == false
        });
        assert_eq!(text(&app, panel), "new result");
        assert_eq!(panel_path(app.world(), panel), Some(file));
        stop(&mut app);
    }

    #[test]
    fn moved_preview_rebinds_and_confirmed_trash_unloads_sole_tab() {
        let fixture = Fixture::new();
        let source = fixture.text("before.txt", "retained data");
        let destination = fixture.path("after.txt");
        let mut app = host();
        let (_workspace, layout) = workspace(&mut app, "Origin");
        let panel = tree(&mut app, layout, &fixture.0);
        request(&mut app, panel, &source, FileTreeActivation::Keyboard);
        let viewer = viewers(&mut app)[0];
        wait(&mut app, |world| {
            world.get::<FilePanel>(viewer).unwrap().loading == false
        });
        fs::rename(&source, &destination).unwrap();
        operation(
            &mut app,
            panel,
            &fixture.0,
            FileTreeChange::Moved {
                source,
                destination: destination.clone(),
            },
        );
        wait(&mut app, |world| {
            world.get::<FilePanel>(viewer).unwrap().loading == false
        });
        assert_eq!(
            app.world().get::<FilePanel>(viewer).unwrap().path.as_ref(),
            Some(&destination)
        );
        assert_eq!(
            app.world().get::<PanelInstance>(viewer).unwrap().title,
            "after.txt"
        );
        fs::remove_file(&destination).unwrap();
        operation(
            &mut app,
            panel,
            &fixture.0,
            FileTreeChange::Trashed { path: destination },
        );
        let state = app.world().get::<FilePanel>(viewer).unwrap();
        assert!(state.path.is_none());
        assert!(state.content.is_none());
        assert!(state.loading == false);
        assert!(state.missing);
        assert_eq!(
            app.world().get::<PanelInstance>(viewer).unwrap().title,
            FilePanel::LABEL
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
        let (_workspace, layout) = workspace(&mut app, "Project");
        let other_layout = app.world_mut().spawn(Layout::default()).id();
        app.world_mut()
            .spawn((Area::default(), InLayout(other_layout)));
        let first_tree = tree(&mut app, layout, &fixture.0);
        let second_tree = tree(&mut app, other_layout, &fixture.0);
        request(&mut app, first_tree, &source, FileTreeActivation::Keyboard);
        request(&mut app, second_tree, &source, FileTreeActivation::Keyboard);
        let panels = viewers(&mut app);
        wait(&mut app, |world| {
            panels
                .iter()
                .all(|panel| world.get::<FilePanel>(*panel).unwrap().content.is_some())
        });
        let id = app
            .world()
            .get::<DocumentView>(panels[0])
            .unwrap()
            .document();
        refresh_document(app.world_mut(), id).unwrap();
        app.update();
        fs::rename(fixture.path("before"), fixture.path("after")).unwrap();
        operation(
            &mut app,
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
                    .get::<FilePanel>(*panel)
                    .is_some_and(|state| state.loading == false)
            })
        });
        for panel in panels {
            let state = app.world().get::<FilePanel>(panel).unwrap();
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
        let (_origin, layout) = workspace(&mut app, "Origin");
        let panel = tree(&mut app, layout, &fixture.0);
        request(&mut app, panel, &file, FileTreeActivation::Keyboard);
        let viewer = viewers(&mut app)[0];
        wait(&mut app, |world| {
            world.get::<FilePanel>(viewer).unwrap().content.is_some()
        });
        // The nested parent was never expanded. Retargeting the tree removes
        // even its root watch; the preview must own its source observation.
        app.world_mut()
            .entity_mut(panel)
            .insert(FileTreeRoot(other_root.0.clone()));
        let (other, _) = workspace(&mut app, "Foreground");
        Workbench::new(app.world_mut())
            .activate_layout(other)
            .unwrap();
        let generation = app.world().get::<FilePanel>(viewer).unwrap().generation;
        fs::remove_dir_all(fixture.path("never-expanded")).unwrap();
        wait(&mut app, |world| {
            world.get::<FilePanel>(viewer).unwrap().path.is_none()
        });
        let state = app.world().get::<FilePanel>(viewer).unwrap();
        assert!(state.content.is_none());
        assert!(state.loading == false);
        assert!(state.error.is_none());
        assert!(state.missing);
        assert!(state.generation > generation);
        assert_eq!(
            app.world().get::<PanelInstance>(viewer).unwrap().title,
            FilePanel::LABEL
        );
        assert!(!app.world().get::<PanelInstance>(viewer).unwrap().preview);
        assert_eq!(active_layout(app.world()), Some(other));
        stop(&mut app);
    }

    #[test]
    fn external_delete_closes_shared_area_tab_but_unloads_sole_tab_in_other_layout() {
        let fixture = Fixture::new();
        let shared = fixture.text("shared.txt", "deleted content");
        let retained = fixture.text("retained.txt", "surviving content");
        let mut app = host();
        let (_workspace, layout) = workspace(&mut app, "Project");
        let other_layout = app.world_mut().spawn(Layout::default()).id();
        app.world_mut()
            .spawn((Area::default(), InLayout(other_layout)));
        let left_tree = tree(&mut app, layout, &fixture.0);
        let right_tree = tree(&mut app, other_layout, &fixture.0);
        request(&mut app, left_tree, &shared, FileTreeActivation::Keyboard);
        let left = viewers(&mut app)[0];
        request(&mut app, left_tree, &retained, FileTreeActivation::Keyboard);
        let survivor = viewers(&mut app)
            .into_iter()
            .find(|panel| *panel != left)
            .unwrap();
        request(&mut app, right_tree, &shared, FileTreeActivation::Keyboard);
        let right = viewers(&mut app)
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
                    .get::<FilePanel>(*panel)
                    .is_some_and(|state| state.content.is_some())
            })
        });
        fs::remove_file(&shared).unwrap();
        wait(&mut app, |world| {
            world.get::<FilePanel>(left).is_none()
                && world
                    .get::<FilePanel>(right)
                    .is_some_and(|state| state.path.is_none())
        });
        assert!(
            app.world()
                .get::<FilePanel>(right)
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
        let (_workspace, layout) = workspace(&mut app, "Project");
        let tree = tree(&mut app, layout, &fixture.0);
        for file in [&first, &second, &sibling] {
            request(&mut app, tree, file, FileTreeActivation::Keyboard);
        }
        let panels = viewers(&mut app);
        let retained = panels
            .iter()
            .copied()
            .find(|panel| {
                app.world().get::<FilePanel>(*panel).unwrap().path.as_ref() == Some(&sibling)
            })
            .unwrap();
        wait(&mut app, |world| {
            panels
                .iter()
                .all(|panel| world.get::<FilePanel>(*panel).unwrap().content.is_some())
        });
        // Do not invoke OS Trash in tests. Delete the disposable fixture and
        // inject only the confirmed own-operation outcome before polling.
        fs::remove_dir_all(fixture.path("folder")).unwrap();
        operation(
            &mut app,
            tree,
            &fixture.0,
            FileTreeChange::Trashed {
                path: fixture.path("folder"),
            },
        );
        assert_eq!(viewers(&mut app), vec![retained]);
        assert_eq!(text(&app, retained), "sibling");
        stop(&mut app);
    }

    #[test]
    fn late_read_cannot_resurrect_source_deleted_during_initial_loading() {
        let fixture = Fixture::new();
        let source = fixture.text("source.txt", "content");
        let mut app = host();
        let (_, layout) = workspace(&mut app, "Project");
        let panel = preview_panel(app.world_mut(), layout);
        open_file_panel(app.world_mut(), panel, source.clone(), false).unwrap();
        fs::remove_file(&source).unwrap();
        wait(&mut app, |world| {
            world
                .get::<FilePanel>(panel)
                .is_some_and(|s| s.loading == false && (s.missing || s.error.is_some()))
        });
        for _ in 0..4 {
            app.update();
        }
        assert!(app.world().get::<DocumentView>(panel).is_none());
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
        for _ in 0..65 {
            let panel = preview_panel(app.world_mut(), layout);
            open_file_panel(app.world_mut(), panel, retained.clone(), false).unwrap();
            panels.push(panel);
        }
        let last = preview_panel(app.world_mut(), layout);
        open_file_panel(app.world_mut(), last, deleted.clone(), false).unwrap();
        wait(&mut app, |world| {
            panels
                .iter()
                .chain([&last])
                .all(|p| world.get::<DocumentView>(*p).is_some())
        });
        fs::remove_file(deleted).unwrap();
        wait(&mut app, |world| world.get::<FilePanel>(last).is_none());
        for panel in panels {
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
        let source = fixture.text("restricted/source.txt", "retained");
        let parent = source.parent().unwrap().to_owned();
        let mut app = host();
        let (_, layout) = workspace(&mut app, "Project");
        let panel = preview_panel(app.world_mut(), layout);
        open_file_panel(app.world_mut(), panel, source.clone(), false).unwrap();
        wait(&mut app, |world| world.get::<DocumentView>(panel).is_some());
        struct Restore(PathBuf, fs::Permissions);
        impl Drop for Restore {
            fn drop(&mut self) {
                let _ = fs::set_permissions(&self.0, self.1.clone());
            }
        }
        let _restore = Restore(parent.clone(), fs::metadata(&parent).unwrap().permissions());
        fs::set_permissions(&parent, fs::Permissions::from_mode(0)).unwrap();
        if fs::metadata(&source).is_ok() {
            stop(&mut app);
            return;
        }
        let id = app.world().get::<DocumentView>(panel).unwrap().document();
        refresh_document(app.world_mut(), id).unwrap();
        wait(&mut app, |world| {
            inspect_document(world, id)
                .unwrap()
                .last_save_error
                .is_some()
        });
        assert_eq!(text(&app, panel), "retained");
        assert!(!inspect_document(app.world(), id).unwrap().source_missing);
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
        let image = fixture.path("foo.bin");
        fs::write(&image, b"\x89PNG\0binary").unwrap();
        let mut app = host();
        let (_owner, layout) = workspace(&mut app, "Shared route");
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
            app.world().get::<FilePanel>(image.panel).unwrap().content,
            Some(FilePresentation::Metadata(_))
        ));
        assert_eq!(viewers(&mut app).len(), 3);
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
    fn automatic_hex_fallback_reuses_tabs_and_explicit_hex_preserves_dirty_code() {
        use crate::opening::{FileOpenMode, ViewerId, open_file, open_file_with};
        let fixture = Fixture::new();
        let binary = fixture.path("unknown.dat");
        fs::write(&binary, [0, 255, 128]).unwrap();
        let mut app = host();
        let (_owner, layout) = workspace(&mut app, "Hex");
        let opened =
            open_file(app.world_mut(), layout, binary.clone(), FileOpenMode::Focus).unwrap();
        wait(&mut app, |world| opened.result(world).is_some());
        opened.result(app.world()).unwrap().unwrap();
        assert_eq!(
            app.world().get::<FilePanel>(opened.panel).unwrap().viewer,
            ViewerId::Hex
        );
        assert!(app.world().get::<DocumentView>(opened.panel).is_some());
        assert_eq!(
            open_file(app.world_mut(), layout, binary, FileOpenMode::Focus)
                .unwrap()
                .panel,
            opened.panel
        );
        let path = fixture.text("edits.txt", "original");
        let code = open_file(app.world_mut(), layout, path.clone(), FileOpenMode::Focus).unwrap();
        wait(&mut app, |world| code.result(world).is_some());
        edit_text(&mut app, code.panel, "unsaved original");
        let hex = open_file_with(
            app.world_mut(),
            layout,
            path.clone(),
            FileOpenMode::Focus,
            Some(ViewerId::Hex),
            Some(code.panel),
        )
        .unwrap();
        wait(&mut app, |world| hex.result(world).is_some());
        hex.result(app.world()).unwrap().unwrap();
        assert_ne!(code.panel, hex.panel);
        assert_eq!(text(&app, code.panel), "unsaved original");
        assert!(app.world().get::<DocumentView>(hex.panel).is_some());
        assert!(app.world().get::<DocumentView>(hex.panel).is_some());
        stop(&mut app);
    }

    #[test]
    fn corrupt_images_fall_back_only_for_automatic_opening() {
        use crate::opening::{FileOpenMode, ViewerId, open_file, open_file_with};
        let fixture = Fixture::new();
        let path = fixture.text("wrong.png", "this is text");
        let mut app = host();
        let (_, layout) = workspace(&mut app, "Fallback");
        let auto = open_file(app.world_mut(), layout, path.clone(), FileOpenMode::Focus).unwrap();
        wait(&mut app, |world| auto.result(world).is_some());
        auto.result(app.world()).unwrap().unwrap();
        assert_eq!(
            app.world().get::<FilePanel>(auto.panel).unwrap().viewer,
            ViewerId::Code
        );
        assert!(
            app.world()
                .get::<FilePanel>(auto.panel)
                .unwrap()
                .notice
                .is_some()
        );
        let explicit = open_file_with(
            app.world_mut(),
            layout,
            path,
            FileOpenMode::Focus,
            Some(ViewerId::Image),
            Some(auto.panel),
        )
        .unwrap();
        wait(&mut app, |world| explicit.result(world).is_some());
        assert!(explicit.result(app.world()).unwrap().is_err());
        assert_eq!(
            app.world().get::<FilePanel>(explicit.panel).unwrap().viewer,
            ViewerId::Image
        );
        stop(&mut app);
    }

    #[test]
    fn image_associations_and_explicit_viewers_preserve_both_tabs() {
        use crate::opening::{FileOpenMode, ViewerId, open_file, open_file_with};
        let fixture = Fixture::new();
        let path = fixture.path("picture.PNG");
        fs::write(
            &path,
            include_bytes!("../../bedtime/tests/fixtures/images/sample.png"),
        )
        .unwrap();
        assert_eq!(
            crate::opening::viewers(&path),
            vec![ViewerId::Image, ViewerId::Code, ViewerId::Hex]
        );
        let mut app = host();
        let (_, layout) = workspace(&mut app, "Images");
        let image =
            open_file(app.world_mut(), layout, path.clone(), FileOpenMode::Preview).unwrap();
        wait(&mut app, |world| image.result(world).is_some());
        image.result(app.world()).unwrap().unwrap();
        assert!(app.world().get::<DocumentView>(image.panel).is_some());
        let code = open_file_with(
            app.world_mut(),
            layout,
            path.clone(),
            FileOpenMode::Focus,
            Some(ViewerId::Code),
            Some(image.panel),
        )
        .unwrap();
        wait(&mut app, |world| code.result(world).is_some());
        assert_ne!(image.panel, code.panel);
        assert!(
            app.world()
                .get::<PanelInstance>(image.panel)
                .unwrap()
                .preview
        );
        assert!(
            !app.world()
                .get::<PanelInstance>(code.panel)
                .unwrap()
                .preview
        );
        let area = app.world().get::<InArea>(image.panel).unwrap().0;
        assert_eq!(
            panels_in_area(app.world(), area),
            &[image.panel, code.panel]
        );
        assert_eq!(
            open_file(app.world_mut(), layout, path, FileOpenMode::Focus)
                .unwrap()
                .panel,
            image.panel
        );
        stop(&mut app);
    }

    #[test]
    fn svg_image_and_code_share_unsaved_edits_and_invalid_source_retains_pixels() {
        use crate::opening::{FileOpenMode, ViewerId, open_file, open_file_with};
        let fixture = Fixture::new();
        let path = fixture.text("live.svg", r#"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="2"><rect width="4" height="2" fill="red"/></svg>"#);
        let mut app = host();
        let (_, layout) = workspace(&mut app, "SVG");
        let image =
            open_file(app.world_mut(), layout, path.clone(), FileOpenMode::Preview).unwrap();
        wait(&mut app, |world| image.result(world).is_some());
        image.result(app.world()).unwrap().unwrap();
        assert!(
            app.world()
                .get::<bedtime_editor2::workbench::WorkbenchEditor>(image.panel)
                .is_none()
        );
        let code = open_file_with(
            app.world_mut(),
            layout,
            path.clone(),
            FileOpenMode::Focus,
            Some(ViewerId::Code),
            Some(image.panel),
        )
        .unwrap();
        wait(&mut app, |world| code.result(world).is_some());
        assert_eq!(
            app.world()
                .get::<DocumentView>(image.panel)
                .unwrap()
                .document(),
            app.world()
                .get::<DocumentView>(code.panel)
                .unwrap()
                .document()
        );
        app.world_mut()
            .get_mut::<PanelPresentation>(image.panel)
            .unwrap()
            .visible = true;
        let changed = r#"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="3"/>"#;
        edit_text(&mut app, code.panel, changed);
        wait(&mut app, |world| {
            world
                .get::<crate::images::ImagePreview>(image.panel)
                .unwrap()
                .view
                .dimensions()
                == Some(egui::vec2(8.0, 3.0))
        });
        assert!(!fs::read_to_string(&path).unwrap().contains("width=\"8\""));
        edit_text(&mut app, code.panel, "<svg");
        wait(&mut app, |world| {
            world
                .get::<crate::images::ImagePreview>(image.panel)
                .unwrap()
                .error
                .is_some()
        });
        assert_eq!(
            app.world()
                .get::<crate::images::ImagePreview>(image.panel)
                .unwrap()
                .view
                .dimensions(),
            Some(egui::vec2(8.0, 3.0))
        );
        edit_text(&mut app, code.panel, changed);
        wait(&mut app, |world| {
            world
                .get::<crate::images::ImagePreview>(image.panel)
                .unwrap()
                .error
                .is_none()
        });
        let next = fixture.text("next.txt", "next");
        let next = open_file(app.world_mut(), layout, next, FileOpenMode::Preview).unwrap();
        assert_ne!(
            next.panel, image.panel,
            "A dirty shared SVG preview must be protected"
        );
        stop(&mut app);
    }

    #[test]
    fn replaceable_preview_switches_viewer_and_dropped_images_use_image_canvas() {
        use crate::opening::{FileOpenMode, ViewerId, open_file};
        let fixture = Fixture::new();
        let text_path = fixture.text("file.txt", "text");
        let svg = fixture.text(
            "picture.svg",
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="2"/>"#,
        );
        let mut app = host();
        let (_, layout) = workspace(&mut app, "Preview");
        let text_open = open_file(
            app.world_mut(),
            layout,
            text_path.clone(),
            FileOpenMode::Preview,
        )
        .unwrap();
        wait(&mut app, |world| text_open.result(world).is_some());
        let image = open_file(app.world_mut(), layout, svg.clone(), FileOpenMode::Preview).unwrap();
        assert_eq!(image.panel, text_open.panel);
        wait(&mut app, |world| image.result(world).is_some());
        image.result(app.world()).unwrap().unwrap();
        let area = app.world().get::<InArea>(image.panel).unwrap().0;
        let factory = crate::opening::file_panel_factory(svg);
        let dropped = (factory.create)(app.world_mut(), area);
        assert_eq!(
            app.world().get::<FilePanel>(dropped).unwrap().viewer,
            ViewerId::Image
        );
        let reopened =
            open_file(app.world_mut(), layout, text_path, FileOpenMode::Preview).unwrap();
        assert_eq!(reopened.panel, image.panel);
        assert!(
            app.world()
                .get::<crate::images::ImagePreview>(reopened.panel)
                .is_none()
        );
        wait(&mut app, |world| reopened.result(world).is_some());
        assert_eq!(text(&app, reopened.panel), "text");
        stop(&mut app);
    }

    #[test]
    fn image_disk_changes_refresh_and_invalid_image_falls_back_to_text() {
        use crate::opening::{FileOpenMode, open_file};
        let fixture = Fixture::new();
        let path = fixture.text(
            "disk.svg",
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="2"/>"#,
        );
        let mut app = host();
        let (_, layout) = workspace(&mut app, "Disk");
        let image = open_file(app.world_mut(), layout, path.clone(), FileOpenMode::Focus).unwrap();
        wait(&mut app, |world| image.result(world).is_some());
        app.world_mut()
            .get_mut::<PanelPresentation>(image.panel)
            .unwrap()
            .visible = true;
        fs::write(
            &path,
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20"/>"#,
        )
        .unwrap();
        wait(&mut app, |world| {
            world
                .get::<crate::images::ImagePreview>(image.panel)
                .unwrap()
                .view
                .dimensions()
                == Some(egui::vec2(40.0, 20.0))
        });
        let invalid = fixture.text("invalid.png", "not an image");
        let invalid = open_file(app.world_mut(), layout, invalid, FileOpenMode::Focus).unwrap();
        wait(&mut app, |world| invalid.result(world).is_some());
        invalid.result(app.world()).unwrap().unwrap();
        assert_eq!(
            app.world().get::<FilePanel>(invalid.panel).unwrap().viewer,
            crate::opening::ViewerId::Code
        );
        assert_eq!(text(&app, invalid.panel), "not an image");
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
    #[test]
    fn model_associations_canonical_edits_and_session_state_follow_file_policy() {
        use crate::opening::{FileOpenMode, ViewerId, open_file, open_file_with};
        let fixture = Fixture::new();
        let path = fixture.text(
            "live.gltf",
            include_str!("../../bedtime/tests/fixtures/gltf/cube.gltf"),
        );
        let mut app = host();
        let (_owner, layout) = workspace(&mut app, "Models");
        assert_eq!(
            crate::opening::viewers(&path),
            vec![ViewerId::Model, ViewerId::Code, ViewerId::Hex]
        );
        assert_eq!(
            crate::opening::default_viewer(Path::new("scene.GLB")),
            ViewerId::Model
        );
        assert_eq!(
            crate::opening::default_viewer(Path::new("scene.stl")),
            ViewerId::Model
        );
        let model =
            open_file(app.world_mut(), layout, path.clone(), FileOpenMode::Preview).unwrap();
        wait(&mut app, |world| {
            world.get::<DocumentView>(model.panel).is_some()
                && world
                    .get::<bedtime_model2::ModelScene>(model.panel)
                    .is_some_and(|scene| scene.is_loading())
        });
        assert!(
            app.world()
                .get::<bedtime_editor2::workbench::WorkbenchEditor>(model.panel)
                .is_none()
        );
        let code = open_file_with(
            app.world_mut(),
            layout,
            path.clone(),
            FileOpenMode::Focus,
            Some(ViewerId::Code),
            Some(model.panel),
        )
        .unwrap();
        wait(&mut app, |world| code.result(world).is_some());
        assert_eq!(
            app.world()
                .get::<DocumentView>(model.panel)
                .unwrap()
                .document(),
            app.world()
                .get::<DocumentView>(code.panel)
                .unwrap()
                .document()
        );
        app.world_mut()
            .get_mut::<PanelPresentation>(model.panel)
            .unwrap()
            .visible = true;
        edit_text(&mut app, code.panel, "invalid gltf");
        wait(&mut app, |world| {
            world
                .get::<crate::models::ModelPreview>(model.panel)
                .unwrap()
                .error
                .is_some()
        });
        assert_ne!(fs::read_to_string(&path).unwrap(), "invalid gltf");
        assert!(!crate::file_editor::can_replace(app.world(), model.panel));
        let another = fixture.text("another.txt", "another source");
        let other = open_file(app.world_mut(), layout, another, FileOpenMode::Preview).unwrap();
        assert_ne!(other.panel, model.panel);
        let mut view = bedtime_model2::ViewState::default();
        view.appearance.exposure = 1.25;
        app.world_mut()
            .get_mut::<bedtime_model2::ModelScene>(model.panel)
            .unwrap()
            .view = view;
        let mut captured =
            Workbench::capture_world::<ReflectWorkbenchPersist>(app.world_mut()).unwrap();
        project_persistence(app.world_mut(), &mut captured);
        let saved = captured.component::<PersistedFile>(model.panel).unwrap();
        assert_eq!(saved.model, Some(view));
        assert!(app.world().get::<PersistedFile>(model.panel).is_none());
        stop(&mut app);
    }
}

use bedtime_workbench2::persistence::{CapturedWorld, ReflectWorkbenchPersist};
#[derive(Component, Reflect, Clone, serde::Serialize, serde::Deserialize)]
#[reflect(opaque)]
#[reflect(Component, Serialize, Deserialize, WorkbenchPersist)]
#[require(FilePanel)]
#[serde(deny_unknown_fields)]
pub(crate) struct PersistedFile {
    #[serde(with = "bedtime_workbench2::native_path::option")]
    path: Option<PathBuf>,
    viewer: String,
    explicit: bool,
    model: Option<bedtime_model2::ViewState>,
}
#[derive(Component, Reflect, Clone, serde::Serialize, serde::Deserialize)]
#[reflect(opaque)]
#[reflect(Component, Serialize, Deserialize, WorkbenchPersist)]
#[require(FileTreePanel)]
#[serde(deny_unknown_fields)]
pub(crate) struct PersistedTree {
    #[serde(with = "bedtime_workbench2::native_path::option")]
    pub(crate) root: Option<PathBuf>,
}
#[derive(Resource, Reflect, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[reflect(opaque)]
#[reflect(Resource, Serialize, Deserialize, ProjectPersist)]
pub(crate) struct PersistedVisibility(pub bedtime_filetree2::FileTreePreferences);
pub(crate) fn register_persistence(app: &mut App) {
    app.register_type::<PersistedFile>()
        .register_type::<PersistedTree>()
        .register_type::<PersistedVisibility>();
}
pub(crate) fn project_persistence(
    world: &mut World,
    captured: &mut bedtime_workbench2::persistence::CapturedWorld,
) {
    let files: Vec<_> = world
        .query::<(Entity, &FilePanel)>()
        .iter(world)
        .map(|(e, p)| {
            (
                e,
                PersistedFile {
                    path: p.path.clone(),
                    viewer: p.viewer.id().into(),
                    explicit: p.explicit,
                    model: world.get::<bedtime_model2::ModelScene>(e).map(|s| s.view),
                },
            )
        })
        .collect();
    for (e, p) in files {
        captured.insert_component(e, p);
    }
    let trees: Vec<_> = world
        .query_filtered::<(Entity, Option<&FileTreeRoot>), With<FileTreePanel>>()
        .iter(world)
        .map(|(e, r)| {
            (
                e,
                PersistedTree {
                    root: r.map(|r| r.0.clone()),
                },
            )
        })
        .collect();
    for (e, p) in trees {
        captured.insert_component(e, p);
    }
}
pub(crate) fn clear_persistence(world: &mut World) {
    let entities: Vec<_> = world
        .query_filtered::<Entity, Or<(With<PersistedFile>, With<PersistedTree>)>>()
        .iter(world)
        .collect();
    for e in entities {
        world
            .entity_mut(e)
            .remove::<(PersistedFile, PersistedTree)>();
    }
}
pub(crate) fn validate_persistence(prepared: &CapturedWorld) -> Result<(), String> {
    for (_, p) in prepared.components::<PersistedFile>() {
        if crate::opening::ViewerId::from_id(&p.viewer).is_none() {
            return Err("Unknown persisted viewer".into());
        }
        if p.path
            .as_ref()
            .is_some_and(|p| crate::persistence::validate_absolute_path(p).is_err())
        {
            return Err("Invalid file source descriptor".into());
        }
        if p.model.is_some_and(|view| view.validate().is_err()) {
            return Err("Invalid model view".into());
        }
    }
    for (_, p) in prepared.components::<PersistedTree>() {
        if p.root
            .as_ref()
            .is_some_and(|p| crate::persistence::validate_absolute_path(p).is_err())
        {
            return Err("Invalid Files root descriptor".into());
        }
    }
    Ok(())
}
pub(crate) fn activate_persistence(world: &mut World) {
    let entities: Vec<_> = world
        .query_filtered::<Entity, With<PersistedTree>>()
        .iter(world)
        .collect();
    for e in entities {
        let p = world.entity_mut(e).take::<PersistedTree>().unwrap();
        if let Some(root) = p
            .root
            .or_else(|| world.get_resource::<ProjectRoot>().map(|r| r.0.clone()))
        {
            world.entity_mut(e).insert(FileTreeRoot(root));
        }
    }
    let entities: Vec<_> = world
        .query_filtered::<Entity, With<PersistedFile>>()
        .iter(world)
        .collect();
    for e in entities {
        let p = world.entity_mut(e).take::<PersistedFile>().unwrap();
        let metadata = world
            .get::<PanelInstance>(e)
            .map(|p| (p.title.clone(), p.preview));
        let viewer = crate::opening::ViewerId::from_id(&p.viewer).unwrap();
        let _ = set_viewer(world, e, viewer);
        world.get_mut::<FilePanel>(e).unwrap().explicit = p.explicit;
        if let Some(path) = p.path {
            if let Err(error) = crate::opening::restore_file_panel(world, e, path) {
                request_error_dialog(
                    world,
                    ErrorDialog {
                        title: "Restore file".into(),
                        message: "The session was restored, but a file could not be reopened."
                            .into(),
                        details: Some(error),
                    },
                );
            }
        }
        if let Some(view) = p.model {
            if let Some(mut model) = world.get_mut::<bedtime_model2::ModelScene>(e) {
                model.view = view;
            }
        }
        if let Some((title, preview)) = metadata {
            let mut panel = world.get_mut::<PanelInstance>(e).unwrap();
            panel.title = title;
            panel.preview = preview;
        }
    }
}

/// Global layout defaults must not carry project source identity.
pub(crate) fn validate_layout_default(
    capture: &bedtime_workbench2::persistence::CapturedLayout,
) -> Result<(), String> {
    if !capture.components::<PersistedFile>().is_empty()
        || capture
            .components::<PersistedTree>()
            .iter()
            .any(|(_, tree)| tree.root.is_some())
        || !capture
            .components::<crate::layouts::LayoutSlot>()
            .is_empty()
    {
        return Err(
            "Layout defaults cannot contain file viewers, project paths or numbered slots".into(),
        );
    }
    Ok(())
}

pub(crate) fn register_menu(app: &mut App) {
    crate::menus::register_panel::<bedtime_filetree2::FileTreePanel>(
        app,
        Some(20),
        bedtime_workbench2::PanelFactory::new("Files", |world, area| {
            let panel = world
                .spawn((
                    bedtime_filetree2::FileTreePanel::default(),
                    bedtime_workbench2::InArea(area),
                ))
                .id();
            if let Some(root) = world.get_resource::<ProjectRoot>().map(|r| r.0.clone()) {
                world
                    .entity_mut(panel)
                    .insert(bedtime_filetree2::FileTreeRoot(root));
            }
            panel
        }),
        crate::storage::ready,
    );
}
