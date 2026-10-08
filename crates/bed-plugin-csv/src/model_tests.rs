use super::*;
use std::cell::Cell;

fn csv(text: &str) -> Table {
    parse(Arc::from(text.as_bytes()), Some(b','), "data.csv", || false).unwrap()
}

fn apply(table: &Table, edits: Vec<ByteEdit>) -> String {
    assert!(
        edits
            .windows(2)
            .all(|pair| pair[0].range.end <= pair[1].range.start)
    );
    let mut bytes = table.bytes.to_vec();
    for edit in edits.iter().rev() {
        bytes.splice(edit.range.clone(), edit.bytes.iter().copied());
    }
    String::from_utf8(bytes).unwrap()
}

fn change(table: &Table, changes: &[(usize, usize, &str)]) -> String {
    let changes = changes
        .iter()
        .map(|&(row, column, value)| (row, column, value.to_owned()))
        .collect::<Vec<_>>();
    apply(table, table.replace_cells(&changes).unwrap())
}

fn visible(table: &Table, mode: SortMode, descending: bool) -> Vec<usize> {
    table
        .visible_rows(
            false,
            "",
            &[],
            Some(&Sort {
                column: 0,
                descending,
                mode,
            }),
            || false,
        )
        .unwrap()
}

#[test]
fn indexes_quoted_multiline_unicode_and_mixed_terminators() {
    let table = csv("name,note\r\nZoë,\"one, two\nthree \"\"quoted\"\"\"\rblank,\r\n");
    assert_eq!(table.records.len(), 3);
    assert_eq!(table.columns, 2);
    assert_eq!(table.line_ending, b"\r\n");
    assert_eq!(table.cell(1, 0), "Zoë");
    assert_eq!(table.cell(1, 1), "one, two\nthree \"quoted\"");
    assert_eq!(table.cell(2, 1), "");
    assert!(matches!(table.cell(1, 0), Cow::Borrowed(_)));
    assert!(matches!(table.cell(1, 1), Cow::Owned(_)));
}

#[test]
fn empty_files_and_terminators_do_not_add_phantom_records() {
    for (text, count) in [("", 0), ("\n", 1), ("\n\n", 2), ("x\n", 1), ("x\n\n", 2)] {
        let table = csv(text);
        assert_eq!(table.records.len(), count, "{text:?}");
        assert_eq!(table.columns, 1);
    }
    let table = csv("a,b,\nshort\n");
    assert_eq!(table.columns, 3);
    assert_eq!(table.record_columns(0), 3);
    assert_eq!(table.record_columns(1), 1);
    assert_eq!(table.cell(1, 2), "");
    assert_eq!(table.cell(50, 50), "");
}

#[test]
fn detects_supported_delimiters_and_honors_overrides() {
    for delimiter in DELIMITERS {
        let delimiter = char::from(delimiter);
        let text = format!("name{delimiter}count\nA{delimiter}1\nB{delimiter}2\n");
        let table = parse(Arc::from(text.as_bytes()), None, "export.csv", || false).unwrap();
        assert_eq!(char::from(table.delimiter), delimiter);
        assert_eq!(table.cell(2, 1), "2");
    }
    let table = parse(
        Arc::from(&b"a,b\tx\nc,d\ty\n"[..]),
        None,
        "export.tsv",
        || false,
    )
    .unwrap();
    assert_eq!(table.delimiter, b'\t');
    let table = parse(
        Arc::from(&b"a,b,c\tx\nc,d,e\ty\n"[..]),
        None,
        "export.tsv",
        || false,
    )
    .unwrap();
    assert_eq!(table.delimiter, b'\t');
    let table = parse(
        Arc::from(&b"\"a,b\";x\n\"c,d\";y\n"[..]),
        None,
        "export.csv",
        || false,
    )
    .unwrap();
    assert_eq!(table.delimiter, b';');
    let table = parse(Arc::from(&b"one;two"[..]), Some(b','), "export.csv", || {
        false
    })
    .unwrap();
    assert_eq!(table.columns, 1);
    assert_eq!(table.cell(0, 0), "one;two");
    let table = parse(Arc::from(&b"no separators"[..]), None, "EXPORT.TSV", || {
        false
    })
    .unwrap();
    assert_eq!(table.delimiter, b'\t');
}

#[test]
fn header_detection_requires_labels_and_data_contrast() {
    assert!(csv("name,count\nAlice,10\nBob,20\n").detected_header);
    assert!(csv("enabled\ntrue\nfalse\n").detected_header);
    assert!(csv("date\n2026-10-08\n2026-10-09\n").detected_header);
    assert!(!csv("Alice,10\nBob,20\n").detected_header);
    assert!(!csv("name,city\nAlice,Paris\nBob,Rome\n").detected_header);
    assert!(!csv("name,name\nAlice,10\n").detected_header);
    assert!(!csv("name\n").detected_header);
    assert!(!csv("name,\nAlice,10\n").detected_header);
}

#[test]
fn malformed_quotes_unsupported_encoding_and_limits_are_reported() {
    for text in ["\"unfinished", "un\"quoted", "\"closed\"junk"] {
        assert!(parse(Arc::from(text.as_bytes()), Some(b','), "x.csv", || false).is_err());
    }
    assert!(parse(Arc::from(&b"\xff\xfe"[..]), None, "x.csv", || false).is_err());
    assert!(
        parse(Arc::from(&b"a\0b"[..]), Some(b','), "x.csv", || false)
            .unwrap_err()
            .contains("NUL")
    );
    assert!(parse(Arc::from(&b"x"[..]), Some(b':'), "x.csv", || false).is_err());
    let wide = vec![","; MAX_COLUMNS].join("");
    assert!(parse(Arc::from(wide.as_bytes()), Some(b','), "x.csv", || false).is_err());
    assert!(check_index_size(MAX_INDEX_BYTES, 1).is_err());
}

#[test]
fn replacements_preserve_untouched_spelling_and_group_each_record() {
    let table = csv("\"001\",  text  ,\"unchanged\"\r\nnext,\"q\"\"uote\",value");
    let edits = table
        .replace_cells(&[(0, 1, "a,b".into()), (1, 2, "line\n\"quote\"".into())])
        .unwrap();
    assert_eq!(edits.len(), 2);
    assert_eq!(
        apply(&table, edits),
        "\"001\",\"a,b\",\"unchanged\"\r\nnext,\"q\"\"uote\",\"line\n\"\"quote\"\"\""
    );
    assert!(
        table
            .replace_cells(&[(0, 0, "001".into())])
            .unwrap()
            .is_empty()
    );
    assert!(
        table
            .replace_cells(&[(0, 2, "unchanged".into())])
            .unwrap()
            .is_empty()
    );
}

#[test]
fn missing_fields_materialize_only_when_required() {
    let table = csv("a,b,c\nshort\nother\n");
    assert_eq!(
        change(&table, &[(1, 2, "new")]),
        "a,b,c\nshort,,new\nother\n"
    );
    assert_eq!(change(&table, &[(0, 0, "x")]), "x,b,c\nshort\nother\n");
    assert!(
        table
            .replace_cells(&[(1, 2, String::new())])
            .unwrap()
            .is_empty()
    );
}

#[test]
fn clear_preserves_the_final_record_even_without_a_terminator() {
    let table = csv("first\nlast");
    let text = change(&table, &[(1, 0, "")]);
    assert_eq!(text, "first\n\n");
    assert_eq!(csv(&text).records.len(), 2);
    assert_eq!(change(&csv("last"), &[(0, 0, "")]), "\n");
    assert_eq!(
        change(&csv("last"), &[(0, 0, ""), (1, 0, "next")]),
        "\nnext"
    );
}

#[test]
fn range_edits_append_contiguous_rows_and_columns() {
    let table = csv("a,b\nx,y");
    assert_eq!(
        change(
            &table,
            &[(1, 1, "z"), (1, 2, "new"), (2, 0, "r"), (2, 1, "s")]
        ),
        "a,b\nx,z,new\nr,s,"
    );
    let table = csv("a,b\r\n");
    assert_eq!(change(&table, &[(1, 0, "x")]), "a,b\r\nx,\r\n");
    assert_eq!(change(&csv(""), &[(0, 0, "x"), (1, 0, "y")]), "x\ny");
    assert_eq!(change(&csv(""), &[(0, 0, "")]), "\n");
    assert!(table.replace_cells(&[(3, 0, "gap".into())]).is_err());
    assert!(table.replace_cells(&[(0, 4, "gap".into())]).is_err());
    assert!(
        table
            .replace_cells(&[(0, MAX_COLUMNS, "wide".into())])
            .is_err()
    );
}

#[test]
fn insert_rows_before_after_and_into_empty_files() {
    let table = csv("a,b\r\nc,d");
    assert_eq!(
        apply(&table, table.insert_row(0).unwrap()),
        ",\r\na,b\r\nc,d"
    );
    assert_eq!(
        apply(&table, table.insert_row(1).unwrap()),
        "a,b\r\n,\r\nc,d"
    );
    assert_eq!(
        apply(&table, table.insert_row(2).unwrap()),
        "a,b\r\nc,d\r\n,\r\n"
    );
    assert_eq!(apply(&csv(""), csv("").insert_row(0).unwrap()), "\n");
    assert_eq!(
        apply(&csv("one"), csv("one").insert_row(1).unwrap()),
        "one\n\n"
    );
    assert_eq!(
        apply(&csv("one\n"), csv("one\n").insert_row(1).unwrap()),
        "one\n\n"
    );
}

#[test]
fn deleting_rows_preserves_final_newline_and_survivor_order() {
    for (text, rows, expected) in [
        ("a\nb\nc", vec![0], "b\nc"),
        ("a\nb\nc", vec![1], "a\nc"),
        ("a\nb\nc", vec![2], "a\nb"),
        ("a\nb\nc\n", vec![2], "a\nb\n"),
        ("a\nb\nc", vec![2, 0, 0], "b"),
        ("a\nb\nc", vec![0, 1, 2], ""),
        ("a\nb\nc\n", vec![0, 1, 2], ""),
        ("a\n\n", vec![1], "a\n"),
    ] {
        let table = csv(text);
        assert_eq!(
            apply(&table, table.delete_rows(&rows).unwrap()),
            expected,
            "{text:?} {rows:?}"
        );
    }
    let table = csv("one");
    assert!(table.delete_rows(&[1]).is_err());
    assert!(table.delete_rows(&[]).unwrap().is_empty());
}

#[test]
fn byte_order_mark_survives_all_operations() {
    let table = csv("\u{feff}name,count\nAlice,1");
    assert_eq!(table.cell(0, 0), "name");
    assert_eq!(
        change(&table, &[(0, 0, "label")]),
        "\u{feff}label,count\nAlice,1"
    );
    assert_eq!(
        apply(&table, table.delete_rows(&[0, 1]).unwrap()),
        "\u{feff}"
    );
    assert_eq!(
        apply(&table, table.insert_row(0).unwrap()),
        "\u{feff},\nname,count\nAlice,1"
    );
}

#[test]
fn column_operations_preserve_source_fields_and_handle_ragged_rows() {
    let table = csv("name,\"value\",third\n\"001\",x,z\nshort");
    assert_eq!(
        apply(&table, table.insert_column(1, true).unwrap()),
        "name,Column 2,\"value\",third\n\"001\",,x,z\nshort,"
    );
    assert_eq!(
        apply(&table, table.delete_columns(&[1]).unwrap()),
        "name,third\n\"001\",z\nshort"
    );
    assert_eq!(
        apply(&table, table.delete_columns(&[0, 2]).unwrap()),
        "\"value\"\nx\n\n"
    );
    assert!(table.delete_columns(&[0, 1, 2]).is_err());
    assert!(table.delete_columns(&[3]).is_err());
    assert_eq!(
        apply(&csv(""), csv("").insert_column(1, false).unwrap()),
        ","
    );
}

#[test]
fn clipboard_round_trips_tabs_newlines_quotes_and_empty_records() {
    let table = csv("\"a\tb\",\"line\nnext\"\n\"quote\"\"here\",001\n");
    let text = table.copy_tsv(&[0, 1], 0..=1);
    assert_eq!(
        parse_clipboard(&text).unwrap(),
        vec![vec!["a\tb", "line\nnext"], vec!["quote\"here", "001"]]
    );
    assert_eq!(parse_clipboard("").unwrap(), vec![vec![""]]);
    assert_eq!(parse_clipboard("\u{feff}").unwrap(), vec![vec![""]]);
    assert_eq!(
        parse_clipboard("a\tb\r\nc\td\r\n").unwrap(),
        vec![vec!["a", "b"], vec!["c", "d"]]
    );
    let table = csv("a\n\n");
    assert_eq!(
        parse_clipboard(&table.copy_tsv(&[0, 1], 0..=0)).unwrap(),
        vec![vec!["a"], vec![""]]
    );
    assert!(parse_clipboard("\"unclosed").is_err());
}

#[test]
fn filters_are_case_insensitive_and_combine_with_and() {
    let table = csv("name,city,note\nALICE,Paris,x\nBob,Paris,\nAlice,Rome,\nCarol,PARIS\n");
    let filters = vec![
        ColumnFilter {
            column: 1,
            value: "paris".into(),
            op: FilterOp::Equals,
        },
        ColumnFilter {
            column: 2,
            value: "ignored".into(),
            op: FilterOp::Empty,
        },
    ];
    assert_eq!(
        table
            .visible_rows(true, "", &filters, None, || false)
            .unwrap(),
        vec![2, 4]
    );
    assert_eq!(
        table
            .visible_rows(true, "ali", &[], None, || false)
            .unwrap(),
        vec![1, 3]
    );
    assert_eq!(
        table
            .visible_rows(true, "bob", &filters, None, || false)
            .unwrap(),
        vec![2]
    );
    assert_eq!(
        table
            .visible_rows(false, "NAME", &[], None, || false)
            .unwrap(),
        vec![0]
    );
    assert_eq!(
        table
            .visible_rows(
                true,
                "",
                &[ColumnFilter {
                    column: 0,
                    value: "LI".into(),
                    op: FilterOp::Contains
                }],
                None,
                || false
            )
            .unwrap(),
        vec![1, 3]
    );
}

#[test]
fn sorting_is_stable_and_empties_stay_last_in_both_directions() {
    let table = csv("b\nA\na\n\nC\n");
    assert_eq!(visible(&table, SortMode::Text, false), vec![1, 2, 0, 4, 3]);
    assert_eq!(visible(&table, SortMode::Text, true), vec![4, 0, 1, 2, 3]);
    let table = csv("10\n2\n001\n1\n\n-3\n");
    assert_eq!(
        visible(&table, SortMode::Auto, false),
        vec![5, 2, 3, 1, 0, 4]
    );
    assert_eq!(
        visible(&table, SortMode::Auto, true),
        vec![0, 1, 2, 3, 5, 4]
    );
    assert_eq!(
        visible(&table, SortMode::Text, false),
        vec![5, 2, 3, 0, 1, 4]
    );
    assert_eq!(&*table.bytes, b"10\n2\n001\n1\n\n-3\n");
}

#[test]
fn numeric_sort_handles_invalid_and_nonfinite_values_after_numbers() {
    let table = csv("10\nword\n2\nNaN\n\ninf\n");
    assert_eq!(
        visible(&table, SortMode::Number, false),
        vec![2, 0, 5, 3, 1, 4]
    );
    assert_eq!(
        visible(&table, SortMode::Number, true),
        vec![0, 2, 1, 3, 5, 4]
    );
    assert_eq!(
        visible(&table, SortMode::Auto, false),
        vec![0, 2, 5, 3, 1, 4]
    );
    let rows = table.visible_rows(
        false,
        "",
        &[],
        Some(&Sort {
            column: 10,
            descending: false,
            mode: SortMode::Auto,
        }),
        || false,
    );
    assert!(rows.is_err());
}

#[test]
fn parsing_filtering_and_sorting_can_be_cancelled() {
    let text = (0..10_000)
        .map(|i| format!("{i},record {i}\n"))
        .collect::<String>();
    let calls = Cell::new(0);
    let result = parse(Arc::from(text.as_bytes()), Some(b','), "large.csv", || {
        calls.set(calls.get() + 1);
        calls.get() > 30
    });
    assert_eq!(result.unwrap_err(), "Cancelled");
    let quoted = format!("\"{}\"", "\"\"".repeat(50_000));
    let calls = Cell::new(0);
    assert_eq!(
        parse(
            Arc::from(quoted.as_bytes()),
            Some(b','),
            "quoted.csv",
            || {
                calls.set(calls.get() + 1);
                calls.get() > 4
            }
        )
        .unwrap_err(),
        "Cancelled"
    );
    let table = csv(&text);
    let calls = Cell::new(0);
    assert_eq!(
        table
            .visible_rows(false, "", &[], None, || {
                calls.set(calls.get() + 1);
                calls.get() > 4
            })
            .unwrap_err(),
        "Cancelled"
    );
    let calls = Cell::new(0);
    // Let filtering and key construction finish, then interrupt merging.
    assert_eq!(
        table
            .visible_rows(
                false,
                "",
                &[],
                Some(&Sort {
                    column: 0,
                    descending: true,
                    mode: SortMode::Number
                }),
                || {
                    calls.set(calls.get() + 1);
                    calls.get() > 90
                }
            )
            .unwrap_err(),
        "Cancelled"
    );
}

#[test]
fn hundred_thousand_rows_filter_sort_and_edit_keep_source_row_identity() {
    let mut text = String::from("id,group\n");
    for row in 0..100_000 {
        text.push_str(&format!(
            "{row},{}\n",
            if row % 2 == 0 { "keep" } else { "hide" }
        ));
    }
    let table = csv(&text);
    assert_eq!(table.records.len(), 100_001);
    let rows = table
        .visible_rows(
            true,
            "keep",
            &[],
            Some(&Sort {
                column: 0,
                descending: true,
                mode: SortMode::Auto,
            }),
            || false,
        )
        .unwrap();
    assert_eq!(rows.len(), 50_000);
    assert_eq!(rows[0], 99_999);
    assert_eq!(*rows.last().unwrap(), 1);
    let edited = change(&table, &[(rows[0], 1, "changed"), (rows[1], 1, "changed")]);
    assert!(edited.contains("99998,changed\n"));
    assert!(edited.contains("99997,hide\n"));
    assert!(edited.contains("99996,changed\n"));
    assert_eq!(table.cell(99_999, 1), "keep");
}

#[test]
#[ignore = "manual end-to-end structural-edit performance measurement"]
fn benchmark_hundred_thousand_row_column_transaction() {
    use bed_document_session::EditorSession;
    use std::time::Instant;

    let mut text = String::from("id,value\n");
    for row in 0..100_000 {
        text.push_str(&format!("{row},value {row}\n"));
    }
    let table = csv(&text);
    let mut session = EditorSession::new();
    let document = session.create_document(text.as_bytes()).unwrap();
    let start = Instant::now();
    let edits = table.insert_column(1, true).unwrap();
    eprintln!("CSV model: {} edits in {:?}", edits.len(), start.elapsed());
    assert_eq!(edits.len(), 98);
    let start = Instant::now();
    session
        .apply_edits(
            document,
            session.document_revision(document).unwrap(),
            &edits,
        )
        .unwrap();
    eprintln!(
        "CSV session: applied column insertion in {:?}",
        start.elapsed()
    );
    let after = session.snapshot(document).unwrap();
    let mut expected = String::from("id,Column 2,value\n");
    for row in 0..100_000 {
        expected.push_str(&format!("{row},,value {row}\n"));
    }
    assert_eq!(after.bytes, expected.as_bytes());
    let after = parse(after.bytes.into(), Some(b','), "benchmark.csv", || false).unwrap();
    assert_eq!(after.records.len(), 100_001);
    assert_eq!(after.columns, 3);
    assert_eq!(after.cell(0, 1), "Column 2");
    assert_eq!(after.cell(100_000, 2), "value 99999");
    let start = Instant::now();
    session.undo_document(document).unwrap();
    eprintln!(
        "CSV session: undid column insertion in {:?}",
        start.elapsed()
    );
    assert_eq!(session.snapshot(document).unwrap().bytes, text.as_bytes());
}

#[test]
fn bulk_edits_preserve_untouched_gaps_and_mixed_terminators() {
    let mut text = String::new();
    let mut expected = String::new();
    let mut changes = Vec::new();
    let mut deleted = Vec::new();
    let mut surviving = String::new();
    for row in 0..1100 {
        let ending = if row % 2 == 0 { "\r\n" } else { "\n" };
        let original = format!("\"{row:04}\",  value {row}  {ending}");
        text.push_str(&original);
        if row % 2 == 0 {
            changes.push((row, 1, "changed".to_owned()));
            expected.push_str(&format!("\"{row:04}\",changed{ending}"));
            deleted.push(row);
        } else {
            expected.push_str(&original);
            surviving.push_str(&original);
        }
    }
    let table = csv(&text);
    let edits = table.replace_cells(&changes).unwrap();
    assert_eq!(edits.len(), 1);
    assert_eq!(apply(&table, edits), expected);
    let edits = table.delete_rows(&deleted).unwrap();
    assert_eq!(edits.len(), 1);
    assert_eq!(apply(&table, edits), surviving);
    let edits = table.insert_column(1, false).unwrap();
    assert_eq!(edits.len(), 2);
    let after = apply(&table, edits);
    assert_eq!(after, text.replace(",", ",,"));
}

#[test]
fn bulk_edit_blocks_do_not_exceed_one_mib_of_source() {
    let field = "x".repeat(17_000);
    let text = format!("{field},old\n").repeat(129);
    let table = csv(&text);
    let changes = (0..129)
        .map(|row| (row, 1, "new".to_owned()))
        .collect::<Vec<_>>();
    let edits = table.replace_cells(&changes).unwrap();
    assert_eq!(edits.len(), 3);
    assert!(edits.iter().all(|edit| edit.range.len() <= 1024 * 1024));
    assert_eq!(apply(&table, edits), text.replace(",old\n", ",new\n"));
}
