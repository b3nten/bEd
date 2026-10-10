//! First-class exact-byte editing over the same session lifecycle as text.
use bed_document_session::{ByteEdit, DocumentId, DocumentKind, EditorSession};
use dear_imgui_rs::{FocusedFlags, Key, ListClipper, Ui, WindowFlags};
use std::{io, ops::Range};

pub struct HexEditor {
    document: DocumentId,
    cursor: usize,
    anchor: usize,
    insert: bool,
    pending_nibble: Option<u8>,
    input: String,
    error: Option<String>,
    reveal_cursor: bool,
    observed_revision: Option<(u64, u64)>,
}
impl HexEditor {
    pub fn new(document: DocumentId) -> Self {
        Self {
            document,
            cursor: 0,
            anchor: 0,
            insert: false,
            pending_nibble: None,
            input: String::new(),
            error: None,
            reveal_cursor: false,
            observed_revision: None,
        }
    }
    pub fn document(&self) -> DocumentId {
        self.document
    }
    pub fn document_id(&self) -> DocumentId {
        self.document
    }
    pub fn state(&self) -> serde_json::Value {
        serde_json::json!({ "cursor": self.cursor, "anchor": self.anchor, "insert": self.insert })
    }
    pub fn restore_state(&mut self, state: &serde_json::Value) {
        self.cursor = state.get("cursor").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        self.anchor = state
            .get("anchor")
            .and_then(|v| v.as_u64())
            .unwrap_or(self.cursor as u64) as usize;
        self.insert = state
            .get("insert")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        self.reveal_cursor = true;
    }
    /// Update selection positions after edits in another view, including when
    /// this panel is hidden and the host is about to persist its state.
    pub fn synchronize(&mut self, session: &EditorSession) -> io::Result<()> {
        let current = session.document_revision(self.document)?;
        if let Some(previous) = self
            .observed_revision
            .filter(|previous| *previous != current)
        {
            let mut offsets = [self.cursor, self.anchor];
            session.transform_byte_offsets(self.document, previous, &mut offsets)?;
            [self.cursor, self.anchor] = offsets;
            self.pending_nibble = None;
        }
        self.observed_revision = Some(current);
        Ok(())
    }
    fn selection(&self, len: usize) -> Range<usize> {
        self.anchor.min(self.cursor).min(len)
            ..self.anchor.max(self.cursor).saturating_add(1).min(len)
    }
    fn move_cursor(&mut self, cursor: usize, shift: bool, len: usize) {
        self.cursor = cursor.min(len);
        if !shift {
            self.anchor = self.cursor;
        }
        self.pending_nibble = None;
        self.reveal_cursor = true;
    }
    fn replace(
        &mut self,
        session: &mut EditorSession,
        range: Range<usize>,
        bytes: Vec<u8>,
    ) -> io::Result<()> {
        let end = range.start + bytes.len();
        let revision = session.document_revision(self.document)?;
        session.apply_edits(self.document, revision, &[ByteEdit { range, bytes }])?;
        self.observed_revision = Some(session.document_revision(self.document)?);
        self.cursor = end;
        self.anchor = end;
        self.pending_nibble = None;
        self.reveal_cursor = true;
        Ok(())
    }
    pub fn copy(&mut self, ui: &Ui, session: &EditorSession) -> io::Result<()> {
        self.synchronize(session)?;
        let snapshot = session.snapshot(self.document)?;
        let text = snapshot.bytes[self.selection(snapshot.bytes.len())]
            .iter()
            .map(|byte| format!("{byte:02X}"))
            .collect::<Vec<_>>()
            .join(" ");
        set_clipboard(ui, &text);
        Ok(())
    }
    pub fn paste(&mut self, ui: &Ui, session: &mut EditorSession) -> io::Result<()> {
        self.synchronize(session)?;
        if let Some(text) = clipboard(ui) {
            let bytes = parse_hex(&text)?;
            if bytes.is_empty() {
                return Ok(());
            }
            let len = session.with_document(self.document, |state| state.byte_size())?;
            let range = if self.insert {
                self.cursor.min(len)..self.cursor.min(len)
            } else if self.anchor != self.cursor {
                self.selection(len)
            } else {
                self.cursor.min(len)..(self.cursor + bytes.len()).min(len)
            };
            self.replace(session, range, bytes)?;
        }
        Ok(())
    }
    pub fn cut(&mut self, ui: &Ui, session: &mut EditorSession) -> io::Result<()> {
        self.copy(ui, session)?;
        let len = session.with_document(self.document, |state| state.byte_size())?;
        self.replace(session, self.selection(len), Vec::new())
    }
    pub fn select_all(&mut self, session: &EditorSession) -> io::Result<()> {
        let len = session.with_document(self.document, |state| state.byte_size())?;
        self.anchor = 0;
        self.cursor = len.saturating_sub(1);
        Ok(())
    }
    pub fn draw(&mut self, ui: &Ui, session: &mut EditorSession) -> io::Result<()> {
        if session.document_kind(self.document)? != DocumentKind::Bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Hex editor requires an exact-byte document",
            ));
        }
        self.synchronize(session)?;
        let len = session.with_document(self.document, |state| state.byte_size())?;
        self.cursor = self.cursor.min(len);
        self.anchor = self.anchor.min(len);
        let _id = ui.push_id(&format!("hex-{}", self.document.0));
        ui.checkbox("Insert", &mut self.insert);
        ui.same_line();
        ui.text(format!("{len} bytes  |  offset 0x{:X}", self.cursor));
        ui.set_next_item_width(220.0);
        let enter = ui
            .input_text("Hex bytes", &mut self.input)
            .enter_returns_true(true)
            .build();
        ui.same_line();
        if ui.button("Apply") || enter {
            let result = (|| {
                let bytes = parse_hex(&self.input)?;
                if bytes.is_empty() {
                    return Ok(());
                }
                let range = if self.insert {
                    self.cursor..self.cursor
                } else if self.anchor != self.cursor {
                    self.selection(len)
                } else {
                    self.cursor..(self.cursor + bytes.len()).min(len)
                };
                self.replace(session, range, bytes)
            })();
            self.error = result.err().map(|e: io::Error| e.to_string());
        }
        if let Some(error) = &self.error {
            ui.text_colored([1.0, 0.45, 0.35, 1.0], error);
        }
        ui.separator();
        let mut clicked = None;
        ui.child_window("bytes")
            .size([0.0, 0.0])
            .flags(WindowFlags::HORIZONTAL_SCROLLBAR)
            .build(ui, || {
                let focused = ui.is_window_focused_with_flags(FocusedFlags::ROOT_AND_CHILD_WINDOWS);
                if focused
                    && !ui.is_any_item_active()
                    && let Err(error) = self.keyboard(ui, session, len)
                {
                    self.error = Some(error.to_string());
                }
                let current_len = session
                    .with_document(self.document, |state| state.byte_size())
                    .unwrap_or(len);
                let rows = current_len / 16 + 1;
                let row_height = ui.text_line_height_with_spacing();
                let mut clipper = ListClipper::new(rows).items_height(row_height).begin(ui);
                if self.reveal_cursor {
                    clipper.include_item_by_index((self.cursor / 16).min(rows - 1));
                }
                let selection = self.selection(current_len);
                while clipper.step() {
                    for row in clipper.display_start()..clipper.display_end() {
                        let offset = row * 16;
                        let mut bytes = [0_u8; 16];
                        let count = 16.min(current_len.saturating_sub(offset));
                        let _ = session.with_document(self.document, |state| {
                            state.copy_bytes(offset, count, &mut bytes)
                        });
                        ui.text(format!("{offset:08X}  "));
                        for (column, byte) in bytes.iter().enumerate().take(count) {
                            ui.same_line_with_spacing(0.0, 3.0);
                            let pos = offset + column;
                            if ui
                                .selectable_config(format!("{byte:02X}##{pos}"))
                                .selected(selection.contains(&pos))
                                .size([ui.calc_text_size("FF")[0] + 3.0, ui.text_line_height()])
                                .build()
                            {
                                clicked = Some(pos);
                            }
                        }
                        if offset + count == current_len && count < 16 {
                            ui.same_line_with_spacing(0.0, 3.0);
                            if ui
                                .selectable_config(format!("--##eof{offset}"))
                                .selected(self.cursor == current_len)
                                .size([ui.calc_text_size("FF")[0] + 3.0, ui.text_line_height()])
                                .build()
                            {
                                clicked = Some(current_len);
                            }
                        }
                        ui.same_line_with_pos(
                            10.0 * ui.calc_text_size("0")[0]
                                + 16.0 * (ui.calc_text_size("FF")[0] + 6.0)
                                + 18.0,
                        );
                        let ascii: String = bytes[..count]
                            .iter()
                            .map(|&b| {
                                if b.is_ascii_graphic() || b == b' ' {
                                    b as char
                                } else {
                                    '.'
                                }
                            })
                            .collect();
                        ui.text(ascii);
                        if self.reveal_cursor && row == self.cursor / 16 {
                            ui.set_scroll_here_y(0.5);
                        }
                    }
                }
                self.reveal_cursor = false;
                if let Some(_popup) = ui.begin_popup_context_window() {
                    if ui.menu_item("Copy hex") {
                        let _ = self.copy(ui, session);
                    }
                    if ui.menu_item("Paste hex")
                        && let Err(error) = self.paste(ui, session)
                    {
                        self.error = Some(error.to_string());
                    }
                    if ui.menu_item("Delete bytes")
                        && let Err(error) =
                            self.replace(session, self.selection(current_len), Vec::new())
                    {
                        self.error = Some(error.to_string());
                    }
                    ui.separator();
                    if ui.menu_item("Undo") {
                        let _ = session.undo_document(self.document);
                    }
                    if ui.menu_item("Redo") {
                        let _ = session.redo_document(self.document);
                    }
                }
            });
        if let Some(pos) = clicked {
            self.move_cursor(pos, ui.io().key_shift(), len);
        }
        Ok(())
    }
    fn keyboard(&mut self, ui: &Ui, session: &mut EditorSession, len: usize) -> io::Result<()> {
        let primary = ui.io().key_ctrl() || ui.io().key_super();
        let shift = ui.io().key_shift();
        if primary {
            if ui.is_key_pressed(Key::A) {
                self.select_all(session)?;
            }
            if ui.is_key_pressed(Key::C) {
                self.copy(ui, session)?;
            }
            if ui.is_key_pressed(Key::V) {
                self.paste(ui, session)?;
            }
            if ui.is_key_pressed(Key::X) {
                self.cut(ui, session)?;
            }
            if ui.is_key_pressed(Key::Z) {
                if shift {
                    session.redo_document(self.document)?;
                } else {
                    session.undo_document(self.document)?;
                }
                self.synchronize(session)?;
                self.pending_nibble = None;
            }
            if ui.is_key_pressed(Key::Y) {
                session.redo_document(self.document)?;
                self.synchronize(session)?;
            }
            return Ok(());
        }
        if ui.is_key_pressed(Key::LeftArrow) {
            self.move_cursor(self.cursor.saturating_sub(1), shift, len);
        }
        if ui.is_key_pressed(Key::RightArrow) {
            self.move_cursor(self.cursor + 1, shift, len);
        }
        if ui.is_key_pressed(Key::UpArrow) {
            self.move_cursor(self.cursor.saturating_sub(16), shift, len);
        }
        if ui.is_key_pressed(Key::DownArrow) {
            self.move_cursor(self.cursor + 16, shift, len);
        }
        if ui.is_key_pressed(Key::Home) {
            self.move_cursor(self.cursor / 16 * 16, shift, len);
        }
        if ui.is_key_pressed(Key::End) {
            self.move_cursor((self.cursor / 16 * 16 + 15).min(len), shift, len);
        }
        if ui.is_key_pressed(Key::Insert) {
            self.insert = !self.insert;
        }
        if ui.is_key_pressed(Key::Delete) {
            self.replace(session, self.selection(len), Vec::new())?;
        }
        if ui.is_key_pressed(Key::Backspace) {
            let range = if self.anchor != self.cursor {
                self.selection(len)
            } else {
                self.cursor.saturating_sub(1)..self.cursor
            };
            self.replace(session, range, Vec::new())?;
        }
        let input = ui.with_bound_context(crate::editor_input::take_input_characters);
        for ch in input.chars() {
            if let Some(nibble) = ch.to_digit(16).map(|n| n as u8) {
                if let Some(high) = self.pending_nibble.take() {
                    let current_len =
                        session.with_document(self.document, |state| state.byte_size())?;
                    let range = if self.insert {
                        self.cursor..self.cursor
                    } else {
                        self.selection(current_len)
                    };
                    self.replace(session, range, vec![(high << 4) | nibble])?;
                } else {
                    self.pending_nibble = Some(nibble);
                }
            }
        }
        Ok(())
    }
}
fn parse_hex(text: &str) -> io::Result<Vec<u8>> {
    let digits: Vec<_> = text.chars().filter(|ch| !ch.is_whitespace()).collect();
    if digits.len() % 2 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Hex input needs two digits per byte",
        ));
    }
    digits
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            let high = pair[0].to_digit(16).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Hex input contains a non-hexadecimal digit",
                )
            })?;
            let low = pair[1].to_digit(16).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Hex input contains a non-hexadecimal digit",
                )
            })?;
            Ok(((high << 4) | low) as u8)
        })
        .collect()
}
fn clipboard(ui: &Ui) -> Option<String> {
    ui.with_bound_context(|| unsafe {
        let ptr = dear_imgui_sys::igGetClipboardText();
        (!ptr.is_null()).then(|| std::ffi::CStr::from_ptr(ptr).to_string_lossy().into_owned())
    })
}
fn set_clipboard(ui: &Ui, text: &str) {
    if let Ok(text) = std::ffi::CString::new(text) {
        ui.with_bound_context(|| unsafe { dear_imgui_sys::igSetClipboardText(text.as_ptr()) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dear_imgui_rs::{Condition, Context, FramePrepareOptions};
    fn context() -> Context {
        let mut context = Context::create();
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
    fn frame(context: &mut Context, editor: &mut HexEditor, session: &mut EditorSession) -> usize {
        context.prepare_frame(FramePrepareOptions::new([900.0, 600.0], 1.0 / 60.0));
        let ui = context.frame();
        ui.window("Hex editor fixture")
            .position([0.0; 2], Condition::Always)
            .size([850.0, 550.0], Condition::Always)
            .focused(true)
            .build(|| {
                editor.draw(ui, session).unwrap();
            });
        context.render_legacy().draw_data().total_vtx_count()
    }
    #[test]
    fn native_hex_typing_insert_delete_and_undo_redo_use_the_byte_session() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = context();
        let mut session = EditorSession::new();
        let document = session
            .create_document_with_kind(&[0x11, 0x22, 0x33], DocumentKind::Bytes)
            .unwrap();
        let mut editor = HexEditor::new(document);
        frame(&mut context, &mut editor, &mut session);
        context.io_mut().add_input_characters_utf8("A");
        frame(&mut context, &mut editor, &mut session);
        assert_eq!(
            session.snapshot(document).unwrap().bytes,
            [0x11, 0x22, 0x33]
        );
        context.io_mut().add_input_characters_utf8("B");
        frame(&mut context, &mut editor, &mut session);
        assert_eq!(
            session.snapshot(document).unwrap().bytes,
            [0xab, 0x22, 0x33]
        );
        editor.insert = true;
        context.io_mut().add_input_characters_utf8("00FF");
        frame(&mut context, &mut editor, &mut session);
        assert_eq!(
            session.snapshot(document).unwrap().bytes,
            [0xab, 0x00, 0xff, 0x22, 0x33]
        );
        editor.anchor = 1;
        editor.cursor = 2;
        context.io_mut().add_key_event(Key::Delete, true);
        frame(&mut context, &mut editor, &mut session);
        assert_eq!(
            session.snapshot(document).unwrap().bytes,
            [0xab, 0x22, 0x33]
        );
        context.io_mut().add_key_event(Key::Delete, false);
        frame(&mut context, &mut editor, &mut session);
        context.io_mut().add_key_event(Key::ModCtrl, true);
        context.io_mut().add_key_event(Key::Z, true);
        frame(&mut context, &mut editor, &mut session);
        assert_eq!(
            session.snapshot(document).unwrap().bytes,
            [0xab, 0x00, 0xff, 0x22, 0x33]
        );
        context.io_mut().add_key_event(Key::Z, false);
        frame(&mut context, &mut editor, &mut session);
        context.io_mut().add_key_event(Key::ModShift, true);
        context.io_mut().add_key_event(Key::Z, true);
        frame(&mut context, &mut editor, &mut session);
        assert_eq!(
            session.snapshot(document).unwrap().bytes,
            [0xab, 0x22, 0x33]
        );
    }
    #[test]
    fn split_hex_views_transform_sibling_positions_without_replaying_own_edits() {
        let mut session = EditorSession::new();
        let document = session
            .create_document_with_kind(b"abcde", DocumentKind::Bytes)
            .unwrap();
        let mut left = HexEditor::new(document);
        let mut right = HexEditor::new(document);
        right.cursor = 2;
        right.anchor = 1;
        left.synchronize(&session).unwrap();
        right.synchronize(&session).unwrap();
        right.pending_nibble = Some(0xa);
        left.replace(&mut session, 1..1, b"XY".to_vec()).unwrap();
        right.synchronize(&session).unwrap();
        left.synchronize(&session).unwrap();
        assert_eq!((right.anchor, right.cursor), (3, 4));
        assert_eq!(left.cursor, 3);
        assert_eq!(right.pending_nibble, None);
        left.replace(&mut session, 0..2, Vec::new()).unwrap();
        right.synchronize(&session).unwrap();
        assert_eq!((right.anchor, right.cursor), (1, 2));
        session.undo_document(document).unwrap();
        right.synchronize(&session).unwrap();
        assert_eq!((right.anchor, right.cursor), (3, 4));
        session.redo_document(document).unwrap();
        right.synchronize(&session).unwrap();
        assert_eq!((right.anchor, right.cursor), (1, 2));
    }
    #[test]
    fn large_byte_views_clip_rows_and_restore_selection_without_rendering_whole_file() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = context();
        let mut session = EditorSession::new();
        let document = session
            .create_document_with_kind(&vec![0xff; 1024 * 1024], DocumentKind::Bytes)
            .unwrap();
        let mut editor = HexEditor::new(document);
        editor.restore_state(&serde_json::json!({"cursor":900_000,"anchor":899_999,"insert":true}));
        let vertices = frame(&mut context, &mut editor, &mut session);
        assert!(
            vertices > 0 && vertices < 100_000,
            "unclipped hex rendering produced {vertices} vertices"
        );
        assert_eq!(editor.selection(1024 * 1024), 899_999..900_001);
        assert_eq!(editor.state()["insert"], true);
    }
}
