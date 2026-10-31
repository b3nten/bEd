//! Text documents: `TextBuffer`, a persistent rope with line and display-width
//! metrics and grapheme-aware movement, edited with `BufferEdit`.
mod edit;
mod rope;
#[cfg(test)]
mod tests;

use crate::DocumentModel;
pub use edit::BufferEdit;
use rope::*;
use std::{fmt, ops::Range, sync::Arc};
use unicode_segmentation::{GraphemeCursor, GraphemeIncomplete};

impl DocumentModel for TextBuffer {
    type Change = BufferEdit;
    type OpenState = TextOpenState;
    fn apply(&mut self, change: &BufferEdit) -> Result<BufferEdit, String> {
        change.apply(self)
    }
    fn state_at_line(line: u32, column: u32) -> TextOpenState {
        TextOpenState {
            cursor: Some(TextPosition::LineColumn { line, column }),
            selection: None,
        }
    }
}

/// Where to open a text document: the cursor (revealed), and a selection.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TextOpenState {
    pub cursor: Option<TextPosition>,
    /// Bytes to select.
    pub selection: Option<std::ops::Range<usize>>,
}

/// A place in a text document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextPosition {
    /// From 0; the column counts characters.
    LineColumn {
        line: u32,
        column: u32,
    },
    Byte(usize),
}

#[derive(Clone)]
pub struct TextBuffer(Arc<Node>);
impl Default for TextBuffer {
    fn default() -> Self {
        Self(Node::leaf(String::new()))
    }
}

impl From<&str> for TextBuffer {
    fn from(text: &str) -> Self {
        Self(Node::build(text))
    }
}

impl From<String> for TextBuffer {
    fn from(text: String) -> Self {
        Self::from(text.as_str())
    }
}

impl fmt::Debug for TextBuffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TextBuffer")
            .field("bytes", &self.len())
            .field("lines", &self.line_count())
            .finish()
    }
}

impl fmt::Display for TextBuffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for chunk in self.chunks() {
            f.write_str(chunk)?;
        }
        Ok(())
    }
}

impl PartialEq for TextBuffer {
    fn eq(&self, other: &Self) -> bool {
        self.ptr_eq(other)
            || (self.len() == other.len()
                && self.0.summary().hash == other.0.summary().hash
                && self
                    .chunks()
                    .flat_map(str::bytes)
                    .eq(other.chunks().flat_map(str::bytes)))
    }
}

impl Eq for TextBuffer {}

impl PartialEq<str> for TextBuffer {
    fn eq(&self, other: &str) -> bool {
        self.len() == other.len() && self.chunks().flat_map(str::bytes).eq(other.bytes())
    }
}

impl PartialEq<&str> for TextBuffer {
    fn eq(&self, other: &&str) -> bool {
        self == *other
    }
}

impl TextBuffer {
    pub fn len(&self) -> usize {
        self.0.summary().bytes
    }

    pub fn is_ascii(&self) -> bool {
        self.0.summary().ascii
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn line_count(&self) -> usize {
        self.0.summary().breaks + 1
    }

    pub fn widest_line_cells(&self) -> usize {
        self.0.summary().widest
    }

    pub fn line_start(&self, row: usize) -> usize {
        self.0.line_start(row)
    }

    pub fn line_range(&self, row: usize) -> Range<usize> {
        let start = self.line_start(row.min(self.line_count() - 1));
        let mut end = self.line_start(row + 1);
        if end > start && self.byte(end - 1).is_some_and(|b| b == b'\n' || b == b'\r') {
            end -= 1;
        }
        if end > start && self.byte(end) == Some(b'\n') && self.byte(end - 1) == Some(b'\r') {
            end -= 1;
        }
        start..end
    }

    pub fn line_of(&self, byte: usize) -> usize {
        let byte = byte.min(self.len());
        let mut boundary = byte;
        while !self.is_char_boundary(boundary) {
            boundary -= 1;
        }
        let mut row = self.0.breaks_before(boundary);
        if boundary > 0
            && self.byte(boundary - 1) == Some(b'\r')
            && self.byte(boundary) == Some(b'\n')
        {
            row = row.saturating_sub(1);
        }
        row
    }

    pub fn leading_whitespace_end(&self, row: usize, limit: usize) -> usize {
        let start = self.line_start(row);
        let end = self.clamp_boundary(self.line_range(row).end.min(limit));
        start + self.0.range_summary(start..end).leading
    }

    pub fn trailing_whitespace_start(&self, range: Range<usize>) -> usize {
        range.end - self.0.range_summary(range).trailing
    }

    pub fn concat(&self, other: &Self) -> Self {
        Self(Node::join(self.0.clone(), other.0.clone()))
    }

    pub fn column(&self, byte: usize) -> usize {
        let row = self.line_of(byte);
        let range = self.line_range(row);
        self.0.range_summary(range.start..byte.min(range.end)).last
    }

    pub fn byte_at_column(&self, row: usize, column: usize) -> usize {
        let range = self.line_range(row);
        if column == 0 {
            return range.start;
        }
        self.0
            .byte_at_cell(range.clone(), column, &mut 0, 0)
            .unwrap_or(range.end)
    }

    pub fn is_char_boundary(&self, byte: usize) -> bool {
        if byte == self.len() {
            return true;
        }
        if byte > self.len() {
            return false;
        }
        let (chunk, start) = self.chunk_at(byte);
        chunk.is_char_boundary(byte - start)
    }

    pub fn clamp_boundary(&self, byte: usize) -> usize {
        let mut byte = byte.min(self.len());
        while !self.is_char_boundary(byte) {
            byte -= 1;
        }
        byte
    }

    pub fn byte(&self, byte: usize) -> Option<u8> {
        if byte >= self.len() {
            return None;
        }
        let (chunk, start) = self.chunk_at(byte);
        Some(chunk.as_bytes()[byte - start])
    }

    pub fn chunk_at(&self, byte: usize) -> (&str, usize) {
        let mut node = self.0.as_ref();
        let mut base = 0;
        loop {
            match node {
                Node::Leaf { text, .. } => return (text, base),
                Node::Branch { left, right, .. } => {
                    if byte < base + left.summary().bytes {
                        node = left;
                    } else {
                        base += left.summary().bytes;
                        node = right;
                    }
                }
            }
        }
    }

    pub fn slice(&self, range: Range<usize>) -> Self {
        assert!(
            range.start <= range.end
                && self.is_char_boundary(range.start)
                && self.is_char_boundary(range.end)
        );
        let (_, tail) = Node::split(&self.0, range.start);
        let (body, _) = Node::split(&tail, range.len());
        Self(body)
    }

    /// Copy only a requested range, without constructing intermediate rope
    /// nodes. Rendering uses this for bounded visible line fragments.
    pub fn range_text(&self, range: Range<usize>) -> String {
        assert!(
            range.start <= range.end
                && self.is_char_boundary(range.start)
                && self.is_char_boundary(range.end)
        );
        fn copy(node: &Node, range: Range<usize>, out: &mut String) {
            match node {
                Node::Leaf { text, .. } => out.push_str(&text[range]),
                Node::Branch { left, right, .. } => {
                    let size = left.summary().bytes;
                    if range.start < size {
                        copy(left, range.start..range.end.min(size), out);
                    }
                    if range.end > size {
                        copy(
                            right,
                            range.start.saturating_sub(size)..range.end - size,
                            out,
                        );
                    }
                }
            }
        }
        let mut out = String::with_capacity(range.len());
        copy(&self.0, range, &mut out);
        out
    }

    pub fn replace(&mut self, range: Range<usize>, text: &Self) -> Result<Self, String> {
        if range.start > range.end
            || !self.is_char_boundary(range.start)
            || !self.is_char_boundary(range.end)
        {
            return Err("Edit range must use valid UTF-8 byte boundaries".into());
        }
        let (before, rest) = Node::split(&self.0, range.start);
        let (removed, after) = Node::split(&rest, range.len());
        self.0 = Node::join(Node::join(before, text.0.clone()), after);
        Ok(Self(removed))
    }

    pub fn chunks(&self) -> Chunks<'_> {
        Chunks {
            stack: vec![self.0.as_ref()],
        }
    }

    pub fn fingerprint(&self) -> String {
        format!("{}:{:016x}", self.len(), self.0.summary().hash)
    }

    pub fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    pub fn next_grapheme(&self, byte: usize) -> usize {
        self.grapheme_boundary(byte, true)
    }

    pub fn previous_grapheme(&self, byte: usize) -> usize {
        self.grapheme_boundary(byte, false)
    }

    fn grapheme_boundary(&self, byte: usize, forward: bool) -> usize {
        if (forward && byte == self.len()) || (!forward && byte == 0) {
            return byte;
        }
        let mut cursor = GraphemeCursor::new(byte, self.len(), true);
        let (mut chunk, mut start) = self.chunk_at(if forward { byte } else { byte - 1 });
        loop {
            let result = if forward {
                cursor.next_boundary(chunk, start)
            } else {
                cursor.prev_boundary(chunk, start)
            };
            match result {
                Ok(Some(boundary)) => return boundary,
                Ok(None) => return if forward { self.len() } else { 0 },
                Err(GraphemeIncomplete::NextChunk) => {
                    (chunk, start) = self.chunk_at(start + chunk.len());
                }
                Err(GraphemeIncomplete::PrevChunk) => {
                    (chunk, start) = self.chunk_at(start - 1);
                }
                Err(GraphemeIncomplete::PreContext(end)) => {
                    let (context, base) = self.chunk_at(end - 1);
                    cursor.provide_context(&context[..end - base], base);
                }
                Err(GraphemeIncomplete::InvalidOffset) => unreachable!("Validated UTF-8 position"),
            }
        }
    }
}

pub struct Chunks<'a> {
    stack: Vec<&'a Node>,
}

impl<'a> Iterator for Chunks<'a> {
    type Item = &'a str;
    fn next(&mut self) -> Option<Self::Item> {
        while let Some(node) = self.stack.pop() {
            match node {
                Node::Leaf { text, .. } => return Some(text),
                Node::Branch { left, right, .. } => {
                    self.stack.push(right);
                    self.stack.push(left);
                }
            }
        }
        None
    }
}

/// Streaming construction for worker-produced content. Never owns a flat copy
/// of the complete result. CRLF may cross leaves; Node summaries handle it.
#[derive(Default)]
pub struct BufferBuilder {
    leaves: Vec<Arc<Node>>,
    pending: String,
}

impl BufferBuilder {
    pub fn push(&mut self, mut text: &str) {
        while !text.is_empty() {
            let mut take = (LEAF_BYTES - self.pending.len()).min(text.len());
            while !text.is_char_boundary(take) {
                take -= 1;
            }
            if take == 0 {
                self.leaves
                    .push(Node::leaf(std::mem::take(&mut self.pending)));
                continue;
            }
            self.pending.push_str(&text[..take]);
            text = &text[take..];
            if self.pending.len() == LEAF_BYTES {
                self.leaves
                    .push(Node::leaf(std::mem::take(&mut self.pending)));
            }
        }
    }

    pub fn push_range(
        &mut self,
        text: &TextBuffer,
        range: Range<usize>,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> bool {
        let mut at = range.start;
        while at < range.end {
            if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                return false;
            }
            let (chunk, base) = text.chunk_at(at);
            let end = (base + chunk.len()).min(range.end);
            self.push(&chunk[at - base..end - base]);
            at = end;
        }
        true
    }

    pub fn finish(mut self) -> TextBuffer {
        if !self.pending.is_empty() {
            self.leaves.push(Node::leaf(self.pending));
        }
        fn build(leaves: &[Arc<Node>]) -> Arc<Node> {
            match leaves.len() {
                0 => Node::leaf(String::new()),
                1 => leaves[0].clone(),
                n => Node::branch(build(&leaves[..n / 2]), build(&leaves[n / 2..])),
            }
        }
        TextBuffer(build(&self.leaves))
    }
}
