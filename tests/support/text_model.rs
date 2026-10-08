// Test-only translation of nealmick/ned tests/monaco/text_model.{h,cpp}, pinned
// at 2d3e9b53b0ebc44c6da6901ff242a95edaa4a1ff. MIT/X Consortium; see LICENSE and NOTICE.
// Preserve the upstream one-based byte-coordinate facade over the real model.
use bed_editing::{
    editor_operations::{EditorOperations, OpKind, TextOp},
    editor_state::EditorState,
    util::utf8::snap_to_utf8_char_boundary,
};

pub type Position = (i32, i32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Range(pub i32, pub i32, pub i32, pub i32);
impl Range {
    // Public upstream helpers remain available to future translated fixtures.
    #[allow(dead_code)]
    pub fn is_empty(self) -> bool {
        (self.0, self.1) == (self.2, self.3)
    }
    pub fn ordered(self) -> Self {
        let Self(sl, sc, el, ec) = self;
        if (sl, sc) > (el, ec) {
            Self(el, ec, sl, sc)
        } else {
            self
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndOfLinePreference {
    TextDefined,
    Lf,
    CrLf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SingleEditOperation {
    pub range: Range,
    pub text: Vec<u8>,
}

pub struct TextModel {
    state: EditorState,
    ops: EditorOperations,
}

impl TextModel {
    pub fn create(text: &str) -> Self {
        let mut state = EditorState::new();
        state.set_from_bytes(text.as_bytes());
        Self {
            state,
            ops: EditorOperations::new(),
        }
    }
    pub fn create_from_lines(lines: &[&str]) -> Self {
        Self::create(&lines.join("\n"))
    }
    pub fn set_value(&mut self, text: &str) {
        self.state.set_from_bytes(text.as_bytes());
        self.ops.clear_pending();
    }
    pub fn set_eol(&mut self, eol: &str) {
        self.state.set_line_ending(eol.as_bytes());
    }
    pub fn get_value(&self) -> Vec<u8> {
        self.state.join()
    }
    pub fn get_eol(&self) -> &[u8] {
        &self.state.line_ending
    }
    pub fn get_line_count(&self) -> i32 {
        self.state.line_count().max(1)
    }
    pub fn get_line_content(&self, line_number: i32) -> Vec<u8> {
        if self.state.line_count() <= 0 {
            return Vec::new();
        }
        self.state
            .line(line_number.clamp(1, self.state.line_count()) - 1)
    }
    pub fn get_lines_content(&self) -> Vec<Vec<u8>> {
        self.state.lines()
    }

    #[allow(dead_code)]
    pub fn state(&self) -> &EditorState {
        &self.state
    }
    #[allow(dead_code)]
    pub fn state_mut(&mut self) -> &mut EditorState {
        &mut self.state
    }

    fn to_ned(&self, p: Position) -> (i32, i32) {
        if self.state.line_count() <= 0 {
            return (0, 0);
        }
        let row = (p.0 - 1).max(0).clamp(0, self.state.line_count() - 1);
        let col = (p.1 - 1).max(0).clamp(0, self.state.line_length(row));
        (row, col)
    }
    fn clamp_range(&self, r: Range) -> Range {
        let Range(sl, sc, el, ec) = r.ordered();
        let s = self.validate_position((sl, sc));
        let e = self.validate_position((el, ec));
        Range(s.0, s.1, e.0, e.1).ordered()
    }
    pub fn validate_position(&self, mut p: Position) -> Position {
        if self.state.line_count() <= 0 {
            return (1, 1);
        }
        p.0 = p.0.max(1);
        p.1 = p.1.max(1);
        if p.0 > self.state.line_count() {
            p.0 = self.state.line_count();
            p.1 = self.state.line_length(p.0 - 1) + 1;
            return p;
        }
        let line = self.state.line(p.0 - 1);
        p.1 = p.1.min(line.len() as i32 + 1);
        p.1 = snap_to_utf8_char_boundary(&line, p.1 - 1) + 1;
        p
    }
    #[allow(dead_code)]
    pub fn validate_range(&self, r: Range) -> Range {
        let s = self.validate_position((r.0, r.1));
        let e = self.validate_position((r.2, r.3));
        Range(s.0, s.1, e.0, e.1).ordered()
    }
    pub fn get_value_in_range(&self, range: Range, eol: EndOfLinePreference) -> Vec<u8> {
        let Range(sl, sc, el, ec) = self.clamp_range(range);
        let (sr, sc) = self.to_ned((sl, sc));
        let (er, ec) = self.to_ned((el, ec));
        let text = EditorOperations::extract_text(&self.state, sr, sc, er, ec);
        if eol == EndOfLinePreference::TextDefined {
            return text;
        }
        let want: &[u8] = if eol == EndOfLinePreference::Lf {
            b"\n"
        } else {
            b"\r\n"
        };
        if want == self.state.line_ending {
            return text;
        }
        let le = &self.state.line_ending;
        let (mut out, mut i) = (Vec::with_capacity(text.len()), 0);
        while i < text.len() {
            if !le.is_empty() && text[i..].starts_with(le) {
                out.extend_from_slice(want);
                i += le.len();
            } else {
                out.push(text[i]);
                i += 1;
            }
        }
        out
    }
    pub fn get_value_length_in_range(&self, range: Range, eol: EndOfLinePreference) -> i32 {
        self.get_value_in_range(range, eol).len() as i32
    }
    pub fn modify_position(&self, p: Position, offset: i32) -> Position {
        let (row, col) = self.to_ned(self.validate_position(p));
        let off = self.state.offset_from_row_col(row, col);
        let off = if offset >= 0 {
            (off + offset as usize).min(self.state.byte_size())
        } else {
            off.saturating_sub(offset.unsigned_abs() as usize)
        };
        let (row, col) = self.state.row_col_from_offset(off);
        (row + 1, col + 1)
    }
    pub fn get_line_first_non_whitespace_column(&self, line_number: i32) -> i32 {
        let line = self.get_line_content(line_number);
        line.iter()
            .position(|b| !matches!(b, b' ' | b'\t'))
            .map_or(0, |i| i as i32 + 1)
    }
    pub fn get_line_last_non_whitespace_column(&self, line_number: i32) -> i32 {
        let line = self.get_line_content(line_number);
        line.iter()
            .rposition(|b| !matches!(b, b' ' | b'\t'))
            .map_or(0, |i| i as i32 + 2)
    }
    fn apply_one(&mut self, raw: &SingleEditOperation) -> SingleEditOperation {
        let Range(sl, sc, el, ec) = self.clamp_range(raw.range);
        let (sr, sc) = self.to_ned((sl, sc));
        let (er, ec) = self.to_ned((el, ec));
        let text = EditorOperations::normalize_line_endings(&self.state, &raw.text);
        let delete_len = EditorOperations::measure_length(&self.state, sr, sc, er, ec);
        let mut deleted = Vec::new();
        if delete_len > 0 {
            deleted = self
                .ops
                .apply(
                    &mut self.state,
                    &TextOp {
                        kind: OpKind::Delete,
                        row: sr,
                        column: sc,
                        length: delete_len,
                        ..TextOp::default()
                    },
                )
                .deleted_text;
        }
        let (mut end_row, mut end_col) = (sr, sc);
        if !text.is_empty() {
            let result = self.ops.apply(
                &mut self.state,
                &TextOp {
                    row: sr,
                    column: sc,
                    text,
                    ..TextOp::default()
                },
            );
            (end_row, end_col) = (result.end_row, result.end_column);
        }
        SingleEditOperation {
            range: Range(sr + 1, sc + 1, end_row + 1, end_col + 1),
            text: deleted,
        }
    }
    pub fn apply_edits(
        &mut self,
        operations: &[SingleEditOperation],
        compute_undo_edits: bool,
    ) -> Vec<SingleEditOperation> {
        if operations.is_empty() {
            return Vec::new();
        }
        let before = if compute_undo_edits {
            self.get_value()
        } else {
            Vec::new()
        };
        let mut order: Vec<usize> = (0..operations.len()).collect();
        order.sort_by_key(|&i| {
            let Range(sl, sc, _, _) = operations[i].range.ordered();
            std::cmp::Reverse((sl, sc))
        });
        let mut last_inverse = None;
        for idx in order {
            last_inverse = Some(self.apply_one(&operations[idx]));
        }
        if !compute_undo_edits {
            return Vec::new();
        }
        if operations.len() == 1 {
            return vec![last_inverse.unwrap()];
        }
        vec![SingleEditOperation {
            range: Range(
                1,
                1,
                self.get_line_count(),
                self.get_line_content(self.get_line_count()).len() as i32 + 1,
            ),
            text: before,
        }]
    }
}

pub fn edit_op(sl: i32, sc: i32, el: i32, ec: i32, text_lines: &[&str]) -> SingleEditOperation {
    SingleEditOperation {
        range: Range(sl, sc, el, ec),
        text: text_lines.join("\n").into_bytes(),
    }
}
pub fn create_single_edit_op(
    text: &str,
    line: i32,
    column: i32,
    selection: Option<Position>,
) -> SingleEditOperation {
    let (sl, sc) = selection.unwrap_or((line, column));
    SingleEditOperation {
        range: Range(sl, sc, line, column),
        text: text.as_bytes().to_vec(),
    }
}
