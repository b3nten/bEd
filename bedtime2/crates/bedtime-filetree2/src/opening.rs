//! Resolve native source identities before invoking the host's opening handler.
use super::*;
use bedtime_filesystem2::{Filesystem, FsDocumentSource};
use bedtime_workbench2::ManagedTask;

pub struct FileTreeSourceOpen {
    pub panel: Entity,
    pub source: FsDocumentSource,
    pub activation: FileTreeActivation,
    pub choice: Option<String>,
}
#[derive(Resource)]
pub struct FileTreeOnOpen(pub fn(&mut World, FileTreeSourceOpen) -> Result<(), String>);
#[derive(Resource, Default)]
pub(super) struct SourceOpens(Vec<Pending>);
struct Pending {
    panel: Entity,
    generation: u64,
    root: PathBuf,
    activation: FileTreeActivation,
    choice: Option<String>,
    task: ManagedTask<Result<FsDocumentSource, String>>,
    callback: fn(&mut World, FileTreeSourceOpen) -> Result<(), String>,
}
impl Drop for Pending {
    fn drop(&mut self) {
        self.task.cancel();
    }
}
pub(super) fn queue(
    world: &mut World,
    panel: Entity,
    state: &FileTreePanel,
    path: PathBuf,
    activation: FileTreeActivation,
    choice: Option<String>,
) -> Result<bool, String> {
    let Some(callback) = world.get_resource::<FileTreeOnOpen>().map(|h| h.0) else {
        return Ok(false);
    };
    let task = world.resource_mut::<ManagedWork>().spawn_task(
        Some(panel),
        "Resolve file source",
        move |_| {
            Filesystem::local()
                .document_source(path)
                .map_err(|e| e.to_string())
        },
    )?;
    world.resource_mut::<SourceOpens>().0.push(Pending {
        panel,
        generation: state.generation,
        root: state.snapshot.root.clone(),
        activation,
        choice,
        task,
        callback,
    });
    Ok(true)
}
pub(super) fn poll(world: &mut World) {
    let pending = std::mem::take(&mut world.resource_mut::<SourceOpens>().0);
    let mut remaining = Vec::new();
    for mut request in pending {
        if !world
            .get::<FileTreePanel>(request.panel)
            .is_some_and(|p| p.generation == request.generation && p.snapshot.root == request.root)
        {
            continue;
        }
        let result = match request.task.try_recv() {
            Ok(r) => Some(r),
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(_) => Some(Err("File opening cancelled".into())),
        };
        let Some(result) = result else {
            remaining.push(request);
            continue;
        };
        let result = result.and_then(|source| {
            (request.callback)(
                world,
                FileTreeSourceOpen {
                    panel: request.panel,
                    source,
                    activation: request.activation,
                    choice: request.choice.take(),
                },
            )
        });
        if let Err(error) = result {
            bedtime_workbench2::request_error_dialog(
                world,
                bedtime_workbench2::ErrorDialog {
                    title: "Open file".into(),
                    message: error,
                    details: None,
                },
            );
        }
    }
    world.resource_mut::<SourceOpens>().0 = remaining;
}
