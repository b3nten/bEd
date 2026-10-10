use crate::model::{self, Tree};
use bed_plugin::Revision;
use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
};

pub struct Job {
    pub serial: u64,
    pub revision: Revision,
    pub jsonc: bool,
    pub bytes: Arc<[u8]>,
}
pub struct Output {
    pub serial: u64,
    pub revision: Revision,
    pub result: Result<Tree, String>,
}

/// One active parse and one pending immutable snapshot per panel. Both job and
/// result channels are bounded; revisions cannot accumulate full source copies.
pub struct Worker {
    sender: Option<mpsc::SyncSender<Job>>,
    receiver: mpsc::Receiver<Output>,
    serial: Arc<AtomicU64>,
}
impl Worker {
    pub fn new() -> Result<Self, String> {
        let (sender, jobs) = mpsc::sync_channel::<Job>(1);
        let (results, receiver) = mpsc::sync_channel(1);
        let serial = Arc::new(AtomicU64::new(0));
        let active = Arc::clone(&serial);
        thread::Builder::new()
            .name("json-tree".into())
            .spawn(move || {
                while let Ok(job) = jobs.recv() {
                    let cancelled = || active.load(Ordering::Relaxed) != job.serial;
                    if cancelled() {
                        continue;
                    }
                    let result = model::parse(job.bytes, job.jsonc, cancelled);
                    if !cancelled()
                        && results
                            .send(Output {
                                serial: job.serial,
                                revision: job.revision,
                                result,
                            })
                            .is_err()
                    {
                        break;
                    }
                }
            })
            .map_err(|error| format!("Could not start the JSON preview worker: {error}"))?;
        Ok(Self {
            sender: Some(sender),
            receiver,
            serial,
        })
    }
    pub fn invalidate(&self) -> u64 {
        self.serial.fetch_add(1, Ordering::Relaxed) + 1
    }
    pub fn submit(&self, job: Job) -> bool {
        self.sender
            .as_ref()
            .is_some_and(|sender| sender.try_send(job).is_ok())
    }
    pub fn poll(&self) -> impl Iterator<Item = Output> + '_ {
        self.receiver.try_iter()
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.invalidate();
        self.sender.take();
        // Dropping the receiver also releases a worker waiting to publish.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn bounded_worker_discards_obsolete_jobs_and_publishes_latest_revision() {
        let worker = Worker::new().unwrap();
        let stale = worker.invalidate();
        let latest = worker.invalidate();
        assert!(worker.submit(Job {
            serial: stale,
            revision: (1, 1),
            bytes: Arc::from(&b"[old source]"[..]),
            jsonc: false,
        }));
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if worker.submit(Job {
                serial: latest,
                revision: (1, 3),
                bytes: Arc::from(&b"{\"unsaved\":2}"[..]),
                jsonc: false,
            }) {
                break;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        }
        loop {
            if let Some(output) = worker.poll().next() {
                assert_eq!(output.serial, latest);
                assert_eq!(output.revision, (1, 3));
                assert_eq!(output.result.unwrap().nodes[1].summary, "2");
                break;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn closing_panel_invalidates_work_and_does_not_wait_for_parsing() {
        let worker = Worker::new().unwrap();
        let observer = Arc::clone(&worker.serial);
        let serial = worker.invalidate();
        worker.submit(Job {
            serial,
            revision: (1, 0),
            bytes: Arc::from(&b"null"[..]),
            jsonc: false,
        });
        drop(worker);
        assert_ne!(observer.load(Ordering::Relaxed), serial);
    }
}
