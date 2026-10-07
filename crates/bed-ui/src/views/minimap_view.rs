//! Density strip, cached runs and viewport slider translated from ned
//! editor/views/minimap_view.{h,cpp}; see LICENSE and NOTICE.
use crate::views::view_layout::ViewLayout;
use bed_core::{editor_state::EditorState, editor_view_state::EditorViewState};
use bed_highlight::highlight_service::EditorHighlight;
use dear_imgui_rs::{MouseButton, Ui};

pub const WIDTH_FONT_MUL: f32 = 4.0;
pub const MIN_PANE_FONT_MUL: f32 = 16.0;
#[derive(Clone, Copy)]
struct Density {
    line_h: f32,
    char_w: f32,
    dot_h: f32,
    pad_x: f32,
    slider_min: f32,
}
impl Density {
    fn new(fs: f32) -> Self {
        let fs = fs.max(1.0);
        let line_h = (fs * 0.1).max(1.0);
        Self {
            line_h,
            char_w: (fs * 0.05).max(1.0),
            dot_h: (line_h * 0.75).max(1.0),
            pad_x: (fs * 0.1).max(1.0),
            slider_min: (fs * 0.2).max(4.0),
        }
    }
}
#[derive(Default)]
struct Strip {
    start: i32,
    end: i32,
    view_h: f32,
    max_scroll: f32,
    slider_top: f32,
    slider_h: f32,
    ratio: f32,
}
fn make_strip(
    state: &EditorState,
    layout: &ViewLayout,
    scroll_y: f32,
    height: f32,
    d: Density,
) -> Strip {
    let mut s = Strip {
        end: -1,
        ..Default::default()
    };
    let n = state.line_count();
    let elh = layout.line_height;
    if n <= 0 || height <= 1.0 || elh <= 0.0 {
        return s;
    }
    let scroll_y = scroll_y.max(0.0);
    s.view_h = elh.max((layout.size[1] - layout.editor_top_margin).max(0.0));
    s.max_scroll = (layout.total_height.max(s.view_h) - s.view_h).max(0.0);
    // Tiny strips can be shorter than the upstream minimum; avoid invalid clamp bounds.
    s.slider_h = ((s.view_h / elh) * d.line_h)
        .floor()
        .clamp(d.slider_min.min(height), height);
    let max_top = (height - s.slider_h).min((n as f32 * d.line_h - s.slider_h).max(0.0));
    s.ratio = if s.max_scroll > 1.0 {
        max_top / s.max_scroll
    } else {
        0.0
    };
    s.slider_top = (scroll_y * s.ratio).clamp(0.0, max_top);
    let fit = ((height / d.line_h) as i32).max(1);
    if n <= fit {
        s.end = n - 1;
    } else {
        s.start = (scroll_y / elh - s.slider_top / d.line_h).floor() as i32;
        s.start = s.start.clamp(0, n - fit);
        s.end = (n - 1).min(s.start + fit - 1);
    }
    s
}
#[derive(Clone, Copy, PartialEq)]
struct CacheKey {
    version: i32,
    highlight_gen: u64,
    start: i32,
    end: i32,
    max_cols: i32,
    default_ink: [f32; 4],
    font_size: f32,
}
#[derive(Clone, Copy)]
struct Run {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    color: [f32; 4],
}
#[derive(Default)]
pub struct MinimapView {
    dragging: bool,
    drag_y0: f32,
    drag_scroll0: f32,
    drag_ratio: f32,
    cache_key: Option<CacheKey>,
    cache_runs: Vec<Run>,
}
fn dim(mut color: [f32; 4]) -> [f32; 4] {
    for c in &mut color[..3] {
        *c *= 0.72;
    }
    color
}
impl MinimapView {
    fn rebuild_density_cache(
        &mut self,
        key: CacheKey,
        state: &EditorState,
        highlight: &EditorHighlight,
    ) {
        self.cache_runs.clear();
        self.cache_key = Some(key);
        if key.end < key.start || key.max_cols <= 0 {
            return;
        }
        self.cache_runs
            .reserve(((key.end - key.start + 1) * 8) as usize);
        let d = Density::new(key.font_size);
        let cap = key.max_cols as usize * 4 + 8;
        let mut line = Vec::new();
        for row in key.start..=key.end {
            state.line_into(row, &mut line, cap);
            let spans = highlight.spans_for_line(row);
            let mut sp = 0;
            let mut run_start: Option<i32> = None;
            let mut run_ink = key.default_ink;
            let mut col = 0;
            let mut i = 0;
            let flush = |col: i32, start: &mut Option<i32>, ink: [f32; 4], runs: &mut Vec<Run>| {
                if let Some(start) = start.take()
                    && col > start
                {
                    runs.push(Run {
                        x: d.pad_x + start as f32 * d.char_w,
                        y: (row - key.start) as f32 * d.line_h,
                        w: (col - start) as f32 * d.char_w,
                        h: d.dot_h,
                        color: ink,
                    });
                }
            };
            while i < line.len() && col < key.max_cols {
                let byte = i as i32;
                let c = line[i];
                i += 1;
                if c & 0xc0 == 0x80 {
                    continue;
                }
                if c == b'\t' {
                    flush(col, &mut run_start, run_ink, &mut self.cache_runs);
                    col = key.max_cols.min(col + (4 - col % 4));
                    continue;
                }
                if c <= b' ' {
                    flush(col, &mut run_start, run_ink, &mut self.cache_runs);
                    col += 1;
                    continue;
                }
                while sp < spans.len() && spans[sp].end <= byte {
                    sp += 1;
                }
                let ink = spans
                    .get(sp)
                    .filter(|s| s.start <= byte)
                    .map_or(key.default_ink, |s| dim(highlight.color_for_slot(s.slot)));
                if run_start.is_none() || ink != run_ink {
                    flush(col, &mut run_start, run_ink, &mut self.cache_runs);
                    run_start = Some(col);
                    run_ink = ink;
                }
                col += 1;
                while i < line.len() && line[i] & 0xc0 == 0x80 {
                    i += 1;
                }
            }
            flush(col, &mut run_start, run_ink, &mut self.cache_runs);
        }
    }
    pub fn interact(
        &mut self,
        ui: &Ui,
        state: &EditorState,
        view: &mut EditorViewState,
        layout: &ViewLayout,
    ) {
        if layout.minimap_width <= 0.5 {
            return;
        }
        let a = layout.minimap_min;
        let b = layout.minimap_max;
        let height = b[1] - a[1];
        if height <= 1.0 {
            return;
        }
        if self.dragging {
            if !ui.is_mouse_down(MouseButton::Left) {
                self.dragging = false;
                return;
            }
            if self.drag_ratio > 1e-6 {
                view.request_scroll(
                    view.scroll_position[0],
                    self.drag_scroll0 + (ui.io().mouse_pos()[1] - self.drag_y0) / self.drag_ratio,
                );
            }
            return;
        }
        let mouse = ui.io().mouse_pos();
        if mouse[0] < a[0] || mouse[0] > b[0] || mouse[1] < a[1] || mouse[1] > b[1] {
            return;
        }
        let d = Density::new(ui.current_font_size());
        let s = make_strip(state, layout, view.scroll_position[1], height, d);
        if ui.io().mouse_wheel() != 0.0 && layout.line_height > 0.0 {
            view.request_scroll(
                view.scroll_position[0],
                view.scroll_position[1] - ui.io().mouse_wheel() * layout.line_height * 3.0,
            );
        }
        if ui.is_mouse_clicked(MouseButton::Left) {
            let local = (mouse[1] - a[1]).clamp(0.0, height);
            let line = if s.end < s.start {
                0
            } else {
                (s.start + (local / d.line_h) as i32).clamp(s.start, s.end)
            };
            let y = line as f32 * layout.line_height - s.view_h * 0.5;
            view.request_scroll(view.scroll_position[0], y);
            self.dragging = true;
            self.drag_y0 = mouse[1];
            self.drag_scroll0 = y;
            self.drag_ratio = if s.ratio > 1e-6 {
                s.ratio
            } else if s.max_scroll > 1.0 {
                height / s.max_scroll
            } else {
                0.0
            };
        }
    }
    pub fn draw(
        &mut self,
        ui: &Ui,
        state: &EditorState,
        view: &EditorViewState,
        highlight: &EditorHighlight,
        layout: &ViewLayout,
    ) {
        let w = layout.minimap_width;
        let h = layout.minimap_max[1] - layout.minimap_min[1];
        if w <= 1.0 || h <= 1.0 {
            return;
        }
        let a = layout.minimap_min;
        let d = Density::new(ui.current_font_size());
        let s = make_strip(state, layout, view.scroll_position[1], h, d);
        if s.end < s.start {
            return;
        }
        ui.set_cursor_screen_pos(a);
        ui.invisible_button("##mm", [w, h]);
        let key = CacheKey {
            version: state.version,
            highlight_gen: highlight.visual_generation(),
            start: s.start,
            end: s.end,
            max_cols: (((w - d.pad_x * 2.0) / d.char_w) as i32).max(1),
            default_ink: dim(highlight.default_text_color()),
            font_size: ui.current_font_size(),
        };
        if self.cache_key != Some(key) {
            self.rebuild_density_cache(key, state, highlight);
        }
        let draw = ui.get_window_draw_list();
        for run in &self.cache_runs {
            let p = [a[0] + run.x, a[1] + run.y];
            draw.add_rect(p, [p[0] + run.w, p[1] + run.h], run.color)
                .filled(true)
                .build();
        }
        if s.slider_h > 0.0 {
            let y0 = a[1] + s.slider_top;
            let y1 = (a[1] + h).min(y0 + s.slider_h);
            draw.add_rect(
                [a[0], y0],
                [a[0] + w, y1],
                [180.0 / 255.0, 180.0 / 255.0, 220.0 / 255.0, 40.0 / 255.0],
            )
            .filled(true)
            .build();
            draw.add_rect(
                [a[0] + 0.5, y0],
                [a[0] + w - 0.5, y1],
                [220.0 / 255.0, 220.0 / 255.0, 1.0, 110.0 / 255.0],
            )
            .build();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sliding_strip_tracks_viewport_at_start_middle_and_end() {
        let mut state = EditorState::new();
        state.set_from_bytes(&b"x\n".repeat(1000));
        let layout = ViewLayout {
            line_height: 20.0,
            size: [500.0, 400.0],
            total_height: 20020.0,
            ..Default::default()
        };
        let d = Density::new(20.0);
        let start = make_strip(&state, &layout, 0.0, 400.0, d);
        let end = make_strip(&state, &layout, 19620.0, 400.0, d);
        assert_eq!((start.start, start.end), (0, 199));
        assert_eq!((end.start, end.end), (801, 1000));
        assert!((end.slider_top - 360.0).abs() < 0.001);
        assert_eq!(end.slider_h, 40.0);
        let middle = make_strip(&state, &layout, 9800.0, 400.0, d);
        assert!(middle.start > 0 && middle.end < 1000);
    }
    #[test]
    fn short_document_keeps_all_rows_and_zero_scroll_ratio() {
        let mut state = EditorState::new();
        state.set_from_bytes(b"a\nb");
        let layout = ViewLayout {
            line_height: 20.0,
            size: [500.0, 400.0],
            total_height: 40.0,
            ..Default::default()
        };
        let strip = make_strip(&state, &layout, 0.0, 400.0, Density::new(20.0));
        assert_eq!((strip.start, strip.end), (0, 1));
        assert_eq!(strip.ratio, 0.0);
    }
}
