//! Keyboard/mouse to commands, translated from ned editor/editor_input.{h,cpp}.
//! See LICENSE and NOTICE for upstream attribution.
use crate::views::view_layout::{ViewLayout, column_at_x};
use bed_core::{editor_commands::CursorReveal, util::utf8::snap_to_utf8_char_boundary};
use bed_session::ViewContext;
#[cfg(test)]
use bed_session::editor::Editor;
use dear_imgui_rs::{Key, MouseButton, Ui, WindowHoveredFlags, sys};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostAction {
    Open,
    Save,
    SaveAs,
}

/// Navigation at a clicked text position. Columns use the document's UTF-8
/// byte coordinates; LSP adapters convert them to UTF-16 when sending requests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DefinitionRequest {
    pub row: i32,
    pub column: i32,
}

#[derive(Default)]
pub struct EditorInput {
    pub suppress_next_enter: bool,
    dragging: bool,
    anchor: Option<(i32, i32)>,
    definition_request: Option<DefinitionRequest>,
}

impl EditorInput {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn process(
        &mut self,
        ui: &Ui,
        editor: &mut ViewContext<'_>,
        layout: &ViewLayout,
    ) -> Vec<HostAction> {
        self.process_with_navigation(ui, editor, layout, !editor.view.block_input)
    }

    pub(crate) fn process_with_navigation(
        &mut self,
        ui: &Ui,
        editor: &mut ViewContext<'_>,
        layout: &ViewLayout,
        allow_navigation: bool,
    ) -> Vec<HostAction> {
        self.definition_request = None;
        if ui.is_window_hovered_with_flags(WindowHoveredFlags::ALLOW_WHEN_BLOCKED_BY_ACTIVE_ITEM) {
            self.process_mouse(ui, editor, layout, allow_navigation);
        }
        if editor.view.block_input {
            return Vec::new();
        }
        self.process_keyboard(ui, editor)
    }

    pub fn take_definition_request(&mut self) -> Option<DefinitionRequest> {
        self.definition_request.take()
    }

    fn process_keyboard(&mut self, ui: &Ui, editor: &mut ViewContext<'_>) -> Vec<HostAction> {
        let primary = ui.io().key_ctrl() || ui.io().key_super();
        let shift = ui.io().key_shift();
        let alt = ui.io().key_alt();
        let mut actions = Vec::new();
        let mut commands = editor.commands();
        if alt && !primary {
            if ui.is_key_pressed(Key::LeftArrow) {
                commands.move_word_left(shift);
            }
            if ui.is_key_pressed(Key::RightArrow) {
                commands.move_word_right(shift);
            }
            if ui.is_key_pressed(Key::UpArrow) {
                commands.add_cursor_above();
            }
            if ui.is_key_pressed(Key::DownArrow) {
                commands.add_cursor_below();
            }
        }
        if ui.is_key_pressed(Key::Escape) {
            commands.collapse_selection();
        }
        if ui.is_window_focused() {
            if ui.is_key_pressed(Key::Tab) {
                if shift {
                    commands.outdent();
                } else {
                    commands.indent();
                }
                ui.set_keyboard_focus_here_with_offset(-1);
            }
            if primary {
                if ui.is_key_pressed(Key::A) {
                    commands.select_all();
                }
                if !alt {
                    if ui.is_key_pressed(Key::LeftArrow) {
                        commands.move_line_start(shift);
                    }
                    if ui.is_key_pressed(Key::RightArrow) {
                        commands.move_line_end(shift);
                    }
                    if ui.is_key_pressed(Key::UpArrow) {
                        commands.move_lines(-5, shift);
                    }
                    if ui.is_key_pressed(Key::DownArrow) {
                        commands.move_lines(5, shift);
                    }
                }
            }
        }

        // The upstream order is character queue, Enter/Delete, plain arrows,
        // then primary shortcuts. The queue is consumed even with a modifier.
        let text = ui.with_bound_context(take_input_characters);
        if !text.is_empty() {
            commands.type_text(text.as_bytes());
        }
        if ui.is_key_pressed(Key::Enter) {
            if self.suppress_next_enter {
                self.suppress_next_enter = false;
            } else {
                commands.insert_newline();
            }
        }
        if ui.is_key_pressed(Key::Backspace) {
            commands.delete_left(alt);
        }
        if ui.is_key_pressed(Key::Delete) {
            commands.delete_right(alt);
        }
        if !alt && !primary {
            if ui.is_key_pressed(Key::UpArrow) {
                commands.move_up(shift);
            }
            if ui.is_key_pressed(Key::DownArrow) {
                commands.move_down(shift);
            }
            if ui.is_key_pressed(Key::LeftArrow) {
                commands.move_left(shift);
            }
            if ui.is_key_pressed(Key::RightArrow) {
                commands.move_right(shift);
            }
        }
        // Additional portable navigation bindings retained by Bed.
        if ui.is_key_pressed(Key::Home) {
            if primary {
                commands.move_doc_start(shift);
            } else {
                commands.move_line_start(shift);
            }
        }
        if ui.is_key_pressed(Key::End) {
            if primary {
                commands.move_doc_end(shift);
            } else {
                commands.move_line_end(shift);
            }
        }
        if ui.is_key_pressed(Key::PageUp) {
            commands.move_lines(-20, shift);
        }
        if ui.is_key_pressed(Key::PageDown) {
            commands.move_lines(20, shift);
        }
        if primary {
            if ui.is_key_pressed_with_repeat(Key::C, false) {
                let text = commands.copy();
                if !text.is_empty() {
                    set_clipboard(ui, &text);
                }
            }
            if ui.is_key_pressed_with_repeat(Key::X, false) {
                let text = commands.cut();
                set_clipboard(ui, &text);
            }
            if ui.is_key_pressed_with_repeat(Key::V, false)
                && let Some(text) = get_clipboard(ui)
            {
                commands.paste(text.as_bytes());
            }
            if ui.is_key_pressed(Key::Z) {
                if shift {
                    commands.redo();
                } else {
                    commands.undo();
                }
            }
            if ui.is_key_pressed_with_repeat(Key::S, false) {
                actions.push(if shift {
                    HostAction::SaveAs
                } else {
                    HostAction::Save
                });
            }
            if ui.is_key_pressed_with_repeat(Key::O, false) {
                actions.push(HostAction::Open);
            }
        }
        actions
    }

    fn process_mouse(
        &mut self,
        ui: &Ui,
        editor: &mut ViewContext<'_>,
        layout: &ViewLayout,
        allow_navigation: bool,
    ) {
        if layout.line_height <= 0.0 || editor.state.line_count() <= 0 {
            return;
        }
        let (content_min, content_max) = content_region(ui);
        if !ui.is_mouse_hovering_rect_with_clip(content_min, content_max, false) {
            if self.dragging && ui.is_mouse_released(MouseButton::Left) {
                self.release_mouse();
            }
            return;
        }
        if !self.dragging && ui.is_any_item_active() {
            return;
        }
        let mouse = ui.mouse_pos();
        let row = (((mouse[1] - layout.text_pos[1]) / layout.line_height) as i32)
            .clamp(0, editor.state.line_count() - 1);
        let line = editor.state.line(row);
        let col = snap_to_utf8_char_boundary(
            &line,
            column_at_x(ui, &line, mouse[0] - layout.text_pos[0]),
        );
        // ImGui 1.92 swaps incoming physical Cmd/Ctrl when MacOSXBehaviors is
        // enabled. Hosts may configure it, so use the physical platform key.
        let navigation_modifier = if cfg!(target_os = "macos") == ui.io().config_macosx_behaviors()
        {
            ui.io().key_ctrl()
        } else {
            ui.io().key_super()
        };
        if navigation_modifier && ui.is_mouse_clicked(MouseButton::Left) {
            self.release_mouse();
            if allow_navigation {
                self.definition_request = Some(DefinitionRequest { row, column: col });
            }
            return;
        }
        if ui.is_mouse_double_clicked(MouseButton::Left) {
            editor.commands().select_word_at(row, col);
            return;
        }
        if ui.is_mouse_clicked(MouseButton::Left) {
            if ui.io().key_shift() {
                let (anchor_row, anchor_column) = if editor.view.selection_empty() {
                    (editor.view.row, editor.view.column)
                } else {
                    let p = editor.view.primary();
                    (p.anchor_row, p.anchor_column)
                };
                self.anchor = Some((anchor_row, anchor_column));
                editor.commands().set_selection(
                    anchor_row,
                    anchor_column,
                    row,
                    col,
                    CursorReveal::Ensure,
                );
            } else {
                editor
                    .commands()
                    .set_cursor(row, col, false, CursorReveal::Ensure);
                self.anchor = Some((row, col));
            }
            self.dragging = true;
            editor.update_pending_cursor();
        } else if self.dragging && ui.is_mouse_dragging(MouseButton::Left) {
            let (ar, ac) = *self
                .anchor
                .get_or_insert((editor.view.row, editor.view.column));
            editor
                .commands()
                .set_selection(ar, ac, row, col, CursorReveal::Ensure);
        } else if ui.is_mouse_released(MouseButton::Left) {
            self.release_mouse();
        }
    }

    fn release_mouse(&mut self) {
        self.dragging = false;
        self.anchor = None;
    }
}

fn get_clipboard(ui: &Ui) -> Option<String> {
    ui.with_bound_context(|| unsafe {
        let ptr = sys::igGetClipboardText();
        (!ptr.is_null()).then(|| std::ffi::CStr::from_ptr(ptr).to_string_lossy().into_owned())
    })
}
fn set_clipboard(ui: &Ui, bytes: &[u8]) {
    if let Ok(text) = std::ffi::CString::new(String::from_utf8_lossy(bytes).as_bytes()) {
        ui.with_bound_context(|| unsafe { sys::igSetClipboardText(text.as_ptr()) });
    }
}

fn content_region(ui: &Ui) -> ([f32; 2], [f32; 2]) {
    ui.with_bound_context(|| {
        // SAFETY: a window is active throughout input processing. The pinned
        // ImGui ContentRegionMin/Max functions return this rect minus window
        // position; upstream adds that position back before its hit test.
        unsafe {
            let rect = (*sys::igGetCurrentWindowRead()).ContentRegionRect;
            ([rect.Min.x, rect.Min.y], [rect.Max.x, rect.Max.y])
        }
    })
}

/// ImGui 1.92 exposes its Unicode queue through the C API. Copy codepoints while
/// the active Ui owns the context, encode with Rust, and consume the queue once.
pub fn take_input_characters() -> String {
    // SAFETY: called on the UI thread during an active frame. The queue belongs
    // to that context and no references escape before Size is reset.
    unsafe {
        let queue = &mut (*sys::igGetIO_Nil()).InputQueueCharacters;
        let mut text = String::new();
        if queue.Size > 0 {
            for codepoint in std::slice::from_raw_parts(queue.Data, queue.Size as usize) {
                if *codepoint >= 32
                    && *codepoint != 127
                    && let Some(ch) = char::from_u32(*codepoint)
                {
                    text.push(ch);
                }
            }
        }
        queue.Size = 0;
        text
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

    fn render(
        context: &mut Context,
        editor: &mut Editor,
        input: &mut EditorInput,
        focus_editor: bool,
        active_item: bool,
    ) -> ([f32; 2], [f32; 2], ViewLayout) {
        // One second separates clicks so independent scenarios do not become
        // native double clicks. Key presses still have their usual first frame.
        context.prepare_frame(FramePrepareOptions::new([640.0, 480.0], 1.0));
        let ui = context.frame();
        if !focus_editor {
            ui.window("Other pane")
                .position([360.0, 20.0], Condition::Always)
                .size([250.0, 200.0], Condition::Always)
                .build(|| ui.set_window_focus(None));
        }
        let mut rect = ([0.0; 2], [0.0; 2], ViewLayout::default());
        ui.window("Input editor")
            .position([20.0, 20.0], Condition::Always)
            .size([300.0, 200.0], Condition::Always)
            .flags(dear_imgui_rs::WindowFlags::NO_TITLE_BAR | dear_imgui_rs::WindowFlags::NO_MOVE)
            .build(|| {
                if focus_editor {
                    ui.set_window_focus(None);
                }
                let (min, max) = content_region(ui);
                let layout = ViewLayout {
                    text_pos: ui.cursor_screen_pos(),
                    line_height: ui.text_line_height(),
                    ..Default::default()
                };
                if active_item {
                    ui.with_bound_context(|| unsafe {
                        sys::igSetActiveID(1234, sys::igGetCurrentWindow());
                    });
                }
                assert!(
                    input
                        .process(ui, &mut editor.view_context(), &layout)
                        .is_empty()
                );
                if active_item {
                    ui.with_bound_context(|| unsafe { sys::igClearActiveID() });
                }
                rect = (min, max, layout);
            });
        drop(context.render_legacy());
        rect
    }

    #[test]
    fn queued_unicode_is_inserted_before_plain_arrows_and_consumed_with_modifiers() {
        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = context();
        let mut editor = Editor::new();
        let mut input = EditorInput::default();
        editor.set_content(b"ab");
        editor
            .commands()
            .set_cursor(0, 1, false, CursorReveal::Ensure);
        render(&mut context, &mut editor, &mut input, true, false);
        context.io_mut().add_input_characters_utf8("雪");
        context.io_mut().add_key_event(Key::LeftArrow, true);
        render(&mut context, &mut editor, &mut input, true, false);
        assert_eq!(editor.state.join(), "a雪b".as_bytes());
        assert_eq!(editor.view.column, 1);

        context.io_mut().add_key_event(Key::LeftArrow, false);
        context.io_mut().add_key_event(Key::ModCtrl, true);
        context.io_mut().add_input_characters_utf8("é");
        render(&mut context, &mut editor, &mut input, true, false);
        assert_eq!(editor.state.join(), "aé雪b".as_bytes());
        assert_eq!(editor.view.column, 3);
        context.io_mut().add_key_event(Key::ModCtrl, false);
        render(&mut context, &mut editor, &mut input, true, false);
        assert_eq!(editor.state.join(), "aé雪b".as_bytes());
    }

    #[test]
    fn tab_and_primary_navigation_obey_the_source_window_focus_guard() {
        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = context();
        let mut editor = Editor::new();
        let mut input = EditorInput::default();
        editor.set_content(b"abcd");
        editor
            .commands()
            .set_cursor(0, 2, false, CursorReveal::Ensure);
        render(&mut context, &mut editor, &mut input, true, false);
        render(&mut context, &mut editor, &mut input, false, false);
        context.io_mut().add_key_event(Key::Tab, true);
        render(&mut context, &mut editor, &mut input, false, false);
        assert_eq!(editor.state.join(), b"abcd");
        context.io_mut().add_key_event(Key::Tab, false);
        context.io_mut().add_key_event(Key::ModCtrl, true);
        context.io_mut().add_key_event(Key::LeftArrow, true);
        render(&mut context, &mut editor, &mut input, false, false);
        assert_eq!(editor.view.column, 2);
        context.io_mut().add_key_event(Key::LeftArrow, false);
        context.io_mut().add_key_event(Key::A, true);
        render(&mut context, &mut editor, &mut input, false, false);
        assert!(editor.view.selection_empty());
        context.io_mut().add_key_event(Key::A, false);
        context.io_mut().add_key_event(Key::LeftArrow, true);
        render(&mut context, &mut editor, &mut input, true, false);
        assert_eq!(editor.view.column, 0);
    }

    #[test]
    fn mouse_uses_native_content_bounds_and_active_item_guard() {
        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = context();
        let mut editor = Editor::new();
        let mut input = EditorInput::default();
        editor.set_content(b"abcd");
        let (_, max, layout) = render(&mut context, &mut editor, &mut input, true, false);
        let edge = [max[0] - 1.0, layout.text_pos[1] + 5.0];
        // This point is in content and inside the previous hardcoded 14px dead
        // strip, when ImGui has no scrollbar and its default padding is 8px.
        assert!(edge[0] > 20.0 + 300.0 - 14.0);
        context.io_mut().add_mouse_pos_event(edge);
        render(&mut context, &mut editor, &mut input, true, false);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, true);
        render(&mut context, &mut editor, &mut input, true, false);
        assert_eq!(editor.view.column, 4);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, false);
        render(&mut context, &mut editor, &mut input, true, false);
        assert!(!input.dragging);

        context
            .io_mut()
            .add_mouse_pos_event([layout.text_pos[0] + 1.0, edge[1]]);
        render(&mut context, &mut editor, &mut input, true, false);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, true);
        render(&mut context, &mut editor, &mut input, true, true);
        assert_eq!(editor.view.column, 4);
        assert!(!input.dragging);
    }

    #[test]
    fn mouse_click_updates_pending_undo_final_caret_before_redo() {
        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = context();
        let mut editor = Editor::new();
        let mut input = EditorInput::default();
        editor.state.path = "pending-mouse-caret.txt".into();
        editor.set_content(b"ab");
        editor.commands().type_text(b"x");
        let (_, _, layout) = render(&mut context, &mut editor, &mut input, true, false);
        context
            .io_mut()
            .add_mouse_pos_event([layout.text_pos[0] + 1.0, layout.text_pos[1] + 5.0]);
        render(&mut context, &mut editor, &mut input, true, false);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, true);
        render(&mut context, &mut editor, &mut input, true, false);
        assert_eq!(editor.view.column, 0);
        editor.commands().undo();
        assert_eq!(editor.state.join(), b"ab");
        editor.commands().redo();
        assert_eq!(editor.state.join(), b"xab");
        assert_eq!(editor.view.column, 0);
    }
}
