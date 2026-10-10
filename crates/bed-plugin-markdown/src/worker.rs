use crate::model::{self, Document};
use bed_plugin::Revision;
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
};

pub struct Job {
    pub serial: u64,
    pub revision: Revision,
    pub bytes: Arc<[u8]>,
}
pub struct Output {
    pub serial: u64,
    pub revision: Revision,
    pub result: Result<Document, String>,
}
pub struct Worker {
    sender: Option<mpsc::SyncSender<Job>>,
    output: Arc<Mutex<Option<Output>>>,
    serial: Arc<AtomicU64>,
}
impl Worker {
    pub fn new() -> Result<Self, String> {
        let (sender, jobs) = mpsc::sync_channel::<Job>(1);
        let output = Arc::new(Mutex::new(None));
        let results = Arc::clone(&output);
        let serial = Arc::new(AtomicU64::new(0));
        let active = Arc::clone(&serial);
        thread::Builder::new()
            .name("markdown-preview".into())
            .spawn(move || {
                while let Ok(job) = jobs.recv() {
                    let cancelled = || active.load(Ordering::Relaxed) != job.serial;
                    if cancelled() {
                        continue;
                    }
                    let result = model::parse(&job.bytes, cancelled);
                    if !cancelled() {
                        // A bounded result queue cannot retain obsolete document models.
                        if let Ok(mut output) = results.lock() {
                            *output = Some(Output {
                                serial: job.serial,
                                revision: job.revision,
                                result,
                            });
                        }
                    }
                }
            })
            .map_err(|error| format!("Could not start Markdown preview worker: {error}"))?;
        Ok(Self {
            sender: Some(sender),
            output,
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
    pub fn poll(&self) -> Option<Output> {
        self.output.lock().ok()?.take()
    }
    pub fn close(&mut self) {
        self.invalidate();
        self.sender.take();
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.close();
    }
}
