# Shared Tree-sitter analysis

Revision-bound code analysis for highlighting, whole-line folding and code
outlines. This crate owns parsing/query execution; it has no editor, UI, ECS,
Workbench or filesystem dependency. Native grammars and queries come from the
single [bundled grammar crate](../bedtime-grammars2/README.md).

`Source` reads immutable UTF-8 chunks and a byte length. The optional `documents` feature implements it
for the shared persistent `bedtime-document2::TextBuffer`; submitting an `Arc` around that cheap root does
not materialize source text on the caller's thread. Hosts explicitly own an
`AnalysisSession`, submit source/revision/language, poll completion, and share its
`Arc<Observation>` with consumers. `with_worker` exposes the execution half for a
host-managed task; `new` starts a standalone worker. Dropping the session cancels
and wakes its worker without joining on the UI thread.

## Identity and execution

Each session has a unique source identity. An observation key also contains the
canonical revision and language generation. A new request immediately hides the
old observation. The mailbox holds one active job, one newest pending job and
one completed result; typing cannot accumulate requests or completions. Parsing
and queries cooperate with cancellation. Results publish only for the admitted
key; retirement cannot recreate an attachment.

One primary tree supplies all three outputs. The worker materializes at most
8 MiB of source for query predicates and retained incremental input; bare CR is
normalized to LF without changing byte offsets. Incremental parsing compares the
retained worker snapshots to derive one conservative `InputEdit`, using LF rows
and byte columns. No diff runs on the UI thread. Reuse requires matching source,
language generation and a host-confirmed base revision with a complete journal.
Gaps, replacement invalidations and skipped bases parse fresh. Grouped edits and
undo/redo use the same path. All output queries rerun after a parse, covering
scope changes and injection boundaries as well as changed tree ranges.

Bundled text predicates and local-binding predicates apply to highlights.
Supported injections use included ranges, child exclusion and combined patterns;
Markdown inline and fenced code and HTML script/style have regression fixtures.
Unbundled injection languages keep the surrounding presentation. Injection trees
supply highlighting; outlines and fold rules describe the primary language.
Errors in source remain useful recoverable trees.

Highlights are resolved into disjoint source-ordered spans with binary visible-range
lookup. Outline entries form a flat forest whose parents precede their children;
IDs derive from declaration ancestry, kind, label and occurrence. All byte ranges
are UTF-8 validated before publication. Keep node borrows inside the lifetime of
the observation's owning tree. Labels are bounded to 160 characters.

## Selection and bounds

`language_name` keeps aliases, extensions, special filenames and injection
resolution with the selected language set. Hosts choose explicit language IDs
before filename detection; no file I/O or automatic document-language policy is
installed. Every selected language has highlight, fold and outline fixtures.
These are syntactic declarations, not an LSP symbol model: e.g. Dockerfile exposes
FROM stages, Make exposes targets/defines, and data formats expose keys/elements.
The current code-only Structure consumer uses those same observations.

States are Pending, Ready, Unsupported, Limited and Failed. Initial limits are:

| Bound | Limit |
| --- | --- |
| Source bytes | 8 MiB |
| Analysis duration | Five seconds, checked at parse/query/processing boundaries |
| Captures/folds and resolved highlight storage | 250,000 entries |
| Outline storage | 250,000 entries |
| Concurrent query matches | 8,192 |
| Injection depth | Three |
| Injection parses / source ranges | 4,096 / 8,192 |
| Cumulative injected bytes | 24 MiB |

Limits refuse the observation rather than exposing partial or stale clickable
ranges. Canonical text remains editable. Source/tree memory depends on grammar and
input; these are work/result bounds, not a hard allocator cap. Grammar compilation
is synchronous inside the worker and checks cancellation at its next boundary.

The runtime dependency is pinned `arborium-tree-sitter` 2.18.2: v1 already selects
its `links = "tree-sitter"` runtime, and Cargo permits one such runtime in the
workspace dependency graph. No Arborium umbrella or per-language crates enter a
v2-only dependency tree.

## Verification

```sh
cargo test --offline --locked -p bedtime-treesitter2
cargo test --offline --locked -p bedtime-treesitter2 analysis_measurement -- --ignored --nocapture
```

Verified 2026-10-19 on Apple M4 Pro, unoptimized workspace code: seven backend tests
passed, including all 30 fixtures and incremental/fresh comparisons. The explicit
94,000-byte Rust fixture (2,000 functions and macro injections) measured 211 ms
initial analysis, 171 ms incremental analysis, and 23.4 ms for 100,000 indexed span
lookups. Its standalone test process peaked at 26,902,528 bytes RSS while retaining
both observations. These measure complete background analysis and process memory,
not parser-only speed or native UI frame time; different contents can hit limits.
