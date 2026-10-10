use super::*;
use bed_editing::editor_state::EditorState;

fn key(language: &str) -> OutlineKey {
    OutlineKey {
        document: DocumentId(1),
        generation: 1,
        revision: 1,
        path: format!("file.{language}"),
        language_id: language.into(),
    }
}
fn snapshot(source: &str) -> Snapshot {
    let mut state = EditorState::new();
    state.set_from_bytes(source.as_bytes());
    state.snapshot()
}
fn outline(language: &str, source: &str) -> OutlineResult {
    extract(
        &mut Parser::new(),
        &mut HashMap::new(),
        key(language),
        &snapshot(source),
        || false,
    )
    .unwrap()
}

#[test]
fn every_supported_outline_grammar_has_working_nested_definitions() {
    let fixtures = [
        (
            "rs",
            "mod demo { struct Item { value: i32 } impl Item { fn run(&self) {} } }",
            "demo",
            "Item",
            "value",
        ),
        (
            "c",
            "struct Item { int value; }; int run(void) { return 0; }",
            "Item",
            "value",
            "run",
        ),
        (
            "cpp",
            "namespace demo { class Item { int value; void run() {} }; }",
            "demo",
            "Item",
            "run",
        ),
        (
            "cs",
            "namespace Demo { class Item { int value; void Run() {} } }",
            "Demo",
            "Item",
            "Run",
        ),
        (
            "go",
            "package demo\ntype Item struct { Value int }\nfunc Run() {}\n",
            "Item",
            "Value",
            "Run",
        ),
        (
            "java",
            "class Item { int value; void run() {} }",
            "Item",
            "value",
            "run",
        ),
        (
            "js",
            "class Item { run() {} } const make = () => {};",
            "Item",
            "run",
            "make",
        ),
        (
            "tsx",
            "interface Item { value: number; run(): void; } const make = () => <div/>;",
            "Item",
            "value",
            "make",
        ),
        (
            "ts",
            "interface Item { value: number; run(): void; } const make = () => {};",
            "Item",
            "value",
            "make",
        ),
        (
            "py",
            "class Item:\n    def run(self):\n        pass\nLIMIT = 3\n",
            "Item",
            "run",
            "LIMIT",
        ),
        (
            "rb",
            "module Demo\n class Item\n  def run\n  end\n end\nend\n",
            "Demo",
            "Item",
            "run",
        ),
        (
            "kt",
            "class Item { val value: Int = 1; fun run() {} }",
            "Item",
            "value",
            "run",
        ),
        (
            "sh",
            "outer() { inner() { echo hi; }; }",
            "outer",
            "inner",
            "outer",
        ),
        (
            "json",
            r#"{"items": [{"value": 1}]}"#,
            "items",
            "[0]",
            "value",
        ),
        (
            "toml",
            "[demo]\nvalue = { nested = 1 }\n",
            "demo",
            "value",
            "nested",
        ),
        (
            "tf",
            "resource \"demo\" \"item\" { value = { nested = 1 } }",
            "resource \"demo\" \"item\"",
            "value",
            "nested",
        ),
        (
            "html",
            "<main><section><br/></section></main>",
            "<main>",
            "<section>",
            "<br>",
        ),
        (
            "css",
            "@media screen { .item { color: red; } } @keyframes spin { from { opacity: 0; } }",
            "@media screen",
            ".item",
            "@keyframes spin",
        ),
    ];
    for (language, source, first, second, third) in fixtures {
        let result = outline(language, source);
        assert_eq!(
            result.status,
            OutlineStatus::Ready,
            "{language}: {:?}",
            result.status
        );
        let labels = result
            .nodes
            .iter()
            .map(|node| node.label.as_str())
            .collect::<Vec<_>>();
        for expected in [first, second, third] {
            assert!(
                labels.contains(&expected),
                "{language}: missing {expected}: {labels:?}"
            );
        }
        assert!(
            result.nodes.iter().any(|node| node.parent.is_some()),
            "{language}: missing hierarchy: {labels:?}"
        );
        for (index, node) in result.nodes.iter().enumerate() {
            assert!(
                node.range.start <= node.name_range.start && node.name_range.end <= node.range.end,
                "{language}: {node:?}"
            );
            assert!(node.range.end <= source.len());
            if let Some(parent) = node.parent {
                assert!(parent < index);
                let parent = &result.nodes[parent];
                assert!(
                    parent.range.start <= node.range.start && node.range.end <= parent.range.end
                );
                assert_eq!(node.depth, parent.depth + 1);
            }
        }
        assert!(
            result
                .nodes
                .windows(2)
                .all(|nodes| nodes[0].range.start <= nodes[1].range.start)
        );
    }
}

#[test]
fn outlines_keep_definitions_through_errors_and_ignore_local_values_and_imports() {
    let result = outline(
        "rs",
        "use std::fmt; const LIMIT: i32 = 2; fn run() { const LOCAL: i32 = 1; let local = 2; } fn broken() { let x =",
    );
    assert_eq!(result.status, OutlineStatus::Ready);
    let labels = result
        .nodes
        .iter()
        .map(|node| node.label.as_str())
        .collect::<Vec<_>>();
    assert!(
        labels.contains(&"LIMIT") && labels.contains(&"run"),
        "{labels:?}"
    );
    assert!(!labels.contains(&"LOCAL") && !labels.contains(&"local") && !labels.contains(&"fmt"));
    let result = outline(
        "js",
        "const make = () => { const local = 2; }; const LIMIT = 2;",
    );
    assert_eq!(
        result
            .nodes
            .iter()
            .filter(|node| node.label == "make")
            .count(),
        1
    );
    assert!(!result.nodes.iter().any(|node| node.label == "local"));
}

#[test]
fn identities_survive_offset_changes_and_distinguish_duplicate_names() {
    let source = "mod one { fn run() {} fn run() {} } mod two { fn run() {} }";
    let first = outline("rs", source);
    let shifted = outline("rs", &format!("// 🙂\n{source}"));
    assert_eq!(
        first.nodes.iter().map(|node| node.id).collect::<Vec<_>>(),
        shifted.nodes.iter().map(|node| node.id).collect::<Vec<_>>()
    );
    let ids = first
        .nodes
        .iter()
        .map(|node| node.id)
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(ids.len(), first.nodes.len());
}

#[test]
fn name_ranges_preserve_utf8_bytes_and_all_line_endings() {
    for ending in ["\n", "\r\n", "\r"] {
        let source = format!("// 🙂{ending}fn café() {{}}{ending}");
        let result = outline("rs", &source);
        let node = result
            .nodes
            .iter()
            .find(|node| node.label == "café")
            .unwrap();
        assert_eq!(
            &source.as_bytes()[node.name_range.clone()],
            "café".as_bytes()
        );
        let mut state = EditorState::new();
        state.set_from_bytes(source.as_bytes());
        assert_eq!(state.row_col_from_offset(node.name_range.start), (1, 3));
    }
}

#[test]
fn empty_unsupported_and_canceled_buffers_have_distinct_results() {
    assert_eq!(outline("txt", "hello").status, OutlineStatus::Unsupported);
    assert_eq!(
        outline("yaml", "key: value").status,
        OutlineStatus::Unsupported
    );
    assert_eq!(
        outline("lua", "local value = 1").status,
        OutlineStatus::Unsupported
    );
    let result = outline("rs", "");
    assert_eq!(result.status, OutlineStatus::Ready);
    assert!(result.nodes.is_empty());
    assert!(
        extract(
            &mut Parser::new(),
            &mut HashMap::new(),
            key("rs"),
            &snapshot("fn run() {}"),
            || true
        )
        .is_none()
    );
}

#[test]
fn markdown_sections_nest_headings_and_ignore_headings_in_fenced_code() {
    let source = "# Overview\n\nIntro.\n\n## First\n\nText.\n\n### Child\n\n```markdown\n# Example\n```\n\n## Second\n\nNext\n====\n\nSubsection\n----------\n";
    for language in ["md", "markdown"] {
        let result = outline(language, source);
        assert_eq!(result.status, OutlineStatus::Ready);
        let labels = result
            .nodes
            .iter()
            .map(|node| node.label.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            labels,
            ["Overview", "First", "Child", "Second", "Next", "Subsection"]
        );
        let parents = result
            .nodes
            .iter()
            .map(|node| node.parent)
            .collect::<Vec<_>>();
        assert_eq!(parents, [None, Some(0), Some(1), Some(0), None, Some(4)]);
        let next_root = source.find("Next\n").unwrap();
        let second = source.find("## Second").unwrap();
        let ends = result
            .nodes
            .iter()
            .map(|node| node.range.end)
            .collect::<Vec<_>>();
        assert_eq!(
            ends,
            [
                next_root,
                second,
                second,
                next_root,
                source.len(),
                source.len()
            ]
        );
        for node in &result.nodes {
            assert_eq!(node.kind, "section");
            assert!(
                node.range.start <= node.name_range.start && node.name_range.end <= node.range.end
            );
            assert!(node.range.end <= source.len());
        }
    }
}

fn wait(service: &mut OutlineService) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while service.updating() {
        assert!(Instant::now() < deadline, "outline worker timed out");
        service.poll();
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn worker_coalesces_edits_rejects_old_sources_and_clears() {
    let mut service = OutlineService::default();
    service.request(key("rs"), snapshot("fn initial() {}"));
    wait(&mut service);
    assert_eq!(service.result().unwrap().nodes[0].label, "initial");
    for revision in 2..100 {
        let mut key = key("rs");
        key.revision = revision;
        service.request(key, snapshot(&format!("fn edit_{revision}() {{}}")));
    }
    assert!(service.updating());
    assert_eq!(service.result().unwrap().nodes[0].label, "initial");
    wait(&mut service);
    assert_eq!(service.result().unwrap().nodes[0].label, "edit_99");
    let mut other = key("py");
    other.document = DocumentId(2);
    service.request(other, snapshot("def other(): pass"));
    assert!(service.result().is_none());
    wait(&mut service);
    assert_eq!(service.result().unwrap().nodes[0].label, "other");
    service.clear();
    assert!(service.requested().is_none() && service.result().is_none() && !service.updating());
}

#[test]
fn file_scoped_namespace_contains_its_following_declarations() {
    let result = outline("cs", "namespace Demo; class Item { void Run() {} }");
    assert_eq!(result.status, OutlineStatus::Ready);
    let item = result
        .nodes
        .iter()
        .find(|node| node.label == "Item")
        .unwrap();
    assert_eq!(result.nodes[item.parent.unwrap()].label, "Demo");
}

#[test]
fn c_declarators_and_constants_resolve_to_names_and_skip_local_constants() {
    for language in ["c", "cpp"] {
        let result = outline(
            language,
            "const int LIMIT = 3; struct Item { int *value; }; int *run(void) { const int LOCAL = 1; return 0; }",
        );
        assert_eq!(result.status, OutlineStatus::Ready);
        let labels = result
            .nodes
            .iter()
            .map(|node| node.label.as_str())
            .collect::<Vec<_>>();
        for name in ["LIMIT", "Item", "value", "run"] {
            assert!(labels.contains(&name), "{language}: {labels:?}");
        }
        assert!(!labels.contains(&"LOCAL"));
    }
}

#[test]
fn cancellation_during_work_does_not_poison_the_next_parse() {
    let text = snapshot(&"fn item() {}\n".repeat(10000));
    let mut parser = Parser::new();
    let mut queries = HashMap::new();
    let checks = std::cell::Cell::new(0);
    let result = extract(&mut parser, &mut queries, key("rs"), &text, || {
        checks.set(checks.get() + 1);
        checks.get() > 20
    });
    assert!(result.is_none());
    let result = extract(
        &mut parser,
        &mut queries,
        key("rs"),
        &snapshot("fn fresh() {}"),
        || false,
    )
    .unwrap();
    assert_eq!(result.nodes[0].label, "fresh");
}

#[test]
fn large_file_limit_skips_parsing_before_materializing_worker_bytes() {
    let source = vec![b' '; SKIP_TREE_SITTER_BYTES + 1];
    let mut state = EditorState::new();
    state.set_from_bytes(&source);
    drop(source);
    let mut parser = Parser::new();
    let mut queries = HashMap::new();
    let result = extract(
        &mut parser,
        &mut queries,
        key("rs"),
        &state.snapshot(),
        || false,
    )
    .unwrap();
    assert_eq!(result.status, OutlineStatus::TooLarge);
    assert!(result.nodes.is_empty() && queries.is_empty());
}

#[test]
fn json_empty_keys_and_nested_arrays_remain_navigable() {
    let source = r#"{"": {" ": [1, [2]]}}"#;
    let result = outline("json", source);
    assert_eq!(result.status, OutlineStatus::Ready);
    let labels = result
        .nodes
        .iter()
        .map(|node| node.label.as_str())
        .collect::<Vec<_>>();
    assert_eq!(labels, ["\"\"", "\" \"", "[0]", "[1]", "[0]"]);
    assert_eq!(result.nodes.last().unwrap().depth, 3);
}
