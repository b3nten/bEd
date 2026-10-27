//! Where a document's contents live. Sources do blocking I/O; hosts call them
//! on worker threads.
use crate::DocumentModel;
use std::{
    fmt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

/// Identifies a source for the life of the process. Opening the same source
/// twice yields the same document; clones of a source share its identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SourceId(u64);

impl SourceId {
    pub fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

impl Default for SourceId {
    fn default() -> Self {
        Self::new()
    }
}

/// An opaque token for what the source currently holds (a modification time,
/// a hash, a revision number). Equal versions mean unchanged contents.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SourceVersion(pub String);

impl From<&str> for SourceVersion {
    fn from(value: &str) -> Self {
        Self(value.into())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceError {
    Failed(String),
    /// The source no longer matches the expected version, so a save was refused.
    Changed,
    /// The source no longer exists.
    Missing,
}

impl fmt::Display for SourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Failed(error) => f.write_str(error),
            Self::Changed => f.write_str("It was changed by something else"),
            Self::Missing => f.write_str("It no longer exists"),
        }
    }
}

impl std::error::Error for SourceError {}

pub struct SourceLoad<M> {
    pub model: M,
    pub version: Option<SourceVersion>,
    /// False for contents that were never saved (a new, untitled document),
    /// which therefore start out unsaved.
    pub saved: bool,
}

pub trait DocumentSource<M: DocumentModel>: Send + Sync + 'static {
    fn id(&self) -> SourceId;
    
    fn title(&self) -> String;
    
    fn load(&self) -> Result<SourceLoad<M>, SourceError>;

    /// Write `model`, refusing with `Changed` unless the source still holds
    /// `expected` (the version last loaded or saved). Returns the new version.
    fn save(
        &self,
        model: M,
        expected: Option<SourceVersion>,
    ) -> Result<Option<SourceVersion>, SourceError>;

    fn writable(&self) -> bool {
        true
    }

    /// A cheap way to notice outside changes: the current version, without
    /// loading. `None` means this source can't tell, and isn't checked.
    fn version(&self) -> Result<Option<SourceVersion>, SourceError> {
        Ok(None)
    }

    /// How often to check `version` for outside changes.
    fn check_interval(&self) -> Option<Duration> {
        None
    }
}

/// An in-memory source. Clones share contents and identity, so a test (or an
/// application) can change the contents "from outside" with `set`.
pub struct MemorySource<M: DocumentModel> {
    id: SourceId,
    title: String,
    state: Arc<Mutex<MemoryState<M>>>,
}

struct MemoryState<M> {
    model: Option<M>,
    version: u64,
    saved: bool,
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

    /// Contents that start out unsaved, like a new untitled document.
    pub fn unsaved(title: impl Into<String>, model: M) -> Self {
        Self::create(title.into(), model, false)
    }

    fn create(title: String, model: M, saved: bool) -> Self {
        Self {
            id: SourceId::new(),
            title,
            state: Arc::new(Mutex::new(MemoryState {
                model: Some(model),
                version: 0,
                saved,
            })),
        }
    }

    /// The current contents, or `None` once removed.
    pub fn get(&self) -> Option<M> {
        self.state.lock().unwrap().model.clone()
    }

    /// Replace the contents, as if something else wrote them.
    pub fn set(&self, model: M) {
        let mut state = self.state.lock().unwrap();
        state.model = Some(model);
        state.version += 1;
        state.saved = true;
    }

    /// Remove the contents, as if the source were deleted.
    pub fn remove(&self) {
        let mut state = self.state.lock().unwrap();
        state.model = None;
        state.version += 1;
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
            model: state.model.clone().ok_or(SourceError::Missing)?,
            version: Some(SourceVersion(state.version.to_string())),
            saved: state.saved,
        })
    }

    fn save(
        &self,
        model: M,
        expected: Option<SourceVersion>,
    ) -> Result<Option<SourceVersion>, SourceError> {
        let mut state = self.state.lock().unwrap();
        if state.saved && expected != Some(SourceVersion(state.version.to_string())) {
            return Err(SourceError::Changed);
        }
        state.model = Some(model);
        state.version += 1;
        state.saved = true;
        Ok(Some(SourceVersion(state.version.to_string())))
    }

    fn version(&self) -> Result<Option<SourceVersion>, SourceError> {
        Ok(Some(SourceVersion(
            self.state.lock().unwrap().version.to_string(),
        )))
    }
}
