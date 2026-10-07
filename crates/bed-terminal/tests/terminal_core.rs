//! Core terminal comparisons against pinned ImGui-Terminal fixture output.
//! Original adapter/tests attribution: resources/terminal/LICENSE and NOTICE.
use bed_terminal::terminal::{ATTR_BOLD, ATTR_ITALIC, ATTR_WDUMMY, Terminal, TerminalColor};
use serde_json::Value;

#[test]
fn all_nine_upstream_terminal_scripts_preserve_cells_styles_colors_and_cursor() {
    let fixtures = [
        (
            "attrs_basic",
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../tests/fixtures/terminal_core/attrs_basic/output.bin"
            ))
            .as_slice(),
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../tests/fixtures/terminal_core/attrs_basic/expected.json"
            )),
        ),
        (
            "colors_ansi_16",
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../tests/fixtures/terminal_core/colors_ansi_16/output.bin"
            ))
            .as_slice(),
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../tests/fixtures/terminal_core/colors_ansi_16/expected.json"
            )),
        ),
        (
            "colors_osc4",
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../tests/fixtures/terminal_core/colors_osc4/output.bin"
            ))
            .as_slice(),
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../tests/fixtures/terminal_core/colors_osc4/expected.json"
            )),
        ),
        (
            "colors_truecolor",
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../tests/fixtures/terminal_core/colors_truecolor/output.bin"
            ))
            .as_slice(),
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../tests/fixtures/terminal_core/colors_truecolor/expected.json"
            )),
        ),
        (
            "cursor_bar",
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../tests/fixtures/terminal_core/cursor_bar/output.bin"
            ))
            .as_slice(),
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../tests/fixtures/terminal_core/cursor_bar/expected.json"
            )),
        ),
        (
            "cursor_block",
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../tests/fixtures/terminal_core/cursor_block/output.bin"
            ))
            .as_slice(),
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../tests/fixtures/terminal_core/cursor_block/expected.json"
            )),
        ),
        (
            "cursor_underline",
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../tests/fixtures/terminal_core/cursor_underline/output.bin"
            ))
            .as_slice(),
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../tests/fixtures/terminal_core/cursor_underline/expected.json"
            )),
        ),
        (
            "hello",
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../tests/fixtures/terminal_core/hello/output.bin"
            ))
            .as_slice(),
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../tests/fixtures/terminal_core/hello/expected.json"
            )),
        ),
        (
            "wide_cjk",
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../tests/fixtures/terminal_core/wide_cjk/output.bin"
            ))
            .as_slice(),
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../tests/fixtures/terminal_core/wide_cjk/expected.json"
            )),
        ),
    ];
    for (name, output, expected) in fixtures {
        let expected: Value = serde_json::from_str(expected).unwrap();
        let mut terminal = Terminal::new(80, 24);
        // Chunking also verifies parser and UTF-8 state across arbitrary reads.
        for chunk in output.chunks(3) {
            terminal.feed(chunk);
        }
        assert_eq!(
            terminal.cursor().shape,
            expected["cursor_shape"].as_u64().unwrap() as u8,
            "{name}"
        );
        assert_eq!(
            terminal.title(),
            expected["title"].as_str().unwrap(),
            "{name}"
        );
        for row in expected["rows"].as_array().unwrap() {
            let row_number = row["row"].as_u64().unwrap() as usize;
            for op in row["ops"].as_array().unwrap() {
                if op["kind"] != "TEXT" {
                    continue;
                }
                let col = ((op["p0"][0].as_i64().unwrap() - 2) / 8) as usize;
                let cell = terminal.cell(row_number, col);
                assert_eq!(
                    cell.mode & ATTR_WDUMMY,
                    0,
                    "{name} row {row_number} col {col}"
                );
                assert_eq!(
                    cell.character.to_string(),
                    op["text"].as_str().unwrap(),
                    "{name} row {row_number} col {col}"
                );
                let fg = match cell.fg {
                    TerminalColor::Rgb(rgb) => rgb,
                    TerminalColor::Indexed(index) => {
                        terminal.palette()[if index < 8 && cell.mode & ATTR_BOLD != 0 {
                            index + 8
                        } else {
                            index
                        }]
                    }
                };
                assert_eq!(
                    format!("#{:02x}{:02x}{:02x}", fg[0], fg[1], fg[2]),
                    op["col"].as_str().unwrap(),
                    "{name} row {row_number} col {col}"
                );
                let font = match cell.mode & (ATTR_BOLD | ATTR_ITALIC) {
                    0 => "regular",
                    ATTR_BOLD => "bold",
                    ATTR_ITALIC => "italic",
                    _ => "bold_italic",
                };
                assert_eq!(
                    font,
                    op["font"].as_str().unwrap(),
                    "{name} row {row_number} col {col}"
                );
            }
        }
    }
}

#[test]
fn difficult_state_transitions_match_unchanged_upstream_methods() {
    use bed_terminal::terminal::{SelectionSnap, TerminalEvent};
    let fixture: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/terminal_core/state.json"
    )))
    .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let mut terminal = Terminal::new(
            case["cols"].as_u64().unwrap() as usize,
            case["rows"].as_u64().unwrap() as usize,
        );
        for (step_number, step) in case["steps"].as_array().unwrap().iter().enumerate() {
            let events = match step["kind"].as_str().unwrap() {
                "feed" => step["text"]
                    .as_str()
                    .unwrap()
                    .as_bytes()
                    .chunks(3)
                    .flat_map(|bytes| terminal.feed(bytes))
                    .collect(),
                "resize" => {
                    terminal.resize(
                        step["cols"].as_u64().unwrap() as usize,
                        step["rows"].as_u64().unwrap() as usize,
                    );
                    Vec::new()
                }
                "select_start" => {
                    terminal.select_start(
                        step["col"].as_u64().unwrap() as usize,
                        step["row"].as_u64().unwrap() as usize,
                        match step["snap"].as_i64().unwrap() {
                            1 => SelectionSnap::Word,
                            2 => SelectionSnap::Line,
                            _ => SelectionSnap::None,
                        },
                    );
                    Vec::new()
                }
                "select_extend" => {
                    terminal.select_extend(
                        step["col"].as_u64().unwrap() as usize,
                        step["row"].as_u64().unwrap() as usize,
                        step["rectangular"].as_bool().unwrap(),
                        step["done"].as_bool().unwrap(),
                    );
                    Vec::new()
                }
                kind => panic!("unknown fixture operation {kind}"),
            };
            let expected = &step["expected"];
            let native_color = |color: TerminalColor| match color {
                TerminalColor::Indexed(index) => index as u32,
                TerminalColor::Rgb([r, g, b]) => {
                    (1 << 24) | (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)
                }
            };
            for (row, cells) in expected["cells"].as_array().unwrap().iter().enumerate() {
                for (col, cell) in cells.as_array().unwrap().iter().enumerate() {
                    let actual = terminal.cell(row, col);
                    let actual = [
                        actual.character as u32,
                        u32::from(actual.mode),
                        native_color(actual.fg),
                        native_color(actual.bg),
                    ];
                    let expected: Vec<u32> = cell
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|v| v.as_u64().unwrap() as u32)
                        .collect();
                    assert_eq!(
                        actual.as_slice(),
                        expected,
                        "{name} step {step_number} cell ({col},{row})"
                    );
                }
            }
            assert_eq!(
                [terminal.cursor().col, terminal.cursor().row],
                [
                    expected["cursor"][0].as_u64().unwrap() as usize,
                    expected["cursor"][1].as_u64().unwrap() as usize
                ],
                "{name} step {step_number} cursor"
            );
            assert_eq!(
                terminal.wrap_pending(),
                expected["wrap_pending"].as_bool().unwrap(),
                "{name} step {step_number} pending wrap"
            );
            assert_eq!(
                terminal.cursor().shape,
                expected["cursor_shape"].as_u64().unwrap() as u8,
                "{name} step {step_number} cursor style"
            );
            assert_eq!(
                terminal.cursor().blinking,
                expected["cursor_blinking"].as_bool().unwrap(),
                "{name} step {step_number} blinking cursor"
            );
            assert_eq!(
                terminal.title(),
                expected["title"].as_str().unwrap(),
                "{name} step {step_number} title"
            );
            assert_eq!(
                terminal.selection_text().as_deref(),
                expected["selection"].as_str(),
                "{name} step {step_number} selection"
            );
            let modes = terminal.modes();
            let mode = 3
                | (u32::from(modes.app_keypad) << 2)
                | (u32::from(modes.mouse_button) << 3)
                | (u32::from(modes.mouse_motion) << 4)
                | (u32::from(modes.reverse) << 5)
                | (u32::from(modes.keyboard_lock) << 6)
                | (u32::from(!terminal.cursor().visible) << 7)
                | (u32::from(modes.app_cursor) << 8)
                | (u32::from(modes.mouse_sgr) << 9)
                | (u32::from(modes.eight_bit) << 10)
                | (u32::from(modes.focus_reporting) << 13)
                | (u32::from(modes.mouse_x10) << 14)
                | (u32::from(modes.mouse_many) << 15)
                | (u32::from(modes.bracket_paste) << 16)
                | (u32::from(modes.num_lock) << 17);
            assert_eq!(
                mode,
                expected["win_mode"].as_u64().unwrap() as u32,
                "{name} step {step_number} window modes"
            );
            assert_eq!(
                modes.alt_screen,
                expected["term_mode"].as_u64().unwrap() & 4 != 0,
                "{name} step {step_number} alternate screen"
            );
            for (index, color) in expected["palette"].as_array().unwrap().iter().enumerate() {
                let color: Vec<u8> = color
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_u64().unwrap() as u8)
                    .collect();
                assert_eq!(
                    terminal.palette()[index].as_slice(),
                    color,
                    "{name} step {step_number} palette {index}"
                );
            }
            let actual_replies: Vec<u8> = events
                .into_iter()
                .filter_map(|event| {
                    if let TerminalEvent::Write(bytes) = event {
                        Some(bytes)
                    } else {
                        None
                    }
                })
                .flatten()
                .collect();
            let expected_replies: Vec<u8> = expected["replies"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_u64().unwrap() as u8)
                .collect();
            assert_eq!(
                actual_replies, expected_replies,
                "{name} step {step_number} PTY replies"
            );
        }
    }
}
