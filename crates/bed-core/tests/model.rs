// Translated fixture-for-fixture from nealmick/ned tests/editor/
// text_model_data_test.cpp, text_model_test.cpp, model_edit_operation_test.cpp,
// and editable_text_model_test.cpp at UPSTREAM_REVISION. MIT; see LICENSE.
#[path = "../../../tests/support/mod.rs"]
mod support;
use support::text_model::*;

fn bytes(lines: &[&str]) -> Vec<Vec<u8>> {
    lines.iter().map(|s| s.as_bytes().to_vec()).collect()
}

#[test]
fn text_model_data_one_line() {
    let m = TextModel::create("Hello world!");
    assert!(m.get_eol() == b"\n" || m.get_eol() == b"\r\n");
    assert_eq!(m.get_lines_content(), bytes(&["Hello world!"]));
}
#[test]
fn text_model_data_multiline() {
    let m = TextModel::create("Hello,\r\ndear friend\nHow\rare\r\nyou?");
    assert_eq!(m.get_eol(), b"\r\n");
    assert_eq!(
        m.get_lines_content(),
        bytes(&["Hello,", "dear friend", "How", "are", "you?"])
    );
}
#[test]
fn text_model_data_non_basic_ascii() {
    let m = TextModel::create("Hello,\nZürich");
    assert_eq!(m.get_eol(), b"\n");
    assert_eq!(m.get_lines_content(), bytes(&["Hello,", "Zürich"]));
}
#[test]
fn text_model_data_rtl_hebrew() {
    let m = TextModel::create("Hello,\nזוהי עובדה מבוססת שדעתו");
    assert_eq!(m.get_eol(), b"\n");
    assert_eq!(m.get_line_count(), 2);
    assert_eq!(m.get_line_content(1), b"Hello,");
    assert_eq!(m.get_line_content(2), "זוהי עובדה מבוססת שדעתו".as_bytes());
}
#[test]
fn text_model_data_rtl_arabic() {
    let m = TextModel::create("Hello,\nهناك حقيقة مثبتة منذ زمن طويل");
    assert_eq!(m.get_eol(), b"\n");
    assert_eq!(m.get_line_count(), 2);
    assert_eq!(m.get_line_content(1), b"Hello,");
}

#[test]
fn range_length_crlf() {
    let m = TextModel::create("My First Line\r\nMy Second Line\r\nMy Third Line");
    for (r, expected) in [
        (Range(1, 1, 1, 1), ""),
        (Range(1, 1, 1, 2), "M"),
        (Range(1, 2, 1, 3), "y"),
        (Range(1, 1, 1, 14), "My First Line"),
        (Range(1, 1, 2, 1), "My First Line\r\n"),
        (Range(1, 2, 2, 1), "y First Line\r\n"),
        (Range(1, 2, 2, 2), "y First Line\r\nM"),
        (Range(1, 2, 2, 1000), "y First Line\r\nMy Second Line"),
        (Range(1, 2, 3, 1), "y First Line\r\nMy Second Line\r\n"),
        (
            Range(1, 2, 3, 1000),
            "y First Line\r\nMy Second Line\r\nMy Third Line",
        ),
        (
            Range(1, 1, 1000, 1000),
            "My First Line\r\nMy Second Line\r\nMy Third Line",
        ),
    ] {
        assert_eq!(
            m.get_value_length_in_range(r, EndOfLinePreference::TextDefined),
            expected.len() as i32,
            "{r:?}"
        );
    }
}
#[test]
fn range_length_lf() {
    let m = TextModel::create("My First Line\nMy Second Line\nMy Third Line");
    for (r, expected) in [
        (Range(1, 1, 1, 1), ""),
        (Range(1, 1, 1, 2), "M"),
        (Range(1, 1, 1, 14), "My First Line"),
        (Range(1, 1, 2, 1), "My First Line\n"),
        (Range(1, 2, 2, 1), "y First Line\n"),
        (Range(1, 2, 2, 2), "y First Line\nM"),
        (
            Range(1, 1, 1000, 1000),
            "My First Line\nMy Second Line\nMy Third Line",
        ),
    ] {
        assert_eq!(
            m.get_value_length_in_range(r, EndOfLinePreference::TextDefined),
            expected.len() as i32,
            "{r:?}"
        );
    }
}
#[test]
fn range_length_different_eol() {
    let m = TextModel::create("My First Line\r\nMy Second Line\r\nMy Third Line");
    for (preference, expected) in [
        (EndOfLinePreference::TextDefined, "My First Line\r\n"),
        (EndOfLinePreference::CrLf, "My First Line\r\n"),
        (EndOfLinePreference::Lf, "My First Line\n"),
    ] {
        assert_eq!(
            m.get_value_length_in_range(Range(1, 1, 2, 1), preference),
            expected.len() as i32
        );
    }
    assert_eq!(
        m.get_value_length_in_range(Range(1, 1, 1000, 1000), EndOfLinePreference::Lf),
        "My First Line\nMy Second Line\nMy Third Line".len() as i32
    );
    let m = TextModel::create("My First Line\nMy Second Line\nMy Third Line");
    for (preference, expected) in [
        (EndOfLinePreference::TextDefined, "My First Line\n"),
        (EndOfLinePreference::Lf, "My First Line\n"),
        (EndOfLinePreference::CrLf, "My First Line\r\n"),
    ] {
        assert_eq!(
            m.get_value_length_in_range(Range(1, 1, 2, 1), preference),
            expected.len() as i32
        );
    }
}
#[test]
fn validate_position() {
    let m = TextModel::create("line one\nline two");
    for (p, expected) in [
        ((0, 0), (1, 1)),
        ((0, 1), (1, 1)),
        ((1, 1), (1, 1)),
        ((1, 2), (1, 2)),
        ((1, 30), (1, 9)),
        ((2, 0), (2, 1)),
        ((2, 1), (2, 1)),
        ((2, 2), (2, 2)),
        ((2, 30), (2, 9)),
        ((3, 0), (2, 9)),
        ((3, 1), (2, 9)),
        ((3, 30), (2, 9)),
        ((30, 30), (2, 9)),
    ] {
        assert_eq!(m.validate_position(p), expected, "{p:?}");
    }
}
#[test]
fn validate_position_multibyte() {
    let m = TextModel::create("a📚b");
    for (column, expected) in [
        (1, 1),
        (2, 2),
        (3, 2),
        (4, 2),
        (5, 2),
        (6, 6),
        (7, 7),
        (30, 7),
    ] {
        assert_eq!(m.validate_position((1, column)), (1, expected));
    }
}
#[test]
fn modify_position() {
    let m = TextModel::create("line one\nline two");
    for (p, offset, expected) in [
        ((1, 1), 0, (1, 1)),
        ((0, 0), 0, (1, 1)),
        ((30, 1), 0, (2, 9)),
        ((1, 1), 17, (2, 9)),
        ((1, 1), 1, (1, 2)),
        ((1, 1), 3, (1, 4)),
        ((1, 2), 10, (2, 3)),
        ((1, 5), 13, (2, 9)),
        ((1, 2), 16, (2, 9)),
        ((2, 9), -17, (1, 1)),
        ((1, 2), -1, (1, 1)),
        ((1, 4), -3, (1, 1)),
        ((2, 3), -10, (1, 2)),
        ((2, 9), -13, (1, 5)),
        ((2, 9), -16, (1, 2)),
        ((1, 2), 17, (2, 9)),
        ((1, 2), 100, (2, 9)),
        ((1, 2), -2, (1, 1)),
        ((1, 2), -100, (1, 1)),
        ((2, 2), -100, (1, 1)),
        ((2, 9), -18, (1, 1)),
    ] {
        assert_eq!(m.modify_position(p, offset), expected, "{p:?}+{offset}");
    }
}
#[test]
fn first_non_whitespace_column() {
    let m = TextModel::create_from_lines(&[
        "asd", " asd", "\tasd", "  asd", "\t\tasd", " ", "  ", "\t", "\t\t", "  \tasd", "", "",
    ]);
    for (i, expected) in [1, 2, 2, 3, 3, 0, 0, 0, 0, 4, 0, 0].into_iter().enumerate() {
        assert_eq!(
            m.get_line_first_non_whitespace_column(i as i32 + 1),
            expected
        );
    }
}
#[test]
fn last_non_whitespace_column() {
    let m = TextModel::create_from_lines(&[
        "asd", "asd ", "asd\t", "asd  ", "asd\t\t", " ", "  ", "\t", "\t\t", "asd  \t", "", "",
    ]);
    for (i, expected) in [4, 4, 4, 4, 4, 0, 0, 0, 0, 4, 0, 0].into_iter().enumerate() {
        assert_eq!(
            m.get_line_last_non_whitespace_column(i as i32 + 1),
            expected
        );
    }
}
#[test]
fn invalid_range_issue_50471() {
    let m = TextModel::create("My First Line\r\nMy Second Line\r\nMy Third Line");
    assert_eq!(
        m.get_value_in_range(Range(1, 1, 1, 3), EndOfLinePreference::TextDefined),
        b"My"
    );
}
#[test]
fn set_value_resets_buffer() {
    let mut m = TextModel::create("hello world!");
    m.set_value("Hello,\nזוהי עובדה מבוססת שדעתו");
    assert_eq!(m.get_line_count(), 2);
    m.set_value("hello world!");
    assert_eq!(m.get_line_count(), 1);
    assert_eq!(m.get_value(), b"hello world!");
}

const BASE: &[&str] = &[
    "My First Line",
    "\t\tMy Second Line",
    "    Third Line",
    "",
    "1",
];
fn assert_single_edit(op: SingleEditOperation, edited: &[&str]) {
    let mut model = TextModel::create("My First Line\r\n\t\tMy Second Line\n    Third Line\n\r\n1");
    let inverse = model.apply_edits(std::slice::from_ref(&op), true);
    assert_eq!(model.get_line_count(), edited.len() as i32);
    for (i, line) in edited.iter().enumerate() {
        assert_eq!(model.get_line_content(i as i32 + 1), line.as_bytes());
    }
    let original = model.apply_edits(&inverse, true);
    assert_eq!(model.get_line_count(), 5);
    for (i, line) in BASE.iter().enumerate() {
        assert_eq!(model.get_line_content(i as i32 + 1), line.as_bytes());
    }
    assert_eq!(original.len(), 1);
    let strip_eol = |s: &[u8]| {
        s.iter()
            .copied()
            .filter(|&b| b != b'\r')
            .collect::<Vec<_>>()
    };
    assert_eq!(strip_eol(&original[0].text), strip_eol(&op.text));
    assert_eq!(original[0].range.ordered(), op.range.ordered());
}
#[test]
fn single_edit_insert_inline() {
    assert_single_edit(
        create_single_edit_op("a", 1, 1, None),
        &["aMy First Line", BASE[1], BASE[2], BASE[3], BASE[4]],
    );
}
#[test]
fn single_edit_replace_inline_1() {
    assert_single_edit(
        create_single_edit_op(" incredibly awesome", 1, 3, None),
        &[
            "My incredibly awesome First Line",
            BASE[1],
            BASE[2],
            BASE[3],
            BASE[4],
        ],
    );
}
#[test]
fn single_edit_replace_inline_2() {
    assert_single_edit(
        create_single_edit_op(" with text at the end.", 1, 14, None),
        &[
            "My First Line with text at the end.",
            BASE[1],
            BASE[2],
            BASE[3],
            BASE[4],
        ],
    );
}
#[test]
fn single_edit_replace_inline_3() {
    assert_single_edit(
        create_single_edit_op("My new First Line.", 1, 1, Some((1, 14))),
        &["My new First Line.", BASE[1], BASE[2], BASE[3], BASE[4]],
    );
}
#[test]
fn single_edit_replace_multiline_1() {
    assert_single_edit(
        create_single_edit_op("My new First Line.", 1, 1, Some((3, 15))),
        &["My new First Line.", BASE[3], BASE[4]],
    );
}
#[test]
fn single_edit_replace_multiline_2() {
    assert_single_edit(
        create_single_edit_op("My new First Line.", 1, 2, Some((3, 15))),
        &["MMy new First Line.", BASE[3], BASE[4]],
    );
}
#[test]
fn single_edit_replace_multiline_3() {
    assert_single_edit(
        create_single_edit_op("My new First Line.", 1, 2, Some((3, 2))),
        &["MMy new First Line.   Third Line", BASE[3], BASE[4]],
    );
}
#[test]
fn single_edit_insert_multiline() {
    assert_single_edit(
        create_single_edit_op("1\n2\n3\n4\n", 1, 1, None),
        &[
            "1", "2", "3", "4", BASE[0], BASE[1], BASE[2], BASE[3], BASE[4],
        ],
    );
}

fn assert_edits(original: &[&str], edits: &[SingleEditOperation], expected: &[&str]) {
    let mut model = TextModel::create_from_lines(original);
    model.set_eol("\n");
    let inverse = model.apply_edits(edits, true);
    assert_eq!(model.get_line_count(), expected.len() as i32);
    for (i, line) in expected.iter().enumerate() {
        assert_eq!(model.get_line_content(i as i32 + 1), line.as_bytes());
    }
    model.apply_edits(&inverse, false);
    assert_eq!(model.get_line_count(), original.len() as i32);
    for (i, line) in original.iter().enumerate() {
        assert_eq!(model.get_line_content(i as i32 + 1), line.as_bytes());
    }
}
macro_rules! base_edit_case {
    ($name:ident, $range:expr, $text:expr, $expected:expr) => {
        #[test]
        fn $name() {
            let Range(sl, sc, el, ec) = $range;
            assert_edits(BASE, &[edit_op(sl, sc, el, ec, $text)], $expected);
        }
    };
}
base_edit_case!(apply_edits_empty, Range(1, 1, 1, 1), &[""], BASE);
base_edit_case!(
    apply_edits_insert_inline_1,
    Range(1, 1, 1, 1),
    &["foo "],
    &["foo My First Line", BASE[1], BASE[2], BASE[3], BASE[4]]
);
base_edit_case!(
    apply_edits_insert_inline_2,
    Range(1, 3, 1, 3),
    &[" foo"],
    &["My foo First Line", BASE[1], BASE[2], BASE[3], BASE[4]]
);
base_edit_case!(
    apply_edits_insert_newline,
    Range(1, 4, 1, 4),
    &["", ""],
    &["My ", "First Line", BASE[1], BASE[2], BASE[3], BASE[4]]
);
base_edit_case!(
    apply_edits_insert_one_newline,
    Range(1, 3, 1, 3),
    &[" new line", "No longer"],
    &[
        "My new line",
        "No longer First Line",
        BASE[1],
        BASE[2],
        BASE[3],
        BASE[4]
    ]
);
base_edit_case!(
    apply_edits_insert_two_newlines,
    Range(1, 3, 1, 3),
    &[" new line", "One more line in the middle", "No longer"],
    &[
        "My new line",
        "One more line in the middle",
        "No longer First Line",
        BASE[1],
        BASE[2],
        BASE[3],
        BASE[4]
    ]
);
base_edit_case!(
    apply_edits_insert_many_newlines,
    Range(1, 3, 1, 3),
    &["", "", "", "", ""],
    &[
        "My",
        "",
        "",
        "",
        " First Line",
        BASE[1],
        BASE[2],
        BASE[3],
        BASE[4]
    ]
);
base_edit_case!(
    apply_edits_delete_inline_1,
    Range(1, 1, 1, 2),
    &[""],
    &["y First Line", BASE[1], BASE[2], BASE[3], BASE[4]]
);
base_edit_case!(
    apply_edits_delete_inline_2,
    Range(1, 1, 1, 3),
    &["a"],
    &["a First Line", BASE[1], BASE[2], BASE[3], BASE[4]]
);
base_edit_case!(
    apply_edits_delete_line,
    Range(1, 1, 1, 14),
    &[""],
    &["", BASE[1], BASE[2], BASE[3], BASE[4]]
);
base_edit_case!(
    apply_edits_delete_two_lines,
    Range(1, 4, 2, 6),
    &[""],
    &["My Second Line", BASE[2], BASE[3], BASE[4]]
);
base_edit_case!(
    apply_edits_delete_many_lines,
    Range(1, 4, 3, 5),
    &[""],
    &["My Third Line", BASE[3], BASE[4]]
);
base_edit_case!(apply_edits_delete_all, Range(1, 1, 5, 2), &[""], &[""]);
#[test]
fn apply_edits_two_unrelated() {
    assert_edits(
        &[BASE[0], BASE[1], BASE[2], BASE[3], "123"],
        &[edit_op(2, 1, 2, 3, &["\t"]), edit_op(3, 1, 3, 5, &[""])],
        &[BASE[0], "\tMy Second Line", "Third Line", "", "123"],
    );
}
#[test]
fn apply_edits_two_on_one_line() {
    assert_edits(
        &[
            "\t\tfirst\t    ",
            "\t\tsecond line",
            "\tthird line",
            "fourth line",
            "\t\t<!@#fifth#@!>\t\t",
        ],
        &[edit_op(5, 3, 5, 7, &[""]), edit_op(5, 12, 5, 16, &[""])],
        &[
            "\t\tfirst\t    ",
            "\t\tsecond line",
            "\tthird line",
            "fourth line",
            "\t\tfifth\t\t",
        ],
    );
}
#[test]
fn apply_edits_issue_19872() {
    assert_edits(
        &["something", " A", "", " B", "something else"],
        &[edit_op(2, 1, 2, 2, &[""]), edit_op(3, 1, 4, 2, &[""])],
        &["something", "A", "B", "something else"],
    );
}
#[test]
fn apply_edits_issue_19872_inverse() {
    assert_edits(
        &["something", "A", "B", "something else"],
        &[edit_op(2, 1, 2, 1, &[" "]), edit_op(3, 1, 3, 1, &["", " "])],
        &["something", " A", "", " B", "something else"],
    );
}
#[test]
fn apply_edits_last_op_noop() {
    assert_edits(
        BASE,
        &[edit_op(1, 1, 1, 2, &[""]), edit_op(4, 1, 4, 1, &[""])],
        &["y First Line", BASE[1], BASE[2], BASE[3], BASE[4]],
    );
}
#[test]
fn apply_edits_many() {
    assert_edits(
        &["{\"x\" : 1}"],
        &[
            edit_op(1, 2, 1, 2, &["", "  "]),
            edit_op(1, 5, 1, 6, &[""]),
            edit_op(1, 9, 1, 9, &["", ""]),
        ],
        &["{", "  \"x\": 1", "}"],
    );
}
#[test]
fn apply_edits_many_reversed() {
    assert_edits(
        &["{", "  \"x\": 1", "}"],
        &[
            edit_op(1, 2, 2, 3, &[""]),
            edit_op(2, 6, 2, 6, &[" "]),
            edit_op(2, 9, 3, 1, &[""]),
        ],
        &["{\"x\" : 1}"],
    );
}
#[test]
fn apply_edits_utf8_1() {
    assert_edits(
        &["📚some", "very nice", "text"],
        &[edit_op(1, 2, 1, 2, &["a"])],
        &["a📚some", "very nice", "text"],
    );
}
#[test]
fn apply_edits_utf8_2() {
    assert_edits(
        &["📚some", "very nice", "text"],
        &[edit_op(1, 1, 1, 5, &["a"])],
        &["asome", "very nice", "text"],
    );
}
#[test]
fn apply_edits_issue_47733_unicode_undo() {
    assert_edits(&["'👁'"], &[edit_op(1, 1, 1, 1, &["a"])], &["a'👁'"]);
}
