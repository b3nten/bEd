//! On-demand file classification for viewer menus. Filesystem and SSH reads stay
//! on a worker, and only the prefix used by Bed's text detector is transferred.
use bed_remote::{LocalBackend, RemoteClient, Request, Response};
use bed_workbench_api::DocumentKind;
use std::{
    collections::HashMap,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

const CACHE_LIMIT: usize = 128;
const CACHE_LIFETIME: Duration = Duration::from_secs(5);

fn affected_path(candidate: &str, path: &str) -> bool {
    candidate == path
        || candidate
            .strip_prefix(path.trim_end_matches(['/', '\\']))
            .is_some_and(|tail| tail.starts_with(['/', '\\']))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ContentStatus {
    Loading,
    Ready(DocumentKind),
    Unavailable(String),
}
impl ContentStatus {
    pub fn kind(&self) -> Option<DocumentKind> {
        match self {
            Self::Ready(kind) => Some(*kind),
            _ => None,
        }
    }
}

struct Job {
    serial: u64,
    root: String,
    path: String,
    remote: Option<RemoteClient>,
}
struct Completed {
    serial: u64,
    path: String,
    result: Result<(String, DocumentKind), String>,
}
struct Worker {
    sender: mpsc::SyncSender<Job>,
    receiver: mpsc::Receiver<Completed>,
}
impl Worker {
    fn new() -> Self {
        let (sender, jobs) = mpsc::sync_channel::<Job>(1);
        let (results, receiver) = mpsc::channel();
        thread::spawn(move || {
            while let Ok(job) = jobs.recv() {
                let request = Request::ReadFilePrefix {
                    root: job.root,
                    path: job.path.clone(),
                };
                let response = match job.remote {
                    Some(client) => client.call(request),
                    None => LocalBackend.call(request).map_err(|error| error.into_io()),
                };
                let result = response
                    .map_err(|error| error.to_string())
                    .and_then(|response| match response {
                        Response::FilePrefix { path, bytes } => {
                            Ok((path, bed_files::files::classify_bytes(&bytes)))
                        }
                        _ => Err("Unexpected file prefix response".into()),
                    });
                if results
                    .send(Completed {
                        serial: job.serial,
                        path: job.path,
                        result,
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        Self { sender, receiver }
    }
}

struct Cached {
    updated: Instant,
    result: Result<(String, DocumentKind), String>,
    used_by_open_document: bool,
}

#[derive(Default)]
pub(super) struct FileContent {
    scope: String,
    root: String,
    remote: bool,
    connected: bool,
    client: Option<RemoteClient>,
    serial: u64,
    worker: Option<Worker>,
    pending: HashMap<String, u64>,
    invalidations: Vec<(String, u64)>,
    cache: HashMap<String, Cached>,
}
impl FileContent {
    pub fn configure(
        &mut self,
        scope: &str,
        root: &str,
        remote: bool,
        client: Option<RemoteClient>,
    ) {
        let connected = !remote || client.as_ref().is_some_and(RemoteClient::is_connected);
        if self.scope != scope
            || self.root != root
            || self.remote != remote
            || self.connected != connected
        {
            self.reset();
            self.scope = scope.into();
            self.root = root.into();
            self.remote = remote;
            self.connected = connected;
        }
        self.client = client;
    }

    pub fn reset(&mut self) {
        self.cache.clear();
        self.pending.clear();
        self.invalidations.clear();
        // A request on an old connection must not delay a new workspace.
        self.worker = None;
    }

    pub fn invalidate(&mut self, path: &str) {
        self.cache.retain(|candidate, cached| {
            !affected_path(candidate, path)
                && !cached
                    .result
                    .as_ref()
                    .is_ok_and(|(resolved, _)| affected_path(resolved, path))
        });
        self.pending
            .retain(|candidate, _| !affected_path(candidate, path));
        if self.pending.is_empty() {
            self.invalidations.clear();
        } else {
            // An in-flight symlink's canonical target is known only on reply.
            self.serial = self.serial.wrapping_add(1);
            self.invalidations.push((path.into(), self.serial));
            if self.invalidations.len() >= CACHE_LIMIT {
                self.pending.clear();
                self.invalidations.clear();
            }
        }
    }

    /// The backend's canonical path lets symlink rows find the open document.
    pub fn resolved_path(&self, path: &str) -> Option<&str> {
        let cached = self.cache.get(path)?;
        (cached.used_by_open_document || cached.updated.elapsed() < CACHE_LIFETIME)
            .then(|| cached.result.as_ref().ok().map(|(path, _)| path.as_str()))
            .flatten()
    }

    pub fn use_open_document(&mut self, path: &str) {
        if let Some(cached) = self.cache.get_mut(path) {
            cached.used_by_open_document = true;
        }
    }

    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        let Some(worker) = &self.worker else {
            return false;
        };
        while let Ok(completed) = worker.receiver.try_recv() {
            // A filesystem change can invalidate a request while it is reading.
            if self.pending.get(&completed.path) != Some(&completed.serial) {
                continue;
            }
            self.pending.remove(&completed.path);
            changed = true;
            if completed.result.as_ref().is_ok_and(|(resolved, _)| {
                self.invalidations.iter().any(|(path, serial)| {
                    *serial > completed.serial && affected_path(resolved, path)
                })
            }) {
                continue;
            }
            if self.cache.len() >= CACHE_LIMIT
                && let Some(oldest) = self
                    .cache
                    .iter()
                    .min_by_key(|(_, cached)| cached.updated)
                    .map(|(path, _)| path.clone())
            {
                self.cache.remove(&oldest);
            }
            self.cache.insert(
                completed.path,
                Cached {
                    updated: Instant::now(),
                    result: completed.result,
                    used_by_open_document: false,
                },
            );
        }
        if self.pending.is_empty() {
            self.invalidations.clear();
        }
        changed
    }

    pub fn inspect(&mut self, path: &str) -> ContentStatus {
        if let Some(cached) = self.cache.get(path)
            && cached.updated.elapsed() < CACHE_LIFETIME
            && !cached.used_by_open_document
        {
            return match &cached.result {
                Ok((_, kind)) => ContentStatus::Ready(*kind),
                Err(error) => ContentStatus::Unavailable(error.clone()),
            };
        }
        self.cache.remove(path);
        if self.remote && !self.connected {
            return ContentStatus::Unavailable("Remote workspace disconnected".into());
        }
        if !self.pending.contains_key(path) {
            self.serial = self.serial.wrapping_add(1);
            let job = Job {
                serial: self.serial,
                root: self.root.clone(),
                path: path.into(),
                remote: self.client.clone(),
            };
            let worker = self.worker.get_or_insert_with(Worker::new);
            match worker.sender.try_send(job) {
                Ok(()) => {
                    self.pending.insert(path.into(), self.serial);
                }
                Err(mpsc::TrySendError::Full(_)) => {}
                Err(mpsc::TrySendError::Disconnected(_)) => {
                    return ContentStatus::Unavailable("File inspection worker unavailable".into());
                }
            }
        }
        ContentStatus::Loading
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        sync::atomic::{AtomicU64, Ordering},
    };

    struct TempDir(std::path::PathBuf);
    impl TempDir {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(1);
            let path = std::env::temp_dir().join(format!(
                "bed-file-content-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn root(&self) -> &str {
            self.0.to_str().unwrap()
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn await_inspection(content: &mut FileContent, path: &str) -> ContentStatus {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            content.poll();
            let status = content.inspect(path);
            if status != ContentStatus::Loading {
                return status;
            }
            assert!(Instant::now() < deadline, "inspection did not complete");
            thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn inspects_only_requested_files_and_rechecks_changed_content() {
        let temp = TempDir::new();
        fs::write(temp.0.join("data.csv"), b"name,value\nfirst,1\n").unwrap();
        let mut content = FileContent::default();
        content.configure("local", temp.root(), false, None);
        assert!(content.worker.is_none());
        assert_eq!(content.inspect("data.csv"), ContentStatus::Loading);
        assert_eq!(
            await_inspection(&mut content, "data.csv"),
            ContentStatus::Ready(DocumentKind::Text)
        );
        fs::write(temp.0.join("data.csv"), [0; 32]).unwrap();
        content.invalidate("data.csv");
        assert_eq!(content.inspect("data.csv"), ContentStatus::Loading);
        assert_eq!(
            await_inspection(&mut content, "data.csv"),
            ContentStatus::Ready(DocumentKind::Bytes)
        );
        assert!(matches!(
            await_inspection(&mut content, "missing"),
            ContentStatus::Unavailable(_)
        ));
    }

    #[test]
    fn disconnected_remote_never_inspects_a_coincident_local_file() {
        let temp = TempDir::new();
        fs::write(temp.0.join("readme"), b"local text").unwrap();
        let mut content = FileContent::default();
        content.configure("remote", temp.root(), true, None);
        assert!(matches!(
            content.inspect("readme"),
            ContentStatus::Unavailable(_)
        ));
        assert!(content.worker.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_rows_use_unsaved_document_bytes_and_refresh_after_close() {
        use bed_workbench_api::{HostContext, PluginDocument};
        use std::{cell::RefCell, rc::Rc, sync::Arc};
        let temp = TempDir::new();
        let target = temp.0.join("target.csv");
        fs::write(&target, b"name,value\nfirst,1\n").unwrap();
        std::os::unix::fs::symlink("target.csv", temp.0.join("alias.csv")).unwrap();
        let mut content = FileContent::default();
        content.configure("local", temp.root(), false, None);
        assert_eq!(
            await_inspection(&mut content, "alias.csv"),
            ContentStatus::Ready(DocumentKind::Text),
        );
        let canonical = fs::canonicalize(&target)
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        assert_eq!(content.resolved_path("alias.csv"), Some(canonical.as_str()));
        let document = PluginDocument {
            id: bed_document_session::DocumentId::next(),
            path: canonical.clone(),
            kind: DocumentKind::Bytes,
            language_id: String::new(),
            revision: (1, 1),
            dirty: true,
            bytes: Arc::from(&[0; 32][..]),
            text: None,
        };
        let textures = HashMap::new();
        let host = HostContext {
            remote: false,
            default_viewers: &serde_json::Value::Null,
            viewer_menu: None,
            documents: &[document],
            active_document: None,
            settings: &serde_json::Value::Null,
            textures: &textures,
            animations: false,
            workspace: 0,
            diagnostics: &serde_json::Value::Null,
        };
        let menus = crate::RegistryMenus {
            commands: Vec::new(),
            viewers: Rc::default(),
        };
        let content = RefCell::new(content);
        assert_eq!(
            menus.content_status("alias.csv", &host, &content),
            ContentStatus::Ready(DocumentKind::Bytes)
        );
        content
            .borrow_mut()
            .cache
            .get_mut("alias.csv")
            .unwrap()
            .updated = Instant::now() - CACHE_LIFETIME;
        assert_eq!(
            menus.content_status("alias.csv", &host, &content),
            ContentStatus::Ready(DocumentKind::Bytes),
            "open symlink documents retain their mapping after the disk cache expires",
        );
        fs::write(&target, [0; 32]).unwrap();
        assert_eq!(
            await_inspection(&mut content.borrow_mut(), "alias.csv"),
            ContentStatus::Ready(DocumentKind::Bytes),
        );
        content.borrow_mut().invalidate(&canonical);
        assert!(
            content.borrow().cache.is_empty(),
            "target changes invalidate cached symlinks"
        );
    }

    #[test]
    fn directory_invalidation_accepts_windows_path_separators() {
        let mut content = FileContent::default();
        content
            .pending
            .insert("C:\\project\\dir\\file.csv".into(), 1);
        content
            .pending
            .insert("C:\\project\\directory\\file.csv".into(), 2);
        content.invalidate("C:\\project\\dir");
        assert!(!content.pending.contains_key("C:\\project\\dir\\file.csv"));
        assert!(
            content
                .pending
                .contains_key("C:\\project\\directory\\file.csv")
        );
    }

    #[test]
    fn invalidated_inflight_results_cannot_replace_newer_classification() {
        let mut content = FileContent::default();
        let (sender, _jobs) = mpsc::sync_channel(1);
        let (results, receiver) = mpsc::channel();
        content.worker = Some(Worker { sender, receiver });
        content.pending.insert("/project/data.csv".into(), 1);
        content.invalidate("/project");
        content.pending.insert("/project/data.csv".into(), 2);
        for (serial, kind) in [(1, DocumentKind::Text), (2, DocumentKind::Bytes)] {
            results
                .send(Completed {
                    serial,
                    path: "/project/data.csv".into(),
                    result: Ok(("/project/data.csv".into(), kind)),
                })
                .unwrap();
        }
        assert!(content.poll());
        assert_eq!(
            content.inspect("/project/data.csv"),
            ContentStatus::Ready(DocumentKind::Bytes)
        );
        content.serial = 3;
        content.pending.insert("/project/alias.csv".into(), 3);
        content.invalidate("/project/target.csv");
        results
            .send(Completed {
                serial: 3,
                path: "/project/alias.csv".into(),
                result: Ok(("/project/target.csv".into(), DocumentKind::Text)),
            })
            .unwrap();
        assert!(content.poll());
        assert!(
            !content.cache.contains_key("/project/alias.csv"),
            "a target change invalidates an in-flight symlink prefix"
        );
        content.configure("another workspace", "/project", false, None);
        assert!(content.cache.is_empty());
        assert!(content.pending.is_empty());
        assert!(content.worker.is_none());
    }
}
