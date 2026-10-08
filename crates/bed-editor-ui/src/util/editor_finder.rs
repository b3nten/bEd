//! In-buffer find and replacement translated from ned editor/util/editor_finder.{h,cpp}.
//! See LICENSE and NOTICE for source attribution.
use bed_document_session::{ViewContext, editor::Editor};
use bed_editing::{editor_commands::CursorReveal, editor_view_state::Selection};
use dear_imgui_rs::{
    FocusedFlags, InputTextFlags, Key, MouseButton, StyleColor, StyleVar, Ui, WindowHoveredFlags,
    sys,
};

#[derive(Clone, Copy, Debug)]
pub struct EditorOverlayStyle {
    pub background_color: [f32; 4],
    /// Host editor position and size. Omit to use the current ImGui window.
    pub pane: Option<([f32; 2], [f32; 2])>,
}
impl Default for EditorOverlayStyle {
    fn default() -> Self {
        Self {
            background_color: [0.0, 0.0, 0.0, 1.0],
            pane: None,
        }
    }
}
impl EditorOverlayStyle {
    pub(crate) fn dimmed_background(&self, ui: &Ui) -> [f32; 4] {
        let mut color = bed_editing::util::color::blend(
            ui.style_color(StyleColor::Text),
            self.background_color,
            0.055,
        );
        color[3] = 1.0;
        color
    }
}

/// The composition root applies these to EditorInput/ImGui before processing the document.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OverlayOutcome {
    pub block_input: bool,
    pub suppress_next_enter: bool,
    pub clear_input_keys: bool,
}

#[derive(Default)]
pub struct EditorFinder {
    pub active: bool,
    pub query: String,
    replacement: String,
    case_sensitive: bool,
    should_focus: bool,
    matches: Vec<(i32, i32)>,
    match_index: Option<usize>,
    built_version: Option<i32>,
    built_query: String,
    built_case: bool,
    replace_field_focused: bool,
    find_field_holds_focus: bool,
    box_rect: Option<([f32; 2], [f32; 2])>,
    close_pending: bool,
    release_block_next_frame: bool,
    line_scratch: Vec<u8>,
    hay_scratch: Vec<u8>,
}

impl EditorFinder {
    pub fn open(&mut self, editor: &Editor) {
        self.active = true;
        self.should_focus = true;
        self.find_field_holds_focus = false;
        self.close_pending = false;
        self.release_block_next_frame = false;
        if editor.view.has_selection() {
            let (sr, sc, er, ec) = editor.view.primary().ordered();
            if sr == er {
                let line = editor.state.line(sr);
                self.query =
                    String::from_utf8_lossy(&line[sc.max(0) as usize..ec.max(sc) as usize])
                        .into_owned();
            }
        }
    }

    pub fn invalidate_matches(&mut self) {
        self.built_version = None;
    }

    pub fn dismiss(&mut self) {
        if !self.active {
            return;
        }
        self.active = false;
        self.box_rect = None;
        self.close_pending = true;
    }

    pub fn rebuild_matches(&mut self, editor: &Editor) {
        if self.built_version == Some(editor.state.version)
            && self.built_query == self.query
            && self.built_case == self.case_sensitive
        {
            return;
        }
        self.built_version = Some(editor.state.version);
        self.built_query.clone_from(&self.query);
        self.built_case = self.case_sensitive;
        self.match_index = None;
        self.matches.clear();
        if self.query.is_empty() {
            return;
        }
        let needle = if self.case_sensitive {
            self.query.as_bytes().to_vec()
        } else {
            self.query
                .as_bytes()
                .iter()
                .map(u8::to_ascii_lowercase)
                .collect()
        };
        for row in 0..editor.state.line_count() {
            editor
                .state
                .line_into(row, &mut self.line_scratch, usize::MAX);
            let haystack = if self.case_sensitive {
                &self.line_scratch
            } else {
                self.hay_scratch.clear();
                self.hay_scratch
                    .extend(self.line_scratch.iter().map(u8::to_ascii_lowercase));
                &self.hay_scratch
            };
            for (col, part) in haystack.windows(needle.len()).enumerate() {
                if part == needle {
                    self.matches.push((row, col as i32));
                }
            }
        }
    }

    pub fn step_match(&mut self, editor: &mut ViewContext<'_>, direction: i32) {
        self.rebuild_matches(editor);
        if self.matches.is_empty() {
            return;
        }
        let count = self.matches.len();
        let index = if let Some(index) = self.match_index {
            (index as i32 + direction).rem_euclid(count as i32) as usize
        } else {
            let best = self
                .matches
                .iter()
                .position(|p| *p >= (editor.view.row, editor.view.column))
                .unwrap_or(count - 1);
            if direction < 0 {
                (best + count - 1) % count
            } else {
                best
            }
        };
        self.match_index = Some(index);
        let (row, col) = self.matches[index];
        editor.commands().set_selection(
            row,
            col,
            row,
            col + self.query.len() as i32,
            CursorReveal::Center,
        );
    }

    fn selections(&self) -> Vec<Selection> {
        self.matches
            .iter()
            .map(|&(row, col)| Selection {
                anchor_row: row,
                anchor_column: col,
                head_row: row,
                head_column: col + self.query.len() as i32,
                preferred_column: 0,
            })
            .collect()
    }

    pub fn select_all_matches(&mut self, editor: &mut ViewContext<'_>) {
        self.rebuild_matches(editor);
        if self.matches.is_empty() {
            return;
        }
        let primary = self
            .matches
            .iter()
            .enumerate()
            .min_by_key(|(_, (row, col))| {
                i64::from((*row - editor.view.row).abs()) * 100000
                    + i64::from((*col - editor.view.column).abs())
            })
            .map(|(index, _)| index)
            .unwrap_or(0);
        editor
            .commands()
            .set_selections(self.selections(), primary, CursorReveal::Center);
        self.dismiss();
        editor.view_mut().block_input = true;
        editor.view_mut().request_focus = true;
    }

    pub fn replace_current(&mut self, editor: &mut ViewContext<'_>) {
        self.rebuild_matches(editor);
        if self.match_index.is_none() {
            self.step_match(editor, 1);
        }
        let Some(index) = self.match_index else {
            return;
        };
        let (row, col) = self.matches[index];
        let mut commands = editor.commands();
        commands.set_selection(
            row,
            col,
            row,
            col + self.query.len() as i32,
            CursorReveal::Center,
        );
        if self.replacement.is_empty() {
            commands.delete_selection();
        } else {
            commands.type_text(self.replacement.as_bytes());
        }
        self.built_version = None;
        self.match_index = None;
        drop(commands);
        self.step_match(editor, 1);
    }

    pub fn replace_all(&mut self, editor: &mut ViewContext<'_>) {
        self.rebuild_matches(editor);
        if self.matches.is_empty() {
            return;
        }
        let (row, col) = (editor.view.row, editor.view.column);
        let delta = self.replacement.len() as i32 - self.query.len() as i32;
        let corrected = col
            + self
                .matches
                .iter()
                .filter(|&&(r, c)| r == row && c < col)
                .count() as i32
                * delta;
        let mut commands = editor.commands();
        commands.set_selections(self.selections(), 0, CursorReveal::Center);
        if self.replacement.is_empty() {
            commands.delete_selection();
        } else {
            commands.type_text(self.replacement.as_bytes());
        }
        commands.set_cursor(row, corrected, false, CursorReveal::Ensure);
        self.built_version = None;
        self.match_index = None;
    }

    pub fn draw(&mut self, ui: &Ui, editor: &mut ViewContext<'_>) -> OverlayOutcome {
        self.draw_with_style(ui, editor, &EditorOverlayStyle::default())
    }

    fn consume_close(
        &mut self,
        editor: &mut ViewContext<'_>,
        return_focus: bool,
    ) -> OverlayOutcome {
        self.close_pending = false;
        self.release_block_next_frame = true;
        editor.view_mut().block_input = true;
        if return_focus {
            editor.view_mut().request_focus = true;
        }
        OverlayOutcome {
            block_input: true,
            suppress_next_enter: true,
            clear_input_keys: false,
        }
    }

    fn owning_view_focused(&self, ui: &Ui, editor: &Editor) -> bool {
        if ui.is_window_focused_with_flags(FocusedFlags::CHILD_WINDOWS) {
            return true;
        }
        if !self.should_focus && !editor.view.request_focus {
            return false;
        }
        // Native menu/tab focus initially targets the editor's container, before
        // a child widget owns navigation. Accept that explicit open/focus request
        // without treating another view in the same host as this view's focus.
        ui.with_bound_context(|| unsafe {
            let window = sys::igGetCurrentWindow();
            let navigation = (*sys::igGetCurrentContext()).NavWindow;
            !window.is_null() && !navigation.is_null() && (*window).ParentWindow == navigation
        })
    }

    pub fn draw_with_style(
        &mut self,
        ui: &Ui,
        editor: &mut ViewContext<'_>,
        style: &EditorOverlayStyle,
    ) -> OverlayOutcome {
        let owner_focused = self.owning_view_focused(ui, editor);
        if self.close_pending {
            return self.consume_close(editor, owner_focused);
        }
        if !self.active {
            self.box_rect = None;
            if self.release_block_next_frame {
                self.release_block_next_frame = false;
                editor.view_mut().block_input = false;
            }
            return OverlayOutcome::default();
        }
        self.release_block_next_frame = false;
        editor.view_mut().block_input = true;
        let outside_click = self.box_rect.is_some_and(|(min, max)| {
            ui.is_mouse_clicked(MouseButton::Left)
                && !ui.is_mouse_hovering_rect_with_clip(min, max, true)
        });
        if (owner_focused && ui.is_key_pressed(Key::Escape)) || outside_click {
            let return_focus = owner_focused
                && (!outside_click
                    || ui.is_window_hovered_with_flags(
                        WindowHoveredFlags::CHILD_WINDOWS
                            | WindowHoveredFlags::ALLOW_WHEN_BLOCKED_BY_ACTIVE_ITEM,
                    ));
            self.dismiss();
            return self.consume_close(editor, return_focus);
        }
        if self.should_focus && owner_focused {
            truncate_input(&mut self.query, 255);
            self.should_focus = false;
        }
        let fs = ui.current_font_size();
        let pad_x = fs * 0.5;
        ui.set_cursor_pos([ui.cursor_pos()[0] + pad_x, ui.cursor_pos()[1] + fs * 0.5]);
        let instance = self as *const Self;
        ui.group(|| {
            let row_width = (ui.content_region_avail()[0] - pad_x).max(fs * 6.0);
            ui.set_next_item_width(row_width * 0.5);
            {
                let _round = ui.push_style_var(StyleVar::FrameRounding(fs * 0.3));
                let _border_size = ui.push_style_var(StyleVar::FrameBorderSize(1.0));
                let _bg = ui.push_style_color(StyleColor::FrameBg, style.dimmed_background(ui));
                let _border =
                    ui.push_style_color(StyleColor::Border, ui.style_color(StyleColor::Border));
                if owner_focused && !self.find_field_holds_focus && !ui.is_any_item_active() {
                    ui.set_keyboard_focus_here();
                }
                bounded_input::<256>(
                    ui,
                    &format!("##findbox_{instance:p}"),
                    Some("Search String"),
                    &mut self.query,
                    InputTextFlags::AUTO_SELECT_ALL,
                );
                self.find_field_holds_focus = ui.is_item_focused();
            }
            if !self.query.is_empty() {
                self.rebuild_matches(editor);
                ui.same_line();
                ui.dummy([fs * 0.5, 0.0]);
                ui.same_line();
                if let Some(index) = self.match_index.filter(|_| !self.matches.is_empty()) {
                    ui.text(format!("{}/{}", index + 1, self.matches.len()));
                } else {
                    ui.text("Not Found");
                }
            }
            ui.same_line();
            ui.dummy([fs * 0.5, 0.0]);
            ui.same_line();
            let _round = ui.push_style_var(StyleVar::FrameRounding(fs * 0.3));
            let _border_size = ui.push_style_var(StyleVar::FrameBorderSize(1.0));
            let _bg = ui.push_style_color(StyleColor::FrameBg, style.dimmed_background(ui));
            let _border =
                ui.push_style_color(StyleColor::Border, ui.style_color(StyleColor::Border));
            ui.checkbox("Case Sensitive", &mut self.case_sensitive);
        });
        let (search_min, search_max) = ui.item_rect();
        ui.dummy([0.0, fs * 0.15]);
        ui.set_cursor_pos_x(ui.cursor_pos()[0] + pad_x);
        ui.group(|| {
            let row_width = (ui.content_region_avail()[0] - pad_x).max(fs * 6.0);
            ui.set_next_item_width(row_width * 0.5);
            {
                let _round = ui.push_style_var(StyleVar::FrameRounding(fs * 0.3));
                let _border_size = ui.push_style_var(StyleVar::FrameBorderSize(1.0));
                let _bg = ui.push_style_color(StyleColor::FrameBg, style.dimmed_background(ui));
                let _border =
                    ui.push_style_color(StyleColor::Border, ui.style_color(StyleColor::Border));
                bounded_input::<256>(
                    ui,
                    &format!("##replacebox_{instance:p}"),
                    Some("Replace String"),
                    &mut self.replacement,
                    InputTextFlags::empty(),
                );
                self.replace_field_focused = ui.is_item_focused();
                self.find_field_holds_focus |= self.replace_field_focused;
            }
            ui.same_line();
            ui.dummy([fs * 0.5, 0.0]);
            ui.same_line();
            let _round = ui.push_style_var(StyleVar::FrameRounding(fs * 0.3));
            let _border_size = ui.push_style_var(StyleVar::FrameBorderSize(1.0));
            let _bg = ui.push_style_color(StyleColor::FrameBg, style.dimmed_background(ui));
            let _border =
                ui.push_style_color(StyleColor::Border, ui.style_color(StyleColor::Border));
            if ui.button("Replace All") {
                self.replace_all(editor);
            }
            self.find_field_holds_focus |= ui.is_item_focused();
        });
        let (replace_min, replace_max) = ui.item_rect();
        self.box_rect = Some((
            [
                search_min[0].min(replace_min[0]),
                search_min[1].min(replace_min[1]),
            ],
            [
                search_max[0].max(replace_max[0]),
                search_max[1].max(replace_max[1]),
            ],
        ));
        ui.dummy([0.0, fs * 0.3]);
        if self.owning_view_focused(ui, editor) && ui.is_key_pressed_with_repeat(Key::Enter, false)
        {
            if self.replace_field_focused {
                self.replace_current(editor);
            } else if ui.io().key_ctrl() || ui.io().key_super() {
                self.select_all_matches(editor);
            } else {
                self.step_match(editor, if ui.io().key_shift() { -1 } else { 1 });
            }
        }
        if self.close_pending {
            self.consume_close(editor, self.owning_view_focused(ui, editor))
        } else {
            OverlayOutcome {
                block_input: true,
                ..OverlayOutcome::default()
            }
        }
    }
}

pub(crate) fn truncate_input(value: &mut String, max_bytes: usize) {
    let mut end = value.len().min(max_bytes);
    if let Some(nul) = value.as_bytes()[..end].iter().position(|&byte| byte == 0) {
        end = nul;
    }
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value.truncate(end);
}

/// Use ImGui's fixed buffer API: the String builder grows automatically beyond upstream limits.
pub(crate) fn bounded_input<const N: usize>(
    ui: &Ui,
    label: &str,
    hint: Option<&str>,
    value: &mut String,
    flags: InputTextFlags,
) -> bool {
    use std::ffi::CString;
    truncate_input(value, N - 1);
    let mut bytes = [0_u8; N];
    bytes[..value.len()].copy_from_slice(value.as_bytes());
    let label = CString::new(label).expect("Internal input label contains NUL");
    let hint = hint.map(|s| CString::new(s).expect("Internal input hint contains NUL"));
    // SAFETY: the Ui binds its context for this synchronous call. Both C strings
    // and the N-byte writable, NUL-terminated buffer live throughout the call;
    // no resize callback allows ImGui to exceed the original fixed capacity.
    let changed = ui.with_bound_context(|| unsafe {
        if let Some(hint) = hint {
            sys::igInputTextWithHint(
                label.as_ptr(),
                hint.as_ptr(),
                bytes.as_mut_ptr().cast(),
                N,
                flags.bits(),
                None,
                std::ptr::null_mut(),
            )
        } else {
            sys::igInputText(
                label.as_ptr(),
                bytes.as_mut_ptr().cast(),
                N,
                flags.bits(),
                None,
                std::ptr::null_mut(),
            )
        }
    });
    let len = bytes.iter().position(|&byte| byte == 0).unwrap_or(N - 1);
    *value = String::from_utf8_lossy(&bytes[..len]).into_owned();
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use dear_imgui_rs::{Context, FramePrepareOptions};

    #[test]
    fn matches_overlap_and_use_byte_columns() {
        let mut editor = Editor::new();
        editor.set_content("éababa ABABA".as_bytes());
        let mut finder = EditorFinder {
            query: "aba".into(),
            ..Default::default()
        };
        finder.rebuild_matches(&editor);
        assert_eq!(finder.matches, [(0, 2), (0, 4), (0, 8), (0, 10)]);
        finder.case_sensitive = true;
        finder.rebuild_matches(&editor);
        assert_eq!(finder.matches, [(0, 2), (0, 4)]);
    }

    #[test]
    fn stepping_from_caret_preserves_upstream_wrap_and_nearest_match_rules() {
        let mut editor = Editor::new();
        editor.set_content(b"x x x");
        let mut finder = EditorFinder {
            query: "x".into(),
            ..Default::default()
        };
        editor
            .commands()
            .set_cursor(0, 2, false, CursorReveal::Ensure);
        finder.step_match(&mut editor.view_context(), 1);
        assert_eq!(editor.view.primary().ordered(), (0, 2, 0, 3));
        finder.step_match(&mut editor.view_context(), 1);
        assert_eq!(editor.view.primary().ordered(), (0, 4, 0, 5));
        finder.step_match(&mut editor.view_context(), 1);
        assert_eq!(editor.view.primary().ordered(), (0, 0, 0, 1));
        editor
            .commands()
            .set_cursor(0, 5, false, CursorReveal::Ensure);
        finder.invalidate_matches();
        finder.step_match(&mut editor.view_context(), -1);
        assert_eq!(editor.view.primary().ordered(), (0, 2, 0, 3));
    }

    #[test]
    fn replace_all_is_one_undo_group_and_restores_adjusted_caret() {
        let mut editor = Editor::new();
        editor.set_content(b"aba x aba\naba");
        editor.state.path = "finder-test.txt".into();
        editor
            .commands()
            .set_cursor(0, 8, false, CursorReveal::Ensure);
        let mut finder = EditorFinder {
            query: "aba".into(),
            replacement: "q".into(),
            ..Default::default()
        };
        finder.replace_all(&mut editor.view_context());
        assert_eq!(editor.state.join(), b"q x q\nq");
        assert_eq!((editor.view.row, editor.view.column), (0, 4));
        assert_eq!(editor.view.selection_count(), 1);
        editor.commands().undo();
        assert_eq!(editor.state.join(), b"aba x aba\naba");
        editor.commands().redo();
        assert_eq!(editor.state.join(), b"q x q\nq");
    }

    #[test]
    fn empty_replacement_deletes_selected_match_and_steps_after_it() {
        let mut editor = Editor::new();
        editor.set_content(b"find then find");
        let mut finder = EditorFinder {
            query: "find".into(),
            ..Default::default()
        };
        finder.replace_current(&mut editor.view_context());
        assert_eq!(editor.state.join(), b" then find");
        assert_eq!(editor.view.primary().ordered(), (0, 6, 0, 10));
    }

    #[test]
    fn inline_ui_preserves_two_rows_tab_focus_and_close_block_lifecycle() {
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
        let mut editor = Editor::new();
        editor.set_content(b"needle");
        let mut finder = EditorFinder::default();
        finder.open(&editor);
        let render = |context: &mut Context, finder: &mut EditorFinder, editor: &mut Editor| {
            context.prepare_frame(FramePrepareOptions::new([800.0, 600.0], 1.0 / 60.0));
            let ui = context.frame();
            let mut outcome = OverlayOutcome::default();
            ui.window("Editor host")
                .size([700.0, 450.0], dear_imgui_rs::Condition::Always)
                .build(|| {
                    outcome = finder.draw(ui, &mut editor.view_context());
                    if finder.active {
                        let (min, max) = finder.box_rect.unwrap();
                        assert!(max[1] - min[1] > ui.frame_height() * 2.0);
                    }
                });
            drop(context.render_legacy());
            outcome
        };
        render(&mut context, &mut finder, &mut editor);
        render(&mut context, &mut finder, &mut editor);
        context.io_mut().add_input_characters_utf8("needle");
        render(&mut context, &mut finder, &mut editor);
        assert_eq!(finder.query, "needle");
        assert!(finder.find_field_holds_focus);
        context.io_mut().add_key_event(Key::Tab, true);
        render(&mut context, &mut finder, &mut editor);
        context.io_mut().add_key_event(Key::Tab, false);
        render(&mut context, &mut finder, &mut editor);
        assert!(finder.replace_field_focused);
        context.io_mut().add_input_characters_utf8("replacement");
        render(&mut context, &mut finder, &mut editor);
        assert_eq!(finder.replacement, "replacement");
        context.io_mut().add_key_event(Key::Escape, true);
        let closed = render(&mut context, &mut finder, &mut editor);
        assert!(!finder.active);
        assert!(closed.block_input);
        assert!(closed.suppress_next_enter);
        assert!(editor.view.request_focus);
        context.io_mut().add_key_event(Key::Escape, false);
        let released = render(&mut context, &mut finder, &mut editor);
        assert!(!released.block_input);
        assert!(!editor.view.block_input);
        finder.open(&editor);
        render(&mut context, &mut finder, &mut editor);
        render(&mut context, &mut finder, &mut editor);
        context.io_mut().add_mouse_pos_event([790.0, 590.0]);
        render(&mut context, &mut finder, &mut editor);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, true);
        let closed = render(&mut context, &mut finder, &mut editor);
        assert!(!finder.active);
        assert!(closed.block_input && closed.suppress_next_enter);
    }

    #[test]
    fn inactive_finder_in_a_sibling_view_does_not_grab_focus_or_global_keys() {
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
        let mut editor = Editor::new();
        editor.set_content(b"needle needle");
        let mut finder = EditorFinder {
            query: "needle".into(),
            ..Default::default()
        };
        finder.open(&editor);
        let original_selections = editor.view.selections.clone();
        let mut other_text = String::new();
        let render = |context: &mut Context,
                      finder: &mut EditorFinder,
                      editor: &mut Editor,
                      other_text: &mut String,
                      focus: Option<bool>| {
            context.prepare_frame(FramePrepareOptions::new([900.0, 600.0], 1.0 / 60.0));
            let ui = context.frame();
            let mut other_focused = false;
            ui.window("Shared embedding host")
                .position([20.0; 2], dear_imgui_rs::Condition::Always)
                .size([850.0, 500.0], dear_imgui_rs::Condition::Always)
                .build(|| {
                    ui.child_window("Source editor")
                        .size([400.0, 450.0])
                        .build(ui, || {
                            if focus == Some(true) {
                                ui.set_window_focus(None);
                            }
                            finder.draw(ui, &mut editor.view_context());
                        });
                    ui.same_line();
                    ui.child_window("Other editor")
                        .size([400.0, 450.0])
                        .build(ui, || {
                            if focus == Some(false) {
                                ui.set_window_focus(None);
                                ui.set_keyboard_focus_here();
                            }
                            ui.input_text("Other input", other_text)
                                .flags(InputTextFlags::ENTER_RETURNS_TRUE)
                                .build();
                            other_focused =
                                ui.is_window_focused_with_flags(FocusedFlags::CHILD_WINDOWS);
                        });
                });
            drop(context.render_legacy());
            other_focused
        };
        render(
            &mut context,
            &mut finder,
            &mut editor,
            &mut other_text,
            Some(true),
        );
        render(
            &mut context,
            &mut finder,
            &mut editor,
            &mut other_text,
            None,
        );
        assert!(finder.find_field_holds_focus);
        render(
            &mut context,
            &mut finder,
            &mut editor,
            &mut other_text,
            Some(false),
        );
        assert!(render(
            &mut context,
            &mut finder,
            &mut editor,
            &mut other_text,
            None
        ));
        context.io_mut().add_input_characters_utf8("other");
        assert!(render(
            &mut context,
            &mut finder,
            &mut editor,
            &mut other_text,
            None
        ));
        assert_eq!(other_text, "other");
        assert_eq!(finder.query, "needle");
        // Enter releases the other InputText's active ID. An unfocused Finder
        // must neither run its match command nor re-grab that now-idle focus.
        context.io_mut().add_key_event(Key::Enter, true);
        assert!(render(
            &mut context,
            &mut finder,
            &mut editor,
            &mut other_text,
            None
        ));
        assert_eq!(finder.match_index, None);
        assert_eq!(editor.view.selections, original_selections);
        context.io_mut().add_key_event(Key::Enter, false);
        for _ in 0..3 {
            assert!(render(
                &mut context,
                &mut finder,
                &mut editor,
                &mut other_text,
                None
            ));
        }
        assert!(!finder.find_field_holds_focus);
        context.io_mut().add_key_event(Key::Escape, true);
        assert!(render(
            &mut context,
            &mut finder,
            &mut editor,
            &mut other_text,
            None
        ));
        assert!(finder.active);
        assert!(!editor.view.request_focus);
        assert_eq!(editor.state.join(), b"needle needle");
    }

    #[test]
    fn fixed_inputs_enforce_native_byte_capacity() {
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
        let mut value = String::new();
        let render = |context: &mut Context, value: &mut String| {
            context.prepare_frame(FramePrepareOptions::new([640.0, 480.0], 1.0 / 60.0));
            let ui = context.frame();
            ui.window("Bounded input").build(|| {
                ui.set_keyboard_focus_here();
                bounded_input::<32>(ui, "##number", None, value, InputTextFlags::CHARS_DECIMAL);
            });
            drop(context.render_legacy());
        };
        render(&mut context, &mut value);
        render(&mut context, &mut value);
        context
            .io_mut()
            .add_input_characters_utf8("1234567890123456789012345678901234567890abc");
        render(&mut context, &mut value);
        assert_eq!(value, "1234567890123456789012345678901");
        let mut query = "雪".repeat(100);
        truncate_input(&mut query, 255);
        assert_eq!(query.len(), 255);
        let mut query = "a".repeat(254) + "🙂";
        truncate_input(&mut query, 255);
        assert_eq!(query.len(), 254);
    }
}
