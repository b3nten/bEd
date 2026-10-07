//! Presentation frame translated from ned editor/editor_frame.cpp. The basic
//! Draws the custom document views and coordinates exclusive service overlays.
use crate::{
    editor_input::{EditorInput, HostAction},
    util::{editor_finder::EditorFinder, editor_line_jump::EditorLineJump},
    views::{
        caret_view::CaretView,
        gutter_view::GutterView,
        hover_tooltip::{TooltipArbiter, render_diagnostic_tooltip},
        hover_trigger::{HoverTrigger, Info, Target, Zone},
        minimap_view::{MIN_PANE_FONT_MUL, MinimapView, WIDTH_FONT_MUL},
        text_view::TextView,
        view_layout::{ViewLayout, column_at_x, glyph_advance_bytes, line_column_x},
    },
};
use bed_core::editor_events::Overlay;
use bed_session::{ViewContext, editor::Editor};
#[cfg(test)]
use dear_imgui_rs::{Condition, StyleColor};
use dear_imgui_rs::{FocusedFlags, Key, StyleVar, Ui, WindowFlags, sys};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

// Explicit location jumps ease into place; ordinary scrolling stays native.
const SCROLL_SPRING_FREQUENCY: f32 = 36.0;

struct SmoothScroll {
    position: [f32; 2],
    target: [f32; 2],
    velocity: [f32; 2],
    last_requested: [f32; 2],
}

impl SmoothScroll {
    fn new(position: [f32; 2], target: [f32; 2]) -> Self {
        Self {
            position,
            target,
            velocity: [0.0; 2],
            last_requested: position,
        }
    }

    fn advance(&mut self, delta: f32) {
        // Exact critically damped integration preserves velocity on retarget
        // and keeps the same trajectory across frame rates and pixel rounding.
        let delta = delta.max(0.0);
        let decay = (-SCROLL_SPRING_FREQUENCY * delta).exp();
        for axis in 0..2 {
            let displacement = self.position[axis] - self.target[axis];
            let rate = self.velocity[axis] + SCROLL_SPRING_FREQUENCY * displacement;
            self.position[axis] = self.target[axis] + (displacement + rate * delta) * decay;
            self.velocity[axis] =
                (self.velocity[axis] - SCROLL_SPRING_FREQUENCY * rate * delta) * decay;
        }
    }

    fn settled(&self) -> bool {
        (0..2).all(|axis| {
            (self.position[axis] - self.target[axis]).abs() < 0.35
                && self.velocity[axis].abs() < 12.0
        })
    }
}

pub struct EditorFrame {
    pub layout: ViewLayout,
    pub finder: EditorFinder,
    pub line_jump: EditorLineJump,
    pub error: Option<String>,
    dirty_rows: Rc<RefCell<Vec<(i32, i32, i32)>>>,
    width_full: bool,
    width_max: f32,
    width_longest: i32,
    width_lines: i32,
    width_font: f32,
    width_document_generation: u64,
    smooth_scroll: Option<SmoothScroll>,
    pub navigation_animations: bool,
    was_focused: bool,
    pub rainbow_mode: bool,
    pub line_jump_key: Option<Key>,
    pub external_overlay: bool,
    pub background_color: [f32; 4],
    pub minimap_enabled: bool,
    pub minimap: MinimapView,
    exclusive_overlay: Rc<Cell<Option<Overlay>>>,
    hover_trigger: HoverTrigger,
    pub tooltip_arbiter: TooltipArbiter,
    frame_dismissed: bool,
    last_mouse_pos: [f32; 2],
    last_scroll: [f32; 2],
}

impl EditorFrame {
    pub fn new(editor: &mut ViewContext<'_>) -> Self {
        let dirty_rows = Rc::new(RefCell::new(Vec::new()));
        let changes = Rc::downgrade(&dirty_rows);
        editor
            .events()
            .subscribe_did_edit_document_weak(move |event, state| {
                let Some(changes) = changes.upgrade() else {
                    return false;
                };
                changes
                    .borrow_mut()
                    .push((event.first_row, event.last_row, state.line_count()));
                true
            });
        let exclusive_overlay = Rc::new(Cell::new(None));
        let request = Rc::downgrade(&exclusive_overlay);
        editor.events().subscribe_overlay_weak(move |event| {
            let Some(request) = request.upgrade() else {
                return false;
            };
            if let Some(event) = event {
                request.set(Some(event.keep));
            }
            true
        });
        Self {
            layout: ViewLayout::default(),
            finder: EditorFinder::default(),
            line_jump: EditorLineJump::default(),
            error: None,
            dirty_rows,
            width_full: true,
            width_max: 0.0,
            width_longest: -1,
            width_lines: 0,
            width_font: 0.0,
            width_document_generation: 0,
            smooth_scroll: None,
            navigation_animations: true,
            was_focused: false,
            rainbow_mode: true,
            line_jump_key: Some(Key::Semicolon),
            external_overlay: false,
            background_color: [0.0, 0.0, 0.0, 1.0],
            minimap_enabled: false,
            minimap: MinimapView::default(),
            exclusive_overlay,
            hover_trigger: HoverTrigger::new(),
            tooltip_arbiter: TooltipArbiter::default(),
            frame_dismissed: false,
            last_mouse_pos: [0.0; 2],
            last_scroll: [0.0; 2],
        }
    }

    pub fn invalidate_content_width(&mut self) {
        self.width_full = true;
        self.finder.invalidate_matches();
    }

    fn close_internal_overlays_except(&mut self, keep: Overlay) {
        if keep != Overlay::Find {
            self.finder.dismiss();
        }
        if keep != Overlay::LineJump {
            self.line_jump.dismiss();
        }
    }

    fn content_width(&mut self, ui: &Ui, editor: &Editor) -> f32 {
        let dirty = std::mem::take(&mut *self.dirty_rows.borrow_mut());
        let font_size = ui.current_font_size();
        let measure = |row| {
            let line = editor.state.line(row);
            let width = glyph_advance_bytes(ui, &line);
            (width + line.len() as f32 * 0.1 * (24.0 / font_size) * 10.0) * 1.01
        };
        let line_count = editor.state.line_count();
        if self.width_font != font_size
            || self.width_lines == 0
            || self.width_document_generation != editor.document_generation()
        {
            self.width_full = true;
            self.finder.invalidate_matches();
        }
        if !self.width_full {
            for &(row, _, post_edit_lines) in &dirty {
                let delta = post_edit_lines - self.width_lines;
                if delta > 0 {
                    if self.width_longest > row {
                        self.width_longest += delta;
                    } else if self.width_longest == row {
                        self.width_longest = -1;
                    }
                } else if self.width_longest >= row && self.width_longest < row - delta {
                    self.width_longest = -1;
                } else if self.width_longest >= row - delta {
                    self.width_longest += delta;
                }
                if self.width_longest >= post_edit_lines {
                    self.width_longest = -1;
                }
                self.width_lines = post_edit_lines;
            }
            if line_count != self.width_lines {
                self.width_full = true;
            }
        }
        if self.width_full {
            self.width_max = 0.0;
            self.width_longest = -1;
            for row in 0..line_count {
                let width = measure(row);
                if width > self.width_max {
                    self.width_max = width;
                    self.width_longest = row;
                }
            }
            self.width_full = false;
        } else {
            if let Some((lo, hi)) = dirty
                .iter()
                .map(|&(lo, hi, _)| (lo.min(hi), lo.max(hi)))
                .reduce(|(a, b), (c, d)| (a.min(c), b.max(d)))
            {
                let lo = lo.clamp(0, line_count - 1);
                let hi = hi.clamp(0, line_count - 1);
                let longest_in_dirty = self.width_longest >= lo && self.width_longest <= hi;
                let mut local_max = 0.0;
                let mut local_longest = -1;
                for row in lo.max(0)..=hi.min(line_count - 1) {
                    let width = measure(row);
                    if width > local_max {
                        local_max = width;
                        local_longest = row;
                    }
                }
                if local_max > self.width_max || (longest_in_dirty && local_max >= self.width_max) {
                    self.width_max = local_max;
                    self.width_longest = local_longest;
                } else if longest_in_dirty {
                    // Preserve upstream's safe overestimate when a long line
                    // shrinks; never rescan a large file for each keystroke.
                    self.width_longest = -1;
                }
            }
        }
        self.width_font = font_size;
        self.width_lines = line_count;
        self.width_document_generation = editor.document_generation();
        self.width_max + (font_size * 7.5).max(self.width_max * 0.15) + font_size * 10.0
    }

    pub fn run(
        &mut self,
        ui: &Ui,
        editor: &mut ViewContext<'_>,
        input: &mut EditorInput,
    ) -> Vec<HostAction> {
        let primary = ui.io().key_ctrl() || ui.io().key_super();
        if let Some(keep) = self.exclusive_overlay.get() {
            self.close_internal_overlays_except(keep);
        }
        let pane_focused = ui.is_window_focused_with_flags(FocusedFlags::CHILD_WINDOWS);
        if !self.external_overlay
            && pane_focused
            && primary
            && !ui.io().key_shift()
            && ui.is_key_pressed_with_repeat(Key::F, false)
        {
            self.line_jump.dismiss();
            self.finder.open(editor);
            editor.api().request_exclusive_overlay(Overlay::Find);
        }
        if !self.external_overlay
            && pane_focused
            && primary
            && self
                .line_jump_key
                .is_some_and(|key| ui.is_key_pressed_with_repeat(key, false))
        {
            self.finder.dismiss();
            if self.line_jump.active {
                self.line_jump.dismiss();
                editor.view_mut().request_focus = true;
            } else {
                self.line_jump.open();
                editor.api().request_exclusive_overlay(Overlay::LineJump);
            }
        }
        let overlay_style = super::util::editor_finder::EditorOverlayStyle {
            background_color: self.background_color,
            pane: Some((ui.window_pos(), ui.window_size())),
        };
        let jump = self.line_jump.draw_with_style(ui, editor, &overlay_style);
        let find = self.finder.draw_with_style(ui, editor, &overlay_style);
        input.suppress_next_enter |= jump.suppress_next_enter || find.suppress_next_enter;
        if jump.clear_input_keys || find.clear_input_keys {
            ui.with_bound_context(|| unsafe {
                dear_imgui_rs::sys::ImGuiIO_ClearInputKeys(dear_imgui_rs::sys::igGetIO_Nil());
            });
        }
        let overlay_active = jump.block_input || find.block_input || self.external_overlay;
        let _spacing = ui.push_style_var(StyleVar::ItemSpacing([0.0, 0.0]));
        let fs = ui.current_font_size();

        if let Some(error) = &self.error {
            ui.text_colored(
                crate::presentation::readable_color(ui, [1.0, 0.45, 0.4, 1.0]),
                error,
            );
            if ui.button("Dismiss error") {
                self.error = None;
            }
        }
        self.layout.pane_pos = ui.window_pos();
        self.layout.pane_size = ui.window_size();
        self.layout.size = ui.content_region_avail();
        self.layout.line_height = ui.text_line_height();
        self.layout.total_height = editor.state.line_count() as f32 * self.layout.line_height;
        self.layout.editor_top_margin = fs * 0.1;
        self.layout.text_left_margin = fs * 0.35;
        self.layout.rainbow_mode = self.rainbow_mode;
        self.layout.minimap_width =
            if self.minimap_enabled && self.layout.size[0] >= fs * MIN_PANE_FONT_MUL {
                fs * WIDTH_FONT_MUL
            } else {
                0.0
            };
        let origin = ui.cursor_screen_pos();
        self.layout.minimap_min = [
            origin[0] + self.layout.size[0] - self.layout.minimap_width,
            origin[1],
        ];
        self.layout.minimap_max = [
            origin[0] + self.layout.size[0],
            origin[1] + self.layout.size[1],
        ];
        editor.view_mut().cursor_blink_time += ui.io().delta_time();
        let gutter_width =
            GutterView::width(ui, &editor.state) + GutterView::diagnostic_column_width(ui, editor);
        let mut gutter_pos = ui.cursor_screen_pos();
        let mut gutter_draw = std::ptr::null_mut();
        let _gutter_border = ui.push_style_var(StyleVar::ChildBorderSize(0.0));
        let _gutter_padding = ui.push_style_var(StyleVar::WindowPadding([0.0; 2]));
        ui.child_window("LineNumbers")
            .size([gutter_width, self.layout.size[1]])
            .flags(WindowFlags::NO_SCROLLBAR | WindowFlags::NO_SCROLL_WITH_MOUSE)
            .build(ui, || {
                gutter_pos = ui.cursor_screen_pos();
                gutter_draw =
                    ui.with_bound_context(|| unsafe { dear_imgui_rs::sys::igGetWindowDrawList() });
            });
        drop(_gutter_padding);
        drop(_gutter_border);
        ui.same_line_with_spacing(0.0, 0.0);
        let content_width = self.content_width(ui, editor);
        let mut actions = Vec::new();
        let _padding = ui.push_style_var(StyleVar::WindowPadding([0.0, 0.0]));
        let _rounding = ui.push_style_var(StyleVar::ChildRounding(0.0));
        let _child_border = ui.push_style_var(StyleVar::ChildBorderSize(1.0));
        let _scrollbar = ui.push_style_var(StyleVar::ScrollbarSize(fs * 0.6));
        ui.with_bound_context(|| unsafe {
            dear_imgui_rs::sys::igSetNextWindowContentSize(
                [content_width, self.layout.total_height].into(),
            );
        });
        ui.child_window("##editor")
            .size([
                (self.layout.size[0] - gutter_width - self.layout.minimap_width).max(1.0),
                self.layout.size[1],
            ])
            .flags(WindowFlags::HORIZONTAL_SCROLLBAR | WindowFlags::NO_NAV_INPUTS)
            .build(ui, || {
                if editor.view.request_focus && !overlay_active {
                    ui.set_window_focus(None);
                    ui.set_keyboard_focus_here();
                    editor.view_mut().request_focus = false;
                    editor.view_mut().block_input = false;
                    self.was_focused = true;
                }
                let focused = ui.is_window_focused_with_flags(FocusedFlags::CHILD_WINDOWS)
                    || ui.is_window_focused();
                if !focused {
                    editor.view_mut().block_input = true;
                } else if !self.was_focused {
                    editor.view_mut().block_input = false;
                    ui.set_keyboard_focus_here();
                } else if !editor.view.block_input && ui.is_window_appearing() {
                    ui.set_keyboard_focus_here();
                }
                if overlay_active {
                    editor.view_mut().block_input = true;
                }
                self.was_focused = focused;
                editor.view_mut().scroll_position = [ui.scroll_x(), ui.scroll_y()];
                self.layout.text_pos = ui.cursor_screen_pos();
                self.layout.text_pos[0] += self.layout.text_left_margin;
                self.layout.text_pos[1] += self.layout.editor_top_margin;
                // A mouse click can target an unfocused sibling view. Only an
                // overlay blocks navigation; keyboard focus blocks typing.
                actions = input.process_with_navigation(ui, editor, &self.layout, !overlay_active);
                self.layout.total_height =
                    editor.state.line_count() as f32 * self.layout.line_height;
                let (state, view) = editor.state_and_view();
                self.minimap.interact(ui, state, view, &self.layout);
                self.update_scroll(ui, editor);
                self.update_hover_trigger(ui, editor, gutter_pos, gutter_width);
                TextView::draw_with_highlight(
                    ui,
                    &editor.state,
                    &editor.view,
                    &self.layout,
                    Some(&editor.highlight),
                );
                if let Some(diagnostics) = &editor.diagnostics {
                    TextView::draw_diagnostics(
                        ui,
                        &editor.state,
                        &self.layout,
                        diagnostics,
                        self.hover_trigger.info(),
                        &self.tooltip_arbiter,
                    );
                }
                CaretView::draw(ui, &editor.state, &editor.view, &self.layout);
                let mut cursor = ui.cursor_pos();
                cursor[1] += self.layout.total_height + self.layout.editor_top_margin;
                ui.set_cursor_pos(cursor);
                ui.dummy([0.0, 0.0]);
                editor.view_mut().scroll_position = [ui.scroll_x(), ui.scroll_y()];
            });
        drop(_scrollbar);
        drop(_child_border);
        drop(_rounding);
        drop(_padding);
        if self.layout.minimap_width > 0.5 {
            self.minimap.draw(
                ui,
                &editor.state,
                &editor.view,
                &editor.highlight,
                &self.layout,
            );
        }
        if !gutter_draw.is_null() {
            // The child owns this list until the end of this same Ui frame.
            // EndChild does not destroy it. No draw-list borrow is retained or
            // independently mutated while this frame-scoped wrapper is alive.
            // Drawing here uses the final editor scroll, in the child's own
            // viewport layer above its background rather than in its parent.
            let draw = unsafe { dear_imgui_rs::DrawListMut::from_raw_mut(ui, gutter_draw) };
            draw.with_clip_rect(
                [gutter_pos[0], gutter_pos[1] + self.layout.editor_top_margin],
                [
                    gutter_pos[0] + gutter_width,
                    gutter_pos[1] + self.layout.size[1],
                ],
                || {
                    GutterView::draw(ui, &draw, editor, &self.layout, gutter_pos, gutter_width);
                },
            );
        }
        let hover = self.hover_trigger.info();
        if hover.active
            && hover.zone == Zone::Gutter
            && let Some(diagnostics) = &editor.diagnostics
        {
            render_diagnostic_tooltip(
                ui,
                &diagnostics.for_line(&editor.state.path, hover.row),
                &self.tooltip_arbiter,
            );
        }
        self.tooltip_arbiter
            .render_retained_diagnostic(ui, overlay_active);
        drop(_spacing);
        actions
    }

    pub fn hover_info(&self) -> Info {
        self.hover_trigger.info()
    }
    pub fn hover_dismissed(&self) -> bool {
        self.frame_dismissed
    }

    fn update_hover_trigger(
        &mut self,
        ui: &Ui,
        editor: &Editor,
        gutter_pos: [f32; 2],
        gutter_width: f32,
    ) {
        self.frame_dismissed = editor.view.block_input
            || ui.is_mouse_down(dear_imgui_rs::MouseButton::Left)
            || ui.with_bound_context(|| unsafe {
                (*dear_imgui_rs::sys::igGetIO_Nil())
                    .KeysData
                    .iter()
                    .any(|key| key.Down && key.DownDuration == 0.0)
            });
        let scroll = editor.view.scroll_position;
        self.frame_dismissed |= scroll != self.last_scroll;
        self.last_scroll = scroll;
        let mouse = ui.io().mouse_pos();
        let moved = ui.is_mouse_pos_valid() && mouse != self.last_mouse_pos;
        self.last_mouse_pos = mouse;
        let target = if ui.is_window_hovered() {
            self.hover_hit_test(ui, editor, gutter_pos, gutter_width)
        } else {
            Target::default()
        };
        let delay = ui
            .with_bound_context(|| unsafe { (*dear_imgui_rs::sys::igGetStyle()).HoverDelayNormal });
        self.hover_trigger.update_at(
            ui.time(),
            moved,
            self.frame_dismissed,
            target,
            f64::from(delay),
        );
    }

    fn hover_hit_test(
        &self,
        ui: &Ui,
        editor: &Editor,
        gutter_pos: [f32; 2],
        gutter_width: f32,
    ) -> Target {
        let layout = &self.layout;
        if layout.line_height <= 0.0 || !ui.is_mouse_pos_valid() {
            return Target::default();
        }
        let mouse = ui.io().mouse_pos();
        let gutter_y = gutter_pos[1] + layout.editor_top_margin;
        let bottom = layout.pane_pos[1] + layout.pane_size[1];
        if mouse[0] >= gutter_pos[0]
            && mouse[0] < gutter_pos[0] + gutter_width
            && mouse[1] >= gutter_y
            && mouse[1] < bottom
        {
            let row = ((mouse[1] - gutter_y + editor.view.scroll_position[1]) / layout.line_height)
                as i32;
            return if (0..editor.state.line_count()).contains(&row) {
                Target {
                    zone: Zone::Gutter,
                    row,
                    column: 0,
                }
            } else {
                Target::default()
            };
        }
        let minimap = layout.minimap_width > 0.5;
        if minimap
            && mouse[0] >= layout.minimap_min[0]
            && mouse[0] <= layout.minimap_max[0]
            && mouse[1] >= layout.minimap_min[1]
            && mouse[1] <= layout.minimap_max[1]
        {
            return Target::default();
        }
        let right = if minimap {
            layout.minimap_min[0]
        } else {
            layout.pane_pos[0] + layout.pane_size[0]
        };
        if mouse[0] < layout.text_pos[0]
            || mouse[0] >= right
            || mouse[1] < layout.text_pos[1]
            || mouse[1] >= bottom
        {
            return Target::default();
        }
        let row = ((mouse[1] - layout.text_pos[1]) / layout.line_height) as i32;
        if !(0..editor.state.line_count()).contains(&row) {
            return Target::default();
        }
        let line = editor.state.line(row);
        let column = bed_core::util::utf8::snap_to_utf8_char_boundary(
            &line,
            column_at_x(ui, &line, mouse[0] - layout.text_pos[0]),
        );
        Target {
            zone: Zone::Text,
            row,
            column,
        }
    }

    fn update_scroll(&mut self, ui: &Ui, editor: &mut ViewContext<'_>) {
        let (state, view) = editor.state_and_view();
        if let Some((row, column)) = view.pending_cursor_center.take() {
            view.primary_mut().head_row = row.clamp(0, state.line_count() - 1);
            let row = view.primary().head_row;
            view.primary_mut().head_column = column.clamp(0, state.line_length(row));
            view.primary_mut().collapse_to_head();
            view.collapse_to_primary();
            view.sync_primary_mirrors();
            view.center_cursor_vertical = true;
            view.ensure_cursor_visible.horizontal = true;
        }
        let baseline = [ui.scroll_x(), ui.scroll_y()];
        let mut next = baseline;
        let max_x = ui.scroll_max_x();
        let max_y = if ui.scroll_max_y() < 1.0 && self.layout.total_height > ui.window_height() {
            self.layout.total_height + self.layout.editor_top_margin - ui.window_height()
        } else {
            ui.scroll_max_y()
        };
        let clamp = |p: [f32; 2]| {
            [
                p[0].clamp(0.0, max_x.max(0.0)),
                p[1].clamp(0.0, max_y.max(0.0)),
            ]
        };
        if self.smooth_scroll.as_ref().is_some_and(|scroll| {
            let expected = clamp(scroll.last_requested);
            (0..2).any(|axis| (baseline[axis] - expected[axis]).abs() > 1.5)
        }) {
            // Native/host scroll changes interrupt navigation. ImGui rounds
            // requested positions to pixels, so tolerate that small difference.
            self.smooth_scroll = None;
        }
        // Wheel input and scrollbar drags use ImGui's ordinary panel behavior.
        // Manual scrolling takes precedence over pending caret reveal requests.
        let manual_scroll = ui.with_bound_context(|| unsafe {
            let native = &*sys::igGetCurrentContext();
            let active = native.ActiveId;
            let window = sys::igGetCurrentWindowRead();
            (active != 0
                && (active == sys::igGetWindowScrollbarID(window, sys::ImGuiAxis_X)
                    || active == sys::igGetWindowScrollbarID(window, sys::ImGuiAxis_Y)))
                || (native.WheelingWindow == window
                    && native.WheelingWindowScrolledFrame == native.FrameCount)
        });
        if manual_scroll {
            self.smooth_scroll = None;
            view.center_cursor_vertical = false;
            view.ensure_cursor_visible.horizontal = false;
            view.ensure_cursor_visible.vertical = false;
        }
        let reveal_horizontal = |mut target: [f32; 2]| {
            let x = line_column_x(ui, &state.line(view.row), view.column, 0.0);
            let margin = ui.current_font_size() * 2.0;
            let viewport = ui.window_width() - ui.current_font_size() * 0.6;
            if x < target[0] + margin {
                target[0] = x - margin;
            } else if x + ui.current_font_size() > target[0] + viewport - margin {
                target[0] = x + ui.current_font_size() - viewport + margin;
            }
            target
        };
        let mut navigation_started = false;
        if let Some(requested) = view.requested_scroll.take() {
            self.smooth_scroll = None;
            next = clamp(requested);
            view.center_cursor_vertical = false;
            view.ensure_cursor_visible.horizontal = false;
            view.ensure_cursor_visible.vertical = false;
        } else if view.center_cursor_vertical {
            let mut target = reveal_horizontal(next);
            target[1] = view.row as f32 * self.layout.line_height
                - (ui.window_height() - self.layout.line_height) * 0.5;
            target = clamp(target);
            view.center_cursor_vertical = false;
            // Center and horizontal reveal are one navigation request. Leaving
            // Ensure flags pending would interrupt centering on the next frame.
            view.ensure_cursor_visible.horizontal = false;
            view.ensure_cursor_visible.vertical = false;
            if self.navigation_animations && target != next {
                if let Some(scroll) = &mut self.smooth_scroll {
                    scroll.target = target;
                } else {
                    navigation_started = true;
                    self.smooth_scroll = Some(SmoothScroll::new(next, target));
                }
            } else {
                next = target;
                self.smooth_scroll = None;
            }
        } else if view.ensure_cursor_visible.horizontal || view.ensure_cursor_visible.vertical {
            self.smooth_scroll = None;
            // Editing, caret movement and document clicks resume ordinary
            // cursor-follow scrolling from the currently displayed viewport.
            let mut target = next;
            if view.ensure_cursor_visible.horizontal {
                target = reveal_horizontal(target);
            }
            if view.ensure_cursor_visible.vertical {
                let y = view.row as f32 * self.layout.line_height;
                if y < target[1] + self.layout.line_height {
                    target[1] = y - self.layout.line_height;
                } else if y + self.layout.line_height
                    > target[1] + ui.window_height() - self.layout.line_height
                {
                    target[1] = y + self.layout.line_height * 2.0 - ui.window_height();
                }
            }
            view.ensure_cursor_visible.horizontal = false;
            view.ensure_cursor_visible.vertical = false;
            next = clamp(target);
        }
        if let Some(scroll) = &mut self.smooth_scroll {
            scroll.target = clamp(scroll.target);
            if !self.navigation_animations {
                scroll.position = scroll.target;
                scroll.velocity = [0.0; 2];
            } else if !navigation_started {
                // A slow file open happened before this request. Begin its
                // animation on the following frame rather than counting that delay.
                scroll.advance(ui.io().delta_time());
            }
            let position = clamp(scroll.position);
            for axis in 0..2 {
                if position[axis] != scroll.position[axis] {
                    scroll.velocity[axis] = 0.0;
                }
            }
            scroll.position = position;
            let settled = scroll.settled();
            next = if settled {
                scroll.target
            } else {
                scroll.position
            };
            scroll.last_requested = next;
            if settled {
                self.smooth_scroll = None;
            }
        }
        if next != baseline {
            ui.set_scroll_x(next[0]);
            ui.set_scroll_y(next[1]);
        }
        view.scroll_position = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dear_imgui_rs::{Context, FramePrepareOptions};

    #[test]
    fn opaque_gutter_children_own_line_glyphs_after_their_background() {
        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = Context::create();
        context
            .set_ini_filename(None::<std::path::PathBuf>)
            .unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        let mut editors = [Editor::new(), Editor::new()];
        let mut frames = editors.each_mut().map(|editor| {
            editor.set_content(b"one\ntwo\nthree\nfour\nfive");
            let mut frame = EditorFrame::new(&mut editor.view_context());
            frame.rainbow_mode = false;
            frame
        });
        let mut inputs = [EditorInput::default(), EditorInput::default()];
        for _ in 0..2 {
            context.prepare_frame(FramePrepareOptions::new([800.0, 500.0], 1.0 / 60.0));
            let ui = context.frame();
            let _background = ui.push_style_color(StyleColor::ChildBg, [0.2, 0.1, 0.3, 1.0]);
            for index in 0..2 {
                ui.window(format!("Opaque gutter {index}"))
                    .position([index as f32 * 390.0, 0.0], Condition::Always)
                    .size([380.0, 450.0], Condition::Always)
                    .build(|| {
                        frames[index].run(
                            ui,
                            &mut editors[index].view_context(),
                            &mut inputs[index],
                        );
                    });
            }
            let muted = crate::presentation::muted_text_color(ui);
            let current = crate::presentation::readable_color(ui, ui.style_color(StyleColor::Text));
            let gutters = ui.with_bound_context(|| unsafe {
                let muted = sys::igColorConvertFloat4ToU32(muted.into());
                let current = sys::igColorConvertFloat4ToU32(current.into());
                let native = &*dear_imgui_rs::sys::igGetCurrentContext();
                let mut count = 0;
                for index in 0..native.Windows.Size {
                    let window = &**native.Windows.Data.add(index as usize);
                    let name = std::ffi::CStr::from_ptr(window.Name).to_string_lossy();
                    if !name.contains("Opaque gutter") || !name.contains("LineNumbers") {
                        continue;
                    }
                    count += 1;
                    let draw = &*window.DrawList;
                    let vertices = std::slice::from_raw_parts(
                        draw.VtxBuffer.Data,
                        draw.VtxBuffer.Size as usize,
                    );
                    let inactive: Vec<_> = vertices
                        .iter()
                        .enumerate()
                        .filter(|(_, v)| v.col == muted)
                        .collect();
                    assert_eq!(inactive.len(), 16, "four inactive line numbers: {name}");
                    assert!(inactive[0].0 >= 4, "glyphs follow the opaque background");
                    assert!(vertices.iter().any(|v| v.col == current));
                    let leading = inactive
                        .iter()
                        .map(|(_, v)| v.pos.x)
                        .fold(f32::INFINITY, f32::min)
                        - window.Pos.x;
                    assert!(
                        (0.0..6.0).contains(&leading),
                        "compact one-digit gutter in {name}: {leading}px"
                    );
                    assert!(inactive.iter().all(|(_, v)| {
                        v.pos.x >= window.Pos.x && v.pos.x < window.Pos.x + window.Size.x
                    }));
                }
                count
            });
            assert_eq!(gutters, 2);
            drop(_background);
            drop(context.render_legacy());
        }
    }

    #[test]
    fn width_cache_tracks_each_action_before_render_and_uses_raw_font_measurement() {
        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
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
        editor.set_content(b"a\nb\nthis is the longest line\nend");
        let mut frame = EditorFrame::new(&mut editor.view_context());
        context.prepare_frame(FramePrepareOptions::new([640.0, 480.0], 1.0 / 60.0));
        let ui = context.frame();
        ui.window("Width fixture")
            .size([640.0, 480.0], Condition::Always)
            .build(|| {
                frame.content_width(ui, &editor);
                assert_eq!(frame.width_longest, 2);
                editor.commands().set_cursor(
                    0,
                    0,
                    false,
                    bed_core::editor_commands::CursorReveal::Ensure,
                );
                editor.commands().insert_newline();
                editor.commands().set_cursor(
                    4,
                    0,
                    false,
                    bed_core::editor_commands::CursorReveal::Ensure,
                );
                editor.commands().insert_newline();
                frame.content_width(ui, &editor);
                // The second insertion is below the longest row. Applying the
                // combined delta at the first insertion would incorrectly yield 4.
                assert_eq!(frame.width_longest, 3);
                assert_eq!(frame.width_lines, 6);

                editor.set_content(b"\tX");
                let fs = ui.current_font_size();
                let measured = glyph_advance_bytes(ui, b"\tX");
                let expected = (measured + 2.0 * 0.1 * (24.0 / fs) * 10.0) * 1.01;
                frame.content_width(ui, &editor);
                assert!((frame.width_max - expected).abs() < 0.001);
                editor.set_content(b"short");
                frame.content_width(ui, &editor);
                assert_eq!(frame.width_longest, 0);
                assert!(frame.width_max > expected);
            });
        drop(context.render_legacy());
    }

    #[test]
    fn custom_frame_draws_and_routes_unicode_keys_and_scroll_intent() {
        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
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
        editor.set_content(&(0..100).map(|_| "line\n").collect::<String>().into_bytes());
        editor.view.request_focus = true;
        let mut frame = EditorFrame::new(&mut editor.view_context());
        let mut input = EditorInput::default();
        let render = |context: &mut Context,
                      frame: &mut EditorFrame,
                      editor: &mut Editor,
                      input: &mut EditorInput| {
            context.prepare_frame(FramePrepareOptions::new([640.0, 480.0], 1.0 / 60.0));
            let ui = context.frame();
            ui.window("Custom document host")
                .size([640.0, 480.0], Condition::Always)
                .build(|| {
                    assert!(frame.run(ui, &mut editor.view_context(), input).is_empty());
                });
            let draw = context.render_legacy();
            assert!(draw.draw_data().total_vtx_count() > 0);
        };
        render(&mut context, &mut frame, &mut editor, &mut input);
        context.io_mut().add_input_characters_utf8("hé🙂");
        render(&mut context, &mut frame, &mut editor, &mut input);
        assert_eq!(editor.state.line(0), "hé🙂line".as_bytes());
        assert_eq!(editor.view.column, 7);
        context.io_mut().add_key_event(Key::LeftArrow, true);
        render(&mut context, &mut frame, &mut editor, &mut input);
        assert_eq!(editor.view.column, 3);
        context.io_mut().add_key_event(Key::LeftArrow, false);
        editor.view.request_scroll(0.0, 200.0);
        render(&mut context, &mut frame, &mut editor, &mut input);
        assert!(editor.view.requested_scroll.is_none());
        editor.view.request_cursor_center(50, 2);
        render(&mut context, &mut frame, &mut editor, &mut input);
        assert_eq!((editor.view.row, editor.view.column), (50, 2));
        assert!(editor.view.pending_cursor_center.is_none());
        assert!(!editor.view.center_cursor_vertical);
    }
}

#[cfg(test)]
#[path = "navigation_scroll_tests.rs"]
mod navigation_scroll_tests;
