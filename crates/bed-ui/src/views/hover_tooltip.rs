//! Translated from ned editor/views/hover_tooltip.{h,cpp}; see LICENSE and NOTICE.
use crate::views::{
    diagnostic_style::{severity_color, severity_label},
    hover_markdown::{parse_hover_markdown, split_hover_lines},
};
use bed_highlight::{
    capture_map::{THEME_SLOTS, ThemeSlot},
    tree_sitter::{LineColorSpans, TreeSitter},
};
use bed_lsp::diagnostics::diagnostics_store::DiagnosticItem;
use bed_session::editor::Editor;
use dear_imgui_rs::{Condition, StyleColor, StyleVar, Ui, WindowFlags, sys};
use std::cell::{Cell, RefCell};

pub type HoverRect = ([f32; 2], [f32; 2]);
const HOVER_FONT_SCALE: f32 = 0.9;
const HOVER_STICKY_PADDING: f32 = 12.0;

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

/// An input-capable tooltip: native tooltips disable mouse input, which also
/// prevents their scrollbars and wheel scrolling from working.
pub fn render_hover_popup(
    ui: &Ui,
    name: &str,
    anchor: [f32; 2],
    previous_rect: Option<HoverRect>,
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
        (fs * 30.0).min((viewport_size[0] - margin * 2.0).max(1.0)),
        (fs * 18.0).min((viewport_size[1] - margin * 2.0).max(1.0)),
    ];
    let size = previous_rect.map_or(maximum, |(min, max)| {
        [
            (max[0] - min[0]).min(maximum[0]),
            (max[1] - min[1]).min(maximum[1]),
        ]
    });
    let position = std::array::from_fn(|axis| {
        let minimum = viewport_pos[axis] + margin;
        let maximum = (viewport_pos[axis] + viewport_size[axis] - size[axis] - margin).max(minimum);
        anchor[axis].clamp(minimum, maximum)
    });
    let _padding = ui.push_style_var(StyleVar::WindowPadding([fs * 0.6, fs * 0.4]));
    let _minimum = ui.push_style_var(StyleVar::WindowMinSize([1.0; 2]));
    let _rounding = ui.push_style_var(StyleVar::WindowRounding(fs * 0.35));
    let _border = ui.push_style_var(StyleVar::WindowBorderSize(1.0));
    let _scrollbar = ui.push_style_var(StyleVar::ScrollbarSize(fs * 0.65));
    let _background =
        ui.push_style_color(StyleColor::WindowBg, ui.style_color(StyleColor::PopupBg));
    ui.set_next_window_viewport(viewport.id());
    let mut window = ui
        .window(name)
        .position(position, Condition::Always)
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
        // Wrap prose within the compact viewport; fenced code keeps its
        // original lines and can scroll horizontally.
        let _wrap = ui.push_text_wrap_pos(fs * 28.0);
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
}
impl Default for TooltipArbiter {
    fn default() -> Self {
        Self {
            claimed_frame: Cell::new(None),
            diagnostic: RefCell::new(None),
        }
    }
}
impl TooltipArbiter {
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

struct HoverColors {
    slots: [[f32; 4]; THEME_SLOTS.len()],
    bold: [f32; 4],
}

impl HoverColors {
    fn new(ui: &Ui, editor: &Editor) -> Self {
        use bed_core::util::color::{blend, relative_luminance};
        let mut slots = THEME_SLOTS.map(|slot| {
            crate::presentation::readable_color(ui, editor.highlight.color_for_slot(slot))
        });
        let text = crate::presentation::readable_color(ui, ui.style_color(StyleColor::Text));
        slots[ThemeSlot::Text as usize] = text;
        let background = ui.style_color(StyleColor::WindowBg);
        let stronger = if relative_luminance(text) > relative_luminance(background) {
            [1.0, 1.0, 1.0, text[3]]
        } else {
            [0.0, 0.0, 0.0, text[3]]
        };
        let bold = crate::presentation::readable_color(ui, blend(text, stronger, 0.85));
        Self { slots, bold }
    }

    fn color(&self, slot: ThemeSlot) -> [f32; 4] {
        self.slots[slot as usize]
    }
}

fn draw_code_line(ui: &Ui, line: &str, spans: &LineColorSpans, colors: &HoverColors) {
    let height = ui.text_line_height();
    if line.is_empty() {
        ui.dummy([1.0, height]);
        return;
    }
    let pos = ui.cursor_screen_pos();
    let mut x = pos[0];
    let mut span_index = 0;
    let mut offset = 0;
    while offset < line.len() {
        while span_index < spans.len() && spans[span_index].end <= offset as i32 {
            span_index += 1;
        }
        let (color, end) = match spans.get(span_index) {
            Some(span) if span.start <= offset as i32 => {
                (colors.color(span.slot), line.len().min(span.end as usize))
            }
            Some(span) => (
                colors.color(ThemeSlot::Text),
                line.len().min(span.start as usize),
            ),
            None => (colors.color(ThemeSlot::Text), line.len()),
        };
        let bytes = line.as_bytes();
        ui.with_bound_context(|| unsafe {
            // SAFETY: native AddText consumes this bounded UTF-8 run during the
            // active Ui frame; no pointers escape the call.
            sys::ImDrawList_AddText_Vec2(
                sys::igGetWindowDrawList(),
                [x, pos[1]].into(),
                sys::igColorConvertFloat4ToU32(color.into()),
                bytes.as_ptr().add(offset).cast(),
                bytes.as_ptr().add(end).cast(),
            );
        });
        x += ui.calc_text_size(&line[offset..end])[0];
        offset = end;
    }
    ui.dummy([(x - pos[0]).max(1.0), height]);
}

fn draw_prose_line(ui: &Ui, line: &str, colors: &HoverColors) {
    let text = colors.color(ThemeSlot::Text);
    let code = colors.color(ThemeSlot::String);
    let mut first = true;
    let mut emit = |piece: &str, color: [f32; 4]| {
        if piece.is_empty() {
            return;
        }
        if !first {
            ui.same_line_with_spacing(0.0, 0.0);
        }
        first = false;
        let _text = ui.push_style_color(StyleColor::Text, color);
        ui.text(piece);
    };
    let bytes = line.as_bytes();
    let mut offset = 0;
    while offset < bytes.len() {
        if bytes[offset] == b'`'
            && let Some(end) = line[offset + 1..].find('`').map(|n| n + offset + 1)
        {
            emit(&line[offset + 1..end], code);
            offset = end + 1;
            continue;
        }
        if line[offset..].starts_with("**")
            && let Some(end) = line[offset + 2..].find("**").map(|n| n + offset + 2)
        {
            emit(&line[offset + 2..end], colors.bold);
            offset = end + 2;
            continue;
        }
        if bytes[offset] == b'['
            && let Some(close) = line[offset + 1..].find(']').map(|n| n + offset + 1)
            && bytes.get(close + 1) == Some(&b'(')
            && let Some(end) = line[close + 2..].find(')').map(|n| n + close + 2)
        {
            emit(&line[offset + 1..close], text);
            offset = end + 1;
            continue;
        }
        let next = (offset + 1..bytes.len())
            .find(|&n| {
                bytes[n] == b'`'
                    || bytes[n] == b'['
                    || (bytes[n] == b'*' && bytes.get(n + 1) == Some(&b'*'))
            })
            .unwrap_or(bytes.len());
        emit(&line[offset..next], text);
        offset = next;
    }
    if first {
        ui.dummy([1.0, ui.text_line_height()]);
    }
}

pub fn render_hover_markdown(ui: &Ui, markdown: &str, editor: &Editor, fallback_language: &str) {
    let fs = ui.current_font_size();
    let _wrap = ui.push_text_wrap_pos(fs * 28.0);
    // Resolve the small theme palette once, independent of code/prose length.
    let palette = HoverColors::new(ui, editor);
    for (index, block) in parse_hover_markdown(markdown).iter().enumerate() {
        if index > 0 {
            ui.dummy([0.0, fs * 0.15]);
        }
        if block.text == "---" {
            ui.separator();
            continue;
        }
        let _spacing = ui.push_style_var(StyleVar::ItemSpacing([0.0, 0.0]));
        if block.code {
            let language = if block.language.is_empty() {
                fallback_language
            } else {
                &block.language
            };
            let colors = if language.is_empty() {
                Vec::new()
            } else {
                TreeSitter::highlight_snippet(language, block.text.as_bytes())
            };
            for (row, line) in split_hover_lines(&block.text).iter().enumerate() {
                draw_code_line(ui, line, colors.get(row).unwrap_or(&Vec::new()), &palette);
            }
        } else {
            for line in split_hover_lines(&block.text) {
                if line.is_empty() {
                    ui.dummy([1.0, ui.text_line_height() * 0.35]);
                } else {
                    draw_prose_line(ui, &line, &palette);
                }
            }
        }
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
    hover.rect = render_hover_popup(ui, &name, hover.anchor, hover.rect, || {
        let fs = ui.current_font_size();
        let _spacing = ui.push_style_var(StyleVar::ItemSpacing([fs * 0.35, fs * 0.2]));
        for (index, item) in hover.items.iter().enumerate() {
            if index > 0 {
                ui.separator();
            }
            ui.text_colored(
                crate::presentation::readable_color(ui, severity_color(item.severity)),
                severity_label(item.severity),
            );
            if !item.source.is_empty() {
                ui.same_line();
                ui.text_disabled(&item.source);
            }
            ui.text(&item.message);
        }
    });
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
    fn native_hover_prose_code_and_bold_remain_readable_across_theme_switches() {
        use bed_core::util::color::{blend, contrast_ratio};

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
                    render_hover_popup(ui, "##hover theme fixture", [20.0, 20.0], None, || {
                        let palette = HoverColors::new(ui, &editor);
                        let contrast =
                            |color| contrast_ratio(blend(color, background, color[3]), background);
                        assert!(
                            contrast(palette.bold)
                                >= contrast(palette.color(ThemeSlot::Text)) - 0.001
                        );
                        for color in palette.slots {
                            assert!(contrast(color) >= 4.499);
                        }
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
                    });
                });
            }
            assert!(drew_text.get());
        }
    }

    #[test]
    fn native_hover_bounds_long_code_and_scrolls_in_both_axes() {
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
            sample.get().1[0] > 100.0 && sample.get().1[1] > 100.0,
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
        context.io_mut().add_mouse_wheel_event([-2.0, 0.0]);
        frame(&mut context, draw);
        assert!(sample.get().0[0] > 0.0, "{sample:?}");
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
