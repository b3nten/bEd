# Tree sitter and bEd2 code structure plan

Status: implemented initial slice, 2026-10-19. The records below preserve the
accepted plan; current contracts and bounds live in the implementing READMEs:
[analysis](../bedtime2/crates/bedtime-treesitter2/README.md),
[grammar bundle](../bedtime2/crates/bedtime-grammars2/README.md),
[editor](../bedtime2/crates/bedtime-editor2/README.md), and
[Structure](../bed2/crates/bed-structure2/README.md).

Packaging revision accepted during implementation: use one vendored
`bedtime-grammars2` crate, compiling 30 native grammars plus Markdown inline.
Sources/queries remain pinned to the selected Arborium packages; no umbrella or
per-language Arborium dependencies enter v2 builds. Runtime installation is
outside this slice. The shared runtime remains pinned to v1's selected
`arborium-tree-sitter` because Cargo permits one `links = "tree-sitter"` runtime.

The implementation derives a conservative combined InputEdit from retained
worker snapshots and complete-journal admission. Primary parsing is incremental;
highlight/outline/fold queries rerun, and injection parses are fresh. Source
materialization/diffing happens only on the bounded worker. No new edit metadata
or buffer crate was needed. Initial limits and measured evidence are recorded in
the analysis README. Native Code/Structure acceptance passed eight captures;
file I/O and arbitrary-document protocols remain out of scope.

Follow-up accepted and implemented: restore Files as the application's way to open
real documents. Background loading, canonical file/save bindings, shared views
and visibility preferences are application-owned in `bed2/src/files.rs` and
`file_editor.rs`; code controls no longer create an in-memory sample. The current
[bEd2 contract](../bed2/README.md) records the file workflow. The original stages
below describe the scope before this follow-up.

Add shared Tree-sitter analysis through Arborium, integrate highlighting and
folding into the editor behind a feature, and add an application-owned Structure
panel for code documents. Structure will exercise editor contributions and public
navigation without introducing a protocol for arbitrary document types.

## Current foundation

[`bedtime-editor2`](../bedtime2/crates/bedtime-editor2/README.md) owns the indexed
UTF-8 buffer and editor views. `TextSession` exposes immutable snapshots and a
bounded change journal; Workbench integration is optional. Revision-bound
`Navigation`, `EditorView::request_navigation`, and `workbench::navigate_editor`
already provide external reveal, selection, and focus control. Syntax rendering
and folding remain unimplemented.

[`bed2`](../bed2/README.md) currently installs no document viewers, editor panels,
or file/save backends. This work needs a small code-editor panel to make the
integration usable. The existing [document contract](workbench-documents.md)
continues to govern canonical content, history, drafts, and retirement.

V1's [`bedtime-highlight`](../bedtime/crates/bedtime-highlight/src/grammars.rs)
provides capture mappings, grammar fixes, fold rules, and outline queries worth
carrying forward selectively. Its whole-snapshot highlighter and independent
parsing services are reference implementations, not the v2 execution model.

## Package boundaries

Use workspace package names with hyphens and Rust imports with underscores.

| Package and proposed location | Responsibility |
| --- | --- |
| `bedtime-grammars2`, under `bedtime2/crates/` | Vendored native parsers, scanners, upstream queries and notices in one crate |
| `bedtime-treesitter2`, under `bedtime2/crates/` | Grammar integration, source reading, parsing, and revision-bound highlights, fold regions, and code outlines |
| Existing `bedtime-editor2` | Optional `treesitter` feature, source adapter, shared analysis integration, syntax rendering, fold interaction, and public navigation |
| `bed-structure2`, under `bed2/crates/` | Application-owned code Structure panel, source tracking, and direct editor navigation |
| `bed2` application | Code-editor panel composition, document/language setup, and installation of Structure |

The dependency direction is `bedtime-editor2 -> bedtime-treesitter2` when the
feature is enabled. `bed-structure2` depends on both, plus Workbench and UI2.
Tree-sitter has no editor, Workbench, UI, filesystem, or application dependency.
The editor's existing `workbench` feature remains independent of `treesitter`.
Compiling a capability and installing its execution remain separate decisions.

Because `TextBuffer` lives in the editor, Tree-sitter cannot depend on it directly.
Define a small source-reading contract in `bedtime-treesitter2`, implemented for
the immutable editor buffer when the feature is enabled. It must support length,
byte-offset chunk access, and the source access needed for queries without
flattening the file. Retained source snapshots remain cheap shared roots. Resolve
the exact methods during the parsing spike; keep this contract specific to
Tree-sitter input. Moving the entire buffer into another crate is unnecessary.

The editor's plain build and standalone example continue to work without
Tree-sitter or Workbench. Add an opt-in syntax example for standalone embedding.

## State and contribution data

| State | Owner |
| --- | --- |
| Canonical text, revision, history, and change journal | Existing document/session |
| Language selection and analysis attachment | Document's editor integration |
| Parsers, trees, query caches, pending work, and observations | One analysis session per attached document |
| Selection, composition, wrapping, and collapsed regions | Each `EditorView` |
| Syntax palette | Editor presentation supplied by the host |
| Remembered source panel | One Structure state per workspace |
| Tree expansion and selected outline item | Each Structure panel |

Expose Tree-sitter-shaped observations: an owning immutable source/tree snapshot,
highlight captures with byte ranges, fold regions, and code outline entries.
Outline entries carry IDs, labels, kinds, parent relationships, declaration ranges,
and name ranges. Tree node borrows must never outlive their owning observation.
Consumers share observations rather than copying a tree or all spans per frame.

An observation identifies its source instance, document revision, and language
generation. The Workbench adapter associates that source with `DocumentId`.
Changing language invalidates prior observations even if the text revision is
unchanged. Rebinding a panel cannot make an old observation refer to its new
document. Validate results and UTF-8 ranges at publication; paint and interaction
code consume the accepted representation.

Highlighting, folding, and outlines share parsing and language knowledge but have
separate outputs. A function may contribute to all three; a foldable statement
block need not appear in Structure. Code outline queries belong with the grammar
integration. Structure consumes those results directly.

## Parsing and execution

Run parsing and query preparation off the UI thread. Each analysis session keeps
at most one active request and one newest pending request, so typing cannot build
an unbounded work queue. Workers read immutable snapshots and never edit documents.
Publication checks source identity, revision, language generation, and cancellation.

Retain trees and apply accepted edits for incremental parsing. The current journal
records byte ranges and inserted lengths in successive pre-change states; it does
not directly supply Tree-sitter's newline and byte-column points. First prove how
to obtain correct input edits from retained snapshots and journal data, including
grouped edits and undo/redo. A conservative combined replacement is acceptable
when derived correctly. Add minimal change metadata only if that proof requires
it. Do not diff or flatten an entire file on the UI thread. A journal gap or model
replacement requires a fresh parse.

Arborium 2.18.2's high-level highlighting path accepts a complete string and its
compiled-grammar parse path starts without an old tree. The first spike must
establish a lower-level path that retains incremental parsing while preserving
query predicates, locals, and embedded-language behavior. Reuse bundled grammars
and queries; verify any required adapter before building the UI around it.

Requery all potentially affected syntax context, including enclosing scopes and
injection boundaries. Tree-sitter changed ranges alone do not establish that every
previous highlight or outline entry is still correct.

Expose pending, ready, unsupported, limited, and failed analysis states. Resource
limits for source bytes, query work, injections, and result storage must be bounded
and documented after measurement. Large files remain editable when analysis is
limited. Never render old semantic ranges as if they describe a newer revision.

Start work lazily for attached documents with consumers. In Workbench, share the
attachment across viewers and tie worker cleanup to document/workspace lifecycle;
retire idle analysis when its last consumer detaches. Standalone hosts explicitly
own and drive the analysis session. Cancellation and shutdown must not block a UI
frame or leave results able to recreate retired attachments.

## Language selection

Start with the repository's pinned Arborium 2.18.2 and an explicit set of 30
language features with `default-features = false`:

| Area | Languages and formats |
| --- | --- |
| Systems and application development | Rust, C, C++, C#, Java, Go, Swift, Kotlin, Dart |
| Scripting | Python, Ruby, PHP, Lua, Bash, PowerShell |
| Web | JavaScript, TypeScript, TSX, HTML, CSS |
| Data and documents | JSON, YAML, TOML, XML, Markdown, SQL |
| Build and infrastructure | Dockerfile, CMake, Make, HCL |

Explicit language choice wins over filename detection. Keep aliases, extensions,
special filenames, and injection-language resolution together. Unsupported
injection languages retain the surrounding language's usable presentation.

All selected languages need a fixture exercising highlighting and their meaningful
fold/outline constructs. Empty results can be legitimate; record capability limits
per language instead of manufacturing symbols or folds. Include HTML script/style
and Markdown fenced-code fixtures. Port the existing Markdown inline supplement
and Kotlin adjustments only where fixtures establish the need. Account for helper
grammars separately from the 30 user-facing language selections.

Check the dependency tree for a v2-only build: v1's `all-languages` feature can
unify into a combined workspace build. Update grammar/query attribution alongside
the new dependency declaration.

## Editor highlighting and folding

Index accepted spans for visible byte-range lookup. Map captures to a small syntax
palette at drawing time, so theme changes do not require parsing. Syntax styles
initially change color while preserving the editor's existing text geometry.
Bound shaping and span processing for long lines and large visible fragments.

Fold regions are document-derived; collapsed state belongs to the view. Add gutter
controls, placeholders, and fold/unfold actions. Fold controls work with line
numbers hidden. One row mapping must drive folding, wrapping, painting, pointer
hits, selections, caret placement, and keyboard movement; rebuilding the entire
document's visible-row list every frame is unacceptable.

Map surviving collapsed regions through known edits, expanding regions whose
identity or boundaries become ambiguous. A journal gap clears uncertain state.
Selection, copy, and edits continue to use canonical source bytes. Navigation and
Find reveal targets inside folds. Fold changes never enter document undo history.
IME drafts stay local and do not become parse input before canonical commit.

## Structure source selection

Track one source per workspace, consisting of a panel and its current document
binding. Eligibility means an open code editor with supported Tree-sitter outline
analysis. A pending refresh retains that source but supplies no stale clickable
items. A valid empty outline is still an eligible observation.

Resolve source selection after Workbench focus and closure changes are visible:

1. If the currently focused panel is eligible, use it. Automatic tab selection
   after closure counts exactly like any other focus change.
2. Otherwise retain the remembered source if its panel and binding remain valid.
3. Otherwise clear the source and show **No structure available**.

Structure and unrelated panels do not replace an eligible remembered source.
There is no stack of past sources and no scan for another open code document.
Closing a source can establish a replacement only through ordinary current focus.
A different viewer of the same document is not a replacement merely because it
exists. If a focused panel's analysis becomes ready later, reevaluate eligibility.
Rebinding a remembered panel clears its old binding; current focus may establish
the new one. Retain workspace-local tracking across layout switches and retire it
with the workspace.

Use Workbench's existing focus APIs; no distinction between intentional and
automatic focus is needed. If no outline is ready, render the basic placeholder.
Diagnostic details remain available to inspection without adding a new panel flow.

## Structure activation and editor control

Structure captures the source panel, document binding, observation key, and outline
item at activation. Validate all of them before changing focus or selection. A
closed/rebound source, stale observation, missing item, or blocked navigation has
no activation effect. Do not reopen panels or choose another viewer.

For a valid activation, use the editor's public navigation path to reveal the name
range, select the name, and focus the source. Make its existing layout and tab
visible through Workbench, including when Structure retained a source from another
layout in the same workspace. Keep validation and application together so rejected
activation leaves layout, focus, caret, and selection unchanged.

Extend `Navigation` and `navigate_editor` rather than adding a separate command
bus. Keep navigation available with Tree-sitter disabled. Add a `scroll_to_line`
convenience that resolves a zero-based logical line against a supplied current
snapshot and uses the same revision-bound path. Reject invalid lines and stale
requests before mutation. Byte ranges remain the canonical coordinate system;
future debugger and LSP adapters convert their coordinates at their boundaries.

Preserve explicit reveal positioning, optional selection, and focus intent.
Reveal-only requests preserve the caret. Navigation expands enclosing folds and
uses the wrapping/folding row mapping. Existing composition gates stay
authoritative; an IME draft is never silently discarded to navigate.

## Implementation stages

1. **Prove parsing and ownership.** Add `bedtime-treesitter2` and the editor source
   adapter. Exercise Rust, Python, HTML, and Markdown; establish incremental input,
   query/injection correctness, cancellation, and shared observations. Set measured
   limits before considering the backend complete.
2. **Integrate editor contributions.** Add the optional feature, standalone syntax
   example, document-scoped Workbench bridge, palette, and viewport highlighting.
   Add the full selected language set and fixtures as the backend stabilizes.
3. **Implement folding.** Integrate row geometry, gutter interaction, placeholders,
   edit reconciliation, Find/reveal behavior, and public fold actions. Extend
   navigation and add the logical-line convenience independently of the feature.
4. **Compose the application panels.** Add a small `bed2` code-editor panel and
   `bed-structure2` package. Use explicitly created in-memory code documents to
   prove shared viewers and structure navigation. Filetree routing, file I/O,
   save backends, and restoring the removed Files demo remain separate work.
5. **Verify behavior and record the contract.** Exercise the scenarios below,
   document APIs and measured limits in the implementing READMEs, and update
   CONTEXT when the planned behavior becomes implemented.

## Verification and acceptance

Backend tests compare incremental results with fresh analysis after insertion,
deletion, grouped edits, undo/redo, language changes, and journal gaps. Include
Unicode, LF/CRLF/bare CR point mapping, malformed code, predicates/locals,
injections, stale worker completion, and cancellation/retirement. Syntax errors
in source must not make a recoverable tree unusable.

Editor tests cover independent folds in sibling views, wrapping and folding
together, hit testing, hidden line numbers, selections across folds, Find and
external reveal, theme changes, and composition gates. Recheck million-line and
long-line fixtures with analysis enabled and limited; record UI work separately
from worker duration, including peak analysis memory and result lookup costs.

Structure acceptance scenarios must include:

- Focus editor A, then editor B, then Structure or Terminal: B remains the source.
- Close focused B and automatically focus eligible A: A becomes the source.
- Close remembered B while Structure stays focused: show the placeholder even if
  A remains open. Focusing A afterward establishes it normally.
- Close B and automatically focus an ineligible panel: show the placeholder.
- Keep two viewers of one document: share analysis but target the actual source
  panel, with independent fold state.
- Activate a declaration in a hidden source tab or retained source layout: reveal
  that panel and unfold/scroll/select the declaration.
- Edit, rebind, or close the source between observation and activation: obsolete
  activation has no effect and never navigates another document.
- Navigate with a draft or invalid line: reject without changing view state.
- Show the placeholder with no eligible source, unsupported analysis, or no ready
  outline; a current eligible empty outline does not resurrect a previous source.

Run focused crate tests as each stage lands. The existing baseline commands are:

```sh
cargo test --offline --locked -p bedtime-editor2 --features workbench
cargo check --offline --locked -p bedtime-editor2 --no-default-features --example standalone
cargo test --offline --locked -p bedtime-workbench2 --no-default-features
```

Once the new packages/features exist, test the Tree-sitter backend and Structure
package, editor combinations `treesitter` and `workbench,treesitter`, a v2-only
dependency tree, and the bEd2 composition. Extend optional bEd2 automation for the
real focus, closure, and navigation scenarios; capture highlighting and fold UI
in a deliberate desktop session. These are implementation acceptance requirements,
not passing-run claims.
