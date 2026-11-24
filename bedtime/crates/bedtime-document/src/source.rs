//! Where a document's contents live. Sources do blocking I/O; hosts call them
//! on worker threads.
use crate::DocumentModel;
use std::{
    fmt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
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

/// A source's name that lasts across runs, so it can be found again: a
/// scheme, a colon, then the rest. Files are `file://` and their path
/// (`file:///Users/me/notes.txt`); new documents are `untitled:Untitled 2`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SourceName(String);

impl SourceName {
    pub fn new(scheme: &str, rest: &str) -> Self {
        Self(format!("{scheme}:{rest}"))
    }

    /// A name as written (`as_str`); `None` without a scheme.
    pub fn parse(name: impl Into<String>) -> Option<Self> {
        let name = name.into();
        name.split_once(':')
            .is_some_and(|(scheme, _)| !scheme.is_empty())
            .then_some(Self(name))
    }

    /// A file's name. `None` for a path that isn't Unicode, which can't be
    /// named (and so isn't found again across runs).
    pub fn file(path: &std::path::Path) -> Option<Self> {
        Some(Self(format!("file://{}", path.to_str()?)))
    }

    pub fn scheme(&self) -> &str {
        self.0.split_once(':').map_or("", |(scheme, _)| scheme)
    }

    /// What follows the scheme and its colon.
    pub fn rest(&self) -> &str {
        self.0.split_once(':').map_or("", |(_, rest)| rest)
    }

    /// The file it names, if it names one.
    pub fn to_file(&self) -> Option<std::path::PathBuf> {
        self.0.strip_prefix("file://").map(Into::into)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SourceName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Told that a watched source may have changed. Call it from any thread;
/// a call that turns out to be for nothing costs a version check.
#[derive(Clone)]
pub struct SourceChanged(Arc<dyn Fn() + Send + Sync>);

impl SourceChanged {
    pub fn new(changed: impl Fn() + Send + Sync + 'static) -> Self {
        Self(Arc::new(changed))
    }

    pub fn notify(&self) {
        (self.0)()
    }
}

impl fmt::Debug for SourceChanged {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SourceChanged")
    }
}

/// A source being watched: watching goes on while this is kept.
pub struct SourceWatch(#[allow(dead_code)] Box<dyn Send + Sync>);

impl SourceWatch {
    /// Watching ends when `guard` is dropped.
    pub fn new(guard: impl Send + Sync + 'static) -> Self {
        Self(Box::new(guard))
    }
}

pub struct SourceLoad<M> {
    pub model: M,
    pub version: Option<SourceVersion>,
    /// False for contents that were never saved (a new, untitled document).
    /// They count as unsaved once they differ from `model`.
    pub saved: bool,
}

pub trait DocumentSource<M: DocumentModel>: Send + Sync + 'static {
    fn id(&self) -> SourceId;

    fn title(&self) -> String;

    /// Its name across runs, for finding it again (reopening it in a later
    /// session, its unsaved changes kept meanwhile). `None`: it can't be.
    fn name(&self) -> Option<SourceName> {
        None
    }

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

    /// Each change is saved as soon as it's made (an app's own store, a
    /// database), rather than when someone saves: the document is never
    /// unsaved, so closing and quitting never ask about it, and hot exit
    /// has nothing to keep.
    fn saves_every_change(&self) -> bool {
        false
    }

    /// The current version, without loading: how a document tells whether
    /// the source changed before reading it again. `None` means this source
    /// can't tell, so it's read again only when asked (`refresh_document`).
    fn version(&self) -> Result<Option<SourceVersion>, SourceError> {
        Ok(None)
    }

    /// Notice outside changes: call `changed` whenever what the source holds
    /// may have changed, until the returned watch is dropped. Calls for
    /// nothing are fine; the document checks `version` first. `None` means
    /// this source can't watch.
    fn watch(&self, changed: SourceChanged) -> Option<SourceWatch> {
        let _ = changed;
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
    /// Told when `set` or `remove` changes the contents, by watch id.
    watchers: Vec<(u64, SourceChanged)>,
    next_watch: u64,
}

impl<M> MemoryState<M> {
    fn changed(&self) {
        for (_, changed) in &self.watchers {
            changed.notify();
        }
    }
}

/// Watching a `MemorySource`: stops when dropped.
struct MemoryWatch<M> {
    id: u64,
    state: std::sync::Weak<Mutex<MemoryState<M>>>,
}

impl<M> Drop for MemoryWatch<M> {
    fn drop(&mut self) {
        if let Some(state) = self.state.upgrade() {
            state
                .lock()
                .unwrap()
                .watchers
                .retain(|(id, _)| *id != self.id);
        }
    }
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

    /// Contents that were never saved, like a new untitled document: unsaved
    /// from the first change.
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
                watchers: Vec::new(),
                next_watch: 0,
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
        state.changed();
    }

    /// Remove the contents, as if the source were deleted.
    pub fn remove(&self) {
        let mut state = self.state.lock().unwrap();
        state.model = None;
        state.version += 1;
        state.changed();
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
        // Nothing there (removed): expected to be nothing.
        let current = state
            .model
            .as_ref()
            .map(|_| SourceVersion(state.version.to_string()));
        if state.saved && expected != current {
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

    /// Told of `set` and `remove`, not of saves.
    fn watch(&self, changed: SourceChanged) -> Option<SourceWatch> {
        let mut state = self.state.lock().unwrap();
        state.next_watch += 1;
        let id = state.next_watch;
        state.watchers.push((id, changed));
        Some(SourceWatch::new(MemoryWatch {
            id,
            state: Arc::downgrade(&self.state),
        }))
    }
}
