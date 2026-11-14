use super::*;
use crate::editor::tests::{focus, frame, key};
use std::sync::atomic::AtomicBool;

fn at(view: &mut EditorView, doc: &MemoryTextSession, anchor: usize, head: usize) {
    view.selection = TextSelection::new(doc.text(), anchor, head).unwrap();
}
fn settle(
    ctx: &egui::Context,
    view: &mut EditorView,
    doc: &mut MemoryTextSession,
    id: egui::Id,
    ready: impl Fn(&EditorView) -> bool,
) -> EditorOutput {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        let output = frame(ctx, view, doc, id, vec![]);
        if ready(view) {
            return output;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "background edit did not settle"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}
#[test]
fn whitespace_edits_preserve_crlf_indent_selection_and_shared_undo() {
    let ctx = egui::Context::default();
    let id = egui::Id::new("whitespace");
    let mut doc = MemoryTextSession::new("    {}\r\nnext");
    let mut view = EditorView::default();
    focus(&ctx, &mut view, &mut doc, id);
    at(&mut view, &doc, 5, 5);
    frame(
        &ctx,
        &mut view,
        &mut doc,
        id,
        vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
    );
    assert_eq!(doc.text(), "    {\r\n        \r\n    }\r\nnext");
    assert_eq!(view.selection.head(), 15);
    frame(
        &ctx,
        &mut view,
        &mut doc,
        id,
        vec![key(egui::Key::Backspace, egui::Modifiers::NONE)],
    );
    assert_eq!(doc.text(), "    {\r\n    \r\n    }\r\nnext");
    doc.history(HistoryAction::Undo).unwrap();
    assert_eq!(doc.text(), "    {\r\n        \r\n    }\r\nnext");
    doc.history(HistoryAction::Undo).unwrap();
    assert_eq!(doc.text(), "    {}\r\nnext");

    let mut doc = MemoryTextSession::new("a\r\n\tb\r\nc");
    let mut view = EditorView::default();
    focus(&ctx, &mut view, &mut doc, id);
    at(&mut view, &doc, 7, 0); // exclusive start of third line
    frame(
        &ctx,
        &mut view,
        &mut doc,
        id,
        vec![key(egui::Key::Tab, egui::Modifiers::NONE)],
    );
    assert_eq!(doc.text(), "    a\r\n    \tb\r\nc");
    assert!(view.selection.anchor() > view.selection.head());
    frame(
        &ctx,
        &mut view,
        &mut doc,
        id,
        vec![key(egui::Key::Tab, egui::Modifiers::SHIFT)],
    );
    assert_eq!(doc.text(), "a\r\n\tb\r\nc");
    assert_eq!(view.selection.range(), 0..7);
    doc.history(HistoryAction::Undo).unwrap();
    assert_eq!(doc.text(), "    a\r\n    \tb\r\nc");
}

#[test]
fn tab_stops_home_and_plain_indentation_settings_work_without_language_tools() {
    let ctx = egui::Context::default();
    let id = egui::Id::new("stops");
    let mut doc = MemoryTextSession::new(" \t  word");
    let mut view = EditorView::default();
    focus(&ctx, &mut view, &mut doc, id);
    at(&mut view, &doc, 4, 4);
    frame(
        &ctx,
        &mut view,
        &mut doc,
        id,
        vec![key(egui::Key::Backspace, egui::Modifiers::NONE)],
    );
    assert_eq!(doc.text(), " \tword");
    frame(
        &ctx,
        &mut view,
        &mut doc,
        id,
        vec![key(egui::Key::Backspace, egui::Modifiers::NONE)],
    );
    assert_eq!(doc.text(), "word");
    view.settings.indentation = Indentation::TwoSpaces;
    frame(
        &ctx,
        &mut view,
        &mut doc,
        id,
        vec![key(egui::Key::Tab, egui::Modifiers::NONE)],
    );
    assert_eq!(doc.text(), "  word");
    frame(
        &ctx,
        &mut view,
        &mut doc,
        id,
        vec![key(egui::Key::Home, egui::Modifiers::NONE)],
    );
    assert_eq!(view.selection.head(), 0);
    frame(
        &ctx,
        &mut view,
        &mut doc,
        id,
        vec![key(egui::Key::Home, egui::Modifiers::NONE)],
    );
    assert_eq!(view.selection.head(), 2);
    view.settings.smart_indent = false;
    frame(
        &ctx,
        &mut view,
        &mut doc,
        id,
        vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
    );
    assert_eq!(doc.text(), "  \nword");
}

#[test]
fn literal_search_spans_rope_leaves_and_handles_case_whole_words_and_crlf() {
    let source = format!(
        "{}café\r\nCAFÉ caféine café\r\nİ i\u{307} Σ σ",
        "x".repeat(4093)
    );
    let snapshot = TextSnapshot {
        revision: 9,
        text: source.as_str().into(),
    };
    let query = SearchQuery {
        text: "café".into(),
        match_case: false,
        whole_word: true,
    };
    let result =
        search::Results::find(&snapshot, query.clone(), 0, &AtomicBool::new(false)).unwrap();
    assert_eq!(result.count, 2); // first is part of the x... word
    assert_eq!(snapshot.text.range_text(result.ranges[0].clone()), "CAFÉ");
    let (replacement, count) =
        search::replace_all(&snapshot, &query, "🙂\n", &AtomicBool::new(false))
            .unwrap()
            .unwrap();
    assert_eq!(count, 2);
    assert_eq!(
        replacement,
        source
            .replace("CAFÉ", "🙂\n")
            .replace("café\r\nİ", "🙂\n\r\nİ")
            .as_str()
    );
    let result = search::Results::find(
        &snapshot,
        SearchQuery {
            text: "i\u{307}".into(),
            ..Default::default()
        },
        0,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(result.count, 2);
    let result = search::Results::find(
        &snapshot,
        SearchQuery {
            text: "σ".into(),
            ..Default::default()
        },
        0,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(result.count, 2);
    assert!(search::Results::find(&snapshot, query, 0, &AtomicBool::new(true)).is_none());
}

#[test]
fn search_navigation_wraps_beyond_the_match_cache_and_replace_all_is_one_undo() {
    let snapshot = TextSnapshot {
        revision: 0,
        text: "a ".repeat(10_100).into(),
    };
    let result = search::Results::find(
        &snapshot,
        SearchQuery {
            text: "a".into(),
            ..Default::default()
        },
        20_100,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(result.count, 10_100);
    assert_eq!(result.at(20_100, true), Some(20_100..20_101));
    assert_eq!(result.at(20_100, false), Some(20_098..20_099));
    let mut doc = MemoryTextSession::new(snapshot.text.clone());
    let (replacement, _) =
        search::replace_all(&snapshot, &result.query, "🙂", &AtomicBool::new(false))
            .unwrap()
            .unwrap();
    doc.edit_buffer(0, BufferEdit::new(0..doc.text().len(), replacement))
        .unwrap();
    assert_eq!(doc.text().len(), "🙂 ".len() * 10_100);
    doc.history(HistoryAction::Undo).unwrap();
    assert!(doc.text().ptr_eq(&snapshot.text));
    doc.history(HistoryAction::Redo).unwrap();
    assert_eq!(doc.text().len(), "🙂 ".len() * 10_100);
}

#[test]
fn wrapped_rows_share_caret_hit_testing_and_visual_arrow_navigation() {
    let ctx = egui::Context::default();
    let id = egui::Id::new("wrapped");
    let mut doc = MemoryTextSession::new(format!("    {}\nlast", "abcdef ".repeat(50)));
    let mut view = EditorView::default();
    view.settings.wrap = true;
    focus(&ctx, &mut view, &mut doc, id);
    settle(&ctx, &mut view, &mut doc, id, |v| v.wrap_active);
    let index = view.wrapping.index.as_ref().unwrap();
    let continuation = index.segment(1);
    assert_eq!(continuation.indent, 4);
    let expected = doc.text().byte_at_column(0, continuation.columns.start + 2);
    at(&mut view, &doc, 6, 6);
    frame(
        &ctx,
        &mut view,
        &mut doc,
        id,
        vec![key(egui::Key::ArrowDown, egui::Modifiers::NONE)],
    );
    assert_eq!(view.selection.head(), expected);
    let output = frame(&ctx, &mut view, &mut doc, id, vec![]);
    let click = output.caret_rect.center();
    frame(
        &ctx,
        &mut view,
        &mut doc,
        id,
        vec![
            egui::Event::PointerMoved(click),
            egui::Event::PointerButton {
                pos: click,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ],
    );
    assert_eq!(view.selection.head(), expected);
    frame(
        &ctx,
        &mut view,
        &mut doc,
        id,
        vec![egui::Event::Text("!".into())],
    );
    assert!(view.wrap_active);
    assert_eq!(doc.text().range_text(expected..expected + 1), "!");
}

#[test]
fn wrapping_updates_multiple_journal_edits_without_losing_lines_or_graphemes() {
    let mut snapshot = TextSnapshot {
        revision: 0,
        text: ("    café🙂 words and more words\r\n".repeat(600)).into(),
    };
    let mut index =
        wrapping::WrapIndex::build(snapshot.clone(), 18, &AtomicBool::new(false)).unwrap();
    for step in 0..80 {
        let mut changes = Vec::new();
        for n in 0..3 {
            let start = snapshot
                .text
                .clamp_boundary((step * 167 + n * 531) % snapshot.text.len());
            let end = snapshot.text.next_grapheme(start);
            let insert = if step % 3 == 0 { "x\r\n  " } else { "🙂" };
            BufferEdit::new(start..end, insert)
                .apply(&mut snapshot.text)
                .unwrap();
            snapshot.revision += 1;
            changes.push(TextDelta {
                range: start..end,
                inserted_bytes: insert.len(),
            });
        }
        assert!(index.update(snapshot.clone(), &changes));
        let rebuilt =
            wrapping::WrapIndex::build(snapshot.clone(), 18, &AtomicBool::new(false)).unwrap();
        assert_eq!(index.rows(), rebuilt.rows());
        for row in 0..index.rows() {
            let a = index.segment(row);
            let b = rebuilt.segment(row);
            assert_eq!(a.bytes, b.bytes);
            if row > 0 && a.line == index.segment(row - 1).line {
                assert_eq!(index.segment(row - 1).bytes.end, a.bytes.start);
            }
        }
    }
}

#[test]
fn find_shortcut_owns_its_input_and_escape_returns_to_editor() {
    let ctx = egui::Context::default();
    let id = egui::Id::new("find-input");
    let mut doc = MemoryTextSession::new("hello world hello");
    let mut view = EditorView::default();
    focus(&ctx, &mut view, &mut doc, id);
    frame(
        &ctx,
        &mut view,
        &mut doc,
        id,
        vec![key(egui::Key::F, egui::Modifiers::COMMAND)],
    );
    assert!(view.search.open);
    frame(
        &ctx,
        &mut view,
        &mut doc,
        id,
        vec![egui::Event::Text("hello".into())],
    );
    settle(&ctx, &mut view, &mut doc, id, |v| {
        v.search.results.is_some()
    });
    assert_eq!(doc.text(), "hello world hello");
    assert_eq!(view.search.results.as_ref().unwrap().count, 2);
    assert_eq!(view.selection.range(), 0..5);
    frame(
        &ctx,
        &mut view,
        &mut doc,
        id,
        vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
    );
    assert_eq!(view.selection.range(), 12..17);
    frame(
        &ctx,
        &mut view,
        &mut doc,
        id,
        vec![key(egui::Key::Escape, egui::Modifiers::NONE)],
    );
    assert!(!view.search.open);
    frame(
        &ctx,
        &mut view,
        &mut doc,
        id,
        vec![egui::Event::Text("🙂".into())],
    );
    assert_eq!(doc.text(), "hello world 🙂");
}

#[test]
fn stale_background_replacement_is_rejected_and_noop_replacement_reuses_text() {
    let ctx = egui::Context::default();
    let id = egui::Id::new("stale-replace");
    let mut doc = MemoryTextSession::new("one one");
    let original = doc.snapshot().unwrap();
    let query = SearchQuery {
        text: "one".into(),
        match_case: true,
        whole_word: false,
    };
    let (same, count) = search::replace_all(&original, &query, "one", &AtomicBool::new(false))
        .unwrap()
        .unwrap();
    assert_eq!(count, 2);
    assert!(same.ptr_eq(&original.text));
    let mut view = EditorView::default();
    focus(&ctx, &mut view, &mut doc, id);
    view.search.open = true;
    view.search.query = query.clone();
    view.search.replacement_job = Some((
        0,
        background::Job::start(&ctx, move |cancel| {
            search::replace_all(&original, &query, "two", cancel)
        }),
    ));
    doc.edit(
        0,
        TextEdit {
            range: 0..0,
            replacement: "changed ".into(),
        },
    )
    .unwrap();
    settle(&ctx, &mut view, &mut doc, id, |v| {
        v.search.replacement_job.is_none()
    });
    assert_eq!(doc.text(), "changed one one");
    assert!(
        view.search
            .message
            .as_ref()
            .unwrap()
            .contains("not applied")
    );
    doc.history(HistoryAction::Undo).unwrap();
    assert_eq!(doc.text(), "one one");
}

#[test]
fn wrapped_end_has_upstream_affinity_and_unicode_highlights_keep_global_alignment() {
    let ctx = egui::Context::default();
    let id = egui::Id::new("wrap-end");
    let mut doc = MemoryTextSession::new("    a🙂é ".repeat(50));
    let mut view = EditorView::default();
    view.settings.wrap = true;
    focus(&ctx, &mut view, &mut doc, id);
    settle(&ctx, &mut view, &mut doc, id, |v| v.wrap_active);
    let output = frame(
        &ctx,
        &mut view,
        &mut doc,
        id,
        vec![key(egui::Key::End, egui::Modifiers::NONE)],
    );
    assert_eq!(
        view.selection.head(),
        view.wrapping.index.as_ref().unwrap().segment(0).bytes.end
    );
    assert!(view.upstream);
    let y = output.caret_rect.top();
    let output = frame(
        &ctx,
        &mut view,
        &mut doc,
        id,
        vec![key(egui::Key::ArrowRight, egui::Modifiers::NONE)],
    );
    assert!(output.caret_rect.top() > y);

    let snapshot = TextSnapshot {
        revision: 0,
        text: "a".repeat(50_000).into(),
    };
    let result = search::Results::find(
        &snapshot,
        SearchQuery {
            text: "aaa".into(),
            match_case: true,
            whole_word: false,
        },
        0,
        &AtomicBool::new(false),
    )
    .unwrap();
    let visible = search::visible_matches(&snapshot.text, &result, 40_003..40_103);
    assert!(!visible.is_empty());
    for range in visible {
        assert_eq!(range.start % 3, 0);
        assert_eq!(range.len(), 3);
    }
}

#[test]
fn large_selected_indentation_runs_as_one_transaction_and_preserves_crlf_metrics() {
    let ctx = egui::Context::default();
    let id = egui::Id::new("large-indent");
    let mut doc = MemoryTextSession::new(" \tcafé🙂\r\n".repeat(25_000));
    let original = doc.snapshot().unwrap();
    let mut view = EditorView::default();
    focus(&ctx, &mut view, &mut doc, id);
    let end = doc.text().len();
    at(&mut view, &doc, 0, end);
    frame(
        &ctx,
        &mut view,
        &mut doc,
        id,
        vec![key(egui::Key::Tab, egui::Modifiers::NONE)],
    );
    assert!(view.indent_job.is_some());
    settle(&ctx, &mut view, &mut doc, id, |v| v.indent_job.is_none());
    assert_eq!(doc.text().line_count(), 25_001);
    assert_eq!(doc.text().line_start(25_000), original.text.len() + 100_000);
    assert_eq!(doc.text().line_range(24_999).len(), "     \tcafé🙂".len());
    assert_eq!(view.selection.range().end, doc.text().len());
    doc.history(HistoryAction::Undo).unwrap();
    assert!(doc.text().ptr_eq(&original.text));
}

#[test]
#[ignore = "Explicit 107 MiB / million-line wrapped editing and streaming search fixture"]
fn million_line_wrapping_and_search_keep_editing_bounded() {
    let ctx = egui::Context::default();
    let id = egui::Id::new("million-wrapped");
    let mut doc = MemoryTextSession::new(format!("    {}\n", "x".repeat(107)).repeat(1_000_000));
    assert_eq!(doc.text().len(), 112_000_000);
    let mut view = EditorView::default();
    view.settings.wrap = true;
    focus(&ctx, &mut view, &mut doc, id);
    let started = std::time::Instant::now();
    settle(&ctx, &mut view, &mut doc, id, |v| v.wrap_active);
    let build = started.elapsed();
    let snapshot = doc.snapshot().unwrap();
    let target = snapshot.text.line_start(999_999) + 6;
    view.request_navigation(
        &snapshot,
        Navigation {
            revision: snapshot.revision,
            range: target..target,
            selection: Some(TextSelection::caret(&snapshot.text, target).unwrap()),
            reveal: Scroll::Center,
            focus: true,
        },
    )
    .unwrap();
    frame(&ctx, &mut view, &mut doc, id, vec![]);
    let started = std::time::Instant::now();
    let output = frame(
        &ctx,
        &mut view,
        &mut doc,
        id,
        vec![egui::Event::Text("!".into())],
    );
    let edit = started.elapsed();
    assert!(output.rendered_lines < 64 && output.laid_out_bytes < 16 * 1024);
    assert!(view.wrap_active);
    assert_eq!(doc.text().range_text(target..target + 1), "!");
    view.search.open = true;
    view.search.query = SearchQuery {
        text: "!".into(),
        ..Default::default()
    };
    let started = std::time::Instant::now();
    let output = settle(&ctx, &mut view, &mut doc, id, |v| {
        v.search.results.is_some()
    });
    let search = started.elapsed();
    assert_eq!(view.search_match_count(), Some(1));
    assert!(output.rendered_lines < 64 && output.laid_out_bytes < 16 * 1024);
    eprintln!(
        "112 MB wrapped fixture: index {build:?}, incremental edit {edit:?}, async search {search:?}; {} rows, {} shaped bytes",
        output.rendered_lines, output.laid_out_bytes
    );
}

#[test]
fn smart_whitespace_handles_non_ascii_prefixes_and_huge_indentation_without_flattening() {
    let ctx = egui::Context::default();
    let id = egui::Id::new("unicode-indent");
    let mut view = EditorView::default();
    view.settings.indentation = Indentation::TwoSpaces;
    let mut doc = MemoryTextSession::new(" 🙂é\n🙂");
    focus(&ctx, &mut view, &mut doc, id);
    at(&mut view, &doc, 0, doc.text().len());
    frame(
        &ctx,
        &mut view,
        &mut doc,
        id,
        vec![key(egui::Key::Tab, egui::Modifiers::SHIFT)],
    );
    assert_eq!(doc.text(), "🙂é\n🙂");
    at(&mut view, &doc, 7, 7);
    frame(
        &ctx,
        &mut view,
        &mut doc,
        id,
        vec![egui::Event::Text("}".into())],
    );
    assert_eq!(doc.text(), "🙂é\n}🙂"); // a closing delimiter at column zero keeps the newline

    let mut doc = MemoryTextSession::new(" ".repeat(2_000_000));
    let mut view = EditorView::default();
    focus(&ctx, &mut view, &mut doc, id);
    at(&mut view, &doc, 2_000_000, 2_000_000);
    let output = frame(
        &ctx,
        &mut view,
        &mut doc,
        id,
        vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
    );
    assert_eq!(doc.text().len(), 2_000_001);
    assert_eq!(doc.text().line_start(1), 1);
    assert_eq!(doc.text().byte(0), Some(b'\n'));
    assert!(output.rendered_lines < 64 && output.laid_out_bytes < 16 * 1024);
}

#[test]
fn select_line_grows_by_whole_lines_and_keys_alone_run_no_commands() {
    let ctx = egui::Context::default();
    let id = egui::Id::new("select-line");
    let mut doc = MemoryTextSession::new("one\ntwo\r\nthree");
    let mut view = EditorView::default();
    focus(&ctx, &mut view, &mut doc, id);
    let press = |ctx: &egui::Context,
                 view: &mut EditorView,
                 doc: &mut MemoryTextSession,
                 _: egui::Modifiers| {
        view.queue_command(EditorCommand::SelectLine);
        frame(ctx, view, doc, id, vec![]).selection.range()
    };

    // The editor maps no keys to commands: the workbench delivers them.
    at(&mut view, &doc, 5, 5);
    let output = frame(
        &ctx,
        &mut view,
        &mut doc,
        id,
        vec![key(egui::Key::L, egui::Modifiers::COMMAND)],
    );
    assert_eq!(output.selection.range(), 5..5);

    // The caret's line with its CRLF, then the next, then no further.
    assert_eq!(
        press(&ctx, &mut view, &mut doc, egui::Modifiers::COMMAND),
        4..9
    );
    assert_eq!(view.selection().head(), 9);
    assert_eq!(
        press(&ctx, &mut view, &mut doc, egui::Modifiers::COMMAND),
        4..14
    );
    assert_eq!(
        press(&ctx, &mut view, &mut doc, egui::Modifiers::COMMAND),
        4..14
    );

    // A selection within lines grows to them whole, without another.
    at(&mut view, &doc, 1, 6);
    assert_eq!(
        press(&ctx, &mut view, &mut doc, egui::Modifiers::COMMAND),
        0..9
    );
    // One ending at a line start, from inside a line, keeps its end.
    at(&mut view, &doc, 2, 4);
    assert_eq!(
        press(&ctx, &mut view, &mut doc, egui::Modifiers::COMMAND),
        0..4
    );

    // From a menu: queued, then run on the next draw, read-only too.
    view.settings.read_only = true;
    at(&mut view, &doc, 12, 12);
    view.queue_command(EditorCommand::SelectLine);
    assert_eq!(
        frame(&ctx, &mut view, &mut doc, id, vec![])
            .selection
            .range(),
        9..14
    );
    assert_eq!(doc.text(), "one\ntwo\r\nthree");
}
