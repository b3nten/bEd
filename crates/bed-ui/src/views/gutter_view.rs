//! Line-number portion of ned editor/views/gutter_view.cpp.
use crate::source_debug::{BreakpointStatus, SourceDebugPresentation};
use crate::views::view_layout::{ViewLayout, rainbow_color};
use bed_core::editor_state::EditorState;
use bed_session::editor::Editor;
use dear_imgui_rs::{DrawListMut, StyleColor, Ui, sys};

pub struct GutterView;

impl GutterView {
    pub fn width(ui: &Ui, state: &EditorState) -> f32 {
        // Keep upstream's 2px leading and 10px trailing spacing, reserving
        // digits for this document rather than a minimum of three digits.
        ui.calc_text_size(state.line_count().max(1).to_string())[0] + 12.0
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
            (ui.current_font_size() * 1.3).max(16.0)
        } else {
            0.0
        }
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
        let first = (view.scroll_position[1] / layout.line_height) as i32;
        let last = state.line_count().min(
            ((view.scroll_position[1] + layout.size[1] - layout.editor_top_margin)
                / layout.line_height) as i32
                + 1,
        );
        let (selection_start, selection_end) = view.selection_line_span();
        let pack = |color: [f32; 4]| {
            ui.with_bound_context(|| unsafe { sys::igColorConvertFloat4ToU32(color.into()) })
        };
        let text = crate::presentation::readable_color(ui, ui.style_color(StyleColor::Text));
        let current_color = if layout.rainbow_mode {
            pack(crate::presentation::readable_color(
                ui,
                rainbow_color(ui.time() as f32),
            ))
        } else {
            pack(text)
        };
        let edited_color = pack(text);
        let muted_color = pack(crate::presentation::muted_text_color(ui));
        let breakpoint_color = pack(crate::presentation::readable_color(
            ui,
            [0.93, 0.25, 0.28, 1.0],
        ));
        let execution_color = pack(crate::presentation::readable_color(
            ui,
            [0.95, 0.68, 0.12, 1.0],
        ));
        let severity_colors: [u32; 4] = std::array::from_fn(|index| {
            pack(crate::presentation::readable_color(
                ui,
                super::diagnostic_style::severity_color(index as i32 + 1),
            ))
        });
        for row in first..last {
            let selected = row >= selection_start && row < selection_end;
            let color = if row == view.row || selected {
                current_color
            } else if git.is_line_edited(&state.path, row + 1) {
                edited_color
            } else {
                muted_color
            };
            let label = (row + 1).to_string();
            let x = pos[0] + width - ui.calc_text_size(&label)[0] - 10.0;
            let y = pos[1] + layout.editor_top_margin + row as f32 * layout.line_height
                - view.scroll_position[1];
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
    use crate::source_debug::SourceBreakpoint;
    use dear_imgui_rs::{Condition, Context, FramePrepareOptions};

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
                            crate::presentation::readable_color(ui, [0.95, 0.68, 0.12, 1.0]).into(),
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
