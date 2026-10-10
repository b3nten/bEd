// Translated from nealmick/ned tests/editor/{capture_map,highlight_queries,
// highlight_service}_test.cpp at 2d3e9b53b0ebc44c6da6901ff242a95edaa4a1ff. License in LICENSE/NOTICE.
use bed_editing::{
    editor_operations::{EditorOperations, OpKind, TextOp},
    editor_state::EditorState,
};
use bed_highlight::{
    capture_map::{
        ThemeSlot, capture_priority, theme_key_for_capture, theme_slot_for_capture,
        theme_slot_for_key,
    },
    highlight_service::{EditorHighlight, PRIME_QUERY_LINES},
    tree_sitter::{ThemeColors, TreeSitter},
};
use std::time::{Duration, Instant};

fn document(bytes: &[u8], path: &str, language: &str) -> EditorState {
    let mut state = EditorState::new();
    state.set_from_bytes(bytes);
    state.path = path.to_owned();
    state.language_id = language.to_owned();
    state
}
fn many_c_lines(n: usize) -> Vec<u8> {
    let mut text = String::new();
    for i in 0..n {
        text.push_str(&format!("int x{i} = 0;\n"));
    }
    text.into_bytes()
}
fn wait_spans(
    hl: &mut EditorHighlight,
    state: &EditorState,
    ops: &EditorOperations,
    row: i32,
) -> bool {
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        hl.poll(state, ops);
        if !hl.spans_for_line(row).is_empty() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    hl.poll(state, ops);
    !hl.spans_for_line(row).is_empty()
}
#[test]
fn capture_priority_specific_roles_beat_bare_variable() {
    assert!(capture_priority("variable") < capture_priority("function"));
    assert!(capture_priority("variable") < capture_priority("type"));
    assert!(capture_priority("variable") < capture_priority("keyword"));
    assert!(capture_priority("property") < capture_priority("function"));
    assert!(capture_priority("function") <= capture_priority("function.builtin"));
    assert!(capture_priority("punctuation.bracket") < capture_priority("function"));
}
#[test]
fn theme_key_for_capture_hierarchical_map() {
    for (capture, key) in [
        ("keyword", "keyword"),
        ("keyword.import", "keyword"),
        ("function", "function"),
        ("function.method", "function"),
        ("function.method.call", "function"),
        ("type.builtin", "special"),
        ("string.escape", "special"),
        ("comment.documentation", "comment"),
        ("variable.parameter", "parameter"),
        ("variable.member", "property"),
        ("property", "property"),
        ("constructor", "type"),
        ("constant.builtin", "special"),
        ("constant", "constant"),
        ("operator", "operator"),
        ("punctuation.bracket", "punctuation"),
        ("tag", "type"),
        ("unknown.capture.xyz", "text"),
        ("default", "text"),
    ] {
        assert_eq!(theme_key_for_capture(capture), key, "{capture}");
    }
    assert!(capture_priority("function.macro") > capture_priority("keyword.exception"));
}
#[test]
fn theme_slot_for_key_matches_theme_json_keys() {
    for (key, slot) in [
        ("comment", ThemeSlot::Comment),
        ("keyword", ThemeSlot::Keyword),
        ("string", ThemeSlot::String),
        ("function", ThemeSlot::Function),
        ("operator", ThemeSlot::Operator),
        ("text", ThemeSlot::Text),
        ("nope", ThemeSlot::Text),
    ] {
        assert_eq!(theme_slot_for_key(key), slot);
    }
    for (capture, slot) in [
        ("keyword.import", ThemeSlot::Keyword),
        ("function.method", ThemeSlot::Function),
        ("type.builtin", ThemeSlot::Special),
        ("variable.parameter", ThemeSlot::Parameter),
        ("unknown.capture.xyz", ThemeSlot::Text),
    ] {
        assert_eq!(theme_slot_for_capture(capture), slot);
    }
}
#[test]
fn highlight_queries_compile_for_all_languages() {
    TreeSitter::compile_all_queries().unwrap();
}
#[test]
fn highlight_snippet_colors_a_cpp_signature() {
    let spans = TreeSitter::highlight_snippet("cpp", b"int foo(int x);");
    assert!(!spans.is_empty());
    assert!(spans.iter().flatten().any(|s| s.slot != ThemeSlot::Text));
}
#[test]
fn full_rebuild_colors_the_prime_window_before_poll() {
    let state = document(&many_c_lines(2000), "test.c", "c");
    let mut ops = EditorOperations::new();
    let mut hl = EditorHighlight::new();
    hl.reset_for_document(&state, state.line_count() as usize);
    hl.highlight_content(&state, &mut ops);
    assert!(wait_spans(&mut hl, &state, &ops, 0));
    hl.reset_for_document(&state, state.line_count() as usize);
    hl.highlight_content(&state, &mut ops);
    assert!(!hl.spans_for_line(0).is_empty());
    assert_eq!(hl.spans_for_line(0)[0].slot, ThemeSlot::Special);
    assert!(!hl.spans_for_line(PRIME_QUERY_LINES - 1).is_empty());
    assert!(hl.spans_for_line(PRIME_QUERY_LINES).is_empty());
    assert!(wait_spans(&mut hl, &state, &ops, PRIME_QUERY_LINES));
    assert!(!hl.spans_for_line(PRIME_QUERY_LINES).is_empty());
}
#[test]
fn insert_at_end_of_span_keeps_slot_before_recolor() {
    let mut text = b"// hi\n".to_vec();
    text.extend(many_c_lines(2000));
    let mut state = document(&text, "test.c", "c");
    let mut ops = EditorOperations::new();
    let mut hl = EditorHighlight::new();
    hl.reset_for_document(&state, state.line_count() as usize);
    hl.highlight_content(&state, &mut ops);
    assert!(wait_spans(&mut hl, &state, &ops, 0));
    assert_eq!(
        hl.spans_for_line(0).last().unwrap().slot,
        ThemeSlot::Comment
    );
    let col = state.line_length(0);
    assert!(
        ops.apply(
            &mut state,
            &TextOp {
                kind: OpKind::Insert,
                row: 0,
                column: col,
                text: b"x".to_vec(),
                length: 0
            }
        )
        .ok
    );
    hl.highlight_content(&state, &mut ops);
    let after = hl.spans_for_line(0);
    assert!(!after.is_empty());
    assert_eq!(after.last().unwrap().slot, ThemeSlot::Comment);
    assert!(after.last().unwrap().end > col);
}
#[test]
fn theme_swap_remaps_palette_without_dropping_spans() {
    let state = document(b"int main(void) { return 0; }\n", "test.c", "c");
    let mut ops = EditorOperations::new();
    let mut hl = EditorHighlight::new();
    hl.reset_for_document(&state, state.line_count() as usize);
    hl.highlight_content(&state, &mut ops);
    assert!(wait_spans(&mut hl, &state, &ops, 0));
    let slot = hl.spans_for_line(0)[0].slot;
    let generation = hl.visual_generation();
    hl.force_color_update(ThemeColors::default(), &state, &mut ops);
    assert!(hl.visual_generation() > generation);
    assert!(!hl.spans_for_line(0).is_empty());
    assert_eq!(hl.spans_for_line(0)[0].slot, slot);
}
#[test]
fn language_switch_replaces_c_spans_with_python_spans() {
    let mut state = document(b"def foo():\n    return 1\n", "test.py", "c");
    let mut ops = EditorOperations::new();
    let mut hl = EditorHighlight::new();
    hl.reset_for_document(&state, state.line_count() as usize);
    hl.highlight_content(&state, &mut ops);
    assert!(wait_spans(&mut hl, &state, &ops, 0));
    state.language_id = "py".into();
    ops.bump_generation();
    hl.highlight_content(&state, &mut ops);
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut saw_keyword = false;
    while Instant::now() < deadline {
        hl.poll(&state, &ops);
        saw_keyword |= hl
            .spans_for_line(0)
            .iter()
            .any(|s| s.slot == ThemeSlot::Keyword);
        if saw_keyword {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(saw_keyword);
}
#[test]
fn start_background_prewarm_compiles_shipped_queries() {
    TreeSitter::start_background_prewarm();
    let ts = TreeSitter::new();
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if ts.query_ready("cpp") && ts.query_ready("json") && ts.query_ready("py") {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(ts.query_ready("cpp"));
    assert!(ts.query_ready("json"));
    assert!(ts.query_ready("py"));
}
#[test]
fn empty_buffer_stays_spannless() {
    let state = document(b"", "empty.c", "c");
    let mut ops = EditorOperations::new();
    let mut hl = EditorHighlight::new();
    hl.reset_for_document(&state, state.line_count() as usize);
    hl.highlight_content(&state, &mut ops);
    hl.poll(&state, &ops);
    assert!(hl.spans_for_line(0).is_empty());
    assert!(hl.spans_for_line(1).is_empty());
}

#[test]
fn capture_spans_match_pinned_upstream_for_all_seventeen_languages() {
    let fixtures: Vec<serde_json::Value> = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/highlight.json"
    )))
    .unwrap();
    assert_eq!(fixtures.len(), 17);
    for fixture in fixtures {
        let language = fixture["language"].as_str().unwrap();
        let source = fixture["source"].as_str().unwrap();
        let expected: Vec<Vec<[i32; 3]>> =
            serde_json::from_value(fixture["spans"].clone()).unwrap();
        let got: Vec<Vec<[i32; 3]>> = TreeSitter::highlight_snippet(language, source.as_bytes())
            .iter()
            .map(|line| {
                line.iter()
                    .map(|s| [s.start, s.end, s.slot as i32])
                    .collect()
            })
            .collect();
        assert!(got.iter().flatten().any(|s| s[2] != 0), "{}", language);
        assert_eq!(got, expected, "{language}");
    }
}

#[test]
fn incremental_multi_line_edits_match_full_snapshot_captures() {
    TreeSitter::compile_all_queries().unwrap();
    let mut state = document(
        b"// caf\xc3\xa9\nint one = 1;\nint two = 2;\n",
        "edits.c",
        "c",
    );
    let mut ops = EditorOperations::new();
    let mut hl = EditorHighlight::new();
    hl.reset_for_document(&state, state.line_count() as usize);
    hl.highlight_content(&state, &mut ops);
    for operation in [
        TextOp {
            kind: OpKind::Insert,
            row: 1,
            column: 0,
            text: b"/* heading */\n".to_vec(),
            length: 0,
        },
        TextOp {
            kind: OpKind::Insert,
            row: 0,
            column: 8,
            text: b"\xe2\x9c\x93".to_vec(),
            length: 0,
        },
        TextOp {
            kind: OpKind::Delete,
            row: 1,
            column: 0,
            text: Vec::new(),
            length: 14,
        },
        TextOp {
            kind: OpKind::Delete,
            row: 1,
            column: 3,
            text: Vec::new(),
            length: 12,
        },
        TextOp {
            kind: OpKind::Insert,
            row: 1,
            column: 3,
            text: b" main() {\n return 42;\n}".to_vec(),
            length: 0,
        },
    ] {
        assert!(ops.apply(&mut state, &operation).ok);
        hl.highlight_content(&state, &mut ops);
        let got: Vec<_> = (0..state.line_count())
            .map(|row| hl.spans_for_line(row).to_vec())
            .collect();
        assert_eq!(
            got,
            TreeSitter::highlight_snippet("c", &state.join()),
            "{operation:?}"
        );
    }
}

#[test]
fn cleared_service_rejects_in_flight_worker_results() {
    let state = document(&many_c_lines(20000), "canceled.c", "c");
    let mut ops = EditorOperations::new();
    let mut hl = EditorHighlight::new();
    hl.reset_for_document(&state, state.line_count() as usize);
    hl.highlight_content(&state, &mut ops);
    hl.clear();
    for _ in 0..20 {
        hl.poll(&state, &ops);
        assert!(hl.spans_for_line(0).is_empty());
        assert!(hl.spans_for_line(19999).is_empty());
        std::thread::sleep(Duration::from_millis(5));
    }
}
