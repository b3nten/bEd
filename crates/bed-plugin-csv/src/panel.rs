use crate::{
    model::{self, ColumnFilter, FilterOp, Sort, SortMode, Table},
    worker::{Job, View, Worker},
};
use bed_core::identity::DocumentId;
use bed_plugin::{EditToken, HostContext, HostRequest, PanelAction, PluginPanel, Revision};
use bed_session::editor_session::ByteEdit;
use dear_imgui_rs::{
    FocusedFlags, InputTextCallback, InputTextFlags, Key, ListClipper, MouseButton, StyleVar,
    TableColumnFlags, TableColumnWidth, TableFlags, TableRowFlags, Ui,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{any::Any, ops::RangeInclusive, sync::Arc};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Selection {
    anchor: [usize; 2],
    focus: [usize; 2],
}
impl Selection {
    fn single(row: usize, column: usize) -> Self {
        Self {
            anchor: [row, column],
            focus: [row, column],
        }
    }
    fn rows(self) -> RangeInclusive<usize> {
        self.anchor[0].min(self.focus[0])..=self.anchor[0].max(self.focus[0])
    }
    fn columns(self) -> RangeInclusive<usize> {
        self.anchor[1].min(self.focus[1])..=self.anchor[1].max(self.focus[1])
    }
    fn contains(self, row: usize, column: usize) -> bool {
        self.rows().contains(&row) && self.columns().contains(&column)
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct ViewState {
    delimiter: Option<u8>,
    header: Option<bool>,
    widths: Vec<f32>,
    sort: Option<Sort>,
    filters: Vec<ColumnFilter>,
    global_filter: String,
    selection: Option<Selection>,
    scroll: [f32; 2],
}

struct Draft {
    record: usize,
    column: usize,
    text: String,
    revision: Revision,
    focus: bool,
    expanded: bool,
    conflicted: bool,
}

pub struct CsvPanel {
    document: DocumentId,
    worker: Worker,
    state: ViewState,
    revision: Option<Revision>,
    loaded_revision: Option<Revision>,
    serial: u64,
    submitted: bool,
    updating: bool,
    awaiting_revision: bool,
    view: Option<View>,
    parse_error: Option<String>,
    message: Option<String>,
    draft: Option<Draft>,
    pending_token: Option<EditToken>,
    pending_draft: Option<Draft>,
    pending_state: Option<ViewState>,
    find_open: bool,
    find_focus: bool,
    find: String,
    select_all_pending: bool,
    restore_scroll: bool,
    reveal_selection: bool,
    drag_selecting: bool,
}
impl CsvPanel {
    /// Whether the grid has caught up with the current document and interpretation.
    pub fn is_ready(&self) -> bool {
        self.ready()
    }
    /// Current parse or edit error, for embedding hosts and diagnostics.
    pub fn error(&self) -> Option<&str> {
        self.parse_error.as_deref().or(self.message.as_deref())
    }
    pub fn new(document: DocumentId, state: &Value) -> Self {
        let mut state: ViewState = serde_json::from_value(state.clone()).unwrap_or_default();
        state.widths.truncate(510);
        for width in &mut state.widths {
            *width = if width.is_finite() {
                width.clamp(40.0, 2000.0)
            } else {
                150.0
            };
        }
        for scroll in &mut state.scroll {
            *scroll = if scroll.is_finite() {
                scroll.max(0.0)
            } else {
                0.0
            };
        }
        let worker = Worker::new();
        let serial = worker.invalidate();
        Self {
            document,
            worker,
            state,
            revision: None,
            loaded_revision: None,
            serial,
            submitted: false,
            updating: true,
            awaiting_revision: false,
            view: None,
            parse_error: None,
            message: None,
            draft: None,
            pending_token: None,
            pending_draft: None,
            pending_state: None,
            find_open: false,
            find_focus: false,
            find: String::new(),
            select_all_pending: false,
            restore_scroll: true,
            reveal_selection: false,
            drag_selecting: false,
        }
    }
    fn invalidate(&mut self) {
        // A filter can hide the edited record; keep its draft reachable above the grid.
        if let Some(draft) = &mut self.draft {
            draft.expanded = true;
        }
        self.serial = self.worker.invalidate();
        self.submitted = false;
        self.updating = true;
        self.parse_error = None;
    }
    fn sync(&mut self, host: &HostContext<'_>) {
        let Some(document) = host.document(self.document) else {
            self.parse_error = Some("This document is no longer open.".into());
            return;
        };
        if self.revision != Some(document.revision) {
            if let Some(draft) = &mut self.draft
                && draft.revision != document.revision
            {
                draft.conflicted = true;
                self.message = Some("The document changed in another view. Your draft is retained; restart the edit before applying it.".into());
            }
            self.revision = Some(document.revision);
            self.awaiting_revision = false;
            self.invalidate();
        }
        if !self.submitted {
            self.submitted = self.worker.submit(Job {
                serial: self.serial,
                revision: document.revision,
                bytes: Arc::clone(&document.bytes),
                path: document.path.clone(),
                delimiter: self.state.delimiter,
                header: self.state.header,
                global_filter: self.state.global_filter.clone(),
                filters: self.state.filters.clone(),
                sort: self.state.sort.clone(),
            });
        }
        let outputs: Vec<_> = self.worker.poll().collect();
        for output in outputs {
            if output.serial != self.serial || Some(output.revision) != self.revision {
                continue;
            }
            self.updating = false;
            self.loaded_revision = Some(output.revision);
            match output.result {
                Ok(view) => {
                    self.state.widths.resize(view.table.columns, 150.0);
                    self.state
                        .filters
                        .retain(|filter| filter.column < view.table.columns);
                    if self
                        .state
                        .sort
                        .as_ref()
                        .is_some_and(|sort| sort.column >= view.table.columns)
                    {
                        self.state.sort = None;
                    }
                    self.view = Some(view);
                    self.parse_error = None;
                    self.clamp_selection();
                    if self.select_all_pending {
                        self.select_all();
                    }
                }
                Err(error) => {
                    self.view = None;
                    self.parse_error = Some(error);
                }
            }
        }
    }
    fn ready(&self) -> bool {
        self.view.is_some()
            && !self.updating
            && !self.awaiting_revision
            && self.pending_token.is_none()
            && self.loaded_revision == self.revision
    }
    fn transformed(&self) -> bool {
        self.state.sort.is_some()
            || !self.state.global_filter.is_empty()
            || !self.state.filters.is_empty()
    }
    fn clamp_selection(&mut self) {
        let Some(view) = &self.view else {
            return;
        };
        if view.rows.is_empty() {
            self.state.selection = None;
            return;
        }
        if self.state.selection.is_none() {
            self.state.selection = Some(Selection::single(0, 0));
        }
        if let Some(selection) = &mut self.state.selection {
            for point in [&mut selection.anchor, &mut selection.focus] {
                point[0] = point[0].min(view.rows.len() - 1);
                point[1] = point[1].min(view.table.columns - 1);
            }
        }
    }
    fn select_all(&mut self) {
        if !self.ready() {
            self.select_all_pending = true;
            return;
        }
        self.select_all_pending = false;
        if let Some(view) = &self.view {
            self.state.selection = (!view.rows.is_empty()).then(|| Selection {
                anchor: [0, 0],
                focus: [view.rows.len() - 1, view.table.columns - 1],
            });
        } else {
            self.select_all_pending = true;
        }
    }
    fn select(&mut self, row: usize, column: usize, extend: bool) {
        if extend && let Some(selection) = &mut self.state.selection {
            selection.focus = [row, column];
        } else {
            self.state.selection = Some(Selection::single(row, column));
        }
    }
    fn begin_edit(
        &mut self,
        row: usize,
        column: usize,
        replacement: Option<String>,
        expanded: bool,
    ) {
        if !self.ready() || self.draft.is_some() {
            return;
        }
        let Some(view) = &self.view else {
            return;
        };
        let Some(&record) = view.rows.get(row) else {
            return;
        };
        self.begin_record_edit(record, column, replacement, expanded);
    }
    fn begin_record_edit(
        &mut self,
        record: usize,
        column: usize,
        replacement: Option<String>,
        expanded: bool,
    ) {
        if !self.ready() || self.draft.is_some() {
            return;
        }
        let view = self.view.as_ref().unwrap();
        let text = replacement.unwrap_or_else(|| view.table.cell(record, column).into_owned());
        let expanded = expanded || text.contains(['\r', '\n']);
        self.draft = Some(Draft {
            record,
            column,
            text,
            revision: self.revision.unwrap(),
            focus: true,
            expanded,
            conflicted: false,
        });
        self.message = None;
    }
    fn queue_edits(
        &mut self,
        result: Result<Vec<ByteEdit>, String>,
        requests: &mut Vec<HostRequest>,
    ) -> Result<(), String> {
        let edits = result?;
        if !edits.is_empty() {
            let token = EditToken::next();
            requests.push(HostRequest::ApplyEditsWithResult {
                token,
                document: self.document,
                revision: self.revision.ok_or("Document is loading")?,
                edits,
            });
            self.awaiting_revision = true;
            self.pending_token = Some(token);
            self.pending_state = Some(self.state.clone());
        }
        self.message = None;
        Ok(())
    }
    fn commit(
        &mut self,
        host: &HostContext<'_>,
        requests: &mut Vec<HostRequest>,
    ) -> Result<(), String> {
        if self.pending_token.is_some() {
            return Err("The previous table edit is still being applied.".into());
        }
        let Some(draft) = &self.draft else {
            return Ok(());
        };
        let revision = host
            .document(self.document)
            .ok_or("This document is no longer open")?
            .revision;
        if draft.conflicted
            || revision != draft.revision
            || self.loaded_revision != Some(draft.revision)
        {
            let error = "The document changed while this cell was being edited. Your draft is retained; restart the edit against the current document.".to_owned();
            self.message = Some(error.clone());
            return Err(error);
        }
        let table = &self.view.as_ref().ok_or("CSV is still loading")?.table;
        let edits = if table.cell(draft.record, draft.column).as_ref() == draft.text {
            Ok(Vec::new())
        } else {
            table.replace_cells(&[(draft.record, draft.column, draft.text.clone())])
        };
        if let Err(error) = self.queue_edits(edits, requests) {
            self.message = Some(error.clone());
            return Err(error);
        }
        if self.pending_token.is_some() {
            self.pending_draft = self.draft.take();
        } else {
            self.draft = None;
        }
        Ok(())
    }
    fn operation(
        &mut self,
        operation: impl FnOnce(&Table) -> Result<Vec<ByteEdit>, String>,
        requests: &mut Vec<HostRequest>,
    ) {
        if !self.ready() || self.draft.is_some() {
            return;
        }
        let result = operation(&self.view.as_ref().unwrap().table);
        if let Err(error) = self.queue_edits(result, requests) {
            self.message = Some(error);
        }
    }
    fn selected_records(&self) -> Vec<usize> {
        match (&self.view, self.state.selection) {
            (Some(view), Some(selection)) => selection
                .rows()
                .filter_map(|row| view.rows.get(row).copied())
                .collect(),
            _ => Vec::new(),
        }
    }
    fn copy(&self, ui: &Ui) {
        if let (Some(view), Some(selection)) = (&self.view, self.state.selection) {
            let text = view
                .table
                .copy_tsv(&self.selected_records(), selection.columns());
            if let Ok(text) = std::ffi::CString::new(text) {
                ui.binding().with_bound_context(|| unsafe {
                    dear_imgui_sys::igSetClipboardText(text.as_ptr());
                });
            }
        }
    }
    fn clear(&mut self, requests: &mut Vec<HostRequest>) {
        let Some(selection) = self.state.selection else {
            return;
        };
        let cells: Vec<_> = self
            .selected_records()
            .into_iter()
            .flat_map(|row| {
                selection
                    .columns()
                    .map(move |column| (row, column, String::new()))
            })
            .collect();
        self.operation(|table| table.replace_cells(&cells), requests);
    }
    fn paste(&mut self, ui: &Ui, requests: &mut Vec<HostRequest>) {
        if !self.ready() || self.draft.is_some() {
            return;
        }
        let text = ui.binding().with_bound_context(|| unsafe {
            let text = dear_imgui_sys::igGetClipboardText();
            (!text.is_null()).then(|| {
                std::ffi::CStr::from_ptr(text)
                    .to_string_lossy()
                    .into_owned()
            })
        });
        let Some(text) = text else {
            return;
        };
        let result = model::parse_clipboard(&text).and_then(|cells| {
            let view = self.view.as_ref().unwrap();
            let anchor = self.state.selection.map_or([0, 0], |selection| {
                [*selection.rows().start(), *selection.columns().start()]
            });
            let edits = paste_cells(view, anchor, &cells, self.transformed())?;
            view.table.replace_cells(&edits)
        });
        if let Err(error) = self.queue_edits(result, requests) {
            self.message = Some(error);
        }
    }
    fn insert_row(&mut self, after: bool, requests: &mut Vec<HostRequest>) {
        if self.transformed() {
            self.message = Some("Clear sorting and filters before inserting rows.".into());
            return;
        }
        let record = self
            .state
            .selection
            .and_then(|selection| self.view.as_ref()?.rows.get(selection.focus[0]).copied())
            .map(|record| record + usize::from(after))
            .unwrap_or_else(|| {
                self.view
                    .as_ref()
                    .map_or(0, |view| view.table.records.len())
            });
        self.operation(|table| table.insert_row(record), requests);
    }
    fn delete_rows(&mut self, requests: &mut Vec<HostRequest>) {
        let rows = self.selected_records();
        if !rows.is_empty() {
            self.operation(|table| table.delete_rows(&rows), requests);
        }
    }
    fn insert_column(&mut self, column: usize, requests: &mut Vec<HostRequest>) {
        let header = self.view.as_ref().is_some_and(|view| view.header);
        self.operation(|table| table.insert_column(column, header), requests);
        if self.awaiting_revision {
            self.state
                .widths
                .insert(column.min(self.state.widths.len()), 150.0);
            if let Some(sort) = &mut self.state.sort
                && sort.column >= column
            {
                sort.column += 1;
            }
            for filter in &mut self.state.filters {
                if filter.column >= column {
                    filter.column += 1;
                }
            }
        }
    }
    fn delete_columns(&mut self, requests: &mut Vec<HostRequest>) {
        let Some(selection) = self.state.selection else {
            return;
        };
        let columns: Vec<_> = selection.columns().collect();
        self.operation(|table| table.delete_columns(&columns), requests);
        if self.awaiting_revision {
            if let Some(sort) = &mut self.state.sort {
                if columns.contains(&sort.column) {
                    self.state.sort = None;
                } else {
                    sort.column -= columns
                        .iter()
                        .filter(|&&column| column < sort.column)
                        .count();
                }
            }
            self.state
                .filters
                .retain(|filter| !columns.contains(&filter.column));
            for filter in &mut self.state.filters {
                filter.column -= columns
                    .iter()
                    .filter(|&&column| column < filter.column)
                    .count();
            }
            for &column in columns.iter().rev() {
                if column < self.state.widths.len() {
                    self.state.widths.remove(column);
                }
            }
        }
    }
    fn next_cell(&mut self, backwards: bool) {
        let Some(view) = &self.view else {
            return;
        };
        let Some(selection) = self.state.selection else {
            return;
        };
        if view.rows.is_empty() {
            return;
        }
        let count = view.rows.len() * view.table.columns;
        let offset = selection.focus[0] * view.table.columns + selection.focus[1];
        let next = if backwards {
            offset.saturating_sub(1)
        } else {
            (offset + 1).min(count - 1)
        };
        self.state.selection = Some(Selection::single(
            next / view.table.columns,
            next % view.table.columns,
        ));
        self.reveal_selection = true;
    }
    fn find_next(&mut self, backwards: bool) {
        let Some(view) = &self.view else {
            return;
        };
        if self.find.is_empty() || view.rows.is_empty() {
            return;
        }
        let needle = self.find.to_lowercase();
        let count = view.rows.len() * view.table.columns;
        let start = self.state.selection.map_or(count - 1, |selection| {
            selection.focus[0] * view.table.columns + selection.focus[1]
        });
        for step in 1..=count {
            let position = if backwards {
                (start + count - step) % count
            } else {
                (start + step) % count
            };
            let row = position / view.table.columns;
            let column = position % view.table.columns;
            if view
                .table
                .cell(view.rows[row], column)
                .to_lowercase()
                .contains(&needle)
            {
                self.state.selection = Some(Selection::single(row, column));
                self.reveal_selection = true;
                self.message = None;
                return;
            }
        }
        self.message = Some("No matching visible cells.".into());
    }
    fn cycle_sort(&mut self, column: usize) {
        self.state.sort = match self.state.sort.take() {
            Some(sort) if sort.column == column && sort.descending => None,
            Some(mut sort) if sort.column == column => {
                sort.descending = true;
                Some(sort)
            }
            _ => Some(Sort {
                column,
                descending: false,
                mode: SortMode::Auto,
            }),
        };
        self.invalidate();
    }
    fn keyboard(&mut self, ui: &Ui, host: &HostContext<'_>, requests: &mut Vec<HostRequest>) {
        if !ui.is_window_focused_with_flags(FocusedFlags::ROOT_AND_CHILD_WINDOWS) {
            return;
        }
        let pressed = |key| ui.is_key_pressed_with_repeat(key, false);
        let primary = ui.io().key_ctrl() || ui.io().key_super();
        if self.draft.is_some() {
            if pressed(Key::Escape) {
                self.draft = None;
                self.message = None;
            }
            return;
        }
        if ui.is_any_item_active() {
            return;
        }
        if primary {
            if pressed(Key::A) {
                self.select_all();
            }
            if pressed(Key::F) {
                self.find_open = true;
                self.find_focus = true;
            }
            if pressed(Key::C) {
                self.copy(ui);
            }
            if pressed(Key::X) {
                self.copy(ui);
                self.clear(requests);
            }
            if pressed(Key::V) {
                self.paste(ui, requests);
            }
            if pressed(Key::Z) {
                requests.push(if ui.io().key_shift() {
                    HostRequest::Redo {
                        document: self.document,
                    }
                } else {
                    HostRequest::Undo {
                        document: self.document,
                    }
                });
            }
            if pressed(Key::Y) {
                requests.push(HostRequest::Redo {
                    document: self.document,
                });
            }
            return;
        }
        if pressed(Key::Delete) || pressed(Key::Backspace) {
            self.clear(requests);
        }
        if pressed(Key::Enter) || pressed(Key::F2) {
            if let Some(selection) = self.state.selection {
                self.begin_edit(selection.focus[0], selection.focus[1], None, false);
            }
        } else if pressed(Key::Tab) {
            self.next_cell(ui.io().key_shift());
        } else if let (Some(view), Some(selection)) = (&self.view, self.state.selection) {
            let mut point = selection.focus;
            if pressed(Key::UpArrow) {
                point[0] = point[0].saturating_sub(1);
            }
            if pressed(Key::DownArrow) {
                point[0] = (point[0] + 1).min(view.rows.len().saturating_sub(1));
            }
            if pressed(Key::LeftArrow) {
                point[1] = point[1].saturating_sub(1);
            }
            if pressed(Key::RightArrow) {
                point[1] = (point[1] + 1).min(view.table.columns - 1);
            }
            if point != selection.focus {
                self.select(point[0], point[1], ui.io().key_shift());
                self.reveal_selection = true;
            }
            // InputQueueCharacters preserves IME and non-US keyboard input. Reading is scoped to this Ui's context.
            let typed = ui.binding().with_bound_context(|| unsafe {
                let io = dear_imgui_sys::igGetIO_Nil();
                let queue = &(*io).InputQueueCharacters;
                if queue.Size <= 0 || queue.Data.is_null() {
                    return String::new();
                }
                std::slice::from_raw_parts(queue.Data, queue.Size as usize)
                    .iter()
                    .filter_map(|&code| char::from_u32(code))
                    .filter(|character| !character.is_control())
                    .collect::<String>()
            });
            if !typed.is_empty() {
                // Focus the new widget next frame; consume this frame's queue once so text is not inserted twice.
                ui.binding().with_bound_context(|| unsafe {
                    (*dear_imgui_sys::igGetIO_Nil()).InputQueueCharacters.Size = 0;
                });
                self.begin_edit(selection.focus[0], selection.focus[1], Some(typed), false);
            }
        }
        let _ = host;
    }
    fn toolbar(&mut self, ui: &Ui, host: &HostContext<'_>, requests: &mut Vec<HostRequest>) {
        if ui.button("Find") {
            self.find_open = !self.find_open;
            self.find_focus = self.find_open;
        }
        same_line_or_wrap(ui, 220.0);
        ui.set_next_item_width(ui.content_region_avail()[0].clamp(80.0, 220.0));
        if ui
            .input_text("##csv-filter", &mut self.state.global_filter)
            .hint("Filter all columns")
            .build()
        {
            self.invalidate();
        }
        same_line_or_wrap(ui, 145.0);
        if ui.button("Clear filters/sort") {
            self.state.global_filter.clear();
            self.state.filters.clear();
            self.state.sort = None;
            self.invalidate();
        }
        same_line_or_wrap(ui, 85.0);
        if ui.button("Format…") {
            ui.open_popup("csv-format");
        }
        if let Some(_popup) = ui.begin_popup("csv-format") {
            ui.text("Interpretation only");
            let mut delimiter = match self.state.delimiter {
                None => 0,
                Some(b',') => 1,
                Some(b'\t') => 2,
                Some(b';') => 3,
                Some(b'|') => 4,
                _ => 0,
            };
            if ui.combo_simple_string(
                "Delimiter",
                &mut delimiter,
                &["Detect automatically", "Comma", "Tab", "Semicolon", "Pipe"],
            ) && self.commit(host, requests).is_ok()
            {
                self.state.delimiter =
                    [None, Some(b','), Some(b'\t'), Some(b';'), Some(b'|')][delimiter];
                self.invalidate();
            }
            let mut header = self
                .state
                .header
                .map_or(0, |value| if value { 1 } else { 2 });
            if ui.combo_simple_string(
                "First row",
                &mut header,
                &[
                    "Detect header automatically",
                    "First row is header",
                    "First row is data",
                ],
            ) && self.commit(host, requests).is_ok()
            {
                self.state.header = [None, Some(true), Some(false)][header];
                self.invalidate();
            }
            if let Some(view) = &self.view {
                ui.text_disabled(format!(
                    "Detected: {} delimiter, {}",
                    delimiter_name(view.table.delimiter),
                    if view.table.detected_header {
                        "header row"
                    } else {
                        "no header row"
                    }
                ));
            }
        }
        same_line_or_wrap(ui, 90.0);
        if ui.button("Edit cell") {
            if let Some(draft) = &mut self.draft {
                draft.expanded = true;
                draft.focus = true;
            } else if let Some(selection) = self.state.selection {
                self.begin_edit(selection.focus[0], selection.focus[1], None, true);
            }
        }
        if self.find_open {
            if self.find_focus {
                ui.set_keyboard_focus_here();
                self.find_focus = false;
            }
            ui.set_next_item_width(ui.content_region_avail()[0].clamp(80.0, 270.0));
            let enter = ui
                .input_text("##csv-find", &mut self.find)
                .hint("Find in visible cells")
                .enter_returns_true(true)
                .build();
            same_line_or_wrap(ui, 85.0);
            if ui.button("Previous") {
                self.find_next(true);
            }
            same_line_or_wrap(ui, 55.0);
            if ui.button("Next") || enter {
                self.find_next(false);
            }
            same_line_or_wrap(ui, 60.0);
            if ui.button("Close##find") {
                self.find_open = false;
            }
        }
    }
    fn expanded_editor(
        &mut self,
        ui: &Ui,
        host: &HostContext<'_>,
        requests: &mut Vec<HostRequest>,
    ) {
        if !self
            .draft
            .as_ref()
            .is_some_and(|draft| draft.expanded || draft.conflicted)
        {
            return;
        }
        let draft = self.draft.as_mut().unwrap();
        ui.separator();
        ui.text(format!(
            "Edit source row {}, column {}",
            draft.record + 1,
            draft.column + 1
        ));
        if draft.focus {
            ui.set_keyboard_focus_here();
            draft.focus = false;
        }
        ui.input_text_multiline(
            "##csv-expanded",
            &mut draft.text,
            [ui.content_region_avail()[0], 110.0],
        )
        .build();
        let conflicted = draft.conflicted;
        let record = draft.record;
        let column = draft.column;
        if ui.button("Apply") {
            let _ = self.commit(host, requests);
        }
        ui.same_line();
        if ui.button("Cancel") {
            self.draft = None;
            self.message = None;
        }
        if conflicted {
            ui.same_line();
            if ui.button("Restart with this draft") && self.ready() {
                if self.view.as_ref().is_some_and(|view| {
                    record < view.table.records.len() && column < view.table.columns
                }) {
                    let text = self.draft.as_ref().map(|draft| draft.text.clone());
                    self.draft = None;
                    self.begin_record_edit(record, column, text, true);
                } else {
                    self.message = Some(
                        "This source cell no longer exists. Copy the draft into another cell."
                            .into(),
                    );
                }
            }
        } else {
            ui.same_line();
            ui.text_disabled("Newlines are kept inside this cell.");
        }
        ui.separator();
    }
    fn cell_menu(&mut self, ui: &Ui, host: &HostContext<'_>, requests: &mut Vec<HostRequest>) {
        if ui.menu_item("Edit cell…")
            && let Some(selection) = self.state.selection
        {
            self.begin_edit(selection.focus[0], selection.focus[1], None, true);
        }
        if ui.menu_item("Copy") {
            self.copy(ui);
        }
        if ui.menu_item("Cut") {
            self.copy(ui);
            self.clear(requests);
        }
        if ui.menu_item("Paste") {
            self.paste(ui, requests);
        }
        if ui.menu_item("Clear cells") {
            self.clear(requests);
        }
        let _ = host;
    }
    fn row_menu(&mut self, ui: &Ui, requests: &mut Vec<HostRequest>) {
        let insert_enabled = self.ready() && !self.transformed() && self.draft.is_none();
        if ui.menu_item_enabled_selected_no_shortcut("Insert row above", false, insert_enabled) {
            self.insert_row(false, requests);
        }
        if ui.menu_item_enabled_selected_no_shortcut("Insert row below", false, insert_enabled) {
            self.insert_row(true, requests);
        }
        if !insert_enabled && self.transformed() {
            ui.text_disabled("Clear filters/sort to insert rows");
        }
        if ui.menu_item("Delete selected rows") {
            self.delete_rows(requests);
        }
    }
    fn column_menu(&mut self, ui: &Ui, column: usize, requests: &mut Vec<HostRequest>) {
        let header = self.view.as_ref().is_some_and(|view| view.header);
        if header && ui.menu_item("Rename header…") {
            self.begin_record_edit(0, column, None, true);
        }
        if ui.menu_item("Select column")
            && let Some(view) = &self.view
            && !view.rows.is_empty()
        {
            self.state.selection = Some(Selection {
                anchor: [0, column],
                focus: [view.rows.len() - 1, column],
            });
        }
        if ui.menu_item("Insert column before") {
            self.insert_column(column, requests);
        }
        if ui.menu_item("Insert column after") {
            self.insert_column(column + 1, requests);
        }
        if ui.menu_item("Delete selected columns") {
            self.delete_columns(requests);
        }
        ui.separator();
        if let Some(sort) = &mut self.state.sort
            && sort.column == column
        {
            let mut mode = match sort.mode {
                SortMode::Auto => 0,
                SortMode::Text => 1,
                SortMode::Number => 2,
            };
            if ui.combo_simple_string("Sort as", &mut mode, &["Auto", "Text", "Number"]) {
                sort.mode = [SortMode::Auto, SortMode::Text, SortMode::Number][mode];
                self.invalidate();
            }
        } else if ui.menu_item("Sort ascending") {
            self.state.sort = Some(Sort {
                column,
                descending: false,
                mode: SortMode::Auto,
            });
            self.invalidate();
        }
        let existing = self
            .state
            .filters
            .iter()
            .position(|filter| filter.column == column);
        let mut filter = existing
            .map(|index| self.state.filters[index].clone())
            .unwrap_or(ColumnFilter {
                column,
                value: String::new(),
                op: FilterOp::Contains,
            });
        let mut op = match filter.op {
            FilterOp::Contains => 0,
            FilterOp::Equals => 1,
            FilterOp::Empty => 2,
        };
        let mut changed =
            ui.combo_simple_string("Filter", &mut op, &["Contains", "Equals", "Is empty"]);
        filter.op = [FilterOp::Contains, FilterOp::Equals, FilterOp::Empty][op];
        if op != 2 {
            changed |= ui.input_text("Value", &mut filter.value).build();
        }
        if changed || ui.button("Apply filter") {
            if let Some(index) = existing {
                self.state.filters[index] = filter;
            } else {
                self.state.filters.push(filter);
            }
            self.invalidate();
        }
        if existing.is_some() && ui.button("Remove filter") {
            self.state.filters.retain(|filter| filter.column != column);
            self.invalidate();
        }
    }
    fn grid(&mut self, ui: &Ui, host: &HostContext<'_>, requests: &mut Vec<HostRequest>) {
        let Some(view) = &self.view else {
            return;
        };
        let table = Arc::clone(&view.table);
        let rows = Arc::clone(&view.rows);
        let header = view.header;
        let columns = table.columns;
        let height = ui.content_region_avail()[1] - ui.text_line_height_with_spacing() * 1.6;
        let flags = TableFlags::RESIZABLE
            | TableFlags::SCROLL_X
            | TableFlags::SCROLL_Y
            | TableFlags::BORDERS_INNER
            | TableFlags::ROW_BG
            | TableFlags::NO_SAVED_SETTINGS;
        let Some(_table) = ui.begin_table_with_sizing(
            "csv-grid",
            columns + 1,
            flags,
            [0.0, height.max(60.0)],
            0.0,
        ) else {
            return;
        };
        ui.table_setup_column(
            "#",
            TableColumnFlags::NO_RESIZE,
            Some(TableColumnWidth::Fixed(65.0)),
        );
        for column in 0..columns {
            ui.table_setup_column(
                format!("##column-{column}"),
                TableColumnFlags::NONE,
                Some(TableColumnWidth::Fixed(
                    self.state.widths.get(column).copied().unwrap_or(150.0),
                )),
            );
        }
        ui.table_setup_scroll_freeze(1, 1);
        if self.restore_scroll {
            ui.set_scroll_x(self.state.scroll[0]);
            ui.set_scroll_y(self.state.scroll[1]);
            self.restore_scroll = false;
        }
        if self.reveal_selection
            && let Some(selection) = self.state.selection
        {
            ui.set_scroll_y(selection.focus[0] as f32 * (ui.frame_height() + 4.0));
        }
        ui.table_next_row_with_flags(TableRowFlags::HEADERS, 0.0);
        ui.table_next_column();
        if ui.selectable_config("#").selected(false).build() {
            self.select_all();
        }
        for column in 0..columns {
            if !ui.table_next_column() {
                continue;
            }
            let name = if header {
                table.cell(0, column).into_owned()
            } else {
                format!("Column {}", column + 1)
            };
            let suffix = self
                .state
                .sort
                .as_ref()
                .filter(|sort| sort.column == column)
                .map_or("", |sort| if sort.descending { " ▼" } else { " ▲" });
            let filtered = if self
                .state
                .filters
                .iter()
                .any(|filter| filter.column == column)
            {
                " •"
            } else {
                ""
            };
            let _id = ui.push_id(column);
            if ui
                .selectable_config(format!(
                    "{}{}{}##header",
                    display_cell(&name),
                    suffix,
                    filtered
                ))
                .build()
                && self.draft.is_none()
            {
                if !rows.is_empty() {
                    self.state.selection = Some(Selection {
                        anchor: [0, column],
                        focus: [rows.len() - 1, column],
                    });
                }
                self.cycle_sort(column);
            }
            if ui.is_item_hovered() {
                ui.tooltip_text(format!(
                    "{}\nClick to sort; right-click for column operations",
                    name
                ));
            }
            if let Some(_popup) = ui.begin_popup_context_item_with_label(Some("column-menu")) {
                if !self
                    .state
                    .selection
                    .is_some_and(|selection| selection.columns().contains(&column))
                    && !rows.is_empty()
                {
                    self.state.selection = Some(Selection {
                        anchor: [0, column],
                        focus: [rows.len() - 1, column],
                    });
                }
                self.column_menu(ui, column, requests);
            }
        }
        // Input widgets and selectables share a fixed row height, including multiline source fields.
        let row_height = ui.frame_height() + 4.0;
        let _spacing = ui.push_style_var(StyleVar::ItemSpacing([0.0, 0.0]));
        for index in ListClipper::new(rows.len())
            .items_height(row_height)
            .begin(ui)
            .iter()
        {
            let row = index;
            let record = rows[row];
            let _row_id = ui.push_id(record);
            ui.table_next_row_with_flags(TableRowFlags::NONE, row_height);
            ui.table_next_column();
            let selected = self.state.selection.is_some_and(|selection| {
                selection.rows().contains(&row)
                    && *selection.columns().start() == 0
                    && *selection.columns().end() == columns - 1
            });
            if ui
                .selectable_config((record + 1).to_string())
                .selected(selected)
                .size([0.0, row_height - 4.0])
                .build()
                && self.draft.is_none()
            {
                let anchor = if ui.io().key_shift() {
                    self.state
                        .selection
                        .map_or(row, |selection| selection.anchor[0])
                } else {
                    row
                };
                self.state.selection = Some(Selection {
                    anchor: [anchor, 0],
                    focus: [row, columns - 1],
                });
            }
            if let Some(_popup) = ui.begin_popup_context_item_with_label(Some("row-menu")) {
                if !self
                    .state
                    .selection
                    .is_some_and(|selection| selection.rows().contains(&row))
                {
                    self.state.selection = Some(Selection {
                        anchor: [row, 0],
                        focus: [row, columns - 1],
                    });
                }
                self.row_menu(ui, requests);
            }
            for column in 0..columns {
                if !ui.table_next_column() {
                    continue;
                }
                let _cell_id = ui.push_id(column as i32);
                let editing = self.draft.as_ref().is_some_and(|draft| {
                    draft.record == record
                        && draft.column == column
                        && !draft.expanded
                        && !draft.conflicted
                });
                if editing {
                    let draft = self.draft.as_mut().unwrap();
                    if draft.focus {
                        ui.set_keyboard_focus_here();
                        draft.focus = false;
                    }
                    ui.set_next_item_width(-1.0);
                    let submitted = ui
                        .input_text("##edit", &mut draft.text)
                        .flags(InputTextFlags::ENTER_RETURNS_TRUE)
                        // Own Tab so ImGui does not activate another toolbar input.
                        .callback_flags(InputTextCallback::COMPLETION)
                        .build();
                    let active = ui.is_item_active();
                    let enter = submitted
                        || active
                            && (ui.is_key_pressed_with_repeat(Key::Enter, false)
                                || ui.is_key_pressed_with_repeat(Key::KeypadEnter, false));
                    let blur = ui.is_item_deactivated_after_edit();
                    let tab = active && ui.is_key_pressed_with_repeat(Key::Tab, false);
                    if active && ui.is_key_pressed_with_repeat(Key::Escape, false) {
                        self.draft = None;
                    } else if (enter || tab || blur) && self.commit(host, requests).is_ok() {
                        ui.binding().with_bound_context(|| unsafe {
                            dear_imgui_sys::igClearActiveID();
                        });
                        if tab {
                            self.next_cell(ui.io().key_shift());
                        }
                    }
                } else {
                    let cell = table.cell(record, column);
                    let label = format!("{}##cell", display_cell(&cell));
                    let selected = self
                        .state
                        .selection
                        .is_some_and(|selection| selection.contains(row, column));
                    let clicked = ui
                        .selectable_config(label)
                        .selected(selected)
                        .allow_double_click(true)
                        .size([0.0, row_height - 4.0])
                        .build();
                    let hovered = ui.is_item_hovered();
                    if clicked {
                        if self.draft.is_some() {
                            let _ = self.commit(host, requests);
                        }
                        if self.draft.is_none() {
                            self.select(row, column, ui.io().key_shift());
                            self.drag_selecting = true;
                            if ui.is_mouse_double_clicked(MouseButton::Left) {
                                self.begin_edit(row, column, None, false);
                            }
                        }
                    } else if hovered
                        && self.drag_selecting
                        && ui.is_mouse_down(MouseButton::Left)
                        && self.draft.is_none()
                    {
                        self.select(row, column, true);
                    }
                    if hovered && (cell.len() > 50 || cell.contains(['\r', '\n'])) {
                        ui.tooltip_text(cell.as_ref());
                    }
                    if let Some(_popup) = ui.begin_popup_context_item_with_label(Some("cell-menu"))
                    {
                        if !selected {
                            self.select(row, column, false);
                        }
                        self.cell_menu(ui, host, requests);
                        ui.separator();
                        self.row_menu(ui, requests);
                    }
                }
                if self.reveal_selection
                    && self
                        .state
                        .selection
                        .is_some_and(|selection| selection.focus == [row, column])
                {
                    ui.set_scroll_here_x(0.5);
                    ui.set_scroll_here_y(0.5);
                    self.reveal_selection = false;
                }
            }
        }
        self.state.scroll = [ui.scroll_x(), ui.scroll_y()];
        // The safe wrapper exposes initial widths only. Capture the current widths for workspace restoration.
        ui.binding().with_bound_context(|| unsafe {
            let table = dear_imgui_sys::igGetCurrentTable();
            if !self.awaiting_revision && !table.is_null() && !(*table).Columns.Data.is_null() {
                for column in 0..columns {
                    let width = (*(*table).Columns.Data.add(column + 1)).WidthGiven;
                    if width.is_finite()
                        && width >= 40.0
                        && let Some(saved) = self.state.widths.get_mut(column)
                    {
                        *saved = width;
                    }
                }
            }
        });
    }
}

impl PluginPanel for CsvPanel {
    fn title(&self, host: &HostContext<'_>) -> String {
        host.document(self.document).map_or_else(
            || "CSV Table".into(),
            |document| {
                document
                    .path
                    .rsplit(['/', '\\'])
                    .next()
                    .unwrap_or("CSV Table")
                    .to_owned()
            },
        )
    }
    fn attached_document(&self) -> Option<DocumentId> {
        Some(self.document)
    }
    fn save_state(&self) -> Value {
        serde_json::to_value(&self.state).unwrap_or(Value::Null)
    }
    fn edit_result(&mut self, token: EditToken, result: Result<Revision, String>) {
        if self.pending_token != Some(token) {
            return;
        }
        self.pending_token = None;
        match result {
            Ok(revision) => {
                self.pending_draft = None;
                self.pending_state = None;
                self.awaiting_revision = self.loaded_revision != Some(revision);
            }
            Err(error) => {
                self.awaiting_revision = false;
                if let Some(state) = self.pending_state.take() {
                    self.state = state;
                }
                if let Some(mut draft) = self.pending_draft.take() {
                    draft.conflicted = true;
                    draft.expanded = true;
                    self.draft = Some(draft);
                }
                self.message = Some(format!("The edit could not be applied: {error}"));
            }
        }
    }
    fn action(
        &mut self,
        action: PanelAction,
        host: &HostContext<'_>,
        requests: &mut Vec<HostRequest>,
    ) -> Result<bool, String> {
        match action {
            PanelAction::Find => {
                self.find_open = true;
                self.find_focus = true;
            }
            PanelAction::SelectAll => {
                self.sync(host);
                self.select_all();
            }
            PanelAction::CommitEdit => self.commit(host, requests)?,
        }
        Ok(true)
    }
    fn draw(&mut self, ui: &Ui, host: &HostContext<'_>, requests: &mut Vec<HostRequest>) {
        self.sync(host);
        self.keyboard(ui, host, requests);
        self.toolbar(ui, host, requests);
        self.expanded_editor(ui, host, requests);
        if let Some(error) = &self.parse_error {
            ui.text_wrapped(error);
            if ui.button("Open in Text Editor")
                && let Some(document) = host.document(self.document)
            {
                requests.push(HostRequest::OpenFile {
                    path: document.path.clone(),
                    viewer: Some("bed.text".into()),
                });
            }
        } else {
            self.grid(ui, host, requests);
        }
        if !ui.is_mouse_down(MouseButton::Left) {
            self.drag_selecting = false;
        }
        if let Some(view) = &self.view {
            let total = view
                .table
                .records
                .len()
                .saturating_sub(usize::from(view.header));
            let selection = self.state.selection.map_or(String::new(), |selection| {
                format!(
                    "  ·  {} × {} selected",
                    selection.rows().count(),
                    selection.columns().count()
                )
            });
            ui.text_disabled(format!(
                "{} / {} rows  ·  {} columns{}{}",
                view.rows.len(),
                total,
                view.table.columns,
                selection,
                if self.updating || self.awaiting_revision {
                    "  ·  Updating…"
                } else {
                    ""
                }
            ));
            if view.rows.is_empty() && !self.transformed() && ui.button("Add row") {
                self.insert_row(false, requests);
            }
        } else if self.updating {
            ui.text_disabled("Parsing CSV…");
        }
        if let Some(message) = &self.message {
            ui.text_wrapped(message);
        }
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

fn delimiter_name(delimiter: u8) -> &'static str {
    match delimiter {
        b'\t' => "tab",
        b';' => "semicolon",
        b'|' => "pipe",
        _ => "comma",
    }
}
fn same_line_or_wrap(ui: &Ui, width: f32) {
    ui.same_line();
    if ui.content_region_avail()[0] < width {
        ui.new_line();
    }
}
fn display_cell(value: &str) -> String {
    let mut display = String::new();
    for character in value.chars().take(160) {
        match character {
            '\n' => display.push_str(" ↵ "),
            '\r' => {}
            '\t' => display.push_str(" ⇥ "),
            '\0' => display.push('�'),
            _ => display.push(character),
        }
    }
    if value.chars().nth(160).is_some() {
        display.push('…');
    }
    // ImGui's double hash suffix is an ID delimiter; the visible text must remain literal.
    display.replace("##", "#\u{200b}#")
}
fn paste_cells(
    view: &View,
    anchor: [usize; 2],
    cells: &[Vec<String>],
    transformed: bool,
) -> Result<Vec<(usize, usize, String)>, String> {
    let width = cells.iter().map(Vec::len).max().unwrap_or(0);
    if transformed
        && (anchor[0].saturating_add(cells.len()) > view.rows.len()
            || anchor[1].saturating_add(width) > view.table.columns)
    {
        return Err("The entire paste was rejected because it exceeds the visible table. Clear filters and sorting to append rows or columns.".into());
    }
    let mut replacements = Vec::new();
    for (offset, values) in cells.iter().enumerate() {
        let row = anchor[0] + offset;
        let record = view
            .rows
            .get(row)
            .copied()
            .unwrap_or(view.table.records.len() + row.saturating_sub(view.rows.len()));
        for (offset, value) in values.iter().enumerate() {
            replacements.push((record, anchor[1] + offset, value.clone()));
        }
    }
    Ok(replacements)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ready_panel() -> CsvPanel {
        let table = Arc::new(
            model::parse(
                Arc::from(&b"name,value\nfirst,001\n"[..]),
                Some(b','),
                "data.csv",
                || false,
            )
            .unwrap(),
        );
        let mut panel = CsvPanel::new(
            DocumentId(1),
            &serde_json::json!({"header":true,"widths":[150.0,150.0]}),
        );
        panel.view = Some(View {
            table,
            rows: vec![1].into(),
            header: true,
        });
        panel.revision = Some((0, 0));
        panel.loaded_revision = Some((0, 0));
        panel.updating = false;
        panel.state.selection = Some(Selection::single(0, 0));
        panel
    }
    #[test]
    fn rejected_commit_preserves_the_exact_draft_and_clears_pending_state() {
        let mut panel = ready_panel();
        panel.begin_record_edit(1, 0, Some("local 🦉\ntext".into()), true);
        let documents = [bed_plugin::PluginDocument {
            id: DocumentId(1),
            path: "data.csv".into(),
            kind: bed_plugin::DocumentKind::Text,
            language_id: "csv".into(),
            revision: (0, 0),
            dirty: false,
            bytes: Arc::from(&b"name,value\nfirst,001\n"[..]),
            text: None,
        }];
        let host = HostContext {
            documents: &documents,
            active_document: Some(DocumentId(1)),
            settings: &Value::Null,
            textures: &std::collections::HashMap::new(),
            animations: false,
            workspace: 1,
            diagnostics: &Value::Null,
        };
        let mut requests = Vec::new();
        panel.commit(&host, &mut requests).unwrap();
        let token = panel.pending_token.unwrap();
        assert!(panel.draft.is_none());
        assert_eq!(panel.pending_draft.as_ref().unwrap().text, "local 🦉\ntext");
        assert!(panel.commit(&host, &mut requests).is_err());
        panel.edit_result(token, Err("revision changed".into()));
        let restored = panel.draft.as_ref().unwrap();
        assert_eq!(restored.text, "local 🦉\ntext");
        assert!(restored.expanded && restored.conflicted);
        assert!(panel.pending_token.is_none());
        assert!(!panel.awaiting_revision);
    }
    #[test]
    fn rejected_structural_edit_restores_column_preferences_and_readiness() {
        let mut panel = ready_panel();
        panel.state.selection = Some(Selection::single(0, 1));
        let mut requests = Vec::new();
        panel.delete_columns(&mut requests);
        assert_eq!(panel.state.widths.len(), 1);
        let token = panel.pending_token.unwrap();
        panel.edit_result(token, Err("size limit".into()));
        assert_eq!(panel.state.widths.len(), 2);
        assert!(panel.ready());
        panel.operation(|table| table.insert_row(2), &mut requests);
        let token = panel.pending_token.unwrap();
        panel.edit_result(token, Ok((0, 0)));
        assert!(
            panel.ready(),
            "same-revision acknowledgements must not leave edits waiting forever"
        );
    }
    #[test]
    fn paste_maps_visible_rows_and_rejects_overflow_atomically() {
        let table = Arc::new(
            model::parse(
                Arc::from(&b"id,value\n1,a\n2,b\n3,c\n"[..]),
                Some(b','),
                "data.csv",
                || false,
            )
            .unwrap(),
        );
        let view = View {
            table,
            rows: vec![3, 1].into(),
            header: true,
        };
        assert_eq!(
            paste_cells(&view, [0, 1], &[vec!["x".into()], vec!["y".into()]], true).unwrap(),
            vec![(3, 1, "x".into()), (1, 1, "y".into())]
        );
        assert!(paste_cells(&view, [1, 1], &[vec!["x".into()], vec!["y".into()]], true).is_err());
        assert!(paste_cells(&view, [0, 1], &[vec!["x".into(), "y".into()]], true).is_err());
    }
    #[test]
    fn plain_paste_can_append_records_and_columns() {
        let table = Arc::new(
            model::parse(Arc::from(&b"a\n"[..]), Some(b','), "data.csv", || false).unwrap(),
        );
        let view = View {
            table,
            rows: vec![0].into(),
            header: false,
        };
        let replacements = paste_cells(
            &view,
            [0, 0],
            &[vec!["x".into(), "y".into()], vec!["z".into()]],
            false,
        )
        .unwrap();
        assert_eq!(
            replacements,
            vec![(0, 0, "x".into()), (0, 1, "y".into()), (1, 0, "z".into())]
        );
    }
    #[test]
    fn saved_view_state_round_trips_and_clamps_untrusted_widths() {
        let state = serde_json::json!({ "delimiter": 9, "header": true, "widths": [-10.0, 4500.0], "scroll": [-3.0, 21.0], "global_filter": "abc", "selection": {"anchor":[0,1],"focus":[4,2]} });
        let panel = CsvPanel::new(DocumentId(1), &state);
        let saved = panel.save_state();
        assert_eq!(saved["widths"], serde_json::json!([40.0, 2000.0]));
        assert_eq!(saved["scroll"], serde_json::json!([0.0, 21.0]));
        assert_eq!(saved["selection"], state["selection"]);
    }
}
