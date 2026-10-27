use crate::DocumentModel;
use std::{
    fmt, io,
    ops::Range,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

/// Process-local logical identity. Locations and observed versions may change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SourceId(u64);
impl SourceId {
    pub fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(
            NEXT.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
                .expect("Source identities exhausted"),
        )
    }
}
impl Default for SourceId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SourceVersion(pub String);
impl From<&str> for SourceVersion {
    fn from(value: &str) -> Self {
        Self(value.into())
    }
}
impl From<String> for SourceVersion {
    fn from(value: String) -> Self {
        Self(value)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceError {
    Failed(String),
    Unsupported(String),
    Changed(String),
    Uncertain,
    Missing,
    ReadOnly,
}
impl fmt::Display for SourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Failed(s) | Self::Unsupported(s) | Self::Changed(s) => f.write_str(s),
            Self::Uncertain => f.write_str("Source write outcome is uncertain"),
            Self::Missing => f.write_str("Source no longer exists"),
            Self::ReadOnly => f.write_str("Source is read-only"),
        }
    }
}
impl std::error::Error for SourceError {}

pub struct SourceLoad<M> {
    pub model: M,
    pub version: Option<SourceVersion>,
    pub acknowledged: bool,
}
pub struct SourceSave<M> {
    pub model: M,
    pub baseline: Option<M>,
    pub expected_version: Option<SourceVersion>,
}

/// A provider owns I/O and delegates representation conversion to the model.
/// Implementations must check the captured baseline before publishing a write.
pub trait DocumentSource<M: DocumentModel>: Send + Sync + 'static {
    fn id(&self) -> SourceId;
    fn title(&self) -> String;
    fn load(&self) -> Result<SourceLoad<M>, SourceError>;
    fn writable(&self) -> bool {
        true
    }
    fn write_blocked(&self) -> bool {
        false
    }
    fn location_generation(&self) -> u64 {
        0
    }
    fn save(&self, snapshot: SourceSave<M>) -> Result<Option<SourceVersion>, SourceError>;
    fn path(&self) -> Option<std::path::PathBuf> {
        None
    }
    /// Cheap source observation performed on a worker. None opts out of polling.
    fn observed_version(&self) -> Result<Option<SourceVersion>, SourceError> {
        Ok(None)
    }
    fn observation_interval(&self) -> Option<std::time::Duration> {
        None
    }
}

/// Version-bound range access. A read must fail if its source version changed.
pub trait ByteSnapshot: Send + Sync + 'static {
    fn len(&self) -> u64;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    fn version(&self) -> SourceVersion;
    fn read_range(&self, range: Range<u64>) -> Result<Vec<u8>, SourceError>;
}

/// A read-only representation; workers perform bounded reads through its snapshot.
#[derive(Clone)]
pub struct BinaryDocument {
    pub snapshot: Arc<dyn ByteSnapshot>,
}
impl PartialEq for BinaryDocument {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.snapshot, &other.snapshot)
    }
}
impl DocumentModel for BinaryDocument {
    type Change = std::convert::Infallible;
    fn apply(&mut self, change: &Self::Change) -> Result<Self::Change, String> {
        match *change {}
    }
}

/// A canonical in-memory source. Clones retain identity and acknowledged storage.
pub struct MemorySource<M: DocumentModel> {
    id: SourceId,
    title: String,
    state: Arc<Mutex<MemoryState<M>>>,
}
struct MemoryState<M> {
    model: M,
    version: u64,
    acknowledged: bool,
}
impl<M: DocumentModel> Clone for MemorySource<M> {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            title: self.title.clone(),
            state: self.state.clone(),
        }
    }
}
impl<M: DocumentModel> MemorySource<M> {
    pub fn new(title: impl Into<String>, model: M) -> Self {
        Self::create(title.into(), model, true)
    }
    pub fn unsaved(title: impl Into<String>, model: M) -> Self {
        Self::create(title.into(), model, false)
    }
    fn create(title: String, model: M, acknowledged: bool) -> Self {
        Self {
            id: SourceId::new(),
            title,
            state: Arc::new(Mutex::new(MemoryState {
                model,
                version: 0,
                acknowledged,
            })),
        }
    }
}
impl<M: DocumentModel> DocumentSource<M> for MemorySource<M> {
    fn id(&self) -> SourceId {
        self.id
    }
    fn title(&self) -> String {
        self.title.clone()
    }
    fn load(&self) -> Result<SourceLoad<M>, SourceError> {
        let state = self.state.lock().unwrap();
        Ok(SourceLoad {
            model: state.model.clone(),
            version: Some(SourceVersion(state.version.to_string())),
            acknowledged: state.acknowledged,
        })
    }
    fn save(&self, snapshot: SourceSave<M>) -> Result<Option<SourceVersion>, SourceError> {
        let mut state = self.state.lock().unwrap();
        if snapshot.expected_version != Some(SourceVersion(state.version.to_string()))
            && (state.acknowledged || snapshot.expected_version.is_some())
        {
            return Err(SourceError::Changed("Memory source changed".into()));
        }
        state.model = snapshot.model;
        state.acknowledged = true;
        state.version += 1;
        Ok(Some(SourceVersion(state.version.to_string())))
    }
}

impl From<io::Error> for SourceError {
    fn from(error: io::Error) -> Self {
        if matches!(
            error.kind(),
            io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
        ) {
            Self::Missing
        } else {
            Self::Failed(error.to_string())
        }
    }
}
