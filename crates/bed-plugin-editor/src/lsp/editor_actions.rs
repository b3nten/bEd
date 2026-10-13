//! Manual language actions retain a document revision and selection through
//! their request, picker, resolution and edit application.
use bed_api::features::HostAction;
use bed_documents::{DocumentId, DocumentSession, ViewId};
use bed_editing::util::utf8::utf8_byte_offset_to_utf16;
use bed_editor_widgets::EditorView;
use bed_lsp::{
    jsonrpc::RpcId, lsp_client::LspClient, lsp_request::LspRequestState, lsp_uri::LspUri,
    workspace_lsp::LspRequestOrigin,
};
use dear_imgui_rs::{Key, Ui};
use serde_json::{Value, json};
use std::{cell::RefCell, io, rc::Rc, sync::Arc, time::Instant};

pub(crate) const COMMANDS: [(&str, &str); 4] = [
    ("bed.editor.completion", "Complete"),
    ("bed.editor.format_document", "Format Document"),
    ("bed.editor.rename", "Rename Symbol"),
    ("bed.editor.quick_fix", "Quick Fix"),
];

#[derive(Debug)]
enum Stage {
    Format,
    PrepareRename,
    RenameInput {
        name: String,
        opened: bool,
    },
    Rename,
    QuickFix,
    Picker {
        items: Vec<Value>,
        selected: usize,
        opened: bool,
    },
    ResolveAction {
        original: Value,
    },
    Apply {
        action: Value,
    },
    Command,
}

struct Pending {
    view: ViewId,
    document: DocumentId,
    origin: LspRequestOrigin,
    revision: (u64, u64),
    selection: Vec<bed_editing::text_view_state::Selection>,
    position: Value,
    uri: String,
    stage: Stage,
    client: Rc<RefCell<LspClient>>,
    response: Arc<LspRequestState<Result<Value, String>>>,
    rpc: Option<RpcId>,
    started: Instant,
    command_generation: Option<u64>,
}

impl Pending {
    fn lifetime_current(&self, session: &DocumentSession) -> bool {
        session.session_id() == self.origin.session_id
            && session.document_for_view(self.view) == Some(self.document)
            && session
                .lsp()
                .and_then(|pool| pool.request_origin(self.document, self.view, self.revision.0, 0))
                .is_some_and(|origin| origin.server_generation == self.origin.server_generation)
    }

    fn current(&self, session: &DocumentSession) -> bool {
        session.document_for_view(self.view) == Some(self.document)
            && session.document_revision(self.document).ok() == Some(self.revision)
            && session.accepts_lsp_origin(&self.origin)
            && session.view_snapshot(self.view).is_ok_and(|view| {
                matches!(self.stage, Stage::Format) || view.selections == self.selection
            })
    }

    fn send(&mut self, method: &str, params: Value, stage: Stage) -> io::Result<()> {
        self.response.cancel();
        if let Some(id) = self.rpc.take() {
            let _ = self.client.borrow_mut().cancel_request(&id);
        }
        let ticket = self.response.begin();
        let state = self.response.clone();
        self.rpc = Some(
            self.client
                .borrow_mut()
                .send_request(method, params, move |result| {
                    state.deliver(ticket, Some(result.map_err(|error| error.message)));
                })?,
        );
        self.stage = stage;
        self.started = Instant::now();
        Ok(())
    }

    fn cancel(&mut self) {
        self.response.cancel();
        if let Some(id) = self.rpc.take() {
            let _ = self.client.borrow_mut().cancel_request(&id);
        }
    }

    fn refresh_origin(&mut self, session: &DocumentSession) -> io::Result<()> {
        self.revision = session.document_revision(self.document)?;
        self.selection = session.view_snapshot(self.view)?.selections;
        self.origin = session
            .lsp()
            .and_then(|pool| pool.request_origin(self.document, self.view, self.revision.0, 0))
            .ok_or_else(|| io::Error::other("Language server is unavailable"))?;
        Ok(())
    }
}

#[derive(Default)]
pub(crate) struct EditorActions {
    pending: Option<Pending>,
    finished_commands: Vec<u64>,
}

fn notify(requests: &mut Vec<HostAction>, message: impl Into<String>) {
    requests.push(HostAction::Notify {
        message: message.into(),
    });
}

fn command_value(action: &Value) -> Option<Value> {
    if action["command"].is_string() {
        Some(action.clone())
    } else {
        action
            .get("command")
            .filter(|command| command.is_object())
            .cloned()
    }
}

fn quick_fixes(result: Value) -> io::Result<Vec<Value>> {
    if result.is_null() {
        return Ok(Vec::new());
    }
    let items = result.as_array().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "Invalid code action response")
    })?;
    let mut items = items
        .iter()
        .filter(|item| {
            item["title"].is_string()
                && item
                    .get("kind")
                    .and_then(Value::as_str)
                    .is_none_or(|kind| kind == "quickfix" || kind.starts_with("quickfix."))
        })
        .cloned()
        .collect::<Vec<_>>();
    items.sort_by_key(|item| {
        (
            !item["isPreferred"].as_bool().unwrap_or(false),
            item["disabled"].is_object(),
        )
    });
    Ok(items)
}

fn prepare_name(value: &Value, bytes: &[u8]) -> io::Result<String> {
    if value.is_null() {
        return Err(io::Error::other("This symbol cannot be renamed"));
    }
    if let Some(name) = value["placeholder"].as_str() {
        return Ok(name.to_owned());
    }
    if value["defaultBehavior"] == true {
        return Ok(String::new());
    }
    let range = value.get("range").unwrap_or(value);
    let edits = bed_lsp::workspace_edit::parse_text_edits(&json!([{"range":range,"newText":""}]))?;
    let edits = bed_lsp::workspace_edit::byte_edits(bytes, &edits)?;
    String::from_utf8(bytes[edits[0].0.clone()].to_vec())
        .map_err(|error| io::Error::other(error.to_string()))
}

impl EditorActions {
    pub fn cancel(&mut self) {
        if let Some(mut pending) = self.pending.take() {
            pending.cancel();
            if let Some(generation) = pending.command_generation {
                self.finished_commands.push(generation);
            }
        }
    }

    pub fn forget_view(&mut self, view: ViewId) {
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.view == view)
        {
            self.cancel();
        }
    }

    pub fn blocks_view(&self, view: ViewId) -> bool {
        self.pending.as_ref().is_some_and(|pending| {
            pending.view == view
                && matches!(
                    pending.stage,
                    Stage::RenameInput { .. } | Stage::Picker { .. }
                )
        })
    }

    pub fn request(
        &mut self,
        session: &mut DocumentSession,
        view: ViewId,
        command: &str,
    ) -> io::Result<()> {
        self.cancel();
        let document = session
            .document_for_view(view)
            .ok_or_else(|| io::Error::other("Editor view is detached"))?;
        if session.with_document(document, |state| state.read_only)? {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "This document is read-only",
            ));
        }
        let snapshot = session.snapshot(document)?;
        let state = session.view_snapshot(view)?;
        let revision = session.document_revision(document)?;
        let client = session
            .lsp()
            .and_then(|pool| pool.client_for_document(document))
            .filter(|client| client.borrow().is_initialized())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::Unsupported,
                    "This action requires an initialized language server",
                )
            })?;
        let origin = session
            .lsp()
            .and_then(|pool| pool.request_origin(document, view, revision.0, 0))
            .ok_or_else(|| io::Error::other("Language server is unavailable"))?;
        if matches!(command, "bed.editor.rename" | "bed.editor.quick_fix") {
            session.capture_lsp_workspace_revisions(&origin)?;
        }
        let uri = (if session
            .lsp()
            .is_some_and(|pool| pool.ssh_target().is_some())
        {
            LspUri::file_uri_from_remote_path(&snapshot.path)
        } else {
            LspUri::file_uri_from_path(&snapshot.path)
        })?
        .to_string();
        let line = session.with_document(document, |document| document.line(state.row))?;
        let position =
            json!({"line":state.row,"character":utf8_byte_offset_to_utf16(&line,state.column)});
        let mut pending = Pending {
            view,
            document,
            origin,
            revision,
            selection: state.selections.clone(),
            position: position.clone(),
            uri: uri.clone(),
            stage: Stage::Format,
            client,
            response: Arc::new(LspRequestState::new()),
            rpc: None,
            started: Instant::now(),
            command_generation: None,
        };
        match command {
            "bed.editor.format_document" => pending.send(
                "textDocument/formatting",
                json!({"textDocument":{"uri":uri},"options":{"tabSize":4,"insertSpaces":true}}),
                Stage::Format,
            )?,
            "bed.editor.rename" => {
                if pending
                    .client
                    .borrow()
                    .supports_method("textDocument/prepareRename")
                {
                    pending.send(
                        "textDocument/prepareRename",
                        json!({"textDocument":{"uri":uri},"position":position}),
                        Stage::PrepareRename,
                    )?;
                } else {
                    if !pending
                        .client
                        .borrow()
                        .supports_method("textDocument/rename")
                    {
                        return Err(io::Error::new(
                            io::ErrorKind::Unsupported,
                            "Language server does not support rename",
                        ));
                    }
                    pending.stage = Stage::RenameInput {
                        name: identifier_name(
                            &snapshot.bytes,
                            session.with_document(document, |s| {
                                s.offset_from_row_col(state.row, state.column)
                            })?,
                        ),
                        opened: false,
                    };
                }
            }
            "bed.editor.quick_fix" => {
                let (ar, ac, br, bc) = state.selections[state.primary_index].ordered();
                let start_line = session.with_document(document, |s| s.line(ar))?;
                let end_line = session.with_document(document, |s| s.line(br))?;
                let mut start = (ar, utf8_byte_offset_to_utf16(&start_line, ac));
                let mut end = (br, utf8_byte_offset_to_utf16(&end_line, bc));
                let diagnostics = pending
                    .client
                    .borrow()
                    .diagnostics()
                    .raw_for_document(&snapshot.path);
                let diagnostics = diagnostics
                    .into_iter()
                    .filter(|d| {
                        let begin = (
                            d["range"]["start"]["line"].as_i64().unwrap_or(-1) as i32,
                            d["range"]["start"]["character"].as_i64().unwrap_or(-1) as i32,
                        );
                        let finish = (
                            d["range"]["end"]["line"].as_i64().unwrap_or(-1) as i32,
                            d["range"]["end"]["character"].as_i64().unwrap_or(-1) as i32,
                        );
                        begin <= end && finish >= start
                    })
                    .collect::<Vec<_>>();
                if start == end
                    && let Some(diagnostic) = diagnostics.first()
                {
                    start = (
                        diagnostic["range"]["start"]["line"]
                            .as_i64()
                            .unwrap_or(start.0 as i64) as i32,
                        diagnostic["range"]["start"]["character"]
                            .as_i64()
                            .unwrap_or(start.1 as i64) as i32,
                    );
                    end = (
                        diagnostic["range"]["end"]["line"]
                            .as_i64()
                            .unwrap_or(end.0 as i64) as i32,
                        diagnostic["range"]["end"]["character"]
                            .as_i64()
                            .unwrap_or(end.1 as i64) as i32,
                    );
                }
                pending.send("textDocument/codeAction",json!({"textDocument":{"uri":uri},"range":{"start":{"line":start.0,"character":start.1},"end":{"line":end.0,"character":end.1}},"context":{"diagnostics":diagnostics,"only":["quickfix"],"triggerKind":1}}),Stage::QuickFix)?;
            }
            _ => return Err(io::Error::other("Unknown language action")),
        }
        self.pending = Some(pending);
        Ok(())
    }

    pub fn execute_completion_command(
        &mut self,
        session: &mut DocumentSession,
        view: ViewId,
        command: Value,
    ) -> io::Result<()> {
        self.cancel();
        for generation in std::mem::take(&mut self.finished_commands) {
            session.finish_lsp_command(generation);
        }
        let document = session
            .document_for_view(view)
            .ok_or_else(|| io::Error::other("Completion view is detached"))?;
        let revision = session.document_revision(document)?;
        let client = session
            .lsp()
            .and_then(|pool| pool.client_for_document(document))
            .ok_or_else(|| io::Error::other("Language server is unavailable"))?;
        let origin = session
            .lsp()
            .and_then(|pool| pool.request_origin(document, view, revision.0, 0))
            .ok_or_else(|| io::Error::other("Language server is unavailable"))?;
        let mut pending = Pending {
            view,
            document,
            origin,
            revision,
            selection: session.view_snapshot(view)?.selections,
            position: Value::Null,
            uri: String::new(),
            stage: Stage::Command,
            client,
            response: Arc::new(LspRequestState::new()),
            rpc: None,
            started: Instant::now(),
            command_generation: Some(origin.server_generation),
        };
        session.begin_lsp_command(&origin, true)?;
        if let Err(error) = pending.send("workspace/executeCommand",json!({"command":command["command"],"arguments":command.get("arguments").cloned().unwrap_or_else(|| json!([]))}),Stage::Command) {
            session.finish_lsp_command(origin.server_generation); return Err(error);
        }
        self.pending = Some(pending);
        Ok(())
    }

    pub fn tick(
        &mut self,
        session: &mut DocumentSession,
        requests: &mut Vec<HostAction>,
    ) -> io::Result<()> {
        for generation in std::mem::take(&mut self.finished_commands) {
            session.finish_lsp_command(generation);
        }
        let Some(mut pending) = self.pending.take() else {
            return Ok(());
        };
        let active_application = matches!(&pending.stage,Stage::Apply{action} if action.get("edit").is_some_and(|edit| session.lsp_workspace_edit_is_pending(edit,Some(&pending.origin))));
        let current = if active_application || matches!(pending.stage, Stage::Command) {
            pending.lifetime_current(session)
        } else {
            pending.current(session)
        };
        if !current {
            pending.cancel();
            if let Some(generation) = pending.command_generation {
                session.finish_lsp_command(generation);
            }
            notify(
                requests,
                "Language action cancelled because its document or selection changed",
            );
            return Ok(());
        }
        if pending.rpc.is_some()
            && pending.started.elapsed() >= pending.client.borrow().request_timeout()
        {
            pending.cancel();
            if let Some(generation) = pending.command_generation {
                session.finish_lsp_command(generation);
            }
            notify(requests, "Language action timed out");
            return Ok(());
        }
        if pending.rpc.is_some() && !pending.response.is_pending() {
            pending.rpc = None;
            let result = pending
                .response
                .snapshot()
                .unwrap_or_else(|| Err("Language server returned no response".into()));
            let value = match result {
                Ok(value) => value,
                Err(error) => {
                    if let Some(generation) = pending.command_generation {
                        session.finish_lsp_command(generation);
                    }
                    notify(requests, error);
                    return Ok(());
                }
            };
            let result = match &pending.stage {
                Stage::Format => {
                    if let Err(error) =
                        session.apply_lsp_text_edits(pending.document, pending.revision, &value)
                    {
                        notify(requests, error.to_string());
                    }
                    return Ok(());
                }
                Stage::PrepareRename => {
                    let bytes = session.snapshot(pending.document)?.bytes;
                    prepare_name(&value, &bytes).map(|name| {
                        pending.stage = Stage::RenameInput {
                            name,
                            opened: false,
                        }
                    })
                }
                Stage::Rename => {
                    pending.stage = Stage::Apply {
                        action: json!({"edit":value}),
                    };
                    Ok(())
                }
                Stage::QuickFix => quick_fixes(value).and_then(|items| {
                    if items.is_empty() {
                        return Err(io::Error::other("No quick fixes available"));
                    }
                    pending.stage = Stage::Picker {
                        items,
                        selected: 0,
                        opened: false,
                    };
                    Ok(())
                }),
                Stage::ResolveAction { original } => {
                    let mut action = original.clone();
                    if let (Some(target), Some(source)) =
                        (action.as_object_mut(), value.as_object())
                    {
                        target.extend(source.clone());
                    }
                    pending.stage = Stage::Apply { action };
                    Ok(())
                }
                Stage::Command => {
                    session.finish_lsp_command(pending.origin.server_generation);
                    return Ok(());
                }
                _ => Err(io::Error::other("Unexpected language action response")),
            };
            if let Err(error) = result {
                notify(requests, error.to_string());
                return Ok(());
            }
        }
        if let Stage::Apply { action } = &pending.stage {
            let action = action.clone();
            if let Some(edit) = action.get("edit").filter(|edit| !edit.is_null()) {
                match session.apply_lsp_workspace_edit(edit, Some(&pending.origin)) {
                    Ok(_) => pending.refresh_origin(session)?,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        self.pending = Some(pending);
                        return Ok(());
                    }
                    Err(error) => {
                        notify(requests, error.to_string());
                        return Ok(());
                    }
                }
            }
            if let Some(command) = command_value(&action) {
                session.begin_lsp_command(
                    &pending.origin,
                    action.get("edit").is_some_and(|edit| !edit.is_null()),
                )?;
                pending.command_generation = Some(pending.origin.server_generation);
                if let Err(error) = pending.send("workspace/executeCommand",json!({"command":command["command"],"arguments":command.get("arguments").cloned().unwrap_or_else(|| json!([]))}),Stage::Command) {
                    session.finish_lsp_command(pending.origin.server_generation); notify(requests,error.to_string()); return Ok(());
                }
            } else {
                return Ok(());
            }
        }
        self.pending = Some(pending);
        Ok(())
    }

    pub fn render(
        &mut self,
        ui: &Ui,
        view: &EditorView,
        session: &mut DocumentSession,
        requests: &mut Vec<HostAction>,
    ) -> io::Result<()> {
        self.tick(session, requests)?;
        let Some(pending) = &mut self.pending else {
            return Ok(());
        };
        if pending.view != view.id() {
            return Ok(());
        }
        let popup = format!("##language_action_{}", view.id().0);
        let mut close = false;
        let mut rename = None;
        let mut chosen = None;
        match &mut pending.stage {
            Stage::RenameInput { name, opened } => {
                if !*opened {
                    ui.open_popup(&popup);
                    *opened = true;
                }
                if let Some(_popup) = ui.begin_popup(&popup) {
                    ui.text("Rename Symbol");
                    if ui.is_window_appearing() {
                        ui.set_keyboard_focus_here();
                    }
                    if ui
                        .input_text("##new_symbol_name", name)
                        .enter_returns_true(true)
                        .auto_select_all(true)
                        .build()
                        && !name.trim().is_empty()
                    {
                        rename = Some(name.clone());
                        ui.close_current_popup();
                    }
                    if ui.is_key_pressed(Key::Escape) {
                        close = true;
                        ui.close_current_popup();
                    }
                } else {
                    close = true;
                }
            }
            Stage::Picker {
                items,
                selected,
                opened,
            } => {
                if !*opened {
                    ui.open_popup(&popup);
                    *opened = true;
                }
                if let Some(_popup) = ui.begin_popup(&popup) {
                    ui.text("Quick Fix");
                    ui.separator();
                    if ui.is_key_pressed(Key::UpArrow) {
                        *selected = (*selected + items.len() - 1) % items.len();
                    }
                    if ui.is_key_pressed(Key::DownArrow) {
                        *selected = (*selected + 1) % items.len();
                    }
                    for (index, item) in items.iter().enumerate() {
                        let disabled = item["disabled"]["reason"].as_str();
                        let label = format!(
                            "{}##quick_fix_{index}",
                            item["title"].as_str().unwrap_or("Quick Fix")
                        );
                        let _disabled = ui.begin_disabled_with_cond(disabled.is_some());
                        if ui
                            .selectable_config(&label)
                            .selected(index == *selected)
                            .build()
                            && disabled.is_none()
                        {
                            chosen = Some(item.clone());
                        }
                        drop(_disabled);
                        if let Some(reason) = disabled {
                            ui.text_disabled(reason);
                        }
                    }
                    if (ui.is_key_pressed(Key::Enter) || ui.is_key_pressed(Key::KeypadEnter))
                        && !items[*selected]["disabled"].is_object()
                    {
                        chosen = Some(items[*selected].clone());
                    }
                    if chosen.is_some() {
                        ui.close_current_popup();
                    }
                    if ui.is_key_pressed(Key::Escape) {
                        close = true;
                        ui.close_current_popup();
                    }
                } else {
                    close = true;
                }
            }
            _ => {}
        }
        if let Some(name) = rename {
            pending.send("textDocument/rename",json!({"textDocument":{"uri":pending.uri},"position":pending.position,"newName":name}),Stage::Rename)?;
        }
        if let Some(action) = chosen {
            if !action["command"].is_string()
                && pending.client.borrow().server_capabilities()["codeActionProvider"]["resolveProvider"]
                    == true
            {
                pending.send(
                    "codeAction/resolve",
                    action.clone(),
                    Stage::ResolveAction { original: action },
                )?;
            } else {
                pending.stage = Stage::Apply { action };
            }
        }
        if close {
            self.cancel();
        }
        self.tick(session, requests)
    }
}

fn identifier_name(bytes: &[u8], caret: usize) -> String {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return String::new();
    };
    let start = text[..caret]
        .char_indices()
        .rev()
        .find(|(_, c)| !(*c == '_' || c.is_alphanumeric()))
        .map_or(0, |(i, c)| i + c.len_utf8());
    let end = text[caret..]
        .char_indices()
        .find(|(_, c)| !(*c == '_' || c.is_alphanumeric()))
        .map_or(text.len(), |(i, _)| caret + i);
    text[start..end].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quick_fix_filters_refactors_and_prefers_server_preferred_items() {
        let fixes = quick_fixes(json!([
            {"title":"refactor","kind":"refactor.extract"},
            {"title":"disabled","kind":"quickfix","disabled":{"reason":"Cannot apply"}},
            {"title":"normal","kind":"quickfix"},
            {"title":"preferred","kind":"quickfix","isPreferred":true},
            {"title":"command","command":"fix"}
        ]))
        .unwrap();
        assert_eq!(fixes.len(), 4);
        assert_eq!(fixes[0]["title"], "preferred");
        assert_eq!(command_value(&fixes[2]).unwrap()["command"], "fix");
    }

    #[test]
    fn rename_preparation_validates_utf16_range() {
        let bytes = "let café😀".as_bytes();
        assert_eq!(
            prepare_name(
                &json!({"range":{"start":{"line":0,"character":4},"end":{"line":0,"character":8}}}),
                bytes
            )
            .unwrap(),
            "café"
        );
        assert!(prepare_name(&Value::Null, bytes).is_err());
        assert_eq!(identifier_name("let café = 1".as_bytes(), 9), "café");
    }

    fn pump(
        actions: &mut EditorActions,
        session: &mut DocumentSession,
        predicate: impl Fn(&EditorActions) -> bool,
    ) {
        let until = Instant::now() + std::time::Duration::from_secs(5);
        loop {
            session.tick();
            actions.tick(session, &mut Vec::new()).unwrap();
            if predicate(actions) {
                return;
            }
            assert!(Instant::now() < until, "Language action fixture timed out");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn prepare_rename_lazy_quick_fix_and_format_use_real_transport() {
        let (_dir, mut session, document) = super::super::test_fixture::session();
        let view = EditorView::new(&mut session, document).unwrap();
        session
            .with_commands(view.id(), |c| {
                c.set_cursor(
                    0,
                    14,
                    false,
                    bed_editing::text_commands::CursorReveal::Ensure,
                )
            })
            .unwrap();
        let mut actions = EditorActions::default();
        actions
            .request(&mut session, view.id(), "bed.editor.rename")
            .unwrap();
        pump(&mut actions, &mut session, |actions| {
            matches!(
                actions.pending.as_ref().map(|p| &p.stage),
                Some(Stage::RenameInput { .. })
            )
        });
        let pending = actions.pending.as_mut().unwrap();
        assert!(matches!(&pending.stage,Stage::RenameInput{name,..} if name=="St"));
        pending.send("textDocument/rename",json!({"textDocument":{"uri":pending.uri},"position":pending.position,"newName":"Renamed"}),Stage::Rename).unwrap();
        pump(&mut actions, &mut session, |actions| {
            actions.pending.is_none()
        });
        assert_eq!(
            session.snapshot(document).unwrap().bytes,
            b"let value = Renamed"
        );
        session.undo_document(document).unwrap();
        session
            .with_commands(view.id(), |c| {
                c.set_cursor(
                    0,
                    14,
                    false,
                    bed_editing::text_commands::CursorReveal::Ensure,
                )
            })
            .unwrap();
        actions
            .request(&mut session, view.id(), "bed.editor.quick_fix")
            .unwrap();
        pump(&mut actions, &mut session, |actions| {
            matches!(
                actions.pending.as_ref().map(|p| &p.stage),
                Some(Stage::Picker { .. })
            )
        });
        let pending = actions.pending.as_mut().unwrap();
        let Stage::Picker {
            items, selected, ..
        } = &pending.stage
        else {
            unreachable!()
        };
        assert_eq!(*selected, 0);
        assert_eq!(items[0]["title"], "Fix value");
        assert_eq!(items[1]["disabled"]["reason"], "Unavailable here");
        let action = items[0].clone();
        pending
            .send(
                "codeAction/resolve",
                action.clone(),
                Stage::ResolveAction { original: action },
            )
            .unwrap();
        pump(&mut actions, &mut session, |actions| {
            actions.pending.is_none()
        });
        assert_eq!(
            session.snapshot(document).unwrap().bytes,
            b"let value = Fixed"
        );
        session.undo_document(document).unwrap();
        session
            .with_commands(view.id(), |c| {
                c.set_cursor(
                    0,
                    14,
                    false,
                    bed_editing::text_commands::CursorReveal::Ensure,
                )
            })
            .unwrap();
        actions
            .request(&mut session, view.id(), "bed.editor.format_document")
            .unwrap();
        pump(&mut actions, &mut session, |actions| {
            actions.pending.is_none()
        });
        assert_eq!(
            session.snapshot(document).unwrap().bytes,
            b"let value = St;"
        );
        session.undo_document(document).unwrap();
        assert_eq!(session.snapshot(document).unwrap().bytes, b"let value = St");
    }

    #[test]
    fn manual_action_replies_cannot_mutate_changed_documents_or_selections() {
        let (_dir, mut session, document) = super::super::test_fixture::session();
        let view = EditorView::new(&mut session, document).unwrap();
        let mut actions = EditorActions::default();
        actions
            .request(&mut session, view.id(), "bed.editor.format_document")
            .unwrap();
        session
            .with_commands(view.id(), |c| c.type_text(b"x"))
            .unwrap();
        let mut requests = Vec::new();
        actions.tick(&mut session, &mut requests).unwrap();
        assert!(actions.pending.is_none());
        assert!(!requests.is_empty());
        assert_eq!(
            session.snapshot(document).unwrap().bytes,
            b"xlet value = St"
        );
        session.undo_document(document).unwrap();
        for command in ["bed.editor.rename", "bed.editor.quick_fix"] {
            session
                .with_commands(view.id(), |c| {
                    c.set_cursor(
                        0,
                        14,
                        false,
                        bed_editing::text_commands::CursorReveal::Ensure,
                    )
                })
                .unwrap();
            actions.request(&mut session, view.id(), command).unwrap();
            session
                .with_commands(view.id(), |c| {
                    c.set_cursor(
                        0,
                        0,
                        false,
                        bed_editing::text_commands::CursorReveal::Ensure,
                    )
                })
                .unwrap();
            actions.tick(&mut session, &mut Vec::new()).unwrap();
            assert!(actions.pending.is_none());
        }
        std::thread::sleep(std::time::Duration::from_millis(60));
        session.tick();
        assert_eq!(session.snapshot(document).unwrap().bytes, b"let value = St");
    }

    #[test]
    fn native_rename_popup_owns_typing_and_enter_and_quick_fix_picker_owns_keys() {
        use dear_imgui_rs::{Condition, Context, FramePrepareOptions, Key};
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let (_dir, mut session, document) = super::super::test_fixture::session();
        let mut view = EditorView::new(&mut session, document).unwrap();
        session
            .with_commands(view.id(), |c| {
                c.set_cursor(
                    0,
                    14,
                    false,
                    bed_editing::text_commands::CursorReveal::Ensure,
                )
            })
            .unwrap();
        let mut runtime = crate::EditorRuntime::default();
        let config = crate::EditorConfig::default();
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
                    runtime: &mut crate::EditorRuntime,
                    session: &mut DocumentSession,
                    view: &mut EditorView| {
            session.tick();
            let mut requests = Vec::new();
            runtime.tick(session, &mut requests).unwrap();
            context.prepare_frame(FramePrepareOptions::new([1000.0, 700.0], 1.0 / 60.0));
            let ui = context.frame();
            ui.window("actions_host")
                .size([850.0, 550.0], Condition::Always)
                .build(|| {
                    crate::feature::draw_text_view(
                        ui,
                        view,
                        runtime,
                        session,
                        &config,
                        &mut requests,
                    )
                    .unwrap();
                });
            drop(context.render_legacy());
            assert!(requests.is_empty(), "Unexpected action error: {requests:?}");
        };
        session.request_focus(view.id()).unwrap();
        draw(&mut context, &mut runtime, &mut session, &mut view);
        runtime
            .request_lsp_command(&mut session, view.id(), "bed.editor.rename")
            .unwrap();
        let until = Instant::now() + std::time::Duration::from_secs(5);
        while !runtime.language_action_blocks_view(view.id()) {
            draw(&mut context, &mut runtime, &mut session, &mut view);
            assert!(Instant::now() < until);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        draw(&mut context, &mut runtime, &mut session, &mut view);
        context.io_mut().add_input_characters_utf8("Renamed");
        draw(&mut context, &mut runtime, &mut session, &mut view);
        assert_eq!(session.snapshot(document).unwrap().bytes, b"let value = St");
        context.io_mut().add_key_event(Key::Enter, true);
        draw(&mut context, &mut runtime, &mut session, &mut view);
        context.io_mut().add_key_event(Key::Enter, false);
        let until = Instant::now() + std::time::Duration::from_secs(5);
        while session.snapshot(document).unwrap().bytes != b"let value = Renamed" {
            draw(&mut context, &mut runtime, &mut session, &mut view);
            assert!(Instant::now() < until, "Rename UI did not submit");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        session.undo_document(document).unwrap();
        session
            .with_commands(view.id(), |c| {
                c.set_cursor(
                    0,
                    14,
                    false,
                    bed_editing::text_commands::CursorReveal::Ensure,
                )
            })
            .unwrap();
        session.request_focus(view.id()).unwrap();
        draw(&mut context, &mut runtime, &mut session, &mut view);
        runtime
            .request_lsp_command(&mut session, view.id(), "bed.editor.quick_fix")
            .unwrap();
        let until = Instant::now() + std::time::Duration::from_secs(5);
        while !runtime.language_action_blocks_view(view.id()) {
            draw(&mut context, &mut runtime, &mut session, &mut view);
            assert!(Instant::now() < until);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        draw(&mut context, &mut runtime, &mut session, &mut view);
        context.io_mut().add_key_event(Key::Enter, true);
        draw(&mut context, &mut runtime, &mut session, &mut view);
        context.io_mut().add_key_event(Key::Enter, false);
        let until = Instant::now() + std::time::Duration::from_secs(5);
        while session.snapshot(document).unwrap().bytes != b"let value = Fixed" {
            draw(&mut context, &mut runtime, &mut session, &mut view);
            assert!(Instant::now() < until, "Quick fix UI did not apply");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        session.undo_document(document).unwrap();
        assert_eq!(session.snapshot(document).unwrap().bytes, b"let value = St");
    }
}
