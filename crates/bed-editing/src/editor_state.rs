// Ported from nealmick/ned editor/editor_state.{h,cpp} at
// 2d3e9b53b0ebc44c6da6901ff242a95edaa4a1ff. MIT/X Consortium; see LICENSE.
//! Document bytes and metadata. Cursor, view, and GUI state live elsewhere.

use crate::buffer::text_buffer::{Snapshot, TextBuffer};
use std::path::Path;

/// Text documents normalize line endings; byte documents preserve their exact representation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DocumentKind {
    #[default]
    Text,
    Bytes,
}

#[derive(Clone, Debug)]
pub struct EditorState {
    pub kind: DocumentKind,
    pub path: String,
    pub line_ending: Vec<u8>,
    pub language_id: String,
    pub version: i32,
    pub dirty: bool,
    pub utf8_bom: bool,
    text: TextBuffer,
}

impl Default for EditorState {
    fn default() -> Self {
        Self {
            kind: DocumentKind::Text,
            path: String::new(),
            line_ending: Self::platform_line_ending(),
            language_id: String::new(),
            version: 0,
            dirty: false,
            utf8_bom: false,
            text: TextBuffer::new(),
        }
    }
}

impl EditorState {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn platform_line_ending() -> Vec<u8> {
        b"\n".to_vec()
    }

    pub fn split_lines(raw: &[u8]) -> (Vec<Vec<u8>>, Vec<u8>) {
        let mut lines = Vec::new();
        let (mut saw_crlf, mut saw_lf, mut saw_cr) = (false, false, false);
        let (mut start, mut i) = (0, 0);
        while i < raw.len() {
            if raw[i] == b'\r' && raw.get(i + 1) == Some(&b'\n') {
                lines.push(raw[start..i].to_vec());
                saw_crlf = true;
                i += 2;
                start = i;
            } else if raw[i] == b'\n' || raw[i] == b'\r' {
                lines.push(raw[start..i].to_vec());
                if raw[i] == b'\n' {
                    saw_lf = true;
                } else {
                    saw_cr = true;
                }
                i += 1;
                start = i;
            } else {
                i += 1;
            }
        }
        lines.push(raw[start..].to_vec());
        let ending = if saw_crlf {
            b"\r\n".to_vec()
        } else if saw_lf {
            b"\n".to_vec()
        } else if saw_cr {
            b"\r".to_vec()
        } else {
            Self::platform_line_ending()
        };
        (lines, ending)
    }

    pub fn language_id_from_path(file_path: &str) -> String {
        Path::new(file_path)
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("")
            .to_owned()
    }

    pub fn set_from_bytes(&mut self, raw: &[u8]) {
        self.set_from_bytes_with_kind(raw, DocumentKind::Text);
    }

    pub fn set_from_bytes_with_kind(&mut self, raw: &[u8], kind: DocumentKind) {
        self.kind = kind;
        if kind == DocumentKind::Bytes {
            self.utf8_bom = false;
            self.line_ending = Self::platform_line_ending();
            self.text.assign(raw);
            self.version = 0;
            self.dirty = false;
            return;
        }
        self.utf8_bom = raw.starts_with(&[0xef, 0xbb, 0xbf]);
        let content = if self.utf8_bom { &raw[3..] } else { raw };
        let (lines, ending) = Self::split_lines(content);
        self.line_ending = ending;
        self.text.assign(&join_lines(&lines, &self.line_ending));
        self.version = 0;
        self.dirty = false;
    }

    pub fn set_line_ending(&mut self, eol: &[u8]) {
        if self.kind == DocumentKind::Bytes || eol.is_empty() || eol == self.line_ending {
            return;
        }
        let lines = self.lines();
        self.line_ending = eol.to_vec();
        self.text.assign(&join_lines(&lines, &self.line_ending));
    }

    pub fn join(&self) -> Vec<u8> {
        self.text.bytes()
    }
    pub fn byte_size(&self) -> usize {
        self.text.size()
    }
    pub fn line_count(&self) -> i32 {
        self.text.line_count()
    }
    pub fn line_length(&self, row: i32) -> i32 {
        self.text.line_length(row)
    }
    pub fn line(&self, row: i32) -> Vec<u8> {
        self.text.line(row)
    }
    pub fn line_into(&self, row: i32, out: &mut Vec<u8>, max_bytes: usize) {
        self.text.line_into(row, out, max_bytes);
    }
    pub fn lines_into(&self, out: &mut Vec<Vec<u8>>) {
        out.resize_with(self.line_count() as usize, Vec::new);
        for (row, line) in out.iter_mut().enumerate() {
            self.line_into(row as i32, line, usize::MAX);
        }
    }
    pub fn lines(&self) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        self.lines_into(&mut out);
        out
    }
    pub fn offset_from_row_col(&self, row: i32, column: i32) -> usize {
        self.text.offset_from_row_col(row, column)
    }
    pub fn row_col_from_offset(&self, offset: usize) -> (i32, i32) {
        self.text.row_col_from_offset(offset)
    }
    pub fn copy_bytes(&self, off: usize, len: usize, out: &mut [u8]) {
        self.text.copy_bytes(off, len, out);
    }
    pub fn bytes_equal(&self, bytes: &[u8]) -> bool {
        if bytes.len() != self.byte_size() {
            return false;
        }
        let mut buffer = [0; 4096];
        for (index, chunk) in bytes.chunks(buffer.len()).enumerate() {
            self.copy_bytes(index * buffer.len(), chunk.len(), &mut buffer);
            if &buffer[..chunk.len()] != chunk {
                return false;
            }
        }
        true
    }
    pub fn line_starts(&self, out: &mut Vec<u32>) {
        self.text.line_starts(out);
    }
    pub fn contains_byte(&self, byte: u8) -> bool {
        self.text.contains_byte(byte)
    }
    pub fn snapshot(&self) -> Snapshot {
        self.text.snapshot()
    }
    pub fn mark_edited(&mut self) {
        self.version += 1;
        self.dirty = true;
    }
    pub fn mark_saved(&mut self) {
        self.dirty = false;
    }
    pub(crate) fn buffer_insert(&mut self, off: usize, bytes: &[u8]) {
        self.text.insert(off, bytes);
    }
    pub(crate) fn buffer_erase(&mut self, off: usize, len: usize) {
        self.text.erase(off, len);
    }
}

fn join_lines(lines: &[Vec<u8>], ending: &[u8]) -> Vec<u8> {
    let Some(first) = lines.first() else {
        return Vec::new();
    };
    let capacity = lines.iter().map(Vec::len).sum::<usize>() + ending.len() * (lines.len() - 1);
    let mut out = Vec::with_capacity(capacity);
    out.extend_from_slice(first);
    for line in &lines[1..] {
        out.extend_from_slice(ending);
        out.extend_from_slice(line);
    }
    out
}

#[cfg(test)]
mod tests {
    // Translated alongside tests/editor/text_buffer_test.cpp. Mutations follow
    // the upstream EditorOperations -> EditorState -> TextBuffer boundary.
    use super::*;
    use crate::editor_operations::{EditorOperations, OpKind, TextOp};

    fn loaded(raw: &[u8]) -> EditorState {
        let mut state = EditorState::new();
        state.set_from_bytes(raw);
        state
    }
    fn insert_at(state: &mut EditorState, row: i32, column: i32, text: &[u8]) {
        assert!(
            EditorOperations::new()
                .apply(
                    state,
                    &TextOp {
                        kind: OpKind::Insert,
                        row,
                        column,
                        text: text.to_vec(),
                        ..TextOp::default()
                    }
                )
                .ok
        );
    }
    fn erase_at(state: &mut EditorState, row: i32, column: i32, length: i32) {
        assert!(
            EditorOperations::new()
                .apply(
                    state,
                    &TextOp {
                        kind: OpKind::Delete,
                        row,
                        column,
                        length,
                        ..TextOp::default()
                    }
                )
                .ok
        );
    }

    #[test]
    fn empty_defaults() {
        let state = EditorState::new();
        assert_eq!(state.byte_size(), 0);
        assert_eq!(state.line_count(), 1);
        assert!(state.line(0).is_empty());
        assert!(state.join().is_empty());
        assert_eq!(state.line_length(0), 0);
        assert_eq!(state.offset_from_row_col(0, 0), 0);
    }

    #[test]
    fn assign_single_line() {
        let state = loaded(b"hello");
        assert_eq!(state.byte_size(), 5);
        assert_eq!(state.line_count(), 1);
        assert_eq!(state.line(0), b"hello");
        assert_eq!(state.join(), b"hello");
        assert_eq!(state.line_length(0), 5);
    }

    #[test]
    fn multiline_lf() {
        let state = loaded(b"aa\nbbb\nc");
        assert_eq!(state.line_count(), 3);
        assert_eq!(
            state.lines(),
            vec![b"aa".to_vec(), b"bbb".to_vec(), b"c".to_vec()]
        );
        assert_eq!(state.join(), b"aa\nbbb\nc");
        assert_eq!(
            (
                state.line_length(0),
                state.line_length(1),
                state.line_length(2)
            ),
            (2, 3, 1)
        );
    }

    #[test]
    fn multiline_crlf() {
        let state = loaded(b"aa\r\nbbb\r\nc");
        assert_eq!(state.line_count(), 3);
        assert_eq!(
            state.lines(),
            vec![b"aa".to_vec(), b"bbb".to_vec(), b"c".to_vec()]
        );
        assert_eq!(state.join(), b"aa\r\nbbb\r\nc");
    }

    #[test]
    fn line_into_reuses_and_caps_buffer() {
        let mut state = loaded(b"hello world\nnext");
        let mut out = b"garbage".to_vec();
        state.line_into(0, &mut out, 5);
        assert_eq!(out, b"hello");
        state.line_into(0, &mut out, usize::MAX);
        assert_eq!(out, b"hello world");
        state.line_into(1, &mut out, 2);
        assert_eq!(out, b"ne");
        state.line_into(0, &mut out, 0);
        assert!(out.is_empty());
        state.set_from_bytes(b"hello\r\nnext");
        state.line_into(0, &mut out, 5);
        assert_eq!(out, b"hello");
        state.line_into(0, &mut out, usize::MAX);
        assert_eq!(out, b"hello");
    }

    #[test]
    fn large_crlf_file_preserves_line_bodies() {
        let bodies: Vec<Vec<u8>> = (0..40)
            .map(|i| format!("line-{i}-xxxxxxxx").into_bytes())
            .chain(std::iter::once(b"tail".to_vec()))
            .collect();
        let state = loaded(&join_lines(&bodies, b"\r\n"));
        assert_eq!(state.line_count(), bodies.len() as i32);
        for (i, body) in bodies.iter().enumerate() {
            assert_eq!(&state.line(i as i32), body);
        }
    }

    #[test]
    fn insert_newline_mid_file_preserves_later_lines() {
        let mut raw = (0..30)
            .map(|i| format!("row{i}abcdef"))
            .collect::<Vec<_>>()
            .join("\n")
            .into_bytes();
        while raw.len() < 300 {
            raw.extend_from_slice(b"padding-line\n");
        }
        let mut state = loaded(&raw);
        let before = state.line_count();
        assert!(before > 5);
        let (line3, line4, last) = (state.line(3), state.line(4), state.line(before - 1));
        insert_at(&mut state, 3, 0, b"\n");
        assert_eq!(state.line_count(), before + 1);
        assert!(state.line(3).is_empty());
        assert_eq!(state.line(4), line3);
        assert_eq!(state.line(5), line4);
        assert_eq!(state.line(state.line_count() - 1), last);
    }

    #[test]
    fn insert_crlf_mid_file_preserves_later_lines() {
        let bodies: Vec<Vec<u8>> = (0..40)
            .map(|i| format!("L{i}-yyyyyyyy").into_bytes())
            .chain(std::iter::once(Vec::new()))
            .collect();
        let mut state = loaded(&join_lines(&bodies, b"\r\n"));
        state.line_ending = b"\r\n".to_vec();
        assert!(state.line_count() >= 40);
        let keep = state.line(10);
        insert_at(&mut state, 10, 0, b"\r\n");
        assert!(state.line(10).is_empty());
        assert_eq!(state.line(11), keep);
        assert_eq!(state.line(20), bodies[19]);
    }

    #[test]
    fn trailing_newline_yields_empty_last_line() {
        let state = loaded(b"x\n");
        assert_eq!(state.line_count(), 2);
        assert_eq!(state.line(0), b"x");
        assert!(state.line(1).is_empty());
    }

    #[test]
    fn offset_row_col_round_trip_lf() {
        let state = loaded(b"aa\nbbb\nc");
        for off in 0..=state.byte_size() {
            let (row, col) = state.row_col_from_offset(off);
            let back = state.offset_from_row_col(row, col);
            assert!(back <= state.byte_size());
            assert_eq!(state.row_col_from_offset(back), (row, col));
        }
    }

    #[test]
    fn known_row_col_offsets() {
        let state = loaded(b"aa\nbbb\nc");
        for (row, col, off) in [
            (0, 0, 0),
            (0, 2, 2),
            (1, 0, 3),
            (1, 3, 6),
            (2, 0, 7),
            (2, 1, 8),
        ] {
            assert_eq!(state.offset_from_row_col(row, col), off);
        }
    }

    #[test]
    fn insert_mid_line_via_operations() {
        let mut state = loaded(b"hello");
        insert_at(&mut state, 0, 5, b"!");
        assert_eq!(state.join(), b"hello!");
        insert_at(&mut state, 0, 0, b"X");
        assert_eq!(state.join(), b"Xhello!");
        insert_at(&mut state, 0, 2, b"y");
        assert_eq!(state.join(), b"Xhyello!");
    }

    #[test]
    fn insert_multiline_via_operations() {
        let mut state = loaded(b"ab");
        state.line_ending = b"\n".to_vec();
        state.set_from_bytes(b"ab");
        insert_at(&mut state, 0, 1, b"x\ny");
        assert_eq!(state.join(), b"ax\nyb");
        assert_eq!(state.line_count(), 2);
        assert_eq!(state.line(0), b"ax");
        assert_eq!(state.line(1), b"yb");
    }

    #[test]
    fn erase_via_operations() {
        let mut state = loaded(b"hello world");
        erase_at(&mut state, 0, 5, 1);
        assert_eq!(state.join(), b"helloworld");
        erase_at(&mut state, 0, 0, 5);
        assert_eq!(state.join(), b"world");
        let len = state.byte_size() as i32;
        erase_at(&mut state, 0, 0, len);
        assert!(state.join().is_empty());
        assert_eq!(state.line_count(), 1);
    }

    #[test]
    fn erase_across_lines() {
        let mut state = loaded(b"aa\nbbb\nc");
        erase_at(&mut state, 0, 2, 5);
        assert_eq!(state.join(), b"aac");
        assert_eq!(state.line_count(), 1);
    }

    #[test]
    fn copy_bytes() {
        let state = loaded(b"hello world");
        let mut tmp = [0; 6];
        state.copy_bytes(6, 5, &mut tmp);
        assert_eq!(&tmp[..5], b"world");
    }

    #[test]
    fn snapshot_stable_across_edits() {
        let mut state = loaded(b"hello");
        let snap = state.snapshot();
        assert_eq!(snap.bytes(), b"hello");
        assert_eq!(snap.size(), 5);
        insert_at(&mut state, 0, 5, b" world");
        assert_eq!(state.join(), b"hello world");
        assert_eq!(snap.bytes(), b"hello");
        assert_eq!(snap.size(), 5);
    }

    #[test]
    fn large_assign_and_edit() {
        let big = (0..100)
            .map(|i| format!("line {i}\n"))
            .collect::<String>()
            .into_bytes();
        let mut state = loaded(&big);
        assert_eq!(state.join(), big);
        assert_eq!(state.line_count(), 101);
        insert_at(&mut state, 0, 0, b"HEAD\n");
        assert_eq!(state.line(0), b"HEAD");
        assert_eq!(&state.join()[..5], b"HEAD\n");
        erase_at(&mut state, 0, 0, 5);
        assert_eq!(state.join(), big);
    }

    #[test]
    fn many_small_inserts_stay_coherent() {
        let mut state = EditorState::new();
        for _ in 0..200 {
            let col = state.byte_size() as i32;
            insert_at(&mut state, 0, col, b"x");
        }
        assert_eq!(state.byte_size(), 200);
        assert_eq!(state.join(), vec![b'x'; 200]);
        assert_eq!(state.line_count(), 1);
    }

    #[test]
    fn line_starts_and_contains_byte() {
        let mut state = loaded(b"aa\nbbb\nc");
        let mut starts = Vec::new();
        state.line_starts(&mut starts);
        assert_eq!(starts, [0, 3, 7]);
        assert!(!state.contains_byte(b'\t'));
        insert_at(&mut state, 0, 1, b"\t");
        assert!(state.contains_byte(b'\t'));
        let mut line = Vec::new();
        state.line_into(1, &mut line, usize::MAX);
        assert_eq!(line, b"bbb");
        state.line_into(1, &mut line, usize::MAX);
        assert_eq!(line, b"bbb");
    }

    #[test]
    fn set_line_ending_rebuilds_rope() {
        let mut state = loaded(b"a\nb");
        assert_eq!(state.line_ending, b"\n");
        state.set_line_ending(b"\r\n");
        assert_eq!(state.line_ending, b"\r\n");
        assert_eq!(state.join(), b"a\r\nb");
        assert_eq!(state.line_count(), 2);
    }

    #[test]
    fn bom_and_arbitrary_bytes_survive_loading() {
        let mut state = loaded(&[0xef, 0xbb, 0xbf, b'a', 0xff, 0, b'\n', b'b']);
        assert!(state.utf8_bom);
        assert_eq!(state.join(), [b'a', 0xff, 0, b'\n', b'b']);
        state.mark_edited();
        assert!(state.dirty);
        assert_eq!(state.version, 1);
        state.mark_saved();
        assert!(!state.dirty);
        assert_eq!(state.version, 1);
        state.set_from_bytes(b"plain");
        assert!(!state.utf8_bom);
        assert!(!state.dirty);
        assert_eq!(state.version, 0);
    }

    #[test]
    fn mixed_line_endings_use_upstream_priority() {
        let state = loaded(b"a\nb\rc\r\nd\n");
        assert_eq!(state.line_ending, b"\r\n");
        assert_eq!(state.join(), b"a\r\nb\r\nc\r\nd\r\n");
        assert_eq!(loaded(b"a\rb\n").line_ending, b"\n");
        assert_eq!(loaded(b"a\rb").line_ending, b"\r");
    }

    #[test]
    fn clamped_indices_and_crlf_terminator_mapping() {
        let state = loaded(b"aa\r\nb");
        assert_eq!(state.offset_from_row_col(-1, -1), 0);
        assert_eq!(state.offset_from_row_col(99, 99), state.byte_size());
        assert_eq!(state.row_col_from_offset(2), (0, 2));
        assert_eq!(state.row_col_from_offset(3), (1, 0));
        assert_eq!(state.row_col_from_offset(4), (1, 0));
        assert_eq!(state.row_col_from_offset(99), (1, 1));
        assert!(state.line(-1).is_empty());
        assert!(state.line(99).is_empty());
    }
}
