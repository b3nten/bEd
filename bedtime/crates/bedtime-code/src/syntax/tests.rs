use super::*;
use crate::syntax::analysis::Backend;
use std::{
    thread,
    time::{Duration, Instant},
};

const FIXTURES: &[(&str, &str)] = &[
    (
        "rust",
        "struct Cat {\n name: String,\n}\nimpl Cat {\n fn greet(&self) {\n println!(\"hi\");\n }\n}\n",
    ),
    ("c", "int greet(void) {\n return 42;\n}\n"),
    (
        "cpp",
        "class Cat {\n public: int greet() {\n return 42;\n }\n};\n",
    ),
    (
        "c-sharp",
        "class Cat {\n public int Greet() {\n return 42;\n }\n}\n",
    ),
    (
        "java",
        "class Cat {\n public int greet() {\n return 42;\n }\n}\n",
    ),
    ("go", "package main\nfunc greet() int {\n return 42\n}\n"),
    ("swift", "class Cat {\n func greet() {\n print(42)\n }\n}\n"),
    (
        "kotlin",
        "class Cat {\n fun greet() {\n println(42)\n }\n}\n",
    ),
    ("dart", "class Cat {\n int greet() {\n return 42;\n }\n}\n"),
    ("python", "class Cat:\n def greet(self):\n  return 42\n"),
    ("ruby", "class Cat\n def greet\n  42\n end\nend\n"),
    (
        "php",
        "<?php\nclass Cat {\n function greet() {\n return 42;\n }\n}\n",
    ),
    ("lua", "function greet()\n return 42\nend\n"),
    ("bash", "greet() {\n echo 42\n}\n"),
    ("powershell", "function Get-Cat {\n Write-Output 42\n}\n"),
    ("javascript", "function greet() {\n return 42;\n}\n"),
    (
        "typescript",
        "interface Cat {\n name: string;\n}\nfunction greet(): number {\n return 42;\n}\n",
    ),
    ("tsx", "function Cat() {\n return <div>Cat</div>;\n}\n"),
    (
        "html",
        "<html>\n<body>\n<script>const value = 42;</script>\n<style>body { color: red; }</style>\n</body>\n</html>\n",
    ),
    ("css", ".cat {\n color: red;\n}\n"),
    ("json", "{\n \"cat\": {\n  \"age\": 42\n }\n}\n"),
    ("yaml", "cat:\n name: Momo\n age: 42\n"),
    ("toml", "[cat]\nname = \"Momo\"\nage = 42\n"),
    ("xml", "<cats>\n<cat age=\"42\">\nMomo\n</cat>\n</cats>\n"),
    (
        "markdown",
        "# Cats\n\n**bold** and *emphasis*\n\n```rust\nfn greet() { let age = 42; }\n```\n",
    ),
    (
        "sql",
        "CREATE TABLE cats (\n name TEXT,\n age INTEGER\n);\n",
    ),
    (
        "dockerfile",
        "FROM alpine AS cats\nRUN echo cats \\\n && echo 42\n",
    ),
    (
        "cmake",
        "function(greet)\n message(STATUS \"cats\")\nendfunction()\n",
    ),
    ("make", "greet:\n\techo cats\n\techo 42\n"),
    ("hcl", "resource \"cat\" \"momo\" {\n age = 42\n}\n"),
];
fn key(revision: u64) -> ObservationKey {
    ObservationKey {
        source: 1,
        revision,
        language_generation: 0,
    }
}
fn analyze(
    backend: &mut Backend,
    language: &'static str,
    text: &str,
    revision: u64,
    base: Option<u64>,
) -> Observation {
    backend
        .analyze(
            key(revision),
            language,
            Arc::new(text.to_string()),
            base,
            &|| false,
        )
        .unwrap_or_else(|e| panic!("{language}: {e:?}"))
}
#[test]
fn selected_languages_contribute_highlights_folds_and_outlines() {
    assert_eq!(FIXTURES.len(), LANGUAGES.len());
    let mut backend = Backend::default();
    for &(language, text) in FIXTURES {
        let observation = analyze(&mut backend, language, text, 0, None);
        assert!(
            !observation.highlights.is_empty(),
            "{language}: no highlights"
        );
        assert!(
            !observation.folds.is_empty(),
            "{language}: no folds: {}",
            observation.tree.root_node().to_sexp()
        );
        assert!(
            !observation.outline.is_empty(),
            "{language}: no outline: {}",
            observation.tree.root_node().to_sexp()
        );
        for pair in observation.highlights.windows(2) {
            assert!(pair[0].range.end <= pair[1].range.start);
        }
        for (i, node) in observation.outline.iter().enumerate() {
            assert!(node.parent.is_none_or(|parent| parent < i));
        }
    }
}
#[test]
fn incremental_equals_fresh_for_unicode_newlines_and_multiple_changes() {
    let mut backend = Backend::default();
    for (language, before) in FIXTURES.iter().copied() {
        let _ = analyze(&mut backend, language, before, 0, None);
        for (revision, after) in [
            format!("\n{before}"),
            before.replace("42", "123"),
            before.replace("Cat", "Café"),
            before.replace('\n', "\r\n"),
            before.replace('\n', "\r"),
            before.to_owned(),
        ]
        .iter()
        .enumerate()
        {
            let incremental = analyze(
                &mut backend,
                language,
                after,
                revision as u64 + 1,
                Some(revision as u64),
            );
            assert!(incremental.incremental);
            let fresh = analyze(
                &mut Backend::default(),
                language,
                after,
                revision as u64 + 1,
                None,
            );
            assert_eq!(
                incremental.tree.root_node().to_sexp(),
                fresh.tree.root_node().to_sexp(),
                "{language}"
            );
            assert_eq!(incremental.highlights, fresh.highlights, "{language}");
            assert_eq!(incremental.folds, fresh.folds, "{language}");
            assert_eq!(incremental.outline, fresh.outline, "{language}");
        }
        assert!(!analyze(&mut backend, language, before, 99, None).incremental);
    }
}
#[test]
fn html_and_markdown_injections_preserve_child_language_roles() {
    let mut backend = Backend::default();
    for (language, text, token) in [
        ("html", FIXTURES[18].1, "const"),
        ("markdown", FIXTURES[24].1, "fn"),
    ] {
        let observation = analyze(&mut backend, language, text, 0, None);
        let start = text.find(token).unwrap();
        assert!(
            observation
                .highlights_in(start..start + token.len())
                .iter()
                .any(|s| s.capture.starts_with("keyword")),
            "{language}: missing keyword {token}"
        );
    }
    let text = FIXTURES[24].1;
    let observation = analyze(&mut backend, "markdown", text, 0, None);
    let start = text.find("**bold**").unwrap();
    assert!(
        observation
            .highlights_in(start..start + 8)
            .iter()
            .any(|s| s.capture.as_ref() == "text.strong")
    );
}
fn ready(session: &mut AnalysisSession) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        session.poll();
        if session.status() != &AnalysisStatus::Pending {
            break;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(2));
    }
}
#[test]
fn newest_request_language_changes_limits_and_retirement() {
    let mut session = AnalysisSession::new();
    session.request(Arc::new("fn old() {}".to_string()), 0, "rust", None);
    session.request(
        Arc::new("def newest():\n return 1\n".to_string()),
        1,
        "python",
        None,
    );
    assert!(session.observation().is_none());
    ready(&mut session);
    let observation = session.observation().unwrap().clone();
    assert_eq!(observation.language, "python");
    assert_eq!(observation.key.revision, 1);
    assert!(observation.outline.iter().any(|n| n.label == "newest"));
    session.request(Arc::new("".to_string()), 1, "rust", None);
    assert!(session.observation().is_none());
    ready(&mut session);
    assert_ne!(
        session.observation().unwrap().key.language_generation,
        observation.key.language_generation
    );
    session.request(Arc::new("".to_string()), 2, "text", None);
    assert_eq!(session.status(), &AnalysisStatus::Unsupported);
    session.request(Arc::new("x".repeat(MAX_SOURCE_BYTES + 1)), 3, "rust", None);
    assert!(matches!(session.status(), AnalysisStatus::Limited(_)));
    let (session, worker) = AnalysisSession::with_worker();
    drop(session);
    let start = Instant::now();
    worker.run(|| false);
    assert!(start.elapsed() < Duration::from_millis(100));
}
#[test]
fn aliases_are_limited_to_the_bundle() {
    for (path, language) in [
        ("a.rs", "rust"),
        ("a.jsx", "javascript"),
        ("CMakeLists.txt", "cmake"),
        ("/tmp/Makefile", "make"),
        ("Dockerfile", "dockerfile"),
        ("python3", "python"),
        ("C#", "c-sharp"),
    ] {
        assert_eq!(language_name(path), Some(language));
    }
    assert_eq!(language_name("zig"), None);
}

#[test]
fn local_bindings_override_builtin_predicates_and_combined_injections_keep_structure() {
    let text = "console; function f(console) { return console; }\nconst page = html`<div>${value}<span>cat</span></div>`;\n";
    let observation = analyze(&mut Backend::default(), "javascript", text, 0, None);
    let occurrences: Vec<_> = text.match_indices("console").map(|(at, _)| at).collect();
    assert!(
        observation
            .highlights_in(occurrences[0]..occurrences[0] + 7)
            .iter()
            .any(|s| s.capture.as_ref() == "variable.builtin")
    );
    assert!(
        !observation
            .highlights_in(occurrences[2]..occurrences[2] + 7)
            .iter()
            .any(|s| s.capture.as_ref() == "variable.builtin")
    );
    let at = text.find("span>").unwrap();
    assert!(
        observation
            .highlights_in(at..at + 4)
            .iter()
            .any(|s| s.capture.starts_with("tag"))
    );
}

#[test]
#[ignore = "Explicit analysis timing and resident-memory fixture"]
fn analysis_measurement() {
    let text = "fn cat() {\n let age=42;\n println!(\"{}\",age);\n}\n".repeat(2000);
    let mut backend = Backend::default();
    let first = analyze(&mut backend, "rust", &text, 0, None);
    let mut next = text.clone();
    next.insert(next.find("42").unwrap(), '1');
    let second = analyze(&mut backend, "rust", &next, 1, Some(0));
    let start = Instant::now();
    for _ in 0..100_000 {
        std::hint::black_box(second.highlights_in(100..500));
    }
    eprintln!(
        "bytes={} initial={:?} incremental={:?} highlights={} folds={} outline={} lookup_100k={:?}",
        text.len(),
        first.elapsed,
        second.elapsed,
        second.highlights.len(),
        second.folds.len(),
        second.outline.len(),
        start.elapsed()
    );
}

#[test]
fn malformed_source_reports_failure_and_new_requests_can_recover() {
    struct Invalid;
    impl Source for Invalid {
        fn len(&self) -> usize {
            1
        }
        fn chunk(&self, _: usize) -> &[u8] {
            &[255]
        }
    }
    let mut session = AnalysisSession::new();
    session.request(Arc::new(Invalid), 0, "rust", None);
    ready(&mut session);
    assert!(matches!(session.status(), AnalysisStatus::Failed(_)));
    assert!(session.observation().is_none());
    session.request(Arc::new("fn cat() {".to_string()), 1, "rust", None);
    ready(&mut session);
    assert_eq!(session.status(), &AnalysisStatus::Ready);
    assert!(session.observation().unwrap().tree.root_node().has_error());
}
