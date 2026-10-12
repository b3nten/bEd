//! Translated from ned editor/views/hover_tooltip.{h,cpp}; see LICENSE and NOTICE.
use crate::views::{
    diagnostic_style::{severity_color, severity_label},
    hover_markdown::{
        HoverProseRun, HoverProseStyle, parse_hover_markdown, parse_hover_prose, split_hover_lines,
    },
};
use bed_document_session::editor::Editor;
use bed_editing::diagnostic::DiagnosticItem;
use bed_highlight::{
    capture_map::{THEME_SLOTS, ThemeSlot},
    tree_sitter::{LineColorSpans, TreeSitter},
};
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
        (fs * 30.0)
            .min(viewport_size[0] - margin * 2.0)
            .floor()
            .max(1.0),
        (fs * 18.0)
            .min(viewport_size[1] - margin * 2.0)
            .floor()
            .max(1.0),
    ];
    let size = previous_rect
        .filter(|(min, max)| max[0] > min[0] && max[1] > min[1])
        .map_or(maximum, |(min, max)| {
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
    let _style = hover_popup_style(ui);
    ui.set_next_window_viewport(viewport.id());
    let mut window = ui
        .window(name)
        .position(position, Condition::Always)
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
        use bed_ui::color::{blend, relative_luminance};
        let mut slots = THEME_SLOTS.map(|slot| {
            bed_ui::presentation::readable_color(ui, editor.highlight.color_for_slot(slot))
        });
        let text = bed_ui::presentation::readable_color(ui, ui.style_color(StyleColor::Text));
        slots[ThemeSlot::Text as usize] = text;
        let background = ui.style_color(StyleColor::WindowBg);
        let stronger = if relative_luminance(text) > relative_luminance(background) {
            [1.0, 1.0, 1.0, text[3]]
        } else {
            [0.0, 0.0, 0.0, text[3]]
        };
        let bold = bed_ui::presentation::readable_color(ui, blend(text, stronger, 0.85));
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
    let visual_lines = wrapped_hover_lines(ui, line, hover_wrap_width(ui), true);
    let pos = ui.cursor_screen_pos();
    let font_size = ui.current_font_size();
    let mut maximum_width: f32 = 1.0;
    let mut span_index = 0;
    for (row, visual_line) in visual_lines.iter().enumerate() {
        let mut x = pos[0];
        let mut offset = visual_line.start;
        while offset < visual_line.end {
            while span_index < spans.len() && spans[span_index].end <= offset as i32 {
                span_index += 1;
            }
            let (color, end) = match spans.get(span_index) {
                Some(span) if span.start <= offset as i32 => (
                    colors.color(span.slot),
                    visual_line.end.min(span.end as usize),
                ),
                Some(span) => (
                    colors.color(ThemeSlot::Text),
                    visual_line.end.min(span.start as usize),
                ),
                None => (colors.color(ThemeSlot::Text), visual_line.end),
            };
            let advance = ui.with_bound_context(|| unsafe {
                // Native AddText consumes bounded UTF-8 runs during the frame.
                // Original byte ranges keep highlighting across visual wraps.
                sys::ImDrawList_AddText_Vec2(
                    sys::igGetWindowDrawList(),
                    [x, pos[1] + row as f32 * height].into(),
                    sys::igColorConvertFloat4ToU32(color.into()),
                    line.as_ptr().add(offset).cast(),
                    line.as_ptr().add(end).cast(),
                );
                sys::ImFont_CalcTextSizeA(
                    sys::igGetFont(),
                    font_size,
                    f32::MAX,
                    0.0,
                    line.as_ptr().add(offset).cast(),
                    line.as_ptr().add(end).cast(),
                    std::ptr::null_mut(),
                )
                .x
            });
            x += advance;
            offset = end;
        }
        maximum_width = maximum_width.max(x - pos[0]);
    }
    ui.dummy([maximum_width, visual_lines.len() as f32 * height]);
}

fn wrapped_hover_lines(
    ui: &Ui,
    text: &str,
    width: f32,
    keep_blanks: bool,
) -> Vec<std::ops::Range<usize>> {
    let mut lines = Vec::new();
    let font_size = ui.current_font_size();
    let mut source_offset = 0;
    for source_line in text.split('\n') {
        if source_line.is_empty() {
            lines.push(source_offset..source_offset);
        }
        let mut offset = 0;
        while offset < source_line.len() {
            let end = ui.with_bound_context(|| unsafe {
                let begin = source_line.as_ptr().add(offset).cast();
                // Wrap the whole line before applying inline or syntax colors.
                // Keep code whitespace and hard-break long identifiers safely.
                let wrapped = sys::igImFontCalcWordWrapPositionEx(
                    sys::igGetFont(),
                    font_size,
                    begin,
                    source_line.as_ptr().add(source_line.len()).cast(),
                    width.max(1.0),
                    if keep_blanks {
                        sys::ImDrawTextFlags_WrapKeepBlanks as i32
                    } else {
                        0
                    },
                );
                offset + wrapped.offset_from(begin) as usize
            });
            assert!(end > offset && source_line.is_char_boundary(end));
            lines.push(source_offset + offset..source_offset + end);
            offset = end;
            while !keep_blanks
                && source_line
                    .as_bytes()
                    .get(offset)
                    .is_some_and(|byte| matches!(byte, b' ' | b'\t'))
            {
                offset += 1;
            }
        }
        source_offset += source_line.len() + 1;
    }
    lines
}

fn hover_wrap_width(ui: &Ui) -> f32 {
    // A vertical scrollbar is chosen from the previous frame's content. Keep
    // the wrap width stable when long documentation makes it appear next frame.
    let scrollbar_reserve = ui.with_bound_context(|| unsafe {
        if (*sys::igGetCurrentWindow()).ScrollbarY {
            0.0
        } else {
            (*sys::igGetStyle()).ScrollbarSize
        }
    });
    // Native window sizes and content bounds round independently. Reserve a
    // pixel so a fitting prose line cannot create a fractional scrollbar.
    (ui.content_region_avail()[0] - scrollbar_reserve - 1.0).max(1.0)
}

fn draw_prose(ui: &Ui, text: &str, runs: &[HoverProseRun], colors: &HoverColors) {
    let lines = wrapped_hover_lines(ui, text, hover_wrap_width(ui), false);
    let pos = ui.cursor_screen_pos();
    let height = ui.text_line_height();
    let font_size = ui.current_font_size();
    let mut y = pos[1];
    let mut maximum_width: f32 = 1.0;
    let mut run_index = 0;
    for line in &lines {
        let mut x = pos[0];
        while run_index < runs.len() && runs[run_index].range.end <= line.start {
            run_index += 1;
        }
        for run in &runs[run_index..] {
            if run.range.start >= line.end {
                break;
            }
            let start = run.range.start.max(line.start);
            let end = run.range.end.min(line.end);
            let color = match run.style {
                HoverProseStyle::Text => colors.color(ThemeSlot::Text),
                HoverProseStyle::InlineCode => colors.color(ThemeSlot::String),
                HoverProseStyle::Bold => colors.bold,
            };
            let advance = ui.with_bound_context(|| unsafe {
                // Bounded UTF-8 ranges are consumed synchronously by ImGui.
                // Keep fractional glyph advances across runs; rounding each
                // run would widen a styled word and change its layout.
                sys::ImDrawList_AddText_Vec2(
                    sys::igGetWindowDrawList(),
                    [x, y].into(),
                    sys::igColorConvertFloat4ToU32(color.into()),
                    text.as_ptr().add(start).cast(),
                    text.as_ptr().add(end).cast(),
                );
                sys::ImFont_CalcTextSizeA(
                    sys::igGetFont(),
                    font_size,
                    f32::MAX,
                    0.0,
                    text.as_ptr().add(start).cast(),
                    text.as_ptr().add(end).cast(),
                    std::ptr::null_mut(),
                )
                .x
            });
            x += advance;
        }
        maximum_width = maximum_width.max(x - pos[0]);
        // Empty lines retain the paragraph gap rather than gaining a full
        // additional paragraph's worth of height.
        y += if line.is_empty() {
            height * 0.35
        } else {
            height
        };
    }
    ui.dummy([maximum_width, (y - pos[1]).max(height * 0.35)]);
}

/// Render literal LSP plaintext documentation in the same bounded layout as
/// hover markdown, without interpreting punctuation as inline markup.
pub fn render_hover_plaintext(ui: &Ui, text: &str, editor: &Editor) {
    let palette = HoverColors::new(ui, editor);
    let runs = [HoverProseRun {
        range: 0..text.len(),
        style: HoverProseStyle::Text,
    }];
    draw_prose(ui, text, &runs, &palette);
}

pub fn render_hover_markdown(ui: &Ui, markdown: &str, editor: &Editor, fallback_language: &str) {
    let fs = ui.current_font_size();
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
            let (text, runs) = parse_hover_prose(&block.text);
            draw_prose(ui, &text, &runs, &palette);
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
    fn native_prose_wraps_styled_words_unicode_and_long_words_at_available_width() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = context();
        let editor = Editor::new();
        let markdown = "Prefix Con**cat**enate `变量🙂` and [résumé](https://example.org).\n\nExplicit break.\nABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
        let (text, _) = parse_hover_prose(markdown);
        frame(&mut context, |ui| {
            for width in [260.0, 120.0, 30.0] {
                let lines = wrapped_hover_lines(ui, &text, width, false);
                assert!(lines.len() > 4);
                assert!(lines.iter().any(std::ops::Range::is_empty));
                for line in &lines {
                    assert!(text.is_char_boundary(line.start) && text.is_char_boundary(line.end));
                    assert!(
                        ui.calc_text_size(&text[line.clone()])[0] <= width + 1.0,
                        "width {width}, {:?}",
                        &text[line.clone()]
                    );
                }
                // Inline formatting cannot change the break points for words.
                let visible = lines
                    .iter()
                    .map(|line| &text[line.clone()])
                    .collect::<String>();
                assert_eq!(
                    visible.split_whitespace().collect::<String>(),
                    text.split_whitespace().collect::<String>()
                );
            }
            ui.child_window("narrow documentation")
                .size([130.0, 120.0])
                .flags(WindowFlags::HORIZONTAL_SCROLLBAR)
                .build(ui, || {
                    let start = ui.cursor_screen_pos()[1];
                    render_hover_markdown(ui, &markdown.repeat(3), &editor, "");
                    assert!(ui.cursor_screen_pos()[1] - start > ui.text_line_height() * 8.0);
                });
        });
        for _ in 0..3 {
            frame(&mut context, |ui| {
                ui.child_window("narrow documentation")
                    .size([130.0, 120.0])
                    .flags(WindowFlags::HORIZONTAL_SCROLLBAR)
                    .build(ui, || {
                        render_hover_markdown(ui, &markdown.repeat(3), &editor, "");
                        assert!(ui.scroll_max_y() > 0.0);
                        assert_eq!(
                            ui.scroll_max_x(),
                            0.0,
                            "prose wraps instead of scrolling horizontally"
                        );
                    });
            });
        }
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
                        || {
                            let signature = &parse_hover_markdown(markdown)[1].text;
                            assert!(
                                wrapped_hover_lines(ui, signature, hover_wrap_width(ui), true)
                                    .len()
                                    > 3
                            );
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
    fn native_wrapped_code_preserves_indentation_unicode_and_highlighting() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = context();
        let editor = Editor::new();
        let code =
            "    pub fn extraordinarily_long_identifier_with_变量_🙂(name: &str) -> usize { 1 }";
        frame(&mut context, |ui| {
            for width in [120.0, 30.0] {
                let lines = wrapped_hover_lines(ui, code, width, true);
                assert!(lines.len() > 2);
                assert_eq!(
                    lines
                        .iter()
                        .map(|range| &code[range.clone()])
                        .collect::<String>(),
                    code
                );
                for line in lines {
                    assert!(code.is_char_boundary(line.start) && code.is_char_boundary(line.end));
                    assert!(ui.calc_text_size(&code[line])[0] <= width + 1.0);
                }
            }
            ui.child_window("wrapped highlighted signature")
                .size([130.0, 220.0])
                .build(ui, || {
                    let palette = HoverColors::new(ui, &editor);
                    let spans = TreeSitter::highlight_snippet("rust", code.as_bytes());
                    assert!(!spans[0].is_empty());
                    let start_y = ui.cursor_screen_pos()[1];
                    let start_vertex = ui.with_bound_context(|| unsafe {
                        (*sys::igGetWindowDrawList()).VtxBuffer.Size
                    });
                    draw_code_line(ui, code, &spans[0], &palette);
                    assert!(ui.cursor_screen_pos()[1] - start_y > ui.text_line_height() * 3.0);
                    let colors = ui.with_bound_context(|| unsafe {
                        let vertices = &(*sys::igGetWindowDrawList()).VtxBuffer;
                        (start_vertex..vertices.Size)
                            .map(|index| (*vertices.Data.add(index as usize)).col)
                            .collect::<std::collections::HashSet<_>>()
                    });
                    assert!(colors.len() > 1, "wrapped code retains syntax colors");
                });
        });
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
