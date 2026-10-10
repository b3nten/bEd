// Translated from nealmick/ned editor/editor_view_state.{h,cpp} at 2d3e9b53b0ebc44c6da6901ff242a95edaa4a1ff.
// Upstream copyright and license are preserved in LICENSE and NOTICE.
use crate::{
    editor_state::EditorState,
    folding::FoldState,
    util::utf8::{next_utf8_char, prev_utf8_char, snap_to_utf8_char_boundary},
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CursorVisibility {
    pub vertical: bool,
    pub horizontal: bool,
}

/// Columns are UTF-8 byte offsets, as in the upstream editor.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    pub head_row: i32,
    pub head_column: i32,
    pub anchor_row: i32,
    pub anchor_column: i32,
    pub preferred_column: i32,
}
impl Selection {
    pub fn empty(&self) -> bool {
        (self.head_row, self.head_column) == (self.anchor_row, self.anchor_column)
    }
    pub fn collapse_to_head(&mut self) {
        self.anchor_row = self.head_row;
        self.anchor_column = self.head_column;
    }
    pub fn set_both(&mut self, row: i32, column: i32) {
        self.head_row = row;
        self.head_column = column;
        self.collapse_to_head();
    }
    pub fn ordered(&self) -> (i32, i32, i32, i32) {
        if (self.anchor_row, self.anchor_column) <= (self.head_row, self.head_column) {
            (
                self.anchor_row,
                self.anchor_column,
                self.head_row,
                self.head_column,
            )
        } else {
            (
                self.head_row,
                self.head_column,
                self.anchor_row,
                self.anchor_column,
            )
        }
    }
}

/// Carets and viewport intent. Rendering and scroll application belong to the GUI layer.
#[derive(Clone, Debug, PartialEq)]
pub struct EditorViewState {
    pub folds: FoldState,
    pub block_input: bool,
    pub request_focus: bool,
    pub selections: Vec<Selection>,
    pub primary_index: usize,
    pub row: i32,
    pub column: i32,
    pub cursor_column_preferred: i32,
    pub ensure_cursor_visible: CursorVisibility,
    pub center_cursor_vertical: bool,
    pub cursor_blink_time: f32,
    pub scroll_position: [f32; 2],
    pub requested_scroll: Option<[f32; 2]>,
    pub pending_cursor_center: Option<(i32, i32)>,
}
impl Default for EditorViewState {
    fn default() -> Self {
        Self {
            folds: FoldState::default(),
            block_input: false,
            request_focus: false,
            selections: vec![Selection::default()],
            primary_index: 0,
            row: 0,
            column: 0,
            cursor_column_preferred: 0,
            ensure_cursor_visible: CursorVisibility::default(),
            center_cursor_vertical: false,
            cursor_blink_time: 0.0,
            scroll_position: [0.0; 2],
            requested_scroll: None,
            pending_cursor_center: None,
        }
    }
}
impl EditorViewState {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn primary(&self) -> &Selection {
        &self.selections[self.primary_index.min(self.selections.len() - 1)]
    }
    pub fn primary_mut(&mut self) -> &mut Selection {
        self.ensure_selections();
        &mut self.selections[self.primary_index]
    }
    pub fn selection_count(&self) -> usize {
        self.selections.len()
    }
    pub fn has_selection(&self) -> bool {
        self.selections.iter().any(|s| !s.empty())
    }
    pub fn selection_empty(&self) -> bool {
        self.primary().empty()
    }
    pub fn get_ordered(&self) -> (i32, i32, i32, i32) {
        self.primary().ordered()
    }
    pub fn is_input_blocked(&self) -> bool {
        self.block_input
    }
    pub fn ensure_selections(&mut self) {
        if self.selections.is_empty() {
            self.selections.push(Selection::default());
        }
        if self.primary_index >= self.selections.len() {
            self.primary_index = 0;
        }
    }
    pub fn set_both(&mut self, row: i32, column: i32) {
        self.primary_mut().set_both(row, column);
        self.collapse_to_primary();
        self.sync_primary_mirrors();
    }
    pub fn collapse_selection(&mut self) {
        for s in &mut self.selections {
            s.collapse_to_head();
        }
        self.collapse_to_primary();
    }
    pub fn collapse_to_primary(&mut self) {
        self.ensure_selections();
        let keep = *self.primary();
        self.selections = vec![keep];
        self.primary_index = 0;
        self.sync_primary_mirrors();
    }
    pub fn select_all(&mut self, state: &EditorState) {
        let last = state.line_count() - 1;
        self.selections = vec![Selection {
            head_row: last,
            head_column: state.line_length(last),
            ..Selection::default()
        }];
        self.primary_index = 0;
        self.sync_primary_mirrors();
    }
    pub fn sync_primary_mirrors(&mut self) {
        self.ensure_selections();
        let p = *self.primary();
        self.row = p.head_row;
        self.column = p.head_column;
        self.cursor_column_preferred = p.preferred_column;
    }
    pub fn apply_mirrors_to_primary(&mut self) {
        self.ensure_selections();
        let empty = self.primary().empty();
        let (row, column, preferred) = (self.row, self.column, self.cursor_column_preferred);
        let p = self.primary_mut();
        p.head_row = row;
        p.head_column = column;
        p.preferred_column = preferred;
        if empty {
            p.collapse_to_head();
        }
    }
    pub fn clamp_selection(state: &EditorState, s: &mut Selection) {
        let last = (state.line_count() - 1).max(0);
        s.head_row = s.head_row.clamp(0, last);
        s.anchor_row = s.anchor_row.clamp(0, last);
        let head = state.line(s.head_row);
        let anchor = state.line(s.anchor_row);
        s.head_column =
            snap_to_utf8_char_boundary(&head, s.head_column.clamp(0, head.len() as i32));
        s.anchor_column =
            snap_to_utf8_char_boundary(&anchor, s.anchor_column.clamp(0, anchor.len() as i32));
    }
    pub fn clamp_all(&mut self, state: &EditorState) {
        self.ensure_selections();
        for s in &mut self.selections {
            Self::clamp_selection(state, s);
        }
        self.merge_selections();
        self.sync_primary_mirrors();
    }
    pub fn clamp(&mut self, state: &EditorState) {
        self.clamp_all(state);
    }
    pub fn merge_selections(&mut self) {
        self.ensure_selections();
        if self.selections.len() <= 1 {
            return;
        }
        let primary = *self.primary();
        self.selections.sort_by_key(|s| (s.head_row, s.head_column));
        let mut merged: Vec<Selection> = Vec::with_capacity(self.selections.len());
        for s in &self.selections {
            let Some(last) = merged.last_mut() else {
                merged.push(*s);
                continue;
            };
            if (last.head_row, last.head_column) == (s.head_row, s.head_column) {
                if last.empty() && !s.empty() {
                    *last = *s;
                }
                continue;
            }
            let (asr, asc, aer, aec) = last.ordered();
            let (bsr, bsc, ber, bec) = s.ordered();
            if (aer, aec) >= (bsr, bsc) && (ber, bec) >= (asr, asc) && (!last.empty() || !s.empty())
            {
                let (sr, sc) = (asr, asc).min((bsr, bsc));
                let (er, ec) = (aer, aec).max((ber, bec));
                last.anchor_row = sr;
                last.anchor_column = sc;
                last.head_row = er;
                last.head_column = ec;
            } else {
                merged.push(*s);
            }
        }
        self.selections = merged;
        self.primary_index = self
            .selections
            .iter()
            .position(|s| (s.head_row, s.head_column) == (primary.head_row, primary.head_column))
            .unwrap_or(0);
    }
    pub fn set_selections(
        &mut self,
        state: &EditorState,
        mut selections: Vec<Selection>,
        primary: usize,
    ) {
        if selections.is_empty() {
            selections.push(Selection::default());
        }
        self.primary_index = primary.min(selections.len() - 1);
        self.selections = selections;
        self.clamp_all(state);
    }
    pub fn is_position_selected(&self, row: i32, col: i32) -> bool {
        self.selections.iter().any(|s| {
            if s.empty() {
                return false;
            }
            let (sr, sc, er, ec) = s.ordered();
            if row < sr || row > er {
                false
            } else if sr == er {
                col >= sc && col < ec
            } else if row == sr {
                col >= sc
            } else if row == er {
                col < ec
            } else {
                true
            }
        })
    }
    pub fn selection_line_span(&self) -> (i32, i32) {
        let mut span: Option<(i32, i32)> = None;
        for s in &self.selections {
            if s.empty() {
                continue;
            }
            let (sr, _, er, ec) = s.ordered();
            let end = if ec > 0 || er == sr { er + 1 } else { er };
            span = Some(match span {
                Some((a, b)) => (a.min(sr), b.max(end)),
                None => (sr, end),
            });
        }
        span.unwrap_or((0, 0))
    }
    pub fn calculate_visual_column(state: &EditorState, s: &mut Selection) {
        let line = state.line(s.head_row);
        let mut visual = 0;
        // Preserve upstream's byte-counting visual preference, including non-ASCII bytes.
        for b in line.iter().take(s.head_column.max(0) as usize) {
            visual = if *b == b'\t' {
                ((visual / 4) + 1) * 4
            } else {
                visual + 1
            };
        }
        s.preferred_column = visual;
    }
    pub fn calculate_primary_visual_column(&mut self, state: &EditorState) {
        Self::calculate_visual_column(state, self.primary_mut());
        self.sync_primary_mirrors();
    }
    fn find_column_from_visual_column(state: &EditorState, s: &mut Selection, row: i32) {
        let text = state.line(row);
        let mut visual = 0;
        let mut pos = 0;
        while pos < text.len() && visual < s.preferred_column {
            if text[pos] == b'\t' {
                let next = ((visual / 4) + 1) * 4;
                if next > s.preferred_column {
                    break;
                }
                visual = next;
            } else {
                visual += 1;
            }
            pos += 1;
        }
        s.head_row = row;
        s.head_column = pos as i32;
    }
    pub fn cursor_left(state: &EditorState, s: &mut Selection) {
        if s.head_column > 0 {
            s.head_column = prev_utf8_char(&state.line(s.head_row), s.head_column);
        } else if s.head_row > 0 {
            s.head_row -= 1;
            s.head_column = state.line_length(s.head_row);
        }
        Self::calculate_visual_column(state, s);
    }
    pub fn cursor_right(state: &EditorState, s: &mut Selection) {
        let line = state.line(s.head_row);
        if s.head_column < line.len() as i32 {
            s.head_column = next_utf8_char(&line, s.head_column);
        } else if s.head_row + 1 < state.line_count() {
            s.head_row += 1;
            s.head_column = 0;
        }
        Self::calculate_visual_column(state, s);
    }
    fn move_cursor_vertically(state: &EditorState, s: &mut Selection, delta: i32) -> bool {
        if state.line_count() <= 0 {
            return false;
        }
        let target = (s.head_row + delta).clamp(0, state.line_count() - 1);
        if target == s.head_row {
            return false;
        }
        if s.preferred_column == 0 && s.head_column != 0 {
            Self::calculate_visual_column(state, s);
        }
        Self::find_column_from_visual_column(state, s, target);
        true
    }
    pub fn cursor_up(state: &EditorState, s: &mut Selection) {
        Self::move_cursor_vertically(state, s, -1);
        Self::clamp_selection(state, s);
    }
    pub fn cursor_down(state: &EditorState, s: &mut Selection) {
        Self::move_cursor_vertically(state, s, 1);
        Self::clamp_selection(state, s);
    }
    pub fn move_word_forward(state: &EditorState, s: &mut Selection) {
        let line = state.line(s.head_row);
        let len = line.len() as i32;
        let mut pos = s.head_column;
        if pos < len {
            while pos < len && !is_word_char(line[pos as usize]) {
                pos += 1;
            }
            while pos < len && is_word_char(line[pos as usize]) {
                pos += 1;
            }
        } else if s.head_row + 1 < state.line_count() {
            s.head_row += 1;
            s.head_column = 0;
            Self::calculate_visual_column(state, s);
            return;
        }
        if s.head_column != pos {
            s.head_column = pos;
            s.preferred_column = pos;
        }
    }
    pub fn move_word_backward(state: &EditorState, s: &mut Selection) {
        let line = state.line(s.head_row);
        let mut pos = s.head_column;
        if pos > 0 {
            while pos > 0 && !is_word_char(line[(pos - 1) as usize]) {
                pos -= 1;
            }
            while pos > 0 && is_word_char(line[(pos - 1) as usize]) {
                pos -= 1;
            }
        } else if s.head_row > 0 {
            s.head_row -= 1;
            s.head_column = state.line_length(s.head_row);
            Self::calculate_visual_column(state, s);
            return;
        }
        if s.head_column != pos {
            s.head_column = pos;
            s.preferred_column = pos;
        }
    }
    pub fn update_blink_time(&mut self, delta_time: f32) {
        self.cursor_blink_time += delta_time;
    }
    pub fn get_scroll_position(&self) -> [f32; 2] {
        self.scroll_position
    }
    pub fn set_scroll_position(&mut self, position: [f32; 2]) {
        self.scroll_position = position;
    }
    pub fn request_scroll(&mut self, x: f32, y: f32) {
        self.requested_scroll = Some([x, y]);
    }
    pub fn request_cursor_center(&mut self, line: i32, character: i32) {
        self.folds.reveal(line);
        self.pending_cursor_center = Some((line, character));
    }
}

pub(crate) fn is_word_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}
pub fn find_word_boundaries(line: &[u8], pos: i32) -> (i32, i32) {
    let len = line.len() as i32;
    let pos = pos.clamp(0, len);
    let mut probe = pos;
    if probe >= len || !is_word_char(line[probe as usize]) {
        if probe > 0 && is_word_char(line[(probe - 1) as usize]) {
            probe -= 1;
        } else {
            return (pos, pos);
        }
    }
    let mut start = probe;
    while start > 0 && is_word_char(line[(start - 1) as usize]) {
        start -= 1;
    }
    let mut end = probe + 1;
    while end < len && is_word_char(line[end as usize]) {
        end += 1;
    }
    (start, end)
}
