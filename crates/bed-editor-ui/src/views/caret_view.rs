//! Translated from ned editor/views/caret_view.{h,cpp}; see LICENSE and NOTICE.
use crate::views::view_layout::{ViewLayout, line_column_x, rainbow_color};
use bed_editing::{editor_state::EditorState, editor_view_state::EditorViewState};
use dear_imgui_rs::{StyleColor, Ui, sys};

pub struct CaretView;

impl CaretView {
    pub fn draw(ui: &Ui, state: &EditorState, view: &EditorViewState, layout: &ViewLayout) {
        Self::draw_projected(ui, state, view, layout, None);
    }
    pub(crate) fn draw_projected(
        ui: &Ui,
        state: &EditorState,
        view: &EditorViewState,
        layout: &ViewLayout,
        projection: Option<&crate::diff::RowProjection>,
    ) {
        if view.block_input {
            return;
        }
        let draw = ui.get_window_draw_list();
        let blink_alpha = ((view.cursor_blink_time * 4.0).sin() + 1.0) * 0.5;
        let ink = bed_ui::presentation::readable_color(
            ui,
            if layout.rainbow_mode {
                rainbow_color(ui.time() as f32)
            } else {
                ui.style_color(StyleColor::Text)
            },
        );
        let color = ui.with_bound_context(|| unsafe { sys::igColorConvertFloat4ToU32(ink.into()) });
        let (primary_color, secondary_color) = if layout.rainbow_mode {
            (color, color)
        } else {
            // Source IM_COL32 truncates blink alpha explicitly, while rainbow
            // uses ColorConvertFloat4ToU32's rounded channels.
            (
                (color & 0x00ff_ffff) | (((blink_alpha * 255.0) as u32) << 24),
                (color & 0x00ff_ffff) | 0xa000_0000,
            )
        };
        for (index, sel) in view.selections.iter().enumerate() {
            if projection.is_some_and(|p| !p.is_document_visible(sel.head_row)) {
                continue;
            }
            let x = projection
                .map_or_else(
                    || {
                        line_column_x(
                            ui,
                            &state.line(sel.head_row),
                            sel.head_column,
                            layout.text_pos[0],
                        )
                    },
                    |p| p.position_x(ui, state, sel.head_row, sel.head_column, layout.text_pos[0]),
                )
                .floor();
            let visual = projection.map_or(sel.head_row as usize, |p| {
                p.visual_position(sel.head_row, sel.head_column)
            });
            let top = projection.map_or(visual as f32, |p| p.row_top(visual));
            let height = projection.map_or(1.0, |p| p.row_height(visual)) * layout.line_height;
            if height <= 0.0 {
                continue;
            }
            let y = (layout.text_pos[1] + top * layout.line_height).floor();
            let color = if index == view.primary_index {
                primary_color
            } else {
                secondary_color
            };
            draw.add_rect(
                [x - 1.0, y],
                [x + 1.0, y + height.min(layout.line_height - 1.0)],
                color,
            )
            .filled(true)
            .build();
        }
    }
}
