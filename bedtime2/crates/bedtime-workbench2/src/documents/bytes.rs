//! Captured byte projections. No read performs I/O in the main World.
use super::*;
use bedtime_document2::{ByteSnapshot, SourceError, SourceId};
use std::{
    io::{self, Read, Seek, SeekFrom},
    ops::Range,
};

#[derive(Debug)]
pub enum DocumentByteError {
    Changed,
    Io(io::Error),
}
impl std::fmt::Display for DocumentByteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Changed => f.write_str("Document source changed; refresh required"),
            Self::Io(e) => e.fmt(f),
        }
    }
}
impl std::error::Error for DocumentByteError {}
impl From<io::Error> for DocumentByteError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}
fn byte_error(e: SourceError) -> DocumentByteError {
    match e {
        SourceError::Changed(_) => DocumentByteError::Changed,
        e => DocumentByteError::Io(io::Error::other(e)),
    }
}
#[derive(Clone, Debug)]
pub struct DocumentByteMetadata {
    pub len: u64,
    pub version: SourceVersion,
}
#[derive(Clone)]
pub struct DocumentByteSource {
    id: SourceId,
    snapshot: Arc<dyn ByteSnapshot>,
}
impl fmt::Debug for DocumentByteSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DocumentByteSource")
            .field("id", &self.id)
            .field("version", &self.snapshot.version())
            .finish()
    }
}
impl PartialEq for DocumentByteSource {
    fn eq(&self, o: &Self) -> bool {
        self.id == o.id && self.snapshot.version() == o.snapshot.version()
    }
}
impl Eq for DocumentByteSource {}
impl PartialOrd for DocumentByteSource {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for DocumentByteSource {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        (self.id, &self.snapshot.version().0).cmp(&(o.id, &o.snapshot.version().0))
    }
}
impl DocumentByteSource {
    pub fn new(id: SourceId, snapshot: Arc<dyn ByteSnapshot>) -> Self {
        Self { id, snapshot }
    }
    pub fn metadata(&self) -> Result<DocumentByteMetadata, DocumentByteError> {
        Ok(DocumentByteMetadata {
            len: self.snapshot.len(),
            version: self.snapshot.version(),
        })
    }
    pub fn read_range(
        &self,
        version: &SourceVersion,
        range: Range<u64>,
    ) -> Result<Vec<u8>, DocumentByteError> {
        if *version != self.snapshot.version() {
            return Err(DocumentByteError::Changed);
        }
        if range.end.saturating_sub(range.start) > 16 * 1024 * 1024 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Document range reads are limited to 16 MiB; use a bounded stream",
            )
            .into());
        }
        self.snapshot.read_range(range).map_err(byte_error)
    }
    /// Worker-only whole-input convenience with an explicit consumer budget.
    pub fn read_bounded(&self, limit: u64) -> Result<Vec<u8>, DocumentByteError> {
        if self.snapshot.len() > limit {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Document exceeds consumer read limit",
            )
            .into());
        }
        let mut output = Vec::new();
        let mut reader = self.open()?;
        reader
            .by_ref()
            .take(limit.saturating_add(1))
            .read_to_end(&mut output)?;
        reader.verify()?;
        if output.len() as u64 > limit {
            return Err(DocumentByteError::Changed);
        }
        Ok(output)
    }
    pub fn open(&self) -> Result<DocumentByteReader, DocumentByteError> {
        Ok(DocumentByteReader {
            source: self.clone(),
            metadata: self.metadata()?,
            at: 0,
        })
    }
}
pub struct DocumentByteReader {
    source: DocumentByteSource,
    metadata: DocumentByteMetadata,
    at: u64,
}
impl DocumentByteReader {
    pub fn metadata(&self) -> &DocumentByteMetadata {
        &self.metadata
    }
    pub fn verify(&self) -> Result<(), DocumentByteError> {
        self.source
            .read_range(&self.metadata.version, 0..0)
            .map(|_| ())
    }
}
impl Read for DocumentByteReader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if self.at >= self.metadata.len {
            return Ok(0);
        }
        let end = self
            .at
            .saturating_add(output.len().min(64 * 1024) as u64)
            .min(self.metadata.len);
        let bytes = self
            .source
            .read_range(&self.metadata.version, self.at..end)
            .map_err(io::Error::other)?;
        output[..bytes.len()].copy_from_slice(&bytes);
        self.at = end;
        Ok(bytes.len())
    }
}
impl Seek for DocumentByteReader {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let at = match from {
            SeekFrom::Start(at) => at as i128,
            SeekFrom::Current(n) => self.at as i128 + n as i128,
            SeekFrom::End(n) => self.metadata.len as i128 + n as i128,
        };
        if !(0..=u64::MAX as i128).contains(&at) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid byte offset",
            ));
        }
        self.at = at as u64;
        Ok(self.at)
    }
}
struct TextBytes {
    text: TextDocument,
    revision: u64,
}
impl ByteSnapshot for TextBytes {
    fn len(&self) -> u64 {
        self.text.len() as u64
    }
    fn version(&self) -> SourceVersion {
        SourceVersion(format!("text:{}", self.revision))
    }
    fn read_range(&self, range: Range<u64>) -> Result<Vec<u8>, SourceError> {
        if range.start > range.end || range.end > self.len() {
            return Err(SourceError::Failed("Invalid document byte range".into()));
        }
        let mut result = Vec::with_capacity((range.end - range.start) as usize);
        let mut at = range.start as usize;
        while at < range.end as usize {
            let (chunk, base) = self.text.chunk_at(at);
            let end = (base + chunk.len()).min(range.end as usize);
            result.extend_from_slice(&chunk.as_bytes()[at - base..end - base]);
            at = end;
        }
        Ok(result)
    }
}
/// Captures current text, including unsaved edits, or a version-bound binary source.
pub fn document_bytes(world: &World, id: DocumentId) -> Result<DocumentByteSource, DocumentError> {
    let source = document_source_id(world, id).ok_or(DocumentError::NoSaveBinding)?;
    if let Ok(handle) = cast_document::<TextDocument>(world, id) {
        return Ok(DocumentByteSource::new(
            source,
            Arc::new(TextBytes {
                text: read_document(world, handle)?.clone(),
                revision: inspect_document(world, id)?.revision,
            }),
        ));
    }
    let handle = cast_document::<BinaryDocument>(world, id)?;
    Ok(DocumentByteSource::new(
        source,
        read_document(world, handle)?.snapshot.clone(),
    ))
}

use std::collections::{BTreeMap, BTreeSet};
pub const DOCUMENT_PAGE_BYTES: u64 = 64 * 1024;
const DOCUMENT_CACHE_PAGES: usize = 32;
struct DocumentPageJob {
    page: u64,
    task: crate::ManagedTask<Result<Vec<u8>, DocumentByteError>>,
}
pub struct DocumentBytePageCache {
    pub stamp: DocumentByteMetadata,
    pub pages: BTreeMap<u64, Arc<[u8]>>,
    pub error: Option<String>,
    job: Option<DocumentPageJob>,
}
impl DocumentBytePageCache {
    pub fn byte(&self, offset: u64) -> Option<u8> {
        self.pages
            .get(&(offset / DOCUMENT_PAGE_BYTES))?
            .get((offset % DOCUMENT_PAGE_BYTES) as usize)
            .copied()
    }
}
impl Drop for DocumentBytePageCache {
    fn drop(&mut self) {
        if let Some(job) = &self.job {
            job.task.cancel();
        }
    }
}
#[derive(bevy::prelude::Resource, Default)]
struct DocumentByteCaches(BTreeMap<DocumentByteSource, DocumentBytePageCache>);
pub fn document_byte_cache<'a>(
    world: &'a World,
    source: &DocumentByteSource,
) -> Option<&'a DocumentBytePageCache> {
    world.get_resource::<DocumentByteCaches>()?.0.get(source)
}
pub fn accept_document_byte_cache(
    world: &mut World,
    source: DocumentByteSource,
    stamp: DocumentByteMetadata,
    bytes: Arc<[u8]>,
) {
    world.init_resource::<DocumentByteCaches>();
    let mut caches = world.resource_mut::<DocumentByteCaches>();
    if caches
        .0
        .get(&source)
        .is_none_or(|s| s.stamp.version != stamp.version || s.error.is_some())
    {
        caches.0.insert(
            source,
            DocumentBytePageCache {
                stamp,
                pages: BTreeMap::from([(0, bytes)]),
                error: None,
                job: None,
            },
        );
    }
}
fn poll<T: Send + 'static>(
    task: &crate::ManagedTask<Result<T, DocumentByteError>>,
) -> Option<Result<T, DocumentByteError>> {
    match task.try_recv() {
        Ok(result) => Some(result),
        Err(std::sync::mpsc::TryRecvError::Empty) => None,
        Err(_) => Some(Err(DocumentByteError::Io(io::Error::other(
            "Document page worker stopped",
        )))),
    }
}
pub fn update_document_byte_cache(
    world: &mut World,
    demand: BTreeMap<DocumentByteSource, BTreeSet<u64>>,
) {
    if let Some(mut sources) = world.remove_resource::<DocumentByteCaches>() {
        let wanted = demand;
        sources.0.retain(|path, _| wanted.contains_key(path));
        for (path, source) in &mut sources.0 {
            let pages = &wanted[path];
            if let Some(result) = source.job.as_ref().and_then(|j| poll(&j.task)) {
                let page = source.job.take().unwrap().page;
                match result {
                    Ok(bytes) => {
                        source.pages.insert(page, bytes.into());
                    }
                    Err(error) => {
                        source.pages.clear();
                        source.error = Some(error.to_string());
                    }
                }
            }
            while source.pages.len() > DOCUMENT_CACHE_PAGES {
                let remove = source.pages.keys().copied().find(|p| !pages.contains(p));
                if let Some(page) = remove.or_else(|| source.pages.keys().next().copied()) {
                    source.pages.remove(&page);
                } else {
                    break;
                }
            }
            if source
                .job
                .as_ref()
                .is_some_and(|job| !pages.contains(&job.page))
            {
                source.job.take().unwrap().task.cancel();
            }
            if source.job.is_none()
                && source.error.is_none()
                && let Some(page) = pages
                    .iter()
                    .find(|p| !source.pages.contains_key(p))
                    .copied()
            {
                let capture = source.stamp.clone();
                let path = path.clone();
                if let Some(mut work) = world.get_resource_mut::<ManagedWork>()
                    && !work.is_closing()
                {
                    match work.spawn_task(None, "Read document page", move |_| {
                        let start = page * DOCUMENT_PAGE_BYTES;
                        path.read_range(
                            &capture.version,
                            start..start.saturating_add(DOCUMENT_PAGE_BYTES).min(capture.len),
                        )
                    }) {
                        Ok(task) => source.job = Some(DocumentPageJob { page, task }),
                        Err(error) => source.error = Some(error),
                    }
                }
            }
        }
        world.insert_resource(sources);
    }
}
