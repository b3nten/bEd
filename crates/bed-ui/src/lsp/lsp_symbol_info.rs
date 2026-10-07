//! Translated from ned lsp/lsp_symbol_info.{h,cpp}; see LICENSE and NOTICE.
use crate::{
    lsp::lsp_ui::LspView,
    views::{
        hover_tooltip::{
            HoverRect, hover_key_pressed, hover_rect_contains, render_hover_markdown,
            render_hover_popup,
        },
        hover_trigger::Zone,
        view_layout::line_column_x,
    },
};
use bed_core::util::utf8::utf8_byte_offset_to_utf16;
use bed_lsp::{
    lsp_client::LspClient, lsp_request::LspRequestState, lsp_uri::LspUri,
    workspace_lsp::LspRequestOrigin,
};
use bed_session::editor::Editor;
use dear_imgui_rs::Ui;
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
pub fn format_hover_contents(contents: &Value, fallback_language: &str) -> io::Result<String> {
    if let Some(kind) = contents.get("kind") {
        let kind = kind.as_str().ok_or_else(invalid_hover)?;
        let text = contents
            .get("value")
            .and_then(Value::as_str)
            .ok_or_else(invalid_hover)?;
        return Ok(if kind == "plaintext" {
            fence_wrap(fallback_language, text)
        } else {
            text.to_owned()
        });
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

pub struct LspSymbolInfo {
    at_caret: bool,
    fresh_caret_request: bool,
    requested_for_cell: bool,
    hover_state: Arc<LspRequestState<String>>,
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
    pub fn get(&mut self, client: &mut LspClient, editor: &Editor) {
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
        editor: &Editor,
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
        editor: &Editor,
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
        let utf16 = utf8_byte_offset_to_utf16(&editor.state.line(row), byte_column);
        let origin = origin.map(|origin| self.hover_state.begin_for(origin));
        let ticket = origin.map_or_else(|| self.hover_state.begin(), |origin| origin.ticket);
        let language = editor.state.language_id.clone();
        let state = self.hover_state.clone();
        let result = LspUri::file_uri_from_path(&editor.state.path).and_then(|uri| {
            client.send_request("textDocument/hover",json!({"textDocument":{"uri":uri.to_string()},"position":{"line":row as u32,"character":utf16 as u32}}),move |result| {
                let text = result.ok().and_then(|result| result.get("contents").and_then(|contents| format_hover_contents(contents, &language).ok())).filter(|text| !text.is_empty());
                if let Some(origin) = origin { state.deliver_for(origin, text); } else { state.deliver(ticket, text); }
            }).map(|_| ())
        });
        if let Err(error) = result {
            eprintln!("LSP: hover request failed: {error}");
            if let Some(origin) = origin {
                self.hover_state.deliver_for(origin, None);
            } else {
                self.hover_state.deliver(ticket, None);
            }
        }
    }
    fn update_mouse_hover(
        &mut self,
        ui: &Ui,
        client: &mut LspClient,
        editor: &Editor,
        view: &LspView<'_>,
    ) {
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
    pub fn render(&mut self, ui: &Ui, client: &mut LspClient, editor: &Editor, view: &LspView<'_>) {
        self.update_mouse_hover(ui, client, editor, view);
        let Some(markdown) = self.hover_state.snapshot().filter(|text| !text.is_empty()) else {
            return;
        };
        if !view.tooltip_arbiter.claim(ui) {
            return;
        }
        let fs = ui.current_font_size();
        let anchor = if self.at_caret {
            let x = line_column_x(
                ui,
                &editor.state.line(editor.view.row),
                editor.view.column,
                view.layout.text_pos[0],
            )
            .floor();
            [
                x + fs * 0.25,
                view.layout.text_pos[1]
                    + (editor.view.row + 1) as f32 * view.layout.line_height
                    + fs * 0.25,
            ]
        } else {
            *self.popup_anchor.get_or_insert_with(|| {
                let mouse = ui.io().mouse_pos();
                [mouse[0] + 8.0, mouse[1] + 12.0]
            })
        };
        self.popup_rect =
            render_hover_popup(ui, "##bed_lsp_hover", anchor, self.popup_rect, || {
                render_hover_markdown(ui, &markdown, editor, &editor.state.language_id);
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::{
        hover_tooltip::TooltipArbiter, hover_trigger::Info, view_layout::ViewLayout,
    };
    use dear_imgui_rs::{Condition, Context, FramePrepareOptions, Key, MouseButton};

    fn symbol_frame(
        context: &mut Context,
        symbol: &mut LspSymbolInfo,
        editor: &Editor,
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
                    },
                );
            });
        drop(context.render_legacy());
    }
    #[test]
    fn original_hover_shapes_preserve_plaintext_fences_and_single_array_boundaries() {
        assert_eq!(
            format_hover_contents(&json!({"kind":"plaintext","value":"int foo();"}), "cpp")
                .unwrap(),
            "```cpp\nint foo();\n```"
        );
        assert_eq!(
            format_hover_contents(&json!({"kind":"markdown","value":"**hi**"}), "rust").unwrap(),
            "**hi**"
        );
        // Pinned Enumeration<string> retains custom values. The original
        // formatter fences only PlainText; all other kinds use raw markdown.
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
    }
    #[test]
    fn typed_hover_changes_document_identity_at_the_same_cell_and_rejects_old_replies() {
        use bed_core::identity::{DocumentId, ViewId, WorkspaceId};
        let mut symbol = LspSymbolInfo::default();
        let first = LspRequestOrigin {
            workspace_id: WorkspaceId(1),
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
        symbol
            .hover_state
            .deliver_for(first, Some("first document".into()));
        let second = LspRequestOrigin {
            document_id: DocumentId(7),
            view_id: ViewId(8),
            ..first
        };
        symbol.set_mouse_origin(Some(second));
        assert!(!symbol.requested_for_cell);
        assert!(symbol.hover_state.snapshot().is_none());
        let second = symbol.hover_state.begin_for(second);
        symbol
            .hover_state
            .deliver_for(first, Some("late first document".into()));
        assert!(symbol.hover_state.is_pending());
        symbol
            .hover_state
            .deliver_for(second, Some("second document".into()));
        assert_eq!(symbol.origin(), Some(second));
        assert_eq!(
            symbol.hover_state.snapshot().as_deref(),
            Some("second document")
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
        let editor = Editor::new();
        let mut client = LspClient::new("/bed-no-lsp-config.json");
        let mut symbol = LspSymbolInfo::default();
        let ticket = symbol.hover_state.begin();
        symbol
            .hover_state
            .deliver(ticket, Some("a **symbol**".into()));
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
                        },
                    )
                });
            drop(context.render_legacy());
        }
        assert!(symbol.popup_rect.is_some());
        symbol.cancel();
        symbol.hover_state.deliver(ticket, Some("late".into()));
        assert!(symbol.hover_state.snapshot().is_none());
        assert!(symbol.popup_rect.is_none());
        assert!(!symbol.hover_state.is_pending());
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
        let editor = Editor::new();
        let mut client = LspClient::new("/bed-no-lsp-config.json");
        let mut symbol = LspSymbolInfo {
            at_caret: true,
            fresh_caret_request: true,
            ..Default::default()
        };
        let ticket = symbol.hover_state.begin();
        symbol
            .hover_state
            .deliver(ticket, Some("Symbol documentation.\n".repeat(50)));
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
