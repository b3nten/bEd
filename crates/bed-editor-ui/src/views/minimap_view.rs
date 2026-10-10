//! Density strip, cached runs and viewport slider translated from ned
//! editor/views/minimap_view.{h,cpp}; see LICENSE and NOTICE.
use crate::{
    diff::{ProjectedRow, RowProjection},
    views::view_layout::{ViewLayout, glyph_advance},
};
use bed_editing::{editor_state::EditorState, editor_view_state::EditorViewState};
use bed_highlight::highlight_service::EditorHighlight;
use dear_imgui_rs::{MouseButton, Ui, WindowHoveredFlags, sys};
use std::hash::{Hash, Hasher};

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
    start_rows: f32,
    view_h: f32,
    max_scroll: f32,
    slider_top: f32,
    slider_h: f32,
    ratio: f32,
}
fn make_strip(
    n: i32,
    layout: &ViewLayout,
    scroll_y: f32,
    height: f32,
    d: Density,
    projection: Option<&RowProjection>,
) -> Strip {
    let mut s = Strip {
        end: -1,
        ..Default::default()
    };
    let elh = layout.line_height;
    let rows = projection.map_or(n as f32, RowProjection::total_rows);
    if rows <= 0.0 || height <= 1.0 || elh <= 0.0 {
        return s;
    }
    let scroll_y = scroll_y.max(0.0);
    s.view_h = elh.max((layout.size[1] - layout.editor_top_margin).max(0.0));
    s.max_scroll = (layout.total_height.max(s.view_h) - s.view_h).max(0.0);
    // Tiny strips can be shorter than the upstream minimum; avoid invalid clamp bounds.
    s.slider_h = ((s.view_h / elh) * d.line_h)
        .floor()
        .clamp(d.slider_min.min(height), height);
    let max_top = (height - s.slider_h).min((rows * d.line_h - s.slider_h).max(0.0));
    s.ratio = if s.max_scroll > 1.0 {
        max_top / s.max_scroll
    } else {
        0.0
    };
    s.slider_top = (scroll_y * s.ratio).clamp(0.0, max_top);
    let fit = ((height / d.line_h) as i32).max(1);
    if rows <= fit as f32 {
        s.end = projection.map_or(n - 1, |p| p.len() as i32 - 1);
    } else {
        s.start_rows = (scroll_y / elh - s.slider_top / d.line_h)
            .floor()
            .clamp(0.0, rows - fit as f32);
        s.start = projection.map_or(s.start_rows as i32, |p| p.visual_at_y(s.start_rows) as i32);
        s.end = projection.map_or((n - 1).min(s.start + fit - 1), |p| {
            p.visual_at_y(s.start_rows + fit as f32 - 0.001) as i32
        });
    }
    s
}
#[derive(Clone, Copy, PartialEq)]
struct CacheKey {
    version: i32,
    highlight_gen: u64,
    start: i32,
    end: i32,
    start_rows: f32,
    max_cols: i32,
    default_ink: [f32; 4],
    font_size: f32,
    projection_hash: u64,
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

fn minimap_wheel_scroll(ui: &Ui) -> Option<[f32; 2]> {
    if !ui.is_window_hovered_with_flags(
        WindowHoveredFlags::ROOT_AND_CHILD_WINDOWS
            | WindowHoveredFlags::NO_POPUP_HIERARCHY
            | WindowHoveredFlags::ALLOW_WHEN_BLOCKED_BY_ACTIVE_ITEM,
    ) {
        return None;
    }
    ui.with_bound_context(|| unsafe {
        let native = &*sys::igGetCurrentContext();
        let window_ptr = sys::igGetCurrentWindowRead();
        let window = &*window_ptr;
        // The minimap is beside the scrolling text child. Forward only wheel
        // input which native scrolling has not already consumed or locked away.
        if native.IO.KeyCtrl
            || native.WheelingWindowScrolledFrame == native.FrameCount
            || (!native.WheelingWindow.is_null() && native.WheelingWindow != window_ptr)
            || window.Collapsed
            || window.Flags
                & (sys::ImGuiWindowFlags_NoScrollWithMouse | sys::ImGuiWindowFlags_NoMouseInputs)
                != 0
        {
            return None;
        }
        let mut wheel = [
            if sys::igTestKeyOwner(sys::ImGuiKey_MouseWheelX, window.ID) {
                native.IO.MouseWheelH
            } else {
                0.0
            },
            if sys::igTestKeyOwner(sys::ImGuiKey_MouseWheelY, window.ID) {
                native.IO.MouseWheel
            } else {
                0.0
            },
        ];
        if native.IO.MouseWheelRequestAxisSwap {
            wheel = [wheel[1], 0.0];
        }
        let mut scroll_axis = [
            wheel[0] != 0.0 && window.ScrollMax.x != 0.0,
            wheel[1] != 0.0 && window.ScrollMax.y != 0.0,
        ];
        if scroll_axis[0] && scroll_axis[1] {
            scroll_axis[usize::from(native.WheelingAxisAvg.x > native.WheelingAxisAvg.y)] = false;
        }
        if !scroll_axis[0] && !scroll_axis[1] {
            return None;
        }
        let mut target = [window.Scroll.x, window.Scroll.y];
        let inner_size = [
            window.InnerRect.Max.x - window.InnerRect.Min.x,
            window.InnerRect.Max.y - window.InnerRect.Min.y,
        ];
        for axis in 0..2 {
            if scroll_axis[axis] {
                let font_step = window.FontRefSize * if axis == 0 { 2.0 } else { 5.0 };
                target[axis] -= wheel[axis] * font_step.min(inner_size[axis] * 0.67).trunc();
            }
        }
        Some(target)
    })
}
impl MinimapView {
    fn rebuild_density_cache(
        &mut self,
        ui: &Ui,
        key: CacheKey,
        state: &EditorState,
        highlight: &EditorHighlight,
        projection: Option<&RowProjection>,
    ) {
        self.cache_runs.clear();
        self.cache_key = Some(key);
        if key.end < key.start || key.max_cols <= 0 {
            return;
        }
        self.cache_runs
            .reserve(((key.end - key.start + 1) * 8) as usize);
        let d = Density::new(key.font_size);
        let space_width = glyph_advance(ui, " ");
        let cap = key.max_cols as usize * 4 + 8;
        let mut line = Vec::new();
        let mut loaded_line = None;
        for visual in key.start..=key.end {
            let height = projection.map_or(1.0, |p| p.row_height(visual as usize)) * d.line_h;
            if height <= 0.0 {
                continue;
            }
            let top = projection.map_or(visual as f32, |p| p.row_top(visual as usize));
            let (row, historical) = match projection.and_then(|p| p.rows.get(visual as usize)) {
                Some(ProjectedRow::Document { row, .. }) => {
                    if loaded_line != Some((false, *row as usize)) {
                        state.line_into(
                            *row,
                            &mut line,
                            if projection.is_some_and(RowProjection::is_wrapped) {
                                usize::MAX
                            } else {
                                cap
                            },
                        );
                        loaded_line = Some((false, *row as usize));
                    }
                    (*row, false)
                }
                Some(ProjectedRow::Historical { old_row, bytes }) => {
                    if loaded_line != Some((true, *old_row)) {
                        line.clear();
                        line.extend_from_slice(bytes);
                        loaded_line = Some((true, *old_row));
                    }
                    (-1, true)
                }
                Some(_) => continue,
                None => {
                    state.line_into(visual, &mut line, cap);
                    loaded_line = Some((false, visual as usize));
                    (visual, false)
                }
            };
            let segment = projection
                .and_then(|p| p.segment(visual as usize))
                .unwrap_or(0..line.len());
            let indent_columns =
                projection.map_or(0.0, |p| p.segment_indent(visual as usize)) / space_width;
            let max_cols = (key.max_cols as f32 - indent_columns).max(0.0);
            let spans = if historical {
                &[][..]
            } else {
                highlight.spans_for_line(row)
            };
            let mut sp = 0;
            let mut run_start: Option<i32> = None;
            let mut run_ink = key.default_ink;
            let mut col = 0;
            let mut i = segment.start;
            let flush = |col: i32, start: &mut Option<i32>, ink: [f32; 4], runs: &mut Vec<Run>| {
                if let Some(start) = start.take()
                    && (col as f32).min(max_cols) > start as f32
                {
                    runs.push(Run {
                        x: d.pad_x + (indent_columns + start as f32) * d.char_w,
                        y: (top - key.start_rows) * d.line_h,
                        w: ((col as f32).min(max_cols) - start as f32) * d.char_w,
                        h: d.dot_h.min(height),
                        color: ink,
                    });
                }
            };
            while i < segment.end && (col as f32) < max_cols {
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
                while i < segment.end && line[i] & 0xc0 == 0x80 {
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
        self.interact_projected(ui, state, view, layout, None);
    }
    pub(crate) fn interact_projected(
        &mut self,
        ui: &Ui,
        state: &EditorState,
        view: &mut EditorViewState,
        layout: &ViewLayout,
        projection: Option<&RowProjection>,
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
        let s = make_strip(
            projection.map_or(state.line_count(), |p| p.len() as i32),
            layout,
            view.scroll_position[1],
            height,
            d,
            projection,
        );
        if let Some([x, y]) = minimap_wheel_scroll(ui) {
            view.request_scroll(x, y);
        }
        if ui.is_mouse_clicked(MouseButton::Left) {
            let local = (mouse[1] - a[1]).clamp(0.0, height);
            let top = if s.end < s.start {
                0.0
            } else {
                let rows = s.start_rows + local / d.line_h;
                projection.map_or(rows.floor().clamp(s.start as f32, s.end as f32), |p| {
                    p.row_top(p.visual_at_y(rows))
                })
            };
            let y = top * layout.line_height - s.view_h * 0.5;
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
        self.draw_projected(ui, state, view, highlight, layout, None);
    }
    pub(crate) fn draw_projected(
        &mut self,
        ui: &Ui,
        state: &EditorState,
        view: &EditorViewState,
        highlight: &EditorHighlight,
        layout: &ViewLayout,
        projection: Option<&RowProjection>,
    ) {
        let w = layout.minimap_width;
        let h = layout.minimap_max[1] - layout.minimap_min[1];
        if w <= 1.0 || h <= 1.0 {
            return;
        }
        let a = layout.minimap_min;
        let d = Density::new(ui.current_font_size());
        let s = make_strip(
            projection.map_or(state.line_count(), |p| p.len() as i32),
            layout,
            view.scroll_position[1],
            h,
            d,
            projection,
        );
        if s.end < s.start {
            return;
        }
        ui.set_cursor_screen_pos(a);
        ui.invisible_button("##mm", [w, h]);
        let projection_hash = projection.map_or(0, |projection| {
            let mut hash = std::collections::hash_map::DefaultHasher::new();
            for visual in s.start..=s.end {
                projection
                    .row_top(visual as usize)
                    .to_bits()
                    .hash(&mut hash);
                projection
                    .row_height(visual as usize)
                    .to_bits()
                    .hash(&mut hash);
                projection.segment(visual as usize).hash(&mut hash);
                projection
                    .segment_indent(visual as usize)
                    .to_bits()
                    .hash(&mut hash);
                match projection.rows.get(visual as usize) {
                    Some(ProjectedRow::Document { row, .. }) => row.hash(&mut hash),
                    Some(ProjectedRow::Historical { old_row, bytes }) => {
                        old_row.hash(&mut hash);
                        bytes.hash(&mut hash);
                    }
                    _ => {}
                }
            }
            hash.finish()
        });
        let key = CacheKey {
            version: state.version,
            highlight_gen: highlight.visual_generation(),
            start: s.start,
            end: s.end,
            start_rows: s.start_rows,
            max_cols: (((w - d.pad_x * 2.0) / d.char_w) as i32).max(1),
            default_ink: dim(highlight.default_text_color()),
            font_size: ui.current_font_size(),
            projection_hash,
        };
        if self.cache_key != Some(key) {
            self.rebuild_density_cache(ui, key, state, highlight, projection);
        }
        let draw = ui.get_window_draw_list();
        let _clip = projection
            .filter(|p| p.is_cropped())
            .map(|_| draw.push_clip_rect(a, [a[0] + w, a[1] + h], true));
        for run in &self.cache_runs {
            let p = [a[0] + run.x, a[1] + run.y];
            draw.add_rect(p, [p[0] + run.w, p[1] + run.h], run.color)
                .filled(true)
                .build();
        }
        if s.slider_h > 0.0 {
            let y0 = a[1] + s.slider_top;
            let y1 = (a[1] + h).min(y0 + s.slider_h);
            draw.add_rect([a[0], y0], [a[0] + w, y1], {
                let mut color = ui.style_color(dear_imgui_rs::StyleColor::Text);
                color[3] = 0.12;
                color
            })
            .filled(true)
            .build();
            draw.add_rect([a[0] + 0.5, y0], [a[0] + w - 0.5, y1], {
                let mut color = ui.style_color(dear_imgui_rs::StyleColor::Text);
                color[3] = 0.45;
                color
            })
            .build();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dear_imgui_rs::{Condition, Context, FramePrepareOptions, Key, WindowFlags};

    fn wheel_frame(context: &mut Context, size: [f32; 2], draw: impl FnOnce(&Ui)) {
        context.prepare_frame(FramePrepareOptions::new([640.0, 480.0], 1.0 / 60.0));
        let ui = context.frame();
        ui.window("minimap wheel fixture")
            .position([0.0; 2], Condition::Always)
            .size([640.0, 480.0], Condition::Always)
            .flags(WindowFlags::NO_TITLE_BAR | WindowFlags::NO_SCROLL_WITH_MOUSE)
            .build(|| {
                ui.with_bound_context(|| unsafe {
                    sys::igSetNextWindowContentSize([1000.0, 2000.0].into());
                });
                ui.child_window("text")
                    .size(size)
                    .flags(WindowFlags::HORIZONTAL_SCROLLBAR)
                    .build(ui, || draw(ui));
            });
        drop(context.render_legacy());
    }

    fn wheel_context(size: [f32; 2]) -> Context {
        let mut context = Context::create();
        context
            .set_ini_filename(None::<std::path::PathBuf>)
            .unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        context.io_mut().add_mouse_pos_event([500.0, 300.0]);
        for _ in 0..3 {
            wheel_frame(&mut context, size, |_| {});
        }
        context
    }

    fn wheel_request(ui: &Ui) -> Option<[f32; 2]> {
        let mut state = EditorState::new();
        state.set_from_bytes(&b"x\n".repeat(1000));
        let mut view = EditorViewState::default();
        let mouse = ui.io().mouse_pos();
        let layout = ViewLayout {
            line_height: 20.0,
            total_height: 20000.0,
            size: [200.0, 180.0],
            minimap_width: 40.0,
            minimap_min: [mouse[0] - 20.0, mouse[1] - 20.0],
            minimap_max: [mouse[0] + 20.0, mouse[1] + 20.0],
            ..Default::default()
        };
        MinimapView::default().interact(ui, &state, &mut view, &layout);
        view.requested_scroll
    }

    #[test]
    fn minimap_wheel_uses_native_font_and_viewport_steps() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        for size in [[200.0, 180.0], [40.0, 36.0]] {
            let mut context = wheel_context(size);
            for axis in 0..2 {
                let mut wheel = [0.0; 2];
                wheel[axis] = -2.0;
                context.io_mut().add_mouse_wheel_event(wheel);
                wheel_frame(&mut context, size, |ui| {
                    let (step, font_step) = ui.with_bound_context(|| unsafe {
                        let window = &*sys::igGetCurrentWindowRead();
                        let extent = if axis == 0 {
                            window.InnerRect.Max.x - window.InnerRect.Min.x
                        } else {
                            window.InnerRect.Max.y - window.InnerRect.Min.y
                        };
                        let font_step = window.FontRefSize * if axis == 0 { 2.0 } else { 5.0 };
                        (font_step.min(extent * 0.67).trunc(), font_step)
                    });
                    let mut expected = [ui.scroll_x(), ui.scroll_y()];
                    expected[axis] += 2.0 * step;
                    assert_eq!(wheel_request(ui), Some(expected));
                    if size[1] < 40.0 {
                        assert!(step < font_step, "small viewport caps each axis");
                    }
                });
            }
        }
    }

    #[test]
    fn minimap_wheel_respects_modifiers_axis_priority_and_key_owners() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let size = [200.0, 180.0];
        let mut context = wheel_context(size);
        context.io_mut().set_config_macosx_behaviors(false);
        context.io_mut().add_key_event(Key::ModShift, true);
        context.io_mut().add_mouse_wheel_event([0.0, -1.0]);
        wheel_frame(&mut context, size, |ui| {
            let request = wheel_request(ui).unwrap();
            assert!(request[0] > 0.0 && request[1] == 0.0);
        });
        context.io_mut().set_config_macosx_behaviors(true);
        context.io_mut().add_mouse_wheel_event([0.0, -1.0]);
        wheel_frame(&mut context, size, |ui| {
            let request = wheel_request(ui).unwrap();
            assert!(request[0] == 0.0 && request[1] > 0.0);
        });
        context.io_mut().set_config_macosx_behaviors(false);
        context.io_mut().add_key_event(Key::ModShift, false);
        context.io_mut().add_key_event(Key::ModCtrl, true);
        context.io_mut().add_mouse_wheel_event([0.0, -1.0]);
        wheel_frame(&mut context, size, |ui| assert_eq!(wheel_request(ui), None));
        context.io_mut().add_key_event(Key::ModCtrl, false);
        context.io_mut().add_key_event(Key::ModShift, false);
        context.io_mut().add_mouse_wheel_event([-1.0, -1.0]);
        wheel_frame(&mut context, size, |ui| {
            ui.with_bound_context(|| unsafe {
                (*sys::igGetCurrentContext()).WheelingAxisAvg = [9.0, 2.0].into();
            });
            let request = wheel_request(ui).unwrap();
            assert!(request[0] > 0.0 && request[1] == 0.0);
            ui.with_bound_context(|| unsafe {
                (*sys::igGetCurrentContext()).WheelingAxisAvg = [1.0, 9.0].into();
            });
            let request = wheel_request(ui).unwrap();
            assert!(request[0] == 0.0 && request[1] > 0.0);
            ui.with_bound_context(|| unsafe {
                sys::igSetKeyOwner(sys::ImGuiKey_MouseWheelX, 123, 0);
                sys::igSetKeyOwner(sys::ImGuiKey_MouseWheelY, 123, 0);
            });
            assert_eq!(wheel_request(ui), None);
        });
    }

    #[test]
    fn minimap_wheel_does_not_repeat_native_text_child_scrolling() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let size = [200.0, 180.0];
        let mut context = wheel_context(size);
        context.io_mut().add_mouse_pos_event([30.0, 40.0]);
        wheel_frame(&mut context, size, |_| {});
        context.io_mut().add_mouse_wheel_event([0.0, -1.0]);
        wheel_frame(&mut context, size, |ui| {
            assert!(ui.scroll_y() > 0.0);
            ui.with_bound_context(|| unsafe {
                let native = &*sys::igGetCurrentContext();
                assert_eq!(native.WheelingWindow, sys::igGetCurrentWindowRead());
                assert_eq!(native.WheelingWindowScrolledFrame, native.FrameCount);
            });
            assert_eq!(wheel_request(ui), None);
        });
    }

    #[test]
    fn popup_without_scroll_extent_blocks_minimap_wheel_forwarding() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let size = [200.0, 180.0];
        let mut context = wheel_context(size);
        let popup = |ui: &Ui| {
            ui.with_bound_context(|| unsafe {
                sys::igSetNextWindowPos([480.0, 280.0].into(), 0, [0.0; 2].into());
                sys::igSetNextWindowSize([120.0, 80.0].into(), 0);
            });
            ui.popup("minimap wheel blocker", || {
                ui.text("Popup");
                assert_eq!([ui.scroll_max_x(), ui.scroll_max_y()], [0.0; 2]);
            });
        };
        wheel_frame(&mut context, size, |ui| {
            ui.open_popup("minimap wheel blocker");
            popup(ui);
        });
        wheel_frame(&mut context, size, popup);
        context.io_mut().add_mouse_wheel_event([0.0, -1.0]);
        wheel_frame(&mut context, size, |ui| {
            popup(ui);
            assert_eq!(ui.io().mouse_wheel(), -1.0);
            ui.with_bound_context(|| unsafe {
                let native = &*sys::igGetCurrentContext();
                assert_ne!(native.WheelingWindowScrolledFrame, native.FrameCount);
                assert!(sys::igTestKeyOwner(
                    sys::ImGuiKey_MouseWheelY,
                    (*sys::igGetCurrentWindowRead()).ID,
                ));
            });
            assert_eq!(wheel_request(ui), None);
        });
    }

    #[test]
    fn folded_density_keeps_only_visible_header_and_following_lines() {
        use bed_editing::folding::FoldRange;

        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = wheel_context([200.0, 180.0]);
        let mut state = EditorState::new();
        state.set_from_bytes(b"HEADER\nHIDDEN\nHIDDEN\nAFTER");
        let highlight = EditorHighlight::new();
        let view = EditorViewState::default();
        let mut minimap = MinimapView::default();
        wheel_frame(&mut context, [200.0, 180.0], |ui| {
            let mut projection = RowProjection::build(&state, None, None);
            projection.fold(&[FoldRange {
                start_line: 0,
                end_line: 2,
            }]);
            let layout = ViewLayout {
                size: [200.0, 180.0],
                line_height: ui.text_line_height(),
                total_height: ui.text_line_height() * 2.0,
                minimap_width: 40.0,
                minimap_min: [30.0, 40.0],
                minimap_max: [70.0, 200.0],
                ..Default::default()
            };
            minimap.draw_projected(ui, &state, &view, &highlight, &layout, Some(&projection));
            assert_eq!(
                minimap.cache_runs.len(),
                2,
                "hidden rows add no density runs"
            );
            let d = Density::new(ui.current_font_size());
            assert_eq!(minimap.cache_runs[0].w, 6.0 * d.char_w);
            assert_eq!(minimap.cache_runs[1].w, 5.0 * d.char_w);
            assert_eq!(minimap.cache_runs[1].y, d.line_h);
        });
    }

    #[test]
    fn animated_density_tracks_partial_rows_even_when_the_visible_row_count_stays_the_same() {
        use crate::fold_animation::FoldVisual;
        use bed_editing::folding::FoldRange;

        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = wheel_context([200.0, 180.0]);
        let mut state = EditorState::new();
        state.set_from_bytes(b"HEADER\nFIRST\nSECOND\nTHIRD\nAFTER");
        let highlight = EditorHighlight::new();
        let view = EditorViewState::default();
        let mut minimap = MinimapView::default();
        let range = FoldRange {
            start_line: 0,
            end_line: 3,
        };
        for openness in [0.5, 0.6] {
            wheel_frame(&mut context, [200.0, 180.0], |ui| {
                let visual = [FoldVisual { range, openness }];
                let mut projection = RowProjection::build(&state, None, None);
                projection.fold_animated(&[range], &visual);
                projection.crop_folds(&visual);
                let layout = ViewLayout {
                    size: [200.0, 180.0],
                    line_height: ui.text_line_height(),
                    total_height: ui.text_line_height() * projection.total_rows(),
                    minimap_width: 40.0,
                    minimap_min: [30.0, 40.0],
                    minimap_max: [70.0, 200.0],
                    ..Default::default()
                };
                minimap.draw_projected(ui, &state, &view, &highlight, &layout, Some(&projection));
                let d = Density::new(ui.current_font_size());
                assert_eq!(minimap.cache_runs.len(), 4);
                assert!(
                    (minimap.cache_runs[3].y - (1.0 + 3.0 * openness) * d.line_h).abs() < 0.001
                );
                assert!(
                    (minimap.cache_runs[2].h - d.dot_h.min((3.0 * openness - 1.0) * d.line_h))
                        .abs()
                        < 0.001
                );
            });
        }
    }

    #[test]
    fn wrapped_density_reflows_when_resize_keeps_the_same_visual_row_count() {
        use crate::views::view_layout::glyph_advance;

        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = wheel_context([200.0, 180.0]);
        let mut state = EditorState::new();
        state.set_from_bytes(b"AAAAA");
        let highlight = EditorHighlight::new();
        let view = EditorViewState::default();
        let mut minimap = MinimapView::default();
        for first_line_chars in [3, 4] {
            wheel_frame(&mut context, [200.0, 180.0], |ui| {
                let mut projection = RowProjection::build(&state, None, None);
                projection.wrap(
                    ui,
                    &state,
                    glyph_advance(ui, "A") * first_line_chars as f32 + 0.01,
                );
                assert_eq!(projection.len(), 2);
                let layout = ViewLayout {
                    size: [200.0, 180.0],
                    line_height: ui.text_line_height(),
                    total_height: ui.text_line_height() * 2.0,
                    minimap_width: 40.0,
                    minimap_min: [30.0, 40.0],
                    minimap_max: [70.0, 200.0],
                    ..Default::default()
                };
                let start = ui
                    .with_bound_context(|| unsafe { (*sys::igGetWindowDrawList()).VtxBuffer.Size });
                minimap.draw_projected(ui, &state, &view, &highlight, &layout, Some(&projection));
                ui.with_bound_context(|| unsafe {
                    let vertices = &(*sys::igGetWindowDrawList()).VtxBuffer;
                    let ink =
                        sys::igColorConvertFloat4ToU32(dim(highlight.default_text_color()).into());
                    let density: Vec<_> = (start..vertices.Size)
                        .map(|index| *vertices.Data.add(index as usize))
                        .filter(|v| v.col == ink)
                        .collect();
                    assert_eq!(density.len(), 8, "two visual rows appear in the minimap");
                    let d = Density::new(ui.current_font_size());
                    assert!(
                        (density[1].pos.x - density[0].pos.x - first_line_chars as f32 * d.char_w)
                            .abs()
                            < 0.01
                    );
                    assert!(
                        (density[5].pos.x
                            - density[4].pos.x
                            - (5 - first_line_chars) as f32 * d.char_w)
                            .abs()
                            < 0.01
                    );
                    assert!((density[4].pos.y - density[0].pos.y - d.line_h).abs() < 0.01);
                });
            });
        }
    }

    #[test]
    fn wrapped_density_preserves_indentation_and_local_tab_stops() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = wheel_context([200.0, 180.0]);
        let highlight = EditorHighlight::new();
        let view = EditorViewState::default();
        for (source, width_columns, expected_columns) in [
            (b"        AAAA BBBB CCCC".as_slice(), 20.0, vec![8.0]),
            (b"\t\tAAAA BBBB CCCC", 20.0, vec![8.0]),
            (b"  AAAA BBBB C\tD", 12.0, vec![2.0, 6.0]),
        ] {
            let mut state = EditorState::new();
            state.set_from_bytes(source);
            let mut minimap = MinimapView::default();
            wheel_frame(&mut context, [200.0, 180.0], |ui| {
                let mut projection = RowProjection::build(&state, None, None);
                projection.wrap(ui, &state, glyph_advance(ui, " ") * width_columns + 0.01);
                assert_eq!(projection.len(), 2);
                let layout = ViewLayout {
                    size: [200.0, 180.0],
                    line_height: ui.text_line_height(),
                    total_height: ui.text_line_height() * 2.0,
                    minimap_width: 40.0,
                    minimap_min: [30.0, 40.0],
                    minimap_max: [70.0, 200.0],
                    ..Default::default()
                };
                let start = ui
                    .with_bound_context(|| unsafe { (*sys::igGetWindowDrawList()).VtxBuffer.Size });
                minimap.draw_projected(ui, &state, &view, &highlight, &layout, Some(&projection));
                ui.with_bound_context(|| unsafe {
                    let vertices = &(*sys::igGetWindowDrawList()).VtxBuffer;
                    let ink =
                        sys::igColorConvertFloat4ToU32(dim(highlight.default_text_color()).into());
                    let density: Vec<_> = (start..vertices.Size)
                        .map(|index| *vertices.Data.add(index as usize))
                        .filter(|v| v.col == ink)
                        .collect();
                    let d = Density::new(ui.current_font_size());
                    let continuation: Vec<_> = density
                        .chunks_exact(4)
                        .filter(|rectangle| {
                            (rectangle[0].pos.y - layout.minimap_min[1] - d.line_h).abs() < 0.01
                        })
                        .collect();
                    assert_eq!(continuation.len(), expected_columns.len());
                    for (rectangle, column) in continuation.iter().zip(&expected_columns) {
                        let expected_x = layout.minimap_min[0] + d.pad_x + column * d.char_w;
                        assert!((rectangle[0].pos.x - expected_x).abs() < 0.01);
                    }
                });
                assert_eq!(state.join(), source);
            });
        }
    }

    #[test]
    fn wrapped_density_cache_updates_when_only_capped_indent_changes() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = wheel_context([200.0, 180.0]);
        let mut state = EditorState::new();
        state.set_from_bytes(b"        AB");
        let highlight = EditorHighlight::new();
        let view = EditorViewState::default();
        let mut minimap = MinimapView::default();
        for width_columns in [9.0, 9.5] {
            wheel_frame(&mut context, [200.0, 180.0], |ui| {
                let space = glyph_advance(ui, " ");
                let mut projection = RowProjection::build(&state, None, None);
                projection.wrap(ui, &state, space * width_columns);
                assert_eq!(projection.len(), 2);
                assert_eq!(projection.segment(0), Some(0..9));
                assert_eq!(projection.segment(1), Some(9..10));
                let layout = ViewLayout {
                    size: [200.0, 180.0],
                    line_height: ui.text_line_height(),
                    total_height: ui.text_line_height() * 2.0,
                    minimap_width: 40.0,
                    minimap_min: [30.0, 40.0],
                    minimap_max: [70.0, 200.0],
                    ..Default::default()
                };
                let start = ui
                    .with_bound_context(|| unsafe { (*sys::igGetWindowDrawList()).VtxBuffer.Size });
                minimap.draw_projected(ui, &state, &view, &highlight, &layout, Some(&projection));
                ui.with_bound_context(|| unsafe {
                    let vertices = &(*sys::igGetWindowDrawList()).VtxBuffer;
                    let ink =
                        sys::igColorConvertFloat4ToU32(dim(highlight.default_text_color()).into());
                    let density: Vec<_> = (start..vertices.Size)
                        .map(|index| *vertices.Data.add(index as usize))
                        .filter(|v| v.col == ink)
                        .collect();
                    assert_eq!(density.len(), 8);
                    let d = Density::new(ui.current_font_size());
                    let expected_x =
                        layout.minimap_min[0] + d.pad_x + width_columns * 0.5 * d.char_w;
                    assert!((density[4].pos.x - expected_x).abs() < 0.01);
                });
            });
        }
    }

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
        let start = make_strip(state.line_count(), &layout, 0.0, 400.0, d, None);
        let end = make_strip(state.line_count(), &layout, 19620.0, 400.0, d, None);
        assert_eq!((start.start, start.end), (0, 199));
        assert_eq!((end.start, end.end), (801, 1000));
        assert!((end.slider_top - 360.0).abs() < 0.001);
        assert_eq!(end.slider_h, 40.0);
        let middle = make_strip(state.line_count(), &layout, 9800.0, 400.0, d, None);
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
        let strip = make_strip(
            state.line_count(),
            &layout,
            0.0,
            400.0,
            Density::new(20.0),
            None,
        );
        assert_eq!((strip.start, strip.end), (0, 1));
        assert_eq!(strip.ratio, 0.0);
    }
}
