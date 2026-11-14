//! File operations from the tree: creating, renaming, copying, moving,
//! duplicating and trashing, run by `bedtime-filesystem` on a worker. They
//! never replace an existing file (copies into the same folder get a new
//! name), and moved or trashed files take their open documents with them.
//! When one finishes the folders are read again; what went wrong is reported
//! as notifications. A new file opens in the editor.
use crate::{ExplorerView, Folders};
use bedtime_filesystem::operations::{
    Change, Operation, OperationRequest, OperationResult, execute,
};
use bedtime_workbench::{
    ManagedTask, ManagedWork, Notification, PathOptions, ViewerFile, WorkbenchWorldExt,
};
use bevy::prelude::*;
use std::path::{Path, PathBuf};

/// Running operations, and what's been copied or cut in a tree.
#[derive(Resource, Default)]
pub struct FileOperations {
    running: Vec<Running>,
    next_id: u64,
    /// Copied (or cut) files, for Paste.
    clipboard: Option<(Vec<PathBuf>, bool)>,
}

struct Running {
    task: ManagedTask<OperationResult>,
    /// The tree that asked: it selects what was made.
    panel: Option<Entity>,
    /// Open what was created (a new file).
    open_created: bool,
}

impl FileOperations {
    /// Whether any operation is running.
    pub fn busy(&self) -> bool {
        !self.running.is_empty()
    }

    /// What Copy or Cut put aside: the paths, and whether it was Cut.
    pub fn clipboard(&self) -> Option<(&[PathBuf], bool)> {
        self.clipboard
            .as_ref()
            .map(|(paths, cut)| (paths.as_slice(), *cut))
    }
}

/// Start `operation` under the tree's root, on a worker.
pub fn run_file_operation(
    world: &mut World,
    operation: Operation,
    panel: Option<Entity>,
) -> Result<(), String> {
    let root = match world.resource::<Folders>().root() {
        Some(Ok(root)) => root.to_owned(),
        _ => return Err("No folder is open".into()),
    };
    let open_created = matches!(operation, Operation::CreateFile { .. });
    let label = describe(&operation);
    let mut operations = world.resource_mut::<FileOperations>();
    operations.next_id += 1;
    let request = OperationRequest {
        id: operations.next_id,
        root,
        operation,
    };
    let task = world
        .resource_mut::<ManagedWork>()
        .spawn_task(None, label, move |cancel| {
            execute(request, || cancel.is_cancelled(), |_| {})
        })?;
    world
        .resource_mut::<FileOperations>()
        .running
        .push(Running {
            task,
            panel,
            open_created,
        });
    Ok(())
}

fn describe(operation: &Operation) -> String {
    match operation {
        Operation::CreateFile { name, .. } => format!("Creating {}", name.to_string_lossy()),
        Operation::CreateDirectory { name, .. } => format!("Creating {}", name.to_string_lossy()),
        Operation::Rename { name, .. } => format!("Renaming to {}", name.to_string_lossy()),
        Operation::Copy { .. } => "Copying files".into(),
        Operation::Move { .. } => "Moving files".into(),
        Operation::Duplicate { .. } => "Duplicating files".into(),
        Operation::Trash { .. } => "Moving files to the Trash".into(),
    }
}

/// Copy (or Cut) `paths` for Paste, here and on the system clipboard.
pub fn copy_files(world: &mut World, paths: Vec<PathBuf>, cut: bool) {
    if let Some(mut clipboard) = world.get_resource_mut::<bedtime_workbench::Clipboard>() {
        let _ = clipboard.write_files(&paths);
    }
    world.resource_mut::<FileOperations>().clipboard = Some((paths, cut));
}

/// Paste into `directory`: what was copied is copied; what was cut is moved
/// (once). Files copied elsewhere (the system clipboard) are copied.
pub fn paste_files(
    world: &mut World,
    directory: PathBuf,
    panel: Option<Entity>,
) -> Result<(), String> {
    let own = world.resource_mut::<FileOperations>().clipboard.clone();
    let (sources, cut) = match own {
        Some((paths, cut)) => (paths, cut),
        None => (
            world
                .get_resource_mut::<bedtime_workbench::Clipboard>()
                .and_then(|mut clipboard| clipboard.read_files().ok())
                .unwrap_or_default(),
            false,
        ),
    };
    if sources.is_empty() {
        return Err("Nothing to paste".into());
    }
    if cut {
        world.resource_mut::<FileOperations>().clipboard = None;
    }
    let operation = if cut {
        Operation::Move { sources, directory }
    } else {
        Operation::Copy { sources, directory }
    };
    run_file_operation(world, operation, panel)
}

/// Report finished operations, read the folders again, select (or open) what
/// was made. In `Update`.
pub(crate) fn finish_operations(world: &mut World) {
    let mut finished = Vec::new();
    world
        .resource_mut::<FileOperations>()
        .running
        .retain(|running| match running.task.try_recv() {
            Ok(result) => {
                finished.push((result, running.panel, running.open_created));
                false
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => true,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => false,
        });
    for (result, panel, open_created) in finished {
        world.resource_mut::<Folders>().refresh();
        follow_moves(world, &result.completed);
        if let Some(error) = result.errors.first() {
            let name = error
                .path
                .file_name()
                .unwrap_or(error.path.as_os_str())
                .to_string_lossy()
                .into_owned();
            let more = match result.errors.len() {
                1 => String::new(),
                n => format!(" (and {} more)", n - 1),
            };
            world.workbench_mut().notify(Notification::error(format!(
                "{name}: {}{more}",
                error.message
            )));
        }
        // Select the last thing made, and show its folder.
        let made = result
            .completed
            .iter()
            .rev()
            .find_map(|change| match change {
                Change::Created { path } => Some(path.clone()),
                Change::Copied { destination, .. } | Change::Moved { destination, .. } => {
                    Some(destination.clone())
                }
                Change::Trashed { .. } => None,
            });
        if let (Some(path), Some(panel)) = (made.clone(), panel) {
            reveal(world, panel, &path);
        }
        if open_created && let Some(path) = made {
            // Somewhere other than the explorer that made it.
            let options = PathOptions {
                place: panel.map(bedtime_workbench::Placement::From),
                ..default()
            };
            let _ = world.workbench_mut().open_path(&path, options);
        }
    }
}

/// Viewers of moved files reopen them where they went (their documents
/// already follow, in `bedtime-filesystem`).
fn follow_moves(world: &mut World, changes: &[Change]) {
    let moves: Vec<(&Path, &Path)> = changes
        .iter()
        .filter_map(|change| match change {
            Change::Moved {
                source,
                destination,
            } => Some((source.as_path(), destination.as_path())),
            _ => None,
        })
        .collect();
    if moves.is_empty() {
        return;
    }
    let mut viewers = world.query::<&mut ViewerFile>();
    for mut file in viewers.iter_mut(world) {
        for &(source, destination) in &moves {
            if let Ok(rest) = file.0.strip_prefix(source) {
                file.0 = if rest.as_os_str().is_empty() {
                    destination.to_owned()
                } else {
                    destination.join(rest)
                };
                break;
            }
        }
    }
}

/// Select `path`, opening the folders above it in the tree.
pub(crate) fn reveal(world: &mut World, panel: Entity, path: &Path) {
    let root = match world.resource::<Folders>().root() {
        Some(Ok(root)) => root.to_owned(),
        _ => return,
    };
    if let Some(mut tree) = world.get_mut::<crate::FileExplorer>(panel) {
        let mut folder = path.parent();
        while let Some(at) = folder.filter(|at| at.starts_with(&root)) {
            if let Ok(relative) = at.strip_prefix(&root) {
                tree.set_expanded(relative, true);
            }
            folder = at.parent();
        }
    }
    if let Some(mut view) = world.get_mut::<ExplorerView>(panel) {
        view.selected = Some(path.to_owned());
        view.reveal = Some(path.to_owned());
    }
}
