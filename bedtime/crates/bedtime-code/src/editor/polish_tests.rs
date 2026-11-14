// Comparing selections to `[a..b]` is the point.
#![allow(clippy::single_range_in_vec_init)]
use super::carets_tests::{COMMAND, Fixture};
use super::tests::key;
use super::*;
use std::time::{Duration, Instant};

fn analyzed(code: &str) -> Fixture {
    let mut f = Fixture::new(code);
    f.view.language = Some("rust");
    let mut analysis = crate::syntax::AnalysisSession::new();
    syntax::update_analysis(&mut analysis, &f.doc, "rust").unwrap();
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
fn fold_commands_act_at_every_caret_and_keep_carets_visible() {
    let code = "fn a() {\n    1;\n}\nfn b() {\n    2;\n}\n";
    let mut f = analyzed(code);
    let (one, two) = (code.find("1;").unwrap(), code.find("2;").unwrap());
    f.at(&[(one, one), (two, two)]);
    let (fold, unfold) = if cfg!(target_os = "macos") {
        (
            key(egui::Key::OpenBracket, COMMAND | egui::Modifiers::ALT),
            key(egui::Key::CloseBracket, COMMAND | egui::Modifiers::ALT),
        )
    } else {
        (
            key(egui::Key::OpenBracket, COMMAND | egui::Modifiers::SHIFT),
            key(egui::Key::CloseBracket, COMMAND | egui::Modifiers::SHIFT),
        )
    };
    f.send(vec![fold]);
    assert_eq!(f.view.collapsed_regions().len(), 2);
    // Hidden carets go to the end of their fold's first line.
    assert_eq!(f.ranges(), [8..8, 26..26]);
    f.send(vec![unfold]);
    assert!(f.view.collapsed_regions().is_empty());
    f.view.queue_command(EditorCommand::ToggleFold);
    f.send(vec![]);
    assert_eq!(f.view.collapsed_regions().len(), 2);
    f.view.queue_command(EditorCommand::ToggleFold);
    f.send(vec![]);
    assert!(f.view.collapsed_regions().is_empty());
    f.view.queue_command(EditorCommand::FoldAll);
    f.send(vec![]);
    assert_eq!(f.view.collapsed_regions().len(), 2);
    f.view.queue_command(EditorCommand::UnfoldAll);
    f.send(vec![]);
    assert!(f.view.collapsed_regions().is_empty());
}

#[test]
fn occurrences_are_a_selected_word_or_a_short_selection() {
    let mut f = Fixture::new("let total = total + 1;");
    let snapshot = f.doc.snapshot().unwrap();
    // A caret alone, in or beside a word, highlights nothing.
    f.at(&[(6, 6)]);
    assert_eq!(f.view.occurrence(&snapshot), None);
    f.at(&[(9, 9)]);
    assert_eq!(f.view.occurrence(&snapshot), None);
    // A selected word matches whole words; part of one, exactly.
    f.at(&[(4, 9)]);
    assert_eq!(f.view.occurrence(&snapshot), Some(("total".into(), true)));
    f.at(&[(4, 7)]);
    assert_eq!(f.view.occurrence(&snapshot), Some(("tot".into(), false)));
    f.at(&[(10, 12)]);
    assert_eq!(f.view.occurrence(&snapshot), Some(("= ".into(), false)));
    f.at(&[(9, 10)]);
    assert_eq!(f.view.occurrence(&snapshot), None);
}

#[test]
fn tidying_trims_lines_and_ends_with_the_text_s_own_newline() {
    let tidy = |text: &str, trim, newline| {
        let mut buffer = TextBuffer::from(text);
        for edit in saving::tidy_edits(&buffer.clone(), trim, newline) {
            edit.apply(&mut buffer).unwrap();
        }
        buffer.to_string()
    };
    assert_eq!(tidy("a  \nb\t\n  \nc ", true, true), "a\nb\n\nc\n");
    assert_eq!(tidy("a \r\nb", true, true), "a\r\nb\r\n");
    assert_eq!(tidy("a \nb", false, true), "a \nb\n");
    assert_eq!(tidy("a \nb", true, false), "a\nb");
    assert_eq!(tidy("", true, true), "");
    assert_eq!(tidy("done\n", true, true), "done\n");
}
