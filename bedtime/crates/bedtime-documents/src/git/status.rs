//! Repository-wide status, shared by documents; only polling publishes worker results.
use super::git_repo::GitRepo;
use std::{
    cell::RefCell,
    collections::{BTreeMap, HashSet},
    path::PathBuf,
    rc::{Rc, Weak},
    sync::mpsc::{self, Receiver, TryRecvError},
    time::{Duration, Instant},
};

type ScanResult = Result<HashSet<String>, String>;
pub(crate) type SharedStatuses = Rc<RefCell<RepositoryStatuses>>;
pub(crate) type StatusHandle = Rc<RefCell<RepositoryStatus>>;

#[derive(Default)]
pub(crate) struct RepositoryStatuses {
    repositories: BTreeMap<(PathBuf, PathBuf), Weak<RefCell<RepositoryStatus>>>,
    snapshot: Rc<HashSet<String>>,
    revisions: Option<Vec<((PathBuf, PathBuf), u64)>>,
}

impl RepositoryStatuses {
    pub fn register(&mut self, git_dir: PathBuf, root: PathBuf) -> StatusHandle {
        let key = (git_dir, root.clone());
        if let Some(status) = self.repositories.get(&key).and_then(Weak::upgrade) {
            return status;
        }
        let status = Rc::new(RefCell::new(RepositoryStatus {
            root,
            paths: HashSet::new(),
            revision: 0,
            last_scan: None,
            pending: None,
            refresh: true,
            error: None,
        }));
        status.borrow_mut().poll(Instant::now());
        self.repositories.insert(key, Rc::downgrade(&status));
        self.revisions = None;
        status
    }

    pub fn snapshot(&mut self) -> Rc<HashSet<String>> {
        self.repositories
            .retain(|_, status| status.strong_count() != 0);
        let revisions: Vec<_> = self
            .repositories
            .iter()
            .filter_map(|(key, weak)| {
                weak.upgrade()
                    .map(|status| (key.clone(), status.borrow().revision))
            })
            .collect();
        if self.revisions.as_ref() != Some(&revisions) {
            let mut paths = HashSet::new();
            for weak in self.repositories.values() {
                if let Some(status) = weak.upgrade() {
                    paths.extend(status.borrow().paths.iter().cloned());
                }
            }
            self.snapshot = Rc::new(paths);
            self.revisions = Some(revisions);
        }
        Rc::clone(&self.snapshot)
    }
}

pub(crate) struct RepositoryStatus {
    root: PathBuf,
    pub paths: HashSet<String>,
    revision: u64,
    last_scan: Option<Instant>,
    pending: Option<Receiver<ScanResult>>,
    refresh: bool,
    pub error: Option<String>,
}

impl RepositoryStatus {
    #[cfg(test)]
    pub fn last_scan(&self) -> Instant {
        self.last_scan.unwrap()
    }

    #[cfg(test)]
    pub fn finish_scan(&mut self) {
        let result = self
            .pending
            .take()
            .expect("scan pending")
            .recv_timeout(Duration::from_secs(10))
            .expect("worker completes");
        let paths = result.expect("scan succeeds");
        if self.paths != paths {
            self.paths = paths;
            self.revision += 1;
        }
    }

    pub fn invalidate(&mut self) {
        self.refresh = true;
    }

    pub fn poll(&mut self, now: Instant) {
        if let Some(receiver) = &self.pending {
            let result = match receiver.try_recv() {
                Ok(result) => Some(result),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    Some(Err("Git status worker disconnected".into()))
                }
            };
            if let Some(result) = result {
                self.pending = None;
                match result {
                    Ok(paths) => {
                        if self.paths != paths {
                            self.paths = paths;
                            self.revision += 1;
                        }
                        self.error = None;
                    }
                    Err(error) => self.error = Some(error),
                }
            }
        }
        if self.pending.is_some()
            || (!self.refresh
                && self
                    .last_scan
                    .is_some_and(|last| now.duration_since(last) < Duration::from_secs(1)))
        {
            return;
        }
        self.refresh = false;
        self.last_scan = Some(now);
        let root = self.root.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        self.pending = Some(receiver);
        let spawned = std::thread::Builder::new()
            .name("git-status".into())
            .spawn(move || {
                let result = (|| {
                    let mut repo = GitRepo::new();
                    if !repo.open(&root.to_string_lossy()) {
                        return Err(format!("Cannot open Git repository {}", root.display()));
                    }
                    let paths = repo
                        .try_modified_paths()
                        .map_err(|error| error.to_string())?;
                    Ok(paths
                        .into_iter()
                        .filter_map(|path| {
                            let path = std::str::from_utf8(&path).ok()?;
                            // Rebuild components so Git's '/' separators match native
                            // canonical paths on Windows as well as Unix.
                            let absolute: PathBuf = root.join(path).components().collect();
                            Some(absolute.to_string_lossy().into_owned())
                        })
                        .collect())
                })();
                let _ = sender.send(result);
            });
        if let Err(error) = spawned {
            self.pending = None;
            self.error = Some(error.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{git::git_service::DocumentGitChanges, test_support::TempDir};
    use bedtime_editing::document_state::DocumentState;

    #[test]
    fn many_documents_share_one_repository_scan_and_snapshot() {
        let temp = TempDir::new();
        let repo = git2::Repository::init(temp.root()).unwrap();
        let path = temp.write("untracked", b"new");
        let statuses: SharedStatuses = Default::default();
        let mut documents = Vec::new();
        for _ in 0..30 {
            let mut document = DocumentGitChanges::new();
            document.bind_statuses(Rc::clone(&statuses));
            document.init(&DocumentState::new(), temp.root().to_str().unwrap(), true);
            documents.push(document);
        }
        let status = statuses
            .borrow()
            .repositories
            .values()
            .next()
            .unwrap()
            .upgrade()
            .unwrap();
        status.borrow_mut().finish_scan();
        assert!(
            documents
                .iter()
                .all(|document| document.is_file_modified(path.to_str().unwrap())),
            "one published result updates every document before individual polling"
        );
        for document in &mut documents {
            document.poll();
        }
        assert!(
            status.borrow().pending.is_none(),
            "tabs do not request duplicate scans"
        );
        let snapshot = statuses.borrow_mut().snapshot();
        assert!(snapshot.contains(std::fs::canonicalize(&path).unwrap().to_str().unwrap()));
        assert!(Rc::ptr_eq(&snapshot, &statuses.borrow_mut().snapshot()));
        drop(documents);
        drop(status);
        assert!(statuses.borrow_mut().snapshot().is_empty());
        drop(repo);
    }

    #[test]
    fn in_flight_refresh_is_coalesced_and_failures_keep_last_good_status() {
        let root = PathBuf::from("/not-used-by-controlled-results");
        let paths = HashSet::from(["existing".to_owned()]);
        let (sender, receiver) = mpsc::sync_channel(1);
        let now = Instant::now();
        let mut status = RepositoryStatus {
            root,
            paths: paths.clone(),
            revision: 1,
            last_scan: Some(now),
            pending: Some(receiver),
            refresh: false,
            error: None,
        };
        status.invalidate();
        status.invalidate();
        status.poll(now);
        assert!(status.pending.is_some());
        assert!(status.refresh, "refresh waits for the active scan");
        // Avoid starting a real follow-up job in this controlled failure test.
        status.refresh = false;
        sender.send(Err("scan failed".into())).unwrap();
        status.poll(now);
        assert_eq!(status.paths, paths);
        assert_eq!(status.error.as_deref(), Some("scan failed"));
        assert!(status.pending.is_none());
    }

    #[test]
    fn old_repository_completion_cannot_publish_after_rebinding() {
        let temp = TempDir::new();
        let repo = git2::Repository::init(temp.root()).unwrap();
        let mut statuses = RepositoryStatuses::default();
        let old = statuses.register(repo.path().into(), temp.root().into());
        let (sender, receiver) = mpsc::sync_channel(1);
        old.borrow_mut().pending = Some(receiver);
        drop(old);
        assert!(sender.send(Ok(HashSet::from(["obsolete".into()]))).is_err());
        let new = statuses.register(repo.path().into(), temp.root().into());
        new.borrow_mut().finish_scan();
        assert!(!statuses.snapshot().contains("obsolete"));
    }
}
