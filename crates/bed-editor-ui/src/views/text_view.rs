//! Document text, selection, indentation guides and current-line painting.
//! Translated from ned editor/views/text_view.{h,cpp}; see LICENSE and NOTICE.
//! Diagnostic marks are added with the diagnostics service in the LSP stage.
use crate::diff::{DiffLineKind, ProjectedRow, RowProjection};
use crate::views::view_layout::{ViewLayout, glyph_advance, glyph_advance_bytes};
use bed_editing::{editor_state::EditorState, editor_view_state::EditorViewState};
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
        diagnostics: &[bed_editing::diagnostic::DiagnosticItem],
        hover: super::hover_trigger::Info,
        arbiter: &super::hover_tooltip::TooltipArbiter,
    ) {
        Self::draw_projected_diagnostics(ui, state, layout, diagnostics, hover, arbiter, None);
    }
    pub(crate) fn draw_projected_diagnostics(
        ui: &Ui,
        state: &EditorState,
        layout: &ViewLayout,
        diagnostics: &[bed_editing::diagnostic::DiagnosticItem],
        hover: super::hover_trigger::Info,
        arbiter: &super::hover_tooltip::TooltipArbiter,
        projection: Option<&RowProjection>,
    ) {
        use crate::views::{
            diagnostic_style::severity_color, hover_tooltip::render_diagnostic_tooltip,
            hover_trigger::Zone, view_layout::line_column_x,
        };
        use bed_editing::{
            diagnostic::diagnostic_contains,
            util::utf8::{utf8_byte_offset_to_utf16, utf16_to_utf8_byte_offset},
        };
        if layout.line_height <= 0.0 || state.path.is_empty() {
            return;
        }
        let first_visual = ((ui.scroll_y() / layout.line_height) as i32 - 2).max(0);
        let last_visual = ((ui.scroll_y() + ui.window_height()) / layout.line_height) as i32 + 2;
        let first = projection.map_or(first_visual, |p| {
            p.nearest_document_row(first_visual as usize)
        });
        let last = projection
            .map_or(last_visual, |p| {
                p.nearest_document_row(last_visual as usize)
            })
            .min(state.line_count() - 1);
        let draw = ui.get_window_draw_list();
        let window = ui.window_pos();
        let severity_colors: [u32; 4] = std::array::from_fn(|index| {
            native_color(
                ui,
                bed_ui::presentation::readable_color(ui, severity_color(index as i32 + 1)),
            )
        });
        for item in diagnostics {
            if item.end_line < first || item.start_line > last {
                continue;
            }
            let color = severity_colors[(item.severity.clamp(1, 4) - 1) as usize];
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
                let first_row = projection.map_or(row as usize, |p| p.visual_row(row));
                let last_row =
                    projection.map_or(row as usize, |p| p.visual_position(row, line.len() as i32));
                for visual in
                    first_row.max(first_visual as usize)..=last_row.min(last_visual as usize)
                {
                    let segment = projection
                        .and_then(|p| p.segment(visual))
                        .unwrap_or(0..line.len());
                    if start == end {
                        // A zero-width diagnostic belongs to the same visual
                        // row as its caret, including at a wrap boundary.
                        if projection.is_some_and(|p| p.visual_position(row, start) != visual) {
                            continue;
                        }
                    } else if end <= segment.start as i32 || start >= segment.end as i32 {
                        continue;
                    }
                    let column_start = start.max(segment.start as i32) - segment.start as i32;
                    let column_end = end.min(segment.end as i32) - segment.start as i32;
                    let origin_x =
                        layout.text_pos[0] + projection.map_or(0.0, |p| p.segment_indent(visual));
                    let bytes = &line[segment];
                    let mut x0 = line_column_x(ui, bytes, column_start, origin_x);
                    let mut x1 = line_column_x(ui, bytes, column_end, origin_x);
                    if x1 - x0 < 4.0 {
                        x1 = x0 + 8.0;
                    }
                    x0 = x0.max(window[0]);
                    x1 = x1.min(window[0] + ui.window_width());
                    if x1 <= x0 {
                        continue;
                    }
                    let y = layout.text_pos[1] + (visual as f32 + 1.0) * layout.line_height - 3.0;
                    let mut previous = [x0, y];
                    let mut x = x0 + 2.0;
                    while x <= x1 + 0.01 {
                        let next = [x.min(x1), y + ((x - x0) * 1.2).sin() * 1.25];
                        draw.add_line(previous, next, color).thickness(1.4).build();
                        previous = next;
                        x += 2.0;
                    }
                }
            }
        }
        if hover.active && hover.zone == Zone::Text {
            let utf16 = utf8_byte_offset_to_utf16(&state.line(hover.row), hover.column);
            if diagnostics
                .iter()
                .any(|item| diagnostic_contains(item, hover.row, utf16))
            {
                let items: Vec<_> = diagnostics
                    .iter()
                    .filter(|item| item.start_line <= hover.row && hover.row <= item.end_line)
                    .cloned()
                    .collect();
                render_diagnostic_tooltip(ui, &items, arbiter);
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
        Self::draw_with_debug_highlight(ui, state, view, layout, highlight, None);
    }

    pub fn draw_with_debug_highlight(
        ui: &Ui,
        state: &EditorState,
        view: &EditorViewState,
        layout: &ViewLayout,
        highlight: Option<&EditorHighlight>,
        execution_row: Option<i32>,
    ) {
        Self::draw_projected(ui, state, view, layout, highlight, execution_row, None);
    }
    pub(crate) fn draw_projected(
        ui: &Ui,
        state: &EditorState,
        view: &EditorViewState,
        layout: &ViewLayout,
        highlight: Option<&EditorHighlight>,
        execution_row: Option<i32>,
        projection: Option<&RowProjection>,
    ) {
        if layout.line_height <= 0.0 || state.line_count() <= 0 {
            return;
        }
        let draw = ui.get_window_draw_list();
        let first = ((ui.scroll_y() / layout.line_height) as i32 - 2).max(0);
        let last = (((ui.scroll_y() + ui.window_height()) / layout.line_height) as i32 + 2)
            .min(projection.map_or(state.line_count(), |p| p.len() as i32) - 1);
        let window_pos = ui.window_pos();
        let caret_visual = projection.map_or(view.row, |p| {
            p.visual_position(view.row, view.column) as i32
        });
        if caret_visual >= first && caret_visual <= last {
            let y = layout.text_pos[1] + caret_visual as f32 * layout.line_height;
            draw.add_rect(
                [window_pos[0] + 6.0, y],
                [window_pos[0] + ui.window_width(), y + layout.line_height],
                native_color(ui, {
                    let mut color = ui.style_color(dear_imgui_rs::StyleColor::Text);
                    color[3] = 0.055;
                    color
                }),
            )
            .filled(true)
            .build();
        }
        if let Some(row) = execution_row.filter(|row| (0..state.line_count()).contains(row)) {
            let start = projection.map_or(row, |p| p.visual_row(row) as i32);
            let end = projection.map_or(row, |p| {
                p.visual_position(row, state.line_length(row)) as i32
            });
            for visual in start.max(first)..=end.min(last) {
                let y = layout.text_pos[1] + visual as f32 * layout.line_height;
                draw.add_rect(
                    [window_pos[0] + 6.0, y],
                    [window_pos[0] + ui.window_width(), y + layout.line_height],
                    native_color(ui, [0.95, 0.68, 0.12, 0.19]),
                )
                .filled(true)
                .build();
            }
        }
        let base_origin_x = layout.text_pos[0];
        let clip_right = window_pos[0] + ui.window_width();
        let space_width = glyph_advance(ui, " ");
        let selection_color = native_color(
            ui,
            ui.style_color(dear_imgui_rs::StyleColor::TextSelectedBg),
        );
        let guide_color = native_color(ui, {
            let mut color = ui.style_color(dear_imgui_rs::StyleColor::Text);
            color[3] = 0.18;
            color
        });
        let mut line = Vec::new();
        let mut loaded_line = None;
        for visual in first..=last {
            let (row, historical, added) =
                match projection.and_then(|p| p.rows.get(visual as usize)) {
                    Some(ProjectedRow::Document { row, kind, .. }) => {
                        if loaded_line != Some((false, *row as usize)) {
                            state.line_into(*row, &mut line, usize::MAX);
                            loaded_line = Some((false, *row as usize));
                        }
                        (*row, false, *kind == DiffLineKind::Added)
                    }
                    Some(ProjectedRow::Historical { old_row, bytes }) => {
                        if loaded_line != Some((true, *old_row)) {
                            line.clear();
                            line.extend_from_slice(bytes);
                            loaded_line = Some((true, *old_row));
                        }
                        (-1, true, false)
                    }
                    Some(_) => continue,
                    None => {
                        state.line_into(visual, &mut line, usize::MAX);
                        loaded_line = Some((false, visual as usize));
                        (visual, false, false)
                    }
                };
            let segment = projection
                .and_then(|p| p.segment(visual as usize))
                .unwrap_or(0..line.len());
            let indent = projection.map_or(0.0, |p| p.segment_indent(visual as usize));
            let origin_x = base_origin_x + indent;
            let y = layout.text_pos[1] + visual as f32 * layout.line_height;
            if historical || added {
                draw.add_rect(
                    [window_pos[0], y],
                    [clip_right, y + layout.line_height],
                    native_color(
                        ui,
                        if historical {
                            [0.85, 0.24, 0.26, 0.15]
                        } else {
                            [0.2, 0.7, 0.35, 0.15]
                        },
                    ),
                )
                .filled(true)
                .build();
            }
            let whitespace_width = if segment.start == 0 {
                let end = line[..segment.end]
                    .iter()
                    .take_while(|c| **c == b' ' || **c == b'\t')
                    .count();
                super::view_layout::line_column_x(ui, &line, end as i32, 0.0)
            } else {
                indent
            };
            let mut level = 1;
            while level as f32 * 4.0 * space_width < whitespace_width - 0.01 {
                let x = base_origin_x + level as f32 * 4.0 * space_width;
                if x >= clip_right {
                    break;
                }
                if x >= window_pos[0] {
                    draw.add_line([x, y - 2.0], [x, y - 2.0 + layout.line_height], guide_color)
                        .build();
                }
                level += 1;
            }
            let mut x = origin_x;
            let spans = highlight
                .filter(|h| h.enabled && !historical)
                .map_or(&[][..], |h| h.spans_for_line(row));
            let mut span_index = 0;
            let default_color = highlight.map_or_else(
                || ui.style_color(dear_imgui_rs::StyleColor::Text),
                EditorHighlight::default_text_color,
            );
            let mut run: Option<TextRun> = None;
            let mut start = segment.start;
            while start < segment.end {
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
                    while end < segment.end && line[end] & 0xc0 == 0x80 {
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
                if visible && !historical && view.is_position_selected(row, start as i32) {
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
                while start < segment.end && line[start] & 0xc0 == 0x80 {
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
    use bed_document_session::editor::Editor;
    use bed_editing::editor_commands::CursorReveal;
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
                sys::igColorConvertFloat4ToU32(ui.style_color(StyleColor::TextSelectedBg).into())
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

    #[test]
    fn wrapped_text_keeps_document_selection_and_caret_on_visual_rows() {
        use crate::views::caret_view::CaretView;

        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = context();
        let mut editor = Editor::new();
        editor.set_content(b"AAAAAA");
        editor
            .commands()
            .set_selection(0, 1, 0, 5, CursorReveal::Ensure);
        render(&mut context, |ui, layout| {
            let advance = glyph_advance(ui, "A");
            let mut projection = RowProjection::build(&editor.state, None, None);
            projection.wrap(ui, &editor.state, advance * 2.0 + 0.01);
            let before = vertices(ui).len();
            TextView::draw_projected(
                ui,
                &editor.state,
                &editor.view,
                layout,
                None,
                None,
                Some(&projection),
            );
            let actual = vertices(ui)[before..].to_vec();
            let selection_color = native_color(ui, ui.style_color(StyleColor::TextSelectedBg));
            let text_color = native_color(ui, ui.style_color(StyleColor::Text));
            let text: Vec<_> = actual
                .iter()
                .filter(|v| v.col == text_color)
                .copied()
                .collect();
            assert_eq!(text.len(), 24, "each glyph is drawn exactly once");
            assert_eq!(
                actual.iter().filter(|v| v.col == selection_color).count(),
                16
            );
            let reference_start = vertices(ui).len();
            for visual in 0..3 {
                ui.get_window_draw_list().add_text(
                    [
                        layout.text_pos[0],
                        layout.text_pos[1] + visual as f32 * layout.line_height,
                    ],
                    ui.style_color(StyleColor::Text),
                    "AA",
                );
            }
            assert_eq!(text, vertices(ui)[reference_start..]);

            let caret_start = vertices(ui).len();
            CaretView::draw_projected(ui, &editor.state, &editor.view, layout, Some(&projection));
            let caret = &vertices(ui)[caret_start..];
            assert_eq!(caret.len(), 4);
            let x = (layout.text_pos[0] + advance).floor();
            let y = (layout.text_pos[1] + 2.0 * layout.line_height).floor();
            assert_eq!(caret[0].pos.x, x - 1.0);
            assert_eq!(caret[0].pos.y, y);
            assert_eq!(editor.state.join(), b"AAAAAA");
        });

        editor
            .commands()
            .set_selection(0, 2, 0, 2, CursorReveal::Ensure);
        render(&mut context, |ui, layout| {
            let mut projection = RowProjection::build(&editor.state, None, None);
            projection.wrap(ui, &editor.state, glyph_advance(ui, "A") * 2.0 + 0.01);
            let before = vertices(ui).len();
            CaretView::draw_projected(ui, &editor.state, &editor.view, layout, Some(&projection));
            let caret = &vertices(ui)[before..];
            assert_eq!(caret[0].pos.x, layout.text_pos[0].floor() - 1.0);
            assert_eq!(
                caret[0].pos.y,
                (layout.text_pos[1] + layout.line_height).floor()
            );
        });
    }

    #[test]
    fn wrapped_continuations_preserve_spaces_tabs_and_selection_geometry() {
        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = context();
        for source in [b"        AAAA BBBB CCCC".as_slice(), b"\t\tAAAA BBBB CCCC"] {
            let mut editor = Editor::new();
            editor.set_content(source);
            editor
                .commands()
                .set_selection(0, 0, 0, source.len() as i32, CursorReveal::Ensure);
            render(&mut context, |ui, layout| {
                let space = glyph_advance(ui, " ");
                let mut projection = RowProjection::build(&editor.state, None, None);
                projection.wrap(ui, &editor.state, space * 20.0 + 0.01);
                assert_eq!(projection.len(), 2);
                assert_eq!(&source[projection.segment(1).unwrap()], b"CCCC");
                let y = layout.text_pos[1] + layout.line_height;
                let origin_x = layout.text_pos[0] + space * 8.0;
                let before = vertices(ui).len();
                TextView::draw_projected(
                    ui,
                    &editor.state,
                    &editor.view,
                    layout,
                    None,
                    None,
                    Some(&projection),
                );
                let actual = vertices(ui)[before..].to_vec();
                let text_color = native_color(ui, ui.style_color(StyleColor::Text));
                let text: Vec<_> = actual
                    .iter()
                    .filter(|v| v.col == text_color && v.pos.y >= y)
                    .copied()
                    .collect();
                let reference_start = vertices(ui).len();
                ui.get_window_draw_list().add_text(
                    [origin_x, y],
                    ui.style_color(StyleColor::Text),
                    "CCCC",
                );
                assert_eq!(text, vertices(ui)[reference_start..]);

                let selection_color = native_color(ui, ui.style_color(StyleColor::TextSelectedBg));
                let selection: Vec<_> = actual
                    .iter()
                    .filter(|v| v.col == selection_color)
                    .copied()
                    .collect();
                let continuation: Vec<_> = selection
                    .chunks_exact(4)
                    .filter(|rectangle| rectangle[0].pos.y == y)
                    .flatten()
                    .collect();
                assert_eq!(
                    continuation.len(),
                    16,
                    "only the four source glyphs are selected"
                );
                assert_eq!(continuation[0].pos.x, origin_x);
                assert!(continuation.iter().all(|v| v.pos.x >= origin_x));

                let guide_color = native_color(ui, {
                    let mut color = ui.style_color(StyleColor::Text);
                    color[3] = 0.18;
                    color
                });
                let guides: Vec<_> = actual
                    .iter()
                    .filter(|v| v.col == guide_color)
                    .copied()
                    .collect();
                let reference_start = vertices(ui).len();
                for visual in 0..2 {
                    let guide_x = layout.text_pos[0] + space * 4.0;
                    let guide_y = layout.text_pos[1] + visual as f32 * layout.line_height - 2.0;
                    ui.get_window_draw_list()
                        .add_line(
                            [guide_x, guide_y],
                            [guide_x, guide_y + layout.line_height],
                            guide_color,
                        )
                        .build();
                }
                let reference_guides: Vec<_> = vertices(ui)[reference_start..]
                    .iter()
                    .filter(|v| v.col == guide_color)
                    .copied()
                    .collect();
                assert_eq!(guides, reference_guides);
                assert_eq!(editor.state.join(), source);
            });
        }
    }

    #[test]
    fn tabs_within_a_continuation_restart_after_virtual_indentation() {
        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = context();
        let mut editor = Editor::new();
        editor.set_content(b"  AAAA BBBB C\tD");
        render(&mut context, |ui, layout| {
            let space = glyph_advance(ui, " ");
            let mut projection = RowProjection::build(&editor.state, None, None);
            projection.wrap(ui, &editor.state, space * 12.0 + 0.01);
            assert_eq!(projection.len(), 2);
            assert_eq!(
                &editor.state.line(0)[projection.segment(1).unwrap()],
                b"C\tD"
            );
            let before = vertices(ui).len();
            TextView::draw_projected(
                ui,
                &editor.state,
                &editor.view,
                layout,
                None,
                None,
                Some(&projection),
            );
            let y = layout.text_pos[1] + layout.line_height;
            let color = ui.style_color(StyleColor::Text);
            let text_color = native_color(ui, color);
            let text: Vec<_> = vertices(ui)[before..]
                .iter()
                .filter(|v| v.col == text_color && v.pos.y >= y)
                .copied()
                .collect();
            let reference_start = vertices(ui).len();
            ui.get_window_draw_list()
                .add_text([layout.text_pos[0] + 2.0 * space, y], color, "C");
            ui.get_window_draw_list()
                .add_text([layout.text_pos[0] + 6.0 * space, y], color, "D");
            assert_eq!(text, vertices(ui)[reference_start..]);
        });
    }

    #[test]
    fn wrapped_diagnostics_start_at_the_preserved_indent() {
        use bed_editing::diagnostic::DiagnosticItem;

        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = context();
        let arbiter = super::super::hover_tooltip::TooltipArbiter::default();
        for source in [b"        AAAA BBBB CCCC".as_slice(), b"\t\tAAAA BBBB CCCC"] {
            let mut editor = Editor::new();
            editor.set_content(source);
            editor.state.path = "/virtual/wrapped-indent.rs".into();
            let mut reference = Editor::new();
            reference.set_content(b"CCCC");
            reference.state.path = editor.state.path.clone();
            render(&mut context, |ui, layout| {
                let space = glyph_advance(ui, " ");
                let mut projection = RowProjection::build(&editor.state, None, None);
                projection.wrap(ui, &editor.state, space * 20.0 + 0.01);
                let start = source.len() as i32 - 4;
                for length in [0, 4] {
                    let before = vertices(ui).len();
                    TextView::draw_projected_diagnostics(
                        ui,
                        &editor.state,
                        layout,
                        &[DiagnosticItem {
                            start_character: start,
                            end_character: start + length,
                            severity: 1,
                            ..Default::default()
                        }],
                        Default::default(),
                        &arbiter,
                        Some(&projection),
                    );
                    let actual = vertices(ui)[before..].to_vec();
                    assert!(!actual.is_empty());
                    let reference_start = vertices(ui).len();
                    TextView::draw_diagnostics(
                        ui,
                        &reference.state,
                        &ViewLayout {
                            text_pos: [
                                layout.text_pos[0] + space * 8.0,
                                layout.text_pos[1] + layout.line_height,
                            ],
                            ..*layout
                        },
                        &[DiagnosticItem {
                            end_character: length,
                            severity: 1,
                            ..Default::default()
                        }],
                        Default::default(),
                        &arbiter,
                    );
                    assert_eq!(actual, vertices(ui)[reference_start..]);
                }
            });
        }
    }

    #[test]
    fn wrapped_diagnostics_follow_utf16_ranges_and_boundary_carets() {
        use bed_editing::diagnostic::DiagnosticItem;

        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = context();
        let mut editor = Editor::new();
        editor.set_content("🙂A🙂A🙂A".as_bytes());
        editor.state.path = "/virtual/wrapped.rs".into();
        let arbiter = super::super::hover_tooltip::TooltipArbiter::default();
        render(&mut context, |ui, layout| {
            let width = glyph_advance(ui, "🙂") + glyph_advance(ui, "A") + 0.01;
            let mut projection = RowProjection::build(&editor.state, None, None);
            projection.wrap(ui, &editor.state, width);
            for (start_character, end_character, expected_rows) in
                [(0, 9, vec![0, 1, 2]), (3, 3, vec![1])]
            {
                let before = vertices(ui).len();
                TextView::draw_projected_diagnostics(
                    ui,
                    &editor.state,
                    layout,
                    &[DiagnosticItem {
                        start_character,
                        end_character,
                        severity: 1,
                        ..Default::default()
                    }],
                    Default::default(),
                    &arbiter,
                    Some(&projection),
                );
                let actual = &vertices(ui)[before..];
                assert!(!actual.is_empty());
                let mut rows: Vec<_> = actual
                    .iter()
                    .map(|v| ((v.pos.y - layout.text_pos[1]) / layout.line_height).floor() as i32)
                    .collect();
                rows.sort_unstable();
                rows.dedup();
                assert_eq!(rows, expected_rows);
                assert!(
                    actual
                        .iter()
                        .all(|v| v.pos.x < layout.text_pos[0] + width + 10.0)
                );
            }
        });
    }

    #[test]
    fn execution_highlight_tracks_its_source_row_independently_of_caret() {
        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = context();
        let mut editor = Editor::new();
        editor.set_content(b"first\nsecond\nthird\nfourth");
        editor.view.row = 0;
        render(&mut context, |ui, layout| {
            let before = vertices(ui).len();
            TextView::draw_with_debug_highlight(
                ui,
                &editor.state,
                &editor.view,
                layout,
                None,
                Some(2),
            );
            let actual = &vertices(ui)[before..];
            let execution_color = native_color(ui, [0.95, 0.68, 0.12, 0.19]);
            let execution: Vec<_> = actual
                .iter()
                .filter(|vertex| vertex.col == execution_color)
                .collect();
            assert_eq!(execution.len(), 4);
            let top = layout.text_pos[1] + 2.0 * layout.line_height;
            assert!(
                execution
                    .iter()
                    .all(|vertex| vertex.pos.y >= top && vertex.pos.y <= top + layout.line_height)
            );
            assert_eq!(editor.view.row, 0);
            assert_eq!(editor.state.join(), b"first\nsecond\nthird\nfourth");
        });
        render(&mut context, |ui, layout| {
            let before = vertices(ui).len();
            TextView::draw_with_debug_highlight(
                ui,
                &editor.state,
                &editor.view,
                layout,
                None,
                Some(100),
            );
            let execution_color = native_color(ui, [0.95, 0.68, 0.12, 0.19]);
            assert!(
                vertices(ui)[before..]
                    .iter()
                    .all(|vertex| vertex.col != execution_color)
            );
        });
    }

    #[test]
    fn native_diagnostic_squiggles_keep_all_severities_readable_on_both_surfaces() {
        use bed_editing::diagnostic::DiagnosticItem;
        use bed_editing::util::color::{blend, contrast_ratio};

        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = context();
        let mut editor = Editor::new();
        editor.state.path = "/virtual/theme-diagnostics.rs".into();
        editor.set_content(
            &("wide diagnostic range ".repeat(10) + "\n")
                .repeat(4)
                .into_bytes(),
        );
        let diagnostics: Vec<_> = (0..4)
            .map(|row| DiagnosticItem {
                start_line: row,
                end_line: row,
                end_character: 100,
                severity: row + 1,
                ..Default::default()
            })
            .collect();
        let arbiter = super::super::hover_tooltip::TooltipArbiter::default();
        for background in [[0.96, 0.94, 0.90, 1.0], [0.04, 0.05, 0.08, 1.0]] {
            context
                .style_mut()
                .set_color(StyleColor::WindowBg, background);
            render(&mut context, |ui, layout| {
                let start = vertices(ui).len();
                TextView::draw_diagnostics(
                    ui,
                    &editor.state,
                    layout,
                    &diagnostics,
                    Default::default(),
                    &arbiter,
                );
                let actual = &vertices(ui)[start..];
                assert!(actual.len() > 100);
                let mut inks = std::collections::HashSet::new();
                for vertex in actual {
                    let color: [f32; 4] = std::array::from_fn(|channel| {
                        ((vertex.col >> (channel * 8)) & 255) as f32 / 255.0
                    });
                    if color[3] > 0.5 {
                        assert!(
                            contrast_ratio(blend(color, background, color[3]), background) >= 4.45
                        );
                        inks.insert(vertex.col);
                    }
                }
                assert_eq!(inks.len(), 4);
            });
        }
    }
}
