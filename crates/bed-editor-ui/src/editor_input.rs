//! Keyboard/mouse to commands, translated from ned editor/editor_input.{h,cpp}.
//! See LICENSE and NOTICE for upstream attribution.
use crate::diff::RowProjection;
use crate::views::view_layout::{ViewLayout, column_at_x};
use bed_document_session::ViewContext;
#[cfg(test)]
use bed_document_session::editor::Editor;
use bed_editing::{
    editor_commands::CursorReveal,
    util::utf8::{prev_utf8_char, snap_to_utf8_char_boundary},
};
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
    pub(crate) folding_enabled: bool,
    dragging: bool,
    anchor: Option<(i32, i32)>,
    definition_request: Option<DefinitionRequest>,
    visual_navigation: Option<VisualNavigation>,
}

/// Pixel preference belongs to the view. Document selections continue to use
/// byte columns, including while a caret crosses a soft wrap.
struct VisualNavigation {
    generation: u64,
    width: u32,
    line_height: u32,
    font_metrics: (usize, u32),
    heads: Vec<(i32, i32)>,
    xs: Vec<f32>,
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
        self.process_input(ui, editor, layout, allow_navigation, None, false)
    }
    pub(crate) fn process_projected(
        &mut self,
        ui: &Ui,
        editor: &mut ViewContext<'_>,
        layout: &ViewLayout,
        allow_navigation: bool,
        projection: &crate::diff::RowProjection,
        read_only: bool,
    ) -> Vec<HostAction> {
        self.process_input(
            ui,
            editor,
            layout,
            allow_navigation,
            Some(projection),
            read_only,
        )
    }
    fn process_input(
        &mut self,
        ui: &Ui,
        editor: &mut ViewContext<'_>,
        layout: &ViewLayout,
        allow_navigation: bool,
        projection: Option<&crate::diff::RowProjection>,
        read_only: bool,
    ) -> Vec<HostAction> {
        self.definition_request = None;
        if ui.is_window_hovered_with_flags(WindowHoveredFlags::ALLOW_WHEN_BLOCKED_BY_ACTIVE_ITEM) {
            self.process_mouse(ui, editor, layout, allow_navigation, projection);
        }
        if editor.view.block_input {
            return Vec::new();
        }
        self.process_keyboard(ui, editor, layout, projection, read_only)
    }

    pub fn take_definition_request(&mut self) -> Option<DefinitionRequest> {
        self.definition_request.take()
    }

    fn process_keyboard(
        &mut self,
        ui: &Ui,
        editor: &mut ViewContext<'_>,
        layout: &ViewLayout,
        projection: Option<&RowProjection>,
        read_only: bool,
    ) -> Vec<HostAction> {
        let primary = ui.io().key_ctrl() || ui.io().key_super();
        let shift = ui.io().key_shift();
        let alt = ui.io().key_alt();
        let mut actions = Vec::new();
        let projection = projection.map(|rows| (rows, editor.ops.generation()));
        if self.folding_enabled && primary && alt {
            if ui.is_key_pressed_with_repeat(Key::LeftBracket, false) {
                self.visual_navigation = None;
                if shift {
                    editor.commands().fold_all();
                } else {
                    let row = editor.view.row;
                    editor.commands().toggle_fold(row);
                }
                return actions;
            }
            if ui.is_key_pressed_with_repeat(Key::RightBracket, false) {
                self.visual_navigation = None;
                editor.commands().unfold_all();
                return actions;
            }
        }
        if alt && !primary {
            if ui.is_key_pressed(Key::LeftArrow) {
                self.visual_navigation = None;
                editor.commands().move_word_left(shift);
            }
            if ui.is_key_pressed(Key::RightArrow) {
                self.visual_navigation = None;
                editor.commands().move_word_right(shift);
            }
            if ui.is_key_pressed(Key::UpArrow) {
                self.move_vertical(ui, editor, layout, projection, -1, shift, true);
            }
            if ui.is_key_pressed(Key::DownArrow) {
                self.move_vertical(ui, editor, layout, projection, 1, shift, true);
            }
        }
        if ui.is_key_pressed(Key::Escape) {
            self.visual_navigation = None;
            editor.commands().collapse_selection();
        }
        if ui.is_window_focused() {
            if !read_only && ui.is_key_pressed(Key::Tab) {
                if shift {
                    editor.commands().outdent();
                } else {
                    editor.commands().indent();
                }
                ui.set_keyboard_focus_here_with_offset(-1);
            }
            if primary {
                if ui.is_key_pressed(Key::A) {
                    self.visual_navigation = None;
                    editor.commands().select_all();
                }
                if !alt {
                    if ui.is_key_pressed(Key::LeftArrow) {
                        self.visual_navigation = None;
                        editor.commands().move_line_start(shift);
                    }
                    if ui.is_key_pressed(Key::RightArrow) {
                        self.visual_navigation = None;
                        editor.commands().move_line_end(shift);
                    }
                    if ui.is_key_pressed(Key::UpArrow) {
                        self.move_vertical(ui, editor, layout, projection, -5, shift, false);
                    }
                    if ui.is_key_pressed(Key::DownArrow) {
                        self.move_vertical(ui, editor, layout, projection, 5, shift, false);
                    }
                }
            }
        }

        // The upstream order is character queue, Enter/Delete, plain arrows,
        // then primary shortcuts. The queue is consumed even with a modifier.
        let text = ui.with_bound_context(take_input_characters);
        if !read_only && !text.is_empty() {
            editor.commands().type_text(text.as_bytes());
        }
        if ui.is_key_pressed(Key::Enter) {
            if self.suppress_next_enter {
                self.suppress_next_enter = false;
            } else if !read_only {
                editor.commands().insert_newline();
            }
        }
        if !read_only && ui.is_key_pressed(Key::Backspace) {
            editor.commands().delete_left(alt);
        }
        if !read_only && ui.is_key_pressed(Key::Delete) {
            editor.commands().delete_right(alt);
        }
        if !alt && !primary {
            if ui.is_key_pressed(Key::UpArrow) {
                self.move_vertical(ui, editor, layout, projection, -1, shift, false);
            }
            if ui.is_key_pressed(Key::DownArrow) {
                self.move_vertical(ui, editor, layout, projection, 1, shift, false);
            }
            if ui.is_key_pressed(Key::LeftArrow) {
                self.visual_navigation = None;
                editor.commands().move_left(shift);
            }
            if ui.is_key_pressed(Key::RightArrow) {
                self.visual_navigation = None;
                editor.commands().move_right(shift);
            }
        }
        // Additional portable navigation bindings retained by Bed.
        if ui.is_key_pressed(Key::Home) {
            self.visual_navigation = None;
            if primary {
                editor.commands().move_doc_start(shift);
            } else {
                editor.commands().move_line_start(shift);
            }
        }
        if ui.is_key_pressed(Key::End) {
            self.visual_navigation = None;
            if primary {
                editor.commands().move_doc_end(shift);
            } else {
                editor.commands().move_line_end(shift);
            }
        }
        if ui.is_key_pressed(Key::PageUp) {
            self.move_vertical(ui, editor, layout, projection, -20, shift, false);
        }
        if ui.is_key_pressed(Key::PageDown) {
            self.move_vertical(ui, editor, layout, projection, 20, shift, false);
        }
        if primary {
            if ui.is_key_pressed_with_repeat(Key::C, false) {
                let text = editor.commands().copy();
                if !text.is_empty() {
                    set_clipboard(ui, &text);
                }
            }
            if !read_only && ui.is_key_pressed_with_repeat(Key::X, false) {
                let text = editor.commands().cut();
                set_clipboard(ui, &text);
            }
            if !read_only
                && ui.is_key_pressed_with_repeat(Key::V, false)
                && let Some(text) = get_clipboard(ui)
            {
                editor.commands().paste(text.as_bytes());
            }
            if !read_only && ui.is_key_pressed(Key::Z) {
                if shift {
                    editor.commands().redo();
                } else {
                    editor.commands().undo();
                }
            }
            if !read_only && ui.is_key_pressed_with_repeat(Key::S, false) {
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

    fn move_vertical(
        &mut self,
        ui: &Ui,
        editor: &mut ViewContext<'_>,
        layout: &ViewLayout,
        projection: Option<(&RowProjection, u64)>,
        delta: i32,
        select: bool,
        add_cursor: bool,
    ) {
        let Some((projection, generation)) =
            projection.filter(|(projection, _)| projection.is_wrapped() || projection.has_folds())
        else {
            self.visual_navigation = None;
            if add_cursor {
                if delta < 0 {
                    editor.commands().add_cursor_above();
                } else {
                    editor.commands().add_cursor_below();
                }
            } else {
                editor.commands().move_lines(delta, select);
            }
            return;
        };
        // Editing precedes some navigation bindings. Reflow only when a
        // subsequent movement needs the updated text in this same frame.
        let refreshed_projection = (editor.ops.generation() != generation).then(|| {
            let mut refreshed = RowProjection::build(&editor.state, None, None);
            if self.folding_enabled {
                refreshed.fold(editor.view.folds.collapsed_ranges());
            }
            if let Some(width) = projection.wrap_width() {
                refreshed.wrap(ui, &editor.state, width);
            }
            refreshed
        });
        let projection = refreshed_projection.as_ref().unwrap_or(projection);
        let heads: Vec<_> = editor
            .view
            .selections
            .iter()
            .map(|selection| (selection.head_row, selection.head_column))
            .collect();
        let width = projection.wrap_width().map_or(0, f32::to_bits);
        let line_height = layout.line_height.to_bits();
        let font_metrics = (
            ui.with_bound_context(|| unsafe { sys::igGetFont() as usize }),
            ui.current_font_size().to_bits(),
        );
        let xs = if let Some(previous) = &self.visual_navigation
            && previous.generation == editor.ops.generation()
            && previous.width == width
            && previous.line_height == line_height
            && previous.font_metrics == font_metrics
            && previous.heads == heads
        {
            previous.xs.clone()
        } else {
            heads
                .iter()
                .map(|&(row, column)| projection.position_x(ui, &editor.state, row, column, 0.0))
                .collect()
        };
        let positions: Vec<_> = heads
            .iter()
            .zip(&xs)
            .map(|(&(row, column), &x)| {
                let start = projection.visual_position(row, column);
                let target = vertical_target(projection, start, delta);
                if target == start {
                    return (row, column);
                }
                let target_row = projection.interactive_document_row(target).unwrap();
                let column = projected_column(ui, &editor.state, projection, target, x);
                (target_row, column)
            })
            .collect();
        let mut preferences: Vec<_> = positions.iter().copied().zip(xs.iter().copied()).collect();
        if add_cursor {
            let primary = editor.view.primary_index;
            let (row, column) = positions[primary];
            editor.commands().add_cursor_at(row, column);
            preferences = heads.iter().copied().zip(xs.iter().copied()).collect();
            preferences.push(((row, column), xs[primary]));
        } else {
            editor.commands().move_carets_to(&positions, select);
        }
        // Selection merging can reorder or remove carets. Match the resulting
        // heads back to their desired X instead of retaining stale indices.
        let heads: Vec<_> = editor
            .view
            .selections
            .iter()
            .map(|selection| (selection.head_row, selection.head_column))
            .collect();
        let xs = heads
            .iter()
            .map(|&(row, column)| {
                preferences
                    .iter()
                    .find(|(position, _)| *position == (row, column))
                    .map(|(_, x)| *x)
                    .unwrap_or_else(|| projection.position_x(ui, &editor.state, row, column, 0.0))
            })
            .collect();
        self.visual_navigation = Some(VisualNavigation {
            generation: editor.ops.generation(),
            width,
            line_height,
            font_metrics,
            heads,
            xs,
        });
    }

    fn process_mouse(
        &mut self,
        ui: &Ui,
        editor: &mut ViewContext<'_>,
        layout: &ViewLayout,
        allow_navigation: bool,
        projection: Option<&crate::diff::RowProjection>,
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
        let y = (mouse[1] - layout.text_pos[1]) / layout.line_height;
        let mut visual = projection.map_or(y.floor().max(0.0) as usize, |p| p.visual_at_y(y));
        let row = if let Some(projection) = projection {
            if let Some(row) = projection.interactive_document_row(visual) {
                row
            } else if self.dragging || y >= projection.total_rows() {
                visual = nearest_editable_visual(projection, visual);
                projection.interactive_document_row(visual).unwrap()
            } else {
                return;
            }
        } else {
            (visual as i32).clamp(0, editor.state.line_count() - 1)
        };
        let line = editor.state.line(row);
        let col = if let Some(projection) = projection {
            projected_column(
                ui,
                &editor.state,
                projection,
                visual,
                mouse[0] - layout.text_pos[0],
            )
        } else {
            snap_to_utf8_char_boundary(&line, column_at_x(ui, &line, mouse[0] - layout.text_pos[0]))
        };
        // ImGui 1.92 swaps incoming physical Cmd/Ctrl when MacOSXBehaviors is
        // enabled. Hosts may configure it, so use the physical platform key.
        let navigation_modifier = if cfg!(target_os = "macos") == ui.io().config_macosx_behaviors()
        {
            ui.io().key_ctrl()
        } else {
            ui.io().key_super()
        };
        if navigation_modifier && ui.is_mouse_clicked(MouseButton::Left) {
            self.visual_navigation = None;
            self.release_mouse();
            if allow_navigation {
                self.definition_request = Some(DefinitionRequest { row, column: col });
            }
            return;
        }
        if ui.is_mouse_double_clicked(MouseButton::Left) {
            self.visual_navigation = None;
            editor.commands().select_word_at(row, col);
            return;
        }
        if ui.is_mouse_clicked(MouseButton::Left) {
            self.visual_navigation = None;
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
            self.visual_navigation = None;
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

fn vertical_target(projection: &RowProjection, start: usize, delta: i32) -> usize {
    let last = projection.len().saturating_sub(1);
    let mut target = (start as i64 + delta as i64).clamp(0, last as i64) as usize;
    while projection.interactive_document_row(target).is_none() {
        if delta < 0 && target > 0 {
            target -= 1;
        } else if delta > 0 && target < last {
            target += 1;
        } else {
            return nearest_editable_visual(projection, target);
        }
    }
    target
}

fn nearest_editable_visual(projection: &RowProjection, visual: usize) -> usize {
    let visual = visual.min(projection.len().saturating_sub(1));
    for distance in 0..projection.len() {
        if projection
            .interactive_document_row(visual + distance)
            .is_some()
        {
            return visual + distance;
        }
        if distance <= visual
            && projection
                .interactive_document_row(visual - distance)
                .is_some()
        {
            return visual - distance;
        }
    }
    unreachable!("every projection contains a document row");
}

fn projected_column(
    ui: &Ui,
    state: &bed_editing::editor_state::EditorState,
    projection: &RowProjection,
    visual: usize,
    x: f32,
) -> i32 {
    let row = projection.document_row(visual).unwrap();
    let line = state.line(row);
    let column = snap_to_utf8_char_boundary(&line, projection.hit_column(ui, state, visual, x));
    // A seam canonically belongs to the following screen row. Keep vertical
    // movement and clicks on their target row when X passes its last glyph.
    if projection
        .segment(visual)
        .is_some_and(|segment| column as usize == segment.end && segment.end < line.len())
    {
        prev_utf8_char(&line, column)
    } else {
        column
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

    #[test]
    fn vertical_navigation_skips_the_closing_body_until_it_has_finished_animating() {
        use crate::fold_animation::FoldVisual;
        use bed_editing::folding::FoldRange;

        let mut state = bed_editing::editor_state::EditorState::new();
        state.set_from_bytes(b"header\nfirst\nsecond\nthird\nafter");
        let range = FoldRange {
            start_line: 0,
            end_line: 3,
        };
        let visual = [FoldVisual {
            range,
            openness: 0.5,
        }];
        let mut projection = RowProjection::build(&state, None, None);
        projection.fold_animated(&[range], &visual);
        projection.crop_folds(&visual);
        let after = projection.visual_row(4);
        assert_eq!(projection.document_row(1), Some(1));
        assert_eq!(projection.interactive_document_row(1), None);
        assert_eq!(vertical_target(&projection, 0, 1), after);
        assert_eq!(vertical_target(&projection, after, -1), 0);
        assert_eq!(projection.visual_at_y(2.6), after);

        // Once opening, fully exposed body rows accept navigation; the bottom
        // partial row stays clipped and cannot acquire a caret yet.
        let mut opening = RowProjection::build(&state, None, None);
        opening.fold_animated(&[], &visual);
        opening.crop_folds(&visual);
        assert_eq!(vertical_target(&opening, 0, 1), 1);
        assert_eq!(vertical_target(&opening, 1, 1), opening.visual_row(4));
    }

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

    fn monospace_context() -> Context {
        use dear_imgui_rs::{FontConfig, FontSource};
        let mut context = Context::create();
        context
            .set_ini_filename(None::<std::path::PathBuf>)
            .unwrap();
        context
            .font_atlas()
            .add_font(&[FontSource::default_bitmap_with_size(13.0).with_config(
                FontConfig::new()
                    .glyph_min_advance_x(8.0)
                    .glyph_max_advance_x(8.0),
            )]);
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
        render_input(context, editor, input, focus_editor, active_item, None)
    }

    fn render_input(
        context: &mut Context,
        editor: &mut Editor,
        input: &mut EditorInput,
        focus_editor: bool,
        active_item: bool,
        wrap_columns: Option<usize>,
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
                let actions = if let Some(columns) = wrap_columns {
                    let mut projection = RowProjection::build(&editor.state, None, None);
                    let width =
                        crate::views::view_layout::glyph_advance(ui, "W") * columns as f32 + 0.1;
                    projection.wrap(ui, &editor.state, width);
                    input.process_projected(
                        ui,
                        &mut editor.view_context(),
                        &layout,
                        true,
                        &projection,
                        false,
                    )
                } else {
                    input.process(ui, &mut editor.view_context(), &layout)
                };
                assert!(actions.is_empty());
                if active_item {
                    ui.with_bound_context(|| unsafe { sys::igClearActiveID() });
                }
                rect = (min, max, layout);
            });
        drop(context.render_legacy());
        rect
    }

    fn press_wrapped(
        context: &mut Context,
        editor: &mut Editor,
        input: &mut EditorInput,
        key: Key,
    ) {
        context.io_mut().add_key_event(key, true);
        render_input(context, editor, input, true, false, Some(4));
        context.io_mut().add_key_event(key, false);
        render_input(context, editor, input, true, false, Some(4));
    }

    #[test]
    fn wrapped_vertical_navigation_preserves_x_across_short_lines() {
        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = context();
        let mut editor = Editor::new();
        let mut input = EditorInput::default();
        editor.set_content(b"WWWWWWWW\nW\nWWWWWWWW");
        editor
            .commands()
            .set_cursor(0, 3, false, CursorReveal::Ensure);
        render_input(&mut context, &mut editor, &mut input, true, false, Some(4));
        press_wrapped(&mut context, &mut editor, &mut input, Key::DownArrow);
        assert_eq!((editor.view.row, editor.view.column), (0, 7));
        press_wrapped(&mut context, &mut editor, &mut input, Key::DownArrow);
        assert_eq!((editor.view.row, editor.view.column), (1, 1));
        press_wrapped(&mut context, &mut editor, &mut input, Key::DownArrow);
        assert_eq!((editor.view.row, editor.view.column), (2, 3));
        press_wrapped(&mut context, &mut editor, &mut input, Key::UpArrow);
        assert_eq!((editor.view.row, editor.view.column), (1, 1));
        press_wrapped(&mut context, &mut editor, &mut input, Key::UpArrow);
        assert_eq!((editor.view.row, editor.view.column), (0, 7));
        assert!(editor.view.selection_empty());
    }

    #[test]
    fn wrapped_navigation_moves_multiple_carets_and_extends_selection() {
        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = context();
        let mut editor = Editor::new();
        let mut input = EditorInput::default();
        editor.set_content(b"WWWWWWWW\nWWWWWWWW");
        let selections = [0, 1]
            .into_iter()
            .map(|row| {
                let mut selection = bed_editing::editor_view_state::Selection::default();
                selection.set_both(row, 1);
                selection
            })
            .collect();
        editor
            .commands()
            .set_selections(selections, 0, CursorReveal::Ensure);
        render_input(&mut context, &mut editor, &mut input, true, false, Some(4));
        press_wrapped(&mut context, &mut editor, &mut input, Key::DownArrow);
        assert_eq!(
            editor
                .view
                .selections
                .iter()
                .map(|s| (s.head_row, s.head_column))
                .collect::<Vec<_>>(),
            vec![(0, 5), (1, 5)]
        );
        assert!(editor.view.selections.iter().all(|s| s.empty()));
        context.io_mut().add_key_event(Key::ModShift, true);
        press_wrapped(&mut context, &mut editor, &mut input, Key::UpArrow);
        assert_eq!(
            editor
                .view
                .selections
                .iter()
                .map(|s| s.ordered())
                .collect::<Vec<_>>(),
            vec![(0, 1, 0, 5), (1, 1, 1, 5)]
        );
    }

    #[test]
    fn wrapped_navigation_reflows_text_inserted_in_the_same_frame() {
        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = context();
        let mut editor = Editor::new();
        let mut input = EditorInput::default();
        editor.set_content(b"WWWWWWWW");
        editor
            .commands()
            .set_cursor(0, 3, false, CursorReveal::Ensure);
        render_input(&mut context, &mut editor, &mut input, true, false, Some(4));
        context.io_mut().add_input_characters_utf8("W");
        press_wrapped(&mut context, &mut editor, &mut input, Key::DownArrow);
        assert_eq!(editor.state.join(), b"WWWWWWWWW");
        assert_eq!((editor.view.row, editor.view.column), (0, 8));
    }

    #[test]
    fn wrapped_mouse_click_maps_a_continuation_to_document_columns() {
        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = context();
        let mut editor = Editor::new();
        let mut input = EditorInput::default();
        editor.set_content(b"WWWWWWWW");
        let (_, _, layout) =
            render_input(&mut context, &mut editor, &mut input, true, false, Some(4));
        context.io_mut().add_mouse_pos_event([
            layout.text_pos[0] + 1.0,
            layout.text_pos[1] + 1.5 * layout.line_height,
        ]);
        render_input(&mut context, &mut editor, &mut input, true, false, Some(4));
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, true);
        render_input(&mut context, &mut editor, &mut input, true, false, Some(4));
        assert_eq!((editor.view.row, editor.view.column), (0, 4));
    }

    #[test]
    fn wrapped_mouse_clicks_preserve_indentation_and_utf8_columns() {
        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        for (text, indent_columns) in [("  éééééééééé", 2.0), (" \téééééééééé", 4.0)]
        {
            let mut context = monospace_context();
            let mut editor = Editor::new();
            let mut input = EditorInput::default();
            editor.set_content(text.as_bytes());
            context.prepare_frame(FramePrepareOptions::new([640.0, 480.0], 1.0));
            let ui = context.frame();
            let advance = crate::views::view_layout::glyph_advance(ui, "W");
            assert_eq!(crate::views::view_layout::glyph_advance(ui, " "), advance);
            assert_eq!(crate::views::view_layout::glyph_advance(ui, "é"), advance);
            let mut projection = RowProjection::build(&editor.state, None, None);
            projection.wrap(ui, &editor.state, advance * 8.0 + 0.1);
            let continuation = projection.visual_row(0) + 1;
            let range = projection.segment(continuation).unwrap();
            assert_eq!(
                &text.as_bytes()[range.start..range.start + 2],
                "é".as_bytes()
            );
            drop(context.render_legacy());

            let (_, _, layout) =
                render_input(&mut context, &mut editor, &mut input, true, false, Some(8));
            let indent = indent_columns * advance;
            // Clicking the visual padding stays at the real segment start.
            // Clicking through the first glyph advances by its UTF-8 byte length.
            for (x, expected_column) in [
                (indent / 2.0, range.start),
                (indent + advance, range.start + 2),
            ] {
                context.io_mut().add_mouse_pos_event([
                    layout.text_pos[0] + x,
                    layout.text_pos[1] + (continuation as f32 + 0.5) * layout.line_height,
                ]);
                render_input(&mut context, &mut editor, &mut input, true, false, Some(8));
                context
                    .io_mut()
                    .add_mouse_button_event(MouseButton::Left, true);
                render_input(&mut context, &mut editor, &mut input, true, false, Some(8));
                assert_eq!(
                    (editor.view.row, editor.view.column),
                    (0, expected_column as i32)
                );
                assert!(editor.view.selection_empty());
                context
                    .io_mut()
                    .add_mouse_button_event(MouseButton::Left, false);
                render_input(&mut context, &mut editor, &mut input, true, false, Some(8));
            }
            assert_eq!(editor.state.join(), text.as_bytes());
        }
    }

    #[test]
    fn wrapped_vertical_navigation_preserves_x_through_different_indents() {
        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = monospace_context();
        let mut editor = Editor::new();
        let mut input = EditorInput::default();
        editor.set_content("  éééééééééé\n W\n    WWWWWWWWW\nWWWW".as_bytes());
        context.prepare_frame(FramePrepareOptions::new([640.0, 480.0], 1.0));
        let ui = context.frame();
        let width = crate::views::view_layout::glyph_advance(ui, "W") * 8.0 + 0.1;
        let mut projection = RowProjection::build(&editor.state, None, None);
        projection.wrap(ui, &editor.state, width);
        let start = projection.visual_row(1) - 1;
        let start_column = projection.segment(start).unwrap().start + "é".len();
        let first_deeper_line = projection.visual_row(2);
        let last_deeper_line = projection.visual_row(3) - 1;
        assert!(last_deeper_line > first_deeper_line);
        drop(context.render_legacy());
        editor
            .commands()
            .set_cursor(0, start_column as i32, false, CursorReveal::Ensure);
        render_input(&mut context, &mut editor, &mut input, true, false, Some(8));

        let mut positions = vec![(0, start_column as i32), (1, 2), (2, 3)];
        // The desired X is three cells: beyond the short second line, but
        // before the four-cell padding on the third line's continuations.
        positions.extend(
            (first_deeper_line + 1..=last_deeper_line)
                .map(|visual| (2, projection.segment(visual).unwrap().start as i32)),
        );
        positions.push((3, 3));
        for &(row, column) in &positions[1..] {
            context.io_mut().add_key_event(Key::DownArrow, true);
            render_input(&mut context, &mut editor, &mut input, true, false, Some(8));
            context.io_mut().add_key_event(Key::DownArrow, false);
            render_input(&mut context, &mut editor, &mut input, true, false, Some(8));
            assert_eq!((editor.view.row, editor.view.column), (row, column));
        }
        for &(row, column) in positions[..positions.len() - 1].iter().rev() {
            context.io_mut().add_key_event(Key::UpArrow, true);
            render_input(&mut context, &mut editor, &mut input, true, false, Some(8));
            context.io_mut().add_key_event(Key::UpArrow, false);
            render_input(&mut context, &mut editor, &mut input, true, false, Some(8));
            assert_eq!((editor.view.row, editor.view.column), (row, column));
        }
        assert!(editor.view.selection_empty());
    }

    #[test]
    fn wrapped_page_navigation_counts_screen_rows() {
        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = context();
        let mut editor = Editor::new();
        let mut input = EditorInput::default();
        editor.set_content(&vec![b'W'; 120]);
        editor
            .commands()
            .set_cursor(0, 1, false, CursorReveal::Ensure);
        render_input(&mut context, &mut editor, &mut input, true, false, Some(4));
        press_wrapped(&mut context, &mut editor, &mut input, Key::PageDown);
        assert_eq!((editor.view.row, editor.view.column), (0, 81));
        press_wrapped(&mut context, &mut editor, &mut input, Key::PageUp);
        assert_eq!((editor.view.row, editor.view.column), (0, 1));
    }

    #[test]
    fn wrapped_alt_down_adds_a_caret_on_the_next_screen_row() {
        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = context();
        let mut editor = Editor::new();
        let mut input = EditorInput::default();
        editor.set_content(b"WWWWWWWW");
        editor
            .commands()
            .set_cursor(0, 1, false, CursorReveal::Ensure);
        render_input(&mut context, &mut editor, &mut input, true, false, Some(4));
        context.io_mut().add_key_event(Key::ModAlt, true);
        press_wrapped(&mut context, &mut editor, &mut input, Key::DownArrow);
        assert_eq!(editor.view.selection_count(), 2);
        assert_eq!((editor.view.row, editor.view.column), (0, 5));
        assert!(editor.view.selections.iter().all(|s| s.empty()));
    }

    #[test]
    fn projected_navigation_skips_history_and_hunk_controls() {
        let mut editor = Editor::new();
        editor.set_content(b"a\nb");
        let diff = crate::diff::DiffPresentation {
            baseline: std::sync::Arc::from(&b"a\nremoved\nb"[..]),
            comparison: 1,
            actions: Vec::new(),
            actions_enabled: true,
            saved: None,
        };
        let projection = RowProjection::build(&editor.state, Some(&diff), None);
        let first = projection.visual_row(0);
        let last = projection.visual_row(1);
        assert!(last > first + 1);
        assert_eq!(vertical_target(&projection, first, 1), last);
        assert_eq!(vertical_target(&projection, last, -1), first);
        assert_eq!(nearest_editable_visual(&projection, last - 1), last);
    }

    #[test]
    fn projected_paging_reaches_first_and_last_editable_rows() {
        let mut editor = Editor::new();
        editor.set_content(b"a\nb");
        for baseline in [&b"a\nb\nremoved"[..], &b"removed\na\nb"[..]] {
            let diff = crate::diff::DiffPresentation {
                baseline: std::sync::Arc::from(baseline),
                comparison: 1,
                actions: Vec::new(),
                actions_enabled: true,
                saved: None,
            };
            let projection = RowProjection::build(&editor.state, Some(&diff), None);
            let first = projection.visual_row(0);
            let last = projection.visual_row(1);
            assert_eq!(vertical_target(&projection, first, 20), last);
            assert_eq!(vertical_target(&projection, last, -20), first);
        }
    }

    #[test]
    fn wrapped_navigation_recomputes_x_when_same_size_font_changes() {
        use dear_imgui_rs::{FontConfig, FontSource};
        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = Context::create();
        context
            .set_ini_filename(None::<std::path::PathBuf>)
            .unwrap();
        let narrow = context
            .font_atlas()
            .add_font(&[FontSource::default_bitmap_with_size(13.0)]);
        let wide = context
            .font_atlas()
            .add_font(&[FontSource::default_bitmap_with_size(13.0).with_config(
                FontConfig::new()
                    .glyph_min_advance_x(12.0)
                    .glyph_max_advance_x(12.0),
            )]);
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        let mut editor = Editor::new();
        let mut input = EditorInput::default();
        editor.set_content(b"WWWWWWWWWWWWWWWW");
        editor
            .commands()
            .set_cursor(0, 2, false, CursorReveal::Ensure);
        context.prepare_frame(FramePrepareOptions::new([640.0, 480.0], 1.0));
        let ui = context.frame();
        ui.window("Font navigation").build(|| {
            let font = ui.push_font_with_size(Some(narrow), 13.0);
            let width = crate::views::view_layout::glyph_advance(ui, "W") * 4.0 + 0.1;
            let layout = ViewLayout {
                line_height: ui.text_line_height(),
                ..Default::default()
            };
            let mut projection = RowProjection::build(&editor.state, None, None);
            projection.wrap(ui, &editor.state, width);
            let generation = editor.ops.generation();
            input.move_vertical(
                ui,
                &mut editor.view_context(),
                &layout,
                Some((&projection, generation)),
                1,
                false,
                false,
            );
            assert_eq!(editor.view.column, 6);
            drop(font);
            let _font = ui.push_font_with_size(Some(wide), 13.0);
            assert_eq!(layout.line_height, ui.text_line_height());
            let mut projection = RowProjection::build(&editor.state, None, None);
            projection.wrap(ui, &editor.state, width);
            input.move_vertical(
                ui,
                &mut editor.view_context(),
                &layout,
                Some((&projection, generation)),
                1,
                false,
                false,
            );
            assert_eq!(editor.view.column, 8);
        });
        drop(context.render_legacy());
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
