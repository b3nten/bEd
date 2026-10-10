//! Whole-line syntax folds from bundled grammars. One lazy, cancellable worker
//! parses the latest immutable snapshot; only matching results reach a view.
use crate::{
    grammars::language_name, highlight_service::SKIP_TREE_SITTER_BYTES, tree_sitter::parser_text,
};
use arborium::tree_sitter::{self, ParseOptions, Parser};
use bed_editing::{
    buffer::text_buffer::Snapshot,
    editor_state::{DocumentKind, EditorState},
    folding::FoldRange,
};
use std::{
    ops::ControlFlow,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const EDIT_DEBOUNCE: Duration = Duration::from_millis(150);

#[derive(Clone, PartialEq, Eq)]
struct SourceKey {
    generation: u64,
    revision: u64,
    path: String,
    language_id: String,
    kind: DocumentKind,
}
impl SourceKey {
    fn matches(&self, state: &EditorState, generation: u64, revision: u64) -> bool {
        self.generation == generation
            && self.revision == revision
            && self.path == state.path
            && self.language_id == state.language_id
            && self.kind == state.kind
    }
    fn same_source(&self, state: &EditorState, generation: u64) -> bool {
        self.generation == generation
            && self.path == state.path
            && self.language_id == state.language_id
            && self.kind == state.kind
    }
}
struct Job {
    token: u64,
    key: SourceKey,
    language: &'static str,
    text: Snapshot,
    deadline: Instant,
}
#[derive(Default)]
struct Mailbox {
    pending: Option<Job>,
    stopped: bool,
}
struct FoldResult {
    key: SourceKey,
    ranges: Vec<FoldRange>,
}

#[derive(Default)]
pub struct FoldingService {
    mailbox: Arc<(Mutex<Mailbox>, Condvar)>,
    token: Arc<AtomicU64>,
    worker: Option<JoinHandle<()>>,
    receiver: Option<mpsc::Receiver<FoldResult>>,
    requested: Option<SourceKey>,
    result: Option<FoldResult>,
    supported: bool,
}
impl Drop for FoldingService {
    fn drop(&mut self) {
        self.token.fetch_add(1, Ordering::Relaxed);
        let (lock, wake) = &*self.mailbox;
        {
            let mut slot = lock.lock().unwrap();
            slot.stopped = true;
            slot.pending = None;
        }
        wake.notify_one();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
impl FoldingService {
    /// Keep the fold gutter stable while new syntax ranges are being parsed.
    pub fn supported(&self) -> bool {
        self.supported
    }

    /// Request and poll folds for the current snapshot. `None` means parsing
    /// is pending; callers should retain their edit-transformed fold state.
    pub fn update(
        &mut self,
        state: &EditorState,
        generation: u64,
        revision: u64,
    ) -> Option<&[FoldRange]> {
        if self
            .requested
            .as_ref()
            .is_none_or(|key| !key.matches(state, generation, revision))
        {
            let same_source = self
                .requested
                .as_ref()
                .is_some_and(|key| key.same_source(state, generation));
            let key = SourceKey {
                generation,
                revision,
                path: state.path.clone(),
                language_id: state.language_id.clone(),
                kind: state.kind,
            };
            let token = self.token.fetch_add(1, Ordering::Relaxed) + 1;
            self.requested = Some(key.clone());
            self.result = None;
            // Manual language selection wins; otherwise recognize full filenames
            // as well as extensions, matching the highlighting service.
            let language =
                if state.language_id == EditorState::language_id_from_path(&state.path) {
                    language_name(&state.path).or_else(|| language_name(&state.language_id))
                } else {
                    language_name(&state.language_id)
                }
                .filter(|language| fold_kinds(language).is_some());
            self.supported = state.kind == DocumentKind::Text
                && state.byte_size() <= SKIP_TREE_SITTER_BYTES
                && language.is_some();
            if !self.supported {
                self.mailbox.0.lock().unwrap().pending = None;
                self.mailbox.1.notify_one();
                self.result = Some(FoldResult {
                    key,
                    ranges: Vec::new(),
                });
            } else {
                // Ordinary text and unsupported languages never create a thread.
                if self.worker.is_none() {
                    let (sender, receiver) = mpsc::channel();
                    self.receiver = Some(receiver);
                    let mailbox = Arc::clone(&self.mailbox);
                    let worker_token = Arc::clone(&self.token);
                    self.worker = Some(thread::spawn(move || {
                        let (lock, wake) = &*mailbox;
                        let mut parser = Parser::new();
                        loop {
                            let job = {
                                let mut slot = lock.lock().unwrap();
                                loop {
                                    if slot.stopped {
                                        return;
                                    }
                                    if let Some(job) = &slot.pending {
                                        let now = Instant::now();
                                        if now >= job.deadline {
                                            break slot.pending.take().unwrap();
                                        }
                                        let delay = job.deadline - now;
                                        slot = wake.wait_timeout(slot, delay).unwrap().0;
                                    } else {
                                        slot = wake.wait(slot).unwrap();
                                    }
                                }
                            };
                            let canceled = || worker_token.load(Ordering::Relaxed) != job.token;
                            if let Some(ranges) =
                                extract(&mut parser, job.language, &job.text, canceled)
                                && !canceled()
                                && sender
                                    .send(FoldResult {
                                        key: job.key,
                                        ranges,
                                    })
                                    .is_err()
                            {
                                return;
                            }
                        }
                    }));
                }
                self.mailbox.0.lock().unwrap().pending = Some(Job {
                    token,
                    key,
                    language: language.unwrap(),
                    text: state.snapshot(),
                    deadline: Instant::now()
                        + if same_source {
                            EDIT_DEBOUNCE
                        } else {
                            Duration::ZERO
                        },
                });
                self.mailbox.1.notify_one();
            }
        }
        if let Some(receiver) = &self.receiver {
            while let Ok(result) = receiver.try_recv() {
                if self.requested.as_ref() == Some(&result.key) {
                    self.result = Some(result);
                }
            }
        }
        self.result.as_ref().map(|result| result.ranges.as_slice())
    }
}

// Fold structural containers and compound statements, not arbitrary multiline
// expressions. Python needs the compound statement's header, not its body block.
fn fold_kinds(language: &str) -> Option<&'static [&'static str]> {
    Some(match language {
        "rust" => &[
            "function_item",
            "impl_item",
            "mod_item",
            "trait_item",
            "struct_item",
            "enum_item",
            "if_expression",
            "else_clause",
            "for_expression",
            "while_expression",
            "loop_expression",
            "match_expression",
            "block",
            "declaration_list",
            "field_declaration_list",
            "enum_variant_list",
            "match_block",
            "field_initializer_list",
            "array_expression",
            "use_list",
            "block_comment",
            "raw_string_literal",
        ],
        "python" => &[
            "function_definition",
            "class_definition",
            "if_statement",
            "elif_clause",
            "else_clause",
            "for_statement",
            "while_statement",
            "with_statement",
            "try_statement",
            "except_clause",
            "finally_clause",
            "match_statement",
            "case_clause",
            "dictionary",
            "list",
            "set",
            "tuple",
            "string",
        ],
        "javascript" | "typescript" | "tsx" => &[
            "statement_block",
            "class_body",
            "switch_body",
            "enum_body",
            "interface_body",
            "object_type",
            "object",
            "array",
            "jsx_element",
            "comment",
            "template_string",
        ],
        "json" => &["object", "array", "comment"],
        "c" | "cpp" => &[
            "function_definition",
            "struct_specifier",
            "class_specifier",
            "enum_specifier",
            "namespace_definition",
            "compound_statement",
            "declaration_list",
            "field_declaration_list",
            "enumerator_list",
            "initializer_list",
            "comment",
            "raw_string_literal",
        ],
        "go" => &[
            "block",
            "field_declaration_list",
            "interface_type",
            "literal_value",
            "import_spec_list",
            "var_spec_list",
            "expression_switch_statement",
            "type_switch_statement",
            "select_statement",
            "comment",
            "raw_string_literal",
        ],
        "html" => &["element", "script_element", "style_element", "comment"],
        "css" => &["block", "keyframe_block_list", "comment"],
        "java" => &[
            "block",
            "class_body",
            "interface_body",
            "enum_body",
            "constructor_body",
            "module_body",
            "annotation_type_body",
            "array_initializer",
            "switch_block",
            "block_comment",
        ],
        "c-sharp" => &[
            "block",
            "declaration_list",
            "enum_member_declaration_list",
            "accessor_list",
            "initializer_expression",
            "switch_body",
            "switch_expression",
            "comment",
            "verbatim_string_literal",
            "raw_string_literal",
        ],
        "kotlin" => &[
            "class_body",
            "enum_class_body",
            "function_body",
            "control_structure_body",
            "when_expression",
            "multiline_comment",
            "string_literal",
        ],
        "ruby" => &[
            "class",
            "module",
            "method",
            "singleton_method",
            "singleton_class",
            "do_block",
            "block",
            "if",
            "unless",
            "while",
            "until",
            "for",
            "case",
            "case_match",
            "begin",
            "array",
            "hash",
            "heredoc_body",
            "comment",
        ],
        "bash" => &[
            "compound_statement",
            "function_definition",
            "if_statement",
            "elif_clause",
            "else_clause",
            "for_statement",
            "while_statement",
            "case_statement",
            "heredoc_body",
        ],
        "hcl" => &["block", "object", "tuple", "comment", "heredoc_template"],
        "toml" => &[
            "table",
            "table_array_element",
            "array",
            "inline_table",
            "string",
        ],
        _ => return None,
    })
}

fn extract(
    parser: &mut Parser,
    language_name: &str,
    text: &Snapshot,
    canceled: impl Fn() -> bool,
) -> Option<Vec<FoldRange>> {
    if canceled() {
        return None;
    }
    let kinds = fold_kinds(language_name)?;
    let language = arborium::get_language(language_name)?;
    parser
        .set_language(&language)
        .expect("bundled folding grammar must match the parser");
    let source = parser_text(text.bytes());
    if canceled() {
        return None;
    }
    let mut progress = |_: &tree_sitter::ParseState| {
        if canceled() {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    };
    let tree = parser.parse_with_options(
        &mut |offset, _| &source.as_bytes()[offset..],
        None,
        Some(ParseOptions::new().progress_callback(&mut progress)),
    );
    let Some(tree) = tree else {
        // Canceled parser state must never resume against a different snapshot.
        parser.reset();
        return if canceled() { None } else { Some(Vec::new()) };
    };
    let mut ranges = Vec::new();
    let mut cursor = tree.walk();
    loop {
        if canceled() {
            return None;
        }
        let node = cursor.node();
        if kinds.contains(&node.kind()) {
            let start = node.start_position();
            let end = node.end_position();
            // Whole-line folding must not hide the next statement's header
            // when it shares a line with this node's closing token. Likewise,
            // an exclusive end at column zero owns only the preceding line.
            let has_suffix = source.as_bytes()[node.end_byte()..]
                .iter()
                .take_while(|&&byte| byte != b'\n')
                .any(|byte| !byte.is_ascii_whitespace());
            let end_line = end
                .row
                .saturating_sub(usize::from(end.column == 0 || has_suffix));
            if end_line > start.row {
                ranges.push(FoldRange {
                    start_line: start.row as i32,
                    end_line: end_line as i32,
                });
            }
        }
        if cursor.goto_first_child() {
            continue;
        }
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                return Some(FoldRange::normalize(ranges));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folds(language: &str, source: &[u8]) -> Vec<FoldRange> {
        let mut state = EditorState::new();
        state.set_from_bytes(source);
        extract(&mut Parser::new(), language, &state.snapshot(), || false).unwrap()
    }
    fn range(start_line: i32, end_line: i32) -> FoldRange {
        FoldRange {
            start_line,
            end_line,
        }
    }

    #[test]
    fn rust_folds_nested_blocks_comments_and_containers_once_per_header() {
        let source = b"/* comment\n   details */\nfn run() {\n    if ready {\n        work();\n    }\n    let values = [\n        1,\n        2,\n    ];\n}\n";
        assert_eq!(
            folds("rust", source),
            [range(0, 1), range(2, 10), range(3, 5), range(6, 8)]
        );
    }

    #[test]
    fn python_keeps_compound_headers_and_stops_at_the_last_body_line() {
        let source = b"class Item:\n    def run(self):\n        if ready:\n            work()\n        else:\n            rest()\nnext_item = 1\n";
        assert_eq!(
            folds("python", source),
            [range(0, 5), range(1, 5), range(2, 5), range(4, 5)]
        );
    }

    #[test]
    fn a_shared_closing_line_keeps_the_next_statement_visible_and_foldable() {
        let source = b"fn a() {\n    x();\n} fn b() {\n    y();\n}\n";
        assert_eq!(folds("rust", source), [range(0, 1), range(2, 4)]);
    }

    #[test]
    fn javascript_typescript_and_json_fold_bodies_not_multiline_calls() {
        let source = b"function run() {\n    if (ready) {\n        call(\n            1,\n            2,\n        );\n    }\n}\n";
        for language in ["javascript", "typescript", "tsx"] {
            assert_eq!(folds(language, source), [range(0, 7), range(1, 6)]);
        }
        let json = b"{\n  \"items\": [\n    {\n      \"value\": 1\n    }\n  ]\n}\n";
        assert_eq!(folds("json", json), [range(0, 6), range(1, 5), range(2, 4)]);
    }

    #[test]
    fn utf8_invalid_bytes_and_all_line_endings_keep_document_coordinates() {
        let source = b"// \xc3\xa9 \xff\nfn run() {\n    work();\n}\n";
        for ending in ["\n", "\r\n", "\r"] {
            let bytes = source
                .split(|byte| *byte == b'\n')
                .collect::<Vec<_>>()
                .join(ending.as_bytes());
            assert_eq!(folds("rust", &bytes), [range(1, 3)]);
        }
    }

    #[test]
    fn structural_folds_work_for_the_other_supported_grammars() {
        for (language, source) in [
            ("c", "int run(void) {\n    return 0;\n}\n"),
            (
                "cpp",
                "namespace demo {\n    class Item {\n        int value;\n    };\n}\n",
            ),
            ("go", "package demo\nfunc Run() {\n    work()\n}\n"),
            (
                "html",
                "<main>\n    <section>\n        <br>\n    </section>\n</main>\n",
            ),
            ("css", ".item {\n    color: red;\n}\n"),
            (
                "java",
                "class Item {\n    void run() {\n        work();\n    }\n}\n",
            ),
            (
                "c-sharp",
                "class Item {\n    void Run() {\n        Work();\n    }\n}\n",
            ),
            (
                "kotlin",
                "class Item {\n    fun run() {\n        work()\n    }\n}\n",
            ),
            (
                "ruby",
                "class Item\n    def run\n        work\n    end\nend\n",
            ),
            ("bash", "run() {\n    echo hi\n}\n"),
            ("hcl", "resource \"demo\" \"item\" {\n    value = 1\n}\n"),
            ("toml", "[demo]\nvalue = 1\nother = 2\n"),
        ] {
            let ranges = folds(language, source.as_bytes());
            assert!(!ranges.is_empty(), "{language}: no folds");
            assert!(ranges.iter().all(|range| range.end_line > range.start_line));
            for pair in ranges.windows(2) {
                assert!(pair[0].start_line < pair[1].start_line);
            }
        }
    }

    #[test]
    fn service_rejects_old_revisions_and_unsupported_documents() {
        let mut state = EditorState::new();
        state.language_id = "rs".into();
        state.set_from_bytes(b"fn run() {\n    work();\n}\n");
        let mut service = FoldingService::default();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(ranges) = service.update(&state, 1, 1) {
                assert_eq!(ranges, [range(0, 2)]);
                break;
            }
            assert!(Instant::now() < deadline, "folding worker did not finish");
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(service.update(&state, 1, 2), None);
        state.language_id = "json".into();
        assert!(
            service
                .update(&state, 1, 2)
                .is_none_or(|ranges| ranges.is_empty())
        );
        state.language_id = "txt".into();
        assert_eq!(service.update(&state, 1, 2), Some([].as_slice()));
        state.language_id = "yaml".into();
        assert_eq!(service.update(&state, 1, 2), Some([].as_slice()));
        state.kind = DocumentKind::Bytes;
        state.language_id = "rs".into();
        assert_eq!(service.update(&state, 1, 2), Some([].as_slice()));
    }
}
