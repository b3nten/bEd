//! Background work owned by the app. Tasks run on their own threads and hand
//! back a value through a `ManagedTask`; they never touch the World. A task
//! tied to an owner entity is cancelled when the owner goes away, and shutdown
//! (on `AppExit`, or `ManagedWork::shutdown`) cancels everything and waits for
//! it to stop.
use bevy::prelude::*;
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WorkId(pub u64);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkStatus {
    Running,
    Cancelling,
    Cancelled,
    Finished,
    Failed(String),
}

/// Cooperative cancellation, shared between a task and whoever started it.
#[derive(Clone)]
pub struct Cancellation(Arc<Control>);

struct Control {
    cancelled: AtomicBool,
    status: Mutex<WorkStatus>,
    wake: Condvar,
}

impl Cancellation {
    fn new() -> Self {
        Self(Arc::new(Control {
            cancelled: AtomicBool::new(false),
            status: Mutex::new(WorkStatus::Running),
            wake: Condvar::new(),
        }))
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::Acquire)
    }

    pub fn cancel(&self) {
        self.0.cancelled.store(true, Ordering::Release);
        let mut status = self.0.status.lock().unwrap();
        if matches!(*status, WorkStatus::Running) {
            *status = WorkStatus::Cancelling;
        }
        self.0.wake.notify_all();
    }

    /// Sleep for up to `duration`, waking early on cancellation. Returns whether
    /// the task was cancelled.
    pub fn wait(&self, duration: Duration) -> bool {
        let guard = self.0.status.lock().unwrap();
        let _ = self
            .0
            .wake
            .wait_timeout_while(guard, duration, |_| !self.is_cancelled())
            .unwrap();
        self.is_cancelled()
    }

    pub fn status(&self) -> WorkStatus {
        self.0.status.lock().unwrap().clone()
    }

    fn finish(&self, result: Result<(), String>) {
        *self.0.status.lock().unwrap() = match result {
            Err(error) => WorkStatus::Failed(error),
            Ok(()) if self.is_cancelled() => WorkStatus::Cancelled,
            Ok(()) => WorkStatus::Finished,
        };
        self.0.wake.notify_all();
    }
}

/// The receiving end of a task. A cancelled task's value is never delivered.
pub struct ManagedTask<T> {
    pub id: WorkId,
    receiver: Mutex<mpsc::Receiver<T>>,
    cancellation: Cancellation,
}

impl<T> ManagedTask<T> {
    pub fn try_recv(&self) -> Result<T, mpsc::TryRecvError> {
        self.receiver.lock().unwrap().try_recv()
    }

    pub fn cancel(&self) {
        self.cancellation.cancel();
    }

    pub fn status(&self) -> WorkStatus {
        self.cancellation.status()
    }
}

#[derive(Clone, Debug)]
pub struct WorkInfo {
    pub id: WorkId,
    pub owner: Option<Entity>,
    pub label: String,
    pub status: WorkStatus,
}

struct Running {
    owner: Option<Entity>,
    label: String,
    cancellation: Cancellation,
    worker: JoinHandle<()>,
}

#[derive(Resource, Default)]
pub struct ManagedWork {
    next_id: u64,
    closing: bool,
    running: BTreeMap<WorkId, Running>,
}

impl ManagedWork {
    /// After shutdown no new work is accepted.
    pub fn is_closing(&self) -> bool {
        self.closing
    }

    /// Run `task` on its own thread. With an `owner`, the task is cancelled
    /// when that entity is despawned.
    pub fn spawn_task<T: Send + 'static>(
        &mut self,
        owner: Option<Entity>,
        label: impl Into<String>,
        task: impl FnOnce(Cancellation) -> T + Send + 'static,
    ) -> Result<ManagedTask<T>, String> {
        if self.closing {
            return Err("The application is closing".into());
        }
        self.next_id += 1;
        let id = WorkId(self.next_id);
        let cancellation = Cancellation::new();
        let cancel = cancellation.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("managed-task".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let value = task(cancel.clone());
                    if !cancel.is_cancelled() {
                        let _ = sender.send(value);
                    }
                }));
                cancel.finish(result.map_err(|_| "The task panicked".into()));
            })
            .map_err(|e| e.to_string())?;
        self.running.insert(
            id,
            Running {
                owner,
                label: label.into(),
                cancellation: cancellation.clone(),
                worker,
            },
        );
        Ok(ManagedTask {
            id,
            receiver: Mutex::new(receiver),
            cancellation,
        })
    }

    pub fn list(&self) -> Vec<WorkInfo> {
        self.running
            .iter()
            .map(|(&id, job)| WorkInfo {
                id,
                owner: job.owner,
                label: job.label.clone(),
                status: job.cancellation.status(),
            })
            .collect()
    }

    pub fn cancel(&mut self, id: WorkId) -> Result<(), String> {
        self.running
            .get(&id)
            .ok_or("Work is unavailable")?
            .cancellation
            .cancel();
        Ok(())
    }

    /// Cancel everything and stop accepting work. `is_finished` reports when
    /// every task has stopped.
    pub fn shutdown(&mut self) {
        self.closing = true;
        for job in self.running.values() {
            job.cancellation.cancel();
        }
    }

    /// Accept work again after `shutdown`, for a session that goes on
    /// (`restart_session`).
    pub(crate) fn reopen(&mut self) {
        self.closing = false;
    }

    pub fn is_finished(&self) -> bool {
        self.running.values().all(|job| job.worker.is_finished())
    }

    /// Cancel tasks whose owner is gone, and forget tasks that have stopped.
    pub(crate) fn poll(&mut self, owner_exists: impl Fn(Entity) -> bool) {
        for job in self.running.values() {
            if job.owner.is_some_and(|owner| !owner_exists(owner)) {
                job.cancellation.cancel();
            }
        }
        let finished: Vec<_> = self
            .running
            .iter()
            .filter_map(|(&id, job)| job.worker.is_finished().then_some(id))
            .collect();
        for id in finished {
            let job = self.running.remove(&id).unwrap();
            if job.worker.join().is_err() {
                job.cancellation.finish(Err("The task panicked".into()));
            }
        }
    }
}

impl Drop for ManagedWork {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Keeps `ManagedWork` and cancels tasks whose owner is gone. When the app
/// exits, work is shut down.
pub struct WorkPlugin;

impl Plugin for WorkPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ManagedWork>()
            .add_systems(Last, poll.in_set(WorkSystems::Poll));
    }
}

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WorkSystems {
    /// Cancelling orphaned tasks and collecting finished ones, in `Last`.
    Poll,
}

fn poll(world: &mut World) {
    let exiting = world
        .get_resource::<Messages<AppExit>>()
        .is_some_and(|m| !m.is_empty());
    world.resource_scope(|world, mut work: Mut<ManagedWork>| {
        if exiting {
            work.shutdown();
        }
        work.poll(|owner| world.get_entity(owner).is_ok());
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn tasks_are_cancelled_with_their_owner_and_by_shutdown() {
        let mut app = App::new();
        app.add_plugins(WorkPlugin);
        let owner = app.world_mut().spawn_empty().id();
        let mut work = app.world_mut().resource_mut::<ManagedWork>();
        let owned = work
            .spawn_task(Some(owner), "Owned", |stop| {
                while !stop.wait(Duration::from_secs(5)) {}
            })
            .unwrap();
        let unowned = work
            .spawn_task(
                None,
                "Unowned",
                |stop| while !stop.wait(Duration::from_secs(5)) {},
            )
            .unwrap();
        let done = work.spawn_task(None, "Done", |_| 42).unwrap();

        app.world_mut().despawn(owner);
        app.update();
        assert!(matches!(
            owned.status(),
            WorkStatus::Cancelling | WorkStatus::Cancelled
        ));
        assert_eq!(unowned.status(), WorkStatus::Running);

        app.world_mut().resource_mut::<ManagedWork>().shutdown();
        assert!(
            app.world_mut()
                .resource_mut::<ManagedWork>()
                .spawn_task(None, "Late", |_| ())
                .is_err()
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while !app.world().resource::<ManagedWork>().is_finished() {
            assert!(Instant::now() < deadline);
            app.update();
        }
        assert_eq!(unowned.status(), WorkStatus::Cancelled);
        assert!(unowned.try_recv().is_err());
        assert_eq!(done.try_recv(), Ok(42));
    }
}
