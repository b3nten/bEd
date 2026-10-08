//! Cancellable filesystem jobs shared by native file commands and SSH transfers.
//! Conflict and lifecycle acknowledgements run on the host thread before commit.
use bed_remote::{RemoteClient, Request, Response, TRANSFER_CHUNK_BYTES, TransferEntry};
use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, UNIX_EPOCH},
};

#[derive(Clone, Debug)]
pub enum Endpoint {
    Local { root: PathBuf },
    Remote { root: String, client: RemoteClient },
}
impl Endpoint {
    pub fn local(root: impl Into<PathBuf>) -> Self {
        Self::Local { root: root.into() }
    }
    fn root(&self) -> String {
        match self {
            Self::Local { root } => root.to_string_lossy().into_owned(),
            Self::Remote { root, .. } => root.clone(),
        }
    }
    fn call(&self, request: Request) -> io::Result<Response> {
        match self {
            Self::Local { .. } => bed_remote::LocalBackend
                .call(request)
                .map_err(|error| error.into_io()),
            Self::Remote { client, .. } => client.call(request),
        }
    }
    fn stat(&self, path: &str) -> io::Result<TransferEntry> {
        match self.call(Request::TransferStat {
            root: self.root(),
            path: path.into(),
        })? {
            Response::TransferStat { entry } => Ok(entry),
            _ => Err(unexpected()),
        }
    }
    fn children(&self, path: &str) -> io::Result<Vec<String>> {
        match self.call(Request::ReadDirectory {
            root: self.root(),
            path: path.into(),
            classify_gitignored: false,
        })? {
            Response::Directory { entries, .. } => {
                Ok(entries.into_iter().map(|entry| entry.path).collect())
            }
            _ => Err(unexpected()),
        }
    }
    fn remove(&self, path: &str, directory: bool) -> io::Result<()> {
        self.call(Request::Remove {
            root: self.root(),
            path: path.into(),
            is_directory: directory,
        })?;
        Ok(())
    }
    fn same_storage(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Local { root: a }, Self::Local { root: b }) => a == b,
            (
                Self::Remote {
                    root: a,
                    client: ac,
                },
                Self::Remote {
                    root: b,
                    client: bc,
                },
            ) => a == b && ac.same_connection(bc),
            _ => false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperationKind {
    Copy,
    Move,
    Delete,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConflictChoice {
    Skip,
    KeepBoth,
    Replace,
    Merge,
}
#[derive(Clone, Copy, Debug)]
pub struct ConflictDecision {
    pub choice: ConflictChoice,
    pub apply_to_all: bool,
}
#[derive(Debug)]
pub struct Conflict {
    pub source: String,
    pub destination: String,
    pub source_directory: bool,
    pub destination_directory: bool,
    pub reply: mpsc::SyncSender<ConflictDecision>,
}
#[derive(Debug)]
pub enum OperationEvent {
    Progress {
        path: String,
        completed: usize,
        bytes: u64,
    },
    Conflict(Conflict),
    BeforeReplace {
        path: String,
        reply: mpsc::SyncSender<Result<(), String>>,
    },
    Moved {
        source: String,
        destination: String,
        reply: mpsc::SyncSender<Result<(), String>>,
    },
    Deleted {
        path: String,
    },
    Finished(OperationSummary),
}
#[derive(Clone, Debug, Default)]
pub struct OperationSummary {
    pub completed: usize,
    pub skipped: usize,
    pub bytes: u64,
    pub errors: Vec<String>,
    pub canceled: bool,
    pub destinations: Vec<String>,
}
pub type Trash = Arc<dyn Fn(&Path) -> io::Result<()> + Send + Sync>;
pub struct OperationRequest {
    pub kind: OperationKind,
    pub source: Endpoint,
    pub destination: Endpoint,
    pub paths: Vec<String>,
    pub directory: String,
    pub duplicate: bool,
    pub trash: Option<Trash>,
}
pub struct OperationJob {
    pub events: mpsc::Receiver<OperationEvent>,
    canceled: Arc<AtomicBool>,
}
impl OperationJob {
    pub fn start(request: OperationRequest) -> Self {
        let (sender, events) = mpsc::sync_channel(64);
        let canceled = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&canceled);
        thread::spawn(move || {
            let mut worker = Worker {
                request,
                sender,
                canceled: stop,
                summary: OperationSummary::default(),
                file_choice: None,
                folder_choice: None,
                copied_paths: std::collections::BTreeMap::new(),
            };
            worker.run();
            let _ = worker.sender.send(OperationEvent::Finished(worker.summary));
        });
        Self { events, canceled }
    }
    pub fn cancel(&self) {
        self.canceled.store(true, Ordering::Release);
    }
    pub fn is_canceled(&self) -> bool {
        self.canceled.load(Ordering::Acquire)
    }
}
impl Drop for OperationJob {
    fn drop(&mut self) {
        self.cancel();
    }
}

struct Worker {
    request: OperationRequest,
    sender: mpsc::SyncSender<OperationEvent>,
    canceled: Arc<AtomicBool>,
    summary: OperationSummary,
    file_choice: Option<ConflictChoice>,
    folder_choice: Option<ConflictChoice>,
    copied_paths: std::collections::BTreeMap<String, String>,
}
impl Worker {
    fn check(&self) -> io::Result<()> {
        if self.canceled.load(Ordering::Acquire) {
            Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "File operation canceled",
            ))
        } else {
            Ok(())
        }
    }
    fn send(&self, event: OperationEvent) -> io::Result<()> {
        self.sender
            .send(event)
            .map_err(|_| io::Error::new(io::ErrorKind::Interrupted, "File operation host closed"))
    }
    fn await_reply<T>(&self, receiver: mpsc::Receiver<T>) -> io::Result<T> {
        loop {
            self.check()?;
            match receiver.recv_timeout(Duration::from_millis(50)) {
                Ok(value) => {
                    self.check()?;
                    return Ok(value);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::Interrupted,
                        "File operation host closed",
                    ));
                }
            }
        }
    }
    fn run(&mut self) {
        let paths = top_level_paths(&self.request.paths);
        for source in paths {
            self.copied_paths.clear();
            if self.check().is_err() {
                self.summary.canceled = true;
                break;
            }
            let result = if self.request.kind == OperationKind::Delete {
                self.delete(&source)
            } else {
                let directory = if self.request.duplicate {
                    parent(&source)
                } else {
                    self.request.directory.clone()
                };
                filename(&source)
                    .map(|name| join(&directory, name))
                    .and_then(|mut destination| {
                        if self.request.duplicate {
                            destination = self.unique(&destination)?;
                        }
                        if matches!(self.request.source, Endpoint::Local { .. })
                            && matches!(self.request.destination, Endpoint::Local { .. })
                        {
                            let parent = fs::canonicalize(&directory)?;
                            if fs::symlink_metadata(&source)?.is_dir()
                                && parent.starts_with(fs::canonicalize(&source)?)
                            {
                                return Err(io::Error::new(
                                    io::ErrorKind::InvalidInput,
                                    "Cannot copy or move a folder into itself",
                                ));
                            }
                        }
                        if contains(&source, &directory)
                            && self.request.source.same_storage(&self.request.destination)
                        {
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidInput,
                                "Cannot copy or move a folder into itself",
                            ));
                        }
                        if source == destination
                            && self.request.kind == OperationKind::Copy
                            && self.request.source.same_storage(&self.request.destination)
                        {
                            destination = self.unique(&destination)?;
                        }
                        self.transfer(&source, &destination, true).map(|completed| {
                            if let Some(destination) = completed {
                                self.summary.destinations.push(destination);
                            }
                        })
                    })
            };
            if let Err(error) = result {
                if error.kind() == io::ErrorKind::Interrupted {
                    self.summary.canceled = true;
                    break;
                }
                self.summary.errors.push(format!("{source}: {error}"));
            }
        }
    }
    fn delete(&mut self, source: &str) -> io::Result<()> {
        let entry = self.request.source.stat(source)?;
        if let (Endpoint::Local { root }, Some(trash)) = (&self.request.source, &self.request.trash)
        {
            let path = crate::actions::validate_project_entry(root, Path::new(source))?;
            trash(&path)?;
        } else {
            self.request.source.remove(source, entry.is_directory)?;
        }
        self.summary.completed += 1;
        self.send(OperationEvent::Deleted {
            path: source.into(),
        })?;
        self.progress(source)
    }
    fn unique(&self, path: &str) -> io::Result<String> {
        let name = filename(path)?;
        let parent = parent(path);
        let (stem, extension) = match name.rsplit_once('.') {
            Some((stem, ext)) if !stem.is_empty() => (stem, format!(".{ext}")),
            _ => (name, String::new()),
        };
        for n in 1..100_000 {
            let candidate = join(
                &parent,
                &format!(
                    "{stem} copy{}{extension}",
                    if n == 1 {
                        String::new()
                    } else {
                        format!(" {n}")
                    }
                ),
            );
            match self.request.destination.stat(&candidate) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(candidate),
                Err(error) => return Err(error),
                _ => {}
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "Cannot choose a unique filename",
        ))
    }
    fn resolve(
        &mut self,
        source: &str,
        destination: &str,
        a: &TransferEntry,
        b: &TransferEntry,
    ) -> io::Result<ConflictChoice> {
        let folder = a.is_directory && b.is_directory;
        if let Some(choice) = if folder {
            self.folder_choice
        } else {
            self.file_choice
        } && (choice != ConflictChoice::Replace || (!a.is_directory && !b.is_directory))
        {
            return Ok(choice);
        }
        let (reply, receiver) = mpsc::sync_channel(1);
        self.send(OperationEvent::Conflict(Conflict {
            source: source.into(),
            destination: destination.into(),
            source_directory: a.is_directory,
            destination_directory: b.is_directory,
            reply,
        }))?;
        let decision = self.await_reply(receiver)?;
        if (decision.choice == ConflictChoice::Merge && !folder)
            || (decision.choice == ConflictChoice::Replace && (a.is_directory || b.is_directory))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid conflict resolution",
            ));
        }
        if decision.apply_to_all {
            if folder {
                self.folder_choice = Some(decision.choice);
            } else {
                self.file_choice = Some(decision.choice);
            }
        }
        Ok(decision.choice)
    }
    fn acknowledge(
        &self,
        event: impl FnOnce(mpsc::SyncSender<Result<(), String>>) -> OperationEvent,
    ) -> io::Result<()> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.send(event(reply))?;
        self.await_reply(receiver)?.map_err(io::Error::other)
    }
    fn transfer(
        &mut self,
        source: &str,
        destination: &str,
        allow_move: bool,
    ) -> io::Result<Option<String>> {
        self.check()?;
        if source == destination
            && self.request.source.same_storage(&self.request.destination)
            && self.request.kind == OperationKind::Move
        {
            return Ok(Some(destination.into()));
        }
        let before = self.request.source.stat(source)?;
        let mut destination = destination.to_owned();
        let existing = match self.request.destination.stat(&destination) {
            Ok(entry) => Some(entry),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        let mut replace = false;
        let mut merge = false;
        if let Some(existing) = existing {
            match self.resolve(source, &destination, &before, &existing)? {
                ConflictChoice::Skip => {
                    self.summary.skipped += 1;
                    return Ok(None);
                }
                ConflictChoice::KeepBoth => destination = self.unique(&destination)?,
                ConflictChoice::Replace => replace = true,
                ConflictChoice::Merge => merge = true,
            }
        }
        if allow_move
            && self.request.kind == OperationKind::Move
            && !replace
            && !merge
            && self.request.source.same_storage(&self.request.destination)
        {
            let result = self.request.source.call(Request::Rename {
                root: self.request.source.root(),
                from: source.into(),
                to: destination.clone(),
            });
            match result {
                Ok(_) => {
                    self.moved(source, &destination)?;
                    self.summary.completed += 1;
                    self.progress(source)?;
                    return Ok(Some(destination));
                }
                Err(error)
                    if error.raw_os_error() == Some(18)
                        || error.kind() == io::ErrorKind::CrossesDevices => {}
                Err(error) => return Err(error),
            }
        }
        if before.is_directory {
            let manifest = if allow_move && self.request.kind == OperationKind::Move {
                let mut manifest = Vec::new();
                self.capture_manifest(source, &mut manifest)?;
                Some(manifest)
            } else {
                None
            };
            if !merge {
                self.request.destination.call(Request::CreateDirectory {
                    root: self.request.destination.root(),
                    path: destination.clone(),
                })?;
            }
            let mut all = true;
            for child in self.request.source.children(source)? {
                self.check()?;
                match self.transfer(&child, &join(&destination, filename(&child)?), false) {
                    Ok(done) => all &= done.is_some(),
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => return Err(error),
                    Err(error) => {
                        all = false;
                        self.summary.errors.push(format!("{child}: {error}"));
                    }
                }
            }
            if all {
                self.copied_paths.insert(source.into(), destination.clone());
            }
            if all && let Some(manifest) = manifest {
                // Every item in a selected folder must be copied and verified
                // before originals are eligible for removal.
                for (path, expected) in &manifest {
                    self.check()?;
                    if self.request.source.stat(path)? != *expected {
                        return Err(io::Error::new(
                            io::ErrorKind::WouldBlock,
                            "Source changed during transfer; originals retained",
                        ));
                    }
                }
                for (path, expected) in manifest.into_iter().rev() {
                    self.check()?;
                    if expected.is_directory {
                        self.request.source.call(Request::RemoveEmptyDirectory {
                            root: self.request.source.root(),
                            path: path.clone(),
                        })?;
                    } else {
                        if self.request.source.stat(&path)? != expected {
                            return Err(io::Error::new(
                                io::ErrorKind::WouldBlock,
                                "Source changed during cleanup; remaining originals retained",
                            ));
                        }
                        self.request.source.remove(&path, false)?;
                    }
                    self.moved(&path, self.copied_paths.get(&path).ok_or_else(unexpected)?)?;
                }
            }
            self.summary.completed += 1;
            self.progress(source)?;
            return Ok(all.then_some(destination));
        }
        let temporary = temporary_sibling(&destination);
        let transfer = (|| {
            if let Some(target) = &before.symlink_target {
                self.request.destination.call(Request::CreateSymlink {
                    root: self.request.destination.root(),
                    path: temporary.clone(),
                    target: target.clone(),
                })?;
                if self.request.source.stat(source)? != before {
                    return Err(io::Error::new(
                        io::ErrorKind::WouldBlock,
                        "Source changed during transfer; original retained",
                    ));
                }
            } else {
                let mut offset = 0;
                loop {
                    self.check()?;
                    let bytes = match self.request.source.call(Request::ReadFileChunk {
                        root: self.request.source.root(),
                        path: source.into(),
                        offset,
                        max_bytes: TRANSFER_CHUNK_BYTES,
                    })? {
                        Response::FileChunk { bytes } => bytes,
                        _ => return Err(unexpected()),
                    };
                    let len = bytes.len();
                    self.request.destination.call(Request::WriteFileChunk {
                        root: self.request.destination.root(),
                        path: temporary.clone(),
                        offset,
                        bytes,
                        create: offset == 0,
                        // Preserve read-only/executable permissions only after
                        // writing the last chunk, while the handle is writable.
                        mode: (offset + len as u64 >= before.len).then_some(before.mode),
                    })?;
                    offset += len as u64;
                    self.summary.bytes += len as u64;
                    self.progress(source)?;
                    if len < TRANSFER_CHUNK_BYTES || offset == before.len {
                        break;
                    }
                }
                let after = self.request.source.stat(source)?;
                if before != after || offset != before.len {
                    return Err(io::Error::new(
                        io::ErrorKind::WouldBlock,
                        "Source changed during transfer; original retained",
                    ));
                }
            }
            self.check()?;
            if replace {
                self.acknowledge(|reply| OperationEvent::BeforeReplace {
                    path: destination.clone(),
                    reply,
                })?;
            }
            self.check()?;
            self.request.destination.call(Request::CommitFileTransfer {
                root: self.request.destination.root(),
                temporary: temporary.clone(),
                path: destination.clone(),
                replace,
            })?;
            Ok(())
        })();
        if let Err(error) = transfer {
            let _ = self.request.destination.remove(&temporary, false);
            return Err(error);
        }
        self.copied_paths.insert(source.into(), destination.clone());
        if allow_move && self.request.kind == OperationKind::Move {
            self.check()?;
            if self.request.source.stat(source)? != before {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "Source changed during transfer; original retained",
                ));
            }
            self.request.source.remove(source, false)?;
            self.moved(source, &destination)?;
        }
        self.summary.completed += 1;
        self.progress(source)?;
        Ok(Some(destination))
    }
    fn capture_manifest(
        &self,
        path: &str,
        manifest: &mut Vec<(String, TransferEntry)>,
    ) -> io::Result<()> {
        self.check()?;
        let entry = self.request.source.stat(path)?;
        let directory = entry.is_directory;
        manifest.push((path.into(), entry));
        if directory {
            for child in self.request.source.children(path)? {
                self.capture_manifest(&child, manifest)?;
            }
        }
        Ok(())
    }
    fn moved(&self, source: &str, destination: &str) -> io::Result<()> {
        self.acknowledge(|reply| OperationEvent::Moved {
            source: source.into(),
            destination: destination.into(),
            reply,
        })
    }
    fn progress(&self, path: &str) -> io::Result<()> {
        self.send(OperationEvent::Progress {
            path: path.into(),
            completed: self.summary.completed,
            bytes: self.summary.bytes,
        })
    }
}

pub fn top_level_paths(paths: &[String]) -> Vec<String> {
    let mut paths = paths.to_vec();
    paths.sort();
    paths.dedup();
    let mut result: Vec<String> = Vec::new();
    for path in paths {
        if !result.iter().any(|parent| contains(parent, &path)) {
            result.push(path);
        }
    }
    result
}
fn contains(parent: &str, child: &str) -> bool {
    child == parent
        || child
            .strip_prefix(parent.trim_end_matches('/'))
            .is_some_and(|suffix| suffix.starts_with('/'))
}
fn filename(path: &str) -> io::Result<&str> {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty() && *name != "." && *name != "..")
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Missing filename"))
}
fn parent(path: &str) -> String {
    path.rsplit_once('/').map_or_else(
        || ".".into(),
        |(parent, _)| {
            if parent.is_empty() {
                "/".into()
            } else {
                parent.into()
            }
        },
    )
}
fn join(directory: &str, name: &str) -> String {
    format!("{}/{name}", directory.trim_end_matches('/'))
}
fn unexpected() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "Unexpected file transfer response",
    )
}
fn temporary_sibling(path: &str) -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    join(
        &parent(path),
        &format!(
            ".bed-transfer-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Seek, SeekFrom, Write};

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "bed-operations-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&root).unwrap();
            Self(root.canonicalize().unwrap())
        }
        fn path(&self, path: &str) -> String {
            self.0.join(path).to_str().unwrap().into()
        }
        fn file(&self, path: &str, bytes: &[u8]) -> String {
            let path = self.0.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, bytes).unwrap();
            path.to_str().unwrap().into()
        }
        fn directory(&self, path: &str) -> String {
            fs::create_dir_all(self.0.join(path)).unwrap();
            self.path(path)
        }
        fn request(
            &self,
            kind: OperationKind,
            paths: Vec<String>,
            directory: &str,
        ) -> OperationRequest {
            OperationRequest {
                kind,
                source: Endpoint::local(&self.0),
                destination: Endpoint::local(&self.0),
                paths,
                directory: self.path(directory),
                duplicate: false,
                trash: None,
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn drive(
        job: &OperationJob,
        mut handle: impl FnMut(&OperationJob, OperationEvent),
    ) -> OperationSummary {
        loop {
            let event = job
                .events
                .recv_timeout(Duration::from_secs(10))
                .expect("filesystem job stalled");
            match event {
                OperationEvent::Finished(summary) => return summary,
                event => handle(job, event),
            }
        }
    }
    fn finish(request: OperationRequest, choices: &[ConflictChoice]) -> OperationSummary {
        let job = OperationJob::start(request);
        let mut choices = choices.iter();
        let summary = drive(&job, |_, event| match event {
            OperationEvent::Conflict(conflict) => conflict
                .reply
                .send(ConflictDecision {
                    choice: *choices.next().expect("unexpected conflict"),
                    apply_to_all: false,
                })
                .unwrap(),
            OperationEvent::BeforeReplace { reply, .. } | OperationEvent::Moved { reply, .. } => {
                reply.send(Ok(())).unwrap()
            }
            _ => {}
        });
        assert!(choices.next().is_none(), "unused conflict choice");
        summary
    }
    fn no_temporaries(path: &Path) {
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            assert!(
                !entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".bed-transfer-")
            );
            if entry.file_type().unwrap().is_dir() {
                no_temporaries(&entry.path());
            }
        }
    }

    #[test]
    fn duplicate_keeps_each_selected_items_parent_and_deduplicates_children() {
        let fx = Fixture::new();
        let first = fx.file("first/a.txt", b"a");
        let second = fx.file("second/b.txt", b"b");
        let mut request = fx.request(OperationKind::Copy, vec![first, second], "first");
        request.duplicate = true;
        let summary = finish(request, &[]);
        assert!(summary.errors.is_empty(), "{:?}", summary.errors);
        assert_eq!(fs::read(fx.path("first/a copy.txt")).unwrap(), b"a");
        assert_eq!(fs::read(fx.path("second/b copy.txt")).unwrap(), b"b");
        assert_eq!(
            summary.destinations,
            [fx.path("first/a copy.txt"), fx.path("second/b copy.txt")]
        );
        assert_eq!(
            top_level_paths(&[
                fx.path("first"),
                fx.path("first/a.txt"),
                fx.path("first"),
                fx.path("first-other")
            ]),
            [fx.path("first"), fx.path("first-other")]
        );
    }

    #[test]
    fn keep_both_reports_the_actual_destination_and_keeps_the_original() {
        let fx = Fixture::new();
        let source = fx.file("source/a.txt", b"source");
        fx.file("destination/a.txt", b"destination");
        let summary = finish(
            fx.request(OperationKind::Copy, vec![source], "destination"),
            &[ConflictChoice::KeepBoth],
        );
        assert!(summary.errors.is_empty());
        assert_eq!(summary.destinations, [fx.path("destination/a copy.txt")]);
        assert_eq!(
            fs::read(fx.path("destination/a.txt")).unwrap(),
            b"destination"
        );
        assert_eq!(
            fs::read(fx.path("destination/a copy.txt")).unwrap(),
            b"source"
        );
    }

    #[test]
    fn merge_preserves_unrelated_contents_and_resolves_nested_files() {
        let fx = Fixture::new();
        fx.file("source/folder/a", b"source a");
        fx.file("source/folder/b", b"source b");
        fx.file("source/folder/c", b"source c");
        fx.file("destination/folder/a", b"old a");
        fx.file("destination/folder/b", b"old b");
        fx.file("destination/folder/c", b"old c");
        fx.file("destination/folder/unrelated", b"keep");
        let summary = finish(
            fx.request(
                OperationKind::Copy,
                vec![fx.path("source/folder")],
                "destination",
            ),
            &[
                ConflictChoice::Merge,
                ConflictChoice::Replace,
                ConflictChoice::KeepBoth,
                ConflictChoice::Skip,
            ],
        );
        assert!(summary.errors.is_empty(), "{:?}", summary.errors);
        assert_eq!(summary.skipped, 1);
        assert_eq!(
            fs::read(fx.path("destination/folder/a")).unwrap(),
            b"source a"
        );
        assert_eq!(fs::read(fx.path("destination/folder/b")).unwrap(), b"old b");
        assert_eq!(
            fs::read(fx.path("destination/folder/b copy")).unwrap(),
            b"source b"
        );
        assert_eq!(fs::read(fx.path("destination/folder/c")).unwrap(), b"old c");
        assert_eq!(
            fs::read(fx.path("destination/folder/unrelated")).unwrap(),
            b"keep"
        );
        no_temporaries(&fx.0);
    }

    #[test]
    fn apply_to_all_file_replacements_acknowledge_each_destination() {
        let fx = Fixture::new();
        let a = fx.file("source/a", b"new a");
        let b = fx.file("source/b", b"new b");
        fx.file("destination/a", b"old a");
        fx.file("destination/b", b"old b");
        let job = OperationJob::start(fx.request(OperationKind::Copy, vec![a, b], "destination"));
        let mut conflicts = 0;
        let mut replacements = 0;
        let summary = drive(&job, |_, event| match event {
            OperationEvent::Conflict(conflict) => {
                conflicts += 1;
                conflict
                    .reply
                    .send(ConflictDecision {
                        choice: ConflictChoice::Replace,
                        apply_to_all: true,
                    })
                    .unwrap();
            }
            OperationEvent::BeforeReplace { reply, .. } => {
                replacements += 1;
                reply.send(Ok(())).unwrap();
            }
            _ => {}
        });
        assert!(summary.errors.is_empty());
        assert_eq!((conflicts, replacements), (1, 2));
        assert_eq!(fs::read(fx.path("destination/a")).unwrap(), b"new a");
        assert_eq!(fs::read(fx.path("destination/b")).unwrap(), b"new b");
    }

    #[test]
    fn rejected_replacement_leaves_both_files_and_cleans_temporary() {
        let fx = Fixture::new();
        let source = fx.file("source/a", b"new");
        fx.file("destination/a", b"old");
        let job = OperationJob::start(fx.request(
            OperationKind::Move,
            vec![source.clone()],
            "destination",
        ));
        let summary = drive(&job, |_, event| match event {
            OperationEvent::Conflict(conflict) => conflict
                .reply
                .send(ConflictDecision {
                    choice: ConflictChoice::Replace,
                    apply_to_all: false,
                })
                .unwrap(),
            OperationEvent::BeforeReplace { reply, .. } => {
                reply.send(Err("draft validation failed".into())).unwrap()
            }
            _ => {}
        });
        assert_eq!(summary.errors.len(), 1);
        assert_eq!(fs::read(source).unwrap(), b"new");
        assert_eq!(fs::read(fx.path("destination/a")).unwrap(), b"old");
        no_temporaries(&fx.0);
    }

    #[test]
    fn canceled_folder_transfer_preserves_all_originals_and_completed_copies() {
        let fx = Fixture::new();
        let destination = Fixture::new();
        fx.file("folder/a", b"first");
        let second = fx.file("folder/b", &vec![7; TRANSFER_CHUNK_BYTES * 4]);
        let mut request = fx.request(OperationKind::Move, vec![fx.path("folder")], "");
        request.destination = Endpoint::local(&destination.0);
        request.directory = destination.path("");
        let job = OperationJob::start(request);
        let summary = drive(&job, |job, event| match event {
            OperationEvent::Progress { path, .. } if path == second => job.cancel(),
            OperationEvent::Moved { .. } => {
                panic!("originals moved before the selected folder finished copying")
            }
            _ => {}
        });
        assert!(summary.canceled);
        assert_eq!(fs::read(fx.path("folder/a")).unwrap(), b"first");
        assert_eq!(
            fs::metadata(&second).unwrap().len(),
            (TRANSFER_CHUNK_BYTES * 4) as u64
        );
        assert_eq!(fs::read(destination.path("folder/a")).unwrap(), b"first");
        no_temporaries(&destination.0);
    }

    #[test]
    fn folder_move_merges_and_follows_keep_both_destinations() {
        let fx = Fixture::new();
        fx.file("source/folder/a", b"source");
        fx.file("destination/folder/a", b"destination");
        let job = OperationJob::start(fx.request(
            OperationKind::Move,
            vec![fx.path("source/folder")],
            "destination",
        ));
        let mut moved = Vec::new();
        let summary = drive(&job, |_, event| match event {
            OperationEvent::Conflict(conflict) => conflict
                .reply
                .send(ConflictDecision {
                    choice: if conflict.source_directory {
                        ConflictChoice::Merge
                    } else {
                        ConflictChoice::KeepBoth
                    },
                    apply_to_all: false,
                })
                .unwrap(),
            OperationEvent::Moved {
                source,
                destination,
                reply,
            } => {
                moved.push((source, destination));
                reply.send(Ok(())).unwrap();
            }
            _ => {}
        });
        assert!(summary.errors.is_empty(), "{:?}", summary.errors);
        assert!(!Path::new(&fx.path("source/folder")).exists());
        assert!(moved.contains(&(
            fx.path("source/folder/a"),
            fx.path("destination/folder/a copy")
        )));
        assert_eq!(
            fs::read(fx.path("destination/folder/a")).unwrap(),
            b"destination"
        );
        assert_eq!(
            fs::read(fx.path("destination/folder/a copy")).unwrap(),
            b"source"
        );
    }

    #[test]
    fn a_skipped_child_retains_all_originals_in_a_merged_move() {
        let fx = Fixture::new();
        fx.file("source/folder/a", b"source a");
        fx.file("source/folder/b", b"source b");
        fx.file("destination/folder/b", b"destination b");
        let summary = finish(
            fx.request(
                OperationKind::Move,
                vec![fx.path("source/folder")],
                "destination",
            ),
            &[ConflictChoice::Merge, ConflictChoice::Skip],
        );
        assert!(summary.errors.is_empty());
        assert_eq!(summary.skipped, 1);
        assert_eq!(fs::read(fx.path("source/folder/a")).unwrap(), b"source a");
        assert_eq!(fs::read(fx.path("source/folder/b")).unwrap(), b"source b");
        assert_eq!(
            fs::read(fx.path("destination/folder/a")).unwrap(),
            b"source a"
        );
    }

    #[test]
    fn source_changes_while_a_move_waits_for_conflict_keep_originals() {
        let fx = Fixture::new();
        fx.file("source/folder/a", b"source a");
        fx.file("source/folder/b", b"source b");
        fx.file("destination/folder/b", b"destination b");
        let job = OperationJob::start(fx.request(
            OperationKind::Move,
            vec![fx.path("source/folder")],
            "destination",
        ));
        let summary = drive(&job, |_, event| match event {
            OperationEvent::Conflict(conflict) => {
                if !conflict.source_directory {
                    fs::write(fx.path("source/folder/a"), b"edited after copy").unwrap();
                }
                conflict
                    .reply
                    .send(ConflictDecision {
                        choice: if conflict.source_directory {
                            ConflictChoice::Merge
                        } else {
                            ConflictChoice::Replace
                        },
                        apply_to_all: false,
                    })
                    .unwrap();
            }
            OperationEvent::BeforeReplace { reply, .. } => reply.send(Ok(())).unwrap(),
            OperationEvent::Moved { .. } => panic!("changed originals must remain"),
            _ => {}
        });
        assert_eq!(summary.errors.len(), 1);
        assert_eq!(
            fs::read(fx.path("source/folder/a")).unwrap(),
            b"edited after copy"
        );
        assert_eq!(fs::read(fx.path("source/folder/b")).unwrap(), b"source b");
        no_temporaries(&fx.0);
    }

    #[cfg(unix)]
    #[test]
    fn symbolic_links_are_copied_without_traversal_even_when_pointing_at_parent() {
        let fx = Fixture::new();
        let destination = fx.directory("destination");
        std::os::unix::fs::symlink(&fx.0, fx.path("link")).unwrap();
        let summary = finish(
            fx.request(OperationKind::Copy, vec![fx.path("link")], "destination"),
            &[],
        );
        assert!(summary.errors.is_empty(), "{:?}", summary.errors);
        assert_eq!(
            fs::read_link(Path::new(&destination).join("link")).unwrap(),
            fx.0
        );
        assert!(
            fs::symlink_metadata(fx.path("link"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn transfers_can_copy_a_file_larger_than_the_editor_limit() {
        let fx = Fixture::new();
        let source = fx.file("source/large", b"");
        let destination = fx.directory("destination");
        let size = bed_remote::MAX_FILE_BYTES as u64 + 17;
        let mut file = fs::OpenOptions::new().write(true).open(&source).unwrap();
        file.set_len(size).unwrap();
        file.seek(SeekFrom::End(-4)).unwrap();
        file.write_all(b"tail").unwrap();
        drop(file);
        let summary = finish(
            fx.request(OperationKind::Copy, vec![source], "destination"),
            &[],
        );
        assert!(summary.errors.is_empty(), "{:?}", summary.errors);
        assert_eq!(summary.bytes, size);
        let mut output = fs::File::open(Path::new(&destination).join("large")).unwrap();
        assert_eq!(output.metadata().unwrap().len(), size);
        output.seek(SeekFrom::End(-4)).unwrap();
        let mut tail = [0; 4];
        std::io::Read::read_exact(&mut output, &mut tail).unwrap();
        assert_eq!(&tail, b"tail");
        no_temporaries(&fx.0);
    }

    #[cfg(unix)]
    #[test]
    fn readonly_files_remain_writable_to_the_transfer_until_the_final_chunk() {
        use std::os::unix::fs::PermissionsExt;
        let fx = Fixture::new();
        let source = fx.file("source/readonly", &vec![3; TRANSFER_CHUNK_BYTES + 17]);
        fx.directory("destination");
        fs::set_permissions(&source, fs::Permissions::from_mode(0o444)).unwrap();
        let summary = finish(
            fx.request(OperationKind::Copy, vec![source], "destination"),
            &[],
        );
        assert!(summary.errors.is_empty(), "{:?}", summary.errors);
        let metadata = fs::metadata(fx.path("destination/readonly")).unwrap();
        assert_eq!(metadata.len(), TRANSFER_CHUNK_BYTES as u64 + 17);
        assert_eq!(metadata.permissions().mode() & 0o777, 0o444);
    }

    #[test]
    fn root_mutations_and_moves_into_a_source_subtree_are_rejected() {
        let fx = Fixture::new();
        fx.file("folder/a", b"keep");
        fx.directory("folder/child");
        let summary = finish(
            fx.request(OperationKind::Move, vec![fx.path("folder")], "folder/child"),
            &[],
        );
        assert_eq!(summary.errors.len(), 1);
        assert_eq!(fs::read(fx.path("folder/a")).unwrap(), b"keep");
        let summary = finish(
            fx.request(OperationKind::Delete, vec![fx.path("")], ""),
            &[],
        );
        assert_eq!(summary.errors.len(), 1);
        assert!(fx.0.is_dir());
    }
}
