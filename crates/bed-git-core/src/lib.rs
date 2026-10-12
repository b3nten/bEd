//! GUI-free, typed Git operations. Git owns index locks, hooks, signing and config.
//! Call `execute` from a worker, or use `Job`; neither API performs UI work.
mod process;
mod repository;

use serde::{Deserialize, Serialize};
use std::{
    fmt, io,
    path::Path,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
};

pub use repository::execute;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
pub enum DiffSide {
    Staged,
    Unstaged,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub enum RepositoryOperation {
    #[default]
    None,
    Merge,
    Rebase,
    CherryPick,
    Revert,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum ConflictChoice {
    Current,
    Incoming,
    Delete,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum Operation {
    Status,
    Diff {
        path: String,
        side: DiffSide,
    },
    /// An empty path list means all changes in the repository.
    Stage {
        paths: Vec<String>,
    },
    Unstage {
        paths: Vec<String>,
    },
    StageHunk {
        path: String,
        snapshot: String,
        hunk: usize,
    },
    UnstageHunk {
        path: String,
        snapshot: String,
        hunk: usize,
    },
    DiscardHunk {
        path: String,
        snapshot: String,
        hunk: usize,
    },
    /// Apply a displayed contiguous change block independently of Git's diff
    /// grouping. Ranges address zero-based LF-delimited rows in the snapshot.
    StageRange {
        path: String,
        snapshot: String,
        old_range: std::ops::Range<usize>,
        new_range: std::ops::Range<usize>,
    },
    UnstageRange {
        path: String,
        snapshot: String,
        old_range: std::ops::Range<usize>,
        new_range: std::ops::Range<usize>,
    },
    DiscardRange {
        path: String,
        snapshot: String,
        old_range: std::ops::Range<usize>,
        new_range: std::ops::Range<usize>,
    },
    Discard {
        path: String,
        snapshot: String,
    },
    Commit {
        message: String,
    },
    CreateBranch {
        name: String,
    },
    SwitchBranch {
        name: String,
    },
    Fetch,
    Pull,
    Push,
    Resolve {
        path: String,
        choice: ConflictChoice,
    },
    Continue,
    Abort,
}
impl Operation {
    pub fn is_mutating(&self) -> bool {
        !matches!(self, Self::Status | Self::Diff { .. })
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct Status {
    pub root: String,
    /// Writes require an explicitly opened repository-root workspace.
    pub writable: bool,
    pub branch: String,
    pub head: Option<String>,
    pub upstream: Option<String>,
    pub ahead: usize,
    pub behind: usize,
    pub operation: RepositoryOperation,
    pub entries: Vec<StatusEntry>,
    pub branches: Vec<String>,
    pub snapshot: String,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct StatusEntry {
    pub path: String,
    pub old_path: Option<String>,
    pub index_status: char,
    pub worktree_status: char,
    pub conflicted: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Added,
    Removed,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: LineKind,
    pub text: String,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Hunk {
    /// Zero-based row positions; an empty range is an insertion position.
    pub old_start: usize,
    pub old_count: usize,
    pub new_start: usize,
    pub new_count: usize,
    pub header: String,
    pub lines: Vec<DiffLine>,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Diff {
    pub path: String,
    pub old_path: Option<String>,
    pub side: DiffSide,
    pub snapshot: String,
    pub old_bytes: Vec<u8>,
    pub new_bytes: Vec<u8>,
    pub hunks: Vec<Hunk>,
    pub binary: bool,
    pub can_apply_hunks: bool,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum Output {
    Status(Status),
    Diff(Diff),
    Done { output: String },
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum ErrorKind {
    InvalidInput,
    NotRepository,
    PermissionDenied,
    Stale,
    Busy,
    Canceled,
    Failed,
    TooLarge,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Error {
    pub kind: ErrorKind,
    pub message: String,
    pub output: String,
}
impl Error {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            output: String::new(),
        }
    }
    pub fn into_io(self) -> io::Error {
        let kind = match self.kind {
            ErrorKind::InvalidInput | ErrorKind::TooLarge => io::ErrorKind::InvalidInput,
            ErrorKind::NotRepository => io::ErrorKind::NotFound,
            ErrorKind::PermissionDenied => io::ErrorKind::PermissionDenied,
            ErrorKind::Stale | ErrorKind::Busy => io::ErrorKind::WouldBlock,
            ErrorKind::Canceled => io::ErrorKind::Interrupted,
            ErrorKind::Failed => io::ErrorKind::Other,
        };
        io::Error::new(kind, self)
    }
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)?;
        if !self.output.is_empty() {
            write!(f, "\n{}", self.output)?;
        }
        Ok(())
    }
}
impl std::error::Error for Error {}
impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::new(ErrorKind::Failed, error.to_string())
    }
}
pub type Result<T> = std::result::Result<T, Error>;

/// One asynchronous operation. Dropping the job requests cancellation, including
/// child hooks. Completion can still mean a partial effect: always refresh Git.
pub struct Job {
    canceled: Arc<AtomicBool>,
    completed: mpsc::Receiver<Result<Output>>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Job {
    pub fn start(root: PathBuf, operation: Operation) -> Self {
        let canceled = Arc::new(AtomicBool::new(false));
        let stop = canceled.clone();
        let (sender, completed) = mpsc::sync_channel(1);
        let worker = thread::spawn(move || {
            let _ = sender.send(repository::execute_with_cancel(&root, &operation, stop));
        });
        Self {
            canceled,
            completed,
            worker: Some(worker),
        }
    }
    pub fn try_result(&mut self) -> Option<Result<Output>> {
        match self.completed.try_recv() {
            Ok(result) => Some(result),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                Some(Err(Error::new(ErrorKind::Failed, "Git worker stopped")))
            }
        }
    }
    pub fn cancel(&self) {
        self.canceled.store(true, Ordering::Release);
    }
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancel();
        if let Some(worker) = self.worker.take() {
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(200);
            while !worker.is_finished() && std::time::Instant::now() < deadline {
                thread::sleep(std::time::Duration::from_millis(2));
            }
            if worker.is_finished() {
                let _ = worker.join();
            }
        }
    }
}

/// Canonical repository metadata directory, used to serialize linked worktrees.
/// This performs Git I/O and belongs on a worker thread.
pub fn repository_key(root: &Path) -> Result<PathBuf> {
    repository::repository_key(root)
}
