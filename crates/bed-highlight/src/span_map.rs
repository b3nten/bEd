// Translated from nealmick/ned editor/services/highlight/span_map.{h,cpp}.
// Source revision and attribution in NOTICE; upstream MIT/X Consortium license in NOTICE (ned section).
use crate::tree_sitter::{ColorRangeMap, ColorSpan, LineColorSpans};
use bed_editing::editor_operations::{OpKind, PendingEdit};

#[derive(Default, Debug)]
pub struct SpanMap {
    pub lines: ColorRangeMap,
    pub lens: Vec<i32>,
}
impl SpanMap {
    pub fn clear(&mut self) {
        self.lines.clear();
        self.lens.clear();
    }
    pub fn assign_empty(&mut self, line_count: usize) {
        self.lines = vec![Vec::new(); line_count];
        self.lens = vec![0; line_count];
    }
    pub fn at(&self, row: i32) -> &[ColorSpan] {
        if row < 0 {
            &[]
        } else {
            self.lines.get(row as usize).map_or(&[], Vec::as_slice)
        }
    }
    pub fn insert_bytes(spans: &mut LineColorSpans, col: i32, n: i32) {
        if n <= 0 {
            return;
        }
        let mut covered = false;
        for s in spans.iter_mut() {
            if s.start >= col {
                s.start += n;
                s.end += n;
            } else if s.end >= col {
                s.end += n;
                covered = true;
            }
        }
        if covered {
            return;
        }
        for s in spans.iter_mut().rev() {
            if s.end <= col {
                s.end = col + n;
                return;
            }
        }
    }
    pub fn delete_bytes(spans: &mut LineColorSpans, col: i32, n: i32) {
        if n <= 0 {
            return;
        }
        let del_end = col + n;
        let mut out = Vec::with_capacity(spans.len());
        for s in spans.iter() {
            if s.end <= col {
                out.push(*s);
            } else if s.start >= del_end {
                out.push(ColorSpan {
                    start: s.start - n,
                    end: s.end - n,
                    slot: s.slot,
                });
            } else {
                if s.start < col {
                    out.push(ColorSpan {
                        start: s.start,
                        end: col,
                        slot: s.slot,
                    });
                }
                if s.end > del_end {
                    out.push(ColorSpan {
                        start: col,
                        end: s.end - n,
                        slot: s.slot,
                    });
                }
            }
        }
        out.retain(|s| s.start < s.end);
        *spans = out;
    }
    pub fn insert(&mut self, row: i32, col: i32, text: &[u8], eol: &[u8]) {
        if text.is_empty() || row < 0 {
            return;
        }
        let row = row as usize;
        if row >= self.lines.len() {
            self.lines.resize_with(row + 1, Vec::new);
            self.lens.resize(row + 1, 0);
        }
        let parts = split_on_separator(text, eol);
        if parts.len() == 1 {
            let n = parts[0].len() as i32;
            Self::insert_bytes(&mut self.lines[row], col, n);
            self.lens[row] += n;
            return;
        }
        let (mut head, mut tail) = (Vec::new(), Vec::new());
        for s in &self.lines[row] {
            if s.end <= col {
                head.push(*s);
            } else if s.start >= col {
                tail.push(ColorSpan {
                    start: s.start - col,
                    end: s.end - col,
                    slot: s.slot,
                });
            } else {
                head.push(ColorSpan {
                    start: s.start,
                    end: col,
                    slot: s.slot,
                });
                tail.push(ColorSpan {
                    start: 0,
                    end: s.end - col,
                    slot: s.slot,
                });
            }
        }
        let first_len = parts[0].len() as i32;
        let last_len = parts.last().unwrap().len() as i32;
        let tail_len = (self.lens[row] - col).max(0);
        Self::insert_bytes(&mut head, col, first_len);
        self.lines[row] = head;
        self.lens[row] = col + first_len;
        let mut added = vec![Vec::new(); parts.len() - 1];
        let mut added_lens: Vec<i32> = parts[1..].iter().map(|p| p.len() as i32).collect();
        *added_lens.last_mut().unwrap() = last_len + tail_len;
        for s in &mut tail {
            s.start += last_len;
            s.end += last_len;
        }
        *added.last_mut().unwrap() = tail;
        self.lines.splice(row + 1..row + 1, added);
        self.lens.splice(row + 1..row + 1, added_lens);
    }
    pub fn remove(&mut self, row: i32, col: i32, length: i32, sep_len: i32) {
        if length <= 0 || self.lines.is_empty() {
            return;
        }
        let row = row.clamp(0, self.lines.len() as i32 - 1) as usize;
        let col = col.clamp(0, self.lens[row]);
        let (mut remaining, sep_len) = (length, sep_len.max(0));
        while remaining > 0 && row < self.lines.len() {
            let avail = self.lens[row] - col;
            if remaining <= avail {
                Self::delete_bytes(&mut self.lines[row], col, remaining);
                self.lens[row] -= remaining;
                return;
            }
            Self::delete_bytes(&mut self.lines[row], col, 1 << 30);
            remaining -= avail;
            self.lens[row] = col;
            if row + 1 >= self.lines.len() {
                return;
            }
            remaining = if remaining < sep_len {
                0
            } else {
                remaining - sep_len
            };
            let mut next = self.lines.remove(row + 1);
            let next_len = self.lens.remove(row + 1);
            for s in &mut next {
                s.start += col;
                s.end += col;
            }
            self.lines[row].extend(next);
            self.lens[row] = col + next_len;
        }
    }
    pub fn apply_edits(&mut self, edits: &[PendingEdit], eol: &[u8]) -> bool {
        if edits.is_empty() || self.lens.len() != self.lines.len() || self.lines.is_empty() {
            return false;
        }
        for pe in edits {
            if pe.op.kind == OpKind::Insert {
                self.insert(pe.op.row, pe.op.column, &pe.op.text, eol);
            } else {
                self.remove(pe.op.row, pe.op.column, pe.op.length, eol.len() as i32);
            }
        }
        true
    }
}
fn split_on_separator<'a>(text: &'a [u8], sep: &[u8]) -> Vec<&'a [u8]> {
    if sep.is_empty() {
        return vec![text];
    }
    let mut parts = Vec::new();
    let mut start = 0;
    while start <= text.len() {
        let Some(pos) = text[start..].windows(sep.len()).position(|w| w == sep) else {
            parts.push(&text[start..]);
            break;
        };
        let pos = start + pos;
        parts.push(&text[start..pos]);
        start = pos + sep.len();
        if start == text.len() {
            parts.push(&text[start..]);
            break;
        }
    }
    if parts.is_empty() {
        parts.push(text);
    }
    parts
}
