//! Translated from ned lsp/lsp_goto.{h,cpp}; see LICENSE and NOTICE.
use bed_document_session::editor::Editor;
use bed_editing::util::utf8::utf8_byte_offset_to_utf16;
use bed_lsp::{
    lsp_client::LspClient,
    lsp_locations::{LspLocation, from_definition_result, from_references_result},
    lsp_request::LspRequestState,
    lsp_uri::LspUri,
    workspace_lsp::LspRequestOrigin,
};
use serde_json::json;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Definition,
    References,
}
pub struct LspGoto {
    pub(crate) kind: Kind,
    pub(crate) show: bool,
    state: Arc<LspRequestState<Vec<LspLocation>>>,
}
impl LspGoto {
    pub fn new(kind: Kind) -> Self {
        Self {
            kind,
            show: false,
            state: Arc::new(LspRequestState::new()),
        }
    }
    pub fn is_visible(&self) -> bool {
        self.show
    }
    pub fn cancel(&mut self) {
        self.show = false;
        self.state.cancel();
    }
    pub fn title(&self) -> &'static str {
        match self.kind {
            Kind::Definition => "Goto Definition",
            Kind::References => "Goto Reference",
        }
    }
    pub fn snapshot(&self) -> Vec<LspLocation> {
        self.state.snapshot().unwrap_or_default()
    }
    pub fn is_pending(&self) -> bool {
        self.state.is_pending()
    }
    pub fn origin(&self) -> Option<LspRequestOrigin> {
        self.state.origin()
    }
    pub fn retain_request(&mut self, valid: &mut impl FnMut(&LspRequestOrigin) -> bool) {
        if self.state.origin().is_some_and(|origin| !valid(&origin)) {
            self.cancel();
        }
    }
    pub fn get(&mut self, client: &mut LspClient, editor: &Editor) {
        self.get_with_optional_origin(client, editor, None, (editor.view.row, editor.view.column));
    }
    pub fn get_with_origin(
        &mut self,
        client: &mut LspClient,
        editor: &Editor,
        origin: LspRequestOrigin,
    ) {
        self.get_with_optional_origin(
            client,
            editor,
            Some(origin),
            (editor.view.row, editor.view.column),
        );
    }
    pub fn get_with_origin_at_position(
        &mut self,
        client: &mut LspClient,
        editor: &Editor,
        origin: LspRequestOrigin,
        row: i32,
        column: i32,
    ) {
        self.get_with_optional_origin(client, editor, Some(origin), (row, column));
    }
    fn get_with_optional_origin(
        &mut self,
        client: &mut LspClient,
        editor: &Editor,
        origin: Option<LspRequestOrigin>,
        position: (i32, i32),
    ) {
        if !client.is_initialized() || !client.is_process_started() {
            return;
        }
        let (row, column) = position;
        let row = row.clamp(0, editor.state.line_count().saturating_sub(1).max(0));
        let utf16 = utf8_byte_offset_to_utf16(&editor.state.line(row), column);
        self.show = true;
        let origin = origin.map(|origin| self.state.begin_for(origin));
        let ticket = origin.map_or_else(|| self.state.begin(), |origin| origin.ticket);
        let kind = self.kind;
        let state = self.state.clone();
        let result = LspUri::file_uri_from_path(&editor.state.path).and_then(|uri| {
            let mut params = json!({"textDocument":{"uri":uri.to_string()},"position":{"line":row as u32,"character":utf16 as u32}});
            let method = match kind {
                Kind::Definition => "textDocument/definition",
                Kind::References => { params["context"] = json!({"includeDeclaration":false}); "textDocument/references" }
            };
            client.send_request(method, params, move |result| {
                let locations = result.ok().and_then(|value| match kind {
                    Kind::Definition => from_definition_result(&value).ok(),
                    Kind::References => from_references_result(&value).ok(),
                });
                if let Some(origin) = origin { state.deliver_for(origin, locations); } else { state.deliver(ticket, locations); }
            }).map(|_| ())
        });
        if let Err(error) = result {
            eprintln!("LSP: goto request failed: {error}");
            if let Some(origin) = origin {
                self.state.deliver_for(origin, None);
            } else {
                self.state.deliver(ticket, None);
            }
        }
    }
}
