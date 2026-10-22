//! Application-owned opening policy. Producers supply intent; this module
//! chooses viewers, tab reuse and destination areas independently of Files UI.
use crate::files::{FilePreviewPanel, FolderWorkspace, WorkspaceRoot};
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
    if !crate::close::accepts_user_actions(world) {
        return Err("Cancel quit before opening a file".into());
    }
    if !path.is_absolute() {
        return Err("File source path must be absolute".into());
    }
    let mut workbench = Workbench::for_entity(world, layout)?;
    if !crate::storage::ready(workbench.world(), workbench.workspace_id().entity()) {
        return Err("The workspace is still loading or has a settings error".into());
    }
    let panels = workbench.panels(layout);
    // Viewer selection currently has one implementation: text with a metadata
    // fallback for unsupported content. Add file-kind routing here when another
    // viewer exists; producers must not choose a concrete viewer themselves.
    let same = panels
        .iter()
        .copied()
        .find(|&panel| crate::files::panel_path(workbench.world(), panel).as_ref() == Some(&path));
    let pin = !matches!(mode, FileOpenMode::Preview);
    let panel = if let Some(panel) = same {
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
            workbench.world().get::<FilePreviewPanel>(panel).is_some()
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
            let area = panels
                .iter()
                .find_map(|&panel| {
                    workbench
                        .world()
                        .get::<FilePreviewPanel>(panel)
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
                .spawn((FilePreviewPanel::default(), InArea(area)))
                .id()
        };
        workbench
            .world_mut()
            .get_mut::<PanelInstance>(panel)
            .unwrap()
            .preview = !pin;
        crate::files::load_preview(workbench.world_mut(), panel, path, false)?;
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

/// Session restore retains its recorded placement/viewer identity, while source
/// loading uses the same viewer backend as newly routed file opens.
pub fn restore_file_panel(world: &mut World, panel: Entity, path: PathBuf) -> Result<(), String> {
    crate::files::load_preview(world, panel, path, false)
}

pub type FolderOpenResult = Mutex<mpsc::Receiver<Result<Entity, String>>>;

struct FolderOpen {
    validation: std::thread::JoinHandle<Result<PathBuf, String>>,
    reply: mpsc::SyncSender<Result<Entity, String>>,
}

#[derive(Resource, Default)]
pub(crate) struct FolderOpens(Vec<FolderOpen>);

/// Open or reuse a folder workspace. Validation/canonicalization happens off
/// thread; the result acknowledges activation, followed by normal session restore.
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
            let existing = world.query_filtered::<(Entity, &WorkspaceRoot), (With<Workspace>, With<FolderWorkspace>)>()
                .iter(world).find_map(|(workspace, current)| (current.0 == root).then_some(workspace));
            if let Some(existing) = existing {
                let id = Workbench::for_entity(world, existing)?.workspace_id();
                Workspaces::new(world).activate(id)?;
                return Ok(existing);
            }
            let name = root.file_name().unwrap_or(root.as_os_str()).to_string_lossy().into_owned();
            let workspace = Workspaces::new(world).open(name);
            world.entity_mut(workspace.entity()).insert((WorkspaceRoot(root), FolderWorkspace));
            Workspaces::new(world).activate(workspace)?;
            crate::session::remember_initial_state(world, workspace.entity())?;
            Ok(workspace.entity())
        });
        if let Err(error) = &result {
            request_error_dialog(
                world,
                ErrorDialog {
                    title: "Open Folder".into(),
                    message:
                        "This folder couldn't be opened. The current workspace was left unchanged."
                            .into(),
                    details: Some(error.clone()),
                },
            );
        }
        let _ = reply.send(result);
    }
}
