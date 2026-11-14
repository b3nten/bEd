// Comparing selections to `[a..b]` is the point.
#![allow(clippy::single_range_in_vec_init)]
use super::*;
use crate::editor::carets::Caret;
use crate::editor::tests::{focus, frame, key};

pub(super) const COMMAND: egui::Modifiers = egui::Modifiers::COMMAND;

/// The editor's commands by action, as the workbench would find them.
fn commands() -> Vec<(&'static str, EditorCommand)> {
    fn of<A: EditorAction>() -> (&'static str, EditorCommand) {
        (A::KEY, A::COMMAND)
    }
    vec![
        of::<SelectLine>(),
        of::<AddCaretAbove>(),
        of::<AddCaretBelow>(),
        of::<AddNextOccurrence>(),
        of::<SelectAllOccurrences>(),
        of::<AddCaretsToLineEnds>(),
        of::<DeleteLine>(),
        of::<CopyLineUp>(),
        of::<CopyLineDown>(),
        of::<MoveLineUp>(),
        of::<MoveLineDown>(),
        of::<InsertLineAbove>(),
        of::<InsertLineBelow>(),
        of::<JoinLines>(),
        of::<IndentLines>(),
        of::<OutdentLines>(),
        of::<ExpandSelection>(),
        of::<ShrinkSelection>(),
        of::<ToggleLineComment>(),
        of::<ToggleBlockComment>(),
        of::<JumpToBracket>(),
        of::<Fold>(),
        of::<Unfold>(),
        of::<ToggleFold>(),
        of::<FoldAll>(),
        of::<UnfoldAll>(),
    ]
}

/// The command `event` presses in `crate::keymap::VSCODE`: what the
/// workbench's claim pass does for a focused editor, without an app.
fn command_for(event: &egui::Event) -> Option<EditorCommand> {
    use bedtime_workbench::Modifiers;
    use bevy::input::keyboard::KeyCode;
    let egui::Event::Key {
        key,
        pressed: true,
        modifiers,
        ..
    } = event
    else {
        return None;
    };
    let mac = cfg!(target_os = "macos");
    let mut held = Modifiers::NONE;
    for (on, flag) in [
        (
            modifiers.ctrl || modifiers.command && !mac,
            Modifiers::CONTROL,
        ),
        (modifiers.shift, Modifiers::SHIFT),
        (modifiers.alt, Modifiers::ALT),
        (
            modifiers.mac_cmd || modifiers.command && mac,
            Modifiers::META,
        ),
    ] {
        if on {
            held = held.union(flag);
        }
    }
    let code = |key: KeyCode| match key {
        KeyCode::KeyD => Some(egui::Key::D),
        KeyCode::KeyI => Some(egui::Key::I),
        KeyCode::KeyJ => Some(egui::Key::J),
        KeyCode::KeyK => Some(egui::Key::K),
        KeyCode::KeyL => Some(egui::Key::L),
        KeyCode::ArrowUp => Some(egui::Key::ArrowUp),
        KeyCode::ArrowDown => Some(egui::Key::ArrowDown),
        KeyCode::ArrowLeft => Some(egui::Key::ArrowLeft),
        KeyCode::ArrowRight => Some(egui::Key::ArrowRight),
        KeyCode::Enter => Some(egui::Key::Enter),
        KeyCode::BracketLeft => Some(egui::Key::OpenBracket),
        KeyCode::BracketRight => Some(egui::Key::CloseBracket),
        KeyCode::Slash => Some(egui::Key::Slash),
        KeyCode::Backslash => Some(egui::Key::Backslash),
        _ => None,
    };
    let commands = commands();
    crate::keymap::VSCODE.iter().find_map(|&(action, chords)| {
        chords
            .iter()
            .any(|chord| {
                let chord = chord.resolved();
                chord.modifiers == held && code(chord.key) == Some(*key)
            })
            .then(|| commands.iter().find(|(a, _)| *a == action).map(|(_, c)| *c))
            .flatten()
    })
}

pub(super) struct Fixture {
    pub ctx: egui::Context,
    pub id: egui::Id,
    pub view: EditorView,
    pub doc: MemoryTextSession,
}

impl Fixture {
    pub(super) fn new(text: &str) -> Self {
        let ctx = egui::Context::default();
        let id = egui::Id::new("carets");
        let mut doc = MemoryTextSession::new(text);
        let mut view = EditorView::default();
        focus(&ctx, &mut view, &mut doc, id);
        Self { ctx, id, view, doc }
    }
    /// Carets at `(anchor, head)`s; the last is the primary.
    pub(super) fn at(&mut self, selections: &[(usize, usize)]) {
        let carets = selections
            .iter()
            .enumerate()
            .map(|(index, &(anchor, head))| Caret {
                selection: TextSelection::new(self.doc.text(), anchor, head).unwrap(),
                column: None,
                upstream: false,
                primary: index + 1 == selections.len(),
            })
            .collect();
        let revision = self.doc.snapshot().unwrap().revision;
        self.view.set_carets(carets, revision);
    }
    /// Draw a frame with `events`. A key the keymap binds to an editor
    /// command runs it, as the workbench would (before the frame's typing).
    pub(super) fn send(&mut self, events: Vec<egui::Event>) {
        let mut typed = Vec::new();
        for event in events {
            match command_for(&event) {
                Some(command) => {
                    let earlier = std::mem::take(&mut typed);
                    if !earlier.is_empty() {
                        frame(&self.ctx, &mut self.view, &mut self.doc, self.id, earlier);
                    }
                    self.view.queue_command(command);
                }
                None => typed.push(event),
            }
        }
        frame(&self.ctx, &mut self.view, &mut self.doc, self.id, typed);
    }
    pub(super) fn ranges(&self) -> Vec<Range<usize>> {
        self.view.selections().iter().map(|s| s.range()).collect()
    }
    pub(super) fn text(&self) -> String {
        self.doc.text().to_string()
    }
    pub(super) fn revision(&self) -> u64 {
        self.doc.snapshot().unwrap().revision
    }
}

pub(super) fn text(text: &str) -> egui::Event {
    egui::Event::Text(text.into())
}

#[test]
fn typing_at_several_carets_is_one_revision_and_one_undo_step() {
    let mut f = Fixture::new("a\nb\nc");
    f.at(&[(1, 1), (3, 3), (5, 5)]);
    let before = f.revision();
    f.send(vec![text("!")]);
    assert_eq!(f.text(), "a!\nb!\nc!");
    assert_eq!(f.revision(), before + 1);
    assert_eq!(f.ranges(), [2..2, 5..5, 8..8]);
    f.send(vec![text("?")]);
    assert_eq!(f.text(), "a!?\nb!?\nc!?");
    f.send(vec![key(egui::Key::Backspace, egui::Modifiers::NONE)]);
    assert_eq!(f.text(), "a!\nb!\nc!");
    assert_eq!(f.ranges(), [2..2, 5..5, 8..8]);
    // Typing is one group: undone at once, every caret back in place.
    f.send(vec![key(egui::Key::Z, COMMAND)]);
    assert_eq!(f.text(), "a\nb\nc");
    assert_eq!(f.ranges(), [1..1, 3..3, 5..5]);
    // Escape leaves the primary (the last).
    f.send(vec![key(egui::Key::Escape, egui::Modifiers::NONE)]);
    assert_eq!(f.ranges(), [5..5]);
}

#[test]
fn carets_move_together_and_merge_when_they_meet() {
    let mut f = Fixture::new("abc\ndef");
    f.at(&[(1, 1), (2, 2), (5, 5)]);
    f.send(vec![key(egui::Key::ArrowLeft, egui::Modifiers::NONE)]);
    assert_eq!(f.ranges(), [0..0, 1..1, 4..4]);
    // Two at the line start meet and become one.
    f.send(vec![key(egui::Key::ArrowLeft, egui::Modifiers::NONE)]);
    assert_eq!(f.ranges(), [0..0, 3..3]);
    f.send(vec![key(egui::Key::ArrowRight, egui::Modifiers::SHIFT)]);
    assert_eq!(f.ranges(), [0..1, 3..4]);
    // Each keeps its own column going down.
    let mut f = Fixture::new("abcdef\nab\nabcdef\nabcdef");
    f.at(&[(15, 15), (22, 22)]);
    let up = key(egui::Key::ArrowUp, egui::Modifiers::NONE);
    f.send(vec![up.clone()]);
    assert_eq!(f.ranges(), [9..9, 15..15]);
    f.send(vec![up]);
    assert_eq!(f.ranges(), [5..5, 9..9]);
}

#[test]
fn deletion_enter_and_tab_apply_at_every_caret() {
    let mut f = Fixture::new("  ab\n  cd");
    f.at(&[(4, 4), (9, 9)]);
    f.send(vec![key(egui::Key::Backspace, egui::Modifiers::NONE)]);
    assert_eq!(f.text(), "  a\n  c");
    f.send(vec![key(egui::Key::Enter, egui::Modifiers::NONE)]);
    assert_eq!(f.text(), "  a\n  \n  c\n  ");
    assert_eq!(f.ranges(), [6..6, 13..13]);
    f.send(vec![key(egui::Key::Tab, egui::Modifiers::NONE)]);
    assert_eq!(f.text(), "  a\n    \n  c\n    ");
    // Shift+Tab outdents each line once, even with two carets on it.
    let mut f = Fixture::new("    a b\n    c");
    f.at(&[(5, 5), (7, 7), (13, 13)]);
    f.send(vec![key(egui::Key::Tab, egui::Modifiers::SHIFT)]);
    assert_eq!(f.text(), "a b\nc");
    assert_eq!(f.ranges(), [1..1, 3..3, 5..5]);
    // Overlapping word deletions join.
    let mut f = Fixture::new("ab cd");
    f.at(&[(2, 2), (3, 3)]);
    f.send(vec![key(egui::Key::Backspace, egui::Modifiers::ALT)]);
    assert_eq!(f.text(), "cd");
    assert_eq!(f.ranges(), [0..0]);
}

#[test]
fn clipboard_joins_selections_and_spreads_a_paste_line_per_caret() {
    let mut f = Fixture::new("one two three");
    f.at(&[(0, 3), (4, 7), (8, 13)]);
    f.send(vec![egui::Event::Cut]);
    assert_eq!(f.text(), "  ");
    assert_eq!(f.ranges(), [0..0, 1..1, 2..2]);
    f.send(vec![egui::Event::Paste("1\n2\n3".into())]);
    assert_eq!(f.text(), "1 2 3");
    // Lines not matching the carets: the whole text at each.
    f.send(vec![egui::Event::Paste("x\ny".into())]);
    assert_eq!(f.text(), "1x\ny 2x\ny 3x\ny");
}

#[test]
fn next_occurrence_selects_the_word_then_whole_word_matches_and_wraps() {
    let mut f = Fixture::new("foo food foo bar foo");
    f.at(&[(10, 10)]);
    let d = key(egui::Key::D, COMMAND);
    f.send(vec![d.clone()]);
    assert_eq!(f.ranges(), [9..12]);
    f.send(vec![d.clone()]);
    assert_eq!(f.ranges(), [9..12, 17..20]);
    // Wraps past the end, skipping "food".
    f.send(vec![d.clone()]);
    assert_eq!(f.ranges(), [0..3, 9..12, 17..20]);
    // All taken: nothing more.
    f.send(vec![d]);
    assert_eq!(f.ranges().len(), 3);
    f.send(vec![text("x")]);
    assert_eq!(f.text(), "x food x bar x");

    let mut f = Fixture::new("ab.ab.ab");
    f.at(&[(3, 5)]);
    f.send(vec![key(egui::Key::L, COMMAND | egui::Modifiers::SHIFT)]);
    assert_eq!(f.ranges(), [0..2, 3..5, 6..8]);
    // The caret just after a word takes it.
    let mut f = Fixture::new("let x = y;");
    f.at(&[(3, 3)]);
    f.send(vec![key(egui::Key::D, COMMAND)]);
    assert_eq!(f.ranges(), [0..3]);
}

#[test]
fn carets_are_added_above_below_at_line_ends_and_by_alt_click() {
    let mut f = Fixture::new("abc\nabc\nabc");
    f.at(&[(2, 2)]);
    let down = key(egui::Key::ArrowDown, COMMAND | egui::Modifiers::ALT);
    f.send(vec![down.clone(), down.clone(), down]);
    assert_eq!(f.ranges(), [2..2, 6..6, 10..10]);
    f.send(vec![text("!")]);
    assert_eq!(f.text(), "ab!c\nab!c\nab!c");

    let mut f = Fixture::new("abc\nabc\nabc");
    f.at(&[(9, 9)]);
    f.send(vec![key(
        egui::Key::ArrowUp,
        COMMAND | egui::Modifiers::ALT,
    )]);
    assert_eq!(f.ranges(), [5..5, 9..9]);
    // The new one is the primary: Escape keeps it.
    f.send(vec![key(egui::Key::Escape, egui::Modifiers::NONE)]);
    assert_eq!(f.ranges(), [5..5]);

    let mut f = Fixture::new("ab\ncd\nef");
    f.at(&[(1, 7)]);
    f.send(vec![key(egui::Key::I, COMMAND | egui::Modifiers::ALT)]);
    assert_eq!(f.ranges(), [2..2, 5..5, 7..7]);

    let mut f = Fixture::new("abc");
    f.at(&[(0, 0)]);
    let revision = f.revision();
    f.view.toggle_caret(revision, 2);
    assert_eq!(f.ranges(), [0..0, 2..2]);
    f.view.toggle_caret(revision, 0);
    assert_eq!(f.ranges(), [2..2]);
}

#[test]
fn others_follow_edits_made_elsewhere_and_read_only_ignores_typing() {
    let mut f = Fixture::new("abc abc");
    f.at(&[(1, 1), (5, 5)]);
    // Another view inserts at the start.
    let revision = f.revision();
    f.doc
        .edit(
            revision,
            TextEdit {
                range: 0..0,
                replacement: ">>".into(),
            },
        )
        .unwrap();
    f.send(vec![]);
    assert_eq!(f.ranges(), [3..3, 7..7]);
    f.view.settings.read_only = true;
    f.send(vec![text("x")]);
    assert_eq!(f.text(), ">>abc abc");
    // Select Line takes every caret's line.
    let mut f = Fixture::new("a\nb\nc");
    f.at(&[(0, 0), (4, 4)]);
    f.send(vec![key(egui::Key::L, COMMAND)]);
    assert_eq!(f.ranges(), [0..2, 4..5]);
}
