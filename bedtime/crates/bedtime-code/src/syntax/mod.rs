//! Syntax: tree-sitter analysis of a document's text on a worker, giving
//! highlights, folds and an outline at a revision. `DocumentSyntax` runs it
//! for each text document an editor shows; the editor draws its results.
mod analysis;
mod folds;
mod languages;
mod outline;
mod session;
pub use languages::{LANGUAGES, document_language, language_name, register_language_formats};
pub use session::{AnalysisSession, AnalysisWorker};
use std::{ops::Range, sync::Arc};
pub use tree_sitter;
mod documents;
pub use documents::*;

/// Immutable UTF-8 source. `chunk` returns bytes starting exactly at `byte`,
/// nonempty until EOF. Cloning the host snapshot must not flatten the document.
pub trait Source: Send + Sync + 'static {
    fn len(&self) -> usize;
    fn chunk(&self, byte: usize) -> &[u8];
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
impl Source for String {
    fn len(&self) -> usize {
        String::len(self)
    }
    fn chunk(&self, byte: usize) -> &[u8] {
        &self.as_bytes()[byte..]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ObservationKey {
    pub source: u64,
    pub revision: u64,
    pub language_generation: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AnalysisStatus {
    Pending,
    Ready,
    Unsupported,
    Limited(&'static str),
    Failed(String),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Highlight {
    pub range: Range<usize>,
    pub capture: Arc<str>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoldRegion {
    /// Header line remains visible; subsequent complete lines are hidden.
    pub range: Range<usize>,
    pub start_line: usize,
    pub end_line: usize,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutlineNode {
    pub id: u64,
    pub label: String,
    pub kind: String,
    pub range: Range<usize>,
    pub name_range: Range<usize>,
    pub parent: Option<usize>,
    pub depth: usize,
}

/// Owning observation: node borrows cannot outlive `tree`. Published ranges are
/// UTF-8 validated, highlights disjoint and sorted, outline parents precede children.
pub struct Observation {
    pub key: ObservationKey,
    pub language: &'static str,
    pub source: Arc<dyn Source>,
    pub tree: tree_sitter::Tree,
    pub highlights: Vec<Highlight>,
    pub folds: Vec<FoldRegion>,
    pub outline: Vec<OutlineNode>,
    pub incremental: bool,
    pub elapsed: std::time::Duration,
}
impl Observation {
    /// O(log n + visible captures), without scanning the document's span list.
    pub fn highlights_in(&self, range: Range<usize>) -> &[Highlight] {
        let first = self
            .highlights
            .partition_point(|s| s.range.end <= range.start);
        let last = self
            .highlights
            .partition_point(|s| s.range.start < range.end);
        &self.highlights[first.min(last)..last]
    }
}

/// Deliberate initial bounds; analysis refusal never limits canonical editing.
pub const MAX_SOURCE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_RESULTS: usize = 250_000;
pub const MAX_ANALYSIS_TIME: std::time::Duration = std::time::Duration::from_secs(5);

#[cfg(test)]
mod tests;

impl Source for bedtime_document::TextBuffer {
    fn len(&self) -> usize {
        bedtime_document::TextBuffer::len(self)
    }
    fn chunk(&self, byte: usize) -> &[u8] {
        let (chunk, start) = self.chunk_at(byte);
        &chunk.as_bytes()[byte - start..]
    }
}
