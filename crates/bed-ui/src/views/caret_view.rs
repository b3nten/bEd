//! Translated from ned editor/views/caret_view.{h,cpp}; see LICENSE and NOTICE.
use crate::views::view_layout::{ViewLayout, line_column_x, rainbow_color};
use bed_core::{editor_state::EditorState, editor_view_state::EditorViewState};
use dear_imgui_rs::{StyleColor, Ui, sys};

pub struct CaretView;

impl CaretView {
    pub fn draw(ui: &Ui, state: &EditorState, view: &EditorViewState, layout: &ViewLayout) {
        if view.block_input {
            return;
        }
        let draw = ui.get_window_draw_list();
        let blink_alpha = ((view.cursor_blink_time * 4.0).sin() + 1.0) * 0.5;
        let ink = crate::presentation::readable_color(
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
            let x = line_column_x(
                ui,
                &state.line(sel.head_row),
                sel.head_column,
                layout.text_pos[0],
            )
            .floor();
            let y = (layout.text_pos[1] + sel.head_row as f32 * layout.line_height).floor();
            let color = if index == view.primary_index {
                primary_color
            } else {
                secondary_color
            };
            draw.add_rect([x - 1.0, y], [x + 1.0, y + layout.line_height - 1.0], color)
                .filled(true)
                .build();
        }
    }
}
