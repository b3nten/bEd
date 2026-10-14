//! Translated from ned lsp/lsp_goto.{h,cpp}; see LICENSE and NOTICE.
use bedtime_documents::document_runtime::DocumentRuntime;
use bedtime_editing::util::utf8::utf8_byte_offset_to_utf16;
use bedtime_lsp::{
    jsonrpc::{ResponseError, RpcId},
    lsp_client::LspClient,
    lsp_locations::{LspLocation, from_definition_result, from_references_result},
    lsp_request::LspRequestState,
    lsp_uri::LspUri,
    workspace_lsp::LspRequestOrigin,
};
use serde_json::json;
use std::{sync::Arc, time::Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Definition,
    References,
}

#[derive(Clone, Debug, PartialEq)]
pub enum NavigationError {
    Rpc(ResponseError),
    InvalidResult(String),
    Send(String),
}

#[derive(Clone)]
struct NavigationReply {
    result: Result<Vec<LspLocation>, NavigationError>,
    received: Instant,
}

impl NavigationError {
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Rpc(error) if matches!(error.code, -32801 | -32802))
    }

    pub fn message(&self) -> String {
        match self {
            Self::Rpc(error) => error.to_string(),
            Self::InvalidResult(message) | Self::Send(message) => message.clone(),
        }
    }
}

pub struct LspGoto {
    pub(crate) kind: Kind,
    pub(crate) show: bool,
    state: Arc<LspRequestState<NavigationReply>>,
    rpc_id: Option<RpcId>,
}

impl LspGoto {
    pub fn new(kind: Kind) -> Self {
        Self {
            kind,
            show: false,
            state: Arc::new(LspRequestState::new()),
            rpc_id: None,
        }
    }

    pub fn is_visible(&self) -> bool {
        self.show
    }

    pub fn cancel(&mut self) {
        self.show = false;
        self.state.cancel();
        self.rpc_id = None;
    }

    pub fn title(&self) -> &'static str {
        match self.kind {
            Kind::Definition => "Goto Definition",
            Kind::References => "Goto Reference",
        }
    }

    pub fn snapshot(&self) -> Vec<LspLocation> {
        self.state
            .snapshot()
            .and_then(|reply| reply.result.ok())
            .unwrap_or_default()
    }

    pub fn error(&self) -> Option<NavigationError> {
        self.state.snapshot().and_then(|reply| reply.result.err())
    }

    pub fn received_at(&self) -> Option<Instant> {
        self.state.snapshot().map(|reply| reply.received)
    }

    pub fn rpc_id(&self) -> Option<RpcId> {
        self.rpc_id.clone()
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

    pub fn get(&mut self, client: &mut LspClient, editor: &DocumentRuntime) {
        self.get_with_optional_origin(client, editor, None, (editor.view.row, editor.view.column));
    }

    pub fn get_with_origin(
        &mut self,
        client: &mut LspClient,
        editor: &DocumentRuntime,
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
        editor: &DocumentRuntime,
        origin: LspRequestOrigin,
        row: i32,
        column: i32,
    ) {
        self.get_with_optional_origin(client, editor, Some(origin), (row, column));
    }

    fn get_with_optional_origin(
        &mut self,
        client: &mut LspClient,
        editor: &DocumentRuntime,
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
                let locations = result.map_err(NavigationError::Rpc).and_then(|value| {
                    match kind {
                        Kind::Definition => from_definition_result(&value),
                        Kind::References => from_references_result(&value),
                    }.map_err(|error| NavigationError::InvalidResult(error.to_string()))
                });
                let reply = Some(NavigationReply { result: locations, received: Instant::now() });
                if let Some(origin) = origin { state.deliver_for(origin, reply); } else { state.deliver(ticket, reply); }
            })
        });
        match result {
            Ok(id) => self.rpc_id = Some(id),
            Err(error) => {
                self.rpc_id = None;
                let result = Some(NavigationReply {
                    result: Err(NavigationError::Send(error.to_string())),
                    received: Instant::now(),
                });
                if let Some(origin) = origin {
                    self.state.deliver_for(origin, result);
                } else {
                    self.state.deliver(ticket, result);
                }
            }
        }
    }
}
