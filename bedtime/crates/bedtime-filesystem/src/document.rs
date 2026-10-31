//! Files as document sources: one source per canonical path (moves keep it),
//! loaded and saved through a `FileModel`.
use crate::{FileSource, FileVersion, Filesystem, WriteExpectation};
use bedtime_document::*;
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc, RwLock,
        atomic::{AtomicU64, Ordering},
    },
};

pub(crate) struct FileDocumentState {
    id: SourceId,
    /// What was last loaded or saved: its version and a hash of its bytes.
    /// A version can change without the contents changing (a move changes a
    /// file's change time), so a save checks the contents then.
    known: std::sync::Mutex<Option<(SourceVersion, u64)>>,
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
                    known: std::sync::Mutex::new(None),
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
}

/// A document model kept in a file: how its bytes become the model and back.
pub trait FileModel: DocumentModel {
    /// Read the whole file, refusing files that aren't this model (binary
    /// data as text, say).
    fn decode(bytes: &[u8]) -> Result<Self, String>;
    /// Write the model out.
    fn encode(&self, out: &mut dyn Write) -> std::io::Result<()>;
    /// Files larger than this aren't opened as this model.
    const MAX_LEN: u64 = 256 * 1024 * 1024;
    /// The viewer reads the file itself (a video, played from disk): loading
    /// only checks it's there, and `decode` is given no bytes.
    const UNREAD: bool = false;
}

impl FileModel for TextBuffer {
    fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.contains(&0) {
            return Err("It looks like binary data, not text".into());
        }
        let text = std::str::from_utf8(bytes).map_err(|_| "It isn't UTF-8 text".to_owned())?;
        Ok(TextBuffer::from(text))
    }
    fn encode(&self, out: &mut dyn Write) -> std::io::Result<()> {
        for chunk in self.chunks() {
            out.write_all(chunk.as_bytes())?;
        }
        Ok(())
    }
}

/// A file's version as a document source version, and back.
fn version(version: &FileVersion) -> SourceVersion {
    let hex: String = version
        .as_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    SourceVersion(hex)
}
fn file_version(version: &SourceVersion) -> Option<FileVersion> {
    let hex = version.0.as_bytes();
    if !hex.len().is_multiple_of(2) {
        return None;
    }
    hex.chunks(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok())
        .collect::<Option<Vec<_>>>()
        .map(FileVersion::new)
}

pub(crate) fn source_error(error: crate::Error) -> SourceError {
    match error {
        crate::Error::Changed => SourceError::Changed,
        crate::Error::Io(e) if e.kind() == std::io::ErrorKind::NotFound => SourceError::Missing,
        e => SourceError::Failed(e.to_string()),
    }
}

impl<M: FileModel> DocumentSource<M> for FsDocumentSource {
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
    fn writable(&self) -> bool {
        !WriteAccess::local().blocks(&self.path())
    }
    fn check_interval(&self) -> Option<std::time::Duration> {
        Some(std::time::Duration::from_millis(500))
    }
    fn version(&self) -> Result<Option<SourceVersion>, SourceError> {
        if self.is_deleted() {
            return Err(SourceError::Missing);
        }
        self.bytes()
            .metadata()
            .map(|m| Some(version(&m.version)))
            .map_err(source_error)
    }
    fn load(&self) -> Result<SourceLoad<M>, SourceError> {
        if self.is_deleted() {
            return Err(SourceError::Missing);
        }
        let source = self.bytes();
        let metadata = source.metadata().map_err(source_error)?;
        if !metadata.is_file() {
            return Err(SourceError::Failed("It isn't a regular file".into()));
        }
        if M::UNREAD {
            let model = M::decode(&[]).map_err(SourceError::Failed)?;
            return Ok(SourceLoad {
                model,
                version: Some(version(&metadata.version)),
                saved: true,
            });
        }
        if metadata.len > M::MAX_LEN {
            return Err(SourceError::Failed("It's too large to open".into()));
        }
        let read = source.read(M::MAX_LEN).map_err(source_error)?;
        let model = M::decode(&read.bytes).map_err(SourceError::Failed)?;
        let loaded = version(&read.metadata.version);
        *self.state.known.lock().unwrap() = Some((loaded.clone(), hash(&read.bytes)));
        Ok(SourceLoad {
            model,
            version: Some(loaded),
            saved: true,
        })
    }
    fn save(
        &self,
        model: M,
        expected: Option<SourceVersion>,
    ) -> Result<Option<SourceVersion>, SourceError> {
        if self.is_deleted() {
            return Err(SourceError::Missing);
        }
        let generation = self.state.generation.load(Ordering::Acquire);
        let source = self.bytes();
        let _lease = WriteAccess::local()
            .file(source.path())
            .map_err(SourceError::Failed)?;
        let mut bytes = Vec::new();
        model
            .encode(&mut bytes)
            .map_err(|e| SourceError::Failed(e.to_string()))?;
        let written = hash(&bytes);
        let pending = source
            .prepare_write(&mut std::io::Cursor::new(bytes))
            .map_err(source_error)?;
        if generation != self.state.generation.load(Ordering::Acquire) {
            return Err(SourceError::Changed);
        }
        let receipt = match expected {
            // Never loaded from disk: it must not exist yet.
            None => pending.commit(WriteExpectation::Missing),
            Some(expected) => {
                let current = source.metadata().map_err(source_error)?;
                if Some(&current.version) == file_version(&expected).as_ref() {
                    pending.commit(WriteExpectation::Version(&current.version))
                } else {
                    // The version moved on; the contents may not have (a move,
                    // a permissions change). Save only over what was expected.
                    let known = self.state.known.lock().unwrap().clone();
                    let on_disk = source.read(M::MAX_LEN).map_err(source_error)?;
                    match known {
                        Some((known, contents))
                            if known == expected && contents == hash(&on_disk.bytes) =>
                        {
                            pending.commit(WriteExpectation::Contents(&mut std::io::Cursor::new(
                                on_disk.bytes,
                            )))
                        }
                        _ => return Err(SourceError::Changed),
                    }
                }
            }
        }
        .map_err(source_error)?;
        let saved = version(&receipt.version);
        *self.state.known.lock().unwrap() = Some((saved.clone(), written));
        Ok(Some(saved))
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

fn hash(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}
