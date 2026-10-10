use bed_editing::{
    editor_operations::{EditorOperations, OpKind, TextOp},
    editor_state::EditorState,
};
use bed_highlight::{
    capture_map::ThemeSlot, highlight_service::EditorHighlight, tree_sitter::TreeSitter,
};

fn slot(source: &str, token: &str, language: &str) -> Option<ThemeSlot> {
    let offset = source.find(token).unwrap();
    let row = source[..offset]
        .bytes()
        .filter(|&byte| byte == b'\n')
        .count();
    let column = offset - source[..offset].rfind('\n').map_or(0, |at| at + 1);
    let lines = TreeSitter::highlight_snippet(language, source.as_bytes());
    lines[row]
        .iter()
        .find(|span| span.start <= column as i32 && span.end > column as i32)
        .map(|span| span.slot)
}

#[test]
fn markdown_formats_inline_text_and_injects_code_fences() {
    let source = "# Heading\n\n**bold** and *emphasis*, `inline code`, [link](https://example.com).\n\n```rust\nfn main() { let value = 42; }\n```\n\n```python\ndef greet():\n    return \"hello\"\n```\n";
    for (token, expected) in [
        ("Heading", ThemeSlot::Function),
        ("bold", ThemeSlot::Keyword),
        ("emphasis", ThemeSlot::Special),
        ("inline code", ThemeSlot::String),
        ("https://example.com", ThemeSlot::String),
        ("fn", ThemeSlot::Keyword),
        ("main", ThemeSlot::Function),
        ("def", ThemeSlot::Keyword),
        ("greet", ThemeSlot::Function),
        ("hello", ThemeSlot::String),
    ] {
        assert_eq!(slot(source, token, "md"), Some(expected), "{token}");
    }
}

#[test]
fn unknown_fences_stay_plain_and_do_not_hide_later_markdown() {
    let source = "```unknown-language\nplain words here\n```\n\n# Still a heading\n";
    assert_eq!(slot(source, "plain words", "markdown"), None);
    assert_eq!(slot(source, "Still", "markdown"), Some(ThemeSlot::Function));
}

#[test]
fn broad_grammar_set_highlights_real_syntax() {
    for (language, source) in [
        ("yaml", "name: \"hello\"\ncount: 42\n"),
        ("sql", "SELECT name FROM users WHERE id = 42;"),
        ("lua", "local function greet() return \"hello\" end"),
        ("zig", "const value: u32 = 42;"),
        ("swift", "func greet() -> String { return \"hello\" }"),
        ("nix", "{ name = \"hello\"; count = 42; }"),
        ("dockerfile", "FROM alpine:latest\nRUN echo hello\n"),
        ("make", "all:\n\techo hello\n"),
        ("xml", "<item name=\"hello\">value</item>"),
        ("toml", "name = \"hello\"\ncount = 42\n"),
    ] {
        assert!(
            TreeSitter::highlight_snippet(language, source.as_bytes())
                .iter()
                .flatten()
                .any(|span| span.slot != ThemeSlot::Text),
            "{language}"
        );
    }
}

#[test]
fn html_injects_javascript_and_css() {
    let source = "<script>function greet() { return \"hello\"; }</script>\n<style>body { color: red; }</style>";
    assert_eq!(slot(source, "function", "html"), Some(ThemeSlot::Keyword));
    assert_eq!(slot(source, "greet", "html"), Some(ThemeSlot::Function));
    assert_eq!(slot(source, "hello", "html"), Some(ThemeSlot::String));
    assert_eq!(slot(source, "color", "html"), Some(ThemeSlot::Property));
}

#[test]
fn editing_fence_language_recolors_existing_contents() {
    TreeSitter::prewarm("md").unwrap();
    let mut state = EditorState::new();
    state.path = "notes.md".into();
    state.language_id = "md".into();
    state.set_from_bytes(b"```unknown\nfn main() {}\n```\n");
    let mut operations = EditorOperations::new();
    let mut highlight = EditorHighlight::new();
    highlight.reset_for_document(&state, state.line_count() as usize);
    highlight.highlight_content(&state, &mut operations);
    assert!(highlight.spans_for_line(1).is_empty());
    for op in [
        TextOp {
            kind: OpKind::Delete,
            row: 0,
            column: 3,
            length: 7,
            text: Vec::new(),
        },
        TextOp {
            kind: OpKind::Insert,
            row: 0,
            column: 3,
            length: 0,
            text: b"rust".to_vec(),
        },
    ] {
        assert!(operations.apply(&mut state, &op).ok);
    }
    highlight.highlight_content(&state, &mut operations);
    assert!(
        highlight
            .spans_for_line(1)
            .iter()
            .any(|span| span.slot == ThemeSlot::Keyword)
    );
    assert_eq!(
        highlight.spans_for_line(1),
        TreeSitter::highlight_snippet("md", &state.join())[1]
    );
}

#[test]
fn special_filenames_use_their_grammar_and_manual_language_wins() {
    for (path, source) in [
        ("Dockerfile", "FROM alpine:latest\n"),
        ("CMakeLists.txt", "cmake_minimum_required(VERSION 3.20)\n"),
        ("Makefile", "all:\n\techo hello\n"),
    ] {
        let mut state = EditorState::new();
        state.path = path.into();
        state.language_id = EditorState::language_id_from_path(path);
        state.set_from_bytes(source.as_bytes());
        TreeSitter::prewarm(path).unwrap();
        let mut operations = EditorOperations::new();
        let mut highlight = EditorHighlight::new();
        highlight.reset_for_document(&state, state.line_count() as usize);
        highlight.highlight_content(&state, &mut operations);
        assert!(!highlight.spans_for_line(0).is_empty(), "{path}");
        state.language_id = "plain-text".into();
        operations.bump_generation();
        highlight.highlight_content(&state, &mut operations);
        assert!(
            highlight.spans_for_line(0).is_empty(),
            "manual language for {path}"
        );
    }
}

#[test]
fn unicode_crlf_and_invalid_bytes_keep_document_offsets() {
    let source = b"# caf\xc3\xa9\r\n\r\ntext \xff `code`\r\n";
    let colors = TreeSitter::highlight_snippet("md", source);
    assert_eq!(colors.len(), 4);
    assert!(
        colors[0]
            .iter()
            .any(|span| span.start <= 2 && span.end == 7 && span.slot == ThemeSlot::Function)
    );
    assert!(
        colors[2]
            .iter()
            .any(|span| span.start <= 8 && span.end >= 12 && span.slot == ThemeSlot::String)
    );
}
