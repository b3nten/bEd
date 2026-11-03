use super::*;

pub(super) fn key(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    }
}
pub(crate) fn frame(
    ctx: &egui::Context,
    view: &mut EditorView,
    document: &mut dyn TextSession,
    id: egui::Id,
    events: Vec<egui::Event>,
) -> EditorOutput {
    ctx.options_mut(|options| options.max_passes = std::num::NonZeroUsize::new(1).unwrap());
    let time = ctx.cumulative_frame_nr() as f64 / 60.0;
    let mut result = None;
    let mut full_output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(600.0, 300.0),
            )),
            time: Some(time),
            focused: true,
            events,
            ..Default::default()
        },
        |ui| {
            result = Some(
                view.show(ui, id, document, MotionPolicy::default())
                    .unwrap(),
            );
        },
    );
    full_output.textures_delta.clear();
    result.unwrap()
}
pub(super) fn focus(
    ctx: &egui::Context,
    view: &mut EditorView,
    document: &mut dyn TextSession,
    id: egui::Id,
) {
    ctx.memory_mut(|memory| memory.request_focus(id));
    frame(ctx, view, document, id, vec![]);
}

#[test]
fn invalid_edits_are_atomic_and_history_uses_document_revisions() {
    let mut document = MemoryTextSession::new("aé🙂");
    assert!(
        document
            .edit(
                0,
                TextEdit {
                    range: 2..3,
                    replacement: "!".into()
                }
            )
            .is_err()
    );
    assert_eq!(document.text(), "aé🙂");
    assert_eq!(document.snapshot().unwrap().revision, 0);
    document
        .edit(
            0,
            TextEdit {
                range: 1..3,
                replacement: "e".into(),
            },
        )
        .unwrap();
    document
        .edit(
            1,
            TextEdit {
                range: 2..2,
                replacement: "!".into(),
            },
        )
        .unwrap();
    assert_eq!(document.text(), "ae!🙂");
    document.history(HistoryAction::Undo).unwrap();
    assert_eq!(document.text(), "aé🙂");
    document.history(HistoryAction::Redo).unwrap();
    assert_eq!(document.text(), "ae!🙂");
    assert_eq!(document.snapshot().unwrap().revision, 4);
    assert!(
        document
            .edit(
                1,
                TextEdit {
                    range: 0..0,
                    replacement: "stale".into()
                }
            )
            .is_err()
    );
}

#[test]
fn typing_undo_and_more_typing_in_one_pass_keep_input_order() {
    let ctx = egui::Context::default();
    let mut view = EditorView::default();
    let mut document = MemoryTextSession::new("hello");
    let id = egui::Id::new("editor");
    focus(&ctx, &mut view, &mut document, id);
    frame(
        &ctx,
        &mut view,
        &mut document,
        id,
        vec![
            egui::Event::Text("A".into()),
            key(egui::Key::Z, egui::Modifiers::COMMAND),
            egui::Event::Text("B".into()),
        ],
    );
    assert_eq!(document.text(), "Bhello");
    assert_eq!(view.selection().head(), 1);
    frame(
        &ctx,
        &mut view,
        &mut document,
        id,
        vec![key(egui::Key::Z, egui::Modifiers::COMMAND)],
    );
    assert_eq!(document.text(), "hello");
    frame(
        &ctx,
        &mut view,
        &mut document,
        id,
        vec![key(
            egui::Key::Z,
            egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
        )],
    );
    assert_eq!(document.text(), "Bhello");
}

#[test]
fn shared_views_map_selections_through_each_edit_not_a_combined_replacement() {
    let ctx = egui::Context::default();
    let mut document = MemoryTextSession::new("abcdef");
    let mut first = EditorView::default();
    let mut second = EditorView::default();
    second.selection = TextSelection::caret(document.text(), 3).unwrap();
    let a = egui::Id::new("a");
    let b = egui::Id::new("b");
    frame(&ctx, &mut second, &mut document, b, vec![]);
    focus(&ctx, &mut first, &mut document, a);
    frame(
        &ctx,
        &mut first,
        &mut document,
        a,
        vec![egui::Event::Text("L".into())],
    );
    document
        .edit(
            1,
            TextEdit {
                range: 7..7,
                replacement: "R".into(),
            },
        )
        .unwrap();
    frame(&ctx, &mut second, &mut document, b, vec![]);
    assert_eq!(document.text(), "LabcdefR");
    assert_eq!(second.selection().head(), 4);
    assert_eq!(first.selection().head(), 1);
    focus(&ctx, &mut second, &mut document, b);
    frame(
        &ctx,
        &mut second,
        &mut document,
        b,
        vec![key(egui::Key::Z, egui::Modifiers::COMMAND)],
    );
    assert_eq!(document.text(), "abcdef");
    assert_eq!(second.selection().head(), 3);
}

#[test]
fn external_reveal_scrolls_without_moving_selection_or_claiming_focus() {
    let ctx = egui::Context::default();
    let mut view = EditorView::default();
    let mut document = MemoryTextSession::new(
        (0..200)
            .map(|line| format!("Line {line}\n"))
            .collect::<String>(),
    );
    let id = egui::Id::new("reveal");
    frame(&ctx, &mut view, &mut document, id, vec![]);
    let snapshot = document.snapshot().unwrap();
    let target = snapshot.text.line_start(180);
    view.request_navigation(
        &snapshot,
        Navigation {
            revision: snapshot.revision,
            range: target..target,
            selection: None,
            reveal: Reveal::Center,
            focus: false,
        },
    )
    .unwrap();
    for _ in 0..45 {
        frame(&ctx, &mut view, &mut document, id, vec![]);
    }
    assert!(view.scroll_offset().y > 1000.0);
    assert_eq!(view.selection(), TextSelection::default());
    assert!(!ctx.memory(|memory| memory.has_focus(id)));
    assert!(
        view.request_navigation(
            &snapshot,
            Navigation {
                revision: 99,
                range: 0..0,
                selection: None,
                reveal: Reveal::Minimal,
                focus: true
            }
        )
        .is_err()
    );
}

#[test]
fn unicode_selection_clipboard_and_line_numbers_use_the_same_layout() {
    let ctx = egui::Context::default();
    let mut view = EditorView::default();
    let mut document = MemoryTextSession::new("aé🙂\nsecond");
    let id = egui::Id::new("unicode");
    focus(&ctx, &mut view, &mut document, id);
    let snapshot = document.snapshot().unwrap();
    view.request_navigation(
        &snapshot,
        Navigation {
            revision: 0,
            range: 1..7,
            selection: Some(TextSelection::new(&snapshot.text, 1, 7).unwrap()),
            reveal: Reveal::Minimal,
            focus: true,
        },
    )
    .unwrap();
    frame(
        &ctx,
        &mut view,
        &mut document,
        id,
        vec![egui::Event::Paste("世界".into())],
    );
    assert_eq!(document.text(), "a世界\nsecond");
    assert_eq!(view.selection().head(), "a世界".len());
    let with = frame(&ctx, &mut view, &mut document, id, vec![]);
    view.settings.line_numbers = false;
    let without = frame(&ctx, &mut view, &mut document, id, vec![]);
    assert!(without.text_rect.left() < with.text_rect.left());
    assert_eq!(without.selection, with.selection);
}

#[test]
fn read_only_editor_keeps_text_and_history_unchanged() {
    let ctx = egui::Context::default();
    let id = egui::Id::new("readonly");
    let mut view = EditorView::default();
    view.settings.read_only = true;
    let mut document = MemoryTextSession::new("Original");
    focus(&ctx, &mut view, &mut document, id);
    frame(
        &ctx,
        &mut view,
        &mut document,
        id,
        vec![
            key(egui::Key::A, egui::Modifiers::COMMAND),
            egui::Event::Paste("Changed".into()),
            key(egui::Key::Backspace, egui::Modifiers::NONE),
        ],
    );
    assert_eq!(document.text(), "Original");
    assert_eq!(document.snapshot().unwrap().revision, 0);
}

#[test]
fn mouse_hit_testing_respects_tabs_and_unicode_and_enters_text_at_that_position() {
    let ctx = egui::Context::default();
    let id = egui::Id::new("mouse");
    let mut view = EditorView::default();
    let mut document = MemoryTextSession::new("a\tbé\nsecond");
    let output = frame(&ctx, &mut view, &mut document, id, vec![]);
    let font = egui::TextStyle::Monospace.resolve(&ctx.global_style());
    let cell = ctx.fonts_mut(|fonts| fonts.glyph_width(&font, ' '));
    let position = egui::pos2(
        output.text_rect.left() + 8.0 + cell * 5.0,
        output.text_rect.top() + 6.0 + output.caret_rect.height() * 0.5,
    );
    frame(
        &ctx,
        &mut view,
        &mut document,
        id,
        vec![
            egui::Event::PointerMoved(position),
            egui::Event::PointerButton {
                pos: position,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ],
    );
    frame(
        &ctx,
        &mut view,
        &mut document,
        id,
        vec![egui::Event::PointerButton {
            pos: position,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }],
    );
    assert_eq!(view.selection().head(), 3);
    frame(
        &ctx,
        &mut view,
        &mut document,
        id,
        vec![egui::Event::Text("Z".into())],
    );
    assert_eq!(document.text(), "a\tbZé\nsecond");
}

#[test]
fn custom_text_focus_reserves_noop_editing_keys_and_leaves_save_to_the_host() {
    let ctx = egui::Context::default();
    let id = egui::Id::new("capture");
    let mut view = EditorView::default();
    let mut document = MemoryTextSession::new("");
    focus(&ctx, &mut view, &mut document, id);
    assert!(ctx.text_edit_focused());
    frame(
        &ctx,
        &mut view,
        &mut document,
        id,
        vec![
            key(egui::Key::ArrowLeft, egui::Modifiers::NONE),
            key(egui::Key::S, egui::Modifiers::COMMAND),
        ],
    );
    let remaining = ctx.input(|input| input.events.clone());
    assert!(!remaining.iter().any(|event| matches!(
        event,
        egui::Event::Key {
            key: egui::Key::ArrowLeft,
            ..
        }
    )));
    assert!(remaining.iter().any(|event| matches!(
        event,
        egui::Event::Key {
            key: egui::Key::S,
            ..
        }
    )));
    assert_eq!(document.text(), "");
}

#[test]
fn ime_preedit_is_a_draft_and_commits_once() {
    let ctx = egui::Context::default();
    let id = egui::Id::new("ime");
    let mut view = EditorView::default();
    let mut document = MemoryTextSession::new("text");
    focus(&ctx, &mut view, &mut document, id);
    let output = frame(
        &ctx,
        &mut view,
        &mut document,
        id,
        vec![egui::Event::Ime(egui::ImeEvent::Preedit {
            text: "に".into(),
            active_range_chars: None,
        })],
    );
    assert!(output.composing);
    assert_eq!(document.text(), "text");
    assert!(document.history(HistoryAction::Undo).is_err());
    frame(
        &ctx,
        &mut view,
        &mut document,
        id,
        vec![key(egui::Key::Z, egui::Modifiers::COMMAND)],
    );
    assert!(view.is_composing());
    assert_eq!(document.text(), "text");
    frame(
        &ctx,
        &mut view,
        &mut document,
        id,
        vec![egui::Event::Ime(egui::ImeEvent::Commit("日本".into()))],
    );
    assert!(!view.is_composing());
    assert_eq!(document.text(), "日本text");
    assert_eq!(document.snapshot().unwrap().revision, 1);
    document.history(HistoryAction::Undo).unwrap();
    assert_eq!(document.text(), "text");
}

#[test]
fn a_second_view_cannot_clear_another_views_composition_gate() {
    let mut document = MemoryTextSession::new("text");
    document.set_composing(egui::Id::new("first"), true);
    document.set_composing(egui::Id::new("second"), false);
    assert!(
        document
            .edit(
                0,
                TextEdit {
                    range: 0..0,
                    replacement: "!".into()
                }
            )
            .is_err()
    );
    assert_eq!(document.text(), "text");
}

#[test]
fn switching_editing_views_never_merges_their_undo_groups() {
    let ctx = egui::Context::default();
    let a = egui::Id::new("owner-a");
    let b = egui::Id::new("owner-b");
    let mut first = EditorView::default();
    let mut second = EditorView::default();
    let mut document = MemoryTextSession::new("base");
    focus(&ctx, &mut first, &mut document, a);
    frame(
        &ctx,
        &mut first,
        &mut document,
        a,
        vec![egui::Event::Text("A".into())],
    );
    frame(&ctx, &mut second, &mut document, b, vec![]);
    // The newly focused view draws before the old view observes focus loss.
    ctx.memory_mut(|memory| memory.request_focus(b));
    frame(
        &ctx,
        &mut second,
        &mut document,
        b,
        vec![egui::Event::Text("B".into())],
    );
    assert_eq!(document.text(), "BAbase");
    document.history(HistoryAction::Undo).unwrap();
    assert_eq!(document.text(), "Abase");
}

#[test]
fn journal_expiry_refreshes_positions_without_invalid_utf8_offsets() {
    let ctx = egui::Context::default();
    let id = egui::Id::new("dormant");
    let mut view = EditorView::default();
    let mut document = MemoryTextSession::new("é🙂");
    view.selection = TextSelection::caret(document.text(), document.text().len()).unwrap();
    frame(&ctx, &mut view, &mut document, id, vec![]);
    for revision in 0..140 {
        document
            .edit(
                revision,
                TextEdit {
                    range: 0..0,
                    replacement: "x".into(),
                },
            )
            .unwrap();
    }
    assert!(document.changes_since(0).unwrap().is_none());
    frame(&ctx, &mut view, &mut document, id, vec![]);
    assert!(document.text().is_char_boundary(view.selection().head()));
    assert!(view.selection().head() <= document.text().len());
}

#[test]
fn long_lines_are_virtualized_horizontally_and_delete_whole_graphemes() {
    let ctx = egui::Context::default();
    let id = egui::Id::new("long-line");
    let mut view = EditorView::default();
    let mut document = MemoryTextSession::new(format!("{}e\u{301}👩‍🚀", "x".repeat(2_000_000)));
    focus(&ctx, &mut view, &mut document, id);
    let snapshot = document.snapshot().unwrap();
    let end = snapshot.text.len();
    view.request_navigation(
        &snapshot,
        Navigation {
            revision: 0,
            range: end..end,
            selection: Some(TextSelection::caret(&snapshot.text, end).unwrap()),
            reveal: Reveal::Minimal,
            focus: true,
        },
    )
    .unwrap();
    let output = frame(
        &ctx,
        &mut view,
        &mut document,
        id,
        vec![key(egui::Key::Backspace, egui::Modifiers::NONE)],
    );
    assert!(output.laid_out_bytes < 4096);
    assert_eq!(document.text().len(), end - "👩‍🚀".len());
    frame(
        &ctx,
        &mut view,
        &mut document,
        id,
        vec![key(egui::Key::Backspace, egui::Modifiers::NONE)],
    );
    assert_eq!(document.text().len(), 2_000_000);
    document.history(HistoryAction::Undo).unwrap();
    assert_eq!(document.text().len(), end);
}

#[test]
fn word_navigation_and_selection_scan_runs_across_rope_leaves() {
    let text = TextBuffer::from(format!("{}  café_e\u{301} 👩‍🚀", "identifier".repeat(10000)));
    let first_end = "identifier".len() * 10000;
    assert_eq!(word_range(&text, 4500), 0..first_end);
    assert_eq!(word_boundary(&text, 0, true), first_end + 2);
    assert_eq!(word_boundary(&text, first_end, false), 0);
    let emoji_start = text.len() - "👩‍🚀".len();
    assert_eq!(word_range(&text, emoji_start), emoji_start..text.len());
    assert_eq!(word_boundary(&text, text.len(), false), emoji_start);
}

#[test]
#[ignore = "Explicit 107 MiB / million-line performance fixture"]
fn million_line_editing_keeps_layout_bounded() {
    use std::time::Instant;
    let fixture = format!("{}\n", "x".repeat(111)).repeat(1_000_000);
    let started = Instant::now();
    let mut document = MemoryTextSession::new(fixture);
    let load = started.elapsed();
    let ctx = egui::Context::default();
    let id = egui::Id::new("million");
    let mut view = EditorView::default();
    focus(&ctx, &mut view, &mut document, id);
    let snapshot = document.snapshot().unwrap();
    let original = snapshot.text.clone();
    let target = snapshot.text.line_start(999_999);
    view.request_navigation(
        &snapshot,
        Navigation {
            revision: 0,
            range: target..target,
            selection: Some(TextSelection::caret(&snapshot.text, target).unwrap()),
            reveal: Reveal::Center,
            focus: true,
        },
    )
    .unwrap();
    let mut times = Vec::new();
    for _ in 0..60 {
        let started = Instant::now();
        let output = frame(&ctx, &mut view, &mut document, id, vec![]);
        times.push(started.elapsed());
        assert!(output.rendered_lines < 64, "{} rows", output.rendered_lines);
        assert!(
            output.laid_out_bytes < 16 * 1024,
            "{} layout bytes",
            output.laid_out_bytes
        );
    }
    let started = Instant::now();
    frame(
        &ctx,
        &mut view,
        &mut document,
        id,
        vec![egui::Event::Text("!".into())],
    );
    let edit = started.elapsed();
    assert_eq!(document.text().len(), 112_000_001);
    assert_eq!(document.text().range_text(target..target + 1), "!");
    let started = Instant::now();
    frame(
        &ctx,
        &mut view,
        &mut document,
        id,
        vec![key(egui::Key::Z, egui::Modifiers::COMMAND)],
    );
    let undo = started.elapsed();
    assert!(document.text().ptr_eq(&original));
    times.sort();
    println!(
        "million-line fixture: {} bytes, load {:?}, idle median {:?}, max {:?}, edit {:?}, undo {:?}",
        original.len(),
        load,
        times[times.len() / 2],
        times.last().unwrap(),
        edit,
        undo
    );
}

#[test]
fn line_navigation_validates_before_mutation_and_does_not_require_language_features() {
    let snapshot = TextSnapshot {
        revision: 7,
        text: "one\r\né🙂\rlast".into(),
    };
    let mut view = EditorView::default();
    view.scroll_to_line(&snapshot, 2, Reveal::Center, false)
        .unwrap();
    assert_eq!(
        view.navigation.as_ref().unwrap().range,
        snapshot.text.line_start(2)..snapshot.text.line_start(2)
    );
    let before = view.navigation.clone().unwrap();
    assert!(
        view.scroll_to_line(&snapshot, 3, Reveal::Minimal, true)
            .is_err()
    );
    let mut stale = before.clone();
    stale.revision = 6;
    assert!(view.request_navigation(&snapshot, stale).is_err());
    assert_eq!(view.navigation.as_ref().unwrap().range, before.range);
    assert!(!view.navigation.as_ref().unwrap().focus);
    assert_eq!(view.selection(), TextSelection::default());
}

#[test]
fn independent_wheel_inputs_choose_their_axis_without_a_pause() {
    let mut gesture = ScrollGesture::default();
    assert_eq!(
        gesture.multiplier(egui::vec2(-2.0, -10.0)),
        egui::vec2(0.0, 1.0)
    );
    assert_eq!(
        gesture.multiplier(egui::vec2(-20.0, -1.0)),
        egui::vec2(1.0, 0.0)
    );
    assert_eq!(gesture.multiplier(egui::Vec2::ZERO), egui::vec2(1.0, 0.0));
    assert_eq!(
        gesture.multiplier(egui::vec2(-1.0, -20.0)),
        egui::vec2(0.0, 1.0)
    );
}

#[test]
fn horizontal_scrolling_requires_overflow_and_is_disabled_when_wrapped() {
    let ctx = egui::Context::default();
    let mut document = MemoryTextSession::new("short text\n");
    let mut view = EditorView::default();
    let id = egui::Id::new("horizontal-overflow");
    let scroll_right = |rect: egui::Rect| {
        vec![
            egui::Event::PointerMoved(rect.center()),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(-100.0, 0.0),
                phase: egui::TouchPhase::Move,
                modifiers: egui::Modifiers::NONE,
            },
        ]
    };
    let initial = frame(&ctx, &mut view, &mut document, id, vec![]);
    let fitting = frame(
        &ctx,
        &mut view,
        &mut document,
        id,
        scroll_right(initial.text_rect),
    );
    assert_eq!(
        fitting.scroll.x, 0.0,
        "fitting text must not scroll horizontally"
    );

    let mut document = MemoryTextSession::new("x".repeat(300));
    frame(&ctx, &mut view, &mut document, id, vec![]);
    let overflowing = frame(
        &ctx,
        &mut view,
        &mut document,
        id,
        scroll_right(initial.text_rect),
    );
    assert!(
        overflowing.scroll.x > 0.0,
        "long unwrapped lines must scroll"
    );

    view.settings.wrap = true;
    for _ in 0..4 {
        let wrapped = frame(
            &ctx,
            &mut view,
            &mut document,
            id,
            scroll_right(initial.text_rect),
        );
        assert_eq!(
            wrapped.scroll.x, 0.0,
            "wrapping must reset and disable horizontal scrolling"
        );
    }
}

#[test]
fn diagonal_wheel_inputs_switch_axes_without_a_gesture_restart() {
    let ctx = egui::Context::default();
    let mut document = MemoryTextSession::new(("x".repeat(200) + "\n").repeat(100));
    let mut view = EditorView::default();
    let id = egui::Id::new("axis-locked-editor");
    let initial = frame(&ctx, &mut view, &mut document, id, vec![]);
    let wheel = |delta, phase| {
        let mut events = vec![egui::Event::PointerMoved(initial.text_rect.center())];
        if phase == egui::TouchPhase::Start {
            events.push(egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::Vec2::ZERO,
                phase,
                modifiers: egui::Modifiers::NONE,
            });
        }
        events.push(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta,
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
        });
        events
    };
    let horizontal = frame(
        &ctx,
        &mut view,
        &mut document,
        id,
        wheel(egui::vec2(-80.0, -10.0), egui::TouchPhase::Start),
    );
    assert!(horizontal.scroll.x > 0.0);
    assert_eq!(horizontal.scroll.y, 0.0);
    let wobble = frame(
        &ctx,
        &mut view,
        &mut document,
        id,
        wheel(egui::vec2(-1.0, -100.0), egui::TouchPhase::Move),
    );
    assert!(wobble.scroll.y > 0.0);
    assert_eq!(wobble.scroll.x, horizontal.scroll.x);
    // A native start follows the same per-input rule.
    let vertical = frame(
        &ctx,
        &mut view,
        &mut document,
        id,
        wheel(egui::vec2(-1.0, -80.0), egui::TouchPhase::Start),
    );
    assert!(vertical.scroll.y > 0.0);
    assert_eq!(vertical.scroll.x, wobble.scroll.x);
}

#[test]
fn pointer_presses_place_caret_then_select_word_and_line_before_release() {
    let ctx = egui::Context::default();
    let mut document = MemoryTextSession::new("alpha beta\r\nnext");
    let mut view = EditorView::default();
    let id = egui::Id::new("press-selection");
    let initial = frame(&ctx, &mut view, &mut document, id, vec![]);
    let position = initial.text_rect.min + egui::vec2(26.0, 12.0);
    for click in 1..=3 {
        let down = frame(
            &ctx,
            &mut view,
            &mut document,
            id,
            vec![
                egui::Event::PointerMoved(position),
                egui::Event::PointerButton {
                    pos: position,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        match click {
            1 => {
                assert_eq!(down.selection.anchor, down.selection.head);
                assert!(down.selection.head > 0);
            }
            2 => assert_eq!(down.selection, TextSelection { anchor: 0, head: 5 }),
            3 => assert_eq!(
                down.selection,
                TextSelection {
                    anchor: 0,
                    head: 10
                }
            ),
            _ => unreachable!(),
        }
        let up = frame(
            &ctx,
            &mut view,
            &mut document,
            id,
            vec![egui::Event::PointerButton {
                pos: position,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        assert_eq!(up.selection, down.selection);
    }
}

#[test]
fn resizing_keeps_wrapped_text_while_new_width_is_prepared() {
    let ctx = egui::Context::default();
    let mut document = MemoryTextSession::new("alpha beta ".repeat(200));
    let snapshot = document.snapshot().unwrap();
    let mut view = EditorView::default();
    view.settings.wrap = true;
    view.wrapping.index = Some(
        wrapping::WrapIndex::build(
            snapshot.clone(),
            24,
            &std::sync::atomic::AtomicBool::new(false),
        )
        .unwrap(),
    );
    view.wrap_active = true;
    let output = frame(
        &ctx,
        &mut view,
        &mut document,
        egui::Id::new("resize-wrapping"),
        vec![],
    );
    assert!(view.wrapping_ready());
    assert!(output.rendered_lines > 1);
    assert!(view.active_wrap(&snapshot, Some(60)).is_some());
}
