//! Completion owns one caret session. Server ranges are decoded once against
//! the request snapshot and transformed only through observed prefix typing.
use crate::presentation::LspPresentationOptions;
use bedtime_documents::{ByteEdit, DocumentId, DocumentSession, ViewId};
use bedtime_editing::text_commands::CursorReveal;
use bedtime_editor_widgets::views::hover_tooltip::{
    hover_popup_style, popup_placement, render_hover_markdown, render_hover_plaintext,
};
use bedtime_editor_widgets::{EditorView, ViewResponse};
use bedtime_highlight::capture_map::{THEME_SLOTS, ThemeSlot};
use bedtime_lsp::{
    jsonrpc::RpcId, lsp_client::LspClient, lsp_request::LspRequestState, lsp_uri::LspUri,
    workspace_lsp::LspRequestOrigin,
};
use dear_imgui_rs::{Condition, FocusedFlags, MouseButton, StyleColor, Ui, WindowFlags};
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    io,
    ops::Range,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

#[derive(Clone, Debug)]
struct Item {
    label: String,
    detail: String,
    filter: String,
    sort: String,
    kind: u32,
    documentation: String,
    markdown: bool,
    edit: ByteEdit,
    additional: Vec<ByteEdit>,
    raw: Value,
    resolved: bool,
    resolve_error: Option<String>,
}

#[derive(Clone, Default, PartialEq)]
struct Details {
    label: String,
    detail: String,
    documentation: String,
    markdown: bool,
    resolving: bool,
}

#[derive(Clone, Debug, Default)]
struct List {
    items: Vec<Item>,
    incomplete: bool,
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "Invalid completion response")
}

fn identifier(c: char) -> bool {
    c == '_' || c.is_alphanumeric()
}

fn prefix_start(bytes: &[u8], caret: usize) -> usize {
    let Ok(prefix) = std::str::from_utf8(&bytes[..caret]) else {
        return caret;
    };
    prefix
        .char_indices()
        .rev()
        .find(|(_, c)| !identifier(*c))
        .map_or(0, |(i, c)| i + c.len_utf8())
}

fn position_offset(bytes: &[u8], position: &Value) -> io::Result<usize> {
    let line = position["line"]
        .as_u64()
        .and_then(|n| usize::try_from(n).ok())
        .ok_or_else(invalid)?;
    let character = position["character"]
        .as_u64()
        .and_then(|n| usize::try_from(n).ok())
        .ok_or_else(invalid)?;
    bedtime_lsp::workspace_edit::position_offset(
        bytes,
        bedtime_lsp::workspace_edit::Position { line, character },
    )
}

fn decode_edit(value: &Value, bytes: &[u8]) -> io::Result<ByteEdit> {
    let range = &value["range"];
    let start = position_offset(bytes, &range["start"])?;
    let end = position_offset(bytes, &range["end"])?;
    if end < start {
        return Err(invalid());
    }
    Ok(ByteEdit {
        range: start..end,
        bytes: value["newText"]
            .as_str()
            .ok_or_else(invalid)?
            .as_bytes()
            .to_vec(),
    })
}

fn parse_item(
    mut raw: Value,
    defaults: Option<&Value>,
    bytes: &[u8],
    caret: usize,
    resolved: bool,
) -> io::Result<Option<Item>> {
    // Do not advertise snippets, and reject a server that sends them anyway.
    let format = raw
        .get("insertTextFormat")
        .or_else(|| defaults.and_then(|d| d.get("insertTextFormat")));
    if format.and_then(Value::as_u64) == Some(2) {
        return Ok(None);
    }
    let label = raw["label"].as_str().ok_or_else(invalid)?.to_owned();
    let filter = raw["filterText"].as_str().unwrap_or(&label).to_owned();
    let sort = raw["sortText"].as_str().unwrap_or(&label).to_owned();
    let label_details = raw.get("labelDetails");
    let detail = raw["detail"]
        .as_str()
        .or_else(|| label_details.and_then(|v| v["description"].as_str()))
        .unwrap_or("")
        .to_owned();
    let text = raw["textEditText"]
        .as_str()
        .or_else(|| raw["insertText"].as_str())
        .unwrap_or(&label)
        .to_owned();
    // Defaults are protocol data, not presentation metadata: resolution must
    // retain the same insertion range as the original list.
    if raw.get("textEdit").is_none()
        && let Some(range) = defaults.and_then(|d| d.get("editRange"))
    {
        raw["textEdit"] = if range.get("insert").is_some() {
            json!({"insert":range["insert"],"replace":range["replace"],"newText":text})
        } else {
            json!({"range":range,"newText":text})
        };
    }
    if raw.get("insertTextFormat").is_none()
        && let Some(format) = defaults.and_then(|d| d.get("insertTextFormat"))
    {
        raw["insertTextFormat"] = format.clone();
    }
    let edit = if let Some(edit) = raw.get("textEdit") {
        if edit.get("range").is_some() {
            decode_edit(edit, bytes)?
        } else {
            decode_edit(
                &json!({"range":edit["replace"],"newText":edit["newText"]}),
                bytes,
            )?
        }
    } else if let Some(range) = defaults.and_then(|d| d.get("editRange")) {
        let range = range.get("replace").unwrap_or(range);
        decode_edit(&json!({"range":range,"newText":text}), bytes)?
    } else {
        ByteEdit {
            range: prefix_start(bytes, caret)..caret,
            bytes: text.as_bytes().to_vec(),
        }
    };
    if edit.range.start > caret || edit.range.end < caret {
        return Err(invalid());
    }
    let additional = raw
        .get("additionalTextEdits")
        .and_then(Value::as_array)
        .map(|edits| {
            edits
                .iter()
                .map(|edit| decode_edit(edit, bytes))
                .collect::<io::Result<Vec<_>>>()
        })
        .transpose()?
        .unwrap_or_default();
    let doc = &raw["documentation"];
    let documentation = doc
        .as_str()
        .or_else(|| doc["value"].as_str())
        .unwrap_or("")
        .to_owned();
    let markdown = doc["kind"].as_str() == Some("markdown");
    if raw.get("data").is_none()
        && let Some(data) = defaults.and_then(|d| d.get("data"))
    {
        raw["data"] = data.clone();
    }
    Ok(Some(Item {
        label,
        detail,
        filter,
        sort,
        kind: raw["kind"].as_u64().unwrap_or(0) as u32,
        documentation,
        markdown,
        edit,
        additional,
        raw,
        resolved,
        resolve_error: None,
    }))
}

fn parse_list(value: Value, bytes: &[u8], caret: usize) -> io::Result<List> {
    if value.is_null() {
        return Ok(List::default());
    }
    let incomplete = value["isIncomplete"].as_bool().unwrap_or(false);
    let defaults = value.get("itemDefaults");
    let items = value
        .as_array()
        .or_else(|| value["items"].as_array())
        .ok_or_else(invalid)?;
    let mut parsed = Vec::new();
    for item in items {
        // An unsupported or malformed individual item must not hide good results.
        if let Ok(Some(item)) = parse_item(item.clone(), defaults, bytes, caret, false) {
            parsed.push(item);
        }
    }
    parsed.sort_by(|a, b| a.sort.cmp(&b.sort));
    Ok(List {
        items: parsed,
        incomplete,
    })
}

fn matches(filter: &str, prefix: &str) -> bool {
    let mut remaining = filter.chars().flat_map(char::to_lowercase);
    prefix
        .chars()
        .flat_map(char::to_lowercase)
        .all(|c| remaining.by_ref().any(|candidate| c == candidate))
}

fn transform_range(range: &mut Range<usize>, change: Range<usize>, length: usize) {
    let transform = |offset: usize, end: bool| {
        if offset < change.start || offset == change.start && !end {
            offset
        } else if offset >= change.end {
            offset - change.len() + length
        } else {
            change.start + if end { length } else { 0 }
        }
    };
    *range = transform(range.start, false)..transform(range.end, true);
}

fn transform_item(item: &mut Item, changes: &[(Range<usize>, usize)]) {
    for (change, length) in changes {
        transform_range(&mut item.edit.range, change.clone(), *length);
        for edit in &mut item.additional {
            transform_range(&mut edit.range, change.clone(), *length);
        }
    }
}

struct Active {
    view: ViewId,
    document: DocumentId,
    origin: LspRequestOrigin,
    revision: (u64, u64),
    bytes: Vec<u8>,
    baseline: Vec<u8>,
    baseline_caret: usize,
    caret: usize,
    prefix_start: usize,
    changes: Vec<(Range<usize>, usize)>,
    items: Vec<Item>,
    filtered: Vec<usize>,
    selected: usize,
    incomplete: bool,
    client: Rc<RefCell<LspClient>>,
    rpc: Option<RpcId>,
    resolve_rpc: Option<RpcId>,
    request: Arc<LspRequestState<Result<List, String>>>,
    resolve: Arc<LspRequestState<Result<Option<Item>, String>>>,
    resolving: Option<usize>,
    accept_pending: bool,
    received: bool,
    // A refresh has a newer request snapshot while displayed items still use
    // baseline and changes. Switch baselines only when its reply arrives.
    refresh_snapshot: Option<(Vec<u8>, usize, usize)>,
    popup_rect: Option<([f32; 2], [f32; 2])>,
    docs_rect: Option<([f32; 2], [f32; 2])>,
    scroll_selection: bool,
    docs_item: Option<usize>,
    selection_motion: bedtime_ui::util::transition::Motion,
    docs_transition: bedtime_ui::util::transition::ContentTransition<Details>,
}

impl Active {
    fn filter(&mut self) {
        let previous = self.filtered.get(self.selected).copied();
        let prefix = String::from_utf8_lossy(&self.bytes[self.prefix_start..self.caret]);
        self.filtered = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| matches(&item.filter, &prefix))
            .map(|(i, _)| i)
            .collect();
        self.selected = previous
            .and_then(|previous| self.filtered.iter().position(|i| *i == previous))
            .unwrap_or(0);
    }

    fn current(&self, session: &DocumentSession) -> bool {
        session.document_for_view(self.view) == Some(self.document)
            && session.document_revision(self.document).ok() == Some(self.revision)
            && session
                .lsp()
                .and_then(|pool| pool.request_origin(self.document, self.view, self.revision.0, 0))
                .is_some_and(|origin| {
                    origin.session_id == self.origin.session_id
                        && origin.server_generation == self.origin.server_generation
                })
    }

    fn cancel(&mut self) {
        self.request.cancel();
        self.resolve.cancel();
        for id in [self.rpc.take(), self.resolve_rpc.take()]
            .into_iter()
            .flatten()
        {
            let _ = self.client.borrow_mut().cancel_request(&id);
        }
    }

    fn receive(&mut self) {
        if !self.received && !self.request.is_pending() {
            self.received = true;
            self.rpc = None;
            if let Some((bytes, caret, changes_start)) = self.refresh_snapshot.take() {
                self.resolve.cancel();
                if let Some(id) = self.resolve_rpc.take() {
                    let _ = self.client.borrow_mut().cancel_request(&id);
                }
                self.resolving = None;
                self.baseline = bytes;
                self.baseline_caret = caret;
                self.changes.drain(..changes_start);
                self.docs_item = None;
            }
            let selected_label = self
                .filtered
                .get(self.selected)
                .map(|&index| self.items[index].label.clone());
            self.items.clear();
            self.filtered.clear();
            self.selected = 0;
            if let Some(Ok(list)) = self.request.snapshot() {
                self.items = list.items;
                self.incomplete = list.incomplete;
                for item in &mut self.items {
                    transform_item(item, &self.changes);
                }
                self.filter();
                if let Some(row) = self
                    .filtered
                    .iter()
                    .position(|&index| Some(&self.items[index].label) == selected_label.as_ref())
                {
                    self.selected = row;
                }
                self.scroll_selection = true;
            }
        }
        if self.resolving.is_some() && !self.resolve.is_pending() {
            self.resolve_rpc = None;
            let index = self.resolving.take().unwrap();
            match self.resolve.snapshot() {
                Some(Ok(Some(mut item))) => {
                    transform_item(&mut item, &self.changes);
                    self.items[index] = item;
                }
                result => {
                    self.items[index].resolved = true;
                    self.items[index].resolve_error = Some(match result {
                        Some(Err(error)) => error,
                        _ => "Completion resolution failed".into(),
                    });
                }
            }
        }
    }

    fn resolve_selected(&mut self) {
        let Some(index) = self.filtered.get(self.selected).copied() else {
            return;
        };
        if self.items[index].resolved {
            return;
        }
        let supported =
            self.client.borrow().server_capabilities()["completionProvider"]["resolveProvider"]
                .as_bool()
                == Some(true);
        if !supported {
            self.items[index].resolved = true;
            return;
        }
        if self.resolving == Some(index) {
            return;
        }
        self.resolve.cancel();
        if let Some(id) = self.resolve_rpc.take() {
            let _ = self.client.borrow_mut().cancel_request(&id);
        }
        let ticket = self.resolve.begin();
        let state = self.resolve.clone();
        let bytes = self.baseline.clone();
        let caret = self.baseline_caret;
        let original = self.items[index].raw.clone();
        self.resolving = Some(index);
        let result = self.client.borrow_mut().send_request(
            "completionItem/resolve",
            original.clone(),
            move |result| {
                let item = result
                    .map_err(|error| io::Error::other(error.message))
                    .and_then(|resolved| {
                        // Resolve may return only newly supplied fields; retain original values.
                        let mut merged = original;
                        if let (Some(target), Some(source)) =
                            (merged.as_object_mut(), resolved.as_object())
                        {
                            target.extend(source.clone());
                        }
                        parse_item(merged, None, &bytes, caret, true)
                    });
                state.deliver(ticket, Some(item.map_err(|error| error.to_string())));
            },
        );
        match result {
            Ok(id) => self.resolve_rpc = Some(id),
            Err(error) => {
                self.items[index].resolved = true;
                self.items[index].resolve_error = Some(error.to_string());
                self.resolving = None;
                self.resolve.cancel();
            }
        }
    }
}
// io::Error is not Clone, so request results use strings at the async boundary.

type ScheduledCompletion = (ViewId, Instant, (u64, u64), (i32, i32));

#[derive(Default)]
pub(crate) struct Completion {
    active: Option<Active>,
    scheduled: Option<ScheduledCompletion>,
    commands: Vec<(ViewId, Value)>,
    error: Option<String>,
}

impl Completion {
    pub fn visible_for(&self, view: ViewId) -> bool {
        self.active
            .as_ref()
            .is_some_and(|active| active.view == view)
    }

    pub fn take_error(&mut self) -> Option<String> {
        self.error.take()
    }

    pub fn take_commands(&mut self) -> Vec<(ViewId, Value)> {
        std::mem::take(&mut self.commands)
    }

    pub fn cancel(&mut self) {
        if let Some(mut active) = self.active.take() {
            active.cancel();
        }
        self.scheduled = None;
    }

    pub fn forget_view(&mut self, view: ViewId) {
        if self.visible_for(view) || self.scheduled.is_some_and(|(v, _, _, _)| v == view) {
            self.cancel();
        }
    }

    pub fn retain(&mut self, session: &DocumentSession) {
        if self
            .active
            .as_ref()
            .is_some_and(|active| !active.current(session))
        {
            self.cancel();
        }
    }

    pub fn request(
        &mut self,
        session: &mut DocumentSession,
        view: ViewId,
        trigger: Option<String>,
        incomplete: bool,
    ) -> io::Result<()> {
        let refreshing = incomplete
            && self
                .active
                .as_ref()
                .is_some_and(|active| active.view == view && active.current(session));
        if !refreshing {
            self.cancel();
        }
        let document = session
            .document_for_view(view)
            .ok_or_else(|| io::Error::other("Completion view is detached"))?;
        let snapshot = session.snapshot(document)?;
        let position = session.view_snapshot(view)?;
        let allowed = position.selections.len() == 1
            && position.selections[0].empty()
            && !session.with_document(document, |state| state.read_only)?;
        if !allowed {
            self.cancel();
            return Ok(());
        }
        let client = session
            .lsp()
            .and_then(|pool| pool.client_for_document(document))
            .filter(|client| client.borrow().is_initialized())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::Unsupported,
                    "Completion requires an initialized language server",
                )
            })?;
        if !client.borrow().supports_method("textDocument/completion") {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Language server does not support completion",
            ));
        }
        let caret = session.with_document(document, |state| {
            state.offset_from_row_col(position.row, position.column)
        })?;
        let revision = session.document_revision(document)?;
        let origin = session
            .lsp()
            .and_then(|pool| pool.request_origin(document, view, revision.0, 0))
            .ok_or_else(|| io::Error::other("Completion origin unavailable"))?;
        let uri = (if session
            .lsp()
            .is_some_and(|pool| pool.ssh_target().is_some())
        {
            LspUri::file_uri_from_remote_path(&snapshot.path)
        } else {
            LspUri::file_uri_from_path(&snapshot.path)
        })?
        .to_string();
        let line = session.with_document(document, |state| state.line(position.row))?;
        let utf16 = bedtime_editing::util::utf8::utf8_byte_offset_to_utf16(&line, position.column);
        let context = if incomplete {
            json!({"triggerKind":3})
        } else if let Some(character) = trigger {
            json!({"triggerKind":2,"triggerCharacter":character})
        } else {
            json!({"triggerKind":1})
        };
        let request = Arc::new(LspRequestState::new());
        let ticket = request.begin_for(origin);
        let state = request.clone();
        let prefix = prefix_start(&snapshot.bytes, caret);
        let bytes = snapshot.bytes.clone();
        let rpc = client.borrow_mut().send_request("textDocument/completion", json!({
            "textDocument":{"uri":uri}, "position":{"line":position.row,"character":utf16}, "context":context
        }), move |result| {
            let list = result.map_err(|error| error.message)
                .and_then(|result| parse_list(result, &bytes, caret).map_err(|error| error.to_string()));
            state.deliver_for(ticket, Some(list));
        })?;
        if refreshing {
            let active = self.active.as_mut().unwrap();
            active.request.cancel();
            if let Some(id) = active.rpc.take() {
                let _ = active.client.borrow_mut().cancel_request(&id);
            }
            active.request = request;
            active.rpc = Some(rpc);
            active.received = false;
            active.origin = origin;
            active.refresh_snapshot = Some((snapshot.bytes, caret, active.changes.len()));
            self.scheduled = None;
            return Ok(());
        }
        self.active = Some(Active {
            view,
            document,
            origin,
            revision,
            bytes: snapshot.bytes.clone(),
            baseline: snapshot.bytes,
            baseline_caret: caret,
            caret,
            prefix_start: prefix,
            changes: Vec::new(),
            items: Vec::new(),
            filtered: Vec::new(),
            selected: 0,
            incomplete: false,
            client,
            rpc: Some(rpc),
            resolve_rpc: None,
            request,
            resolve: Arc::new(LspRequestState::new()),
            resolving: None,
            accept_pending: false,
            received: false,
            refresh_snapshot: None,
            popup_rect: None,
            docs_rect: None,
            scroll_selection: true,
            selection_motion: Default::default(),
            docs_transition: Default::default(),
            docs_item: None,
        });
        Ok(())
    }

    /// Called before document input so Tab/Enter and list navigation never reach
    /// indentation, newline or caret commands while completion owns them.
    pub fn before_draw(
        &mut self,
        ui: &Ui,
        view: &mut EditorView,
        session: &mut DocumentSession,
        settings: &LspPresentationOptions,
        blocked: bool,
    ) -> io::Result<()> {
        self.retain(session);
        if !settings.autocomplete {
            self.scheduled = None;
        }
        if ui.is_window_focused_with_flags(FocusedFlags::CHILD_WINDOWS)
            && self
                .active
                .as_ref()
                .is_some_and(|active| active.view != view.id())
        {
            self.cancel();
        }
        if !ui.is_window_focused_with_flags(FocusedFlags::CHILD_WINDOWS) {
            // Mouse interaction with our windows may temporarily take ImGui
            // focus. Only retain the originating view when the pointer is there.
            let over = self.active.as_ref().is_some_and(|a| {
                a.view == view.id()
                    && [a.popup_rect, a.docs_rect]
                        .into_iter()
                        .flatten()
                        .any(|(min, max)| {
                            let mouse = ui.io().mouse_pos();
                            mouse[0] >= min[0]
                                && mouse[0] <= max[0]
                                && mouse[1] >= min[1]
                                && mouse[1] <= max[1]
                        })
            });
            if !over {
                self.forget_view(view.id());
                return Ok(());
            }
        }
        if blocked || view.find_visible() || view.line_jump_visible() || view.read_only() {
            self.forget_view(view.id());
            return Ok(());
        }
        if settings.shortcuts.take(ui, "bedtime.editor.completion")
            && let Err(error) = self.request(session, view.id(), None, false)
        {
            self.error = Some(error.to_string());
        }
        if let Some((id, _, revision, position)) = self.scheduled
            && id == view.id()
        {
            let state = session.view_snapshot(id)?;
            if session.document_revision(view.document_id())? != revision
                || (state.row, state.column) != position
            {
                self.scheduled = None;
            }
        }
        if self
            .scheduled
            .is_some_and(|(id, at, _, _)| id == view.id() && Instant::now() >= at)
        {
            let _ = self.request(session, view.id(), None, false);
        }
        let Some(active) = &mut self.active else {
            return Ok(());
        };
        if active.view != view.id() {
            return Ok(());
        }
        let selection = session.view_snapshot(view.id())?;
        let caret = session.with_document(active.document, |state| {
            state.offset_from_row_col(selection.row, selection.column)
        })?;
        if selection.selections.len() != 1
            || !selection.selections[0].empty()
            || caret != active.caret
        {
            self.cancel();
            return Ok(());
        }
        active.receive();
        if settings
            .shortcuts
            .take(ui, "bedtime.editor.collapse_selection")
        {
            self.cancel();
            return Ok(());
        }
        if !active.filtered.is_empty() {
            if settings.shortcuts.take(ui, "bedtime.navigation.move_up") {
                active.selected =
                    (active.selected + active.filtered.len() - 1) % active.filtered.len();
                active.scroll_selection = true;
                active.accept_pending = false;
            }
            if settings.shortcuts.take(ui, "bedtime.navigation.move_down") {
                active.selected = (active.selected + 1) % active.filtered.len();
                active.scroll_selection = true;
                active.accept_pending = false;
            }
            if settings.shortcuts.take(ui, "bedtime.editor.indent")
                || settings.shortcuts.take(ui, "bedtime.editor.newline")
            {
                active.accept_pending = true;
            }
        }
        active.resolve_selected();
        self.accept_if_ready(session)?;
        Ok(())
    }

    pub fn observe(
        &mut self,
        response: &ViewResponse,
        session: &mut DocumentSession,
        settings: &LspPresentationOptions,
    ) -> io::Result<()> {
        let mut refresh = false;
        if let Some(active) = &mut self.active
            && active.view == response.view
        {
            let revision = session.document_revision(active.document)?;
            let selection = session.view_snapshot(active.view)?;
            let caret = session.with_document(active.document, |state| {
                state.offset_from_row_col(selection.row, selection.column)
            })?;
            if revision != active.revision {
                let bytes = session.snapshot(active.document)?.bytes;
                let insertion = response
                    .typed_text
                    .as_ref()
                    .is_some_and(|text| text.chars().all(identifier));
                let insertion = insertion
                    && caret >= active.caret
                    && bytes[..active.caret.min(bytes.len())] == active.bytes[..active.caret]
                    && bytes.get(caret..) == active.bytes.get(active.caret..);
                let deletion = response.prefix_backspace
                    && caret >= active.prefix_start
                    && caret < active.caret
                    && bytes.get(..caret) == active.bytes.get(..caret)
                    && bytes.get(caret..) == active.bytes.get(active.caret..);
                if revision.0 != active.revision.0
                    || selection.selections.len() != 1
                    || !selection.selections[0].empty()
                    || !(insertion || deletion)
                {
                    self.cancel();
                } else {
                    let (change, length) = if insertion {
                        (active.caret..active.caret, caret - active.caret)
                    } else {
                        (caret..active.caret, 0)
                    };
                    for item in &mut active.items {
                        transform_item(item, &[(change.clone(), length)]);
                    }
                    active.changes.push((change, length));
                    active.bytes = bytes;
                    active.caret = caret;
                    active.revision = revision;
                    active.filter();
                    refresh = active.incomplete;
                }
            } else if caret != active.caret
                || selection.selections.len() != 1
                || !selection.selections[0].empty()
            {
                self.cancel();
            }
        }
        if !response.focused {
            return Ok(());
        }
        if refresh {
            let _ = self.request(session, response.view, None, true);
            return Ok(());
        }
        if let Some(text) = response.typed_text.as_ref()
            && settings.autocomplete
        {
            let last = text
                .chars()
                .last()
                .map(|c| c.to_string())
                .unwrap_or_default();
            let triggered = session
                .lsp()
                .and_then(|pool| pool.client_for_document(response.document))
                .is_some_and(|client| {
                    client.borrow().server_capabilities()["completionProvider"]["triggerCharacters"]
                        .as_array()
                        .is_some_and(|triggers| {
                            triggers
                                .iter()
                                .any(|trigger| trigger.as_str() == Some(&last))
                        })
                });
            if triggered {
                let _ = self.request(session, response.view, Some(last), false);
            } else if text.chars().all(identifier) && self.active.is_none() {
                let state = session.view_snapshot(response.view)?;
                self.scheduled = Some((
                    response.view,
                    Instant::now() + Duration::from_millis(settings.completion_delay_ms),
                    session.document_revision(response.document)?,
                    (state.row, state.column),
                ));
            } else if !text.chars().all(identifier) {
                self.scheduled = None;
            }
        }
        Ok(())
    }

    fn accept_if_ready(&mut self, session: &mut DocumentSession) -> io::Result<()> {
        let Some(active) = &mut self.active else {
            return Ok(());
        };
        if !active.accept_pending {
            return Ok(());
        }
        let Some(index) = active.filtered.get(active.selected).copied() else {
            return Ok(());
        };
        if !active.items[index].resolved {
            return Ok(());
        }
        if !active.current(session) {
            self.cancel();
            return Ok(());
        }
        let item = &active.items[index];
        if let Some(error) = &item.resolve_error {
            self.error = Some(error.clone());
            self.cancel();
            return Ok(());
        }
        let command = item
            .raw
            .get("command")
            .filter(|command| command.is_object())
            .cloned();
        let mut caret = item.edit.range.start + item.edit.bytes.len();
        for edit in &item.additional {
            if edit.range.end <= item.edit.range.start {
                caret = caret - edit.range.len() + edit.bytes.len();
            }
        }
        let mut edits = item.additional.clone();
        edits.push(item.edit.clone());
        let result = session.apply_edits(active.document, active.revision, &edits);
        if result.is_ok() {
            let (row, column) =
                session.with_document(active.document, |state| state.row_col_from_offset(caret))?;
            session.with_commands(active.view, |commands| {
                commands.set_cursor(row, column, false, CursorReveal::Ensure)
            })?;
            if let Some(command) = command {
                self.commands.push((active.view, command));
            }
        }
        self.cancel();
        if let Err(error) = result {
            self.error = Some(error.to_string());
        }
        Ok(())
    }

    pub fn render(
        &mut self,
        ui: &Ui,
        view: &EditorView,
        session: &mut DocumentSession,
        animations: bool,
    ) -> io::Result<()> {
        let Some(active) = &mut self.active else {
            return Ok(());
        };
        if active.view != view.id() || !active.current(session) {
            return Ok(());
        }
        active.receive();
        if active.filtered.is_empty() {
            active.popup_rect = None;
            active.docs_rect = None;
            active.docs_item = None;
            return Ok(());
        }
        active.resolve_selected();
        let frame = view.presentation();
        frame.tooltip_arbiter.claim(ui);
        let _tooltip_style = hover_popup_style(ui);
        let fs = ui.current_font_size();
        let viewport = ui.window_viewport();
        let vp = viewport.work_pos();
        let vs = viewport.work_size();
        let width = (fs * 26.0).min(vs[0] - 12.0).max(1.0);
        let height = (active.filtered.len().min(8) as f32 * ui.text_line_height_with_spacing()
            + ui.clone_style().window_padding()[1] * 2.0)
            .ceil()
            .min(vs[1] - 12.0)
            .max(1.0);
        let caret = frame.caret_position;
        let caret_line = Some([caret[1], caret[1] + frame.layout.line_height]);
        let bounds = (
            vp.map(|value| value + 4.0),
            std::array::from_fn(|axis| vp[axis] + vs[axis] - 4.0),
        );
        let ([x, y], [width, height]) = popup_placement(
            [caret[0], caret[1] + frame.layout.line_height],
            [width, height],
            bounds,
            caret_line,
        );
        if height < 1.0 {
            active.popup_rect = None;
            active.docs_rect = None;
            return Ok(());
        }
        let flags = WindowFlags::NO_TITLE_BAR
            | WindowFlags::NO_MOVE
            | WindowFlags::NO_RESIZE
            | WindowFlags::NO_COLLAPSE
            | WindowFlags::NO_SAVED_SETTINGS
            | WindowFlags::NO_FOCUS_ON_APPEARING
            | WindowFlags::NO_NAV
            | WindowFlags::NO_DOCKING;
        let kind_colors = session.with_view(active.view, |editor| {
            THEME_SLOTS.map(|slot| editor.highlight.color_for_slot(slot))
        })?;
        let background = ui.style_color(StyleColor::WindowBg);
        let accent = ui.style_color(StyleColor::CheckMark);
        let selected_background = bedtime_ui::color::blend(accent, background, 0.24);
        let hovered_background = bedtime_ui::color::blend(accent, background, 0.16);
        let text = bedtime_ui::presentation::readable_color(ui, ui.style_color(StyleColor::Text));
        let detail_color = bedtime_ui::presentation::muted_text_color(ui);
        let _selected = ui.push_style_color(StyleColor::Header, [0.0; 4]);
        let _hovered = ui.push_style_color(StyleColor::HeaderHovered, hovered_background);
        let _pressed = ui.push_style_color(StyleColor::HeaderActive, selected_background);
        ui.set_next_window_viewport(viewport.id());
        ui.window(format!("##completion_{}", active.view.0))
            .position([x, y], Condition::Always)
            .size([width, height], Condition::Always)
            .flags(flags)
            .build(|| {
                ui.with_bound_context(|| unsafe {
                    dear_imgui_rs::sys::igBringWindowToDisplayFront(
                        dear_imgui_rs::sys::igGetCurrentWindow(),
                    );
                });
                let selected_row = active
                    .selection_motion
                    .sample(
                        active.selected as f32,
                        ui.time(),
                        animations,
                        &bedtime_ui::UiFrameContext::for_ui(ui).theme().motion,
                    )
                    .clamp(0.0, (active.filtered.len() - 1) as f32);
                let pos = ui.cursor_screen_pos();
                let lower = selected_row.floor() as usize;
                let upper = selected_row.ceil() as usize;
                let mut lower_rect = None;
                let mut upper_rect = None;
                let draw = ui.get_window_draw_list();
                // Measure native selectable bounds, including pixel alignment
                // and half-spacing padding, then paint the moving highlight
                // behind the rows. Font spacing alone drifts down the list.
                draw.channels_split(2, |channels| {
                    channels.set_current(1);
                    for (row, index) in active.filtered.iter().copied().enumerate() {
                        let item = &active.items[index];
                        let pos = ui.cursor_screen_pos();
                        let row_width = ui.content_region_avail()[0];
                        let activated = ui
                            .selectable_config(format!("##completion_item_{index}"))
                            .selected(row == active.selected)
                            .size([0.0, ui.text_line_height()])
                            .build();
                        if row == lower {
                            lower_rect = Some((ui.item_rect_min(), ui.item_rect_max()));
                        }
                        if row == upper {
                            upper_rect = Some((ui.item_rect_min(), ui.item_rect_max()));
                        }
                        // Returning focus to the editor on mouse-down can clear
                        // ImGui's active row before its default release activation.
                        if activated
                            || (ui.is_item_hovered() && ui.is_mouse_clicked(MouseButton::Left))
                        {
                            active.selected = row;
                            active.accept_pending = true;
                        }
                        let row_background = if ui.is_item_hovered() {
                            if ui.is_item_active() {
                                selected_background
                            } else {
                                hovered_background
                            }
                        } else if row == active.selected {
                            selected_background
                        } else {
                            background
                        };
                        let readable = |color| {
                            bedtime_ui::color::ensure_contrast(
                                bedtime_ui::color::ensure_contrast(color, selected_background, 4.5),
                                row_background,
                                4.5,
                            )
                        };
                        let (kind, slot) = kind_presentation(item.kind);
                        let kind_width = fs * 4.0;
                        let name_pos = [pos[0] + kind_width, pos[1]];
                        let name = bedtime_ui::presentation::fit_text(
                            ui,
                            &item.label,
                            (row_width - kind_width).max(1.0),
                        );
                        draw.add_text(pos, readable(kind_colors[slot as usize]), kind);
                        draw.add_text(name_pos, readable(text), &name);
                        let detail_pos =
                            [name_pos[0] + ui.calc_text_size(&name)[0] + fs * 0.4, pos[1]];
                        let label_detail =
                            item.raw["labelDetails"]["detail"].as_str().unwrap_or("");
                        let description = item.raw["labelDetails"]["description"]
                            .as_str()
                            .unwrap_or("");
                        let detail = if label_detail.is_empty() {
                            description.to_owned()
                        } else if description.is_empty() {
                            label_detail.to_owned()
                        } else {
                            format!("{label_detail} · {description}")
                        };
                        let detail = bedtime_ui::presentation::fit_text(
                            ui,
                            &detail,
                            (pos[0] + row_width - detail_pos[0]).max(0.0),
                        );
                        draw.add_text(detail_pos, readable(detail_color), &detail);
                    }
                    let (lower_min, lower_max) = lower_rect.unwrap();
                    let (upper_min, upper_max) = upper_rect.unwrap();
                    let fraction = selected_row.fract();
                    let min = std::array::from_fn(|axis| {
                        lower_min[axis] + (upper_min[axis] - lower_min[axis]) * fraction
                    });
                    let max = std::array::from_fn(|axis| {
                        lower_max[axis] + (upper_max[axis] - lower_max[axis]) * fraction
                    });
                    channels.set_current(0);
                    draw.add_rect(min, max, selected_background)
                        .rounding(3.0)
                        .filled(true)
                        .build();
                    if active.scroll_selection
                        || (selected_row - active.selected as f32).abs() > 0.001
                    {
                        let viewport_height = ui.with_bound_context(|| unsafe {
                            let window = &*dear_imgui_rs::sys::igGetCurrentWindowRead();
                            window.InnerRect.Max.y
                                - window.InnerRect.Min.y
                                - window.WindowPadding.y * 2.0
                        });
                        let row_top = min[1] - pos[1];
                        let row_bottom = max[1] - pos[1];
                        let scroll = ui.scroll_y();
                        let target = if row_top < scroll {
                            row_top
                        } else if row_bottom > scroll + viewport_height {
                            row_bottom - viewport_height
                        } else {
                            scroll
                        };
                        if target != scroll {
                            ui.set_scroll_y(target.max(0.0));
                        }
                    }
                });
                active.scroll_selection = false;
            });
        active.popup_rect = Some(([x, y], [x + width, y + height]));
        active.resolve_selected();
        let selected_index = active.filtered[active.selected];
        let reset_docs_scroll = active.docs_item != Some(selected_index);
        let item = &active.items[selected_index];
        {
            // Keep the same details window while resolving a new selection.
            // Hiding it would restart its opening animation when the reply arrives.
            active.docs_item = Some(selected_index);
            let doc_width = (fs * 30.0).min((vs[0] - 12.0).max(1.0));
            let mut doc_height = (fs * 18.0).min((vs[1] - 12.0).max(1.0));
            let (doc_x, doc_y) = if x + width + doc_width + 8.0 <= vp[0] + vs[0] {
                (x + width + 4.0, y)
            } else if x - doc_width - 4.0 >= vp[0] {
                (x - doc_width - 4.0, y)
            } else {
                let above = (y - vp[1] - 8.0).max(1.0);
                let below = (vp[1] + vs[1] - y - height - 8.0).max(1.0);
                doc_height = doc_height.min(above.max(below));
                (
                    x.clamp(
                        vp[0] + 4.0,
                        (vp[0] + vs[0] - doc_width - 4.0).max(vp[0] + 4.0),
                    ),
                    if below >= above {
                        y + height + 4.0
                    } else {
                        y - doc_height - 4.0
                    },
                )
            };
            let ([doc_x, doc_y], [doc_width, doc_height]) =
                popup_placement([doc_x, doc_y], [doc_width, doc_height], bounds, caret_line);
            let details = Details {
                label: item.label.clone(),
                detail: item.detail.clone(),
                documentation: item.documentation.clone(),
                markdown: item.markdown,
                resolving: !item.resolved,
            };
            ui.set_next_window_viewport(viewport.id());
            let mut docs_window = ui
                .window(format!("##completion_docs_{}", active.view.0))
                .position([doc_x, doc_y], Condition::Always)
                .size([doc_width, doc_height], Condition::Always)
                .flags(flags | WindowFlags::HORIZONTAL_SCROLLBAR);
            if reset_docs_scroll {
                docs_window = docs_window.scroll([0.0; 2]);
            }
            docs_window.build(|| {
                ui.with_bound_context(|| unsafe {
                    dear_imgui_rs::sys::igBringWindowToDisplayFront(
                        dear_imgui_rs::sys::igGetCurrentWindow(),
                    );
                });
                let _ = session.with_view(active.view, |editor| {
                    active
                        .docs_transition
                        .draw(ui, &details, animations, |ui, details| {
                            render_hover_plaintext(ui, &details.label, editor);
                            ui.separator();
                            if !details.detail.is_empty() {
                                render_hover_plaintext(ui, &details.detail, editor);
                            }
                            if details.resolving && details.documentation.is_empty() {
                                ui.text_wrapped("Loading documentation…");
                            }
                            if !details.documentation.is_empty() {
                                if !details.detail.is_empty() {
                                    ui.separator();
                                }
                                if details.markdown {
                                    render_hover_markdown(
                                        ui,
                                        &details.documentation,
                                        editor,
                                        &editor.state.language_id,
                                    );
                                } else {
                                    render_hover_plaintext(ui, &details.documentation, editor);
                                }
                            }
                        });
                });
            });
            active.docs_rect = Some(([doc_x, doc_y], [doc_x + doc_width, doc_y + doc_height]));
        }
        if ui.is_mouse_clicked(MouseButton::Left) {
            let mouse = ui.io().mouse_pos();
            let over = [active.popup_rect, active.docs_rect]
                .into_iter()
                .flatten()
                .any(|(min, max)| {
                    mouse[0] >= min[0]
                        && mouse[0] <= max[0]
                        && mouse[1] >= min[1]
                        && mouse[1] <= max[1]
                });
            if over {
                session.request_focus(active.view)?;
            } else {
                self.cancel();
                return Ok(());
            }
        }
        self.accept_if_ready(session)
    }
}

fn kind_presentation(kind: u32) -> (&'static str, ThemeSlot) {
    match kind {
        2 | 3 => ("fn", ThemeSlot::Function),
        4 => ("ctor", ThemeSlot::Type),
        5 => ("field", ThemeSlot::Property),
        6 => ("var", ThemeSlot::Variable),
        7 => ("type", ThemeSlot::Type),
        8 => ("trait", ThemeSlot::Type),
        9 => ("mod", ThemeSlot::Type),
        10 => ("prop", ThemeSlot::Property),
        11 => ("unit", ThemeSlot::Number),
        12 => ("value", ThemeSlot::Constant),
        13 => ("enum", ThemeSlot::Type),
        14 => ("kw", ThemeSlot::Keyword),
        17 => ("file", ThemeSlot::String),
        18 => ("ref", ThemeSlot::Variable),
        19 => ("dir", ThemeSlot::String),
        20 => ("enum", ThemeSlot::Constant),
        21 => ("const", ThemeSlot::Constant),
        22 => ("struct", ThemeSlot::Type),
        23 => ("event", ThemeSlot::Function),
        24 => ("op", ThemeSlot::Operator),
        25 => ("T", ThemeSlot::Type),
        _ => ("·", ThemeSlot::Text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_decode_ranges_and_defaults_and_reject_snippets() {
        let bytes = "let café😀 = Str".as_bytes();
        let list = parse_list(json!({"isIncomplete":true,"itemDefaults":{"editRange":{"start":{"line":0,"character":13},"end":{"line":0,"character":16}}},"items":[
            {"label":"String","textEditText":"String","sortText":"1","additionalTextEdits":[{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":0}},"newText":"use String;\n"}]},
            {"label":"snippet","insertTextFormat":2,"insertText":"f(${1:x})"},
            {"label":"invalid","textEdit":{"range":{"start":{"line":0,"character":9},"end":{"line":0,"character":16}},"newText":"x"}}
        ]}), bytes, bytes.len()).unwrap();
        assert!(list.incomplete);
        assert_eq!(list.items.len(), 1);
        assert_eq!(&bytes[list.items[0].edit.range.clone()], b"Str");
        assert_eq!(list.items[0].additional[0].bytes, b"use String;\n");
    }

    #[test]
    fn prefix_transforms_extend_replacement_and_shift_import_ranges() {
        let mut list = parse_list(json!([{"label":"String","textEdit":{"newText":"String","insert":{"start":{"line":0,"character":0},"end":{"line":0,"character":2}},"replace":{"start":{"line":0,"character":0},"end":{"line":0,"character":4}}}}]), b"Stxx", 2).unwrap();
        transform_item(&mut list.items[0], &[(2..2, 1)]);
        assert_eq!(list.items[0].edit.range, 0..5);
        let mut range = 3..6;
        transform_range(&mut range, 1..2, 0);
        assert_eq!(range, 2..5);
        assert!(matches("StringBuilder", "sbd"));
        assert!(!matches("String", "zz"));
    }

    #[test]
    fn utf16_ranges_reject_surrogate_splits_and_bad_lines() {
        let bytes = "a😀\r\nbé".as_bytes();
        assert_eq!(
            position_offset(bytes, &json!({"line":0,"character":3})).unwrap(),
            5
        );
        assert!(position_offset(bytes, &json!({"line":0,"character":2})).is_err());
        assert_eq!(
            position_offset(bytes, &json!({"line":1,"character":2})).unwrap(),
            10
        );
        assert!(position_offset(bytes, &json!({"line":2,"character":0})).is_err());
    }

    #[test]
    fn completion_accepts_click_tab_enter_and_imports_are_one_undo() {
        use crate::{EditorConfig, EditorRuntime};
        use dear_imgui_rs::{Context, FramePrepareOptions, Key};
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let (_dir, mut session, document) = super::super::test_fixture::session();
        let mut view = EditorView::new(&mut session, document).unwrap();
        session
            .with_commands(view.id(), |commands| {
                commands.set_cursor(0, 14, false, CursorReveal::Ensure)
            })
            .unwrap();
        let mut runtime = EditorRuntime::default();
        let mut config = EditorConfig::default();
        config.lsp.autocomplete = false;
        let mut context = Context::create();
        context
            .set_ini_filename(None::<std::path::PathBuf>)
            .unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        let draw = |context: &mut Context,
                    runtime: &mut EditorRuntime,
                    session: &mut DocumentSession,
                    view: &mut EditorView| {
            session.tick();
            runtime.tick(session, &mut Vec::new()).unwrap();
            session.request_focus(view.id()).unwrap();
            context.prepare_frame(FramePrepareOptions::new([1000.0, 700.0], 1.0 / 60.0));
            let ui = context.frame();
            ui.window("completion_host")
                .size([850.0, 550.0], Condition::Always)
                .build(|| {
                    crate::feature::draw_text_view(
                        ui,
                        view,
                        runtime,
                        session,
                        &config,
                        &mut Vec::new(),
                    )
                    .unwrap();
                });
            assert!(context.render_legacy().draw_data().total_vtx_count() > 0);
        };
        draw(&mut context, &mut runtime, &mut session, &mut view);
        for key in [Some(Key::Tab), Some(Key::Enter), None] {
            runtime.request_completion(&mut session, view.id()).unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                draw(&mut context, &mut runtime, &mut session, &mut view);
                if runtime.completion.active.as_ref().is_some_and(|active| {
                    !active.filtered.is_empty()
                        && active.items[active.filtered[active.selected]].resolved
                }) {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "Completion/resolve response timed out"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
            context.io_mut().add_key_event(Key::DownArrow, true);
            draw(&mut context, &mut runtime, &mut session, &mut view);
            assert_eq!(session.view_snapshot(view.id()).unwrap().column, 14);
            assert_eq!(runtime.completion.active.as_ref().unwrap().selected, 1);
            context.io_mut().add_key_event(Key::DownArrow, false);
            if key.is_some() {
                context.io_mut().add_key_event(Key::UpArrow, true);
            }
            draw(&mut context, &mut runtime, &mut session, &mut view);
            context.io_mut().add_key_event(Key::UpArrow, false);
            draw(&mut context, &mut runtime, &mut session, &mut view);
            if let Some(key) = key {
                context.io_mut().add_key_event(key, true);
            } else {
                let name = std::ffi::CString::new(format!("##completion_{}", view.id().0)).unwrap();
                let point = context.binding().with_bound_context(|| unsafe {
                    let window = &*dear_imgui_rs::sys::igFindWindowByName(name.as_ptr());
                    [
                        window.DC.CursorStartPos.x + 30.0,
                        window.DC.CursorStartPos.y + 4.0,
                    ]
                });
                context.io_mut().add_mouse_pos_event(point);
                draw(&mut context, &mut runtime, &mut session, &mut view);
                context
                    .io_mut()
                    .add_mouse_button_event(MouseButton::Left, true);
                draw(&mut context, &mut runtime, &mut session, &mut view);
                context
                    .io_mut()
                    .add_mouse_button_event(MouseButton::Left, false);
            }
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                draw(&mut context, &mut runtime, &mut session, &mut view);
                if runtime.completion.active.is_none() {
                    break;
                }
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(5));
            }
            if let Some(key) = key {
                context.io_mut().add_key_event(key, false);
            }
            assert_eq!(
                session.snapshot(document).unwrap().bytes,
                b"use String;\nlet value = String"
            );
            session.undo_document(document).unwrap();
            assert_eq!(session.snapshot(document).unwrap().bytes, b"let value = St");
            session
                .with_commands(view.id(), |commands| {
                    commands.set_cursor(0, 14, false, CursorReveal::Ensure)
                })
                .unwrap();
            draw(&mut context, &mut runtime, &mut session, &mut view);
        }
    }

    fn native_context() -> dear_imgui_rs::Context {
        let mut context = dear_imgui_rs::Context::create();
        context
            .set_ini_filename(None::<std::path::PathBuf>)
            .unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        context
    }

    fn native_frame(
        context: &mut dear_imgui_rs::Context,
        runtime: &mut crate::EditorRuntime,
        session: &mut DocumentSession,
        view: &mut EditorView,
        config: &crate::EditorConfig,
    ) {
        session.tick();
        let mut requests = Vec::new();
        runtime.tick(session, &mut requests).unwrap();
        session.request_focus(view.id()).unwrap();
        context.prepare_frame(dear_imgui_rs::FramePrepareOptions::new(
            [1000.0, 700.0],
            1.0 / 60.0,
        ));
        let ui = context.frame();
        ui.window("completion_host")
            .size([850.0, 550.0], Condition::Always)
            .build(|| {
                crate::feature::draw_text_view(ui, view, runtime, session, config, &mut requests)
                    .unwrap();
            });
        drop(context.render_legacy());
        assert!(
            requests.is_empty(),
            "Unexpected language action notification: {requests:?}"
        );
    }

    fn wait_completion(
        context: &mut dear_imgui_rs::Context,
        runtime: &mut crate::EditorRuntime,
        session: &mut DocumentSession,
        view: &mut EditorView,
        config: &crate::EditorConfig,
    ) {
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            native_frame(context, runtime, session, view, config);
            if runtime.completion.active.as_ref().is_some_and(|a| {
                a.received && !a.filtered.is_empty() && a.items[a.filtered[a.selected]].resolved
            }) {
                return;
            }
            assert!(Instant::now() < until, "Completion fixture timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn native_completion_keyboard_highlight_matches_hovered_row_bounds() {
        use dear_imgui_rs::sys;

        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        for size in [13.0, 20.0, 21.5] {
            let (_dir, mut session, document) = super::super::test_fixture::session();
            let mut view = EditorView::new(&mut session, document).unwrap();
            session
                .with_commands(view.id(), |commands| {
                    commands.set_cursor(0, 14, false, CursorReveal::Ensure)
                })
                .unwrap();
            let mut runtime = crate::EditorRuntime::default();
            let mut config = crate::EditorConfig::default();
            config.lsp.autocomplete = false;
            let mut context = native_context();
            let mut fonts = bedtime_ui::font::UiFonts::default();
            fonts
                .apply(
                    &mut context,
                    &bedtime_ui::appearance::UiFontOptions {
                        size,
                        ..Default::default()
                    },
                    false,
                )
                .unwrap();
            config.options.font = fonts.main;
            context
                .font_atlas()
                .try_claim_legacy_renderer()
                .unwrap()
                .build();
            native_frame(&mut context, &mut runtime, &mut session, &mut view, &config);
            runtime.request_completion(&mut session, view.id()).unwrap();
            wait_completion(&mut context, &mut runtime, &mut session, &mut view, &config);
            let active = runtime.completion.active.as_mut().unwrap();
            let item = active.items[active.filtered[active.selected]].clone();
            active.items = vec![item; 24];
            active.filtered = (0..24).collect();
            let name = std::ffi::CString::new(format!("##completion_{}", view.id().0)).unwrap();
            let background = [0.04, 0.05, 0.08, 1.0];
            let accent = [0.15, 0.40, 0.70, 1.0];
            context
                .style_mut()
                .set_color(StyleColor::PopupBg, background);
            context.style_mut().set_color(StyleColor::CheckMark, accent);
            let bounds = |context: &dear_imgui_rs::Context, amount| {
                context.binding().with_bound_context(|| unsafe {
                    let color = bedtime_ui::color::blend(accent, background, amount);
                    let color = sys::igColorConvertFloat4ToU32(color.into());
                    let window = &*sys::igFindWindowByName(name.as_ptr());
                    let white = (*(*sys::igGetIO_Nil()).Fonts).TexUvWhitePixel;
                    let vertices = &(*window.DrawList).VtxBuffer;
                    let mut min = [f32::INFINITY; 2];
                    let mut max = [f32::NEG_INFINITY; 2];
                    for index in 0..vertices.Size {
                        let vertex = &*vertices.Data.add(index as usize);
                        let same_color = (0..4).all(|channel| {
                            let shift = channel * 8;
                            ((vertex.col >> shift) & 255).abs_diff((color >> shift) & 255) <= 1
                        });
                        if same_color && vertex.uv.x == white.x && vertex.uv.y == white.y {
                            min[0] = min[0].min(vertex.pos.x);
                            min[1] = min[1].min(vertex.pos.y);
                            max[0] = max[0].max(vertex.pos.x);
                            max[1] = max[1].max(vertex.pos.y);
                        }
                    }
                    assert!(
                        min.into_iter().chain(max).all(f32::is_finite),
                        "missing rectangle: font={size}, blend={amount}, color={color:#x}"
                    );
                    (min, max)
                })
            };
            for animations in [false, true] {
                config.navigation_animations = animations;
                for selected in [0, 6, 20] {
                    context.io_mut().add_mouse_pos_event([-100.0; 2]);
                    let active = runtime.completion.active.as_mut().unwrap();
                    active.selected = selected;
                    active.scroll_selection = true;
                    for _ in 0..14 {
                        native_frame(&mut context, &mut runtime, &mut session, &mut view, &config);
                    }
                    let keyboard = bounds(&context, 0.24);
                    // Use the actual native layout stride, including its pixel
                    // alignment, to place the pointer on the selected row.
                    let mouse = context.binding().with_bound_context(|| unsafe {
                        let window = &*sys::igFindWindowByName(name.as_ptr());
                        let start = window.DC.CursorStartPos;
                        let stride = (window.DC.CursorPosPrevLine.y - start.y) / 23.0;
                        [
                            start.x + 5.0,
                            start.y + selected as f32 * stride + size * 0.5,
                        ]
                    });
                    context.io_mut().add_mouse_pos_event(mouse);
                    for _ in 0..2 {
                        native_frame(&mut context, &mut runtime, &mut session, &mut view, &config);
                    }
                    let hovered = bounds(&context, 0.16);
                    for (keyboard, hovered) in keyboard
                        .0
                        .into_iter()
                        .chain(keyboard.1)
                        .zip(hovered.0.into_iter().chain(hovered.1))
                    {
                        assert!(
                            (keyboard - hovered).abs() <= 0.6,
                            "font={size}, row={selected}, animations={animations}: keyboard={keyboard}, hover={hovered}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn native_completion_selection_eases_highlight_and_scroll_together() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let (_dir, mut session, document) = super::super::test_fixture::session();
        let mut view = EditorView::new(&mut session, document).unwrap();
        session
            .with_commands(view.id(), |commands| {
                commands.set_cursor(0, 14, false, CursorReveal::Ensure)
            })
            .unwrap();
        let mut runtime = crate::EditorRuntime::default();
        let mut config = crate::EditorConfig::default();
        config.lsp.autocomplete = false;
        let mut context = native_context();
        native_frame(&mut context, &mut runtime, &mut session, &mut view, &config);
        runtime.request_completion(&mut session, view.id()).unwrap();
        wait_completion(&mut context, &mut runtime, &mut session, &mut view, &config);
        let active = runtime.completion.active.as_mut().unwrap();
        let item = active.items[active.filtered[active.selected]].clone();
        active.items = vec![item; 24];
        active.filtered = (0..24).collect();
        active.selected = 0;
        for _ in 0..12 {
            native_frame(&mut context, &mut runtime, &mut session, &mut view, &config);
        }
        let active = runtime.completion.active.as_mut().unwrap();
        active.selected = 20;
        active.scroll_selection = true;
        let mut scrolls = Vec::new();
        let name = std::ffi::CString::new(format!("##completion_{}", view.id().0)).unwrap();
        for _ in 0..12 {
            native_frame(&mut context, &mut runtime, &mut session, &mut view, &config);
            scrolls.push(context.binding().with_bound_context(|| unsafe {
                (*dear_imgui_rs::sys::igFindWindowByName(name.as_ptr()))
                    .Scroll
                    .y
            }));
        }
        let end = *scrolls.last().unwrap();
        assert!(end > 0.0);
        assert!(
            scrolls
                .iter()
                .any(|scroll| *scroll > 0.0 && *scroll < end - 1.0),
            "selection must scroll through intermediate positions: {scrolls:?}"
        );
        assert!(scrolls.windows(2).all(|pair| pair[0] <= pair[1]));
        config.navigation_animations = false;
        let active = runtime.completion.active.as_mut().unwrap();
        active.selected = 0;
        active.scroll_selection = true;
        for _ in 0..2 {
            native_frame(&mut context, &mut runtime, &mut session, &mut view, &config);
        }
        context.binding().with_bound_context(|| unsafe {
            assert_eq!(
                (*dear_imgui_rs::sys::igFindWindowByName(name.as_ptr()))
                    .Scroll
                    .y,
                0.0
            );
        });
    }

    #[test]
    fn docked_completion_insertion_keeps_editor_origin_and_ancestor_scroll_fixed() {
        use bedtime_api::documents::{PanelBinding, PanelInput};
        use bedtime_api::workbench::{PanelCreation, WorkbenchRegistry};
        use bevy::prelude::App;
        use dear_imgui_rs::sys;

        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        for (click, wrap) in [(false, false), (true, false), (false, true), (true, true)] {
            let (_dir, mut session, document) = super::super::test_fixture::session();
            let source = format!("let value = {}St", " ".repeat(220));
            let revision = session.document_revision(document).unwrap();
            session
                .apply_edits(
                    document,
                    revision,
                    &[ByteEdit {
                        range: 0..14,
                        bytes: source.as_bytes().to_vec(),
                    }],
                )
                .unwrap();
            session.save(document).unwrap();
            let mut app = App::new();
            app.world_mut().insert_non_send(session);
            app.add_plugins((
                bedtime_workbench::WorkbenchPlugin,
                crate::feature::EditorPlugin,
            ));
            app.world_mut()
                .non_send_mut::<crate::EditorConfig>()
                .lsp
                .autocomplete = false;
            let panel = bedtime_workbench::spawn_panel(
                app.world_mut(),
                json!({"panel_type": crate::TEXT_PANEL_TYPE}),
                None,
                None,
            )
            .unwrap();
            app.world_mut().entity_mut(panel).insert(PanelBinding {
                input: PanelInput::Document(document),
                document: Some(document),
                ..Default::default()
            });
            let factory = app
                .world()
                .resource::<WorkbenchRegistry>()
                .panel(&bedtime_api::PanelTypeId::from(crate::TEXT_PANEL_TYPE))
                .unwrap()
                .factory;
            let frontend = app
                .world_mut()
                .run_system_with(
                    factory,
                    PanelCreation {
                        entity: panel,
                        state: Value::Null,
                    },
                )
                .unwrap()
                .unwrap();
            bedtime_workbench::install_panel_frontend(app.world_mut(), panel, frontend).unwrap();
            let view_id = {
                let mut views = app
                    .world_mut()
                    .non_send_mut::<crate::feature::EditorViews>();
                let view = views.text_mut(panel).unwrap();
                view.set_minimap_enabled(false);
                view.set_soft_wrap(wrap);
                view.id()
            };
            app.world_mut()
                .non_send_mut::<DocumentSession>()
                .with_commands(view_id, |commands| {
                    commands.set_cursor(0, source.len() as i32, false, CursorReveal::Ensure)
                })
                .unwrap();
            let mut context = native_context();
            let mut fonts = bedtime_ui::font::UiFonts::default();
            fonts
                .apply(
                    &mut context,
                    &bedtime_ui::appearance::UiFontOptions::default(),
                    false,
                )
                .unwrap();
            app.world_mut()
                .non_send_mut::<crate::EditorConfig>()
                .options
                .font = fonts.main;
            context
                .font_atlas()
                .try_claim_legacy_renderer()
                .unwrap()
                .build();
            context
                .io_mut()
                .set_config_flags(dear_imgui_rs::ConfigFlags::DOCKING_ENABLE);
            let published_revision = std::cell::Cell::new(
                app.world()
                    .non_send::<DocumentSession>()
                    .document_revision(document)
                    .unwrap(),
            );
            let draw = |context: &mut dear_imgui_rs::Context, app: &mut App| {
                app.world_mut().non_send_mut::<DocumentSession>().tick();
                app.update();
                // Debugger facts are published at frame entry and may lag edits
                // by a frame. Keyboard acceptance edits before drawing; mouse
                // acceptance edits after drawing. Exercise both stale snapshots.
                let revision = app
                    .world()
                    .non_send::<DocumentSession>()
                    .document_revision(document)
                    .unwrap();
                app.world_mut()
                    .non_send_mut::<bedtime_editor_widgets::extensions::EditorExtensions>()
                    .set_source_debug(
                        document,
                        published_revision.replace(revision),
                        bedtime_editor_widgets::SourceDebugPresentation {
                            breakpoints: vec![bedtime_editor_widgets::SourceBreakpoint {
                                row: 0,
                                enabled: true,
                                status: bedtime_editor_widgets::BreakpointStatus::Pending,
                            }],
                            execution_row: None,
                        },
                    );
                context.prepare_frame(dear_imgui_rs::FramePrepareOptions::new(
                    [1001.0, 701.0],
                    1.0 / 60.0,
                ));
                bedtime_workbench::render_ui(app.world_mut(), context.frame());
                drop(context.render_legacy());
                bedtime_workbench::process_requests(app.world_mut());
            };
            for _ in 0..30 {
                draw(&mut context, &mut app);
            }
            let mut session = app
                .world_mut()
                .remove_non_send::<DocumentSession>()
                .unwrap();
            app.world_mut()
                .non_send_mut::<crate::EditorRuntime>()
                .request_completion(&mut session, view_id)
                .unwrap();
            app.world_mut().insert_non_send(session);
            let until = Instant::now() + Duration::from_secs(5);
            loop {
                draw(&mut context, &mut app);
                if app
                    .world()
                    .non_send::<crate::EditorRuntime>()
                    .completion
                    .active
                    .as_ref()
                    .is_some_and(|a| {
                        a.received
                            && !a.filtered.is_empty()
                            && a.items[a.filtered[a.selected]].resolved
                    })
                {
                    break;
                }
                assert!(Instant::now() < until);
                std::thread::sleep(Duration::from_millis(5));
            }
            let baseline = app
                .world()
                .non_send::<crate::feature::EditorViews>()
                .text(panel)
                .unwrap()
                .presentation()
                .layout
                .text_pos[0]
                + app
                    .world()
                    .non_send::<DocumentSession>()
                    .view_snapshot(view_id)
                    .unwrap()
                    .scroll_position[0];
            {
                let mut runtime = app.world_mut().non_send_mut::<crate::EditorRuntime>();
                let active = runtime.completion.active.as_mut().unwrap();
                let index = active.filtered[active.selected];
                active.items[index].edit.range = source.len() - 2..source.len();
                active.items[index].edit.bytes =
                    "LongRustCompletionIdentifier".repeat(180).into_bytes();
            }
            if click {
                let (min, _) = app
                    .world()
                    .non_send::<crate::EditorRuntime>()
                    .completion
                    .active
                    .as_ref()
                    .unwrap()
                    .popup_rect
                    .unwrap();
                context
                    .io_mut()
                    .add_mouse_pos_event([min[0] + 30.0, min[1] + 10.0]);
                draw(&mut context, &mut app);
                context
                    .io_mut()
                    .add_mouse_button_event(MouseButton::Left, true);
            } else {
                context
                    .io_mut()
                    .add_key_event(dear_imgui_rs::Key::Enter, true);
            }
            for frame in 0..8 {
                draw(&mut context, &mut app);
                context
                    .io_mut()
                    .add_key_event(dear_imgui_rs::Key::Enter, false);
                context
                    .io_mut()
                    .add_mouse_button_event(MouseButton::Left, false);
                let position = app
                    .world()
                    .non_send::<DocumentSession>()
                    .view_snapshot(view_id)
                    .unwrap();
                assert_eq!(
                    (position.row, position.column),
                    (
                        1,
                        (source.len() - 2 + "LongRustCompletionIdentifier".len() * 180) as i32
                    ),
                    "completion caret, frame {frame}, click={click}, wrap={wrap}",
                );
                let scroll = position.scroll_position[0];
                let origin = app
                    .world()
                    .non_send::<crate::feature::EditorViews>()
                    .text(panel)
                    .unwrap()
                    .presentation()
                    .layout
                    .text_pos[0]
                    + scroll;
                assert!(
                    (origin - baseline).abs() < 1.0,
                    "frame {frame}, click={click}, wrap={wrap}: {baseline} -> {origin}"
                );
                context.binding().with_bound_context(|| unsafe {
                    let context = &*sys::igGetCurrentContext();
                    for index in 0..context.Windows.Size {
                        let window = &**context.Windows.Data.add(index as usize);
                        let name = std::ffi::CStr::from_ptr(window.Name).to_string_lossy();
                        if window.Active
                            && !window.Hidden
                            && name.contains("bed_tab_")
                            && !name.contains("/##editor_")
                        {
                            assert_eq!(
                                window.Scroll.x, 0.0,
                                "ancestor {name}, frame {frame}, wrap={wrap}"
                            );
                        }
                    }
                });
            }
            assert!(
                app.world()
                    .non_send::<crate::EditorRuntime>()
                    .completion
                    .active
                    .is_none()
            );
        }
    }

    #[test]
    fn native_completion_acceptance_keeps_horizontal_scroll_stable() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let draw = |context: &mut dear_imgui_rs::Context,
                    runtime: &mut crate::EditorRuntime,
                    session: &mut DocumentSession,
                    view: &mut EditorView,
                    config: &crate::EditorConfig| {
            session.tick();
            runtime.tick(session, &mut Vec::new()).unwrap();
            context.prepare_frame(dear_imgui_rs::FramePrepareOptions::new(
                [1000.0, 700.0],
                1.0 / 60.0,
            ));
            let ui = context.frame();
            ui.window("completion_host")
                .size([850.0, 550.0], Condition::Always)
                .build(|| {
                    crate::feature::draw_text_view(
                        ui,
                        view,
                        runtime,
                        session,
                        config,
                        &mut Vec::new(),
                    )
                    .unwrap();
                });
            drop(context.render_legacy());
        };
        for (click, wrap) in [(false, false), (true, false), (false, true), (true, true)] {
            let (_dir, mut session, document) = super::super::test_fixture::session();
            let revision = session.document_revision(document).unwrap();
            let source = format!("let value = {}St", " ".repeat(220));
            session
                .apply_edits(
                    document,
                    revision,
                    &[ByteEdit {
                        range: 0..14,
                        bytes: source.as_bytes().to_vec(),
                    }],
                )
                .unwrap();
            let mut view = EditorView::new(&mut session, document).unwrap();
            view.set_soft_wrap(wrap);
            view.set_minimap_enabled(false);
            session
                .with_commands(view.id(), |commands| {
                    commands.set_cursor(0, source.len() as i32, false, CursorReveal::Ensure)
                })
                .unwrap();
            let mut runtime = crate::EditorRuntime::default();
            let mut config = crate::EditorConfig::default();
            config.lsp.autocomplete = false;
            let mut context = dear_imgui_rs::Context::create();
            context
                .set_ini_filename(None::<std::path::PathBuf>)
                .unwrap();
            let mut fonts = bedtime_ui::font::UiFonts::default();
            fonts
                .apply(
                    &mut context,
                    &bedtime_ui::appearance::UiFontOptions::default(),
                    false,
                )
                .unwrap();
            config.options.font = fonts.main;
            context
                .font_atlas()
                .try_claim_legacy_renderer()
                .unwrap()
                .build();
            for _ in 0..4 {
                native_frame(&mut context, &mut runtime, &mut session, &mut view, &config);
            }
            let before = session.view_snapshot(view.id()).unwrap().scroll_position[0];
            if !wrap {
                assert!(before > 500.0);
            }
            let origin = view.presentation().layout.text_pos[0] + before;

            runtime.request_completion(&mut session, view.id()).unwrap();
            wait_completion(&mut context, &mut runtime, &mut session, &mut view, &config);
            let active = runtime.completion.active.as_mut().unwrap();
            let selected = active.filtered[active.selected];
            active.items[selected].edit.range = source.len() - 2..source.len();
            let completion = "LongRustCompletionIdentifier".repeat(180);
            active.items[selected].edit.bytes = completion.as_bytes().to_vec();
            if click {
                let (min, _) = active.popup_rect.unwrap();
                context
                    .io_mut()
                    .add_mouse_pos_event([min[0] + 30.0, min[1] + 10.0]);
                draw(&mut context, &mut runtime, &mut session, &mut view, &config);
                context
                    .io_mut()
                    .add_mouse_button_event(MouseButton::Left, true);
            } else {
                context
                    .io_mut()
                    .add_key_event(dear_imgui_rs::Key::Enter, true);
            }
            for index in 0..6 {
                draw(&mut context, &mut runtime, &mut session, &mut view, &config);
                let scroll = session.view_snapshot(view.id()).unwrap().scroll_position[0];
                let next_origin = view.presentation().layout.text_pos[0] + scroll;
                assert!(
                    (next_origin - origin).abs() < 1.0,
                    "editor layout shifted on frame {index}: {origin} -> {next_origin}, click={click}"
                );
                assert!(
                    scroll >= before - 1.0,
                    "completion moved left on frame {index}: {before} -> {scroll}, click={click}"
                );
                context
                    .io_mut()
                    .add_key_event(dear_imgui_rs::Key::Enter, false);
                context
                    .io_mut()
                    .add_mouse_button_event(MouseButton::Left, false);
            }
            assert!(runtime.completion.active.is_none());
            assert_eq!(
                session.snapshot(document).unwrap().bytes,
                format!("use String;\nlet value = {}{completion}", " ".repeat(220)).as_bytes()
            );
        }
    }

    #[test]
    fn native_completion_tooltips_keep_shared_style_and_reset_late_documentation_scroll() {
        use dear_imgui_rs::sys;

        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let (_dir, mut session, document) = super::super::test_fixture::session();
        let mut view = EditorView::new(&mut session, document).unwrap();
        session
            .with_commands(view.id(), |commands| {
                commands.set_cursor(0, 14, false, CursorReveal::Ensure)
            })
            .unwrap();
        let mut runtime = crate::EditorRuntime::default();
        let mut config = crate::EditorConfig::default();
        config.lsp.autocomplete = false;
        let mut context = native_context();
        context.style_mut().set_window_padding([0.0; 2]);
        context.style_mut().set_window_rounding(0.0);
        context.style_mut().set_window_border_size(0.0);
        context.style_mut().set_item_spacing([17.0, 19.0]);
        context.style_mut().set_scrollbar_size(23.0);
        native_frame(&mut context, &mut runtime, &mut session, &mut view, &config);
        runtime.request_completion(&mut session, view.id()).unwrap();
        wait_completion(&mut context, &mut runtime, &mut session, &mut view, &config);
        let active = runtime.completion.active.as_mut().unwrap();
        let selected = active.filtered[active.selected];
        active.items[selected].documentation = active.items[selected].documentation.repeat(3);
        for _ in 0..3 {
            native_frame(&mut context, &mut runtime, &mut session, &mut view, &config);
        }
        let list_name = std::ffi::CString::new(format!("##completion_{}", view.id().0)).unwrap();
        let docs_name =
            std::ffi::CString::new(format!("##completion_docs_{}", view.id().0)).unwrap();
        context.binding().with_bound_context(|| unsafe {
            let list = sys::igFindWindowByName(list_name.as_ptr());
            let docs = sys::igFindWindowByName(docs_name.as_ptr());
            assert!(!list.is_null() && !docs.is_null());
            let list = &*list;
            let docs = &*docs;
            assert!(list.Active && docs.Active);
            assert_eq!(
                list.WindowRounding,
                bedtime_ui::appearance::UiMetrics::default().popup_rounding
            );
            assert_eq!(docs.WindowRounding, list.WindowRounding);
            assert_eq!(
                list.WindowBorderSize,
                bedtime_ui::appearance::UiMetrics::default().border
            );
            assert_eq!(docs.WindowBorderSize, list.WindowBorderSize);
            assert!(list.WindowPadding.x > 0.0 && list.WindowPadding.y > 0.0);
            assert_eq!(docs.WindowPadding.x, list.WindowPadding.x);
            assert_eq!(docs.WindowPadding.y, list.WindowPadding.y);
            assert_eq!(docs.ScrollMax.x, 0.0);
            assert!(docs.ScrollMax.y > 50.0);
            sys::igSetScrollY_WindowPtr(sys::igFindWindowByName(docs_name.as_ptr()), 80.0);
        });
        native_frame(&mut context, &mut runtime, &mut session, &mut view, &config);
        context.binding().with_bound_context(|| unsafe {
            assert!((*sys::igFindWindowByName(docs_name.as_ptr())).Scroll.y > 0.0);
        });
        // A newly selected item can have no docs until its resolve finishes.
        // Showing its eventual content must start at the top of the sidecar.
        let active = runtime.completion.active.as_mut().unwrap();
        active.selected = 1;
        let selected = active.filtered[active.selected];
        active.items[selected].resolved = true;
        active.items[selected].documentation.clear();
        active.items[selected].detail.clear();
        native_frame(&mut context, &mut runtime, &mut session, &mut view, &config);
        let active = runtime.completion.active.as_mut().unwrap();
        assert!(
            active.docs_rect.is_some(),
            "details stay visible while documentation arrives"
        );
        active.items[selected].documentation =
            "Late documentation for the newly selected item. ".repeat(50);
        native_frame(&mut context, &mut runtime, &mut session, &mut view, &config);
        context.binding().with_bound_context(|| unsafe {
            assert_eq!((*sys::igFindWindowByName(docs_name.as_ptr())).Scroll.y, 0.0);
        });
        // Near the bottom edge the list opens above the cursor. Its taller
        // documentation must also fit above or below the cursor's visual line.
        for viewport_height in [180.0, 360.0] {
            context.prepare_frame(dear_imgui_rs::FramePrepareOptions::new(
                [1000.0, viewport_height],
                1.0 / 60.0,
            ));
            let ui = context.frame();
            ui.window("completion_host")
                .position([0.0, viewport_height - 80.0], Condition::Always)
                .size([850.0, 75.0], Condition::Always)
                .build(|| {
                    crate::feature::draw_text_view(
                        ui,
                        &mut view,
                        &mut runtime,
                        &mut session,
                        &config,
                        &mut Vec::new(),
                    )
                    .unwrap();
                    let frame = view.presentation();
                    let top = frame.caret_position[1];
                    let bottom = top + frame.layout.line_height;
                    let active = runtime.completion.active.as_ref().unwrap();
                    for (min, max) in [active.popup_rect.unwrap(), active.docs_rect.unwrap()] {
                        assert!(
                            min[1] >= bottom || max[1] <= top,
                            "{min:?} {max:?} cover cursor line {top}..{bottom}"
                        );
                        assert!(min[1] >= 0.0 && max[1] <= viewport_height);
                    }
                });
            drop(context.render_legacy());
        }
        assert_eq!(context.style().window_padding(), [0.0; 2]);
        assert_eq!(context.style().window_rounding(), 0.0);
        assert_eq!(context.style().window_border_size(), 0.0);
        assert_eq!(context.style().item_spacing(), [17.0, 19.0]);
        assert_eq!(context.style().scrollbar_size(), 23.0);
    }

    #[test]
    fn native_completion_kind_name_and_details_remain_readable_on_light_and_dark_rows() {
        use bedtime_highlight::tree_sitter::ThemeColors;
        use dear_imgui_rs::sys;

        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let (_dir, mut session, document) = super::super::test_fixture::session();
        let mut view = EditorView::new(&mut session, document).unwrap();
        session
            .with_commands(view.id(), |commands| {
                commands.set_cursor(0, 14, false, CursorReveal::Ensure)
            })
            .unwrap();
        let mut palette = ThemeColors::default();
        palette.slots[ThemeSlot::Function as usize] = [0.48, 0.18, 0.58, 1.0];
        palette.slots[ThemeSlot::Type as usize] = [0.12, 0.48, 0.36, 1.0];
        session.set_highlight_theme(palette);
        let mut runtime = crate::EditorRuntime::default();
        let mut config = crate::EditorConfig::default();
        config.lsp.autocomplete = false;
        let mut context = native_context();
        native_frame(&mut context, &mut runtime, &mut session, &mut view, &config);
        runtime.request_completion(&mut session, view.id()).unwrap();
        wait_completion(&mut context, &mut runtime, &mut session, &mut view, &config);
        let active = runtime.completion.active.as_mut().unwrap();
        active.items[active.filtered[0]].kind = 3;
        active.items[active.filtered[1]].kind = 7;
        active.items[active.filtered[0]].raw["labelDetails"] =
            json!({"detail":"(参数)\nextra","description":"module"});
        let name = std::ffi::CString::new(format!("##completion_{}", view.id().0)).unwrap();
        let accent = [0.15, 0.40, 0.70, 1.0];
        for background in [[0.97, 0.95, 0.92, 1.0], [0.04, 0.05, 0.08, 1.0]] {
            context
                .style_mut()
                .set_color(StyleColor::PopupBg, background);
            context
                .style_mut()
                .set_color(StyleColor::Text, [0.52, 0.52, 0.52, 1.0]);
            context.style_mut().set_color(StyleColor::CheckMark, accent);
            context.io_mut().add_mouse_pos_event([-100.0; 2]);
            for _ in 0..3 {
                native_frame(&mut context, &mut runtime, &mut session, &mut view, &config);
            }
            context.binding().with_bound_context(|| unsafe {
                let window = sys::igFindWindowByName(name.as_ptr());
                assert!(!window.is_null());
                let window = &*window;
                let white = (*(*sys::igGetIO_Nil()).Fonts).TexUvWhitePixel;
                let vertices = &(*window.DrawList).VtxBuffer;
                let mut text_colors = std::collections::HashSet::new();
                for index in 0..vertices.Size {
                    let vertex = &*vertices.Data.add(index as usize);
                    // Native borders can also use textured antialiasing. Limit
                    // this check to the row contents before selecting glyph UVs.
                    if vertex.pos.x < window.DC.CursorStartPos.x
                        || vertex.pos.y < window.DC.CursorStartPos.y
                        || vertex.pos.x > window.Pos.x + window.Size.x - window.WindowPadding.x
                        || vertex.pos.y > window.Pos.y + window.Size.y - window.WindowPadding.y
                        || (vertex.uv.x == white.x && vertex.uv.y == white.y)
                    {
                        continue;
                    }
                    let color: [f32; 4] = std::array::from_fn(|channel| {
                        ((vertex.col >> (channel * 8)) & 255) as f32 / 255.0
                    });
                    let selected =
                        vertex.pos.y < window.DC.CursorStartPos.y + window.FontRefSize + 1.0;
                    let row_background = if selected {
                        bedtime_ui::color::blend(accent, background, 0.24)
                    } else {
                        background
                    };
                    assert!(
                        bedtime_ui::color::contrast_ratio(
                            bedtime_ui::color::blend(color, row_background, color[3]),
                            row_background
                        ) >= 4.4,
                        "{color:?} on {row_background:?}"
                    );
                    text_colors.insert(vertex.col);
                }
                assert!(
                    text_colors.len() >= 3,
                    "kind, name, and detail use distinct readable colors"
                );
                assert_eq!(window.ScrollMax.x, 0.0);
            });
        }
    }

    #[test]
    fn manual_completion_consumes_native_shortcut_text_with_autocomplete_disabled() {
        use bedtime_keybindings::{Bindings, Overrides, Platform};
        use dear_imgui_rs::Key;

        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        for (mac_mode, shortcut, key, text) in [
            (false, "Ctrl+Space", Key::Space, " "),
            (true, "Ctrl+Space", Key::Space, " "),
            (true, "Ctrl+J", Key::J, "j"),
        ] {
            let (_dir, mut session, document) = super::super::test_fixture::session();
            let mut view = EditorView::new(&mut session, document).unwrap();
            session
                .with_commands(view.id(), |c| {
                    c.set_cursor(0, 14, false, CursorReveal::Ensure)
                })
                .unwrap();
            let mut runtime = crate::EditorRuntime::default();
            let mut config = crate::EditorConfig::default();
            config.lsp.autocomplete = false;
            let actions = bedtime_editor_widgets::shortcuts::actions();
            let mut overrides = Overrides::default();
            overrides
                .replace(
                    actions
                        .iter()
                        .find(|a| a.id == "bedtime.editor.completion")
                        .unwrap(),
                    Some(0),
                    shortcut.parse().unwrap(),
                )
                .unwrap();
            config.options.shortcuts.bindings =
                Rc::new(Bindings::new(actions, &overrides, Platform::current()));
            let mut context = native_context();
            context.io_mut().set_config_macosx_behaviors(mac_mode);
            native_frame(&mut context, &mut runtime, &mut session, &mut view, &config);
            context.io_mut().add_key_event(Key::ModCtrl, true);
            context.io_mut().add_key_event(key, true);
            context.io_mut().add_input_characters_utf8(text);
            native_frame(&mut context, &mut runtime, &mut session, &mut view, &config);
            assert_eq!(session.snapshot(document).unwrap().bytes, b"let value = St");
            // Holding a non-repeating command may still emit native text on
            // later frames. It must neither type nor cancel the pending list.
            for _ in 0..3 {
                context.io_mut().add_key_event(key, true);
                context.io_mut().add_input_characters_utf8(text);
                native_frame(&mut context, &mut runtime, &mut session, &mut view, &config);
                assert_eq!(session.snapshot(document).unwrap().bytes, b"let value = St");
                assert!(runtime.completion.active.is_some());
            }
            context.io_mut().add_key_event(key, false);
            context.io_mut().add_key_event(Key::ModCtrl, false);
            wait_completion(&mut context, &mut runtime, &mut session, &mut view, &config);
            assert!(runtime.completion.visible_for(view.id()));
            context.io_mut().add_key_event(Key::Tab, true);
            native_frame(&mut context, &mut runtime, &mut session, &mut view, &config);
            assert_eq!(
                session.snapshot(document).unwrap().bytes,
                b"use String;\nlet value = String"
            );
            assert!(runtime.completion.active.is_none());
        }
    }

    #[test]
    fn unavailable_manual_completion_consumes_its_space_but_ordinary_space_types() {
        use dear_imgui_rs::{FramePrepareOptions, Key};

        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut session = DocumentSession::new();
        let document = session.create_document(b"unchanged").unwrap();
        let mut view = EditorView::new(&mut session, document).unwrap();
        session
            .with_commands(view.id(), |c| {
                c.set_cursor(0, 9, false, CursorReveal::Ensure)
            })
            .unwrap();
        let mut runtime = crate::EditorRuntime::default();
        let mut config = crate::EditorConfig::default();
        config.lsp.autocomplete = false;
        let mut context = native_context();
        native_frame(&mut context, &mut runtime, &mut session, &mut view, &config);
        context.io_mut().add_key_event(Key::ModCtrl, true);
        context.io_mut().add_key_event(Key::Space, true);
        context.io_mut().add_input_characters_utf8(" ");
        context.prepare_frame(FramePrepareOptions::new([1000.0, 700.0], 1.0 / 60.0));
        let ui = context.frame();
        let mut requests = Vec::new();
        ui.window("completion_host")
            .size([850.0, 550.0], Condition::Always)
            .build(|| {
                crate::feature::draw_text_view(
                    ui,
                    &mut view,
                    &mut runtime,
                    &mut session,
                    &config,
                    &mut requests,
                )
                .unwrap();
            });
        drop(context.render_legacy());
        assert_eq!(session.snapshot(document).unwrap().bytes, b"unchanged");
        assert!(runtime.completion.active.is_none());
        assert!(
            matches!(requests.as_slice(), [bedtime_api::features::HostAction::Notify { message }]
            if message.contains("initialized language server"))
        );
        context.io_mut().add_key_event(Key::Space, false);
        context.io_mut().add_key_event(Key::ModCtrl, false);
        native_frame(&mut context, &mut runtime, &mut session, &mut view, &config);
        context.io_mut().add_key_event(Key::Space, true);
        context.io_mut().add_input_characters_utf8(" ");
        native_frame(&mut context, &mut runtime, &mut session, &mut view, &config);
        assert_eq!(session.snapshot(document).unwrap().bytes, b"unchanged ");
    }

    #[test]
    fn automatic_delay_setting_and_incomplete_refresh_use_actual_input() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let (dir, mut session, document) = super::super::test_fixture::session_mode("incomplete");
        let mut view = EditorView::new(&mut session, document).unwrap();
        session
            .apply_edits(
                document,
                session.document_revision(document).unwrap(),
                &[ByteEdit {
                    range: 12..14,
                    bytes: Vec::new(),
                }],
            )
            .unwrap();
        session
            .with_commands(view.id(), |c| {
                c.set_cursor(0, 12, false, CursorReveal::Ensure)
            })
            .unwrap();
        let mut runtime = crate::EditorRuntime::default();
        let mut config = crate::EditorConfig::default();
        config.lsp.completion_delay_ms = 50;
        let mut context = native_context();
        native_frame(&mut context, &mut runtime, &mut session, &mut view, &config);
        context.io_mut().add_input_characters_utf8("S");
        native_frame(&mut context, &mut runtime, &mut session, &mut view, &config);
        assert!(runtime.completion.scheduled.is_some());
        assert!(runtime.completion.active.is_none());
        native_frame(&mut context, &mut runtime, &mut session, &mut view, &config);
        assert!(
            runtime.completion.active.is_none(),
            "Must honor debounce delay"
        );
        config.lsp.autocomplete = false;
        std::thread::sleep(Duration::from_millis(60));
        native_frame(&mut context, &mut runtime, &mut session, &mut view, &config);
        assert!(runtime.completion.scheduled.is_none());
        assert!(runtime.completion.active.is_none());
        // Manual Ctrl+Space remains available with the automatic setting disabled.
        context
            .io_mut()
            .add_key_event(dear_imgui_rs::Key::ModCtrl, true);
        context
            .io_mut()
            .add_key_event(dear_imgui_rs::Key::Space, true);
        // Native keyboard events carry both the key and its printable text.
        context.io_mut().add_input_characters_utf8(" ");
        native_frame(&mut context, &mut runtime, &mut session, &mut view, &config);
        assert_eq!(session.snapshot(document).unwrap().bytes, b"let value = S");
        context
            .io_mut()
            .add_key_event(dear_imgui_rs::Key::Space, false);
        context
            .io_mut()
            .add_key_event(dear_imgui_rs::Key::ModCtrl, false);
        wait_completion(&mut context, &mut runtime, &mut session, &mut view, &config);
        assert!(runtime.completion.active.as_ref().unwrap().incomplete);
        context.io_mut().add_input_characters_utf8("t");
        native_frame(&mut context, &mut runtime, &mut session, &mut view, &config);
        let active = runtime.completion.active.as_ref().unwrap();
        assert!(!active.received, "Refresh must still be awaiting its reply");
        assert!(
            active.popup_rect.is_some(),
            "Typing must keep the list visible"
        );
        assert!(
            active.docs_rect.is_some(),
            "Keep current documentation while refreshing"
        );
        // Another keystroke can replace the pending refresh. The displayed
        // items retain their original edit baseline until the newest reply.
        context.io_mut().add_input_characters_utf8("r");
        native_frame(&mut context, &mut runtime, &mut session, &mut view, &config);
        assert!(
            runtime
                .completion
                .active
                .as_ref()
                .unwrap()
                .popup_rect
                .is_some()
        );
        wait_completion(&mut context, &mut runtime, &mut session, &mut view, &config);
        assert!(!runtime.completion.active.as_ref().unwrap().incomplete);
        assert!(
            std::fs::read_to_string(dir.path("trace"))
                .unwrap()
                .contains("completion:3")
        );
        assert_eq!(
            session.snapshot(document).unwrap().bytes,
            b"let value = Str"
        );
        context
            .io_mut()
            .add_key_event(dear_imgui_rs::Key::Tab, true);
        native_frame(&mut context, &mut runtime, &mut session, &mut view, &config);
        context
            .io_mut()
            .add_key_event(dear_imgui_rs::Key::Tab, false);
        assert_eq!(
            session.snapshot(document).unwrap().bytes,
            b"use String;\nlet value = String"
        );
    }

    #[test]
    fn delayed_completion_reply_is_discarded_after_caret_movement() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let (_dir, mut session, document) = super::super::test_fixture::session_mode("delayed");
        let mut view = EditorView::new(&mut session, document).unwrap();
        session
            .with_commands(view.id(), |c| {
                c.set_cursor(0, 14, false, CursorReveal::Ensure)
            })
            .unwrap();
        let mut runtime = crate::EditorRuntime::default();
        let config = crate::EditorConfig::default();
        let mut context = native_context();
        native_frame(&mut context, &mut runtime, &mut session, &mut view, &config);
        runtime.request_completion(&mut session, view.id()).unwrap();
        session
            .with_commands(view.id(), |c| {
                c.set_cursor(0, 0, false, CursorReveal::Ensure)
            })
            .unwrap();
        native_frame(&mut context, &mut runtime, &mut session, &mut view, &config);
        assert!(runtime.completion.active.is_none());
        std::thread::sleep(Duration::from_millis(140));
        native_frame(&mut context, &mut runtime, &mut session, &mut view, &config);
        assert!(runtime.completion.active.is_none());
        assert_eq!(session.snapshot(document).unwrap().bytes, b"let value = St");
    }

    #[test]
    fn completion_command_server_edit_joins_the_insertion_undo_group() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let (dir, mut session, document) = super::super::test_fixture::session_mode("commands");
        let mut view = EditorView::new(&mut session, document).unwrap();
        session
            .with_commands(view.id(), |c| {
                c.set_cursor(0, 14, false, CursorReveal::Ensure)
            })
            .unwrap();
        let mut runtime = crate::EditorRuntime::default();
        let config = crate::EditorConfig::default();
        let mut context = native_context();
        native_frame(&mut context, &mut runtime, &mut session, &mut view, &config);
        runtime.request_completion(&mut session, view.id()).unwrap();
        wait_completion(&mut context, &mut runtime, &mut session, &mut view, &config);
        context
            .io_mut()
            .add_key_event(dear_imgui_rs::Key::Tab, true);
        native_frame(&mut context, &mut runtime, &mut session, &mut view, &config);
        context
            .io_mut()
            .add_key_event(dear_imgui_rs::Key::Tab, false);
        let until = Instant::now() + Duration::from_secs(5);
        while !session
            .snapshot(document)
            .unwrap()
            .bytes
            .ends_with(b"String!")
        {
            native_frame(&mut context, &mut runtime, &mut session, &mut view, &config);
            assert!(
                Instant::now() < until,
                "Server command did not apply its edit: {:?}; trace={:?}",
                String::from_utf8_lossy(&session.snapshot(document).unwrap().bytes),
                std::fs::read_to_string(dir.path("trace"))
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            session.snapshot(document).unwrap().bytes,
            b"use String;\nlet value = String!"
        );
        session.undo_document(document).unwrap();
        assert_eq!(session.snapshot(document).unwrap().bytes, b"let value = St");
    }
}
