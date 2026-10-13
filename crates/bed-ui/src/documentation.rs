//! The documentation formatting shared by LSP tooltips and persistent Docs.
use crate::documentation_markdown::{
    HoverProseRun, HoverProseStyle, parse_hover_markdown, parse_hover_prose, split_hover_lines,
};
#[cfg(feature = "documentation-syntax")]
pub use bed_highlight::tree_sitter::ThemeColors;
#[cfg(feature = "documentation-syntax")]
use bed_highlight::{
    capture_map::{THEME_SLOTS, ThemeSlot},
    tree_sitter::{LineColorSpans, TreeSitter},
};
use dear_imgui_rs::{StyleColor, StyleVar, Ui, sys};
#[cfg(not(feature = "documentation-syntax"))]
pub use plain::ThemeColors;
#[cfg(not(feature = "documentation-syntax"))]
use plain::{LineColorSpans, THEME_SLOTS, ThemeSlot};

/// Documentation works with the UI palette when syntax services are absent.
/// Enabling `documentation-syntax` retains the editor's syntax theme API.
#[cfg(not(feature = "documentation-syntax"))]
mod plain {
    #[derive(Clone, Copy, Debug)]
    pub enum ThemeSlot {
        Text,
        String,
    }
    pub const THEME_SLOTS: [ThemeSlot; 2] = [ThemeSlot::Text, ThemeSlot::String];

    #[derive(Clone, Debug, PartialEq)]
    pub struct ThemeColors {
        pub slots: [[f32; 4]; 2],
    }
    impl Default for ThemeColors {
        fn default() -> Self {
            Self {
                slots: [[0.9, 0.9, 0.9, 1.0]; 2],
            }
        }
    }

    pub struct LineColorSpan {
        pub start: i32,
        pub end: i32,
        pub slot: ThemeSlot,
    }
    pub type LineColorSpans = Vec<LineColorSpan>;
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Documentation {
    pub text: String,
    pub markdown: bool,
    pub language: String,
    pub colors: ThemeColors,
}

impl Documentation {
    pub fn render(&self, ui: &Ui) {
        if self.markdown {
            render_markdown(ui, &self.text, &self.colors, &self.language);
        } else {
            render_plaintext(ui, &self.text, &self.colors);
        }
    }
}

impl From<String> for Documentation {
    fn from(text: String) -> Self {
        Self {
            text,
            ..Default::default()
        }
    }
}

struct HoverColors {
    slots: [[f32; 4]; THEME_SLOTS.len()],
    bold: [f32; 4],
}

impl HoverColors {
    fn new(ui: &Ui, colors: &ThemeColors) -> Self {
        use crate::color::{blend, relative_luminance};
        let mut slots = THEME_SLOTS
            .map(|slot| crate::presentation::readable_color(ui, colors.slots[slot as usize]));
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
                        sys::ImDrawTextFlags_WrapKeepBlanks
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
pub fn render_plaintext(ui: &Ui, text: &str, colors: &ThemeColors) {
    let palette = HoverColors::new(ui, colors);
    let runs = [HoverProseRun {
        range: 0..text.len(),
        style: HoverProseStyle::Text,
    }];
    draw_prose(ui, text, &runs, &palette);
}

pub fn render_markdown(ui: &Ui, markdown: &str, colors: &ThemeColors, fallback_language: &str) {
    let fs = ui.current_font_size();
    // Resolve the small theme palette once, independent of code/prose length.
    let palette = HoverColors::new(ui, colors);
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
            #[cfg(feature = "documentation-syntax")]
            let colors = if language.is_empty() {
                Vec::new()
            } else {
                TreeSitter::highlight_snippet(language, block.text.as_bytes())
            };
            #[cfg(not(feature = "documentation-syntax"))]
            let colors: Vec<LineColorSpans> = {
                let _ = language;
                Vec::new()
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

#[cfg(test)]
mod tests {
    use super::*;
    use dear_imgui_rs::{Condition, Context, FramePrepareOptions, WindowFlags};

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
        ui.window("documentation fixture")
            .position([0.0; 2], Condition::Always)
            .size([640.0, 480.0], Condition::Always)
            .build(|| draw(ui));
        drop(context.render_legacy());
    }

    #[test]
    fn native_prose_wraps_styled_words_unicode_and_long_words_at_available_width() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = context();
        let editor = ThemeColors::default();
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
                    render_markdown(ui, &markdown.repeat(3), &editor, "");
                    assert!(ui.cursor_screen_pos()[1] - start > ui.text_line_height() * 8.0);
                });
        });
        for _ in 0..3 {
            frame(&mut context, |ui| {
                ui.child_window("narrow documentation")
                    .size([130.0, 120.0])
                    .flags(WindowFlags::HORIZONTAL_SCROLLBAR)
                    .build(ui, || {
                        render_markdown(ui, &markdown.repeat(3), &editor, "");
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
    fn native_wrapped_code_preserves_indentation_unicode_and_highlighting() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = context();
        let editor = ThemeColors::default();
        #[cfg(feature = "documentation-syntax")]
        let editor = {
            let mut editor = editor;
            editor.slots[ThemeSlot::Keyword as usize] = [0.9, 0.3, 0.4, 1.0];
            editor
        };
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
                    #[cfg(feature = "documentation-syntax")]
                    let spans = TreeSitter::highlight_snippet("rust", code.as_bytes());
                    #[cfg(not(feature = "documentation-syntax"))]
                    let spans: Vec<LineColorSpans> = vec![Vec::new()];
                    #[cfg(feature = "documentation-syntax")]
                    assert!(!spans[0].is_empty());
                    let start_y = ui.cursor_screen_pos()[1];
                    #[cfg(feature = "documentation-syntax")]
                    let start_vertex = ui.with_bound_context(|| unsafe {
                        (*sys::igGetWindowDrawList()).VtxBuffer.Size
                    });
                    draw_code_line(ui, code, &spans[0], &palette);
                    assert!(ui.cursor_screen_pos()[1] - start_y > ui.text_line_height() * 3.0);
                    #[cfg(feature = "documentation-syntax")]
                    let colors = ui.with_bound_context(|| unsafe {
                        let vertices = &(*sys::igGetWindowDrawList()).VtxBuffer;
                        (start_vertex..vertices.Size)
                            .map(|index| (*vertices.Data.add(index as usize)).col)
                            .collect::<std::collections::HashSet<_>>()
                    });
                    #[cfg(feature = "documentation-syntax")]
                    assert!(colors.len() > 1, "wrapped code retains syntax colors");
                });
        });
    }
}
