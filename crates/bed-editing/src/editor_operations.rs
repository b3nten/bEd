// Translated from nealmick/ned editor/editor_operations.{h,cpp} at 2d3e9b53b0ebc44c6da6901ff242a95edaa4a1ff.
// Upstream copyright and license are preserved in LICENSE and NOTICE.
use crate::{
    editor_events::DocumentChange, editor_state::EditorState, util::utf8::utf8_byte_offset_to_utf16,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OpKind {
    #[default]
    Insert,
    Delete,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TextOp {
    pub kind: OpKind,
    pub row: i32,
    pub column: i32,
    pub text: Vec<u8>,
    pub length: i32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PendingEdit {
    pub op: TextOp,
    pub start_byte: u32,
    pub old_end_byte: u32,
    pub new_end_byte: u32,
    pub removed_bytes: Vec<u8>,
    pub range_start_line: i32,
    pub range_start_character: i32,
    pub range_end_line: i32,
    pub range_end_character: i32,
}

impl PendingEdit {
    pub fn to_document_change(&self) -> DocumentChange {
        DocumentChange {
            start_line: self.range_start_line,
            start_character: self.range_start_character,
            end_line: self.range_end_line,
            end_character: self.range_end_character,
            text: if self.op.kind == OpKind::Insert {
                self.op.text.clone()
            } else {
                Vec::new()
            },
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ApplyResult {
    pub ok: bool,
    pub deleted_text: Vec<u8>,
    pub end_row: i32,
    pub end_column: i32,
}

/// Document mutations only; the document is supplied per call to avoid a raw session pointer.
#[derive(Default)]
pub struct EditorOperations {
    pending: Vec<PendingEdit>,
    generation: u64,
    event_cursor: usize,
}

impl EditorOperations {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn bump_generation(&mut self) {
        self.generation += 1;
    }
    pub fn pending_edits(&self) -> &[PendingEdit] {
        &self.pending
    }
    pub fn take_pending(&mut self) -> Vec<PendingEdit> {
        self.event_cursor = 0;
        std::mem::take(&mut self.pending)
    }
    pub fn clear_pending(&mut self) {
        self.pending.clear();
        self.event_cursor = 0;
    }

    /// Session transactions publish changes before handing this queue to the
    /// optional highlighter. Reinstalled edits must never emit LSP changes twice.
    pub fn install_highlight_edits(&mut self, edits: Vec<PendingEdit>) {
        self.event_cursor = edits.len();
        self.pending = edits;
    }
    /// Events and highlighting share the same ordered edits without consuming
    /// one another's input. A user action publishes each change exactly once.
    pub fn take_document_changes(&mut self) -> Vec<DocumentChange> {
        let changes = self.pending[self.event_cursor..]
            .iter()
            .map(PendingEdit::to_document_change)
            .collect();
        self.event_cursor = self.pending.len();
        changes
    }

    pub fn normalize_line_endings(state: &EditorState, text: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(text.len());
        let mut i = 0;
        while i < text.len() {
            if text[i] == b'\r' && text.get(i + 1) == Some(&b'\n') {
                out.extend_from_slice(&state.line_ending);
                i += 2;
            } else if text[i] == b'\n' || text[i] == b'\r' {
                out.extend_from_slice(&state.line_ending);
                i += 1;
            } else {
                out.push(text[i]);
                i += 1;
            }
        }
        out
    }

    fn normalize_range(
        state: &EditorState,
        sr: i32,
        sc: i32,
        er: i32,
        ec: i32,
    ) -> Option<(i32, i32, i32, i32)> {
        if state.line_count() <= 0 {
            return None;
        }
        let sr = sr.clamp(0, state.line_count() - 1);
        let er = er.clamp(0, state.line_count() - 1);
        let sc = sc.clamp(0, state.line_length(sr));
        let ec = ec.clamp(0, state.line_length(er));
        Some(if (sr, sc) > (er, ec) {
            (er, ec, sr, sc)
        } else {
            (sr, sc, er, ec)
        })
    }

    pub fn measure_length(state: &EditorState, sr: i32, sc: i32, er: i32, ec: i32) -> i32 {
        let Some((sr, sc, er, ec)) = Self::normalize_range(state, sr, sc, er, ec) else {
            return 0;
        };
        (state.offset_from_row_col(er, ec) - state.offset_from_row_col(sr, sc)) as i32
    }

    pub fn extract_text(state: &EditorState, sr: i32, sc: i32, er: i32, ec: i32) -> Vec<u8> {
        let Some((sr, sc, er, ec)) = Self::normalize_range(state, sr, sc, er, ec) else {
            return Vec::new();
        };
        let a = state.offset_from_row_col(sr, sc);
        let b = state.offset_from_row_col(er, ec);
        let mut out = vec![0; b.saturating_sub(a)];
        let len = out.len();
        state.copy_bytes(a, len, &mut out);
        out
    }

    pub fn invert(op: &TextOp, deleted_text: &[u8]) -> TextOp {
        TextOp {
            kind: if op.kind == OpKind::Insert {
                OpKind::Delete
            } else {
                OpKind::Insert
            },
            row: op.row,
            column: op.column,
            text: if op.kind == OpKind::Delete {
                deleted_text.to_vec()
            } else {
                Vec::new()
            },
            length: if op.kind == OpKind::Insert {
                op.text.len() as i32
            } else {
                0
            },
        }
    }

    /// Exact offset mutation for byte documents. The caller validates a complete
    /// transaction before invoking this method and owns its undo grouping.
    pub fn splice_bytes(
        &mut self,
        state: &mut EditorState,
        range: std::ops::Range<usize>,
        bytes: &[u8],
    ) -> Option<Vec<u8>> {
        if range.start > range.end || range.end > state.byte_size() {
            return None;
        }
        let mut removed = vec![0; range.len()];
        let len = removed.len();
        state.copy_bytes(range.start, len, &mut removed);
        state.buffer_erase(range.start, range.len());
        state.buffer_insert(range.start, bytes);
        self.bump_generation();
        state.mark_edited();
        Some(removed)
    }

    pub fn apply(&mut self, state: &mut EditorState, op: &TextOp) -> ApplyResult {
        let row = op.row.clamp(0, (state.line_count() - 1).max(0));
        let line = state.line(row);
        let column = op.column.clamp(0, line.len() as i32);
        let start = state.offset_from_row_col(op.row, op.column);
        let mut edit = PendingEdit {
            op: op.clone(),
            start_byte: start as u32,
            range_start_line: row,
            range_start_character: utf8_byte_offset_to_utf16(&line, column),
            ..PendingEdit::default()
        };
        let result = if op.kind == OpKind::Insert {
            edit.old_end_byte = edit.start_byte;
            edit.new_end_byte = edit.start_byte.wrapping_add(op.text.len() as u32);
            edit.range_end_line = edit.range_start_line;
            edit.range_end_character = edit.range_start_character;
            if op.text.is_empty() {
                ApplyResult {
                    ok: true,
                    end_row: row,
                    end_column: column,
                    ..ApplyResult::default()
                }
            } else {
                state.buffer_insert(start, &op.text);
                let (end_row, end_column) = state.row_col_from_offset(start + op.text.len());
                ApplyResult {
                    ok: true,
                    end_row,
                    end_column,
                    ..ApplyResult::default()
                }
            }
        } else {
            edit.old_end_byte = edit.start_byte.wrapping_add(op.length.max(0) as u32);
            edit.new_end_byte = edit.start_byte;
            let (end_row, end_column) = if op.length > 0 {
                state.row_col_from_offset((start + op.length as usize).min(state.byte_size()))
            } else {
                (row, column)
            };
            edit.range_end_line = end_row;
            edit.range_end_character = utf8_byte_offset_to_utf16(&state.line(end_row), end_column);
            if op.length <= 0 {
                ApplyResult {
                    ok: op.length == 0,
                    end_row: row,
                    end_column: column,
                    ..ApplyResult::default()
                }
            } else {
                let start = start.min(state.byte_size());
                let len = (op.length as usize).min(state.byte_size() - start);
                let mut deleted_text = vec![0; len];
                state.copy_bytes(start, len, &mut deleted_text);
                if len > 0 {
                    state.buffer_erase(start, len);
                }
                let (end_row, end_column) = state.row_col_from_offset(start);
                ApplyResult {
                    ok: true,
                    deleted_text,
                    end_row,
                    end_column,
                }
            }
        };
        if result.ok {
            if op.kind == OpKind::Delete {
                edit.removed_bytes = result.deleted_text.clone();
            }
            self.pending.push(edit);
            self.bump_generation();
            state.mark_edited();
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn insert_delete_invert_round_trip() {
        let mut state = EditorState::new();
        state.set_from_bytes(b"hello world");
        let mut ops = EditorOperations::new();
        let op = TextOp {
            kind: OpKind::Delete,
            column: 5,
            length: 1,
            ..TextOp::default()
        };
        let r = ops.apply(&mut state, &op);
        assert!(r.ok);
        assert_eq!(state.join(), b"helloworld");
        assert!(
            ops.apply(&mut state, &EditorOperations::invert(&op, &r.deleted_text))
                .ok
        );
        assert_eq!(state.join(), b"hello world");
    }
    #[test]
    fn multiline_insert_and_utf16_ranges() {
        let mut state = EditorState::new();
        state.set_from_bytes(b"ab");
        state.line_ending = b"\n".to_vec();
        let mut ops = EditorOperations::new();
        assert!(
            ops.apply(
                &mut state,
                &TextOp {
                    column: 1,
                    text: b"x\ny".to_vec(),
                    ..TextOp::default()
                }
            )
            .ok
        );
        assert_eq!(state.lines(), vec![b"ax".to_vec(), b"yb".to_vec()]);
        state.set_from_bytes("café x".as_bytes());
        ops.clear_pending();
        ops.apply(
            &mut state,
            &TextOp {
                column: 5,
                text: b"y".to_vec(),
                ..TextOp::default()
            },
        );
        let edit = &ops.pending_edits()[0];
        assert_eq!(
            (
                edit.range_start_line,
                edit.range_start_character,
                edit.range_end_line,
                edit.range_end_character
            ),
            (0, 4, 0, 4)
        );
        assert_eq!(edit.op.text, b"y");
    }
    #[test]
    fn delete_spans_utf16_range() {
        let mut state = EditorState::new();
        state.set_from_bytes(b"hello");
        let mut ops = EditorOperations::new();
        assert!(
            ops.apply(
                &mut state,
                &TextOp {
                    kind: OpKind::Delete,
                    column: 1,
                    length: 3,
                    ..TextOp::default()
                }
            )
            .ok
        );
        let edit = &ops.pending_edits()[0];
        assert_eq!(
            (
                edit.range_start_line,
                edit.range_start_character,
                edit.range_end_line,
                edit.range_end_character
            ),
            (0, 1, 0, 4)
        );
    }
    #[test]
    fn offset_row_col_round_trip() {
        let mut state = EditorState::new();
        state.set_from_bytes(b"aa\nbbb\nc");
        for off in 0..=state.byte_size() {
            let (row, col) = state.row_col_from_offset(off);
            assert_eq!(state.offset_from_row_col(row, col), off);
        }
    }
    #[test]
    fn event_changes_publish_once_while_highlight_retains_all_edits() {
        let mut state = EditorState::new();
        let mut ops = EditorOperations::new();
        for (column, text) in [(0, b"a".to_vec()), (1, b"b".to_vec())] {
            ops.apply(
                &mut state,
                &TextOp {
                    column,
                    text,
                    ..TextOp::default()
                },
            );
            assert_eq!(ops.take_document_changes().len(), 1);
            assert!(ops.take_document_changes().is_empty());
        }
        assert_eq!(ops.pending_edits().len(), 2);
        assert_eq!(ops.take_pending().len(), 2);
        ops.apply(
            &mut state,
            &TextOp {
                column: 2,
                text: b"c".to_vec(),
                ..TextOp::default()
            },
        );
        assert_eq!(ops.take_document_changes()[0].start_character, 2);
        ops.clear_pending();
        assert!(ops.take_document_changes().is_empty());
    }
}
