//! Persistent AVL rope. Immutable snapshots share nodes; edits copy only a path
//! and boundary leaves. Line/cell metrics are maintained at the same owner.
use std::{fmt, ops::Range, sync::Arc};
use unicode_segmentation::{GraphemeCursor, GraphemeIncomplete};
use unicode_width::UnicodeWidthChar;

const LEAF_BYTES: usize = 4096;
const BASE: u64 = 0x100000001b3;

#[derive(Clone, Copy, Debug, Default)]
struct Summary {
    bytes: usize,
    leading: usize,
    trailing: usize,
    ascii: bool,
    breaks: usize,
    starts_lf: bool,
    ends_cr: bool,
    first: [usize; 4],
    last: usize,
    widest: usize,
    hash: u64,
    power: u64,
}
impl Summary {
    fn leaf(text: &str) -> Self {
        let mut summary = Self {
            bytes: text.len(),
            ascii: text.is_ascii(),
            trailing: text
                .bytes()
                .rev()
                .take_while(|b| matches!(b, b' ' | b'\t'))
                .count(),
            leading: text
                .bytes()
                .take_while(|b| matches!(b, b' ' | b'\t'))
                .count(),
            starts_lf: text.starts_with('\n'),
            ends_cr: text.ends_with('\r'),
            power: 1,
            ..Self::default()
        };
        let mut columns = [0, 1, 2, 3];
        let mut previous_cr = false;
        for ch in text.chars() {
            if ch == '\r' || ch == '\n' {
                if ch != '\n' || !previous_cr {
                    if summary.breaks == 0 {
                        summary.first = std::array::from_fn(|i| columns[i] - i);
                    }
                    summary.widest = summary.widest.max(columns[0]);
                    summary.breaks += 1;
                    columns = [0; 4];
                }
            } else {
                for column in &mut columns {
                    *column += cell_width(ch, *column);
                }
            }
            previous_cr = ch == '\r';
        }
        if summary.breaks == 0 {
            summary.first = std::array::from_fn(|i| columns[i] - i);
        }
        summary.last = columns[0];
        summary.widest = summary.widest.max(summary.last);
        for byte in text.bytes() {
            summary.hash = summary
                .hash
                .wrapping_mul(BASE)
                .wrapping_add(byte as u64 + 1);
            summary.power = summary.power.wrapping_mul(BASE);
        }
        summary
    }
    fn merge(a: Self, b: Self) -> Self {
        if a.bytes == 0 {
            return b;
        }
        if b.bytes == 0 {
            return a;
        }
        let joined_crlf = a.ends_cr && b.starts_lf;
        let breaks = a.breaks + b.breaks - usize::from(joined_crlf);
        let first = if a.breaks > 0 {
            a.first
        } else {
            std::array::from_fn(|i| a.first[i] + b.first[(i + a.first[i]) % 4])
        };
        let last = if b.breaks > 0 {
            b.last
        } else {
            a.last + b.first[a.last % 4]
        };
        let joined = a.last + b.first[a.last % 4];
        Self {
            bytes: a.bytes + b.bytes,
            leading: if a.leading == a.bytes {
                a.bytes + b.leading
            } else {
                a.leading
            },
            trailing: if b.trailing == b.bytes {
                a.trailing + b.bytes
            } else {
                b.trailing
            },
            ascii: a.ascii && b.ascii,
            breaks,
            starts_lf: a.starts_lf,
            ends_cr: b.ends_cr,
            first,
            last,
            widest: a.widest.max(b.widest).max(joined),
            hash: a.hash.wrapping_mul(b.power).wrapping_add(b.hash),
            power: a.power.wrapping_mul(b.power),
        }
    }
}

fn cell_width(ch: char, column: usize) -> usize {
    if ch == '\t' {
        4 - column % 4
    } else {
        ch.width().unwrap_or(0)
    }
}

enum Node {
    Leaf {
        text: String,
        summary: Summary,
        line_starts: Vec<u16>,
    },
    Branch {
        left: Arc<Node>,
        right: Arc<Node>,
        summary: Summary,
        height: u8,
    },
}
impl Node {
    fn summary(&self) -> Summary {
        match self {
            Self::Leaf { summary, .. } | Self::Branch { summary, .. } => *summary,
        }
    }
    fn height(&self) -> u8 {
        match self {
            Self::Leaf { .. } => 1,
            Self::Branch { height, .. } => *height,
        }
    }
    fn leaf(text: String) -> Arc<Self> {
        let mut line_starts = vec![0];
        let mut bytes = text.bytes().enumerate().peekable();
        while let Some((at, byte)) = bytes.next() {
            if byte == b'\r' || byte == b'\n' {
                let end = if byte == b'\r' && bytes.peek().is_some_and(|(_, byte)| *byte == b'\n') {
                    bytes.next().unwrap().0 + 1
                } else {
                    at + 1
                };
                line_starts.push(end as u16);
            }
        }
        Arc::new(Self::Leaf {
            summary: Summary::leaf(&text),
            text,
            line_starts,
        })
    }
    fn branch(left: Arc<Self>, right: Arc<Self>) -> Arc<Self> {
        if left.summary().bytes == 0 {
            return right;
        }
        if right.summary().bytes == 0 {
            return left;
        }
        Arc::new(Self::Branch {
            summary: Summary::merge(left.summary(), right.summary()),
            height: left.height().max(right.height()) + 1,
            left,
            right,
        })
    }
    fn join(left: Arc<Self>, right: Arc<Self>) -> Arc<Self> {
        if left.summary().bytes == 0 {
            return right;
        }
        if right.summary().bytes == 0 {
            return left;
        }
        if left.height() > right.height() + 1 {
            let Self::Branch {
                left: ll,
                right: lr,
                ..
            } = left.as_ref()
            else {
                unreachable!()
            };
            return Self::balance(ll.clone(), Self::join(lr.clone(), right));
        }
        if right.height() > left.height() + 1 {
            let Self::Branch {
                left: rl,
                right: rr,
                ..
            } = right.as_ref()
            else {
                unreachable!()
            };
            return Self::balance(Self::join(left, rl.clone()), rr.clone());
        }
        if let (Self::Leaf { text: a, .. }, Self::Leaf { text: b, .. }) =
            (left.as_ref(), right.as_ref())
        {
            if a.len() + b.len() <= LEAF_BYTES {
                let mut text = a.clone();
                text.push_str(b);
                return Self::leaf(text);
            }
        }
        Self::branch(left, right)
    }
    fn balance(left: Arc<Self>, right: Arc<Self>) -> Arc<Self> {
        if left.height() > right.height() + 1 {
            let Self::Branch {
                left: ll,
                right: lr,
                ..
            } = left.as_ref()
            else {
                unreachable!()
            };
            if ll.height() >= lr.height() {
                return Self::branch(ll.clone(), Self::branch(lr.clone(), right));
            }
            let Self::Branch {
                left: lrl,
                right: lrr,
                ..
            } = lr.as_ref()
            else {
                unreachable!()
            };
            return Self::branch(
                Self::branch(ll.clone(), lrl.clone()),
                Self::branch(lrr.clone(), right),
            );
        }
        if right.height() > left.height() + 1 {
            let Self::Branch {
                left: rl,
                right: rr,
                ..
            } = right.as_ref()
            else {
                unreachable!()
            };
            if rr.height() >= rl.height() {
                return Self::branch(Self::branch(left, rl.clone()), rr.clone());
            }
            let Self::Branch {
                left: rll,
                right: rlr,
                ..
            } = rl.as_ref()
            else {
                unreachable!()
            };
            return Self::branch(
                Self::branch(left, rll.clone()),
                Self::branch(rlr.clone(), rr.clone()),
            );
        }
        Self::branch(left, right)
    }
    fn build(text: &str) -> Arc<Self> {
        if text.len() <= LEAF_BYTES {
            return Self::leaf(text.into());
        }
        let mut mid = text.len() / 2;
        while !text.is_char_boundary(mid) {
            mid += 1;
        }
        // Keeping CRLF in a leaf makes line lookup at the boundary unambiguous.
        if text.as_bytes().get(mid - 1) == Some(&b'\r') && text.as_bytes().get(mid) == Some(&b'\n')
        {
            mid += 1;
        }
        Self::branch(Self::build(&text[..mid]), Self::build(&text[mid..]))
    }
    fn split(node: &Arc<Self>, at: usize) -> (Arc<Self>, Arc<Self>) {
        if at == 0 {
            return (Self::leaf(String::new()), node.clone());
        }
        if at == node.summary().bytes {
            return (node.clone(), Self::leaf(String::new()));
        }
        match node.as_ref() {
            Self::Leaf { text, .. } => {
                (Self::leaf(text[..at].into()), Self::leaf(text[at..].into()))
            }
            Self::Branch { left, right, .. } => {
                let size = left.summary().bytes;
                if at < size {
                    let (a, b) = Self::split(left, at);
                    (a, Self::join(b, right.clone()))
                } else {
                    let (a, b) = Self::split(right, at - size);
                    (Self::join(left.clone(), a), b)
                }
            }
        }
    }
    fn line_start(&self, row: usize) -> usize {
        if row == 0 {
            return 0;
        }
        if row > self.summary().breaks {
            return self.summary().bytes;
        }
        match self {
            Self::Leaf {
                text, line_starts, ..
            } => line_starts
                .get(row)
                .map_or(text.len(), |start| *start as usize),
            Self::Branch { left, right, .. } => {
                let complete = left.summary().breaks
                    - usize::from(left.summary().ends_cr && right.summary().starts_lf);
                if row <= complete {
                    left.line_start(row)
                } else {
                    left.summary().bytes + right.line_start(row - complete)
                }
            }
        }
    }
    fn breaks_before(&self, byte: usize) -> usize {
        match self {
            Self::Leaf {
                text, line_starts, ..
            } => {
                line_starts.partition_point(|start| *start as usize <= byte) - 1
                    + usize::from(
                        byte > 0
                            && text.as_bytes().get(byte - 1) == Some(&b'\r')
                            && text.as_bytes().get(byte) == Some(&b'\n'),
                    )
            }
            Self::Branch { left, right, .. } => {
                let size = left.summary().bytes;
                if byte <= size {
                    left.breaks_before(byte)
                } else {
                    left.summary().breaks + right.breaks_before(byte - size)
                        - usize::from(left.summary().ends_cr && right.summary().starts_lf)
                }
            }
        }
    }
    fn range_summary(&self, range: Range<usize>) -> Summary {
        if range.start == 0 && range.end == self.summary().bytes {
            return self.summary();
        }
        match self {
            Self::Leaf { text, .. } => Summary::leaf(&text[range]),
            Self::Branch { left, right, .. } => {
                let size = left.summary().bytes;
                if range.end <= size {
                    left.range_summary(range)
                } else if range.start >= size {
                    right.range_summary(range.start - size..range.end - size)
                } else {
                    Summary::merge(
                        left.range_summary(range.start..size),
                        right.range_summary(0..range.end - size),
                    )
                }
            }
        }
    }
    fn byte_at_cell(
        &self,
        range: Range<usize>,
        wanted: usize,
        column: &mut usize,
        base: usize,
    ) -> Option<usize> {
        if range.is_empty() {
            return None;
        }
        if range.start == 0 && range.end == self.summary().bytes && self.summary().breaks == 0 {
            let width = self.summary().first[*column % 4];
            if *column + width < wanted {
                *column += width;
                return None;
            }
        }
        match self {
            Self::Leaf { text, .. } => {
                for (byte, ch) in text[range.clone()].char_indices() {
                    let next = *column + cell_width(ch, *column);
                    if next >= wanted {
                        return Some(
                            base + range.start
                                + byte
                                + if wanted - *column > next - wanted {
                                    ch.len_utf8()
                                } else {
                                    0
                                },
                        );
                    }
                    *column = next;
                }
                None
            }
            Self::Branch { left, right, .. } => {
                let size = left.summary().bytes;
                if range.start < size {
                    if let Some(byte) =
                        left.byte_at_cell(range.start..range.end.min(size), wanted, column, base)
                    {
                        return Some(byte);
                    }
                }
                if range.end > size {
                    right.byte_at_cell(
                        range.start.saturating_sub(size)..range.end - size,
                        wanted,
                        column,
                        base + size,
                    )
                } else {
                    None
                }
            }
        }
    }
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
    pub(crate) fn is_ascii(&self) -> bool {
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
    pub(crate) fn leading_whitespace_end(&self, row: usize, limit: usize) -> usize {
        let start = self.line_start(row);
        let end = self.clamp_boundary(self.line_range(row).end.min(limit));
        start + self.0.range_summary(start..end).leading
    }
    pub(crate) fn trailing_whitespace_start(&self, range: Range<usize>) -> usize {
        range.end - self.0.range_summary(range).trailing
    }
    pub(crate) fn concat(&self, other: &Self) -> Self {
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
    pub(crate) fn replace(&mut self, range: Range<usize>, text: &Self) -> Result<Self, String> {
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
    pub(crate) fn ptr_eq(&self, other: &Self) -> bool {
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

/// A range replacement with cheap, exact snapshot restoration for history.
/// Cached roots are trusted only when the expected root matches by identity.
#[derive(Clone, Debug)]
pub struct BufferEdit {
    pub(crate) range: Range<usize>,
    pub(crate) replacement: TextBuffer,
    restore: Option<(TextBuffer, TextBuffer)>,
}
impl BufferEdit {
    pub fn new(range: Range<usize>, replacement: impl Into<TextBuffer>) -> Self {
        Self {
            range,
            replacement: replacement.into(),
            restore: None,
        }
    }
    pub fn range(&self) -> Range<usize> {
        self.range.clone()
    }
    pub fn replacement(&self) -> &TextBuffer {
        &self.replacement
    }
    pub fn apply(&self, text: &mut TextBuffer) -> Result<Self, String> {
        if self.range.start > self.range.end
            || !text.is_char_boundary(self.range.start)
            || !text.is_char_boundary(self.range.end)
        {
            return Err("Edit range must use valid UTF-8 byte boundaries".into());
        }
        let before = text.clone();
        let existing = text.slice(self.range.clone());
        if existing == self.replacement {
            return Ok(Self {
                range: self.range.clone(),
                replacement: existing,
                restore: Some((before.clone(), before)),
            });
        }
        let removed = if let Some((expected, previous)) = &self.restore {
            if text.ptr_eq(expected) {
                let removed = text.slice(self.range.clone());
                *text = previous.clone();
                removed
            } else {
                text.replace(self.range.clone(), &self.replacement)?
            }
        } else {
            text.replace(self.range.clone(), &self.replacement)?
        };
        Ok(Self {
            range: self.range.start..self.range.start + self.replacement.len(),
            replacement: removed,
            restore: Some((text.clone(), before)),
        })
    }
}

#[cfg(test)]
mod tests;

/// Streaming construction for worker-produced content. Never owns a flat copy
/// of the complete result. CRLF may cross leaves; Node summaries handle it.
#[derive(Default)]
pub(crate) struct BufferBuilder {
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
