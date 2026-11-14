// Comparing selections to `[a..b]` is the point.
#![allow(clippy::single_range_in_vec_init)]
use super::carets_tests::{COMMAND, Fixture, text};
use super::tests::key;
use super::*;
use std::time::{Duration, Instant};

fn fixture(code: &str, language: Option<&'static str>) -> Fixture {
    let mut f = Fixture::new(code);
    f.view.language = language;
    f
}

fn analyzed(code: &str, language: &'static str) -> Fixture {
    let mut f = fixture(code, Some(language));
    let mut analysis = crate::syntax::AnalysisSession::new();
    syntax::update_analysis(&mut analysis, &f.doc, language).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while analysis.status() == &crate::syntax::AnalysisStatus::Pending {
        analysis.poll();
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    f.view.set_analysis(&analysis);
    f
}

#[test]
fn brackets_close_step_over_and_undo_as_one() {
    let mut f = fixture("f", Some("rust"));
    f.at(&[(1, 1)]);
    f.send(vec![text("(")]);
    assert_eq!(f.text(), "f()");
    assert_eq!(f.ranges(), [2..2]);
    f.send(vec![text("x")]);
    f.send(vec![text(")")]);
    assert_eq!(f.text(), "f(x)");
    assert_eq!(f.ranges(), [4..4]);
    // Not before a word; a closer with nothing to step over is typed.
    let mut f = fixture("abc", Some("rust"));
    f.at(&[(0, 0)]);
    f.send(vec![text("(")]);
    assert_eq!(f.text(), "(abc");
    f.send(vec![text("]")]);
    assert_eq!(f.text(), "(]abc");
    // One undo takes the pair.
    let mut f = fixture("", Some("rust"));
    f.at(&[(0, 0)]);
    f.send(vec![text("{")]);
    assert_eq!(f.text(), "{}");
    f.send(vec![key(egui::Key::Z, COMMAND)]);
    assert_eq!(f.text(), "");
    // Off: only what's typed.
    let mut f = fixture("", Some("rust"));
    f.view.settings.auto_close = false;
    f.at(&[(0, 0)]);
    f.send(vec![text("(")]);
    assert_eq!(f.text(), "(");
}

#[test]
fn quotes_close_by_language_and_not_after_a_word() {
    let mut f = fixture("", Some("rust"));
    f.at(&[(0, 0)]);
    f.send(vec![text("\"")]);
    assert_eq!(f.text(), "\"\"");
    f.send(vec![text("\"")]);
    assert_eq!(f.text(), "\"\"");
    assert_eq!(f.ranges(), [2..2]);
    f.send(vec![text("'")]);
    assert_eq!(f.text(), "\"\"'");
    let mut f = fixture("don", Some("python"));
    f.at(&[(3, 3)]);
    f.send(vec![text("'")]);
    assert_eq!(f.text(), "don'");
    let mut f = fixture(" ", Some("python"));
    f.at(&[(1, 1)]);
    f.send(vec![text("'")]);
    assert_eq!(f.text(), " ''");
    // Plain text pairs brackets but not quotes.
    let mut f = fixture("", None);
    f.at(&[(0, 0)]);
    f.send(vec![text("\""), text("[")]);
    assert_eq!(f.text(), "\"[]");
}

#[test]
fn typing_wraps_selections_and_backspace_takes_empty_pairs() {
    let mut f = fixture("abc def", Some("rust"));
    f.at(&[(0, 3), (4, 7)]);
    f.send(vec![text("[")]);
    assert_eq!(f.text(), "[abc] [def]");
    assert_eq!(f.ranges(), [1..4, 7..10]);
    f.send(vec![text("\"")]);
    assert_eq!(f.text(), "[\"abc\"] [\"def\"]");
    // A closing bracket replaces the selection.
    let mut f = fixture("abc", Some("rust"));
    f.at(&[(0, 3)]);
    f.send(vec![text(")")]);
    assert_eq!(f.text(), ")");

    let mut f = fixture("() x", Some("rust"));
    f.at(&[(1, 1), (4, 4)]);
    f.send(vec![key(egui::Key::Backspace, egui::Modifiers::NONE)]);
    assert_eq!(f.text(), " ");
    assert_eq!(f.ranges(), [0..0, 1..1]);
}

#[test]
fn matching_brackets_use_the_tree_and_jump_between_each_other() {
    let mut f = fixture("a(b[c]d)e", None);
    f.at(&[(1, 1)]);
    let snapshot = f.doc.snapshot().unwrap();
    assert_eq!(f.view.matching_bracket(&snapshot, 1), Some((1, 7)));
    assert_eq!(f.view.matching_bracket(&snapshot, 8), Some((7, 1)));
    assert_eq!(f.view.matching_bracket(&snapshot, 0), None);
    // Beside a character of several bytes: no bracket, no panic.
    let wide = fixture("(🙂)", None);
    let wide_snapshot = wide.doc.snapshot().unwrap();
    assert_eq!(wide.view.matching_bracket(&wide_snapshot, 5), Some((5, 0)));
    assert_eq!(wide.view.matching_bracket(&wide_snapshot, 4), None);
    let jump = key(egui::Key::Backslash, COMMAND | egui::Modifiers::SHIFT);
    f.send(vec![jump.clone()]);
    assert_eq!(f.ranges(), [7..7]);
    f.send(vec![jump]);
    assert_eq!(f.ranges(), [1..1]);

    // A bracket in a string is no bracket.
    let code = "fn m() { f(\"(\", x); }";
    let f = analyzed(code, "rust");
    let open = code.find("f(").unwrap() + 1;
    let close = code.find(");").unwrap();
    let snapshot = f.doc.snapshot().unwrap();
    assert_eq!(
        f.view.matching_bracket(&snapshot, open),
        Some((open, close))
    );
    let quoted = code.find("\"(").unwrap() + 1;
    assert_eq!(f.view.matching_bracket(&snapshot, quoted), None);
}

#[test]
fn box_selection_takes_the_columns_of_each_line() {
    let mut f = fixture("abcd\nab\nabcd", None);
    f.at(&[(0, 0)]);
    let snapshot = f.doc.snapshot().unwrap();
    f.view.select_box(&snapshot, (0, 1), (2, 3));
    assert_eq!(f.ranges(), [1..3, 6..7, 9..11]);
    f.send(vec![text("x")]);
    assert_eq!(f.text(), "axd\nax\naxd");
    // Upwards and leftwards: the anchor stays where the drag began.
    let mut f = fixture("abcd\nabcd", None);
    let snapshot = f.doc.snapshot().unwrap();
    f.view.select_box(&snapshot, (1, 3), (0, 1));
    assert_eq!(
        f.view.selections(),
        [
            TextSelection::new(f.doc.text(), 3, 1).unwrap(),
            TextSelection::new(f.doc.text(), 8, 6).unwrap()
        ]
    );
    assert_eq!(
        f.view.selection(),
        TextSelection::new(f.doc.text(), 3, 1).unwrap()
    );
}

/// A frame with modifiers held (the shared one holds none).
fn frame_with(
    f: &mut Fixture,
    modifiers: egui::Modifiers,
    events: Vec<egui::Event>,
) -> EditorOutput {
    let ctx = f.ctx.clone();
    ctx.options_mut(|options| options.max_passes = std::num::NonZeroUsize::new(1).unwrap());
    let time = ctx.cumulative_frame_nr() as f64 / 60.0;
    let mut result = None;
    let _ = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(600.0, 300.0),
            )),
            time: Some(time),
            focused: true,
            events: std::iter::once(egui::Event::ModifiersChanged(modifiers))
                .chain(events)
                .collect(),
            ..Default::default()
        },
        |ui| {
            result = Some(
                f.view
                    .show(ui, f.id, &mut f.doc, MotionPolicy::default())
                    .unwrap(),
            );
        },
    );
    result.unwrap()
}

#[test]
fn shift_alt_or_middle_drag_selects_a_box() {
    let mut f = fixture("abcd\nab\nabcd", None);
    // Where bytes are drawn: the caret's place at each.
    let mut at = |byte: usize| {
        f.at(&[(byte, byte)]);
        frame_with(&mut f, egui::Modifiers::NONE, vec![])
            .caret_rect
            .center()
    };
    let (from, to) = (at(1), at(11));
    let held = egui::Modifiers::ALT | egui::Modifiers::SHIFT;
    let button = |pos, pressed, button, modifiers| egui::Event::PointerButton {
        pos,
        button,
        pressed,
        modifiers,
    };
    for (button_, modifiers) in [
        (egui::PointerButton::Primary, held),
        (egui::PointerButton::Middle, egui::Modifiers::NONE),
    ] {
        f.at(&[(0, 0)]);
        frame_with(
            &mut f,
            modifiers,
            vec![
                egui::Event::PointerMoved(from),
                button(from, true, button_, modifiers),
            ],
        );
        for step in 1..=4 {
            let pos = from + (to - from) * (step as f32 / 4.0);
            frame_with(&mut f, modifiers, vec![egui::Event::PointerMoved(pos)]);
        }
        frame_with(
            &mut f,
            modifiers,
            vec![button(to, false, button_, modifiers)],
        );
        assert_eq!(f.ranges(), [1..3, 6..7, 9..11], "{button_:?}");
        // Released: an ordinary drag selects as usual again.
        assert!(f.view.box_anchor.is_none());
    }
}
