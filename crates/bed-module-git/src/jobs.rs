use bed_git::{Error, Job, Operation, Output};
use bed_remote::{RemoteClient, Request, Response};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    thread,
    time::Duration,
};

/// Remote calls execute on a module worker. Polling the helper never blocks the UI.
pub(crate) enum Worker {
    Local(Job),
    Remote {
        result: Receiver<Result<Output, String>>,
        cancelled: Arc<AtomicBool>,
    },
}
impl Worker {
    pub fn start(root: String, operation: Operation, remote: Option<RemoteClient>) -> Self {
        let Some(client) = remote else {
            return Self::Local(Job::start(PathBuf::from(root), operation));
        };
        let (sender, result) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancel = cancelled.clone();
        thread::spawn(move || {
            let mutating = operation.is_mutating();
            let result = (|| {
                let job_id = match client
                    .call(Request::GitStart { root, operation })
                    .map_err(|e| e.to_string())?
                {
                    Response::GitStarted { job_id } => job_id,
                    _ => return Err("Unexpected response starting Git job".into()),
                };
                let outcome = loop {
                    if cancel.load(Ordering::Acquire) {
                        let _ = client.call(Request::GitCancel { job_id });
                    }
                    match client.call(Request::GitPoll { job_id }) {
                        Ok(Response::GitJob {
                            result: Some(result),
                        }) => break result.map_err(display_error),
                        Ok(Response::GitJob { result: None }) => {
                            thread::sleep(Duration::from_millis(60))
                        }
                        Ok(_) => break Err("Unexpected response polling Git job".into()),
                        Err(e) => {
                            break Err(if mutating {
                                format!(
                                    "SSH connection lost; Git operation outcome is unknown. Refresh after reconnecting. {e}"
                                )
                            } else {
                                format!(
                                    "SSH connection lost while reading Git state. Refresh after reconnecting. {e}"
                                )
                            });
                        }
                    }
                };
                let _ = client.call(Request::GitRelease { job_id });
                outcome
            })();
            let _ = sender.send(result);
        });
        Self::Remote { result, cancelled }
    }
    pub fn poll(&mut self) -> Option<Result<Output, String>> {
        match self {
            Self::Local(job) => job.try_result().map(|r| r.map_err(display_error)),
            Self::Remote { result, .. } => match result.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => {
                    Some(Err("Git worker exited without a result".into()))
                }
            },
        }
    }
    pub fn cancel(&self) {
        match self {
            Self::Local(job) => job.cancel(),
            Self::Remote { cancelled, .. } => cancelled.store(true, Ordering::Release),
        }
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.cancel();
    }
}
fn display_error(error: Error) -> String {
    if error.output.trim().is_empty() {
        error.message
    } else {
        format!("{}\n{}", error.message, error.output)
    }
}
