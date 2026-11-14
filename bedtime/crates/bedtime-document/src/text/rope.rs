//! The persistent AVL rope behind `TextBuffer`. Immutable snapshots share
//! nodes; edits copy only a path and boundary leaves. Line and display-cell
//! metrics are kept in the same nodes.
use std::{ops::Range, sync::Arc};
use unicode_width::UnicodeWidthChar;

pub(super) const LEAF_BYTES: usize = 4096;
pub(super) const BASE: u64 = 0x100000001b3;

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Summary {
    pub(super) bytes: usize,
    pub(super) leading: usize,
    pub(super) trailing: usize,
    pub(super) ascii: bool,
    pub(super) breaks: usize,
    pub(super) starts_lf: bool,
    pub(super) ends_cr: bool,
    pub(super) first: [usize; 4],
    pub(super) last: usize,
    pub(super) widest: usize,
    pub(super) hash: u64,
    pub(super) power: u64,
}

impl Summary {
    pub(super) fn leaf(text: &str) -> Self {
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

    pub(super) fn merge(a: Self, b: Self) -> Self {
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

pub(super) fn cell_width(ch: char, column: usize) -> usize {
    if ch == '\t' {
        4 - column % 4
    } else {
        ch.width().unwrap_or(0)
    }
}

pub(super) enum Node {
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
    pub(super) fn summary(&self) -> Summary {
        match self {
            Self::Leaf { summary, .. } | Self::Branch { summary, .. } => *summary,
        }
    }

    pub(super) fn height(&self) -> u8 {
        match self {
            Self::Leaf { .. } => 1,
            Self::Branch { height, .. } => *height,
        }
    }

    pub(super) fn leaf(text: String) -> Arc<Self> {
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

    pub(super) fn branch(left: Arc<Self>, right: Arc<Self>) -> Arc<Self> {
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

    pub(super) fn join(left: Arc<Self>, right: Arc<Self>) -> Arc<Self> {
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

    pub(super) fn balance(left: Arc<Self>, right: Arc<Self>) -> Arc<Self> {
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

    pub(super) fn build(text: &str) -> Arc<Self> {
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

    pub(super) fn split(node: &Arc<Self>, at: usize) -> (Arc<Self>, Arc<Self>) {
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

    pub(super) fn line_start(&self, row: usize) -> usize {
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

    pub(super) fn breaks_before(&self, byte: usize) -> usize {
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

    pub(super) fn range_summary(&self, range: Range<usize>) -> Summary {
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

    pub(super) fn byte_at_cell(
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
