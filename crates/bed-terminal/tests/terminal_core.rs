//! Bed-authored terminal semantics and protocol regressions.
use bed_terminal::terminal::{
    ATTR_BOLD, ATTR_ITALIC, ATTR_REVERSE, ATTR_STRUCK, SelectionSnap, Terminal, TerminalColor,
    TerminalEvent, TerminalUnderline,
};
use std::sync::Arc;

fn row(terminal: &Terminal, number: usize) -> String {
    (0..terminal.cols())
        .map(|col| terminal.display_cell(number, col))
        .filter(|cell| cell.width != 0)
        .map(|cell| cell.text.to_string())
        .collect::<String>()
        .trim_end()
        .to_owned()
}

#[test]
fn arbitrary_read_boundaries_preserve_unicode_graphemes_and_columns() {
    let mut terminal = Terminal::new(20, 3);
    for byte in "Aé e\u{301} 👩\u{200d}💻界".as_bytes() {
        terminal.feed(&[*byte]);
    }
    assert_eq!(terminal.cell(0, 3).text.as_ref(), "e\u{301}");
    assert_eq!(terminal.cell(0, 3).width, 1);
    assert_eq!(terminal.cell(0, 5).text.as_ref(), "👩\u{200d}💻");
    assert_eq!(terminal.cell(0, 5).width, 2);
    assert_eq!(terminal.cell(0, 6).width, 0);
    assert_eq!(terminal.cell(0, 7).text.as_ref(), "界");
    assert_eq!(terminal.cursor().col, 9);
    assert_eq!(row(&terminal, 0), "Aé e\u{301} 👩\u{200d}💻界");
}

#[test]
fn styles_retain_rich_underlines_truecolor_and_attributes() {
    let mut terminal = Terminal::new(20, 3);
    terminal.feed(b"\x1b[1;3;7;9;4:3;38;2;18;52;86;58;2;210;160;110mX\x1b[0mY");
    let cell = terminal.cell(0, 0);
    assert_eq!(
        cell.mode & (ATTR_BOLD | ATTR_ITALIC | ATTR_REVERSE | ATTR_STRUCK),
        ATTR_BOLD | ATTR_ITALIC | ATTR_REVERSE | ATTR_STRUCK
    );
    assert_eq!(cell.fg, TerminalColor::Rgb([18, 52, 86]));
    assert_eq!(cell.underline, TerminalUnderline::Curly);
    assert_eq!(
        cell.underline_color,
        Some(TerminalColor::Rgb([210, 160, 110]))
    );
    assert_eq!(terminal.cell(0, 1).underline, TerminalUnderline::None);
}

#[test]
fn osc_links_survive_wraps_and_close_at_the_requested_boundary() {
    let mut terminal = Terminal::new(5, 3);
    terminal.feed(b"\x1b]8;id=docs;https://example.com/docs\x1b\\abcdef\x1b]8;;\x1b\\Z");
    for (row, col) in [(0, 0), (0, 4), (1, 0)] {
        assert_eq!(
            terminal.cell(row, col).hyperlink.as_deref(),
            Some("https://example.com/docs")
        );
    }
    assert!(terminal.cell(1, 1).hyperlink.is_none());
    assert!(terminal.snapshot().lines[0].wrapped);
}

#[test]
fn keyboard_negotiation_and_grapheme_override_are_owned_by_rio() {
    let mut terminal = Terminal::new(20, 3);
    assert!(terminal.modes().grapheme_clustering);
    let events = terminal.feed(b"\x1b[>3u\x1b[?u\x1b[>4;2m\x1b[?2027l");
    assert_eq!(terminal.modes().kitty_keyboard, 3);
    assert_eq!(terminal.modes().modify_other_keys, 2);
    assert!(!terminal.modes().grapheme_clustering);
    assert!(events.contains(&TerminalEvent::Write(b"\x1b[?3u".to_vec())));
    terminal.feed(b"\x1b[<u\x1bc");
    assert_eq!(terminal.modes().kitty_keyboard, 0);
    assert!(terminal.modes().grapheme_clustering);
}

#[test]
fn synchronized_updates_publish_together_and_final_output_can_be_flushed() {
    let mut terminal = Terminal::new(20, 3);
    terminal.feed(b"old");
    let original = terminal.snapshot();
    terminal.feed(b"\x1b[?2026h\rnew");
    assert!(terminal.sync_deadline().is_some());
    assert!(terminal.flush_sync().is_empty());
    let middle = terminal.snapshot();
    assert_eq!(original.lines[0].cells, middle.lines[0].cells);
    assert!(Arc::ptr_eq(&original.lines[0], &middle.lines[0]));
    terminal.feed(b"\x1b[?2026l");
    assert_eq!(row(&terminal, 0), "new");
    terminal.feed(b"\x1b[?2026h\rfinal");
    terminal.finish_output();
    assert_eq!(row(&terminal, 0), "final");
}

#[test]
fn resize_reflows_wrapped_output_instead_of_discarding_columns() {
    let mut terminal = Terminal::new(5, 4);
    terminal.feed(b"abcdefghij");
    terminal.resize(10, 4);
    assert_eq!(row(&terminal, 0), "abcdefghij");
    terminal.resize(3, 4);
    terminal.select_all();
    assert_eq!(terminal.selection_text().unwrap().trim_end(), "abcdefghij");
}

#[test]
fn snapshots_share_unchanged_rows_through_cursor_and_selection_changes() {
    let mut terminal = Terminal::new(20, 3);
    terminal.feed(b"one\r\ntwo");
    let original = terminal.snapshot();
    terminal.feed(b"\x1b[1;1H");
    let moved = terminal.snapshot();
    for row in 0..3 {
        assert!(Arc::ptr_eq(&original.lines[row], &moved.lines[row]));
    }
    terminal.select_start(0, 0, SelectionSnap::Word);
    let selected = terminal.snapshot();
    assert_eq!(selected.selection_text.as_deref(), Some("one"));
    assert_eq!(selected.selection[0].end, 3);
    for row in 0..3 {
        assert!(Arc::ptr_eq(&moved.lines[row], &selected.lines[row]));
    }
    terminal.feed(b"X");
    let changed = terminal.snapshot();
    assert!(!Arc::ptr_eq(&selected.lines[0], &changed.lines[0]));
    assert!(Arc::ptr_eq(&selected.lines[1], &changed.lines[1]));
    assert_eq!(original.lines[0].cells[0].character, 'o');
}

#[test]
fn selection_copies_whole_graphemes_and_select_all_includes_history() {
    let mut terminal = Terminal::new(12, 2);
    terminal.feed("e\u{301}👩\u{200d}💻X".as_bytes());
    terminal.select_start(0, 0, SelectionSnap::None);
    terminal.select_extend(2, 0, false, true);
    assert_eq!(
        terminal.selection_text().as_deref(),
        Some("e\u{301}👩\u{200d}💻")
    );
    terminal.feed(b"\r\nnext\r\nlast");
    terminal.select_all();
    assert_eq!(
        terminal.selection_text().as_deref(),
        Some("e\u{301}👩\u{200d}💻X\nnext\nlast\n")
    );
}

#[test]
fn kitty_images_preserve_rgba_and_do_not_reupload_on_cursor_move() {
    let mut terminal = Terminal::new(20, 3);
    terminal.feed(b"\x1b_Gf=32,s=1,v=1,i=7,a=T,q=2;/wAA/w==\x1b\\");
    let snapshot = terminal.snapshot();
    assert_eq!(snapshot.images.len(), 1);
    let image = &snapshot.images[0];
    assert_eq!(image.image.id, 7);
    assert_eq!(image.image.rgba.as_ref(), &[255, 0, 0, 255]);
    assert_eq!(image.source_rect, [0.0, 0.0, 1.0, 1.0]);
    terminal.feed(b"\x1b[1;2H");
    assert!(Arc::ptr_eq(
        &image.image,
        &terminal.snapshot().images[0].image
    ));
    terminal.feed(b"\x1b_Ga=d,d=I,i=7,q=2\x1b\\");
    assert!(terminal.snapshot().images.is_empty());
}

#[test]
fn oversized_header_is_rejected_before_decode_and_text_continues() {
    let mut terminal = Terminal::new(20, 3);
    let events = terminal.feed(b"\x1b_Gf=32,s=10000,v=10000,i=9,a=T;AAAA\x1b\\OK");
    assert!(events.iter().any(|event| matches!(event, TerminalEvent::Write(bytes) if bytes.windows(5).any(|window| window == b"E2BIG"))));
    assert!(terminal.snapshot().images.is_empty());
    assert_eq!(row(&terminal, 0), "OK");
}

#[test]
fn core_and_snapshot_can_cross_the_worker_boundary() {
    fn send<T: Send>() {}
    fn sync<T: Sync>() {}
    send::<Terminal>();
    send::<bed_terminal::terminal::TerminalSnapshot>();
    sync::<bed_terminal::terminal::TerminalSnapshot>();
}

#[test]
fn kitty_virtual_placeholders_draw_the_correct_image_slice() {
    let mut terminal = Terminal::new(20, 3);
    terminal.feed(b"\x1b_Gf=32,s=1,v=1,i=7,a=T,U=1,c=1,r=1,q=2;/wAA/w==\x1b\\");
    terminal.feed("\x1b[38;2;0;0;7m\u{10EEEE}\u{305}\u{305}".as_bytes());
    let snapshot = terminal.snapshot();
    assert_eq!(snapshot.images.len(), 1);
    assert_eq!(snapshot.images[0].image.id, 7);
    assert_eq!(snapshot.images[0].source_rect, [0.0, 0.0, 1.0, 1.0]);
}

#[test]
fn sixel_and_iterm2_images_use_the_same_immutable_pixel_path() {
    let mut sixel = Terminal::new(20, 3);
    sixel.feed(b"\x1bPq#0;2;100;0;0#0~\x1b\\");
    let image = sixel.snapshot();
    assert_eq!(image.images.len(), 1);
    assert_eq!(&image.images[0].image.rgba[..4], &[255, 0, 0, 255]);
    let mut iterm = Terminal::new(20, 3);
    // A 1x1 red RGBA PNG with valid chunk CRCs.
    iterm.feed(b"\x1b]1337;File=inline=1:iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg==\x07");
    assert_eq!(iterm.snapshot().images.len(), 1);
}

#[test]
fn reset_releases_retained_image_buffers_after_the_last_snapshot_is_consumed() {
    let mut terminal = Terminal::new(20, 3);
    terminal.feed(b"\x1b_Gf=32,s=1,v=1,i=7,a=T,q=2;/wAA/w==\x1b\\");
    let before = terminal.snapshot();
    let pixels = Arc::downgrade(&before.images[0].image);
    drop(before);
    terminal.feed(b"\x1bc");
    let reset = terminal.snapshot();
    assert!(reset.images.is_empty());
    assert!(pixels.upgrade().is_none());
}

#[test]
fn oversized_capability_queries_have_bounded_replies_and_resume_after_the_terminator() {
    let mut terminal = Terminal::new(20, 3);
    terminal.feed(b"\x1bP+q");
    for _ in 0..17 {
        assert!(terminal.feed(&[b'4'; 4096]).is_empty());
    }
    let events = terminal.feed(b"\x1b\\OK\x1bP+q544e\x1b\\");
    let replies: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            TerminalEvent::Write(bytes) => Some(bytes.as_slice()),
            _ => None,
        })
        .collect();
    assert_eq!(replies.len(), 2);
    assert_eq!(replies[0], b"\x1bP0+r\x1b\\");
    assert!(replies[1].starts_with(b"\x1bP1+r544E="));
    assert_eq!(row(&terminal, 0), "OK");
}

#[test]
fn capability_reply_expansion_fits_the_workers_read_watermark_headroom() {
    let mut terminal = Terminal::new(20, 3);
    // The sgr capability has a long parameterized value and a short query.
    let query = "736772;".repeat((64 * 1024) / 7);
    terminal.feed(b"\x1bP+q");
    for chunk in query.as_bytes().chunks(4096) {
        assert!(terminal.feed(chunk).is_empty());
    }
    let events = terminal.feed(b"\x1b\\");
    let reply_bytes: usize = events
        .iter()
        .filter_map(|event| match event {
            TerminalEvent::Write(bytes) => Some(bytes.len()),
            _ => None,
        })
        .sum();
    assert!(reply_bytes > 1024 * 1024);
    assert!(reply_bytes < 3 * 1024 * 1024);
}
