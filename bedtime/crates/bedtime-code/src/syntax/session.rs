use crate::syntax::*;
use std::{
    sync::{
        Condvar, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::Duration,
};

static NEXT_SOURCE: AtomicU64 = AtomicU64::new(1);
struct Request {
    key: ObservationKey,
    language: &'static str,
    source: Arc<dyn Source>,
    /// Only reuse a tree when the host still has positional knowledge of this revision.
    base_revision: Option<u64>,
    token: u64,
}
#[derive(Default)]
struct Mailbox {
    pending: Option<Request>,
    completed: Option<(ObservationKey, Result<Arc<Observation>, AnalysisStatus>)>,
    stopped: bool,
}
struct Shared {
    mailbox: Mutex<Mailbox>,
    wake: Condvar,
    token: AtomicU64,
}

/// One active request and one replaceable pending request. Dropping cancels
/// without joining on the caller's thread. Hosts may run the worker under their
/// own managed lifecycle using `with_worker` instead of `new`.
pub struct AnalysisSession {
    shared: Arc<Shared>,
    source: u64,
    generation: u64,
    language: Option<&'static str>,
    requested: Option<ObservationKey>,
    status: AnalysisStatus,
    observation: Option<Arc<Observation>>,
}
/// The execution half, suitable for a host's managed background task.
pub struct AnalysisWorker {
    shared: Arc<Shared>,
}
impl Default for AnalysisSession {
    fn default() -> Self {
        Self::new()
    }
}
impl AnalysisSession {
    pub fn new() -> Self {
        let (session, worker) = Self::with_worker();
        thread::spawn(move || worker.run(|| false));
        session
    }
    pub fn with_worker() -> (Self, AnalysisWorker) {
        let shared = Arc::new(Shared {
            mailbox: Mutex::new(Mailbox::default()),
            wake: Condvar::new(),
            token: AtomicU64::new(0),
        });
        (
            Self {
                shared: shared.clone(),
                source: NEXT_SOURCE.fetch_add(1, Ordering::Relaxed),
                generation: 0,
                language: None,
                requested: None,
                status: AnalysisStatus::Unsupported,
                observation: None,
            },
            AnalysisWorker { shared },
        )
    }
    pub fn key(&self) -> Option<ObservationKey> {
        self.requested
    }
    pub fn language(&self) -> Option<&'static str> {
        self.language
    }
    pub fn status(&self) -> &AnalysisStatus {
        &self.status
    }
    pub fn observation(&self) -> Option<&Arc<Observation>> {
        self.observation.as_ref()
    }
    /// `base_revision` is a revision for which the host still has a complete
    /// journal. A gap or replacement supplies None and forces a fresh parse.
    pub fn request(
        &mut self,
        source: Arc<dyn Source>,
        revision: u64,
        language: &str,
        base_revision: Option<u64>,
    ) {
        let language = language_name(language);
        if language != self.language {
            self.generation += 1;
            self.language = language;
            self.requested = None;
        }
        let key = ObservationKey {
            source: self.source,
            revision,
            language_generation: self.generation,
        };
        if self.requested == Some(key) {
            return;
        }
        self.requested = Some(key);
        self.observation = None;
        let token = self.shared.token.fetch_add(1, Ordering::Relaxed) + 1;
        let mut mailbox = self.shared.mailbox.lock().unwrap();
        mailbox.completed = None;
        mailbox.pending = None;
        self.status = match language {
            None => AnalysisStatus::Unsupported,
            Some(_) if source.len() > MAX_SOURCE_BYTES => {
                AnalysisStatus::Limited("Source exceeds 8 MiB")
            }
            Some(language) => {
                mailbox.pending = Some(Request {
                    key,
                    source,
                    language,
                    base_revision,
                    token,
                });
                AnalysisStatus::Pending
            }
        };
        self.shared.wake.notify_one();
    }
    pub fn poll(&mut self) {
        if let Some((key, result)) = self.shared.mailbox.lock().unwrap().completed.take()
            && Some(key) == self.requested
        {
            match result {
                Ok(observation) => {
                    self.observation = Some(observation);
                    self.status = AnalysisStatus::Ready;
                }
                Err(status) => {
                    self.observation = None;
                    self.status = status;
                }
            }
        }
    }
}
impl Drop for AnalysisSession {
    fn drop(&mut self) {
        self.shared.token.fetch_add(1, Ordering::Relaxed);
        let mut mailbox = self.shared.mailbox.lock().unwrap();
        mailbox.stopped = true;
        mailbox.pending = None;
        mailbox.completed = None;
        self.shared.wake.notify_one();
    }
}
impl AnalysisWorker {
    /// Cancellation is also checked while idle, parsing and querying. No source
    /// result can resurrect a dropped session. Return means cleanup has finished.
    pub fn run(self, canceled: impl Fn() -> bool) {
        let mut backend = crate::syntax::analysis::Backend::default();
        loop {
            let request = {
                let mut mailbox = self.shared.mailbox.lock().unwrap();
                loop {
                    if mailbox.stopped || canceled() {
                        return;
                    }
                    if let Some(request) = mailbox.pending.take() {
                        break request;
                    }
                    mailbox = self
                        .shared
                        .wake
                        .wait_timeout(mailbox, Duration::from_millis(30))
                        .unwrap()
                        .0;
                }
            };
            let obsolete =
                || canceled() || self.shared.token.load(Ordering::Relaxed) != request.token;
            let result = backend.analyze(
                request.key,
                request.language,
                request.source,
                request.base_revision,
                &obsolete,
            );
            if !obsolete() {
                let mut mailbox = self.shared.mailbox.lock().unwrap();
                if !mailbox.stopped {
                    mailbox.completed = Some((request.key, result.map(Arc::new)));
                }
            }
        }
    }
}
