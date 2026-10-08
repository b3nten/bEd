use bed_editing::util::color::contrast_ratio;
use bed_terminal::terminal::{Terminal, TerminalColor, TerminalTheme};

fn theme(background: [f32; 4]) -> TerminalTheme {
    // Include deliberately unreadable colors to exercise theme contrast repair.
    let mut ansi = [background; 16];
    ansi[1] = [0.95, 0.35, 0.30, 1.0];
    ansi[2] = [0.40, 0.80, 0.45, 1.0];
    ansi[3] = [0.90, 0.85, 0.30, 1.0];
    TerminalTheme::new(background, background, ansi)
}

fn rgba(rgb: [u8; 3]) -> [f32; 4] {
    [
        rgb[0] as f32 / 255.0,
        rgb[1] as f32 / 255.0,
        rgb[2] as f32 / 255.0,
        1.0,
    ]
}

const LIGHT: [f32; 4] = [0.96, 0.94, 0.90, 1.0];
const DARK: [f32; 4] = [0.04, 0.05, 0.07, 1.0];

#[test]
fn defaults_ansi_and_cursor_remain_readable_after_theme_switches() {
    let mut terminal = Terminal::new(8, 2);
    terminal.feed(b"content");
    for background in [LIGHT, DARK, LIGHT] {
        let theme = theme(background);
        let before = terminal.revision();
        assert!(terminal.set_theme(&theme));
        assert_ne!(terminal.revision(), before);
        let palette = terminal.palette();
        for index in (0..16).chain([256, 258]) {
            assert!(
                contrast_ratio(rgba(palette[index]), rgba(palette[259])) >= 4.5,
                "index {index} is unreadable on {background:?}"
            );
        }
        assert_eq!(palette[257], palette[259]);
        assert_eq!(terminal.cell(0, 0).character, 'c');
        let revision = terminal.revision();
        assert!(!terminal.set_theme(&theme));
        assert_eq!(terminal.revision(), revision);
    }
}

#[test]
fn osc_overrides_survive_theme_switches_and_resets_restore_active_theme() {
    let mut terminal = Terminal::new(8, 2);
    terminal.set_theme(&theme(DARK));
    terminal.feed(b"\x1b]4;1;#123456\x07\x1b]10;#abcdef\x07\x1b]11;#fedcba\x07\x1b]12;#654321\x07");
    terminal.set_theme(&theme(LIGHT));
    assert_eq!(terminal.palette()[1], [0x12, 0x34, 0x56]);
    assert_eq!(terminal.palette()[258], [0xab, 0xcd, 0xef]);
    assert_eq!(terminal.palette()[259], [0xfe, 0xdc, 0xba]);
    assert_eq!(terminal.palette()[256], [0x65, 0x43, 0x21]);
    let mut expected = Terminal::new(8, 2);
    expected.set_theme(&theme(LIGHT));
    terminal.feed(b"\x1b]104;1\x07\x1b]110\x07\x1b]111\x07\x1b]112\x07");
    assert_eq!(terminal.palette(), expected.palette());
    terminal.feed(b"\x1b]4;2;#121212\x07\x1b]10;#efefef\x07\x1bc");
    assert_eq!(terminal.palette(), expected.palette());
    terminal.feed(b"\x1b]4;2;#121212\x07\x1b]104\x07");
    assert_eq!(terminal.palette(), expected.palette());
}

#[test]
fn truecolor_and_explicit_xterm_colors_are_preserved() {
    let mut terminal = Terminal::new(8, 2);
    terminal.feed(b"\x1b[38;2;240;240;240mT\x1b[38;5;231mX");
    let explicit_color = terminal.palette()[231];
    for background in [LIGHT, DARK] {
        terminal.set_theme(&theme(background));
        assert_eq!(terminal.cell(0, 0).fg, TerminalColor::Rgb([240, 240, 240]));
        assert_eq!(terminal.cell(0, 1).fg, TerminalColor::Indexed(231));
        assert_eq!(terminal.palette()[231], explicit_color);
    }
}

#[cfg(feature = "ui")]
#[test]
fn painted_text_faint_ansi_selection_and_cursor_use_theme_colors() {
    use bed_terminal::{
        terminal::SelectionSnap,
        terminal_view::{DrawOp, PaintMetrics, render_ops},
    };
    let metrics = PaintMetrics {
        cw: 8.0,
        ch: 16.0,
        ascent: 12.0,
        width: 100.0,
        height: 36.0,
    };
    let paint = |terminal: &Terminal| {
        render_ops(
            terminal,
            metrics,
            true,
            true,
            false,
            [false; 4],
            |_, _, pos| pos,
        )
    };
    let text_color = |ops: &[DrawOp], character: char| {
        ops.iter()
            .find_map(|op| match op {
                DrawOp::Text {
                    character: actual,
                    color,
                    ..
                } if *actual == character => Some(*color),
                _ => None,
            })
            .unwrap()
    };
    let mut terminal = Terminal::new(8, 2);
    terminal.feed(b"N\x1b[2mF\x1b[0;33mY\x1b[0;38;2;240;240;240mT\x1b[0m");
    for background in [LIGHT, DARK] {
        terminal.set_theme(&theme(background));
        let colors = terminal.palette();
        let output = paint(&terminal);
        for character in ['N', 'F', 'Y'] {
            assert!(
                contrast_ratio(
                    rgba(text_color(&output.rows[0], character)),
                    rgba(colors[259])
                ) >= 4.5
            );
        }
        assert_eq!(text_color(&output.rows[0], 'T'), [240, 240, 240]);
        assert!(
            output
                .overlay
                .iter()
                .any(|op| matches!(op, DrawOp::Rect { color, .. } if *color == colors[256]))
        );
        terminal.select_start(0, 0, SelectionSnap::Word);
        let selected = paint(&terminal);
        assert_eq!(text_color(&selected.rows[0], 'N'), terminal.palette()[259]);
        terminal.clear_selection();
    }
}
