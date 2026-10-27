//! Application-owned opening policy. Producers supply intent; this module
//! chooses viewers, tab reuse and destination areas independently of Files UI.
use crate::files::{FilePanel, ProjectRoot};
use bedtime_filetree2::FileTreePanel;
use bedtime_workbench2::*;
use bevy::prelude::*;
use std::{
    path::PathBuf,
    sync::{Mutex, mpsc},
};

#[derive(Clone, Copy)]
pub enum FileOpenMode {
    Preview,
    KeepOpen,
    Focus,
}

/// Stable application viewer identities, independent of panel type discovery.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum ViewerId {
    #[default]
    Code,
    Image,
    Model,
    Hex,
}

impl ViewerId {
    pub fn id(self) -> &'static str {
        match self {
            Self::Code => "code",
            Self::Image => "image",
            Self::Model => "model",
            Self::Hex => "hex",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Code => "Code",
            Self::Image => "Image",
            Self::Model => "Model",
            Self::Hex => "Hex",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "code" => Some(Self::Code),
            "image" => Some(Self::Image),
            "model" => Some(Self::Model),
            "hex" => Some(Self::Hex),
            _ => None,
        }
    }
}

struct ViewerAssociation {
    viewer: ViewerId,
    extensions: &'static [&'static str],
    priority: i32,
}

const ASSOCIATIONS: &[ViewerAssociation] = &[
    ViewerAssociation {
        viewer: ViewerId::Model,
        extensions: bedtime_model2::SUPPORTED_EXTENSIONS,
        priority: 100,
    },
    ViewerAssociation {
        viewer: ViewerId::Hex,
        extensions: &[],
        priority: -100,
    },
    ViewerAssociation {
        viewer: ViewerId::Image,
        extensions: bedtime_image2::SUPPORTED_EXTENSIONS,
        priority: 100,
    },
    ViewerAssociation {
        viewer: ViewerId::Code,
        extensions: &[],
        priority: 0,
    },
];

pub fn viewers(path: &std::path::Path) -> Vec<ViewerId> {
    let extension = path.extension().and_then(|e| e.to_str());
    let mut candidates: Vec<_> = ASSOCIATIONS
        .iter()
        .filter(|rule| {
            rule.extensions.is_empty()
                || extension.is_some_and(|ext| {
                    rule.extensions
                        .iter()
                        .any(|candidate| ext.eq_ignore_ascii_case(candidate))
                })
        })
        .collect();
    candidates.sort_by_key(|rule| (std::cmp::Reverse(rule.priority), rule.viewer));
    candidates.into_iter().map(|rule| rule.viewer).collect()
}

pub fn default_viewer(path: &std::path::Path) -> ViewerId {
    viewers(path)[0]
}

pub fn tree_viewers(path: &std::path::Path) -> Vec<bedtime_filetree2::FileTreeOpenChoice> {
    let choices = viewers(path);
    if choices.len() < 2 {
        return Vec::new();
    }
    choices
        .into_iter()
        .enumerate()
        .map(|(i, viewer)| bedtime_filetree2::FileTreeOpenChoice {
            id: viewer.id().into(),
            label: format!(
                "{}{}",
                viewer.label(),
                if i == 0 { " (default)" } else { "" }
            ),
        })
        .collect()
}

/// A particular viewer load, so a later rebind cannot complete an older open.
pub struct OpenedFile {
    pub panel: Entity,
    generation: u64,
}

impl OpenedFile {
    pub fn result(&self, world: &World) -> Option<Result<(), String>> {
        crate::files::file_open_result(world, self.panel, self.generation)
    }
}

/// One route for file-opening intentions from Files, terminal helpers and later
/// producers. Source-specific validation belongs at the producer's boundary.
pub fn open_file(
    world: &mut World,
    layout: Entity,
    path: PathBuf,
    mode: FileOpenMode,
) -> Result<OpenedFile, String> {
    open_file_with(world, layout, path, mode, None, None)
}

pub fn open_file_with(
    world: &mut World,
    layout: Entity,
    path: PathBuf,
    mode: FileOpenMode,
    requested: Option<ViewerId>,
    origin: Option<Entity>,
) -> Result<OpenedFile, String> {
    open_file_impl(world, layout, path, mode, requested, origin, None)
}
pub fn open_source_file(
    world: &mut World,
    layout: Entity,
    source: bedtime_filesystem2::FsDocumentSource,
    mode: FileOpenMode,
    requested: Option<ViewerId>,
    origin: Option<Entity>,
) -> Result<OpenedFile, String> {
    open_file_impl(
        world,
        layout,
        source.path(),
        mode,
        requested,
        origin,
        Some(source),
    )
}
fn open_file_impl(
    world: &mut World,
    layout: Entity,
    path: PathBuf,
    mode: FileOpenMode,
    requested: Option<ViewerId>,
    origin: Option<Entity>,
    source: Option<bedtime_filesystem2::FsDocumentSource>,
) -> Result<OpenedFile, String> {
    let viewer = requested.unwrap_or_else(|| default_viewer(&path));
    if !viewers(&path).contains(&viewer) {
        return Err("This viewer does not support the file".into());
    }
    if !crate::close::accepts_user_actions(world) {
        return Err("Cancel quit before opening a file".into());
    }
    if !path.is_absolute() {
        return Err("File source path must be absolute".into());
    }
    let mut workbench = Workbench::for_entity(world, layout)?;
    if !crate::storage::ready(workbench.world()) {
        return Err("The workspace is still loading or has a settings error".into());
    }
    let panels = workbench.panels(layout);
    let same = panels.iter().copied().find(|&panel| {
        crate::files::panel_path(workbench.world(), panel).as_ref() == Some(&path)
            && workbench
                .world()
                .get::<FilePanel>(panel)
                .is_some_and(|p| p.viewer == viewer || (requested.is_none() && !p.explicit))
    });
    let pin = !matches!(mode, FileOpenMode::Preview);
    let panel = if let Some(panel) = same {
        if requested.is_some() {
            workbench
                .world_mut()
                .get_mut::<FilePanel>(panel)
                .unwrap()
                .explicit = true;
        }
        if pin {
            workbench
                .world_mut()
                .get_mut::<PanelInstance>(panel)
                .unwrap()
                .preview = false;
        }
        crate::files::select_structure_source(workbench.world_mut(), panel);
        panel
    } else {
        let replace = panels.iter().copied().find(|&panel| {
            requested.is_none()
                && workbench.world().get::<FilePanel>(panel).is_some()
                && crate::file_editor::can_replace(workbench.world(), panel)
                && (crate::files::panel_path(workbench.world(), panel).is_none()
                    || workbench
                        .world()
                        .get::<PanelInstance>(panel)
                        .is_some_and(|p| p.preview))
        });
        let panel = if let Some(panel) = replace {
            panel
        } else {
            // Prefer an existing viewer area; otherwise use the largest area
            // without Files. Keep Files visible beside its previews by splitting
            // only when every area contains a tree. The same rules serve shells.
            let areas = workbench
                .world()
                .get::<LayoutAreas>(layout)
                .ok_or("Layout has no areas")?
                .entities()
                .to_vec();
            let area = origin
                .and_then(|panel| workbench.world().get::<InArea>(panel).map(|a| a.0))
                .or_else(|| {
                    panels
                        .iter()
                        .find_map(|&panel| {
                            workbench
                                .world()
                                .get::<FilePanel>(panel)
                                .and_then(|_| workbench.world().get::<InArea>(panel).map(|a| a.0))
                        })
                        .or_else(|| {
                            areas
                                .iter()
                                .copied()
                                .filter(|&area| {
                                    panels_in_area(workbench.world(), area)
                                        .iter()
                                        .all(|&panel| {
                                            workbench.world().get::<FileTreePanel>(panel).is_none()
                                        })
                                })
                                .max_by_key(|&area| {
                                    let rect = workbench.world().get::<Area>(area).unwrap().rect;
                                    (rect.max[0] - rect.min[0]) * (rect.max[1] - rect.min[1])
                                })
                        })
                });
            let area = match area {
                Some(area) => area,
                None => {
                    let source = workbench.destination_area(layout)?;
                    let bounds = workbench.world().get::<Area>(source).unwrap().rect;
                    let at = bounds.min[0] + (bounds.max[0] - bounds.min[0]) * 2 / 5;
                    workbench.split_area(source, SplitAxis::Horizontal, at, [1, 1])?
                }
            };
            workbench
                .world_mut()
                .spawn((FilePanel::default(), InArea(area)))
                .id()
        };
        workbench
            .world_mut()
            .get_mut::<PanelInstance>(panel)
            .unwrap()
            .preview = !pin;
        crate::files::set_viewer(workbench.world_mut(), panel, viewer)?;
        workbench
            .world_mut()
            .get_mut::<FilePanel>(panel)
            .unwrap()
            .explicit = requested.is_some();
        match source {
            Some(source) => {
                crate::files::open_resolved_file_panel(workbench.world_mut(), panel, source, false)?
            }
            None => crate::files::open_file_panel(workbench.world_mut(), panel, path, false)?,
        };
        if let Some(origin) = origin {
            let area = workbench.world().get::<InArea>(origin).unwrap().0;
            let index = panels_in_area(workbench.world(), area)
                .iter()
                .position(|&p| p == origin)
                .unwrap()
                + 1;
            workbench.move_tab(panel, area, index)?;
        }
        panel
    };
    workbench.select_panel(panel)?;
    if matches!(mode, FileOpenMode::Focus) {
        let area = workbench.world().get::<InArea>(panel).unwrap().0;
        workbench.focus_area(area)?;
    }
    Ok(OpenedFile {
        panel,
        generation: crate::files::file_open_generation(workbench.world(), panel),
    })
}

/// A dropped file always gets a pinned viewer at the explicit destination.
/// Loading uses the same canonical documents and async backend as ordinary opens.
pub fn file_panel_factory(path: PathBuf) -> PanelFactory {
    PanelFactory::new("File", move |world, area| {
        let panel = world.spawn((FilePanel::default(), InArea(area))).id();
        // A fresh viewer has no drafts or pending edits. The loading backend
        // records task-admission and read failures on the viewer for reporting.
        let _ = crate::files::set_viewer(world, panel, default_viewer(&path))
            .and_then(|_| restore_file_panel(world, panel, path.clone()));
        panel
    })
}

/// Session restore retains its recorded placement/viewer identity, while source
/// loading uses the same viewer backend as newly routed file opens.
pub fn restore_file_panel(world: &mut World, panel: Entity, path: PathBuf) -> Result<(), String> {
    if !world.get::<FilePanel>(panel).unwrap().explicit {
        crate::files::set_viewer(world, panel, default_viewer(&path))?;
    }
    crate::files::open_file_panel(world, panel, path, false)
}

pub fn viewer_menu(ui: &mut bedtime_ui2::egui::Ui, world: &mut World, panel: Entity) {
    let Some(path) = crate::files::panel_path(world, panel) else {
        return;
    };
    let choices = viewers(&path);
    if choices.len() < 2 {
        return;
    }
    ui.menu_button("Open With", |ui| {
        for (i, viewer) in choices.iter().copied().enumerate() {
            let label = format!(
                "{}{}",
                viewer.label(),
                if i == 0 { " (default)" } else { "" }
            );
            if ui.button(label).clicked() {
                let result = Workbench::for_entity(world, panel)
                    .and_then(|wb| wb.panel_layout(panel))
                    .and_then(|layout| {
                        open_file_with(
                            world,
                            layout,
                            path.clone(),
                            FileOpenMode::Focus,
                            Some(viewer),
                            Some(panel),
                        )
                    });
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
                ui.close();
            }
        }
    });
}

#[derive(Clone, Debug)]
pub enum FolderOpened {
    Reused,
    Launched(
        #[cfg_attr(
            not(feature = "automation"),
            expect(
                dead_code,
                reason = "Only optional automation inspects the child PID; shell helpers acknowledge launch"
            )
        )]
        u32,
    ),
}

pub type FolderOpenResult = Mutex<mpsc::Receiver<Result<FolderOpened, String>>>;

struct FolderOpen {
    validation: std::thread::JoinHandle<Result<PathBuf, String>>,
    reply: mpsc::SyncSender<Result<FolderOpened, String>>,
}

#[derive(Resource, Default)]
pub(crate) struct FolderOpens(Vec<FolderOpen>);

/// Reuse the current project, or attempt a new instance and request normal quit.
/// Validation/canonicalization happens off thread. A launch result does not imply
/// child readiness; even a launch failure does not veto the quit request.
pub fn open_folder(world: &mut World, path: PathBuf) -> Result<FolderOpenResult, String> {
    if !crate::close::accepts_user_actions(world) {
        return Err("Cancel quit before opening another folder".into());
    }
    let mut requests = world
        .get_resource_mut::<FolderOpens>()
        .ok_or("File opening is unavailable")?;
    let (reply, result) = mpsc::sync_channel(1);
    requests.0.push(FolderOpen {
        validation: std::thread::spawn(move || crate::files::resolve_root(Some(path))),
        reply,
    });
    Ok(Mutex::new(result))
}

pub(crate) fn poll_folder_opens(world: &mut World) {
    if !crate::close::accepts_user_actions(world) {
        return;
    }
    let completed = {
        let mut requests = world.resource_mut::<FolderOpens>();
        let mut completed = Vec::new();
        let mut index = 0;
        while index < requests.0.len() {
            if requests.0[index].validation.is_finished() {
                let request = requests.0.remove(index);
                completed.push((
                    request
                        .validation
                        .join()
                        .unwrap_or_else(|_| Err("Folder validation worker panicked".into())),
                    request.reply,
                ));
            } else {
                index += 1;
            }
        }
        completed
    };
    for (result, reply) in completed {
        let result = result.and_then(|root| {
            if world
                .get_resource::<ProjectRoot>()
                .is_some_and(|current| current.0 == root)
                && world.contains_resource::<crate::files::ProjectFolder>()
            {
                return Ok(FolderOpened::Reused);
            }
            let launch = (|| {
                // The child independently claims admission too, resolving races.
                let claim = crate::project::ProjectLock::acquire(&root)?;
                drop(claim);
                let executable = std::env::current_exe().map_err(|e| e.to_string())?;
                let mut child = std::process::Command::new(executable)
                    .arg(root)
                    .arg("--no-automation")
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .spawn()
                    .map_err(|e| e.to_string())?;
                let pid = child.id();
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
                Ok(FolderOpened::Launched(pid))
            })();
            // Replacement is deliberately best-effort. Finish this frame's
            // folder/shell replies before CloseSystems handles normal quit;
            // do not wait for the child or let a launch error veto closing.
            world.write_message(CloseRequested { window: None });
            launch
        });
        if let Err(error) = &result {
            request_error_dialog(
                world,
                ErrorDialog {
                    title: "Open Folder".into(),
                    message: "This folder couldn't be opened.".into(),
                    details: Some(error.clone()),
                },
            );
        }
        let _ = reply.send(result);
    }
}

#[cfg(feature = "automation")]
pub(crate) fn automation_open(
    world: &mut World,
    args: serde_json::Value,
) -> bedtime_automation2::AutomationResult {
    let layout = args
        .get("layout")
        .and_then(|v| v.as_str())
        .ok_or("layout must be an entity ID")?
        .parse::<u64>()
        .map(Entity::from_bits)
        .map_err(|_| "Invalid layout ID")?;
    let path = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or("path must be a string")?;
    let requested = args
        .get("viewer")
        .map(|v| {
            v.as_str()
                .and_then(ViewerId::from_id)
                .ok_or("Unknown viewer")
        })
        .transpose()?;
    let opened = open_file_with(
        world,
        layout,
        PathBuf::from(path),
        FileOpenMode::Focus,
        requested,
        None,
    )?;
    Ok(serde_json::json!({"panel":opened.panel.to_bits().to_string()}))
}
