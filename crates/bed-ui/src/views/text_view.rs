//! Document text, selection, indentation guides and current-line painting.
//! Translated from ned editor/views/text_view.{h,cpp}; see LICENSE and NOTICE.
//! Diagnostic marks are added with the diagnostics service in the LSP stage.
use crate::views::view_layout::{ViewLayout, glyph_advance, glyph_advance_bytes};
use bed_core::{editor_state::EditorState, editor_view_state::EditorViewState};
use bed_highlight::highlight_service::EditorHighlight;
use dear_imgui_rs::{Ui, sys};

struct TextRun {
    start: usize,
    end: usize,
    pos: [f32; 2],
    color: [f32; 4],
}

fn flush_run(ui: &Ui, line: &[u8], run: &mut Option<TextRun>) {
    let Some(run) = run.take() else {
        return;
    };
    ui.with_bound_context(|| {
        // SAFETY: the active Ui owns this window's draw list; the byte range is
        // bounded by the borrowed line and AddText consumes it synchronously.
        // Passing bytes preserves ImGui's decoder behavior for malformed files.
        unsafe {
            let start = line.as_ptr().add(run.start).cast();
            let end = line.as_ptr().add(run.end).cast();
            sys::ImDrawList_AddText_Vec2(
                sys::igGetWindowDrawList(),
                run.pos.into(),
                sys::igColorConvertFloat4ToU32(run.color.into()),
                start,
                end,
            );
        }
    });
}

fn native_color(ui: &Ui, color: [f32; 4]) -> u32 {
    ui.with_bound_context(|| unsafe { sys::igColorConvertFloat4ToU32(color.into()) })
}

pub struct TextView;

impl TextView {
    pub fn draw_diagnostics(
        ui: &Ui,
        state: &EditorState,
        layout: &ViewLayout,
        diagnostics: &bed_lsp::diagnostics::diagnostics_store::LspDiagnostics,
        hover: super::hover_trigger::Info,
        arbiter: &super::hover_tooltip::TooltipArbiter,
    ) {
        use crate::views::{
            diagnostic_style::severity_mark, hover_tooltip::render_diagnostic_tooltip,
            hover_trigger::Zone, view_layout::line_column_x,
        };
        use bed_core::util::utf8::{utf8_byte_offset_to_utf16, utf16_to_utf8_byte_offset};
        if layout.line_height <= 0.0 || state.path.is_empty() {
            return;
        }
        let first = ((ui.scroll_y() / layout.line_height) as i32 - 2).max(0);
        let last = (((ui.scroll_y() + ui.window_height()) / layout.line_height) as i32 + 2)
            .min(state.line_count() - 1);
        let draw = ui.get_window_draw_list();
        let window = ui.window_pos();
        for item in diagnostics.for_document(&state.path) {
            if item.end_line < first || item.start_line > last {
                continue;
            }
            for row in item.start_line.max(first)..=item.end_line.min(last) {
                let line = state.line(row);
                let mut start = if row == item.start_line {
                    utf16_to_utf8_byte_offset(&line, item.start_character)
                } else {
                    0
                };
                let mut end = if row == item.end_line {
                    utf16_to_utf8_byte_offset(&line, item.end_character)
                } else {
                    line.len() as i32
                };
                if end < start {
                    std::mem::swap(&mut start, &mut end);
                }
                let mut x0 = line_column_x(ui, &line, start, layout.text_pos[0]);
                let mut x1 = line_column_x(ui, &line, end, layout.text_pos[0]);
                if x1 - x0 < 4.0 {
                    x1 = x0 + 8.0;
                }
                if x1 <= x0 {
                    x1 = x0 + 6.0;
                }
                x0 = x0.max(window[0]);
                x1 = x1.min(window[0] + ui.window_width());
                if x1 <= x0 {
                    continue;
                }
                let y =
                    layout.text_pos[1] + row as f32 * layout.line_height + layout.line_height - 3.0;
                let mut previous = [x0, y];
                let mut x = x0 + 2.0;
                while x <= x1 + 0.01 {
                    let next = [x.min(x1), y + ((x - x0) * 1.2).sin() * 1.25];
                    draw.add_line(previous, next, severity_mark(item.severity))
                        .thickness(1.4)
                        .build();
                    previous = next;
                    x += 2.0;
                }
            }
        }
        if hover.active && hover.zone == Zone::Text {
            let utf16 = utf8_byte_offset_to_utf16(&state.line(hover.row), hover.column);
            if diagnostics.contains(&state.path, hover.row, utf16) {
                render_diagnostic_tooltip(
                    ui,
                    &diagnostics.for_line(&state.path, hover.row),
                    arbiter,
                );
            }
        }
    }
    pub fn draw(ui: &Ui, state: &EditorState, view: &EditorViewState, layout: &ViewLayout) {
        Self::draw_with_highlight(ui, state, view, layout, None);
    }
    pub fn draw_with_highlight(
        ui: &Ui,
        state: &EditorState,
        view: &EditorViewState,
        layout: &ViewLayout,
        highlight: Option<&EditorHighlight>,
    ) {
        if layout.line_height <= 0.0 || state.line_count() <= 0 {
            return;
        }
        let draw = ui.get_window_draw_list();
        let first = ((ui.scroll_y() / layout.line_height) as i32 - 2).max(0);
        let last = (((ui.scroll_y() + ui.window_height()) / layout.line_height) as i32 + 2)
            .min(state.line_count() - 1);
        let window_pos = ui.window_pos();
        if view.row >= first && view.row <= last {
            let y = layout.text_pos[1] + view.row as f32 * layout.line_height;
            draw.add_rect(
                [window_pos[0] + 6.0, y],
                [window_pos[0] + ui.window_width(), y + layout.line_height],
                native_color(ui, [0.5, 0.5, 0.5, 0.08]),
            )
            .filled(true)
            .build();
        }
        let origin_x = layout.text_pos[0];
        let clip_right = window_pos[0] + ui.window_width();
        let space_width = glyph_advance(ui, " ");
        let selection_color = native_color(ui, [1.0, 0.1, 0.7, 0.3]);
        let guide_color = native_color(ui, [0.3, 0.3, 0.3, 0.4]);
        let mut line = Vec::new();
        for row in first..=last {
            state.line_into(row, &mut line, usize::MAX);
            let y = layout.text_pos[1] + row as f32 * layout.line_height;
            let whitespace_units: usize = line
                .iter()
                .take_while(|c| **c == b' ' || **c == b'\t')
                .map(|c| if *c == b'\t' { 4 } else { 1 })
                .sum();
            for level in 1..whitespace_units.div_ceil(4) {
                let x = origin_x + level as f32 * 4.0 * space_width;
                if x >= clip_right {
                    break;
                }
                if x >= window_pos[0] {
                    draw.add_line([x, y - 2.0], [x, y - 2.0 + layout.line_height], guide_color)
                        .build();
                }
            }
            let mut x = origin_x;
            let spans = highlight
                .filter(|h| h.enabled)
                .map_or(&[][..], |h| h.spans_for_line(row));
            let mut span_index = 0;
            let default_color = highlight.map_or_else(
                || ui.style_color(dear_imgui_rs::StyleColor::Text),
                EditorHighlight::default_text_color,
            );
            let mut run: Option<TextRun> = None;
            let mut start = 0;
            while start < line.len() {
                if line[start] & 0xc0 == 0x80 {
                    start += 1;
                    continue;
                }
                while span_index < spans.len() && spans[span_index].end <= start as i32 {
                    span_index += 1;
                }
                let color = if let Some(span) = spans
                    .get(span_index)
                    .filter(|span| span.start <= start as i32)
                {
                    highlight.unwrap().color_for_slot(span.slot)
                } else {
                    default_color
                };
                let is_tab = line[start] == b'\t';
                let mut end = start + 1;
                if !is_tab && line[start] & 0x80 != 0 {
                    while end < line.len() && line[end] & 0xc0 == 0x80 {
                        end += 1;
                    }
                }
                let width = if is_tab {
                    let column = ((x - origin_x) / space_width) as i32;
                    (((column / 4) + 1) * 4 - column) as f32 * space_width
                } else {
                    glyph_advance_bytes(ui, &line[start..end])
                };
                if x > clip_right {
                    flush_run(ui, &line, &mut run);
                    break;
                }
                let visible = x + width >= window_pos[0];
                if visible && view.is_position_selected(row, start as i32) {
                    draw.add_rect([x, y], [x + width, y + layout.line_height], selection_color)
                        .filled(true)
                        .build();
                }
                if !visible || is_tab {
                    flush_run(ui, &line, &mut run);
                } else if run
                    .as_ref()
                    .is_none_or(|run| run.color != color || run.end != start)
                {
                    flush_run(ui, &line, &mut run);
                    run = Some(TextRun {
                        start,
                        end,
                        pos: [x, y],
                        color,
                    });
                } else if let Some(run) = &mut run {
                    run.end = end;
                }
                x += width;
                // advanceUtf8 skips following continuation bytes even after an
                // ASCII byte; glyph measurement above only groups them for UTF-8.
                start += 1;
                while start < line.len() && line[start] & 0xc0 == 0x80 {
                    start += 1;
                }
            }
            flush_run(ui, &line, &mut run);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bed_core::editor_commands::CursorReveal;
    use bed_session::editor::Editor;
    use dear_imgui_rs::{Condition, Context, FramePrepareOptions, StyleColor};

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

    fn vertices(ui: &Ui) -> Vec<sys::ImDrawVert> {
        ui.with_bound_context(|| {
            // Copy the native buffer while this frame owns it. No draw or font
            // mutation runs until the slice is copied, and no reference escapes.
            unsafe {
                let buffer = &(*sys::igGetWindowDrawList()).VtxBuffer;
                if buffer.Size == 0 {
                    Vec::new()
                } else {
                    std::slice::from_raw_parts(buffer.Data, buffer.Size as usize).to_vec()
                }
            }
        })
    }

    fn render(context: &mut Context, draw: impl FnOnce(&Ui, &ViewLayout)) {
        context.prepare_frame(FramePrepareOptions::new([640.0, 480.0], 1.0 / 60.0));
        let ui = context.frame();
        ui.window("Text renderer")
            .position([20.0, 20.0], Condition::Always)
            .size([600.0, 400.0], Condition::Always)
            .flags(dear_imgui_rs::WindowFlags::NO_TITLE_BAR)
            .build(|| {
                draw(
                    ui,
                    &ViewLayout {
                        text_pos: [40.25, 50.5],
                        line_height: ui.text_line_height(),
                        ..Default::default()
                    },
                );
            });
        drop(context.render_legacy());
    }

    #[test]
    fn same_color_run_retains_fractional_advances_of_one_native_text_call() {
        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = context();
        let mut editor = Editor::new();
        editor.set_content(b"AAAA");
        // Hide the current-line rectangle to compare just the glyph geometry.
        editor.view.row = -1;
        render(&mut context, |ui, layout| {
            ui.with_bound_context(|| {
                // A deliberately fractional metric exercises the behavior of
                // proportional/scaled fonts without depending on rasterizer
                // hinting. This test context exclusively owns its baked font.
                unsafe {
                    let baked = sys::igGetFontBaked();
                    let glyph = sys::ImFontBaked_FindGlyph(baked, b'A'.into());
                    (*glyph).AdvanceX = 7.25;
                    *(*baked).IndexAdvanceX.Data.add(b'A' as usize) = 7.25;
                }
            });
            let before = vertices(ui).len();
            TextView::draw(ui, &editor.state, &editor.view, layout);
            let actual = vertices(ui)[before..].to_vec();
            assert_eq!(actual.len(), 16);
            ui.get_window_draw_list().add_text(
                layout.text_pos,
                ui.style_color(StyleColor::Text),
                "AAAA",
            );
            let reference = vertices(ui)[before + actual.len()..].to_vec();
            assert_eq!(actual, reference);
            assert_eq!(actual[4].pos.x - actual[0].pos.x, 7.25);
        });
    }

    #[test]
    fn selected_glyph_rectangles_precede_their_whole_text_run() {
        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = context();
        let mut editor = Editor::new();
        editor.set_content(b"ABCD");
        editor
            .commands()
            .set_selection(0, 1, 0, 3, CursorReveal::Ensure);
        render(&mut context, |ui, layout| {
            let before = vertices(ui).len();
            TextView::draw(ui, &editor.state, &editor.view, layout);
            let actual = vertices(ui)[before..].to_vec();
            let selection_color = ui.with_bound_context(|| unsafe {
                sys::igColorConvertFloat4ToU32([1.0, 0.1, 0.7, 0.3].into())
            });
            let text_color = ui.with_bound_context(|| unsafe {
                sys::igColorConvertFloat4ToU32(ui.style_color(StyleColor::Text).into())
            });
            let last_selection = actual
                .iter()
                .rposition(|vertex| vertex.col == selection_color)
                .unwrap();
            let first_text = actual
                .iter()
                .position(|vertex| vertex.col == text_color)
                .unwrap();
            assert_eq!(
                actual.iter().filter(|v| v.col == selection_color).count(),
                8
            );
            assert!(last_selection < first_text);
        });
    }

    #[test]
    fn tabs_and_stray_utf8_continuations_preserve_source_glyph_positions() {
        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = context();
        let mut editor = Editor::new();
        editor.set_content(b"\x80A\x80\tB");
        editor.view.row = -1;
        render(&mut context, |ui, layout| {
            let before = vertices(ui).len();
            TextView::draw(ui, &editor.state, &editor.view, layout);
            let actual = vertices(ui)[before..].to_vec();
            assert_eq!(actual.len(), 8);
            let draw = ui.get_window_draw_list();
            let color = ui.style_color(StyleColor::Text);
            draw.add_text(layout.text_pos, color, "A");
            draw.add_text(
                [
                    layout.text_pos[0] + 4.0 * glyph_advance(ui, " "),
                    layout.text_pos[1],
                ],
                color,
                "B",
            );
            let reference = vertices(ui)[before + actual.len()..].to_vec();
            assert_eq!(actual, reference);
        });
    }
}
