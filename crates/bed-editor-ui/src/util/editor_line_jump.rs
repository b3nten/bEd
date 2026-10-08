//! Go-to-line overlay translated from ned editor/util/editor_line_jump.{h,cpp}.
//! See LICENSE and NOTICE for source attribution.
use crate::util::editor_finder::{EditorOverlayStyle, OverlayOutcome, bounded_input};
use bed_document_session::ViewContext;
#[cfg(test)]
use bed_document_session::editor::Editor;
use dear_imgui_rs::{Condition, InputTextFlags, Key, StyleColor, StyleVar, Ui, WindowFlags};

#[derive(Default)]
pub struct EditorLineJump {
    pub active: bool,
    input: String,
    should_focus: bool,
}

impl EditorLineJump {
    pub fn open(&mut self) {
        self.active = true;
        self.should_focus = true;
        self.input.clear();
    }
    pub fn dismiss(&mut self) {
        self.active = false;
        self.should_focus = false;
        self.input.clear();
    }
    pub fn jump_to_line(&mut self, editor: &mut ViewContext<'_>, line: i32) -> OverlayOutcome {
        editor.commands().go_to_line(line);
        editor.api().request_ensure_visible();
        OverlayOutcome {
            suppress_next_enter: true,
            ..OverlayOutcome::default()
        }
    }
    pub fn draw(&mut self, ui: &Ui, editor: &mut ViewContext<'_>) -> OverlayOutcome {
        self.draw_with_style(ui, editor, &EditorOverlayStyle::default())
    }
    pub fn draw_with_style(
        &mut self,
        ui: &Ui,
        editor: &mut ViewContext<'_>,
        style: &EditorOverlayStyle,
    ) -> OverlayOutcome {
        if !self.active {
            return OverlayOutcome::default();
        }
        if ui.is_key_pressed(Key::Escape) {
            self.dismiss();
            editor.view_mut().block_input = false;
            return OverlayOutcome::default();
        }
        editor.view_mut().block_input = true;
        let fs = ui.current_font_size();
        let pane = style
            .pane
            .unwrap_or_else(|| (ui.window_pos(), ui.window_size()));
        let (position, size) = line_jump_geometry(fs, pane);
        let background = style.dimmed_background(ui);
        let _round = ui.push_style_var(StyleVar::WindowRounding(fs * 0.5));
        let _border_size = ui.push_style_var(StyleVar::WindowBorderSize(1.0));
        let _padding = ui.push_style_var(StyleVar::WindowPadding([fs * 0.8; 2]));
        let _bg = ui.push_style_color(StyleColor::WindowBg, background);
        let _border = ui.push_style_color(StyleColor::Border, ui.style_color(StyleColor::Border));
        let _frame_bg = ui.push_style_color(StyleColor::FrameBg, background);
        let instance = self as *const Self;
        let mut enter_pressed = false;
        ui.window(format!("LineJump###lj_{instance:p}"))
            .size(size, Condition::Always)
            .position(position, Condition::Always)
            .flags(
                WindowFlags::NO_TITLE_BAR
                    | WindowFlags::NO_RESIZE
                    | WindowFlags::NO_MOVE
                    | WindowFlags::NO_SCROLLBAR
                    | WindowFlags::NO_SCROLL_WITH_MOUSE,
            )
            .build(|| {
                ui.text("Jump to line:");
                ui.spacing();
                ui.spacing();
                if self.should_focus {
                    ui.set_keyboard_focus_here();
                    self.should_focus = false;
                }
                ui.set_next_item_width(ui.content_region_avail()[0]);
                {
                    let _round = ui.push_style_var(StyleVar::FrameRounding(fs * 0.2));
                    let _border_size = ui.push_style_var(StyleVar::FrameBorderSize(1.0));
                    let _pad = ui.push_style_var(StyleVar::FramePadding([fs * 0.4; 2]));
                    let _border =
                        ui.push_style_color(StyleColor::Border, ui.style_color(StyleColor::Border));
                    let _bg = ui.push_style_color(StyleColor::FrameBg, background);
                    // As upstream, re-grab every frame after Enter deactivates InputText.
                    ui.set_keyboard_focus_here();
                    enter_pressed = bounded_input::<32>(
                        ui,
                        &format!("##LineJumpInput_{instance:p}"),
                        None,
                        &mut self.input,
                        InputTextFlags::CHARS_DECIMAL | InputTextFlags::ENTER_RETURNS_TRUE,
                    );
                }
                if !enter_pressed {
                    ui.spacing();
                    ui.text("Type line number then Enter");
                }
            });
        if enter_pressed {
            let line = parse_line_number(&self.input).saturating_sub(1);
            let mut outcome = self.jump_to_line(editor, line);
            outcome.clear_input_keys = true;
            self.dismiss();
            editor.view_mut().block_input = false;
            outcome
        } else {
            OverlayOutcome {
                block_input: true,
                ..OverlayOutcome::default()
            }
        }
    }
}

pub fn line_jump_geometry(
    font_size: f32,
    (pane_pos, pane_size): ([f32; 2], [f32; 2]),
) -> ([f32; 2], [f32; 2]) {
    let size = [font_size * 20.0, font_size * 6.0];
    let mut pos = [
        pane_pos[0] + pane_size[0] * 0.5 - size[0] * 0.5,
        pane_pos[1] + pane_size[1] * 0.35 - size[1] * 0.5,
    ];
    // Preserve the sequential upstream clamps, including panes smaller than the overlay.
    for axis in 0..2 {
        if pos[axis] < pane_pos[axis] {
            pos[axis] = pane_pos[axis];
        }
        if pos[axis] + size[axis] > pane_pos[axis] + pane_size[axis] {
            pos[axis] = pane_pos[axis] + pane_size[axis] - size[axis];
        }
    }
    (pos, size)
}

/// atoi accepts a signed integer prefix (InputText's decimal filter also admits '.' and operators).
/// Overflow was undefined in C++; clamp it to a defined i32 value in Rust.
fn parse_line_number(input: &str) -> i32 {
    let bytes = input
        .trim_start_matches(|c: char| c.is_ascii_whitespace())
        .as_bytes();
    let (negative, digits) = match bytes.first() {
        Some(b'-') => (true, &bytes[1..]),
        Some(b'+') => (false, &bytes[1..]),
        _ => (false, bytes),
    };
    let magnitude =
        digits
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .fold(0_i64, |value, &byte| {
                value
                    .saturating_mul(10)
                    .saturating_add(i64::from(byte - b'0'))
            });
    let value = if negative { -magnitude } else { magnitude };
    value.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

#[cfg(test)]
mod tests {
    use super::*;
    use dear_imgui_rs::{Context, FramePrepareOptions};
    #[test]
    fn geometry_uses_font_pane_fraction_and_sequential_clamps() {
        assert_eq!(
            line_jump_geometry(20.0, ([100.0, 80.0], [1000.0, 600.0])),
            ([400.0, 230.0], [400.0, 120.0])
        );
        assert_eq!(
            line_jump_geometry(20.0, ([100.0, 80.0], [100.0, 50.0])),
            ([-200.0, 10.0], [400.0, 120.0])
        );
    }
    #[test]
    fn numeric_prefix_matches_atoi_and_overflow_has_defined_bounds() {
        for (input, expected) in [
            ("", 0),
            ("12", 12),
            ("12.5", 12),
            ("+42*2", 42),
            ("-3", -3),
            (".2", 0),
            (" /9", 0),
        ] {
            assert_eq!(parse_line_number(input), expected);
        }
        assert_eq!(
            parse_line_number("9999999999999999999999999999999"),
            i32::MAX
        );
        assert_eq!(
            parse_line_number("-999999999999999999999999999999"),
            i32::MIN
        );
    }
    #[test]
    fn jump_and_dismiss_preserve_upstream_caret_and_reset_input() {
        let mut editor = Editor::new();
        editor.set_content(b"a\nb\nc");
        let mut jump = EditorLineJump::default();
        jump.open();
        jump.input = "2".into();
        let outcome = jump.jump_to_line(&mut editor.view_context(), 1);
        assert_eq!((editor.view.row, editor.view.column), (1, 0));
        assert!(editor.view.center_cursor_vertical);
        assert!(editor.view.ensure_cursor_visible.vertical);
        assert!(editor.view.ensure_cursor_visible.horizontal);
        assert!(outcome.suppress_next_enter);
        jump.dismiss();
        assert!(!jump.active);
        assert!(!jump.should_focus);
        assert!(jump.input.is_empty());
    }

    #[test]
    fn enter_in_numeric_overlay_jumps_clears_input_and_suppresses_document_enter() {
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
        editor.set_content(b"a\nb\nc");
        let mut jump = EditorLineJump::default();
        jump.open();
        let render = |context: &mut Context, jump: &mut EditorLineJump, editor: &mut Editor| {
            context.prepare_frame(FramePrepareOptions::new([800.0, 600.0], 1.0 / 60.0));
            let ui = context.frame();
            let mut outcome = OverlayOutcome::default();
            ui.window("Editor host")
                .size([700.0, 450.0], Condition::Always)
                .build(|| {
                    outcome = jump.draw(ui, &mut editor.view_context());
                });
            drop(context.render_legacy());
            outcome
        };
        render(&mut context, &mut jump, &mut editor);
        render(&mut context, &mut jump, &mut editor);
        context.io_mut().add_input_characters_utf8("2.5a");
        assert!(render(&mut context, &mut jump, &mut editor).block_input);
        assert_eq!(jump.input, "2.5");
        context.io_mut().add_key_event(Key::Enter, true);
        let outcome = render(&mut context, &mut jump, &mut editor);
        assert_eq!((editor.view.row, editor.view.column), (1, 0));
        assert_eq!(editor.state.join(), b"a\nb\nc");
        assert!(!jump.active);
        assert!(jump.input.is_empty());
        assert!(outcome.suppress_next_enter && outcome.clear_input_keys);
        assert!(!outcome.block_input);
        context.io_mut().clear_input_keys();
        jump.open();
        render(&mut context, &mut jump, &mut editor);
        context.io_mut().add_key_event(Key::Escape, true);
        let outcome = render(&mut context, &mut jump, &mut editor);
        assert!(!jump.active);
        assert_eq!(outcome, OverlayOutcome::default());
    }
}
