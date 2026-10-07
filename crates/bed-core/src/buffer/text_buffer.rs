// Ported from nealmick/ned editor/buffer/text_buffer.{h,cpp} at
// 2d3e9b53b0ebc44c6da6901ff242a95edaa4a1ff. MIT/X Consortium; see LICENSE.
//! Copy-on-write rope of document bytes, with CR/LF/CRLF line metrics.
//! Only the document state mutates the rope; snapshots share immutable nodes.

use std::sync::Arc;

pub const MAX_LEAF_BYTES: usize = 128;

#[derive(Clone, Copy, Default, Debug)]
struct Summary {
    bytes: usize,
    breaks: u32,
    last_line_bytes: u32,
    ends_with_cr: bool,
    starts_with_lf: bool,
}

impl Summary {
    fn leaf(bytes: &[u8]) -> Self {
        let mut sum = Self {
            bytes: bytes.len(),
            ..Self::default()
        };
        let (mut i, mut line_start) = (0, 0);
        while i < bytes.len() {
            if bytes[i] == b'\r' && bytes.get(i + 1) == Some(&b'\n') {
                sum.breaks += 1;
                i += 2;
                line_start = i;
            } else if bytes[i] == b'\r' || bytes[i] == b'\n' {
                sum.breaks += 1;
                i += 1;
                line_start = i;
            } else {
                i += 1;
            }
        }
        sum.last_line_bytes = (bytes.len() - line_start) as u32;
        sum.starts_with_lf = bytes.first() == Some(&b'\n');
        sum.ends_with_cr = bytes.last() == Some(&b'\r');
        sum
    }

    fn merge(a: Self, b: Self) -> Self {
        if a.bytes == 0 {
            return b;
        }
        if b.bytes == 0 {
            return a;
        }
        let crlf_join = a.ends_with_cr && b.starts_with_lf;
        Self {
            bytes: a.bytes + b.bytes,
            breaks: a.breaks + b.breaks - u32::from(crlf_join),
            last_line_bytes: if b.breaks == 0 && !crlf_join {
                a.last_line_bytes + b.bytes as u32
            } else {
                b.last_line_bytes
            },
            starts_with_lf: a.starts_with_lf,
            ends_with_cr: b.ends_with_cr,
        }
    }

    fn lines(self) -> i32 {
        self.breaks as i32 + 1
    }
}

#[derive(Debug)]
enum Contents {
    Leaf(Vec<u8>),
    Branch { left: Arc<Node>, right: Arc<Node> },
}

#[derive(Debug)]
struct Node {
    sum: Summary,
    contents: Contents,
}

impl Node {
    fn leaf(data: Vec<u8>) -> Arc<Self> {
        Arc::new(Self {
            sum: Summary::leaf(&data),
            contents: Contents::Leaf(data),
        })
    }

    fn empty() -> Arc<Self> {
        Self::leaf(Vec::new())
    }

    fn branch(left: Arc<Self>, right: Arc<Self>) -> Arc<Self> {
        if left.sum.bytes == 0 {
            return right;
        }
        if right.sum.bytes == 0 {
            return left;
        }
        Arc::new(Self {
            sum: Summary::merge(left.sum, right.sum),
            contents: Contents::Branch { left, right },
        })
    }

    fn from_bytes(bytes: &[u8]) -> Arc<Self> {
        if bytes.len() <= MAX_LEAF_BYTES {
            return Self::leaf(bytes.to_vec());
        }
        let mut mid = bytes.len() / 2;
        if mid > 0 && mid < bytes.len() && bytes[mid - 1] == b'\r' && bytes[mid] == b'\n' {
            mid += 1;
        }
        if mid == 0 || mid >= bytes.len() {
            mid = bytes.len() / 2;
        }
        Self::branch(
            Self::from_bytes(&bytes[..mid]),
            Self::from_bytes(&bytes[mid..]),
        )
    }

    fn concat(a: Arc<Self>, b: Arc<Self>) -> Arc<Self> {
        if a.sum.bytes == 0 {
            return b;
        }
        if b.sum.bytes == 0 {
            return a;
        }
        if let (Contents::Leaf(la), Contents::Leaf(lb)) = (&a.contents, &b.contents)
            && la.len() + lb.len() <= MAX_LEAF_BYTES
        {
            let mut data = la.clone();
            data.extend_from_slice(lb);
            return Self::leaf(data);
        }
        Self::branch(a, b)
    }

    fn slice(n: &Arc<Self>, off: usize, len: usize) -> Arc<Self> {
        if len == 0 || off >= n.sum.bytes {
            return Self::empty();
        }
        let len = len.min(n.sum.bytes - off);
        match &n.contents {
            Contents::Leaf(data) => Self::from_bytes(&data[off..off + len]),
            Contents::Branch { left, right } => {
                let left_bytes = left.sum.bytes;
                if off + len <= left_bytes {
                    return Self::slice(left, off, len);
                }
                if off >= left_bytes {
                    return Self::slice(right, off - left_bytes, len);
                }
                let take = left_bytes - off;
                Self::concat(
                    Self::slice(left, off, take),
                    Self::slice(right, 0, len - take),
                )
            }
        }
    }

    fn insert(n: &Arc<Self>, off: usize, bytes: &[u8]) -> Arc<Self> {
        if bytes.is_empty() {
            return Arc::clone(n);
        }
        if n.sum.bytes == 0 {
            return Self::from_bytes(bytes);
        }
        let off = off.min(n.sum.bytes);
        match &n.contents {
            Contents::Leaf(data) => {
                let mut merged = Vec::with_capacity(data.len() + bytes.len());
                merged.extend_from_slice(&data[..off]);
                merged.extend_from_slice(bytes);
                merged.extend_from_slice(&data[off..]);
                Self::from_bytes(&merged)
            }
            Contents::Branch { left, right } => {
                if off <= left.sum.bytes {
                    Self::branch(Self::insert(left, off, bytes), Arc::clone(right))
                } else {
                    Self::branch(
                        Arc::clone(left),
                        Self::insert(right, off - left.sum.bytes, bytes),
                    )
                }
            }
        }
    }

    fn erase(n: &Arc<Self>, off: usize, len: usize) -> Arc<Self> {
        if len == 0 || off >= n.sum.bytes {
            return Arc::clone(n);
        }
        let len = len.min(n.sum.bytes - off);
        if off == 0 && len == n.sum.bytes {
            return Self::empty();
        }
        Self::concat(
            Self::slice(n, 0, off),
            Self::slice(n, off + len, n.sum.bytes - off - len),
        )
    }

    fn collect(&self, out: &mut Vec<u8>) {
        match &self.contents {
            Contents::Leaf(data) => out.extend_from_slice(data),
            Contents::Branch { left, right } => {
                left.collect(out);
                right.collect(out);
            }
        }
    }

    fn copy(&self, off: usize, len: usize, out: &mut [u8]) {
        if len == 0 || off >= self.sum.bytes {
            return;
        }
        let len = len.min(self.sum.bytes - off).min(out.len());
        match &self.contents {
            Contents::Leaf(data) => out[..len].copy_from_slice(&data[off..off + len]),
            Contents::Branch { left, right } => {
                if off + len <= left.sum.bytes {
                    left.copy(off, len, out);
                } else if off >= left.sum.bytes {
                    right.copy(off - left.sum.bytes, len, out);
                } else {
                    let take = left.sum.bytes - off;
                    let (first, rest) = out.split_at_mut(take);
                    left.copy(off, take, first);
                    right.copy(0, len - take, rest);
                }
            }
        }
    }

    fn line_start_offset(&self, row: i32) -> usize {
        if row <= 0 {
            return 0;
        }
        if row >= self.sum.lines() {
            return self.sum.bytes;
        }
        match &self.contents {
            Contents::Leaf(data) => {
                let (mut i, mut seen) = (0, 0);
                while i < data.len() {
                    if data[i] == b'\r' && data.get(i + 1) == Some(&b'\n') {
                        seen += 1;
                        i += 2;
                        if seen == row {
                            return i;
                        }
                    } else if data[i] == b'\r' || data[i] == b'\n' {
                        seen += 1;
                        i += 1;
                        if seen == row {
                            return i;
                        }
                    } else {
                        i += 1;
                    }
                }
                data.len()
            }
            Contents::Branch { left, right } => {
                let crlf_join = left.sum.ends_with_cr && right.sum.starts_with_lf;
                let complete = left.sum.breaks - u32::from(crlf_join);
                if row as u32 <= complete {
                    left.line_start_offset(row)
                } else {
                    left.sum.bytes + right.line_start_offset(row - complete as i32)
                }
            }
        }
    }

    fn collect_line_starts(&self, base: usize, out: &mut Vec<u32>, pending_after_cr: &mut usize) {
        match &self.contents {
            Contents::Leaf(data) => {
                let mut i = 0;
                while i < data.len() {
                    if *pending_after_cr != 0 {
                        let after_cr = *pending_after_cr;
                        *pending_after_cr = 0;
                        if data[i] == b'\n' {
                            i += 1;
                            out.push((base + i) as u32);
                            continue;
                        }
                        out.push(after_cr as u32);
                    }
                    if data[i] == b'\r' && data.get(i + 1) == Some(&b'\n') {
                        i += 2;
                        out.push((base + i) as u32);
                    } else if data[i] == b'\r' {
                        i += 1;
                        if i == data.len() {
                            *pending_after_cr = base + i;
                        } else {
                            out.push((base + i) as u32);
                        }
                    } else if data[i] == b'\n' {
                        i += 1;
                        out.push((base + i) as u32);
                    } else {
                        i += 1;
                    }
                }
            }
            Contents::Branch { left, right } => {
                left.collect_line_starts(base, out, pending_after_cr);
                right.collect_line_starts(base + left.sum.bytes, out, pending_after_cr);
            }
        }
    }

    fn line_starts(&self, out: &mut Vec<u32>) {
        out.clear();
        out.push(0);
        let mut pending = 0;
        self.collect_line_starts(0, out, &mut pending);
        if pending != 0 {
            out.push(pending as u32);
        }
    }

    fn contains_byte(&self, byte: u8) -> bool {
        match &self.contents {
            Contents::Leaf(data) => data.contains(&byte),
            Contents::Branch { left, right } => {
                left.contains_byte(byte) || right.contains_byte(byte)
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct TextBuffer {
    root: Arc<Node>,
}

impl Default for TextBuffer {
    fn default() -> Self {
        Self {
            root: Node::empty(),
        }
    }
}

impl TextBuffer {
    pub fn new() -> Self {
        Self::default()
    }
    pub(crate) fn assign(&mut self, bytes: &[u8]) {
        self.root = Node::from_bytes(bytes);
    }
    pub(crate) fn insert(&mut self, off: usize, bytes: &[u8]) {
        if !bytes.is_empty() {
            self.root = Node::insert(&self.root, off, bytes);
        }
    }
    pub(crate) fn erase(&mut self, off: usize, len: usize) {
        if len != 0 {
            self.root = Node::erase(&self.root, off, len);
        }
    }
    pub fn size(&self) -> usize {
        self.root.sum.bytes
    }
    pub fn line_count(&self) -> i32 {
        self.root.sum.lines()
    }

    fn line_body(&self, row: i32) -> (usize, usize) {
        if row < 0 || row >= self.line_count() {
            return (0, 0);
        }
        let start = self.root.line_start_offset(row);
        let end = if row + 1 < self.line_count() {
            self.root.line_start_offset(row + 1)
        } else {
            self.size()
        };
        if end <= start {
            return (start, 0);
        }
        let len = end - start;
        if row + 1 >= self.line_count() {
            return (start, len);
        }
        let mut tail = [0; 2];
        if len >= 2 {
            self.root.copy(end - 2, 2, &mut tail);
            if tail == *b"\r\n" {
                return (start, len - 2);
            }
            if tail[1] == b'\r' || tail[1] == b'\n' {
                return (start, len - 1);
            }
        } else {
            self.root.copy(end - 1, 1, &mut tail);
            if tail[0] == b'\r' || tail[0] == b'\n' {
                return (start, len - 1);
            }
        }
        (start, len)
    }

    pub fn line_length(&self, row: i32) -> i32 {
        self.line_body(row).1 as i32
    }
    pub fn offset_from_row_col(&self, row: i32, col: i32) -> usize {
        let row = row.clamp(0, self.line_count() - 1);
        let col = col.clamp(0, self.line_length(row));
        self.root.line_start_offset(row) + col as usize
    }

    pub fn row_col_from_offset(&self, off: usize) -> (i32, i32) {
        if self.size() == 0 {
            return (0, 0);
        }
        let off = off.min(self.size());
        let (mut lo, mut hi) = (0, self.line_count() - 1);
        while lo < hi {
            let mid = lo + (hi - lo + 1) / 2;
            if self.root.line_start_offset(mid) <= off {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        let (mut row, mut col) = (lo, (off - self.root.line_start_offset(lo)) as i32);
        let len = self.line_length(row);
        if col > len {
            if row + 1 < self.line_count() {
                row += 1;
                col = 0;
            } else {
                col = len;
            }
        }
        (row, col)
    }

    pub fn line(&self, row: i32) -> Vec<u8> {
        let mut out = Vec::new();
        self.line_into(row, &mut out, usize::MAX);
        out
    }
    pub fn line_into(&self, row: i32, out: &mut Vec<u8>, max_bytes: usize) {
        out.clear();
        if max_bytes == 0 {
            return;
        }
        let (start, len) = self.line_body(row);
        let len = len.min(max_bytes);
        out.resize(len, 0);
        self.root.copy(start, len, out);
    }
    pub fn bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.size());
        self.root.collect(&mut out);
        out
    }
    pub fn copy_bytes(&self, off: usize, len: usize, out: &mut [u8]) {
        self.root.copy(off, len, out);
    }
    pub fn line_starts(&self, out: &mut Vec<u32>) {
        self.root.line_starts(out);
    }
    pub fn contains_byte(&self, byte: u8) -> bool {
        self.root.contains_byte(byte)
    }
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            root: Some(Arc::clone(&self.root)),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    root: Option<Arc<Node>>,
}

impl Snapshot {
    pub fn size(&self) -> usize {
        self.root.as_ref().map_or(0, |root| root.sum.bytes)
    }
    pub fn line_count(&self) -> i32 {
        self.root.as_ref().map_or(1, |root| root.sum.lines())
    }
    pub fn copy_bytes(&self, off: usize, len: usize, out: &mut [u8]) {
        if let Some(root) = &self.root {
            root.copy(off, len, out);
        }
    }
    pub fn bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.size());
        if let Some(root) = &self.root {
            root.collect(&mut out);
        }
        out
    }
    pub fn line_starts(&self, out: &mut Vec<u32>) {
        if let Some(root) = &self.root {
            root.line_starts(out);
        } else {
            out.clear();
            out.push(0);
        }
    }
}
