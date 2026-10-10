// Translated from nealmick/ned editor/editor_commands.{h,cpp} at 2d3e9b53b0ebc44c6da6901ff242a95edaa4a1ff.
// Upstream copyright and license are preserved in LICENSE and NOTICE.
use crate::{
    editor_events::{DidEdit, EditorEvents},
    editor_operations::{ApplyResult, EditorOperations, OpKind, TextOp},
    editor_state::EditorState,
    editor_view_state::{EditorViewState, Selection, find_word_boundaries},
    project_undo::{HistoryEdit, HistoryStep, ProjectUndo, SelectionSnapshot},
    util::utf8::{next_utf8_char, prev_utf8_char, snap_to_utf8_char_boundary},
};
use std::{
    cell::RefMut,
    ops::{Deref, DerefMut},
};

/// The standalone history or the one project store shared by embedded tabs.
/// The guard keeps a shared store exclusively borrowed for a command action.
pub enum ProjectUndoGuard<'a> {
    Borrowed(&'a mut ProjectUndo),
    Shared(RefMut<'a, ProjectUndo>),
}
impl Deref for ProjectUndoGuard<'_> {
    type Target = ProjectUndo;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Borrowed(undo) => undo,
            Self::Shared(undo) => undo,
        }
    }
}
impl DerefMut for ProjectUndoGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        match self {
            Self::Borrowed(undo) => undo,
            Self::Shared(undo) => undo,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CursorReveal {
    #[default]
    Ensure,
    Center,
}
#[derive(Clone, Copy, Default)]
enum CaretAfter {
    #[default]
    ToResult,
    Leave,
    AtPosition(i32, i32),
}
#[derive(Clone, Copy)]
struct ApplyOptions {
    caret: CaretAfter,
    collapse_selection: bool,
    selection_index: Option<usize>,
}
impl Default for ApplyOptions {
    fn default() -> Self {
        Self {
            caret: CaretAfter::ToResult,
            collapse_selection: true,
            selection_index: None,
        }
    }
}
impl ApplyOptions {
    fn at(index: usize) -> Self {
        Self {
            selection_index: Some(index),
            ..Self::default()
        }
    }
}

/// A borrowed command facade keeps document, history and view ownership separate.
pub struct EditorCommands<'a> {
    state: &'a mut EditorState,
    view: &'a mut EditorViewState,
    ops: &'a mut EditorOperations,
    project_undo: ProjectUndoGuard<'a>,
    events: &'a mut EditorEvents,
    history_key: String,
    edit_batch_depth: usize,
    edit_batch_pending: bool,
    edit_dirty_lo: i32,
    edit_dirty_hi: i32,
    undo_steps: Vec<HistoryStep>,
    undo_selections_before: Vec<SelectionSnapshot>,
    undo_primary_before: usize,
    undo_has_before: bool,
}
impl<'a> EditorCommands<'a> {
    pub fn new(
        state: &'a mut EditorState,
        view: &'a mut EditorViewState,
        ops: &'a mut EditorOperations,
        project_undo: &'a mut ProjectUndo,
        events: &'a mut EditorEvents,
    ) -> Self {
        Self::new_with_project_undo(
            state,
            view,
            ops,
            ProjectUndoGuard::Borrowed(project_undo),
            events,
        )
    }

    pub fn new_with_project_undo(
        state: &'a mut EditorState,
        view: &'a mut EditorViewState,
        ops: &'a mut EditorOperations,
        project_undo: ProjectUndoGuard<'a>,
        events: &'a mut EditorEvents,
    ) -> Self {
        let history_key = state.path.clone();
        Self {
            state,
            view,
            ops,
            project_undo,
            events,
            history_key,
            edit_batch_depth: 0,
            edit_batch_pending: false,
            edit_dirty_lo: 0,
            edit_dirty_hi: -1,
            undo_steps: Vec::new(),
            undo_selections_before: Vec::new(),
            undo_primary_before: 0,
            undo_has_before: false,
        }
    }

    pub fn with_history_key(mut self, key: String) -> Self {
        self.history_key = key;
        self
    }
    fn snapshot_selections(&self) -> Vec<SelectionSnapshot> {
        self.view
            .selections
            .iter()
            .map(|s| SelectionSnapshot {
                head_row: s.head_row,
                head_column: s.head_column,
                anchor_row: s.anchor_row,
                anchor_column: s.anchor_column,
                preferred_column: s.preferred_column,
            })
            .collect()
    }
    fn restore_selections(&mut self, snaps: &[SelectionSnapshot], primary: usize) {
        let next = snaps
            .iter()
            .map(|s| Selection {
                head_row: s.head_row,
                head_column: s.head_column,
                anchor_row: s.anchor_row,
                anchor_column: s.anchor_column,
                preferred_column: s.preferred_column,
            })
            .collect();
        self.view.set_selections(self.state, next, primary);
    }
    fn capture_selections_before_if_needed(&mut self) {
        if self.undo_has_before {
            return;
        }
        self.undo_selections_before = self.snapshot_selections();
        self.undo_primary_before = self.view.primary_index;
        self.undo_has_before = true;
    }
    fn commit_undo_group(&mut self) {
        if !self.undo_has_before || self.undo_steps.is_empty() {
            self.undo_steps.clear();
            self.undo_selections_before.clear();
            self.undo_has_before = false;
            return;
        }
        let edit = HistoryEdit {
            steps: std::mem::take(&mut self.undo_steps),
            selections_before: std::mem::take(&mut self.undo_selections_before),
            primary_before: self.undo_primary_before,
            selections_after: self.snapshot_selections(),
            primary_after: self.view.primary_index,
        };
        self.project_undo.record(&self.history_key, edit);
        self.undo_has_before = false;
    }
    fn apply_user_edit(&mut self, op: TextOp, opt: ApplyOptions) -> bool {
        if self.state.read_only {
            return false;
        }
        self.capture_selections_before_if_needed();
        let result = self.ops.apply(self.state, &op);
        if !result.ok {
            return false;
        }
        self.view.ensure_selections();
        let index = opt
            .selection_index
            .unwrap_or(self.view.primary_index)
            .min(self.view.selection_count() - 1);
        self.adjust_selections_after_edit(index, &op, &result);
        let sel = &mut self.view.selections[index];
        match opt.caret {
            CaretAfter::ToResult => {
                sel.head_row = result.end_row;
                sel.head_column = result.end_column;
            }
            CaretAfter::Leave => {}
            CaretAfter::AtPosition(r, c) => {
                sel.head_row = r;
                sel.head_column = c;
            }
        }
        if opt.collapse_selection {
            sel.collapse_to_head();
        }
        if index == self.view.primary_index {
            self.view.sync_primary_mirrors();
        }
        self.undo_steps.push(HistoryStep {
            op: op.clone(),
            deleted_text: result.deleted_text,
        });
        self.note_dirty(op.row, result.end_row);
        self.finish_user_edit();
        true
    }
    fn note_dirty(&mut self, a: i32, b: i32) {
        let (lo, hi) = (a.min(b), a.max(b));
        if self.edit_dirty_hi < self.edit_dirty_lo {
            self.edit_dirty_lo = lo;
            self.edit_dirty_hi = hi;
        } else {
            self.edit_dirty_lo = self.edit_dirty_lo.min(lo);
            self.edit_dirty_hi = self.edit_dirty_hi.max(hi);
        }
    }
    fn begin_batch(&mut self) {
        self.edit_batch_depth += 1;
    }
    fn end_batch(&mut self) {
        self.edit_batch_depth = self.edit_batch_depth.saturating_sub(1);
        if self.edit_batch_depth == 0 && self.edit_batch_pending {
            self.edit_batch_pending = false;
            self.finish_user_edit();
        }
    }
    fn emit_did_edit(&mut self, first_row: i32, last_row: i32) {
        let changes = self.ops.take_document_changes();
        self.view.folds.apply_changes(&changes);
        self.events.emit_did_edit_document(
            &DidEdit {
                version: self.state.version,
                first_row,
                last_row,
                changes,
            },
            self.state,
        );
    }
    fn finish_user_edit(&mut self) {
        if self.edit_batch_depth > 0 {
            self.edit_batch_pending = true;
            return;
        }
        self.commit_undo_group();
        let (lo, hi) = if self.edit_dirty_hi >= self.edit_dirty_lo {
            (self.edit_dirty_lo, self.edit_dirty_hi)
        } else {
            (0, (self.state.line_count() - 1).max(0))
        };
        self.edit_dirty_lo = 0;
        self.edit_dirty_hi = -1;
        self.emit_did_edit(lo, hi);
        self.request_ensure_visible();
    }
    /// Apply validated operations as one undo unit, separated from adjacent
    /// typing. Session-level transactions provide range/revision validation.
    pub fn apply_transaction(&mut self, operations: &[TextOp]) {
        self.project_undo.seal_file(&self.history_key);
        self.begin_batch();
        for op in operations {
            self.view.ensure_selections();
            let index = self.view.primary_index;
            let mut primary = self.view.selections[index];
            if self.apply_user_edit(
                op.clone(),
                ApplyOptions {
                    caret: CaretAfter::Leave,
                    collapse_selection: false,
                    selection_index: None,
                },
            ) {
                let edit = self.ops.pending_edits().last().unwrap();
                for (row, column) in [
                    (&mut primary.head_row, &mut primary.head_column),
                    (&mut primary.anchor_row, &mut primary.anchor_column),
                ] {
                    if op.kind == OpKind::Insert {
                        shift_pos_after_insert(
                            row,
                            column,
                            op.row,
                            op.column,
                            &op.text,
                            &self.state.line_ending,
                        );
                    } else {
                        shift_pos_after_delete(
                            row,
                            column,
                            op.row,
                            op.column,
                            &edit.removed_bytes,
                            &self.state.line_ending,
                        );
                    }
                }
                self.view.selections[index] = primary;
                self.view.sync_primary_mirrors();
            }
        }
        self.end_batch();
        self.project_undo.seal_file(&self.history_key);
    }

    fn selection_order_reverse(&self) -> Vec<usize> {
        let mut order: Vec<_> = (0..self.view.selection_count()).collect();
        order.sort_by_key(|&i| {
            std::cmp::Reverse((
                self.view.selections[i].head_row,
                self.view.selections[i].head_column,
            ))
        });
        order
    }
    fn adjust_selections_after_edit(&mut self, index: usize, op: &TextOp, result: &ApplyResult) {
        for (i, s) in self.view.selections.iter_mut().enumerate() {
            if i == index {
                continue;
            }
            if op.kind == OpKind::Insert {
                shift_pos_after_insert(
                    &mut s.head_row,
                    &mut s.head_column,
                    op.row,
                    op.column,
                    &op.text,
                    &self.state.line_ending,
                );
                shift_pos_after_insert(
                    &mut s.anchor_row,
                    &mut s.anchor_column,
                    op.row,
                    op.column,
                    &op.text,
                    &self.state.line_ending,
                );
            } else {
                shift_pos_after_delete(
                    &mut s.head_row,
                    &mut s.head_column,
                    op.row,
                    op.column,
                    &result.deleted_text,
                    &self.state.line_ending,
                );
                shift_pos_after_delete(
                    &mut s.anchor_row,
                    &mut s.anchor_column,
                    op.row,
                    op.column,
                    &result.deleted_text,
                    &self.state.line_ending,
                );
            }
        }
    }
    fn begin_select_gesture(&mut self, select: bool) {
        if !select {
            for s in &mut self.view.selections {
                s.collapse_to_head();
            }
        }
    }
    fn end_select_gesture(&mut self, select: bool) {
        if !select {
            for s in &mut self.view.selections {
                s.collapse_to_head();
            }
        }
        self.view.merge_selections();
        self.view.clamp_all(self.state);
        self.request_ensure_visible();
    }
    pub fn request_ensure_visible(&mut self) {
        for selection in &self.view.selections {
            self.view.folds.reveal(selection.head_row);
        }
        self.view.ensure_cursor_visible.vertical = true;
        self.view.ensure_cursor_visible.horizontal = true;
    }
    fn apply_reveal(&mut self, reveal: CursorReveal) {
        for selection in &self.view.selections {
            self.view.folds.reveal(selection.head_row);
        }
        if reveal == CursorReveal::Center {
            self.view.center_cursor_vertical = true;
            self.view.ensure_cursor_visible.horizontal = true;
        } else {
            self.request_ensure_visible();
        }
    }
    fn nav_move(&mut self, select: bool, movement: fn(&EditorState, &mut Selection)) {
        self.begin_select_gesture(select);
        for s in &mut self.view.selections {
            movement(self.state, s);
        }
        self.end_select_gesture(select);
    }
    pub fn move_left(&mut self, select: bool) {
        self.nav_move(select, EditorViewState::cursor_left);
    }
    pub fn move_right(&mut self, select: bool) {
        self.nav_move(select, EditorViewState::cursor_right);
    }
    pub fn move_up(&mut self, select: bool) {
        self.nav_move(select, EditorViewState::cursor_up);
    }
    pub fn move_down(&mut self, select: bool) {
        self.nav_move(select, EditorViewState::cursor_down);
    }
    pub fn move_word_left(&mut self, select: bool) {
        self.nav_move(select, EditorViewState::move_word_backward);
    }
    pub fn move_word_right(&mut self, select: bool) {
        self.nav_move(select, EditorViewState::move_word_forward);
    }
    pub fn move_line_start(&mut self, select: bool) {
        self.begin_select_gesture(select);
        for s in &mut self.view.selections {
            let line = self.state.line(s.head_row);
            let indent = line
                .iter()
                .take_while(|&&b| b == b' ' || b == b'\t')
                .count() as i32;
            s.head_column = if s.head_column > indent { indent } else { 0 };
            s.preferred_column = s.head_column;
        }
        self.end_select_gesture(select);
    }
    pub fn move_line_end(&mut self, select: bool) {
        self.begin_select_gesture(select);
        for s in &mut self.view.selections {
            s.head_column = self.state.line_length(s.head_row);
            s.preferred_column = s.head_column;
        }
        self.end_select_gesture(select);
    }
    pub fn move_doc_start(&mut self, select: bool) {
        self.begin_select_gesture(select);
        for s in &mut self.view.selections {
            s.head_row = 0;
            s.head_column = 0;
            s.preferred_column = 0;
        }
        self.end_select_gesture(select);
    }
    pub fn move_doc_end(&mut self, select: bool) {
        self.begin_select_gesture(select);
        let last = self.state.line_count() - 1;
        for s in &mut self.view.selections {
            s.head_row = last;
            s.head_column = self.state.line_length(last);
            EditorViewState::calculate_visual_column(self.state, s);
        }
        self.end_select_gesture(select);
    }
    pub fn move_lines(&mut self, delta: i32, select: bool) {
        if delta == 0 {
            return;
        }
        self.begin_select_gesture(select);
        for _ in 0..delta.unsigned_abs() {
            for s in &mut self.view.selections {
                if delta > 0 {
                    EditorViewState::cursor_down(self.state, s);
                } else {
                    EditorViewState::cursor_up(self.state, s);
                }
            }
        }
        self.end_select_gesture(select);
    }
    /// Apply view-computed navigation destinations in selection order. The view
    /// can navigate screen rows while this command owns anchors and merging.
    pub fn move_carets_to(&mut self, positions: &[(i32, i32)], select: bool) {
        assert_eq!(positions.len(), self.view.selections.len());
        self.begin_select_gesture(select);
        for (selection, &(row, column)) in self.view.selections.iter_mut().zip(positions) {
            selection.head_row = row;
            selection.head_column = column;
            EditorViewState::clamp_selection(self.state, selection);
            EditorViewState::calculate_visual_column(self.state, selection);
        }
        self.end_select_gesture(select);
    }
    pub fn set_cursor(&mut self, row: i32, column: i32, select: bool, reveal: CursorReveal) {
        if self.state.line_count() <= 0 {
            return;
        }
        let row = row.clamp(0, self.state.line_count() - 1);
        let line = self.state.line(row);
        let column = snap_to_utf8_char_boundary(&line, column.clamp(0, line.len() as i32));
        let mut s = if select {
            *self.view.primary()
        } else {
            Selection::default()
        };
        s.head_row = row;
        s.head_column = column;
        if !select {
            s.collapse_to_head();
        }
        self.view.selections = vec![s];
        self.view.primary_index = 0;
        self.view.calculate_primary_visual_column(self.state);
        self.apply_reveal(reveal);
    }
    pub fn set_selection(
        &mut self,
        anchor_row: i32,
        anchor_col: i32,
        active_row: i32,
        active_col: i32,
        reveal: CursorReveal,
    ) {
        if self.state.line_count() <= 0 {
            return;
        }
        let last = self.state.line_count() - 1;
        let anchor_row = anchor_row.clamp(0, last);
        let active_row = active_row.clamp(0, last);
        let anchor = self.state.line(anchor_row);
        let active = self.state.line(active_row);
        self.view.selections = vec![Selection {
            anchor_row,
            anchor_column: snap_to_utf8_char_boundary(
                &anchor,
                anchor_col.clamp(0, anchor.len() as i32),
            ),
            head_row: active_row,
            head_column: snap_to_utf8_char_boundary(
                &active,
                active_col.clamp(0, active.len() as i32),
            ),
            preferred_column: 0,
        }];
        self.view.primary_index = 0;
        self.view.calculate_primary_visual_column(self.state);
        self.apply_reveal(reveal);
    }
    pub fn set_selections(
        &mut self,
        selections: Vec<Selection>,
        primary: usize,
        reveal: CursorReveal,
    ) {
        self.view.set_selections(self.state, selections, primary);
        for s in &mut self.view.selections {
            EditorViewState::calculate_visual_column(self.state, s);
        }
        self.view.sync_primary_mirrors();
        self.apply_reveal(reveal);
    }
    pub fn go_to_line(&mut self, line: i32) {
        self.set_cursor(line, 0, false, CursorReveal::Center);
    }
    pub fn toggle_fold(&mut self, row: i32) -> bool {
        if !self.view.folds.toggle(row) {
            return false;
        }
        if self.move_hidden_endpoints_to_fold_headers() {
            self.request_ensure_visible();
        }
        true
    }
    pub fn fold_all(&mut self) {
        self.view.folds.fold_all();
        if self.move_hidden_endpoints_to_fold_headers() {
            self.request_ensure_visible();
        }
    }
    pub fn unfold_all(&mut self) {
        self.view.folds.unfold_all();
    }
    /// Only moved carets require scrolling; an adjusted anchor may be offscreen.
    fn move_hidden_endpoints_to_fold_headers(&mut self) -> bool {
        let mut moved_head = false;
        for selection in &mut self.view.selections {
            let previous_head_row = selection.head_row;
            for (row, column) in [
                (&mut selection.head_row, &mut selection.head_column),
                (&mut selection.anchor_row, &mut selection.anchor_column),
            ] {
                if let Some(range) = self
                    .view
                    .folds
                    .collapsed_ranges()
                    .iter()
                    .find(|range| range.start_line < *row && *row <= range.end_line)
                {
                    *row = range.start_line;
                    *column = self.state.line_length(*row);
                }
            }
            moved_head |= selection.head_row != previous_head_row;
            EditorViewState::calculate_visual_column(self.state, selection);
        }
        self.view.clamp_all(self.state);
        moved_head
    }
    pub fn select_all(&mut self) {
        self.view.select_all(self.state);
        self.request_ensure_visible();
    }
    pub fn collapse_selection(&mut self) {
        self.view.collapse_selection();
    }
    pub fn select_word_at(&mut self, row: i32, column: i32) {
        let row = row.clamp(0, self.state.line_count() - 1);
        let line = self.state.line(row);
        let column = snap_to_utf8_char_boundary(&line, column);
        let (start, end) = find_word_boundaries(&line, column);
        self.set_selection(row, start, row, end, CursorReveal::Ensure);
    }
    fn add_cursor_vertical(&mut self, delta: i32) {
        if self.state.line_count() <= 0 || delta == 0 {
            return;
        }
        let mut seed = *self.view.primary();
        if seed.preferred_column == 0 && seed.head_column != 0 {
            EditorViewState::calculate_visual_column(self.state, &mut seed);
        }
        let mut new = seed;
        new.collapse_to_head();
        if delta < 0 {
            EditorViewState::cursor_up(self.state, &mut new);
        } else {
            EditorViewState::cursor_down(self.state, &mut new);
        }
        new.collapse_to_head();
        if new.head_row == seed.head_row {
            return;
        }
        self.insert_cursor(new);
    }
    /// Add a caret at a view-computed position, including another screen row of
    /// the same document line. Existing caret positions remain unique.
    pub fn add_cursor_at(&mut self, row: i32, column: i32) {
        let mut new = Selection {
            head_row: row,
            head_column: column,
            ..Selection::default()
        };
        EditorViewState::clamp_selection(self.state, &mut new);
        EditorViewState::calculate_visual_column(self.state, &mut new);
        new.collapse_to_head();
        self.insert_cursor(new);
    }
    fn insert_cursor(&mut self, new: Selection) {
        if self
            .view
            .selections
            .iter()
            .any(|s| (s.head_row, s.head_column) == (new.head_row, new.head_column))
        {
            return;
        }
        self.view.selections.push(new);
        self.view.primary_index = self.view.selections.len() - 1;
        self.view.merge_selections();
        if let Some(i) = self
            .view
            .selections
            .iter()
            .position(|s| (s.head_row, s.head_column) == (new.head_row, new.head_column))
        {
            self.view.primary_index = i;
        }
        self.view.sync_primary_mirrors();
        self.request_ensure_visible();
    }
    pub fn add_cursor_above(&mut self) {
        self.add_cursor_vertical(-1);
    }
    pub fn add_cursor_below(&mut self) {
        self.add_cursor_vertical(1);
    }

    fn delete_range_at(&mut self, index: usize) {
        let s = self.view.selections[index];
        if s.empty() {
            return;
        }
        let (sr, sc, er, ec) = s.ordered();
        let length = EditorOperations::measure_length(self.state, sr, sc, er, ec);
        if length > 0 {
            self.apply_user_edit(
                TextOp {
                    kind: OpKind::Delete,
                    row: sr,
                    column: sc,
                    length,
                    ..TextOp::default()
                },
                ApplyOptions::at(index),
            );
        }
    }
    fn merge_after_edit(&mut self) {
        self.view.merge_selections();
        self.view.sync_primary_mirrors();
    }
    pub fn delete_selection(&mut self) {
        if !self.view.has_selection() {
            return;
        }
        self.begin_batch();
        for index in self.selection_order_reverse() {
            self.delete_range_at(index);
        }
        self.merge_after_edit();
        self.end_batch();
        for s in &mut self.view.selections {
            s.preferred_column = 0;
        }
        self.view.sync_primary_mirrors();
    }
    pub fn type_text(&mut self, utf8: &[u8]) {
        if utf8.is_empty() {
            return;
        }
        self.begin_batch();
        for index in self.selection_order_reverse() {
            self.delete_range_at(index);
            let s = self.view.selections[index];
            self.apply_user_edit(
                TextOp {
                    row: s.head_row,
                    column: s.head_column,
                    text: utf8.to_vec(),
                    ..TextOp::default()
                },
                ApplyOptions::at(index),
            );
        }
        self.merge_after_edit();
        self.end_batch();
    }
    pub fn insert_newline(&mut self) {
        self.begin_batch();
        for index in self.selection_order_reverse() {
            self.delete_range_at(index);
            let s = self.view.selections[index];
            let line = self.state.line(s.head_row);
            let count = line
                .iter()
                .take(s.head_column.max(0) as usize)
                .take_while(|&&b| b == b' ' || b == b'\t')
                .count();
            let mut text = self.state.line_ending.clone();
            text.extend_from_slice(&line[..count]);
            self.apply_user_edit(
                TextOp {
                    row: s.head_row,
                    column: s.head_column,
                    text,
                    ..TextOp::default()
                },
                ApplyOptions::at(index),
            );
            self.view.selections[index].preferred_column = 0;
        }
        self.merge_after_edit();
        self.end_batch();
    }
    pub fn delete_left(&mut self, by_word: bool) {
        if self.view.has_selection() {
            self.delete_selection();
            return;
        }
        self.begin_batch();
        for index in self.selection_order_reverse() {
            let s = self.view.selections[index];
            let op = if by_word {
                let mut start = s;
                EditorViewState::move_word_backward(self.state, &mut start);
                self.view.selections[index].preferred_column = start.preferred_column;
                TextOp {
                    kind: OpKind::Delete,
                    row: start.head_row,
                    column: start.head_column,
                    length: EditorOperations::measure_length(
                        self.state,
                        start.head_row,
                        start.head_column,
                        s.head_row,
                        s.head_column,
                    ),
                    ..TextOp::default()
                }
            } else if s.head_column > 0 {
                let line = self.state.line(s.head_row);
                let pos = snap_to_utf8_char_boundary(&line, s.head_column);
                let prev = prev_utf8_char(&line, pos);
                TextOp {
                    kind: OpKind::Delete,
                    row: s.head_row,
                    column: prev,
                    length: pos - prev,
                    ..TextOp::default()
                }
            } else if s.head_row > 0 {
                TextOp {
                    kind: OpKind::Delete,
                    row: s.head_row - 1,
                    column: self.state.line_length(s.head_row - 1),
                    length: self.state.line_ending.len() as i32,
                    ..TextOp::default()
                }
            } else {
                TextOp {
                    kind: OpKind::Delete,
                    ..TextOp::default()
                }
            };
            if op.length > 0 {
                self.apply_user_edit(op, ApplyOptions::at(index));
            }
            self.view.selections[index].preferred_column = 0;
        }
        self.merge_after_edit();
        self.end_batch();
    }
    pub fn delete_right(&mut self, by_word: bool) {
        if self.view.has_selection() {
            self.delete_selection();
            return;
        }
        self.begin_batch();
        for index in self.selection_order_reverse() {
            let s = self.view.selections[index];
            let line = self.state.line(s.head_row);
            let op = if by_word {
                let mut end = s;
                EditorViewState::move_word_forward(self.state, &mut end);
                self.view.selections[index].preferred_column = end.preferred_column;
                TextOp {
                    kind: OpKind::Delete,
                    row: s.head_row,
                    column: s.head_column,
                    length: EditorOperations::measure_length(
                        self.state,
                        s.head_row,
                        s.head_column,
                        end.head_row,
                        end.head_column,
                    ),
                    ..TextOp::default()
                }
            } else if s.head_column < line.len() as i32 {
                let pos = snap_to_utf8_char_boundary(&line, s.head_column);
                let next = next_utf8_char(&line, pos);
                TextOp {
                    kind: OpKind::Delete,
                    row: s.head_row,
                    column: pos,
                    length: next - pos,
                    ..TextOp::default()
                }
            } else if s.head_row + 1 < self.state.line_count() {
                TextOp {
                    kind: OpKind::Delete,
                    row: s.head_row,
                    column: self.state.line_length(s.head_row),
                    length: self.state.line_ending.len() as i32,
                    ..TextOp::default()
                }
            } else {
                TextOp {
                    kind: OpKind::Delete,
                    ..TextOp::default()
                }
            };
            if op.length > 0 {
                self.apply_user_edit(op, ApplyOptions::at(index));
            }
            self.view.selections[index].preferred_column = 0;
        }
        self.merge_after_edit();
        self.end_batch();
    }
    pub fn copy(&self) -> Vec<u8> {
        if self.view.selection_empty() {
            return Vec::new();
        }
        let (sr, sc, er, ec) = self.view.get_ordered();
        let text = EditorOperations::extract_text(self.state, sr, sc, er, ec);
        let le = &self.state.line_ending;
        let platform = EditorState::platform_line_ending();
        if le.is_empty() || *le == platform {
            return text;
        }
        let mut out = Vec::new();
        let mut i = 0;
        while i < text.len() {
            if text[i..].starts_with(le) {
                out.extend_from_slice(&platform);
                i += le.len();
            } else {
                out.push(text[i]);
                i += 1;
            }
        }
        out
    }
    pub fn cut(&mut self) -> Vec<u8> {
        if !self.view.selection_empty() {
            let text = self.copy();
            self.delete_selection();
            return text;
        }
        let row = self.view.row;
        let last = row + 1 >= self.state.line_count();
        let mut text = self.state.line(row);
        text.extend_from_slice(&EditorState::platform_line_ending());
        let (r, c, len) = if last && row > 0 {
            (
                row - 1,
                self.state.line_length(row - 1),
                self.state.line_ending.len() as i32 + self.state.line_length(row),
            )
        } else if last {
            (0, 0, self.state.line_length(0))
        } else {
            (
                row,
                0,
                self.state.line_length(row) + self.state.line_ending.len() as i32,
            )
        };
        self.apply_user_edit(
            TextOp {
                kind: OpKind::Delete,
                row: r,
                column: c,
                length: len,
                ..TextOp::default()
            },
            ApplyOptions::at(self.view.primary_index),
        );
        self.view.collapse_to_primary();
        text
    }
    fn normalize_paste(&self, raw: &[u8]) -> Vec<u8> {
        let mut text = Vec::new();
        if self.state.contains_byte(b'\t') {
            let mut i = 0;
            while i < raw.len() {
                if raw[i] == b' ' {
                    let mut count = 0;
                    while i < raw.len() && raw[i] == b' ' && count < 4 {
                        count += 1;
                        i += 1;
                    }
                    if count == 4 {
                        text.push(b'\t');
                    } else {
                        text.extend(std::iter::repeat_n(b' ', count));
                    }
                } else {
                    text.push(raw[i]);
                    i += 1;
                }
            }
        } else {
            for &byte in raw {
                if byte == b'\t' {
                    text.extend_from_slice(b"    ");
                } else {
                    text.push(byte);
                }
            }
        }
        EditorOperations::normalize_line_endings(self.state, &text)
    }
    pub fn paste(&mut self, raw: &[u8]) {
        let text = self.normalize_paste(raw);
        if text.is_empty() {
            return;
        }
        self.type_text(&text);
        self.request_ensure_visible();
    }
    pub fn indent(&mut self) {
        if !self.view.selection_empty() {
            self.indent_multi_line();
        } else {
            self.indent_single_line();
        }
    }
    fn indent_single_line(&mut self) {
        self.begin_batch();
        for index in self.selection_order_reverse() {
            let s = self.view.selections[index];
            self.apply_user_edit(
                TextOp {
                    row: s.head_row,
                    text: vec![b'\t'],
                    ..TextOp::default()
                },
                ApplyOptions {
                    caret: CaretAfter::AtPosition(s.head_row, s.head_column + 1),
                    ..ApplyOptions::at(index)
                },
            );
            self.view.selections[index].preferred_column = 0;
        }
        self.merge_after_edit();
        self.end_batch();
    }
    fn indent_multi_line(&mut self) {
        let (sr, _, mut er, ec) = self.view.get_ordered();
        if ec == 0 && er > sr {
            er -= 1;
        }
        self.begin_batch();
        for row in (sr..=er).rev() {
            self.apply_user_edit(
                TextOp {
                    row,
                    text: vec![b'\t'],
                    ..TextOp::default()
                },
                ApplyOptions {
                    caret: CaretAfter::Leave,
                    collapse_selection: false,
                    selection_index: Some(self.view.primary_index),
                },
            );
        }
        self.end_batch();
        let p = self.view.primary_mut();
        if (sr..=er).contains(&p.anchor_row) {
            p.anchor_column += 1;
        }
        if (sr..=er).contains(&p.head_row) {
            p.head_column += 1;
        }
        self.view.sync_primary_mirrors();
    }
    pub fn outdent(&mut self) {
        let (sr, _, mut er, ec) = if !self.view.selection_empty() {
            self.view.get_ordered()
        } else {
            (
                self.view.row,
                self.view.column,
                self.view.row,
                self.view.column,
            )
        };
        if ec == 0 && er > sr {
            er -= 1;
        }
        let mut removed = vec![0; (er - sr + 1) as usize];
        self.begin_batch();
        for row in (sr..=er).rev() {
            let line = self.state.line(row);
            let n = if line.starts_with(b"    ") {
                4
            } else if line.first() == Some(&b'\t') {
                1
            } else {
                0
            };
            if n == 0 {
                continue;
            }
            if self.apply_user_edit(
                TextOp {
                    kind: OpKind::Delete,
                    row,
                    length: n,
                    ..TextOp::default()
                },
                ApplyOptions {
                    caret: CaretAfter::Leave,
                    collapse_selection: false,
                    selection_index: Some(self.view.primary_index),
                },
            ) {
                removed[(row - sr) as usize] = n;
            }
        }
        self.end_batch();
        let state = &self.state;
        let adjust = |row: i32, col: &mut i32| {
            if (sr..=er).contains(&row) {
                *col = (*col - removed[(row - sr) as usize])
                    .max(0)
                    .min(state.line_length(row));
            }
        };
        let p = self.view.primary_mut();
        if p.empty() {
            adjust(p.head_row, &mut p.head_column);
            p.collapse_to_head();
        } else {
            adjust(p.anchor_row, &mut p.anchor_column);
            adjust(p.head_row, &mut p.head_column);
        }
        self.view.sync_primary_mirrors();
    }
    fn apply_history(&mut self, edit: &HistoryEdit, is_undo: bool) {
        let mut any = false;
        self.edit_dirty_lo = 0;
        self.edit_dirty_hi = -1;
        if is_undo {
            for step in edit.steps.iter().rev() {
                let op = EditorOperations::invert(&step.op, &step.deleted_text);
                let result = self.ops.apply(self.state, &op);
                if result.ok {
                    any = true;
                    self.note_dirty(op.row, result.end_row);
                }
            }
            self.restore_selections(&edit.selections_before, edit.primary_before);
        } else {
            for step in &edit.steps {
                let result = self.ops.apply(self.state, &step.op);
                if result.ok {
                    any = true;
                    self.note_dirty(step.op.row, result.end_row);
                }
            }
            self.restore_selections(&edit.selections_after, edit.primary_after);
        }
        self.view.clamp_all(self.state);
        if any {
            let (lo, hi) = if self.edit_dirty_hi >= self.edit_dirty_lo {
                (self.edit_dirty_lo, self.edit_dirty_hi)
            } else {
                (0, (self.state.line_count() - 1).max(0))
            };
            self.emit_did_edit(lo, hi);
        }
        self.request_ensure_visible();
    }
    pub fn undo(&mut self) {
        if self.state.read_only {
            return;
        }
        if self.undo_has_before && !self.undo_steps.is_empty() {
            self.commit_undo_group();
        }
        if let Some(edit) = self.project_undo.undo(&self.history_key) {
            self.apply_history(&edit, true);
            self.request_ensure_visible();
        }
    }
    pub fn redo(&mut self) {
        if self.state.read_only {
            return;
        }
        if self.undo_has_before && !self.undo_steps.is_empty() {
            self.commit_undo_group();
        }
        if let Some(edit) = self.project_undo.redo(&self.history_key) {
            self.apply_history(&edit, false);
            self.request_ensure_visible();
        }
    }
}

fn shift_pos_after_insert(
    row: &mut i32,
    col: &mut i32,
    at_row: i32,
    at_col: i32,
    text: &[u8],
    le: &[u8],
) {
    if (*row, *col) < (at_row, at_col) {
        return;
    }
    let mut newlines = 0;
    let mut last_break = 0;
    let mut i = 0;
    if !le.is_empty() {
        while i + le.len() <= text.len() {
            if text[i..].starts_with(le) {
                newlines += 1;
                last_break = i;
                i += le.len();
            } else {
                i += 1;
            }
        }
    }
    if newlines == 0 {
        if *row == at_row {
            *col += text.len() as i32;
        }
        return;
    }
    let last_len = (text.len() - (last_break + le.len())) as i32;
    if *row == at_row {
        *col = (*col - at_col) + last_len;
        *row = at_row + newlines;
    } else {
        *row += newlines;
    }
}
fn shift_pos_after_delete(
    row: &mut i32,
    col: &mut i32,
    at_row: i32,
    at_col: i32,
    deleted: &[u8],
    le: &[u8],
) {
    let (mut end_row, mut end_col) = (at_row, at_col);
    let mut i = 0;
    while i < deleted.len() {
        if !le.is_empty() && deleted[i..].starts_with(le) {
            end_row += 1;
            end_col = 0;
            i += le.len();
        } else {
            end_col += 1;
            i += 1;
        }
    }
    if (*row, *col) < (at_row, at_col) {
        return;
    }
    if (*row, *col) < (end_row, end_col) {
        *row = at_row;
        *col = at_col;
        return;
    }
    if end_row == at_row {
        if *row == at_row {
            *col -= end_col - at_col;
        }
        return;
    }
    if *row == end_row {
        *row = at_row;
        *col = at_col + (*col - end_col);
    } else {
        *row -= end_row - at_row;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::folding::FoldRange;
    use std::{cell::RefCell, rc::Rc};

    struct Fixture {
        state: EditorState,
        view: EditorViewState,
        ops: EditorOperations,
        undo: ProjectUndo,
        events: EditorEvents,
    }
    impl Fixture {
        fn new(text: &[u8]) -> Self {
            let mut state = EditorState::new();
            state.set_from_bytes(text);
            state.path = "/test/document".to_owned();
            Self {
                state,
                view: EditorViewState::new(),
                ops: EditorOperations::new(),
                undo: ProjectUndo::new(String::new()),
                events: EditorEvents::new(),
            }
        }
        fn commands(&mut self) -> EditorCommands<'_> {
            EditorCommands::new(
                &mut self.state,
                &mut self.view,
                &mut self.ops,
                &mut self.undo,
                &mut self.events,
            )
        }
        fn caret(&mut self, row: i32, col: i32) {
            self.commands()
                .set_cursor(row, col, false, CursorReveal::Ensure);
        }
        fn selection(&mut self, ar: i32, ac: i32, hr: i32, hc: i32) {
            self.commands()
                .set_selection(ar, ac, hr, hc, CursorReveal::Ensure);
        }
        fn carets(&mut self, carets: &[(i32, i32)], primary: usize) {
            self.commands().set_selections(
                carets
                    .iter()
                    .map(|&(r, c)| Selection {
                        head_row: r,
                        head_column: c,
                        anchor_row: r,
                        anchor_column: c,
                        preferred_column: 0,
                    })
                    .collect(),
                primary,
                CursorReveal::Ensure,
            );
        }
        fn content(&self) -> Vec<u8> {
            self.state.join()
        }
    }

    #[test]
    fn folding_moves_hidden_selection_endpoints_and_navigation_reveals_targets() {
        let mut f = Fixture::new(b"fn outer() {\n  a\n  b\n}\nafter");
        f.selection(1, 1, 2, 2);
        f.view.folds.set_ranges(vec![FoldRange {
            start_line: 0,
            end_line: 3,
        }]);
        assert!(f.commands().toggle_fold(2));
        assert!(f.view.folds.is_hidden(2));
        assert_eq!(f.view.get_ordered(), (0, 12, 0, 12));
        f.commands().set_cursor(2, 1, false, CursorReveal::Center);
        assert!(!f.view.folds.is_hidden(2));
        assert_eq!((f.view.row, f.view.column), (2, 1));
        f.commands().fold_all();
        assert_eq!((f.view.row, f.view.column), (0, 12));
        f.commands().set_selection(0, 0, 4, 5, CursorReveal::Ensure);
        assert!(f.view.folds.is_hidden(2));
        assert_eq!(f.commands().copy(), b"fn outer() {\n  a\n  b\n}\nafter");
        f.commands().unfold_all();
        assert!(!f.view.folds.is_hidden(2));
    }

    #[test]
    fn folding_offscreen_blocks_does_not_request_scrolling_to_an_unaffected_caret() {
        let mut f = Fixture::new(b"before\nmore\nfn outer() {\n  a\n}\nafter");
        f.view.folds.set_ranges(vec![FoldRange {
            start_line: 2,
            end_line: 4,
        }]);
        f.view.scroll_position = [0.0, 100.0];
        assert!(f.commands().toggle_fold(2));
        assert!(f.view.folds.is_hidden(3));
        assert_eq!(f.view.ensure_cursor_visible, Default::default());
        f.commands().unfold_all();
        assert!(!f.view.folds.is_hidden(3));
        assert_eq!(f.view.ensure_cursor_visible, Default::default());
        f.commands().fold_all();
        assert!(f.view.folds.is_hidden(3));
        assert_eq!(f.view.ensure_cursor_visible, Default::default());
        assert_eq!(f.view.scroll_position, [0.0, 100.0]);

        // Folding may adjust an anchor while the active caret stays above it.
        f.commands().unfold_all();
        f.selection(3, 1, 0, 0);
        f.view.ensure_cursor_visible = Default::default();
        f.commands().toggle_fold(2);
        assert_eq!(f.view.primary().anchor_row, 2);
        assert_eq!(f.view.primary().head_row, 0);
        assert_eq!(f.view.ensure_cursor_visible, Default::default());
    }

    // Cases translated from tests/editor/editor_commands_test.cpp.
    #[test]
    fn type_text_inserts_and_advances_caret() {
        let mut f = Fixture::new(b"hello");
        f.caret(0, 5);
        f.commands().type_text(b"!");
        assert_eq!(f.content(), b"hello!");
        assert_eq!((f.view.row, f.view.column), (0, 6));
        assert!(f.state.dirty);
    }
    #[test]
    fn type_text_multibyte_utf8() {
        let mut f = Fixture::new(b"");
        f.commands().type_text("é".as_bytes());
        assert_eq!(f.content(), "é".as_bytes());
        assert_eq!(f.view.column, 2);
        assert_eq!(f.state.line(0), "é".as_bytes());
        let mut f = Fixture::new(b"a");
        f.caret(0, 1);
        f.commands().type_text("📚".as_bytes());
        f.commands().type_text(b"b");
        assert_eq!(f.content(), "a📚b".as_bytes());
        assert_eq!(f.view.column, 6);
    }
    #[test]
    fn type_text_replaces_selection_in_one_did_edit() {
        let mut f = Fixture::new(b"abcdef");
        f.selection(0, 1, 0, 4);
        let edits = Rc::new(RefCell::new(Vec::<DidEdit>::new()));
        let seen = edits.clone();
        f.events.subscribe_did_edit(Box::new(move |e: &DidEdit| {
            seen.borrow_mut().push(e.clone())
        }));
        f.commands().type_text(b"X");
        assert_eq!(f.content(), b"aXef");
        assert_eq!((f.view.row, f.view.column), (0, 2));
        assert!(!f.view.has_selection());
        assert_eq!(edits.borrow().len(), 1);
        assert_eq!(edits.borrow()[0].changes.len(), 2);
    }
    #[test]
    fn did_edit_carries_incremental_changes() {
        let mut f = Fixture::new(b"hello");
        f.caret(0, 5);
        let edits = Rc::new(RefCell::new(Vec::<DidEdit>::new()));
        let seen = edits.clone();
        f.events.subscribe_did_edit(Box::new(move |e: &DidEdit| {
            seen.borrow_mut().push(e.clone())
        }));
        f.commands().type_text(b"!");
        let edits = edits.borrow();
        assert_eq!(edits[0].changes.len(), 1);
        let change = &edits[0].changes[0];
        assert_eq!(
            (
                change.start_line,
                change.start_character,
                change.end_line,
                change.end_character
            ),
            (0, 5, 0, 5)
        );
        assert_eq!(change.text, b"!");
    }
    #[test]
    fn type_text_empty_is_noop() {
        let mut f = Fixture::new(b"x");
        f.caret(0, 1);
        let version = f.state.version;
        f.commands().type_text(b"");
        assert_eq!(f.content(), b"x");
        assert_eq!(f.state.version, version);
        assert_eq!(f.view.column, 1);
    }
    #[test]
    fn delete_left_mid_line_and_line_start() {
        let mut f = Fixture::new(b"abcd");
        f.caret(0, 3);
        f.commands().delete_left(false);
        assert_eq!(f.content(), b"abd");
        assert_eq!(f.view.column, 2);
        let mut f = Fixture::new(b"ab\ncd");
        f.caret(1, 0);
        f.commands().delete_left(false);
        assert_eq!(f.content(), b"abcd");
        assert_eq!((f.view.row, f.view.column), (0, 2));
    }
    #[test]
    fn delete_left_removes_whole_utf8_character_and_selection() {
        let mut f = Fixture::new("aéx".as_bytes());
        f.caret(0, 3);
        f.commands().delete_left(false);
        assert_eq!(f.content(), b"ax");
        assert_eq!(f.view.column, 1);
        let mut f = Fixture::new(b"hello world");
        f.selection(0, 0, 0, 5);
        f.commands().delete_left(false);
        assert_eq!(f.content(), b" world");
        assert_eq!(f.view.column, 0);
    }
    #[test]
    fn delete_right_mid_line() {
        let mut f = Fixture::new(b"abcd");
        f.caret(0, 1);
        f.commands().delete_right(false);
        assert_eq!(f.content(), b"acd");
        assert_eq!(f.view.column, 1);
    }
    #[test]
    fn insert_newline_copies_leading_indent() {
        let mut f = Fixture::new(b"\tfoo");
        f.caret(0, 4);
        f.commands().insert_newline();
        assert_eq!(f.state.line_count(), 2);
        assert_eq!(f.state.line(0), b"\tfoo");
        assert_eq!(f.state.line(1), b"\t");
        assert_eq!((f.view.row, f.view.column), (1, 1));
    }
    #[test]
    fn indent_single_and_multi_line() {
        let mut f = Fixture::new(b"foo");
        f.caret(0, 2);
        f.commands().indent();
        assert_eq!(f.content(), b"\tfoo");
        assert_eq!(f.view.column, 3);
        let mut f = Fixture::new(b"a\nb\nc");
        f.selection(0, 0, 1, 1);
        f.commands().indent();
        assert_eq!(f.state.line(0), b"\ta");
        assert_eq!(f.state.line(1), b"\tb");
        assert_eq!(f.state.line(2), b"c");
    }
    #[test]
    fn outdent_tab_and_four_spaces() {
        let mut f = Fixture::new(b"\tfoo");
        f.caret(0, 2);
        f.commands().outdent();
        assert_eq!(f.content(), b"foo");
        let mut f = Fixture::new(b"    foo");
        f.caret(0, 4);
        f.commands().outdent();
        assert_eq!(f.content(), b"foo");
    }
    #[test]
    fn paste_and_replace_selection() {
        let mut f = Fixture::new(b"ab");
        f.caret(0, 1);
        f.commands().paste(b"XY");
        assert_eq!(f.content(), b"aXYb");
        assert_eq!(f.view.column, 3);
        let mut f = Fixture::new(b"hello");
        f.selection(0, 1, 0, 4);
        f.commands().paste(b"XX");
        assert_eq!(f.content(), b"hXXo");
    }
    #[test]
    fn multiline_type_and_paste() {
        for paste in [false, true] {
            let mut f = Fixture::new(b"ab");
            f.state.line_ending = b"\n".to_vec();
            f.caret(0, 1);
            if paste {
                f.commands().paste(b"x\ny");
            } else {
                f.commands().type_text(b"x\ny");
            }
            assert_eq!(f.state.lines(), vec![b"ax".to_vec(), b"yb".to_vec()]);
        }
        let f = Fixture::new(b"a\nb");
        assert_eq!(
            EditorOperations::normalize_line_endings(&f.state, b"x\r\ny"),
            b"x\ny"
        );
    }
    #[test]
    fn copy_and_cut_primary_selection() {
        let mut f = Fixture::new(b"hello world");
        f.selection(0, 0, 0, 5);
        assert_eq!(f.commands().copy(), b"hello");
        assert_eq!(f.content(), b"hello world");
        assert_eq!(f.commands().cut(), b"hello");
        assert_eq!(f.content(), b" world");
    }
    #[test]
    fn select_all_then_delete() {
        let mut f = Fixture::new(b"one\ntwo");
        f.commands().select_all();
        assert!(f.view.has_selection());
        f.commands().delete_selection();
        assert_eq!(f.state.line_count(), 1);
        assert!(f.state.line(0).is_empty());
        assert!(f.content().is_empty());
    }
    #[test]
    fn move_left_right_and_shift_selection() {
        let mut f = Fixture::new(b"ab");
        f.caret(0, 1);
        f.commands().move_right(false);
        assert_eq!(f.view.column, 2);
        f.commands().move_left(false);
        assert_eq!(f.view.column, 1);
        let mut f = Fixture::new(b"abc");
        f.caret(0, 2);
        f.commands().move_left(true);
        assert!(f.view.has_selection());
        assert_eq!(f.view.get_ordered(), (0, 1, 0, 2));
    }

    // Cases translated from tests/editor/multi_cursor_test.cpp.
    #[test]
    fn multi_cursor_type_at_every_caret() {
        let mut f = Fixture::new(b"abc\ndef\nghi");
        f.carets(&[(0, 1), (1, 1), (2, 1)], 0);
        f.commands().type_text(b"X");
        assert_eq!(f.content(), b"aXbc\ndXef\ngXhi");
        assert_eq!(f.view.selection_count(), 3);
    }
    #[test]
    fn multi_cursor_one_undo_restores_text_and_carets() {
        let mut f = Fixture::new(b"abc\ndef");
        f.carets(&[(0, 1), (1, 1)], 0);
        f.commands().type_text(b"Z");
        assert_eq!(f.content(), b"aZbc\ndZef");
        assert_eq!(f.view.selection_count(), 2);
        f.commands().undo();
        assert_eq!(f.content(), b"abc\ndef");
        assert_eq!(f.view.selection_count(), 2);
        assert_eq!(
            (
                f.view.selections[0].head_row,
                f.view.selections[0].head_column
            ),
            (0, 1)
        );
        assert_eq!(
            (
                f.view.selections[1].head_row,
                f.view.selections[1].head_column
            ),
            (1, 1)
        );
        f.commands().redo();
        assert_eq!(f.content(), b"aZbc\ndZef");
        assert_eq!(f.view.selection_count(), 2);
    }
    #[test]
    fn multi_cursor_same_line_shifts_later_heads() {
        let mut f = Fixture::new(b"abcdef");
        f.carets(&[(0, 1), (0, 4)], 0);
        f.commands().type_text(b"X");
        assert_eq!(f.content(), b"aXbcdXef");
        assert_eq!(f.view.selection_count(), 2);
        assert_eq!(f.view.selections[0].head_column, 2);
        assert_eq!(f.view.selections[1].head_column, 6);
    }
    #[test]
    fn multi_cursor_delete_left() {
        let mut f = Fixture::new(b"aXbYc");
        f.carets(&[(0, 2), (0, 4)], 0);
        f.commands().delete_left(false);
        assert_eq!(f.content(), b"abc");
    }
    #[test]
    fn multi_cursor_replace_ranges_and_undo() {
        let mut f = Fixture::new(b"one two three");
        f.commands().set_selections(
            vec![
                Selection {
                    anchor_column: 0,
                    head_column: 3,
                    ..Selection::default()
                },
                Selection {
                    anchor_column: 8,
                    head_column: 13,
                    ..Selection::default()
                },
            ],
            0,
            CursorReveal::Ensure,
        );
        f.commands().type_text(b"X");
        assert_eq!(f.content(), b"X two X");
        f.commands().undo();
        assert_eq!(f.content(), b"one two three");
        assert_eq!(f.view.selection_count(), 2);
        assert!(f.view.has_selection());
    }
    #[test]
    fn adjacent_single_caret_inserts_coalesce() {
        let mut f = Fixture::new(b"");
        f.commands().type_text(b"a");
        f.commands().type_text(b"b");
        f.commands().type_text(b"c");
        assert_eq!(f.content(), b"abc");
        f.commands().undo();
        assert_eq!(f.content(), b"");
    }
    #[test]
    fn replace_selection_is_one_undo_group() {
        let mut f = Fixture::new(b"abcdef");
        f.selection(0, 1, 0, 4);
        f.commands().type_text(b"Z");
        assert_eq!(f.content(), b"aZef");
        f.commands().undo();
        assert_eq!(f.content(), b"abcdef");
    }
    #[test]
    fn collapse_selection_drops_secondary_carets() {
        let mut f = Fixture::new(b"a\nb\nc");
        f.carets(&[(0, 0), (1, 0), (2, 0)], 1);
        assert_eq!(f.view.selection_count(), 3);
        f.commands().collapse_selection();
        assert_eq!(f.view.selection_count(), 1);
        assert_eq!((f.view.row, f.view.column), (1, 0));
    }
    #[test]
    fn navigation_moves_every_caret_without_creating_ranges() {
        let mut f = Fixture::new(b"aa\nbb\ncc");
        f.carets(&[(0, 0), (1, 0)], 0);
        f.commands().move_right(false);
        assert_eq!(f.view.selections[0].head_column, 1);
        assert_eq!(f.view.selections[1].head_column, 1);
        assert!(!f.view.has_selection());
        assert!(f.view.selections.iter().all(Selection::empty));
        let mut f = Fixture::new(b"abcdef");
        f.caret(0, 2);
        f.commands().move_right(false);
        assert!(!f.view.has_selection());
        assert!(f.view.primary().empty());
        assert_eq!(f.view.column, 3);
        f.commands().move_left(false);
        f.commands().move_down(false);
        assert!(!f.view.has_selection());
    }
    #[test]
    fn shift_move_extends_and_plain_move_clears() {
        let mut f = Fixture::new(b"abcdef");
        f.caret(0, 1);
        f.commands().move_right(true);
        f.commands().move_right(true);
        assert!(f.view.has_selection());
        assert_eq!(f.view.get_ordered(), (0, 1, 0, 3));
        f.commands().move_right(false);
        assert!(!f.view.has_selection());
        assert_eq!(f.view.column, 4);
    }
    #[test]
    fn add_cursor_below_builds_column() {
        let mut f = Fixture::new(b"aaa\nbbb\nccc\nddd");
        f.caret(0, 1);
        f.commands().add_cursor_below();
        assert_eq!(f.view.selection_count(), 2);
        assert_eq!(
            (f.view.primary().head_row, f.view.primary().head_column),
            (1, 1)
        );
        f.commands().add_cursor_below();
        assert_eq!(f.view.selection_count(), 3);
        assert_eq!(f.view.primary().head_row, 2);
        f.commands().type_text(b"X");
        assert_eq!(f.content(), b"aXaa\nbXbb\ncXcc\nddd");
    }
    #[test]
    fn add_cursor_above_follows_new_caret_and_stops_at_edge() {
        let mut f = Fixture::new(b"aaa\nbbb\nccc");
        f.caret(2, 1);
        f.commands().add_cursor_above();
        assert_eq!(f.view.selection_count(), 2);
        assert_eq!(f.view.primary().head_row, 1);
        f.commands().add_cursor_above();
        assert_eq!(f.view.selection_count(), 3);
        assert_eq!(f.view.primary().head_row, 0);
        f.commands().add_cursor_above();
        assert_eq!(f.view.selection_count(), 3);
    }
    #[test]
    fn add_cursor_below_ignores_existing_caret() {
        let mut f = Fixture::new(b"aaa\nbbb");
        f.carets(&[(0, 1), (1, 1)], 0);
        f.commands().add_cursor_below();
        assert_eq!(f.view.selection_count(), 2);
    }
    // Additional cases from tests/editor/undo_service_test.cpp.
    #[test]
    fn insert_then_delete_are_separate_undo_steps() {
        let mut f = Fixture::new(b"x");
        f.caret(0, 1);
        f.commands().type_text(b"y");
        assert_eq!(f.content(), b"xy");
        f.commands().delete_left(false);
        assert_eq!(f.content(), b"x");
        f.commands().undo();
        assert_eq!(f.content(), b"xy");
        f.commands().undo();
        assert_eq!(f.content(), b"x");
    }
    #[test]
    fn new_edit_after_undo_discards_redo_branch() {
        let mut f = Fixture::new(b"");
        f.commands().type_text(b"a");
        f.commands().undo();
        assert_eq!(f.content(), b"");
        f.commands().type_text(b"b");
        f.commands().redo();
        assert_eq!(f.content(), b"b");
    }
    #[test]
    fn independent_sessions_can_share_project_history() {
        let mut a = Fixture::new(b"A");
        let mut b = Fixture::new(b"B");
        a.state.path = "file-a".to_owned();
        b.state.path = "file-b".to_owned();
        a.caret(0, 1);
        b.caret(0, 1);
        let mut shared = ProjectUndo::default();
        shared.ensure_file(&a.state.path);
        shared.ensure_file(&b.state.path);
        EditorCommands::new(
            &mut a.state,
            &mut a.view,
            &mut a.ops,
            &mut shared,
            &mut a.events,
        )
        .type_text(b"1");
        EditorCommands::new(
            &mut b.state,
            &mut b.view,
            &mut b.ops,
            &mut shared,
            &mut b.events,
        )
        .type_text(b"2");
        assert_eq!(a.content(), b"A1");
        assert_eq!(b.content(), b"B2");
        EditorCommands::new(
            &mut b.state,
            &mut b.view,
            &mut b.ops,
            &mut shared,
            &mut b.events,
        )
        .undo();
        assert_eq!(b.content(), b"B");
        assert_eq!(a.content(), b"A1");
        EditorCommands::new(
            &mut a.state,
            &mut a.view,
            &mut a.ops,
            &mut shared,
            &mut a.events,
        )
        .undo();
        assert_eq!(a.content(), b"A");
    }

    #[test]
    fn find_all_shape_spawns_selections_and_undo_restores() {
        let mut f = Fixture::new(b"foo bar foo baz foo");
        let sels = [0, 8, 16]
            .iter()
            .map(|&col| Selection {
                anchor_column: col,
                head_column: col + 3,
                ..Selection::default()
            })
            .collect();
        f.commands().set_selections(sels, 1, CursorReveal::Ensure);
        assert_eq!(f.view.selection_count(), 3);
        assert!(f.view.has_selection());
        assert_eq!(f.view.primary_index, 1);
        assert_eq!(f.view.primary().anchor_column, 8);
        f.commands().type_text(b"X");
        assert_eq!(f.content(), b"X bar X baz X");
        f.commands().undo();
        assert_eq!(f.content(), b"foo bar foo baz foo");
        assert_eq!(f.view.selection_count(), 3);
    }
}
