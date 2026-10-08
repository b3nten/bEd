//! Scrollback keeps shell output separate from the live parser screen.
use bed_terminal::terminal::{SelectionSnap, Terminal};

fn numbered_lines(terminal: &mut Terminal, count: usize) {
    let output = (0..count)
        .map(|number| format!("{number:05}"))
        .collect::<Vec<_>>()
        .join("\r\n");
    terminal.feed(output.as_bytes());
}

fn displayed_row(terminal: &Terminal, row: usize) -> String {
    (0..terminal.cols())
        .map(|col| terminal.display_cell(row, col).character)
        .collect::<String>()
        .trim_end_matches(' ')
        .to_owned()
}

fn displayed_rows(terminal: &Terminal) -> Vec<String> {
    (0..terminal.rows())
        .map(|row| displayed_row(terminal, row))
        .collect()
}

fn live_rows(terminal: &Terminal) -> Vec<String> {
    (0..terminal.rows())
        .map(|row| {
            (0..terminal.cols())
                .map(|col| terminal.cell(row, col).character)
                .collect::<String>()
                .trim_end_matches(' ')
                .to_owned()
        })
        .collect()
}

#[test]
fn scrolling_reveals_retained_glyphs_and_invalidates_only_when_the_viewport_moves() {
    let mut terminal = Terminal::new(12, 3);
    numbered_lines(&mut terminal, 8);
    assert_eq!(displayed_rows(&terminal), ["00005", "00006", "00007"]);
    assert_eq!(terminal.display_offset(), 0);
    let revision = terminal.revision();
    terminal.scroll_display(3);
    assert_eq!(terminal.display_offset(), 3);
    assert_eq!(displayed_rows(&terminal), ["00002", "00003", "00004"]);
    assert_eq!(live_rows(&terminal), ["00005", "00006", "00007"]);
    assert!(terminal.revision() > revision);

    terminal.scroll_display(100);
    assert_eq!(terminal.display_offset(), 5);
    assert_eq!(displayed_rows(&terminal), ["00000", "00001", "00002"]);
    let revision = terminal.revision();
    terminal.scroll_display(100);
    assert_eq!(terminal.revision(), revision);
    terminal.scroll_display(-100);
    assert_eq!(terminal.display_offset(), 0);
    assert_eq!(displayed_rows(&terminal), live_rows(&terminal));
}

#[test]
fn history_is_bounded_to_the_most_recent_ten_thousand_scrolled_rows() {
    let mut terminal = Terminal::new(8, 3);
    numbered_lines(&mut terminal, 10_020);
    terminal.scroll_display(i32::MAX);
    assert_eq!(terminal.display_offset(), 10_000);
    assert_eq!(displayed_rows(&terminal), ["00017", "00018", "00019"]);
    terminal.scroll_display(-i32::MAX);
    assert_eq!(displayed_rows(&terminal), ["10017", "10018", "10019"]);
}

#[test]
fn new_output_keeps_a_scrolled_viewport_pinned_until_the_user_returns_to_the_bottom() {
    let mut terminal = Terminal::new(12, 3);
    numbered_lines(&mut terminal, 8);
    terminal.scroll_display(3);
    let pinned = displayed_rows(&terminal);
    terminal.feed(b"\r\n00008\r\n00009");
    assert_eq!(terminal.display_offset(), 5);
    assert_eq!(displayed_rows(&terminal), pinned);
    assert_eq!(live_rows(&terminal), ["00007", "00008", "00009"]);
    terminal.scroll_display(-100);
    assert_eq!(displayed_rows(&terminal), ["00007", "00008", "00009"]);
    terminal.feed(b"\r\n00010");
    assert_eq!(terminal.display_offset(), 0);
    assert_eq!(displayed_rows(&terminal), ["00008", "00009", "00010"]);
}

#[test]
fn selection_copies_the_historical_rows_under_the_visible_viewport_coordinates() {
    let mut terminal = Terminal::new(12, 3);
    numbered_lines(&mut terminal, 8);
    terminal.scroll_display(4);
    assert_eq!(displayed_rows(&terminal), ["00001", "00002", "00003"]);
    terminal.select_start(0, 0, SelectionSnap::None);
    terminal.select_extend(4, 1, false, false);
    terminal.select_extend(4, 1, false, true);
    assert_eq!(terminal.selection_text().as_deref(), Some("00001\n00002"));
    assert!(terminal.is_selected(0, 0));
    assert!(terminal.is_selected(4, 1));
    assert!(!terminal.is_selected(5, 1));
    assert_eq!(live_rows(&terminal), ["00005", "00006", "00007"]);

    terminal.scroll_display(-1);
    assert_eq!(terminal.selection_text(), None);
    assert!(!terminal.is_selected(0, 0));
    terminal.select_start(0, 0, SelectionSnap::Word);
    assert_eq!(terminal.selection_text().as_deref(), Some("00002"));
    terminal.scroll_display(0);
    assert_eq!(terminal.selection_text().as_deref(), Some("00002"));
    let pinned = displayed_rows(&terminal);
    terminal.feed(b"x");
    assert_eq!(displayed_rows(&terminal), pinned);
    assert_eq!(terminal.selection_text(), None);
    assert!(!terminal.is_selected(0, 0));
}

#[test]
fn resizing_keeps_history_and_the_viewport_anchor_without_reflowing_truncated_columns() {
    let mut terminal = Terminal::new(12, 3);
    let output = (0..8)
        .map(|number| format!("{number:05}-tail"))
        .collect::<Vec<_>>()
        .join("\r\n");
    terminal.feed(output.as_bytes());
    terminal.scroll_display(3);
    assert_eq!(displayed_row(&terminal, 0), "00002-tail");
    terminal.resize(5, 3);
    assert_eq!(terminal.display_offset(), 3);
    assert_eq!(displayed_rows(&terminal), ["00002", "00003", "00004"]);
    terminal.resize(12, 5);
    assert_eq!(terminal.display_offset(), 3);
    assert_eq!(
        displayed_rows(&terminal),
        ["00002", "00003", "00004", "00005", "00006"]
    );
    // The cursor is on live row 2. Shrinking to two rows archives the
    // overflowing live row and increases the browsing offset by one.
    terminal.resize(12, 2);
    assert_eq!(terminal.display_offset(), 4);
    assert_eq!(displayed_rows(&terminal), ["00002", "00003"]);
    assert_eq!(live_rows(&terminal), ["00006", "00007"]);
    terminal.scroll_display(100);
    assert_eq!(displayed_row(&terminal, 0), "00000");
    assert_eq!(displayed_row(&terminal, 1), "00001");
}

#[test]
fn alternate_screen_output_and_history_erasure_leave_primary_history_and_offset_intact() {
    let mut terminal = Terminal::new(12, 3);
    numbered_lines(&mut terminal, 8);
    terminal.scroll_display(3);
    let primary = displayed_rows(&terminal);
    terminal.feed(b"\x1b[?1049h\x1b[Halt-0\r\nalt-1\r\nalt-2\r\nalt-3");
    assert!(terminal.modes().alt_screen);
    assert_eq!(terminal.display_offset(), 0);
    assert_eq!(displayed_rows(&terminal), ["alt-1", "alt-2", "alt-3"]);
    terminal.scroll_display(100);
    assert_eq!(terminal.display_offset(), 0);
    terminal.feed(b"\x1b[3J\x1b[?1049l");
    assert!(!terminal.modes().alt_screen);
    assert_eq!(terminal.display_offset(), 3);
    assert_eq!(displayed_rows(&terminal), primary);
    terminal.scroll_display(100);
    assert_eq!(displayed_row(&terminal, 0), "00000");
}

#[test]
fn erase_saved_lines_clears_history_and_returns_to_the_unchanged_live_screen() {
    let mut terminal = Terminal::new(12, 3);
    numbered_lines(&mut terminal, 8);
    let live = live_rows(&terminal);
    terminal.scroll_display(3);
    let revision = terminal.revision();
    terminal.feed(b"\x1b[3J");
    assert!(terminal.revision() > revision);
    assert_eq!(terminal.display_offset(), 0);
    assert_eq!(displayed_rows(&terminal), live);
    terminal.scroll_display(100);
    assert_eq!(terminal.display_offset(), 0);
    terminal.feed(b"\r\n00008");
    terminal.scroll_display(100);
    assert_eq!(terminal.display_offset(), 1);
    assert_eq!(displayed_rows(&terminal), ["00005", "00006", "00007"]);
}

#[test]
fn reset_from_a_resized_alternate_screen_clears_history_and_restores_primary_capacity() {
    let mut terminal = Terminal::new(12, 3);
    numbered_lines(&mut terminal, 8);
    terminal.scroll_display(3);
    terminal.feed(b"\x1b[?1049h\x1b[Halt-0\r\nalt-1\r\nalt-2");
    terminal.resize(8, 2);
    terminal.feed(b"\x1bc");
    assert!(!terminal.modes().alt_screen);
    assert_eq!((terminal.cols(), terminal.rows()), (8, 2));
    assert_eq!(terminal.history_size(), 0);
    assert_eq!(terminal.display_offset(), 0);
    assert_eq!(displayed_rows(&terminal), ["", ""]);
    terminal.scroll_display(100);
    assert_eq!(terminal.display_offset(), 0);

    numbered_lines(&mut terminal, 6);
    terminal.scroll_display(100);
    assert_eq!(terminal.display_offset(), 4);
    assert_eq!(displayed_rows(&terminal), ["00000", "00001"]);

    terminal.feed(b"\x1bc");
    numbered_lines(&mut terminal, 10_020);
    terminal.scroll_display(i32::MAX);
    assert_eq!(terminal.history_size(), 10_000);
    assert_eq!(terminal.display_offset(), 10_000);
    assert_eq!(displayed_rows(&terminal), ["00018", "00019"]);
}
