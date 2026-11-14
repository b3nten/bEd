// Comparing selections to `[a..b]` is the point.
#![allow(clippy::single_range_in_vec_init)]
use super::carets_tests::{COMMAND, Fixture};
use super::tests::key;
use super::*;
use std::time::{Duration, Instant};

const ALT: egui::Modifiers = egui::Modifiers::ALT;
const SHIFT: egui::Modifiers = egui::Modifiers::SHIFT;

fn press(f: &mut Fixture, key_: egui::Key, modifiers: egui::Modifiers) {
    f.send(vec![key(key_, modifiers)]);
}

#[test]
fn delete_line_keeps_the_column_and_takes_the_newline_before_the_last_line() {
    let mut f = Fixture::new("a\nbb\nc");
    f.at(&[(3, 3)]);
    press(&mut f, egui::Key::K, COMMAND | SHIFT);
    assert_eq!(f.text(), "a\nc");
    assert_eq!(f.ranges(), [3..3]);
    press(&mut f, egui::Key::K, COMMAND | SHIFT);
    assert_eq!(f.text(), "a");
    assert_eq!(f.ranges(), [1..1]);
    press(&mut f, egui::Key::K, COMMAND | SHIFT);
    assert_eq!(f.text(), "");
    // Carets sharing lines delete them once; undo is one step.
    let mut f = Fixture::new("1\n2\n3\n4");
    f.at(&[(0, 0), (1, 3), (6, 6)]);
    press(&mut f, egui::Key::K, COMMAND | SHIFT);
    assert_eq!(f.text(), "3");
    press(&mut f, egui::Key::Z, COMMAND);
    assert_eq!(f.text(), "1\n2\n3\n4");
}

#[test]
fn copy_lines_up_and_down_with_and_without_a_final_newline() {
    let mut f = Fixture::new("ab\ncd");
    f.at(&[(1, 1)]);
    press(&mut f, egui::Key::ArrowDown, SHIFT | ALT);
    assert_eq!(f.text(), "ab\nab\ncd");
    assert_eq!(f.ranges(), [4..4]);
    let mut f = Fixture::new("ab\ncd");
    f.at(&[(4, 4)]);
    press(&mut f, egui::Key::ArrowDown, SHIFT | ALT);
    assert_eq!(f.text(), "ab\ncd\ncd");
    assert_eq!(f.ranges(), [7..7]);
    let mut f = Fixture::new("ab\ncd");
    f.at(&[(4, 4)]);
    press(&mut f, egui::Key::ArrowUp, SHIFT | ALT);
    assert_eq!(f.text(), "ab\ncd\ncd");
    assert_eq!(f.ranges(), [4..4]);
}

#[test]
fn move_lines_swaps_with_the_neighbour_keeping_line_endings_in_place() {
    let mut f = Fixture::new("a\nb\nc");
    f.at(&[(0, 0)]);
    press(&mut f, egui::Key::ArrowDown, ALT);
    assert_eq!(f.text(), "b\na\nc");
    assert_eq!(f.ranges(), [2..2]);
    press(&mut f, egui::Key::ArrowDown, ALT);
    assert_eq!(f.text(), "b\nc\na");
    assert_eq!(f.ranges(), [4..4]);
    // Already last: nothing.
    press(&mut f, egui::Key::ArrowDown, ALT);
    assert_eq!(f.text(), "b\nc\na");
    press(&mut f, egui::Key::ArrowUp, ALT);
    assert_eq!(f.text(), "b\na\nc");
    assert_eq!(f.ranges(), [2..2]);

    let mut f = Fixture::new("a\r\nb");
    f.at(&[(0, 0)]);
    press(&mut f, egui::Key::ArrowDown, ALT);
    assert_eq!(f.text(), "b\r\na");
    assert_eq!(f.ranges(), [3..3]);

    // A selection's lines move together; its end at a line start follows.
    let mut f = Fixture::new("1\n2\n3\n4");
    f.at(&[(0, 4)]);
    press(&mut f, egui::Key::ArrowDown, ALT);
    assert_eq!(f.text(), "3\n1\n2\n4");
    assert_eq!(f.ranges(), [2..6]);
    // Carets on neighbouring lines move as one block.
    let mut f = Fixture::new("1\n2\n3\n4");
    f.at(&[(2, 2), (4, 4)]);
    press(&mut f, egui::Key::ArrowUp, ALT);
    assert_eq!(f.text(), "2\n3\n1\n4");
    assert_eq!(f.ranges(), [0..0, 2..2]);
}

#[test]
fn insert_and_join_lines() {
    let mut f = Fixture::new("  ab");
    f.at(&[(3, 3)]);
    press(&mut f, egui::Key::Enter, COMMAND);
    assert_eq!(f.text(), "  ab\n  ");
    assert_eq!(f.ranges(), [7..7]);
    let mut f = Fixture::new("  ab");
    f.at(&[(3, 3)]);
    press(&mut f, egui::Key::Enter, COMMAND | SHIFT);
    assert_eq!(f.text(), "  \n  ab");
    assert_eq!(f.ranges(), [2..2]);

    let ctrl = egui::Modifiers::CTRL;
    let mut f = Fixture::new("a  \n   b\nc");
    f.at(&[(0, 0)]);
    press(&mut f, egui::Key::J, ctrl);
    assert_eq!(f.text(), "a b\nc");
    assert_eq!(f.ranges(), [1..1]);
    press(&mut f, egui::Key::J, ctrl);
    assert_eq!(f.text(), "a b c");
    // A selection's lines join with each other; an empty line adds no space.
    let mut f = Fixture::new("x\n\ny\nz");
    f.at(&[(0, 5)]);
    press(&mut f, egui::Key::J, ctrl);
    assert_eq!(f.text(), "x y\nz");
}

#[test]
fn indent_lines_and_whole_line_clipboard() {
    let mut f = Fixture::new("a\nb");
    f.at(&[(1, 1), (2, 2)]);
    press(&mut f, egui::Key::CloseBracket, COMMAND);
    assert_eq!(f.text(), "    a\n    b");
    press(&mut f, egui::Key::OpenBracket, COMMAND);
    assert_eq!(f.text(), "a\nb");

    // Nothing selected: Copy takes the line, and pasting it puts it above.
    let mut f = Fixture::new("one\ntwo");
    f.at(&[(1, 1)]);
    f.send(vec![egui::Event::Copy]);
    f.at(&[(5, 5)]);
    f.send(vec![egui::Event::Paste("one\n".into())]);
    assert_eq!(f.text(), "one\none\ntwo");
    assert_eq!(f.ranges(), [9..9]);
    // Cut takes the line out.
    f.send(vec![egui::Event::Cut]);
    assert_eq!(f.text(), "one\none");
    // Other text pastes as usual.
    f.at(&[(0, 0)]);
    f.send(vec![egui::Event::Paste("x".into())]);
    assert_eq!(f.text(), "xone\none");
}

/// Cmd/Ctrl+X arrives as its key and then the `Cut` the UI runtime adds for
/// it: one cut, of the selection, not a second one taking the line too.
#[test]
fn cut_chord_cuts_the_selection_once() {
    let mut f = Fixture::new("one two\nthree");
    f.at(&[(4, 7)]);
    f.send(vec![
        key(egui::Key::X, COMMAND),
        egui::Event::Cut,
    ]);
    assert_eq!(f.text(), "one \nthree");
    assert_eq!(f.ranges(), [4..4]);
    f.send(vec![
        key(egui::Key::V, COMMAND),
        egui::Event::Paste("two".into()),
    ]);
    assert_eq!(f.text(), "one two\nthree");
}

#[test]
fn expand_selection_follows_the_syntax_tree_and_shrink_steps_back() {
    let code = "fn main() {\n    let total = add(1, 2);\n}\n";
    let mut f = Fixture::new(code);
    let mut analysis = crate::syntax::AnalysisSession::new();
    syntax::update_analysis(&mut analysis, &f.doc, "rust").unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while analysis.status() == &crate::syntax::AnalysisStatus::Pending {
        analysis.poll();
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    f.view.set_analysis(&analysis);
    let one = code.find("1,").unwrap();
    f.at(&[(one, one)]);
    let (expand, shrink) = if cfg!(target_os = "macos") {
        (
            COMMAND | egui::Modifiers::CTRL | SHIFT,
            COMMAND | egui::Modifiers::CTRL | SHIFT,
        )
    } else {
        (SHIFT | ALT, SHIFT | ALT)
    };
    let selected = |f: &Fixture| {
        f.ranges()
            .iter()
            .map(|r| code[r.clone()].to_owned())
            .collect::<Vec<_>>()
    };
    press(&mut f, egui::Key::ArrowRight, expand);
    assert_eq!(selected(&f), ["1"]);
    press(&mut f, egui::Key::ArrowRight, expand);
    assert_eq!(selected(&f), ["(1, 2)"]);
    press(&mut f, egui::Key::ArrowRight, expand);
    assert_eq!(selected(&f), ["add(1, 2)"]);
    press(&mut f, egui::Key::ArrowLeft, shrink);
    assert_eq!(selected(&f), ["(1, 2)"]);
    press(&mut f, egui::Key::ArrowLeft, shrink);
    press(&mut f, egui::Key::ArrowLeft, shrink);
    assert_eq!(f.ranges(), [one..one]);

    // Without an analysis: word, line, everything.
    let mut f = Fixture::new("foo bar\nbaz");
    f.at(&[(5, 5)]);
    press(&mut f, egui::Key::ArrowRight, expand);
    assert_eq!(f.ranges(), [4..7]);
    press(&mut f, egui::Key::ArrowRight, expand);
    assert_eq!(f.ranges(), [0..7]);
    press(&mut f, egui::Key::ArrowRight, expand);
    assert_eq!(f.ranges(), [0..11]);
}
