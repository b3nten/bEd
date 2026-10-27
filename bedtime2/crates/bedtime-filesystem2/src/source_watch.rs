//! One managed invalidation worker for retained file documents in a World.
use crate::watch::DirectoryWatcher;
use bedtime_workbench2::*;
use bevy::prelude::*;
use std::{
    collections::BTreeSet,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
#[derive(Default)]
struct Mail {
    demand: BTreeSet<PathBuf>,
    changed: BTreeSet<PathBuf>,
    rescan: bool,
    error: Option<String>,
}
#[derive(Resource)]
struct SourceWatch {
    mail: Arc<Mutex<Mail>>,
    task: ManagedTask<()>,
}
impl Drop for SourceWatch {
    fn drop(&mut self) {
        self.task.cancel();
    }
}
pub(crate) fn error(world: &World) -> Option<String> {
    world
        .get_resource::<SourceWatch>()?
        .mail
        .lock()
        .unwrap()
        .error
        .clone()
}
pub(crate) fn update(world: &mut World) {
    let files: Vec<_> = document_ids(world)
        .into_iter()
        .filter_map(|id| Some((id, document_source_location(world, id)?)))
        .collect();
    if files.is_empty() {
        world.remove_resource::<SourceWatch>();
        return;
    }
    if world.resource::<ManagedWork>().is_closing() {
        return;
    }
    if !world.contains_resource::<SourceWatch>() {
        let mail = Arc::new(Mutex::new(Mail::default()));
        let worker_mail = mail.clone();
        let Ok(task) = world.resource_mut::<ManagedWork>().spawn_task(
            None,
            "Watch document sources",
            move |cancel| {
                let mut watcher = match DirectoryWatcher::new() {
                    Ok(w) => w,
                    Err(e) => {
                        worker_mail.lock().unwrap().error = Some(e.to_string());
                        return;
                    }
                };
                let mut registered = BTreeSet::new();
                while !cancel.is_cancelled() {
                    let demand = worker_mail.lock().unwrap().demand.clone();
                    for path in registered.difference(&demand).cloned().collect::<Vec<_>>() {
                        let _ = watcher.unwatch(&path);
                        registered.remove(&path);
                    }
                    for path in demand.difference(&registered).cloned().collect::<Vec<_>>() {
                        match watcher.watch(&path) {
                            Ok(()) => {
                                registered.insert(path);
                            }
                            Err(e) => worker_mail.lock().unwrap().error = Some(e.to_string()),
                        }
                    }
                    match watcher.poll() {
                        Ok(batch) => {
                            let mut mail = worker_mail.lock().unwrap();
                            mail.rescan |= batch.rescan;
                            if let Some(error) = batch.error {
                                mail.error = Some(error);
                            }
                            for event in batch.events {
                                let path = if demand.contains(&event.path) {
                                    Some(event.path.clone())
                                } else {
                                    event
                                        .path
                                        .parent()
                                        .filter(|p| demand.contains(*p))
                                        .map(|p| p.to_owned())
                                };
                                if let Some(path) = path {
                                    mail.changed.insert(path.clone());
                                    if event.lost {
                                        registered.remove(&path);
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            let mut mail = worker_mail.lock().unwrap();
                            mail.error = Some(e.to_string());
                            mail.rescan = true;
                        }
                    }
                    cancel.wait(Duration::from_millis(20));
                }
            },
        ) else {
            return;
        };
        world.insert_resource(SourceWatch { mail, task });
    }
    let watch = world.resource::<SourceWatch>();
    let (changed, rescan) = {
        let mut mail = watch.mail.lock().unwrap();
        mail.demand = files
            .iter()
            .flat_map(|(_, path)| {
                path.parent()
                    .into_iter()
                    .chain(path.parent().and_then(|p| p.parent()))
            })
            .map(|p| p.to_owned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .take(512)
            .collect();
        (
            std::mem::take(&mut mail.changed),
            std::mem::take(&mut mail.rescan),
        )
    };
    for (id, path) in files {
        if rescan || changed.iter().any(|p| path.starts_with(p)) {
            invalidate_document_source(world, id);
        }
    }
}
