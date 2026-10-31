//! Headless file access. Call blocking operations from a host-owned worker.
//! Filesystem identity and native paths identify sources; documents own content.
mod document;
mod local;
#[cfg(target_os = "macos")]
mod mac_watch;
pub mod operations;
pub use document::*;
pub mod watch;
pub use local::LocalFilesystem;

use std::{
    fmt,
    io::{self, Read, Seek},
    ops::Range,
    path::{Path, PathBuf},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::SystemTime,
};

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    Changed,
    CommitUncertain(io::Error),
    TooLarge { limit: u64 },
}
impl Error {
    pub fn kind(&self) -> io::ErrorKind {
        match self {
            Self::Io(error) | Self::CommitUncertain(error) => error.kind(),
            Self::Changed => io::ErrorKind::Other,
            Self::TooLarge { .. } => io::ErrorKind::InvalidData,
        }
    }
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => error.fmt(f),
            Self::CommitUncertain(error) => write!(
                f,
                "Write may have committed; refresh before retrying: {error}"
            ),
            Self::Changed => f.write_str("File changed during access. Reload to refresh."),
            Self::TooLarge { limit } => write!(f, "File exceeds the {limit} byte source limit"),
        }
    }
}
impl std::error::Error for Error {}
impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}
impl From<Error> for io::Error {
    fn from(error: Error) -> Self {
        io::Error::new(error.kind(), error)
    }
}
pub type Result<T> = std::result::Result<T, Error>;

/// Provider-owned equality token. It is neither a document revision nor a promise
/// of a locked snapshot. Tokens are scoped to the producing filesystem/source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileVersion(Vec<u8>);
impl FileVersion {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Directory,
    Symlink,
    Other,
}
#[derive(Clone, Debug)]
pub struct Metadata {
    pub kind: EntryKind,
    pub len: u64,
    pub modified: Option<SystemTime>,
    pub readonly: bool,
    pub version: FileVersion,
}
impl Metadata {
    pub fn is_file(&self) -> bool {
        self.kind == EntryKind::File
    }
    pub fn is_dir(&self) -> bool {
        self.kind == EntryKind::Directory
    }
}

/// An open, observed source. `verify` rejects changes to either the opened
/// object or its path. Seek/read remain worker-only and never mutate documents.
pub trait FileReader: Read + Seek + Send {
    fn metadata(&self) -> &Metadata;
    fn verify(&self) -> Result<()>;
}

/// Bytes have been staged, but the destination has not changed. Dropping this
/// value abandons staging. Commit compares exact baseline bytes before replacing.
pub trait PendingWrite: Send {
    fn commit(self: Box<Self>, expected: WriteExpectation<'_>) -> Result<Metadata>;
}
pub enum WriteExpectation<'a> {
    Missing,
    Contents(&'a mut dyn Read),
    /// The target still has this version (as last read or written).
    Version(&'a FileVersion),
}

#[derive(Clone, Debug)]
pub struct DirectoryEntry {
    pub path: PathBuf,
    pub name: std::ffi::OsString,
    pub kind: EntryKind,
    pub hidden: bool,
}

/// Synchronous filesystem boundary. Scheduling, admission and document policy
/// belong to callers. Native implementation is supplied; no SSH transport runs.
pub trait FilesystemBackend: Send + Sync + 'static {
    fn canonicalize(&self, path: &Path) -> Result<PathBuf>;
    fn metadata(&self, path: &Path) -> Result<Metadata>;
    fn entry_metadata(&self, path: &Path) -> Result<Metadata>;
    fn read_directory(
        &self,
        path: &Path,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Vec<DirectoryEntry>>;
    fn open(&self, path: &Path) -> Result<Box<dyn FileReader>>;
    fn prepare_write(&self, path: &Path, content: &mut dyn Read) -> Result<Box<dyn PendingWrite>>;
}

#[derive(Clone)]
pub struct Filesystem {
    id: u64,
    backend: Arc<dyn FilesystemBackend>,
    documents: Arc<
        std::sync::Mutex<
            std::collections::BTreeMap<PathBuf, std::sync::Weak<document::FileDocumentState>>,
        >,
    >,
}
impl Filesystem {
    pub fn new(backend: impl FilesystemBackend) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self {
            id: NEXT
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
                .expect("Filesystem identities exhausted"),
            backend: Arc::new(backend),
            documents: Default::default(),
        }
    }
    /// All local handles share one identity, including independently created ones.
    pub fn local() -> Self {
        static LOCAL: OnceLock<Filesystem> = OnceLock::new();
        LOCAL.get_or_init(|| Self::new(LocalFilesystem)).clone()
    }
    pub fn source(&self, path: impl Into<PathBuf>) -> FileSource {
        FileSource {
            filesystem: self.clone(),
            path: path.into(),
        }
    }
}

#[derive(Clone)]
pub struct FileSource {
    filesystem: Filesystem,
    path: PathBuf,
}
impl fmt::Debug for FileSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FileSource")
            .field("filesystem", &self.filesystem.id)
            .field("path", &self.path)
            .finish()
    }
}
impl PartialEq for FileSource {
    fn eq(&self, other: &Self) -> bool {
        self.filesystem.id == other.filesystem.id && self.path == other.path
    }
}
impl Eq for FileSource {}
impl PartialOrd for FileSource {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for FileSource {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.filesystem.id, &self.path).cmp(&(other.filesystem.id, &other.path))
    }
}
impl std::hash::Hash for FileSource {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.filesystem.id.hash(state);
        self.path.hash(state);
    }
}

pub struct ReadSnapshot {
    pub bytes: Vec<u8>,
    pub metadata: Metadata,
}
impl FileSource {
    pub fn local(path: impl Into<PathBuf>) -> Self {
        Filesystem::local().source(path)
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn canonicalize(&self) -> Result<Self> {
        Ok(self
            .filesystem
            .source(self.filesystem.backend.canonicalize(&self.path)?))
    }
    pub fn metadata(&self) -> Result<Metadata> {
        self.filesystem.backend.metadata(&self.path)
    }
    pub fn entry_metadata(&self) -> Result<Metadata> {
        self.filesystem.backend.entry_metadata(&self.path)
    }
    pub fn read_directory(&self, cancelled: &dyn Fn() -> bool) -> Result<Vec<DirectoryEntry>> {
        self.filesystem
            .backend
            .read_directory(&self.path, cancelled)
    }
    pub fn open(&self) -> Result<Box<dyn FileReader>> {
        self.filesystem.backend.open(&self.path)
    }
    pub fn read(&self, limit: u64) -> Result<ReadSnapshot> {
        let mut reader = self.open()?;
        let metadata = reader.metadata().clone();
        if metadata.len > limit {
            return Err(Error::TooLarge { limit });
        }
        let mut bytes = Vec::new();
        reader
            .by_ref()
            .take(limit.saturating_add(1))
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > limit {
            return Err(Error::TooLarge { limit });
        }
        reader.verify()?;
        Ok(ReadSnapshot { bytes, metadata })
    }
    pub fn read_prefix(&self, limit: u64) -> Result<ReadSnapshot> {
        let mut reader = self.open()?;
        let metadata = reader.metadata().clone();
        let mut bytes = Vec::new();
        reader.by_ref().take(limit).read_to_end(&mut bytes)?;
        reader.verify()?;
        Ok(ReadSnapshot { bytes, metadata })
    }
    pub fn read_range(&self, version: &FileVersion, range: Range<u64>) -> Result<Vec<u8>> {
        let mut reader = self.open()?;
        if &reader.metadata().version != version {
            return Err(Error::Changed);
        }
        if range.start > range.end || range.end > reader.metadata().len {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "Invalid file range").into());
        }
        let len = usize::try_from(range.end - range.start).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "File range exceeds address space",
            )
        })?;
        reader.seek(io::SeekFrom::Start(range.start))?;
        let mut bytes = vec![0; len];
        reader.read_exact(&mut bytes).map_err(|error| {
            if error.kind() == io::ErrorKind::UnexpectedEof {
                Error::Changed
            } else {
                error.into()
            }
        })?;
        reader.verify()?;
        Ok(bytes)
    }
    pub fn matches(&self, expected: &mut dyn Read) -> Result<bool> {
        let mut reader = self.open()?;
        let equal = equal_bytes(&mut reader, expected)?;
        reader.verify()?;
        Ok(equal)
    }
    pub fn prepare_write(&self, content: &mut dyn Read) -> Result<Box<dyn PendingWrite>> {
        self.filesystem.backend.prepare_write(&self.path, content)
    }
}

/// Incremental comparison tolerates readers returning different chunk lengths.
fn equal_bytes(left: &mut dyn Read, right: &mut dyn Read) -> io::Result<bool> {
    let mut a = [0; 8192];
    let mut b = [0; 8192];
    loop {
        let count = left.read(&mut a)?;
        if count == 0 {
            return Ok(right.read(&mut b[..1])? == 0);
        }
        match right.read_exact(&mut b[..count]) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(false),
            Err(error) => return Err(error),
        }
        if a[..count] != b[..count] {
            return Ok(false);
        }
    }
}

/// Stream an existing chunked model without collecting another full byte buffer.
pub struct ChunkReader<'a, I: Iterator<Item = &'a [u8]>> {
    chunks: I,
    current: &'a [u8],
}
impl<'a, I: Iterator<Item = &'a [u8]>> ChunkReader<'a, I> {
    pub fn new(chunks: I) -> Self {
        Self {
            chunks,
            current: &[],
        }
    }
}
impl<'a, I: Iterator<Item = &'a [u8]>> Read for ChunkReader<'a, I> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        while self.current.is_empty() {
            let Some(chunk) = self.chunks.next() else {
                return Ok(0);
            };
            self.current = chunk;
        }
        self.current.read(buffer)
    }
}

#[cfg(test)]
mod tests;
