// Comparing selections to `[a..b]` is the point.
#![allow(clippy::single_range_in_vec_init)]
use super::carets_tests::{COMMAND, Fixture};
use super::tests::key;
use super::*;
use std::time::{Duration, Instant};

fn fixture(text: &str, language: Option<&'static str>) -> Fixture {
    let mut f = Fixture::new(text);
    f.view.language = language;
    f
}

fn line_comment(f: &mut Fixture) {
    f.send(vec![key(egui::Key::Slash, COMMAND)]);
}

fn block_comment(f: &mut Fixture) {
    f.send(vec![key(
        egui::Key::Slash,
        COMMAND | egui::Modifiers::SHIFT,
    )]);
}

#[test]
fn line_comments_skip_blank_lines_align_and_toggle_back_in_one_step() {
    let code = "fn a() {\n    x;\n\n    y;\n}";
    let mut f = fixture(code, Some("rust"));
    f.at(&[(9, 23)]);
    line_comment(&mut f);
    assert_eq!(f.text(), "fn a() {\n    // x;\n\n    // y;\n}");
    line_comment(&mut f);
    assert_eq!(f.text(), code);
    line_comment(&mut f);
    f.send(vec![key(egui::Key::Z, COMMAND)]);
    assert_eq!(f.text(), code);

    // The least indentation; partly commented lines all get comments.
    let mut f = fixture("  a\n    b", Some("rust"));
    f.at(&[(0, 9)]);
    line_comment(&mut f);
    assert_eq!(f.text(), "  // a\n  //   b");
    line_comment(&mut f);
    assert_eq!(f.text(), "  a\n    b");
    let mut f = fixture("// a\nb", Some("rust"));
    f.at(&[(0, 6)]);
    line_comment(&mut f);
    assert_eq!(f.text(), "// // a\n// b");

    // A caret at the text goes after the marker; several carets, one each.
    let mut f = fixture("  x\ny", Some("rust"));
    f.at(&[(2, 2), (4, 4)]);
    line_comment(&mut f);
    assert_eq!(f.text(), "  // x\n// y");
    assert_eq!(f.ranges(), [5..5, 10..10]);
    let mut f = fixture("x = 1", Some("python"));
    f.at(&[(0, 0)]);
    line_comment(&mut f);
    assert_eq!(f.text(), "# x = 1");
}

#[test]
fn block_only_languages_wrap_lines_and_unknown_ones_are_left_alone() {
    let mut f = fixture("  a { }\n  b { }  ", Some("css"));
    f.at(&[(3, 12)]);
    line_comment(&mut f);
    assert_eq!(f.text(), "  /* a { }\n  b { } */  ");
    line_comment(&mut f);
    assert_eq!(f.text(), "  a { }\n  b { }  ");
    for language in [None, Some("json")] {
        let mut f = fixture("{}", language);
        f.at(&[(0, 0)]);
        line_comment(&mut f);
        block_comment(&mut f);
        assert_eq!(f.text(), "{}");
    }
}

#[test]
fn block_comments_wrap_and_unwrap_keeping_the_inside_selected() {
    let mut f = fixture("a = x + y;", Some("rust"));
    f.at(&[(4, 9)]);
    block_comment(&mut f);
    assert_eq!(f.text(), "a = /* x + y */;");
    assert_eq!(f.ranges(), [7..12]);
    f.at(&[(4, 15)]);
    block_comment(&mut f);
    assert_eq!(f.text(), "a = x + y;");
    assert_eq!(f.ranges(), [4..9]);
    // At a caret: an empty comment, the caret inside.
    let mut f = fixture("f()", Some("rust"));
    f.at(&[(2, 2)]);
    block_comment(&mut f);
    assert_eq!(f.text(), "f(/*  */)");
    assert_eq!(f.ranges(), [5..5]);
}

#[test]
fn embedded_languages_use_their_own_comments() {
    let code = "<p>a</p>\n<script>\nlet x = 1;\n</script>\n";
    let mut f = fixture(code, Some("html"));
    let mut analysis = crate::syntax::AnalysisSession::new();
    syntax::update_analysis(&mut analysis, &f.doc, "html").unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while analysis.status() == &crate::syntax::AnalysisStatus::Pending {
        analysis.poll();
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    f.view.set_analysis(&analysis);
    let script = code.find("let").unwrap();
    f.at(&[(1, 1), (script, script)]);
    line_comment(&mut f);
    assert_eq!(
        f.text(),
        "<!-- <p>a</p> -->\n<script>\n// let x = 1;\n</script>\n"
    );
}
