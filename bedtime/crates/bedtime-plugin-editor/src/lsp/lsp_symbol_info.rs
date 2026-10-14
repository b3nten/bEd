//! Translated from ned lsp/lsp_symbol_info.{h,cpp}; see LICENSE and NOTICE.
use crate::lsp::lsp_ui::LspView;
use bedtime_documents::document_runtime::DocumentRuntime;
use bedtime_documents::{DocumentSession, ViewId};
use bedtime_editing::util::utf8::utf8_byte_offset_to_utf16;
use bedtime_editor_widgets::views::{
    hover_tooltip::{
        HoverRect, hover_key_pressed, hover_rect_contains, render_hover_markdown,
        render_hover_plaintext, render_hover_popup,
    },
    hover_trigger::Zone,
    view_layout::line_column_x,
};
use bedtime_lsp::{
    lsp_client::LspClient, lsp_request::LspRequestState, lsp_uri::LspUri,
    workspace_lsp::LspRequestOrigin,
};
use dear_imgui_rs::{MouseButton, PopupQueryFlags, Ui};
use serde_json::{Value, json};
use std::{io, sync::Arc};

fn invalid_hover() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "Invalid LSP hover contents")
}

fn fence_wrap(language: &str, body: &str) -> String {
    if body.is_empty() {
        String::new()
    } else {
        format!("```{language}\n{body}\n```")
    }
}

fn marked_string_text(value: &Value) -> io::Result<String> {
    if let Some(text) = value.as_str() {
        return Ok(text.to_owned());
    }
    let language = value
        .get("language")
        .and_then(Value::as_str)
        .ok_or_else(invalid_hover)?;
    let text = value
        .get("value")
        .and_then(Value::as_str)
        .ok_or_else(invalid_hover)?;
    Ok(fence_wrap(language, text))
}

pub fn format_hover_contents(contents: &Value, _fallback_language: &str) -> io::Result<String> {
    if let Some(kind) = contents.get("kind") {
        kind.as_str().ok_or_else(invalid_hover)?;
        let text = contents
            .get("value")
            .and_then(Value::as_str)
            .ok_or_else(invalid_hover)?;
        return Ok(text.to_owned());
    }
    if let Some(parts) = contents.as_array() {
        let parts = parts
            .iter()
            .map(marked_string_text)
            .collect::<io::Result<Vec<_>>>()?;
        return Ok(parts
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("\n"));
    }
    marked_string_text(contents)
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum HoverContents {
    Markdown(String),
    Plaintext(String),
}

impl HoverContents {
    fn decode(contents: &Value, fallback_language: &str) -> io::Result<Self> {
        let text = format_hover_contents(contents, fallback_language)?;
        Ok(
            if contents.get("kind").and_then(Value::as_str) == Some("plaintext") {
                Self::Plaintext(text)
            } else {
                Self::Markdown(text)
            },
        )
    }

    fn text(&self) -> &str {
        match self {
            Self::Markdown(text) | Self::Plaintext(text) => text,
        }
    }
}

/// Each editor frontend owns its clicked symbol independently of transient hovers.
#[derive(bevy::prelude::Component, Default)]
pub(crate) struct LspSymbolDocumentation {
    state: Arc<LspRequestState<HoverContents>>,
    language: String,
    colors: bedtime_highlight::tree_sitter::ThemeColors,
}

impl LspSymbolDocumentation {
    #[cfg(test)]
    pub(crate) fn pending(&self) -> bool {
        self.state.is_pending()
    }

    pub(crate) fn request(
        &mut self,
        session: &mut DocumentSession,
        view: ViewId,
        row: i32,
        column: i32,
    ) -> io::Result<()> {
        self.state.cancel();
        let document = session
            .document_for_view(view)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Editor view is detached"))?;
        let snapshot = session.snapshot(document)?;
        (self.language, self.colors) = session.with_view(view, |editor| {
            (
                editor.state.language_id.clone(),
                bedtime_editor_widgets::views::hover_tooltip::documentation_colors(editor),
            )
        })?;
        let Some(pool) = session.lsp() else {
            return Ok(());
        };
        let Some(origin) = pool.request_origin(document, view, snapshot.generation, 0) else {
            return Ok(());
        };
        let Some(client) = pool.client_for_document(document) else {
            return Ok(());
        };
        let mut client = client.borrow_mut();
        if !client.is_initialized() || !client.supports_method("textDocument/hover") {
            return Ok(());
        }
        session.with_view(view, |editor| {
            request_hover(&mut client, editor, row, column, Some(origin), &self.state);
        })
    }

    pub(crate) fn snapshot(
        &self,
        session: &DocumentSession,
    ) -> Option<bedtime_ui::documentation::Documentation> {
        let origin = self.state.origin()?;
        if !session.accepts_lsp_origin(&origin) {
            return None;
        }
        let contents = self.state.snapshot_for(&origin)?;
        Some(bedtime_ui::documentation::Documentation {
            text: contents.text().to_owned(),
            markdown: matches!(contents, HoverContents::Markdown(_)),
            language: self.language.clone(),
            colors: self.colors.clone(),
        })
        .filter(|docs| !docs.text.trim().is_empty())
    }
}

fn request_hover(
    client: &mut LspClient,
    editor: &DocumentRuntime,
    row: i32,
    byte_column: i32,
    origin: Option<LspRequestOrigin>,
    state: &Arc<LspRequestState<HoverContents>>,
) {
    let utf16 = utf8_byte_offset_to_utf16(&editor.state.line(row), byte_column);
    let origin = origin.map(|origin| state.begin_for(origin));
    let ticket = origin.map_or_else(|| state.begin(), |origin| origin.ticket);
    let language = editor.state.language_id.clone();
    let reply = state.clone();
    let result = LspUri::file_uri_from_path(&editor.state.path).and_then(|uri| {
        client.send_request("textDocument/hover",json!({"textDocument":{"uri":uri.to_string()},"position":{"line":row as u32,"character":utf16 as u32}}),move |result| {
            let text = result.ok().and_then(|result| result.get("contents").and_then(|contents| HoverContents::decode(contents, &language).ok())).filter(|text| !text.text().trim().is_empty());
            if let Some(origin) = origin { reply.deliver_for(origin, text); } else { reply.deliver(ticket, text); }
        }).map(|_| ())
    });
    if let Err(error) = result {
        eprintln!("LSP: hover request failed: {error}");
        if let Some(origin) = origin {
            state.deliver_for(origin, None);
        } else {
            state.deliver(ticket, None);
        }
    }
}

pub struct LspSymbolInfo {
    at_caret: bool,
    fresh_caret_request: bool,
    requested_for_cell: bool,
    hover_state: Arc<LspRequestState<HoverContents>>,
    hover_row: i32,
    hover_column: i32,
    popup_rect: Option<HoverRect>,
    popup_anchor: Option<[f32; 2]>,
    mouse_origin: Option<LspRequestOrigin>,
}

impl Default for LspSymbolInfo {
    fn default() -> Self {
        Self {
            at_caret: false,
            fresh_caret_request: false,
            requested_for_cell: false,
            hover_state: Arc::new(LspRequestState::new()),
            hover_row: -1,
            hover_column: -1,
            popup_rect: None,
            popup_anchor: None,
            mouse_origin: None,
        }
    }
}

impl LspSymbolInfo {
    pub fn at_caret(&self) -> bool {
        self.at_caret
    }

    pub fn origin(&self) -> Option<LspRequestOrigin> {
        self.hover_state.origin()
    }

    pub fn popup_contains(&self, mouse: [f32; 2]) -> bool {
        self.popup_rect
            .is_some_and(|rect| hover_rect_contains(rect, mouse))
    }

    pub fn get(&mut self, client: &mut LspClient, editor: &DocumentRuntime) {
        if !client.is_initialized() {
            return;
        }
        self.at_caret = true;
        self.fresh_caret_request = true;
        self.request_at(client, editor, editor.view.row, editor.view.column, None);
    }

    pub fn get_with_origin(
        &mut self,
        client: &mut LspClient,
        editor: &DocumentRuntime,
        origin: LspRequestOrigin,
    ) {
        if !client.is_initialized() {
            return;
        }
        self.at_caret = true;
        self.fresh_caret_request = true;
        self.request_at(
            client,
            editor,
            editor.view.row,
            editor.view.column,
            Some(origin),
        );
    }

    pub fn set_mouse_origin(&mut self, origin: Option<LspRequestOrigin>) {
        if self.mouse_origin != origin && !self.at_caret {
            self.hide_mouse_hover();
            self.requested_for_cell = false;
        }
        self.mouse_origin = origin;
    }

    pub fn retain_request(&mut self, valid: &mut impl FnMut(&LspRequestOrigin) -> bool) {
        if self
            .hover_state
            .origin()
            .is_some_and(|origin| !valid(&origin))
        {
            self.cancel();
        }
    }

    pub fn cancel(&mut self) {
        self.at_caret = false;
        self.fresh_caret_request = false;
        self.hide_mouse_hover();
    }

    fn hide_mouse_hover(&mut self) {
        if self.at_caret {
            return;
        }
        self.requested_for_cell = false;
        self.hover_row = -1;
        self.hover_column = -1;
        self.popup_rect = None;
        self.popup_anchor = None;
        self.hover_state.cancel();
    }

    fn request_at(
        &mut self,
        client: &mut LspClient,
        editor: &DocumentRuntime,
        row: i32,
        byte_column: i32,
        origin: Option<LspRequestOrigin>,
    ) {
        if !client.is_process_started() {
            return;
        }
        self.popup_rect = None;
        self.popup_anchor = None;
        self.requested_for_cell = true;
        request_hover(client, editor, row, byte_column, origin, &self.hover_state);
    }

    fn update_mouse_hover(
        &mut self,
        ui: &Ui,
        client: &mut LspClient,
        editor: &DocumentRuntime,
        view: &LspView<'_>,
    ) {
        // Menus take over from symbol hovers, including clicks inside the
        // tooltip's sticky bounds and replies arriving while a menu is open.
        if ui.is_mouse_clicked(MouseButton::Right)
            || ui.is_popup_open_with_flags("", PopupQueryFlags::ANY_POPUP)
        {
            self.cancel();
            return;
        }
        let over_popup = ui.is_mouse_pos_valid() && self.popup_contains(ui.io().mouse_pos());
        // Scrollbar drags and wheel input belong to the popup. Keyboard input,
        // clicks outside it, and scrolling the editor dismiss the hover.
        let dismissed = hover_key_pressed(ui) || (view.hover_dismissed && !over_popup);
        if self.at_caret {
            // The shortcut that opened this hover also dismisses existing
            // hovers in the editor frame. Accept it for this first frame.
            if std::mem::take(&mut self.fresh_caret_request) {
                return;
            }
            if dismissed {
                self.cancel();
            }
            return;
        }
        if dismissed {
            self.hide_mouse_hover();
            return;
        }
        if over_popup && (self.hover_state.is_pending() || self.hover_state.snapshot().is_some()) {
            return;
        }
        if !client.is_initialized() {
            return;
        }
        let info = view.hover_info;
        if !info.active
            || info.zone != Zone::Text
            || editor.state.path.is_empty()
            || !client.is_document_open(&editor.state.path)
        {
            self.hide_mouse_hover();
            return;
        }
        if info.row != self.hover_row || info.column != self.hover_column {
            self.hover_row = info.row;
            self.hover_column = info.column;
            self.requested_for_cell = false;
            self.popup_rect = None;
            self.popup_anchor = None;
            self.hover_state.cancel();
        }
        if !self.requested_for_cell {
            self.request_at(client, editor, info.row, info.column, self.mouse_origin);
        }
    }

    pub fn render(
        &mut self,
        ui: &Ui,
        client: &mut LspClient,
        editor: &DocumentRuntime,
        view: &LspView<'_>,
    ) {
        self.update_mouse_hover(ui, client, editor, view);
        let Some(contents) = self
            .hover_state
            .snapshot()
            .filter(|text| !text.text().is_empty())
        else {
            return;
        };
        if !view.tooltip_arbiter.claim(ui) {
            return;
        }
        let fs = ui.current_font_size();
        let caret = view.caret_position.unwrap_or_else(|| {
            [
                line_column_x(
                    ui,
                    &editor.state.line(editor.view.row),
                    editor.view.column,
                    view.layout.text_pos[0],
                )
                .floor(),
                view.layout.text_pos[1] + editor.view.row as f32 * view.layout.line_height,
            ]
        });
        let anchor = if self.at_caret {
            [
                caret[0] + fs * 0.25,
                caret[1] + view.layout.line_height + fs * 0.25,
            ]
        } else {
            *self.popup_anchor.get_or_insert_with(|| {
                let mouse = ui.io().mouse_pos();
                [mouse[0] + 8.0, mouse[1] + 12.0]
            })
        };
        self.popup_rect = render_hover_popup(
            ui,
            "##bed_lsp_hover",
            anchor,
            self.popup_rect,
            Some([caret[1], caret[1] + view.layout.line_height]),
            || match &contents {
                HoverContents::Markdown(markdown) => {
                    render_hover_markdown(ui, markdown, editor, &editor.state.language_id);
                }
                HoverContents::Plaintext(text) => render_hover_plaintext(ui, text, editor),
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clicked_symbol_replies_preserve_markup_and_reject_superseded_or_edited_requests() {
        use std::time::{Duration, Instant};
        let (_directory, mut session, document) =
            crate::lsp::test_fixture::session_mode("hover-delayed");
        let view = bedtime_editor_widgets::EditorView::new(&mut session, document).unwrap();
        let sibling = bedtime_editor_widgets::EditorView::new(&mut session, document).unwrap();
        let mut docs = LspSymbolDocumentation::default();
        let mut sibling_docs = LspSymbolDocumentation::default();
        docs.request(&mut session, view.id(), 0, 12).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while docs.snapshot(&session).is_none() {
            session.tick();
            assert!(Instant::now() < deadline, "hover did not arrive");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            docs.snapshot(&session).unwrap().text,
            "```rust\npub fn St()\n```\n\nA **Unicode** `string`."
        );
        // Moving focus into Docs must leave the selected symbol readable.
        session.request_focus(sibling.id()).unwrap();
        assert!(docs.snapshot(&session).is_some());
        sibling_docs
            .request(&mut session, sibling.id(), 0, 12)
            .unwrap();
        docs.request(&mut session, view.id(), 0, 12).unwrap();
        docs.request(&mut session, view.id(), 0, 13).unwrap();
        assert!(
            docs.snapshot(&session).is_none(),
            "the new request has no reply yet"
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while docs.state.is_pending() {
            session.tick();
            if let Some(text) = docs.snapshot(&session) {
                assert_eq!(text.text, "Literal **text** with `punctuation`.");
                assert!(!text.markdown);
            }
            assert!(Instant::now() < deadline, "latest hover did not arrive");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            docs.snapshot(&session).unwrap().text,
            "Literal **text** with `punctuation`."
        );
        assert_eq!(
            sibling_docs.snapshot(&session).unwrap().text,
            "```rust\npub fn St()\n```\n\nA **Unicode** `string`."
        );
        docs.request(&mut session, view.id(), 0, 0).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while docs.state.is_pending() {
            session.tick();
            assert!(Instant::now() < deadline, "empty hover did not arrive");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(docs.snapshot(&session).is_none());
        docs.request(&mut session, view.id(), 0, 12).unwrap();
        let revision = session.document_revision(document).unwrap();
        session
            .apply_edits(
                document,
                revision,
                &[bedtime_documents::ByteEdit {
                    range: 0..0,
                    bytes: b"x".to_vec(),
                }],
            )
            .unwrap();
        assert!(
            docs.snapshot(&session).is_none(),
            "edits invalidate pending docs"
        );
        let deadline = Instant::now() + Duration::from_millis(200);
        while Instant::now() < deadline {
            session.tick();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            docs.snapshot(&session).is_none(),
            "late reply cannot restore edited docs"
        );
        assert!(sibling_docs.snapshot(&session).is_none());
    }
    use bedtime_editor_widgets::views::{
        hover_tooltip::TooltipArbiter, hover_trigger::Info, view_layout::ViewLayout,
    };
    use dear_imgui_rs::{Condition, Context, FramePrepareOptions, Key, MouseButton};

    fn symbol_frame(
        context: &mut Context,
        symbol: &mut LspSymbolInfo,
        editor: &DocumentRuntime,
        client: &mut LspClient,
        dismissed: bool,
    ) {
        context.prepare_frame(FramePrepareOptions::new([800.0, 600.0], 1.0 / 60.0));
        let ui = context.frame();
        ui.window("host")
            .size([700.0, 500.0], Condition::Always)
            .build(|| {
                symbol.render(
                    ui,
                    client,
                    editor,
                    &LspView {
                        layout: &ViewLayout::default(),
                        hover_info: Info::default(),
                        hover_dismissed: dismissed,
                        tooltip_arbiter: &TooltipArbiter::default(),
                        caret_position: None,
                    },
                );
            });
        drop(context.render_legacy());
    }

    #[test]
    fn hover_shapes_preserve_literal_plaintext_and_single_array_boundaries() {
        assert_eq!(
            format_hover_contents(&json!({"kind":"plaintext","value":"int foo();"}), "cpp")
                .unwrap(),
            "int foo();"
        );
        assert_eq!(
            format_hover_contents(&json!({"kind":"markdown","value":"**hi**"}), "rust").unwrap(),
            "**hi**"
        );
        // Pinned Enumeration<string> retains custom values as raw markdown.
        assert_eq!(
            format_hover_contents(&json!({"kind":"custom","value":"**hi**"}), "rust").unwrap(),
            "**hi**"
        );
        assert_eq!(
            format_hover_contents(&json!({"language":"cpp","value":"int x;"}), "rust").unwrap(),
            "```cpp\nint x;\n```"
        );
        assert_eq!(
            format_hover_contents(
                &json!(["",{"language":"rust","value":"fn a() {}"},"detail"]),
                "rust"
            )
            .unwrap(),
            "```rust\nfn a() {}\n```\ndetail"
        );
        assert!(format_hover_contents(&json!([1]), "rust").is_err());
        assert_eq!(
            format_hover_contents(&json!({"kind":"plaintext","value":""}), "rust").unwrap(),
            ""
        );
        let literal = "**bold** `code` [link](https://example.org) ```rust";
        assert_eq!(
            HoverContents::decode(&json!({"kind":"plaintext","value":literal}), "rust").unwrap(),
            HoverContents::Plaintext(literal.into())
        );
    }

    #[test]
    fn native_rust_analyzer_plaintext_hover_wraps_its_signature_and_prose() {
        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = Context::create();
        context
            .set_ini_filename(None::<std::path::PathBuf>)
            .unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        let editor = DocumentRuntime::new();
        let mut client = LspClient::new("/bed-no-lsp-config.json");
        let mut symbol = LspSymbolInfo::default();
        // Captured before Markdown negotiation was enabled. Plaintext must not
        // become an unwrapped Rust fence or interpret literal markup.
        let contents = json!({"kind":"plaintext","value":"bed_hover_payload\n\npub fn documented_function_with_a_long_signature(name: &str, result: Result<Option<Vec<String>>, std::io::Error>, count: usize) -> usize\n\n\nA documented function with a long paragraph explaining how it handles a result, a collection of strings, and a caller supplied name while preserving all of the input data for later inspection."});
        let ticket = symbol.hover_state.begin();
        symbol.hover_state.deliver(
            ticket,
            Some(HoverContents::decode(&contents, "rust").unwrap()),
        );
        for _ in 0..4 {
            context.prepare_frame(FramePrepareOptions::new([240.0, 180.0], 1.0 / 60.0));
            let ui = context.frame();
            ui.window("plaintext hover fixture")
                .size([240.0, 180.0], Condition::Always)
                .build(|| {
                    symbol.render(
                        ui,
                        &mut client,
                        &editor,
                        &LspView {
                            layout: &ViewLayout::default(),
                            hover_info: Info::default(),
                            hover_dismissed: false,
                            tooltip_arbiter: &TooltipArbiter::default(),
                            caret_position: None,
                        },
                    );
                    ui.with_bound_context(|| unsafe {
                        let window =
                            dear_imgui_rs::sys::igFindWindowByName(c"##bed_lsp_hover".as_ptr());
                        assert!(!window.is_null());
                        assert_eq!((*window).ScrollMax.x, 0.0);
                    });
                    let (minimum, maximum) = symbol.popup_rect.unwrap();
                    assert!(minimum[0] >= 0.0 && minimum[1] >= 0.0);
                    assert!(maximum[0] <= 240.0 && maximum[1] <= 180.0);
                });
            drop(context.render_legacy());
        }
    }

    #[test]
    fn typed_hover_changes_document_identity_at_the_same_cell_and_rejects_old_replies() {
        use bedtime_editing::identity::{DocumentId, SessionId, ViewId};
        let mut symbol = LspSymbolInfo::default();
        let first = LspRequestOrigin {
            session_id: SessionId(1),
            document_id: DocumentId(2),
            view_id: ViewId(3),
            server_generation: 4,
            document_generation: 5,
            version: 6,
            ticket: 0,
        };
        symbol.set_mouse_origin(Some(first));
        symbol.requested_for_cell = true;
        symbol.hover_row = 0;
        symbol.hover_column = 0;
        let first = symbol.hover_state.begin_for(first);
        symbol.hover_state.deliver_for(
            first,
            Some(HoverContents::Markdown("first document".into())),
        );
        let second = LspRequestOrigin {
            document_id: DocumentId(7),
            view_id: ViewId(8),
            ..first
        };
        symbol.set_mouse_origin(Some(second));
        assert!(!symbol.requested_for_cell);
        assert!(symbol.hover_state.snapshot().is_none());
        let second = symbol.hover_state.begin_for(second);
        symbol.hover_state.deliver_for(
            first,
            Some(HoverContents::Markdown("late first document".into())),
        );
        assert!(symbol.hover_state.is_pending());
        symbol.hover_state.deliver_for(
            second,
            Some(HoverContents::Markdown("second document".into())),
        );
        assert_eq!(symbol.origin(), Some(second));
        assert_eq!(
            symbol.hover_state.snapshot(),
            Some(HoverContents::Markdown("second document".into()))
        );
        symbol.retain_request(&mut |origin| origin.document_id == DocumentId(2));
        assert!(symbol.origin().is_none());
        assert!(symbol.hover_state.snapshot().is_none());
    }

    #[test]
    fn native_hover_tooltip_owns_rect_and_cancellation_rejects_late_reply() {
        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = Context::create();
        context
            .set_ini_filename(None::<std::path::PathBuf>)
            .unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        let editor = DocumentRuntime::new();
        let mut client = LspClient::new("/bed-no-lsp-config.json");
        let mut symbol = LspSymbolInfo::default();
        let ticket = symbol.hover_state.begin();
        symbol
            .hover_state
            .deliver(ticket, Some(HoverContents::Markdown("a **symbol**".into())));
        let layout = ViewLayout {
            text_pos: [30.0, 40.0],
            line_height: 13.0,
            ..Default::default()
        };
        let arbiter = TooltipArbiter::default();
        for _ in 0..2 {
            context.prepare_frame(FramePrepareOptions::new([800.0, 600.0], 1.0 / 60.0));
            let ui = context.frame();
            ui.window("host")
                .size([700.0, 500.0], Condition::Always)
                .build(|| {
                    symbol.render(
                        ui,
                        &mut client,
                        &editor,
                        &LspView {
                            layout: &layout,
                            hover_info: Info::default(),
                            hover_dismissed: false,
                            tooltip_arbiter: &arbiter,
                            caret_position: None,
                        },
                    )
                });
            drop(context.render_legacy());
        }
        assert!(symbol.popup_rect.is_some());
        symbol.cancel();
        symbol
            .hover_state
            .deliver(ticket, Some(HoverContents::Markdown("late".into())));
        assert!(symbol.hover_state.snapshot().is_none());
        assert!(symbol.popup_rect.is_none());
        assert!(!symbol.hover_state.is_pending());
    }

    #[test]
    fn native_hover_dismisses_for_right_click_and_open_context_menu() {
        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        for at_caret in [false, true] {
            for right_click in [false, true] {
                let mut context = Context::create();
                context
                    .set_ini_filename(None::<std::path::PathBuf>)
                    .unwrap();
                context
                    .font_atlas()
                    .try_claim_legacy_renderer()
                    .unwrap()
                    .build();
                let editor = DocumentRuntime::new();
                let mut client = LspClient::new("/bed-no-lsp-config.json");
                let mut symbol = LspSymbolInfo {
                    at_caret,
                    ..Default::default()
                };
                let ticket = symbol.hover_state.begin();
                symbol.hover_state.deliver(
                    ticket,
                    Some(HoverContents::Markdown("Symbol documentation.".into())),
                );
                symbol_frame(&mut context, &mut symbol, &editor, &mut client, false);
                let (min, _) = symbol.popup_rect.expect("hover is visible before the menu");
                context.io_mut().add_mouse_pos_event(min);
                context
                    .io_mut()
                    .add_mouse_button_event(MouseButton::Right, right_click);
                // Keep the pointer in the sticky bounds on both the opening
                // frame and a later frame with the context menu still open.
                for frame in 0..2 {
                    context.prepare_frame(FramePrepareOptions::new([800.0, 600.0], 1.0 / 60.0));
                    let ui = context.frame();
                    ui.window("host")
                        .size([700.0, 500.0], Condition::Always)
                        .build(|| {
                            if !right_click {
                                if frame == 0 {
                                    ui.open_popup("context menu");
                                }
                                let _popup = ui.begin_popup("context menu").unwrap();
                                ui.text("Menu action");
                            }
                            symbol.render(
                                ui,
                                &mut client,
                                &editor,
                                &LspView {
                                    layout: &ViewLayout::default(),
                                    hover_info: Info::default(),
                                    hover_dismissed: right_click && frame == 0,
                                    tooltip_arbiter: &TooltipArbiter::default(),
                                    caret_position: None,
                                },
                            );
                        });
                    drop(context.render_legacy());
                    assert!(symbol.popup_rect.is_none());
                    assert!(!symbol.at_caret());
                    symbol
                        .hover_state
                        .deliver(ticket, Some(HoverContents::Markdown("Late reply.".into())));
                    assert!(symbol.hover_state.snapshot().is_none());
                    context
                        .io_mut()
                        .add_mouse_button_event(MouseButton::Right, false);
                }
            }
        }
    }

    #[test]
    fn native_caret_hover_accepts_its_shortcut_and_scrollbar_then_escape_cancels() {
        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = Context::create();
        context
            .set_ini_filename(None::<std::path::PathBuf>)
            .unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        let editor = DocumentRuntime::new();
        let mut client = LspClient::new("/bed-no-lsp-config.json");
        let mut symbol = LspSymbolInfo {
            at_caret: true,
            fresh_caret_request: true,
            ..Default::default()
        };
        let ticket = symbol.hover_state.begin();
        symbol.hover_state.deliver(
            ticket,
            Some(HoverContents::Markdown(
                "Symbol documentation.\n".repeat(50),
            )),
        );
        context.io_mut().add_key_event(Key::K, true);
        symbol_frame(&mut context, &mut symbol, &editor, &mut client, true);
        assert!(symbol.at_caret());
        assert!(symbol.hover_state.snapshot().is_some());
        context.io_mut().add_key_event(Key::K, false);
        symbol_frame(&mut context, &mut symbol, &editor, &mut client, false);
        let (min, max) = symbol.popup_rect.unwrap();
        context
            .io_mut()
            .add_mouse_pos_event([max[0] - 4.0, min[1] + 20.0]);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, true);
        symbol_frame(&mut context, &mut symbol, &editor, &mut client, true);
        assert!(symbol.at_caret());
        assert!(symbol.hover_state.snapshot().is_some());
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, false);
        symbol_frame(&mut context, &mut symbol, &editor, &mut client, false);
        context.io_mut().add_key_event(Key::Escape, true);
        symbol_frame(&mut context, &mut symbol, &editor, &mut client, true);
        assert!(!symbol.at_caret());
        assert!(symbol.popup_rect.is_none());
        assert!(symbol.hover_state.snapshot().is_none());
    }
}
