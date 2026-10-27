//! Canonical document sources. Native byte access remains independent of models.
use crate::{FileSource, FileVersion, Filesystem, Metadata, WriteExpectation};
use bedtime_document::*;
use std::{
    ops::Range,
    path::{Path, PathBuf},
    sync::{
        Arc, RwLock,
        atomic::{AtomicU64, Ordering},
    },
};

pub(crate) struct FileDocumentState {
    id: SourceId,
    path: RwLock<PathBuf>,
    generation: AtomicU64,
    deleted: std::sync::atomic::AtomicBool,
}
#[derive(Clone)]
pub struct FsDocumentSource {
    filesystem: Filesystem,
    state: Arc<FileDocumentState>,
}
impl std::fmt::Debug for FsDocumentSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FsDocumentSource")
            .field("id", &self.id())
            .field("path", &self.path())
            .finish()
    }
}
impl Filesystem {
    /// Worker-only: resolve aliases before interning a canonical source identity.
    pub fn document_source(&self, path: impl AsRef<Path>) -> crate::Result<FsDocumentSource> {
        let path = self.backend.canonicalize(path.as_ref())?;
        let mut sources = self.documents.lock().unwrap();
        sources.retain(|_, entry| entry.strong_count() != 0);
        let state = sources
            .get(&path)
            .and_then(std::sync::Weak::upgrade)
            .unwrap_or_else(|| {
                let state = Arc::new(FileDocumentState {
                    id: SourceId::new(),
                    path: RwLock::new(path.clone()),
                    generation: AtomicU64::new(0),
                    deleted: std::sync::atomic::AtomicBool::new(false),
                });
                sources.insert(path, Arc::downgrade(&state));
                state
            });
        Ok(FsDocumentSource {
            filesystem: self.clone(),
            state,
        })
    }
    /// Only confirmed native outcomes may transfer source identity.
    pub fn move_document_sources(&self, from: &Path, to: &Path) {
        let mut sources = self.documents.lock().unwrap();
        let moved: Vec<_> = sources
            .iter()
            .filter_map(|(path, source)| {
                let suffix = path.strip_prefix(from).ok()?;
                Some((
                    path.clone(),
                    if suffix.as_os_str().is_empty() {
                        to.to_owned()
                    } else {
                        to.join(suffix)
                    },
                    source.upgrade()?,
                ))
            })
            .collect();
        for (old, new, state) in moved {
            sources.remove(&old);
            *state.path.write().unwrap() = new.clone();
            state.generation.fetch_add(1, Ordering::AcqRel);
            sources.insert(new, Arc::downgrade(&state));
        }
    }
}
impl Filesystem {
    pub fn delete_document_sources(&self, path: &Path) {
        let mut sources = self.documents.lock().unwrap();
        let deleted: Vec<_> = sources
            .keys()
            .filter(|p| p.starts_with(path))
            .cloned()
            .collect();
        for path in deleted {
            if let Some(source) = sources.remove(&path).and_then(|s| s.upgrade()) {
                source.deleted.store(true, Ordering::Release);
                source.generation.fetch_add(1, Ordering::AcqRel);
            }
        }
    }
}

impl FsDocumentSource {
    pub fn is_deleted(&self) -> bool {
        self.state.deleted.load(Ordering::Acquire)
    }
    pub fn location_generation(&self) -> u64 {
        self.state.generation.load(Ordering::Acquire)
    }
    pub fn id(&self) -> SourceId {
        self.state.id
    }
    pub fn path(&self) -> PathBuf {
        self.state.path.read().unwrap().clone()
    }
    pub fn bytes(&self) -> FileSource {
        self.filesystem.source(self.path())
    }
    pub fn snapshot(&self) -> Result<Arc<dyn ByteSnapshot>, SourceError> {
        if self.is_deleted() {
            return Err(SourceError::Missing);
        }
        let source = self.bytes();
        let metadata = source.metadata().map_err(source_error)?;
        if !metadata.is_file() {
            return Err(SourceError::Failed("Source is not a regular file".into()));
        }
        Ok(Arc::new(FileSnapshot {
            source,
            metadata,
            _binding: self.state.clone(),
        }))
    }
}
fn version(version: &FileVersion) -> SourceVersion {
    SourceVersion(format!("{version:?}"))
}
pub(crate) fn source_error(error: crate::Error) -> SourceError {
    match error {
        crate::Error::Changed => SourceError::Changed("File changed on disk".into()),
        crate::Error::CommitUncertain(_) => SourceError::Uncertain,
        crate::Error::Io(e) => e.into(),
        e => SourceError::Failed(e.to_string()),
    }
}
struct FileSnapshot {
    source: FileSource,
    metadata: Metadata,
    _binding: Arc<FileDocumentState>,
}
impl ByteSnapshot for FileSnapshot {
    fn len(&self) -> u64 {
        self.metadata.len
    }
    fn version(&self) -> SourceVersion {
        version(&self.metadata.version)
    }
    fn read_range(&self, range: Range<u64>) -> Result<Vec<u8>, SourceError> {
        self.source
            .read_range(&self.metadata.version, range)
            .map_err(source_error)
    }
}
impl<M: ByteDocument> DocumentSource<M> for FsDocumentSource {
    fn id(&self) -> SourceId {
        self.id()
    }
    fn title(&self) -> String {
        let path = self.path();
        path.file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy()
            .into_owned()
    }
    fn path(&self) -> Option<PathBuf> {
        Some(self.path())
    }
    fn writable(&self) -> bool {
        M::writable()
    }
    fn write_blocked(&self) -> bool {
        WriteAccess::local().blocks(&self.path())
    }
    fn location_generation(&self) -> u64 {
        self.location_generation()
    }
    fn observation_interval(&self) -> Option<std::time::Duration> {
        Some(std::time::Duration::from_millis(500))
    }
    fn observed_version(&self) -> Result<Option<SourceVersion>, SourceError> {
        self.bytes()
            .metadata()
            .map(|m| Some(version(&m.version)))
            .map_err(source_error)
    }
    fn load(&self) -> Result<SourceLoad<M>, SourceError> {
        let snapshot = self.snapshot()?;
        let version = snapshot.version();
        Ok(SourceLoad {
            model: M::open(snapshot)?,
            version: Some(version),
            acknowledged: true,
        })
    }
    fn save(&self, snapshot: SourceSave<M>) -> Result<Option<SourceVersion>, SourceError> {
        if self.is_deleted() {
            return Err(SourceError::Missing);
        }
        if !M::writable() {
            return Err(SourceError::ReadOnly);
        }
        let generation = self.state.generation.load(Ordering::Acquire);
        let source = self.bytes();
        let _lease = WriteAccess::local()
            .file(source.path())
            .map_err(SourceError::Failed)?;
        if snapshot.expected_version.is_none() {
            return Err(SourceError::Changed(
                "Refresh the source before saving without a checkpoint".into(),
            ));
        }
        let baseline = snapshot
            .baseline
            .ok_or_else(|| SourceError::Changed("Source checkpoint is unavailable".into()))?;
        let pending = source
            .prepare_write(&mut snapshot.model.reader().map_err(SourceError::from)?)
            .map_err(source_error)?;
        if generation != self.state.generation.load(Ordering::Acquire) {
            return Err(SourceError::Changed("Source moved during save".into()));
        }
        let receipt = pending
            .commit(WriteExpectation::Contents(
                &mut baseline.reader().map_err(SourceError::from)?,
            ))
            .map_err(source_error)?;
        if generation != self.state.generation.load(Ordering::Acquire) {
            return Err(SourceError::Uncertain);
        }
        Ok(Some(version(&receipt.version)))
    }
}

/// Shared nonblocking admission for file and repository mutations.
#[derive(Clone, Default)]
pub struct WriteAccess(Arc<std::sync::Mutex<Vec<WriteScope>>>);
#[derive(Clone, PartialEq, Eq)]
enum WriteScope {
    File(PathBuf),
    Repository(PathBuf),
}
pub struct WriteLease {
    access: WriteAccess,
    scope: WriteScope,
}
impl WriteAccess {
    pub fn local() -> Self {
        static ACCESS: std::sync::OnceLock<WriteAccess> = std::sync::OnceLock::new();
        ACCESS.get_or_init(Self::default).clone()
    }
    pub fn file(&self, path: &Path) -> Result<WriteLease, String> {
        self.acquire(WriteScope::File(path.to_owned()))
    }
    pub fn repository(&self, path: &Path) -> Result<WriteLease, String> {
        self.acquire(WriteScope::Repository(path.to_owned()))
    }
    pub fn blocks(&self, path: &Path) -> bool {
        self.0
            .lock()
            .unwrap()
            .iter()
            .any(|s| matches!(s, WriteScope::Repository(root) if path.starts_with(root)))
    }
    fn acquire(&self, scope: WriteScope) -> Result<WriteLease, String> {
        let mut active = self.0.lock().unwrap();
        if active.iter().any(|other| match (&scope, other) {
            (WriteScope::File(a), WriteScope::File(b)) => a == b,
            (WriteScope::Repository(a), WriteScope::Repository(b)) => {
                a.starts_with(b) || b.starts_with(a)
            }
            (WriteScope::File(a), WriteScope::Repository(b))
            | (WriteScope::Repository(b), WriteScope::File(a)) => a.starts_with(b),
        }) {
            return Err("A save or filesystem operation is writing this source".into());
        }
        active.push(scope.clone());
        Ok(WriteLease {
            access: self.clone(),
            scope,
        })
    }
}
impl Drop for WriteLease {
    fn drop(&mut self) {
        let mut active = self.access.0.lock().unwrap();
        let i = active
            .iter()
            .position(|s| s == &self.scope)
            .expect("write lease owns admission");
        active.swap_remove(i);
    }
}
