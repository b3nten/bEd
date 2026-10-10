//! Line-number portion of ned editor/views/gutter_view.cpp.
use crate::source_debug::{BreakpointStatus, SourceDebugPresentation};
use crate::views::view_layout::{ViewLayout, rainbow_color};
use bed_document_session::editor::Editor;
use bed_editing::editor_state::EditorState;
use dear_imgui_rs::{DrawListMut, StyleColor, Ui, sys};

const LEADING_PADDING: f32 = 2.0;
const TRAILING_PADDING: f32 = 4.0;

pub struct GutterView;

impl GutterView {
    pub(crate) fn fold_column_width(ui: &Ui, enabled: bool) -> f32 {
        if enabled {
            ui.current_font_size().max(14.0)
        } else {
            0.0
        }
    }

    pub(crate) fn draw_folds(
        ui: &Ui,
        draw: &DrawListMut<'_>,
        editor: &Editor,
        layout: &ViewLayout,
        pos: [f32; 2],
        width: f32,
        column_width: f32,
        projection: &crate::diff::RowProjection,
    ) {
        let color = bed_ui::presentation::muted_text_color(ui);
        let first = projection.visual_at_y(editor.view.scroll_position[1] / layout.line_height);
        let last = projection
            .visual_at_y((editor.view.scroll_position[1] + layout.size[1]) / layout.line_height);
        for range in editor.view.folds.ranges() {
            if !projection.is_document_visible(range.start_line) {
                continue;
            }
            let visual = projection.visual_row(range.start_line);
            if visual < first || visual > last {
                continue;
            }
            let top =
                pos[1] + layout.editor_top_margin + projection.row_top(visual) * layout.line_height
                    - editor.view.scroll_position[1];
            let height = projection.row_height(visual) * layout.line_height;
            if height <= 0.0 {
                continue;
            }
            let _clip = (height < layout.line_height)
                .then(|| draw.push_clip_rect([pos[0], top], [pos[0] + width, top + height], true));
            let center = [
                pos[0] + width - column_width * 0.5,
                top + layout.line_height * 0.5,
            ];
            let half = column_width * 0.22;
            let angle = projection.fold_openness(range.start_line) * std::f32::consts::FRAC_PI_2;
            let (sin, cos) = angle.sin_cos();
            let points = [[-half * 0.5, -half], [half, 0.0], [-half * 0.5, half]]
                .map(|[x, y]| [center[0] + x * cos - y * sin, center[1] + x * sin + y * cos]);
            draw.add_triangle(points[0], points[1], points[2], color)
                .filled(true)
                .build();
        }
    }

    pub fn width(ui: &Ui, state: &EditorState) -> f32 {
        // Reserve only this document's digits and a little space on each side.
        ui.calc_text_size(state.line_count().max(1).to_string())[0]
            + LEADING_PADDING
            + TRAILING_PADDING
    }

    pub fn diagnostic_column_width(ui: &Ui, editor: &Editor) -> f32 {
        if editor.diagnostics.is_some() {
            6.0_f32.max(ui.current_font_size() * 0.55)
        } else {
            0.0
        }
    }

    pub fn debug_column_width(ui: &Ui, enabled: bool) -> f32 {
        if enabled {
            (ui.current_font_size() * 0.9).max(16.0)
        } else {
            0.0
        }
    }

    pub fn draw_breakpoint_preview(
        ui: &Ui,
        draw: &DrawListMut<'_>,
        editor: &Editor,
        layout: &ViewLayout,
        pos: [f32; 2],
        row: i32,
    ) {
        Self::draw_projected_breakpoint_preview(ui, draw, editor, layout, pos, row, None);
    }
    pub(crate) fn draw_projected_breakpoint_preview(
        ui: &Ui,
        draw: &DrawListMut<'_>,
        editor: &Editor,
        layout: &ViewLayout,
        pos: [f32; 2],
        row: i32,
        projection: Option<&crate::diff::RowProjection>,
    ) {
        let visual = projection.map_or(row as usize, |p| p.visual_row(row));
        let height = projection.map_or(1.0, |p| p.row_height(visual)) * layout.line_height;
        if height <= 0.0 {
            return;
        }
        let top = pos[1]
            + layout.editor_top_margin
            + projection.map_or(visual as f32, |p| p.row_top(visual)) * layout.line_height
            - editor.view.scroll_position[1];
        let width = Self::debug_column_width(ui, true);
        let _clip = (height < layout.line_height)
            .then(|| draw.push_clip_rect([pos[0], top], [pos[0] + width, top + height], true));
        let center = [pos[0] + width * 0.32, top + layout.line_height * 0.5];
        let radius = (layout.line_height * 0.24).min(width * 0.25);
        let mut color = bed_ui::presentation::readable_color(ui, [0.93, 0.25, 0.28, 1.0]);
        color[3] = 0.45;
        draw.add_circle(center, radius, color).filled(true).build();
    }

    pub fn draw(
        ui: &Ui,
        draw: &DrawListMut<'_>,
        editor: &Editor,
        layout: &ViewLayout,
        pos: [f32; 2],
        width: f32,
    ) {
        Self::draw_with_debug(ui, draw, editor, layout, pos, width, None);
    }

    pub fn draw_with_debug(
        ui: &Ui,
        draw: &DrawListMut<'_>,
        editor: &Editor,
        layout: &ViewLayout,
        pos: [f32; 2],
        width: f32,
        debug: Option<&SourceDebugPresentation>,
    ) {
        Self::draw_projected(ui, draw, editor, layout, pos, width, debug, None, false);
    }
    pub(crate) fn draw_projected(
        ui: &Ui,
        draw: &DrawListMut<'_>,
        editor: &Editor,
        layout: &ViewLayout,
        pos: [f32; 2],
        width: f32,
        debug: Option<&SourceDebugPresentation>,
        projection: Option<&crate::diff::RowProjection>,
        diff: bool,
    ) {
        let state = &editor.state;
        let view = &editor.view;
        let git = editor.git.borrow();
        let column_width = Self::diagnostic_column_width(ui, editor);
        let debug_width = Self::debug_column_width(ui, debug.is_some());
        let severity_by_line = editor
            .diagnostics
            .as_ref()
            .filter(|_| !state.path.is_empty())
            .map(|diagnostics| diagnostics.max_severity_by_line(&state.path, state.line_count()))
            .unwrap_or_default();
        let first_y = view.scroll_position[1] / layout.line_height;
        let last_y = (view.scroll_position[1] + layout.size[1] - layout.editor_top_margin)
            / layout.line_height;
        let first = projection.map_or(first_y as i32, |p| p.visual_at_y(first_y) as i32);
        let last = projection
            .map_or(state.line_count(), |p| p.len() as i32)
            .min(projection.map_or(last_y as i32, |p| p.visual_at_y(last_y) as i32) + 1);
        let (selection_start, selection_end) = view.selection_line_span();
        let pack = |color: [f32; 4]| {
            ui.with_bound_context(|| unsafe { sys::igColorConvertFloat4ToU32(color.into()) })
        };
        let text = bed_ui::presentation::readable_color(ui, ui.style_color(StyleColor::Text));
        let current_color = if layout.rainbow_mode {
            pack(bed_ui::presentation::readable_color(
                ui,
                rainbow_color(ui.time() as f32),
            ))
        } else {
            pack(text)
        };
        let edited_color = pack(text);
        let muted_color = pack(bed_ui::presentation::muted_text_color(ui));
        let breakpoint_color = pack(bed_ui::presentation::readable_color(
            ui,
            [0.93, 0.25, 0.28, 1.0],
        ));
        let execution_color = pack(bed_ui::presentation::readable_color(
            ui,
            [0.95, 0.68, 0.12, 1.0],
        ));
        let severity_colors: [u32; 4] = std::array::from_fn(|index| {
            pack(bed_ui::presentation::readable_color(
                ui,
                super::diagnostic_style::severity_color(index as i32 + 1),
            ))
        });
        for visual in first..last {
            let height =
                projection.map_or(1.0, |p| p.row_height(visual as usize)) * layout.line_height;
            if height <= 0.0 {
                continue;
            }
            if projection
                .and_then(|p| p.segment(visual as usize))
                .is_some_and(|segment| segment.start != 0)
            {
                continue;
            }
            let y = pos[1]
                + layout.editor_top_margin
                + projection.map_or(visual as f32, |p| p.row_top(visual as usize))
                    * layout.line_height
                - view.scroll_position[1];
            let _clip = (height < layout.line_height)
                .then(|| draw.push_clip_rect([pos[0], y], [pos[0] + width, y + height], true));
            let (row, old_row) = match projection.and_then(|p| p.rows.get(visual as usize)) {
                Some(crate::diff::ProjectedRow::Document { row, old_row, .. }) => (*row, *old_row),
                Some(crate::diff::ProjectedRow::Historical { old_row, .. }) => {
                    let label = format!("{}  −", old_row + 1);
                    draw.add_text([pos[0] + width * 0.12, y], muted_color, label);
                    continue;
                }
                Some(_) => continue,
                None => (visual, None),
            };
            let selected = row >= selection_start && row < selection_end;
            let color = if row == view.row || selected {
                current_color
            } else if git.is_line_edited(&state.path, row + 1) {
                edited_color
            } else {
                muted_color
            };
            let label = if diff {
                format!(
                    "{}  {}",
                    old_row
                        .map(|row| (row + 1).to_string())
                        .unwrap_or_else(|| "+".into()),
                    row + 1
                )
            } else {
                (row + 1).to_string()
            };
            let x = pos[0] + width - ui.calc_text_size(&label)[0] - TRAILING_PADDING;
            if let Some(debug) = debug {
                let center = [pos[0] + debug_width * 0.32, y + layout.line_height * 0.5];
                let radius = (layout.line_height * 0.24).min(debug_width * 0.25);
                if let Some(breakpoint) = debug
                    .breakpoints
                    .iter()
                    .find(|breakpoint| breakpoint.row == row)
                {
                    let color = if breakpoint.enabled {
                        breakpoint_color
                    } else {
                        muted_color
                    };
                    draw.add_circle(center, radius, color)
                        .filled(
                            breakpoint.enabled && breakpoint.status == BreakpointStatus::Verified,
                        )
                        .thickness(1.5)
                        .build();
                    if breakpoint.enabled && breakpoint.status == BreakpointStatus::Rejected {
                        let extent = radius * 0.55;
                        for direction in [-1.0, 1.0] {
                            draw.add_line(
                                [center[0] - extent, center[1] - extent * direction],
                                [center[0] + extent, center[1] + extent * direction],
                                color,
                            )
                            .thickness(1.2)
                            .build();
                        }
                    }
                }
                if debug.execution_row == Some(row) {
                    let left = pos[0] + debug_width * 0.67;
                    let right = pos[0] + debug_width * 0.97;
                    let half = layout.line_height * 0.22;
                    draw.add_triangle(
                        [left, center[1] - half],
                        [right, center[1]],
                        [left, center[1] + half],
                        execution_color,
                    )
                    .filled(true)
                    .build();
                }
            }
            if let Some(&severity) = severity_by_line.get(row as usize)
                && severity > 0
                && column_width > 0.0
            {
                let mark_width = 3.0_f32.max(column_width * 0.45);
                let x = pos[0] + debug_width + (column_width - mark_width) * 0.5;
                draw.add_rect(
                    [x, y + 2.0],
                    [x + mark_width, y + layout.line_height - 2.0],
                    severity_colors[(severity.clamp(1, 4) - 1) as usize],
                )
                .filled(true)
                .build();
            }
            draw.add_text([x, y], color, label);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::RowProjection;
    use crate::source_debug::SourceBreakpoint;
    use dear_imgui_rs::{Condition, Context, FramePrepareOptions};

    #[test]
    fn wrapped_gutter_draws_line_number_and_source_markers_once() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
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
        editor.set_content(b"AAAAAAAA");
        let debug = SourceDebugPresentation {
            breakpoints: vec![SourceBreakpoint {
                row: 0,
                enabled: true,
                status: BreakpointStatus::Verified,
            }],
            execution_row: Some(0),
        };
        context.prepare_frame(FramePrepareOptions::new([500.0, 300.0], 1.0 / 60.0));
        let ui = context.frame();
        ui.window("wrapped gutter")
            .position([0.0; 2], Condition::Always)
            .size([480.0, 280.0], Condition::Always)
            .build(|| {
                let layout = ViewLayout {
                    size: [400.0, 200.0],
                    line_height: ui.text_line_height(),
                    ..Default::default()
                };
                let mut projection = RowProjection::build(&editor.state, None, None);
                projection.wrap(
                    ui,
                    &editor.state,
                    crate::views::view_layout::glyph_advance(ui, "A") * 2.0 + 0.01,
                );
                assert_eq!(projection.len(), 4);
                let pos = [30.0, 30.0];
                let width =
                    GutterView::width(ui, &editor.state) + GutterView::debug_column_width(ui, true);
                let vertices = || {
                    ui.with_bound_context(|| unsafe {
                        let v = &(*sys::igGetWindowDrawList()).VtxBuffer;
                        std::slice::from_raw_parts(v.Data, v.Size as usize).to_vec()
                    })
                };
                let before = vertices().len();
                let draw = ui.get_window_draw_list();
                GutterView::draw_projected(
                    ui,
                    &draw,
                    &editor,
                    &layout,
                    pos,
                    width,
                    Some(&debug),
                    Some(&projection),
                    false,
                );
                let wrapped = vertices()[before..].to_vec();
                let reference_start = vertices().len();
                GutterView::draw_with_debug(ui, &draw, &editor, &layout, pos, width, Some(&debug));
                assert_eq!(wrapped, vertices()[reference_start..]);
            });
        drop(context.render_legacy());
    }

    #[test]
    fn breakpoint_states_and_execution_arrow_coexist_in_their_own_column() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
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
        editor.set_content(b"first\nsecond\nthird");
        let mut states = Vec::new();
        for (enabled, status) in [
            (true, BreakpointStatus::Pending),
            (true, BreakpointStatus::Verified),
            (true, BreakpointStatus::Rejected),
            (false, BreakpointStatus::Verified),
        ] {
            context.prepare_frame(FramePrepareOptions::new([500.0, 300.0], 1.0 / 60.0));
            let ui = context.frame();
            ui.window("source marker fixture")
                .position([0.0; 2], Condition::Always)
                .size([480.0, 280.0], Condition::Always)
                .build(|| {
                    let layout = ViewLayout {
                        size: [400.0, 200.0],
                        line_height: ui.text_line_height(),
                        ..Default::default()
                    };
                    let debug = SourceDebugPresentation {
                        breakpoints: vec![SourceBreakpoint {
                            row: 1,
                            enabled,
                            status,
                        }],
                        execution_row: Some(1),
                    };
                    let pos = [30.0, 30.0];
                    let debug_width = GutterView::debug_column_width(ui, true);
                    let width = GutterView::width(ui, &editor.state) + debug_width;
                    let start = ui.with_bound_context(|| unsafe {
                        (*sys::igGetWindowDrawList()).VtxBuffer.Size
                    });
                    GutterView::draw_with_debug(
                        ui,
                        &ui.get_window_draw_list(),
                        &editor,
                        &layout,
                        pos,
                        width,
                        Some(&debug),
                    );
                    ui.with_bound_context(|| unsafe {
                        let vertices = &(*sys::igGetWindowDrawList()).VtxBuffer;
                        let marker: Vec<_> = (start..vertices.Size)
                            .map(|index| *vertices.Data.add(index as usize))
                            .filter(|vertex| vertex.pos.x < pos[0] + debug_width)
                            .map(|vertex| (vertex.pos.x, vertex.pos.y, vertex.col))
                            .collect();
                        assert!(!marker.is_empty());
                        let middle = pos[1] + layout.editor_top_margin + layout.line_height * 1.5;
                        assert!(
                            marker
                                .iter()
                                .all(|(_, y, _)| (y - middle).abs() < layout.line_height * 0.4)
                        );
                        let arrow_color = sys::igColorConvertFloat4ToU32(
                            bed_ui::presentation::readable_color(ui, [0.95, 0.68, 0.12, 1.0])
                                .into(),
                        );
                        assert!(marker.iter().any(|(_, _, color)| *color == arrow_color));
                        states.push(marker);
                    });
                });
            drop(context.render_legacy());
        }
        for first in 0..states.len() {
            for second in first + 1..states.len() {
                assert_ne!(
                    states[first], states[second],
                    "breakpoint states must remain distinguishable"
                );
            }
        }
    }
}
