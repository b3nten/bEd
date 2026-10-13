//! Translated from ned editor/views/hover_tooltip.{h,cpp}; see LICENSE and NOTICE.
use crate::views::diagnostic_style::{severity_color, severity_label};
use bed_document_session::editor::Editor;
use bed_editing::diagnostic::DiagnosticItem;
use bed_highlight::capture_map::THEME_SLOTS;
use dear_imgui_rs::{
    ColorStackToken, Condition, StyleColor, StyleStackToken, StyleVar, Ui, WindowFlags, sys,
};
use std::cell::{Cell, RefCell};

pub type HoverRect = ([f32; 2], [f32; 2]);
const HOVER_FONT_SCALE: f32 = 0.9;
const HOVER_STICKY_PADDING: f32 = 12.0;

pub struct HoverPopupStyle<'ui> {
    vars: Vec<StyleStackToken<'ui>>,
    _background: ColorStackToken<'ui>,
}

impl Drop for HoverPopupStyle<'_> {
    fn drop(&mut self) {
        while let Some(token) = self.vars.pop() {
            drop(token);
        }
    }
}

/// Share the input-capable tooltip appearance while preserving host styles.
pub fn hover_popup_style(ui: &Ui) -> HoverPopupStyle<'_> {
    let fs = ui.current_font_size();
    HoverPopupStyle {
        vars: [
            StyleVar::WindowPadding([(fs * 0.6).floor(), (fs * 0.4).floor()]),
            StyleVar::ItemSpacing([fs * 0.35, fs * 0.2]),
            StyleVar::WindowMinSize([1.0; 2]),
            StyleVar::WindowRounding(bed_ui::util::popup_style::POPUP_ROUNDING),
            StyleVar::WindowBorderSize(bed_ui::util::popup_style::POPUP_BORDER_SIZE),
            StyleVar::ScrollbarSize((fs * 0.65).floor().max(1.0)),
        ]
        .into_iter()
        .map(|var| ui.push_style_var(var))
        .collect(),
        _background: ui.push_style_color(StyleColor::WindowBg, ui.style_color(StyleColor::PopupBg)),
    }
}

pub fn hover_rect_contains(rect: HoverRect, mouse: [f32; 2]) -> bool {
    let (min, max) = rect;
    mouse[0].is_finite()
        && mouse[1].is_finite()
        && mouse[0] >= min[0] - HOVER_STICKY_PADDING
        && mouse[0] <= max[0] + HOVER_STICKY_PADDING
        && mouse[1] >= min[1] - HOVER_STICKY_PADDING
        && mouse[1] <= max[1] + HOVER_STICKY_PADDING
}

pub fn hover_key_pressed(ui: &Ui) -> bool {
    ui.with_bound_context(|| unsafe {
        (*sys::igGetIO_Nil())
            .KeysData
            .iter()
            // ImGui also mirrors mouse buttons and wheel events into KeysData.
            // Those events must remain available to the popup's scrollbars.
            .take((sys::ImGuiKey_MouseLeft - sys::ImGuiKey_NamedKey_BEGIN) as usize)
            .any(|key| key.Down && key.DownDuration == 0.0)
    })
}

/// Keep a popup inside its viewport and outside the cursor's visual line.
/// If neither side fits, shorten it to the larger side so it can scroll.
pub fn popup_placement(
    anchor: [f32; 2],
    mut size: [f32; 2],
    viewport: HoverRect,
    caret_line: Option<[f32; 2]>,
) -> ([f32; 2], [f32; 2]) {
    let min = viewport.0.map(f32::ceil);
    let max = viewport.1.map(f32::floor);
    for axis in 0..2 {
        size[axis] = size[axis].min((max[axis] - min[axis]).max(0.0)).floor();
    }
    let mut position = std::array::from_fn(|axis| {
        anchor[axis].clamp(min[axis], (max[axis] - size[axis]).max(min[axis]))
    });
    if let Some([top, bottom]) = caret_line.map(|[top, bottom]| [top.floor(), bottom.ceil()])
        && position[1] < bottom
        && position[1] + size[1] > top
    {
        let above = (top.min(max[1]) - min[1]).max(0.0);
        let below = (max[1] - bottom.max(min[1])).max(0.0);
        let use_below = if below >= size[1] {
            anchor[1] >= top || above < size[1]
        } else if above >= size[1] {
            false
        } else {
            below >= above
        };
        size[1] = size[1].min(if use_below { below } else { above });
        position[1] = if use_below {
            bottom.max(min[1])
        } else {
            top.min(max[1]) - size[1]
        };
    }
    (position, size)
}

/// An input-capable tooltip: native tooltips disable mouse input, which also
/// prevents their scrollbars and wheel scrolling from working.
pub fn render_hover_popup(
    ui: &Ui,
    name: &str,
    anchor: [f32; 2],
    previous_rect: Option<HoverRect>,
    caret_line: Option<[f32; 2]>,
    contents: impl FnOnce(),
) -> Option<HoverRect> {
    let base_size = ui.with_bound_context(|| unsafe {
        (*sys::igGetCurrentContext()).FontSizeBase * HOVER_FONT_SCALE
    });
    let _font = ui.push_font_with_size(None, base_size);
    let fs = ui.current_font_size();
    let viewport = ui.window_viewport();
    let viewport_pos = viewport.work_pos();
    let viewport_size = viewport.work_size();
    let margin = fs * 0.4;
    let maximum = [
        (fs * 30.0)
            .min(viewport_size[0] - margin * 2.0)
            .floor()
            .max(1.0),
        (fs * 18.0)
            .min(viewport_size[1] - margin * 2.0)
            .floor()
            .max(1.0),
    ];
    let (position, maximum) = popup_placement(
        anchor,
        maximum,
        (
            viewport_pos.map(|value| value + margin),
            std::array::from_fn(|axis| viewport_pos[axis] + viewport_size[axis] - margin),
        ),
        caret_line,
    );
    if maximum[1] < 1.0 {
        return None;
    }
    let _style = hover_popup_style(ui);
    ui.set_next_window_viewport(viewport.id());
    // Anchor above-line tooltips by their bottom edge so auto-fitting short
    // contents keeps them close to the cursor rather than leaving a large gap.
    let above_line = caret_line.is_some_and(|line| position[1] + maximum[1] <= line[0]);
    ui.with_bound_context(|| unsafe {
        sys::igSetNextWindowPos(
            sys::ImVec2 {
                x: position[0],
                y: position[1] + if above_line { maximum[1] } else { 0.0 },
            },
            Condition::Always as i32,
            sys::ImVec2 {
                x: 0.0,
                y: if above_line { 1.0 } else { 0.0 },
            },
        );
    });
    let mut window = ui
        .window(name)
        // Start at the bound until native layout knows the content height.
        // Subsequent zero heights auto-fit without positioning from a tiny
        // initial measurement that could push a growing popup offscreen.
        .size(
            [
                maximum[0],
                if previous_rect.is_none() {
                    maximum[1]
                } else {
                    0.0
                },
            ],
            Condition::Always,
        )
        .size_constraints([1.0; 2], maximum)
        .flags(
            WindowFlags::NO_TITLE_BAR
                | WindowFlags::NO_RESIZE
                | WindowFlags::NO_MOVE
                | WindowFlags::NO_COLLAPSE
                | WindowFlags::NO_SAVED_SETTINGS
                | WindowFlags::NO_FOCUS_ON_APPEARING
                | WindowFlags::NO_NAV
                | WindowFlags::NO_DOCKING
                | WindowFlags::ALWAYS_AUTO_RESIZE
                | WindowFlags::HORIZONTAL_SCROLLBAR,
        );
    if previous_rect.is_none() {
        window = window.scroll([0.0; 2]);
    }
    window.build(|| {
        ui.with_bound_context(|| unsafe {
            sys::igBringWindowToDisplayFront(sys::igGetCurrentWindow());
        });
        // Keep ordinary diagnostic text within the same compact viewport as
        // the styled prose and code renderers.
        let _wrap = ui.push_text_wrap_pos(ui.cursor_pos()[0] + ui.content_region_avail()[0]);
        contents();
        let min = ui.window_pos();
        let size = ui.window_size();
        (min, [min[0] + size[0], min[1] + size[1]])
    })
}

struct DiagnosticHover {
    items: Vec<DiagnosticItem>,
    anchor: [f32; 2],
    rect: Option<HoverRect>,
    last_frame: usize,
}

pub struct TooltipArbiter {
    claimed_frame: Cell<Option<usize>>,
    diagnostic: RefCell<Option<DiagnosticHover>>,
    caret_line: Cell<Option<[f32; 2]>>,
}

impl Default for TooltipArbiter {
    fn default() -> Self {
        Self {
            claimed_frame: Cell::new(None),
            diagnostic: RefCell::new(None),
            caret_line: Cell::new(None),
        }
    }
}

impl TooltipArbiter {
    pub fn set_caret_line(&self, top: f32, height: f32) {
        self.caret_line.set(Some([top, top + height]));
    }

    pub fn claim(&self, ui: &Ui) -> bool {
        let frame = ui.frame_count();
        if self.claimed_frame.get() == Some(frame) {
            return false;
        }
        self.claimed_frame.set(Some(frame));
        true
    }

    /// Keep a diagnostic visible while the pointer moves from its source into
    /// the popup, including while dragging its scrollbar.
    pub fn render_retained_diagnostic(&self, ui: &Ui, overlay_active: bool) {
        let mut retained = self.diagnostic.borrow_mut();
        let Some(hover) = retained.as_mut() else {
            return;
        };
        if overlay_active {
            *retained = None;
            return;
        }
        if hover.last_frame == ui.frame_count() {
            return;
        }
        let over_popup = hover
            .rect
            .is_some_and(|rect| hover_rect_contains(rect, ui.io().mouse_pos()));
        if hover.last_frame + 1 != ui.frame_count() || !over_popup || hover_key_pressed(ui) {
            *retained = None;
            return;
        }
        if self.claim(ui) {
            draw_diagnostic_hover(ui, hover, self);
        }
    }
}

pub fn render_hover_plaintext(ui: &Ui, text: &str, editor: &Editor) {
    bed_ui::documentation::render_plaintext(ui, text, &documentation_colors(editor));
}

pub fn render_hover_markdown(ui: &Ui, markdown: &str, editor: &Editor, fallback_language: &str) {
    bed_ui::documentation::render_markdown(
        ui,
        markdown,
        &documentation_colors(editor),
        fallback_language,
    );
}

pub fn documentation_colors(editor: &Editor) -> bed_highlight::tree_sitter::ThemeColors {
    bed_highlight::tree_sitter::ThemeColors {
        slots: THEME_SLOTS.map(|slot| editor.highlight.color_for_slot(slot)),
    }
}

pub fn render_diagnostic_tooltip(ui: &Ui, items: &[DiagnosticItem], arbiter: &TooltipArbiter) {
    if items.is_empty() || !arbiter.claim(ui) {
        return;
    }
    let mut retained = arbiter.diagnostic.borrow_mut();
    if retained
        .as_ref()
        .is_none_or(|hover| hover.items != items || hover.last_frame + 1 < ui.frame_count())
    {
        let mouse = ui.io().mouse_pos();
        *retained = Some(DiagnosticHover {
            items: items.to_vec(),
            anchor: [mouse[0] + 8.0, mouse[1] + 12.0],
            rect: None,
            last_frame: ui.frame_count(),
        });
    }
    draw_diagnostic_hover(ui, retained.as_mut().unwrap(), arbiter);
}

fn draw_diagnostic_hover(ui: &Ui, hover: &mut DiagnosticHover, arbiter: &TooltipArbiter) {
    // Each editor owns an arbiter, so a popup in a split remains attached to
    // the editor that supplied its diagnostics.
    let name = format!("##bed_diagnostic_hover_{:p}", arbiter);
    hover.rect = render_hover_popup(
        ui,
        &name,
        hover.anchor,
        hover.rect,
        arbiter.caret_line.get(),
        || {
            for (index, item) in hover.items.iter().enumerate() {
                if index > 0 {
                    ui.separator();
                }
                ui.text_colored(
                    bed_ui::presentation::readable_color(ui, severity_color(item.severity)),
                    severity_label(item.severity),
                );
                if !item.source.is_empty() {
                    ui.same_line();
                    ui.text_disabled(&item.source);
                }
                ui.text(&item.message);
            }
        },
    );
    hover.last_frame = ui.frame_count();
}

#[cfg(test)]
mod tests {
    use super::*;
    use dear_imgui_rs::{Context, FramePrepareOptions, Key};

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

    fn frame(context: &mut Context, draw: impl FnOnce(&Ui)) {
        context.prepare_frame(FramePrepareOptions::new([640.0, 480.0], 1.0 / 60.0));
        let ui = context.frame();
        ui.window("hover fixture")
            .position([0.0; 2], Condition::Always)
            .size([640.0, 480.0], Condition::Always)
            .flags(WindowFlags::NO_TITLE_BAR | WindowFlags::NO_SAVED_SETTINGS)
            .build(|| draw(ui));
        drop(context.render_legacy());
    }

    #[test]
    fn native_hover_prose_stays_inside_a_narrow_viewport() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = context();
        let editor = Editor::new();
        let rect = Cell::new(None);
        for _ in 0..4 {
            context.prepare_frame(FramePrepareOptions::new([180.0, 140.0], 1.0 / 60.0));
            let ui = context.frame();
            ui.window("narrow hover fixture")
                .position([0.0; 2], Condition::Always)
                .size([180.0, 140.0], Condition::Always)
                .build(|| {
                    rect.set(render_hover_popup(
                        ui,
                        "##narrow prose popup",
                        [175.0, 135.0],
                        rect.get(),
                        None,
                        || {
                            render_hover_markdown(
                                ui,
                                &"Words with **inline styling** and `code` keep wrapping. "
                                    .repeat(20),
                                &editor,
                                "",
                            );
                            assert_eq!(ui.scroll_max_x(), 0.0);
                        },
                    ));
                    let (minimum, maximum) = rect.get().unwrap();
                    assert!(minimum[0] >= 0.0 && minimum[1] >= 0.0);
                    assert!(
                        maximum[0] <= 180.0 && maximum[1] <= 140.0,
                        "{minimum:?} {maximum:?}"
                    );
                });
            drop(context.render_legacy());
        }
    }

    #[test]
    fn native_rust_analyzer_hover_wraps_signatures_and_documentation() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = context();
        let editor = Editor::new();
        // Captured from rust-analyzer with hover.contentFormat advertising
        // Markdown. Its signature is a fenced code block, not ordinary prose.
        let markdown = "\n```rust\nbed_hover_payload\n```\n\n```rust\npub fn documented_function_with_a_long_signature(name: &str, result: Result<Option<Vec<String>>, std::io::Error>, count: usize) -> usize\n```\n\n---\n\nA documented function with a long paragraph explaining how it handles a result, a collection of strings, and a caller supplied name while preserving all of the input data for later inspection.";
        let rect = Cell::new(None);
        for _ in 0..4 {
            context.prepare_frame(FramePrepareOptions::new([240.0, 180.0], 1.0 / 60.0));
            let ui = context.frame();
            ui.window("rust-analyzer hover fixture")
                .position([0.0; 2], Condition::Always)
                .size([240.0, 180.0], Condition::Always)
                .build(|| {
                    rect.set(render_hover_popup(
                        ui,
                        "##rust-analyzer hover popup",
                        [235.0, 175.0],
                        rect.get(),
                        None,
                        || {
                            render_hover_markdown(ui, markdown, &editor, "rust");
                            assert_eq!(ui.scroll_max_x(), 0.0);
                        },
                    ));
                    let (minimum, maximum) = rect.get().unwrap();
                    assert!(minimum[0] >= 0.0 && minimum[1] >= 0.0);
                    assert!(maximum[0] <= 240.0 && maximum[1] <= 180.0);
                });
            drop(context.render_legacy());
        }
    }

    #[test]
    fn native_hover_prose_code_and_bold_remain_readable_across_theme_switches() {
        use bed_ui::color::{blend, contrast_ratio};

        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = context();
        let editor = Editor::new();
        for (background, text) in [
            ([0.96, 0.94, 0.90, 1.0], [0.55, 0.55, 0.55, 1.0]),
            ([0.04, 0.05, 0.08, 1.0], [0.20, 0.20, 0.20, 1.0]),
        ] {
            context
                .style_mut()
                .set_color(StyleColor::PopupBg, background);
            context.style_mut().set_color(StyleColor::Text, text);
            let drew_text = Cell::new(false);
            for _ in 0..3 {
                frame(&mut context, |ui| {
                    render_hover_popup(
                        ui,
                        "##hover theme fixture",
                        [20.0, 20.0],
                        None,
                        None,
                        || {
                            let contrast = |color| {
                                contrast_ratio(blend(color, background, color[3]), background)
                            };
                            let start = ui.with_bound_context(|| unsafe {
                                (*sys::igGetWindowDrawList()).VtxBuffer.Size
                            });
                            render_hover_markdown(
                                ui,
                                "plain **bold** `inline`\n\n```\nsnippet\n```",
                                &editor,
                                "",
                            );
                            ui.with_bound_context(|| unsafe {
                                let vertices = &(*sys::igGetWindowDrawList()).VtxBuffer;
                                for index in start..vertices.Size {
                                    let packed = (*vertices.Data.add(index as usize)).col;
                                    let color = std::array::from_fn(|channel| {
                                        ((packed >> (channel * 8)) & 255) as f32 / 255.0
                                    });
                                    assert!(
                                        contrast(color) >= 4.45,
                                        "hover text {color:?} on {background:?}"
                                    );
                                    drew_text.set(true);
                                }
                            });
                        },
                    );
                });
            }
            assert!(drew_text.get());
        }
    }

    #[test]
    fn native_hover_bounds_and_wraps_long_code_with_vertical_scrolling() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = context();
        let editor = Editor::new();
        let markdown = format!("```\n{}\n```", ("code ".repeat(80) + "\n").repeat(80));
        let rect = Cell::new(None);
        let sample = Cell::new(([0.0; 2], [0.0; 2], 0.0, 0.0));
        let draw = |ui: &Ui| {
            let parent_font_size = ui.current_font_size();
            rect.set(render_hover_popup(
                ui,
                "##hover bounds",
                [600.0, 450.0],
                rect.get(),
                None,
                || {
                    render_hover_markdown(ui, &markdown, &editor, "");
                    sample.set((
                        [ui.scroll_x(), ui.scroll_y()],
                        [ui.scroll_max_x(), ui.scroll_max_y()],
                        ui.current_font_size(),
                        parent_font_size,
                    ));
                },
            ));
            assert_eq!(ui.current_font_size(), parent_font_size);
        };
        for _ in 0..3 {
            frame(&mut context, draw);
        }
        let (min, max) = rect.get().unwrap();
        assert!(
            max[0] - min[0] <= sample.get().2 * 30.0 + 1.0,
            "{min:?} {max:?} {sample:?}"
        );
        assert!(
            max[1] - min[1] <= sample.get().2 * 18.0 + 1.0,
            "{min:?} {max:?} {sample:?}"
        );
        assert!(min[0] >= 0.0 && min[1] >= 0.0 && max[0] <= 640.0 && max[1] <= 480.0);
        assert!(
            sample.get().1[0] == 0.0 && sample.get().1[1] > 100.0,
            "{sample:?}"
        );
        assert!(sample.get().2 < sample.get().3, "{sample:?}");

        context
            .io_mut()
            .add_mouse_pos_event([min[0] + 30.0, min[1] + 30.0]);
        // Native ImGui discovers the new hovered window on the next frame.
        frame(&mut context, draw);
        context.io_mut().add_mouse_wheel_event([0.0, -2.0]);
        frame(&mut context, draw);
        assert!(sample.get().0[1] > 0.0, "{sample:?}");
    }

    #[test]
    fn native_hover_keeps_cursor_line_clear_near_viewport_edges() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = context();
        for top in [0.0, 220.0, 220.4, 460.0] {
            let caret_line = [top, top + 20.0];
            let rect = Cell::new(None);
            for frame_index in 0..3 {
                frame(&mut context, |ui| {
                    rect.set(render_hover_popup(
                        ui,
                        "##hover cursor clearance",
                        [600.0, top + 5.0],
                        rect.get(),
                        Some(caret_line),
                        || ui.text("Long documentation.\n".repeat(80)),
                    ));
                    let (min, max) = rect.get().unwrap();
                    assert!(
                        min[1] >= caret_line[1] || max[1] <= caret_line[0],
                        "{min:?} {max:?} cover {caret_line:?}"
                    );
                    assert!(min[0] >= 0.0 && min[1] >= 0.0 && max[0] <= 640.0 && max[1] <= 480.0);
                    if frame_index > 0 {
                        assert!(
                            ui.with_bound_context(|| unsafe {
                                (*sys::igFindWindowByName(c"##hover cursor clearance".as_ptr()))
                                    .ScrollMax
                                    .y
                            }) > 0.0
                        );
                    }
                });
            }
        }
    }

    #[test]
    fn native_diagnostic_hover_survives_pointer_and_scroll_then_escape_dismisses() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = context();
        let arbiter = TooltipArbiter::default();
        let diagnostics = vec![DiagnosticItem {
            message: "A long diagnostic message with more details.\n".repeat(60),
            ..Default::default()
        }];
        context.io_mut().add_mouse_pos_event([100.0, 100.0]);
        for _ in 0..3 {
            frame(&mut context, |ui| {
                render_diagnostic_tooltip(ui, &diagnostics, &arbiter)
            });
        }
        let (min, _) = arbiter.diagnostic.borrow().as_ref().unwrap().rect.unwrap();
        context
            .io_mut()
            .add_mouse_pos_event([min[0] + 20.0, min[1] + 20.0]);
        frame(&mut context, |ui| {
            arbiter.render_retained_diagnostic(ui, false)
        });
        context.io_mut().add_mouse_wheel_event([0.0, -2.0]);
        frame(&mut context, |ui| {
            arbiter.render_retained_diagnostic(ui, false);
            let retained = arbiter.diagnostic.borrow();
            let hover = retained
                .as_ref()
                .expect("pointer and scrolling keep the popup open");
            assert_eq!(hover.last_frame, ui.frame_count());
            ui.with_bound_context(|| unsafe {
                let name = std::ffi::CString::new(format!("##bed_diagnostic_hover_{:p}", &arbiter))
                    .unwrap();
                let window = sys::igFindWindowByName(name.as_ptr());
                assert!(!window.is_null());
                assert!((*window).Scroll.y > 0.0);
            });
        });
        context.io_mut().add_key_event(Key::Escape, true);
        frame(&mut context, |ui| {
            arbiter.render_retained_diagnostic(ui, false)
        });
        assert!(arbiter.diagnostic.borrow().is_none());
    }
}
