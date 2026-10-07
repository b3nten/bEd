//! Translated from ned editor/views/hover_tooltip.{h,cpp}; see LICENSE and NOTICE.
use crate::views::{
    diagnostic_style::{severity_color, severity_label},
    hover_markdown::{parse_hover_markdown, split_hover_lines},
};
use bed_highlight::{
    capture_map::ThemeSlot,
    tree_sitter::{LineColorSpans, TreeSitter},
};
use bed_lsp::diagnostics::diagnostics_store::DiagnosticItem;
use bed_session::editor::Editor;
use dear_imgui_rs::{StyleColor, StyleVar, Ui, sys};
use std::cell::Cell;

pub struct TooltipArbiter {
    claimed_frame: Cell<Option<usize>>,
}
impl Default for TooltipArbiter {
    fn default() -> Self {
        Self {
            claimed_frame: Cell::new(None),
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
}

fn draw_code_line(ui: &Ui, line: &str, spans: &LineColorSpans, editor: &Editor) {
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
            Some(span) if span.start <= offset as i32 => (
                editor.highlight.color_for_slot(span.slot),
                line.len().min(span.end as usize),
            ),
            Some(span) => (
                editor.highlight.default_text_color(),
                line.len().min(span.start as usize),
            ),
            None => (editor.highlight.default_text_color(), line.len()),
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

fn draw_prose_line(ui: &Ui, line: &str, editor: &Editor) {
    let text = editor.highlight.default_text_color();
    let code = editor.highlight.color_for_slot(ThemeSlot::String);
    let mut first = true;
    let mut emit = |piece: &str, color: [f32; 4], bold: bool| {
        if piece.is_empty() {
            return;
        }
        if !first {
            ui.same_line_with_spacing(0.0, 0.0);
        }
        first = false;
        let mut color = color;
        if bold {
            for channel in &mut color[..3] {
                *channel = (*channel * 1.15).min(1.0);
            }
        }
        let _text = ui.push_style_color(StyleColor::Text, color);
        ui.text(piece);
    };
    let bytes = line.as_bytes();
    let mut offset = 0;
    while offset < bytes.len() {
        if bytes[offset] == b'`'
            && let Some(end) = line[offset + 1..].find('`').map(|n| n + offset + 1)
        {
            emit(&line[offset + 1..end], code, false);
            offset = end + 1;
            continue;
        }
        if line[offset..].starts_with("**")
            && let Some(end) = line[offset + 2..].find("**").map(|n| n + offset + 2)
        {
            emit(&line[offset + 2..end], text, true);
            offset = end + 2;
            continue;
        }
        if bytes[offset] == b'['
            && let Some(close) = line[offset + 1..].find(']').map(|n| n + offset + 1)
            && bytes.get(close + 1) == Some(&b'(')
            && let Some(end) = line[close + 2..].find(')').map(|n| n + close + 2)
        {
            emit(&line[offset + 1..close], text, false);
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
        emit(&line[offset..next], text, false);
        offset = next;
    }
    if first {
        ui.dummy([1.0, ui.text_line_height()]);
    }
}

pub fn render_hover_markdown(ui: &Ui, markdown: &str, editor: &Editor, fallback_language: &str) {
    let fs = ui.current_font_size();
    let _wrap = ui.push_text_wrap_pos(fs * 32.0);
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
                draw_code_line(ui, line, colors.get(row).unwrap_or(&Vec::new()), editor);
            }
        } else {
            for line in split_hover_lines(&block.text) {
                if line.is_empty() {
                    ui.dummy([1.0, ui.text_line_height() * 0.35]);
                } else {
                    draw_prose_line(ui, &line, editor);
                }
            }
        }
    }
}

pub fn render_diagnostic_tooltip(ui: &Ui, items: &[DiagnosticItem], arbiter: &TooltipArbiter) {
    if items.is_empty() || !arbiter.claim(ui) {
        return;
    }
    let fs = ui.current_font_size();
    let _padding = ui.push_style_var(StyleVar::WindowPadding([fs * 0.65, fs * 0.45]));
    let _spacing = ui.push_style_var(StyleVar::ItemSpacing([fs * 0.4, fs * 0.25]));
    if let Some(_tooltip) = ui.begin_tooltip() {
        for (index, item) in items.iter().enumerate() {
            if index > 0 {
                ui.separator();
            }
            ui.text_colored(severity_color(item.severity), severity_label(item.severity));
            if !item.source.is_empty() {
                ui.same_line();
                ui.text_disabled(&item.source);
            }
            let _wrap = ui.push_text_wrap_pos(fs * 28.0);
            ui.text(&item.message);
        }
    }
}
