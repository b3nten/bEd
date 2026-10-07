//! Line-number portion of ned editor/views/gutter_view.cpp.
use crate::views::view_layout::{ViewLayout, rainbow_color};
use bed_core::editor_state::EditorState;
use bed_session::editor::Editor;
use dear_imgui_rs::{DrawListMut, Ui, sys};

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

    pub fn draw(
        ui: &Ui,
        draw: &DrawListMut<'_>,
        editor: &Editor,
        layout: &ViewLayout,
        pos: [f32; 2],
        width: f32,
    ) {
        let state = &editor.state;
        let view = &editor.view;
        let git = editor.git.borrow();
        let column_width = Self::diagnostic_column_width(ui, editor);
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
        let current_color = if layout.rainbow_mode {
            let rainbow = rainbow_color(ui.time() as f32);
            ui.with_bound_context(|| unsafe { sys::igColorConvertFloat4ToU32(rainbow.into()) })
        } else {
            0xffff_ffff
        };
        for row in first..last {
            let selected = row >= selection_start && row < selection_end;
            let color = if row == view.row || selected {
                current_color
            } else if git.is_line_edited(&state.path, row + 1) {
                0xffff_ffff
            } else {
                0x9680_8080
            };
            let label = (row + 1).to_string();
            let x = pos[0] + width - ui.calc_text_size(&label)[0] - 10.0;
            let y = pos[1] + layout.editor_top_margin + row as f32 * layout.line_height
                - view.scroll_position[1];
            if let Some(&severity) = severity_by_line.get(row as usize)
                && severity > 0
                && column_width > 0.0
            {
                let mark_width = 3.0_f32.max(column_width * 0.45);
                let x = pos[0] + (column_width - mark_width) * 0.5;
                draw.add_rect(
                    [x, y + 2.0],
                    [x + mark_width, y + layout.line_height - 2.0],
                    super::diagnostic_style::severity_mark(severity),
                )
                .filled(true)
                .build();
            }
            draw.add_text([x, y], color, label);
        }
    }
}
